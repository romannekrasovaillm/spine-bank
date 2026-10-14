//! Обобщение области урока (L6, ADR-088): детерминированное расширение
//! `scope` по распределению доказательств и откат по контрпримеру.
//!
//! # Правило (ADR-088)
//!
//! Урок с одной сигнатурой, подтверждённый в ≥ 2 разных модулях одного
//! пакета, поднимается до `package`; в ≥ 2 пакетах — до `repo`. Контрпример
//! (`miss` в новой области) откатывает область назад. Счётчики — в
//! `evidence`, переходы — только по ним (AD-2: LLM не участвует).
//!
//! # Геометрия путей
//!
//! «Пакет» и «модуль» выводятся из путей доказательств без чтения манифестов
//! (чистая функция, детерминизм, отсутствие обращения к файловой системе):
//! - **пакет** = первый сегмент пути (каталог верхнего уровня);
//! - **модуль** = родительский каталог файла (путь без последнего сегмента).
//!
//! Это честная эвристика: богатый разбор манифестов сборки даёт
//! `recover`/`arch_diff` (ADR-063/069) — компоненты по `Cargo.toml`/`pom.xml`
//! и т.п. Здесь нужна бесплатная, не-I/O детерминированная оценка для слоя
//! уроков; расхождение с манифестным разбором — ложный отрицательный
//! (обобщение медленнее), не ложный положительный.

use std::collections::BTreeSet;

use super::{Lesson, Scope, ScopeLevel};

/// Пороги расширения области (ADR-088: «≥ 2»).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GeneralizeThresholds {
    /// Разных модулей одного пакета до подъёма к `package`.
    pub modules: usize,
    /// Разных пакетов до подъёма к `repo`.
    pub packages: usize,
}

impl Default for GeneralizeThresholds {
    fn default() -> Self {
        Self {
            modules: 2,
            packages: 2,
        }
    }
}

/// Пакет пути — первый сегмент (каталог верхнего уровня). `None` — файл в
/// корне репозитория (пакета не имеет): сегмент один, значит это имя файла,
/// а не каталог.
#[must_use]
pub fn package_of(path: &str) -> Option<String> {
    let segments = split_path(path);
    if segments.len() < 2 {
        None
    } else {
        segments.into_iter().next()
    }
}

/// Модуль пути — родительский каталог (путь без последнего сегмента-файла).
/// Файл в корне — пустая строка (модуль-корень).
#[must_use]
pub fn module_of(path: &str) -> String {
    let segments = split_path(path);
    if segments.len() <= 1 {
        String::new()
    } else {
        segments[..segments.len() - 1].join("/")
    }
}

/// Расширяет область урока по распределению доказательств.
///
/// Ширина считается по файлам `evidence` (все исходы: и наблюдения FAIL, и
/// применения PASS — это и есть «подтверждённость» в разных местах). Без
/// файлов или при недостатке сигнала возвращает текущую область без изменений.
/// Функция только расширяет: уже широкий уровень не сужается.
#[must_use]
pub fn widen_scope(lesson: &Lesson, th: &GeneralizeThresholds) -> Scope {
    let files: Vec<&str> = lesson
        .evidence
        .iter()
        .filter_map(|e| e.file.as_deref())
        .collect();
    if files.is_empty() {
        return lesson.scope.clone();
    }

    let packages: BTreeSet<String> = files.iter().filter_map(|f| package_of(f)).collect();
    let modules: BTreeSet<String> = files.iter().map(|f| module_of(f)).collect();

    let target = if packages.len() >= th.packages {
        ScopeLevel::Repo
    } else if modules.len() >= th.modules {
        ScopeLevel::Package
    } else {
        return lesson.scope.clone();
    };

    // Обобщение только расширяет: не сужаем уже достигнутую ширину.
    if target.rank() <= lesson.scope.level.rank() {
        return lesson.scope.clone();
    }

    match target {
        ScopeLevel::Repo => Scope::repo(),
        ScopeLevel::Package => Scope::new(
            ScopeLevel::Package,
            packages.iter().map(|p| format!("{p}/**")),
        ),
        // `file`/`module` не являются целью расширения (правило поднимает к
        // `package`/`repo`), поэтому сюда не попадаем.
        _ => lesson.scope.clone(),
    }
}

/// Итоговая область урока: расширение [`widen_scope`], отменённое контрпримером.
///
/// Контрпример — `miss`, чей файл лежит **вне** записанной (узкой) области
/// урока: обобщение утверждало «работает шире», а промах в новой области это
/// опровергает — откат к записанной области. Промах внутри уже покрытой
/// области контрпримером не считается (обычный «не помогло здесь»).
#[must_use]
pub fn generalize_scope(lesson: &Lesson, th: &GeneralizeThresholds) -> Scope {
    let widened = widen_scope(lesson, th);
    if widened == lesson.scope {
        return widened;
    }
    let counterexample = lesson
        .miss_files
        .iter()
        .any(|file| !lesson.scope.matches_path(file));
    if counterexample {
        lesson.scope.clone()
    } else {
        widened
    }
}

