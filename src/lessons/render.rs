//! Детерминированный рендер реестра уроков в `LESSONS.md` (ADR-085).
//!
//! Рендер — **производная** журнала: он собирается только из сохранённых
//! данных и отсортирован по `id`, поэтому два прогона на одном журнале дают
//! байт-в-байт одинаковый файл (метка времени генерации намеренно не
//! печатается — иначе файл менялся бы сам собой и дрейфовал в git).
//!
//! Запись атомарна: текст кладётся во временный файл рядом с целевым и
//! переименовывается (`rename` в пределах каталога атомарен), поэтому
//! читатель-исполнитель никогда не видит половину файла.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use crate::error::{HarnessError, Result};

use super::ledger::{Lesson, LessonStatus};

/// Заголовок файла реестра (стабильный — входит в байт-в-байт контракт).
pub const TITLE: &str = "# Реестр уроков контура (Lesson Ledger)";

/// Пояснение под заголовком: реестр событий первичен, файл — производная.
pub const NOTE: &str = "> Файл собран детерминированно из `.arch/lessons/ledger.jsonl` (ADR-085):\n\
                        > правьте журнал событий, а не этот файл. Уроков в статусе `retired`\n\
                        > в блок инъекции не попадает. Сортировка — по `id`.\n";

/// Рендерит реестр в Markdown. Функция чистая: один и тот же вход — один и
/// тот же выход (байт-в-байт).
#[must_use]
pub fn render(lessons: &[Lesson]) -> String {
    let mut sorted: Vec<&Lesson> = lessons.iter().collect();
    sorted.sort_by(|a, b| a.id.cmp(&b.id));

    let mut out = String::new();
    let _ = writeln!(out, "{TITLE}");
    let _ = writeln!(out);
    let _ = write!(out, "{NOTE}");
    let _ = writeln!(out);
    let _ = writeln!(out, "{}", summary_line(&sorted));
    let _ = writeln!(out);
    if sorted.is_empty() {
        let _ = writeln!(
            out,
            "Реестр пуст: уроков нет. Записи появляются из вердиктов гейта и \
             fitness-правил (`arch-be lessons record`)."
        );
        return out;
    }
    for lesson in sorted {
        let _ = write!(out, "{}", render_lesson(lesson));
        let _ = writeln!(out);
    }
    out
}

/// Строка сводки: всего уроков и разбивка по статусам в фиксированном порядке.
#[must_use]
pub fn summary_line(lessons: &[&Lesson]) -> String {
    let counts: Vec<String> = LessonStatus::ALL
        .iter()
        .map(|status| {
            let n = lessons.iter().filter(|l| l.status == *status).count();
            format!("{status} {n}")
        })
        .collect();
    format!("Уроков: {} ({})", lessons.len(), counts.join(", "))
}

/// Рендер одного урока (завершается переводом строки, без пустой строки).
#[must_use]
pub fn render_lesson(lesson: &Lesson) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "## {id} — {signature} · {status}",
        id = lesson.id,
        signature = lesson.signature.describe(),
        status = lesson.status
    );
    let _ = writeln!(out);
    let statement = if lesson.statement.trim().is_empty() {
        "(формулировка не зафиксирована — статус от текста не зависит)"
    } else {
        lesson.statement.trim()
    };
    let _ = writeln!(out, "- **Правило:** {statement}");
    let _ = writeln!(out, "- **Область:** {}", lesson.scope.describe());
    let _ = writeln!(
        out,
        "- **Счётчики:** hits {} · misses {} (подряд {}) · fails {} · задач {}",
        lesson.hits,
        lesson.misses,
        lesson.consecutive_misses,
        lesson.fails,
        lesson.task_count()
    );
    let _ = writeln!(
        out,
        "- **Провенанс:** записал {} · arch-be {} · sha256 вердикта {}",
        lesson.provenance.recorder,
        lesson.provenance.arch_be,
        short_hash(&lesson.provenance.verdict_sha256)
    );
    if let Some(rule) = &lesson.promoted_rule {
        let _ = writeln!(out, "- **Правило реестра:** {rule}");
    }
    if let Some(reason) = &lesson.retired_reason {
        let _ = writeln!(out, "- **Причина вывода:** {reason}");
    }
    if let Some(expiry) = &lesson.expiry {
        let _ = writeln!(out, "- **Срок:** {expiry}");
    }
    if lesson.evidence.is_empty() {
        let _ = writeln!(out, "- **Доказательства:** нет");
    } else {
        let _ = writeln!(out, "- **Доказательства:**");
        for evidence in &lesson.evidence {
            let _ = writeln!(out, "{}", render_evidence(evidence));
        }
    }
    out
}

