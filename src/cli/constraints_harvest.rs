//! Обработчик `arch-be constraints-harvest` (ADR-073): детерминированный
//! (без LLM, AD-2) сборщик фактов-ограничений для секции Context ADR
//! (цикл DECIDE) и гейтов GOVERN/COMPLY.
//!
//! Информационная команда: exit 0, кроме ошибок (тогда exit 1 через anyhow).
//! Форматы: md (человекочитаемо, секции + `файл:строка`) | json
//! (`arch-be/constraints-harvest/v1`). `--out` пишет отчёт в файл,
//! иначе — в stdout. Два прогона на одном дереве байт-в-байт идентичны.

use std::path::Path;

use anyhow::{Context, Result};

use arch_harness::constraints_harvest::{harvest, render_json, render_md};

/// Прогон `constraints-harvest`: отчёт фактов в формате md | json (+ `--out`).
pub(crate) fn cmd_constraints_harvest(repo: &Path, format: &str, out: Option<&Path>) -> Result<()> {
    let h = harvest(repo).with_context(|| format!("constraints-harvest {}", repo.display()))?;
    let rendered = match format {
        "md" => render_md(&h),
        "json" => render_json(&h)?,
        other => anyhow::bail!("неизвестный формат '{other}' (допустимы: md, json)"),
    };
    match out {
        Some(path) => {
            std::fs::write(path, rendered).with_context(|| format!("запись {}", path.display()))?;
            eprintln!(
                "constraints-harvest: {} — фактов {}, пробелов {}; записано в {}",
                h.repo,
                h.fact_count(),
                h.gap_count(),
                path.display()
            );
        }
        None => print!("{rendered}"),
    }
    Ok(())
}
