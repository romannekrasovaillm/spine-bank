//! Интеграционные тесты реестра уроков (Lesson Ledger, ADR-085): CLI-контракт
//! `arch-be lessons …`, детерминизм рендера, конкурентная дозапись журнала
//! двумя процессами и отказ импорту урока без доказательств.
//!
//! Все тесты детерминированы и офлайн: дом харнесса изолирован в tempdir
//! (`HOME`/`ARCH_HOME`), cwd — тот же tempdir, LLM не вызывается (реестр —
//! Core-модуль без LLM на любой ветке, AD-2).

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// Путь к собранному `arch-be`.
///
/// `env!("CARGO_BIN_EXE_arch-be")` в этом окружении не задаётся (дефис в имени
/// bin-таргета — известный обход cargo, см. память по репозиторию), зато
/// переменная видна в рантайме; запасной путь — бинарь рядом с каталогом
/// тестового исполняемого файла (`target/<профиль>/deps/… → target/<профиль>`).
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

/// Команда `arch-be` с изолированным домом: реальный `~` и ключи не читаются.
fn arch_cmd(home: &Path) -> Command {
    let mut cmd = Command::new(arch_bin());
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

/// Прогон команды реестра в каталоге `home`; возвращает вывод целиком.
fn run(home: &Path, args: &[&str]) -> Output {
    arch_cmd(home).args(args).output().expect("запуск arch-be")
}

/// Прогон с обязательным успехом (stdout/stderr попадают в панику).
fn run_ok(home: &Path, args: &[&str]) -> Output {
    let out = run(home, args);
    assert!(
        out.status.success(),
        "arch-be {args:?} упал с {:?}\nstdout: {}\nstderr: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    out
}

/// Запись урока в реестр репозитория `repo`.
fn record(repo: &Path, code: &str, run_id: &str, task: &str, outcome: &str) {
    run_ok(
        repo,
        &[
            "lessons",
            "record",
            "--repo",
            &repo.display().to_string(),
            "--source",
            "gate",
            "--code",
            code,
            "--statement",
            &format!("Правило {code}"),
            "--run-id",
            run_id,
            "--task",
            task,
            "--outcome",
            outcome,
            "--excerpt",
            &format!("[{code}] вердикт"),
            "--scope",
            "module",
            "--glob",
            "src/**",
        ],
    );
}

fn ledger_path(repo: &Path) -> PathBuf {
    repo.join(".arch/lessons/ledger.jsonl")
}

fn lessons_md(repo: &Path) -> String {
    std::fs::read_to_string(repo.join(".arch/lessons/LESSONS.md")).expect("LESSONS.md")
}

/// Все строки журнала — валидный JSON (журнал не испорчен дозаписью).
fn assert_jsonl_intact(path: &Path) -> Vec<serde_json::Value> {
    let text = std::fs::read_to_string(path).expect("журнал");
    assert!(text.ends_with('\n'), "журнал завершается переводом строки");
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            serde_json::from_str::<serde_json::Value>(line)
                .unwrap_or_else(|e| panic!("строка журнала не JSON: {e}\n{line}"))
        })
        .collect()
}

fn bundle(lessons: &serde_json::Value) -> String {
    serde_json::json!({
        "format": "lessons-bundle/v0",
        "version": 1,
        "arch_be": "0.4.11",
        "count": lessons.as_array().map_or(0, Vec::len),
        "lessons": lessons,
    })
    .to_string()
}

/// Урок с доказательством (ts фиксирован — рендер не зависит от часов).
fn lesson_json(code: &str, evidence: &serde_json::Value) -> serde_json::Value {
    let id = signature_id("gate", code);
    serde_json::json!({
        "id": id,
        "signature": {"source": "gate", "code": code},
        "statement": format!("Правило {code}"),
        "scope": {"level": "module", "globs": ["src/**"]},
        "evidence": evidence,
        "fails": 1,
        "hits": 0,
        "misses": 0,
        "consecutive_misses": 0,
        "status": "observed",
        "provenance": {"recorder": "cli", "verdict_sha256": "00", "arch_be": "0.4.11"}
    })
}

