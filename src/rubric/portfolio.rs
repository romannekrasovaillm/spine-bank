//! E10.3/E10.4: смысловой срез по портфелю продуктов и пакет для архкомитета.
//!
//! Решение гейта отвечает на вопрос про один продукт; на уровне ДКА нужно
//! другое: сколько решений ушло человеку, где судьи разошлись и какие инварианты
//! нарушаются чаще всего. Модуль читает машиночитаемые отчёты рубрик
//! (`reports/rubric/*.json`) по списку корней продуктов и собирает:
//!
//! - **срез** ([`PortfolioReport`]) — решения по типам и доля `human`,
//!   расхождения двух судей (E5.1), нарушенные инварианты и отказы детекторов;
//! - **пакет комитета** ([`PortfolioReport::committee_markdown`]) — только
//!   `fail` и `human` с доказательствами: цитатами и указателями на источники
//!   (E9.1), причинами решения и ссылками на отчёты. `pass` в пакет не
//!   попадает: комитет разбирает спорное, а не подтверждённое.
//!
//! Источник данных — отчёты, а не живой прогон: срез строится по тому, что уже
//! оценено, и ничего не вызывает сам.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::rubric::{CriterionFlag, RubricArtifact, RubricDecision};

/// Потолок отчётов, читаемых с одного продукта (защита от гигантских каталогов).
const MAX_REPORTS_PER_PRODUCT: usize = 500;

/// Балл, при котором блокирующий критерий считается нарушением.
const VIOLATION_MAX_SCORE: u8 = 2;

/// Один разобранный отчёт: что решено, где и с какими доказательствами.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ReviewSummary {
    /// Метка продукта (имя каталога или заданное имя).
    pub product: String,
    /// Имя рубрики.
    pub rubric: String,
    /// Субъект оценки (путь документа или досье).
    pub subject: String,
    /// Решение (`pass` / `fail` / `human`; `—` — отчёт старой схемы).
    pub decision: String,
    /// Взвешенный итог.
    pub weighted_total: f64,
    /// Модель-судья.
    pub judge_model: String,
    /// Путь отчёта относительно продукта.
    pub report: String,
    /// Почему решение такое (E4.4).
    pub reasons: Vec<String>,
    /// Подтверждённые нарушения: критерии с низким баллом в отчёте, решение
    /// которого `fail` (решение `fail` механика выносит только по
    /// подтверждённым блокирующим нарушениям, E4.1).
    pub violated: Vec<String>,
    /// Возможные нарушения: низкий балл при решении `human` — обвинение, которое
    /// механика не подтвердила. Комитету показываются отдельно от
    /// подтверждённых.
    pub suspected: Vec<String>,
    /// Красные детекторы досье (`fitness`).
    pub detector_failures: Vec<String>,
    /// Расхождение второго судьи (E5.1), если было.
    pub disagreement: Option<String>,
    /// Доказательства для комитета: подтверждённые цитаты и указатели (E9.1).
    pub evidence: Vec<String>,
}

impl ReviewSummary {
    /// Отчёт спорный: решение `fail` или `human`. Только такие попадают в
    /// пакет комитета (E10.4).
    #[must_use]
    pub fn is_contested(&self) -> bool {
        matches!(self.decision.as_str(), "fail" | "human")
    }

    /// Строка места: `продукт · рубрика · субъект`.
    #[must_use]
    pub fn place(&self) -> String {
        format!("{} · {} · {}", self.product, self.rubric, self.subject)
    }
}

/// Смысловой срез по портфелю (E10.3).
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct PortfolioReport {
    /// Продукты (метки) в порядке входа.
    pub products: Vec<String>,
    /// Разобранных отчётов (вторые судьи не считаются).
    pub reports: usize,
    /// Отчётов без решения (схема до 0.3.9).
    pub undecided: usize,
    /// Решений `pass`.
    pub pass: usize,
    /// Решений `fail`.
    pub fail: usize,
    /// Решений `human`.
    pub human: usize,
    /// Доля `human` среди решённых (0..1).
    pub human_share: f64,
    /// Расхождения судей (E5.1): `продукт · субъект — различия`.
    pub disagreements: Vec<String>,
    /// Нарушенные инварианты (критерии `fail` + красные детекторы) со счётчиком.
    pub violated_invariants: BTreeMap<String, usize>,
    /// Спорные отчёты (`fail` / `human`) — основа пакета комитета.
    pub contested: Vec<ReviewSummary>,
    /// Все разобранные отчёты (для машинного чтения среза).
    pub reviews: Vec<ReviewSummary>,
}

