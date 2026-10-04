//! ACP-клиент прогона кодового агента (ADR-057, C2): JSON-RPC 2.0 поверх stdio.
//!
//! Единый протокол вместо зоопарка флагов: `initialize` (capabilities
//! `fs = false`, `terminal = false`, `elicitation = false`) → `session/new`
//! (`cwd` = каталог прогона) → `session/prompt` → чтение стрима
//! `session/update`. Финальный текст ответа — ПОСЛЕДНЕЕ агентское сообщение
//! (агрегат по `messageId`; без id — эвристика «чанки после последнего
//! `tool_call`») — подаётся потребителем как «stdout» для разбора
//! JSON-контракта результата.
//!
//! Границы (честные, ADR-057):
//! - `session/request_permission` — авто-выбор первой allow-опции с
//!   журналированием: это СЕГОДНЯШНИЕ skip-permissions/yolo, не улучшение
//!   security boundary; политика допуска — отдельная волна;
//! - вызовы `fs/*`, `terminal/*`, `elicitation/*` агентом → JSON-RPC error
//!   `-32601` (method not found): capabilities объявлены `false`;
//! - таймаут прогона → `session/cancel` + окно graceful до SIGKILL
//!   процессной группы (последний рубеж — механика [`crate::proc`]).
//!
//! Формат — newline-delimited JSON (NDJSON) поверх stdin/stdout, как в
//! эталонных ACP-реализациях (`@agentclientprotocol/sdk`, `ndJsonStream`).
//! Новые зависимости не вводятся: транспорт — `serde_json` + `tokio`.

use std::path::Path;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::mpsc;

use crate::config::{AcpConfig, AcpMode, CodingHarnessConfig};

use super::{Termination, repo_changed_since};

/// Версия ACP, которую реализует клиент (схема `ProtocolVersion`, текущая — 1).
const ACP_PROTOCOL_VERSION: u64 = 1;
/// Максимум удерживаемого stderr, байт (хвост) — как в headless-пути.
const OUTPUT_CAP: usize = 256 * 1024;
/// Дефолтное окно graceful-остановки после `session/cancel`, секунды.
const CANCEL_GRACE_DEFAULT_SECS: u64 = 10;
/// Короткое окно ожидания ответа на `session/close` (F5): финальный аккорд
/// прогона, не критерий — молчание не удлиняет завершение.
const SESSION_CLOSE_TIMEOUT_SECS: u64 = 2;
/// Потолок журнала событий протокола (длинные прогоны не растят память).
const MAX_JOURNAL: usize = 200;
/// Шаг опроса очереди сообщений, миллисекунды (мелкий — чтобы дедлайны
/// абсолютного и idle-таймаутов срабатывали вовремя).
const POLL_STEP: Duration = Duration::from_millis(250);

/// Разрешённый режим прогона после умного выбора (C3, вариант (а)).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolvedMode {
    /// Headless-путь (поведение до ADR-057).
    Prompt,
    /// ACP-клиент.
    Acp,
}

/// Умный выбор режима (C3): `mode = "auto"` — ACP при задекларированной
/// секции, иначе headless; `acp` — принудительный ACP; `prompt` — headless.
#[must_use]
pub fn resolve_mode(cfg: &CodingHarnessConfig) -> ResolvedMode {
    match cfg.mode {
        AcpMode::Prompt => ResolvedMode::Prompt,
        AcpMode::Acp => ResolvedMode::Acp,
        AcpMode::Auto => {
            if cfg.acp.is_some() {
                ResolvedMode::Acp
            } else {
                ResolvedMode::Prompt
            }
        }
    }
}

/// Фаза, в которой провалился ACP-прогон. От неё зависит умный выбор (C3):
/// провал ИНИЦИАЛИЗАЦИИ при `auto` откатывает на headless, провал прогона —
/// ошибка без отката (агент уже мог изменить рабочее дерево).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcpPhase {
    /// `initialize`/`session/new`: процесс не стартовал как ACP-агент.
    Init,
    /// `session/prompt` и стрим: сессия состоялась, прогон провален.
    Run,
}

/// Ошибка ACP-прогона с фазой — основание для отката в `mode = "auto"`.
#[derive(Debug, Clone)]
pub struct AcpError {
    phase: AcpPhase,
    message: String,
}

impl AcpError {
    /// Провал инициализации (таймаут `initialize`, ошибка протокола, ранний exit).
    #[must_use]
    pub fn init(message: impl Into<String>) -> Self {
        Self {
            phase: AcpPhase::Init,
            message: message.into(),
        }
    }

    /// Провал прогона после установленной сессии (откат не выполняется).
    #[must_use]
    pub fn run(message: impl Into<String>) -> Self {
        Self {
            phase: AcpPhase::Run,
            message: message.into(),
        }
    }

    /// Провал на фазе инициализации — при `mode = "auto"` откат на headless.
    #[must_use]
    pub fn is_init(&self) -> bool {
        self.phase == AcpPhase::Init
    }

    /// Фаза провала.
    #[must_use]
    pub fn phase(&self) -> AcpPhase {
        self.phase
    }

    /// Текст предупреждения для итога прогона при откате ACP → headless.
    #[must_use]
    pub fn fallback_warning(&self) -> String {
        format!(
            "ACP-инициализация провалена ({}): {}. Прогон выполнен headless \
             (mode = \"auto\"); для прогонов, где ACP обязателен, — mode = \"acp\"",
            match self.phase {
                AcpPhase::Init => "фаза initialize",
                AcpPhase::Run => "фаза прогона",
            },
            self.message
        )
    }

    /// Сообщение без фазы.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl std::fmt::Display for AcpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let phase = match self.phase {
            AcpPhase::Init => "инициализация",
            AcpPhase::Run => "прогон",
        };
        write!(f, "ACP ({phase}): {}", self.message)
    }
}

impl std::error::Error for AcpError {}

