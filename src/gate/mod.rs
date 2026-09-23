//! Единый архитектурный гейт репозитория (`arch-be gate`) — одна команда,
//! прогоняющая детерминированный контур контроля (AD-2, AD-9) целиком и
//! сводящая исходы в один exit-код: провал ЛЮБОЙ составляющей → exit 1
//! (механически, без разбора строк вывода — строки для людей, код для CI
//! и хуков `arch-be connect`).
//!
//! Составляющие (на любом маршруте): fitness (`control check`), гейт прямых
//! правок спайна (`delta guard`), анти-ослабление реестра правил
//! ([`crate::control::rule_weakened`]), линтер спайна (`control spine`), трассировка
//! (`trace check`). На маршрутах Standard/Critical добавляются сенсоры
//! спецификаций ([`crate::control::sensors_check`] по `<repo>/docs/spec`),
//! количественные NFR (все четыре проверки `nfr`) и проверка evidence-бандлов
//! активных дельт (`changes/<name>/EVIDENCE.yaml`).
//!
//! Fail-soft (статус SKIP, не падение): у составляющей нет входа — нет
//! `CONSTRAINTS.yaml`, не git-репозиторий, нет `model/`, нет активных
//! бандлов. Сбой выполнения при НАЛИЧИИ входа (битый YAML, нерабочее правило)
//! — FAIL с причиной: гейт, молча пропускающий поломку собственной
//! конфигурации, не гейт (антикейс бэклога: агент под давлением «зеленеет»
//! правкой `CONSTRAINTS.yaml` — `rule_weakened` это ловит).
//!
//! Маршрут: `--route auto` (дефолт) вычисляет маршрут механически из
//! git-диффа ([`crate::control::detect_diff_triggers`] + [`crate::control::score_with_sources`]
//! с пустым declared — тот же anti-bypass floor S-1, что у MCP
//! `significance_from_diff`); дифф недоступен (не git, нет HEAD) — fail-safe
//! маршрут Critical. Явный `--route fast|standard|critical` переопределяет
//! авто-режим.
//!
//! Вывод: текстовый рендер — [`render`]; машинные форматы для CI
//! (`--format sarif|junit|gitlab-codequality|markdown`, вывод в stdout для
//! редиректа в файл-артефакт) — модуль [`crate::report_fmt`] поверх
//! структурированных находок [`GateFinding`].
//!
//! Разбиение модуля (волна B1): `types` — типы вердикта (статусы, находки,
//! отчёт, аттестация); `git` — обвязка git и пути реестра; `components` —
//! составляющие контура контроля; `semantic` — составляющая
//! `semantic_quality`; `route` — маршрут (`auto` из диффа, ROUTE.lock);
//! `verdict` — сборка вердикта (`run*`); `attest` — сверка конверта
//! вердикта (`--verify-envelope`); `explain` — текстовый рендер.

mod attest;
mod components;
mod explain;
mod git;
mod route;
mod semantic;
#[cfg(test)]
mod testkit;
mod types;
mod verdict;

pub use attest::{EnvelopeDrift, verify_envelope};
pub(crate) use components::adr_is_accepted;
pub use explain::render;
pub use types::{
    GateComponent, GateFinding, GateOptions, GateOutcome, GateReport, GateRequirements, GateStatus,
};
pub use verdict::{run, run_opts, run_with};
