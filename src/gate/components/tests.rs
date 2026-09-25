//! Тесты составляющих гейта: вынесены из `components.rs` чистым
//! перемещением — продовый модуль обязан укладываться в границу
//! `prod_file_length_limit` (C-33), а тесты растут быстрее кода.
//! Видимость прежняя: `super::*` — это модуль `components`.

use super::*;
use crate::gate::testkit::*;

use crate::gate::{
    GateOptions, GateOutcome, GateReport, GateRequirements, GateStatus, render, run, run_opts,
    run_with,
};

// --- Н7: качество решений как составляющая гейта (ADR-042) -------------

/// Репозиторий с одним Accepted-ADR и (опционально) отчётом рубрики.
fn make_quality_repo(dir: &Path, score: Option<f64>, author: Option<&str>) {
    make_gate_repo(dir);
    std::fs::create_dir_all(dir.join("docs/adr")).expect("mkdir adr");
    let adr = dir.join("docs/adr/ADR-001-reshenie.md");
    std::fs::write(
            &adr,
            "# ADR-001. Решение\n\n- Date: 2026-09-19\n- Status: Accepted\n\n## Context\n\nПричина.\n\n## Alternatives\n\nВариант Б.\n\n## Consequences\n\nЦена.\n",
        )
        .expect("adr");
    git(dir, &["add", "."]);
    // `--allow-empty`: тест может пересобрать фикстуру в том же каталоге.
    git(dir, &["commit", "-q", "--allow-empty", "-m", "adr"]);
    if let Some(total) = score {
        let sha = crate::hash::sha256_file(&adr).expect("sha");
        let artifact = serde_json::json!({
            "schema": crate::rubric::RUBRIC_REPORT_SCHEMA,
            "rubric": "adr_quality",
            "target": "docs/adr/ADR-001-reshenie.md",
            "target_sha256": sha,
            "judge_model": "judge-x",
            "author_model": author,
            "weighted_total": total,
            "verdict": "OK",
            "unstable": false,
            "evidence_not_found": 0,
            "judged_at": "2026-09-19T10:00:00+00:00",
        });
        let reports = dir.join(crate::rubric::RUBRIC_REPORTS_DIR);
        std::fs::create_dir_all(&reports).expect("mkdir reports");
        std::fs::write(
            reports.join("ADR-001-reshenie.json"),
            serde_json::to_string_pretty(&artifact).expect("json"),
        )
        .expect("write report");
    }
}
/// С включённой составляющей требования передаются явно.
fn with_quality(route: Route) -> GateRequirements {
    let mut req = GateRequirements::default();
    let list = match route {
        Route::Fast => &mut req.fast,
        Route::Standard => &mut req.standard,
        Route::Critical => &mut req.critical,
    };
    list.push("decision_quality".to_string());
    req
}

/// По умолчанию составляющая — SKIP: включение только через `[gate.required]`.
#[test]
fn decision_quality_is_skip_by_default() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = tmp.path();
    make_quality_repo(dir, Some(1.0), Some("judge-x"));
    let report = run_with(
        dir,
        Some(Route::Fast),
        None,
        None,
        (1, 4),
        &GateRequirements::default(),
    )
    .expect("gate");
    assert_eq!(
        status_of(&report, "decision_quality"),
        GateStatus::Skip,
        "ADR с низким баллом не краснит гейт без явного включения"
    );
    assert_eq!(report.outcome, GateOutcome::Pass);
}

/// Слабый ADR (картонный: секции есть, содержания нет) — ниже порога.
#[test]
fn decision_quality_fails_below_threshold() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = tmp.path();
    make_quality_repo(dir, Some(1.30), Some("judge-x"));
    let report = run_with(
        dir,
        Some(Route::Fast),
        None,
        None,
        (1, 4),
        &with_quality(Route::Fast),
    )
    .expect("gate");
    assert_eq!(status_of(&report, "decision_quality"), GateStatus::Fail);
    let findings = &report
        .components
        .iter()
        .find(|c| c.name == "decision_quality")
        .expect("comp")
        .findings;
    assert!(
        findings
            .iter()
            .any(|f| f.rule.as_deref() == Some("decision_quality_low")),
        "{findings:?}"
    );
    // Сильный ADR (3.90) — тот же порог пройден.
    make_quality_repo(dir, Some(3.90), Some("judge-x"));
    let ok = run_with(
        dir,
        Some(Route::Fast),
        None,
        None,
        (1, 4),
        &with_quality(Route::Fast),
    )
    .expect("gate");
    assert_eq!(status_of(&ok, "decision_quality"), GateStatus::Pass);
}

