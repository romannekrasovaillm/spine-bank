//! Адаптеры кодовых харнессов: запуск исполнителей по handoff-пакету.
//!
//! КОНТРАКТ (владелец: агент `harness`):
//! - известные харнессы: claude-code, qwen-code, openclaw, hermes, theseus,
//!   codewhale, kimi-code ([`known`]); конфиги — из `Config::harnesses`;
//! - генерация handoff-пакета (`<repo>/.arch-handoff/`) живёт в
//!   [`crate::handoff`] (core-модуль, чисто файловый: TASK.md, ARCHITECTURE.md,
//!   CONSTRAINTS.yaml, SPEC.md, RUBRIC.yaml, ROLLBACK.yaml, MANIFEST.json, adr/,
//!   git-предгейт baseline); этот модуль — только ПРОГОН: он доступен лишь
//!   в сборке `harness`;
//! - [`run_harness`] — запуск бинаря харнесса (`PromptMode` positional/flag/stdin)
//!   в каталоге repo, таймаут, захват stdout/stderr → `HarnessRun`;
//! - [`tools`] — инструмент `harness_run` для агентного цикла (прогон пакета
//!   харнессом — только через `harness_run`, не bash);
//!   `harness_run(background=true)` — фоновый прогон: инструмент возвращается
//!   сразу (задача `hr-*` в общем реестре фоновых задач), агент остаётся
//!   доступным пользователю, результат — через `subagent_result`.

use std::fmt::Write as _;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use crate::config::{CodingHarnessConfig, Config, PromptMode};
use crate::error::{HarnessError, Result};
use crate::handoff::{HANDOFF_DIR, git_out, recommended_timeout_secs};
use crate::llm::ToolSpec;
use crate::proc::kill_process_group;
use crate::tool::{Tool, ToolContext, ToolOutput};

/// Минимальный абсолютный таймаут прогона кодового харнесса, секунд:
/// меньшие значения (модель оптимистично просит «5 минут») поднимаются —
/// ранний обрыв оставлял репозиторий в полусобранном состоянии.
const MIN_HARNESS_TIMEOUT_SECS: u64 = 600;

/// Имена известных кодовых харнессов.
#[must_use]
pub fn known() -> Vec<&'static str> {
    vec![
        "claude-code",
        "qwen-code",
        "openclaw",
        "hermes",
        "theseus",
        "codewhale",
        "kimi-code",
    ]
}

/// Итог прогона кодового харнесса.
#[derive(Debug, Clone)]
pub struct HarnessRun {
    /// Имя харнесса.
    pub harness: String,
    /// Код возврата.
    pub exit_code: Option<i32>,
    /// stdout (при прерывании — частичный).
    pub stdout: String,
    /// stderr (при прерывании — частичный).
    pub stderr: String,
    /// Длительность, секунды.
    pub duration_secs: f64,
    /// Как завершился прогон.
    pub termination: Termination,
    /// Авто-коммит незакоммиченных правок исполнителя (None — не потребовался:
    /// дерево чистое, прогон прерван, репозиторий не git или опция выключена).
    pub auto_commit: Option<AutoCommit>,
    /// Механически разобранный JSON-контракт результата из stdout
    /// (валидация схемы — [`parse_result_contract`]).
    pub contract: ContractParse,
}

/// Итог авто-коммита оставшихся после исполнителя правок.
#[derive(Debug, Clone)]
pub struct AutoCommit {
    /// Сколько путей вошло в коммит.
    pub files: usize,
    /// Короткий хеш коммита.
    pub hash: String,
    /// Сообщение коммита.
    pub message: String,
}

/// Способ завершения прогона харнесса.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Termination {
    /// Процесс завершился сам.
    Completed,
    /// Прерван по абсолютному потолку `timeout_secs`.
    AbsoluteTimeout,
    /// Прерван по таймауту тишины: нет вывода и изменений файлов репо
    /// дольше `idle_timeout_secs`.
    IdleTimeout,
}

impl std::fmt::Display for Termination {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Completed => write!(f, "завершён"),
            Self::AbsoluteTimeout => write!(f, "абсолютный таймаут"),
            Self::IdleTimeout => write!(f, "idle-таймаут (тишина)"),
        }
    }
}

/// Собирает argv и данные для stdin по режиму [`PromptMode`]:
/// - Positional: `args + [task]`;
/// - Flag: подстановка `{prompt}` в args, иначе `args + [task]`;
/// - Stdin: `args`, задача уходит в stdin.
fn build_argv(cfg: &CodingHarnessConfig, task: &str) -> (Vec<String>, Option<String>) {
    match cfg.prompt_mode {
        PromptMode::Positional => {
            let mut argv = cfg.args.clone();
            argv.push(task.into());
            (argv, None)
        }
        PromptMode::Flag => {
            if cfg.args.iter().any(|a| a.contains("{prompt}")) {
                (
                    cfg.args
                        .iter()
                        .map(|a| a.replace("{prompt}", task))
                        .collect(),
                    None,
                )
            } else {
                let mut argv = cfg.args.clone();
                argv.push(task.into());
                (argv, None)
            }
        }
        PromptMode::Stdin => (cfg.args.clone(), Some(task.into())),
    }
}

/// Максимум удерживаемого вывода каждого потока (stdout/stderr), байт —
/// при превышении хранится хвост (начало важно редко, диагностика в конце).
const OUTPUT_CAP: usize = 256 * 1024;

