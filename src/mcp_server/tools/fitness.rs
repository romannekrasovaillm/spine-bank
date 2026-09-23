//! Ручные инструменты контура контроля (разбиение B1): `spine_lint`,
//! `fitness_check` (с моделью доверия `command_succeeds`, A3), маршрут
//! значимости `significance_score` (включая форму `triggers` — [`TriggersArg`])
//! и `significance_from_diff` (anti-bypass S-1 из git-диффа), `trace_check`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::{Value, json};

use crate::{control, model, trace};

use crate::mcp_server::types::{CallError, McpServe, blocking, parse_args};

/// Аргумент `triggers` инструмента `significance_score`: каноничная карта
/// «триггер → bool» ЛИБО компактный массив строк вида `"name=true"`,
/// `"name=false"` или голое `"name"` (= true) — та же форма, что у CLI
/// `control score --trigger` (агенты-хосты часто копируют её в вызов MCP).
#[derive(Deserialize)]
#[serde(untagged)]
enum TriggersArg {
    /// Карта «триггер → сработал» (каноничная форма).
    Map(BTreeMap<String, bool>),
    /// Массив строк «name[=true|false]»; элемент без `=` — «name=true».
    List(Vec<String>),
}

impl TriggersArg {
    /// Приводит аргумент к карте триггеров; значение после `=`, отличное от
    /// `true`/`false`, и пустое имя — ошибка разбора (`-32602`).
    fn into_map(self) -> std::result::Result<BTreeMap<String, bool>, String> {
        match self {
            Self::Map(map) => Ok(map),
            Self::List(items) => {
                let mut map = BTreeMap::new();
                for item in items {
                    let (name, raw_value) = item.split_once('=').unwrap_or((item.as_str(), "true"));
                    let name = name.trim();
                    if name.is_empty() {
                        return Err(format!("пустое имя триггера в элементе '{item}'"));
                    }
                    let fired = match raw_value.trim() {
                        "true" => true,
                        "false" => false,
                        other => {
                            return Err(format!(
                                "значение '{other}' триггера '{name}' не bool \
                                 (ожидается true/false)"
                            ));
                        }
                    };
                    map.insert(name.to_string(), fired);
                }
                Ok(map)
            }
        }
    }
}

impl McpServe {
    /// `spine_lint`: линтер ARCHITECTURE-SPINE.md → verdict
    /// (`passed` = нет находок severity error).
    pub(in crate::mcp_server) async fn tool_spine_lint(
        &self,
        args: Value,
    ) -> std::result::Result<Value, CallError> {
        #[derive(Deserialize)]
        struct Args {
            /// Путь к ARCHITECTURE-SPINE.md.
            path: String,
        }
        let args: Args = parse_args(args, "spine_lint")?;
        let path = PathBuf::from(args.path);
        let issues = blocking("spine_lint", move || control::lint_spine(&path)).await?;
        let errors = issues.iter().filter(|i| i.severity == "error").count();
        let warns = issues.len() - errors;
        let summary = if issues.is_empty() {
            "spine: нарушений нет".to_string()
        } else {
            format!(
                "spine: {} находок (error: {errors}, warn: {warns})",
                issues.len()
            )
        };
        Ok(json!({
            "passed": errors == 0,
            "issue_count": issues.len(),
            "error_count": errors,
            "warn_count": warns,
            "issues": issues,
            "summary": summary,
        }))
    }

