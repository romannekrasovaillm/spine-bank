//! Ручные инструменты реестра уроков (`lessons_query`, `lessons_record`,
//! ADR-085) — межсессионная память хост-агента, проверенная гейтом.
//!
//! Оба инструмента — `[Core]`: работают в слим-сборке
//! (`--no-default-features --features core`) и не требуют LLM ни на одной
//! ветке (AD-2). Статус урока выводится только из счётчиков доказательств;
//! `statement` — текст, который хост может сформулировать сам.
//!
//! Запись (`lessons_record`) — явное действие хоста, поэтому она не зависит
//! от `[lessons] enabled` (флаг гасит автоматические записи гейта, L2, и блок
//! инъекции, L3).

use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::{Value, json};

use crate::lessons::{
    self, Evidence, Ledger, Lesson, LessonSource, LessonStatus, Outcome, RecordInput, Scope,
    ScopeLevel, Signature, Thresholds,
};

use crate::mcp_server::types::{CallError, McpServe, blocking, parse_args};

impl McpServe {
    /// `lessons_query`: выборка уроков реестра (по id, статусу, уровню
    /// области) с рендером в Markdown. Read-only: файлов не создаёт.
    pub(in crate::mcp_server) async fn tool_lessons_query(
        &self,
        args: Value,
    ) -> std::result::Result<Value, CallError> {
        #[derive(Deserialize)]
        struct Args {
            /// Каталог с реестром (по умолчанию — каталог вызова). Каноничное
            /// имя аргумента пути — `path`; `repo` принимается синонимом (Н8).
            #[serde(alias = "repo")]
            path: Option<String>,
            /// Пользовательский реестр `$ARCH_HOME/lessons` вместо репозиторного.
            user: Option<bool>,
            /// Организационный реестр `[lessons] org_dir` (ADR-091).
            org: Option<bool>,
            /// Идентификатор урока: вернуть карточку одного урока.
            id: Option<String>,
            /// Фильтр по статусу (observed|candidate|confirmed|promoted|retired).
            status: Option<String>,
            /// Фильтр по уровню области (file|module|package|repo|user|org).
            scope: Option<String>,
            /// Рабочий каталог клиента: относительный `path` резолвится от него.
            cwd: Option<String>,
        }
        let args: Args = parse_args(args, "lessons_query")?;
        let ledger = resolve_ledger(
            args.path.as_deref(),
            args.user.unwrap_or(false),
            args.org.unwrap_or(false),
            self.cfg.lessons.org_dir.as_deref(),
            args.cwd.as_deref(),
        )?;
        let status = args
            .status
            .as_deref()
            .map(str::parse::<LessonStatus>)
            .transpose()
            .map_err(|e| CallError::invalid_params(e.to_string()))?;
        let scope = args
            .scope
            .as_deref()
            .map(str::parse::<ScopeLevel>)
            .transpose()
            .map_err(|e| CallError::invalid_params(e.to_string()))?;
        let id_filter = args.id;
        let th = thresholds(self);
        let registry = ledger.path().display().to_string();
        let lessons_md = ledger.lessons_md_path().display().to_string();
        let (snapshot, selected) = blocking("lessons_query", move || {
            let snapshot = ledger.snapshot(&th)?;
            let selected: Vec<Lesson> = snapshot
                .lessons
                .iter()
                .filter(|l| id_filter.as_deref().is_none_or(|id| l.id == id))
                .filter(|l| status.is_none_or(|s| l.status == s))
                .filter(|l| scope.is_none_or(|level| l.scope.level == level))
                .cloned()
                .collect();
            Ok((snapshot, selected))
        })
        .await?;
        let report = lessons::render::render(&selected);
        Ok(json!({
            "registry": registry,
            "lessons_md": lessons_md,
            "events": snapshot.events,
            "malformed_lines": snapshot.malformed,
            "count": selected.len(),
            "lessons": selected,
            "report_markdown": report,
            "summary": format!(
                "уроков {} из {} (событий {})",
                selected.len(),
                snapshot.lessons.len(),
                snapshot.events
            ),
        }))
    }

