//! Реплей маршрута значимости по истории git (D4): маршрут и сработавшие
//! триггеры по каждому коммиту диапазона (`arch-be control score --replay`).
//!
//! Это инструмент ИЗМЕРЕНИЯ детектора: распределение маршрутов, частота
//! триггеров, доля обновлений зависимостей (замер для ADR-061) — по реальной
//! истории, а не по ощущениям. Информационная команда (exit 0), детерминизм
//! по тем же коммитам: повторный прогон даёт тот же отчёт.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;
use std::process::{Command, Stdio};

use serde::Serialize;

use super::diff_triggers::{DiffGlobs, detect_diff_triggers_with, significance_score_with_limits};
use super::types::Route;
use crate::error::{HarnessError, Result};

/// Потолок коммитов одного реплея: больше — сужайте диапазон (каждый коммит
/// — два вызова `git diff`, неограниченный диапазон раздувает прогон).
pub const MAX_REPLAY_COMMITS: usize = 5000;

/// Известный git «пустой» объект-дерево: база диффа для корневого коммита
/// (у которого нет родителя). Присутствует в любом объектном хранилище.
const EMPTY_TREE: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";

/// Одна строка реплея: маршрут и триггеры одного коммита.
#[derive(Debug, Clone, Serialize)]
pub struct ReplayEntry {
    /// Полный SHA коммита.
    pub commit: String,
    /// Первая строка сообщения коммита.
    pub subject: String,
    /// Маршрут по диффу `родитель..коммит`.
    pub route: Route,
    /// Счёт сработавших триггеров.
    pub score: usize,
    /// Сработавшие триггеры (канонические имена, по алфавиту).
    pub triggers: Vec<String>,
    /// Основания срабатываний («триггер: файл»).
    pub evidence: Vec<String>,
    /// Контекст D3: обновления версий зависимостей, классификация контрактов.
    pub notes: Vec<String>,
    /// Ошибка детектора на этом коммите (fail-soft: строка сохраняется,
    /// маршрут — fail-safe Critical, как у гейта без диффа).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Итог реплея: строки по коммитам + сводка распределений.
#[derive(Debug, Clone, Serialize)]
pub struct ReplayReport {
    /// Схема машинного вывода (`--json`).
    pub schema: String,
    /// Диапазон, как задан вызывающим.
    pub range: String,
    /// Коммитов в реплее.
    pub total: usize,
    /// Строки по коммитам в хронологическом порядке (старые первые).
    pub commits: Vec<ReplayEntry>,
    /// Распределение маршрутов («Fast»/«Standard»/«Critical» → число).
    pub routes: BTreeMap<String, usize>,
    /// Частота триггеров (триггер → число коммитов, где сработал).
    pub trigger_frequency: BTreeMap<String, usize>,
    /// Заметок «обновление зависимости …» (D3) — смен версии, которые до
    /// ADR-061 зажигали `new_vendor`: числитель замера доли шума.
    pub vendor_updates: usize,
    /// Коммитов со срабатыванием `new_vendor` (истинно новые имена пакетов).
    pub new_vendor_fires: usize,
    /// Коммитов с ломающей классификацией контракта (`contract_diff`).
    pub contract_breaking: usize,
    /// Коммитов с ошибкой детектора (fail-soft строки).
    pub detector_errors: usize,
}

/// Коммит истории для реплея: SHA + первая строка сообщения.
struct CommitRow {
    /// Полный SHA.
    sha: String,
    /// Subject.
    subject: String,
}

/// `git log --reverse --format=%H%x00%s <range>` — хронологический список.
fn list_commits(repo: &Path, range: &str) -> Result<Vec<CommitRow>> {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["log", "--reverse", "--format=%H%x00%s", range])
        .stdin(Stdio::null())
        .output()
        .map_err(|e| {
            HarnessError::Control(format!("score --replay: не удалось запустить git ({e})"))
        })?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stdout);
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(HarnessError::Control(format!(
            "score --replay: некорректный диапазон '{range}' — {} {}",
            stderr.trim(),
            err.trim()
        )));
    }
    let mut rows = Vec::new();
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let Some((sha, subject)) = line.split_once('\0') else {
            continue;
        };
        rows.push(CommitRow {
            sha: sha.to_string(),
            subject: subject.to_string(),
        });
    }
    Ok(rows)
}