/// Отчёт, снятый с прежней редакции ADR, обесценивается.
#[test]
fn decision_quality_flags_stale_report() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = tmp.path();
    make_quality_repo(dir, Some(4.5), Some("judge-x"));
    // Правка документа после оценки — при том же пути.
    let adr = dir.join("docs/adr/ADR-001-reshenie.md");
    let mut text = std::fs::read_to_string(&adr).expect("read");
    text.push_str("\nДописано после оценки.\n");
    std::fs::write(&adr, text).expect("write");
    let report = run_with(
        dir,
        Some(Route::Fast),
        None,
        None,
        (1, 4),
        &with_quality(Route::Fast),
    )
    .expect("gate");
    assert_eq!(status_of(&report, "decision_quality"), GateStatus::Fail);
    let findings = &report
        .components
        .iter()
        .find(|c| c.name == "decision_quality")
        .expect("comp")
        .findings;
    assert!(
        findings
            .iter()
            .any(|f| f.rule.as_deref() == Some("rubric_report_stale")),
        "{findings:?}"
    );
}

/// Судья = автор (или автор не указан) — отдельная находка.
#[test]
fn decision_quality_warns_when_judge_is_author() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = tmp.path();
    for author in [Some("judge-x"), None] {
        make_quality_repo(dir, Some(4.5), author);
        let report = run_with(
            dir,
            Some(Route::Fast),
            None,
            None,
            (1, 4),
            &with_quality(Route::Fast),
        )
        .expect("gate");
        let findings = &report
            .components
            .iter()
            .find(|c| c.name == "decision_quality")
            .expect("comp")
            .findings;
        let comp = report
            .components
            .iter()
            .find(|c| c.name == "decision_quality")
            .expect("comp");
        let hit = findings
            .iter()
            .find(|f| f.rule.as_deref() == Some("judge_is_author"))
            .unwrap_or_else(|| {
                panic!(
                    "нет judge_is_author для {author:?}: {:?} / {}",
                    findings, comp.detail
                )
            });
        assert_eq!(hit.severity, "warn", "{findings:?}");
        // warn не краснит составляющую: балл выше порога.
        assert_eq!(status_of(&report, "decision_quality"), GateStatus::Pass);
    }
}

/// Нет отчёта вовсе — решение не оценено (дефект D9: «картонный» ADR).
#[test]
fn decision_quality_requires_report_when_enabled() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = tmp.path();
    make_quality_repo(dir, None, None);
    let report = run_with(
        dir,
        Some(Route::Fast),
        None,
        None,
        (1, 4),
        &with_quality(Route::Fast),
    )
    .expect("gate");
    assert_eq!(status_of(&report, "decision_quality"), GateStatus::Fail);
    assert!(
        report
            .components
            .iter()
            .find(|c| c.name == "decision_quality")
            .expect("comp")
            .findings
            .iter()
            .any(|f| f.rule.as_deref() == Some("rubric_report_missing")),
        "ожидалась rubric_report_missing"
    );
}
/// Н2: битая ссылка модели краснит гейт — без каталога `model/` секция
/// честно SKIP, вердикт тот же, что у `model validate`.
#[test]
fn gate_fails_on_broken_model_link() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = tmp.path();
    make_gate_repo(dir);
    write_model(
        dir,
        &[
            ("CMP-001", "depends_on: [CMP-002]"),
            ("CMP-002", "depends_on: []"),
        ],
    );
    // Модель коммитится: с Н4 delta guard видит и неотслеживаемые файлы,
    // и незакоммиченная модель — это правка спайна без дельты.
    git(dir, &["add", "."]);
    git(dir, &["commit", "-q", "-m", "model"]);
    let limits = (1, 4);
    let clean = run_with(
        dir,
        Some(Route::Standard),
        None,
        None,
        limits,
        &GateRequirements::default(),
    )
    .expect("gate");
    assert_eq!(status_of(&clean, "model_validate"), GateStatus::Pass);
    assert_ne!(clean.outcome, GateOutcome::Fail);
    // Конверт вердикта содержит составляющую (П7).
    let envelope = clean.envelope_json();
    assert!(
        envelope["components"]
            .as_array()
            .expect("components")
            .iter()
            .any(|c| c["name"] == "model_validate"),
        "{envelope}"
    );
    // Ссылка на несуществующую сущность — гейт краснеет.
    write_model(
        dir,
        &[
            ("CMP-001", "depends_on: [CMP-099]"),
            ("CMP-002", "depends_on: []"),
        ],
    );
    git(dir, &["add", "."]);
    git(dir, &["commit", "-q", "-m", "broken link"]);
    let broken = run_with(
        dir,
        Some(Route::Standard),
        None,
        None,
        limits,
        &GateRequirements::default(),
    )
    .expect("gate");
    assert_eq!(status_of(&broken, "model_validate"), GateStatus::Fail);
    assert!(!broken.passed);
    assert_eq!(broken.outcome, GateOutcome::Fail);
}

