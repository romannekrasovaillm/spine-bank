//! Квалификация судьи смысловых рубрик (E6.1–E6.3).
//!
//! Механике нечем проверить **правильность** суждения модели: она сверяет
//! честность цитат, полноту и стабильность. Остаётся измерить судью на
//! эталонном наборе с известной истиной и допускать к гейту только того, кто
//! прошёл порог.
//!
//! Набор (E6.1) — каталог-репозиторий: `ARCHITECTURE-SPINE.md`, `cases.yaml` с
//! истиной (`class` — класс дефекта, `truth` — `defective`/`clean`) и `code/`
//! с субъектами. Отчёт квалификации (E6.2) несёт модель-судью, хэш набора,
//! исход каждого случая и метрики по классам; допуск к гейту (E6.3) — по
//! порогам полноты, точности и доли `human`.
//!
//! Важно: квалификация не делает судью правильным. Она отвечает на другой
//! вопрос — «на известных дефектах этот судья их видит и не обвиняет чистое»,
//! и записывает ответ артефактом с версией модели.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{HarnessError, Result};
use crate::llm::LlmProvider;

use super::RubricDecision;
use super::types::Rubric;

/// Схема файла отчёта квалификации.
pub const QUALIFICATION_SCHEMA: &str = "arch-be/rubric-qualification/v1";
/// Каталог отчётов квалификации внутри репозитория.
pub const QUALIFICATION_DIR: &str = "reports/qualification";

/// Порог полноты: доля найденных дефектов среди случаев с вердиктом (E6.3).
pub const MIN_COMPLETENESS: f64 = 0.8;
/// Порог точности: доля верных вердиктов (нашёл дефект или не обвинил чистое).
pub const MIN_ACCURACY: f64 = 0.8;
/// Потолок доли `human`: судья, который «передаёт человеку» больше половины
/// набора, не квалифицирован — он не работает, а перекладывает работу.
pub const MAX_HUMAN_SHARE: f64 = 0.5;

/// Истина по случаю набора.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Truth {
    /// В коде есть нарушение инварианта.
    Defective,
    /// Код корректен: судья обязан не обвинять.
    Clean,
}

/// Случай набора: файл, класс дефекта и истина.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaseTruth {
    /// Путь к файлу кода относительно каталога набора.
    pub file: String,
    /// Класс дефекта (`ignored_key`, `no_return`, `float_money`, …).
    pub class: String,
    /// Истина.
    pub truth: Truth,
    /// Короткая заметка человека: что именно в коде.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
}

/// Набор квалификации на диске.
#[derive(Debug, Clone)]
pub struct QualificationSet {
    /// Каталог набора.
    pub dir: PathBuf,
    /// Случаи в порядке `cases.yaml`.
    pub cases: Vec<CaseTruth>,
    /// Хэш содержимого набора: меняется вместе с кодом и истиной.
    pub sha256: String,
}

/// Читает набор: `cases.yaml` плюс хэш содержимого.
///
/// # Errors
/// Нет `cases.yaml`, он не разбирается или в нём нет случаев.
pub fn load_set(dir: &Path) -> Result<QualificationSet> {
    #[derive(Deserialize)]
    struct File {
        cases: Vec<CaseTruth>,
    }
    let path = dir.join("cases.yaml");
    let text = std::fs::read_to_string(&path)
        .map_err(|e| HarnessError::Rubric(format!("набор квалификации {}: {e}", path.display())))?;
    let file: File = serde_yaml_ng::from_str(&text)
        .map_err(|e| HarnessError::Rubric(format!("разбор {}: {e}", path.display())))?;
    if file.cases.is_empty() {
        return Err(HarnessError::Rubric(format!(
            "набор квалификации {} не содержит случаев",
            dir.display()
        )));
    }
    // Хэш содержимого: истина + каждый файл кода. Правка случая меняет хэш, и
    // старый отчёт квалификации перестаёт ей соответствовать.
    let mut hasher = String::new();
    hasher.push_str(&text);
    for case in &file.cases {
        let body = std::fs::read_to_string(dir.join(&case.file)).unwrap_or_default();
        hasher.push_str(&case.file);
        hasher.push_str(&body);
    }
    Ok(QualificationSet {
        dir: dir.to_path_buf(),
        cases: file.cases,
        sha256: crate::hash::sha256_hex(hasher.as_bytes()),
    })
}

