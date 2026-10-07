//! Рендеры архитектурного диффа (K3, ADR-063): markdown-сводка одного
//! экрана (детали — сворачиваемые блоки), mermaid «до/после» (стиль
//! `model::graph_mermaid`), машинный JSON `arch-be/arch-diff/v1` и SARIF
//! (через общий рендер `report_fmt`) для площадок PR.

use std::collections::BTreeSet;
use std::fmt::Write as _;

use super::diff::{ArchDiff, ContractClass, EdgeChange, ModelStatus};
use super::types::{ARCH_DIFF_SCHEMA, ArchGraph, ArchNode};
use crate::error::Result;

/// Потолок строк деталей в сводке md (дальше — сворачиваемый блок):
/// экран ревьюера читается без прокрутки.
const MD_INLINE_LIMIT: usize = 8;

/// Заголовок узла для md: `id · title`.
fn node_label(diff: &ArchDiff, id: &str) -> String {
    diff.added_nodes
        .iter()
        .chain(&diff.removed_nodes)
        .find(|n| n.id == id)
        .map_or_else(|| id.to_string(), |n| format!("{} · {}", n.id, n.title))
}

/// Статус ребра в модели короткой строкой.
fn model_note(status: Option<ModelStatus>) -> &'static str {
    match status {
        Some(ModelStatus::InModel) => "в модели: да",
        Some(ModelStatus::NotInModel) => "в модели: НЕТ",
        None => "модели нет — колонка не применима",
    }
}

/// Строка изменения ребра для md (`+`/`-`).
fn edge_line(diff: &ArchDiff, change: &EdgeChange, sign: char) -> String {
    let e = &change.edge;
    let mut line = format!(
        "  {sign} {} → {} ({})",
        node_label(diff, &e.from),
        node_label(diff, &e.to),
        e.kind.label()
    );
    let mut notes = vec![model_note(change.model_status).to_string()];
    // Задетые инварианты через концы ребра.
    let ids = [&e.from, &e.to];
    let ads: Vec<&str> = diff
        .invariants_touched
        .iter()
        .filter(|h| h.via_components.iter().any(|c| ids.contains(&c)))
        .map(|h| h.ad_id.as_str())
        .collect();
    if !ads.is_empty() {
        notes.push(format!("задет инвариант {}", ads.join(", ")));
    }
    let _ = write!(line, " — {}", notes.join("; "));
    line
}

/// Строки секции изменений рёбер (md).
fn edge_section(diff: &ArchDiff, changes: &[EdgeChange], sign: char) -> Vec<String> {
    let mut lines = Vec::new();
    for change in changes {
        lines.push(edge_line(diff, change, sign));
        for ev in &change.edge.evidence {
            lines.push(format!("    основание: {ev}"));
        }
    }
    lines
}

/// Блок md: первые [`MD_INLINE_LIMIT`] строк развёрнуты, остаток —
/// в сворачиваемом `<details>`.
fn details_block(out: &mut String, lines: &[String]) {
    if lines.len() <= MD_INLINE_LIMIT {
        for l in lines {
            let _ = writeln!(out, "{l}");
        }
        return;
    }
    for l in &lines[..MD_INLINE_LIMIT] {
        let _ = writeln!(out, "{l}");
    }
    let _ = writeln!(
        out,
        "<details><summary>… ещё {}</summary>\n\n```",
        lines.len() - MD_INLINE_LIMIT
    );
    for l in &lines[MD_INLINE_LIMIT..] {
        let _ = writeln!(out, "{l}");
    }
    let _ = writeln!(out, "```\n</details>");
}

/// Класс изменения контракта по-русски.
fn contract_class_label(class: ContractClass) -> &'static str {
    match class {
        ContractClass::Added => "добавлен",
        ContractClass::Removed => "удалён (ломающее)",
        ContractClass::Breaking => "ЛОМАЮЩЕЕ",
        ContractClass::Additive => "аддитивное",
        ContractClass::Unknown => "формат не распознан",
    }
}

/// Значение метрики NFR для md (`None` — «—»).
fn nfr_value(v: Option<f64>) -> String {
    v.map_or_else(|| "—".to_string(), |v| format!("{v:.0}"))
}

