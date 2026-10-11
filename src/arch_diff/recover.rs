//! Команда `recover` (R5, ADR-069): полный граф «как построено» одной
//! ревизии — без диффа и без merge-base — плюс черновик `model/`
//! (`status: inferred`) и MCP-инструменты `recover`/`arch_diff`.
//!
//! Механика — та же, что у `arch-diff` ([`super::as_built`]), не вторая:
//! узлы (компоненты, внешние системы, хранилища, контракты, топики) и рёбра
//! (импорты, обращения, контрактные ссылки, вызовы) с основаниями
//! `файл:строка`. Детерминизм — сортировками движка: два прогона на одном
//! SHA дают байт-в-байт тот же JSON схемы [`RECOVER_SCHEMA`].
//!
//! Слой `[inferred]` отделён от `[confirmed]` (R6): `recover` не создаёт
//! `[confirmed]` — только черновик модели (`status: inferred`, `source:
//! recover@<sha>`), который архитектор подтверждает правкой. Повторный
//! прогон не затирает сущность со `status`, отличным от `inferred`.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use serde::Serialize;
use serde_json::{Value, json};

use super::as_built::{ComponentCtx, scan_revision};
use super::diff::{ArchDiffInput, arch_diff};
use super::report::{mermaid_id, mermaid_label, render_json, render_md, render_mermaid};
use super::types::{ArchGraph, ArchNode, NodeKind, RECOVER_SCHEMA};
use crate::control::DiffGlobs;
use crate::error::{HarnessError, Result};
use crate::imports::normalize_root;
use crate::llm::ToolSpec;
use crate::model::{EntityKind, parse_id};
use crate::tool::{Tool, ToolContext, ToolOutput};

/// Тело черновика компонента (ниже frontmatter).
const BODY_CMP: &str = "Выведено `arch-be recover` по манифесту сборки. Требует подтверждения архитектором\n\
     (подтверждение — смена `status` с `inferred` на `adopted`).";

/// Тело черновика системы.
const BODY_SYS: &str =
    "Восстановленная система: корень графа `recover`. Уточните назначение и владельца.";

/// Тело черновика интеграции (контракт).
const BODY_INT: &str = "Контракт найден в репозитории; интеграция выведена `arch-be recover`. Требует\n\
     подтверждения (направление обмена, владелец домена).";

/// Итог `recover`: граф ревизии и (если запрошен) черновик модели.
#[derive(Debug)]
pub struct RecoverOutcome {
    /// Полный граф «как построено» ревизии.
    pub graph: ArchGraph,
    /// Каталог черновика модели, если запрошен `--model-out`.
    pub model_dir: Option<PathBuf>,
    /// Id созданных/обновлённых сущностей (`inferred`).
    pub written: Vec<String>,
    /// Id пропущенных сущностей (`status` не `inferred` — правки архитектора).
    pub skipped: Vec<String>,
}

/// Строит полный граф ревизии `rev` (по умолчанию `HEAD`) репозитория `repo`
/// и — при `model_out` — черновик `model/` со `status: inferred`.
///
/// # Errors
/// Как у [`scan_revision`]; плюс ошибки записи черновика модели.
pub fn recover(
    repo: &Path,
    rev: Option<&str>,
    globs: &DiffGlobs,
    model_out: Option<&Path>,
) -> Result<RecoverOutcome> {
    let scan = scan_revision(repo, rev.unwrap_or("HEAD"), globs)?;
    // Частичный перенос публичных полей: `RevisionScan` без `Drop`.
    let graph = scan.graph;
    let components = scan.components;
    let (model_dir, written, skipped) = match model_out {
        Some(dir) => {
            let (w, s) = draft_model(dir, repo, &graph, &components)?;
            (Some(dir.to_path_buf()), w, s)
        }
        None => (None, Vec::new(), Vec::new()),
    };
    Ok(RecoverOutcome {
        graph,
        model_dir,
        written,
        skipped,
    })
}

/// Frontmatter черновика сущности: только известные модели поля
/// (`Frontmatter` модели — `deny_unknown_fields`).
#[derive(Serialize)]
struct DraftFront<'a> {
    id: &'a str,
    #[serde(rename = "type")]
    kind: &'a str,
    title: &'a str,
    status: &'a str,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    code_roots: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    contract: Option<String>,
    source: &'a str,
}