/// Запускает кодовый харнесс с задачей в репозитории.
///
/// Бинарь запускается с `cwd = repo` в СОБСТВЕННОЙ процессной группе
/// (`process_group(0)`): харнессы — обёртки вокруг node/python и плодят
/// дочерние процессы; при прерывании убивается ВСЯ группа (TERM → grace →
/// KILL), поэтому сирот (как живой Claude Code после таймаута обёртки)
/// не остаётся.
///
/// Умные таймауты:
/// - абсолютный потолок — `cfg.timeout_secs`;
/// - таймаут тишины — `cfg.idle_timeout_secs` (0 выключает): активность =
///   вывод в stdout/stderr ИЛИ свежие mtime файлов репозитория (молчащий,
///   но работающий харнесс не трогаем).
///
/// При прерывании возвращается Ok с частичным выводом и
/// [`Termination`] ≠ Completed — вызывающий видит, что харнесс успел сделать.
///
/// # Errors
/// Бинарь не найден (с подсказкой по установке/конфигу), сбой запуска/ожидания.
pub async fn run_harness(
    name: &str,
    cfg: &CodingHarnessConfig,
    repo: &Path,
    task: &str,
) -> Result<HarnessRun> {
    use std::sync::Mutex;
    use tokio::io::AsyncReadExt;

    // Читатели потоков: перекладывают в ограниченные буферы и трогают heartbeat.
    fn spawn_reader<R>(
        mut pipe: R,
        buf: Arc<Mutex<Vec<u8>>>,
        act: Arc<Mutex<Instant>>,
    ) -> tokio::task::JoinHandle<()>
    where
        R: tokio::io::AsyncRead + Unpin + Send + 'static,
    {
        tokio::spawn(async move {
            let mut chunk = [0u8; 8192];
            loop {
                match pipe.read(&mut chunk).await {
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
                        drop(b);
                        *act.lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner) = Instant::now();
                    }
                }
            }
        })
    }

    let (argv, stdin_data) = build_argv(cfg, task);
    let mut cmd = Command::new(&cfg.binary);
    cmd.args(&argv).current_dir(repo);
    // Whitelist окружения: чужие переменные хоста (модели, прокси, ключи)
    // не протекают в дочерний процесс; `env` адаптера — поверх всегда.
    if !cfg.env_allow.is_empty() {
        cmd.env_clear();
        for name in &cfg.env_allow {
            if let Ok(v) = std::env::var(name) {
                cmd.env(name, v);
            }
        }
    }
    cmd.envs(&cfg.env);
    // Своя процессная группа: убивать будем группу целиком (unix; на
    // остальных ОС дерева нет — kill_on_drop добирает только сам процесс).
    #[cfg(unix)]
    cmd.process_group(0);
    cmd.kill_on_drop(true)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if stdin_data.is_some() {
        cmd.stdin(Stdio::piped());
    } else {
        cmd.stdin(Stdio::null());
    }

    let started = Instant::now();
    let mut child = cmd.spawn().map_err(|e| {
        if e.kind() == ErrorKind::NotFound {
            HarnessError::Harness(format!(
                "бинарь '{}' не найден: установите {} или поправьте config.toml [harnesses.{name}]",
                cfg.binary, cfg.binary
            ))
        } else {
            HarnessError::Harness(format!("не удалось запустить '{}': {e}", cfg.binary))
        }
    })?;
    let pid = child.id().unwrap_or(0);

    // Активность: последний вывод ИЛИ свежая файловая активность в репо.
    let activity = Arc::new(Mutex::new(Instant::now()));
    let stdout_buf = Arc::new(Mutex::new(Vec::<u8>::new()));
    let stderr_buf = Arc::new(Mutex::new(Vec::<u8>::new()));

    let mut readers = Vec::new();
    if let Some(out) = child.stdout.take() {
        readers.push(spawn_reader(out, stdout_buf.clone(), activity.clone()));
    }
    if let Some(err) = child.stderr.take() {
        readers.push(spawn_reader(err, stderr_buf.clone(), activity.clone()));
    }

    // Пишем задачу в stdin отдельной задачей, чтобы не было дедлока на
    // заполненном pipe-буфере, пока читаются stdout/stderr.
    let writer = match (child.stdin.take(), stdin_data) {
        (Some(mut pipe), Some(data)) => Some(tokio::spawn(async move {
            // Ошибка записи осознанно игнорируется: процесс вправе закрыть stdin раньше.
            let _ = pipe.write_all(data.as_bytes()).await;
            // drop(pipe) закрывает stdin — EOF для процесса.
        })),
        _ => None,
    };

    let abs_limit = Duration::from_secs(cfg.timeout_secs.max(1));
    let idle_limit =
        (cfg.idle_timeout_secs > 0).then(|| Duration::from_secs(cfg.idle_timeout_secs));
    // Файловый heartbeat: скан не чаще раза в 15 с (и не реже четверти
    // idle-окна, чтобы мелкие окна тоже ловили активность); базовая отсечка —
    // старт прогона (старые файлы репо активностью не считаются).
    let scan_interval = idle_limit.map_or(Duration::from_secs(15), |i| {
        (i / 4).clamp(Duration::from_secs(1), Duration::from_secs(15))
    });
    let mut last_scan = std::time::SystemTime::now();
    let mut scan_due = Instant::now();

    let termination = loop {
        match child.try_wait() {
            Ok(Some(_)) => break Termination::Completed,
            Ok(None) => {}
            Err(e) => {
                return Err(HarnessError::Harness(format!(
                    "сбой ожидания '{}': {e}",
                    cfg.binary
                )));
            }
        }
        let elapsed = started.elapsed();
        if elapsed >= abs_limit {
            kill_process_group(pid, &mut child).await;
            break Termination::AbsoluteTimeout;
        }
        if let Some(idle) = idle_limit {
            if Instant::now() >= scan_due {
                scan_due = Instant::now() + scan_interval;
                let scan_start = std::time::SystemTime::now();
                if repo_changed_since(repo, last_scan) {
                    *activity
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner) = Instant::now();
                }
                last_scan = scan_start;
            }
            let silent_for = activity
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .elapsed();
            if silent_for >= idle {
                kill_process_group(pid, &mut child).await;
                break Termination::IdleTimeout;
            }
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    };

    if let Some(w) = &writer {
        w.abort();
    }
    // Читатели завершаются по EOF на закрытых пайпах; страховочный лимит.
    for r in readers {
        let _ = tokio::time::timeout(Duration::from_secs(2), r).await;
    }

    let take = |b: &Arc<Mutex<Vec<u8>>>| {
        String::from_utf8_lossy(&b.lock().unwrap_or_else(std::sync::PoisonError::into_inner))
            .into_owned()
    };
    // Страховка финализации: контракт TASK.md требует от исполнителя
    // финальный git-коммит; не сделал — фиксируем сами, иначе работа
    // теряется для оркестратора (результат забирается из git).
    let auto_commit = if termination == Termination::Completed && cfg.auto_commit {
        auto_commit_leftovers(repo, name, task)
    } else {
        None
    };
    let stdout = take(&stdout_buf);
    let stderr = take(&stderr_buf);
    // Контракт разбирается один раз на стороне запуска — механически,
    // а не эвристикой у потребителей.
    let contract = parse_result_contract(&stdout);
    Ok(HarnessRun {
        harness: name.into(),
        exit_code: child.try_wait().ok().flatten().and_then(|s| s.code()),
        stdout,
        stderr,
        duration_secs: started.elapsed().as_secs_f64(),
        termination,
        auto_commit,
        contract,
    })
}

/// Коммитит незакоммиченные правки исполнителя (кроме `.arch-handoff/` и
/// мусора `__pycache__/`/`*.pyc`/`.pytest_cache/`). None — не git-репозиторий
/// или дерево чистое. Сбой коммита не роняет прогон: исполнитель мог
/// закоммитить частично, диагностику видно по `git status`.
fn auto_commit_leftovers(repo: &Path, harness: &str, task: &str) -> Option<AutoCommit> {
    // Не git-репозиторий — нечего фиксировать.
    git_out(repo, &["rev-parse", "--git-dir"])?;
    // Добавляем всё, кроме служебного пакета и типичного мусора интерпретеров.
    git_out(
        repo,
        &[
            "add",
            "-A",
            "--",
            ".",
            ":!.arch-handoff",
            ":(exclude,glob)**/__pycache__/**",
            ":(exclude,glob)**/*.pyc",
            ":(exclude,glob)**/.pytest_cache/**",
        ],
    )?;
    let staged = git_out(repo, &["diff", "--cached", "--name-only"])?;
    let files = staged.lines().filter(|l| !l.trim().is_empty()).count();
    if files == 0 {
        return None;
    }
    let first_line = task.lines().next().unwrap_or("задача").trim();
    let mut title: String = first_line.chars().take(60).collect();
    if first_line.chars().count() > 60 {
        title.push('…');
    }
    let message = format!("harness({harness}): {title}");
    // Явная идентичность: в свежих worktree/контейнерах user.name/user.email
    // часто не настроены, и без этого коммит падает.
    git_out(
        repo,
        &[
            "-c",
            "user.name=spine-harness",
            "-c",
            "user.email=spine-harness@localhost",
            "commit",
            "-q",
            "-m",
            &message,
        ],
    )?;
    let hash = git_out(repo, &["rev-parse", "--short", "HEAD"])?
        .trim()
        .to_string();
    Some(AutoCommit {
        files,
        hash,
        message,
    })
}

/// Есть ли в репозитории файлы, изменённые после `since` (heartbeat активности
/// молчащего харнесса). Служебные/тяжёлые каталоги пропускаются; лимит —
/// 8000 записей, глубина 8 (дорогое сканирование не нужно: свежие файлы
/// почти всегда наверху).
fn repo_changed_since(repo: &Path, since: std::time::SystemTime) -> bool {
    const SKIP: [&str; 6] = [
        ".git",
        "target",
        "node_modules",
        "dist",
        "__pycache__",
        ".next",
    ];
    let mut seen = 0usize;
    for entry in walkdir::WalkDir::new(repo)
        .max_depth(8)
        .into_iter()
        .filter_entry(|e| {
            e.depth() == 0 || !SKIP.contains(&e.file_name().to_string_lossy().as_ref())
        })
        .filter_map(std::result::Result::ok)
    {
        seen += 1;
        if seen > 8000 {
            break;
        }
        if !entry.file_type().is_file() {
            continue;
        }
        let fresh = entry
            .metadata()
            .ok()
            .and_then(|m| m.modified().ok())
            .is_some_and(|mt| mt > since);
        if fresh {
            return true;
        }
    }
    false
}

/// Лимит вывода `harness_run` (stdout + stderr), символов.
const HARNESS_RUN_MAX_CHARS: usize = 24 * 1024;

/// Статус из контракта результата (схема TASK.md).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContractStatus {
    /// Выполнено полностью.
    Complete,
    /// Частично.
    Partial,
    /// Заблокировано (интеграция невозможна до разбора).
    Blocked,
}

impl ContractStatus {
    fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "complete" => Some(Self::Complete),
            "partial" => Some(Self::Partial),
            "blocked" => Some(Self::Blocked),
            _ => None,
        }
    }

    /// Строковое представление как в контракте.
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Complete => "complete",
            Self::Partial => "partial",
            Self::Blocked => "blocked",
        }
    }
}

/// Механически разобранный и проверенный по схеме контракт результата.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResultContract {
    /// Статус прогона.
    pub status: ContractStatus,
    /// Допущения исполнителя.
    pub assumptions: Vec<String>,
    /// Открытые вопросы к архитектору.
    pub open_questions: Vec<String>,
    /// Расхождения с принятыми решениями (ADR/spine) — останавливают интеграцию.
    pub conflicts: Vec<String>,
}

/// Исход механического разбора контракта результата.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContractParse {
    /// Контракт найден и валиден по схеме.
    Valid(ResultContract),
    /// Блок со `status` найден, но схема нарушена (причина).
    Invalid(String),
    /// Контракта в выводе нет.
    Missing,
}

/// Проверяет кандидата по схеме контракта: `status` строго из
/// complete|partial|blocked; списки опциональны (дефолт — пустые), но если
/// присутствуют — обязаны быть массивами (элементы приводятся к строкам).
fn validate_contract(v: &Value) -> std::result::Result<ResultContract, String> {
    let status_raw = v
        .get("status")
        .and_then(Value::as_str)
        .ok_or("поле `status` отсутствует или не строка")?;
    let status = ContractStatus::parse(status_raw)
        .ok_or_else(|| format!("status='{status_raw}' вне complete|partial|blocked"))?;
    let list = |key: &str| -> std::result::Result<Vec<String>, String> {
        match v.get(key) {
            None => Ok(Vec::new()),
            Some(Value::Array(items)) => Ok(items
                .iter()
                .map(|i| match i {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                })
                .collect()),
            Some(_) => Err(format!("поле `{key}` не массив")),
        }
    };
    Ok(ResultContract {
        status,
        assumptions: list("assumptions")?,
        open_questions: list("open_questions")?,
        conflicts: list("conflicts_with_prior_decisions")?,
    })
}

