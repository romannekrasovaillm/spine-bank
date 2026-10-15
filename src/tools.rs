//! Ядерные инструменты: bash и файловые операции.
//!
//! КОНТРАКТ (владелец: агент `tools`):
//! - [`bash`] — выполнение shell-команд с таймаутом и лимитом вывода;
//! - [`fs`] — read/write/edit/glob/grep;
//! - [`ask`] — интерактивный выбор вариантов пользователем (`propose_options`);
//! - [`core_registry`] — реестр ядерных инструментов;
//! - [`full_registry`] — ядро + доменные инструменты (`mermaid::tools()`,
//!   `rubric::tools()`, `kb::tools()`, `control::tools()`,
//!   `openapi::tools()`, `model::tools()`, `trace::tools()`,
//!   `handoff::tools()` и др.; под фичей `harness` дополнительно
//!   `web::tools()`, `harness::tools()` (прогон `harness_run`), `subagent`,
//!   `ralph`, `worktree`, `distill`).

use std::collections::HashSet;
use std::fmt::Write as _;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{Value, json};

use crate::config::Config;
use crate::error::{HarnessError, Result};
use crate::llm::ToolSpec;
use crate::tool::{Tool, ToolContext, ToolOutput, ToolRegistry};

pub mod ask;
pub mod bash;
pub mod fs;
pub mod screenshot;

/// Реестр ядерных инструментов: bash, файлы, glob/grep, `propose_options`,
/// изображения (скриншот/чтение — нативная мультимодальность `deepseek-flash`).
#[must_use]
pub fn core_registry() -> ToolRegistry {
    let mut reg = ToolRegistry::new()
        .with(Arc::new(bash::BashTool))
        .with(Arc::new(fs::ReadFileTool))
        .with(Arc::new(fs::WriteFileTool))
        .with(Arc::new(fs::EditFileTool))
        .with(Arc::new(fs::GlobTool))
        .with(Arc::new(fs::GrepTool))
        .with(Arc::new(ask::ProposeOptionsTool));
    for tool in screenshot::tools() {
        reg.register(tool);
    }
    reg
}

/// Полный реестр: ядро + специализированные инструменты архитектора.
/// Политика автономии — из `Config::policy` (R-уровни).
#[must_use]
pub fn full_registry(cfg: &Config) -> ToolRegistry {
    let mut reg = core_registry();
    for tool in domain_tools(cfg) {
        reg.register(tool);
    }
    let policy = crate::policy::Policy::parse(&cfg.policy.autonomy).unwrap_or_default();
    reg.with_policy(policy)
}

/// Доменные инструменты, снимаемые профилем `orchestrate` (`[agent]
/// tools_profile`): оркестрационные сессии работают базовыми инструментами,
/// `harness_run` и `significance_score` (отчёт «две гипотезы», 2026-10-14:
/// 84% первого запроса — схемы всех 67 инструментов; вызваны хотя бы раз 12
/// из 51 доменного). Список — ЯВНЫЙ drop-list: инструмент, забытый сюда при
/// добавлении, остаётся видим (fail-open) — профиль никогда не скрывает
/// новую возможность молча.
const ORCHESTRATE_DROP: &[&str] = &[
    "adr_new",
    "adr_registry",
    "agentsmd_generate",
    "agentsmd_lint",
    "arch_diff",
    "archify_compare",
    "archify_deliver",
    "archify_show",
    "archify_validate",
    "architect_review",
    "asyncapi_lint",
    "bench_run",
    "change_impact",
    "contract_diff",
    "contracts",
    "cron",
    "delta_guard",
    "delta_propose",
    "evidence_pack",
    "evidence_verify",
    "fitness_check",
    "fleet_audit",
    "landscape_report",
    "mermaid_render",
    "model_drift",
    "model_graph",
    "model_query",
    "model_validate",
    "nfr_check",
    "openapi_lint",
    "openspec_coverage",
    "plugin_list",
    "recover",
    "report",
    "reverse_survey",
    "rubric_accept",
    "rubric_evaluate",
    "rubric_generate",
    "rubric_handover",
    "rubric_list",
    "rule_template_apply",
    "rule_template_list",
    "rule_template_show",
    "skill_distill",
    "spine_lint",
    "trace_check",
];

