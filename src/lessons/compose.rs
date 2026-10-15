//! Композиция уроков в составные чек-листы `LSN-C-*` (L6, ADR-088; E4).
//!
//! # Правило (ADR-088)
//!
//! Уроки, которые ≥ [`COMPOSE_MIN_FAILS`] раз проваливались в одной задаче или
//! по одному `subject_kind` («новый публичный символ», «новая миграция», …),
//! сворачиваются в составной чек-лист `LSN-C-*` со ссылками на исходные.
//! Урок с `misses > 0` в композицию не входит (начал мешать — TASK §7.2).
//!
//! # Границы (AD-2)
//!
//! Композит — **производная** сущность: он вычисляется из проекции реестра, не
//! пишется в журнал отдельной записью, поэтому удаление композита не трогает
//! исходные уроки (ADR-088, Reversibility). Статус композита — не выше
//! минимального из исходных (жизненный цикл не «уболтать»). Формулировка
//! `statement` детерминированна; LLM может её заменить текстом с пометкой
//! `[inferred]`, но **применимость** композита решает только счётчик исходных
//! уроков (отбор L4 по их области/BM25), а не текст.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use crate::hash::sha256_hex;

use super::{Evidence, Lesson, LessonStatus, Outcome, Scope, ScopeLevel};

/// Префикс идентификатора составного урока.
pub const COMPOSITE_PREFIX: &str = "LSN-C-";

/// Минимальное число наблюдённых FAIL, чтобы урок вошёл в композицию
/// («≥ 2 раз проваливался»).
pub const COMPOSITE_MIN_FAILS: u32 = 2;

/// Минимальный размер группы: два и больше уроков складываются в чек-лист.
pub const COMPOSITE_MIN_GROUP: usize = 2;

/// Составной чек-лист: производная сущность из ≥ 2 уроков.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Composite {
    /// `LSN-C-<sha8 отсортированных id исходных>`.
    pub id: String,
    /// Id исходных уроков (отсортированы, без дублей).
    pub sources: Vec<String>,
    /// Общий вид субъекта, если все исходные разделяют один (иначе `None`).
    pub subject_kind: Option<String>,
    /// Детерминированная формулировка чек-листа (может быть заменена LLM на
    /// `[inferred]`; применимость от текста не зависит).
    pub statement: String,
    /// Статус композита — не выше минимального из исходных.
    pub status: LessonStatus,
    /// Область композита — самая широкая из исходных (объединение глобов).
    pub scope: Scope,
}

/// Сворачивает уроки в составные чек-листы. Чистая функция: один и тот же
/// вход даёт тот же порядок композитов байт-в-байт (детерминизм).
///
/// В композицию входят уроки с `misses == 0` и `fails ≥ COMPOSE_MIN_FAILS`.
/// Группы образуются связностью по двум признакам (объединение):
/// 1. общий `subject_kind`;
/// 2. общий FAIL-задача (совпадение ключа задачи в доказательствах FAIL).
///
/// Каждая связная группа размера ≥ [`COMPOSITE_MIN_GROUP`] даёт один композит.
#[must_use]
pub fn compose(lessons: &[Lesson]) -> Vec<Composite> {
    let eligible: Vec<&Lesson> = lessons
        .iter()
        .filter(|l| l.misses == 0 && l.fails >= COMPOSITE_MIN_FAILS)
        .collect();

    let groups = connected_groups(&eligible);
    let mut composites: Vec<Composite> = groups
        .into_iter()
        .filter(|group| group.len() >= COMPOSITE_MIN_GROUP)
        .map(|group| build_composite(&group))
        .collect();
    composites.sort_by(|a, b| a.id.cmp(&b.id));
    composites
}