impl PortfolioReport {
    /// Доля `human` в процентах (для отчёта ДКА).
    #[must_use]
    pub fn human_pct(&self) -> f64 {
        self.human_share * 100.0
    }

    /// Срез в markdown: решения, доля `human`, расхождения судей, нарушенные
    /// инварианты (E10.3).
    #[must_use]
    pub fn render_markdown(&self) -> String {
        use std::fmt::Write as _;
        let mut out = String::from("## Смысловой срез портфеля (E10.3)\n\n");
        let _ = writeln!(
            out,
            "- Продуктов: **{}**, отчётов рубрик: **{}** (без решения: {})",
            self.products.len(),
            self.reports,
            self.undecided
        );
        let _ = writeln!(
            out,
            "- Решения: **pass {}**, **fail {}**, **human {}** — доля `human` **{:.0}%**",
            self.pass,
            self.fail,
            self.human,
            self.human_pct()
        );
        if !self.violated_invariants.is_empty() {
            let list = self
                .violated_invariants
                .iter()
                .map(|(k, v)| format!("{k} ×{v}"))
                .collect::<Vec<_>>()
                .join(", ");
            let _ = writeln!(out, "- Нарушенные инварианты и отказы детекторов: {list}");
        }
        if self.disagreements.is_empty() {
            let _ = writeln!(out, "- Расхождений судей нет.");
        } else {
            let _ = writeln!(out, "- Расхождения судей (E5.1):");
            for d in &self.disagreements {
                let _ = writeln!(out, "  - {d}");
            }
        }
        let _ = writeln!(
            out,
            "- Спорных отчётов для комитета (`fail`/`human`): **{}**",
            self.contested.len()
        );
        out
    }

    /// Пакет для архкомитета (E10.4): только `fail` и `human`, каждый — с
    /// причинами решения и доказательствами (цитаты и указатели на источники).
    /// `pass` в пакет не входит.
    #[must_use]
    pub fn committee_markdown(&self) -> String {
        use std::fmt::Write as _;
        let mut out = String::from("# Пакет для архитектурного комитета (E10.4)\n\n");
        let _ = writeln!(
            out,
            "В пакет входят только решения `fail` и `human` — то, что механика не \
             закрыла сама. Подтверждённое (`pass`) комитету не показывается.\n"
        );
        let _ = writeln!(
            out,
            "Срез: продуктов {}, отчётов {}, из них спорных {} (fail {}, human {}), \
             доля `human` {:.0}%.\n",
            self.products.len(),
            self.reports,
            self.contested.len(),
            self.fail,
            self.human,
            self.human_pct()
        );
        if self.contested.is_empty() {
            let _ = writeln!(out, "Спорных отчётов нет: комитету нечего разбирать.");
            return out;
        }
        let _ = writeln!(
            out,
            "| Продукт | Рубрика | Субъект | Решение | Итог | Отчёт |"
        );
        let _ = writeln!(out, "| --- | --- | --- | --- | --- | --- |");
        for r in &self.contested {
            let _ = writeln!(
                out,
                "| {} | {} | {} | {} | {:.2} | `{}` |",
                r.product, r.rubric, r.subject, r.decision, r.weighted_total, r.report
            );
        }
        for r in &self.contested {
            let _ = writeln!(out, "\n### {}\n", r.place());
            let _ = writeln!(
                out,
                "- Решение: **{}** (итог {:.2}, судья `{}`, отчёт `{}`)",
                r.decision, r.weighted_total, r.judge_model, r.report
            );
            for reason in &r.reasons {
                let _ = writeln!(out, "- Причина: {reason}");
            }
            if !r.violated.is_empty() {
                let _ = writeln!(out, "- Подтверждённые нарушения: {}", r.violated.join(", "));
            }
            if !r.suspected.is_empty() {
                let _ = writeln!(
                    out,
                    "- Возможные нарушения (механика не подтвердила): {}",
                    r.suspected.join(", ")
                );
            }
            if !r.detector_failures.is_empty() {
                let _ = writeln!(
                    out,
                    "- Красные детекторы: {}",
                    r.detector_failures.join(", ")
                );
            }
            if let Some(d) = &r.disagreement {
                let _ = writeln!(out, "- Расхождение судей: {d}");
            }
            if r.evidence.is_empty() {
                let _ = writeln!(out, "- Доказательств в отчёте нет — разбирать по отчёту.");
            } else {
                let _ = writeln!(out, "- Доказательства:");
                for e in &r.evidence {
                    let _ = writeln!(out, "  - {e}");
                }
            }
        }
        out
    }
}

