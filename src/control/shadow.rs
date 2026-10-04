//! Флотовой срез теневого гейта (волна D, D2; ADR-059).
//!
//! Гейт с `--shadow-constraints <файл>` вычисляет решающий вердикт по
//! текущему реестру и ДОПОЛНИТЕЛЬНО теневой — по файлу-кандидату; разница
//! («что покраснеет при переходе») сохраняется отчётом прогона в
//! `<проект>/.arch-handoff/shadow.json` ([`SHADOW_RECORD_PATH`]).
//!
//! [`control_report`](crate::control::control_report) уровня `corp` собирает
//! эти отчёты по проектам (сам репозиторий и его непосредственные
//! подкаталоги — как у [`crate::fleet`]) и агрегирует счётчики появляющихся
//! находок по правилам ([`ShadowFleet`]): решение о поднятии пина `extends`
//! становится измеримым, а не рукописным разделом CHANGELOG.
//!
//! Запись — артефакт прогона, не состояние вердикта: её отсутствие ничего не
//! меняет, а нечитаемый JSON пропускается молча (битый отчёт прогона не
//! должен ронять флотовой срез).

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{HarnessError, Result};

/// Путь отчёта теневого прогона относительно корня проекта.
pub const SHADOW_RECORD_PATH: &str = ".arch-handoff/shadow.json";

/// Схема отчёта теневого прогона.
pub const SHADOW_RECORD_SCHEMA: &str = "arch-be/shadow/v1";

/// Смена severity правила (по id правила).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShadowChange {
    /// Было (текущий реестр).
    pub from: String,
    /// Стало (реестр-кандидат).
    pub to: String,
}

/// Отчёт одного теневого прогона: находки-кандидаты по id правила.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShadowRecord {
    /// Схема (`arch-be/shadow/v1`).
    pub schema: String,
    /// Файл-кандидат реестра (как передан вызову).
    pub shadow_constraints: String,
    /// Появляющиеся правила: id правила → худшая severity (`error`|`warn`).
    #[serde(default)]
    pub new_findings: BTreeMap<String, String>,
    /// Смены severity по id правила.
    #[serde(default)]
    pub severity_changes: BTreeMap<String, ShadowChange>,
    /// Сводка «что покраснеет при переходе».
    #[serde(default)]
    pub summary: String,
}

/// Агрегат появляющегося правила по флоту.
#[derive(Debug, Clone, Serialize)]
pub struct ShadowFleetRule {
    /// Id правила.
    pub rule: String,
    /// Худшая severity среди проектов.
    pub severity: String,
    /// Проекты, где правило появится.
    pub projects: Vec<String>,
}

/// Агрегат смены severity по флоту.
#[derive(Debug, Clone, Serialize)]
pub struct ShadowFleetChange {
    /// Id правила.
    pub rule: String,
    /// Было.
    pub from: String,
    /// Стало.
    pub to: String,
    /// Проекты, где смена произойдёт.
    pub projects: Vec<String>,
}

/// Флотовой срез «что покраснеет по флоту».
#[derive(Debug, Clone, Default, Serialize)]
pub struct ShadowFleet {
    /// Проектов с сохранёнными shadow-результатами.
    pub projects: usize,
    /// Новых правил (появляющихся хотя бы у одного проекта).
    pub new_rules: Vec<ShadowFleetRule>,
    /// Смен severity по флоту.
    pub severity_changes: Vec<ShadowFleetChange>,
}

impl ShadowFleet {
    /// Пуст ли срез (нет сохранённых shadow-результатов).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.projects == 0
    }
}

/// Сохраняет отчёт теневого прогона в `<root>/.arch-handoff/shadow.json`.
///
/// # Errors
/// Ошибки записи файла (нет прав на `.arch-handoff/`).
pub fn save_shadow_record(root: &Path, record: &ShadowRecord) -> Result<()> {
    let path = root.join(SHADOW_RECORD_PATH);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| HarnessError::io(parent, e))?;
    }
    let text = serde_json::to_string_pretty(record)
        .map_err(|e| HarnessError::Config(format!("сериализация shadow-отчёта: {e}")))?;
    std::fs::write(&path, text).map_err(|e| HarnessError::io(&path, e))
}

