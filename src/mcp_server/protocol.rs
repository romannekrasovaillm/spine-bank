//! JSON-RPC костяк и диспетчер MCP-сервера (разбиение B1): разбор строк
//! транспорта (`handle_line`/`handle_request`), `tools/call` (диспетчер
//! ручных инструментов + мост в реестр с белыми списками режима),
//! журналирование вызовов (mcp-calls.jsonl), `prompts/list`/`prompts/get`
//! (плейбуки spine-workflows), отсев незнакомых аргументов (Н8), спеки
//! `tools/list` (ручные + мостовые с аннотациями политики), цикл [`run_loop`]
//! и точки входа [`serve`]/[`serve_with_mode`].

use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};

use crate::config::Config;
use crate::error::Result;
use crate::tool::ToolContext;
use crate::{mcp, plugin};

use super::types::{
    BRIDGE_OUTPUT_MAX_CHARS, BRIDGE_READ_ONLY, BRIDGE_READ_WRITE, CallError, INTERNAL_ERROR,
    INVALID_PARAMS, INVALID_REQUEST, MAX_LINE_BYTES, MAX_PROMPT_ARG_VALUE_CHARS, METHOD_NOT_FOUND,
    McpServe, PARSE_ERROR, PATH_ARG_ALIASES, PLAYBOOK_PROMPTS, PlaybookPrompt,
    SKILL_TEXT_MAX_CHARS, ServeMode, blocking, error_response, ok_response,
    verdict_from_bridge_text, verdict_from_structured,
};

/// Исход успешного вызова инструмента: чем заполнить `content`/`structuredContent`.
enum DispatchOutcome {
    /// Машиночитаемый verdict (ручные инструменты): text-дубль — pretty JSON.
    Structured(Value),
    /// Текстовый вывод доменного инструмента реестра (мост): `text` идёт в
    /// `content` как есть (его читает клиент [`crate::mcp`]), structured —
    /// обёртка `{tool, output}` для машиночитаемого контура.
    Text {
        /// Обёртка `{tool, output}` для `structuredContent`.
        structured: Value,
        /// Сырой текстовый вывод инструмента (уже усечённый до лимита).
        text: String,
    },
}

