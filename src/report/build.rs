//! Сборка модели отчёта о восстановленной архитектуре (ADR-080, этап 1).
//!
//! Единственное место, где строятся элементы, назначаются report-ID и
//! уверенность A/B/C (AD-11): входы — граф [`recover`] одной ревизии,
//! обследование [`crate::survey`] и факты [`crate::constraints_harvest`], все
//! read-only. Детерминизм (правило 10): сортировки, `BTreeMap`, дата из git —
//! повторный прогон одной ревизии даёт байт-в-байт тот же JSON.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use crate::arch_diff::{ArchGraph, ArchNode, EdgeKind, NodeKind, Snapshot, recover, snapshot_at};
use crate::constraints_harvest::{self, Harvest};
use crate::control::DiffGlobs;
use crate::error::{HarnessError, Result};
use crate::report::check;
use crate::report::model::{
    CheckFinding, ConfidenceAbc, Family, REPORT_SCHEMA, ReportElement, ReportMeta, ReportModel,
    ReportRelation, ReportSection, SECTION_COUNT, SECTION_KINDS, SECTION_TITLES, STATUS_AUTO,
    SectionItem,
};
use crate::survey::{self, SurveyReport};

/// Порог god-модуля: сумма степеней import-рёбер компонента строго больше.
const GOD_DEGREE: usize = 10;
/// Потолок автопутей в §9.
const MAX_SCENARIOS: usize = 5;
/// Потолок строк «топ-рисков» в §1.
const MAX_TOP_RISKS: usize = 5;

/// Сборка модели отчёта по ревизии `repo`/`rev` (по умолчанию HEAD).
///
/// # Errors
/// Репозиторий не git-репозиторий, ревизия не разрешается, снимок/обследование
/// или сбор фактов падают — ошибка с контекстом.
pub fn build(repo: &Path, rev: Option<&str>) -> Result<ReportModel> {
    let outcome = recover(repo, rev, &DiffGlobs::default(), None)
        .map_err(|e| HarnessError::Model(format!("recover {}: {e}", repo.display())))?;
    let graph = outcome.graph;
    let snap = snapshot_at(repo, &graph.rev)
        .map_err(|e| HarnessError::Model(format!("снимок ревизии {}: {e}", graph.rev)))?;
    let survey = survey::scan(repo)
        .map_err(|e| HarnessError::Model(format!("survey {}: {e}", repo.display())))?;
    let harvest = constraints_harvest::harvest(repo)
        .map_err(|e| HarnessError::Model(format!("constraints-harvest {}: {e}", repo.display())))?;

    let ctx = Ctx {
        repo,
        snap: &snap,
        graph: &graph,
        survey: &survey,
        harvest: &harvest,
    };

    let mut elements = Vec::new();
    let mut node_elem: BTreeMap<String, usize> = BTreeMap::new();
    for node in &graph.nodes {
        node_elem.insert(node.id.clone(), elements.len());
        elements.push(element_from_node(&ctx, node));
    }
    elements.extend(risk_elements(&ctx));
    elements.extend(interface_elements(&ctx));
    elements.extend(context_elements(&ctx));
    elements.extend(decision_elements(&ctx));
    assign_ids(&mut elements);

    // node id → report-ID (biекция).
    let graph_to_id: BTreeMap<String, String> = elements
        .iter()
        .filter_map(|e| e.graph_id.clone().map(|g| (g, e.id.clone())))
        .collect();

    let relations = relations_from_edges(&ctx, &graph_to_id);
    let sections = sections(&ctx, &elements, &relations);

    let mut meta = meta(&ctx);
    meta.confidence_distribution = confidence_distribution(&elements);

    let mut model = ReportModel {
        schema: REPORT_SCHEMA.to_string(),
        arch_be: env!("CARGO_PKG_VERSION").to_string(),
        meta,
        elements,
        relations,
        sections,
        checks: Vec::new(),
    };
    let findings: Vec<CheckFinding> = check::check(&model, &snap, repo);
    model.checks = findings;
    Ok(model)
}

/// Общий контекст сборки (входы, доступные всем стадиям).
struct Ctx<'a> {
    repo: &'a Path,
    snap: &'a Snapshot,
    graph: &'a ArchGraph,
    survey: &'a SurveyReport,
    harvest: &'a Harvest,
}

/// Ссылки-доказательства, которые разрешаются в дереве ревизии/рабочем дереве.
fn resolvable(ctx: &Ctx, refs: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut out: Vec<String> = refs
        .into_iter()
        .map(|r| r.trim().to_string())
        .filter(|r| !r.is_empty() && check::resolve_ref(ctx.repo, ctx.snap, r).is_none())
        .collect();
    out.sort();
    out.dedup();
    out
}

/// Уверенность по наличию непустого evidence: пусто — `C`, иначе — `B`.
fn conf_from_evidence(evidence: &[String]) -> ConfidenceAbc {
    if evidence.is_empty() {
        ConfidenceAbc::C
    } else {
        ConfidenceAbc::B
    }
}

