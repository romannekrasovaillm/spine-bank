//! Связка «критерий рубрики ↔ fitness-правило» в гейте (ADR-093, волна 2).
//!
//! Принцип fitness-функции: одна архитектурная характеристика — пара
//! «детерминированное измерение + суждение по рубрике». Связка объявляется в
//! рубрике (`bound_rules` у критерия), исходы правил берутся из ЭТОГО ЖЕ
//! прогона гейта (составляющая `fitness`), а таблица сочетаний ниже решает,
//! что делает механика. Модель здесь не вызывается и баллы не пересчитываются.
//!
//! | Исход правила для субъекта | Роль | Что делает механика |
//! |---|---|---|
//! | `fail` | `necessary` | критерий провален без судьи: отчёт не нужен |
//! | `fail`, судья поставил ≥ 3 | любая | `binding_contradiction` — измерение первично |
//! | `pass` | `evidence` | исход — свидетельство, судья ищет то, чего правило не видит |
//! | `pass`, зубья не подтверждены | `necessary` | `binding_rule_toothless` — условие не измерено |
//! | `skip` / нет прогона | любая | `binding_not_measured` — суждению не на что опереться |
//! | `na` / `overridden` | любая | предупреждение: правило не применимо или снято решением |
//! | правила нет в реестре | любая | `binding_rule_unknown` — связка на пустоту |
//!
//! Режим связки (`binding_mode`) решает вес: `advisory` — только
//! предупреждения (кроме ошибки конфигурации `binding_rule_unknown`),
//! `blocking` — провал и эскалации по таблице. Волна 4: блокирующая связка
//! опирается на суждение судьи только при допуске судьи на эталонном наборе —
//! иначе зависящие от балла нарушения (противоречие) понижаются до
//! предупреждений, а понижение называется находкой `binding_unqualified`
//! (эскалация). Провал необходимого правила — чистое измерение и от допуска
//! судьи не зависит.
//!
//! Правило — на весь репозиторий, субъект — один артефакт: красное правило
//! относится к субъекту, только если его находки лежат в файле субъекта или
//! правило глобальное (находки на уровне реестра, без файла).

use crate::control::{RuleDetector, RuleOutcome};
use crate::rubric::{BindingMode, BindingRole, Criterion, RubricArtifact};

use super::types::GateFinding;

/// Ниже этого балла судья согласен с красным правилом (нарушение — балл ≤ 2,
/// как у `DETECTOR_CONTRADICTION_MIN_SCORE` движка рубрик).
const CONTRADICTION_MIN_SCORE: u8 = 3;

/// Имя файла реестра: находки правил уровня реестра (команды, `file_exists`
/// без цели) адресуются ему — такие правила глобальные.
const REGISTRY_FILE_SUFFIX: &str = "CONSTRAINTS.yaml";

/// Вид нарушения связки.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum BindingIssueKind {
    /// Необходимое правило красное для этого субъекта.
    NecessaryFailed {
        /// Находки правила, относящиеся к субъекту (первые).
        findings: Vec<String>,
    },
    /// Правило красное, а судья нарушения не подтвердил.
    Contradiction {
        /// Балл судьи по критерию.
        score: u8,
    },
    /// Правило не прогонялось (нет ресурса, доверия, прогона реестра).
    NotMeasured {
        /// Почему.
        reason: String,
    },
    /// Правило неприменимо (`na`) или снято override с ADR.
    NotApplicable {
        /// Исход (`na` / `overridden`) и причина.
        reason: String,
    },
    /// Необходимое правило зелёное, но его зубья не подтверждены.
    Toothless {
        /// Статус замера (`toothless`, `trivial`, `glob_empty`) или «не измерены».
        teeth: String,
    },
    /// Правила с таким идентификатором нет в реестре.
    RuleUnknown,
    /// Связка объявлена блокирующей, а судья рубрики не квалифицирован
    /// (волна 4): блокировать по неоткалиброванному суждению нельзя.
    Unqualified {
        /// Причина отказа допуска.
        reason: String,
    },
    /// Связка объявлена блокирующей, но человек её не подписал или подпись
    /// устарела (волна 4): до подписи связка советующая.
    Unsigned {
        /// Почему подпись не действует.
        reason: String,
    },
}

