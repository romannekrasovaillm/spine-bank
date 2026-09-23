//! Подкоманды `arch-be fleet` и `arch-be worktree` и их обработчики:
//! аудит флота worktree и изоляция агентной работы (B1: выделено из `main.rs`).

use std::path::PathBuf;
use std::sync::Arc;

#[cfg(feature = "harness")]
use anyhow::Context;
use anyhow::Result;
use clap::Subcommand;

use arch_harness::config::Config;

/// Подкоманды `arch-be fleet`.
#[derive(Subcommand)]
pub(crate) enum FleetCmd {
    /// SSOT-аудит флота: точные дубли документации и дрейф копий спайна.
    /// Дрейф хотя бы одного файла (разное содержимое у владельцев одного
    /// пути; канон — majority-версия) — exit code 1 (гейт для CI).
    Audit {
        /// Каталоги-worktree (каждый с копией архитектурных файлов).
        paths: Vec<PathBuf>,
        /// Репозиторий: worktree перечисляются из `git worktree list`
        /// (добавляются к позиционным путям).
        #[arg(long)]
        repo: Option<PathBuf>,
        /// Сузить сканирование glob'ом (повторяемый), напр. --include 'model/**'.
        /// По умолчанию — **/*.md|yaml|yml|json без .git/target/node_modules/.arch-handoff.
        #[arg(long)]
        include: Vec<String>,
        /// Формат вывода: text (таблица + топ расхождений) или json.
        #[arg(long, default_value = "text")]
        format: String,
        /// Exit 1, если доля точных дублей выше порога (проценты, напр. 50).
        #[arg(long)]
        fail_on_dupes: Option<f64>,
    },
    /// Гейт мерджа результата прогона флота (worktree `arch/<run-id>`) в
    /// основную ветку — «агент не имеет пути в main», интеграцию подтверждает
    /// владелец. Без --owner-approve печатает сводку прогона (diff stat,
    /// коммиты ветки, статус контракта из лога-evidence) и ОТКАЗЫВАЕТ мержить
    /// (exit 1). Режим гейта — [fleet] `merge_gate` ("owner" по умолчанию,
    /// "none" — без гейта). Только сборка `harness` (worktree-фабрика).
    #[cfg(feature = "harness")]
    Merge {
        /// Run-id прогона (имя worktree без префикса arch/, напр.
        /// claude-code-20260825103000 — его сообщает `harness_run` при
        /// [fleet] `require_worktree` = true).
        run_id: String,
        /// Явное подтверждение владельца: выполнить merge в основную ветку.
        #[arg(long)]
        owner_approve: bool,
        /// Репозиторий (по умолчанию — текущий каталог).
        #[arg(long)]
        repo: Option<PathBuf>,
    },
}

/// Подкоманды `arch-be worktree` (только сборка `harness`).
#[cfg(feature = "harness")]
#[derive(Subcommand)]
pub(crate) enum WorktreeCmd {
    /// Создать изолированный worktree (ветка arch/<name>).
    New {
        /// Имя (kebab-case [a-z0-9-]).
        name: String,
        /// Репозиторий (по умолчанию — текущий каталог).
        #[arg(long)]
        repo: Option<PathBuf>,
        /// Базовая ветка/коммит (по умолчанию HEAD).
        #[arg(long)]
        base: Option<String>,
    },
    /// Список worktree фабрики.
    List {
        /// Репозиторий (по умолчанию — текущий каталог).
        #[arg(long)]
        repo: Option<PathBuf>,
    },
    /// Diff ветки worktree против HEAD (review).
    Diff {
        /// Имя worktree.
        name: String,
        /// Репозиторий (по умолчанию — текущий каталог).
        #[arg(long)]
        repo: Option<PathBuf>,
    },
    /// Принять: merge в текущую ветку + уборка worktree.
    Accept {
        /// Имя worktree.
        name: String,
        /// Репозиторий (по умолчанию — текущий каталог).
        #[arg(long)]
        repo: Option<PathBuf>,
    },
    /// Удалить worktree без merge (только чистое).
    Drop {
        /// Имя worktree.
        name: String,
        /// Репозиторий (по умолчанию — текущий каталог).
        #[arg(long)]
        repo: Option<PathBuf>,
    },
}

