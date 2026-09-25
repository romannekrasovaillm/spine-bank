//! Git-предпроверки и пути реестра ограничений (B1): обвязка вызовов
//! `git` для составляющих гейта и резолв `CONSTRAINTS.yaml` к репозиторию.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::control;
use crate::error::{HarnessError, Result};

/// Предпроверка git-окружения репозитория (один раз на прогон).
pub(super) struct GitProbe {
    /// Это git-репозиторий (`git rev-parse --git-dir` успешен).
    pub(super) repo: bool,
    /// Есть HEAD (`git rev-parse --verify HEAD`): в репозитории без единого
    /// коммита базы для диффа/сравнения нет.
    pub(super) head: bool,
}

impl GitProbe {
    /// Снимает состояние git для `repo`. Любой сбой запуска git трактуется
    /// как «не git» (fail-soft: git-составляющие получат SKIP).
    pub(super) fn probe(repo: &Path) -> Self {
        let is_repo = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(["rev-parse", "--git-dir"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success());
        let head = is_repo
            && Command::new("git")
                .arg("-C")
                .arg(repo)
                .args(["rev-parse", "--verify", "--quiet", "HEAD"])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|s| s.success());
        Self {
            repo: is_repo,
            head,
        }
    }
}

/// Левая сторона диапазона диффа как одиночная ревизия для `git show`
/// (`origin/main...HEAD` → `origin/main`): `git show` диапазон не принимает.
/// Реализация — единая с диффом ([`control::base_rev`]), чтобы формы базы не
/// разъезжались между составляющими гейта (T-03).
pub(super) fn base_rev(base: &str) -> &str {
    control::base_rev(base)
}

