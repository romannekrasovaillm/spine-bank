//! Процессные тесты MCP-серверного режима `arch-be mcp serve` (P1-2, ADR-008):
//! дочернему процессу скармливаются JSON-RPC запросы через stdin, NDJSON-
//! ответы читаются из stdout. Всё детерминированно и офлайн: дом изолирован
//! в tempdir (см. [`common::arch_cmd`]), живой LLM не вызывается (тест
//! `rubric_run` с ключом — `#[ignore]`).

mod common;

use std::path::Path;

use serde_json::{Value, json};

use common::arch_cmd;

/// Прогоняет пачку NDJSON-запросов через дочерний `arch-be mcp serve`
/// (stdin закрывается после записи — сервер завершается по EOF) и
/// возвращает разобранные ответы в порядке выдачи.
fn mcp_serve(home: &Path, requests: &str) -> Vec<Value> {
    mcp_serve_with_args(home, &[], requests)
}

/// [`mcp_serve`] с дополнительными аргументами командной строки
/// (например, `--rw` для rw-режима моста).
fn mcp_serve_with_args(home: &Path, extra_args: &[&str], requests: &str) -> Vec<Value> {
    let mut cmd = arch_cmd(home);
    cmd.arg("mcp").arg("serve").args(extra_args);
    let output = cmd
        .write_stdin(requests)
        .output()
        .expect("запуск arch-be mcp serve");
    assert!(
        output.status.success(),
        "сервер завершился сбоем: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("stdout — utf8");
    stdout
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("каждая строка stdout — валидный JSON-RPC"))
        .collect()
}

/// Одна строка запроса `tools/call`.
fn call(id: u64, name: &str, args: &Value) -> String {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "tools/call",
        "params": {"name": name, "arguments": args},
    })
    .to_string()
}

/// Склеивает запросы в NDJSON-пачку.
fn batch(requests: &[String]) -> String {
    let mut out = requests.join("\n");
    out.push('\n');
    out
}

/// Структурированный verdict успешного вызова инструмента.
fn structured(response: &Value, idx: u64) -> &Value {
    assert_eq!(response["id"], idx, "id ответа");
    assert_eq!(
        response["result"]["isError"], false,
        "вызов не должен быть isError: {response}"
    );
    &response["result"]["structuredContent"]
}

/// Фикстура spine-файла: чистый либо с дублем AD-1 и пустым полем.
/// Каждый вариант — свой подкаталог `name` (пути не должны пересекаться).
fn spine_fixture(home: &Path, name: &str, broken: bool) -> String {
    let dir = home.join(name);
    std::fs::create_dir_all(&dir).expect("mkdir spine");
    let path = dir.join("ARCHITECTURE-SPINE.md");
    let body = if broken {
        "### AD-1. Брокер\n- Binds: контур\n- Prevents: хаос\n- Rule: только брокер\n\n\
         ### AD-1. Дубль\n- Binds:\n- Prevents: y\n- Rule: z\n"
    } else {
        "### AD-1. Брокер\n- Binds: контур\n- Prevents: хаос\n- Rule: только брокер\n"
    };
    std::fs::write(&path, body).expect("запись spine");
    path.display().to_string()
}

/// Фикстура репозитория `name` с нарушением карточного правила:
/// `file_exists` на отсутствующий файл, правило несёт `ad`/`fix_hint`/`skill`.
fn repo_card_fixture(home: &Path, name: &str) -> String {
    let repo = home.join(name);
    let handoff = repo.join(".arch-handoff");
    std::fs::create_dir_all(&handoff).expect("mkdir");
    std::fs::write(
        handoff.join("CONSTRAINTS.yaml"),
        "rules:\n  - name: spine_present\n    type: file_exists\n    path: \"ARCHITECTURE-SPINE.md\"\n    severity: error\n    ad: AD-9\n    fix_hint: \"вернуть ARCHITECTURE-SPINE.md в корень\"\n    skill: spine-invariants\n",
    )
    .expect("constraints");
    repo.display().to_string()
}

/// Фикстура репозитория `name` с CONSTRAINTS.yaml: правило `file_exists`
/// уровня error на файл, который есть либо нет.
fn repo_fixture(home: &Path, name: &str, create_required_file: bool) -> String {
    let repo = home.join(name);
    let handoff = repo.join(".arch-handoff");
    std::fs::create_dir_all(&handoff).expect("mkdir");
    std::fs::write(
        handoff.join("CONSTRAINTS.yaml"),
        "rules:\n  - name: spine_present\n    type: file_exists\n    path: \"ARCHITECTURE-SPINE.md\"\n    severity: error\n",
    )
    .expect("constraints");
    if create_required_file {
        std::fs::write(repo.join("ARCHITECTURE-SPINE.md"), "# spine\n").expect("spine");
    }
    repo.display().to_string()
}

