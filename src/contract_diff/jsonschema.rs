//! Дифф JSON Schema (топики/тела сообщений, CD-J01..CD-J05):
//! свойства/`required`/тип, рекурсия по вложенным `properties` с потолком
//! [`MAX_JSONSCHEMA_DEPTH`]; `$ref` не резолвится.

use std::collections::BTreeSet;
use std::path::Path;

use serde_json::Value;

use crate::error::{HarnessError, Result};

use super::detect::is_json_schema;
use super::diffbase::parse_contract;
use super::types::{Finding, escape_segment};

/// Потолок рекурсии по вложенным `properties` JSON Schema (защита от
/// патологически глубоких схем; `$ref` всё равно не резолвится).
const MAX_JSONSCHEMA_DEPTH: usize = 16;

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
pub(crate) fn diff_jsonschema(
    old_text: &str,
    new_text: &str,
    old: &Path,
    new: &Path,
) -> Result<Vec<Finding>> {
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

#[cfg(test)]
mod tests {
    use crate::contract_diff::testkit::diff_text;
    use crate::tool::ToolOutput;

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
}
