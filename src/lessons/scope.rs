//! Область действия урока: уровень (`file|module|package|repo|user|org`) и
//! глобы путей, к которым урок применим (ADR-085).
//!
//! Сопоставление глобов детерминированное и не касается файловой системы:
//! `**` — любая глубина (включая ноль сегментов), `*` — внутри одного сегмента,
//! `?` — один символ. Разделитель пути — `/`; ведущие `./` и пустые сегменты
//! отбрасываются, поэтому `./src/a.rs` и `src/a.rs` совпадают одинаково.
//!
//! Пустой список глобов читается по уровню: широкие уровни (`repo` и шире)
//! охватывают всё, узкие (`file`/`module`/`package`) без глоба не совпадают
//! ни с чем — неопределённая область не должна «прилипать» ко всем путям.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::error::HarnessError;

/// Уровень области действия — от узкого (файл) к широкому (организация).
///
/// Порядок вариантов задаёт ширину ([`ScopeLevel::rank`]): сравнение уровней
/// детерминировано и используется отбором уроков (L4) и расширением области
/// (L6).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum ScopeLevel {
    /// Один файл (смысл задаёт глоб).
    File,
    /// Модуль/каталог (смысл задаёт глоб).
    Module,
    /// Пакет: крейт, сервис (смысл задаёт глоб).
    Package,
    /// Весь репозиторий — уровень по умолчанию.
    #[default]
    Repo,
    /// Пользовательский уровень (`$ARCH_HOME/lessons`).
    User,
    /// Организация: общий реестр (ADR-091).
    Org,
}

impl ScopeLevel {
    /// Все уровни в порядке возрастания ширины — для справок и тестов.
    pub const ALL: [ScopeLevel; 6] = [
        ScopeLevel::File,
        ScopeLevel::Module,
        ScopeLevel::Package,
        ScopeLevel::Repo,
        ScopeLevel::User,
        ScopeLevel::Org,
    ];

    /// Каноническая строка уровня (она же — форма в JSON и TOML).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::File => "file",
            Self::Module => "module",
            Self::Package => "package",
            Self::Repo => "repo",
            Self::User => "user",
            Self::Org => "org",
        }
    }

    /// Ширина уровня: больше — шире область.
    #[must_use]
    pub fn rank(self) -> u8 {
        match self {
            Self::File => 0,
            Self::Module => 1,
            Self::Package => 2,
            Self::Repo => 3,
            Self::User => 4,
            Self::Org => 5,
        }
    }

    /// Широкий уровень: охватывает репозиторий и шире, глобы не обязательны.
    #[must_use]
    pub fn is_wide(self) -> bool {
        self.rank() >= Self::Repo.rank()
    }
}

impl fmt::Display for ScopeLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for ScopeLevel {
    type Err = HarnessError;

    /// Разбор имени уровня; регистр не важен (значения конфига/CLI/MCP).
    ///
    /// # Errors
    /// Неизвестное имя — [`HarnessError::Config`] с перечнем допустимых.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "file" => Ok(Self::File),
            "module" => Ok(Self::Module),
            "package" => Ok(Self::Package),
            "repo" => Ok(Self::Repo),
            "user" => Ok(Self::User),
            "org" => Ok(Self::Org),
            other => Err(HarnessError::Config(format!(
                "реестр уроков: неизвестный уровень области '{other}' \
                 (допустимо: file, module, package, repo, user, org)"
            ))),
        }
    }
}

/// Область действия урока: уровень плюс глобы путей.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Scope {
    /// Уровень области (по умолчанию — [`ScopeLevel::Repo`]).
    pub level: ScopeLevel,
    /// Глобы путей относительно корня репозитория.
    pub globs: Vec<String>,
}

impl Default for Scope {
    fn default() -> Self {
        Self {
            level: ScopeLevel::Repo,
            globs: Vec::new(),
        }
    }
}

impl Scope {
    /// Область всего репозитория (без глобов).
    #[must_use]
    pub fn repo() -> Self {
        Self::default()
    }

