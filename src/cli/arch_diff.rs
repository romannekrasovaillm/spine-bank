//! Обработчик `arch-be arch-diff` (волна K, ADR-063): архитектурный дифф PR
//! одним экраном / mermaid / JSON-контракт `arch-be/arch-diff/v1` / SARIF.
//! Информационная команда: exit 0 всегда, кроме `--fail-on` (exit 1).
//! Подкоманды `accept`/`reject` (K5, ADR-064): предложения модели оформляются
//! дельтой либо отклоняются записью в журнал решений.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::Subcommand;

use arch_harness::arch_diff::{
    ArchDiffInput, DecideInput, FailOn, arch_diff, matched_failures, render_json, render_md,
    render_mermaid, render_sarif,
};
use arch_harness::config::Config;

/// Подкоманды `arch-be arch-diff` (K5, ADR-064): решения по предложениям
/// правки модели.
#[derive(Subcommand)]
pub(crate) enum ArchDiffCmd {
    /// Принять пронумерованные предложения: оформляет их дельтой
    /// changes/<name>/DELTA.md и пишет accept-записи в журнал решений
    /// (.arch-handoff/arch-diff-decisions.json). Файлы model/ команда не
    /// правит: модель меняется через дельту (delta guard действует как
    /// обычно). Предложение с ⚠ принимается, конфликт фиксируется в дельте.
    Accept {
        /// Номера предложений из свежего вывода `arch-be arch-diff --base …`.
        numbers: Vec<usize>,
        /// База диффа (та же, что у прогона с предложениями).
        #[arg(long)]
        base: String,
        /// Голова диффа (по умолчанию HEAD).
        #[arg(long)]
        head: Option<String>,
        /// Имя дельты (по умолчанию arch-diff-<head8>; занято — суффикс -2…).
        #[arg(long)]
        name: Option<String>,
        /// Корень репозитория.
        #[arg(long, default_value = ".")]
        repo: PathBuf,
    },
    /// Отклонить предложение с причиной: запись в журнал решений; повторно
    /// ребро не предлагается, пока не изменились его основания (файл:строка).
    Reject {
        /// Номер предложения из свежего вывода `arch-be arch-diff --base …`.
        n: usize,
        /// Причина отклонения (обязательна: журнал без причины бессмыслен).
        #[arg(long)]
        reason: String,
        /// База диффа (та же, что у прогона с предложениями).
        #[arg(long)]
        base: String,
        /// Голова диффа (по умолчанию HEAD).
        #[arg(long)]
        head: Option<String>,
        /// Корень репозитория.
        #[arg(long, default_value = ".")]
        repo: PathBuf,
    },
}

/// Прогон `arch-diff`: дифф, рендер выбранного формата, `--fail-on`.
pub(crate) fn cmd_arch_diff(
    cfg: &Config,
    repo: &Path,
    base: Option<&str>,
    head: Option<&str>,
    format: &str,
    trigger: &[String],
    fail_on: &[String],
) -> Result<()> {
    // `--base` обязателен для просмотра (clap отпускает: обязательность
    // мешала бы подкомандам accept/reject) — проверяем на краю CLI.
    let base = base.ok_or_else(|| {
        anyhow::anyhow!(
            "arch-diff: укажите --base <ревизия> (база диффа, напр. origin/main; \
             диапазон A...B тоже принимается)"
        )
    })?;
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

/// `arch-diff accept` / `arch-diff reject` (K5, ADR-064): решения по
/// предложениям правки модели.
pub(crate) fn cmd_arch_diff_decide(cfg: &Config, cmd: &ArchDiffCmd) -> Result<()> {
    let limits = cfg
        .significance
        .limits()
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let globs = cfg.significance.diff_globs();
    // Источник решения — честно из окружения (ARCH_BE_ACTOR), без механики
    // принуждения (связывание с A3 — ADR-060, не эта волна).
    let source = arch_harness::arch_diff::detect_source();
    match cmd {
        ArchDiffCmd::Accept {
            numbers,
            base,
            head,
            name,
            repo,
        } => {
            let report = arch_harness::arch_diff::accept_proposals(
                repo,
                &DecideInput {
                    base,
                    head: head.as_deref(),
                    limits,
                    globs: &globs,
                },
                numbers,
                name.as_deref(),
                source,
            )?;
            println!(
                "Принятые предложения оформлены дельтой: {}",
                report.delta_path.display()
            );
            let ns: Vec<String> = report
                .accepted
                .iter()
                .map(|p| format!("№{}", p.n))
                .collect();
            println!(
                "Принято: {} ({}) — записано в журнал {} (источник: {})",
                report.accepted.len(),
                ns.join(", "),
                report.journal_path.display(),
                source.label()
            );
            for p in &report.accepted {
                if !p.conflicts.is_empty() {
                    println!(
                        "⚠ №{} ({}) принято с конфликтом: противоречит {} — конфликт \
                         зафиксирован в дельте",
                        p.n,
                        p.file,
                        p.conflicts.join(", ")
                    );
                }
            }
            if source == arch_harness::arch_diff::DecisionSource::Unknown {
                println!(
                    "источник решения не задан (unknown); для честного учёта — \
                     ARCH_BE_ACTOR=human|agent"
                );
            }
            println!("\nСледующие шаги:");
            println!(
                "  1. внесите правки в файлы model/ по тексту дельты — для delta guard они \
                 покрыты дельтой '{}'",
                report.delta_name
            );
            println!(
                "  2. проверка: arch-be delta validate {}; после применения — arch-be delta \
                 archive {}",
                report.delta_name, report.delta_name
            );
        }
        ArchDiffCmd::Reject {
            n,
            reason,
            base,
            head,
            repo,
        } => {
            let report = arch_harness::arch_diff::reject_proposal(
                repo,
                &DecideInput {
                    base,
                    head: head.as_deref(),
                    limits,
                    globs: &globs,
                },
                *n,
                reason,
                source,
            )?;
            println!(
                "Отклонено предложение №{n} ({}): {}",
                report.entry.edge_id, report.entry.reason
            );
            println!(
                "Записано в {} (источник: {}); повторно не предлагается, пока не изменятся \
                 основания (файл:строка) ребра",
                report.journal_path.display(),
                source.label()
            );
        }
    }
    Ok(())
}