impl McpServe {
    /// Обрабатывает одну строку транспорта; `None` — отвечать не нужно
    /// (уведомления по JSON-RPC ответа не имеют).
    pub(super) async fn handle_line(&self, line: &str) -> Option<Value> {
        let message: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(e) => {
                return Some(error_response(
                    &Value::Null,
                    PARSE_ERROR,
                    format!("невалидный JSON: {e}"),
                ));
            }
        };
        let Some(obj) = message.as_object() else {
            return Some(error_response(
                &Value::Null,
                INVALID_REQUEST,
                "сообщение не является JSON-объектом",
            ));
        };
        let id = obj.get("id").cloned();
        let method = obj.get("method").and_then(Value::as_str);
        match (id, method) {
            // Уведомления (без id) не получают ответа — ни на notifications/*,
            // ни на неизвестные методы-уведомления.
            (None, Some(_)) => None,
            (None, None) => Some(error_response(
                &Value::Null,
                INVALID_REQUEST,
                "нет поля 'method'",
            )),
            (Some(id), None) => Some(error_response(&id, INVALID_REQUEST, "нет поля 'method'")),
            (Some(id), Some(method)) => {
                let params = obj.get("params").cloned().unwrap_or(Value::Null);
                Some(self.handle_request(&id, method, params).await)
            }
        }
    }

    /// Диспетчер запросов (методы с `id`, требующие ответа).
    async fn handle_request(&self, id: &Value, method: &str, params: Value) -> Value {
        match method {
            "initialize" => {
                // Echo известной версии из запроса, иначе — наша текущая:
                // клиент сам решит, устраивает ли его ответная версия.
                let requested = params.get("protocolVersion").and_then(Value::as_str);
                let version = match requested {
                    Some(v)
                        if v == mcp::PROTOCOL_VERSION || v == mcp::PROTOCOL_VERSION_FALLBACK =>
                    {
                        v
                    }
                    _ => mcp::PROTOCOL_VERSION,
                };
                // `clientInfo` — единственное, что хост говорит о себе сам:
                // запоминаем как ЗАЯВЛЕННОЕ имя и версию (ADR-048: это метка
                // хоста, а не удостоверение того, кто отвечал на промпты).
                let host = params.get("clientInfo").and_then(|ci| {
                    let name = ci.get("name").and_then(Value::as_str)?;
                    if name.trim().is_empty() {
                        return None;
                    }
                    Some(crate::judge::HostInfo {
                        name: name.to_string(),
                        version: ci
                            .get("version")
                            .and_then(Value::as_str)
                            .map(str::to_string),
                    })
                });
                let session_id = {
                    let mut session = self.session();
                    if let Some(host) = host {
                        session.set_host(host);
                    }
                    session.id().to_string()
                };
                ok_response(
                    id,
                    &json!({
                        "protocolVersion": version,
                        "sessionId": session_id,
                        "capabilities": {
                            "tools": {"listChanged": false},
                            "prompts": {"listChanged": false},
                        },
                        "serverInfo": {
                            "name": "arch-harness",
                            "version": env!("CARGO_PKG_VERSION"),
                        },
                        "instructions": "Архитектурный контроль Spine для кодового агента: \
                                         перед коммитом изменения вызывайте fitness_check \
                                         (repo + CONSTRAINTS.yaml), trace_check (case) и \
                                         spine_lint (path); passed=false с находками error — \
                                         основание ОТКАЗАТЬ изменению, нарушающему AD-*, \
                                         перечислив находки. significance_score — маршрут \
                                         значимости (fast/standard/critical). model_query — \
                                         карточки и связи сущностей модели. rubric_run — \
                                         LLM-оценка документа рубрикой (нужен API-ключ); \
                                         без ключа — split-judge: rubric_prompt выдаёт промпты \
                                         судьи и JSON-схему ответа, rubric_verify механически \
                                         собирает отчёт из сырых ответов вашей модели. \
                                         Детерминированный контур реестра (openapi_lint, \
                                         asyncapi_lint, contract_diff, fleet_audit, \
                                         agentsmd_lint, archify_validate, rubric_list, \
                                         plugin_list, nfr_check, model_validate, model_drift, \
                                         delta_guard, evidence_verify) доступен напрямую; \
                                         отчёты реестров (landscape_report, adr_registry, \
                                         rules_report, openspec_coverage, model_graph) — \
                                         read-only JSON со счётчиками; составные инструменты: \
                                         architect_review (всё ревью одним вызовом — маршрут, \
                                         контур контроля, модель, контракты) и change_impact \
                                         (что заденет изменение и с кем согласовывать); \
                                         rules_suggest — кандидатные fitness-правила из \
                                         пробелов кейса (EARS, таймауты контрактов, REQ→TASK, \
                                         RTO/RPO→ADR, аудит оператора); аргумент `cwd` — \
                                         рабочий каталог клиента для относительных путей. \
                                         Чтение знаний (read-only): kb_search — поиск по \
                                         базе знаний архитектора; skill_search/skill_load — \
                                         библиотека скиллов; mermaid_render — диаграмма \
                                         mermaid (code/path) в ASCII-арт. Плейбуки spine-* \
                                         (подключение, гейты, разбор, судья рубрик, визуализация) \
                                         доступны как промпты (prompts/list, prompts/get) — \
                                         слэш-команды хоста с полным сценарием в сообщении.",
                    }),
                )
            }
            "ping" => ok_response(id, &json!({})),
            "tools/list" => ok_response(id, &json!({"tools": self.all_tool_specs()})),
            "tools/call" => self.handle_tool_call(id, &params).await,
            "prompts/list" => self.handle_prompts_list(id).await,
            "prompts/get" => self.handle_prompts_get(id, &params).await,
            other => error_response(id, METHOD_NOT_FOUND, format!("неизвестный метод '{other}'")),
        }
    }

    /// `tools/call`: разбор `name`/`arguments`, диспетчер инструментов,
    /// упаковка verdict'а в MCP-ответ.
    async fn handle_tool_call(&self, id: &Value, params: &Value) -> Value {
        let Some(name) = params.get("name").and_then(Value::as_str) else {
            return error_response(id, INVALID_PARAMS, "tools/call: нет строкового поля 'name'");
        };
        let args = params
            .get("arguments")
            .cloned()
            .unwrap_or_else(|| json!({}));
        if !args.is_object() {
            return error_response(
                id,
                INVALID_PARAMS,
                format!("tools/call: 'arguments' должен быть объектом, получено: {args}"),
            );
        }
        let started = std::time::Instant::now();
        let outcome = self.dispatch_tool(name, args).await;
        // Счётчик вызовов сессии растёт ПОСЛЕ вызова: во время обработки
        // `self.session().calls()` — это число уже прошедших вызовов, и отчёт
        // судьи честно называет, сколько работы было в сессии ДО судейства
        // (косвенный признак рабочего контекста автора, ADR-048).
        self.session().note_call();
        self.journal_call(name, started.elapsed(), &outcome);
        match outcome {
            Ok(DispatchOutcome::Structured(structured)) => {
                // Клиент нашего же mcp.rs читает только text-части — дублируем
                // verdict pretty-JSON; structuredContent — для MCP-клиентов.
                let text = serde_json::to_string_pretty(&structured)
                    .unwrap_or_else(|_| structured.to_string());
                ok_response(
                    id,
                    &json!({
                        "content": [{"type": "text", "text": text}],
                        "structuredContent": structured,
                        "isError": false,
                    }),
                )
            }
            Ok(DispatchOutcome::Text { structured, text }) => ok_response(
                id,
                &json!({
                    "content": [{"type": "text", "text": text}],
                    "structuredContent": structured,
                    "isError": false,
                }),
            ),
            Err(CallError::Execution(message)) => ok_response(
                id,
                &json!({
                    "content": [{"type": "text", "text": message}],
                    "isError": true,
                }),
            ),
            Err(CallError::Protocol { code, message }) => error_response(id, code, message),
        }
    }

    /// Журналирует вызов в проектный журнал `.arch-handoff/mcp-calls.jsonl`
    /// (модуль [`crate::mcp_journal`]): инструмент, вердикт, длительность,
    /// имена правил из error-находок — БЕЗ содержимого аргументов. Здесь
    /// проходят и ручные, и мостовые вызовы (единая точка `tools/call`).
    /// Fail-soft: журнал — аудит, а не часть вызова; его сбой (каталог не
    /// создать, ФС только на чтение) не должен ломать инструмент.
    fn journal_call(
        &self,
        name: &str,
        duration: std::time::Duration,
        outcome: &std::result::Result<DispatchOutcome, CallError>,
    ) {
        let (verdict, rules) = match outcome {
            Ok(DispatchOutcome::Structured(v)) => verdict_from_structured(v),
            Ok(DispatchOutcome::Text { text, .. }) => verdict_from_bridge_text(text),
            Err(CallError::Execution(_)) => ("error", Vec::new()),
            Err(CallError::Protocol { .. }) => ("invalid", Vec::new()),
        };
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        // Сессия и хост: без них «судейство в рабочей сессии» неотличимо от
        // судейства в чистой (ADR-048).
        let (session_id, host) = {
            let session = self.session();
            (
                Some(session.id().to_string()),
                session.host().map(|h| h.name),
            )
        };
        let mut entry = crate::mcp_journal::JournalEntry::new(name, verdict, duration, rules)
            .with_session(session_id, host);
        // Мета судейства — только у `rubric_verify`: метки судьи и автора и
        // уровень независимости. Это единственное исключение из правила «в
        // журнале нет содержимого аргументов»: метки — не содержимое документа
        // (ADR-048). Сами ответы судьи в журнал не пишутся.
        if name == "rubric_verify" {
            if let Ok(DispatchOutcome::Structured(v)) = outcome {
                let target = v.get("artifact_path").and_then(Value::as_str);
                if let Some(meta) =
                    crate::mcp_journal::JournalEntry::judging_from_response(target, v)
                {
                    entry = entry.with_judging(meta);
                }
            }
        }
        // Ошибка записи журнала осознанно глушится (fail-soft по контракту
        // mcp_journal): аудит не должен ломать вызовы инструментов.
        let _ = crate::mcp_journal::append(&cwd, &entry);
    }

    /// `prompts/list`: десять плейбуков [`PLAYBOOK_PROMPTS`] с описаниями из
    /// frontmatter и объявлениями аргументов. Пагинация не нужна (список
    /// фиксирован и мал) — `cursor` из params принимается и игнорируется.
    async fn handle_prompts_list(&self, id: &Value) -> Value {
        let dirs = self.cfg.plugins.dirs.clone();
        let listed = blocking("prompts/list", move || -> Result<Value> {
            let prompts: Vec<Value> = PLAYBOOK_PROMPTS
                .iter()
                .map(|pb| {
                    let (_, description) = resolve_playbook(&dirs, pb);
                    json!({
                        "name": pb.name,
                        "description": description,
                        "arguments": pb.arguments.iter().map(|(an, ad)| json!({
                            "name": an,
                            "description": ad,
                            "required": false,
                        })).collect::<Vec<_>>(),
                    })
                })
                .collect();
            Ok(json!({"prompts": prompts}))
        })
        .await;
        match listed {
            Ok(result) => ok_response(id, &result),
            // Доменных сбоев тут нет (fallback встроенный) — только срыв
            // blocking-задачи: внутренняя ошибка сервера.
            Err(CallError::Execution(message)) => error_response(id, INTERNAL_ERROR, message),
            Err(CallError::Protocol { code, message }) => error_response(id, code, message),
        }
    }

    /// `prompts/get`: слэш-команда хоста — одно user-сообщение с инструкцией
    /// «действуй по этому плейбуку», полным текстом SKILL.md и эхом переданных
    /// (объявленных) аргументов. Неизвестное имя и битые аргументы → `-32602`.
    async fn handle_prompts_get(&self, id: &Value, params: &Value) -> Value {
        let Some(name) = params.get("name").and_then(Value::as_str) else {
            return error_response(
                id,
                INVALID_PARAMS,
                "prompts/get: нет строкового поля 'name'",
            );
        };
        let Some(pb) = PLAYBOOK_PROMPTS.iter().find(|p| p.name == name) else {
            return error_response(
                id,
                INVALID_PARAMS,
                format!("prompts/get: неизвестный промпт '{name}'; список — prompts/list"),
            );
        };
        let arguments = params
            .get("arguments")
            .cloned()
            .unwrap_or_else(|| json!({}));
        let Some(arg_obj) = arguments.as_object() else {
            return error_response(
                id,
                INVALID_PARAMS,
                format!(
                    "prompts/get: 'arguments' должен быть объектом «имя → строка», получено: {arguments}"
                ),
            );
        };
        // Эхом подставляются только объявленные аргументы (незнакомые ключи
        // хоста игнорируются — форвард-совместимость); значения — строки по
        // спецификации PromptArgument, иное → -32602.
        let mut provided: Vec<(&str, String)> = Vec::new();
        for (an, _) in pb.arguments {
            if let Some(v) = arg_obj.get(*an) {
                let Some(s) = v.as_str() else {
                    return error_response(
                        id,
                        INVALID_PARAMS,
                        format!("prompts/get: аргумент '{an}' должен быть строкой, получено: {v}"),
                    );
                };
                provided.push((*an, s.chars().take(MAX_PROMPT_ARG_VALUE_CHARS).collect()));
            }
        }
        let dirs = self.cfg.plugins.dirs.clone();
        let built = blocking("prompts/get", move || -> Result<Value> {
            let (mut body, description) = resolve_playbook(&dirs, pb);
            if body.chars().count() > SKILL_TEXT_MAX_CHARS {
                let cut: String = body.chars().take(SKILL_TEXT_MAX_CHARS).collect();
                body = format!("{cut}\n… [усечено: {SKILL_TEXT_MAX_CHARS} символов]");
            }
            let mut text = format!(
                "Действуй по этому плейбуку — скилл `{}` плагина spine-workflows \
                 (MCP-сервер Spine): выполняй его шаги по порядку, вызывая \
                 инструменты этого сервера; вердикты (`passed`, находки) \
                 докладывай архитектору.\n\n{body}",
                pb.name
            );
            if !provided.is_empty() {
                text.push_str("\n\nАргументы запуска (подставь в шаги плейбука):");
                for (an, av) in &provided {
                    // write! в String не падает — игнор результата безопасен.
                    let _ = write!(text, "\n- {an} = \"{av}\"");
                }
            }
            Ok(json!({
                "description": description,
                "messages": [{
                    "role": "user",
                    "content": {"type": "text", "text": text},
                }],
            }))
        })
        .await;
        match built {
            Ok(result) => ok_response(id, &result),
            Err(CallError::Execution(message)) => error_response(id, INTERNAL_ERROR, message),
            Err(CallError::Protocol { code, message }) => error_response(id, code, message),
        }
    }

    /// Отвергает незнакомые аргументы вызова, перечисляя допустимые (Н8).
    ///
    /// Проверяются инструменты с объявленной схемой; у мостовых инструментов
    /// схема живёт в модуле и проверку делает `serde` при разборе — там
    /// перечень печатает сама ошибка десериализации.
    fn reject_unknown_args(name: &str, args: &Value) -> std::result::Result<(), CallError> {
        let Some(map) = args.as_object() else {
            return Ok(());
        };
        if map.is_empty() {
            return Ok(());
        }
        let Some(schema) = tool_specs()
            .into_iter()
            .find(|t| t.get("name").and_then(Value::as_str) == Some(name))
            .and_then(|t| t.get("inputSchema").cloned())
        else {
            return Ok(());
        };
        let Some(props) = schema.get("properties").and_then(Value::as_object) else {
            return Ok(());
        };
        // Исторические имена пути — синонимы каноничного `path` (Н8), а не
        // незнакомые аргументы: старые клиенты и скрипты обязаны работать.
        let path_accepted = props.contains_key("path");
        let known = |k: &String| {
            props.contains_key(k) || (path_accepted && PATH_ARG_ALIASES.contains(&k.as_str()))
        };
        let unknown: Vec<&String> = map.keys().filter(|k| !known(k)).collect();
        if unknown.is_empty() {
            return Ok(());
        }
        let mut allowed: Vec<&str> = props.keys().map(String::as_str).collect();
        allowed.sort_unstable();
        let mut hint = String::new();
        if allowed.contains(&"path") {
            hint.push_str(
                " (путь во всех инструментах называется `path`; исторические `dir`,                  `repo`, `case`, `change_dir` принимаются как синонимы)",
            );
        }
        Err(CallError::invalid_params(format!(
            "{name}: неизвестный аргумент {}; допустимые: {}{hint}",
            unknown
                .iter()
                .map(|k| format!("'{k}'"))
                .collect::<Vec<_>>()
                .join(", "),
            allowed.join(", ")
        )))
    }

    /// Маршрутизация вызова по имени инструмента: сначала ручные
    /// реализации (оттестированная поверхность ADR-008), затем мост в
    /// реестр по белым спискам режима, иначе — `-32602`.
    async fn dispatch_tool(
        &self,
        name: &str,
        args: Value,
    ) -> std::result::Result<DispatchOutcome, CallError> {
        // Н8: неизвестный аргумент — ошибка вызова с ПЕРЕЧНЕМ допустимых, а не
        // молчаливый игнор (serde пропускает незнакомые поля, и опечатка в
        // имени выглядела как «инструмент не сработал»).
        Self::reject_unknown_args(name, &args)?;
        match name {
            "spine_lint" => self
                .tool_spine_lint(args)
                .await
                .map(DispatchOutcome::Structured),
            "fitness_check" => self
                .tool_fitness_check(args)
                .await
                .map(DispatchOutcome::Structured),
            "significance_score" => {
                Self::tool_significance_score(args).map(DispatchOutcome::Structured)
            }
            "significance_from_diff" => self
                .tool_significance_from_diff(args)
                .await
                .map(DispatchOutcome::Structured),
            "trace_check" => self
                .tool_trace_check(args)
                .await
                .map(DispatchOutcome::Structured),
            "model_query" => self
                .tool_model_query(args)
                .await
                .map(DispatchOutcome::Structured),
            "rubric_run" => self
                .tool_rubric_run(args)
                .await
                .map(DispatchOutcome::Structured),
            // Split-judge без LLM у сервера: промпты судьи наружу,
            // механическая сборка отчёта из ответов хоста.
            "rubric_prompt" => self
                .tool_rubric_prompt(args)
                .await
                .map(DispatchOutcome::Structured),
            "rubric_verify" => self
                .tool_rubric_verify(args)
                .await
                .map(DispatchOutcome::Structured),
            // T4 (ADR-015): чтение знаний наружу; тонкие адаптеры к ядру
            // агентных инструментов kb.rs/plugin.rs/mermaid.rs (общая логика
            // живёт там — MCP-слой только парсит аргументы и формирует JSON).
            "kb_search" => self
                .tool_kb_search(args)
                .await
                .map(DispatchOutcome::Structured),
            "skill_search" => self
                .tool_skill_search(args)
                .await
                .map(DispatchOutcome::Structured),
            "skill_load" => self
                .tool_skill_load(args)
                .await
                .map(DispatchOutcome::Structured),
            "mermaid_render" => self
                .tool_mermaid_render(args)
                .await
                .map(DispatchOutcome::Structured),
            // Кандидатные fitness-правила из пробелов кейса (read-only
            // эвристики, src/rules_suggest.rs).
            "rules_suggest" => self
                .tool_rules_suggest(args)
                .await
                .map(DispatchOutcome::Structured),
            // Паспорт вердикта (W1): вердикт гейта + его границы.
            "verdict_explain" => self
                .tool_verdict_explain(args)
                .await
                .map(DispatchOutcome::Structured),
            // Метрика доверия к контуру (W4): место на шкале 1–5 с якорями
            // и доказательствами. Поверх журнала вызовов — того самого,
            // который пишет этот сервер.
            "trust_report" => self
                .tool_trust_report(args)
                .await
                .map(DispatchOutcome::Structured),
            // Мост в реестр инструментов харнесса (белые списки режима).
            other if self.bridge_allowed(other) => self.bridge_dispatch(other, args).await,
            other => Err(CallError::invalid_params(format!(
                "неизвестный инструмент '{other}' (список — tools/list)"
            ))),
        }
    }

    /// Мостовой вызов: маршрутизация в [`ToolRegistry::dispatch`] с
    /// [`ToolContext`] БЕЗ LLM (инструменты, требующие модель, в белые
    /// списки не входят). `cwd` — дополнительный аргумент моста: рабочий
    /// каталог клиента, от которого резолвятся относительные пути
    /// (по умолчанию — cwd процесса сервера, поведение ручных инструментов).
    /// Решение политики R-уровней (Deny/RequireConfirm) приходит из
    /// `dispatch` как `ToolOutput::err` и отдаётся доменным isError
    /// с текстом причины: подтверждение в неинтерактивном MCP невозможно,
    /// `RequireConfirm` трактуется как отказ.
    async fn bridge_dispatch(
        &self,
        name: &str,
        args: Value,
    ) -> std::result::Result<DispatchOutcome, CallError> {
        if self.registry.get(name).is_none() {
            return Err(CallError::invalid_params(format!(
                "{name}: инструмент отключён конфигом сервера (см. [archify]/[web] enabled)"
            )));
        }
        let cwd = args
            .get("cwd")
            .and_then(Value::as_str)
            .map_or_else(|| PathBuf::from("."), PathBuf::from);
        let ctx = ToolContext::new(cwd, Arc::clone(&self.cfg)).with_exec(self.exec.clone());
        let out = self.registry.dispatch(name, args, &ctx).await;
        if out.is_error {
            return Err(CallError::Execution(out.content));
        }
        // Разобранный результат (T-12): инструмент отдаёт его полем `data`,
        // а если структурной формы нет — пробуем разобрать сам текст (часть
        // инструментов отвечает JSON-вердиктом строкой). Строковое `output`
        // сохраняется: на него опираются клиенты, написанные раньше.
        let structured_data = out.data.clone().or_else(|| {
            serde_json::from_str::<Value>(&out.content)
                .ok()
                .filter(Value::is_object)
        });
        let text = out.truncated(BRIDGE_OUTPUT_MAX_CHARS).content;
        let structured = match structured_data {
            Some(Value::Object(mut map)) => {
                map.insert("tool".to_string(), json!(name));
                map.insert("output".to_string(), json!(text.clone()));
                Value::Object(map)
            }
            _ => json!({"tool": name, "output": text.clone()}),
        };
        Ok(DispatchOutcome::Text { structured, text })
    }
}