    /// Область с уровня и глобов; глобы нормализуются (обрезка, сортировка,
    /// дедупликация) — байт-в-байт стабильный рендер начинается отсюда.
    #[must_use]
    pub fn new(level: ScopeLevel, globs: impl IntoIterator<Item = impl Into<String>>) -> Self {
        let mut globs: Vec<String> = globs
            .into_iter()
            .map(Into::into)
            .map(|g| g.trim().trim_start_matches("./").replace('\\', "/"))
            .filter(|g| !g.is_empty())
            .collect();
        globs.sort();
        globs.dedup();
        Self { level, globs }
    }

    /// Применим ли урок к пути (пути — относительно корня, разделитель `/`).
    #[must_use]
    pub fn matches_path(&self, path: &str) -> bool {
        let path = path.trim().trim_start_matches("./").replace('\\', "/");
        if self.globs.is_empty() {
            // Узкий уровень без глоба не сопоставим ни с чем: неизвестно, к
            // какому файлу/модулю относится урок, — «прилипать» ко всему нельзя.
            return self.level.is_wide();
        }
        self.globs.iter().any(|g| glob_match(g, &path))
    }

    /// Короткое описание области (форма рендера, стабильна байт-в-байт).
    #[must_use]
    pub fn describe(&self) -> String {
        if self.globs.is_empty() {
            self.level.to_string()
        } else {
            format!("{} (globs: {})", self.level, self.globs.join(", "))
        }
    }

    /// Область задана содержательно, а не дефолтом «весь репозиторий без
    /// глобов».
    ///
    /// Нужна свёртке журнала: повторная запись без явной области не должна
    /// затирать уже вычисленную узкую область урока («повторился FAIL» — это
    /// доказательство, а не новая область действия).
    #[must_use]
    pub fn is_specified(&self) -> bool {
        !self.globs.is_empty() || self.level != ScopeLevel::Repo
    }
}

/// Сопоставление пути глобу с `**`/`*`/`?` (сегментная семантика, как у
/// `scripts/deps-check.py` и правил `dependency_direction`).
#[must_use]
pub fn glob_match(pattern: &str, path: &str) -> bool {
    let pat: Vec<&str> = pattern.split('/').filter(|s| !s.is_empty()).collect();
    let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    if pat.is_empty() {
        return parts.is_empty();
    }
    match_here(&pat, &parts)
}

fn match_here(pat: &[&str], parts: &[&str]) -> bool {
    match pat.first() {
        None => parts.is_empty(),
        Some(&"**") => (0..=parts.len()).any(|skip| match_here(&pat[1..], &parts[skip..])),
        Some(segment) => match parts.first() {
            Some(piece) => segment_match(segment, piece) && match_here(&pat[1..], &parts[1..]),
            None => false,
        },
    }
}

