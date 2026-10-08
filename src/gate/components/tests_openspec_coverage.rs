//! Тесты составляющей `openspec_coverage` (F2, ADR-067): покрытие требований
//! `OpenSpec` правилами реестра как часть единого гейта — по `[gate.required]`,
//! с областью changed/all и связкой с измерением зубьев правил (волна B).
//! Вынесены отдельным файлом — родительский `tests.rs` держится под границей
//! `prod_file_length_limit` (C-33), прецедент — `tests_secrets.rs`,
//! `tests_delta_guard_openspec.rs`.

use super::*;
use crate::gate::testkit::*;

use crate::gate::{GateOptions, GateReport, GateRequirements, GateStatus, render, run_opts};

/// Идентификатор требования живой спеки фикстуры.
fn live_req_id() -> String {
    crate::openspec::requirement_id(
        "payments",
        &["Система SHALL хранить суммы в minor units.".to_string()],
    )
}

/// Идентификатор требования дельты активного change фикстуры.
fn delta_req_id() -> String {
    crate::openspec::requirement_id(
        "payments",
        &["Повторный вызов MUST NOT менять лимит.".to_string()],
    )
}

/// Репо-фикстура F2: git + корневой реестр + разметка `OpenSpec` (живая спека
/// и активный change `add-limits`, у каждого по одному SHALL). `covers`
/// перечисляет, какие из двух требований покрывает правило-детектор.
fn make_f2_repo(repo: &Path, covers: &[String]) {
    let mut constraints = String::from(
        "rules:\n  - name: spine_present\n    type: file_exists\n    path: \"ARCHITECTURE-SPINE.md\"\n    severity: error\n",
    );
    if !covers.is_empty() {
        let list = covers
            .iter()
            .map(|id| format!("\"{id}\""))
            .collect::<Vec<_>>()
            .join(", ");
        let _ = writeln!(
            constraints,
            "  - name: no_f64_money\n    type: must_not_contain\n    glob: \"src/**/*.rs\"\n    \
             pattern: '\\bf64\\b'\n    severity: error\n    covers: [{list}]"
        );
    }
    write_fixture(repo, "CONSTRAINTS.yaml", &constraints);
    write_fixture(repo, "ARCHITECTURE-SPINE.md", "# Spine\n");
    write_fixture(repo, "src/lib.rs", "pub fn charge() -> u64 { 1 }\n");
    write_fixture(
        repo,
        "openspec/specs/payments/spec.md",
        "# payments\n\n### Requirement: Точные деньги\n\
         Система SHALL хранить суммы в minor units.\n",
    );
    write_fixture(
        repo,
        "openspec/changes/add-limits/proposal.md",
        "## Why\nНужны лимиты.\n",
    );
    write_fixture(
        repo,
        "openspec/changes/add-limits/specs/payments/spec.md",
        "## ADDED Requirements\n\n### Requirement: Лимиты идемпотентны\n\
         Повторный вызов MUST NOT менять лимит.\n",
    );
    git(repo, &["init", "-q"]);
    git(repo, &["add", "."]);
    git(repo, &["commit", "-q", "-m", "init"]);
}

/// Матрица обязательности: `openspec_coverage` обязательна на Critical.
fn requirements_with_openspec() -> GateRequirements {
    let critical = vec![
        "fitness".to_string(),
        "spine_lint".to_string(),
        "openspec_coverage".to_string(),
    ];
    GateRequirements {
        fast: vec!["fitness".to_string()],
        standard: vec!["fitness".to_string()],
        critical,
    }
}

/// Настройки гейта с областью `all` (полное покрытие, без диффа).
fn opts_all() -> GateOptions {
    GateOptions {
        openspec_coverage: crate::config::OpenspecCoverageConfig {
            scope: crate::config::OpenspecCoverageScope::All,
        },
        ..GateOptions::default()
    }
}

/// Прогон гейта фикстуры на явном маршруте Critical с заданной матрицей.
fn run_f2(repo: &Path, requirements: &GateRequirements, options: &GateOptions) -> GateReport {
    run_opts(
        repo,
        Some(Route::Critical),
        None,
        None,
        (1, 4),
        requirements,
        options,
    )
    .expect("гейт")
}

