//! Тесты составляющей `decision_quality` (Н7/E1.4, ADR-042): качество решений
//! как составляющая гейта — отчёты рубрики, досье, квалификация судей.
//! Вынесены из `components/tests.rs` чистым перемещением под границу
//! `prod_file_length_limit` (C-33); имена тестов прежние.

use super::tests::{make_quality_repo, with_quality};
use super::*;
use crate::gate::testkit::*;

use crate::gate::{
    GateOptions, GateOutcome, GateReport, GateRequirements, GateStatus, render, run, run_with,
};

// --- E1.4: отчёты по досье в decision_quality --------------------------

/// Репозиторий с отчётом смысловой рубрики по досье (`code_vs_spine`) и,
/// опционально, сырым ответом судьи под slug'ом досье. Каталога `docs/adr`
/// здесь нет намеренно: составляющая обязана судить решение о коде и без
/// принятых ADR.
fn write_code_pack_report(dir: &Path, raw: Option<(&str, &str)>) {
    make_gate_repo(dir);
    let reports = dir.join(crate::rubric::RUBRIC_REPORTS_DIR);
    std::fs::create_dir_all(&reports).expect("mkdir reports");
    let artifact = serde_json::json!({
        "schema": crate::rubric::RUBRIC_REPORT_SCHEMA,
        "rubric": "code_invariant_conformance",
        "judge_model": "judge-x",
        "author_model": "agent-y",
        "weighted_total": 4.5,
        "verdict": "OK",
        "unstable": false,
        "evidence_not_found": 0,
        "judged_at": "2026-09-25T10:00:00+00:00",
        "pack_kind": "code_vs_spine",
        "subject": "src/control.rs",
        "pack_sha256": "a".repeat(64),
        "inputs": [{"path": "src/control.rs", "sha256": "b".repeat(64), "role": "subject"}],
    });
    std::fs::write(
        reports.join("control--code_vs_spine.json"),
        serde_json::to_string_pretty(&artifact).expect("json"),
    )
    .expect("write pack report");
    if let Some((text, recorded_sha)) = raw {
        let raw_dir = dir
            .join(crate::judge::RUBRIC_RAW_DIR)
            .join("control--code_vs_spine");
        std::fs::create_dir_all(&raw_dir).expect("mkdir raw");
        let answer = serde_json::json!({
            "schema": crate::judge::RUBRIC_RAW_SCHEMA,
            "rubric": "code_invariant_conformance",
            "sample": 1,
            "judge_model": "judge-x",
            "sha256": recorded_sha,
            "dropped": false,
            "text": text,
            "saved_at": "2026-09-25T10:00:00+00:00",
        });
        std::fs::write(
            raw_dir.join(crate::judge::raw_file_name(1)),
            serde_json::to_string_pretty(&answer).expect("json"),
        )
        .expect("write raw");
    }
}

/// E1.4: отчёт по досье без сырых ответов судьи — находка гейта
/// (`rubric_report_unreproducible`, warn: read-only контур MCP файлов не
/// пишет, и провалом это краснило бы настройку, а не решение).
#[test]
fn decision_quality_flags_unreproducible_code_report() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = tmp.path();
    write_code_pack_report(dir, None);
    let report = run_with(
        dir,
        Some(Route::Fast),
        None,
        None,
        (1, 4),
        &with_quality(Route::Fast),
    )
    .expect("gate");
    let comp = report
        .components
        .iter()
        .find(|c| c.name == "decision_quality")
        .expect("comp");
    assert!(
        comp.detail.contains("отчётов по досье: 1"),
        "отчёт по досье посчитан: {}",
        comp.detail
    );
    assert!(
        comp.findings
            .iter()
            .any(|f| f.rule.as_deref() == Some("rubric_report_unreproducible")),
        "{:?}",
        comp.findings
    );
    assert!(
        comp.not_verified
            .iter()
            .any(|n| n.contains("невоспроизводима") && n.contains("(1)")),
        "отчёт по досье назван в примечании с числом: {:?}",
        comp.not_verified
    );
    assert_eq!(
        status_of(&report, "decision_quality"),
        GateStatus::Pass,
        "warn не краснит составляющую: {}",
        render(&report)
    );
}

/// Вердикт «судья = автор» подтверждается примечанием паспорта: находка без
/// примечания читалась бы как «всё в порядке».
#[test]
fn decision_quality_notes_name_judge_is_author() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = tmp.path();
    make_quality_repo(dir, Some(4.5), Some("judge-x"));
    let note_present = |author: &str| {
        patch_report(dir, "ADR-001-reshenie.json", |v| {
            v["author_model"] = serde_json::json!(author);
        });
        let report = run_quality_on(dir, Route::Fast, false);
        report
            .components
            .iter()
            .find(|c| c.name == "decision_quality")
            .expect("comp")
            .not_verified
            .iter()
            .any(|n| n.contains("независимость судьи не подтверждена"))
    };
    assert!(note_present("judge-x"), "судья = автор — примечание есть");
    assert!(!note_present("agent-y"), "разные модели — примечания нет");
}

/// Второй судья разошёлся: находка, счётчик и режим «решение человека».
#[test]
fn decision_quality_second_judge_disagreement_is_reported() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = tmp.path();
    write_code_pack_report(dir, None);
    let set_second = |agreement: bool, differences: Vec<String>| {
        patch_report(dir, "control--code_vs_spine.json", |v| {
            v["second_judge"] = serde_json::json!({
                "model": "judge-y",
                "report": "reports/rubric/control--code_vs_spine--second.json",
                "weighted_total": 2.0,
                "agreement": agreement,
                "differences": differences,
            });
        });
        let report = run_quality_on(dir, Route::Fast, false);
        let comp = report
            .components
            .iter()
            .find(|c| c.name == "decision_quality")
            .expect("comp");
        (
            comp.findings
                .iter()
                .any(|f| f.rule.as_deref() == Some("judge_disagreement")),
            comp.detail.clone(),
            comp.status,
        )
    };
    let (finding, detail, status) = set_second(false, vec!["критерий X: 3 против 5".to_string()]);
    assert!(finding, "расхождение названо: {detail}");
    assert_eq!(status, GateStatus::Skip, "решение человека: {detail}");
    assert!(
        detail.contains("механика не подтверждает: 1"),
        "счётчик расхождений: {detail}"
    );
    let (finding, detail, _) = set_second(true, Vec::new());
    assert!(!finding, "согласие судей — не расхождение: {detail}");
}

/// E1.4: подмена сохранённого ответа судьи по рубрике кода — находка
/// гейта с провалом составляющей, как и у отчёта по документу.
#[test]
fn decision_quality_flags_tampered_code_raw_answer() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = tmp.path();
    let text = "{\"scores\": [], \"verdict\": \"ok\"}";
    write_code_pack_report(dir, Some((text, &"f".repeat(64))));
    let report = run_with(
        dir,
        Some(Route::Fast),
        None,
        None,
        (1, 4),
        &with_quality(Route::Fast),
    )
    .expect("gate");
    let comp = report
        .components
        .iter()
        .find(|c| c.name == "decision_quality")
        .expect("comp");
    assert!(
        comp.findings
            .iter()
            .any(|f| f.rule.as_deref() == Some("rubric_raw_tampered")),
        "{:?}",
        comp.findings
    );
    assert_eq!(status_of(&report, "decision_quality"), GateStatus::Fail);
}

