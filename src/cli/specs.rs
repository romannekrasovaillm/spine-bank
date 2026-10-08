//! Подкоманды спецификаций и публикаций: `adr`, `publish`, `evidence`,
//! `delta`, `openspec`, `agents-md` и их обработчики (B1: выделено из `main.rs`).

use std::path::PathBuf;

use anyhow::Result;
use clap::Subcommand;

use arch_harness::config::Config;

/// Подкоманды `arch-be adr` (реестр ADR, ADR-036).
#[derive(Subcommand)]
pub(crate) enum AdrCmd {
    /// Глобальный реестр ADR по набору проектов: сам ROOT + непосредственные
    /// подкаталоги; источники — docs/adr/*.md (проза) и model/ADR-*.md
    /// (типизированные сущности ADR-003).
    Registry {
        /// Корневой каталог набора проектов.
        root: PathBuf,
        /// Машиночитаемый вывод: единый JSON {entries, findings}.
        #[arg(long)]
        json: bool,
        /// Exit 1 при любой находке (расхождение прозы и типизированной
        /// записи `prose_model_divergence`, коллизия номеров внутри одного
        /// представления, дубль заголовка, пропуск даты/статуса) — гейт для
        /// CI; по умолчанию exit 0.
        #[arg(long)]
        strict: bool,
    },
}

/// Подкоманды `arch-be publish` (файловые адаптеры, ADR-033).
#[derive(Subcommand)]
pub(crate) enum PublishCmd {
    /// Markdown → Confluence storage format (XHTML) в stdout: заголовки,
    /// таблицы, код-блоки, списки, инлайн-разметка (подмножество).
    Confluence {
        /// Markdown-файл (spine, ADR, evidence-индекс).
        file: PathBuf,
    },
    /// JSON результата handoff → Jira-CSV импорта (Summary,Type,Description,Labels).
    Jira {
        /// Файл результата handoff (`status`/`assumptions`/`open_questions`/…).
        result: PathBuf,
        /// Ключ проекта Jira — метка `spine-<ключ>` для фильтрации.
        #[arg(long)]
        project: Option<String>,
    },
}

#[derive(Subcommand)]
pub(crate) enum EvidenceCmd {
    /// Собрать bundle (EVIDENCE.yaml) по каталогу изменения.
    Pack {
        /// Каталог изменения.
        dir: PathBuf,
        /// Маршрут: fast|standard|critical.
        #[arg(long, default_value = "standard")]
        route: String,
    },
    /// Проверить bundle: полнота + целостность хэшей.
    Verify {
        /// Каталог изменения.
        dir: PathBuf,
    },
    /// Записать машинное evidence прогона (A1): выполняет команду и пишет
    /// `.arch-handoff/evidence/<kind>.json` (команда, exit-код, время, HEAD,
    /// хэш входов, итог). Отчёт о прогоне пишет машина, а не автор.
    Record {
        /// Вид записи: fitness (прогон реестра правил), tests (тесты),
        /// skeleton (walking skeleton).
        kind: String,
        /// Каталог кейса/изменения (по умолчанию — текущий).
        #[arg(long)]
        dir: Option<PathBuf>,
        /// Команда прогона. Для fitness по умолчанию `arch-be control check .`;
        /// для tests/skeleton — обязательна.
        #[arg(long)]
        cmd: Option<String>,
        /// Таймаут прогона в секундах (0 — дефолт 900).
        #[arg(long, default_value = "0")]
        timeout_secs: u64,
    },
}

