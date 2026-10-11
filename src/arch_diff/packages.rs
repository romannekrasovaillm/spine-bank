//! R4 (0.4.0): пакетный уровень для монолитов — кластеры пакетов одного
//! модуля сборки.
//!
//! Когда в ревизии ровно один модуль сборки (`layout.modules` пуст, см.
//! [`super::modules::build_layout`]), компонент «по факту» не должен
//! вырождаться в корень репозитория: граф разбирает репозиторий на кластеры
//! пакетов, а рёбра между ними дают обычные импорты
//! ([`crate::imports`], с B4/B5 — включая wildcard `import a.b.*`).
//!
//! Эвристика (авто; конфигурируемый `component_globs` — отдельным слоем):
//! общий базовый пакет (JVM: длиннейший общий каталог под
//! `src/main/java|kotlin`; Python: верхний пакет с `__init__.py`), затем
//! рекурсивный спуск от его детей — каталог становится кластером, если
//! содержит исходники напрямую ЛИБО ветвится на ≥2 каталога с исходниками.
//! Это даёт «первый уровень под базовым пакетом» для плоских монолитов
//! (library → `catalogue`/`commons`/`lending`) и корректную глубину для
//! гексагона (buckpal → `application.domain`, `adapter.in.web`, …).
//!
//! Детерминизм (правило 10 волны K): обход `BTreeSet`/`BTreeMap`, никакой
//! зависимости от порядка файлов; повторный прогон на том же снимке даёт те
//! же кластеры.

use std::collections::{BTreeMap, BTreeSet};

use super::snapshot::Snapshot;
use crate::survey::TEST_DIR_NAMES;

/// Кластер пакетов → узел-компонент пакетного уровня.
pub(crate) struct PackageCluster {
    /// Стабильный id узла (`dir:<исходный каталог>`).
    pub(crate) id: String,
    /// Точечное имя пакета относительно базового (`application.domain`).
    pub(crate) title: String,
    /// Корни кода: исходный каталог (владение файлами) + координата пакета
    /// (разрешение импортов) — тот же приём, что у `code_roots` модели.
    pub(crate) roots: Vec<String>,
}

/// Известные JVM-префиксы исходников (длинные вперёд: `app/src/main/java`
/// перекрывает хвост `src/main/java`).
const JVM_SOURCE_PREFIXES: &[&[&str]] = &[
    &["app", "src", "main", "java"],
    &["src", "main", "java"],
    &["src", "main", "kotlin"],
];

/// Кластеры пакетов снимка одномодульного репозитория. Пусто — кластеризовать
/// нечего (нет пакетной структуры или единственный пакет).
pub(crate) fn clusters(snap: &Snapshot) -> Vec<PackageCluster> {
    let mut found: Vec<PackageCluster> = Vec::new();
    found.extend(jvm_clusters(snap));
    found.extend(python_clusters(snap));
    // Слияние кластеров с одинаковым именем (Java и Kotlin одного пакета
    // лежат в разных srcroot): корни объединяются, id — минимальный.
    let mut by_title: BTreeMap<String, PackageCluster> = BTreeMap::new();
    for c in found {
        by_title
            .entry(c.title.clone())
            .and_modify(|e| {
                e.roots.extend(c.roots.iter().cloned());
                if c.id < e.id {
                    e.id.clone_from(&c.id);
                }
            })
            .or_insert(c);
    }
    let mut out: Vec<PackageCluster> = by_title.into_values().collect();
    for c in &mut out {
        c.roots.sort();
        c.roots.dedup();
    }
    // ids уникальны после слияния (разные имена → разные каталоги); на всякий
    // случай сохраняем инвариант детерминированной устойчивостью.
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out.dedup_by(|a, b| a.id == b.id);
    out
}

