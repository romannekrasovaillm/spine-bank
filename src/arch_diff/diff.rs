//! Дифф двух ревизий в трёх измерениях (K2): реальность PR (добавленные и
//! удалённые узлы/рёбра графа as-built), расхождение с моделью (ребро
//! «в модели / нет в модели», объявленные рёбра без кода) и смысл (задетые
//! инварианты AD с правилами-охранниками и классом зубов, затронутые ADR,
//! классификация контрактов, сдвиг NFR, маршрут значимости с источниками
//! триггеров).
//!
//! Детерминизм (правило 10): все коллекции отсортированы; повторный прогон
//! на тех же коммитах даёт тот же JSON.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde::Serialize;

use super::as_built::{RevisionScan, contract_paths, scan_revision};
use super::types::{ArchEdge, ArchGraph, ArchNode, EdgeKind, NodeKind};
use crate::control::{DiffGlobs, detect_diff_triggers_with, score_with_sources};
use crate::error::{HarnessError, Result};
use crate::model::{EntityKind, Model};

/// Статус ребра относительно модели: объявлено (`depends_on`/`INT`) или нет.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelStatus {
    /// Ребро объявлено в модели.
    InModel,
    /// Ребра в модели нет (код ушёл от модели или модель отстала).
    NotInModel,
}

/// Изменение ребра: само ребро и его статус в модели соответствующей
/// ревизии (`None` — модели в ревизии нет, колонка не применима).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EdgeChange {
    /// Ребро (from/to/kind + основания).
    #[serde(flatten)]
    pub edge: ArchEdge,
    /// Статус в модели (head для добавленных, base для удалённых).
    pub model_status: Option<ModelStatus>,
}

/// Объявленное в модели ребро `depends_on`, за которым в коде ревизии нет
/// ни одного импорта (семантика находки `declared-edge-unused` волны C2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DeclaredEdge {
    /// CMP-источник объявленной связи.
    pub from: String,
    /// CMP-цель.
    pub to: String,
}

/// Класс зубов правила-охранника (интерфейс к волне B: `.arch-handoff/
/// teeth.json`; без файла — честное «не измерены»).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TeethClass {
    /// Зубы подтверждены мутацией (волна B).
    Confirmed,
    /// Текстовое правило (зубьев нет либо это показано измерением).
    Text,
    /// Зубы не измерены (файла teeth.json нет или правила в нём нет).
    Unknown,
}

impl TeethClass {
    /// Метка для вывода.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Confirmed => "зубы подтверждены",
            Self::Text => "текст",
            Self::Unknown => "зубы не измерены",
        }
    }
}

/// Правило реестра, охраняющее инвариант (`verified_by: C-NNN` у AD).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RuleGuard {
    /// Идентификатор правила (`C-007`).
    pub id: String,
    /// Имя правила в реестре (`None` — правило в CONSTRAINTS.yaml не найдено).
    pub name: Option<String>,
    /// Класс зубов.
    pub teeth: TeethClass,
}

/// Задетый изменением инвариант модели (AD).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InvariantHit {
    /// Id инварианта (`AD-2`).
    pub ad_id: String,
    /// Заголовок инварианта.
    pub ad_title: String,
    /// Компоненты, через которые изменение задевает инвариант (`affects` у
    /// AD либо `implements` у CMP).
    pub via_components: Vec<String>,
    /// Правила-охранники с классом зубов.
    pub rules: Vec<RuleGuard>,
}

/// Класс изменения контракта (классификация `contract_diff`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ContractClass {
    /// Контракт добавлен.
    Added,
    /// Контракт удалён (ломающее по определению).
    Removed,
    /// Ломающее изменение (находки `error` у `contract_diff`).
    Breaking,
    /// Аддитивное изменение (без error-находок).
    Additive,
    /// Формат не распознан — классификация невозможна (честный пропуск).
    Unknown,
}

/// Изменение контракта между ревизиями.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ContractChange {
    /// Путь контракта.
    pub path: String,
    /// Класс изменения.
    pub classification: ContractClass,
    /// Первые находки диффа контракта (`правило: сообщение`), ограничены.
    pub details: Vec<String>,
}

/// Сдвиг количественного NFR между ревизиями (`nfr budget`/`nfr capacity`).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct NfrShift {
    /// Проверка-источник (`budget` | `capacity`).
    pub check: String,
    /// Id NFR.
    pub nfr: String,
    /// Метрика (`target_ms`, `sum_ms`, `rps_target`, `required_instances`).
    pub metric: String,
    /// Было (base). `None` — метрики на base не было.
    pub was: Option<f64>,
    /// Стало (head). `None` — метрики на head нет.
    pub now: Option<f64>,
}

/// Триггер значимости с источником срабатывания.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TriggerHit {
    /// Каноническое имя триггера.
    pub name: String,
    /// Источник: `declared` | `diff` | `declared+diff`.
    pub source: String,
}

/// Маршрут значимости диффа и его основания.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RouteInfo {
    /// Маршрут (`fast` | `standard` | `critical`).
    pub route: String,
    /// Счёт (число сработавших канонических триггеров).
    pub score: usize,
    /// Сработавшие триггеры с источниками (отсортированы по имени).
    pub triggers: Vec<TriggerHit>,
    /// Основания детекторов (`триггер: файл` — из диффа).
    pub evidence: Vec<String>,
    /// Найденные диффом, но не заявленные триггеры (anti-bypass сигнал).
    pub undeclared: Vec<String>,
}

/// Вид предложения правки модели.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProposalKind {
    /// `depends_on += <цель>` в файле CMP-источника.
    AddDependsOn,
    /// Новая сущность модели (CMP/SYS/INT) по факту кода.
    NewEntity,
}

/// Пронумерованное предложение правки модели. Принятие/отклонение — K5
/// (`arch-diff accept`/`reject`, ADR-064): `edge_id` + `grounds_hash` —
/// ключ журнала решений.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ModelProposal {
    /// Номер предложения (1-based; решённые журналом исчезают из списка,
    /// номера остальных сохраняются — на номер ссылаются `accept`/`reject`
    /// одного сеанса, ADR-064).
    pub n: usize,
    /// Вид правки.
    pub kind: ProposalKind,
    /// Файл модели, который предлагается поправить/создать.
    pub file: String,
    /// Суть правки одной строкой.
    pub summary: String,
    /// Инварианты, которым правка противоречит (пометка ⚠ в выводе).
    pub conflicts: Vec<String>,
    /// Стабильный id объекта решения (K5, ADR-064): `<kind>:<from>-><to>`
    /// для рёбер, `node:<id узла>` для новых компонентов. Аддитивное поле
    /// `arch-be/arch-diff/v1` (ADR-063: добавление поля — минорно).
    pub edge_id: String,
    /// Хэш оснований предложения (sha256 списка `файл:строка` ребра; для
    /// узла — его id). Основания изменились — решённое ребро предлагается
    /// снова (ADR-064).
    pub grounds_hash: String,
}

