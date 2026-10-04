//! Теневой гейт (волна D, D2; ADR-059): второй прогон того же конвейера
//! вердикта по файлу-кандидату реестра правил (`--shadow-constraints`).
//!
//! Решающий вердикт НЕ меняется: exit-код, схема `gate-verdict/v1` и состав
//! обязательных составляющих определяет основной прогон (правило 4 TASK).
//! Теневой прогон переиспользует существующий конвейер
//! ([`super::verdict::run_opts`]) с другим `CONSTRAINTS`-источником и даёт
//! отдельный блок — какие находки составляющей `fitness` появятся/исчезнут
//! и какие правила сменят severity (по id правила), плюс сводку «что
//! покраснеет при переходе».
//!
//! Теневой файл с ошибкой разбора не трогает основной вердикт: находка
//! `shadow_constraints_invalid` живёт в shadow-блоке.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use super::types::GateOptions;
use crate::control;
use crate::error::Result;

/// Схема сохранённого отчёта теневого прогона (для `control report`).
pub use crate::control::SHADOW_RECORD_SCHEMA;

/// Находка одного правила в теневом диффе.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShadowFinding {
    /// Id правила.
    pub rule: String,
    /// Худшая severity (`error`|`warn`).
    pub severity: String,
    /// Число находок правила.
    pub findings: usize,
}

/// Смена severity правила (по id правила).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShadowSeverityChange {
    /// Id правила.
    pub rule: String,
    /// Было (текущий реестр).
    pub from: String,
    /// Стало (реестр-кандидат).
    pub to: String,
}

/// Результат теневого прогона: дифф «что покраснеет при переходе».
#[derive(Debug, Clone)]
pub struct ShadowReport {
    /// Файл-кандидат реестра (метка для вывода).
    pub shadow_constraints: String,
    /// Ошибка разбора файла-кандидата (`shadow_constraints_invalid`) —
    /// теневой дифф не построен, основной вердикт не тронут.
    pub invalid: Option<String>,
    /// Появляющиеся находки (правила, которых нет в текущем вердикте).
    pub new_findings: Vec<ShadowFinding>,
    /// Исчезающие находки (правила, которых нет в теневом вердикте).
    pub disappeared: Vec<ShadowFinding>,
    /// Смены severity по id правила.
    pub severity_changes: Vec<ShadowSeverityChange>,
    /// Сводка «что покраснеет при переходе» одной строкой.
    pub summary: String,
}

impl ShadowReport {
    /// Отчёт «файл не разобран».
    fn invalid(label: String, message: String) -> Self {
        Self {
            shadow_constraints: label,
            invalid: Some(message),
            new_findings: Vec::new(),
            disappeared: Vec::new(),
            severity_changes: Vec::new(),
            summary: "теневой реестр не разобран — переход не оценить".to_string(),
        }
    }

    /// Запись отчёта прогона для флотового среза (D2, `control report`).
    #[must_use]
    pub fn to_record(&self) -> control::ShadowRecord {
        let mut new_findings = BTreeMap::new();
        for f in &self.new_findings {
            new_findings.insert(f.rule.clone(), f.severity.clone());
        }
        let mut severity_changes = BTreeMap::new();
        for c in &self.severity_changes {
            severity_changes.insert(
                c.rule.clone(),
                control::ShadowChange {
                    from: c.from.clone(),
                    to: c.to.clone(),
                },
            );
        }
        control::ShadowRecord {
            schema: SHADOW_RECORD_SCHEMA.to_string(),
            shadow_constraints: self.shadow_constraints.clone(),
            new_findings,
            severity_changes,
            summary: self.summary.clone(),
        }
    }
}

/// Худшая severity правила (error > warn) и число находок — снимок находок
/// реестра правил ([`control::check_with_options`]).
///
/// Дифф строится по находкам ПРОВЕРКИ реестра, а не по составляющей `fitness`
/// вердикта: зелёная `fitness` отдаёт PASS без warn-находок (они не красят
/// гейт), и смена `warn → error` в вердикте была бы невидима. Реестр влияет
/// на код именно через эту проверку — её находки и есть «что покраснеет».
fn rule_severities(issues: &[control::LintIssue]) -> BTreeMap<String, (String, usize)> {
    let mut out: BTreeMap<String, (String, usize)> = BTreeMap::new();
    for i in issues {
        let entry = out
            .entry(i.rule.clone())
            .or_insert_with(|| (i.severity.clone(), 0));
        entry.1 += 1;
        if i.severity == "error" {
            entry.0 = "error".to_string();
        }
    }
    out
}

