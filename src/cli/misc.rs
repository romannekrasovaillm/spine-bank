//! Разовые обработчики: `arch-be survey` (обратное обследование) и
//! `arch-be policy` (уровни автономии) (B1: выделено из `main.rs`).

use std::path::Path;

use anyhow::Result;

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
