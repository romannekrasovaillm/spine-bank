//! R1 (TASK §5): компоненты в много­модульных сборках. Корневой манифест
//! агрегатора (`pom.xml` с `<modules>`, `settings.gradle(.kts)` с `include(...)`,
//! `go.work`, `*.sln`, `package.json` с `workspaces`) НЕ является компонентом —
//! компоненты-кандидаты это модули сборки. Тестовые проекты
//! (`*/tests/*.csproj`, `src/test`, `__tests__`, `spec`) — не компоненты.
//!
//! Компонент = модуль сборки ⨝ юнит развёртывания: каталог модуля получает имя
//! сервиса из k8s `Deployment`/`StatefulSet`, `docker-compose` `services:`,
//! `skaffold.yaml` (`build.artifacts[].context` → образ). Сопоставление —
//! по контексту skaffold, базовому имени образа либо имени юнита. Юниты без
//! модуля (инфраструктура compose: zipkin/grafana/prometheus) компонентами не
//! становятся — иначе в граф просочились бы чужие сервисы.
//!
//! Разбор — только имеющимися зависимостями: YAML/JSON (`serde_yaml_ng`,
//! `serde_json`); XML/Groovy/`.sln`/`go.work` — текстовым разбором (`regex`).
//! Детерминизм (правило 10): множества отсортированы, обход карт —
//! `BTreeMap`/`BTreeSet`.

use std::collections::{BTreeMap, BTreeSet};

use regex::Regex;
use serde_json::Value as Json;
use serde_yaml_ng::Value as Yaml;

use super::snapshot::Snapshot;
use crate::survey::TEST_DIR_NAMES;

/// Нормализует относительный каталог: слэши `/`, без ведущего `./` и краевых
/// `/`; корень репозитория — пустая строка.
fn norm(dir: &str) -> String {
    let mut s = dir.trim().replace('\\', "/");
    while let Some(rest) = s.strip_prefix("./") {
        s = rest.to_string();
    }
    let trimmed = s.trim_matches('/').to_string();
    if trimmed == "." {
        String::new()
    } else {
        trimmed
    }
}

/// Склеивает каталог и относительный путь внутри него (абсолютный от корня,
/// если путь начинается с `/`).
fn join(base: &str, rel: &str) -> String {
    let rel = rel.replace('\\', "/");
    if let Some(abs) = rel.strip_prefix('/') {
        return norm(abs);
    }
    if base.is_empty() {
        norm(&rel)
    } else {
        norm(&format!("{base}/{rel}"))
    }
}

/// Каталог пути (пусто для файлов в корне репозитория).
fn dir_of(path: &str) -> String {
    path.rsplit_once('/')
        .map_or(String::new(), |(d, _)| d.to_string())
}

/// Последний сегмент пути.
fn basename(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// Путь лежит внутри каталога (или равен ему); пустой каталог — корень дерева.
fn under(path: &str, dir: &str) -> bool {
    if dir.is_empty() {
        return true;
    }
    path == dir || path.strip_prefix(dir).is_some_and(|r| r.starts_with('/'))
}

/// Каталог — тестовый (сегмент из [`TEST_DIR_NAMES`]).
fn is_test_dir(dir: &str) -> bool {
    !dir.is_empty() && dir.split('/').any(|seg| TEST_DIR_NAMES.contains(&seg))
}

/// Имя манифеста тестового проекта (`*.Tests.csproj` — `src/Cart.Tests.csproj`).
fn is_test_manifest(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.ends_with(".csproj") && (n.contains(".tests.") || n.contains(".test."))
}

/// Имя — решение Visual Studio (`*.sln`, регистронезависимо).
fn is_sln_manifest(name: &str) -> bool {
    std::path::Path::new(name)
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("sln"))
}

