//! Тесты составляющих гейта: вынесены из `components.rs` чистым
//! перемещением — продовый модуль обязан укладываться в границу
//! `prod_file_length_limit` (C-33), а тесты растут быстрее кода.
//! Видимость прежняя: `super::*` — это модуль `components`.

use super::*;
use crate::gate::testkit::*;

use crate::gate::{
    GateOptions, GateOutcome, GateRequirements, GateStatus, render, run, run_opts, run_with,
};

// --- Н7: качество решений как составляющая гейта (ADR-042) -------------

/// Репозиторий с одним Accepted-ADR и (опционально) отчётом рубрики.
pub(crate) fn make_quality_repo(dir: &Path, score: Option<f64>, author: Option<&str>) {
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
pub(crate) fn with_quality(route: Route) -> GateRequirements {
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
    // удаление правила (гейт «только через ADR»). A2: ADR существует,
    // принят, срок в горизонте.
    let until = (chrono::Local::now() + chrono::Duration::days(182))
        .format("%Y-%m-%d")
        .to_string();
    std::fs::write(
            repo.join(".arch-handoff/CONSTRAINTS.yaml"),
            format!("rules:\n  - name: spine_present\n    type: file_exists\n    path: \"ARCHITECTURE-SPINE.md\"\n    severity: error\noverrides:\n  - rule: no_pan\n    adr: ADR-007\n    until: \"{until}\"\n"),
        )
        .expect("constraints с override");
    std::fs::create_dir_all(repo.join("docs/adr")).expect("mkdir adr");
    std::fs::write(
        repo.join("docs/adr/ADR-007-fixture.md"),
        "# ADR-007. Фикстура\n\n- Date: 2026-01-01\n- Status: Accepted\n\n## Context\n\nтест\n",
    )
    .expect("ADR-007");
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
        requires_skipped: Vec::new(),
        fingerprint: None,
    };
    let detail = skip_detail(&report(vec![
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
        skip_detail(&report(vec![skip("r_warn", "warn")])).is_none(),
        "warn-пропуски не меняют вердикт"
    );
    assert!(skip_detail(&report(Vec::new())).is_none());
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
        requires_skipped: Vec::new(),
        fingerprint: None,
    };
    let skip = |rule: &str, severity: &str| control::UntrustedSkippedRule {
        rule: rule.to_string(),
        severity: severity.to_string(),
        reason: crate::cmd_trust::deny_reason_text(crate::cmd_trust::DenyReason::NoExec),
    };
    let detail = skip_detail(&report(vec![skip("warn_rule", "warn")]))
        .expect("warn-пропуск по доверию блокирует (A3)");
    assert!(detail.contains("warn_rule"), "{detail}");
    assert!(
        detail.contains(crate::cmd_trust::COMMAND_UNTRUSTED),
        "маркер находки в детали: {detail}"
    );
    assert!(skip_detail(&report(Vec::new())).is_none());
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

// --- ADR-068: requires — ресурсный SKIP в гейте --------------------------

/// `skip_detail` включает ресурсные пропуски при ЛЮБОМ severity (как A3):
/// перечень правил + обязательная строка прогона на стенде.
#[test]
fn requires_skip_detail_lists_rules_and_stand_line() {
    let report = |requires_skipped: Vec<control::RequiresSkippedRule>| control::FitnessReport {
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
        untrusted_skipped: Vec::new(),
        requires_skipped,
        fingerprint: None,
    };
    let skip = |rule: &str, severity: &str| control::RequiresSkippedRule {
        rule: rule.to_string(),
        severity: severity.to_string(),
        resources: vec!["cuda".to_string()],
        reason: format!(
            "недоступны ресурсы среды: cuda — {}",
            control::requires::STAND_RUN_LINE
        ),
    };
    let detail = skip_detail(&report(vec![
        skip("warn_rule", "warn"),
        skip("err_rule", "error"),
    ]))
    .expect("ресурсный пропуск блокирует");
    assert!(detail.contains("warn_rule"), "{detail}");
    assert!(detail.contains("err_rule"), "{detail}");
    assert!(
        detail.contains(control::requires::STAND_RUN_LINE),
        "{detail}"
    );
    assert!(skip_detail(&report(Vec::new())).is_none());
}

/// Реестр с правилом, привязанным к `cuda`, при недоступном ресурсе —
/// составляющая `fitness` SKIP с перечнем правил и обязательной строкой;
/// ресурс доступен — правило исполняется как обычно (FAIL при нарушении).
#[test]
fn gate_requires_absent_resource_skips_fitness_with_stand_line() {
    let tmp = tempfile::tempdir().expect("tmp");
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(repo.join(".arch-handoff")).expect("mkdir");
    std::fs::write(
        repo.join(".arch-handoff/CONSTRAINTS.yaml"),
        "rules:\n  - name: gpu_only\n    type: file_exists\n    path: \"NOPE.md\"\n    \
         severity: error\n    requires: [cuda]\n",
    )
    .expect("constraints");
    std::fs::write(repo.join("ARCHITECTURE-SPINE.md"), "# Spine\n").expect("spine");
    // Ресурс недоступен: правило не прогоняется — SKIP, а не FAIL.
    let absent = GateOptions {
        resources: Some(control::requires::AvailableResources::none()),
        ..GateOptions::default()
    };
    let report = run_opts(
        &repo,
        Some(crate::control::Route::Fast),
        None,
        None,
        (50, 50),
        &GateRequirements::default(),
        &absent,
    )
    .expect("гейт");
    let fitness = report
        .components
        .iter()
        .find(|c| c.name == "fitness")
        .expect("составляющая fitness");
    assert_eq!(fitness.status, GateStatus::Skip, "{}", fitness.detail);
    assert!(fitness.detail.contains("gpu_only"), "{}", fitness.detail);
    assert!(
        fitness.detail.contains(control::requires::STAND_RUN_LINE),
        "{}",
        fitness.detail
    );
    assert_eq!(
        report.outcome,
        GateOutcome::Incomplete,
        "обязательная составляющая в SKIP — зелёный неполон"
    );
    // Ресурс доступен: обычная семантика — правило исполняется и краснеет.
    let present = GateOptions {
        resources: Some(control::requires::AvailableResources::all()),
        ..GateOptions::default()
    };
    let report = run_opts(
        &repo,
        Some(crate::control::Route::Fast),
        None,
        None,
        (50, 50),
        &GateRequirements::default(),
        &present,
    )
    .expect("гейт");
    assert_eq!(status_of(&report, "fitness"), GateStatus::Fail);
    assert!(
        !report
            .components
            .iter()
            .find(|c| c.name == "fitness")
            .expect("fitness")
            .detail
            .contains(control::requires::STAND_RUN_LINE),
        "ресурс доступен — не строки про стенд"
    );
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
            .contains("покрытие: ARCHITECTURE-SPINE.md ← 'delta:spine-update'"),
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