/// Семейство элемента по виду узла графа.
fn node_family(node: &ArchNode) -> Family {
    match node.kind {
        NodeKind::ExternalSystem => Family::Context,
        NodeKind::Datastore => Family::Decision,
        NodeKind::Contract | NodeKind::Topic => Family::Interface,
        NodeKind::Component => match node.id.strip_prefix("dir:") {
            Some(path) if !path.contains('/') => Family::Container,
            _ => Family::Component,
        },
    }
}

/// Элемент по узлу графа: семейство, уверенность, основания, атрибуты.
fn element_from_node(ctx: &Ctx, node: &ArchNode) -> ReportElement {
    let mut raw: Vec<String> = Vec::new();
    for e in ctx
        .graph
        .edges
        .iter()
        .filter(|e| e.from == node.id || e.to == node.id)
    {
        raw.extend(e.evidence.iter().cloned());
    }
    if let Some(p) = node.id.strip_prefix("dir:") {
        raw.push(format!("{}/", p.trim_end_matches('/')));
    } else if let Some(p) = node.id.strip_prefix("contract:") {
        raw.push(p.to_string());
    }
    let evidence = resolvable(ctx, raw);

    let confirmed = ctx.graph.edges.iter().any(|e| {
        (e.from == node.id || e.to == node.id)
            && e.confidence == Some(crate::arch_diff::Confidence::Confirmed)
    });
    let confidence = if (node.model_id.is_some() || confirmed) && !evidence.is_empty() {
        ConfidenceAbc::A
    } else {
        conf_from_evidence(&evidence)
    };

    let mut props = BTreeMap::new();
    props.insert("kind".to_string(), node.kind.label().to_string());
    if node.inferred {
        props.insert("inferred".to_string(), "true".to_string());
    }
    if let Some(mid) = &node.model_id {
        props.insert("model_id".to_string(), mid.clone());
    }

    ReportElement {
        id: String::new(),
        family: node_family(node),
        graph_id: Some(node.id.clone()),
        title: node.title.clone(),
        confidence,
        evidence,
        props,
    }
}

/// RISK-элементы: циклы импортов, god-модули, мёртвые компоненты, churn/долг.
fn risk_elements(ctx: &Ctx) -> Vec<ReportElement> {
    let mut out = Vec::new();

    for (members, evidence) in import_cycles(ctx.graph) {
        let evidence = resolvable(ctx, evidence);
        out.push(ReportElement {
            id: String::new(),
            family: Family::Risk,
            graph_id: None,
            title: format!("Цикл зависимостей: {}", members.join(" → ")),
            confidence: conf_from_evidence(&evidence),
            evidence,
            props: props("cycle"),
        });
    }

    for (id, title, evidence) in god_modules(ctx.graph) {
        let evidence = resolvable(ctx, evidence);
        out.push(ReportElement {
            id: String::new(),
            family: Family::Risk,
            graph_id: Some(id),
            title: format!("God-модуль (> {GOD_DEGREE} import-связей): {title}"),
            confidence: conf_from_evidence(&evidence),
            evidence,
            props: props("god_module"),
        });
    }

    for node in dead_components(ctx.graph) {
        let mut raw = Vec::new();
        if let Some(p) = node.id.strip_prefix("dir:") {
            raw.push(format!("{}/", p.trim_end_matches('/')));
        }
        let evidence = resolvable(ctx, raw);
        out.push(ReportElement {
            id: String::new(),
            family: Family::Risk,
            graph_id: Some(node.id.clone()),
            title: format!("{} — нет связей: кандидат в dead code", node.title),
            confidence: conf_from_evidence(&evidence),
            evidence,
            props: props("dead_code"),
        });
    }

    for needle in ["Риски изменения", "Долг и трупы"] {
        for finding in survey_findings(ctx.survey, needle) {
            let evidence = resolvable(ctx, finding.1.clone());
            out.push(ReportElement {
                id: String::new(),
                family: Family::Risk,
                graph_id: None,
                title: finding.0.clone(),
                confidence: conf_from_evidence(&evidence),
                evidence,
                props: props("survey"),
            });
        }
    }
    out
}

/// INT-элементы: точки входа из обследования.
fn interface_elements(ctx: &Ctx) -> Vec<ReportElement> {
    survey_findings(ctx.survey, "Точки входа")
        .into_iter()
        .map(|(text, raw)| {
            let evidence = resolvable(ctx, raw);
            ReportElement {
                id: String::new(),
                family: Family::Interface,
                graph_id: None,
                title: text,
                confidence: conf_from_evidence(&evidence),
                evidence,
                props: props("entry_point"),
            }
        })
        .collect()
}

