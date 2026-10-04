//! `arch-be connect git-hooks` (B1): pre-commit (быстрый `control check`) и
//! pre-push (полный `gate --route auto`) в `.git/hooks` — маркерные блоки,
//! идемпотентно, fail-soft.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use super::ci::{BLOCK_BEGIN, splice_marked_block};
use super::files::commit_file;
use super::types::ConnectReport;
use crate::error::{HarnessError, Result};

/// Общий git-каталог репозитория (`git rev-parse --git-common-dir`): в
/// worktree хуки живут в основном `.git`, поэтому голый `dir/.git` не подходит.
///
/// # Errors
/// `dir` — не git-репозиторий или git недоступен.
fn git_common_dir(dir: &Path) -> Result<PathBuf> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["rev-parse", "--git-common-dir"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .map_err(|e| HarnessError::Config(format!("git не запустился: {e}")))?;
    if !out.status.success() {
        return Err(HarnessError::Config(format!(
            "{}: не git-репозиторий — хуки ставятся только в git-проект",
            dir.display()
        )));
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let raw = text.trim();
    let path = PathBuf::from(raw);
    Ok(if path.is_absolute() {
        path
    } else {
        dir.join(path)
    })
}

/// Блок pre-commit: быстрый гейт fitness-правил. Тонкий shim на бинарь
/// (ROADMAP 1.7 п.2): резолв реестра (корень / `.arch-handoff/`), запуск
/// `control check` и код выхода (1 — блок) — в `arch-be hook pre-commit`,
/// версионируется с ядром. Fail-soft: нет `arch-be` в PATH — молча пропуск.
fn pre_commit_hook_block() -> String {
    "# spine-connect:begin — быстрый архитектурный гейт перед коммитом (arch-be)\n\
     # Fail-soft: нет arch-be в PATH — пропуск; семантика — в бинаре (arch-be hook pre-commit).\n\
     if command -v arch-be >/dev/null 2>&1; then\n\
     \x20 arch-be hook pre-commit\n\
     fi\n\
     # spine-connect:end"
        .to_string()
}

/// Блок pre-push: полный единый гейт (fitness + delta guard + анти-ослабление
/// правил + линтер спайна + трассировка + целостность модели; маршрут — из
/// диффа). Тонкий shim на бинарь (ROADMAP 1.7 п.2): stdin git'а (remote sha)
/// и выбор базы диффа — в `arch-be hook pre-push` (Н5: без базы уже
/// закоммиченное ослабление правил хуком не ловилось бы). Fail-soft при
/// отсутствии `arch-be`; без входа гейт сам уходит в SKIP и пропускает пуш.
pub(super) fn pre_push_hook_block() -> String {
    "# spine-connect:begin — полный архитектурный гейт перед пушем (arch-be)\n\
     # Fail-soft: нет arch-be в PATH — пропуск; база диффа из stdin git'а и коды — в бинаре.\n\
     if command -v arch-be >/dev/null 2>&1; then\n\
     \x20 arch-be hook pre-push\n\
     fi\n\
     # spine-connect:end"
        .to_string()
}

/// Право на исполнение для hook-файла (unix); вне unix — no-op.
#[cfg(unix)]
fn make_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    let mut perms = std::fs::metadata(path)
        .map_err(|e| HarnessError::io(path, e))?
        .permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(path, perms).map_err(|e| HarnessError::io(path, e))
}

/// Право на исполнение для hook-файла: вне unix не требуется.
#[cfg(not(unix))]
fn make_executable(_path: &Path) -> Result<()> {
    Ok(())
}

