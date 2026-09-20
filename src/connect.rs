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
//!   повторном запуске), `.claude/skills/<имя>/` (скиллы: источник истины —
//!   библиотека пользователя из `[plugins].dirs` конфига, включая плагины
//!   без манифеста и правки пользователя; встроенные плагины
//!   [`crate::assets::embedded_plugin_files`] — fallback для скиллов,
//!   которых на диске нет, т.е. работает и из релизного бинаря без
//!   `arch-be init`), `CLAUDE.md`
//!   (создаётся краткий либо блок между маркерами
//!   `<!-- SPINE:BEGIN -->`/`<!-- SPINE:END -->`; рукописное не затирается);
//! - `qwen`: `.qwen/settings.json` (мердж `mcpServers`; Qwen Code — форк
//!   gemini-cli, ключ `mcpServers` верхнего уровня подтверждён) + скиллы в
//!   `.qwen/skills/` (нативный project-scope в qwen-code ≥ 0.24; существующий
//!   каталог скиллов не перетирается). Хуки НЕ пишутся: схема хуков Qwen Code
//!   не подтверждена — печатается сниппет-референс;
//! - `gigacode` (`GigaCode` — форк Qwen Code): автоопределение каталога
//!   настроек проекта — существующий `.gigacode/`; иначе существующий
//!   `.qwen/` (совместимость layout форка); иначе создаётся `.gigacode/`.
//!   Дальше механика qwen (мердж `mcpServers.spine` в `<каталог>/settings.json`),
//!   скиллы — как у `claude` (раскладка в `<каталог>/skills/` с обновлением
//!   версий источника), хуки — печать сниппета (как у qwen);
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
//!   скиллы библиотеки раскладываются туда же, как у claude; каталог уже
//!   есть — не трогаем (чужую библиотеку не перетираем). Хуков через
//!   connect нет: механизм хуков omp — TypeScript-расширения, печатается
//!   указание на `omp --hook <file.ts>`;
//! - `generic`: только печать — сниппеты `.mcp.json` и хуков, куда что
//!   вставить вручную.
//!
//! Особые значения host — не агенты, а гейты, не зависящие от хоста
//! (бэклог волны 2, п.8: хуки ненадёжны — у qwen headless-файринг не
//! подтверждён, у Codex lifecycle-хуков нет):
//! - `ci` (`--provider gitlab|github|jenkins`): готовая джоба архитектурного
//!   гейта — `.gitlab-ci.yml` (мердж-блок между маркерами
//!   `# spine-connect:begin/end`, чужое не затирается; нарушения видны в
//!   интерфейсе merge request из артефакта `reports.codequality`),
//!   `.github/workflows/spine-gate.yml` (новый файл; существующий без
//!   маркера — отказ), `Jenkinsfile` (мердж-блок, `junit(...)`). Джоба
//!   запускает `arch-be gate --route auto --format <нативный формат
//!   площадки>`; установка бинаря — curl из релизов (linux-x86_64) или
//!   офлайн-бандл (оба варианта — в комментарии джобы);
//! - `git-hooks`: `.git/hooks/pre-commit` (быстрый `arch-be control check .`)
//!   и `pre-push` (полный `arch-be gate --route auto`); блоки между маркерами,
//!   идемпотентно, чужие хуки не затираются; оба хука fail-soft — при
//!   отсутствии `arch-be` в PATH молча пропускаются, как хуки connect.
//!   Расположение реестра правил в шаблонах не зашито (T-01): его резолвит
//!   бинарь — корень кейса, затем `.arch-handoff/`.
//!
//! Хуки (для `claude` — запись в `settings.json`; для остальных — печать).
//! События Claude Code: `Stop` (дефолт) и `PostToolUse` с matcher
//! `Edit|Write|MultiEdit` (только под `--strict-hooks`). Семантика
//! exit-кодов Claude Code: 0 — ок, 2 — блок с показом stderr агенту.
//! Консервативный дефолт — fail-soft на инфраструктуру, fail-hard на
//! вердикт: хук молча пропускается (exit 0), если `arch-be` не в PATH; блок
//! (exit 2) —
//! когда `arch-be gate --route auto` завершился ненулевым кодом (провал
//! любой составляющей: fitness, delta guard, `rule_weakened`, spine, trace).
//! Так инфраструктурные сбои (нет входа у составляющих гейта — SKIP внутри
//! `arch-be gate`) не останавливают сессию, а реальные нарушения — стопят
//! её. `PostToolUse` в дефолт не входит: гейт на репозитории с правилами
//! `command_succeeds` может гонять сборки/тесты — для каждой правки это
//! слишком дорого.
//!
//! Идемпотентность: повторный запуск даёт тот же результат — JSON
//! смерджен ключ-в-ключ, хуки не дублируются (поиск маркера
//! [`HOOK_MARKER`]), скиллы побайтово совпадают. `--dry-run` печатает план
//! без единой записи.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

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
    /// `GigaCode` (форк Qwen Code): автоопределение каталога настроек
    /// (`.gigacode/` → `.qwen/` → новый `.gigacode/`), мердж `mcpServers` в
    /// `<каталог>/settings.json`, скиллы в `<каталог>/skills/`, хуки — печать.
    GigaCode,
    /// Любой другой агент: только печать сниппетов.
    Generic,
}

impl Host {
    /// Разбор значения CLI: `claude` | `qwen` | `gigacode` | `codex` | `kimi` |
    /// `omp` | `generic` (допускаются составные алиасы `claude-code`,
    /// `qwen-code`, `kimi-code`, `giga-code`, `gcode`, `oh-my-pi`).
    ///
    /// # Errors
    /// Неизвестное имя хоста — сообщение со списком допустимых.
    pub fn parse(raw: &str) -> std::result::Result<Self, String> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "claude" | "claude-code" => Ok(Self::Claude),
            "qwen" | "qwen-code" => Ok(Self::Qwen),
            "gigacode" | "giga-code" | "gcode" => Ok(Self::GigaCode),
            "codex" => Ok(Self::Codex),
            "kimi" | "kimi-code" => Ok(Self::Kimi),
            "omp" | "oh-my-pi" => Ok(Self::Omp),
            "generic" => Ok(Self::Generic),
            other => Err(format!(
                "неизвестный хост '{other}' (допустимы: claude, qwen, gigacode, codex, kimi, omp, generic)"
            )),
        }
    }

    /// Каноничное имя для вывода.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Qwen => "qwen",
            Self::GigaCode => "gigacode",
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
    /// Узкий режим записи (`arch-be mcp serve --rw=reports`): запись разрешена
    /// только отчётам рубрики. Судейскому харнессу не нужны `adr_new`,
    /// `delta_propose` и `handoff_create`, а широкий `--rw` открывал их разом.
    pub rw_reports: bool,
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
    /// Каталоги библиотеки плагинов пользователя (`[plugins].dirs` конфига):
    /// источник скиллов для доставки в хост — диск приоритетен над
    /// встроенными ассетами (см. `install_skills`); пусто — только встроенные
    /// (поведение до волны A2).
    pub plugins_dirs: Vec<PathBuf>,
}

impl ConnectOptions {
    /// Дефолтные параметры для хоста и каталога: всё включено, запись в дом
    /// выключена, dry-run выключен, библиотека пользователя не подключена
    /// (скиллы — только встроенные).
    #[must_use]
    pub fn new(host: Host, dir: PathBuf) -> Self {
        Self {
            host,
            dir,
            rw: false,
            rw_reports: false,
            skills: true,
            hooks: true,
            agents_md: true,
            strict_hooks: false,
            apply_global: false,
            dry_run: false,
            home: None,
            plugins_dirs: Vec::new(),
        }
    }
}

/// Исход копирования одного скилла.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkillAction {
    /// Скопирован заново (число записанных файлов).
    Copied(usize),
    /// Обновлён версией источника (число перезаписанных файлов; прежняя
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
    /// Скиллов доставлено из библиотеки пользователя (`[plugins].dirs`;
    /// диск — источник истины для всего, что на нём есть).
    pub skills_from_library: usize,
    /// Скиллов доставлено из встроенных ассетов бинаря (на диске их нет
    /// вообще — fallback свежей машины без `arch-be init`).
    pub skills_from_embedded: usize,
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
        Host::GigaCode => connect_gigacode(opts, &mut report)?,
        Host::Codex => connect_codex(opts, &mut report)?,
        Host::Kimi => connect_kimi(opts, &mut report)?,
        Host::Omp => connect_omp(opts, &mut report)?,
        Host::Generic => connect_generic(opts, &mut report),
    }
    write_connect_manifest(opts, &mut report)?;
    Ok(report)
}

/// Провенанс установки (П3 ДКА): `connect` записывает
/// `.arch-handoff/connect-manifest.json` — какие пути положил сам Spine,
/// с версией и хэшами. Детекторы значимости ([`crate::control`]) исключают
/// эти пути, поэтому подключение инструмента не меняет маршрут проекта.
///
/// # Errors
/// Ошибка записи манифеста (нет прав на `.arch-handoff/`).
fn write_connect_manifest(opts: &ConnectOptions, report: &mut ConnectReport) -> Result<()> {
    if opts.dry_run {
        return Ok(());
    }
    let rel = |p: &Path| {
        p.strip_prefix(&opts.dir)
            .ok()
            .map(|r| r.to_string_lossy().replace('\\', "/"))
    };
    let path = opts.dir.join(crate::control::CONNECT_MANIFEST_PATH);
    // Манифест — про установленное СОСТОЯНИЕ, а не про текущий прогон: слияние
    // с предыдущим делает повтор `connect` байт-в-байт идемпотентным и не
    // «теряет» пути, которые в этом прогоне уже были без изменений.
    let prev: Option<Value> = std::fs::read_to_string(&path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok());
    let mut paths: Vec<String> = prev
        .as_ref()
        .and_then(|v| v.get("paths"))
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    let mut sha256: serde_json::Map<String, Value> = prev
        .as_ref()
        .and_then(|v| v.get("sha256"))
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    for p in report.created.iter().chain(report.merged.iter()) {
        if let Some(r) = rel(p) {
            paths.push(r.clone());
            if p.is_file() {
                if let Ok(bytes) = std::fs::read(p) {
                    sha256.insert(r, json!(crate::hash::sha256_hex(&bytes)));
                }
            }
        }
    }
    // Каталоги скиллов — паттерном (файлов много, исключается весь каталог).
    for root in [
        ".claude/skills",
        ".qwen/skills",
        ".gigacode/skills",
        ".kimi-code/skills",
    ] {
        if opts.dir.join(root).is_dir() {
            paths.push(format!("{root}/**"));
        }
    }
    paths.sort();
    paths.dedup();
    // Пути, которых больше нет, из манифеста выпадают.
    paths.retain(|p| p.ends_with("/**") || opts.dir.join(p).exists());
    sha256.retain(|k, _| paths.contains(k));
    if paths.is_empty() {
        return Ok(());
    }
    let manifest = json!({
        "arch_be": env!("CARGO_PKG_VERSION"),
        "host": format!("{:?}", opts.host).to_ascii_lowercase(),
        "installed_at": chrono::Local::now().to_rfc3339(),
        "paths": paths,
        "sha256": Value::Object(sha256),
    });
    // Идемпотентность: неизменившийся манифест не перезаписывается (иначе
    // `installed_at` ломает байт-в-байт повтор `connect`).
    let unchanged = prev.as_ref().is_some_and(|p| {
        p.get("paths") == manifest.get("paths")
            && p.get("sha256") == manifest.get("sha256")
            && p.get("host") == manifest.get("host")
            && p.get("arch_be") == manifest.get("arch_be")
    });
    if unchanged {
        report.notes.push(format!(
            "провенанс установки: {} (без изменений)",
            crate::control::CONNECT_MANIFEST_PATH
        ));
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| HarnessError::io(parent, e))?;
    }
    let text = serde_json::to_string_pretty(&manifest)
        .map_err(|e| HarnessError::Config(format!("сериализация connect-manifest: {e}")))?;
    std::fs::write(&path, text).map_err(|e| HarnessError::io(&path, e))?;
    report.notes.push(format!(
        "провенанс установки: {} ({} путей исключены из детекторов значимости)",
        crate::control::CONNECT_MANIFEST_PATH,
        manifest["paths"].as_array().map_or(0, Vec::len)
    ));
    Ok(())
}

