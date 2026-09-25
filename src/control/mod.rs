//! Архитектурный контроль: fitness functions, линтер spine, сенсоры спек,
//! маршрутизация по Architecture Significance Score.
//!
//! КОНТРАКТ (владелец: агент `control`) — детерминированный механический слой
//! (идеи из `docs/SOURCE_BRIEF.md`: линтер spine, сенсоры required-sections и
//! upstream-coverage, 15 триггеров значимости, маршруты Fast/Standard/Critical):
//! - [`lint_spine`] — проверки ARCHITECTURE-SPINE.md: дубли AD-id, пустые
//!   Binds/Prevents/Rule, заглушки (TODO/TBD), непиннутые версии, ссылки на
//!   несуществующие AD;
//! - [`sensors_check`] — сенсоры спецификаций: наличие обязательных секций,
//!   upstream-coverage (артефакт ссылается на входы из consumes);
//! - [`check`] — fitness functions из `CONSTRAINTS.yaml`: `must_contain` /
//!   `must_not_contain` (regex по glob-набору файлов),
//!   `each_file_must_contain` (regex обязателен в КАЖДОМ файле набора),
//!   `file_exists`, `dir_must_have_file` (обязательный файл в каждом
//!   каталоге набора), `max_age` (свежесть файла: mtime не старше N дней),
//!   `command_succeeds` (с таймаутом),
//!   `dependency_direction` (структурная проверка направления зависимостей:
//!   импорты файлов набора против `forbid`/`allow`-списков модулей,
//!   ADR-029) и `context_boundary` (границы контекстов: импорты не
//!   пересекают `code_roots` CMP-сущностей модели без `depends_on`,
//!   ADR-030), `deny_dependency` (запрещённые пакеты в манифестах
//!   Cargo.toml/pom.xml/requirements.txt — детектор тех-радара,
//!   `docs/corp-spine.md`); итог PASS/FAIL + находки + длительность каждого
//!   правила (per-rule timing, [`RuleDuration`]); baseline-режим (ratchet) для
//!   brownfield — исторический долг не ломает гейт, ломают только новые
//!   нарушения и рост счётчика правила, а `--changed-since` прогоняет файловые
//!   правила на срезе изменённых файлов (модуль [`baseline`],
//!   [`check_with_options`]);
//! - наследование корпоративного контекста (`docs/corp-spine.md`): поле
//!   верхнего уровня `extends: [<ref>@<version>]` подмешивает правила
//!   родительских constraint-файлов с меткой источника и проверкой пина
//!   версии (расхождение — error-находка «родитель обновился», а не
//!   молчаливая поломка); `overrides:` — исключение правила только через
//!   ADR (rule + adr + until, неполный — error, просроченный — warn и
//!   правило снова действует); `severity: block|warn` — warn-правила не
//!   ломают гейт; [`control_report`] — отчёт вверх (`--level corp`);
//! - [`rules_report`] — отчёт по реестру правил `CONSTRAINTS.yaml` (сводка,
//!   таблица карточек, находки: без owner/expiry, просроченные, `exclude_glob`,
//!   git-прокси стоимости сопровождения, суммарный `effort_hours`);
//! - [`significance_score`] — по ответам на 15 триггеров → Score + Route
//!   (пороги — [`significance_score_with_limits`], дефолты
//!   [`DEFAULT_FAST_MAX`]/[`DEFAULT_STANDARD_MAX`], ADR-034);
//! - [`detect_diff_triggers`] — механический anti-bypass floor (ADR-034):
//!   вывод триггеров значимости из git-диффа; [`score_with_sources`] —
//!   fail-safe объединение заявленных и найденных триггеров.
//!
//! Разбиение модуля (волна B1): `types` — типы данных контракта; `rules` —
//! модель и разбор `CONSTRAINTS.yaml`; `registry` — наследование/антидрейф
//! реестра; `exec` — исполнение правил; `report` — отчёты и линтеры;
//! `diff_triggers` — значимость и anti-bypass; `templates` — шаблон ADR;
//! `tools` — агентные инструменты; `baseline` — ratchet-режим.

/// Baseline-режим (ratchet) для brownfield и срез изменённых файлов
/// (`--baseline`/`--baseline-update`/`--changed-since`) — см. модуль.
pub mod baseline;
mod diff_triggers;
mod exec;
mod registry;
pub(crate) mod report;
mod rules;
mod templates;
mod tools;
mod types;

pub use diff_triggers::{
    CONNECT_MANIFEST_PATH, DEFAULT_FAST_MAX, DEFAULT_STANDARD_MAX, DiffGlobs, DiffTriggers,
    SIGNIFICANCE_TRIGGERS, SPINEIGNORE_PATH, ScoredTriggers, TriggerSource, base_rev,
    detect_diff_triggers, detect_diff_triggers_with, normalize_base_range, score_with_sources,
    significance_score, significance_score_with_limits, suggest_trigger, unknown_trigger_names,
    unknown_triggers_error,
};
pub(crate) use exec::glob_matches;
pub use exec::{check, check_with_options, command_strings};
pub use registry::{
    CONSTRAINTS_REGISTRY_ENV, ResolvedConstraints, ResolvedParent, RuleAnchor, check_anchored,
    default_anchor_base, expiry_is_past, load_constraints_resolved, rule_anchor, rule_weakened,
};
pub use report::{
    ControlReport, OverrideReportEntry, REQUIRED_SECTIONS, SensorResult, control_report,
    lint_spine, render_control_report, rules_report, sensors_check, spine_ad_ids,
};
pub use rules::{
    ConstraintsPathResolution, HANDOFF_CONSTRAINTS_PATH, ROOT_CONSTRAINTS_PATH,
    constraints_drift_note, load_fitness_rules, load_fitness_rules_with_skips,
    resolve_constraints_path, resolve_constraints_path_detailed, rule_cards,
};
pub(crate) use templates::kebab_slug;
pub use templates::{adr_new, adr_new_with_author};
pub use tools::{
    AdrNewTool, FitnessCheckTool, RulesReportTool, SignificanceScoreTool, SpineLintTool, tools,
};
pub use types::{
    BEHAVIOUR_RULE_KINDS, FitnessReport, FitnessRule, LintIssue, OverrideEntry, OverrideInfo,
    Route, RuleCard, RuleDuration, RulesFingerprint, RunnerSkippedRule, Significance,
    SkippedUnknownRule, SourceCount, UntrustedSkippedRule,
};
pub(crate) use types::{RuleKind, normalize_severity};