/// Рендер одного доказательства (строка списка, отступ в два пробела).
fn render_evidence(evidence: &super::ledger::Evidence) -> String {
    let mut parts = Vec::new();
    parts.push(evidence.ts.clone());
    parts.push(evidence.outcome.to_string());
    parts.push(evidence.run_id.clone());
    if let Some(harness) = &evidence.harness {
        parts.push(harness.clone());
    }
    if let Some(location) = evidence.location() {
        parts.push(location);
    }
    let mut line = format!("  - {}", parts.join(" · "));
    if !evidence.verdict_excerpt.trim().is_empty() {
        let excerpt = evidence.verdict_excerpt.trim().replace('\n', " ");
        let _ = write!(line, " · `{excerpt}`");
    }
    line
}

/// Первые 12 hex-символов хэша (в рендере хэш нужен для сверки, целиком — в JSON).
fn short_hash(hash: &str) -> String {
    if hash.is_empty() {
        return "—".to_string();
    }
    hash.chars().take(12).collect()
}

/// Атомарная запись текста: временный файл рядом с целевым плюс `rename`.
///
/// # Errors
/// Ошибка создания каталога, записи временного файла или переименования.
pub fn write_atomic(path: &Path, text: &str) -> Result<()> {
    let dir = path
        .parent()
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
    std::fs::create_dir_all(&dir).map_err(|e| HarnessError::io(&dir, e))?;
    let name = path.file_name().map_or_else(
        || "LESSONS.md".to_string(),
        |n| n.to_string_lossy().into_owned(),
    );
    // Уникальность временного имени — по pid: два процесса могут перерисовывать
    // реестр одновременно (флот, L8), и общий tmp-файл был бы гонкой.
    let tmp = dir.join(format!(".{name}.tmp.{}", std::process::id()));
    std::fs::write(&tmp, text).map_err(|e| HarnessError::io(&tmp, e))?;
    std::fs::rename(&tmp, path).map_err(|e| HarnessError::io(path, e))
}