/// Без каталога `model/` составляющая пропускается fail-soft, и на
/// маршруте, где она НЕ обязательна, это не даёт INCOMPLETE.
#[test]
fn gate_skips_model_validate_without_model_dir() {
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
    .expect("gate");
    assert_eq!(status_of(&report, "model_validate"), GateStatus::Skip);
    assert_eq!(
        report.outcome,
        GateOutcome::Pass,
        "{:?}",
        report.not_checked
    );
    // А на Standard она обязательна: SKIP даёт INCOMPLETE (exit 3).
    let standard = run_with(
        dir,
        Some(Route::Standard),
        None,
        None,
        (1, 4),
        &GateRequirements::default(),
    )
    .expect("gate");
    assert_eq!(standard.outcome, GateOutcome::Incomplete);
    assert!(
        standard.not_checked.contains(&"model_validate".to_string()),
        "{:?}",
        standard.not_checked
    );
}

/// На маршруте Critical NFR без способа проверки — error, а не warn.
#[test]
fn critical_route_promotes_nfr_without_verification() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = tmp.path();
    make_gate_repo(dir);
    write_model(
        dir,
        &[
            ("CMP-001", "depends_on: []"),
            ("NFR-001", "verification: \"\"\naffects: [CMP-001]"),
        ],
    );
    git(dir, &["add", "."]);
    git(dir, &["commit", "-q", "-m", "model"]);
    let standard = run_with(
        dir,
        Some(Route::Standard),
        None,
        None,
        (1, 4),
        &GateRequirements::default(),
    )
    .expect("gate");
    let comp = standard
        .components
        .iter()
        .find(|c| c.name == "model_validate")
        .expect("comp");
    assert_eq!(
        status_of(&standard, "model_validate"),
        GateStatus::Pass,
        "{:?}",
        comp.findings
    );
    let critical = run_with(
        dir,
        Some(Route::Critical),
        None,
        None,
        (1, 4),
        &GateRequirements::default(),
    )
    .expect("gate");
    assert_eq!(status_of(&critical, "model_validate"), GateStatus::Fail);
}
#[test]
fn gate_fails_when_rule_removed_to_green_the_gate() {
    let tmp = tempfile::tempdir().expect("tmp");
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).expect("mkdir");
    make_gate_repo(&repo);
    // Агент удалил правило no_pan, чтобы пройти гейт.
    std::fs::write(
            repo.join(".arch-handoff/CONSTRAINTS.yaml"),
            "rules:\n  - name: spine_present\n    type: file_exists\n    path: \"ARCHITECTURE-SPINE.md\"\n    severity: error\n",
        )
        .expect("ослабленный constraints");
    let report = run(&repo, None, None, None, (1, 4)).expect("гейт");
    assert!(!report.passed, "ослабление обязано валить гейт");
    assert_eq!(status_of(&report, "rule_weakened"), GateStatus::Fail);
    // delta guard молчит: его дефолт защищает корневой CONSTRAINTS.yaml,
    // а правка — в .arch-handoff/ (ослабление ловит именно rule_weakened).
    assert_eq!(status_of(&report, "delta_guard"), GateStatus::Pass);
    let text = render(&report);
    assert!(text.contains("rule_weakened"), "{text}");
    assert!(text.contains("no_pan"), "{text}");
    assert!(text.contains("Итог: FAIL"), "{text}");
    assert!(
        text.contains("Гейт поймал 1 нарушений до ревью — исправьте и перепроверьте"),
        "квитанция ценности при FAIL: {text}"
    );
}

#[test]
fn gate_active_override_legalizes_rule_removal() {
    let tmp = tempfile::tempdir().expect("tmp");
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).expect("mkdir");
    make_gate_repo(&repo);
    // Дельта покрывает правку CONSTRAINTS.yaml, override узаконивает
    // удаление правила (гейт «только через ADR»).
    std::fs::write(
            repo.join(".arch-handoff/CONSTRAINTS.yaml"),
            "rules:\n  - name: spine_present\n    type: file_exists\n    path: \"ARCHITECTURE-SPINE.md\"\n    severity: error\noverrides:\n  - rule: no_pan\n    adr: ADR-007\n    until: \"2999-01\"\n",
        )
        .expect("constraints с override");
    let delta_dir = repo.join("changes/drop-pan");
    std::fs::create_dir_all(&delta_dir).expect("mkdir delta");
    std::fs::write(
        delta_dir.join("DELTA.md"),
        "# Дельта\n\nСнимаем правило no_pan по ADR-007: CONSTRAINTS.yaml.\n",
    )
    .expect("delta");
    let report = run(&repo, None, None, None, (1, 4)).expect("гейт");
    assert_eq!(
        status_of(&report, "rule_weakened"),
        GateStatus::Pass,
        "активный override узаконивает: {}",
        render(&report)
    );
    assert_eq!(status_of(&report, "delta_guard"), GateStatus::Pass);
    assert!(report.passed, "{}", render(&report));
}

