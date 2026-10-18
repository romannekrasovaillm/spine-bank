//! Связки «критерий ↔ правило» через полный прогон гейта (ADR-093, волны
//! 1–2): исходы правил того же прогона, экспорт детекторов, короткое
//! замыкание по измерению и согласованность досье хоста и гейта.

use std::path::{Path, PathBuf};

use super::testkit::*;
use super::verdict::run_inner;
use super::{GateOptions, GateReport, GateRequirements, GateStatus};
use crate::control::Route;

/// Правило на текст ADR: слово-маркер запрещённого в решении.
const RULE_ID: &str = "C-90";

/// Репозиторий: Accepted-ADR, спайн с инвариантом, реестр с правилом C-90.
fn make_repo(dir: &Path, adr_body: &str) -> PathBuf {
    make_gate_repo(dir);
    std::fs::write(
        dir.join(".arch-handoff/CONSTRAINTS.yaml"),
        format!(
            "rules:\n  - id: {RULE_ID}\n    name: adr_no_direct_ledger_write\n    type: must_not_contain\n    glob: \"docs/adr/*.md\"\n    pattern: 'ПРЯМАЯ ЗАПИСЬ'\n    severity: error\n    ad: AD-1\n"
        ),
    )
    .expect("constraints");
    std::fs::create_dir_all(dir.join("docs/adr")).expect("mkdir adr");
    let adr = dir.join("docs/adr/ADR-001-reshenie.md");
    std::fs::write(
        &adr,
        format!("# ADR-001. Решение\n\n- Date: 2026-10-18\n- Status: Accepted\n\n## Decision\n\n{adr_body}\n"),
    )
    .expect("adr");
    std::fs::write(
        dir.join("ARCHITECTURE-SPINE.md"),
        "# Spine\n\n## AD-1: Журнал только дописывается\n\n\
         - **Binds**: журнал операций ↔ код записи\n\
         - **Prevents**: правку и удаление строк журнала\n\
         - **Rule**: строки журнала не правятся.\n",
    )
    .expect("spine");
    git(dir, &["add", "."]);
    git(dir, &["commit", "-q", "-m", "binding"]);
    adr
}

/// Каталог рубрик: встроенная `adr_spine_consistency` с привязкой главного
/// критерия к правилу `rule` в заданной роли и режиме.
fn rubrics_with_binding(dir: &Path, rule: &str, role: &str, mode: &str) -> PathBuf {
    let rubrics = dir.join("rubrics");
    std::fs::create_dir_all(&rubrics).expect("mkdir rubrics");
    let mut rubric: crate::rubric::Rubric =
        serde_yaml_ng::from_str(crate::assets::RUBRIC_ADR_SPINE_CONSISTENCY).expect("рубрика");
    let main = rubric
        .criteria
        .iter_mut()
        .find(|c| c.blocking)
        .expect("главный критерий");
    main.bound_rules = vec![crate::rubric::BoundRule {
        rule: rule.to_string(),
        role: serde_yaml_ng::from_str(role).expect("роль"),
    }];
    main.binding_mode = serde_yaml_ng::from_str(mode).expect("режим");
    std::fs::write(
        rubrics.join("adr_spine_consistency.yaml"),
        serde_yaml_ng::to_string(&rubric).expect("yaml"),
    )
    .expect("рубрика");
    rubrics
}

/// Настройки составляющей: одна рубрика, все субъекты.
fn cfg(export: bool) -> crate::config::SemanticQualityConfig {
    crate::config::SemanticQualityConfig {
        rubrics: vec!["adr_spine_consistency".to_string()],
        scope: crate::config::SemanticScope::All,
        export_detectors: export,
        ..crate::config::SemanticQualityConfig::default()
    }
}

/// Прогон гейта на Fast с обязательной `semantic_quality`.
fn run(dir: &Path, cfg: crate::config::SemanticQualityConfig, rubrics: &Path) -> GateReport {
    let mut req = GateRequirements::default();
    req.fast.push("semantic_quality".to_string());
    let options = GateOptions {
        semantic_quality: cfg,
        rubrics_dir: rubrics.to_path_buf(),
        ..GateOptions::default()
    };
    run_inner(dir, Some(Route::Fast), None, None, (1, 4), &req, &options).expect("гейт")
}

/// Коды находок составляющей с их severity.
fn semantic_findings(report: &GateReport) -> Vec<(String, String)> {
    report
        .components
        .iter()
        .find(|c| c.name == "semantic_quality")
        .map(|c| {
            c.findings
                .iter()
                .map(|f| (f.rule.clone().unwrap_or_default(), f.severity.clone()))
                .collect()
        })
        .unwrap_or_default()
}