/// Реестр инструментов агентной сессии (TUI, `run`, субагенты, ralph):
/// полный реестр, профилированный `[agent] tools_profile` (рекомендация 3
/// отчёта «две гипотезы»). Мост MCP (`mcp serve`) сюда не ходит — у моста
/// свои белые списки (`mcp_server`), профиль сессии его не режет.
#[must_use]
pub fn session_registry(cfg: &Config) -> ToolRegistry {
    let registry = full_registry(cfg);
    match cfg.agent.tools_profile.as_deref().map(str::trim) {
        None | Some("" | "full" | "architect") => registry,
        Some("orchestrate") => registry.excluding(ORCHESTRATE_DROP),
        Some(other) => {
            // Опечатка в конфиге не должна ронять сессию — полный реестр
            // (поведение до появления профилей) и предупреждение в лог.
            tracing::warn!("[agent] tools_profile = '{other}' неизвестен — используется full");
            registry
        }
    }
}

fn domain_tools(cfg: &Config) -> Vec<Arc<dyn Tool>> {
    let mut out: Vec<Arc<dyn Tool>> = Vec::new();
    out.extend(crate::mermaid::tools());
    // Archify-контур диаграмм (AD-1): при `[archify].enabled = false`
    // инструменты не регистрируются — гейт на уровне регистрации (как web).
    if cfg.archify.enabled {
        out.extend(crate::archify::tools());
    }
    out.extend(crate::rubric::tools());
    // Передача и приёмка судейства (ADR-048): read-only, в мост MCP отдаются
    // белым списком `BRIDGE_READ_ONLY` (src/mcp_server.rs).
    out.extend(crate::judge::tools());
    // Шаблоны исполняемых правил: list/show — read-only, apply — пишущий
    // (мост MCP отдаёт его только под `--rw`, ADR-050).
    out.extend(crate::rule_templates::tools());
    // Egress-дисциплина (AD-BE5, GAP-C1): при `[web].enabled = false` веб-канал
    // выключен конфигом — инструменты не регистрируются, агент их не видит.
    // Гейт живёт на уровне регистрации, сами web-инструменты о нём не знают (AD-4).
    // Модуль `web` собирается только под фичей `harness` (тащит reqwest/scraper).
    #[cfg(feature = "harness")]
    if cfg.web.enabled {
        out.extend(crate::web::tools());
    }
    out.extend(crate::kb::tools());
    // Префлайт-подбор уроков под задачу (L4): read-only, детерминированный
    // отбор из реестра — ядро без LLM (AD-2).
    out.extend(crate::lessons::matching::tools());
    out.extend(crate::control::tools());
    out.extend(crate::openapi::tools());
    out.extend(crate::asyncapi::tools());
    out.extend(crate::contract_diff::tools());
    out.extend(crate::model::tools());
    out.extend(crate::trace::tools());
    // Количественные NFR, дельта-протокол и evidence-бандлы (транш 1
    // инверсии — read-only верификаторы + пишущие pack/propose; мост MCP
    // отдаёт их по белым спискам `mcp_server.rs`).
    out.extend(crate::nfr::tools());
    out.extend(crate::delta::tools());
    out.extend(crate::evidence::tools());
    // Отчётный контур (транш 2 инверсии — read-only реестры и отчёты;
    // мост MCP отдаёт их по белому списку `mcp_server.rs`).
    out.extend(crate::landscape::tools());
    out.extend(crate::adr_registry::tools());
    out.extend(crate::openspec::tools());
    // Составные инструменты (транш 3 инверсии): единое ревью и радиус
    // изменения — read-only, мост MCP по белому списку.
    out.extend(crate::review::tools());
    // Генерация handoff-пакета — чисто файловая: в обеих сборках (core и
    // harness). Прогон пакета кодовым харнессом (harness_run) и домены
    // агентного цикла — только в сборке `harness` (инверсия, шаг 4;
    // handoff_create в core — волна 2, п.10).
    out.extend(crate::handoff::tools(cfg));
    #[cfg(feature = "harness")]
    out.extend(crate::harness::tools(cfg));
    out.extend(crate::plugin::tools(cfg));
    out.extend(crate::agentsmd::tools(cfg));
    #[cfg(feature = "harness")]
    out.extend(crate::subagent::tools(cfg));
    #[cfg(feature = "harness")]
    out.extend(crate::ralph::tools(cfg));
    #[cfg(feature = "harness")]
    out.push(Arc::new(crate::worktree::WorktreeNewTool));
    out.push(Arc::new(crate::fleet::FleetAuditTool));
    out.extend(crate::survey::tools());
    out.extend(crate::arch_diff::tools());
    // Отчёт о восстановленной архитектуре (ADR-080 + ADR-083): read-only мост
    // к `src/report/`, отдаётся MCP по белому списку `BRIDGE_READ_ONLY`.
    out.extend(crate::report::tools());
    #[cfg(feature = "harness")]
    out.extend(crate::distill::tools(cfg));
    out
}