/// Нормализует корни кода: [`normalize_root`], `.` → пустой корень репозитория,
/// сортировка и дедупликация (стабильный ключ сопоставления).
fn norm_roots(raw: &[String]) -> Vec<String> {
    let mut out: Vec<String> = raw
        .iter()
        .map(|r| {
            let n = normalize_root(r);
            if n == "." { String::new() } else { n }
        })
        .collect();
    out.sort();
    out.dedup();
    out
}

/// ASCII-слаг заголовка для имени файла (русские заголовки → пустой слаг).
fn slug(title: &str) -> String {
    let mut s = String::new();
    let mut dash = false;
    for ch in title.chars() {
        if ch.is_ascii_alphanumeric() {
            s.push(ch.to_ascii_lowercase());
            dash = false;
        } else if !dash && !s.is_empty() {
            s.push('-');
            dash = true;
        }
    }
    while s.ends_with('-') {
        s.pop();
    }
    s
}

/// Путь файла черновика: существующий (переиспользование id) либо по слагу.
fn entity_path(dir: &Path, id: &str, slug: &str, existing: Option<&PathBuf>) -> PathBuf {
    if let Some(p) = existing {
        return p.clone();
    }
    let name = if slug.is_empty() {
        format!("{id}.md")
    } else {
        format!("{id}-{slug}.md")
    };
    dir.join(name)
}

/// Пишет файл сущности: YAML-frontmatter (`---`) + тело.
fn write_entity(path: &Path, front: &DraftFront<'_>, body: &str) -> Result<()> {
    let yaml = serde_yaml_ng::to_string(front)?;
    let content = format!("---\n{yaml}---\n\n{body}\n");
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| HarnessError::io(parent, e))?;
    }
    std::fs::write(path, content).map_err(|e| HarnessError::io(path, e))
}