/// Нарушение одной связки «критерий ↔ правило».
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct BindingIssue {
    /// Критерий рубрики.
    pub criterion: String,
    /// Правило (как записано в связке).
    pub rule: String,
    /// Роль правила.
    pub role: BindingRole,
    /// Вес нарушения: режим связки критерия, для зависящих от судьи видов —
    /// после понижения неквалифицированного судьи (волна 4).
    pub mode: BindingMode,
    /// Вид нарушения.
    pub kind: BindingIssueKind,
}

/// Итог связок субъекта.
#[derive(Debug, Default)]
pub(super) struct BindingVerdict {
    /// Нарушения в стабильном порядке критериев и связок.
    pub issues: Vec<BindingIssue>,
}

impl BindingVerdict {
    /// Решён ли субъект детерминированно: необходимое правило красное в
    /// блокирующей связке. Тогда отчёт судьи не нужен — «нет отчёта» и
    /// «отчёт устарел» не находки (короткое замыкание).
    #[must_use]
    pub fn decided_fail(&self) -> bool {
        self.issues.iter().any(|i| {
            i.mode == BindingMode::Blocking
                && matches!(i.kind, BindingIssueKind::NecessaryFailed { .. })
        })
    }

    /// Эффект связок критерия в блокирующем режиме: (провал измерением,
    /// решение человека при политике `human`) — для теневого режима.
    #[must_use]
    pub fn criterion_effect(&self, criterion: &str) -> (bool, bool) {
        let mine = self.issues.iter().filter(|i| i.criterion == criterion);
        let mut fail = false;
        let mut escalate = false;
        for issue in mine {
            fail |= matches!(issue.kind, BindingIssueKind::NecessaryFailed { .. });
            escalate |= escalates(issue, crate::config::HumanPolicy::Human);
        }
        (fail, escalate)
    }

    /// Сколько нарушений требуют решения человека при этой политике.
    #[must_use]
    pub fn escalations(&self, policy: crate::config::HumanPolicy) -> usize {
        self.issues.iter().filter(|i| escalates(i, policy)).count()
    }

    /// Находки гейта по нарушениям связок.
    #[must_use]
    pub fn findings(
        &self,
        rubric: &str,
        subject: &str,
        policy: crate::config::HumanPolicy,
    ) -> Vec<GateFinding> {
        self.issues
            .iter()
            .map(|issue| finding_of(issue, rubric, subject, policy))
            .collect()
    }
}

/// Требует ли нарушение решения человека (вердикт INCOMPLETE).
fn escalates(issue: &BindingIssue, policy: crate::config::HumanPolicy) -> bool {
    if issue.mode == BindingMode::Advisory {
        return false;
    }
    match &issue.kind {
        // Измерение против суждения: политика маршрута его не понижает.
        BindingIssueKind::Contradiction { .. } | BindingIssueKind::Unqualified { .. } => true,
        BindingIssueKind::NotMeasured { .. } => policy == crate::config::HumanPolicy::Human,
        BindingIssueKind::NecessaryFailed { .. }
        | BindingIssueKind::NotApplicable { .. }
        | BindingIssueKind::Toothless { .. }
        | BindingIssueKind::Unsigned { .. }
        | BindingIssueKind::RuleUnknown => false,
    }
}

/// Severity находки: error — провал, эскалация или ошибка конфигурации.
fn severity_of(issue: &BindingIssue, policy: crate::config::HumanPolicy) -> &'static str {
    if matches!(issue.kind, BindingIssueKind::RuleUnknown) {
        return "error";
    }
    if issue.mode == BindingMode::Advisory {
        return "warn";
    }
    match issue.kind {
        BindingIssueKind::NecessaryFailed { .. } => "error",
        BindingIssueKind::Unsigned { .. } => "warn",
        _ if escalates(issue, policy) => "error",
        _ => "warn",
    }
}

