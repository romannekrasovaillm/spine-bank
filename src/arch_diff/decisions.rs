//! Журнал решений по предложениям архитектурного диффа (K5, ADR-064):
//! `.arch-handoff/arch-diff-decisions.json` (схема
//! [`DECISIONS_SCHEMA`]). Append-only: новое решение по ребру дописывается,
//! действует ПОСЛЕДНЕЕ (last-wins); предложение подавляется, пока
//! `grounds_hash` последней записи совпадает с текущими основаниями ребра
//! (список `файл:строка`). Основания изменились — ребро предлагается снова.
//!
//! Журнал — артефакт рабочего дерева (как `teeth.json` волны B): дифф читает
//! его при каждом прогоне, поэтому `arch-be arch-diff` не показывает решённое.
//! Битый журнал — ошибка, а не молчаливый сброс: потерянные отказы хуже
//! упавшего прогона.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{HarnessError, Result};

/// Имя машинного контракта журнала решений (поле `schema`).
pub const DECISIONS_SCHEMA: &str = "arch-be/arch-diff-decisions/v1";

/// Путь журнала относительно корня репозитория.
pub const DECISIONS_PATH: &str = ".arch-handoff/arch-diff-decisions.json";

/// Переменная окружения с честным источником решения (`human`|`agent`);
/// без неё (или с иным значением) — `unknown`. Связывание с A3-подписью —
/// ADR-060 (не этот модуль); механики принуждения здесь нет.
pub const ACTOR_ENV: &str = "ARCH_BE_ACTOR";

/// Решение по предложению диффа.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    /// Принято (оформлено дельтой).
    Accept,
    /// Отклонено (с обязательной причиной).
    Reject,
}

impl Decision {
    /// Метка для вывода.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Accept => "accept",
            Self::Reject => "reject",
        }
    }
}

/// Источник решения: человек, агент или честное «неизвестно» (ADR-064).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionSource {
    /// Человек (`ARCH_BE_ACTOR=human`).
    Human,
    /// Агент (`ARCH_BE_ACTOR=agent`).
    Agent,
    /// Источник не задан — неизвестно (значение по умолчанию, без выдумок).
    Unknown,
}

impl DecisionSource {
    /// Метка для вывода.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Human => "human",
            Self::Agent => "agent",
            Self::Unknown => "unknown",
        }
    }
}

/// Запись журнала решений (схема `arch-be/arch-diff-decisions/v1`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecisionEntry {
    /// Стабильный id объекта решения (`<kind>:<from>-><to>`, `node:<id>`).
    pub edge_id: String,
    /// Решение.
    pub decision: Decision,
    /// Причина (обязательна для reject; у accept — пустая строка).
    #[serde(default)]
    pub reason: String,
    /// Хэш оснований предложения на момент решения (sha256 списка
    /// `файл:строка`; для узла — id). Не совпал с текущим — предложение
    /// снова показывается.
    pub grounds_hash: String,
    /// Момент решения (RFC 3339, локальное время).
    pub decided_at: String,
    /// Источник решения (честный; см. [`ACTOR_ENV`]).
    pub source: DecisionSource,
    /// Дельта, оформившая принятие (`accept`; у reject отсутствует).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delta: Option<String>,
}

/// Журнал решений целиком (файл `.arch-handoff/arch-diff-decisions.json`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecisionJournal {
    /// Версия схемы ([`DECISIONS_SCHEMA`]); потребители сверяют перед разбором.
    pub schema: String,
    /// Записи в порядке дописки (append-only; last-wins по `edge_id`).
    #[serde(default)]
    pub entries: Vec<DecisionEntry>,
}

impl Default for DecisionJournal {
    fn default() -> Self {
        Self {
            schema: DECISIONS_SCHEMA.to_string(),
            entries: Vec::new(),
        }
    }
}

impl DecisionJournal {
    /// Последняя запись по ребру (last-wins), если решение по нему было.
    #[must_use]
    pub fn last_of<'a>(&'a self, edge_id: &str) -> Option<&'a DecisionEntry> {
        self.entries.iter().rev().find(|e| e.edge_id == edge_id)
    }
}

/// Путь журнала решений репозитория.
#[must_use]
pub fn decisions_path(repo: &Path) -> PathBuf {
    repo.join(DECISIONS_PATH)
}

/// Читает журнал; нет файла — пустой журнал (ещё ничего не решали).
///
/// # Errors
/// Файл есть, но не читается или не разбирается — ошибка (решения не
/// «теряются» молча).
pub fn load(repo: &Path) -> Result<DecisionJournal> {
    let path = decisions_path(repo);
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Ok(DecisionJournal::default()); // нет файла — решений не было
    };
    serde_json::from_str(&text).map_err(|e| {
        HarnessError::Control(format!(
            "{}: журнал решений arch-diff не разбирается ({e}) — решения не должны \
             теряться молча; почините JSON или удалите файл осознанно",
            path.display()
        ))
    })
}

