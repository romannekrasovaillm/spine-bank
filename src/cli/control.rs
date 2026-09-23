//! Подкоманды `arch-be control` (+ `control fp`) и их обработчик:
//! fitness-контроль репозитория по `CONSTRAINTS.yaml` (B1: выделено из `main.rs`).

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Subcommand;

use super::resolve_constraints_cli;

#[derive(Subcommand)]
pub(crate) enum ControlCmd {
    /// Fitness-контроль репозитория по `CONSTRAINTS.yaml`.
    Check {
        /// Репозиторий.
        repo: PathBuf,
        /// Файл ограничений (по умолчанию <repo>/.arch-handoff/`CONSTRAINTS.yaml`).
        #[arg(long)]
        constraints: Option<PathBuf>,
        /// Машиночитаемый вывод: JSON-отчёт `FitnessReport` (SDK-контракт v1).
        #[arg(long)]
        json: bool,
        /// Baseline-файл долга (JSON), режим ratchet для brownfield
        /// (`docs/control.md`): находки из baseline — долг (гейт не ломают),
        /// ломают только НОВЫЕ нарушения и рост счётчика правила.
        #[arg(long, value_name = "PATH")]
        baseline: Option<PathBuf>,
        /// Перезаписать baseline текущим состоянием. Принимается только при
        /// неухудшении долга (ratchet); без `--baseline` путь по умолчанию —
        /// <repo>/.arch-handoff/baseline.json.
        #[arg(long)]
        baseline_update: bool,
        /// Проверять только файлы, изменённые против `GIT_REF` (`git diff
        /// --name-only GIT_REF` по рабочему дереву + untracked): файловые
        /// правила — на срезе, глобальные — SKIP с пометкой. Для быстрых
        /// прогонов (PostToolUse-хуки); полный прогон остаётся истиной гейта.
        #[arg(long, value_name = "GIT_REF")]
        changed_since: Option<String>,
        /// Формат вывода для CI: text (дефолт) | sarif | junit |
        /// gitlab-codequality | markdown (машинные — в stdout, как --json;
        /// несовместим с --json).
        #[arg(
            long,
            default_value = "text",
            value_name = "FORMAT",
            conflicts_with = "json"
        )]
        format: String,
        /// База git для сверки состава правил (П5, анти-ослабление): по
        /// умолчанию — merge-base с основной веткой, иначе HEAD. Сверка
        /// добавляет находку `rule_weakened`, если правило исчезло или
        /// ослаблено; недоступность базы честно печатается в сводке.
        #[arg(long, value_name = "GIT_REF")]
        base: Option<String>,
        /// НЕ исполнять правила `command_succeeds` (модель доверия A3,
        /// ADR-053): они уходят в пропуск `command_untrusted` — для проверки
        /// чужих репозиториев. То же делает `ARCH_NO_EXEC=1` (явное `0` —
        /// выключить); приоритет над allow-файлом `rules allow`.
        #[arg(long)]
        no_exec: bool,
    },
    /// Линтер ARCHITECTURE-SPINE.md.
    Spine {
        /// Путь к spine-файлу.
        file: PathBuf,
    },
    /// Сенсоры спецификаций (required-sections, upstream-coverage).
    Sensors {
        /// Каталог спецификаций.
        dir: PathBuf,
    },
    /// Architecture Significance Score: `--trigger new_component=true ...`
    Score {
        /// Триггеры вида имя=true/false.
        #[arg(long)]
        trigger: Vec<String>,
        /// Anti-bypass floor (ADR-034): механически вывести триггеры из
        /// git-диффа и объединить с заявленными (fail-safe — детектор только
        /// добавляет). Без значения — рабочее дерево против HEAD; со
        /// значением — `git diff GIT_REF...HEAD` (готовый диапазон `A...B`
        /// принимается как есть).
        #[arg(long, num_args = 0..=1, default_missing_value = "HEAD", value_name = "GIT_REF")]
        from_diff: Option<String>,
    },
    /// Отчёт по реестру правил `CONSTRAINTS.yaml` (сводка, таблица карточек,
    /// находки: без owner/expiry, просроченные, `exclude_glob`, git-прокси
    /// стоимости сопровождения, суммарный `effort_hours`).
    RulesReport {
        /// Репозиторий.
        repo: PathBuf,
        /// Файл ограничений (по умолчанию <repo>/.arch-handoff/`CONSTRAINTS.yaml`).
        #[arg(long)]
        constraints: Option<PathBuf>,
    },
    /// Кандидатные fitness-правила из содержательных пробелов кейса
    /// (read-only эвристики, `src/rules_suggest.rs`): EARS-критерии приёмки,
    /// численные таймауты в контрактах, декомпозиция REQ→работы, RTO/RPO без
    /// ADR, аудит операторских действий. Печать — markdown-отчёт + готовые
    /// YAML-фрагменты для `CONSTRAINTS.yaml` (взятие правила и severity —
    /// решение архитектора).
    RulesSuggest {
        /// Корень кейса (каталог с docs/, model/, .arch-handoff/).
        path: PathBuf,
    },
    /// Отчёт вверх по корпоративному контуру (наследование `extends`,
    /// `docs/corp-spine.md`): покрытие корп-правил, исходы (pass/fail/warn),
    /// overrides со статусами, просроченные правила, расхождения пинов версий.
    Report {
        /// Репозиторий.
        repo: PathBuf,
        /// Файл ограничений (по умолчанию <repo>/.arch-handoff/`CONSTRAINTS.yaml`).
        #[arg(long)]
        constraints: Option<PathBuf>,
        /// Уровень: corp (только унаследованные правила) | all (все).
        #[arg(long, default_value = "corp")]
        level: String,
        /// Машиночитаемый вывод: JSON-отчёт `ControlReport` (SDK-контракт v1).
        #[arg(long)]
        json: bool,
    },
    /// Новый ADR.
    Adr {
        /// Заголовок решения.
        title: String,
        /// Каталог ADR (по умолчанию ./docs/adr).
        #[arg(long)]
        dir: Option<PathBuf>,
    },
    /// Гейт контрольной точки (пока A4 — conformance evidence: репетиция
    /// отката handoff-пакета, см. docs/control.md).
    Gate {
        /// Идентификатор гейта (реализован только A4).
        gate: String,
        /// Репозиторий (с .arch-handoff/) или каталог handoff-пакета.
        packet: PathBuf,
        /// Перед оценкой гейта прогнать репетицию отката (обновляет
        /// .arch-handoff/REHEARSAL.json).
        #[arg(long)]
        rehearse: bool,
        /// Репетиция обязательна для маршрутов не ниже порога:
        /// fast|standard|critical|never (дефолт critical).
        #[arg(long, default_value = "critical")]
        require_rehearsal: String,
    },
    /// Регистр ложных срабатываний правил (FP, `docs/outcome-metrics.md` §2).
    Fp {
        #[command(subcommand)]
        cmd: FpCmd,
    },
}

