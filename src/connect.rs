//! `arch-be connect <host>` — подключение Spine к внешнему CLI-агенту.
//!
//! Шаг 3 плана инверсии харнесса (`spine-bank-inversion`): Spine без
//! собственной LLM становится «органом» чужого агента — MCP-сервер
//! (`arch-be mcp serve`, контракт — `mcp_server.rs`), пакет скиллов и хуки
//! жизненного цикла; думает хост (Claude Code, Codex, Qwen Code, Kimi Code).
//!
//! ОТСТУПЛЕНИЕ ОТ ПЛАНА: в плане шаг назван `arch-be export <host>`; имя
//! `export` занято командой экспорта журнала сессии в Word/Excel
//! (`Cmd::Export` в `main.rs`), поэтому команда называется `connect`.
//!
//! Что пишется в проект (`--dir`, по умолчанию — текущий каталог):
//! - `claude`: `.mcp.json` (мердж ключа `mcpServers.spine`, чужие серверы
//!   сохраняются), `.claude/settings.json` (мердж хуков; наши помечены
//!   комментарием `# spine-connect:*` в команде и не дублируются при
//!   повторном запуске), `.claude/skills/<имя>/` (копии скиллов из
//!   встроенных плагинов [`crate::assets::embedded_plugin_files`] —
//!   работает из релизного бинаря без `arch-be init`), `CLAUDE.md`
//!   (создаётся краткий либо блок между маркерами
//!   `<!-- SPINE:BEGIN -->`/`<!-- SPINE:END -->`; рукописное не затирается);
//! - `qwen`: `.qwen/settings.json` (мердж `mcpServers`; Qwen Code — форк
//!   gemini-cli, ключ `mcpServers` верхнего уровня подтверждён). Скиллы и
//!   хуки НЕ пишутся: layout скиллов и схема хуков Qwen Code не
//!   подтверждены — печатаются сниппеты с заметкой (поведение `generic`);
//! - `codex`: MCP у Codex — только пользовательский `~/.codex/config.toml`
//!   (`[mcp_servers.spine]`); молча в дом пользователя не пишем: печать
//!   готового TOML-блока, запись с мерджем — только по явному
//!   `--apply-global` (перед первой перезаписью — бэкап
//!   `*.bak-spine-connect`). AGENTS.md Codex читает нативно: файл сами не
//!   генерируем (наименее инвазивный вариант), при отсутствии —
//!   рекомендация `arch-be agents-md refresh .`;
//! - `kimi`: проектный `.kimi-code/mcp.json` (мердж `mcpServers.spine` по
//!   правилам `.mcp.json` claude; layout подтверждён официальной докой
//!   kimi.com/code/docs: project-уровень перекрывает user-уровень, поле
//!   `cwd` не пишем — сервер наследует cwd харнесса). Плюс печать: блок для
//!   ручной регистрации в user-level `~/.kimi-code/mcp.json` (альтернатива;
//!   запись туда с мерджем и бэкапом — по `--apply-global`) и TOML-блок
//!   хука `[[hooks]]` для `~/.kimi-code/config.toml` (хуки у Kimi Code
//!   бывают только пользовательского уровня; схема `event`/`matcher`/
//!   `command`/`timeout` подтверждена докой, exit 2 = блок) и напоминание
//!   про trust-диалог при первом запуске в каталоге;
//! - `omp` (oh-my-pi): `.mcp.json` (мердж, как у claude — omp дискаверит
//!   проектный файл автоматически, регистрация не нужна). Скиллы omp читает
//!   нативно из `.claude/skills/`: если такого каталога в проекте ещё нет,
//!   встроенные скиллы раскладываются туда же, как у claude; каталог уже
//!   есть — не трогаем (чужую библиотеку не перетираем). Хуков через
//!   connect нет: механизм хуков omp — TypeScript-расширения, печатается
//!   указание на `omp --hook <file.ts>`;
//! - `generic`: только печать — сниппеты `.mcp.json` и хуков, куда что
//!   вставить вручную.
//!
//! Хуки (для `claude` — запись в `settings.json`; для остальных — печать).
//! События Claude Code: `Stop` (дефолт) и `PostToolUse` с matcher
//! `Edit|Write|MultiEdit` (только под `--strict-hooks`). Семантика
//! exit-кодов Claude Code: 0 — ок, 2 — блок с показом stderr агенту.
//! Консервативный дефолт — fail-soft на инфраструктуру, fail-hard на
//! вердикт: хук молча пропускается (exit 0), если `arch-be` не в PATH или
//! в проекте нет `.arch-handoff/CONSTRAINTS.yaml` (гард); блок (exit 2) —
//! только когда `arch-be control check .` завершился строкой «Итог: FAIL».
//! Так инфраструктурные сбои (битый конфиг, ошибка запуска — в выводе
//! «Error:», а не «Итог: FAIL») не останавливают сессию, а реальные
//! нарушения fitness-правил — стопят её. `PostToolUse` в дефолт не входит:
//! `control check` на репозитории с правилами `command_succeeds` может
//! гонять сборки/тесты — для каждой правки это слишком дорого.
//!
//! Идемпотентность: повторный запуск даёт тот же результат — JSON
//! смерджен ключ-в-ключ, хуки не дублируются (поиск маркера
//! [`HOOK_MARKER`]), скиллы побайтово совпадают. `--dry-run` печатает план
//! без единой записи.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::error::{HarnessError, Result};

/// Имя нашего MCP-сервера в конфигах хостов.
const MCP_SERVER_NAME: &str = "spine";
/// Маркер наших хуков: комментарий в конце команды; по нему повторный
/// `connect` узнаёт свои записи и не плодит дубли.
const HOOK_MARKER: &str = "spine-connect";
/// Маркер начала блока Spine в CLAUDE.md.
const SPINE_BEGIN: &str = "<!-- SPINE:BEGIN -->";
/// Маркер конца блока Spine в CLAUDE.md.
const SPINE_END: &str = "<!-- SPINE:END -->";
/// Потолок размера одного файла скилла при копировании (байт): защита от
/// случайного переноса тяжёлых вложений в проект хоста.
const MAX_SKILL_FILE_BYTES: usize = 200 * 1024;
/// Таймаут Stop-хука, секунды (поле `timeout` Claude Code).
const STOP_HOOK_TIMEOUT_SECS: u64 = 150;
/// Таймаут PostToolUse-хука под `--strict-hooks`, секунды.
const POST_TOOL_USE_HOOK_TIMEOUT_SECS: u64 = 300;

/// Хост-получатель подключения.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Host {
    /// Claude Code: `.mcp.json`, `.claude/settings.json`, `.claude/skills/`, CLAUDE.md.
    Claude,
    /// Qwen Code: `.qwen/settings.json` (mcpServers); скиллы/хуки — печать.
    Qwen,
    /// Codex: `~/.codex/config.toml` — печать блока или `--apply-global`.
    Codex,
    /// Kimi Code: проектный `.kimi-code/mcp.json` (мердж); user-level
    /// `~/.kimi-code/mcp.json` — печать блока или `--apply-global`; хук —
    /// печать TOML-блока для `~/.kimi-code/config.toml`.
    Kimi,
    /// oh-my-pi: `.mcp.json` (автодискавери) + скиллы в `.claude/skills/`,
    /// если того каталога ещё нет.
    Omp,
    /// Любой другой агент: только печать сниппетов.
    Generic,
}

impl Host {
    /// Разбор значения CLI: `claude` | `qwen` | `codex` | `kimi` | `omp` |
    /// `generic` (допускаются составные алиасы `claude-code`, `qwen-code`,
    /// `kimi-code`, `oh-my-pi`).
    ///
    /// # Errors
    /// Неизвестное имя хоста — сообщение со списком допустимых.
    pub fn parse(raw: &str) -> std::result::Result<Self, String> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "claude" | "claude-code" => Ok(Self::Claude),
            "qwen" | "qwen-code" => Ok(Self::Qwen),
            "codex" => Ok(Self::Codex),
            "kimi" | "kimi-code" => Ok(Self::Kimi),
            "omp" | "oh-my-pi" => Ok(Self::Omp),
            "generic" => Ok(Self::Generic),
            other => Err(format!(
                "неизвестный хост '{other}' (допустимы: claude, qwen, codex, kimi, omp, generic)"
            )),
        }
    }

    /// Каноничное имя для вывода.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Qwen => "qwen",
            Self::Codex => "codex",
            Self::Kimi => "kimi",
            Self::Omp => "omp",
            Self::Generic => "generic",
        }
    }
}

/// Параметры подключения (собираются из CLI в `main.rs`).
#[derive(Debug, Clone)]
pub struct ConnectOptions {
    /// Хост-получатель.
    pub host: Host,
    /// Каталог проекта.
    pub dir: PathBuf,
    /// Открыть rw-контур MCP-сервера (`arch-be mcp serve --rw`).
    pub rw: bool,
    /// Раскладывать скиллы (false = `--no-skills`).
    pub skills: bool,
    /// Встраивать хуки (false = `--no-hooks`).
    pub hooks: bool,
    /// Трогать CLAUDE.md / рекомендацию AGENTS.md (false = `--no-agents-md`).
    pub agents_md: bool,
    /// PostToolUse-гейт на каждую правку (только claude).
    pub strict_hooks: bool,
    /// Записывать пользовательский конфиг хоста (codex/kimi) вместо печати.
    pub apply_global: bool,
    /// Только план, без записи.
    pub dry_run: bool,
    /// Домашний каталог пользователя для `--apply-global` (в тестах — tempdir).
    pub home: Option<PathBuf>,
}

impl ConnectOptions {
    /// Дефолтные параметры для хоста и каталога: всё включено, запись в дом
    /// выключена, dry-run выключен.
    #[must_use]
    pub fn new(host: Host, dir: PathBuf) -> Self {
        Self {
            host,
            dir,
            rw: false,
            skills: true,
            hooks: true,
            agents_md: true,
            strict_hooks: false,
            apply_global: false,
            dry_run: false,
            home: None,
        }
    }
}

