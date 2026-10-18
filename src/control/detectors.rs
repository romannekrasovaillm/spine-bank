//! Экспорт исходов fitness-правил как детекторов досье (ADR-093, волна 1).
//!
//! Слот «детерминированное свидетельство → судья» в досье был с E7.1
//! (`reports/detectors/*.json`, роль `detector`), но его никто не наполнял:
//! компонент `fitness` своих исходов туда не писал, и судья видел только то,
//! что проект положил руками. Этот модуль превращает прогон реестра в набор
//! детекторов — по файлу на правило — в `reports/detectors/rules/`.
//!
//! Подкаталог, а не корень `reports/detectors/`: проектные детекторы корня
//! входят в досье `code_vs_spine` целиком (поведение E7.1), а детекторы
//! правил отбираются в досье по относимости к субъекту — иначе каждое досье
//! раздувалось бы всем реестром и устаревало бы при смене исхода любого
//! правила.
//!
//! Файл детерминирован: ни меток времени, ни длительностей, ни HEAD. Хэш
//! детектора входит в `pack_sha256`, и повторный экспорт того же исхода не
//! обязан обесценивать отчёт судьи — обесценивает только смена измерения.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::types::{FitnessReport, FitnessRule};
use crate::error::{HarnessError, Result};

/// Каталог детекторов правил внутри репозитория.
pub const RULE_DETECTORS_DIR: &str = "reports/detectors/rules";

/// Схема файла детектора правила.
pub const RULE_DETECTOR_SCHEMA: &str = "arch-be/rule-detector/v1";

/// Сколько находок правила попадает в детектор: досье — ориентир судьи, а не
/// полный журнал находок (полный — в выводе `control check`).
pub const MAX_DETECTOR_FINDINGS: usize = 10;

/// Потолок длины сообщения находки в детекторе (символов): лимит досье общий
/// для всех источников, длинное сообщение одного правила не должно вытеснять
/// субъект.
pub const MAX_DETECTOR_MESSAGE_CHARS: usize = 160;

/// Исход правила в прогоне реестра.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleOutcome {
    /// Правило прогнано, находок нет.
    Pass,
    /// Есть находки уровня error.
    Fail,
    /// Только warn-находки: правило сработало, но гейт не краснит.
    Warn,
    /// Правило не прогонялось: нет ресурса, прогонщика или доверия к команде,
    /// либо файлы правила вне среза `--changed-since`.
    Skip,
    /// Объявленные условия применимости (`applies_to`) не выполнены.
    Na,
    /// Правило выключено действующим override с ADR.
    Overridden,
}

impl RuleOutcome {
    /// Строковое имя — как в поле `status` детектора.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pass => "pass",
            Self::Fail => "fail",
            Self::Warn => "warn",
            Self::Skip => "skip",
            Self::Na => "na",
            Self::Overridden => "overridden",
        }
    }

    /// Разбирает имя исхода; незнакомое — `None` (детектор чужой редакции).
    #[must_use]
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "pass" => Some(Self::Pass),
            "fail" => Some(Self::Fail),
            "warn" => Some(Self::Warn),
            "skip" => Some(Self::Skip),
            "na" => Some(Self::Na),
            "overridden" => Some(Self::Overridden),
            _ => None,
        }
    }

    /// Прогнано ли правило по-настоящему: только тогда его исход —
    /// измерение, на которое может опереться связка с рубрикой.
    #[must_use]
    pub fn measured(self) -> bool {
        matches!(self, Self::Pass | Self::Fail | Self::Warn)
    }
}

/// Находка правила в детекторе (сжатая).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DetectorFinding {
    /// Файл относительно репозитория.
    pub file: String,
    /// Строка (0 — файл целиком).
    pub line: usize,
    /// Сообщение (усечённое до [`MAX_DETECTOR_MESSAGE_CHARS`]).
    pub message: String,
}

