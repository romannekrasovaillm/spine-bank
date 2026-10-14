//! Подкоманды `arch-be lessons …`: реестр уроков Lesson Ledger (ADR-085).
//!
//! Команды — явные действия человека или хост-агента, поэтому они работают
//! независимо от `[lessons] enabled` (флаг гасит автоматические записи гейта
//! и блок инъекции, L2/L3). Пороги переходов читаются из `[lessons]` конфига:
//! CLI не умеет «продавить» статус — он лишь дозаписывает доказательства.
//!
//! Реестр выбирается одним из двух: `--repo <PATH>` (по умолчанию текущий
//! каталог, журнал `<repo>/.arch/lessons/ledger.jsonl`) или `--user`
//! (`$ARCH_HOME/lessons`).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::Subcommand;

use arch_harness::config::Config;
use arch_harness::lessons::{
    self, Evidence, Ledger, Lesson, LessonSource, LessonStatus, Mutation, Outcome, PromoteOptions,
    RecordInput, Scope, ScopeLevel, Signature, Thresholds,
};

/// Подкоманды `arch-be lessons`.
///
/// `Record` заметно крупнее прочих вариантов (13 аргументов доказательства) —
/// это плата clap-derive за плоский CLI; enum живёт один раз за процесс,
/// поэтому боксить вариант нечем и незачем.
#[expect(
    clippy::large_enum_variant,
    reason = "CLI-вариант с аргументами доказательства: clap-derive не боксит поля, enum живёт один раз за процесс"
)]
#[derive(Subcommand)]
pub(crate) enum LessonsCmd {
    /// Список уроков реестра (сводка: подпись, статус, счётчики, область).
    ///
    /// Пустой реестр — не ошибка: печатается шапка с «Уроков: 0».
    List {
        /// Репозиторий с реестром (по умолчанию — текущий каталог).
        #[arg(long, value_name = "PATH")]
        repo: Option<PathBuf>,
        /// Пользовательский реестр `$ARCH_HOME/lessons` вместо репозиторного.
        #[arg(long)]
        user: bool,
        /// Организационный реестр `[lessons] org_dir`.
        #[arg(long)]
        org: bool,
        /// Фильтр по статусу (observed|candidate|confirmed|promoted|retired).
        #[arg(long, value_name = "STATUS")]
        status: Option<String>,
        /// Фильтр по уровню области (file|module|package|repo|user|org).
        #[arg(long = "scope", value_name = "LEVEL")]
        scope_level: Option<String>,
        /// Формат вывода: text (дефолт) | json.
        #[arg(long, default_value = "text", value_name = "FORMAT")]
        format: String,
    },
    /// Показать урок по id (карточка: доказательства, провенанс, причина вывода).
    Show {
        /// Идентификатор урока (`LSN-…`).
        id: String,
        /// Репозиторий с реестром.
        #[arg(long, value_name = "PATH")]
        repo: Option<PathBuf>,
        /// Пользовательский реестр `$ARCH_HOME/lessons`.
        #[arg(long)]
        user: bool,
        /// Организационный реестр `[lessons] org_dir`.
        #[arg(long)]
        org: bool,
        /// Машиночитаемый вывод: JSON урока целиком.
        #[arg(long)]
        json: bool,
    },
    /// Записать исход по подписи: создать урок либо добавить доказательство
    /// (`pass_after_apply` — применение с PASS, `fail` — провал по сигнатуре).
    Record {
        /// Источник урока: gate|fitness|tool|review.
        #[arg(long, value_name = "SOURCE")]
        source: String,
        /// Код: `G5`, id правила реестра, класс ошибки инструмента.
        #[arg(long, value_name = "CODE")]
        code: String,
        /// Вид субъекта (L6): `new_public_symbol`, `new_migration`, …
        #[arg(long, value_name = "KIND")]
        subject_kind: Option<String>,
        /// Текст правила (может быть `[inferred]` — на статус не влияет).
        #[arg(long, value_name = "TEXT")]
        statement: Option<String>,
        /// Уровень области действия (по умолчанию repo).
        #[arg(long = "scope", value_name = "LEVEL")]
        scope_level: Option<String>,
        /// Глоб путей области действия (флаг повторяемый).
        #[arg(long = "glob", value_name = "GLOB")]
        globs: Vec<String>,
        /// Идентификатор прогона, из которого пришло доказательство.
        #[arg(long, value_name = "ID")]
        run_id: String,
        /// Задача (ключ «≥ 2 разных задач» в решении о confirmed).
        #[arg(long, value_name = "TASK")]
        task: Option<String>,
        /// Исполнитель прогона (claude-code, qwen-code, …).
        #[arg(long, value_name = "HARNESS")]
        harness: Option<String>,
        /// Выдержка вердикта (усекается до 512 символов).
        #[arg(long, value_name = "TEXT")]
        excerpt: Option<String>,
        /// Файл нарушения.
        #[arg(long, value_name = "PATH")]
        file: Option<String>,
        /// Строка нарушения.
        #[arg(long, value_name = "N")]
        line: Option<u32>,
        /// Исход доказательства: `fail` (дефолт) | `pass_after_apply`.
        #[arg(long, default_value = "fail", value_name = "OUTCOME")]
        outcome: String,
        /// Кто записал (cli, mcp, gate, hook, agent).
        #[arg(long, default_value = "cli", value_name = "WHO")]
        recorder: String,
        /// Срок жизни урока в днях (0 — без срока; перекрывает `[lessons]
        /// expiry_days`).
        #[arg(long, value_name = "DAYS")]
        expiry_days: Option<u32>,
        /// Репозиторий с реестром.
        #[arg(long, value_name = "PATH")]
        repo: Option<PathBuf>,
        /// Пользовательский реестр `$ARCH_HOME/lessons`.
        #[arg(long)]
        user: bool,
        /// Организационный реестр `[lessons] org_dir`.
        #[arg(long)]
        org: bool,
    },
    /// Вывести урок из реестра (решение человека либо удаление правила).
    Retire {
        /// Идентификатор урока.
        id: String,
        /// Причина вывода (уходит в событие и в провенанс рендера).
        #[arg(long, value_name = "TEXT")]
        reason: String,
        /// Репозиторий с реестром.
        #[arg(long, value_name = "PATH")]
        repo: Option<PathBuf>,
        /// Пользовательский реестр `$ARCH_HOME/lessons`.
        #[arg(long)]
        user: bool,
        /// Организационный реестр `[lessons] org_dir`.
        #[arg(long)]
        org: bool,
    },
    /// Экспорт реестра в подписанный `lessons-bundle/v1` (SHA-256 + хэши
    /// доказательств, ADR-091).
    Export {
        /// Куда писать JSON (по умолчанию — stdout).
        #[arg(long, value_name = "FILE")]
        out: Option<PathBuf>,
        /// Минимальный статус отбора (observed|candidate|confirmed|promoted).
        #[arg(long, value_name = "STATUS")]
        min_status: Option<String>,
        /// Фильтр по уровню области (file|module|package|repo|user|org).
        #[arg(long = "scope", value_name = "LEVEL")]
        scope: Option<String>,
        /// Репозиторий с реестром.
        #[arg(long, value_name = "PATH")]
        repo: Option<PathBuf>,
        /// Пользовательский реестр `$ARCH_HOME/lessons`.
        #[arg(long)]
        user: bool,
        /// Организационный реестр `[lessons] org_dir`.
        #[arg(long)]
        org: bool,
    },
    /// Импорт реестра из JSON: `lessons-bundle/v1` проверяется по подписи и
    /// хэшам доказательств; урок БЕЗ доказательств отклоняется целиком
    /// (сначала проверяются все, потом записывается что можно). Статус из
    /// файла не принимается — чужой урок понижается до `candidate` (ADR-091).
    Import {
        /// Файл JSON (или `-` — stdin).
        file: String,
        /// Репозиторий с реестром.
        #[arg(long, value_name = "PATH")]
        repo: Option<PathBuf>,
        /// Пользовательский реестр `$ARCH_HOME/lessons`.
        #[arg(long)]
        user: bool,
        /// Организационный реестр `[lessons] org_dir`.
        #[arg(long)]
        org: bool,
        /// Разрешить запись уроков уровня user/org в репозиторный реестр
        /// (без флага такие уроки отсекаются — анти-утечка, ADR-091).
        #[arg(long)]
        share: bool,
        /// Только показать, что было бы записано.
        #[arg(long)]
        dry_run: bool,
    },
    /// Миграция предложенных уроков памяти сбоев (`failure_lessons.md`) в
    /// реестр (ADR-087): файл, который до 0.5.0 не читал никто (B2), отдаёт
    /// свои уроки реестру. Идемпотентна: повторный прогон не плодит
    /// доказательств; отсутствие файла — «мигрировать нечего», не ошибка.
    Migrate {
        /// Каталог состояния харнесса с `failure_lessons.md`
        /// (по умолчанию — `paths.state_dir` конфига).
        #[arg(long, value_name = "PATH")]
        state_dir: Option<PathBuf>,
        /// Репозиторий с реестром.
        #[arg(long, value_name = "PATH")]
        repo: Option<PathBuf>,
        /// Пользовательский реестр `$ARCH_HOME/lessons`.
        #[arg(long)]
        user: bool,
        /// Организационный реестр `[lessons] org_dir`.
        #[arg(long)]
        org: bool,
    },
    /// Промоушен урока в fitness-правило (L7, ADR-089): confirmed-урок,
    /// выразимый шаблоном `rule_templates`, проходит redteam-засев и пишется в
    /// CONSTRAINTS.yaml со статусом `proposed` через дельту. На маршрутах
    /// Standard/Critical обязательна подпись человека (`--sign-off`).
    Promote {
        /// Идентификатор урока (`LSN-…`) в статусе confirmed.
        id: String,
        /// Репозиторий с реестром (по умолчанию — текущий каталог).
        #[arg(long, value_name = "PATH")]
        repo: Option<PathBuf>,
        /// Маршрут значимости: fast|standard|critical (по умолчанию fast).
        #[arg(long, default_value = "fast", value_name = "ROUTE")]
        route: String,
        /// Подпись человека: маркер `[РЕШЕНИЕ ЧЕЛОВЕКА]` уходит в дельту
        /// (обязателен на Standard/Critical).
        #[arg(long)]
        sign_off: bool,
        /// Инвариант спайна для поля `ad` правила (`AD-6`).
        #[arg(long, value_name = "AD")]
        ad: Option<String>,
        /// Имя дельты (по умолчанию — `promote-<LSN>`).
        #[arg(long, value_name = "NAME")]
        delta_name: Option<String>,
    },
}