/// Подкоманды `arch-be control fp` (регистр ложных срабатываний).
#[derive(Subcommand)]
pub(crate) enum FpCmd {
    /// Пометить срабатывание правила как ложное: append строки
    /// `| дата | правило | файл | примечание |` в `evidence/fp-register.md`
    /// проекта (файл создаётся с шапкой при отсутствии).
    Mark {
        /// Имя правила из CONSTRAINTS.yaml.
        rule: String,
        /// Файл срабатывания (обычно `путь:строка`).
        file: String,
        /// Примечание (причина/решение: поправить правило / записать
        /// отступление / принять).
        #[arg(long)]
        note: Option<String>,
        /// Репозиторий проекта (по умолчанию — текущий каталог).
        #[arg(long)]
        repo: Option<PathBuf>,
    },
}

pub(crate) fn cmd_control(cfg: &arch_harness::config::Config, cmd: ControlCmd) -> Result<()> {
    match cmd {
        ControlCmd::Check {
            repo,
            constraints,
            json,
            baseline,
            baseline_update,
            changed_since,
            format,
            base,
            no_exec,
        } => {
            let explicit = constraints.is_some();
            let c = resolve_constraints_cli(&repo, constraints);
            // T-01: реестра нет ни в корне, ни в `.arch-handoff/` — раньше
            // сюда улетал сырой `io: ./.arch-handoff/CONSTRAINTS.yaml: No such
            // file` (адрес, которого пользователь не выбирал). Pre-commit-хук
            // читает этот текст, поэтому причина называется прямо.
            if !explicit && !c.is_file() {
                anyhow::bail!(
                    "реестр правил не найден: ни {} в корне, ни {} — создайте каркас: `arch-be bootstrap`",
                    arch_harness::control::ROOT_CONSTRAINTS_PATH,
                    arch_harness::control::HANDOFF_CONSTRAINTS_PATH
                );
            }
            let options = arch_harness::control::baseline::CheckOptions {
                baseline,
                baseline_update,
                changed_since,
                // A3: политика исполнения команд реестра — флаг + ARCH_NO_EXEC
                // + allow-файл (CLI-край вычисляет, библиотека получает снимок).
                exec: arch_harness::cmd_trust::ExecPolicy::cli(no_exec),
            };
            // П5: сверка состава правил с git-базой — «правило выполняется»
            // плюс «правило ещё существует» в любом канале, не только в gate.
            let report =
                arch_harness::control::check_anchored(&repo, &c, &options, base.as_deref())?;
            let format = arch_harness::report_fmt::ReportFormat::parse(&format)
                .map_err(anyhow::Error::msg)?;
            if json {
                // SDK-контракт v1: машиночитаемый отчёт, exit code как в текстовом режиме.
                println!(
                    "{}",
                    serde_json::to_string(&report).expect("FitnessReport сериализуется")
                );
            } else if format != arch_harness::report_fmt::ReportFormat::Text {
                // Машинные форматы CI (SARIF/JUnit/GitLab Code Quality/markdown):
                // строго в stdout, exit-код как у текста.
                print!(
                    "{}",
                    arch_harness::report_fmt::render(
                        format,
                        &arch_harness::report_fmt::FmtReport::from_fitness(&report),
                    )
                );
            } else {
                println!("{}", report.summary);
                // Наследование корп-спайна (extends): метки источников видны
                // в выводе (docs/corp-spine.md).
                if !report.inherited.is_empty() {
                    let sources = report
                        .inherited
                        .iter()
                        .map(|s| format!("{} ({})", s.source, s.rules))
                        .collect::<Vec<_>>()
                        .join(", ");
                    println!("Источники правил: {sources}");
                }
                for o in &report.overrides {
                    println!(
                        "  [override:{}] {} (adr {}, until {}) — {}",
                        o.status, o.rule, o.adr, o.until, o.note
                    );
                }
                for i in &report.issues {
                    println!(
                        "  [{}] {}:{} {} — {}",
                        i.severity,
                        i.file.display(),
                        i.line,
                        i.rule,
                        i.message
                    );
                    // Карточный контекст правила — одной строкой-отступом и
                    // только при наличии rationale/fix_hint (не раздуваем).
                    if i.rationale.is_some() || i.fix_hint.is_some() {
                        let mut parts: Vec<String> = Vec::new();
                        if let Some(ad) = &i.ad {
                            parts.push(ad.clone());
                        }
                        if let Some(adr) = &i.adr {
                            parts.push(adr.clone());
                        }
                        if let Some(rationale) = &i.rationale {
                            parts.push(format!("зачем: {rationale}"));
                        }
                        if let Some(fix_hint) = &i.fix_hint {
                            parts.push(format!("как чинить: {fix_hint}"));
                        }
                        if let Some(skill) = &i.skill {
                            parts.push(format!("скилл: {skill}"));
                        }
                        if let Some(owner) = &i.owner {
                            parts.push(format!("владелец: {owner}"));
                        }
                        println!("      ↳ {}", parts.join(" · "));
                    }
                }
                // Режим ratchet: долг по правилам и владельцам + закрытые
                // находки (долг не попадает в список issues выше).
                if let Some(baseline_report) = &report.baseline {
                    print!(
                        "{}",
                        arch_harness::control::baseline::render_baseline_section(baseline_report)
                    );
                }
                // Режим --changed-since: размер среза и пропущенные правила.
                if let Some(reference) = &report.changed_since {
                    print!(
                        "{}",
                        arch_harness::control::baseline::render_scope_section(
                            reference,
                            report.changed_files.unwrap_or(0),
                            &report.skipped,
                        )
                    );
                }
                // Топ-5 самых медленных правил — только если есть правила > 1s.
                let mut slow: Vec<&arch_harness::control::RuleDuration> =
                    report.durations.iter().filter(|d| d.ms > 1000).collect();
                slow.sort_by(|a, b| b.ms.cmp(&a.ms).then(a.rule.cmp(&b.rule)));
                if !slow.is_empty() {
                    println!("Самые медленные правила:");
                    for d in slow.iter().take(5) {
                        println!("  {:.1}s {}", d.ms as f64 / 1000.0, d.rule);
                    }
                }
                // Пропущенные из-за отсутствия прогонщика (A2): не находки,
                // но и не проверенные правила — отдельный блок, чтобы
                // «Итог: PASS» не читался как «прогнано всё».
                if !report.runner_skipped.is_empty() {
                    println!(
                        "Пропущены правила (нет прогонщика): {}",
                        report.runner_skipped.len()
                    );
                    for s in &report.runner_skipped {
                        println!("  [skip] {} — {}", s.rule, s.reason);
                    }
                }
                // Пропущенные по модели доверия (A3): команды НЕ исполнялись —
                // тем более отдельный блок с подсказкой, как разрешить.
                if !report.untrusted_skipped.is_empty() {
                    println!(
                        "Пропущены правила ({}): {}",
                        arch_harness::cmd_trust::COMMAND_UNTRUSTED,
                        report.untrusted_skipped.len()
                    );
                    for s in &report.untrusted_skipped {
                        println!("  [skip] {} — {}", s.rule, s.reason);
                    }
                }
                println!("Итог: {}", if report.passed { "PASS" } else { "FAIL" });
            }
            if !report.passed {
                std::process::exit(1);
            }
        }
        ControlCmd::Spine { file } => {
            let issues = arch_harness::control::lint_spine(&file)?;
            if issues.is_empty() {
                println!("spine: нарушений нет");
            }
            for i in &issues {
                println!(
                    "[{}] {}:{} {} — {}",
                    i.severity,
                    i.file.display(),
                    i.line,
                    i.rule,
                    i.message
                );
            }
            // Гейт в CI: error-находки ломают сборку (warn — только отчёт).
            let errors = issues.iter().filter(|i| i.severity == "error").count();
            if !issues.is_empty() {
                println!("Итог: {} находок (error: {errors})", issues.len());
            }
            if errors > 0 {
                std::process::exit(1);
            }
        }
        ControlCmd::Sensors { dir } => {
            let results = arch_harness::control::sensors_check(&dir)?;
            // Провал сенсора — ненулевой exit (годится для CI): до волны
            // DB-гейта команда печатала [FAIL], но завершалась с кодом 0,
            // и дефект формы спецификации проходил контур незамеченным
            // (red-team кейса 011, вариант 06). PASS всех сенсоров — exit 0,
            // поведение зелёных кейсов не меняется.
            let failed = results.iter().filter(|r| !r.passed).count();
            for r in &results {
                println!(
                    "  [{}] {} {} — {}",
                    if r.passed { "PASS" } else { "FAIL" },
                    r.sensor,
                    r.file.display(),
                    r.details
                );
            }
            println!(
                "Итог: {} — сенсоров: {}, провалено: {failed}",
                if failed == 0 { "PASS" } else { "FAIL" },
                results.len()
            );
            if failed > 0 {
                std::process::exit(1);
            }
        }
        ControlCmd::Score { trigger, from_diff } => {
            // Пороги маршрутов — из конфига ([significance], ADR-034);
            // невалидные границы — понятная ошибка при чтении.
            let (fast_max, standard_max) = cfg
                .significance
                .limits()
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            let mut answers = std::collections::BTreeMap::new();
            for t in &trigger {
                let (k, v) = t
                    .split_once('=')
                    .with_context(|| format!("триггер '{t}' не вида имя=true"))?;
                answers.insert(k.to_string(), v == "true");
            }
            // T-04: незнакомое имя — ошибка, а не завышенный маршрут. Раньше
            // `--trigger foo=true` попадал в счёт наравне с каноническим.
            let unknown = arch_harness::control::unknown_trigger_names(&answers);
            if !unknown.is_empty() {
                anyhow::bail!(
                    "control score: {}",
                    arch_harness::control::unknown_triggers_error(&unknown)
                );
            }
            if let Some(git_ref) = from_diff {
                // S-1 anti-bypass: «HEAD» (дефолт флага) — рабочее дерево
                // против HEAD; иное значение — GIT_REF...HEAD.
                let git_ref = (git_ref != "HEAD").then_some(git_ref);
                // T-05: глобы детекторов — из `[significance]` конфига.
                let diff = arch_harness::control::detect_diff_triggers_with(
                    std::path::Path::new("."),
                    git_ref.as_deref(),
                    &cfg.significance.diff_globs(),
                )?;
                let scored = arch_harness::control::score_with_sources(
                    &answers,
                    &diff,
                    fast_max,
                    standard_max,
                );
                let fired = scored
                    .significance
                    .fired
                    .iter()
                    .map(|f| {
                        scored
                            .sources
                            .get(f)
                            .map_or_else(|| f.clone(), |s| format!("{f} ({})", s.label()))
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                println!(
                    "Score: {} ({fired} триггеров) → маршрут {:?}",
                    scored.significance.score, scored.significance.route
                );
                for e in &diff.evidence {
                    println!("  diff: {e}");
                }
                if !scored.undeclared.is_empty() {
                    println!(
                        "ВНИМАНИЕ — расхождение: заявлено флагами vs видно по диффу: {}",
                        scored.undeclared.join(", ")
                    );
                }
            } else {
                let s = arch_harness::control::significance_score_with_limits(
                    &answers,
                    fast_max,
                    standard_max,
                );
                println!(
                    "Score: {} ({} триггеров) → маршрут {:?}",
                    s.score,
                    s.fired.join(", "),
                    s.route
                );
            }
        }
        ControlCmd::RulesReport { repo, constraints } => {
            let resolution = arch_harness::control::resolve_constraints_path_detailed(
                &repo,
                constraints.as_deref(),
            );
            let c = resolution.as_ref().map_or_else(
                || repo.join(arch_harness::control::HANDOFF_CONSTRAINTS_PATH),
                |r| r.path.clone(),
            );
            if let Some(note) = resolution.and_then(|r| r.drift_note()) {
                println!("Внимание: {note}");
            }
            print!("{}", arch_harness::control::rules_report(&repo, &c)?);
        }
        ControlCmd::RulesSuggest { path } => {
            let report = arch_harness::rules_suggest::suggest(&path)?;
            print!("{}", arch_harness::rules_suggest::render_markdown(&report));
        }
        ControlCmd::Report {
            repo,
            constraints,
            level,
            json,
        } => {
            let c = resolve_constraints_cli(&repo, constraints);
            let report = arch_harness::control::control_report(&repo, &c, &level)?;
            if json {
                // SDK-контракт v1: машиночитаемый отчёт; report — отчётность,
                // exit code гейт не дублирует.
                println!(
                    "{}",
                    serde_json::to_string(&report).expect("ControlReport сериализуется")
                );
            } else {
                print!("{}", arch_harness::control::render_control_report(&report));
            }
        }
        ControlCmd::Adr { title, dir } => {
            let dir = dir.unwrap_or_else(|| PathBuf::from("docs/adr"));
            let path = arch_harness::control::adr_new(&dir, &title)?;
            println!("ADR создан: {}", path.display());
        }
        ControlCmd::Gate {
            gate,
            packet,
            rehearse,
            require_rehearsal,
        } => {
            use arch_harness::rehearsal as rh;
            if !gate.eq_ignore_ascii_case("a4") {
                anyhow::bail!("гейт '{gate}' не реализован механически (пока только A4)");
            }
            let requirement: rh::RehearsalRequirement = require_rehearsal
                .parse()
                .map_err(|e: String| anyhow::anyhow!("--require-rehearsal: {e}"))?;
            let (repo, packet_dir) = rh::locate_packet(&packet)?;
            // Н6: неполный пакет — находка архитектурного процесса с тем, что
            // сделать, а не io-ошибка «нет MANIFEST.json».
            if let Some(f) = rh::check_packet(&packet_dir)? {
                println!("[error] {} — {}", f.rule, f.message);
                println!("  → {}", f.fix_hint);
                println!("Итог: FAIL");
                std::process::exit(1);
            }
            let route = rh::packet_route(&packet_dir)?;
            let report = if rehearse {
                let report = rh::rehearse(&repo, &packet_dir)?;
                println!("Репетиция отката (baseline {}):", report.baseline_commit);
                for line in &report.log {
                    println!("  {line}");
                }
                println!(
                    "  evidence: {}",
                    packet_dir.join(rh::REHEARSAL_FILE).display()
                );
                Some(report)
            } else {
                rh::load_report(&packet_dir)?
            };
            // Свежесть evidence сверяем с текущим планом (если он читается).
            let plan_baseline = rh::load_plan(&packet_dir)
                .ok()
                .map(|p| p.baseline_commit.trim().to_string());
            let verdict = rh::gate_a4(
                route,
                requirement,
                plan_baseline.as_deref(),
                report.as_ref(),
            );
            println!("{}", verdict.summary);
            println!("Итог: {}", if verdict.passed { "PASS" } else { "FAIL" });
            if !verdict.passed {
                std::process::exit(1);
            }
        }
        ControlCmd::Fp { cmd } => match cmd {
            FpCmd::Mark {
                rule,
                file,
                note,
                repo,
            } => {
                let repo = repo.unwrap_or_else(|| PathBuf::from("."));
                let path =
                    arch_harness::digest::fp_register_mark(&repo, &rule, &file, note.as_deref())?;
                println!("Пометка FP записана: {}", path.display());
            }
        },
    }
    Ok(())
}
