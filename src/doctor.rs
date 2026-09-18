//! Диагностика окружения харнесса (по опыту Claude Code `/doctor` и Theseus
//! `doctor.rs`): проверки ДО того, как архитектор начнёт сессию, — ключи,
//! каталоги, плагины, кодовые харнессы в PATH, MCP, крон. Платные
//! chat-эндпоинты не вызываются (дорого и шумно); проверяется только то,
//! что видно локально.
//!
//! КОНТРАКТ (владелец: агент `agent`):
//! - [`run_checks`] — чистое ядро: список [`Check`] с вердиктами;
//! - [`run_host_checks`] — точечная проверка подключения `connect <host>`
//!   (`arch-be doctor --host <host>`): бинарь arch-be в PATH, файл настроек
//!   хоста с `mcpServers.spine`, скиллы на месте (для записываемых хостов),
//!   версия хоста по `<бинарь> --version`;
//! - [`render`] / [`render_host`] — текстовый отчёт (иконки ✓/⚠/✗);
//!   [`exit_code`] — 1 при хотя бы одном Fail (для CLI `arch-be doctor`);
//! - проверки не мутируют состояние, кроме временного файла в `sessions_dir`
//!   (создаётся и тут же удаляется).

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use crate::config::Config;
use crate::connect::Host;

/// Вердикт одной проверки.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Всё в порядке.
    Ok,
    /// Работать можно, но стоит обратить внимание.
    Warn,
    /// Критично: часть функций недоступна.
    Fail,
}

impl Verdict {
    /// Иконка вердикта для отчёта.
    #[must_use]
    pub const fn icon(self) -> &'static str {
        match self {
            Self::Ok => "✓",
            Self::Warn => "⚠",
            Self::Fail => "✗",
        }
    }
}

/// Результат одной проверки окружения.
#[derive(Debug, Clone)]
pub struct Check {
    /// Короткое имя проверки (для колонки отчёта).
    pub name: &'static str,
    /// Вердикт.
    pub verdict: Verdict,
    /// Пояснение (что проверено / что не так / как чинить).
    pub text: String,
}

/// Все проверки окружения по конфигу. Чистая функция — без вывода.
#[must_use]
pub fn run_checks(cfg: &Config) -> Vec<Check> {
    vec![
        check_default_model(cfg),
        check_api_keys(cfg),
        check_sessions_dir(cfg),
        check_plugins(cfg),
        check_knowledge(cfg),
        check_harnesses(cfg),
        check_mcp(cfg),
        check_cron(cfg),
        check_web(cfg),
        check_archify(cfg),
        check_git(),
    ]
}

/// Текстовый отчёт по списку проверок.
#[must_use]
pub fn render(checks: &[Check]) -> String {
    render_with_title("arch-be doctor — диагностика окружения", checks)
}

/// Текстовый отчёт `doctor --host`: те же иконки/итог, свой заголовок.
#[must_use]
pub fn render_host(host: Host, checks: &[Check]) -> String {
    render_with_title(
        &format!(
            "arch-be doctor --host {} — проверка подключения",
            host.name()
        ),
        checks,
    )
}

/// Общий рендер отчёта с заданным заголовком.
#[must_use]
fn render_with_title(title: &str, checks: &[Check]) -> String {
    let mut out = format!("{title}\n\n");
    for c in checks {
        let _ = writeln!(out, "  {} {:<14} {}", c.verdict.icon(), c.name, c.text);
    }
    let fails = checks.iter().filter(|c| c.verdict == Verdict::Fail).count();
    let warns = checks.iter().filter(|c| c.verdict == Verdict::Warn).count();
    let _ = writeln!(out);
    if fails == 0 && warns == 0 {
        let _ = writeln!(out, "Итог: здоров ({} проверок)", checks.len());
    } else {
        let _ = writeln!(
            out,
            "Итог: {fails} проблем(ы), {warns} предупреждений из {} проверок",
            checks.len()
        );
    }
    out
}

/// Код выхода CLI: 1 при любом Fail, иначе 0.
#[must_use]
pub fn exit_code(checks: &[Check]) -> i32 {
    i32::from(checks.iter().any(|c| c.verdict == Verdict::Fail))
}

