//! Подкоманды `arch-be archunit` и их обработчик: JVM-гейты `ArchUnit`
//! (B1: выделено из `main.rs`).

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Subcommand;

use super::resolve_constraints_cli;

/// Подкоманды `arch-be archunit` (ADR-039).
#[derive(Subcommand)]
pub(crate) enum ArchunitCmd {
    /// Сгенерировать артефакты для JVM-репо: `ArchFitnessTest.java` (`JUnit` 5 +
    /// `ArchUnit`, встраивается в репо команды) и `archunit-rules.json` (спек).
    Gen {
        /// JVM-репозиторий.
        repo: PathBuf,
        /// Файл ограничений (по умолчанию <repo>/.arch-handoff/`CONSTRAINTS.yaml`).
        #[arg(long)]
        constraints: Option<PathBuf>,
        /// Каталог типизированной модели (для `context_boundary`; дефолт
        /// <repo>/model).
        #[arg(long)]
        model_dir: Option<PathBuf>,
        /// Каталог вывода (по умолчанию <repo>/archunit-fitness).
        #[arg(long)]
        out_dir: Option<PathBuf>,
        /// Базовый пакет для `@AnalyzeClasses` (без него — выводится из
        /// якорных пакетов спека, иначе сканируется всё `..`).
        #[arg(long)]
        base_package: Option<String>,
    },
    /// Standalone-гейт: исполнить java-правила `CONSTRAINTS.yaml` настоящим
    /// `ArchUnit` на скомпилированных классах (без правок JVM-репо).
    Check {
        /// JVM-репозиторий.
        repo: PathBuf,
        /// Файл ограничений (по умолчанию <repo>/.arch-handoff/`CONSTRAINTS.yaml`).
        #[arg(long)]
        constraints: Option<PathBuf>,
        /// Каталог типизированной модели (для `context_boundary`; дефолт
        /// <repo>/model).
        #[arg(long)]
        model_dir: Option<PathBuf>,
        /// Каталог скомпилированных классов (без него — авто-детект
        /// target/classes, build/classes/java/main, out/production, classes).
        #[arg(long)]
        classes: Option<PathBuf>,
        /// Каталог с jar'ами `ArchUnit` (без него — $`ARCHUNIT_HOME`, затем
        /// ~/.arch-harness/archunit/lib).
        #[arg(long)]
        jar_dir: Option<PathBuf>,
        /// Таймаут гейта, секунды (дефолт 300).
        #[arg(long)]
        timeout_secs: Option<u64>,
        /// Машиночитаемый вывод: JSON-отчёт.
        #[arg(long)]
        json: bool,
    },
    /// Скачать пиннутые jar'ы `ArchUnit` (archunit + slf4j) с Maven Central в
    /// кэш с проверкой SHA-256. Только сборка `harness` (сетевой стек).
    #[cfg(feature = "harness")]
    Fetch {
        /// Каталог назначения (по умолчанию ~/.arch-harness/archunit/lib).
        #[arg(long)]
        jar_dir: Option<PathBuf>,
    },
}