#[derive(Subcommand)]
pub(crate) enum DeltaCmd {
    /// Новая дельта (каркас changes/<name>/DELTA.md).
    New {
        /// Имя изменения (kebab-case).
        name: String,
        /// Репозиторий (по умолчанию — текущий каталог).
        #[arg(long)]
        repo: Option<PathBuf>,
    },
    /// Список дельт (предложенные/архивные).
    List {
        /// Репозиторий.
        #[arg(long)]
        repo: Option<PathBuf>,
    },
    /// Валидация структуры дельты.
    Validate {
        /// Имя дельты.
        name: String,
        /// Репозиторий.
        #[arg(long)]
        repo: Option<PathBuf>,
    },
    /// Архивировать дельту после apply (вливание в живую истину).
    Archive {
        /// Имя дельты.
        name: String,
        /// Репозиторий.
        #[arg(long)]
        repo: Option<PathBuf>,
    },
    /// Гейт прямых правок спайна: изменённые защищённые файлы обязаны
    /// упоминаться в активной дельте changes/<name>/DELTA.md, иначе exit 1.
    /// Новые untracked-файлы git-diff не видит — для CI используйте --base.
    Guard {
        /// Репозиторий (по умолчанию — текущий каталог).
        #[arg(long)]
        repo: Option<PathBuf>,
        /// База diff (по умолчанию HEAD — staged+unstaged рабочего дерева;
        /// для CI — напр. origin/main...HEAD: трёхточечную форму разбирает
        /// сам git).
        #[arg(long)]
        base: Option<String>,
        /// Защищаемый путь/префикс (повторяемый). Если задан хотя бы один —
        /// заменяет дефолт: model/, ARCHITECTURE-SPINE.md, `CONSTRAINTS.yaml`.
        #[arg(long)]
        protect: Vec<String>,
    },
}

/// Подкоманды `arch-be openspec` (адаптер `OpenSpec`, MVP; `docs/openspec.md`).
#[derive(Subcommand)]
pub(crate) enum OpenspecCmd {
    /// Список требований `OpenSpec`: живые спеки (openspec/specs/) и дельты
    /// активных changes; стабильный id `openspec:<capability>#<hash8>`,
    /// текст, источник (файл:строка).
    Scan {
        /// Корень репозитория с разметкой `OpenSpec`.
        root: PathBuf,
        /// Машиночитаемый вывод: JSON-отчёт `ScanReport`.
        #[arg(long)]
        json: bool,
    },
    /// Отчёт покрытия требований правилами CONSTRAINTS (связь — поле
    /// `covers:` правила): SHALL всего / покрыто детектором / unverifiable
    /// с owner / без решения; непокрытые — поимённо. Exit code: 0, если нет
    /// --strict; с --strict — 1 при наличии требований «без решения»
    /// (ни детектора, ни unverifiable с назначенным owner).
    Coverage {
        /// Корень репозитория с разметкой `OpenSpec`.
        root: PathBuf,
        /// Файл ограничений (по умолчанию <root>/.arch-handoff/CONSTRAINTS.yaml,
        /// иначе <root>/CONSTRAINTS.yaml; нет файла — все «без решения»).
        #[arg(long)]
        constraints: Option<PathBuf>,
        /// Машиночитаемый вывод: JSON-отчёт `CoverageReport`.
        #[arg(long)]
        json: bool,
        /// Строгий режим: exit 1 при требованиях «без решения» (гейт CI).
        #[arg(long)]
        strict: bool,
    },
    /// Генерация артефактов перехода `OpenSpec` → Spine: скелет
    /// CONSTRAINTS.from-openspec.yaml (все SHALL как заглушки
    /// `unverifiable: true` с пустым owner и проставленным `covers:`),
    /// SPINE.draft.md (кандидаты из design.md активных changes) и печать
    /// отчёта покрытия. Существующие файлы не затираются без --force.
    Init {
        /// Корень репозитория с разметкой `OpenSpec`.
        root: PathBuf,
        /// Каталог вывода (по умолчанию — сам ROOT).
        #[arg(long)]
        out: Option<PathBuf>,
        /// Перезаписать существующие файлы (регенерация детерминирована).
        #[arg(long)]
        force: bool,
    },
    /// Гейт change (точка CI). Два режима:
    /// `--archive <change-id>` — гейт архивации (перед `openspec archive`):
    /// exit 1, если у требований change нет решения (ни детектора, ни
    /// unverifiable с owner) или падает `control check`;
    /// `--change <id>` — гейт активного change (F3, ADR-067): покрытие
    /// требований его дельты, `delta_guard` с этим change как источником,
    /// `control check` и маршрут по диффу диапазона change; exit 1 при
    /// провале. (Roadmap: --expiry — `docs/openspec.md`.)
    Gate {
        /// Режим гейта: архивация change.
        #[arg(long)]
        archive: bool,
        /// Режим гейта: активный change (MR, реализующий change) — id
        /// задаётся значением флага: `openspec gate --change add-limits .`.
        #[arg(long, value_name = "ID", conflicts_with = "archive")]
        change: Option<String>,
        /// Корень репозитория с разметкой `OpenSpec`.
        root: PathBuf,
        /// Идентификатор change (для --archive; при --change id идёт
        /// значением флага).
        change_id: Option<String>,
        /// Файл ограничений (умолчание — как у `coverage`).
        #[arg(long)]
        constraints: Option<PathBuf>,
        /// База git для диффа диапазона change (только --change; по умолчанию
        /// HEAD — рабочее дерево; для CI — напр. origin/main...HEAD).
        #[arg(long)]
        base: Option<String>,
    },
}

