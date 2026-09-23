//! Агентные инструменты рубрик (B1): `rubric_list`, `rubric_evaluate`,
//! `rubric_generate` поверх каталога рубрик и LLM-судьи.

use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{Value, json};

use crate::error::{HarnessError, Result};
use crate::llm::ToolSpec;
use crate::tool::{Tool, ToolContext, ToolOutput};

use super::catalog::{list, load};
use super::judge::{check_target_len, evaluate_with_options, generate_dynamic};

/// Инструменты домена: `rubric_list`, `rubric_evaluate`, `rubric_generate`.
#[must_use]
pub fn tools() -> Vec<Arc<dyn Tool>> {
    vec![
        Arc::new(RubricListTool),
        Arc::new(RubricEvaluateTool),
        Arc::new(RubricGenerateTool),
    ]
}

/// Резолвит рубрику: прямой путь (через [`ToolContext::resolve`]) → файл в
/// каталоге рубрик → имя без расширения в каталоге рубрик.
fn resolve_rubric_path(ctx: &ToolContext, name: &str) -> PathBuf {
    let direct = ctx.resolve(name);
    if direct.is_file() {
        return direct;
    }
    let dir = ctx.config.paths.rubrics_dir();
    let in_dir = dir.join(name);
    if in_dir.is_file() {
        return in_dir;
    }
    dir.join(format!("{name}.yaml"))
}

/// Инструмент `rubric_list`: список рубрик каталога `assets/rubrics`.
struct RubricListTool;

#[async_trait]
impl Tool for RubricListTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "rubric_list".into(),
            description: "Список рубрик архитектурного контроля (имя, описание, число критериев)"
                .into(),
            parameters: json!({"type": "object", "properties": {}}),
        }
    }

    async fn call(&self, _args: Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let dir = ctx.config.paths.rubrics_dir();
        let items = list(&dir)?;
        if items.is_empty() {
            return Ok(ToolOutput::ok(format!(
                "рубрики не найдены в {}",
                dir.display()
            )));
        }
        let mut out = String::new();
        for r in &items {
            let _ = writeln!(
                out,
                "- {} — {} ({} критериев; {})",
                r.name,
                r.description,
                r.criteria_count,
                r.path.display()
            );
        }
        Ok(ToolOutput::ok(out))
    }
}

/// Инструмент `rubric_evaluate`: оценка текста по рубрике через LLM-судью.
struct RubricEvaluateTool;

#[async_trait]
impl Tool for RubricEvaluateTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "rubric_evaluate".into(),
            description: "Оценить текст (ADR, дизайн-документ) по рубрике через независимого \
                          LLM-судью (k сэмплов, медиана; оценка требует цитаты из текста); \
                          с dynamic_subject рубрика генерируется под предмет от якорной"
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "rubric": {"type": "string", "description": "Имя рубрики в assets/rubrics или путь к YAML"},
                    "target": {"type": "string", "description": "Путь к оцениваемому тексту"},
                    "dynamic_subject": {"type": "string", "description": "Опц.: предмет для динамической рубрики (rubric — якорь)"}
                },
                "required": ["rubric", "target"]
            }),
        }
    }

    async fn call(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let Some(registry) = &ctx.llm else {
            return Ok(ToolOutput::err("нет LLM в контексте"));
        };
        let Some(rubric_arg) = args.get("rubric").and_then(Value::as_str) else {
            return Ok(ToolOutput::err("аргумент 'rubric' обязателен (string)"));
        };
        let Some(target_arg) = args.get("target").and_then(Value::as_str) else {
            return Ok(ToolOutput::err("аргумент 'target' обязателен (string)"));
        };
        let llm = registry.default();
        let target_path = ctx.resolve(target_arg);
        let text =
            std::fs::read_to_string(&target_path).map_err(|e| HarnessError::io(&target_path, e))?;
        // ADR-004: длинный документ — понятная ошибка для модели, а не
        // тихое усечение перед отправкой судье.
        if let Err(e) = check_target_len(&text) {
            return Ok(ToolOutput::err(e.to_string()));
        }
        let rubric_path = resolve_rubric_path(ctx, rubric_arg);
        let rubric = match args.get("dynamic_subject").and_then(Value::as_str) {
            Some(subject) => {
                let anchor = load(&rubric_path).ok();
                generate_dynamic(subject, anchor.as_ref(), llm.as_ref()).await?
            }
            None => load(&rubric_path)?,
        };
        let report = evaluate_with_options(&rubric, &text, llm.as_ref(), &ctx.config.judge).await?;
        Ok(ToolOutput::ok(report.to_markdown()))
    }
}