    /// `lessons_record`: записать исход по подписи урока (создать урок либо
    /// добавить доказательство). После записи реестр перерисовывается в
    /// `LESSONS.md` (атомарно, tmp + rename).
    pub(in crate::mcp_server) async fn tool_lessons_record(
        &self,
        args: Value,
    ) -> std::result::Result<Value, CallError> {
        #[derive(Deserialize)]
        struct Args {
            /// Источник урока: gate|fitness|tool|review. Обязателен (схема);
            /// проверяется и здесь — понятной ошибкой вместо serde-сообщения.
            source: Option<String>,
            /// Код: `G5`, id правила реестра, класс ошибки инструмента.
            code: Option<String>,
            /// Вид субъекта (L6): `new_public_symbol`, `new_migration`, …
            subject_kind: Option<String>,
            /// Текст правила (может быть `[inferred]` — на статус не влияет).
            statement: Option<String>,
            /// Уровень области действия (по умолчанию repo).
            scope_level: Option<String>,
            /// Глобы путей области действия.
            #[serde(default)]
            globs: Vec<String>,
            /// Идентификатор прогона, из которого пришло доказательство.
            run_id: Option<String>,
            /// Задача (ключ «≥ 2 разных задач»).
            task: Option<String>,
            /// Исполнитель прогона.
            harness: Option<String>,
            /// Выдержка вердикта (усекается до 512 символов).
            excerpt: Option<String>,
            /// Файл нарушения.
            file: Option<String>,
            /// Строка нарушения.
            line: Option<u32>,
            /// Исход: `fail` (дефолт) | `pass_after_apply`.
            outcome: Option<String>,
            /// Кто записал (mcp, gate, hook, agent) — по умолчанию `mcp`.
            recorder: Option<String>,
            /// Каталог с реестром (по умолчанию — каталог вызова). Каноничное
            /// имя аргумента пути — `path`; `repo` принимается синонимом (Н8).
            #[serde(alias = "repo")]
            path: Option<String>,
            /// Пользовательский реестр `$ARCH_HOME/lessons`.
            user: Option<bool>,
            /// Организационный реестр `[lessons] org_dir` (ADR-091).
            org: Option<bool>,
            /// Рабочий каталог клиента.
            cwd: Option<String>,
        }
        let args: Args = parse_args(args, "lessons_record")?;
        let ledger = resolve_ledger(
            args.path.as_deref(),
            args.user.unwrap_or(false),
            args.org.unwrap_or(false),
            self.cfg.lessons.org_dir.as_deref(),
            args.cwd.as_deref(),
        )?;
        // Обязательные по схеме аргументы отсутствуют — отказ с перечнем
        // (серде сообщение «missing field» не объясняет, что подставить).
        let source = args.source.ok_or_else(|| {
            CallError::invalid_params(
                "lessons_record: укажите source (gate|fitness|tool|review)".to_string(),
            )
        })?;
        let code = args.code.ok_or_else(|| {
            CallError::invalid_params(
                "lessons_record: укажите code (G5, id правила, класс)".to_string(),
            )
        })?;
        let run_id = args.run_id.ok_or_else(|| {
            CallError::invalid_params(
                "lessons_record: укажите run_id — доказательство без прогона не принимается"
                    .to_string(),
            )
        })?;
        let mut signature = Signature::new(
            source
                .parse::<LessonSource>()
                .map_err(|e| CallError::invalid_params(e.to_string()))?,
            code,
        );
        if let Some(kind) = args.subject_kind {
            signature = signature.with_subject(kind);
        }
        let level: ScopeLevel = match args.scope_level {
            Some(raw) => raw
                .parse::<ScopeLevel>()
                .map_err(|e| CallError::invalid_params(e.to_string()))?,
            None => ScopeLevel::default(),
        };
        let outcome: Outcome = match args.outcome.as_deref() {
            Some(raw) => raw
                .parse::<Outcome>()
                .map_err(|e| CallError::invalid_params(e.to_string()))?,
            None => Outcome::Fail,
        };
        let input = RecordInput::new(
            signature,
            Evidence {
                run_id,
                harness: args.harness,
                verdict_excerpt: args.excerpt.unwrap_or_default(),
                file: args.file,
                line: args.line,
                ts: lessons::now_iso(),
                outcome,
                task: args.task,
            },
        )
        .with_statement(args.statement.unwrap_or_default())
        .with_scope(Scope::new(level, args.globs))
        .with_recorder(args.recorder.unwrap_or_else(|| "mcp".to_string()));
        let th = thresholds(self);
        let today = lessons::today();
        let registry = ledger.path().display().to_string();
        let lessons_md = ledger.lessons_md_path();
        let (mutation, lesson_count) = blocking("lessons_record", move || {
            let mutation = ledger.record(&input, &th, &today)?;
            let snapshot = ledger.snapshot(&th)?;
            lessons::render::render_to(&lessons_md, &snapshot.lessons)?;
            Ok((mutation, snapshot.lessons.len()))
        })
        .await?;
        Ok(json!({
            "registry": registry,
            "lesson": mutation.lesson,
            "created": mutation.created,
            "previous_status": mutation.previous_status,
            "status_changed": mutation.status_changed(),
            "events": mutation.events.len(),
            "lesson_count": lesson_count,
        }))
    }

