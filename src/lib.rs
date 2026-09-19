//! # arch-harness — доменный харнесс solution-архитектора
//!
//! Тонкий агентный харнесс для архитектора в корпоративном контуре:
//! TUI (ratatui), mermaid→ASCII рендер, якорные/динамические рубрики
//! архитектурного контроля, специализированные бенчмарки, MCP-интеграции,
//! веб-доступ к доменным знаниям, локальная база знаний, handoff-пакеты
//! кодовым харнессам (Claude Code, Qwen Code, `OpenClaw`, Hermes, Theseus,
//! `CodeWhale`), fitness functions и линтер architecture-spine, крон md-задач.
//!
//! Происхождение идей — `docs/SOURCE_BRIEF.md` (разбор SDD-харнессов и
//! корпоративных агентных фреймворков, август 2026).
//!
//! Cargo-фичи (шаг 4 инверсии «Spine без собственной LLM»):
//! - `harness` (включена по умолчанию) — полная сборка: TUI, агентный цикл,
//!   сетевые LLM-провайдеры (`llm::deepseek` и др.), веб-доступ;
//! - `core` (`--no-default-features --features core`) — слим-сборка без
//!   reqwest/ratatui/crossterm/arboard/scraper: MCP-сервер, CLI контроля и
//!   LLM-судья через внешний CLI (`llm::harness_cli`, `kind = "cli"`).
//!
//! ```no_run
//! use arch_harness::config::Config;
//! let cfg = Config::load(None).expect("config");
//! assert!(cfg.models.contains_key("deepseek"));
//! ```

pub mod adr_registry;
#[cfg(feature = "harness")]
pub mod agent;
pub mod agentsmd;
pub mod archify;
pub mod archunit;
pub mod assets;
pub mod asyncapi;
#[cfg(feature = "harness")]
pub mod bench;
pub mod clipboard;
pub mod config;
pub mod connect;
pub mod contract_diff;
pub mod control;
#[cfg(feature = "harness")]
pub mod cron;
pub mod delta;
pub mod detectors;
pub mod digest;
#[cfg(feature = "harness")]
pub mod distill;
pub mod doctor;
pub mod error;
#[cfg(feature = "harness")]
pub mod eval;
pub mod evidence;
pub mod export;
pub mod failure_memory;
pub mod fleet;
pub mod gate;
pub mod handoff;
#[cfg(feature = "harness")]
pub mod harness;
pub mod hash;
pub mod hooks;
pub mod injection;
pub mod kb;
pub mod landscape;
pub mod llm;
pub mod matchers;
pub mod mcp;
pub mod mcp_journal;
pub mod mcp_server;
pub mod memory;
pub mod mermaid;
pub mod metrics;
pub mod model;
#[cfg(feature = "harness")]
pub mod net;
pub mod nfr;
pub mod openapi;
pub mod openspec;
pub mod passport;
pub mod plugin;
pub mod policy;
pub mod publish;
#[cfg(feature = "harness")]
pub mod ralph;
pub mod redteam;
pub mod rehearsal;
pub mod report_fmt;
pub mod retry;
pub mod review;
pub mod rubric;
pub mod rules_suggest;
pub mod secrets;
pub mod selftest;
pub mod stubs;
#[cfg(feature = "harness")]
pub mod subagent;
pub mod survey;
pub mod tool;
pub mod tools;
pub mod trace;
#[cfg(feature = "harness")]
pub mod tui;
#[cfg(feature = "harness")]
pub mod web;
#[cfg(feature = "harness")]
pub mod worktree;

pub use config::Config;
pub use error::{HarnessError, Result};