/// Детектор одного правила — `reports/detectors/rules/<id>.json`.
///
/// Поля `name` и `status` — общий контракт детектора E7.1: по ним досье
/// называет источник и сверяет вердикт судьи с измерением (E7.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuleDetector {
    /// Схема файла.
    pub schema: String,
    /// Имя детектора: идентификатор правила (`C-05`), без него — имя правила.
    pub name: String,
    /// Имя правила (код находки).
    pub rule: String,
    /// Исход: `pass` / `fail` / `warn` / `skip` / `na` / `overridden`.
    pub status: String,
    /// Серьёзность правила после нормализации (`error` / `warn`).
    pub severity: String,
    /// Тип проверки (`must_not_contain`, `command_succeeds`, …).
    pub kind: String,
    /// Заявленный инвариант (`AD-n`) из карточки правила.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ad: Option<String>,
    /// Почему правило не прогонялось (для `skip` / `na` / `overridden`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Статус измерения зубьев (`confirmed`, `toothless`, …) — только если
    /// отпечаток правила совпал с замером `rules teeth`; правка тела правила
    /// делает замер неприменимым, и поле отсутствует («не измерено»).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub teeth: Option<String>,
    /// Всего находок правила.
    #[serde(default)]
    pub findings_total: usize,
    /// Файлы с находками — по ним детектор относится к субъекту досье.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<String>,
    /// Первые [`MAX_DETECTOR_FINDINGS`] находок в стабильном порядке.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub findings: Vec<DetectorFinding>,
}

impl RuleDetector {
    /// Исход как перечисление; незнакомая строка — `None`.
    #[must_use]
    pub fn outcome(&self) -> Option<RuleOutcome> {
        RuleOutcome::parse(&self.status)
    }

    /// Называется ли правило этим ключом: идентификатором или именем.
    #[must_use]
    pub fn answers_to(&self, key: &str) -> bool {
        let key = key.trim();
        !key.is_empty() && (self.name == key || self.rule == key)
    }

    /// Каноническое тело файла: однострочный JSON + перевод строки. Порядок
    /// полей — порядок структуры, поэтому текст детерминирован.
    ///
    /// # Errors
    /// Сериализация не удалась (недостижимо для этой структуры).
    pub fn to_text(&self) -> Result<String> {
        serde_json::to_string(self)
            .map(|s| s + "\n")
            .map_err(|e| HarnessError::Rubric(format!("сериализация детектора: {e}")))
    }
}

/// Имя файла детектора: идентификатор, обезвреженный до имени файла.
fn file_stem_of(name: &str) -> String {
    let stem: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let stem = stem.trim_matches(['-', '.']).to_string();
    if stem.is_empty() {
        "rule".to_string()
    } else {
        stem
    }
}

/// Усекает строку до `max` символов с многоточием.
fn clip(text: &str, max: usize) -> String {
    let flat: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max {
        flat
    } else {
        let cut: String = flat.chars().take(max.saturating_sub(1)).collect();
        format!("{cut}…")
    }
}

/// Слешевый путь находки относительно репозитория.
fn rel_file(repo: &Path, file: &Path) -> String {
    file.strip_prefix(repo)
        .unwrap_or(file)
        .display()
        .to_string()
        .replace('\\', "/")
}

/// Детекторы правил по прогону реестра: по одному на правило, кроме
/// `unverifiable` (ручной контроль ничего не измеряет).
///
/// # Errors
/// Реестр не читается.
pub fn rule_detectors(
    repo: &Path,
    constraints: &Path,
    report: &FitnessReport,
) -> Result<Vec<RuleDetector>> {
    let resolved = super::load_constraints_resolved(constraints)?;
    let teeth = super::teeth::load(repo);
    Ok(detectors_for(repo, &resolved.rules, report, teeth.as_ref()))
}

