//! Архитектурный дифф каждого PR (волна K, ADR-063): граф «как построено»
//! (`as_built`) из кода ревизии — узлы (компоненты по `code_roots` модели или
//! выведенные по манифестам сборки, внешние системы `host:port`, хранилища по
//! строкам подключения, контракты) и рёбра (импорты, обращения, контрактные
//! ссылки) с основаниями `файл:строка` — и дифф двух ревизий (`arch_diff`).
//!
//! Границы: core (без сети, TUI и LLM, AD-2), read-only по отношению к
//! рабочему дереву (снимки читаются из git), детерминизм (правило 10:
//! повторный прогон на тех же коммитах даёт байт-в-байт тот же результат).
//!
//! Семантика детекторов не дублируется, а переиспользуется: импорты —
//! [`crate::imports`] (общий код с `control::exec`), интеграции
//! `host:port` — паттерн и разбор [`crate::survey`], строки подключения и
//! опознание контрактов по содержимому — [`crate::control::diff_triggers`],
//! классификация контрактов — [`crate::contract_diff`], NFR — [`crate::nfr`],
//! маршрут значимости — [`crate::control::score_with_sources`].
//!
//! Модуль [`decisions`] — журнал решений по диффу (`.arch-handoff/
//! arch-diff-decisions.json`, схема `arch-be/arch-diff-decisions/v1`):
//! пишет `arch-diff accept|reject` (K5, ADR-064), читает составляющая гейта
//! `arch_drift` (K6); [`snapshot::snapshot_worktree`] — снимок рабочего
//! дерева (голова диффа гейта: незакоммиченные правки).

mod as_built;
mod decisions;
mod diff;
mod report;
mod snapshot;
mod types;

pub use as_built::{as_built, as_built_with};
pub(crate) use as_built::{scan_revision, scan_worktree};
pub use decisions::{
    ARCH_DIFF_DECISIONS_PATH, ARCH_DIFF_DECISIONS_SCHEMA, Decision, DecisionEntry,
    DecisionsJournal, edge_id, grounds_hash, load_decisions,
};
pub(crate) use diff::edge_model_status;
pub use diff::{
    ArchDiff, ArchDiffInput, ContractChange, ContractClass, DeclaredEdge, EdgeChange, FailOn,
    InvariantHit, ModelProposal, ModelStatus, NfrShift, ProposalKind, RouteInfo, RuleGuard,
    TeethClass, TriggerHit, arch_diff, matched_failures,
};
pub use report::{render_json, render_md, render_mermaid, render_sarif};
pub use snapshot::{Snapshot, resolve_rev, snapshot_at, snapshot_worktree};
pub use types::{
    ARCH_DIFF_SCHEMA, ArchEdge, ArchGraph, ArchNode, EdgeKind, MAX_EDGE_EVIDENCE, NodeKind,
};