/// Описание нашего MCP-сервера для JSON-конфигов хостов.
fn mcp_server_value(rw: bool, rw_reports: bool) -> Value {
    json!({"command": "arch-be", "args": mcp_server_args(rw, rw_reports)})
}

/// Аргументы запуска MCP-сервера: без флага — read-only, `--rw` — полный
/// контур записи, `--rw=reports` — только отчёты рубрики (J7).
fn mcp_server_args(rw: bool, rw_reports: bool) -> Vec<&'static str> {
    if rw_reports {
        vec!["mcp", "serve", "--rw=reports"]
    } else if rw {
        vec!["mcp", "serve", "--rw"]
    } else {
        vec!["mcp", "serve"]
    }
}

/// Режим подключения одним словом — для подсказок и `doctor --host` (J7).
#[must_use]
pub fn mode_label(rw: bool, rw_reports: bool) -> &'static str {
    if rw_reports {
        "rw=reports"
    } else if rw {
        "rw"
    } else {
        "read-only"
    }
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
// Четыре булевых флага подключения (режим записи, бэкап, dry-run) —
// независимые опции одного шага, а не состояние: их разбор в структуру
// опций — отдельная задача, здесь это ухудшило бы читаемость вызова.
#[allow(clippy::fn_params_excessive_bools)]
fn merge_mcp_servers_json(
    path: &Path,
    rw: bool,
    rw_reports: bool,
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
    servers.insert(
        MCP_SERVER_NAME.to_string(),
        mcp_server_value(rw, rw_reports),
    );
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

/// База для хуков сессии (Stop / PostToolUse): точка ответвления от основной
/// ветки — тот же список веток и тот же `merge-base`, что у
/// [`crate::control::default_anchor_base`]. Ветка не найдена — пусто, и хук
/// работает без базы (как 0.3.3). После Н5 хуки обязаны видеть УЖЕ
/// закоммиченное ослабление правила: без базы сравнение шло с `HEAD`.
///
/// T-03: в `--base` уходит ГОЛАЯ ревизия. Раньше шаблоны передавали готовый
/// диапазон `$BASE...HEAD`, гейт дописывал `...HEAD` второй раз, и дифф не
/// вычислялся вовсе — гейт молча уходил в fail-safe Critical.
const ANCHOR_BASE_SNIPPET: &str = "\
BASE=\"\"\n\
for anchor in origin/main main origin/master master; do\n\
\x20 if git rev-parse --verify --quiet \"$anchor\" >/dev/null 2>&1; then\n\
\x20   BASE=$(git merge-base \"$anchor\" HEAD 2>/dev/null || true)\n\
\x20   break\n\
\x20 fi\n\
done\n";

/// Команда Stop-хука Claude Code: единый архитектурный гейт перед завершением
/// сессии. Гард — только `command -v arch-be` (fail-soft на инфраструктуру:
/// нет бинаря — пропуск); блок (exit 2, stderr агенту) — по коду возврата
/// `arch-be gate` (ненулевой = провал хотя бы одной составляющей; строки
/// вывода не разбираются).
///
/// T-01: раньше шаблон сам проверял наличие `.arch-handoff/CONSTRAINTS.yaml`,
/// и на кейсе, собранном `bootstrap` (реестр в КОРНЕ), хук молча пропускал
/// красный гейт — контур молчал там, где обязан остановить. Расположение
/// реестра знает только бинарь (резолвер `control::resolve_constraints_path`:
/// корень, затем `.arch-handoff/`), поэтому в shell его больше нет: нет
/// реестра нигде — `gate` печатает «реестр правил не найден» и отдаёт
/// INCOMPLETE, хук блокирует.
fn stop_hook_command() -> String {
    format!(
        "if command -v arch-be >/dev/null 2>&1; then \
         {ANCHOR_BASE_SNIPPET}\
         if [ -n \"$BASE\" ]; then \
         if ! out=$(arch-be gate --route auto --base \"$BASE\" 2>&1); then \
         printf '%s\\n\\n%s\\n' \"$out\" \
         \"{HOOK_MARKER}: архитектурный гейт FAIL — исправьте находки error перед завершением \
         (подробности выше; гейт: arch-be gate)\" >&2; exit 2; fi; \
         elif ! out=$(arch-be gate --route auto 2>&1); then \
         printf '%s\\n\\n%s\\n' \"$out\" \
         \"{HOOK_MARKER}: архитектурный гейт FAIL — исправьте находки error перед завершением \
         (подробности выше; гейт: arch-be gate)\" >&2; exit 2; fi; fi \
         # {HOOK_MARKER}:stop"
    )
}

/// Команда PostToolUse-хука (`--strict-hooks`): тот же гейт на каждую
/// правку файла (matcher `Edit|Write|MultiEdit`).
fn post_tool_use_hook_command() -> String {
    format!(
        "if command -v arch-be >/dev/null 2>&1; then \
         {ANCHOR_BASE_SNIPPET}\
         if [ -n \"$BASE\" ]; then \
         if ! out=$(arch-be gate --route auto --base \"$BASE\" 2>&1); then \
         printf '%s\\n\\n%s\\n' \"$out\" \
         \"{HOOK_MARKER}: правка не проходит архитектурный гейт (arch-be gate FAIL) — \
         исправьте находки error\" >&2; exit 2; fi; \
         elif ! out=$(arch-be gate --route auto 2>&1); then \
         printf '%s\\n\\n%s\\n' \"$out\" \
         \"{HOOK_MARKER}: правка не проходит архитектурный гейт (arch-be gate FAIL) — \
         исправьте находки error\" >&2; exit 2; fi; fi \
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
        "семантика хуков: fail-soft на инфраструктуру (нет arch-be — молча \
         пропуск; нет входа у составляющих — SKIP), блок (exit 2) — по ненулевому коду \
         `arch-be gate --route auto` (провал любой составляющей: fitness, \
         delta guard, rule_weakened, spine, trace); PostToolUse-гейт на \
         каждую правку — через --strict-hooks (дорого на репозиториях с \
         command_succeeds-правилами)"
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

/// Собирает файлы скиллов из библиотеки пользователя на диске
/// (`[plugins].dirs`): для каждого каталога — каждый подкаталог с `skills/`
/// (манифест `plugin.json` НЕ требуется: плагины-мешки вроде arch-distilled
/// и самодельные каталоги скиллов доставляются тоже — тот же критерий, что
/// у синтеза манифеста в [`crate::plugin::discover`]), рекурсивно
/// `skills/<скилл>/<файл>`. Потолок размера и дедуп путей назначения — как
/// у встроенного сборщика [`collect_skill_files_from`]; не-UTF-8 файлы
/// пропускаются с заметкой (конвейер доставки текстовый).
fn collect_skill_files_from_disk(dirs: &[PathBuf]) -> (Vec<(PathBuf, String)>, Vec<String>) {
    let mut out: Vec<(PathBuf, String)> = Vec::new();
    let mut skipped = Vec::new();
    let mut seen: BTreeSet<PathBuf> = BTreeSet::new();
    for dir in dirs {
        let Ok(rd) = std::fs::read_dir(dir) else {
            continue; // каталога нет — источник просто пуст (свежая машина)
        };
        let mut plugins: Vec<PathBuf> = rd
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect();
        plugins.sort();
        for plugin in plugins {
            let skills_root = plugin.join("skills");
            if !skills_root.is_dir() {
                continue;
            }
            for entry in walkdir::WalkDir::new(&skills_root)
                .follow_links(false)
                .sort_by_file_name()
                .into_iter()
                .flatten()
            {
                if !entry.file_type().is_file() {
                    continue;
                }
                let path = entry.path();
                let Ok(rel) = path.strip_prefix(&skills_root) else {
                    continue; // вне корня скиллов — недостижимо для WalkDir
                };
                let segs: Vec<&str> = rel
                    .components()
                    .filter_map(|c| c.as_os_str().to_str())
                    .collect();
                // Назначение — <скилл>/<файл…>; файл прямо в skills/ (без
                // каталога скилла) — не layout, пропускаем с заметкой.
                let [skill, rest @ ..] = segs.as_slice() else {
                    continue;
                };
                if rest.is_empty() {
                    skipped.push(format!(
                        "{}: файл вне layout skills/<имя>/ — пропущен",
                        path.display()
                    ));
                    continue;
                }
                let dest = PathBuf::from(skill).join(rest.join("/"));
                if !seen.insert(dest.clone()) {
                    skipped.push(format!(
                        "{}: дубль пути назначения {}",
                        path.display(),
                        dest.display()
                    ));
                    continue;
                }
                let Ok(content) = std::fs::read_to_string(path) else {
                    skipped.push(format!(
                        "{}: не текстовый файл (UTF-8) — пропущен",
                        path.display()
                    ));
                    continue;
                };
                if content.len() > MAX_SKILL_FILE_BYTES {
                    skipped.push(format!(
                        "{}: {} КБ — больше потолка {} КБ",
                        path.display(),
                        content.len() / 1024,
                        MAX_SKILL_FILE_BYTES / 1024
                    ));
                    continue;
                }
                let body = if rest == ["SKILL.md"] {
                    ensure_frontmatter(&content, skill)
                } else {
                    content
                };
                out.push((dest, body));
            }
        }
    }
    out.sort();
    (out, skipped)
}

/// Раскладывает скиллы в `root` (`.claude/skills` хоста). Источник истины —
/// библиотека пользователя на диске (`plugin_dirs`, т.е. `[plugins].dirs`
/// конфига): всё, что там есть (включая плагины без манифеста и правки
/// пользователя), едет в хост; встроенные ассеты бинаря — fallback для
/// скиллов, которых на диске нет вообще (свежая машина без `arch-be init`).
/// Прецедент приоритета диска — MCP-промпты и `skill_load`
/// (`mcp_server.rs::resolve_playbook`). Новые копируются, отличающиеся —
/// перезаписываются версией источника (заметка в отчёте), совпадающие —
/// пропускаются (идемпотентность).
fn install_skills(
    root: &Path,
    dry_run: bool,
    report: &mut ConnectReport,
    plugin_dirs: &[PathBuf],
) -> Result<()> {
    let (embedded, skipped_embedded) =
        collect_skill_files_from(crate::assets::embedded_plugin_files());
    let (disk, skipped_disk) = collect_skill_files_from_disk(plugin_dirs);
    report.skills_skipped.extend(skipped_embedded);
    report.skills_skipped.extend(skipped_disk);
    // Слияние источников: диск побеждает пофайлово; скилл «из библиотеки»,
    // если хотя бы один его файл пришёл с диска.
    let mut by_dest: BTreeMap<PathBuf, (&str, bool)> = BTreeMap::new(); // → (содержимое, с_диска)
    let embedded_map: BTreeMap<&Path, &str> = embedded
        .iter()
        .map(|(p, c)| (p.as_path(), c.as_str()))
        .collect();
    let disk_map: BTreeMap<&Path, &str> = disk
        .iter()
        .map(|(p, c)| (p.as_path(), c.as_str()))
        .collect();
    for (dest, content) in &embedded_map {
        by_dest.insert(dest.to_path_buf(), (*content, false));
    }
    for (dest, content) in &disk_map {
        by_dest.insert(dest.to_path_buf(), (*content, true));
    }
    // Имена скиллов (первый сегмент назначения) по источникам + счёт
    // расходящихся копий (диск отличается от встроенной по тому же пути).
    let skill_of = |dest: &Path| {
        dest.components().next().map_or_else(
            || "?".into(),
            |c| c.as_os_str().to_string_lossy().into_owned(),
        )
    };
    let library_skills: BTreeSet<String> = disk_map.keys().map(|p| skill_of(p)).collect();
    report.skills_from_library = library_skills.len();
    report.skills_from_embedded = embedded_map
        .keys()
        .map(|p| skill_of(p))
        .collect::<BTreeSet<_>>()
        .difference(&library_skills)
        .count();
    let overriding = library_skills
        .iter()
        .filter(|skill| {
            disk_map.iter().any(|(dest, disk_content)| {
                skill_of(dest) == **skill
                    && embedded_map
                        .get(dest)
                        .is_some_and(|embedded_content| embedded_content != disk_content)
            })
        })
        .count();
    if overriding > 0 {
        report.notes.push(format!(
            "{overriding} скиллов берутся из библиотеки пользователя поверх встроенных \
             копий (правки и новые версии пользователя в силе)"
        ));
    }
    // Агрегация по имени скилла: (новых, перезаписанных, без изменений).
    let mut by_skill: BTreeMap<String, (usize, usize, usize)> = BTreeMap::new();
    for (rel, (content, _from_disk)) in &by_dest {
        let path = root.join(rel);
        let skill = skill_of(rel);
        let old = std::fs::read_to_string(&path).ok();
        let counts = by_skill.entry(skill).or_default();
        match old.as_deref() {
            None => counts.0 += 1,
            Some(prev) if prev == *content => counts.2 += 1,
            Some(_) => counts.1 += 1,
        }
        if dry_run || old.as_deref() == Some(*content) {
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
                "скилл «{name}»: прежняя локальная правка заменена версией источника \
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

/// Финальная строка `next_steps` каждого хоста («первая ценность за
/// 5 минут», C-1): плейбук-промпт MCP-сервера и демо-кейс FAIL→fix→PASS.
const FIRST_VALUE_STEP: &str = "Первая ценность за 5 минут: попросите агента \
     «действуй по плейбуку spine-quickstart»; демо FAIL→fix→PASS — кейс \
     drift-control из репозитория Spine.";

/// `arch-be connect claude`: проектный `.mcp.json`, хуки в
/// `.claude/settings.json`, скиллы в `.claude/skills/`, блок в CLAUDE.md.
fn connect_claude(opts: &ConnectOptions, report: &mut ConnectReport) -> Result<()> {
    merge_mcp_servers_json(
        &opts.dir.join(".mcp.json"),
        opts.rw,
        opts.rw_reports,
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
        install_skills(
            &opts.dir.join(".claude/skills"),
            opts.dry_run,
            report,
            &opts.plugins_dirs,
        )?;
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
            if opts.rw_reports {
                "rw=reports (запись только отчётов рубрики: rubric_verify → reports/rubric/)"
            } else if opts.rw {
                "rw (разрешены аддитивные записи: handoff_create, adr_new, …)"
            } else {
                "read-only"
            }
        ),
        FIRST_VALUE_STEP.to_string(),
    ]);
    Ok(())
}

/// Сниппет `.mcp.json` для печати (generic-потоки).
fn mcp_json_snippet(rw: bool, rw_reports: bool) -> String {
    let v = json!({"mcpServers": {MCP_SERVER_NAME: mcp_server_value(rw, rw_reports)}});
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
fn codex_toml_block(rw: bool, rw_reports: bool) -> String {
    let args = if rw_reports {
        "[\"mcp\", \"serve\", \"--rw=reports\"]"
    } else if rw {
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
    rw_reports: bool,
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
    // Одна функция аргументов на все каналы: кодовая копия логики режима
    // разошлась бы с остальными при первой же правке (J7).
    let args = mcp_server_args(rw, rw_reports);
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
/// (Qwen Code — форк gemini-cli, ключ подтверждён) + скиллы в
/// `.qwen/skills/` (подтверждено на qwen-code 0.24.0 — нативный project-scope);
/// хуки не пишутся (headless-файринг не подтверждён) — сниппет-референс.
fn connect_qwen(opts: &ConnectOptions, report: &mut ConnectReport) -> Result<()> {
    merge_mcp_servers_json(
        &opts.dir.join(".qwen/settings.json"),
        opts.rw,
        opts.rw_reports,
        false,
        opts.dry_run,
        report,
    )?;
    if opts.skills {
        let skills_root = opts.dir.join(".qwen/skills");
        if skills_root.exists() {
            report.notes.push(format!(
                "{} уже существует — скиллы НЕ раскладываются (чужую \
                 библиотеку не перетираем)",
                skills_root.display()
            ));
        } else {
            install_skills(&skills_root, opts.dry_run, report, &opts.plugins_dirs)?;
            report.notes.push(
                "скиллы разложены в .qwen/skills/ — Qwen Code ≥ 0.24 читает этот \
                 каталог нативно (project scope)"
                    .into(),
            );
        }
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
        FIRST_VALUE_STEP.to_string(),
    ]);
    Ok(())
}

/// Исход автоопределения каталога настроек `GigaCode` в проекте.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GigacodeDirOrigin {
    /// В проекте уже есть `.gigacode/` — он и используется.
    ExistingGigacode,
    /// `.gigacode/` нет, но есть `.qwen/`: `GigaCode` — форк Qwen Code,
    /// layout совместим — пишем в него.
    FromQwen,
    /// Ни того ни другого — создаётся `.gigacode/`.
    NewGigacode,
}

/// Автоопределение каталога настроек `GigaCode`: существующий `.gigacode/`
/// (приоритет), иначе существующий `.qwen/`, иначе — новый `.gigacode/`.
/// Используется и `connect gigacode`, и `doctor --host gigacode` (проверка
/// смотрит в тот же каталог, куда писал connect).
#[must_use]
pub fn gigacode_settings_dir(project_dir: &Path) -> (PathBuf, GigacodeDirOrigin) {
    let gigacode = project_dir.join(".gigacode");
    if gigacode.is_dir() {
        return (gigacode, GigacodeDirOrigin::ExistingGigacode);
    }
    let qwen = project_dir.join(".qwen");
    if qwen.is_dir() {
        return (qwen, GigacodeDirOrigin::FromQwen);
    }
    (gigacode, GigacodeDirOrigin::NewGigacode)
}

/// `arch-be connect gigacode` (`GigaCode` — форк Qwen Code): каталог настроек
/// определяется автоматически ([`gigacode_settings_dir`]); в него пишется
/// `settings.json` (мердж `mcpServers.spine`, как у qwen) и раскладываются
/// скиллы в `<каталог>/skills/` (как у claude — с обновлением встроенных
/// версий: целевой хост `GigaCode` освоил project-скиллы по layout Qwen Code
/// 0.24). Хуки не пишутся (подтверждённой схемы файла хуков у форка нет —
/// в qwen-code 0.24 хуки управляются UI `qwen hooks` и в headless не файрят)
/// — печатается сниппет-референс, как у qwen.
fn connect_gigacode(opts: &ConnectOptions, report: &mut ConnectReport) -> Result<()> {
    let (settings_dir, origin) = gigacode_settings_dir(&opts.dir);
    match origin {
        GigacodeDirOrigin::ExistingGigacode => report.notes.push(format!(
            "каталог настроек автоопределён: {} (существующий .gigacode/)",
            settings_dir.display()
        )),
        GigacodeDirOrigin::FromQwen => report.notes.push(format!(
            "каталог настроек автоопределён: {} (`.gigacode/` нет, найден `.qwen/` — \
             GigaCode — форк Qwen Code, layout совместим)",
            settings_dir.display()
        )),
        GigacodeDirOrigin::NewGigacode => report.notes.push(format!(
            "каталога настроек в проекте нет — {} (ни .gigacode/, ни .qwen/)",
            if opts.dry_run {
                format!("будет создан {}", settings_dir.display())
            } else {
                format!("создан {}", settings_dir.display())
            }
        )),
    }
    merge_mcp_servers_json(
        &settings_dir.join("settings.json"),
        opts.rw,
        opts.rw_reports,
        false,
        opts.dry_run,
        report,
    )?;
    if opts.skills {
        install_skills(
            &settings_dir.join("skills"),
            opts.dry_run,
            report,
            &opts.plugins_dirs,
        )?;
        report.notes.push(
            "скиллы разложены в <каталог настроек>/skills/ — layout project-скиллов \
             Qwen Code ≥ 0.24, унаследован форком"
                .into(),
        );
    }
    if opts.hooks {
        report.notes.push(
            "хуки не записаны: подтверждённой схемы файла хуков у GigaCode нет \
             (в qwen-code 0.24 хуки управляются через `qwen hooks` (UI) и в \
             headless-режиме не файрят). Информационный вариант из docs/GIGACODE.md — \
             вручную в settings.json: \"hooks\": {\"SessionEnd\": [{\"hooks\": \
             [{\"type\": \"command\", \"command\": \"arch-be gate --route auto 2>&1 | \
             tail -3\"}]}]} (показывает вердикт гейта, но НЕ блокирует завершение). \
             Блокирующие гейты есть у Claude Code/Kimi/omp"
                .into(),
        );
        report.snippets.push((
            "Хуки (формат Claude Code, референс)".to_string(),
            hooks_snippet(opts.strict_hooks),
        ));
    }
    report.next_steps.extend([
        "перезапустите GigaCode в этом каталоге".to_string(),
        "одобрите project-сервер, если хост попросит (в Qwen Code: \
         `qwen mcp approve spine`; в GigaCode — аналог вашей сборки)"
            .to_string(),
        "проверьте подключение: `arch-be doctor --host gigacode`".to_string(),
        FIRST_VALUE_STEP.to_string(),
    ]);
    Ok(())
}

/// `arch-be connect codex`: MCP — пользовательский `~/.codex/config.toml`;
/// по умолчанию только печать TOML-блока (в дом молча не пишем).
fn connect_codex(opts: &ConnectOptions, report: &mut ConnectReport) -> Result<()> {
    if opts.apply_global {
        let path = global_config_path(opts, ".codex/config.toml")?;
        merge_codex_config(&path, opts.rw, opts.rw_reports, opts.dry_run, report)?;
    } else {
        report.snippets.push((
            "MCP-сервер для Codex — добавьте в ~/.codex/config.toml:".to_string(),
            codex_toml_block(opts.rw, opts.rw_reports),
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
            "у Codex нет lifecycle-хуков уровня PreToolUse/Stop — архитектурный гейт \
             остаётся ручным (`arch-be gate`) или в CI"
                .into(),
        );
    }
    report.next_steps.extend([
        "перезапустите `codex` в этом каталоге".to_string(),
        "проверьте список MCP-серверов: `codex mcp list` — в нём «spine»".to_string(),
        FIRST_VALUE_STEP.to_string(),
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
        opts.rw_reports,
        false,
        opts.dry_run,
        report,
    )?;
    if opts.apply_global {
        let path = global_config_path(opts, ".kimi-code/mcp.json")?;
        merge_mcp_servers_json(&path, opts.rw, opts.rw_reports, true, opts.dry_run, report)?;
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
            mcp_json_snippet(opts.rw, opts.rw_reports),
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
            if opts.rw_reports {
                "rw=reports (запись только отчётов рубрики: rubric_verify → reports/rubric/)"
            } else if opts.rw {
                "rw (разрешены аддитивные записи: handoff_create, adr_new, …)"
            } else {
                "read-only"
            }
        ),
        FIRST_VALUE_STEP.to_string(),
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
/// проекте ещё нет, скиллы библиотеки раскладываются туда тем же
/// `install_skills`, что у claude; каталог уже есть — не трогаем, чтобы не
/// перетирать чужую библиотеку (об этом заметка). Хуков через connect нет:
/// механизм хуков omp — TypeScript-расширения (`omp --hook <file.ts>`),
/// печатается указание.
fn connect_omp(opts: &ConnectOptions, report: &mut ConnectReport) -> Result<()> {
    merge_mcp_servers_json(
        &opts.dir.join(".mcp.json"),
        opts.rw,
        opts.rw_reports,
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
                "{} уже существует — скиллы НЕ раскладываются (чужую \
                 библиотеку не перетираем; omp читает .claude/skills нативно, \
                 принудительно разложить встроенные может `arch-be connect claude`)",
                skills_root.display()
            ));
        } else {
            install_skills(&skills_root, opts.dry_run, report, &opts.plugins_dirs)?;
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
             расширения: `arch-be gate`"
                .into(),
        );
    }
    report.next_steps.extend([
        "перезапустите omp в этом каталоге — сервер «spine» подхватится из .mcp.json".to_string(),
        format!(
            "инструменты видны агенту как инструменты сервера «spine»; режим: {}",
            if opts.rw_reports {
                "rw=reports (запись только отчётов рубрики: rubric_verify → reports/rubric/)"
            } else if opts.rw {
                "rw (разрешены аддитивные записи: handoff_create, adr_new, …)"
            } else {
                "read-only"
            }
        ),
        FIRST_VALUE_STEP.to_string(),
    ]);
    Ok(())
}

/// `arch-be connect generic`: ничего не пишет — печатает сниппеты и
/// инструкцию ручной установки.
fn connect_generic(opts: &ConnectOptions, report: &mut ConnectReport) {
    report.snippets.push((
        "MCP-сервер (формат `.mcp.json` Claude Code — его понимает большинство хостов):"
            .to_string(),
        mcp_json_snippet(opts.rw, opts.rw_reports),
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
        FIRST_VALUE_STEP.to_string(),
    ]);
}

// ---------------------------------------------------------------------------
// `connect ci` и `connect git-hooks` — гейты, не зависящие от хоста
// ---------------------------------------------------------------------------

/// Маркер начала нашего блока в файле CI/хуке (комментарий; префикс `#`/`//`
/// зависит от файла — ищется голая подстрока маркера).
const BLOCK_BEGIN: &str = "spine-connect:begin";
/// Маркер конца нашего блока.
const BLOCK_END: &str = "spine-connect:end";

/// CI-провайдер для `arch-be connect ci --provider …`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CiProvider {
    /// GitLab CI: мердж-блок в `.gitlab-ci.yml`, артефакт `reports.codequality`.
    GitLab,
    /// GitHub Actions: новый `.github/workflows/spine-gate.yml`, SARIF-артефакт.
    GitHub,
    /// Jenkins: мердж-блок в `Jenkinsfile`, публикация `junit(...)`.
    Jenkins,
}

impl CiProvider {
    /// Разбор значения CLI: `gitlab` | `github` | `jenkins`.
    ///
    /// # Errors
    /// Неизвестный провайдер — сообщение со списком допустимых.
    pub fn parse(raw: &str) -> std::result::Result<Self, String> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "gitlab" | "git-lab" => Ok(Self::GitLab),
            "github" | "git-hub" => Ok(Self::GitHub),
            "jenkins" => Ok(Self::Jenkins),
            other => Err(format!(
                "неизвестный CI-провайдер '{other}' (допустимы: gitlab, github, jenkins)"
            )),
        }
    }

    /// Каноничное имя для вывода.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::GitLab => "gitlab",
            Self::GitHub => "github",
            Self::Jenkins => "jenkins",
        }
    }

    /// Путь целевого файла джобы в проекте.
    fn job_file(self, dir: &Path) -> PathBuf {
        match self {
            Self::GitLab => dir.join(".gitlab-ci.yml"),
            Self::GitHub => dir.join(".github/workflows/spine-gate.yml"),
            Self::Jenkins => dir.join("Jenkinsfile"),
        }
    }

    /// Текст джобы (между маркерами [`BLOCK_BEGIN`]/[`BLOCK_END`]).
    fn job_block(self) -> String {
        match self {
            Self::GitLab => gitlab_ci_block(),
            Self::GitHub => github_workflow_block(),
            Self::Jenkins => jenkinsfile_block(),
        }
    }
}