/// Архитектурный дифф `base..head` (машинный контракт — `arch-be/arch-diff/v1`,
/// см. рендер JSON в K3).
#[derive(Debug, Clone, Serialize)]
pub struct ArchDiff {
    /// Полный sha базы.
    pub base: String,
    /// Полный sha головы.
    pub head: String,
    /// Добавленные узлы (отсортированы по id).
    pub added_nodes: Vec<ArchNode>,
    /// Удалённые узлы.
    pub removed_nodes: Vec<ArchNode>,
    /// Добавленные рёбра (отсортированы по from/to/kind).
    pub added_edges: Vec<EdgeChange>,
    /// Удалённые рёбра.
    pub removed_edges: Vec<EdgeChange>,
    /// Объявленные в модели рёбра без кода на head.
    pub declared_unused: Vec<DeclaredEdge>,
    /// Задетые инварианты AD с правилами-охранниками.
    pub invariants_touched: Vec<InvariantHit>,
    /// Затронутые ADR (по `affects`).
    pub adrs_touched: Vec<String>,
    /// Изменения контрактов с классификацией.
    pub contract_changes: Vec<ContractChange>,
    /// Сдвиги количественных NFR.
    pub nfr_shifts: Vec<NfrShift>,
    /// Маршрут значимости и триггеры с источниками.
    pub route: RouteInfo,
    /// Предложения правки модели (пронумерованные; уже без решённых журналом
    /// `.arch-handoff/arch-diff-decisions.json` — K5, ADR-064).
    pub proposals: Vec<ModelProposal>,
}

impl ArchDiff {
    /// Пустой дифф: архитектурных изменений нет (узлы/рёбра/контракты/NFR
    /// не изменились) — маршрут и инварианты из этого следуют.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.added_nodes.is_empty()
            && self.removed_nodes.is_empty()
            && self.added_edges.is_empty()
            && self.removed_edges.is_empty()
            && self.contract_changes.is_empty()
            && self.nfr_shifts.is_empty()
    }

    /// Есть ли добавленные рёбра вне модели (`--fail-on undeclared-edge`).
    #[must_use]
    pub fn has_undeclared_edges(&self) -> bool {
        self.added_edges
            .iter()
            .any(|e| e.model_status == Some(ModelStatus::NotInModel))
    }

    /// Есть ли ломающие изменения контрактов (`--fail-on breaking-contract`).
    #[must_use]
    pub fn has_breaking_contracts(&self) -> bool {
        self.contract_changes.iter().any(|c| {
            matches!(
                c.classification,
                ContractClass::Breaking | ContractClass::Removed
            )
        })
    }

    /// Задеты ли инварианты (`--fail-on invariant-touched`).
    #[must_use]
    pub fn has_invariant_touched(&self) -> bool {
        !self.invariants_touched.is_empty()
    }
}

/// Условие `--fail-on` (K3): какой факт диффа превращает информационный
/// прогон в красный (exit 1). Имена — значения CLI-флага.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailOn {
    /// Добавленное ребро вне модели.
    UndeclaredEdge,
    /// Ломающее изменение контракта (включая удаление).
    BreakingContract,
    /// Задет инвариант AD.
    InvariantTouched,
}

impl FailOn {
    /// Допустимые имена (для сообщения об ошибке CLI).
    pub const NAMES: [&str; 3] = ["undeclared-edge", "breaking-contract", "invariant-touched"];

    /// Разбор имени флага.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        match name.trim() {
            "undeclared-edge" => Some(Self::UndeclaredEdge),
            "breaking-contract" => Some(Self::BreakingContract),
            "invariant-touched" => Some(Self::InvariantTouched),
            _ => None,
        }
    }

    /// Каноническое имя.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::UndeclaredEdge => "undeclared-edge",
            Self::BreakingContract => "breaking-contract",
            Self::InvariantTouched => "invariant-touched",
        }
    }
}

/// Сработавшие условия `--fail-on` (в каноническом порядке имён): по ним CLI
/// завершается exit 1. Без флагов дифф информационный (exit 0).
#[must_use]
pub fn matched_failures(diff: &ArchDiff, fail_on: &[FailOn]) -> Vec<&'static str> {
    let mut out = Vec::new();
    for f in fail_on {
        let hit = match f {
            FailOn::UndeclaredEdge => diff.has_undeclared_edges(),
            FailOn::BreakingContract => diff.has_breaking_contracts(),
            FailOn::InvariantTouched => diff.has_invariant_touched(),
        };
        if hit {
            out.push(f.name());
        }
    }
    out
}

/// Вход диффа: база (ссылка или диапазон `A...B` — берётся левая ревизия),
/// голова (по умолчанию `HEAD`), заявленные триггеры и пороги маршрута.
pub struct ArchDiffInput<'a> {
    /// База диффа (ветка/тег/sha или диапазон `A...HEAD`).
    pub base: &'a str,
    /// Голова диффа (по умолчанию `HEAD`).
    pub head: Option<&'a str>,
    /// Заявленные триггеры значимости (объединяются с детектором, ADR-034).
    pub declared: BTreeMap<String, bool>,
    /// Пороги маршрута (`fast_max`, `standard_max`) из `[significance]`.
    pub limits: (usize, usize),
    /// Глобы детекторов (T-05).
    pub globs: &'a DiffGlobs,
}

/// Потолок находок диффа контракта в деталях одного изменения.
const MAX_CONTRACT_DETAILS: usize = 5;

/// Карта классов зубов правил (имя/id правила → класс).
struct TeethMap(BTreeMap<String, TeethClass>);

impl TeethMap {
    /// Класс зубов правила по имени или id; неизвестное — «не измерены».
    fn class_of(&self, id: &str, name: Option<&str>) -> TeethClass {
        name.and_then(|n| self.0.get(n))
            .or_else(|| self.0.get(id))
            .copied()
            .unwrap_or(TeethClass::Unknown)
    }
}

/// Читает `.arch-handoff/teeth.json` рабочего дерева (интерфейс волны B;
/// сам файл волна K НЕ создаёт). Толерантные формы: `{"rules": {"<имя>":
/// {"teeth": "confirmed"|"text"|"unknown"}}}`, та же карта без обёртки
/// `rules`, значение-строка вместо объекта. Нет файла/не JSON — пустая
/// карта (все правила честно «зубы не измерены»).
fn load_teeth(repo: &Path) -> TeethMap {
    let mut map = BTreeMap::new();
    let Ok(text) = std::fs::read_to_string(repo.join(".arch-handoff/teeth.json")) else {
        return TeethMap(map); // нет файла — зубы не измерены
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
        return TeethMap(map); // битый JSON — то же честное «не измерено»
    };
    let table = value
        .get("rules")
        .and_then(serde_json::Value::as_object)
        .or_else(|| value.as_object());
    let parse_class = |raw: &serde_json::Value| {
        let word = raw
            .as_str()
            .or_else(|| raw.get("teeth").and_then(serde_json::Value::as_str))
            .or_else(|| raw.get("class").and_then(serde_json::Value::as_str))
            .unwrap_or_default();
        match word {
            "confirmed" | "with_teeth" | "verified" => TeethClass::Confirmed,
            "text" | "toothless" | "none" => TeethClass::Text,
            _ => TeethClass::Unknown,
        }
    };
    if let Some(obj) = table {
        for (name, raw) in obj {
            map.insert(name.clone(), parse_class(raw));
        }
    }
    TeethMap(map)
}

/// Хост (`host` без порта и схемы) из id узла `sys:`/`store:`.
fn host_of_node(node_id: &str) -> Option<&str> {
    let rest = node_id.strip_prefix("sys:").or_else(|| {
        node_id
            .strip_prefix("store:")
            .and_then(|s| s.split_once("://").map(|x| x.1))
    })?;
    Some(rest.split(':').next().unwrap_or(rest))
}

