//! Подкоманды `arch-be model`, `arch-be trace`, `arch-be nfr` и их
//! обработчики: типизированная модель архитектуры и проверки над ней
//! (B1: выделено из `main.rs`).

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Subcommand;

use arch_harness::config::Config;

/// Подкоманды `arch-be model` (ADR-003).
#[derive(Subcommand)]
pub(crate) enum ModelCmd {
    /// Ссылочная целостность модели: битая ссылка/дубль ID/цикл `depends_on` —
    /// error (exit code 1, как у `control check`); ADR без CMP, NFR без
    /// способа проверки — warn.
    Validate {
        /// Каталог модели.
        dir: PathBuf,
    },
    /// Карточка сущности: шапка, связи, обратные ссылки, тело.
    Show {
        /// ID сущности (ADR-001, CMP-002, …).
        id: String,
        /// Каталог модели (по умолчанию ./model).
        #[arg(long, default_value = "model")]
        dir: PathBuf,
    },
    /// Граф связей модели.
    Graph {
        /// Каталог модели (по умолчанию ./model).
        #[arg(long, default_value = "model")]
        dir: PathBuf,
        /// Формат: text (список) или mermaid (flowchart, совместим с `arch-be mermaid`).
        #[arg(long, default_value = "text")]
        format: String,
    },
    /// Проекция: рендер ADR-файлов из модели в <кейс>/.arch-handoff/adr/
    /// (зеркально; устаревшие ADR-*.md удаляются).
    Project {
        /// Каталог модели.
        dir: PathBuf,
    },
    /// Экспорт модели в отраслевой формат (ADR-009, ADR-032): Structurizr
    /// DSL, `PlantUML`, drawio (SYS/CMP/INT + связи) или `ArchiMate` Open
    /// Exchange 3.2 (SYS/CMP/INT/CAP/REQ/NFR/AD + связи) — на stdout.
    Export {
        /// Каталог модели.
        dir: PathBuf,
        /// Формат: structurizr, plantuml, drawio или archimate.
        #[arg(long)]
        format: String,
    },
    /// Импорт внешнего реестра/описания в модель: Structurizr DSL
    /// (SYS/CMP/INT + связи) либо реестр систем (csv/xlsx/backstage →
    /// `SYS-*` + `OWNER-*`; по одному .md на сущность; существующие —
    /// skip, перезапись — только `--force`, и на месте их файлов).
    Import {
        /// Файл-источник (Structurizr DSL, CSV, xlsx, catalog-info.yaml).
        file: PathBuf,
        /// Формат: structurizr, csv, xlsx, backstage.
        #[arg(long)]
        format: String,
        /// Каталог модели-получателя (создаётся при отсутствии).
        #[arg(long = "out", visible_alias = "dir", default_value = "model")]
        dir: PathBuf,
        /// Перезаписывать существующие сущности (только csv/xlsx/backstage).
        #[arg(long)]
        force: bool,
        /// Только план: разбор и отчёт без записи файлов
        /// (только csv/xlsx/backstage).
        #[arg(long)]
        dry_run: bool,
    },
    /// Дрейф «модель ↔ код» (read-only): CMP с несуществующими `code_roots`
    /// — error (exit code 1); каталог с манифестом сборки без покрывающего
    /// CMP — warn; звено `INT → контракт` в семантике `trace check`
    /// (ADR-035: битый путь `contract` — error, поле не задано — warn).
    Drift {
        /// Корень кейса (каталог с model/ внутри).
        dir: PathBuf,
        /// JSON-вердикт `{passed, issues, summary}` вместо текста.
        #[arg(long)]
        json: bool,
    },
    /// Радиус взрыва изменения (бэклог волны 3, п.13): от сущности (`--id`)
    /// или файлов (`--paths` → CMP по `code_roots`, ADR-030) транзитивный
    /// обход графа связей модели → затронутые сущности по типам, правила
    /// `CONSTRAINTS.yaml` (C-NNN с владельцами), контракты INT, владельцы
    /// OWNER — «что я задену и с кем согласовывать». Отчёт, не гейт.
    Impact {
        /// Корень кейса (каталог с model/).
        dir: PathBuf,
        /// ID сущности-источника (CMP-001, INT-002, …).
        #[arg(long)]
        id: Option<String>,
        /// Файл изменения (повторяемый флаг). Источники — CMP, чьи
        /// `code_roots` покрывают путь; непокрытые пути — в отчёте как gap.
        #[arg(long)]
        paths: Vec<String>,
        /// Машиночитаемый вывод: JSON-отчёт.
        #[arg(long)]
        json: bool,
    },
    /// Ландшафт систем набора проектов (EA-3, ADR-036/ADR-037): агрегация
    /// `model/` самого ROOT и непосредственных подкаталогов в единый
    /// реестр систем SYS/INT с дедупликацией по имени (без глобальных ID),
    /// находки (id-divergence, status-conflict, dangling-ref,
    /// cross-project-link) и топ связности.
    Landscape {
        /// Корневой каталог набора проектов.
        root: PathBuf,
        /// Дополнительно вывести mermaid `graph TD` ландшафта.
        #[arg(long)]
        mermaid: bool,
        /// Карта алиасов (yaml/json «вариант имени → каноничное имя»):
        /// дедупликация учитывает алиасы.
        #[arg(long)]
        aliases: Option<PathBuf>,
        /// Дифф ландшафта против версии в git: ссылка (ветка/тег/sha) или
        /// дата YYYY-MM-DD (последний коммит не позже конца дня).
        #[arg(long)]
        diff_since: Option<String>,
    },
}

