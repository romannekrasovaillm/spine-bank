#![cfg(feature = "harness")]
//! Обработчики агентного запуска: `arch-be run` и системный промпт
//! по умолчанию (B1: выделено из `main.rs`; модуль есть только в сборке `harness`).

use std::io::{IsTerminal, Read as _, Write as _};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};

use arch_harness::agent::AgentSession;
use arch_harness::config::Config;
use arch_harness::llm::LlmRegistry;
use arch_harness::tool::ToolContext;

/// Опции headless-прогона `arch-be run` (бюджеты).
#[cfg(feature = "harness")]
pub(crate) struct RunOptions {
    /// Общий таймаут прогона, секунды.
    pub(crate) timeout_secs: Option<u64>,
    /// Переопределение `agent.max_tool_turns` на прогон.
    pub(crate) max_turns: Option<u64>,
}

/// `arch-be run`: headless агент.
///
/// Строгий режим (`--quiet`, как `dsh --profile headless` у `DeepSeek`
/// Harness) = без стриминга: stdout несёт ТОЛЬКО финальный ответ ассистента
/// (пригоден для пайпов), события хода молчат; пустая задача отклоняется
/// до запуска; сбой — причина в stderr и ненулевой код выхода.
///
/// Бюджеты: `--timeout SECS` — общий потолок прогона (превышение — причина
/// в stderr и exit 1), `--max-turns N` — лимит итераций инструментов
/// (перекрывает `agent.max_tool_turns` на клоне конфига).
///
/// В стрим-режиме stdout несёт только текст ответа (дельты); прогресс
/// (вызовы инструментов, заметки) уходит в stderr — пайп остаётся чистым.
#[cfg(feature = "harness")]
pub(crate) async fn cmd_run(
    cfg: &Arc<Config>,
    prompt: Option<String>,
    model: Option<String>,
    stream: bool,
    think: Option<String>,
    opts: RunOptions,
) -> Result<()> {
    let input = match prompt.as_deref() {
        Some("-") | None if !std::io::stdin().is_terminal() => {
            let mut buf = String::new();
            std::io::stdin()
                .read_to_string(&mut buf)
                .context("чтение stdin")?;
            buf
        }
        Some(p) => p.to_string(),
        None => anyhow::bail!("нет промпта: передайте аргумент или пайп в stdin"),
    };
    if input.trim().is_empty() {
        anyhow::bail!("пустая задача: передайте непустой промпт аргументом или пайпом в stdin");
    }
    let thinking = match think.as_deref() {
        Some("on") => Some(true),
        Some("off") => Some(false),
        Some(other) => anyhow::bail!("--think: ожидается on|off, получено '{other}'"),
        None => None,
    };

    // Переопределение бюджета итераций — на клоне конфига, глобальный не трогаем.
    let cfg = match opts.max_turns {
        Some(n) => {
            let mut owned = (**cfg).clone();
            owned.agent.max_tool_turns =
                usize::try_from(n).context("--max-turns: значение не помещается в usize")?;
            Arc::new(owned)
        }
        None => cfg.clone(),
    };

    let registry = Arc::new(LlmRegistry::from_config(&cfg)?);
    let provider = match &model {
        Some(name) => registry.get(name)?,
        None => registry.default(),
    };
    let tools = arch_harness::tools::full_registry(&cfg);
    let cwd = std::env::current_dir().context("cwd")?;
    let tool_ctx = ToolContext::new(cwd, cfg.clone())
        .with_llm(registry.clone())
        .with_provider(provider.clone())
        .with_subagents(arch_harness::subagent::SubagentRegistry::new());
    let system = default_system_prompt(&cfg);
    let mut session = AgentSession::new(cfg.clone(), provider, tools, tool_ctx, system);
    session.set_thinking(thinking);

    let send = async {
        if stream {
            let (tx, mut rx) = tokio::sync::mpsc::channel(64);
            let printer = tokio::spawn(async move {
                use arch_harness::agent::AgentEvent;
                while let Some(ev) = rx.recv().await {
                    // StdoutLock/StderrLock не Send — лочим на каждое событие,
                    // не через await.
                    match ev {
                        AgentEvent::Delta(text) => {
                            let mut out = std::io::stdout().lock();
                            let _ = out.write_all(text.as_bytes());
                            let _ = out.flush();
                        }
                        // «Мысли» — приглушённо в stderr (прогресс-канал).
                        AgentEvent::ReasoningDelta(text) => {
                            let mut err = std::io::stderr().lock();
                            let _ = write!(err, "\x1b[2m{text}\x1b[0m");
                            let _ = err.flush();
                        }
                        // Прогресс — в stderr: stdout пайпа несёт только ответ.
                        AgentEvent::ToolStart { name, .. } => {
                            let _ =
                                writeln!(std::io::stderr().lock(), "\x1b[2m▶ tool: {name}\x1b[0m");
                        }
                        AgentEvent::ToolEnd {
                            name,
                            is_error,
                            summary,
                            ..
                        } => {
                            let mark = if is_error { "✗" } else { "✓" };
                            let _ = writeln!(
                                std::io::stderr().lock(),
                                "\x1b[2m{mark} {name}: {summary}\x1b[0m"
                            );
                        }
                        AgentEvent::Note(text) => {
                            let _ = writeln!(std::io::stderr().lock(), "\x1b[2m» {text}\x1b[0m");
                        }
                        AgentEvent::TurnDone => {
                            let _ = writeln!(std::io::stdout().lock());
                        }
                        // Телеметрия индикатора контекста — только для TUI.
                        AgentEvent::ContextUsage(_) => {}
                    }
                }
            });
            let r = session.send(&input, Some(tx)).await;
            let _ = printer.await;
            r.map_err(anyhow::Error::from)
        } else {
            session
                .send(&input, None)
                .await
                .map_err(anyhow::Error::from)
        }
    };
    let reply = match opts.timeout_secs {
        Some(secs) => match tokio::time::timeout(Duration::from_secs(secs), send).await {
            Ok(r) => r?,
            Err(_) => anyhow::bail!(
                "таймаут прогона ({secs}с): провайдер или инструмент не ответил вовремя"
            ),
        },
        None => send.await?,
    };
    if !stream {
        // Печать через writeln с игнорированием BrokenPipe: `arch-be run -q … | head`
        // обрывает stdout — для пайпа это норма, а не повод для паники println!.
        let mut out = std::io::stdout().lock();
        let _ = writeln!(out, "{reply}");
        let _ = out.flush();
    }
    Ok(())
}