/// Дописывает запись в журнал (append-only; читатели применяют last-wins).
///
/// # Errors
/// Ошибки чтения/записи журнала.
pub fn append(repo: &Path, entry: DecisionEntry) -> Result<PathBuf> {
    let mut journal = load(repo)?;
    journal.entries.push(entry);
    let path = decisions_path(repo);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| HarnessError::io(parent, e))?;
    }
    let text = serde_json::to_string_pretty(&journal)
        .map_err(|e| HarnessError::Config(format!("сериализация журнала решений: {e}")))?;
    std::fs::write(&path, format!("{text}\n")).map_err(|e| HarnessError::io(&path, e))?;
    Ok(path)
}

/// Разбор значения источника решения (`human`/`agent`, регистр не важен);
/// неизвестное или отсутствующее — `Unknown` (честно, без выдумок).
#[must_use]
pub fn parse_actor_env(value: Option<&str>) -> DecisionSource {
    match value.map(|v| v.trim().to_ascii_lowercase()).as_deref() {
        Some("human") => DecisionSource::Human,
        Some("agent") => DecisionSource::Agent,
        _ => DecisionSource::Unknown,
    }
}

/// Источник решения из окружения процесса ([`ACTOR_ENV`]); без переменной —
/// `Unknown`. Конфиг и подпись A3 — не здесь (ADR-060).
#[must_use]
pub fn detect_source() -> DecisionSource {
    parse_actor_env(std::env::var(ACTOR_ENV).ok().as_deref())
}

/// Момент решения (RFC 3339) — одна точка форматирования для записей журнала.
#[must_use]
pub fn now_stamp() -> String {
    chrono::Local::now().to_rfc3339()
}

/// Убирает из списка предложения с действующим решением журнала (последняя
/// запись по `edge_id`, и её `grounds_hash` совпадает с текущим). Номера
/// оставшихся СОХРАНЯЮТСЯ (без перенумерации): accept/reject ссылаются на
/// номера одного вывода в одном сеансе — `accept 1` не должен сдвигать
/// номер соседнего предложения (дырки в нумерации — след решений).
///
/// # Errors
/// Битый журнал — ошибка (см. [`load`]).
pub(crate) fn suppress_decided(
    repo: &Path,
    proposals: &mut Vec<super::diff::ModelProposal>,
) -> Result<()> {
    let journal = load(repo)?;
    if journal.entries.is_empty() {
        return Ok(());
    }
    proposals.retain(|p| {
        journal
            .last_of(&p.edge_id)
            .is_none_or(|e| e.grounds_hash != p.grounds_hash)
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Запись-фикстура с переопределяемыми полями.
    fn entry(edge_id: &str, decision: Decision, grounds_hash: &str) -> DecisionEntry {
        DecisionEntry {
            edge_id: edge_id.to_string(),
            decision,
            reason: String::new(),
            grounds_hash: grounds_hash.to_string(),
            decided_at: "2026-10-08T00:00:00+00:00".to_string(),
            source: DecisionSource::Unknown,
            delta: None,
        }
    }

    /// Журнал версионирован (поле schema), append-only: две записи по одному
    /// ребру хранятся обе, действует последняя (last-wins).
    #[test]
    fn journal_is_versioned_append_only_last_wins() {
        let tmp = tempfile::tempdir().expect("tmp");
        let repo = tmp.path();
        append(repo, entry("import:A->B", Decision::Reject, "h1")).expect("append 1");
        append(repo, entry("import:A->B", Decision::Accept, "h2")).expect("append 2");
        let raw: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(decisions_path(repo)).expect("файл журнала"),
        )
        .expect("json");
        assert_eq!(raw["schema"], DECISIONS_SCHEMA, "журнал версионирован");
        assert_eq!(raw["entries"].as_array().map_or(0, Vec::len), 2);
        let journal = load(repo).expect("load");
        assert_eq!(journal.entries.len(), 2, "append-only: обе записи хранятся");
        let last = journal.last_of("import:A->B").expect("запись есть");
        assert_eq!(last.decision, Decision::Accept);
        assert_eq!(last.grounds_hash, "h2", "действует последняя запись");
    }

    /// Нет файла — пустой журнал; битый JSON — ошибка, а не молчаливый сброс.
    #[test]
    fn load_missing_is_empty_and_corrupt_is_error() {
        let tmp = tempfile::tempdir().expect("tmp");
        let journal = load(tmp.path()).expect("пустой");
        assert_eq!(journal.entries.len(), 0, "{journal:?}");
        let path = decisions_path(tmp.path());
        std::fs::create_dir_all(path.parent().expect("родитель")).expect("mkdir");
        std::fs::write(&path, "{битый json").expect("write");
        let err = load(tmp.path()).expect_err("битый журнал — ошибка");
        assert!(err.to_string().contains("не разбирается"), "{err}");
    }

    /// Разбор источника решения: human/agent (любой регистр), остальное и
    /// отсутствие — unknown (честно, без выдумок).
    #[test]
    fn parse_actor_env_honest() {
        assert_eq!(parse_actor_env(Some("human")), DecisionSource::Human);
        assert_eq!(parse_actor_env(Some("AGENT")), DecisionSource::Agent);
        assert_eq!(parse_actor_env(Some(" agent ")), DecisionSource::Agent);
        assert_eq!(parse_actor_env(Some("robot")), DecisionSource::Unknown);
        assert_eq!(parse_actor_env(Some("")), DecisionSource::Unknown);
        assert_eq!(parse_actor_env(None), DecisionSource::Unknown);
    }
}