/// Идемпотентно дописывает черновик `model/`: `SYS` (если нет), `CMP` по
/// выведенным компонентам (`code_roots`) и `INT` по контрактам (`contract`).
///
/// Сущность со `status`, отличным от `inferred`, НЕ трогается (правки
/// архитектора) — попадает в список пропущенных.
fn draft_model(
    dir: &Path,
    repo: &Path,
    graph: &ArchGraph,
    components: &[ComponentCtx],
) -> Result<(Vec<String>, Vec<String>)> {
    std::fs::create_dir_all(dir).map_err(|e| HarnessError::io(dir, e))?;
    let model = crate::model::load_model_tolerant(dir)?;

    let mut file_by_id: BTreeMap<String, PathBuf> = BTreeMap::new();
    let mut status_by_id: BTreeMap<String, String> = BTreeMap::new();
    let mut cmp_by_roots: BTreeMap<Vec<String>, String> = BTreeMap::new();
    let mut int_by_contract: BTreeMap<String, String> = BTreeMap::new();
    let mut has_sys = false;
    let mut max_cmp = 0u64;
    let mut max_int = 0u64;
    for e in &model.entities {
        file_by_id.insert(e.id.clone(), e.file.clone());
        status_by_id.insert(e.id.clone(), e.status.to_ascii_lowercase());
        if let Some((kind, n)) = parse_id(&e.id) {
            match kind {
                EntityKind::Cmp => max_cmp = max_cmp.max(n),
                EntityKind::Int => max_int = max_int.max(n),
                EntityKind::Sys => has_sys = true,
                _ => {}
            }
        }
        match e.kind {
            EntityKind::Cmp if !e.code_roots.is_empty() => {
                cmp_by_roots.insert(norm_roots(&e.code_roots), e.id.clone());
            }
            EntityKind::Int => {
                if let Some(c) = &e.contract {
                    int_by_contract.insert(c.clone(), e.id.clone());
                }
            }
            _ => {}
        }
    }

    let source = format!("recover@{}", graph.rev);
    let mut written: Vec<String> = Vec::new();
    let mut skipped: Vec<String> = Vec::new();

    // SYS: одна система на граф, только если модели системы ещё нет.
    if !has_sys {
        let id = "SYS-001".to_string();
        if status_by_id.get(&id).is_none_or(|s| s == "inferred") {
            let repo_name = repo
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("system");
            let title = format!("{repo_name} (восстановлено recover)");
            let front = DraftFront {
                id: &id,
                kind: "sys",
                title: &title,
                status: "inferred",
                code_roots: Vec::new(),
                contract: None,
                source: &source,
            };
            let path = entity_path(dir, &id, &slug(&title), file_by_id.get(&id));
            write_entity(&path, &front, BODY_SYS)?;
            written.push(id);
        } else {
            skipped.push(id);
        }
    }

    // CMP: по выведенным компонентам (без сущности модели), с переиспользованием
    // id по совпадению корней кода.
    for comp in components {
        if comp.model_id.is_some() {
            continue; // уже сущность модели — черновик не нужен
        }
        let key = norm_roots(&comp.roots);
        let id = if let Some(existing) = cmp_by_roots.get(&key) {
            existing.clone()
        } else {
            max_cmp += 1;
            format!("CMP-{max_cmp:03}")
        };
        if status_by_id.get(&id).is_some_and(|s| s != "inferred") {
            skipped.push(id);
            continue;
        }
        let title = comp.title.clone();
        let code_roots: Vec<String> = key
            .iter()
            .map(|r| {
                if r.is_empty() {
                    ".".to_string()
                } else {
                    r.clone()
                }
            })
            .collect();
        let front = DraftFront {
            id: &id,
            kind: "cmp",
            title: &title,
            status: "inferred",
            code_roots,
            contract: None,
            source: &source,
        };
        let path = entity_path(dir, &id, &slug(&title), file_by_id.get(&id));
        write_entity(&path, &front, BODY_CMP)?;
        cmp_by_roots.insert(key, id.clone());
        status_by_id.insert(id.clone(), "inferred".to_string());
        written.push(id);
    }

    // INT: по контрактам графа, с переиспользованием id по пути контракта.
    for node in &graph.nodes {
        if node.kind != NodeKind::Contract {
            continue;
        }
        let path = node.title.clone();
        let id = if let Some(existing) = int_by_contract.get(&path) {
            existing.clone()
        } else {
            max_int += 1;
            format!("INT-{max_int:03}")
        };
        if status_by_id.get(&id).is_some_and(|s| s != "inferred") {
            skipped.push(id);
            continue;
        }
        let front = DraftFront {
            id: &id,
            kind: "int",
            title: &path,
            status: "inferred",
            code_roots: Vec::new(),
            contract: Some(path.clone()),
            source: &source,
        };
        let file = entity_path(dir, &id, &slug(&path), file_by_id.get(&id));
        write_entity(&file, &front, BODY_INT)?;
        int_by_contract.insert(path, id.clone());
        status_by_id.insert(id.clone(), "inferred".to_string());
        written.push(id);
    }

    Ok((written, skipped))
}

/// Машинный контракт `arch-be/recover/v1` (ADR-069): `schema` + версия бинаря
/// + сериализованный граф ревизии. Детерминизм — сортировками движка.
///
/// # Errors
/// Сериализация графа (практически недостижимо — структуры плоские).
pub fn render_recover_json(graph: &ArchGraph) -> Result<String> {
    let mut map = serde_json::Map::new();
    map.insert(
        "schema".to_string(),
        serde_json::Value::String(RECOVER_SCHEMA.to_string()),
    );
    map.insert(
        "arch_be".to_string(),
        serde_json::Value::String(env!("CARGO_PKG_VERSION").to_string()),
    );
    if let serde_json::Value::Object(m) = serde_json::to_value(graph)? {
        map.extend(m);
    }
    Ok(serde_json::to_string_pretty(&serde_json::Value::Object(
        map,
    ))?)
}