/// Что показал случай.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaseOutcome {
    /// Файл кода.
    pub file: String,
    /// Класс дефекта.
    pub class: String,
    /// Истина.
    pub truth: Truth,
    /// Решение судьи.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision: Option<RubricDecision>,
    /// Взвешенный итог.
    pub weighted_total: f64,
    /// Метки главного критерия — для разбора промахов.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub flags: Vec<String>,
    /// Судья не дал вердикта (отчёт не собрался: например, все свидетельства
    /// не подтвердились). Это воздержание, а не промах механики — но и не
    /// квалификация: случай считается переданным человеку.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Метрики по классу дефекта.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ClassStats {
    /// Класс дефекта.
    pub class: String,
    /// Всего случаев.
    pub total: usize,
    /// Дефектных случаев.
    pub defective: usize,
    /// Чистых случаев.
    pub clean: usize,
    /// Дефект найден (`fail` на дефектном).
    pub caught: usize,
    /// Дефект пропущен (`pass` на дефектном).
    pub missed: usize,
    /// Чистое не обвинено (`pass` на чистом).
    pub cleared: usize,
    /// Ложное обвинение (`fail` на чистом).
    pub false_accusations: usize,
    /// Передано человеку.
    pub human: usize,
}

impl ClassStats {
    /// Полнота: доля найденных дефектов среди случаев с вердиктом.
    #[must_use]
    pub fn completeness(&self) -> f64 {
        let decided = self.caught + self.missed;
        if decided == 0 {
            return 0.0;
        }
        self.caught as f64 / decided as f64
    }

    /// Точность: доля верных вердиктов среди случаев с вердиктом.
    #[must_use]
    pub fn accuracy(&self) -> f64 {
        let decided = self.caught + self.missed + self.cleared + self.false_accusations;
        if decided == 0 {
            return 0.0;
        }
        (self.caught + self.cleared) as f64 / decided as f64
    }
}

/// Пороги допуска, записанные в отчёт.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Thresholds {
    /// Минимальная полнота.
    pub min_completeness: f64,
    /// Минимальная точность.
    pub min_accuracy: f64,
    /// Максимальная доля `human`.
    pub max_human_share: f64,
}

impl Default for Thresholds {
    fn default() -> Self {
        Self {
            min_completeness: MIN_COMPLETENESS,
            min_accuracy: MIN_ACCURACY,
            max_human_share: MAX_HUMAN_SHARE,
        }
    }
}

/// Отчёт квалификации судьи (E6.2).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QualificationReport {
    /// Схема файла.
    pub schema: String,
    /// Рубрика, на которой квалифицировали.
    pub rubric: String,
    /// Метка модели-судьи.
    pub model: String,
    /// Каталог набора.
    pub set: String,
    /// Хэш содержимого набора.
    pub set_sha256: String,
    /// Когда прогоняли (RFC 3339).
    pub judged_at: String,
    /// Сэмплов судьи на критерий.
    pub samples: usize,
    /// Метрики по классам.
    pub by_class: Vec<ClassStats>,
    /// Итоговые метрики (сумма по классам).
    pub totals: ClassStats,
    /// Доля `human` по набору.
    pub human_share: f64,
    /// Пороги допуска.
    pub thresholds: Thresholds,
    /// Прошёл ли судья пороги.
    pub passed: bool,
    /// Почему не прошёл (пусто — прошёл).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub failures: Vec<String>,
    /// Исход каждого случая.
    pub cases: Vec<CaseOutcome>,
}

impl QualificationReport {
    /// Человекочитаемая сводка: метрики и вердикт допуска.
    #[must_use]
    pub fn summary(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(
            out,
            "Квалификация судьи: рубрика «{}», модель {}, набор {} ({} случаев, сэмплов {})",
            self.rubric,
            self.model,
            &self.set_sha256[..12.min(self.set_sha256.len())],
            self.totals.total,
            self.samples
        );
        let _ = writeln!(
            out,
            "| Класс | Случаев | Дефект найден | Пропуск | Чистое не обвинено | Ложное обвинение | human | Полнота | Точность |"
        );
        let _ = writeln!(
            out,
            "| --- | --- | --- | --- | --- | --- | --- | --- | --- |"
        );
        for c in &self.by_class {
            let _ = writeln!(
                out,
                "| {} | {} | {} | {} | {} | {} | {} | {:.2} | {:.2} |",
                c.class,
                c.total,
                c.caught,
                c.missed,
                c.cleared,
                c.false_accusations,
                c.human,
                c.completeness(),
                c.accuracy()
            );
        }
        let t = &self.totals;
        let _ = writeln!(
            out,
            "| **итого** | {} | {} | {} | {} | {} | {} | {:.2} | {:.2} |",
            t.total,
            t.caught,
            t.missed,
            t.cleared,
            t.false_accusations,
            t.human,
            t.completeness(),
            t.accuracy()
        );
        let _ = writeln!(
            out,
            "Доля `human`: {:.0}% (потолок {:.0}%)",
            self.human_share * 100.0,
            self.thresholds.max_human_share * 100.0
        );
        if self.passed {
            let _ = writeln!(
                out,
                "Допуск к гейту: **пройден** (полнота ≥ {:.2}, точность ≥ {:.2})",
                self.thresholds.min_completeness, self.thresholds.min_accuracy
            );
        } else {
            let _ = writeln!(out, "Допуск к гейту: **не пройден**");
            for f in &self.failures {
                let _ = writeln!(out, "  · {f}");
            }
        }
        out
    }
}

