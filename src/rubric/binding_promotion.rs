//! Калибровка и промоушен связок «критерий ↔ правило» (ADR-093, волна 4).
//!
//! Принцип «советующая, пока не откалибрована»: связка, объявленная в рубрике
//! `binding_mode: blocking`, решает вердикт гейта, только когда выполнены три
//! условия, и все три механика проверяет сама:
//!
//! 1. **Теневой режим прожит.** Пока связка советующая, гейт считает, что
//!    было бы при `blocking` (сколько субъектов провалилось бы, сколько ушло
//!    бы человеку), и дописывает это в журнал `.arch-handoff/binding-shadow.jsonl`.
//!    Подписать связку, эффект которой никто не видел, нельзя.
//! 2. **Измерение и суждение откалиброваны.** У необходимых правил связки
//!    подтверждены зубья (`arch-be rules teeth --save`), судья рубрики прошёл
//!    строгую квалификацию на эталонном наборе.
//! 3. **Человек подписал.** Запись `reports/human/bindings/<рубрика>--<критерий>.json`
//!    привязана к SHA-256 файла рубрики: правка рубрики после подписи
//!    обесценивает подпись, и связка снова советующая.
//!
//! Подпись не заменяет суждение архитектора — она фиксирует, что он видел
//! теневые цифры и калибровку, и решил дать связке право блокировать.

use std::collections::BTreeMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::types::{BindingMode, BindingRole, Rubric};
use crate::error::{HarnessError, Result};

/// Схема записи подписи связки.
pub const BINDING_PROMOTION_SCHEMA: &str = "arch-be/binding-promotion/v1";
/// Каталог подписей внутри репозитория.
pub const BINDING_PROMOTIONS_DIR: &str = "reports/human/bindings";
/// Журнал теневого режима связок (дописывается гейтом).
pub const BINDING_SHADOW_LOG: &str = ".arch-handoff/binding-shadow.jsonl";
/// Сколько теневых прогонов нужно, прежде чем связку можно подписать.
pub const MIN_SHADOW_RUNS: usize = 1;

/// Строка теневого журнала: что дала бы связка критерия при `blocking` в
/// одном прогоне гейта.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ShadowRecord {
    /// Рубрика.
    pub rubric: String,
    /// Критерий.
    pub criterion: String,
    /// Субъектов в прогоне.
    pub subjects: usize,
    /// Провалилось бы измерением (необходимое правило красное).
    pub would_fail: usize,
    /// Ушло бы человеку (противоречие с правилом, правило не измерено).
    pub would_escalate: usize,
    /// Когда (RFC 3339).
    pub at: String,
}

/// Дописывает теневые записи прогона в журнал.
///
/// # Errors
/// Каталог не создаётся или файл не пишется.
pub fn append_shadow(repo: &Path, records: &[ShadowRecord]) -> Result<()> {
    if records.is_empty() {
        return Ok(());
    }
    let path = repo.join(BINDING_SHADOW_LOG);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| HarnessError::io(parent, e))?;
    }
    let mut text = String::new();
    for record in records {
        let line = serde_json::to_string(record)
            .map_err(|e| HarnessError::Rubric(format!("сериализация теневой записи: {e}")))?;
        text.push_str(&line);
        text.push('\n');
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|e| HarnessError::io(&path, e))?;
    file.write_all(text.as_bytes())
        .map_err(|e| HarnessError::io(&path, e))
}

/// Теневые записи связки критерия; битые строки пропускаются.
#[must_use]
pub fn shadow_records(repo: &Path, rubric: &str, criterion: &str) -> Vec<ShadowRecord> {
    let Ok(text) = std::fs::read_to_string(repo.join(BINDING_SHADOW_LOG)) else {
        return Vec::new();
    };
    text.lines()
        .filter_map(|l| serde_json::from_str::<ShadowRecord>(l).ok())
        .filter(|r| r.rubric == rubric && r.criterion == criterion)
        .collect()
}