/// Человекочитаемый отчёт: сводка, узлы по видам, рёбра, пробелы.
#[must_use]
pub fn render_recover_md(graph: &ArchGraph) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "# Граф ревизии (recover)\n");
    let _ = writeln!(out, "Ревизия: `{}`", graph.rev);
    let _ = writeln!(
        out,
        "Узлов: {} (компонент {}, внешних систем {}, хранилищ {}, контрактов {}, топиков {})",
        graph.nodes.len(),
        graph.count_kind(NodeKind::Component),
        graph.count_kind(NodeKind::ExternalSystem),
        graph.count_kind(NodeKind::Datastore),
        graph.count_kind(NodeKind::Contract),
        graph.count_kind(NodeKind::Topic),
    );
    let _ = writeln!(out, "Рёбер: {}\n", graph.edges.len());

    for kind in [
        NodeKind::Component,
        NodeKind::ExternalSystem,
        NodeKind::Datastore,
        NodeKind::Contract,
        NodeKind::Topic,
    ] {
        let nodes: Vec<&ArchNode> = graph.nodes.iter().filter(|n| n.kind == kind).collect();
        if nodes.is_empty() {
            continue;
        }
        let _ = writeln!(out, "## {} ({})", kind.label(), nodes.len());
        for n in nodes {
            let inferred = if n.inferred { " (выведен)" } else { "" };
            let model = n
                .model_id
                .as_deref()
                .map_or_else(String::new, |m| format!(" → {m}"));
            let _ = writeln!(out, "- `{}` {}{inferred}{model}", n.id, n.title);
        }
        let _ = writeln!(out);
    }

    if !graph.edges.is_empty() {
        let _ = writeln!(out, "## Рёбра ({})", graph.edges.len());
        for e in &graph.edges {
            let via = e
                .via
                .map_or_else(String::new, |v| format!(" · {}", v.label()));
            let ev = match e.evidence.first() {
                Some(first) => {
                    let extra = e.evidence.len().saturating_sub(1);
                    if extra > 0 {
                        format!("  ({first} +{extra})")
                    } else {
                        format!("  ({first})")
                    }
                }
                None => String::new(),
            };
            let _ = writeln!(
                out,
                "- `{}` --{}{via}--> `{}`{ev}",
                e.from,
                e.kind.label(),
                e.to
            );
        }
        let _ = writeln!(out);
    }

    if !graph.gaps.is_empty() {
        let _ = writeln!(out, "## Пробелы ([gap], {})", graph.gaps.len());
        for g in &graph.gaps {
            let _ = writeln!(out, "- {g}");
        }
        let _ = writeln!(out);
    }
    out
}

/// Mermaid-flowchart полного графа ревизии (стиль `report::render_mermaid`).
#[must_use]
pub fn render_recover_mermaid(graph: &ArchGraph) -> String {
    let mut out = String::from("flowchart LR\n");
    let _ = writeln!(out, "  %% ревизия {}", graph.rev);
    for n in &graph.nodes {
        let _ = writeln!(out, "  {}[\"{}\"]", mermaid_id(&n.id), mermaid_label(n));
    }
    for e in &graph.edges {
        let via = e
            .via
            .map(|v| format!(" · {}", v.label()))
            .unwrap_or_default();
        let _ = writeln!(
            out,
            "  {} -->|\"{}{via}\"| {}",
            mermaid_id(&e.from),
            e.kind.label(),
            mermaid_id(&e.to)
        );
    }
    out
}

/// Structurizr-DSL-представление графа (детерминированное: порядок узлов/рёбер
/// из графа).
#[must_use]
pub fn render_structurizr(graph: &ArchGraph) -> String {
    let mut out = String::new();
    let mut used: BTreeSet<String> = BTreeSet::new();
    let mut alias: BTreeMap<&str, String> = BTreeMap::new();
    for n in &graph.nodes {
        alias.insert(n.id.as_str(), unique_ident(&n.id, &mut used));
    }
    let _ = writeln!(out, "workspace \"recover {}\" {{", esc(&graph.rev));
    let _ = writeln!(out, "    model {{");
    for n in &graph.nodes {
        let a = alias.get(n.id.as_str()).map_or("_", String::as_str);
        let _ = writeln!(
            out,
            "        {a} = {} \"{}\" \"{}\"",
            structurizr_kw(n.kind),
            esc(&n.title),
            esc(&n.id)
        );
    }
    for e in &graph.edges {
        if let (Some(f), Some(t)) = (alias.get(e.from.as_str()), alias.get(e.to.as_str())) {
            let via = e
                .via
                .map_or_else(String::new, |v| format!(" · {}", v.label()));
            let _ = writeln!(out, "        {f} -> {t} \"{}{via}\"", e.kind.label());
        }
    }
    let _ = writeln!(out, "    }}");
    let _ = writeln!(out, "    views {{");
    let _ = writeln!(out, "    }}");
    let _ = writeln!(out, "}}");
    out
}