/// Итог ACP-сессии: финальный текст (как «stdout»), stderr, завершение и
/// метаданные журнала (C5).
#[derive(Debug, Clone)]
pub struct AcpSession {
    /// Финальный текст агента: последнее сообщение по `messageId`, иначе —
    /// агрегат чанков после последнего `tool_call`.
    pub text: String,
    /// stderr ACP-процесса (при прерывании — частичный).
    pub stderr: String,
    /// Код возврата процесса (если завершился сам).
    pub exit_code: Option<i32>,
    /// Как завершился прогон.
    pub termination: Termination,
    /// Версия ACP-адаптера из `initialize` (`agentInfo.name version`), если есть.
    pub adapter: Option<String>,
    /// Сколько раз агент спросил допуск (`session/request_permission`).
    pub permissions: usize,
    /// Журнал событий протокола (`tool_call`/`plan`/`request_permission`) —
    /// для итога прогона.
    pub journal: Vec<String>,
}

/// Запускает ACP-сессию: spawn → initialize → session/new → session/prompt →
/// стрим `session/update` → финальный текст. Изоляция (cwd/env-план волны C,
/// процессная группа) — та же, что у headless-пути.
///
/// # Errors
/// Провал инициализации (нет секции `acp`, таймаут `initialize`, ошибка
/// протокола, ранний exit) — [`AcpPhase::Init`]; провал прогона после
/// установленной сессии — [`AcpPhase::Run`].
pub async fn run_session(
    cfg: &CodingHarnessConfig,
    repo: &Path,
    task: &str,
) -> Result<AcpSession, AcpError> {
    let Some(acp) = cfg.acp.as_ref() else {
        return Err(AcpError::init(
            "секция [harnesses.<имя>.acp] не задекларирована",
        ));
    };
    if acp.binary.trim().is_empty() {
        return Err(AcpError::init("не задан binary ACP-адаптера"));
    }
    let mut session = Session::spawn(cfg, acp, repo)?;
    let started = session.started;

    // --- initialize -------------------------------------------------------
    let id = session
        .send_request(
            "initialize",
            json!({
                "protocolVersion": ACP_PROTOCOL_VERSION,
                "clientCapabilities": {
                    "fs": {"readTextFile": false, "writeTextFile": false},
                    "terminal": false,
                    "elicitation": false,
                },
                "clientInfo": {
                    "name": "arch-harness",
                    "version": env!("CARGO_PKG_VERSION"),
                },
            }),
        )
        .await?;
    let init_deadline = deadline_from(acp.init_timeout_secs);
    match session.pump_until_response(id, init_deadline).await? {
        Wait::Response(res) => {
            session.adapter = adapter_label(&res);
            // Capability `session/close` (F5): без неё метод не зовётся.
            session.session_close_cap = res
                .pointer("/agentCapabilities/sessionCapabilities/close")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let negotiated = res.get("protocolVersion").and_then(Value::as_u64);
            if let Some(v) = negotiated {
                session.journal(format!("initialize: protocolVersion={v}"));
            }
            // F2: непустые `authMethods` — агент требует аутентификацию, которой
            // клиент не поддерживает. Честный отказ (auto → fallback, acp → ошибка),
            // а не «сессия вроде работает».
            if res
                .get("authMethods")
                .and_then(Value::as_array)
                .is_some_and(|methods| !methods.is_empty())
            {
                session.shutdown().await;
                return Err(AcpError::init(
                    "агент требует аутентификацию (authMethods), клиент её не поддерживает",
                ));
            }
        }
        Wait::Error(err) => {
            session.shutdown().await;
            return Err(AcpError::init(format!(
                "initialize вернул ошибку {}: {}",
                err.get("code").and_then(Value::as_i64).unwrap_or(0),
                err.get("message").and_then(Value::as_str).unwrap_or("")
            )));
        }
        Wait::Timeout => {
            session.shutdown().await;
            return Err(AcpError::init(format!(
                "таймаут ответа на initialize ({} с)",
                acp.init_timeout_secs
            )));
        }
        Wait::Closed => {
            session.shutdown().await;
            return Err(AcpError::init(
                "процесс ACP завершился до ответа на initialize (ранний exit)",
            ));
        }
    }

    // --- session/new ------------------------------------------------------
    let cwd = std::fs::canonicalize(repo).unwrap_or_else(|_| repo.to_path_buf());
    let id = session
        .send_request(
            "session/new",
            json!({"cwd": cwd.to_string_lossy(), "mcpServers": []}),
        )
        .await?;
    let new_deadline = deadline_from(acp.init_timeout_secs);
    match session.pump_until_response(id, new_deadline).await? {
        Wait::Response(res) => {
            let Some(sid) = res.get("sessionId").and_then(Value::as_str) else {
                session.shutdown().await;
                return Err(AcpError::init("session/new не вернул sessionId"));
            };
            session.session_id = Some(sid.to_string());
        }
        Wait::Error(err) => {
            session.shutdown().await;
            return Err(AcpError::init(format!(
                "session/new вернул ошибку {}: {}",
                err.get("code").and_then(Value::as_i64).unwrap_or(0),
                err.get("message").and_then(Value::as_str).unwrap_or("")
            )));
        }
        Wait::Timeout => {
            session.shutdown().await;
            return Err(AcpError::init("таймаут ответа на session/new"));
        }
        Wait::Closed => {
            session.shutdown().await;
            return Err(AcpError::init(
                "процесс ACP завершился до ответа на session/new (ранний exit)",
            ));
        }
    }

    // --- session/prompt ---------------------------------------------------
    let sid = session.session_id.clone().unwrap_or_default();
    let prompt_id = session
        .send_request(
            "session/prompt",
            json!({"sessionId": sid, "prompt": [{"type": "text", "text": task}]}),
        )
        .await?;
    let termination = match session.pump_prompt(prompt_id, cfg, repo, started).await {
        Ok(t) => t,
        Err(e) => {
            // Провал прогона — процессную группу завершаем, сирот не оставляем.
            session.shutdown().await;
            return Err(e);
        }
    };
    let text = session.final_text();
    session.finish(termination).await?;

    let stderr = session.take_stderr();
    Ok(AcpSession {
        text,
        stderr,
        exit_code: session
            .child
            .try_wait()
            .ok()
            .flatten()
            .and_then(|s| s.code()),
        termination,
        adapter: session.adapter.clone(),
        permissions: session.permissions,
        journal: session.journal.clone(),
    })
}