/// Находка гейта по одному нарушению связки.
fn finding_of(
    issue: &BindingIssue,
    rubric: &str,
    subject: &str,
    policy: crate::config::HumanPolicy,
) -> GateFinding {
    let who = if matches!(
        issue.kind,
        BindingIssueKind::Unqualified { .. } | BindingIssueKind::Unsigned { .. }
    ) {
        format!(
            "субъект '{subject}' (рубрика {rubric}, критерий {} ↔ правила {} [{}])",
            issue.criterion,
            issue.rule,
            issue.mode.as_str()
        )
    } else {
        format!(
            "субъект '{subject}' (рубрика {rubric}, критерий {} ↔ правило {} [{} · {}])",
            issue.criterion,
            issue.rule,
            issue.role.as_str(),
            issue.mode.as_str()
        )
    };
    let escalated = escalates(issue, policy);
    let tail = if escalated {
        " — решение за человеком"
    } else if issue.mode == BindingMode::Advisory {
        " — связка советующая, вердикт не меняется"
    } else {
        ""
    };
    let (code, message) = match &issue.kind {
        BindingIssueKind::NecessaryFailed { findings } => (
            "binding_rule_failed",
            format!(
                "{who}: необходимое правило красное для этого субъекта ({}) — критерий \
                 провален измерением, суждение судьи его не отменяет{tail}",
                findings.join("; ")
            ),
        ),
        BindingIssueKind::Contradiction { score } => (
            "binding_contradiction",
            format!(
                "{who}: правило красное, а судья поставил критерию {score} — нарушение, о \
                 котором сообщает измерение, суждением не подтверждено{tail}"
            ),
        ),
        BindingIssueKind::NotMeasured { reason } => (
            "binding_not_measured",
            format!(
                "{who}: правило не прогонялось ({reason}) — суждению по критерию не на что \
                 опереться{tail}"
            ),
        ),
        BindingIssueKind::NotApplicable { reason } => (
            "binding_rule_not_applicable",
            format!("{who}: правило не применялось ({reason}){tail}"),
        ),
        BindingIssueKind::Toothless { teeth } => (
            "binding_rule_toothless",
            format!(
                "{who}: необходимое правило зелёное, но зубья не подтверждены ({teeth}) — \
                 «зелёный» не доказывает выполнение условия; `arch-be rules teeth --save`{tail}"
            ),
        ),
        BindingIssueKind::RuleUnknown => (
            "binding_rule_unknown",
            format!(
                "{who}: правила с таким идентификатором или именем нет в реестре — связка \
                 указывает в пустоту"
            ),
        ),
        BindingIssueKind::Unsigned { reason } => (
            "binding_unsigned",
            format!(
                "{who}: связка объявлена блокирующей, но не действует ({reason}) — до подписи \
                 человека она советующая; теневые цифры — в {}",
                crate::rubric::BINDING_SHADOW_LOG
            ),
        ),
        BindingIssueKind::Unqualified { reason } => (
            "binding_unqualified",
            format!(
                "{who}: связка объявлена блокирующей, а судья не допущен ({reason}) — \
                 блокировать по неоткалиброванному суждению нельзя; `arch-be rubric qualify`{tail}"
            ),
        ),
    };
    GateFinding::ruled(
        severity_of(issue, policy).to_string(),
        code.to_string(),
        message,
    )
}

/// Относится ли исход правила к субъекту: находки в файле субъекта, либо
/// правило глобальное (находки без файла или на уровне реестра).
fn applies_to_subject(detector: &RuleDetector, subject_path: Option<&str>) -> bool {
    let global = detector.files.is_empty()
        || detector
            .files
            .iter()
            .all(|f| f.is_empty() || f.ends_with(REGISTRY_FILE_SUFFIX));
    global || subject_path.is_some_and(|p| detector.files.iter().any(|f| f == p))
}

/// Находки правила, относящиеся к субъекту — для сообщения.
fn subject_findings(detector: &RuleDetector, subject_path: Option<&str>) -> Vec<String> {
    let mut out: Vec<String> = detector
        .findings
        .iter()
        .filter(|f| {
            subject_path.is_none_or(|p| f.file == p)
                || f.file.is_empty()
                || f.file.ends_with(REGISTRY_FILE_SUFFIX)
        })
        .take(3)
        .map(|f| {
            if f.line > 0 {
                format!("{}:{} {}", f.file, f.line, f.message)
            } else {
                format!("{} {}", f.file, f.message)
            }
        })
        .collect();
    if out.is_empty() {
        out.push(format!("находок: {}", detector.findings_total));
    }
    out
}

