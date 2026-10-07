//! Извлечение импортов из исходных файлов (ADR-029) и разрешение импорта во
//! владеющий контекст (`code_roots`, ADR-030) — общий код движка
//! fitness-правил (`control::exec`: `dependency_direction`,
//! `context_boundary`) и графа «как построено» (`arch_diff`, волна K).
//!
//! Модуль выделен из `control/exec.rs` чистым перемещением: семантика
//! извлечения и разрешения не изменена; единственное расширение — разбор
//! импортов Go (`.go`), ранее возвращавший пустой список (нужен графу
//! as-built: детерминированный обход PR на Go-монорепозиториях).
//!
//! Детерминизм (правило 10 волны K): все функции — чистые над переданным
//! содержимым; файловая система входит только через явные параметры
//! (`exists`-замыкание, [`candidate_bases`]).

use std::collections::BTreeSet;
use std::path::Path;

use regex::Regex;

use crate::error::{HarnessError, Result};

/// Извлечённый импорт: модуль в координатах `/` и номер строки (1-based).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ImportEdge {
    /// Модульный путь (`crate::a::b` → `a/b`; `a.b` → `a/b`; Go — как есть).
    pub module: String,
    /// Номер строки в файле (1-based).
    pub line: usize,
}

/// Расширения файлов, из которых извлекаются импорты (единый список для
/// `control::exec` и `arch_diff`).
pub const IMPORT_EXTENSIONS: [&str; 10] = [
    "rs", "py", "java", "kt", "ts", "tsx", "js", "jsx", "mjs", "go",
];

