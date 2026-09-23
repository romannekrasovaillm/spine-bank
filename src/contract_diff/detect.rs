//! Детектор формата контракта: расширение файла, затем содержимое
//! (маркерные поля JSON/YAML, текстовые маркеры proto/DDL).

use std::path::Path;

use serde_json::Value;

use crate::error::{HarnessError, Result};

use super::types::ContractFormat;

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
pub(crate) fn is_json_schema(doc: &Value) -> bool {
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