    /// `lessons_match`: префлайт-подбор уроков реестра под задачу (L4) —
    /// отбор по области (пути задачи и пути из текста) + BM25 против
    /// `statement` и `signature` + приоритет статусов, без LLM. Read-only:
    /// файлов не создаёт. Возвращает top-K уроков и рендер в Markdown.
    pub(in crate::mcp_server) async fn tool_lessons_match(
        &self,
        args: Value,
    ) -> std::result::Result<Value, CallError> {
        #[derive(Deserialize)]
        struct Args {
            /// Текст задачи: подбор против `statement` и `signature`, пути — из
            /// текста. Пусто/отсутствует — пустой отбор (вся область репозитория).
            task: Option<String>,
            /// Явные пути задачи (объединяются с путями из текста). Отсутствует —
            /// только пути, извлечённые из текста.
            paths: Option<Vec<String>>,
            /// Каталог с реестром (по умолчанию — каталог вызова). Каноничное
            /// имя аргумента пути — `path`; `repo` принимается синонимом (Н8).
            #[serde(alias = "repo")]
            path: Option<String>,
            /// Максимум уроков в выдаче (по умолчанию — `[lessons] k`).
            k: Option<usize>,
            /// Рабочий каталог клиента: относительный `path` резолвится от него.
            cwd: Option<String>,
        }
        let args: Args = parse_args(args, "lessons_match")?;
        let root = resolve_repo(args.path.as_deref(), args.cwd.as_deref());
        let k = args.k.unwrap_or(self.cfg.lessons.k);
        let task = args.task.unwrap_or_default();
        let paths = args.paths;
        let cfg = std::sync::Arc::clone(&self.cfg);
        let registry = crate::lessons::repo_ledger_path(&root)
            .display()
            .to_string();
        let matched = blocking("lessons_match", move || {
            let lessons = crate::lessons::matching::load_lessons(&cfg, &root);
            let matched =
                crate::lessons::matching::lessons_match(lessons, &task, paths.as_deref(), k);
            Ok(matched)
        })
        .await?;
        let report = lessons::render::render(&matched);
        let count = matched.len();
        Ok(json!({
            "registry": registry,
            "count": count,
            "lessons": matched,
            "report_markdown": report,
            "summary": format!("подобрано уроков: {count}"),
        }))
    }
}

/// Корень репозитория по аргументам инструмента (относительный путь — от
/// `cwd` клиента).
fn resolve_repo(repo: Option<&str>, cwd: Option<&str>) -> PathBuf {
    let raw = repo.map_or_else(|| PathBuf::from("."), PathBuf::from);
    match cwd {
        Some(cwd) if !raw.is_absolute() => PathBuf::from(cwd).join(raw),
        _ => raw,
    }
}

/// Реестр по аргументам инструмента: `org` → `[lessons] org_dir`, `user` →
/// `$ARCH_HOME/lessons`, иначе `<repo>/.arch/lessons` (относительный путь — от
/// `cwd` клиента). `org` без настроенного `org_dir` — понятная ошибка, а не
/// молчание (ADR-091).
fn resolve_ledger(
    repo: Option<&str>,
    user: bool,
    org: bool,
    org_dir: Option<&Path>,
    cwd: Option<&str>,
) -> std::result::Result<Ledger, CallError> {
    if user && org {
        return Err(CallError::invalid_params(
            "реестр уроков: укажите одно — user или org".to_string(),
        ));
    }
    if org {
        let dir = org_dir.ok_or_else(|| {
            CallError::invalid_params(
                "[lessons] org_dir не настроен — org-реестр недоступен".to_string(),
            )
        })?;
        return Ok(Ledger::org(dir));
    }
    if user {
        return Ok(Ledger::user());
    }
    Ok(Ledger::repo(&resolve_repo(repo, cwd)))
}

