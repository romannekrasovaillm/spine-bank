#![cfg(feature = "harness")]
//! Подкоманды `arch-be bench` и их обработчик: архитектурные бенчмарки
//! (B1: выделено из `main.rs`; модуль есть только в сборке `harness`).

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use clap::Subcommand;

use arch_harness::config::Config;
use arch_harness::llm::LlmRegistry;

use super::resolve_asset;

/// Подкоманды `arch-be bench` (только сборка `harness`).
#[cfg(feature = "harness")]
#[derive(Subcommand)]
pub(crate) enum BenchCmd {
    /// Список бенчмарков.
    List,
    /// Прогнать бенчмарк.
    Run {
        /// Имя файла бенчмарка в assets/benchmarks (или путь).
        name: Option<String>,
        /// Испытуемая модель (для --golden — модель-судья).
        #[arg(long)]
        model: Option<String>,
        /// Прогон судьи по golden-set (assets/benchmarks/golden): метрика
        /// согласия с эталоном MAE; выше порога `judge.golden_max_mae` — exit 1.
        #[arg(long)]
        golden: bool,
        /// Прогон одной рубрики: годен только с `--golden` (смысловые рубрики
        /// калибруются поимённо, ADR-051).
        #[arg(long)]
        rubric: Option<String>,
        /// Дописать результат golden-прогона строкой JSON в evidence-журнал
        /// (M-2, история — `bench golden-history`).
        #[arg(long)]
        record: Option<PathBuf>,
    },
    /// История golden-прогонов из evidence-журнала (JSONL) — markdown-таблица.
    GoldenHistory {
        /// Путь к журналу, записанному `bench run --golden --record`.
        path: PathBuf,
    },
    /// Согласие golden-эталонов с оценками живых архитекторов (J-3,
    /// протокол — docs/judge-human-agreement.md).
    HumanAgreement {
        /// Каталог golden-set (`<имя>.md` + `<имя>.expected.yaml`).
        #[arg(long)]
        golden_dir: PathBuf,
        /// Каталог человеческих анкет (`<документ>.<участник>.expected.yaml`).
        #[arg(long)]
        humans: PathBuf,
    },
}

/// `arch-be bench` (только сборка `harness`).
#[cfg(feature = "harness")]
pub(crate) async fn cmd_bench(cfg: &Arc<Config>, cmd: BenchCmd) -> Result<()> {
    match cmd {
        BenchCmd::List => {
            for b in arch_harness::bench::list(&cfg.paths.benchmarks_dir())? {
                println!("  {:<32} {} [{}]", b.name, b.description, b.tags.join(", "));
            }
        }
        BenchCmd::Run {
            name,
            model,
            golden,
            rubric,
            record,
        } => {
            if record.is_some() && !golden {
                anyhow::bail!("`bench run --record` применим только с --golden");
            }
            if rubric.is_some() && !golden {
                anyhow::bail!("`bench run --rubric` применим только с --golden");
            }
            let registry = LlmRegistry::from_config(cfg)?;
            let provider = match &model {
                Some(m) => registry.get(m)?,
                None => registry.default(),
            };
            if golden {
                if name.is_some() {
                    anyhow::bail!("`bench run --golden` не совместим с именем бенчмарка");
                }
                // Регрессионный гейт качества судьи (ADR-004): согласие с
                // эталоном ниже порога — exit 1, как у `control check`.
                let report = arch_harness::bench::run_golden_filtered(
                    provider.as_ref(),
                    &cfg.paths.rubrics_dir(),
                    &cfg.paths.benchmarks_dir().join("golden"),
                    &cfg.judge,
                    rubric.as_deref(),
                )
                .await?;
                println!(
                    "Golden-прогон судьи '{}' (сэмплов на критерий: {}):",
                    report.judge_model, cfg.judge.samples
                );
                for case in &report.cases {
                    println!(
                        "  {:<32} MAE {:.2} ({} критериев)",
                        case.doc, case.mae, case.compared
                    );
                }
                // Механическая диагностика: MAE по критериям и length bias.
                print!("{}", report.diagnostics_text());
                let passed = report.mae <= cfg.judge.golden_max_mae;
                println!(
                    "Итог MAE: {:.2} по {} парам (порог {:.2}) — {}",
                    report.mae,
                    report.compared,
                    cfg.judge.golden_max_mae,
                    if passed { "PASS" } else { "FAIL" }
                );
                // Evidence-журнал (M-2): запись не зависит от исхода гейта —
                // история хранит и регрессии.
                if let Some(path) = &record {
                    let entry = arch_harness::bench::GoldenRecord::from_report(
                        &report,
                        chrono::Local::now().format("%Y-%m-%d").to_string(),
                    );
                    arch_harness::bench::record_golden(path, &entry)?;
                    println!("Записано в журнал: {}", path.display());
                }
                if !passed {
                    std::process::exit(1);
                }
                return Ok(());
            }
            let Some(name) = name else {
                anyhow::bail!("укажите имя бенчмарка или флаг --golden");
            };
            let path = resolve_asset(&cfg.paths.benchmarks_dir(), &name, "yaml");
            let bench = arch_harness::bench::load(&path)?;
            let report = arch_harness::bench::run(
                &bench,
                provider.as_ref(),
                &cfg.paths.rubrics_dir(),
                &cfg.paths.reports_dir,
                &cfg.judge,
            )
            .await?;
            println!(
                "Бенчмарк '{}': {:.2} (порог {:.2}) — {}",
                report.bench_name,
                report.rubric_report.weighted_total,
                bench.pass_threshold,
                if report.passed { "PASS" } else { "FAIL" }
            );
        }
        BenchCmd::GoldenHistory { path } => {
            let (records, broken) = arch_harness::bench::load_golden_history(&path)?;
            if broken > 0 {
                eprintln!("пропущено битых строк: {broken}");
            }
            if records.is_empty() {
                println!("Журнал {} пуст.", path.display());
            } else {
                print!("{}", arch_harness::bench::golden_history_markdown(&records));
            }
        }
        BenchCmd::HumanAgreement { golden_dir, humans } => {
            let report = arch_harness::bench::human_agreement(&golden_dir, &humans)?;
            for doc in &report.skipped {
                eprintln!("пропущен {doc}: нет человеческих анкет");
            }
            print!("{}", report.to_markdown());
        }
    }
    Ok(())
}
