//! Дифф `OpenAPI` 3.x (CD-001..CD-010): пути, операции, параметры, ответы,
//! схемы; `$ref`/`allOf` резолвятся (`resolve_schema`), глубина раскрытия
//! ограничена [`SCHEMA_MAX_DEPTH`].

use std::collections::BTreeSet;
use std::path::Path;

use serde_json::Value;

use crate::error::{HarnessError, Result};

use super::diffbase::parse_contract;
use super::types::{Finding, escape_segment, location};

/// Методы операций `OpenAPI` (остальные ключи path item — служебные).
const OPERATION_METHODS: [&str; 8] = [
    "get", "put", "post", "delete", "options", "head", "patch", "trace",
];

/// Дифф двух текстов `OpenAPI` 3.x (JSON или YAML).
///
/// # Errors
/// Текст не парсится, документ не `OpenAPI` 3.x.
pub(crate) fn diff_openapi(
    old_text: &str,
    new_text: &str,
    old: &Path,
    new: &Path,
) -> Result<Vec<Finding>> {
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

#[cfg(test)]
mod tests {
    use super::SCHEMA_MAX_DEPTH;
    use crate::contract_diff::testkit::{BASE, diff_text};

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
}