    /// `fitness_check`: прогон CONSTRAINTS.yaml по репозиторию → verdict
    /// (семантика [`control::check`]: `passed` = нет находок severity error).
    pub(in crate::mcp_server) async fn tool_fitness_check(
        &self,
        args: Value,
    ) -> std::result::Result<Value, CallError> {
        #[derive(Deserialize)]
        struct Args {
            /// Корень репозитория клиента.
            #[serde(alias = "path")]
            repo: String,
            /// Файл ограничений (дефолт `<repo>/.arch-handoff/CONSTRAINTS.yaml`,
            /// иначе `<repo>/CONSTRAINTS.yaml`).
            constraints: Option<String>,
            /// База git для сверки состава правил (П5): по умолчанию —
            /// merge-base с основной веткой, иначе HEAD.
            base: Option<String>,
        }
        let args: Args = parse_args(args, "fitness_check")?;
        let repo = PathBuf::from(args.repo);
        // Единый резолвер реестра (E2): явный путь → пакетная копия →
        // корневой fallback; ни одной копии — канонический дефолт, чтобы
        // ошибка «файл не читается» ссылалась на пакетный путь.
        let resolution = control::resolve_constraints_path_detailed(
            &repo,
            args.constraints.as_deref().map(Path::new),
        );
        let drift_note = resolution
            .as_ref()
            .and_then(control::ConstraintsPathResolution::drift_note);
        let constraints =
            resolution.map_or_else(|| repo.join(control::HANDOFF_CONSTRAINTS_PATH), |r| r.path);
        let constraints_label = constraints.display().to_string();
        let base = args.base;
        // П5: сверка состава правил с git-базой — анти-ослабление доступно
        // не только составному гейту. A3: серверный снимок модели доверия —
        // по умолчанию no-exec (команды реестра не исполняются), снимается
        // ARCH_NO_EXEC=0 в окружении сервера.
        let exec = self.exec.clone();
        let report = blocking("fitness_check", move || {
            control::check_anchored(
                &repo,
                &constraints,
                &control::baseline::CheckOptions {
                    exec,
                    ..control::baseline::CheckOptions::default()
                },
                base.as_deref(),
            )
        })
        .await?;
        Ok(json!({
            "passed": report.passed,
            "repo": report.repo,
            "constraints": constraints_label,
            "drift_note": drift_note,
            "issue_count": report.issues.len(),
            "issues": report.issues,
            "fingerprint": report.fingerprint,
            "untrusted_skipped": report.untrusted_skipped,
            "summary": report.summary,
        }))
    }

    /// `significance_score`: маршрут значимости по 15 триггерам
    /// (информационный инструмент, verdict `passed` не применим).
    /// Не метод: конфиг не нужен (clippy `unused_self` — `&self` осознанно
    /// отсутствует, в отличие от соседних инструментов).
    pub(in crate::mcp_server) fn tool_significance_score(
        args: Value,
    ) -> std::result::Result<Value, CallError> {
        #[derive(Deserialize)]
        struct Args {
            /// Триггеры: карта «триггер → сработал» либо массив строк
            /// «name[=true|false]» ([`TriggersArg`]).
            triggers: TriggersArg,
        }
        let args: Args = parse_args(args, "significance_score")?;
        let triggers = args
            .triggers
            .into_map()
            .map_err(|e| CallError::invalid_params(format!("significance_score: {e}")))?;
        // T-04: незнакомое имя триггера — ошибка вызова, а не тихо
        // завышенный маршрут. Раньше «foo» попадал в unknown_triggers и
        // ОДНОВРЕМЕННО в счёт: пять выдуманных имён давали Critical.
        let unknown = control::unknown_trigger_names(&triggers);
        if !unknown.is_empty() {
            return Err(CallError::invalid_params(format!(
                "significance_score: {}",
                control::unknown_triggers_error(&unknown)
            )));
        }
        let s = control::significance_score(&triggers);
        Ok(json!({
            "score": s.score,
            "fired": s.fired,
            "route": s.route,
            "summary": format!("Score: {} → маршрут {}", s.score, s.route),
        }))
    }

