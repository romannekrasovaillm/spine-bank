//! Снимок ревизии git-репозитория в память: список файлов (`ls-tree`) и
//! содержимое нужных сканерам файлов (`git show`), без изменения рабочего
//! дерева (волна K, правило 10: по одной ревизии — байт-в-байт тот же граф).

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::process::{Command, Stdio};

use crate::error::{HarnessError, Result};

/// Потолок размера файла, читаемого в снимок (как у `survey`: regex-сканеры
/// ориентированы на тексты, гигабайтные дампы не нужны).
const MAX_FILE_BYTES: u64 = 1024 * 1024;

/// Потолок числа файлов, читаемых из ревизии (bounded-работа: по одному
/// `git show` на файл, как у ландшафт-диффа [`crate::landscape`]) — дефолт,
/// когда `[gate.arch_drift] max_files` не задан.
pub const MAX_SNAPSHOT_CONTENT_FILES: usize = 2000;

/// Пределы сканирования снимка (K6, ADR-046 Am.2): потолок числа читаемых
/// файлов и подстроки путей-исключений — из секции `[gate.arch_drift]`.
///
/// `ignore` применяется И к снимку ревизии, И к снимку рабочего дерева:
/// асимметрия база/голова породила бы ложные рёбра (компонент «исчезает» на
/// одной стороне диффа). Дефолт — прежнее поведение (лимит 2000, без
/// исключений), поэтому существующие вызовы не меняются.
#[derive(Debug, Clone)]
pub struct ScanLimits {
    /// Потолок числа файлов с содержимым.
    pub max_content_files: usize,
    /// Подстроки путей-исключений (пустая подстрока игнорируется).
    pub ignore: Vec<String>,
}

impl Default for ScanLimits {
    fn default() -> Self {
        Self {
            max_content_files: MAX_SNAPSHOT_CONTENT_FILES,
            ignore: Vec::new(),
        }
    }
}

impl ScanLimits {
    /// Путь исключён (содержит одну из непустых подстрок `ignore`).
    #[must_use]
    pub fn ignored(&self, path: &str) -> bool {
        self.ignore
            .iter()
            .any(|pat| !pat.is_empty() && path.contains(pat.as_str()))
    }
}

/// Снимок одной ревизии: пути всех файлов и содержимое текстовых файлов,
/// нужных сканерам (исходники импортов, конфиги, контракты, модель).
#[derive(Debug, Clone)]
pub struct Snapshot {
    /// Полный sha ревизии.
    pub rev: String,
    /// Все относительные пути файлов ревизии (отсортированы множеством).
    pub files: BTreeSet<String>,
    /// Содержимое отобранных текстовых файлов (UTF-8 с потерями).
    pub contents: BTreeMap<String, String>,
}

impl Snapshot {
    /// Содержимое файла снимка (если отобрано при построении).
    #[must_use]
    pub fn content(&self, path: &str) -> Option<&str> {
        self.contents.get(path).map(String::as_str)
    }

    /// Файл существует в ревизии (замыкание `exists` для `imports`).
    #[must_use]
    pub fn exists(&self, path: &str) -> bool {
        self.files.contains(path)
    }
}

/// Прогон git с читаемой ошибкой (первая строка stderr), stdout как String.
fn git_text(repo: &Path, args: &[&str]) -> Result<String> {
    let out = git_output(repo, args)?;
    Ok(String::from_utf8_lossy(&out).into_owned())
}

/// Прогон git с бинарным stdout.
fn git_output(repo: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| {
            HarnessError::Control(format!(
                "arch-diff: git недоступен ({e}) — снимок невозможен"
            ))
        })?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let reason = stderr
            .lines()
            .map(str::trim)
            .find(|l| !l.is_empty())
            .map_or(
                "git завершился с ошибкой без сообщения",
                |l| l.strip_prefix("fatal:").map_or(l, str::trim),
            );
        let detail: String = reason.chars().take(160).collect();
        return Err(HarnessError::Control(format!(
            "arch-diff: git {}: {detail}",
            args.join(" ")
        )));
    }
    Ok(out.stdout)
}

