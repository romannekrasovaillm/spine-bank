//! Подкоманды локальных библиотек: `skills`, `plugins`, `prompts`,
//! `memory`, а также `init` (B1: выделено из `main.rs`).

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Subcommand;

use arch_harness::config::Config;

/// Подкоманды `arch-be memory`.
#[derive(Subcommand)]
pub(crate) enum MemoryCmd {
    /// Дописать заметку в конец файла памяти.
    Add {
        /// Текст заметки.
        text: String,
    },
}

#[derive(Subcommand)]
pub(crate) enum SkillsCmd {
    /// Список всех скиллов библиотеки.
    List,
    /// Поиск по скиллам.
    Search {
        /// Запрос.
        query: String,
        /// Максимум результатов.
        #[arg(long, default_value_t = 8)]
        limit: usize,
    },
    /// Показать полный текст скилла.
    Show {
        /// Точное имя скилла.
        name: String,
    },
}

#[derive(Subcommand)]
pub(crate) enum PluginsCmd {
    /// Список плагинов.
    List,
    /// Подробности плагина (манифест, скиллы, MCP-серверы).
    Show {
        /// Имя плагина.
        name: String,
    },
    /// Файл-замок библиотеки плагинов (C1): записать `plugins.lock`
    /// (SHA-256 каждого файла плагина) или `--check` — сверить без записи
    /// (exit 1 при расхождении).
    Lock {
        /// Только проверить расхождение с текущим замком, ничего не писать.
        #[arg(long)]
        check: bool,
    },
}

/// `arch-be init`: конфиг + ассеты в ~/.arch-harness.
pub(crate) fn cmd_init(cfg: &Config) -> Result<()> {
    let home = Config::home_dir();
    std::fs::create_dir_all(&home).context("создание домашнего каталога")?;
    let written = arch_harness::assets::write_defaults(&home)?;
    let cfg_path = cfg.save_default()?;
    println!("Инициализация завершена:");
    println!("  конфиг:  {}", cfg_path.display());
    println!("  домашний каталог: {}", home.display());
    for f in &written {
        println!("  ассет:   {}", f.display());
    }
    Ok(())
}

/// `arch-be prompts` (только сборка `harness`).
#[cfg(feature = "harness")]
pub(crate) fn cmd_prompts(cfg: &Config, name: Option<String>) -> Result<()> {
    let lib = arch_harness::agent::prompts::load_library(&cfg.paths.prompts_dir())?;
    match name {
        None => {
            println!(
                "Библиотека промптов ({}):",
                cfg.paths.prompts_dir().display()
            );
            for tpl in &lib {
                println!("  {:<24} {}", tpl.name, tpl.description);
            }
        }
        Some(n) => {
            let tpl = lib
                .iter()
                .find(|t| t.name == n)
                .with_context(|| format!("шаблон '{n}' не найден"))?;
            println!("{}", tpl.body);
        }
    }
    Ok(())
}

/// `arch-be memory [add <текст>]`: путь и содержимое глобальной md-памяти
/// либо дописка заметки в конец файла.
pub(crate) fn cmd_memory(cfg: &Config, cmd: Option<MemoryCmd>) -> Result<()> {
    let path = &cfg.paths.memory_file;
    match cmd {
        None => match arch_harness::memory::load(path)? {
            Some(content) => println!("Память ({}):\n{content}", path.display()),
            None => println!(
                "память пустая, файл: {} (дописать — arch-be memory add <текст>)",
                path.display()
            ),
        },
        Some(MemoryCmd::Add { text }) => {
            arch_harness::memory::append(path, &text)?;
            println!("заметка дописана в память: {}", path.display());
        }
    }
    Ok(())
}

/// `arch-be skills`: библиотека скиллов.
pub(crate) fn cmd_skills(cfg: &Config, cmd: SkillsCmd) -> Result<()> {
    // T-09: индекс — настроенные плагины ПЛЮС скиллы, разложенные в проекте
    // (`connect`); без второго поиск пуст в свежем проекте.
    let root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let dirs = arch_harness::plugin::skill_search_dirs(&cfg.plugins.dirs, &root);
    let plugins = arch_harness::plugin::discover(&dirs);
    match cmd {
        SkillsCmd::List => {
            let total: usize = plugins.iter().map(|p| p.skills.len()).sum();
            println!("Скиллов: {total} в {} плагинах", plugins.len());
            for p in &plugins {
                for s in &p.skills {
                    println!(
                        "  {:<28} {:<14} {}",
                        s.name,
                        p.manifest.name,
                        first_line(&s.description, 80)
                    );
                }
            }
        }
        SkillsCmd::Search { query, limit } => {
            let hits = arch_harness::plugin::search(&plugins, &query, limit);
            if hits.is_empty() {
                println!(
                    "{}.",
                    arch_harness::plugin::empty_index_answer(
                        &query,
                        plugins.iter().map(|p| p.skills.len()).sum::<usize>(),
                        &dirs
                    )
                );
            }
            for h in &hits {
                println!(
                    "── {} [{}] (score {:.1})",
                    h.meta.name, h.meta.plugin, h.score
                );
                println!("   {}", first_line(&h.meta.description, 100));
                if !h.snippet.is_empty() {
                    println!("{}", h.snippet);
                }
            }
        }
        SkillsCmd::Show { name } => {
            let meta = arch_harness::plugin::skill_by_name(&plugins, &name)
                .with_context(|| format!("скилл '{name}' не найден (см. `arch-be skills list`)"))?;
            println!("{}", arch_harness::plugin::load_skill(meta)?);
        }
    }
    Ok(())
}

