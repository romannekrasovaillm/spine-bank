//! Ручные инструменты чтения знаний (T4, ADR-015; разбиение B1): `kb_search`
//! (база знаний из knowledge.dirs), `skill_search`/`skill_load` (библиотека
//! скиллов plugins.dirs), `mermaid_render` (диаграмма → ASCII-арт, ядро —
//! [`mermaid_render_value`]). MCP-слой только парсит аргументы и собирает JSON.

use std::path::PathBuf;

use serde::Deserialize;
use serde_json::{Value, json};

use crate::error::Result;
use crate::{kb, mermaid, plugin};

use crate::mcp_server::types::{
    CallError, KB_SEARCH_DEFAULT_LIMIT, KNOWLEDGE_MAX_HITS, McpServe, SKILL_SEARCH_DEFAULT_LIMIT,
    SKILL_TEXT_MAX_CHARS, blocking, parse_args,
};

impl McpServe {
    /// `kb_search`: поиск по локальной базе знаний харнесса
    /// (`knowledge.dirs` из конфига arch, а не каталоги репозитория клиента).
    /// Ядро — [`kb::search`] (то же, что у агентного инструмента);
    /// MCP-слой только формирует JSON из хитов.
    pub(in crate::mcp_server) async fn tool_kb_search(
        &self,
        args: Value,
    ) -> std::result::Result<Value, CallError> {
        #[derive(Deserialize)]
        struct Args {
            /// Поисковый запрос: термины через пробел.
            query: String,
            /// Максимум хитов (по умолчанию 10, не больше 20).
            limit: Option<usize>,
        }
        let args: Args = parse_args(args, "kb_search")?;
        let limit = args
            .limit
            .unwrap_or(KB_SEARCH_DEFAULT_LIMIT)
            .min(KNOWLEDGE_MAX_HITS);
        let hits = kb::search(
            &self.cfg.knowledge.dirs,
            &self.cfg.knowledge.extensions,
            &args.query,
            limit,
        )
        .await
        .map_err(|e| CallError::execution("kb_search", e))?;
        let summary = if hits.is_empty() {
            format!(
                "По запросу «{}» в базе знаний ничего не найдено.",
                args.query
            )
        } else {
            format!("По запросу «{}» найдено хитов: {}", args.query, hits.len())
        };
        Ok(json!({
            "query": args.query,
            "count": hits.len(),
            "hits": hits,
            "summary": summary,
        }))
    }

    /// `skill_search`: поиск по библиотеке скиллов (`plugins.dirs` из конфига
    /// arch). Ядро — [`plugin::discover`] + [`plugin::search`] (агентный
    /// инструмент `skill_search` использует те же функции).
    pub(in crate::mcp_server) async fn tool_skill_search(
        &self,
        args: Value,
    ) -> std::result::Result<Value, CallError> {
        #[derive(Deserialize)]
        struct Args {
            /// Поисковый запрос.
            query: String,
            /// Максимум результатов (по умолчанию 8, не больше 20).
            limit: Option<usize>,
        }
        let args: Args = parse_args(args, "skill_search")?;
        let limit = args
            .limit
            .unwrap_or(SKILL_SEARCH_DEFAULT_LIMIT)
            .min(KNOWLEDGE_MAX_HITS);
        // T-09: скиллы, разложенные в проекте (`connect`), — часть индекса,
        // иначе поиск пуст до `arch-be init`, хотя скиллы на диске есть.
        let root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let dirs = plugin::skill_search_dirs(&self.cfg.plugins.dirs, &root);
        let query = args.query;
        blocking("skill_search", move || -> Result<Value> {
            let plugins = plugin::discover(&dirs);
            let total: usize = plugins.iter().map(|p| p.skills.len()).sum();
            let hits = plugin::search(&plugins, &query, limit);
            let summary = if hits.is_empty() {
                plugin::empty_index_answer(&query, total, &dirs)
            } else {
                format!("по запросу '{query}' найдено скиллов: {}", hits.len())
            };
            Ok(json!({
                "query": query,
                "count": hits.len(),
                "hits": hits.iter().map(|h| json!({
                    "name": h.meta.name,
                    "plugin": h.meta.plugin,
                    "score": h.score,
                    "description": h.meta.description,
                    "snippet": h.snippet,
                })).collect::<Vec<_>>(),
                "summary": summary,
            }))
        })
        .await
    }

