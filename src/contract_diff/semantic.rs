//! Семантические зонды контракта (`--semantic`, AB-12): смысловые сдвиги,
//! невидимые структурному диффу (CD-001..CD-010) — изменение значений
//! `enum`, единиц измерения в имени поля, `default` и ключа
//! партиционирования. Зонды аддитивны (`contract-diff/v1` не меняется),
//! включаются флагом и для формата `OpenAPI` (основной формат AB-12).
//!
//! Правила (все — семантический список `semantic_changes`):
//! - `semantic_enum_narrowed` (error): значение удалено из `enum` — потребитель
//!   со старым значением ломается;
//! - `semantic_enum_widened` (warn): значение добавлено в `enum` — расширение;
//! - `semantic_unit_changed` (error): поле с маркером единицы в имени
//!   переименовано со сменой семейства единиц (`amount_cents` →
//!   `amount_rubles`) или потеряло маркер (`amount_cents` → `amount`) —
//!   «потребители парсят по имени»; смена единицы внутри одного семейства
//!   (`cents` → `kopecks`) допустима и находки не даёт;
//! - `semantic_default_shift` (warn): `default` существующего поля изменён
//!   или снят;
//! - `semantic_partition_key_changed` (warn): ключ партиционирования
//!   (`…partitionKey`/`…partitionId`) переименован/удалён — потребители,
//!   читающие по ключу, ломаются.
//!
//! Смена обязательности поля — механика CD (CD-003/CD-008/CD-010), зонд её
//! НЕ дублирует (проверяется тестом, что `semantic_changes` пуст).
//!
//! Потребители семантически изменённых полей ищутся упрощённо: сканом файлов
//! корня кейса (метка `[inferred]`, без гарантий — модель/граф точнее, если
//! задан `model`).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::error::Result;

use super::openapi::read_openapi;
use super::types::{Finding, location};

/// Число семантических зондов (ADR-079: enum, единица измерения, `default`,
/// ключ партиционирования). Диагностика 5 (0.4.7): рендер честно печатает
/// «применено N, сигналов 0», отличая «зонд не искали» от «искали и чисто».
pub(crate) const PROBE_COUNT: usize = 4;

/// Методы операций `OpenAPI` (дублирует приватный список openapi.rs — зонд
/// самостоятелен).
const OPERATION_METHODS: [&str; 8] = [
    "get", "put", "post", "delete", "options", "head", "patch", "trace",
];

/// Каталоги, не сканируемые в поиске потребителей поля.
const SKIP_DIRS: [&str; 8] = [
    ".git",
    "target",
    "node_modules",
    "dist",
    "build",
    ".venv",
    "venv",
    "__pycache__",
];

/// Потолок глубины обхода корня кейса.
const MAX_SCAN_DEPTH: usize = 6;
/// Потолок числа просмотренных файлов.
const MAX_SCAN_FILES: usize = 2000;
/// Потолок размера файла-кандидата в потребители.
const MAX_FILE_BYTES: u64 = 512 * 1024;
/// Потолок числа найденных потребителей.
const MAX_CONSUMERS: usize = 15;

/// Семейства единиц по суффиксу имени поля: длинные суффиксы раньше коротких.
const UNIT_TABLE: &[(&str, &str)] = &[
    // деньги: младшие единицы (центы/копейки — одно семейство)
    ("_cents", "money_minor"),
    ("_cent", "money_minor"),
    ("_kopecks", "money_minor"),
    ("_kopek", "money_minor"),
    ("_minor", "money_minor"),
    // деньги: старшие единицы
    ("_rubles", "money_major"),
    ("_roubles", "money_major"),
    ("_rub", "money_major"),
    ("_major", "money_major"),
    // время
    ("_millis", "time_ms"),
    ("_ms", "time_ms"),
    ("_nanos", "time_ns"),
    ("_ns", "time_ns"),
    ("_micros", "time_us"),
    ("_seconds", "time_s"),
    ("_secs", "time_s"),
    ("_sec", "time_s"),
    ("_s", "time_s"),
    // объём
    ("_bytes", "bytes"),
    ("_byte", "bytes"),
];

