//! Ландшафт систем (EA-3, ADR-037): агрегация моделей `model/` набора проектов.
//!
//! Ответ на замечание ревью «спайн живёт внутри проекта; нет связи с
//! ландшафтом и реестром систем» — в той же механике, что реестр ADR
//! (ADR-036): read-only агрегация файлов по ROOT + непосредственным
//! подкаталогам (ADR-033: реестры — агрегация файлов, не сервис).
//! Дедупликация — по НОРМАЛИЗОВАННОМУ ИМЕНИ системы, БЕЗ глобального
//! пространства ID (глобальные ID инвазивны: потребовали бы переписывания
//! существующих `model/` всех проектов). Опциональная карта АЛИАСОВ
//! (`--aliases`, yaml/json «алиас → каноничное имя») склеивает известные
//! варианты написания до дедупликации (обе стороны нормализуются тем же
//! [`canonical_name`]).
//!
//! Команда `arch-be model landscape <ROOT> [--mermaid] [--aliases <file>]
//! [--diff-since <ref|date>]`:
//! - реестр систем: каноническое имя (lowercase, схлопывание пробелов и
//!   дефисов) → проекты/id/статусы всех вхождений (дедуп-витрина);
//! - находки: `id-divergence` (одна система — разные id в разных проектах),
//!   `status-conflict` (расхождение статусов), `dangling-ref` (связь
//!   `depends_on`/`implements`/`affects` на id, отсутствующий во всём
//!   наборе), `cross-project-link` (связь на систему другого проекта —
//!   показываемый положительный факт, не ошибка);
//! - топ-5 систем по связности (вход+исход); `--mermaid` — `graph TD`
//!   канонических систем;
//! - `--diff-since` — дифф ландшафта против версии в git: ссылка
//!   (ветка/тег/sha) или дата `YYYY-MM-DD` (последний коммит не позже конца
//!   этого дня). Состояние «тогда» материализуется во временный каталог
//!   (`git ls-tree` + `git show` файлов `model/*.md`) и агрегируется тем же
//!   кодом; в диффе — появившиеся/исчезнувшие/изменившиеся системы (смена
//!   статуса или набора id). Текущее состояние — РАБОЧЕЕ ДЕРЕВО (не HEAD).

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::error::{HarnessError, Result};
use crate::llm::ToolSpec;
use crate::model::{EntityKind, LinkKind};
use crate::tool::{Tool, ToolContext, ToolOutput};

/// Каталоги, которые не считаются проектами при сканировании `ROOT`.
const SKIP_DIRS: [&str; 3] = [".git", "target", "node_modules"];

/// Виды связей, учитываемых ландшафтом (`verified_by` — про правила, не про
/// ландшафт систем).
const LINK_KINDS: [LinkKind; 3] = [LinkKind::DependsOn, LinkKind::Implements, LinkKind::Affects];

/// Вхождение системы: сущность конкретного проекта.
#[derive(Debug, Clone)]
pub struct Occurrence {
    /// Проект (имя подкаталога ROOT; сам ROOT — имя его каталога).
    pub project: String,
    /// Идентификатор сущности (`SYS-001`, `INT-007`).
    pub id: String,
    /// Заголовок сущности (как записан в проекте).
    pub title: String,
    /// Статус сущности.
    pub status: String,
    /// Тип сущности (`SYS` или `INT`).
    pub kind: EntityKind,
}

/// Каноническая система ландшафта (дедуп по нормализованному имени).
#[derive(Debug, Clone)]
pub struct LandscapeSystem {
    /// Каноническая форма имени (ключ дедупликации).
    pub canonical: String,
    /// Представительное имя (title первого вхождения).
    pub display: String,
    /// Вхождения (сортировка: проект, id).
    pub occurrences: Vec<Occurrence>,
}

/// Находка ландшафта.
#[derive(Debug, Clone)]
pub struct LandscapeFinding {
    /// Тип: `id-divergence` | `status-conflict` | `dangling-ref` |
    /// `cross-project-link`.
    pub kind: String,
    /// Описание.
    pub message: String,
}

/// Отчёт ландшафта систем.
#[derive(Debug)]
pub struct LandscapeReport {
    /// Сканированный корень.
    pub root: PathBuf,
    /// Реестр систем (сортировка по каноническому имени).
    pub systems: Vec<LandscapeSystem>,
    /// Находки.
    pub findings: Vec<LandscapeFinding>,
    /// Рёбра ландшафта: (канон. источник, канон. цель), отсортированные и
    /// дедуплицированные (для mermaid и топа связности).
    pub edges: Vec<(String, String)>,
    /// Топ связности: (представительное имя, число связей вход+исход).
    pub top: Vec<(String, usize)>,
}