/// Стабильный id составного урока: `LSN-C-<sha8>`, где хэш — от отсортированных
/// id исходных, склеенных unit separator (защита от склейки `a`+`b` ≠ `ab`).
/// Сортировка внутри — id не зависит от порядка передачи исходных.
#[must_use]
pub fn composite_id(source_ids: &[String]) -> String {
    let mut sorted: Vec<String> = source_ids.to_vec();
    sorted.sort();
    sorted.dedup();
    let joined = sorted.join("\u{1f}");
    let hex = sha256_hex(joined.as_bytes());
    let head: String = hex.chars().take(8).collect();
    format!("{COMPOSITE_PREFIX}{head}")
}

/// Связные группы уроков по общему `subject_kind` или общей FAIL-задаче
/// (union-find по парам, детерминированный порядок входа).
fn connected_groups<'a>(lessons: &[&'a Lesson]) -> Vec<Vec<&'a Lesson>> {
    let n = lessons.len();
    let mut parent: Vec<usize> = (0..n).collect();
    for i in 0..n {
        for j in (i + 1)..n {
            if same_subject(lessons[i], lessons[j]) || share_fail_task(lessons[i], lessons[j]) {
                union(&mut parent, i, j);
            }
        }
    }
    let mut by_root: BTreeMap<usize, Vec<&Lesson>> = BTreeMap::new();
    for (idx, lesson) in lessons.iter().enumerate() {
        by_root
            .entry(find(&mut parent, idx))
            .or_default()
            .push(*lesson);
    }
    by_root.into_values().collect()
}

fn same_subject(a: &Lesson, b: &Lesson) -> bool {
    match (&a.signature.subject_kind, &b.signature.subject_kind) {
        (Some(x), Some(y)) => x == y,
        _ => false,
    }
}

/// Общая FAIL-задача: пересечение ключей задач у доказательств с исходом FAIL.
fn share_fail_task(a: &Lesson, b: &Lesson) -> bool {
    let ta: BTreeSet<&str> = fail_tasks(a);
    let tb: BTreeSet<&str> = fail_tasks(b);
    ta.iter().any(|task| tb.contains(task))
}

fn fail_tasks(lesson: &Lesson) -> BTreeSet<&str> {
    lesson
        .evidence
        .iter()
        .filter(|e| e.outcome == Outcome::Fail)
        .map(Evidence::task_key)
        .collect()
}

fn find(parent: &mut [usize], i: usize) -> usize {
    if parent[i] != i {
        parent[i] = find(parent, parent[i]);
    }
    parent[i]
}

fn union(parent: &mut [usize], a: usize, b: usize) {
    let (ra, rb) = (find(parent, a), find(parent, b));
    if ra != rb {
        parent[ra] = rb;
    }
}

/// Собирает композит из группы уроков.
///
/// Группа приходит в порядке union-find (зависит от порядка входа), поэтому
/// здесь она сортируется по `id` — иначе `statement` (перечисление исходных)
/// менялся бы от порядка входа и ломал детерминизм.
fn build_composite(group: &[&Lesson]) -> Composite {
    let mut sorted: Vec<&Lesson> = group.to_vec();
    sorted.sort_by(|a, b| a.id.cmp(&b.id));
    let group = sorted.as_slice();

    let mut ids: Vec<String> = group.iter().map(|l| l.id.clone()).collect();
    ids.dedup();
    let id = composite_id(&ids);

    let subject_kind = common_subject(group).map(str::to_string);

    let status = group
        .iter()
        .map(|l| l.status)
        .min()
        .unwrap_or(LessonStatus::Observed);

    let scope = widest_scope(group);

    let statement = checklist(group);

    Composite {
        id,
        sources: ids,
        subject_kind,
        statement,
        status,
        scope,
    }
}

/// Общий `subject_kind`, если он один на всю группу; иначе `None`.
fn common_subject<'a>(group: &[&'a Lesson]) -> Option<&'a str> {
    let first = group.first()?.signature.subject_kind.as_deref()?;
    group
        .iter()
        .all(|l| l.signature.subject_kind.as_deref() == Some(first))
        .then_some(first)
}

