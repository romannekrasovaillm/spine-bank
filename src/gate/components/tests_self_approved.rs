//! Тесты диапазона прогона исполнителя (A5, ADR-055): самоодобрение через
//! ADR+override внутри диапазона и честная граница без диапазона. Вынесены
//! из `components/tests.rs` чистым перемещением под границу C-33.

use super::*;
use crate::gate::testkit::*;

use crate::gate::{GateOptions, GateRequirements, GateStatus, render, run, run_opts};

// --- A5: диапазон прогона исполнителя (ADR-055) -------------------------

/// Заглушка A5, сценарий RA-7 (минимальный): базовый коммит — реестр с
/// правилом `no_pan_in_logs` (error); «исполнитель» вторым коммитом создаёт
/// НАСТОЯЩИЙ принятый `ADR-900`, ослабляет реестр до `warn`, ставит override
/// на ADR-900 (горизонт в пределах) и пишет дельту `relax3`, упоминающую
/// изменённый реестр. Возвращает (repo, sha базового коммита).
///
/// MANIFEST.json тут нет: тест изолирует канал ADR+override и честную границу
/// варианта (а) — обычный гейт без диапазона зелёный (дельта покрывает
/// `delta_guard`), пост-гейт с диапазоном ловит самоодобрение.
fn make_ra7_repo(repo: &Path) -> String {
    write_fixture(
        repo,
        ".arch-handoff/CONSTRAINTS.yaml",
        "rules:\n  - id: X-1\n    name: no_pan_in_logs\n    type: must_not_contain\n    \
         glob: \"src/**/*.rs\"\n    pattern: \"PAN=\"\n    severity: error\n",
    );
    write_fixture(repo, "src/a.rs", "fn main() {}\n");
    write_fixture(repo, "ARCHITECTURE-SPINE.md", "# Spine\n");
    git(repo, &["init", "-q"]);
    git(repo, &["add", "."]);
    git(repo, &["commit", "-q", "-m", "baseline"]);
    let base = git_stdout(repo, &["rev-parse", "HEAD"]).trim().to_string();
    // Исполнитель: настоящий ADR + override + ослабление + своя дельта.
    let until = (chrono::Local::now() + chrono::Duration::days(182))
        .format("%Y-%m-%d")
        .to_string();
    write_fixture(
        repo,
        "docs/adr/ADR-900-real.md",
        "# ADR-900. Настоящее решение\n\n- Status: Accepted\n\n## Context\n\nсоздан исполнителем\n",
    );
    write_fixture(
        repo,
        ".arch-handoff/CONSTRAINTS.yaml",
        &format!(
            "rules:\n  - id: X-1\n    name: no_pan_in_logs\n    type: must_not_contain\n    \
             glob: \"src/**/*.rs\"\n    pattern: \"PAN=\"\n    severity: warn\n\
             overrides:\n  - rule: X-1\n    adr: ADR-900\n    until: \"{until}\"\n"
        ),
    );
    write_fixture(
        repo,
        "changes/relax3/DELTA.md",
        "# Дельта relax3\n\nПравим .arch-handoff/CONSTRAINTS.yaml: ослабление по ADR-900.\n",
    );
    git(repo, &["add", "."]);
    git(repo, &["commit", "-q", "-m", "relax3"]);
    base
}

/// A5 (ADR-055), канал ADR+override: с установленным диапазоном прогона
/// настоящий принятый ADR и override, созданные исполнителем, ослабление НЕ
/// узаконивают — `rule_weakened` (error) + `self_approved` (error); итог FAIL.
/// `self_approved` видна в паспорте (error-счётчик составляющей).
#[test]
fn gate_self_approved_on_adr_override_inside_range() {
    let tmp = tempfile::tempdir().expect("tmp");
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).expect("mkdir");
    let base = make_ra7_repo(&repo);
    let options = GateOptions {
        agent_range: Some(base.clone()),
        ..GateOptions::default()
    };
    let report = run_opts(
        &repo,
        None,
        Some(&base),
        None,
        (1, 4),
        &GateRequirements::default(),
        &options,
    )
    .expect("гейт");
    assert!(!report.passed, "{}", render(&report));
    assert_eq!(status_of(&report, "rule_weakened"), GateStatus::Fail);
    let text = render(&report);
    assert!(text.contains("rule_weakened"), "{text}");
    assert!(text.contains("[error] self_approved"), "{text}");
    assert!(text.contains("диапазоне прогона исполнителя"), "{text}");
    // Паспорт вердикта: находка `self_approved` — error-находка составляющей,
    // значит видна в счётчике ошибок паспорта (ADR-055, п. 3).
    let passport = crate::passport::Passport::build(&report, &repo).to_json();
    let rule_weakened = passport["checked"]
        .as_array()
        .expect("checked")
        .iter()
        .find(|c| c["name"] == "rule_weakened")
        .expect("составляющая rule_weakened в паспорте");
    assert!(
        rule_weakened["findings"]["error"].as_u64().unwrap_or(0) >= 2,
        "self_approved в паспорте: {rule_weakened}"
    );
}

/// A5 (ADR-055), честная граница варианта (а): тот же сценарий RA-7 БЕЗ
/// диапазона (обычный `arch-be gate`) остаётся зелёным — A2 проверяет
/// существование и статус ADR, происхождение вне worktree-контура невидимо.
#[test]
fn gate_without_range_keeps_ra7_green() {
    let tmp = tempfile::tempdir().expect("tmp");
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).expect("mkdir");
    let base = make_ra7_repo(&repo);
    let report = run(&repo, None, Some(&base), None, (1, 4)).expect("гейт");
    assert!(report.passed, "{}", render(&report));
    assert_eq!(status_of(&report, "rule_weakened"), GateStatus::Pass);
    assert!(
        !render(&report).contains("self_approved"),
        "без диапазона самоодобрения нет: {}",
        render(&report)
    );
}
