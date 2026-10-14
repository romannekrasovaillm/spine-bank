//! Агентный цикл харнесса: сообщения ↔ вызовы инструментов, слэш-команды,
//! библиотека промптов, append-only журнал сессии (memlog-паттерн).
//!
//! КОНТРАКТ (владелец: агент `agent`):
//! - [`AgentSession`] — цикл: user → (LLM → `tool_calls` → `ToolRegistry::dispatch`
//!   → `tool_result`)* → финальный текст; лимит итераций из `AgentConfig`,
//!   при исчерпании — финальный ответ без инструментов (не ошибка);
//!   отмена хода по [`CancellationToken`] (TUI: Esc/Alt+Enter) — LLM-запрос
//!   или вызов инструмента обрывается, висячие tool-вызовы получают
//!   результат «прервано» (контракт tool-пар не нарушается);
//!   бюджет контекста: грубая оценка токенов ([`ChatMessage::rough_tokens`]),
//!   при переполнении — склейка старых tool-результатов (усечение) с пометкой;
//! - события [`AgentEvent`] через mpsc для TUI/стриминга в CLI;
//! - журнал сессии: JSONL append-only в `paths.sessions_dir` (ISO-метки,
//!   события user/assistant/tool/system) — воспроизводимость и resume;
//! - память сбоев инструментов (правило «ошибся дважды → урок»): на каждый
//!   `ToolOutput::err` считается подпись сбоя, повторная подпись порождает
//!   урок по режиму `[agent] failure_memory` (см. [`crate::failure_memory`]);
//! - [`slash`] — слэш-команды (см. файл); [`prompts`] — библиотека шаблонов.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::config::Config;
use crate::error::{HarnessError, Result};
use crate::llm::{ChatMessage, ChatRequest, LlmEvent, LlmProvider, Role, ToolPairRepair, Usage};
use crate::tool::{ToolContext, ToolOutput, ToolRegistry};

mod journal;
mod network;
pub mod prompts;
pub mod slash;
// Интеграционные тесты контракта tool-пар (ADR-084) вынесены в отдельный
// файл: agent.rs у порога лимита длины (C-33), а тесты не должны его пробить.
#[cfg(test)]
mod tool_pair_tests;

pub use journal::{SessionLogInfo, list_session_logs};
use journal::{now_iso, open_journal, summarize};

/// Таймаут интерактивного вопроса пользователю (`propose_options`): человеку
/// на архитектурное решение нужно больше пяти минут — час, не 300 секунд.
const ASK_TIMEOUT_SECS: u64 = 3600;
/// Максимум символов результата инструмента, сохраняемых в истории.
const TOOL_RESULT_MAX_CHARS: usize = 8192;
/// Длина tool-сообщения после усечения при компактификации.
const COMPACT_TOOL_CHARS: usize = 500;
/// Сколько последних tool-сообщений не усечь при компактификации.
const COMPACT_KEEP_TOOL: usize = 4;
/// Сколько последних сообщений не удалять при жёсткой компактификации.
const COMPACT_KEEP_TAIL: usize = 6;
/// Сколько символов пользовательского ввода уходит в BM25-отбор блока уроков
/// (ADR-087): длинный ввод — это вставленный контекст, а не запрос.
const LESSONS_QUERY_MAX_CHARS: usize = 1000;
/// Сколько раз подряд финальный ответ, оборванный потолком `max_tokens`
/// (`finish_reason=length`), дозапрашивается «продолжи с места обрыва».
const MAX_LENGTH_CONTINUATIONS: usize = 2;

/// Событие агентного цикла (для TUI/CLI-стриминга).
#[derive(Debug, Clone)]
pub enum AgentEvent {
    /// Порция текста ответа.
    Delta(String),
    /// Порция `reasoning_content` («мысли» модели): TUI показывает компактным
    /// приглушённым блоком, в текст ответа не входит.
    ReasoningDelta(String),
    /// Начался вызов инструмента.
    ToolStart {
        /// Имя инструмента.
        name: String,
        /// Аргументы (JSON).
        args: Value,
    },
    /// Инструмент завершился.
    ToolEnd {
        /// Имя инструмента.
        name: String,
        /// Признак ошибки.
        is_error: bool,
        /// Краткий итог (первая строка вывода).
        summary: String,
        /// Полный вывод (после редакции секретов, БЕЗ усечения) — для вкладок
        /// правой панели (mermaid-арт показывается целиком). В историю и
        /// журнал уходит усечённая версия (`TOOL_RESULT_MAX_CHARS`) — бюджет
        /// контекста модели защищён отдельно от UI.
        content: String,
    },
    /// Служебная заметка (детекторы циклов, компактификация, хуки).
    Note(String),
    /// Текущая оценка токенов истории — живое обновление индикатора
    /// контекста в UI по ходу длинного хода (не дожидаясь `TurnFinished`).
    ContextUsage(usize),
    /// Ход завершён.
    TurnDone,
}

/// Сессия агента: история сообщений + реестр инструментов + провайдер.
pub struct AgentSession {
    /// Конфигурация харнесса.
    config: Arc<Config>,
    /// Активный провайдер LLM.
    provider: Arc<dyn LlmProvider>,
    /// Реестр инструментов.
    tools: ToolRegistry,
    /// Контекст вызова инструментов.
    tool_ctx: ToolContext,
    /// Системный промпт (первым сообщением каждого запроса).
    system_prompt: String,
    /// Закреплённый блок уроков реестра (ADR-087): дописывается к системному
    /// промпту каждого запроса и **пересобирается из реестра** после
    /// компактификации — L1-маскирование и L3-саммари его не пересказывают,
    /// потому что блока в истории нет вовсе (закрывает B5).
    lessons_block: String,
    /// Пути последнего отбора блока (из текста задачи) — пересборка
    /// повторяет отбор, а не «прилипает» к самому первому.
    lessons_paths: Vec<String>,
    /// Текст последнего запроса для BM25-отбора блока (усечённый).
    lessons_query: Option<String>,
    /// История хода (без системного сообщения).
    history: Vec<ChatMessage>,
    /// Путь к журналу сессии (None — журнал не удалось открыть).
    log_path: Option<PathBuf>,
    /// Append-only файл журнала (JSONL). Записи малы, флашим каждую —
    /// синхронный файл здесь осознанно: журнал должен переживать падение.
    log_file: Option<std::fs::File>,
    /// Детекторы циклов (doom-loop / doom-text / exploration spiral).
    detectors: crate::detectors::LoopDetectors,
    /// Редактор секретов: маскировка ключей/токенов в выводе инструментов
    /// до записи в историю и журнал (в журнал секрет не попадает никогда).
    redactor: crate::secrets::Redactor,
    /// Хуки жизненного цикла из конфига (`[hooks]`).
    hooks: crate::hooks::HookSet,
    /// Память сбоев инструментов: счётчики подписей ошибок, персистентные
    /// в `paths.state_dir/failure_memory.json` (правило «ошибся дважды →
    /// урок», режим — `[agent] failure_memory`).
    failure_memory: crate::failure_memory::FailureMemory,
    /// L3-саммаризация признана бесполезной (после неё всё равно выше
    /// порога): больше не вызывать — защита от сжигания API каждый ход.
    l3_futile: bool,
    /// Переключатель ризонинга (`/think on|off`): None — дефолт провайдера.
    thinking: Option<bool>,
    /// Токен отмены текущего хода (TUI: Esc/Alt+Enter — «прервать и
    /// вклиниться»). None вне TUI (CLI, субагенты, ralph) — ход неотменяем.
    cancel: Option<CancellationToken>,
    /// Реальная статистика токенов последнего стрим-ответа (`LlmEvent::Done`
    /// с `stream_options.include_usage`): забирается `log_usage` при записи
    /// assistant-сообщения в журнал. None — провайдер usage не отдал
    /// (нестриминговый `complete` или API без `usage`).
    last_usage: Option<Usage>,
    /// Тестовая замена политики [`crate::retry::NETWORK_PATIENCE`] —
    /// миллисекундные паузы вместо секундных, чтобы тесты не ждали.
    #[cfg(test)]
    net_patience_policy: Option<crate::retry::RetryPolicy>,
}

/// Префиксы заметок «сеть/DPI» живут в подмодуле [`network`]; реэкспорт
/// сохраняет пути `crate::agent::NET_*` (их распознаёт TUI, см. app.rs).
pub(crate) use network::{NET_BACK_NOTE_PREFIX, NET_WAIT_NOTE_PREFIX};