/// C4-PlantUML-представление графа (детерминированное).
#[must_use]
pub fn render_c4_plantuml(graph: &ArchGraph) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "@startuml");
    let _ = writeln!(out, "!include <C4/C4_Container>");
    let _ = writeln!(out, "title recover {}", esc(&graph.rev));
    let mut used: BTreeSet<String> = BTreeSet::new();
    let mut alias: BTreeMap<&str, String> = BTreeMap::new();
    for n in &graph.nodes {
        let a = unique_ident(&n.id, &mut used);
        alias.insert(n.id.as_str(), a.clone());
        let _ = writeln!(
            out,
            "{}({}, \"{}\", \"{}\")",
            c4_macro(n.kind),
            a,
            esc(&n.title),
            esc(&n.id)
        );
    }
    for e in &graph.edges {
        if let (Some(f), Some(t)) = (alias.get(e.from.as_str()), alias.get(e.to.as_str())) {
            let via = e
                .via
                .map_or_else(String::new, |v| format!(" · {}", v.label()));
            let _ = writeln!(out, "Rel({f}, {t}, \"{}{via}\")", e.kind.label());
        }
    }
    let _ = writeln!(out, "@enduml");
    out
}

/// Ключевое слово элемента Structurizr по виду узла.
fn structurizr_kw(kind: NodeKind) -> &'static str {
    match kind {
        NodeKind::ExternalSystem => "softwareSystem",
        NodeKind::Component | NodeKind::Datastore | NodeKind::Contract | NodeKind::Topic => {
            "container"
        }
    }
}

/// Макрос C4-PlantUML по виду узла.
fn c4_macro(kind: NodeKind) -> &'static str {
    match kind {
        NodeKind::ExternalSystem => "System_Ext",
        NodeKind::Datastore => "ContainerDb",
        NodeKind::Topic => "ContainerQueue",
        NodeKind::Component | NodeKind::Contract => "Container",
    }
}

/// Идентификатор для Structurizr/PlantUML: только `[A-Za-z0-9_]`, ведущая
/// цифра сдвигается подчёркиванием.
fn ident(id: &str) -> String {
    let mut s = String::new();
    for ch in id.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            s.push(ch);
        } else {
            s.push('_');
        }
    }
    if s.is_empty() {
        s.push('_');
    }
    if s.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        s.insert(0, '_');
    }
    s
}

/// Уникализирует [`ident`] в пределах графа (детерминированно по порядку узлов).
fn unique_ident(id: &str, used: &mut BTreeSet<String>) -> String {
    let base = ident(id);
    if used.insert(base.clone()) {
        return base;
    }
    let mut i = 2u32;
    loop {
        let cand = format!("{base}_{i}");
        if used.insert(cand.clone()) {
            return cand;
        }
        i += 1;
    }
}

/// Экранирует двойные кавычки для строк DSL.
fn esc(s: &str) -> String {
    s.replace('"', "'")
}

/// Инструменты домена `recover`: `recover` и `arch_diff` (оба read-only).
#[must_use]
pub fn tools() -> Vec<Arc<dyn Tool>> {
    vec![Arc::new(RecoverTool), Arc::new(ArchDiffTool)]
}

/// Инструмент агента: `recover` — полный граф ревизии (read-only).
pub struct RecoverTool;

#[async_trait]
impl Tool for RecoverTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "recover".into(),
            description: "Полный граф архитектуры ревизии (as-built) без диффа и merge-base \
                (R5, ADR-069): узлы (компоненты, внешние системы, хранилища, контракты, топики) \
                и рёбра (импорты, обращения, контрактные ссылки, вызовы) с основаниями \
                файл:строка. Форматы: md | json (схема arch-be/recover/v1) | mermaid | \
                structurizr | c4-plantuml. Read-only: рабочее дерево не изменяется, черновик \
                модели не пишется (для черновика — CLI `arch-be recover --model-out`). \
                Детерминизм: два прогона на одном SHA дают байт-в-байт тот же JSON."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Каталог репозитория (относительно cwd или абсолютный)"
                    },
                    "rev": {
                        "type": "string",
                        "description": "Ревизия (ветка/тег/sha; по умолчанию HEAD)"
                    },
                    "format": {
                        "type": "string",
                        "description": "md | json | mermaid | structurizr | c4-plantuml (по умолчанию json)"
                    }
                },
                "required": ["path"]
            }),
        }
    }

    async fn call(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let Some(repo) = args
            .get("path")
            .or_else(|| args.get("repo"))
            .and_then(Value::as_str)
        else {
            return Ok(ToolOutput::err(
                "recover: обязательный аргумент 'path' (string) отсутствует",
            ));
        };
        let repo = ctx.resolve(repo);
        let rev = args.get("rev").and_then(Value::as_str);
        let format = args.get("format").and_then(Value::as_str).unwrap_or("json");
        let globs = ctx.config.significance.diff_globs();
        let outcome = match recover(&repo, rev, &globs, None) {
            Ok(o) => o,
            Err(e) => return Ok(ToolOutput::err(format!("recover: {e}"))),
        };
        let graph = &outcome.graph;
        let text = match format {
            "md" => render_recover_md(graph),
            "mermaid" => render_recover_mermaid(graph),
            "structurizr" => render_structurizr(graph),
            "c4-plantuml" => render_c4_plantuml(graph),
            _ => match render_recover_json(graph) {
                Ok(s) => s,
                Err(e) => return Ok(ToolOutput::err(format!("recover: {e}"))),
            },
        };
        Ok(ToolOutput::ok(text))
    }
}