/// Подкоманды `arch-be trace` (ADR-006).
#[derive(Subcommand)]
pub(crate) enum TraceCmd {
    /// Позвенная трассируемость: REQ → NFR → AD/ADR → CMP → правило
    /// `CONSTRAINTS.yaml`; AD без правила и без `unverifiable` — error
    /// (exit code 1). Отчёт markdown, пригоден для evidence bundle.
    Check {
        /// Корень кейса (каталог с model/).
        dir: PathBuf,
        /// Формат вывода: text (дефолт — markdown-отчёт звеньев) | sarif |
        /// junit | gitlab-codequality | markdown (нормализованная таблица
        /// находок, `src/report_fmt.rs`; машинные — в stdout).
        #[arg(long, default_value = "text", value_name = "FORMAT")]
        format: String,
    },
}

/// Подкоманды `arch-be nfr` (ADR-007).
#[derive(Subcommand)]
pub(crate) enum NfrCmd {
    /// Latency-бюджет: сумма бюджетов hop'ов INT-* против цели p99 из NFR-*;
    /// hop без бюджета или превышение — error (exit code 1).
    Budget {
        /// Корень кейса (каталог с model/).
        dir: PathBuf,
    },
    /// Доступность: композиция последовательных/параллельных участков против
    /// SLA из NFR-* + цели RTO/RPO; ниже SLA — error (exit code 1).
    Availability {
        /// Корень кейса (каталог с model/).
        dir: PathBuf,
    },
    /// Пропускная способность: RPS-цель против ёмкости компонентов
    /// (instances × `rps_per_instance`); дефицит — error (exit code 1).
    Capacity {
        /// Корень кейса (каталог с model/).
        dir: PathBuf,
    },
    /// Стоимость: TCO (инстансы × тариф) и цена выхода (Σ `exit_cost`)
    /// по тарифным данным сущностей.
    Cost {
        /// Корень кейса (каталог с model/).
        dir: PathBuf,
    },
}