/// JVM-кластеры: по каждому исходному корню (`src/main/java|kotlin`) —
/// общий базовый пакет, затем спуск.
fn jvm_clusters(snap: &Snapshot) -> Vec<PackageCluster> {
    // srcroot → каталоги (относительно srcroot), содержащие исходники.
    let mut groups: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for path in &snap.files {
        let Some((srcroot, dir)) = jvm_source_dir(path) else {
            continue;
        };
        if dir.is_empty() {
            continue; // файлы пакета по умолчанию — вне кластеров
        }
        groups.entry(srcroot).or_default().insert(dir);
    }
    let mut out = Vec::new();
    for (srcroot, dirs) in groups {
        let base = common_dir_prefix(dirs.iter().map(String::as_str));
        for dir in resolve_clusters(&base, &dirs) {
            let title = strip_prefix_dir(&dir, &base).replace('/', ".");
            let root_dir = format!("{srcroot}{dir}");
            out.push(PackageCluster {
                id: format!("dir:{root_dir}"),
                title,
                roots: vec![root_dir, dir],
            });
        }
    }
    out
}

/// `(srcroot, каталог пакета)` для JVM-файла по его репозиторному пути;
/// `None` — не JVM или нестандартная раскладка (нет `src/main/java|kotlin`).
fn jvm_source_dir(path: &str) -> Option<(String, String)> {
    let ext = path.rsplit('.').next().unwrap_or_default();
    if ext != "java" && ext != "kt" {
        return None;
    }
    let segs: Vec<&str> = path.split('/').collect();
    for prefix in JVM_SOURCE_PREFIXES {
        if let Some(i) = find_subslice(&segs, prefix) {
            let srcroot = format!("{}/", segs[..i + prefix.len()].join("/"));
            let rest = &segs[i + prefix.len()..];
            // Последний сегмент — имя файла; каталог = префикс без него.
            let dir = rest[..rest.len().saturating_sub(1)].join("/");
            return Some((srcroot, dir));
        }
    }
    None
}

/// Python-кластеры: верхний пакет (каталог с `__init__.py` без `__init__.py`
/// в родителе), затем тот же спуск. Несколько верхних пакетов или ни одного —
/// не кластеризуем (неоднозначно / не пакетная структура).
fn python_clusters(snap: &Snapshot) -> Vec<PackageCluster> {
    let is_test = |dir: &str| TEST_DIR_NAMES.iter().any(|t| has_segment(dir, t));
    let packages: BTreeSet<String> = snap
        .files
        .iter()
        .filter_map(|p| {
            let (d, n) = p.rsplit_once('/')?;
            (n == "__init__.py" && !is_test(d)).then(|| d.to_string())
        })
        .collect();
    if packages.is_empty() {
        return Vec::new();
    }
    let top: Vec<&String> = packages
        .iter()
        .filter(|p| !packages.contains(&parent_dir(p)))
        .collect();
    if top.len() != 1 {
        return Vec::new();
    }
    let base = top[0].clone();
    let mut dirs: BTreeSet<String> = BTreeSet::new();
    for path in &snap.files {
        if path.rsplit('.').next() != Some("py") {
            continue;
        }
        let Some((d, _)) = path.rsplit_once('/') else {
            continue;
        };
        if is_test(d) {
            continue;
        }
        if d == base || d.starts_with(&format!("{base}/")) {
            dirs.insert(d.to_string());
        }
    }
    resolve_clusters(&base, &dirs)
        .into_iter()
        .map(|dir| {
            let title = strip_prefix_dir(&dir, &base).replace('/', ".");
            PackageCluster {
                id: format!("dir:{dir}"),
                title,
                roots: vec![dir],
            }
        })
        .collect()
}

/// Общий (по сегментам) каталог-префикс набора каталогов; пусто — общего нет.
fn common_dir_prefix<'a>(dirs: impl Iterator<Item = &'a str>) -> String {
    let lists: Vec<Vec<&str>> = dirs.map(|d| d.split('/').collect()).collect();
    let Some(first) = lists.first() else {
        return String::new();
    };
    let mut common: Vec<&str> = first.clone();
    for l in &lists[1..] {
        let n = common.iter().zip(l).take_while(|(a, b)| a == b).count();
        common.truncate(n);
    }
    common.join("/")
}

