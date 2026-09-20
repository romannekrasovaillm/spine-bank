//! Сравнение двух версий контракта — агентный инструмент `contract_diff`.
//!
//! Третий инструмент контрактного контура (транш T1, ADR-015; расширен
//! бэклогом волны 3, п.14). Форматы:
//! - `OpenAPI` 3.x (CD-001..CD-010): удалённые пути (CD-001), операции
//!   (CD-002), обязательные параметры / ставшие required (CD-003), коды
//!   ответов (CD-004) — breaking (error); добавленные пути/операции/
//!   необязательные параметры/коды ответов (CD-005) — non-breaking (warn);
//!   смена типа поля схемы (CD-006) — breaking; обязательное поле в теле
//!   запроса (CD-008, T-06) — breaking; **CD-007: ломающий дифф без
//!   смены major-компонента `info.version`** (error);
//! - protobuf/gRPC (`.proto`, CD-P01..CD-P06): удалённое message (CD-P01),
//!   удалённое/перенумерованное/переименованное поле (CD-P02; удаление,
//!   покрытое `reserved` в новой версии, — допустимо, warn CD-P05), смена
//!   типа поля (CD-P03), удалённый rpc/service (CD-P04) — breaking;
//!   добавления — warn (CD-P05); **CD-P06: ломающий дифф без смены
//!   major-суффикса пакета (`…​.vN`)** (error; суффикса нет — major не
//!   определим, правило молчит — ограничение);
//! - Avro (`.avsc`, CD-A01..CD-A05): удалённый record (CD-A01), удалённое
//!   поле без `default` (CD-A02), несовместимая смена типа (CD-A03),
//!   добавленное поле без `default` (CD-A04) — breaking; удалённое поле с
//!   `default`, добавленное поле с `default`, расширение типа по таблице
//!   промоушенов Avro (`int→long→float→double`) или расширение union —
//!   warn (CD-A05). Версии в Avro нет — правила major нет (ограничение);
//! - JSON Schema (топики/тела сообщений; `.json`/yaml с `$schema`
//!   json-schema или `properties`/`required`, CD-J01..CD-J05): удалённое
//!   свойство (CD-J01), свойство стало обязательным / добавлено сразу
//!   обязательным (CD-J02), сужение или смена типа (CD-J03) — breaking;
//!   снятие обязательности (CD-J04), добавленное необязательное свойство /
//!   расширение типа (CD-J05) — warn. Рекурсия по вложенным `properties`
//!   с потолком [`MAX_JSONSCHEMA_DEPTH`]; `$ref` не резолвится
//!   (ограничение, как у OpenAPI-скелета);
//! - DDL-миграции (`.sql`, CD-S01..CD-S05): файл приводится к итоговому
//!   состоянию (`CREATE TABLE` + `ALTER TABLE` + `DROP TABLE`), дифф —
//!   по состояниям: удалённая таблица (CD-S01), колонка (CD-S02),
//!   несовместимая смена типа (CD-S03), `NOT NULL` без `DEFAULT` (у
//!   существующей или добавленной колонки) (CD-S04) — breaking; добавленные
//!   таблица/колонка (nullable или с default), расширение типа
//!   (`varchar(N→M>N)`, `int→bigint`, …), снятие `NOT NULL` — warn (CD-S05).
//!   Процедурные блоки и `$$`-тела не поддерживаются (консервативно —
//!   разбор по `;`); версии в DDL нет — правила major нет (ограничение).
//!
//! Формат определяется автоматически (расширение, затем содержимое) либо
//! явно (`format`: `auto`|`openapi`|`proto`|`avro`|`jsonschema`|`ddl`);
//! оба файла обязаны быть одного формата. Парсинг — без новых
//! зависимостей: `serde_json`/`serde_yaml_ng`/`regex` уже в Cargo.toml.
//!
//! Связка с моделью (ADR-035, п.14): при заданном `model` (корень кейса с
//! `model/`) по полю `contract` сущностей INT находятся интеграции, чей
//! контракт совпал с путём `old`/`new`, и через [`crate::review::impact_from_ids`]
//! в ответ включается секция `impact`: затронутые потребители (CMP/SYS),
//! правила и владельцы — «ломающее изменение сразу возвращает потребителей
//! и владельцев». Ни одного совпадения — честная пометка gap.
//!
//! CD-008 (T-06): поле, ставшее обязательным в `requestBody` (новое или
//! переведённое из необязательных), — ломающее изменение: потребитель,
//! который поля не присылает, ломается на валидации. Разбираются `$ref`
//! (в `components.schemas` того же документа), `allOf` и вложенные объекты.
//!
//! CD-009/CD-010 (Д5, 0.3.5): удалённое поле ТЕЛА ОТВЕТА (CD-009) —
//! ломающее, направление обратно запросу: потребитель ответ читает, поэтому
//! появление нового обязательного поля ответа безопасно и находки не даёт.
//! Тело запроса, ставшее обязательным (`requestBody.required: false → true`,
//! CD-010), — ломающее: вызов без тела перестаёт работать. Параметры за
//! `$ref` (`#/components/parameters/…`) резолвятся тем же `resolve_schema`,
//! что и схемы тел, — иначе CD-003 не видит удаление обязательного параметра,
//! объявленного через компонент. Все три правила подчиняются CD-007
//! (ломающий дифф без смены major `info.version`).
//!
//! Известные ограничения скелета `OpenAPI` (Deferred): CD-006 сравнивает
//! только прямое поле `type` у `components.schemas.*.properties.*`;
//! обязательность параметра — по полю `required`; удаление необязательного
//! параметра и добавление обязательного не флагаются. Локализация находок —
//! JSON Pointer (`#/…`) для OpenAPI/JSON Schema, псевдо-поинтеры
//! `#/proto/…`, `#/avro/…`, `#/ddl/…` для остальных форматов.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::error::{HarnessError, Result};
use crate::llm::ToolSpec;
use crate::model::{EntityKind, load_model};
use crate::tool::{Tool, ToolContext, ToolOutput};

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

/// Потолок рекурсии по вложенным `properties` JSON Schema (защита от
/// патологически глубоких схем; `$ref` всё равно не резолвится).
const MAX_JSONSCHEMA_DEPTH: usize = 16;

/// Потолок операторов в одном DDL-файле (защита от гигантских дампов;
/// реальные миграции на порядки меньше).
const MAX_DDL_STATEMENTS: usize = 10_000;

/// Потолок строк в одном `.proto`-файле (та же защита).
const MAX_PROTO_LINES: usize = 200_000;

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

// ---------------------------------------------------------------------------
// Точка входа и детектор формата
// ---------------------------------------------------------------------------

/// Читает файл контракта текстом.
///
/// # Errors
/// Файл не читается.
fn read_text(path: &Path) -> Result<String> {
    std::fs::read_to_string(path).map_err(|e| HarnessError::io(path, e))
}

/// Детектор формата: расширение файла, затем содержимое.
///
/// Расширения: `.proto` → proto, `.avsc` → avro, `.sql` → ddl;
/// `.json`/`.yaml`/`.yml` и прочие — по содержимому: маркер `openapi`
/// (`OpenAPI`), `$schema` c «json-schema»/«draft» или парный набор
/// `properties`/`required`/`type` (JSON Schema), `type: record` + `fields`
/// в JSON (Avro), `syntax = "proto…"` / строки `message ` / `service `
/// (protobuf), первый оператор `CREATE`/`ALTER`/`DROP` (DDL).
///
/// # Errors
/// Формат не распознан (сообщение перечисляет проверенные признаки) либо
/// файл — `AsyncAPI` (дифф `AsyncAPI` не поддержан — только линт).
pub fn detect_format(path: &Path, content: &str) -> Result<ContractFormat> {
    if let Some(format) = match path.extension().and_then(|e| e.to_str()) {
        Some("proto") => Some(ContractFormat::Proto),
        Some("avsc") => Some(ContractFormat::Avro),
        Some("sql") => Some(ContractFormat::Ddl),
        _ => None,
    } {
        return Ok(format);
    }
    let unknown = || {
        HarnessError::Tool(format!(
            "{}: не удалось определить формат контракта: не OpenAPI 3.x (нет поля openapi \
             вида «3.x.y»), не JSON Schema (нет $schema json-schema / properties+required), \
             не Avro record, не proto (нет syntax/message/service), не DDL (нет \
             CREATE/ALTER/DROP) — задайте format явно (openapi|proto|avro|jsonschema|ddl)",
            path.display()
        ))
    };
    let trimmed = content.trim_start();
    // Текстовые признаки proto/DDL — до JSON/YAML-разбора (они не парсеры).
    if trimmed.starts_with("syntax") && trimmed.contains("proto") {
        return Ok(ContractFormat::Proto);
    }
    let upper = trimmed.to_ascii_uppercase();
    if upper.starts_with("CREATE ") || upper.starts_with("ALTER ") || upper.starts_with("DROP ") {
        return Ok(ContractFormat::Ddl);
    }
    // JSON/YAML-семейство: разбираем и смотрим маркерные поля.
    let doc: Value = if trimmed.starts_with('{') {
        match serde_json::from_str(trimmed) {
            Ok(v) => v,
            Err(_) => {
                // Невалидный JSON: последний шанс — голые текстовые маркеры.
                return detect_by_markers(content, &unknown);
            }
        }
    } else {
        match serde_yaml_ng::from_str(trimmed) {
            Ok(v) => v,
            Err(_) => return detect_by_markers(content, &unknown),
        }
    };
    if doc.get("asyncapi").is_some() {
        return Err(HarnessError::Tool(format!(
            "{}: это AsyncAPI — contract_diff его не сравнивает (только линт asyncapi_lint)",
            path.display()
        )));
    }
    if doc.get("openapi").is_some() {
        return Ok(ContractFormat::OpenApi);
    }
    if is_avro_record(&doc) {
        return Ok(ContractFormat::Avro);
    }
    if is_json_schema(&doc) {
        return Ok(ContractFormat::JsonSchema);
    }
    detect_by_markers(content, &unknown)
}

/// Признак Avro-схемы: JSON-объект с `type: "record"` и массивом `fields`.
fn is_avro_record(doc: &Value) -> bool {
    doc.get("type").and_then(Value::as_str) == Some("record") && doc.get("fields").is_some()
}

/// Признак JSON Schema: `$schema` с «json-schema»/«draft», либо типовой
/// набор ключей схемы (`properties`/`required`/`type`/`items`).
fn is_json_schema(doc: &Value) -> bool {
    if let Some(schema) = doc.get("$schema").and_then(Value::as_str) {
        if schema.contains("json-schema") || schema.contains("draft") {
            return true;
        }
    }
    doc.get("properties").is_some()
        || doc.get("required").is_some()
        || doc.get("type").is_some()
        || doc.get("items").is_some()
}

/// Последний шанс детектора — голые текстовые маркеры (битый JSON/YAML,
/// но читаемые маркеры формата).
fn detect_by_markers(content: &str, unknown: &dyn Fn() -> HarnessError) -> Result<ContractFormat> {
    let mut protoish = false;
    for line in content.lines() {
        let t = line.trim_start();
        if t.starts_with("message ") || t.starts_with("service ") || t.starts_with("package ") {
            protoish = true;
            break;
        }
    }
    if protoish {
        return Ok(ContractFormat::Proto);
    }
    if content.contains("\"openapi\"")
        || content
            .lines()
            .any(|l| l.trim_start().starts_with("openapi:"))
    {
        return Ok(ContractFormat::OpenApi);
    }
    Err(unknown())
}

/// Сравнивает два контракта: авто-детект формата, без связки с моделью.
///
/// Совместимость транша T1: пара OpenAPI-документов ведёт себя ровно как
/// раньше (те же CD-001..CD-006; CD-007 добавлен п.14 — см. заголовок
/// модуля).
///
/// # Errors
/// Файл не читается, формат не распознан, форматы файлов разные.
pub fn diff_contracts(old: &Path, new: &Path) -> Result<Vec<Finding>> {
    Ok(diff_report(old, new, None, None)?.findings)
}

/// Полный дифф: явный формат (`None` — авто-детект) + опциональная связка
/// с моделью (`model_case` — корень кейса с `model/`).
///
/// # Errors
/// Файл не читается/не парсится, формат не распознан или различается между
/// файлами, модель задана, но не читается.
pub fn diff_report(
    old: &Path,
    new: &Path,
    format_override: Option<ContractFormat>,
    model_case: Option<&Path>,
) -> Result<DiffReport> {
    let old_text = read_text(old)?;
    let new_text = read_text(new)?;
    let format = if let Some(f) = format_override {
        f
    } else {
        let f_old = detect_format(old, &old_text)?;
        let f_new = detect_format(new, &new_text)?;
        if f_old != f_new {
            return Err(HarnessError::Tool(format!(
                "форматы различаются: {} — {}, {} — {} (сравнивать нужно одноформатное)",
                old.display(),
                f_old.name(),
                new.display(),
                f_new.name()
            )));
        }
        f_old
    };
    let mut findings = match format {
        ContractFormat::OpenApi => diff_openapi(&old_text, &new_text, old, new)?,
        ContractFormat::Proto => diff_proto(&old_text, &new_text),
        ContractFormat::Avro => diff_avro(&old_text, &new_text, old, new)?,
        ContractFormat::JsonSchema => diff_jsonschema(&old_text, &new_text, old, new)?,
        ContractFormat::Ddl => diff_ddl(&old_text, &new_text),
    };
    major_rule(format, &old_text, &new_text, &mut findings);
    let impact = match model_case {
        Some(case) => Some(build_impact(case, old, new)?),
        None => None,
    };
    Ok(DiffReport {
        format,
        findings,
        impact,
    })
}

/// Правило «ломающий дифф без смены major — error», где major определим:
/// `OpenAPI` — major-компонент semver `info.version` (CD-007); proto —
/// суффикс `.vN` пакета (CD-P06). Major не определим (нет поля/суффикса) —
/// правило молчит (задокументированное ограничение; у Avro/JSON Schema/DDL
/// версии нет вовсе).
fn major_rule(format: ContractFormat, old: &str, new: &str, out: &mut Vec<Finding>) {
    let breaking = out.iter().filter(|f| f.severity == "error").count();
    if breaking == 0 {
        return;
    }
    let versions: Option<(String, String)> = match format {
        ContractFormat::OpenApi => {
            let old_v = parse_contract(old, Path::new("<old>"))
                .ok()
                .and_then(|d| d.get("info")?.get("version")?.as_str().map(str::to_string));
            let new_v = parse_contract(new, Path::new("<new>"))
                .ok()
                .and_then(|d| d.get("info")?.get("version")?.as_str().map(str::to_string));
            old_v.zip(new_v)
        }
        ContractFormat::Proto => {
            let old_p = parse_proto(old).package;
            let new_p = parse_proto(new).package;
            old_p.zip(new_p)
        }
        _ => None,
    };
    let Some((old_v, new_v)) = versions else {
        return;
    };
    let majors = match format {
        ContractFormat::OpenApi => (semver_major(&old_v), semver_major(&new_v)),
        ContractFormat::Proto => (proto_package_major(&old_v), proto_package_major(&new_v)),
        _ => (None, None),
    };
    let (Some(old_m), Some(new_m)) = majors else {
        return;
    };
    if old_m == new_m {
        let (rule, location, what) = match format {
            ContractFormat::OpenApi => ("CD-007", "#/info/version", "info.version"),
            ContractFormat::Proto => ("CD-P06", "#/proto/package", "major-суффикс пакета (.vN)"),
            _ => unreachable!("major_rule вызывается только для openapi/proto"),
        };
        out.push(Finding {
            severity: "error".into(),
            rule: rule.into(),
            location: location.into(),
            message: format!(
                "ломающих изменений: {breaking}, а {what} не изменился ({old_v} → {new_v}) — \
                 ломающий дифф требует смены major"
            ),
        });
    }
}

/// Major-компонент semver (`1.2.3` → 1).
fn semver_major(version: &str) -> Option<u64> {
    version.trim().split('.').next()?.parse().ok()
}

/// Major proto-пакета: число суффикса `.vN` (`acme.payments.v2` → 2).
fn proto_package_major(package: &str) -> Option<u64> {
    package.rsplit('.').next()?.strip_prefix('v')?.parse().ok()
}

// ---------------------------------------------------------------------------
// Связка с моделью (ADR-035, п.14)
// ---------------------------------------------------------------------------

/// Нормализация пути для сверки с `INT.contract`: относительно корня кейса,
/// без `./` и обратных слэшей.
fn normalize_contract_path(case: &Path, raw: &Path) -> String {
    let stripped = if raw.is_absolute() {
        raw.strip_prefix(case).unwrap_or(raw).to_path_buf()
    } else {
        raw.to_path_buf()
    };
    let mut s = stripped.to_string_lossy().replace('\\', "/");
    while let Some(rest) = s.strip_prefix("./") {
        s = rest.to_string();
    }
    s
}