/// CTX-элементы: пробелы графа (внешние имена без компонента) и интеграции.
fn context_elements(ctx: &Ctx) -> Vec<ReportElement> {
    let mut out = Vec::new();
    for gap in &ctx.graph.gaps {
        out.push(ReportElement {
            id: String::new(),
            family: Family::Context,
            graph_id: None,
            title: format!("[gap] {gap} — имя из конфигурации/кода без компонента"),
            confidence: ConfidenceAbc::C,
            evidence: Vec::new(),
            props: props("gap"),
        });
    }
    for (text, raw) in survey_findings(ctx.survey, "Интеграции") {
        let evidence = resolvable(ctx, raw);
        out.push(ReportElement {
            id: String::new(),
            family: Family::Context,
            graph_id: None,
            title: text,
            confidence: conf_from_evidence(&evidence),
            evidence,
            props: props("integration"),
        });
    }
    out
}

/// ADR-кандидаты: выбор топологии (синтетический) и кандидаты по хранилищам.
fn decision_elements(ctx: &Ctx) -> Vec<ReportElement> {
    let mut out = vec![ReportElement {
        id: String::new(),
        family: Family::Decision,
        graph_id: None,
        title: "Выбор топологии развёртывания (монорепо/сервисы)".to_string(),
        confidence: ConfidenceAbc::C,
        evidence: Vec::new(),
        props: props("topology"),
    }];
    let datastore_evidence: Vec<String> = ctx
        .graph
        .nodes
        .iter()
        .filter(|n| n.kind == NodeKind::Datastore)
        .flat_map(|n| {
            ctx.graph
                .edges
                .iter()
                .filter(|e| e.from == n.id || e.to == n.id)
                .flat_map(|e| e.evidence.iter().cloned())
        })
        .collect();
    let evidence = resolvable(ctx, datastore_evidence);
    out.push(ReportElement {
        id: String::new(),
        family: Family::Decision,
        graph_id: None,
        title: "Выбор хранилища(щ) данных".to_string(),
        confidence: ConfidenceAbc::C,
        evidence,
        props: props("datastore_choice"),
    });
    out
}

/// Атрибуты элемента (`source` = вид вывода).
fn props(source: &str) -> BTreeMap<String, String> {
    let mut p = BTreeMap::new();
    p.insert("source".to_string(), source.to_string());
    p
}

/// Находки секции обследования по подстроке заголовка: `(текст, ссылки)`.
fn survey_findings(survey: &SurveyReport, needle: &str) -> Vec<(String, Vec<String>)> {
    survey
        .sections
        .iter()
        .find(|s| s.title.contains(needle))
        .map(|s| {
            s.findings
                .iter()
                .map(|f| (f.text.clone(), f.refs.clone()))
                .collect()
        })
        .unwrap_or_default()
}

/// Назначение report-ID: по семейству, сортировка по `graph_id`/`title`,
/// нумерация с 01. Стабильно между прогонами одной ревизии.
fn assign_ids(elements: &mut [ReportElement]) {
    for family in Family::ALL {
        let mut idx: Vec<usize> = (0..elements.len())
            .filter(|&i| elements[i].family == family)
            .collect();
        idx.sort_by(|&a, &b| {
            let ka = elements[a].graph_id.as_deref().unwrap_or("");
            let kb = elements[b].graph_id.as_deref().unwrap_or("");
            ka.cmp(kb)
                .then_with(|| elements[a].title.cmp(&elements[b].title))
        });
        for (n, i) in idx.into_iter().enumerate() {
            elements[i].id = format!("{}-{:02}", family.prefix(), n + 1);
        }
    }
    elements.sort_by(|a, b| a.family.cmp(&b.family).then_with(|| a.id.cmp(&b.id)));
}

/// Связи отчёта из рёбер графа: обе стороны имеют report-ID.
fn relations_from_edges(ctx: &Ctx, graph_to_id: &BTreeMap<String, String>) -> Vec<ReportRelation> {
    let mut out = Vec::new();
    for e in &ctx.graph.edges {
        let (Some(from), Some(to)) = (graph_to_id.get(&e.from), graph_to_id.get(&e.to)) else {
            continue;
        };
        out.push(ReportRelation {
            from: from.clone(),
            to: to.clone(),
            kind: e.kind.label().to_string(),
            evidence: resolvable(ctx, e.evidence.clone()),
        });
    }
    out
}

/// Метрики снимка: (число файлов, сумма строк по доступному содержимому).
fn metrics(snap: &Snapshot) -> (usize, usize) {
    let loc = snap.contents.values().map(|c| c.lines().count()).sum();
    (snap.files.len(), loc)
}

