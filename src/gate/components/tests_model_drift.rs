//! Тесты составляющей `model_drift` (C1, 0.3.14): дрейф «модель ↔ код»
//! ([`crate::model::drift_check`]) как часть единого гейта.
//!
//! Контракт составляющей:
//! - в `[gate.required]` маршрута по умолчанию её нет, и находки дрейфа —
//!   warn (вердикт не ломают); в required — error-находки дрейфа валят гейт;
//! - нет `model/` — честный SKIP;
//! - модель без единого `code_roots` — SKIP, но не молчит: паспорт (блок
//!   «заявлено, но механикой не проверяется») пишет «модель не привязана
//!   к коду: 0 из N CMP имеют `code_roots`».

use super::*;
use crate::control::Route;
use crate::gate::testkit::*;
use crate::gate::{GateOutcome, GateRequirements, GateStatus, render, run_with};

/// Требования маршрута Fast + `model_drift` (включение через `[gate.required]`).
fn with_model_drift() -> GateRequirements {
    let mut req = GateRequirements::default();
    req.fast.push("model_drift".to_string());
    req
}

/// Репо гейта с моделью из одного CMP; `code_roots` — по параметру:
/// `Some("services/billing")` — живой корень (каталог создаётся),
/// `Some("ghost")` — битый путь (error `code-root-missing`),
/// `None` — корней нет вовсе (модель не привязана к коду).
fn make_drift_repo(dir: &Path, roots: Option<&str>) {
    make_gate_repo(dir);
    let roots_line = roots.map_or(String::new(), |r| format!("code_roots: [{r}]\n"));
    write_fixture(
        dir,
        "model/CMP-001-billing.md",
        &format!(
            "---\nid: CMP-001\ntype: cmp\ntitle: Billing\nstatus: adopted\n{roots_line}---\nКомпонент.\n"
        ),
    );
    if let Some("services/billing") = roots {
        write_fixture(
            dir,
            "services/billing/Cargo.toml",
            "[package]\nname = \"billing\"\n",
        );
    }
    // Модель коммитим: незакоммиченная model/ — правка спайна без дельты
    // (delta_guard), а этот тест про model_drift.
    git(dir, &["add", "."]);
    git(dir, &["commit", "-q", "-m", "model"]);
}

/// C1: по умолчанию составляющей нет в `[gate.required]` ни одного маршрута,
/// и error-находка дрейфа (битый `code_roots`) предупреждает, но не ломает
/// вердикт (схема «warn → error»).
#[test]
fn model_drift_off_by_default_warns_without_breaking() {
    let defaults = GateRequirements::default();
    for route in [&defaults.fast, &defaults.standard, &defaults.critical] {
        assert!(
            !route.iter().any(|r| r == "model_drift"),
            "по умолчанию model_drift не обязательна: {route:?}"
        );
    }
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = tmp.path();
    make_drift_repo(dir, Some("ghost"));
    let report = run_with(
        dir,
        Some(Route::Fast),
        None,
        None,
        (1, 4),
        &GateRequirements::default(),
    )
    .expect("гейт");
    let comp = report
        .components
        .iter()
        .find(|c| c.name == "model_drift")
        .expect("составляющая model_drift");
    assert_eq!(
        comp.status,
        GateStatus::Pass,
        "warn-уровень: {}",
        render(&report)
    );
    let finding = comp
        .findings
        .iter()
        .find(|f| f.rule.as_deref() == Some("code-root-missing"))
        .expect("находка code-root-missing");
    assert_eq!(
        finding.severity, "warn",
        "вне [gate.required] error дрейфа понижается до warn: {:?}",
        comp.findings
    );
    assert!(
        report.passed,
        "вердикт не ломается без явного включения: {}",
        render(&report)
    );
    assert_eq!(report.outcome, GateOutcome::Pass);
}