/// Детекторы по уже разобранным правилам (чистая функция — для тестов и для
/// вызывающих, у которых реестр уже в руках).
#[must_use]
pub fn detectors_for(
    repo: &Path,
    rules: &[FitnessRule],
    report: &FitnessReport,
    teeth: Option<&super::teeth::TeethReport>,
) -> Vec<RuleDetector> {
    let mut skipped: BTreeMap<&str, &str> = BTreeMap::new();
    for s in &report.runner_skipped {
        skipped.insert(s.rule.as_str(), s.reason.as_str());
    }
    for s in &report.untrusted_skipped {
        skipped.insert(s.rule.as_str(), s.reason.as_str());
    }
    for s in &report.requires_skipped {
        skipped.insert(s.rule.as_str(), s.reason.as_str());
    }
    for s in &report.skipped {
        skipped.insert(s.rule.as_str(), s.reason.as_str());
    }
    let na: BTreeMap<&str, &str> = report
        .not_applicable
        .iter()
        .map(|n| (n.rule.as_str(), n.reason.as_str()))
        .collect();
    // Полный override (без глобов) выключает правило; узкий только сужает
    // набор файлов — правило прогоняется, и его исход измерен.
    let overridden: BTreeMap<&str, String> = report
        .overrides
        .iter()
        .filter(|o| o.status == "active" && o.exclude_glob.is_empty())
        .map(|o| {
            (
                o.rule.as_str(),
                format!("override с {} до {}", o.adr, o.until),
            )
        })
        .collect();
    let mut by_rule: BTreeMap<&str, Vec<&super::types::LintIssue>> = BTreeMap::new();
    for issue in &report.issues {
        by_rule.entry(issue.rule.as_str()).or_default().push(issue);
    }
    let mut out = Vec::new();
    for rule in rules.iter().filter(|r| !r.unverifiable) {
        let name = rule.name.as_str();
        let issues = by_rule.get(name).map(Vec::as_slice).unwrap_or_default();
        let (outcome, reason) = if let Some(why) = overridden.get(name) {
            (RuleOutcome::Overridden, Some(why.clone()))
        } else if let Some(why) = na.get(name) {
            (RuleOutcome::Na, Some((*why).to_string()))
        } else if let Some(why) = skipped.get(name) {
            (RuleOutcome::Skip, Some((*why).to_string()))
        } else if issues.iter().any(|i| i.severity == "error") {
            (RuleOutcome::Fail, None)
        } else if issues.is_empty() {
            (RuleOutcome::Pass, None)
        } else {
            (RuleOutcome::Warn, None)
        };
        let mut findings: Vec<DetectorFinding> = issues
            .iter()
            .map(|i| DetectorFinding {
                file: rel_file(repo, &i.file),
                line: i.line,
                message: clip(&i.message, MAX_DETECTOR_MESSAGE_CHARS),
            })
            .collect();
        findings.sort_by(|a, b| (&a.file, a.line, &a.message).cmp(&(&b.file, b.line, &b.message)));
        findings.dedup();
        let files: BTreeSet<String> = findings
            .iter()
            .map(|f| f.file.clone())
            .filter(|f| !f.is_empty())
            .collect();
        let total = findings.len();
        findings.truncate(MAX_DETECTOR_FINDINGS);
        let severity = super::types::normalize_severity(&rule.severity, &rule.name)
            .map_or_else(|_| rule.severity.clone(), str::to_string);
        // Замер зубьев засчитывается только для той же редакции правила.
        let teeth_status = teeth.and_then(|t| {
            let fingerprint = super::teeth::rule_fingerprint(rule);
            t.entries
                .iter()
                .find(|e| e.name == rule.name && e.fingerprint == fingerprint)
                .map(|e| e.status.as_str().to_string())
        });
        out.push(RuleDetector {
            schema: RULE_DETECTOR_SCHEMA.to_string(),
            name: rule.id.clone().unwrap_or_else(|| rule.name.clone()),
            rule: rule.name.clone(),
            status: outcome.as_str().to_string(),
            severity,
            kind: rule.kind.as_str().to_string(),
            ad: rule.ad.clone(),
            reason,
            teeth: teeth_status,
            findings_total: total,
            files: files.into_iter().collect(),
            findings,
        });
    }
    out
}

/// Пишет детекторы в `reports/detectors/rules/` и удаляет файлы правил,
/// которых в прогоне нет (правило удалено или стало `unverifiable`): иначе
/// досье продолжало бы показывать судье исход несуществующего правила.
///
/// Файл с тем же содержимым не перезаписывается — время изменения остаётся
/// прежним, а хэш досье не меняется по построению.
///
/// # Errors
/// Каталог не создаётся или файл не пишется.
pub fn write_rule_detectors(repo: &Path, detectors: &[RuleDetector]) -> Result<Vec<PathBuf>> {
    let dir = repo.join(RULE_DETECTORS_DIR);
    std::fs::create_dir_all(&dir).map_err(|e| HarnessError::io(&dir, e))?;
    let mut written = Vec::new();
    let mut keep: BTreeSet<String> = BTreeSet::new();
    for detector in detectors {
        let file = format!("{}.json", file_stem_of(&detector.name));
        let path = dir.join(&file);
        let text = detector.to_text()?;
        let same = std::fs::read_to_string(&path).is_ok_and(|old| old == text);
        if !same {
            std::fs::write(&path, &text).map_err(|e| HarnessError::io(&path, e))?;
        }
        keep.insert(file);
        written.push(path);
    }
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if path.extension().is_some_and(|x| x == "json") && !keep.contains(&name) {
                // Удаляется только наш собственный экспорт в нашем подкаталоге;
                // неудача удаления не ломает прогон — устаревший детектор
                // назовёт себя статусом, а следующий экспорт повторит попытку.
                let _ = std::fs::remove_file(&path);
            }
        }
    }
    Ok(written)
}

