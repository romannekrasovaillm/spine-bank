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
    let _ = writeln!(out, "Итог: {}", if breaking == 0 { "PASS" } else { "FAIL" });
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
    json!({
        "tool": "contract_diff",
        "passed": !report.has_breaking(),
        "format": report.format.name(),
        "breaking": breaking,
        "non_breaking": report.findings.len() - breaking,
        "findings": report.findings.iter().map(|f| json!({
            "severity": f.severity,
            "rule": f.rule,
            "location": f.location,
            "message": f.message,
        })).collect::<Vec<_>>(),
        "impact": report.impact.as_ref().map(|i| json!({
            "matched_int": i.matched_int,
            "matched_paths": i.matched_paths,
            "consumers": i.consumers,
            "rules": i.rules,
            "owners": i.owners,
            "summary": i.summary,
        })),
        "summary": format!(
            "{} изменений (breaking: {breaking}), формат {}",
            report.findings.len(),
            report.format.name()
        ),
    })
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
        };
        let text = render_report(&clean);
        assert!(
            text.contains("1 изменений (breaking: 0, non-breaking: 1)"),
            "{text}"
        );
        assert!(text.contains("Итог: PASS"), "{text}");
    }
}