/// Подстановка версии крейта в шаблоны CI (токен `@ARCH_BE_VERSION@` —
/// `format!` не подходит: в шаблонах есть `${{ … }}` GitHub Actions).
fn with_version(template: &str) -> String {
    template.replace("@ARCH_BE_VERSION@", env!("CARGO_PKG_VERSION"))
}

/// Джоба GitLab CI: `arch-be gate` с нативным форматом `gitlab-codequality`
/// в артефакт `reports.codequality` (нарушения появляются в интерфейсе merge
/// request без ручной настройки) + текстовая сводка в лог джобы.
fn gitlab_ci_block() -> String {
    with_version(
        "# spine-connect:begin — архитектурный гейт Spine (arch-be)\n\
         # Блок перегенерируется: `arch-be connect ci --provider gitlab`; свои правки — вне маркеров.\n\
         # Нарушения видны в интерфейсе merge request (Code Quality) из артефакта\n\
         # reports.codequality — ручной настройки не нужно.\n\
         spine-gate:\n\
         \x20 stage: test\n\
         \x20 image: debian:bookworm-slim\n\
         \x20 variables:\n\
         \x20   ARCH_BE_VERSION: \"@ARCH_BE_VERSION@\"\n\
         \x20   # Откуда брать бинарь arch-be (linux-x86_64):\n\
         \x20   #   A) релизы: ${RELEASES_URL}/v${ARCH_BE_VERSION}/arch-be-linux-x86_64\n\
         \x20   #   B) закрытый контур: офлайн-бандл spine-offline-*-linux-x86_64.tar.gz\n\
         \x20   #      во внутреннем хранилище артефактов (scripts/make_offline_bundle.sh).\n\
         \x20   RELEASES_URL: \"https://github.com/<org>/<repo>/releases/download\"\n\
         \x20   GIT_DEPTH: \"0\"   # полная история: гейту нужна база диффа origin/<целевая ветка>\n\
         \x20 before_script:\n\
         \x20   - apt-get update -qq && apt-get install -y -qq curl ca-certificates git > /dev/null\n\
         \x20   - curl -fsSL -o /usr/local/bin/arch-be \"${RELEASES_URL}/v${ARCH_BE_VERSION}/arch-be-linux-x86_64\" && chmod +x /usr/local/bin/arch-be\n\
         \x20   - arch-be --version\n\
         \x20 script:\n\
         \x20   # Машинный отчёт — в файл артефакта; при провале гейта джоба красная (exit 1).\n\
         \x20   - arch-be gate --route auto --base \"origin/${CI_MERGE_REQUEST_TARGET_BRANCH_NAME:-main}\" --format gitlab-codequality > codequality-spine.json || GATE_EXIT=$?\n\
         \x20   # Текстовая сводка в лог джобы (при дорогих правилах command_succeeds строку можно убрать).\n\
         \x20   - arch-be gate --route auto --base \"origin/${CI_MERGE_REQUEST_TARGET_BRANCH_NAME:-main}\" || true\n\
         \x20   - exit ${GATE_EXIT:-0}\n\
         \x20 artifacts:\n\
         \x20   when: always\n\
         \x20   reports:\n\
         \x20     codequality: codequality-spine.json   # → виджет Code Quality в merge request\n\
         # spine-connect:end",
    )
}