#[test]
fn gate_fails_on_broken_constraints_yaml() {
    let tmp = tempfile::tempdir().expect("tmp");
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).expect("mkdir");
    make_gate_repo(&repo);
    // Битый YAML при наличии входа — FAIL, а не молчаливый пропуск.
    std::fs::write(repo.join(".arch-handoff/CONSTRAINTS.yaml"), "{битый yaml")
        .expect("битый constraints");
    let report = run(&repo, None, None, None, (1, 4)).expect("гейт");
    assert!(!report.passed);
    assert_eq!(status_of(&report, "fitness"), GateStatus::Fail);
    assert_eq!(status_of(&report, "rule_weakened"), GateStatus::Fail);
}

#[test]
fn gate_fails_on_fitness_violation() {
    let tmp = tempfile::tempdir().expect("tmp");
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).expect("mkdir");
    make_gate_repo(&repo);
    // Удаляем обязательный файл: fitness FAIL, ослаблений правил нет.
    std::fs::remove_file(repo.join("ARCHITECTURE-SPINE.md")).expect("remove spine");
    let report = run(&repo, None, None, None, (1, 4)).expect("гейт");
    assert!(!report.passed);
    assert_eq!(status_of(&report, "fitness"), GateStatus::Fail);
    // spine_lint пропущен: файла нет — входа нет (fail-soft).
    assert_eq!(status_of(&report, "spine_lint"), GateStatus::Skip);
}

/// A2: пропуск error-правила из-за отсутствия прогонщика — составляющая
/// fitness уходит в SKIP (деталь для `GateComponent::skip`), warn-пропуск
/// вердикт не меняет. Детерминировано: отчёт собран руками, окружение не
/// участвует.
#[test]
fn runner_skip_detail_only_for_error_severity() {
    let skip = |rule: &str, severity: &str| control::RunnerSkippedRule {
        rule: rule.to_string(),
        severity: severity.to_string(),
        runners: vec!["pytest".to_string()],
        reason: "нет прогонщика pytest: `python3` есть, но нет модуля pytest \
                     (`python3 -m pip install pytest`)"
            .to_string(),
    };
    let report = |runner_skipped: Vec<control::RunnerSkippedRule>| control::FitnessReport {
        repo: PathBuf::from("."),
        passed: true,
        issues: Vec::new(),
        summary: String::new(),
        durations: Vec::new(),
        inherited: Vec::new(),
        overrides: Vec::new(),
        baseline: None,
        skipped: Vec::new(),
        changed_since: None,
        changed_files: None,
        skipped_unknown: Vec::new(),
        runner_skipped,
        untrusted_skipped: Vec::new(),
        fingerprint: None,
    };
    let detail = exec_skip_detail(&report(vec![
        skip("r_err", "error"),
        skip("r_warn", "warn"),
    ]))
    .expect("error-пропуск блокирует");
    assert!(detail.contains("r_err"), "{detail}");
    assert!(
        !detail.contains("r_warn"),
        "warn-пропуск не блокирует: {detail}"
    );
    assert!(detail.contains("pip install pytest"), "{detail}");
    assert!(
        exec_skip_detail(&report(vec![skip("r_warn", "warn")])).is_none(),
        "warn-пропуски не меняют вердикт"
    );
    assert!(exec_skip_detail(&report(Vec::new())).is_none());
}

/// A3: пропуск по модели доверия (no-exec/untrusted) блокирует при ЛЮБОМ
/// severity — иначе блок 3 паспорта не увидел бы warn-правила, не
/// исполненные по решению политики. Отчёт собран руками (детерминированно).
#[test]
fn untrusted_skip_detail_blocks_at_any_severity() {
    let report = |untrusted_skipped: Vec<control::UntrustedSkippedRule>| control::FitnessReport {
        repo: PathBuf::from("."),
        passed: true,
        issues: Vec::new(),
        summary: String::new(),
        durations: Vec::new(),
        inherited: Vec::new(),
        overrides: Vec::new(),
        baseline: None,
        skipped: Vec::new(),
        changed_since: None,
        changed_files: None,
        skipped_unknown: Vec::new(),
        runner_skipped: Vec::new(),
        untrusted_skipped,
        fingerprint: None,
    };
    let skip = |rule: &str, severity: &str| control::UntrustedSkippedRule {
        rule: rule.to_string(),
        severity: severity.to_string(),
        reason: crate::cmd_trust::deny_reason_text(crate::cmd_trust::DenyReason::NoExec),
    };
    let detail = exec_skip_detail(&report(vec![skip("warn_rule", "warn")]))
        .expect("warn-пропуск по доверию блокирует (A3)");
    assert!(detail.contains("warn_rule"), "{detail}");
    assert!(
        detail.contains(crate::cmd_trust::COMMAND_UNTRUSTED),
        "маркер находки в детали: {detail}"
    );
    assert!(exec_skip_detail(&report(Vec::new())).is_none());
}