impl AgentSession {
    /// Новая сессия: создаёт `sessions_dir`, открывает журнал
    /// `session-<yyyymmdd-hhmmss>.jsonl` и пишет событие `system`.
    /// Файл журнала начинается с «#»-шапки — ASCII-логотип `ArchSpine`
    /// (banking edition) первой строкой лога (см. [`open_journal`]).
    /// Журнал недоступен — не фатально: сессия работает без записи
    /// (предупреждение в tracing).
    pub fn new(
        config: Arc<Config>,
        provider: Arc<dyn LlmProvider>,
        tools: ToolRegistry,
        tool_ctx: ToolContext,
        system_prompt: String,
    ) -> Self {
        let (log_path, log_file) = match open_journal(&config.paths.sessions_dir) {
            Ok((path, file)) => (Some(path), Some(file)),
            Err(e) => {
                tracing::warn!("журнал сессии недоступен: {e}");
                (None, None)
            }
        };
        let hooks = {
            // Хуки: конфиг + плагины (`include_hooks`); конфиг — первым.
            let mut specs = config.hooks.specs.clone();
            if config.plugins.include_hooks {
                specs.extend(crate::hooks::specs_from_plugin_dirs(&config.plugins.dirs));
            }
            crate::hooks::HookSet::from_specs(&specs)
        };
        let mut tool_ctx = tool_ctx;
        // C1 (0.3.12): замок библиотеки плагинов. Подменённый плагин уже не
        // загружен (`discover` отверг его), но событие обязано попасть в
        // ЖУРНАЛ СЕССИИ — канал аудита, а не только в tracing.
        let plugin_dirs = config.plugins.dirs.clone();
        // Активная модель в контексте инструментов: субагенты и дистилляция
        // наследуют её (и следят за /model через set_provider).
        tool_ctx.provider = Some(provider.clone());
        // Память сбоев грузится с диска до переноса config в структуру.
        let failure_memory = crate::failure_memory::FailureMemory::load(&config.paths.state_dir);
        // Блок уроков собирается из реестра на старте сессии (ADR-087):
        // область — рабочий каталог сессии, пути задачи уточнят его на ходу.
        let lessons_block = crate::lesson_block::block(&config, &tool_ctx.cwd, &[]);
        let mut session = Self {
            config,
            provider,
            tools,
            tool_ctx,
            system_prompt,
            lessons_block,
            lessons_paths: Vec::new(),
            lessons_query: None,
            history: Vec::new(),
            log_path,
            log_file,
            detectors: crate::detectors::LoopDetectors::new(),
            redactor: crate::secrets::Redactor::from_environment(),
            hooks,
            failure_memory,
            l3_futile: false,
            thinking: None,
            cancel: None,
            last_usage: None,
            #[cfg(test)]
            net_patience_policy: None,
        };
        let prompt = session.system_prompt.clone();
        session.log_event("system", serde_json::json!({ "content": prompt }));
        // C1: события целостности плагинов — в журнал сессии (findings канала
        // аудита), плагины уже отвергнуты загрузкой.
        for event in crate::plugin::discover_report(&plugin_dirs).tampered {
            session.log_event(
                "plugin_tampered",
                serde_json::json!({
                    "plugin": event.plugin,
                    "path": event.dir.display().to_string(),
                    "reason": event.reason,
                }),
            );
        }
        session.fire_hook(crate::hooks::HookEvent::SessionStart, None, "{}");
        session
    }

    /// Honest-refusal (ADR-083 §5): провайдер `kind = "cli"` не умеет
    /// tool-calls (контракт [`crate::llm::harness_cli`] — ответ всегда без
    /// вызовов), поэтому агентный цикл с инструментами на нём ломался бы на
    /// полпути. Вместо этого — честный отказ ДО первого запроса к модели.
    /// CLI-транспорт без инструментов (`tools` пуст) работает штатно, поэтому
    /// отказ возникает лишь когда инструменты действительно нужны.
    ///
    /// # Errors
    /// Провайдер сессии — `kind = "cli"`, а реестр инструментов непуст.
    fn ensure_tool_capable(&self) -> Result<()> {
        let is_cli = self
            .config
            .models
            .get(self.provider.name())
            .is_some_and(|mc| mc.kind.as_deref() == Some("cli"));
        if is_cli && !self.tools.specs().is_empty() {
            return Err(HarnessError::Agent(format!(
                "провайдер '{}' (kind=cli) не поддерживает tool-calls: агентный цикл с \
                 инструментами недоступен — выберите API-провайдера или запустите задачу \
                 без инструментов",
                self.provider.name()
            )));
        }
        Ok(())
    }