/// Точечная проверка подключения `connect <host>` (`arch-be doctor --host`):
/// бинарь `arch-be` в PATH (MCP-сервер стартует у хоста командой `arch-be`),
/// файл настроек хоста несёт `mcpServers.spine` с правильной командой,
/// скиллы на месте (для хостов, куда connect их пишет), версия хоста —
/// по `<бинарь> --version`, если бинарь находится.
///
/// Проверки ничего не мутируют. `project_dir` — каталог проекта, куда
/// делался connect (для codex и пользовательского уровня kimi смотрится
/// `home`).
#[must_use]
pub fn run_host_checks(host: Host, project_dir: &Path, home: Option<&Path>) -> Vec<Check> {
    let mut checks = vec![check_arch_be_binary()];
    match host {
        // claude и omp делят layout: проектный `.mcp.json` + `.claude/skills/`.
        Host::Claude | Host::Omp => {
            checks.push(check_json_mcp_settings(&project_dir.join(".mcp.json")));
            checks.push(check_skills_dir(&project_dir.join(".claude/skills")));
        }
        Host::Qwen => {
            checks.push(check_json_mcp_settings(
                &project_dir.join(".qwen/settings.json"),
            ));
            checks.push(check_skills_dir(&project_dir.join(".qwen/skills")));
        }
        Host::GigaCode => {
            let (dir, origin) = crate::connect::gigacode_settings_dir(project_dir);
            let mut settings = check_json_mcp_settings(&dir.join("settings.json"));
            // Видно, куда смотрит проверка: автоопределение как у connect.
            settings.text = format!(
                "{} [каталог: {}]",
                settings.text,
                match origin {
                    crate::connect::GigacodeDirOrigin::ExistingGigacode => ".gigacode/",
                    crate::connect::GigacodeDirOrigin::FromQwen => ".qwen/ (совместимость форка)",
                    crate::connect::GigacodeDirOrigin::NewGigacode => ".gigacode/ (ещё не создан)",
                }
            );
            checks.push(settings);
            checks.push(check_skills_dir(&dir.join("skills")));
        }
        Host::Codex => {
            let path = home.map(|h| h.join(".codex/config.toml"));
            checks.push(check_codex_settings(path.as_deref()));
        }
        Host::Kimi => {
            let project = project_dir.join(".kimi-code/mcp.json");
            let user = home.map(|h| h.join(".kimi-code/mcp.json"));
            checks.push(check_kimi_settings(&project, user.as_deref()));
        }
        Host::Generic => checks.push(Check {
            name: "settings",
            verdict: Verdict::Warn,
            text: "generic не пишет файлы — проверять нечего; сверьте установку со \
                   сниппетами `arch-be connect generic`"
                .into(),
        }),
    }
    if let Some(binary) = host_binary(host) {
        checks.push(check_host_version(binary));
    }
    checks
}

/// Имя бинаря хоста для проверки версии (None у generic — его нет).
fn host_binary(host: Host) -> Option<&'static str> {
    match host {
        Host::Claude => Some("claude"),
        Host::Qwen => Some("qwen"),
        Host::GigaCode => Some("gigacode"),
        Host::Codex => Some("codex"),
        Host::Kimi => Some("kimi"),
        Host::Omp => Some("omp"),
        Host::Generic => None,
    }
}

/// Бинарь `arch-be` доступен в PATH: без него MCP-сервер из записи
/// `mcpServers.spine` (command = "arch-be") у хоста не стартует.
fn check_arch_be_binary() -> Check {
    let ok = binary_in_path("arch-be");
    Check {
        name: "arch-be",
        verdict: if ok { Verdict::Ok } else { Verdict::Fail },
        text: if ok {
            "в PATH — MCP-сервер хоста стартанёт".into()
        } else {
            "не найден в PATH: запись mcpServers.spine ссылается на команду `arch-be` — \
             установите бинарь (релиз/бандл) в ~/.local/bin или добавьте его каталог в PATH"
                .into()
        },
    }
}

/// JSON-файл настроек хоста (`.mcp.json` claude/omp, `.qwen/settings.json`,
/// `.kimi-code/mcp.json`): содержит `mcpServers.spine` с командой `arch-be`.
fn check_json_mcp_settings(path: &Path) -> Check {
    let parsed = std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok());
    let Some(v) = parsed else {
        return Check {
            name: "settings",
            verdict: Verdict::Fail,
            text: format!(
                "{} — не найден или не JSON: хост не подключён (лечение: \
                 `arch-be connect <host>` в корне проекта)",
                path.display()
            ),
        };
    };
    check_spine_server_spec(&v, path)
}

/// Проверка спеки `mcpServers.spine` в разобранном JSON настроек.
fn check_spine_server_spec(v: &serde_json::Value, path: &Path) -> Check {
    let Some(spec) = v
        .get("mcpServers")
        .and_then(|s| s.as_object())
        .and_then(|m| m.get("spine"))
    else {
        return Check {
            name: "settings",
            verdict: Verdict::Fail,
            text: format!(
                "{}: нет ключа mcpServers.spine — хост не подключён \
                 (`arch-be connect <host>`)",
                path.display()
            ),
        };
    };
    let command = spec.get("command").and_then(|c| c.as_str()).unwrap_or("");
    if command != "arch-be" {
        return Check {
            name: "settings",
            verdict: Verdict::Fail,
            text: format!(
                "{}: mcpServers.spine.command = «{command}» вместо «arch-be» — \
                 перезапишите: `arch-be connect <host>` (мердж сохранит чужие ключи)",
                path.display()
            ),
        };
    }
    let args: Vec<&str> = spec
        .get("args")
        .and_then(|a| a.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_str()).collect())
        .unwrap_or_default();
    let (verdict, mode) = match args.as_slice() {
        ["mcp", "serve"] => (Verdict::Ok, "read-only"),
        ["mcp", "serve", "--rw"] => (Verdict::Ok, "rw"),
        other => (
            Verdict::Warn,
            // Нестандартные args — показываем как есть (может быть осознанная
            // кастомизация; connect пишет ровно `mcp serve [--rw]`).
            if other.is_empty() {
                "без args (ожидалось mcp serve)"
            } else {
                "нестандартные args (ожидалось mcp serve [--rw])"
            },
        ),
    };
    Check {
        name: "settings",
        verdict,
        text: format!(
            "{}: mcpServers.spine → arch-be mcp serve ({mode})",
            path.display()
        ),
    }
}