/// Механический разбор контракта результата из stdout харнесса
/// (замена текстовой эвристике «последний `` ```json `` со `status`»):
///
/// 1. fenced `` ```json ``-блоки с конца вывода (контракт обязан идти последним);
///    блок со `status`, не парсящийся как JSON, — это Invalid, а не промах;
/// 2. запасной путь: голый JSON-объект в хвосте вывода (модели иногда роняют
///    fence) — перебор `{`-позиций последних 4 КБ с конца.
///
/// Найденный кандидат валидируется по схеме [`validate_contract`].
#[must_use]
pub fn parse_result_contract(stdout: &str) -> ContractParse {
    let mut invalid: Option<String> = None;
    let mut blocks = Vec::new();
    let mut rest = stdout;
    while let Some(start) = rest.find("```json") {
        let after = &rest[start + "```json".len()..];
        match after.find("```") {
            Some(end) => {
                blocks.push(after[..end].trim());
                rest = &after[end + 3..];
            }
            None => break,
        }
    }
    for block in blocks.into_iter().rev() {
        match serde_json::from_str::<Value>(block) {
            Ok(v) if v.get("status").is_some() => {
                return match validate_contract(&v) {
                    Ok(c) => ContractParse::Valid(c),
                    Err(e) => ContractParse::Invalid(e),
                };
            }
            Err(e) if block.contains("\"status\"") => {
                invalid = Some(format!("невалидный JSON в ```json-блоке со status: {e}"));
            }
            Ok(_) | Err(_) => {}
        }
    }
    // Голый JSON в хвосте (fence уронен): перебираем `{` с конца хвоста.
    // `floor_char_boundary` стабилизирован в 1.91 — выше MSRV 1.85: идём к
    // ближайшей границе символа вручную (эквивалент по семантике).
    let mut tail_at = stdout.len().saturating_sub(4096);
    while !stdout.is_char_boundary(tail_at) {
        tail_at -= 1;
    }
    let tail = &stdout[tail_at..];
    let braces: Vec<usize> = tail.match_indices('{').map(|(i, _)| i).collect();
    for i in braces.into_iter().rev().take(8) {
        let cand = tail[i..].trim();
        if !cand.contains("\"status\"") {
            continue;
        }
        if let Ok(v) = serde_json::from_str::<Value>(cand) {
            if v.get("status").is_some() {
                return match validate_contract(&v) {
                    Ok(c) => ContractParse::Valid(c),
                    Err(e) => ContractParse::Invalid(e),
                };
            }
        }
    }
    match invalid {
        Some(e) => ContractParse::Invalid(e),
        None => ContractParse::Missing,
    }
}

/// Инструмент `harness_run`: прогон handoff-пакета (или явной задачи)
/// кодовым харнессом — без импровизации через bash (квотинг, permission-
/// промпты, короткие таймауты bash — частые точки отказа такой импровизации).
struct HarnessRunTool {
    /// Конфигурация (адаптеры харнессов).
    cfg: Config,
}

impl HarnessRunTool {
    /// Живой конфиг: файл перечитывается при каждом вызове — правки
    /// `[harnesses.*]` в config.toml подхватываются без перезапуска сессии
    /// (инцидент: агент исправил адаптеры в файле, а прогон шёл со снапшота
    /// конфига, загруженного при старте процесса). Перечитывается только
    /// файл, из которого конфиг был загружен (`loaded_from`): снапшот без
    /// файла (тесты, чистые дефолты) окружение не подхватывает. При сбое
    /// чтения — снапшот процесса.
    fn live_config(&self) -> Config {
        match self.cfg.loaded_from.as_deref() {
            Some(path) => Config::load(Some(path)).unwrap_or_else(|_| self.cfg.clone()),
            None => self.cfg.clone(),
        }
    }

    /// Фоновый прогон: задача регистрируется в общем реестре фоновых задач
    /// (префикс `hr-`, видна в `subagent_list`), исполнение — в отдельной
    /// tokio-задаче; инструмент возвращается немедленно. Полный лог пишется
    /// файлом (`reports_dir/harness/<id>.log`), в реестр — усечённый отчёт
    /// (лимит [`crate::subagent::REPORT_MAX_CHARS`], как у субагентов).
    /// Прерывание хода (Esc/Alt+Enter) фоновый прогон НЕ затрагивает —
    /// задача не привязана к токену отмены сессии.
    fn launch_background(
        &self,
        name: &str,
        hcfg: CodingHarnessConfig,
        repo: PathBuf,
        task: String,
        note: &str,
        ctx: &ToolContext,
    ) -> ToolOutput {
        let Some(registry) = &ctx.subagents else {
            return ToolOutput::err(
                "harness_run background: реестр фоновых задач не подключён \
                 (headless-режим без TUI/раннера) — запустите без background",
            );
        };
        if registry.running() >= registry.capacity() {
            return ToolOutput::err(format!(
                "все слоты фоновых задач заняты ({}); дождитесь завершения — subagent_list",
                registry.capacity()
            ));
        }
        let id = registry.next_id("hr");
        registry.insert(crate::subagent::SubagentTask {
            id: id.clone(),
            agent: format!("harness:{name}"),
            task: task.chars().take(2000).collect(),
            status: crate::subagent::TaskStatus::Running,
            report: String::new(),
            started_at: crate::subagent::now_iso(),
            finished_at: None,
        });
        let report_path = ctx
            .config
            .paths
            .reports_dir
            .join("harness")
            .join(format!("{id}.log"));
        let registry = registry.clone();
        let run_id = id.clone();
        let run_name = name.to_string();
        let note_run = note.to_string();
        let report_path_run = report_path.clone();
        tokio::spawn(async move {
            let out = execute_run(&run_name, &hcfg, &repo, &task, note_run).await;
            let status = if out.is_error {
                crate::subagent::TaskStatus::Failed
            } else {
                crate::subagent::TaskStatus::Done
            };
            // Полный лог — файлом (best effort: отчёт живёт и в реестре).
            if let Some(dir) = report_path_run.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            if let Err(e) = std::fs::write(&report_path_run, &out.content) {
                tracing::warn!(
                    "лог фонового прогона не записан {}: {e}",
                    report_path_run.display()
                );
            }
            let report: String = out
                .content
                .chars()
                .take(crate::subagent::REPORT_MAX_CHARS)
                .collect();
            registry.finish(&run_id, status, report);
        });
        ToolOutput::ok(format!(
            "{note}Прогон харнесса '{name}' запущен в ФОНЕ: {id}. Этот ход агента НЕ ждёт \
             завершения — агент остаётся доступен пользователю. Статус — subagent_list, \
             результат — subagent_result(id=\"{id}\"), полный лог — {}. \
             Сообщи пользователю, что прогон идёт в фоне, и продолжай диалог.",
            report_path.display()
        ))
    }
}

/// Enforcement `[fleet] require_worktree`: прогон кодового харнесса не имеет
/// прямого пути в основное дерево репозитория — рабочим каталогом прогона
/// становится изолированный git worktree (ветка `arch/<slug>-<timestamp>`,
/// фабрика [`crate::worktree`]). Handoff-пакет `.arch-handoff/` по контракту
/// в git не коммитится, поэтому копируется в worktree как есть. Интеграция
/// результата — только через гейт владельца (`arch fleet merge <run-id>`).
///
/// Возвращает `None`, если enforcement выключен (`require_worktree = false`,
/// дефолт — поведение прежних версий), иначе `Some((каталог прогона, run-id))`.
///
/// # Errors
/// `require_worktree = true`, а каталог — не git-репозиторий (понятная ошибка
/// ДО запуска харнесса); сбой создания worktree.
pub async fn enforce_run_worktree(
    cfg: &Config,
    repo: &Path,
    slug: &str,
) -> Result<Option<(PathBuf, String)>> {
    if !cfg.fleet.require_worktree {
        return Ok(None);
    }
    // Kebab-слаг из имени харнесса (валидатор worktree: [a-z0-9-], ≤ 48
    // символов); timestamp «-yyyymmddhhmmss» занимает 15 — слаг до 32.
    let slug: String = slug
        .to_ascii_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_lowercase() || c.is_ascii_digit() {
                c
            } else {
                '-'
            }
        })
        .take(32)
        .collect();
    let run_id = format!(
        "{}-{}",
        slug.trim_matches('-'),
        Utc::now().format("%Y%m%d%H%M%S")
    );
    let dir = crate::worktree::create(cfg, repo, &run_id, None).await?;
    // `.arch-handoff/` не входит в коммиты (контракт TASK.md) — в свежем
    // worktree его нет; копируем, иначе исполнитель потеряет пакет задачи.
    let handoff = repo.join(HANDOFF_DIR);
    if handoff.is_dir() && !dir.join(HANDOFF_DIR).exists() {
        copy_dir(&handoff, &dir.join(HANDOFF_DIR))?;
    }
    Ok(Some((dir, run_id)))
}

