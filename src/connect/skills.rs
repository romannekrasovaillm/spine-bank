//! Доставка скиллов в хост (B1): сбор файлов из встроенных плагинов и из
//! библиотеки пользователя на диске (`[plugins].dirs`), слияние источников
//! (диск приоритетен), раскладка с отчётом по исходам.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use super::types::{ConnectReport, SkillAction, SkillOutcome};
use crate::error::{HarnessError, Result};

/// Потолок размера одного файла скилла при копировании (байт): защита от
/// случайного переноса тяжёлых вложений в проект хоста.
const MAX_SKILL_FILE_BYTES: usize = 200 * 1024;

/// Гарантирует минимальный frontmatter SKILL.md (agent-plugins.org): если
/// шапки `---` с ключами `name:`/`description:` нет — синтезирует из имени
/// каталога и первой непустой строки тела. Все встроенные скиллы шапку уже
/// имеют (охраняется тестом `assets.rs`) — это страховка на будущее.
fn ensure_frontmatter(content: &str, fallback_name: &str) -> String {
    let mut has_name = false;
    let mut has_description = false;
    if content.starts_with("---") {
        for line in content.lines().skip(1) {
            if line.trim() == "---" {
                break;
            }
            if line.starts_with("name:") {
                has_name = true;
            }
            if line.starts_with("description:") {
                has_description = true;
            }
        }
    }
    if has_name && has_description {
        return content.to_string();
    }
    let description = content
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map_or_else(
            || "Скилл библиотеки Spine".to_string(),
            |l| l.trim_start_matches('#').trim().chars().take(160).collect(),
        );
    format!("---\nname: {fallback_name}\ndescription: {description}\n---\n\n{content}")
}

/// Собирает файлы скиллов из встроенных плагинов: «относительный путь
/// назначения внутри skills-root → содержимое». Файлы крупнее
/// [`MAX_SKILL_FILE_BYTES`] пропускаются с заметкой; дубли пути назначения
/// (одно имя скилла в двух плагинах) — тоже (побеждает первый).
fn collect_skill_files_from(files: &[(&str, &str)]) -> (Vec<(PathBuf, String)>, Vec<String>) {
    let mut out: Vec<(PathBuf, String)> = Vec::new();
    let mut skipped = Vec::new();
    let mut seen: BTreeSet<PathBuf> = BTreeSet::new();
    for (rel, content) in files {
        let segs: Vec<&str> = rel.split('/').collect();
        // plugins/<plugin>/skills/<skill>/<файл…>
        if segs.len() < 5 || segs[0] != "plugins" || segs[2] != "skills" {
            continue;
        }
        if content.len() > MAX_SKILL_FILE_BYTES {
            skipped.push(format!(
                "{rel}: {} КБ — больше потолка {} КБ",
                content.len() / 1024,
                MAX_SKILL_FILE_BYTES / 1024
            ));
            continue;
        }
        let skill = segs[3];
        let rest = segs[4..].join("/");
        let dest = PathBuf::from(skill).join(&rest);
        if !seen.insert(dest.clone()) {
            skipped.push(format!("{rel}: дубль пути назначения {}", dest.display()));
            continue;
        }
        let body = if rest == "SKILL.md" {
            ensure_frontmatter(content, skill)
        } else {
            (*content).to_string()
        };
        out.push((dest, body));
    }
    out.sort();
    (out, skipped)
}