/// Настройки Codex: `[mcp_servers.spine]` в `~/.codex/config.toml`.
fn check_codex_settings(path: Option<&Path>) -> Check {
    let Some(path) = path else {
        return Check {
            name: "settings",
            verdict: Verdict::Warn,
            text: "домашний каталог не определён — ~/.codex/config.toml не проверить".into(),
        };
    };
    let parsed = std::fs::read_to_string(path)
        .ok()
        .and_then(|t| toml::from_str::<toml::Value>(&t).ok());
    let Some(doc) = parsed else {
        return Check {
            name: "settings",
            verdict: Verdict::Fail,
            text: format!(
                "{} — не найден или не TOML: сервер не зарегистрирован (запись: \
                 `arch-be connect codex --apply-global` либо блок из `arch-be connect codex`)",
                path.display()
            ),
        };
    };
    let command = doc
        .get("mcp_servers")
        .and_then(|s| s.get("spine"))
        .and_then(|s| s.get("command"))
        .and_then(|c| c.as_str());
    match command {
        Some("arch-be") => Check {
            name: "settings",
            verdict: Verdict::Ok,
            text: format!(
                "{}: [mcp_servers.spine] → arch-be mcp serve",
                path.display()
            ),
        },
        Some(other) => Check {
            name: "settings",
            verdict: Verdict::Fail,
            text: format!(
                "{}: mcp_servers.spine.command = «{other}» вместо «arch-be»",
                path.display()
            ),
        },
        None => Check {
            name: "settings",
            verdict: Verdict::Fail,
            text: format!(
                "{}: нет секции [mcp_servers.spine] — сервер не зарегистрирован",
                path.display()
            ),
        },
    }
}

/// Настройки Kimi Code: проектный `.kimi-code/mcp.json` в приоритете
/// (перекрывает пользовательский), иначе пользовательский уровень.
fn check_kimi_settings(project: &Path, user: Option<&Path>) -> Check {
    if project.is_file() {
        return check_json_mcp_settings(project);
    }
    if let Some(user) = user.filter(|p| p.is_file()) {
        let mut check = check_json_mcp_settings(user);
        if check.verdict == Verdict::Ok {
            check.text = format!(
                "{} (пользовательский уровень; проектный {} не найден)",
                check.text,
                project.display()
            );
        }
        return check;
    }
    Check {
        name: "settings",
        verdict: Verdict::Fail,
        text: format!(
            "ни проектного {}, ни пользовательского ~/.kimi-code/mcp.json — \
             хост не подключён (`arch-be connect kimi`)",
            project.display()
        ),
    }
}

/// Скиллы хоста на месте: каталог существует и несёт хотя бы один SKILL.md.
/// Отсутствие — Warn, не Fail: MCP-контур работает и без скиллов (а у qwen/
/// omp connect осознанно не трогает уже существующий каталог скиллов).
fn check_skills_dir(dir: &Path) -> Check {
    let skills = count_files_named(dir, "SKILL.md", 3);
    Check {
        name: "skills",
        verdict: if skills > 0 {
            Verdict::Ok
        } else {
            Verdict::Warn
        },
        text: if skills > 0 {
            format!("{}: {skills} скиллов", dir.display())
        } else {
            format!(
                "{}: скиллов нет (раскладка: `arch-be connect <host>`; если каталог \
                 существовал до connect — встроенные не перетирались, скопируйте из \
                 ~/.arch-harness/plugins вручную)",
                dir.display()
            )
        },
    }
}

/// Версия хоста по `<бинарь> --version` (первая строка). Бинарь не найден —
/// Warn: хост может быть установлен под другим именем или на другой машине.
fn check_host_version(binary: &str) -> Check {
    match binary_version(binary) {
        Some(version) => Check {
            name: "host",
            verdict: Verdict::Ok,
            text: format!("{binary} — {version}"),
        },
        None if binary_in_path(binary) => Check {
            name: "host",
            verdict: Verdict::Warn,
            text: format!("{binary} в PATH, но `{binary} --version` не ответил"),
        },
        None => Check {
            name: "host",
            verdict: Verdict::Warn,
            text: format!(
                "{binary} не найден в PATH — версия не определяется (проверьте установку хоста)"
            ),
        },
    }
}

/// Первая строка вывода `<bin> --version` (None при любой ошибке запуска).
fn binary_version(bin: &str) -> Option<String> {
    let out = std::process::Command::new(bin)
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    text.lines().next().map(|l| l.trim().to_string())
}