/// Имена ядра (ADR-090) — всегда в запросе для ленивых профилей. Отсутствующие
/// в реестре имена (например, `harness_run` в core-сборке) отсекаются
/// пересечением с фактическим реестром при сборке профиля.
const CORE_EAGER: &[&str] = &[
    "bash",
    "read_file",
    "write_file",
    "edit_file",
    "glob",
    "grep",
    "propose_options",
    "harness_run",
    "lessons_match",
    "tool_search",
];

/// Имена контрольного контура (профиль `architect`, ADR-090): детерминированный
/// слой контроля без LLM (AD-2) — всегда в запросе у архитектора.
const ARCHITECT_EAGER: &[&str] = &[
    "adr_new",
    "spine_lint",
    "fitness_check",
    "significance_score",
    "rules_report",
];

/// Ленивый tool-профиль сессии (ADR-090): какие схемы всегда в запросе,
/// остальные доменные — по требованию через `tool_search`. Профиль `full`
/// (как раньше) и `reverse` (ADR-083 п.3) ленивой загрузки не используют —
/// там полный реестр.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolProfile {
    /// Ядро + `significance_score` (оркестрация исполнителей).
    Orchestrate,
    /// Ядро + контрольный контур (AD-2).
    Architect,
}

/// Разделяемое состояние ленивой загрузки доменных схем (ADR-090): каталог
/// доменных инструментов для `tool_search` и кэш уже загруженных имён. Кэш
/// живёт до конца сессии; его пишут `tool_search` (найденные) и агентный цикл
/// (вызванный незагруженный доменный инструмент), читает — сборка запроса.
#[derive(Clone, Default)]
pub struct ToolLazy {
    /// Каталог доменных инструментов (имя, описание, параметры).
    catalog: Arc<Vec<ToolSpec>>,
    /// Имена доменных инструментов, чьи схемы уже в запросе.
    loaded: Arc<std::sync::Mutex<HashSet<String>>>,
}

impl ToolLazy {
    /// Состояние с доменным каталогом; кэш загрузки пуст.
    #[must_use]
    pub fn new(catalog: Vec<ToolSpec>) -> Self {
        Self {
            catalog: Arc::new(catalog),
            loaded: Arc::new(std::sync::Mutex::new(HashSet::new())),
        }
    }