/// Связки одного критерия против исходов правил.
fn criterion_issues(
    criterion: &Criterion,
    (mode, judge_mode): (BindingMode, BindingMode),
    outcomes: Option<&[RuleDetector]>,
    subject_path: Option<&str>,
    artifact: Option<&RubricArtifact>,
    out: &mut Vec<BindingIssue>,
) {
    let score = artifact.and_then(|a| {
        a.scores
            .iter()
            .find(|s| s.criterion_id == criterion.id)
            .filter(|s| !s.flags.iter().any(|f| f.excludes_from_total()))
            .map(|s| s.score)
    });
    for bound in &criterion.bound_rules {
        let issue = |kind| BindingIssue {
            criterion: criterion.id.clone(),
            rule: bound.rule.clone(),
            role: bound.role,
            mode,
            kind,
        };
        let Some(outcomes) = outcomes else {
            out.push(issue(BindingIssueKind::NotMeasured {
                reason: "реестр правил в этом прогоне не исполнялся".to_string(),
            }));
            continue;
        };
        let Some(detector) = outcomes.iter().find(|d| d.answers_to(&bound.rule)) else {
            out.push(issue(BindingIssueKind::RuleUnknown));
            continue;
        };
        match detector.outcome() {
            Some(RuleOutcome::Fail) if applies_to_subject(detector, subject_path) => {
                if bound.role == BindingRole::Necessary {
                    out.push(issue(BindingIssueKind::NecessaryFailed {
                        findings: subject_findings(detector, subject_path),
                    }));
                } else if let Some(score) = score.filter(|s| *s >= CONTRADICTION_MIN_SCORE) {
                    out.push(BindingIssue {
                        mode: judge_mode,
                        ..issue(BindingIssueKind::Contradiction { score })
                    });
                }
            }
            // Красное правило в чужих файлах для этого субъекта — зелёное.
            Some(RuleOutcome::Fail | RuleOutcome::Pass | RuleOutcome::Warn) => {
                let teeth = detector.teeth.as_deref();
                if bound.role == BindingRole::Necessary && teeth != Some("confirmed") {
                    out.push(issue(BindingIssueKind::Toothless {
                        teeth: teeth.unwrap_or("не измерены").to_string(),
                    }));
                }
            }
            Some(RuleOutcome::Skip) | None => out.push(issue(BindingIssueKind::NotMeasured {
                reason: detector
                    .reason
                    .clone()
                    .unwrap_or_else(|| format!("исход '{}'", detector.status)),
            })),
            Some(outcome @ (RuleOutcome::Na | RuleOutcome::Overridden)) => {
                out.push(issue(BindingIssueKind::NotApplicable {
                    reason: format!(
                        "{}: {}",
                        outcome.as_str(),
                        detector.reason.as_deref().unwrap_or("без причины")
                    ),
                }));
            }
        }
    }
}

