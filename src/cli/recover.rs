//! Обработчик `arch-be recover` (R5, ADR-069): полный граф «как построено»
//! одной ревизии — без диффа и без merge-base — в форматах md | json
//! (`arch-be/recover/v1`) | mermaid | structurizr | c4-plantuml, плюс
//! опциональный черновик `model/` (`status: inferred`, `source: recover@<sha>`).
//!
//! Информационная команда: exit 0, кроме ошибок (тогда exit 1 через anyhow).
//! Повторный прогон с `--model-out` не затирает правки архитектора: сущность
//! со `status`, отличным от `inferred`, не трогается (список `skipped`).

use std::path::Path;

use anyhow::{Context, Result};

use arch_harness::arch_diff::{
    recover, render_c4_plantuml, render_recover_json, render_recover_md, render_recover_mermaid,
    render_structurizr,
};
use arch_harness::config::Config;

/// Прогон `recover`: граф ревизии в выбранном формате + черновик модели.
pub(crate) fn cmd_recover(
    cfg: &Config,
    repo: &Path,
    rev: Option<&str>,
    format: &str,
    model_out: Option<&Path>,
) -> Result<()> {
    let globs = cfg.significance.diff_globs();
    let outcome = recover(repo, rev, &globs, model_out)
        .with_context(|| format!("recover {}", repo.display()))?;
    let graph = &outcome.graph;

    match format {
        "md" => print!("{}", render_recover_md(graph)),
        "json" => println!("{}", render_recover_json(graph)?),
        "mermaid" => print!("{}", render_recover_mermaid(graph)),
        "structurizr" => print!("{}", render_structurizr(graph)),
        "c4-plantuml" => print!("{}", render_c4_plantuml(graph)),
        other => anyhow::bail!(
            "неизвестный формат '{other}' (допустимы: md, json, mermaid, structurizr, c4-plantuml)"
        ),
    }

    if let Some(dir) = &outcome.model_dir {
        eprintln!(
            "recover: черновик модели {} — записано {} сущностей ([inferred])",
            dir.display(),
            outcome.written.len()
        );
        if !outcome.written.is_empty() {
            eprintln!("  записано: {}", outcome.written.join(", "));
        }
        if !outcome.skipped.is_empty() {
            eprintln!(
                "  пропущено (правки архитектора, status ≠ inferred): {}",
                outcome.skipped.join(", ")
            );
        }
    }
    Ok(())
}
