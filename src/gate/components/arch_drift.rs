//! Составляющая `arch_drift` (K6 волны K 0.3.14): рёбра графа «как
//! построено» против модели — замыкание цикла «код не может молча уйти от
//! модели, модель не может молча отстать от кода».
//!
//! Два класса находок (severity — `warn` по умолчанию, `error` в
//! `[gate.required]` маршрута):
//!
//! - `undeclared-edge`: ребро добавлено между базой (HEAD или `--base`) и
//!   рабочим деревом и помечено «нет в модели» ([`edge_model_status`] —
//!   та же функция, что у `arch_diff`, семантика не дублируется);
//! - `rejected-edge-present`: ребро, отклонённое решением `arch-diff reject`
//!   (журнал `.arch-handoff/arch-diff-decisions.json`, пишет K5), осталось в
//!   коде. Отклонение действует, пока `grounds_hash` записи совпадает с
//!   отпечатком текущих оснований ребра; основания изменились — ребро снова
//!   предложение (для гейта — обычный `undeclared-edge`, если оно в диффе).
//!
//! Производительность: граф строится дважды (снимок базовой ревизии + обход
//! рабочего дерева), на большом репозитории это секунды — поэтому по
//! умолчанию составляющая SKIP, а вызывается только когда названа в
//! `[gate.required]` маршрута ИЛИ включена флагом `[gate.arch_drift] enabled`.
//!
//! Граница с `model_drift` (C1): та отвечает за привязку модели к коду
//! (существование путей `code_roots`, покрытие манифестов, контракты INT);
//! эта — за рёбра графа кода против модели. Логика не дублируется: обе
//! стороны пользуются общими [`crate::model::drift`] и [`crate::arch_diff`].

use std::collections::BTreeSet;
use std::path::Path;

use super::super::git::{GitProbe, base_rev, git_rev_exists};
use super::super::types::{GateComponent, GateFinding, GateOptions};
use crate::arch_diff::{self, Decision, ModelStatus};