/// Рекурсивное копирование каталога (handoff-пакет в worktree прогона).
fn copy_dir(src: &Path, dst: &Path) -> Result<()> {
    std::fs::create_dir_all(dst).map_err(|e| HarnessError::io(dst, e))?;
    for entry in std::fs::read_dir(src).map_err(|e| HarnessError::io(src, e))? {
        let entry = entry.map_err(|e| HarnessError::io(src, e))?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if from.is_dir() {
            copy_dir(&from, &to)?;
        } else {
            std::fs::copy(&from, &to).map_err(|e| HarnessError::io(&to, e))?;
        }
    }
    Ok(())
}

#[async_trait]
impl Tool for HarnessRunTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "harness_run".into(),
            description: "Запустить кодовый харнесс на репозитории и вернуть его вывод. \
                Обычно следует за handoff_create: задача читается из \
                <repo>/.arch-handoff/TASK.md (или передаётся явно). Запуск идёт через \
                настроенный адаптер [harnesses.<имя>] (режим prompt, env); config.toml \
                перечитывается на каждый вызов — правки адаптеров применяются без \
                перезапуска сессии. Умные таймауты: \
                абсолютный потолок 30 мин (по умолчанию) + таймаут тишины 10 мин — прогон \
                прерывается, только если харнесс не выводит и не меняет файлы репозитория; \
                при прерывании убивается вся процессная группа (сирот не остаётся) и \
                возвращается частичный вывод. НЕ занижайте timeout_secs: значения ниже \
                600 поднимаются до 600 — кодовый харнесс за меньшее время почти никогда \
                не успевает. stdout/stderr и код возврата захватываются, JSON-контракт \
                результата (status/assumptions/open_questions) разбирается механически \
                (валидация схемы, эскалация blocked/conflicts). \
                НЕ запускать харнесс через bash — там промпт ломается о квотинг, таймаут \
                слишком короткий, а env-scrub прячет от команды переменные *_KEY/*_TOKEN, \
                через которые харнесс может авторизовываться. \
                background=true — прогон в фоне: инструмент возвращается сразу (id задачи \
                hr-*), агент остаётся доступным пользователю; статус — subagent_list, \
                результат — subagent_result(id). Используй background для длинных прогонов \
                и когда пользователь продолжает диалог во время работы харнесса."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "harness": {
                        "type": "string",
                        "description": "Имя харнесса: claude-code, qwen-code, openclaw, hermes, theseus, codewhale, kimi-code"
                    },
                    "path": {
                        "type": "string",
                        "description": "Корень репозитория (относительно cwd или абсолютный)"
                    },
                    "task": {
                        "type": "string",
                        "description": "Явная задача; если не задана — читается <repo>/.arch-handoff/TASK.md"
                    },
                    "timeout_secs": {
                        "type": "integer",
                        "description": "Переопределить АБСОЛЮТНЫЙ таймаут адаптера, секунды (минимум 600 — меньшие значения поднимаются; максимум 7200). Тишина контролируется отдельно (idle_timeout_secs адаптера, по умолчанию 600)",
                        "minimum": 600,
                        "maximum": 7200
                    },
                    "background": {
                        "type": "boolean",
                        "description": "true — прогон в фоне (немедленный возврат, задача hr-* в реестре фоновых задач; результат забирается через subagent_result). false/отсутствует — синхронно: ход агента ждёт завершения прогона"
                    }
                },
                "required": ["harness", "repo"]
            }),
        }
    }

    /// Прогон кодового харнесса может идти до 7200 с (потолок аргумента
    /// `timeout_secs`) плюс запас на групповое завершение и сбор вывода;
    /// берём максимум из адаптеров конфига — иначе агентный цикл обрывает
    /// длинный прогон раньше собственного таймаута адаптера (инцидент 11-24).
    fn timeout_secs(&self) -> u64 {
        let live = self.live_config();
        let adapter_max = live
            .harnesses
            .values()
            .map(|h| h.timeout_secs)
            .max()
            .unwrap_or(0);
        adapter_max.max(7200) + 120
    }

    async fn call(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let Some(name) = args.get("harness").and_then(Value::as_str) else {
            return Ok(ToolOutput::err(
                "harness_run: обязательный аргумент 'harness' (string) отсутствует",
            ));
        };
        let Some(repo) = args.get("repo").and_then(Value::as_str) else {
            return Ok(ToolOutput::err(
                "harness_run: обязательный аргумент 'repo' (string) отсутствует",
            ));
        };
        // Адаптеры читаем из ЖИВОГО конфига: правки config.toml в ходе
        // сессии применяются немедленно, без перезапуска агента.
        let live = self.live_config();
        let Some(hcfg) = live.harnesses.get(name) else {
            return Ok(ToolOutput::err(format!(
                "harness_run: харнесс '{name}' не настроен. Известные: {}; \
                 адаптеры — в config.toml [harnesses.<имя>]",
                known().join(", ")
            )));
        };
        let repo = ctx.resolve(repo);
        // [fleet] require_worktree: путь в main для прогона закрыт платформенно —
        // работа идёт в изолированном git worktree, интеграция результата —
        // только гейтом владельца (`arch fleet merge <run-id>`).
        let (repo, gate_note) = match enforce_run_worktree(&live, &repo, name).await {
            Ok(Some((dir, run_id))) => (
                dir,
                format!(
                    "Изоляция прогона ([fleet] require_worktree): run-id '{run_id}', рабочий \
                     каталог — git worktree ветки arch/{run_id}; основное дерево НЕ изменяется. \
                     Интеграция — только владельцем: `arch fleet merge {run_id} --owner-approve` \
                     (влить) или `arch worktree drop {run_id}` (отклонить). Сообщи это \
                     пользователю в финальном ответе.\n"
                ),
            ),
            Ok(None) => (repo, String::new()),
            Err(e) => return Ok(ToolOutput::err(format!("harness_run: {e}"))),
        };
        let task = if let Some(t) = args.get("task").and_then(Value::as_str) {
            t.to_string()
        } else {
            let path = repo.join(HANDOFF_DIR).join("TASK.md");
            match std::fs::read_to_string(&path) {
                Ok(t) => t,
                Err(e) => {
                    return Ok(ToolOutput::err(format!(
                        "harness_run: нет аргумента 'task' и не читается {}: {e}. \
                         Сначала handoff_create или передайте task явно",
                        path.display()
                    )));
                }
            }
        };
        // Переопределение таймаута — копией конфига адаптера. Минимум 600 с:
        // модели склонны занижать таймаут («успеет за 5 минут»), а кодовый
        // харнесс на реальной задаче работает дольше — ранний таймаут
        // обрывал прогон и оставлял репозиторий в полусобранном состоянии.
        let mut hcfg = hcfg.clone();
        let mut note = gate_note;
        if let Some(t) = args.get("timeout_secs").and_then(Value::as_u64) {
            if t < MIN_HARNESS_TIMEOUT_SECS {
                let _ = writeln!(
                    note,
                    "timeout_secs={t} поднят до {MIN_HARNESS_TIMEOUT_SECS} (минимум для кодового харнесса)."
                );
            }
            hcfg.timeout_secs = t.clamp(MIN_HARNESS_TIMEOUT_SECS, 7200);
        } else if let Some(t) = recommended_timeout_secs(&repo) {
            // Таймаут не задан явно — берём рекомендацию пакета (маршрут
            // значимости из handoff_create): Critical-эпик в дефолтные
            // 30 минут адаптера не влезает.
            hcfg.timeout_secs = t.clamp(MIN_HARNESS_TIMEOUT_SECS, 7200);
            let _ = writeln!(
                note,
                "timeout_secs={} — рекомендация пакета (MANIFEST.json).",
                hcfg.timeout_secs
            );
        }
        // Фоновый прогон: инструмент возвращается немедленно, агент остаётся
        // доступным пользователю (длинные handoff-прогоны не блокируют диалог).
        if args.get("background").and_then(Value::as_bool) == Some(true) {
            return Ok(self.launch_background(name, hcfg, repo, task, &note, ctx));
        }
        Ok(execute_run(name, &hcfg, &repo, &task, note).await)
    }
}