/// Фикстура кейса трассировки: полный (AD-1 с правилом C-001) либо
/// рвущий обязательное звено (AD-1 без `verified_by`/`unverifiable`).
fn case_fixture(home: &Path, full: bool) -> String {
    let case = home.join("case");
    let model = case.join("model");
    std::fs::create_dir_all(&model).expect("mkdir model");
    let ad_links = if full { "\nverified_by: [C-001]" } else { "" };
    std::fs::write(
        model.join("AD-1.md"),
        format!("---\nid: AD-1\ntype: ad\ntitle: Инвариант\nstatus: ADOPTED{ad_links}\n---\n\nПравило.\n"),
    )
    .expect("AD");
    std::fs::write(
        model.join("CMP-001.md"),
        "---\nid: CMP-001\ntype: cmp\ntitle: Компонент\nstatus: designed\nimplements: [AD-1]\n---\n\nТело.\n",
    )
    .expect("CMP");
    std::fs::write(
        case.join("CONSTRAINTS.yaml"),
        "constraints:\n  - id: C-001\n    name: правило\n",
    )
    .expect("constraints");
    case.display().to_string()
}

/// Фикстура модели для `model_query`.
fn model_fixture(home: &Path) -> String {
    let model = home.join("model");
    std::fs::create_dir_all(&model).expect("mkdir model");
    std::fs::write(
        model.join("AD-1.md"),
        "---\nid: AD-1\ntype: ad\ntitle: Инвариант\nstatus: ADOPTED\n---\n\nПравило.\n",
    )
    .expect("AD");
    std::fs::write(
        model.join("ADR-001-x.md"),
        "---\nid: ADR-001\ntype: adr\ntitle: Решение\nstatus: Accepted\nimplements: [AD-1]\n---\n\nКонтекст.\n",
    )
    .expect("ADR");
    model.display().to_string()
}

#[test]
fn handshake_then_tools_list_over_stdio() {
    let home = tempfile::tempdir().expect("tmp");
    let responses = mcp_serve(
        home.path(),
        &batch(&[
            json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
                "protocolVersion":"2025-06-18","capabilities":{},
                "clientInfo":{"name":"claude-code","version":"1.0"}}})
            .to_string(),
            json!({"jsonrpc":"2.0","method":"notifications/initialized"}).to_string(),
            json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}).to_string(),
        ]),
    );
    assert_eq!(
        responses.len(),
        2,
        "уведомление не получает ответа: {responses:?}"
    );
    let init = &responses[0]["result"];
    assert_eq!(init["protocolVersion"], "2025-06-18");
    assert_eq!(init["serverInfo"]["name"], "arch-harness");
    assert!(init["capabilities"]["tools"].is_object());
    let tools = responses[1]["result"]["tools"].as_array().expect("tools");
    let names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
    for want in [
        "spine_lint",
        "fitness_check",
        "significance_score",
        "significance_from_diff",
        "trace_check",
        "model_query",
        "rubric_run",
        "kb_search",
        "skill_search",
        "skill_load",
        "mermaid_render",
    ] {
        assert!(names.contains(&want), "нет инструмента {want}: {names:?}");
    }
    // Split-judge (механический судья без LLM) и read-only мост реестра
    // (детерминированный контур + верификаторы транша 1 инверсии) тоже
    // в списке read-only режима.
    for want in [
        "rubric_prompt",
        "rubric_verify",
        "openapi_lint",
        "asyncapi_lint",
        "contract_diff",
        "fleet_audit",
        "agentsmd_lint",
        "archify_validate",
        "rubric_list",
        "plugin_list",
        "nfr_check",
        "model_validate",
        "delta_guard",
        "evidence_verify",
    ] {
        assert!(names.contains(&want), "нет инструмента {want}: {names:?}");
    }
    assert_eq!(
        tools.len(),
        25,
        "ровно 25 инструментов в ro-режиме (13 ручных + 12 read-only моста)"
    );
    // rw-контур и write/exec-принадлежность хоста закрыты в ro-режиме.
    for banned in [
        "handoff_create",
        "adr_new",
        "agentsmd_generate",
        "skill_distill",
        "evidence_pack",
        "delta_propose",
        "bash",
        "write_file",
        "harness_run",
    ] {
        assert!(
            !names.contains(&banned),
            "инструмент {banned} не должен отдаваться в ro-режиме: {names:?}"
        );
    }
}