/// Сопоставление одного сегмента (`*` и `?` внутри сегмента).
fn segment_match(pattern: &str, text: &str) -> bool {
    let symbols: Vec<char> = pattern.chars().collect();
    let chars: Vec<char> = text.chars().collect();
    let (mut p, mut t) = (0, 0);
    let mut star: Option<usize> = None;
    let mut mark = 0;
    while t < chars.len() {
        if p < symbols.len() && (symbols[p] == '?' || symbols[p] == chars[t]) {
            p += 1;
            t += 1;
        } else if p < symbols.len() && symbols[p] == '*' {
            star = Some(p);
            p += 1;
            mark = t;
        } else if let Some(sp) = star {
            p = sp + 1;
            mark += 1;
            t = mark;
        } else {
            return false;
        }
    }
    while p < symbols.len() && symbols[p] == '*' {
        p += 1;
    }
    p == symbols.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn level_roundtrip_and_rank() {
        for level in ScopeLevel::ALL {
            let parsed: ScopeLevel = level.as_str().parse().expect("разбор уровня");
            assert_eq!(parsed, level, "каноническая строка: {}", level.as_str());
            assert_eq!(level.to_string(), level.as_str());
        }
        assert!(ScopeLevel::File.rank() < ScopeLevel::Repo.rank());
        assert!(ScopeLevel::Repo.rank() < ScopeLevel::Org.rank());
        assert!(!ScopeLevel::Module.is_wide());
        assert!(ScopeLevel::Repo.is_wide() && ScopeLevel::Org.is_wide());
    }

    #[test]
    fn unknown_level_is_rejected() {
        let err = "каталог".parse::<ScopeLevel>().expect_err("отказ");
        assert!(err.to_string().contains("неизвестный уровень"), "{err}");
    }

    #[test]
    fn glob_table() {
        // (глоб, путь, ожидание)
        let cases: &[(&str, &str, bool)] = &[
            ("src/**", "src/a.rs", true),
            ("src/**", "src/deep/nested/a.rs", true),
            ("src/**", "tests/a.rs", false),
            ("src/*.rs", "src/a.rs", true),
            ("src/*.rs", "src/deep/a.rs", false),
            ("**/*.sql", "db/migrations/001.sql", true),
            ("**/*.sql", "001.sql", true),
            ("src/lessons/*", "src/lessons/mod.rs", true),
            ("src/lessons/*", "src/lessons/sub/mod.rs", false),
            ("a/?c.rs", "a/bc.rs", true),
            ("a/?c.rs", "a/bbc.rs", false),
            ("src/**/mod.rs", "src/lessons/mod.rs", true),
            ("src/**/mod.rs", "src/mod.rs", true),
            ("src/**/mod.rs", "src/lessons/other.rs", false),
        ];
        for (pattern, path, want) in cases {
            assert_eq!(
                glob_match(pattern, path),
                *want,
                "глоб {pattern} против {path}"
            );
        }
    }

    #[test]
    fn narrow_level_without_globs_matches_nothing() {
        let file_scope = Scope::new(ScopeLevel::File, Vec::<String>::new());
        assert!(!file_scope.matches_path("src/a.rs"));
        let module_scope = Scope::new(ScopeLevel::Module, Vec::<String>::new());
        assert!(!module_scope.matches_path("src/a.rs"));
        let repo = Scope::repo();
        assert!(repo.matches_path("anything/at/all.rs"));
    }

    #[test]
    fn globs_are_normalized_and_paths_are_normalized() {
        let scope = Scope::new(
            ScopeLevel::Module,
            vec![
                " src/b/** ".to_string(),
                "./src/a/**".to_string(),
                "src/a/**".into(),
            ],
        );
        assert_eq!(
            scope.globs,
            vec!["src/a/**", "src/b/**"],
            "сортировка+дедуп"
        );
        assert!(scope.matches_path("./src/a/x.rs"));
        assert!(scope.matches_path("src\\b\\y.rs"), "обратные слэши");
        assert!(!scope.matches_path("docs/x.md"));
        assert_eq!(scope.describe(), "module (globs: src/a/**, src/b/**)");
        assert_eq!(Scope::repo().describe(), "repo");
    }

    #[test]
    fn only_specified_scopes_count_as_refinements() {
        assert!(!Scope::repo().is_specified(), "дефолт — не уточнение");
        assert!(!Scope::new(ScopeLevel::Repo, Vec::<String>::new()).is_specified());
        assert!(Scope::new(ScopeLevel::Repo, vec!["src/**".to_string()]).is_specified());
        assert!(Scope::new(ScopeLevel::User, Vec::<String>::new()).is_specified());
    }

    #[test]
    fn scope_serialization_is_stable() {
        let scope = Scope::new(ScopeLevel::Package, vec!["paylib/**".to_string()]);
        let json = serde_json::to_string(&scope).expect("serde");
        assert_eq!(
            json, r#"{"level":"package","globs":["paylib/**"]}"#,
            "форма JSON закреплена: контракт реестра"
        );
        let back: Scope = serde_json::from_str(&json).expect("serde");
        assert_eq!(back, scope);
    }
}