/// `arch-be plugins`: пакеты скиллов + MCP и файл-замок библиотеки (C1).
pub(crate) fn cmd_plugins(cfg: &Config, cmd: PluginsCmd) -> Result<()> {
    if let PluginsCmd::Lock { check } = cmd {
        return cmd_plugins_lock(cfg, check);
    }
    let discovery = arch_harness::plugin::discover_report(&cfg.plugins.dirs);
    for event in &discovery.tampered {
        eprintln!("ОШИБКА {} ({})", event.render(), event.dir.display());
    }
    let plugins = discovery.plugins;
    match cmd {
        PluginsCmd::Lock { .. } => unreachable!("обработано выше"),
        PluginsCmd::List => {
            println!("Плагины ({}):", plugins.len());
            for p in &plugins {
                let mcp_count = if cfg.plugins.include_mcp {
                    arch_harness::plugin::mcp_servers(std::slice::from_ref(p)).len()
                } else {
                    0
                };
                println!(
                    "  {:<24} v{:<8} скиллов: {:<3} mcp: {:<2} {}",
                    p.manifest.name,
                    p.manifest.version,
                    p.skills.len(),
                    mcp_count,
                    first_line(&p.manifest.description, 60)
                );
            }
        }
        PluginsCmd::Show { name } => {
            let p = plugins
                .iter()
                .find(|p| p.manifest.name == name)
                .with_context(|| format!("плагин '{name}' не найден"))?;
            println!(
                "{} v{} — {}",
                p.manifest.name, p.manifest.version, p.manifest.description
            );
            println!("Каталог: {}", p.dir.display());
            if !p.manifest.keywords.is_empty() {
                println!("Ключевые слова: {}", p.manifest.keywords.join(", "));
            }
            println!("Скиллы ({}):", p.skills.len());
            for s in &p.skills {
                println!("  {:<28} {}", s.name, first_line(&s.description, 70));
            }
            let servers = arch_harness::plugin::mcp_servers(std::slice::from_ref(p));
            if !servers.is_empty() {
                println!("MCP-серверы ({}):", servers.len());
                for s in &servers {
                    println!("  {:<24} {} {}", s.name, s.command, s.args.join(" "));
                }
            }
        }
    }
    Ok(())
}

/// `arch-be plugins lock [--check]`: файл-замок библиотеки плагинов (C1).
///
/// Без `--check` — перезаписать замок по текущему состоянию библиотеки и
/// напечатать, что изменилось (легитимное изменение плагина проходит через
/// перегенерацию). С `--check` — сверить без записи; расхождение (подмена,
/// добавленный/удалённый файл, новый плагин) даёт exit 1. Замка нет — честное
/// сообщение (библиотека не защищена), но не ошибка: защита включается
/// генерацией.
fn cmd_plugins_lock(cfg: &Config, check: bool) -> Result<()> {
    use arch_harness::plugin_lock::{self, PluginsLock};
    let mut failed: Vec<String> = Vec::new();
    let mut locked = 0usize;
    for dir in &cfg.plugins.dirs {
        if !dir.is_dir() {
            continue;
        }
        locked += 1;
        let path = plugin_lock::lock_path_for_library(dir);
        let current = PluginsLock::generate(dir);
        let previous =
            plugin_lock::read(&path).with_context(|| format!("чтение замка {}", path.display()))?;
        if check {
            match previous {
                None => println!(
                    "замок не найден: {} — библиотека {} не защищена \
                     (создайте: `arch-be plugins lock`)",
                    path.display(),
                    dir.display()
                ),
                Some(prev) => {
                    let changes = prev.diff(&current);
                    if changes.is_empty() {
                        println!(
                            "замок {}: расхождений нет ({} плагинов, {} файлов)",
                            path.display(),
                            current.plugins.len(),
                            current.file_count()
                        );
                    } else {
                        println!("замок {}: расхождения ({}):", path.display(), changes.len());
                        for line in changes.iter().take(50) {
                            println!("  {line}");
                        }
                        failed.push(path.display().to_string());
                    }
                }
            }
        } else {
            let changes = previous.as_ref().map_or_else(
                || vec!["первичный замок".to_string()],
                |prev| prev.diff(&current),
            );
            plugin_lock::write(&current, &path)?;
            println!(
                "замок записан: {} ({} плагинов, {} файлов)",
                path.display(),
                current.plugins.len(),
                current.file_count()
            );
            if changes.is_empty() {
                println!("  изменений относительно прежнего замка нет");
            } else {
                for line in changes.iter().take(50) {
                    println!("  {line}");
                }
                let rest = changes.len().saturating_sub(50);
                if rest > 0 {
                    println!("  … и ещё {rest}");
                }
            }
        }
    }
    if locked == 0 {
        println!("каталогов плагинов нет — нечего замковывать");
    }
    if !failed.is_empty() {
        anyhow::bail!(
            "замок плагинов разошёлся: {} — возможна подмена; изменение плагина \
             узаконивается осознанной перегенерацией: `arch-be plugins lock`",
            failed.join(", ")
        );
    }
    Ok(())
}

/// Первая строка текста, усечённая до `max` символов.
fn first_line(text: &str, max: usize) -> String {
    let line = text.lines().next().unwrap_or("").trim();
    let cut: String = line.chars().take(max).collect();
    if line.chars().count() > max {
        format!("{cut}…")
    } else {
        cut
    }
}