/// A3 в гейте целиком: реестр с command-правилом при no-exec — составляющая
/// `fitness` SKIP с маркером `command_untrusted` и именем правила, вердикт
/// INCOMPLETE (fitness обязательна на всех маршрутах), а блок 3 паспорта
/// перечисляет пропущенное с причиной. Команда при этом НЕ исполняется
/// (маяк не создан). Политика инжектируется опцией — окружение не трогается.
#[test]
fn gate_no_exec_skips_fitness_and_passport_lists_it() {
    let tmp = tempfile::tempdir().expect("tmp");
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(repo.join(".arch-handoff")).expect("mkdir");
    std::fs::write(
        repo.join(".arch-handoff/CONSTRAINTS.yaml"),
        "rules:\n  - name: touched\n    type: command_succeeds\n    \
             command: 'touch marker.txt'\n    severity: error\n  - name: spine_present\n    \
             type: file_exists\n    path: \"ARCHITECTURE-SPINE.md\"\n    severity: error\n",
    )
    .expect("constraints");
    std::fs::write(repo.join("ARCHITECTURE-SPINE.md"), "# Spine\n").expect("spine");
    let options = GateOptions {
        exec: crate::cmd_trust::ExecPolicy {
            no_exec: true,
            trust_file: None,
        },
        ..GateOptions::default()
    };
    let report = run_opts(
        &repo,
        Some(crate::control::Route::Fast),
        None,
        None,
        (50, 50),
        &GateRequirements::default(),
        &options,
    )
    .expect("гейт");
    let fitness = report
        .components
        .iter()
        .find(|c| c.name == "fitness")
        .expect("составляющая fitness");
    assert_eq!(fitness.status, GateStatus::Skip, "{}", fitness.detail);
    assert!(
        fitness.detail.contains(crate::cmd_trust::COMMAND_UNTRUSTED),
        "{}",
        fitness.detail
    );
    assert!(fitness.detail.contains("touched"), "{}", fitness.detail);
    assert_eq!(
        report.outcome,
        GateOutcome::Incomplete,
        "обязательная составляющая в SKIP — зелёный неполон"
    );
    assert!(!repo.join("marker.txt").exists(), "команда не исполнялась");
    // Паспорт (блок 3): пропущенное по доверию перечислено с причиной.
    let passport = crate::passport::Passport::build(&report, &repo);
    let fitness_nc = passport
        .not_checked
        .iter()
        .find(|n| n.name == "fitness")
        .expect("fitness в блоке 3 паспорта");
    assert!(
        fitness_nc
            .reason
            .contains(crate::cmd_trust::COMMAND_UNTRUSTED),
        "{}",
        fitness_nc.reason
    );
    assert!(
        fitness_nc.reason.contains("touched"),
        "{}",
        fitness_nc.reason
    );
    assert!(fitness_nc.required, "fitness обязательна для Fast");
}
// --- составляющая `sensors` (D5) ----------------------------------------
/// Пишет спецификацию в `<repo>/docs/spec/<name>`.
fn write_spec(repo: &Path, name: &str, text: &str) {
    let dir = repo.join("docs/spec");
    std::fs::create_dir_all(&dir).expect("mkdir spec");
    std::fs::write(dir.join(name), text).expect("spec");
}

#[test]
fn gate_sensors_fail_when_spec_lost_required_section() {
    let tmp = tempfile::tempdir().expect("tmp");
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).expect("mkdir");
    make_gate_repo(&repo);
    // Red-team 06: из спеки удалена секция «## Критерии приёмки».
    write_spec(
        &repo,
        "payments.md",
        "# Спека\n\n## Проблема\nТекст.\n\n## Риски\nТекст.\n",
    );
    // Маршрут Standard: sensors в контуре (на Fast её нет — см. ниже).
    let report = run(&repo, Some(Route::Standard), None, None, (1, 4)).expect("гейт");
    assert!(!report.passed, "{}", render(&report));
    assert_eq!(status_of(&report, "sensors"), GateStatus::Fail);
    let text = render(&report);
    assert!(text.contains("required_sections"), "{text}");
    assert!(text.contains("## Критерии приёмки"), "{text}");
    // На маршруте Fast составляющей sensors нет вовсе (лёгкий контур).
    let fast = run(&repo, Some(Route::Fast), None, None, (1, 4)).expect("гейт fast");
    assert!(
        !fast.components.iter().any(|c| c.name == "sensors"),
        "маршрут Fast — sensors вне гейта"
    );
}