/// Читает сохранённые отчёты теневого прогона по флоту: сам `root` и его
/// непосредственные подкаталоги; метка проекта — путь относительно `root`
/// (`.` — сам корень). Нечитаемый/битый отчёт пропускается.
#[must_use]
pub fn shadow_records(root: &Path) -> Vec<(String, ShadowRecord)> {
    let mut dirs = vec![root.to_path_buf()];
    if let Ok(rd) = std::fs::read_dir(root) {
        for entry in rd.flatten() {
            let p = entry.path();
            if p.is_dir() {
                dirs.push(p);
            }
        }
    }
    let mut out = Vec::new();
    for dir in dirs {
        let path = dir.join(SHADOW_RECORD_PATH);
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(record) = serde_json::from_str::<ShadowRecord>(&text) else {
            continue;
        };
        let label = dir
            .strip_prefix(root)
            .ok()
            .filter(|p| !p.as_os_str().is_empty())
            .map_or_else(|| ".".to_string(), |p| p.display().to_string());
        out.push((label, record));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// Агрегирует сохранённые отчёты в флотовой срез «что покраснеет по флоту».
#[must_use]
pub fn aggregate_shadow(records: &[(String, ShadowRecord)]) -> ShadowFleet {
    // rule → (худшая severity, проекты)
    let mut by_rule: BTreeMap<String, (String, BTreeSet<String>)> = BTreeMap::new();
    // (rule, from, to) → проекты
    let mut changes: BTreeMap<(String, String, String), BTreeSet<String>> = BTreeMap::new();
    for (project, record) in records {
        for (rule, severity) in &record.new_findings {
            let entry = by_rule
                .entry(rule.clone())
                .or_insert_with(|| (severity.clone(), BTreeSet::new()));
            if severity == "error" {
                entry.0 = "error".to_string();
            }
            entry.1.insert(project.clone());
        }
        for (rule, change) in &record.severity_changes {
            changes
                .entry((rule.clone(), change.from.clone(), change.to.clone()))
                .or_default()
                .insert(project.clone());
        }
    }
    ShadowFleet {
        projects: records.len(),
        new_rules: by_rule
            .into_iter()
            .map(|(rule, (severity, projects))| ShadowFleetRule {
                rule,
                severity,
                projects: projects.into_iter().collect(),
            })
            .collect(),
        severity_changes: changes
            .into_iter()
            .map(|((rule, from, to), projects)| ShadowFleetChange {
                rule,
                from,
                to,
                projects: projects.into_iter().collect(),
            })
            .collect(),
    }
}

/// Собирает флотовой срез по каталогу проектов.
#[must_use]
pub fn shadow_fleet(root: &Path) -> ShadowFleet {
    aggregate_shadow(&shadow_records(root))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(new: &[(&str, &str)], changes: &[(&str, &str, &str)]) -> ShadowRecord {
        ShadowRecord {
            schema: SHADOW_RECORD_SCHEMA.to_string(),
            shadow_constraints: "candidate.yaml".to_string(),
            new_findings: new
                .iter()
                .map(|(r, s)| ((*r).to_string(), (*s).to_string()))
                .collect(),
            severity_changes: changes
                .iter()
                .map(|(r, f, t)| {
                    (
                        (*r).to_string(),
                        ShadowChange {
                            from: (*f).to_string(),
                            to: (*t).to_string(),
                        },
                    )
                })
                .collect(),
            summary: "тест".to_string(),
        }
    }

    /// Два проекта: правило X-2 появляется у обоих, X-1 сменяет severity —
    /// агрегат считает проекты по правилам.
    #[test]
    fn fleet_aggregates_new_rules_by_project() {
        let records = vec![
            (
                "p1".to_string(),
                record(&[("X-2", "error")], &[("X-1", "warn", "error")]),
            ),
            (
                "p2".to_string(),
                record(&[("X-2", "warn"), ("X-3", "error")], &[]),
            ),
        ];
        let fleet = aggregate_shadow(&records);
        assert_eq!(fleet.projects, 2);
        let x2 = fleet
            .new_rules
            .iter()
            .find(|r| r.rule == "X-2")
            .expect("X-2");
        assert_eq!(x2.severity, "error", "худшая severity по флоту");
        assert_eq!(x2.projects, ["p1", "p2"]);
        assert!(fleet.new_rules.iter().any(|r| r.rule == "X-3"));
        let change = fleet
            .severity_changes
            .iter()
            .find(|c| c.rule == "X-1")
            .expect("X-1");
        assert_eq!(
            (change.from.as_str(), change.to.as_str()),
            ("warn", "error")
        );
        assert_eq!(change.projects, ["p1"]);
        assert!(!fleet.is_empty());
    }

    /// Пустой срез — нет сохранённых записей.
    #[test]
    fn fleet_empty_without_records() {
        let fleet = aggregate_shadow(&[]);
        assert!(fleet.is_empty());
        assert!(fleet.new_rules.is_empty());
    }

    /// Скан читает сам корень и непосредственные подкаталоги, битый JSON
    /// пропускается.
    #[test]
    fn scan_reads_root_and_subdirs_skipping_broken() {
        let tmp = tempfile::tempdir().expect("tmp");
        let root = tmp.path();
        save_shadow_record(root, &record(&[("X-2", "error")], &[])).expect("root record");
        let p1 = root.join("p1");
        std::fs::create_dir_all(p1.join(".arch-handoff")).expect("mkdir");
        save_shadow_record(&p1, &record(&[("X-2", "warn")], &[])).expect("p1 record");
        let p2 = root.join("p2");
        std::fs::create_dir_all(p2.join(".arch-handoff")).expect("mkdir");
        std::fs::write(p2.join(SHADOW_RECORD_PATH), "{ not json").expect("broken");

        let records = shadow_records(root);
        let labels: Vec<&str> = records.iter().map(|(p, _)| p.as_str()).collect();
        assert_eq!(labels, [".", "p1"], "битая запись пропущена: {records:?}");
        let fleet = shadow_fleet(root);
        assert_eq!(fleet.projects, 2);
    }
}