/// Состояние зубьев необходимого правила на момент подписи.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuleTeethNote {
    /// Правило.
    pub rule: String,
    /// Статус зубьев (`confirmed`, …) или `не измерены`.
    pub teeth: String,
}

/// Подпись связки критерия (`reports/human/bindings/<рубрика>--<критерий>.json`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BindingPromotion {
    /// Схема.
    pub schema: String,
    /// Рубрика.
    pub rubric: String,
    /// Критерий.
    pub criterion: String,
    /// SHA-256 файла рубрики на момент подписи.
    pub rubric_sha256: String,
    /// Правила связки.
    pub rules: Vec<String>,
    /// Модель-судья, чья квалификация проверена.
    pub judge_model: String,
    /// Зубья необходимых правил.
    pub teeth: Vec<RuleTeethNote>,
    /// Теневых прогонов на момент подписи и их итог.
    pub shadow_runs: usize,
    /// Сколько субъектов провалилось бы в последнем теневом прогоне.
    pub shadow_would_fail: usize,
    /// Сколько ушло бы человеку в последнем теневом прогоне.
    pub shadow_would_escalate: usize,
    /// Кто подписал.
    pub decided_by: String,
    /// Когда (RFC 3339).
    pub decided_at: String,
    /// Обоснование.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reason: String,
}

/// Slug части имени файла.
fn slug(raw: &str) -> String {
    raw.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect()
}

/// Путь записи подписи.
#[must_use]
pub fn promotion_path(repo: &Path, rubric: &str, criterion: &str) -> PathBuf {
    repo.join(BINDING_PROMOTIONS_DIR)
        .join(format!("{}--{}.json", slug(rubric), slug(criterion)))
}

/// Читает подпись; нет файла или он не разбирается — `None`.
#[must_use]
pub fn read_promotion(repo: &Path, rubric: &str, criterion: &str) -> Option<BindingPromotion> {
    let text = std::fs::read_to_string(promotion_path(repo, rubric, criterion)).ok()?;
    let record: BindingPromotion = serde_json::from_str(&text).ok()?;
    (record.schema == BINDING_PROMOTION_SCHEMA).then_some(record)
}

/// Действует ли подпись блокирующей связки критерия: `Ok` — да, `Err` —
/// почему связка остаётся советующей.
///
/// # Errors
/// Подписи нет, она относится к другой редакции рубрики или к другому набору
/// правил.
pub fn signed(
    repo: &Path,
    rubric: &Rubric,
    rubric_path: &Path,
    criterion: &str,
) -> std::result::Result<BindingPromotion, String> {
    let Some(record) = read_promotion(repo, &rubric.name, criterion) else {
        return Err(format!(
            "подписи нет — `arch-be rubric promote {} {criterion}` после теневого режима",
            rubric.name
        ));
    };
    let Some(sha) = crate::hash::sha256_file(rubric_path) else {
        return Err(format!(
            "файл рубрики {} не читается",
            rubric_path.display()
        ));
    };
    if sha != record.rubric_sha256 {
        return Err(format!(
            "рубрику изменили после подписи ({} от {}) — подпись относится к другой редакции",
            record.decided_by, record.decided_at
        ));
    }
    let rules: Vec<String> = rubric
        .criteria
        .iter()
        .find(|c| c.id == criterion)
        .map(|c| c.bound_rules.iter().map(|b| b.rule.clone()).collect())
        .unwrap_or_default();
    if rules != record.rules {
        return Err("состав правил связки не совпадает с подписанным".to_string());
    }
    Ok(record)
}

/// Готовность связки к подписи: что механика проверила и что мешает.
#[derive(Debug, Clone, Default)]
pub struct PromotionCheck {
    /// Препятствия; пусто — подписать можно.
    pub blockers: Vec<String>,
    /// Зубья необходимых правил.
    pub teeth: Vec<RuleTeethNote>,
    /// Теневые прогоны.
    pub shadow: Vec<ShadowRecord>,
    /// Причина отказа или подтверждение квалификации судьи.
    pub qualification: String,
}

