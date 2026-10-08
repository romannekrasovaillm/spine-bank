//! Журнал решений по архитектурному диффу (волна K):
//! `.arch-handoff/arch-diff-decisions.json`, схема
//! `arch-be/arch-diff-decisions/v1`.
//!
//! Журнал ПИШЕТ `arch-be arch-diff accept|reject` (K5, ADR-064); этот модуль
//! — чтение и общие идентификаторы (K6): составляющая гейта `arch_drift`
//! сверяет журнал с графом рабочего дерева. Общие [`edge_id`] и
//! [`grounds_hash`] — единая точка обеих сторон: писатель и читатель не
//! имеют права разъехаться в том, как называется ребро и что считается его
//! основаниями.
//!
//! Отклонение (`reject`) действует, пока `grounds_hash` записи совпадает с
//! отпечатком текущих оснований ребра: основания изменились (код сместился,
//! импорт переехал) — ребро снова становится предложением, а не rejected.

use std::fmt::Write as _;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::types::ArchEdge;
use crate::error::{HarnessError, Result};

/// Имя машинного контракта журнала решений (как `arch-be/arch-diff/v1` у
/// диффа): потребители сверяют поле `schema` перед разбором.
pub const ARCH_DIFF_DECISIONS_SCHEMA: &str = "arch-be/arch-diff-decisions/v1";

/// Путь журнала решений от корня репозитория.
pub const ARCH_DIFF_DECISIONS_PATH: &str = ".arch-handoff/arch-diff-decisions.json";

/// Решение по ребру диффа (K5): принять в модель или отклонить.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Decision {
    /// Ребро принято (правка модели оформляется дельтой — K5).
    Accept,
    /// Ребро отклонено: повторно не предлагается, пока не изменятся
    /// основания; оставшееся в коде — находка гейта (K6).
    Reject,
}

/// Запись журнала решений (схема `arch-be/arch-diff-decisions/v1`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecisionEntry {
    /// Канонический id ребра ([`edge_id`]).
    pub edge_id: String,
    /// Решение: accept | reject.
    pub decision: Decision,
    /// Причина решения (для `reject` — почему ребро вне модели законно).
    #[serde(default)]
    pub reason: String,
    /// Отпечаток оснований ребра на момент решения ([`grounds_hash`]).
    pub grounds_hash: String,
    /// Время решения (ISO 8601, строкой — как пишет K5).
    #[serde(default)]
    pub decided_at: String,
    /// Источник решения (кто/что принял: человек, агент — как в A3).
    #[serde(default)]
    pub source: String,
}

/// Журнал решений целиком.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecisionsJournal {
    /// Схема файла (обязана быть [`ARCH_DIFF_DECISIONS_SCHEMA`]).
    pub schema: String,
    /// Записи решений (порядок — порядок записи, K5).
    #[serde(default)]
    pub entries: Vec<DecisionEntry>,
}

/// Канонический id ребра графа для журнала решений:
/// `<kind>:<from>-><to>` (например, `import:CMP-001->CMP-004`). Стабилен между
/// прогонами при неизменном ребре (узлы и вид — из детерминированного графа).
#[must_use]
pub fn edge_id(edge: &ArchEdge) -> String {
    format!("{}:{}->{}", edge.kind.label(), edge.from, edge.to)
}

/// Отпечаток оснований ребра (`файл:строка`): sha256 канонической записи
/// «версия формата + id ребра + основания по порядку графа». Основания
/// изменились (правка кода сместила импорт) — отпечаток другой, и отклонение
/// перестаёт действовать: ребро снова предложение (K5), а не `rejected` (K6).
#[must_use]
pub fn grounds_hash(edge: &ArchEdge) -> String {
    let mut canon = format!("arch-be/arch-diff-edge-grounds/v1\n{}\n", edge_id(edge));
    for line in &edge.evidence {
        // Запись в String не может завершиться ошибкой — игнор безопасен.
        let _ = writeln!(canon, "{line}");
    }
    format!("sha256:{}", crate::hash::sha256_hex(canon.as_bytes()))
}