#[test]
fn gate_sensors_skip_without_docs_spec_and_pass_on_full_spec() {
    let tmp = tempfile::tempdir().expect("tmp");
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).expect("mkdir");
    make_gate_repo(&repo);
    // Без docs/spec — честный SKIP (fail-soft на инфраструктуру).
    let report = run(&repo, Some(Route::Standard), None, None, (1, 4)).expect("гейт");
    assert_eq!(status_of(&report, "sensors"), GateStatus::Skip);
    assert_eq!(
        report.outcome,
        GateOutcome::Incomplete,
        "{}",
        render(&report)
    );
    // Полная спека (все секции REQUIRED_SECTIONS, ссылок нет) — PASS.
    write_spec(
        &repo,
        "payments.md",
        "# Спека\n\n## Проблема\nТекст.\n\n## Критерии приёмки\n- [ ] тест.\n\n## Риски\nТекст.\n",
    );
    let report = run(&repo, Some(Route::Standard), None, None, (1, 4)).expect("гейт");
    assert_eq!(
        status_of(&report, "sensors"),
        GateStatus::Pass,
        "{}",
        render(&report)
    );
    // Требование к sensors выполнено: его нет в «не проверено». Итог всё
    // ещё INCOMPLETE — trace_check/nfr обязательны на Standard, а model/
    // в этом репозитории нет.
    assert!(!report.not_checked.iter().any(|n| n == "sensors"));
    assert!(report.not_checked.iter().any(|n| n == "trace_check"));
    assert_eq!(
        report.outcome,
        GateOutcome::Incomplete,
        "{}",
        render(&report)
    );
}
// --- пути ограничений и fail-closed `rule_weakened` (D6) -----------------

/// Ослабленный реестр: правило `no_pan` удалено (антикейс «зеленения» гейта).
const WEAKENED_CONSTRAINTS: &str = "rules:\n  - name: spine_present\n    type: file_exists\n    path: \"ARCHITECTURE-SPINE.md\"\n    severity: error\n";

#[test]
fn gate_explicit_constraints_inside_repo_keeps_weakened_protection() {
    let tmp = tempfile::tempdir().expect("tmp");
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).expect("mkdir");
    make_gate_repo(&repo);
    // Явный --constraints АБСОЛЮТНЫМ путём внутри репозитория (red-team,
    // наблюдение 1): раньше секция уходила в SKIP «вне репозитория».
    let explicit = repo.join(".arch-handoff/CONSTRAINTS.yaml");
    std::fs::write(&explicit, WEAKENED_CONSTRAINTS).expect("ослабленный constraints");
    let report = run(&repo, None, None, Some(&explicit), (1, 4)).expect("гейт");
    assert!(!report.passed, "{}", render(&report));
    assert_eq!(status_of(&report, "rule_weakened"), GateStatus::Fail);
    let text = render(&report);
    assert!(text.contains("no_pan"), "{text}");
}

#[test]
fn gate_explicit_constraints_outside_repo_fails_closed() {
    let tmp = tempfile::tempdir().expect("tmp");
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).expect("mkdir");
    make_gate_repo(&repo);
    // Файл ограничений ВНЕ репозитория: анти-ослабление невозможно —
    // FAIL с причиной, а не молчаливый SKIP.
    let outside_dir = tmp.path().join("outside");
    std::fs::create_dir_all(&outside_dir).expect("mkdir outside");
    let outside = outside_dir.join("CONSTRAINTS.yaml");
    std::fs::write(&outside, WEAKENED_CONSTRAINTS).expect("внешний constraints");
    let report = run(&repo, None, None, Some(&outside), (1, 4)).expect("гейт");
    assert!(!report.passed, "{}", render(&report));
    assert_eq!(status_of(&report, "rule_weakened"), GateStatus::Fail);
    let component = report
        .components
        .iter()
        .find(|c| c.name == "rule_weakened")
        .expect("составляющая");
    assert!(
        component.detail.contains("анти-ослабление невозможно")
            && component.detail.contains("вне репозитория"),
        "{}",
        component.detail
    );
    // Fitness при этом честно прогоняет внешний файл (вход есть).
    assert_eq!(status_of(&report, "fitness"), GateStatus::Pass);
}