/// Текст и описание плейбука-промпта: сначала пользовательская копия из
/// `plugins.dirs` (та же логика, что у `skill_load`, — правки пользователя
/// в силе), иначе встроенный ассет (чистая машина без `arch-be init`).
/// Ошибка чтения пользовательской копии не фатальна — откат на встроенный
/// текст: сервер не падает из-за одного битого файла.
fn resolve_playbook(dirs: &[PathBuf], pb: &PlaybookPrompt) -> (String, String) {
    let plugins = plugin::discover(dirs);
    if let Some(meta) = plugin::skill_by_name(&plugins, pb.name) {
        if let Ok(text) = plugin::load_skill(meta) {
            return (text, meta.description.clone());
        }
    }
    let description = plugin::parse_frontmatter_text(pb.embedded)
        .map(|(_, d)| d)
        .unwrap_or_default();
    (pb.embedded.to_string(), description)
}

/// Спецификации ручных инструментов для `tools/list` (имена и аргументы —
/// ADR-008; порядок первых десяти зафиксирован тестами).
// Декларативная таблица: дробление на fn-по-инструменту ухудшит обзорность.
#[expect(clippy::too_many_lines, reason = "декларативная таблица спецификаций")]
/// Схемы ручных инструментов. Правило поверхности: поле, которое разбирает
/// реализация, ОБЯЗАНО быть в схеме — иначе `reject_unknown_args` (Н8)
/// отвергнет вызов, который инструмент умеет обслужить, и поле станет
/// недостижимым для всех клиентов (Н13: `rubric_verify.author_model`).
/// Заодно тестами зафиксирован обратный край: объявленный обязательный
/// аргумент обязан разбираться.
fn tool_specs() -> Vec<Value> {
    let read_only = json!({
        "readOnlyHint": true,
        "destructiveHint": false,
        "idempotentHint": true,
        "openWorldHint": false,
    });
    vec![
        json!({
            "name": "spine_lint",
            "description": "Линтер ARCHITECTURE-SPINE.md: дубли AD-id, пустые/отсутствующие \
                            Binds/Prevents/Rule, заглушки (TODO/TBD), непиннутые версии, ссылки \
                            на несуществующие AD. Verdict: passed=false (есть находки error) — \
                            spine нарушен, отказать изменению с перечнем находок",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Путь к ARCHITECTURE-SPINE.md"}
                },
                "required": ["path"],
            },
            "annotations": read_only,
        }),
        json!({
            "name": "fitness_check",
            "description": "Fitness-контроль репозитория по CONSTRAINTS.yaml: must_contain / \
                            must_not_contain (regex по glob), file_exists, command_succeeds \
                            (с таймаутом). Вызывать ПЕРЕД коммитом: verdict passed=false — \
                            изменение нарушает архитектурные правила (AD-*), отказать и \
                            перечислить находки. Модель доверия (ADR-053): в MCP-режиме \
                            command_succeeds по умолчанию НЕ исполняются (no-exec) — такие \
                            правила возвращаются пропусками command_untrusted (поле \
                            untrusted_skipped); снятие — ARCH_NO_EXEC=0 в окружении сервера, \
                            доверие реестру — `arch-be rules allow` (CLI)",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Корень репозитория"},
                    "constraints": {
                        "type": "string",
                        "description": "Путь к CONSTRAINTS.yaml (по умолчанию <repo>/.arch-handoff/CONSTRAINTS.yaml)",
                    },
                    "base": {
                        "type": "string",
                        "description": "База git для сверки состава правил (анти-ослабление, П5): по умолчанию merge-base с основной веткой, иначе HEAD",
                    },
                },
                "required": ["path"],
            },
            "annotations": read_only,
        }),
        json!({
            "name": "significance_score",
            "description": "Architecture Significance Score по 15 триггерам → маршрут изменения: \
                            Fast (0–1), Standard (2–4), Critical (5+ или критические триггеры \
                            security_boundary_change / irreversible_migration / \
                            criticality_or_exception)",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "triggers": {
                        "oneOf": [
                            {"type": "object", "additionalProperties": {"type": "boolean"}},
                            {"type": "array", "items": {"type": "string"}},
                        ],
                        "description": "Триггеры: карта «триггер → true/false» (ключи — из 15 канонических) ЛИБО массив строк \"name=true\" / \"name=false\" / голое \"name\" (= true). Незнакомое имя — ошибка вызова (-32602) с перечнем канонических триггеров и ближайшим совпадением: выдуманный триггер не поднимает маршрут",
                    }
                },
                "required": ["triggers"],
            },
            "annotations": read_only,
        }),
        json!({
            "name": "trace_check",
            "description": "Трассируемость как fitness-функция: покрытие звеньев REQ → NFR → \
                            AD/ADR → CMP → правило CONSTRAINTS.yaml, поимённые сироты, сверка \
                            модели с ARCHITECTURE-SPINE.md. AD без правила и без unverifiable — \
                            error. Verdict: passed=false — отказать изменению; report_markdown \
                            пригоден для evidence bundle",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Корень кейса (каталог с model/)"}
                },
                "required": ["path"],
            },
            "annotations": read_only,
        }),
        json!({
            "name": "model_query",
            "description": "Запрос к типизированной модели архитектуры (каталог model/): \
                            карточка сущности по id со связями и обратными ссылками, либо \
                            список сущностей (фильтр по типу: cap, sys, cmp, int, nfr, req, \
                            ad, adr, risk, owner)",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Корень кейса или каталог `model/` — инструмент находит модель сам (по умолчанию `model` от cwd сервера)"},
                    "id": {"type": "string", "description": "ID сущности (ADR-001, CMP-002, …): карточка со связями"},
                    "type": {"type": "string", "description": "Фильтр списка по типу (cmp, adr, …)"},
                },
            },
            "annotations": read_only,
        }),
        json!({
            "name": "rubric_run",
            "description": "Оценка документа рубрикой архитектурного контроля через LLM-судью \
                            (evidence-bound, ADR-004; требует API-ключ провайдера из конфига \
                            arch — без ключа понятная JSON-RPC ошибка). Укажите ровно один из \
                            target / target_text",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "rubric": {"type": "string", "description": "Рубрика: имя в каталоге рубрик arch или путь к YAML"},
                    "target": {"type": "string", "description": "Путь к оцениваемому документу (md/txt)"},
                    "target_text": {"type": "string", "description": "Текст документа inline (альтернатива target)"},
                    "model": {"type": "string", "description": "Модель-судья (имя из [models]; по умолчанию — дефолтная)"},
                    "cwd": {"type": "string", "description": "Рабочий каталог клиента: относительный target резолвится от него (по умолчанию — cwd процесса сервера)"},
                },
                "required": ["rubric"],
            },
            "annotations": {
                "readOnlyHint": true,
                "destructiveHint": false,
                "idempotentHint": false,
                "openWorldHint": true,
            },
        }),
        // T4 (ADR-015): чтение знаний наружу. Только чтение — write/exec
        // инструменты агенту НЕ отдаются (см. тест write_tools_are_not_exposed).
        json!({
            "name": "kb_search",
            "description": "Поиск по локальной базе знаний архитектора (каталоги \
                            knowledge.dirs из конфига arch): ранжированные хиты — \
                            путь, строка, сниппет с контекстом. Вызывай, когда \
                            нужен доменный материал (ADRs, заметки, статьи)",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": {"type": "string", "description": "Поисковый запрос: термины через пробел"},
                    "limit": {"type": "integer", "description": "Максимум хитов (по умолчанию 10, не больше 20)"},
                },
                "required": ["query"],
            },
            "annotations": read_only,
        }),
        json!({
            "name": "skill_search",
            "description": "Поиск по библиотеке архитектурных скиллов (плагины \
                            arch: навыки, MCP, субагенты). Вызывай, когда нужна \
                            методика по теме (ADR, saga, NFR, рубрики…)",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": {"type": "string", "description": "Поисковый запрос"},
                    "limit": {"type": "integer", "description": "Максимум результатов (по умолчанию 8, не больше 20)"},
                },
                "required": ["query"],
            },
            "annotations": read_only,
        }),
        json!({
            "name": "skill_load",
            "description": "Загрузить полный текст архитектурного скилла по точному \
                            имени (после skill_search). Скилл — методика: приёмы, \
                            чек-листы, антипаттерны",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": {"type": "string", "description": "Точное имя скилла"},
                },
                "required": ["name"],
            },
            "annotations": read_only,
        }),
        json!({
            "name": "mermaid_render",
            "description": "Рендерит mermaid-диаграмму (flowchart, sequenceDiagram, \
                            erDiagram или C4Context/C4Container/C4Component) в \
                            ASCII-арт. Вход: 'code' (исходник) или 'path' (путь к \
                            .mmd-файлу относительно cwd сервера)",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "code": {"type": "string", "description": "Исходный код mermaid-диаграммы"},
                    "path": {"type": "string", "description": "Путь к .mmd-файлу (резолвится от cwd сервера)"},
                },
            },
            "annotations": read_only,
        }),
        // Split-judge (механический судья без LLM у сервера): хост исполняет
        // промпт своей моделью, сервер собирает отчёт тем же кодом, что
        // у встроенного судьи rubric_run (медиана, unstable, evidence_not_found).
        json!({
            "name": "rubric_prompt",
            "description": "Split-judge, фаза 1 (без API-ключа): собирает system+user промпты \
                            архитектурного судьи по рубрике и целевому документу + JSON-схему \
                            ответа + judge_config (число сэмплов k). Вместо документа можно \
                            указать досье (pack+subject) — вход смысловой рубрики, который \
                            собирается из репозитория вместе с хэшами источников. Выполните \
                            промпт k раз СВОЕЙ моделью и передайте сырые ответы массивом \
                            'answers' в rubric_verify с теми же rubric и target/target_text \
                            (или pack/subject/root)",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "rubric": {"type": "string", "description": "Рубрика: имя в каталоге рубрик arch или путь к YAML"},
                    "target": {"type": "string", "description": "Путь к оцениваемому документу (md/txt)"},
                    "target_text": {"type": "string", "description": "Текст документа inline (альтернатива target)"},
                    "pack": {"type": "string", "description": "Вид досье смысловой рубрики (ADR-051): adr_vs_spine | entity_links | nfr_mechanism | code_vs_spine; взаимоисключающ с target/target_text"},
                    "subject": {"type": "string", "description": "Субъект досье: путь к ADR или файлу кода (можно с диапазоном строк, 'src/gate.rs#12-88') либо идентификатор сущности модели (CMP-001)"},
                    "root": {"type": "string", "description": "Опц.: корень репозитория для сборки досье (по умолчанию — текущий каталог)"},
                    "dynamic_subject": {"type": "string", "description": "НЕ поддерживается в MCP-режиме (нужен LLM у сервера): сгенерируйте рубрику своей моделью и передайте путь в rubric"},
                },
                "required": ["rubric"],
            },
            "annotations": read_only,
        }),
        json!({
            "name": "rubric_verify",
            "description": "Split-judge, фаза 2 (без API-ключа): принимает сырые ответы вашей \
                            модели на промпт rubric_prompt (массив строк 'answers') и строит \
                            отчёт рубрики: медиана баллов по сэмплам, метки unstable (разброс) \
                            и evidence_not_found (цитата не подтверждена target'ом). Битые \
                            ответы отбрасываются со счётчиком в answers.dropped. Под `--rw` \
                            отчёт ложится в reports/rubric/ и его находит гейт: для досье — \
                            с хэшем досье и поимёнными хэшами источников (ADR-051)",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "rubric": {"type": "string", "description": "Та же рубрика, что в rubric_prompt"},
                    "target": {"type": "string", "description": "Тот же документ (путь), что судился — для проверки цитат"},
                    "target_text": {"type": "string", "description": "Тот же текст inline (альтернатива target)"},
                    "pack": {"type": "string", "description": "Тот же вид досье, что в rubric_prompt (ADR-051): adr_vs_spine | entity_links | nfr_mechanism | code_vs_spine"},
                    "subject": {"type": "string", "description": "Тот же субъект досье, что в rubric_prompt (с диапазоном строк, если досье фрагментировано)"},
                    "root": {"type": "string", "description": "Опц.: корень репозитория для сборки досье — тот же, что в rubric_prompt"},
                    "answers": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "Сырые ответы модели хоста (каждый — JSON судьи по response_json_schema)",
                    },
                    "model": {"type": "string", "description": "Опц.: метка судьи для отчёта (имя модели хоста)"},
                    "judge_model": {"type": "string", "description": "Опц.: метка судьи, перекрывает model — фиксируйте фактическую модель-судью (anti-bias «автор = судья»: судья ДОЛЖЕН отличаться от модели-автора документа)"},
                    "author_model": {"type": "string", "description": "Опц.: модель-АВТОР документа — если совпадает с судьёй, отчёт помечается «судья судил свою работу» (Н7, ADR-042); попадает в отчёт рубрики и в находку judge_is_author составляющей гейта decision_quality"},
                },
                "required": ["rubric", "answers"],
            },
            "annotations": read_only,
        }),
        // Anti-bypass floor (S-1, ADR-034): маршрут из механики диффа, а не из
        // самооценки агента. В конце vec — порядок первых 12 ручных
        // инструментов зафиксирован тестами.
        json!({
            "name": "significance_from_diff",
            "description": "Маршрут значимости Fast/Standard/Critical, выведенный из git-диффа \
                            репозитория (anti-bypass S-1, ADR-034): детекторы new_component / \
                            new_vendor / api_contract_change / irreversible_migration / \
                            new_datastore объединяются с заявленными 'declared' (детектор \
                            только добавляет). Ответ: route+score, sources каждого триггера \
                            (declared/diff/declared+diff), undeclared — найденные диффом, но \
                            не заявленные триггеры с файлами-основаниями. Информационный \
                            инструмент (без passed)",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Корень git-репозитория (по умолчанию — рабочий каталог процесса сервера)",
                    },
                    "base_ref": {
                        "type": "string",
                        "description": "Опц.: база диффа — голая ревизия (git diff BASE_REF...HEAD) или готовый диапазон A...HEAD как есть; без неё — рабочее дерево против HEAD (staged + unstaged + untracked)",
                    },
                    "declared": {
                        "type": "object",
                        "description": "Опц.: заявленные триггеры («триггер → true/false», ключи — из 15 канонических, как у significance_score). Незнакомое имя — ошибка вызова (-32602): опечатка молча оставила бы настоящий триггер незаявленным",
                        "additionalProperties": {"type": "boolean"},
                    },
                },
            },
            "annotations": read_only,
        }),
        // Кандидатные fitness-правила из пробелов кейса (src/rules_suggest.rs).
        // В конце vec — порядок первых 12 ручных инструментов зафиксирован
        // тестами.
        json!({
            "name": "rules_suggest",
            "description": "Кандидатные fitness-правила из содержательных пробелов кейса \
                            (read-only эвристики): EARS-критерии приёмки, численные таймауты \
                            в контрактах, декомпозиция REQ→работы, RTO/RPO без ADR, аудит \
                            операторских действий. Ответ: candidates (id, rationale, \
                            source_skill, yaml — готовый фрагмент CONSTRAINTS.yaml или null \
                            для честного advisory) + report_markdown. Информационный \
                            инструмент, без passed",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Корень кейса (каталог с docs/, model/, .arch-handoff/)"},
                    "cwd": {"type": "string", "description": "Рабочий каталог клиента: относительный path резолвится от него (по умолчанию — cwd процесса сервера)"},
                },
                "required": ["path"],
            },
            "annotations": read_only,
        }),
        json!({
            "name": "trust_report",
            "description": "Метрика доверия к контуру: положение на шкале 1–5 с ЯКОРЯМИ и \
                            ДОКАЗАТЕЛЬСТВАМИ — контур подключён (журнал вызовов), гейт \
                            останавливал работу (fail → починка), правила сопровождаются \
                            (владелец, срок, проверка поведения), пакет защищён измеренно \
                            (доля обнаружения redteam), вердикт полон и подписан. У каждого \
                            якоря: чем подтверждён и почему не достигнут. Ничего не \
                            блокирует: отвечает, насколько можно верить зелёному контура. \
                            Прогон гейта внутри оценки подчиняется модели доверия (ADR-053): \
                            command_succeeds по умолчанию не исполняются (no-exec, снятие — \
                            ARCH_NO_EXEC=0 в окружении сервера)",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Репозиторий или кейс (по умолчанию — каталог вызова)"},
                    "cwd": {"type": "string", "description": "Рабочий каталог клиента: относительный path резолвится от него (по умолчанию — cwd процесса сервера)"},
                },
            },
            "annotations": read_only,
        }),
        json!({
            "name": "verdict_explain",
            "description": "Паспорт вердикта: прогон гейта + страница «что зелёный НЕ означает» \
                            в трёх блоках — проверено (составляющие с числами), заявлено, но \
                            механикой не проверяется (подпись A3, семантика ссылок, независимость \
                            судьи и ревьюера, адекватность решения), не проверено (SKIP с \
                            причиной). Отвечает тем же вердиктом, что `arch-be gate`, и его \
                            границами; решения не принимает. Модель доверия (ADR-053): в \
                            MCP-режиме command_succeeds по умолчанию не исполняются (no-exec) — \
                            блок «не проверено» перечисляет их как command_untrusted; снятие — \
                            ARCH_NO_EXEC=0 в окружении сервера",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Репозиторий (по умолчанию — каталог вызова)"},
                    "route": {"type": "string", "description": "auto (по умолчанию) | fast | standard | critical"},
                    "base": {"type": "string", "description": "База git для диффа (по умолчанию — рабочее дерево против HEAD)"},
                    "constraints": {"type": "string", "description": "Файл ограничений (по умолчанию <repo>/.arch-handoff/CONSTRAINTS.yaml)"},
                    "cwd": {"type": "string", "description": "Рабочий каталог клиента: относительный path резолвится от него (по умолчанию — cwd процесса сервера)"},
                },
            },
            "annotations": read_only,
        }),
    ]
}