/// Исход копирования одного скилла.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkillAction {
    /// Скопирован заново (число записанных файлов).
    Copied(usize),
    /// Обновлён встроенной версией (число перезаписанных файлов; прежняя
    /// локальная правка заменена).
    Updated(usize),
    /// Уже актуален (число файлов, побайтово совпадают).
    Unchanged(usize),
}

/// Строка отчёта по одному скиллу.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillOutcome {
    /// Имя скилла (каталог назначения).
    pub name: String,
    /// Что произошло.
    pub action: SkillAction,
}

/// Отчёт о подключении (печать — [`render_report`]).
#[derive(Debug, Default)]
pub struct ConnectReport {
    /// Созданные файлы.
    pub created: Vec<PathBuf>,
    /// Существующие файлы, в которые смерджены наши ключи.
    pub merged: Vec<PathBuf>,
    /// Файлы, уже бывшие актуальными (идемпотентный повтор).
    pub unchanged: Vec<PathBuf>,
    /// Итоги копирования скиллов (по одному элементу на скилл).
    pub skills: Vec<SkillOutcome>,
    /// Пропущенные файлы скиллов (с причиной).
    pub skills_skipped: Vec<String>,
    /// Заметки (сохранённые чужие ключи, отступления, пропуски).
    pub notes: Vec<String>,
    /// Сниппеты для ручной вставки: (заголовок, текст).
    pub snippets: Vec<(String, String)>,
    /// Следующие шаги для пользователя.
    pub next_steps: Vec<String>,
    /// Режим dry-run: ничего не записано, списки выше — план.
    pub dry_run: bool,
}

/// Выполняет подключение (или планирует его при `dry_run`).
///
/// # Errors
/// Существующий целевой JSON/TOML не разбирается или имеет неожиданную
/// форму (чужое не затираем — разбор вручную); ошибки чтения/записи файлов;
/// `--apply-global` без определимого домашнего каталога.
pub fn connect(opts: &ConnectOptions) -> Result<ConnectReport> {
    let mut report = ConnectReport {
        dry_run: opts.dry_run,
        ..ConnectReport::default()
    };
    if !opts.dir.is_dir() {
        if opts.dry_run {
            report
                .notes
                .push(format!("каталог {} будет создан", opts.dir.display()));
        } else {
            std::fs::create_dir_all(&opts.dir).map_err(|e| HarnessError::io(&opts.dir, e))?;
        }
    }
    match opts.host {
        Host::Claude => connect_claude(opts, &mut report)?,
        Host::Qwen => connect_qwen(opts, &mut report)?,
        Host::Codex => connect_codex(opts, &mut report)?,
        Host::Kimi => connect_kimi(opts, &mut report)?,
        Host::Omp => connect_omp(opts, &mut report)?,
        Host::Generic => connect_generic(opts, &mut report),
    }
    Ok(report)
}

/// Описание нашего MCP-сервера для JSON-конфигов хостов.
fn mcp_server_value(rw: bool) -> Value {
    let args = if rw {
        vec!["mcp", "serve", "--rw"]
    } else {
        vec!["mcp", "serve"]
    };
    json!({"command": "arch-be", "args": args})
}

/// Читает JSON-объект из файла: (объект, исходный текст, если файл был).
/// Отсутствующий файл — пустой объект; битый JSON или не-объект верхнего
/// уровня — ошибка (чужой файл не затираем).
fn read_json_object(path: &Path) -> Result<(serde_json::Map<String, Value>, Option<String>)> {
    match std::fs::read_to_string(path) {
        Ok(text) => {
            let value: Value = serde_json::from_str(&text).map_err(|e| {
                HarnessError::Config(format!(
                    "{}: существующий файл не разбирается как JSON ({e}) — \
                     не затираю; разберите вручную",
                    path.display()
                ))
            })?;
            let obj = value.as_object().cloned().ok_or_else(|| {
                HarnessError::Config(format!(
                    "{}: ожидался JSON-объект верхнего уровня — не затираю",
                    path.display()
                ))
            })?;
            Ok((obj, Some(text)))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok((serde_json::Map::new(), None)),
        Err(e) => Err(HarnessError::io(path, e)),
    }
}

/// Фиксирует новое содержимое файла: пишет (при dry-run — только планирует)
/// и классифицирует в отчёте. `old = None` — файл ранее не существовал.
fn commit_file(
    path: &Path,
    old: Option<&str>,
    new: &str,
    dry_run: bool,
    report: &mut ConnectReport,
) -> Result<()> {
    if old == Some(new) {
        report.unchanged.push(path.to_path_buf());
        return Ok(());
    }
    if !dry_run {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| HarnessError::io(parent, e))?;
        }
        std::fs::write(path, new).map_err(|e| HarnessError::io(path, e))?;
    }
    if old.is_some() {
        report.merged.push(path.to_path_buf());
    } else {
        report.created.push(path.to_path_buf());
    }
    Ok(())
}

/// Разовый бэкап файла перед перезаписью (только `--apply-global`):
/// `<имя>.bak-spine-connect` создаётся один раз, повторные прогоны его
/// не трогают.
fn backup_once(path: &Path, old: &str, dry_run: bool, report: &mut ConnectReport) -> Result<()> {
    let name = path.file_name().map_or_else(
        || "config.bak-spine-connect".to_string(),
        |n| format!("{}.bak-spine-connect", n.to_string_lossy()),
    );
    let backup = path.with_file_name(name);
    if backup.exists() || dry_run {
        return Ok(());
    }
    std::fs::write(&backup, old).map_err(|e| HarnessError::io(&backup, e))?;
    report
        .notes
        .push(format!("бэкап прежнего конфига: {}", backup.display()));
    Ok(())
}

/// Мердж `mcpServers.spine` в JSON-конфиг хоста (claude/omp `.mcp.json`,
/// qwen `.qwen/settings.json`, kimi `.kimi-code/mcp.json` проекта и
/// `~/.kimi-code/mcp.json`): чужие ключи верхнего уровня и чужие серверы
/// сохраняются, перезаписывается только наш сервер. `backup` — для
/// пользовательских конфигов (`--apply-global`).
fn merge_mcp_servers_json(
    path: &Path,
    rw: bool,
    backup: bool,
    dry_run: bool,
    report: &mut ConnectReport,
) -> Result<()> {
    let (mut root, old) = read_json_object(path)?;
    let servers_value = root
        .entry("mcpServers".to_string())
        .or_insert_with(|| Value::Object(serde_json::Map::new()));
    let Some(servers) = servers_value.as_object_mut() else {
        return Err(HarnessError::Config(format!(
            "{}: ключ mcpServers не является объектом — не затираю",
            path.display()
        )));
    };
    let foreign: Vec<String> = servers
        .keys()
        .filter(|k| k.as_str() != MCP_SERVER_NAME)
        .cloned()
        .collect();
    servers.insert(MCP_SERVER_NAME.to_string(), mcp_server_value(rw));
    if !foreign.is_empty() {
        report.notes.push(format!(
            "{}: существующие MCP-серверы сохранены: {}",
            path.display(),
            foreign.join(", ")
        ));
    }
    let new = match serde_json::to_string_pretty(&Value::Object(root)) {
        Ok(text) => format!("{text}\n"),
        Err(e) => return Err(HarnessError::Json(e)),
    };
    if backup {
        if let Some(prev) = old.as_deref() {
            if prev != new {
                backup_once(path, prev, dry_run, report)?;
            }
        }
    }
    commit_file(path, old.as_deref(), &new, dry_run, report)
}

/// Команда Stop-хука Claude Code: fitness-гейт перед завершением сессии.
/// Гард `command -v arch-be` + наличие `.arch-handoff/CONSTRAINTS.yaml`;
/// блок (exit 2, stderr агенту) — только по строке «Итог: FAIL».
fn stop_hook_command() -> String {
    format!(
        "if command -v arch-be >/dev/null 2>&1 && [ -f .arch-handoff/CONSTRAINTS.yaml ]; then \
         out=$(arch-be control check . 2>&1); \
         case \"$out\" in *\"Итог: FAIL\"*) \
         printf '%s\\n\\n%s\\n' \"$out\" \
         \"{HOOK_MARKER}: fitness-гейт FAIL — исправьте находки error перед завершением \
         (подробности выше; гейт: arch-be control check .)\" >&2; exit 2;; esac; fi \
         # {HOOK_MARKER}:stop"
    )
}

/// Команда PostToolUse-хука (`--strict-hooks`): тот же гейт на каждую
/// правку файла (matcher `Edit|Write|MultiEdit`).
fn post_tool_use_hook_command() -> String {
    format!(
        "if command -v arch-be >/dev/null 2>&1 && [ -f .arch-handoff/CONSTRAINTS.yaml ]; then \
         out=$(arch-be control check . 2>&1); \
         case \"$out\" in *\"Итог: FAIL\"*) \
         printf '%s\\n\\n%s\\n' \"$out\" \
         \"{HOOK_MARKER}: правка нарушает fitness-правила (control check FAIL) — \
         исправьте находки error\" >&2; exit 2;; esac; fi \
         # {HOOK_MARKER}:post-tool-use"
    )
}

/// Есть ли в массиве групп хуков события запись с нашим маркером.
fn has_marked_hook(entries: &[Value]) -> bool {
    entries.iter().any(|group| {
        group
            .get("hooks")
            .and_then(Value::as_array)
            .is_some_and(|cmds| {
                cmds.iter().any(|c| {
                    c.get("command")
                        .and_then(Value::as_str)
                        .is_some_and(|cmd| cmd.contains(HOOK_MARKER))
                })
            })
    })
}