#[derive(Subcommand)]
pub(crate) enum AgentsMdCmd {
    /// Сгенерировать или обновить AGENTS.md (рукописная зона сохраняется).
    Refresh {
        /// Репозиторий.
        repo: PathBuf,
    },
    /// Проверить AGENTS.md: свежесть (дрейф источников), ссылки, заглушки.
    Lint {
        /// Репозиторий.
        repo: PathBuf,
    },
    /// Прогнать линтер по реестру репозиториев (файл: путь на строку).
    LintAll {
        /// Файл реестра (по умолчанию ~/.arch-harness/repos.txt).
        #[arg(long)]
        registry: Option<PathBuf>,
    },
}

/// `arch-be adr`: реестр ADR по набору проектов (ADR-036).
pub(crate) fn cmd_adr(cmd: AdrCmd) -> Result<()> {
    match cmd {
        AdrCmd::Registry { root, json, strict } => {
            let report = arch_harness::adr_registry::build_registry(&root)?;
            if json {
                println!(
                    "{}",
                    serde_json::to_string(&report).expect("RegistryReport сериализуется")
                );
            } else {
                println!("{}", arch_harness::adr_registry::render_markdown(&report));
            }
            let code = arch_harness::adr_registry::exit_code(&report, strict);
            if code != 0 {
                std::process::exit(code);
            }
        }
    }
    Ok(())
}

/// Публикация артефактов в корпоративные системы (файловые адаптеры, ADR-033).
pub(crate) fn cmd_publish(cmd: PublishCmd) -> Result<()> {
    match cmd {
        PublishCmd::Confluence { file } => {
            let md = std::fs::read_to_string(&file)
                .map_err(|e| arch_harness::error::HarnessError::io(&file, e))?;
            println!("{}", arch_harness::publish::markdown_to_confluence(&md));
        }
        PublishCmd::Jira { result, project } => {
            print!(
                "{}",
                arch_harness::publish::handoff_json_to_jira_csv(&result, project.as_deref())?
            );
        }
    }
    Ok(())
}

