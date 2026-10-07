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

mod as_built;
mod diff;
mod snapshot;
mod types;

pub use as_built::{as_built, as_built_with};
pub use diff::{
    ArchDiff, ArchDiffInput, ContractChange, ContractClass, DeclaredEdge, EdgeChange, InvariantHit,
    ModelProposal, ModelStatus, NfrShift, ProposalKind, RouteInfo, RuleGuard, TeethClass,
    TriggerHit, arch_diff,
};
pub use snapshot::{Snapshot, resolve_rev, snapshot_at};
pub use types::{
    ARCH_DIFF_SCHEMA, ArchEdge, ArchGraph, ArchNode, EdgeKind, MAX_EDGE_EVIDENCE, NodeKind,
};