/// Планировка сборки репозитория: агрегаторы (не компоненты), объявленные
/// модули и каталоги тестовых проектов (не компоненты).
#[derive(Debug, Default)]
pub(crate) struct BuildLayout {
    /// Каталоги-агрегаторы (`<modules>`/`include`/`go.work`/`workspaces`/`.sln`).
    pub(crate) aggregators: BTreeSet<String>,
    /// Объявленные модули агрегаторов (относительные каталоги).
    pub(crate) modules: BTreeSet<String>,
    /// Каталоги тестовых проектов.
    pub(crate) tests: BTreeSet<String>,
}

impl BuildLayout {
    /// Каталог — агрегирующий корень (не компонент).
    pub(crate) fn is_aggregator(&self, dir: &str) -> bool {
        self.aggregators.contains(dir)
    }

    /// Каталог — тестовый проект (не компонент).
    pub(crate) fn is_test(&self, dir: &str) -> bool {
        self.tests.contains(dir)
    }
}

/// Разбирает планировку сборки снимка: агрегаторы, их модули, тестовые проекты.
pub(crate) fn build_layout(snap: &Snapshot) -> BuildLayout {
    let mut layout = BuildLayout::default();
    for path in &snap.files {
        let name = basename(path);
        let dir = dir_of(path);
        if is_test_dir(&dir) || is_test_manifest(name) {
            layout.tests.insert(dir.clone());
        }
        let Some(content) = snap.content(path) else {
            continue;
        };
        let declared: Vec<String> = if name == "pom.xml" {
            pom_modules(content)
        } else if name == "settings.gradle" || name == "settings.gradle.kts" {
            gradle_includes(content)
        } else if name == "go.work" {
            go_work_uses(content)
        } else if is_sln_manifest(name) {
            // `.sln` объявляет проекты путями ФАЙЛОВ (`src\cartservice.csproj`);
            // модуль сборки — каталог проекта (путь файла в модули не берём,
            // иначе тестовый проект `tests\*.tests.csproj` просочится в компонент).
            sln_projects(content)
                .into_iter()
                .map(|p| dir_of(&p))
                .collect()
        } else if name == "package.json" {
            npm_workspaces(content).unwrap_or_default()
        } else {
            Vec::new()
        };
        if declared.is_empty() {
            continue;
        }
        layout.aggregators.insert(dir.clone());
        for decl in declared {
            for d in expand(&dir, &decl, snap) {
                layout.modules.insert(d);
            }
        }
    }
    layout
}

/// Разворачивает объявление модуля в каталоги: обычный путь — как есть,
/// маска с `*` — каталоги первого уровня под префиксом.
fn expand(base: &str, decl: &str, snap: &Snapshot) -> Vec<String> {
    let full = join(base, decl);
    if !full.contains('*') {
        return vec![full];
    }
    let prefix = full
        .split('*')
        .next()
        .unwrap_or("")
        .trim_end_matches('/')
        .to_string();
    let mut out: BTreeSet<String> = BTreeSet::new();
    for path in &snap.files {
        let rest = if prefix.is_empty() {
            path.as_str()
        } else if let Some(r) = path.strip_prefix(&prefix) {
            r.trim_start_matches('/')
        } else {
            continue;
        };
        if let Some((seg, _)) = rest.split_once('/') {
            if seg.is_empty() {
                continue;
            }
            out.insert(if prefix.is_empty() {
                seg.to_string()
            } else {
                format!("{prefix}/{seg}")
            });
        }
    }
    out.into_iter().collect()
}

