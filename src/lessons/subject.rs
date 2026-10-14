//! Классификатор `subject_kind` (L6, ADR-088): детерминированный вид субъекта
//! урока по диффу исполнителя.
//!
//! # Зачем
//!
//! Композиция (L6, [`super::compose`]) сворачивает уроки, которые проваливались
//! по одному `subject_kind` («новый публичный символ», «новая миграция», …).
//! Чтобы этот признак не проставляла LLM (AD-2: детерминированный контур без
//! LLM), вид субъекта выводится из **диффа исполнителя** — набора изменённых
//! файлов с добавленными строками. Тот же набор фактов, что питает
//! `recover`/`arch_diff` (ADR-063/069): перечень файлов ревизии и их содержимое.
//!
//! # Классификация
//!
//! Чистая функция по одному файлу диффа ([`classify_path`]) и по списку
//! ([`classify`]). Порядок распознавания фиксирован (специфичное раньше
//! общего):
//! 1. миграция/SQL — путь под `migrations/` или суффикс `.sql`;
//! 2. новый маршрут — добавленные строки с маркером маршрута;
//! 3. новый публичный символ — добавленные строки с маркером публичности;
//! 4. новый файл — файл добавлен, других признаков нет.
//!
//! Классификатор эвристичен на границах (переименования, сгенерированный код):
//! ошибка даёт **ложный отрицательный** (уроки не складываются), а не ложный
//! положительный — приемлемо (ADR-088, Negative).

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::error::HarnessError;

/// Маркеры публичного символа в добавленных строках (Rust `pub`, Python
/// `def`/`class`, JS/TS `export`).
const SYMBOL_MARKERS: &[&str] = &["pub ", "def ", "class ", "export "];

/// Маркеры нового маршрута/эндпоинта в добавленных строках.
const ROUTE_MARKERS: &[&str] = &[
    ".route(",
    "@app.",
    "@router.",
    "@blueprint.",
    "endpoint",
    "#[get(",
    "#[post(",
    "#[put(",
    "#[delete(",
    "#[patch(",
    "openapi",
];

/// Суффиксы файлов SQL-миграций.
const SQL_SUFFIXES: &[&str] = &[".sql"];

/// Каталоги миграций в пути (достаточно подстроки в сегменте).
const MIGRATION_SEGMENTS: &[&str] = &["migrations", "migration"];

/// Вид субъекта урока (ADR-088) — канонический признак в подписи.
///
/// Канон строк: `new_file`, `new_public_symbol`, `new_route`, `new_migration`.
/// `new_migration` покрывает «новая SQL-миграция» (ADR-088 §3 «новый SQL»),
/// `new_route` — «новый эндпоинт» (ADR-088 §2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubjectKind {
    /// Новый файл в пакете.
    NewFile,
    /// Новый публичный символ (функция/структура/экспорт).
    NewPublicSymbol,
    /// Новый маршрут/эндпоинт.
    NewRoute,
    /// Новая миграция (SQL-миграция).
    NewMigration,
}

impl SubjectKind {
    /// Все виды в каноническом порядке — для справок и тестов.
    pub const ALL: [SubjectKind; 4] = [
        SubjectKind::NewFile,
        SubjectKind::NewPublicSymbol,
        SubjectKind::NewRoute,
        SubjectKind::NewMigration,
    ];

    /// Каноническая строка вида (она же — форма в JSON/подписи урока).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NewFile => "new_file",
            Self::NewPublicSymbol => "new_public_symbol",
            Self::NewRoute => "new_route",
            Self::NewMigration => "new_migration",
        }
    }
}

impl fmt::Display for SubjectKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for SubjectKind {
    type Err = HarnessError;

    /// Разбор канонического имени вида (регистр не важен).
    ///
    /// # Errors
    /// Неизвестное имя — [`HarnessError::Config`] с перечнем допустимых.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "new_file" => Ok(Self::NewFile),
            "new_public_symbol" => Ok(Self::NewPublicSymbol),
            "new_route" => Ok(Self::NewRoute),
            "new_migration" | "new_sql" => Ok(Self::NewMigration),
            other => Err(HarnessError::Config(format!(
                "реестр уроков: неизвестный вид субъекта '{other}' \
                 (допустимо: new_file, new_public_symbol, new_route, new_migration)"
            ))),
        }
    }
}

/// Один изменённый файл диффа исполнителя (источник фактов классификатора).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffFile {
    /// Путь файла относительно корня репозитория.
    pub path: String,
    /// Файл добавлен (новый), а не изменён.
    pub added: bool,
    /// Добавленные строки (для распознавания символов/маршрутов); `None` —
    /// содержимое недоступно, тогда остаётся только признак «новый файл».
    pub added_lines: Option<String>,
}

/// Классифицирует один путь диффа в вид субъекта.
///
/// `added` различает «новый файл» от «изменённого»; `added_lines` позволяют
/// распознать публичный символ или маршрут в изменённом файле (а не только в
/// новом). Порядок: миграция → маршрут → символ → новый файл.
#[must_use]
pub fn classify_path(path: &str, added: bool, added_lines: Option<&str>) -> Option<SubjectKind> {
    let normalized = path.trim().trim_start_matches("./").replace('\\', "/");
    if is_migration(&normalized) {
        return Some(SubjectKind::NewMigration);
    }
    if let Some(lines) = added_lines {
        if ROUTE_MARKERS.iter().any(|m| lines.contains(m)) {
            return Some(SubjectKind::NewRoute);
        }
        if SYMBOL_MARKERS.iter().any(|m| lines.contains(m)) {
            return Some(SubjectKind::NewPublicSymbol);
        }
    }
    if added {
        return Some(SubjectKind::NewFile);
    }
    None
}