/// Спуск от детей `base`: каталог — кластер, если содержит исходники прямо
/// или ветвится на ≥2 каталога с исходниками; иначе спускаемся дальше.
fn resolve_clusters(base: &str, direct: &BTreeSet<String>) -> BTreeSet<String> {
    let mut children: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for d in direct {
        if d == base {
            continue;
        }
        let mut cur = d.clone();
        loop {
            let parent = parent_dir(&cur);
            children
                .entry(parent.clone())
                .or_default()
                .insert(cur.clone());
            if parent == base || parent.is_empty() {
                break;
            }
            cur = parent;
        }
    }
    let mut out = BTreeSet::new();
    if let Some(kids) = children.get(base) {
        for k in kids {
            resolve_one(k, direct, &children, &mut out);
        }
    }
    out
}

/// Один шаг спуска (см. [`resolve_clusters`]).
fn resolve_one(
    dir: &str,
    direct: &BTreeSet<String>,
    children: &BTreeMap<String, BTreeSet<String>>,
    out: &mut BTreeSet<String>,
) {
    if direct.contains(dir) {
        out.insert(dir.to_string());
        return;
    }
    let Some(kids) = children.get(dir) else {
        return;
    };
    let kids_direct = kids.iter().filter(|k| direct.contains(*k)).count();
    if kids_direct >= 2 {
        out.insert(dir.to_string());
        return;
    }
    for k in kids {
        resolve_one(k, direct, children, out);
    }
}

/// Родительский каталог (пусто у верхнего уровня).
fn parent_dir(dir: &str) -> String {
    dir.rsplit_once('/')
        .map_or_else(String::new, |(p, _)| p.to_string())
}

/// `dir` без префикса `base` (по сегментной границе).
fn strip_prefix_dir<'a>(dir: &'a str, base: &str) -> &'a str {
    if base.is_empty() {
        return dir;
    }
    dir.strip_prefix(base)
        .map_or(dir, |r| r.trim_start_matches('/'))
}

/// Есть ли `want` среди сегментов пути `path`.
fn has_segment(path: &str, want: &str) -> bool {
    path.split('/').any(|s| s == want)
}

/// Индекс первого вхождения подпоследовательности-префикса `needle` в `hay`.
fn find_subslice(hay: &[&str], needle: &[&str]) -> Option<usize> {
    if needle.is_empty() || needle.len() > hay.len() {
        return None;
    }
    (0..=hay.len() - needle.len()).find(|&i| hay[i..i + needle.len()] == *needle)
}

#[cfg(test)]
mod tests {
    use super::super::snapshot::Snapshot;
    use super::*;
    use std::collections::{BTreeMap, BTreeSet};

    fn snap(paths: &[&str]) -> Snapshot {
        Snapshot {
            rev: "TEST".to_string(),
            files: paths.iter().map(|s| (*s).to_string()).collect(),
            contents: BTreeMap::new(),
        }
    }

    /// Много файлов в одних каталогах — компактно: `(каталог, n)`.
    fn files_in(dirs: &[(&str, usize)], ext: &str) -> Vec<String> {
        let mut out = Vec::new();
        for (d, n) in dirs {
            for i in 0..*n {
                out.push(format!("{d}/F{i}.{ext}"));
            }
        }
        out
    }

    /// Гексагон (buckpal-подобный): базовый пакет = `io/reflectoring/buckpal`.
    fn buckpal() -> Snapshot {
        let dirs = [
            ("src/main/java/io/reflectoring/buckpal", 3),
            ("src/main/java/io/reflectoring/buckpal/adapter/in/web", 1),
            (
                "src/main/java/io/reflectoring/buckpal/adapter/out/persistence",
                7,
            ),
            (
                "src/main/java/io/reflectoring/buckpal/application/domain/model",
                4,
            ),
            (
                "src/main/java/io/reflectoring/buckpal/application/domain/service",
                4,
            ),
            (
                "src/main/java/io/reflectoring/buckpal/application/port/in",
                5,
            ),
            (
                "src/main/java/io/reflectoring/buckpal/application/port/out",
                3,
            ),
            ("src/main/java/io/reflectoring/buckpal/common", 3),
            ("src/main/java/io/reflectoring/buckpal/common/validation", 1),
        ];
        let paths = files_in(&dirs, "java");
        snap(&paths.iter().map(String::as_str).collect::<Vec<_>>())
    }

