//! MCP-инструмент `report` (ADR-080 + ADR-083): отчёт о восстановленной
//! архитектуре ревизии — read-only мост к [`crate::report::build`].
//!
//! Тонкая обёртка (AD-11 дельты `report-v1`): вся сборка модели и назначение
//! ID/уверенности живут в `src/report/`; инструмент лишь резолвит путь, зовёт
//! сборку и рендерит результат. Отдаётся по белому списку
//! `BRIDGE_READ_ONLY` (`src/mcp_server/types.rs`).
//!
//! Fail-closed (как CLI `arch-be report`): блокирующие находки автопроверки
//! (`error`) не пропускаются ни в одном формате — инструмент возвращает
//! `ToolOutput::err` с текстом `check`; байпас-флага нет.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{Value, json};

use crate::error::Result;
use crate::llm::ToolSpec;
use crate::report::{build, render_check, render_json, render_md};
use crate::tool::{Tool, ToolContext, ToolOutput};

/// Инструменты домена `report` (один read-only инструмент).
#[must_use]
pub fn tools() -> Vec<Arc<dyn Tool>> {
    vec![Arc::new(ReportTool)]
}

/// Инструмент агента: `report` — отчёт о восстановленной архитектуре (read-only).
pub struct ReportTool;

#[async_trait]
impl Tool for ReportTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "report".into(),
            description: "Отчёт о восстановленной архитектуре ревизии (ADR-080): §1–§14 + \
                приложения, элементы с уверенностью A/B/C и основаниями файл:строка. Форматы: \
                md | json (схема arch-be/report/v1) | check. Read-only: рабочее дерево не \
                изменяется. Fail-closed: находки error блокируют выдачу (байпас-флага нет). \
                Детерминизм: два прогона на одном SHA дают байт-в-байт тот же JSON."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Каталог репозитория (относительно cwd или абсолютный)"
                    },
                    "rev": {
                        "type": "string",
                        "description": "Ревизия (ветка/тег/sha; по умолчанию HEAD)"
                    },
                    "format": {
                        "type": "string",
                        "description": "md | json | check (по умолчанию json)"
                    }
                },
                "required": ["path"]
            }),
        }
    }

    async fn call(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let Some(repo) = args
            .get("path")
            .or_else(|| args.get("repo"))
            .and_then(Value::as_str)
        else {
            return Ok(ToolOutput::err(
                "report: обязательный аргумент 'path' (string) отсутствует",
            ));
        };
        let repo = ctx.resolve(repo);
        let rev = args.get("rev").and_then(Value::as_str);
        let format = args.get("format").and_then(Value::as_str).unwrap_or("json");
        let model = match build(&repo, rev) {
            Ok(m) => m,
            Err(e) => return Ok(ToolOutput::err(format!("report: {e}"))),
        };
        // Fail-closed: блокирующие находки не пропускаем ни в одном формате.
        if model.has_errors() {
            return Ok(ToolOutput::err(format!(
                "report: fail-closed — {} error (выдача заблокирована, байпас-флага нет)\n{}",
                model.error_count(),
                render_check(&model)
            )));
        }
        let text = match format {
            "md" => render_md(&model),
            "check" => render_check(&model),
            _ => match render_json(&model) {
                Ok(s) => s,
                Err(e) => return Ok(ToolOutput::err(format!("report: {e}"))),
            },
        };
        Ok(ToolOutput::ok(text))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::arch_diff::snapshot::tests::{git_repo, write_file};
    use crate::config::Config;
    use crate::report::ReportModel;
    use std::path::Path;

    /// Минимальная фикстура (верхний крейт + точка входа + контракт).
    fn fixture(repo: &Path) {
        write_file(repo, "api/Cargo.toml", "[package]\nname = \"api\"\n");
        write_file(
            repo,
            "api/src/main.rs",
            "fn main() {}\n// axum: .route(\"/api/pay\", post(pay))\n",
        );
        write_file(repo, "contracts/ledger.yaml", "openapi: 3.0.0\n");
    }

    fn build_repo(dir: &Path, fixture: fn(&Path)) -> std::path::PathBuf {
        let repo = dir.join("repo");
        std::fs::create_dir_all(&repo).expect("mkdir");
        fixture(&repo);
        git_repo(&repo);
        repo
    }

    /// JSON инструмента байт-в-байт совпадает с CLI-рендером ([`render_json`]).
    #[tokio::test]
    async fn report_tool_json_matches_cli_render() {
        let dir = tempfile::tempdir().expect("tmp");
        let repo = build_repo(dir.path(), fixture);
        let ctx = ToolContext::new(dir.path().to_path_buf(), Arc::new(Config::default()));
        let out = ReportTool
            .call(json!({"path": "repo", "format": "json"}), &ctx)
            .await
            .expect("call");
        assert!(!out.is_error, "инструмент вернул ошибку: {}", out.content);
        let model: ReportModel = build(&repo, Some("HEAD")).expect("build");
        let expected = render_json(&model).expect("json");
        assert_eq!(out.content, expected, "JSON моста не паритетен CLI");
    }

    /// `check` инструмента совпадает с общим рендером автопроверки.
    #[tokio::test]
    async fn report_tool_check_matches_render_check() {
        let dir = tempfile::tempdir().expect("tmp");
        let repo = build_repo(dir.path(), fixture);
        let ctx = ToolContext::new(dir.path().to_path_buf(), Arc::new(Config::default()));
        let out = ReportTool
            .call(json!({"path": "repo", "format": "check"}), &ctx)
            .await
            .expect("call");
        assert!(!out.is_error, "инструмент вернул ошибку: {}", out.content);
        let model = build(&repo, Some("HEAD")).expect("build");
        assert_eq!(
            out.content,
            render_check(&model),
            "check моста не паритетен"
        );
    }

    /// Отсутствие обязательного `path` — честная ошибка, не паника.
    #[tokio::test]
    async fn report_tool_requires_path() {
        let dir = tempfile::tempdir().expect("tmp");
        let ctx = ToolContext::new(dir.path().to_path_buf(), Arc::new(Config::default()));
        let out = ReportTool.call(json!({}), &ctx).await.expect("call");
        assert!(out.is_error);
        assert!(out.content.contains("'path'"), "вывод: {}", out.content);
    }
}