/// Рендерит и атомарно записывает `LESSONS.md` рядом с журналом.
///
/// # Errors
/// Ошибка записи файла.
pub fn render_to(path: &Path, lessons: &[Lesson]) -> Result<()> {
    write_atomic(path, &render(lessons))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lessons::ledger::{
        Evidence, Ledger, LessonSource, Outcome, Provenance, RecordInput, Signature, Thresholds,
    };
    use crate::lessons::scope::{Scope, ScopeLevel};

    fn lesson(code: &str, status_note: &str) -> (RecordInput, String) {
        let signature = Signature::new(LessonSource::Gate, code);
        let id = signature.id();
        let input = RecordInput::new(
            signature,
            Evidence {
                run_id: format!("run-{code}"),
                harness: Some("claude-code".to_string()),
                verdict_excerpt: format!("[{code}] {status_note}"),
                file: Some("src/lib.rs".to_string()),
                line: Some(12),
                ts: "2026-10-14T10:00:00.000Z".to_string(),
                outcome: Outcome::Fail,
                task: Some("T1".to_string()),
            },
        )
        .with_statement(format!("Правило {code}"))
        .with_scope(Scope::new(ScopeLevel::Module, vec!["src/**".to_string()]));
        (input, id)
    }

    /// Байт-в-байт детерминизм: два прогона рендера дают один файл, порядок —
    /// по id независимо от порядка входа.
    #[test]
    fn render_is_byte_stable_and_sorted_by_id() {
        let th = Thresholds::default();
        let dir = tempfile::tempdir().expect("tempdir");
        let ledger = Ledger::at(dir.path().join("ledger.jsonl"));
        for code in ["G6", "G5", "G7"] {
            let (input, _) = lesson(code, "нарушение");
            ledger.record(&input, &th, "2026-10-14").expect("record");
        }
        let snapshot = ledger.snapshot(&th).expect("snapshot");
        let first = render(&snapshot.lessons);
        let second = render(&snapshot.lessons);
        assert_eq!(first, second, "рендер детерминирован");

        // Порядок входа не влияет: обратный список даёт тот же текст.
        let mut reversed = snapshot.lessons.clone();
        reversed.reverse();
        assert_eq!(render(&reversed), first, "сортировка по id");

        // Уроки идут по возрастанию id.
        let ids: Vec<&str> = snapshot.lessons.iter().map(|l| l.id.as_str()).collect();
        let mut sorted = ids.clone();
        sorted.sort_unstable();
        assert_eq!(ids, sorted, "проекция уже отсортирована");
        assert!(first.contains("Уроков: 3"), "{first}");
        assert!(first.contains("observed 3"), "{first}");
    }

    /// Детерминизм сквозь диск: запись → рендер → перерисовка из журнала
    /// даёт те же байты (и с временным файлом не остаётся мусора).
    #[test]
    fn render_to_disk_is_stable_and_leaves_no_temp_files() {
        let th = Thresholds::default();
        let dir = tempfile::tempdir().expect("tempdir");
        let ledger = Ledger::at(dir.path().join("ledger.jsonl"));
        let (input, id) = lesson("G5", "нет __all__");
        ledger.record(&input, &th, "2026-10-14").expect("record");

        let path = ledger.lessons_md_path();
        let snapshot = ledger.snapshot(&th).expect("snapshot");
        render_to(&path, &snapshot.lessons).expect("render 1");
        let first = std::fs::read(&path).expect("read");
        render_to(&path, &snapshot.lessons).expect("render 2");
        let second = std::fs::read(&path).expect("read");
        assert_eq!(first, second, "файл байт-в-байт стабилен");
        let text = String::from_utf8(first).expect("utf-8");
        assert!(text.contains(&id), "урок попал в рендер");
        assert!(text.contains("gate · G5"), "{text}");
        assert!(text.contains("module (globs: src/**)"), "{text}");
        assert!(text.contains("`[G5] нет __all__`"), "{text}");
        assert!(text.contains("src/lib.rs:12"), "{text}");
        let leftovers: Vec<String> = std::fs::read_dir(dir.path())
            .expect("dir")
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains(".tmp."))
            .collect();
        assert!(
            leftovers.is_empty(),
            "временных файлов не осталось: {leftovers:?}"
        );
    }

    #[test]
    fn empty_registry_renders_header_without_lessons() {
        let text = render(&[]);
        assert!(text.starts_with(TITLE), "{text}");
        assert!(text.contains("Уроков: 0"), "{text}");
        assert!(text.contains("Реестр пуст"), "{text}");
        assert_eq!(render(&[]), text, "пустой рендер тоже детерминирован");
    }

    #[test]
    fn evidence_without_optional_fields_renders_gracefully() {
        let th = Thresholds::default();
        let dir = tempfile::tempdir().expect("tempdir");
        let ledger = Ledger::at(dir.path().join("ledger.jsonl"));
        let input = RecordInput::new(
            Signature::new(LessonSource::Tool, "bash:код 2"),
            Evidence {
                run_id: "r1".to_string(),
                harness: None,
                verdict_excerpt: String::new(),
                file: None,
                line: None,
                ts: "2026-10-14T10:00:00.000Z".to_string(),
                outcome: Outcome::Fail,
                task: None,
            },
        );
        ledger.record(&input, &th, "2026-10-14").expect("record");
        let snapshot = ledger.snapshot(&th).expect("snapshot");
        let text = render(&snapshot.lessons);
        assert!(
            text.contains("2026-10-14T10:00:00.000Z · fail · r1"),
            "{text}"
        );
        assert!(
            !text.contains("harness"),
            "пустые опциональные поля не печатаются"
        );
    }

    #[test]
    fn provenance_hash_is_shortened_in_markdown() {
        let provenance = Provenance::new("cli", None, "вердикт");
        assert_eq!(provenance.verdict_sha256.len(), 64);
        assert_eq!(short_hash(&provenance.verdict_sha256).len(), 12);
        assert_eq!(short_hash(""), "—");
    }
}