/// E2: вход документа помечен prompt-инъекцией — «проверить нельзя, нужен
/// человек». Составляющая уходит в SKIP с находкой `rubric_input_injection`,
/// вердикт гейта — INCOMPLETE (exit 3): ни PASS (судья мог подчиниться
/// строке), ни FAIL (документ ничего не нарушил).
#[test]
fn decision_quality_input_injection_makes_gate_incomplete() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = tmp.path();
    make_quality_repo(dir, Some(4.5), Some("judge-x"));
    let path = dir
        .join(crate::rubric::RUBRIC_REPORTS_DIR)
        .join("ADR-001-reshenie.json");
    let mut artifact: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).expect("отчёт")).expect("JSON");
    artifact["input_injection_lines"] = serde_json::json!([2]);
    std::fs::write(
        &path,
        serde_json::to_string_pretty(&artifact).expect("json"),
    )
    .expect("write report");
    let report = run_with(
        dir,
        Some(Route::Fast),
        None,
        None,
        (1, 4),
        &with_quality(Route::Fast),
    )
    .expect("gate");
    let comp = report
        .components
        .iter()
        .find(|c| c.name == "decision_quality")
        .expect("comp");
    assert_eq!(comp.status, GateStatus::Skip, "{}", render(&report));
    assert!(
        comp.findings
            .iter()
            .any(|f| f.rule.as_deref() == Some("rubric_input_injection")),
        "{:?}",
        comp.findings
    );

    assert_eq!(
        report.outcome,
        GateOutcome::Incomplete,
        "{}",
        render(&report)
    );
}

/// Патч отчёта `make_quality_repo` полями E3: доля невалидных сэмплов и
/// метки критериев. Возвращает путь отчёта.
fn patch_quality_report(dir: &Path, invalid_ratio: Option<f64>, flags: &[&str]) -> PathBuf {
    let path = dir
        .join(crate::rubric::RUBRIC_REPORTS_DIR)
        .join("ADR-001-reshenie.json");
    let mut artifact: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).expect("отчёт")).expect("JSON");
    if let Some(ratio) = invalid_ratio {
        artifact["invalid_samples_ratio"] = serde_json::json!(ratio);
    }
    if !flags.is_empty() {
        artifact["scores"] = serde_json::json!([{
            "criterion_id": "context",
            "weight": 1.0,
            "score": 4,
            "rationale": "цитата",
            "samples": [4],
            "stdev": 0.0,
            "flags": flags,
            "evidence_unconfirmed_ratio": 0.0,
            "invalid_samples": 0,
            "checked": [],
        }]);
    }
    std::fs::write(
        &path,
        serde_json::to_string_pretty(&artifact).expect("json"),
    )
    .expect("write report");
    path
}

/// Пройденная квалификация судьи `judge-x` на рубрике
/// `code_invariant_conformance` (рубрика по досье): на блокирующем маршруте
/// её требует E6.3, когда проект включил `require_qualified_judge`.
fn write_passing_qualification(dir: &Path) {
    let rubric = crate::rubric::load(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("assets/rubrics/code_invariant_conformance.yaml"),
    )
    .expect("рубрика code_invariant_conformance");
    let set = crate::rubric::QualificationSet {
        dir: std::path::PathBuf::from("/набор"),
        cases: Vec::new(),
        sha256: "e".repeat(64),
    };
    let outcome = |truth: crate::rubric::Truth| crate::rubric::CaseOutcome {
        file: "code/x.py".to_string(),
        class: "ignored_key".to_string(),
        truth,
        decision: Some(match truth {
            crate::rubric::Truth::Defective => crate::rubric::RubricDecision::Fail,
            crate::rubric::Truth::Clean => crate::rubric::RubricDecision::Pass,
        }),
        weighted_total: 4.0,
        flags: Vec::new(),
        error: None,
    };
    let report = crate::rubric::build_qualification_report(
        &rubric,
        "judge-x",
        &set,
        vec![
            outcome(crate::rubric::Truth::Defective),
            outcome(crate::rubric::Truth::Clean),
        ],
        1,
    );
    assert!(report.passed, "{:?}", report.failures);
    crate::rubric::write_qualification(dir, &report).expect("квалификация");
}

/// Прогон гейта с настройками допуска судьи (E6.3).
fn run_quality_on(dir: &Path, route: Route, require_qualified: bool) -> GateReport {
    let mut options = GateOptions::default();
    options.decision_quality.require_qualified_judge = require_qualified;
    options.route = Some(route);
    crate::gate::verdict::run_inner(
        dir,
        Some(route),
        None,
        None,
        (1, 4),
        &with_quality(route),
        &options,
    )
    .expect("гейт")
}

/// E6.3: с включённым допуском судья без квалификации на Critical не
/// проходит (SKIP → INCOMPLETE), после пройденной — проходит; с выключенным
/// флагом проверки нет (поведение 0.3.8).
#[test]
fn decision_quality_requires_qualification_when_enabled() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = tmp.path();
    // Отчёт по досье рубрики кода: эталонный набор существует именно для неё.
    write_code_pack_report(dir, None);
    let before = run_quality_on(dir, Route::Critical, true);
    assert_eq!(
        status_of(&before, "decision_quality"),
        GateStatus::Skip,
        "{}",
        render(&before)
    );
    let comp = before
        .components
        .iter()
        .find(|c| c.name == "decision_quality")
        .expect("comp");
    assert!(
        comp.findings
            .iter()
            .any(|f| f.rule.as_deref() == Some("judge_unqualified")),
        "{:?}",
        comp.findings
    );
    write_passing_qualification(dir);
    let after = run_quality_on(dir, Route::Critical, true);
    assert_eq!(
        status_of(&after, "decision_quality"),
        GateStatus::Pass,
        "{}",
        render(&after)
    );
    let off = run_quality_on(dir, Route::Critical, false);
    assert_eq!(
        status_of(&off, "decision_quality"),
        GateStatus::Pass,
        "без флага проверки нет: {}",
        render(&off)
    );
}