/// Нормализация имени системы: lowercase, дефисы → пробелы, схлопывание
/// пробелов. Ключ дедупликации («Процессинг» == «процессинг» ==
/// «Процессинг  СБОЛ» по пробелам).
/// `pub(crate)`: тот же ключ использует импорт реестров (`model::registry`)
/// для идемпотентного сопоставления по названию.
pub(crate) fn canonical_name(title: &str) -> String {
    title
        .trim()
        .to_lowercase()
        .replace('-', " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Имя проекта по каталогу (для ROOT — имя самого каталога).
fn project_name(dir: &Path) -> String {
    dir.canonicalize()
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_else(|| ".".to_string())
}

/// Собирает сущности проекта с меткой проекта; `None` — нет `model/`
/// (проект молча пропускается).
fn load_project(dir: &Path) -> Result<Option<(String, crate::model::Model)>> {
    let model_dir = dir.join("model");
    if !model_dir.is_dir() {
        return Ok(None);
    }
    let model = crate::model::load_model(&model_dir)?;
    Ok(Some((project_name(dir), model)))
}

/// Строит ландшафт систем по `ROOT`: сам каталог + непосредственные
/// подкаталоги с `model/`.
///
/// # Errors
/// `ROOT` не каталог; ни в самом `ROOT`, ни в подкаталогах нет ни одного
/// `model/` — понятная ошибка; модель проекта не разбирается.
pub fn build_landscape(root: &Path) -> Result<LandscapeReport> {
    build_landscape_with_aliases(root, &BTreeMap::new())
}

/// Строит ландшафт систем с картой алиасов (нормализованное имя-вариант →
/// нормализованное каноническое имя): дедупликация учитывает алиасы.
///
/// # Errors
/// См. [`build_landscape`].
pub fn build_landscape_with_aliases(
    root: &Path,
    aliases: &BTreeMap<String, String>,
) -> Result<LandscapeReport> {
    if !root.is_dir() {
        return Err(HarnessError::Model(format!(
            "ландшафт: корень недоступен: {}",
            root.display()
        )));
    }

    let mut projects: Vec<(String, crate::model::Model)> = Vec::new();
    if let Some(p) = load_project(root)? {
        projects.push(p);
    }
    let mut dirs: Vec<PathBuf> = Vec::new();
    let rd = std::fs::read_dir(root).map_err(|e| HarnessError::io(root, e))?;
    for entry in rd {
        let entry = entry.map_err(|e| HarnessError::io(root, e))?;
        let p = entry.path();
        if !p.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if SKIP_DIRS.contains(&name.as_str()) {
            continue;
        }
        dirs.push(p);
    }
    dirs.sort();
    for d in &dirs {
        if let Some(p) = load_project(d)? {
            projects.push(p);
        }
    }

    if projects.is_empty() {
        return Err(HarnessError::Model(format!(
            "ландшафт: ни в {}, ни в его непосредственных подкаталогах не найдено \
             каталога model/ — нечего агрегировать",
            root.display()
        )));
    }

    // Глобальный индекс id → (проект, сущность) по всем проектам набора
    // (разрешение связей — сквозное: dangling/cross-project определяются
    // против ВСЕГО набора, а не локальной модели).
    let mut global: BTreeMap<&str, Vec<(&str, &crate::model::Entity)>> = BTreeMap::new();
    for (project, model) in &projects {
        for e in &model.entities {
            global
                .entry(e.id.as_str())
                .or_default()
                .push((project.as_str(), e));
        }
    }

    // Реестр систем: только SYS/INT, дедуп по каноническому имени
    // (алиасы склеивают известные варианты написания до дедупликации).
    let mut by_canonical: BTreeMap<String, Vec<Occurrence>> = BTreeMap::new();
    for (project, model) in &projects {
        for e in &model.entities {
            if !matches!(e.kind, EntityKind::Sys | EntityKind::Int) {
                continue;
            }
            let raw = canonical_name(&e.title);
            let canonical = aliases.get(&raw).cloned().unwrap_or(raw);
            if canonical.is_empty() {
                continue;
            }
            by_canonical.entry(canonical).or_default().push(Occurrence {
                project: project.clone(),
                id: e.id.clone(),
                title: e.title.trim().to_string(),
                status: e.status.clone(),
                kind: e.kind,
            });
        }
    }
    let mut systems: Vec<LandscapeSystem> = by_canonical
        .into_iter()
        .map(|(canonical, mut occurrences)| {
            occurrences.sort_by(|a, b| (&a.project, &a.id).cmp(&(&b.project, &b.id)));
            // Представительное имя — заголовок первого вхождения.
            let display = occurrences
                .first()
                .map_or_else(|| canonical.clone(), |o| o.title.clone());
            LandscapeSystem {
                canonical,
                display,
                occurrences,
            }
        })
        .collect();
    systems.sort_by(|a, b| a.canonical.cmp(&b.canonical));
    let canonical_of: BTreeMap<&str, &str> = systems
        .iter()
        .flat_map(|s| {
            s.occurrences
                .iter()
                .map(move |o| (o.id.as_str(), s.canonical.as_str()))
        })
        .collect();

    let mut findings = Vec::new();

    // id-divergence / status-conflict по реестру.
    for s in &systems {
        let projects: BTreeSet<&str> = s.occurrences.iter().map(|o| o.project.as_str()).collect();
        let ids: BTreeSet<&str> = s.occurrences.iter().map(|o| o.id.as_str()).collect();
        let whereabouts = s
            .occurrences
            .iter()
            .map(|o| format!("{} {} ({})", o.project, o.id, o.status))
            .collect::<Vec<_>>()
            .join(", ");
        if projects.len() > 1 && ids.len() > 1 {
            findings.push(LandscapeFinding {
                kind: "id-divergence".to_string(),
                message: format!(
                    "«{}» — разные id в разных проектах: {whereabouts}",
                    s.display
                ),
            });
        }
        let statuses: BTreeSet<String> = s
            .occurrences
            .iter()
            .map(|o| o.status.trim().to_lowercase())
            .collect();
        if statuses.len() > 1 {
            findings.push(LandscapeFinding {
                kind: "status-conflict".to_string(),
                message: format!("«{}» — статусы расходятся: {whereabouts}", s.display),
            });
        }
    }

    // Связи: dangling-ref / cross-project-link + рёбра ландшафта.
    let mut edges: BTreeSet<(String, String)> = BTreeSet::new();
    let mut seen_dangling: BTreeSet<(String, String, String)> = BTreeSet::new();
    let mut seen_cross: BTreeSet<(String, String, String)> = BTreeSet::new();
    for (project, model) in &projects {
        for e in &model.entities {
            for kind in LINK_KINDS {
                for target in e.link_targets(kind) {
                    match global.get(target.as_str()) {
                        None => {
                            // dangling-ref: цели нет ни в одном проекте набора.
                            if seen_dangling.insert((
                                e.id.clone(),
                                kind.field_name().to_string(),
                                target.clone(),
                            )) {
                                findings.push(LandscapeFinding {
                                    kind: "dangling-ref".to_string(),
                                    message: format!(
                                        "{project} {} -[{}]-> {target}: цель отсутствует \
                                         во всех моделях набора",
                                        e.id,
                                        kind.field_name()
                                    ),
                                });
                            }
                        }
                        Some(targets) => {
                            for (t_project, t_entity) in targets {
                                // cross-project-link: цель — система (SYS/INT)
                                // ДРУГОГО проекта. Положительный факт, не ошибка.
                                if *t_project == project.as_str()
                                    || !matches!(t_entity.kind, EntityKind::Sys | EntityKind::Int)
                                {
                                    continue;
                                }
                                if seen_cross.insert((
                                    e.id.clone(),
                                    target.clone(),
                                    (*t_project).to_string(),
                                )) {
                                    findings.push(LandscapeFinding {
                                        kind: "cross-project-link".to_string(),
                                        message: format!(
                                            "{project} {} -[{}]-> {target} (проект {t_project}, «{}»)",
                                            e.id,
                                            kind.field_name(),
                                            t_entity.title.trim()
                                        ),
                                    });
                                }
                            }
                        }
                    }
                    // Ребро ландшафта — только между системами (SYS/INT),
                    // разрешёнными в канонические имена.
                    let (Some(src_canonical), Some(dst_canonical)) = (
                        canonical_of
                            .get(e.id.as_str())
                            .filter(|_| matches!(e.kind, EntityKind::Sys | EntityKind::Int)),
                        canonical_of.get(target.as_str()),
                    ) else {
                        continue;
                    };
                    if src_canonical != dst_canonical {
                        edges.insert(((*src_canonical).to_string(), (*dst_canonical).to_string()));
                    }
                }
            }
        }
    }

    // Топ связности: вход+исход по рёбрам ландшафта.
    let mut degree: BTreeMap<&str, usize> = BTreeMap::new();
    for (src, dst) in &edges {
        *degree.entry(src.as_str()).or_default() += 1;
        *degree.entry(dst.as_str()).or_default() += 1;
    }
    let mut ranked: Vec<(String, String, usize)> = systems
        .iter()
        .map(|s| {
            (
                s.canonical.clone(),
                s.display.clone(),
                degree.get(s.canonical.as_str()).copied().unwrap_or(0),
            )
        })
        .collect();
    ranked.sort_by(|a, b| b.2.cmp(&a.2).then(a.0.cmp(&b.0)));
    let top: Vec<(String, usize)> = ranked
        .into_iter()
        .take(5)
        .map(|(_, display, n)| (display, n))
        .collect();

    Ok(LandscapeReport {
        root: root.to_path_buf(),
        systems,
        findings,
        edges: edges.into_iter().collect(),
        top,
    })
}

/// Рендер отчёта в markdown: реестр систем, находки, топ связности.
#[must_use]
pub fn render_markdown(report: &LandscapeReport) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "# Ландшафт систем: {}\n", report.root.display());
    let _ = writeln!(out, "## Реестр систем\n");
    let _ = writeln!(out, "| Система | Проекты и ID | Статусы |");
    let _ = writeln!(out, "|---|---|---|");
    for s in &report.systems {
        let ids = s
            .occurrences
            .iter()
            .map(|o| format!("{}: {}", o.project, o.id))
            .collect::<Vec<_>>()
            .join("; ");
        let statuses: BTreeSet<&str> = s.occurrences.iter().map(|o| o.status.as_str()).collect();
        let _ = writeln!(
            out,
            "| {} | {} | {} |",
            s.display,
            ids,
            statuses.into_iter().collect::<Vec<_>>().join(", ")
        );
    }
    let _ = writeln!(out, "\n## Находки\n");
    if report.findings.is_empty() {
        let _ = writeln!(out, "- нет");
    } else {
        for f in &report.findings {
            let _ = writeln!(out, "- [{}] {}", f.kind, f.message);
        }
    }
    let _ = writeln!(out, "\n## Топ связности\n");
    for (i, (name, n)) in report.top.iter().enumerate() {
        let _ = writeln!(out, "{}. {name} — {n} связей", i + 1);
    }
    let _ = write!(
        out,
        "\nИтого: {} систем, {} связей, {} находок",
        report.systems.len(),
        report.edges.len(),
        report.findings.len()
    );
    out
}

