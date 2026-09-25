//! Подкоманды `arch-be rubric` и их обработчик: рубрики архитектурного
//! контроля с LLM-судьёй (B1: выделено из `main.rs`).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use clap::Subcommand;

use arch_harness::config::Config;
use arch_harness::llm::LlmRegistry;

use super::resolve_asset;

#[derive(Subcommand)]
pub(crate) enum RubricCmd {
    /// Список якорных рубрик.
    List,
    /// Оценить файл по рубрике (LLM-судья).
    Run {
        /// Рубрика (имя файла в assets/rubrics или путь).
        rubric: String,
        /// Целевой документ (md/txt); с `--all-accepted` не нужен, для
        /// смысловой рубрики задаётся `--pack`/`--subject`.
        target: Option<PathBuf>,
        /// Вид досье смысловой рубрики (ADR-051): `adr_vs_spine` |
        /// `entity_links` | `nfr_mechanism` | `code_vs_spine`.
        #[arg(long)]
        pack: Option<String>,
        /// Субъект досье: путь к ADR или файлу кода либо идентификатор
        /// сущности модели — вместе с `--pack`.
        #[arg(long)]
        subject: Option<String>,
        /// Корень репозитория для сборки досье (по умолчанию — текущий каталог).
        #[arg(long)]
        root: Option<PathBuf>,
        /// Модель-судья.
        #[arg(long)]
        model: Option<String>,
        /// Сначала сгенерировать динамическую рубрику под предмет.
        #[arg(long)]
        dynamic_subject: Option<String>,
        /// Модель-автор документа: `judge == author` — судья судил свою же
        /// работу (метка `judge_is_author` в составляющей `decision_quality`).
        /// Если в шапке документа есть `Модель-автор`, значение из шапки
        /// сильнее аргумента (ADR-048).
        #[arg(long)]
        author_model: Option<String>,
        /// Сэмплов судьи на критерий (по умолчанию — из секции `[judge]`).
        #[arg(long)]
        samples: Option<usize>,
        /// Оценить все принятые ADR кейса без свежего отчёта — одна команда
        /// закрывает `rubric_report_missing` и `rubric_report_stale` (ADR-048).
        #[arg(long)]
        all_accepted: bool,
        /// Кейс для `--all-accepted` (по умолчанию — рабочий каталог).
        #[arg(long)]
        dir: Option<PathBuf>,
    },
    /// Что осталось оценить и чем: перечень принятых ADR без свежего отчёта
    /// плюс два готовых текста передачи судейства. Ничего не пишет (ADR-048).
    Handover {
        /// Кейс (по умолчанию — рабочий каталог).
        #[arg(long)]
        dir: Option<PathBuf>,
        /// Рубрика оценки решений.
        #[arg(long, default_value = "adr_quality")]
        rubric: String,
    },
    /// Приёмка судейства: по каждому принятому ADR — свежий ли отчёт, кто
    /// судил, какой уровень независимости, сходятся ли ответы. Ненулевой код,
    /// если гейт по `decision_quality` всё ещё красный (ADR-048).
    Accept {
        /// Кейс (по умолчанию — рабочий каталог).
        #[arg(long)]
        dir: Option<PathBuf>,
    },
    /// Пересобрать отчёт из сохранённых сырых ответов судьи и сверить с
    /// записанным: расхождение — ненулевой код (ADR-048).
    ///
    /// Отвечает на один вопрос: соответствует ли записанный балл ответам, из
    /// которых он объявлен собранным. Отчёт без сохранённых ответов
    /// (до 0.3.5) сверке не подлежит — это не расхождение, а отсутствие
    /// свидетельства.
    Reverify {
        /// Отчёт (`reports/rubric/<slug>.json`) или каталог с отчётами.
        path: PathBuf,
    },
    /// Записать решение архитектора по решению `human` (E4.5): принято или
    /// отклонено, кем и почему. Запись привязывается к хэшу файла отчёта и
    /// кладётся рядом с пакетом человека; коммитится подписанным коммитом.
    /// Гейт читает её и снимает эскалацию при `accept`.
    Decide {
        /// Отчёт (`reports/rubric/<slug>.json`).
        report: PathBuf,
        /// Принять суждение судьи (снять эскалацию `human`).
        #[arg(long)]
        accept: bool,
        /// Отклонить суждение судьи (эскалация остаётся).
        #[arg(long)]
        reject: bool,
        /// Кто решил: имя и, при желании, адрес (`Иван Петров <ivan@bank>`).
        #[arg(long)]
        by: String,
        /// Обоснование решения.
        #[arg(long, default_value = "")]
        reason: String,
    },
    /// Собрать досье судьи (вход смысловой рубрики) и напечатать его с хэшем.
    Pack {
        /// Вид досье: `adr_vs_spine` | `entity_links` | `nfr_mechanism` |
        /// `code_vs_spine`.
        kind: String,
        /// Субъект: путь к ADR/файлу кода либо идентификатор сущности модели.
        subject: String,
        /// Корень репозитория (по умолчанию — текущий каталог).
        #[arg(long)]
        root: Option<PathBuf>,
    },
}