#[test]
fn gate_on_repo_subdirectory_compares_against_case_file_not_outer_registry() {
    // Регрессия D6b: кейс-подкаталог внутри чужого монорепо (как кейсы/
    // внутри spine-core). `<rev>:<path>` резолвится git'ом от toplevel —
    // сравнение обязано идти с файлом кейса, а не с реестром внешнего репо.
    let tmp = tempfile::tempdir().expect("tmp");
    let outer = tmp.path().join("outer");
    let case = outer.join("cases").join("demo");
    std::fs::create_dir_all(&case).expect("mkdir case");
    // Ловушка: реестр внешнего репозитория с правилом, которого нет у кейса.
    std::fs::write(
            outer.join("CONSTRAINTS.yaml"),
            "rules:\n  - name: outer_only_rule\n    type: file_exists\n    path: \"OUTER.md\"\n    severity: error\n",
        )
        .expect("outer constraints");
    std::fs::write(
            case.join("CONSTRAINTS.yaml"),
            "rules:\n  - name: spine_present\n    type: file_exists\n    path: \"ARCHITECTURE-SPINE.md\"\n    severity: error\n  - name: no_pan\n    type: must_not_contain\n    glob: \"**/*.py\"\n    pattern: 'PAN'\n    severity: error\n",
        )
        .expect("case constraints");
    std::fs::write(case.join("ARCHITECTURE-SPINE.md"), "# Spine\n").expect("spine");
    git(&outer, &["init", "-q"]);
    git(&outer, &["add", "."]);
    git(&outer, &["commit", "-q", "-m", "init"]);
    // Ослабление реестра кейса в рабочем дереве: правило no_pan удалено.
    std::fs::write(case.join("CONSTRAINTS.yaml"), WEAKENED_CONSTRAINTS)
        .expect("ослабленный constraints");
    let report = run(&case, None, None, None, (1, 4)).expect("гейт");
    assert_eq!(
        status_of(&report, "rule_weakened"),
        GateStatus::Fail,
        "{}",
        render(&report)
    );
    let text = render(&report);
    assert!(text.contains("no_pan"), "{text}");
    assert!(
        !text.contains("outer_only_rule"),
        "сравнение с реестром внешнего репо даёт ложные находки: {text}"
    );
}

#[test]
fn gate_falls_back_to_root_constraints_yaml() {
    let tmp = tempfile::tempdir().expect("tmp");
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).expect("mkdir");
    // Кейс без handoff-пакета: реестр правил — КОРНЕВОЙ CONSTRAINTS.yaml
    // (как кейс 011 и сам этот репозиторий).
    std::fs::write(
            repo.join("CONSTRAINTS.yaml"),
            "rules:\n  - name: spine_present\n    type: file_exists\n    path: \"ARCHITECTURE-SPINE.md\"\n    severity: error\n  - name: no_pan\n    type: must_not_contain\n    glob: \"**/*.py\"\n    pattern: 'PAN'\n    severity: error\n",
        )
        .expect("constraints");
    std::fs::write(repo.join("ARCHITECTURE-SPINE.md"), "# Spine\n").expect("spine");
    git(&repo, &["init", "-q"]);
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-q", "-m", "init"]);
    // До ослабления: fitness прогоняется по корневому файлу (не SKIP),
    // rule_weakened сравнивает по корневому пути.
    let report = run(&repo, None, None, None, (1, 4)).expect("гейт");
    assert!(report.passed, "{}", render(&report));
    assert_eq!(status_of(&report, "fitness"), GateStatus::Pass);
    let fitness = report
        .components
        .iter()
        .find(|c| c.name == "fitness")
        .expect("составляющая");
    assert!(
        fitness.detail.contains("файл: CONSTRAINTS.yaml"),
        "секция печатает использованный путь: {}",
        fitness.detail
    );
    let weakened = report
        .components
        .iter()
        .find(|c| c.name == "rule_weakened")
        .expect("составляющая");
    assert_eq!(weakened.status, GateStatus::Pass);
    assert!(
        weakened.detail.contains("CONSTRAINTS.yaml"),
        "{}",
        weakened.detail
    );
    // Ослабление корневого реестра ловится тем же анти-ослаблением.
    std::fs::write(repo.join("CONSTRAINTS.yaml"), WEAKENED_CONSTRAINTS)
        .expect("ослабленный constraints");
    let report = run(&repo, None, None, None, (1, 4)).expect("гейт");
    assert!(!report.passed, "{}", render(&report));
    assert_eq!(status_of(&report, "rule_weakened"), GateStatus::Fail);
}

#[test]
fn gate_repo_without_commits_skips_weakened_honestly() {
    let tmp = tempfile::tempdir().expect("tmp");
    let repo = tmp.path().join("repo");
    make_uncommitted_repo(&repo);
    let report = run(&repo, None, None, None, (1, 4)).expect("гейт");
    // Базы для сравнения нет: rule_weakened честно SKIP; П1 — INCOMPLETE,
    // потому что маршрут Critical требует эту составляющую.
    assert!(!report.passed, "{}", render(&report));
    assert_eq!(report.outcome, GateOutcome::Incomplete);
    assert_eq!(status_of(&report, "rule_weakened"), GateStatus::Skip);
    let component = report
        .components
        .iter()
        .find(|c| c.name == "rule_weakened")
        .expect("составляющая");
    assert!(
        component.detail.contains("не существует"),
        "{}",
        component.detail
    );
}

// --- T-02: две копии реестра правил ----------------------------------

