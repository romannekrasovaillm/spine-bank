//! Типы данных `connect` (B1): хост-получатель, опции, исходы по скиллам,
//! отчёт о подключении.

use std::path::PathBuf;

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

/// Отчёт о подключении (печать — [`crate::connect::render_report`]).
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