/// Разделы §1–§14 (детерминированные — наполнены, интерпретационные — плейсхолдер).
fn sections(
    ctx: &Ctx,
    elements: &[ReportElement],
    relations: &[ReportRelation],
) -> Vec<ReportSection> {
    let ids = |f: Family| -> Vec<String> {
        elements
            .iter()
            .filter(|e| e.family == f)
            .map(|e| e.id.clone())
            .collect()
    };
    let (files, loc) = metrics(ctx.snap);

    let mut counts_summary = BTreeMap::new();
    counts_summary.insert("elements".to_string(), elements.len());
    counts_summary.insert("relations".to_string(), relations.len());
    counts_summary.insert("contexts".to_string(), ids(Family::Context).len());
    counts_summary.insert("containers".to_string(), ids(Family::Container).len());
    counts_summary.insert("components".to_string(), ids(Family::Component).len());
    counts_summary.insert("interfaces".to_string(), ids(Family::Interface).len());
    counts_summary.insert("risks".to_string(), ids(Family::Risk).len());
    counts_summary.insert("decisions".to_string(), ids(Family::Decision).len());

    let top_risks: Vec<SectionItem> = elements
        .iter()
        .filter(|e| e.family == Family::Risk)
        .take(MAX_TOP_RISKS)
        .map(|e| SectionItem {
            label: format!("{}: {}", e.id, e.title),
            refs: e.evidence.clone(),
            note: Some(e.confidence.as_str().to_string()),
        })
        .collect();

    let mut overview_items: Vec<SectionItem> = Vec::new();
    for needle in [
        "Карта компонентов",
        "Ограничения платформы",
        "Тесты и наблюдаемость",
    ] {
        for (text, raw) in survey_findings(ctx.survey, needle) {
            overview_items.push(SectionItem {
                label: text,
                refs: resolvable(ctx, raw),
                note: None,
            });
        }
    }

    let mut cross_items: Vec<SectionItem> = Vec::new();
    if let Some(env) = ctx.harvest.sections.iter().find(|s| s.id == "env") {
        for fact in &env.facts {
            let refs = resolvable(ctx, [fact.evidence.clone()]);
            cross_items.push(SectionItem {
                label: fact.text.clone(),
                refs,
                note: None,
            });
        }
    }

    let data_items: Vec<SectionItem> = survey_findings(ctx.survey, "Модель данных")
        .into_iter()
        .map(|(text, raw)| SectionItem {
            label: text,
            refs: resolvable(ctx, raw),
            note: None,
        })
        .collect();

    let scenarios = scenario_items(elements, relations);

    let deploy_items: Vec<SectionItem> = ctx
        .snap
        .files
        .iter()
        .filter(|f| is_deploy_file(f))
        .map(|f| SectionItem {
            label: f.clone(),
            refs: vec![f.clone()],
            note: None,
        })
        .collect();

    let uncertainty_items: Vec<SectionItem> = elements
        .iter()
        .filter(|e| e.confidence == ConfidenceAbc::C)
        .map(|e| SectionItem {
            label: format!("{}: {}", e.id, e.title),
            refs: e.evidence.clone(),
            note: Some("C".to_string()),
        })
        .collect();

    let exec =
        |idx: usize, element_ids: Vec<String>, counts, items: Vec<SectionItem>| ReportSection {
            id: (idx + 1).to_string(),
            title: SECTION_TITLES[idx].to_string(),
            kind: SECTION_KINDS[idx],
            element_ids,
            counts,
            placeholder: None,
            items,
        };
    let mut interp = |idx: usize, items: Vec<SectionItem>| {
        let empty = items.is_empty();
        ReportSection {
            id: (idx + 1).to_string(),
            title: SECTION_TITLES[idx].to_string(),
            kind: SECTION_KINDS[idx],
            element_ids: Vec::new(),
            counts: BTreeMap::new(),
            items,
            placeholder: Some(if empty {
                "[gap]".to_string()
            } else {
                "[Интерпретация Spine]".to_string()
            }),
        }
    };
    let _ = &mut interp;

    let mut out = Vec::with_capacity(SECTION_COUNT);
    out.push(exec(0, Vec::new(), counts_summary, top_risks));
    out.push(exec(1, Vec::new(), BTreeMap::new(), Vec::new()));
    out.push(exec(
        2,
        Vec::new(),
        [("files".to_string(), files), ("loc".to_string(), loc)]
            .into_iter()
            .collect(),
        overview_items,
    ));
    out.push(exec(3, ids(Family::Context), BTreeMap::new(), Vec::new()));
    out.push(exec(4, ids(Family::Container), BTreeMap::new(), Vec::new()));
    out.push(exec(5, ids(Family::Component), BTreeMap::new(), Vec::new()));
    out.push(interp(6, data_items));
    out.push(exec(7, ids(Family::Interface), BTreeMap::new(), Vec::new()));
    out.push(interp(8, scenarios));
    out.push(interp(9, cross_items));
    out.push(exec(
        10,
        Vec::new(),
        [("deploy_files".to_string(), deploy_items.len())]
            .into_iter()
            .collect(),
        deploy_items,
    ));
    out.push(exec(11, ids(Family::Decision), BTreeMap::new(), Vec::new()));
    out.push(exec(12, ids(Family::Risk), BTreeMap::new(), Vec::new()));
    out.push(interp(13, uncertainty_items));
    out
}