/// Собирает теневой дифф по находкам двух реестров (текущего и кандидата):
/// появление/исчезновение находок и смена severity по id правила.
#[must_use]
pub fn diff(
    main_issues: &[control::LintIssue],
    shadow_issues: &[control::LintIssue],
    label: &str,
) -> ShadowReport {
    let main_rules = rule_severities(main_issues);
    let shadow_rules = rule_severities(shadow_issues);
    let mut new_findings = Vec::new();
    for (rule, (severity, findings)) in &shadow_rules {
        if !main_rules.contains_key(rule) {
            new_findings.push(ShadowFinding {
                rule: rule.clone(),
                severity: severity.clone(),
                findings: *findings,
            });
        }
    }
    let mut disappeared = Vec::new();
    for (rule, (severity, findings)) in &main_rules {
        if !shadow_rules.contains_key(rule) {
            disappeared.push(ShadowFinding {
                rule: rule.clone(),
                severity: severity.clone(),
                findings: *findings,
            });
        }
    }
    let mut severity_changes = Vec::new();
    for (rule, (severity, _)) in &shadow_rules {
        if let Some((was, _)) = main_rules.get(rule) {
            if was != severity {
                severity_changes.push(ShadowSeverityChange {
                    rule: rule.clone(),
                    from: was.clone(),
                    to: severity.clone(),
                });
            }
        }
    }
    let will_redden = new_findings
        .iter()
        .filter(|f| f.severity == "error")
        .count()
        + severity_changes
            .iter()
            .filter(|c| c.to == "error" && c.from != "error")
            .count();
    let summary =
        if new_findings.is_empty() && disappeared.is_empty() && severity_changes.is_empty() {
            "переход ничего не покраснит: находки совпадают".to_string()
        } else {
            format!(
                "при переходе: новых правил {} (error {}), исчезнет {}, смена severity {}; \
             станет красным: {}",
                new_findings.len(),
                new_findings
                    .iter()
                    .filter(|f| f.severity == "error")
                    .count(),
                disappeared.len(),
                severity_changes.len(),
                will_redden
            )
        };
    ShadowReport {
        shadow_constraints: label.to_string(),
        invalid: None,
        new_findings,
        disappeared,
        severity_changes,
        summary,
    }
}

/// Метка файла-кандидата для вывода: относительный путь от корня, иначе —
/// как передан.
fn label(repo: &Path, path: &Path) -> String {
    path.strip_prefix(repo)
        .ok()
        .filter(|p| !p.as_os_str().is_empty())
        .map_or_else(|| path.display().to_string(), |p| p.display().to_string())
}

/// Резолвит путь файла-кандидата: абсолютный — как есть; относительный —
/// от корня репозитория, если такой файл там есть, иначе от текущего каталога.
fn resolve(repo: &Path, shadow_path: &Path) -> PathBuf {
    if shadow_path.is_absolute() {
        return shadow_path.to_path_buf();
    }
    let from_repo = repo.join(shadow_path);
    if from_repo.is_file() {
        from_repo
    } else {
        shadow_path.to_path_buf()
    }
}

/// Путь основного реестра: явный `--constraints` либо тот же резолвер, что у
/// гейта (пакетная копия → корневой fallback).
fn main_constraints(repo: &Path, constraints: Option<&Path>) -> PathBuf {
    if let Some(path) = constraints {
        return path.to_path_buf();
    }
    control::resolve_constraints_path_detailed(repo, None)
        .map_or_else(|| repo.join(control::HANDOFF_CONSTRAINTS_PATH), |r| r.path)
}