/// Синхронный прогон харнесса и форматирование результата: итог (код
/// возврата/прерывание/авто-коммит), механический разбор JSON-контракта,
/// stdout/stderr. Общий код синхронного пути (`harness_run`) и фонового
/// (`background=true` — вызывается из spawned-задачи).
async fn execute_run(
    name: &str,
    hcfg: &CodingHarnessConfig,
    repo: &Path,
    task: &str,
    note: String,
) -> ToolOutput {
    match run_harness(name, hcfg, repo, task).await {
        Ok(run) => {
            let code = run.exit_code.map_or("сигнал".into(), |c| c.to_string());
            let mut content = note;
            match run.termination {
                Termination::Completed => {
                    let _ = writeln!(
                        content,
                        "Харнесс '{name}' завершился: код {code}, {:.1} с.",
                        run.duration_secs
                    );
                }
                Termination::AbsoluteTimeout => {
                    let _ = writeln!(
                        content,
                        "Харнесс '{name}' ПРЕРВАН по абсолютному таймауту {} с \
                             (проработал {:.1} с). Процессная группа завершена \
                             (TERM→KILL), осиротевших процессов нет. Вывод ниже — \
                             частичный. Репозиторий может быть в промежуточном \
                             состоянии: перед повторным запуском проверьте git status/diff. \
                             Если задача объективно длинная — перезапустите с большим \
                             timeout_secs (до 7200) или разбейте её.",
                        hcfg.timeout_secs, run.duration_secs
                    );
                }
                Termination::IdleTimeout => {
                    let _ = writeln!(
                        content,
                        "Харнесс '{name}' ПРЕРВАН по таймауту тишины {} с: нет вывода и \
                             изменений файлов репозитория — процесс, вероятно, завис \
                             (например, ждал интерактивного ввода; для claude-code обязателен \
                             --dangerously-skip-permissions). Процессная группа завершена \
                             (TERM→KILL), сирот нет. Вывод ниже — частичный; перед \
                             повторным запуском проверьте git status/diff.",
                        hcfg.idle_timeout_secs
                    );
                }
            }
            if let Some(ac) = &run.auto_commit {
                let _ = writeln!(
                    content,
                    "АВТО-КОММИТ: исполнитель не зафиксировал результат — \
                         харнесс закоммитил {} путей: {} «{}». \
                         Контракт TASK.md требует финального коммита от самого \
                         исполнителя; при повторении проверьте задачу/доступ к git.",
                    ac.files, ac.hash, ac.message
                );
            }
            match &run.contract {
                ContractParse::Valid(c) => {
                    let _ = writeln!(
                        content,
                        "Контракт результата: status={}; assumptions: {}; \
                             open_questions: {}; conflicts: {}.",
                        c.status.as_str(),
                        c.assumptions.len(),
                        c.open_questions.len(),
                        c.conflicts.len(),
                    );
                    if c.status == ContractStatus::Blocked {
                        content.push_str(
                            "СТАТУС blocked: интеграция невозможна — сначала разберите \
                                 причины (open_questions/assumptions ниже) с архитектором.\n",
                        );
                    }
                    if !c.conflicts.is_empty() {
                        content.push_str(
                                "КОНФЛИКТЫ со spine/ADR (ОСТАНАВЛИВАЮТ интеграцию до решения архитектора):\n",
                            );
                        for conflict in &c.conflicts {
                            let _ = writeln!(content, "- {conflict}");
                        }
                    }
                    if !c.open_questions.is_empty() {
                        content.push_str("Открытые вопросы к архитектору:\n");
                        for q in &c.open_questions {
                            let _ = writeln!(content, "- {q}");
                        }
                    }
                }
                ContractParse::Invalid(reason) => {
                    let _ = writeln!(
                        content,
                        "ВНИМАНИЕ: JSON-контракт найден, но НЕВАЛИДЕН по схеме: {reason}. \
                             Машинная приёмка невозможна — перезапустите с напоминанием \
                             о схеме контракта (status из complete|partial|blocked, списки — массивы)."
                    );
                }
                ContractParse::Missing => {
                    content.push_str(
                        "ВНИМАНИЕ: JSON-контракт результата (```json с полем status) \
                             в stdout не найден — ответ может быть неполным; при необходимости \
                             перезапустите с напоминанием о контракте.\n",
                    );
                }
            }
            content.push_str("--- stdout ---\n");
            content.push_str(run.stdout.trim_end());
            if !run.stderr.trim().is_empty() {
                content.push_str("\n--- stderr ---\n");
                content.push_str(run.stderr.trim_end());
            }
            let is_error = run.exit_code != Some(0) || run.termination != Termination::Completed;
            ToolOutput {
                content,
                is_error,
                images: Vec::new(),
                data: None,
            }
            .truncated(HARNESS_RUN_MAX_CHARS)
        }
        Err(e) => ToolOutput::err(format!("harness_run: {e}")),
    }
}

