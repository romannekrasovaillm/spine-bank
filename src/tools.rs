//! Ядерные инструменты: bash и файловые операции.
//!
//! КОНТРАКТ (владелец: агент `tools`):
//! - [`bash`] — выполнение shell-команд с таймаутом и лимитом вывода;
//! - [`fs`] — read/write/edit/glob/grep;
//! - [`ask`] — интерактивный выбор вариантов пользователем (`propose_options`);
//! - [`core_registry`] — реестр ядерных инструментов;
//! - [`full_registry`] — ядро + доменные инструменты (`mermaid::tools()`,
//!   `rubric::tools()`, `kb::tools()`, `control::tools()`,
//!   `openapi::tools()`, `model::tools()`, `trace::tools()` и др.;
//!   под фичей `harness` дополнительно `web::tools()`, `harness::tools()`,
//!   `subagent`, `ralph`, `worktree`, `distill`).

use std::sync::Arc;

use crate::config::Config;
use crate::tool::{Tool, ToolRegistry};

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

fn domain_tools(cfg: &Config) -> Vec<Arc<dyn Tool>> {
    let mut out: Vec<Arc<dyn Tool>> = Vec::new();
    out.extend(crate::mermaid::tools());
    // Archify-контур диаграмм (AD-1): при `[archify].enabled = false`
    // инструменты не регистрируются — гейт на уровне регистрации (как web).
    if cfg.archify.enabled {
        out.extend(crate::archify::tools());
    }
    out.extend(crate::rubric::tools());
    // Egress-дисциплина (AD-BE5, GAP-C1): при `[web].enabled = false` веб-канал
    // выключен конфигом — инструменты не регистрируются, агент их не видит.
    // Гейт живёт на уровне регистрации, сами web-инструменты о нём не знают (AD-4).
    // Модуль `web` собирается только под фичей `harness` (тащит reqwest/scraper).
    #[cfg(feature = "harness")]
    if cfg.web.enabled {
        out.extend(crate::web::tools());
    }
    out.extend(crate::kb::tools());
    out.extend(crate::control::tools());
    out.extend(crate::openapi::tools());
    out.extend(crate::asyncapi::tools());
    out.extend(crate::contract_diff::tools());
    out.extend(crate::model::tools());
    out.extend(crate::trace::tools());
    // Домены агентного цикла — только в сборке `harness` (инверсия, шаг 4):
    // кодовые харнессы, субагенты, ralph, worktree, дистилляция скиллов.
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
    #[cfg(feature = "harness")]
    out.extend(crate::distill::tools(cfg));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

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
            "skill_search",
            "mermaid_render",
            "archify_validate",
            "archify_deliver",
            "archify_show",
            "archify_compare",
            "model_query",
            "trace_check",
            "openapi_lint",
            "asyncapi_lint",
            "contract_diff",
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
}
