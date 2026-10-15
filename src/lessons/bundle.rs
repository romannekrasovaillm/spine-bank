//! Подписанный бандл уроков `lessons-bundle/v1` (ADR-091, L10).
//!
//! Экспорт реестра наружу упаковывает уроки в JSON с:
//! - хэшем SHA-256 каждого доказательства (`evidence_sha256`) — ловит подмену
//!   отдельного evidence после экспорта;
//! - подписью SHA-256 канонической сериализации уроков (`signature`) — ловит
//!   подмену тела урока (формулировки, области, статуса) и перестановку.
//!
//! Паттерн receipt как у archify: подпись — детерминированный криптографический
//! хэш, а не MAC с ключом. Он отличает случайное повреждение и «наивную» правку
//! от целого бандла, но не от подделки тем, кто умеет пересчитать SHA-256.
//! Поэтому импорт (ADR-091) всё равно понижает статус урока до `candidate`:
//! доверие к чужому знанию зарабатывается своим гейтом, а не подписью.
//!
//! Каноническая сериализация — `serde_json::to_string`: для фиксированных
//! структур с `#[derive(Serialize)]` порядок полей и пропуск `None`/
//! `skip_serializing_if` детерминированы, поэтому один и тот же набор уроков
//! даёт один и тот же хэш на экспорте и на импорте.

use serde::{Deserialize, Serialize};

use crate::error::{HarnessError, Result};
use crate::hash::sha256_hex;

use super::ledger::Lesson;

/// Маркер формата бандла.
pub const BUNDLE_FORMAT: &str = "lessons-bundle/v1";
/// Версия схемы бандла (изменения — только аддитивно, ADR-091).
pub const BUNDLE_VERSION: u32 = 1;

/// Урок в бандле: сам урок плюс хэши его доказательств.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BundledLesson {
    /// Урок целиком (id, подпись, формулировка, область, доказательства, статус).
    #[serde(flatten)]
    pub lesson: Lesson,
    /// SHA-256 канонической сериализации каждого доказательства в порядке
    /// `lesson.evidence`.
    pub evidence_sha256: Vec<String>,
}

/// Бандл уроков: подписанный снимок реестра для переноса между репозиториями.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LessonsBundle {
    /// Маркер формата (`lessons-bundle/v1`).
    pub format: String,
    /// Версия схемы бандла.
    pub v: u32,
    /// Версия arch-be, собравшего бандл.
    pub arch_be: String,
    /// Число уроков в бандле.
    pub count: usize,
    /// Уроки с хэшами доказательств.
    pub lessons: Vec<BundledLesson>,
    /// SHA-256 канонической сериализации уроков (`lessons` без полей хэшей).
    pub signature: String,
}

/// Собирает подписанный бандл из уроков реестра (экспорт).
///
/// # Errors
/// Ошибка сериализации (на собственных типах практической причины нет).
pub fn build(lessons: &[Lesson], arch_be: &str) -> Result<LessonsBundle> {
    let mut bundled = Vec::with_capacity(lessons.len());
    for lesson in lessons {
        let mut evidence_sha256 = Vec::with_capacity(lesson.evidence.len());
        for evidence in &lesson.evidence {
            evidence_sha256.push(sha256_hex(canonical_json(evidence)?.as_bytes()));
        }
        bundled.push(BundledLesson {
            lesson: lesson.clone(),
            evidence_sha256,
        });
    }
    let lessons_only: Vec<Lesson> = bundled.iter().map(|b| b.lesson.clone()).collect();
    let signature = sha256_hex(canonical_json(&lessons_only)?.as_bytes());
    Ok(LessonsBundle {
        format: BUNDLE_FORMAT.to_string(),
        v: BUNDLE_VERSION,
        arch_be: arch_be.to_string(),
        count: bundled.len(),
        lessons: bundled,
        signature,
    })
}

/// Проверяет целостность бандла: маркер формата, подпись и хэши доказательств.
///
/// # Errors
/// [`HarnessError::Control`] с причиной отказа — подпись/хэш не совпали либо
/// маркер формата не `lessons-bundle/v1`.
pub fn verify(bundle: &LessonsBundle) -> Result<()> {
    if bundle.format != BUNDLE_FORMAT {
        return Err(HarnessError::Control(format!(
            "реестр уроков: бандл формата '{}' не является {BUNDLE_FORMAT}",
            bundle.format
        )));
    }
    let lessons: Vec<Lesson> = bundle.lessons.iter().map(|b| b.lesson.clone()).collect();
    let expected = sha256_hex(canonical_json(&lessons)?.as_bytes());
    if expected != bundle.signature {
        return Err(HarnessError::Control(
            "реестр уроков: подпись бандла не совпала — данные изменены после экспорта".to_string(),
        ));
    }
    for bundled in &bundle.lessons {
        if bundled.evidence_sha256.len() != bundled.lesson.evidence.len() {
            return Err(HarnessError::Control(format!(
                "реестр уроков: урок {} — число хэшей доказательств не совпадает \
                 с числом доказательств",
                bundled.lesson.id
            )));
        }
        for (index, evidence) in bundled.lesson.evidence.iter().enumerate() {
            let actual = sha256_hex(canonical_json(evidence)?.as_bytes());
            if bundled.evidence_sha256[index] != actual {
                return Err(HarnessError::Control(format!(
                    "реестр уроков: урок {} — хэш доказательства {index} не совпал \
                     (evidence изменено после экспорта)",
                    bundled.lesson.id
                )));
            }
        }
    }
    Ok(())
}