    /// Снимок загруженных имён (для сборки запроса).
    #[must_use]
    pub fn loaded_names(&self) -> HashSet<String> {
        match self.loaded.lock() {
            Ok(guard) => guard.clone(),
            // Отравление мьютекса невозможно в штатном потоке (под замком нет
            // паник); при нём отдаём всё, что успели загрузить.
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }

    /// Помечает инструмент загруженным (идемпотентно).
    pub fn mark_loaded(&self, name: &str) {
        match self.loaded.lock() {
            Ok(mut guard) => {
                guard.insert(name.to_string());
            }
            Err(poisoned) => {
                poisoned.into_inner().insert(name.to_string());
            }
        }
    }

    /// Детерминированный поиск доменных инструментов по имени/описанию
    /// (без регистра; пустой запрос — пустой результат). Найденные схемы
    /// помечаются загруженными. Результат отсортирован по имени.
    #[must_use]
    pub fn search(&self, query: &str) -> Vec<ToolSpec> {
        let q = query.trim().to_lowercase();
        if q.is_empty() {
            return Vec::new();
        }
        let mut matched: Vec<ToolSpec> = self
            .catalog
            .iter()
            .filter(|s| {
                s.name.to_lowercase().contains(&q) || s.description.to_lowercase().contains(&q)
            })
            .cloned()
            .collect();
        matched.sort_by(|a, b| a.name.cmp(&b.name));
        let mut loaded = match self.loaded.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        for spec in &matched {
            loaded.insert(spec.name.clone());
        }
        matched
    }
}

/// Инструмент `tool_search` (ADR-090): детерминированный поиск доменных
/// инструментов. Найденные схемы подгружаются в запрос (кэш до конца сессии).
struct ToolSearchTool {
    lazy: ToolLazy,
}

#[async_trait]
impl Tool for ToolSearchTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "tool_search".into(),
            description: "Найти доменный инструмент архитектора по имени или описанию \
                (например, «дифф контрактов», «модель», «рубрика»). Найденные схемы \
                подгружаются в запрос и остаются доступными до конца сессии — после \
                поиска вызывай их по имени. Ищи инструмент, только если нужного нет \
                в уже доступном списке."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Имя или описание инструмента (подстрока, без регистра)"
                    }
                },
                "required": ["query"]
            }),
        }
    }

    async fn call(&self, args: Value, _ctx: &ToolContext) -> Result<ToolOutput> {
        let query = args
            .get("query")
            .and_then(|q| q.as_str())
            .ok_or_else(|| HarnessError::Tool("tool_search: нет аргумента query".into()))?;
        let matched = self.lazy.search(query);
        if matched.is_empty() {
            return Ok(ToolOutput::ok(
                "Ничего не найдено. Уточните запрос: ищите по имени инструмента или \
                 ключевому слову из его назначения (например, «контракт», «модель», \
                 «рубрика»).",
            ));
        }
        let mut out = String::new();
        let _ = writeln!(
            out,
            "Найдено инструментов: {}. Их схемы подгружены в запрос (до конца сессии).",
            matched.len()
        );
        for spec in &matched {
            let _ = writeln!(out, "\n### {}\n{}", spec.name, spec.description);
        }
        Ok(ToolOutput::ok(out))
    }
}

/// Собранный ленивый инструментарий (ADR-090): полный реестр для
/// диспетчеризации, имена ядра/профиля (всегда в запросе) и разделяемое
/// состояние загрузки доменных схем.
#[derive(Clone)]
pub struct LazyToolset {
    registry: ToolRegistry,
    eager: HashSet<String>,
    lazy: ToolLazy,
}

impl LazyToolset {
    /// Полный реестр (диспетчеризация всех инструментов, включая незагруженные).
    #[must_use]
    pub fn registry(&self) -> &ToolRegistry {
        &self.registry
    }

    /// Имена, чьи схемы всегда в запросе (ядро + профиль).
    #[must_use]
    pub fn eager(&self) -> &HashSet<String> {
        &self.eager
    }

    /// Разделяемое состояние ленивой загрузки.
    #[must_use]
    pub fn lazy(&self) -> &ToolLazy {
        &self.lazy
    }