/// Строит связку диффа с моделью: INT, чьё поле `contract` совпало с путём
/// `old`/`new` (относительно корня кейса), и радиус изменения от них
/// ([`crate::review::impact_from_ids`]). Нет совпадений — `matched_int`
/// пуст (gap, а не ошибка: контракт может легитимно жить вне модели).
///
/// # Errors
/// Модель задана, но `model/` не читается/не разбирается.
fn build_impact(case: &Path, old: &Path, new: &Path) -> Result<ContractImpact> {
    let model_dir = case.join("model");
    if !model_dir.is_dir() {
        return Err(HarnessError::Model(format!(
            "contract_diff --model: нет каталога модели {}",
            model_dir.display()
        )));
    }
    let model = load_model(&model_dir)?;
    let old_rel = normalize_contract_path(case, old);
    let new_rel = normalize_contract_path(case, new);
    let mut matched_int: Vec<String> = Vec::new();
    let mut matched_paths: BTreeSet<String> = BTreeSet::new();
    for e in &model.entities {
        if e.kind != EntityKind::Int {
            continue;
        }
        let Some(contract) = e.contract.as_deref().map(str::trim) else {
            continue;
        };
        if contract.is_empty() {
            continue;
        }
        let norm = contract.replace('\\', "/");
        let norm = norm.strip_prefix("./").unwrap_or(&norm).to_string();
        if norm == old_rel || norm == new_rel {
            matched_int.push(e.id.clone());
            matched_paths.insert(norm);
        }
    }
    matched_int.sort();
    if matched_int.is_empty() {
        return Ok(ContractImpact {
            matched_int,
            matched_paths: Vec::new(),
            consumers: Vec::new(),
            rules: Vec::new(),
            owners: Vec::new(),
            summary: String::new(),
        });
    }
    let impact = crate::review::impact_from_ids(case, &matched_int)?;
    let matched: BTreeSet<&str> = matched_int.iter().map(String::as_str).collect();
    let consumers: Vec<String> = impact
        .affected
        .iter()
        .filter(|a| (a.kind == "cmp" || a.kind == "sys") && !matched.contains(a.id.as_str()))
        .map(|a| format!("{} · {}", a.id, a.title))
        .collect();
    let rules: Vec<String> = impact
        .rules
        .iter()
        .map(|r| {
            let name = r.name.as_deref().unwrap_or("?");
            match &r.owner {
                Some(owner) => format!("{} ({name}; владелец: {owner})", r.id),
                None => format!("{} ({name})", r.id),
            }
        })
        .collect();
    Ok(ContractImpact {
        matched_int,
        matched_paths: matched_paths.into_iter().collect(),
        consumers,
        rules,
        owners: impact.owners,
        summary: impact.summary,
    })
}

// ---------------------------------------------------------------------------
// OpenAPI 3.x (транш T1 — без изменений семантики CD-001..CD-006)
// ---------------------------------------------------------------------------

/// Методы операций `OpenAPI` (остальные ключи path item — служебные).
const OPERATION_METHODS: [&str; 8] = [
    "get", "put", "post", "delete", "options", "head", "patch", "trace",
];

/// Дифф двух текстов `OpenAPI` 3.x (JSON или YAML).
///
/// # Errors
/// Текст не парсится, документ не `OpenAPI` 3.x.
fn diff_openapi(old_text: &str, new_text: &str, old: &Path, new: &Path) -> Result<Vec<Finding>> {
    let old_doc = read_openapi(old_text, old)?;
    let new_doc = read_openapi(new_text, new)?;
    Ok(diff_documents(&old_doc, &new_doc))
}

/// Читает и распознаёт один контракт `OpenAPI` 3.x.
///
/// # Errors
/// Текст не парсится, документ не `OpenAPI` 3.x.
fn read_openapi(content: &str, path: &Path) -> Result<Value> {
    let doc = parse_contract(content, path)?;
    if !is_openapi3(&doc) {
        return Err(HarnessError::Tool(format!(
            "{}: не OpenAPI 3.x: ожидается поле openapi вида «3.x.y»",
            path.display()
        )));
    }
    Ok(doc)
}

/// Разбирает текст контракта: `{` в начале — JSON, иначе YAML.
///
/// # Errors
/// Содержимое не парсится выбранным форматом.
fn parse_contract(content: &str, path: &Path) -> Result<Value> {
    let trimmed = content.trim_start();
    if trimmed.starts_with('{') {
        serde_json::from_str(trimmed)
            .map_err(|e| HarnessError::Tool(format!("{}: невалидный JSON: {e}", path.display())))
    } else {
        serde_yaml_ng::from_str(trimmed)
            .map_err(|e| HarnessError::Tool(format!("{}: невалидный YAML: {e}", path.display())))
    }
}

/// Признак `OpenAPI` 3.x: поле `openapi` со значением вида `3.x.y`.
fn is_openapi3(doc: &Value) -> bool {
    match doc.get("openapi").and_then(Value::as_str) {
        Some(version) => {
            let parts: Vec<&str> = version.split('.').collect();
            parts.len() >= 2
                && parts[0] == "3"
                && parts[1..]
                    .iter()
                    .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
        }
        None => false,
    }
}

/// Прогоняет все правила по разобранным документам.
fn diff_documents(old: &Value, new: &Value) -> Vec<Finding> {
    let mut findings = Vec::new();
    diff_paths(old, new, &mut findings);
    diff_schemas(old, new, &mut findings);
    findings
}

/// Правила по `paths`: CD-001/CD-002/CD-003/CD-004/CD-005/CD-008.
fn diff_paths(old: &Value, new: &Value, out: &mut Vec<Finding>) {
    let old_paths = old.get("paths").and_then(Value::as_object);
    let new_paths = new.get("paths").and_then(Value::as_object);

    // CD-001 (error): удалённый путь; операции/параметры/ответы — общий путь.
    if let Some(old_paths) = old_paths {
        for (path, old_item) in old_paths {
            let path = path.as_str();
            match new_paths.and_then(|np| np.get(path)) {
                None => out.push(Finding {
                    severity: "error".into(),
                    rule: "CD-001".into(),
                    location: location(&["paths", path]),
                    message: format!("удалён путь «{path}»"),
                }),
                Some(new_item) => diff_path_item(old, new, path, old_item, new_item, out),
            }
        }
    }

    // CD-005 (warn): добавленный путь.
    if let Some(new_paths) = new_paths {
        for (path, _) in new_paths {
            let path = path.as_str();
            if !old_paths.is_some_and(|op| op.contains_key(path)) {
                out.push(Finding {
                    severity: "warn".into(),
                    rule: "CD-005".into(),
                    location: location(&["paths", path]),
                    message: format!("добавлен путь «{path}»"),
                });
            }
        }
    }
}

/// Правила одного path item: CD-002 (операции) и CD-003/CD-004/CD-008
/// (общие операции). Документы нужны для резолва `$ref` (CD-008).
fn diff_path_item(
    old_doc: &Value,
    new_doc: &Value,
    path: &str,
    old: &Value,
    new: &Value,
    out: &mut Vec<Finding>,
) {
    for method in OPERATION_METHODS {
        let old_op = old.get(method).filter(|v| v.is_object());
        let new_op = new.get(method).filter(|v| v.is_object());
        match (old_op, new_op) {
            // CD-002 (error): удалённая операция.
            (Some(_), None) => out.push(Finding {
                severity: "error".into(),
                rule: "CD-002".into(),
                location: location(&["paths", path, method]),
                message: format!("удалена операция {method}"),
            }),
            // CD-005 (warn): добавленная операция.
            (None, Some(_)) => out.push(Finding {
                severity: "warn".into(),
                rule: "CD-005".into(),
                location: location(&["paths", path, method]),
                message: format!("добавлена операция {method}"),
            }),
            (Some(old_op), Some(new_op)) => {
                diff_operation(
                    old_doc, new_doc, path, method, old, new, old_op, new_op, out,
                );
            }
            (None, None) => {}
        }
    }
}

/// Правила общей операции: CD-003 (параметры), CD-004 (ответы) и CD-008
/// (обязательные поля тела запроса). Документы — для резолва `$ref` (CD-008),
/// поэтому аргументов девять; разбор на структуру здесь был бы шумом.
#[allow(clippy::too_many_arguments)]
fn diff_operation(
    old_doc: &Value,
    new_doc: &Value,
    path: &str,
    method: &str,
    old_item: &Value,
    new_item: &Value,
    old_op: &Value,
    new_op: &Value,
    out: &mut Vec<Finding>,
) {
    diff_parameters(
        old_doc, new_doc, path, method, old_item, new_item, old_op, new_op, out,
    );
    diff_responses(old_doc, new_doc, path, method, old_op, new_op, out);
    diff_request_body(old_doc, new_doc, path, method, old_op, new_op, out);
}

/// CD-008 (T-06): поле, ставшее обязательным в теле запроса, — ломающее
/// изменение. Потребитель, который его не присылает, ломается на валидации
/// (в отличие от нового необязательного поля, которое безопасно).
///
/// Собираются «пути обязательности» схемы тела запроса: имя поля, которое
/// `required` у своего объекта, и дальше рекурсивно — обязательные поля
/// вложенных объектов. Схема раскрывается по `$ref` и `allOf`, поэтому
/// изменение в `components.schemas.*`, на которую ссылается тело запроса,
/// видно так же, как правка inline-схемы.
fn diff_request_body(
    old_doc: &Value,
    new_doc: &Value,
    path: &str,
    method: &str,
    old_op: &Value,
    new_op: &Value,
    out: &mut Vec<Finding>,
) {
    let op_location = location(&["paths", path, method]);
    // CD-010: тело запроса стало обязательным. Раньше потребитель мог звать
    // операцию без тела, теперь обязан его прислать — вызов без тела ломается.
    // Сравниваются сами флаги `requestBody.required`, а не схемы: проверка не
    // зависит от того, читается ли содержимое тела.
    let body_required = |op: &Value| {
        op.get("requestBody")
            .and_then(|b| b.get("required"))
            .and_then(Value::as_bool)
            .unwrap_or(false)
    };
    if !body_required(old_op) && body_required(new_op) {
        out.push(Finding {
            severity: "error".into(),
            rule: "CD-010".into(),
            location: format!("{op_location}/requestBody/required"),
            message: format!(
                "тело запроса {method} {path} стало обязательным (requestBody.required: \
                 false → true) — вызов без тела ломается"
            ),
        });
    }
    let (Some(old_schema), Some(new_schema)) =
        (request_body_schema(old_op), request_body_schema(new_op))
    else {
        return;
    };
    let old_required = required_field_paths(old_doc, old_schema);
    let new_required = required_field_paths(new_doc, new_schema);
    for field in new_required.difference(&old_required) {
        out.push(Finding {
            severity: "error".into(),
            rule: "CD-008".into(),
            location: format!(
                "{op_location}/requestBody/content/schema/{}",
                field.replace('.', "/properties/")
            ),
            message: format!(
                "поле «{field}» стало обязательным в теле запроса {method} {path} — \
                 потребитель, который его не присылает, ломается на валидации"
            ),
        });
    }
}

/// Схема тела запроса операции (первый `content.*.schema`).
fn request_body_schema(op: &Value) -> Option<&Value> {
    op.get("requestBody")?
        .get("content")?
        .as_object()?
        .values()
        .find_map(|media| media.get("schema"))
}

/// Какие пути схемы собирать: только обязательные (тело ЗАПРОСА: потребитель
/// ломается, когда обязан прислать больше) или все подряд (тело ОТВЕТА:
/// потребитель ломается, когда перестаёт получать то, на что опирался).
#[derive(Clone, Copy, PartialEq, Eq)]
enum PathsMode {
    /// Только поля из `required` (+ рекурсия по ним).
    Required,
    /// Все `properties`, включая необязательные.
    All,
}

/// Пути обязательных полей схемы: `a`, `a.b`, … — от корня тела запроса.
///
/// Рекурсия идёт только по обязательным ветвям: необязательный объект со
/// своим обязательным полем ломает не всех потребителей, и выдавать его за
/// безусловное «стало обязательным» значило бы поднимать тревогу на
/// безопасной правке. Незнакомый `$ref` или глубина больше
/// [`SCHEMA_MAX_DEPTH`] — ветка не раскрывается (лучше пропустить, чем
/// утверждать несуществующее).
fn required_field_paths(doc: &Value, schema: &Value) -> BTreeSet<String> {
    schema_paths(doc, schema, PathsMode::Required)
}

/// `path` — потомок `ancestor` по сегментам пути (`a.b` — потомок `a`,
/// `ab` — нет). Сегментное сравнение, а не префикс строки: поля `amount` и
/// `amount_total` — разные поля.
fn is_descendant(path: &str, ancestor: &str) -> bool {
    path.len() > ancestor.len()
        && path.starts_with(ancestor)
        && path.as_bytes()[ancestor.len()] == b'.'
}

/// Все пути полей схемы (включая необязательные) — для тел ОТВЕТОВ (CD-009).
/// Режим `All` не пропускает необязательные ветви: удаление необязательного
/// поля ответа тоже ломает потребителя, который его читал.
fn all_field_paths(doc: &Value, schema: &Value) -> BTreeSet<String> {
    schema_paths(doc, schema, PathsMode::All)
}

/// Сбор путей схемы в выбранном режиме — общая рекурсия для тел запросов
/// (CD-008) и ответов (CD-009): один обход, два вопроса к нему.
fn schema_paths(doc: &Value, schema: &Value, mode: PathsMode) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    collect_paths(doc, schema, "", &mut out, 0, mode);
    out
}

/// Рекурсивный сбор путей полей (внутренняя часть [`schema_paths`]).
fn collect_paths(
    doc: &Value,
    schema: &Value,
    prefix: &str,
    out: &mut BTreeSet<String>,
    depth: usize,
    mode: PathsMode,
) {
    if depth > SCHEMA_MAX_DEPTH {
        return;
    }
    let schema = resolve_schema(doc, schema, 0);
    let Some(obj) = schema.as_object() else {
        return;
    };
    let required: Vec<&str> = obj
        .get("required")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let props = obj.get("properties").and_then(Value::as_object);
    let names: Vec<&str> = match mode {
        PathsMode::Required => required,
        PathsMode::All => props
            .map(|p| p.keys().map(String::as_str).collect())
            .unwrap_or_default(),
    };
    for name in names {
        let path = if prefix.is_empty() {
            name.to_string()
        } else {
            format!("{prefix}.{name}")
        };
        out.insert(path.clone());
        if let Some(prop) = props.and_then(|p| p.get(name)) {
            collect_paths(doc, prop, &path, out, depth + 1, mode);
        }
    }
}

/// Схема с раскрытыми `$ref` (JSON Pointer внутрь того же документа) и
/// слитыми ветвями `allOf` (`properties` и `required` объединяются).
fn resolve_schema(doc: &Value, schema: &Value, depth: usize) -> Value {
    if depth > SCHEMA_MAX_DEPTH {
        return schema.clone();
    }
    if let Some(reference) = schema.get("$ref").and_then(Value::as_str) {
        if let Some(target) = resolve_pointer(doc, reference) {
            return resolve_schema(doc, target, depth + 1);
        }
        return schema.clone();
    }
    let Some(branches) = schema.get("allOf").and_then(Value::as_array) else {
        return schema.clone();
    };
    let mut merged = schema.clone();
    let mut props = serde_json::Map::new();
    let mut required: Vec<Value> = Vec::new();
    for branch in branches {
        let branch = resolve_schema(doc, branch, depth + 1);
        if let Some(p) = branch.get("properties").and_then(Value::as_object) {
            for (k, v) in p {
                props.insert(k.clone(), v.clone());
            }
        }
        if let Some(r) = branch.get("required").and_then(Value::as_array) {
            required.extend(r.iter().cloned());
        }
    }
    if !props.is_empty() {
        merged["properties"] = Value::Object(props);
    }
    if !required.is_empty() {
        merged["required"] = Value::Array(required);
    }
    merged
}

/// Значение по JSON Pointer (`#/components/schemas/Charge`).
fn resolve_pointer<'a>(doc: &'a Value, pointer: &str) -> Option<&'a Value> {
    let path = pointer.strip_prefix('#')?;
    let mut current = doc;
    for raw in path.split('/').filter(|s| !s.is_empty()) {
        let segment = raw.replace("~1", "/").replace("~0", "~");
        current = match current {
            Value::Object(map) => map.get(&segment)?,
            Value::Array(items) => items.get(segment.parse::<usize>().ok()?)?,
            _ => return None,
        };
    }
    Some(current)
}

/// Предел рекурсии раскрытия схемы тела запроса (CD-008).
const SCHEMA_MAX_DEPTH: usize = 12;

/// Эффективный набор параметров операции: `path_item.parameters` +
/// `operation.parameters` (операция перекрывает path item по ключу `name`+`in`).
///
/// Параметр за `$ref` резолвится тем же [`resolve_schema`], что схемы тел
/// (Д5): компоненты параметров — обычная практика (`#/components/parameters/…`),
/// и пропускать их значило не видеть ни удаления обязательного параметра
/// (CD-003), ни перевода в `required`. Нерезолвящаяся или внешняя ссылка
/// (не JSON Pointer внутрь документа) по-прежнему пропускается: выдумывать
/// параметр по имени файла нельзя.
fn effective_parameters(
    doc: &Value,
    path_item: &Value,
    op: &Value,
) -> std::collections::HashMap<(String, String), Value> {
    let mut map = std::collections::HashMap::new();
    for level in [path_item.get("parameters"), op.get("parameters")]
        .into_iter()
        .flatten()
    {
        let Some(params) = level.as_array() else {
            continue;
        };
        for param in params {
            let resolved = resolve_schema(doc, param, 0);
            if resolved.get("$ref").is_some() || !resolved.is_object() {
                continue;
            }
            let (Some(name), Some(in_)) = (
                resolved.get("name").and_then(Value::as_str),
                resolved.get("in").and_then(Value::as_str),
            ) else {
                continue;
            };
            map.insert((name.to_string(), in_.to_string()), resolved);
        }
    }
    map
}