/// Вычисляет теневой дифф: проверяет файл-кандидат, при успехе прогоняет
/// проверку реестра ПО нему и сравнивает находки с текущим реестром. Основной
/// вердикт не трогается: ошибка разбора кандидата — `invalid` в отчёте.
///
/// # Errors
/// Репозиторий недоступен (ошибки разбора файла-кандидата — не ошибка, а
/// `invalid` в отчёте).
pub fn evaluate(
    repo: &Path,
    constraints: Option<&Path>,
    options: &GateOptions,
    shadow_path: &Path,
) -> Result<ShadowReport> {
    let path = resolve(repo, shadow_path);
    let label = label(repo, &path);
    // Ошибка разбора файла-кандидата не должна красить основной вердикт:
    // сообщаем находкой shadow_constraints_invalid, дифа нет.
    if let Err(e) = control::load_constraints_resolved(&path) {
        return Ok(ShadowReport::invalid(
            label,
            format!("shadow_constraints_invalid: {e}"),
        ));
    }
    // Те же политика исполнения и настройки override, что у составляющей
    // `fitness` основного гейта: сравнение идёт по сопоставимым прогонам.
    let check_options = control::baseline::CheckOptions {
        exec: options.exec.clone(),
        overrides: control::baseline::OverrideSettings {
            adr_dir: options.overrides.adr_dir.clone(),
            max_horizon_months: options.overrides.max_horizon_months,
        },
        ..control::baseline::CheckOptions::default()
    };
    let main_path = main_constraints(repo, constraints);
    let main_issues = control::check_with_options(repo, &main_path, &check_options)
        .map(|r| r.issues)
        .unwrap_or_default();
    let shadow_issues = control::check_with_options(repo, &path, &check_options)
        .map(|r| r.issues)
        .unwrap_or_default();
    Ok(diff(&main_issues, &shadow_issues, &label))
}