/// Пороги переходов из секции `[lessons]` конфига сервера.
fn thresholds(server: &McpServe) -> Thresholds {
    Thresholds::from(&server.cfg.lessons)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp_server::testkit::{run_lines, run_lines_on, rw_server};

    /// Запись урока — инструмент контура записи: вызовы идут на сервер в
    /// режиме `--rw` (read-only состав сервера писать не умеет).
    async fn call_rw(input: &[&str]) -> Vec<Value> {
        run_lines_on(rw_server(), input).await
    }

    /// Запрос `lessons_record` в репозиторий `repo`.
    fn record_call(id: u32, repo: &str, extra: &Value) -> String {
        let mut arguments = json!({
            "path": repo,
            "source": "gate",
            "code": "G5",
            "run_id": format!("run-{id}"),
        });
        if let (Some(target), Some(source)) = (arguments.as_object_mut(), extra.as_object()) {
            for (key, value) in source {
                target.insert(key.clone(), value.clone());
            }
        }
        json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "tools/call",
            "params": {"name": "lessons_record", "arguments": arguments},
        })
        .to_string()
    }

    #[tokio::test]
    async fn record_then_query_roundtrip() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let repo = tmp.path().display().to_string();
        let first = record_call(
            1,
            &repo,
            &json!({
                "statement": "новый публичный символ требует __all__",
                "task": "T1",
                "harness": "claude-code",
                "excerpt": "[G5] отсутствует __all__",
                "file": "paylib/__init__.py",
                "line": 1
            }),
        );
        let responses = call_rw(&[&first]).await;
        let recorded = &responses[0]["result"]["structuredContent"];
        assert_eq!(
            recorded["created"], true,
            "первый FAIL создаёт урок: {recorded}"
        );
        assert_eq!(recorded["lesson"]["status"], "observed");
        assert_eq!(recorded["lesson"]["fails"], 1);
        let id = recorded["lesson"]["id"].as_str().expect("id").to_string();
        assert!(
            id.starts_with(lessons::ID_PREFIX),
            "id с префиксом реестра: {id}"
        );

        // Журнал и рендер созданы в переданном репозитории.
        assert!(tmp.path().join(".arch/lessons/ledger.jsonl").is_file());
        let markdown =
            std::fs::read_to_string(tmp.path().join(".arch/lessons/LESSONS.md")).expect("рендер");
        assert!(markdown.contains(&id), "{markdown}");

        // Запрос по id.
        // Каноничное имя пути — `path`; исторический синоним `repo`
        // принимается как алиас (Н8) — проверяем оба.
        let query = json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {
                "name": "lessons_query",
                "arguments": {"path": repo, "id": id}
            }
        })
        .to_string();
        let responses = run_lines(&[&query]).await;
        let found = &responses[0]["result"]["structuredContent"];
        assert_eq!(found["count"], 1, "{found}");
        assert_eq!(
            found["lessons"][0]["statement"],
            "новый публичный символ требует __all__"
        );
        assert!(
            found["report_markdown"]
                .as_str()
                .unwrap_or("")
                .contains(&id)
        );

        // Фильтр по чужому статусу — ноль уроков, не ошибка.
        let empty = json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": {
                "name": "lessons_query",
                "arguments": {"path": repo, "status": "promoted"}
            }
        })
        .to_string();
        let responses = run_lines(&[&empty]).await;
        assert_eq!(responses[0]["result"]["structuredContent"]["count"], 0);
    }

    #[tokio::test]
    async fn status_comes_from_counters_not_from_the_host() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let repo = tmp.path().display().to_string();
        // Первый FAIL, повторный FAIL (candidate), два PASS в разных задачах
        // (confirmed) — статус двигают только счётчики доказательств.
        let calls = [
            record_call(1, &repo, &json!({"task": "T1"})),
            record_call(2, &repo, &json!({"task": "T2"})),
            record_call(
                3,
                &repo,
                &json!({"task": "T3", "outcome": "pass_after_apply"}),
            ),
            record_call(
                4,
                &repo,
                &json!({"task": "T4", "outcome": "pass_after_apply"}),
            ),
        ];
        let refs: Vec<&str> = calls.iter().map(String::as_str).collect();
        let responses = call_rw(&refs).await;
        let statuses: Vec<&str> = responses
            .iter()
            .map(|r| {
                r["result"]["structuredContent"]["lesson"]["status"]
                    .as_str()
                    .unwrap_or("")
            })
            .collect();
        assert_eq!(
            statuses,
            vec!["observed", "candidate", "candidate", "confirmed"],
            "{responses:?}"
        );
        assert_eq!(
            responses[3]["result"]["structuredContent"]["lesson"]["hits"],
            2
        );
    }

    #[tokio::test]
    async fn invalid_source_is_an_invalid_params_error() {
        let bad = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {
                "name": "lessons_record",
                "arguments": {"source": "телепатия", "code": "G5", "run_id": "r1"}
            }
        })
        .to_string();
        let responses = call_rw(&[&bad]).await;
        assert_eq!(responses[0]["error"]["code"], -32602, "{responses:?}");
        assert!(
            responses[0]["error"]["message"]
                .as_str()
                .unwrap_or("")
                .contains("неизвестный источник"),
            "{responses:?}"
        );
    }

    #[tokio::test]
    async fn query_on_empty_registry_is_fail_soft() {
        let tmp = tempfile::tempdir().expect("tempdir");
        // Историческое имя пути `repo` доходит до инструмента (Н8): сервер
        // принимает его как синоним каноничного `path`.
        let query = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {
                "name": "lessons_query",
                "arguments": {"repo": tmp.path().display().to_string()}
            }
        })
        .to_string();
        let responses = run_lines(&[&query]).await;
        let out = &responses[0]["result"]["structuredContent"];
        assert_eq!(out["count"], 0);
        assert_eq!(out["events"], 0);
        assert!(
            out["report_markdown"]
                .as_str()
                .unwrap_or("")
                .contains("Уроков: 0"),
            "{out}"
        );
        // Чтение не создаёт файлов: реестра нет — нет и побочных эффектов.
        assert!(!tmp.path().join(".arch/lessons/ledger.jsonl").exists());
    }

    /// Режим `--rw`: оба инструмента в `tools/list`; read-only состав отдаёт
    /// только чтение, а вызов записи отклоняется с понятной причиной
    /// (read-only сервер не пишет в каталог клиента — тот же принцип, что у
    /// белых списков моста).
    #[tokio::test]
    async fn write_tool_is_gated_by_rw_mode() {
        let listing = r#"{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}"#;
        let names = |responses: &[Value]| -> Vec<String> {
            responses[0]["result"]["tools"]
                .as_array()
                .expect("tools")
                .iter()
                .filter_map(|tool| tool["name"].as_str().map(str::to_string))
                .collect()
        };
        let read_only = run_lines(&[listing]).await;
        let names_read_only = names(&read_only);
        assert!(names_read_only.contains(&"lessons_query".to_string()));
        assert!(
            !names_read_only.contains(&"lessons_record".to_string()),
            "read-only состав не отдаёт пишущий инструмент: {names_read_only:?}"
        );

        let rw = call_rw(&[listing]).await;
        let names_rw = names(&rw);
        assert!(names_rw.contains(&"lessons_query".to_string()));
        assert!(
            names_rw.contains(&"lessons_record".to_string()),
            "под --rw инструмент записи доступен: {names_rw:?}"
        );

        // Вызов записи в read-only режиме — отказ, а не молчаливая запись.
        let tmp = tempfile::tempdir().expect("tempdir");
        let call = record_call(1, &tmp.path().display().to_string(), &json!({}));
        let responses = run_lines(&[&call]).await;
        assert_eq!(responses[0]["error"]["code"], -32602, "{responses:?}");
        assert!(
            responses[0]["error"]["message"]
                .as_str()
                .unwrap_or("")
                .contains("--rw"),
            "{responses:?}"
        );
        assert!(
            !tmp.path().join(".arch/lessons/ledger.jsonl").exists(),
            "read-only вызов ничего не записал"
        );
    }
}