/// Workflow GitHub Actions: `arch-be gate` с SARIF (артефакт прогона; при
/// включённом Advanced Security — загрузка в code scanning, вариант в
/// комментарии) + markdown-сводка в Job Summary. Внешних действий сверх
/// официальных `actions/checkout` и `actions/upload-artifact` нет.
fn github_workflow_block() -> String {
    with_version(
        "# spine-connect:begin — архитектурный гейт Spine (arch-be)\n\
         # Файл целиком генерируется `arch-be connect ci --provider github`; перегенерация — той же командой.\n\
         name: spine-gate\n\
         on:\n\
         \x20 pull_request:\n\
         \x20 push:\n\
         \x20   branches: [main]\n\
         permissions: {}\n\
         jobs:\n\
         \x20 gate:\n\
         \x20   runs-on: ubuntu-latest\n\
         \x20   steps:\n\
         \x20     - uses: actions/checkout@v4\n\
         \x20       with:\n\
         \x20         fetch-depth: 0   # полная история для --base (дифф к целевой ветке)\n\
         \x20     - name: Установка arch-be\n\
         \x20       env:\n\
         \x20         ARCH_BE_VERSION: \"@ARCH_BE_VERSION@\"\n\
         \x20         # A) релизы (linux-x86_64); B) закрытый контур — URL офлайн-бандла\n\
         \x20         # из внутреннего хранилища артефактов.\n\
         \x20         RELEASES_URL: \"https://github.com/<org>/<repo>/releases/download\"\n\
         \x20       run: |\n\
         \x20         sudo curl -fsSL -o /usr/local/bin/arch-be \"${RELEASES_URL}/v${ARCH_BE_VERSION}/arch-be-linux-x86_64\" && sudo chmod +x /usr/local/bin/arch-be\n\
         \x20         arch-be --version\n\
         \x20     - name: Архитектурный гейт\n\
         \x20       run: |\n\
         \x20         BASE=\"origin/${{ github.base_ref || 'main' }}\"\n\
         \x20         arch-be gate --route auto --base \"${BASE}\" --format sarif > spine-gate.sarif || GATE_EXIT=$?\n\
         \x20         arch-be gate --route auto --base \"${BASE}\" --format markdown >> \"$GITHUB_STEP_SUMMARY\" || true\n\
         \x20         exit ${GATE_EXIT:-0}\n\
         \x20     - name: Отчёт SARIF артефактом\n\
         \x20       if: always()\n\
         \x20       uses: actions/upload-artifact@v4\n\
         \x20       with:\n\
         \x20         name: spine-gate-sarif\n\
         \x20         path: spine-gate.sarif\n\
         \x20     # Вариант для code scanning (Security → Code scanning), если включён\n\
         \x20     # GitHub Advanced Security:\n\
         \x20     # - name: Загрузка SARIF\n\
         \x20     #   if: always()\n\
         \x20     #   uses: github/codeql-action/upload-sarif@v3\n\
         \x20     #   with: { sarif_file: spine-gate.sarif }\n\
         # spine-connect:end",
    )
}

/// Джоба Jenkins (declarative pipeline): `arch-be gate --format junit` в файл,
/// публикация `junit(...)` (находки — как упавшие тесты) и `error(...)` по
/// коду возврата гейта.
fn jenkinsfile_block() -> String {
    with_version(
        "// spine-connect:begin — архитектурный гейт Spine (arch-be)\n\
         // Блок перегенерируется: `arch-be connect ci --provider jenkins`; свои правки — вне маркеров.\n\
         pipeline {\n\
         \x20   agent any\n\
         \x20   stages {\n\
         \x20       stage('Spine gate') {\n\
         \x20           steps {\n\
         \x20               // Бинарь arch-be (linux-x86_64): A) curl из релизов (ниже);\n\
         \x20               // B) закрытый контур — офлайн-бандл из внутреннего хранилища\n\
         \x20               // (укажите его адрес в RELEASES_URL).\n\
         \x20               sh '''\n\
         \x20                 if ! command -v arch-be >/dev/null 2>&1; then\n\
         \x20                   curl -fsSL -o /tmp/arch-be \"${RELEASES_URL:-https://github.com/<org>/<repo>/releases/download}/v@ARCH_BE_VERSION@/arch-be-linux-x86_64\" && install -m 755 /tmp/arch-be /usr/local/bin/arch-be\n\
         \x20                 fi\n\
         \x20                 arch-be --version\n\
         \x20               '''\n\
         \x20               script {\n\
         \x20                   // Код возврата сохраняем: junit() публикуем даже при красном гейте.\n\
         \x20                   env.SPINE_GATE_EXIT = sh(script: 'arch-be gate --route auto --format junit > spine-gate.xml', returnStatus: true).toString()\n\
         \x20                   sh 'arch-be gate --route auto || true'   // текстовая сводка в лог\n\
         \x20               }\n\
         \x20           }\n\
         \x20           post {\n\
         \x20               always {\n\
         \x20                   junit testResults: 'spine-gate.xml', allowEmptyResults: true\n\
         \x20                   archiveArtifacts artifacts: 'spine-gate.xml', allowEmptyArchive: true\n\
         \x20                   script {\n\
         \x20                       if (env.SPINE_GATE_EXIT != '0') {\n\
         \x20                           error('Spine gate FAIL — находки в spine-gate.xml и в логе джобы')\n\
         \x20                       }\n\
         \x20                   }\n\
         \x20               }\n\
         \x20           }\n\
         \x20       }\n\
         \x20   }\n\
         }\n\
         // spine-connect:end",
    )
}