/// Идентификатор урока по подписи — той же формулой, что у реестра
/// (`LSN-<sha8 канонической подписи>`); независимая реализация в тесте
/// ловит расхождение контракта id.
fn signature_id(source: &str, code: &str) -> String {
    // SHA-256 считаем через крейт (часть публичного API ядра).
    let canonical = format!("{source}\u{1f}{code}\u{1f}");
    let hex = arch_harness::hash::sha256_hex(canonical.as_bytes());
    format!("LSN-{}", &hex[..8])
}

#[test]
fn lessons_md_is_a_deterministic_derivative_of_the_journal() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = tmp.path();
    // Три урока, две серии доказательств у первого (observed → candidate).
    record(repo, "G5", "run-1", "T1", "fail");
    record(repo, "G5", "run-2", "T2", "fail");
    record(repo, "G6", "run-3", "T1", "fail");
    record(repo, "G7", "run-4", "T1", "fail");

    let text = lessons_md(repo);
    assert!(text.contains("Уроков: 3"), "{text}");
    assert!(
        text.contains("candidate 1"),
        "G5 подтверждён наблюдением: {text}"
    );
    assert!(text.contains("observed 2"), "{text}");

    // Файл — производная журнала: рендер из прочитанного журнала совпадает
    // байт-в-байт, и повторный рендер не меняет ни байта.
    let ledger = arch_harness::lessons::Ledger::repo(repo);
    let snapshot = ledger
        .snapshot(&arch_harness::lessons::Thresholds::default())
        .expect("проекция журнала");
    let first = arch_harness::lessons::render(&snapshot.lessons);
    let second = arch_harness::lessons::render(&snapshot.lessons);
    assert_eq!(first, text, "файл = рендер журнала");
    assert_eq!(second, text, "рендер байт-в-байт стабилен");

    // Сортировка по id (а не по порядку записи).
    let ids: Vec<&str> = snapshot
        .lessons
        .iter()
        .map(|lesson| lesson.id.as_str())
        .collect();
    let mut sorted = ids.clone();
    sorted.sort_unstable();
    assert_eq!(ids, sorted, "проекция отсортирована по id");
}

/// Байт-в-байт детерминизм сквозь ДВА процесса: одинаковая замороженная
/// посылка (фиксированные метки времени в доказательствах) даёт одинаковый
/// `LESSONS.md`, то есть в рендер не протекает ни метка генерации, ни путь.
#[test]
fn render_is_byte_identical_across_processes() {
    let first = tempfile::tempdir().expect("tempdir");
    let second = tempfile::tempdir().expect("tempdir");
    let frozen = serde_json::json!([{"run_id": "run-1", "ts": "2026-10-14T10:00:00.000Z",
                                    "outcome": "fail", "verdict_excerpt": "[G5] нет __all__",
                                    "file": "src/lib.rs", "line": 12}]);
    let payload = bundle(&serde_json::json!([lesson_json("G5", &frozen)]));
    for repo in [first.path(), second.path()] {
        let file = repo.join("bundle.json");
        std::fs::write(&file, &payload).expect("посылка");
        run_ok(
            repo,
            &[
                "lessons",
                "import",
                &file.display().to_string(),
                "--repo",
                &repo.display().to_string(),
            ],
        );
    }
    let a = lessons_md(first.path());
    let b = lessons_md(second.path());
    assert_eq!(a, b, "рендер не зависит от процесса и каталога");
    assert!(a.contains("LSN-"), "{a}");
}