/// `<module>…</module>` из `pom.xml` (агрегатор Maven).
fn pom_modules(content: &str) -> Vec<String> {
    let re = Regex::new(r"<module>\s*([^<]+?)\s*</module>").expect("regex модулей Maven");
    re.captures_iter(content)
        .map(|c| c[1].trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Строки `include …` из `settings.gradle(.kts)`: `:a:b` → `a/b`.
fn gradle_includes(content: &str) -> Vec<String> {
    let quotes = Regex::new(r#"['"]([^'"]+)['"]"#).expect("regex строк Gradle");
    let mut out = Vec::new();
    for line in content.lines() {
        if !line.contains("include") {
            continue;
        }
        for cap in quotes.captures_iter(line) {
            let m = cap[1].trim_start_matches(':').replace(':', "/");
            if !m.is_empty() {
                out.push(m);
            }
        }
    }
    out
}

/// Каталоги `use …` из `go.work` (одиночная форма и блок `use ( … )`).
fn go_work_uses(content: &str) -> Vec<String> {
    let block = Regex::new(r"(?s)use\s*\(([^)]*)\)").expect("regex блока use");
    let single = Regex::new(r"(?m)^\s*use\s+(\S+)").expect("regex use");
    let mut out = Vec::new();
    for cap in block.captures_iter(content) {
        for tok in cap[1].split_whitespace() {
            if !tok.starts_with("//") {
                out.push(tok.to_string());
            }
        }
    }
    for cap in single.captures_iter(content) {
        let tok = cap[1].trim();
        if tok != "(" {
            out.push(tok.to_string());
        }
    }
    out
}

/// Пути проектов из `*.sln` (`Project("{GUID}") = "name", "path.csproj"`).
fn sln_projects(content: &str) -> Vec<String> {
    let re = Regex::new(r#"Project\("\{[^}]*\}"\)\s*=\s*"[^"]*",\s*"([^"]+)""#)
        .expect("regex проектов sln");
    re.captures_iter(content)
        .map(|c| c[1].replace('\\', "/"))
        .collect()
}

/// Паттерны `workspaces` из `package.json` (массив либо `{ packages: […] }`).
fn npm_workspaces(content: &str) -> Option<Vec<String>> {
    let v: Json = serde_json::from_str(content).ok()?;
    let ws = v.get("workspaces")?;
    let arr = match ws {
        Json::Array(a) => a.as_slice(),
        Json::Object(o) => o.get("packages")?.as_array()?.as_slice(),
        _ => return None,
    };
    let pats: Vec<String> = arr
        .iter()
        .filter_map(|x| x.as_str().map(str::to_string))
        .collect();
    if pats.is_empty() { None } else { Some(pats) }
}

/// Юнит развёртывания: имя сервиса и (если задан) базовое имя образа.
#[derive(Debug, Clone)]
pub(crate) struct DeployUnit {
    /// Имя юнита (`Deployment.metadata.name` / ключ `services:`).
    pub(crate) name: String,
    /// Базовое имя образа без реестра и тега (`gcr.io/x/adservice:1 → adservice`).
    pub(crate) image_base: Option<String>,
}

/// Топология развёртывания снимка: юниты и контексты сборки skaffold.
#[derive(Debug, Default)]
pub(crate) struct DeployTopology {
    /// Юниты k8s/compose (отсортированы по имени).
    pub(crate) units: Vec<DeployUnit>,
    /// skaffold: контекст сборки → базовое имя образа.
    pub(crate) contexts: BTreeMap<String, String>,
}

impl DeployTopology {
    /// Имя юнита по базовому имени образа.
    fn unit_by_image(&self, image_base: &str) -> Option<&str> {
        self.units
            .iter()
            .find(|u| u.image_base.as_deref() == Some(image_base))
            .map(|u| u.name.as_str())
    }
}

/// Топология развёртывания снимка: k8s `Deployment`/`StatefulSet`
/// (`kubernetes-manifests/*.yaml`), `docker-compose` `services:`, skaffold
/// `build.artifacts`. Один файл — от одного до нескольких YAML-документов.
pub(crate) fn deploy_topology(snap: &Snapshot) -> DeployTopology {
    let mut topo = DeployTopology::default();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for (path, content) in &snap.contents {
        let ext = std::path::Path::new(path)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        if ext != "yaml" && ext != "yml" {
            continue;
        }
        let dir = dir_of(path);
        for doc in split_docs(content) {
            let Ok(v) = serde_yaml_ng::from_str::<Yaml>(doc) else {
                continue;
            };
            let Some(map) = v.as_mapping() else {
                continue;
            };
            if let Some(arts) = map
                .get("build")
                .and_then(|b| b.get("artifacts"))
                .and_then(Yaml::as_sequence)
            {
                for a in arts {
                    let img = a.get("image").and_then(Yaml::as_str);
                    let ctx = a.get("context").and_then(Yaml::as_str);
                    if let (Some(img), Some(ctx)) = (img, ctx) {
                        topo.contexts.insert(join(&dir, ctx), image_base(img));
                    }
                }
            }
            let kind = map.get("kind").and_then(Yaml::as_str);
            match kind {
                Some("Deployment" | "StatefulSet") => {
                    let name = map
                        .get("metadata")
                        .and_then(|m| m.get("name"))
                        .and_then(Yaml::as_str);
                    if let Some(name) = name {
                        let unit = DeployUnit {
                            name: name.to_string(),
                            image_base: first_container_image(&v).map(image_base),
                        };
                        if seen.insert(unit.name.clone()) {
                            topo.units.push(unit);
                        }
                    }
                }
                None => {
                    if let Some(services) = map.get("services").and_then(Yaml::as_mapping) {
                        for (k, svc) in services {
                            let Some(name) = k.as_str() else { continue };
                            if !seen.insert(name.to_string()) {
                                continue;
                            }
                            let image_base =
                                svc.get("image").and_then(Yaml::as_str).map(image_base);
                            topo.units.push(DeployUnit {
                                name: name.to_string(),
                                image_base,
                            });
                        }
                    }
                }
                _ => {}
            }
        }
    }
    topo.units.sort_by(|a, b| a.name.cmp(&b.name));
    topo
}

/// Образ первого контейнера пода (`spec.template.spec.containers[].image`).
fn first_container_image(doc: &Yaml) -> Option<&str> {
    doc.get("spec")?
        .get("template")?
        .get("spec")?
        .get("containers")?
        .as_sequence()?
        .iter()
        .find_map(|c| c.get("image").and_then(Yaml::as_str))
}

/// Базовое имя образа: без digest, тега и реестра.
fn image_base(image: &str) -> String {
    let no_digest = image.split('@').next().unwrap_or(image);
    let after_slash = no_digest.rfind('/').map_or(0, |i| i + 1);
    let tag_pos = no_digest[after_slash..].find(':').map(|i| after_slash + i);
    let name = tag_pos.map_or(no_digest, |p| &no_digest[..p]);
    name.rsplit('/').next().unwrap_or(name).to_string()
}

/// Имя компонента для каталога модуля: юнит развёртывания по контексту
/// skaffold / базовому имени образа / имени юнита; иначе — имя каталога.
pub(crate) fn component_name(dir: &str, topo: &DeployTopology) -> String {
    if let Some(img) = topo.contexts.get(dir) {
        return topo
            .unit_by_image(img)
            .map_or_else(|| img.clone(), str::to_string);
    }
    let base = basename(dir);
    if let Some(name) = topo.unit_by_image(base) {
        return name.to_string();
    }
    if let Some(u) = topo.units.iter().find(|u| u.name == base) {
        return u.name.clone();
    }
    if dir.is_empty() {
        "(корень репозитория)".to_string()
    } else {
        base.to_string()
    }
}

/// Каталог содержит файлы снимка (кандидат реально существует в ревизии).
pub(crate) fn dir_present(snap: &Snapshot, dir: &str) -> bool {
    snap.files.iter().any(|f| under(f, dir))
}

/// Режет текст на YAML-документы по строкам-разделителям `---`.
fn split_docs(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0usize;
    let mut idx = 0usize;
    for line in text.split_inclusive('\n') {
        if line.trim() == "---" {
            out.push(&text[start..idx]);
            start = idx + line.len();
        }
        idx += line.len();
    }
    out.push(&text[start..]);
    out.into_iter().filter(|d| !d.trim().is_empty()).collect()
}

#[cfg(test)]
mod tests {
    use super::super::snapshot::tests::{git_repo, write_file};
    use super::*;

    fn snapshot(dir: &std::path::Path) -> Snapshot {
        git_repo(dir);
        super::super::snapshot::snapshot_at(dir, "HEAD").expect("снимок")
    }

    /// Maven: корневой `pom.xml` с `<modules>` — агрегатор; объявленные модули
    /// попадают в набор модулей и не являются компонентами-агрегаторами.
    #[test]
    fn maven_aggregator_root_excluded() {
        let dir = tempfile::tempdir().expect("tmp");
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).expect("mkdir");
        write_file(
            &repo,
            "pom.xml",
            "<project><modules>\n<module>svc-a</module>\n<module>svc-b</module>\n</modules></project>\n",
        );
        write_file(&repo, "svc-a/pom.xml", "<project/>\n");
        write_file(&repo, "svc-b/pom.xml", "<project/>\n");
        let snap = snapshot(&repo);
        let layout = build_layout(&snap);
        assert!(layout.is_aggregator(""), "{:?}", layout.aggregators);
        assert!(layout.modules.contains("svc-a"), "{:?}", layout.modules);
        assert!(layout.modules.contains("svc-b"), "{:?}", layout.modules);
        assert!(!layout.is_aggregator("svc-a"));
    }

    /// Gradle: `include ':lib:core'` — агрегатор и модуль `lib/core`.
    #[test]
    fn gradle_include_detected() {
        let dir = tempfile::tempdir().expect("tmp");
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).expect("mkdir");
        write_file(
            &repo,
            "settings.gradle",
            "rootProject.name = 'x'\ninclude 'app', ':lib:core'\n",
        );
        write_file(&repo, "app/build.gradle", "plugins {}\n");
        write_file(&repo, "lib/core/build.gradle", "plugins {}\n");
        let snap = snapshot(&repo);
        let layout = build_layout(&snap);
        assert!(layout.is_aggregator(""), "{:?}", layout.aggregators);
        assert!(layout.modules.contains("app"), "{:?}", layout.modules);
        assert!(layout.modules.contains("lib/core"), "{:?}", layout.modules);
    }

    /// `go.work`: `use ./svc/a` — агрегатор и модуль `svc/a`.
    #[test]
    fn go_work_use_detected() {
        let dir = tempfile::tempdir().expect("tmp");
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).expect("mkdir");
        write_file(&repo, "go.work", "go 1.24\nuse ./svc/a\n");
        write_file(&repo, "svc/a/go.mod", "module a\n");
        let snap = snapshot(&repo);
        let layout = build_layout(&snap);
        assert!(layout.is_aggregator(""), "{:?}", layout.aggregators);
        assert!(layout.modules.contains("svc/a"), "{:?}", layout.modules);
    }

    /// `.sln` объявляет проекты ПУТЯМИ ФАЙЛОВ (`src\cartservice.csproj`);
    /// модуль сборки — каталог проекта, а не путь файла. Тестовый проект
    /// (`tests\cartservice.tests.csproj`) — в тестах, не в модулях
    /// (регресс: путь файла просачивался кандидатом в компонент).
    #[test]
    fn sln_module_is_project_directory() {
        let dir = tempfile::tempdir().expect("tmp");
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).expect("mkdir");
        write_file(
            &repo,
            "cartservice.sln",
            "Project(\"{FAE04EC0-301F-11D3-BF4B-00C04F79EFBC}\") = \"cartservice\", \"src\\cartservice.csproj\", \"{2348C29F}\"\nEndProject\nProject(\"{FAE04EC0-301F-11D3-BF4B-00C04F79EFBC}\") = \"cartservice.tests\", \"tests\\cartservice.tests.csproj\", \"{59825342}\"\nEndProject\n",
        );
        write_file(&repo, "src/cartservice.csproj", "<Project/>\n");
        write_file(&repo, "tests/cartservice.tests.csproj", "<Project/>\n");
        let snap = snapshot(&repo);
        let layout = build_layout(&snap);
        assert!(layout.is_aggregator(""), "{:?}", layout.aggregators);
        assert!(layout.modules.contains("src"), "{:?}", layout.modules);
        assert!(layout.modules.contains("tests"), "{:?}", layout.modules);
        assert!(
            !layout.modules.iter().any(|m| m.ends_with(".csproj")),
            "в модулях не должно быть путей файлов: {:?}",
            layout.modules
        );
        assert!(layout.is_test("tests"), "{:?}", layout.tests);
    }

    /// Тестовый проект `*/tests/*.csproj` — не компонент.
    #[test]
    fn test_project_excluded() {
        let dir = tempfile::tempdir().expect("tmp");
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).expect("mkdir");
        write_file(
            &repo,
            "src/cartservice/tests/cartservice.tests.csproj",
            "<Project/>\n",
        );
        write_file(
            &repo,
            "src/cartservice/src/cartservice.csproj",
            "<Project/>\n",
        );
        let snap = snapshot(&repo);
        let layout = build_layout(&snap);
        assert!(
            layout.is_test("src/cartservice/tests"),
            "{:?}",
            layout.tests
        );
        assert!(!layout.is_test("src/cartservice/src"));
    }

    /// Юниты развёртывания: k8s `Deployment` и `compose services:` с образами.
    #[test]
    fn deploy_units_from_k8s_and_compose() {
        let dir = tempfile::tempdir().expect("tmp");
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).expect("mkdir");
        write_file(
            &repo,
            "k8s/customers.yaml",
            "apiVersion: apps/v1\nkind: Deployment\nmetadata:\n  name: customers-service\nspec:\n  template:\n    spec:\n      containers:\n      - name: app\n        image: acme/customers-service:1.2\n",
        );
        write_file(
            &repo,
            "docker-compose.yml",
            "services:\n  gateway:\n    image: acme/gateway:latest\n  zipkin:\n    image: openzipkin/zipkin\n",
        );
        let snap = snapshot(&repo);
        let topo = deploy_topology(&snap);
        let names: Vec<&str> = topo.units.iter().map(|u| u.name.as_str()).collect();
        assert!(names.contains(&"customers-service"), "{names:?}");
        assert!(names.contains(&"gateway"), "{names:?}");
        assert!(names.contains(&"zipkin"), "{names:?}");
        let cust = topo
            .units
            .iter()
            .find(|u| u.name == "customers-service")
            .expect("юнит");
        assert_eq!(cust.image_base.as_deref(), Some("customers-service"));
    }

    /// Сопряжение: каталог модуля Maven и compose-юнит по базовому имени образа.
    #[test]
    fn join_by_image_base() {
        let dir = tempfile::tempdir().expect("tmp");
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).expect("mkdir");
        write_file(
            &repo,
            "spring-petclinic-customers-service/pom.xml",
            "<project/>\n",
        );
        write_file(
            &repo,
            "docker-compose.yml",
            "services:\n  customers-service:\n    image: springcommunity/spring-petclinic-customers-service\n",
        );
        let snap = snapshot(&repo);
        let topo = deploy_topology(&snap);
        assert_eq!(
            component_name("spring-petclinic-customers-service", &topo),
            "customers-service"
        );
    }

    /// Сопряжение по контексту skaffold: каталог `…/src` → образ → юнит.
    #[test]
    fn join_by_skaffold_context() {
        let dir = tempfile::tempdir().expect("tmp");
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).expect("mkdir");
        write_file(
            &repo,
            "src/cartservice/src/cartservice.csproj",
            "<Project/>\n",
        );
        write_file(
            &repo,
            "skaffold.yaml",
            "build:\n  artifacts:\n  - image: cartservice\n    context: src/cartservice/src\n",
        );
        write_file(
            &repo,
            "kubernetes-manifests/cartservice.yaml",
            "apiVersion: apps/v1\nkind: Deployment\nmetadata:\n  name: cartservice\nspec:\n  template:\n    spec:\n      containers:\n      - name: server\n        image: cartservice\n",
        );
        let snap = snapshot(&repo);
        let topo = deploy_topology(&snap);
        assert_eq!(
            topo.contexts.get("src/cartservice/src").map(String::as_str),
            Some("cartservice")
        );
        assert_eq!(component_name("src/cartservice/src", &topo), "cartservice");
    }
}