/// Родитель коммита; у корневого — известное пустое дерево.
fn parent_of(repo: &Path, sha: &str) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["rev-parse", "--verify", "--quiet", &format!("{sha}^")])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output();
    match out {
        Ok(o) if o.status.success() => {
            let parent = String::from_utf8_lossy(&o.stdout).trim().to_string();
            if parent.is_empty() {
                EMPTY_TREE.to_string()
            } else {
                parent
            }
        }
        _ => EMPTY_TREE.to_string(),
    }
}

/// Реплей значимости по диапазону: для каждого коммита — детектор диффа
/// `родитель..коммит` и маршрут по найденным триггерам (declared пусты:
/// реплей меряет именно механический слой).
///
/// # Errors
/// Не git-репозиторий, некорректный диапазон, диапазон пуст или длиннее
/// [`MAX_REPLAY_COMMITS`].
pub fn replay_significance(
    repo: &Path,
    range: &str,
    globs: &DiffGlobs,
    limits: (usize, usize),
) -> Result<ReplayReport> {
    let rows = list_commits(repo, range)?;
    if rows.is_empty() {
        return Err(HarnessError::Control(format!(
            "score --replay: диапазон '{range}' пуст — нечего воспроизводить"
        )));
    }
    if rows.len() > MAX_REPLAY_COMMITS {
        return Err(HarnessError::Control(format!(
            "score --replay: в диапазоне '{range}' {} коммитов, потолок {MAX_REPLAY_COMMITS} — \
             сузьте диапазон (например, '<тег>..HEAD')",
            rows.len()
        )));
    }
    let mut commits = Vec::with_capacity(rows.len());
    for row in &rows {
        let base = parent_of(repo, &row.sha);
        let diff_range = format!("{base}..{}", row.sha);
        match detect_diff_triggers_with(repo, Some(&diff_range), globs) {
            Ok(diff) => {
                let sig = significance_score_with_limits(
                    &diff
                        .triggers
                        .iter()
                        .map(|t| (t.clone(), true))
                        .collect::<BTreeMap<_, _>>(),
                    limits.0,
                    limits.1,
                );
                commits.push(ReplayEntry {
                    commit: row.sha.clone(),
                    subject: row.subject.clone(),
                    route: sig.route,
                    score: sig.score,
                    triggers: sig.fired,
                    evidence: diff.evidence,
                    notes: diff.notes,
                    error: None,
                });
            }
            Err(e) => commits.push(ReplayEntry {
                commit: row.sha.clone(),
                subject: row.subject.clone(),
                route: Route::Critical,
                score: 0,
                triggers: Vec::new(),
                evidence: Vec::new(),
                notes: Vec::new(),
                error: Some(e.to_string()),
            }),
        }
    }
    Ok(summarize(range, commits))
}

/// Сводка реплея: распределения маршрутов и триггеров, счётчики заметок D3.
fn summarize(range: &str, commits: Vec<ReplayEntry>) -> ReplayReport {
    let mut routes: BTreeMap<String, usize> = BTreeMap::new();
    let mut trigger_frequency: BTreeMap<String, usize> = BTreeMap::new();
    let mut vendor_updates = 0usize;
    let mut new_vendor_fires = 0usize;
    let mut contract_breaking = 0usize;
    let mut detector_errors = 0usize;
    for c in &commits {
        *routes.entry(c.route.to_string()).or_default() += 1;
        for t in &c.triggers {
            *trigger_frequency.entry(t.clone()).or_default() += 1;
        }
        if c.triggers.iter().any(|t| t == "new_vendor") {
            new_vendor_fires += 1;
        }
        vendor_updates += c
            .notes
            .iter()
            .filter(|n| n.starts_with("обновление зависимости "))
            .count();
        if c.notes
            .iter()
            .any(|n| n.starts_with("contract_diff:") && n.contains("ломающее"))
        {
            contract_breaking += 1;
        }
        if c.error.is_some() {
            detector_errors += 1;
        }
    }
    ReplayReport {
        schema: "arch-be/significance-replay/v1".to_string(),
        range: range.to_string(),
        total: commits.len(),
        commits,
        routes,
        trigger_frequency,
        vendor_updates,
        new_vendor_fires,
        contract_breaking,
        detector_errors,
    }
}

