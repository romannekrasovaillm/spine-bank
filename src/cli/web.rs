#![cfg(feature = "harness")]
//! Подкоманды `arch-be web` и их обработчик: поиск и фетч по архитектурным
//! сайтам (B1: выделено из `main.rs`; модуль есть только в сборке `harness`).

use anyhow::Result;
use clap::Subcommand;

use arch_harness::config::Config;

/// Подкоманды `arch-be web` (только сборка `harness`).
#[cfg(feature = "harness")]
#[derive(Subcommand)]
pub(crate) enum WebCmd {
    /// Поиск в вебе.
    Search {
        /// Запрос.
        query: String,
        /// Ограничить кураторскими архитектурными сайтами.
        #[arg(long)]
        arch: bool,
    },
    /// Загрузить страницу текстом.
    Fetch {
        /// URL.
        url: String,
    },
    /// Кураторский список сайтов архитектора.
    Sites,
}

/// `arch-be web` (только сборка `harness`).
#[cfg(feature = "harness")]
pub(crate) async fn cmd_web(cfg: &Config, cmd: WebCmd) -> Result<()> {
    match cmd {
        WebCmd::Search { query, arch } => {
            let results = if arch {
                arch_harness::web::search_arch_sites(&query, &[], &cfg.web).await?
            } else {
                arch_harness::web::search(&query, &cfg.web).await?
            };
            for r in &results {
                println!("• {}\n  {}\n  {}\n", r.title, r.url, r.snippet);
            }
            if results.is_empty() {
                println!("Ничего не найдено.");
            }
        }
        WebCmd::Fetch { url } => {
            let text = arch_harness::web::fetch(&url, &cfg.web).await?;
            println!("{text}");
        }
        WebCmd::Sites => {
            println!("Кураторские сайты архитектора:");
            for s in arch_harness::web::curated_sites(&cfg.web) {
                println!("  {:<16} {:<40} {}", s.name, s.base_url, s.description);
            }
        }
    }
    Ok(())
}