/// Отчёт судьи по досье, собранному так же, как его соберёт гейт.
fn write_report(dir: &Path, rubrics: &Path, subject: &str, score: u8) {
    let rubric =
        crate::rubric::load(&rubrics.join("adr_spine_consistency.yaml")).expect("рубрика");
    let packs = crate::rubric_pack::build_for(dir, &rubric, subject).expect("досье");
    let artifact = serde_json::json!({
        "schema": crate::rubric::RUBRIC_REPORT_SCHEMA,
        "rubric": "adr_spine_consistency",
        "judge_model": "judge-x",
        "author_model": "author-y",
        "weighted_total": 4.5,
        "verdict": "OK",
        "pack_kind": "adr_vs_spine",
        "subject": subject,
        "pack_sha256": packs[0].sha256,
        "inputs": packs[0].inputs.iter().map(|i| serde_json::json!({
            "path": i.path, "sha256": i.sha256, "role": i.role.as_str(), "id": i.id,
        })).collect::<Vec<_>>(),
        "scores": [{
            "criterion_id": "no_contradiction",
            "weight": 3.0,
            "score": score,
            "rationale": "Цитата subject: \"Решение\". Цитата reference: \"Rule\".",
            "samples": [score],
            "stdev": 0.0,
            "flags": [],
            "checked": ["AD-1"],
        }],
        "judged_at": "2026-10-18T10:00:00+03:00",
    });
    let reports = dir.join(crate::rubric::RUBRIC_REPORTS_DIR);
    std::fs::create_dir_all(&reports).expect("mkdir reports");
    std::fs::write(
        reports.join("semantic.json"),
        serde_json::to_string_pretty(&artifact).expect("json"),
    )
    .expect("отчёт");
}

const SUBJECT: &str = "docs/adr/ADR-001-reshenie.md";

/// Необходимое правило красное в файле субъекта, связка блокирующая:
/// субъект решён измерением — без отчёта судьи и без находки «нет отчёта».
#[test]
fn necessary_rule_failure_decides_subject_without_report() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = tmp.path();
    make_repo(dir, "Сервис делает ПРЯМАЯ ЗАПИСЬ в журнал в обход API.");
    let rubrics = rubrics_with_binding(dir, RULE_ID, "necessary", "blocking");
    let report = run(dir, cfg(false), &rubrics);
    let findings = semantic_findings(&report);
    assert_eq!(
        status_of(&report, "semantic_quality"),
        GateStatus::Fail,
        "{}",
        crate::gate::render(&report)
    );
    assert!(
        findings.contains(&("binding_rule_failed".to_string(), "error".to_string())),
        "{findings:?}"
    );
    assert!(
        !findings.iter().any(|(c, _)| c == "semantic_report_missing"),
        "короткое замыкание: отчёт не нужен — {findings:?}"
    );
}

/// Та же связка в советующем режиме вердикт не решает: отчёт по-прежнему
/// обязателен, а провал правила виден предупреждением.
#[test]
fn advisory_binding_keeps_report_requirement() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = tmp.path();
    make_repo(dir, "Сервис делает ПРЯМАЯ ЗАПИСЬ в журнал в обход API.");
    let rubrics = rubrics_with_binding(dir, RULE_ID, "necessary", "advisory");
    let report = run(dir, cfg(false), &rubrics);
    let findings = semantic_findings(&report);
    assert!(
        findings.contains(&("semantic_report_missing".to_string(), "error".to_string())),
        "{findings:?}"
    );
    assert!(
        findings.contains(&("binding_rule_failed".to_string(), "warn".to_string())),
        "{findings:?}"
    );
}

/// Связка на правило, которого нет в реестре, — ошибка конфигурации.
#[test]
fn binding_to_unknown_rule_is_error() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = tmp.path();
    make_repo(dir, "Запись в журнал только через API.");
    let rubrics = rubrics_with_binding(dir, "C-404", "evidence", "advisory");
    write_report(dir, &rubrics, SUBJECT, 5);
    let report = run(dir, cfg(false), &rubrics);
    let findings = semantic_findings(&report);
    assert!(
        findings.contains(&("binding_rule_unknown".to_string(), "error".to_string())),
        "{findings:?}"
    );
    assert_eq!(status_of(&report, "semantic_quality"), GateStatus::Fail);
}