/// Печатает решение рубрики и, если оно `human`, кладёт пакет для архитектора
/// (E4.1/E4.4). Возвращает код выхода решения: 0 — pass, 1 — fail, 2 — human.
fn emit_decision(
    repo: &Path,
    slug: &str,
    subject: Option<&str>,
    rubric_name: &str,
    report: &arch_harness::rubric::RubricReport,
    artifact: Option<&Path>,
) -> i32 {
    let Some(decision) = report.decision else {
        return 0;
    };
    println!(
        "Решение рубрики: {} ({}) — код выхода {}",
        decision.label_ru(),
        decision.as_str(),
        decision.exit_code()
    );
    for reason in &report.decision_reasons {
        println!("  · {reason}");
    }
    if decision == arch_harness::rubric::RubricDecision::Human {
        let raw = arch_harness::judge::raw_dir(repo, slug);
        let body = arch_harness::rubric::human_package(
            rubric_name,
            subject,
            report,
            &report.decision_reasons,
            Some(&raw),
            artifact,
        );
        match arch_harness::rubric::write_human_package(repo, slug, &body) {
            Ok(path) => println!("Пакет архитектору: {}", path.display()),
            Err(e) => eprintln!("⚠ пакет архитектору не записан: {e}"),
        }
    }
    decision.exit_code()
}