/// Обрабатывает `arch-be lessons …`.
pub(crate) fn cmd_lessons(cfg: &Config, cmd: LessonsCmd) -> Result<()> {
    match cmd {
        LessonsCmd::List {
            repo,
            user,
            org,
            status,
            scope_level,
            format,
        } => {
            let ledger = select_ledger(cfg.lessons.org_dir.as_deref(), repo.as_deref(), user, org)?;
            let th = thresholds(cfg);
            let snapshot = ledger.snapshot(&th)?;
            let status_filter = status
                .as_deref()
                .map(str::parse::<LessonStatus>)
                .transpose()?;
            let scope_filter = scope_level
                .as_deref()
                .map(str::parse::<ScopeLevel>)
                .transpose()?;
            let selected: Vec<&Lesson> = snapshot
                .lessons
                .iter()
                .filter(|l| status_filter.is_none_or(|s| l.status == s))
                .filter(|l| scope_filter.is_none_or(|level| l.scope.level == level))
                .collect();
            if format.trim().eq_ignore_ascii_case("json") {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "registry": ledger.path().display().to_string(),
                        "lessons_md": ledger.lessons_md_path().display().to_string(),
                        "events": snapshot.events,
                        "malformed_lines": snapshot.malformed,
                        "count": selected.len(),
                        "lessons": selected,
                    }))?
                );
                return Ok(());
            }
            println!("Реестр: {}", ledger.path().display());
            println!(
                "Событий: {}, нераспознанных строк: {}",
                snapshot.events, snapshot.malformed
            );
            println!(
                "Уроков: {} (отобрано {})",
                snapshot.lessons.len(),
                selected.len()
            );
            for lesson in selected {
                println!("  {}", lesson.summary_line());
            }
            if snapshot.lessons.is_empty() {
                println!(
                    "  реестр пуст: записи появляются из вердиктов гейта \
                     (`arch-be lessons record …`)"
                );
            }
            Ok(())
        }
        LessonsCmd::Show {
            id,
            repo,
            user,
            org,
            json,
        } => {
            let ledger = select_ledger(cfg.lessons.org_dir.as_deref(), repo.as_deref(), user, org)?;
            let th = thresholds(cfg);
            let snapshot = ledger.snapshot(&th)?;
            let Some(lesson) = snapshot.get(&id) else {
                bail!(
                    "реестр уроков: урок {id} не найден в {}",
                    ledger.path().display()
                );
            };
            if json {
                println!("{}", serde_json::to_string_pretty(lesson)?);
            } else {
                println!("Реестр: {}", ledger.path().display());
                println!();
                print!("{}", lessons::render_lesson(lesson));
            }
            Ok(())
        }
        LessonsCmd::Record {
            source,
            code,
            subject_kind,
            statement,
            scope_level,
            globs,
            run_id,
            task,
            harness,
            excerpt,
            file,
            line,
            outcome,
            recorder,
            expiry_days,
            repo,
            user,
            org,
        } => {
            let ledger = select_ledger(cfg.lessons.org_dir.as_deref(), repo.as_deref(), user, org)?;
            let th = thresholds(cfg);
            let today = lessons::today();
            let mut signature = Signature::new(source.parse::<LessonSource>()?, code);
            if let Some(kind) = subject_kind {
                signature = signature.with_subject(kind);
            }
            let level: ScopeLevel = resolve_scope_level(scope_level)?;
            let evidence = Evidence {
                run_id,
                harness,
                verdict_excerpt: excerpt.unwrap_or_default(),
                file,
                line,
                ts: lessons::now_iso(),
                outcome: outcome.parse::<Outcome>()?,
                task,
            };
            let mut input = RecordInput::new(signature, evidence)
                .with_statement(statement.unwrap_or_default())
                .with_scope(Scope::new(level, globs))
                .with_recorder(recorder);
            // Срок отсчитывается от даты записи (сегодня); флаг перекрывает
            // `[lessons] expiry_days`.
            let mut policy = cfg.lessons.clone();
            if let Some(days) = expiry_days {
                policy.expiry_days = days;
            }
            if let Some(expiry) = policy.expiry_from(&today) {
                input = input.with_expiry(expiry);
            }
            let mutation = ledger.record(&input, &th, &today)?;
            write_render(&ledger, &th)?;
            report_mutation(&ledger, &mutation, !cfg.lessons.enabled);
            Ok(())
        }
        LessonsCmd::Retire {
            id,
            reason,
            repo,
            user,
            org,
        } => {
            let ledger = select_ledger(cfg.lessons.org_dir.as_deref(), repo.as_deref(), user, org)?;
            let th = thresholds(cfg);
            let mutation = ledger.retire(&id, &reason, &th)?;
            write_render(&ledger, &th)?;
            if mutation.events.is_empty() {
                println!("Урок {id} уже в статусе retired — событие не добавлено");
            } else {
                report_mutation(&ledger, &mutation, false);
            }
            Ok(())
        }
        LessonsCmd::Export {
            out,
            min_status,
            scope,
            repo,
            user,
            org,
        } => {
            let ledger = select_ledger(cfg.lessons.org_dir.as_deref(), repo.as_deref(), user, org)?;
            let th = thresholds(cfg);
            let snapshot = ledger.snapshot(&th)?;
            let min = min_status
                .as_deref()
                .map(str::parse::<LessonStatus>)
                .transpose()?
                .map(status_rank);
            let scope_filter = scope.as_deref().map(str::parse::<ScopeLevel>).transpose()?;
            let selected: Vec<Lesson> = snapshot
                .lessons
                .iter()
                .filter(|l| min.is_none_or(|m| status_rank(l.status) >= m))
                .filter(|l| scope_filter.is_none_or(|level| l.scope.level == level))
                .cloned()
                .collect();
            let bundle = lessons::bundle::build(&selected, lessons::arch_be_version())?;
            let text = serde_json::to_string_pretty(&bundle)?;
            match out {
                Some(path) => {
                    lessons::write_atomic(&path, &format!("{text}\n"))
                        .with_context(|| format!("запись {}", path.display()))?;
                    println!("Экспорт: {}", path.display());
                }
                None => println!("{text}"),
            }
            Ok(())
        }
        LessonsCmd::Import {
            file,
            repo,
            user,
            org,
            share,
            dry_run,
        } => {
            let ledger = select_ledger(cfg.lessons.org_dir.as_deref(), repo.as_deref(), user, org)?;
            let th = thresholds(cfg);
            let text = read_text(&file)?;
            let imported = parse_import(&text)?;
            if imported.is_empty() {
                println!("В файле нет уроков — импортировать нечего");
                return Ok(());
            }
            // Сначала проверяем ВСЁ (урок без доказательств отклоняет импорт),
            // и только потом пишем: частично залитый чужой реестр хуже отказа.
            for lesson in &imported {
                lessons::validate_imported(lesson)
                    .with_context(|| format!("импорт отклонён (урок {})", lesson.id))?;
            }
            // Анти-утечка (ADR-091): в репозиторный реестр без --share не пишем
            // уроки уровня user/org — личное знание не становится свойством
            // чужого репозитория.
            let target_repo = !user && !org;
            let (kept, excluded) = import_scoped(&imported, share, target_repo);
            if dry_run {
                println!("Проверено уроков: {}", imported.len());
                for lesson in &kept {
                    println!("  {} (будет заведён как candidate — чужой урок)", lesson.id);
                }
                if excluded > 0 {
                    println!("Отсечено user/org без --share: {excluded}");
                }
                return Ok(());
            }
            let today = lessons::today();
            for lesson in &kept {
                let mutation = ledger.adopt(lesson, &th, &today)?;
                report_mutation(&ledger, &mutation, false);
            }
            write_render(&ledger, &th)?;
            println!("Импортировано уроков: {}", kept.len());
            if excluded > 0 {
                println!("Отсечено user/org без --share: {excluded}");
            }
            Ok(())
        }
        LessonsCmd::Migrate {
            state_dir,
            repo,
            user,
            org,
        } => {
            let ledger = select_ledger(cfg.lessons.org_dir.as_deref(), repo.as_deref(), user, org)?;
            let th = thresholds(cfg);
            let dir = state_dir.unwrap_or_else(|| cfg.paths.state_dir.clone());
            let file = dir.join(arch_harness::failure_memory::LESSONS_FILE);
            let moved = arch_harness::failure_memory::migrate_to_ledger(
                &file,
                &ledger,
                &th,
                &lessons::today(),
            )
            .with_context(|| format!("миграция {}", file.display()))?;
            if moved == 0 {
                println!(
                    "Мигрировать нечего: {} нет или уроков в нём нет",
                    file.display()
                );
                return Ok(());
            }
            write_render(&ledger, &th)?;
            println!(
                "Мигрировано уроков из {}: {} (источник tool, статус — по счётчикам)",
                file.display(),
                moved
            );
            Ok(())
        }
        LessonsCmd::Promote {
            id,
            repo,
            route,
            sign_off,
            ad,
            delta_name,
        } => {
            let repo = repo.unwrap_or_else(|| PathBuf::from("."));
            let th = thresholds(cfg);
            let route = route
                .parse::<arch_harness::control::Route>()
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            let opts = PromoteOptions {
                route,
                sign_off,
                ad,
                delta_name,
            };
            let report = lessons::promote(&repo, &id, &opts, &th)?;
            println!("Урок {} → правило {}", report.lesson_id, report.rule_name);
            println!("  вид: {}", report.kind.as_str());
            println!("  реестр: {}", report.constraints.display());
            println!("  дельта: {}", report.delta.display());
            println!("\nФрагмент правила:\n{}", report.yaml);
            Ok(())
        }
    }
}