/// Семейство единиц по имени поля (`None` — маркера нет).
fn unit_family(name: &str) -> Option<&'static str> {
    let lower = name.to_ascii_lowercase();
    UNIT_TABLE
        .iter()
        .find(|(suf, _)| lower.ends_with(suf))
        .map(|(_, fam)| *fam)
}

/// Основа имени без маркера единицы (совпадает с именем, если маркера нет).
fn unit_base(name: &str) -> String {
    let lower = name.to_ascii_lowercase();
    match UNIT_TABLE.iter().find(|(suf, _)| lower.ends_with(suf)) {
        Some((suf, _)) => lower[..lower.len() - suf.len()].to_string(),
        None => lower,
    }
}

/// Значения `enum` схемы как канонические JSON-строки (для set-сравнения).
fn enum_values(prop: &Value) -> Option<Vec<String>> {
    prop.get("enum")
        .and_then(Value::as_array)
        .map(|a| a.iter().map(Value::to_string).collect())
}

/// Признак ключа партиционирования по имени параметра.
fn is_partition_name(name: &str) -> bool {
    let norm = name.to_ascii_lowercase().replace(['-', '_'], "");
    norm.ends_with("partitionkey") || norm.ends_with("partitionid")
}

/// Семантический дифф двух `OpenAPI`-документов: находки + имена затронутых
/// полей (для поиска потребителей).
///
/// # Errors
/// Текст не парсится или не `OpenAPI` 3.x.
pub(crate) fn diff_openapi_semantic(
    old_text: &str,
    new_text: &str,
    old: &Path,
    new: &Path,
) -> Result<(Vec<Finding>, Vec<String>)> {
    let old_doc = read_openapi(old_text, old)?;
    let new_doc = read_openapi(new_text, new)?;
    let mut out = Vec::new();
    let mut fields: BTreeSet<String> = BTreeSet::new();
    probe_schemas(&old_doc, &new_doc, &mut out, &mut fields);
    probe_partitioning(&old_doc, &new_doc, &mut out, &mut fields);
    Ok((out, fields.into_iter().collect()))
}

/// Зонды по `components.schemas.*.properties.*` (глубина — как у CD-006).
fn probe_schemas(old: &Value, new: &Value, out: &mut Vec<Finding>, fields: &mut BTreeSet<String>) {
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
        probe_schema(name, old_schema, new_schema, out, fields);
    }
}

/// Зонды одной схемы: совпавшие свойства (enum/default/единица-по-format) и
/// переименования со сменой единицы (совпадение по основе имени).
fn probe_schema(
    name: &str,
    old_schema: &Value,
    new_schema: &Value,
    out: &mut Vec<Finding>,
    fields: &mut BTreeSet<String>,
) {
    let old_props = old_schema.get("properties").and_then(Value::as_object);
    let new_props = new_schema.get("properties").and_then(Value::as_object);
    let (Some(old_props), Some(new_props)) = (old_props, new_props) else {
        return;
    };
    for (prop, old_prop) in old_props {
        let prop = prop.as_str();
        if let Some(new_prop) = new_props.get(prop) {
            probe_property_pair(name, prop, old_prop, new_prop, out, fields);
            continue;
        }
        probe_unit_rename(name, prop, new_props, out, fields);
    }
}

/// Свойство с тем же именем: `enum`, `default`, единица-по-`format`.
fn probe_property_pair(
    name: &str,
    prop: &str,
    old_prop: &Value,
    new_prop: &Value,
    out: &mut Vec<Finding>,
    fields: &mut BTreeSet<String>,
) {
    probe_enum(name, prop, old_prop, new_prop, out, fields);
    probe_default(name, prop, old_prop, new_prop, out, fields);
    probe_unit_format(name, prop, old_prop, new_prop, out, fields);
}

