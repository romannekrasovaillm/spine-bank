//! Модель отчёта о восстановленной архитектуре — контракт `arch-be/report/v1`
//! (ADR-080, этап 1).
//!
//! ЕДИНСТВЕННЫЙ источник истины (AD-11 дельты `report-v1`): сбор модели и
//! назначение ID/уверенности живут только здесь; CLI и будущий MCP-мост —
//! тонкие обёртки над [`crate::report::build`].
//!
//! Детерминизм (правило 10): поля сортированы, время берётся из ревизии git,
//! а не из часов, сериализация — serde. Повторный прогон одной ревизии даёт
//! байт-в-байт тот же JSON.

use std::collections::BTreeMap;

use serde::Serialize;

/// Имя машинного контракта JSON-вывода (`--format json`). Потребители
/// сверяют поле `schema` перед разбором (как у `recover/v1`).
pub const REPORT_SCHEMA: &str = "arch-be/report/v1";

/// Статус модели: отчёт восстановлен механически и не верифицирован
/// человеком. Строка попадает в `meta.status` и в шапку markdown.
pub const STATUS_AUTO: &str = "Автоматически восстановлено, не верифицировано человеком";

/// Уверенность элемента A/B/C (ADR-080).
///
/// - `A` — элемент опирается на модель (`model_id`) либо на подтверждённое
///   ребро (`confidence = confirmed`);
/// - `B` — выведено эвристикой/сканером (например, компонент по манифесту);
/// - `C` — пробел (`[gap]`), LLM-блок или кандидат в решение.
///
/// LLM-слой не может назначить `A`/`B` — только `C`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub enum ConfidenceAbc {
    /// Модель или подтверждённое ребро.
    A,
    /// Выведено сканером.
    B,
    /// Пробел/LLM/кандидат.
    C,
}

impl ConfidenceAbc {
    /// Метка для текста/JSON (`A`/`B`/`C`).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::A => "A",
            Self::B => "B",
            Self::C => "C",
        }
    }

    /// Требует ли уверенность непустого evidence (инвариант и1): для `A`/`B`
    /// доказательство обязательно, для `C` — нет.
    #[must_use]
    pub fn requires_evidence(self) -> bool {
        matches!(self, Self::A | Self::B)
    }
}

/// Семейство report-ID: префикс и назначение.
///
/// - `CTX` — внешние системы/контекст;
/// - `CNT` — контейнеры (верхнеуровневые компоненты, хранилища, топология);
/// - `CMP` — компоненты внутри контейнеров;
/// - `INT` — интерфейсы (контракты, топики, точки входа);
/// - `RISK` — риски (циклы, god-модули, мёртвый код, churn, пробелы);
/// - `ADR` — кандидаты в архитектурные решения.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Family {
    /// Внешняя система/контекст.
    Context,
    /// Контейнер.
    Container,
    /// Компонент.
    Component,
    /// Интерфейс.
    Interface,
    /// Риск.
    Risk,
    /// Кандидат в решение.
    Decision,
}

impl Family {
    /// Все семейства в каноничном порядке (порядок нумерации ID).
    pub const ALL: [Self; 6] = [
        Self::Context,
        Self::Container,
        Self::Component,
        Self::Interface,
        Self::Risk,
        Self::Decision,
    ];

    /// Префикс report-ID (`CTX`/`CNT`/`CMP`/`INT`/`RISK`/`ADR`).
    #[must_use]
    pub fn prefix(self) -> &'static str {
        match self {
            Self::Context => "CTX",
            Self::Container => "CNT",
            Self::Component => "CMP",
            Self::Interface => "INT",
            Self::Risk => "RISK",
            Self::Decision => "ADR",
        }
    }

    /// Человекочитаемая метка семейства.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Context => "внешняя система",
            Self::Container => "контейнер",
            Self::Component => "компонент",
            Self::Interface => "интерфейс",
            Self::Risk => "риск",
            Self::Decision => "кандидат в решение",
        }
    }
}

/// Элемент отчёта: единица трассируемости с report-ID, уверенностью и
/// доказательствами `файл:строка`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReportElement {
    /// Report-ID (`CTX-01`, `CMP-03`, …); стабилен между прогонами ревизии.
    pub id: String,
    /// Семейство ID.
    pub family: Family,
    /// Id узла графа `recover`, которому соответствует элемент (если есть) —
    /// биективная связь отчёт↔граф.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub graph_id: Option<String>,
    /// Заголовок элемента.
    pub title: String,
    /// Уверенность A/B/C.
    pub confidence: ConfidenceAbc,
    /// Доказательства `путь:строка` (отсортированы, без дублей).
    pub evidence: Vec<String>,
    /// Дополнительные атрибуты (вид узла, признак вывода, id модели и пр.).
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub props: BTreeMap<String, String>,
}

/// Связь между элементами отчёта (report-ID → report-ID), выведенная из ребра
/// графа `recover`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReportRelation {
    /// Report-ID источника.
    pub from: String,
    /// Report-ID цели.
    pub to: String,
    /// Вид связи (метка [`crate::arch_diff::EdgeKind`]).
    pub kind: String,
    /// Доказательства `путь:строка`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<String>,
}

/// Шапка модели: система, репозиторий, ревизия и сводная статистика.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReportMeta {
    /// Имя системы (имя каталога репозитория).
    pub system: String,
    /// Путь/имя репозитория.
    pub repo: String,
    /// Текущая ветка (если определима).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// Полный sha ревизии, по которой построен отчёт.
    pub rev: String,
    /// Дата ревизии (из git, НЕ из часов — детерминизм).
    pub generated_at: String,
    /// Статус отчёта ([`STATUS_AUTO`]).
    pub status: String,
    /// Счётчики разделов (элементы по семействам, связи и пр.).
    pub statistics: BTreeMap<String, usize>,
    /// Распределение уверенности `{"A": n, "B": n, "C": n}`.
    pub confidence_distribution: BTreeMap<String, usize>,
}