/// `default_model` присутствует в реестре моделей.
fn check_default_model(cfg: &Config) -> Check {
    let ok = cfg.models.contains_key(&cfg.default_model);
    Check {
        name: "default_model",
        verdict: if ok { Verdict::Ok } else { Verdict::Fail },
        text: if ok {
            format!(
                "«{}» → {}",
                cfg.default_model, cfg.models[&cfg.default_model].model
            )
        } else {
            format!(
                "«{}» не найдена в [models] (есть: {})",
                cfg.default_model,
                cfg.models.keys().cloned().collect::<Vec<_>>().join(", ")
            )
        },
    }
}

/// API-ключи: env-переменные моделей установлены (значения не выводим!).
/// Ключ считается доступным и по `api_key_file` (запасной файл с ключом).
/// Модели с `kind = "cli"` (внешний авторизованный CLI-агент как провайдер,
/// `llm::harness_cli`) ключа не требуют и в проверке не участвуют.
/// В core-сборке сетевых провайдеров нет вовсе (собрано без reqwest):
/// отсутствие ключа — Warn с подсказкой, а не Fail.
fn check_api_keys(cfg: &Config) -> Check {
    let key_available = |mc: &crate::config::ModelConfig| {
        if std::env::var_os(&mc.api_key_env).is_some_and(|v| !v.is_empty()) {
            return true;
        }
        mc.api_key_file.as_deref().is_some_and(|path| {
            let expanded = match path.strip_prefix("~/") {
                Some(rest) => {
                    dirs::home_dir().map_or_else(|| PathBuf::from(path), |h| h.join(rest))
                }
                None => PathBuf::from(path),
            };
            std::fs::metadata(&expanded).is_ok_and(|m| m.len() > 0)
        })
    };
    // kind = "cli" — провайдер без собственного ключа: внешний CLI уже
    // авторизован на машине (платит подписка хоста).
    let keyed: Vec<(&String, &crate::config::ModelConfig)> = cfg
        .models
        .iter()
        .filter(|(_, mc)| mc.kind.as_deref() != Some("cli"))
        .collect();
    let mut missing = Vec::new();
    for (name, mc) in &keyed {
        if !key_available(mc) {
            missing.push(format!("{name} ({})", mc.api_key_env));
        }
    }
    let cli_count = cfg.models.len() - keyed.len();
    let cli_note = if cli_count > 0 {
        format!("; cli-провайдеров без ключа: {cli_count} (норма)")
    } else {
        String::new()
    };
    let default_missing = keyed
        .iter()
        .any(|(n, mc)| *n == &cfg.default_model && !key_available(mc));
    let (verdict, text) = if missing.is_empty() {
        (
            Verdict::Ok,
            format!("все {} ключей на месте{cli_note}", keyed.len()),
        )
    } else if default_missing && cfg!(feature = "harness") {
        (
            Verdict::Fail,
            format!(
                "нет ключа модели по умолчанию; отсутствуют: {}{cli_note}",
                missing.join(", ")
            ),
        )
    } else if cfg!(not(feature = "harness")) {
        (
            Verdict::Warn,
            format!(
                "core-сборка без сетевых провайдеров: ключи не требуются (отсутствуют: {}){cli_note}",
                missing.join(", ")
            ),
        )
    } else {
        (
            Verdict::Warn,
            format!(
                "нет части ключей (нужны только при выборе модели): {}{cli_note}",
                missing.join(", ")
            ),
        )
    };
    Check {
        name: "api-keys",
        verdict,
        text,
    }
}

/// Каталог сессий: создаётся и доступен на запись.
fn check_sessions_dir(cfg: &Config) -> Check {
    let dir = &cfg.paths.sessions_dir;
    let probe = dir.join(".doctor-probe");
    let result = std::fs::create_dir_all(dir)
        .and_then(|()| std::fs::write(&probe, b"ok"))
        .and_then(|()| std::fs::remove_file(&probe));
    Check {
        name: "sessions",
        verdict: if result.is_ok() {
            Verdict::Ok
        } else {
            Verdict::Fail
        },
        text: match result {
            Ok(()) => format!("{} — запись возможна", dir.display()),
            Err(e) => format!("{} — нет записи: {e}", dir.display()),
        },
    }
}

/// Плагины: каталоги существуют, считаем плагины и скиллы.
fn check_plugins(cfg: &Config) -> Check {
    let mut plugins = 0usize;
    let mut skills = 0usize;
    let mut missing = Vec::new();
    for dir in &cfg.plugins.dirs {
        if !dir.is_dir() {
            missing.push(dir.display().to_string());
            continue;
        }
        if let Ok(rd) = std::fs::read_dir(dir) {
            for entry in rd.flatten() {
                let p = entry.path();
                if p.join("plugin.json").is_file() {
                    plugins += 1;
                }
                skills += count_files_named(&p, "SKILL.md", 3);
            }
        }
    }
    let verdict = if plugins == 0 {
        Verdict::Fail
    } else if missing.is_empty() {
        Verdict::Ok
    } else {
        Verdict::Warn
    };
    let mut text = format!("{plugins} плагинов, {skills} скиллов");
    if !missing.is_empty() {
        let _ = write!(text, "; нет каталогов: {}", missing.join(", "));
    }
    Check {
        name: "plugins",
        verdict,
        text,
    }
}