/// Markdown-сводка диффа: один экран, детали — сворачиваемые блоки.
#[must_use]
pub fn render_md(diff: &ArchDiff) -> String {
    let mut out = String::new();
    let base_short = &diff.base[..diff.base.len().min(12)];
    let head_short = &diff.head[..diff.head.len().min(12)];
    let triggers = diff
        .route
        .triggers
        .iter()
        .map(|t| format!("{} [{}]", t.name, t.source))
        .collect::<Vec<_>>()
        .join(", ");
    let _ = writeln!(
        out,
        "Архитектурный дифф {base_short}..{head_short} · маршрут {} ({}){}",
        diff.route.route,
        diff.route.score,
        if triggers.is_empty() {
            String::new()
        } else {
            format!(" — {triggers}")
        }
    );
    if diff.is_empty() {
        let _ = writeln!(out, "\nАрхитектурных изменений нет.");
        return out;
    }

    if !diff.added_edges.is_empty() {
        let _ = writeln!(out, "\nНовые связи");
        details_block(&mut out, &edge_section(diff, &diff.added_edges, '+'));
    }
    if !diff.removed_edges.is_empty() {
        let _ = writeln!(out, "\nУдалённые связи");
        details_block(&mut out, &edge_section(diff, &diff.removed_edges, '-'));
    }
    if !diff.added_nodes.is_empty() {
        let _ = writeln!(out, "\nНовые узлы");
        let lines: Vec<String> = diff
            .added_nodes
            .iter()
            .map(|n| {
                let inferred = if n.inferred {
                    " (выведен по манифесту)"
                } else {
                    ""
                };
                format!("  + {} · {} [{}]{inferred}", n.id, n.title, n.kind.label())
            })
            .collect();
        details_block(&mut out, &lines);
    }
    if !diff.removed_nodes.is_empty() {
        let _ = writeln!(out, "\nУдалённые узлы");
        let lines: Vec<String> = diff
            .removed_nodes
            .iter()
            .map(|n| format!("  - {} · {} [{}]", n.id, n.title, n.kind.label()))
            .collect();
        details_block(&mut out, &lines);
    }

    let _ = writeln!(out, "\nКонтракты");
    if diff.contract_changes.is_empty() {
        let _ = writeln!(out, "  без изменений");
    } else {
        for c in &diff.contract_changes {
            let _ = writeln!(
                out,
                "  {} — {}",
                c.path,
                contract_class_label(c.classification)
            );
            for d in &c.details {
                let _ = writeln!(out, "    {d}");
            }
        }
    }

    let _ = writeln!(out, "NFR");
    if diff.nfr_shifts.is_empty() {
        let _ = writeln!(out, "  без изменений");
    } else {
        for s in &diff.nfr_shifts {
            let _ = writeln!(
                out,
                "  {} {} ({}): {} → {}",
                s.check,
                s.nfr,
                s.metric,
                nfr_value(s.was),
                nfr_value(s.now)
            );
        }
    }

    if !diff.declared_unused.is_empty() {
        let _ = writeln!(out, "\nОбъявлено в модели, но не используется в коде");
        for d in &diff.declared_unused {
            let _ = writeln!(out, "  ~ {} → {} (depends_on без импорта)", d.from, d.to);
        }
    }
    if !diff.invariants_touched.is_empty() {
        let _ = writeln!(out, "\nЗадетые инварианты");
        for h in &diff.invariants_touched {
            let _ = writeln!(out, "  ! {} «{}»", h.ad_id, h.ad_title);
            for r in &h.rules {
                let _ = writeln!(
                    out,
                    "    правило {} ({}, {})",
                    r.id,
                    r.name.as_deref().unwrap_or("—"),
                    r.teeth.label()
                );
            }
        }
    }
    if !diff.adrs_touched.is_empty() {
        let _ = writeln!(out, "\nЗатронутые ADR: {}", diff.adrs_touched.join(", "));
    }
    if !diff.proposals.is_empty() {
        let _ = writeln!(out, "\nПредложение модели (принятие — волна K5):");
        for p in &diff.proposals {
            let warn = if p.conflicts.is_empty() {
                String::new()
            } else {
                format!("   ⚠ противоречит {}", p.conflicts.join(", "))
            };
            let _ = writeln!(out, "  {}. {}{warn}", p.n, p.summary);
        }
    }
    if !diff.route.evidence.is_empty() {
        let lines: Vec<String> = diff
            .route
            .evidence
            .iter()
            .map(|e| format!("  {e}"))
            .collect();
        let _ = writeln!(
            out,
            "\n<details><summary>Основания триггеров</summary>\n\n```"
        );
        for l in lines {
            let _ = writeln!(out, "{l}");
        }
        let _ = writeln!(out, "```\n</details>");
    }
    out
}