/// Полный список спецификаций `tools/list`: ручные инструменты (см.
/// [`tool_specs`]) + мостовые по белым спискам текущего режима.
impl McpServe {
    /// `tools/list` текущего режима: ручные + мостовые (сортированы по имени).
    fn all_tool_specs(&self) -> Vec<Value> {
        let mut specs = tool_specs();
        specs.extend(self.bridge_tool_specs());
        specs
    }

    /// MCP-спеки мостовых инструментов: генерируются из `Tool::spec()`
    /// реестра (name/description/parameters) + annotations по членству в
    /// списках и классу риска [`crate::policy::classify_tool`]. В схему
    /// каждого добавляется опциональный аргумент `cwd` моста. Инструменты,
    /// отключённые конфигом (напр. `[archify].enabled = false`), пропускаются.
    fn bridge_tool_specs(&self) -> Vec<Value> {
        let mut names: Vec<&str> = Vec::new();
        names.extend_from_slice(BRIDGE_READ_ONLY);
        if self.mode.allows_write() {
            names.extend_from_slice(BRIDGE_READ_WRITE);
        }
        names.sort_unstable();
        let mut out = Vec::with_capacity(names.len());
        for name in names {
            let Some(tool) = self.registry.get(name) else {
                continue;
            };
            let spec = tool.spec();
            let mut parameters = spec.parameters;
            if let Some(props) = parameters
                .get_mut("properties")
                .and_then(Value::as_object_mut)
            {
                props.insert(
                    "cwd".into(),
                    json!({
                        "type": "string",
                        "description": "Рабочий каталог клиента: относительные пути вызова \
                                        резолвятся от него (по умолчанию — cwd процесса сервера)",
                    }),
                );
            }
            out.push(json!({
                "name": spec.name,
                "description": spec.description,
                "inputSchema": parameters,
                "annotations": bridge_annotations(&spec.name),
            }));
        }
        out
    }
}