/// E3.2: доля сэмплов судьи с баллом вне шкалы выше порога — суждению
/// верить нельзя: SKIP с находкой `rubric_invalid_samples`, вердикт
/// INCOMPLETE (exit 3), решение за человеком.
#[test]
fn decision_quality_invalid_samples_above_threshold_escalates() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = tmp.path();
    make_quality_repo(dir, Some(4.5), Some("judge-x"));
    patch_quality_report(dir, Some(0.8), &[]);
    let report = run_with(
        dir,
        Some(Route::Fast),
        None,
        None,
        (1, 4),
        &with_quality(Route::Fast),
    )
    .expect("gate");
    let comp = report
        .components
        .iter()
        .find(|c| c.name == "decision_quality")
        .expect("comp");
    assert_eq!(comp.status, GateStatus::Skip, "{}", render(&report));
    assert!(
        comp.findings
            .iter()
            .any(|f| f.rule.as_deref() == Some("rubric_invalid_samples") && f.severity == "error"),
        "{:?}",
        comp.findings
    );
    assert_eq!(
        report.outcome,
        GateOutcome::Incomplete,
        "{}",
        render(&report)
    );
}

/// E3.2, контрпроба: доля ниже порога — только предупреждение, вердикт
/// остаётся PASS: один сбойный сэмпл не повод блокировать решение.
#[test]
fn decision_quality_invalid_samples_below_threshold_is_warn() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = tmp.path();
    make_quality_repo(dir, Some(4.5), Some("judge-x"));
    patch_quality_report(dir, Some(0.25), &[]);
    let report = run_with(
        dir,
        Some(Route::Fast),
        None,
        None,
        (1, 4),
        &with_quality(Route::Fast),
    )
    .expect("gate");
    let comp = report
        .components
        .iter()
        .find(|c| c.name == "decision_quality")
        .expect("comp");
    assert_eq!(comp.status, GateStatus::Pass, "{}", render(&report));
    assert!(
        comp.findings
            .iter()
            .any(|f| f.rule.as_deref() == Some("rubric_invalid_samples") && f.severity == "warn"),
        "{:?}",
        comp.findings
    );
    assert_eq!(report.outcome, GateOutcome::Pass);
}

/// E3.3: `evidence_partial` на Critical — решение за человеком (SKIP →
/// INCOMPLETE), на Fast — предупреждение с вердиктом PASS.
#[test]
fn decision_quality_evidence_partial_follows_route() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = tmp.path();
    make_quality_repo(dir, Some(4.5), Some("judge-x"));
    write_passing_qualification(dir);
    write_passing_qualification(dir);
    patch_quality_report(dir, None, &["evidence_partial"]);
    // Critical: оговорку судьи принимает человек.
    let critical = run_with(
        dir,
        Some(Route::Critical),
        None,
        None,
        (1, 4),
        &with_quality(Route::Critical),
    )
    .expect("gate critical");
    let comp = critical
        .components
        .iter()
        .find(|c| c.name == "decision_quality")
        .expect("comp");
    assert_eq!(comp.status, GateStatus::Skip, "{}", render(&critical));
    assert!(
        comp.findings
            .iter()
            .any(|f| f.rule.as_deref() == Some("rubric_evidence_partial")),
        "{:?}",
        comp.findings
    );
    assert_eq!(
        critical.outcome,
        GateOutcome::Incomplete,
        "{}",
        render(&critical)
    );
    // Fast: то же состояние — предупреждение, вердикт PASS.
    let fast = run_with(
        dir,
        Some(Route::Fast),
        None,
        None,
        (1, 4),
        &with_quality(Route::Fast),
    )
    .expect("gate fast");
    let comp = fast
        .components
        .iter()
        .find(|c| c.name == "decision_quality")
        .expect("comp");
    assert_eq!(comp.status, GateStatus::Pass, "{}", render(&fast));
    assert_eq!(fast.outcome, GateOutcome::Pass, "{}", render(&fast));
}

/// Реестра нет нигде — сообщение «создайте каркас»; явный путь вне
/// репозитория — «нечего прогонять»: это разные диагнозы.
#[test]
fn fitness_message_distinguishes_absent_contour_from_wrong_path() {
    let tmp = tempfile::tempdir().expect("tmp");
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).expect("mkdir");
    let exec = crate::cmd_trust::ExecPolicy {
        no_exec: false,
        trust_file: None,
    };
    let implicit = ConstraintsPath {
        path: repo.join("CONSTRAINTS.yaml"),
        explicit: false,
        drift: None,
    };
    let component = component_fitness(
        &repo,
        &implicit,
        &exec,
        &crate::config::OverridesConfig::default(),
        None,
        &std::collections::BTreeMap::new(),
    );
    assert_eq!(component.status, GateStatus::Skip);
    assert!(
        component.detail.contains("создайте каркас"),
        "{}",
        component.detail
    );
    let explicit = ConstraintsPath {
        path: repo.join("CONSTRAINTS.yaml"),
        explicit: true,
        drift: None,
    };
    let component = component_fitness(
        &repo,
        &explicit,
        &exec,
        &crate::config::OverridesConfig::default(),
        None,
        &std::collections::BTreeMap::new(),
    );
    assert_eq!(component.status, GateStatus::Skip);
    assert!(
        component.detail.contains("нечего прогонять"),
        "{}",
        component.detail
    );
}

/// Граница вердикта `fitness` считает долю правил «по тексту»: два
/// правила, из которых одно исполняемое, — это «1 из 2».
#[test]
fn mention_rule_notes_counts_text_rules_against_total() {
    let tmp = tempfile::tempdir().expect("tmp");
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).expect("mkdir");
    let registry = repo.join("CONSTRAINTS.yaml");
    std::fs::write(
            &registry,
            "rules:\n  - name: spine_present\n    type: file_exists\n    path: \"ARCHITECTURE-SPINE.md\"\n    severity: error\n  - name: tests_run\n    type: command_succeeds\n    command: 'true'\n    severity: error\n",
        )
        .expect("registry");
    let notes = mention_rule_notes(&repo, &registry);
    assert_eq!(notes.len(), 1, "{notes:?}");
    assert!(notes[0].contains("— 1 из 2"), "{}", notes[0]);
    assert!(
        notes[0].contains("исполняемых проверок поведения: 1"),
        "{}",
        notes[0]
    );
    // Все правила исполняемые — примечания нет вовсе.
    std::fs::write(
            &registry,
            "rules:\n  - name: tests_run\n    type: command_succeeds\n    command: 'true'\n    severity: error\n",
        )
        .expect("registry");
    assert_eq!(
        mention_rule_notes(&repo, &registry),
        [] as [std::string::String; 0]
    );
}