/// Дедлайн ожидания ответа: `0` секунд — «без таймаута» (сутки).
fn deadline_from(secs: u64) -> Instant {
    Instant::now() + Duration::from_secs(if secs == 0 { 86_400 } else { secs })
}

/// Метка адаптера из ответа `initialize`: `agentInfo.name version`.
fn adapter_label(res: &Value) -> Option<String> {
    let info = res.get("agentInfo")?;
    let name = info.get("name").and_then(Value::as_str)?;
    let version = info.get("version").and_then(Value::as_str).unwrap_or("");
    Some(if version.is_empty() {
        name.to_string()
    } else {
        format!("{name} {version}")
    })
}

/// Исход ожидания ответа JSON-RPC.
enum Wait {
    /// Ответ пришёл (`result`).
    Response(Value),
    /// Ответ пришёл с ошибкой (`error`).
    Error(Value),
    /// Дедлайн истёк.
    Timeout,
    /// Поток закрылся (процесс завершился).
    Closed,
}

/// Пришедшее сообщение очереди.
enum NextMsg {
    Msg(Value),
    Timeout,
    Closed,
}

/// Сессия ACP: процесс, транспорт и накопленное состояние стрима.
// Явное имя `session_id` точнее протокольного контекста, чем обход линта.
#[allow(clippy::struct_field_names)]
struct Session {
    child: Child,
    stdin: ChildStdin,
    rx: mpsc::UnboundedReceiver<Value>,
    pid: u32,
    started: Instant,
    next_id: u64,
    session_id: Option<String>,
    adapter: Option<String>,
    /// Полный агрегат `agent_message_chunk`.
    all_text: String,
    /// Агрегат `agent_message_chunk` после последнего `tool_call` — фолбэк
    /// финального ответа, когда `messageId` в чанках нет (F1).
    post_tool_text: String,
    /// Агрегат чанков ТЕКУЩЕГО сообщения по `messageId` (F1): смена id —
    /// новое сообщение, финальный ответ — последнее из них.
    coded_text: String,
    /// `messageId` текущего сообщения (F1).
    message_id: Option<String>,
    /// Пришли ли хоть раз чанки с `messageId` (F1): переключает финальный
    /// ответ с эвристики «после последнего `tool_call`» на агрегат по id.
    saw_message_id: bool,
    /// `stopReason` ответа на `session/prompt` (F5) — `end_turn` и др.
    stop_reason: Option<String>,
    /// Capability `sessionCapabilities.close` из `initialize` (F5).
    session_close_cap: bool,
    /// Прогон в фазе отмены (F4): запросы допуска отвечаются `cancelled`,
    /// а не авто-allow — агент не должен получить разрешение на работе.
    cancelling: bool,
    permissions: usize,
    journal: Vec<String>,
    activity: Arc<Mutex<Instant>>,
    stderr_buf: Arc<Mutex<Vec<u8>>>,
    stdout_reader: tokio::task::JoinHandle<()>,
    stderr_reader: tokio::task::JoinHandle<()>,
}

