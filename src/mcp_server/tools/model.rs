//! Ручной инструмент `model_query` (разбиение B1): список сущностей
//! типизированной модели (фильтр по типу) либо карточка сущности по `id`
//! со связями и обратными ссылками; сборка ответа — [`model_query_value`].

use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::{Value, json};

use crate::error::Result;
use crate::model;

use crate::mcp_server::types::{CallError, McpServe, blocking, parse_args};

impl McpServe {
    /// `model_query`: список сущностей модели (с фильтром по типу) либо
    /// карточка сущности по `id` со связями и обратными ссылками.
    pub(in crate::mcp_server) async fn tool_model_query(
        &self,
        args: Value,
    ) -> std::result::Result<Value, CallError> {
        #[derive(Deserialize)]
        struct Args {
            /// Корень кейса или каталог `model/` (T-13): инструмент находит
            /// модель сам; дефолт — `model` от cwd процесса сервера.
            #[serde(alias = "path")]
            dir: Option<String>,
            /// ID сущности — карточка (без `id` — список).
            id: Option<String>,
            /// Фильтр списка по типу (`cmp`, `adr`, … или префикс `CMP`).
            #[serde(rename = "type")]
            kind: Option<String>,
        }
        let args: Args = parse_args(args, "model_query")?;
        let kind = match &args.kind {
            Some(raw) => {
                let norm = raw.trim().to_ascii_lowercase();
                model::EntityKind::from_type_str(&norm)
                    .or_else(|| model::EntityKind::from_prefix(&raw.trim().to_ascii_uppercase()))
                    .ok_or_else(|| {
                        CallError::invalid_params(format!(
                            "model_query: неизвестный тип '{raw}' (допустимы: {})",
                            model::EntityKind::type_names().join(", ")
                        ))
                    })
                    .map(Some)?
            }
            None => None,
        };
        let dir = model::model_dir_from(&PathBuf::from(args.dir.unwrap_or_else(|| "model".into())));
        let id = args.id;
        blocking("model_query", move || {
            // Толерантная загрузка (E3): ответ по валидному подмножеству +
            // поле `load_issues` в JSON.
            let m = model::load_model_tolerant(&dir)?;
            model_query_value(&m, id.as_deref(), kind, &dir)
        })
        .await
    }
}

/// Строит JSON-ответ `model_query`: список сущностей либо карточка по `id`.
fn model_query_value(
    m: &model::Model,
    id: Option<&str>,
    kind: Option<model::EntityKind>,
    dir: &Path,
) -> Result<Value> {
    // E3: сущности, пропущенные при толерантной загрузке (пусто — модель
    // разобралась целиком).
    let load_issues = json!(m.load_issues);
    if let Some(id) = id {
        let e = m.get(id).ok_or_else(|| {
            crate::error::HarnessError::Model(format!(
                "model_query: сущность '{id}' не найдена (всего сущностей: {})",
                m.entities.len()
            ))
        })?;
        let mut links = serde_json::Map::new();
        for lk in model::LinkKind::ALL {
            let targets = e.link_targets(lk);
            if !targets.is_empty() {
                links.insert(lk.field_name().to_string(), json!(targets));
            }
        }
        let referents: Vec<Value> = m
            .referents(&e.id)
            .iter()
            .map(|(src, lk)| json!({"id": src.id, "title": src.title, "via": lk.field_name()}))
            .collect();
        return Ok(json!({
            "entity": {
                "id": e.id,
                "kind": e.kind.type_str(),
                "kind_ru": e.kind.title_ru(),
                "status": e.status,
                "title": e.title,
                "date": e.date,
                "verification": e.verification,
                "file": e.file,
                "links": links,
                "referents": referents,
                "body": e.body,
            },
            "card": model::card(m, e),
            "load_issues": load_issues,
        }));
    }
    let entities: Vec<Value> = m
        .entities
        .iter()
        .filter(|e| kind.is_none_or(|k| e.kind == k))
        .map(|e| {
            let links: usize = model::LinkKind::ALL
                .iter()
                .map(|k| e.link_targets(*k).len())
                .sum();
            json!({
                "id": e.id,
                "kind": e.kind.type_str(),
                "status": e.status,
                "title": e.title,
                "links": links,
            })
        })
        .collect();
    Ok(json!({
        "dir": dir,
        "total": entities.len(),
        "entities": entities,
        "load_issues": load_issues,
    }))
}