/// Составляющая `openspec_coverage` из отчёта.
fn component_of(report: &GateReport) -> &GateComponent {
    report
        .components
        .iter()
        .find(|c| c.name == "openspec_coverage")
        .expect("составляющая openspec_coverage в отчёте")
}

/// Без каталога `openspec/` составляющая — SKIP с явной причиной (паспорт
/// подхватывает её в блок «не проверено»).
#[test]
fn openspec_coverage_skips_without_openspec_dir() {
    let tmp = tempfile::tempdir().expect("tmp");
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).expect("mkdir");
    make_gate_repo(&repo);
    let report = run_f2(
        &repo,
        &requirements_with_openspec(),
        &GateOptions::default(),
    );
    let component = component_of(&report);
    assert_eq!(component.status, GateStatus::Skip, "{}", render(&report));
    assert!(
        component.detail.contains("openspec/"),
        "явная пометка об отсутствии разметки: {}",
        component.detail
    );
}

/// Приёмка F2, красная половина: `openspec_coverage` в `[gate.required]`
/// маршрута → непокрытый SHALL активного change — error-находка
/// `requirement_uncovered`, составляющая FAIL, гейт красный.
#[test]
fn openspec_coverage_fails_on_uncovered_shall_when_required() {
    let tmp = tempfile::tempdir().expect("tmp");
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).expect("mkdir");
    make_f2_repo(&repo, &[]);
    let report = run_f2(&repo, &requirements_with_openspec(), &opts_all());
    let component = component_of(&report);
    assert_eq!(component.status, GateStatus::Fail, "{}", render(&report));
    let uncovered: Vec<&GateFinding> = component
        .findings
        .iter()
        .filter(|f| f.rule.as_deref() == Some("requirement_uncovered"))
        .collect();
    assert_eq!(uncovered.len(), 2, "оба SHALL без решения: {uncovered:?}");
    assert!(
        uncovered
            .iter()
            .all(|f| f.severity == "error" && f.message.contains("covers:")),
        "error с подсказкой про covers:: {uncovered:?}"
    );
    assert!(
        uncovered
            .iter()
            .any(|f| f.message.contains(&delta_req_id())),
        "требование change названо по id: {uncovered:?}"
    );
    assert!(!report.passed, "гейт красный: {}", render(&report));
}

/// Не в `[gate.required]` — та же непокрытость остаётся warn: составляющая
/// проходит, предупреждения доезжают (схема «warn → error по включению»).
#[test]
fn openspec_coverage_warns_when_not_required() {
    let tmp = tempfile::tempdir().expect("tmp");
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).expect("mkdir");
    make_f2_repo(&repo, &[]);
    let report = run_f2(&repo, &GateRequirements::default(), &opts_all());
    let component = component_of(&report);
    assert_eq!(
        component.status,
        GateStatus::Pass,
        "warn не валит составляющую: {}",
        render(&report)
    );
    assert!(
        component
            .findings
            .iter()
            .any(|f| f.rule.as_deref() == Some("requirement_uncovered") && f.severity == "warn"),
        "находка остаётся warn: {:?}",
        component.findings
    );
}

/// Приёмка F2, зелёная половина + связка с волной B: правило с `covers:` без
/// измеренных зубьев — покрытие «текстом» (отдельная строка детали и пометка
/// в `not_verified`); с подтверждёнными зубьями (teeth.json, confirmed) —
/// полное покрытие.
#[test]
fn openspec_coverage_distinguishes_text_and_confirmed_teeth() {
    let tmp = tempfile::tempdir().expect("tmp");
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).expect("mkdir");
    make_f2_repo(&repo, &[live_req_id(), delta_req_id()]);
    let report = run_f2(&repo, &requirements_with_openspec(), &opts_all());
    let component = component_of(&report);
    assert_eq!(
        component.status,
        GateStatus::Pass,
        "покрыто — гейт зелёный: {}",
        render(&report)
    );
    assert!(
        component.detail.contains("покрыто текстом: 2"),
        "текстовое покрытие — отдельной строкой: {}",
        component.detail
    );
    assert!(
        component
            .not_verified
            .iter()
            .any(|n| n.contains("rules teeth --save")),
        "неизмеренность зубьев названа: {:?}",
        component.not_verified
    );

    // Измерение с подтверждёнными зубьями у покрывающего правила.
    write_teeth(&repo, "confirmed");
    let report = run_f2(&repo, &requirements_with_openspec(), &opts_all());
    let component = component_of(&report);
    assert_eq!(component.status, GateStatus::Pass, "{}", render(&report));
    assert!(
        component.detail.contains("с подтверждёнными зубьями: 2"),
        "оба требования покрыты правилом с зубьями: {}",
        component.detail
    );
    assert!(
        component.detail.contains("покрыто текстом: 0"),
        "текстового покрытия больше нет: {}",
        component.detail
    );
}