/// Автопути §9: из точек входа по рёбрам, не более [`MAX_SCENARIOS`], уверенность C.
fn scenario_items(elements: &[ReportElement], relations: &[ReportRelation]) -> Vec<SectionItem> {
    let int_ids: BTreeSet<&str> = elements
        .iter()
        .filter(|e| e.family == Family::Interface)
        .map(|e| e.id.as_str())
        .collect();
    relations
        .iter()
        .filter(|r| int_ids.contains(r.from.as_str()))
        .take(MAX_SCENARIOS)
        .map(|r| SectionItem {
            label: format!("{} → {} ({})", r.from, r.to, r.kind),
            refs: r.evidence.clone(),
            note: Some("C".to_string()),
        })
        .collect()
}

/// Файл относится к развёртыванию (docker/compose/k8s/helm).
fn is_deploy_file(file: &str) -> bool {
    let f = file.to_ascii_lowercase();
    f.contains("dockerfile")
        || f.contains("compose")
        || f.contains("/k8s/")
        || f.contains("/kubernetes/")
        || f.contains("/helm/")
        || f.ends_with(".k8s.yaml")
        || f.ends_with(".k8s.yml")
}

/// Шапка модели: система, repo, ветка, ревизия, дата из git, статус, счётчики.
fn meta(ctx: &Ctx) -> ReportMeta {
    let canonical = ctx
        .repo
        .canonicalize()
        .unwrap_or_else(|_| ctx.repo.to_path_buf());
    let system = canonical.file_name().map_or_else(
        || canonical.display().to_string(),
        |n| n.to_string_lossy().into(),
    );
    let branch =
        git_stdout(ctx.repo, &["rev-parse", "--abbrev-ref", "HEAD"]).filter(|b| b != "HEAD");
    let generated_at = git_stdout(ctx.repo, &["show", "-s", "--format=%cI", &ctx.graph.rev])
        .unwrap_or_else(|| ctx.graph.rev.clone());
    ReportMeta {
        system,
        repo: ctx.repo.display().to_string(),
        branch,
        rev: ctx.graph.rev.clone(),
        generated_at,
        status: STATUS_AUTO.to_string(),
        statistics: BTreeMap::new(),
        confidence_distribution: BTreeMap::new(),
    }
}

/// Распределение уверенности по элементам `{"A": n, "B": n, "C": n}`.
fn confidence_distribution(elements: &[ReportElement]) -> BTreeMap<String, usize> {
    let mut dist: BTreeMap<String, usize> = BTreeMap::new();
    for c in [ConfidenceAbc::A, ConfidenceAbc::B, ConfidenceAbc::C] {
        dist.insert(c.as_str().to_string(), 0);
    }
    for e in elements {
        *dist.entry(e.confidence.as_str().to_string()).or_default() += 1;
    }
    dist
}

/// stdout git-команды (trim), `None` при ненулевом коде/ошибке запуска.
fn git_stdout(repo: &Path, args: &[&str]) -> Option<String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Циклы импортов среди компонентов: `(участники, основания)`.
fn import_cycles(graph: &ArchGraph) -> Vec<(Vec<String>, Vec<String>)> {
    let comps: BTreeSet<String> = graph
        .nodes
        .iter()
        .filter(|n| n.kind == NodeKind::Component)
        .map(|n| n.id.clone())
        .collect();
    let nodes: Vec<String> = comps.iter().cloned().collect();
    let mut adj: BTreeMap<String, BTreeSet<String>> =
        nodes.iter().map(|n| (n.clone(), BTreeSet::new())).collect();
    for e in &graph.edges {
        if e.kind == EdgeKind::Import && comps.contains(&e.from) && comps.contains(&e.to) {
            adj.entry(e.from.clone()).or_default().insert(e.to.clone());
        }
    }

    let mut reach: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for n in &nodes {
        let mut seen = BTreeSet::new();
        let mut stack = vec![n.clone()];
        while let Some(x) = stack.pop() {
            if let Some(nei) = adj.get(&x) {
                for y in nei {
                    if seen.insert(y.clone()) {
                        stack.push(y.clone());
                    }
                }
            }
        }
        reach.insert(n.clone(), seen);
    }

    let mut out = Vec::new();
    let mut assigned: BTreeSet<String> = BTreeSet::new();
    for n in &nodes {
        if assigned.contains(n) {
            continue;
        }
        let mut group = vec![n.clone()];
        assigned.insert(n.clone());
        for m in &nodes {
            if m == n || assigned.contains(m) {
                continue;
            }
            let n_to_m = reach.get(n).is_some_and(|r| r.contains(m));
            let m_to_n = reach.get(m).is_some_and(|r| r.contains(n));
            if n_to_m && m_to_n {
                group.push(m.clone());
                assigned.insert(m.clone());
            }
        }
        if group.len() > 1 {
            let set: BTreeSet<String> = group.iter().cloned().collect();
            let mut evidence = Vec::new();
            for e in &graph.edges {
                if e.kind == EdgeKind::Import && set.contains(&e.from) && set.contains(&e.to) {
                    evidence.extend(e.evidence.iter().cloned());
                }
            }
            out.push((group, evidence));
        }
    }
    out
}