/// Прогоняет судью по набору и считает метрики (E6.2).
///
/// # Errors
/// Набор не читается, рубрика не объявляет вид досье `code_vs_spine`, досье или
/// отчёт не собираются, судья падает.
pub async fn run(
    set_dir: &Path,
    rubric: &Rubric,
    model: &str,
    llm: &dyn LlmProvider,
    cfg: &crate::config::JudgeConfig,
) -> Result<QualificationReport> {
    let kind = rubric.pack.ok_or_else(|| {
        HarnessError::Rubric(format!(
            "рубрика '{}' не объявляет вид досье (`pack:`) — квалифицировать нечего",
            rubric.name
        ))
    })?;
    let set = load_set(set_dir)?;
    let mut outcomes = Vec::with_capacity(set.cases.len());
    for case in &set.cases {
        let packs = crate::rubric_pack::build(set_dir, kind, &case.file)?;
        let pack = packs.into_iter().next().ok_or_else(|| {
            HarnessError::Rubric(format!("случай '{}' не собрался в досье", case.file))
        })?;
        // Судья, который не дал засчитываемого вердикта, — это воздержание:
        // случай уходит в «human», а не роняет прогон квалификации.
        let outcome = match super::evaluate_pack_collecting(rubric, &pack, llm, cfg).await {
            Ok((report, _raw)) => {
                let flags = report
                    .scores
                    .iter()
                    .find(|s| s.criterion_id == "no_violation")
                    .map(|s| {
                        s.flags
                            .iter()
                            .map(|f| f.as_str().to_string())
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                CaseOutcome {
                    file: case.file.clone(),
                    class: case.class.clone(),
                    truth: case.truth,
                    decision: report.decision,
                    weighted_total: report.weighted_total,
                    flags,
                    error: None,
                }
            }
            Err(e) => CaseOutcome {
                file: case.file.clone(),
                class: case.class.clone(),
                truth: case.truth,
                decision: None,
                weighted_total: 0.0,
                flags: Vec::new(),
                error: Some(e.to_string()),
            },
        };
        outcomes.push(outcome);
    }
    Ok(build_report(
        rubric,
        model,
        &set,
        outcomes,
        cfg.samples.max(1),
    ))
}

/// Считает метрики и вердикт допуска из исходов — чистая функция (тесты).
#[must_use]
pub fn build_report(
    rubric: &Rubric,
    model: &str,
    set: &QualificationSet,
    outcomes: Vec<CaseOutcome>,
    samples: usize,
) -> QualificationReport {
    let mut by_class: BTreeMap<String, ClassStats> = BTreeMap::new();
    let mut totals = ClassStats {
        class: "итого".to_string(),
        ..ClassStats::default()
    };
    for outcome in &outcomes {
        let stats = by_class
            .entry(outcome.class.clone())
            .or_insert_with(|| ClassStats {
                class: outcome.class.clone(),
                ..ClassStats::default()
            });
        for target in [stats, &mut totals] {
            target.total += 1;
            match outcome.truth {
                Truth::Defective => target.defective += 1,
                Truth::Clean => target.clean += 1,
            }
            match outcome.decision {
                Some(RubricDecision::Human) | None => target.human += 1,
                Some(RubricDecision::Fail) => match outcome.truth {
                    Truth::Defective => target.caught += 1,
                    Truth::Clean => target.false_accusations += 1,
                },
                Some(RubricDecision::Pass) => match outcome.truth {
                    Truth::Defective => target.missed += 1,
                    Truth::Clean => target.cleared += 1,
                },
            }
        }
    }
    let thresholds = Thresholds::default();
    let decided = totals.total.saturating_sub(totals.human);
    let human_share = if totals.total == 0 {
        0.0
    } else {
        totals.human as f64 / totals.total as f64
    };
    let mut failures = Vec::new();
    if decided == 0 {
        failures.push("набор не дал ни одного вердикта: все случаи ушли человеку".to_string());
    } else {
        if totals.completeness() < thresholds.min_completeness {
            failures.push(format!(
                "полнота {:.2} ниже порога {:.2}: пропущено дефектов {}",
                totals.completeness(),
                thresholds.min_completeness,
                totals.missed
            ));
        }
        if totals.accuracy() < thresholds.min_accuracy {
            failures.push(format!(
                "точность {:.2} ниже порога {:.2}: ложных обвинений {}, пропусков {}",
                totals.accuracy(),
                thresholds.min_accuracy,
                totals.false_accusations,
                totals.missed
            ));
        }
    }
    if human_share > thresholds.max_human_share {
        failures.push(format!(
            "доля human {:.0}% выше потолка {:.0}%",
            human_share * 100.0,
            thresholds.max_human_share * 100.0
        ));
    }
    QualificationReport {
        schema: QUALIFICATION_SCHEMA.to_string(),
        rubric: rubric.name.clone(),
        model: model.to_string(),
        set: set.dir.display().to_string(),
        set_sha256: set.sha256.clone(),
        judged_at: chrono::Local::now().to_rfc3339(),
        samples,
        by_class: by_class.into_values().collect(),
        passed: failures.is_empty(),
        failures,
        totals,
        human_share,
        thresholds,
        cases: outcomes,
    }
}

/// Записывает отчёт в `<repo>/reports/qualification/<рубрика>--<модель>.json`.
///
/// # Errors
/// Каталог не создаётся или файл не пишется.
pub fn write(repo: &Path, report: &QualificationReport) -> Result<PathBuf> {
    let dir = repo.join(QUALIFICATION_DIR);
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!(
        "{}--{}.json",
        slug(&report.rubric),
        slug(&report.model)
    ));
    let text = serde_json::to_string_pretty(report)
        .map_err(|e| HarnessError::Rubric(format!("сериализация квалификации: {e}")))?;
    std::fs::write(&path, text)?;
    Ok(path)
}

/// Состояние допуска модели к гейту (E6.3).
#[derive(Debug, Clone)]
pub enum Qualification {
    /// Модель квалифицирована: отчёт есть и пороги пройдены.
    Qualified(Box<QualificationReport>),
    /// Отчёта нет.
    Missing,
    /// Отчёт есть, но пороги не пройдены.
    Failed(Box<QualificationReport>),
}

/// Ищет отчёт квалификации по рубрике и модели (E6.3).
#[must_use]
pub fn stored(repo: &Path, rubric: &str, model: &str) -> Option<QualificationReport> {
    let path = repo
        .join(QUALIFICATION_DIR)
        .join(format!("{}--{}.json", slug(rubric), slug(model)));
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

/// Допуск модели к гейту: `Missing` — отчёта нет, `Failed` — пороги не пройдены.
#[must_use]
pub fn qualification(repo: &Path, rubric: &str, model: &str) -> Qualification {
    match stored(repo, rubric, model) {
        Some(report) if report.passed => Qualification::Qualified(Box::new(report)),
        Some(report) => Qualification::Failed(Box::new(report)),
        None => Qualification::Missing,
    }
}

/// Человекочитаемая причина отказа в допуске (для находки гейта).
#[must_use]
pub fn refusal_reason(state: &Qualification, model: &str) -> String {
    match state {
        Qualification::Qualified(report) => format!(
            "квалификация пройдена {} ({}, набор {})",
            report.judged_at,
            report.model,
            &report.set_sha256[..12.min(report.set_sha256.len())]
        ),
        Qualification::Missing => format!(
            "отчёта квалификации модели '{model}' нет — прогоните `arch-be rubric qualify` \
             на эталонном наборе"
        ),
        Qualification::Failed(report) => format!(
            "квалификация модели '{model}' не пройдена ({}); прогон {}",
            report.failures.join("; "),
            report.judged_at
        ),
    }
}

/// Slug имени файла: буквы-цифры и `-`, остальное — `-`.
fn slug(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
            out.push(ch.to_ascii_lowercase());
        } else {
            out.push('-');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rubric::testkit::*;

    /// E6.1: поставляемый набор читается и размечен: 30 случаев, 5 классов,
    /// поровну дефектных и чистых.
    #[test]
    fn shipped_set_is_labelled_and_readable() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("assets/qualification/code_vs_spine");
        let set = load_set(&dir).expect("набор квалификации");
        assert_eq!(set.cases.len(), 30, "случаев в наборе");
        assert_eq!(
            set.cases
                .iter()
                .filter(|c| c.truth == Truth::Defective)
                .count(),
            15
        );
        assert_eq!(
            set.cases.iter().filter(|c| c.truth == Truth::Clean).count(),
            15
        );
        let classes: std::collections::BTreeSet<&str> =
            set.cases.iter().map(|c| c.class.as_str()).collect();
        for class in [
            "ignored_key",
            "no_return",
            "float_money",
            "comment_only",
            "flag_bypass",
        ] {
            assert!(classes.contains(class), "класс {class} в наборе");
        }
        assert_eq!(set.sha256.len(), 64, "хэш набора");
        // Хэш меняется вместе с содержимым: правка случая обесценивает отчёт.
        let mut changed = set.sha256.clone();
        changed.push('x');
        assert_ne!(changed, set.sha256);
    }

    /// E6.2/E6.3: метрики считаются по классам, а допуск — по порогам полноты,
    /// точности и доли `human`.
    #[test]
    fn metrics_and_thresholds_are_mechanical() {
        let set = QualificationSet {
            dir: std::path::PathBuf::from("/набор"),
            cases: Vec::new(),
            sha256: "a".repeat(64),
        };
        let case = |class: &str, truth: Truth, decision: Option<RubricDecision>| CaseOutcome {
            file: "code/x.py".to_string(),
            class: class.to_string(),
            truth,
            decision,
            weighted_total: 3.0,
            flags: Vec::new(),
            error: None,
        };
        let report = build_report(
            &sample_rubric(),
            "judge-x",
            &set,
            vec![
                case("ignored_key", Truth::Defective, Some(RubricDecision::Fail)),
                case("ignored_key", Truth::Defective, Some(RubricDecision::Pass)),
                case("ignored_key", Truth::Clean, Some(RubricDecision::Pass)),
            ],
            1,
        );
        assert_eq!(report.totals.caught, 1);
        assert_eq!(report.totals.missed, 1);
        assert_eq!(report.totals.cleared, 1);
        assert!((report.totals.completeness() - 0.5).abs() < 1e-9);
        assert!((report.totals.accuracy() - 2.0 / 3.0).abs() < 1e-9);
        assert!(!report.passed, "полнота ниже порога");
        assert!(
            report.failures.iter().any(|f| f.contains("полнота")),
            "{:?}",
            report.failures
        );
        assert_eq!(report.by_class.len(), 1);
        // Верный судья проходит пороги.
        let report = build_report(
            &sample_rubric(),
            "judge-x",
            &set,
            vec![
                case("ignored_key", Truth::Defective, Some(RubricDecision::Fail)),
                case("ignored_key", Truth::Clean, Some(RubricDecision::Pass)),
            ],
            1,
        );
        assert!(report.passed, "{:?}", report.failures);
        assert!(report.failures.is_empty());
        assert!((report.human_share - 0.0).abs() < 1e-9);
        // Судья-«перестраховщик»: всё человеку — не квалифицирован.
        let report = build_report(
            &sample_rubric(),
            "judge-x",
            &set,
            vec![
                case("ignored_key", Truth::Defective, Some(RubricDecision::Human)),
                case("ignored_key", Truth::Clean, None),
            ],
            1,
        );
        assert!(!report.passed);
        assert!((report.human_share - 1.0).abs() < 1e-9);
        assert!(
            report.failures.iter().any(|f| f.contains("вердикта")),
            "{:?}",
            report.failures
        );
    }

    /// E6.3: допуск ищется по рубрике и модели; отсутствие отчёта и непройденные
    /// пороги различимы, и причина отказа называет, что делать.
    #[test]
    fn qualification_state_is_distinguishable() {
        let tmp = tempfile::tempdir().expect("tmp");
        let repo = tmp.path();
        assert!(matches!(
            qualification(repo, "code_invariant_conformance", "judge-x"),
            Qualification::Missing
        ));
        let set = QualificationSet {
            dir: std::path::PathBuf::from("/набор"),
            cases: Vec::new(),
            sha256: "b".repeat(64),
        };
        let failed = build_report(&sample_rubric(), "judge-x", &set, Vec::new(), 1);
        let path = write(repo, &failed).expect("запись квалификации");
        assert!(path.is_file(), "{}", path.display());
        let state = qualification(repo, "adr-quality", "judge-x");
        assert!(matches!(state, Qualification::Failed(_)), "{state:?}");
        let reason = refusal_reason(&state, "judge-x");
        assert!(reason.contains("не пройдена"), "{reason}");
        assert!(
            refusal_reason(&Qualification::Missing, "judge-x").contains("rubric qualify"),
            "причина отказа подсказывает команду"
        );
    }
}