/// Непокрытые инварианты называются поимённо, а сверх потолка имён —
/// счётчиком «и ещё N»; ровно потолок — счётчика нет.
#[test]
fn ads_without_behaviour_names_uncovered_and_counts_the_rest() {
    let tmp = tempfile::tempdir().expect("tmp");
    let repo = tmp.path().join("repo");
    let model = repo.join("model");
    std::fs::create_dir_all(&model).expect("mkdir model");
    let write_rules = |count: usize| {
        let mut registry = String::from("rules:\n");
        for k in 1..=count {
            let _ = write!(
                registry,
                "  - id: C-{k:03}\n    name: text_rule_{k}\n    type: must_contain\n    glob: '**/*.py'\n    pattern: 'x'\n    severity: error\n    ad: AD-{k:03}\n"
            );
        }
        std::fs::write(repo.join("CONSTRAINTS.yaml"), registry).expect("registry");
    };
    // MAX_AD_NAMES + 2 инварианта, каждый проверяется текстовым правилом.
    for k in 1..=(MAX_AD_NAMES + 2) {
        std::fs::write(
                model.join(format!("AD-{k:03}.md")),
                format!(
                    "---\nid: AD-{k:03}\ntype: ad\ntitle: \"AD {k}\"\nstatus: \"ADOPTED\"\nverified_by: [C-{k:03}]\n---\n\nТело.\n"
                ),
            )
            .expect("ad");
    }
    write_rules(MAX_AD_NAMES + 2);
    let line = ads_without_behaviour(&repo).expect("инварианты без поведения есть");
    assert!(line.contains("и ещё 2"), "{line}");
    assert!(line.contains("AD-001"), "{line}");
    // Непокрытых ровно MAX_AD_NAMES — счётчика нет: первые два
    // инварианта закрыты исполняемыми правилами, остальные восемь —
    // по-прежнему судят по тексту.
    let mut registry = String::from("rules:\n");
    for k in 1..=2 {
        let _ = write!(
            registry,
            "  - id: C-{k:03}\n    name: behaviour_rule_{k}\n    type: command_succeeds\n    command: 'true'\n    severity: error\n    ad: AD-{k:03}\n"
        );
    }
    for k in 3..=(MAX_AD_NAMES + 2) {
        let _ = write!(
            registry,
            "  - id: C-{k:03}\n    name: text_rule_{k}\n    type: must_contain\n    glob: '**/*.py'\n    pattern: 'x'\n    severity: error\n    ad: AD-{k:03}\n"
        );
    }
    std::fs::write(repo.join("CONSTRAINTS.yaml"), registry).expect("registry");
    let line = ads_without_behaviour(&repo).expect("есть непокрытые");
    assert!(line.contains("AD-003"), "{line}");
    assert!(!line.contains("и ещё"), "{line}");
}

/// Сводка покрытия `delta_guard`: пустые упоминания не печатаются, а
/// сверх потолка записей идёт счётчик.
#[test]
fn coverage_note_skips_empty_mentions_and_caps_entries() {
    let report = |mentions: Vec<(String, Vec<String>)>| delta::GuardReport {
        base: "HEAD".to_string(),
        changed: mentions.len(),
        changed_files: Vec::new(),
        protected_changed: mentions.iter().map(|(f, _)| f.clone()).collect(),
        covered: Vec::new(),
        violations: Vec::new(),
        self_approved: Vec::new(),
        passed: true,
        active_deltas: 1,
        active_changes: 0,
        sources: vec!["spine".to_string()],
        archived_in_range: Vec::new(),
        mentions,
        reasons: Vec::new(),
    };
    let deltas = |names: &[&str]| names.iter().map(|n| (*n).to_string()).collect::<Vec<_>>();
    // Пустое упоминание в первых трёх не превращается в «file ← ».
    let note = coverage_note(&report(vec![
        ("empty.md".to_string(), Vec::new()),
        ("a.md".to_string(), deltas(&["d1"])),
    ]));
    assert_eq!(note, "a.md ← 'd1'", "{note}");
    // Ровно потолок — счётчика нет; сверх — «и ещё 1».
    let note = coverage_note(&report(vec![
        ("a.md".to_string(), deltas(&["d1"])),
        ("b.md".to_string(), deltas(&["d1"])),
        ("c.md".to_string(), deltas(&["d1"])),
    ]));
    assert!(!note.contains("и ещё"), "{note}");
    let note = coverage_note(&report(vec![
        ("a.md".to_string(), deltas(&["d1"])),
        ("b.md".to_string(), deltas(&["d1"])),
        ("c.md".to_string(), deltas(&["d1"])),
        ("d.md".to_string(), deltas(&["d1"])),
    ]));
    assert!(note.contains("… и ещё 1"), "{note}");
}

/// Защищённая правка без активных дельт: FAIL и находка говорит именно
/// «активных дельт нет», а не «не упоминается ни в одной из 0».
#[test]
fn delta_guard_names_absence_of_active_deltas() {
    let tmp = tempfile::tempdir().expect("tmp");
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).expect("mkdir");
    make_gate_repo(&repo);
    std::fs::write(repo.join("ARCHITECTURE-SPINE.md"), "# Spine v2\n").expect("edit");
    let report = run(&repo, None, None, None, (1, 4)).expect("гейт");
    assert_eq!(status_of(&report, "delta_guard"), GateStatus::Fail);
    let component = report
        .components
        .iter()
        .find(|c| c.name == "delta_guard")
        .expect("составляющая");
    assert!(
        component.detail.contains("активных дельт: 0"),
        "{}",
        component.detail
    );
    assert!(
        component
            .findings
            .iter()
            .any(|f| f.message.contains("активных дельт нет")),
        "{:?}",
        component.findings
    );
}

/// Только warn-находки спайна — это PASS с «error: 0», а не FAIL.
#[test]
fn spine_lint_warn_only_passes_with_zero_errors() {
    let tmp = tempfile::tempdir().expect("tmp");
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).expect("mkdir");
    std::fs::write(
            repo.join("ARCHITECTURE-SPINE.md"),
            "# Spine\n\n## AD-1: Идемпотентность\n\n- **Binds:** Processor.authorize\n- **Prevents:** двойное списание\n- **Rule:** ключ из команды; TODO уточнить формулировку\n",
        )
        .expect("spine");
    let component = component_spine_lint(&repo);
    assert_eq!(component.status, GateStatus::Pass, "{}", component.detail);
    assert!(
        component.detail.contains("error: 0"),
        "{}",
        component.detail
    );
    assert!(
        component.detail.contains("находок: 1"),
        "warn-находка видна: {}",
        component.detail
    );
}

/// Каталог `docs/spec` без markdown-спек — SKIP, а не «проверили и чисто».
#[test]
fn sensors_skip_when_spec_dir_has_no_markdown() {
    let tmp = tempfile::tempdir().expect("tmp");
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(repo.join("docs/spec")).expect("mkdir spec");
    std::fs::write(repo.join("docs/spec/notes.txt"), "не спека\n").expect("txt");
    let component = component_sensors(&repo);
    assert_eq!(component.status, GateStatus::Skip, "{}", component.detail);
    assert!(
        component.detail.contains("нет *.md"),
        "{}",
        component.detail
    );
}