/// Системный промпт по умолчанию: из библиотеки промптов или встроенный,
/// дополненный глобальной md-памятью (`paths.memory_file`, см. `memory`).
#[cfg(feature = "harness")]
fn default_system_prompt(cfg: &Config) -> String {
    let dir = cfg.paths.prompts_dir();
    let base = match arch_harness::agent::prompts::load_library(&dir) {
        Ok(lib) => match lib.iter().find(|t| t.name == "architect") {
            Some(tpl) => tpl.body.clone(),
            None => fallback_system_prompt(),
        },
        Err(_) => fallback_system_prompt(),
    };
    // Ошибка чтения памяти не фатальна: сессия работает без неё.
    let memory = arch_harness::memory::load(&cfg.paths.memory_file)
        .ok()
        .flatten();
    arch_harness::memory::augment_system_prompt(&base, memory.as_deref(), &cfg.paths.memory_file)
}

/// Встроенный системный промпт (fallback, когда библиотека недоступна).
#[cfg(feature = "harness")]
fn fallback_system_prompt() -> String {
    "Ты — solution-архитектор в корпоративном контуре банка. Помогаешь проектировать \
     решения, ведёшь ADR и architecture-spine, оцениваешь архитектуру по рубрикам, \
     готовишь handoff-пакеты кодовым агентам. Отвечай по-русски, точно и по делу."
        .into()
}
