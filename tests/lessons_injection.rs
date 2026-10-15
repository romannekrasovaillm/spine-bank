//! Интеграционные тесты инъекции уроков (ADR-087, пункт L3 задачи 0.5.0):
//! хуки `session-start`/`user-prompt-submit` (fail-soft: без реестра — пустой
//! вывод и exit 0), блок уроков в задаче исполнителя `harness-run` и
//! миграция `failure_lessons.md` в реестр.
//!
//! Все тесты детерминированы и офлайн: дом харнесса изолирован в tempdir
//! (`HOME`/`XDG_CONFIG_HOME`), `ARCH_HOME` снят, LLM не вызывается — сборка
//! блока и отбор уроков детерминированы (AD-2), кодовый харнесс подменён
//! скриптом-эхом.

use std::path::{Path, PathBuf};
use std::process::Output;

/// Путь к собранному `arch-be` (см. `tests/lessons_ledger.rs`: переменная
/// `CARGO_BIN_EXE_arch-be` с дефисом в имени не задаётся cargo).
fn arch_bin() -> PathBuf {
    if let Some(path) = std::env::var_os("CARGO_BIN_EXE_arch-be") {
        return PathBuf::from(path);
    }
    let exe = std::env::current_exe().expect("путь тестового бинаря");
    let profile = exe
        .parent()
        .and_then(Path::parent)
        .expect("каталог профиля target/<profile>");
    let candidate = profile.join("arch-be");
    assert!(
        candidate.is_file(),
        "бинарь arch-be не найден: {} (соберите: cargo build --bin arch-be)",
        candidate.display()
    );
    candidate
}

/// `arch-be` с изолированным домом: реальный `~`, ключи и `ARCH_HOME` не читаются.
fn arch_cmd(home: &Path) -> std::process::Command {
    let mut cmd = std::process::Command::new(arch_bin());
    cmd.env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .current_dir(home);
    for var in [
        "ARCH_HOME",
        "ARCH_NO_EXEC",
        "DEEPSEEK_API_KEY",
        "ZHIPU_API_KEY",
        "KIMI_API_KEY",
        "ANTHROPIC_API_KEY",
        "OPENAI_API_KEY",
    ] {
        cmd.env_remove(var);
    }
    cmd
}

/// Пишет тестовый `config.toml`: `[lessons]` включён/выключен, пользовательский
/// реестр — внутри tempdir (иначе читался бы реальный дом).
fn write_config(home: &Path, enabled: bool) -> PathBuf {
    let path = home.join("config.toml");
    let text = format!(
        "[lessons]\nenabled = {enabled}\nuser_dir = '{}'\n",
        home.join("user-lessons").display()
    );
    std::fs::write(&path, text).expect("запись config.toml");
    path
}

/// Заводит урок реестра в репозитории через CLI (`lessons record`).
fn record_lesson(home: &Path, repo: &Path, code: &str, statement: &str, scope_glob: Option<&str>) {
    let mut cmd = arch_cmd(home);
    cmd.args(["lessons", "record", "--repo"])
        .arg(repo)
        .args(["--source", "gate", "--code", code])
        .args(["--run-id", &format!("run-{code}")])
        .args(["--excerpt", &format!("[{code}] нарушение")])
        .args(["--outcome", "fail"])
        .args(["--statement", statement]);
    if let Some(glob) = scope_glob {
        cmd.args(["--scope", "module", "--glob", glob]);
    }
    let out = cmd.output().expect("lessons record");
    assert!(
        out.status.success(),
        "record {code}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn run_with_config(home: &Path, config: &Path, args: &[&str], stdin: Option<&str>) -> Output {
    use std::io::Write as _;
    use std::process::Stdio;
    let mut cmd = arch_cmd(home);
    cmd.arg("--config").arg(config).args(args);
    if let Some(text) = stdin {
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = cmd.spawn().expect("spawn arch-be");
        child
            .stdin
            .as_mut()
            .expect("stdin")
            .write_all(text.as_bytes())
            .expect("запись stdin");
        return child.wait_with_output().expect("вывод arch-be");
    }
    cmd.output().expect("запуск arch-be")
}

/// Хук `session-start` без реестра — пустой stdout и exit 0 (fail-soft):
/// контур не имеет права ломать старт сессии хоста.
#[test]
fn session_start_hook_without_registry_is_empty_and_zero() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let config = write_config(tmp.path(), false);
    let out = run_with_config(tmp.path(), &config, &["hook", "session-start"], None);
    assert!(
        out.status.success(),
        "exit 0: {:?}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        out.stdout.is_empty(),
        "без реестра stdout пуст: {:?}",
        String::from_utf8_lossy(&out.stdout)
    );

    // И с включённым флагом, но без реестра — тоже пусто.
    let config_on = write_config(tmp.path(), true);
    let out = run_with_config(tmp.path(), &config_on, &["hook", "session-start"], None);
    assert!(out.status.success());
    assert!(
        out.stdout.is_empty(),
        "{:?}",
        String::from_utf8_lossy(&out.stdout)
    );
}

