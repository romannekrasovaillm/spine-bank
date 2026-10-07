//! Обработчик `arch-be arch-diff` (волна K, ADR-063): архитектурный дифф PR
//! одним экраном / mermaid / JSON-контракт `arch-be/arch-diff/v1` / SARIF.
//! Информационная команда: exit 0 всегда, кроме `--fail-on` (exit 1).

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result};

use arch_harness::arch_diff::{
    ArchDiffInput, FailOn, arch_diff, matched_failures, render_json, render_md, render_mermaid,
    render_sarif,
};
use arch_harness::config::Config;

/// Прогон `arch-diff`: дифф, рендер выбранного формата, `--fail-on`.
pub(crate) fn cmd_arch_diff(
    cfg: &Config,
    repo: &Path,
    base: &str,
    head: Option<&str>,
    format: &str,
    trigger: &[String],
    fail_on: &[String],
) -> Result<()> {
    let (fast_max, standard_max) = cfg
        .significance
        .limits()
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    // Заявленные триггеры (`--trigger имя=true`), словарь — как у
    // `control score` (T-04: незнакомое имя — ошибка, а не завышение).
    let mut declared = BTreeMap::new();
    for t in trigger {
        let (k, v) = t
            .split_once('=')
            .with_context(|| format!("триггер '{t}' не вида имя=true"))?;
        declared.insert(k.to_string(), v == "true");
    }
    let unknown = arch_harness::control::unknown_trigger_names(&declared);
    if !unknown.is_empty() {
        anyhow::bail!(
            "arch-diff: {}",
            arch_harness::control::unknown_triggers_error(&unknown)
        );
    }
    let mut flags = Vec::new();
    for f in fail_on {
        let flag = FailOn::from_name(f).with_context(|| {
            format!(
                "неизвестное условие --fail-on '{f}' (допустимы: {})",
                FailOn::NAMES.join(", ")
            )
        })?;
        flags.push(flag);
    }
    let globs = cfg.significance.diff_globs();
    let diff = arch_diff(
        repo,
        &ArchDiffInput {
            base,
            head,
            declared,
            limits: (fast_max, standard_max),
            globs: &globs,
        },
    )
    .with_context(|| format!("архитектурный дифф {}", repo.display()))?;

    match format {
        "md" => print!("{}", render_md(&diff)),
        "mermaid" => {
            // Графы до/после — по уже разрешённым sha диффа (детерминировано).
            let base_g = arch_harness::arch_diff::as_built_with(repo, &diff.base, &globs)
                .context("граф базы для mermaid")?;
            let head_g = arch_harness::arch_diff::as_built_with(repo, &diff.head, &globs)
                .context("граф головы для mermaid")?;
            print!("{}", render_mermaid(&base_g, &head_g, &diff));
        }
        "json" => println!("{}", render_json(&diff)?),
        "sarif" => println!("{}", render_sarif(&diff)),
        other => {
            anyhow::bail!("неизвестный формат '{other}' (допустимы: md, mermaid, json, sarif)")
        }
    }

    let failed = matched_failures(&diff, &flags);
    if !failed.is_empty() {
        eprintln!("--fail-on сработал: {}", failed.join(", "));
        std::process::exit(1);
    }
    Ok(())
}