/// Собирает файлы скиллов из библиотеки пользователя на диске
/// (`[plugins].dirs`): для каждого каталога — каждый подкаталог с `skills/`
/// (манифест `plugin.json` НЕ требуется: плагины-мешки вроде arch-distilled
/// и самодельные каталоги скиллов доставляются тоже — тот же критерий, что
/// у синтеза манифеста в [`crate::plugin::discover`]), рекурсивно
/// `skills/<скилл>/<файл>`. Потолок размера и дедуп путей назначения — как
/// у встроенного сборщика [`collect_skill_files_from`]; не-UTF-8 файлы
/// пропускаются с заметкой (конвейер доставки текстовый).
fn collect_skill_files_from_disk(dirs: &[PathBuf]) -> (Vec<(PathBuf, String)>, Vec<String>) {
    let mut out: Vec<(PathBuf, String)> = Vec::new();
    let mut skipped = Vec::new();
    let mut seen: BTreeSet<PathBuf> = BTreeSet::new();
    for dir in dirs {
        let Ok(rd) = std::fs::read_dir(dir) else {
            continue; // каталога нет — источник просто пуст (свежая машина)
        };
        let mut plugins: Vec<PathBuf> = rd
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect();
        plugins.sort();
        for plugin in plugins {
            let skills_root = plugin.join("skills");
            if !skills_root.is_dir() {
                continue;
            }
            for entry in walkdir::WalkDir::new(&skills_root)
                .follow_links(false)
                .sort_by_file_name()
                .into_iter()
                .flatten()
            {
                if !entry.file_type().is_file() {
                    continue;
                }
                let path = entry.path();
                let Ok(rel) = path.strip_prefix(&skills_root) else {
                    continue; // вне корня скиллов — недостижимо для WalkDir
                };
                let segs: Vec<&str> = rel
                    .components()
                    .filter_map(|c| c.as_os_str().to_str())
                    .collect();
                // Назначение — <скилл>/<файл…>; файл прямо в skills/ (без
                // каталога скилла) — не layout, пропускаем с заметкой.
                let [skill, rest @ ..] = segs.as_slice() else {
                    continue;
                };
                if rest.is_empty() {
                    skipped.push(format!(
                        "{}: файл вне layout skills/<имя>/ — пропущен",
                        path.display()
                    ));
                    continue;
                }
                let dest = PathBuf::from(skill).join(rest.join("/"));
                if !seen.insert(dest.clone()) {
                    skipped.push(format!(
                        "{}: дубль пути назначения {}",
                        path.display(),
                        dest.display()
                    ));
                    continue;
                }
                let Ok(content) = std::fs::read_to_string(path) else {
                    skipped.push(format!(
                        "{}: не текстовый файл (UTF-8) — пропущен",
                        path.display()
                    ));
                    continue;
                };
                if content.len() > MAX_SKILL_FILE_BYTES {
                    skipped.push(format!(
                        "{}: {} КБ — больше потолка {} КБ",
                        path.display(),
                        content.len() / 1024,
                        MAX_SKILL_FILE_BYTES / 1024
                    ));
                    continue;
                }
                let body = if rest == ["SKILL.md"] {
                    ensure_frontmatter(&content, skill)
                } else {
                    content
                };
                out.push((dest, body));
            }
        }
    }
    out.sort();
    (out, skipped)
}