    /// `skill_load`: полный текст скилла по точному имени. Ядро —
    /// [`plugin::skill_by_name`] + [`plugin::load_skill`]; лимит тела —
    /// как у агентного инструмента (`SKILL_TEXT_MAX_CHARS`).
    pub(in crate::mcp_server) async fn tool_skill_load(
        &self,
        args: Value,
    ) -> std::result::Result<Value, CallError> {
        #[derive(Deserialize)]
        struct Args {
            /// Точное имя скилла.
            name: String,
        }
        let args: Args = parse_args(args, "skill_load")?;
        let root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let dirs = plugin::skill_search_dirs(&self.cfg.plugins.dirs, &root);
        let name = args.name;
        blocking("skill_load", move || -> Result<Value> {
            let plugins = plugin::discover(&dirs);
            let Some(meta) = plugin::skill_by_name(&plugins, &name) else {
                return Err(crate::error::HarnessError::Tool(format!(
                    "скилл '{name}' не найден; сначала skill_search"
                )));
            };
            let mut text = plugin::load_skill(meta)?;
            if text.chars().count() > SKILL_TEXT_MAX_CHARS {
                let cut: String = text.chars().take(SKILL_TEXT_MAX_CHARS).collect();
                text = format!("{cut}\n… [усечено: {SKILL_TEXT_MAX_CHARS} символов]");
            }
            Ok(json!({
                "name": meta.name,
                "plugin": meta.plugin,
                "description": meta.description,
                "path": meta.path,
                "body": text,
                "summary": format!("Скилл '{}' загружен (плагин '{}').", meta.name, meta.plugin),
            }))
        })
        .await
    }

    /// `mermaid_render`: диаграмма mermaid → ASCII-арт. Вход — `code`
    /// (исходник) или `path` (файл относительно cwd сервера, как и пути
    /// контрольных инструментов). Ядро — [`mermaid::render`] /
    /// [`mermaid::read_diagram_source`] (агентный `mermaid_render`).
    pub(in crate::mcp_server) async fn tool_mermaid_render(
        &self,
        args: Value,
    ) -> std::result::Result<Value, CallError> {
        #[derive(Deserialize)]
        struct Args {
            /// Исходный код mermaid-диаграммы.
            code: Option<String>,
            /// Путь к .mmd-файлу (резолвится от cwd сервера).
            path: Option<String>,
        }
        let args: Args = parse_args(args, "mermaid_render")?;
        match args.code {
            Some(code) if !code.trim().is_empty() => {
                blocking("mermaid_render", move || mermaid_render_value(&code)).await
            }
            _ => match args.path {
                Some(path) => {
                    let path = PathBuf::from(path);
                    blocking("mermaid_render", move || {
                        let code = mermaid::read_diagram_source(&path)?;
                        mermaid_render_value(&code)
                    })
                    .await
                }
                None => Err(CallError::Execution(
                    "mermaid_render: нужен аргумент 'code' (исходник) или 'path' (файл)".into(),
                )),
            },
        }
    }
}