/// Санитизация метки узла mermaid: кавычки и скобки — в безопасные символы.
fn mermaid_label(text: &str) -> String {
    text.replace(['"', '[', ']', '|'], "'")
}

/// Рендер ландшафта в mermaid `graph TD`: узлы — канонические системы
/// (при id-divergence — с перечнем проектов), рёбра — связи. Идентификаторы
/// узлов — санитизированные `s1..sN` в порядке канонических имён.
#[must_use]
pub fn render_mermaid(report: &LandscapeReport) -> String {
    let mut out = String::from("graph TD\n");
    let mut node_ids: BTreeMap<&str, String> = BTreeMap::new();
    for (i, s) in report.systems.iter().enumerate() {
        let node = format!("s{}", i + 1);
        node_ids.insert(s.canonical.as_str(), node.clone());
        let projects: BTreeSet<&str> = s.occurrences.iter().map(|o| o.project.as_str()).collect();
        let ids: BTreeSet<&str> = s.occurrences.iter().map(|o| o.id.as_str()).collect();
        // Метка проекта — только при id-divergence (разные id у одной системы).
        let label = if projects.len() > 1 && ids.len() > 1 {
            format!(
                "{} ({})",
                s.display,
                projects.into_iter().collect::<Vec<_>>().join(", ")
            )
        } else {
            s.display.clone()
        };
        let _ = writeln!(out, "    {node}[\"{}\"]", mermaid_label(&label));
    }
    for (src, dst) in &report.edges {
        let (Some(a), Some(b)) = (node_ids.get(src.as_str()), node_ids.get(dst.as_str())) else {
            continue;
        };
        let _ = writeln!(out, "    {a} --> {b}");
    }
    out
}

