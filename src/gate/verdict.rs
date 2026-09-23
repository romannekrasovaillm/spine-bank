//! Сборка вердикта гейта (B1): точки входа `run`/`run_opts`/`run_with`,
//! состав составляющих по маршруту и хэши входов вердикта (П7, ADR-043).

use std::path::{Path, PathBuf};
use std::process::Command;

use super::components::{
    component_decision_quality, component_delta_guard, component_evidence, component_fitness,
    component_model_validate, component_nfr, component_rule_weakened, component_sensors,
    component_spine_lint, component_trace, evidence_bundle_dirs,
};
use super::git::{ConstraintsPath, GitProbe, constraints_label};
use super::route::{
    auto_route, component_route_lock, read_route_lock, route_lock_path, route_rank,
};
use super::semantic::component_semantic_quality;
use super::types::{GateOptions, GateOutcome, GateReport, GateRequirements};
use crate::control::{self, Route};
use crate::error::{HarnessError, Result};

/// Прогоняет единый гейт по репозиторию с матрицей обязательности по
/// умолчанию (П1 ДКА). Полная форма — [`run_with`].
///
/// # Errors
/// Репозиторий недоступен. Провалы составляющих — НЕ ошибка: они в отчёте
/// (`outcome`/`passed = false`), exit-код ставит CLI-край.
pub fn run(
    repo: &Path,
    route_override: Option<Route>,
    base: Option<&str>,
    constraints: Option<&Path>,
    limits: (usize, usize),
) -> Result<GateReport> {
    run_with(
        repo,
        route_override,
        base,
        constraints,
        limits,
        &GateRequirements::default(),
    )
}

/// Полная форма: матрица обязательности + настройки семантики (0.3.4).
///
/// # Errors
/// Репозиторий недоступен. Провалы составляющих — НЕ ошибка.
pub fn run_opts(
    repo: &Path,
    route_override: Option<Route>,
    base: Option<&str>,
    constraints: Option<&Path>,
    limits: (usize, usize),
    requirements: &GateRequirements,
    options: &GateOptions,
) -> Result<GateReport> {
    run_inner(
        repo,
        route_override,
        base,
        constraints,
        limits,
        requirements,
        options,
    )
}

/// Прогоняет единый гейт по репозиторию с заданной матрицей обязательных
/// составляющих (`[gate.required]` конфига, П1 ДКА).
///
/// # Errors
/// Репозиторий недоступен. Провалы составляющих — НЕ ошибка: они в отчёте
/// (`outcome`/`passed = false`), exit-код ставит CLI-край.
pub fn run_with(
    repo: &Path,
    route_override: Option<Route>,
    base: Option<&str>,
    constraints: Option<&Path>,
    limits: (usize, usize),
    requirements: &GateRequirements,
) -> Result<GateReport> {
    run_inner(
        repo,
        route_override,
        base,
        constraints,
        limits,
        requirements,
        &GateOptions::default(),
    )
}