/// Текстовый рендер реплея: строка на коммит + сводка. Детерминирован по
/// входу (хронологический порядок, сортированные карты).
#[must_use]
pub fn render_replay(report: &ReplayReport) -> String {
    let mut out = String::new();
    // Запись в String не может завершиться ошибкой — игноры безопасны.
    let _ = writeln!(
        out,
        "Реплей значимости: {} — {} коммитов",
        report.range, report.total
    );
    for c in &report.commits {
        let short: String = c.commit.chars().take(9).collect();
        let triggers = if c.triggers.is_empty() {
            "—".to_string()
        } else {
            c.triggers.join(", ")
        };
        let subject: String = c.subject.chars().take(70).collect();
        let _ = writeln!(
            out,
            "{short} {:<9} score {:<2} {triggers}   {subject}",
            c.route.to_string(),
            c.score
        );
        if let Some(e) = &c.error {
            let reason: String = e
                .lines()
                .next()
                .unwrap_or_default()
                .chars()
                .take(100)
                .collect();
            let _ = writeln!(out, "    ошибка детектора: {reason}");
        }
    }
    let _ = writeln!(out);
    let _ = writeln!(out, "Маршруты:");
    for (route, n) in &report.routes {
        let _ = writeln!(out, "  {route}: {n}");
    }
    let _ = writeln!(out, "Триггеры (коммитов со срабатыванием):");
    for (trigger, n) in &report.trigger_frequency {
        let _ = writeln!(out, "  {trigger}: {n}");
    }
    let _ = writeln!(
        out,
        "Заметки D3: обновлений зависимостей {}, срабатываний new_vendor {}, \
         коммитов с ломающим контрактом {}",
        report.vendor_updates, report.new_vendor_fires, report.contract_breaking
    );
    if report.detector_errors > 0 {
        let _ = writeln!(out, "Ошибок детектора: {}", report.detector_errors);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// Пишет файл в каталог и возвращает его путь.
    fn write_file(dir: &Path, name: &str, content: &str) -> PathBuf {
        let p = dir.join(name);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&p, content).unwrap();
        p
    }

    /// git в каталоге с тестовой идентичностью коммиттера (изоляция AD-7).
    fn git_in(dir: &Path, args: &[&str]) {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .output()
            .expect("git");
        assert!(
            out.status.success(),
            "git {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// Репозиторий из трёх коммитов: база (readme), новый компонент
    /// (манифест+src), bump версии существующей зависимости.
    fn repo_with_history(dir: &Path) -> PathBuf {
        let repo = dir.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        git_in(&repo, &["init", "-q", "-b", "main"]);
        write_file(&repo, "README.md", "base\n");
        write_file(
            &repo,
            "Cargo.toml",
            "[package]\nname = \"x\"\n\n[dependencies]\nserde = \"1.0\"\n",
        );
        git_in(&repo, &["add", "."]);
        git_in(&repo, &["commit", "-q", "-m", "база"]);

        write_file(
            &repo,
            "billing/Cargo.toml",
            "[package]\nname = \"billing\"\n\n[dependencies]\ntokio = \"1\"\n",
        );
        write_file(&repo, "billing/src/main.rs", "fn main() {}\n");
        git_in(&repo, &["add", "."]);
        git_in(&repo, &["commit", "-q", "-m", "новый компонент billing"]);

        write_file(
            &repo,
            "Cargo.toml",
            "[package]\nname = \"x\"\n\n[dependencies]\nserde = \"1.1\"\n",
        );
        git_in(&repo, &["add", "."]);
        git_in(&repo, &["commit", "-q", "-m", "bump serde 1.1"]);
        repo
    }

    /// Реплей по диапазону всей истории: корневой коммит (от пустого дерева),
    /// компонентный коммит и bump — каждый со своим маршрутом; bump не
    /// зажигает `new_vendor`, а оставляет заметку (D3, ADR-061).
    #[test]
    fn replay_walks_each_commit_with_its_own_diff() {
        let dir = tempfile::tempdir().unwrap();
        let repo = repo_with_history(dir.path());
        let report = replay_significance(&repo, "HEAD~2..HEAD", &DiffGlobs::default(), (1, 4))
            .expect("реплей");
        assert_eq!(report.total, 2, "{report:?}");

        let component = &report.commits[0];
        assert_eq!(component.subject, "новый компонент billing");
        assert!(
            component.triggers.contains(&"new_component".to_string()),
            "{component:?}"
        );
        // Манифест billing новый → его зависимости — новые имена: tokio
        // (строка `name = "billing"` эвристикой тоже выглядит зависимостью —
        // fail-safe over-match, прежнее поведение детектора).
        assert!(
            component.triggers.contains(&"new_vendor".to_string()),
            "{component:?}"
        );
        assert!(component.error.is_none(), "{component:?}");

        let bump = &report.commits[1];
        assert_eq!(bump.subject, "bump serde 1.1");
        assert!(
            !bump.triggers.contains(&"new_vendor".to_string()),
            "bump версии — не новый вендор: {bump:?}"
        );
        assert_eq!(bump.route, Route::Fast, "{bump:?}");
        assert!(
            bump.notes
                .iter()
                .any(|n| n.contains("обновление зависимости serde")),
            "{:?}",
            bump.notes
        );
        assert_eq!(report.vendor_updates, 1);
        assert_eq!(report.new_vendor_fires, 1);
        // Сводка: оба коммита разошлись по маршрутам.
        assert_eq!(
            report.routes.get("Standard"),
            Some(&1),
            "{:?}",
            report.routes
        );
        assert_eq!(report.routes.get("Fast"), Some(&1), "{:?}", report.routes);
    }

    /// Корневой коммит реплея диффится от пустого дерева: вся стартовая
    /// структура видна как добавленная.
    #[test]
    fn replay_root_commit_diffs_against_empty_tree() {
        let dir = tempfile::tempdir().unwrap();
        let repo = repo_with_history(dir.path());
        // Вся история: первый коммит — корневой.
        let root = {
            let out = Command::new("git")
                .arg("-C")
                .arg(&repo)
                .args(["rev-list", "--max-parents=0", "HEAD"])
                .output()
                .expect("git");
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        };
        let report =
            replay_significance(&repo, &root, &DiffGlobs::default(), (1, 4)).expect("реплей корня");
        assert_eq!(report.total, 1);
        let first = &report.commits[0];
        assert_eq!(first.subject, "база");
        assert!(
            first.triggers.contains(&"new_vendor".to_string()),
            "serde в корневом манифесте — новое имя: {first:?}"
        );
        assert!(first.error.is_none(), "{first:?}");
    }

    /// Пустой и неизвестный диапазоны — понятная ошибка, а не пустой отчёт.
    #[test]
    fn replay_rejects_empty_and_unknown_ranges() {
        let dir = tempfile::tempdir().unwrap();
        let repo = repo_with_history(dir.path());
        let err = replay_significance(&repo, "HEAD..HEAD", &DiffGlobs::default(), (1, 4))
            .expect_err("пустой диапазон");
        assert!(err.to_string().contains("пуст"), "{err}");
        let err = replay_significance(&repo, "нет-такого..HEAD", &DiffGlobs::default(), (1, 4))
            .expect_err("неизвестный диапазон");
        assert!(err.to_string().contains("нет-такого"), "{err}");
    }

    /// Детерминизм: повторный прогон по тому же диапазону даёт тот же JSON.
    #[test]
    fn replay_is_deterministic() {
        let dir = tempfile::tempdir().unwrap();
        let repo = repo_with_history(dir.path());
        let a =
            replay_significance(&repo, "HEAD~2..HEAD", &DiffGlobs::default(), (1, 4)).expect("a");
        let b =
            replay_significance(&repo, "HEAD~2..HEAD", &DiffGlobs::default(), (1, 4)).expect("b");
        let ja = serde_json::to_string(&a).expect("json a");
        let jb = serde_json::to_string(&b).expect("json b");
        assert_eq!(ja, jb, "повторный прогон разошёлся");
        assert!(ja.contains("arch-be/significance-replay/v1"), "{ja}");
        // Текстовый рендер: строка на коммит + сводка.
        let text = render_replay(&a);
        assert!(
            text.contains("Реплей значимости: HEAD~2..HEAD — 2 коммитов"),
            "{text}"
        );
        assert!(text.contains("bump serde 1.1"), "{text}");
        assert!(text.contains("Маршруты:"), "{text}");
        assert!(text.contains("Standard: 1"), "{text}");
    }
}