/// Извлекает уроки из проверенного бандла (для записи через [`super::ledger::Ledger::adopt`]).
#[must_use]
pub fn lessons_of(bundle: &LessonsBundle) -> Vec<Lesson> {
    bundle
        .lessons
        .iter()
        .map(|bundled| bundled.lesson.clone())
        .collect()
}

/// Каноническая JSON-сериализация (детерминированная для фиксированных типов).
fn canonical_json<T: Serialize>(value: &T) -> Result<String> {
    serde_json::to_string(value).map_err(HarnessError::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lessons::ledger::{
        Evidence, Ledger, LessonSource, Outcome, RecordInput, Signature, Thresholds,
    };
    use crate::lessons::scope::{Scope, ScopeLevel};

    fn evidence(run: &str, outcome: Outcome) -> Evidence {
        Evidence {
            run_id: run.to_string(),
            harness: Some("claude-code".to_string()),
            verdict_excerpt: format!("[{run}] вердикт"),
            file: Some("src/lib.rs".to_string()),
            line: Some(12),
            ts: format!("2026-10-14T10:00:{:02}Z", run.len()),
            outcome,
            task: Some(run.to_string()),
        }
    }

    fn confirmed_lesson() -> Lesson {
        let th = Thresholds::default();
        let dir = tempfile::tempdir().expect("tempdir");
        let ledger = Ledger::at(dir.path().join("ledger.jsonl"));
        // 1 FAIL + 2 PASS в двух разных задачах → confirmed.
        ledger
            .record(
                &RecordInput::new(
                    Signature::new(LessonSource::Gate, "G5"),
                    evidence("t1", Outcome::Fail),
                )
                .with_statement("новый публичный символ требует __all__")
                .with_scope(Scope::new(
                    ScopeLevel::Module,
                    vec!["paylib/**".to_string()],
                )),
                &th,
                "2026-10-14",
            )
            .expect("record fail");
        ledger
            .record(
                &RecordInput::new(
                    Signature::new(LessonSource::Gate, "G5"),
                    evidence("t2", Outcome::PassAfterApply),
                ),
                &th,
                "2026-10-14",
            )
            .expect("hit 1");
        let confirmed = ledger
            .record(
                &RecordInput::new(
                    Signature::new(LessonSource::Gate, "G5"),
                    evidence("t3", Outcome::PassAfterApply),
                ),
                &th,
                "2026-10-14",
            )
            .expect("hit 2");
        assert_eq!(
            confirmed.lesson.status,
            crate::lessons::LessonStatus::Confirmed
        );
        confirmed.lesson
    }

    #[test]
    fn build_then_verify_roundtrips() {
        let lesson = confirmed_lesson();
        let bundle = build(&[lesson], "0.5.0").expect("build");
        assert_eq!(bundle.format, BUNDLE_FORMAT);
        assert_eq!(bundle.v, BUNDLE_VERSION);
        assert_eq!(bundle.count, 1);
        assert_eq!(bundle.signature.len(), 64, "подпись — полный SHA-256");
        assert_eq!(bundle.lessons[0].evidence_sha256.len(), 3);
        verify(&bundle).expect("свежий бандл проходит проверку");
    }

    #[test]
    fn tampered_evidence_hash_is_rejected() {
        let lesson = confirmed_lesson();
        let mut bundle = build(&[lesson], "0.5.0").expect("build");
        // Меняем поле хэша доказательства, не трогая ни тело evidence, ни
        // подпись, — проверка обязана поймать расхождение именно хэша.
        let hash = bundle.lessons[0].evidence_sha256[0].clone();
        let mut chars: Vec<char> = hash.chars().collect();
        chars[0] = if chars[0] == '0' { '1' } else { '0' };
        bundle.lessons[0].evidence_sha256[0] = chars.into_iter().collect();
        let err = verify(&bundle).expect_err("битый хэш отклонён");
        assert!(err.to_string().contains("хэш доказательства"), "{err}");
    }

    #[test]
    fn tampered_statement_is_rejected_by_the_signature() {
        let lesson = confirmed_lesson();
        let mut bundle = build(&[lesson], "0.5.0").expect("build");
        // Меняем тело урока, не пересчитывая подпись.
        bundle.lessons[0].lesson.statement = "чужое правило".to_string();
        let err = verify(&bundle).expect_err("подмена тела отклонена");
        assert!(err.to_string().contains("подпись бандла"), "{err}");
    }

    #[test]
    fn wrong_format_marker_is_rejected() {
        let lesson = confirmed_lesson();
        let mut bundle = build(&[lesson], "0.5.0").expect("build");
        bundle.format = "lessons-bundle/v0".to_string();
        let err = verify(&bundle).expect_err("чужой формат отклонён");
        assert!(err.to_string().contains("не является"), "{err}");
    }

    #[test]
    fn lessons_are_extracted_in_order() {
        let a = confirmed_lesson();
        let mut b = confirmed_lesson();
        b.id = "LSN-87654321".to_string();
        let bundle = build(&[a.clone(), b.clone()], "0.5.0").expect("build");
        let extracted = lessons_of(&bundle);
        assert_eq!(extracted.len(), 2);
        assert_eq!(extracted[0].id, a.id);
        assert_eq!(extracted[1].id, b.id);
    }
}
