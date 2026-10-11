//! Команда `landscape` (AB-02, ADR-077): сверка ландшафта с выгрузкой CMDB.

use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Subcommand;

use arch_harness::arch_diff::{NodeKind, as_built_with};
use arch_harness::config::Config;
use arch_harness::landscape::{
    diff_cmdb, load_aliases, render_cmdb_diff_json, render_cmdb_diff_markdown,
};

/// Подкоманды `arch-be landscape`.
#[derive(Debug, Subcommand)]
pub(crate) enum LandscapeCmd {
    /// Сверка ландшафта с выгрузкой CMDB (AB-02, ADR-077): три класса
    /// расхождений — система не в CMDB, запись CMDB без модели/кода,
    /// конфликт владельца.
    Diff {
        /// Корень ландшафта (сам каталог + непосредственные подкаталоги с
        /// `model/`).
        #[arg(long, default_value = ".")]
        root: PathBuf,
        /// Файл выгрузки CMDB (yaml или json).
        #[arg(long)]
        cmdb: PathBuf,
        /// Карта алиасов имён (yaml/json; как у `landscape`-агрегации).
        #[arg(long)]
        aliases: Option<PathBuf>,
        /// Формат: md | json (arch-be/landscape-diff/v1).
        #[arg(long, default_value = "md")]
        format: String,
        /// Каталог черновика модели из записей CMDB (`status: inferred`;
        /// повторный прогон не затирает правки архитектора).
        #[arg(long = "model-out")]
        model_out: Option<PathBuf>,
        /// Корень репозитория кода для recover-графа (тогда сверка идёт
        /// и по коду).
        #[arg(long)]
        repo: Option<PathBuf>,
        /// Ревизия для recover-графа (по умолчанию HEAD).
        #[arg(long)]
        rev: Option<String>,
    },
}

/// Обработчик `arch-be landscape ...`.
///
/// # Errors
/// Ошибки чтения модели/CMDB, построения графа кода, записи черновика.
pub(crate) fn cmd_landscape(cfg: &Config, cmd: LandscapeCmd) -> Result<()> {
    match cmd {
        LandscapeCmd::Diff {
            root,
            cmdb,
            aliases,
            format,
            model_out,
            repo,
            rev,
        } => {
            let alias_map: BTreeMap<String, String> = match &aliases {
                Some(p) => {
                    load_aliases(p).with_context(|| format!("карта алиасов {}", p.display()))?
                }
                None => BTreeMap::new(),
            };
            let code_systems: Vec<String> = match &repo {
                Some(r) => {
                    let globs = cfg.significance.diff_globs();
                    let graph = as_built_with(r, rev.as_deref().unwrap_or("HEAD"), &globs)
                        .with_context(|| format!("recover-граф {}", r.display()))?;
                    graph
                        .nodes
                        .iter()
                        .filter(|n| matches!(n.kind, NodeKind::Component))
                        .map(|n| n.title.clone())
                        .collect()
                }
                None => Vec::new(),
            };
            let report = diff_cmdb(
                &root,
                &cmdb,
                &alias_map,
                &code_systems,
                model_out.as_deref(),
            )?;
            match format.as_str() {
                "md" => print!("{}", render_cmdb_diff_markdown(&report)),
                "json" => println!("{}", render_cmdb_diff_json(&report)?),
                other => anyhow::bail!("неизвестный формат '{other}' (ожидается md|json)"),
            }
            if let Some(dir) = &report.model_dir {
                eprintln!(
                    "landscape diff: черновик модели {} — записано {} систем ([inferred])",
                    dir.display(),
                    report.written.len()
                );
                if !report.written.is_empty() {
                    eprintln!("  записано: {}", report.written.join(", "));
                }
                if !report.skipped.is_empty() {
                    eprintln!("  пропущено (уже в модели): {}", report.skipped.join(", "));
                }
            }
            eprintln!(
                "landscape diff: ошибок {}, предупреждений {}",
                report.error_count(),
                report.warn_count()
            );
            Ok(())
        }
    }
}
