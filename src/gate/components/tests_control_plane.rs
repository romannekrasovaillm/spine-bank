//! Тесты составляющей `control_plane` (A3): пины контрольной плоскости —
//! ловля снятого Stop-хука, сниженного порога рубрики, ослабленного реестра,
//! появления/удаления непиннутого файла и покрытия дельтой. Вынесены из
//! `components/tests.rs` чистым перемещением под границу C-33.

use super::*;
use crate::gate::testkit::*;

use crate::gate::{GateStatus, render, run};

// --- A3: пины контрольной плоскости (control_plane) ---------------------

/// Пишет `MANIFEST.json` с пинами контрольной плоскости: пары
/// «путь → Option<sha256>» (`None` — пин отсутствия файла при выдаче пакета).
fn write_control_plane_manifest(dir: &Path, pins: &[(&str, Option<String>)]) {
    let mut map = serde_json::Map::new();
    for (path, hash) in pins {
        map.insert(
            (*path).to_string(),
            match hash {
                Some(h) => serde_json::json!({ "sha256": h }),
                None => serde_json::Value::Null,
            },
        );
    }
    let manifest = serde_json::json!({ "control_plane": map }).to_string();
    std::fs::write(dir.join(".arch-handoff/MANIFEST.json"), manifest).expect("manifest");
}

/// sha256 файла внутри репозитория-фикстуры.
fn hash_of(dir: &Path, rel: &str) -> String {
    crate::hash::sha256_file(&dir.join(rel)).expect("hash файла")
}

/// Сценарий приёмки A3: удаление Stop-хука из запиненного
/// `.claude/settings.json` после выдачи пакета — `exit 1` (в отчёте)
/// с находкой `control_plane_tampered`.
#[test]
fn control_plane_catches_stop_hook_removal() {
    let tmp = tempfile::tempdir().expect("tmp");
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).expect("mkdir");
    make_gate_repo(&repo);
    write_fixture(
        &repo,
        ".claude/settings.json",
        r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"arch-be gate ."}]}]}}"#,
    );
    let pin = hash_of(&repo, ".claude/settings.json");
    write_control_plane_manifest(&repo, &[(".claude/settings.json", Some(pin))]);
    // Исполнитель убрал Stop-хук — пин разошёлся.
    write_fixture(&repo, ".claude/settings.json", r#"{"hooks":{}}"#);
    let report = run(&repo, None, None, None, (1, 4)).expect("гейт");
    assert!(!report.passed, "{}", render(&report));
    assert_eq!(status_of(&report, "control_plane"), GateStatus::Fail);
    let text = render(&report);
    assert!(text.contains("[error] control_plane_tampered"), "{text}");
    assert!(text.contains(".claude/settings.json"), "{text}");
    assert!(text.contains("вернуть файл или оформить дельту"), "{text}");
}

/// Понижение порога рубрики в запиненном `.arch-handoff/RUBRIC.yaml` — тоже
/// подмена контрольной плоскости (критерий приёмки A3).
#[test]
fn control_plane_catches_rubric_threshold_lowered() {
    let tmp = tempfile::tempdir().expect("tmp");
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).expect("mkdir");
    make_gate_repo(&repo);
    write_fixture(
        &repo,
        ".arch-handoff/RUBRIC.yaml",
        "scale_max: 5\npass_score: 4\n",
    );
    let pin = hash_of(&repo, ".arch-handoff/RUBRIC.yaml");
    write_control_plane_manifest(&repo, &[(".arch-handoff/RUBRIC.yaml", Some(pin))]);
    // Планка судьи понижена: 4 → 1.
    write_fixture(
        &repo,
        ".arch-handoff/RUBRIC.yaml",
        "scale_max: 5\npass_score: 1\n",
    );
    let report = run(&repo, None, None, None, (1, 4)).expect("гейт");
    assert!(!report.passed, "{}", render(&report));
    assert_eq!(status_of(&report, "control_plane"), GateStatus::Fail);
    let text = render(&report);
    assert!(text.contains("control_plane_tampered"), "{text}");
    assert!(text.contains("RUBRIC.yaml"), "{text}");
}

/// RA-4b: ослабление пакетной копии реестра (severity error→warn) при
/// выставленном пине ловится `control_plane_tampered` — дыра «синхронного
/// ослабления» закрыта.
#[test]
fn control_plane_catches_registry_weakened_after_pin() {
    let tmp = tempfile::tempdir().expect("tmp");
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).expect("mkdir");
    make_gate_repo(&repo);
    let pin = hash_of(&repo, ".arch-handoff/CONSTRAINTS.yaml");
    write_control_plane_manifest(&repo, &[(".arch-handoff/CONSTRAINTS.yaml", Some(pin))]);
    // Правка правила мимо дельты: та же форма, другой severity.
    write_fixture(
        &repo,
        ".arch-handoff/CONSTRAINTS.yaml",
        "rules:\n  - name: spine_present\n    type: file_exists\n    path: \"ARCHITECTURE-SPINE.md\"\n    severity: warn\n",
    );
    let report = run(&repo, None, None, None, (1, 4)).expect("гейт");
    assert!(!report.passed, "{}", render(&report));
    assert_eq!(status_of(&report, "control_plane"), GateStatus::Fail);
    let text = render(&report);
    assert!(text.contains("control_plane_tampered"), "{text}");
    assert!(text.contains(".arch-handoff/CONSTRAINTS.yaml"), "{text}");
}