    /// Полный ход: пользовательский ввод → ответ ассистента (с инструментами).
    /// `events` — опциональный канал стриминга (None — тихий режим).
    /// При исчерпании лимита итераций ход не падает: добирается финальный
    /// ответ без инструментов (заметка в UI + событие `tool_turn_limit`
    /// в журнале).
    ///
    /// # Errors
    /// Ошибка модели (сеть, API, лимит контекста без прогресса).
    pub async fn send(
        &mut self,
        input: &str,
        events: Option<mpsc::Sender<AgentEvent>>,
    ) -> Result<String> {
        // Fail-closed до любого побочного эффекта: kind=cli + инструменты —
        // честный отказ (ADR-083 §5), а не поломка на середине цикла.
        self.ensure_tool_capable()?;
        // UserPromptSubmit: exit 2 хука отклоняет промпт целиком.
        let prompt_ctx = serde_json::json!({
            "prompt": input.chars().take(2000).collect::<String>(),
        })
        .to_string();
        let outcomes = self.fire_hook(crate::hooks::HookEvent::UserPromptSubmit, None, &prompt_ctx);
        if crate::hooks::any_blocked(&outcomes) {
            return Err(HarnessError::Agent(format!(
                "промпт отклонён хуком: {}",
                crate::hooks::block_reason(&outcomes)
            )));
        }
        // Блок уроков уточняется под ввод (пути задачи, BM25) — до запроса
        // к модели: «перед каждым ходом» (ADR-087).
        self.retarget_lessons_block(input);
        self.history.push(ChatMessage::user(input));
        self.log_event("user", serde_json::json!({ "content": input }));
        self.emit_context_usage(&events);

        let max_turns = self.config.agent.max_tool_turns;
        for _turn in 0..max_turns {
            // Отмена могла прийти между итерациями (пока шли tool-вызовы).
            if self.cancelled() {
                return self.finish_cancelled(&events).await;
            }
            self.compact_history(&events).await;
            // Компактификация могла существенно срезать историю — обновим UI.
            self.emit_context_usage(&events);
            let (request, repair) = self.build_request();
            self.log_tool_pair_repair(repair);
            // Не-стрим пути («мысли» не приходят дельтами) — эмитим
            // reasoning_content целиком после ответа (см. ниже).
            let mut streamed = false;
            let first = match (&events, self.config.agent.stream) {
                (Some(tx), true) => {
                    streamed = true;
                    race_cancel(self.cancel.clone(), self.stream_request(request, tx)).await
                }
                _ => race_cancel(self.cancel.clone(), self.provider.complete(request)).await,
            };
            // Отмена во время запроса к модели: история консистентна
            // (висячих tool-вызовов нет) — просто завершаем ход.
            let Some(first) = first else {
                return self.finish_cancelled(&events).await;
            };
            // Транспортные окна (DPI/VPN режут связь пачками по минутам —
            // см. NETWORK_PATIENCE): сетевую ошибку пережидаем в пределах
            // бюджета, а не роняем ход сразу.
            let first = match first {
                Err(e) => {
                    let Some(retried) = self.network_patience(e, &events, &mut streamed).await
                    else {
                        return self.finish_cancelled(&events).await;
                    };
                    retried
                }
                ok => ok,
            };
            // On-error compact & resubmit (Grok/Theseus): «контекст не влез»
            // (HTTP 413, context length) — принудительная L3-саммаризация
            // и ровно один повтор хода; дальше ошибка отдаётся наверх.
            let reply = match first {
                Ok(r) => r,
                Err(e) => {
                    let err_text = e.to_string();
                    let overflow = crate::retry::is_context_overflow(None, &err_text);
                    if overflow && !self.l3_futile {
                        self.emit_note(
                            &events,
                            "⚠ контекст не влезает — L3-компактификация и повтор хода".into(),
                        );
                        match self.l3_summarize().await {
                            Ok(_) => {
                                let (request, repair) = self.build_request();
                                self.log_tool_pair_repair(repair);
                                let retry = match (&events, self.config.agent.stream) {
                                    (Some(tx), true) => {
                                        streamed = true;
                                        race_cancel(
                                            self.cancel.clone(),
                                            self.stream_request(request, tx),
                                        )
                                        .await
                                    }
                                    _ => {
                                        race_cancel(
                                            self.cancel.clone(),
                                            self.provider.complete(request),
                                        )
                                        .await
                                    }
                                };
                                let Some(retried) = retry else {
                                    return self.finish_cancelled(&events).await;
                                };
                                retried?
                            }
                            Err(_) => return Err(e),
                        }
                    } else if crate::retry::is_tool_pair_contract_error(None, &err_text) {
                        // ADR-084: HTTP 400 «висячий tool_call_id» — санитайзер
                        // собирает запрос заново и делает ровно один повтор хода
                        // без L3-компактификации.
                        self.emit_note(
                            &events,
                            "⚠ нарушен контракт tool-пар — санитайзер и повтор хода".into(),
                        );
                        let (request, repair) = self.build_request();
                        self.log_tool_pair_repair(repair);
                        let retry = match (&events, self.config.agent.stream) {
                            (Some(tx), true) => {
                                streamed = true;
                                race_cancel(self.cancel.clone(), self.stream_request(request, tx))
                                    .await
                            }
                            _ => {
                                race_cancel(self.cancel.clone(), self.provider.complete(request))
                                    .await
                            }
                        };
                        let Some(retried) = retry else {
                            return self.finish_cancelled(&events).await;
                        };
                        retried?
                    } else {
                        return Err(e);
                    }
                }
            };

            if reply.tool_calls.is_empty() {
                // Обрыв финального ответа потолком max_tokens (thinking-модели
                // съедают бюджет размышлениями — Kimi K3 ризонит всегда):
                // дозапрашиваем продолжение и склеиваем, а не отдаём обрубок.
                let reply = self
                    .continue_on_length(reply, &events, &mut streamed)
                    .await?;
                if !streamed {
                    self.emit_reasoning_full(&events, &reply).await;
                }
                let (_, note) = self.detectors.note_reply_text(&reply.content);
                if let Some(note) = note {
                    self.emit_note(&events, note.text);
                }
                if reply.finish_reason.as_deref() == Some("length") {
                    self.emit_note(
                        &events,
                        "ответ усечён потолком max_tokens — текст неполный; \
                         скажите «продолжай» или поднимите max_tokens в конфиге модели"
                            .into(),
                    );
                }
                self.history.push(reply.clone());
                self.emit_context_usage(&events);
                // reasoning_content — в журнал (аудит цепочек рассуждений;
                // restore его игнорирует — обратная совместимость сохранена).
                self.log_event(
                    "assistant",
                    serde_json::json!({
                        "content": reply.content,
                        "reasoning_content": reply.reasoning_content,
                    }),
                );
                self.log_usage();
                if let Some(tx) = &events {
                    let _ = tx.send(AgentEvent::TurnDone).await;
                }
                return Ok(reply.content);
            }

            // Аргументы вызовов тоже проходят редакцию: модель могла
            // процитировать прочитанный секрет в параметрах инструмента.
            let logged_calls = serde_json::to_string(&reply.tool_calls)
                .map(|s| self.redactor.redact(&s))
                .ok()
                .and_then(|s| serde_json::from_str::<Value>(&s).ok())
                .unwrap_or(Value::Null);
            self.log_event(
                "assistant",
                serde_json::json!({
                    "content": reply.content,
                    "tool_calls": logged_calls,
                    "reasoning_content": reply.reasoning_content,
                }),
            );
            self.log_usage();
            // doom-text на промежуточном ответе: напоминание вклеивается
            // после результатов инструментов (контракт tool-пар не нарушается).
            let (reminder, note) = self.detectors.note_reply_text(&reply.content);
            if let Some(note) = note {
                self.emit_note(&events, note.text);
            }
            self.history.push(reply.clone());
            self.emit_context_usage(&events);
            if !streamed {
                self.emit_reasoning_full(&events, &reply).await;
            }

            for (call_idx, call) in reply.tool_calls.iter().enumerate() {
                if let Some(tx) = &events {
                    let _ = tx
                        .send(AgentEvent::ToolStart {
                            name: call.name.clone(),
                            args: call.arguments.clone(),
                        })
                        .await;
                }
                // Хук PreToolUse (exit 2) отменяет вызов — до детекторов:
                // заблокированный вызов не должен копить doom-окно.
                let hook_ctx =
                    serde_json::json!({ "tool": call.name, "args": call.arguments }).to_string();
                let pre = self.fire_hook(
                    crate::hooks::HookEvent::PreToolUse,
                    Some(&call.name),
                    &hook_ctx,
                );
                // Битые аргументы (усечённый ответ модели) отклоняем раньше
                // хуков и детекторов: исполнять обрезок нельзя.
                let hook_verdict = broken_arguments_verdict(&reply, call).or_else(|| {
                    if crate::hooks::any_blocked(&pre) {
                        Some(format!(
                            "BLOCKED by hook: {}",
                            crate::hooks::block_reason(&pre)
                        ))
                    } else {
                        None
                    }
                });
                // Детекторы циклов: вердикт подменяет исполнение — контракт
                // tool-сообщений сохраняется (каждый id получает ровно один ответ).
                let (verdict, note) = match hook_verdict {
                    Some(v) => (Some(v), None),
                    None => self.detectors.check_call(&call.name, &call.arguments),
                };
                if let Some(note) = note {
                    self.emit_note(&events, note.text);
                }
                let out = if let Some(text) = verdict {
                    ToolOutput::err(text)
                } else {
                    // Интерактивный выбор (propose_options) ждёт человека —
                    // ему расширенный таймаут; остальным — per-tool таймаут
                    // из реестра (harness_run — до 7200 с + запас; раньше
                    // все сидели на жёстких 300 с и длинные прогоны
                    // кодовых харнессов обрывались агентным циклом).
                    let wait = if call.name == crate::tools::ask::PROPOSE_OPTIONS {
                        ASK_TIMEOUT_SECS
                    } else {
                        self.tools.timeout_secs(&call.name)
                    };
                    let dispatched = tokio::time::timeout(
                        Duration::from_secs(wait),
                        self.tools
                            .dispatch(&call.name, call.arguments.clone(), &self.tool_ctx),
                    );
                    match race_cancel(self.cancel.clone(), dispatched).await {
                        Some(Ok(out)) => out,
                        Some(Err(_)) => {
                            ToolOutput::err(format!("{}: таймаут {}с", call.name, wait))
                        }
                        // Отмена посреди пачки вызовов: текущему и всем
                        // незапущенным даём результат «прервано» — иначе
                        // контракт tool-пар сломается и следующий ход
                        // упадёт с HTTP 400 от API.
                        None => {
                            self.cancel_pending_tools(&reply, call_idx, &events).await;
                            return self.finish_cancelled(&events).await;
                        }
                    }
                };
                // Редакция секретов до событий/истории/журнала: вывод
                // инструмента мог содержать ключи из .env, конфигов, PEM.
                let mut redacted = self.redactor.redact(&out.content);
                if redacted != out.content {
                    self.log_event(
                        "event",
                        serde_json::json!({ "event": "secrets_redacted", "tool": call.name }),
                    );
                }
                // Warn-детектор prompt-инъекций (T2, ADR-038): только вывод
                // инструментов чтения (read_file/grep/glob), НЕ bash — там
                // shell-код с кавычками даёт постоянные ложные срабатывания.
                // Маркер дописывается в вывод (видит модель) + событие в
                // журнал (аудит); вывод НЕ блокируется (warn-first).
                if crate::injection::is_scanned_tool(&call.name) {
                    let hits = crate::injection::scan(&redacted);
                    if !hits.is_empty() {
                        for hit in &hits {
                            let _ = write!(redacted, "\n{}", crate::injection::marker(hit));
                        }
                        self.log_event(
                            "event",
                            serde_json::json!({
                                "event": "prompt_injection",
                                "tool": call.name,
                                "matches": hits
                                    .iter()
                                    .map(|h| serde_json::json!({
                                        "line": h.line,
                                        "pattern": h.pattern,
                                    }))
                                    .collect::<Vec<_>>(),
                            }),
                        );
                    }
                }
                // Память сбоев («ошибся дважды → урок»): подпись ошибки
                // считается по редакционному выводу (без секретов); второй
                // случай подписи порождает урок по режиму конфига.
                if out.is_error {
                    self.note_tool_failure(&call.name, &redacted, &events);
                }
                // Аудит периметра файловых инструментов (ADR-083 п.1): отказ
                // «вне рабочей зоны» пишется отдельным событием журнала сессии
                // — видно, что модель пыталась выйти за cwd, каким инструментом
                // и куда (без содержимого аргументов; секреты уже вычищены).
                if crate::tool::is_fs_zone_denial(&out) {
                    self.log_event(
                        "event",
                        serde_json::json!({
                            "event": "fs_zone_denied",
                            "tool": call.name,
                            "reason": redacted.chars().take(300).collect::<String>(),
                        }),
                    );
                }
                // Аудит интерактивных выборов: метрика «approval theater»
                // (обзоры _24_августа: >90–95% авто-согласий = театр) в arch-be metrics.
                if call.name == crate::tools::ask::PROPOSE_OPTIONS {
                    if let Some((choice, declined)) =
                        crate::tools::ask::classify_answer(&out.content)
                    {
                        let recommended = call
                            .arguments
                            .get("recommended")
                            .and_then(Value::as_str)
                            .unwrap_or("");
                        self.log_event(
                            "event",
                            serde_json::json!({
                                "event": "ask",
                                "question": call
                                    .arguments
                                    .get("question")
                                    .and_then(Value::as_str)
                                    .unwrap_or("")
                                    .chars()
                                    .take(200)
                                    .collect::<String>(),
                                "recommended": recommended,
                                "choice": choice,
                                "declined": declined,
                                "chose_recommended": !declined
                                    && !recommended.is_empty()
                                    && choice == recommended,
                            }),
                        );
                    }
                }
                // PostToolUse-хуки: stdout дописывается к результату (маркер).
                let post_ctx = serde_json::json!({
                    "tool": call.name,
                    "is_error": out.is_error,
                    "output_head": redacted.chars().take(500).collect::<String>(),
                })
                .to_string();
                let post = self.fire_hook(
                    crate::hooks::HookEvent::PostToolUse,
                    Some(&call.name),
                    &post_ctx,
                );
                let hook_extra: String = post
                    .iter()
                    .map(|o| o.stdout.trim())
                    .filter(|s| !s.is_empty())
                    .collect::<Vec<_>>()
                    .join("\n");
                if !hook_extra.is_empty() {
                    let _ = write!(
                        redacted,
                        "\n[hook] {}",
                        hook_extra.chars().take(1000).collect::<String>()
                    );
                }
                let content = truncate_chars(&redacted, TOOL_RESULT_MAX_CHARS);
                if let Some(tx) = &events {
                    let _ = tx
                        .send(AgentEvent::ToolEnd {
                            name: call.name.clone(),
                            is_error: out.is_error,
                            summary: summarize(&redacted),
                            // UI получает ПОЛНЫЙ вывод (mermaid-арт на вкладке
                            // не должен обрываться); усечённая версия — только
                            // в историю и журнал (бюджет контекста модели).
                            content: redacted.clone(),
                        })
                        .await;
                }
                self.log_event(
                    "tool",
                    serde_json::json!({
                        "name": call.name,
                        "tool_call_id": call.id,
                        "is_error": out.is_error,
                        "content": content,
                    }),
                );
                self.history
                    .push(ChatMessage::tool_result(call.id.clone(), content));
                // Мультимодальность: `DeepSeek` принимает изображения только в
                // user-сообщениях — если инструмент вернул изображение
                // (скриншот/чтение), добавляем синтетическое user-сообщение,
                // чтобы модель его увидела и проанализировала.
                if !out.images.is_empty() {
                    let mut img_msg = ChatMessage::user(format!(
                        "Инструмент «{}» вернул {} изображени{} — проанализируй:",
                        call.name,
                        out.images.len(),
                        if out.images.len() == 1 { "е" } else { "я" }
                    ));
                    img_msg.images.clone_from(&out.images);
                    self.history.push(img_msg);
                }
                self.emit_context_usage(&events);
            }
            if let Some(reminder) = reminder {
                self.history.push(ChatMessage::user(reminder));
            }
        }
        // Бюджет итераций исчерпан (длинная легитимная работа или петля):
        // ход НЕ убиваем — добираем финальный ответ БЕЗ инструментов,
        // чтобы пользователь получил результат, а не мёртвую ошибку.
        self.emit_note(
            &events,
            format!("лимит итераций инструментов ({max_turns}) — собираю финальный ответ без инструментов"),
        );
        self.log_event(
            "event",
            serde_json::json!({ "event": "tool_turn_limit", "limit": max_turns }),
        );
        let (mut request, repair) = self.build_request();
        self.log_tool_pair_repair(repair);
        request.tools.clear();
        // Служебная реплика — только в запрос, в историю не пишется.
        request.messages.push(ChatMessage::user(
            "[харнесс] Лимит итераций инструментов исчерпан. Дай финальный ответ по уже \
             собранным данным, без вызовов инструментов: что сделано, что осталось, \
             следующий шаг для пользователя.",
        ));
        let mut final_streamed = false;
        let reply = match (&events, self.config.agent.stream) {
            (Some(tx), true) => {
                final_streamed = true;
                self.stream_request(request, tx).await
            }
            _ => self.provider.complete(request).await,
        }?;
        if !final_streamed {
            self.emit_reasoning_full(&events, &reply).await;
        }
        let content = reply.content.clone();
        self.log_event(
            "assistant",
            serde_json::json!({
                "content": reply.content,
                "reasoning_content": reply.reasoning_content,
            }),
        );
        self.log_usage();
        self.history.push(reply);
        self.emit_context_usage(&events);
        if let Some(tx) = &events {
            let _ = tx.send(AgentEvent::TurnDone).await;
        }
        Ok(content)
    }

