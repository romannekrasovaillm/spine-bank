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
    let findings: Vec<GateFinding> = report
        .issues
        .iter()
        .map(|i| {
            let mut finding = GateFinding::lint(i);
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
    let errors = findings.iter().filter(|f| f.severity == "error").count();
    let detail = format!(
        "CMP с code_roots: {} из {}, находок: {} (error: {errors}){}",
        report.components_with_roots,
        report.components_total,
        findings.len(),
        if required || errors == 0 {
            String::new()
        } else {
            " — вне [gate.required] error дрейфа предупреждают; добавьте 'model_drift' \
             в [gate.required] маршрута, чтобы дрейф блокировал выпуск"
                .to_string()
        }
    );
    if errors > 0 {
        GateComponent::fail("model_drift", detail, findings)
    } else {
        GateComponent::pass_with_findings("model_drift", detail, findings)
    }
}