/// Текстовый блок теневого гейта (аддитивная печать ПОСЛЕ основного отчёта;
/// основной вердикт и его вывод не меняются).
#[must_use]
pub fn render(report: &ShadowReport) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    let _ = writeln!(out, "\nТеневой гейт ({}):", report.shadow_constraints);
    if let Some(message) = &report.invalid {
        let _ = writeln!(out, "  [error] {message}");
        let _ = writeln!(out, "  основной вердикт не изменён");
        return out;
    }
    if !report.new_findings.is_empty() {
        let _ = writeln!(out, "  Новые находки ({}):", report.new_findings.len());
        for f in &report.new_findings {
            let _ = writeln!(
                out,
                "    + {} [{}] — находок {}",
                f.rule, f.severity, f.findings
            );
        }
    }
    if !report.disappeared.is_empty() {
        let _ = writeln!(out, "  Исчезнувшие находки ({}):", report.disappeared.len());
        for f in &report.disappeared {
            let _ = writeln!(out, "    - {} [{}]", f.rule, f.severity);
        }
    }
    if !report.severity_changes.is_empty() {
        let _ = writeln!(out, "  Смена severity ({}):", report.severity_changes.len());
        for c in &report.severity_changes {
            let _ = writeln!(out, "    ~ {} {} → {}", c.rule, c.from, c.to);
        }
    }
    let _ = writeln!(out, "  Сводка: {}", report.summary);
    let _ = writeln!(out, "  основной вердикт не изменён");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gate::testkit::*;
    use crate::gate::{GateOptions, GateRequirements, GateStatus, run_opts};

    /// Репозиторий D2: правило X-1 (`must_not_contain` по PAN, warn) уже
    /// срабатывает, файл содержит и вторую метку (SECRET) для X-2.
    fn make_shadow_repo(dir: &Path) -> (std::path::PathBuf, std::path::PathBuf) {
        std::fs::create_dir_all(dir.join(".arch-handoff")).expect("mkdir handoff");
        std::fs::create_dir_all(dir.join("src")).expect("mkdir src");
        std::fs::write(dir.join("src/a.py"), "PAN=4111\nSECRET=42\n").expect("py");
        std::fs::write(dir.join("ARCHITECTURE-SPINE.md"), "# Spine\n").expect("spine");
        std::fs::write(
            dir.join(".arch-handoff/CONSTRAINTS.yaml"),
            "rules:\n\
             \x20 - name: X-1\n\
             \x20   type: must_not_contain\n\
             \x20   glob: \"**/*.py\"\n\
             \x20   pattern: 'PAN'\n\
             \x20   severity: warn\n",
        )
        .expect("main constraints");
        git(dir, &["init", "-q"]);
        git(dir, &["add", "."]);
        git(dir, &["commit", "-q", "-m", "init"]);
        let shadow = dir.join("shadow-constraints.yaml");
        std::fs::write(
            &shadow,
            "rules:\n\
             \x20 - name: X-1\n\
             \x20   type: must_not_contain\n\
             \x20   glob: \"**/*.py\"\n\
             \x20   pattern: 'PAN'\n\
             \x20   severity: error\n\
             \x20 - name: X-2\n\
             \x20   type: must_not_contain\n\
             \x20   glob: \"**/*.py\"\n\
             \x20   pattern: 'SECRET'\n\
             \x20   severity: error\n",
        )
        .expect("shadow constraints");
        (dir.join(".arch-handoff/CONSTRAINTS.yaml"), shadow)
    }

    /// Приёмка D2: теневой реестр добавляет X-2 и ужесточает X-1 до error —
    /// основной вердикт и exit-код НЕ изменились, shadow-блок показывает
    /// X-2 (new) и смену severity X-1 warn → error.
    #[test]
    fn shadow_diff_reports_new_rule_and_severity_change_without_touching_main() {
        let tmp = tempfile::tempdir().expect("tmp");
        let repo = tmp.path().join("repo");
        let (constraints, shadow_file) = make_shadow_repo(&repo);
        let main = run_opts(
            &repo,
            None,
            None,
            Some(&constraints),
            (1, 4),
            &GateRequirements::default(),
            &GateOptions::default(),
        )
        .expect("main gate");
        // Основной реестр: X-1 только warn → решающий вердикт зелёный.
        assert!(main.passed, "основной вердикт зелёный");
        assert_eq!(status_of(&main, "fitness"), GateStatus::Pass);
        let main_attestation = main.attestation.clone();

        let shadow = evaluate(
            &repo,
            Some(&constraints),
            &GateOptions::default(),
            &shadow_file,
        )
        .expect("shadow gate");
        assert!(shadow.invalid.is_none(), "{:?}", shadow.invalid);
        assert!(
            shadow
                .new_findings
                .iter()
                .any(|f| f.rule == "X-2" && f.severity == "error"),
            "X-2 — новая: {:?}",
            shadow.new_findings
        );
        assert!(
            shadow
                .severity_changes
                .iter()
                .any(|c| c.rule == "X-1" && c.from == "warn" && c.to == "error"),
            "X-1: warn → error: {:?}",
            shadow.severity_changes
        );
        // Ключевая инварианта приёмки: основной вердикт не тронут.
        assert!(main.passed);
        assert_eq!(main.attestation, main_attestation);
        // Блок shadow печатается отдельно и не меняет решающий вывод.
        let text = render(&shadow);
        assert!(text.contains("X-2"), "{text}");
        assert!(text.contains("X-1 warn → error"), "{text}");
        assert!(text.contains("основной вердикт не изменён"), "{text}");
    }

    /// Теневой файл с ошибкой YAML → `shadow_constraints_invalid` в shadow-блоке,
    /// основной вердикт зелёный.
    #[test]
    fn shadow_invalid_file_does_not_touch_main_verdict() {
        let tmp = tempfile::tempdir().expect("tmp");
        let repo = tmp.path().join("repo");
        let (constraints, _) = make_shadow_repo(&repo);
        let broken = repo.join("broken-shadow.yaml");
        std::fs::write(&broken, "rules: [\n  - name: broken\n").expect("broken yaml");
        let main = run_opts(
            &repo,
            None,
            None,
            Some(&constraints),
            (1, 4),
            &GateRequirements::default(),
            &GateOptions::default(),
        )
        .expect("main gate");
        assert!(main.passed);
        let shadow = evaluate(&repo, Some(&constraints), &GateOptions::default(), &broken)
            .expect("shadow gate");
        let message = shadow.invalid.as_ref().expect("shadow_constraints_invalid");
        assert!(message.contains("shadow_constraints_invalid"), "{message}");
        assert!(main.passed, "основной вердикт не изменён");
        assert!(render(&shadow).contains("основной вердикт не изменён"));
    }

    /// Сводка «ничего не покраснеет» — когда реестры дают те же находки.
    #[test]
    fn shadow_identical_registry_summarizes_no_change() {
        let tmp = tempfile::tempdir().expect("tmp");
        let repo = tmp.path().join("repo");
        let (constraints, _) = make_shadow_repo(&repo);
        let _ = run_opts(
            &repo,
            None,
            None,
            Some(&constraints),
            (1, 4),
            &GateRequirements::default(),
            &GateOptions::default(),
        )
        .expect("main gate");
        let shadow = evaluate(
            &repo,
            Some(&constraints),
            &GateOptions::default(),
            &constraints,
        )
        .expect("shadow gate");
        assert!(shadow.new_findings.is_empty(), "{:?}", shadow.new_findings);
        assert!(
            shadow.summary.contains("ничего не покраснит"),
            "{}",
            shadow.summary
        );
    }
}