/// God-модули: компоненты со степенью import-рёбер строго больше [`GOD_DEGREE`].
fn god_modules(graph: &ArchGraph) -> Vec<(String, String, Vec<String>)> {
    let mut out = Vec::new();
    for n in graph.nodes.iter().filter(|n| n.kind == NodeKind::Component) {
        let incident: Vec<&crate::arch_diff::ArchEdge> = graph
            .edges
            .iter()
            .filter(|e| e.kind == EdgeKind::Import && (e.from == n.id || e.to == n.id))
            .collect();
        if incident.len() > GOD_DEGREE {
            let evidence: Vec<String> = incident
                .iter()
                .flat_map(|e| e.evidence.iter().cloned())
                .collect();
            out.push((n.id.clone(), n.title.clone(), evidence));
        }
    }
    out
}

/// Мёртвые компоненты: без входящих и исходящих рёбер (аннотация, не находка).
fn dead_components(graph: &ArchGraph) -> Vec<ArchNode> {
    let mut connected: BTreeSet<&str> = BTreeSet::new();
    for e in &graph.edges {
        connected.insert(e.from.as_str());
        connected.insert(e.to.as_str());
    }
    graph
        .nodes
        .iter()
        .filter(|n| n.kind == NodeKind::Component && !connected.contains(n.id.as_str()))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arch_diff::snapshot::tests::{git_repo, write_file};
    use crate::report::model::CheckSeverity;
    use crate::report::render_json;

    /// Фикстура без корневого манифеста (монорепо «каталоги верхнего уровня»):
    /// верхний крейт `api` (точка входа axum → CNT + INT), вложенный
    /// `services/ledger` (строка подключения к БД → `store:` и интеграция),
    /// вложенный висячий `services/orphan` (dead code → RISK), контракт,
    /// миграция и Dockerfile — все шесть семейств на одном репозитории.
    ///
    /// Корневой `Cargo.toml` намеренно отсутствует: `build_layout` не разбирает
    /// Cargo-манифесты, поэтому корневой манифест дал бы компонент `dir:.` с
    /// пустым корнем, покрывающим все вложенные каталоги, — вложенных CMP не
    /// возникло бы.
    fn report_fixture(repo: &Path) {
        write_file(repo, "api/Cargo.toml", "[package]\nname = \"api\"\n");
        write_file(
            repo,
            "api/src/main.rs",
            "fn main() {}\n// axum: .route(\"/api/pay\", post(pay))\n",
        );
        write_file(
            repo,
            "services/ledger/Cargo.toml",
            "[package]\nname = \"ledger\"\n",
        );
        write_file(
            repo,
            "services/ledger/config/application.yaml",
            "dsn: \"postgres://db.internal:5432/ledger\"\n",
        );
        write_file(
            repo,
            "services/orphan/Cargo.toml",
            "[package]\nname = \"orphan\"\n",
        );
        write_file(repo, "contracts/ledger.yaml", "openapi: 3.0.0\n");
        write_file(
            repo,
            "migrations/0001_init.sql",
            "CREATE TABLE payments (id bigint, amount numeric);\n",
        );
        write_file(repo, "Dockerfile", "FROM scratch\n");
    }

    /// Фикстура с подтверждённым межкомпонентным ребром и пробелом: к базовой
    /// добавляется конфиг `api`, где `lb://ledger` точно совпадает с
    /// компонентом `ledger` (ребро `calls` = confirmed → A), а
    /// `lb://phantom-service` не разрешается (пробел `[gap]` → C).
    fn confirmed_fixture(repo: &Path) {
        report_fixture(repo);
        write_file(
            repo,
            "api/config/application.yaml",
            "routes:\n  - uri: lb://ledger\n  - uri: lb://phantom-service\n",
        );
    }

    /// Общая сборка: tempdir + фикстура + git + модель HEAD; tempdir
    /// возвращается, чтобы репозиторий жил до конца теста (снимок читает
    /// рабочее дерево для резолва ссылок).
    fn build_with(fixture: fn(&Path)) -> (tempfile::TempDir, std::path::PathBuf, ReportModel) {
        let dir = tempfile::tempdir().expect("tmp");
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).expect("mkdir");
        fixture(&repo);
        git_repo(&repo);
        let model = build(&repo, Some("HEAD")).expect("build");
        (dir, repo, model)
    }

    /// Собирает базовую фикстуру (все шесть семейств) и модель HEAD.
    fn build_fixture() -> (tempfile::TempDir, std::path::PathBuf, ReportModel) {
        build_with(report_fixture)
    }

    /// Группа 1: все шесть семейств присутствуют, ID стабильны между прогонами,
    /// схема совпадает с контрактом, A/B несут основания (и1 держит сборщик).
    #[test]
    fn build_fixture_covers_all_families_with_stable_ids() {
        let (_dir, repo, model) = build_fixture();
        assert_eq!(model.schema, REPORT_SCHEMA, "схема контракта");
        assert!(!model.elements.is_empty(), "нет элементов");

        for family in Family::ALL {
            assert!(
                !model.elements_of(family).is_empty(),
                "нет элементов семейства {}: {:?}",
                family.prefix(),
                model
                    .elements
                    .iter()
                    .map(|e| e.id.clone())
                    .collect::<Vec<_>>()
            );
        }

        // и1 на собранной модели: A/B всегда с непустым evidence.
        for e in &model.elements {
            if e.confidence.requires_evidence() {
                assert!(!e.evidence.is_empty(), "{} без основания", e.id);
            }
        }

        // Стабильность ID: повторный прогон той же ревизии — те же ID.
        let again = build(&repo, Some("HEAD")).expect("build #2");
        let ids1: Vec<&str> = model.elements.iter().map(|e| e.id.as_str()).collect();
        let ids2: Vec<&str> = again.elements.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(ids1, ids2, "report-ID не стабильны между прогонами");
    }

    /// Группа 2: автопроверка ловит нарушения и1/и2/и3 как `error` и висячий
    /// элемент как `warn` (без блокировки). Базовая фикстура — чистая.
    #[test]
    fn autocheck_blocks_missing_evidence_bad_ref_and_broken_location() {
        let (_dir, repo, mut model) = build_fixture();
        let snap = snapshot_at(&repo, &model.meta.rev).expect("снимок");
        assert!(
            !model.has_errors(),
            "базовая фикстура неожиданно красная: {:?}",
            model.checks
        );

        // и1: B без основания.
        model.elements.push(ReportElement {
            id: "CMP-99".to_string(),
            family: Family::Component,
            graph_id: None,
            title: "элемент без основания".to_string(),
            confidence: ConfidenceAbc::B,
            evidence: Vec::new(),
            props: BTreeMap::new(),
        });
        // и2: связь на несуществующий report-ID.
        model.relations.push(ReportRelation {
            from: "CMP-99".to_string(),
            to: "ZZZ-01".to_string(),
            kind: "Import".to_string(),
            evidence: Vec::new(),
        });
        // и3: битый адрес при C (и1 молчит — C не требует evidence).
        model.elements.push(ReportElement {
            id: "RISK-99".to_string(),
            family: Family::Risk,
            graph_id: None,
            title: "битая ссылка".to_string(),
            confidence: ConfidenceAbc::C,
            evidence: vec!["ghost/missing.rs:5".to_string()],
            props: BTreeMap::new(),
        });

        let findings = check::check(&model, &snap, &repo);
        let has = |inv: &str, sev: CheckSeverity| {
            findings
                .iter()
                .any(|f| f.invariant == inv && f.severity == sev)
        };
        assert!(
            has("и1", CheckSeverity::Error),
            "и1 не найден: {findings:?}"
        );
        assert!(
            has("и2", CheckSeverity::Error),
            "и2 не найден: {findings:?}"
        );
        assert!(
            has("и3", CheckSeverity::Error),
            "и3 не найден: {findings:?}"
        );
        // Инжектированные элементы не упомянуты ни в одном разделе → orphan.
        assert!(
            findings
                .iter()
                .any(|f| f.severity == CheckSeverity::Warn && f.message.contains("orphan_element")),
            "orphan warn не найден: {findings:?}"
        );
        // Ошибки блокируют выдачу (fail-closed: CLI по этому флагу делает bail).
        model.checks = findings;
        assert!(model.has_errors(), "модель обязана стать красной");
    }

    /// Группа 3: JSON байт-в-байт детерминирован — один и тот же прогон и две
    /// независимые сборки одной ревизии.
    #[test]
    fn render_json_is_byte_deterministic() {
        let (_dir, repo, model) = build_fixture();
        let a = render_json(&model).expect("json #1");
        let b = render_json(&model).expect("json #2");
        assert_eq!(a, b, "JSON не детерминирован");
        assert!(a.contains(REPORT_SCHEMA), "нет схемы: {a}");
        assert!(a.contains("\"elements\""), "нет elements");

        let again = build(&repo, Some("HEAD")).expect("build #2");
        assert_eq!(
            a,
            render_json(&again).expect("json #3"),
            "JSON не стабилен между сборками одной ревизии"
        );
    }

    /// Группа 4: распределение A/B/C считается по элементам, доли (в счётчиках)
    /// сходятся к числу элементов, шапка и метод не расходятся.
    #[test]
    fn confidence_distribution_matches_elements() {
        let (_dir, _repo, model) = build_fixture();
        let dist = model.confidence_distribution();
        let total: usize = dist.values().sum();
        assert_eq!(
            total,
            model.elements.len(),
            "сумма распределения != числа элементов"
        );
        assert_eq!(
            model.meta.confidence_distribution, dist,
            "meta.confidence_distribution и метод расходятся"
        );
        for key in ["A", "B", "C"] {
            assert!(dist.contains_key(key), "нет ключа {key} в {dist:?}");
        }
        // Проценты рендера: сумма долей A+B+C = 100 (усечение вниз на ≤2).
        let sum_pct: usize = ["A", "B", "C"]
            .iter()
            .map(|k| dist.get(*k).copied().unwrap_or(0) * 100 / total)
            .sum();
        assert!((97..=100).contains(&sum_pct), "доли не сходятся: {sum_pct}");
    }

    /// Группа 5: правила уверенности — манифест без модели → B с основанием,
    /// подтверждённое ребро вызовов → A, неразрешённое имя из конфигурации →
    /// C-пробел без основания.
    #[test]
    fn confidence_rules_manifest_b_confirmed_edge_a_gap_c() {
        // База: компонент из манифеста без `model_id` и без confirmed-рёбер.
        let (_dir, _repo, base) = build_fixture();
        let api = base
            .elements
            .iter()
            .find(|e| e.graph_id.as_deref() == Some("dir:api"))
            .expect("нет элемента dir:api");
        assert_eq!(api.confidence, ConfidenceAbc::B, "манифест без модели → B");
        assert!(!api.evidence.is_empty(), "B обязан нести основание");

        // Вторая фикстура: `lb://ledger` разрешается точно (confirmed `calls`),
        // `lb://phantom-service` не разрешается (пробел `[gap]`).
        let (_dir2, _repo2, model) = build_with(confirmed_fixture);

        // A: конечная точка подтверждённого ребра вызовов.
        let ledger = model
            .elements
            .iter()
            .find(|e| e.graph_id.as_deref() == Some("dir:services/ledger"))
            .expect("нет элемента dir:services/ledger");
        assert_eq!(
            ledger.confidence,
            ConfidenceAbc::A,
            "подтверждённое ребро → A"
        );
        assert!(!ledger.evidence.is_empty(), "A обязан нести основание");

        // Оба конца ребра видны в ссылках: api (Container) → ledger (Component).
        let api_id = model
            .elements
            .iter()
            .find(|e| e.graph_id.as_deref() == Some("dir:api"))
            .map(|e| e.id.clone())
            .expect("нет api во второй фикстуре");
        assert!(
            model
                .relations
                .iter()
                .any(|r| r.from == api_id && r.to == ledger.id),
            "нет связи {api_id}→{}: {:?}",
            ledger.id,
            model
                .relations
                .iter()
                .map(|r| format!("{}→{}", r.from, r.to))
                .collect::<Vec<_>>()
        );

        // C-пробел: неразрешённое имя из конфигурации, без основания.
        let gap = model
            .elements
            .iter()
            .find(|e| e.props.get("source").is_some_and(|s| s == "gap"))
            .expect("нет элемента-пробела (source=gap)");
        assert_eq!(gap.confidence, ConfidenceAbc::C, "пробел → C");
        assert!(gap.evidence.is_empty(), "пробел не несёт основания");
        assert!(
            gap.title.contains("phantom-service"),
            "пробел не про phantom-service: {}",
            gap.title
        );
    }

    /// Группа 6: висячий элемент (orphan) — только `warn`, выдача не
    /// блокируется: в проверках нет `error`, `has_errors()` ложно.
    #[test]
    fn orphan_element_warns_without_blocking() {
        let (_dir, repo, mut model) = build_fixture();
        let snap = snapshot_at(&repo, &model.meta.rev).expect("снимок");
        assert!(!model.has_errors(), "база неожиданно красная");

        // C без основания и без ссылки ни в одном разделе → ровно orphan.
        model.elements.push(ReportElement {
            id: "CMP-98".to_string(),
            family: Family::Component,
            graph_id: None,
            title: "висячий компонент".to_string(),
            confidence: ConfidenceAbc::C,
            evidence: Vec::new(),
            props: BTreeMap::new(),
        });
        model.checks = check::check(&model, &snap, &repo);

        assert!(
            !model
                .checks
                .iter()
                .any(|f| f.severity == CheckSeverity::Error),
            "orphan не должен давать error: {:?}",
            model.checks
        );
        assert!(
            model
                .checks
                .iter()
                .any(|f| f.severity == CheckSeverity::Warn
                    && f.message.contains("orphan_element")
                    && f.element_id.as_deref() == Some("CMP-98")),
            "нет orphan warn про CMP-98: {:?}",
            model.checks
        );
        assert!(
            !model.has_errors(),
            "warn не должен блокировать выдачу (fail-closed только на error)"
        );
    }
}