/// Извлекает импорты из исходного файла по его расширению (ADR-029).
///
/// Поддерживаемые формы:
/// - Rust (`.rs`): `use crate::…` и инлайн-пути `crate::…::` (`::` → `/`);
///   внешние крейты (`use std::…`) не извлекаются;
/// - Python (`.py`): `import a.b`, `from a.b import …` (`.` → `/`);
/// - Java/Kotlin (`.java`, `.kt`): `import a.b.C;` (`.` → `/`);
/// - TS/JS (`.ts`, `.tsx`, `.js`, `.jsx`, `.mjs`): `from '…'`, `import '…'`,
///   `require('…')`; относительные пути (`./…`) сохраняются как есть —
///   их разрешает `context_boundary`, а `dependency_direction` пропускает;
/// - Go (`.go`): `import "path"`, `import alias "path"` и блочная форма
///   `import ( … )`; путь сохраняется как есть (в Go он уже с `/`).
///
/// Строки-комментарии (`//`, `///`, `//!`, `#`) игнорируются; блочные
/// комментарии и строковые литералы не разбираются — эвристика
/// документированно приблизительна (ложное срабатывание возможно на
/// `crate::…` внутри строки). Для файлов неподдерживаемых расширений
/// возвращается пустой список.
///
/// # Errors
/// Внутренний regex не компилируется (инвариант кода; практически
/// недостижимо — паттерны константны).
pub fn extract_imports(rel: &str, content: &str) -> Result<Vec<ImportEdge>> {
    let ext = Path::new(rel)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default();
    let comment_prefix = match ext {
        "rs" | "java" | "kt" | "ts" | "tsx" | "js" | "jsx" | "mjs" | "go" => "//",
        "py" => "#",
        _ => return Ok(Vec::new()),
    };
    let compile = |pat: &str| {
        Regex::new(pat)
            .map_err(|e| HarnessError::Control(format!("внутренний regex импортов '{pat}': {e}")))
    };
    let mut out = Vec::new();
    // Состояние блочной формы Go: между `import (` и `)` каждая строка с
    // путём в кавычках — импорт.
    let mut go_import_block = false;
    for (idx, line) in content.lines().enumerate() {
        if line.trim_start().starts_with(comment_prefix) {
            continue;
        }
        let lineno = idx + 1;
        let mut push = |module: String, line: usize| out.push(ImportEdge { module, line });
        match ext {
            "rs" => {
                let re =
                    compile(r"\bcrate::([A-Za-z_][A-Za-z0-9_]*(?:::[A-Za-z_][A-Za-z0-9_]*)*)")?;
                for c in re.captures_iter(line) {
                    push(c[1].replace("::", "/"), lineno);
                }
            }
            "py" => {
                let re_import = compile(r"^\s*import\s+([A-Za-z_][\w.]*)")?;
                let re_from = compile(r"^\s*from\s+([A-Za-z_][\w.]*)\s+import\b")?;
                for c in re_import
                    .captures(line)
                    .into_iter()
                    .chain(re_from.captures(line))
                {
                    push(c[1].replace('.', "/"), lineno);
                }
            }
            "java" | "kt" => {
                let re = compile(r"^\s*import\s+(?:static\s+)?([A-Za-z_][\w.]*)\s*;")?;
                if let Some(c) = re.captures(line) {
                    push(c[1].replace('.', "/"), lineno);
                }
            }
            "go" => {
                let trimmed = line.trim_start();
                if go_import_block {
                    if trimmed.starts_with(')') {
                        go_import_block = false;
                        continue;
                    }
                    // Строка блока: `"path"` или `alias "path"`.
                    let re = compile(r#"^\s*(?:[A-Za-z_][\w.]*\s+)?"([^"]+)""#)?;
                    if let Some(c) = re.captures(line) {
                        push(c[1].to_string(), lineno);
                    }
                    continue;
                }
                if trimmed.starts_with("import") {
                    let rest = trimmed.trim_start_matches("import").trim_start();
                    if rest.starts_with('(') {
                        // `import (` — открытие блока (путь на той же строке
                        // после скобки — экзотика, документированно не
                        // разбирается).
                        go_import_block = true;
                        continue;
                    }
                    // Одиночный импорт: `import "path"` / `import a "path"`.
                    let re = compile(r#"^\s*import\s+(?:[A-Za-z_][\w.]*\s+)?"([^"]+)""#)?;
                    if let Some(c) = re.captures(line) {
                        push(c[1].to_string(), lineno);
                    }
                }
            }
            _ => {
                // ts/tsx/js/jsx/mjs: путь сохраняется сырым (включая `./…`).
                let re_from = compile(r#"\bfrom\s+['"]([^'"]+)['"]"#)?;
                let re_import = compile(r#"^\s*import\s+['"]([^'"]+)['"]"#)?;
                let re_require = compile(r#"\brequire\(\s*['"]([^'"]+)['"]\s*\)"#)?;
                for c in re_from
                    .captures_iter(line)
                    .chain(re_import.captures_iter(line))
                    .chain(re_require.captures_iter(line))
                {
                    push(c[1].to_string(), lineno);
                }
            }
        }
    }
    Ok(out)
}

/// Совпадение модуля с записью списка по префиксу пути с границей сегмента:
/// `agent/slash` совпадает с `agent`, `agentworld` — нет.
#[must_use]
pub fn module_prefix_match(module: &str, entry: &str) -> bool {
    module == entry
        || module
            .strip_prefix(entry)
            .is_some_and(|rest| rest.starts_with('/'))
}

/// Нормализует `code_root`: срезает пробелы, ведущий `./` и хвостовые `/`.
#[must_use]
pub fn normalize_root(root: &str) -> String {
    root.trim()
        .trim_start_matches("./")
        .trim_end_matches('/')
        .to_string()
}

/// Нормализует относительный путь: раскрывает `.` и `..` по сегментам.
#[must_use]
pub fn normalize_rel_path(path: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    for seg in path.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                out.pop();
            }
            s => out.push(s),
        }
    }
    out.join("/")
}

/// Контекст (владелец), которому принадлежит путь `path` по префиксу корней.
///
/// Контексты — пары `(владелец, нормализованные корни)`: у `control::exec`
/// владелец — сущность модели, у `arch_diff` — узел графа; обобщение по `C`
/// позволяет не дублировать обход (K1).
#[must_use]
pub fn owner_by_path<'c, C>(contexts: &'c [(C, Vec<String>)], path: &str) -> Option<&'c C> {
    contexts
        .iter()
        .find(|(_, roots)| roots.iter().any(|r| module_prefix_match(path, r)))
        .map(|(e, _)| e)
}