    /// Текущая история сообщений.
    #[must_use]
    pub fn messages(&self) -> &[ChatMessage] {
        &self.history
    }

    /// Устанавливает/снимает токен отмены текущего хода (см. поле `cancel`).
    /// TUI ставит свежий токен на каждый ход; CLI/субагенты не ставят ничего.
    pub fn set_cancel_token(&mut self, token: Option<CancellationToken>) {
        self.cancel = token;
    }

    /// Установлен ли флаг отмены текущего хода.
    fn cancelled(&self) -> bool {
        self.cancel
            .as_ref()
            .is_some_and(CancellationToken::is_cancelled)
    }

    /// Завершение хода по отмене пользователя: заметка в UI, событие
    /// `turn_cancelled` в журнал, `TurnDone`. Текст пуст — TUI не рисует
    /// пустой assistant-блок, заметка уже показана событием.
    async fn finish_cancelled(
        &mut self,
        events: &Option<mpsc::Sender<AgentEvent>>,
    ) -> Result<String> {
        self.emit_note(
            events,
            "⛔ ход прерван пользователем; история сессии цела — продолжайте диалог".into(),
        );
        self.log_event("event", serde_json::json!({ "event": "turn_cancelled" }));
        self.emit_context_usage(events);
        if let Some(tx) = events {
            let _ = tx.send(AgentEvent::TurnDone).await;
        }
        Ok(String::new())
    }

    /// Отмена посреди пачки tool-вызовов: вызову `from` (его `ToolStart` уже
    /// показан в UI — закрываем `ToolEnd`) и всем незапущенным рассылаем
    /// результат «прервано», чтобы каждый `call.id` получил ровно один
    /// `tool_result` (контракт tool-пар API).
    async fn cancel_pending_tools(
        &mut self,
        reply: &ChatMessage,
        from: usize,
        events: &Option<mpsc::Sender<AgentEvent>>,
    ) {
        /// Текст результата отменённого вызова (в историю, журнал и UI).
        const CANCELLED: &str = "⛔ вызов прерван пользователем (Esc/Alt+Enter)";
        for (idx, call) in reply.tool_calls.iter().enumerate().skip(from) {
            if idx == from {
                if let Some(tx) = events {
                    let _ = tx
                        .send(AgentEvent::ToolEnd {
                            name: call.name.clone(),
                            is_error: true,
                            summary: CANCELLED.into(),
                            content: CANCELLED.into(),
                        })
                        .await;
                }
            }
            self.log_event(
                "tool",
                serde_json::json!({
                    "name": call.name,
                    "tool_call_id": call.id,
                    "is_error": true,
                    "cancelled": true,
                    "content": CANCELLED,
                }),
            );
            self.history.push(ChatMessage::tool_result(
                call.id.clone(),
                CANCELLED.to_string(),
            ));
        }
    }