/// Инструменты домена: `landscape_report`.
#[must_use]
pub fn tools() -> Vec<Arc<dyn Tool>> {
    vec![Arc::new(LandscapeReportTool)]
}

/// Инструмент `landscape_report`: ландшафт систем набора проектов (EA-3,
/// ADR-037) — отчёт/граф текстом + сводка (мост в MCP, транш 2 инверсии;
/// read-only).
pub struct LandscapeReportTool;

#[derive(Debug, Deserialize)]
struct LandscapeReportArgs {
    /// Корневой каталог набора проектов.
    path: String,
    /// Формат отчёта: markdown (дефолт) | mermaid.
    format: Option<String>,
}

#[async_trait]
impl Tool for LandscapeReportTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "landscape_report".into(),
            description: "Ландшафт систем набора проектов (EA-3, ADR-037): агрегация model/ \
                          самого ROOT и непосредственных подкаталогов в реестр систем SYS/INT \
                          с дедупликацией по нормализованному имени, находки (id-divergence, \
                          status-conflict, dangling-ref, cross-project-link) и топ связности. \
                          Ответ — JSON: format + счётчики systems/edges/findings + summary + \
                          report (markdown-отчёт или mermaid graph TD). Отчёт, а не гейт: \
                          verdict passed не применим"
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Корневой каталог набора проектов"},
                    "format": {
                        "type": "string",
                        "description": "Формат отчёта: markdown (по умолчанию) | mermaid",
                        "enum": ["markdown", "mermaid"]
                    }
                },
                "required": ["path"]
            }),
        }
    }

    async fn call(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let args: LandscapeReportArgs = match serde_json::from_value(args) {
            Ok(a) => a,
            Err(e) => {
                return Ok(ToolOutput::err(format!(
                    "landscape_report: невалидные аргументы: {e}"
                )));
            }
        };
        let format = args
            .format
            .as_deref()
            .unwrap_or("markdown")
            .trim()
            .to_ascii_lowercase();
        if format != "markdown" && format != "mermaid" {
            return Ok(ToolOutput::err(format!(
                "landscape_report: неизвестный формат '{format}' (допустимы: markdown, mermaid)"
            )));
        }
        let root = ctx.resolve(&args.path);
        let report = match build_landscape(&root) {
            Ok(r) => r,
            Err(e) => return Ok(ToolOutput::err(format!("landscape_report: {e}"))),
        };
        let text = if format == "mermaid" {
            render_mermaid(&report)
        } else {
            render_markdown(&report)
        };
        let summary = format!(
            "Ландшафт {}: {} систем, {} связей, {} находок",
            report.root.display(),
            report.systems.len(),
            report.edges.len(),
            report.findings.len()
        );
        let verdict = json!({
            "tool": "landscape_report",
            "format": format,
            "systems": report.systems.len(),
            "edges": report.edges.len(),
            "findings": report.findings.len(),
            "summary": summary,
            "report": text,
        });
        // Сериализация собранного объекта не падает; запасной вариант — компактная форма.
        let text = serde_json::to_string_pretty(&verdict).unwrap_or_else(|_| verdict.to_string());
        Ok(ToolOutput::ok(text))
    }
}
// ---------------------------------------------------------------------------
// Карта алиасов и дифф между датами (бэклог волны 3, роадмап горизонт 3)
// ---------------------------------------------------------------------------

/// Потолок файлов `model/*.md`, материализуемых из git для диффа (bounded
/// работа: по одному `git show` на файл).
const MAX_GIT_SHOW_FILES: usize = 2000;

/// Загружает карту алиасов ландшафта: плоское отображение «вариант имени →
/// каноничное имя» (yaml или json — по расширению `.json`). Обе стороны
/// нормализуются [`canonical_name`]; пустые ключи/значения пропускаются.
///
/// # Errors
/// Файл не читается или не разбирается как плоское отображение строк.
pub fn load_aliases(path: &Path) -> Result<BTreeMap<String, String>> {
    let text = std::fs::read_to_string(path).map_err(|e| HarnessError::io(path, e))?;
    let raw: BTreeMap<String, String> = if path.extension().is_some_and(|e| e == "json") {
        serde_json::from_str(&text).map_err(|e| {
            HarnessError::Model(format!("{}: карта алиасов (json): {e}", path.display()))
        })?
    } else {
        serde_yaml_ng::from_str(&text).map_err(|e| {
            HarnessError::Model(format!("{}: карта алиасов (yaml): {e}", path.display()))
        })?
    };
    let mut out = BTreeMap::new();
    for (alias, canonical) in raw {
        let (alias, canonical) = (canonical_name(&alias), canonical_name(&canonical));
        if alias.is_empty() || canonical.is_empty() || alias == canonical {
            continue;
        }
        out.insert(alias, canonical);
    }
    Ok(out)
}

/// Дифф ландшафта против версии в git.
#[derive(Debug)]
pub struct LandscapeDiff {
    /// Коммит, к которому разрешился `--diff-since` (полный sha).
    pub base: String,
    /// Появившиеся системы (представительные имена).
    pub added: Vec<String>,
    /// Исчезнувшие системы.
    pub removed: Vec<String>,
    /// Изменившиеся системы (что поменялось — статусы/id).
    pub changed: Vec<String>,
}

