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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract_diff::types::ContractFormat;

    fn detect(name: &str, content: &str) -> Result<ContractFormat> {
        detect_format(Path::new(name), content)
    }

    /// Расширение решает раньше содержимого — включая файлы, чьё содержимое
    /// выглядело бы как другой формат.
    #[test]
    fn extension_wins_over_content() {
        assert_eq!(
            detect("s.proto", "{\"openapi\": \"3.0.0\"}").expect("proto"),
            ContractFormat::Proto
        );
        assert_eq!(
            detect("s.avsc", "CREATE TABLE t (id int);").expect("avro"),
            ContractFormat::Avro
        );
        assert_eq!(
            detect("s.sql", "{\"properties\": {}}").expect("ddl"),
            ContractFormat::Ddl
        );
    }

    /// `syntax` без слова proto — не proto: иначе строка «syntax = "avro"»
    /// уводила бы в protobuf-дифф.
    #[test]
    fn syntax_without_proto_word_is_not_proto() {
        let err = detect("schema.txt", "syntax = \"avro\";\n").expect_err("не формат");
        assert!(err.to_string().contains("не удалось определить формат"));
    }

    /// Каждое DDL-слово проверяется отдельно: CREATE, ALTER и DROP —
    /// независимые признаки, а не пара.
    #[test]
    fn ddl_keywords_are_recognized_individually() {
        for text in [
            "CREATE TABLE t (id int);",
            "ALTER TABLE t ADD COLUMN name text;",
            "DROP TABLE t;",
            "create table t (id int);",
        ] {
            assert_eq!(
                detect("a.txt", text).expect("ddl"),
                ContractFormat::Ddl,
                "{text}"
            );
        }
    }

    /// Avro — ровно `type: record` **и** `fields`; ни одного из двух мало.
    #[test]
    fn avro_needs_type_record_and_fields() {
        assert_eq!(
            detect("a.json", "{\"type\": \"record\", \"fields\": []}").expect("avro"),
            ContractFormat::Avro
        );
        // type: record без fields — не Avro (JSON Schema по ключу type).
        assert_eq!(
            detect("a.json", "{\"type\": \"record\"}").expect("jsonschema"),
            ContractFormat::JsonSchema
        );
        // fields без type: record — тоже не Avro.
        assert!(detect("a.json", "{\"fields\": []}").is_err());
        // type: string с fields — не Avro.
        assert_eq!(
            detect("a.json", "{\"type\": \"string\", \"fields\": []}").expect("jsonschema"),
            ContractFormat::JsonSchema
        );
    }

    /// `$schema` опознаётся по любой из двух половин: «json-schema» и «draft».
    #[test]
    fn json_schema_uri_markers_are_independent() {
        assert_eq!(
            detect(
                "s.json",
                "{\"$schema\": \"https://json-schema.org/draft/2020-12/schema\"}"
            )
            .expect("json-schema"),
            ContractFormat::JsonSchema
        );
        assert_eq!(
            detect("s.json", "{\"$schema\": \"http://draft.example/7\"}").expect("draft"),
            ContractFormat::JsonSchema
        );
        assert_eq!(
            detect("s.json", "{\"$schema\": \"http://x/json-schema/v1\"}").expect("json-schema"),
            ContractFormat::JsonSchema
        );
    }

    /// Каждый типовой ключ схемы опознаётся сам по себе.
    #[test]
    fn json_schema_marker_keys_are_independent() {
        for text in [
            "{\"properties\": {\"id\": {\"type\": \"string\"}}}",
            "{\"required\": [\"id\"]}",
            "{\"type\": \"object\"}",
            "{\"items\": {\"type\": \"string\"}}",
        ] {
            assert_eq!(
                detect("s.json", text).expect("json-schema"),
                ContractFormat::JsonSchema,
                "{text}"
            );
        }
    }

    /// Текстовые маркеры proto: каждое из слов message/service/package само
    /// по себе достаточно (содержимое при этом не парсится как JSON/YAML).
    #[test]
    fn markers_recognize_each_proto_keyword() {
        for text in [
            "{\n  message Foo {\n",
            "{\n  service Payments {\n",
            "{\n  package acme.payments;\n",
        ] {
            assert_eq!(
                detect("x.txt", text).expect("proto"),
                ContractFormat::Proto,
                "{text}"
            );
        }
    }

    /// Маркеры `OpenAPI`: подстрока `"openapi"` в битом JSON и строка
    /// `openapi:` в битом YAML — независимые признаки.
    #[test]
    fn markers_recognize_openapi_variants() {
        assert_eq!(
            detect("x.txt", "{\n  \"openapi\": 3").expect("openapi-подстрока"),
            ContractFormat::OpenApi
        );
        assert_eq!(
            detect("x.yaml", "openapi: 3.0.0\n  bad: [\n").expect("openapi-строка"),
            ContractFormat::OpenApi
        );
    }

    /// `AsyncAPI` отвергается с объяснением: `contract_diff` его не сравнивает.
    #[test]
    fn asyncapi_is_rejected_with_reason() {
        let err = detect("a.yaml", "asyncapi: 2.6.0\ninfo:\n  title: t\n").expect_err("asyncapi");
        let text = err.to_string();
        assert!(text.contains("AsyncAPI"), "{text}");
        assert!(text.contains("asyncapi_lint"), "{text}");
    }

    /// Валидный YAML с полем openapi идёт по разбору документа, а не по маркерам.
    #[test]
    fn valid_yaml_openapi_document_is_detected() {
        assert_eq!(
            detect("a.yaml", "openapi: 3.0.0\ninfo:\n  title: t\npaths: {}\n").expect("openapi"),
            ContractFormat::OpenApi
        );
    }

    /// Неопознанное содержимое — ошибка со списком проверенных признаков и
    /// подсказкой задать формат явно.
    #[test]
    fn unknown_content_lists_checked_markers() {
        let err = detect("z.txt", "просто текст без маркеров").expect_err("не формат");
        let text = err.to_string();
        assert!(text.contains("z.txt"), "{text}");
        assert!(text.contains("задайте format явно"), "{text}");
        assert!(text.contains("CREATE/ALTER/DROP"), "{text}");
    }

    /// Битый JSON уходит на текстовые маркеры, а не падает с ошибкой парсера.
    #[test]
    fn broken_json_falls_back_to_markers() {
        assert_eq!(
            detect("b.json", "{ \"openapi\": ").expect("маркер"),
            ContractFormat::OpenApi
        );
        // Битый JSON без маркеров — честный отказ.
        assert!(detect("b.json", "{ \"foo\": ").is_err());
    }
}
