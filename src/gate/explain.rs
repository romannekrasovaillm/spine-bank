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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::Route;
    use crate::gate::{GateComponent, GateFinding};

    fn finding(severity: &str) -> GateFinding {
        GateFinding {
            severity: severity.to_string(),
            rule: Some("no_pan".to_string()),
            file: Some("src/a.py".to_string()),
            line: Some(7),
            message: "PAN в коде".to_string(),
        }
    }

    fn component(
        name: &'static str,
        status: GateStatus,
        findings: Vec<GateFinding>,
    ) -> GateComponent {
        GateComponent {
            name,
            status,
            detail: format!("детали {name}"),
            findings,
            not_verified: Vec::new(),
        }
    }

    fn report(
        components: Vec<GateComponent>,
        outcome: GateOutcome,
        required: Vec<String>,
        not_checked: Vec<String>,
    ) -> GateReport {
        GateReport {
            repo: std::path::PathBuf::from("."),
            route: Route::Fast,
            route_auto: false,
            route_note: "auto".to_string(),
            components,
            outcome,
            required,
            not_checked,
            inputs: Vec::new(),
            attestation: "abc123".to_string(),
            passed: outcome == GateOutcome::Pass,
        }
    }

    /// Звёздочка стоит ровно у обязательных составляющих и ни у кого больше.
    #[test]
    fn required_marker_selects_only_required_components() {
        let out = render(&report(
            vec![
                component("fitness", GateStatus::Pass, vec![]),
                component("delta_guard", GateStatus::Pass, vec![]),
            ],
            GateOutcome::Pass,
            vec!["fitness".to_string()],
            vec![],
        ));
        assert!(out.contains("  [PASS] fitness * — детали fitness"), "{out}");
        assert!(
            out.contains("  [PASS] delta_guard — детали delta_guard"),
            "{out}"
        );
        assert!(!out.contains("delta_guard *"), "{out}");
    }

    /// Потолок находок: ровно MAX печатается целиком, MAX+1 даёт строку
    /// «и ещё 1 находок», а не молчание и не «ещё 0».
    #[test]
    fn findings_are_capped_exactly_at_the_limit() {
        let at_limit: Vec<GateFinding> = (0..MAX_COMPONENT_FINDINGS)
            .map(|_| finding("error"))
            .collect();
        let out = render(&report(
            vec![component("fitness", GateStatus::Fail, at_limit)],
            GateOutcome::Fail,
            vec![],
            vec![],
        ));
        assert!(!out.contains("и ещё"), "на потолке остатка нет: {out}");

        let mut over: Vec<GateFinding> = (0..MAX_COMPONENT_FINDINGS)
            .map(|_| finding("error"))
            .collect();
        over.push(finding("error"));
        let out = render(&report(
            vec![component("fitness", GateStatus::Fail, over)],
            GateOutcome::Fail,
            vec![],
            vec![],
        ));
        assert!(out.contains("и ещё 1 находок"), "{out}");
    }

    /// Пустые «не проверено» и пустая аттестация не печатают своих строк.
    #[test]
    fn empty_not_checked_and_attestation_print_no_lines() {
        let mut r = report(
            vec![component("fitness", GateStatus::Pass, vec![])],
            GateOutcome::Pass,
            vec![],
            vec![],
        );
        r.attestation = String::new();
        let out = render(&r);
        assert!(!out.contains("Не проверено"), "{out}");
        assert!(!out.contains("Аттестация вердикта"), "{out}");

        // Непустые — печатают, и INCOMPLETE называет именно их.
        let r = report(
            vec![component("fitness", GateStatus::Skip, vec![])],
            GateOutcome::Incomplete,
            vec!["fitness".to_string()],
            vec!["fitness".to_string()],
        );
        let out = render(&r);
        assert!(out.contains("Аттестация вердикта: sha256:abc123"), "{out}");
        assert!(out.contains("Итог: INCOMPLETE"), "{out}");
        assert!(
            out.contains("INCOMPLETE — обязательные составляющие без входа: fitness (exit 3)"),
            "{out}"
        );
        assert!(
            out.contains("Не проверено (обязательно для маршрута Fast): fitness"),
            "{out}"
        );
    }

    /// FAIL считает проваленные составляющие и печатает квитанцию пойманных
    /// error-находок.
    #[test]
    fn fail_counts_components_and_caught_findings() {
        let out = render(&report(
            vec![
                component(
                    "fitness",
                    GateStatus::Fail,
                    vec![finding("error"), finding("warn"), finding("error")],
                ),
                component("spine_lint", GateStatus::Fail, vec![finding("error")]),
                // Провалов три, непровалов четыре: числа разные, иначе подмена
                // «статус == Fail» на «!=» дала бы тот же счёт.
                component("model_validate", GateStatus::Fail, vec![finding("error")]),
                component("delta_guard", GateStatus::Pass, vec![]),
                component("trace_check", GateStatus::Skip, vec![]),
                component("route_lock", GateStatus::Pass, vec![]),
                component("nfr", GateStatus::Pass, vec![]),
            ],
            GateOutcome::Fail,
            vec![],
            vec![],
        ));
        assert!(
            out.contains("Итог: FAIL — провалено составляющих: 3 (exit 1)"),
            "{out}"
        );
        assert!(out.contains("Гейт поймал 4 нарушений до ревью"), "{out}");
    }
}
