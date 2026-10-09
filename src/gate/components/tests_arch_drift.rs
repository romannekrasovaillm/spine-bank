//! Тесты составляющей `arch_drift` (K6, 0.3.14): рёбра графа «как построено»
//! против модели в едином гейте.
//!
//! Контракт составляющей:
//! - по умолчанию SKIP (дорогая проверка): включается записью `arch_drift` в
//!   `[gate.required]` маршрута (находки error) или `[gate.arch_drift]
//!   enabled = true` (находки warn);
//! - `undeclared-edge`: ребро добавлено в код между базой и рабочим деревом,
//!   но его нет в модели;
//! - `rejected-edge-present`: отклонённое `arch-diff reject` ребро (журнал
//!   `.arch-handoff/arch-diff-decisions.json`, пишет K5) осталось в коде —
//!   пока `grounds_hash` записи совпадает с текущими основаниями ребра.

use super::*;
use crate::control::Route;
use crate::gate::testkit::*;
use crate::gate::{GateOptions, GateOutcome, GateRequirements, GateStatus, render, run_opts};

/// Опции с включённой составляющей (флаг конфига, предупреждающий режим).
fn options_enabled() -> GateOptions {
    GateOptions {
        arch_drift: crate::config::ArchDriftConfig {
            enabled: true,
            ..crate::config::ArchDriftConfig::default()
        },
        ..GateOptions::default()
    }
}

/// Требования маршрута Fast + `arch_drift` (включение через `[gate.required]`).
fn required_with_arch_drift() -> GateRequirements {
    let mut req = GateRequirements::default();
    req.fast.push("arch_drift".to_string());
    req
}

/// Прогон гейта на фикстуре с заданными требованиями и опциями.
fn run_gate(dir: &Path, req: &GateRequirements, options: &GateOptions) -> crate::gate::GateReport {
    run_opts(dir, Some(Route::Fast), None, None, (1, 4), req, options).expect("гейт")
}

/// Репо с моделью из двух CMP (`skeleton/intake`, `skeleton/ledger`) без
/// `depends_on` между ними; `with_import` — писать ли в worktree импорт ядра
/// из приёма (ребро CMP-001 → CMP-004, в модели отсутствует).
fn make_edge_repo(dir: &Path, with_import: bool) {
    make_gate_repo(dir);
    write_fixture(
        dir,
        "model/CMP-001-intake.md",
        "---\nid: CMP-001\ntype: cmp\ntitle: Приём\nstatus: adopted\ncode_roots: [skeleton/intake]\n---\nПриём.\n",
    );
    write_fixture(
        dir,
        "model/CMP-004-ledger.md",
        "---\nid: CMP-004\ntype: cmp\ntitle: Ядро\nstatus: adopted\ncode_roots: [skeleton/ledger]\n---\nЯдро.\n",
    );
    write_fixture(dir, "skeleton/ledger/client.py", "def post():\n    pass\n");
    git(dir, &["add", "."]);
    git(dir, &["commit", "-q", "-m", "base"]);
    write_fixture(
        dir,
        "skeleton/intake/writer.py",
        if with_import {
            "from skeleton.ledger import client\n\ndef write():\n    client.post()\n"
        } else {
            "def write():\n    pass\n"
        },
    );
}

/// Журнал решений (схема `arch-be/arch-diff-decisions/v1`) с одним отказом
/// по ребру CMP-001 → CMP-004; `grounds` — основания, записываемые в запись
/// (`current` — актуальные, вычисляются из рабочего дерева; `stale` —
/// заведомо чужие).
enum Grounds {
    /// Актуальные основания ребра из рабочего дерева (как пишет K5).
    Current,
    /// Заведомо устаревшие основания (ребро изменилось после отказа).
    Stale,
}