impl PromotionCheck {
    /// Можно ли подписывать.
    #[must_use]
    pub fn ready(&self) -> bool {
        self.blockers.is_empty()
    }
}

/// Проверяет готовность связки критерия к подписи.
#[must_use]
pub fn check_promotion(
    repo: &Path,
    rubric: &Rubric,
    criterion: &str,
    judge_model: &str,
) -> PromotionCheck {
    let mut check = PromotionCheck::default();
    let Some(c) = rubric.criteria.iter().find(|c| c.id == criterion) else {
        check.blockers.push(format!(
            "у рубрики '{}' нет критерия '{criterion}'",
            rubric.name
        ));
        return check;
    };
    if c.bound_rules.is_empty() {
        check
            .blockers
            .push(format!("у критерия '{criterion}' нет связок (bound_rules)"));
        return check;
    }
    if c.binding_mode != BindingMode::Blocking {
        check.blockers.push(
            "в рубрике связка не объявлена `binding_mode: blocking` — сначала поменяйте \
             режим, затем подписывайте: подпись привязана к содержимому рубрики"
                .to_string(),
        );
    }
    let detectors: BTreeMap<String, crate::control::RuleDetector> =
        crate::control::load_rule_detectors(repo)
            .into_iter()
            .map(|(_, d)| (d.name.clone(), d))
            .collect();
    for bound in c
        .bound_rules
        .iter()
        .filter(|b| b.role == BindingRole::Necessary)
    {
        let detector = detectors.values().find(|d| d.answers_to(&bound.rule));
        let teeth = detector
            .and_then(|d| d.teeth.clone())
            .unwrap_or_else(|| "не измерены".to_string());
        if detector.is_none() {
            check.blockers.push(format!(
                "исхода правила {} нет в {} — `arch-be control check . --export-detectors`",
                bound.rule,
                crate::control::RULE_DETECTORS_DIR
            ));
        } else if teeth != "confirmed" {
            check.blockers.push(format!(
                "зубья необходимого правила {} не подтверждены ({teeth}) — \
                 `arch-be rules teeth --save`, затем повторный экспорт детекторов",
                bound.rule
            ));
        }
        check.teeth.push(RuleTeethNote {
            rule: bound.rule.clone(),
            teeth,
        });
    }
    let state = super::qualify::qualification_strict(repo, &rubric.name, judge_model);
    check.qualification = super::qualify::refusal_reason(&state, judge_model);
    if !matches!(state, super::qualify::Qualification::Qualified(_)) {
        check
            .blockers
            .push(format!("судья не допущен: {}", check.qualification));
    }
    check.shadow = shadow_records(repo, &rubric.name, criterion);
    if check.shadow.len() < MIN_SHADOW_RUNS {
        check.blockers.push(format!(
            "теневых прогонов {} из {MIN_SHADOW_RUNS} — прогоните гейт с советующей связкой, \
             чтобы увидеть её эффект ({BINDING_SHADOW_LOG})",
            check.shadow.len()
        ));
    }
    check
}