/// Прогон git с читаемой ошибкой (первые строки stderr).
fn git(root: &Path, args: &[&str]) -> Result<String> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .map_err(|e| HarnessError::Model(format!("ландшафт-дифф: git недоступен: {e}")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let hint = stderr.lines().next().unwrap_or("").trim().to_string();
        return Err(HarnessError::Model(format!(
            "ландшафт-дифф: git {}: {hint}",
            args.join(" ")
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Прогон git с бинарным выводом (`git show` файла).
fn git_show(root: &Path, spec: &str) -> Result<Vec<u8>> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["show", spec])
        .output()
        .map_err(|e| HarnessError::Model(format!("ландшафт-дифф: git недоступен: {e}")))?;
    if !output.status.success() {
        return Err(HarnessError::Model(format!(
            "ландшафт-дифф: git show {spec}: файл не читается"
        )));
    }
    Ok(output.stdout)
}

/// Разрешает `--diff-since` в sha коммита: дата `YYYY-MM-DD` (последний
/// коммит не позже конца дня по `rev-list --before`) или ссылка
/// (ветка/тег/sha через `rev-parse --verify`).
fn resolve_since(root: &Path, since: &str) -> Result<String> {
    let date_re = regex::Regex::new(r"^\d{4}-\d{2}-\d{2}$")
        .map_err(|e| HarnessError::Model(format!("внутренний regex даты: {e}")))?;
    if date_re.is_match(since.trim()) {
        let out = git(
            root,
            &[
                "rev-list",
                "-1",
                &format!("--before={}T23:59:59", since.trim()),
                "HEAD",
            ],
        )?;
        let sha = out.trim();
        if sha.is_empty() {
            return Err(HarnessError::Model(format!(
                "ландшафт-дифф: нет коммитов не позже {since} (HEAD)"
            )));
        }
        return Ok(sha.to_string());
    }
    let spec = format!("{}^{{commit}}", since.trim());
    Ok(git(root, &["rev-parse", "--verify", &spec])?
        .trim()
        .to_string())
}

/// Материализует файлы `model/*.md` коммита `sha` в каталог `dest` (пути —
/// относительно `prefix`, только глубины `model/x.md` и `<sub>/model/x.md` —
/// ровно то, что читает [`build_landscape`]). Возвращает число файлов.
fn materialize_models_at_ref(root: &Path, sha: &str, prefix: &str, dest: &Path) -> Result<usize> {
    let list = if prefix.is_empty() {
        git(root, &["ls-tree", "-r", "--name-only", sha])?
    } else {
        git(root, &["ls-tree", "-r", "--name-only", sha, "--", prefix])?
    };
    let mut count = 0usize;
    for path in list.lines() {
        let stripped = path
            .strip_prefix(prefix)
            .map_or(path, |s| s.strip_prefix('/').unwrap_or(s));
        let segments: Vec<&str> = stripped.split('/').collect();
        let is_model_file = matches!(segments.as_slice(), ["model", name] | [_, "model", name] if name.to_ascii_lowercase().ends_with(".md"));
        if !is_model_file {
            continue;
        }
        if count >= MAX_GIT_SHOW_FILES {
            return Err(HarnessError::Model(format!(
                "ландшафт-дифф: в {sha} более {MAX_GIT_SHOW_FILES} файлов модели — отказ"
            )));
        }
        let bytes = git_show(root, &format!("{sha}:{path}"))?;
        let target = dest.join(stripped);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|e| HarnessError::io(parent, e))?;
        }
        std::fs::write(&target, &bytes).map_err(|e| HarnessError::io(&target, e))?;
        count += 1;
    }
    Ok(count)
}

/// Сводка системы для сравнения: отсортированные id и статусы (lowercase).
fn system_signature(s: &LandscapeSystem) -> (Vec<String>, Vec<String>) {
    let mut ids: Vec<String> = s.occurrences.iter().map(|o| o.id.clone()).collect();
    ids.sort();
    let mut statuses: Vec<String> = s
        .occurrences
        .iter()
        .map(|o| o.status.trim().to_lowercase())
        .collect();
    statuses.sort();
    statuses.dedup();
    (ids, statuses)
}

/// Дифф ландшафта: текущее состояние (рабочее дерево `root`) против версии
/// в git на `since` (ссылка или дата `YYYY-MM-DD`).
///
/// # Errors
/// `root` вне git-репозитория, ссылка/дата не разрешается в коммит,
/// модель (текущая или историческая) не агрегируется.
pub fn diff_landscape(
    root: &Path,
    since: &str,
    aliases: &BTreeMap<String, String>,
) -> Result<LandscapeDiff> {
    // Корень репозитория и префикс `root` внутри него (дифф возможен из
    // подкаталога репо — видна только его часть ландшафта).
    let toplevel = git(root, &["rev-parse", "--show-toplevel"]).map_err(|e| {
        HarnessError::Model(format!(
            "ландшафт-дифф: {} не в git-репозитории ({e})",
            root.display()
        ))
    })?;
    let toplevel = PathBuf::from(toplevel.trim());
    let prefix = std::fs::canonicalize(root)
        .ok()
        .and_then(|r| {
            std::fs::canonicalize(&toplevel).ok().and_then(|t| {
                r.strip_prefix(t)
                    .ok()
                    .map(|p| p.to_string_lossy().replace('\\', "/"))
            })
        })
        .unwrap_or_default();
    let sha = resolve_since(root, since)?;
    let current = build_landscape_with_aliases(root, aliases)?;

    let tmp = tempfile::tempdir()
        .map_err(|e| HarnessError::Model(format!("ландшафт-дифф: временный каталог: {e}")))?;
    let files = materialize_models_at_ref(root, &sha, &prefix, tmp.path())?;
    // Пустая историческая модель (model/ появились позже) — легитимна:
    // все текущие системы «появились».
    let past = if files == 0 {
        None
    } else {
        Some(build_landscape_with_aliases(tmp.path(), aliases)?)
    };

    let mut added = Vec::new();
    let mut removed = Vec::new();
    let mut changed = Vec::new();
    let past_systems: BTreeMap<&str, &LandscapeSystem> = past
        .as_ref()
        .map(|p| {
            p.systems
                .iter()
                .map(|s| (s.canonical.as_str(), s))
                .collect()
        })
        .unwrap_or_default();
    let current_systems: BTreeMap<&str, &LandscapeSystem> = current
        .systems
        .iter()
        .map(|s| (s.canonical.as_str(), s))
        .collect();
    for (canonical, s) in &current_systems {
        match past_systems.get(canonical) {
            None => added.push(s.display.clone()),
            Some(old) => {
                let (old_ids, old_statuses) = system_signature(old);
                let (ids, statuses) = system_signature(s);
                if old_ids != ids || old_statuses != statuses {
                    let mut parts = Vec::new();
                    if old_ids != ids {
                        parts.push(format!("id {} → {}", old_ids.join(","), ids.join(",")));
                    }
                    if old_statuses != statuses {
                        parts.push(format!(
                            "статусы {} → {}",
                            old_statuses.join(","),
                            statuses.join(",")
                        ));
                    }
                    changed.push(format!("«{}»: {}", s.display, parts.join("; ")));
                }
            }
        }
    }
    for (canonical, s) in &past_systems {
        if !current_systems.contains_key(canonical) {
            removed.push(s.display.clone());
        }
    }
    Ok(LandscapeDiff {
        base: sha,
        added,
        removed,
        changed,
    })
}

/// Рендер диффа ландшафта в markdown-секцию.
#[must_use]
pub fn render_diff_markdown(diff: &LandscapeDiff, since: &str) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "## Дифф ландшафта (против {since})\n");
    let section = |out: &mut String, title: &str, items: &[String]| {
        let _ = writeln!(out, "### {title}");
        if items.is_empty() {
            let _ = writeln!(out, "- нет\n");
        } else {
            for item in items {
                let _ = writeln!(out, "- {item}");
            }
            out.push('\n');
        }
    };
    section(&mut out, "Новые системы", &diff.added);
    section(&mut out, "Исчезнувшие системы", &diff.removed);
    section(&mut out, "Изменившиеся системы", &diff.changed);
    let _ = write!(
        out,
        "Итого: +{} / −{} / ~{}",
        diff.added.len(),
        diff.removed.len(),
        diff.changed.len()
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    /// Пишет файл в каталог и возвращает его путь.
    fn write_file(dir: &Path, name: &str, content: &str) -> PathBuf {
        let p = dir.join(name);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&p, content).unwrap();
        p
    }

    /// Фикстура: p1 и p2 с model/ (общая система «Процессинг» под разными
    /// id и статусами, ссылка на несуществующий SYS-099, кросс-проектная
    /// связь p2 → SYS-001 из p1), p3 без model/ (молча пропускается).
    fn fixture(dir: &Path) -> PathBuf {
        let root = dir.join("root");
        write_file(
            &root,
            "p1/model/SYS-001-processing.md",
            "---\nid: SYS-001\ntype: sys\ntitle: Процессинг\nstatus: adopted\n---\nСистема.\n",
        );
        write_file(
            &root,
            "p1/model/INT-002-gateway.md",
            "---\nid: INT-002\ntype: int\ntitle: Шлюз\nstatus: adopted\ndepends_on: [SYS-001, SYS-099]\n---\nИнтеграция.\n",
        );
        write_file(
            &root,
            "p2/model/INT-007-processing.md",
            "---\nid: INT-007\ntype: int\ntitle: процессинг\nstatus: proposed\naffects: [SYS-001]\n---\nТа же система в другом проекте.\n",
        );
        write_file(
            &root,
            "p2/model/SYS-003-fraud.md",
            "---\nid: SYS-003\ntype: sys\ntitle: Фрод-монитор\nstatus: adopted\ndepends_on: [INT-007]\n---\nСистема.\n",
        );
        // CMP в model/ — не система ландшафта, в реестр не попадает.
        write_file(
            &root,
            "p2/model/CMP-001-core.md",
            "---\nid: CMP-001\ntype: cmp\ntitle: Ядро\nstatus: adopted\n---\nКомпонент.\n",
        );
        write_file(&root, "p3/README.md", "проект без модели\n");
        root
    }

    #[test]
    fn landscape_builds_deduped_registry_and_findings() {
        let dir = tempfile::tempdir().unwrap();
        let root = fixture(dir.path());
        let report = build_landscape(&root).unwrap();

        // Реестр: 3 системы; «Процессинг» — ОДНА строка с двумя вхождениями.
        assert_eq!(report.systems.len(), 3, "{:?}", report.systems);
        let proc = report
            .systems
            .iter()
            .find(|s| s.canonical == "процессинг")
            .unwrap();
        assert_eq!(proc.occurrences.len(), 2, "{proc:?}");
        let ids: BTreeSet<&str> = proc.occurrences.iter().map(|o| o.id.as_str()).collect();
        assert_eq!(ids, BTreeSet::from(["SYS-001", "INT-007"]));
        // CMP не система; p3 без model/ молча пропущен.
        assert!(
            report.systems.iter().all(|s| s.canonical != "ядро"),
            "{:?}",
            report.systems
        );
        assert!(
            report
                .systems
                .iter()
                .flat_map(|s| &s.occurrences)
                .all(|o| o.project != "p3")
        );

        // Находки: все четыре типа.
        let kinds = |k: &str| {
            report
                .findings
                .iter()
                .filter(|f| f.kind == k)
                .collect::<Vec<_>>()
        };
        let divergence = kinds("id-divergence");
        assert_eq!(divergence.len(), 1, "{divergence:?}");
        assert!(
            divergence[0].message.contains("Процессинг"),
            "{divergence:?}"
        );
        let conflicts = kinds("status-conflict");
        assert_eq!(conflicts.len(), 1, "{conflicts:?}");
        assert!(conflicts[0].message.contains("adopted"), "{conflicts:?}");
        let dangling = kinds("dangling-ref");
        assert_eq!(dangling.len(), 1, "{dangling:?}");
        assert!(dangling[0].message.contains("SYS-099"), "{dangling:?}");
        let cross = kinds("cross-project-link");
        assert_eq!(cross.len(), 1, "{cross:?}");
        assert!(
            cross[0].message.contains("INT-007") && cross[0].message.contains("p1"),
            "{cross:?}"
        );

        // Топ связности: процессинг (2 вход: от шлюза и фрод-монитора).
        assert_eq!(report.top[0].0, "Процессинг", "{:?}", report.top);
        assert_eq!(report.top[0].1, 2, "{:?}", report.top);
        assert_eq!(report.top[1].0, "Фрод-монитор", "{:?}", report.top);
        assert_eq!(report.top[1].1, 1, "{:?}", report.top);
        // Рёбра: шлюз → процессинг, фрод-монитор → процессинг (саморебро
        // INT-007→SYS-001 схлопнуто дедупликацией).
        assert_eq!(
            report.edges,
            vec![
                ("фрод монитор".to_string(), "процессинг".to_string()),
                ("шлюз".to_string(), "процессинг".to_string())
            ],
            "{:?}",
            report.edges
        );
    }

    #[test]
    fn landscape_mermaid_shape() {
        let dir = tempfile::tempdir().unwrap();
        let root = fixture(dir.path());
        let report = build_landscape(&root).unwrap();
        let mm = render_mermaid(&report);
        assert!(mm.starts_with("graph TD\n"), "{mm}");
        // Узлы санитизированы (sN) в порядке канонических имён:
        // s1 — процессинг, s2 — фрод монитор, s3 — шлюз.
        assert!(mm.contains("s2[\"Фрод-монитор\"]"), "{mm}");
        assert!(
            mm.contains("s1[\"Процессинг (p1, p2)\"]"),
            "метка проектов при id-divergence: {mm}"
        );
        // Рёбра: фрод-монитор → процессинг, шлюз → процессинг.
        assert!(mm.contains("s2 --> s1"), "{mm}");
        assert!(mm.contains("s3 --> s1"), "{mm}");
        // Невалидных символов mermaid в метках нет.
        assert!(!mm.contains("[\"Шлюз ["), "{mm}");
    }

    #[test]
    fn landscape_markdown_sections() {
        let dir = tempfile::tempdir().unwrap();
        let root = fixture(dir.path());
        let report = build_landscape(&root).unwrap();
        let md = render_markdown(&report);
        assert!(md.contains("## Реестр систем"), "{md}");
        assert!(
            md.contains("| Процессинг | p1: SYS-001; p2: INT-007 | adopted, proposed |"),
            "{md}"
        );
        assert!(md.contains("[id-divergence]"), "{md}");
        assert!(md.contains("[dangling-ref]"), "{md}");
        assert!(md.contains("[cross-project-link]"), "{md}");
        assert!(md.contains("## Топ связности"), "{md}");
        assert!(md.contains("1. Процессинг — 2 связей"), "{md}");
    }

    #[test]
    fn landscape_without_any_model_is_clear_error() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root");
        write_file(&root, "p3/README.md", "без моделей\n");
        let err = build_landscape(&root).unwrap_err();
        assert!(err.to_string().contains("model/"), "{err}");
        assert!(build_landscape(&root.join("missing")).is_err());
    }

    /// Инструмент `landscape_report`: markdown и mermaid на фикстуре, счётчики,
    /// мягкие ошибки на битом формате и несуществующем корне.
    #[tokio::test]
    async fn landscape_report_tool_markdown_mermaid_and_errors() {
        let dir = tempfile::tempdir().expect("tmp");
        let root = fixture(dir.path());
        let ctx = ToolContext::new(root, Arc::new(crate::config::Config::default()));
        let out = LandscapeReportTool
            .call(json!({"path": "."}), &ctx)
            .await
            .expect("вызов");
        assert!(!out.is_error, "{}", out.content);
        let v: Value = serde_json::from_str(&out.content).expect("JSON-вердикт");
        assert_eq!(v["tool"], "landscape_report");
        assert_eq!(v["format"], "markdown");
        assert!(v["systems"].as_u64().expect("systems") >= 1, "{v}");
        assert!(v["findings"].as_u64().expect("findings") >= 1, "{v}");
        let report = v["report"].as_str().expect("report");
        assert!(report.contains("# Ландшафт систем"), "{report}");
        assert!(
            v["summary"].as_str().expect("summary").contains("систем"),
            "{v}"
        );
        // mermaid — graph TD канонических систем.
        let out = LandscapeReportTool
            .call(json!({"path": ".", "format": "mermaid"}), &ctx)
            .await
            .expect("вызов");
        let v: Value = serde_json::from_str(&out.content).expect("JSON");
        assert!(
            v["report"]
                .as_str()
                .expect("report")
                .starts_with("graph TD"),
            "{v}"
        );
        // Битый формат и несуществующий корень — мягкие ошибки инструмента.
        let out = LandscapeReportTool
            .call(json!({"path": ".", "format": "svg"}), &ctx)
            .await
            .expect("вызов");
        assert!(out.is_error, "{}", out.content);
        assert!(out.content.contains("формат"), "{}", out.content);
        let out = LandscapeReportTool
            .call(json!({"path": "missing"}), &ctx)
            .await
            .expect("вызов");
        assert!(out.is_error, "{}", out.content);
    }

    #[test]
    fn aliases_merge_name_variants_before_dedup() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root");
        write_file(
            &root,
            "p1/model/SYS-001.md",
            "---\nid: SYS-001\ntype: sys\ntitle: Процессинг\nstatus: adopted\n---\nСистема.\n",
        );
        write_file(
            &root,
            "p2/model/SYS-002.md",
            "---\nid: SYS-002\ntype: sys\ntitle: Процессинг-СБОЛ\nstatus: adopted\n---\nСистема.\n",
        );
        // Без карты — две системы.
        let report = build_landscape(&root).unwrap();
        assert_eq!(report.systems.len(), 2, "{:?}", report.systems);

        // Карта (json): вариант → каноничное имя; обе стороны нормализуются.
        let aliases_file = dir.path().join("aliases.json");
        std::fs::write(&aliases_file, "{\"Процессинг СБОЛ\": \" Процессинг \"}").unwrap();
        let aliases = load_aliases(&aliases_file).unwrap();
        assert_eq!(
            aliases.get("процессинг сбол").map(String::as_str),
            Some("процессинг")
        );
        let report = build_landscape_with_aliases(&root, &aliases).unwrap();
        assert_eq!(report.systems.len(), 1, "{:?}", report.systems);
        assert_eq!(report.systems[0].occurrences.len(), 2);
        // Слияние по алиасу делает видимой расходку id (id-divergence).
        assert!(
            report.findings.iter().any(|f| f.kind == "id-divergence"),
            "{:?}",
            report.findings
        );

        // yaml-вариант карты.
        let yaml = dir.path().join("aliases.yaml");
        std::fs::write(&yaml, "Процессинг СБОЛ: процессинг\n").unwrap();
        assert_eq!(load_aliases(&yaml).unwrap(), aliases);
        // Не плоское отображение — понятная ошибка.
        let bad = dir.path().join("bad.yaml");
        std::fs::write(&bad, "- не\n- карта\n").unwrap();
        assert!(load_aliases(&bad).is_err());
    }

    /// Git-фикстура: коммит «тогда» (SYS-001 adopted + SYS-002) против
    /// рабочего дерева «сейчас» (SYS-001 proposed, SYS-002 удалена,
    /// +SYS-003). Возвращает корень репо.
    fn git_fixture(dir: &Path) -> PathBuf {
        let repo = dir.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let git = |args: &[&str]| {
            let out = std::process::Command::new("git")
                .arg("-C")
                .arg(&repo)
                .args(args)
                .env("GIT_AUTHOR_NAME", "t")
                .env("GIT_AUTHOR_EMAIL", "t@t")
                .env("GIT_COMMITTER_NAME", "t")
                .env("GIT_COMMITTER_EMAIL", "t@t")
                .env("GIT_COMMITTER_DATE", "2026-01-10T12:00:00")
                .output()
                .expect("git");
            assert!(out.status.success(), "git {args:?}: {:?}", out.status);
        };
        git(&["init", "-q"]);
        write_file(
            &repo,
            "p1/model/SYS-001.md",
            "---\nid: SYS-001\ntype: sys\ntitle: Процессинг\nstatus: adopted\n---\nСистема.\n",
        );
        write_file(
            &repo,
            "p2/model/SYS-002.md",
            "---\nid: SYS-002\ntype: sys\ntitle: Фрод-монитор\nstatus: adopted\n---\nСистема.\n",
        );
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "тогда"]);
        // «Сейчас» — рабочее дерево без коммита.
        write_file(
            &repo,
            "p1/model/SYS-001.md",
            "---\nid: SYS-001\ntype: sys\ntitle: Процессинг\nstatus: proposed\n---\nСистема.\n",
        );
        std::fs::remove_file(repo.join("p2/model/SYS-002.md")).unwrap();
        write_file(
            &repo,
            "p1/model/SYS-003.md",
            "---\nid: SYS-003\ntype: sys\ntitle: Шлюз СБП\nstatus: adopted\n---\nСистема.\n",
        );
        repo
    }

    #[test]
    fn landscape_diff_since_ref_and_date() {
        let dir = tempfile::tempdir().unwrap();
        let repo = git_fixture(dir.path());
        let aliases = BTreeMap::new();

        // По ссылке HEAD.
        let diff = diff_landscape(&repo, "HEAD", &aliases).unwrap();
        assert_eq!(diff.added, vec!["Шлюз СБП".to_string()], "{diff:?}");
        assert_eq!(diff.removed, vec!["Фрод-монитор".to_string()], "{diff:?}");
        assert_eq!(diff.changed.len(), 1, "{diff:?}");
        assert!(
            diff.changed[0].contains("Процессинг")
                && diff.changed[0].contains("adopted → proposed"),
            "{diff:?}"
        );
        let md = render_diff_markdown(&diff, "HEAD");
        assert!(md.contains("## Дифф ландшафта"), "{md}");
        assert!(md.contains("+1 / −1 / ~1"), "{md}");

        // По дате (последний коммит не позже 15.01) — тот же дифф.
        let diff_by_date = diff_landscape(&repo, "2026-01-15", &aliases).unwrap();
        assert_eq!(
            diff_by_date.base, diff.base,
            "дата разрешилась в тот же sha"
        );

        // Дата до первого коммита — понятная ошибка.
        let err = diff_landscape(&repo, "2025-12-31", &aliases).unwrap_err();
        assert!(err.to_string().contains("нет коммитов"), "{err}");
        // Мусорная ссылка — понятная ошибка.
        assert!(diff_landscape(&repo, "no-such-ref", &aliases).is_err());
    }

    #[test]
    fn landscape_diff_outside_git_is_clear_error() {
        let dir = tempfile::tempdir().unwrap();
        let root = fixture(dir.path());
        let err = diff_landscape(&root, "HEAD", &BTreeMap::new()).unwrap_err();
        assert!(err.to_string().contains("git"), "{err}");
    }

    #[test]
    fn landscape_diff_with_empty_past_model_reports_all_added() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let git = |args: &[&str]| {
            let out = std::process::Command::new("git")
                .arg("-C")
                .arg(&repo)
                .args(args)
                .env("GIT_AUTHOR_NAME", "t")
                .env("GIT_AUTHOR_EMAIL", "t@t")
                .env("GIT_COMMITTER_NAME", "t")
                .env("GIT_COMMITTER_EMAIL", "t@t")
                .output()
                .expect("git");
            assert!(out.status.success(), "git {args:?}");
        };
        git(&["init", "-q"]);
        write_file(&repo, "README.md", "без модели\n");
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "до моделей"]);
        write_file(
            &repo,
            "p1/model/SYS-001.md",
            "---\nid: SYS-001\ntype: sys\ntitle: Процессинг\nstatus: adopted\n---\nСистема.\n",
        );
        let diff = diff_landscape(&repo, "HEAD", &BTreeMap::new()).unwrap();
        assert_eq!(diff.added, vec!["Процессинг".to_string()], "{diff:?}");
        assert!(diff.removed.is_empty() && diff.changed.is_empty());
    }
}