/// C1: в `[gate.required]` маршрута error-находки дрейфа ломают гейт.
#[test]
fn model_drift_in_required_breaks_the_gate_on_errors() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = tmp.path();
    make_drift_repo(dir, Some("ghost"));
    let report = run_with(
        dir,
        Some(Route::Fast),
        None,
        None,
        (1, 4),
        &with_model_drift(),
    )
    .expect("гейт");
    let comp = report
        .components
        .iter()
        .find(|c| c.name == "model_drift")
        .expect("составляющая model_drift");
    assert_eq!(comp.status, GateStatus::Fail, "{}", render(&report));
    let finding = comp
        .findings
        .iter()
        .find(|f| f.rule.as_deref() == Some("code-root-missing"))
        .expect("находка code-root-missing");
    assert_eq!(finding.severity, "error", "{:?}", comp.findings);
    assert!(!report.passed, "{}", render(&report));
    assert_eq!(report.outcome, GateOutcome::Fail);
}

/// C1: warn-находки дрейфа (непокрытый манифест) и в required не ломают
/// вердикт — ломают только error.
#[test]
fn model_drift_required_stays_green_on_warn_only() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = tmp.path();
    make_drift_repo(dir, Some("services/billing"));
    // Каталог с манифестом вне code_roots — warn `uncovered-manifest`.
    write_fixture(
        dir,
        "services/notify/package.json",
        "{\"name\": \"notify\"}\n",
    );
    let report = run_with(
        dir,
        Some(Route::Fast),
        None,
        None,
        (1, 4),
        &with_model_drift(),
    )
    .expect("гейт");
    let comp = report
        .components
        .iter()
        .find(|c| c.name == "model_drift")
        .expect("составляющая model_drift");
    assert_eq!(comp.status, GateStatus::Pass, "{}", render(&report));
    let finding = comp
        .findings
        .iter()
        .find(|f| f.rule.as_deref() == Some("uncovered-manifest"))
        .expect("находка uncovered-manifest");
    assert_eq!(finding.severity, "warn", "{:?}", comp.findings);
    assert!(report.passed, "{}", render(&report));
}

/// C1: без каталога `model/` — честный SKIP (входа нет), а не «проверено».
#[test]
fn model_drift_skips_without_model_dir() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = tmp.path();
    make_gate_repo(dir);
    let report = run_with(
        dir,
        Some(Route::Fast),
        None,
        None,
        (1, 4),
        &GateRequirements::default(),
    )
    .expect("гейт");
    assert_eq!(status_of(&report, "model_drift"), GateStatus::Skip);
}

/// C1: модель без единого `code_roots` — SKIP, который НЕ молчит: паспорт
/// (блок 2 «заявлено, но механикой не проверяется») пишет «модель не
/// привязана к коду: 0 из N CMP имеют `code_roots`»; на маршруте, где
/// составляющая обязательна, тот же SKIP — INCOMPLETE (exit 3), а не зелёный.
#[test]
fn model_drift_unbound_model_is_named_in_the_passport() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = tmp.path();
    make_drift_repo(dir, None);
    let report = run_with(
        dir,
        Some(Route::Fast),
        None,
        None,
        (1, 4),
        &GateRequirements::default(),
    )
    .expect("гейт");
    let comp = report
        .components
        .iter()
        .find(|c| c.name == "model_drift")
        .expect("составляющая model_drift");
    assert_eq!(comp.status, GateStatus::Skip, "{}", render(&report));
    assert!(
        comp.detail
            .contains("модель не привязана к коду: 0 из 1 CMP имеют code_roots"),
        "{}",
        comp.detail
    );
    // Паспорт: блок «заявлено, но механикой не проверяется» несёт пометку.
    let passport = crate::passport::Passport::build_labelled(&report, dir, "кейс");
    let claim = passport
        .claimed
        .iter()
        .find(|c| c.source == "model_drift")
        .expect("утверждение model_drift в блоке 2 паспорта");
    assert!(
        claim
            .text
            .contains("модель не привязана к коду: 0 из 1 CMP имеют code_roots"),
        "{claim:?}"
    );
    let page = passport.render();
    assert!(
        page.contains("модель не привязана к коду: 0 из 1 CMP имеют code_roots"),
        "{page}"
    );
    // Тот же SKIP на маршруте, где составляющая обязательна, — INCOMPLETE.
    let required = run_with(
        dir,
        Some(Route::Fast),
        None,
        None,
        (1, 4),
        &with_model_drift(),
    )
    .expect("гейт");
    assert_eq!(
        required.outcome,
        GateOutcome::Incomplete,
        "{}",
        render(&required)
    );
    assert!(
        required.not_checked.iter().any(|n| n == "model_drift"),
        "{:?}",
        required.not_checked
    );
}
