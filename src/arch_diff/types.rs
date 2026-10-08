//! Типы графа «как построено» и архитектурного диффа (волна K, ADR-063).
//!
//! Контракт детерминизма (правило 10): узлы и рёбра отсортированы, id
//! стабильны между прогонами одной ревизии, сериализация — через serde
//! (повторный прогон на тех же коммитах даёт байт-в-байт тот же JSON).

use serde::Serialize;

/// Имя машинного контракта JSON-вывода (`--format json`), как у
/// `gate-verdict/v1`: потребители сверяют поле `schema` перед разбором.
pub const ARCH_DIFF_SCHEMA: &str = "arch-be/arch-diff/v1";

/// Потолок оснований `файл:строка` у одного ребра (дальше — счётчик
/// сокращения в последнем элементе `… (+N)`).
pub const MAX_EDGE_EVIDENCE: usize = 10;

/// Вид узла графа as-built.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeKind {
    /// Компонент (по `code_roots` CMP модели либо выведенный по манифесту).
    Component,
    /// Внешняя система (`host:port` из конфигов, семантика `survey`).
    ExternalSystem,
    /// Хранилище (строка подключения, семантика детектора `new_datastore`).
    Datastore,
    /// Контракт (файл OpenAPI/AsyncAPI/protobuf по детектору T-05).
    Contract,
}

impl NodeKind {
    /// Метка вида для текста/JSON.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Component => "component",
            Self::ExternalSystem => "external_system",
            Self::Datastore => "datastore",
            Self::Contract => "contract",
        }
    }
}

/// Узел графа as-built.
///
/// Стабильный id: сущность модели — её id (`CMP-001`); выведенный компонент —
/// `dir:<каталог>`; внешняя система — `sys:<host:port>`; хранилище —
/// `store:<scheme>://<host>`; контракт — `contract:<путь>`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ArchNode {
    /// Стабильный идентификатор узла.
    pub id: String,
    /// Вид узла.
    pub kind: NodeKind,
    /// Человекочитаемый заголовок (из модели либо по id).
    pub title: String,
    /// Выведен по манифесту сборки (нет модели/`code_roots`).
    pub inferred: bool,
    /// Id сущности модели, которой соответствует узел (если есть).
    pub model_id: Option<String>,
}

/// Вид ребра графа as-built.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EdgeKind {
    /// Импорт модулей цели из компонента-источника (`imports`).
    Import,
    /// Обращение к внешней системе/хранилищу (строка подключения в конфиге).
    Connect,
    /// Публикация/реализация контракта (файл контракта в корне компонента).
    ContractRef,
}

impl EdgeKind {
    /// Метка вида для текста/JSON.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Import => "import",
            Self::Connect => "connect",
            Self::ContractRef => "contract",
        }
    }
}

/// Ребро графа as-built с основаниями `файл:строка` (отсортированы, без
/// дублей, ограничены [`MAX_EDGE_EVIDENCE`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ArchEdge {
    /// Id узла-источника.
    pub from: String,
    /// Id узла-цели.
    pub to: String,
    /// Вид ребра.
    pub kind: EdgeKind,
    /// Основания `файл:строка` (детерминированный порядок).
    pub evidence: Vec<String>,
}

/// Граф «как построено» по снимку одной ревизии.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ArchGraph {
    /// Полный sha ревизии, по которой построен граф.
    pub rev: String,
    /// Узлы (отсортированы по id).
    pub nodes: Vec<ArchNode>,
    /// Рёбра (отсортированы по from/to/kind).
    pub edges: Vec<ArchEdge>,
}

impl ArchGraph {
    /// Узел по id.
    #[must_use]
    pub fn node(&self, id: &str) -> Option<&ArchNode> {
        self.nodes.iter().find(|n| n.id == id)
    }

    /// Число узлов вида.
    #[must_use]
    pub fn count_kind(&self, kind: NodeKind) -> usize {
        self.nodes.iter().filter(|n| n.kind == kind).count()
    }
}
