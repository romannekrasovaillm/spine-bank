//! Рендер отчёта диффа контракта: человеко-читаемый текст
//! (`render_report`) и JSON-форма (`report_json`, `--json` CLI /
//! `structuredContent` MCP).

use std::fmt::Write as _;

use serde_json::{Value, json};

use super::types::DiffReport;

/// Собирает отчёт в стиле `spine_lint`: сводка с форматом, строки находок,
/// итог, опциональная секция связки с моделью.
#[must_use]
pub fn render_report(report: &DiffReport) -> String {
    let findings = &report.findings;
    let breaking = findings.iter().filter(|f| f.severity == "error").count();
    let non_breaking = findings.len() - breaking;
    let mut out = format!(
        "contract_diff: {} изменений (breaking: {breaking}, non-breaking: {non_breaking})\nФормат: {}",
        findings.len(),
        report.format.name()
    );
    // Запись в String не может завершиться ошибкой — игноры безопасны.
    for f in findings {
        let _ = writeln!(
            out,
            "[{}] {} {} — {}",
            f.severity, f.location, f.rule, f.message
        );
    }
    let _ = writeln!(
        out,
        "Итог: {}",
        if report.has_breaking() {
            "FAIL"
        } else {
            "PASS"
        }
    );
    if let Some(impact) = &report.impact {
        if impact.matched_int.is_empty() {
            let _ = writeln!(
                out,
                "Связь с моделью: ни один INT.contract не совпал с путями old/new (gap — контракт вне модели, ADR-035)"
            );
        } else {
            let _ = writeln!(
                out,
                "Связь с моделью: {} ({})",
                impact.matched_int.join(", "),
                impact.matched_paths.join(", ")
            );
            if !impact.consumers.is_empty() {
                let _ = writeln!(out, "  Потребители: {}", impact.consumers.join("; "));
            }
            if !impact.rules.is_empty() {
                let _ = writeln!(out, "  Правила: {}", impact.rules.join("; "));
            }
            if !impact.owners.is_empty() {
                let _ = writeln!(
                    out,
                    "  Владельцы (согласовать): {}",
                    impact.owners.join("; ")
                );
            }
            let _ = writeln!(out, "  {}", impact.summary);
        }
    }
    // Секция семантических зондов печатается только при `--semantic` (иначе
    // список пуст) — вывод без флага не меняется.
    if !report.semantic_changes.is_empty() {
        let _ = writeln!(
            out,
            "Семантические изменения (--semantic): {}",
            report.semantic_changes.len()
        );
        for f in &report.semantic_changes {
            let _ = writeln!(
                out,
                "[{}] {} {} — {}",
                f.severity, f.location, f.rule, f.message
            );
        }
    }
    if !report.semantic_consumers.is_empty() {
        let _ = writeln!(
            out,
            "Потребители полей [inferred]: {}",
            report.semantic_consumers.join("; ")
        );
    }
    out
}

/// JSON-форма отчёта (`arch-be contract-diff --json`).
#[must_use]
pub fn report_json(report: &DiffReport) -> Value {
    let breaking = report
        .findings
        .iter()
        .filter(|f| f.severity == "error")
        .count();
    let mut obj = serde_json::Map::new();
    obj.insert("tool".into(), json!("contract_diff"));
    obj.insert("passed".into(), json!(!report.has_breaking()));
    obj.insert("format".into(), json!(report.format.name()));
    obj.insert("breaking".into(), json!(breaking));
    obj.insert(
        "non_breaking".into(),
        json!(report.findings.len() - breaking),
    );
    obj.insert(
        "findings".into(),
        json!(
            report
                .findings
                .iter()
                .map(|f| json!({
                    "severity": f.severity,
                    "rule": f.rule,
                    "location": f.location,
                    "message": f.message,
                }))
                .collect::<Vec<_>>()
        ),
    );
    obj.insert(
        "impact".into(),
        report.impact.as_ref().map_or(Value::Null, |i| {
            json!({
                "matched_int": i.matched_int,
                "matched_paths": i.matched_paths,
                "consumers": i.consumers,
                "rules": i.rules,
                "owners": i.owners,
                "summary": i.summary,
            })
        }),
    );
    // Семантические блоки — только при непустом значении (`--semantic`):
    // вывод без флага не приобретает новых ключей.
    if !report.semantic_changes.is_empty() {
        obj.insert(
            "semantic_changes".into(),
            json!(
                report
                    .semantic_changes
                    .iter()
                    .map(|f| json!({
                        "severity": f.severity,
                        "rule": f.rule,
                        "location": f.location,
                        "message": f.message,
                    }))
                    .collect::<Vec<_>>()
            ),
        );
    }
    if !report.semantic_consumers.is_empty() {
        obj.insert(
            "semantic_consumers".into(),
            json!(report.semantic_consumers),
        );
    }
    obj.insert(
        "summary".into(),
        json!(format!(
            "{} изменений (breaking: {breaking}), формат {}",
            report.findings.len(),
            report.format.name()
        )),
    );
    Value::Object(obj)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract_diff::types::{ContractFormat, Finding};

    fn finding(severity: &str) -> Finding {
        Finding {
            severity: severity.to_string(),
            rule: "CD-001".to_string(),
            location: "#/paths/~1v1~1pets".to_string(),
            message: "сообщение".to_string(),
        }
    }

    /// Счётчики breaking/non-breaking в шапке — мутант `-`→`+` в
    /// `findings.len() - breaking` (пойман cargo-mutants 2026-09-24, волна C2).
    #[test]
    fn render_report_counts_breaking_and_non_breaking() {
        let report = DiffReport {
            format: ContractFormat::OpenApi,
            findings: vec![finding("error"), finding("error"), finding("warn")],
            impact: None,
            semantic_changes: Vec::new(),
            semantic_consumers: Vec::new(),
        };
        let text = render_report(&report);
        assert!(
            text.contains("3 изменений (breaking: 2, non-breaking: 1)"),
            "{text}"
        );
        assert!(text.contains("Итог: FAIL"), "{text}");

        let clean = DiffReport {
            format: ContractFormat::OpenApi,
            findings: vec![finding("warn")],
            impact: None,
            semantic_changes: Vec::new(),
            semantic_consumers: Vec::new(),
        };
        let text = render_report(&clean);
        assert!(
            text.contains("1 изменений (breaking: 0, non-breaking: 1)"),
            "{text}"
        );
        assert!(text.contains("Итог: PASS"), "{text}");
    }
}