/// Пакеты доказательств: файл в `changes/` — не пакет, `changes/archive/`
/// — архив, а не активная дельта; корневой и дельта-пакеты видны.
#[test]
fn evidence_bundle_dirs_selects_only_real_bundles() {
    let tmp = tempfile::tempdir().expect("tmp");
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(repo.join("changes/one")).expect("mkdir one");
    std::fs::create_dir_all(repo.join("changes/archive")).expect("mkdir archive");
    std::fs::write(repo.join("changes/one/EVIDENCE.yaml"), "bundle: 1\n").expect("bundle");
    std::fs::write(repo.join("changes/archive/EVIDENCE.yaml"), "bundle: old\n").expect("arch");
    std::fs::write(repo.join("changes/README.md"), "не пакет\n").expect("file");
    std::fs::write(repo.join("EVIDENCE.yaml"), "bundle: root\n").expect("root");
    let found = evidence_bundle_dirs(&repo);
    assert_eq!(found.len(), 2, "{found:?}");
    assert!(found.contains(&repo), "{found:?}");
    assert!(found.contains(&repo.join("changes/one")), "{found:?}");
}

/// Ровно пороговый балл — не «ниже порога»: сравнение строгое, иначе
/// решение на самой планке краснело бы как недотянувшее.
#[test]
fn decision_quality_score_exactly_at_threshold_is_not_low() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = tmp.path();
    make_quality_repo(dir, Some(3.5), Some("judge-x"));
    let report = run_with(
        dir,
        Some(Route::Fast),
        None,
        None,
        (1, 4),
        &with_quality(Route::Fast),
    )
    .expect("gate");
    let comp = report
        .components
        .iter()
        .find(|c| c.name == "decision_quality")
        .expect("comp");
    assert!(
        !comp
            .findings
            .iter()
            .any(|f| f.rule.as_deref() == Some("decision_quality_low")),
        "{:?}",
        comp.findings
    );
    assert_eq!(comp.status, GateStatus::Pass, "{}", render(&report));
}

/// Доля невалидных сэмплов ровно на пороге — ещё не эскалация: строгое
/// «больше порога» отделяет шумную выборку от сломанной.
#[test]
fn decision_quality_invalid_ratio_at_threshold_does_not_escalate() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = tmp.path();
    make_quality_repo(dir, Some(4.5), Some("judge-x"));
    patch_quality_report(dir, Some(0.5), &[]);
    let report = run_with(
        dir,
        Some(Route::Fast),
        None,
        None,
        (1, 4),
        &with_quality(Route::Fast),
    )
    .expect("gate");
    let comp = report
        .components
        .iter()
        .find(|c| c.name == "decision_quality")
        .expect("comp");
    assert_ne!(
        comp.status,
        GateStatus::Skip,
        "на пороге вердикт не выносится человеку: {}",
        render(&report)
    );
    assert!(
        !comp
            .findings
            .iter()
            .any(|f| f.severity == "error" && f.rule.as_deref() == Some("rubric_invalid_samples")),
        "{:?}",
        comp.findings
    );
}

/// Досье-отчёт без каталога `docs/adr` — повод работать, а не SKIP:
/// оценка кода не должна выпадать из составляющей только из-за
/// отсутствия ADR.
#[test]
fn decision_quality_runs_without_adr_dir_when_dossier_report_exists() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = tmp.path();
    write_code_pack_report(dir, None);
    assert!(!dir.join("docs/adr").is_dir(), "каталога ADR нет");
    let report = run_with(
        dir,
        Some(Route::Fast),
        None,
        None,
        (1, 4),
        &with_quality(Route::Fast),
    )
    .expect("gate");
    let comp = report
        .components
        .iter()
        .find(|c| c.name == "decision_quality")
        .expect("comp");
    assert_ne!(
        comp.status,
        GateStatus::Skip,
        "досье есть — составляющая работает: {}",
        render(&report)
    );
}

/// Счётчики составляющей `nfr`: число прогнанных проверок и разбивка
/// находок по критичности — это то, по чему читатель вердикта понимает,
/// что именно посчитано.
#[test]
fn nfr_counts_checks_and_findings_by_severity() {
    let tmp = tempfile::tempdir().expect("tmp");
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(repo.join("model")).expect("mkdir model");
    // Перерасход бюджета (error) и непокрытый hop (warn).
    for (name, fm) in [
        (
            "INT-001.md",
            "id: INT-001\ntype: int\ntitle: Hop 1\nstatus: accepted\nlatency_budget_ms: 1500\n",
        ),
        (
            "INT-002.md",
            "id: INT-002\ntype: int\ntitle: Hop 2\nstatus: accepted\nlatency_budget_ms: 800\n",
        ),
        (
            "INT-003.md",
            "id: INT-003\ntype: int\ntitle: Hop 3\nstatus: accepted\nlatency_budget_ms: 300\n",
        ),
        (
            "NFR-001.md",
            "id: NFR-001\ntype: nfr\ntitle: Бюджет\nstatus: accepted\nverification: v\np99_target_ms: 2000\naffects: [INT-001, INT-002]\n",
        ),
    ] {
        std::fs::write(
            repo.join("model").join(name),
            format!("---\n{fm}---\n\nТело.\n"),
        )
        .expect("entity");
    }
    let component = component_nfr(&repo);
    assert_eq!(component.status, GateStatus::Fail, "{}", component.detail);
    assert!(
        component.detail.contains("проверок: 4"),
        "прогнаны все четыре проверки: {}",
        component.detail
    );
    let errors = component
        .findings
        .iter()
        .filter(|f| f.severity == "error")
        .count();
    let warns = component
        .findings
        .iter()
        .filter(|f| f.severity == "warn")
        .count();
    assert!(errors > 0 && warns > 0, "{:?}", component.findings);
    assert!(
        component.detail.contains(&format!("error: {errors}")),
        "{}",
        component.detail
    );
    assert!(
        component
            .detail
            .contains(&format!("находок: {}", errors + warns)),
        "{}",
        component.detail
    );
}

/// Число error и warn в детали `trace_check` совпадает с фактическими
/// находками: подмена счёта (не-ошибки как ошибки) видна, потому что
/// числа в фикстуре разные.
#[test]
fn trace_detail_counts_match_findings() {
    let tmp = tempfile::tempdir().expect("tmp");
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).expect("mkdir");
    make_gate_repo(&repo);
    // Корневой реестр — вход звена fitness: без него trace честно
    // пропускается, и счётчиков не проверить.
    std::fs::copy(
        repo.join(".arch-handoff/CONSTRAINTS.yaml"),
        repo.join("CONSTRAINTS.yaml"),
    )
    .expect("root registry");
    std::fs::write(
            repo.join("ARCHITECTURE-SPINE.md"),
            "# Spine\n\n## AD-1: Идемпотентность\n\n- **Binds:** Processor.authorize\n- **Prevents:** двойное списание\n- **Rule:** ключ из команды\n",
        )
        .expect("spine");
    write_model(
        &repo,
        &[
            ("AD-001", ""),
            ("REQ-001", "depends_on: []"),
            ("NFR-001", "verification: \"\"\naffects: []"),
        ],
    );
    let component = component_trace(&repo, &GateOptions::default());
    assert_eq!(component.status, GateStatus::Fail, "{}", component.detail);
    let errors = component
        .findings
        .iter()
        .filter(|f| f.severity == "error")
        .count();
    let warns = component
        .findings
        .iter()
        .filter(|f| f.severity == "warn")
        .count();
    assert!(
        errors > 0 && warns > 0 && errors != warns,
        "фикстура обязана дать разные числа: error {errors}, warn {warns}: {:?}",
        component.findings
    );
    assert!(
        component
            .detail
            .contains(&format!("error: {errors}, warn: {warns}")),
        "{}",
        component.detail
    );
}