/// Разбивает путь на непустые сегменты (нормализация `./` и обратных слэшей).
fn split_path(path: &str) -> Vec<String> {
    path.trim()
        .trim_start_matches("./")
        .replace('\\', "/")
        .split('/')
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lessons::{Evidence, LessonSource, Outcome, RecordInput, Signature, Thresholds};

    /// Урок с доказательствами по файлам (файлы задают геометрию областей).
    fn lesson_with_files(files: &[&str], misses: u32, miss_files: &[&str]) -> Lesson {
        let signature = Signature::new(LessonSource::Gate, "G5");
        let evidence: Vec<Evidence> = files
            .iter()
            .map(|f| Evidence {
                run_id: format!("run-{f}"),
                harness: Some("claude-code".to_string()),
                verdict_excerpt: format!("[{f}] нарушение"),
                file: Some((*f).to_string()),
                line: Some(1),
                ts: "2026-10-14T10:00:00.000Z".to_string(),
                outcome: Outcome::Fail,
                task: Some(format!("T-{f}")),
            })
            .collect();
        let input = RecordInput::new(signature, evidence[0].clone()).with_scope(Scope::new(
            ScopeLevel::Module,
            vec!["src/lessons/**".to_string()],
        ));
        let mut lesson = Lesson::from_input(&input);
        lesson.evidence = evidence;
        lesson.misses = misses;
        lesson.miss_files = miss_files.iter().map(|f| (*f).to_string()).collect();
        lesson.fails = u32::try_from(files.len()).unwrap_or(u32::MAX);
        lesson.status = lesson.derived_status(&Thresholds::default());
        lesson
    }

    #[test]
    fn path_geometry_package_and_module() {
        assert_eq!(package_of("src/lessons/ledger.rs"), Some("src".to_string()));
        assert_eq!(
            package_of("tests/lessons_ledger.rs"),
            Some("tests".to_string())
        );
        assert_eq!(package_of("lib.rs"), None, "файл в корне без пакета");
        assert_eq!(module_of("src/lessons/ledger.rs"), "src/lessons");
        assert_eq!(module_of("src/lessons/mod.rs"), "src/lessons");
        assert_eq!(module_of("src/lib.rs"), "src");
        assert_eq!(module_of("lib.rs"), "", "файл в корне — модуль-корень");
        assert_eq!(module_of("./paylib/money.py"), "paylib", "нормализация ./");
        assert_eq!(module_of("paylib\\money.py"), "paylib", "обратные слэши");
    }

    /// Табличный тест расширения: два модуля одного пакета → package;
    /// два пакета → repo; один модуль — без изменений.
    #[test]
    fn widen_scope_table() {
        let th = GeneralizeThresholds::default();
        let cases: &[(&[&str], ScopeLevel, &str)] = &[
            (
                &["src/lessons/ledger.rs", "src/lessons/match.rs"],
                ScopeLevel::Module,
                "один модуль — без расширения",
            ),
            (
                &["src/lessons/ledger.rs", "src/lib.rs"],
                ScopeLevel::Package,
                "два модуля одного пакета src",
            ),
            (
                &["src/lessons/ledger.rs", "tests/lessons_ledger.rs"],
                ScopeLevel::Repo,
                "два пакета (src, tests)",
            ),
            (
                &["paylib/money.py", "paylib/registry.py", "cli/main.py"],
                ScopeLevel::Repo,
                "два пакета (paylib, cli) при двух модулях в paylib",
            ),
        ];
        for (files, want_level, why) in cases {
            let lesson = lesson_with_files(files, 0, &[]);
            let scope = widen_scope(&lesson, &th);
            assert_eq!(scope.level, *want_level, "{why}: {scope:?}");
        }
    }

    /// Расширение не сужает: уже package-урок не «откатывается» к module при
    /// одном модуле доказательств.
    #[test]
    fn widen_never_narrows() {
        let th = GeneralizeThresholds::default();
        let mut lesson = lesson_with_files(&["src/lessons/ledger.rs"], 0, &[]);
        lesson.scope = Scope::new(ScopeLevel::Package, vec!["src/**".to_string()]);
        let scope = widen_scope(&lesson, &th);
        assert_eq!(scope.level, ScopeLevel::Package);
        assert_eq!(scope.globs, vec!["src/**".to_string()]);
    }

    /// Табличный тест отката: miss вне записанной области откатывает
    /// расширение; miss внутри — нет.
    #[test]
    fn generalize_scope_rollback_table() {
        type Case<'a> = (&'a [&'a str], u32, &'a [&'a str], ScopeLevel, &'a str);
        let th = GeneralizeThresholds::default();
        let wide_evidence = &["src/lessons/ledger.rs", "src/lib.rs"]; // → package
        let cases: &[Case] = &[
            (
                wide_evidence,
                0,
                &[],
                ScopeLevel::Package,
                "без промахов — расширяется",
            ),
            (
                wide_evidence,
                1,
                &["src/render.rs"],
                ScopeLevel::Module,
                "miss в новой области откатывает",
            ),
            (
                wide_evidence,
                1,
                &["src/lessons/other.rs"],
                ScopeLevel::Package,
                "miss внутри области не контрпример",
            ),
        ];
        for (files, misses, miss_files, want_level, why) in cases {
            let lesson = lesson_with_files(files, *misses, miss_files);
            assert_eq!(generalize_scope(&lesson, &th).level, *want_level, "{why}");
        }
    }

    /// Детерминизм: повторный вызов на одном уроке даёт ту же область, и
    /// расширение к `repo` даёт область без глобов.
    #[test]
    fn generalize_is_deterministic_and_repo_scope_is_clean() {
        let th = GeneralizeThresholds::default();
        let lesson = lesson_with_files(
            &["src/lessons/ledger.rs", "tests/lessons_ledger.rs"],
            0,
            &[],
        );
        let a = generalize_scope(&lesson, &th);
        let b = generalize_scope(&lesson, &th);
        assert_eq!(a, b, "обобщение детерминировано");
        assert_eq!(a, Scope::repo(), "repo — без глобов");
    }
}