/// Классифицирует дифф целиком: множество видов субъекта, затронутых
/// изменением (отсортировано канонически, без дублей — детерминизм).
#[must_use]
pub fn classify(diff: &[DiffFile]) -> Vec<SubjectKind> {
    let mut kinds: std::collections::BTreeSet<SubjectKind> = std::collections::BTreeSet::new();
    for file in diff {
        if let Some(kind) = classify_path(&file.path, file.added, file.added_lines.as_deref()) {
            kinds.insert(kind);
        }
    }
    kinds.into_iter().collect()
}

/// Путь похож на SQL-миграцию: сегмент `migrations`/`migration` либо `.sql`.
fn is_migration(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower
        .split('/')
        .any(|seg| MIGRATION_SEGMENTS.contains(&seg))
        || SQL_SUFFIXES.iter().any(|s| lower.ends_with(s))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn diff(path: &str, added: bool, lines: Option<&str>) -> DiffFile {
        DiffFile {
            path: path.to_string(),
            added,
            added_lines: lines.map(str::to_string),
        }
    }

    #[test]
    fn subject_kind_roundtrip_and_aliases() {
        for kind in SubjectKind::ALL {
            let parsed: SubjectKind = kind.as_str().parse().expect("разбор вида");
            assert_eq!(parsed, kind, "канон: {}", kind.as_str());
            assert_eq!(kind.to_string(), kind.as_str());
        }
        // Синоним «новый SQL» (ADR-088 §3) читается как new_migration.
        assert_eq!(
            "new_sql".parse::<SubjectKind>().expect("синоним"),
            SubjectKind::NewMigration
        );
        let err = "wat".parse::<SubjectKind>().expect_err("отказ");
        assert!(
            err.to_string().contains("неизвестный вид субъекта"),
            "{err}"
        );
    }

    /// Табличный тест классификатора: путь + добавленные строки → вид.
    #[test]
    fn classify_path_table() {
        type Case<'a> = (&'a str, bool, Option<&'a str>, Option<SubjectKind>, &'a str);
        let cases: &[Case] = &[
            // Миграции — по пути, независимо от строк.
            (
                "db/migrations/001_init.sql",
                true,
                None,
                Some(SubjectKind::NewMigration),
                "sql-суффикс",
            ),
            (
                "db/migration/V2__add.sql",
                false,
                None,
                Some(SubjectKind::NewMigration),
                "сегмент migration",
            ),
            (
                "src/lessons/subject.rs",
                false,
                None,
                None,
                "изменённый файл без строк",
            ),
            // Новый маршрут — по добавленным строкам.
            (
                "src/routes/users.rs",
                true,
                Some("@router.get(\"/users\", get_users)"),
                Some(SubjectKind::NewRoute),
                "fastapi",
            ),
            (
                "src/api.rs",
                true,
                Some("#[get(\"/health\")]\npub async fn health"),
                Some(SubjectKind::NewRoute),
                "actix",
            ),
            // Новый публичный символ — по добавленным строкам.
            (
                "src/lib.rs",
                true,
                Some("pub fn to_money(x: Decimal) -> Money"),
                Some(SubjectKind::NewPublicSymbol),
                "rust pub fn",
            ),
            (
                "paylib/money.py",
                true,
                Some("def round_money(x): ..."),
                Some(SubjectKind::NewPublicSymbol),
                "python def",
            ),
            // Новый файл — без других признаков.
            (
                "src/lessons/compose.rs",
                true,
                None,
                Some(SubjectKind::NewFile),
                "новый файл без строк",
            ),
            (
                "src/lessons/compose.rs",
                true,
                Some("// просто комментарий"),
                Some(SubjectKind::NewFile),
                "строки без маркеров",
            ),
        ];
        for (path, added, lines, want, why) in cases {
            assert_eq!(
                classify_path(path, *added, *lines),
                *want,
                "{path} (+added={added}): {why}"
            );
        }
    }

    /// Специфичное раньше общего: маршрут с `pub async fn` — маршрут, а не
    /// публичный символ (маркер маршрута проверяется первым).
    #[test]
    fn route_wins_over_public_symbol() {
        let kind = classify_path(
            "src/api.rs",
            true,
            Some("pub async fn health() { /* .route(\"/health\", get(health)) */ }"),
        );
        assert_eq!(kind, Some(SubjectKind::NewRoute), "маршрут специфичнее");
    }

    /// Дифф целиком: множество видов, отсортированное канонически, без дублей.
    #[test]
    fn classify_batch_is_sorted_and_deduplicated() {
        let files = vec![
            diff("src/a.rs", true, Some("pub fn a() {}")),
            diff("src/b.py", true, Some("def b(): pass")),
            diff("db/migrations/1.sql", true, None),
            diff("src/c.rs", true, Some("pub fn c() {}")), // дубль по виду
        ];
        assert_eq!(
            classify(&files),
            vec![SubjectKind::NewPublicSymbol, SubjectKind::NewMigration,],
            "виды отсортированы и дедуплицированы"
        );
        assert_eq!(classify(&[]), Vec::<SubjectKind>::new(), "пустой дифф");
    }
}