/// Ранг `nfr-without-verification` зависит от маршрута: на Standard это
/// предупреждение, на Critical — ошибка. Проверяется и вердикт, и
/// критичность самой находки, а не только счётчик.
#[test]
fn model_validate_promotes_nfr_without_verification_only_on_critical() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = tmp.path();
    make_gate_repo(dir);
    write_model(
        dir,
        &[
            ("CMP-001", "depends_on: [MISSING-1]"),
            ("NFR-001", "verification: \"\"\naffects: [CMP-001]"),
        ],
    );
    git(dir, &["add", "."]);
    git(dir, &["commit", "-q", "-m", "model"]);
    let severity_of = |route: Route| {
        let report = run_with(
            dir,
            Some(route),
            None,
            None,
            (1, 4),
            &GateRequirements::default(),
        )
        .expect("gate");
        let comp = report
            .components
            .iter()
            .find(|c| c.name == "model_validate")
            .expect("составляющая");
        let finding = comp
            .findings
            .iter()
            .find(|f| f.rule.as_deref() == Some("nfr-without-verification"))
            .unwrap_or_else(|| panic!("нет находки: {:?}", comp.findings));
        (comp.status, finding.severity.clone())
    };
    let (standard_status, standard_severity) = severity_of(Route::Standard);
    assert_eq!(
        standard_status,
        GateStatus::Fail,
        "сломанная ссылка валит модель"
    );
    assert_eq!(
        standard_severity, "warn",
        "на Standard оговорка остаётся предупреждением"
    );
    let (_, critical_severity) = severity_of(Route::Critical);
    assert_eq!(
        critical_severity, "error",
        "на Critical та же оговорка — ошибка"
    );
}

/// Не-ADR markdown в каталоге решений не считается решением: иначе
/// составляющая требовала бы отчёт рубрики для заметок и черновиков.
#[test]
fn decision_quality_ignores_non_adr_markdown() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = tmp.path();
    make_quality_repo(dir, Some(4.5), Some("judge-x"));
    std::fs::write(
        dir.join("docs/adr/notes.md"),
        "# Заметки\n\n- Status: Accepted\n\nНе решение.\n",
    )
    .expect("notes");
    let report = run_with(
        dir,
        Some(Route::Fast),
        None,
        None,
        (1, 4),
        &with_quality(Route::Fast),
    )
    .expect("gate");
    let comp = report
        .components
        .iter()
        .find(|c| c.name == "decision_quality")
        .expect("comp");
    assert!(
        !comp
            .findings
            .iter()
            .any(|f| f.rule.as_deref() == Some("rubric_report_missing")),
        "заметки не требуют отчёта: {:?}",
        comp.findings
    );
}

/// Отчёт находится по хвосту пути цели: судья мог записать длинный путь,
/// и это тот же документ. Признак пути — не только точное равенство.
#[test]
fn decision_quality_matches_report_by_path_suffix() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = tmp.path();
    make_quality_repo(dir, Some(4.5), Some("judge-x"));
    let path = dir
        .join(crate::rubric::RUBRIC_REPORTS_DIR)
        .join("ADR-001-reshenie.json");
    let mut artifact: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).expect("отчёт")).expect("JSON");
    // Отчёта по хэшу нет: сверка идёт по пути, и путь — длиннее нашего.
    artifact["target_sha256"] = serde_json::Value::Null;
    artifact["target"] = serde_json::json!("/абсолютный/путь/до/docs/adr/ADR-001-reshenie.md");
    std::fs::write(
        &path,
        serde_json::to_string_pretty(&artifact).expect("json"),
    )
    .expect("write");
    let report = run_with(
        dir,
        Some(Route::Fast),
        None,
        None,
        (1, 4),
        &with_quality(Route::Fast),
    )
    .expect("gate");
    let comp = report
        .components
        .iter()
        .find(|c| c.name == "decision_quality")
        .expect("comp");
    assert!(
        !comp
            .findings
            .iter()
            .any(|f| f.rule.as_deref() == Some("rubric_report_missing")),
        "хвост пути опознаёт тот же документ: {:?}",
        comp.findings
    );
}

/// Чужой путь не подменяет решение: отчёт по другому ADR не считается
/// отчётом по этому — иначе оценка одного решения выдавалась бы за другое.
#[test]
fn decision_quality_does_not_use_report_of_another_adr() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = tmp.path();
    make_quality_repo(dir, Some(4.5), Some("judge-x"));
    let path = dir
        .join(crate::rubric::RUBRIC_REPORTS_DIR)
        .join("ADR-001-reshenie.json");
    let mut artifact: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).expect("отчёт")).expect("JSON");
    artifact["target_sha256"] = serde_json::Value::Null;
    artifact["target"] = serde_json::json!("docs/adr/ADR-002-drugoe-reshenie.md");
    std::fs::write(
        &path,
        serde_json::to_string_pretty(&artifact).expect("json"),
    )
    .expect("write");
    let report = run_with(
        dir,
        Some(Route::Fast),
        None,
        None,
        (1, 4),
        &with_quality(Route::Fast),
    )
    .expect("gate");
    let comp = report
        .components
        .iter()
        .find(|c| c.name == "decision_quality")
        .expect("comp");
    assert!(
        comp.findings
            .iter()
            .any(|f| f.rule.as_deref() == Some("rubric_report_missing")),
        "чужой отчёт не закрывает наше решение: {:?}",
        comp.findings
    );
}

/// Патч JSON отчёта качества: тесты задают происхождение, независимость,
/// второго судью и решения точечно, не пересобирая фикстуру.
fn patch_report<F: FnOnce(&mut serde_json::Value)>(dir: &Path, name: &str, f: F) {
    let path = dir.join(crate::rubric::RUBRIC_REPORTS_DIR).join(name);
    let mut value: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).expect("отчёт")).expect("JSON");
    f(&mut value);
    std::fs::write(&path, serde_json::to_string_pretty(&value).expect("json"))
        .expect("write report");
}

/// Нулевая доля невалидных сэмплов — не повод для предупреждения: порог
/// строгий, иначе каждый отчёт получал бы находку «вне шкалы».
#[test]
fn decision_quality_zero_invalid_ratio_adds_no_finding() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = tmp.path();
    make_quality_repo(dir, Some(4.5), Some("judge-x"));
    let report = run_quality_on(dir, Route::Fast, false);
    let comp = report
        .components
        .iter()
        .find(|c| c.name == "decision_quality")
        .expect("comp");
    assert!(
        !comp
            .findings
            .iter()
            .any(|f| f.rule.as_deref() == Some("rubric_invalid_samples")),
        "нулевая доля — тишина: {:?}",
        comp.findings
    );
}