/// Выбор реестра: `--repo` либо `--user` либо `--org` (ровно одно осмысленно).
/// Org-реестр без `[lessons] org_dir` — понятная ошибка, а не молчание.
fn select_ledger(
    org_dir: Option<&Path>,
    repo: Option<&Path>,
    user: bool,
    org: bool,
) -> Result<Ledger> {
    match (repo, user, org) {
        (_, true, true) => bail!("укажите одно: --user или --org"),
        (Some(_), true, _) | (Some(_), _, true) => bail!(
            "укажите одно: --repo <PATH> (реестр репозитория), --user \
             ($ARCH_HOME/lessons) или --org ([lessons] org_dir)"
        ),
        (_, false, true) => {
            let dir = org_dir.ok_or_else(|| {
                anyhow::anyhow!("[lessons] org_dir не настроен — org-реестр недоступен")
            })?;
            Ok(Ledger::org(dir))
        }
        (_, true, false) => Ok(Ledger::user()),
        (Some(path), false, false) => Ok(Ledger::repo(path)),
        (None, false, false) => Ok(Ledger::repo(Path::new("."))),
    }
}

/// Пороги переходов из секции `[lessons]`.
fn thresholds(cfg: &Config) -> Thresholds {
    Thresholds::from(&cfg.lessons)
}