/// Раскладывает скиллы в `root` (`.claude/skills` хоста). Источник истины —
/// библиотека пользователя на диске (`plugin_dirs`, т.е. `[plugins].dirs`
/// конфига): всё, что там есть (включая плагины без манифеста и правки
/// пользователя), едет в хост; встроенные ассеты бинаря — fallback для
/// скиллов, которых на диске нет вообще (свежая машина без `arch-be init`).
/// Прецедент приоритета диска — MCP-промпты и `skill_load`
/// (`mcp_server.rs::resolve_playbook`). Новые копируются, отличающиеся —
/// перезаписываются версией источника (заметка в отчёте), совпадающие —
/// пропускаются (идемпотентность).
pub(super) fn install_skills(
    root: &Path,
    dry_run: bool,
    report: &mut ConnectReport,
    plugin_dirs: &[PathBuf],
) -> Result<()> {
    let (embedded, skipped_embedded) =
        collect_skill_files_from(crate::assets::embedded_plugin_files());
    let (disk, skipped_disk) = collect_skill_files_from_disk(plugin_dirs);
    report.skills_skipped.extend(skipped_embedded);
    report.skills_skipped.extend(skipped_disk);
    // Слияние источников: диск побеждает пофайлово; скилл «из библиотеки»,
    // если хотя бы один его файл пришёл с диска.
    let mut by_dest: BTreeMap<PathBuf, (&str, bool)> = BTreeMap::new(); // → (содержимое, с_диска)
    let embedded_map: BTreeMap<&Path, &str> = embedded
        .iter()
        .map(|(p, c)| (p.as_path(), c.as_str()))
        .collect();
    let disk_map: BTreeMap<&Path, &str> = disk
        .iter()
        .map(|(p, c)| (p.as_path(), c.as_str()))
        .collect();
    for (dest, content) in &embedded_map {
        by_dest.insert(dest.to_path_buf(), (*content, false));
    }
    for (dest, content) in &disk_map {
        by_dest.insert(dest.to_path_buf(), (*content, true));
    }
    // Имена скиллов (первый сегмент назначения) по источникам + счёт
    // расходящихся копий (диск отличается от встроенной по тому же пути).
    let skill_of = |dest: &Path| {
        dest.components().next().map_or_else(
            || "?".into(),
            |c| c.as_os_str().to_string_lossy().into_owned(),
        )
    };
    let library_skills: BTreeSet<String> = disk_map.keys().map(|p| skill_of(p)).collect();
    report.skills_from_library = library_skills.len();
    report.skills_from_embedded = embedded_map
        .keys()
        .map(|p| skill_of(p))
        .collect::<BTreeSet<_>>()
        .difference(&library_skills)
        .count();
    let overriding = library_skills
        .iter()
        .filter(|skill| {
            disk_map.iter().any(|(dest, disk_content)| {
                skill_of(dest) == **skill
                    && embedded_map
                        .get(dest)
                        .is_some_and(|embedded_content| embedded_content != disk_content)
            })
        })
        .count();
    if overriding > 0 {
        report.notes.push(format!(
            "{overriding} скиллов берутся из библиотеки пользователя поверх встроенных \
             копий (правки и новые версии пользователя в силе)"
        ));
    }
    // Агрегация по имени скилла: (новых, перезаписанных, без изменений).
    let mut by_skill: BTreeMap<String, (usize, usize, usize)> = BTreeMap::new();
    for (rel, (content, _from_disk)) in &by_dest {
        let path = root.join(rel);
        let skill = skill_of(rel);
        let old = std::fs::read_to_string(&path).ok();
        let counts = by_skill.entry(skill).or_default();
        match old.as_deref() {
            None => counts.0 += 1,
            Some(prev) if prev == *content => counts.2 += 1,
            Some(_) => counts.1 += 1,
        }
        if dry_run || old.as_deref() == Some(*content) {
            continue;
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| HarnessError::io(parent, e))?;
        }
        std::fs::write(&path, content).map_err(|e| HarnessError::io(&path, e))?;
    }
    for (name, (copied, updated, unchanged)) in by_skill {
        let action = if copied == 0 && updated == 0 {
            SkillAction::Unchanged(unchanged)
        } else if updated > 0 {
            SkillAction::Updated(copied + updated)
        } else {
            SkillAction::Copied(copied)
        };
        if updated > 0 {
            report.notes.push(format!(
                "скилл «{name}»: прежняя локальная правка заменена версией источника \
                 ({updated} файлов)"
            ));
        }
        report.skills.push(SkillOutcome { name, action });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connect::testkit::{read, user_library};
    use crate::connect::{ConnectOptions, Host, connect};

    /// Сборка скиллов из встроенных плагинов: layout, потолок размера,
    /// дубли путей, страховка frontmatter.
    #[test]
    fn collect_skill_files_layout_ceiling_and_frontmatter() {
        let big = "x".repeat(MAX_SKILL_FILE_BYTES + 1);
        let files: Vec<(&str, &str)> = vec![
            (
                "plugins/p1/skills/alpha/SKILL.md",
                "---\nname: alpha\ndescription: А\n---\n\n# A\n",
            ),
            ("plugins/p1/skills/alpha/references/r.md", "# ref\n"),
            ("plugins/p1/plugin.json", "{}"),
            (
                "plugins/p2/skills/beta/SKILL.md",
                "# Без фронтматтера\n\nТело.\n",
            ),
            ("plugins/p3/skills/big/SKILL.md", big.as_str()),
            (
                "plugins/p2/skills/alpha/SKILL.md",
                "---\nname: alpha\ndescription: Дубль\n---\n",
            ),
        ];
        let (out, skipped) = collect_skill_files_from(&files);
        let paths: Vec<&str> = out.iter().map(|(p, _)| p.to_str().expect("utf8")).collect();
        assert_eq!(
            paths,
            vec!["alpha/SKILL.md", "alpha/references/r.md", "beta/SKILL.md"],
            "{paths:?}"
        );
        assert_eq!(skipped.len(), 2, "большой + дубль: {skipped:?}");
        // Скилл без frontmatter получает минимальный (имя — из каталога).
        let beta = out
            .iter()
            .find(|(p, _)| p == &PathBuf::from("beta/SKILL.md"))
            .map(|(_, c)| c)
            .expect("beta");
        assert!(
            beta.starts_with("---\nname: beta\ndescription: Без фронтматтера\n"),
            "{beta}"
        );
        // Валидный frontmatter не трогается.
        let alpha = &out[0].1;
        assert!(alpha.starts_with("---\nname: alpha\n"), "{alpha}");
    }

    /// Скилл с локальной правкой обновляется встроенной версией (с заметкой),
    /// совпадающий — помечается «без изменений».
    #[test]
    fn skills_update_reports_overwritten_local_edit() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path().join("proj");
        connect(&ConnectOptions::new(Host::Claude, dir.clone())).expect("первый");
        let edited = dir.join(".claude/skills/adr-authoring/SKILL.md");
        std::fs::write(
            &edited,
            "---\nname: adr-authoring\ndescription: локальная правка\n---\n",
        )
        .expect("правка");
        let report = connect(&ConnectOptions::new(Host::Claude, dir.clone())).expect("второй");
        let outcome = report
            .skills
            .iter()
            .find(|s| s.name == "adr-authoring")
            .expect("скилл в отчёте");
        assert_eq!(outcome.action, SkillAction::Updated(1), "{outcome:?}");
        assert!(
            report.notes.iter().any(|n| n.contains("adr-authoring")),
            "{:?}",
            report.notes
        );
        assert!(
            read(&edited).contains("AI-DLC"),
            "встроенная версия на месте"
        );
        // А нетронутые скиллы — «без изменений».
        assert!(
            report
                .skills
                .iter()
                .any(|s| s.name == "bulkhead" && s.action == SkillAction::Unchanged(1)),
            "{:?}",
            report.skills
        );
    }

    /// (а) Скилл, существующий только в библиотеке пользователя (плагин без
    /// манифеста), доставляется в хост; счётчик `from_library` его считает.
    #[test]
    fn skills_from_user_library_delivered_when_absent_from_embedded() {
        let tmp = tempfile::tempdir().expect("tmp");
        let lib = user_library(
            tmp.path(),
            "arch-distilled",
            "user-only-skill",
            "---\nname: user-only-skill\ndescription: свой\n---\n\n# Свой скилл\n",
        );
        let dir = tmp.path().join("proj");
        let opts = ConnectOptions {
            plugins_dirs: vec![lib],
            ..ConnectOptions::new(Host::Claude, dir.clone())
        };
        let report = connect(&opts).expect("connect");
        let delivered = dir.join(".claude/skills/user-only-skill/SKILL.md");
        assert!(delivered.is_file(), "скилл из библиотеки доставлен");
        assert!(read(&delivered).contains("Свой скилл"));
        assert_eq!(
            report.skills_from_library, 1,
            "один скилл — из библиотеки пользователя"
        );
        assert!(
            report.skills_from_embedded > 0,
            "встроенные тоже доехали (fallback)"
        );
        // Идемпотентность: повтор — без изменений.
        let report2 = connect(&opts).expect("повтор");
        assert!(
            report2
                .skills
                .iter()
                .any(|s| s.name == "user-only-skill" && s.action == SkillAction::Unchanged(1)),
            "{:?}",
            report2.skills
        );
    }

    /// (б) Дисковая копия скилла отличается от встроенной → в хост пишется
    /// дисковая версия; сводная заметка «поверх встроенных копий» — одна.
    #[test]
    fn library_copy_overrides_embedded_with_summary_note() {
        let tmp = tempfile::tempdir().expect("tmp");
        let lib = user_library(
            tmp.path(),
            "arch-core",
            "adr-authoring",
            "---\nname: adr-authoring\ndescription: пользовательская редакция\n---\n\n# Моя редакция ADR\n",
        );
        let dir = tmp.path().join("proj");
        let opts = ConnectOptions {
            plugins_dirs: vec![lib],
            ..ConnectOptions::new(Host::Claude, dir.clone())
        };
        let report = connect(&opts).expect("connect");
        let delivered = dir.join(".claude/skills/adr-authoring/SKILL.md");
        assert!(
            read(&delivered).contains("Моя редакция ADR"),
            "в хосте — версия библиотеки пользователя, не встроенная"
        );
        let overriding: Vec<_> = report
            .notes
            .iter()
            .filter(|n| n.contains("поверх встроенных копий"))
            .collect();
        assert_eq!(
            overriding.len(),
            1,
            "ровно одна сводная заметка: {:?}",
            report.notes
        );
        assert!(overriding[0].contains("1 скиллов"), "{overriding:?}");
        assert_eq!(report.skills_from_library, 1);
    }

    /// (в) Пустой plugins.dirs — поведение как раньше: все скиллы из
    /// встроенных, заметок о библиотеке нет.
    #[test]
    fn empty_plugins_dirs_keeps_embedded_only_behavior() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path().join("proj");
        let report = connect(&ConnectOptions::new(Host::Claude, dir.clone())).expect("connect");
        assert_eq!(report.skills_from_library, 0);
        assert_eq!(
            report.skills_from_embedded,
            report.skills.len(),
            "все скиллы — из встроенных: {} vs {}",
            report.skills_from_embedded,
            report.skills.len()
        );
        assert!(
            !report
                .notes
                .iter()
                .any(|n| n.contains("поверх встроенных копий")),
            "{:?}",
            report.notes
        );
    }
}