    /// Спеки, которые попадут в запрос сейчас: ядро + загруженные доменные.
    #[must_use]
    pub fn active_specs(&self) -> Vec<ToolSpec> {
        let loaded = self.lazy.loaded_names();
        self.registry
            .specs()
            .into_iter()
            .filter(|s| self.eager.contains(&s.name) || loaded.contains(&s.name))
            .collect()
    }

    /// Разбирает на части (для передачи в сессию: реестр, ядро, кэш).
    #[must_use]
    pub fn into_parts(self) -> (ToolRegistry, HashSet<String>, ToolLazy) {
        (self.registry, self.eager, self.lazy)
    }
}

/// Собирает ленивый инструментарий для профиля `orchestrate`/`architect`
/// (ADR-090). Профили `full`/`reverse` сюда не передаются — они используют
/// [`full_registry`] без ленивой загрузки.
#[must_use]
pub fn lazy_toolset(cfg: &Config, profile: ToolProfile) -> LazyToolset {
    let mut registry = full_registry(cfg);
    let mut eager = eager_names(profile, &registry);
    // Каталог tool_search — всё, что не в ядре/профиле: доменные схемы и
    // скриншоты подгружаются по требованию, а не раздувают каждый запрос.
    let catalog: Vec<ToolSpec> = registry
        .specs()
        .into_iter()
        .filter(|s| !eager.contains(&s.name))
        .collect();
    let lazy = ToolLazy::new(catalog);
    // `tool_search` регистрируется после сборки каталога (сам не доменный) и
    // входит в ядро каждого ленивого профиля.
    registry.register(Arc::new(ToolSearchTool { lazy: lazy.clone() }));
    eager.insert("tool_search".to_string());
    LazyToolset {
        registry,
        eager,
        lazy,
    }
}

/// Имена, всегда входящие в запрос для профиля (пересечение с реестром: в
/// core-сборке `harness_run` отсутствует и отсекается).
fn eager_names(profile: ToolProfile, registry: &ToolRegistry) -> HashSet<String> {
    let mut names: HashSet<String> = CORE_EAGER.iter().map(|s| (*s).to_string()).collect();
    match profile {
        ToolProfile::Orchestrate => {
            names.insert("significance_score".to_string());
        }
        ToolProfile::Architect => {
            names.extend(ARCHITECT_EAGER.iter().map(|s| (*s).to_string()));
        }
    }
    names.retain(|n| registry.get(n).is_some());
    names
}