/// `enum`: удаление значения — error, добавление — warn.
fn probe_enum(
    name: &str,
    prop: &str,
    old_prop: &Value,
    new_prop: &Value,
    out: &mut Vec<Finding>,
    fields: &mut BTreeSet<String>,
) {
    let (Some(old_vals), Some(new_vals)) = (enum_values(old_prop), enum_values(new_prop)) else {
        return;
    };
    let loc = location(&["components", "schemas", name, "properties", prop]);
    for v in &old_vals {
        if !new_vals.contains(v) {
            fields.insert(prop.to_string());
            out.push(Finding {
                severity: "error".into(),
                rule: "semantic_enum_narrowed".into(),
                location: loc.clone(),
                message: format!(
                    "значение {v} удалено из enum поля «{prop}» схемы «{name}» — потребитель со \
                     старым значением ломается"
                ),
            });
        }
    }
    for v in &new_vals {
        if !old_vals.contains(v) {
            fields.insert(prop.to_string());
            out.push(Finding {
                severity: "warn".into(),
                rule: "semantic_enum_widened".into(),
                location: loc.clone(),
                message: format!(
                    "значение {v} добавлено в enum поля «{prop}» схемы «{name}» — расширение \
                     множества значений"
                ),
            });
        }
    }
}

/// `default`: изменён или снят — warn (появление default у поля без него
/// безопаснее и не флагается).
fn probe_default(
    name: &str,
    prop: &str,
    old_prop: &Value,
    new_prop: &Value,
    out: &mut Vec<Finding>,
    fields: &mut BTreeSet<String>,
) {
    let Some(old_def) = old_prop.get("default") else {
        return;
    };
    let changed = match new_prop.get("default") {
        None => true,
        Some(new_def) => old_def != new_def,
    };
    if changed {
        fields.insert(prop.to_string());
        let what = match new_prop.get("default") {
            None => "default снят".to_string(),
            Some(new_def) => format!("default изменён: {old_def} → {new_def}"),
        };
        out.push(Finding {
            severity: "warn".into(),
            rule: "semantic_default_shift".into(),
            location: location(&["components", "schemas", name, "properties", prop]),
            message: format!("поле «{prop}» схемы «{name}»: {what}"),
        });
    }
}

/// Единица-по-`format`: у поля с маркером единицы в имени изменился `format`.
fn probe_unit_format(
    name: &str,
    prop: &str,
    old_prop: &Value,
    new_prop: &Value,
    out: &mut Vec<Finding>,
    fields: &mut BTreeSet<String>,
) {
    if unit_family(prop).is_none() {
        return;
    }
    let (Some(old_fmt), Some(new_fmt)) = (
        old_prop.get("format").and_then(Value::as_str),
        new_prop.get("format").and_then(Value::as_str),
    ) else {
        return;
    };
    if old_fmt != new_fmt {
        fields.insert(prop.to_string());
        out.push(Finding {
            severity: "error".into(),
            rule: "semantic_unit_changed".into(),
            location: location(&["components", "schemas", name, "properties", prop]),
            message: format!(
                "поле «{prop}» схемы «{name}» (маркер единицы в имени) сменило формат: \
                 {old_fmt} → {new_fmt} — потребители парсят по имени"
            ),
        });
    }
}

/// Переименование свойства со сменой семейства единиц (`amount_cents` →
/// `amount_rubles`) или потерей/появлением маркера — error; смена внутри
/// одного семейства (`cents` → `kopecks`) допустима.
fn probe_unit_rename(
    name: &str,
    prop: &str,
    new_props: &Map<String, Value>,
    out: &mut Vec<Finding>,
    fields: &mut BTreeSet<String>,
) {
    let base = unit_base(prop);
    let old_family = unit_family(prop);
    for (cand, _) in new_props {
        let cand = cand.as_str();
        if unit_base(cand) != base {
            continue;
        }
        let new_family = unit_family(cand);
        if old_family.is_none() && new_family.is_none() {
            continue;
        }
        if old_family == new_family {
            continue;
        }
        let from = old_family.unwrap_or("без единицы");
        let to = new_family.unwrap_or("без единицы");
        fields.insert(prop.to_string());
        fields.insert(cand.to_string());
        out.push(Finding {
            severity: "error".into(),
            rule: "semantic_unit_changed".into(),
            location: location(&["components", "schemas", name, "properties", prop]),
            message: format!(
                "единица измерения поля «{prop}» → «{cand}» схемы «{name}»: {from} → {to} — \
                 потребители парсят по имени"
            ),
        });
        return;
    }
}