/// Добавляет наш хук в массив события `hooks[event]`, если записи с
/// маркером там ещё нет (идемпотентность). Возвращает true, если изменил.
///
/// # Errors
/// Существующее значение `hooks[event]` не массив — не затираем чужое.
fn upsert_hook(
    hooks: &mut serde_json::Map<String, Value>,
    event: &str,
    matcher: Option<&str>,
    command: &str,
    timeout_secs: u64,
) -> Result<bool> {
    let entries_value = hooks.entry(event.to_string()).or_insert_with(|| json!([]));
    let Some(entries) = entries_value.as_array_mut() else {
        return Err(HarnessError::Config(format!(
            "hooks.{event}: ожидался массив групп хуков — не затираю"
        )));
    };
    if has_marked_hook(entries) {
        return Ok(false);
    }
    let mut group = json!({
        "hooks": [{"type": "command", "command": command, "timeout": timeout_secs}]
    });
    if let Some(m) = matcher {
        group["matcher"] = json!(m);
    }
    entries.push(group);
    Ok(true)
}

/// Мердж хуков в `.claude/settings.json`: событие `Stop` всегда,
/// `PostToolUse` (matcher `Edit|Write|MultiEdit`) — под `--strict-hooks`.
/// Чужие ключи и хуки не трогаются.
fn merge_claude_settings(
    path: &Path,
    strict: bool,
    dry_run: bool,
    report: &mut ConnectReport,
) -> Result<()> {
    let (mut root, old) = read_json_object(path)?;
    let hooks_value = root.entry("hooks".to_string()).or_insert_with(|| json!({}));
    let Some(hooks) = hooks_value.as_object_mut() else {
        return Err(HarnessError::Config(format!(
            "{}: ключ hooks не является объектом — не затираю",
            path.display()
        )));
    };
    let mut added: Vec<&str> = Vec::new();
    if upsert_hook(
        hooks,
        "Stop",
        None,
        &stop_hook_command(),
        STOP_HOOK_TIMEOUT_SECS,
    )? {
        added.push("Stop");
    }
    if strict
        && upsert_hook(
            hooks,
            "PostToolUse",
            Some("Edit|Write|MultiEdit"),
            &post_tool_use_hook_command(),
            POST_TOOL_USE_HOOK_TIMEOUT_SECS,
        )?
    {
        added.push("PostToolUse");
    }
    if !added.is_empty() {
        report.notes.push(format!(
            "хуки добавлены: {} (маркер «{HOOK_MARKER}» — повторный connect дублей не плодит)",
            added.join(", ")
        ));
    }
    report.notes.push(
        "семантика хуков: fail-soft на инфраструктуру (нет arch-be или \
         .arch-handoff/CONSTRAINTS.yaml — молча пропуск), блок (exit 2) — \
         только при вердикте «Итог: FAIL»; PostToolUse-гейт на каждую правку — \
         через --strict-hooks (дорого на репозиториях с command_succeeds-правилами)"
            .into(),
    );
    let new = match serde_json::to_string_pretty(&Value::Object(root)) {
        Ok(text) => format!("{text}\n"),
        Err(e) => return Err(HarnessError::Json(e)),
    };
    commit_file(path, old.as_deref(), &new, dry_run, report)
}

/// Гарантирует минимальный frontmatter SKILL.md (agent-plugins.org): если
/// шапки `---` с ключами `name:`/`description:` нет — синтезирует из имени
/// каталога и первой непустой строки тела. Все встроенные скиллы шапку уже
/// имеют (охраняется тестом `assets.rs`) — это страховка на будущее.
fn ensure_frontmatter(content: &str, fallback_name: &str) -> String {
    let mut has_name = false;
    let mut has_description = false;
    if content.starts_with("---") {
        for line in content.lines().skip(1) {
            if line.trim() == "---" {
                break;
            }
            if line.starts_with("name:") {
                has_name = true;
            }
            if line.starts_with("description:") {
                has_description = true;
            }
        }
    }
    if has_name && has_description {
        return content.to_string();
    }
    let description = content
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map_or_else(
            || "Скилл библиотеки Spine".to_string(),
            |l| l.trim_start_matches('#').trim().chars().take(160).collect(),
        );
    format!("---\nname: {fallback_name}\ndescription: {description}\n---\n\n{content}")
}

/// Собирает файлы скиллов из встроенных плагинов: «относительный путь
/// назначения внутри skills-root → содержимое». Файлы крупнее
/// [`MAX_SKILL_FILE_BYTES`] пропускаются с заметкой; дубли пути назначения
/// (одно имя скилла в двух плагинах) — тоже (побеждает первый).
fn collect_skill_files_from(files: &[(&str, &str)]) -> (Vec<(PathBuf, String)>, Vec<String>) {
    let mut out: Vec<(PathBuf, String)> = Vec::new();
    let mut skipped = Vec::new();
    let mut seen: BTreeSet<PathBuf> = BTreeSet::new();
    for (rel, content) in files {
        let segs: Vec<&str> = rel.split('/').collect();
        // plugins/<plugin>/skills/<skill>/<файл…>
        if segs.len() < 5 || segs[0] != "plugins" || segs[2] != "skills" {
            continue;
        }
        if content.len() > MAX_SKILL_FILE_BYTES {
            skipped.push(format!(
                "{rel}: {} КБ — больше потолка {} КБ",
                content.len() / 1024,
                MAX_SKILL_FILE_BYTES / 1024
            ));
            continue;
        }
        let skill = segs[3];
        let rest = segs[4..].join("/");
        let dest = PathBuf::from(skill).join(&rest);
        if !seen.insert(dest.clone()) {
            skipped.push(format!("{rel}: дубль пути назначения {}", dest.display()));
            continue;
        }
        let body = if rest == "SKILL.md" {
            ensure_frontmatter(content, skill)
        } else {
            (*content).to_string()
        };
        out.push((dest, body));
    }
    out.sort();
    (out, skipped)
}

/// Раскладывает встроенные скиллы в `root` (`.claude/skills` хоста):
/// новые копируются, отличающиеся — перезаписываются встроенной версией
/// (заметка в отчёте), совпадающие — пропускаются (идемпотентность).
fn install_skills(root: &Path, dry_run: bool, report: &mut ConnectReport) -> Result<()> {
    let (files, skipped) = collect_skill_files_from(crate::assets::embedded_plugin_files());
    report.skills_skipped.extend(skipped);
    // Агрегация по имени скилла: (новых, перезаписанных, без изменений).
    let mut by_skill: BTreeMap<String, (usize, usize, usize)> = BTreeMap::new();
    for (rel, content) in &files {
        let path = root.join(rel);
        let skill = rel.components().next().map_or_else(
            || "?".into(),
            |c| c.as_os_str().to_string_lossy().into_owned(),
        );
        let old = std::fs::read_to_string(&path).ok();
        let counts = by_skill.entry(skill).or_default();
        match old.as_deref() {
            None => counts.0 += 1,
            Some(prev) if prev == content => counts.2 += 1,
            Some(_) => counts.1 += 1,
        }
        if dry_run || old.as_deref() == Some(content.as_str()) {
            continue;
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| HarnessError::io(parent, e))?;
        }
        std::fs::write(&path, content).map_err(|e| HarnessError::io(&path, e))?;
    }
    for (name, (copied, updated, unchanged)) in by_skill {
        let action = if copied == 0 && updated == 0 {
            SkillAction::Unchanged(unchanged)
        } else if updated > 0 {
            SkillAction::Updated(copied + updated)
        } else {
            SkillAction::Copied(copied)
        };
        if updated > 0 {
            report.notes.push(format!(
                "скилл «{name}»: прежняя локальная правка заменена встроенной версией \
                 ({updated} файлов)"
            ));
        }
        report.skills.push(SkillOutcome { name, action });
    }
    Ok(())
}

/// Тело блока Spine в CLAUDE.md (между маркерами [`SPINE_BEGIN`]/[`SPINE_END`]).
fn claude_md_block() -> String {
    format!(
        "{SPINE_BEGIN}\n\
         ## Архитектурный контроль (Spine)\n\
         \n\
         В репозитории подключён MCP-сервер `spine` (`arch-be mcp serve`, см. `.mcp.json`):\n\
         - `fitness_check` — прогон CONSTRAINTS.yaml; вызывай ПЕРЕД коммитом: `passed=false` \
         с находками error — откажи изменению и перечисли находки.\n\
         - `spine_lint` / `trace_check` / `significance_score` — линтер спайна, \
         трассируемость, маршрут значимости.\n\
         - Чтение знаний: `kb_search`, `skill_search`, `skill_load`; модель архитектуры: \
         `model_query`.\n\
         \n\
         Точка входа в инструкции репозитория — AGENTS.md (нет файла → \
         `arch-be agents-md refresh .`).\n\
         {SPINE_END}"
    )
}

/// Вставляет блок в существующий файл: замена между маркерами, нет
/// маркеров — дописка в конец. Рукописная зона сохраняется; детерминировано
/// (повторный прогон побайтово совпадает).
fn splice_block(existing: &str, block: &str) -> String {
    let trimmed_block = block.trim_end();
    match (existing.find(SPINE_BEGIN), existing.find(SPINE_END)) {
        (Some(b), Some(e)) if b < e => {
            let pre = existing[..b].trim_end();
            let tail = existing[e + SPINE_END.len()..].trim();
            if tail.is_empty() {
                format!("{pre}\n\n{trimmed_block}\n")
            } else {
                format!("{pre}\n\n{trimmed_block}\n\n{tail}\n")
            }
        }
        _ => format!("{}\n\n{trimmed_block}\n", existing.trim_end()),
    }
}

/// Создаёт CLAUDE.md (краткий: ссылка на AGENTS.md и как звать Spine) или
/// дописывает/обновляет помеченный блок — рукописное не затирается.
fn upsert_claude_md(path: &Path, dry_run: bool, report: &mut ConnectReport) -> Result<()> {
    let block = claude_md_block();
    let old = std::fs::read_to_string(path).ok();
    let new = match old.as_deref() {
        None => format!(
            "# CLAUDE.md\n\n\
             Контекст для Claude Code. Точка входа в инструкции репозитория — AGENTS.md.\n\n\
             {block}\n"
        ),
        Some(existing) => splice_block(existing, &block),
    };
    commit_file(path, old.as_deref(), &new, dry_run, report)
}