/// Инструменты домена: `harness_run` (`handoff_create` живёт в
/// [`crate::handoff`] и регистрируется отдельно — в обеих сборках).
#[must_use]
pub fn tools(cfg: &Config) -> Vec<Arc<dyn Tool>> {
    vec![Arc::new(HarnessRunTool { cfg: cfg.clone() })]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Конфиг с assets внутри временного каталога (изоляция от ~/.arch-harness).
    fn cfg_in(dir: &Path) -> Config {
        let mut cfg = Config::default();
        cfg.paths.assets_dir = dir.join("assets");
        cfg
    }

    fn write_file(path: &Path, text: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("mkdir");
        }
        std::fs::write(path, text).expect("write");
    }

    #[test]
    fn builds_argv_per_prompt_mode() {
        let cfg = |args: &[&str], mode: PromptMode| CodingHarnessConfig {
            binary: "bin".into(),
            args: args.iter().map(|s| (*s).into()).collect(),
            prompt_mode: mode,
            ..CodingHarnessConfig::default()
        };

        // Positional: задача — позиционный аргумент в конце.
        let (argv, stdin) = build_argv(&cfg(&["-p"], PromptMode::Positional), "TASK");
        assert_eq!(argv, ["-p", "TASK"]);
        assert!(stdin.is_none());

        // Flag с плейсхолдером: подстановка на место {prompt}.
        let (argv, stdin) = build_argv(
            &cfg(&["agent", "--message", "{prompt}"], PromptMode::Flag),
            "TASK",
        );
        assert_eq!(argv, ["agent", "--message", "TASK"]);
        assert!(stdin.is_none());

        // Flag без плейсхолдера: задача добавляется в конец.
        let (argv, stdin) = build_argv(&cfg(&["run", "--task"], PromptMode::Flag), "TASK");
        assert_eq!(argv, ["run", "--task", "TASK"]);
        assert!(stdin.is_none());

        // Stdin: argv без задачи, задача — в stdin.
        let (argv, stdin) = build_argv(&cfg(&["-p"], PromptMode::Stdin), "TASK");
        assert_eq!(argv, ["-p"]);
        assert_eq!(stdin.as_deref(), Some("TASK"));
    }

    #[tokio::test]
    async fn runs_stdin_harness_and_captures_output() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let cfg = CodingHarnessConfig {
            binary: "cat".into(),
            prompt_mode: PromptMode::Stdin,
            timeout_secs: 30,
            ..CodingHarnessConfig::default()
        };
        let run = run_harness("test-cat", &cfg, tmp.path(), "привет, харнесс")
            .await
            .expect("run");
        assert_eq!(run.harness, "test-cat");
        assert_eq!(run.exit_code, Some(0));
        assert_eq!(run.stdout, "привет, харнесс");
        assert!(run.stderr.is_empty());
        assert!(run.duration_secs >= 0.0);
        assert_eq!(run.termination, Termination::Completed);
    }

    #[tokio::test]
    async fn missing_binary_returns_hint() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let cfg = CodingHarnessConfig {
            binary: "definitely-missing-bin".into(),
            timeout_secs: 5,
            ..CodingHarnessConfig::default()
        };
        let err = run_harness("theseus", &cfg, tmp.path(), "задача")
            .await
            .expect_err("должна быть ошибка");
        let msg = err.to_string();
        assert!(msg.contains("definitely-missing-bin"), "{msg}");
        assert!(msg.contains("установите"), "{msg}");
        assert!(msg.contains("[harnesses.theseus]"), "{msg}");
    }

    #[tokio::test]
    async fn absolute_timeout_returns_partial_output() {
        // Регрессия 09-12: раньше таймаут возвращал Err без вывода — модель
        // не видела, что харнесс успел сделать.
        let tmp = tempfile::tempdir().expect("tempdir");
        let cfg = CodingHarnessConfig {
            binary: "sh".into(),
            args: vec!["-c".into(), "echo MARKER; sleep 60".into()],
            prompt_mode: PromptMode::Stdin,
            timeout_secs: 1,
            idle_timeout_secs: 0,
            ..CodingHarnessConfig::default()
        };
        let run = run_harness("slow", &cfg, tmp.path(), "задача")
            .await
            .expect("прерывание — Ok с частичным выводом");
        assert_eq!(run.termination, Termination::AbsoluteTimeout);
        assert!(run.stdout.contains("MARKER"), "stdout: {}", run.stdout);
        assert!(run.duration_secs < 30.0, "{:?}", run.duration_secs);
    }

    #[tokio::test]
    async fn idle_timeout_fires_on_silence() {
        // Молчащий и непишущий процесс убивается по idle, не дожидаясь
        // абсолютного потолка.
        let tmp = tempfile::tempdir().expect("tempdir");
        let cfg = CodingHarnessConfig {
            binary: "sleep".into(),
            args: vec!["60".into()],
            prompt_mode: PromptMode::Stdin,
            timeout_secs: 120,
            idle_timeout_secs: 2,
            ..CodingHarnessConfig::default()
        };
        let run = run_harness("silent", &cfg, tmp.path(), "задача")
            .await
            .expect("run");
        assert_eq!(run.termination, Termination::IdleTimeout);
        assert!(run.duration_secs < 30.0, "{:?}", run.duration_secs);
    }

    #[tokio::test]
    async fn file_activity_resets_idle() {
        // Молчащий, но пишущий файлы процесс (типичный кодовый харнесс)
        // НЕ считается зависшим: heartbeat по mtime репозитория.
        let tmp = tempfile::tempdir().expect("tempdir");
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).expect("mkdir repo");
        let cfg = CodingHarnessConfig {
            binary: "sh".into(),
            args: vec![
                "-c".into(),
                "i=0; while [ $i -lt 10 ]; do touch \"f$i\"; i=$((i+1)); sleep 1; done; echo done"
                    .into(),
            ],
            prompt_mode: PromptMode::Stdin,
            timeout_secs: 60,
            // Запас против нагрузки параллельного тест-сьюта: gap между
            // touch ~1 с при окне 8 с — флаки не будет.
            idle_timeout_secs: 8,
            ..CodingHarnessConfig::default()
        };
        let run = run_harness("writer", &cfg, &repo, "задача")
            .await
            .expect("run");
        assert_eq!(
            run.termination,
            Termination::Completed,
            "stderr: {}",
            run.stderr
        );
        assert!(run.stdout.contains("done"), "stdout: {}", run.stdout);
        assert!(repo.join("f9").is_file());
    }

    #[tokio::test]
    async fn timeout_kills_whole_process_group() {
        // Регрессия 09-12 (скриншот пользователя): таймаут убивал обёртку,
        // а дочерний процесс харнесса оставался жить сиротой. Теперь группа
        // завершается целиком (TERM → KILL по -pgid).
        let tmp = tempfile::tempdir().expect("tempdir");
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).expect("mkdir repo");
        let cfg = CodingHarnessConfig {
            binary: "sh".into(),
            args: vec![
                "-c".into(),
                "sleep 300 & echo $! > child.pid; sleep 300".into(),
            ],
            prompt_mode: PromptMode::Stdin,
            timeout_secs: 1,
            idle_timeout_secs: 0,
            ..CodingHarnessConfig::default()
        };
        let run = run_harness("spawner", &cfg, &repo, "задача")
            .await
            .expect("run");
        assert_eq!(run.termination, Termination::AbsoluteTimeout);
        let pid = std::fs::read_to_string(repo.join("child.pid")).expect("child.pid");
        let pid = pid.trim();
        // Процесс «убит» = /proc нет ИЛИ зомби (Z/X): зомби уже мёртв, его
        // просто ещё не забрал родитель. kill -0 на зомби возвращает success,
        // поэтому проверяем state, а не сам факт ответа.
        let alive = std::fs::read_to_string(format!("/proc/{pid}/stat"))
            .ok()
            .and_then(|s| {
                s.rsplit(')')
                    .next()?
                    .split_whitespace()
                    .next()
                    .map(str::to_owned)
            })
            .is_some_and(|state| !matches!(state.as_str(), "Z" | "X" | "x"));
        assert!(!alive, "дочерний процесс {pid} пережил таймаут — сирота");
    }

    /// git-репозиторий с одним baseline-коммитом (явная идентичность —
    /// на CI/в контейнерах user.name/user.email может не быть).
    fn git_repo_with_baseline(dir: &std::path::Path) {
        std::fs::create_dir_all(dir).expect("mkdir repo");
        std::fs::write(dir.join("README.md"), "# baseline\n").expect("readme");
        let git = |args: &[&str]| {
            let out = std::process::Command::new("git")
                .arg("-C")
                .arg(dir)
                .args(args)
                .output()
                .expect("git");
            assert!(out.status.success(), "git {args:?}: {:?}", out.stderr);
        };
        git(&["init", "-q"]);
        git(&["add", "README.md"]);
        git(&[
            "-c",
            "user.name=test",
            "-c",
            "user.email=test@test",
            "commit",
            "-q",
            "-m",
            "baseline",
        ]);
    }

    /// Харнесс-заглушка: пишет код + интерпретерный мусор, НЕ коммитит
    /// (воспроизводит дефект «агенты завершились без финального коммита»).
    fn dirty_executor_cfg() -> CodingHarnessConfig {
        CodingHarnessConfig {
            binary: "sh".into(),
            args: vec![
                "-c".into(),
                "mkdir -p spinecalc __pycache__ .arch-handoff; \
                 echo 'def validate_amount(v, l): return True' > spinecalc/amount.py; \
                 echo junk > __pycache__/x.pyc; \
                 echo meta > .arch-handoff/TASK.md; \
                 echo '{\"status\": \"complete\"}'"
                    .into(),
            ],
            prompt_mode: PromptMode::Stdin,
            timeout_secs: 30,
            idle_timeout_secs: 0,
            ..CodingHarnessConfig::default()
        }
    }

    #[tokio::test]
    async fn auto_commit_commits_executor_leftovers() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let repo = tmp.path().join("repo");
        git_repo_with_baseline(&repo);
        let run = run_harness(
            "dirty",
            &dirty_executor_cfg(),
            &repo,
            "реализовать модуль amount",
        )
        .await
        .expect("run");
        assert_eq!(run.termination, Termination::Completed);
        let ac = run.auto_commit.expect("харнесс обязан до-коммитить хвост");
        assert_eq!(ac.files, 1, "только код, без мусора: {ac:?}");
        assert!(
            ac.message
                .starts_with("harness(dirty): реализовать модуль amount")
        );
        assert!(!ac.hash.is_empty());
        // В истории — baseline + авто-коммит с кодом; физически в дереве
        // остаются лишь некоммитимые служебные/мусорные каталоги.
        let status = git_out(&repo, &["status", "--porcelain"]).expect("status");
        for line in status.lines() {
            assert!(
                line.contains(".arch-handoff/") || line.contains("__pycache__/"),
                "посторонний незакоммиченный путь: {line}"
            );
        }
        let log = git_out(&repo, &["log", "--oneline"]).expect("log");
        assert_eq!(log.lines().count(), 2, "{log}");
        let committed =
            git_out(&repo, &["show", "--name-only", "--pretty=%s", "HEAD"]).expect("show");
        assert!(committed.contains("spinecalc/amount.py"), "{committed}");
        assert!(!committed.contains("__pycache__"), "{committed}");
        assert!(!committed.contains(".arch-handoff"), "{committed}");
    }

    #[tokio::test]
    async fn auto_commit_disabled_leaves_tree_dirty() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let repo = tmp.path().join("repo");
        git_repo_with_baseline(&repo);
        let cfg = CodingHarnessConfig {
            auto_commit: false,
            ..dirty_executor_cfg()
        };
        let run = run_harness("dirty-off", &cfg, &repo, "задача")
            .await
            .expect("run");
        assert_eq!(run.termination, Termination::Completed);
        assert!(run.auto_commit.is_none());
        let status = git_out(&repo, &["status", "--porcelain"]).expect("status");
        assert!(status.contains("spinecalc/"), "{status}");
    }

    #[tokio::test]
    async fn env_allow_whitelist_isolates_harness_env() {
        // Разрыв P1 «окружение протекает между харнессами»: при непустом
        // env_allow процесс стартует с чистым окружением + whitelist + env
        // адаптера; пустой список — наследование окружения (как раньше).
        let tmp = tempfile::tempdir().expect("tempdir");
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).expect("mkdir");
        let probe = |env_allow: Vec<&str>| CodingHarnessConfig {
            binary: "/bin/sh".into(),
            args: vec![
                "-c".into(),
                "echo \"H=${HOME:-EMPTY} P=${PATH:+set} E=${EXTRA:-EMPTY}\"".into(),
            ],
            prompt_mode: PromptMode::Stdin,
            timeout_secs: 30,
            idle_timeout_secs: 0,
            auto_commit: false,
            env_allow: env_allow.iter().map(|s| (*s).into()).collect(),
            env: [("EXTRA".to_string(), "yes".to_string())]
                .into_iter()
                .collect(),
        };
        // Наследование по умолчанию: HOME и PATH видны, EXTRA из env — тоже.
        let run = run_harness("probe", &probe(vec![]), &repo, "задача")
            .await
            .expect("run");
        assert!(run.stdout.contains("H=/"), "наследование: {}", run.stdout);
        assert!(run.stdout.contains("P=set E=yes"), "{}", run.stdout);
        // Whitelist без HOME: HOME у процесса пуст, PATH и EXTRA на месте.
        let run = run_harness("probe", &probe(vec!["PATH"]), &repo, "задача")
            .await
            .expect("run");
        assert!(
            run.stdout.contains("H=EMPTY P=set E=yes"),
            "изоляция: {}",
            run.stdout
        );
    }

    #[tokio::test]
    async fn auto_commit_clean_repo_is_noop() {
        // Исполнитель всё закоммитил сам (или ничего не писал) — харнесс
        // не плодит пустых коммитов.
        let tmp = tempfile::tempdir().expect("tempdir");
        let repo = tmp.path().join("repo");
        git_repo_with_baseline(&repo);
        let cfg = CodingHarnessConfig {
            binary: "sh".into(),
            args: vec!["-c".into(), "echo ok".into()],
            prompt_mode: PromptMode::Stdin,
            timeout_secs: 30,
            idle_timeout_secs: 0,
            ..CodingHarnessConfig::default()
        };
        let run = run_harness("clean", &cfg, &repo, "задача")
            .await
            .expect("run");
        assert_eq!(run.termination, Termination::Completed);
        assert!(run.auto_commit.is_none(), "пустой коммит не нужен");
        let log = git_out(&repo, &["log", "--oneline"]).expect("log");
        assert_eq!(log.lines().count(), 1, "{log}");
    }

    /// Конфиг с поддельным харнессом `fake` на бинаре `cat` (stdin → stdout).
    fn cfg_with_fake_harness(dir: &Path) -> Config {
        let mut cfg = cfg_in(dir);
        cfg.harnesses.insert(
            "fake".into(),
            CodingHarnessConfig {
                binary: "cat".into(),
                prompt_mode: PromptMode::Stdin,
                timeout_secs: 30,
                ..CodingHarnessConfig::default()
            },
        );
        cfg
    }

    #[test]
    fn parse_result_contract_validates_schema_mechanically() {
        // Валидный полный контракт в последнем fenced-блоке.
        let stdout = "текст\n```json\n{\"status\": \"partial\", \"assumptions\": [\"a\"], \
                      \"open_questions\": [], \"conflicts_with_prior_decisions\": []}\n```\n";
        let ContractParse::Valid(c) = parse_result_contract(stdout) else {
            panic!("контракт найден и валиден");
        };
        assert_eq!(c.status, ContractStatus::Partial);
        assert_eq!(c.assumptions, vec!["a".to_string()]);
        // Списки опциональны: дефолт — пустые.
        let ContractParse::Valid(c) =
            parse_result_contract("```json\n{\"status\": \"complete\"}\n```")
        else {
            panic!("валиден без списков");
        };
        assert_eq!(c.status, ContractStatus::Complete);
        assert!(c.open_questions.is_empty() && c.conflicts.is_empty());
        // Без поля status — не контракт.
        assert_eq!(
            parse_result_contract("```json\n{\"x\": 1}\n```"),
            ContractParse::Missing
        );
        assert_eq!(parse_result_contract("plain text"), ContractParse::Missing);
        // Битый JSON без status — промах; со status — Invalid (не молчим).
        assert_eq!(
            parse_result_contract("```json\n{oops}\n```"),
            ContractParse::Missing
        );
        let ContractParse::Invalid(reason) =
            parse_result_contract("```json\n{\"status\": \"complete\",\n```")
        else {
            panic!("битый блок со status — Invalid");
        };
        assert!(reason.contains("невалидный JSON"), "{reason}");
        // Схема: status вне перечисления — Invalid.
        let ContractParse::Invalid(reason) =
            parse_result_contract("```json\n{\"status\": \"done\"}\n```")
        else {
            panic!("status вне перечисления — Invalid");
        };
        assert!(reason.contains("done"), "{reason}");
        // Список не массивом — Invalid.
        let ContractParse::Invalid(reason) =
            parse_result_contract("```json\n{\"status\": \"complete\", \"assumptions\": {}}\n```")
        else {
            panic!("assumptions не массив — Invalid");
        };
        assert!(reason.contains("assumptions"), "{reason}");
        // Fence уронен — голый JSON в хвосте подхватывается.
        let ContractParse::Valid(c) = parse_result_contract(
            "проза ответа\n{\"status\": \"blocked\", \"open_questions\": [\"нужен доступ к КШД\"]}",
        ) else {
            panic!("голый JSON в хвосте — валидный контракт");
        };
        assert_eq!(c.status, ContractStatus::Blocked);
        assert_eq!(c.open_questions, vec!["нужен доступ к КШД".to_string()]);
        // Последний из нескольких fenced-блоков со status побеждает.
        let two = "```json\n{\"status\": \"partial\"}\n```\nпромежуток\n```json\n{\"status\": \"complete\"}\n```";
        let ContractParse::Valid(c) = parse_result_contract(two) else {
            panic!("последний блок валиден");
        };
        assert_eq!(c.status, ContractStatus::Complete);
    }

    #[tokio::test]
    async fn harness_run_tool_validates_args() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let cfg = cfg_with_fake_harness(tmp.path());
        let tool = HarnessRunTool { cfg: cfg.clone() };
        assert_eq!(tool.spec().name, "harness_run");
        let ctx = ToolContext::new(tmp.path().to_path_buf(), Arc::new(cfg));

        let out = tool.call(json!({"repo": "."}), &ctx).await.expect("call");
        assert!(
            out.is_error && out.content.contains("'harness'"),
            "{}",
            out.content
        );

        let out = tool
            .call(json!({"harness": "nope", "repo": "."}), &ctx)
            .await
            .expect("call");
        assert!(out.is_error, "{}", out.content);
        assert!(out.content.contains("не настроен"), "{}", out.content);
        assert!(
            out.content.contains("claude-code"),
            "список известных: {}",
            out.content
        );

        // Нет task и нет TASK.md — понятная ошибка с подсказкой.
        let out = tool
            .call(json!({"harness": "fake", "repo": "."}), &ctx)
            .await
            .expect("call");
        assert!(out.is_error, "{}", out.content);
        assert!(out.content.contains("handoff_create"), "{}", out.content);
    }

    #[tokio::test]
    async fn harness_run_background_registers_and_completes() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let mut cfg = cfg_with_fake_harness(tmp.path());
        cfg.paths.reports_dir = tmp.path().join("reports");
        let tool = HarnessRunTool { cfg: cfg.clone() };
        let registry = crate::subagent::SubagentRegistry::new();
        let ctx = ToolContext::new(tmp.path().to_path_buf(), Arc::new(cfg))
            .with_subagents(registry.clone());

        // Фон: инструмент возвращается сразу, задача hr-* в реестре.
        let out = tool
            .call(
                json!({"harness": "fake", "repo": ".", "task": "фон", "background": true}),
                &ctx,
            )
            .await
            .expect("call");
        assert!(!out.is_error, "{}", out.content);
        assert!(out.content.contains("hr-"), "id задачи: {}", out.content);
        assert_eq!(registry.running(), 1, "прогон числится запущенным");

        // cat завершается мгновенно — дожидаемся финиша задачи.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            let task = registry
                .list()
                .into_iter()
                .find(|t| t.id.starts_with("hr-"))
                .expect("задача hr-* в реестре");
            if task.status != crate::subagent::TaskStatus::Running {
                assert_eq!(
                    task.status,
                    crate::subagent::TaskStatus::Done,
                    "отчёт: {}",
                    task.report
                );
                assert!(task.report.contains("завершился"), "отчёт: {}", task.report);
                assert!(task.finished_at.is_some());
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "фоновый прогон не завершился за 10 с"
            );
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        // Полный лог лежит файлом reports/harness/<id>.log.
        let logs = std::fs::read_dir(tmp.path().join("reports/harness")).expect("logs dir");
        assert_eq!(logs.count(), 1, "один лог-файл фонового прогона");
    }

    #[tokio::test]
    async fn harness_run_background_without_registry_is_clear_error() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let cfg = cfg_with_fake_harness(tmp.path());
        let tool = HarnessRunTool { cfg: cfg.clone() };
        // Без with_subagents — реестр не подключён (headless).
        let ctx = ToolContext::new(tmp.path().to_path_buf(), Arc::new(cfg));
        let out = tool
            .call(
                json!({"harness": "fake", "repo": ".", "task": "фон", "background": true}),
                &ctx,
            )
            .await
            .expect("call");
        assert!(out.is_error, "{}", out.content);
        assert!(out.content.contains("реестр"), "{}", out.content);
    }

    #[tokio::test]
    async fn harness_run_hot_reloads_adapter_config() {
        // Регрессия: агент исправил [harnesses.*] в config.toml в ходе сессии,
        // а прогон шёл со снапшота конфига, загруженного при старте процесса
        // (дефолтный `-p` для hermes — «unrecognized arguments»). Теперь
        // адаптер перечитывается из файла на каждый вызов.
        let tmp = tempfile::tempdir().expect("tempdir");
        let cfg_path = tmp.path().join("config.toml");
        let write_cfg = |marker: &str| {
            std::fs::write(
                &cfg_path,
                format!(
                    "default_model = \"deepseek\"\n[harnesses.fake]\nbinary = \"sh\"\n\
                     args = [\"-c\", \"echo {marker}\"]\nprompt_mode = \"stdin\"\n\
                     timeout_secs = 30\nidle_timeout_secs = 0\nauto_commit = false\n"
                ),
            )
            .expect("write config");
        };
        write_cfg("ADAPTER_V1");
        let cfg = Config::load(Some(&cfg_path)).expect("load");
        assert_eq!(cfg.loaded_from.as_deref(), Some(cfg_path.as_path()));
        let tool = HarnessRunTool { cfg: cfg.clone() };
        let ctx = ToolContext::new(tmp.path().to_path_buf(), Arc::new(cfg));

        let out = tool
            .call(
                json!({"harness": "fake", "repo": ".", "task": "задача"}),
                &ctx,
            )
            .await
            .expect("call 1");
        assert!(out.content.contains("ADAPTER_V1"), "{}", out.content);

        // Правка файла между вызовами — без пересоздания инструмента.
        write_cfg("ADAPTER_V2");
        let out = tool
            .call(
                json!({"harness": "fake", "repo": ".", "task": "задача"}),
                &ctx,
            )
            .await
            .expect("call 2");
        assert!(out.content.contains("ADAPTER_V2"), "{}", out.content);
        assert!(!out.content.contains("ADAPTER_V1"), "{}", out.content);
    }

    #[tokio::test]
    async fn harness_run_reads_task_md_and_extracts_contract() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let repo = tmp.path().join("repo");
        write_file(
            &repo.join(".arch-handoff/TASK.md"),
            "Сделай фичу\n\n```json\n{\"status\": \"complete\", \"assumptions\": [], \
             \"open_questions\": [\"q1\"], \"conflicts_with_prior_decisions\": []}\n```\n",
        );
        let cfg = cfg_with_fake_harness(tmp.path());
        let tool = HarnessRunTool { cfg };
        let ctx = ToolContext::new(tmp.path().to_path_buf(), Arc::new(Config::default()));

        // cat вернёт TASK.md в stdout — контракт извлекается в сводку.
        let out = tool
            .call(json!({"harness": "fake", "repo": "repo"}), &ctx)
            .await
            .expect("call");
        assert!(!out.is_error, "{}", out.content);
        assert!(out.content.contains("код 0"), "{}", out.content);
        assert!(out.content.contains("status=complete"), "{}", out.content);
        assert!(out.content.contains("open_questions: 1"), "{}", out.content);
        assert!(out.content.contains("Сделай фичу"), "{}", out.content);
    }

    #[tokio::test]
    async fn harness_run_warns_when_contract_missing() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let mut cfg = cfg_in(tmp.path());
        cfg.harnesses.insert(
            "fake".into(),
            CodingHarnessConfig {
                binary: "echo".into(),
                args: vec!["нет контракта".into()],
                prompt_mode: PromptMode::Stdin,
                timeout_secs: 30,
                ..CodingHarnessConfig::default()
            },
        );
        let tool = HarnessRunTool { cfg };
        let ctx = ToolContext::new(tmp.path().to_path_buf(), Arc::new(Config::default()));
        // echo не читает stdin, печатает строку без контракта, код 0.
        let out = tool
            .call(
                json!({"harness": "fake", "repo": ".", "task": "задача"}),
                &ctx,
            )
            .await
            .expect("call");
        assert!(!out.is_error, "{}", out.content);
        assert!(
            out.content.contains("контракт результата"),
            "{}",
            out.content
        );
        assert!(out.content.contains("не найден"), "{}", out.content);
    }

    #[tokio::test]
    async fn harness_run_raises_tiny_timeout_to_floor() {
        // Модель оптимистично просит 30 с — поднимаем до 600 и честно
        // сообщаем об этом в сводке (ранний обрыв оставлял репо полусобранным).
        let tmp = tempfile::tempdir().expect("tempdir");
        let cfg = cfg_with_fake_harness(tmp.path());
        let tool = HarnessRunTool { cfg };
        let ctx = ToolContext::new(tmp.path().to_path_buf(), Arc::new(Config::default()));
        let out = tool
            .call(
                json!({"harness": "fake", "repo": ".", "task": "задача", "timeout_secs": 30}),
                &ctx,
            )
            .await
            .expect("call");
        assert!(!out.is_error, "{}", out.content);
        assert!(out.content.contains("поднят до 600"), "{}", out.content);
        // Явный разумный таймаут не трогаем.
        let out = tool
            .call(
                json!({"harness": "fake", "repo": ".", "task": "задача", "timeout_secs": 900}),
                &ctx,
            )
            .await
            .expect("call");
        assert!(!out.content.contains("поднят"), "{}", out.content);
    }

    #[test]
    fn harness_run_tool_timeout_covers_longest_run() {
        // Регрессия 11-24: агентный цикл обрывал вызов на жёстких 300 с
        // (TOOL_TIMEOUT_SECS), пока адаптер ждал 1800. Таймаут инструмента
        // обязан покрывать потолок аргумента (7200) плюс запас.
        let tool = HarnessRunTool {
            cfg: Config::default(),
        };
        assert!(tool.timeout_secs() >= 7200 + 120, "{}", tool.timeout_secs());
        let mut cfg = Config::default();
        cfg.harnesses.insert(
            "long".into(),
            CodingHarnessConfig {
                binary: "true".into(),
                timeout_secs: 8000,
                ..CodingHarnessConfig::default()
            },
        );
        assert_eq!(HarnessRunTool { cfg }.timeout_secs(), 8000 + 120);
    }

    #[tokio::test]
    async fn enforce_run_worktree_disabled_by_default() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let cfg = cfg_in(tmp.path());
        // require_worktree = false (дефолт) — enforcement выключен.
        let res = enforce_run_worktree(&cfg, tmp.path(), "fake")
            .await
            .expect("enforce");
        assert!(res.is_none());
    }

    #[tokio::test]
    async fn enforce_run_worktree_creates_worktree_and_copies_handoff() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let repo = tmp.path().join("repo");
        git_repo_with_baseline(&repo);
        // Handoff-пакет по контракту НЕ коммитится — в свежем worktree его нет,
        // enforcement обязан его скопировать.
        write_file(&repo.join(".arch-handoff/TASK.md"), "задача из пакета\n");
        let mut cfg = cfg_in(tmp.path());
        cfg.paths.reports_dir = tmp.path().join("reports");
        cfg.fleet.require_worktree = true;

        let (dir, run_id) = enforce_run_worktree(&cfg, &repo, "Claude Code")
            .await
            .expect("enforce")
            .expect("worktree создан");
        // Имя — kebab-case «<слаг>-<timestamp>» (слаг санитизирован).
        assert!(run_id.starts_with("claude-code-"), "{run_id}");
        assert!(dir.join("README.md").is_file(), "worktree имеет базу");
        let task = std::fs::read_to_string(dir.join(".arch-handoff/TASK.md")).expect("TASK.md");
        assert_eq!(task, "задача из пакета\n");
        // Основное дерево не тронуто: ветка worktree в списке фабрики.
        let infos = crate::worktree::list(&repo).await.expect("list");
        assert_eq!(infos.len(), 1, "{infos:?}");
        assert_eq!(infos[0].name, run_id);
    }

    #[tokio::test]
    async fn enforce_run_worktree_requires_git_repo() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let mut cfg = cfg_in(tmp.path());
        cfg.fleet.require_worktree = true;
        // Не git-репозиторий — понятная ошибка ДО запуска харнесса.
        let err = enforce_run_worktree(&cfg, tmp.path(), "fake")
            .await
            .expect_err("не git-репозиторий");
        assert!(err.to_string().contains("не git-репозиторий"), "{err}");
    }

    #[tokio::test]
    async fn harness_run_tool_enforces_worktree_when_required() {
        // Сквозной путь: [fleet] require_worktree = true → harness_run прогоняет
        // харнесс в worktree, основное дерево остаётся чистым, в сводке —
        // run-id и указание на гейт владельца.
        let tmp = tempfile::tempdir().expect("tempdir");
        let repo = tmp.path().join("repo");
        git_repo_with_baseline(&repo);
        let mut cfg = cfg_with_fake_harness(tmp.path());
        cfg.paths.reports_dir = tmp.path().join("reports");
        cfg.fleet.require_worktree = true;
        // Исполнитель пишет файл и НЕ коммитит (auto_commit выключен, чтобы
        // работа осталась незакоммиченной в worktree — main всё равно чист).
        cfg.harnesses.get_mut("fake").expect("fake").auto_commit = false;
        cfg.harnesses.insert(
            "writer".into(),
            CodingHarnessConfig {
                binary: "sh".into(),
                args: vec![
                    "-c".into(),
                    "echo код > feature.txt; echo '{\"status\": \"complete\"}'".into(),
                ],
                prompt_mode: PromptMode::Stdin,
                timeout_secs: 30,
                idle_timeout_secs: 0,
                auto_commit: false,
                ..CodingHarnessConfig::default()
            },
        );
        let tool = HarnessRunTool { cfg };
        let ctx = ToolContext::new(tmp.path().to_path_buf(), Arc::new(Config::default()));
        let out = tool
            .call(
                json!({"harness": "writer", "repo": "repo", "task": "сделать фичу"}),
                &ctx,
            )
            .await
            .expect("call");
        assert!(!out.is_error, "{}", out.content);
        assert!(out.content.contains("run-id 'writer-"), "{}", out.content);
        assert!(
            out.content.contains("arch fleet merge"),
            "указание на гейт: {}",
            out.content
        );
        // Основное дерево чисто — работа осталась в worktree.
        let status = git_out(&repo, &["status", "--porcelain"]).expect("status");
        assert!(status.trim().is_empty(), "main не тронут: {status}");
        assert!(!repo.join("feature.txt").is_file());
        let infos = crate::worktree::list(&repo).await.expect("list");
        assert_eq!(infos.len(), 1, "{infos:?}");
        assert!(infos[0].path.join("feature.txt").is_file());
    }
}
