//! Агентный инструмент `contract_diff`: спецификация, аргументы, вызов.

use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::error::Result;
use crate::llm::ToolSpec;
use crate::tool::{Tool, ToolContext, ToolOutput};

use super::diffbase::diff_report;
use super::report::{render_report, report_json};
use super::types::ContractFormat;

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
    use crate::contract_diff::testkit::{BASE, PROTO_V1, diff_text};
    use crate::tool::ToolContext;

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
}