/// Инструмент `rubric_generate`: динамическая рубрика под предмет оценки.
struct RubricGenerateTool;

#[async_trait]
impl Tool for RubricGenerateTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "rubric_generate".into(),
            description: "Сгенерировать динамическую рубрику оценки под предмет \
                          (опц. от якорной основы); ответ — YAML рубрики"
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "subject": {"type": "string", "description": "Предмет оценки (напр. 'ADR миграции платёжного шлюза')"},
                    "anchor": {"type": "string", "description": "Опц.: имя/путь якорной рубрики-основы"}
                },
                "required": ["subject"]
            }),
        }
    }

    async fn call(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let Some(registry) = &ctx.llm else {
            return Ok(ToolOutput::err("нет LLM в контексте"));
        };
        let Some(subject) = args.get("subject").and_then(Value::as_str) else {
            return Ok(ToolOutput::err("аргумент 'subject' обязателен (string)"));
        };
        let llm = registry.default();
        let anchor = match args.get("anchor").and_then(Value::as_str) {
            Some(name) => Some(load(&resolve_rubric_path(ctx, name))?),
            None => None,
        };
        let rubric = generate_dynamic(subject, anchor.as_ref(), llm.as_ref()).await?;
        let yaml = serde_yaml_ng::to_string(&rubric)?;
        Ok(ToolOutput::ok(yaml))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rubric::testkit::*;
    use crate::rubric::types::MAX_TARGET_CHARS;

    #[tokio::test]
    async fn tools_are_registered_under_contract_names() {
        let names: Vec<String> = tools().iter().map(|t| t.spec().name.clone()).collect();
        assert_eq!(names, ["rubric_list", "rubric_evaluate", "rubric_generate"]);
    }

    #[tokio::test]
    async fn rubric_list_tool_reads_configured_dir() {
        let dir = tempfile::tempdir().expect("tempdir");
        let rubrics = dir.path().join("assets").join("rubrics");
        std::fs::create_dir_all(&rubrics).expect("mkdir");
        std::fs::write(
            rubrics.join("r.yaml"),
            serde_yaml_ng::to_string(&sample_rubric()).expect("yaml"),
        )
        .expect("write");
        let mut cfg = crate::config::Config::default();
        cfg.paths.assets_dir = dir.path().join("assets");
        let ctx = ToolContext::new(dir.path().to_path_buf(), Arc::new(cfg));
        let out = RubricListTool.call(json!({}), &ctx).await.expect("call");
        assert!(!out.is_error);
        assert!(
            out.content.contains("adr-quality"),
            "вывод: {}",
            out.content
        );
    }

    #[tokio::test]
    async fn rubric_evaluate_tool_without_llm_is_err() {
        let dir = tempfile::tempdir().expect("tempdir");
        let ctx = ToolContext::new(
            dir.path().to_path_buf(),
            Arc::new(crate::config::Config::default()),
        );
        let out = RubricEvaluateTool
            .call(json!({"rubric": "x", "target": "y"}), &ctx)
            .await
            .expect("call");
        assert!(out.is_error);
        assert!(out.content.contains("нет LLM в контексте"));
    }

    #[tokio::test]
    async fn rubric_evaluate_tool_long_target_is_err_output_not_panic() {
        let dir = tempfile::tempdir().expect("tempdir");
        let rubrics = dir.path().join("assets").join("rubrics");
        std::fs::create_dir_all(&rubrics).expect("mkdir");
        std::fs::write(
            rubrics.join("r.yaml"),
            serde_yaml_ng::to_string(&sample_rubric()).expect("yaml"),
        )
        .expect("write");
        let long: String = "б".repeat(MAX_TARGET_CHARS + 500);
        std::fs::write(dir.path().join("big.md"), &long).expect("write");
        let mut cfg = crate::config::Config::default();
        cfg.paths.assets_dir = dir.path().join("assets");
        let cfg = Arc::new(cfg);
        let registry = Arc::new(crate::llm::LlmRegistry::from_config(&cfg).expect("registry"));
        let ctx = ToolContext::new(dir.path().to_path_buf(), cfg).with_llm(registry);
        let out = RubricEvaluateTool
            .call(json!({"rubric": "r", "target": "big.md"}), &ctx)
            .await
            .expect("call");
        assert!(out.is_error, "длинный документ — ToolOutput::err");
        assert!(
            out.content.contains("слишком длинный"),
            "вывод: {}",
            out.content
        );
        assert!(
            out.content.contains("лимите 24000"),
            "вывод: {}",
            out.content
        );
    }
}
