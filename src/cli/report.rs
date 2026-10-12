//! Обработчик `arch-be report` (ADR-080, этап 1): отчёт о восстановленной
//! архитектуре одной ревизии — форматы md | json (`arch-be/report/v1`) | check.
//!
//! Тонкая обёртка над `arch_harness::report::build` (AD-11 дельты `report-v1`:
//! сборка модели и назначение ID/уверенности живут только в `src/report/`).
//!
//! Fail-closed: если автопроверка модели нашла блокирующие находки (`error`),
//! выдача блокируется — находки печатаются в stderr, exit 1. Байпас-флага нет.
//! Формат `check` — только автопроверка без рендера отчёта.

use std::path::Path;

use anyhow::{Context, Result};

use arch_harness::report::{build, render_check, render_json, render_md};

/// Прогон `report`: отчёт о восстановленной архитектуре в формате md | json | check.
pub(crate) fn cmd_report(
    repo: &Path,
    rev: Option<&str>,
    format: &str,
    out: Option<&Path>,
) -> Result<()> {
    let model = build(repo, rev).with_context(|| format!("report {}", repo.display()))?;

    // Fail-closed: блокирующие находки не пропускаем ни в одном формате.
    if model.has_errors() {
        eprint!("{}", render_check(&model));
        anyhow::bail!(
            "report: fail-closed — {} error (выдача заблокирована, байпас-флага нет)",
            model.error_count()
        );
    }

    let rendered = match format {
        "md" => render_md(&model),
        "json" => render_json(&model)?,
        "check" => render_check(&model),
        other => anyhow::bail!("неизвестный формат '{other}' (допустимы: md, json, check)"),
    };
    match out {
        Some(path) => {
            std::fs::write(path, rendered).with_context(|| format!("запись {}", path.display()))?;
            eprintln!(
                "report: {} — элементов {}, связей {}, error {} / warn {}; записано в {}",
                model.meta.rev,
                model.elements.len(),
                model.relations.len(),
                model.error_count(),
                model.warn_count(),
                path.display()
            );
        }
        None => print!("{rendered}"),
    }
    Ok(())
}