#[test]
fn ping_and_unknown_method_and_broken_json() {
    let home = tempfile::tempdir().expect("tmp");
    let responses = mcp_serve(
        home.path(),
        &batch(&[
            json!({"jsonrpc":"2.0","id":1,"method":"ping"}).to_string(),
            json!({"jsonrpc":"2.0","id":2,"method":"resources/list"}).to_string(),
            "{битый json".to_string(),
            json!({"jsonrpc":"2.0","id":3,"method":"ping"}).to_string(),
        ]),
    );
    assert_eq!(responses.len(), 4, "сервер ответил всем и остался жив");
    assert_eq!(responses[0]["result"], json!({}));
    assert_eq!(responses[1]["error"]["code"], -32601, "unknown method");
    assert_eq!(responses[2]["error"]["code"], -32700, "parse error");
    assert_eq!(responses[2]["id"], Value::Null);
    assert_eq!(responses[3]["result"], json!({}), "ping после мусора");
}

#[test]
fn unknown_tool_and_bad_arguments_are_32602() {
    let home = tempfile::tempdir().expect("tmp");
    let responses = mcp_serve(
        home.path(),
        &batch(&[
            call(1, "ghost_tool", &json!({})),
            call(2, "spine_lint", &json!({})), // нет path
            call(3, "fitness_check", &json!({"repo": 42})), // repo не строка
            call(4, "trace_check", &json!({"case": null})), // case не строка
        ]),
    );
    for (i, r) in responses.iter().enumerate() {
        assert_eq!(r["error"]["code"], -32602, "ответ {}: {r}", i + 1);
    }
}

#[test]
fn spine_lint_verdict_blocks_and_passes() {
    let home = tempfile::tempdir().expect("tmp");
    let bad = spine_fixture(home.path(), "bad", true);
    let good = spine_fixture(home.path(), "good", false);
    let responses = mcp_serve(
        home.path(),
        &batch(&[
            call(1, "spine_lint", &json!({"path": bad})),
            call(2, "spine_lint", &json!({"path": good})),
            call(
                3,
                "spine_lint",
                &json!({"path": home.path().join("ghost.md")}),
            ),
        ]),
    );
    let verdict = structured(&responses[0], 1);
    assert_eq!(verdict["passed"], false, "{verdict}");
    assert!(verdict["error_count"].as_u64().expect("число") >= 1);
    let rules: Vec<&str> = verdict["issues"]
        .as_array()
        .expect("issues")
        .iter()
        .filter_map(|i| i["rule"].as_str())
        .collect();
    assert!(rules.contains(&"dup_ad_id"), "{rules:?}");
    assert!(rules.contains(&"empty_field"), "{rules:?}");

    let verdict = structured(&responses[1], 2);
    assert_eq!(verdict["passed"], true, "{verdict}");
    assert_eq!(verdict["issue_count"], 0);

    // Файл не читается — доменный сбой: isError-результат, не protocol error.
    assert_eq!(responses[2]["result"]["isError"], true);
    assert!(responses[2].get("error").is_none());
}

#[test]
fn fitness_check_verdict_blocks_and_passes() {
    let home = tempfile::tempdir().expect("tmp");
    let broken = repo_fixture(home.path(), "broken-repo", false);
    let clean = repo_fixture(home.path(), "clean-repo", true);
    let responses = mcp_serve(
        home.path(),
        &batch(&[
            call(1, "fitness_check", &json!({"repo": broken})),
            call(2, "fitness_check", &json!({"repo": clean})),
        ]),
    );
    let verdict = structured(&responses[0], 1);
    assert_eq!(verdict["passed"], false, "{verdict}");
    let issue = &verdict["issues"][0];
    assert_eq!(issue["rule"], "spine_present");
    assert_eq!(issue["severity"], "error");
    // Правило без карточки: аддитивные поля (ad/fix_hint/skill) отсутствуют
    // в JSON, а не null (SDK-контракт v1 не ломается).
    for key in ["ad", "adr", "rationale", "owner", "fix_hint", "skill"] {
        assert!(issue.get(key).is_none(), "у находки без карточки нет {key}");
    }

    let verdict = structured(&responses[1], 2);
    assert_eq!(verdict["passed"], true, "{verdict}");
    assert!(
        verdict["summary"]
            .as_str()
            .expect("summary")
            .contains("Правил: 1")
    );
}

