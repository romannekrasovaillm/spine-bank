//! Маршрут гейта (B1): вычисление из git-диффа (`--route auto`, S-1,
//! ADR-034) и храповик заявленного маршрута `ROUTE.lock` (П4).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use super::git::{
    GitProbe, base_rev, git_rel_path, git_rev_exists, git_rev_has_path, git_show_file,
};
use super::types::{GateComponent, GateFinding};
use crate::control::{self, Route};

/// Вычисляет маршрут из git-диффа (`--route auto`): [`control::detect_diff_triggers`]
/// и [`control::score_with_sources`] с пустым declared (механический минимум
/// S-1, ADR-034). Дифф недоступен (не git-репозиторий, нет HEAD) — fail-safe
/// маршрут Critical с пометкой причины.
pub(super) fn auto_route(
    repo: &Path,
    base: Option<&str>,
    limits: (usize, usize),
    globs: &control::DiffGlobs,
) -> (Route, String) {
    match control::detect_diff_triggers_with(repo, base, globs) {
        Ok(diff) => {
            let scored = control::score_with_sources(&BTreeMap::new(), &diff, limits.0, limits.1);
            let fired = if scored.significance.fired.is_empty() {
                "триггеров нет".to_string()
            } else {
                scored.significance.fired.join(", ")
            };
            let excluded_note = if diff.excluded.is_empty() {
                String::new()
            } else {
                format!(
                    "; исключено по манифесту connect/.spineignore: {} файлов",
                    diff.excluded.len()
                )
            };
            (
                scored.significance.route,
                format!(
                    "auto: score {} ({fired}){excluded_note}",
                    scored.significance.score
                ),
            )
        }
        Err(e) => (
            Route::Critical,
            format!("auto: дифф недоступен ({e}) — fail-safe маршрут Critical"),
        ),
    }
}

/// Заявленный маршрут репозитория из `.arch-handoff/ROUTE.lock` (П4 ДКА).
#[derive(Debug, Clone)]
pub(super) struct RouteLock {
    /// Минимальный маршрут контроля для репозитория.
    pub(super) route: Route,
    /// Кем решён (ожидается ссылка на ADR при понижении).
    pub(super) decided_by: Option<String>,
}

/// Сырой YAML `ROUTE.lock`.
#[derive(Debug, serde::Deserialize)]
struct RouteLockRaw {
    /// Маршрут строкой (`fast|standard|critical`).
    route: String,
    /// Ссылка на решение (ADR-…).
    #[serde(default)]
    decided_by: Option<String>,
}

/// Ранг маршрута для операции «не ниже»: Fast < Standard < Critical.
pub(super) fn route_rank(route: Route) -> u8 {
    match route {
        Route::Fast => 0,
        Route::Standard => 1,
        Route::Critical => 2,
    }
}

/// Разбирает `ROUTE.lock`; невалидный YAML/маршрут — `None` (fail-soft:
/// файла нет или он битый не должен валить гейт, но и не поднимает маршрут).
fn parse_route_lock(text: &str) -> Option<RouteLock> {
    let raw: RouteLockRaw = serde_yaml_ng::from_str(text).ok()?;
    let route = raw
        .route
        .trim()
        .to_ascii_lowercase()
        .parse::<Route>()
        .ok()?;
    Some(RouteLock {
        route,
        decided_by: raw.decided_by,
    })
}

/// Путь `ROUTE.lock` в репозитории (пакетный, затем корневой).
pub(super) fn route_lock_path(repo: &Path) -> Option<PathBuf> {
    let handoff = repo.join(".arch-handoff/ROUTE.lock");
    if handoff.is_file() {
        return Some(handoff);
    }
    let root = repo.join("ROUTE.lock");
    root.is_file().then_some(root)
}

/// Заявленный маршрут репозитория.
pub(super) fn read_route_lock(repo: &Path) -> Option<RouteLock> {
    let path = route_lock_path(repo)?;
    let text = std::fs::read_to_string(path).ok()?;
    parse_route_lock(&text)
}