/// Тело гейта: единая точка сборки состава и настроек.
pub(super) fn run_inner(
    repo: &Path,
    route_override: Option<Route>,
    base: Option<&str>,
    constraints: Option<&Path>,
    limits: (usize, usize),
    requirements: &GateRequirements,
    options: &GateOptions,
) -> Result<GateReport> {
    if !repo.is_dir() {
        return Err(HarnessError::Control(format!(
            "репозиторий недоступен: {}",
            repo.display()
        )));
    }
    let (mut route, route_auto, mut route_note) = if let Some(r) = route_override {
        (r, false, format!("явный --route {r}"))
    } else {
        let (r, note) = auto_route(repo, base, limits, &options.diff_globs);
        (r, true, note)
    };
    // П4: храповик маршрута — эффективный маршрут не ниже заявленного в
    // ROUTE.lock. Критический проект проверяется как Critical даже на чистом
    // дереве и на маленьком MR (раньше auto давал Fast).
    let route_lock = read_route_lock(repo);
    if let Some(lock) = &route_lock {
        if route_rank(lock.route) > route_rank(route) {
            route_note = format!(
                "{route_note}; поднят ROUTE.lock → {} ({})",
                lock.route,
                lock.decided_by.as_deref().unwrap_or("без ADR")
            );
            route = lock.route;
        }
    }
    let constraints = if let Some(path) = constraints {
        ConstraintsPath {
            path: path.to_path_buf(),
            explicit: true,
            drift: None,
        }
    } else {
        // Единый резолвер (E2): пакетная копия → корневой fallback (D6);
        // ни одной копии — дефолтный путь, составляющие дадут SKIP (раньше
        // на кейсе без handoff-пакета гейт зеленел «из-за пропусков»).
        match control::resolve_constraints_path_detailed(repo, None) {
            Some(resolution) => ConstraintsPath {
                path: resolution.path,
                explicit: false,
                drift: resolution.drift,
            },
            None => ConstraintsPath {
                path: repo.join(control::HANDOFF_CONSTRAINTS_PATH),
                explicit: false,
                drift: None,
            },
        }
    };
    let git = GitProbe::probe(repo);

    let mut components = vec![
        component_fitness(repo, &constraints, &options.exec),
        component_delta_guard(repo, base, &git),
        component_rule_weakened(repo, &constraints, base.unwrap_or("HEAD"), &git),
        component_spine_lint(repo),
        component_trace(repo, options),
        // Н2: целостность модели — часть гейта на ЛЮБОМ маршруте (SKIP без
        // каталога model/); обязательность по маршрутам — в `[gate.required]`.
        component_model_validate(repo, route),
    ];
    if matches!(route, Route::Standard | Route::Critical) {
        components.push(component_sensors(repo));
        components.push(component_nfr(repo));
        components.push(component_evidence(repo, &options.evidence));
    }
    if let Some(lock) = &route_lock {
        components.push(component_route_lock(repo, base, &git, lock));
    }
    // Н7: качество решений — необязательная составляющая; включается только
    // через `[gate.required]` (по умолчанию SKIP, чтобы не краснить чужие
    // пайплайны без предупреждения).
    let required_names = requirements.for_route(route);
    components.push(component_decision_quality(
        repo,
        options,
        required_names.iter().any(|r| r == "decision_quality"),
    ));
    // ADR-052: смысловые рубрики — та же дисциплина, что у `decision_quality`:
    // необязательная составляющая, включается только через `[gate.required]`.
    components.push(component_semantic_quality(
        repo,
        &options.semantic_quality,
        &options.rubrics_dir,
        base,
        &git,
        required_names.iter().any(|r| r == "semantic_quality"),
    ));
    let mut report = GateReport {
        repo: repo.to_path_buf(),
        route,
        route_auto,
        route_note,
        components,
        outcome: GateOutcome::Pass,
        required: requirements.for_route(route).to_vec(),
        not_checked: Vec::new(),
        inputs: collect_inputs(repo, &constraints.path, base, &git),
        attestation: String::new(),
        passed: true,
    };
    report.recompute();
    Ok(report)
}