/// Критерий приёмки бэклога: в ответе `fitness_check` по нарушению видно,
/// какой AD-* задет и какой скилл загрузить для исправления.
#[test]
fn fitness_check_issue_carries_card_context() {
    let home = tempfile::tempdir().expect("tmp");
    let broken = repo_card_fixture(home.path(), "card-repo");
    let responses = mcp_serve(
        home.path(),
        &batch(&[call(1, "fitness_check", &json!({"repo": broken}))]),
    );
    let verdict = structured(&responses[0], 1);
    assert_eq!(verdict["passed"], false, "{verdict}");
    let issue = &verdict["issues"][0];
    assert_eq!(issue["rule"], "spine_present");
    assert_eq!(issue["ad"], "AD-9", "{issue}");
    assert_eq!(
        issue["fix_hint"], "вернуть ARCHITECTURE-SPINE.md в корень",
        "{issue}"
    );
    assert_eq!(issue["skill"], "spine-invariants", "{issue}");
}

#[test]
fn significance_score_routes_change() {
    let home = tempfile::tempdir().expect("tmp");
    let responses = mcp_serve(
        home.path(),
        &batch(&[
            call(
                1,
                "significance_score",
                &json!({"triggers": {"security_boundary_change": true}}),
            ),
            call(2, "significance_score", &json!({"triggers": {}})),
        ]),
    );
    let critical = structured(&responses[0], 1);
    assert_eq!(critical["route"], "Critical");
    assert_eq!(critical["score"], 1);
    let fast = structured(&responses[1], 2);
    assert_eq!(fast["route"], "Fast");
    assert_eq!(fast["score"], 0);
}