    #[test]
    fn buckpal_clusters_match_archunit_zones() {
        let got: BTreeSet<String> = clusters(&buckpal()).into_iter().map(|c| c.title).collect();
        let want: BTreeSet<String> = [
            "application.domain",
            "application.port",
            "adapter.in.web",
            "adapter.out.persistence",
            "common",
        ]
        .iter()
        .map(|s| (*s).to_string())
        .collect();
        assert_eq!(got, want);
    }

    #[test]
    fn buckpal_roots_cover_file_and_package_coords() {
        let c = clusters(&buckpal())
            .into_iter()
            .find(|c| c.title == "application.domain")
            .expect("кластер application.domain");
        assert_eq!(
            c.roots,
            vec![
                "io/reflectoring/buckpal/application/domain".to_string(),
                "src/main/java/io/reflectoring/buckpal/application/domain".to_string(),
            ]
        );
    }

    #[test]
    fn flat_packages_first_level_under_base() {
        // library-подобный: первый уровень под базовым пакетом.
        let dirs = [
            ("src/main/java/io/pillopl/library", 1),
            ("src/main/java/io/pillopl/library/catalogue", 10),
            ("src/main/java/io/pillopl/library/commons/aggregates", 2),
            ("src/main/java/io/pillopl/library/commons/commands", 2),
            ("src/main/java/io/pillopl/library/commons/events", 3),
            (
                "src/main/java/io/pillopl/library/commons/events/publisher",
                5,
            ),
            ("src/main/java/io/pillopl/library/lending", 2),
            ("src/main/java/io/pillopl/library/lending/book/model", 7),
        ];
        let files = files_in(&dirs, "java");
        let got: BTreeSet<String> =
            clusters(&snap(&files.iter().map(String::as_str).collect::<Vec<_>>()))
                .into_iter()
                .map(|c| c.title)
                .collect();
        let want: BTreeSet<String> = ["catalogue", "commons", "lending"]
            .iter()
            .map(|s| (*s).to_string())
            .collect();
        assert_eq!(got, want);
    }

    #[test]
    fn single_package_is_not_clustered() {
        // Один пакет без ветвления — кластеров нет (компонент остаётся один).
        let files = files_in(&[("src/main/java/hipstershop", 3)], "java");
        let s = snap(&files.iter().map(String::as_str).collect::<Vec<_>>());
        assert!(clusters(&s).is_empty());
    }

    #[test]
    fn python_top_package_children_clustered() {
        let paths = [
            "myapp/__init__.py",
            "myapp/api/__init__.py",
            "myapp/api/handler.py",
            "myapp/core/__init__.py",
            "myapp/core/model.py",
        ];
        let got: BTreeSet<String> = clusters(&snap(&paths))
            .into_iter()
            .map(|c| c.title)
            .collect();
        let want: BTreeSet<String> = ["api", "core"].iter().map(|s| (*s).to_string()).collect();
        assert_eq!(got, want);
    }

    #[test]
    fn determinism_byte_for_byte() {
        let a: Vec<String> = clusters(&buckpal()).into_iter().map(|c| c.id).collect();
        let b: Vec<String> = clusters(&buckpal()).into_iter().map(|c| c.id).collect();
        assert_eq!(a, b);
    }

    #[test]
    fn common_prefix_and_strip() {
        assert_eq!(
            common_dir_prefix(["a/b/c", "a/b/d/e"].into_iter()),
            "a/b".to_string()
        );
        assert_eq!(strip_prefix_dir("a/b/c", "a/b"), "c");
        assert_eq!(strip_prefix_dir("top", ""), "top");
    }
}