/// Хук `session-start` с реестром отдаёт закреплённый блок в stdout;
/// `cwd` из stdin хоста задаёт область отбора.
#[test]
fn session_start_hook_emits_pinned_block() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let home = tmp.path().join("home");
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&home).expect("home");
    std::fs::create_dir_all(&repo).expect("repo");
    let config = write_config(&home, true);
    record_lesson(
        &home,
        &repo,
        "G5",
        "Новый публичный символ обязан попасть в __all__ пакета",
        None,
    );

    let stdin = format!(
        "{{\"cwd\": \"{}\", \"hook_event_name\": \"SessionStart\"}}",
        repo.display()
    );
    let out = run_with_config(&home, &config, &["hook", "session-start"], Some(&stdin));
    assert!(
        out.status.success(),
        "{:?}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("## Уроки контура"), "{stdout}");
    assert!(stdout.contains("__all__"), "{stdout}");
    assert!(stdout.contains("DATA lessons"), "маркер данных: {stdout}");
}

/// Хук `user-prompt-submit`: блок отбирается по путям, упомянутым в промпте.
#[test]
fn user_prompt_submit_hook_selects_by_prompt_paths() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let home = tmp.path().join("home");
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&home).expect("home");
    std::fs::create_dir_all(&repo).expect("repo");
    let config = write_config(&home, true);
    record_lesson(
        &home,
        &repo,
        "G5",
        "Правило про __all__ пакета",
        Some("src/**"),
    );
    record_lesson(
        &home,
        &repo,
        "G7",
        "Документация обязана ссылаться на ADR",
        Some("docs/**"),
    );

    let stdin = format!(
        "{{\"cwd\": \"{}\", \"prompt\": \"обнови docs/readme.md\"}}",
        repo.display()
    );
    let out = run_with_config(
        &home,
        &config,
        &["hook", "user-prompt-submit"],
        Some(&stdin),
    );
    assert!(
        out.status.success(),
        "{:?}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("ADR"),
        "урок области docs в блоке: {stdout}"
    );
    assert!(
        !stdout.contains("__all__"),
        "урок src в блок для docs не попал: {stdout}"
    );
}

/// Миграция: `failure_lessons.md` отдаёт уроки реестру (B1, B2), повторный
/// прогон идемпотентен, отсутствие файла — не ошибка.
#[test]
fn migrate_command_moves_failure_lessons_into_registry() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let home = tmp.path().join("home");
    let repo = tmp.path().join("repo");
    let state = tmp.path().join("state");
    std::fs::create_dir_all(&home).expect("home");
    std::fs::create_dir_all(&repo).expect("repo");
    std::fs::create_dir_all(&state).expect("state");
    let config = write_config(&home, true);
    std::fs::write(
        state.join("failure_lessons.md"),
        "## 2026-10-14T10:00:00.000Z · `read_file` · io: <путь> not found\n\n\
         - Подпись: `read_file:io: <путь> not found`\n\
         - Случаев: 2 (первый: 2026-10-13T10:00:00.000Z, последний: 2026-10-14T10:00:00.000Z)\n\
         - Пример вывода: `io: /a/x.txt: not found`\n\
         - Рекомендация: инструмент повторно падает.\n",
    )
    .expect("failure_lessons.md");

    let state_flag = state.display().to_string();
    let repo_flag = repo.display().to_string();
    let out = run_with_config(
        &home,
        &config,
        &[
            "lessons",
            "migrate",
            "--state-dir",
            &state_flag,
            "--repo",
            &repo_flag,
        ],
        None,
    );
    assert!(
        out.status.success(),
        "миграция: {:?}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("Мигрировано уроков"), "{stdout}");

    // Урок виден в реестре репозитория.
    let list = run_with_config(
        &home,
        &config,
        &["lessons", "list", "--repo", &repo_flag],
        None,
    );
    let listed = String::from_utf8_lossy(&list.stdout);
    assert!(listed.contains("read_file"), "{listed}");

    // Повторный прогон — не ошибка (доказательства не дублируются).
    let again = run_with_config(
        &home,
        &config,
        &[
            "lessons",
            "migrate",
            "--state-dir",
            &state_flag,
            "--repo",
            &repo_flag,
        ],
        None,
    );
    assert!(again.status.success());

    // Нет файла — «мигрировать нечего», exit 0.
    let empty = run_with_config(
        &home,
        &config,
        &[
            "lessons",
            "migrate",
            "--state-dir",
            &tmp.path().join("нет-такого").display().to_string(),
            "--repo",
            &repo_flag,
        ],
        None,
    );
    assert!(empty.status.success());
    assert!(
        String::from_utf8_lossy(&empty.stdout).contains("Мигрировать нечего"),
        "{}",
        String::from_utf8_lossy(&empty.stdout)
    );
}