/// git в каталоге с тестовой идентичностью коммиттера
/// (образец — `src/delta.rs::make_guard_repo`).
fn git(dir: &Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .output()
        .expect("git");
    assert!(
        out.status.success(),
        "git {}: {}",
        args.join(" "),
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Репо-фикстура для `significance_from_diff`: один коммит с README.
fn git_repo_fixture(home: &Path, name: &str) -> std::path::PathBuf {
    let repo = home.join(name);
    std::fs::create_dir_all(&repo).expect("mkdir");
    git(&repo, &["init", "-q"]);
    std::fs::write(repo.join("README.md"), "# t\n").expect("readme");
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-q", "-m", "init"]);
    repo
}

/// Критерий приёмки бэклога: агент, заявивший Fast (пустой `declared`) на
/// диффе с новым каталогом и манифестом, получает Standard или выше с
/// указанием файла-причины.
#[test]
fn significance_from_diff_overrides_fast_claim() {
    let home = tempfile::tempdir().expect("tmp");
    let repo = git_repo_fixture(home.path(), "diff-repo");
    // Рабочее дерево: новый компонент untracked — каталог services/risk с
    // манифестом (детектор new_component) и строкой зависимости (new_vendor).
    std::fs::create_dir_all(repo.join("services/risk/src")).expect("mkdir svc");
    std::fs::write(
        repo.join("services/risk/Cargo.toml"),
        "[package]\nname = \"risk\"\nversion = \"0.1.0\"\n\n[dependencies]\nserde = \"1.0\"\n",
    )
    .expect("manifest");
    std::fs::write(repo.join("services/risk/src/lib.rs"), "pub fn f() {}\n").expect("src");
    let repo_str = repo.display().to_string();

    let responses = mcp_serve(
        home.path(),
        &batch(&[
            // Пустой declared («заявили Fast»): дифф обязан поднять маршрут.
            call(1, "significance_from_diff", &json!({"path": repo_str})),
            // Частично заявлено: источник new_component — «declared+diff».
            call(
                2,
                "significance_from_diff",
                &json!({"path": repo_str, "declared": {"new_component": true}}),
            ),
            // Не git-репозиторий — доменный сбой (isError), не protocol error.
            call(
                3,
                "significance_from_diff",
                &json!({"path": home.path().join("ghost")}),
            ),
        ]),
    );

    let verdict = structured(&responses[0], 1);
    assert_eq!(verdict["route"], "Standard", "{verdict}");
    assert_eq!(verdict["score"], 2, "{verdict}");
    assert_eq!(verdict["sources"]["new_component"], "diff", "{verdict}");
    assert_eq!(verdict["sources"]["new_vendor"], "diff", "{verdict}");
    let undeclared = verdict["undeclared"].as_array().expect("undeclared");
    let nc = undeclared
        .iter()
        .find(|u| u["trigger"] == "new_component")
        .expect("new_component в undeclared");
    let evidence: Vec<&str> = nc["evidence"]
        .as_array()
        .expect("evidence")
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert!(
        evidence
            .iter()
            .any(|e| e.contains("services/risk/Cargo.toml")),
        "файл-причина new_component: {evidence:?}"
    );
    assert!(
        verdict["summary"]
            .as_str()
            .expect("summary")
            .contains("не заявлены"),
        "{verdict}"
    );

    let verdict = structured(&responses[1], 2);
    assert_eq!(
        verdict["sources"]["new_component"], "declared+diff",
        "{verdict}"
    );
    // new_vendor остался незаявленным — anti-bypass сигнал сохраняется.
    let undeclared = verdict["undeclared"].as_array().expect("undeclared");
    assert!(
        undeclared.iter().any(|u| u["trigger"] == "new_vendor"),
        "{verdict}"
    );

    assert_eq!(responses[2]["result"]["isError"], true);
    assert!(responses[2].get("error").is_none());
}

/// Режим `base_ref`: дифф `BASE_REF...HEAD` по закоммиченному компоненту.
#[test]
fn significance_from_diff_with_base_ref() {
    let home = tempfile::tempdir().expect("tmp");
    let repo = git_repo_fixture(home.path(), "ref-repo");
    std::fs::create_dir_all(repo.join("services/risk")).expect("mkdir svc");
    std::fs::write(
        repo.join("services/risk/Cargo.toml"),
        "[package]\nname = \"risk\"\nversion = \"0.1.0\"\n\n[dependencies]\nserde = \"1.0\"\n",
    )
    .expect("manifest");
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-q", "-m", "add risk service"]);
    let repo_str = repo.display().to_string();

    let responses = mcp_serve(
        home.path(),
        &batch(&[call(
            1,
            "significance_from_diff",
            &json!({"path": repo_str, "base_ref": "HEAD~1"}),
        )]),
    );
    let verdict = structured(&responses[0], 1);
    assert_eq!(verdict["route"], "Standard", "{verdict}");
    assert_eq!(verdict["sources"]["new_component"], "diff", "{verdict}");
    assert_eq!(verdict["sources"]["new_vendor"], "diff", "{verdict}");
}

#[test]
fn trace_check_verdict_blocks_broken_chain() {
    let home = tempfile::tempdir().expect("tmp");
    let full = case_fixture(home.path(), true);
    let broken = case_fixture(&home.path().join("other"), false);
    // case_fixture пишет от переданного корня: для broken корень другой.
    let responses = mcp_serve(
        home.path(),
        &batch(&[
            call(1, "trace_check", &json!({"case": full})),
            call(2, "trace_check", &json!({"case": broken})),
        ]),
    );
    let verdict = structured(&responses[0], 1);
    assert_eq!(verdict["passed"], true, "{verdict}");
    assert!(
        verdict["report_markdown"]
            .as_str()
            .expect("markdown")
            .contains("| Звено | Покрыто |")
    );

    let verdict = structured(&responses[1], 2);
    assert_eq!(verdict["passed"], false, "{verdict}");
    let rules: Vec<&str> = verdict["issues"]
        .as_array()
        .expect("issues")
        .iter()
        .filter_map(|i| i["rule"].as_str())
        .collect();
    assert!(rules.contains(&"ad-not-verified"), "{rules:?}");
}

#[test]
fn model_query_lists_and_shows_card() {
    let home = tempfile::tempdir().expect("tmp");
    let dir = model_fixture(home.path());
    let responses = mcp_serve(
        home.path(),
        &batch(&[
            call(1, "model_query", &json!({"dir": dir, "type": "adr"})),
            call(2, "model_query", &json!({"dir": dir, "id": "ADR-001"})),
            call(3, "model_query", &json!({"dir": dir, "id": "CMP-999"})),
            call(4, "model_query", &json!({"dir": dir, "type": "widget"})),
        ]),
    );
    let list = structured(&responses[0], 1);
    assert_eq!(list["total"], 1, "фильтр adr: {list}");
    assert_eq!(list["entities"][0]["id"], "ADR-001");

    let card = structured(&responses[1], 2);
    assert_eq!(card["entity"]["id"], "ADR-001");
    assert_eq!(card["entity"]["links"]["implements"], json!(["AD-1"]));
    assert!(card["card"].as_str().expect("карточка").contains("Решение"));

    // Несуществующий id — доменная ошибка (isError), не protocol error.
    assert_eq!(responses[2]["result"]["isError"], true);
    // Несуществующий тип — ошибка параметров -32602.
    assert_eq!(responses[3]["error"]["code"], -32602);
}

#[test]
fn rubric_run_without_api_key_is_clear_json_rpc_error() {
    let home = tempfile::tempdir().expect("tmp");
    // arch_cmd снимает ключи провайдеров и изолирует HOME (файл-ключ kimi
    // тоже недоступен): дефолтный deepseek без ключа → -32603 с подсказкой.
    let responses = mcp_serve(
        home.path(),
        &batch(&[
            call(
                1,
                "rubric_run",
                &json!({"rubric": "solution_architecture", "target_text": "текст"}),
            ),
            call(2, "ping_tool", &json!({})), // неизвестный инструмент после ошибки
        ]),
    );
    let error = &responses[0]["error"];
    assert_eq!(error["code"], -32603, "{error}");
    let message = error["message"].as_str().expect("сообщение");
    assert!(message.contains("API-ключ"), "{message}");
    assert!(message.contains("DEEPSEEK_API_KEY"), "{message}");
    // Сервер жив после ошибки rubric_run.
    assert_eq!(responses[1]["error"]["code"], -32602);
}

/// Живой прогон `rubric_run` с реальным ключом (сеть + LLM): в общем гейте
/// пропускается, запуск — `cargo test --test mcp_serve -- --ignored`.
#[test]
#[ignore = "нужен API-ключ LLM и сеть (живой смоук)"]
fn rubric_run_live_with_key() {
    let home = tempfile::tempdir().expect("tmp");
    let doc = home.path().join("doc.md");
    std::fs::write(&doc, "# Архитектура\n\nСистема из одного скрипта.\n").expect("doc");
    // Без изоляции окружения: ключи берутся из env разработчика.
    let output = assert_cmd::Command::cargo_bin("arch-be")
        .expect("бинарь")
        .args(["mcp", "serve"])
        .write_stdin(
            call(
                1,
                "rubric_run",
                &json!({"rubric": "solution_architecture", "target": doc.display().to_string()}),
            ) + "\n",
        )
        .output()
        .expect("запуск");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("utf8");
    let response: Value =
        serde_json::from_str(stdout.lines().next().expect("ответ")).expect("json");
    if let Some(error) = response.get("error") {
        panic!("живой вызов не должен давать protocol error: {error}");
    }
    let result = &response["result"];
    assert!(
        result["isError"] != true,
        "живой вызов не должен давать isError: {result}"
    );
    let verdict = &result["structuredContent"];
    assert!(verdict["weighted_total"].as_f64().expect("балл") > 0.0);
    assert!(!verdict["scores"].as_array().expect("оценки").is_empty());
}

#[test]
fn rw_mode_lists_bridge_write_tools_over_stdio() {
    let home = tempfile::tempdir().expect("tmp");
    let responses = mcp_serve_with_args(
        home.path(),
        &["--rw"],
        &batch(&[json!({"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}).to_string()]),
    );
    let tools = responses[0]["result"]["tools"].as_array().expect("tools");
    let names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
    // В core-сборке домены harness/distill не собираются — их rw-инструменты
    // (handoff_create, skill_distill) мост не отдаёт (спеки строятся от реестра).
    #[cfg(feature = "harness")]
    let rw_want = [
        "handoff_create",
        "adr_new",
        "agentsmd_generate",
        "skill_distill",
        "reverse_survey",
        "archify_deliver",
        "evidence_pack",
        "delta_propose",
        // read-only мост остаётся доступен и под --rw:
        "openapi_lint",
        "nfr_check",
        "rubric_prompt",
    ];
    #[cfg(not(feature = "harness"))]
    let rw_want = [
        "adr_new",
        "agentsmd_generate",
        "reverse_survey",
        "archify_deliver",
        "evidence_pack",
        "delta_propose",
        "openapi_lint",
        "nfr_check",
        "rubric_prompt",
    ];
    for want in rw_want {
        assert!(
            names.contains(&want),
            "нет инструмента {want} в --rw: {names:?}"
        );
    }
    #[cfg(not(feature = "harness"))]
    for banned in ["handoff_create", "skill_distill"] {
        assert!(
            !names.contains(&banned),
            "harness-инструмент {banned} не должен отдаваться в core: {names:?}"
        );
    }
    // Принадлежность хоста не отдаётся ни в одном режиме.
    for banned in [
        "bash",
        "read_file",
        "write_file",
        "edit_file",
        "glob",
        "grep",
        "harness_run",
        "subagent_run",
        "ralph_run",
        "worktree_new",
        "web_search",
        "web_fetch",
        "rubric_evaluate",
        "rubric_generate",
    ] {
        assert!(
            !names.contains(&banned),
            "never-инструмент {banned} не должен отдаваться и под --rw: {names:?}"
        );
    }
    // Аннотации mutating-инструмента (handoff_create — домен сборки `harness`).
    #[cfg(feature = "harness")]
    {
        let handoff = tools
            .iter()
            .find(|t| t["name"] == "handoff_create")
            .expect("handoff_create");
        assert_eq!(handoff["annotations"]["readOnlyHint"], false);
        assert_eq!(handoff["annotations"]["destructiveHint"], true);
    }
}

#[test]
fn bridge_openapi_lint_call_over_stdio() {
    let home = tempfile::tempdir().expect("tmp");
    std::fs::write(
        home.path().join("api.yaml"),
        "openapi: 3.0.3\ninfo:\n  title: T\n  version: 1.0.0\npaths:\n  /v1/x:\n    get:\n      operationId: getX\n      responses:\n        '200':\n          description: ok\n",
    )
    .expect("контракт");
    let responses = mcp_serve(
        home.path(),
        &batch(&[call(
            1,
            "openapi_lint",
            &json!({"path": home.path().join("api.yaml")}),
        )]),
    );
    let result = &responses[0]["result"];
    assert_eq!(result["isError"], false, "{result}");
    let verdict = structured(&responses[0], 1);
    assert_eq!(verdict["tool"], "openapi_lint");
    let output = verdict["output"].as_str().expect("output");
    assert!(
        output.contains("openapi:"),
        "отчёт мостового инструмента: {output}"
    );
    // text-часть моста — сырой вывод инструмента (не JSON-обёртка).
    assert_eq!(result["content"][0]["text"].as_str().expect("text"), output);
}

/// Фикстура кейса NFR: `model/` с INT-hop'ом (бюджет `hop_budget_ms`) и NFR
/// с целью p99 `target_ms` (образец — `tests/cli.rs::nfr_budget_case`).
fn nfr_case_fixture(home: &Path, name: &str, target_ms: u32, hop_budget_ms: Option<u32>) -> String {
    let case = home.join(name);
    let model = case.join("model");
    std::fs::create_dir_all(&model).expect("mkdir model");
    let budget = hop_budget_ms.map_or(String::new(), |b| format!("latency_budget_ms: {b}\n"));
    std::fs::write(
        model.join("INT-001-hop.md"),
        format!("---\nid: INT-001\ntype: int\ntitle: Hop\nstatus: accepted\n{budget}---\n"),
    )
    .expect("write INT");
    std::fs::write(
        model.join("NFR-001-lat.md"),
        format!(
            "---\nid: NFR-001\ntype: nfr\ntitle: Latency\nstatus: accepted\n\
             verification: histogram\np99_target_ms: {target_ms}\naffects: [INT-001]\n---\n"
        ),
    )
    .expect("write NFR");
    case.display().to_string()
}

/// Транш 1 инверсии: `nfr_check` по NDJSON возвращает JSON-вердикт
/// passed/issues/summary в `structuredContent.output` (мостовой вызов,
/// без bash у агента). Зелёный кейс — passed, превышение бюджета — FAIL
/// с виновным hop'ом в находке.
#[test]
fn bridge_nfr_check_verdict_over_stdio() {
    let home = tempfile::tempdir().expect("tmp");
    let ok_case = nfr_case_fixture(home.path(), "nfr-ok", 2000, Some(800));
    let bad_case = nfr_case_fixture(home.path(), "nfr-bad", 2000, Some(3000));
    let responses = mcp_serve(
        home.path(),
        &batch(&[
            call(1, "nfr_check", &json!({"path": ok_case, "kind": "all"})),
            call(2, "nfr_check", &json!({"path": bad_case, "kind": "budget"})),
            // Кейса без model/ — доменный сбой: isError, не protocol error.
            call(3, "nfr_check", &json!({"path": home.path().join("ghost")})),
        ]),
    );
    let verdict = structured(&responses[0], 1);
    assert_eq!(verdict["tool"], "nfr_check");
    let output: Value = serde_json::from_str(verdict["output"].as_str().expect("output"))
        .expect("output — JSON-вердикт {passed, issues, summary}");
    assert_eq!(output["passed"], true, "{output}");
    assert_eq!(
        output["checks"],
        json!(["budget", "availability", "capacity", "cost"])
    );

    let verdict = structured(&responses[1], 2);
    let output: Value =
        serde_json::from_str(verdict["output"].as_str().expect("output")).expect("JSON-вердикт");
    assert_eq!(output["passed"], false, "{output}");
    let issue = &output["issues"][0];
    assert_eq!(issue["rule"], "budget-exceeded");
    assert!(
        issue["message"]
            .as_str()
            .expect("message")
            .contains("INT-001=3000"),
        "виновный hop в находке: {issue}"
    );

    assert_eq!(responses[2]["result"]["isError"], true);
    assert!(responses[2].get("error").is_none());
}

/// Транш 1 инверсии, rw-контур: `delta_propose` недоступен в ro-режиме
/// (-32602), под `--rw` создаёт скелет дельты мостовым вызовом.
#[test]
fn bridge_delta_propose_rw_only_over_stdio() {
    let home = tempfile::tempdir().expect("tmp");
    let repo = home.path().join("repo");
    std::fs::create_dir_all(&repo).expect("mkdir repo");
    let repo_str = repo.display().to_string();
    // ro-режим: инструмент закрыт, ничего не создаётся.
    let responses = mcp_serve(
        home.path(),
        &batch(&[call(
            1,
            "delta_propose",
            &json!({"name": "saga-pilot", "path": repo_str}),
        )]),
    );
    assert_eq!(responses[0]["error"]["code"], -32602);
    assert!(!repo.join("changes/saga-pilot/DELTA.md").exists());
    // rw-режим: скелет создан, вердикт — JSON в output моста.
    let responses = mcp_serve_with_args(
        home.path(),
        &["--rw"],
        &batch(&[call(
            1,
            "delta_propose",
            &json!({"name": "saga-pilot", "path": repo_str}),
        )]),
    );
    let verdict = structured(&responses[0], 1);
    assert_eq!(verdict["tool"], "delta_propose");
    let output: Value =
        serde_json::from_str(verdict["output"].as_str().expect("output")).expect("JSON-вердикт");
    assert!(
        output["created"]
            .as_str()
            .expect("created")
            .ends_with("changes/saga-pilot/DELTA.md"),
        "{output}"
    );
    assert!(repo.join("changes/saga-pilot/DELTA.md").is_file());
}

#[test]
fn split_judge_prompt_then_verify_over_stdio() {
    let home = tempfile::tempdir().expect("tmp");
    std::fs::write(
        home.path().join("r.yaml"),
        "name: t-rubric\ndescription: тестовая\nscale_max: 5\norigin: anchor\ncriteria:\n  - id: context\n    name: Контекст\n    description: Описан контекст\n    weight: 1.0\n",
    )
    .expect("рубрика");
    let prompt = call(
        1,
        "rubric_prompt",
        &json!({"rubric": home.path().join("r.yaml"), "target_text": "контекст описан явно"}),
    );
    let verify = call(
        2,
        "rubric_verify",
        &json!({
            "rubric": home.path().join("r.yaml"),
            "target_text": "контекст описан явно",
            "answers": [
                "{\"scores\":[{\"criterion_id\":\"context\",\"score\":4,\"rationale\":\"Цитата: \\\"контекст описан явно\\\" — да\"}],\"verdict\":\"годно\"}",
                "битый ответ хоста"
            ],
        }),
    );
    let responses = mcp_serve(home.path(), &batch(&[prompt, verify]));
    // Фаза 1: промпты + схема + k из judge-конфига.
    let prompt_out = structured(&responses[0], 1);
    assert!(
        prompt_out["system_prompt"]
            .as_str()
            .expect("system")
            .contains("НАЧАЛО ОЦЕНИВАЕМОГО ТЕКСТА")
    );
    assert!(prompt_out["response_json_schema"]["properties"]["scores"].is_object());
    assert_eq!(prompt_out["judge_config"]["samples"], 3);
    // Фаза 2: отчёт по одному валидному ответу, битый посчитан в dropped.
    let verdict = structured(&responses[1], 2);
    assert_eq!(verdict["judge_samples"], 1);
    assert_eq!(
        verdict["answers"],
        json!({"total": 2, "valid": 1, "dropped": 1})
    );
    assert_eq!(verdict["scores"][0]["score"], 4);
    assert_eq!(verdict["scores"][0]["flags"], json!([]));
    assert!(
        (verdict["weighted_total"].as_f64().expect("итог") - 4.0).abs() < 1e-9,
        "{}",
        verdict["weighted_total"]
    );
    assert!(
        verdict["warning"].is_string(),
        "доля отброшенных: {verdict}"
    );
}