/// Читает журнал решений репозитория.
///
/// `Ok(None)` — файла нет: решений по диффу не принимали (нормальное
/// состояние, а не сбой). `Err` — файл ЕСТЬ, но не читается, не JSON или
/// чужая схема: сломанный вход читатель обязан назвать — молчаливый «нет
/// решений» скрыл бы подмену журнала (в нём — отклонения рёбер).
///
/// # Errors
/// Файл есть, но недоступен/битый/схема не `arch-be/arch-diff-decisions/v1`.
pub fn load_decisions(repo: &Path) -> Result<Option<DecisionsJournal>> {
    let path = repo.join(ARCH_DIFF_DECISIONS_PATH);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(HarnessError::io(path, e)),
    };
    let journal: DecisionsJournal = serde_json::from_str(&text).map_err(|e| {
        HarnessError::Control(format!(
            "{ARCH_DIFF_DECISIONS_PATH}: журнал решений arch-diff не разобран: {e}"
        ))
    })?;
    if journal.schema != ARCH_DIFF_DECISIONS_SCHEMA {
        return Err(HarnessError::Control(format!(
            "{ARCH_DIFF_DECISIONS_PATH}: схема '{}' не поддерживается (ожидается \
             '{ARCH_DIFF_DECISIONS_SCHEMA}')",
            journal.schema
        )));
    }
    Ok(Some(journal))
}

#[cfg(test)]
mod tests {
    use super::super::types::EdgeKind;
    use super::*;

    /// Ребро фикстуры с заданными основаниями.
    fn edge(evidence: &[&str]) -> ArchEdge {
        ArchEdge {
            from: "CMP-001".to_string(),
            to: "CMP-004".to_string(),
            kind: EdgeKind::Import,
            evidence: evidence.iter().map(|s| (*s).to_string()).collect(),
        }
    }

    /// id и отпечаток ребра стабильны и чувствительны к основаниям: сдвиг
    /// строки импорта меняет отпечаток (отклонение устаревает), тот же набор
    /// оснований — тот же отпечаток (K5 и K6 считают одно и то же).
    #[test]
    fn edge_id_and_grounds_hash_are_stable_and_grounds_sensitive() {
        let e = edge(&["skeleton/intake/writer.py:2"]);
        assert_eq!(edge_id(&e), "import:CMP-001->CMP-004");
        assert_eq!(grounds_hash(&e), grounds_hash(&e));
        assert!(
            grounds_hash(&e).starts_with("sha256:"),
            "{}",
            grounds_hash(&e)
        );
        // Другие основания (строка сместилась) — другой отпечаток.
        let moved = edge(&["skeleton/intake/writer.py:5"]);
        assert_eq!(edge_id(&moved), edge_id(&e), "id ребра тот же");
        assert_ne!(grounds_hash(&moved), grounds_hash(&e));
        // Другой вид ребра при тех же концах — другой id.
        let mut connect = e.clone();
        connect.kind = EdgeKind::Connect;
        assert_ne!(edge_id(&connect), edge_id(&e));
    }

    /// Журнал: нет файла — `None`; валидный файл — записи; битый JSON и чужая
    /// схема — ошибка (сломанный вход не маскируется под «нет решений»).
    #[test]
    fn load_decisions_absent_valid_broken() {
        let dir = tempfile::tempdir().expect("tmp");
        let repo = dir.path();
        assert_eq!(load_decisions(repo).expect("нет файла"), None);

        let journal = DecisionsJournal {
            schema: ARCH_DIFF_DECISIONS_SCHEMA.to_string(),
            entries: vec![DecisionEntry {
                edge_id: edge_id(&edge(&["a.py:1"])),
                decision: Decision::Reject,
                reason: "законный обход".to_string(),
                grounds_hash: grounds_hash(&edge(&["a.py:1"])),
                decided_at: "2026-10-08T12:00:00+03:00".to_string(),
                source: "human".to_string(),
            }],
        };
        let path = repo.join(ARCH_DIFF_DECISIONS_PATH);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        std::fs::write(&path, serde_json::to_string_pretty(&journal).expect("json"))
            .expect("запись журнала");
        let loaded = load_decisions(repo)
            .expect("валидный журнал")
            .expect("Some");
        assert_eq!(loaded, journal, "круговой обход записи и чтения");

        std::fs::write(&path, "{битый json").expect("битый журнал");
        assert!(load_decisions(repo).is_err(), "битый JSON — ошибка");
        std::fs::write(&path, "{\"schema\": \"other/v9\", \"entries\": []}").expect("чужая схема");
        let err = load_decisions(repo).expect_err("чужая схема — ошибка");
        assert!(err.to_string().contains("other/v9"), "{err}");
    }
}