/// Отчёт без автора не даёт находки о семействе: сравнивать не с чем.
#[test]
fn decision_quality_missing_author_adds_no_family_finding() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = tmp.path();
    make_quality_repo(dir, Some(4.5), None);
    let report = run_quality_on(dir, Route::Fast, false);
    let comp = report
        .components
        .iter()
        .find(|c| c.name == "decision_quality")
        .expect("comp");
    assert!(
        !comp.findings.iter().any(|f| f.message.contains("семейств")),
        "автора нет — о семействе не говорим: {:?}",
        comp.findings
    );
    // «Автор не указан» при этом назван отдельной находкой.
    assert!(
        comp.findings
            .iter()
            .any(|f| f.rule.as_deref() == Some("judge_is_author")),
        "{:?}",
        comp.findings
    );
}

/// «Судья = автор» по умолчанию — предупреждение; ошибкой это становится
/// только когда проект потребовал разделения (`require_distinct_judge`).
#[test]
fn decision_quality_judge_is_author_severity_follows_project_demand() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = tmp.path();
    make_quality_repo(dir, Some(4.5), Some("judge-x"));
    let severity_in = |require_distinct: bool| {
        let mut options = GateOptions::default();
        options.decision_quality.require_distinct_judge = require_distinct;
        options.route = Some(Route::Fast);
        let report = crate::gate::verdict::run_inner(
            dir,
            Some(Route::Fast),
            None,
            None,
            (1, 4),
            &with_quality(Route::Fast),
            &options,
        )
        .expect("гейт");
        report
            .components
            .iter()
            .find(|c| c.name == "decision_quality")
            .expect("comp")
            .findings
            .iter()
            .find(|f| f.rule.as_deref() == Some("judge_is_author"))
            .map(|f| f.severity.clone())
            .expect("находка judge_is_author")
    };
    assert_eq!(severity_in(false), "warn", "по умолчанию — предупреждение");
    assert_eq!(severity_in(true), "error", "проект потребовал — ошибка");
}

/// На неблокирующем маршруте квалификация судьи не спрашивается, даже если
/// проект включил флаг: останавливать нечего. Проверка идёт по отчёту ПО ДОСЬЕ
/// — именно там эталонный набор и стоит проверка квалификации.
#[test]
fn decision_quality_qualification_not_checked_on_warn_policy() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = tmp.path();
    write_code_pack_report(dir, None);
    let report = run_quality_on(dir, Route::Fast, true);
    let comp = report
        .components
        .iter()
        .find(|c| c.name == "decision_quality")
        .expect("comp");
    assert!(
        !comp
            .findings
            .iter()
            .any(|f| f.rule.as_deref() == Some("judge_unqualified")),
        "на Fast человека нет — квалификация не требуется: {:?}",
        comp.findings
    );
}

/// Счётчики примечаний: невоспроизводимость названа с числом (отчёт без
/// сырых ответов), а рабочая сессия — нет (происхождение не задано).
#[test]
fn decision_quality_counter_notes_follow_their_counters() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = tmp.path();
    make_quality_repo(dir, Some(4.5), Some("judge-x"));
    let report = run_quality_on(dir, Route::Fast, false);
    let comp = report
        .components
        .iter()
        .find(|c| c.name == "decision_quality")
        .expect("comp");
    assert!(
        !comp
            .not_verified
            .iter()
            .any(|n| n.contains("рабочей сессии")),
        "нулевой счётчик сессии не даёт примечания: {:?}",
        comp.not_verified
    );
    assert!(
        comp.not_verified
            .iter()
            .any(|n| n.contains("невоспроизводима") && n.contains("(1)")),
        "отчёт без сырых ответов назван с числом: {:?}",
        comp.not_verified
    );
}

/// Отчёт по досье с сохранёнными ответами не попадает в «невоспроизводимые» и
/// не объявляется расходящимся с собственными ответами.
#[test]
fn decision_quality_reproducible_dossier_report_has_no_unreproducible_note() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = tmp.path();
    let answer = "{\"scores\": []}";
    let recorded = crate::hash::sha256_hex(answer.as_bytes());
    write_code_pack_report(dir, Some((answer, &recorded)));
    let report = run_quality_on(dir, Route::Fast, false);
    let comp = report
        .components
        .iter()
        .find(|c| c.name == "decision_quality")
        .expect("comp");
    assert!(
        !comp
            .not_verified
            .iter()
            .any(|n| n.contains("невоспроизводима")),
        "сырые ответы сохранены: {:?}",
        comp.not_verified
    );
    assert!(
        !comp
            .findings
            .iter()
            .any(|f| f.rule.as_deref() == Some("rubric_report_unreproducible")),
        "{:?}",
        comp.findings
    );
    assert!(
        !comp
            .findings
            .iter()
            .any(|f| f.rule.as_deref() == Some("rubric_report_inconsistent")),
        "без расхождений отчёт не «несоответствует»: {:?}",
        comp.findings
    );
}

/// Примечание о рабочей сессии: только режим «заявлена» И число вызовов
/// выше потолка; на самом потолке и в режиме запуска Spine — тишина.
#[test]
fn decision_quality_session_note_needs_declared_mode_above_limit() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = tmp.path();
    make_quality_repo(dir, Some(4.5), Some("judge-x"));
    let note_present = |mode: &str, calls: usize| {
        patch_report(dir, "ADR-001-reshenie.json", |v| {
            v["provenance"] = serde_json::json!({
                "mode": mode,
                "session_calls_before": calls,
            });
        });
        let report = run_quality_on(dir, Route::Fast, false);
        let comp = report
            .components
            .iter()
            .find(|c| c.name == "decision_quality")
            .expect("comp");
        comp.not_verified
            .iter()
            .any(|n| n.contains("рабочей сессии"))
    };
    assert!(
        note_present(crate::judge::MODE_DECLARED, 4),
        "заявленная сессия сверх потолка — примечание"
    );
    assert!(
        !note_present(crate::judge::MODE_DECLARED, 3),
        "ровно потолок — ещё чисто"
    );
    assert!(
        !note_present(crate::judge::MODE_LAUNCHED, 9),
        "судью запускал Spine — признак не тот"
    );
}