/// Аннотации MCP для мостового инструмента: `readOnlyHint` — по членству в
/// rw-списке (честно о записи: rw-инструменты пишут в рабочий каталог
/// клиента, даже если политика считает их `ReadOnly`), `destructiveHint` — по
/// классу риска из [`crate::policy::classify_tool`] (`Mutating`+ → true;
/// аддитивные записи вроде `skill_distill`/`archify_*` — false).
fn bridge_annotations(name: &str) -> Value {
    let class = crate::policy::classify_tool(name, &Value::Null);
    let mutating = !BRIDGE_READ_ONLY.contains(&name);
    json!({
        "readOnlyHint": !mutating,
        "destructiveHint": class != crate::policy::RiskClass::ReadOnly,
        "idempotentHint": !mutating,
        "openWorldHint": false,
    })
}

/// Цикл сервера поверх произвольных AsyncRead/AsyncWrite: строка → ответ
/// (или молчание на уведомление), flush на каждый ответ, выход по EOF
/// либо по ошибке чтения (транспорт мёртв — сервер завершается чисто).
pub(super) async fn run_loop<R, W>(server: &McpServe, reader: R, mut writer: W) -> Result<()>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut lines = BufReader::new(reader).lines();
    loop {
        match lines.next_line().await {
            Ok(Some(line)) => {
                if line.trim().is_empty() {
                    continue; // пустые строки — не сообщения, пропускаем молча
                }
                let response = if line.len() > MAX_LINE_BYTES {
                    Some(error_response(
                        &Value::Null,
                        INVALID_REQUEST,
                        format!("строка длиннее лимита {MAX_LINE_BYTES} байт"),
                    ))
                } else {
                    server.handle_line(&line).await
                };
                if let Some(response) = response {
                    let mut payload = response.to_string();
                    payload.push('\n');
                    writer.write_all(payload.as_bytes()).await?;
                    writer.flush().await?;
                }
            }
            Ok(None) => break,
            Err(e) => {
                tracing::warn!(error = %e, "mcp-serve: ошибка чтения stdin, завершение");
                break;
            }
        }
    }
    writer.flush().await?;
    Ok(())
}