/// Правило, измеренное беззубым (toothless в teeth.json), — покрытие
/// формально: warn-находка `requirement_text_only` с подсказкой.
#[test]
fn openspec_coverage_flags_measured_toothless_cover() {
    let tmp = tempfile::tempdir().expect("tmp");
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).expect("mkdir");
    make_f2_repo(&repo, &[live_req_id(), delta_req_id()]);
    write_teeth(&repo, "toothless");
    let report = run_f2(&repo, &requirements_with_openspec(), &opts_all());
    let component = component_of(&report);
    assert_eq!(
        component.status,
        GateStatus::Pass,
        "покрытие есть, блокировки нет: {}",
        render(&report)
    );
    let text_only: Vec<&GateFinding> = component
        .findings
        .iter()
        .filter(|f| f.rule.as_deref() == Some("requirement_text_only"))
        .collect();
    assert_eq!(
        text_only.len(),
        2,
        "оба требования — по находке: {text_only:?}"
    );
    assert!(
        text_only
            .iter()
            .all(|f| f.severity == "warn" && f.message.contains("no_f64_money")),
        "warn с именем беззубого правила: {text_only:?}"
    );
}

/// Область `changed`: в неё входят требования дельт changes, затронутых
/// диффом, и требования живых спек, чьи файлы изменены; остальное — вне
/// области (дешёвый режим потока доработок).
#[test]
fn openspec_coverage_changed_scope_follows_the_diff() {
    let tmp = tempfile::tempdir().expect("tmp");
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).expect("mkdir");
    // Покрыто только требование change; требование живой спеки — без решения.
    make_f2_repo(&repo, &[delta_req_id()]);
    let options = GateOptions {
        openspec_coverage: crate::config::OpenspecCoverageConfig::default(),
        ..GateOptions::default()
    };
    // Чистое дерево: область changed пуста — непокрытое требование живой
    // спеки вне области, составляющая зелёная.
    let report = run_f2(&repo, &requirements_with_openspec(), &options);
    let component = component_of(&report);
    assert_eq!(
        component.status,
        GateStatus::Pass,
        "чистое дерево — область пуста: {}",
        render(&report)
    );
    assert!(
        component.detail.contains("SHALL: 0"),
        "в области никого: {}",
        component.detail
    );

    // Правим ДЕЛЬТУ change (добавляем второе, непокрытое требование) —
    // область входит весь change: его непокрытое требование краснит гейт,
    // а требование живой спеки остаётся вне области.
    write_fixture(
        &repo,
        "openspec/changes/add-limits/specs/payments/spec.md",
        "## ADDED Requirements\n\n### Requirement: Лимиты идемпотентны\n\
         Повторный вызов MUST NOT менять лимит.\n\n\
         ### Requirement: Лимиты атомарны\n\
         Смена лимита SHALL быть атомарной.\n",
    );
    let report = run_f2(&repo, &requirements_with_openspec(), &options);
    let component = component_of(&report);
    assert_eq!(component.status, GateStatus::Fail, "{}", render(&report));
    let uncovered: Vec<&GateFinding> = component
        .findings
        .iter()
        .filter(|f| f.rule.as_deref() == Some("requirement_uncovered"))
        .collect();
    assert_eq!(
        uncovered.len(),
        1,
        "только новое требование change: {uncovered:?}"
    );
    assert!(
        uncovered[0].message.contains("Лимиты атомарны"),
        "{:?}",
        uncovered[0].message
    );
    assert!(
        !uncovered[0].message.contains(&live_req_id()),
        "живая спека вне области: {:?}",
        uncovered[0].message
    );

    // Правим ЖИВУЮ спеку (незакоммиченная правка) — её требования входят
    // в область, и её непокрытое требование краснит гейт.
    git(&repo, &["checkout", "--", "openspec/changes/add-limits"]);
    write_fixture(
        &repo,
        "openspec/specs/payments/spec.md",
        "# payments\n\n### Requirement: Точные деньги\n\
         Система SHALL хранить суммы в minor units точно.\n",
    );
    let report = run_f2(&repo, &requirements_with_openspec(), &options);
    let component = component_of(&report);
    assert_eq!(
        component.status,
        GateStatus::Fail,
        "изменённая живая спека входит в область: {}",
        render(&report)
    );
    assert!(
        component
            .findings
            .iter()
            .any(|f| f.rule.as_deref() == Some("requirement_uncovered")),
        "{:?}",
        component.findings
    );
}