/// Расхождение двух копий реестра — находка `registry_diverged` (error), а
/// не пометка в тексте. Гейт читает пакетную копию первой, поэтому
/// расхождение означает: правила корневой копии — те, что написал
/// архитектор, — в вердикте не участвуют вовсе. Раньше `fitness` при этом
/// оставался PASS с припиской «копии реестра различаются».
#[test]
fn diverged_registries_are_a_finding_not_a_note() {
    let tmp = tempfile::tempdir().expect("tmp");
    let repo = tmp.path().join("repo");
    make_gate_repo(&repo);
    let packet_registry = repo.join(".arch-handoff/CONSTRAINTS.yaml");
    let packet_rules = std::fs::read_to_string(&packet_registry).expect("реестр пакета");
    // Корневая копия — другой реестр (в жизни так делает `bootstrap`:
    // реестр в корне, а `handoff` кладёт в пакет заготовку).
    std::fs::write(
            repo.join("CONSTRAINTS.yaml"),
            "rules:\n  - id: C-001\n    name: readme_exists\n    type: file_exists\n    path: \"README.md\"\n    severity: error\n",
        )
        .expect("корневой реестр");
    std::fs::write(repo.join("README.md"), "# Проект\n").expect("readme");

    let report = run(&repo, None, None, None, (1, 4)).expect("гейт");
    let fitness = report
        .components
        .iter()
        .find(|c| c.name == "fitness")
        .expect("составляющая");
    assert_eq!(status_of(&report, "fitness"), GateStatus::Fail);
    let finding = fitness
        .findings
        .iter()
        .find(|f| f.rule.as_deref() == Some("registry_diverged"))
        .unwrap_or_else(|| panic!("нет находки registry_diverged: {}", render(&report)));
    assert_eq!(finding.severity, "error");
    // Текст — действие, а не диагноз: названы оба пути и оба числа правил.
    assert!(finding.message.contains("2 правил"), "{}", finding.message);
    assert!(finding.message.contains("1 правил"), "{}", finding.message);
    assert!(finding.message.contains("cp "), "{}", finding.message);

    // Синхронизация копий снимает находку.
    std::fs::copy(repo.join("CONSTRAINTS.yaml"), &packet_registry).expect("синхронизация");
    let report = run(&repo, None, None, None, (1, 4)).expect("гейт");
    assert_eq!(status_of(&report, "fitness"), GateStatus::Pass);
    assert!(
        !render(&report).contains("registry_diverged"),
        "{}",
        render(&report)
    );

    // Приоритет копий виден в числах первой проверки: «прочитано 2»
    // относится к ПАКЕТНОЙ копии (`make_gate_repo`), а не к корневой.
    assert_eq!(
        packet_rules.lines().filter(|l| l.contains("name:")).count(),
        2
    );
}
#[test]
fn gate_delta_guard_detail_shows_coverage() {
    let tmp = tempfile::tempdir().expect("tmp");
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).expect("mkdir");
    make_gate_repo(&repo);
    // Защищённая правка, покрытая активной дельтой: деталь секции —
    // отчёт «что изменено и чем покрыто», а не голая галочка (D8).
    std::fs::write(repo.join("ARCHITECTURE-SPINE.md"), "# Spine v2\n").expect("edit");
    let delta_dir = repo.join("changes/spine-update");
    std::fs::create_dir_all(&delta_dir).expect("mkdir delta");
    std::fs::write(
        delta_dir.join("DELTA.md"),
        "# Дельта\n\nПравим ARCHITECTURE-SPINE.md (v2).\n",
    )
    .expect("delta");
    let report = run(&repo, None, None, None, (1, 4)).expect("гейт");
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
        component
            .detail
            .contains("покрытие: ARCHITECTURE-SPINE.md ← 'spine-update'"),
        "{}",
        component.detail
    );
}
// --- П1/П4/П7: честный зелёный, храповик маршрута, конверт вердикта ------

/// П1 (Д1): evidence-бандл в КОРНЕ репозитория виден гейту — раньше он
/// искался только в `changes/<имя>/` и составляющая молча уходила в SKIP.
#[test]
fn gate_finds_evidence_bundle_in_repo_root() {
    let tmp = tempfile::tempdir().expect("tmp");
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).expect("mkdir");
    make_gate_repo(&repo);
    // Неполный бандл Critical в корне: обязательных артефактов нет.
    std::fs::write(
        repo.join("EVIDENCE.yaml"),
        "route: Critical\npacked_at: \"2026-09-19T00:00:00+00:00\"\nitems: []\n",
    )
    .expect("bundle");
    let report = run(&repo, Some(Route::Critical), None, None, (1, 4)).expect("гейт");
    assert_eq!(
        status_of(&report, "evidence_verify"),
        GateStatus::Fail,
        "корневой бандл обязан проверяться: {}",
        render(&report)
    );
    assert_eq!(report.outcome, GateOutcome::Fail, "{}", render(&report));
}

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
    assert_eq!(
        status_of(&report, "decision_quality"),
        GateStatus::Pass,
        "warn не краснит составляющую: {}",
        render(&report)
    );
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
    let component = component_fitness(&repo, &implicit, &exec);
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
    let component = component_fitness(&repo, &explicit, &exec);
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
    assert!(mention_rule_notes(&repo, &registry).is_empty());
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
        passed: true,
        active_deltas: 1,
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