/// Заменяет зону между маркерами [`BLOCK_BEGIN`]/[`BLOCK_END`] в существующем
/// файле; маркеров нет — дописка блока в конец (чужое содержимое сохраняется).
/// Детерминировано: повторный прогон побайтово совпадает.
fn splice_marked_block(existing: &str, block: &str) -> String {
    let trimmed = block.trim_end();
    match (existing.find(BLOCK_BEGIN), existing.find(BLOCK_END)) {
        (Some(b), Some(e)) if b < e => {
            // Границы — по строкам: начало строки с маркером begin и конец
            // строки с маркером end.
            let start = existing[..b].rfind('\n').map_or(0, |i| i + 1);
            let end = existing[e..]
                .find('\n')
                .map_or(existing.len(), |i| e + i + 1);
            let pre = existing[..start].trim_end();
            let tail = existing[end..].trim();
            // Те же разделители, что в ветке дописки (пустая строка между
            // зонами) — иначе повторный прогон менял бы файл после дописки.
            match (pre.is_empty(), tail.is_empty()) {
                (true, true) => format!("{trimmed}\n"),
                (true, false) => format!("{trimmed}\n\n{tail}\n"),
                (false, true) => format!("{pre}\n\n{trimmed}\n"),
                (false, false) => format!("{pre}\n\n{trimmed}\n\n{tail}\n"),
            }
        }
        _ => format!("{}\n\n{trimmed}\n", existing.trim_end()),
    }
}