pub(crate) async fn cmd_rubric(cfg: &Arc<Config>, cmd: RubricCmd) -> Result<()> {
    match cmd {
        RubricCmd::List => {
            let list = arch_harness::rubric::list(&cfg.paths.rubrics_dir())?;
            for r in &list {
                println!(
                    "  {:<32} {} ({} критериев)",
                    r.name, r.description, r.criteria_count
                );
            }
        }
        RubricCmd::Run {
            rubric,
            target,
            pack,
            subject,
            root,
            model,
            dynamic_subject,
            author_model,
            samples,
            all_accepted,
            dir,
        } => {
            let registry = Arc::new(LlmRegistry::from_config(cfg)?);
            let judge = match &model {
                Some(name) => registry.get(name)?,
                None => registry.default(),
            };
            let rub = if let Some(subject) = dynamic_subject {
                let anchor_path = resolve_asset(&cfg.paths.rubrics_dir(), &rubric, "yaml");
                let anchor = arch_harness::rubric::load(&anchor_path).ok();
                arch_harness::rubric::generate_dynamic(&subject, anchor.as_ref(), judge.as_ref())
                    .await?
            } else {
                let path = resolve_asset(&cfg.paths.rubrics_dir(), &rubric, "yaml");
                arch_harness::rubric::load(&path)?
            };
            // Досье (ADR-051): субъект и вид заданы — собираем из репозитория
            // и судим с проверкой цитат по ролям; иначе обычный документ.
            let dossier = match (&pack, &subject) {
                (Some(kind), Some(subject)) => {
                    let repo = root.clone().unwrap_or_else(|| PathBuf::from("."));
                    let repo = repo.canonicalize().unwrap_or(repo);
                    let kind = arch_harness::rubric_pack::PackKind::parse(kind)?;
                    let packs = arch_harness::rubric_pack::build(&repo, kind, subject)?;
                    if packs.len() > 1 {
                        anyhow::bail!(
                            "досье дробится на {} фрагментов — вызывайте по каждому, \
                             указав субъект с диапазоном строк (например, '{subject}#1-40')",
                            packs.len()
                        );
                    }
                    Some(packs.into_iter().next().expect("один фрагмент"))
                }
                (None, None) => None,
                _ => {
                    anyhow::bail!("`--pack` и `--subject` задаются вместе: вид досье и его субъект")
                }
            };
            if dossier.is_some() && target.is_some() {
                anyhow::bail!(
                    "`--pack`/`--subject` несовместимы с целевым документом: досье \
                     собирается из репозитория"
                );
            }
            if dossier.is_some() && all_accepted {
                anyhow::bail!(
                    "`--pack`/`--subject` несовместимы с `--all-accepted`: досье \
                     собирается по одному субъекту"
                );
            }
            // E4.1: худшее решение прогона определяет код выхода (0/1/2).
            let mut exit_code = 0i32;
            // `--samples` перекрывает секцию [judge] для этого прогона — и для
            // документа, и для досье: судят по одним правилам (J8, ADR-048).
            let mut judge_cfg = cfg.judge.clone();
            if let Some(samples) = samples {
                judge_cfg.samples = samples;
            }
            if let Some(pack) = &dossier {
                let (report, raw) = arch_harness::rubric::evaluate_pack_collecting(
                    &rub,
                    pack,
                    judge.as_ref(),
                    &judge_cfg,
                )
                .await?;
                println!("{}", report.to_markdown());
                let out =
                    cfg.paths
                        .reports_dir
                        .join(format!("rubric-{}-{}.md", rub.name, timestamp()));
                if let Some(parent) = out.parent() {
                    std::fs::create_dir_all(parent).ok();
                }
                std::fs::write(&out, report.to_markdown())?;
                eprintln!("Отчёт: {}", out.display());
                // Машиночитаемый отчёт о досье (ADR-051): его читает
                // составляющая `semantic_quality`. Репозиторий — тот, из
                // которого собрано досье, иначе гейт его не найдёт.
                let repo = root
                    .clone()
                    .unwrap_or_else(|| PathBuf::from("."))
                    .canonicalize()
                    .unwrap_or_else(|_| PathBuf::from("."));
                // Происхождение и сырые ответы — по тому же канону, что у
                // документа (F1): иначе `rubric reverify` называет отчёт по
                // досье невоспроизводимым (J1/J2, ADR-048). Автора досье
                // решает аргумент: шапки у собранного досье нет (как в MCP
                // `rubric_verify`).
                let choice = arch_harness::judge::choose_author(None, author_model.clone());
                let model_name = model.clone().unwrap_or_else(|| cfg.default_model.clone());
                let mut provenance = arch_harness::judge::RubricProvenance::launched(
                    arch_harness::judge::launcher_for(cfg, &model_name),
                );
                if cfg.judge.record_operator {
                    provenance.operator = arch_harness::judge::operator(&repo);
                }
                let extras = arch_harness::rubric::ArtifactExtras {
                    provenance: Some(provenance),
                    author_source: Some(choice.source.clone()),
                    author_model_declared: choice.declared.clone(),
                    families: cfg.judge.families.clone(),
                    judge_config: Some(arch_harness::rubric::JudgeConfigSnapshot {
                        samples: judge_cfg.samples.max(1),
                        unstable_stdev: judge_cfg.unstable_stdev,
                        evidence_min_similarity: judge_cfg.evidence_min_similarity,
                    }),
                    // Сырые ответы судьи — рядом с отчётом: отчёт обязан
                    // пересобираться из них (J2, ADR-048).
                    raw_answers: raw
                        .into_iter()
                        .map(|text| arch_harness::judge::RawAnswerInput {
                            text,
                            dropped: false,
                        })
                        .collect(),
                };
                let artifact = match arch_harness::rubric::write_artifact_for_subject_with(
                    &repo,
                    &report,
                    &arch_harness::rubric::ArtifactSubject::Pack(pack),
                    choice.author.as_deref(),
                    &extras,
                ) {
                    Ok(path) => {
                        eprintln!("Отчёт для гейта: {}", path.display());
                        Some(path)
                    }
                    Err(e) => {
                        eprintln!("⚠ машиночитаемый отчёт не записан: {e}");
                        None
                    }
                };
                exit_code = exit_code.max(emit_decision(
                    &repo,
                    &arch_harness::rubric::pack_artifact_slug(pack.kind.as_str(), &pack.subject),
                    Some(&pack.subject),
                    &rub.name,
                    &report,
                    artifact.as_deref(),
                ));
            } else {
                // Цели прогона: один документ либо все принятые ADR без свежего
                // отчёта (J9, ADR-048).
                let targets: Vec<PathBuf> = if all_accepted {
                    if target.is_some() {
                        anyhow::bail!("`--all-accepted` не совместим с целевым документом");
                    }
                    let case = match &dir {
                        Some(d) => d.clone(),
                        None => std::env::current_dir().context("cwd")?,
                    };
                    let work = arch_harness::judge::handover(&case, &rubric, cfg)?;
                    let files: Vec<PathBuf> = work
                        .pending()
                        .iter()
                        .map(|item| case.join(&item.path))
                        .collect();
                    if files.is_empty() {
                        eprintln!(
                            "Оценивать нечего: у всех принятых ADR кейса {} свежий отчёт",
                            case.display()
                        );
                    } else {
                        eprintln!("К оценке: {} документ(ов)", files.len());
                    }
                    files
                } else {
                    vec![
                        target
                            .clone()
                            .context("укажите целевой документ, `--all-accepted` или `--pack`")?,
                    ]
                };
                if targets.is_empty() {
                    return Ok(());
                }
                for target in targets {
                    let text = std::fs::read_to_string(&target)
                        .with_context(|| format!("чтение {}", target.display()))?;
                    let (report, raw) = arch_harness::rubric::evaluate_collecting(
                        &rub,
                        &text,
                        judge.as_ref(),
                        &judge_cfg,
                    )
                    .await?;
                    println!("{}", report.to_markdown());
                    let out = cfg.paths.reports_dir.join(format!(
                        "rubric-{}-{}.md",
                        rub.name,
                        timestamp()
                    ));
                    if let Some(parent) = out.parent() {
                        std::fs::create_dir_all(parent).ok();
                    }
                    std::fs::write(&out, report.to_markdown())?;
                    eprintln!("Отчёт: {}", out.display());
                    // Машиночитаемый отчёт (Н7, ADR-042): его читает составляющая
                    // гейта `decision_quality`. Пишем в репозиторий, к которому
                    // относится документ, — иначе гейт его не найдёт.
                    let abs_target = target.canonicalize().unwrap_or_else(|_| target.clone());
                    let repo = arch_harness::rubric::repo_root_of(&abs_target);
                    // Автор документа — из шапки ADR, если он там записан: значение из
                    // документа сильнее аргумента вызова (J3, ADR-048).
                    let choice = arch_harness::judge::choose_author(
                        arch_harness::adr_registry::author_model_of(&abs_target),
                        author_model.clone(),
                    );
                    // Происхождение `launched`: судью запустил Spine — известны
                    // команда и аргументы запуска (или имя API-модели), неизвестна
                    // отвечавшая модель (ADR-048).
                    let model_name = model.clone().unwrap_or_else(|| cfg.default_model.clone());
                    let mut provenance = arch_harness::judge::RubricProvenance::launched(
                        arch_harness::judge::launcher_for(cfg, &model_name),
                    );
                    if cfg.judge.record_operator {
                        provenance.operator = arch_harness::judge::operator(&repo);
                    }
                    let extras = arch_harness::rubric::ArtifactExtras {
                        provenance: Some(provenance),
                        author_source: Some(choice.source.clone()),
                        author_model_declared: choice.declared.clone(),
                        families: cfg.judge.families.clone(),
                        judge_config: Some(arch_harness::rubric::JudgeConfigSnapshot {
                            samples: judge_cfg.samples.max(1),
                            unstable_stdev: judge_cfg.unstable_stdev,
                            evidence_min_similarity: judge_cfg.evidence_min_similarity,
                        }),
                        // Сырые ответы судьи — рядом с отчётом: отчёт обязан
                        // пересобираться из них (J2, ADR-048).
                        raw_answers: raw
                            .into_iter()
                            .map(|text| arch_harness::judge::RawAnswerInput {
                                text,
                                dropped: false,
                            })
                            .collect(),
                    };
                    let artifact = match arch_harness::rubric::write_artifact_with(
                        &repo,
                        &report,
                        Some(&abs_target),
                        choice.author.as_deref(),
                        &extras,
                    ) {
                        Ok(path) => {
                            eprintln!("Отчёт для гейта: {}", path.display());
                            Some(path)
                        }
                        Err(e) => {
                            eprintln!("⚠ машиночитаемый отчёт не записан: {e}");
                            None
                        }
                    };
                    exit_code = exit_code.max(emit_decision(
                        &repo,
                        &arch_harness::rubric::artifact_slug(Some(&abs_target)),
                        None,
                        &rub.name,
                        &report,
                        artifact.as_deref(),
                    ));
                }
            }
            // E4.1: решение рубрики — код выхода 0 (годно) / 1 (нарушение) /
            // 2 (нужен человек). До 0.3.9 `rubric run` всегда возвращал 0.
            if exit_code != 0 {
                std::process::exit(exit_code);
            }
        }
        RubricCmd::Handover { dir, rubric } => {
            let case = match dir {
                Some(d) => d,
                None => std::env::current_dir().context("cwd")?,
            };
            let work = arch_harness::judge::handover(&case, &rubric, cfg)?;
            let (spine, second) = arch_harness::judge::handover_texts(&work);
            println!(
                "Кейс {} · рубрика «{}»: принятых ADR {}, без свежего отчёта {}",
                work.case.display(),
                work.rubric,
                work.items.len(),
                work.pending().len()
            );
            for item in &work.items {
                println!(
                    "  [{:7}] {} — автор: {}, судья отчёта: {}, независимость: {}",
                    item.state,
                    item.path,
                    item.author_model.as_deref().unwrap_or("не указана"),
                    item.report_judge.as_deref().unwrap_or("—"),
                    item.independence.as_deref().unwrap_or("—")
                );
            }
            println!("\nСудьи CLI в [models]:");
            if work.judges.is_empty() {
                println!("  нет: добавьте модель с kind = \"cli\" — ключ для неё не нужен");
            } else {
                for j in &work.judges {
                    println!(
                        "  {} (команда {}, семейство {})",
                        j.name,
                        j.command.as_deref().unwrap_or("—"),
                        j.family
                    );
                }
            }
            println!(
                "\nСпособ 1 — судью запускает Spine (уровень «обеспечена запуском»):\n{spine}"
            );
            println!("\nСпособ 2 — судит второй харнесс (уровень «заявлена»):\n{second}");
        }
        RubricCmd::Accept { dir } => {
            let case = match dir {
                Some(d) => d,
                None => std::env::current_dir().context("cwd")?,
            };
            let report = arch_harness::judge::accept(&case, cfg)?;
            for item in &report.items {
                println!(
                    "  [{:7}] {} — судья: {}, независимость: {}, ответы сходятся: {}",
                    item.state,
                    item.path,
                    item.report_judge.as_deref().unwrap_or("—"),
                    item.independence.as_deref().unwrap_or("—"),
                    match item.reproduced {
                        Some(true) => "да",
                        Some(false) => "НЕТ",
                        None => "ответы не сохранены",
                    }
                );
            }
            println!("{}", report.summary());
            if report.gate_red {
                std::process::exit(1);
            }
        }
        RubricCmd::Reverify { path } => {
            let reports = collect_artifacts(&path)?;
            if reports.is_empty() {
                anyhow::bail!(
                    "отчётов рубрики не найдено: {} (ожидается reports/rubric/<slug>.json или каталог)",
                    path.display()
                );
            }
            let rubrics_dir = cfg.paths.rubrics_dir();
            let mut bad = 0usize;
            for (artifact_path, artifact) in &reports {
                let repo = arch_harness::rubric::repo_root_of(artifact_path);
                let check =
                    arch_harness::judge::reverify(&repo, artifact, &rubrics_dir, &cfg.judge);
                let name = artifact_path.file_name().map_or_else(
                    || artifact_path.display().to_string(),
                    |n| n.to_string_lossy().into_owned(),
                );
                if !check.raw_saved {
                    println!("~ {name}: сырые ответы не сохранены — отчёт невоспроизводим");
                    continue;
                }
                // Порядок силы: правка сохранённого ответа — прямое свидетельство
                // подмены следа; устаревание досье называется, только если след цел.
                if !check.tampered.is_empty() {
                    for file in &check.tampered {
                        println!("✗ {name}: {file} — текст ответа не сходится с записанным хэшем");
                    }
                    bad += 1;
                    println!("✗ {name}: отчёт собран из подменённых ответов");
                    continue;
                }
                if let Some(reason) = &check.stale {
                    println!("✗ {name}: досье устарело — {reason}");
                    bad += 1;
                    continue;
                }
                if let Some(reason) = &check.unavailable {
                    println!("~ {name}: сверка невозможна — {reason}");
                    continue;
                }
                for diff in &check.differences {
                    println!("✗ {name}: {diff}");
                }
                if check.reproduced() {
                    println!("✓ {name}: отчёт воспроизводится из своих ответов");
                } else {
                    bad += 1;
                    println!("✗ {name}: отчёт не соответствует своим ответам");
                }
            }
            if bad > 0 {
                anyhow::bail!("отчётов с расхождением: {bad}");
            }
        }
        RubricCmd::Decide {
            report,
            accept,
            reject,
            by,
            reason,
        } => {
            if accept == reject {
                anyhow::bail!("укажите ровно одно: `--accept` или `--reject`");
            }
            let path = report.canonicalize().unwrap_or_else(|_| report.clone());
            let text = std::fs::read_to_string(&path)
                .with_context(|| format!("чтение отчёта {}", path.display()))?;
            let artifact: arch_harness::rubric::RubricArtifact =
                serde_json::from_str(&text).context("разбор отчёта рубрики")?;
            let repo = arch_harness::rubric::repo_root_of(&path);
            let slug = path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .context("имя файла отчёта")?;
            let verdict = if accept {
                arch_harness::rubric::HumanVerdict::Accept
            } else {
                arch_harness::rubric::HumanVerdict::Reject
            };
            let record = arch_harness::rubric::HumanDecision::new(
                &artifact,
                &arch_harness::rubric::report_rel_path(&repo, &path),
                &arch_harness::hash::sha256_hex(text.as_bytes()),
                verdict,
                &by,
                &reason,
            );
            let written = arch_harness::rubric::write_decision(&repo, &slug, &record)?;
            println!(
                "Решение архитектора: {} · {} — {}",
                artifact.rubric,
                artifact
                    .subject
                    .as_deref()
                    .or(artifact.target.as_deref())
                    .unwrap_or("(без субъекта)"),
                verdict.as_str()
            );
            println!("Запись решения: {}", written.display());
            println!(
                "Закоммитьте её подписанным коммитом, чтобы решение попало в аудиторский след:\n  \
                 git add {rel}\n  git commit -S -m \"decision({rubric}): {verdict}\" --trailer \
                 \"Signed-off-by: {by}\"",
                rel = arch_harness::rubric::report_rel_path(&repo, &written),
                rubric = artifact.rubric,
                verdict = verdict.as_str(),
                by = by,
            );
        }
        RubricCmd::Pack {
            kind,
            subject,
            root,
        } => {
            let repo = root.unwrap_or_else(|| PathBuf::from("."));
            let repo = repo.canonicalize().unwrap_or(repo);
            let kind = arch_harness::rubric_pack::PackKind::parse(&kind)?;
            let packs = arch_harness::rubric_pack::build(&repo, kind, &subject)?;
            for p in &packs {
                println!("{}", p.text);
                println!();
                println!(
                    "досье '{}' · субъект '{}' · sha256:{} · источников {} (ссылочных {})",
                    p.kind.as_str(),
                    p.subject,
                    p.sha256,
                    p.inputs.len(),
                    p.references().len()
                );
                for i in &p.inputs {
                    println!(
                        "  [{}] {} {}",
                        i.role.as_str(),
                        i.path,
                        i.id.as_deref().unwrap_or("-")
                    );
                }
            }
            eprintln!(
                "Досье собрано: {} (субъект '{}', вид '{}')",
                packs.len(),
                subject,
                kind.as_str()
            );
        }
    }
    Ok(())
}

