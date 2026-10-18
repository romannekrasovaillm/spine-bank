//! Детекторы fitness-правил в досье (ADR-093, волна 1): какие исходы правил
//! из `reports/detectors/rules/` судья видит рядом с субъектом.
//!
//! Проектные детекторы корня `reports/detectors/` входят в досье
//! `code_vs_spine` целиком (E7.1). Детекторы правил — во **все** виды досье,
//! но по относимости, иначе каждое досье несло бы весь реестр:
//!
//! 1. правило явно привязано к критерию рубрики (`bound_rules`, волна 2);
//! 2. находки правила лежат в файле субъекта;
//! 3. правило охраняет инвариант (`ad:`) или само является ссылкой досье
//!    (карточка правила в `entity_links`).
//!
//! Отбор детерминирован и не зависит от модели: тот же репозиторий даёт то же
//! досье, а значит тот же `pack_sha256` у хоста и у гейта.

use std::collections::BTreeSet;
use std::path::Path;

use super::{InputRole, Source, relative};
use crate::control::{RULE_DETECTORS_DIR, RuleDetector, RuleOutcome};
use crate::error::Result;

/// Потолок числа детекторов правил в одном досье: лимит досье общий для всех
/// источников, и измерения не должны вытеснять субъект.
pub const MAX_RULE_DETECTORS_PER_PACK: usize = 12;

/// Почему детектор попал в досье — порядок важности при отсечении потолком.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Relevance {
    /// Правило привязано к критерию рубрики явно.
    Bound,
    /// Находки правила — в файле субъекта.
    SubjectFile,
    /// Правило — ссылка досье или охраняет инвариант из ссылок досье.
    Reference,
}

/// Порядок исхода при отсечении: красное измерение важнее зелёного.
fn outcome_rank(detector: &RuleDetector) -> u8 {
    match detector.outcome() {
        Some(RuleOutcome::Fail) => 0,
        Some(RuleOutcome::Warn) => 1,
        Some(RuleOutcome::Skip | RuleOutcome::Na | RuleOutcome::Overridden) => 2,
        Some(RuleOutcome::Pass) => 3,
        None => 4,
    }
}

/// Путь субъекта без диапазона строк фрагмента (`src/a.rs#10-20` → `src/a.rs`).
fn subject_path(subject: &str) -> &str {
    subject.split_once('#').map_or(subject, |(p, _)| p).trim()
}

/// Детекторы правил, относящиеся к досье, как источники роли `detector`.
///
/// `bound` — идентификаторы или имена правил, явно привязанных к критериям
/// рубрики: они попадают в досье всегда, даже без совпадения по файлу или
/// инварианту (связка объявлена человеком, а не выведена).
///
/// # Errors
/// Файл детектора не читается.
pub(super) fn select(
    repo: &Path,
    sources: &[Source],
    subject: &str,
    bound: &[String],
) -> Result<Vec<Source>> {
    let dir = repo.join(RULE_DETECTORS_DIR);
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let subject = subject_path(subject);
    let reference_ids: BTreeSet<&str> = sources
        .iter()
        .filter(|s| s.role == InputRole::Reference)
        .filter_map(|s| s.id.as_deref())
        .collect();
    let mut picked: Vec<(Relevance, u8, String, std::path::PathBuf, RuleDetector)> = Vec::new();
    for (path, detector) in crate::control::load_rule_detectors(repo) {
        let relevance = if bound.iter().any(|b| detector.answers_to(b)) {
            Some(Relevance::Bound)
        } else if !subject.is_empty() && detector.files.iter().any(|f| f == subject) {
            Some(Relevance::SubjectFile)
        } else if reference_ids.contains(detector.name.as_str())
            || reference_ids.contains(detector.rule.as_str())
            || detector
                .ad
                .as_deref()
                .is_some_and(|ad| reference_ids.contains(ad))
        {
            Some(Relevance::Reference)
        } else {
            None
        };
        if let Some(relevance) = relevance {
            let rank = outcome_rank(&detector);
            picked.push((relevance, rank, detector.name.clone(), path, detector));
        }
    }
    picked.sort_by(|a, b| (a.0, a.1, &a.2).cmp(&(b.0, b.1, &b.2)));
    picked.truncate(MAX_RULE_DETECTORS_PER_PACK);
    // В досье — в стабильном порядке имён: от отсечения зависит только состав.
    picked.sort_by(|a, b| a.2.cmp(&b.2));
    let mut out = Vec::with_capacity(picked.len());
    for (_, _, _, path, detector) in picked {
        let text =
            std::fs::read_to_string(&path).map_err(|e| crate::error::HarnessError::io(&path, e))?;
        out.push(Source::detector(
            relative(repo, &path),
            text.trim_end().to_string(),
            Some(detector.name.clone()),
            Some(detector.status.clone()),
        ));
    }
    Ok(out)
}