/// Создаёт файл джобы либо встраивает блок в существующий (мердж маркерный —
/// чужие джобы/этапы сохраняются). GitHub — особый: `spine-gate.yml` —
/// целиком наш файл, существующий без маркера не затираем (отказ).
fn upsert_ci_job(
    provider: CiProvider,
    dir: &Path,
    dry_run: bool,
    releases_url: Option<&str>,
    report: &mut ConnectReport,
) -> Result<()> {
    let path = provider.job_file(dir);
    let block = substitute_releases_url(&provider.job_block(), releases_url);
    let old = match std::fs::read_to_string(&path) {
        Ok(text) => Some(text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(HarnessError::io(&path, e)),
    };
    let new = match old.as_deref() {
        None => format!("{block}\n"),
        Some(existing) => {
            if provider == CiProvider::GitHub && !existing.contains(BLOCK_BEGIN) {
                return Err(HarnessError::Config(format!(
                    "{}: файл существует и не помечен маркером «{BLOCK_BEGIN}» — не затираю; \
                     переименуйте его или удалите вручную",
                    path.display()
                )));
            }
            if existing.contains(BLOCK_BEGIN) {
                report.notes.push(format!(
                    "{}: блок между маркерами «{BLOCK_BEGIN}/{BLOCK_END}» обновлён, \
                     содержимое вне маркеров сохранено",
                    path.display()
                ));
            } else {
                report.notes.push(format!(
                    "{}: джоба дописана блоком с маркерами «{BLOCK_BEGIN}/{BLOCK_END}» в конец \
                     файла; свои секции проверьте на конфликт имён (job `spine-gate`)",
                    path.display()
                ));
            }
            splice_marked_block(existing, &block)
        }
    };
    commit_file(&path, old.as_deref(), &new, dry_run, report)
}

/// `arch-be connect ci --provider …`: пишет готовую джобу архитектурного
/// гейта под площадку (см. [`CiProvider`]).
///
/// # Errors
/// Целевой файл GitHub существует без нашего маркера; ошибки чтения/записи.
pub fn connect_ci(
    provider: CiProvider,
    dir: &Path,
    dry_run: bool,
    releases_url: Option<&str>,
) -> Result<ConnectReport> {
    let mut report = ConnectReport {
        dry_run,
        ..ConnectReport::default()
    };
    upsert_ci_job(provider, dir, dry_run, releases_url, &mut report)?;
    match provider {
        CiProvider::GitLab => {
            report.notes.push(
                "нарушения появятся в интерфейсе merge request (Code Quality) из артефакта \
                 reports.codequality — ручной настройки площадки не нужно"
                    .into(),
            );
            report.next_steps.extend([
                "закоммитьте .gitlab-ci.yml и откройте merge request — джоба spine-gate \
                 появится в пайплайне MR"
                    .to_string(),
                if releases_url.is_none() {
                    "задайте адрес релизов: `arch-be connect ci --provider gitlab \
                     --releases-url https://github.com/<org>/<repo>/releases/download` — \
                     сейчас в шаблоне заглушка <org>/<repo>"
                        .to_string()
                } else {
                    "адрес релизов подставлен из --releases-url".to_string()
                },
            ]);
        }
        CiProvider::GitHub => {
            report.notes.push(
                "SARIF складывается артефактом прогона (actions/upload-artifact); загрузка в \
                 code scanning (вкладка Security) — закомментированным шагом в файле (нужен \
                 GitHub Advanced Security)"
                    .into(),
            );
            report.next_steps.extend([
                "закоммитьте .github/workflows/spine-gate.yml — workflow spine-gate появится \
                 на pull_request и push в main"
                    .to_string(),
                "замените <org>/<repo> в RELEASES_URL на адрес релизов/хранилища, где лежит \
                 arch-be-linux-x86_64"
                    .to_string(),
            ]);
        }
        CiProvider::Jenkins => {
            report.next_steps.extend([
                "закоммитьте Jenkinsfile (или перенесите блок в свой) — этап 'Spine gate' \
                 публикует находки через junit(...)"
                    .to_string(),
                "бинарь arch-be: готовый в PATH агента Jenkins или curl из релизов/бандла \
                 (варианты — в комментарии этапа)"
                    .to_string(),
            ]);
        }
    }
    Ok(report)
}

/// Общий git-каталог репозитория (`git rev-parse --git-common-dir`): в
/// worktree хуки живут в основном `.git`, поэтому голый `dir/.git` не подходит.
///
/// # Errors
/// `dir` — не git-репозиторий или git недоступен.
fn git_common_dir(dir: &Path) -> Result<PathBuf> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["rev-parse", "--git-common-dir"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .map_err(|e| HarnessError::Config(format!("git не запустился: {e}")))?;
    if !out.status.success() {
        return Err(HarnessError::Config(format!(
            "{}: не git-репозиторий — хуки ставятся только в git-проект",
            dir.display()
        )));
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let raw = text.trim();
    let path = PathBuf::from(raw);
    Ok(if path.is_absolute() {
        path
    } else {
        dir.join(path)
    })
}

/// Есть ли в конфигурации CI проекта незаменённая заглушка `<org>/<repo>`.
///
/// Читает `doctor`: молчаливая заглушка в джобе выглядит как рабочая
/// настройка, а пайплайн упадёт на первом же прогоне — предупреждать надо
/// заранее, а не по факту красного CI.
#[must_use]
pub fn ci_placeholder_present(dir: &Path) -> bool {
    [
        ".gitlab-ci.yml",
        ".github/workflows/spine-gate.yml",
        "Jenkinsfile",
    ]
    .iter()
    .filter_map(|rel| std::fs::read_to_string(dir.join(rel)).ok())
    .any(|text| text.contains("<org>/<repo>"))
}

/// Подставляет адрес релизов в шаблон джобы вместо заглушки `<org>/<repo>`.
///
/// Без `--releases-url` шаблон остаётся с заглушкой (джоба печатается как
/// черновик), но в «Следующих шагах» появляется строка, которую надо
/// отредактировать, а `doctor` предупреждает — молчаливая заглушка в CI
/// выглядит как рабочая конфигурация (Н-CI волны C 0.3.4).
fn substitute_releases_url(block: &str, releases_url: Option<&str>) -> String {
    let Some(url) = releases_url else {
        return block.to_string();
    };
    let url = url.trim().trim_end_matches('/');
    block.replace("https://github.com/<org>/<repo>/releases/download", url)
}

/// Блок pre-commit: быстрый гейт fitness-правил. Fail-soft: нет `arch-be` в
/// PATH — молча пропуск (exit 0), как хуки connect хостов; красный гейт —
/// exit 1 (коммит отменяется).
///
/// T-01: проверки `.arch-handoff/CONSTRAINTS.yaml` в шаблоне нет — реестр
/// кейса `bootstrap` лежит в корне, и прежний гард глушил хук на таких
/// проектах. Путь к реестру резолвит бинарь (корень, затем `.arch-handoff/`);
/// нет реестра нигде — внятное сообщение и ненулевой код.
fn pre_commit_hook_block() -> String {
    "# spine-connect:begin — быстрый архитектурный гейт перед коммитом (arch-be)\n\
     # Fail-soft: нет arch-be в PATH — пропуск; где лежит реестр правил, решает бинарь.\n\
     if command -v arch-be >/dev/null 2>&1; then\n\
     \x20 if ! arch-be control check .; then\n\
     \x20   echo \"spine-connect: pre-commit FAIL — исправьте находки error (отчёт выше)\" >&2\n\
     \x20   exit 1\n\
     \x20 fi\n\
     fi\n\
     # spine-connect:end"
        .to_string()
}

/// База диффа для хуков в виде shell-фрагмента: pre-push получает от git
/// строки `<local ref> <local sha> <remote ref> <remote sha>` на stdin.
///
/// Без базы гейт сравнивал бы состав правил с `HEAD`, и **уже закоммиченное**
/// ослабление правила хуком не ловилось бы (Н5): `--base HEAD~1` краснел, а
/// pre-push — нет, то есть вердикт зависел от способа вызова, а не от
/// изменения. Remote sha из нулей — новая ветка: база — точка ответвления от
/// основной ветки (`merge-base`), тот же список веток, что у
/// [`crate::control::default_anchor_base`].
const PRE_PUSH_BASE_SNIPPET: &str = "\
BASE=\"\"\n\
while read -r local_ref local_sha remote_ref remote_sha; do\n\
\x20 [ -z \"$remote_sha\" ] && continue\n\
\x20 case \"$remote_sha\" in\n\
\x20   0000000000000000000000000000000000000000)\n\
\x20     for main in origin/main main origin/master master; do\n\
\x20       if git rev-parse --verify --quiet \"$main\" >/dev/null 2>&1; then\n\
\x20         BASE=$(git merge-base \"$main\" \"$local_sha\" 2>/dev/null || true)\n\
\x20         break\n\
\x20       fi\n\
\x20     done\n\
\x20     ;;\n\
\x20   *)\n\
\x20     BASE=\"$remote_sha\"\n\
\x20     ;;\n\
\x20 esac\n\
\x20 [ -n \"$BASE\" ] && break\n\
done\n";

/// Блок pre-push: полный единый гейт (fitness + delta guard + анти-ослабление
/// правил + линтер спайна + трассировка + целостность модели; маршрут — из
/// диффа). Fail-soft при отсутствии `arch-be`; без входа гейт сам уходит в SKIP
/// и пропускает пуш.
fn pre_push_hook_block() -> String {
    format!(
        "# spine-connect:begin — полный архитектурный гейт перед пушем (arch-be)\n\
         # Fail-soft: нет arch-be в PATH — пропуск; нет входа у составляющих — SKIP внутри гейта.\n\
         # База диффа — из stdin git\'а (remote sha), иначе анти-ослабление правил\n\
         # не увидело бы УЖЕ закоммиченного ослабления (Н5).\n\
         if command -v arch-be >/dev/null 2>&1; then\n\
         {PRE_PUSH_BASE_SNIPPET}\
         \x20 if [ -n \"$BASE\" ]; then\n\
         \x20   if ! arch-be gate --route auto --base \"$BASE\"; then\n\
         \x20     echo \"spine-connect: pre-push FAIL — arch-be gate не пройден (находки выше)\" >&2\n\
         \x20     exit 1\n\
         \x20   fi\n\
         \x20 elif ! arch-be gate --route auto; then\n\
         \x20   echo \"spine-connect: pre-push FAIL — arch-be gate не пройден (находки выше)\" >&2\n\
         \x20   exit 1\n\
         \x20 fi\n\
         fi\n\
         # spine-connect:end"
    )
}

/// Право на исполнение для hook-файла (unix); вне unix — no-op.
#[cfg(unix)]
fn make_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    let mut perms = std::fs::metadata(path)
        .map_err(|e| HarnessError::io(path, e))?
        .permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(path, perms).map_err(|e| HarnessError::io(path, e))
}

/// Право на исполнение для hook-файла: вне unix не требуется.
#[cfg(not(unix))]
fn make_executable(_path: &Path) -> Result<()> {
    Ok(())
}

/// Встраивает блок в один hook-файл: новый файл — с shebang `#!/bin/sh`;
/// существующий с маркерами — замена блока; существующий без маркеров —
/// дописка (с заметкой про ранний `exit` чужого скрипта). После записи
/// файл делается исполняемым.
fn upsert_git_hook(
    hooks_dir: &Path,
    name: &str,
    block: &str,
    dry_run: bool,
    report: &mut ConnectReport,
) -> Result<()> {
    let path = hooks_dir.join(name);
    let old = match std::fs::read_to_string(&path) {
        Ok(text) => Some(text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(HarnessError::io(&path, e)),
    };
    let new = match old.as_deref() {
        None => format!("#!/bin/sh\n\n{block}\n"),
        Some(existing) => {
            if !existing.contains(BLOCK_BEGIN) {
                report.notes.push(format!(
                    "{}: существующий хук без наших маркеров — блок дописан в конец; если ваш \
                     скрипт завершается `exit`, перенесите наш блок выше него",
                    path.display()
                ));
            }
            splice_marked_block(existing, block)
        }
    };
    commit_file(&path, old.as_deref(), &new, dry_run, report)?;
    // Исполняемость гарантируем и при «без изменений»: git молча игнорирует
    // хук без +x, а connect обязан оставить рабочее состояние.
    if !dry_run && path.is_file() {
        make_executable(&path)?;
    }
    Ok(())
}

/// `arch-be connect git-hooks`: pre-commit (быстрый `control check`) и
/// pre-push (полный `gate --route auto`) в `.git/hooks` (в worktree — в
/// hooks основного git-каталога). Идемпотентно (маркерные блоки), чужие
/// строки хуков сохраняются, `--dry-run` печатает план.
///
/// # Errors
/// `dir` — не git-репозиторий; ошибки чтения/записи файлов хуков.
pub fn connect_git_hooks(dir: &Path, dry_run: bool) -> Result<ConnectReport> {
    let mut report = ConnectReport {
        dry_run,
        ..ConnectReport::default()
    };
    let hooks_dir = git_common_dir(dir)?.join("hooks");
    upsert_git_hook(
        &hooks_dir,
        "pre-commit",
        &pre_commit_hook_block(),
        dry_run,
        &mut report,
    )?;
    upsert_git_hook(
        &hooks_dir,
        "pre-push",
        &pre_push_hook_block(),
        dry_run,
        &mut report,
    )?;
    report.notes.push(
        "семантика: fail-soft на инфраструктуру (нет arch-be/ограничений — пропуск), \
         блок (exit 1) — по коду возврата arch-be; строки вывода хуки не разбирают"
            .into(),
    );
    report.next_steps.extend([
        "сделайте тестовый коммит: pre-commit прогонит `arch-be control check .`".to_string(),
        "проверьте pre-push: `git push --dry-run` (или `arch-be gate --route auto` вручную)"
            .to_string(),
    ]);
    Ok(report)
}

/// Общий рендер отчёта connect: файлы, скиллы, заметки, сниппеты, следующие
/// шаги. Заголовок произвольный — у хостов агентов он строится в
/// [`render_report`], у `connect ci`/`connect git-hooks` — свой.
#[must_use]
pub fn render_plan(title: &str, report: &ConnectReport) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "{title}");
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
        let _ = writeln!(
            out,
            "  источник: из библиотеки пользователя: {}, из встроенных: {}",
            report.skills_from_library, report.skills_from_embedded
        );
        for s in &report.skills {
            let line = match &s.action {
                SkillAction::Copied(n) => format!("скопирован ({n} файлов)"),
                SkillAction::Updated(n) => format!("обновлён ({n} файлов)"),
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

/// Рендерит отчёт команды по-русски: файлы, скиллы, заметки, сниппеты,
/// следующие шаги.
#[must_use]
pub fn render_report(opts: &ConnectOptions, report: &ConnectReport) -> String {
    render_plan(
        &format!(
            "Подключение Spine к хосту «{}» — {}",
            opts.host.name(),
            opts.dir.display()
        ),
        report,
    )
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

    /// Финальной строкой `next_steps` каждого хоста идёт «первая ценность
    /// за 5 минут» (C-1): плейбук spine-quickstart + демо-кейс drift-control.
    #[test]
    fn every_host_next_steps_end_with_first_value() {
        let tmp = tempfile::tempdir().expect("tmp");
        for (i, host) in [
            Host::Claude,
            Host::Qwen,
            Host::GigaCode,
            Host::Codex,
            Host::Kimi,
            Host::Omp,
            Host::Generic,
        ]
        .into_iter()
        .enumerate()
        {
            let dir = tmp.path().join(format!("proj-{i}"));
            std::fs::create_dir_all(&dir).expect("mkdir");
            let report = connect(&ConnectOptions::new(host, dir)).expect("connect");
            let last = report
                .next_steps
                .last()
                .unwrap_or_else(|| panic!("{host:?}: next_steps пуст"));
            assert_eq!(
                last, FIRST_VALUE_STEP,
                "{host:?}: финальная строка — «первая ценность за 5 минут»"
            );
            assert!(
                last.contains("spine-quickstart") && last.contains("drift-control"),
                "{host:?}: {last}"
            );
        }
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

    /// qwen: .qwen/settings.json (мердж mcpServers) + скиллы в
    /// .qwen/skills/ (0.24+); хуки — сниппет-референс.
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
            report.notes.iter().any(|n| n.contains(".qwen/skills")),
            "заметка про скиллы .qwen/skills: {:?}",
            report.notes
        );
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

    /// Библиотека пользователя на диске (plugins.dirs) с фикстурой: плагин
    /// без plugin.json + один скилл.
    fn user_library(root: &Path, plugin: &str, skill: &str, body: &str) -> PathBuf {
        let lib = root.join("lib");
        let skill_dir = lib.join(plugin).join("skills").join(skill);
        std::fs::create_dir_all(&skill_dir).expect("mkdir lib");
        std::fs::write(skill_dir.join("SKILL.md"), body).expect("write skill");
        lib
    }

    /// (а) Скилл, существующий только в библиотеке пользователя (плагин без
    /// манифеста), доставляется в хост; счётчик `from_library` его считает.
    #[test]
    fn skills_from_user_library_delivered_when_absent_from_embedded() {
        let tmp = tempfile::tempdir().expect("tmp");
        let lib = user_library(
            tmp.path(),
            "arch-distilled",
            "user-only-skill",
            "---\nname: user-only-skill\ndescription: свой\n---\n\n# Свой скилл\n",
        );
        let dir = tmp.path().join("proj");
        let opts = ConnectOptions {
            plugins_dirs: vec![lib],
            ..ConnectOptions::new(Host::Claude, dir.clone())
        };
        let report = connect(&opts).expect("connect");
        let delivered = dir.join(".claude/skills/user-only-skill/SKILL.md");
        assert!(delivered.is_file(), "скилл из библиотеки доставлен");
        assert!(read(&delivered).contains("Свой скилл"));
        assert_eq!(
            report.skills_from_library, 1,
            "один скилл — из библиотеки пользователя"
        );
        assert!(
            report.skills_from_embedded > 0,
            "встроенные тоже доехали (fallback)"
        );
        // Идемпотентность: повтор — без изменений.
        let report2 = connect(&opts).expect("повтор");
        assert!(
            report2
                .skills
                .iter()
                .any(|s| s.name == "user-only-skill" && s.action == SkillAction::Unchanged(1)),
            "{:?}",
            report2.skills
        );
    }

    /// (б) Дисковая копия скилла отличается от встроенной → в хост пишется
    /// дисковая версия; сводная заметка «поверх встроенных копий» — одна.
    #[test]
    fn library_copy_overrides_embedded_with_summary_note() {
        let tmp = tempfile::tempdir().expect("tmp");
        let lib = user_library(
            tmp.path(),
            "arch-core",
            "adr-authoring",
            "---\nname: adr-authoring\ndescription: пользовательская редакция\n---\n\n# Моя редакция ADR\n",
        );
        let dir = tmp.path().join("proj");
        let opts = ConnectOptions {
            plugins_dirs: vec![lib],
            ..ConnectOptions::new(Host::Claude, dir.clone())
        };
        let report = connect(&opts).expect("connect");
        let delivered = dir.join(".claude/skills/adr-authoring/SKILL.md");
        assert!(
            read(&delivered).contains("Моя редакция ADR"),
            "в хосте — версия библиотеки пользователя, не встроенная"
        );
        let overriding: Vec<_> = report
            .notes
            .iter()
            .filter(|n| n.contains("поверх встроенных копий"))
            .collect();
        assert_eq!(
            overriding.len(),
            1,
            "ровно одна сводная заметка: {:?}",
            report.notes
        );
        assert!(overriding[0].contains("1 скиллов"), "{overriding:?}");
        assert_eq!(report.skills_from_library, 1);
    }

    /// (в) Пустой plugins.dirs — поведение как раньше: все скиллы из
    /// встроенных, заметок о библиотеке нет.
    #[test]
    fn empty_plugins_dirs_keeps_embedded_only_behavior() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path().join("proj");
        let report = connect(&ConnectOptions::new(Host::Claude, dir.clone())).expect("connect");
        assert_eq!(report.skills_from_library, 0);
        assert_eq!(
            report.skills_from_embedded,
            report.skills.len(),
            "все скиллы — из встроенных: {} vs {}",
            report.skills_from_embedded,
            report.skills.len()
        );
        assert!(
            !report
                .notes
                .iter()
                .any(|n| n.contains("поверх встроенных копий")),
            "{:?}",
            report.notes
        );
    }

    /// (г) Печать результата несёт раздельные счётчики источников.
    #[test]
    fn render_prints_skill_source_counters() {
        let tmp = tempfile::tempdir().expect("tmp");
        let lib = user_library(
            tmp.path(),
            "arch-distilled",
            "user-only-skill",
            "---\nname: user-only-skill\ndescription: свой\n---\n\n# Свой\n",
        );
        let dir = tmp.path().join("proj");
        let opts = ConnectOptions {
            plugins_dirs: vec![lib],
            ..ConnectOptions::new(Host::Claude, dir.clone())
        };
        let report = connect(&opts).expect("connect");
        let text = render_report(&opts, &report);
        assert!(
            text.contains("источник: из библиотеки пользователя: 1, из встроенных:"),
            "раздельные счётчики в печати: {text}"
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

    /// Команды хуков: гард по бинарю, маркер, exit 2 по коду возврата
    /// `arch-be gate` (без разбора строк вывода).
    ///
    /// T-01: расположения реестра в shell-шаблонах НЕТ — на кейсе `bootstrap`
    /// (реестр в корне) прежний гард `[ -f .arch-handoff/CONSTRAINTS.yaml ]`
    /// глушил красный гейт. Путь резолвит бинарь.
    #[test]
    fn hook_commands_are_guarded_and_marked() {
        for cmd in [stop_hook_command(), post_tool_use_hook_command()] {
            assert!(cmd.contains("command -v arch-be"), "{cmd}");
            assert!(
                !cmd.contains("CONSTRAINTS"),
                "путь к реестру знает только бинарь (T-01): {cmd}"
            );
            assert!(cmd.contains("arch-be gate --route auto"), "{cmd}");
            assert!(
                !cmd.contains("Итог: FAIL"),
                "детекция провала — по exit-коду, не по строке: {cmd}"
            );
            assert!(cmd.contains("exit 2"), "{cmd}");
            assert!(cmd.contains(HOOK_MARKER), "{cmd}");
        }
        assert!(stop_hook_command().contains("# spine-connect:stop"));
        assert!(post_tool_use_hook_command().contains("# spine-connect:post-tool-use"));
    }

    /// T-03: шаблоны передают базу ГОЛОЙ ревизией. Готовый диапазон
    /// `rev...HEAD` гейт дополнял вторым `...HEAD`, git отказывал, и гейт
    /// молча уходил в fail-safe Critical — строгость зависела от формы записи
    /// базы, а не от изменения.
    #[test]
    fn templates_pass_a_bare_base_revision() {
        let mut blocks = vec![
            ("stop-хук", stop_hook_command()),
            ("post-tool-use", post_tool_use_hook_command()),
            ("pre-push", pre_push_hook_block()),
        ];
        for provider in [CiProvider::GitLab, CiProvider::GitHub, CiProvider::Jenkins] {
            blocks.push((provider.name(), provider.job_block()));
        }
        for (name, block) in blocks {
            assert!(
                !block.contains("...HEAD"),
                "{name}: база обязана быть голой ревизией: {block}"
            );
            // Jenkins-шаблон базу не передаёт вовсе (гейт считает дифф
            // рабочего дерева против HEAD) — это отдельная тема, не двойной
            // `...HEAD`; проверяем те шаблоны, где база есть.
            if name != "jenkins" {
                assert!(
                    block.contains("--base "),
                    "{name}: гейт вызывается с базой: {block}"
                );
            }
        }
    }

    /// T-01: та же ошибка в git-хуках и CI-шаблонах — гард по
    /// `.arch-handoff/CONSTRAINTS.yaml` глушил pre-commit на кейсе с реестром
    /// в корне. Расположение реестра в шаблонах не зашито: его резолвит
    /// бинарь (корень, затем `.arch-handoff/`).
    #[test]
    fn git_and_ci_templates_do_not_encode_registry_location() {
        for (name, block) in [
            ("pre-commit", pre_commit_hook_block()),
            ("pre-push", pre_push_hook_block()),
        ] {
            assert!(
                !block.contains("CONSTRAINTS"),
                "{name}: путь к реестру знает только бинарь: {block}"
            );
            assert!(block.contains("command -v arch-be"), "{name}: {block}");
            assert!(block.contains("spine-connect"), "{name}: {block}");
        }
        assert!(pre_commit_hook_block().contains("arch-be control check ."));
        assert!(pre_push_hook_block().contains("arch-be gate --route auto"));
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
            all_snippets.contains("arch-be gate --route auto"),
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

    /// qwen: `.qwen/settings.json` + скиллы в `.qwen/skills/` (0.24+);
    /// существующий каталог скиллов не перетирается; dry-run пустой.
    #[test]
    fn qwen_writes_settings_and_qwen_skills() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path().join("proj");
        let report = connect(&ConnectOptions::new(Host::Qwen, dir.clone())).expect("connect");

        assert_eq!(report.skills.len(), 64, "в отчёте все 64 скилла");
        let settings: Value =
            serde_json::from_str(&read(&dir.join(".qwen/settings.json"))).expect("json");
        assert_eq!(settings["mcpServers"]["spine"]["command"], "arch-be");
        assert!(
            dir.join(".qwen/skills/adr-authoring/SKILL.md").is_file(),
            "скиллы разложены в .qwen/skills"
        );

        // Существующий (даже пустой) .qwen/skills не наполняется.
        let dir2 = tmp.path().join("proj2");
        std::fs::create_dir_all(dir2.join(".qwen/skills")).expect("mkdir");
        let report = connect(&ConnectOptions::new(Host::Qwen, dir2.clone())).expect("connect");
        assert!(report.skills.is_empty(), "скиллы не раскладывались");
        assert!(dir2.join(".qwen/settings.json").is_file());

        // --dry-run: ничего не пишется.
        let dir3 = tmp.path().join("proj3");
        let opts = ConnectOptions {
            dry_run: true,
            ..ConnectOptions::new(Host::Qwen, dir3.clone())
        };
        let report = connect(&opts).expect("dry-run");
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

    /// gigacode: автоопределение каталога настроек — приоритет существующего
    /// `.gigacode/`, затем `.qwen/` (форк), иначе новый `.gigacode/`.
    #[test]
    fn gigacode_autodetect_priorities() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path().join("proj");
        // Ни одного каталога — новый .gigacode/.
        let (path, origin) = gigacode_settings_dir(&dir);
        assert_eq!(origin, GigacodeDirOrigin::NewGigacode);
        assert_eq!(path, dir.join(".gigacode"));
        // Есть только .qwen/ — пишем в него.
        std::fs::create_dir_all(dir.join(".qwen")).expect("mkdir .qwen");
        let (path, origin) = gigacode_settings_dir(&dir);
        assert_eq!(origin, GigacodeDirOrigin::FromQwen);
        assert_eq!(path, dir.join(".qwen"));
        // Есть оба — приоритет .gigacode/.
        std::fs::create_dir_all(dir.join(".gigacode")).expect("mkdir .gigacode");
        let (path, origin) = gigacode_settings_dir(&dir);
        assert_eq!(origin, GigacodeDirOrigin::ExistingGigacode);
        assert_eq!(path, dir.join(".gigacode"));
    }

    /// gigacode в пустом проекте: создаётся `.gigacode/settings.json` (мердж
    /// mcpServers.spine), скиллы — в `.gigacode/skills/` (как у claude, с
    /// обновлением встроенных); хуки — только сниппет; заметка про создание
    /// каталога; повтор идемпотентен.
    #[test]
    fn gigacode_scaffolds_new_settings_dir() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path().join("proj");
        std::fs::create_dir_all(&dir).expect("mkdir proj");
        let report = connect(&ConnectOptions::new(Host::GigaCode, dir.clone())).expect("connect");

        let settings: Value =
            serde_json::from_str(&read(&dir.join(".gigacode/settings.json"))).expect("json");
        assert_eq!(settings["mcpServers"]["spine"]["command"], "arch-be");
        assert_eq!(
            settings["mcpServers"]["spine"]["args"],
            json!(["mcp", "serve"])
        );
        assert!(
            dir.join(".gigacode/skills/adr-authoring/SKILL.md")
                .is_file(),
            "скиллы разложены в .gigacode/skills"
        );
        assert!(!dir.join(".qwen").exists(), "каталога .qwen не появилось");
        assert!(
            report
                .notes
                .iter()
                .any(|n| n.contains("создан") && n.contains(".gigacode")),
            "заметка про создание каталога: {:?}",
            report.notes
        );
        assert!(
            report.snippets.iter().any(|(t, _)| t.contains("Хуки")),
            "сниппет хуков напечатан: {:?}",
            report.snippets
        );
        assert!(
            report
                .next_steps
                .iter()
                .any(|s| s.contains("doctor --host gigacode")),
            "{:?}",
            report.next_steps
        );

        // Идемпотентность: повторный прогон побайтово тот же.
        let first = snapshot(&dir);
        let report = connect(&ConnectOptions::new(Host::GigaCode, dir.clone())).expect("повтор");
        assert_eq!(first, snapshot(&dir), "повторный запуск изменил файлы");
        assert!(report.created.is_empty() && report.merged.is_empty());
    }

    /// gigacode в проекте с существующим `.qwen/`: пишет в него (чужой сервер
    /// сохранён), `.gigacode/` не создаётся; существующий чужой скилл в
    /// `.qwen/skills/` не трогается, встроенные докладываются рядом.
    #[test]
    fn gigacode_reuses_existing_qwen_dir() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path().join("proj");
        std::fs::create_dir_all(dir.join(".qwen/skills/foreign")).expect("mkdir");
        std::fs::write(
            dir.join(".qwen/settings.json"),
            "{\n  \"mcpServers\": {\n    \"other\": {\"command\": \"uvx\", \"args\": [\"x\"]}\n  }\n}\n",
        )
        .expect("write settings");
        std::fs::write(
            dir.join(".qwen/skills/foreign/SKILL.md"),
            "---\nname: foreign\ndescription: чужой\n---\n",
        )
        .expect("write foreign skill");

        let report = connect(&ConnectOptions::new(Host::GigaCode, dir.clone())).expect("connect");

        let settings: Value =
            serde_json::from_str(&read(&dir.join(".qwen/settings.json"))).expect("json");
        assert_eq!(settings["mcpServers"]["spine"]["command"], "arch-be");
        assert_eq!(
            settings["mcpServers"]["other"]["command"], "uvx",
            "чужой сервер цел"
        );
        assert!(
            !dir.join(".gigacode").exists(),
            ".gigacode не создаётся при живом .qwen"
        );
        assert!(
            report
                .notes
                .iter()
                .any(|n| n.contains(".qwen/") && n.contains("совместим")),
            "заметка про наследование .qwen: {:?}",
            report.notes
        );
        assert_eq!(
            read(&dir.join(".qwen/skills/foreign/SKILL.md")),
            "---\nname: foreign\ndescription: чужой\n---\n",
            "чужой скилл не тронут"
        );
        assert!(
            dir.join(".qwen/skills/adr-authoring/SKILL.md").is_file(),
            "встроенные скиллы доложены в .qwen/skills"
        );
    }

    /// gigacode --dry-run: только план, ничего не создаётся.
    #[test]
    fn gigacode_dry_run_writes_nothing() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path().join("proj");
        std::fs::create_dir_all(&dir).expect("mkdir proj");
        let opts = ConnectOptions {
            dry_run: true,
            ..ConnectOptions::new(Host::GigaCode, dir.clone())
        };
        let report = connect(&opts).expect("dry-run");
        assert!(!report.created.is_empty(), "план непустой");
        assert!(!report.skills.is_empty(), "план по скиллам непустой");
        assert!(
            report.notes.iter().any(|n| n.contains("будет создан")),
            "заметка про будущее создание каталога: {:?}",
            report.notes
        );
        assert_eq!(
            std::fs::read_dir(&dir).expect("read dir").count(),
            0,
            "dry-run ничего не записал"
        );
    }

    /// Разбор имён хостов: gigacode и его алиасы.
    #[test]
    fn host_parse_accepts_gigacode_aliases() {
        assert_eq!(Host::parse("gigacode"), Ok(Host::GigaCode));
        assert_eq!(Host::parse("GigaCode"), Ok(Host::GigaCode));
        assert_eq!(Host::parse("giga-code"), Ok(Host::GigaCode));
        assert_eq!(Host::parse("gcode"), Ok(Host::GigaCode));
        assert_eq!(Host::GigaCode.name(), "gigacode");
        let err = Host::parse("cursor").expect_err("неизвестный хост");
        assert!(err.contains("gigacode"), "{err}");
    }

    // --- connect ci / connect git-hooks (волна 2, п.8) ----------------------

    /// git в каталоге с тестовой идентичностью коммиттера (для git-hooks).
    fn git(dir: &Path, args: &[&str]) {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .output()
            .expect("git");
        assert!(
            out.status.success(),
            "git {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// Разбор CI-провайдеров: алиасы, регистр, ошибка со списком допустимых.
    #[test]
    fn ci_provider_parse_accepts_aliases_and_rejects_unknown() {
        assert_eq!(CiProvider::parse("gitlab"), Ok(CiProvider::GitLab));
        assert_eq!(CiProvider::parse("GitHub"), Ok(CiProvider::GitHub));
        assert_eq!(CiProvider::parse("jenkins"), Ok(CiProvider::Jenkins));
        let err = CiProvider::parse("gitlab-ci").expect_err("неизвестный провайдер");
        assert!(err.contains("gitlab"), "{err}");
        assert!(err.contains("jenkins"), "{err}");
    }

    /// Джобы всех трёх провайдеров: файл с маркерами и нативной командой
    /// гейта; сухой прогон ничего не пишет; повтор — без дублей.
    #[test]
    fn ci_scaffolds_job_per_provider_idempotently() {
        for (provider, rel, native) in [
            (
                CiProvider::GitLab,
                ".gitlab-ci.yml",
                "--format gitlab-codequality",
            ),
            (
                CiProvider::GitHub,
                ".github/workflows/spine-gate.yml",
                "--format sarif",
            ),
            (CiProvider::Jenkins, "Jenkinsfile", "--format junit"),
        ] {
            let tmp = tempfile::tempdir().expect("tmp");
            let dir = tmp.path().join("proj");
            std::fs::create_dir_all(&dir).expect("mkdir");

            // --dry-run: план есть, файла нет.
            let report = connect_ci(provider, &dir, true, None).expect("dry-run");
            assert!(report.dry_run);
            assert!(
                report.created.iter().any(|p| p.ends_with(rel)),
                "{}: план без {rel}: {:?}",
                provider.name(),
                report.created
            );
            assert!(
                !dir.join(rel).exists(),
                "{}: dry-run записал файл",
                provider.name()
            );

            // Реальный прогон.
            let report = connect_ci(provider, &dir, false, None).expect("connect ci");
            let text = read(&dir.join(rel));
            assert!(text.contains(BLOCK_BEGIN), "{}: {text}", provider.name());
            assert!(text.contains(BLOCK_END), "{}: {text}", provider.name());
            assert!(
                text.contains("arch-be gate --route auto"),
                "{}: {text}",
                provider.name()
            );
            assert!(
                text.contains(native),
                "{}: нативный формат {native}: {text}",
                provider.name()
            );
            assert!(
                text.contains("arch-be-linux-x86_64"),
                "{}: установка бинаря: {text}",
                provider.name()
            );
            assert!(
                report.created.iter().any(|p| p.ends_with(rel)),
                "{:?}",
                report.created
            );

            // Повтор — без изменений и без дублей маркеров.
            let report = connect_ci(provider, &dir, false, None).expect("повтор");
            assert_eq!(
                read(&dir.join(rel)),
                text,
                "{}: повтор изменил файл",
                provider.name()
            );
            assert!(
                report.unchanged.iter().any(|p| p.ends_with(rel)),
                "{}: {:?}",
                provider.name(),
                report.unchanged
            );
            assert_eq!(text.matches(BLOCK_BEGIN).count(), 1, "дубль маркера");
        }
    }

    /// GitLab: существующий `.gitlab-ci.yml` с чужой джобой — блок дописывается,
    /// чужое сохраняется; повтор заменяет блок без дублей.
    #[test]
    fn ci_gitlab_merges_into_existing_pipeline() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path().join("proj");
        std::fs::create_dir_all(&dir).expect("mkdir");
        std::fs::write(
            dir.join(".gitlab-ci.yml"),
            "stages: [test]\n\nunit-tests:\n  stage: test\n  script: cargo test\n",
        )
        .expect("write .gitlab-ci.yml");

        connect_ci(CiProvider::GitLab, &dir, false, None).expect("connect ci");
        let text = read(&dir.join(".gitlab-ci.yml"));
        assert!(text.contains("unit-tests:"), "чужая джоба цела: {text}");
        assert!(text.contains("spine-gate:"), "наша джоба: {text}");
        assert!(
            text.contains("codequality: codequality-spine.json"),
            "{text}"
        );

        // Повтор: блок заменяется, чужая зона не трогается, дублей нет.
        connect_ci(CiProvider::GitLab, &dir, false, None).expect("повтор");
        let again = read(&dir.join(".gitlab-ci.yml"));
        assert_eq!(again, text, "повтор изменил файл");
        assert_eq!(again.matches("spine-gate:").count(), 1, "{again}");
    }

    /// GitHub: существующий workflow без нашего маркера — отказ без затирания;
    /// с маркером — обновление блока.
    #[test]
    fn ci_github_refuses_foreign_workflow_file() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path().join("proj");
        let wf = dir.join(".github/workflows/spine-gate.yml");
        std::fs::create_dir_all(wf.parent().expect("parent")).expect("mkdir");
        std::fs::write(&wf, "name: mine\non: [push]\n").expect("write workflow");

        let err = connect_ci(CiProvider::GitHub, &dir, false, None).expect_err("отказ");
        assert!(err.to_string().contains("не затираю"), "{err}");
        assert_eq!(read(&wf), "name: mine\non: [push]\n", "файл цел");

        // Наш файл (с маркером) обновляется.
        std::fs::write(
            &wf,
            "# spine-connect:begin\nname: spine-gate\n# spine-connect:end\n",
        )
        .expect("write marked");
        connect_ci(CiProvider::GitHub, &dir, false, None).expect("обновление нашего файла");
        let text = read(&wf);
        assert!(text.contains("--format sarif"), "{text}");
        assert_eq!(text.matches(BLOCK_BEGIN).count(), 1, "{text}");
    }

    /// Маркерный сплайс: замена зоны между маркерами, рукописное снаружи цело.
    #[test]
    fn splice_marked_block_preserves_handwritten_zones() {
        let block = "# spine-connect:begin\nA\n# spine-connect:end";
        let existing =
            "голова\n\n# spine-connect:begin\nстарая зона\n# spine-connect:end\n\nхвост\n";
        let out = splice_marked_block(existing, block);
        assert!(out.contains("голова"), "{out}");
        assert!(out.contains("хвост"), "{out}");
        assert!(out.contains("\nA\n"), "{out}");
        assert!(!out.contains("старая зона"), "{out}");
        // Повтор детерминирован.
        assert_eq!(splice_marked_block(&out, block), out);
        // Без маркеров — дописка.
        let appended = splice_marked_block("чужое\n", block);
        assert!(appended.starts_with("чужое\n\n"), "{appended}");
        assert!(appended.contains(BLOCK_BEGIN), "{appended}");
    }

    /// git-hooks: pre-commit и pre-push с маркерами, исполняемые, fail-soft
    /// гарды; повтор без дублей; чужой хук не затирается.
    #[test]
    fn git_hooks_scaffold_in_git_repo_idempotently() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path().join("repo");
        std::fs::create_dir_all(&dir).expect("mkdir");
        git(&dir, &["init", "-q"]);
        // Чужой pre-push уже есть — не затираем.
        std::fs::write(dir.join(".git/hooks/pre-push"), "#!/bin/sh\necho mine\n")
            .expect("чужой pre-push");

        let report = connect_git_hooks(&dir, false).expect("connect git-hooks");
        let pre_commit = read(&dir.join(".git/hooks/pre-commit"));
        assert!(pre_commit.starts_with("#!/bin/sh\n"), "{pre_commit}");
        assert!(pre_commit.contains(BLOCK_BEGIN), "{pre_commit}");
        assert!(
            pre_commit.contains("command -v arch-be"),
            "fail-soft гард: {pre_commit}"
        );
        assert!(
            pre_commit.contains("arch-be control check ."),
            "быстрый гейт: {pre_commit}"
        );
        let pre_push = read(&dir.join(".git/hooks/pre-push"));
        assert!(pre_push.contains("echo mine"), "чужой хук цел: {pre_push}");
        assert!(
            pre_push.contains("arch-be gate --route auto"),
            "полный гейт: {pre_push}"
        );
        assert!(
            report
                .notes
                .iter()
                .any(|n| n.contains("pre-push") && n.contains("дописан")),
            "заметка про чужой хук: {:?}",
            report.notes
        );

        // Исполняемость (unix): git молча игнорирует хук без +x.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            for hook in ["pre-commit", "pre-push"] {
                let mode = std::fs::metadata(dir.join(".git/hooks").join(hook))
                    .expect("stat")
                    .permissions()
                    .mode();
                assert_eq!(mode & 0o111, 0o111, "{hook} не исполняемый");
            }
        }

        // Повтор: побайтово то же, маркеров по одному.
        let first = snapshot(&dir);
        connect_git_hooks(&dir, false).expect("повтор");
        assert_eq!(first, snapshot(&dir), "повторный запуск изменил файлы");
        assert_eq!(
            read(&dir.join(".git/hooks/pre-push"))
                .matches(BLOCK_BEGIN)
                .count(),
            1,
            "дубль маркера"
        );

        // --dry-run поверх — ничего не пишет.
        let report = connect_git_hooks(&dir, true).expect("dry-run");
        assert!(report.dry_run);
        assert_eq!(first, snapshot(&dir), "dry-run что-то записал");
    }

    /// git-hooks вне git-репозитория — понятная ошибка.
    #[test]
    fn git_hooks_outside_git_repo_is_error() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path().join("plain");
        std::fs::create_dir_all(&dir).expect("mkdir");
        let err = connect_git_hooks(&dir, false).expect_err("не git");
        assert!(err.to_string().contains("не git-репозиторий"), "{err}");
    }

    /// git-hooks в worktree: хуки кладутся в hooks ОСНОВНОГО git-каталога
    /// (`git rev-parse --git-common-dir`), а не в файл-указатель worktree.
    #[test]
    fn git_hooks_in_worktree_target_common_dir() {
        let tmp = tempfile::tempdir().expect("tmp");
        let main = tmp.path().join("main");
        std::fs::create_dir_all(&main).expect("mkdir");
        git(&main, &["init", "-q"]);
        std::fs::write(main.join("f"), "x").expect("write f");
        git(&main, &["add", "."]);
        git(&main, &["commit", "-q", "-m", "init"]);
        let wt = tmp.path().join("wt");
        git(
            &main,
            &["worktree", "add", "-q", wt.to_str().expect("utf8")],
        );

        connect_git_hooks(&wt, false).expect("connect git-hooks в worktree");
        assert!(
            main.join(".git/hooks/pre-commit").is_file(),
            "хук в основном .git"
        );
        assert!(
            !wt.join(".git/hooks").exists(),
            "у worktree .git — файл, каталога hooks в нём нет"
        );
    }
}