/// Разрешает ссылку (ветка/тег/sha) в полный sha коммита.
///
/// # Errors
/// Ссылка не разрешается в коммит или git недоступен/отказал.
pub fn resolve_rev(repo: &Path, rev: &str) -> Result<String> {
    let spec = format!("{}^{{commit}}", rev.trim());
    Ok(git_text(repo, &["rev-parse", "--verify", &spec])?
        .trim()
        .to_string())
}

/// Список файлов ревизии с размерами: `git ls-tree -r --long -z`
/// (путь отделён табуляцией, записи — NUL; блобы только: подмодули и
/// симлинки в граф не идут).
fn ls_tree(repo: &Path, sha: &str) -> Result<Vec<(String, u64)>> {
    let raw = git_output(repo, &["ls-tree", "-r", "--long", "-z", sha])?;
    let mut out = Vec::new();
    for rec in raw.split(|b| *b == 0) {
        if rec.is_empty() {
            continue;
        }
        let rec = String::from_utf8_lossy(rec);
        let Some((meta, path)) = rec.split_once('\t') else {
            continue;
        };
        let fields: Vec<&str> = meta.split_whitespace().collect();
        // <mode> <type> <oid> <size>; у деревьев размера нет — пропускаем.
        if fields.len() != 4 || fields[1] != "blob" {
            continue;
        }
        let Ok(size) = fields[3].parse::<u64>() else {
            continue;
        };
        out.push((path.to_string(), size));
    }
    out.sort();
    Ok(out)
}

/// Файл ревизии отбирается для чтения содержимого: исходники импортов,
/// конфиги, контрактные форматы, модель и реестр правил.
fn needs_content(path: &str) -> bool {
    if path == "CONSTRAINTS.yaml" || path == ".arch-handoff/CONSTRAINTS.yaml" {
        return true;
    }
    let name = path.rsplit('/').next().unwrap_or(path);
    let lower_name = name.to_ascii_lowercase();
    if lower_name.contains("openapi") || lower_name.contains("asyncapi") {
        return true;
    }
    if path.starts_with("model/")
        && Path::new(path)
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("md"))
    {
        return true;
    }
    let ext = Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    crate::imports::IMPORT_EXTENSIONS.contains(&ext.as_str())
        || crate::survey::CONFIG_EXTENSIONS.contains(&ext.as_str())
        || matches!(
            ext.as_str(),
            "ini" | "conf" | "config" | "env" | "proto" | "avsc" | "sql"
        )
}