/// Независимость ровно на пороге проекта — не «ниже порога».
#[test]
fn decision_quality_independence_at_threshold_is_not_low() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = tmp.path();
    make_quality_repo(dir, Some(4.5), Some("judge-x"));
    patch_report(dir, "ADR-001-reshenie.json", |v| {
        v["independence"] = serde_json::json!(crate::judge::INDEPENDENCE_DECLARED);
    });
    let mut options = GateOptions::default();
    options.decision_quality.min_independence = crate::judge::INDEPENDENCE_DECLARED.to_string();
    options.route = Some(Route::Fast);
    let report = crate::gate::verdict::run_inner(
        dir,
        Some(Route::Fast),
        None,
        None,
        (1, 4),
        &with_quality(Route::Fast),
        &options,
    )
    .expect("гейт");
    let comp = report
        .components
        .iter()
        .find(|c| c.name == "decision_quality")
        .expect("comp");
    assert!(
        !comp
            .findings
            .iter()
            .any(|f| f.rule.as_deref() == Some("judge_independence_low")),
        "на пороге — не ниже порога: {:?}",
        comp.findings
    );
}

/// Запись решения архитектора по отчёту качества (в том же виде, что пишет
/// `rubric decide`).
fn write_quality_decision(dir: &Path, verdict: crate::rubric::HumanVerdict) {
    let path = dir
        .join(crate::rubric::RUBRIC_REPORTS_DIR)
        .join("ADR-001-reshenie.json");
    let text = std::fs::read_to_string(&path).expect("отчёт");
    let artifact: crate::rubric::RubricArtifact = serde_json::from_str(&text).expect("JSON");
    let slug = crate::judge::artifact_slug_of(&artifact);
    let record = crate::rubric::HumanDecision::new(
        &artifact,
        "reports/rubric/ADR-001-reshenie.json",
        &crate::hash::sha256_hex(text.as_bytes()),
        verdict,
        "Архитектор <arch@bank>",
        "разобрано человеком",
    );
    crate::rubric::write_decision(dir, &slug, &record).expect("решение");
}

/// Оговорка `evidence_partial` на блокирующем маршруте снимается только
/// решением «принято»: «отклонено» эскалацию сохраняет, а решение не
/// подменяется чужое.
#[test]
fn decision_quality_evidence_partial_needs_accept_decision() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = tmp.path();
    make_quality_repo(dir, Some(4.5), Some("judge-x"));
    patch_quality_report(dir, None, &["evidence_partial"]);
    // (статус, есть ли предупреждение «принято»)
    let state = || {
        let report = run_quality_on(dir, Route::Critical, false);
        let comp = report
            .components
            .iter()
            .find(|c| c.name == "decision_quality")
            .expect("comp");
        let accepted = comp.findings.iter().any(|f| {
            f.rule.as_deref() == Some("rubric_evidence_partial")
                && f.severity == "warn"
                && f.message.contains("решение архитектора принято")
        });
        (comp.status, accepted)
    };
    // Без решения — эскалация: человека зовут.
    let (status, _) = state();
    assert_eq!(status, GateStatus::Skip);
    // «Принято» — оговорка разобрана, находка остаётся предупреждением.
    write_quality_decision(dir, crate::rubric::HumanVerdict::Accept);
    let (status, accepted) = state();
    assert_eq!(
        status,
        GateStatus::Pass,
        "принятое решение снимает эскалацию"
    );
    assert!(accepted, "находка названа принятой архитектором");
    // «Отклонено» — эскалация остаётся.
    write_quality_decision(dir, crate::rubric::HumanVerdict::Reject);
    let report = run_quality_on(dir, Route::Critical, false);
    assert_eq!(
        status_of(&report, "decision_quality"),
        GateStatus::Skip,
        "отклонённое суждение не принимается: {}",
        render(&report)
    );
}

/// Сумма категорий «механика не подтверждает»: инъекция и невалидные сэмплы
/// складываются, а не вычитаются — иначе вердикт молчал бы там, где человек
/// нужен двум разным причинам.
#[test]
fn decision_quality_unconfirmed_categories_add_up() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = tmp.path();
    make_quality_repo(dir, Some(4.5), Some("judge-x"));
    // Первый отчёт — вход с инъекцией.
    patch_report(dir, "ADR-001-reshenie.json", |v| {
        v["input_injection_lines"] = serde_json::json!([2]);
    });
    // Второй документ — доля невалидных сэмплов выше порога.
    let adr2 = dir.join("docs/adr/ADR-002-vtoroe.md");
    std::fs::write(&adr2, "# ADR-002\n\n- Status: Accepted\n\nРешение.\n").expect("adr2");
    let sha2 = crate::hash::sha256_file(&adr2).expect("sha");
    let reports = dir.join(crate::rubric::RUBRIC_REPORTS_DIR);
    let second_adr_report = serde_json::json!({
        "schema": crate::rubric::RUBRIC_REPORT_SCHEMA,
        "rubric": "adr_quality",
        "target": "docs/adr/ADR-002-vtoroe.md",
        "target_sha256": sha2,
        "judge_model": "judge-x",
        "author_model": "agent-y",
        "weighted_total": 4.5,
        "verdict": "OK",
        "unstable": false,
        "evidence_not_found": 0,
        "invalid_samples_ratio": 0.9,
        "judged_at": "2026-09-25T10:00:00+00:00",
    });
    std::fs::write(
        reports.join("ADR-002-vtoroe.json"),
        serde_json::to_string_pretty(&second_adr_report).expect("json"),
    )
    .expect("write report2");
    let report = run_quality_on(dir, Route::Fast, false);
    let comp = report
        .components
        .iter()
        .find(|c| c.name == "decision_quality")
        .expect("comp");
    assert_eq!(comp.status, GateStatus::Skip, "{}", comp.detail);
    assert!(
        comp.detail.contains("механика не подтверждает: 2"),
        "две категории складываются: {}",
        comp.detail
    );
    assert!(
        comp.detail.contains("вход с инъекцией: 1")
            && comp.detail.contains("невалидных сэмплов сверх порога: 1"),
        "{}",
        comp.detail
    );
}

/// Судья и автор — разные метки одного семейства: «другая модель» не значит
/// «другой взгляд», и это названо (семейство опознано по метке).
#[test]
fn decision_quality_names_same_family_different_models() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = tmp.path();
    make_quality_repo(dir, Some(4.5), Some("claude-opus-4"));
    patch_report(dir, "ADR-001-reshenie.json", |v| {
        v["judge_model"] = serde_json::json!("claude-sonnet-4");
    });
    let report = run_quality_on(dir, Route::Fast, false);
    let comp = report
        .components
        .iter()
        .find(|c| c.name == "decision_quality")
        .expect("comp");
    assert!(
        comp.findings
            .iter()
            .any(|f| f.message.contains("одного семейства")),
        "разные модели одного семейства названы: {:?}",
        comp.findings
    );
    // А разные семейства — не повод для находки о семействе.
    patch_report(dir, "ADR-001-reshenie.json", |v| {
        v["judge_model"] = serde_json::json!("qwen-max");
    });
    let report = run_quality_on(dir, Route::Fast, false);
    let comp = report
        .components
        .iter()
        .find(|c| c.name == "decision_quality")
        .expect("comp");
    assert!(
        !comp
            .findings
            .iter()
            .any(|f| f.message.contains("одного семейства")),
        "разные семейства — тишина: {:?}",
        comp.findings
    );
}