/// Mermaid-id узла: `-`, `:`, `/`, `.` недопустимы/шумны в id flowchart —
/// замена на `_` (стиль `model::graph_mermaid`).
fn mermaid_id(id: &str) -> String {
    id.replace(['-', ':', '/', '.'], "_")
}

/// Метка узла: `id · title` с усечением и безопасными скобками (стиль
/// `model::graph_mermaid`).
fn mermaid_label(node: &ArchNode) -> String {
    const MAX_TITLE: usize = 48;
    let mut title: String = node.title.chars().take(MAX_TITLE).collect();
    if node.title.chars().count() > MAX_TITLE {
        title.push('…');
    }
    let mut safe = format!("{} · ", node.id);
    for ch in title.chars() {
        safe.push(match ch {
            '"' => '\'',
            '[' | '{' => '(',
            ']' | '}' => ')',
            other => other,
        });
    }
    safe
}

/// Mermaid «до/после» (стиль `graph_mermaid`): объединение узлов base/head,
/// добавленные рёбра — толстые (`==>`), удалённые и рёбра вне модели —
/// пунктирные (`-.->`) с меткой; новые/удалённые узлы — классы `added`/
/// `removed`.
#[must_use]
pub fn render_mermaid(base: &ArchGraph, head: &ArchGraph, diff: &ArchDiff) -> String {
    let mut out = String::from("flowchart LR\n");
    // Узлы: объединение head (актуальные заголовки) и removed (только base).
    let mut nodes: Vec<&ArchNode> = head.nodes.iter().collect();
    let head_ids: BTreeSet<&str> = head.nodes.iter().map(|n| n.id.as_str()).collect();
    nodes.extend(
        base.nodes
            .iter()
            .filter(|n| !head_ids.contains(n.id.as_str())),
    );
    nodes.sort_by(|a, b| a.id.cmp(&b.id));
    for n in &nodes {
        let _ = writeln!(out, "  {}[\"{}\"]", mermaid_id(&n.id), mermaid_label(n));
    }
    let added: BTreeSet<(&str, &str, _)> = diff
        .added_edges
        .iter()
        .map(|c| (c.edge.from.as_str(), c.edge.to.as_str(), c.edge.kind))
        .collect();
    let removed: BTreeSet<(&str, &str, _)> = diff
        .removed_edges
        .iter()
        .map(|c| (c.edge.from.as_str(), c.edge.to.as_str(), c.edge.kind))
        .collect();
    let undeclared: BTreeSet<(&str, &str, _)> = diff
        .added_edges
        .iter()
        .filter(|c| c.model_status == Some(ModelStatus::NotInModel))
        .map(|c| (c.edge.from.as_str(), c.edge.to.as_str(), c.edge.kind))
        .collect();
    let mut drawn: BTreeSet<(&str, &str, _)> = BTreeSet::new();
    // Актуальные рёбра head: неизменённые сплошные, добавленные — толстые
    // (вне модели — пунктир с меткой: сигнал важнее выделения новизны).
    for e in &head.edges {
        let key = (e.from.as_str(), e.to.as_str(), e.kind);
        drawn.insert(key);
        if undeclared.contains(&key) {
            let _ = writeln!(
                out,
                "  {} -.->|\"{} · новое · нет в модели\"| {}",
                mermaid_id(&e.from),
                e.kind.label(),
                mermaid_id(&e.to)
            );
        } else if added.contains(&key) {
            let _ = writeln!(
                out,
                "  {} ==>|\"{} · новое\"| {}",
                mermaid_id(&e.from),
                e.kind.label(),
                mermaid_id(&e.to)
            );
        } else {
            let _ = writeln!(
                out,
                "  {} -->|{}| {}",
                mermaid_id(&e.from),
                e.kind.label(),
                mermaid_id(&e.to)
            );
        }
    }
    // Удалённые рёбра (были на base, нет на head) — пунктир с меткой.
    for e in &base.edges {
        let key = (e.from.as_str(), e.to.as_str(), e.kind);
        if removed.contains(&key) && !drawn.contains(&key) {
            let _ = writeln!(
                out,
                "  {} -.->|\"{} · удалено\"| {}",
                mermaid_id(&e.from),
                e.kind.label(),
                mermaid_id(&e.to)
            );
        }
    }
    let added_nodes: Vec<String> = diff.added_nodes.iter().map(|n| mermaid_id(&n.id)).collect();
    let removed_nodes: Vec<String> = diff
        .removed_nodes
        .iter()
        .map(|n| mermaid_id(&n.id))
        .collect();
    if !added_nodes.is_empty() {
        let _ = writeln!(
            out,
            "  classDef added stroke:#2ea043,stroke-width:2px;\n  class {} added;",
            added_nodes.join(",")
        );
    }
    if !removed_nodes.is_empty() {
        let _ = writeln!(
            out,
            "  classDef removed stroke:#f85149,stroke-width:2px,stroke-dasharray:5 5;\n  class {} removed;",
            removed_nodes.join(",")
        );
    }
    out
}