/// Читает содержимое одного файла ревизии (`git show <sha>:<path>`).
fn show_file(repo: &Path, sha: &str, path: &str) -> Result<String> {
    let bytes = git_output(repo, &["show", &format!("{sha}:{path}")])?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// Строит снимок ревизии `rev` репозитория `repo` (рабочее дерево не
/// изменяется; вся работа — чтением из git) с дефолтными пределами.
///
/// # Errors
/// Не git-репозиторий, ссылка не разрешается в коммит, файлов для чтения
/// больше [`MAX_SNAPSHOT_CONTENT_FILES`].
pub fn snapshot_at(repo: &Path, rev: &str) -> Result<Snapshot> {
    snapshot_at_with(repo, rev, &ScanLimits::default())
}

/// [`snapshot_at`] с явными пределами сканирования (лимит + `ignore` из
/// `[gate.arch_drift]`, ADR-046 Am.2). `ignore` вычитает файлы и из списка, и
/// из содержимого — симметрично снимку рабочего дерева.
///
/// # Errors
/// Как у [`snapshot_at`]; число файлов с содержимым больше
/// `limits.max_content_files`.
pub fn snapshot_at_with(repo: &Path, rev: &str, limits: &ScanLimits) -> Result<Snapshot> {
    let sha = resolve_rev(repo, rev)?;
    let entries = ls_tree(repo, &sha)?;
    let files: BTreeSet<String> = entries
        .iter()
        .map(|(p, _)| p.clone())
        .filter(|p| !limits.ignored(p))
        .collect();
    let to_read: Vec<&String> = entries
        .iter()
        .filter(|(p, size)| *size <= MAX_FILE_BYTES && needs_content(p) && !limits.ignored(p))
        .map(|(p, _)| p)
        .collect();
    if to_read.len() > limits.max_content_files {
        return Err(HarnessError::Control(format!(
            "arch-diff: в ревизии {} более {} сканируемых файлов — снимок отклонён \
             (ограничение bounded-работы); поднимите [gate.arch_drift] max_files или \
             исключите пути через [gate.arch_drift] ignore",
            &sha[..sha.len().min(12)],
            limits.max_content_files
        )));
    }
    let mut contents = BTreeMap::new();
    for path in to_read {
        let text = show_file(repo, &sha, path)?;
        contents.insert(path.clone(), text);
    }
    Ok(Snapshot {
        rev: sha,
        files,
        contents,
    })
}

/// Метка «ревизии» снимка рабочего дерева (K6): голова диффа составляющей
/// `arch_drift` — не коммит, а текущее состояние файлов. Поле `rev` такого
/// снимка не sha: в аттестации и сравнениях ревизий участвовать не может.
pub const WORKTREE_REV: &str = "worktree";

/// Строит снимок РАБОЧЕГО ДЕРЕВА `repo` (K6): та же форма, что у
/// [`snapshot_at`], но источник — файловая система, поэтому видны
/// незакоммиченные правки (гейт проверяет именно их). Обход — тот же контур,
/// что у дрейфа модели: без dot-каталогов и служебных
/// [`crate::survey::SKIP_DIRS`]; `.arch-handoff/CONSTRAINTS.yaml`
/// дочитывается отдельно (реестр головы нужен материализации кейса), остальное
/// содержимое `.arch-handoff/` в снимок не идёт (технические файлы контура).
///
/// Детерминизм в рамках одного состояния дерева: множества отсортированы,
/// повторный прогон на неизменном дереве даёт тот же снимок.
///
/// # Errors
/// Каталог недоступен/не читается, файлов для чтения больше
/// [`MAX_SNAPSHOT_CONTENT_FILES`].
pub fn snapshot_worktree(repo: &Path) -> Result<Snapshot> {
    snapshot_worktree_with(repo, &ScanLimits::default())
}

/// [`snapshot_worktree`] с явными пределами сканирования (лимит + `ignore` из
/// `[gate.arch_drift]`, ADR-046 Am.2). `ignore` — подстроки относительных
/// путей: каталог под исключением не обходится, файл — не попадает ни в
/// список, ни в содержимое.
///
/// # Errors
/// Как у [`snapshot_worktree`]; число файлов с содержимым больше
/// `limits.max_content_files`.
pub fn snapshot_worktree_with(repo: &Path, limits: &ScanLimits) -> Result<Snapshot> {
    let mut files = BTreeSet::new();
    // (путь, размер) — как ls_tree: размер нужен отбору содержимого.
    let mut sized: BTreeMap<String, u64> = BTreeMap::new();
    let walker = walkdir::WalkDir::new(repo).follow_links(false).into_iter();
    for entry in walker.filter_entry(|e| {
        if let Ok(rel) = e.path().strip_prefix(repo) {
            let rel = rel.to_string_lossy().replace('\\', "/");
            if !rel.is_empty() && limits.ignored(&rel) {
                return false;
            }
        }
        if e.depth() > 0 && e.file_type().is_dir() {
            let name = e.file_name().to_string_lossy();
            !(name.starts_with('.') || crate::survey::SKIP_DIRS.contains(&name.as_ref()))
        } else {
            true
        }
    }) {
        let entry = entry.map_err(|e| {
            HarnessError::Control(format!("arch-diff: обход {}: {e}", repo.display()))
        })?;
        if !entry.file_type().is_file() {
            continue;
        }
        let rel = entry
            .path()
            .strip_prefix(repo)
            .map_err(|e| HarnessError::Control(format!("arch-diff: относительный путь: {e}")))?
            .to_string_lossy()
            .replace('\\', "/");
        let size = entry.metadata().map_or(0, |m| m.len());
        files.insert(rel.clone());
        sized.insert(rel, size);
    }
    // Реестр правил головы из `.arch-handoff/` (каталог исключён обходом,
    // но материализация кейса ждёт его содержимое — как у git-снимка).
    let handoff_constraints = repo.join(".arch-handoff/CONSTRAINTS.yaml");
    if let Ok(meta) = handoff_constraints.metadata() {
        if meta.is_file() {
            let rel = ".arch-handoff/CONSTRAINTS.yaml".to_string();
            files.insert(rel.clone());
            sized.insert(rel, meta.len());
        }
    }
    let to_read: Vec<&String> = sized
        .iter()
        .filter(|(p, size)| **size <= MAX_FILE_BYTES && needs_content(p))
        .map(|(p, _)| p)
        .collect();
    if to_read.len() > limits.max_content_files {
        return Err(HarnessError::Control(format!(
            "arch-diff: в рабочем дереве более {} сканируемых файлов — снимок отклонён \
             (ограничение bounded-работы); поднимите [gate.arch_drift] max_files или \
             исключите пути через [gate.arch_drift] ignore",
            limits.max_content_files
        )));
    }
    let mut contents = BTreeMap::new();
    for path in to_read {
        let text = std::fs::read_to_string(repo.join(path))
            .map_err(|e| HarnessError::io(repo.join(path), e))?;
        contents.insert(path.clone(), text);
    }
    Ok(Snapshot {
        rev: WORKTREE_REV.to_string(),
        files,
        contents,
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// git в каталоге с тестовой идентичностью (изоляция AD-7).
    pub(crate) fn git_in(dir: &Path, args: &[&str]) {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .env("GIT_AUTHOR_DATE", "2026-01-01T00:00:00Z")
            .env("GIT_COMMITTER_DATE", "2026-01-01T00:00:00Z")
            .output()
            .expect("git");
        assert!(
            out.status.success(),
            "git {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// Пишет файл фикстуры (родители создаются).
    pub(crate) fn write_file(dir: &Path, name: &str, content: &str) {
        let p = dir.join(name);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).expect("mkdir");
        }
        std::fs::write(&p, content).expect("write");
    }

    /// git-репозиторий с одним коммитом текущего содержимого (пустой —
    /// тоже коммит: `--allow-empty`, фикстуре не обязан что-то писать).
    pub(crate) fn git_repo(dir: &Path) {
        git_in(dir, &["init", "-q", "-b", "main"]);
        git_in(dir, &["add", "-A"]);
        git_in(dir, &["commit", "-q", "--allow-empty", "-m", "base"]);
    }

    #[test]
    fn snapshot_reads_files_and_contents() {
        let dir = tempfile::tempdir().expect("tmp");
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).expect("mkdir");
        write_file(&repo, "src/main.py", "import os\n");
        write_file(&repo, "docs/note.txt", "текст\n");
        write_file(&repo, "model/CMP-001.md", "---\nid: CMP-001\n---\n");
        git_repo(&repo);

        let snap = snapshot_at(&repo, "HEAD").expect("снимок");
        assert!(snap.files.contains("src/main.py"));
        assert!(snap.files.contains("docs/note.txt"));
        // txt не нужен сканерам — путь есть, содержимого нет.
        assert!(snap.content("docs/note.txt").is_none());
        assert_eq!(snap.content("src/main.py"), Some("import os\n"));
        assert_eq!(snap.content("model/CMP-001.md").map(str::len), Some(20));

        // Повторный прогон той же ревизии — тот же снимок.
        let again = snapshot_at(&repo, "HEAD").expect("снимок");
        assert_eq!(snap.rev, again.rev);
        assert_eq!(snap.files, again.files);
        assert_eq!(snap.contents, again.contents);
    }

    #[test]
    fn snapshot_ignores_working_tree() {
        let dir = tempfile::tempdir().expect("tmp");
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).expect("mkdir");
        write_file(&repo, "src/main.py", "import os\n");
        git_repo(&repo);
        // Грязное рабочее дерево после коммита снимок не видит.
        write_file(&repo, "src/main.py", "import changed\n");
        write_file(&repo, "untracked.py", "import ghost\n");
        let snap = snapshot_at(&repo, "HEAD").expect("снимок");
        assert_eq!(snap.content("src/main.py"), Some("import os\n"));
        assert!(!snap.files.contains("untracked.py"));
    }

    #[test]
    fn unknown_rev_is_error() {
        let dir = tempfile::tempdir().expect("tmp");
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).expect("mkdir");
        git_repo(&repo);
        assert!(snapshot_at(&repo, "no-such-ref").is_err());
    }

    // --- ADR-046 Am.2: ignore + max_files в сканере снимка ---

    /// `ignore` (подстроки путей) вычитает файлы и из списка, и из содержимого
    /// снимка рабочего дерева; непричастные пути остаются.
    #[test]
    fn worktree_ignore_filters_paths_and_contents() {
        let dir = tempfile::tempdir().expect("tmp");
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).expect("mkdir");
        write_file(&repo, "env/conf.yaml", "a: 1\n");
        write_file(&repo, "vendor/lib.yaml", "b: 2\n");
        write_file(&repo, "src/app.yaml", "c: 3\n");
        let limits = ScanLimits {
            max_content_files: MAX_SNAPSHOT_CONTENT_FILES,
            ignore: vec!["env/".to_string(), "vendor/".to_string()],
        };
        let snap = snapshot_worktree_with(&repo, &limits).expect("снимок");
        assert!(snap.files.contains("src/app.yaml"), "{:?}", snap.files);
        assert!(!snap.files.contains("env/conf.yaml"), "{:?}", snap.files);
        assert!(!snap.files.contains("vendor/lib.yaml"), "{:?}", snap.files);
        assert_eq!(snap.content("src/app.yaml"), Some("c: 3\n"));
        assert!(snap.content("env/conf.yaml").is_none());
    }

    /// Тот же `ignore` применяется к снимку ревизии (база диффа): иначе
    /// асимметрия база/голова порождает ложные рёбра.
    #[test]
    fn revision_ignore_filters_paths() {
        let dir = tempfile::tempdir().expect("tmp");
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).expect("mkdir");
        write_file(&repo, "env/conf.yaml", "a: 1\n");
        write_file(&repo, "src/app.yaml", "c: 3\n");
        git_repo(&repo);
        let limits = ScanLimits {
            max_content_files: MAX_SNAPSHOT_CONTENT_FILES,
            ignore: vec!["env/".to_string()],
        };
        let snap = snapshot_at_with(&repo, "HEAD", &limits).expect("снимок");
        assert!(snap.files.contains("src/app.yaml"), "{:?}", snap.files);
        assert!(!snap.files.contains("env/conf.yaml"), "{:?}", snap.files);
    }

    /// Превышение лимита — ошибка, называющая конфиг-ключ
    /// `[gate.arch_drift] max_files` (диагностика обязана указывать, что крутить).
    #[test]
    fn worktree_limit_exceeded_names_config_key() {
        let dir = tempfile::tempdir().expect("tmp");
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).expect("mkdir");
        write_file(&repo, "src/a.yaml", "a: 1\n");
        write_file(&repo, "src/b.yaml", "b: 2\n");
        let limits = ScanLimits {
            max_content_files: 1,
            ignore: Vec::new(),
        };
        let err = snapshot_worktree_with(&repo, &limits).expect_err("лимит");
        let text = err.to_string();
        assert!(text.contains("[gate.arch_drift] max_files"), "{text}");
        // Поднятый лимит — проходит.
        let ok = ScanLimits {
            max_content_files: 10,
            ignore: Vec::new(),
        };
        assert!(snapshot_worktree_with(&repo, &ok).is_ok());
    }
}