/// Хэши входов вердикта (П7): чем состояние репозитория отличалось при
/// прогоне — реестр правил, спайн, модель, бандлы доказательств, ROUTE.lock и
/// коммит базы диффа (Н3, ADR-043).
///
/// Только ОТНОСИТЕЛЬНЫЕ пути и никакого времени: тот же коммит, склонированный
/// в другой каталог, обязан дать ту же аттестацию. Отсутствующий вход —
/// честное `absent`, а не пустой хэш.
#[must_use]
pub(super) fn collect_inputs(
    repo: &Path,
    constraints: &Path,
    base: Option<&str>,
    git: &GitProbe,
) -> Vec<(String, String)> {
    let mut inputs: Vec<(String, String)> = Vec::new();
    let mut push = |name: &str, value: String| inputs.push((name.to_string(), value));

    push(
        "constraints",
        crate::hash::sha256_file(constraints)
            .map_or_else(|| "absent".to_string(), |h| format!("sha256:{h}")),
    );
    // T-02: ПО КАКОМУ реестру судили и сколько в нём правил. Хэш отвечает
    // «тот же файл или нет», но не отвечает «а какой файл-то»: при двух
    // копиях (пакетная приоритетна, корневая — fallback) это первое, что
    // нужно знать читателю вердикта. Путь относительный — аттестация не
    // зависит от каталога, куда склонирован репозиторий.
    push("constraints_path", constraints_label(repo, constraints));
    push(
        "constraints_rules",
        control::load_constraints_resolved(constraints)
            .map_or_else(|_| "unreadable".to_string(), |r| r.rules.len().to_string()),
    );
    // Спайн: оба исторических расположения (как у `spine_lint`).
    let spine = ["ARCHITECTURE-SPINE.md", "docs/ARCHITECTURE-SPINE.md"]
        .iter()
        .map(|p| repo.join(p))
        .find(|p| p.is_file());
    push(
        "spine",
        spine
            .and_then(|p| crate::hash::sha256_file(&p))
            .map_or_else(|| "absent".to_string(), |h| format!("sha256:{h}")),
    );
    push(
        "model",
        crate::hash::sha256_tree(&repo.join("model"))
            .map_or_else(|| "absent".to_string(), |h| format!("sha256:{h}")),
    );
    // Сырые ответы судьи рубрик: отчёт объявлен собранным ИЗ НИХ, поэтому
    // правка сохранённого ответа меняет вердикт о качестве решения — а значит
    // обязана менять и аттестацию (J5, ADR-048). Каталога нет (отчётов нет
    // либо они до появления сырых ответов) — честное `absent`: аттестация
    // существующих кейсов не меняется.
    push(
        "judge_raw",
        crate::hash::sha256_tree(&repo.join(crate::judge::RUBRIC_RAW_DIR))
            .map_or_else(|| "absent".to_string(), |h| format!("sha256:{h}")),
    );
    // Каждый проверенный бандл: правка EVIDENCE.yaml обязана менять аттестацию.
    let bundles = evidence_bundle_dirs(repo);
    push("evidence_bundles", bundles.len().to_string());
    for dir in &bundles {
        let rel = dir.strip_prefix(repo).map_or_else(
            |_| ".".to_string(),
            |p| {
                if p.as_os_str().is_empty() {
                    ".".to_string()
                } else {
                    p.display().to_string()
                }
            },
        );
        push(
            &format!("evidence:{rel}"),
            crate::hash::sha256_file(&dir.join("EVIDENCE.yaml"))
                .map_or_else(|| "absent".to_string(), |h| format!("sha256:{h}")),
        );
    }
    // Отчёты рубрик: правивший отчёт меняет вердикт, и аттестация обязана это
    // видеть — иначе подмена отчёта на «удобный» не отличима от прежнего
    // состояния. Касается и `decision_quality`, и смысловых рубрик (ADR-052):
    // отчёт смысловой рубрики входит в конверт тем же правилом.
    let reports_dir = repo.join(crate::rubric::RUBRIC_REPORTS_DIR);
    let mut reports: Vec<PathBuf> = std::fs::read_dir(&reports_dir)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.path())
                .filter(|p| {
                    p.extension()
                        .is_some_and(|x| x.eq_ignore_ascii_case("json"))
                })
                .collect()
        })
        .unwrap_or_default();
    reports.sort();
    push("rubric_reports", reports.len().to_string());
    for path in &reports {
        let name = path.file_name().map_or_else(
            || "report".to_string(),
            |n| n.to_string_lossy().into_owned(),
        );
        push(
            &format!("rubric_report:{name}"),
            crate::hash::sha256_file(path)
                .map_or_else(|| "absent".to_string(), |h| format!("sha256:{h}")),
        );
    }
    let lock = route_lock_path(repo);
    push(
        "route_lock",
        lock.and_then(|p| crate::hash::sha256_file(&p))
            .map_or_else(|| "absent".to_string(), |h| format!("sha256:{h}")),
    );
    // База диффа — коммитом, а не строкой аргумента: `HEAD~1` и его SHA
    // описывают одно состояние и обязаны дать одну аттестацию.
    // T-03: база приходит и голой ревизией, и диапазоном (`origin/main...HEAD`)
    // — для `git rev-parse` годится только одиночная ревизия, иначе коммит
    // базы молча уезжал в `absent`.
    let base_commit = if git.repo {
        git_resolve(repo, control::base_rev(base.unwrap_or("HEAD")))
    } else {
        None
    };
    push("base", base_commit.unwrap_or_else(|| "absent".to_string()));
    inputs
}