/// Статус ребра в модели: import — по `depends_on` CMP; connect — по
/// упоминанию хоста в INT/SYS; contract — по полю `contract` INT (ADR-035).
/// `pub(crate)`: составляющая гейта `arch_drift` (K6) сверяет рёбра рабочего
/// дерева той же функцией — семантика «в модели / нет в модели» одна.
pub(crate) fn edge_model_status(
    graph: &ArchGraph,
    model: Option<&Model>,
    edge: &ArchEdge,
) -> Option<ModelStatus> {
    let model = model?;
    let found = match edge.kind {
        EdgeKind::Import | EdgeKind::Calls => {
            let from_id = graph.node(&edge.from).and_then(|n| n.model_id.as_deref());
            let to_id = graph.node(&edge.to).and_then(|n| n.model_id.as_deref());
            match (from_id, to_id) {
                (Some(f), Some(t)) => model
                    .get(f)
                    .is_some_and(|e| e.depends_on.iter().any(|d| d == t)),
                _ => false,
            }
        }
        EdgeKind::Connect => host_of_node(&edge.to).is_some_and(|host| {
            let host = host.to_ascii_lowercase();
            model.entities.iter().any(|e| {
                matches!(e.kind, EntityKind::Int | EntityKind::Sys)
                    && format!("{} {} {}", e.id, e.title, e.body)
                        .to_ascii_lowercase()
                        .contains(&host)
            })
        }),
        EdgeKind::ContractRef => {
            let path = edge.to.trim_start_matches("contract:");
            model.entities.iter().any(|e| {
                e.kind == EntityKind::Int
                    && e.contract
                        .as_deref()
                        .is_some_and(|c| c.trim().trim_start_matches("./") == path)
            })
        }
    };
    Some(if found {
        ModelStatus::InModel
    } else {
        ModelStatus::NotInModel
    })
}

/// Объявленные рёбра модели без кода на ревизии head (C2-семантика на графе):
/// CMP с `depends_on` на CMP с `code_roots`, но ребра импорта в графе нет.
fn declared_unused(head: &RevisionScan) -> Vec<DeclaredEdge> {
    let Some(model) = &head.model else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for e in &model.entities {
        if e.kind != EntityKind::Cmp || e.code_roots.is_empty() {
            continue;
        }
        for dep in &e.depends_on {
            let target_ok = model
                .get(dep)
                .is_some_and(|t| t.kind == EntityKind::Cmp && !t.code_roots.is_empty());
            if !target_ok {
                continue;
            }
            let used = head
                .graph
                .edges
                .iter()
                .any(|g| g.kind == EdgeKind::Import && g.from == e.id && &g.to == dep);
            if !used {
                out.push(DeclaredEdge {
                    from: e.id.clone(),
                    to: dep.clone(),
                });
            }
        }
    }
    out.sort_by(|a, b| (&a.from, &a.to).cmp(&(&b.from, &b.to)));
    out
}

/// Инварианты, задетые изменением: AD, у которых `affects` пересекается с
/// затронутыми компонентами, либо которые затронутый CMP реализует
/// (`implements`). Охрана — `verified_by: C-NNN` против реестра правил head
/// с классом зубов из teeth.json (волна B).
fn invariants_touched(
    head: &RevisionScan,
    touched: &BTreeSet<String>,
    teeth: &TeethMap,
) -> Vec<InvariantHit> {
    let Some(model) = &head.model else {
        return Vec::new();
    };
    // Правила реестра head (материализованного CONSTRAINTS.yaml): имя по id
    // для карточек охраны. Неразборчивый реестр — имена пусты, класс зубов
    // всё равно честно «не измерены».
    let rules: Vec<crate::control::FitnessRule> = head
        .case_dir()
        .and_then(|case| crate::control::resolve_constraints_path(case, None))
        .and_then(|path| crate::control::load_fitness_rules(&path).ok())
        .unwrap_or_default();
    let mut out = Vec::new();
    for ad in model.entities.iter().filter(|e| e.kind == EntityKind::Ad) {
        let via_affects: BTreeSet<&str> = ad
            .affects
            .iter()
            .filter(|a| touched.contains(a.as_str()))
            .map(String::as_str)
            .collect();
        let via_implements: BTreeSet<&str> = model
            .entities
            .iter()
            .filter(|e| touched.contains(&e.id) && e.implements.contains(&ad.id))
            .map(|e| e.id.as_str())
            .collect();
        let via: Vec<String> = via_affects
            .union(&via_implements)
            .map(|s| (*s).to_string())
            .collect();
        if via.is_empty() {
            continue;
        }
        let rules_guard = ad
            .verified_by
            .iter()
            .filter(|v| v.starts_with("C-"))
            .map(|id| {
                let rule = rules.iter().find(|r| r.id.as_deref() == Some(id.as_str()));
                let name = rule.map(|r| r.name.clone());
                RuleGuard {
                    id: id.clone(),
                    teeth: teeth.class_of(id, name.as_deref()),
                    name,
                }
            })
            .collect();
        out.push(InvariantHit {
            ad_id: ad.id.clone(),
            ad_title: ad.title.clone(),
            via_components: via,
            rules: rules_guard,
        });
    }
    out.sort_by(|a, b| a.ad_id.cmp(&b.ad_id));
    out
}

/// ADR, затронутые изменением: `affects` по затронутым компонентам и задетым
/// инвариантам.
fn adrs_touched(
    model: Option<&Model>,
    touched: &BTreeSet<String>,
    ads: &[InvariantHit],
) -> Vec<String> {
    let Some(model) = model else {
        return Vec::new();
    };
    let ad_ids: BTreeSet<&str> = ads.iter().map(|h| h.ad_id.as_str()).collect();
    let mut out: BTreeSet<String> = BTreeSet::new();
    for e in model.entities.iter().filter(|e| e.kind == EntityKind::Adr) {
        let hit = e
            .affects
            .iter()
            .any(|a| touched.contains(a) || ad_ids.contains(a.as_str()));
        if hit {
            out.insert(e.id.clone());
        }
    }
    out.into_iter().collect()
}