/// `arch-be model`: типизированная модель архитектуры (ADR-003).
///
/// Читающие команды (validate/show/graph) грузят модель толерантно (E3):
/// битая сущность не обнуляет весь модельный контроль — validate отчитывает
/// её error-находкой, show/graph работают по валидному подмножеству с
/// warn-пометкой. Пишущие/обменные (project/export/import) — строгие:
/// частичная модель молча потеряла бы сущности в артефактах.
pub(crate) fn cmd_model(cmd: ModelCmd) -> Result<()> {
    match cmd {
        ModelCmd::Validate { dir } => {
            // Аргумент принимает и корень кейса, и каталог `model/` (T-13).
            let dir = arch_harness::model::model_dir_from(&dir);
            let model = arch_harness::model::load_model_tolerant(&dir)
                .with_context(|| format!("загрузка модели {}", dir.display()))?;
            let report = arch_harness::model::validate(&model);
            for i in &report.issues {
                println!(
                    "[{}] {}: {} — {}",
                    i.severity,
                    i.file.display(),
                    i.rule,
                    i.message
                );
            }
            println!("{}", report.summary());
            println!(
                "Итог: {}",
                if report.has_errors() { "FAIL" } else { "PASS" }
            );
            if report.has_errors() {
                std::process::exit(1);
            }
        }
        ModelCmd::Show { id, dir } => {
            let model = arch_harness::model::load_model_tolerant(&dir)
                .with_context(|| format!("загрузка модели {}", dir.display()))?;
            if let Some(note) = arch_harness::model::load_issues_note(&model.load_issues) {
                println!("Внимание: {note}");
            }
            let entity = model
                .get(&id)
                .with_context(|| format!("сущность '{id}' не найдена в {}", dir.display()))?;
            print!("{}", arch_harness::model::card(&model, entity));
        }
        ModelCmd::Graph { dir, format } => {
            let dir = arch_harness::model::model_dir_from(&dir);
            let model = arch_harness::model::load_model_tolerant(&dir)
                .with_context(|| format!("загрузка модели {}", dir.display()))?;
            if let Some(note) = arch_harness::model::load_issues_note(&model.load_issues) {
                println!("Внимание: {note}");
            }
            match format.as_str() {
                "text" => print!("{}", arch_harness::model::graph_text(&model)),
                "mermaid" => print!("{}", arch_harness::model::graph_mermaid(&model)),
                other => anyhow::bail!("неизвестный формат '{other}' (допустимы: text, mermaid)"),
            }
        }
        ModelCmd::Project { dir } => {
            let report = arch_harness::model::project_adr(&dir)
                .with_context(|| format!("проекция модели {}", dir.display()))?;
            for f in &report.written {
                println!("записан: {}", f.display());
            }
            for f in &report.removed {
                println!("удалён (нет сущности): {}", f.display());
            }
            println!(
                "Проекция {}: {} ADR-файлов, удалено устаревших: {}",
                report.out_dir.display(),
                report.written.len(),
                report.removed.len()
            );
        }
        ModelCmd::Export { dir, format } => {
            let fmt = arch_harness::model::ExportFormat::from_name(&format).with_context(|| {
                format!(
                    "неизвестный формат '{format}' (допустимы: {})",
                    arch_harness::model::ExportFormat::names()
                )
            })?;
            let model = arch_harness::model::load_model(&dir)
                .with_context(|| format!("загрузка модели {}", dir.display()))?;
            let text = arch_harness::model::export_model(&model, fmt)
                .with_context(|| format!("экспорт модели {}", dir.display()))?;
            print!("{text}");
        }
        ModelCmd::Import {
            file,
            format,
            dir,
            force,
            dry_run,
        } => {
            if format.trim().eq_ignore_ascii_case("structurizr") {
                if force || dry_run {
                    anyhow::bail!(
                        "флаги --force/--dry-run поддерживаются для форматов csv/xlsx/backstage; \
                         импорт structurizr и так не затирает файлы (коллизия — ошибка)"
                    );
                }
                let report = arch_harness::model::import_structurizr(&file, &dir)
                    .with_context(|| format!("импорт {} в {}", file.display(), dir.display()))?;
                for f in &report.written {
                    println!("записан: {}", f.display());
                }
                for w in &report.warnings {
                    println!("предупреждение: {w}");
                }
                println!(
                    "Импорт {}: {} сущностей, предупреждений: {}",
                    report.dir.display(),
                    report.written.len(),
                    report.warnings.len()
                );
                return Ok(());
            }
            let fmt =
                arch_harness::model::RegistryFormat::from_name(&format).with_context(|| {
                    format!(
                        "неизвестный формат '{format}' (допустимы: structurizr, {})",
                        arch_harness::model::RegistryFormat::names()
                    )
                })?;
            let report = arch_harness::model::import_registry(
                &file,
                &dir,
                fmt,
                &arch_harness::model::RegistryImportOptions { force, dry_run },
            )
            .with_context(|| format!("импорт {} в {}", file.display(), dir.display()))?;
            let verb = if report.dry_run {
                "записал бы"
            } else {
                "записан"
            };
            for f in &report.written {
                println!("{verb}: {}", f.display());
            }
            for s in &report.skipped {
                println!("пропущен (уже в модели): {s}");
            }
            for w in &report.warnings {
                println!("предупреждение: {w}");
            }
            println!(
                "Импорт {}: записано: {}, пропущено: {}, предупреждений: {}{}",
                report.dir.display(),
                report.written.len(),
                report.skipped.len(),
                report.warnings.len(),
                if report.dry_run { " (dry-run)" } else { "" }
            );
        }
        ModelCmd::Impact {
            dir,
            id,
            paths,
            json,
        } => {
            let report = arch_harness::review::change_impact(&dir, id.as_deref(), &paths)
                .with_context(|| format!("радиус изменения по {}", dir.display()))?;
            if json {
                let verdict = arch_harness::review::impact_json(&report);
                println!(
                    "{}",
                    serde_json::to_string_pretty(&verdict).unwrap_or_else(|_| verdict.to_string())
                );
            } else {
                print!("{}", arch_harness::review::render_impact(&report));
            }
        }
        ModelCmd::Drift { dir, json } => {
            let dir = arch_harness::model::case_root_from(&dir);
            let report = arch_harness::model::drift_check(&dir)
                .with_context(|| format!("дрейф «модель ↔ код» кейса {}", dir.display()))?;
            if json {
                let verdict = arch_harness::model::drift::verdict_json(&report);
                println!("{verdict:#}");
            } else {
                print!("{}", arch_harness::model::drift::render_text(&report));
            }
            if report.has_errors() {
                std::process::exit(1);
            }
        }
        ModelCmd::Landscape {
            root,
            mermaid,
            aliases,
            diff_since,
        } => {
            let aliases = match aliases {
                Some(path) => arch_harness::landscape::load_aliases(&path)
                    .with_context(|| format!("карта алиасов {}", path.display()))?,
                None => std::collections::BTreeMap::new(),
            };
            let report = arch_harness::landscape::build_landscape_with_aliases(&root, &aliases)?;
            println!("{}", arch_harness::landscape::render_markdown(&report));
            if mermaid {
                println!("\n```mermaid");
                println!("{}", arch_harness::landscape::render_mermaid(&report));
                println!("```");
            }
            if let Some(since) = diff_since {
                let diff = arch_harness::landscape::diff_landscape(&root, &since, &aliases)
                    .with_context(|| format!("дифф ландшафта против {since}"))?;
                println!();
                println!(
                    "{}",
                    arch_harness::landscape::render_diff_markdown(&diff, &since)
                );
            }
        }
    }
    Ok(())
}