/// Ключ партиционирования: имена параметров `…partitionKey`/`…partitionId`
/// собраны по документу; изменение набора — warn.
fn probe_partitioning(
    old: &Value,
    new: &Value,
    out: &mut Vec<Finding>,
    fields: &mut BTreeSet<String>,
) {
    let old_keys = partition_keys(old);
    let new_keys = partition_keys(new);
    if old_keys.is_empty() || old_keys == new_keys {
        return;
    }
    for k in &old_keys {
        fields.insert(k.clone());
    }
    out.push(Finding {
        severity: "warn".into(),
        rule: "semantic_partition_key_changed".into(),
        location: location(&["partitioning"]),
        message: format!(
            "ключ партиционирования изменился: [{}] → [{}] — потребители, читающие по ключу, \
             могут сломаться",
            old_keys.join(", "),
            new_keys.join(", ")
        ),
    });
}

/// Имена параметров-ключей партиционирования во всём документе (сорт./дедуп).
fn partition_keys(doc: &Value) -> Vec<String> {
    let mut keys = BTreeSet::new();
    if let Some(params) = doc
        .get("components")
        .and_then(|c| c.get("parameters"))
        .and_then(Value::as_object)
    {
        for p in params.values() {
            collect_partition(p, &mut keys);
        }
    }
    if let Some(paths) = doc.get("paths").and_then(Value::as_object) {
        for item in paths.values() {
            if let Some(ps) = item.get("parameters").and_then(Value::as_array) {
                for p in ps {
                    collect_partition(p, &mut keys);
                }
            }
            for method in OPERATION_METHODS {
                if let Some(op) = item.get(method) {
                    if let Some(ps) = op.get("parameters").and_then(Value::as_array) {
                        for p in ps {
                            collect_partition(p, &mut keys);
                        }
                    }
                }
            }
        }
    }
    keys.into_iter().collect()
}

/// Добавляет имя параметра в множество, если это ключ партиционирования.
fn collect_partition(param: &Value, keys: &mut BTreeSet<String>) {
    if let Some(name) = param.get("name").and_then(Value::as_str) {
        if is_partition_name(name) {
            keys.insert(name.to_string());
        }
    }
}