/// Точка входа `arch-be mcp serve`: цикл на stdin/stdout процесса,
/// read-only режим (поведение по умолчанию).
///
/// # Errors
/// Запись в stdout оборвалась (клиент умер) — сервер завершается с ошибкой
/// транспорта; входной мусор ошибкой не является (ответ `-32700` и дальше).
pub async fn serve(cfg: Arc<Config>) -> Result<()> {
    serve_with_mode(cfg, ServeMode::ReadOnly).await
}

/// Точка входа `arch-be mcp serve [--rw]`: цикл на stdin/stdout процесса
/// в явном режиме [`ServeMode`].
///
/// # Errors
/// Запись в stdout оборвалась (клиент умер) — сервер завершается с ошибкой
/// транспорта; входной мусор ошибкой не является (ответ `-32700` и дальше).
pub async fn serve_with_mode(cfg: Arc<Config>, mode: ServeMode) -> Result<()> {
    let server = McpServe::with_mode(cfg, mode);
    run_loop(&server, tokio::io::stdin(), tokio::io::stdout()).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp_server::testkit::*;
    use crate::mcp_server::types::*;

    #[tokio::test]
    async fn initialize_echoes_known_protocol_and_advertises_tools() {
        for (asked, want) in [
            ("2025-06-18", "2025-06-18"),
            ("2024-11-05", "2024-11-05"),
            ("1999-01-01", "2025-06-18"),
        ] {
            let responses = run_lines(&[&format!(
                r#"{{"jsonrpc":"2.0","id":1,"method":"initialize","params":{{"protocolVersion":"{asked}","capabilities":{{}},"clientInfo":{{"name":"t","version":"0"}}}}}}"#
            )])
            .await;
            assert_eq!(responses.len(), 1);
            let result = &responses[0]["result"];
            assert_eq!(result["protocolVersion"], want, "версия для {asked}");
            assert_eq!(result["serverInfo"]["name"], "arch-harness");
            assert!(result["capabilities"]["tools"].is_object());
            assert!(
                result["capabilities"]["prompts"].is_object(),
                "capability prompts (плейбуки spine-* как слэш-команды)"
            );
        }
    }

    #[tokio::test]
    async fn notifications_and_empty_lines_get_no_response() {
        let responses = run_lines(&[
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
            r#"{"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":1}}"#,
            r#"{"jsonrpc":"2.0","method":"unknown_notification"}"#,
            "",
            "   ",
            r#"{"jsonrpc":"2.0","id":7,"method":"ping"}"#,
        ])
        .await;
        assert_eq!(responses.len(), 1, "ответ только на ping: {responses:?}");
        assert_eq!(responses[0]["id"], 7);
        assert_eq!(responses[0]["result"], json!({}));
    }

    #[tokio::test]
    async fn broken_json_gives_32700_and_loop_continues() {
        let responses = run_lines(&[
            "{это не json",
            r#"{"jsonrpc":"2.0","id":2,"method":"ping"}"#,
        ])
        .await;
        assert_eq!(responses.len(), 2, "сервер пережил битую строку");
        assert_eq!(responses[0]["error"]["code"], PARSE_ERROR);
        assert_eq!(responses[0]["id"], Value::Null);
        assert_eq!(responses[1]["result"], json!({}));
    }

    #[tokio::test]
    async fn unknown_method_gives_32601_with_any_id_type() {
        let responses = run_lines(&[
            r#"{"jsonrpc":"2.0","id":"abc-1","method":"resources/list"}"#,
            "[1,2,3]",
            r#"{"jsonrpc":"2.0","id":3}"#,
        ])
        .await;
        assert_eq!(responses[0]["error"]["code"], METHOD_NOT_FOUND);
        assert_eq!(responses[0]["id"], "abc-1", "строковый id эхом");
        assert_eq!(responses[1]["error"]["code"], INVALID_REQUEST);
        assert_eq!(responses[2]["error"]["code"], INVALID_REQUEST);
        assert_eq!(responses[2]["id"], 3);
    }

    #[tokio::test]
    async fn tools_list_read_only_mode_manual_first_then_bridge() {
        let responses =
            run_lines(&[r#"{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}"#]).await;
        let tools = responses[0]["result"]["tools"].as_array().expect("tools");
        let names: Vec<&str> = tools
            .iter()
            .map(|t| t["name"].as_str().expect("имя"))
            .collect();
        // Первые 10 — ручные инструменты в зафиксированном порядке (ADR-008);
        // далее — split-judge (ручные) и мостовые read-only.
        assert_eq!(
            names[..10],
            [
                "spine_lint",
                "fitness_check",
                "significance_score",
                "trace_check",
                "model_query",
                "rubric_run",
                "kb_search",
                "skill_search",
                "skill_load",
                "mermaid_render"
            ]
        );
        assert_eq!(names[10..12], ["rubric_prompt", "rubric_verify"]);
        for bridged in BRIDGE_READ_ONLY {
            assert!(
                names.contains(bridged),
                "мостовой read-only '{bridged}' обязан быть в tools/list: {names:?}"
            );
        }
        assert_eq!(
            names.len(),
            MANUAL_TOOLS.len() + BRIDGE_READ_ONLY.len(),
            "ro-режим: ручные + split-judge + read-only мост"
        );
        for t in tools {
            assert_eq!(t["annotations"]["readOnlyHint"], true, "{}", t["name"]);
            assert!(t["inputSchema"].is_object(), "{}", t["name"]);
        }
        // У мостовых спек есть дополнительный аргумент моста `cwd`.
        let openapi = tools
            .iter()
            .find(|t| t["name"] == "openapi_lint")
            .expect("openapi_lint");
        assert!(
            openapi["inputSchema"]["properties"]["cwd"].is_object(),
            "аргумент cwd в мостовой спеке: {openapi}"
        );
        assert!(
            openapi["inputSchema"]["properties"]["path"].is_object(),
            "схема инструмента из Tool::spec(): {openapi}"
        );
    }

    #[tokio::test]
    async fn write_and_exec_tools_are_not_exposed() {
        // Read-only дисциплина (T4, ADR-015): сервер отдаёт только чтение —
        // ни одного write/exec-инструмента агентного реестра наружу.
        let responses =
            run_lines(&[r#"{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}"#]).await;
        let names: Vec<&str> = responses[0]["result"]["tools"]
            .as_array()
            .expect("tools")
            .iter()
            .map(|t| t["name"].as_str().expect("имя"))
            .collect();
        for forbidden in BRIDGE_NEVER {
            assert!(
                !names.contains(forbidden),
                "write/exec-инструмент '{forbidden}' не должен отдаваться наружу: {names:?}"
            );
        }
        // rw-контур в read-only режиме закрыт: ни в списке, ни вызовом.
        for rw in BRIDGE_READ_WRITE {
            assert!(
                !names.contains(rw),
                "rw-инструмент '{rw}' не должен отдаваться без --rw: {names:?}"
            );
        }
    }

    #[tokio::test]
    async fn unknown_tool_and_bad_arguments_give_32602() {
        let responses = run_lines(&[
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"ghost","arguments":{}}}"#,
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"spine_lint","arguments":{}}}"#,
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"fitness_check","arguments":{"repo":42}}}"#,
            r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"ping"}}"#,
            r#"{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"arguments":{}}}"#,
        ])
        .await;
        for (i, r) in responses.iter().enumerate() {
            assert_eq!(r["error"]["code"], INVALID_PARAMS, "ответ {}: {r}", i + 1);
        }
        assert!(
            responses[0]["error"]["message"]
                .as_str()
                .expect("сообщение")
                .contains("ghost")
        );
    }

    #[tokio::test]
    async fn domain_failure_is_is_error_result_not_crash() {
        let responses = run_lines(&[
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"spine_lint","arguments":{"path":"/нет/такого/spine.md"}}}"#,
            r#"{"jsonrpc":"2.0","id":2,"method":"ping"}"#,
        ])
        .await;
        assert_eq!(responses[0]["result"]["isError"], true);
        let text = responses[0]["result"]["content"][0]["text"]
            .as_str()
            .expect("text");
        assert!(text.contains("spine_lint"), "{text}");
        assert_eq!(responses[1]["result"], json!({}), "сервер жив после сбоя");
    }

    #[tokio::test]
    async fn prompts_list_has_nine_playbooks_with_frontmatter_descriptions() {
        // Пустой plugins-каталог → встроенные ассеты (чистая машина).
        let tmp = tempfile::tempdir().expect("tmp");
        let server = server_with_dirs(tmp.path(), tmp.path());
        let responses = run_lines_on(
            server,
            &[r#"{"jsonrpc":"2.0","id":1,"method":"prompts/list","params":{}}"#],
        )
        .await;
        let prompts = responses[0]["result"]["prompts"]
            .as_array()
            .expect("prompts");
        let names: Vec<&str> = prompts
            .iter()
            .map(|p| p["name"].as_str().expect("name"))
            .collect();
        assert_eq!(
            names,
            [
                "spine-quickstart",
                "spine-content-bootstrap",
                "spine-architect-review",
                "spine-adr-judge",
                "spine-semantic-judge",
                "spine-contracts-gate",
                "spine-archify-viz",
                "spine-fitness-gate",
                "spine-bundle",
                "spine-judge-handover",
            ],
            "десять плейбуков в зафиксированном порядке"
        );
        for p in prompts {
            assert!(
                p["description"].as_str().is_some_and(|d| !d.is_empty()),
                "description из frontmatter: {p}"
            );
            assert!(p["arguments"].is_array(), "arguments — массив: {p}");
        }
        // Аргументы объявлены только у параметризованных плейбуков.
        let arg_count = |name: &str| {
            prompts.iter().find(|p| p["name"] == name).expect("промпт")["arguments"]
                .as_array()
                .expect("args")
                .len()
        };
        assert_eq!(arg_count("spine-quickstart"), 0);
        assert_eq!(arg_count("spine-adr-judge"), 2, "target + rubric");
        assert_eq!(arg_count("spine-contracts-gate"), 3, "path + old + new");
        assert_eq!(arg_count("spine-archify-viz"), 1, "subject");
    }

    #[tokio::test]
    async fn prompts_get_renders_every_embedded_playbook() {
        let tmp = tempfile::tempdir().expect("tmp");
        for pb in PLAYBOOK_PROMPTS {
            let server = server_with_dirs(tmp.path(), tmp.path());
            let line = format!(
                r#"{{"jsonrpc":"2.0","id":1,"method":"prompts/get","params":{{"name":"{}"}}}}"#,
                pb.name
            );
            let responses = run_lines_on(server, &[&line]).await;
            let result = &responses[0]["result"];
            assert!(
                result["description"]
                    .as_str()
                    .is_some_and(|d| !d.is_empty()),
                "{}: description из frontmatter",
                pb.name
            );
            let messages = result["messages"].as_array().expect("messages");
            assert_eq!(messages.len(), 1, "{}: одно user-сообщение", pb.name);
            assert_eq!(messages[0]["role"], "user", "{}", pb.name);
            assert_eq!(messages[0]["content"]["type"], "text", "{}", pb.name);
            let text = messages[0]["content"]["text"].as_str().expect("text");
            assert!(
                text.starts_with("Действуй по этому плейбуку"),
                "{}: инструкция-команда",
                pb.name
            );
            assert!(
                text.contains(pb.embedded),
                "{}: полный текст встроенного SKILL.md в сообщении",
                pb.name
            );
        }
    }

    #[tokio::test]
    async fn prompts_get_prefers_user_copy_and_echoes_declared_arguments() {
        let tmp = tempfile::tempdir().expect("tmp");
        // Пользовательская копия плейбука в plugins.dirs (логика skill_load).
        let skill_md = tmp
            .path()
            .join("spine-workflows/skills/spine-fitness-gate/SKILL.md");
        std::fs::create_dir_all(skill_md.parent().expect("parent")).expect("dirs");
        std::fs::write(
            &skill_md,
            "---\nname: spine-fitness-gate\ndescription: ПОЛЬЗОВАТЕЛЬСКИЙ плейбук гейта.\n---\n\n\
             # Мой гейт\n\nТело пользователя.\n",
        )
        .expect("SKILL.md");
        let server = server_with_dirs(tmp.path(), tmp.path());
        let responses = run_lines_on(
            server,
            &[
                r#"{"jsonrpc":"2.0","id":1,"method":"prompts/get","params":{"name":"spine-fitness-gate"}}"#,
                r#"{"jsonrpc":"2.0","id":2,"method":"prompts/get","params":{"name":"spine-adr-judge","arguments":{"target":"docs/adr/0001.md","rubric":"adr_quality","чужой":"игнор"}}}"#,
            ],
        )
        .await;
        // Пользовательская копия побеждает встроенную.
        let first = &responses[0]["result"];
        assert_eq!(first["description"], "ПОЛЬЗОВАТЕЛЬСКИЙ плейбук гейта.");
        let text = first["messages"][0]["content"]["text"]
            .as_str()
            .expect("text");
        assert!(text.contains("Тело пользователя."), "{text}");
        // Эхо — только объявленных аргументов, в порядке объявления.
        let text2 = responses[1]["result"]["messages"][0]["content"]["text"]
            .as_str()
            .expect("text");
        assert!(text2.contains("- target = \"docs/adr/0001.md\""), "{text2}");
        assert!(text2.contains("- rubric = \"adr_quality\""), "{text2}");
        assert!(!text2.contains("чужой"), "{text2}");
    }

    #[tokio::test]
    async fn prompts_get_invalid_params_and_resources_stay_guarded() {
        // Имена валидны только в id=3/4, но ошибки параметров ловятся до
        // резолва текста — тест не зависит от реального дома.
        let responses = run_lines(&[
            r#"{"jsonrpc":"2.0","id":1,"method":"prompts/get","params":{}}"#,
            r#"{"jsonrpc":"2.0","id":2,"method":"prompts/get","params":{"name":"ghost"}}"#,
            r#"{"jsonrpc":"2.0","id":3,"method":"prompts/get","params":{"name":"spine-quickstart","arguments":["x"]}}"#,
            r#"{"jsonrpc":"2.0","id":4,"method":"prompts/get","params":{"name":"spine-adr-judge","arguments":{"target":42}}}"#,
            r#"{"jsonrpc":"2.0","id":5,"method":"resources/list","params":{}}"#,
            r#"{"jsonrpc":"2.0","id":6,"method":"resources/templates/list","params":{}}"#,
        ])
        .await;
        for (i, resp) in responses.iter().enumerate() {
            if i < 4 {
                assert_eq!(resp["error"]["code"], INVALID_PARAMS, "ответ {i}: {resp}");
            } else {
                assert_eq!(resp["error"]["code"], METHOD_NOT_FOUND, "ответ {i}: {resp}");
            }
        }
        let msg = responses[1]["error"]["message"].as_str().expect("message");
        assert!(msg.contains("ghost"), "{msg}");
    }

    #[test]
    fn bridge_lists_partition_registry() {
        // Белые списки, ручные имена и never-список попарно не пересекаются.
        for name in BRIDGE_READ_ONLY.iter().chain(BRIDGE_READ_WRITE) {
            assert!(
                !BRIDGE_NEVER.contains(name),
                "{name} и в белом списке, и в never"
            );
            assert!(
                !MANUAL_TOOLS.contains(name),
                "{name} — ручной и мостовой одновременно"
            );
        }
        for name in BRIDGE_NEVER {
            assert!(!MANUAL_TOOLS.contains(name), "{name} — ручной и в never");
        }
        // Все имена списков — реальные члены полного реестра дефолтного
        // конфига (страховка от переименований инструментов доменов).
        let registry = crate::tools::full_registry(&Config::default());
        for name in BRIDGE_READ_ONLY.iter().chain(BRIDGE_READ_WRITE) {
            if HARNESS_ONLY_TOOLS.contains(name) {
                // Домены сборки `harness` (кодовые харнессы, дистилляция): в
                // core-сборке их нет в реестре — мост пропускает их молча.
                if cfg!(feature = "harness") {
                    assert!(
                        registry.get(name).is_some(),
                        "{name} из белого списка отсутствует в full_registry"
                    );
                } else {
                    assert!(
                        registry.get(name).is_none(),
                        "{name} — инструмент сборки harness, в core его быть не должно"
                    );
                }
                continue;
            }
            assert!(
                registry.get(name).is_some(),
                "{name} из белого списка отсутствует в full_registry"
            );
        }
        for name in BRIDGE_NEVER {
            if HARNESS_ONLY_TOOLS.contains(name) {
                if cfg!(feature = "harness") {
                    assert!(
                        registry.get(name).is_some(),
                        "{name} из never-списка отсутствует в full_registry — список протух?"
                    );
                } else {
                    assert!(
                        registry.get(name).is_none(),
                        "{name} — инструмент сборки harness, в core его быть не должно"
                    );
                }
                continue;
            }
            assert!(
                registry.get(name).is_some(),
                "{name} из never-списка отсутствует в full_registry — список протух?"
            );
        }
    }

    #[tokio::test]
    async fn rw_mode_lists_bridge_write_tools_but_never_never() {
        let responses = run_lines_on(
            rw_server(),
            &[r#"{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}"#],
        )
        .await;
        let tools = responses[0]["result"]["tools"].as_array().expect("tools");
        let names: Vec<&str> = tools
            .iter()
            .map(|t| t["name"].as_str().expect("имя"))
            .collect();
        // Инструменты доменов сборки `harness` (skill_distill и др.) в
        // core-сборке в реестре отсутствуют — мост их пропускает.
        let expected_rw: Vec<&str> = BRIDGE_READ_WRITE
            .iter()
            .copied()
            .filter(|n| cfg!(feature = "harness") || !HARNESS_ONLY_TOOLS.contains(n))
            .collect();
        for rw in &expected_rw {
            assert!(
                names.contains(rw),
                "rw-инструмент '{rw}' нужен в --rw: {names:?}"
            );
        }
        // В core-сборке harness-инструментов нет и в выдаче.
        for rw in BRIDGE_READ_WRITE {
            if !cfg!(feature = "harness") && HARNESS_ONLY_TOOLS.contains(rw) {
                assert!(
                    !names.contains(rw),
                    "harness-инструмент '{rw}' не должен собираться в core: {names:?}"
                );
            }
        }
        for forbidden in BRIDGE_NEVER {
            assert!(
                !names.contains(forbidden),
                "never-инструмент '{forbidden}' закрыт и под --rw: {names:?}"
            );
        }
        assert_eq!(
            names.len(),
            MANUAL_TOOLS.len() + BRIDGE_READ_ONLY.len() + expected_rw.len(),
            "rw-режим: ручные + оба белых списка (в core — без harness-доменов)"
        );
        // Аннотации: mutating по классификации политики → destructiveHint.
        // handoff_create — в обеих сборках (core-модуль crate::handoff).
        let handoff = tools
            .iter()
            .find(|t| t["name"] == "handoff_create")
            .expect("handoff_create");
        assert_eq!(handoff["annotations"]["readOnlyHint"], false);
        assert_eq!(handoff["annotations"]["destructiveHint"], true);
        // Аддитивная запись (политика — ReadOnly): readOnlyHint=false по
        // членству в rw-списке, destructiveHint=false по классу риска.
        // (skill_distill живёт в домене сборки `harness`.)
        #[cfg(feature = "harness")]
        {
            let distill = tools
                .iter()
                .find(|t| t["name"] == "skill_distill")
                .expect("skill_distill");
            assert_eq!(distill["annotations"]["readOnlyHint"], false);
            assert_eq!(distill["annotations"]["destructiveHint"], false);
        }
    }

    #[tokio::test]
    async fn rw_tools_callable_only_in_rw_mode() {
        let tmp = tempfile::tempdir().expect("tmp");
        let repo = agentsmd_repo(tmp.path());
        let repo_str = repo.display().to_string();
        let call = |id: u64| {
            format!(
                r#"{{"jsonrpc":"2.0","id":{id},"method":"tools/call","params":{{"name":"agentsmd_generate","arguments":{{"repo":"{repo_str}"}}}}}}"#
            )
        };
        // ro-режим: rw-инструмент недоступен совсем (-32602), запись не идёт.
        let responses = run_lines(&[&call(1)]).await;
        assert_eq!(responses[0]["error"]["code"], INVALID_PARAMS);
        assert!(
            !repo.join("AGENTS.md").exists(),
            "в ro-режиме ничего не создаётся"
        );
        // rw-режим: вызов проходит через реестр, AGENTS.md создан.
        let responses = run_lines_on(rw_server(), &[&call(1)]).await;
        let result = &responses[0]["result"];
        assert_eq!(result["isError"], false, "{result}");
        assert_eq!(result["structuredContent"]["tool"], "agentsmd_generate");
        assert!(
            repo.join("AGENTS.md").is_file(),
            "AGENTS.md создан мостовым вызовом"
        );
    }

    #[tokio::test]
    async fn bridge_openapi_lint_runs_via_registry_with_cwd() {
        let tmp = tempfile::tempdir().expect("tmp");
        std::fs::write(
            tmp.path().join("api.yaml"),
            "openapi: 3.0.3\n\
             info:\n  \
             title: Pet Store API\n  \
             version: 1.0.0\n\
             paths:\n  \
             /v1/pets:\n    \
             get:\n      \
             operationId: listPets\n      \
             responses:\n        \
             '200':\n          \
             description: ok\n",
        )
        .expect("контракт");
        // Относительный путь резолвится от аргумента моста `cwd`.
        let call = format!(
            r#"{{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{{"name":"openapi_lint","arguments":{{"path":"api.yaml","cwd":"{}"}}}}}}"#,
            tmp.path().display()
        );
        let responses = run_lines(&[&call]).await;
        let result = &responses[0]["result"];
        assert_eq!(result["isError"], false, "{result}");
        let sc = &result["structuredContent"];
        assert_eq!(sc["tool"], "openapi_lint");
        let output = sc["output"].as_str().expect("output");
        assert!(output.contains("openapi:"), "отчёт инструмента: {output}");
        // content[0].text моста — сырой вывод инструмента, не JSON-обёртка.
        let text = result["content"][0]["text"].as_str().expect("text");
        assert_eq!(text, output);
    }

    #[tokio::test]
    async fn bridge_policy_require_confirm_becomes_is_error() {
        // Политика R1: agentsmd_generate классифицируется Mutating →
        // RequireConfirm; в неинтерактивном MCP это отказ с пояснением
        // (isError), файл не создаётся.
        let tmp = tempfile::tempdir().expect("tmp");
        let repo = agentsmd_repo(tmp.path());
        let mut cfg = Config::default();
        cfg.policy.autonomy = "R1".into();
        let server = McpServe::with_mode(Arc::new(cfg), ServeMode::ReadWrite);
        let call = format!(
            r#"{{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{{"name":"agentsmd_generate","arguments":{{"repo":"{}"}}}}}}"#,
            repo.display()
        );
        let responses = run_lines_on(server, &[&call]).await;
        let result = &responses[0]["result"];
        assert_eq!(result["isError"], true, "{result}");
        let text = result["content"][0]["text"].as_str().expect("text");
        assert!(
            text.contains("ТРЕБУЕТСЯ ПОДТВЕРЖДЕНИЕ"),
            "причина отказа политики: {text}"
        );
        assert!(
            !repo.join("AGENTS.md").exists(),
            "при RequireConfirm запись не идёт"
        );
    }
}