/// Встраивает блок в один hook-файл: новый файл — с shebang `#!/bin/sh`;
/// существующий с маркерами — замена блока; существующий без маркеров —
/// дописка (с заметкой про ранний `exit` чужого скрипта). После записи
/// файл делается исполняемым.
fn upsert_git_hook(
    hooks_dir: &Path,
    name: &str,
    block: &str,
    dry_run: bool,
    report: &mut ConnectReport,
) -> Result<()> {
    let path = hooks_dir.join(name);
    let old = match std::fs::read_to_string(&path) {
        Ok(text) => Some(text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(HarnessError::io(path, e)),
    };
    let new = match old.as_deref() {
        None => format!("#!/bin/sh\n\n{block}\n"),
        Some(existing) => {
            if !existing.contains(BLOCK_BEGIN) {
                report.notes.push(format!(
                    "{}: существующий хук без наших маркеров — блок дописан в конец; если ваш \
                     скрипт завершается `exit`, перенесите наш блок выше него",
                    path.display()
                ));
            }
            splice_marked_block(existing, block)
        }
    };
    commit_file(&path, old.as_deref(), &new, dry_run, report)?;
    // Исполняемость гарантируем и при «без изменений»: git молча игнорирует
    // хук без +x, а connect обязан оставить рабочее состояние.
    if !dry_run && path.is_file() {
        make_executable(&path)?;
    }
    Ok(())
}

/// `arch-be connect git-hooks`: pre-commit (быстрый `control check`) и
/// pre-push (полный `gate --route auto`) в `.git/hooks` (в worktree — в
/// hooks основного git-каталога). Идемпотентно (маркерные блоки), чужие
/// строки хуков сохраняются, `--dry-run` печатает план.
///
/// # Errors
/// `dir` — не git-репозиторий; ошибки чтения/записи файлов хуков.
pub fn connect_git_hooks(dir: &Path, dry_run: bool) -> Result<ConnectReport> {
    let mut report = ConnectReport {
        dry_run,
        ..ConnectReport::default()
    };
    let hooks_dir = git_common_dir(dir)?.join("hooks");
    upsert_git_hook(
        &hooks_dir,
        "pre-commit",
        &pre_commit_hook_block(),
        dry_run,
        &mut report,
    )?;
    upsert_git_hook(
        &hooks_dir,
        "pre-push",
        &pre_push_hook_block(),
        dry_run,
        &mut report,
    )?;
    report.notes.push(
        "семантика: fail-soft на инфраструктуру (нет arch-be/ограничений — пропуск), \
         блок (exit 1) — по коду возврата arch-be; строки вывода хуки не разбирают"
            .into(),
    );
    report.next_steps.extend([
        "сделайте тестовый коммит: pre-commit прогонит `arch-be control check .`".to_string(),
        "проверьте pre-push: `git push --dry-run` (или `arch-be gate --route auto` вручную)"
            .to_string(),
    ]);
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connect::testkit::{git, read, snapshot};

    /// T-01: та же ошибка в git-хуках и CI-шаблонах — гард по
    /// `.arch-handoff/CONSTRAINTS.yaml` глушил pre-commit на кейсе с реестром
    /// в корне. Расположение реестра в шаблонах не зашито: его резолвит
    /// бинарь (корень, затем `.arch-handoff/`). ROADMAP 1.7 п.2: блоки —
    /// тонкие shim'ы на `arch-be hook <имя>`, логика в бинаре.
    #[test]
    fn git_and_ci_templates_do_not_encode_registry_location() {
        for (name, block) in [
            ("pre-commit", pre_commit_hook_block()),
            ("pre-push", pre_push_hook_block()),
        ] {
            assert!(
                !block.contains("CONSTRAINTS"),
                "{name}: путь к реестру знает только бинарь: {block}"
            );
            assert!(block.contains("command -v arch-be"), "{name}: {block}");
            assert!(block.contains("spine-connect"), "{name}: {block}");
            assert!(block.contains("arch-be hook "), "{name} — shim: {block}");
        }
        assert!(pre_commit_hook_block().contains("arch-be hook pre-commit"));
        assert!(pre_push_hook_block().contains("arch-be hook pre-push"));
    }

    /// git-hooks: pre-commit и pre-push с маркерами, исполняемые, fail-soft
    /// гарды; повтор без дублей; чужой хук не затирается.
    #[test]
    fn git_hooks_scaffold_in_git_repo_idempotently() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path().join("repo");
        std::fs::create_dir_all(&dir).expect("mkdir");
        git(&dir, &["init", "-q"]);
        // Чужой pre-push уже есть — не затираем.
        std::fs::write(dir.join(".git/hooks/pre-push"), "#!/bin/sh\necho mine\n")
            .expect("чужой pre-push");

        let report = connect_git_hooks(&dir, false).expect("connect git-hooks");
        let pre_commit = read(&dir.join(".git/hooks/pre-commit"));
        assert!(pre_commit.starts_with("#!/bin/sh\n"), "{pre_commit}");
        assert!(pre_commit.contains(BLOCK_BEGIN), "{pre_commit}");
        assert!(
            pre_commit.contains("command -v arch-be"),
            "fail-soft гард: {pre_commit}"
        );
        assert!(
            pre_commit.contains("arch-be hook pre-commit"),
            "быстрый гейт — shim на бинарь: {pre_commit}"
        );
        let pre_push = read(&dir.join(".git/hooks/pre-push"));
        assert!(pre_push.contains("echo mine"), "чужой хук цел: {pre_push}");
        assert!(
            pre_push.contains("arch-be hook pre-push"),
            "полный гейт — shim на бинарь: {pre_push}"
        );
        assert!(
            report
                .notes
                .iter()
                .any(|n| n.contains("pre-push") && n.contains("дописан")),
            "заметка про чужой хук: {:?}",
            report.notes
        );

        // Исполняемость (unix): git молча игнорирует хук без +x.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            for hook in ["pre-commit", "pre-push"] {
                let mode = std::fs::metadata(dir.join(".git/hooks").join(hook))
                    .expect("stat")
                    .permissions()
                    .mode();
                assert_eq!(mode & 0o111, 0o111, "{hook} не исполняемый");
            }
        }

        // Повтор: побайтово то же, маркеров по одному.
        let first = snapshot(&dir);
        connect_git_hooks(&dir, false).expect("повтор");
        assert_eq!(first, snapshot(&dir), "повторный запуск изменил файлы");
        assert_eq!(
            read(&dir.join(".git/hooks/pre-push"))
                .matches(BLOCK_BEGIN)
                .count(),
            1,
            "дубль маркера"
        );

        // --dry-run поверх — ничего не пишет.
        let report = connect_git_hooks(&dir, true).expect("dry-run");
        assert!(report.dry_run);
        assert_eq!(first, snapshot(&dir), "dry-run что-то записал");
    }

    /// git-hooks вне git-репозитория — понятная ошибка.
    #[test]
    fn git_hooks_outside_git_repo_is_error() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path().join("plain");
        std::fs::create_dir_all(&dir).expect("mkdir");
        let err = connect_git_hooks(&dir, false).expect_err("не git");
        assert!(err.to_string().contains("не git-репозиторий"), "{err}");
    }

    /// git-hooks в worktree: хуки кладутся в hooks ОСНОВНОГО git-каталога
    /// (`git rev-parse --git-common-dir`), а не в файл-указатель worktree.
    #[test]
    fn git_hooks_in_worktree_target_common_dir() {
        let tmp = tempfile::tempdir().expect("tmp");
        let main = tmp.path().join("main");
        std::fs::create_dir_all(&main).expect("mkdir");
        git(&main, &["init", "-q"]);
        std::fs::write(main.join("f"), "x").expect("write f");
        git(&main, &["add", "."]);
        git(&main, &["commit", "-q", "-m", "init"]);
        let wt = tmp.path().join("wt");
        git(
            &main,
            &["worktree", "add", "-q", wt.to_str().expect("utf8")],
        );

        connect_git_hooks(&wt, false).expect("connect git-hooks в worktree");
        assert!(
            main.join(".git/hooks/pre-commit").is_file(),
            "хук в основном .git"
        );
        assert!(
            !wt.join(".git/hooks").exists(),
            "у worktree .git — файл, каталога hooks в нём нет"
        );
    }
}
