//! Ручные инструменты-метрики контура (разбиение B1): `rules_suggest`
//! (кандидатные fitness-правила из пробелов кейса — read-only эвристики
//! [`crate::rules_suggest`]), `trust_report` (метрика доверия к контуру, W4),
//! `verdict_explain` (паспорт вердикта гейта, W1). Прогоны гейта внутри
//! наследуют серверный снимок модели доверия (A3: no-exec по умолчанию).

use std::path::PathBuf;

use serde::Deserialize;
use serde_json::{Value, json};

use crate::rules_suggest;

use crate::mcp_server::types::{CallError, McpServe, blocking, parse_args};

impl McpServe {
    /// `rules_suggest`: кандидатные fitness-правила из содержательных
    /// пробелов кейса (детекторы [`crate::rules_suggest`]: EARS, таймауты
    /// контрактов, REQ→TASK, RTO/RPO→ADR, аудит операторских действий).
    /// Информационный инструмент (read-only эвристики), verdict `passed`
    /// не применим.
    pub(in crate::mcp_server) async fn tool_rules_suggest(
        &self,
        args: Value,
    ) -> std::result::Result<Value, CallError> {
        #[derive(Deserialize)]
        struct Args {
            /// Корень кейса (каталог с docs/, model/, .arch-handoff/).
            path: String,
            /// Рабочий каталог клиента: относительный `path` резолвится от
            /// него (паттерн мостовых инструментов; по умолчанию — cwd
            /// процесса сервера).
            cwd: Option<String>,
        }
        let args: Args = parse_args(args, "rules_suggest")?;
        let raw = PathBuf::from(args.path);
        let case = match &args.cwd {
            Some(cwd) if !raw.is_absolute() => PathBuf::from(cwd).join(raw),
            _ => raw,
        };
        let case_display = case.display().to_string();
        let report = blocking("rules_suggest", move || rules_suggest::suggest(&case)).await?;
        Ok(json!({
            "case": case_display,
            "candidate_count": report.candidates.len(),
            "candidates": report.candidates,
            "report_markdown": rules_suggest::render_markdown(&report),
            "summary": report.summary,
        }))
    }

    /// Метрика доверия к контуру (W4): шкала 1–5 с якорями и доказательствами.
    /// Ничего не блокирует и ничего не пишет.
    pub(in crate::mcp_server) async fn tool_trust_report(
        &self,
        args: Value,
    ) -> std::result::Result<Value, CallError> {
        #[derive(Deserialize)]
        struct Args {
            /// Репозиторий или кейс (по умолчанию — каталог вызова).
            path: Option<String>,
            /// Рабочий каталог клиента: относительный `path` резолвится от него.
            cwd: Option<String>,
        }
        let args: Args = parse_args(args, "trust_report")?;
        let raw = args.path.map_or_else(|| PathBuf::from("."), PathBuf::from);
        let repo = match &args.cwd {
            Some(cwd) if !raw.is_absolute() => PathBuf::from(cwd).join(raw),
            _ => raw,
        };
        let cfg = self.cfg.clone();
        let repo_for_run = repo.clone();
        // A3: серверный снимок модели доверия — прогон гейта внутри оценки
        // наследует no-exec (дефолт сервера), как у fitness_check.
        let exec = self.exec.clone();
        let trust = blocking("trust_report", move || {
            crate::trust::assess_with(&repo_for_run, &cfg, &exec)
        })
        .await?;
        let mut out = crate::trust::to_json(&trust);
        out["report_markdown"] = Value::String(crate::trust::render(&trust));
        Ok(out)
    }