/// Пишет журнал решений с отказом по ребру импорта CMP-001 → CMP-004.
fn write_reject_journal(dir: &Path, grounds: &Grounds) {
    let hash = match grounds {
        Grounds::Current => {
            let scan = crate::arch_diff::scan_worktree_with(
                dir,
                &crate::control::DiffGlobs::default(),
                &crate::arch_diff::ScanLimits::default(),
            )
            .expect("скан рабочего дерева");
            let edge = scan
                .graph
                .edges
                .iter()
                .find(|e| e.from == "CMP-001" && e.to == "CMP-004")
                .expect("ребро в графе рабочего дерева");
            crate::arch_diff::grounds_hash(edge)
        }
        Grounds::Stale => format!("sha256:{}", "0".repeat(64)),
    };
    let journal = crate::arch_diff::DecisionJournal {
        schema: crate::arch_diff::ARCH_DIFF_DECISIONS_SCHEMA.to_string(),
        entries: vec![crate::arch_diff::DecisionEntry {
            edge_id: "import:CMP-001->CMP-004".to_string(),
            decision: crate::arch_diff::Decision::Reject,
            reason: "законный прямой вызов, в модель не носить".to_string(),
            grounds_hash: hash,
            decided_at: "2026-10-08T12:00:00+03:00".to_string(),
            source: crate::arch_diff::DecisionSource::Human,
            delta: None,
        }],
    };
    let path = dir.join(crate::arch_diff::ARCH_DIFF_DECISIONS_PATH);
    std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    std::fs::write(&path, serde_json::to_string_pretty(&journal).expect("json"))
        .expect("запись журнала");
}

/// Находки составляющей `arch_drift` прогона.
fn arch_drift_findings(report: &crate::gate::GateReport) -> &[crate::gate::GateFinding] {
    &report
        .components
        .iter()
        .find(|c| c.name == "arch_drift")
        .expect("составляющая arch_drift")
        .findings
}

/// K6: по умолчанию составляющая SKIP с причиной (дорогая проверка) — даже
/// когда в рабочем дереве есть ребро вне модели, и вердикт не меняется.
#[test]
fn arch_drift_is_skip_by_default() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = tmp.path();
    make_edge_repo(dir, true);
    let report = run_gate(dir, &GateRequirements::default(), &GateOptions::default());
    let comp = report
        .components
        .iter()
        .find(|c| c.name == "arch_drift")
        .expect("составляющая arch_drift");
    assert_eq!(comp.status, GateStatus::Skip, "{}", render(&report));
    assert!(
        comp.detail.contains("не включена") && comp.detail.contains("[gate.required]"),
        "причина SKIP называет способы включения: {}",
        comp.detail
    );
    assert!(report.passed, "{}", render(&report));
}

/// K6: ребро «нет в модели» из диффа «база → рабочее дерево» — находка
/// `undeclared-edge`; по флагу конфига — warn (вердикт не ломает).
#[test]
fn arch_drift_undeclared_edge_warns_when_enabled_by_flag() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = tmp.path();
    make_edge_repo(dir, true);
    let report = run_gate(dir, &GateRequirements::default(), &options_enabled());
    let comp = report
        .components
        .iter()
        .find(|c| c.name == "arch_drift")
        .expect("составляющая arch_drift");
    assert_eq!(comp.status, GateStatus::Pass, "{}", render(&report));
    let finding = comp
        .findings
        .iter()
        .find(|f| f.rule.as_deref() == Some("undeclared-edge"))
        .unwrap_or_else(|| panic!("нет находки undeclared-edge: {:?}", comp.findings));
    assert_eq!(finding.severity, "warn", "{finding:?}");
    assert!(finding.message.contains("CMP-001"), "{}", finding.message);
    assert!(finding.message.contains("CMP-004"), "{}", finding.message);
    assert!(
        finding.message.contains("skeleton/intake/writer.py:1"),
        "основание файл:строка названо: {}",
        finding.message
    );
    assert!(report.passed, "warn не ломает вердикт: {}", render(&report));
}

