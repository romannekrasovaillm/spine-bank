//! Текстовый рендер отчёта гейта (B1) — человекочитаемый канал; машинные
//! форматы для CI — [`crate::report_fmt`].

use std::fmt::Write as _;

use super::types::{GateOutcome, GateReport, GateStatus, MAX_COMPONENT_FINDINGS};

/// Текстовый рендер отчёта гейта: строка маршрута, по каждой составляющей
/// PASS/FAIL/SKIP + краткая причина, находки отступом (с потолком
/// [`MAX_COMPONENT_FINDINGS`]), итоговая строка `Итог: PASS/FAIL/INCOMPLETE`.
#[must_use]
pub fn render(report: &GateReport) -> String {
    let mut out = String::new();
    // Запись в String не может завершиться ошибкой — игноры безопасны.
    let _ = writeln!(out, "Гейт: {}", report.repo.display());
    let _ = writeln!(out, "Маршрут: {} ({})", report.route, report.route_note);
    for c in &report.components {
        let req = if report.required.iter().any(|r| r == c.name) {
            " *"
        } else {
            ""
        };
        let _ = writeln!(
            out,
            "  [{}] {}{req} — {}",
            c.status.label(),
            c.name,
            c.detail
        );
        for f in c.findings.iter().take(MAX_COMPONENT_FINDINGS) {
            let _ = writeln!(out, "      ↳ {f}");
        }
        if c.findings.len() > MAX_COMPONENT_FINDINGS {
            let _ = writeln!(
                out,
                "      ↳ … и ещё {} находок (полный список — командами составляющих)",
                c.findings.len() - MAX_COMPONENT_FINDINGS
            );
        }
    }
    let failed = report
        .components
        .iter()
        .filter(|c| c.status == GateStatus::Fail)
        .count();
    let _ = writeln!(
        out,
        "Итог: {}",
        match report.outcome {
            GateOutcome::Pass => "PASS".to_string(),
            GateOutcome::Fail => format!("FAIL — провалено составляющих: {failed} (exit 1)"),
            GateOutcome::Incomplete => format!(
                "INCOMPLETE — обязательные составляющие без входа: {} (exit 3)",
                report.not_checked.join(", ")
            ),
        }
    );
    if !report.not_checked.is_empty() {
        let _ = writeln!(
            out,
            "Не проверено (обязательно для маршрута {}): {}",
            report.route,
            report.not_checked.join(", ")
        );
    }
    if !report.attestation.is_empty() {
        let _ = writeln!(out, "Аттестация вердикта: sha256:{}", report.attestation);
    }
    // Квитанция ценности (аддитивная строка): сумма error-находок всех
    // составляющих — это дефекты, остановленные механикой до ревью.
    if report.outcome == GateOutcome::Fail {
        let caught = report
            .components
            .iter()
            .flat_map(|c| &c.findings)
            .filter(|f| f.severity == "error")
            .count();
        let _ = writeln!(
            out,
            "Гейт поймал {caught} нарушений до ревью — исправьте и перепроверьте"
        );
    }
    out
}