/// `arch-be evidence`: Evidence Bundle.
pub(crate) fn cmd_evidence(cfg: &arch_harness::config::Config, cmd: EvidenceCmd) -> Result<()> {
    match cmd {
        EvidenceCmd::Pack { dir, route } => {
            let route = match route.to_lowercase().as_str() {
                "fast" => arch_harness::control::Route::Fast,
                "critical" => arch_harness::control::Route::Critical,
                _ => arch_harness::control::Route::Standard,
            };
            // A2: уровень риска выводится из записи значимости (триггеры и
            // источники — из диффа рабочего дерева), а не из рукописного
            // RISK.md.
            let limits = cfg
                .significance
                .limits()
                .map_err(|e| anyhow::anyhow!("маршруты значимости: {e}"))?;
            let significance = arch_harness::evidence::significance_record(
                &dir,
                route,
                limits,
                &cfg.significance.diff_globs(),
            );
            let (bundle, verdict) =
                arch_harness::evidence::pack_with(&dir, route, Some(significance))?;
            println!("{}", verdict.summary);
            for item in &bundle.items {
                println!("  + {:<20} {} ({} б)", item.key, item.path, item.size);
            }
            for miss in &verdict.missing {
                println!("  ✗ ОТСУТСТВУЕТ: {miss}");
            }
            // Раздельный счёт церемонии (A2): что пишет автор, что выводит машина.
            if let Some(p) = arch_harness::evidence::bundle_progress_split(&dir, route) {
                println!(
                    "Бандл: пишет автор {}/{} · выведет машина {}/{}",
                    p.author_done, p.author_total, p.machine_done, p.machine_total
                );
            }
            println!("Манифест: {}", dir.join("EVIDENCE.yaml").display());
            if !verdict.passed {
                std::process::exit(1);
            }
        }
        EvidenceCmd::Verify { dir } => {
            let v = arch_harness::evidence::verify_with(&dir, &cfg.evidence)?;
            println!("{}", v.summary);
            for w in &v.warnings {
                println!("  ⚠ {w}");
            }
            for m in &v.missing {
                println!("  ✗ ОТСУТСТВУЕТ: {m}");
            }
            for t in &v.tampered {
                println!("  ✗ ИЗМЕНЁН: {t}");
            }
            // Содержание артефактов (Н1, ADR-041): «есть» ≠ «написан».
            for f in &v.semantics {
                let mark = if f.severity == "error" { "✗" } else { "⚠" };
                println!("  {mark} [{}] {}: {}", f.rule, f.key, f.message);
                println!("      → {}", f.fix_hint);
            }
            // Заявленное, но механикой не проверяемое — печатается всегда:
            // Spine не притворяется, что удостоверил подпись или смысл.
            for n in &v.not_verified {
                println!("  · не проверяется механикой: {n}");
            }
            println!(
                "Итог: {}",
                if v.passed {
                    "PASS — выпуск разрешён"
                } else {
                    "FAIL — выпуск заблокирован"
                }
            );
            // W1: вердикт бандла — часть вердикта гейта, а тот печатает
            // паспорт. Ссылка нужна здесь, потому что читатель бандла до
            // гейта может и не дойти.
            println!(
                "Паспорт вердикта (что зелёный НЕ означает): {}",
                arch_harness::passport::hint_command(&dir)
            );
            if !v.passed {
                std::process::exit(1);
            }
        }
        EvidenceCmd::Record {
            kind,
            dir,
            cmd,
            timeout_secs,
        } => {
            let kind: arch_harness::evidence::RecordKind = kind
                .parse()
                .map_err(|e: String| anyhow::anyhow!("evidence record: {e}"))?;
            let dir = dir.unwrap_or_else(|| PathBuf::from("."));
            let Some(command) = cmd.or_else(|| kind.default_command().map(str::to_string)) else {
                anyhow::bail!(
                    "для записи «{}» нет команды по умолчанию — укажите её: \
                     arch-be evidence record {} --cmd \"…\"",
                    kind.as_str(),
                    kind.as_str()
                );
            };
            let rec = arch_harness::evidence::record_run(&dir, kind, &command, timeout_secs)?;
            println!(
                "Запись прогона «{}»: {}",
                kind.as_str(),
                if rec.passed { "PASS" } else { "FAIL" }
            );
            println!(
                "  команда: {} (exit {}, {:.1} с)",
                rec.command,
                rec.exit_code
                    .map_or_else(|| "таймаут".to_string(), |c| c.to_string()),
                rec.duration_secs
            );
            println!("  HEAD: {}", rec.head);
            println!("  входы: {} ({})", rec.inputs_hash, rec.inputs_note);
            if let Some(fp) = &rec.registry_fingerprint {
                println!("  реестр: {fp}");
            }
            println!(
                "  файл: {}",
                arch_harness::evidence::record_path(&dir, kind).display()
            );
            if !rec.passed {
                println!("Итог: FAIL — запись зафиксировала провал прогона");
                std::process::exit(1);
            }
        }
    }
    Ok(())
}