/// Уровень области из аргумента (по умолчанию — `repo`).
fn resolve_scope_level(raw: Option<String>) -> Result<ScopeLevel> {
    match raw {
        Some(level) => Ok(level.parse()?),
        None => Ok(ScopeLevel::default()),
    }
}

/// Перерисовывает детерминированный `LESSONS.md` (ошибка рендера не отменяет
/// уже записанное событие — журнал первичен, файл производная).
fn write_render(ledger: &Ledger, th: &Thresholds) -> Result<()> {
    let snapshot = ledger.snapshot(th)?;
    lessons::render::render_to(&ledger.lessons_md_path(), &snapshot.lessons)
        .with_context(|| format!("рендер {}", ledger.lessons_md_path().display()))
}

/// Печать итога мутации: что записано и как изменился статус.
fn report_mutation(ledger: &Ledger, mutation: &Mutation, note_flag: bool) {
    let lesson = &mutation.lesson;
    println!(
        "{} {} · {} · {}",
        if mutation.created {
            "Создан урок"
        } else {
            "Урок"
        },
        lesson.id,
        lesson.signature.describe(),
        lesson.status
    );
    if let Some(previous) = mutation.previous_status {
        if previous != lesson.status {
            println!("  статус: {previous} → {}", lesson.status);
        }
    }
    println!(
        "  hits {} · misses {} (подряд {}) · fails {} · задач {}",
        lesson.hits,
        lesson.misses,
        lesson.consecutive_misses,
        lesson.fails,
        lesson.task_count()
    );
    println!("  реестр: {}", ledger.path().display());
    println!("  рендер: {}", ledger.lessons_md_path().display());
    if note_flag {
        println!(
            "  [lessons] enabled=false: автоматические записи гейта выключены, \
             запись сделана явной командой"
        );
    }
}