    /// Сменить модель на лету (и в контексте инструментов — для субагентов).
    pub fn set_provider(&mut self, provider: Arc<dyn LlmProvider>) {
        self.tool_ctx.provider = Some(provider.clone());
        self.provider = provider;
    }

    /// Активный провайдер (для слэш-команд вроде `/distill`).
    #[must_use]
    pub fn provider(&self) -> Arc<dyn LlmProvider> {
        self.provider.clone()
    }

    /// Устанавливает переключатель ризонинга (`/think on|off`);
    /// `None` — вернуться к дефолту провайдера.
    pub fn set_thinking(&mut self, thinking: Option<bool>) {
        self.thinking = thinking;
    }

    /// Текущее состояние переключателя ризонинга.
    #[must_use]
    pub fn thinking(&self) -> Option<bool> {
        self.thinking
    }

    /// Имя активной модели.
    #[must_use]
    pub fn model_name(&self) -> String {
        self.provider.model().to_string()
    }

    /// Имена зарегистрированных инструментов (для `/tools`).
    #[must_use]
    pub fn tool_names(&self) -> Vec<String> {
        self.tools.names()
    }

    /// Спецификации инструментов с описаниями (для `/tools`).
    #[must_use]
    pub fn tool_specs(&self) -> Vec<crate::llm::ToolSpec> {
        self.tools.specs()
    }

    /// Путь к журналу сессии (None — журнал не открыт).
    #[must_use]
    pub fn log_path(&self) -> Option<&Path> {
        self.log_path.as_deref()
    }

    /// Очистить историю (с записью события в журнал) и сбросить детекторы.
    pub fn clear(&mut self) {
        self.history.clear();
        self.detectors.reset();
        self.l3_futile = false;
        self.log_event("event", serde_json::json!({ "event": "clear" }));
    }

    /// Начать новую сессию на месте текущей (`/new`): история, детекторы
    /// и L3-флаг сброшены, журнал — НОВЫЙ файл (старый остаётся на диске
    /// для `/sessions` и `/resume`). В старый журнал пишется `session_end`,
    /// в новый — системный промпт и хук `SessionStart`. Модель и
    /// переключатель ризонинга — пользовательские настройки, сохраняются.
    pub fn reset(&mut self) {
        self.log_event(
            "event",
            serde_json::json!({ "event": "session_end", "reason": "new" }),
        );
        match open_journal(&self.config.paths.sessions_dir) {
            Ok((path, file)) => {
                self.log_path = Some(path);
                self.log_file = Some(file);
            }
            Err(e) => {
                tracing::warn!("журнал новой сессии недоступен: {e}");
                self.log_path = None;
                self.log_file = None;
            }
        }
        self.history.clear();
        self.detectors.reset();
        self.l3_futile = false;
        // Новая сессия — блок уроков пересобирается из реестра, отбор
        // возвращается к области репозитория (ADR-087).
        self.lessons_paths.clear();
        self.lessons_query = None;
        self.refresh_lessons_block();
        let prompt = self.system_prompt.clone();
        self.log_event("system", serde_json::json!({ "content": prompt }));
        self.fire_hook(crate::hooks::HookEvent::SessionStart, None, "{}");
    }

    /// Эмит служебной заметки: событие в канал (если есть) + запись в журнал.
    fn emit_note(&mut self, events: &Option<mpsc::Sender<AgentEvent>>, text: String) {
        self.log_event(
            "event",
            serde_json::json!({ "event": "note", "text": text }),
        );
        if let Some(tx) = events {
            let _ = tx.try_send(AgentEvent::Note(text));
        }
    }

    /// Учёт сбоя инструмента в памяти failure-memory: при втором случае той
    /// же подписи (ровно один раз на подпись) формируется урок — заметка
    /// в транскрипт и предложение в `state/failure_lessons.md` (режим
    /// `propose`); в режиме `write` урок дополнительно дописывается в
    /// AGENTS.md рабочего каталога (строго append, вне зоны ARCH:GENERATED).
    /// Сбои записи — warn в tracing/заметка, ход агента не прерывают.
    fn note_tool_failure(
        &mut self,
        tool: &str,
        output: &str,
        events: &Option<mpsc::Sender<AgentEvent>>,
    ) {
        use crate::config::FailureMemoryMode as Mode;
        let mode = self.config.agent.failure_memory;
        if mode == Mode::Off {
            return;
        }
        let Some(lesson) = self.failure_memory.note_failure(tool, output) else {
            return;
        };
        self.log_event(
            "event",
            serde_json::json!({
                "event": "failure_lesson",
                "signature": lesson.signature,
                "tool": lesson.tool,
                "count": lesson.count,
            }),
        );
        let lessons_path = self.failure_memory.lessons_path().to_path_buf();
        let mut where_to =
            match crate::failure_memory::append_lesson(&lessons_path, &lesson.to_markdown()) {
                Ok(()) => format!(
                    "урок предложен: {} (просмотр — /lessons)",
                    lessons_path.display()
                ),
                Err(e) => {
                    tracing::warn!("failure_lessons.md: {e}");
                    format!(
                        "⚠ не удалось записать урок в {}: {e}",
                        lessons_path.display()
                    )
                }
            };
        if mode == Mode::Write {
            match crate::failure_memory::append_to_agents_md(&self.tool_ctx.cwd, &lesson) {
                Ok(path) => {
                    let _ = write!(where_to, "; дописан в {}", path.display());
                }
                Err(e) => {
                    tracing::warn!("AGENTS.md (failure-memory): {e}");
                    let _ = write!(where_to, "; ⚠ AGENTS.md: {e}");
                }
            }
        }
        self.emit_note(
            events,
            format!(
                "🔁 сбой «{}» у инструмента `{tool}` повторился ({} случая) — {where_to}",
                lesson.class, lesson.count
            ),
        );
    }

    /// Эмит текущей оценки токенов истории (индикатор контекста в UI).
    /// `try_send`: телеметрия необязательна — ход агента не блокируется,
    /// если канал занят дельтами; следующая эмиссия обновит значение.
    fn emit_context_usage(&self, events: &Option<mpsc::Sender<AgentEvent>>) {
        if let Some(tx) = events {
            let used: usize = self.history.iter().map(ChatMessage::rough_tokens).sum();
            let _ = tx.try_send(AgentEvent::ContextUsage(used));
        }
    }

    /// Эмитит `reasoning_content` ответа целиком (не-стрим путь: в стриме
    /// «мысли» уже пришли дельтами). Пустой/отсутствующий ризонинг молчит.
    async fn emit_reasoning_full(
        &self,
        events: &Option<mpsc::Sender<AgentEvent>>,
        reply: &ChatMessage,
    ) {
        if let (Some(tx), Some(reasoning)) = (events, reply.reasoning_content.as_deref()) {
            let reasoning = reasoning.trim();
            if !reasoning.is_empty() {
                let _ = tx
                    .send(AgentEvent::ReasoningDelta(reasoning.to_string()))
                    .await;
            }
        }
    }