/// Машинный контракт `arch-be/arch-diff/v1` (ADR-063): `schema` + версия
/// бинаря + сериализованный дифф. Детерминизм — сортировками движка (K2).
///
/// # Errors
/// Сериализация диффа (практически недостижимо — структуры плоские).
pub fn render_json(diff: &ArchDiff) -> Result<String> {
    let mut map = serde_json::Map::new();
    map.insert(
        "schema".to_string(),
        serde_json::Value::String(ARCH_DIFF_SCHEMA.to_string()),
    );
    map.insert(
        "arch_be".to_string(),
        serde_json::Value::String(env!("CARGO_PKG_VERSION").to_string()),
    );
    if let serde_json::Value::Object(m) = serde_json::to_value(diff)? {
        map.extend(m);
    }
    Ok(serde_json::to_string_pretty(&serde_json::Value::Object(
        map,
    ))?)
}

/// Адрес `файл:строка` из основания ребра → (file, line) для SARIF.
fn evidence_addr(evidence: &[String]) -> (Option<String>, Option<usize>) {
    let Some(first) = evidence.first() else {
        return (None, None);
    };
    match first.rsplit_once(':') {
        Some((file, line)) => (Some(file.to_string()), line.parse::<usize>().ok()),
        None => (Some(first.clone()), None),
    }
}