/// `arch-be archunit …`: `ArchUnit`-мост (ADR-039).
// В core-сборке единственный async-участок (fetch по сети) вырезан фичей —
// async-обёртка остаётся для единого вида с полной сборкой.
#[cfg_attr(not(feature = "harness"), allow(clippy::unused_async))]
pub(crate) async fn cmd_archunit(cmd: ArchunitCmd) -> Result<()> {
    match cmd {
        ArchunitCmd::Gen {
            repo,
            constraints,
            model_dir,
            out_dir,
            base_package,
        } => {
            let c = resolve_constraints_cli(&repo, constraints);
            let rules = arch_harness::control::load_fitness_rules(&c)?;
            let refs: Vec<&arch_harness::control::FitnessRule> = rules.iter().collect();
            let mut spec =
                arch_harness::archunit::spec_from_constraints(&repo, &refs, model_dir.as_deref());
            if let Some(bp) = base_package {
                spec.base_package = Some(bp);
            }
            let out = out_dir.unwrap_or_else(|| repo.join("archunit-fitness"));
            std::fs::create_dir_all(&out)
                .with_context(|| format!("не создать каталог вывода {}", out.display()))?;
            let test_path = out.join("ArchFitnessTest.java");
            let json_path = out.join("archunit-rules.json");
            std::fs::write(&test_path, arch_harness::archunit::render_junit_test(&spec))
                .with_context(|| format!("не записать {}", test_path.display()))?;
            std::fs::write(&json_path, arch_harness::archunit::spec_to_json(&spec)?)
                .with_context(|| format!("не записать {}", json_path.display()))?;
            println!(
                "спек: {} правил ArchUnit, {} не смаплено (unsupported)",
                spec.rules.len(),
                spec.unsupported.len()
            );
            for u in &spec.unsupported {
                println!("  [warn] {}: {}", u.rule, u.reason);
            }
            println!("JUnit-тест: {}", test_path.display());
            println!("спек JSON:  {}", json_path.display());
        }
        ArchunitCmd::Check {
            repo,
            constraints,
            model_dir,
            classes,
            jar_dir,
            timeout_secs,
            json,
        } => {
            let c = resolve_constraints_cli(&repo, constraints);
            let rules = arch_harness::control::load_fitness_rules(&c)?;
            let refs: Vec<&arch_harness::control::FitnessRule> = rules.iter().collect();
            let spec =
                arch_harness::archunit::spec_from_constraints(&repo, &refs, model_dir.as_deref());
            for u in &spec.unsupported {
                eprintln!("[warn] unsupported: {}: {}", u.rule, u.reason);
            }
            let classes_dir = match classes {
                Some(dir) => dir,
                None => arch_harness::archunit::find_classes_dir(&repo).with_context(|| {
                    "archunit: скомпилированные классы не найдены (target/classes, \
                     build/classes/java/main, out/production, classes) — соберите проект \
                     (`mvn compile` / `javac -d classes ...`) или укажите --classes"
                })?,
            };
            let opts = arch_harness::archunit::GateOptions {
                classes_dir,
                jar_dir: arch_harness::archunit::resolve_jar_dir(jar_dir.as_deref()),
                runner_cache: arch_harness::archunit::default_runner_cache(),
                timeout: std::time::Duration::from_secs(
                    timeout_secs.unwrap_or(arch_harness::archunit::DEFAULT_GATE_TIMEOUT_SECS),
                ),
            };
            let outcome = arch_harness::archunit::run_gate(&spec, &opts)?;
            let severity_of = |rule_id: &str| {
                spec.rules
                    .iter()
                    .find(|r| r.id == rule_id)
                    .map_or("error", |r| r.severity.as_str())
            };
            let passed = outcome
                .violations
                .iter()
                .all(|v| severity_of(&v.rule_id) != "error");
            if json {
                let report = serde_json::json!({
                    "repo": repo,
                    "passed": passed,
                    "rules_executed": outcome.rules_executed,
                    "violations": outcome.violations.iter().map(|v| serde_json::json!({
                        "rule": v.rule_id,
                        "severity": severity_of(&v.rule_id),
                        "detail": v.detail,
                    })).collect::<Vec<_>>(),
                    "unsupported": spec.unsupported,
                });
                println!("{report}");
            } else {
                println!(
                    "ArchUnit-гейт: исполнено правил {}, нарушений {}",
                    outcome.rules_executed,
                    outcome.violations.len()
                );
                for v in &outcome.violations {
                    println!(
                        "  [{}] {} — {}",
                        severity_of(&v.rule_id),
                        v.rule_id,
                        v.detail
                    );
                }
                println!("Итог: {}", if passed { "PASS" } else { "FAIL" });
            }
            if !passed {
                std::process::exit(1);
            }
        }
        #[cfg(feature = "harness")]
        ArchunitCmd::Fetch { jar_dir } => {
            let dest = jar_dir.unwrap_or_else(|| {
                arch_harness::config::Config::home_dir()
                    .join("archunit")
                    .join("lib")
            });
            let fetched = arch_harness::archunit::fetch_jars(&dest).await?;
            println!("jar'ы ArchUnit → {}", dest.display());
            for f in &fetched {
                println!(
                    "  {} {} sha256:{}",
                    f.file,
                    if f.cached {
                        "(кэш)"
                    } else {
                        "(скачан)"
                    },
                    f.sha256
                );
            }
        }
    }
    Ok(())
}