/// Собирает срез по продуктам: `(метка, корень)`.
///
/// Читаются `reports/rubric/*.json` каждого продукта; отчёты второго судьи
/// (`judge_role = "second"`) пропускаются — их вердикт живёт в сводке первого.
#[must_use]
pub fn collect(products: &[(String, PathBuf)]) -> PortfolioReport {
    let mut report = PortfolioReport::default();
    for (label, root) in products {
        report.products.push(label.clone());
        let dir = root.join("reports").join("rubric");
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut paths: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.is_file()
                    && p.extension()
                        .is_some_and(|ext| ext.eq_ignore_ascii_case("json"))
            })
            .collect();
        paths.sort();
        for path in paths.into_iter().take(MAX_REPORTS_PER_PRODUCT) {
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            let Ok(artifact) = serde_json::from_str::<RubricArtifact>(&text) else {
                continue;
            };
            if artifact.judge_role.as_deref() == Some("second") {
                continue;
            }
            report.reports += 1;
            let summary = summarize(label, root, &path, &artifact);
            match summary.decision.as_str() {
                "pass" => report.pass += 1,
                "fail" => report.fail += 1,
                "human" => report.human += 1,
                _ => report.undecided += 1,
            }
            for v in summary
                .violated
                .iter()
                .chain(summary.suspected.iter())
                .chain(summary.detector_failures.iter())
            {
                *report.violated_invariants.entry(v.clone()).or_insert(0) += 1;
            }
            if let Some(d) = &summary.disagreement {
                report
                    .disagreements
                    .push(format!("{} — {d}", summary.place()));
            }
            if summary.is_contested() {
                report.contested.push(summary.clone());
            }
            report.reviews.push(summary);
        }
    }
    let decided = report.pass + report.fail + report.human;
    report.human_share = if decided == 0 {
        0.0
    } else {
        report.human as f64 / decided as f64
    };
    report
}