/// Разрешает импорт `module` (координаты `/`) во владеющий им контекст.
///
/// Порядок: TS-относительный путь — от каталога файла; прямое префиксное
/// совпадение с корнями; разрешение модуля в существующий файл (базы
/// [`candidate_bases`] + модульные суффиксы [`candidate_paths`]); проверка
/// существования — через `exists` (рабочее дерево у `control::exec`, снимок
/// ревизии у `arch_diff`). `None` — внешняя или неразрешённая зависимость.
#[must_use]
pub fn resolve_import_owner<'c, C>(
    contexts: &'c [(C, Vec<String>)],
    bases: &[String],
    from_rel: &str,
    module: &str,
    exists: &dyn Fn(&str) -> bool,
) -> Option<&'c C> {
    if module.starts_with('.') {
        // TS/JS-относительный импорт: `./foo`, `../bar` — от каталога файла.
        // Модуль передаётся в нормализацию целиком: `../../x` обязан подняться
        // на два уровня (ранняя версия срезала один `.` и теряла уровень).
        let dir = Path::new(from_rel)
            .parent()
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .unwrap_or_default();
        let resolved = normalize_rel_path(&format!("{dir}/{module}"));
        return owner_by_path(contexts, &resolved);
    }
    // Прямое совпадение в координатах импортов (Python/Java-пакеты).
    if let Some(owner) = owner_by_path(contexts, module) {
        return Some(owner);
    }
    // Разрешение модуля в файл (Rust `crate::…`, смешанные монорепо).
    // Путь импорта включает имя элемента (`beta/core/Engine`), поэтому
    // пробуем префиксы от длинного к короткому: `beta/core/Engine` →
    // `beta/core` → `beta`.
    let segments: Vec<&str> = module.split('/').collect();
    for len in (1..=segments.len()).rev() {
        let prefix = segments[..len].join("/");
        for base in bases {
            for cand in candidate_paths(base, &prefix) {
                if exists(&cand) {
                    return owner_by_path(contexts, &cand);
                }
            }
        }
    }
    None
}

/// Разрешение импорта против рабочего дерева (`exists` — `repo.join(p)`).
#[must_use]
pub fn resolve_import_owner_fs<'c, C>(
    repo: &Path,
    contexts: &'c [(C, Vec<String>)],
    bases: &[String],
    from_rel: &str,
    module: &str,
) -> Option<&'c C> {
    resolve_import_owner(contexts, bases, from_rel, module, &|p| {
        repo.join(p).is_file()
    })
}

/// Суффиксное совпадение импорта с корнем контекста (`module` оканчивается
/// на `/root` или равен ему): разрешение Go-пакетов монорепозитория
/// (`example.com/bank/services/ledger` → корень `services/ledger`).
/// `control::exec` его не применяет (семантика границ контекстов прежняя);
/// используется графом as-built (волна K), где важнее не потерять ребро.
#[must_use]
pub fn owner_by_suffix<'c, C>(contexts: &'c [(C, Vec<String>)], module: &str) -> Option<&'c C> {
    let mut best: Option<&'c C> = None;
    let mut best_len = 0usize;
    for (owner, roots) in contexts {
        for root in roots {
            let suffix_hit = !root.is_empty()
                && (module == root
                    || module
                        .strip_suffix(root.as_str())
                        .is_some_and(|head| head.ends_with('/')));
            // Наиболее длинный корень выигрывает: `services/ledger` сильнее
            // `services`, иначе владелец зависел бы от порядка контекстов.
            if suffix_hit && root.len() > best_len {
                best = Some(owner);
                best_len = root.len();
            }
        }
    }
    best
}

/// Статические базовые каталоги разрешения модуля в файл.
const STATIC_BASES: [&str; 4] = ["", "src/", "src/main/java/", "src/main/kotlin/"];

/// Базовые каталоги для разрешения модуля в файл: статические корни
/// исходников плюс крейты Rust-workspace (`crates/*`).
#[must_use]
pub fn candidate_bases_with_crates(crate_dirs: Vec<String>) -> Vec<String> {
    let mut bases: Vec<String> = STATIC_BASES.iter().map(|s| (*s).to_string()).collect();
    bases.extend(crate_dirs);
    bases
}