/// Отчёты рубрики для `rubric reverify`: один файл или все `*.json` каталога
/// (подкаталог сырых ответов `raw/` не читается).
fn collect_artifacts(path: &Path) -> Result<Vec<(PathBuf, arch_harness::rubric::RubricArtifact)>> {
    let mut out = Vec::new();
    let files: Vec<PathBuf> = if path.is_dir() {
        let mut files: Vec<PathBuf> = std::fs::read_dir(path)
            .with_context(|| format!("чтение каталога {}", path.display()))?
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.is_file()
                    && p.extension()
                        .is_some_and(|x| x.eq_ignore_ascii_case("json"))
            })
            .collect();
        files.sort();
        files
    } else {
        vec![path.to_path_buf()]
    };
    for file in files {
        let text =
            std::fs::read_to_string(&file).with_context(|| format!("чтение {}", file.display()))?;
        match serde_json::from_str::<arch_harness::rubric::RubricArtifact>(&text) {
            Ok(artifact) => out.push((file, artifact)),
            Err(e) => eprintln!("⚠ {}: не отчёт рубрики ({e})", file.display()),
        }
    }
    Ok(out)
}

/// Метка времени для имён отчётов.
fn timestamp() -> String {
    chrono::Local::now().format("%Y%m%d-%H%M%S").to_string()
}