    /// Авто-продолжение финального ответа, оборванного потолком `max_tokens`
    /// (`finish_reason=length` с непустым текстом): до [`MAX_LENGTH_CONTINUATIONS`]
    /// дозапросов «продолжи с места обрыва» с удвоенным бюджетом (эскалация —
    /// [`crate::llm::escalated_max_tokens`]). Дельты продолжения стримятся в тот
    /// же ответ; в историю попадает одно склеенное сообщение — служебная пара
    /// «обрубок + инструкция» историю не загрязняет (запрос собирается
    /// локально). Отмена/ошибка продолжения — мягкие: возвращается накопленная
    /// часть, ход не рвётся. Пустой видимый ответ сюда не доходит — его ещё
    /// в провайдере забирает автоэскалация (повтор без дублей в чате).
    async fn continue_on_length(
        &mut self,
        mut reply: ChatMessage,
        events: &Option<mpsc::Sender<AgentEvent>>,
        streamed: &mut bool,
    ) -> Result<ChatMessage> {
        let mut continuations = 0usize;
        while reply.finish_reason.as_deref() == Some("length")
            && !reply.content.trim().is_empty()
            && continuations < MAX_LENGTH_CONTINUATIONS
        {
            continuations += 1;
            let budget = self
                .config
                .models
                .get(self.provider.name())
                .and_then(|m| m.max_tokens);
            let next = crate::llm::escalated_max_tokens(budget);
            self.emit_note(
                events,
                format!(
                    "ответ оборвался на лимите max_tokens — дозапрашиваю продолжение \
                     ({continuations}/{MAX_LENGTH_CONTINUATIONS}{})",
                    next.map_or_else(String::new, |n| format!(", max_tokens ↑ {n}"))
                ),
            );
            self.log_event(
                "event",
                serde_json::json!({
                    "event": "length_continuation",
                    "attempt": continuations,
                    "max_tokens": next,
                }),
            );
            // Запрос-продолжение: история + обрубок как assistant + инструкция.
            // Ремонт tool-пар при повторной сборке журналируем как везде (ADR-084).
            let (mut cont_req, repair) = self.build_request();
            self.log_tool_pair_repair(repair);
            if let Some(next) = next {
                cont_req.max_tokens = Some(next);
            }
            cont_req
                .messages
                .push(ChatMessage::assistant(reply.content.clone(), Vec::new()));
            cont_req.messages.push(ChatMessage::user(
                "[харнесс] Предыдущий ответ оборвался по лимиту max_tokens. \
                 Продолжи строго с места обрыва: не повторяй уже написанное, \
                 не начинай заново, не комментируй обрыв.",
            ));
            let cont = match (events, self.config.agent.stream) {
                (Some(tx), true) => {
                    *streamed = true;
                    race_cancel(self.cancel.clone(), self.stream_request(cont_req, tx)).await
                }
                _ => race_cancel(self.cancel.clone(), self.provider.complete(cont_req)).await,
            };
            let part = match cont {
                Some(Ok(part)) => part,
                Some(Err(e)) => {
                    self.emit_note(
                        events,
                        format!("дозапрос продолжения не удался ({e}) — ответ частичный"),
                    );
                    break;
                }
                None => {
                    self.emit_note(
                        events,
                        "продолжение прервано пользователем — ответ частичный".into(),
                    );
                    break;
                }
            };
            // Продолжение с tool_calls — не склеиваем (обрыв текста лечим,
            // а не смену режима): оставляем накопленный текст как есть.
            if !part.tool_calls.is_empty() {
                break;
            }
            reply.content.push_str(&part.content);
            reply.finish_reason = part.finish_reason;
            if part.reasoning_content.is_some() {
                reply.reasoning_content = part.reasoning_content;
            }
        }
        Ok(reply)
    }

    /// Исполнить хуки события и записать итоги в журнал. Блокирующие
    /// события (exit 2) разбирает вызывающая сторона по возвращённым итогам.
    fn fire_hook(
        &mut self,
        event: crate::hooks::HookEvent,
        tool: Option<&str>,
        context: &str,
    ) -> Vec<crate::hooks::HookOutcome> {
        if self.hooks.is_empty() {
            return Vec::new();
        }
        let outcomes = self.hooks.fire(event, tool, context);
        if !outcomes.is_empty() {
            self.log_event(
                "event",
                serde_json::json!({
                    "event": "hook",
                    "hook_event": event.as_str(),
                    "tool": tool,
                    "outcomes": outcomes.iter().map(|o| serde_json::json!({
                        "command": o.command,
                        "code": o.code,
                        "note": o.note,
                    })).collect::<Vec<_>>(),
                }),
            );
        }
        outcomes
    }

    /// Включить внешний текст в контекст как user-сообщение (без обращения
    /// к модели) — используется `/load`.
    pub fn inject_context(&mut self, label: &str, content: &str) {
        let text = format!("Включи в контекст ({label}):\n\n{content}");
        self.history.push(ChatMessage::user(&text));
        self.log_event(
            "user",
            serde_json::json!({ "content": text, "injected": label }),
        );
    }