/// Характер раздела отчёта: детерминированный или интерпретационный.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SectionKind {
    /// Наполняется механически из графа/сканеров.
    Deterministic,
    /// Содержит блоки `[Интерпретация Spine]`/`[gap]` — заполняет человек/LLM.
    Interpretive,
}

/// Строка раздела (таблицы/списка) с ссылками-доказательствами.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SectionItem {
    /// Текст строки.
    pub label: String,
    /// Ссылки-доказательства `путь:строка`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub refs: Vec<String>,
    /// Необязательная пометка (например, `[gap]`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// Машинные данные раздела отчёта (§1–§14).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReportSection {
    /// Номер раздела (`1`..`14`).
    pub id: String,
    /// Заголовок раздела.
    pub title: String,
    /// Характер раздела.
    pub kind: SectionKind,
    /// Report-ID элементов раздела (каждый обязан существовать в `elements`).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub element_ids: Vec<String>,
    /// Счётчики раздела.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub counts: BTreeMap<String, usize>,
    /// Строки раздела.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub items: Vec<SectionItem>,
    /// Плейсхолдер интерпретации `[Интерпретация Spine]`/`[gap]` (если раздел
    /// интерпретационный и не заполнен механически).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub placeholder: Option<String>,
}

/// Степень находки автопроверки.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckSeverity {
    /// Замечание без блокировки.
    Warn,
    /// Нарушение инварианта — выдача блокируется.
    Error,
}

/// Находка автопроверки модели (fail-closed).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CheckFinding {
    /// Инвариант (и1/и2/и3).
    pub invariant: String,
    /// Степень.
    pub severity: CheckSeverity,
    /// Текст находки.
    pub message: String,
    /// Report-ID, к которому относится находка (если применимо).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub element_id: Option<String>,
}

/// Полная модель отчёта — контракт `arch-be/report/v1`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReportModel {
    /// Имя схемы ([`REPORT_SCHEMA`]).
    pub schema: String,
    /// Версия бинаря, построившего модель.
    pub arch_be: String,
    /// Шапка.
    pub meta: ReportMeta,
    /// Элементы (отсортированы по семейству и ID).
    pub elements: Vec<ReportElement>,
    /// Связи (отсортированы).
    pub relations: Vec<ReportRelation>,
    /// Разделы §1–§14.
    pub sections: Vec<ReportSection>,
    /// Находки автопроверки.
    pub checks: Vec<CheckFinding>,
}

impl ReportModel {
    /// Элемент по report-ID.
    #[must_use]
    pub fn element(&self, id: &str) -> Option<&ReportElement> {
        self.elements.iter().find(|e| e.id == id)
    }

    /// Элементы семейства (в порядке ID).
    #[must_use]
    pub fn elements_of(&self, family: Family) -> Vec<&ReportElement> {
        self.elements
            .iter()
            .filter(|e| e.family == family)
            .collect()
    }

    /// Есть ли блокирующие находки (severity `error`).
    #[must_use]
    pub fn has_errors(&self) -> bool {
        self.error_count() > 0
    }

    /// Число блокирующих находок.
    #[must_use]
    pub fn error_count(&self) -> usize {
        self.checks
            .iter()
            .filter(|c| c.severity == CheckSeverity::Error)
            .count()
    }

    /// Число предупреждений.
    #[must_use]
    pub fn warn_count(&self) -> usize {
        self.checks
            .iter()
            .filter(|c| c.severity == CheckSeverity::Warn)
            .count()
    }

    /// Распределение уверенности по элементам `{"A": n, "B": n, "C": n}`.
    #[must_use]
    pub fn confidence_distribution(&self) -> BTreeMap<String, usize> {
        let mut dist: BTreeMap<String, usize> = BTreeMap::new();
        for c in [ConfidenceAbc::A, ConfidenceAbc::B, ConfidenceAbc::C] {
            dist.insert(c.as_str().to_string(), 0);
        }
        for e in &self.elements {
            *dist.entry(e.confidence.as_str().to_string()).or_default() += 1;
        }
        dist
    }
}

/// Число разделов отчёта (§1–§14).
pub const SECTION_COUNT: usize = 14;

/// Заголовки разделов §1–§14 (единый источник для модели и рендера).
pub const SECTION_TITLES: [&str; SECTION_COUNT] = [
    "Сводка",
    "Легенда",
    "Обзор системы",
    "Контекст (внешние системы)",
    "Контейнеры",
    "Компоненты",
    "Модель данных",
    "Интерфейсы",
    "Сценарии",
    "Сквозные аспекты",
    "Развёртывание",
    "Решения (ADR-карточки)",
    "Качество и риски",
    "Неопределённости",
];

/// Характер каждого раздела §1–§14 (детерминированный/интерпретационный).
pub const SECTION_KINDS: [SectionKind; SECTION_COUNT] = [
    SectionKind::Deterministic,
    SectionKind::Deterministic,
    SectionKind::Deterministic,
    SectionKind::Deterministic,
    SectionKind::Deterministic,
    SectionKind::Deterministic,
    SectionKind::Interpretive,
    SectionKind::Deterministic,
    SectionKind::Interpretive,
    SectionKind::Interpretive,
    SectionKind::Deterministic,
    SectionKind::Deterministic,
    SectionKind::Deterministic,
    SectionKind::Interpretive,
];
