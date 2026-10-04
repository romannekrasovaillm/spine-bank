//! Тесты составляющей `secrets` (C3 волны C 0.3.12): литеральные секреты в
//! исходниках. Вынесены отдельным файлом — родительский `tests.rs` держится
//! под границей `prod_file_length_limit` (C-33), а находки составляющей
//! (`secret_literal`) — самостоятельный предмет.

use super::*;
use crate::gate::testkit::*;

use crate::gate::{GateOptions, GateRequirements, GateStatus, render, run_opts};

// --- C3 (0.3.12): литеральные секреты в исходниках (составляющая `secrets`) ---

/// Репозиторий с файлом `src/leak.go`, содержащим литеральные креденшлы
/// (крипто-фикстуры: валидные по форме, но нерабочие токены). GitHub-префикс
/// приклеивается в рантайме — в исходнике нет литерала `ghp_<36>` (fitness
/// C-05 «нет литералов секретов в коде»).
fn make_leak_repo(dir: &Path) {
    make_gate_repo(dir);
    std::fs::create_dir_all(dir.join("src")).expect("mkdir src");
    let token = format!("ghp_{}", "16C7e42F292c6912E7710c838347Ae178B4a");
    std::fs::write(
        dir.join("src/leak.go"),
        format!(
            "package main\n\nvar awsKey = \"AKIAIOSFODNN7EXAMPLE\"\n\
             var ghToken = \"{token}\"\n"
        ),
    )
    .expect("leak");
}

/// Настройки составляющей `secrets` с явной severity.
fn secrets_options(
    severity: crate::config::SecretSeverity,
    scope: crate::config::SecretScope,
) -> GateOptions {
    GateOptions {
        secrets: crate::config::SecretsConfig { severity, scope },
        ..GateOptions::default()
    }
}

/// RA-10: по умолчанию литеральный секрет — warn-находка `secret_literal` с
/// адресом файл:строка, составляющая остаётся PASS (чужие пайплайны не краснеют).
#[test]
fn secret_literal_is_warn_finding_by_default() {
    let tmp = tempfile::tempdir().expect("tmp");
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).expect("mkdir");
    make_leak_repo(&repo);
    let options = secrets_options(
        crate::config::SecretSeverity::Warn,
        crate::config::SecretScope::Changed,
    );
    let report = run_opts(
        &repo,
        Some(Route::Fast),
        Some("HEAD"),
        None,
        (1, 4),
        &GateRequirements::default(),
        &options,
    )
    .expect("гейт");
    let secrets = report
        .components
        .iter()
        .find(|c| c.name == "secrets")
        .expect("есть составляющая secrets");
    assert_eq!(secrets.status, GateStatus::Pass, "{}", render(&report));
    let aws = secrets
        .findings
        .iter()
        .find(|f| {
            f.rule.as_deref() == Some("secret_literal") && f.file.as_deref() == Some("src/leak.go")
        })
        .expect("находка secret_literal в src/leak.go");
    assert_eq!(aws.severity, "warn");
    assert_eq!(aws.line, Some(3), "строка ключа: {aws:?}");
    assert!(aws.message.contains("aws-access-key-id"), "{aws:?}");
    assert!(
        aws.message.contains("AKIA***"),
        "значение маскировано: {aws:?}"
    );
    assert!(
        !aws.message.contains("IOSFODNN7"),
        "значение не раскрыто: {aws:?}"
    );
    // GitHub-токен тоже найден (RA-10).
    assert!(
        secrets
            .findings
            .iter()
            .any(|f| f.line == Some(4) && f.message.contains("github-token")),
        "{:?}",
        secrets.findings
    );
    // Значения секретов в тексте вердикта не появляются.
    let text = render(&report);
    assert!(
        !text.contains("IOSFODNN7") && !text.contains("16C7e42F"),
        "{text}"
    );
}

/// `[gate.secrets] severity = "error"` повышает находку до блокирующей —
/// составляющая FAIL, итог гейта красный.
#[test]
fn secret_severity_error_fails_component() {
    let tmp = tempfile::tempdir().expect("tmp");
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).expect("mkdir");
    make_leak_repo(&repo);
    let options = secrets_options(
        crate::config::SecretSeverity::Error,
        crate::config::SecretScope::Changed,
    );
    let report = run_opts(
        &repo,
        Some(Route::Fast),
        Some("HEAD"),
        None,
        (1, 4),
        &GateRequirements::default(),
        &options,
    )
    .expect("гейт");
    assert_eq!(status_of(&report, "secrets"), GateStatus::Fail);
    assert!(!report.passed, "{}", render(&report));
}

/// Область `all` сканирует и закоммиченный файл (не только дифф), область
/// `changed` на чистом дереве находок не даёт — детерминизм областей.
#[test]
fn secrets_scope_all_sees_committed_file() {
    let tmp = tempfile::tempdir().expect("tmp");
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).expect("mkdir");
    make_leak_repo(&repo);
    // Фиксируем утечку в истории: в диффе её больше нет.
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-q", "-m", "leak"]);
    let changed = secrets_options(
        crate::config::SecretSeverity::Warn,
        crate::config::SecretScope::Changed,
    );
    let report = run_opts(
        &repo,
        Some(Route::Fast),
        Some("HEAD"),
        None,
        (1, 4),
        &GateRequirements::default(),
        &changed,
    )
    .expect("гейт");
    let s = report
        .components
        .iter()
        .find(|c| c.name == "secrets")
        .expect("secrets");
    assert!(
        s.findings.is_empty(),
        "diff пуст — находок нет: {:?}",
        s.findings
    );

    let all = secrets_options(
        crate::config::SecretSeverity::Warn,
        crate::config::SecretScope::All,
    );
    let report = run_opts(
        &repo,
        Some(Route::Fast),
        Some("HEAD"),
        None,
        (1, 4),
        &GateRequirements::default(),
        &all,
    )
    .expect("гейт");
    let s = report
        .components
        .iter()
        .find(|c| c.name == "secrets")
        .expect("secrets");
    assert!(
        s.findings
            .iter()
            .any(|f| f.file.as_deref() == Some("src/leak.go")),
        "scope=all видит закоммиченный файл: {:?}",
        s.findings
    );
}