/// `arch-be fleet`: аудит флота worktree (дубли/дрейф) и гейт мерджа прогонов.
// В core-сборке остаётся только синхронный аудит (гейт мерджа — за
// worktree-фабрикой сборки `harness`) — async-обёртка едина с полной сборкой.
#[cfg_attr(not(feature = "harness"), allow(clippy::unused_async))]
pub(crate) async fn cmd_fleet(cfg: &Arc<Config>, cmd: FleetCmd) -> Result<()> {
    // В core-сборке живёт только аудит (гейт мерджа — за worktree-фабрикой
    // сборки `harness`); конфиг нужен лишь ему.
    #[cfg(not(feature = "harness"))]
    let _ = cfg;
    match cmd {
        FleetCmd::Audit {
            paths,
            repo,
            include,
            format,
            fail_on_dupes,
        } => {
            let mut roots = paths;
            if let Some(repo) = repo {
                roots.extend(arch_harness::fleet::worktrees_from_git(&repo)?);
            }
            let report = arch_harness::fleet::audit(&roots, &include)?;
            match format.as_str() {
                "json" => println!("{}", serde_json::to_string_pretty(&report)?),
                "text" => print!("{}", arch_harness::fleet::render_text(&report)),
                other => anyhow::bail!("неизвестный формат '{other}' (допустимы: text, json)"),
            }
            // Независимые триггеры гейта: дрейф копий и порог доли дублей.
            let dupes_failed = fail_on_dupes.is_some_and(|pct| report.dup_pct > pct);
            if let Some(pct) = fail_on_dupes {
                if dupes_failed {
                    println!(
                        "Порог дублей превышен: {:.1}% > {pct:.1}% — exit 1",
                        report.dup_pct
                    );
                }
            }
            if report.has_drift || dupes_failed {
                std::process::exit(1);
            }
        }
        #[cfg(feature = "harness")]
        FleetCmd::Merge {
            run_id,
            owner_approve,
            repo,
        } => {
            let repo = match repo {
                Some(r) => r,
                None => std::env::current_dir().context("cwd")?,
            };
            match arch_harness::worktree::gated_merge(cfg, &repo, &run_id, owner_approve).await? {
                arch_harness::worktree::MergeGateOutcome::Refused(summary) => {
                    println!("{summary}");
                    std::process::exit(1);
                }
                arch_harness::worktree::MergeGateOutcome::Merged(msg) => println!("{msg}"),
            }
        }
    }
    Ok(())
}

/// `arch-be worktree`: изоляция агентной работы (создание, review, accept, drop).
#[cfg(feature = "harness")]
pub(crate) async fn cmd_worktree(cfg: &Arc<Config>, cmd: WorktreeCmd) -> Result<()> {
    let cwd = std::env::current_dir().context("cwd")?;
    let repo_of = |repo: Option<PathBuf>| repo.unwrap_or_else(|| cwd.clone());
    match cmd {
        WorktreeCmd::New { name, repo, base } => {
            let path =
                arch_harness::worktree::create(cfg, &repo_of(repo), &name, base.as_deref()).await?;
            println!("worktree создан: {}", path.display());
            println!(
                "review: arch-be worktree diff {name} · accept: arch-be worktree accept {name} · drop: arch-be worktree drop {name}"
            );
        }
        WorktreeCmd::List { repo } => {
            let infos = arch_harness::worktree::list(&repo_of(repo)).await?;
            print!("{}", arch_harness::worktree::render_list(&infos));
        }
        WorktreeCmd::Diff { name, repo } => {
            println!(
                "{}",
                arch_harness::worktree::diff(&repo_of(repo), &name).await?
            );
        }
        WorktreeCmd::Accept { name, repo } => {
            println!(
                "{}",
                arch_harness::worktree::accept(cfg, &repo_of(repo), &name).await?
            );
        }
        WorktreeCmd::Drop { name, repo } => {
            println!(
                "{}",
                arch_harness::worktree::drop(cfg, &repo_of(repo), &name).await?
            );
        }
    }
    Ok(())
}