/// Порядковый номер статуса в жизненном цикле (для `--min-status`).
fn status_rank(status: LessonStatus) -> u8 {
    match status {
        LessonStatus::Observed => 0,
        LessonStatus::Candidate => 1,
        LessonStatus::Confirmed => 2,
        LessonStatus::Promoted => 3,
        LessonStatus::Retired => 4,
    }
}

/// Разбор импортируемого JSON: подписанный `lessons-bundle/v1` (проверяется
/// подпись и хэши доказательств), бандл `{lessons: [...]}`, голый массив или
/// один урок. Битый JSON — понятная ошибка.
fn parse_import(text: &str) -> Result<Vec<Lesson>> {
    let value: serde_json::Value =
        serde_json::from_str(text).context("импорт: файл не является JSON")?;
    // Подписанный бандл v1: уроки возвращаются только после проверки подписи
    // и хэшей — подменённые данные отклоняются до записи (ADR-091).
    if value
        .get("format")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|format| format == lessons::BUNDLE_FORMAT)
    {
        let bundle: lessons::LessonsBundle =
            serde_json::from_value(value).context("импорт: бандл lessons-bundle/v1 не разобран")?;
        lessons::bundle::verify(&bundle)
            .context("импорт отклонён: подпись/хэши бандла не прошли проверку")?;
        return Ok(lessons::bundle::lessons_of(&bundle));
    }
    let items = match value {
        serde_json::Value::Array(items) => items,
        // Бандл: `lessons` (массив) плюс метаданные формата. Бандл без уроков —
        // пустой импорт, а не ошибка.
        serde_json::Value::Object(ref map)
            if map.contains_key("lessons") || map.contains_key("format") =>
        {
            map.get("lessons")
                .and_then(serde_json::Value::as_array)
                .cloned()
                .unwrap_or_default()
        }
        serde_json::Value::Object(_) => vec![value],
        other => bail!("импорт: ожидался бандл, массив или урок, получено {other}"),
    };
    items
        .into_iter()
        .map(|item| {
            serde_json::from_value::<Lesson>(item)
                .context("импорт: элемент не является уроком реестра")
        })
        .collect()
}