    /// Восстанавливает историю диалога из журнала прошлой сессии (JSONL).
    ///
    /// Переносятся сообщения user/assistant (тексты); вызовы инструментов
    /// прошлой сессии в историю не попадают (orphan `tool_calls` недопустимы
    /// для API), но остаются в файле журнала. «#»-строки шапки (логотип)
    /// пропускаются. Возвращает число восстановленных сообщений.
    ///
    /// # Errors
    /// Файл не читается или не является JSONL-журналом сессии.
    pub fn restore_from_log(&mut self, path: &Path) -> Result<usize> {
        let text = std::fs::read_to_string(path).map_err(|e| HarnessError::io(path, e))?;
        let mut restored = 0usize;
        for (n, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
                return Err(HarnessError::Agent(format!(
                    "{}: строка {} — не JSONL журнала сессии",
                    path.display(),
                    n + 1
                )));
            };
            let kind = v.get("kind").and_then(|k| k.as_str()).unwrap_or("");
            let content = v
                .get("content")
                .and_then(|c| c.as_str())
                .unwrap_or_default()
                .to_string();
            match kind {
                "user" if !content.is_empty() => {
                    self.history.push(ChatMessage::user(content));
                    restored += 1;
                }
                "assistant" if !content.is_empty() => {
                    // tool_calls прошлой сессии намеренно не переносим.
                    self.history
                        .push(ChatMessage::assistant(content, Vec::new()));
                    restored += 1;
                }
                _ => {}
            }
        }
        self.log_event(
            "event",
            serde_json::json!({ "event": "resume", "from": path.display().to_string(), "restored": restored }),
        );
        Ok(restored)
    }

    /// Собирает запрос к модели: системный промпт + история, инструменты,
    /// `max_tokens/temperature` из `ModelConfig` активного провайдера.
    ///
    /// На границе сборки прогоняется [`sanitize_tool_pairs`] (ADR-084):
    /// провайдеры падают HTTP 400, если у ассистентского `tool_calls` остался
    /// `id` без tool-ответа. Санитайзер чинит КОПИЮ истории запроса (журнал и
    /// история сессии не трогаются, AD-5); возвращённый [`ToolPairRepair`]
    /// вызывающая сторона фиксирует в журнале событием `tool_pairs_repaired`.
    ///
    /// [`sanitize_tool_pairs`]: crate::llm::sanitize_tool_pairs
    fn build_request(&self) -> (ChatRequest, ToolPairRepair) {
        let mut messages = Vec::with_capacity(self.history.len() + 1);
        // Закреплённый блок уроков — в системном промпте каждого запроса
        // (ADR-087): история компактифицируется, блок — нет.
        messages.push(ChatMessage::system(self.effective_system_prompt()));
        messages.extend(self.history.iter().cloned());
        let repair = crate::llm::sanitize_tool_pairs(&mut messages);
        if repair.repaired() {
            tracing::warn!(
                synthesized = repair.synthesized,
                dropped_orphans = repair.dropped_orphans,
                "контракт tool-пар нарушен при сборке запроса — история отремонтирована"
            );
        }
        let mc = self.config.models.get(self.provider.name());
        let request = ChatRequest {
            messages,
            tools: self.tools.specs(),
            temperature: mc.and_then(|m| m.temperature),
            max_tokens: mc.and_then(|m| m.max_tokens),
            thinking: self.thinking,
        };
        (request, repair)
    }

    /// Фиксирует ремонт контракта tool-пар в журнале сессии (ADR-084).
    ///
    /// Ненулевой [`ToolPairRepair`] — сигнал, что история запроса расходилась
    /// с контрактом на границе сборки; без записи в журнал класс деградации
    /// остался бы невидимым (AD-5: журнал — источник аудита).
    fn log_tool_pair_repair(&mut self, repair: ToolPairRepair) {
        if repair.repaired() {
            self.log_event(
                "tool_pairs_repaired",
                serde_json::json!({
                    "synthesized": repair.synthesized,
                    "dropped_orphans": repair.dropped_orphans,
                }),
            );
        }
    }

    /// Стриминговый запрос: дельты провайдера пересылаются как
    /// [`AgentEvent::Delta`]. Пересыльщик дожидается конца потока, чтобы
    /// ни одна дельта не потерялась до возврата ответа. Попутно забирает
    /// из `LlmEvent::Done` реальную статистику токенов (`include_usage`) —
    /// она пишется в журнал записью `usage` при логировании ответа.
    async fn stream_request(
        &mut self,
        request: ChatRequest,
        events: &mpsc::Sender<AgentEvent>,
    ) -> Result<ChatMessage> {
        self.last_usage = None;
        let (ltx, mut lrx) = mpsc::channel::<LlmEvent>(64);
        let ftx = events.clone();
        // Ячейка для usage из финального чанка: пересыльщик живёт в отдельной
        // задаче, поэтому статистика возвращается через разделяемый Option.
        let usage_cell = std::sync::Arc::new(std::sync::Mutex::new(None::<Usage>));
        let usage_sink = std::sync::Arc::clone(&usage_cell);
        let forward = tokio::spawn(async move {
            while let Some(ev) = lrx.recv().await {
                let mapped = match ev {
                    LlmEvent::Delta(text) => AgentEvent::Delta(text),
                    LlmEvent::ReasoningDelta(text) => AgentEvent::ReasoningDelta(text),
                    LlmEvent::Note(text) => AgentEvent::Note(text),
                    LlmEvent::Done(usage) => {
                        // Нулевые счётчики — это «API usage не отдал» (дефолтная
                        // обёртка трейта шлёт Done(Usage::default())): такую
                        // запись в журнал не несём, иначе сессия выглядела бы
                        // как «с usage» при нулевых токенах.
                        if usage.total() > 0 {
                            // Ошибка отравления мьютекса невозможна: код под
                            // замком не паникует; при отравлении usage теряем.
                            if let Ok(mut guard) = usage_sink.lock() {
                                *guard = Some(usage);
                            }
                        }
                        continue;
                    }
                };
                if ftx.send(mapped).await.is_err() {
                    break;
                }
            }
        });
        let result = self.provider.stream(request, ltx).await;
        let _ = forward.await;
        if result.is_ok() {
            // Отравление мьютекса невозможно (см. выше): при Err просто
            // остаётся None — записи usage в журнале не будет.
            if let Ok(guard) = usage_cell.lock() {
                self.last_usage = *guard;
            }
        }
        result
    }

    /// Запись `usage` в журнал по итогам ответа LLM (реальные токены из
    /// `stream_options.include_usage`). Вызывается после каждого логирования
    /// assistant-сообщения; провайдер usage не отдал (None) — записи нет.
    fn log_usage(&mut self) {
        let Some(usage) = self.last_usage.take() else {
            return;
        };
        self.log_event(
            "usage",
            serde_json::json!({
                "model": self.provider.model(),
                "prompt_tokens": usage.prompt_tokens,
                "completion_tokens": usage.completion_tokens,
            }),
        );
    }

    /// Блок уроков, закреплённый за сессией (ADR-087, L3): пусто при
    /// `[lessons] enabled = false` или пустом реестре.
    #[must_use]
    pub fn lessons_block(&self) -> &str {
        &self.lessons_block
    }

    /// Системный промпт запроса: база плюс закреплённый блок уроков.
    ///
    /// Блок живёт **вне** истории: компактификация (L1-маскирование,
    /// прунинг, L3-саммари) физически не может его «пересказать» — она режет
    /// историю, а блок приходит из реестра при каждой сборке запроса.
    #[must_use]
    pub fn effective_system_prompt(&self) -> String {
        crate::lesson_block::augment_system_prompt(&self.system_prompt, &self.lessons_block)
    }

    /// Пересобирает блок уроков из реестра (ADR-087).
    ///
    /// Вызывается после каждой компактификации и на каждом пользовательском
    /// вводе: урок, выведенный из реестра (`retired`, рост `misses`), исчезает
    /// из блока, а новый — появляется, без перезапуска сессии. Ошибки чтения
    /// реестра гасит сам сборщик (fail-soft), ход агента они не рвут.
    fn refresh_lessons_block(&mut self) {
        let block = crate::lesson_block::build_report(
            &self.config,
            &self.tool_ctx.cwd,
            &self.lessons_paths,
            self.lessons_query.as_deref(),
        )
        .block;
        if block != self.lessons_block {
            self.lessons_block = block;
        }
    }

    /// Уточняет отбор блока по тексту пользовательского ввода: пути — для
    /// scope-отбора, текст — для BM25 (та же логика, что у хука
    /// `user-prompt-submit`, но внутри harness-сессии).
    fn retarget_lessons_block(&mut self, input: &str) {
        if !self.config.lessons.enabled {
            return;
        }
        self.lessons_paths = crate::lesson_block::paths_from_text(input);
        self.lessons_query = Some(input.chars().take(LESSONS_QUERY_MAX_CHARS).collect());
        self.refresh_lessons_block();
    }

    /// Эффективный бюджет контекста: min(`agent.context_budget_tokens`,
    /// окно модели из `ModelConfig.context_limit`). Пороги компактификации
    /// (70%/95%) таким образом привязаны к реальному пределу API активного
    /// провайдера, а не только к конфигурационному бюджету. Публичен для
    /// индикатора заполнения контекста в статус-баре TUI.
    #[must_use]
    pub fn effective_context_budget(&self) -> usize {
        let configured = self.config.agent.context_budget_tokens.max(1);
        self.config
            .models
            .get(self.provider.name())
            .and_then(|m| m.context_limit)
            .filter(|lim| *lim > 0)
            .map_or(configured, |lim| configured.min(lim))
    }

    /// L1: маскирование старых tool-результатов (усечение до
    /// [`COMPACT_TOOL_CHARS`] с пометкой, кроме [`COMPACT_KEEP_TOOL`]
    /// последних). Возвращает число усечённых сообщений.
    fn mask_old_tool_results(&mut self) -> usize {
        let tool_idx: Vec<usize> = self
            .history
            .iter()
            .enumerate()
            .filter(|(_, m)| m.role == Role::Tool)
            .map(|(i, _)| i)
            .collect();
        let cut = tool_idx.len().saturating_sub(COMPACT_KEEP_TOOL);
        let mut truncated = 0usize;
        for &i in &tool_idx[..cut] {
            let msg = &mut self.history[i];
            if msg.content.chars().count() > COMPACT_TOOL_CHARS {
                let mut short: String = msg.content.chars().take(COMPACT_TOOL_CHARS).collect();
                short.push_str("\n[контекст усечён]");
                msg.content = short;
                truncated += 1;
            }
        }
        truncated
    }

    /// Принудительная компактификация по слэш-команде `/compact`:
    /// L1-маскирование + L3-саммари ВНЕ порогов (и даже при `l3_futile` —
    /// явная воля пользователя). Возвращает (токенов до, после, свёрнуто
    /// сообщений, усечено tool-результатов).
    ///
    /// # Errors
    /// Ошибка модели-саммаризатора.
    pub async fn compact_now(&mut self) -> Result<(usize, usize, usize, usize)> {
        let before: usize = self.history.iter().map(ChatMessage::rough_tokens).sum();
        self.fire_hook(crate::hooks::HookEvent::PreCompact, None, "{}");
        let truncated = self.mask_old_tool_results();
        let folded = self.l3_summarize().await?;
        // Блок уроков пересобирается из реестра ПОСЛЕ обеих ступеней
        // компактификации (ADR-087): история могла содержать урок свободным
        // текстом — он мог быть замаскирован (L1) или пересказан саммари (L3),
        // поэтому авторитетный источник блока — реестр, а не история (B5).
        self.refresh_lessons_block();
        let after: usize = self.history.iter().map(ChatMessage::rough_tokens).sum();
        self.log_event(
            "event",
            serde_json::json!({
                "event": "compact_manual",
                "before": before,
                "after": after,
                "folded": folded,
                "truncated_tools": truncated,
            }),
        );
        if truncated > 0 || folded > 0 {
            self.fire_hook(crate::hooks::HookEvent::PostCompact, None, "{}");
        }
        Ok((before, after, folded, truncated))
    }

    /// Компактификация истории (трёхуровневая, по Theseus/Grok/Codex):
    /// - L1 (≥ `compact_l1_pct`% бюджета): маскирование старых tool-результатов
    ///   — усечение до [`COMPACT_TOOL_CHARS`] с пометкой, кроме
    ///   [`COMPACT_KEEP_TOOL`] последних;
    /// - прунинг (только пока итог > 100% бюджета): удаление старейших
    ///   не-системных сообщений, хвост из [`COMPACT_KEEP_TAIL`] неприкосновенен;
    /// - L3 (≥ `compact_l3_pct`% или принудительно при HTTP 413): LLM-саммари
    ///   истории до последнего user-сообщения ([`Self::l3_summarize`]);
    ///   если после L3 итог всё равно выше порога — `l3_futile` отключает
    ///   дальнейшие попытки (иначе каждый ход жёг бы API впустую).
    ///
    /// Факт компактификации пишется в журнал.
    async fn compact_history(&mut self, events: &Option<mpsc::Sender<AgentEvent>>) {
        let budget = self.effective_context_budget();
        let l1 = budget * self.config.agent.compact_l1_pct.min(100) / 100;
        let l3 = budget * self.config.agent.compact_l3_pct / 100;
        let mut total: usize = self.history.iter().map(ChatMessage::rough_tokens).sum();
        if total <= l1 {
            return;
        }
        self.fire_hook(crate::hooks::HookEvent::PreCompact, None, "{}");

        // L1: маскирование старых tool-сообщений.
        let truncated = self.mask_old_tool_results();

        // Прунинг: только при реальном переполнении (>100%).
        total = self.history.iter().map(ChatMessage::rough_tokens).sum();
        let mut removed = 0usize;
        while total > budget && self.history.len() > COMPACT_KEEP_TAIL {
            let Some(pos) = self.history.iter().position(|m| m.role != Role::System) else {
                break;
            };
            total = total.saturating_sub(self.history[pos].rough_tokens());
            self.history.remove(pos);
            removed += 1;
        }

        if truncated > 0 || removed > 0 {
            self.log_event(
                "event",
                serde_json::json!({
                    "event": "compact",
                    "truncated_tools": truncated,
                    "removed_messages": removed,
                    "history_len": self.history.len(),
                    "rough_tokens": total,
                }),
            );
        }

        // L3: LLM-саммари при тяжёлом хвосте.
        total = self.history.iter().map(ChatMessage::rough_tokens).sum();
        let mut l3_folded = 0usize;
        if total > l3 && !self.l3_futile {
            match self.l3_summarize().await {
                Ok(from) if from > 0 => {
                    l3_folded = from;
                    let after: usize = self.history.iter().map(ChatMessage::rough_tokens).sum();
                    self.emit_note(
                        events,
                        format!("⚠ L3-компактификация: {from} сообщений → саммари (~{after} ток.)"),
                    );
                    if after > l3 {
                        self.l3_futile = true;
                    }
                }
                Ok(_) => self.l3_futile = true, // резать нечего — не вызывать зря
                Err(e) => {
                    tracing::warn!("L3-саммаризация не удалась: {e}");
                    self.l3_futile = true;
                }
            }
        }
        if truncated > 0 || removed > 0 || l3_folded > 0 {
            // Блок уроков — из реестра, а не из урезанной истории (B5).
            self.refresh_lessons_block();
            self.fire_hook(crate::hooks::HookEvent::PostCompact, None, "{}");
        }
    }

    /// L3: сворачивает историю ДО последнего user-сообщения в одно
    /// assistant-сообщение с саммари (граница по user никогда не разрывает
    /// пары `tool_call/tool_result`). Возвращает число свёрнутых сообщений
    /// (0 — сворачивать нечего).
    ///
    /// # Errors
    /// Ошибка вызова модели-саммаризатора.
    async fn l3_summarize(&mut self) -> Result<usize> {
        let Some(boundary) = self.history.iter().rposition(|m| m.role == Role::User) else {
            return Ok(0);
        };
        if boundary == 0 {
            return Ok(0);
        }
        // Сериализация старого диалога с усечением (общий дамп ≤ 48k символов).
        let mut dump = String::new();
        for m in &self.history[..boundary] {
            let role = match m.role {
                Role::User => "user",
                Role::Assistant => "assistant",
                Role::Tool => "tool",
                Role::System => "system",
            };
            let content: String = m.content.chars().take(1500).collect();
            let _ = writeln!(dump, "[{role}] {content}");
            if dump.chars().count() > 48_000 {
                dump.push_str("\n… [дамп усечён]");
                break;
            }
        }
        let request = ChatRequest {
            messages: vec![
                ChatMessage::system(
                    "Ты — компактификатор контекста агента-архитектора. Сожми диалог \
                     в «заметки для продолжения» по-русски: цель задачи, принятые \
                     решения (со ссылками на артефакты/файлы), что уже сделано, \
                     открытые вопросы, следующие шаги. До 1500 символов.",
                ),
                ChatMessage::user(dump),
            ],
            tools: Vec::new(),
            // Как в distill: temperature не шлём — Kimi k3 принимает только 1
            // (400 «invalid temperature»); default берётся из ModelConfig.
            temperature: None,
            max_tokens: Some(1200),
            thinking: None,
        };
        let summary = self.provider.complete(request).await?.content;
        let summary = if summary.trim().is_empty() {
            "(пустая суммаризация)".to_string()
        } else {
            summary
        };
        let from = boundary;
        let compacted = ChatMessage::assistant(
            format!("CONTEXT COMPACTED ({from} сообщений → саммари): {summary}"),
            Vec::new(),
        );
        self.history.splice(..boundary, [compacted]);
        self.log_event(
            "event",
            serde_json::json!({ "event": "compact_l3", "folded": from }),
        );
        Ok(from)
    }

    /// Строка в журнал сессии (JSONL). Ошибки записи — в tracing, не рвут ход.
    fn log_event(&mut self, kind: &str, extra: Value) {
        let Some(file) = self.log_file.as_mut() else {
            return;
        };
        let mut obj = serde_json::Map::new();
        obj.insert("ts".into(), Value::String(now_iso()));
        obj.insert("kind".into(), Value::String(kind.to_string()));
        if let Value::Object(map) = extra {
            obj.extend(map);
        }
        let mut line = Value::Object(obj).to_string();
        line.push('\n');
        if let Err(e) = std::io::Write::write_all(file, line.as_bytes())
            .and_then(|()| std::io::Write::flush(file))
        {
            tracing::warn!("журнал сессии: {e}");
        }
    }
}

