//! Отчёт о восстановленной архитектуре (ADR-080, этап 1): SSOT модели
//! `arch-be/report/v1`.
//!
//! Модуль собирает отчёт из уже существующих read-only источников —
//! графа `recover` ([`crate::arch_diff::recover`]), обследования
//! ([`crate::survey`]) и сборщика фактов ([`crate::constraints_harvest`]) — и
//! назначает стабильные report-ID, уверенность A/B/C ([`ConfidenceAbc`]) и
//! основания `файл:строка`. Наружу отдаёт [`ReportModel`], рендеры
//! ([`render_json`], [`render_md`]) и fail-closed автопроверку ([`check`]).
//!
//! AD-11 дельты `report-v1`: сборка модели и назначение ID/уверенности живут
//! ТОЛЬКО здесь; CLI (`arch-be report`) и будущий MCP-мост — тонкие обёртки.

pub mod build;
pub mod check;
pub mod model;
pub mod render;

pub use build::build;
pub use check::{check, resolve_ref};
pub use model::{
    CheckFinding, CheckSeverity, ConfidenceAbc, Family, REPORT_SCHEMA, ReportElement, ReportMeta,
    ReportModel, ReportRelation, ReportSection, SECTION_COUNT, SECTION_KINDS, SECTION_TITLES,
    STATUS_AUTO, SectionItem, SectionKind,
};
pub use render::{render_json, render_md};