#[cfg(test)]
mod tests_releases_url {
    //! Волна C 0.3.4: заглушка `<org>/<repo>` в шаблоне CI не должна выглядеть
    //! рабочей конфигурацией.

    use super::*;

    fn ci_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "arch-be-ci-{name}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos())
        ));
        std::fs::create_dir_all(&dir).expect("mkdir");
        dir
    }

    /// `--releases-url` подставляется в джобу; заглушки в файле не остаётся.
    #[test]
    fn releases_url_is_substituted_into_ci_job() {
        let dir = ci_dir("url");
        connect_ci(
            CiProvider::GitLab,
            &dir,
            false,
            Some("https://releases.example.invalid/arch-be/"),
        )
        .expect("connect ci");
        let text = std::fs::read_to_string(dir.join(".gitlab-ci.yml")).expect("read");
        assert!(
            text.contains(r#"RELEASES_URL: "https://releases.example.invalid/arch-be""#),
            "адрес обязан быть подставлен без хвостового слэша: {text}"
        );
        assert!(
            !text.contains("<org>/<repo>"),
            "заглушки в джобе остаться не должно: {text}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Без адреса — заглушка остаётся, но «Следующие шаги» прямо называют
    /// команду с `--releases-url`, а не молчат.
    #[test]
    fn missing_releases_url_is_named_in_next_steps() {
        let dir = ci_dir("no-url");
        let report = connect_ci(CiProvider::GitLab, &dir, false, None).expect("connect ci");
        assert!(
            report
                .next_steps
                .iter()
                .any(|s| s.contains("--releases-url")),
            "шаг обязан называть команду: {:?}",
            report.next_steps
        );
        // Проверка для doctor/CI-файла: заглушка видна.
        assert!(super::ci_placeholder_present(&dir));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
