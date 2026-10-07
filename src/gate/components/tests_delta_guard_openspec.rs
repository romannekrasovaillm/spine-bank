//! Тесты составляющей `delta_guard` в части источника покрытия `OpenSpec`
//! (F1, ADR-062): покрытие правки модели активным change и правило владения
//! (ADR-055) для change, созданного в диапазоне исполнителя. Вынесены
//! отдельным файлом — родительский `tests.rs` держится под границей
//! `prod_file_length_limit` (C-33), прецедент — `tests_secrets.rs`.

use super::*;
use crate::gate::testkit::*;

use crate::gate::{GateOptions, GateRequirements, GateStatus, render, run, run_opts};

// --- F1 (ADR-062): change OpenSpec — источник покрытия `delta_guard` --------

/// Репо-фикстура F1: гейт-репозиторий + модель CMP-001 + разметка `OpenSpec`;
/// вторым коммитом «исполнитель» создаёт change `add-limits`, упоминающий
/// model/CMP-001.md, и правит сам файл. Возвращает sha базового коммита.
fn make_f1_repo(repo: &Path) -> String {
    make_gate_repo(repo);
    write_model(repo, &[("CMP-001", "")]);
    write_fixture(
        repo,
        "openspec/specs/payments/spec.md",
        "# payments\n\n### Requirement: Idempotent intake\n\
         The system SHALL accept a payment at most once per idempotency key.\n",
    );
    git(repo, &["add", "."]);
    git(repo, &["commit", "-q", "-m", "base"]);
    let base = git_stdout(repo, &["rev-parse", "HEAD"]).trim().to_string();
    write_fixture(
        repo,
        "openspec/changes/add-limits/proposal.md",
        "## Why\nНужны лимиты.\n\n## What Changes\n- model/CMP-001.md: зависимость от лимитов.\n",
    );
    write_fixture(
        repo,
        "openspec/changes/add-limits/tasks.md",
        "- [ ] 1.1 Обновить model/CMP-001.md\n",
    );
    write_fixture(
        repo,
        "model/CMP-001.md",
        "---\nid: CMP-001\ntype: cmp\ntitle: \"CMP-001\"\nstatus: \"designed\"\n\n---\n\n# CMP-001 v2\n",
    );
    git(repo, &["add", "."]);
    git(repo, &["commit", "-q", "-m", "работа исполнителя"]);
    base
}

/// F1 (ADR-062), приёмка через единый гейт: правка `model/` по активному
/// change `OpenSpec` проходит `delta_guard` без `DELTA.md`, и источник назван
/// в детали (`openspec:add-limits`). До фикса: FAIL «активных дельт нет».
#[test]
fn gate_delta_guard_counts_openspec_change() {
    let tmp = tempfile::tempdir().expect("tmp");
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).expect("mkdir");
    let base = make_f1_repo(&repo);
    let report = run(&repo, None, Some(&base), None, (1, 4)).expect("гейт");
    assert_eq!(
        status_of(&report, "delta_guard"),
        GateStatus::Pass,
        "{}",
        render(&report)
    );
    let component = report
        .components
        .iter()
        .find(|c| c.name == "delta_guard")
        .expect("составляющая");
    assert!(
        component.detail.contains("openspec:add-limits"),
        "источник покрытия назван: {}",
        component.detail
    );
}

/// F1 + ADR-055: с диапазоном прогона исполнителя change, созданный ВНУТРИ
/// диапазона, правку не узаконивает — `delta_guard` FAIL с `self_approved`
/// (та же дисциплина, что для самодельной дельты). Без диапазона — PASS
/// (см. тест выше): обычный CI по PR не меняется.
#[test]
fn gate_delta_guard_self_approved_openspec_inside_range() {
    let tmp = tempfile::tempdir().expect("tmp");
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).expect("mkdir");
    let base = make_f1_repo(&repo);
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
    assert_eq!(
        status_of(&report, "delta_guard"),
        GateStatus::Fail,
        "{}",
        render(&report)
    );
    let component = report
        .components
        .iter()
        .find(|c| c.name == "delta_guard")
        .expect("составляющая");
    assert!(
        component
            .findings
            .iter()
            .any(|f| f.rule.as_deref() == Some("self_approved")
                && f.message.contains("openspec:add-limits")),
        "находка self_approved с именем change: {:?}",
        component.findings
    );
    let text = render(&report);
    assert!(text.contains("self_approved"), "{text}");
}