/// Упрощённый поиск потребителей: файлы корня кейса, содержащие имена
/// семантически изменённых полей (метка `[inferred]`). Пустой список полей —
/// пустой результат (скан не запускается).
pub(crate) fn scan_consumers(case_root: &Path, field_names: &[String]) -> Vec<String> {
    if field_names.is_empty() {
        return Vec::new();
    }
    let mut files = Vec::new();
    let mut budget = MAX_SCAN_FILES;
    walk(case_root, 0, &mut files, &mut budget);
    let mut consumers = Vec::new();
    for path in files {
        if consumers.len() >= MAX_CONSUMERS {
            break;
        }
        let Ok(meta) = std::fs::metadata(&path) else {
            continue;
        };
        if meta.len() > MAX_FILE_BYTES {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        if field_names.iter().any(|f| content.contains(f.as_str())) {
            let rel = path.strip_prefix(case_root).unwrap_or(&path);
            consumers.push(format!("{} [inferred]", rel.display()));
        }
    }
    consumers
}

/// Рекурсивный обход каталога с потолками глубины и числа файлов.
fn walk(dir: &Path, depth: usize, out: &mut Vec<PathBuf>, budget: &mut usize) {
    if depth > MAX_SCAN_DEPTH || *budget == 0 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        if *budget == 0 {
            return;
        }
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') {
            continue;
        }
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            if SKIP_DIRS.contains(&name.as_ref()) {
                continue;
            }
            walk(&entry.path(), depth + 1, out, budget);
        } else if file_type.is_file() {
            out.push(entry.path());
            *budget -= 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Прогон зонда на двух текстах `OpenAPI` (пути файлов фиктивны).
    fn sem(old: &str, new: &str) -> (Vec<Finding>, Vec<String>) {
        diff_openapi_semantic(old, new, Path::new("old.yaml"), Path::new("new.yaml"))
            .expect("семантический дифф")
    }

    /// Коды правил находок (для компактных проверок).
    fn rules(findings: &[Finding]) -> Vec<&str> {
        findings.iter().map(|f| f.rule.as_str()).collect()
    }

    /// Документ с одной схемой `Payment` и одним свойством.
    fn doc(prop: &str) -> String {
        format!(
            "openapi: 3.0.3\ninfo:\n  title: T\n  version: 1.0.0\npaths: {{}}\n\
             components:\n  schemas:\n    Payment:\n      type: object\n      properties:\n\
             {prop}"
        )
    }

    /// Зонд 1: удаление значения из `enum` — `semantic_enum_narrowed` (error).
    #[test]
    fn enum_value_removed_is_breaking() {
        let old = doc("        status:\n          type: string\n          enum: [ok, failed]\n");
        let new = doc("        status:\n          type: string\n          enum: [ok]\n");
        let (findings, fields) = sem(&old, &new);
        assert_eq!(
            rules(&findings),
            vec!["semantic_enum_narrowed"],
            "{findings:?}"
        );
        assert_eq!(findings[0].severity, "error");
        assert!(
            findings[0].message.contains("failed"),
            "{}",
            findings[0].message
        );
        assert_eq!(fields, vec!["status".to_string()]);
    }

    /// Зонд 1: добавление значения в `enum` — `semantic_enum_widened` (warn).
    #[test]
    fn enum_value_added_is_non_breaking() {
        let old = doc("        status:\n          type: string\n          enum: [ok]\n");
        let new = doc("        status:\n          type: string\n          enum: [ok, pending]\n");
        let (findings, _) = sem(&old, &new);
        assert_eq!(
            rules(&findings),
            vec!["semantic_enum_widened"],
            "{findings:?}"
        );
        assert_eq!(findings[0].severity, "warn");
    }

    /// Зонд 2: переименование со сменой семейства единиц
    /// (`amount_cents` → `amount_rubles`) — `semantic_unit_changed` (error).
    #[test]
    fn unit_family_change_on_rename_is_breaking() {
        let old = doc("        amount_cents:\n          type: integer\n");
        let new = doc("        amount_rubles:\n          type: integer\n");
        let (findings, fields) = sem(&old, &new);
        assert_eq!(
            rules(&findings),
            vec!["semantic_unit_changed"],
            "{findings:?}"
        );
        assert_eq!(findings[0].severity, "error");
        assert!(fields.contains(&"amount_cents".to_string()));
        assert!(fields.contains(&"amount_rubles".to_string()));
    }

    /// Зонд 2: смена единицы ВНУТРИ одного семейства (`cents` → `kopecks`) —
    /// допустима, находки нет (защита от FP на синонимах).
    #[test]
    fn unit_same_family_rename_is_benign() {
        let old = doc("        amount_cents:\n          type: integer\n");
        let new = doc("        amount_kopecks:\n          type: integer\n");
        let (findings, fields) = sem(&old, &new);
        assert!(findings.is_empty(), "{findings:?}");
        assert!(fields.is_empty(), "{fields:?}");
    }

    /// Зонд 2: у поля с маркером единицы сменился `format` — error.
    #[test]
    fn unit_format_change_is_breaking() {
        let old = doc("        duration_ms:\n          type: string\n          format: int64\n");
        let new = doc("        duration_ms:\n          type: string\n          format: int32\n");
        let (findings, _) = sem(&old, &new);
        assert_eq!(
            rules(&findings),
            vec!["semantic_unit_changed"],
            "{findings:?}"
        );
        assert_eq!(findings[0].severity, "error");
    }

    /// Зонд 3: смена `default` — `semantic_default_shift` (warn).
    #[test]
    fn default_change_is_warn() {
        let old = doc("        limit:\n          type: integer\n          default: 10\n");
        let new = doc("        limit:\n          type: integer\n          default: 50\n");
        let (findings, fields) = sem(&old, &new);
        assert_eq!(
            rules(&findings),
            vec!["semantic_default_shift"],
            "{findings:?}"
        );
        assert_eq!(findings[0].severity, "warn");
        assert_eq!(fields, vec!["limit".to_string()]);
    }

    /// Зонд 3: ПОЯВЛЕНИЕ `default` у поля без него — не флагается (безопасно).
    #[test]
    fn default_added_is_benign() {
        let old = doc("        limit:\n          type: integer\n");
        let new = doc("        limit:\n          type: integer\n          default: 10\n");
        let (findings, _) = sem(&old, &new);
        assert!(findings.is_empty(), "{findings:?}");
    }

    /// Зонд 4: переименование ключа партиционирования — warn (уровень
    /// документа, а не схемы).
    #[test]
    fn partition_key_change_is_warn() {
        let old = "openapi: 3.0.3\ninfo:\n  title: T\n  version: 1.0.0\npaths:\n  /v1/events:\n\
                   \x20   get:\n      parameters:\n        - name: tenant_partition_key\n\
                   \x20         in: query\n          schema:\n            type: string\n\
                   components:\n  schemas: {}\n";
        let new = "openapi: 3.0.3\ninfo:\n  title: T\n  version: 1.0.0\npaths:\n  /v1/events:\n\
                   \x20   get:\n      parameters:\n        - name: tenant_shard\n\
                   \x20         in: query\n          schema:\n            type: string\n\
                   components:\n  schemas: {}\n";
        let (findings, _) = sem(old, new);
        assert_eq!(
            rules(&findings),
            vec!["semantic_partition_key_changed"],
            "{findings:?}"
        );
        assert_eq!(findings[0].severity, "warn");
    }

    /// FP-фикстура: benign-дифф (переименование поля без маркера единицы +
    /// правка описания) — `semantic_changes` пуст.
    #[test]
    fn benign_rename_and_description_is_clean() {
        let old = doc("        status:\n          type: string\n          description: old\n");
        let new = doc(
            "        state:\n          type: string\n          title: status\n          \
             description: new\n",
        );
        let (findings, fields) = sem(&old, &new);
        assert!(findings.is_empty(), "{findings:?}");
        assert!(fields.is_empty(), "{fields:?}");
    }

    /// Смена обязательности поля — механика CD (CD-008): зонд её НЕ дублирует,
    /// `semantic_changes` пуст.
    #[test]
    fn required_change_is_not_a_semantic_probe() {
        let old = "openapi: 3.0.3\ninfo:\n  title: T\n  version: 1.0.0\npaths:\n  /v1/pets:\n\
                   \x20   get:\n      responses:\n        '200':\n          description: ok\n\
                   components:\n  schemas:\n    Pets:\n      type: object\n      required: []\n\
                   \x20     properties:\n        id:\n          type: string\n";
        let new = "openapi: 3.0.3\ninfo:\n  title: T\n  version: 1.0.0\npaths:\n  /v1/pets:\n\
                   \x20   get:\n      responses:\n        '200':\n          description: ok\n\
                   components:\n  schemas:\n    Pets:\n      type: object\n      required: [id]\n\
                   \x20     properties:\n        id:\n          type: string\n";
        let (findings, _) = sem(old, new);
        assert!(findings.is_empty(), "{findings:?}");
    }

    /// `unit_base`/`unit_family`/`is_partition_name` — табличные случаи.
    #[test]
    fn unit_helpers_are_consistent() {
        assert_eq!(unit_family("amount_cents"), Some("money_minor"));
        assert_eq!(unit_family("amount_rubles"), Some("money_major"));
        assert_eq!(unit_family("timeout_ms"), Some("time_ms"));
        assert_eq!(unit_family("plain"), None);
        assert_eq!(unit_base("amount_cents"), "amount");
        assert_eq!(unit_base("plain"), "plain");
        assert!(is_partition_name("tenant_partition_key"));
        assert!(is_partition_name("tenant_partitionKey"));
        assert!(is_partition_name("tenant-partition-id"));
        assert!(!is_partition_name("tenant"));
    }
}