/// Обязательность параметра — по полю `required`.
fn is_required(param: &Value) -> bool {
    param
        .get("required")
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

/// CD-003/CD-005: удаление обязательного параметра, добавление необязательного,
/// переход необязательного в required. Документы — для резолва параметров за
/// `$ref` (Д5), поэтому аргументов девять, как у [`diff_operation`].
#[allow(clippy::too_many_arguments)]
fn diff_parameters(
    old_doc: &Value,
    new_doc: &Value,
    path: &str,
    method: &str,
    old_item: &Value,
    new_item: &Value,
    old_op: &Value,
    new_op: &Value,
    out: &mut Vec<Finding>,
) {
    let op_location = location(&["paths", path, method]);
    let old_params = effective_parameters(old_doc, old_item, old_op);
    let new_params = effective_parameters(new_doc, new_item, new_op);

    // CD-003 (error): удалён обязательный параметр.
    for ((name, in_), param) in &old_params {
        if !new_params.contains_key(&(name.clone(), in_.clone())) && is_required(param) {
            out.push(Finding {
                severity: "error".into(),
                rule: "CD-003".into(),
                location: format!("{op_location}/parameters/{}", escape_segment(name)),
                message: format!("удалён обязательный параметр «{name}» (in: {in_})"),
            });
        }
    }

    // CD-005 (warn): добавлен необязательный параметр (обязательный — вне скелета).
    for ((name, in_), param) in &new_params {
        if !old_params.contains_key(&(name.clone(), in_.clone())) && !is_required(param) {
            out.push(Finding {
                severity: "warn".into(),
                rule: "CD-005".into(),
                location: format!("{op_location}/parameters/{}", escape_segment(name)),
                message: format!("добавлен необязательный параметр «{name}» (in: {in_})"),
            });
        }
    }

    // CD-003 (error): параметр стал required (был необязательным).
    for ((name, in_), old_param) in &old_params {
        let became_required = new_params
            .get(&(name.clone(), in_.clone()))
            .is_some_and(|new_param| !is_required(old_param) && is_required(new_param));
        if became_required {
            out.push(Finding {
                severity: "error".into(),
                rule: "CD-003".into(),
                location: format!("{op_location}/parameters/{}", escape_segment(name)),
                message: format!("параметр «{name}» стал required (in: {in_})"),
            });
        }
    }
}

/// CD-004/CD-005/CD-009: удалённый/добавленный код ответа и удалённое поле
/// тела ответа.
fn diff_responses(
    old_doc: &Value,
    new_doc: &Value,
    path: &str,
    method: &str,
    old_op: &Value,
    new_op: &Value,
    out: &mut Vec<Finding>,
) {
    let op_location = location(&["paths", path, method]);
    let old_responses = old_op.get("responses").and_then(Value::as_object);
    let new_responses = new_op.get("responses").and_then(Value::as_object);

    // CD-004 (error): удалённый код ответа — потребитель мог на него опираться.
    if let Some(old_responses) = old_responses {
        for (code, _) in old_responses {
            if !new_responses.is_some_and(|nr| nr.contains_key(code)) {
                out.push(Finding {
                    severity: "error".into(),
                    rule: "CD-004".into(),
                    location: format!("{op_location}/responses/{}", escape_segment(code)),
                    message: format!("удалён код ответа {code}"),
                });
            }
        }
    }

    // CD-005 (warn): добавленный код ответа.
    if let Some(new_responses) = new_responses {
        for (code, _) in new_responses {
            if !old_responses.is_some_and(|or| or.contains_key(code)) {
                out.push(Finding {
                    severity: "warn".into(),
                    rule: "CD-005".into(),
                    location: format!("{op_location}/responses/{}", escape_segment(code)),
                    message: format!("добавлен код ответа {code}"),
                });
            }
        }
    }

    // CD-009 (error): удалённое поле тела ответа.
    if let (Some(old_responses), Some(new_responses)) = (old_responses, new_responses) {
        for (code, old_response) in old_responses {
            let Some(new_response) = new_responses.get(code) else {
                continue;
            };
            diff_response_body(
                old_doc,
                new_doc,
                &format!("{op_location}/responses/{}", escape_segment(code)),
                old_response,
                new_response,
                out,
            );
        }
    }
}

/// CD-009 (Д5): поле, исчезнувшее из тела ответа, — ломающее изменение.
/// Направление здесь ОБРАТНОЕ телу запроса: потребитель читает ответ, поэтому
/// удаление поля ломает его, а появление нового обязательного поля — нет
/// (потребитель его просто не читал; лишнее поле в ответе безопасно).
///
/// Схема ответа резолвится по `$ref` и `allOf` тем же [`resolve_schema`], что
/// тело запроса, — сравнение идёт по раскрытым схемам. Коды ответов, тела у
/// которых нет (204, редиректы), пропускаются: сравнивать нечего.
fn diff_response_body(
    old_doc: &Value,
    new_doc: &Value,
    response_location: &str,
    old_response: &Value,
    new_response: &Value,
    out: &mut Vec<Finding>,
) {
    let Some(old_content) = old_response.get("content").and_then(Value::as_object) else {
        return;
    };
    let Some(new_content) = new_response.get("content").and_then(Value::as_object) else {
        return;
    };
    for (media, old_media) in old_content {
        let Some(old_schema) = old_media.get("schema") else {
            continue;
        };
        let Some(new_schema) = new_content.get(media).and_then(|m| m.get("schema")) else {
            continue;
        };
        let old_fields = all_field_paths(old_doc, old_schema);
        let new_fields = all_field_paths(new_doc, new_schema);
        let media_location = format!("{response_location}/content/{}", escape_segment(media));
        let removed: Vec<&String> = old_fields.difference(&new_fields).collect();
        for field in &removed {
            // Удаление поля уносит и всё его поддерево: `a.b` и `a.c` не
            // сообщения, а следствие «`a` больше нет». Рекурсивные схемы
            // (дерево, ветка комментариев) иначе дают десяток находок об одном
            // удалении, и настоящий сигнал тонет в них. Называется минимальный
            // путь — тот, у которого нет удалённого предка.
            if removed.iter().any(|other| is_descendant(field, other)) {
                continue;
            }
            out.push(Finding {
                severity: "error".into(),
                rule: "CD-009".into(),
                location: format!(
                    "{media_location}/schema/{}",
                    field.replace('.', "/properties/")
                ),
                message: format!(
                    "поле «{field}» удалено из тела ответа ({media}) — потребитель, который \
                     его читает, ломается"
                ),
            });
        }
    }
}

/// CD-006: изменение типа поля схемы (поверхностно, без `$ref`-резолюции).
fn diff_schemas(old: &Value, new: &Value, out: &mut Vec<Finding>) {
    let old_schemas = old
        .get("components")
        .and_then(|c| c.get("schemas"))
        .and_then(Value::as_object);
    let new_schemas = new
        .get("components")
        .and_then(|c| c.get("schemas"))
        .and_then(Value::as_object);
    let (Some(old_schemas), Some(new_schemas)) = (old_schemas, new_schemas) else {
        return;
    };
    for (name, old_schema) in old_schemas {
        let Some(new_schema) = new_schemas.get(name) else {
            continue;
        };
        diff_schema_properties(name, old_schema, new_schema, out);
    }
}

/// Сравнивает типы верхнеуровневых `properties` одной схемы.
fn diff_schema_properties(
    name: &str,
    old_schema: &Value,
    new_schema: &Value,
    out: &mut Vec<Finding>,
) {
    let old_props = old_schema.get("properties").and_then(Value::as_object);
    let new_props = new_schema.get("properties").and_then(Value::as_object);
    let (Some(old_props), Some(new_props)) = (old_props, new_props) else {
        return;
    };
    for (prop, old_prop) in old_props {
        let prop = prop.as_str();
        let Some(new_prop) = new_props.get(prop) else {
            continue;
        };
        // Поверхностно: только прямое поле type, без $ref-резолюции и рекурсии.
        let old_type = old_prop.get("type").and_then(Value::as_str);
        let new_type = new_prop.get("type").and_then(Value::as_str);
        match (old_type, new_type) {
            (Some(old_type), Some(new_type)) if old_type != new_type => {
                out.push(Finding {
                    severity: "error".into(),
                    rule: "CD-006".into(),
                    location: location(&["components", "schemas", name, "properties", prop]),
                    message: format!(
                        "изменился тип поля «{prop}» схемы «{name}»: {old_type} → {new_type}"
                    ),
                });
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// protobuf/gRPC (.proto)
// ---------------------------------------------------------------------------

/// Поле сообщения proto (идентичность — тег).
#[derive(Debug, Clone)]
struct ProtoField {
    /// Имя поля.
    name: String,
    /// Тип как написан (`string`, `map<string, int64>`, `acme.Money`).
    typ: String,
}

/// Сообщение proto: поля по тегу + резервирование (удаление, покрытое
/// `reserved`, — допустимая эволюция, warn вместо error).
#[derive(Debug, Default)]
struct ProtoMessage {
    /// Поля по тегу.
    fields: BTreeMap<u64, ProtoField>,
    /// Зарезервированные теги (`reserved 2, 5 to 8;`).
    reserved_tags: BTreeSet<u64>,
    /// Зарезервированные имена (`reserved "foo", "bar";`).
    reserved_names: BTreeSet<String>,
}

/// Разобранный `.proto`-файл (консервативный построчный разбор: сообщения
/// с вложенностью `Outer.Inner`, сервисы с rpc, package; enum'ы и их
/// значения не сравниваются — ограничение скелета).
#[derive(Debug, Default)]
struct ProtoDoc {
    /// `package a.b.v1;`.
    package: Option<String>,
    /// Сообщения по полному имени (`Outer.Inner`).
    messages: BTreeMap<String, ProtoMessage>,
    /// Сервисы: имя → набор rpc.
    services: BTreeMap<String, BTreeSet<String>>,
}

/// Разбирает `.proto` построчно. Невалидные/незнакомые строки пропускаются
/// (консервативный скелет: лучше пропустить, чем упасть).
fn parse_proto(text: &str) -> ProtoDoc {
    let field_re = regex::Regex::new(
        r"^\s*(?:optional\s+|required\s+|repeated\s+)?([A-Za-z_][\w.]*|map\s*<[^>]+>)\s+([A-Za-z_]\w*)\s*=\s*(\d+)",
    );
    // regex известной формы компилируется всегда; при сбое — пустой never-match.
    let Ok(field_re) = field_re else {
        return ProtoDoc::default();
    };
    let message_re = regex::Regex::new(r"^\s*message\s+([A-Za-z_]\w*)\s*\{?");
    let service_re = regex::Regex::new(r"^\s*service\s+([A-Za-z_]\w*)\s*\{?");
    let rpc_re = regex::Regex::new(r"^\s*rpc\s+([A-Za-z_]\w*)\s*\(");
    let package_re = regex::Regex::new(r"^\s*package\s+([\w.]+)\s*;");
    let enum_re = regex::Regex::new(r"^\s*enum\s+[A-Za-z_]\w*\s*\{?");
    let reserved_re = regex::Regex::new(r"^\s*reserved\s+(.+?)\s*;");
    let (Ok(message_re), Ok(service_re), Ok(rpc_re), Ok(package_re), Ok(enum_re), Ok(reserved_re)) = (
        message_re,
        service_re,
        rpc_re,
        package_re,
        enum_re,
        reserved_re,
    ) else {
        return ProtoDoc::default();
    };

    let mut doc = ProtoDoc::default();
    // Элемент стека блоков: (вид, имя сообщения для полей/reserved).
    // Вид: 0 — message, 1 — service, 2 — прочий (enum/oneof/…).
    let mut stack: Vec<(u8, Option<String>)> = Vec::new();
    let mut message_path: Vec<String> = Vec::new();
    let mut current_service: Option<String> = None;
    for line in text.lines().take(MAX_PROTO_LINES) {
        // Срезаем //-комментарии (внутри строк — редкость; консервативно).
        let line = line.split("//").next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        if let Some(caps) = package_re.captures(line) {
            doc.package = Some(caps[1].to_string());
            continue;
        }
        if let Some(caps) = message_re.captures(line) {
            message_path.push(caps[1].to_string());
            let full = message_path.join(".");
            doc.messages.entry(full.clone()).or_default();
            stack.push((0, Some(full)));
            continue;
        }
        if enum_re.is_match(line) {
            stack.push((2, None));
            continue;
        }
        if let Some(caps) = service_re.captures(line) {
            let name = caps[1].to_string();
            doc.services.entry(name.clone()).or_default();
            current_service = Some(name);
            stack.push((1, None));
            continue;
        }
        if line.starts_with('}') {
            if let Some((kind, _)) = stack.pop() {
                match kind {
                    // oneof/прочие блоки внутри message не снимают сообщение
                    // со стека путей — их поля относятся к сообщению.
                    0 => {
                        message_path.pop();
                    }
                    1 => current_service = None,
                    _ => {}
                }
            }
            continue;
        }
        // Поля и reserved относятся к ближайшему охватывающему message.
        if let Some(caps) = reserved_re.captures(line) {
            if let Some(msg) = nearest_message(&mut doc, &stack) {
                for part in caps[1].split(',') {
                    let part = part.trim();
                    if let Some(name) = part.strip_prefix('"').and_then(|p| p.strip_suffix('"')) {
                        msg.reserved_names.insert(name.to_string());
                    } else if let Some((from, to)) = part.split_once(" to ") {
                        // Диапазон «N to M» (max не разворачиваем — метка).
                        if let (Ok(a), Ok(b)) =
                            (from.trim().parse::<u64>(), to.trim().parse::<u64>())
                        {
                            for tag in a..=b.min(a + 10_000) {
                                msg.reserved_tags.insert(tag);
                            }
                        }
                    } else if let Ok(tag) = part.parse::<u64>() {
                        msg.reserved_tags.insert(tag);
                    }
                }
            }
            continue;
        }
        if let (Some(caps), Some(msg)) =
            (field_re.captures(line), nearest_message(&mut doc, &stack))
        {
            let tag: u64 = match caps[3].parse() {
                Ok(t) => t,
                Err(_) => continue,
            };
            msg.fields.insert(
                tag,
                ProtoField {
                    name: caps[2].to_string(),
                    typ: caps[1].split_whitespace().collect::<Vec<_>>().join(""),
                },
            );
            continue;
        }
        if let Some(caps) = rpc_re.captures(line) {
            if let Some(service) = &current_service {
                if let Some(rpcs) = doc.services.get_mut(service) {
                    rpcs.insert(caps[1].to_string());
                }
            }
            continue;
        }
        // oneof/любой другой блок с `{` — на стек как «прочий».
        if line.ends_with('{') {
            stack.push((2, None));
        }
    }
    doc
}

/// Ближайшее охватывающее сообщение на стеке (поля oneof относятся к нему).
fn nearest_message<'d>(
    doc: &'d mut ProtoDoc,
    stack: &[(u8, Option<String>)],
) -> Option<&'d mut ProtoMessage> {
    let full = stack.iter().rev().find(|(kind, _)| *kind == 0)?.1.clone()?;
    doc.messages.get_mut(&full)
}

/// Дифф двух `.proto`: CD-P01..CD-P05 (CD-P06 — major-правилом выше).
fn diff_proto(old: &str, new: &str) -> Vec<Finding> {
    let old_doc = parse_proto(old);
    let new_doc = parse_proto(new);
    let mut out = Vec::new();

    // CD-P01 (error): удалённое сообщение; CD-P05 (warn): добавленное.
    for name in old_doc.messages.keys() {
        if !new_doc.messages.contains_key(name) {
            out.push(Finding {
                severity: "error".into(),
                rule: "CD-P01".into(),
                location: format!("#/proto/message/{}", escape_segment(name)),
                message: format!("удалено сообщение «{name}»"),
            });
        }
    }
    for name in new_doc.messages.keys() {
        if !old_doc.messages.contains_key(name) {
            out.push(Finding {
                severity: "warn".into(),
                rule: "CD-P05".into(),
                location: format!("#/proto/message/{}", escape_segment(name)),
                message: format!("добавлено сообщение «{name}»"),
            });
        }
    }

    // Поля общих сообщений: идентичность — тег.
    for (name, old_msg) in &old_doc.messages {
        let Some(new_msg) = new_doc.messages.get(name) else {
            continue;
        };
        let loc = format!("#/proto/message/{}", escape_segment(name));
        for (tag, old_field) in &old_msg.fields {
            match new_msg.fields.get(tag) {
                None => {
                    let covered = new_msg.reserved_tags.contains(tag)
                        || new_msg.reserved_names.contains(&old_field.name);
                    if covered {
                        // Удаление с reserved — допустимая эволюция (как buf).
                        out.push(Finding {
                            severity: "warn".into(),
                            rule: "CD-P05".into(),
                            location: format!("{loc}/field/{tag}"),
                            message: format!(
                                "поле «{}» (тег {tag}) удалено и зарезервировано — допустимо",
                                old_field.name
                            ),
                        });
                    } else {
                        out.push(Finding {
                            severity: "error".into(),
                            rule: "CD-P02".into(),
                            location: format!("{loc}/field/{tag}"),
                            message: format!(
                                "удалено поле «{}» (тег {tag}) без reserved",
                                old_field.name
                            ),
                        });
                    }
                }
                Some(new_field) => {
                    if new_field.name != old_field.name {
                        out.push(Finding {
                            severity: "error".into(),
                            rule: "CD-P02".into(),
                            location: format!("{loc}/field/{tag}"),
                            message: format!(
                                "тег {tag}: поле переименовано «{}» → «{}» (имя — часть JSON/текстового контракта)",
                                old_field.name, new_field.name
                            ),
                        });
                    }
                    if new_field.typ != old_field.typ {
                        out.push(Finding {
                            severity: "error".into(),
                            rule: "CD-P03".into(),
                            location: format!("{loc}/field/{tag}"),
                            message: format!(
                                "тег {tag} («{}»): тип {} → {}",
                                old_field.name, old_field.typ, new_field.typ
                            ),
                        });
                    }
                }
            }
        }
        // Перенумерация: имя осталось, тег изменился.
        for (new_tag, new_field) in &new_msg.fields {
            if let Some(old_tag) = old_msg
                .fields
                .iter()
                .find(|(t, f)| *f.name == new_field.name && **t != *new_tag)
                .map(|(t, _)| *t)
            {
                out.push(Finding {
                    severity: "error".into(),
                    rule: "CD-P02".into(),
                    location: format!("{loc}/field/{new_tag}"),
                    message: format!(
                        "поле «{}» перенумеровано: тег {old_tag} → {new_tag}",
                        new_field.name
                    ),
                });
            }
        }
        for (tag, new_field) in &new_msg.fields {
            if !old_msg.fields.contains_key(tag)
                && !old_msg.fields.values().any(|f| f.name == new_field.name)
            {
                out.push(Finding {
                    severity: "warn".into(),
                    rule: "CD-P05".into(),
                    location: format!("{loc}/field/{tag}"),
                    message: format!("добавлено поле «{}» (тег {tag})", new_field.name),
                });
            }
        }
    }

    // CD-P04 (error): удалённый сервис/rpc; CD-P05 (warn): добавленные.
    for name in old_doc.services.keys() {
        match new_doc.services.get(name) {
            None => out.push(Finding {
                severity: "error".into(),
                rule: "CD-P04".into(),
                location: format!("#/proto/service/{}", escape_segment(name)),
                message: format!("удалён сервис «{name}»"),
            }),
            Some(new_rpcs) => {
                let old_rpcs = &old_doc.services[name];
                for rpc in old_rpcs {
                    if !new_rpcs.contains(rpc) {
                        out.push(Finding {
                            severity: "error".into(),
                            rule: "CD-P04".into(),
                            location: format!(
                                "#/proto/service/{}/rpc/{}",
                                escape_segment(name),
                                escape_segment(rpc)
                            ),
                            message: format!("удалён rpc «{name}.{rpc}»"),
                        });
                    }
                }
                for rpc in new_rpcs {
                    if !old_rpcs.contains(rpc) {
                        out.push(Finding {
                            severity: "warn".into(),
                            rule: "CD-P05".into(),
                            location: format!(
                                "#/proto/service/{}/rpc/{}",
                                escape_segment(name),
                                escape_segment(rpc)
                            ),
                            message: format!("добавлен rpc «{name}.{rpc}»"),
                        });
                    }
                }
            }
        }
    }
    for name in new_doc.services.keys() {
        if !old_doc.services.contains_key(name) {
            out.push(Finding {
                severity: "warn".into(),
                rule: "CD-P05".into(),
                location: format!("#/proto/service/{}", escape_segment(name)),
                message: format!("добавлен сервис «{name}»"),
            });
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Avro (.avsc)
// ---------------------------------------------------------------------------

/// Поле Avro-записи.
#[derive(Debug, Clone)]
struct AvroField {
    /// Нормализованный тип (union — отсортированный список через `|`).
    typ: String,
    /// Есть ли `default`.
    has_default: bool,
}

/// Avro-запись (record) по полному имени.
#[derive(Debug, Default)]
struct AvroRecord {
    /// Поля по имени.
    fields: BTreeMap<String, AvroField>,
}

/// Нормализует тип Avro в стабильную строку: строка — как есть; union
/// (массив) — отсортированные варианты через `|`; объект — компактный JSON.
fn avro_type_norm(typ: &Value) -> String {
    match typ {
        Value::String(s) => s.clone(),
        Value::Array(variants) => {
            let mut v: Vec<String> = variants.iter().map(avro_type_norm).collect();
            v.sort();
            v.join("|")
        }
        other => serde_json::to_string(other).unwrap_or_else(|_| "<bad-type>".to_string()),
    }
}

/// Собирает все record'ы документа Avro (с вложенными): полное имя
/// (`namespace.name`, либо локальное) → поля.
fn collect_avro_records(
    node: &Value,
    namespace: Option<&str>,
    out: &mut BTreeMap<String, AvroRecord>,
) {
    let Some(obj) = node.as_object() else {
        return;
    };
    let ns = obj
        .get("namespace")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| namespace.map(str::to_string));
    if obj.get("type").and_then(Value::as_str) == Some("record") {
        if let (Some(name), Some(fields)) = (
            obj.get("name").and_then(Value::as_str),
            obj.get("fields").and_then(Value::as_array),
        ) {
            let full = match &ns {
                Some(ns) => format!("{ns}.{name}"),
                None => name.to_string(),
            };
            let mut record = AvroRecord::default();
            for field in fields {
                let Some(fname) = field.get("name").and_then(Value::as_str) else {
                    continue;
                };
                let typ = field
                    .get("type")
                    .map_or_else(|| "<нет type>".to_string(), avro_type_norm);
                record.fields.insert(
                    fname.to_string(),
                    AvroField {
                        typ,
                        has_default: field.get("default").is_some(),
                    },
                );
                // Вложенные record в типе поля.
                if let Some(ftype) = field.get("type") {
                    collect_avro_records(ftype, ns.as_deref(), out);
                }
            }
            out.insert(full, record);
        }
    }
    // Рекурсия по значениям объекта (вложенные схемы/варианты).
    for value in obj.values() {
        match value {
            Value::Object(_) => collect_avro_records(value, ns.as_deref(), out),
            Value::Array(items) => {
                for item in items {
                    collect_avro_records(item, ns.as_deref(), out);
                }
            }
            _ => {}
        }
    }
}

/// Совместимые расширения типов Avro (промоушены спецификации):
/// `int→long/float/double`, `long→float/double`, `float→double`.
const AVRO_PROMOTIONS: &[(&str, &str)] = &[
    ("int", "long"),
    ("int", "float"),
    ("int", "double"),
    ("long", "float"),
    ("long", "double"),
    ("float", "double"),
];

fn avro_promotion(old: &str, new: &str) -> bool {
    AVRO_PROMOTIONS.contains(&(old, new))
}

/// Дифф двух `.avsc`: CD-A01..CD-A05.
///
/// # Errors
/// Текст не парсится как JSON/YAML, документ не похож на Avro record.
fn diff_avro(old_text: &str, new_text: &str, old: &Path, new: &Path) -> Result<Vec<Finding>> {
    let old_doc = parse_contract(old_text, old)?;
    let new_doc = parse_contract(new_text, new)?;
    let mut old_records = BTreeMap::new();
    let mut new_records = BTreeMap::new();
    collect_avro_records(&old_doc, None, &mut old_records);
    collect_avro_records(&new_doc, None, &mut new_records);
    if old_records.is_empty() || new_records.is_empty() {
        return Err(HarnessError::Tool(format!(
            "{}: не Avro record (ожидается «type\": «record» с «fields»)",
            if old_records.is_empty() {
                old.display()
            } else {
                new.display()
            }
        )));
    }
    let mut out = Vec::new();

    // CD-A01 (error): удалённый record; CD-A05 (warn): добавленный.
    for name in old_records.keys() {
        if !new_records.contains_key(name) {
            out.push(Finding {
                severity: "error".into(),
                rule: "CD-A01".into(),
                location: format!("#/avro/record/{}", escape_segment(name)),
                message: format!("удалена запись «{name}»"),
            });
        }
    }
    for name in new_records.keys() {
        if !old_records.contains_key(name) {
            out.push(Finding {
                severity: "warn".into(),
                rule: "CD-A05".into(),
                location: format!("#/avro/record/{}", escape_segment(name)),
                message: format!("добавлена запись «{name}»"),
            });
        }
    }

    for (name, old_rec) in &old_records {
        let Some(new_rec) = new_records.get(name) else {
            continue;
        };
        let loc = format!("#/avro/record/{}", escape_segment(name));
        for (fname, old_field) in &old_rec.fields {
            match new_rec.fields.get(fname) {
                // CD-A02 (error): удалённое поле без default; с default — warn.
                None => {
                    if old_field.has_default {
                        out.push(Finding {
                            severity: "warn".into(),
                            rule: "CD-A05".into(),
                            location: format!("{loc}/field/{}", escape_segment(fname)),
                            message: format!("поле «{fname}» удалено (у старой схемы был default — чтение старых данных безопасно)"),
                        });
                    } else {
                        out.push(Finding {
                            severity: "error".into(),
                            rule: "CD-A02".into(),
                            location: format!("{loc}/field/{}", escape_segment(fname)),
                            message: format!("удалено поле «{fname}» (без default в старой схеме)"),
                        });
                    }
                }
                Some(new_field) => {
                    if old_field.typ == new_field.typ {
                        continue;
                    }
                    let old_set: BTreeSet<&str> = old_field.typ.split('|').collect();
                    let new_set: BTreeSet<&str> = new_field.typ.split('|').collect();
                    if avro_promotion(&old_field.typ, &new_field.typ)
                        || (old_set.len() > 1 && old_set.is_subset(&new_set))
                    {
                        // Промоушен / расширение union — warn.
                        out.push(Finding {
                            severity: "warn".into(),
                            rule: "CD-A05".into(),
                            location: format!("{loc}/field/{}", escape_segment(fname)),
                            message: format!(
                                "поле «{fname}»: тип расширен {} → {}",
                                old_field.typ, new_field.typ
                            ),
                        });
                    } else {
                        out.push(Finding {
                            severity: "error".into(),
                            rule: "CD-A03".into(),
                            location: format!("{loc}/field/{}", escape_segment(fname)),
                            message: format!(
                                "поле «{fname}»: несовместимая смена типа {} → {}",
                                old_field.typ, new_field.typ
                            ),
                        });
                    }
                }
            }
        }
        for (fname, new_field) in &new_rec.fields {
            if old_rec.fields.contains_key(fname) {
                continue;
            }
            // CD-A04 (error): добавленное поле без default (читатели новой
            // схемы не прочтут старые данные); с default — warn.
            if new_field.has_default {
                out.push(Finding {
                    severity: "warn".into(),
                    rule: "CD-A05".into(),
                    location: format!("{loc}/field/{}", escape_segment(fname)),
                    message: format!("добавлено поле «{fname}» (с default)"),
                });
            } else {
                out.push(Finding {
                    severity: "error".into(),
                    rule: "CD-A04".into(),
                    location: format!("{loc}/field/{}", escape_segment(fname)),
                    message: format!("добавлено поле «{fname}» без default"),
                });
            }
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// JSON Schema (топики/тела сообщений)
// ---------------------------------------------------------------------------

/// Множество типов свойства (`type` строкой или массивом).
fn js_type_set(prop: &Value) -> Option<BTreeSet<String>> {
    match prop.get("type") {
        Some(Value::String(s)) => Some(BTreeSet::from([s.clone()])),
        Some(Value::Array(items)) => {
            let set: BTreeSet<String> = items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect();
            (!set.is_empty()).then_some(set)
        }
        _ => None,
    }
}

/// Множество имён из `required`.
fn js_required(schema: &Value) -> BTreeSet<String> {
    schema
        .get("required")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// Дифф двух JSON Schema: CD-J01..CD-J05, рекурсия по вложенным
/// `properties` с потолком [`MAX_JSONSCHEMA_DEPTH`].
///
/// # Errors
/// Текст не парсится как JSON/YAML, документ не похож на JSON Schema.
fn diff_jsonschema(old_text: &str, new_text: &str, old: &Path, new: &Path) -> Result<Vec<Finding>> {
    let old_doc = parse_contract(old_text, old)?;
    let new_doc = parse_contract(new_text, new)?;
    if !is_json_schema(&old_doc) || !is_json_schema(&new_doc) {
        return Err(HarnessError::Tool(format!(
            "{}: не JSON Schema (нет $schema/properties/required/type)",
            if is_json_schema(&old_doc) {
                new.display()
            } else {
                old.display()
            }
        )));
    }
    let mut out = Vec::new();
    diff_js_object(&old_doc, &new_doc, "#", 0, &mut out);
    Ok(out)
}

/// Рекурсивный дифф объекта схемы (уровень `properties`+`required`).
fn diff_js_object(old: &Value, new: &Value, loc: &str, depth: usize, out: &mut Vec<Finding>) {
    if depth > MAX_JSONSCHEMA_DEPTH {
        return;
    }
    let old_props = old.get("properties").and_then(Value::as_object);
    let new_props = new.get("properties").and_then(Value::as_object);
    let old_req = js_required(old);
    let new_req = js_required(new);

    if let Some(old_props) = old_props {
        for (name, old_prop) in old_props {
            let ploc = format!("{loc}/properties/{}", escape_segment(name));
            match new_props.and_then(|np| np.get(name)) {
                // CD-J01 (error): удалённое свойство.
                None => out.push(Finding {
                    severity: "error".into(),
                    rule: "CD-J01".into(),
                    location: ploc,
                    message: format!("удалено свойство «{name}»"),
                }),
                Some(new_prop) => {
                    // CD-J04 (warn): снята обязательность.
                    if old_req.contains(name) && !new_req.contains(name) {
                        out.push(Finding {
                            severity: "warn".into(),
                            rule: "CD-J04".into(),
                            location: ploc.clone(),
                            message: format!("свойство «{name}» перестало быть required"),
                        });
                    }
                    // CD-J02 (error): свойство стало обязательным.
                    if !old_req.contains(name) && new_req.contains(name) {
                        out.push(Finding {
                            severity: "error".into(),
                            rule: "CD-J02".into(),
                            location: ploc.clone(),
                            message: format!("свойство «{name}» стало required"),
                        });
                    }
                    // CD-J03/CD-J05: сужение/смена/расширение типа.
                    if let (Some(old_t), Some(new_t)) =
                        (js_type_set(old_prop), js_type_set(new_prop))
                    {
                        if old_t != new_t {
                            if new_t.is_subset(&old_t) {
                                out.push(Finding {
                                    severity: "error".into(),
                                    rule: "CD-J03".into(),
                                    location: ploc.clone(),
                                    message: format!(
                                        "свойство «{name}»: сужение типа [{}] → [{}]",
                                        old_t.into_iter().collect::<Vec<_>>().join("|"),
                                        new_t.into_iter().collect::<Vec<_>>().join("|")
                                    ),
                                });
                            } else if old_t.is_subset(&new_t) {
                                out.push(Finding {
                                    severity: "warn".into(),
                                    rule: "CD-J05".into(),
                                    location: ploc.clone(),
                                    message: format!(
                                        "свойство «{name}»: тип расширен [{}] → [{}]",
                                        old_t.into_iter().collect::<Vec<_>>().join("|"),
                                        new_t.into_iter().collect::<Vec<_>>().join("|")
                                    ),
                                });
                            } else {
                                out.push(Finding {
                                    severity: "error".into(),
                                    rule: "CD-J03".into(),
                                    location: ploc.clone(),
                                    message: format!(
                                        "свойство «{name}»: смена типа [{}] → [{}]",
                                        old_t.into_iter().collect::<Vec<_>>().join("|"),
                                        new_t.into_iter().collect::<Vec<_>>().join("|")
                                    ),
                                });
                            }
                        }
                    }
                    // Рекурсия по вложенным объектам.
                    if old_prop.get("properties").is_some() && new_prop.get("properties").is_some()
                    {
                        diff_js_object(old_prop, new_prop, &ploc, depth + 1, out);
                    }
                }
            }
        }
    }
    if let Some(new_props) = new_props {
        for name in new_props.keys() {
            if old_props.is_some_and(|op| op.contains_key(name)) {
                continue;
            }
            let ploc = format!("{loc}/properties/{}", escape_segment(name));
            // CD-J02 (error): добавлено сразу обязательное свойство;
            // необязательное — CD-J05 (warn).
            if new_req.contains(name) {
                out.push(Finding {
                    severity: "error".into(),
                    rule: "CD-J02".into(),
                    location: ploc,
                    message: format!("добавлено обязательное свойство «{name}»"),
                });
            } else {
                out.push(Finding {
                    severity: "warn".into(),
                    rule: "CD-J05".into(),
                    location: ploc,
                    message: format!("добавлено необязательное свойство «{name}»"),
                });
            }
        }
    }
}

// ---------------------------------------------------------------------------
// DDL-миграции (.sql)
// ---------------------------------------------------------------------------

/// Колонка в итоговом состоянии DDL.
#[derive(Debug, Clone)]
struct DdlColumn {
    /// Тип (нижний регистр, схлопнутые пробелы).
    typ: String,
    /// `NOT NULL` (или `PRIMARY KEY`).
    not_null: bool,
    /// Есть `DEFAULT`.
    has_default: bool,
}

/// Итоговое состояние схемы после применения всех операторов файла.
#[derive(Debug, Default)]
struct DdlSchema {
    /// Таблицы → колонки по имени.
    tables: BTreeMap<String, BTreeMap<String, DdlColumn>>,
}

/// Нормализация идентификатора SQL: срез кавычек/обратных кавычек/скобок,
/// нижний регистр (неквотированные идентификаторы PostgreSQL складываются
/// в нижний регистр; консервативно — для всех диалектов).
fn sql_ident(raw: &str) -> String {
    raw.trim()
        .trim_matches('"')
        .trim_matches('`')
        .trim_matches(['[', ']'])
        .to_ascii_lowercase()
}

/// Ключевые слова-ограничители в определении колонки (по ним обрезается тип).
const SQL_CONSTRAINT_WORDS: [&str; 10] = [
    "primary",
    "not",
    "null",
    "default",
    "references",
    "unique",
    "check",
    "constraint",
    "collate",
    "generated",
];

/// Разбор определения колонки (`name type [constraints]`).
fn parse_column_def(def: &str) -> Option<(String, DdlColumn)> {
    let tokens: Vec<&str> = def.split_whitespace().collect();
    let name = sql_ident(tokens.first()?);
    if SQL_CONSTRAINT_WORDS.contains(&name.as_str()) {
        return None; // табличный constraint, не колонка
    }
    let mut type_tokens: Vec<&str> = Vec::new();
    for tok in tokens.iter().skip(1) {
        let low = tok.to_ascii_lowercase();
        // Тип кончается первым словом-ограничителем (NOT NULL, DEFAULT, …).
        let stripped = low.trim_end_matches([',', ')']);
        if !type_tokens.is_empty() && SQL_CONSTRAINT_WORDS.contains(&stripped) {
            break;
        }
        type_tokens.push(tok);
    }
    let typ = type_tokens
        .join(" ")
        .to_ascii_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let rest = def.to_ascii_lowercase();
    Some((
        name,
        DdlColumn {
            typ,
            not_null: rest.contains("not null") || rest.contains("primary key"),
            has_default: rest.contains("default"),
        },
    ))
}

/// Разбивает тело `CREATE TABLE (...)` на определения по запятым верхнего
/// уровня (запятые внутри `varchar(…)`/check-скобок не режем).
fn split_top_level_commas(body: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut current = String::new();
    for ch in body.chars() {
        match ch {
            '(' => {
                depth += 1;
                current.push(ch);
            }
            ')' => {
                depth -= 1;
                current.push(ch);
            }
            ',' if depth == 0 => {
                if !current.trim().is_empty() {
                    out.push(current.trim().to_string());
                }
                current.clear();
            }
            _ => current.push(ch),
        }
    }
    if !current.trim().is_empty() {
        out.push(current.trim().to_string());
    }
    out
}

/// Приводит DDL-файл к итоговому состоянию схемы: `CREATE TABLE` +
/// `ALTER TABLE` (add/drop column, type, not null) + `DROP TABLE`.
/// Незнакомые операторы пропускаются (консервативный скелет).
fn parse_ddl(text: &str) -> DdlSchema {
    let create_re = regex::Regex::new(
        r"(?i)^\s*create\s+table\s+(?:if\s+not\s+exists\s+)?([^\s(]+)\s*\((.*)\)\s*$",
    );
    let alter_re =
        regex::Regex::new(r"(?i)^\s*alter\s+table\s+(?:if\s+exists\s+)?([^\s]+)\s+(.*)$");
    let drop_re = regex::Regex::new(r"(?i)^\s*drop\s+table\s+(?:if\s+exists\s+)?(.+)$");
    let (Ok(create_re), Ok(alter_re), Ok(drop_re)) = (create_re, alter_re, drop_re) else {
        return DdlSchema::default();
    };

    let mut schema = DdlSchema::default();
    for stmt in text.split(';').take(MAX_DDL_STATEMENTS) {
        // Однострочные комментарии -- срезаем построчно.
        let cleaned: String = stmt
            .lines()
            .map(|l| l.split("--").next().unwrap_or(""))
            .collect::<Vec<_>>()
            .join(" ");
        let stmt = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
        if stmt.is_empty() {
            continue;
        }
        if let Some(caps) = create_re.captures(&stmt) {
            let table = sql_ident(&caps[1]);
            let columns = schema.tables.entry(table).or_default();
            for def in split_top_level_commas(&caps[2]) {
                if let Some((name, col)) = parse_column_def(&def) {
                    columns.insert(name, col);
                }
            }
            continue;
        }
        if let Some(caps) = drop_re.captures(&stmt) {
            for name in caps[1].split(',') {
                let name = name.trim();
                if name.is_empty() {
                    continue;
                }
                // Отсекаем хвосты cascade/restrict.
                let name = name.split_whitespace().next().unwrap_or(name);
                schema.tables.remove(&sql_ident(name));
            }
            continue;
        }
        if let Some(caps) = alter_re.captures(&stmt) {
            let table = sql_ident(&caps[1]);
            let actions = split_top_level_commas(&caps[2]);
            let Some(columns) = schema.tables.get_mut(&table) else {
                continue; // alter несуществующей таблицы — пропускаем
            };
            for action in actions {
                apply_alter_action(columns, &action);
            }
        }
    }
    schema
}

/// Применяет одно действие `ALTER TABLE` к набору колонок.
fn apply_alter_action(columns: &mut BTreeMap<String, DdlColumn>, action: &str) {
    let low = action.to_ascii_lowercase();
    let tokens: Vec<&str> = action.split_whitespace().collect();
    if low.starts_with("add column ") || low.starts_with("add ") {
        let skip = if low.starts_with("add column ") { 2 } else { 1 };
        let Some(def) = tokens.get(skip..).map(|t| t.join(" ")) else {
            return;
        };
        // «if not exists» в определении пропускаем.
        let def = if def.to_ascii_lowercase().starts_with("if not exists ") {
            def.split_whitespace().skip(3).collect::<Vec<_>>().join(" ")
        } else {
            def
        };
        if let Some((name, col)) = parse_column_def(&def) {
            columns.insert(name, col);
        }
        return;
    }
    if low.starts_with("drop column ") || low.starts_with("drop ") {
        let skip = if low.starts_with("drop column ") {
            2
        } else {
            1
        };
        if let Some(name) = tokens.get(skip) {
            columns.remove(&sql_ident(name));
        }
        return;
    }
    if low.starts_with("alter column ") || low.starts_with("alter ") || low.starts_with("modify ") {
        let name_idx = 1;
        let Some(raw_name) = tokens.get(name_idx) else {
            return;
        };
        let name = sql_ident(raw_name);
        let rest = tokens.get(2..).map(|t| t.join(" ")).unwrap_or_default();
        let rest_low = rest.to_ascii_lowercase();
        let Some(col) = columns.get_mut(&name) else {
            return;
        };
        if let Some(pos) = rest_low.find(" type ") {
            // «SET DATA TYPE …» / «TYPE …»: тип до « using » или конца.
            let type_start = pos + " type ".len();
            let tail = &rest[type_start..];
            let tail = tail.split(" using ").next().unwrap_or(tail);
            col.typ = tail.trim().to_ascii_lowercase();
        } else if rest_low.starts_with("set not null") {
            col.not_null = true;
        } else if rest_low.starts_with("drop not null") {
            col.not_null = false;
        } else if low.starts_with("modify ") {
            // MySQL: MODIFY <name> <type> [constraints].
            let def = tokens.get(2..).map(|t| t.join(" ")).unwrap_or_default();
            if let Some((_, new_col)) = parse_column_def(&format!("{name} {def}")) {
                *col = new_col;
            }
        }
    }
}

/// Скалярные совместимые расширения типов SQL (warn).
const SQL_SCALAR_WIDENINGS: &[(&str, &str)] = &[
    ("smallint", "int"),
    ("smallint", "integer"),
    ("smallint", "bigint"),
    ("int", "bigint"),
    ("integer", "bigint"),
    ("real", "double precision"),
    ("float", "double precision"),
    ("varchar", "text"),
];

/// Совместимые расширения типов SQL (warn): `int→bigint`, `smallint→int/bigint`,
/// `real→double precision`, `varchar(N→M≥N)`, `char(N→M≥N)`,
/// `numeric(p,s)→numeric(p'≥p,s'≥s)`.
fn sql_type_widening(old: &str, new: &str) -> bool {
    if old == new {
        return true;
    }
    if SQL_SCALAR_WIDENINGS.contains(&(old, new)) {
        return true;
    }
    // Параметризованные типы: name(a[, b]).
    if let (Some((on, op)), Some((nn, np))) = (sql_type_params(old), sql_type_params(new)) {
        if on != nn
            || !matches!(
                on,
                "varchar" | "char" | "character varying" | "numeric" | "decimal"
            )
        {
            return false;
        }
        return op.len() == np.len() && op.iter().zip(np.iter()).all(|(o, n)| n >= o);
    }
    false
}

/// Разбор параметризованного типа `name(a[, b])` → (имя, параметры).
fn sql_type_params(t: &str) -> Option<(&str, Vec<u64>)> {
    let (name, rest) = t.split_once('(')?;
    let rest = rest.trim_end_matches(')');
    let nums: Option<Vec<u64>> = rest
        .split(',')
        .map(|p| p.trim().parse::<u64>().ok())
        .collect();
    Some((name.trim(), nums?))
}

/// Дифф двух DDL-файлов (итоговых состояний): CD-S01..CD-S05.
fn diff_ddl(old: &str, new: &str) -> Vec<Finding> {
    let old_schema = parse_ddl(old);
    let new_schema = parse_ddl(new);
    let mut out = Vec::new();

    // CD-S01 (error): удалённая таблица; CD-S05 (warn): добавленная.
    for table in old_schema.tables.keys() {
        if !new_schema.tables.contains_key(table) {
            out.push(Finding {
                severity: "error".into(),
                rule: "CD-S01".into(),
                location: format!("#/ddl/table/{}", escape_segment(table)),
                message: format!("удалена таблица «{table}»"),
            });
        }
    }
    for (table, columns) in &new_schema.tables {
        if !old_schema.tables.contains_key(table) {
            out.push(Finding {
                severity: "warn".into(),
                rule: "CD-S05".into(),
                location: format!("#/ddl/table/{}", escape_segment(table)),
                message: format!("добавлена таблица «{table}» ({} колонок)", columns.len()),
            });
        }
    }

    for (table, old_cols) in &old_schema.tables {
        let Some(new_cols) = new_schema.tables.get(table) else {
            continue;
        };
        let loc = format!("#/ddl/table/{}", escape_segment(table));
        for (col, old_col) in old_cols {
            let cloc = format!("{loc}/column/{}", escape_segment(col));
            match new_cols.get(col) {
                // CD-S02 (error): удалённая колонка.
                None => out.push(Finding {
                    severity: "error".into(),
                    rule: "CD-S02".into(),
                    location: cloc,
                    message: format!("удалена колонка «{table}.{col}»"),
                }),
                Some(new_col) => {
                    // CD-S03 (error): несовместимая смена типа; расширение — warn.
                    if old_col.typ != new_col.typ {
                        if sql_type_widening(&old_col.typ, &new_col.typ) {
                            out.push(Finding {
                                severity: "warn".into(),
                                rule: "CD-S05".into(),
                                location: cloc.clone(),
                                message: format!(
                                    "колонка «{table}.{col}»: тип расширен {} → {}",
                                    old_col.typ, new_col.typ
                                ),
                            });
                        } else {
                            out.push(Finding {
                                severity: "error".into(),
                                rule: "CD-S03".into(),
                                location: cloc.clone(),
                                message: format!(
                                    "колонка «{table}.{col}»: несовместимая смена типа {} → {}",
                                    old_col.typ, new_col.typ
                                ),
                            });
                        }
                    }
                    // CD-S04 (error): стала NOT NULL без default; с default — warn.
                    if !old_col.not_null && new_col.not_null {
                        if new_col.has_default {
                            out.push(Finding {
                                severity: "warn".into(),
                                rule: "CD-S05".into(),
                                location: cloc.clone(),
                                message: format!(
                                    "колонка «{table}.{col}» стала NOT NULL (с DEFAULT — существующие строки заполнятся)"
                                ),
                            });
                        } else {
                            out.push(Finding {
                                severity: "error".into(),
                                rule: "CD-S04".into(),
                                location: cloc.clone(),
                                message: format!(
                                    "колонка «{table}.{col}» стала NOT NULL без DEFAULT — существующие NULL не пройдут"
                                ),
                            });
                        }
                    }
                    if old_col.not_null && !new_col.not_null {
                        out.push(Finding {
                            severity: "warn".into(),
                            rule: "CD-S05".into(),
                            location: cloc,
                            message: format!("колонка «{table}.{col}»: снято NOT NULL"),
                        });
                    }
                }
            }
        }
        for (col, new_col) in new_cols {
            if old_cols.contains_key(col) {
                continue;
            }
            let cloc = format!("{loc}/column/{}", escape_segment(col));
            // CD-S04 (error): добавлена NOT NULL колонка без default;
            // nullable/с default — CD-S05 (warn).
            if new_col.not_null && !new_col.has_default {
                out.push(Finding {
                    severity: "error".into(),
                    rule: "CD-S04".into(),
                    location: cloc,
                    message: format!(
                        "добавлена обязательная колонка «{table}.{col}» без DEFAULT — вставка в существующие строки невозможна"
                    ),
                });
            } else {
                out.push(Finding {
                    severity: "warn".into(),
                    rule: "CD-S05".into(),
                    location: cloc,
                    message: format!("добавлена колонка «{table}.{col}»"),
                });
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Инструмент, рендер, CLI-вердикт
// ---------------------------------------------------------------------------

/// JSON Pointer-экранирование сегмента: `~` → `~0`, `/` → `~1`.
fn escape_segment(segment: &str) -> String {
    segment.replace('~', "~0").replace('/', "~1")
}

/// Собирает JSON Pointer с якорем `#` из сегментов.
fn location(segments: &[&str]) -> String {
    let escaped: Vec<String> = segments.iter().map(|s| escape_segment(s)).collect();
    format!("#/{}", escaped.join("/"))
}

/// Собирает отчёт в стиле `spine_lint`: сводка с форматом, строки находок,
/// итог, опциональная секция связки с моделью.
#[must_use]
pub fn render_report(report: &DiffReport) -> String {
    let findings = &report.findings;
    let breaking = findings.iter().filter(|f| f.severity == "error").count();
    let non_breaking = findings.len() - breaking;
    let mut out = format!(
        "contract_diff: {} изменений (breaking: {breaking}, non-breaking: {non_breaking})\nФормат: {}",
        findings.len(),
        report.format.name()
    );
    // Запись в String не может завершиться ошибкой — игноры безопасны.
    for f in findings {
        let _ = writeln!(
            out,
            "[{}] {} {} — {}",
            f.severity, f.location, f.rule, f.message
        );
    }
    let _ = writeln!(out, "Итог: {}", if breaking == 0 { "PASS" } else { "FAIL" });
    if let Some(impact) = &report.impact {
        if impact.matched_int.is_empty() {
            let _ = writeln!(
                out,
                "Связь с моделью: ни один INT.contract не совпал с путями old/new (gap — контракт вне модели, ADR-035)"
            );
        } else {
            let _ = writeln!(
                out,
                "Связь с моделью: {} ({})",
                impact.matched_int.join(", "),
                impact.matched_paths.join(", ")
            );
            if !impact.consumers.is_empty() {
                let _ = writeln!(out, "  Потребители: {}", impact.consumers.join("; "));
            }
            if !impact.rules.is_empty() {
                let _ = writeln!(out, "  Правила: {}", impact.rules.join("; "));
            }
            if !impact.owners.is_empty() {
                let _ = writeln!(
                    out,
                    "  Владельцы (согласовать): {}",
                    impact.owners.join("; ")
                );
            }
            let _ = writeln!(out, "  {}", impact.summary);
        }
    }
    out
}

/// JSON-форма отчёта (`arch-be contract-diff --json`).
#[must_use]
pub fn report_json(report: &DiffReport) -> Value {
    let breaking = report
        .findings
        .iter()
        .filter(|f| f.severity == "error")
        .count();
    json!({
        "tool": "contract_diff",
        "passed": !report.has_breaking(),
        "format": report.format.name(),
        "breaking": breaking,
        "non_breaking": report.findings.len() - breaking,
        "findings": report.findings.iter().map(|f| json!({
            "severity": f.severity,
            "rule": f.rule,
            "location": f.location,
            "message": f.message,
        })).collect::<Vec<_>>(),
        "impact": report.impact.as_ref().map(|i| json!({
            "matched_int": i.matched_int,
            "matched_paths": i.matched_paths,
            "consumers": i.consumers,
            "rules": i.rules,
            "owners": i.owners,
            "summary": i.summary,
        })),
        "summary": format!(
            "{} изменений (breaking: {breaking}), формат {}",
            report.findings.len(),
            report.format.name()
        ),
    })
}

/// Инструменты модуля: `contract_diff`.
#[must_use]
pub fn tools() -> Vec<Arc<dyn Tool>> {
    vec![Arc::new(ContractDiffTool)]
}

/// Инструмент `contract_diff`: сравнение двух версий контракта на breaking
/// changes — `OpenAPI` 3.x (CD-001..CD-010, транш T1+T-06+Д5, ADR-015), protobuf/gRPC
/// (CD-P01..CD-P06), Avro (CD-A01..CD-A05), JSON Schema (CD-J01..CD-J05),
/// DDL-миграции (CD-S01..CD-S05) (бэклог волны 3, п.14).
pub struct ContractDiffTool;

#[derive(Debug, Deserialize)]
struct ContractDiffArgs {
    /// Путь к старой версии контракта.
    old: String,
    /// Путь к новой версии контракта.
    new: String,
    /// Формат: auto (детектор, дефолт) | openapi | proto | avro |
    /// jsonschema | ddl.
    format: Option<String>,
    /// Корень кейса с `model/` — связка с моделью (ADR-035): по полю
    /// `contract` INT находятся потребители и владельцы (impact-секция).
    model: Option<String>,
}

#[async_trait]
impl Tool for ContractDiffTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "contract_diff".into(),
            description: "Сравнить две версии контракта на breaking changes: OpenAPI 3.x \
                          (CD-001..CD-010 — тело запроса/ответа, CD-007: ломающий дифф без смены major \
                          info.version), protobuf/gRPC .proto (удалённые/перенумерованные \
                          поля, rpc, CD-P06 major пакета), Avro .avsc (поля без default, \
                          несовместимые типы), JSON Schema топиков (required/properties/тип), \
                          DDL-миграции .sql (DROP/ALTER/NOT NULL без DEFAULT). format=auto — \
                          детектор по расширению/содержимому. model — корень кейса с model/: \
                          ломающий дифф сразу возвращает потребителей и владельцев по полю \
                          contract у INT (ADR-035)"
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "old": {
                        "type": "string",
                        "description": "Путь к старой версии контракта (yaml/yml/json/proto/avsc/sql)"
                    },
                    "new": {
                        "type": "string",
                        "description": "Путь к новой версии контракта (тот же формат)"
                    },
                    "format": {
                        "type": "string",
                        "description": "Формат: auto (по умолчанию — детектор) | openapi | proto | avro | jsonschema | ddl",
                        "enum": ["auto", "openapi", "proto", "avro", "jsonschema", "ddl"]
                    },
                    "model": {
                        "type": "string",
                        "description": "Корень кейса с model/ — секция impact: потребители/владельцы ломаемого контракта (ADR-035)"
                    }
                },
                "required": ["old", "new"]
            }),
        }
    }

    async fn call(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let args: ContractDiffArgs = match serde_json::from_value(args) {
            Ok(a) => a,
            Err(e) => {
                return Ok(ToolOutput::err(format!(
                    "contract_diff: невалидные аргументы: {e}"
                )));
            }
        };
        let format = match args.format.as_deref().unwrap_or("auto") {
            "auto" => None,
            other => match ContractFormat::from_name(other) {
                Some(f) => Some(f),
                None => {
                    return Ok(ToolOutput::err(format!(
                        "contract_diff: неизвестный формат '{other}' (допустимы: auto, openapi, proto, avro, jsonschema, ddl)"
                    )));
                }
            },
        };
        let old = ctx.resolve(&args.old);
        let new = ctx.resolve(&args.new);
        let model = args.model.as_deref().map(|m| ctx.resolve(m));
        let report = match diff_report(&old, &new, format, model.as_deref()) {
            Ok(r) => r,
            Err(e) => return Ok(ToolOutput::err(format!("contract_diff: {e}"))),
        };
        // Текст — человеко-читаемый рендер (его видит модель), разобранный
        // вердикт — в `data` (T-12): мост MCP кладёт его в `structuredContent`,
        // и клиенту не приходится разбирать JSON из строки.
        Ok(ToolOutput::ok(render_report(&report)).with_data(report_json(&report)))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use serde_json::json;

    use super::*;
    use crate::tool::ToolContext;

    /// Контракт-эталон: один путь с двумя операциями, параметрами, ответами и схемой.
    const BASE: &str = r"openapi: 3.0.3
info:
  title: Pet Store API
  version: 1.0.0
paths:
  /v1/pets:
    get:
      operationId: listPets
      parameters:
        - name: limit
          in: query
          required: false
          schema:
            type: integer
      responses:
        '200':
          description: ok
        '404':
          description: not found
    post:
      operationId: createPet
      parameters:
        - name: idempotency-key
          in: header
          required: true
          schema:
            type: string
      responses:
        '201':
          description: created
components:
  schemas:
    Pet:
      type: object
      properties:
        name:
          type: string
        age:
          type: integer
";

    /// Блок path item `/v1/pets` в [`BASE`] (удаляется в тестах CD-001/CD-005).
    const PETS_PATH: &str = "  /v1/pets:\n    get:\n      operationId: listPets\n      parameters:\n        - name: limit\n          in: query\n          required: false\n          schema:\n            type: integer\n      responses:\n        '200':\n          description: ok\n        '404':\n          description: not found\n    post:\n      operationId: createPet\n      parameters:\n        - name: idempotency-key\n          in: header\n          required: true\n          schema:\n            type: string\n      responses:\n        '201':\n          description: created\n";

    /// Блок post-операции в [`BASE`] (удаляется в тестах CD-002/CD-005).
    const POST_BLOCK: &str = "    post:\n      operationId: createPet\n      parameters:\n        - name: idempotency-key\n          in: header\n          required: true\n          schema:\n            type: string\n      responses:\n        '201':\n          description: created\n";

    /// Обязательный параметр post-операции в [`BASE`] (удаляется в тесте CD-003).
    const REQUIRED_PARAM: &str = "        - name: idempotency-key\n          in: header\n          required: true\n          schema:\n            type: string\n";

    /// Необязательный параметр get-операции в [`BASE`] (удаляется в тесте CD-005).
    const LIMIT_PARAM: &str = "        - name: limit\n          in: query\n          required: false\n          schema:\n            type: integer\n";

    /// Ответ 404 get-операции в [`BASE`] (удаляется в тестах CD-004/CD-005).
    const NOT_FOUND_RESPONSE: &str = "        '404':\n          description: not found\n";

    /// Запускает инструмент на паре контрактов, записанных во временные файлы.
    async fn diff_text(old: &str, new: &str, old_name: &str, new_name: &str) -> ToolOutput {
        let dir = tempfile::tempdir().expect("tmp");
        std::fs::write(dir.path().join(old_name), old).expect("запись старого контракта");
        std::fs::write(dir.path().join(new_name), new).expect("запись нового контракта");
        let ctx = ToolContext::new(
            dir.path().to_path_buf(),
            Arc::new(crate::config::Config::default()),
        );
        tools()[0]
            .call(json!({"old": old_name, "new": new_name}), &ctx)
            .await
            .expect("вызов contract_diff")
    }

    #[test]
    fn factory_exposes_single_contract_diff_tool() {
        let ts = tools();
        assert_eq!(ts.len(), 1);
        let spec = ts[0].spec();
        assert_eq!(spec.name, "contract_diff");
        assert!(
            spec.description.contains("OpenAPI 3.x"),
            "{}",
            spec.description
        );
        assert!(
            spec.description.contains("breaking"),
            "{}",
            spec.description
        );
        assert_eq!(spec.parameters["required"], json!(["old", "new"]));
    }

    #[tokio::test]
    async fn identical_contracts_pass_clean() {
        let out = diff_text(BASE, BASE, "old.yaml", "new.yaml").await;
        assert!(!out.is_error, "{}", out.content);
        assert!(
            out.content
                .contains("contract_diff: 0 изменений (breaking: 0, non-breaking: 0)"),
            "{}",
            out.content
        );
        assert!(out.content.contains("Формат: openapi"), "{}", out.content);
        assert!(out.content.contains("Итог: PASS"), "{}", out.content);
        assert!(!out.content.contains("CD-00"), "{}", out.content);
    }

    /// T-06: новое обязательное поле в теле запроса — ломающее изменение.
    /// Раньше `contract_diff` показывал breaking 0: схема тела запроса
    /// (даже inline) вообще не сравнивалась.
    #[tokio::test]
    async fn cd008_new_required_request_field_is_breaking() {
        let old = "openapi: 3.0.3\ninfo:\n  title: Wallets\n  version: 1.0.0\npaths:\n  /v1/topup:\n    post:\n      operationId: topup\n      requestBody:\n        content:\n          application/json:\n            schema:\n              type: object\n              required: [amount]\n              properties:\n                amount:\n                  type: integer\n                source:\n                  type: string\n      responses:\n        '200':\n          description: ok\n";
        let new = "openapi: 3.0.3\ninfo:\n  title: Wallets\n  version: 1.0.0\npaths:\n  /v1/topup:\n    post:\n      operationId: topup\n      requestBody:\n        content:\n          application/json:\n            schema:\n              type: object\n              required: [amount, source]\n              properties:\n                amount:\n                  type: integer\n                source:\n                  type: string\n      responses:\n        '200':\n          description: ok\n";
        let out = diff_text(old, new, "old.yaml", "new.yaml").await;
        assert!(!out.is_error, "{}", out.content);
        assert!(out.content.contains("CD-008"), "{}", out.content);
        assert!(
            out.content.contains("стало обязательным"),
            "{}",
            out.content
        );
        assert_eq!(
            out.content.matches("CD-008").count(),
            1,
            "одна находка CD-008: {}",
            out.content
        );
        assert!(out.content.contains("Итог: FAIL"), "{}", out.content);
        // CD-007: ломающий дифф без смены major — отдельное требование.
        assert!(
            out.content.contains("major") || out.content.contains("CD-007"),
            "{}",
            out.content
        );

        // Новое НЕобязательное поле — не ломающее.
        let optional = new.replace("required: [amount, source]", "required: [amount]");
        let out = diff_text(old, &optional, "old.yaml", "new.yaml").await;
        assert!(!out.content.contains("CD-008"), "{}", out.content);
        assert!(out.content.contains("breaking: 0"), "{}", out.content);
    }

    /// T-06: то же через `$ref` и во вложенном объекте — схема тела запроса
    /// раскрывается по ссылке и рекурсивно.
    #[tokio::test]
    async fn cd008_sees_ref_and_nested_required_fields() {
        let with_ref = |required: &str| {
            format!(
                "openapi: 3.0.3\ninfo:\n  title: Wallets\n  version: 1.0.0\npaths:\n  /v1/topup:\n    post:\n      operationId: topup\n      requestBody:\n        content:\n          application/json:\n            schema:\n              $ref: '#/components/schemas/Topup'\n      responses:\n        '200':\n          description: ok\ncomponents:\n  schemas:\n    Topup:\n      type: object\n      required: {required}\n      properties:\n        amount:\n          type: integer\n        wallet:\n          type: object\n          required: [id]\n          properties:\n            id:\n              type: string\n            label:\n              type: string\n"
            )
        };
        // Вложение: `wallet.id` был обязателен (wallet обязателен) — новое
        // обязательное `wallet.label` обязано быть названо полным путём.
        let old = with_ref("[amount, wallet]");
        let new = with_ref("[amount, wallet, topup_id]").replace(
            "          required: [id]\n",
            "          required: [id, label]\n",
        );
        let out = diff_text(&old, &new, "old.yaml", "new.yaml").await;
        assert!(!out.is_error, "{}", out.content);
        assert!(out.content.contains("CD-008"), "{}", out.content);
        assert!(out.content.contains("topup_id"), "{}", out.content);
        assert!(out.content.contains("wallet.label"), "{}", out.content);
        assert_eq!(
            out.content.matches("CD-008").count(),
            2,
            "две находки CD-008 (вложенное поле и поле через $ref): {}",
            out.content
        );

        // Незнакомый $ref не выдумывает полей — дифф молчит о теле запроса.
        let broken = new.replace("#/components/schemas/Topup", "#/components/schemas/Nope");
        let out = diff_text(&old, &broken, "old.yaml", "new.yaml").await;
        assert!(!out.content.contains("CD-008"), "{}", out.content);
    }

    /// Д5: поле, исчезнувшее из тела ОТВЕТА, — ломающее. До 0.3.5
    /// `diff_responses` сравнивал только коды ответов: удаление поля из схемы
    /// ответа давало «breaking: 0».
    #[tokio::test]
    async fn openapi_removed_response_property_is_breaking() {
        let contract = |fee: &str, required: &str| {
            format!(
                "openapi: 3.0.3\ninfo:\n  title: Wallets\n  version: 1.0.0\npaths:\n  /v1/topup:\n    post:\n      operationId: topup\n      responses:\n        '200':\n          description: ok\n          content:\n            application/json:\n              schema:\n                $ref: '#/components/schemas/Receipt'\ncomponents:\n  schemas:\n    Receipt:\n      type: object\n      required: {required}\n      properties:\n        id:\n          type: string\n{fee}"
            )
        };
        let old = contract("        fee:\n          type: integer\n", "[id, fee]");
        let new = contract("", "[id]");
        let out = diff_text(&old, &new, "old.yaml", "new.yaml").await;
        assert!(!out.is_error, "{}", out.content);
        assert!(out.content.contains("CD-009"), "{}", out.content);
        assert!(
            out.content
                .contains("/responses/200/content/application~1json/schema/fee"),
            "путь находки называет код ответа, media type и поле: {}",
            out.content
        );
        assert!(out.content.contains("Итог: FAIL"), "{}", out.content);
    }

    /// Д5: удаление поля уносит поддерево — сообщается минимальный путь.
    /// Рекурсивные схемы иначе дают десяток находок об одном удалении.
    #[tokio::test]
    async fn removed_response_field_does_not_report_its_subtree() {
        let contract = |wallet: &str| {
            format!(
                "openapi: 3.0.3\ninfo:\n  title: Wallets\n  version: 1.0.0\npaths:\n  /v1/topup:\n    post:\n      operationId: topup\n      responses:\n        '200':\n          description: ok\n          content:\n            application/json:\n              schema:\n                type: object\n                required: [id]\n                properties:\n                  id:\n                    type: string\n{wallet}"
            )
        };
        let old = contract(
            "                  wallet:\n                    type: object\n                    required: [id]\n                    properties:\n                      id:\n                        type: string\n                      label:\n                        type: string\n",
        );
        let new = contract("");
        let out = diff_text(&old, &new, "old.yaml", "new.yaml").await;
        assert!(!out.is_error, "{}", out.content);
        assert_eq!(
            out.content.matches("CD-009").count(),
            1,
            "одна находка на удалённое поддерево: {}",
            out.content
        );
        assert!(
            out.content.contains("/schema/wallet CD-009"),
            "{}",
            out.content
        );
        assert!(!out.content.contains("wallet.id"), "{}", out.content);
        assert!(!out.content.contains("wallet.label"), "{}", out.content);
    }

    /// Д5: направление у ответов обратное запросам — новое обязательное поле
    /// ответа НЕ ломает: потребитель его просто не читал.
    #[tokio::test]
    async fn openapi_new_required_response_property_is_compatible() {
        let contract = |extra: &str, required: &str| {
            format!(
                "openapi: 3.0.3\ninfo:\n  title: Wallets\n  version: 1.0.0\npaths:\n  /v1/topup:\n    post:\n      operationId: topup\n      responses:\n        '200':\n          description: ok\n          content:\n            application/json:\n              schema:\n                type: object\n                required: {required}\n                properties:\n                  id:\n                    type: string\n{extra}"
            )
        };
        let old = contract("", "[id]");
        let new = contract(
            "                  status:\n                    type: string\n",
            "[id, status]",
        );
        let out = diff_text(&old, &new, "old.yaml", "new.yaml").await;
        assert!(!out.is_error, "{}", out.content);
        assert!(!out.content.contains("CD-009"), "{}", out.content);
        assert!(out.content.contains("breaking: 0"), "{}", out.content);
        assert!(out.content.contains("Итог: PASS"), "{}", out.content);
    }

    /// Д5: тело запроса, ставшее обязательным, — ломающее: вызов без тела
    /// перестаёт работать. Сравниваются флаги `requestBody.required`, а не
    /// содержимое схемы (у тела может не быть схемы вовсе).
    #[tokio::test]
    async fn openapi_request_body_became_required_is_breaking() {
        let contract = |required: &str| {
            format!(
                "openapi: 3.0.3\ninfo:\n  title: Wallets\n  version: 1.0.0\npaths:\n  /v1/topup:\n    post:\n      operationId: topup\n      requestBody:\n        required: {required}\n        content:\n          application/json:\n            schema:\n              type: object\n              properties:\n                amount:\n                  type: integer\n      responses:\n        '200':\n          description: ok\n"
            )
        };
        let out = diff_text(
            &contract("false"),
            &contract("true"),
            "old.yaml",
            "new.yaml",
        )
        .await;
        assert!(!out.is_error, "{}", out.content);
        assert!(out.content.contains("CD-010"), "{}", out.content);
        assert!(
            out.content.contains("/requestBody/required"),
            "{}",
            out.content
        );
        // Обратное направление (true → false) — ослабление, не ломающее.
        let out = diff_text(
            &contract("true"),
            &contract("false"),
            "old.yaml",
            "new.yaml",
        )
        .await;
        assert!(!out.content.contains("CD-010"), "{}", out.content);
        assert!(out.content.contains("breaking: 0"), "{}", out.content);
    }

    /// Д5: обязательный параметр, объявленный через компонент (`$ref`), виден
    /// диффу. Раньше такие параметры пропускались целиком — удаление
    /// обязательного параметра проходило молча.
    #[tokio::test]
    async fn required_ref_parameter_removed_is_breaking() {
        let contract = |params: &str| {
            format!(
                "openapi: 3.0.3\ninfo:\n  title: Wallets\n  version: 1.0.0\npaths:\n  /v1/topup:\n    post:\n      operationId: topup\n{params}      responses:\n        '200':\n          description: ok\ncomponents:\n  parameters:\n    IdempotencyKey:\n      name: Idempotency-Key\n      in: header\n      required: true\n      schema:\n        type: string\n"
            )
        };
        let with_ref = contract(
            "      parameters:\n        - $ref: '#/components/parameters/IdempotencyKey'\n",
        );
        let without = contract("");
        let out = diff_text(&with_ref, &without, "old.yaml", "new.yaml").await;
        assert!(!out.is_error, "{}", out.content);
        assert!(out.content.contains("CD-003"), "{}", out.content);
        assert!(
            out.content.contains("Idempotency-Key"),
            "параметр назван по имени из компонента: {}",
            out.content
        );
        // Необязательный параметр за `$ref` — по-прежнему не ломающий.
        let optional = with_ref.replace(
            "      required: true\n      schema:",
            "      required: false\n      schema:",
        );
        let out = diff_text(&optional, &without, "old.yaml", "new.yaml").await;
        assert!(!out.content.contains("CD-003"), "{}", out.content);
    }

    /// Д5: циклическая `$ref`-схема не зацикливает обход — потолок глубины
    /// [`SCHEMA_MAX_DEPTH`] возвращает ветку нераскрытой, а не падает и не
    /// висит. Проверка идёт по телу запроса, где рекурсия включена.
    #[tokio::test]
    async fn ref_cycle_is_bounded() {
        let contract = |required: &str| {
            format!(
                "openapi: 3.0.3\ninfo:\n  title: Wallets\n  version: 1.0.0\npaths:\n  /v1/nodes:\n    post:\n      operationId: addNode\n      requestBody:\n        content:\n          application/json:\n            schema:\n              $ref: '#/components/schemas/Node'\n      responses:\n        '200':\n          description: ok\ncomponents:\n  schemas:\n    Node:\n      type: object\n      required: {required}\n      properties:\n        name:\n          type: string\n        child:\n          $ref: '#/components/schemas/Node'\n"
            )
        };
        let out = diff_text(
            &contract("[name]"),
            &contract("[name, child]"),
            "old.yaml",
            "new.yaml",
        )
        .await;
        assert!(!out.is_error, "{}", out.content);
        assert!(out.content.contains("CD-008"), "{}", out.content);
        // Обход заканчивается: находки есть, но их число конечно и не растёт
        // экспоненциально (цикл раскрывается до потолка и молча встаёт).
        let count = out.content.matches("CD-008").count();
        assert!(
            (1..60).contains(&count),
            "обход ограничен потолком глубины, находок {count}: {}",
            out.content
        );
        // Потолок соблюдён: пути глубже SCHEMA_MAX_DEPTH сегментов не строится.
        let too_deep = "child.".repeat(SCHEMA_MAX_DEPTH + 1);
        assert!(
            !out.content.contains(&too_deep),
            "ветка глубже потолка не раскрывается: {}",
            out.content
        );
    }

    #[tokio::test]
    async fn cd001_removed_path_is_breaking() {
        let new = BASE.replace(PETS_PATH, "");
        let out = diff_text(BASE, &new, "old.yaml", "new.yaml").await;
        assert!(!out.is_error, "{}", out.content);
        assert!(
            out.content.contains("[error] #/paths/~1v1~1pets CD-001"),
            "{}",
            out.content
        );
        assert!(out.content.contains("Итог: FAIL"), "{}", out.content);
    }

    #[tokio::test]
    async fn cd005_added_path_is_warn_and_pass() {
        let old = BASE.replace(PETS_PATH, "");
        let out = diff_text(&old, BASE, "old.yaml", "new.yaml").await;
        assert!(!out.is_error, "{}", out.content);
        assert!(
            out.content.contains("[warn] #/paths/~1v1~1pets CD-005"),
            "{}",
            out.content
        );
        assert!(out.content.contains("Итог: PASS"), "{}", out.content);
    }

    #[tokio::test]
    async fn cd002_removed_operation_is_breaking() {
        let new = BASE.replace(POST_BLOCK, "");
        let out = diff_text(BASE, &new, "old.yaml", "new.yaml").await;
        assert!(!out.is_error, "{}", out.content);
        assert!(
            out.content
                .contains("[error] #/paths/~1v1~1pets/post CD-002"),
            "{}",
            out.content
        );
        assert!(out.content.contains("Итог: FAIL"), "{}", out.content);
    }

    #[tokio::test]
    async fn cd005_added_operation_is_warn_and_pass() {
        let old = BASE.replace(POST_BLOCK, "");
        let out = diff_text(&old, BASE, "old.yaml", "new.yaml").await;
        assert!(!out.is_error, "{}", out.content);
        assert!(
            out.content
                .contains("[warn] #/paths/~1v1~1pets/post CD-005"),
            "{}",
            out.content
        );
        assert!(out.content.contains("Итог: PASS"), "{}", out.content);
    }

    #[tokio::test]
    async fn cd003_removed_required_parameter_is_breaking() {
        let new = BASE.replace(REQUIRED_PARAM, "");
        let out = diff_text(BASE, &new, "old.yaml", "new.yaml").await;
        assert!(!out.is_error, "{}", out.content);
        assert!(
            out.content
                .contains("[error] #/paths/~1v1~1pets/post/parameters/idempotency-key CD-003"),
            "{}",
            out.content
        );
        assert!(out.content.contains("Итог: FAIL"), "{}", out.content);
    }

    #[tokio::test]
    async fn cd003_parameter_became_required_is_breaking() {
        let new = BASE.replace("required: false", "required: true");
        let out = diff_text(BASE, &new, "old.yaml", "new.yaml").await;
        assert!(!out.is_error, "{}", out.content);
        assert!(
            out.content
                .contains("[error] #/paths/~1v1~1pets/get/parameters/limit CD-003"),
            "{}",
            out.content
        );
        assert!(out.content.contains("Итог: FAIL"), "{}", out.content);
    }

    #[tokio::test]
    async fn cd005_added_optional_parameter_is_warn_and_pass() {
        let old = BASE.replace(LIMIT_PARAM, "");
        let out = diff_text(&old, BASE, "old.yaml", "new.yaml").await;
        assert!(!out.is_error, "{}", out.content);
        assert!(
            out.content
                .contains("[warn] #/paths/~1v1~1pets/get/parameters/limit CD-005"),
            "{}",
            out.content
        );
        assert!(out.content.contains("Итог: PASS"), "{}", out.content);
    }

    #[tokio::test]
    async fn removed_optional_parameter_is_not_flagged() {
        // Удаление необязательного параметра — вне скелета CD-001..CD-006.
        let new = BASE.replace(LIMIT_PARAM, "");
        let out = diff_text(BASE, &new, "old.yaml", "new.yaml").await;
        assert!(!out.is_error, "{}", out.content);
        assert!(!out.content.contains("CD-003"), "{}", out.content);
        assert!(out.content.contains("Итог: PASS"), "{}", out.content);
    }

    #[tokio::test]
    async fn cd004_removed_response_code_is_breaking() {
        let new = BASE.replace(NOT_FOUND_RESPONSE, "");
        let out = diff_text(BASE, &new, "old.yaml", "new.yaml").await;
        assert!(!out.is_error, "{}", out.content);
        assert!(
            out.content
                .contains("[error] #/paths/~1v1~1pets/get/responses/404 CD-004"),
            "{}",
            out.content
        );
        assert!(out.content.contains("Итог: FAIL"), "{}", out.content);
    }

    #[tokio::test]
    async fn cd005_added_response_code_is_warn_and_pass() {
        let old = BASE.replace(NOT_FOUND_RESPONSE, "");
        let out = diff_text(&old, BASE, "old.yaml", "new.yaml").await;
        assert!(!out.is_error, "{}", out.content);
        assert!(
            out.content
                .contains("[warn] #/paths/~1v1~1pets/get/responses/404 CD-005"),
            "{}",
            out.content
        );
        assert!(out.content.contains("Итог: PASS"), "{}", out.content);
    }

    #[tokio::test]
    async fn cd006_changed_schema_type_is_breaking() {
        let new = BASE.replace(
            "        age:\n          type: integer",
            "        age:\n          type: string",
        );
        let out = diff_text(BASE, &new, "old.yaml", "new.yaml").await;
        assert!(!out.is_error, "{}", out.content);
        assert!(
            out.content
                .contains("[error] #/components/schemas/Pet/properties/age CD-006"),
            "{}",
            out.content
        );
        assert!(out.content.contains("Итог: FAIL"), "{}", out.content);
    }

    /// CD-007 (п.14): breaking-дифф без смены major `info.version` — error;
    /// смена major узаконивает.
    #[tokio::test]
    async fn cd007_breaking_without_major_bump_is_error() {
        let new = BASE.replace(PETS_PATH, "");
        let out = diff_text(BASE, &new, "old.yaml", "new.yaml").await;
        assert!(
            out.content.contains("[error] #/info/version CD-007"),
            "{}",
            out.content
        );
        // Смена major (1.0.0 → 2.0.0): CD-007 не срабатывает, CD-001 остаётся.
        let new_v2 = new.replace("version: 1.0.0", "version: 2.0.0");
        let out = diff_text(BASE, &new_v2, "old.yaml", "new.yaml").await;
        assert!(!out.content.contains("CD-007"), "{}", out.content);
        assert!(out.content.contains("CD-001"), "{}", out.content);
        assert!(out.content.contains("Итог: FAIL"), "{}", out.content);
    }

    #[tokio::test]
    async fn mixed_json_and_yaml_are_compared() {
        let old = r#"{
  "openapi": "3.0.3",
  "info": {"title": "T", "version": "1.0.0"},
  "paths": {
    "/v1/pets": {
      "get": {
        "operationId": "listPets",
        "responses": {
          "200": {"description": "ok"},
          "404": {"description": "not found"}
        }
      }
    }
  }
}"#;
        let new = r"openapi: 3.0.3
info:
  title: T
  version: 2.0.0
paths:
  /v1/pets:
    get:
      operationId: listPets
      responses:
        '200':
          description: ok
";
        let out = diff_text(old, new, "old.json", "new.yaml").await;
        assert!(!out.is_error, "{}", out.content);
        assert!(
            out.content
                .contains("[error] #/paths/~1v1~1pets/get/responses/404 CD-004"),
            "{}",
            out.content
        );
        assert!(out.content.contains("Итог: FAIL"), "{}", out.content);
    }

    #[tokio::test]
    async fn non_openapi_file_errors() {
        let out = diff_text(
            "title: hello\nversion: 1.0.0\n",
            BASE,
            "old.yaml",
            "new.yaml",
        )
        .await;
        assert!(out.is_error, "{}", out.content);
        assert!(out.content.contains("не OpenAPI 3.x"), "{}", out.content);
    }

    #[tokio::test]
    async fn missing_file_errors() {
        let dir = tempfile::tempdir().expect("tmp");
        let ctx = ToolContext::new(
            dir.path().to_path_buf(),
            Arc::new(crate::config::Config::default()),
        );
        let out = tools()[0]
            .call(
                json!({"old": "absent.yaml", "new": "also-absent.yaml"}),
                &ctx,
            )
            .await
            .expect("вызов contract_diff");
        assert!(out.is_error, "{}", out.content);
        assert!(out.content.contains("contract_diff"), "{}", out.content);
    }

    // -----------------------------------------------------------------
    // protobuf/gRPC (.proto), CD-P01..CD-P06
    // -----------------------------------------------------------------

    /// Proto-эталон v1: package, два сообщения (одно вложенное), сервис.
    const PROTO_V1: &str = r#"syntax = "proto3";

package acme.payments.v1;

// Запрос списания.
message ChargeRequest {
  string id = 1;
  int64 amount_minor = 2;
  optional string currency = 3;
  Address billing = 4;

  message Address {
    string city = 1;
  }
}

message ChargeResponse {
  string status = 1;
}

service Charging {
  rpc Charge (ChargeRequest) returns (ChargeResponse);
  rpc Refund (ChargeRequest) returns (ChargeResponse);
}
"#;

    /// Прогон proto-диффа через инструмент.
    async fn diff_proto_text(old: &str, new: &str) -> ToolOutput {
        diff_text(old, new, "old.proto", "new.proto").await
    }

    #[tokio::test]
    async fn proto_identical_passes_clean() {
        let out = diff_proto_text(PROTO_V1, PROTO_V1).await;
        assert!(!out.is_error, "{}", out.content);
        assert!(
            out.content.contains("contract_diff: 0 изменений"),
            "{}",
            out.content
        );
        assert!(out.content.contains("Формат: proto"), "{}", out.content);
        assert!(out.content.contains("Итог: PASS"), "{}", out.content);
    }

    #[tokio::test]
    async fn proto_removed_field_and_rpc_are_breaking() {
        let new = PROTO_V1
            .replace("  optional string currency = 3;\n", "")
            .replace(
                "  rpc Refund (ChargeRequest) returns (ChargeResponse);\n",
                "",
            );
        let out = diff_proto_text(PROTO_V1, &new).await;
        assert!(!out.is_error, "{}", out.content);
        assert!(
            out.content
                .contains("[error] #/proto/message/ChargeRequest/field/3 CD-P02"),
            "{}",
            out.content
        );
        assert!(
            out.content
                .contains("[error] #/proto/service/Charging/rpc/Refund CD-P04"),
            "{}",
            out.content
        );
        // Ломающий дифф, пакет остался v1 → CD-P06.
        assert!(
            out.content.contains("[error] #/proto/package CD-P06"),
            "{}",
            out.content
        );
        assert!(out.content.contains("Итог: FAIL"), "{}", out.content);
    }

    #[tokio::test]
    async fn proto_reserved_removal_is_warn_not_breaking() {
        let new = PROTO_V1.replace(
            "  optional string currency = 3;\n",
            "  reserved 3;\n  reserved \"currency\";\n",
        );
        let out = diff_proto_text(PROTO_V1, &new).await;
        assert!(!out.is_error, "{}", out.content);
        assert!(out.content.contains("CD-P05"), "{}", out.content);
        assert!(!out.content.contains("CD-P02"), "{}", out.content);
        assert!(!out.content.contains("CD-P06"), "{}", out.content);
        assert!(out.content.contains("Итог: PASS"), "{}", out.content);
    }

    #[tokio::test]
    async fn proto_type_change_and_renumbering_are_breaking_major_bump_clears_p06() {
        // Смена типа тега 2 (int64 → string) и перенумерация currency 3 → 5.
        let new = PROTO_V1
            .replace("int64 amount_minor = 2;", "string amount_minor = 2;")
            .replace(
                "optional string currency = 3;",
                "optional string currency = 5;",
            )
            .replace("package acme.payments.v1;", "package acme.payments.v2;");
        let out = diff_proto_text(PROTO_V1, &new).await;
        assert!(!out.is_error, "{}", out.content);
        assert!(out.content.contains("CD-P03"), "{}", out.content);
        assert!(out.content.contains("перенумеровано"), "{}", out.content);
        // major поднят (v1 → v2) — CD-P06 молчит.
        assert!(!out.content.contains("CD-P06"), "{}", out.content);
        assert!(out.content.contains("Итог: FAIL"), "{}", out.content);
    }

    #[tokio::test]
    async fn proto_added_field_and_rpc_are_warn() {
        let new = PROTO_V1
            .replace(
                "service Charging {",
                "service Charging {\n  rpc Status (ChargeRequest) returns (ChargeResponse);",
            )
            .replace(
                "message ChargeResponse {",
                "message ChargeResponse {\n  string receipt_id = 2;",
            );
        let out = diff_proto_text(PROTO_V1, &new).await;
        assert!(!out.is_error, "{}", out.content);
        assert!(out.content.contains("CD-P05"), "{}", out.content);
        assert!(out.content.contains("Итог: PASS"), "{}", out.content);
    }

    #[tokio::test]
    async fn proto_removed_message_is_breaking() {
        let new = PROTO_V1.replace("message ChargeResponse {\n  string status = 1;\n}\n\n", "");
        // rpc возвращают ChargeResponse — тип резолвится позже; для диффа
        // важно только удаление сообщения.
        let out = diff_proto_text(PROTO_V1, &new).await;
        assert!(
            out.content
                .contains("[error] #/proto/message/ChargeResponse CD-P01"),
            "{}",
            out.content
        );
        assert!(out.content.contains("Итог: FAIL"), "{}", out.content);
    }

    // -----------------------------------------------------------------
    // Avro (.avsc), CD-A01..CD-A05
    // -----------------------------------------------------------------

    const AVRO_V1: &str = r#"{
  "type": "record",
  "name": "ChargeEvent",
  "namespace": "acme.payments",
  "fields": [
    {"name": "id", "type": "string"},
    {"name": "amount_minor", "type": "long"},
    {"name": "note", "type": ["null", "string"], "default": null}
  ]
}"#;

    async fn diff_avro_text(old: &str, new: &str) -> ToolOutput {
        diff_text(old, new, "old.avsc", "new.avsc").await
    }

    #[tokio::test]
    async fn avro_identical_passes_clean() {
        let out = diff_avro_text(AVRO_V1, AVRO_V1).await;
        assert!(!out.is_error, "{}", out.content);
        assert!(out.content.contains("Формат: avro"), "{}", out.content);
        assert!(out.content.contains("Итог: PASS"), "{}", out.content);
    }

    #[tokio::test]
    async fn avro_removed_field_without_default_is_breaking() {
        let new = AVRO_V1.replace(
            "    {\"name\": \"amount_minor\", \"type\": \"long\"},\n",
            "",
        );
        let out = diff_avro_text(AVRO_V1, &new).await;
        assert!(out.content.contains("CD-A02"), "{}", out.content);
        assert!(out.content.contains("Итог: FAIL"), "{}", out.content);
        // Удаление поля С default — warn (CD-A05).
        let new2 = AVRO_V1.replace(
            "    {\"name\": \"note\", \"type\": [\"null\", \"string\"], \"default\": null}\n",
            "",
        );
        // Запятую после amount_minor чиним, чтобы JSON оставался валидным.
        let new2 = new2.replace(
            "    {\"name\": \"amount_minor\", \"type\": \"long\"},\n",
            "    {\"name\": \"amount_minor\", \"type\": \"long\"}\n",
        );
        let out = diff_avro_text(AVRO_V1, &new2).await;
        assert!(out.content.contains("CD-A05"), "{}", out.content);
        assert!(!out.content.contains("CD-A02"), "{}", out.content);
        assert!(out.content.contains("Итог: PASS"), "{}", out.content);
    }

    #[tokio::test]
    async fn avro_added_field_without_default_is_breaking_with_default_warn() {
        let new = AVRO_V1.replace(
            "    {\"name\": \"note\", \"type\": [\"null\", \"string\"], \"default\": null}",
            "    {\"name\": \"note\", \"type\": [\"null\", \"string\"], \"default\": null},\n    {\"name\": \"trace_id\", \"type\": \"string\"}",
        );
        let out = diff_avro_text(AVRO_V1, &new).await;
        assert!(out.content.contains("CD-A04"), "{}", out.content);
        assert!(out.content.contains("Итог: FAIL"), "{}", out.content);
        let new = AVRO_V1.replace(
            "    {\"name\": \"note\", \"type\": [\"null\", \"string\"], \"default\": null}",
            "    {\"name\": \"note\", \"type\": [\"null\", \"string\"], \"default\": null},\n    {\"name\": \"trace_id\", \"type\": \"string\", \"default\": \"\"}",
        );
        let out = diff_avro_text(AVRO_V1, &new).await;
        assert!(out.content.contains("CD-A05"), "{}", out.content);
        assert!(!out.content.contains("CD-A04"), "{}", out.content);
        assert!(out.content.contains("Итог: PASS"), "{}", out.content);
    }

    #[tokio::test]
    async fn avro_type_change_incompatible_breaking_promotion_warn() {
        // long → string: несовместимо.
        let new = AVRO_V1.replace(
            "{\"name\": \"amount_minor\", \"type\": \"long\"}",
            "{\"name\": \"amount_minor\", \"type\": \"string\"}",
        );
        let out = diff_avro_text(AVRO_V1, &new).await;
        assert!(out.content.contains("CD-A03"), "{}", out.content);
        assert!(out.content.contains("Итог: FAIL"), "{}", out.content);
        // Расширение union ["null","string"] → ["null","string","long"]: warn.
        let new = AVRO_V1.replace("[\"null\", \"string\"]", "[\"null\", \"long\", \"string\"]");
        let out = diff_avro_text(AVRO_V1, &new).await;
        assert!(out.content.contains("CD-A05"), "{}", out.content);
        assert!(!out.content.contains("CD-A03"), "{}", out.content);
        assert!(out.content.contains("Итог: PASS"), "{}", out.content);
    }

    // -----------------------------------------------------------------
    // JSON Schema, CD-J01..CD-J05
    // -----------------------------------------------------------------

    const JSCHEMA_V1: &str = r#"{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "type": "object",
  "properties": {
    "id": {"type": "string"},
    "amount": {"type": ["integer", "null"]},
    "meta": {
      "type": "object",
      "properties": {
        "channel": {"type": "string"}
      },
      "required": ["channel"]
    }
  },
  "required": ["id", "amount"]
}"#;

    async fn diff_js_text(old: &str, new: &str) -> ToolOutput {
        diff_text(old, new, "old.json", "new.json").await
    }

    #[tokio::test]
    async fn jsonschema_identical_passes_clean() {
        let out = diff_js_text(JSCHEMA_V1, JSCHEMA_V1).await;
        assert!(!out.is_error, "{}", out.content);
        assert!(
            out.content.contains("Формат: jsonschema"),
            "{}",
            out.content
        );
        assert!(out.content.contains("Итог: PASS"), "{}", out.content);
    }

    #[tokio::test]
    async fn jsonschema_removed_property_and_required_relaxation() {
        // Удалено свойство amount — CD-J01 error.
        let new = JSCHEMA_V1
            .replace("    \"amount\": {\"type\": [\"integer\", \"null\"]},\n", "")
            .replace(
                "\"required\": [\"id\", \"amount\"]",
                "\"required\": [\"id\"]",
            );
        let out = diff_js_text(JSCHEMA_V1, &new).await;
        assert!(
            out.content.contains("[error] #/properties/amount CD-J01"),
            "{}",
            out.content
        );
        assert!(out.content.contains("Итог: FAIL"), "{}", out.content);

        // amount оставлен, но выведен из required — CD-J04 warn (ослабление).
        let new = JSCHEMA_V1.replace(
            "\"required\": [\"id\", \"amount\"]",
            "\"required\": [\"id\"]",
        );
        let out = diff_js_text(JSCHEMA_V1, &new).await;
        assert!(out.content.contains("CD-J04"), "{}", out.content);
        assert!(out.content.contains("Итог: PASS"), "{}", out.content);
    }

    #[tokio::test]
    async fn jsonschema_type_narrowed_breaking_widened_warn() {
        // ["integer","null"] → ["integer"]: сужение — error.
        let new = JSCHEMA_V1.replace("\"type\": [\"integer\", \"null\"]", "\"type\": \"integer\"");
        let out = diff_js_text(JSCHEMA_V1, &new).await;
        assert!(out.content.contains("CD-J03"), "{}", out.content);
        assert!(out.content.contains("Итог: FAIL"), "{}", out.content);
        // "string" → ["string","null"]: расширение — warn.
        let new = JSCHEMA_V1.replace(
            "\"id\": {\"type\": \"string\"}",
            "\"id\": {\"type\": [\"null\", \"string\"]}",
        );
        let out = diff_js_text(JSCHEMA_V1, &new).await;
        assert!(out.content.contains("CD-J05"), "{}", out.content);
        assert!(!out.content.contains("CD-J03"), "{}", out.content);
        assert!(out.content.contains("Итог: PASS"), "{}", out.content);
    }

    #[tokio::test]
    async fn jsonschema_nested_and_added_required_property() {
        // Вложенное свойство meta.channel удалено (вместе с его required) —
        // находка по вложенному пути.
        let new = JSCHEMA_V1.replace(
            "      \"properties\": {\n        \"channel\": {\"type\": \"string\"}\n      },\n      \"required\": [\"channel\"]\n",
            "      \"properties\": {}\n",
        );
        let out = diff_js_text(JSCHEMA_V1, &new).await;
        assert!(
            out.content
                .contains("[error] #/properties/meta/properties/channel CD-J01"),
            "{}",
            out.content
        );
        // Добавлено сразу обязательное свойство — CD-J02 error.
        let new = JSCHEMA_V1
            .replace(
                "    \"id\": {\"type\": \"string\"},",
                "    \"id\": {\"type\": \"string\"},\n    \"trace_id\": {\"type\": \"string\"},",
            )
            .replace(
                "\"required\": [\"id\", \"amount\"]",
                "\"required\": [\"id\", \"amount\", \"trace_id\"]",
            );
        let out = diff_js_text(JSCHEMA_V1, &new).await;
        assert!(out.content.contains("CD-J02"), "{}", out.content);
        assert!(out.content.contains("Итог: FAIL"), "{}", out.content);
    }

    // -----------------------------------------------------------------
    // DDL-миграции (.sql), CD-S01..CD-S05
    // -----------------------------------------------------------------

    const DDL_V1: &str = "CREATE TABLE charges (
  id varchar(36) NOT NULL,
  amount_minor bigint NOT NULL,
  note text
);

ALTER TABLE charges ADD COLUMN currency varchar(3);
";

    async fn diff_ddl_text(old: &str, new: &str) -> ToolOutput {
        diff_text(old, new, "old.sql", "new.sql").await
    }

    #[tokio::test]
    async fn ddl_identical_passes_clean() {
        let out = diff_ddl_text(DDL_V1, DDL_V1).await;
        assert!(!out.is_error, "{}", out.content);
        assert!(out.content.contains("Формат: ddl"), "{}", out.content);
        assert!(out.content.contains("Итог: PASS"), "{}", out.content);
    }

    #[tokio::test]
    async fn ddl_drop_column_and_table_are_breaking() {
        // Колонка note удалена из CREATE TABLE.
        let new = DDL_V1.replace(",\n  note text\n", "\n");
        let out = diff_ddl_text(DDL_V1, &new).await;
        assert!(out.content.contains("CD-S02"), "{}", out.content);
        assert!(out.content.contains("Итог: FAIL"), "{}", out.content);

        let new = "DROP TABLE charges;\n";
        let out = diff_ddl_text(DDL_V1, new).await;
        assert!(
            out.content.contains("[error] #/ddl/table/charges CD-S01"),
            "{}",
            out.content
        );
    }

    #[tokio::test]
    async fn ddl_not_null_without_default_breaking_with_default_warn() {
        // Существующая nullable-колонка стала NOT NULL без DEFAULT — error.
        let new = DDL_V1.replace("  note text\n", "  note text NOT NULL\n");
        let out = diff_ddl_text(DDL_V1, &new).await;
        assert!(out.content.contains("CD-S04"), "{}", out.content);
        assert!(out.content.contains("Итог: FAIL"), "{}", out.content);
        // С DEFAULT — warn.
        let new = DDL_V1.replace("  note text\n", "  note text NOT NULL DEFAULT ''\n");
        let out = diff_ddl_text(DDL_V1, &new).await;
        assert!(!out.content.contains("CD-S04"), "{}", out.content);
        assert!(out.content.contains("Итог: PASS"), "{}", out.content);
        // Новая обязательная колонка без DEFAULT — error.
        let new =
            format!("{DDL_V1}\nALTER TABLE charges ADD COLUMN trace_id varchar(36) NOT NULL;\n");
        let out = diff_ddl_text(DDL_V1, &new).await;
        assert!(out.content.contains("CD-S04"), "{}", out.content);
    }

    #[tokio::test]
    async fn ddl_type_change_incompatible_breaking_widening_warn() {
        // varchar(36) → int: несовместимо.
        let new = DDL_V1.replace("id varchar(36) NOT NULL", "id int NOT NULL");
        let out = diff_ddl_text(DDL_V1, &new).await;
        assert!(out.content.contains("CD-S03"), "{}", out.content);
        assert!(out.content.contains("Итог: FAIL"), "{}", out.content);
        // varchar(36) → varchar(64) и ALTER TYPE bigint→bigint через alter.
        let new = DDL_V1
            .replace("id varchar(36) NOT NULL", "id varchar(64) NOT NULL")
            .replace(
                "ADD COLUMN currency varchar(3);",
                "ADD COLUMN currency varchar(8);",
            );
        let out = diff_ddl_text(DDL_V1, &new).await;
        assert!(!out.content.contains("CD-S03"), "{}", out.content);
        assert!(out.content.contains("Итог: PASS"), "{}", out.content);
        // ALTER COLUMN TYPE через отдельный оператор (int → bigint — warn).
        let new = format!(
            "{DDL_V1}\nALTER TABLE charges ALTER COLUMN amount_minor SET DATA TYPE numeric(20,0);\n"
        );
        let out = diff_ddl_text(DDL_V1, &new).await;
        assert!(!out.content.contains("CD-S03"), "{}", out.content);
    }

    // -----------------------------------------------------------------
    // Детектор формата и связка с моделью (ADR-035)
    // -----------------------------------------------------------------

    #[tokio::test]
    async fn format_mismatch_and_bad_format_name_error() {
        // old — proto, new — openapi: разные форматы.
        let out = diff_text(PROTO_V1, BASE, "old.proto", "new.yaml").await;
        assert!(out.is_error, "{}", out.content);
        assert!(
            out.content.contains("форматы различаются"),
            "{}",
            out.content
        );
        // Неизвестное имя формата.
        let dir = tempfile::tempdir().expect("tmp");
        std::fs::write(dir.path().join("a.proto"), PROTO_V1).expect("a");
        std::fs::write(dir.path().join("b.proto"), PROTO_V1).expect("b");
        let ctx = ToolContext::new(
            dir.path().to_path_buf(),
            Arc::new(crate::config::Config::default()),
        );
        let out = tools()[0]
            .call(
                json!({"old": "a.proto", "new": "b.proto", "format": "xml"}),
                &ctx,
            )
            .await
            .expect("вызов");
        assert!(out.is_error, "{}", out.content);
        assert!(
            out.content.contains("неизвестный формат"),
            "{}",
            out.content
        );
        // Явный format=proto на proto-файлах работает.
        let out = tools()[0]
            .call(
                json!({"old": "a.proto", "new": "b.proto", "format": "proto"}),
                &ctx,
            )
            .await
            .expect("вызов");
        assert!(!out.is_error, "{}", out.content);
        assert!(out.content.contains("Формат: proto"), "{}", out.content);
    }

    /// Связка с моделью: INT-001 несёт `contract: contracts/pay.proto`;
    /// ломающий дифф возвращает потребителей (CMP/SYS) и владельцев.
    #[tokio::test]
    async fn model_linkage_returns_consumers_and_owners() {
        let dir = tempfile::tempdir().expect("tmp");
        let case = dir.path().join("case");
        let model = case.join("model");
        std::fs::create_dir_all(&model).expect("mkdir model");
        for (name, fm) in [
            (
                "AD-1.md",
                "---\nid: AD-1\ntype: ad\ntitle: Контракты\nstatus: ADOPTED\nverified_by: [C-001]\n---\n\nПравило.\n",
            ),
            (
                "CMP-001.md",
                "---\nid: CMP-001\ntype: cmp\ntitle: Платёжный шлюз\nstatus: designed\nimplements: [AD-1]\ndepends_on: [INT-001]\n---\n\nТело.\n",
            ),
            (
                "INT-001.md",
                "---\nid: INT-001\ntype: int\ntitle: Рельс процессинга\nstatus: accepted\ncontract: contracts/pay.proto\naffects: [OWNER-1]\n---\n\nТело.\n",
            ),
            (
                "OWNER-1.md",
                "---\nid: OWNER-1\ntype: owner\ntitle: Команда процессинга\nstatus: active\n---\n\nТело.\n",
            ),
        ] {
            std::fs::write(model.join(name), fm).expect("сущность");
        }
        std::fs::write(
            case.join("CONSTRAINTS.yaml"),
            "constraints:\n  - id: C-001\n    name: contract_review\n    owner: Команда платежей\n",
        )
        .expect("constraints");
        let contracts = case.join("contracts");
        std::fs::create_dir_all(&contracts).expect("mkdir contracts");
        std::fs::write(contracts.join("old.proto"), PROTO_V1).expect("old");
        let new_proto = PROTO_V1.replace("  optional string currency = 3;\n", "");
        std::fs::write(contracts.join("new.proto"), new_proto).expect("new");

        let ctx = ToolContext::new(
            dir.path().to_path_buf(),
            Arc::new(crate::config::Config::default()),
        );
        let out = tools()[0]
            .call(
                json!({
                    "old": "case/contracts/old.proto",
                    "new": "case/contracts/new.proto",
                    "model": "case",
                }),
                &ctx,
            )
            .await
            .expect("вызов");
        assert!(!out.is_error, "{}", out.content);
        assert!(out.content.contains("CD-P02"), "{}", out.content);
        assert!(out.content.contains("Итог: FAIL"), "{}", out.content);
        // Impact-секция: INT-001 совпал (путь old/new не равен contract
        // строкой — сверка по имени файла? нет: contract=contracts/pay.proto,
        // а дифф по old.proto/new.proto — совпадения НЕТ, gap).
        assert!(out.content.contains("Связь с моделью"), "{}", out.content);
        // Теперь настоящее совпадение: contract указывает на new.proto.
        std::fs::write(
            model.join("INT-001.md"),
            "---\nid: INT-001\ntype: int\ntitle: Рельс процессинга\nstatus: accepted\ncontract: contracts/new.proto\naffects: [OWNER-1]\n---\n\nТело.\n",
        )
        .expect("INT с совпадающим contract");
        let out = tools()[0]
            .call(
                json!({
                    "old": "case/contracts/old.proto",
                    "new": "case/contracts/new.proto",
                    "model": "case",
                }),
                &ctx,
            )
            .await
            .expect("вызов");
        assert!(!out.is_error, "{}", out.content);
        assert!(out.content.contains("INT-001"), "{}", out.content);
        assert!(
            out.content.contains("CMP-001 · Платёжный шлюз"),
            "{}",
            out.content
        );
        assert!(
            out.content.contains("OWNER-1 · Команда процессинга"),
            "{}",
            out.content
        );
        assert!(out.content.contains("C-001"), "{}", out.content);
    }
}