/// `arch-be delta`: дельта-спецификации.
pub(crate) fn cmd_delta(cmd: DeltaCmd) -> Result<()> {
    let cwd = || std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    match cmd {
        DeltaCmd::New { name, repo } => {
            let root = repo.unwrap_or_else(cwd);
            // F1 (ADR-062): репозиторий на чистом OpenSpec (`[delta] sources =
            // ["openspec"]`) — DELTA.md не создаём (двойной учёт), изменение
            // оформляется change'ом OpenSpec; markdown OpenSpec Spine не пишет.
            if let Some(hint) = arch_harness::delta::openspec_only_hint(&root)? {
                anyhow::bail!("{hint}");
            }
            let path = arch_harness::delta::new(&root, &name)?;
            println!("Дельта создана: {}", path.display());
        }
        DeltaCmd::List { repo } => {
            let list = arch_harness::delta::list(&repo.unwrap_or_else(cwd));
            if list.is_empty() {
                println!("Дельт нет (changes/ пуст или отсутствует).");
            }
            for d in &list {
                println!("  {:<30} {:?}", d.name, d.status);
            }
        }
        DeltaCmd::Validate { name, repo } => {
            let issues = arch_harness::delta::validate(&repo.unwrap_or_else(cwd), &name)?;
            if issues.is_empty() {
                println!("дельта '{name}': нарушений нет");
            }
            let mut failed = false;
            for i in &issues {
                println!(
                    "[{}] {}:{} {} — {}",
                    i.severity,
                    i.file.display(),
                    i.line,
                    i.rule,
                    i.message
                );
                failed |= i.severity == "error";
            }
            if failed {
                std::process::exit(1);
            }
        }
        DeltaCmd::Archive { name, repo } => {
            let root = repo.unwrap_or_else(cwd);
            let path = arch_harness::delta::archive(&root, &name)?;
            println!("Дельта заархивирована: {}", path.display());
            if let Some(hint) = arch_harness::delta::archive_order_hint(&root) {
                println!("  → {hint}");
            }
        }
        DeltaCmd::Guard {
            repo,
            base,
            protect,
        } => {
            let report =
                arch_harness::delta::guard(&repo.unwrap_or_else(cwd), base.as_deref(), &protect)?;
            print!("{}", arch_harness::delta::render_guard(&report));
            if !report.passed {
                std::process::exit(1);
            }
        }
    }
    Ok(())
}