/// Рендерит код диаграммы в JSON-ответ `mermaid_render` (общий для inline-кода
/// и файла): арт + вид диаграммы. Ошибки парсера — [`HarnessError::Mermaid`]
/// с номером строки, как у агентного инструмента.
fn mermaid_render_value(code: &str) -> Result<Value> {
    let art = mermaid::render(code)?;
    let kind = match mermaid::diagram_kind(code) {
        mermaid::DiagramKind::Flowchart => "flowchart",
        mermaid::DiagramKind::Sequence => "sequenceDiagram",
        mermaid::DiagramKind::Er => "erDiagram",
        mermaid::DiagramKind::C4 => "c4",
        mermaid::DiagramKind::C4Unsupported | mermaid::DiagramKind::Unknown => {
            // `render` выше уже отклонил эти виды — ветка недостижима, но
            // DiagramKind не знает об успехе; держимся консервативно.
            "unknown"
        }
    };
    Ok(json!({
        "kind": kind,
        "art": art,
        "summary": format!("Диаграмма ({kind}) отрендерена в ASCII-арт."),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::Arc;

    use crate::config::Config;
    use crate::mcp_server::testkit::*;

    #[tokio::test]
    async fn kb_search_returns_hits_from_test_knowledge_catalog() {
        let tmp = tempfile::tempdir().expect("tmp");
        kb_fixture(tmp.path());
        let kb_dir = tmp.path().join("kb");
        std::fs::create_dir(&kb_dir).expect("kb dir");
        std::fs::write(
            kb_dir.join("notes-kafka.md"),
            "# Заметки\n\nKafka как шина событий в проде.\n",
        )
        .expect("notes");
        // Каталог знаний — только kb_dir (в tmp лежат и файлы фикстуры).
        let mut cfg = Config::default();
        cfg.knowledge.dirs = vec![kb_dir.clone()];
        cfg.knowledge.extensions = vec!["md".into()];
        let server = McpServe::new(Arc::new(cfg));
        let call = |id: u64, query: &str| {
            format!(
                r#"{{"jsonrpc":"2.0","id":{id},"method":"tools/call","params":{{"name":"kb_search","arguments":{{"query":"{query}"}}}}}}"#
            )
        };
        let responses = run_lines_on(server, &[&call(1, "kafka")]).await;
        let result = &responses[0]["result"];
        assert_eq!(result["isError"], false, "{result}");
        let sc = &result["structuredContent"];
        assert!(sc["count"].as_u64().expect("count") >= 1, "{sc}");
        let hits = sc["hits"].as_array().expect("hits");
        let top = &hits[0];
        assert!(
            top["path"]
                .as_str()
                .expect("path")
                .ends_with("notes-kafka.md"),
            "верхний хит — файл с kafka в имени: {top}"
        );
        assert!(top["score"].as_f64().expect("score") > 0.0);
        assert!(
            top["snippet"].as_str().expect("snippet").contains("Kafka"),
            "сниппет с матчем: {top}"
        );
        // Текст-дубль — валидный JSON (его разбирает клиент mcp.rs).
        let text = result["content"][0]["text"].as_str().expect("text");
        let parsed: Value = serde_json::from_str(text).expect("text — JSON");
        assert_eq!(parsed["count"], sc["count"]);
    }

    #[tokio::test]
    async fn kb_search_empty_result_is_not_an_error() {
        let tmp = tempfile::tempdir().expect("tmp");
        kb_fixture(tmp.path());
        let server = server_with_dirs(tmp.path(), tmp.path());
        let call = r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"kb_search","arguments":{"query":"никогданесуществующийтермин"}}}"#;
        let responses = run_lines_on(server, &[call]).await;
        let result = &responses[0]["result"];
        assert_eq!(result["isError"], false, "{result}");
        let sc = &result["structuredContent"];
        assert_eq!(sc["count"], 0);
        assert!(
            sc["summary"]
                .as_str()
                .expect("summary")
                .contains("ничего не найдено")
        );
    }

    #[tokio::test]
    async fn skill_search_finds_arch_core_and_load_returns_body() {
        let tmp = tempfile::tempdir().expect("tmp");
        plugin_fixture(tmp.path());
        let server = server_with_dirs(tmp.path(), tmp.path());
        let responses = run_lines_on(
            server,
            &[
                r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"skill_search","arguments":{"query":"arch"}}}"#,
                r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"skill_load","arguments":{"name":"arch-core"}}}"#,
                r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"skill_load","arguments":{"name":"нет-такого"}}}"#,
            ],
        )
        .await;
        // skill_search находит скилл arch-core.
        let search = &responses[0]["result"]["structuredContent"];
        let hits = search["hits"].as_array().expect("hits");
        assert!(
            hits.iter().any(|h| {
                h["name"] == "arch-core"
                    && h["plugin"] == "mine"
                    && h["score"].as_f64().expect("score") > 0.0
            }),
            "arch-core в хитах: {search}"
        );
        // skill_load отдаёт полный текст скилла.
        let loaded = &responses[1]["result"];
        assert_eq!(loaded["isError"], false, "{loaded}");
        let sc = &loaded["structuredContent"];
        assert_eq!(sc["name"], "arch-core");
        assert_eq!(sc["plugin"], "mine");
        assert!(
            sc["body"]
                .as_str()
                .expect("body")
                .contains("Методика принятия решений"),
            "тело скилла: {sc}"
        );
        // Неизвестное имя — доменная ошибка isError, а не protocol error.
        let missing = &responses[2]["result"];
        assert_eq!(missing["isError"], true, "{missing}");
        let text = missing["content"][0]["text"].as_str().expect("text");
        assert!(
            text.contains("не найден") && text.contains("нет-такого"),
            "{text}"
        );
    }

    #[tokio::test]
    async fn mermaid_render_returns_ascii_art_for_flowchart() {
        let server = server();
        let responses = run_lines_on(
            server,
            &[
                r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"mermaid_render","arguments":{"code":"graph LR\nA --> B"}}}"#,
                r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"mermaid_render","arguments":{}}}"#,
            ],
        )
        .await;
        let ok = &responses[0]["result"];
        assert_eq!(ok["isError"], false, "{ok}");
        let sc = &ok["structuredContent"];
        assert_eq!(sc["kind"], "flowchart");
        let art = sc["art"].as_str().expect("art");
        assert!(
            art.contains("│ A │") && art.contains("│ B │") && art.contains('▶'),
            "LR-цепочка из двух узлов:\n{art}"
        );
        assert!(
            sc["summary"]
                .as_str()
                .expect("summary")
                .contains("flowchart"),
            "{}",
            sc["summary"]
        );
        // Без code/path — вежливая доменная ошибка, не protocol error.
        let no_args = &responses[1]["result"];
        assert_eq!(no_args["isError"], true, "{no_args}");
        assert!(
            no_args["content"][0]["text"]
                .as_str()
                .expect("text")
                .contains("нужен аргумент"),
            "{no_args}"
        );
    }

    #[tokio::test]
    async fn mermaid_render_reads_mmd_file_by_path() {
        let tmp = tempfile::tempdir().expect("tmp");
        std::fs::write(tmp.path().join("flow.mmd"), "graph TD\nA --> B\n").expect("mmd");
        let server = server();
        let call = format!(
            r#"{{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{{"name":"mermaid_render","arguments":{{"path":"{}"}}}}}}"#,
            tmp.path().join("flow.mmd").display()
        );
        let responses = run_lines_on(server, &[&call]).await;
        let result = &responses[0]["result"];
        assert_eq!(result["isError"], false, "{result}");
        let sc = &result["structuredContent"];
        assert_eq!(sc["kind"], "flowchart");
        let art = sc["art"].as_str().expect("art");
        assert!(
            art.contains("│ A │") && art.contains("│ B │") && art.contains('▼'),
            "TD-цепочка из файла:\n{art}"
        );
    }
}