impl Session {
    /// Спавн ACP-процесса с ТЕМ ЖЕ cwd/env-планом, что у headless-пути.
    fn spawn(cfg: &CodingHarnessConfig, acp: &AcpConfig, repo: &Path) -> Result<Self, AcpError> {
        let mut cmd = Command::new(&acp.binary);
        cmd.args(&acp.args).current_dir(repo);
        // Env-политика волны C — общая для обоих режимов (ADR-057, п.4).
        let env_plan =
            crate::harness_env::select(cfg, crate::harness_env::manifest_route(repo).as_ref());
        if let Some(allow) = &env_plan.allow {
            cmd.env_clear();
            for name in allow {
                if let Ok(v) = std::env::var(name) {
                    cmd.env(name, v);
                }
            }
        }
        cmd.envs(&cfg.env);
        #[cfg(unix)]
        cmd.process_group(0);
        cmd.kill_on_drop(true)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(Stdio::piped());

        let mut child = cmd.spawn().map_err(|e| {
            AcpError::init(format!(
                "не удалось запустить ACP-адаптер '{}': {e}",
                acp.binary
            ))
        })?;
        let pid = child.id().unwrap_or(0);
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| AcpError::init("нет stdin ACP-процесса"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| AcpError::init("нет stdout ACP-процесса"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| AcpError::init("нет stderr ACP-процесса"))?;

        let (tx, rx) = mpsc::unbounded_channel::<Value>();
        let activity = Arc::new(Mutex::new(Instant::now()));
        let stderr_buf = Arc::new(Mutex::new(Vec::<u8>::new()));

        let stdout_reader = tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                if let Ok(v) = serde_json::from_str::<Value>(line) {
                    // Отправитель живёт до EOF; получатель опустошает очередь
                    // даже после закрытия потока.
                    let _ = tx.send(v);
                }
            }
        });
        let stderr_reader = {
            let buf = stderr_buf.clone();
            tokio::spawn(async move {
                let mut reader = BufReader::new(stderr);
                let mut chunk = [0u8; 8192];
                loop {
                    match reader.read(&mut chunk).await {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            let mut b = buf
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner);
                            b.extend_from_slice(&chunk[..n]);
                            if b.len() > OUTPUT_CAP {
                                let excess = b.len() - OUTPUT_CAP;
                                b.drain(..excess);
                            }
                        }
                    }
                }
            })
        };

        Ok(Self {
            child,
            stdin,
            rx,
            pid,
            started: Instant::now(),
            next_id: 0,
            session_id: None,
            adapter: None,
            all_text: String::new(),
            post_tool_text: String::new(),
            coded_text: String::new(),
            message_id: None,
            saw_message_id: false,
            stop_reason: None,
            session_close_cap: false,
            cancelling: false,
            permissions: 0,
            journal: Vec::new(),
            activity,
            stderr_buf,
            stdout_reader,
            stderr_reader,
        })
    }

    /// Запись кадра NDJSON в stdin процесса.
    async fn write(&mut self, msg: &Value) -> Result<(), AcpError> {
        let mut line = serde_json::to_string(msg)
            .map_err(|e| AcpError::run(format!("сбой сериализации кадра: {e}")))?;
        line.push('\n');
        self.stdin
            .write_all(line.as_bytes())
            .await
            .map_err(|e| AcpError::run(format!("сбой записи в ACP-процесс: {e}")))?;
        self.stdin
            .flush()
            .await
            .map_err(|e| AcpError::run(format!("сбой flush ACP-процесса: {e}")))
    }

    /// Отправляет запрос и возвращает его id.
    async fn send_request(&mut self, method: &str, params: Value) -> Result<u64, AcpError> {
        self.next_id += 1;
        let id = self.next_id;
        let msg = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        self.write(&msg).await?;
        Ok(id)
    }

    /// Отправляет нотификацию (ответа не ждём).
    async fn notify(&mut self, method: &str, params: Value) -> Result<(), AcpError> {
        let msg = json!({"jsonrpc": "2.0", "method": method, "params": params});
        self.write(&msg).await
    }

    /// Ответ на запрос агента (`result`).
    async fn respond_result(&mut self, id: &Value, result: Value) -> Result<(), AcpError> {
        let msg = json!({"jsonrpc": "2.0", "id": id, "result": result});
        self.write(&msg).await
    }

    /// Ответ на запрос агента (`error`), например `-32601` на fs/terminal/elicit.
    async fn respond_error(
        &mut self,
        id: &Value,
        code: i64,
        message: String,
    ) -> Result<(), AcpError> {
        let msg = json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}});
        self.write(&msg).await
    }

    /// Помечает активность протокола (idle-детект C4).
    fn touch(&self) {
        *self
            .activity
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Instant::now();
    }

    /// Журнал событий протокола (с потолком объёма).
    fn journal(&mut self, entry: String) {
        if self.journal.len() < MAX_JOURNAL {
            self.journal.push(entry);
        }
    }

    /// Следующее сообщение очереди с таймаутом.
    async fn next_message(&mut self, dur: Duration) -> NextMsg {
        match tokio::time::timeout(dur, self.rx.recv()).await {
            Err(_) => NextMsg::Timeout,
            Ok(None) => NextMsg::Closed,
            Ok(Some(v)) => NextMsg::Msg(v),
        }
    }

    /// Ждёт ответ с id `wanted`, обрабатывая встречные нотификации и запросы
    /// агента, до дедлайна.
    async fn pump_until_response(
        &mut self,
        wanted: u64,
        deadline: Instant,
    ) -> Result<Wait, AcpError> {
        loop {
            let now = Instant::now();
            if now >= deadline {
                return Ok(Wait::Timeout);
            }
            let step = (deadline - now).min(POLL_STEP);
            match self.next_message(step).await {
                NextMsg::Timeout => {}
                NextMsg::Closed => return Ok(Wait::Closed),
                NextMsg::Msg(v) => {
                    self.touch();
                    if v.get("method").is_none() {
                        if v.get("id").and_then(Value::as_u64) == Some(wanted) {
                            return Ok(match v.get("error") {
                                Some(err) => Wait::Error(err.clone()),
                                None => {
                                    Wait::Response(v.get("result").cloned().unwrap_or(Value::Null))
                                }
                            });
                        }
                        continue;
                    }
                    self.handle_incoming(&v).await?;
                }
            }
        }
    }

    /// Цикл `session/prompt`: стрим `session/update` до ответа агента либо
    /// абсолютного/idle-таймаута. Таймаут — `session/cancel` + graceful окно,
    /// затем SIGKILL процессной группы.
    async fn pump_prompt(
        &mut self,
        prompt_id: u64,
        cfg: &CodingHarnessConfig,
        repo: &Path,
        started: Instant,
    ) -> Result<Termination, AcpError> {
        let abs_limit = Duration::from_secs(cfg.timeout_secs.max(1));
        let idle_limit =
            (cfg.idle_timeout_secs > 0).then(|| Duration::from_secs(cfg.idle_timeout_secs));
        let scan_interval = idle_limit.map_or(Duration::from_secs(15), |i| {
            (i / 4).clamp(Duration::from_secs(1), Duration::from_secs(15))
        });
        let mut last_scan = SystemTime::now();
        let mut scan_due = Instant::now();
        let mut termination = Termination::Completed;
        let mut closed = false;

        loop {
            if started.elapsed() >= abs_limit {
                termination = Termination::AbsoluteTimeout;
                break;
            }
            if let Some(idle) = idle_limit {
                // Файловый heartbeat — наравне с событиями протокола (C4).
                if Instant::now() >= scan_due {
                    scan_due = Instant::now() + scan_interval;
                    let scan_start = SystemTime::now();
                    if repo_changed_since(repo, last_scan) {
                        self.touch();
                    }
                    last_scan = scan_start;
                }
                if self
                    .activity
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .elapsed()
                    >= idle
                {
                    termination = Termination::IdleTimeout;
                    break;
                }
            }
            let mut budget = abs_limit.saturating_sub(started.elapsed());
            if let Some(idle) = idle_limit {
                let silent = self
                    .activity
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .elapsed();
                budget = budget.min(idle.saturating_sub(silent));
            }
            budget = budget.clamp(Duration::from_millis(1), POLL_STEP);
            match self.next_message(budget).await {
                NextMsg::Timeout => {}
                NextMsg::Closed => {
                    closed = true;
                    break;
                }
                NextMsg::Msg(v) => {
                    self.touch();
                    if v.get("method").is_none() {
                        if v.get("id").and_then(Value::as_u64) == Some(prompt_id) {
                            if let Some(err) = v.get("error") {
                                return Err(AcpError::run(format!(
                                    "session/prompt вернул ошибку {}: {}",
                                    err.get("code").and_then(Value::as_i64).unwrap_or(0),
                                    err.get("message").and_then(Value::as_str).unwrap_or("")
                                )));
                            }
                            let stop = v
                                .get("result")
                                .and_then(|r| r.get("stopReason"))
                                .and_then(Value::as_str);
                            self.stop_reason = stop.map(str::to_string);
                            self.journal(format!(
                                "session/prompt: stopReason={}",
                                stop.unwrap_or("?")
                            ));
                            break;
                        }
                        continue;
                    }
                    self.handle_incoming(&v).await?;
                }
            }
        }

        if closed {
            return Err(AcpError::run(
                "процесс ACP завершился до ответа на session/prompt",
            ));
        }
        if termination != Termination::Completed {
            self.cancel_and_grace(cfg).await;
        }
        Ok(termination)
    }

    /// Мягкая отмена текущего хода: `session/cancel` + окно graceful, затем
    /// SIGKILL процессной группы (существующий последний рубеж).
    async fn cancel_and_grace(&mut self, cfg: &CodingHarnessConfig) {
        // F4: прогон отменяется — агент не должен получить допуск на работе.
        // Висящий `session/request_permission` закрывается `outcome=cancelled`
        // ДО `session/cancel`, чтобы агент не остался ждать ответа.
        self.cancelling = true;
        self.drain_pending_requests().await;
        let sid = self.session_id.clone().unwrap_or_default();
        if let Err(e) = self
            .notify("session/cancel", json!({"sessionId": sid}))
            .await
        {
            tracing::warn!("ACP session/cancel не отправлен: {e}");
        }
        let grace_secs = cfg
            .acp
            .as_ref()
            .map_or(CANCEL_GRACE_DEFAULT_SECS, |a| a.cancel_grace_secs);
        let grace_secs = if grace_secs == 0 {
            CANCEL_GRACE_DEFAULT_SECS
        } else {
            grace_secs
        };
        let deadline = Instant::now() + Duration::from_secs(grace_secs);
        loop {
            if Instant::now() >= deadline {
                break;
            }
            if matches!(self.child.try_wait(), Ok(Some(_))) {
                break;
            }
            // Дочитываем стрим: агент может успеть доиграть ход.
            let step = (deadline - Instant::now()).min(POLL_STEP);
            if let NextMsg::Msg(v) = self.next_message(step).await {
                self.touch();
                if v.get("method").is_some() {
                    let _ = self.handle_incoming(&v).await;
                }
            }
        }
        crate::proc::kill_process_group(self.pid, &mut self.child).await;
        self.journal(format!("session/cancel: graceful-окно {grace_secs} с"));
    }

    /// Дочитывает УЖЕ пришедшие кадры перед `session/cancel` (F4): запрос
    /// допуска, застрявший в очереди к моменту отмены, получает
    /// `outcome=cancelled`, а не авто-allow.
    async fn drain_pending_requests(&mut self) {
        while let Ok(v) = self.rx.try_recv() {
            self.touch();
            if v.get("method").is_some() {
                let _ = self.handle_incoming(&v).await;
            }
        }
    }

    /// Обрабатывает нотификацию `session/update` или запрос агента к клиенту.
    async fn handle_incoming(&mut self, v: &Value) -> Result<(), AcpError> {
        let Some(method) = v.get("method").and_then(Value::as_str) else {
            return Ok(());
        };
        if method == "session/update" {
            self.apply_update(v.get("params"));
            return Ok(());
        }
        // Запрос агента к клиенту: без `id` ответить нельзя — нотификацию
        // неизвестного метода игнорируем.
        let Some(id) = v.get("id") else {
            return Ok(());
        };
        match method {
            // F4: в фазе отмены допуск не выдаём — отвечаем `cancelled`.
            "session/request_permission" if self.cancelling => {
                self.deny_permission_on_cancel(id).await
            }
            "session/request_permission" => self.auto_allow(id, v.get("params")).await,
            m if is_capability_method(m) => {
                self.respond_error(
                    id,
                    -32601,
                    format!(
                        "метод '{m}' не поддержан: capabilities fs/terminal/elicitation \
                         объявлены false"
                    ),
                )
                .await
            }
            m => {
                self.respond_error(id, -32601, format!("клиентский метод '{m}' не поддержан"))
                    .await
            }
        }
    }

    /// Применяет `session/update` к накопленному состоянию (C2/C4).
    fn apply_update(&mut self, params: Option<&Value>) {
        let Some(update) = params.and_then(|p| p.get("update")) else {
            return;
        };
        match update.get("sessionUpdate").and_then(Value::as_str) {
            Some("agent_message_chunk") => {
                if let Some(text) = update
                    .get("content")
                    .and_then(|c| c.get("text"))
                    .and_then(Value::as_str)
                {
                    self.all_text.push_str(text);
                    self.post_tool_text.push_str(text);
                    self.apply_chunk_text(update, text);
                }
            }
            Some("tool_call") => {
                // Фолбэк-эвристика: текст ПОСЛЕ последнего tool_call. При
                // агрегации по `messageId` (F1) tool_call сообщение не режет.
                self.post_tool_text.clear();
                let title = update
                    .get("title")
                    .and_then(Value::as_str)
                    .unwrap_or("tool");
                self.journal(format!("tool_call: {title}"));
            }
            Some("tool_call_update") => self.journal(String::from("tool_call_update")),
            Some("plan") => self.journal(String::from("plan")),
            // F3: расширяемость протокола — незнакомый `sessionUpdate`
            // журналируется и игнорируется, прогон не рушится.
            other => self.journal(format!(
                "session/update: неизвестный вариант {} — игнор",
                other.unwrap_or("(отсутствует)")
            )),
        }
    }

    /// Агрегация чанков по `messageId` (F1): смена id или первое появление —
    /// новое сообщение; тот же id — дописывание. `messageId` читается из
    /// `update.content.messageId` (а также `update.messageId` — толерантно).
    /// Без id во ВСЕХ чанках режим не включается — работает фолбэк `post_tool_text`.
    fn apply_chunk_text(&mut self, update: &Value, text: &str) {
        let id = update
            .get("messageId")
            .or_else(|| update.get("content").and_then(|c| c.get("messageId")))
            .and_then(Value::as_str);
        match id {
            Some(id) => {
                if !self.saw_message_id || self.message_id.as_deref() != Some(id) {
                    self.coded_text.clear();
                }
                self.saw_message_id = true;
                self.message_id = Some(id.to_string());
                self.coded_text.push_str(text);
            }
            // Чанк без id при активном id-режиме — продолжение текущего сообщения.
            None if self.saw_message_id => self.coded_text.push_str(text),
            None => {}
        }
    }

    /// Отказ в допуске при отмене прогона (F4): `outcome=cancelled`, чтобы
    /// агент не остался ждать ответа и не принял допуск за состоявшееся решение.
    async fn deny_permission_on_cancel(&mut self, id: &Value) -> Result<(), AcpError> {
        self.permissions += 1;
        self.journal(String::from(
            "request_permission: прогон отменён → outcome=cancelled",
        ));
        self.respond_result(id, json!({"outcome": {"outcome": "cancelled"}}))
            .await
    }

    /// `session/request_permission`: авто-выбор первой allow-опции + журнал.
    /// Это сегодняшние skip-permissions — security boundary не улучшается.
    async fn auto_allow(&mut self, id: &Value, params: Option<&Value>) -> Result<(), AcpError> {
        self.permissions += 1;
        let tool = params
            .and_then(|p| p.get("toolCall"))
            .and_then(|t| t.get("title").or_else(|| t.get("toolCallId")))
            .and_then(Value::as_str)
            .unwrap_or("tool")
            .to_string();
        let options = params
            .and_then(|p| p.get("options"))
            .and_then(Value::as_array);
        let listed = options
            .map(|o| {
                o.iter()
                    .map(|opt| {
                        format!(
                            "{}:{}",
                            opt.get("kind").and_then(Value::as_str).unwrap_or("?"),
                            opt.get("name").and_then(Value::as_str).unwrap_or("?")
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default();
        let chosen = options.map(Vec::as_slice).and_then(pick_allow);
        self.journal(format!(
            "request_permission tool={tool} options=[{listed}] chosen={}",
            chosen.as_deref().unwrap_or("(нет allow-опции)")
        ));
        match chosen {
            Some(option_id) => {
                self.respond_result(
                    id,
                    json!({"outcome": {"outcome": "selected", "optionId": option_id}}),
                )
                .await
            }
            // Нет ни одной allow-опции — честнее отменить, чем выбрать reject.
            None => {
                self.respond_result(id, json!({"outcome": {"outcome": "cancelled"}}))
                    .await
            }
        }
    }

    /// Финальный текст: при наличии `messageId` — ПОСЛЕДНЕЕ агентское
    /// сообщение (агрегат последнего id, F1); иначе фолбэк — агрегат чанков
    /// после последнего `tool_call`, а если после инструментов текста не было —
    /// общий агрегат.
    fn final_text(&self) -> String {
        if self.saw_message_id {
            return self.coded_text.clone();
        }
        if self.post_tool_text.trim().is_empty() {
            self.all_text.clone()
        } else {
            self.post_tool_text.clone()
        }
    }

    /// Завершает сессию: закрывает stdin (EOF для агента), ждёт короткое окно,
    /// затем завершает процессную группу.
    async fn finish(&mut self, termination: Termination) -> Result<(), AcpError> {
        // F5: ход завершён `end_turn` и агент заявил cap `session/close` —
        // освобождаем ресурсы сессии до закрытия транспорта. Без cap или не
        // по `end_turn` — как раньше.
        if termination == Termination::Completed
            && self.session_close_cap
            && self.stop_reason.as_deref() == Some("end_turn")
        {
            self.close_session().await;
        }
        if termination == Termination::Completed {
            let _ = self.stdin.shutdown().await;
            let deadline = Instant::now() + Duration::from_secs(2);
            while Instant::now() < deadline {
                if matches!(self.child.try_wait(), Ok(Some(_))) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
        if !matches!(self.child.try_wait(), Ok(Some(_))) {
            crate::proc::kill_process_group(self.pid, &mut self.child).await;
        }
        let _ = tokio::time::timeout(Duration::from_secs(2), &mut self.stdout_reader).await;
        let _ = tokio::time::timeout(Duration::from_secs(2), &mut self.stderr_reader).await;
        Ok(())
    }

    /// `session/close` по capability (F5): освобождение ресурсов сессии после
    /// `end_turn`. Ответ ждём в коротком окне; сбой или молчание — запись в
    /// журнал, но не провал прогона (финальный аккорд, не критерий приёмки).
    async fn close_session(&mut self) {
        let sid = self.session_id.clone().unwrap_or_default();
        let id = match self
            .send_request("session/close", json!({"sessionId": sid}))
            .await
        {
            Ok(id) => id,
            Err(e) => {
                self.journal(format!("session/close: не отправлен ({e})"));
                return;
            }
        };
        let deadline = Instant::now() + Duration::from_secs(SESSION_CLOSE_TIMEOUT_SECS);
        let note = match self.pump_until_response(id, deadline).await {
            Ok(Wait::Response(_)) => String::from("session/close: подтверждён"),
            Ok(Wait::Timeout) => String::from("session/close: без ответа в окне"),
            Ok(Wait::Closed | Wait::Error(_)) => {
                String::from("session/close: завершён без подтверждения")
            }
            Err(e) => format!("session/close: сбой ({e})"),
        };
        self.journal(note);
    }

    /// Аварийное завершение до установления сессии (провал инициализации).
    async fn shutdown(&mut self) {
        if !matches!(self.child.try_wait(), Ok(Some(_))) {
            crate::proc::kill_process_group(self.pid, &mut self.child).await;
        }
        let _ = tokio::time::timeout(Duration::from_secs(2), &mut self.stdout_reader).await;
        let _ = tokio::time::timeout(Duration::from_secs(2), &mut self.stderr_reader).await;
    }

    /// Забирает накопленный stderr.
    fn take_stderr(&self) -> String {
        String::from_utf8_lossy(
            &self
                .stderr_buf
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        )
        .into_owned()
    }
}

/// Методы, отнесённые к неподдержанным возможностям клиента (C2).
fn is_capability_method(method: &str) -> bool {
    method.starts_with("fs/")
        || method.starts_with("terminal/")
        || method.starts_with("elicitation/")
}

/// Первая allow-опция: `allow_once`, затем `allow_always`; иначе `None`
/// (авто-отмена — выбирать reject за человека нельзя).
fn pick_allow(options: &[Value]) -> Option<String> {
    let by_kind = |kind: &str| {
        options
            .iter()
            .find(|o| o.get("kind").and_then(Value::as_str) == Some(kind))
            .and_then(|o| o.get("optionId"))
            .and_then(Value::as_str)
            .map(str::to_string)
    };
    by_kind("allow_once").or_else(|| by_kind("allow_always"))
}

/// Путь к фикстуре-«агенту» для тестов — self-contained python-скрипт.
#[cfg(test)]
fn fixture() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/acp_agent.py")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    /// Адаптер с ACP-секцией против фикстуры; `mode` — поведение фикстуры.
    fn cfg(mode: &str, acp_mode: AcpMode) -> CodingHarnessConfig {
        CodingHarnessConfig {
            binary: String::new(),
            args: Vec::new(),
            env: BTreeMap::new(),
            env_allow: Vec::new(),
            mode: acp_mode,
            acp: Some(AcpConfig {
                binary: "python3".into(),
                args: vec![fixture().to_string_lossy().into_owned(), mode.to_string()],
                init_timeout_secs: 3,
                cancel_grace_secs: 1,
            }),
            timeout_secs: 20,
            idle_timeout_secs: 0,
            ..CodingHarnessConfig::default()
        }
    }

    fn session(mode: &str) -> CodingHarnessConfig {
        cfg(mode, AcpMode::Acp)
    }

    /// Базовая последовательность: initialize → new → prompt → стрим → финал.
    #[tokio::test]
    async fn handshake_stream_and_final_text() {
        let tmp = tempfile::tempdir().expect("tmp");
        let s = run_session(&session("ok"), tmp.path(), "задача")
            .await
            .expect("ACP-сессия");
        assert_eq!(s.termination, Termination::Completed);
        assert!(s.text.contains("готово"), "text: {}", s.text);
        assert!(
            s.text.contains("\"status\": \"complete\""),
            "финальный текст несёт JSON-контракт: {}",
            s.text
        );
        // Промежуточная реплика ДО tool_call в финальный текст не входит.
        assert!(
            !s.text.contains("читаю задачу"),
            "текст до tool_call отброшен: {}",
            s.text
        );
        assert_eq!(s.permissions, 0);
        assert_eq!(s.adapter.as_deref(), Some("fixture-agent 0.0.1"));
        assert_eq!(s.exit_code, Some(0));
    }

    /// Весь текст (без `tool_call`) — финальный ответ целиком.
    #[tokio::test]
    async fn text_without_tool_calls_is_final() {
        let tmp = tempfile::tempdir().expect("tmp");
        let s = run_session(&session("plain"), tmp.path(), "задача")
            .await
            .expect("ACP-сессия");
        assert_eq!(s.termination, Termination::Completed);
        assert!(s.text.contains("без инструментов"), "{}", s.text);
        assert_eq!(s.permissions, 0);
    }

    /// `session/request_permission` → авто-выбор `allow_once` + журнал.
    #[tokio::test]
    async fn permission_request_auto_allows_once() {
        let tmp = tempfile::tempdir().expect("tmp");
        let s = run_session(&session("permission"), tmp.path(), "задача")
            .await
            .expect("ACP-сессия");
        assert_eq!(s.termination, Termination::Completed);
        assert_eq!(s.permissions, 1, "журнал: {:?}", s.journal);
        // Фикстура подтверждает, что клиент выбрал именно allow_once.
        assert!(s.text.contains("chosen=allow-once-1"), "{}", s.text);
        assert!(
            s.journal.iter().any(|j| j.contains("request_permission")),
            "{:?}",
            s.journal
        );
    }

    /// Запрос fs-метода агентом → error -32601 (capabilities false).
    #[tokio::test]
    async fn fs_method_is_rejected_with_32601() {
        let tmp = tempfile::tempdir().expect("tmp");
        let s = run_session(&session("fs"), tmp.path(), "задача")
            .await
            .expect("ACP-сессия");
        assert_eq!(s.termination, Termination::Completed);
        assert!(
            s.text.contains("fs_error=-32601"),
            "клиент обязан ответить -32601: {}",
            s.text
        );
    }

    /// Провал инициализации: ранний exit процесса.
    #[tokio::test]
    async fn early_exit_is_init_error() {
        let tmp = tempfile::tempdir().expect("tmp");
        let err = run_session(&session("init-exit"), tmp.path(), "задача")
            .await
            .expect_err("ранний exit — ошибка");
        assert!(err.is_init(), "{err}");
        assert!(err.message().contains("initialize"), "{err}");
    }

    /// Провал инициализации: таймаут ответа на initialize.
    #[tokio::test]
    async fn init_timeout_is_init_error() {
        let tmp = tempfile::tempdir().expect("tmp");
        let err = run_session(&session("init-silent"), tmp.path(), "задача")
            .await
            .expect_err("таймаут — ошибка");
        assert!(err.is_init(), "{err}");
        assert!(err.message().contains("таймаут"), "{err}");
    }

    /// Таймаут прогона → session/cancel + graceful; фикстура подтверждает cancel.
    #[tokio::test]
    async fn absolute_timeout_cancels_session() {
        let tmp = tempfile::tempdir().expect("tmp");
        let mut c = session("cancel");
        c.timeout_secs = 2;
        let s = run_session(&c, tmp.path(), "задача")
            .await
            .expect("ACP-сессия");
        assert_eq!(s.termination, Termination::AbsoluteTimeout);
        // Фикстура пишет файл-маркер, получив session/cancel: окно graceful
        // дало ей доиграть ход, SIGKILL не понадобился.
        assert!(
            tmp.path().join("cancel-received").is_file(),
            "агент обязан получить session/cancel"
        );
        assert!(
            s.journal.iter().any(|j| j.contains("session/cancel")),
            "{:?}",
            s.journal
        );
    }

    /// Idle-детект: молчание стрима дольше `idle_timeout` → прерывание.
    #[tokio::test]
    async fn idle_timeout_fires_on_silent_stream() {
        let tmp = tempfile::tempdir().expect("tmp");
        let mut c = session("idle");
        c.timeout_secs = 60;
        c.idle_timeout_secs = 2;
        let s = run_session(&c, tmp.path(), "задача")
            .await
            .expect("ACP-сессия");
        assert_eq!(s.termination, Termination::IdleTimeout);
    }

    /// Активный стрим событий не считается тишиной (C4).
    #[tokio::test]
    async fn active_stream_is_not_interrupted() {
        let tmp = tempfile::tempdir().expect("tmp");
        let mut c = session("active");
        c.timeout_secs = 60;
        // Запас против нагрузки тест-сьюта: чанки идут каждые 0.2 с при окне 3 с.
        c.idle_timeout_secs = 3;
        let s = run_session(&c, tmp.path(), "задача")
            .await
            .expect("ACP-сессия");
        assert_eq!(
            s.termination,
            Termination::Completed,
            "активный стрим не прерывается: {:?}",
            s.journal
        );
        assert!(s.text.contains("поток завершён"), "{}", s.text);
    }

    /// Env-политика волны C применяется и в ACP-режиме: незаявленная
    /// переменная окружения до процесса не доходит.
    #[tokio::test]
    async fn env_whitelist_applies_to_acp_process() {
        let tmp = tempfile::tempdir().expect("tmp");
        let mut c = session("env");
        // Строгий whitelist: только PATH (HOME/LANG/TERM/TMPDIR не критичны,
        // но без PATH не найдётся python3 — он задаётся абсолютным путём ниже).
        let py = which_python();
        if let Some(acp) = c.acp.as_mut() {
            acp.binary = py;
        }
        c.env_allow = vec!["PATH".into()];
        let s = run_session(&c, tmp.path(), "задача")
            .await
            .expect("ACP-сессия");
        assert_eq!(s.termination, Termination::Completed);
        assert!(
            s.text.contains("LEAK=none"),
            "секрет окружения не должен наследоваться: {}",
            s.text
        );
    }

    /// F1: два агентских сообщения с разными `messageId` и `tool_call` между
    /// ними — финальный ответ = ПОСЛЕДНЕЕ сообщение, не «текст после `tool_call`».
    #[tokio::test]
    async fn final_text_is_last_message_by_message_id() {
        let tmp = tempfile::tempdir().expect("tmp");
        let s = run_session(&session("multi-message"), tmp.path(), "задача")
            .await
            .expect("ACP-сессия");
        assert_eq!(s.termination, Termination::Completed);
        assert!(s.text.contains("второе сообщение"), "{}", s.text);
        assert!(
            !s.text.contains("первое сообщение"),
            "финал — только последнее сообщение: {}",
            s.text
        );
        assert!(
            s.text.contains("\"status\": \"complete\""),
            "финальный текст несёт JSON-контракт: {}",
            s.text
        );
    }

    /// F2: непустые `authMethods` — честный отказ инициализации.
    #[tokio::test]
    async fn auth_methods_is_init_error() {
        let tmp = tempfile::tempdir().expect("tmp");
        let err = run_session(&session("auth"), tmp.path(), "задача")
            .await
            .expect_err("authMethods — клиент не поддерживает аутентификацию");
        assert!(err.is_init(), "{err}");
        assert!(err.message().contains("authMethods"), "{err}");
    }

    /// F3: незнакомый `sessionUpdate` журналируется, но не рушит прогон.
    #[tokio::test]
    async fn unknown_session_update_is_journaled_not_fatal() {
        let tmp = tempfile::tempdir().expect("tmp");
        let s = run_session(&session("unknown-update"), tmp.path(), "задача")
            .await
            .expect("ACP-сессия");
        assert_eq!(s.termination, Termination::Completed);
        assert!(
            s.text.contains("\"status\": \"complete\""),
            "прогон продолжился после незнакомого события: {}",
            s.text
        );
        assert!(
            s.journal.iter().any(|j| j.contains("custom_update")),
            "незнакомый вариант в журнале: {:?}",
            s.journal
        );
    }

    /// F4: висящий запрос допуска при отмене получает `outcome=cancelled`,
    /// прогон завершается отменой (не зависанием).
    #[tokio::test]
    async fn pending_permission_denied_on_cancel() {
        let tmp = tempfile::tempdir().expect("tmp");
        let mut c = session("perm-cancel");
        c.timeout_secs = 2;
        let s = run_session(&c, tmp.path(), "задача")
            .await
            .expect("ACP-сессия");
        assert_eq!(s.termination, Termination::AbsoluteTimeout);
        assert!(
            tmp.path().join("cancel-received").is_file(),
            "агент обязан получить session/cancel"
        );
        let outcome = std::fs::read_to_string(tmp.path().join("perm-outcome"))
            .expect("агент записал исход допуска");
        assert!(
            outcome.contains("cancelled"),
            "при отмене допуск не выдаётся: {outcome}"
        );
        assert!(
            s.journal
                .iter()
                .any(|j| j.contains("request_permission") && j.contains("cancelled")),
            "{:?}",
            s.journal
        );
    }

    /// F5: при cap `sessionCapabilities.close` и `end_turn` клиент зовёт
    /// `session/close`; агент отвечает и фиксирует факт.
    #[tokio::test]
    async fn session_close_sent_when_capability_declared() {
        let tmp = tempfile::tempdir().expect("tmp");
        let s = run_session(&session("close"), tmp.path(), "задача")
            .await
            .expect("ACP-сессия");
        assert_eq!(s.termination, Termination::Completed);
        assert!(
            tmp.path().join("session-closed").is_file(),
            "агент обязан получить session/close"
        );
        assert!(
            s.journal.iter().any(|j| j.contains("session/close")),
            "{:?}",
            s.journal
        );
        assert!(s.text.contains("\"status\": \"complete\""), "{}", s.text);
    }

    /// F5 (негатив): без cap `session/close` метод не зовётся.
    #[tokio::test]
    async fn session_close_not_called_without_capability() {
        let tmp = tempfile::tempdir().expect("tmp");
        let s = run_session(&session("ok"), tmp.path(), "задача")
            .await
            .expect("ACP-сессия");
        assert!(
            s.journal.iter().all(|j| !j.contains("session/close")),
            "без cap метод не зовётся: {:?}",
            s.journal
        );
        assert!(!tmp.path().join("session-closed").is_file());
    }

    /// Абсолютный путь к python3 (whitelist может не содержать PATH).
    fn which_python() -> String {
        for candidate in ["/usr/bin/python3", "/usr/local/bin/python3"] {
            if Path::new(candidate).is_file() {
                return candidate.to_string();
            }
        }
        String::from("python3")
    }
}