/// Конкурентная дозапись: два процесса пишут в один журнал одновременно —
/// ни одна строка не потеряна и не склеена (все строки валидны, id уникальны).
#[test]
fn concurrent_records_from_two_processes_keep_jsonl_valid() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = tmp.path().to_path_buf();
    let codes_per_process = 25;
    let children: Vec<_> = [("A", 0), ("B", 100)]
        .into_iter()
        .map(|(prefix, base)| {
            let repo = repo.clone();
            std::thread::spawn(move || {
                for i in 0..codes_per_process {
                    let code = format!("{prefix}{}", base + i);
                    record(&repo, &code, &format!("run-{code}"), "T1", "fail");
                }
            })
        })
        .collect();
    for child in children {
        child.join().expect("процесс дозаписи");
    }

    let events = assert_jsonl_intact(&ledger_path(&repo));
    assert_eq!(
        events.len(),
        codes_per_process * 2,
        "каждая запись попала в журнал целиком"
    );
    // Все уроки различимы: склеенных записей нет.
    let mut ids: Vec<String> = events
        .iter()
        .map(|event| {
            event["lesson"]["id"]
                .as_str()
                .expect("id в событии record")
                .to_string()
        })
        .collect();
    ids.sort();
    let total = ids.len();
    ids.dedup();
    assert_eq!(
        ids.len(),
        total,
        "id уроков уникальны — строки не склеились"
    );

    // Рендер видит все уроки, журнал остаётся читаемым.
    let text = lessons_md(&repo);
    assert!(text.contains(&format!("Уроков: {total}")), "{text}");
    // Замок не остался после работы процессов.
    assert!(
        !repo.join(".arch/lessons/ledger.jsonl.lock").exists(),
        "файл блокировки снят"
    );
}

#[test]
fn import_of_lesson_without_evidence_is_rejected() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = tmp.path();
    // Посылка: один урок с доказательством, второй — без (нарушение §7.6).
    let evidence = serde_json::json!([{"run_id": "r1", "ts": "2026-10-14T10:00:00.000Z",
                                       "outcome": "fail"}]);
    let payload = bundle(&serde_json::json!([
        lesson_json("G5", &evidence),
        lesson_json("G6", &serde_json::json!([])),
    ]));
    let file = repo.join("bundle.json");
    std::fs::write(&file, payload).expect("посылка");

    let out = run(
        repo,
        &[
            "lessons",
            "import",
            &file.display().to_string(),
            "--repo",
            &repo.display().to_string(),
        ],
    );
    assert!(
        !out.status.success(),
        "импорт урока без evidence обязан упасть"
    );
    assert_eq!(out.status.code(), Some(1), "ошибка CLI — код 1");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("evidence"), "причина названа: {stderr}");
    assert!(
        !ledger_path(repo).exists(),
        "валидация идёт ДО записи: частично залитого реестра не остаётся"
    );
}

#[test]
fn import_with_evidence_records_and_downgrades_to_candidate() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = tmp.path();
    // В файле заявлен статус `confirmed` — он НЕ принимается: чужой урок
    // понижается до `candidate` и обязан подтвердиться своим гейтом (ADR-091).
    let evidence = serde_json::json!([{"run_id": "r1", "ts": "2026-10-14T10:00:00.000Z",
                                       "outcome": "fail"}]);
    let mut lesson = lesson_json("G5", &evidence);
    lesson["status"] = serde_json::json!("confirmed");
    lesson["hits"] = serde_json::json!(9);
    let payload = bundle(&serde_json::json!([lesson]));
    let file = repo.join("bundle.json");
    std::fs::write(&file, payload).expect("посылка");

    let out = run_ok(
        repo,
        &[
            "lessons",
            "import",
            &file.display().to_string(),
            "--repo",
            &repo.display().to_string(),
        ],
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("Импортировано уроков: 1"), "{stdout}");

    let listing = run_ok(
        repo,
        &["lessons", "list", "--repo", &repo.display().to_string()],
    );
    let text = String::from_utf8_lossy(&listing.stdout);
    assert!(
        text.contains("candidate"),
        "чужой статус не принят, понижен до candidate: {text}"
    );
    assert!(
        !text.contains("confirmed"),
        "свой гейт ещё не подтвердил урок: {text}"
    );
    assert!(text.contains("gate · G5"), "{text}");

    // Импорт идемпотентен по доказательству: повтор не удваивает счётчики.
    run_ok(
        repo,
        &[
            "lessons",
            "import",
            &file.display().to_string(),
            "--repo",
            &repo.display().to_string(),
        ],
    );
    let out = run_ok(
        repo,
        &[
            "lessons",
            "list",
            "--repo",
            &repo.display().to_string(),
            "--format",
            "json",
        ],
    );
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).expect("json списка");
    assert_eq!(json["count"], 1);
    assert_eq!(
        json["lessons"][0]["fails"], 1,
        "то же доказательство не дублируется"
    );
}