/// K6: в `[gate.required]` маршрута то же ребро — error, гейт красный.
#[test]
fn arch_drift_in_required_breaks_the_gate_on_undeclared_edge() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = tmp.path();
    make_edge_repo(dir, true);
    let report = run_gate(dir, &required_with_arch_drift(), &GateOptions::default());
    let comp = report
        .components
        .iter()
        .find(|c| c.name == "arch_drift")
        .expect("составляющая arch_drift");
    assert_eq!(comp.status, GateStatus::Fail, "{}", render(&report));
    let finding = comp
        .findings
        .iter()
        .find(|f| f.rule.as_deref() == Some("undeclared-edge"))
        .expect("находка undeclared-edge");
    assert_eq!(finding.severity, "error", "{finding:?}");
    assert_eq!(report.outcome, GateOutcome::Fail, "{}", render(&report));
}

/// K6: отклонённое ребро (журнал K5), оставшееся в коде, — находка
/// `rejected-edge-present`, даже когда ребро закоммичено (в дифф его не
/// видно: сверка идёт по графу головы).
#[test]
fn arch_drift_rejected_edge_still_in_code_is_a_finding() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = tmp.path();
    make_edge_repo(dir, true);
    // Ребро закоммичено: дифф «база → дерево» пуст, находка — из журнала.
    git(dir, &["add", "."]);
    git(dir, &["commit", "-q", "-m", "edge committed"]);
    write_reject_journal(dir, &Grounds::Current);
    let report = run_gate(dir, &required_with_arch_drift(), &GateOptions::default());
    let findings = arch_drift_findings(&report);
    let finding = findings
        .iter()
        .find(|f| f.rule.as_deref() == Some("rejected-edge-present"))
        .unwrap_or_else(|| panic!("нет находки rejected-edge-present: {findings:?}"));
    assert_eq!(finding.severity, "error", "{finding:?}");
    assert!(
        finding.message.contains("законный прямой вызов"),
        "причина отказа названа: {}",
        finding.message
    );
    assert!(
        !findings
            .iter()
            .any(|f| f.rule.as_deref() == Some("undeclared-edge")),
        "ребро давно в коде — дифф его не показывает: {findings:?}"
    );
    assert_eq!(report.outcome, GateOutcome::Fail, "{}", render(&report));
}

/// K6: отклонённое ребро, УБРАННОЕ из кода, находкой не является — решение
/// исполнено (журнал против пустого места не краснит).
#[test]
fn arch_drift_rejected_edge_removed_from_code_is_clean() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = tmp.path();
    make_edge_repo(dir, true);
    write_reject_journal(dir, &Grounds::Current);
    // Ребро убрано из рабочего дерева (файл без импорта).
    write_fixture(dir, "skeleton/intake/writer.py", "def write():\n    pass\n");
    let report = run_gate(dir, &required_with_arch_drift(), &GateOptions::default());
    let findings = arch_drift_findings(&report);
    assert!(
        findings.is_empty(),
        "ребра нет ни в диффе, ни в голове: {findings:?}"
    );
    assert_eq!(
        status_of(&report, "arch_drift"),
        GateStatus::Pass,
        "{}",
        render(&report)
    );
}

/// K6: основания ребра изменились после отказа (`grounds_hash` не совпал) —
/// отклонение устарело: ребро снова предложение, для гейта это обычный
/// `undeclared-edge`, а НЕ `rejected-edge-present`.
#[test]
fn arch_drift_stale_rejection_becomes_a_proposal_again() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = tmp.path();
    make_edge_repo(dir, true);
    write_reject_journal(dir, &Grounds::Stale);
    let report = run_gate(dir, &required_with_arch_drift(), &GateOptions::default());
    let findings = arch_drift_findings(&report);
    assert!(
        !findings
            .iter()
            .any(|f| f.rule.as_deref() == Some("rejected-edge-present")),
        "устаревший отказ не действует: {findings:?}"
    );
    assert!(
        findings
            .iter()
            .any(|f| f.rule.as_deref() == Some("undeclared-edge") && f.severity == "error"),
        "ребро снова предложение: {findings:?}"
    );
}