/// Базы против рабочего дерева: каталоги `crates/*` читаются с диска.
/// Отсутствующий `crates/` — не ошибка, просто нет дополнительных баз.
#[must_use]
pub fn candidate_bases(repo: &Path) -> Vec<String> {
    let mut crates = Vec::new();
    if let Ok(rd) = std::fs::read_dir(repo.join("crates")) {
        for entry in rd.flatten() {
            if entry.path().is_dir() {
                crates.push(format!("crates/{}/", entry.file_name().to_string_lossy()));
            }
        }
    }
    crates.sort(); // read_dir не гарантирует порядок — детерминизм явно
    candidate_bases_with_crates(crates)
}

/// Крейты `crates/<name>/` из списка путей снимка ревизии (детерминированно:
/// отсортированы, дубли слиты).
#[must_use]
pub fn crate_dirs_from_paths<'a>(paths: impl Iterator<Item = &'a str>) -> Vec<String> {
    paths
        .filter_map(|p| {
            let mut segs = p.split('/');
            match (segs.next(), segs.next()) {
                (Some("crates"), Some(name)) if !name.is_empty() => Some(format!("crates/{name}/")),
                _ => None,
            }
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// Кандидатные пути файла для модуля `module` под базой `base`.
#[must_use]
pub fn candidate_paths(base: &str, module: &str) -> Vec<String> {
    [
        "{m}.rs",
        "{m}/mod.rs",
        "{m}.py",
        "{m}/__init__.py",
        "{m}.java",
        "{m}.kt",
        "{m}.ts",
        "{m}/index.ts",
        "{m}.js",
        "{m}/index.js",
    ]
    .iter()
    .map(|pat| format!("{base}{}", pat.replace("{m}", module)))
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Импорты одного файла как (модуль, строка) — компактная форма для
    /// assertions.
    fn modules(rel: &str, content: &str) -> Vec<(String, usize)> {
        extract_imports(rel, content)
            .expect("извлечение")
            .into_iter()
            .map(|e| (e.module, e.line))
            .collect()
    }

    #[test]
    fn rust_crate_paths_extracted() {
        let content = "use crate::agent::slash;\nlet x = crate::hash::sha256_hex(b);\nuse std::fs;\n// crate::comment\n";
        let got = modules("src/main.rs", content);
        assert_eq!(
            got,
            vec![
                ("agent/slash".to_string(), 1),
                ("hash/sha256_hex".to_string(), 2),
            ]
        );
    }

    #[test]
    fn python_import_and_from_extracted() {
        let content = "import ledger.core\nfrom skeleton.ledger import client\n# import skip\n";
        let got = modules("skeleton/intake/writer.py", content);
        assert_eq!(
            got,
            vec![
                ("ledger/core".to_string(), 1),
                ("skeleton/ledger".to_string(), 2),
            ]
        );
    }

    #[test]
    fn java_kotlin_imports_extracted() {
        let got = modules(
            "src/main/java/ru/bank/Intake.java",
            "package ru.bank;\nimport ru.bank.ledger.Core;\nimport static ru.bank.Const.X;\n",
        );
        assert_eq!(
            got,
            vec![
                ("ru/bank/ledger/Core".to_string(), 2),
                ("ru/bank/Const/X".to_string(), 3),
            ]
        );
        // Kotlin: эвристика требует завершающий `;` (как в Java) — допустимый
        // стиль; без точки с запятой импорт документированно не извлекается.
        let kt = modules("src/Intake.kt", "import ru.bank.ledger.Core;\n");
        assert_eq!(kt, vec![("ru/bank/ledger/Core".to_string(), 1)]);
        let no_semicolon = modules("src/Intake.kt", "import ru.bank.ledger.Core\n");
        assert_eq!(no_semicolon, Vec::new(), "без ';' импорт не извлекается");
    }

    #[test]
    fn ts_js_forms_extracted_raw() {
        let got = modules(
            "web/app.ts",
            "import { x } from './ledger/client';\nimport 'polyfill';\nconst y = require(\"ledger/sdk\");\n",
        );
        assert_eq!(
            got,
            vec![
                ("./ledger/client".to_string(), 1),
                ("polyfill".to_string(), 2),
                ("ledger/sdk".to_string(), 3),
            ]
        );
    }

    /// Go — расширение волны K: одиночный, с алиасом и блочная форма;
    /// комментарии и строки вне import-блока не считаются.
    #[test]
    fn go_imports_single_alias_block() {
        let content = "package main\n\nimport \"fmt\"\nimport (\n\t\"os\"\n\tledger \"example.com/bank/ledger\"\n)\n\n// import \"skip\"\nfunc main() { fmt.Println(os.Args, ledger.X) }\n";
        let got = modules("services/intake/main.go", content);
        assert_eq!(
            got,
            vec![
                ("fmt".to_string(), 3),
                ("os".to_string(), 5),
                ("example.com/bank/ledger".to_string(), 6),
            ]
        );
    }

    #[test]
    fn unsupported_extension_is_empty() {
        assert_eq!(modules("doc.md", "import nothing"), Vec::new());
        assert_eq!(modules("data.yaml", "import: no"), Vec::new());
    }

    #[test]
    fn prefix_match_segment_boundary() {
        assert!(module_prefix_match("agent/slash", "agent"));
        assert!(module_prefix_match("agent", "agent"));
        assert!(!module_prefix_match("agentworld", "agent"));
        assert!(!module_prefix_match("ag", "agent"));
    }

    #[test]
    fn normalize_root_and_rel_path() {
        assert_eq!(normalize_root(" ./services/billing/ "), "services/billing");
        assert_eq!(normalize_rel_path("a/./b/../c"), "a/c");
        assert_eq!(normalize_rel_path("./x"), "x");
    }

    #[test]
    fn owner_resolution_prefers_direct_then_file() {
        let contexts = vec![
            ("A", vec!["services/intake".to_string()]),
            ("B", vec!["services/ledger".to_string()]),
        ];
        // Прямое префиксное совпадение (Python-координаты).
        let owner = resolve_import_owner(
            &contexts,
            &[],
            "services/intake/writer.py",
            "services/ledger/core",
            &|_| false,
        );
        assert_eq!(owner.copied(), Some("B"));
        // Разрешение в существующий файл под базой.
        let owner = resolve_import_owner(
            &contexts,
            &[],
            "services/intake/writer.py",
            "services/ledger/client",
            &|p| p == "services/ledger/client.py",
        );
        assert_eq!(owner.copied(), Some("B"));
        // Неразрешённый импорт — внешняя зависимость.
        let none = resolve_import_owner(
            &contexts,
            &[],
            "services/intake/writer.py",
            "redis",
            &|_| false,
        );
        assert_eq!(none, None);
        // TS-относительный путь — от каталога файла.
        let rel = resolve_import_owner(
            &contexts,
            &[],
            "services/intake/ui/app.ts",
            "../../ledger/sdk",
            &|_| false,
        );
        assert_eq!(rel.copied(), Some("B"));
    }

    #[test]
    fn suffix_owner_longest_root_wins() {
        let contexts = vec![
            ("A", vec!["services".to_string()]),
            ("B", vec!["services/ledger".to_string()]),
        ];
        let owner = owner_by_suffix(&contexts, "example.com/bank/services/ledger");
        assert_eq!(owner.copied(), Some("B"));
        assert_eq!(
            owner_by_suffix(&contexts, "example.com/x/services").copied(),
            Some("A")
        );
        assert_eq!(owner_by_suffix(&contexts, "example.com/other"), None);
    }

    #[test]
    fn crate_dirs_from_paths_sorted_dedup() {
        let paths = [
            "crates/b/src/lib.rs",
            "src/main.rs",
            "crates/a/src/lib.rs",
            "crates/b/Cargo.toml",
        ];
        assert_eq!(
            crate_dirs_from_paths(paths.into_iter()),
            vec!["crates/a/".to_string(), "crates/b/".to_string()]
        );
    }
}