/// Составляющая `arch_drift`: дифф рёбер «база → рабочее дерево» и журнал
/// решений arch-diff против графа головы.
///
/// `required` — составляющая названа в `[gate.required]` маршрута (находки
/// блокируют); иначе нужен явный `[gate.arch_drift] enabled = true`, и
/// находки — warn.
pub(in crate::gate) fn component_arch_drift(
    repo: &Path,
    base: Option<&str>,
    git: &GitProbe,
    options: &GateOptions,
    required: bool,
) -> GateComponent {
    if !required && !options.arch_drift.enabled {
        return GateComponent::skip(
            "arch_drift",
            "не включена: добавьте 'arch_drift' в [gate.required] нужного маршрута или \
             задайте [gate.arch_drift] enabled = true (проверка дорогая: снимки ревизий git)"
                .to_string(),
        );
    }
    if !git.repo {
        return GateComponent::skip(
            "arch_drift",
            "не git-репозиторий — дифф рёбер недоступен".to_string(),
        );
    }
    if !repo.join("model").is_dir() {
        return GateComponent::skip(
            "arch_drift",
            "нет каталога model/ — рёбрам графа не с чем сверяться".to_string(),
        );
    }
    let rev = base_rev(base.unwrap_or("HEAD"));
    if !git_rev_exists(repo, rev) {
        return GateComponent::skip(
            "arch_drift",
            format!(
                "базовая ревизия '{rev}' не существует (нет коммитов?) — дифф рёбер недоступен"
            ),
        );
    }
    // Журнал решений (K5 пишет, K6 читает): файла нет — решений не было;
    // файл есть, но не читается — сломанный вход, FAIL с причиной (молчаливый
    // «нет решений» скрыл бы подмену журнала отклонений).
    let journal = match arch_diff::load_decisions(repo) {
        Ok(journal) => journal,
        Err(e) => {
            return GateComponent::fail(
                "arch_drift",
                format!("вход сломан: {e}"),
                vec![GateFinding::ruled(
                    "error".to_string(),
                    "arch_diff_decisions_invalid".to_string(),
                    format!(
                        "{e} → восстановите журнал корректной записью `arch-be arch-diff accept|reject`"
                    ),
                )],
            );
        }
    };
    // ADR-068 Am.2: пределы снимка из `[gate.arch_drift]` — `max_files`
    // (большие легаси-кейсы) и `ignore` (подстроки путей, напр. `env/`). Одни
    // и те же пределы для базы и головы: иначе исключённый путь исчезнет на
    // одной стороне и родит ложные рёбра.
    let limits = arch_diff::ScanLimits {
        max_content_files: options
            .arch_drift
            .max_files
            .unwrap_or(arch_diff::MAX_SNAPSHOT_CONTENT_FILES),
        ignore: options.arch_drift.ignore.clone(),
    };
    let base_scan = match arch_diff::scan_revision_with(repo, rev, &options.diff_globs, &limits) {
        Ok(scan) => scan,
        Err(e) => {
            return GateComponent::fail(
                "arch_drift",
                format!("сбой сканирования базы '{rev}': {e}"),
                Vec::new(),
            );
        }
    };
    let head_scan = match arch_diff::scan_worktree_with(repo, &options.diff_globs, &limits) {
        Ok(scan) => scan,
        Err(e) => {
            return GateComponent::fail(
                "arch_drift",
                format!("сбой сканирования рабочего дерева: {e}"),
                Vec::new(),
            );
        }
    };
    let Some(model) = head_scan.model.as_ref() else {
        return GateComponent::skip(
            "arch_drift",
            "в model/ нет читаемых сущностей — сверять рёбра не с чем".to_string(),
        );
    };

    let severity = if required { "error" } else { "warn" };
    let base_keys: BTreeSet<String> = base_scan
        .graph
        .edges
        .iter()
        .map(arch_diff::edge_id)
        .collect();
    let journal_entries = journal.as_ref().map_or(0, |j| j.entries.len());
    let rejects: Vec<&arch_diff::DecisionEntry> = journal
        .as_ref()
        .map(|j| {
            j.entries
                .iter()
                .filter(|e| e.decision == Decision::Reject)
                .collect()
        })
        .unwrap_or_default();

    let mut findings = Vec::new();
    let mut undeclared = 0usize;
    let mut rejected_live = 0usize;
    // Рёбра, ДОБАВЛЕННЫЕ между базой и рабочим деревом, которых нет в модели.
    // Ребро с актуальным отклонением здесь пропускается: ему — более сильная
    // находка `rejected-edge-present` ниже (одно ребро — одна находка).
    for edge in &head_scan.graph.edges {
        if base_keys.contains(&arch_diff::edge_id(edge)) {
            continue;
        }
        if arch_diff::edge_model_status(&head_scan.graph, Some(model), edge)
            != Some(ModelStatus::NotInModel)
        {
            continue;
        }
        let id = arch_diff::edge_id(edge);
        if rejects
            .iter()
            .any(|r| r.edge_id == id && r.grounds_hash == arch_diff::grounds_hash(edge))
        {
            continue;
        }
        undeclared += 1;
        findings.push(GateFinding::ruled(
            severity.to_string(),
            "undeclared-edge".to_string(),
            format!(
                "ребро {id} появилось в коде, но отсутствует в модели (основание: {}) → \
                 примите решение по ребру: `arch-be arch-diff accept` (правка модели дельтой) \
                 либо уберите ребро из кода",
                edge.evidence.first().cloned().unwrap_or_default()
            ),
        ));
    }
    // Отклонённые рёбра, ОСТАВШИЕСЯ в коде: сверка по графу головы целиком
    // (не только по диффу): отклонённое ребро могло быть закоммичено давно.
    for entry in &rejects {
        let Some(edge) = head_scan
            .graph
            .edges
            .iter()
            .find(|e| arch_diff::edge_id(e) == entry.edge_id)
        else {
            continue; // ребра в коде больше нет — решение исполнено
        };
        if entry.grounds_hash != arch_diff::grounds_hash(edge) {
            continue; // основания изменились — отклонение устарело, ребро снова предложение
        }
        rejected_live += 1;
        let reason = if entry.reason.is_empty() {
            "без указания причины".to_string()
        } else {
            format!("причина: {}", entry.reason)
        };
        findings.push(GateFinding::ruled(
            severity.to_string(),
            "rejected-edge-present".to_string(),
            format!(
                "ребро {} отклонено {} ({reason}), но осталось в коде (основание: {}) → \
                 уберите ребро из кода либо пересмотрите отказ новым решением",
                entry.edge_id,
                if entry.decided_at.is_empty() {
                    "без даты".to_string()
                } else {
                    entry.decided_at.clone()
                },
                edge.evidence.first().cloned().unwrap_or_default()
            ),
        ));
    }

    let errors = findings.iter().filter(|f| f.severity == "error").count();
    let detail = format!(
        "база {rev}, рёбер в рабочем дереве: {}, добавлено против базы: {}, вне модели: \
         {undeclared}, отклонённых осталось в коде: {rejected_live} (решений в журнале: \
         {journal_entries})",
        head_scan.graph.edges.len(),
        head_scan
            .graph
            .edges
            .iter()
            .filter(|e| !base_keys.contains(&arch_diff::edge_id(e)))
            .count(),
    );
    // Границы вердикта (W1): что зелёный здесь НЕ означает.
    let notes = vec![
        "рёбра «нет в модели» проверяются только среди добавленных между базой и рабочим \
         деревом; давние рёбра вне модели (уже в базе) видит `arch-be arch-diff`, а не эта \
         составляющая"
            .to_string(),
        "привязку модели к коду (существование code_roots, контракты INT) проверяет \
         составляющая model_drift — arch_drift её не дублирует"
            .to_string(),
    ];
    if errors > 0 {
        GateComponent::fail("arch_drift", detail, findings).noting(notes)
    } else {
        GateComponent::pass_with_findings("arch_drift", detail, findings).noting(notes)
    }
}