/// Пин отсутствия (`null`): появление файла после выдачи пакета — расхождение.
#[test]
fn control_plane_catches_appearance_of_unpinned_file() {
    let tmp = tempfile::tempdir().expect("tmp");
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).expect("mkdir");
    make_gate_repo(&repo);
    write_control_plane_manifest(&repo, &[(".gitlab-ci.yml", None)]);
    // Файла не было при выдаче пакета — появился после.
    write_fixture(&repo, ".gitlab-ci.yml", "stages: [test]\n");
    let report = run(&repo, None, None, None, (1, 4)).expect("гейт");
    assert!(!report.passed, "{}", render(&report));
    assert_eq!(status_of(&report, "control_plane"), GateStatus::Fail);
    assert!(
        render(&report).contains(".gitlab-ci.yml"),
        "{}",
        render(&report)
    );
}

/// Обратная сторона пина: запиненный файл исчез — тоже расхождение.
#[test]
fn control_plane_catches_removed_pinned_file() {
    let tmp = tempfile::tempdir().expect("tmp");
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).expect("mkdir");
    make_gate_repo(&repo);
    write_fixture(&repo, ".arch-handoff/SPEC.md", "# SPEC\n\nконтракты\n");
    let pin = hash_of(&repo, ".arch-handoff/SPEC.md");
    write_control_plane_manifest(&repo, &[(".arch-handoff/SPEC.md", Some(pin))]);
    std::fs::remove_file(repo.join(".arch-handoff/SPEC.md")).expect("remove SPEC");
    let report = run(&repo, None, None, None, (1, 4)).expect("гейт");
    assert!(!report.passed, "{}", render(&report));
    assert_eq!(status_of(&report, "control_plane"), GateStatus::Fail);
    assert!(render(&report).contains("SPEC.md"), "{}", render(&report));
}

/// A3: расхождение, упомянутое АКТИВНОЙ дельтой, не блокирует — правка
/// узаконена явным решением, а не спрятана (PASS с деталью «по дельте»).
#[test]
fn control_plane_change_covered_by_delta_passes() {
    let tmp = tempfile::tempdir().expect("tmp");
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).expect("mkdir");
    make_gate_repo(&repo);
    write_fixture(&repo, ".claude/settings.json", r#"{"hooks":{"Stop":[]}}"#);
    let pin = hash_of(&repo, ".claude/settings.json");
    write_control_plane_manifest(&repo, &[(".claude/settings.json", Some(pin))]);
    // Активная дельта упоминает файл — правка контрольной плоскости оформлена.
    write_fixture(
        &repo,
        "changes/stop-hook-edit/DELTA.md",
        "# Дельта\n\nПравим .claude/settings.json: добавляем Stop-хук.\n",
    );
    write_fixture(
        &repo,
        ".claude/settings.json",
        r#"{"hooks":{"Stop":[{"hooks":[{"command":"arch-be gate ."}]}]}}"#,
    );
    let report = run(&repo, None, None, None, (1, 4)).expect("гейт");
    assert_eq!(
        status_of(&report, "control_plane"),
        GateStatus::Pass,
        "{}",
        render(&report)
    );
    assert_eq!(status_of(&report, "delta_guard"), GateStatus::Pass);
    let text = render(&report);
    assert!(text.contains("изменён по дельте stop-hook-edit"), "{text}");
}

/// Обратная совместимость: нет MANIFEST.json → честный SKIP; MANIFEST без
/// поля `control_plane` (старый пакет) — тоже SKIP, чужой пайплайн не красится.
#[test]
fn control_plane_skips_without_manifest_or_pins() {
    let tmp = tempfile::tempdir().expect("tmp");
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).expect("mkdir");
    make_gate_repo(&repo);
    // (а) пакета нет вовсе.
    let report = run(&repo, None, None, None, (1, 4)).expect("гейт");
    assert_eq!(status_of(&report, "control_plane"), GateStatus::Skip);
    assert!(
        render(&report).contains("нет MANIFEST.json"),
        "{}",
        render(&report)
    );
    // (б) MANIFEST старого формата — поля control_plane нет.
    write_fixture(&repo, ".arch-handoff/MANIFEST.json", r#"{"route":"Fast"}"#);
    let report = run(&repo, None, None, None, (1, 4)).expect("гейт");
    assert_eq!(status_of(&report, "control_plane"), GateStatus::Skip);
    // (в) поле есть, но пусто — тот же SKIP.
    write_fixture(
        &repo,
        ".arch-handoff/MANIFEST.json",
        r#"{"control_plane":{}}"#,
    );
    let report = run(&repo, None, None, None, (1, 4)).expect("гейт");
    assert_eq!(status_of(&report, "control_plane"), GateStatus::Skip);
}

/// Битый MANIFEST.json — не «пинов нет», а сломанный вход: error-находка
/// `control_plane_invalid` (молчание отключало бы проверку подменой файла).
#[test]
fn control_plane_invalid_manifest_is_error() {
    let tmp = tempfile::tempdir().expect("tmp");
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).expect("mkdir");
    make_gate_repo(&repo);
    write_fixture(&repo, ".arch-handoff/MANIFEST.json", "{это не json");
    let report = run(&repo, None, None, None, (1, 4)).expect("гейт");
    assert!(!report.passed, "{}", render(&report));
    assert_eq!(status_of(&report, "control_plane"), GateStatus::Fail);
    let text = render(&report);
    assert!(text.contains("[error] control_plane_invalid"), "{text}");
}
