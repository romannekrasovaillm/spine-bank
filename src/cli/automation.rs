#![cfg(feature = "harness")]
//! Подкоманды `arch-be cron` и `arch eval` и их обработчики (B1: выделено
//! из `main.rs`; модуль есть только в сборке `harness`).

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use clap::Subcommand;

use arch_harness::config::Config;
use arch_harness::llm::LlmRegistry;

/// Подкоманды `arch-be cron` (только сборка `harness`).
#[cfg(feature = "harness")]
#[derive(Subcommand)]
pub(crate) enum CronCmd {
    /// Список задач расписания.
    List,
    /// Запустить задачу по имени сейчас.
    Run {
        /// Имя задачи.
        name: String,
    },
    /// Проверить и запустить дюжные задачи (для системного cron).
    Tick,
}

/// Подкоманды `arch eval` (continuous evals, docs/evals.md; только сборка `harness`).
#[cfg(feature = "harness")]
#[derive(Subcommand)]
pub(crate) enum EvalCmd {
    /// Прогнать eval-сьют: детерминированные проверки (офлайн) + опциональный
    /// LLM-судья. Pass-rate ниже гейта — exit code 1 (регрессионный гейт).
    Run {
        /// Каталог сьюта (YAML-задачи). Без флага — встроенный сьют
        /// agent-config, прогоняемый герметично (ассеты разворачиваются во
        /// временный каталог; живой конфиг не трогается).
        #[arg(long)]
        suite: Option<PathBuf>,
        /// Гейт pass-rate в процентах (дефолт 100): ниже — exit code 1.
        #[arg(long)]
        gate: Option<f64>,
        /// Включить слой LLM-судьи (prompt-задачи и рубрики; нужен API-ключ).
        #[arg(long)]
        judge: bool,
        /// Модель для слоя судьи (имя из [models]; иначе — default).
        #[arg(long)]
        model: Option<String>,
    },
}

/// `arch-be cron` (только сборка `harness`).
#[cfg(feature = "harness")]
pub(crate) async fn cmd_cron(cfg: &Arc<Config>, cmd: CronCmd) -> Result<()> {
    let tab = arch_harness::cron::load(&cfg.cron.file)?;
    match cmd {
        CronCmd::List => {
            for j in &tab.jobs {
                println!(
                    "  {:<24} {:<16} {}",
                    j.name,
                    j.schedule,
                    j.task_md.display()
                );
            }
        }
        CronCmd::Run { name } => {
            let job = tab
                .jobs
                .iter()
                .find(|j| j.name == name)
                .with_context(|| format!("задача '{name}' не найдена"))?;
            let registry = LlmRegistry::from_config(cfg)?;
            let provider = match &job.model {
                Some(m) => registry.get(m)?,
                None => registry.default(),
            };
            let tools = arch_harness::tools::full_registry(cfg);
            let out_dir = job
                .out
                .clone()
                .unwrap_or_else(|| cfg.paths.reports_dir.join("cron"));
            let path =
                arch_harness::cron::run_job(job, provider.as_ref(), &tools, &out_dir).await?;
            println!("Отчёт: {}", path.display());
        }
        CronCmd::Tick => {
            // «Дюжные» задачи между прошлым тиком и сейчас; метка — в state-файле.
            let state_file = Config::home_dir().join("cron-last-tick");
            let now = chrono::Local::now();
            let last = std::fs::read_to_string(&state_file)
                .ok()
                .and_then(|s| {
                    chrono::DateTime::parse_from_rfc3339(s.trim())
                        .ok()
                        .map(|dt| dt.with_timezone(&chrono::Local))
                })
                .unwrap_or_else(|| now - chrono::Duration::hours(24));
            let registry = LlmRegistry::from_config(cfg)?;
            let provider = registry.default();
            let tools = arch_harness::tools::full_registry(cfg);
            let reports_dir = cfg
                .cron
                .out_dir
                .clone()
                .unwrap_or_else(|| cfg.paths.reports_dir.join("cron"));
            let reports = arch_harness::cron::run_due(
                &tab,
                last,
                now,
                provider.as_ref(),
                &tools,
                &reports_dir,
            )
            .await?;
            std::fs::write(&state_file, now.to_rfc3339()).context("запись метки тика")?;
            if reports.is_empty() {
                println!("Дюжных задач нет.");
            }
            for path in &reports {
                println!("Отчёт: {}", path.display());
            }
        }
    }
    Ok(())
}

/// `arch eval`: регрессионные eval-сьюты конфигурации харнесса (docs/evals.md).
///
/// Встроенный сьют (без `--suite`) герметичен: ассеты и конфиг разворачиваются
/// во временный каталог, живой `~/.arch-harness` не трогается — прогон зелёный
/// и в CI без `arch init`. Пользовательский `--suite` бежит против живой
/// установки. Гейт: pass-rate ниже `--gate` (дефолт 100%) — exit code 1.
#[cfg(feature = "harness")]
pub(crate) async fn cmd_eval(cfg: &Arc<Config>, cmd: EvalCmd) -> Result<()> {
    match cmd {
        EvalCmd::Run {
            suite,
            gate,
            judge,
            model,
        } => {
            let gate_pct = gate.unwrap_or(100.0);
            if !(0.0..=100.0).contains(&gate_pct) {
                anyhow::bail!("--gate: ожидается процент 0..=100, получено {gate_pct}");
            }
            // tempdir держим живым до конца прогона (встроенный сьют).
            let mut _tmp = None;
            let (suite_dir, ctx) = if let Some(dir) = &suite {
                (dir.clone(), arch_harness::eval::SuiteContext::for_live()?)
            } else {
                let tmp = tempfile::tempdir().context("временный каталог встроенного сьюта")?;
                let home = arch_harness::eval::prepare_builtin_home(tmp.path())?;
                let ctx = arch_harness::eval::SuiteContext::for_builtin(tmp.path(), &home.config)?;
                _tmp = Some(tmp);
                (home.suite_dir, ctx)
            };
            // Слой судьи: реестр моделей строится только при --judge —
            // офлайн-прогон не требует ни ключей, ни сети.
            let provider = if judge {
                let registry = LlmRegistry::from_config(cfg)?;
                Some(match &model {
                    Some(name) => registry.get(name)?,
                    None => registry.default(),
                })
            } else {
                None
            };
            let rubrics_dir = cfg.paths.rubrics_dir();
            let judge_ctx = provider.as_ref().map(|p| arch_harness::eval::JudgeCtx {
                provider: p.as_ref(),
                cfg: &cfg.judge,
                rubrics_dir: &rubrics_dir,
            });
            let report =
                arch_harness::eval::run_suite(&suite_dir, &ctx, judge_ctx.as_ref(), gate_pct)
                    .await?;
            print!("{}", arch_harness::eval::render_text(&report));
            let out = arch_harness::eval::write_report(&report, &cfg.paths.evals_dir())?;
            eprintln!("Отчёт: {}", out.display());
            if !report.gate_passed {
                std::process::exit(1);
            }
        }
    }
    Ok(())
}