/// K6: журнал решений есть, но не читается — сломанный вход, FAIL с причиной
/// (молчаливый «нет решений» скрыл бы подмену журнала отклонений).
#[test]
fn arch_drift_broken_decisions_journal_fails_when_enabled() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = tmp.path();
    make_edge_repo(dir, false);
    let path = dir.join(crate::arch_diff::ARCH_DIFF_DECISIONS_PATH);
    std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    std::fs::write(&path, "{битый json").expect("битый журнал");
    let report = run_gate(dir, &required_with_arch_drift(), &GateOptions::default());
    let comp = report
        .components
        .iter()
        .find(|c| c.name == "arch_drift")
        .expect("составляющая arch_drift");
    assert_eq!(comp.status, GateStatus::Fail, "{}", render(&report));
    assert!(
        comp.findings
            .iter()
            .any(|f| f.rule.as_deref() == Some("arch_diff_decisions_invalid")),
        "{:?}",
        comp.findings
    );
}

/// K6: не git-репозиторий — честный SKIP (дифф рёбер недоступен), даже когда
/// составляющая обязательна (тогда вердикт INCOMPLETE, не зелёный).
#[test]
fn arch_drift_skips_without_git() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = tmp.path().join("plain");
    std::fs::create_dir_all(&dir).expect("mkdir");
    write_fixture(
        &dir,
        "model/CMP-001.md",
        "---\nid: CMP-001\ntype: cmp\ntitle: Приём\nstatus: adopted\ncode_roots: [src]\n---\nПриём.\n",
    );
    let report = run_gate(&dir, &required_with_arch_drift(), &GateOptions::default());
    assert_eq!(status_of(&report, "arch_drift"), GateStatus::Skip);
    assert!(
        report.not_checked.iter().any(|n| n == "arch_drift"),
        "обязательная составляющая без входа — INCOMPLETE: {:?}",
        report.not_checked
    );
}

/// ADR-068 Am.2: `max_files` из `[gate.arch_drift]` доходит до сканера —
/// заниженный лимит даёт FAIL с диагностикой, называющей конфиг-ключ.
#[test]
fn arch_drift_max_files_limit_names_config_key() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = tmp.path().join("repo");
    make_edge_repo(&dir, true);
    let options = GateOptions {
        arch_drift: crate::config::ArchDriftConfig {
            enabled: true,
            max_files: Some(0),
            ..crate::config::ArchDriftConfig::default()
        },
        ..GateOptions::default()
    };
    let report = run_gate(&dir, &required_with_arch_drift(), &options);
    let comp = report
        .components
        .iter()
        .find(|c| c.name == "arch_drift")
        .expect("составляющая arch_drift");
    assert_eq!(comp.status, GateStatus::Fail, "{}", comp.detail);
    assert!(
        comp.detail.contains("[gate.arch_drift] max_files"),
        "диагностика обязана называть конфиг-ключ: {}",
        comp.detail
    );
}

/// ADR-068 Am.2: `ignore` из `[gate.arch_drift]` доходит до сканера —
/// исключённый `model/` исчезает из снимка, сверять рёбра не с чем (SKIP).
#[test]
fn arch_drift_ignore_reaches_scanner() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = tmp.path().join("repo");
    make_edge_repo(&dir, true);
    let options = GateOptions {
        arch_drift: crate::config::ArchDriftConfig {
            enabled: true,
            ignore: vec!["model/".to_string()],
            ..crate::config::ArchDriftConfig::default()
        },
        ..GateOptions::default()
    };
    let report = run_gate(&dir, &required_with_arch_drift(), &options);
    let comp = report
        .components
        .iter()
        .find(|c| c.name == "arch_drift")
        .expect("составляющая arch_drift");
    assert_eq!(comp.status, GateStatus::Skip, "{}", comp.detail);
    assert!(
        comp.detail.contains("нет читаемых сущностей"),
        "{}",
        comp.detail
    );
}