/// Самая широкая область группы: максимум уровня; у широких уровней (`repo` и
/// шире) глобы не нужны, у узких — объединение глобов всех исходных.
fn widest_scope(group: &[&Lesson]) -> Scope {
    let level = group
        .iter()
        .map(|l| l.scope.level)
        .max()
        .unwrap_or(ScopeLevel::Repo);
    if level.is_wide() {
        return Scope::new(level, Vec::<String>::new());
    }
    let mut globs: Vec<String> = group
        .iter()
        .flat_map(|l| l.scope.globs.iter().cloned())
        .collect();
    globs.sort();
    globs.dedup();
    Scope::new(level, globs)
}

/// Детерминированная формулировка чек-листа: перечисление исходных уроков.
fn checklist(group: &[&Lesson]) -> String {
    let mut out = format!("Составной чек-лист из {} уроков:", group.len());
    for lesson in group {
        let text = if lesson.statement.trim().is_empty() {
            lesson.signature.describe()
        } else {
            lesson.statement.trim().to_string()
        };
        let _ = write!(out, "\n- {} — {text}", lesson.id);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lessons::{Evidence, LessonSource, RecordInput, ScopeLevel, Signature, Thresholds};

    /// Урок для теста: подпись (код, опционально `subject_kind`), счётчики,
    /// доказательства по файлам/задачам.
    fn lesson(
        code: &str,
        subject: Option<&str>,
        fails: u32,
        misses: u32,
        tasks: &[&str],
        file: &str,
    ) -> Lesson {
        let mut signature = Signature::new(LessonSource::Gate, code);
        if let Some(kind) = subject {
            signature = signature.with_subject(kind);
        }
        let evidence: Vec<Evidence> = tasks
            .iter()
            .map(|task| Evidence {
                run_id: format!("run-{task}"),
                harness: Some("claude-code".to_string()),
                verdict_excerpt: format!("[{code}] нарушение"),
                file: Some(file.to_string()),
                line: Some(1),
                ts: "2026-10-14T10:00:00.000Z".to_string(),
                outcome: Outcome::Fail,
                task: Some((*task).to_string()),
            })
            .collect();
        let input = RecordInput::new(signature, evidence[0].clone());
        let mut lesson = Lesson::from_input(&input);
        lesson.evidence = evidence;
        lesson.fails = fails;
        lesson.misses = misses;
        lesson.status = lesson.derived_status(&Thresholds::default());
        lesson
    }

    /// Составной id детерминирован: порядок исходных не влияет на id.
    #[test]
    fn composite_id_is_order_independent() {
        let a = composite_id(&["LSN-aaaa".to_string(), "LSN-bbbb".to_string()]);
        let b = composite_id(&["LSN-bbbb".to_string(), "LSN-aaaa".to_string()]);
        assert_eq!(a, b, "id — от отсортированных исходных");
        assert!(a.starts_with(COMPOSITE_PREFIX));
        assert_eq!(a.len(), COMPOSITE_PREFIX.len() + 8);
    }

    /// Композиция по общему `subject_kind`: два урока про «новый публичный
    /// символ» складываются в один чек-лист.
    #[test]
    fn compose_groups_by_subject_kind() {
        let a = lesson(
            "G5",
            Some("new_public_symbol"),
            2,
            0,
            &["T1", "T2"],
            "src/a.rs",
        );
        let b = lesson(
            "G6",
            Some("new_public_symbol"),
            2,
            0,
            &["T3", "T4"],
            "src/b.rs",
        );
        let a_id = a.id.clone();
        let b_id = b.id.clone();
        let composites = compose(&[a, b]);
        assert_eq!(composites.len(), 1, "{composites:?}");
        let c = &composites[0];
        assert_eq!(c.sources.len(), 2);
        assert_eq!(c.subject_kind.as_deref(), Some("new_public_symbol"));
        assert!(
            c.statement.contains("Составной чек-лист из 2 уроков"),
            "{c:?}"
        );
        assert!(
            c.statement.contains(&a_id) && c.statement.contains(&b_id),
            "{c:?}"
        );
    }

    /// Композиция по общей FAIL-задаче: два урока провалились в одной задаче.
    #[test]
    fn compose_groups_by_shared_fail_task() {
        let a = lesson("G5", None, 2, 0, &["T1", "T2"], "src/a.rs");
        let b = lesson("G7", None, 2, 0, &["T1", "T3"], "src/b.rs");
        let composites = compose(&[a, b]);
        assert_eq!(composites.len(), 1, "{composites:?}");
        assert_eq!(composites[0].subject_kind, None, "нет общего subject_kind");
        assert_eq!(composites[0].sources.len(), 2);
    }

    /// Урок с misses > 0 не входит в композицию; с fails < 2 — тоже.
    #[test]
    fn noisy_and_rarely_failing_lessons_are_excluded() {
        let clean = lesson("G5", Some("new_route"), 2, 0, &["T1", "T2"], "src/a.rs");
        let noisy = lesson(
            "G6",
            Some("new_route"),
            3,
            1,
            &["T1", "T2", "T3"],
            "src/b.rs",
        );
        let rare = lesson("G7", Some("new_route"), 1, 0, &["T1"], "src/c.rs");
        let composites = compose(&[clean, noisy, rare]);
        assert_eq!(
            composites.len(),
            0,
            "группы из ≥2 чистых уроков нет: {composites:?}"
        );
    }

    /// Статус композита не выше минимального из исходных.
    #[test]
    fn composite_status_is_min_of_sources() {
        let mut confirmed = lesson("G5", Some("new_route"), 2, 0, &["T1", "T2"], "src/a.rs");
        confirmed.status = LessonStatus::Confirmed;
        let candidate = lesson("G6", Some("new_route"), 2, 0, &["T3", "T4"], "src/b.rs");
        // candidate — уже по derived_status (fails ≥ 2).
        let composites = compose(&[confirmed, candidate]);
        assert_eq!(composites.len(), 1);
        assert_eq!(
            composites[0].status,
            LessonStatus::Candidate,
            "мин(confirmed, candidate) = candidate"
        );
    }

    /// Детерминизм: порядок входа не влияет на порядок и состав композитов.
    #[test]
    fn composition_is_deterministic_on_reorder() {
        let a = lesson(
            "G5",
            Some("new_public_symbol"),
            2,
            0,
            &["T1", "T2"],
            "src/a.rs",
        );
        let b = lesson(
            "G6",
            Some("new_public_symbol"),
            2,
            0,
            &["T3", "T4"],
            "src/b.rs",
        );
        let c = lesson(
            "G7",
            Some("new_migration"),
            2,
            0,
            &["T5", "T6"],
            "db/m/1.sql",
        );
        let d = lesson(
            "G8",
            Some("new_migration"),
            2,
            0,
            &["T7", "T8"],
            "db/m/2.sql",
        );
        let first = compose(&[a.clone(), b.clone(), c.clone(), d.clone()]);
        let second = compose(&[d.clone(), c.clone(), b.clone(), a.clone()]);
        assert_eq!(first, second, "порядок входа не влияет");
        assert_eq!(first.len(), 2, "две группы: symbol и migration");
    }

    /// Область композита — объединение глобов исходных на самой широкой из
    /// уровней (узкие уровни не «склеивают» глобы до repo).
    #[test]
    fn composite_scope_unions_globs() {
        let mut a = lesson("G5", Some("new_route"), 2, 0, &["T1", "T2"], "src/a.rs");
        a.scope = Scope::new(ScopeLevel::Module, vec!["src/**".to_string()]);
        let mut b = lesson("G6", Some("new_route"), 2, 0, &["T3", "T4"], "src/b.rs");
        b.scope = Scope::new(ScopeLevel::Module, vec!["tests/**".to_string()]);
        let composites = compose(&[a, b]);
        assert_eq!(composites.len(), 1);
        assert_eq!(composites[0].scope.level, ScopeLevel::Module);
        assert_eq!(composites[0].scope.globs, vec!["src/**", "tests/**"]);
    }
}
