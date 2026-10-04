//! Разовые обработчики: `arch-be survey` (обратное обследование),
//! `arch-be policy` (уровни автономии) и `arch-be policy export`
//! (инварианты развёртывания → политики кластера; ADR-059 §D3)
//! (B1: выделено из `main.rs`).

use std::path::{Path, PathBuf};

use anyhow::Result;
use clap::{Subcommand, ValueEnum};

use super::resolve_constraints_cli;
use arch_harness::config::Config;

/// `arch-be survey <repo>`: обратное обследование legacy → docs/reverse/survey.md.
pub(crate) fn cmd_survey(repo: &Path, out: Option<&Path>) -> Result<()> {
    let outcome = arch_harness::survey::run(repo, out)?;
    println!(
        "обследование `{}`: {} находок [confirmed], {} секций [gap]",
        outcome.report.repo_name,
        outcome.report.confirmed_count(),
        outcome.report.gap_count()
    );
    println!("карта: {}", outcome.survey_path.display());
    if outcome.notes_created {
        println!(
            "создана заготовка заметок [inferred]: {}",
            outcome.notes_path.display()
        );
    }
    Ok(())
}

/// Подкоманды `arch-be policy`.
#[derive(Subcommand)]
pub(crate) enum PolicyCmd {
    /// Экспорт инвариантов развёртывания (секция `deployment:`
    /// в `CONSTRAINTS.yaml`) в политики кластера.
    Export {
        /// Целевой формат политики: kyverno | rego.
        #[arg(value_enum)]
        format: PolicyFormat,
        /// Репозиторий с `CONSTRAINTS.yaml` (по умолчанию — текущий каталог).
        #[arg(long, default_value = ".")]
        repo: PathBuf,
        /// Файл ограничений (по умолчанию — единый резолвер:
        /// `.arch-handoff/CONSTRAINTS.yaml` → корневой `CONSTRAINTS.yaml`).
        #[arg(long)]
        constraints: Option<PathBuf>,
        /// Файл вывода (по умолчанию — stdout).
        #[arg(long)]
        output: Option<PathBuf>,
    },
}

/// Формат экспорта политик (`arch-be policy export <format>`).
#[derive(Clone, Copy, Debug, ValueEnum)]
pub(crate) enum PolicyFormat {
    /// Kyverno `ClusterPolicy` (`kyverno.io/v1`).
    Kyverno,
    /// OPA/Conftest Rego (пакет `archbe.deployment`).
    Rego,
}

impl PolicyFormat {
    /// Человекочитаемая метка формата.
    fn label(self) -> &'static str {
        match self {
            Self::Kyverno => "kyverno",
            Self::Rego => "rego",
        }
    }

    /// Формат библиотеки-экспортёра.
    fn export_format(self) -> arch_harness::policy_export::ExportFormat {
        match self {
            Self::Kyverno => arch_harness::policy_export::ExportFormat::Kyverno,
            Self::Rego => arch_harness::policy_export::ExportFormat::Rego,
        }
    }
}

/// `arch-be policy export <kyverno|rego>`: проекция секции `deployment:`
/// `CONSTRAINTS.yaml` в политики кластера. Пустая/отсутствующая секция —
/// сообщение и exit 0 (экспортировать нечего).
pub(crate) fn cmd_policy_export(
    format: PolicyFormat,
    repo: &Path,
    constraints: Option<PathBuf>,
    output: Option<&Path>,
) -> Result<()> {
    let c = resolve_constraints_cli(repo, constraints);
    if !c.is_file() {
        anyhow::bail!(
            "реестр правил не найден: ни {} в корне, ни {} — секцию `deployment:` читать неоткуда",
            arch_harness::control::ROOT_CONSTRAINTS_PATH,
            arch_harness::control::HANDOFF_CONSTRAINTS_PATH
        );
    }
    let policy = match arch_harness::policy_export::load(&c)? {
        None => {
            println!("секция deployment: не найдена, экспортировать нечего");
            return Ok(());
        }
        Some(policy) if !policy.has_invariants() => {
            println!("секция deployment: не содержит инвариантов, экспортировать нечего");
            return Ok(());
        }
        Some(policy) => policy,
    };
    let text = arch_harness::policy_export::export(&policy, format.export_format());
    match output {
        Some(path) => {
            std::fs::write(path, &text)
                .map_err(|e| anyhow::anyhow!("запись {}: {e}", path.display()))?;
            println!(
                "Экспорт {} → {} ({}: секция deployment:)",
                format.label(),
                path.display(),
                c.display()
            );
        }
        None => print!("{text}"),
    }
    Ok(())
}

/// `arch-be policy`: уровень автономии и классификация команды.
pub(crate) fn cmd_policy(cfg: &Config, check: Option<String>) -> Result<()> {
    let policy = arch_harness::policy::Policy::parse(&cfg.policy.autonomy)?;
    match check {
        None => {
            println!(
                "Уровень автономии: R{} (из config [policy] autonomy)",
                policy.level
            );
            println!(
                "  R0 — только чтения авто; R2 — + изменения (дефолт); R4 — деструктив с подтверждением; R5 — полная (красный флаг аудита)"
            );
        }
        Some(cmd) => {
            use arch_harness::policy::{PolicyDecision, classify_bash};
            let class = classify_bash(&cmd);
            let decision = policy.check("bash", &serde_json::json!({"command": cmd}));
            let verdict = match &decision {
                PolicyDecision::Allow => "ALLOW",
                PolicyDecision::RequireConfirm(_) => "REQUIRE-CONFIRM",
                PolicyDecision::Deny(_) => "DENY",
            };
            println!(
                "команда: {cmd}\nкласс риска: {class:?}\nрешение (R{}): {verdict}",
                policy.level
            );
            match &decision {
                PolicyDecision::RequireConfirm(m) | PolicyDecision::Deny(m) => {
                    println!("причина: {m}");
                }
                PolicyDecision::Allow => {}
            }
        }
    }
    Ok(())
}
