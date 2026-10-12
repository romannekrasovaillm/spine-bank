//! Glob-обход репозитория (волна B1, декомпозиция `exec.rs` под C-33): сбор
//! файлов и каталогов ([`collect_files`], [`collect_dirs`]) и матчинг путей
//! ([`glob_matches`], [`segment_matches`]).

use std::path::{Path, PathBuf};

use walkdir::WalkDir;

use crate::error::{HarnessError, Result};

/// Собирает файлы репозитория по простому glob-шаблону (`**` — любая глубина,
/// `*` — внутри сегмента, `?` — один символ). Возвращает (относительный путь,
/// абсолютный путь), отсортированные по относительному пути.
///
/// Служебные и производные каталоги исключены всегда: `.git`, `target`,
/// `node_modules`, `dist`, `__pycache__`, `.next`, `.pytest_cache` и
/// `.arch-handoff` — fitness-правила целятся в АРТЕФАКТЫ РЕАЛИЗАЦИИ, а не в
/// документы решения: пакет handoff содержит текст spine/TASK.md, и правило
/// `must_not_contain` срабатывало на собственные цитаты контракта (кейс 1).
///
/// `pub(crate)`: разделяется с измерением зубьев (`crate::control::teeth`, B1).
pub(crate) fn collect_files(repo: &Path, glob: &str) -> Result<Vec<(String, PathBuf)>> {
    const SKIP: [&str; 8] = [
        ".git",
        "target",
        "node_modules",
        "dist",
        "__pycache__",
        ".next",
        ".pytest_cache",
        ".arch-handoff",
    ];
    let mut out = Vec::new();
    let walker = WalkDir::new(repo).follow_links(false).into_iter();
    for entry in walker.filter_entry(|e| {
        let name = e.file_name().to_string_lossy();
        !(e.file_type().is_dir() && SKIP.contains(&name.as_ref()))
    }) {
        let entry = entry.map_err(|e| {
            HarnessError::Control(format!("обход репозитория {}: {e}", repo.display()))
        })?;
        if !entry.file_type().is_file() {
            continue;
        }
        let rel = entry.path().strip_prefix(repo).map_err(|e| {
            HarnessError::Control(format!(
                "относительный путь {}: {e}",
                entry.path().display()
            ))
        })?;
        let rel_str = rel.to_string_lossy().replace('\\', "/");
        if glob_matches(glob, &rel_str) {
            out.push((rel_str, entry.path().to_path_buf()));
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

/// Собирает КАТАЛОГИ репозитория по glob-шаблону (для `dir_must_have_file`).
/// Возвращает относительные пути каталогов (с `/`-разделителями),
/// отсортированные. Служебные каталоги исключены тем же списком, что и в
/// [`collect_files`]; корень репозитория в выборку не входит.
///
/// `pub(crate)`: разделяется с измерением зубьев (`crate::control::teeth`, B1).
pub(crate) fn collect_dirs(repo: &Path, glob: &str) -> Result<Vec<String>> {
    const SKIP: [&str; 8] = [
        ".git",
        "target",
        "node_modules",
        "dist",
        "__pycache__",
        ".next",
        ".pytest_cache",
        ".arch-handoff",
    ];
    let mut out = Vec::new();
    let walker = WalkDir::new(repo).follow_links(false).into_iter();
    for entry in walker.filter_entry(|e| {
        let name = e.file_name().to_string_lossy();
        !(e.file_type().is_dir() && SKIP.contains(&name.as_ref()))
    }) {
        let entry = entry.map_err(|e| {
            HarnessError::Control(format!("обход репозитория {}: {e}", repo.display()))
        })?;
        if !entry.file_type().is_dir() || entry.path() == repo {
            continue;
        }
        let rel = entry.path().strip_prefix(repo).map_err(|e| {
            HarnessError::Control(format!(
                "относительный путь {}: {e}",
                entry.path().display()
            ))
        })?;
        let rel_str = rel.to_string_lossy().replace('\\', "/");
        if glob_matches(glob, &rel_str) {
            out.push(rel_str);
        }
    }
    out.sort();
    Ok(out)
}

/// Матч одного сегмента пути по glob-шаблону (`*` — любые символы, `?` — один).
pub(super) fn segment_matches(pattern: &str, name: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let n: Vec<char> = name.chars().collect();
    let (mut pi, mut ni) = (0usize, 0usize);
    let mut star: Option<(usize, usize)> = None; // (позиция '*' в шаблоне, позиция в имени)
    while ni < n.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == n[ni]) {
            pi += 1;
            ni += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some((pi, ni));
            pi += 1;
        } else if let Some((sp, sn)) = star {
            pi = sp + 1;
            ni = sn + 1;
            star = Some((sp, sn + 1));
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

/// Матч относительного пути по glob-шаблону с поддержкой `**` (любая глубина,
/// включая ноль сегментов: `**/*.rs` матчит и `main.rs`) и фигурных скобок
/// `{a,b,c}` (brace expansion, как в bash — см. [`expand_braces`]).
///
/// `pub(crate)`: разделяется с аудитом флота (`crate::fleet`, флаг `--include`).
pub(crate) fn glob_matches(pattern: &str, path: &str) -> bool {
    expand_braces(pattern)
        .iter()
        .any(|variant| glob_matches_literal(variant, path))
}

/// Матч пути по УЖЕ раскрытому шаблону (без `{}`).
fn glob_matches_literal(pattern: &str, path: &str) -> bool {
    let pat: Vec<&str> = pattern.split('/').collect();
    let parts: Vec<&str> = path.split('/').collect();
    match_glob_segments(&pat, &parts)
}

/// Раскрывает фигурные скобки glob-шаблона в список альтернатив (brace
/// expansion, как в bash): `**/*.{java,yml,yaml}` → `**/*.java`, `**/*.yml`,
/// `**/*.yaml`. Вложенные скобки дают декартово произведение (`{a,{b,c}}` →
/// `a`, `b`, `c`), `{}` без запятой верхнего уровня не раскрывается (`{x}`
/// остаётся литералом), незакрытая `{` — тоже литерал.
///
/// Без `{}` возвращает единственный вариант, равный входу (поведение
/// байт-в-байт прежнее).
pub(crate) fn expand_braces(pattern: &str) -> Vec<String> {
    let Some(open) = pattern.find('{') else {
        return vec![pattern.to_string()];
    };
    // Парная `}` для первой `{` с учётом вложенности.
    let mut depth = 0usize;
    let mut close = None;
    for (i, ch) in pattern.char_indices().skip_while(|(i, _)| *i < open) {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    close = Some(i);
                    break;
                }
            }
            _ => {}
        }
    }
    let Some(close) = close else {
        // Незакрытая `{` — литерал: фиксируем её и раскрываем остаток.
        let head = &pattern[..=open];
        return expand_braces(&pattern[open + 1..])
            .into_iter()
            .map(|tail| format!("{head}{tail}"))
            .collect();
    };
    let parts = split_top_level_commas(&pattern[open + 1..close]);
    if parts.len() < 2 {
        // Запятой верхнего уровня нет — эти скобки не раскрываются (bash):
        // оставляем `{...}` литералом и раскрываем только остаток.
        let head = &pattern[..=close];
        return expand_braces(&pattern[close + 1..])
            .into_iter()
            .map(|tail| format!("{head}{tail}"))
            .collect();
    }
    let prefix = &pattern[..open];
    let suffix = &pattern[close + 1..];
    let mut out = Vec::new();
    for part in parts {
        out.extend(expand_braces(&format!("{prefix}{part}{suffix}")));
    }
    out
}

/// Делит тело `{...}` на альтернативы по запятым ВЕРХНЕГО уровня (запятые
/// внутри вложенных `{}` не разделяют).
fn split_top_level_commas(body: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut depth = 0usize;
    let mut start = 0usize;
    for (i, ch) in body.char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                parts.push(&body[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    parts.push(&body[start..]);
    parts
}

fn match_glob_segments(pat: &[&str], parts: &[&str]) -> bool {
    if pat.is_empty() {
        return parts.is_empty();
    }
    if pat[0] == "**" {
        return (0..=parts.len()).any(|skip| match_glob_segments(&pat[1..], &parts[skip..]));
    }
    if parts.is_empty() {
        return false;
    }
    segment_matches(pat[0], parts[0]) && match_glob_segments(&pat[1..], &parts[1..])
}
