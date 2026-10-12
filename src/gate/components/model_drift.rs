//! Составляющая `model_drift` (C1 волны C 0.3.14): дрейф «модель ↔ код»
//! ([`crate::model::drift_check_with`]) как часть единого гейта.
//!
//! Дисциплина включения — как у `decision_quality` (образец по TASK v2):
//! по умолчанию составляющей нет в `[gate.required]` ни одного маршрута, и
//! находки дрейфа понижаются до warn (вердикт не ломают — чужие пайплайны
//! не краснеют без явного решения проекта); в `[gate.required]` маршрута
//! error-находки дрейфа (`code-root-missing`, `int-contract-missing`) валят
//! гейт.
//!
//! SKIP без входа не молчит: нет `model/` — честный SKIP (как у
//! `model_validate`); модель есть, но ни у одного CMP нет `code_roots` —
//! SKIP, и паспорт вердикта (блок «заявлено, но механикой не проверяется»)
//! пишет «модель не привязана к коду: 0 из N CMP имеют `code_roots`»
//! (проверка привязки не состоялась, и это видно, а не тонет в зелёном).
//!
//! Граница с `arch_drift` (K6): `model_drift` отвечает за привязку модели
//! к коду (существование путей `code_roots`, покрытие манифестов, звено
//! INT → контракт, мёртвые `depends_on`); рёбра графа «как построено»
//! против модели (ребро вне модели, отклонённое ребро в коде) — составляющая
//! `arch_drift`. Логика не дублируется: общие правила (`declared-edge-unused`)
//! живут в [`crate::model::drift`] и используются обеими сторонами.

use std::path::Path;

use super::super::types::{GateComponent, GateFinding, GateOptions};

/// Составляющая `model_drift`: прогон [`crate::model::drift_check_with`]
/// по корню кейса. `required` — составляющая в `[gate.required]` маршрута
/// (тогда severity находок сохраняются и error ломают гейт).
pub(in crate::gate) fn component_model_drift(
    repo: &Path,
    options: &GateOptions,
    required: bool,
) -> GateComponent {
    if !repo.join("model").is_dir() {
        return GateComponent::skip("model_drift", "нет каталога model/".to_string());
    }
    let drift_options = crate::model::DriftOptions {
        check_nfr_metrics: options.drift.nfr_metric_check,
    };
    let report = match crate::model::drift_check_with(repo, &drift_options) {
        Ok(report) => report,
        Err(e) => {
            return GateComponent::fail("model_drift", format!("сбой выполнения: {e}"), Vec::new());
        }
    };
    // Число исходных error-находок считаем ДО понижения (честность сводки,
    // дефект 2 0.4.7): после понижения вне [gate.required] их всегда 0, и
    // detail ложно выглядел как «серьёзных нет», хотя отдельная команда
    // `model drift` на том же дереве даёт FAIL с error.
    let mut original_errors = 0usize;
    let findings: Vec<GateFinding> = report
        .issues
        .iter()
        .map(|i| {
            let mut finding = GateFinding::lint(i);
            if finding.severity == "error" {
                original_errors += 1;
            }
            if !required {
                // Схема «warn → error по [gate.required]»: без явного
                // включения дрейф предупреждает, но вердикт не ломает.
                finding.severity = "warn".to_string();
            }
            finding
        })
        .collect();
    // SKIP без входа не молчит: модель без единого code_roots — это не
    // «проверено и чисто», а «привязку проверить нечем».
    if report.components_with_roots == 0 && report.components_total > 0 {
        let unbound = format!(
            "модель не привязана к коду: 0 из {} CMP имеют code_roots",
            report.components_total
        );
        return GateComponent::skip_with_findings(
            "model_drift",
            format!("{unbound} — привязку модель ↔ код проверить нечем"),
            findings,
        )
        .noting(vec![unbound]);
    }
    // Вердикт — по фактическим severity находок (после понижения): PASS вне
    // [gate.required] остаётся PASS.
    let errors = findings.iter().filter(|f| f.severity == "error").count();
    // detail показывает ИСХОДНЫЕ severity: при понижении прямо называем, что
    // error-находки дрейфа не исчезли, а ослаблены схемой гейта.
    let error_note = if original_errors > errors {
        format!("error: {original_errors} — вне [gate.required] понижены до warn")
    } else {
        format!("error: {errors}")
    };
    let detail = format!(
        "CMP с code_roots: {} из {}, находок: {} ({error_note})",
        report.components_with_roots,
        report.components_total,
        findings.len(),
    );
    if errors > 0 {
        GateComponent::fail("model_drift", detail, findings)
    } else {
        GateComponent::pass_with_findings("model_drift", detail, findings)
    }
}