/// Резолвит git-ревизию в полный SHA коммита (None — не резолвится).
fn git_resolve(repo: &Path, rev: &str) -> Option<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["rev-parse", "--verify", &format!("{rev}^{{commit}}")])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let sha = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if sha.is_empty() { None } else { Some(sha) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gate::GateStatus;
    use crate::gate::render;
    use crate::gate::testkit::*;

    #[test]
    fn gate_passes_on_clean_repo_and_skips_without_inputs() {
        let tmp = tempfile::tempdir().expect("tmp");
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).expect("mkdir");
        make_gate_repo(&repo);
        let report = run(&repo, None, None, None, (1, 4)).expect("гейт");
        assert!(report.passed, "чистый репо: {}", render(&report));
        assert!(report.route_auto, "маршрут из диффа");
        // Дифф пуст → score 0 → Fast → nfr/evidence вне прогона.
        assert_eq!(report.route, Route::Fast);
        for name in ["fitness", "delta_guard", "rule_weakened", "spine_lint"] {
            assert_eq!(status_of(&report, name), GateStatus::Pass, "{name}");
        }
        assert_eq!(status_of(&report, "trace_check"), GateStatus::Skip);
        assert!(
            !report.components.iter().any(|c| c.name == "nfr"),
            "маршрут Fast — nfr вне гейта"
        );
        let text = render(&report);
        assert!(text.contains("Итог: PASS"), "{text}");
    }
    #[test]
    fn gate_non_git_repo_is_fail_soft() {
        let tmp = tempfile::tempdir().expect("tmp");
        let repo = tmp.path().join("plain");
        std::fs::create_dir_all(repo.join(".arch-handoff")).expect("mkdir");
        std::fs::write(
            repo.join(".arch-handoff/CONSTRAINTS.yaml"),
            "rules:\n  - name: spine_present\n    type: file_exists\n    path: \"ARCHITECTURE-SPINE.md\"\n    severity: error\n",
        )
        .expect("constraints");
        std::fs::write(repo.join("ARCHITECTURE-SPINE.md"), "# Spine\n").expect("spine");
        // Не git: delta guard и rule_weakened — SKIP; auto-маршрут — fail-safe.
        let report = run(&repo, None, None, None, (1, 4)).expect("гейт");
        // Fail-soft на инфраструктуру сохранён (FAIL-составляющих нет), но
        // П1: обязательные для fail-safe Critical составляющие без входа →
        // честный INCOMPLETE, а не зелёный PASS.
        assert!(!report.passed, "{}", render(&report));
        assert_eq!(
            report.outcome,
            GateOutcome::Incomplete,
            "{}",
            render(&report)
        );
        assert!(
            report
                .components
                .iter()
                .all(|c| c.status != GateStatus::Fail),
            "fail-soft: ни одна составляющая не провалена"
        );
        assert_eq!(report.route, Route::Critical, "fail-safe без диффа");
        assert_eq!(status_of(&report, "delta_guard"), GateStatus::Skip);
        assert_eq!(status_of(&report, "rule_weakened"), GateStatus::Skip);
        assert_eq!(status_of(&report, "fitness"), GateStatus::Pass);
    }

    #[test]
    fn gate_standard_route_adds_nfr_and_evidence_components() {
        let tmp = tempfile::tempdir().expect("tmp");
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).expect("mkdir");
        make_gate_repo(&repo);
        let report = run(&repo, Some(Route::Standard), None, None, (1, 4)).expect("гейт");
        assert!(!report.route_auto, "явный маршрут");
        assert_eq!(status_of(&report, "nfr"), GateStatus::Skip, "нет model/");
        assert_eq!(
            status_of(&report, "evidence_verify"),
            GateStatus::Skip,
            "нет активных бандлов"
        );
    }
    #[test]
    fn gate_missing_repo_is_error_not_report() {
        let tmp = tempfile::tempdir().expect("tmp");
        let err = run(&tmp.path().join("ghost"), None, None, None, (1, 4))
            .expect_err("несуществующий репозиторий");
        assert!(err.to_string().contains("недоступен"), "{err}");
    }
    /// Паспорт (JSON `inputs`) называет ПУТЬ прочитанного реестра и число
    /// правил: хэш отвечает «тот же файл или нет», но не «какой именно файл».
    #[test]
    fn inputs_name_the_registry_and_rule_count() {
        let tmp = tempfile::tempdir().expect("tmp");
        let repo = tmp.path().join("repo");
        make_gate_repo(&repo);
        let report = run(&repo, None, None, None, (1, 4)).expect("гейт");
        let inputs: std::collections::BTreeMap<String, String> =
            report.inputs.iter().cloned().collect();
        assert_eq!(
            inputs.get("constraints_path").map(String::as_str),
            Some(".arch-handoff/CONSTRAINTS.yaml")
        );
        assert_eq!(
            inputs.get("constraints_rules").map(String::as_str),
            Some("2")
        );
    }
}