/// `arch-be connect claude`: проектный `.mcp.json`, хуки в
/// `.claude/settings.json`, скиллы в `.claude/skills/`, блок в CLAUDE.md.
fn connect_claude(opts: &ConnectOptions, report: &mut ConnectReport) -> Result<()> {
    merge_mcp_servers_json(
        &opts.dir.join(".mcp.json"),
        opts.rw,
        false,
        opts.dry_run,
        report,
    )?;
    if opts.hooks {
        merge_claude_settings(
            &opts.dir.join(".claude/settings.json"),
            opts.strict_hooks,
            opts.dry_run,
            report,
        )?;
    }
    if opts.skills {
        install_skills(&opts.dir.join(".claude/skills"), opts.dry_run, report)?;
        report.notes.push(
            "субагенты плагинов (agents/*.md) не разворачиваются — при необходимости \
             скопируйте вручную из ~/.arch-harness/plugins после `arch-be init`"
                .into(),
        );
    }
    if opts.agents_md {
        upsert_claude_md(&opts.dir.join("CLAUDE.md"), opts.dry_run, report)?;
    }
    report.next_steps.extend([
        "перезапустите Claude Code (`claude`) в этом каталоге".to_string(),
        "проверьте подключение: `claude mcp list` — в списке сервер «spine»".to_string(),
        format!(
            "инструменты видны агенту как `mcp__spine__*`; режим сервера: {}",
            if opts.rw {
                "rw (разрешены аддитивные записи: handoff_create, adr_new, …)"
            } else {
                "read-only"
            }
        ),
    ]);
    Ok(())
}

/// Сниппет `.mcp.json` для печати (generic-потоки).
fn mcp_json_snippet(rw: bool) -> String {
    let v = json!({"mcpServers": {MCP_SERVER_NAME: mcp_server_value(rw)}});
    match serde_json::to_string_pretty(&v) {
        Ok(text) => text,
        // Сериализация Value не падает; запасной вариант — компактная форма.
        Err(_) => v.to_string(),
    }
}

/// Сниппет хуков для печати (формат Claude Code settings.json): те же
/// команды, что пишутся мерджем, — у хостов без записи виден точный текст.
fn hooks_snippet(strict: bool) -> String {
    let mut hooks = serde_json::Map::new();
    // Свежая карта — upsert заведомо добавляет; ошибка формы невозможна.
    let _ = upsert_hook(
        &mut hooks,
        "Stop",
        None,
        &stop_hook_command(),
        STOP_HOOK_TIMEOUT_SECS,
    );
    if strict {
        let _ = upsert_hook(
            &mut hooks,
            "PostToolUse",
            Some("Edit|Write|MultiEdit"),
            &post_tool_use_hook_command(),
            POST_TOOL_USE_HOOK_TIMEOUT_SECS,
        );
    }
    let v = json!({"hooks": Value::Object(hooks)});
    match serde_json::to_string_pretty(&v) {
        Ok(text) => text,
        // Сериализация Value не падает; запасной вариант — компактная форма.
        Err(_) => v.to_string(),
    }
}

/// TOML-блок `[mcp_servers.spine]` для Codex (`~/.codex/config.toml`).
fn codex_toml_block(rw: bool) -> String {
    let args = if rw {
        "[\"mcp\", \"serve\", \"--rw\"]"
    } else {
        "[\"mcp\", \"serve\"]"
    };
    format!("[mcp_servers.spine]\ncommand = \"arch-be\"\nargs = {args}\n")
}