/// Экспорт прогона реестра одним вызовом: детекторы по правилам и запись.
///
/// # Errors
/// Реестр не читается или файлы не пишутся.
pub fn export_rule_detectors(
    repo: &Path,
    constraints: &Path,
    report: &FitnessReport,
) -> Result<Vec<PathBuf>> {
    let detectors = rule_detectors(repo, constraints, report)?;
    write_rule_detectors(repo, &detectors)
}

/// Читает экспортированные детекторы правил репозитория; битый файл
/// пропускается (досье назовёт его отдельной ошибкой при сборке).
#[must_use]
pub fn load_rule_detectors(repo: &Path) -> Vec<(PathBuf, RuleDetector)> {
    let dir = repo.join(RULE_DETECTORS_DIR);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut paths: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("json")))
        .collect();
    paths.sort();
    paths
        .into_iter()
        .filter_map(|path| {
            let text = std::fs::read_to_string(&path).ok()?;
            let detector: RuleDetector = serde_json::from_str(&text).ok()?;
            Some((path, detector))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::types::LintIssue;

    fn rules_yaml() -> &'static str {
        "rules:\n\
         - id: C-01\n  name: no_float_money\n  type: must_not_contain\n  glob: \"src/**/*.rs\"\n  pattern: \"f64\"\n  severity: critical\n  ad: AD-1\n\
         - id: C-02\n  name: has_readme\n  type: file_exists\n  path: README.md\n  severity: warn\n\
         - id: C-03\n  name: manual_review\n  unverifiable: true\n\
         - name: no_todo\n  type: must_not_contain\n  glob: \"src/**/*.rs\"\n  pattern: \"TODO\"\n  severity: warn\n"
    }

    fn setup() -> (tempfile::TempDir, PathBuf) {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path().to_path_buf();
        std::fs::create_dir_all(dir.join("src")).expect("src");
        std::fs::write(dir.join("src/pay.rs"), "let amount: f64 = 1.0;\n// TODO\n").expect("pay");
        std::fs::write(dir.join("CONSTRAINTS.yaml"), rules_yaml()).expect("rules");
        (tmp, dir)
    }

    /// Исходы правил выводятся из настоящего прогона реестра: fail по
    /// error-находке, warn по warn-находке, pass без находок; `unverifiable`
    /// детектором не становится.
    #[test]
    fn detectors_follow_real_check_outcomes() {
        let (_tmp, dir) = setup();
        let constraints = dir.join("CONSTRAINTS.yaml");
        let report = crate::control::check(&dir, &constraints).expect("check");
        let detectors = rule_detectors(&dir, &constraints, &report).expect("детекторы");
        let status: BTreeMap<String, String> = detectors
            .iter()
            .map(|d| (d.name.clone(), d.status.clone()))
            .collect();
        assert_eq!(status.get("C-01").map(String::as_str), Some("fail"), "{status:?}");
        assert_eq!(status.get("C-02").map(String::as_str), Some("warn"), "{status:?}");
        assert_eq!(status.get("no_todo").map(String::as_str), Some("warn"), "{status:?}");
        assert!(!status.contains_key("C-03"), "unverifiable не детектор: {status:?}");
        let money = detectors.iter().find(|d| d.name == "C-01").expect("C-01");
        assert_eq!(money.ad.as_deref(), Some("AD-1"));
        assert_eq!(money.files, vec!["src/pay.rs".to_string()]);
        assert_eq!(money.severity, "error");
    }

    /// Экспорт детерминирован: повторная запись того же исхода даёт тот же
    /// текст; устаревший файл правила удаляется.
    #[test]
    fn export_is_deterministic_and_prunes_stale_files() {
        let (_tmp, dir) = setup();
        let constraints = dir.join("CONSTRAINTS.yaml");
        let report = crate::control::check(&dir, &constraints).expect("check");
        let stale = dir.join(RULE_DETECTORS_DIR).join("C-99.json");
        std::fs::create_dir_all(stale.parent().expect("parent")).expect("mkdir");
        std::fs::write(&stale, "{}").expect("stale");
        let first = export_rule_detectors(&dir, &constraints, &report).expect("экспорт");
        let texts: Vec<String> = first
            .iter()
            .map(|p| std::fs::read_to_string(p).expect("read"))
            .collect();
        let second = export_rule_detectors(&dir, &constraints, &report).expect("экспорт");
        let again: Vec<String> = second
            .iter()
            .map(|p| std::fs::read_to_string(p).expect("read"))
            .collect();
        assert_eq!(texts, again);
        assert!(!stale.exists(), "детектор удалённого правила убран");
        assert!(texts.iter().all(|t| !t.contains("judged_at") && t.ends_with('\n')));
        let loaded = load_rule_detectors(&dir);
        assert_eq!(loaded.len(), first.len());
    }

    /// Пропуски и N/A не выдаются за измерение.
    #[test]
    fn skipped_and_na_rules_are_not_measured() {
        let (_tmp, dir) = setup();
        let resolved =
            crate::control::load_constraints_resolved(&dir.join("CONSTRAINTS.yaml")).expect("r");
        let mut report = crate::control::check(&dir, &dir.join("CONSTRAINTS.yaml")).expect("check");
        report.issues.retain(|i| i.rule != "no_float_money");
        report.requires_skipped.push(super::super::types::RequiresSkippedRule {
            rule: "no_float_money".to_string(),
            severity: "error".to_string(),
            resources: vec!["stand".to_string()],
            reason: "нет стенда".to_string(),
        });
        report.not_applicable.push(super::super::types::NotApplicableRule {
            rule: "has_readme".to_string(),
            severity: "warn".to_string(),
            reason: "стек не тот".to_string(),
        });
        report.issues.retain(|i| i.rule != "has_readme");
        let detectors = detectors_for(&dir, &resolved.rules, &report, None);
        let money = detectors.iter().find(|d| d.name == "C-01").expect("C-01");
        assert_eq!(money.outcome(), Some(RuleOutcome::Skip));
        assert!(!RuleOutcome::Skip.measured());
        let readme = detectors.iter().find(|d| d.name == "C-02").expect("C-02");
        assert_eq!(readme.outcome(), Some(RuleOutcome::Na));
        assert_eq!(readme.reason.as_deref(), Some("стек не тот"));
    }

    /// Длинные сообщения усекаются, находки ограничены потолком.
    #[test]
    fn findings_are_capped_and_clipped() {
        let (_tmp, dir) = setup();
        let resolved =
            crate::control::load_constraints_resolved(&dir.join("CONSTRAINTS.yaml")).expect("r");
        let mut report = crate::control::check(&dir, &dir.join("CONSTRAINTS.yaml")).expect("check");
        report.issues = (0..25)
            .map(|n| LintIssue {
                file: dir.join(format!("src/f{n:02}.rs")),
                line: n,
                rule: "no_float_money".to_string(),
                message: "x".repeat(400),
                severity: "error".to_string(),
                ..LintIssue::default()
            })
            .collect();
        let detectors = detectors_for(&dir, &resolved.rules, &report, None);
        let money = detectors.iter().find(|d| d.name == "C-01").expect("C-01");
        assert_eq!(money.findings_total, 25);
        assert_eq!(money.findings.len(), MAX_DETECTOR_FINDINGS);
        assert_eq!(money.files.len(), 25);
        assert!(
            money
                .findings
                .iter()
                .all(|f| f.message.chars().count() <= MAX_DETECTOR_MESSAGE_CHARS)
        );
        assert!(money.answers_to("C-01") && money.answers_to("no_float_money"));
        assert_eq!(money.teeth, None, "без замера зубья не выдумываются");
        assert!(!money.answers_to(""));
    }
}