    /// Паспорт вердикта (W1): прогон гейта + страница «что зелёный НЕ
    /// означает». Инструмент чтения: ничего не пишет и решения не принимает —
    /// возвращает тот же вердикт, что `arch-be gate`, и его границы.
    pub(in crate::mcp_server) async fn tool_verdict_explain(
        &self,
        args: Value,
    ) -> std::result::Result<Value, CallError> {
        #[derive(Deserialize)]
        struct Args {
            /// Репозиторий (по умолчанию — каталог вызова).
            path: Option<String>,
            /// Маршрут: auto (по умолчанию) | fast | standard | critical.
            route: Option<String>,
            /// База git для диффа и сравнения правил.
            base: Option<String>,
            /// Файл ограничений (по умолчанию <repo>/.arch-handoff/CONSTRAINTS.yaml).
            constraints: Option<String>,
            /// Рабочий каталог клиента: относительный `path` резолвится от него.
            cwd: Option<String>,
        }
        let args: Args = parse_args(args, "verdict_explain")?;
        let raw = args.path.map_or_else(|| PathBuf::from("."), PathBuf::from);
        let repo = match &args.cwd {
            Some(cwd) if !raw.is_absolute() => PathBuf::from(cwd).join(raw),
            _ => raw,
        };
        let route = match args.route.as_deref().unwrap_or("auto").trim() {
            "auto" | "" => None,
            other => Some(
                other
                    .parse::<crate::control::Route>()
                    .map_err(CallError::invalid_params)?,
            ),
        };
        let base = args.base;
        let constraints = args.constraints.map(PathBuf::from);
        let limits = self
            .cfg
            .significance
            .limits()
            .map_err(|e| CallError::Execution(format!("verdict_explain: {e}")))?;
        let requirements = crate::gate::GateRequirements::from_config(&self.cfg.gate);
        // A3: серверный снимок модели доверия — прогон гейта наследует
        // no-exec (дефолт сервера) / allow-файл, как ручной fitness_check.
        let options = crate::gate::GateOptions {
            exec: self.exec.clone(),
            ..crate::gate::GateOptions::from_config(&self.cfg)
        };
        let repo_for_run = repo.clone();
        let report = blocking("verdict_explain", move || {
            crate::gate::run_opts(
                &repo_for_run,
                route,
                base.as_deref(),
                constraints.as_deref(),
                limits,
                &requirements,
                &options,
            )
        })
        .await?;
        let passport = crate::passport::Passport::build(&report, &repo);
        let mut out = passport.to_json();
        // Вердикт рядом с паспортом — тот же прогон, не второй: паспорт без
        // вердикта читался бы как самостоятельное суждение.
        out["verdict_envelope"] = report.envelope_json();
        out["report_markdown"] = Value::String(passport.render());
        out["summary"] = Value::String(crate::passport::summary_line(&passport));
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use crate::mcp_server::testkit::*;

    #[tokio::test]
    async fn rules_suggest_in_tools_list_and_finds_gap_candidates() {
        // Кейс с пробелом: контракт без численных таймаутов + спека без EARS.
        let dir = tempfile::tempdir().expect("tmp");
        std::fs::create_dir_all(dir.path().join("docs/contracts")).expect("mkdir");
        std::fs::write(
            dir.path().join("docs/contracts/api.md"),
            "# Контракт\n\nСинхронный вызов.\n",
        )
        .expect("contract");
        let case = dir.path().display().to_string();
        let owned = [
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}"#.to_string(),
            format!(
                r#"{{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{{"name":"rules_suggest","arguments":{{"path":"{case}"}}}}}}"#
            ),
        ];
        let refs: Vec<&str> = owned.iter().map(String::as_str).collect();
        let responses = run_lines(&refs).await;
        // Инструмент объявлен в tools/list (read-only режим).
        let tools = responses[0]["result"]["tools"].as_array().expect("tools");
        let spec = tools
            .iter()
            .find(|t| t["name"] == "rules_suggest")
            .expect("rules_suggest в tools/list");
        assert_eq!(spec["annotations"]["readOnlyHint"], true, "{spec}");
        assert!(
            spec["inputSchema"]["properties"]["cwd"].is_object(),
            "аргумент cwd в спеке: {spec}"
        );
        // Вызов находит кандидатов; у механизируемых — готовый YAML.
        let sc = &responses[1]["result"]["structuredContent"];
        let candidates = sc["candidates"].as_array().expect("candidates");
        let ids: Vec<&str> = candidates.iter().filter_map(|c| c["id"].as_str()).collect();
        assert!(
            ids.contains(&"contract-timeouts-numeric"),
            "{ids:?} (case: {case})"
        );
        let timeouts = candidates
            .iter()
            .find(|c| c["id"] == "contract-timeouts-numeric")
            .expect("кандидат");
        assert!(
            timeouts["yaml"]
                .as_str()
                .expect("yaml")
                .contains("must_contain"),
            "{timeouts}"
        );
        assert_eq!(timeouts["source_skill"], "adversarial-review");
        assert!(
            sc["report_markdown"]
                .as_str()
                .expect("markdown")
                .contains("Кандидатные fitness-правила"),
            "{sc}"
        );
        // Чистый кейс (пустой каталог) — честный ноль кандидатов.
        let empty = tempfile::tempdir().expect("tmp");
        let call = format!(
            r#"{{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{{"name":"rules_suggest","arguments":{{"path":"{}"}}}}}}"#,
            empty.path().display()
        );
        let responses = run_lines(&[&call]).await;
        let sc = &responses[0]["result"]["structuredContent"];
        assert_eq!(sc["candidate_count"], 0, "{sc}");
    }
}