#[test]
fn import_rejects_id_that_does_not_match_signature() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = tmp.path();
    let evidence = serde_json::json!([{"run_id": "r1", "ts": "2026-10-14T10:00:00.000Z",
                                       "outcome": "fail"}]);
    let mut lesson = lesson_json("G5", &evidence);
    lesson["id"] = serde_json::json!("LSN-deadbeef");
    let payload = bundle(&serde_json::json!([lesson]));
    let file = repo.join("bundle.json");
    std::fs::write(&file, payload).expect("посылка");

    let out = run(
        repo,
        &[
            "lessons",
            "import",
            &file.display().to_string(),
            "--repo",
            &repo.display().to_string(),
        ],
    );
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("не соответствует сигнатуре"), "{stderr}");
}

/// Экспорт подтверждённого урока и импорт в ДРУГОЙ репозиторий: чужой статус
/// не переносится — урок понижается до `candidate` (ADR-091, L10).
#[test]
fn export_then_import_to_another_repo_gives_candidate() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let src = tmp.path().join("src");
    let dst = tmp.path().join("dst");
    std::fs::create_dir_all(&src).expect("src");
    std::fs::create_dir_all(&dst).expect("dst");

    record(&src, "G5", "run-1", "T1", "fail");
    record(&src, "G5", "run-2", "T2", "pass_after_apply");
    record(&src, "G5", "run-3", "T3", "pass_after_apply");

    // Экспорт подписанного бандла lessons-bundle/v1.
    let exported = run_ok(
        &src,
        &[
            "lessons",
            "export",
            "--repo",
            &src.display().to_string(),
            "--min-status",
            "confirmed",
        ],
    );
    let bundle_json: serde_json::Value = serde_json::from_slice(&exported.stdout).expect("json");
    assert_eq!(bundle_json["format"], "lessons-bundle/v1");
    assert_eq!(bundle_json["v"], 1);
    assert_eq!(bundle_json["count"], 1);
    assert_eq!(bundle_json["lessons"][0]["status"], "confirmed");
    assert_eq!(
        bundle_json["signature"].as_str().expect("подпись").len(),
        64,
        "подпись — полный SHA-256"
    );

    // Импорт в другой репозиторий понижает статус.
    let file = dst.join("bundle.json");
    std::fs::write(&file, &exported.stdout).expect("посылка");
    run_ok(
        &dst,
        &[
            "lessons",
            "import",
            &file.display().to_string(),
            "--repo",
            &dst.display().to_string(),
        ],
    );

    let listed = run_ok(
        &dst,
        &["lessons", "list", "--repo", &dst.display().to_string()],
    );
    let text = String::from_utf8_lossy(&listed.stdout);
    assert!(
        text.contains("candidate"),
        "импорт понизил до candidate: {text}"
    );
    assert!(
        !text.contains("confirmed"),
        "чужой статус не перенесён: {text}"
    );
    assert!(text.contains("gate · G5"), "{text}");
}