/// Инструмент агента: `arch_diff` — дифф двух ревизий (read-only).
pub struct ArchDiffTool;

#[async_trait]
impl Tool for ArchDiffTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "arch_diff".into(),
            description: "Архитектурный дифф PR (K1/K2, ADR-063): что изменилось в системе между \
                ревизиями — связи компонентов, внешние системы, хранилища, контракты, NFR, \
                задетые инварианты, предложение правки модели. Формат json — схема \
                arch-be/arch-diff/v1. Read-only: рабочее дерево не изменяется."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Корень репозитория (относительно cwd или абсолютный)"
                    },
                    "base": {
                        "type": "string",
                        "description": "База диффа (ветка/тег/sha или диапазон A...B)"
                    },
                    "head": {
                        "type": "string",
                        "description": "Голова диффа (по умолчанию HEAD)"
                    },
                    "format": {
                        "type": "string",
                        "description": "md | json | mermaid | sarif (по умолчанию json)"
                    }
                },
                "required": ["path", "base"]
            }),
        }
    }

    async fn call(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let Some(repo) = args
            .get("path")
            .or_else(|| args.get("repo"))
            .and_then(Value::as_str)
        else {
            return Ok(ToolOutput::err(
                "arch_diff: обязательный аргумент 'path' (string) отсутствует",
            ));
        };
        let Some(base) = args.get("base").and_then(Value::as_str) else {
            return Ok(ToolOutput::err(
                "arch_diff: обязательный аргумент 'base' (string) отсутствует",
            ));
        };
        let repo = ctx.resolve(repo);
        let head = args.get("head").and_then(Value::as_str);
        let format = args.get("format").and_then(Value::as_str).unwrap_or("json");
        let globs = ctx.config.significance.diff_globs();
        let (fast_max, standard_max) = match ctx.config.significance.limits() {
            Ok(l) => l,
            Err(e) => return Ok(ToolOutput::err(format!("arch_diff: {e}"))),
        };
        let diff = match arch_diff(
            &repo,
            &ArchDiffInput {
                base,
                head,
                declared: BTreeMap::new(),
                limits: (fast_max, standard_max),
                globs: &globs,
            },
        ) {
            Ok(d) => d,
            Err(e) => return Ok(ToolOutput::err(format!("arch_diff: {e}"))),
        };
        let text = match format {
            "md" => render_md(&diff),
            "mermaid" => {
                let (base_g, head_g) = match (
                    super::as_built::as_built_with(&repo, &diff.base, &globs),
                    super::as_built::as_built_with(&repo, &diff.head, &globs),
                ) {
                    (Ok(b), Ok(h)) => (b, h),
                    (Err(e), _) | (_, Err(e)) => {
                        return Ok(ToolOutput::err(format!("arch_diff: {e}")));
                    }
                };
                render_mermaid(&base_g, &head_g, &diff)
            }
            "sarif" => super::report::render_sarif(&diff),
            _ => match render_json(&diff) {
                Ok(s) => s,
                Err(e) => return Ok(ToolOutput::err(format!("arch_diff: {e}"))),
            },
        };
        Ok(ToolOutput::ok(text))
    }
}