/// SARIF 2.1.0 (общий рендер `report_fmt`): рёбра вне модели, задетые
/// инварианты и ломающие контракты — как предупреждения с адресами, чтобы
/// площадки показали их в интерфейсе PR без доработок.
#[must_use]
pub fn render_sarif(diff: &ArchDiff) -> String {
    use crate::report_fmt::{Finding, FmtGroup, FmtReport, GroupStatus, Severity};

    let mut findings = Vec::new();
    for change in &diff.added_edges {
        if change.model_status != Some(ModelStatus::NotInModel) {
            continue;
        }
        let (file, line) = evidence_addr(&change.edge.evidence);
        findings.push(Finding {
            group: "arch-diff".to_string(),
            rule: "undeclared-edge".to_string(),
            severity: Severity::Warn,
            message: format!(
                "ребро {} → {} ({}) есть в коде, но не объявлено в модели",
                change.edge.from,
                change.edge.to,
                change.edge.kind.label()
            ),
            file,
            line,
        });
    }
    for hit in &diff.invariants_touched {
        findings.push(Finding {
            group: "arch-diff".to_string(),
            rule: "invariant-touched".to_string(),
            severity: Severity::Warn,
            message: format!(
                "изменение задевает инвариант {} «{}» (через {})",
                hit.ad_id,
                hit.ad_title,
                hit.via_components.join(", ")
            ),
            file: None,
            line: None,
        });
    }
    for c in &diff.contract_changes {
        if !matches!(
            c.classification,
            ContractClass::Breaking | ContractClass::Removed
        ) {
            continue;
        }
        findings.push(Finding {
            group: "arch-diff".to_string(),
            rule: "breaking-contract".to_string(),
            severity: Severity::Warn,
            message: format!(
                "контракт {}: {}",
                c.path,
                contract_class_label(c.classification)
            ),
            file: Some(c.path.clone()),
            line: None,
        });
    }
    let group = FmtGroup {
        name: "arch-diff".to_string(),
        status: GroupStatus::Pass,
        detail: format!("находок: {}", findings.len()),
        findings,
    };
    let report = FmtReport {
        tool: "arch-be arch-diff",
        summary: format!(
            "архитектурный дифф {}..{}",
            &diff.base[..diff.base.len().min(12)],
            &diff.head[..diff.head.len().min(12)]
        ),
        passed: true,
        groups: vec![group],
    };
    crate::report_fmt::render(crate::report_fmt::ReportFormat::Sarif, &report)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::super::diff::arch_diff;
    use super::super::diff::tests::{commit_all, fixture_agent_change, fixture_case, input};
    use super::super::snapshot::tests::{git_in, git_repo, write_file};
    use super::*;
    use crate::control::DiffGlobs;

    /// Демо-фикстура (раздел 9 задания): base + правка агента.
    fn demo(dir: &Path) -> (std::path::PathBuf, ArchDiff) {
        let repo = dir.join("case");
        std::fs::create_dir_all(&repo).expect("mkdir");
        fixture_case(&repo);
        git_repo(&repo);
        fixture_agent_change(&repo);
        commit_all(&repo, "agent/direct-ledger-write");
        let globs = DiffGlobs::default();
        let diff = arch_diff(&repo, &input(&globs, "main~1")).expect("дифф");
        (repo, diff)
    }

    /// md-сводка демо-сценария: экран содержит ребро вне модели с основанием,
    /// задетый инвариант с классом зубов, новое хранилище, ADR, маршрут с
    /// источником триггера и пронумерованное предложение с ⚠.
    #[test]
    fn md_demo_screen() {
        let tmp = tempfile::tempdir().expect("tmp");
        let (_repo, diff) = demo(tmp.path());
        let md = render_md(&diff);
        assert!(md.contains("Архитектурный дифф"), "{md}");
        assert!(md.contains("маршрут fast (1)"), "{md}");
        assert!(md.contains("new_datastore [diff]"), "{md}");
        assert!(md.contains("CMP-001"), "{md}");
        assert!(md.contains("в модели: НЕТ"), "{md}");
        assert!(
            md.contains("основание: skeleton/intake/writer.py:2"),
            "{md}"
        );
        assert!(md.contains("store:postgres://ledger-db:5432"), "{md}");
        assert!(md.contains("AD-2"), "{md}");
        assert!(
            md.contains("C-007 (no-direct-ledger-write, зубы не измерены)"),
            "{md}"
        );
        assert!(md.contains("ADR-003"), "{md}");
        assert!(md.contains("CMP-009 → CMP-004"), "{md}"); // declared-unused
        assert!(
            md.contains("1. model/CMP-001-intake.md: depends_on += CMP-004"),
            "{md}"
        );
        assert!(md.contains("⚠ противоречит AD-2"), "{md}");
    }

    /// Чистый рефакторинг: md — одна строка «нет изменений».
    #[test]
    fn md_empty_diff() {
        let tmp = tempfile::tempdir().expect("tmp");
        let repo = tmp.path().join("case");
        std::fs::create_dir_all(&repo).expect("mkdir");
        fixture_case(&repo);
        git_repo(&repo);
        write_file(
            &repo,
            "skeleton/intake/writer.py",
            "from skeleton.orchestrator import api\n\ndef write_registry():\n    api.post()\n",
        );
        git_in(
            &repo,
            &[
                "mv",
                "skeleton/intake/writer.py",
                "skeleton/intake/writer_impl.py",
            ],
        );
        commit_all(&repo, "refactor");
        let globs = DiffGlobs::default();
        let diff = arch_diff(&repo, &input(&globs, "main~1")).expect("дифф");
        let md = render_md(&diff);
        assert!(md.contains("Архитектурных изменений нет."), "{md}");
        assert!(!md.contains("Новые связи"), "{md}");
    }

    /// mermaid «до/после»: добавленное ребро вне модели — пунктир с меткой,
    /// узлы — с заголовками, новое хранилище — классом added; рёбра,
    /// объявленные в модели, остаются сплошными.
    #[test]
    fn mermaid_marks_added_undeclared_removed() {
        let tmp = tempfile::tempdir().expect("tmp");
        let (repo, diff) = demo(tmp.path());
        let globs = DiffGlobs::default();
        let base_g = crate::arch_diff::as_built_with(&repo, &diff.base, &globs).expect("base");
        let head_g = crate::arch_diff::as_built_with(&repo, &diff.head, &globs).expect("head");
        let mm = render_mermaid(&base_g, &head_g, &diff);
        assert!(mm.starts_with("flowchart LR\n"), "{mm}");
        assert!(mm.contains("CMP_001[\"CMP-001 · Приём реестров\"]"), "{mm}");
        // Новое ребро вне модели — пунктир с меткой.
        assert!(
            mm.contains("CMP_001 -.->|\"import · новое · нет в модели\"| CMP_004"),
            "{mm}"
        );
        // Объявленное ребро (CMP-001 → CMP-009) — сплошное.
        assert!(mm.contains("CMP_001 -->|import| CMP_009"), "{mm}");
        // Новое хранилище — класс added.
        assert!(mm.contains("classDef added"), "{mm}");
        assert!(mm.contains("store_postgres___ledger_db_5432"), "{mm}");
        // Валидность синтаксиса — рендером движка `arch-be mermaid`.
        let rendered = crate::mermaid::render(&mm);
        assert!(rendered.is_ok(), "{rendered:?}");
    }

    /// json: контракт `arch-be/arch-diff/v1` — поле схемы, стабильный набор
    /// ключей верхнего уровня, повторная сериализация байт-в-байт та же.
    #[test]
    fn json_contract_v1_stable() {
        let tmp = tempfile::tempdir().expect("tmp");
        let (_repo, diff) = demo(tmp.path());
        let a = render_json(&diff).expect("json");
        let b = render_json(&diff).expect("json");
        assert_eq!(a, b, "сериализация детерминированна");
        let v: serde_json::Value = serde_json::from_str(&a).expect("разбор");
        assert_eq!(v["schema"], ARCH_DIFF_SCHEMA);
        assert!(v["arch_be"].is_string(), "{v}");
        let keys: std::collections::BTreeSet<&str> = v
            .as_object()
            .expect("объект")
            .keys()
            .map(String::as_str)
            .collect();
        let expect: std::collections::BTreeSet<&str> = [
            "schema",
            "arch_be",
            "base",
            "head",
            "added_nodes",
            "removed_nodes",
            "added_edges",
            "removed_edges",
            "declared_unused",
            "invariants_touched",
            "adrs_touched",
            "contract_changes",
            "nfr_shifts",
            "route",
            "proposals",
        ]
        .into_iter()
        .collect();
        assert_eq!(keys, expect, "набор полей контракта v1");
        // Поля ребра (flatten) и его статуса.
        let edge = &v["added_edges"][0];
        for key in ["from", "to", "kind", "evidence", "model_status"] {
            assert!(edge.get(key).is_some(), "нет поля {key}: {edge}");
        }
        assert_eq!(v["route"]["triggers"][0]["name"], "new_datastore");
        assert_eq!(v["route"]["triggers"][0]["source"], "diff");
    }

    /// sarif: валидный SARIF 2.1.0 с находками undeclared-edge (с адресом
    /// `файл:строка`) и invariant-touched как warning.
    #[test]
    fn sarif_warnings_with_locations() {
        let tmp = tempfile::tempdir().expect("tmp");
        let (_repo, diff) = demo(tmp.path());
        let sarif = render_sarif(&diff);
        let v: serde_json::Value = serde_json::from_str(&sarif).expect("разбор sarif");
        assert_eq!(v["version"], "2.1.0");
        let results = v["runs"][0]["results"].as_array().expect("results");
        let rules: Vec<&str> = results
            .iter()
            .filter_map(|r| r["ruleId"].as_str())
            .collect();
        assert!(rules.contains(&"undeclared-edge"), "{rules:?}");
        assert!(rules.contains(&"invariant-touched"), "{rules:?}");
        let edge = results
            .iter()
            .find(|r| r["ruleId"] == "undeclared-edge")
            .expect("ребро");
        assert_eq!(edge["level"], "warning");
        assert_eq!(
            edge["locations"][0]["physicalLocation"]["artifactLocation"]["uri"],
            "skeleton/intake/writer.py"
        );
        assert_eq!(
            edge["locations"][0]["physicalLocation"]["region"]["startLine"],
            2
        );
    }
}