/// `arch-be trace`: трассируемость модели как fitness-функция (ADR-006).
pub(crate) fn cmd_trace(cfg: &Config, cmd: TraceCmd) -> Result<()> {
    match cmd {
        TraceCmd::Check { dir, format } => {
            let format = arch_harness::report_fmt::ReportFormat::parse(&format)
                .map_err(anyhow::Error::msg)?;
            // Требование исполняемой проверки инвариантов — из `[trace]` (ADR-050).
            let report = arch_harness::trace::trace_check_with(&dir, cfg.trace.executable_required)
                .with_context(|| format!("трассировка кейса {}", dir.display()))?;
            match format {
                arch_harness::report_fmt::ReportFormat::Text => {
                    print!("{}", arch_harness::trace::render_markdown(&report));
                }
                machine => {
                    print!(
                        "{}",
                        arch_harness::report_fmt::render(
                            machine,
                            &arch_harness::report_fmt::FmtReport::from_trace(&report),
                        )
                    );
                }
            }
            if report.has_errors() {
                std::process::exit(1);
            }
        }
    }
    Ok(())
}

/// `arch-be nfr`: количественные NFR поверх модели (ADR-007); error — exit code 1.
pub(crate) fn cmd_nfr(cmd: NfrCmd) -> Result<()> {
    match cmd {
        NfrCmd::Budget { dir } => {
            let report = arch_harness::nfr::budget_check(&dir)
                .with_context(|| format!("latency-бюджет кейса {}", dir.display()))?;
            print!("{}", report.render());
            if report.has_errors() {
                std::process::exit(1);
            }
        }
        NfrCmd::Availability { dir } => {
            let report = arch_harness::nfr::availability_check(&dir)
                .with_context(|| format!("расчёт доступности кейса {}", dir.display()))?;
            print!("{}", report.render());
            if report.has_errors() {
                std::process::exit(1);
            }
        }
        NfrCmd::Capacity { dir } => {
            let report = arch_harness::nfr::capacity_check(&dir)
                .with_context(|| format!("расчёт ёмкости кейса {}", dir.display()))?;
            print!("{}", report.render());
            if report.has_errors() {
                std::process::exit(1);
            }
        }
        NfrCmd::Cost { dir } => {
            let report = arch_harness::nfr::cost_check(&dir)
                .with_context(|| format!("расчёт стоимости кейса {}", dir.display()))?;
            print!("{}", report.render());
            if report.has_errors() {
                std::process::exit(1);
            }
        }
    }
    Ok(())
}