/// Составляющая `route_lock` (П4): заявленный маршрут и анти-понижение.
/// Понижение относительно git-базы без `decided_by: ADR-…` — FAIL
/// (по образцу `rule_weakened`).
pub(super) fn component_route_lock(
    repo: &Path,
    base: Option<&str>,
    git: &GitProbe,
    lock: &RouteLock,
) -> GateComponent {
    let rel = if repo.join(".arch-handoff/ROUTE.lock").is_file() {
        ".arch-handoff/ROUTE.lock"
    } else {
        "ROUTE.lock"
    };
    let decided = lock.decided_by.as_deref().unwrap_or("без ADR");
    let detail = format!(
        "заявленный маршрут: {} ({decided}) — файл: {rel}",
        lock.route
    );
    if !git.repo || !git.head {
        return GateComponent::pass("route_lock", detail);
    }
    let rev = base_rev(base.unwrap_or("HEAD"));
    let Some(git_rel) = git_rel_path(repo, &repo.join(rel)) else {
        return GateComponent::pass("route_lock", detail);
    };
    if !git_rev_exists(repo, rev) || !git_rev_has_path(repo, rev, &git_rel) {
        return GateComponent::pass("route_lock", detail);
    }
    let Ok(base_src) = git_show_file(repo, rev, &git_rel) else {
        return GateComponent::pass("route_lock", detail);
    };
    if let Some(base_lock) = parse_route_lock(&base_src) {
        if route_rank(base_lock.route) > route_rank(lock.route) {
            let has_adr = lock
                .decided_by
                .as_deref()
                .is_some_and(|d| d.trim().to_ascii_uppercase().starts_with("ADR"));
            if !has_adr {
                return GateComponent::fail(
                    "route_lock",
                    format!(
                        "route_lowered: маршрут понижен {} → {} без ADR — файл: {rel}",
                        base_lock.route, lock.route
                    ),
                    vec![GateFinding::text(
                        "error",
                        "понижение заявленного маршрута требует decided_by со ссылкой на ADR"
                            .to_string(),
                    )],
                );
            }
            return GateComponent::pass(
                "route_lock",
                format!(
                    "{detail}; понижение {} → {} подтверждено ADR",
                    base_lock.route, lock.route
                ),
            );
        }
    }
    GateComponent::pass("route_lock", detail)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gate::testkit::*;
    use crate::gate::{GateOutcome, GateStatus, render, run};

    #[test]
    fn gate_route_note_lists_diff_triggers() {
        let tmp = tempfile::tempdir().expect("tmp");
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).expect("mkdir");
        make_gate_repo(&repo);
        // Новый компонент в рабочем дереве (untracked): auto поднимает score.
        std::fs::create_dir_all(repo.join("services/risk/src")).expect("mkdir svc");
        std::fs::write(
            repo.join("services/risk/Cargo.toml"),
            "[package]\nname = \"risk\"\nversion = \"0.1.0\"\n",
        )
        .expect("manifest");
        let report = run(&repo, None, None, None, (1, 4)).expect("гейт");
        assert!(
            matches!(report.route, Route::Standard | Route::Critical),
            "маршрут из диффа: {}",
            report.route_note
        );
        assert!(
            report.route_note.contains("new_component"),
            "{}",
            report.route_note
        );
    }
    #[test]
    fn gate_route_note_is_clean_when_diff_base_unavailable() {
        let tmp = tempfile::tempdir().expect("tmp");
        let repo = tmp.path().join("repo");
        make_uncommitted_repo(&repo);
        let report = run(&repo, None, None, None, (1, 4)).expect("гейт");
        assert_eq!(report.route, Route::Critical, "fail-safe без диффа");
        // D9: сырой stderr git (многострочная справка «Используйте «--»…»)
        // в отчёт не протекает — только чистое однострочное сообщение.
        assert!(
            report.route_note.contains("база диффа недоступна"),
            "{}",
            report.route_note
        );
        assert!(
            !report.route_note.contains('\n'),
            "однострочная заметка: {}",
            report.route_note
        );
        for junk in ["Используйте", "Use '--'", "separate paths", "fatal:"] {
            assert!(
                !report.route_note.contains(junk),
                "в заметке маршрута сырой stderr git ({junk}): {}",
                report.route_note
            );
        }
        assert!(
            report.route_note.contains("fail-safe маршрут Critical"),
            "{}",
            report.route_note
        );
    }
    /// П4: `ROUTE.lock` поднимает маршрут на чистом дереве до заявленного,
    /// критические составляющие реально прогоняются.
    #[test]
    fn route_lock_raises_clean_tree_to_critical() {
        let tmp = tempfile::tempdir().expect("tmp");
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).expect("mkdir");
        make_gate_repo(&repo);
        std::fs::write(
            repo.join(".arch-handoff/ROUTE.lock"),
            "route: critical\nreason: \"обработка ЦР, 10/15 триггеров\"\ndecided_by: ADR-012\n",
        )
        .expect("route lock");
        let report = run(&repo, None, None, None, (1, 4)).expect("гейт");
        assert_eq!(report.route, Route::Critical, "{}", render(&report));
        assert!(
            report.components.iter().any(|c| c.name == "nfr"),
            "критические составляющие обязаны попасть в прогон"
        );
        assert!(
            report.route_note.contains("поднят ROUTE.lock"),
            "{}",
            report.route_note
        );
    }

    /// П4: понижение заявленного маршрута без ADR — находка `route_lowered`.
    #[test]
    fn route_lock_lowering_without_adr_fails() {
        let tmp = tempfile::tempdir().expect("tmp");
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).expect("mkdir");
        make_gate_repo(&repo);
        std::fs::write(
            repo.join(".arch-handoff/ROUTE.lock"),
            "route: critical\ndecided_by: ADR-012\n",
        )
        .expect("lock");
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-q", "-m", "route lock"]);
        // Понижаем до fast без ссылки на ADR.
        std::fs::write(repo.join(".arch-handoff/ROUTE.lock"), "route: fast\n").expect("lowered");
        let report = run(&repo, None, None, None, (1, 4)).expect("гейт");
        assert_eq!(
            status_of(&report, "route_lock"),
            GateStatus::Fail,
            "{}",
            render(&report)
        );
        assert_eq!(report.outcome, GateOutcome::Fail);
    }
}