/// Осиротевшая ссылка `covers:` (F4) — warn-находка `covers_orphan` и в
/// гейте, не только в `openspec coverage`; блокирующей она не становится.
#[test]
fn openspec_coverage_reports_orphan_covers_as_warn() {
    let tmp = tempfile::tempdir().expect("tmp");
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).expect("mkdir");
    make_f2_repo(&repo, &[live_req_id(), delta_req_id()]);
    // Второе правило со ссылкой на несуществующий id (текст требования правили).
    let constraints = format!(
        "rules:\n  - name: spine_present\n    type: file_exists\n    path: \"ARCHITECTURE-SPINE.md\"\n    severity: error\n  - name: no_f64_money\n    type: must_not_contain\n    glob: \"src/**/*.rs\"\n    pattern: '\\bf64\\b'\n    severity: error\n    covers: [\"{}\", \"{}\"]\n  - name: stale_link\n    type: must_contain\n    glob: \"src/**/*.rs\"\n    pattern: \"charge\"\n    severity: error\n    covers: [\"openspec:payments#deadbeef\"]\n",
        live_req_id(),
        delta_req_id()
    );
    write_fixture(&repo, "CONSTRAINTS.yaml", &constraints);
    let report = run_f2(&repo, &requirements_with_openspec(), &opts_all());
    let component = component_of(&report);
    assert_eq!(
        component.status,
        GateStatus::Pass,
        "орфан — warn, не блок: {}",
        render(&report)
    );
    assert!(
        component
            .findings
            .iter()
            .any(|f| f.rule.as_deref() == Some("covers_orphan")
                && f.severity == "warn"
                && f.message.contains("openspec:payments#deadbeef")
                && f.message.contains("stale_link")),
        "находка называет id и правило: {:?}",
        component.findings
    );
}

/// Пишет `.arch-handoff/teeth.json` с заданным статусом для правила
/// `no_f64_money` (отпечаток — по живому правилу реестра фикстуры).
fn write_teeth(repo: &Path, status: &str) {
    let resolved = crate::control::load_constraints_resolved(&repo.join("CONSTRAINTS.yaml"))
        .expect("реестр фикстуры");
    let rule = resolved
        .rules
        .iter()
        .find(|r| r.name == "no_f64_money")
        .expect("правило no_f64_money");
    let fingerprint = crate::control::teeth::rule_fingerprint(rule);
    let json = format!(
        "{{\"schema\":\"{}\",\"case\":\"{}\",\"measured_at\":\"2026-10-08T00:00:00+00:00\",\
         \"entries\":[{{\"name\":\"no_f64_money\",\"kind\":\"must_not_contain\",\
         \"status\":\"{status}\",\"fingerprint\":\"{fingerprint}\",\"detail\":\"тест\"}}]}}",
        crate::control::teeth::TEETH_SCHEMA,
        repo.display()
    );
    write_fixture(repo, ".arch-handoff/teeth.json", &json);
}