    /// `significance_from_diff`: маршрут значимости, выведенный из git-диффа
    /// репозитория (S-1 anti-bypass, ADR-034) в fail-safe объединении с
    /// заявленными триггерами (`declared`) — детектор только добавляет.
    /// Информационный инструмент, verdict `passed` не применим.
    ///
    /// В ответе: `route`/`score` по объединённому множеству, источник каждого
    /// триггера (`sources`: declared/diff/declared+diff) и `undeclared` —
    /// найденные диффом, но не заявленные триггеры с файлами-основаниями
    /// (anti-bypass сигнал «заявлено vs видно по диффу»).
    pub(in crate::mcp_server) async fn tool_significance_from_diff(
        &self,
        args: Value,
    ) -> std::result::Result<Value, CallError> {
        #[derive(Deserialize)]
        struct Args {
            /// Корень git-репозитория (дефолт — рабочий каталог процесса
            /// сервера, как у CLI `control score --from-diff`).
            path: Option<String>,
            /// Базовая точка диффа (`git diff BASE_REF...HEAD`); без неё —
            /// рабочее дерево против HEAD (staged + unstaged + untracked).
            base_ref: Option<String>,
            /// Заявленные агентом триггеры (карта «триггер → сработал», как у
            /// `significance_score`); объединяются с найденными по диффу.
            declared: Option<BTreeMap<String, bool>>,
        }
        let args: Args = parse_args(args, "significance_from_diff")?;
        let path = PathBuf::from(args.path.unwrap_or_else(|| ".".to_string()));
        let declared = args.declared.unwrap_or_default();
        // T-04: незнакомое имя в `declared` — ошибка вызова, как и в
        // `significance_score`: иначе «new_components» молча терялся бы, а
        // настоящий триггер остался бы незаявленным.
        let unknown_declared = control::unknown_trigger_names(&declared);
        if !unknown_declared.is_empty() {
            return Err(CallError::invalid_params(format!(
                "significance_from_diff: {}",
                control::unknown_triggers_error(&unknown_declared)
            )));
        }
        // Пороги маршрутов — из конфига сервера ([significance], ADR-034);
        // невалидные границы — понятный доменный сбой, не protocol error.
        let (fast_max, standard_max) = self
            .cfg
            .significance
            .limits()
            .map_err(|e| CallError::execution("significance_from_diff", e))?;
        let base_ref = args.base_ref;
        // T-05: глобы контрактов/компонентов — из секции `[significance]`
        // конфига сервера, а не зашиты в бинарь.
        let globs = self.cfg.significance.diff_globs();
        let diff = blocking("significance_from_diff", move || {
            control::detect_diff_triggers_with(&path, base_ref.as_deref(), &globs)
        })
        .await?;
        let scored = control::score_with_sources(&declared, &diff, fast_max, standard_max);

        let sources: serde_json::Map<String, Value> = scored
            .sources
            .iter()
            .map(|(t, s)| (t.clone(), json!(s.label())))
            .collect();
        // Основания срабатываний — строки вида «<trigger>: <файл-причина>»;
        // имена канонических триггеров не содержат «: », разбиение по первому
        // разделителю однозначно.
        let undeclared: Vec<Value> = scored
            .undeclared
            .iter()
            .map(|t| {
                let evidence: Vec<&str> = diff
                    .evidence
                    .iter()
                    .filter_map(|e| e.split_once(": "))
                    .filter(|(name, _)| name == t)
                    .map(|(_, reason)| reason)
                    .collect();
                json!({"trigger": t, "evidence": evidence})
            })
            .collect();
        let fired: Vec<String> = scored
            .significance
            .fired
            .iter()
            .map(|f| {
                scored
                    .sources
                    .get(f)
                    .map_or_else(|| f.clone(), |s| format!("{f} ({})", s.label()))
            })
            .collect();
        // Незнакомые имена отвергнуты выше — здесь пусто по построению;
        // поле остаётся в ответе для совместимости читателей.
        let unknown: Vec<&str> = Vec::new();
        let undeclared_note = if scored.undeclared.is_empty() {
            String::new()
        } else {
            format!(
                "; ВНИМАНИЕ — не заявлены, но видны по диффу: {}",
                scored.undeclared.join(", ")
            )
        };
        let summary = format!(
            "Score: {} ({}) → маршрут {}{}",
            scored.significance.score,
            if fired.is_empty() {
                "триггеров нет".to_string()
            } else {
                fired.join(", ")
            },
            scored.significance.route,
            undeclared_note,
        );
        Ok(json!({
            "route": scored.significance.route,
            "score": scored.significance.score,
            "fired": scored.significance.fired,
            "sources": sources,
            "undeclared": undeclared,
            "unknown_triggers": unknown,
            "summary": summary,
        }))
    }