/// Блок уроков уходит в задачу исполнителя `harness-run` (ADR-087):
/// фейковый харнесс печатает полученный текст, в нём — заголовок блока.
#[cfg(feature = "harness")]
#[test]
fn harness_run_task_carries_lessons_block() {
    use std::os::unix::fs::PermissionsExt as _;

    let tmp = tempfile::tempdir().expect("tempdir");
    let home = tmp.path().join("home");
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&home).expect("home");
    std::fs::create_dir_all(&repo).expect("repo");
    // Конфиг: `[lessons]` включён + адаптер фейкового харнесса, печатающий stdin.
    let script = home.join("echo-harness.sh");
    std::fs::write(&script, "#!/bin/sh\ncat\n").expect("скрипт");
    let mut perms = std::fs::metadata(&script).expect("stat").permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&script, perms).expect("chmod");
    let config = home.join("config.toml");
    std::fs::write(
        &config,
        format!(
            "[lessons]\nenabled = true\nuser_dir = '{}'\n\n\
             [harnesses.fake]\nbinary = '{}'\nprompt_mode = 'stdin'\n\
             timeout_secs = 30\nidle_timeout_secs = 0\nauto_commit = false\npost_gate = false\n",
            home.join("user-lessons").display(),
            script.display()
        ),
    )
    .expect("config.toml");
    record_lesson(
        &home,
        &repo,
        "G5",
        "Новый публичный символ обязан попасть в __all__ пакета",
        Some("src/**"),
    );

    let repo_flag = repo.display().to_string();
    let out = run_with_config(
        &home,
        &config,
        &[
            "harness-run",
            "fake",
            "--repo",
            &repo_flag,
            "--task",
            "поправь src/lib.rs: публичный символ",
        ],
        None,
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "harness-run: {stdout}\n{stderr}");
    assert!(
        stdout.contains("## Уроки контура"),
        "блок в задаче: {stdout}"
    );
    assert!(stdout.contains("__all__"), "{stdout}");
}

/// Флаг выключен: `harness-run` отдаёт исполнителю задачу байт-в-байт прежней
/// (обратная совместимость 0.4.x по умолчанию).
#[cfg(feature = "harness")]
#[test]
fn harness_run_without_lessons_keeps_task_untouched() {
    use std::os::unix::fs::PermissionsExt as _;

    let tmp = tempfile::tempdir().expect("tempdir");
    let home = tmp.path().join("home");
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&home).expect("home");
    std::fs::create_dir_all(&repo).expect("repo");
    let script = home.join("echo-harness.sh");
    std::fs::write(&script, "#!/bin/sh\ncat\n").expect("скрипт");
    let mut perms = std::fs::metadata(&script).expect("stat").permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&script, perms).expect("chmod");
    let config = home.join("config.toml");
    std::fs::write(
        &config,
        format!(
            "[lessons]\nenabled = false\nuser_dir = '{}'\n\n\
             [harnesses.fake]\nbinary = '{}'\nprompt_mode = 'stdin'\n\
             timeout_secs = 30\nidle_timeout_secs = 0\nauto_commit = false\npost_gate = false\n",
            home.join("user-lessons").display(),
            script.display()
        ),
    )
    .expect("config.toml");
    record_lesson(&home, &repo, "G5", "Правило", None);

    let repo_flag = repo.display().to_string();
    let out = run_with_config(
        &home,
        &config,
        &[
            "harness-run",
            "fake",
            "--repo",
            &repo_flag,
            "--task",
            "проверь src/lib.rs",
        ],
        None,
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{stdout}");
    assert!(
        !stdout.contains("Уроки контура"),
        "выключенный флаг не инжектирует блок: {stdout}"
    );
}