/// Мердж `[mcp_servers.spine]` в `~/.codex/config.toml` (`--apply-global`):
/// чужие секции сохраняются, перезаписывается только наша таблица; перед
/// первой перезаписью — бэкап. Пере-сериализация не сохраняет комментарии
/// и порядок ключей чужого файла — об этом заметка в отчёте.
fn merge_codex_config(
    path: &Path,
    rw: bool,
    dry_run: bool,
    report: &mut ConnectReport,
) -> Result<()> {
    let old = match std::fs::read_to_string(path) {
        Ok(text) => Some(text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(HarnessError::io(path, e)),
    };
    let mut doc: toml::Value = match old.as_deref() {
        None => toml::Value::Table(toml::map::Map::new()),
        Some(text) => toml::from_str(text).map_err(|e| {
            HarnessError::Config(format!(
                "{}: существующий config.toml не разбирается ({e}) — не затираю; \
                 разберите вручную",
                path.display()
            ))
        })?,
    };
    let Some(root) = doc.as_table_mut() else {
        return Err(HarnessError::Config(format!(
            "{}: верхний уровень не TOML-таблица — не затираю",
            path.display()
        )));
    };
    if !root.contains_key("mcp_servers") {
        root.insert(
            "mcp_servers".to_string(),
            toml::Value::Table(toml::map::Map::new()),
        );
    }
    let Some(servers) = root
        .get_mut("mcp_servers")
        .and_then(toml::Value::as_table_mut)
    else {
        return Err(HarnessError::Config(format!(
            "{}: секция mcp_servers не TOML-таблица — не затираю",
            path.display()
        )));
    };
    let foreign: Vec<String> = servers
        .keys()
        .filter(|k| k.as_str() != MCP_SERVER_NAME)
        .cloned()
        .collect();
    let mut spine = toml::map::Map::new();
    spine.insert(
        "command".to_string(),
        toml::Value::String("arch-be".to_string()),
    );
    let args = if rw {
        vec!["mcp", "serve", "--rw"]
    } else {
        vec!["mcp", "serve"]
    };
    spine.insert(
        "args".to_string(),
        toml::Value::Array(
            args.iter()
                .map(|a| toml::Value::String((*a).to_string()))
                .collect(),
        ),
    );
    servers.insert(MCP_SERVER_NAME.to_string(), toml::Value::Table(spine));
    if !foreign.is_empty() {
        report.notes.push(format!(
            "{}: существующие MCP-серверы сохранены: {}",
            path.display(),
            foreign.join(", ")
        ));
    }
    let new = toml::to_string_pretty(&doc)
        .map_err(|e| HarnessError::Config(format!("сериализация TOML: {e}")))?;
    if let Some(prev) = old.as_deref() {
        if prev != new {
            backup_once(path, prev, dry_run, report)?;
            report.notes.push(format!(
                "{} переписан сериализатором TOML: комментарии/порядок ключей не \
                 сохраняются (прежняя версия — в бэкапе)",
                path.display()
            ));
        }
    }
    commit_file(path, old.as_deref(), &new, dry_run, report)
}

/// Домашний конфиг хоста для `--apply-global` или ошибка.
fn global_config_path(opts: &ConnectOptions, rel: &str) -> Result<PathBuf> {
    opts.home.as_ref().map(|h| h.join(rel)).ok_or_else(|| {
        HarnessError::Config(
            "--apply-global: не удалось определить домашний каталог пользователя".into(),
        )
    })
}

/// `arch-be connect qwen`: мердж mcpServers в `.qwen/settings.json`
/// (Qwen Code — форк gemini-cli, ключ подтверждён); скиллы и хуки не
/// пишутся (layout не подтверждён) — сниппеты как у generic.
fn connect_qwen(opts: &ConnectOptions, report: &mut ConnectReport) -> Result<()> {
    merge_mcp_servers_json(
        &opts.dir.join(".qwen/settings.json"),
        opts.rw,
        false,
        opts.dry_run,
        report,
    )?;
    if opts.skills {
        report.notes.push(
            "скиллы не записаны: каталог скиллов Qwen Code не подтверждён — при \
             необходимости скопируйте вручную (см. сниппет ниже)"
                .into(),
        );
        report.snippets.push((
            "Скиллы (ручная установка)".to_string(),
            "встроенные скиллы раскладываются в ~/.arch-harness/plugins командой \
             `arch-be init` — скопируйте нужные `skills/<имя>/SKILL.md` в каталог \
             скиллов вашего хоста"
                .to_string(),
        ));
    }
    if opts.hooks {
        report.notes.push(
            "хуки не записаны: схема хуков Qwen Code не подтверждена — если \
             поддерживается, вставьте блок в .qwen/settings.json вручную"
                .into(),
        );
        report.snippets.push((
            "Хуки (формат Claude Code, референс)".to_string(),
            hooks_snippet(opts.strict_hooks),
        ));
    }
    report.next_steps.extend([
        "перезапустите Qwen Code (`qwen`) в этом каталоге".to_string(),
        "проверьте список MCP-серверов хоста — в нём «spine»".to_string(),
    ]);
    Ok(())
}

/// `arch-be connect codex`: MCP — пользовательский `~/.codex/config.toml`;
/// по умолчанию только печать TOML-блока (в дом молча не пишем).
fn connect_codex(opts: &ConnectOptions, report: &mut ConnectReport) -> Result<()> {
    if opts.apply_global {
        let path = global_config_path(opts, ".codex/config.toml")?;
        merge_codex_config(&path, opts.rw, opts.dry_run, report)?;
    } else {
        report.snippets.push((
            "MCP-сервер для Codex — добавьте в ~/.codex/config.toml:".to_string(),
            codex_toml_block(opts.rw),
        ));
        report.notes.push(
            "в дом пользователя без --apply-global не пишем (перезапуск с флагом \
             вписывает блок с мерджем и бэкапом)"
                .into(),
        );
    }
    if opts.agents_md && !opts.dir.join("AGENTS.md").is_file() {
        report.next_steps.push(
            "Codex нативно читает AGENTS.md: сгенерируйте его командой \
             `arch-be agents-md refresh .` (connect файл не создаёт — наименее \
             инвазивный вариант)"
                .to_string(),
        );
    }
    if opts.hooks {
        report.notes.push(
            "у Codex нет lifecycle-хуков уровня PreToolUse/Stop — fitness-гейт \
             остаётся ручным (`arch-be control check .`) или в CI"
                .into(),
        );
    }
    report.next_steps.extend([
        "перезапустите `codex` в этом каталоге".to_string(),
        "проверьте список MCP-серверов: `codex mcp list` — в нём «spine»".to_string(),
    ]);
    Ok(())
}

/// `arch-be connect kimi`: пишет проектный `.kimi-code/mcp.json` (мердж
/// `mcpServers.spine`; layout подтверждён официальной докой Kimi Code —
/// project-уровень действует только на текущий репозиторий и перекрывает
/// одноимённые user-level записи). Поле `cwd` не пишем: сервер наследует
/// рабочий каталог харнесса, как у остальных хостов. Плюс печать: блок для
/// ручной регистрации в user-level `~/.kimi-code/mcp.json` (запись туда с
/// мерджем и бэкапом — по `--apply-global`) и TOML-блок хука `[[hooks]]`
/// для `~/.kimi-code/config.toml` (проектных хуков у Kimi Code нет).
fn connect_kimi(opts: &ConnectOptions, report: &mut ConnectReport) -> Result<()> {
    merge_mcp_servers_json(
        &opts.dir.join(".kimi-code/mcp.json"),
        opts.rw,
        false,
        opts.dry_run,
        report,
    )?;
    if opts.apply_global {
        let path = global_config_path(opts, ".kimi-code/mcp.json")?;
        merge_mcp_servers_json(&path, opts.rw, true, opts.dry_run, report)?;
        report.notes.push(
            "записаны оба уровня; при совпадении имён проектная запись \
             .kimi-code/mcp.json перекрывает пользовательскую (дока Kimi Code)"
                .into(),
        );
    } else {
        report.snippets.push((
            "MCP-сервер для Kimi Code (альтернатива) — пользовательский уровень \
             ~/.kimi-code/mcp.json, общий для всех проектов (запись с мерджем и \
             бэкапом — перезапуском с --apply-global):"
                .to_string(),
            mcp_json_snippet(opts.rw),
        ));
    }
    if opts.hooks {
        report.snippets.push((
            "Хук-гейт для Kimi Code — добавьте в ~/.kimi-code/config.toml \
             (проектного уровня у хуков нет; схема подтверждена докой Kimi Code):"
                .to_string(),
            kimi_hooks_toml_block(),
        ));
    }
    if opts.skills {
        report.notes.push(
            "скиллы не разворачиваются: у Kimi Code своя библиотека скиллов \
             (extra_skill_dirs в ~/.kimi-code/config.toml) — добавьте каталог \
             ~/.arch-harness/plugins туда вручную при необходимости"
                .into(),
        );
    }
    report.next_steps.extend([
        "перезапустите Kimi Code (`kimi`) в этом каталоге".to_string(),
        "при первом запуске в каталоге Kimi Code покажет trust-диалог со списком \
         project-level MCP-серверов: project MCP не стартует в untrusted-папке \
         (штатная защита) — подтвердите «Trust this folder»"
            .to_string(),
        "проверьте подключение: команда `/mcp` в TUI — в списке сервер «spine»".to_string(),
        format!(
            "режим сервера: {}",
            if opts.rw {
                "rw (разрешены аддитивные записи: handoff_create, adr_new, …)"
            } else {
                "read-only"
            }
        ),
    ]);
    Ok(())
}

/// TOML-блок хука-гейта для `~/.kimi-code/config.toml` (Kimi Code): событие
/// `Stop`, та же команда с гардом, что пишется Claude Code. Схема
/// подтверждена официальной докой Kimi Code (customization/hooks):
/// массив `[[hooks]]` с полями `event`/`matcher`/`command`/`timeout`,
/// только пользовательский уровень; семантика exit-кодов совпадает с
/// Claude Code — 0 пропуск, 2 блок (stderr уходит модели), прочие сбои
/// fail-open; рабочий каталог команды — каталог проекта сессии.
fn kimi_hooks_toml_block() -> String {
    format!(
        "# Семантика: exit 0 — пропустить, exit 2 — блок (stderr уходит модели),\n\
         # прочие ошибки — fail-open. Команда выполняется в каталоге проекта.\n\
         [[hooks]]\n\
         event = \"Stop\"\n\
         command = \"{}\"\n\
         timeout = {}\n",
        toml_escape_basic(&stop_hook_command()),
        STOP_HOOK_TIMEOUT_SECS
    )
}

/// Экранирует строку для встраивания в TOML basic string (печать сниппета):
/// обратный слэш и двойная кавычка.
fn toml_escape_basic(text: &str) -> String {
    text.replace('\\', "\\\\").replace('"', "\\\"")
}

/// `arch-be connect omp` (oh-my-pi): проектный `.mcp.json` формата Claude
/// Desktop — omp дискаверит его автоматически, регистрация не нужна.
/// Скиллы omp читает нативно из `.claude/skills/`: если такого каталога в
/// проекте ещё нет, встроенные скиллы раскладываются туда тем же
/// `install_skills`, что у claude; каталог уже есть — не трогаем, чтобы не
/// перетирать чужую библиотеку (об этом заметка). Хуков через connect нет:
/// механизм хуков omp — TypeScript-расширения (`omp --hook <file.ts>`),
/// печатается указание.
fn connect_omp(opts: &ConnectOptions, report: &mut ConnectReport) -> Result<()> {
    merge_mcp_servers_json(
        &opts.dir.join(".mcp.json"),
        opts.rw,
        false,
        opts.dry_run,
        report,
    )?;
    report.notes.push(
        "omp автоматически дискаверит проектный .mcp.json — отдельная регистрация \
         сервера не нужна"
            .into(),
    );
    if opts.skills {
        let skills_root = opts.dir.join(".claude/skills");
        if skills_root.exists() {
            report.notes.push(format!(
                "{} уже существует — встроенные скиллы НЕ раскладываются (чужую \
                 библиотеку не перетираем; omp читает .claude/skills нативно, \
                 принудительно разложить встроенные может `arch-be connect claude`)",
                skills_root.display()
            ));
        } else {
            install_skills(&skills_root, opts.dry_run, report)?;
            report.notes.push(
                "скиллы разложены в .claude/skills/ — omp читает этот каталог \
                 нативно (тот же layout, что у Claude Code)"
                    .into(),
            );
        }
    }
    if opts.hooks {
        report.notes.push(
            "хуков через connect нет: механизм хуков omp — TypeScript-расширения, \
             подключение: `omp --hook <file.ts>`; команда-гейт для такого \
             расширения: `arch-be control check .`"
                .into(),
        );
    }
    report.next_steps.extend([
        "перезапустите omp в этом каталоге — сервер «spine» подхватится из .mcp.json".to_string(),
        format!(
            "инструменты видны агенту как инструменты сервера «spine»; режим: {}",
            if opts.rw {
                "rw (разрешены аддитивные записи: handoff_create, adr_new, …)"
            } else {
                "read-only"
            }
        ),
    ]);
    Ok(())
}

/// `arch-be connect generic`: ничего не пишет — печатает сниппеты и
/// инструкцию ручной установки.
fn connect_generic(opts: &ConnectOptions, report: &mut ConnectReport) {
    report.snippets.push((
        "MCP-сервер (формат `.mcp.json` Claude Code — его понимает большинство хостов):"
            .to_string(),
        mcp_json_snippet(opts.rw),
    ));
    if opts.hooks {
        report.snippets.push((
            "Хуки (формат settings.json Claude Code; exit 2 = блок, stderr уходит агенту):"
                .to_string(),
            hooks_snippet(opts.strict_hooks),
        ));
    }
    if opts.skills {
        report.snippets.push((
            "Скиллы".to_string(),
            "встроенные скиллы раскладываются в ~/.arch-harness/plugins командой \
             `arch-be init` — скопируйте нужные `skills/<имя>/SKILL.md` в каталог \
             скиллов вашего хоста"
                .to_string(),
        ));
    }
    if opts.agents_md {
        report.snippets.push((
            "AGENTS.md".to_string(),
            "сгенерируйте точку входа агента: `arch-be agents-md refresh .` \
             (рукописная зона сохраняется)"
                .to_string(),
        ));
    }
    report
        .notes
        .push("generic ничего не записывает — только печать".into());
    report.next_steps.extend([
        "вставьте сниппеты выше в конфигурацию вашего агента".to_string(),
        "перезапустите агента и проверьте, что MCP-сервер «spine» поднялся".to_string(),
    ]);
}

/// Рендерит отчёт команды по-русски: файлы, скиллы, заметки, сниппеты,
/// следующие шаги.
#[must_use]
pub fn render_report(opts: &ConnectOptions, report: &ConnectReport) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "Подключение Spine к хосту «{}» — {}",
        opts.host.name(),
        opts.dir.display()
    );
    if report.dry_run {
        let _ = writeln!(out, "(dry-run: ничего не записано — ниже план)");
    }
    out.push('\n');
    if !report.created.is_empty() {
        out.push_str("Записано:\n");
        for p in &report.created {
            let _ = writeln!(out, "  + {}", p.display());
        }
    }
    if !report.merged.is_empty() {
        out.push_str("Смерджено (чужие ключи сохранены):\n");
        for p in &report.merged {
            let _ = writeln!(out, "  ~ {}", p.display());
        }
    }
    if !report.unchanged.is_empty() {
        out.push_str("Без изменений:\n");
        for p in &report.unchanged {
            let _ = writeln!(out, "  = {}", p.display());
        }
    }
    if !report.skills.is_empty() {
        let total = report.skills.len();
        let _ = writeln!(out, "Скиллы ({total}):");
        for s in &report.skills {
            let line = match &s.action {
                SkillAction::Copied(n) => format!("скопирован ({n} файлов)"),
                SkillAction::Updated(n) => format!("обновлён встроенной версией ({n} файлов)"),
                SkillAction::Unchanged(n) => format!("без изменений ({n} файлов)"),
            };
            let _ = writeln!(out, "  ✓ {} — {line}", s.name);
        }
    }
    if !report.skills_skipped.is_empty() {
        out.push_str("Пропущены файлы скиллов:\n");
        for s in &report.skills_skipped {
            let _ = writeln!(out, "  ! {s}");
        }
    }
    if !report.notes.is_empty() {
        out.push_str("Заметки:\n");
        for n in &report.notes {
            let _ = writeln!(out, "  - {n}");
        }
    }
    for (title, text) in &report.snippets {
        let _ = writeln!(out, "\n── {title}\n{text}");
    }
    if !report.next_steps.is_empty() {
        out.push_str("\nСледующие шаги:\n");
        for (i, step) in report.next_steps.iter().enumerate() {
            let _ = writeln!(out, "  {}. {step}", i + 1);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Читает файл в строку (тестовый хелпер).
    fn read(path: &Path) -> String {
        std::fs::read_to_string(path).expect("read")
    }

    /// Снимок дерева каталога: отсортированные (путь, содержимое).
    fn snapshot(root: &Path) -> Vec<(PathBuf, String)> {
        let mut out = Vec::new();
        for entry in walkdir::WalkDir::new(root).sort_by_file_name() {
            let entry = entry.expect("walkdir");
            if entry.file_type().is_file() {
                out.push((
                    entry.path().strip_prefix(root).expect("rel").to_path_buf(),
                    std::fs::read_to_string(entry.path()).expect("read"),
                ));
            }
        }
        out
    }

    /// Число хуков с нашим маркером в событии settings.json.
    fn count_marked_hooks(settings: &Value, event: &str) -> usize {
        settings["hooks"][event].as_array().map_or(0, |entries| {
            entries
                .iter()
                .filter(|g| {
                    g["hooks"].as_array().is_some_and(|cmds| {
                        cmds.iter().any(|c| {
                            c["command"]
                                .as_str()
                                .is_some_and(|cmd| cmd.contains(HOOK_MARKER))
                        })
                    })
                })
                .count()
        })
    }

    /// (a) connect claude в пустой каталог: .mcp.json, settings.json,
    /// CLAUDE.md и скиллы на месте.
    #[test]
    fn claude_scaffolds_empty_dir() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path().join("proj");
        let report = connect(&ConnectOptions::new(Host::Claude, dir.clone())).expect("connect");

        let mcp: Value = serde_json::from_str(&read(&dir.join(".mcp.json"))).expect("json");
        assert_eq!(mcp["mcpServers"]["spine"]["command"], "arch-be");
        assert_eq!(mcp["mcpServers"]["spine"]["args"], json!(["mcp", "serve"]));

        let settings: Value =
            serde_json::from_str(&read(&dir.join(".claude/settings.json"))).expect("json");
        assert_eq!(count_marked_hooks(&settings, "Stop"), 1, "один Stop-хук");
        assert!(
            settings["hooks"].get("PostToolUse").is_none(),
            "PostToolUse — только под --strict-hooks"
        );

        let claude_md = read(&dir.join("CLAUDE.md"));
        assert!(claude_md.contains(SPINE_BEGIN));
        assert!(claude_md.contains("AGENTS.md"));

        let skill = dir.join(".claude/skills/adr-authoring/SKILL.md");
        assert!(skill.is_file(), "скилл разложен: {}", skill.display());
        assert!(read(&skill).starts_with("---\n"), "frontmatter на месте");
        // Скилл с references/ переносится целиком.
        assert!(
            dir.join(".claude/skills/adr-authoring/references/adr-template.md")
                .is_file(),
            "references скилла переносятся"
        );
        assert!(
            report.skills.len() >= 40,
            "скиллов: {}",
            report.skills.len()
        );
        assert!(
            report.skills_skipped.is_empty(),
            "{:?}",
            report.skills_skipped
        );
        assert!(
            report.created.iter().any(|p| p.ends_with(".mcp.json")),
            "{:?}",
            report.created
        );
    }

    /// (b) Повторный запуск: дерево побайтово идентично, хуки не дублируются.
    #[test]
    fn claude_second_run_is_byte_identical() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path().join("proj");
        let opts = ConnectOptions::new(Host::Claude, dir.clone());
        connect(&opts).expect("первый прогон");
        let first = snapshot(&dir);
        let report = connect(&opts).expect("второй прогон");
        let second = snapshot(&dir);
        assert_eq!(first, second, "повторный запуск изменил файлы");
        assert!(
            !report.unchanged.is_empty(),
            "второй прогон — всё «без изменений»"
        );
        assert!(report.merged.is_empty() && report.created.is_empty());

        let settings: Value =
            serde_json::from_str(&read(&dir.join(".claude/settings.json"))).expect("json");
        assert_eq!(count_marked_hooks(&settings, "Stop"), 1, "без дублей хуков");

        // Эскалация до --strict-hooks добавляет PostToolUse и не трогает Stop.
        let strict = ConnectOptions {
            strict_hooks: true,
            ..ConnectOptions::new(Host::Claude, dir.clone())
        };
        connect(&strict).expect("strict прогон");
        let settings: Value =
            serde_json::from_str(&read(&dir.join(".claude/settings.json"))).expect("json");
        assert_eq!(count_marked_hooks(&settings, "Stop"), 1);
        assert_eq!(count_marked_hooks(&settings, "PostToolUse"), 1);
        connect(&strict).expect("повторный strict");
        let settings: Value =
            serde_json::from_str(&read(&dir.join(".claude/settings.json"))).expect("json");
        assert_eq!(
            count_marked_hooks(&settings, "PostToolUse"),
            1,
            "дубль strict"
        );
    }

    /// (c) Мердж: чужой сервер в .mcp.json и чужой хук в settings.json
    /// сохраняются, наши добавляются.
    #[test]
    fn merge_preserves_foreign_servers_and_hooks() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path().join("proj");
        std::fs::create_dir_all(dir.join(".claude")).expect("mkdir");
        std::fs::write(
            dir.join(".mcp.json"),
            "{\n  \"mcpServers\": {\n    \"other\": {\"command\": \"uvx\", \"args\": [\"x\"]}\n  },\n  \"foreignTop\": true\n}\n",
        )
        .expect("write .mcp.json");
        std::fs::write(
            dir.join(".claude/settings.json"),
            "{\n  \"model\": \"opus\",\n  \"hooks\": {\n    \"Stop\": [\n      {\"hooks\": [{\"type\": \"command\", \"command\": \"echo mine\"}]}\n    ]\n  }\n}\n",
        )
        .expect("write settings.json");

        let report = connect(&ConnectOptions::new(Host::Claude, dir.clone())).expect("connect");

        let mcp: Value = serde_json::from_str(&read(&dir.join(".mcp.json"))).expect("json");
        assert_eq!(
            mcp["mcpServers"]["other"]["command"], "uvx",
            "чужой сервер цел"
        );
        assert_eq!(mcp["foreignTop"], json!(true), "чужой верхний ключ цел");
        assert_eq!(mcp["mcpServers"]["spine"]["command"], "arch-be");

        let settings: Value =
            serde_json::from_str(&read(&dir.join(".claude/settings.json"))).expect("json");
        assert_eq!(settings["model"], "opus", "чужой ключ settings цел");
        let stop = settings["hooks"]["Stop"].as_array().expect("массив Stop");
        assert_eq!(stop.len(), 2, "чужой хук + наш: {stop:?}");
        assert_eq!(stop[0]["hooks"][0]["command"], "echo mine", "чужой хук цел");
        assert_eq!(count_marked_hooks(&settings, "Stop"), 1);
        assert!(
            report
                .notes
                .iter()
                .any(|n| n.contains("сохранены") && n.contains("other")),
            "заметка о чужих серверах: {:?}",
            report.notes
        );
        assert!(
            report.merged.iter().any(|p| p.ends_with(".mcp.json")),
            "{:?}",
            report.merged
        );
    }

    /// Битый существующий JSON — отказ без затирания.
    #[test]
    fn broken_existing_json_refuses_overwrite() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path().join("proj");
        std::fs::create_dir_all(&dir).expect("mkdir");
        std::fs::write(dir.join(".mcp.json"), "{это не json").expect("write");
        let err = connect(&ConnectOptions::new(Host::Claude, dir.clone()))
            .expect_err("должна быть ошибка");
        assert!(err.to_string().contains("не затираю"), "{err}");
        assert_eq!(read(&dir.join(".mcp.json")), "{это не json", "файл цел");
    }

    /// (d) --dry-run: только план, файловая система не трогается.
    #[test]
    fn dry_run_writes_nothing() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path().join("proj");
        let opts = ConnectOptions {
            dry_run: true,
            ..ConnectOptions::new(Host::Claude, dir.clone())
        };
        let report = connect(&opts).expect("connect dry-run");
        assert!(!report.created.is_empty(), "план непустой");
        assert!(!dir.exists(), "dry-run даже каталог не создаёт");

        // И на существующем дереве — ни одной правки.
        connect(&ConnectOptions::new(Host::Claude, dir.clone())).expect("реальный прогон");
        let before = snapshot(&dir);
        let opts = ConnectOptions {
            dry_run: true,
            strict_hooks: true, // план шире реальности — всё равно ничего не пишет
            ..ConnectOptions::new(Host::Claude, dir.clone())
        };
        connect(&opts).expect("dry-run поверх");
        assert_eq!(before, snapshot(&dir), "dry-run что-то записал");
    }

    /// (e) codex/generic без --apply-global — только печать: ни в проект,
    /// ни в дом ничего не пишется (kimi с подтверждённым проектным layout
    /// таким больше не является — см. `kimi_writes_project_mcp_json`).
    #[test]
    fn codex_generic_are_print_only_by_default() {
        for host in [Host::Codex, Host::Generic] {
            let tmp = tempfile::tempdir().expect("tmp");
            let dir = tmp.path().join("proj");
            let home = tmp.path().join("home");
            std::fs::create_dir_all(&dir).expect("mkdir");
            let opts = ConnectOptions {
                home: Some(home.clone()),
                ..ConnectOptions::new(host, dir.clone())
            };
            let report = connect(&opts).expect("connect");
            assert_eq!(
                snapshot(&dir),
                Vec::new(),
                "{}: в проект ничего не пишется",
                host.name()
            );
            assert!(!home.exists(), "{}: дом не трогается", host.name());
            assert!(
                !report.snippets.is_empty(),
                "{}: печатаются сниппеты",
                host.name()
            );
        }
        // generic печатает и mcp-сниппет, и хуки.
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path().join("proj");
        let report = connect(&ConnectOptions::new(Host::Generic, dir)).expect("connect");
        let titles: Vec<&str> = report.snippets.iter().map(|(t, _)| t.as_str()).collect();
        assert!(titles.iter().any(|t| t.contains("MCP")), "{titles:?}");
        assert!(titles.iter().any(|t| t.contains("Хуки")), "{titles:?}");
    }

    /// (f) --rw добавляет "--rw" в args MCP-сервера (запись и сниппеты).
    #[test]
    fn rw_flag_extends_mcp_args() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path().join("proj");
        let opts = ConnectOptions {
            rw: true,
            ..ConnectOptions::new(Host::Claude, dir.clone())
        };
        connect(&opts).expect("connect");
        let mcp: Value = serde_json::from_str(&read(&dir.join(".mcp.json"))).expect("json");
        assert_eq!(
            mcp["mcpServers"]["spine"]["args"],
            json!(["mcp", "serve", "--rw"])
        );
        // kimi: --rw в проектном .kimi-code/mcp.json и в user-level сниппете.
        let kimi_dir = tmp.path().join("proj-kimi");
        let opts = ConnectOptions {
            rw: true,
            ..ConnectOptions::new(Host::Kimi, kimi_dir.clone())
        };
        let report = connect(&opts).expect("kimi");
        let mcp: Value =
            serde_json::from_str(&read(&kimi_dir.join(".kimi-code/mcp.json"))).expect("json");
        assert_eq!(
            mcp["mcpServers"]["spine"]["args"],
            json!(["mcp", "serve", "--rw"])
        );
        assert!(
            report.snippets.iter().any(|(_, s)| s.contains("--rw")),
            "{:?}",
            report.snippets
        );
        // omp: --rw в .mcp.json.
        let omp_dir = tmp.path().join("proj-omp");
        let opts = ConnectOptions {
            rw: true,
            ..ConnectOptions::new(Host::Omp, omp_dir.clone())
        };
        connect(&opts).expect("omp");
        let mcp: Value = serde_json::from_str(&read(&omp_dir.join(".mcp.json"))).expect("json");
        assert_eq!(
            mcp["mcpServers"]["spine"]["args"],
            json!(["mcp", "serve", "--rw"])
        );
        // В сниппетах codex тоже.
        let opts = ConnectOptions {
            rw: true,
            ..ConnectOptions::new(Host::Codex, tmp.path().join("p2"))
        };
        let report = connect(&opts).expect("codex");
        assert!(
            report.snippets.iter().any(|(_, s)| s.contains("--rw")),
            "{:?}",
            report.snippets
        );
    }

    /// qwen: пишется только .qwen/settings.json (мердж mcpServers);
    /// скиллы/хуки — сниппеты и заметки.
    #[test]
    fn qwen_writes_settings_json_only() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path().join("proj");
        let report = connect(&ConnectOptions::new(Host::Qwen, dir.clone())).expect("connect");
        let settings: Value =
            serde_json::from_str(&read(&dir.join(".qwen/settings.json"))).expect("json");
        assert_eq!(settings["mcpServers"]["spine"]["command"], "arch-be");
        assert!(!dir.join(".claude").exists(), "каталога .claude нет");
        assert!(
            report.notes.iter().any(|n| n.contains("не подтверждён")),
            "заметки: {:?}",
            report.notes
        );
        assert!(!report.snippets.is_empty(), "печатаются сниппеты");
        // Идемпотентность.
        connect(&ConnectOptions::new(Host::Qwen, dir.clone())).expect("повтор");
        let again: Value =
            serde_json::from_str(&read(&dir.join(".qwen/settings.json"))).expect("json");
        assert_eq!(settings, again);
    }

    /// --apply-global: codex — мердж в ~/.codex/config.toml с бэкапом и без
    /// дублей; kimi — мердж в ~/.kimi-code/mcp.json.
    #[test]
    fn apply_global_merges_home_configs_with_backup() {
        let tmp = tempfile::tempdir().expect("tmp");
        let home = tmp.path().join("home");
        std::fs::create_dir_all(home.join(".codex")).expect("mkdir");
        let cfg = home.join(".codex/config.toml");
        std::fs::write(
            &cfg,
            "model = \"gpt-5\"\n\n[mcp_servers.other]\ncommand = \"uvx\"\n",
        )
        .expect("write config.toml");
        let opts = ConnectOptions {
            apply_global: true,
            home: Some(home.clone()),
            ..ConnectOptions::new(Host::Codex, tmp.path().join("proj"))
        };
        connect(&opts).expect("apply codex");
        let text = read(&cfg);
        assert!(text.contains("[mcp_servers.spine]"), "{text}");
        assert!(
            text.contains("[mcp_servers.other]"),
            "чужая секция цела: {text}"
        );
        assert!(text.contains("model = \"gpt-5\""), "чужой ключ цел: {text}");
        let backup = home.join(".codex/config.toml.bak-spine-connect");
        assert!(backup.is_file(), "бэкап создан");
        assert!(read(&backup).contains("gpt-5"), "бэкап — прежнее");
        // Повтор — без дублей и без нового бэкапа.
        let first = read(&cfg);
        connect(&opts).expect("повтор codex");
        assert_eq!(first, read(&cfg), "повторный apply_global идемпотентен");

        // kimi: JSON-мердж в ~/.kimi-code/mcp.json + проектный файл тоже
        // записывается (проектный уровень перекрывает пользовательский).
        let kimi_proj = tmp.path().join("proj-kimi");
        let opts = ConnectOptions {
            apply_global: true,
            home: Some(home.clone()),
            ..ConnectOptions::new(Host::Kimi, kimi_proj.clone())
        };
        connect(&opts).expect("apply kimi");
        let mcp: Value =
            serde_json::from_str(&read(&home.join(".kimi-code/mcp.json"))).expect("json");
        assert_eq!(mcp["mcpServers"]["spine"]["command"], "arch-be");
        let mcp: Value =
            serde_json::from_str(&read(&kimi_proj.join(".kimi-code/mcp.json"))).expect("json");
        assert_eq!(mcp["mcpServers"]["spine"]["command"], "arch-be");
    }

    /// Сборка скиллов из встроенных плагинов: layout, потолок размера,
    /// дубли путей, страховка frontmatter.
    #[test]
    fn collect_skill_files_layout_ceiling_and_frontmatter() {
        let big = "x".repeat(MAX_SKILL_FILE_BYTES + 1);
        let files: Vec<(&str, &str)> = vec![
            (
                "plugins/p1/skills/alpha/SKILL.md",
                "---\nname: alpha\ndescription: А\n---\n\n# A\n",
            ),
            ("plugins/p1/skills/alpha/references/r.md", "# ref\n"),
            ("plugins/p1/plugin.json", "{}"),
            (
                "plugins/p2/skills/beta/SKILL.md",
                "# Без фронтматтера\n\nТело.\n",
            ),
            ("plugins/p3/skills/big/SKILL.md", big.as_str()),
            (
                "plugins/p2/skills/alpha/SKILL.md",
                "---\nname: alpha\ndescription: Дубль\n---\n",
            ),
        ];
        let (out, skipped) = collect_skill_files_from(&files);
        let paths: Vec<&str> = out.iter().map(|(p, _)| p.to_str().expect("utf8")).collect();
        assert_eq!(
            paths,
            vec!["alpha/SKILL.md", "alpha/references/r.md", "beta/SKILL.md"],
            "{paths:?}"
        );
        assert_eq!(skipped.len(), 2, "большой + дубль: {skipped:?}");
        // Скилл без frontmatter получает минимальный (имя — из каталога).
        let beta = out
            .iter()
            .find(|(p, _)| p == &PathBuf::from("beta/SKILL.md"))
            .map(|(_, c)| c)
            .expect("beta");
        assert!(
            beta.starts_with("---\nname: beta\ndescription: Без фронтматтера\n"),
            "{beta}"
        );
        // Валидный frontmatter не трогается.
        let alpha = &out[0].1;
        assert!(alpha.starts_with("---\nname: alpha\n"), "{alpha}");
    }

    /// Скилл с локальной правкой обновляется встроенной версией (с заметкой),
    /// совпадающий — помечается «без изменений».
    #[test]
    fn skills_update_reports_overwritten_local_edit() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path().join("proj");
        connect(&ConnectOptions::new(Host::Claude, dir.clone())).expect("первый");
        let edited = dir.join(".claude/skills/adr-authoring/SKILL.md");
        std::fs::write(
            &edited,
            "---\nname: adr-authoring\ndescription: локальная правка\n---\n",
        )
        .expect("правка");
        let report = connect(&ConnectOptions::new(Host::Claude, dir.clone())).expect("второй");
        let outcome = report
            .skills
            .iter()
            .find(|s| s.name == "adr-authoring")
            .expect("скилл в отчёте");
        assert_eq!(outcome.action, SkillAction::Updated(1), "{outcome:?}");
        assert!(
            report.notes.iter().any(|n| n.contains("adr-authoring")),
            "{:?}",
            report.notes
        );
        assert!(
            read(&edited).contains("AI-DLC"),
            "встроенная версия на месте"
        );
        // А нетронутые скиллы — «без изменений».
        assert!(
            report
                .skills
                .iter()
                .any(|s| s.name == "bulkhead" && s.action == SkillAction::Unchanged(1)),
            "{:?}",
            report.skills
        );
    }

    /// CLAUDE.md: блок дописывается в чужой файл, при повторе — заменяется,
    /// рукописное цело.
    #[test]
    fn claude_md_block_splices_and_preserves_handwritten() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path().join("proj");
        std::fs::create_dir_all(&dir).expect("mkdir");
        std::fs::write(
            dir.join("CLAUDE.md"),
            "# Наш проект\n\nРукописные договорённости команды.\n",
        )
        .expect("write");
        connect(&ConnectOptions::new(Host::Claude, dir.clone())).expect("первый");
        let text = read(&dir.join("CLAUDE.md"));
        assert!(text.contains("Рукописные договорённости"), "{text}");
        assert_eq!(text.matches(SPINE_BEGIN).count(), 1, "{text}");

        // Правка внутри блока затирается, снаружи — нет.
        let modified = text.replace(SPINE_END, "шум внутри блока\n<!-- SPINE:END -->");
        std::fs::write(dir.join("CLAUDE.md"), modified).expect("правка блока");
        connect(&ConnectOptions::new(Host::Claude, dir.clone())).expect("второй");
        let text = read(&dir.join("CLAUDE.md"));
        assert!(!text.contains("шум внутри блока"), "{text}");
        assert!(text.contains("Рукописные договорённости"), "{text}");
        assert_eq!(text.matches(SPINE_BEGIN).count(), 1);
    }

    /// Команды хуков: гард по бинарю и CONSTRAINTS.yaml, маркер, exit 2.
    #[test]
    fn hook_commands_are_guarded_and_marked() {
        for cmd in [stop_hook_command(), post_tool_use_hook_command()] {
            assert!(cmd.contains("command -v arch-be"), "{cmd}");
            assert!(cmd.contains(".arch-handoff/CONSTRAINTS.yaml"), "{cmd}");
            assert!(cmd.contains("Итог: FAIL"), "{cmd}");
            assert!(cmd.contains("exit 2"), "{cmd}");
            assert!(cmd.contains(HOOK_MARKER), "{cmd}");
        }
        assert!(stop_hook_command().contains("# spine-connect:stop"));
        assert!(post_tool_use_hook_command().contains("# spine-connect:post-tool-use"));
    }

    /// kimi: пишется проектный `.kimi-code/mcp.json` (без поля cwd), дом не
    /// трогается без --apply-global; печатаются user-level сниппет, TOML-блок
    /// хука и напоминание про trust-диалог. Мердж чужих серверов, отказ на
    /// битом JSON, идемпотентность — как у claude `.mcp.json`.
    #[test]
    fn kimi_writes_project_mcp_json_with_merge_and_idempotency() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path().join("proj");
        let home = tmp.path().join("home");
        std::fs::create_dir_all(dir.join(".kimi-code")).expect("mkdir");
        std::fs::write(
            dir.join(".kimi-code/mcp.json"),
            "{\n  \"mcpServers\": {\n    \"other\": {\"command\": \"uvx\", \"args\": [\"x\"]}\n  }\n}\n",
        )
        .expect("write mcp.json");
        let opts = ConnectOptions {
            home: Some(home.clone()),
            ..ConnectOptions::new(Host::Kimi, dir.clone())
        };
        let report = connect(&opts).expect("connect");

        let mcp: Value =
            serde_json::from_str(&read(&dir.join(".kimi-code/mcp.json"))).expect("json");
        assert_eq!(mcp["mcpServers"]["spine"]["command"], "arch-be");
        assert_eq!(mcp["mcpServers"]["spine"]["args"], json!(["mcp", "serve"]));
        assert!(
            mcp["mcpServers"]["spine"].get("cwd").is_none(),
            "поле cwd не пишем — сервер наследует cwd харнесса: {mcp}"
        );
        assert_eq!(
            mcp["mcpServers"]["other"]["command"], "uvx",
            "чужой сервер цел"
        );
        assert!(
            report
                .notes
                .iter()
                .any(|n| n.contains("сохранены") && n.contains("other")),
            "заметка о чужих серверах: {:?}",
            report.notes
        );
        assert!(!home.exists(), "дом не трогается без --apply-global");
        // Печать: user-level альтернатива, TOML-блок хука, trust-диалог.
        let all_snippets = report
            .snippets
            .iter()
            .map(|(t, s)| format!("{t}\n{s}"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            all_snippets.contains("~/.kimi-code/mcp.json"),
            "user-level сниппет: {:?}",
            report.snippets
        );
        assert!(all_snippets.contains("[[hooks]]"), "TOML-блок хука");
        assert!(all_snippets.contains("event = \"Stop\""), "{all_snippets}");
        assert!(
            all_snippets.contains("~/.kimi-code/config.toml"),
            "{all_snippets}"
        );
        assert!(
            all_snippets.contains("arch-be control check ."),
            "{all_snippets}"
        );
        assert!(
            report.next_steps.iter().any(|s| s.contains("trust")),
            "напоминание про trust-диалог: {:?}",
            report.next_steps
        );

        // Идемпотентность: повторный запуск побайтово тот же, без дублей.
        let first = snapshot(&dir);
        let report = connect(&opts).expect("повтор");
        assert_eq!(first, snapshot(&dir), "повторный запуск изменил файлы");
        assert!(report.merged.is_empty() && report.created.is_empty());

        // Битый JSON — отказ без затирания.
        std::fs::write(dir.join(".kimi-code/mcp.json"), "{это не json").expect("write");
        let err = connect(&opts).expect_err("должна быть ошибка");
        assert!(err.to_string().contains("не затираю"), "{err}");
        assert_eq!(
            read(&dir.join(".kimi-code/mcp.json")),
            "{это не json",
            "файл цел"
        );
    }

    /// kimi --dry-run: ни проектный файл, ни дом не трогаются.
    #[test]
    fn kimi_dry_run_writes_nothing() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path().join("proj");
        let opts = ConnectOptions {
            dry_run: true,
            home: Some(tmp.path().join("home")),
            ..ConnectOptions::new(Host::Kimi, dir.clone())
        };
        let report = connect(&opts).expect("dry-run");
        assert!(!report.created.is_empty(), "план непустой");
        assert!(!dir.exists(), "dry-run ничего не записал");
        assert!(!tmp.path().join("home").exists(), "дом не трогается");
    }

    /// omp: пишется только `.mcp.json` + скиллы в `.claude/skills/` (omp
    /// читает его нативно); повторный запуск без дублей; существующий
    /// каталог `.claude/skills` не трогается; --dry-run ничего не пишет.
    #[test]
    fn omp_writes_mcp_json_and_skills() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path().join("proj");
        let report = connect(&ConnectOptions::new(Host::Omp, dir.clone())).expect("connect");

        let mcp: Value = serde_json::from_str(&read(&dir.join(".mcp.json"))).expect("json");
        assert_eq!(mcp["mcpServers"]["spine"]["command"], "arch-be");
        assert!(
            dir.join(".claude/skills/adr-authoring/SKILL.md").is_file(),
            "скиллы разложены в .claude/skills"
        );
        assert!(
            !dir.join(".claude/settings.json").exists(),
            "хуки не пишутся"
        );
        assert!(!dir.join("CLAUDE.md").exists(), "CLAUDE.md не создаётся");
        assert!(
            report.notes.iter().any(|n| n.contains("дискаверит")),
            "заметка про автодискавери: {:?}",
            report.notes
        );
        assert!(
            report.notes.iter().any(|n| n.contains("omp --hook")),
            "указание на TS-хуки: {:?}",
            report.notes
        );

        // Повтор: дерево побайтово то же (каталог скиллов уже есть —
        // скиллы не перетираются, .mcp.json мержится ключ-в-ключ).
        let first = snapshot(&dir);
        let report = connect(&ConnectOptions::new(Host::Omp, dir.clone())).expect("повтор");
        assert_eq!(first, snapshot(&dir), "повторный запуск изменил файлы");
        assert!(report.created.is_empty() && report.merged.is_empty());
        assert!(
            report
                .notes
                .iter()
                .any(|n| n.contains("уже существует") && n.contains("НЕ раскладываются")),
            "скип существующего каталога скиллов: {:?}",
            report.notes
        );

        // Существующий (даже пустой) .claude/skills не наполняется.
        let dir2 = tmp.path().join("proj2");
        std::fs::create_dir_all(dir2.join(".claude/skills")).expect("mkdir");
        let report = connect(&ConnectOptions::new(Host::Omp, dir2.clone())).expect("connect");
        assert!(report.skills.is_empty(), "скиллы не раскладывались");
        assert_eq!(
            std::fs::read_dir(dir2.join(".claude/skills"))
                .expect("read dir")
                .count(),
            0,
            "чужой каталог скиллов не тронут"
        );
        assert!(dir2.join(".mcp.json").is_file(), ".mcp.json записан");

        // --dry-run: ничего не пишется.
        let dir3 = tmp.path().join("proj3");
        let opts = ConnectOptions {
            dry_run: true,
            ..ConnectOptions::new(Host::Omp, dir3.clone())
        };
        let report = connect(&opts).expect("dry-run");
        assert!(!report.created.is_empty(), "план непустой");
        assert!(!report.skills.is_empty(), "план по скиллам непустой");
        assert!(!dir3.exists(), "dry-run ничего не записал");
    }

    /// Разбор имён хостов: алиасы, регистр, ошибка со списком допустимых.
    #[test]
    fn host_parse_accepts_aliases_and_rejects_unknown() {
        assert_eq!(Host::parse("claude"), Ok(Host::Claude));
        assert_eq!(Host::parse("Claude-Code"), Ok(Host::Claude));
        assert_eq!(Host::parse("qwen"), Ok(Host::Qwen));
        assert_eq!(Host::parse("kimi-code"), Ok(Host::Kimi));
        assert_eq!(Host::parse("omp"), Ok(Host::Omp));
        assert_eq!(Host::parse("oh-my-pi"), Ok(Host::Omp));
        assert_eq!(Host::parse("generic"), Ok(Host::Generic));
        let err = Host::parse("cursor").expect_err("неизвестный хост");
        assert!(err.contains("claude"), "{err}");
        assert!(err.contains("omp"), "{err}");
    }

    /// Рендер отчёта: русские секции, dry-run помечен.
    #[test]
    fn render_report_in_russian_marks_dry_run() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path().join("proj");
        let opts = ConnectOptions {
            dry_run: true,
            ..ConnectOptions::new(Host::Claude, dir)
        };
        let report = connect(&opts).expect("connect");
        let text = render_report(&opts, &report);
        assert!(
            text.contains("Подключение Spine к хосту «claude»"),
            "{text}"
        );
        assert!(text.contains("dry-run"), "{text}");
        assert!(text.contains("Следующие шаги"), "{text}");
        assert!(text.contains("claude mcp list"), "{text}");
    }
}