/// Урок с битым хэшем доказательства отклоняется целиком (ничего не записано).
#[test]
fn import_rejects_tampered_evidence_hash() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let src = tmp.path().join("src");
    let dst = tmp.path().join("dst");
    std::fs::create_dir_all(&src).expect("src");
    std::fs::create_dir_all(&dst).expect("dst");
    record(&src, "G5", "run-1", "T1", "fail");

    let exported = run_ok(
        &src,
        &["lessons", "export", "--repo", &src.display().to_string()],
    );
    let mut bundle_json: serde_json::Value =
        serde_json::from_slice(&exported.stdout).expect("json");
    // Портим хэш доказательства (не трогая ни evidence, ни подпись).
    let hash = bundle_json["lessons"][0]["evidence_sha256"][0]
        .as_str()
        .expect("хэш")
        .to_string();
    let mut chars: Vec<char> = hash.chars().collect();
    chars[0] = if chars[0] == '0' { '1' } else { '0' };
    let tampered: String = chars.into_iter().collect();
    bundle_json["lessons"][0]["evidence_sha256"][0] = serde_json::json!(tampered);

    let file = dst.join("bundle.json");
    std::fs::write(&file, bundle_json.to_string()).expect("посылка");
    let out = run(
        &dst,
        &[
            "lessons",
            "import",
            &file.display().to_string(),
            "--repo",
            &dst.display().to_string(),
        ],
    );
    assert!(!out.status.success(), "битый хэш отклонён");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("хэш доказательства"), "{stderr}");
    assert!(!ledger_path(&dst).exists(), "ничего не записано при отказе");
}

/// Подпись бандла проверяется: подмена тела урока ломает подпись → отказ.
#[test]
fn import_verifies_bundle_signature() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let src = tmp.path().join("src");
    let dst = tmp.path().join("dst");
    std::fs::create_dir_all(&src).expect("src");
    std::fs::create_dir_all(&dst).expect("dst");
    record(&src, "G5", "run-1", "T1", "fail");

    let exported = run_ok(
        &src,
        &["lessons", "export", "--repo", &src.display().to_string()],
    );
    let mut bundle_json: serde_json::Value =
        serde_json::from_slice(&exported.stdout).expect("json");
    bundle_json["lessons"][0]["statement"] = serde_json::json!("чужое правило");

    let file = dst.join("bundle.json");
    std::fs::write(&file, bundle_json.to_string()).expect("посылка");
    let out = run(
        &dst,
        &[
            "lessons",
            "import",
            &file.display().to_string(),
            "--repo",
            &dst.display().to_string(),
        ],
    );
    assert!(!out.status.success(), "подмена тела отклонена");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("подпись"), "{stderr}");
    assert!(!ledger_path(&dst).exists(), "ничего не записано при отказе");
}

#[test]
fn full_lifecycle_through_cli_reaches_confirmed_and_retires() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = tmp.path();
    record(repo, "G5", "run-1", "T1", "fail");
    record(repo, "G5", "run-2", "T2", "fail");
    record(repo, "G5", "run-3", "T3", "pass_after_apply");
    let confirmed = run_ok(
        repo,
        &[
            "lessons",
            "record",
            "--repo",
            &repo.display().to_string(),
            "--source",
            "gate",
            "--code",
            "G5",
            "--run-id",
            "run-4",
            "--task",
            "T4",
            "--outcome",
            "pass_after_apply",
        ],
    );
    let stdout = String::from_utf8_lossy(&confirmed.stdout);
    assert!(stdout.contains("confirmed"), "{stdout}");
    assert!(stdout.contains("статус: candidate → confirmed"), "{stdout}");

    // Экспорт подтверждённого урока и отбор по минимальному статусу.
    let exported = run_ok(
        repo,
        &[
            "lessons",
            "export",
            "--repo",
            &repo.display().to_string(),
            "--min-status",
            "confirmed",
        ],
    );
    let bundle: serde_json::Value =
        serde_json::from_slice(&exported.stdout).expect("json экспорта");
    assert_eq!(bundle["count"], 1);
    assert_eq!(bundle["lessons"][0]["status"], "confirmed");
    let id = bundle["lessons"][0]["id"].as_str().expect("id").to_string();

    // Вывод из реестра: причина попадает в событие и в рендер.
    run_ok(
        repo,
        &[
            "lessons",
            "retire",
            &id,
            "--reason",
            "правило удалено из CONSTRAINTS",
            "--repo",
            &repo.display().to_string(),
        ],
    );
    let text = lessons_md(repo);
    assert!(
        text.contains("**Причина вывода:** правило удалено из CONSTRAINTS"),
        "{text}"
    );
    assert!(text.contains("retired 1"), "{text}");

    // Неизвестный id — ошибка с кодом 1 (не «тихий успех»).
    let missing = run(
        repo,
        &[
            "lessons",
            "show",
            "LSN-00000000",
            "--repo",
            &repo.display().to_string(),
        ],
    );
    assert_eq!(missing.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&missing.stderr).contains("не найден"));
}