    /// `trace_check`: позвенная трассируемость кейса → verdict
    /// (`passed` = нет находок severity error) + markdown-отчёт.
    pub(in crate::mcp_server) async fn tool_trace_check(
        &self,
        args: Value,
    ) -> std::result::Result<Value, CallError> {
        #[derive(Deserialize)]
        struct Args {
            /// Корень кейса (каталог с `model/`).
            #[serde(alias = "path")]
            case: String,
        }
        let args: Args = parse_args(args, "trace_check")?;
        let case = PathBuf::from(args.case);
        let report = blocking("trace_check", move || trace::trace_check(&case)).await?;
        let levels: Vec<Value> = report
            .levels
            .iter()
            .map(|l| {
                json!({
                    "name": l.name,
                    "total": l.total,
                    "covered": l.covered,
                    "unverifiable": l.unverifiable,
                    "orphans": l.orphans,
                    "percent": (l.covered * 100).checked_div(l.total),
                })
            })
            .collect();
        let issues: Vec<Value> = report
            .issues
            .iter()
            .map(|i| {
                json!({
                    "severity": i.severity.to_string(),
                    "rule": i.rule,
                    "message": i.message,
                })
            })
            .collect();
        let errors = report
            .issues
            .iter()
            .filter(|i| i.severity == model::Severity::Error)
            .count();
        let passed = !report.has_errors();
        Ok(json!({
            "passed": passed,
            "entities": report.entities,
            "constraint_rules": report.constraint_rules,
            "spine_ads": report.spine_ads,
            "levels": levels,
            "issue_count": report.issues.len(),
            "error_count": errors,
            "warn_count": report.issues.len() - errors,
            "issues": issues,
            "report_markdown": trace::render_markdown(&report),
            "summary": format!(
                "Итог: {} (error: {errors}, warn: {})",
                if passed { "PASS" } else { "FAIL" },
                report.issues.len() - errors
            ),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp_server::testkit::*;
    use crate::mcp_server::types::*;

    /// T-04: незнакомое имя триггера — ошибка вызова, а не завышенный
    /// маршрут. Раньше выдуманные имена попадали в `unknown_triggers` И в
    /// счёт: пять несуществующих триггеров давали Critical.
    #[tokio::test]
    async fn significance_score_routes_and_rejects_unknown() {
        let responses = run_lines(&[
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"significance_score","arguments":{"triggers":{"new_component":true,"security_boundary_change":true}}}}"#,
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"significance_score","arguments":{"triggers":{}}}}"#,
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"significance_score","arguments":{"triggers":{"alien_trigger":true}}}}"#,
            r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"significance_score","arguments":{"triggers":{"new_components":true,"new_datastore":true}}}}"#,
        ])
        .await;
        let sc = &responses[0]["result"]["structuredContent"];
        assert_eq!(sc["route"], "Critical");
        assert_eq!(sc["score"], 2);
        assert_eq!(responses[1]["result"]["structuredContent"]["route"], "Fast");
        let alien = &responses[2]["error"];
        assert_eq!(alien["code"], INVALID_PARAMS, "{alien}");
        assert!(
            alien["message"]
                .as_str()
                .expect("сообщение")
                .contains("Канонические (15)")
        );
        // Опечатка в настоящем имени: названо ближайшее каноническое, маршрута
        // нет — иначе `new_components` тихо занизил бы значимость.
        let typo = &responses[3]["error"];
        assert_eq!(typo["code"], INVALID_PARAMS, "{typo}");
        let text = typo["message"].as_str().expect("сообщение");
        assert!(text.contains("'new_components'"), "{text}");
        assert!(text.contains("'new_component'"), "{text}");
        // text-дубль verdict'а — валидный JSON (его разбирает клиент mcp.rs).
        let text = responses[0]["result"]["content"][0]["text"]
            .as_str()
            .expect("text");
        let parsed: Value = serde_json::from_str(text).expect("text — JSON");
        assert_eq!(parsed["route"], "Critical");
    }

    #[tokio::test]
    async fn significance_score_accepts_trigger_list_form() {
        // Массивная форма (`control score --trigger` стиль): "name=true",
        // "name=false", голое "name" (= true). Незнакомое имя отвергается (T-04).
        let responses = run_lines(&[
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"significance_score","arguments":{"triggers":["new_component=true","security_boundary_change"]}}}"#,
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"significance_score","arguments":{"triggers":["new_component=false"]}}}"#,
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"significance_score","arguments":{"triggers":["alien_trigger=true"]}}}"#,
            r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"significance_score","arguments":{"triggers":["new_component=да"]}}}"#,
        ])
        .await;
        let sc = &responses[0]["result"]["structuredContent"];
        assert_eq!(sc["route"], "Critical", "{sc}");
        assert_eq!(sc["score"], 2, "{sc}");
        // "new_component=false" — не сработал: пустое множество → Fast.
        let off = &responses[1]["result"]["structuredContent"];
        assert_eq!(off["route"], "Fast", "{off}");
        assert_eq!(off["fired"], json!([]), "{off}");
        // T-04: незнакомое имя в массивной форме — та же ошибка вызова.
        let alien = &responses[2]["error"];
        assert_eq!(alien["code"], INVALID_PARAMS, "{alien}");
        // Не-bool значение после '=' — понятная ошибка разбора (-32602).
        assert_eq!(responses[3]["error"]["code"], INVALID_PARAMS);
        assert!(
            responses[3]["error"]["message"]
                .as_str()
                .expect("сообщение")
                .contains("не bool"),
            "{}",
            responses[3]
        );
    }

    #[tokio::test]
    async fn spine_lint_verdict_marks_violations_and_clean() {
        let dir = tempfile::tempdir().expect("tmp");
        let bad = dir.path().join("BAD-SPINE.md");
        std::fs::write(
            &bad,
            "### AD-1. Брокер\n- Binds: контур\n- Prevents: хаос\n- Rule: только брокер\n\n\
             ### AD-1. Дубль\n- Binds: x\n- Prevents: y\n- Rule: z\n",
        )
        .expect("spine");
        let good = dir.path().join("GOOD-SPINE.md");
        std::fs::write(
            &good,
            "### AD-1. Брокер\n- Binds: контур\n- Prevents: хаос\n- Rule: только брокер\n",
        )
        .expect("spine");
        let call = |id: u64, path: &Path| {
            format!(
                r#"{{"jsonrpc":"2.0","id":{id},"method":"tools/call","params":{{"name":"spine_lint","arguments":{{"path":"{}"}}}}}}"#,
                path.display()
            )
        };
        let owned = [call(1, &bad), call(2, &good)];
        let refs: Vec<&str> = owned.iter().map(String::as_str).collect();
        let responses = run_lines(&refs).await;
        let bad_v = &responses[0]["result"]["structuredContent"];
        assert_eq!(bad_v["passed"], false, "{bad_v}");
        assert!(bad_v["issue_count"].as_u64().expect("число") >= 1);
        assert!(
            bad_v["issues"]
                .as_array()
                .expect("issues")
                .iter()
                .any(|i| i["rule"] == "dup_ad_id")
        );
        let good_v = &responses[1]["result"]["structuredContent"];
        assert_eq!(good_v["passed"], true, "{good_v}");
        assert_eq!(good_v["issue_count"], 0);
    }
}