/// Экспорт детекторов (волна 1): гейт пишет исход правила в
/// `reports/detectors/rules/`, привязанное правило входит в досье ролью
/// `detector`, а отчёт, снятый по досье хоста, гейт считает свежим —
/// досье у хоста и у гейта одно и то же.
#[test]
fn exported_detectors_keep_host_and_gate_dossiers_identical() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = tmp.path();
    make_repo(dir, "Запись в журнал только через API.");
    // Советующая связка: допуск судьи (волна 4) здесь не проверяется — тест
    // о согласованности досье, а не о калибровке.
    let rubrics = rubrics_with_binding(dir, RULE_ID, "evidence", "advisory");
    // Первый прогон — экспорт исходов (отчёта ещё нет).
    let first = run(dir, cfg(true), &rubrics);
    let detector = dir.join(crate::control::RULE_DETECTORS_DIR).join("C-90.json");
    assert!(detector.is_file(), "{}", crate::gate::render(&first));
    let text = std::fs::read_to_string(&detector).expect("детектор");
    assert!(text.contains("\"status\":\"pass\""), "{text}");
    let detail = first
        .components
        .iter()
        .find(|c| c.name == "semantic_quality")
        .map(|c| c.detail.clone())
        .unwrap_or_default();
    assert!(detail.contains("детекторы правил: 1"), "{detail}");
    // Хост собирает досье тем же кодом: детектор привязанного правила в нём.
    let rubric =
        crate::rubric::load(&rubrics.join("adr_spine_consistency.yaml")).expect("рубрика");
    let packs = crate::rubric_pack::build_for(dir, &rubric, SUBJECT).expect("досье");
    assert!(
        packs[0]
            .inputs
            .iter()
            .any(|i| i.role == crate::rubric_pack::InputRole::Detector
                && i.id.as_deref() == Some("C-90")
                && i.status.as_deref() == Some("pass")),
        "{:?}",
        packs[0].inputs
    );
    write_report(dir, &rubrics, SUBJECT, 5);
    let second = run(dir, cfg(true), &rubrics);
    let findings = semantic_findings(&second);
    assert!(
        !findings.iter().any(|(c, _)| c == "semantic_report_stale"),
        "повторный экспорт того же исхода отчёт не обесценивает: {findings:?}"
    );
    assert_eq!(
        status_of(&second, "semantic_quality"),
        GateStatus::Pass,
        "{}",
        crate::gate::render(&second)
    );
}

/// Смена исхода привязанного правила обесценивает отчёт, даже если сам
/// субъект не менялся: судья судил по другому измерению.
#[test]
fn changed_rule_outcome_makes_report_stale() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = tmp.path();
    make_repo(dir, "Запись в журнал только через API.");
    let rubrics = rubrics_with_binding(dir, RULE_ID, "evidence", "advisory");
    let _first = run(dir, cfg(true), &rubrics);
    write_report(dir, &rubrics, SUBJECT, 5);
    // Черновик другого решения краснит правило; ADR-001 не тронут.
    std::fs::write(
        dir.join("docs/adr/ADR-002-chernovik.md"),
        "# ADR-002. Черновик\n\n- Status: Proposed\n\nПРЯМАЯ ЗАПИСЬ разрешена.\n",
    )
    .expect("adr-002");
    let report = run(dir, cfg(true), &rubrics);
    let findings = semantic_findings(&report);
    assert!(
        findings.iter().any(|(c, _)| c == "semantic_report_stale"),
        "{findings:?}"
    );
}

/// Правило-свидетельство красное, судья поставил «чисто» — противоречие
/// с измерением эскалирует (вердикт INCOMPLETE) даже на Fast.
#[test]
fn evidence_rule_red_against_clean_judgement_escalates() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = tmp.path();
    make_repo(dir, "Сервис делает ПРЯМАЯ ЗАПИСЬ в журнал в обход API.");
    let rubrics = rubrics_with_binding(dir, RULE_ID, "evidence", "blocking");
    // Допуск судьи к блокирующей связке (волна 4) — пройден.
    let rubric =
        crate::rubric::load(&rubrics.join("adr_spine_consistency.yaml")).expect("рубрика");
    let set = crate::rubric::QualificationSet {
        dir: PathBuf::from("/набор"),
        cases: Vec::new(),
        sha256: "e".repeat(64),
    };
    let outcome = |truth| crate::rubric::CaseOutcome {
        file: "x.md".to_string(),
        class: "c".to_string(),
        truth,
        decision: Some(match truth {
            crate::rubric::Truth::Defective => crate::rubric::RubricDecision::Fail,
            crate::rubric::Truth::Clean => crate::rubric::RubricDecision::Pass,
        }),
        weighted_total: 4.0,
        flags: Vec::new(),
        error: None,
    };
    let qualification = crate::rubric::build_qualification_report(
        &rubric,
        "judge-x",
        &set,
        vec![
            outcome(crate::rubric::Truth::Defective),
            outcome(crate::rubric::Truth::Clean),
        ],
        1,
    );
    crate::rubric::write_qualification(dir, &qualification).expect("квалификация");
    let _first = run(dir, cfg(true), &rubrics);
    write_report(dir, &rubrics, SUBJECT, 5);
    let report = run(dir, cfg(true), &rubrics);
    let findings = semantic_findings(&report);
    assert!(
        findings.contains(&("binding_contradiction".to_string(), "error".to_string())),
        "{findings:?}"
    );
    assert_eq!(
        status_of(&report, "semantic_quality"),
        GateStatus::Skip,
        "{}",
        crate::gate::render(&report)
    );
}