/// Уроки, пригодные для записи в целевой реестр: без `--share` при записи в
/// репозиторий отсекаются уроки уровня шире репозитория (`user`/`org`) —
/// личное/организационное знание не становится свойством чужого репозитория
/// (ADR-091, риск 6). Возвращает `(оставленные, число отсечённых)`.
fn import_scoped(lessons: &[Lesson], share: bool, target_repo: bool) -> (Vec<Lesson>, usize) {
    if share || !target_repo {
        return (lessons.to_vec(), 0);
    }
    let mut kept = Vec::new();
    let mut excluded = 0;
    for lesson in lessons {
        if lesson.scope.level.rank() > ScopeLevel::Repo.rank() {
            excluded += 1;
        } else {
            kept.push(lesson.clone());
        }
    }
    (kept, excluded)
}

/// Чтение файла или stdin (`-`).
fn read_text(file: &str) -> Result<String> {
    if file == "-" {
        use std::io::Read as _;
        let mut buf = String::new();
        std::io::stdin()
            .read_to_string(&mut buf)
            .context("чтение stdin")?;
        return Ok(buf);
    }
    std::fs::read_to_string(file).with_context(|| format!("чтение {file}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn import_parsing_accepts_bundle_array_and_single_lesson() {
        let lesson = serde_json::json!({
            "id": "LSN-12345678",
            "signature": {"source": "gate", "code": "G5"},
            "statement": "правило",
            "scope": {"level": "repo", "globs": []},
            "evidence": [{
                "run_id": "r1",
                "ts": "2026-10-14T10:00:00Z",
                "outcome": "fail",
                "verdict_excerpt": "[G5] нет __all__"
            }],
            "provenance": {"recorder": "cli", "verdict_sha256": "ab", "arch_be": "0.4.11"}
        });
        let bundle = serde_json::json!({"format": "lessons-bundle/v0", "lessons": [lesson]});
        assert_eq!(parse_import(&bundle.to_string()).expect("бандл").len(), 1);
        let array = serde_json::Value::Array(vec![lesson.clone()]);
        assert_eq!(parse_import(&array.to_string()).expect("массив").len(), 1);
        assert_eq!(
            parse_import(&lesson.to_string()).expect("один урок").len(),
            1
        );
        assert!(parse_import("не json").is_err());
        assert_eq!(
            parse_import(r#"{"format":"lessons-bundle/v0"}"#)
                .expect("бандл без уроков")
                .len(),
            0,
            "бандл без уроков — пустой импорт, а не ошибка"
        );
    }

    #[test]
    fn signed_bundle_parses_after_verification_and_rejects_tampering() {
        let lesson = make_lesson(LessonStatus::Confirmed);
        let bundle = lessons::bundle::build(std::slice::from_ref(&lesson), "0.5.0").expect("build");
        let text = serde_json::to_string(&bundle).expect("serde");
        let parsed = parse_import(&text).expect("свежий бандл");
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].id, lesson.id);

        // Подмена тела урока ломает подпись — импорт отклоняется.
        let mut tampered = bundle;
        tampered.lessons[0].lesson.statement = "чужое правило".to_string();
        let text = serde_json::to_string(&tampered).expect("serde");
        let err = parse_import(&text).expect_err("подмена отклонена");
        assert!(err.to_string().contains("подпись"), "{err}");
    }

    #[test]
    fn import_scoped_guards_repo_writes_from_user_org_leak() {
        let repo_lesson = make_lesson(LessonStatus::Candidate);
        let mut user_lesson = make_lesson(LessonStatus::Candidate);
        user_lesson.scope = Scope::new(ScopeLevel::User, Vec::<String>::new());
        let mut org_lesson = make_lesson(LessonStatus::Candidate);
        org_lesson.scope = Scope::new(ScopeLevel::Org, Vec::<String>::new());
        let lessons = vec![repo_lesson, user_lesson, org_lesson];

        // Без --share в репозиторий: остаётся только repo-урок.
        let (kept, excluded) = import_scoped(&lessons, false, true);
        assert_eq!(kept.len(), 1);
        assert_eq!(excluded, 2);
        // С --share — все три.
        let (kept, excluded) = import_scoped(&lessons, true, true);
        assert_eq!(kept.len(), 3);
        assert_eq!(excluded, 0);
        // В нерепозиторный реестр (user/org) фильтр не применяется.
        let (kept, excluded) = import_scoped(&lessons, false, false);
        assert_eq!(kept.len(), 3);
        assert_eq!(excluded, 0);
    }

    #[test]
    fn registry_flags_are_mutually_exclusive() {
        assert!(select_ledger(None, Some(Path::new(".")), true, false).is_err());
        assert!(select_ledger(None, None, true, false).is_ok());
        assert!(select_ledger(None, Some(Path::new(".")), false, false).is_ok());
        assert!(
            select_ledger(None, None, false, true).is_err(),
            "org без org_dir — ошибка"
        );
        assert!(select_ledger(Some(Path::new("/org")), None, false, true).is_ok());
        assert!(
            select_ledger(None, None, true, true).is_err(),
            "--user и --org вместе — ошибка"
        );
    }

    fn make_lesson(status: LessonStatus) -> Lesson {
        let mut lesson = Lesson::from_input(
            &RecordInput::new(
                Signature::new(LessonSource::Gate, "G5"),
                Evidence {
                    run_id: "r1".to_string(),
                    harness: None,
                    verdict_excerpt: "[G5] нет __all__".to_string(),
                    file: None,
                    line: None,
                    ts: "2026-10-14T10:00:00Z".to_string(),
                    outcome: Outcome::Fail,
                    task: None,
                },
            )
            .with_statement("правило"),
        );
        lesson.status = status;
        lesson
    }
}
