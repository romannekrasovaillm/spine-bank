//! Подкоманды `arch-be mcp` и их обработчик: список серверов, вызовы
//! инструментов, MCP-сервер (B1: выделено из `main.rs`).

use std::sync::Arc;

use anyhow::{Context, Result};
use clap::Subcommand;

use arch_harness::config::Config;

#[derive(Subcommand)]
pub(crate) enum McpCmd {
    /// Список серверов и их инструментов.
    List,
    /// Вызвать MCP-инструмент.
    Call {
        /// Составное имя `server__tool`.
        name: String,
        /// Аргументы JSON.
        #[arg(default_value = "{}")]
        args: String,
    },
    /// MCP-сервер (stdio JSON-RPC, NDJSON): архитектурный контроль кодовым
    /// агентам (Claude Code и др.), ADR-008. Read-only состав: 40 инструментов
    /// + 10 промптов-плейбуков spine-* (capability prompts). Ручные (16):
    ///   `spine_lint`, `fitness_check`, `significance_score`,
    ///   `significance_from_diff`, `trace_check`, `model_query`, `rubric_run`,
    ///   `rubric_prompt`, `rubric_verify`, `kb_search`, `skill_search`,
    ///   `skill_load`, `mermaid_render`, `rules_suggest`, `trust_report`,
    ///   `verdict_explain`. Мостовые read-only
    ///   (24): `adr_registry`, `agentsmd_lint`, `archify_validate`,
    ///   `architect_review`, `asyncapi_lint`, `change_impact`, `contract_diff`,
    ///   `delta_guard`, `evidence_verify`, `fleet_audit`, `landscape_report`,
    ///   `model_drift`, `model_graph`, `model_validate`, `nfr_check`,
    ///   `openapi_lint`, `openspec_coverage`, `plugin_list`, `rubric_accept`,
    ///   `rubric_handover`, `rubric_list`, `rule_template_list`,
    ///   `rule_template_show`, `rules_report`.
    Serve {
        /// Открыть rw-контур моста (аддитивные записи в рабочий каталог
        /// клиента: `adr_new`, `agentsmd_generate`, `archify_compare`,
        ///   `archify_deliver`, `archify_show`, `delta_propose`, `evidence_pack`,
        ///   `handoff_create`, `reverse_survey`, `rule_template_apply`,
        ///   `skill_distill`). По умолчанию
        /// сервер строго read-only.
        ///
        /// `--rw=reports` — узкий режим для судейского харнесса: запись
        /// разрешена только отчётам рубрики (`rubric_verify` →
        /// `reports/rubric/`), остальной белый список закрыт.
        #[arg(long, value_name = "full|reports", num_args = 0..=1, default_missing_value = "full")]
        rw: Option<String>,
    },
}

pub(crate) async fn cmd_mcp(cfg: &Arc<Config>, cmd: McpCmd) -> Result<()> {
    // Серверный режим (P1-2, ADR-008) обслуживает клиентов и не подключается
    // к серверам: mcp.json для него не требуется, уходим до его загрузки.
    if let McpCmd::Serve { rw } = &cmd {
        let mode = arch_harness::mcp_server::ServeMode::parse_rw(rw.as_deref())
            .map_err(anyhow::Error::msg)?;
        return arch_harness::mcp_server::serve_with_mode(Arc::clone(cfg), mode)
            .await
            .context("MCP-сервер (stdio)");
    }
    let mut servers = arch_harness::mcp::load_servers(&cfg.mcp.servers_file)
        .with_context(|| format!("чтение {}", cfg.mcp.servers_file.display()))?;
    // Плагины тоже несут MCP-серверы (стандарт: plugin.json mcpServers / .mcp.json).
    if cfg.plugins.include_mcp {
        let plugins = arch_harness::plugin::discover(&cfg.plugins.dirs);
        servers.extend(arch_harness::plugin::mcp_servers(&plugins));
    }
    let manager =
        Arc::new(arch_harness::mcp::McpManager::connect(&servers, cfg.mcp.timeout_secs).await?);
    match cmd {
        McpCmd::List => {
            println!("Серверы: {}", manager.server_names().join(", "));
            for spec in manager.tools().await {
                println!("  {:<40} {}", spec.name, spec.description);
            }
        }
        McpCmd::Call { name, args } => {
            let args: serde_json::Value =
                serde_json::from_str(&args).context("невалидный JSON аргументов")?;
            let out = manager.call(&name, args).await?;
            println!("{}", out.content);
        }
        // Недостижимо: Serve обработан выше возвратом до подключения к серверам.
        McpCmd::Serve { .. } => {}
    }
    manager.shutdown().await;
    Ok(())
}