/// Ревизия существует (`git rev-parse --verify <rev>^{commit}`)?
pub(super) fn git_rev_exists(repo: &Path, rev: &str) -> bool {
    Command::new("git")
        .arg("-C")
        .arg(repo)
        .arg("rev-parse")
        .arg("--verify")
        .arg("--quiet")
        .arg(format!("{rev}^{{commit}}"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// Путь присутствует в ревизии (`git cat-file -e <rev>:<rel>`)? Ревизия
/// обязана существовать (проверяется [`git_rev_exists`]).
pub(super) fn git_rev_has_path(repo: &Path, rev: &str, rel: &str) -> bool {
    Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["cat-file", "-e"])
        .arg(format!("{rev}:{rel}"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// Первая непустая строка stderr git без префикса «fatal:» — краткая причина
/// для отчёта. Сырой stderr целиком в отчёт не проксируем (D9): там
/// многострочная справка использования, засорявшая вывод гейта.
fn git_stderr_reason(stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr);
    let reason = text.lines().map(str::trim).find(|l| !l.is_empty()).map_or(
        "git завершился с ошибкой без сообщения",
        |l| l.strip_prefix("fatal:").map_or(l, str::trim),
    );
    reason.chars().take(160).collect()
}

/// Корень git-репозитория для `repo` (`git rev-parse --show-toplevel`),
/// канонизированный. `None` — git недоступен или каталог вне репозитория.
fn git_toplevel(repo: &Path) -> Option<PathBuf> {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["rev-parse", "--show-toplevel"])
        .stdin(Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8(out.stdout).ok()?;
    PathBuf::from(text.trim()).canonicalize().ok()
}

/// Путь `file` относительно корня git-репозитория (для `<rev>:<path>`).
/// Git резолвит такие пути от toplevel, а не от `-C <dir>`: на кейсе-
/// подкаталоге чужого монорепо только так сравнение идёт с файлом самого
/// кейса, а не с реестром внешнего репозитория (ложные «удалено из реестра»).
pub(super) fn git_rel_path(repo: &Path, file: &Path) -> Option<String> {
    let top = git_toplevel(repo)?;
    let abs = file.canonicalize().ok()?;
    let rel = abs.strip_prefix(&top).ok()?;
    Some(rel.to_string_lossy().replace('\\', "/"))
}

/// Содержимое файла в ревизии (`git show <rev>:<rel>`).
///
/// # Errors
/// `git` недоступен, ревизия плохая, содержимое не UTF-8.
pub(super) fn git_show_file(repo: &Path, rev: &str, rel: &str) -> Result<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .arg("show")
        .arg(format!("{rev}:{rel}"))
        .stdin(Stdio::null())
        .output()
        .map_err(|e| HarnessError::Control(format!("git show не запустился: {e}")))?;
    if !out.status.success() {
        return Err(HarnessError::Control(format!(
            "git show {rev}:{rel}: {}",
            git_stderr_reason(&out.stderr)
        )));
    }
    String::from_utf8(out.stdout)
        .map_err(|_| HarnessError::Control(format!("{rev}:{rel}: содержимое не UTF-8")))
}

/// Разрешённый путь к файлу ограничений гейта: явный `--constraints` либо
/// дефолт с fallback'ом на корневой `CONSTRAINTS.yaml` (D6); резолв —
/// единый [`control::resolve_constraints_path_detailed`] (E2).
pub(super) struct ConstraintsPath {
    /// Файл ограничений (может не существовать — составляющие дадут SKIP).
    pub(super) path: PathBuf,
    /// Путь задан явно флагом `--constraints`: тогда путь вне репозитория
    /// валит `rule_weakened` (fail-closed), а не молча отключает
    /// анти-ослабление (раньше — SKIP «git-сравнение невозможно»).
    pub(super) explicit: bool,
    /// Вторая копия реестра, отличающаяся от использованной (drift, E2) —
    /// пометка в детали составляющей `fitness`.
    pub(super) drift: Option<PathBuf>,
}

/// Есть ли в репозитории хоть одна копия реестра правил — корневая или
/// пакетная (T-01). Отличает «пользователь указал не тот файл» от «контура в
/// проекте нет вообще»: во втором случае сообщение обязано назвать оба места
/// и способ создать каркас.
pub(super) fn has_any_registry(repo: &Path) -> bool {
    repo.join(control::ROOT_CONSTRAINTS_PATH).is_file()
        || repo.join(control::HANDOFF_CONSTRAINTS_PATH).is_file()
}

/// Путь к файлу ограничений для отчёта: относительный к репозиторию, когда
/// файл внутри него (иначе — как передан).
pub(super) fn constraints_label(repo: &Path, constraints: &Path) -> String {
    constraints
        .strip_prefix(repo)
        .unwrap_or(constraints)
        .display()
        .to_string()
}

/// Относительный путь `file` внутри `repo` по канонизированным путям
/// (`None` — файл вне репозитория или канонизация не удалась). Сравнение
/// канонизированно: явный `--constraints` бывает абсолютным или с `./`,
/// тогда как `repo` — `.` (раньше такой путь улетал в SKIP).
pub(super) fn canonical_rel(repo: &Path, file: &Path) -> Option<PathBuf> {
    let abs_file = file.canonicalize().ok()?;
    let abs_repo = repo.canonicalize().ok()?;
    abs_file.strip_prefix(&abs_repo).ok().map(Path::to_path_buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Реестр правил виден в обоих исторических расположениях — и корневом,
    /// и пакетном; отсутствие обоих — честное `false`.
    #[test]
    fn has_any_registry_sees_both_locations() {
        let tmp = tempfile::tempdir().expect("tmp");
        let repo = tmp.path();
        assert!(!has_any_registry(repo), "пустой каталог — реестра нет");

        std::fs::write(
            repo.join(crate::control::ROOT_CONSTRAINTS_PATH),
            "rules: []\n",
        )
        .expect("root registry");
        assert!(has_any_registry(repo), "корневой реестр виден");

        std::fs::remove_file(repo.join(crate::control::ROOT_CONSTRAINTS_PATH)).expect("rm");
        std::fs::create_dir_all(repo.join(".arch-handoff")).expect("mkdir");
        std::fs::write(
            repo.join(crate::control::HANDOFF_CONSTRAINTS_PATH),
            "rules: []\n",
        )
        .expect("handoff registry");
        assert!(has_any_registry(repo), "пакетный реестр виден");
    }

    /// Путь в ревизии: существующий — да, отсутствующий — нет (иначе
    /// храповик маршрута сравнивал бы несуществующий файл).
    #[test]
    fn git_rev_has_path_distinguishes_present_and_missing() {
        let tmp = tempfile::tempdir().expect("tmp");
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).expect("mkdir");
        std::fs::write(repo.join("ROUTE.lock"), "route: fast\n").expect("lock");
        git(&repo, &["init", "-q"]);
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-q", "-m", "lock"]);
        assert!(git_rev_exists(&repo, "HEAD"), "коммит есть");
        assert!(git_rev_has_path(&repo, "HEAD", "ROUTE.lock"), "путь есть");
        assert!(
            !git_rev_has_path(&repo, "HEAD", "nope.lock"),
            "отсутствующего пути нет"
        );
    }

    /// Причина из stderr git: первая НЕПУСТАЯ строка, без префикса `fatal:`,
    /// с потолком 160 символов; пустой stderr — своя формулировка, а не
    /// пустая строка.
    #[test]
    fn git_stderr_reason_is_first_nonempty_line_without_fatal() {
        assert_eq!(
            git_stderr_reason(b""),
            "git завершился с ошибкой без сообщения"
        );
        assert_eq!(
            git_stderr_reason(b"\n   \nfatal: not a git repository\nusage: git ...\n"),
            "not a git repository"
        );
        // Без префикса `fatal:` строка отдаётся как есть (обрезанная по краям).
        assert_eq!(git_stderr_reason(b"  error: pathspec\n"), "error: pathspec");
        // Длинная строка обрезается до 160 символов.
        let long = "x".repeat(300);
        assert_eq!(git_stderr_reason(long.as_bytes()).chars().count(), 160);
    }

    /// git в каталоге с тестовой идентичностью коммиттера (как testkit гейта).
    fn git(dir: &Path, args: &[&str]) {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .output()
            .expect("git");
        assert!(
            out.status.success(),
            "git {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr)
        );
    }
}