#[test]
fn user_level_registry_lives_under_arch_home() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let home = tmp.path().join("arch-home");
    let work = tmp.path().join("work");
    std::fs::create_dir_all(&work).expect("каталог");

    let mut cmd = Command::new(arch_bin());
    cmd.env("HOME", tmp.path())
        .env("XDG_CONFIG_HOME", tmp.path().join(".config"))
        .env("ARCH_HOME", &home)
        .current_dir(&work)
        .args([
            "lessons",
            "record",
            "--user",
            "--source",
            "fitness",
            "--code",
            "C-33",
            "--run-id",
            "run-user",
            "--task",
            "T1",
            "--statement",
            "предел длины файла",
        ]);
    let out = cmd.output().expect("запуск");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        home.join("lessons/ledger.jsonl").is_file(),
        "пользовательский реестр лежит в $ARCH_HOME/lessons"
    );
    assert!(
        home.join("lessons/LESSONS.md").is_file(),
        "рендер пользовательского реестра — рядом с журналом"
    );
    // Репозиторный реестр не создан: уровни не смешиваются.
    assert!(!work.join(".arch/lessons/ledger.jsonl").exists());

    // Чтение пользовательского реестра видит урок.
    let out = arch_cmd(&work)
        .env("ARCH_HOME", &home)
        .args(["lessons", "list", "--user"])
        .output()
        .expect("запуск");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("fitness · C-33"), "{stdout}");
}

#[test]
fn conflicting_registry_flags_are_rejected() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let out = run(
        tmp.path(),
        &[
            "lessons",
            "list",
            "--user",
            "--repo",
            &tmp.path().display().to_string(),
        ],
    );
    assert_eq!(out.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("укажите одно"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn expiry_days_stamps_a_deadline_on_the_lesson() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = tmp.path();
    let repo_display = repo.display().to_string();
    run_ok(
        repo,
        &[
            "lessons",
            "record",
            "--repo",
            &repo_display,
            "--source",
            "gate",
            "--code",
            "G5",
            "--statement",
            "правило со сроком",
            "--run-id",
            "run-1",
            "--expiry-days",
            "3",
        ],
    );
    let listed = run_ok(
        repo,
        &[
            "lessons",
            "list",
            "--repo",
            &repo_display,
            "--format",
            "json",
        ],
    );
    let json: serde_json::Value = serde_json::from_slice(&listed.stdout).expect("json списка");
    let expiry = json["lessons"][0]["expiry"]
        .as_str()
        .expect("срок записан")
        .to_string();
    // Срок отсчитывается от даты записи и лежит строго в будущем.
    let today = arch_harness::lessons::today();
    assert!(expiry > today, "срок {expiry} позже сегодняшнего {today}");
    let policy = arch_harness::config::LessonsConfig {
        expiry_days: 3,
        ..arch_harness::config::LessonsConfig::default()
    };
    assert_eq!(
        policy.expiry_from(&today).as_deref(),
        Some(expiry.as_str()),
        "дата = сегодня + expiry_days"
    );
}

#[test]
fn empty_registry_is_not_an_error() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let out = run_ok(
        tmp.path(),
        &[
            "lessons",
            "list",
            "--repo",
            &tmp.path().display().to_string(),
        ],
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("Уроков: 0"), "{stdout}");
    // Чтение реестра ничего не создаёт.
    assert!(!tmp.path().join(".arch").exists());
}