/// Подписывает связку: проверяет готовность и пишет запись подписи.
///
/// # Errors
/// Связка не готова (причины — в ошибке), пустая подпись или запись не
/// пишется.
pub fn promote(
    repo: &Path,
    rubric: &Rubric,
    rubric_path: &Path,
    criterion: &str,
    judge_model: &str,
    decided_by: &str,
    reason: &str,
) -> Result<PathBuf> {
    if decided_by.trim().is_empty() {
        return Err(HarnessError::Rubric(
            "подпись без имени: укажите --by \"Имя <адрес>\"".to_string(),
        ));
    }
    let check = check_promotion(repo, rubric, criterion, judge_model);
    if !check.ready() {
        return Err(HarnessError::Rubric(format!(
            "связка {}#{criterion} не готова к подписи:\n- {}",
            rubric.name,
            check.blockers.join("\n- ")
        )));
    }
    let sha = crate::hash::sha256_file(rubric_path).ok_or_else(|| {
        HarnessError::Rubric(format!(
            "файл рубрики {} не читается",
            rubric_path.display()
        ))
    })?;
    let last = check.shadow.last().cloned();
    let record = BindingPromotion {
        schema: BINDING_PROMOTION_SCHEMA.to_string(),
        rubric: rubric.name.clone(),
        criterion: criterion.to_string(),
        rubric_sha256: sha,
        rules: rubric
            .criteria
            .iter()
            .find(|c| c.id == criterion)
            .map(|c| c.bound_rules.iter().map(|b| b.rule.clone()).collect())
            .unwrap_or_default(),
        judge_model: judge_model.to_string(),
        teeth: check.teeth,
        shadow_runs: check.shadow.len(),
        shadow_would_fail: last.as_ref().map_or(0, |r| r.would_fail),
        shadow_would_escalate: last.as_ref().map_or(0, |r| r.would_escalate),
        decided_by: decided_by.trim().to_string(),
        decided_at: chrono::Local::now().to_rfc3339(),
        reason: reason.trim().to_string(),
    };
    let path = promotion_path(repo, &rubric.name, criterion);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| HarnessError::io(parent, e))?;
    }
    let text = serde_json::to_string_pretty(&record)
        .map_err(|e| HarnessError::Rubric(format!("сериализация подписи: {e}")))?;
    std::fs::write(&path, text).map_err(|e| HarnessError::io(&path, e))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rubric::BoundRule;

    fn rubric(mode: BindingMode) -> Rubric {
        let mut rubric: Rubric =
            serde_yaml_ng::from_str(crate::assets::RUBRIC_CODE_INVARIANT_CONFORMANCE)
                .expect("рубрика");
        let main = rubric
            .criteria
            .iter_mut()
            .find(|c| c.blocking)
            .expect("главный");
        main.bound_rules = vec![BoundRule {
            rule: "C-01".to_string(),
            role: BindingRole::Necessary,
        }];
        main.binding_mode = mode;
        rubric
    }

    fn write_rubric(dir: &Path, rubric: &Rubric) -> PathBuf {
        let path = dir.join("code_invariant_conformance.yaml");
        std::fs::write(&path, serde_yaml_ng::to_string(rubric).expect("yaml")).expect("write");
        path
    }

    fn write_detector(dir: &Path, teeth: Option<&str>) {
        let detector = crate::control::RuleDetector {
            schema: crate::control::RULE_DETECTOR_SCHEMA.to_string(),
            name: "C-01".to_string(),
            rule: "no_float".to_string(),
            status: "pass".to_string(),
            severity: "error".to_string(),
            kind: "must_not_contain".to_string(),
            ad: Some("AD-1".to_string()),
            reason: None,
            teeth: teeth.map(str::to_string),
            findings_total: 0,
            files: Vec::new(),
            findings: Vec::new(),
        };
        crate::control::write_rule_detectors(dir, &[detector]).expect("детектор");
    }

    fn write_qualification(dir: &Path, rubric: &Rubric, passed_metrics: bool) {
        let set = crate::rubric::QualificationSet {
            dir: PathBuf::from("/набор"),
            cases: Vec::new(),
            sha256: "a".repeat(64),
        };
        let outcome = |truth, right: bool| crate::rubric::CaseOutcome {
            file: "x.py".to_string(),
            class: "c".to_string(),
            truth,
            decision: Some(match (truth, right) {
                (crate::rubric::Truth::Defective, true) | (crate::rubric::Truth::Clean, false) => {
                    crate::rubric::RubricDecision::Fail
                }
                _ => crate::rubric::RubricDecision::Pass,
            }),
            weighted_total: 4.0,
            flags: Vec::new(),
            error: None,
        };
        let report = crate::rubric::build_qualification_report(
            rubric,
            "judge-x",
            &set,
            vec![
                outcome(crate::rubric::Truth::Defective, passed_metrics),
                outcome(crate::rubric::Truth::Clean, passed_metrics),
            ],
            1,
        );
        crate::rubric::write_qualification(dir, &report).expect("квалификация");
    }

    fn shadow(dir: &Path) {
        append_shadow(
            dir,
            &[ShadowRecord {
                rubric: "code_invariant_conformance".to_string(),
                criterion: "no_violation".to_string(),
                subjects: 3,
                would_fail: 1,
                would_escalate: 0,
                at: "2026-10-18T12:00:00+03:00".to_string(),
            }],
        )
        .expect("тень");
    }

    /// Готовая связка подписывается; подпись действует, пока рубрика та же,
    /// и теряет силу после правки рубрики.
    #[test]
    fn promotion_requires_all_preconditions_and_binds_to_rubric_content() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path();
        let r = rubric(BindingMode::Blocking);
        let path = write_rubric(dir, &r);
        let not_ready = check_promotion(dir, &r, "no_violation", "judge-x");
        assert_eq!(not_ready.blockers.len(), 3, "{:?}", not_ready.blockers);
        write_detector(dir, Some("confirmed"));
        write_qualification(dir, &r, true);
        shadow(dir);
        let ready = check_promotion(dir, &r, "no_violation", "judge-x");
        assert!(ready.ready(), "{:?}", ready.blockers);
        let written = promote(
            dir,
            &r,
            &path,
            "no_violation",
            "judge-x",
            "Архитектор <a@b>",
            "ок",
        )
        .expect("подпись");
        assert!(written.is_file());
        let record = signed(dir, &r, &path, "no_violation").expect("подпись действует");
        assert_eq!(record.shadow_runs, 1);
        assert_eq!(record.shadow_would_fail, 1);
        // Правка рубрики после подписи.
        std::fs::write(
            &path,
            "# правка\n".to_string() + &std::fs::read_to_string(&path).expect("r"),
        )
        .expect("правка");
        let err = signed(dir, &r, &path, "no_violation").expect_err("подпись устарела");
        assert!(err.contains("изменили после подписи"), "{err}");
    }

    /// Советующая связка, беззубое правило и неквалифицированный судья — три
    /// названных препятствия; подпись не пишется.
    #[test]
    fn promotion_is_refused_with_named_blockers() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path();
        let r = rubric(BindingMode::Advisory);
        let path = write_rubric(dir, &r);
        write_detector(dir, Some("toothless"));
        write_qualification(dir, &r, false);
        shadow(dir);
        let check = check_promotion(dir, &r, "no_violation", "judge-x");
        let text = check.blockers.join(" | ");
        assert!(text.contains("binding_mode: blocking"), "{text}");
        assert!(text.contains("toothless"), "{text}");
        assert!(text.contains("судья не допущен"), "{text}");
        let err =
            promote(dir, &r, &path, "no_violation", "judge-x", "А", "").expect_err("не готово");
        assert!(err.to_string().contains("не готова к подписи"));
        assert!(!promotion_path(dir, &r.name, "no_violation").exists());
    }

    /// Строгий допуск: ручная правка `passed: true` без метрик не проходит.
    #[test]
    fn strict_qualification_rejects_hand_edited_pass() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path();
        let r = rubric(BindingMode::Blocking);
        write_qualification(dir, &r, false);
        let path = dir
            .join(crate::rubric::QUALIFICATION_DIR)
            .read_dir()
            .expect("dir")
            .flatten()
            .next()
            .expect("отчёт")
            .path();
        let mut value: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).expect("read")).expect("json");
        value["passed"] = serde_json::json!(true);
        value["failures"] = serde_json::json!([]);
        std::fs::write(&path, value.to_string()).expect("правка");
        assert!(matches!(
            crate::rubric::qualification(dir, &r.name, "judge-x"),
            crate::rubric::Qualification::Qualified(_)
        ));
        assert!(matches!(
            crate::rubric::qualification_strict(dir, &r.name, "judge-x"),
            crate::rubric::Qualification::Failed(_)
        ));
    }
}