/// Завершение сессии: хук `SessionEnd` (аудит/нотификации в корпоративном
/// контуре). Журнал к этому моменту ещё открыт — событие фиксируется.
impl Drop for AgentSession {
    fn drop(&mut self) {
        if !self.hooks.is_empty() {
            let outcomes = self
                .hooks
                .fire(crate::hooks::HookEvent::SessionEnd, None, "{}");
            if !outcomes.is_empty() {
                self.log_event(
                    "event",
                    serde_json::json!({
                        "event": "hook",
                        "hook_event": "SessionEnd",
                        "outcomes": outcomes.len(),
                    }),
                );
            }
        }
    }
}

/// Гонка «работа против токена отмены»: `None` — отмена победила и работа
/// дропнута (HTTP-запрос оборвётся; ребёнок `harness_run` убит через
/// `kill_on_drop`). Без токена (CLI, субагенты) — просто `.await`.
async fn race_cancel<F: std::future::Future>(
    cancel: Option<CancellationToken>,
    fut: F,
) -> Option<F::Output> {
    match cancel {
        None => Some(fut.await),
        Some(token) => {
            tokio::select! {
                out = fut => Some(out),
                () = token.cancelled() => None,
            }
        }
    }
}

/// Усекает текст до `max` символов (char-safe) с пометкой об усечении.
fn truncate_chars(text: &str, max: usize) -> String {
    let out: String = text.chars().take(max).collect();
    if out.len() < text.len() {
        let mut marked = out;
        let _ = write!(marked, "\n… [усечено до {max} символов]");
        marked
    } else {
        out
    }
}

/// Вердикт-заглушка для вызова с битыми аргументами: `parse_arguments`
/// возвращает [`Value::String`] только при невалидном JSON — то есть ответ
/// модели обрезался на середине аргументов (потолок `max_tokens` либо обрыв
/// потока в сети/прокси). Исполнять половину heredoc'а опаснее, чем
/// отказать: возвращаем модели точную причину и стратегию восстановления
/// (чанкованная запись), иначе она повторяет гигантский вызов по кругу.
fn broken_arguments_verdict(reply: &ChatMessage, call: &crate::llm::ToolCall) -> Option<String> {
    let Value::String(raw) = &call.arguments else {
        return None;
    };
    let kib = raw.len() / 1024;
    let cause = if reply.finish_reason.as_deref() == Some("length") {
        format!(
            "ответ упёрся в потолок max_tokens и обрезался на середине аргументов \
             (успели прийти ~{kib} КБ)"
        )
    } else {
        format!("поток ответа оборвался на середине аргументов (успели прийти ~{kib} КБ)")
    };
    Some(format!(
        "harness: вызов `{}` отклонён без исполнения — {cause}. \
         Не повторяй его целиком: разбей содержимое на части по 8–12 КБ \
         (write_file с mode=\"append\" или bash-дозапись `>>` частями), \
         собери файл по шагам и продолжай.",
        call.name
    ))
}

#[cfg(test)]
mod tests;
