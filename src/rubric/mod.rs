//! Движок рубрик архитектурного контроля: якорные и динамические.
//!
//! КОНТРАКТ (владелец: агент `rubric`):
//! - [`Rubric`] — YAML-рубрика: название, описание, шкала, критерии с весами
//!   и якорями уровней (anchor descriptors), опц. секция динамической генерации;
//! - якорные рубрики — готовые YAML из assets/rubrics; динамические —
//!   генерируются LLM под предмет оценки от якорной-основы ([`generate_dynamic`]);
//! - [`evaluate`]/[`evaluate_with_options`] — LLM-судья оценивает целевой текст
//!   по критериям `JudgeConfig::samples` независимыми сэмплами (итог — медиана,
//!   разброс σ → метка `unstable`), механически проверяет цитату-свидетельство
//!   из текста в каждом rationale (нет подтверждения → `evidence_not_found`,
//!   критерий исключается из итога), длинный текст — явная ошибка, а не
//!   усечение; текст в промпте изолирован маркерами от prompt injection;
//!   структурированный разбор → [`RubricReport`] (баллы, веса, метки,
//!   markdown-отчёт). Решения и пороги — ADR-004.
//! - split-judge (MCP-режим без LLM у сервера): швы [`judge_system_prompt`],
//!   [`judge_user_prompt`], [`parse_judge_response`], [`build_report`]
//!   видны как `pub(crate)` для `mcp_server` (`rubric_prompt`/`rubric_verify`) —
//!   промпт собирается сервером, отвечает модель хоста, сборка отчёта
//!   механическая и идёт тем же кодом, что у встроенного судьи.
//!
//! Разбиение модуля (волна B1): `types` — типы рубрики и оценок (критерии,
//! метки достоверности, снимки конфигурации судьи); `catalog` — загрузка
//! и список YAML-рубрик каталога; `judge` — LLM-судья (промпты, сэмплы,
//! динамическая генерация); `report` — разбор ответов судьи и механическая
//! сборка отчёта (сверка цитат, медианы, взвешенный итог, потолок вердикта);
//! `artifact` — машиночитаемые отчёты `reports/rubric/` (запись с
//! происхождением, slug'и, загрузка); `tools` — агентные инструменты
//! (`rubric_list`/`rubric_evaluate`/`rubric_generate`); `testkit` — общие
//! фикстуры тестов.

mod artifact;
mod catalog;
mod judge;
mod report;
#[cfg(test)]
mod testkit;
mod tools;
mod types;

pub use artifact::{
    ArtifactExtras, ArtifactSubject, RUBRIC_REPORT_SCHEMA, RUBRIC_REPORTS_DIR, RubricArtifact,
    artifact_json, artifact_slug, independence_for, load_artifacts, pack_artifact_slug,
    repo_root_of, write_artifact, write_artifact_for_subject, write_artifact_with,
};
pub use catalog::{list, load};
pub use judge::{
    check_target_len, evaluate_collecting, evaluate_pack, evaluate_with_options, generate_dynamic,
};
pub(crate) use judge::{judge_system_prompt, judge_user_prompt};
pub(crate) use report::{EvidenceScope, build_report, parse_judge_response};
pub use report::{RubricReport, weighted_total};
pub use tools::tools;
pub use types::{
    Coverage, Criterion, CriterionFlag, CriterionScore, CriterionSnapshot, EvidenceOn,
    JudgeConfigSnapshot, MAX_TARGET_CHARS, Rubric, RubricSummary,
};