/// Счёт файлов с заданным именем в поддереве (глубина ограничена).
fn count_files_named(dir: &Path, name: &str, depth: usize) -> usize {
    if depth == 0 {
        return 0;
    }
    let mut n = usize::from(dir.join(name).is_file());
    if let Ok(rd) = std::fs::read_dir(dir) {
        for entry in rd.flatten() {
            let p = entry.path();
            if p.is_dir() {
                n += count_files_named(&p, name, depth - 1);
            }
        }
    }
    n
}

/// База знаний: сколько каталогов существует.
fn check_knowledge(cfg: &Config) -> Check {
    let existing: Vec<_> = cfg.knowledge.dirs.iter().filter(|d| d.is_dir()).collect();
    let verdict = if existing.is_empty() {
        Verdict::Warn
    } else {
        Verdict::Ok
    };
    Check {
        name: "knowledge",
        verdict,
        text: format!(
            "{} из {} каталогов доступны",
            existing.len(),
            cfg.knowledge.dirs.len()
        ),
    }
}

/// Кодовые харнессы: бинарь в PATH.
fn check_harnesses(cfg: &Config) -> Check {
    let mut found = Vec::new();
    let mut missing = Vec::new();
    for (name, hc) in &cfg.harnesses {
        if hc.binary.is_empty() {
            continue;
        }
        if binary_in_path(&hc.binary) {
            found.push(name.clone());
        } else {
            missing.push(format!("{name} ({})", hc.binary));
        }
    }
    let verdict = if missing.is_empty() {
        Verdict::Ok
    } else {
        Verdict::Warn
    };
    Check {
        name: "harnesses",
        verdict,
        text: format!(
            "в PATH: {} ({}); отсутствуют: {}",
            found.len(),
            found.join(", "),
            if missing.is_empty() {
                "—".into()
            } else {
                missing.join(", ")
            }
        ),
    }
}

/// MCP: файл серверов существует и парсится; бинари команд в PATH.
fn check_mcp(cfg: &Config) -> Check {
    let file = &cfg.mcp.servers_file;
    if !file.is_file() {
        return Check {
            name: "mcp",
            verdict: Verdict::Warn,
            text: format!("{} не найден (MCP опциональны)", file.display()),
        };
    }
    let parsed = std::fs::read_to_string(file)
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok());
    let Some(v) = parsed else {
        return Check {
            name: "mcp",
            verdict: Verdict::Fail,
            text: format!("{} — не JSON", file.display()),
        };
    };
    let servers = v
        .get("mcpServers")
        .and_then(|s| s.as_object())
        .map_or(0, serde_json::Map::len);
    let mut missing_cmds = Vec::new();
    if let Some(map) = v.get("mcpServers").and_then(|s| s.as_object()) {
        for (name, spec) in map {
            let cmd = spec.get("command").and_then(|c| c.as_str()).unwrap_or("");
            if !cmd.is_empty() && !binary_in_path(cmd) {
                missing_cmds.push(format!("{name} ({cmd})"));
            }
        }
    }
    let verdict = if missing_cmds.is_empty() {
        Verdict::Ok
    } else {
        Verdict::Warn
    };
    let mut text = format!("{servers} серверов в {}", file.display());
    if !missing_cmds.is_empty() {
        let _ = write!(text, "; нет бинарей: {}", missing_cmds.join(", "));
    }
    Check {
        name: "mcp",
        verdict,
        text,
    }
}

/// Крон: файл расписания существует и не пуст.
fn check_cron(cfg: &Config) -> Check {
    let file = &cfg.cron.file;
    let ok = file.is_file() && std::fs::metadata(file).is_ok_and(|m| m.len() > 0);
    Check {
        name: "cron",
        verdict: if ok { Verdict::Ok } else { Verdict::Warn },
        text: if ok {
            format!("{} на месте", file.display())
        } else {
            format!("{} отсутствует или пуст (крон опционален)", file.display())
        },
    }
}

/// Веб: кураторский список архитектурных сайтов наполнен.
fn check_web(cfg: &Config) -> Check {
    let n = cfg.web.arch_sites.len();
    Check {
        name: "web",
        verdict: if n == 0 { Verdict::Warn } else { Verdict::Ok },
        text: format!("{n} кураторских сайтов архитектурных знаний"),
    }
}