/// Разбирает один артефакт в сводку: решение, нарушения, доказательства.
fn summarize(product: &str, root: &Path, path: &Path, artifact: &RubricArtifact) -> ReviewSummary {
    let decision = artifact
        .decision
        .map_or_else(|| "—".to_string(), |d| d.as_str().to_string());
    let subject = artifact
        .subject
        .clone()
        .or_else(|| artifact.target.clone())
        .unwrap_or_else(|| "—".to_string());
    // Подтверждённое нарушение — низкий балл в отчёте, решение которого
    // `fail`: такое решение механика выносит только по подтверждённым
    // блокирующим нарушениям (E4.1). Низкий балл при `human` — обвинение без
    // подтверждения: комитету оно нужно, но называется отдельно.
    let low: Vec<String> = artifact
        .scores
        .iter()
        .filter(|s| s.score <= VIOLATION_MAX_SCORE)
        .map(|s| s.criterion_id.clone())
        .collect();
    let (violated, suspected) = match artifact.decision {
        Some(RubricDecision::Fail) => (low, Vec::new()),
        // При `human` подозрением считается только критерий с метками: низкий
        // балл без метки — это «свидетельства нет» по контракту промпта, а не
        // обвинение, и в списке для комитета ему делать нечего.
        Some(RubricDecision::Human) => (
            Vec::new(),
            artifact
                .scores
                .iter()
                .filter(|s| s.score <= VIOLATION_MAX_SCORE && !s.flags.is_empty())
                .map(|s| s.criterion_id.clone())
                .collect(),
        ),
        _ => (Vec::new(), Vec::new()),
    };
    let detector_failures: Vec<String> = artifact
        .inputs
        .iter()
        .filter(|i| i.role == crate::rubric_pack::InputRole::Detector)
        .filter(|i| i.status.as_deref() == Some("fail"))
        .map(|i| i.key().to_string())
        .collect();
    let disagreement = artifact.second_judge.as_ref().and_then(|s| {
        (!s.agreement).then(|| {
            format!(
                "первый судья vs `{}` (итоги {:.2} / {:.2}): {}",
                s.model,
                artifact.weighted_total,
                s.weighted_total,
                s.differences.join("; ")
            )
        })
    });
    let mut reasons = artifact.decision_reasons.clone();
    if decision == "human" && reasons.is_empty() {
        // Отчёты без явных причин: решение объяснимо метками критериев.
        let flagged: Vec<String> = artifact
            .scores
            .iter()
            .filter(|s| !s.flags.is_empty())
            .map(|s| {
                format!(
                    "{}: {}",
                    s.criterion_id,
                    s.flags
                        .iter()
                        .map(CriterionFlag::as_str)
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            })
            .collect();
        if !flagged.is_empty() {
            reasons.push(format!("метки критериев: {}", flagged.join("; ")));
        }
    }
    let evidence = evidence_lines(artifact);
    ReviewSummary {
        product: product.to_string(),
        rubric: artifact.rubric.clone(),
        subject,
        decision,
        weighted_total: artifact.weighted_total,
        judge_model: artifact.judge_model.clone(),
        report: path
            .strip_prefix(root)
            .unwrap_or(path)
            .display()
            .to_string(),
        reasons,
        violated,
        suspected,
        detector_failures,
        disagreement,
        evidence,
    }
}

/// Доказательства отчёта для комитета: цитаты с указателями (E9.1) и цитаты
/// из обоснований. Строки читаются человеком, поэтому — с пометкой источника.
fn evidence_lines(artifact: &RubricArtifact) -> Vec<String> {
    let mut out = Vec::new();
    for score in &artifact.scores {
        for c in &score.citations {
            let mark = if c.confirmed { "✓" } else { "✗" };
            let note = c
                .note
                .as_deref()
                .map(|n| format!(" — {n}"))
                .unwrap_or_default();
            out.push(format!(
                "{} · {} `{}` — «{}» {}{}",
                score.criterion_id,
                if c.role.is_empty() { "?" } else { &c.role },
                c.source,
                c.quote,
                mark,
                note
            ));
        }
        if score.citations.is_empty() {
            for quote in quotes_in(&score.rationale) {
                out.push(format!("{} · цитата — «{}»", score.criterion_id, quote));
            }
        }
    }
    out.truncate(MAX_EVIDENCE_LINES);
    out
}

/// Цитаты из обоснования (`Цитата subject: "…"`, `Цитата: "…"`).
fn quotes_in(rationale: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = rationale;
    while let Some(pos) = rest.find('"') {
        let tail = &rest[pos + 1..];
        let Some(end) = tail.find('"') else { break };
        let quote = tail[..end].trim();
        if quote.len() >= 8 && !out.iter().any(|q: &String| q == quote) {
            out.push(quote.to_string());
        }
        rest = &tail[end + 1..];
    }
    out
}

/// Потолок строк доказательств в одной сводке (пакет читает человек).
const MAX_EVIDENCE_LINES: usize = 30;

/// Разбивает список корней на продукты: метка — имя каталога.
#[must_use]
pub fn products_from_roots(roots: &[PathBuf]) -> Vec<(String, PathBuf)> {
    roots
        .iter()
        .map(|root| {
            let label = root.file_name().map_or_else(
                || root.display().to_string(),
                |n| n.to_string_lossy().into_owned(),
            );
            (label, root.clone())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn artifact_json(rubric: &str, subject: &str, decision: &str, with_citation: bool) -> String {
        // У `human` критерий помечен меткой (обвинение без подтверждения) —
        // иначе это «свидетельства нет» и подозрением он не считается.
        let flags = if decision == "human" {
            r#"["accusation_unconfirmed"]"#
        } else {
            "[]"
        };
        let citations = if with_citation {
            r#"[{"role":"subject","source":"src/pay.py","quote":"self.charged.append(amount_minor)","confirmed":true}]"#
        } else {
            "[]"
        };
        format!(
            r#"{{
              "schema": "arch-rubric-artifact/1",
              "rubric": "{rubric}",
              "subject": "{subject}",
              "judge_model": "judge-1",
              "weighted_total": 2.0,
              "verdict": "тест",
              "decision": "{decision}",
              "decision_reasons": ["подтверждённое нарушение"],
              "scores": [{{
                "criterion_id": "no_violation",
                "weight": 3.0,
                "score": 1,
                "rationale": "Цитата subject: \"self.charged.append(amount_minor)\". Нарушение.",
                "samples": [1],
                "stdev": 0.0,
                "flags": {flags},
                "evidence_unconfirmed_ratio": 0.0,
                "invalid_samples": 0,
                "checked": [],
                "citations": {citations}
              }}],
              "inputs": [
                {{"path": "src/pay.py", "sha256": "aa", "role": "subject"}},
                {{"path": "reports/detectors/fitness.json", "sha256": "bb", "role": "detector", "status": "fail"}}
              ],
              "judged_at": "2026-09-25T12:00:00Z"
            }}"#
        )
    }

    fn product(dir: &Path, name: &str, reports: &[(&str, &str, &str)]) -> (String, PathBuf) {
        let root = dir.join(name);
        let rubric_dir = root.join("reports/rubric");
        std::fs::create_dir_all(&rubric_dir).expect("каталог отчётов");
        for (i, (rubric, subject, decision)) in reports.iter().enumerate() {
            std::fs::write(
                rubric_dir.join(format!("{}--code_vs_spine.json", subject.replace('/', "-"))),
                artifact_json(rubric, subject, decision, true),
            )
            .expect("отчёт");
            let _ = i;
        }
        (name.to_string(), root)
    }

    #[test]
    fn slice_counts_decisions_human_share_and_violations() {
        let tmp = tempfile::tempdir().expect("tmp");
        let a = product(
            tmp.path(),
            "product-a",
            &[
                ("code_invariant_conformance", "src/a", "fail"),
                ("code_invariant_conformance", "src/b", "human"),
                ("adr_quality", "docs/adr/1", "pass"),
            ],
        );
        let b = product(
            tmp.path(),
            "product-b",
            &[("code_invariant_conformance", "src/c", "pass")],
        );
        let report = collect(&[a, b]);
        assert_eq!(report.products, vec!["product-a", "product-b"]);
        assert_eq!(report.reports, 4);
        assert_eq!((report.pass, report.fail, report.human), (2, 1, 1));
        assert!(
            (report.human_share - 0.25).abs() < 1e-9,
            "{}",
            report.human_share
        );
        assert_eq!(report.contested.len(), 2, "в комитет — fail и human");
        // Низкий балл считается нарушением только там, где решение это
        // подтверждает: у `fail` — подтверждённое нарушение, у `human` —
        // возможное. Отчёты `pass` в счёт не идут, даже если балл низкий
        // (значит, критерий не блокирующий).
        assert_eq!(
            report.violated_invariants.get("no_violation"),
            Some(&2),
            "подтверждённое + возможное: {:?}",
            report.violated_invariants
        );
        assert_eq!(
            report
                .violated_invariants
                .get("reports/detectors/fitness.json"),
            Some(&4)
        );
        let md = report.render_markdown();
        assert!(md.contains("Смысловой срез портфеля (E10.3)"), "{md}");
        assert!(md.contains("доля `human` **25%**"), "{md}");
    }

    #[test]
    fn committee_package_contains_only_contested_with_evidence() {
        let tmp = tempfile::tempdir().expect("tmp");
        let a = product(
            tmp.path(),
            "product-a",
            &[
                ("code_invariant_conformance", "src/a", "fail"),
                ("code_invariant_conformance", "src/b", "pass"),
            ],
        );
        let report = collect(&[a]);
        let md = report.committee_markdown();
        assert!(
            md.contains("Пакет для архитектурного комитета (E10.4)"),
            "{md}"
        );
        assert!(md.contains("только решения `fail` и `human`"), "{md}");
        assert!(md.contains("src/a"), "спорное в пакете: {md}");
        assert!(!md.contains("src/b"), "pass в пакет не попадает: {md}");
        // Доказательства — цитата с указателем на источник.
        assert!(md.contains("self.charged.append(amount_minor)"), "{md}");
        assert!(
            md.contains("Красные детекторы: reports/detectors/fitness.json"),
            "{md}"
        );
        assert!(md.contains("Причина: подтверждённое нарушение"), "{md}");
    }

    #[test]
    fn agreement_between_judges_is_not_a_disagreement() {
        let tmp = tempfile::tempdir().expect("tmp");
        let (label, root) = product(
            tmp.path(),
            "product-a",
            &[("code_invariant_conformance", "src/a", "fail")],
        );
        let path = root.join("reports/rubric/src-a--code_vs_spine.json");
        let mut text = std::fs::read_to_string(&path).expect("отчёт");
        text = text.replace(
            "\"weighted_total\": 2.0,",
            "\"weighted_total\": 2.0, \"second_judge\": {\"model\": \"judge-2\", \"report\": \"x\", \"decision\": \"fail\", \"weighted_total\": 2.1, \"agreement\": true, \"differences\": []},",
        );
        std::fs::write(&path, text).expect("правка");
        let report = collect(&[(label, root)]);
        assert!(
            report.disagreements.is_empty(),
            "{:?}",
            report.disagreements
        );
    }

    #[test]
    fn empty_portfolio_is_not_a_division_by_zero() {
        let tmp = tempfile::tempdir().expect("tmp");
        let report = collect(&[("empty".to_string(), tmp.path().to_path_buf())]);
        assert_eq!(report.reports, 0);
        assert!(report.human_share.abs() < f64::EPSILON);
        assert!(report.committee_markdown().contains("Спорных отчётов нет"));
    }
}