/// Связки рубрики для субъекта.
///
/// `qualified` — допущен ли судья рубрики к блокирующим связкам (волна 4):
/// отказ понижает зависящие от судьи нарушения до советующих и добавляет
/// находку `binding_unqualified` (эскалация), чтобы понижение было видно.
/// Вызывающий проверяет допуск только когда отчёт судьи есть — без отчёта
/// суждения нет, и решают одни измерения.
///
/// `unsigned` — критерии с блокирующей связкой без действующей подписи
/// человека (причина): такая связка работает как советующая, а находка
/// `binding_unsigned` называет понижение.
#[must_use]
pub(super) fn evaluate(
    rubric: &crate::rubric::Rubric,
    outcomes: Option<&[RuleDetector]>,
    subject_path: Option<&str>,
    artifact: Option<&RubricArtifact>,
    qualified: &Result<(), String>,
    unsigned: &std::collections::BTreeMap<String, String>,
) -> BindingVerdict {
    let mut issues = Vec::new();
    for criterion in rubric.criteria.iter().filter(|c| !c.bound_rules.is_empty()) {
        let mut mode = criterion.binding_mode;
        if mode == BindingMode::Blocking {
            if let Some(reason) = unsigned.get(&criterion.id) {
                issues.push(BindingIssue {
                    criterion: criterion.id.clone(),
                    rule: criterion
                        .bound_rules
                        .iter()
                        .map(|b| b.rule.as_str())
                        .collect::<Vec<_>>()
                        .join(", "),
                    role: BindingRole::Necessary,
                    mode: BindingMode::Blocking,
                    kind: BindingIssueKind::Unsigned {
                        reason: reason.clone(),
                    },
                });
                mode = BindingMode::Advisory;
            }
        }
        let mut judge_mode = mode;
        if mode == BindingMode::Blocking {
            if let Err(reason) = qualified {
                issues.push(BindingIssue {
                    criterion: criterion.id.clone(),
                    rule: criterion
                        .bound_rules
                        .iter()
                        .map(|b| b.rule.as_str())
                        .collect::<Vec<_>>()
                        .join(", "),
                    role: BindingRole::Necessary,
                    mode: BindingMode::Blocking,
                    kind: BindingIssueKind::Unqualified {
                        reason: reason.clone(),
                    },
                });
                judge_mode = BindingMode::Advisory;
            }
        }
        criterion_issues(
            criterion,
            (mode, judge_mode),
            outcomes,
            subject_path,
            artifact,
            &mut issues,
        );
    }
    BindingVerdict { issues }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::HumanPolicy;
    use crate::rubric::BoundRule;

    fn detector(name: &str, status: &str, files: &[&str], teeth: Option<&str>) -> RuleDetector {
        RuleDetector {
            schema: crate::control::RULE_DETECTOR_SCHEMA.to_string(),
            name: name.to_string(),
            rule: format!("rule_{name}"),
            status: status.to_string(),
            severity: "error".to_string(),
            kind: "must_not_contain".to_string(),
            ad: Some("AD-1".to_string()),
            reason: (status == "skip").then(|| "нет стенда".to_string()),
            teeth: teeth.map(str::to_string),
            findings_total: files.len(),
            files: files.iter().map(|f| (*f).to_string()).collect(),
            findings: files
                .iter()
                .map(|f| crate::control::DetectorFinding {
                    file: (*f).to_string(),
                    line: 3,
                    message: "нарушение".to_string(),
                })
                .collect(),
        }
    }

    fn rubric_with(role: BindingRole, mode: BindingMode) -> crate::rubric::Rubric {
        let yaml = crate::assets::RUBRIC_CODE_INVARIANT_CONFORMANCE;
        let mut rubric: crate::rubric::Rubric = serde_yaml_ng::from_str(yaml).expect("рубрика");
        let main = rubric
            .criteria
            .iter_mut()
            .find(|c| c.blocking)
            .expect("главный критерий");
        main.bound_rules = vec![BoundRule {
            rule: "C-01".to_string(),
            role,
        }];
        main.binding_mode = mode;
        rubric
    }

    fn artifact_with_score(score: u8) -> RubricArtifact {
        serde_json::from_value(serde_json::json!({
            "schema": crate::rubric::RUBRIC_REPORT_SCHEMA,
            "rubric": "code_invariant_conformance",
            "judge_model": "j",
            "weighted_total": 4.0,
            "verdict": "OK",
            "scores": [{"criterion_id": "no_violation", "score": score, "rationale": "r"}],
            "judged_at": "2026-10-18T10:00:00+03:00",
        }))
        .expect("артефакт")
    }

    /// Необходимое правило красное в файле субъекта: критерий провален без
    /// судьи — даже без отчёта (короткое замыкание).
    #[test]
    fn necessary_fail_decides_without_judge() {
        let rubric = rubric_with(BindingRole::Necessary, BindingMode::Blocking);
        let outcomes = [detector("C-01", "fail", &["src/pay.rs"], Some("confirmed"))];
        let verdict = evaluate(
            &rubric,
            Some(&outcomes),
            Some("src/pay.rs"),
            None,
            &Ok(()),
            &std::collections::BTreeMap::new(),
        );
        assert!(verdict.decided_fail(), "{verdict:?}");
        let findings = verdict.findings("r", "src/pay.rs", HumanPolicy::Warn);
        assert_eq!(findings[0].rule.as_deref(), Some("binding_rule_failed"));
        assert_eq!(findings[0].severity, "error");
        assert!(findings[0].message.contains("src/pay.rs:3"));
    }

    /// Красное правило в чужом файле к субъекту не относится.
    #[test]
    fn rule_failure_elsewhere_does_not_touch_subject() {
        let rubric = rubric_with(BindingRole::Necessary, BindingMode::Blocking);
        let outcomes = [detector(
            "C-01",
            "fail",
            &["src/other.rs"],
            Some("confirmed"),
        )];
        let verdict = evaluate(
            &rubric,
            Some(&outcomes),
            Some("src/pay.rs"),
            None,
            &Ok(()),
            &std::collections::BTreeMap::new(),
        );
        assert!(!verdict.decided_fail());
        assert!(verdict.issues.is_empty(), "{verdict:?}");
    }

    /// Глобальное правило (находка на уровне реестра) относится к любому субъекту.
    #[test]
    fn registry_level_failure_is_global() {
        let rubric = rubric_with(BindingRole::Necessary, BindingMode::Blocking);
        let outcomes = [detector(
            "C-01",
            "fail",
            &["CONSTRAINTS.yaml"],
            Some("confirmed"),
        )];
        let verdict = evaluate(
            &rubric,
            Some(&outcomes),
            Some("docs/adr/ADR-1.md"),
            None,
            &Ok(()),
            &std::collections::BTreeMap::new(),
        );
        assert!(verdict.decided_fail());
    }

    /// Советующая связка: тот же провал — только предупреждение.
    #[test]
    fn advisory_binding_only_warns() {
        let rubric = rubric_with(BindingRole::Necessary, BindingMode::Advisory);
        let outcomes = [detector("C-01", "fail", &["src/pay.rs"], Some("confirmed"))];
        let verdict = evaluate(
            &rubric,
            Some(&outcomes),
            Some("src/pay.rs"),
            None,
            &Ok(()),
            &std::collections::BTreeMap::new(),
        );
        assert!(!verdict.decided_fail());
        let findings = verdict.findings("r", "s", HumanPolicy::Human);
        assert_eq!(findings[0].severity, "warn");
        assert_eq!(verdict.escalations(HumanPolicy::Human), 0);
    }

    /// Правило-свидетельство красное, судья поставил «чисто» — противоречие
    /// эскалирует на любом маршруте; судья согласился — нарушения связки нет.
    #[test]
    fn evidence_fail_against_high_score_is_contradiction() {
        let rubric = rubric_with(BindingRole::Evidence, BindingMode::Blocking);
        let outcomes = [detector("C-01", "fail", &["src/pay.rs"], None)];
        let high = artifact_with_score(4);
        let verdict = evaluate(
            &rubric,
            Some(&outcomes),
            Some("src/pay.rs"),
            Some(&high),
            &Ok(()),
            &std::collections::BTreeMap::new(),
        );
        assert!(matches!(
            verdict.issues[0].kind,
            BindingIssueKind::Contradiction { score: 4 }
        ));
        assert_eq!(verdict.escalations(HumanPolicy::Warn), 1);
        let low = artifact_with_score(1);
        let agreed = evaluate(
            &rubric,
            Some(&outcomes),
            Some("src/pay.rs"),
            Some(&low),
            &Ok(()),
            &std::collections::BTreeMap::new(),
        );
        assert!(agreed.issues.is_empty(), "{agreed:?}");
    }

    /// Пропуск правила — «не измерено»: эскалация только при политике human.
    #[test]
    fn skipped_rule_is_not_measured() {
        let rubric = rubric_with(BindingRole::Evidence, BindingMode::Blocking);
        let outcomes = [detector("C-01", "skip", &[], None)];
        let verdict = evaluate(
            &rubric,
            Some(&outcomes),
            Some("src/pay.rs"),
            None,
            &Ok(()),
            &std::collections::BTreeMap::new(),
        );
        assert!(matches!(
            verdict.issues[0].kind,
            BindingIssueKind::NotMeasured { .. }
        ));
        assert_eq!(verdict.escalations(HumanPolicy::Warn), 0);
        assert_eq!(verdict.escalations(HumanPolicy::Human), 1);
        let none = evaluate(
            &rubric,
            None,
            Some("src/pay.rs"),
            None,
            &Ok(()),
            &std::collections::BTreeMap::new(),
        );
        assert!(matches!(
            none.issues[0].kind,
            BindingIssueKind::NotMeasured { .. }
        ));
    }

    /// Связка на несуществующее правило — ошибка конфигурации даже в
    /// советующем режиме.
    #[test]
    fn unknown_rule_is_error_in_any_mode() {
        let rubric = rubric_with(BindingRole::Evidence, BindingMode::Advisory);
        let outcomes = [detector("C-77", "pass", &[], None)];
        let verdict = evaluate(
            &rubric,
            Some(&outcomes),
            Some("src/pay.rs"),
            None,
            &Ok(()),
            &std::collections::BTreeMap::new(),
        );
        let findings = verdict.findings("r", "s", HumanPolicy::Warn);
        assert_eq!(findings[0].rule.as_deref(), Some("binding_rule_unknown"));
        assert_eq!(findings[0].severity, "error");
    }

    /// Зелёное необходимое правило без подтверждённых зубьев — предупреждение.
    #[test]
    fn green_necessary_rule_without_teeth_is_flagged() {
        let rubric = rubric_with(BindingRole::Necessary, BindingMode::Blocking);
        let outcomes = [detector("C-01", "pass", &[], Some("toothless"))];
        let verdict = evaluate(
            &rubric,
            Some(&outcomes),
            Some("src/pay.rs"),
            None,
            &Ok(()),
            &std::collections::BTreeMap::new(),
        );
        assert!(matches!(
            &verdict.issues[0].kind,
            BindingIssueKind::Toothless { teeth } if teeth == "toothless"
        ));
        let confirmed = [detector("C-01", "pass", &[], Some("confirmed"))];
        let ok = evaluate(
            &rubric,
            Some(&confirmed),
            Some("src/pay.rs"),
            None,
            &Ok(()),
            &std::collections::BTreeMap::new(),
        );
        assert_eq!(ok.issues, []);
    }

    /// Неквалифицированный судья понижает зависящее от балла нарушение:
    /// противоречие с правилом — предупреждение, понижение — эскалация
    /// `binding_unqualified`.
    #[test]
    fn unqualified_judge_downgrades_judge_dependent_issues() {
        let rubric = rubric_with(BindingRole::Evidence, BindingMode::Blocking);
        let outcomes = [detector("C-01", "fail", &["src/pay.rs"], None)];
        let high = artifact_with_score(5);
        let verdict = evaluate(
            &rubric,
            Some(&outcomes),
            Some("src/pay.rs"),
            Some(&high),
            &Err("нет отчёта квалификации".to_string()),
            &std::collections::BTreeMap::new(),
        );
        let findings = verdict.findings("r", "s", HumanPolicy::Warn);
        let codes: Vec<(String, String)> = findings
            .into_iter()
            .map(|f| (f.rule.unwrap_or_default(), f.severity))
            .collect();
        assert_eq!(
            codes,
            vec![
                ("binding_unqualified".to_string(), "error".to_string()),
                ("binding_contradiction".to_string(), "warn".to_string()),
            ]
        );
        assert_eq!(verdict.escalations(HumanPolicy::Warn), 1);
    }

    /// Провал необходимого правила — измерение: допуск судьи на него не влияет.
    #[test]
    fn necessary_fail_does_not_depend_on_judge_qualification() {
        let rubric = rubric_with(BindingRole::Necessary, BindingMode::Blocking);
        let outcomes = [detector("C-01", "fail", &["src/pay.rs"], Some("confirmed"))];
        let verdict = evaluate(
            &rubric,
            Some(&outcomes),
            Some("src/pay.rs"),
            Some(&artifact_with_score(5)),
            &Err("нет отчёта квалификации".to_string()),
            &std::collections::BTreeMap::new(),
        );
        assert!(verdict.decided_fail(), "{verdict:?}");
    }

    /// Неподписанная блокирующая связка работает как советующая: провал
    /// правила — предупреждение, понижение — находка `binding_unsigned`.
    #[test]
    fn unsigned_blocking_binding_is_advisory() {
        let rubric = rubric_with(BindingRole::Necessary, BindingMode::Blocking);
        let outcomes = [detector("C-01", "fail", &["src/pay.rs"], Some("confirmed"))];
        let mut unsigned = std::collections::BTreeMap::new();
        unsigned.insert("no_violation".to_string(), "подписи нет".to_string());
        let verdict = evaluate(
            &rubric,
            Some(&outcomes),
            Some("src/pay.rs"),
            None,
            &Ok(()),
            &unsigned,
        );
        assert!(!verdict.decided_fail(), "{verdict:?}");
        let codes: Vec<(String, String)> = verdict
            .findings("r", "s", HumanPolicy::Human)
            .into_iter()
            .map(|f| (f.rule.unwrap_or_default(), f.severity))
            .collect();
        assert_eq!(
            codes,
            vec![
                ("binding_unsigned".to_string(), "warn".to_string()),
                ("binding_rule_failed".to_string(), "warn".to_string()),
            ]
        );
        assert_eq!(verdict.escalations(HumanPolicy::Human), 0);
    }
}