/// `arch-be openspec`: адаптер `OpenSpec` — требования → покрытие fitness-правилами.
pub(crate) fn cmd_openspec(cfg: &Config, cmd: OpenspecCmd) -> Result<()> {
    match cmd {
        OpenspecCmd::Scan { root, json } => {
            let requirements = arch_harness::openspec::scan_requirements(&root)?;
            if json {
                let report = arch_harness::openspec::ScanReport {
                    total: requirements.len(),
                    root,
                    requirements,
                };
                // SDK-контракт v1: машиночитаемый отчёт в stdout.
                println!(
                    "{}",
                    serde_json::to_string(&report).expect("ScanReport сериализуется")
                );
            } else {
                print!(
                    "{}",
                    arch_harness::openspec::render_scan(&root, &requirements)
                );
            }
        }
        OpenspecCmd::Coverage {
            root,
            constraints,
            json,
            strict,
        } => {
            let report = arch_harness::openspec::coverage(&root, constraints.as_deref())?;
            if json {
                println!(
                    "{}",
                    serde_json::to_string(&report).expect("CoverageReport сериализуется")
                );
            } else {
                print!("{}", report.to_markdown());
            }
            // Строгий режим: требования «без решения» ломают гейт (exit 1).
            if strict && report.unresolved > 0 {
                std::process::exit(1);
            }
        }
        OpenspecCmd::Init { root, out, force } => {
            let out_dir = out.unwrap_or_else(|| root.clone());
            let outcome = arch_harness::openspec::init(&root, &out_dir, force)?;
            println!("Записано: {}", outcome.constraints_path.display());
            println!("Записано: {}", outcome.spine_path.display());
            println!(
                "Правил-заглушек: {}, кандидатов в спайн: {}, истории (archive): {}",
                outcome.rules, outcome.candidates, outcome.history
            );
            print!("{}", outcome.coverage.to_markdown());
        }
        OpenspecCmd::Gate {
            archive,
            change,
            root,
            change_id,
            constraints,
            base,
        } => {
            // F3: гейт активного change (MR, реализующий change).
            if let Some(change) = change {
                if change_id.is_some() {
                    anyhow::bail!(
                        "с --change идентификатор задаётся значением флага, позиционный <change-id> не нужен"
                    );
                }
                let limits = cfg
                    .significance
                    .limits()
                    .map_err(|e| anyhow::anyhow!("маршруты значимости: {e}"))?;
                let report = arch_harness::openspec::gate_change(
                    &root,
                    &change,
                    constraints.as_deref(),
                    base.as_deref(),
                    limits,
                    &cfg.significance.diff_globs(),
                )?;
                print!("{}", report.to_markdown());
                if !report.passed {
                    std::process::exit(1);
                }
                return Ok(());
            }
            if !archive {
                anyhow::bail!(
                    "укажите режим гейта: --archive <change-id> или --change <id> \
                     (roadmap: --expiry — docs/openspec.md)"
                );
            }
            let Some(change_id) = change_id else {
                anyhow::bail!("--archive требует <change-id> позиционным аргументом");
            };
            let report =
                arch_harness::openspec::gate_archive(&root, &change_id, constraints.as_deref())?;
            print!("{}", report.to_markdown());
            if !report.passed {
                std::process::exit(1);
            }
        }
    }
    Ok(())
}

/// `arch-be agents-md`: AGENTS.md как канал архитектурного контроля.
pub(crate) fn cmd_agents_md(cfg: &Config, cmd: AgentsMdCmd) -> Result<()> {
    match cmd {
        AgentsMdCmd::Refresh { repo } => {
            let report = arch_harness::agentsmd::generate(&repo)?;
            println!(
                "AGENTS.md: {} ({}) — инвариантов: {}, fitness: {}",
                report.path.display(),
                report.action,
                report.invariants,
                if report.has_constraints {
                    "да"
                } else {
                    "нет"
                }
            );
        }
        AgentsMdCmd::Lint { repo } => {
            let issues = arch_harness::agentsmd::lint(&repo)?;
            if issues.is_empty() {
                println!("AGENTS.md свежий, нарушений нет");
            }
            let mut failed = false;
            for i in &issues {
                println!(
                    "[{}] {}:{} {} — {}",
                    i.severity,
                    i.file.display(),
                    i.line,
                    i.rule,
                    i.message
                );
                failed |= i.severity == "error";
            }
            if failed {
                std::process::exit(1);
            }
        }
        AgentsMdCmd::LintAll { registry } => {
            let registry = registry.unwrap_or_else(|| Config::home_dir().join("repos.txt"));
            let results = arch_harness::agentsmd::lint_registry(&registry)?;
            let mut failed = false;
            for (repo, issues) in &results {
                let errors = issues.iter().filter(|i| i.severity == "error").count();
                let status = if issues.is_empty() {
                    "OK".to_string()
                } else {
                    format!("{} проблем ({} error)", issues.len(), errors)
                };
                println!("{:<50} {}", repo.display(), status);
                failed |= errors > 0;
            }
            let _ = cfg;
            if failed {
                std::process::exit(1);
            }
        }
    }
    Ok(())
}
