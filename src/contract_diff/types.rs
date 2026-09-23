//! Типы модуля `contract_diff`: находка, формат контракта, отчёт, связка
//! с моделью (ADR-035), примитивы локации находок (JSON Pointer).

/// Находка диффа контракта.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    /// Критичность: `error` (breaking) | `warn` (non-breaking).
    pub severity: String,
    /// Код правила (`CD-001`..`CD-010`, `CD-P01`.., `CD-A01`.., `CD-J01`..,
    /// `CD-S01`..).
    pub rule: String,
    /// JSON Pointer (`#/paths/~1v1~1pets/post`) либо псевдо-поинтер формата
    /// (`#/proto/message/Payment/field/3`).
    pub location: String,
    /// Сообщение.
    pub message: String,
}

/// Формат контракта (`format` инструмента/CLI; `auto` — детектор).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContractFormat {
    /// `OpenAPI` 3.x (yaml/json).
    OpenApi,
    /// protobuf/gRPC (`.proto`).
    Proto,
    /// Avro (`.avsc`, JSON).
    Avro,
    /// JSON Schema (топики/тела сообщений; json/yaml).
    JsonSchema,
    /// DDL-миграции (`.sql`).
    Ddl,
}

impl ContractFormat {
    /// Имя формата (как в аргументе `format`, без `auto`).
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::OpenApi => "openapi",
            Self::Proto => "proto",
            Self::Avro => "avro",
            Self::JsonSchema => "jsonschema",
            Self::Ddl => "ddl",
        }
    }

    /// Формат по имени (`auto` здесь не разбирается — это режим детектора).
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        match name.trim().to_ascii_lowercase().as_str() {
            "openapi" => Some(Self::OpenApi),
            "proto" | "protobuf" | "grpc" => Some(Self::Proto),
            "avro" => Some(Self::Avro),
            "jsonschema" | "json-schema" | "json_schema" => Some(Self::JsonSchema),
            "ddl" | "sql" => Some(Self::Ddl),
            _ => None,
        }
    }
}

/// Отчёт диффа: формат, находки, опциональная связка с моделью.
#[derive(Debug)]
pub struct DiffReport {
    /// Формат, по которому прогнан дифф (после детекции/override).
    pub format: ContractFormat,
    /// Находки.
    pub findings: Vec<Finding>,
    /// Связка с моделью (`--model`): INT, чей `contract` совпал с путями
    /// диффа, и радиус изменения от них. `None` — модель не задана.
    pub impact: Option<ContractImpact>,
}

impl DiffReport {
    /// Есть ли breaking-находки (exit code 1 у CLI).
    #[must_use]
    pub fn has_breaking(&self) -> bool {
        self.findings.iter().any(|f| f.severity == "error")
    }
}

/// Связка диффа с моделью кейса (ADR-035): совпавшие INT и радиус от них.
#[derive(Debug)]
pub struct ContractImpact {
    /// INT, чьё поле `contract` совпало с путём `old`/`new` (пусто — gap:
    /// контракт вне модели).
    pub matched_int: Vec<String>,
    /// Пути диффа, как их видела сверка (относительно корня кейса).
    pub matched_paths: Vec<String>,
    /// Затронутые потребители: CMP/SYS из радиуса (`ID · заголовок`).
    pub consumers: Vec<String>,
    /// Затронутые правила CONSTRAINTS.yaml (`C-001 (name; владелец: …)`).
    pub rules: Vec<String>,
    /// Владельцы для согласования (`OWNER-* · заголовок`).
    pub owners: Vec<String>,
    /// Сводка радиуса (пусто, если совпадений нет).
    pub summary: String,
}

/// JSON Pointer-экранирование сегмента: `~` → `~0`, `/` → `~1`.
pub(crate) fn escape_segment(segment: &str) -> String {
    segment.replace('~', "~0").replace('/', "~1")
}

/// Собирает JSON Pointer с якорем `#` из сегментов.
pub(crate) fn location(segments: &[&str]) -> String {
    let escaped: Vec<String> = segments.iter().map(|s| escape_segment(s)).collect();
    format!("#/{}", escaped.join("/"))
}