/// Прокси размера первого запроса (символов): системный промпт плюс сумма
/// длин схем (имя + описание + параметры). Нижняя граница честного замера —
/// обёртка `{"type":"function",…}` и имена полей в неё не включены, но они
/// короче самих описаний и не меняют порядок величины.
#[must_use]
pub fn first_request_chars(system_prompt: &str, specs: &[ToolSpec]) -> usize {
    system_prompt.chars().count()
        + specs
            .iter()
            .map(|s| {
                s.name.chars().count()
                    + s.description.chars().count()
                    + s.parameters.to_string().chars().count()
            })
            .sum::<usize>()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Имена ядра, ожидаемые в harness-сборке (без учёта `tool_search`, который
    /// `lazy_toolset` регистрирует отдельно, — но он входит в eager-набор).
    const ORCHESTRATE_EAGER: &[&str] = &[
        "bash",
        "read_file",
        "write_file",
        "edit_file",
        "glob",
        "grep",
        "propose_options",
        "harness_run",
        "lessons_match",
        "tool_search",
        "significance_score",
    ];

    fn names(specs: &[ToolSpec]) -> Vec<String> {
        let mut out: Vec<String> = specs.iter().map(|s| s.name.clone()).collect();
        out.sort();
        out
    }

    #[test]
    fn lazy_orchestrate_specs_are_core_plus_significance() {
        let cfg = Config::default();
        let toolset = lazy_toolset(&cfg, ToolProfile::Orchestrate);
        let specs = toolset.active_specs();
        // `harness_run` есть только в полной сборке — ожидание пересекаем с
        // фактическим реестром; `tool_search` регистрирует сам lazy_toolset,
        // поэтому он всегда присутствует.
        let full = full_registry(&cfg);
        let mut expected: Vec<String> = ORCHESTRATE_EAGER
            .iter()
            .filter(|n| **n == "tool_search" || full.get(n).is_some())
            .map(|s| (*s).to_string())
            .collect();
        expected.sort();
        assert_eq!(
            names(&specs),
            expected,
            "orchestrate: ядро + significance_score, ничего доменного"
        );
        // Доменные инструменты в orchestrate-запросе отсутствуют.
        for banned in ["model_query", "fitness_check", "arch_diff", "contract_diff"] {
            assert!(
                !specs.iter().any(|s| s.name == banned),
                "{banned} не должен входить в orchestrate-запрос"
            );
        }
    }

    #[test]
    fn lazy_architect_specs_add_control_contour() {
        let cfg = Config::default();
        let toolset = lazy_toolset(&cfg, ToolProfile::Architect);
        let specs = toolset.active_specs();
        let got = names(&specs);
        // Контрольный контур (AD-2) — всегда в запросе архитектора.
        for expected in [
            "adr_new",
            "spine_lint",
            "fitness_check",
            "significance_score",
            "rules_report",
        ] {
            assert!(
                got.contains(&expected.to_string()),
                "нет {expected} в architect"
            );
        }
        // Ядро тоже на месте (harness_run — только в полной сборке).
        for expected in ["bash", "read_file", "lessons_match", "tool_search"] {
            assert!(
                got.contains(&expected.to_string()),
                "нет {expected} в architect"
            );
        }
        #[cfg(feature = "harness")]
        assert!(
            got.contains(&"harness_run".to_string()),
            "нет harness_run в architect"
        );
        // А доменные (не контрольный контур) — по-прежнему ленивы.
        for banned in ["model_query", "mermaid_render", "openapi_lint", "arch_diff"] {
            assert!(
                !got.contains(&banned.to_string()),
                "{banned} не должен входить в architect-запрос"
            );
        }
    }

    #[test]
    fn full_registry_keeps_all_specs_without_lazy() {
        // Профиль full (дефолт) ленивой загрузки не использует: реестр полный.
        let cfg = Config::default();
        let full = full_registry(&cfg);
        let toolset = lazy_toolset(&cfg, ToolProfile::Orchestrate);
        assert!(
            full.specs().len() > toolset.active_specs().len(),
            "full-реестр шире orchestrate-запроса"
        );
    }

    #[test]
    fn tool_search_is_deterministic_and_marks_loaded() {
        let cfg = Config::default();
        let toolset = lazy_toolset(&cfg, ToolProfile::Orchestrate);
        let lazy = toolset.lazy().clone();
        let first = lazy.search("модель");
        let second = lazy.search("модель");
        assert!(!first.is_empty(), "поиск «модель» что-то находит");
        assert_eq!(
            names(&first),
            names(&second),
            "tool_search детерминирован (одинаковый результат на одинаковый запрос)"
        );
        // Найденные схемы помечены загруженными и попали в активные спеки.
        let loaded = lazy.loaded_names();
        for spec in &first {
            assert!(loaded.contains(&spec.name), "{} загружен", spec.name);
        }
        let active = toolset.active_specs();
        let active_names = names(&active);
        for spec in &first {
            assert!(
                active_names.contains(&spec.name),
                "{} в активных спеках",
                spec.name
            );
        }
    }

    #[test]
    fn tool_search_returns_nothing_for_empty_or_unknown() {
        let cfg = Config::default();
        let toolset = lazy_toolset(&cfg, ToolProfile::Orchestrate);
        let lazy = toolset.lazy();
        assert!(
            lazy.search("").is_empty(),
            "пустой запрос — пустой результат"
        );
        assert!(
            lazy.search("нет-такого-инструмента-xyzzy").is_empty(),
            "неизвестный запрос — пустой результат"
        );
    }

    #[test]
    fn mark_loaded_adds_unloaded_domain_spec() {
        let cfg = Config::default();
        let toolset = lazy_toolset(&cfg, ToolProfile::Orchestrate);
        assert!(
            !toolset
                .active_specs()
                .iter()
                .any(|s| s.name == "model_query"),
            "model_query изначально не в запросе"
        );
        // Вызов незагруженного доменного инструмента подгружает его (ADR-090).
        toolset.lazy().mark_loaded("model_query");
        assert!(
            toolset
                .active_specs()
                .iter()
                .any(|s| s.name == "model_query"),
            "после mark_loaded model_query в активных спеках"
        );
    }

    #[test]
    fn orchestrate_first_request_fits_25000_chars() {
        // Цель B6/ADR-090: первый запрос ≤ 25 000 симв. вместе с системным
        // промптом. База промпта — встроенный `architect.md` (ADR-090).
        let cfg = Config::default();
        let toolset = lazy_toolset(&cfg, ToolProfile::Orchestrate);
        let size = first_request_chars(crate::assets::PROMPT_ARCHITECT, &toolset.active_specs());
        assert!(
            size <= 25_000,
            "первый запрос orchestrate: {size} символов > 25 000"
        );
    }

    #[test]
    fn full_registry_contains_core_interaction_and_domain_tools() {
        let cfg = Config::default();
        let names = full_registry(&cfg).names();
        // Ядро и детерминированный контур — в обеих сборках (core и harness).
        for expected in [
            "bash",
            "read_file",
            "propose_options",
            "fleet_audit",
            "reverse_survey",
            "recover",
            "arch_diff",
            "report",
            "skill_search",
            "mermaid_render",
            "archify_validate",
            "archify_deliver",
            "archify_show",
            "archify_compare",
            "model_query",
            "model_validate",
            "model_graph",
            "model_drift",
            "trace_check",
            "openapi_lint",
            "asyncapi_lint",
            "contract_diff",
            "nfr_check",
            "delta_guard",
            "delta_propose",
            "evidence_verify",
            "evidence_pack",
            "landscape_report",
            "adr_registry",
            "rules_report",
            "openspec_coverage",
            "architect_review",
            "change_impact",
            "handoff_create",
            "lessons_match",
        ] {
            assert!(
                names.iter().any(|n| n == expected),
                "нет инструмента {expected}"
            );
        }
        // Домены агентного цикла — только в сборке `harness`.
        #[cfg(feature = "harness")]
        for expected in [
            "subagent_run",
            "subagent_list",
            "subagent_result",
            "ralph_run",
            "worktree_new",
            "skill_distill",
        ] {
            assert!(
                names.iter().any(|n| n == expected),
                "нет инструмента {expected}"
            );
        }
        // В core-сборке harness-инструментов нет (защита от протечки).
        #[cfg(not(feature = "harness"))]
        for banned in [
            "subagent_run",
            "subagent_list",
            "subagent_result",
            "ralph_run",
            "worktree_new",
            "skill_distill",
            "harness_run",
            "web_search",
            "web_fetch",
        ] {
            assert!(
                !names.iter().any(|n| n == banned),
                "инструмент {banned} не должен собираться в core"
            );
        }
    }

    #[test]
    #[cfg(feature = "harness")]
    fn full_registry_omits_web_tools_when_web_disabled() {
        // GAP-C1: при `[web].enabled = false` веб-инструменты отсутствуют в
        // инструментарии сессии — агент их не видит (egress-дисциплина, AD-BE5).
        let mut cfg = Config::default();
        cfg.web.enabled = false;
        let names = full_registry(&cfg).names();
        for banned in ["web_search", "web_fetch", "web_arch_sites"] {
            assert!(
                !names.iter().any(|n| n == banned),
                "инструмент {banned} не должен регистрироваться при [web].enabled=false"
            );
        }
    }

    #[test]
    #[cfg(feature = "harness")]
    fn full_registry_keeps_web_tools_by_default() {
        // Поведение по умолчанию не меняется: без ключа enabled веб-инструменты
        // регистрируются как раньше.
        let cfg = Config::default();
        let names = full_registry(&cfg).names();
        for expected in ["web_search", "web_fetch", "web_arch_sites"] {
            assert!(
                names.iter().any(|n| n == expected),
                "нет веб-инструмента {expected} при дефолтном конфиге"
            );
        }
    }

    #[test]
    #[cfg(feature = "harness")]
    fn reverse_profile_omits_web_tools() {
        // Профиль reverse (ADR-083 п.3): исходящий веб-канал выключен, веб-
        // инструменты не попадают в реестр сессии.
        let mut cfg = Config::default();
        cfg.apply_reverse_profile();
        assert!(!cfg.web.enabled, "профиль reverse выключает веб-канал");
        let names = full_registry(&cfg).names();
        for banned in ["web_search", "web_fetch", "web_arch_sites"] {
            assert!(
                !names.iter().any(|n| n == banned),
                "инструмент {banned} не должен регистрироваться в профиле reverse"
            );
        }
    }

    #[test]
    fn full_registry_omits_archify_tools_when_archify_disabled() {
        // Гейт регистрации (как у web): при `[archify].enabled = false`
        // archify-инструменты отсутствуют в инструментарии сессии.
        let mut cfg = Config::default();
        cfg.archify.enabled = false;
        let names = full_registry(&cfg).names();
        for banned in [
            "archify_validate",
            "archify_deliver",
            "archify_show",
            "archify_compare",
        ] {
            assert!(
                !names.iter().any(|n| n == banned),
                "инструмент {banned} не должен регистрироваться при [archify].enabled=false"
            );
        }
    }

    #[test]
    fn orchestrate_profile_drops_domain_tools_and_keeps_core() {
        // Отчёт «две гипотезы» (2026-10-14): в оркестрации 60% вызовов — 4
        // базовых инструмента, из 51 доменного использованы 12; профиль
        // orchestrate убирает доменный слой из схем каждого запроса.
        let mut cfg = Config::default();
        cfg.agent.tools_profile = Some("orchestrate".into());
        let names = session_registry(&cfg).names();
        for kept in [
            "bash",
            "read_file",
            "write_file",
            "edit_file",
            "glob",
            "grep",
            "handoff_create",
            "skill_search",
            "skill_load",
            "kb_search",
            "significance_score",
        ] {
            assert!(names.iter().any(|n| n == kept), "{kept} обязан остаться");
        }
        // Оркестрационные инструменты — только сборки harness (в core их нет).
        #[cfg(feature = "harness")]
        for kept in ["harness_run", "subagent_run", "ralph_run", "worktree_new"] {
            assert!(names.iter().any(|n| n == kept), "{kept} обязан остаться");
        }
        for dropped in [
            "fitness_check",
            "architect_review",
            "trace_check",
            "model_query",
            "delta_guard",
            "rubric_evaluate",
            "spine_lint",
            "openapi_lint",
            "nfr_check",
            "adr_registry",
            "mermaid_render",
        ] {
            assert!(
                !names.iter().any(|n| n == dropped),
                "{dropped} обязан быть снят профилем orchestrate"
            );
        }
    }

    #[test]
    fn full_architect_and_unknown_profiles_keep_full_registry() {
        for value in [None, Some("full"), Some("architect"), Some("orkestrate")] {
            let mut cfg = Config::default();
            cfg.agent.tools_profile = value.map(str::to_string);
            let names = session_registry(&cfg).names();
            assert!(
                names.iter().any(|n| n == "fitness_check"),
                "при tools_profile={value:?} реестр обязан быть полным"
            );
        }
    }
}