/// Archify-контур диаграмм: node в PATH + настроенный `[archify].cli_path`.
/// Ненастроенная интеграция — Warn (не блокирует остальное окружение).
fn check_archify(cfg: &Config) -> Check {
    if !cfg.archify.enabled {
        return Check {
            name: "archify",
            verdict: Verdict::Ok,
            text: "выключен конфигом ([archify].enabled = false)".into(),
        };
    }
    let node_ok = binary_in_path(&cfg.archify.node_bin);
    let cli = &cfg.archify.cli_path;
    let cli_ok = !cli.as_os_str().is_empty() && cli.is_file();
    let (verdict, text) = match (node_ok, cli_ok) {
        (true, true) => {
            let base = format!(
                "node '{}' ({}) + CLI {}",
                cfg.archify.node_bin,
                node_version(&cfg.archify.node_bin).unwrap_or_else(|| "версия?".into()),
                cli.display()
            );
            match crate::archify::check_cli_version(cli) {
                crate::archify::CliVersionVerdict::Pinned(v) => {
                    (Verdict::Ok, format!("{base} (версия {v} — проверенная)"))
                }
                crate::archify::CliVersionVerdict::Mismatch(v) => (
                    Verdict::Warn,
                    format!(
                        "{base} — версия {v} отличается от проверенной {} (контракт \
                         schemaVersion: 1): движок ставить из вендоренного tarball'а \
                         релиза, а не из сети",
                        crate::archify::PINNED_CLI_VERSION
                    ),
                ),
                crate::archify::CliVersionVerdict::Unknown => (
                    Verdict::Warn,
                    format!(
                        "{base} — версия CLI не определена (нет skill-release.json/\
                         package.json); проверенная — {}",
                        crate::archify::PINNED_CLI_VERSION
                    ),
                ),
            }
        }
        (false, _) => (
            Verdict::Warn,
            format!(
                "node '{}' не найден в PATH — Archify-инструменты недоступны",
                cfg.archify.node_bin
            ),
        ),
        (true, false) => (
            Verdict::Warn,
            "не настроен [archify].cli_path (путь к bin/archify.mjs; движок — \
             вендоренный tarball archify-vendored-*.tar.gz из релиза дистрибутива, \
             распаковать в ~/.arch-harness/)"
                .into(),
        ),
    };
    Check {
        name: "archify",
        verdict,
        text,
    }
}

/// Версия node для строки диагностики (`node --version`, первая строка).
/// `None` при любой ошибке запуска — диагностика деградирует к «версия?».
fn node_version(node_bin: &str) -> Option<String> {
    binary_version(node_bin)
}

/// git в PATH (нужен agentsmd, контролю репозиториев).
fn check_git() -> Check {
    let ok = binary_in_path("git");
    Check {
        name: "git",
        verdict: if ok { Verdict::Ok } else { Verdict::Warn },
        text: if ok {
            "в PATH".into()
        } else {
            "не найден в PATH".into()
        },
    }
}