#[cfg(test)]
mod tests {
    use super::super::snapshot::tests::{git_repo, write_file};
    use super::*;
    use crate::arch_diff::EdgeKind;

    /// Фикстура: компонент-каталог с манифестом + контракт + хранилище из
    /// конфига (строка подключения детектируется только в конфиг-файле).
    fn fixture(repo: &Path) {
        write_file(repo, "Cargo.toml", "[package]\nname = \"bank\"\n");
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
        write_file(repo, "contracts/ledger.yaml", "openapi: 3.0.0\n");
    }

    #[test]
    fn recover_json_is_byte_deterministic() {
        let dir = tempfile::tempdir().expect("tmp");
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).expect("mkdir");
        fixture(&repo);
        git_repo(&repo);

        let globs = DiffGlobs::default();
        let a = recover(&repo, Some("HEAD"), &globs, None).expect("recover #1");
        let b = recover(&repo, Some("HEAD"), &globs, None).expect("recover #2");
        let ja = render_recover_json(&a.graph).expect("json #1");
        let jb = render_recover_json(&b.graph).expect("json #2");
        assert_eq!(ja, jb, "JSON recover не детерминирован");
        assert!(ja.contains(RECOVER_SCHEMA), "нет схемы recover: {ja}");
        assert!(ja.contains("\"nodes\""), "нет узлов: {ja}");
        assert!(ja.contains("\"edges\""), "нет рёбер: {ja}");
    }

    #[test]
    fn recover_finds_component_datastore_contract() {
        let dir = tempfile::tempdir().expect("tmp");
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).expect("mkdir");
        fixture(&repo);
        git_repo(&repo);

        let graph = recover(&repo, Some("HEAD"), &DiffGlobs::default(), None)
            .expect("recover")
            .graph;
        assert!(
            graph.count_kind(NodeKind::Component) >= 1,
            "нет компонента: {graph:?}"
        );
        assert_eq!(graph.count_kind(NodeKind::Datastore), 1, "{graph:?}");
        assert_eq!(graph.count_kind(NodeKind::Contract), 1, "{graph:?}");
        assert!(
            graph.edges.iter().any(|e| e.kind == EdgeKind::Connect),
            "нет ребра connect: {graph:?}"
        );
    }

    #[test]
    fn model_out_drafts_inferred_and_preserves_edits() {
        let dir = tempfile::tempdir().expect("tmp");
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).expect("mkdir");
        fixture(&repo);
        git_repo(&repo);

        let model_out = dir.path().join("model");
        let globs = DiffGlobs::default();
        let first = recover(&repo, Some("HEAD"), &globs, Some(&model_out)).expect("recover #1");
        assert!(!first.written.is_empty(), "черновик пуст: {first:?}");
        assert!(
            first.written.iter().any(|id| id.starts_with("CMP-")),
            "нет CMP: {:?}",
            first.written
        );
        assert!(model_out.join("SYS-001.md").exists() || first.written.contains(&"SYS-001".into()));

        // Идемпотентность: повторный прогон переписывает только [inferred].
        let second = recover(&repo, Some("HEAD"), &globs, Some(&model_out)).expect("recover #2");
        assert!(second.skipped.is_empty(), "ложные пропуски: {second:?}");

        // Правка архитектора: status adopted → сущность не трогается.
        let edited_id = first
            .written
            .iter()
            .find(|id| id.starts_with("CMP-"))
            .cloned()
            .expect("id CMP черновика");
        let edited = std::fs::read_dir(&model_out)
            .expect("read_dir")
            .filter_map(std::result::Result::ok)
            .map(|e| e.path())
            .find(|p| {
                std::fs::read_to_string(p).is_ok_and(|t| t.contains(&format!("id: {edited_id}")))
            })
            .expect("файл CMP черновика");
        let text = std::fs::read_to_string(&edited).expect("read");
        let adopted = text.replace("status: inferred", "status: adopted");
        std::fs::write(&edited, adopted).expect("write adopted");

        let third = recover(&repo, Some("HEAD"), &globs, Some(&model_out)).expect("recover #3");
        assert!(
            third.skipped.contains(&edited_id),
            "правка архитектора не защищена: skipped={:?} (id={edited_id})",
            third.skipped
        );
        let after = std::fs::read_to_string(&edited).expect("read after");
        assert!(after.contains("status: adopted"), "правка затёрта: {after}");
    }
}
