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
/// включая ноль сегментов: `**/*.rs` матчит и `main.rs`).
///
/// `pub(crate)`: разделяется с аудитом флота (`crate::fleet`, флаг `--include`).
pub(crate) fn glob_matches(pattern: &str, path: &str) -> bool {
    let pat: Vec<&str> = pattern.split('/').collect();
    let parts: Vec<&str> = path.split('/').collect();
    match_glob_segments(&pat, &parts)
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