/// Бинарь доступен в PATH (через `which`, без запуска самого бинаря).
fn binary_in_path(name: &str) -> bool {
    std::process::Command::new("which")
        .arg(name)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Конфиг с минимальным окружением в tempdir.
    fn test_config(dir: &Path) -> Config {
        let mut cfg = Config::default();
        cfg.paths.sessions_dir = dir.join("sessions");
        cfg.knowledge.dirs = vec![dir.join("kb")];
        cfg.plugins.dirs = vec![dir.join("plugins")];
        cfg.cron.file = dir.join("cron.toml");
        cfg.mcp.servers_file = dir.join("mcp.json");
        cfg
    }

    #[test]
    fn healthy_minimal_environment() {
        let tmp = tempfile::tempdir().expect("tmp");
        let cfg = test_config(tmp.path());
        // Плагин-заглушка.
        let p = tmp.path().join("plugins/demo");
        std::fs::create_dir_all(p.join("skills/s1")).expect("mkdir");
        std::fs::write(p.join("plugin.json"), "{}").expect("write");
        std::fs::write(p.join("skills/s1/SKILL.md"), "---\nname: s1\n---").expect("write");
        std::fs::create_dir_all(tmp.path().join("kb")).expect("mkdir kb");
        std::fs::write(tmp.path().join("cron.toml"), "[tasks]").expect("write cron");

        let checks = run_checks(&cfg);
        let by = |name: &str| checks.iter().find(|c| c.name == name).expect("check");
        assert_eq!(by("sessions").verdict, Verdict::Ok);
        assert_eq!(by("plugins").verdict, Verdict::Ok, "1 плагин, 1 скилл");
        assert!(by("plugins").text.contains("1 плагинов"));
        assert!(by("plugins").text.contains("1 скиллов"));
        assert_eq!(by("knowledge").verdict, Verdict::Ok);
        assert_eq!(by("cron").verdict, Verdict::Ok);
        assert_eq!(
            by("mcp").verdict,
            Verdict::Warn,
            "mcp.json не создан — warn"
        );
        assert!(render(&checks).contains("arch-be doctor"));
    }

    #[test]
    fn missing_default_model_and_plugins_fail() {
        let tmp = tempfile::tempdir().expect("tmp");
        let mut cfg = test_config(tmp.path());
        cfg.default_model = "нет-такой".into();
        cfg.plugins.dirs = vec![tmp.path().join("пусто")];
        let checks = run_checks(&cfg);
        let by = |name: &str| checks.iter().find(|c| c.name == name).expect("check");
        assert_eq!(by("default_model").verdict, Verdict::Fail);
        assert_eq!(by("plugins").verdict, Verdict::Fail);
        assert_eq!(exit_code(&checks), 1);
        assert!(render(&checks).contains("проблем"));
    }

    #[test]
    fn mcp_invalid_json_fails() {
        let tmp = tempfile::tempdir().expect("tmp");
        let cfg = test_config(tmp.path());
        std::fs::write(tmp.path().join("mcp.json"), "не json").expect("write");
        let checks = run_checks(&cfg);
        let mcp = checks.iter().find(|c| c.name == "mcp").expect("mcp");
        assert_eq!(mcp.verdict, Verdict::Fail);
    }

    #[test]
    fn cli_kind_model_needs_no_api_key() {
        // kind = "cli" (внешний авторизованный CLI-агент как LLM): ключ не
        // требуется ни в одной сборке — такая модель не участвует в проверке.
        let tmp = tempfile::tempdir().expect("tmp");
        let mut cfg = test_config(tmp.path());
        cfg.models.clear();
        cfg.models.insert(
            "claude-cli".into(),
            crate::config::ModelConfig {
                kind: Some("cli".into()),
                command: Some("claude".into()),
                ..Default::default()
            },
        );
        cfg.default_model = "claude-cli".into();
        let checks = run_checks(&cfg);
        let keys = checks
            .iter()
            .find(|c| c.name == "api-keys")
            .expect("api-keys");
        assert_eq!(
            keys.verdict,
            Verdict::Ok,
            "cli-провайдер без ключа — норма: {keys:?}"
        );
        assert!(keys.text.contains("cli-провайдер"), "{}", keys.text);
    }

    /// В core-сборке отсутствие ключей сетевых провайдеров — Warn, не Fail:
    /// сетевого стека в бинаре нет, ключи ему и не нужны.
    #[test]
    #[cfg(not(feature = "harness"))]
    fn core_build_missing_keys_are_warn_not_fail() {
        let tmp = tempfile::tempdir().expect("tmp");
        let cfg = test_config(tmp.path());
        let checks = run_checks(&cfg);
        let keys = checks
            .iter()
            .find(|c| c.name == "api-keys")
            .expect("api-keys");
        assert_eq!(keys.verdict, Verdict::Warn, "core: {keys:?}");
        assert!(keys.text.contains("core-сборка"), "{}", keys.text);
    }

    /// doctor --host после `connect gigacode`: settings и скиллы — Ok.
    /// (На проверки «arch-be»/«host» не ассертим: они зависят от PATH машины
    /// прогона — недетерминировано.)
    #[test]
    fn host_checks_gigacode_after_connect() {
        let tmp = tempfile::tempdir().expect("tmp");
        let proj = tmp.path().join("proj");
        std::fs::create_dir_all(&proj).expect("mkdir");
        crate::connect::connect(&crate::connect::ConnectOptions::new(
            Host::GigaCode,
            proj.clone(),
        ))
        .expect("connect");
        let checks = run_host_checks(Host::GigaCode, &proj, Some(tmp.path()));
        let by = |name: &str| checks.iter().find(|c| c.name == name).expect("check");
        assert_eq!(by("settings").verdict, Verdict::Ok, "{checks:?}");
        assert!(
            by("settings").text.contains(".gigacode/"),
            "каталог автоопределения в тексте: {}",
            by("settings").text
        );
        assert_eq!(by("skills").verdict, Verdict::Ok, "{checks:?}");
    }

    /// doctor --host gigacode на неподключённом проекте: settings — Fail
    /// (лечение подсказано), скиллы — Warn, exit-код 1.
    #[test]
    fn host_checks_gigacode_unconnected_fails() {
        let tmp = tempfile::tempdir().expect("tmp");
        let proj = tmp.path().join("proj");
        std::fs::create_dir_all(&proj).expect("mkdir");
        let checks = run_host_checks(Host::GigaCode, &proj, None);
        let by = |name: &str| checks.iter().find(|c| c.name == name).expect("check");
        assert_eq!(by("settings").verdict, Verdict::Fail, "{checks:?}");
        assert!(
            by("settings").text.contains("connect"),
            "подсказка лечения: {}",
            by("settings").text
        );
        assert_eq!(by("skills").verdict, Verdict::Warn, "{checks:?}");
        assert_eq!(exit_code(&checks), 1);
        let text = render_host(Host::GigaCode, &checks);
        assert!(text.contains("doctor --host gigacode"), "{text}");
    }

    /// doctor --host qwen после connect qwen: settings Ok; битый settings.json
    /// и чужая команда — Fail.
    #[test]
    fn host_checks_qwen_settings_states() {
        let tmp = tempfile::tempdir().expect("tmp");
        let proj = tmp.path().join("proj");
        std::fs::create_dir_all(&proj).expect("mkdir");
        crate::connect::connect(&crate::connect::ConnectOptions::new(
            Host::Qwen,
            proj.clone(),
        ))
        .expect("connect");
        let checks = run_host_checks(Host::Qwen, &proj, None);
        let settings = checks
            .iter()
            .find(|c| c.name == "settings")
            .expect("settings");
        assert_eq!(settings.verdict, Verdict::Ok, "{checks:?}");
        assert!(settings.text.contains("read-only"), "{}", settings.text);

        // Чужая команда в mcpServers.spine — Fail с фактическим значением.
        std::fs::write(
            proj.join(".qwen/settings.json"),
            "{\n  \"mcpServers\": {\"spine\": {\"command\": \"not-arch\", \"args\": []}}\n}\n",
        )
        .expect("write");
        let checks = run_host_checks(Host::Qwen, &proj, None);
        let settings = checks
            .iter()
            .find(|c| c.name == "settings")
            .expect("settings");
        assert_eq!(settings.verdict, Verdict::Fail, "{checks:?}");
        assert!(settings.text.contains("not-arch"), "{}", settings.text);

        // Битый JSON — Fail.
        std::fs::write(proj.join(".qwen/settings.json"), "{битый").expect("write");
        let checks = run_host_checks(Host::Qwen, &proj, None);
        assert_eq!(
            checks
                .iter()
                .find(|c| c.name == "settings")
                .expect("settings")
                .verdict,
            Verdict::Fail
        );
    }

    /// doctor --host kimi: проектный файл в приоритете; при его отсутствии —
    /// пользовательский уровень (с пометкой); нет ни того ни другого — Fail.
    #[test]
    fn host_checks_kimi_project_then_user_level() {
        let tmp = tempfile::tempdir().expect("tmp");
        let proj = tmp.path().join("proj");
        let home = tmp.path().join("home");
        std::fs::create_dir_all(&proj).expect("mkdir proj");
        std::fs::create_dir_all(home.join(".kimi-code")).expect("mkdir home");
        std::fs::write(
            home.join(".kimi-code/mcp.json"),
            "{\n  \"mcpServers\": {\"spine\": {\"command\": \"arch-be\", \"args\": [\"mcp\", \"serve\", \"--rw\"]}}\n}\n",
        )
        .expect("write user mcp.json");
        // Только пользовательский уровень — Ok с пометкой и режимом rw.
        let checks = run_host_checks(Host::Kimi, &proj, Some(home.as_path()));
        let settings = checks
            .iter()
            .find(|c| c.name == "settings")
            .expect("settings");
        assert_eq!(settings.verdict, Verdict::Ok, "{checks:?}");
        assert!(
            settings.text.contains("пользовательский уровень"),
            "{}",
            settings.text
        );
        assert!(settings.text.contains("(rw)"), "{}", settings.text);
        // Проектный уровень появляется — приоритет у него.
        crate::connect::connect(&crate::connect::ConnectOptions::new(
            Host::Kimi,
            proj.clone(),
        ))
        .expect("connect");
        let checks = run_host_checks(Host::Kimi, &proj, Some(home.as_path()));
        let settings = checks
            .iter()
            .find(|c| c.name == "settings")
            .expect("settings");
        assert!(
            settings.text.contains(".kimi-code/mcp.json"),
            "{}",
            settings.text
        );
        assert!(
            !settings.text.contains("пользовательский уровень"),
            "{}",
            settings.text
        );
    }

    /// doctor --host codex: TOML-конфиг с `[mcp_servers.spine]` — Ok; без
    /// секции — Fail.
    #[test]
    fn host_checks_codex_toml() {
        let tmp = tempfile::tempdir().expect("tmp");
        let home = tmp.path().join("home");
        std::fs::create_dir_all(home.join(".codex")).expect("mkdir");
        std::fs::write(
            home.join(".codex/config.toml"),
            "model = \"gpt-5\"\n\n[mcp_servers.spine]\ncommand = \"arch-be\"\nargs = [\"mcp\", \"serve\"]\n",
        )
        .expect("write");
        let checks = run_host_checks(Host::Codex, tmp.path(), Some(home.as_path()));
        let settings = checks
            .iter()
            .find(|c| c.name == "settings")
            .expect("settings");
        assert_eq!(settings.verdict, Verdict::Ok, "{checks:?}");
        std::fs::write(home.join(".codex/config.toml"), "model = \"gpt-5\"\n").expect("write");
        let checks = run_host_checks(Host::Codex, tmp.path(), Some(home.as_path()));
        assert_eq!(
            checks
                .iter()
                .find(|c| c.name == "settings")
                .expect("settings")
                .verdict,
            Verdict::Fail
        );
    }

    /// doctor --host generic: файлов не бывает — Warn-пояснение, проверка
    /// версии хоста отсутствует (бинаря generic нет).
    #[test]
    fn host_checks_generic_has_no_settings_file() {
        let tmp = tempfile::tempdir().expect("tmp");
        let checks = run_host_checks(Host::Generic, tmp.path(), None);
        let settings = checks
            .iter()
            .find(|c| c.name == "settings")
            .expect("settings");
        assert_eq!(settings.verdict, Verdict::Warn);
        assert!(
            checks.iter().all(|c| c.name != "host"),
            "у generic нет бинаря хоста: {checks:?}"
        );
    }
}
