//! Дифф Avro (`.avsc`, CD-A01..CD-A05): record'ы с вложенностью,
//! промоушены типов по таблице Avro (`int→long→float→double`).

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde_json::Value;

use crate::error::{HarnessError, Result};

use super::diffbase::parse_contract;
use super::types::{Finding, escape_segment};

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
pub(crate) fn diff_avro(
    old_text: &str,
    new_text: &str,
    old: &Path,
    new: &Path,
) -> Result<Vec<Finding>> {
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

#[cfg(test)]
mod tests {
    use crate::contract_diff::testkit::diff_text;
    use crate::tool::ToolOutput;

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
}