/// Изменения контрактов между снимками: добавлен/удалён по составу путей,
/// изменён — классификацией `contract_diff` (ломающее/аддитивное).
fn contract_changes(
    base: &RevisionScan,
    head: &RevisionScan,
    globs: &DiffGlobs,
) -> Result<Vec<ContractChange>> {
    let base_paths = contract_paths(&base.snapshot, globs);
    let head_paths = contract_paths(&head.snapshot, globs);
    let mut out = Vec::new();
    for path in head_paths.difference(&base_paths) {
        out.push(ContractChange {
            path: path.clone(),
            classification: ContractClass::Added,
            details: Vec::new(),
        });
    }
    for path in base_paths.difference(&head_paths) {
        out.push(ContractChange {
            path: path.clone(),
            classification: ContractClass::Removed,
            details: Vec::new(),
        });
    }
    // Изменённые: путь общий, содержимое разное — классификация диффом.
    let tmp = tempfile::tempdir().map_err(|e| HarnessError::io(Path::new("arch-diff"), e))?;
    for path in base_paths.intersection(&head_paths) {
        let (Some(old), Some(new)) = (base.snapshot.content(path), head.snapshot.content(path))
        else {
            continue; // содержимое не читалось (размер/тип) — нечего сравнивать
        };
        if old == new {
            continue;
        }
        let ext = Path::new(path)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("yaml");
        let old_f = tmp.path().join(format!("old.{ext}"));
        let new_f = tmp.path().join(format!("new.{ext}"));
        std::fs::write(&old_f, old).map_err(|e| HarnessError::io(&old_f, e))?;
        std::fs::write(&new_f, new).map_err(|e| HarnessError::io(&new_f, e))?;
        match crate::contract_diff::diff_contracts(&old_f, &new_f) {
            Ok(findings) => {
                let breaking = findings.iter().any(|f| f.severity == "error");
                let details = findings
                    .iter()
                    .take(MAX_CONTRACT_DETAILS)
                    .map(|f| format!("{}: {}", f.rule, f.message))
                    .collect();
                out.push(ContractChange {
                    path: path.clone(),
                    classification: if breaking {
                        ContractClass::Breaking
                    } else {
                        ContractClass::Additive
                    },
                    details,
                });
            }
            Err(e) => out.push(ContractChange {
                path: path.clone(),
                classification: ContractClass::Unknown,
                details: vec![format!("формат не распознан: {e}")],
            }),
        }
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(out)
}

/// Модельные файлы снимка (путь → содержимое) для сравнения «модель
/// изменилась».
fn model_files(snap: &super::snapshot::Snapshot) -> BTreeMap<&str, &str> {
    snap.contents
        .iter()
        .filter(|(p, _)| p.starts_with("model/"))
        .map(|(p, c)| (p.as_str(), c.as_str()))
        .collect()
}

/// Число инстансов → f64 для JSON (выше u32 — за пределами разумного,
/// отсекаем максимумом, чтобы не терять точность молча).
fn instances_as_f64(v: u64) -> f64 {
    u32::try_from(v).map_or(f64::MAX, f64::from)
}

/// Сдвиги NFR: `nfr budget` и `nfr capacity` на материализациях base/head
/// (только если изменились файлы модели; без модели на одной из сторон
/// метрика записывается односторонне).
fn nfr_shifts(base: &RevisionScan, head: &RevisionScan) -> Vec<NfrShift> {
    if model_files(&base.snapshot) == model_files(&head.snapshot) {
        return Vec::new(); // модель не изменилась — сдвига нет
    }
    let mut out = Vec::new();
    // Бюджеты p99: цель и сумма hop'ов по каждой NFR. Битая модель ревизии —
    // не отказ диффа: сдвиг просто не считается (разбор покажет валидатор).
    let budget = |scan: &RevisionScan| -> BTreeMap<String, (f64, f64)> {
        let Some(case) = scan.case_dir() else {
            return BTreeMap::new();
        };
        crate::nfr::budget_check(case).map_or_else(
            |_| BTreeMap::new(),
            |r| {
                r.targets
                    .iter()
                    .map(|t| (t.nfr_id.clone(), (t.target_ms, t.sum_ms)))
                    .collect()
            },
        )
    };
    let (b, h) = (budget(base), budget(head));
    for nfr in b.keys().chain(h.keys()).collect::<BTreeSet<_>>() {
        let was = b.get(nfr);
        let now = h.get(nfr);
        if was == now {
            continue;
        }
        if was.map(|x| x.0) != now.map(|x| x.0) {
            out.push(NfrShift {
                check: "budget".to_string(),
                nfr: nfr.clone(),
                metric: "target_ms".to_string(),
                was: was.map(|x| x.0),
                now: now.map(|x| x.0),
            });
        }
        if was.map(|x| x.1) != now.map(|x| x.1) {
            out.push(NfrShift {
                check: "budget".to_string(),
                nfr: nfr.clone(),
                metric: "sum_ms".to_string(),
                was: was.map(|x| x.1),
                now: now.map(|x| x.1),
            });
        }
    }
    // Ёмкость: цель RPS и суммарно требуемые инстансы.
    let capacity = |scan: &RevisionScan| -> BTreeMap<String, (f64, Option<u64>)> {
        let Some(case) = scan.case_dir() else {
            return BTreeMap::new();
        };
        crate::nfr::capacity_check(case).map_or_else(
            |_| BTreeMap::new(),
            |r| {
                r.targets
                    .iter()
                    .map(|t| {
                        let req = t
                            .components
                            .iter()
                            .try_fold(0u64, |acc, c| c.required_instances.map(|r| acc + r));
                        (t.nfr_id.clone(), (t.rps, req))
                    })
                    .collect()
            },
        )
    };
    let (b, h) = (capacity(base), capacity(head));
    for nfr in b.keys().chain(h.keys()).collect::<BTreeSet<_>>() {
        let was = b.get(nfr);
        let now = h.get(nfr);
        if was == now {
            continue;
        }
        if was.map(|x| x.0) != now.map(|x| x.0) {
            out.push(NfrShift {
                check: "capacity".to_string(),
                nfr: nfr.clone(),
                metric: "rps_target".to_string(),
                was: was.map(|x| x.0),
                now: now.map(|x| x.0),
            });
        }
        if was.map(|x| x.1) != now.map(|x| x.1) {
            out.push(NfrShift {
                check: "capacity".to_string(),
                nfr: nfr.clone(),
                metric: "required_instances".to_string(),
                was: was.and_then(|x| x.1).map(instances_as_f64),
                now: now.and_then(|x| x.1).map(instances_as_f64),
            });
        }
    }
    out.sort_by(|a, b| (&a.check, &a.nfr, &a.metric).cmp(&(&b.check, &b.nfr, &b.metric)));
    out
}

/// Маршрут значимости: детектор по диффу `base...head` (три точки) в объёме
/// fail-safe с заявленными триггерами (ADR-034: детектор только добавляет).
fn route_info(
    repo: &Path,
    base: &RevisionScan,
    head: &RevisionScan,
    input: &ArchDiffInput,
) -> Result<RouteInfo> {
    let range = format!("{}...{}", base.snapshot.rev, head.snapshot.rev);
    let diff = detect_diff_triggers_with(repo, Some(&range), input.globs)?;
    let scored = score_with_sources(&input.declared, &diff, input.limits.0, input.limits.1);
    let triggers = scored
        .sources
        .iter()
        .map(|(name, source)| TriggerHit {
            name: name.clone(),
            source: source.label().to_string(),
        })
        .collect();
    Ok(RouteInfo {
        route: scored.significance.route.to_string().to_lowercase(),
        score: scored.significance.score,
        triggers,
        evidence: diff.evidence,
        undeclared: scored.undeclared,
    })
}

/// Следующий свободный id сущности префикса (`SYS-005`) — детерминированно
/// по модели head.
fn next_id(model: Option<&Model>, prefix: &str) -> String {
    let max = model.map_or(0, |m| {
        m.entities
            .iter()
            .filter_map(|e| {
                e.id.strip_prefix(prefix)
                    .and_then(|rest| rest.parse::<u32>().ok())
            })
            .max()
            .unwrap_or(0)
    });
    format!("{prefix}{:03}", max + 1)
}

/// Ключ журнала решений (K5, ADR-064): `<kind>:<from>-><to>` для ребра,
/// `node:<id>` для предложения по новому узлу.
pub(crate) fn proposal_edge_id(kind: EdgeKind, from: &str, to: &str) -> String {
    format!("{}:{}->{}", kind.label(), from, to)
}

/// Хэш оснований предложения: sha256 списка `файл:строка` ребра (для узла —
/// его id). Основания изменились — решение журнала перестаёт действовать.
fn grounds_hash_of(grounds: &str) -> String {
    crate::hash::sha256_hex(grounds.as_bytes())
}

/// Предложения правки модели по добавленным рёбрам/узлам вне модели (K3
/// только выводит; принятие — волна K5). Правка, противоречащая задетому
/// инварианту, помечается его id в `conflicts`.
fn proposals(
    head: &RevisionScan,
    added_edges: &[EdgeChange],
    added_nodes: &[ArchNode],
    invariants: &[InvariantHit],
) -> Vec<ModelProposal> {
    let mut out: Vec<ModelProposal> = Vec::new();
    for change in added_edges {
        if change.model_status != Some(ModelStatus::NotInModel) {
            continue;
        }
        let edge = &change.edge;
        let conflicts: Vec<String> = invariants
            .iter()
            .filter(|h| {
                let ids = [&edge.from, &edge.to];
                h.via_components.iter().any(|c| ids.contains(&c))
            })
            .map(|h| h.ad_id.clone())
            .collect();
        let edge_id = proposal_edge_id(edge.kind, &edge.from, &edge.to);
        let grounds_hash = grounds_hash_of(&edge.evidence.join("\n"));
        match edge.kind {
            EdgeKind::Import | EdgeKind::Calls => {
                let (Some(source_id), Some(target_id)) = (
                    head.graph
                        .node(&edge.from)
                        .and_then(|n| n.model_id.as_deref()),
                    head.graph
                        .node(&edge.to)
                        .and_then(|n| n.model_id.as_deref()),
                ) else {
                    continue; // узел без модели — предложение формирует узел ниже
                };
                let file = head
                    .model
                    .as_ref()
                    .and_then(|m| m.get(source_id))
                    .and_then(|e| head.model_file_rel(&e.file))
                    .unwrap_or_else(|| "model/".to_string());
                out.push(ModelProposal {
                    n: 0,
                    kind: ProposalKind::AddDependsOn,
                    summary: format!("{file}: depends_on += {target_id}"),
                    file,
                    conflicts,
                    edge_id,
                    grounds_hash,
                });
            }
            EdgeKind::Connect => {
                // Внешняя система и хранилище в типизированной модели — SYS
                // (отдельного вида «хранилище» в модели нет).
                let host = host_of_node(&edge.to).unwrap_or(&edge.to);
                let id = next_id(head.model.as_ref(), "SYS-");
                let slug = crate::control::kebab_slug(host);
                out.push(ModelProposal {
                    n: 0,
                    kind: ProposalKind::NewEntity,
                    file: format!("model/{id}-{slug}.md"),
                    summary: format!(
                        "новая сущность {id} (внешняя система/хранилище {host}) + связь от {}",
                        edge.from
                    ),
                    conflicts,
                    edge_id,
                    grounds_hash,
                });
            }
            EdgeKind::ContractRef => {
                let path = edge.to.trim_start_matches("contract:");
                let id = next_id(head.model.as_ref(), "INT-");
                out.push(ModelProposal {
                    n: 0,
                    kind: ProposalKind::NewEntity,
                    file: format!("model/{id}-{}.md", crate::control::kebab_slug(path)),
                    summary: format!("новая сущность {id} с contract: {path}"),
                    conflicts,
                    edge_id,
                    grounds_hash,
                });
            }
        }
    }
    // Новые компоненты вне модели (inferred-узлы).
    for node in added_nodes {
        if node.kind != NodeKind::Component || !node.inferred {
            continue;
        }
        let dir = node.id.trim_start_matches("dir:");
        let id = next_id(head.model.as_ref(), "CMP-");
        out.push(ModelProposal {
            n: 0,
            kind: ProposalKind::NewEntity,
            file: format!("model/{id}-{}.md", crate::control::kebab_slug(dir)),
            summary: format!("новый CMP {id} с code_roots: [{dir}]"),
            conflicts: Vec::new(),
            edge_id: format!("node:{}", node.id),
            grounds_hash: grounds_hash_of(&node.id),
        });
    }
    for (i, p) in out.iter_mut().enumerate() {
        p.n = i + 1;
    }
    out
}

/// Ключ ребра для сравнения графов (fn, а не замыкание: у замыкания с
/// ссылками в выходе вывод времени жизни не привязывает их ко входу).
fn edge_key(e: &ArchEdge) -> (&str, &str, EdgeKind) {
    (e.from.as_str(), e.to.as_str(), e.kind)
}

/// Архитектурный дифф `base..head` репозитория `repo` (K2). Рабочее дерево
/// не изменяется.
///
/// # Errors
/// Не git-репозиторий, ревизии не разрешаются, снимок/граф не строится,
/// детектор триггеров недоступен.
pub fn arch_diff(repo: &Path, input: &ArchDiffInput) -> Result<ArchDiff> {
    let base_ref = crate::control::base_rev(input.base);
    let base = scan_revision(repo, base_ref, input.globs)?;
    let head = scan_revision(repo, input.head.unwrap_or("HEAD"), input.globs)?;

    let base_nodes: BTreeMap<&str, &ArchNode> = base
        .graph
        .nodes
        .iter()
        .map(|n| (n.id.as_str(), n))
        .collect();
    let head_nodes: BTreeMap<&str, &ArchNode> = head
        .graph
        .nodes
        .iter()
        .map(|n| (n.id.as_str(), n))
        .collect();
    let added_nodes: Vec<ArchNode> = head_nodes
        .iter()
        .filter(|entry| !base_nodes.contains_key(entry.0))
        .map(|(_, n)| (*n).clone())
        .collect();
    let removed_nodes: Vec<ArchNode> = base_nodes
        .iter()
        .filter(|entry| !head_nodes.contains_key(entry.0))
        .map(|(_, n)| (*n).clone())
        .collect();

    let base_edges: BTreeMap<(&str, &str, EdgeKind), &ArchEdge> =
        base.graph.edges.iter().map(|e| (edge_key(e), e)).collect();
    let head_edges: BTreeMap<(&str, &str, EdgeKind), &ArchEdge> =
        head.graph.edges.iter().map(|e| (edge_key(e), e)).collect();
    let added_edges: Vec<EdgeChange> = head_edges
        .iter()
        .filter(|entry| !base_edges.contains_key(entry.0))
        .map(|(_, e)| EdgeChange {
            edge: (*e).clone(),
            model_status: edge_model_status(&head.graph, head.model.as_ref(), e),
        })
        .collect();
    let removed_edges: Vec<EdgeChange> = base_edges
        .iter()
        .filter(|entry| !head_edges.contains_key(entry.0))
        .map(|(_, e)| EdgeChange {
            edge: (*e).clone(),
            model_status: edge_model_status(&base.graph, base.model.as_ref(), e),
        })
        .collect();

    // Затронутые компоненты: концы добавленных/удалённых рёбер с id модели.
    let mut touched: BTreeSet<String> = BTreeSet::new();
    for change in added_edges.iter().chain(&removed_edges) {
        for id in [&change.edge.from, &change.edge.to] {
            if let Some(model_id) = head
                .graph
                .node(id)
                .or_else(|| base.graph.node(id))
                .and_then(|n| n.model_id.clone())
            {
                touched.insert(model_id);
            }
        }
    }
    let teeth = load_teeth(repo);
    let invariants = invariants_touched(&head, &touched, &teeth);
    let adrs = adrs_touched(head.model.as_ref(), &touched, &invariants);
    let contracts = contract_changes(&base, &head, input.globs)?;
    let nfr = nfr_shifts(&base, &head);
    let route = route_info(repo, &base, &head, input)?;
    let mut proposals = proposals(&head, &added_edges, &added_nodes, &invariants);
    // K5 (ADR-064): решённое журналом (`.arch-handoff/arch-diff-decisions.json`)
    // не предлагается повторно, пока не изменились основания ребра; номера
    // оставшихся предложений сохраняются (accept/reject ссылаются на них).
    super::decisions::suppress_decided(repo, &mut proposals)?;

    Ok(ArchDiff {
        base: base.snapshot.rev.clone(),
        head: head.snapshot.rev.clone(),
        added_nodes,
        removed_nodes,
        added_edges,
        removed_edges,
        declared_unused: declared_unused(&head),
        invariants_touched: invariants,
        adrs_touched: adrs,
        contract_changes: contracts,
        nfr_shifts: nfr,
        route,
        proposals,
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::super::snapshot::tests::{git_in, git_repo, write_file};
    use super::*;

    /// Коммит поверх текущего состояния фикстуры (пустой — тоже коммит).
    pub(crate) fn commit_all(repo: &Path, msg: &str) {
        git_in(repo, &["add", "-A"]);
        git_in(repo, &["commit", "-q", "--allow-empty", "-m", msg]);
    }

    /// Вход по умолчанию: без заявленных триггеров, дефолтные пороги/глобы.
    pub(crate) fn input<'a>(globs: &'a DiffGlobs, base: &'a str) -> ArchDiffInput<'a> {
        ArchDiffInput {
            base,
            head: None,
            declared: BTreeMap::new(),
            limits: (1, 4),
            globs,
        }
    }

    /// Кейс в духе демо-сценария раздела 9 (salary-payments): приём реестров,
    /// ядро проводок, оркестратор; AD-2 «Проводки только через Оркестратор»
    /// с охранником C-007; ADR-003 затрагивает CMP-001.
    pub(crate) fn fixture_case(repo: &Path) {
        write_file(
            repo,
            "model/CMP-001-intake.md",
            "---\nid: CMP-001\ntype: cmp\ntitle: Приём реестров\nstatus: adopted\ncode_roots: [skeleton/intake]\ndepends_on: [CMP-009]\n---\nПриём.\n",
        );
        write_file(
            repo,
            "model/CMP-004-ledger.md",
            "---\nid: CMP-004\ntype: cmp\ntitle: \"Ядро: счета и проводки\"\nstatus: adopted\ncode_roots: [skeleton/ledger]\n---\nЯдро.\n",
        );
        write_file(
            repo,
            "model/AD-2-ledger-via-orchestrator.md",
            "---\nid: AD-2\ntype: ad\ntitle: Проводки только через Оркестратор\nstatus: accepted\naffects: [CMP-001]\nverified_by: [C-007]\n---\nИнвариант.\n",
        );
        write_file(
            repo,
            "model/ADR-003-intake-shape.md",
            "---\nid: ADR-003\ntype: adr\ntitle: Форма приёма\nstatus: accepted\naffects: [CMP-001]\n---\nРешение.\n",
        );
        write_file(
            repo,
            "CONSTRAINTS.yaml",
            "rules:\n  - id: C-007\n    name: no-direct-ledger-write\n    type: must_not_contain\n    glob: 'skeleton/intake/**'\n    pattern: 'ledger_db'\n",
        );
        // Код base: приём ходит в оркестратор (объявлено и используется),
        // оркестратор объявил зависимость от ядра, но не использует (C2).
        write_file(
            repo,
            "skeleton/intake/writer.py",
            "from skeleton.orchestrator import api\n\ndef write():\n    api.post()\n",
        );
        write_file(
            repo,
            "skeleton/orchestrator/api.py",
            "def post():\n    pass\n",
        );
        write_file(repo, "skeleton/ledger/client.py", "def post():\n    pass\n");
        write_file(
            repo,
            "model/CMP-009-orchestrator.md",
            "---\nid: CMP-009\ntype: cmp\ntitle: Оркестратор\nstatus: adopted\ncode_roots: [skeleton/orchestrator]\ndepends_on: [CMP-004]\n---\nОркестратор.\n",
        );
    }

    /// Демо-правка агента: прямая запись в ядро в обход оркестратора.
    pub(crate) fn fixture_agent_change(repo: &Path) {
        write_file(
            repo,
            "skeleton/intake/writer.py",
            "from skeleton.orchestrator import api\nfrom skeleton.ledger import client\nimport ledger_db\n\ndef write():\n    client.post()\n",
        );
        write_file(
            repo,
            "skeleton/intake/config.yaml",
            "dsn: \"postgres://ledger-db:5432/ledger\"\n",
        );
    }

    /// Демо-сценарий K3 (раздел 9 задания): новое ребро вне модели, задетый
    /// инвариант, затронутый ADR, новое хранилище, маршрут с источником diff
    /// и пронумерованное предложение правки модели с ⚠-конфликтом.
    #[test]
    fn demo_direct_ledger_write() {
        let dir = tempfile::tempdir().expect("tmp");
        let repo = dir.path().join("case");
        std::fs::create_dir_all(&repo).expect("mkdir");
        fixture_case(&repo);
        git_repo(&repo);
        fixture_agent_change(&repo);
        commit_all(&repo, "agent/direct-ledger-write");

        let globs = DiffGlobs::default();
        let diff = arch_diff(&repo, &input(&globs, "main~1")).expect("дифф");

        // Реальность PR: новое ребро CMP-001 → CMP-004 и узел хранилища.
        let edge = diff
            .added_edges
            .iter()
            .find(|e| e.edge.from == "CMP-001" && e.edge.to == "CMP-004")
            .expect("новое ребро");
        assert_eq!(edge.edge.kind, EdgeKind::Import);
        assert_eq!(edge.model_status, Some(ModelStatus::NotInModel));
        assert!(
            edge.edge
                .evidence
                .contains(&"skeleton/intake/writer.py:2".to_string()),
            "{:?}",
            edge.edge.evidence
        );
        assert!(
            diff.added_nodes
                .iter()
                .any(|n| n.id == "store:postgres://ledger-db:5432"),
            "{:?}",
            diff.added_nodes
        );

        // Смысл: задет AD-2 с правилом C-007 (зубы не измерены — файла нет),
        // затронут ADR-003; объявленное ребро CMP-009 → CMP-004 без кода.
        let ad = diff
            .invariants_touched
            .iter()
            .find(|h| h.ad_id == "AD-2")
            .expect("AD-2 задет");
        assert_eq!(ad.via_components, vec!["CMP-001".to_string()]);
        let rule = ad.rules.iter().find(|r| r.id == "C-007").expect("C-007");
        assert_eq!(rule.name.as_deref(), Some("no-direct-ledger-write"));
        assert_eq!(rule.teeth, TeethClass::Unknown);
        assert_eq!(diff.adrs_touched, vec!["ADR-003".to_string()]);
        assert!(
            diff.declared_unused
                .iter()
                .any(|d| d.from == "CMP-009" && d.to == "CMP-004"),
            "{:?}",
            diff.declared_unused
        );

        // Маршрут: new_datastore от детектора (источник diff).
        let trig = diff
            .route
            .triggers
            .iter()
            .find(|t| t.name == "new_datastore")
            .expect("new_datastore");
        assert_eq!(trig.source, "diff");
        assert!(diff.route.undeclared.contains(&"new_datastore".to_string()));

        // Предложение модели №1: depends_on += CMP-004 с конфликтом AD-2.
        let p1 = diff.proposals.first().expect("предложение");
        assert_eq!(p1.n, 1);
        assert_eq!(p1.kind, ProposalKind::AddDependsOn);
        assert_eq!(p1.file, "model/CMP-001-intake.md");
        assert!(p1.summary.contains("depends_on += CMP-004"), "{p1:?}");
        assert_eq!(p1.conflicts, vec!["AD-2".to_string()]);
        // И предложение новой SYS по хранилищу.
        assert!(
            diff.proposals
                .iter()
                .any(|p| p.kind == ProposalKind::NewEntity && p.summary.contains("ledger-db")),
            "{:?}",
            diff.proposals
        );
        assert!(diff.has_undeclared_edges());
        assert!(diff.has_invariant_touched());
        assert!(!diff.has_breaking_contracts());
        assert!(!diff.is_empty());
    }

    /// Чистый рефакторинг внутри одного компонента (переименование функции и
    /// перенос файла внутри `code_roots`) — НЕ изменение архитектуры:
    /// пустой дифф (`git diff -M` не создаёт ложных «удалено/добавлено»).
    #[test]
    fn refactoring_inside_component_is_empty_diff() {
        let dir = tempfile::tempdir().expect("tmp");
        let repo = dir.path().join("case");
        std::fs::create_dir_all(&repo).expect("mkdir");
        fixture_case(&repo);
        git_repo(&repo);
        // Переименование функции + перенос файла внутри того же компонента.
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
        assert!(diff.is_empty(), "{diff:?}");
        assert_eq!(diff.proposals, Vec::new(), "предложений не предвидится");
        assert_eq!(diff.invariants_touched, Vec::new());
    }

    /// Удаление ребра: код импорта убран, объявление в модели осталось —
    /// ребро уходит в removed (в модели было), а на head появляется
    /// declared-unused.
    #[test]
    fn removed_edge_and_declared_unused() {
        let dir = tempfile::tempdir().expect("tmp");
        let repo = dir.path().join("case");
        std::fs::create_dir_all(&repo).expect("mkdir");
        write_file(
            &repo,
            "model/CMP-001-intake.md",
            "---\nid: CMP-001\ntype: cmp\ntitle: Приём\nstatus: adopted\ncode_roots: [skeleton/intake]\ndepends_on: [CMP-004]\n---\n",
        );
        write_file(
            &repo,
            "model/CMP-004-ledger.md",
            "---\nid: CMP-004\ntype: cmp\ntitle: Ядро\nstatus: adopted\ncode_roots: [skeleton/ledger]\n---\n",
        );
        write_file(
            &repo,
            "skeleton/intake/writer.py",
            "from skeleton.ledger import client\n",
        );
        write_file(
            &repo,
            "skeleton/ledger/client.py",
            "def post():\n    pass\n",
        );
        git_repo(&repo);
        // head: импорт убран, depends_on остался.
        write_file(
            &repo,
            "skeleton/intake/writer.py",
            "def write():\n    pass\n",
        );
        commit_all(&repo, "drop-import");

        let globs = DiffGlobs::default();
        let diff = arch_diff(&repo, &input(&globs, "main~1")).expect("дифф");
        let removed = diff
            .removed_edges
            .iter()
            .find(|e| e.edge.from == "CMP-001" && e.edge.to == "CMP-004")
            .expect("удалённое ребро");
        assert_eq!(removed.model_status, Some(ModelStatus::InModel));
        assert!(
            diff.declared_unused
                .iter()
                .any(|d| d.from == "CMP-001" && d.to == "CMP-004"),
            "{:?}",
            diff.declared_unused
        );
        assert!(!diff.has_undeclared_edges());
    }

    /// Контракты: удаление операции — ломающее; добавление необязательного
    /// поля/операции — аддитивное; новый файл контракта — added.
    #[test]
    fn contract_classification_breaking_additive_added() {
        let dir = tempfile::tempdir().expect("tmp");
        let repo = dir.path().join("case");
        std::fs::create_dir_all(&repo).expect("mkdir");
        write_file(
            &repo,
            "contracts/pay-openapi.yaml",
            "openapi: 3.0.3\ninfo: {title: Pay, version: 1.0.0}\npaths:\n  /pay:\n    post:\n      responses: {'200': {description: ok}}\n",
        );
        git_repo(&repo);
        write_file(
            &repo,
            "contracts/pay-openapi.yaml",
            "openapi: 3.0.3\ninfo: {title: Pay, version: 1.0.1}\npaths:\n  /pay:\n    post:\n      responses: {'200': {description: ok}}\n  /refund:\n    post:\n      responses: {'200': {description: ok}}\n",
        );
        commit_all(&repo, "additive");
        let globs = DiffGlobs::default();
        let diff = arch_diff(&repo, &input(&globs, "main~1")).expect("дифф");
        let change = diff
            .contract_changes
            .iter()
            .find(|c| c.path == "contracts/pay-openapi.yaml")
            .expect("изменение контракта");
        assert_eq!(change.classification, ContractClass::Additive, "{change:?}");

        // Удаление операции — ломающее.
        write_file(
            &repo,
            "contracts/pay-openapi.yaml",
            "openapi: 3.0.3\ninfo: {title: Pay, version: 1.1.0}\npaths:\n  /refund:\n    post:\n      responses: {'200': {description: ok}}\n",
        );
        write_file(
            &repo,
            "contracts/status-openapi.yaml",
            "openapi: 3.0.3\ninfo: {title: Status, version: 0.1.0}\npaths: {}\n",
        );
        commit_all(&repo, "breaking");
        let diff = arch_diff(&repo, &input(&globs, "main~1")).expect("дифф");
        let breaking = diff
            .contract_changes
            .iter()
            .find(|c| c.path == "contracts/pay-openapi.yaml")
            .expect("ломающее");
        assert_eq!(
            breaking.classification,
            ContractClass::Breaking,
            "{breaking:?}"
        );
        assert!(
            diff.contract_changes
                .iter()
                .any(|c| c.path == "contracts/status-openapi.yaml"
                    && c.classification == ContractClass::Added),
            "{:?}",
            diff.contract_changes
        );
        assert!(diff.has_breaking_contracts());
    }

    /// Сдвиг NFR: изменение бюджета hop'а INT между ревизиями даёт сдвиг
    /// суммы бюджета цепочки (было → стало).
    #[test]
    fn nfr_budget_shift() {
        let dir = tempfile::tempdir().expect("tmp");
        let repo = dir.path().join("case");
        std::fs::create_dir_all(&repo).expect("mkdir");
        write_file(
            &repo,
            "model/NFR-001-p99.md",
            "---\nid: NFR-001\ntype: nfr\ntitle: p99 приёма\nstatus: accepted\np99_target_ms: 1000\naffects: [INT-001]\nverification: nfr budget\n---\n",
        );
        write_file(
            &repo,
            "model/INT-001-rail.md",
            "---\nid: INT-001\ntype: int\ntitle: Рельс\nstatus: accepted\nlatency_budget_ms: 300\n---\n",
        );
        write_file(&repo, "app.py", "def main():\n    pass\n");
        git_repo(&repo);
        write_file(
            &repo,
            "model/INT-001-rail.md",
            "---\nid: INT-001\ntype: int\ntitle: Рельс\nstatus: accepted\nlatency_budget_ms: 450\n---\n",
        );
        commit_all(&repo, "budget-450");

        let globs = DiffGlobs::default();
        let diff = arch_diff(&repo, &input(&globs, "main~1")).expect("дифф");
        let shift = diff
            .nfr_shifts
            .iter()
            .find(|s| s.nfr == "NFR-001" && s.metric == "sum_ms")
            .expect("сдвиг суммы бюджета");
        assert_eq!(shift.was, Some(300.0));
        assert_eq!(shift.now, Some(450.0));
    }

    /// Заявленный триггер объединяется с детектором (fail-safe ADR-034):
    /// источник declared+diff, когда и флаг, и дифф видят одно и то же.
    #[test]
    fn declared_and_diff_trigger_sources() {
        let dir = tempfile::tempdir().expect("tmp");
        let repo = dir.path().join("case");
        std::fs::create_dir_all(&repo).expect("mkdir");
        fixture_case(&repo);
        git_repo(&repo);
        fixture_agent_change(&repo);
        commit_all(&repo, "change");

        let globs = DiffGlobs::default();
        let mut declared = BTreeMap::new();
        declared.insert("new_datastore".to_string(), true);
        declared.insert("financial_impact".to_string(), true);
        let input = ArchDiffInput {
            base: "main~1",
            head: None,
            declared,
            limits: (1, 4),
            globs: &globs,
        };
        let diff = arch_diff(&repo, &input).expect("дифф");
        let by_name = |n: &str| diff.route.triggers.iter().find(|t| t.name == n);
        assert_eq!(
            by_name("new_datastore").map(|t| t.source.as_str()),
            Some("declared+diff")
        );
        assert_eq!(
            by_name("financial_impact").map(|t| t.source.as_str()),
            Some("declared")
        );
        assert_eq!(diff.route.undeclared, Vec::<String>::new());
    }

    /// Детерминизм (правило 10): два прогона диффа на тех же коммитах —
    /// байт-в-байт тот же JSON.
    #[test]
    fn arch_diff_is_deterministic() {
        let dir = tempfile::tempdir().expect("tmp");
        let repo = dir.path().join("case");
        std::fs::create_dir_all(&repo).expect("mkdir");
        fixture_case(&repo);
        git_repo(&repo);
        fixture_agent_change(&repo);
        commit_all(&repo, "change");

        let globs = DiffGlobs::default();
        let a = arch_diff(&repo, &input(&globs, "main~1")).expect("дифф");
        let b = arch_diff(&repo, &input(&globs, "main~1")).expect("дифф");
        let ja = serde_json::to_string_pretty(&a).expect("json");
        let jb = serde_json::to_string_pretty(&b).expect("json");
        assert_eq!(ja, jb, "дифф обязан быть байт-в-байт воспроизводим");
    }

    /// Без модели: дифф строится по манифестам, статус «в модели» не
    /// применим (None), инвариантов нет.
    #[test]
    fn diff_without_model() {
        let dir = tempfile::tempdir().expect("tmp");
        let repo = dir.path().join("case");
        std::fs::create_dir_all(&repo).expect("mkdir");
        write_file(
            &repo,
            "intake/pyproject.toml",
            "[project]\nname = \"intake\"\n",
        );
        write_file(&repo, "intake/app.py", "X = 1\n");
        write_file(
            &repo,
            "ledger/pyproject.toml",
            "[project]\nname = \"ledger\"\n",
        );
        write_file(&repo, "ledger/core.py", "Y = 2\n");
        git_repo(&repo);
        write_file(&repo, "intake/app.py", "import ledger.core\nX = 1\n");
        commit_all(&repo, "link");

        let globs = DiffGlobs::default();
        let diff = arch_diff(&repo, &input(&globs, "main~1")).expect("дифф");
        let edge = diff
            .added_edges
            .iter()
            .find(|e| e.edge.kind == EdgeKind::Import)
            .expect("ребро");
        assert_eq!(edge.model_status, None, "модели нет — колонка не применима");
        assert_eq!(diff.invariants_touched, Vec::new());
        assert!(
            diff.proposals
                .iter()
                .all(|p| p.kind == ProposalKind::NewEntity),
            "без модели — только предложения новых сущностей: {:?}",
            diff.proposals
        );
    }

    /// teeth.json волны B: правило с «confirmed» читается как подтверждённые
    /// зубы; без файла — «не измерены».
    #[test]
    fn teeth_json_marks_confirmed_rules() {
        let dir = tempfile::tempdir().expect("tmp");
        let repo = dir.path().join("case");
        std::fs::create_dir_all(&repo).expect("mkdir");
        fixture_case(&repo);
        git_repo(&repo);
        fixture_agent_change(&repo);
        commit_all(&repo, "change");
        // teeth.json — рабочее дерево (артефакт измерения, не коммит).
        write_file(
            &repo,
            ".arch-handoff/teeth.json",
            "{\"rules\": {\"no-direct-ledger-write\": {\"teeth\": \"confirmed\"}}}\n",
        );

        let globs = DiffGlobs::default();
        let diff = arch_diff(&repo, &input(&globs, "main~1")).expect("дифф");
        let ad = diff
            .invariants_touched
            .iter()
            .find(|h| h.ad_id == "AD-2")
            .expect("AD-2");
        let rule = ad.rules.iter().find(|r| r.id == "C-007").expect("C-007");
        assert_eq!(rule.teeth, TeethClass::Confirmed);
    }

    /// Битый журнал решений (K5) — громкая ошибка диффа, а не молчаливый
    /// сброс решений: потерянные отказы хуже упавшего прогона.
    #[test]
    fn corrupt_decisions_journal_fails_diff() {
        let dir = tempfile::tempdir().expect("tmp");
        let repo = dir.path().join("case");
        std::fs::create_dir_all(&repo).expect("mkdir");
        fixture_case(&repo);
        git_repo(&repo);
        fixture_agent_change(&repo);
        commit_all(&repo, "change");
        write_file(&repo, ".arch-handoff/arch-diff-decisions.json", "{битый");

        let globs = DiffGlobs::default();
        let err = arch_diff(&repo, &input(&globs, "main~1")).expect_err("битый журнал");
        assert!(err.to_string().contains("не разбирается"), "{err}");
    }

    /// `--fail-on` (K3): срабатывают только названные условия; пустой список
    /// — всегда зелёный (информационный дифф).
    #[test]
    fn fail_on_matched_failures() {
        let dir = tempfile::tempdir().expect("tmp");
        let repo = dir.path().join("case");
        std::fs::create_dir_all(&repo).expect("mkdir");
        fixture_case(&repo);
        git_repo(&repo);
        fixture_agent_change(&repo);
        commit_all(&repo, "change");
        let globs = DiffGlobs::default();
        let diff = arch_diff(&repo, &input(&globs, "main~1")).expect("дифф");

        assert_eq!(matched_failures(&diff, &[]), Vec::<&str>::new());
        assert_eq!(
            matched_failures(&diff, &[FailOn::UndeclaredEdge]),
            vec!["undeclared-edge"]
        );
        assert_eq!(
            matched_failures(&diff, &[FailOn::InvariantTouched]),
            vec!["invariant-touched"]
        );
        assert_eq!(
            matched_failures(&diff, &[FailOn::BreakingContract]),
            Vec::<&str>::new(),
            "контрактов нет — breaking-contract не срабатывает"
        );
        assert_eq!(
            matched_failures(&diff, &[FailOn::BreakingContract, FailOn::UndeclaredEdge]),
            vec!["undeclared-edge"]
        );
        // Разбор имён флага.
        assert_eq!(
            FailOn::from_name("undeclared-edge"),
            Some(FailOn::UndeclaredEdge)
        );
        assert_eq!(FailOn::from_name("nope"), None);
        assert_eq!(FailOn::NAMES.len(), 3);
    }
}
