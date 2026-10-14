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
    self, Evidence, Ledger, Lesson, LessonSource, LessonStatus, Mutation, Outcome, RecordInput,
    Scope, ScopeLevel, Signature, Thresholds,
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
    },
    /// Экспорт реестра в JSON (базовая запись; подписанный `lessons-bundle/v1` —
    /// волна L10).
    Export {
        /// Куда писать JSON (по умолчанию — stdout).
        #[arg(long, value_name = "FILE")]
        out: Option<PathBuf>,
        /// Минимальный статус отбора (observed|candidate|confirmed|promoted).
        #[arg(long, value_name = "STATUS")]
        min_status: Option<String>,
        /// Репозиторий с реестром.
        #[arg(long, value_name = "PATH")]
        repo: Option<PathBuf>,
        /// Пользовательский реестр `$ARCH_HOME/lessons`.
        #[arg(long)]
        user: bool,
    },
    /// Импорт реестра из JSON: урок БЕЗ доказательств отклоняется целиком
    /// (сначала проверяются все, потом записывается что можно). Статус из
    /// файла не принимается — он выводится из счётчиков (AD-2).
    Import {
        /// Файл JSON (или `-` — stdin).
        file: String,
        /// Репозиторий с реестром.
        #[arg(long, value_name = "PATH")]
        repo: Option<PathBuf>,
        /// Пользовательский реестр `$ARCH_HOME/lessons`.
        #[arg(long)]
        user: bool,
        /// Только показать, что было бы записано.
        #[arg(long)]
        dry_run: bool,
    },
}

/// Обрабатывает `arch-be lessons …`.
pub(crate) fn cmd_lessons(cfg: &Config, cmd: LessonsCmd) -> Result<()> {
    match cmd {
        LessonsCmd::List {
            repo,
            user,
            status,
            scope_level,
            format,
        } => {
            let ledger = select_ledger(repo.as_deref(), user)?;
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
            json,
        } => {
            let ledger = select_ledger(repo.as_deref(), user)?;
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
        } => {
            let ledger = select_ledger(repo.as_deref(), user)?;
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
        } => {
            let ledger = select_ledger(repo.as_deref(), user)?;
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
            repo,
            user,
        } => {
            let ledger = select_ledger(repo.as_deref(), user)?;
            let th = thresholds(cfg);
            let snapshot = ledger.snapshot(&th)?;
            let min = min_status
                .as_deref()
                .map(str::parse::<LessonStatus>)
                .transpose()?
                .map(status_rank);
            let bundle = export_bundle(&snapshot.lessons, min);
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
            dry_run,
        } => {
            let ledger = select_ledger(repo.as_deref(), user)?;
            let th = thresholds(cfg);
            let text = read_text(&file)?;
            let imported = parse_bundle(&text)?;
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
            if dry_run {
                println!("Проверено уроков: {}", imported.len());
                for lesson in &imported {
                    println!(
                        "  {} (будет заведён как observed/candidate по счётчикам)",
                        lesson.id
                    );
                }
                return Ok(());
            }
            let today = lessons::today();
            for lesson in &imported {
                let mutation = ledger.adopt(lesson, &th, &today)?;
                report_mutation(&ledger, &mutation, false);
            }
            write_render(&ledger, &th)?;
            println!("Импортировано уроков: {}", imported.len());
            Ok(())
        }
    }
}

/// Выбор реестра: `--repo` либо `--user` (ровно одно осмысленно).
fn select_ledger(repo: Option<&Path>, user: bool) -> Result<Ledger> {
    match (repo, user) {
        (Some(_), true) => bail!(
            "укажите одно: --repo <PATH> (реестр репозитория) или --user \
             ($ARCH_HOME/lessons)"
        ),
        (_, true) => Ok(Ledger::user()),
        (Some(path), false) => Ok(Ledger::repo(path)),
        (None, false) => Ok(Ledger::repo(Path::new("."))),
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

/// Экспортный бандл (базовая форма L1; подпись и `lessons-bundle/v1` — L10).
fn export_bundle(lessons_all: &[Lesson], min_rank: Option<u8>) -> serde_json::Value {
    let selected: Vec<&Lesson> = lessons_all
        .iter()
        .filter(|l| min_rank.is_none_or(|min| status_rank(l.status) >= min))
        .collect();
    serde_json::json!({
        "format": "lessons-bundle/v0",
        "version": lessons::LEDGER_VERSION,
        "arch_be": lessons::arch_be_version(),
        "count": selected.len(),
        "lessons": selected,
    })
}

/// Разбор импортируемого JSON: бандл `{lessons: [...]}`, голый массив или
/// один урок. Битый JSON — понятная ошибка.
fn parse_bundle(text: &str) -> Result<Vec<Lesson>> {
    let value: serde_json::Value =
        serde_json::from_str(text).context("импорт: файл не является JSON")?;
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
    fn bundle_parsing_accepts_bundle_array_and_single_lesson() {
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
        assert_eq!(parse_bundle(&bundle.to_string()).expect("бандл").len(), 1);
        let array = serde_json::Value::Array(vec![lesson.clone()]);
        assert_eq!(parse_bundle(&array.to_string()).expect("массив").len(), 1);
        assert_eq!(
            parse_bundle(&lesson.to_string()).expect("один урок").len(),
            1
        );
        assert!(parse_bundle("не json").is_err());
        assert_eq!(
            parse_bundle(r#"{"format":"lessons-bundle/v0"}"#)
                .expect("бандл без уроков")
                .len(),
            0,
            "бандл без уроков — пустой импорт, а не ошибка"
        );
    }

    #[test]
    fn export_bundle_filters_by_status_rank() {
        let mut lessons = vec![
            make_lesson(LessonStatus::Observed),
            make_lesson(LessonStatus::Confirmed),
        ];
        lessons[1].id = "LSN-87654321".to_string();
        let all = export_bundle(&lessons, None);
        assert_eq!(all["count"], 2);
        let confirmed = export_bundle(&lessons, Some(status_rank(LessonStatus::Confirmed)));
        assert_eq!(confirmed["count"], 1);
        assert_eq!(confirmed["lessons"][0]["status"], "confirmed");
        assert_eq!(all["format"], "lessons-bundle/v0");
    }

    #[test]
    fn repo_and_user_flags_are_mutually_exclusive() {
        assert!(select_ledger(Some(Path::new(".")), true).is_err());
        assert!(select_ledger(None, true).is_ok());
        assert!(select_ledger(Some(Path::new(".")), false).is_ok());
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
