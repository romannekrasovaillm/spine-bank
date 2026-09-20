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
        "rules_suggest",
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
        "model_drift",
        "delta_guard",
        "evidence_verify",
        "landscape_report",
        "adr_registry",
        "rules_report",
        "openspec_coverage",
        "model_graph",
        "architect_review",
        "change_impact",
        "trust_report",
        "verdict_explain",
    ] {
        assert!(names.contains(&want), "нет инструмента {want}: {names:?}");
    }
    assert_eq!(
        tools.len(),
        36,
        "ровно 36 инструментов в ro-режиме (16 ручных + 20 read-only моста; \
         verdict_explain и trust_report — волна W)"
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
fn prompts_list_and_get_over_stdio() {
    let home = tempfile::tempdir().expect("tmp");
    let responses = mcp_serve(
        home.path(),
        &batch(&[
            json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
                "protocolVersion":"2025-06-18","capabilities":{},
                "clientInfo":{"name":"claude-code","version":"1.0"}}})
            .to_string(),
            json!({"jsonrpc":"2.0","id":2,"method":"prompts/list","params":{}}).to_string(),
            json!({"jsonrpc":"2.0","id":3,"method":"prompts/get","params":{
                "name":"spine-architect-review"}})
            .to_string(),
            json!({"jsonrpc":"2.0","id":4,"method":"prompts/get","params":{"name":"ghost"}})
                .to_string(),
            json!({"jsonrpc":"2.0","id":5,"method":"resources/list","params":{}}).to_string(),
        ]),
    );
    // initialize рекламирует capability prompts.
    assert!(responses[0]["result"]["capabilities"]["prompts"].is_object());
    // prompts/list: ровно восемь плейбуков spine-workflows (дом изолирован —
    // тексты и описания из встроенных ассетов).
    let prompts = responses[1]["result"]["prompts"]
        .as_array()
        .expect("prompts");
    assert_eq!(prompts.len(), 8, "восемь плейбуков: {prompts:?}");
    let names: Vec<&str> = prompts.iter().filter_map(|p| p["name"].as_str()).collect();
    for want in [
        "spine-quickstart",
        "spine-content-bootstrap",
        "spine-architect-review",
        "spine-adr-judge",
        "spine-contracts-gate",
        "spine-archify-viz",
        "spine-fitness-gate",
        "spine-bundle",
    ] {
        assert!(names.contains(&want), "нет промпта {want}: {names:?}");
    }
    // prompts/get: одно user-сообщение с инструкцией и телом плейбука.
    let got = &responses[2]["result"];
    assert!(
        got["description"].as_str().is_some_and(|d| !d.is_empty()),
        "description из frontmatter: {got}"
    );
    let messages = got["messages"].as_array().expect("messages");
    assert_eq!(messages.len(), 1, "одно user-сообщение: {got}");
    assert_eq!(messages[0]["role"], "user");
    assert_eq!(messages[0]["content"]["type"], "text");
    let text = messages[0]["content"]["text"].as_str().expect("text");
    assert!(
        text.contains("spine-architect-review") && text.contains("significance_score"),
        "тело плейбука в сообщении: {}",
        &text[..text.len().min(200)]
    );
    // Неизвестный промпт → -32602; resources/* по-прежнему не поддержаны → -32601.
    assert_eq!(responses[3]["error"]["code"], -32602, "{}", responses[3]);
    assert_eq!(responses[4]["error"]["code"], -32601, "{}", responses[4]);
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
    // handoff_create — в обеих сборках (генерация пакета — core-модуль
    // `crate::handoff`, волна 2 п.10); skill_distill — только в harness.
    let mut rw_want = vec![
        "handoff_create",
        "adr_new",
        "agentsmd_generate",
        "reverse_survey",
        "archify_deliver",
        "evidence_pack",
        "delta_propose",
        // read-only мост остаётся доступен и под --rw:
        "openapi_lint",
        "nfr_check",
        "rubric_prompt",
    ];
    #[cfg(feature = "harness")]
    rw_want.push("skill_distill");
    for want in rw_want {
        assert!(
            names.contains(&want),
            "нет инструмента {want} в --rw: {names:?}"
        );
    }
    #[cfg(not(feature = "harness"))]
    for banned in ["skill_distill"] {
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
    // Аннотации mutating-инструмента (handoff_create — в обеих сборках).
    let handoff = tools
        .iter()
        .find(|t| t["name"] == "handoff_create")
        .expect("handoff_create");
    assert_eq!(handoff["annotations"]["readOnlyHint"], false);
    assert_eq!(handoff["annotations"]["destructiveHint"], true);
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

/// Журнал вызовов (пункт 9): после `tools/call` проектный журнал
/// `.arch-handoff/mcp-calls.jsonl` (cwd сервера = дом теста) существует и
/// несёт `tool`/`verdict`/`duration_ms`/`ts`; `fail` несёт имена правил
/// error-находок, неизвестный инструмент — `invalid`. Содержимое
/// аргументов в журнале отсутствует.
#[test]
fn tool_calls_are_journaled_to_project_journal() {
    let home = tempfile::tempdir().expect("tmp");
    let ok_repo = repo_fixture(home.path(), "repo-ok", true);
    let bad_repo = repo_fixture(home.path(), "repo-bad", false);
    let responses = mcp_serve(
        home.path(),
        &batch(&[
            call(1, "fitness_check", &json!({"repo": ok_repo})),
            call(2, "fitness_check", &json!({"repo": bad_repo})),
            call(3, "ghost", &json!({})),
        ]),
    );
    assert_eq!(responses.len(), 3, "все вызовы отвечены: {responses:?}");

    let journal = home.path().join(".arch-handoff/mcp-calls.jsonl");
    let text = std::fs::read_to_string(&journal)
        .expect("журнал создан рядом с cwd сервера (fail-soft не молчит на нормальной ФС)");
    let entries: Vec<Value> = text
        .lines()
        .map(|l| serde_json::from_str(l).expect("строка журнала — валидный JSON"))
        .collect();
    assert_eq!(entries.len(), 3, "три вызова — три записи: {entries:?}");
    // pass: чистый репозиторий.
    assert_eq!(entries[0]["tool"], "fitness_check");
    assert_eq!(entries[0]["verdict"], "pass");
    assert!(entries[0]["duration_ms"].is_number(), "{}", entries[0]);
    assert!(
        entries[0]["ts"].as_str().is_some_and(|t| t.contains('T')),
        "RFC 3339 штамп: {}",
        entries[0]
    );
    // fail: нарушение — имена правил из error-находок.
    assert_eq!(entries[1]["verdict"], "fail");
    assert_eq!(entries[1]["rules"], json!(["spine_present"]));
    // Неизвестный инструмент доехал до диспетчера и отвечен -32602 → invalid.
    assert_eq!(entries[2]["tool"], "ghost");
    assert_eq!(entries[2]["verdict"], "invalid");
    // Содержимого аргументов в журнале нет (контракт приватности).
    assert!(
        !text.contains("repo-ok"),
        "пути-аргументы не журналируются: {text}"
    );
}

/// Транш 2 инверсии: отчётные read-only инструменты по NDJSON.
/// `adr_registry` — счётчики и strict-гейт по находкам; `openspec_coverage` —
/// покрытие требований (счётчики + strict); `model_graph` — граф текстом;
/// `landscape_report` — markdown-отчёт. Всё — мостовые вызовы с JSON в
/// `structuredContent.output`.
#[test]
fn bridge_tranche2_reports_over_stdio() {
    let home = tempfile::tempdir().expect("tmp");

    // Реестр ADR: два проекта, коллизия номера ADR-001 (разные заголовки).
    let adr_root = home.path().join("adr-root");
    for (project, title) in [("p1", "Outbox"), ("p2", "Saga")] {
        let dir = adr_root.join(project).join("docs/adr");
        std::fs::create_dir_all(&dir).expect("mkdir adr");
        std::fs::write(
            dir.join("ADR-001-x.md"),
            format!("# ADR-001. {title}\n\n- Date: 2026-01-01\n- Status: Accepted\n"),
        )
        .expect("adr");
    }

    // OpenSpec-разметка: одно требование, покрытое правилом с covers.
    let os_root = home.path().join("os-root");
    let spec_dir = os_root.join("openspec/specs/payments");
    std::fs::create_dir_all(&spec_dir).expect("mkdir spec");
    std::fs::write(
        spec_dir.join("spec.md"),
        "# payments Specification\n\n## Requirements\n\n\
         ### Requirement: Точные деньги\nСистема SHALL хранить суммы в minor units.\n",
    )
    .expect("spec");
    let requirement_id = arch_harness::openspec::requirement_id(
        "payments",
        &["Система SHALL хранить суммы в minor units.".to_string()],
    );
    std::fs::write(
        os_root.join("CONSTRAINTS.yaml"),
        format!(
            "rules:\n  - name: money-detector\n    type: must_contain\n    glob: 'src/**'\n    pattern: 'minor_units'\n    covers: [\"{requirement_id}\"]\n"
        ),
    )
    .expect("constraints");

    // Модель для model_graph и ландшафта: AD-1 + ADR-001 (implements).
    let case = home.path().join("case");
    let model_dir = case.join("model");
    std::fs::create_dir_all(&model_dir).expect("mkdir model");
    std::fs::write(
        model_dir.join("AD-1.md"),
        "---\nid: AD-1\ntype: ad\ntitle: Инвариант\nstatus: ADOPTED\n---\n\nПравило.\n",
    )
    .expect("AD");
    std::fs::write(
        model_dir.join("ADR-001-x.md"),
        "---\nid: ADR-001\ntype: adr\ntitle: Решение\nstatus: Accepted\nimplements: [AD-1]\n---\n\nКонтекст.\n",
    )
    .expect("ADR");

    let responses = mcp_serve(
        home.path(),
        &batch(&[
            // adr_registry: отчёт (не гейт) — passed=true при находках.
            call(1, "adr_registry", &json!({"path": adr_root})),
            // strict — гейт: коллизия номеров → passed=false.
            call(
                2,
                "adr_registry",
                &json!({"path": adr_root, "strict": true}),
            ),
            // openspec_coverage strict: требование покрыто — passed=true.
            call(
                3,
                "openspec_coverage",
                &json!({"path": os_root, "strict": true}),
            ),
            // model_graph: текстовый граф модели.
            call(4, "model_graph", &json!({"dir": model_dir})),
            // landscape_report: markdown по корню с одним проектом-моделью.
            call(5, "landscape_report", &json!({"path": case})),
        ]),
    );
    assert_eq!(responses.len(), 5, "все вызовы отвечены: {responses:?}");

    let output = |idx: u64| -> Value {
        let verdict = structured(&responses[(idx - 1) as usize], idx);
        serde_json::from_str(verdict["output"].as_str().expect("output"))
            .expect("output — JSON-вердикт")
    };

    let registry = output(1);
    assert_eq!(registry["tool"], "adr_registry");
    assert_eq!(registry["entries"], 2, "{registry}");
    assert!(
        registry["finding_count"].as_u64().expect("findings") >= 1,
        "{registry}"
    );
    assert_eq!(registry["passed"], true, "отчёт, не гейт: {registry}");
    assert!(
        registry["findings"][0]["kind"].as_str().expect("kind") == "number_collision",
        "{registry}"
    );

    let registry_strict = output(2);
    assert_eq!(
        registry_strict["passed"], false,
        "strict-гейт: {registry_strict}"
    );

    let coverage = output(3);
    assert_eq!(coverage["tool"], "openspec_coverage");
    assert_eq!(coverage["total"], 1, "{coverage}");
    assert_eq!(coverage["covered"], 1, "{coverage}");
    assert_eq!(coverage["unresolved"], 0, "{coverage}");
    assert_eq!(coverage["passed"], true, "strict, всё покрыто: {coverage}");

    let graph = output(4);
    assert_eq!(graph["tool"], "model_graph");
    assert_eq!(graph["entities"], 2, "{graph}");
    assert_eq!(graph["edges"], 1, "{graph}");
    assert!(
        graph["graph"]
            .as_str()
            .expect("graph")
            .contains("implements → AD-1"),
        "{graph}"
    );

    let landscape = output(5);
    assert_eq!(landscape["tool"], "landscape_report");
    assert_eq!(
        landscape["systems"], 0,
        "в кейсе нет SYS — только AD/ADR: {landscape}"
    );
    assert!(
        landscape["report"]
            .as_str()
            .expect("report")
            .contains("# Ландшафт систем"),
        "{landscape}"
    );
}

/// Транш 3 инверсии: составные инструменты по NDJSON. `architect_review` —
/// единое ревью (маршрут + контур гейта + модель + контракты) с JSON-вердиктом
/// в `structuredContent.output`; `change_impact` — радиус изменения по
/// `id` (граф модели) и честная мягкая ошибка на неизвестном id.
/// T-02: реестр кейса-фикстуры. Копии (корневая и пакетная) обязаны
/// совпадать: расхождение гейт называет находкой `registry_diverged`, и
/// «чистый кейс» перестаёт быть чистым.
const REGISTRY: &str = "rules:\n  - id: C-001\n    name: no_float_money\n    type: must_not_contain\n    glob: \"**/*.rs\"\n    pattern: 'f64'\n    severity: error\n    owner: Команда платежей\n";

#[test]
fn bridge_tranche3_composite_tools_over_stdio() {
    let home = tempfile::tempdir().expect("tmp");

    // Кейс-фикстура (та же цепочка, что в src/review.rs::make_case):
    // git-репо, model/ с CMP→INT→SYS→AD→OWNER, CONSTRAINTS.yaml с C-001,
    // spine, контракт contracts/api.yaml, .arch-handoff/CONSTRAINTS.yaml.
    let case = home.path().join("case");
    let model_dir = case.join("model");
    std::fs::create_dir_all(&model_dir).expect("mkdir model");
    for (name, fm) in [
        (
            "AD-1.md",
            "---\nid: AD-1\ntype: ad\ntitle: Точные деньги\nstatus: ADOPTED\nverified_by: [C-001]\n---\n\nПравило.\n",
        ),
        (
            "INT-001.md",
            "---\nid: INT-001\ntype: int\ntitle: Рельс процессинга\nstatus: accepted\ncontract: contracts/api.yaml\n---\n\nТело.\n",
        ),
        (
            "OWNER-1.md",
            "---\nid: OWNER-1\ntype: owner\ntitle: Команда процессинга\nstatus: active\n---\n\nТело.\n",
        ),
    ] {
        std::fs::write(model_dir.join(name), fm).expect("сущность");
    }
    // CMP-001 отдельно: несёт связь на владельца (affects OWNER-1).
    std::fs::write(
        model_dir.join("CMP-001.md"),
        "---\nid: CMP-001\ntype: cmp\ntitle: Платёжный шлюз\nstatus: designed\nimplements: [AD-1]\ndepends_on: [INT-001]\naffects: [OWNER-1]\ncode_roots: [services/pay]\n---\n\nТело.\n",
    )
    .expect("CMP с владельцем");
    std::fs::write(case.join("CONSTRAINTS.yaml"), REGISTRY).expect("constraints");
    std::fs::write(
        case.join("ARCHITECTURE-SPINE.md"),
        "# Spine\n\n### AD-1. Точные деньги\n- Binds: денежные суммы\n- Prevents: потеря копеек\n- Rule: суммы в minor units\n",
    )
    .expect("spine");
    let contracts = case.join("contracts");
    std::fs::create_dir_all(&contracts).expect("mkdir contracts");
    std::fs::write(
        contracts.join("api.yaml"),
        "openapi: 3.0.3\ninfo:\n  title: Processing API\n  version: 1.0.0\npaths:\n  /v1/charges:\n    get:\n      operationId: listCharges\n      responses:\n        '200':\n          description: ok\n",
    )
    .expect("контракт");
    std::fs::create_dir_all(case.join(".arch-handoff")).expect("mkdir handoff");
    std::fs::write(case.join(".arch-handoff/CONSTRAINTS.yaml"), REGISTRY)
        .expect("handoff constraints");
    git(&case, &["init", "-q"]);
    git(&case, &["add", "."]);
    git(&case, &["commit", "-q", "-m", "init"]);

    let responses = mcp_serve(
        home.path(),
        &batch(&[
            call(1, "architect_review", &json!({"path": case})),
            call(2, "change_impact", &json!({"path": case, "id": "CMP-001"})),
            call(
                3,
                "change_impact",
                &json!({"path": case, "paths": ["services/pay/src/main.rs", "docs/x.md"]}),
            ),
            call(4, "change_impact", &json!({"path": case, "id": "CMP-999"})),
        ]),
    );
    assert_eq!(responses.len(), 4, "все вызовы отвечены: {responses:?}");

    let output = |idx: u64| -> Value {
        let verdict = structured(&responses[(idx - 1) as usize], idx);
        serde_json::from_str(verdict["output"].as_str().expect("output"))
            .expect("output — JSON-вердикт")
    };

    // architect_review: единый вердикт, все секции прогнались и зелёные.
    let review = output(1);
    assert_eq!(review["tool"], "architect_review");
    assert_eq!(review["passed"], true, "{review}");
    assert_eq!(review["route"], "Fast", "{review}");
    let names: Vec<&str> = review["components"]
        .as_array()
        .expect("components")
        .iter()
        .filter_map(|c| c["name"].as_str())
        .collect();
    for want in [
        "fitness",
        "spine_lint",
        "trace_check",
        "model_validate",
        "contracts",
    ] {
        assert!(names.contains(&want), "нет секции {want}: {names:?}");
    }

    // change_impact по id: вся цепочка + правило с владельцем + контракт.
    let impact = output(2);
    assert_eq!(impact["tool"], "change_impact");
    assert_eq!(impact["seeds"], json!(["CMP-001"]), "{impact}");
    let ids: Vec<&str> = impact["affected"]
        .as_array()
        .expect("affected")
        .iter()
        .filter_map(|a| a["id"].as_str())
        .collect();
    for want in ["CMP-001", "INT-001", "AD-1", "OWNER-1"] {
        assert!(ids.contains(&want), "нет {want} в {ids:?}");
    }
    assert_eq!(
        impact["contracts"],
        json!(["contracts/api.yaml"]),
        "{impact}"
    );
    assert_eq!(impact["rules"][0]["id"], "C-001", "{impact}");
    assert_eq!(
        impact["rules"][0]["owner"],
        json!("Команда платежей"),
        "{impact}"
    );
    assert!(
        impact["owners"].as_array().expect("owners")[0]
            .as_str()
            .expect("owner")
            .contains("Команда процессинга"),
        "{impact}"
    );

    // change_impact по paths: code_roots → CMP-001; непокрытый путь — gap.
    let impact = output(3);
    assert_eq!(impact["seeds"], json!(["CMP-001"]), "{impact}");
    assert_eq!(impact["gaps"], json!(["docs/x.md"]), "{impact}");

    // Неизвестный id — доменный сбой (isError), не protocol error.
    assert_eq!(responses[3]["result"]["isError"], true);
    assert!(responses[3].get("error").is_none());
    assert!(
        responses[3]["result"]["content"][0]["text"]
            .as_str()
            .expect("text")
            .contains("не найдена"),
        "{}",
        responses[3]
    );
}

/// П.10 (волна 2): `handoff_create` отдаётся мостом `--rw` и в core-сборке —
/// главный доказательный кейс drift-control воспроизводится core-бинарём в
/// части создания пакета. Фикстура — спайн кейса 006
/// (`кейсы/drift-control/handoff-example/`): пакет собирается с git-предгейтом
/// (init + baseline-якорь) в tempdir-репо.
#[test]
fn handoff_create_over_stdio_creates_packet_on_drift_control_fixture() {
    let home = tempfile::tempdir().expect("tmp");
    let repo = home.path().join("repo");
    std::fs::create_dir_all(&repo).expect("mkdir repo");
    let spine = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("кейсы/drift-control/handoff-example/ARCHITECTURE-SPINE.md");
    assert!(spine.is_file(), "фикстура кейса drift-control: {spine:?}");

    let list = json!({"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}).to_string();
    let create = call(
        2,
        "handoff_create",
        &json!({
            "repo": repo,
            "task": "Реализуй платёжное ядро: деньги целыми minor units, thiserror, идемпотентный authorize",
            "spec": [spine],
            "route": "fast",
        }),
    );
    let responses = mcp_serve_with_args(home.path(), &["--rw"], &batch(&[list, create]));

    // Инструмент в выдаче (в core-сборке тоже — это и есть пункт приёмки).
    let tools = responses[0]["result"]["tools"].as_array().expect("tools");
    assert!(
        tools.iter().any(|t| t["name"] == "handoff_create"),
        "handoff_create должен отдаваться мостом --rw (сборка: {})",
        if cfg!(feature = "harness") {
            "harness"
        } else {
            "core"
        }
    );

    let out = structured(&responses[1], 2);
    assert_eq!(out["tool"], "handoff_create");
    let text = out["output"].as_str().expect("output");
    assert!(text.contains("Handoff-пакет создан"), "{text}");

    // Пакет на месте: задача со спайном кейса, манифест с baseline-якорем.
    let dir = repo.join(".arch-handoff");
    for file in [
        "TASK.md",
        "ARCHITECTURE.md",
        "MANIFEST.json",
        "CONSTRAINTS.yaml",
        "SPEC.md",
        "ROLLBACK.yaml",
    ] {
        assert!(dir.join(file).is_file(), "нет файла пакета {file}");
    }
    let task_md = std::fs::read_to_string(dir.join("TASK.md")).expect("TASK.md");
    assert!(task_md.contains("платёжное ядро"), "{task_md}");
    assert!(task_md.contains("## План отката"), "{task_md}");
    let arch_md = std::fs::read_to_string(dir.join("ARCHITECTURE.md")).expect("ARCHITECTURE.md");
    assert!(
        arch_md.contains("AD-1"),
        "спайн drift-control доехал в epic-context:\n{arch_md}"
    );
    let manifest: Value = serde_json::from_str(
        &std::fs::read_to_string(dir.join("MANIFEST.json")).expect("MANIFEST.json"),
    )
    .expect("manifest json");
    assert!(
        manifest["baseline_commit"]
            .as_str()
            .is_some_and(|h| !h.is_empty()),
        "git-предгейт: baseline_commit проставлен (git init + якорь): {manifest}"
    );
    // Якорь реально существует в репозитории.
    let log = std::process::Command::new("git")
        .arg("-C")
        .arg(&repo)
        .args(["log", "--oneline"])
        .output()
        .expect("git log");
    let log_text = String::from_utf8_lossy(&log.stdout);
    assert!(
        log_text.contains("baseline"),
        "baseline-коммит в истории: {log_text}"
    );
}

/// Н8 волны C 0.3.4: у каждого инструмента, принимающего путь, каноничное имя
/// аргумента — `path`. Проверяются ОБА направления: схема в `tools/list`
/// объявляет `path` (а не исторические `dir`/`repo`/`case`/`change_dir`), и
/// вызов с `path` действительно доходит до инструмента — ошибка разбора
/// аргументов означала бы, что канонизации нет.
#[test]
fn every_path_argument_is_canonical_path() {
    let tmp = tempfile::tempdir().expect("tmp");
    let home = tmp.path();
    let project = home.join("project");
    std::fs::create_dir_all(project.join("model")).expect("mkdir model");
    std::fs::write(project.join("ARCHITECTURE-SPINE.md"), "# Spine\n").expect("spine");
    std::fs::write(
        project.join("model/CMP-001-core.md"),
        "---\nid: CMP-001\ntype: cmp\ntitle: \"Core\"\nstatus: \"designed\"\n---\n\nТело.\n",
    )
    .expect("entity");

    let listed = mcp_serve(
        home,
        &format!(
            "{}\n",
            json!({
                "jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {}
            })
        ),
    );
    let tools = listed[0]["result"]["tools"]
        .as_array()
        .expect("tools")
        .clone();
    assert!(tools.len() > 20, "ожидался полный список инструментов");

    // Исторические имена пути не должны рекламироваться ни у одного инструмента.
    for tool in &tools {
        let name = tool["name"].as_str().expect("имя");
        let props = tool["inputSchema"]["properties"].as_object();
        let Some(props) = props else { continue };
        for legacy in ["dir", "repo", "case", "change_dir"] {
            assert!(
                !props.contains_key(legacy),
                "{name}: схема обязана объявлять `path`, а не `{legacy}`: {props:?}"
            );
        }
    }

    // Инструменты, принимающие путь: вызов с `path` не должен падать на
    // разборе аргументов (доменная ошибка «нет файла» — это уже успех, значит
    // аргумент принят и дошёл до инструмента).
    let with_path: Vec<String> = tools
        .iter()
        .filter(|t| {
            t["inputSchema"]["properties"]
                .as_object()
                .is_some_and(|p| p.contains_key("path"))
        })
        .map(|t| t["name"].as_str().expect("имя").to_string())
        .collect();
    assert!(
        with_path.len() >= 20,
        "канонизация обязана покрыть всю поверхность с путём: {with_path:?}"
    );

    let mut requests = String::new();
    for (i, name) in with_path.iter().enumerate() {
        let mut args = json!({"path": project.display().to_string()});
        if name == "contract_diff" {
            args = json!({"path": project.display().to_string()});
        }
        requests.push_str(&call(10 + i as u64, name, &args));
        requests.push('\n');
    }
    let responses = mcp_serve(home, &requests);
    for (i, name) in with_path.iter().enumerate() {
        let resp = &responses[i];
        let text = resp.to_string();
        assert!(
            !text.contains("невалидные аргументы"),
            "{name}: вызов с `path` не должен падать на разборе: {text}"
        );
        assert!(
            !text.contains("missing field"),
            "{name}: `path` обязан приниматься: {text}"
        );
    }
}

/// Н8: старые имена аргументов остаются синонимами, а неизвестный аргумент
/// отвергается с перечнем допустимых — опечатка не выглядит «инструмент не
/// сработал».
#[test]
fn legacy_path_argument_names_still_work() {
    let tmp = tempfile::tempdir().expect("tmp");
    let home = tmp.path();
    let project = home.join("project");
    std::fs::create_dir_all(project.join("model")).expect("mkdir model");
    std::fs::write(project.join("ARCHITECTURE-SPINE.md"), "# Spine\n").expect("spine");
    std::fs::write(
        project.join("model/CMP-001-core.md"),
        "---\nid: CMP-001\ntype: cmp\ntitle: \"Core\"\nstatus: \"designed\"\n---\n\nТело.\n",
    )
    .expect("entity");

    // Историческое имя `dir` доходит до инструмента (не «missing field»).
    let resp = mcp_serve(
        home,
        &format!(
            "{}\n",
            call(
                1,
                "model_query",
                &json!({"dir": project.display().to_string()})
            )
        ),
    );
    let text = resp[0].to_string();
    assert!(
        !text.contains("missing field") && !text.contains("невалидные аргументы"),
        "старое имя `dir` обязано приниматься: {text}"
    );

    // Незнакомый аргумент — ошибка с перечнем допустимых.
    let resp = mcp_serve(
        home,
        &format!(
            "{}\n",
            call(2, "spine_lint", &json!({"directory": "/tmp", "path": "x"}))
        ),
    );
    let text = resp[0].to_string();
    assert!(text.contains("неизвестный аргумент"), "{text}");
    assert!(text.contains("'directory'"), "{text}");
    assert!(text.contains("допустимые: path"), "{text}");
}

/// W1: `verdict_explain` через MCP отдаёт три блока паспорта и вердикт того же
/// прогона. Проверяется сквозь процесс: паспорт, который в MCP отличается от
/// CLI, — это два разных ответа на один вопрос.
#[test]
fn verdict_explain_returns_the_three_blocks() {
    let tmp = tempfile::tempdir().expect("tmp");
    let home = tmp.path();
    let project = home.join("project");
    std::fs::create_dir_all(project.join("model")).expect("mkdir model");
    std::fs::write(project.join("ARCHITECTURE-SPINE.md"), "# Spine\n").expect("spine");
    std::fs::write(
        project.join("model/CMP-001-core.md"),
        "---\nid: CMP-001\ntype: cmp\ntitle: \"Core\"\nstatus: \"designed\"\n---\n\nТело.\n",
    )
    .expect("entity");

    let resp = mcp_serve(
        home,
        &format!(
            "{}\n",
            call(
                1,
                "verdict_explain",
                &json!({"path": project.display().to_string(), "route": "fast"})
            )
        ),
    );
    let passport = structured(&resp[0], 1);
    assert_eq!(passport["schema"], "arch-be/verdict-passport/v1");
    assert_eq!(passport["route"], "Fast");
    assert!(
        passport["checked"]
            .as_array()
            .is_some_and(|a| !a.is_empty()),
        "блок 1 обязан нести составляющие: {passport}"
    );
    assert_eq!(
        passport["verdict"], passport["verdict_envelope"]["verdict"],
        "паспорт и вердикт — один прогон, а не два: {passport}"
    );
    assert_eq!(
        passport["attestation"], passport["verdict_envelope"]["attestation"],
        "аттестация паспорта и вердикта совпадает"
    );
    let markdown = passport["report_markdown"].as_str().expect("markdown");
    assert!(markdown.contains("## 1. Проверено"), "{markdown}");
    assert!(markdown.contains("## 2. Заявлено"), "{markdown}");
    assert!(markdown.contains("## 3. Не проверено"), "{markdown}");
    // Блок 2 называет границу ссылок модели — репозиторий её содержит.
    assert!(
        passport["claimed_not_verified"]
            .as_array()
            .is_some_and(|a| a.iter().any(|c| c["text"]
                .as_str()
                .is_some_and(|t| t.contains("СУЩЕСТВУЮЩУЮ")))),
        "блок 2 обязан назвать семантику ссылок: {passport}"
    );
}

/// W4: `trust_report` через MCP отдаёт шкалу с якорями, у каждого — чем
/// подтверждён. Метрика, доступная только из CLI, не помогает агенту,
/// который и есть источник журнала вызовов.
#[test]
fn trust_report_returns_anchors_with_evidence() {
    let tmp = tempfile::tempdir().expect("tmp");
    let home = tmp.path();
    let project = home.join("project");
    std::fs::create_dir_all(&project).expect("mkdir");
    std::fs::write(project.join("ARCHITECTURE-SPINE.md"), "# Spine\n").expect("spine");

    let resp = mcp_serve(
        home,
        &format!(
            "{}\n",
            call(
                1,
                "trust_report",
                &json!({"path": project.display().to_string()})
            )
        ),
    );
    let trust = structured(&resp[0], 1);
    assert_eq!(trust["schema"], "arch-be/trust/v1");
    let anchors = trust["anchors"].as_array().expect("якоря");
    assert_eq!(anchors.len(), 5, "шкала 1–5: {trust}");
    for a in anchors {
        assert!(
            a["evidence"].as_str().is_some_and(|e| !e.is_empty()),
            "у якоря нет доказательства: {a}"
        );
        assert_eq!(a["met"], a["why_not"].is_null(), "якорь несогласован: {a}");
    }
    assert!(
        trust["report_markdown"]
            .as_str()
            .is_some_and(|m| m.contains("Доверие к контуру")),
        "{trust}"
    );
}

/// Н13: поле, которое реализация разбирает, обязано быть в объявленной схеме.
/// `rubric_verify` принимал `author_model`, но не рекламировал его — клиент,
/// читающий `tools/list`, не мог пометить «судья судил свою работу», и
/// защитный отсев незнакомых аргументов (Н8) честно отвергал вызов.
#[test]
fn every_accepted_rubric_argument_is_advertised() {
    let tmp = tempfile::tempdir().expect("tmp");
    let home = tmp.path();
    let target = home.join("ADR-001.md");
    std::fs::write(&target, "# ADR-001\n\n## Alternatives\n\nБ.\n").expect("adr");

    // Схема обязана объявлять все поля, которые разбирает реализация.
    let listed = mcp_serve(
        home,
        &format!(
            "{}\n",
            json!({"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}})
        ),
    );
    let spec = listed[0]["result"]["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .find(|t| t["name"] == "rubric_verify")
        .cloned()
        .expect("rubric_verify");
    for arg in [
        "rubric",
        "target",
        "target_text",
        "answers",
        "model",
        "judge_model",
        "author_model",
    ] {
        assert!(
            spec["inputSchema"]["properties"].get(arg).is_some(),
            "аргумент '{arg}' разбирается, но не объявлен: {spec}"
        );
    }

    // И вызов с ним доходит до инструмента, а не падает на разборе.
    let judge = json!({
        "scores": [{"criterion_id": "context", "score": 3, "rationale": "Кратко."}],
        "verdict": "accept"
    })
    .to_string();
    let resp = mcp_serve(
        home,
        &format!(
            "{}\n",
            call(
                2,
                "rubric_verify",
                &json!({
                    "rubric": "adr_quality",
                    "target": target.display().to_string(),
                    "answers": [judge],
                    "judge_model": "judge-x",
                    "author_model": "judge-x",
                })
            )
        ),
    );
    let text = resp[0].to_string();
    assert!(
        !text.contains("неизвестный аргумент"),
        "author_model обязан приниматься: {text}"
    );
}
/// Рубрика-фикстура для тестов происхождения: один критерий, вес 1.
fn provenance_rubric(home: &Path) -> String {
    let path = home.join("provenance-rubric.yaml");
    std::fs::write(
        &path,
        "name: t-rubric\ndescription: тестовая\nscale_max: 5\norigin: anchor\ncriteria:\n  - id: context\n    name: Контекст\n    description: Описан контекст\n    weight: 1.0\n",
    )
    .expect("рубрика");
    path.display().to_string()
}

/// ADR-001-фикстура внутри кейса с `.arch-handoff/`: по ней работает
/// `repo_root_of`, а значит и запись отчёта, который читает гейт.
fn provenance_case(home: &Path) -> (std::path::PathBuf, std::path::PathBuf) {
    let repo = home.join("case");
    std::fs::create_dir_all(repo.join(".arch-handoff")).expect("mkdir handoff");
    std::fs::write(
        repo.join(".arch-handoff/CONSTRAINTS.yaml"),
        "constraints: []\n",
    )
    .expect("constraints");
    let adr = repo.join("docs/adr/ADR-001-pilot.md");
    std::fs::create_dir_all(adr.parent().expect("каталог ADR")).expect("mkdir adr");
    std::fs::write(
        &adr,
        "# ADR-001. Пилот\n\n- Статус: Accepted\n- Дата: 2026-09-20\n\n## Context\n\nКонтекст описан явно и коротко.\n",
    )
    .expect("ADR");
    (repo, adr)
}

/// Ответ судьи-фикстуры: один разобранный ответ на критерий `context`.
fn provenance_answer() -> String {
    "{\"scores\":[{\"criterion_id\":\"context\",\"score\":4,\"rationale\":\"Цитата: \\\"Контекст описан явно\\\" — да\"}],\"verdict\":\"годно\"}".to_string()
}

/// AP-1 (ADR-048): `initialize` запоминает `clientInfo` и выдаёт идентификатор
/// сессии; отчёт, собранный хостом, несёт ЗАЯВЛЕННОЕ происхождение — режим
/// `declared`, хост с версией, идентификатор сессии, хэш выданного промпта и
/// счётчик вызовов до судейства. Ни одно поле не утверждает «независимость
/// подтверждена»: какая модель отвечала, механика не знает.
#[test]
fn initialize_stores_client_info_and_session() {
    let home = tempfile::tempdir().expect("tmp");
    let (repo, adr) = provenance_case(home.path());
    let rubric = provenance_rubric(home.path());
    let init = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-06-18",
            "clientInfo": {"name": "claude-code", "version": "2.1.278"},
        },
    })
    .to_string();
    let prompt = call(
        2,
        "rubric_prompt",
        &json!({"rubric": rubric, "target": adr.display().to_string()}),
    );
    let verify = call(
        3,
        "rubric_verify",
        &json!({
            "rubric": rubric,
            "target": adr.display().to_string(),
            "judge_model": "glm-5.2",
            "answers": [provenance_answer()],
        }),
    );
    let responses = mcp_serve_with_args(home.path(), &["--rw"], &batch(&[init, prompt, verify]));
    let session_id = responses[0]["result"]["sessionId"]
        .as_str()
        .expect("идентификатор сессии в ответе initialize")
        .to_string();
    assert!(!session_id.is_empty(), "сессия без идентификатора");
    let prompt_out = structured(&responses[1], 2);
    assert!(prompt_out["prompt_sha256"].is_string(), "{prompt_out}");
    let verdict = structured(&responses[2], 3);
    let prov = &verdict["provenance"];
    assert_eq!(prov["mode"], "declared", "{prov}");
    assert_eq!(prov["host"]["name"], "claude-code", "{prov}");
    assert_eq!(prov["host"]["version"], "2.1.278", "{prov}");
    assert_eq!(prov["session_id"], json!(session_id), "{prov}");
    assert_eq!(prov["prompt_issued_in_session"], true, "{prov}");
    // До `rubric_prompt` в сессии не было ни одного вызова инструмента.
    assert_eq!(prov["session_calls_before"], 0, "{prov}");
    assert_eq!(prompt_out["session_id"], json!(session_id), "{prompt_out}");
    assert_eq!(prompt_out["prompt_sha256"], prov["prompt_sha256"], "{prov}");
    // Тот же блок — в файле отчёта, который читает гейт.
    let artifact: Value = serde_json::from_str(
        &std::fs::read_to_string(repo.join("reports/rubric/ADR-001-pilot.json")).expect("отчёт"),
    )
    .expect("JSON отчёта");
    assert_eq!(artifact["provenance"]["mode"], "declared", "{artifact}");
    assert_eq!(
        artifact["provenance"]["host"]["name"], "claude-code",
        "{artifact}"
    );
    assert_eq!(artifact["provenance"]["session_id"], json!(session_id));
}

/// AP-2 (ADR-048): `rubric_verify` без предшествующего `rubric_prompt` в этой
/// сессии — не ошибка: судить могли в другой (в том числе чистой) сессии.
/// Отчёт получается, происхождение честно говорит `prompt_issued_in_session:
/// false` и не выдаёт чужой промпт за свой.
#[test]
fn verify_without_prompt_in_session_is_recorded_not_rejected() {
    let home = tempfile::tempdir().expect("tmp");
    let (_repo, adr) = provenance_case(home.path());
    let rubric = provenance_rubric(home.path());
    let init = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {"clientInfo": {"name": "qwen-code"}},
    })
    .to_string();
    let verify = call(
        2,
        "rubric_verify",
        &json!({
            "rubric": rubric,
            "target": adr.display().to_string(),
            "judge_model": "deepseek-v4-pro",
            "answers": [provenance_answer()],
        }),
    );
    let responses = mcp_serve_with_args(home.path(), &["--rw"], &batch(&[init, verify]));
    let verdict = structured(&responses[1], 2);
    assert_eq!(verdict["answers"]["valid"], 1, "{verdict}");
    let prov = &verdict["provenance"];
    assert_eq!(prov["mode"], "declared", "{prov}");
    assert_eq!(prov["prompt_issued_in_session"], false, "{prov}");
    assert_eq!(prov["host"]["name"], "qwen-code", "{prov}");
    // Версии хост не назвал — поле отсутствует, а не выдумано.
    assert!(prov["host"].get("version").is_none(), "{prov}");
    assert!(prov.get("prompt_sha256").is_none(), "{prov}");
}

/// Кейс, пригодный для гейта: принимаемый ADR, рубрика в каталоге рубрик
/// харнесса и конфиг, включающий составляющую `decision_quality`.
///
/// Отчёт для него собирается через MCP `rubric_verify` — детерминированно и
/// без LLM, поэтому «отчёт сходится со своими ответами» проверяется
/// по-настоящему, а не моком.
fn judge_gate_case(home: &Path) -> (std::path::PathBuf, std::path::PathBuf, String) {
    let (repo, adr) = provenance_case(home);
    // Реестр правил кейса обязан быть непустым: пустой корень — это FAIL
    // составляющей `fitness`, и гейт краснел бы не из-за того, что проверяем.
    std::fs::write(
        repo.join(".arch-handoff/CONSTRAINTS.yaml"),
        "rules:\n  - name: spine_present\n    type: file_exists\n    path: \"ARCHITECTURE-SPINE.md\"\n    severity: error\n",
    )
    .expect("constraints");
    std::fs::write(repo.join("ARCHITECTURE-SPINE.md"), "# Spine\n").expect("spine");
    let assets = home.join("assets-test");
    let rubrics = assets.join("rubrics");
    std::fs::create_dir_all(&rubrics).expect("mkdir rubrics");
    let rubric_path = rubrics.join("t-rubric.yaml");
    std::fs::write(
        &rubric_path,
        "name: t-rubric\ndescription: тестовая\nscale_max: 5\norigin: anchor\ncriteria:\n  - id: context\n    name: Контекст\n    description: Описан контекст\n    weight: 1.0\n",
    )
    .expect("рубрика");
    // Рубрика задаётся и путём (для verify), и каталогом (для гейта).
    std::fs::write(
        home.join("arch-harness.toml"),
        format!(
            "[paths]\nassets_dir = \"{}\"\n\n[gate.required]\nfast = [\"decision_quality\"]\n\
             standard = [\"decision_quality\"]\ncritical = [\"decision_quality\"]\n",
            assets.display()
        ),
    )
    .expect("конфиг");
    vcs_init(&repo);
    (repo, adr, rubric_path.display().to_string())
}

/// Инициализирует git-репозиторий кейса (правила гейта читают git-базу).
fn vcs_init(repo: &Path) {
    for args in [
        vec!["init", "-q"],
        vec!["add", "."],
        vec!["commit", "-q", "-m", "init"],
    ] {
        let status = std::process::Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(&args)
            .env("GIT_AUTHOR_NAME", "judge-test")
            .env("GIT_AUTHOR_EMAIL", "judge-test@example.invalid")
            .env("GIT_COMMITTER_NAME", "judge-test")
            .env("GIT_COMMITTER_EMAIL", "judge-test@example.invalid")
            .status()
            .expect("git доступен");
        assert!(status.success(), "git {args:?}");
    }
}

/// Собирает отчёт через split-judge: один валидный ответ → verify под `--rw`
/// (отчёт и сырые ответы ложатся в репозиторий кейса).
fn judge_report_via_mcp(home: &Path, adr: &Path, rubric: &str, judge_model: &str) {
    let verify = call(
        1,
        "rubric_verify",
        &json!({
            "rubric": rubric,
            "target": adr.display().to_string(),
            "judge_model": judge_model,
            "author_model": "claude-opus-4",
            "answers": [provenance_answer()],
        }),
    );
    let responses = mcp_serve_with_args(home, &["--rw"], &batch(&[verify]));
    let verdict = structured(&responses[0], 1);
    assert_eq!(verdict["answers"]["valid"], 1, "{verdict}");
}

/// Гейт кейса: `arch-be gate --repo <кейс> --route fast` (составляющая
/// `decision_quality` включена конфигом).
fn gate_output(home: &Path, repo: &Path) -> std::process::Output {
    arch_cmd(home)
        .arg("gate")
        .arg("--repo")
        .arg(repo.as_os_str())
        .arg("--route")
        .arg("fast")
        .output()
        .expect("запуск arch-be gate")
}

/// J2 (ADR-048): сырые ответы судьи сохраняются рядом с отчётом, их хэши
/// попадают в `provenance.samples`, а баллы по критериям — в сам отчёт:
/// отсюда берётся воспроизводимость, которой раньше не было.
#[test]
fn raw_answers_are_saved_with_hashes() {
    let home = tempfile::tempdir().expect("tmp");
    let (repo, adr, rubric) = judge_gate_case(home.path());
    judge_report_via_mcp(home.path(), &adr, &rubric, "glm-5.2");
    let raw = repo.join("reports/rubric/raw/ADR-001-pilot/sample-1.json");
    let record: Value =
        serde_json::from_str(&std::fs::read_to_string(&raw).expect("сырой ответ")).expect("JSON");
    assert_eq!(record["schema"], "arch-be/rubric-raw/v1", "{record}");
    assert_eq!(record["sample"], 1, "{record}");
    assert_eq!(record["judge_model"], "glm-5.2", "{record}");
    assert_eq!(record["dropped"], false, "{record}");
    assert_eq!(
        record["text"],
        provenance_answer(),
        "текст ответа сохраняется как есть"
    );
    // Хэш в файле и хэш в отчёте — один и тот же, и он же от текста ответа.
    let sha = arch_harness::hash::sha256_hex(record["text"].as_str().expect("текст").as_bytes());
    assert_eq!(record["sha256"], json!(sha), "{record}");
    let artifact: Value = serde_json::from_str(
        &std::fs::read_to_string(repo.join("reports/rubric/ADR-001-pilot.json")).expect("отчёт"),
    )
    .expect("JSON отчёта");
    assert_eq!(artifact["provenance"]["samples"][0]["sha256"], json!(sha));
    assert_eq!(artifact["provenance"]["samples"][0]["dropped"], false);
    assert_eq!(
        artifact["scores"][0]["criterion_id"], "context",
        "{artifact}"
    );
    assert_eq!(artifact["scores"][0]["score"], 4, "{artifact}");
    // Отчёт без правок гейт проходит: сверка прошла, расхождений нет.
    let gate = gate_output(home.path(), &repo);
    let stdout = String::from_utf8_lossy(&gate.stdout);
    assert!(gate.status.success(), "гейт: {stdout}");
    assert!(
        !stdout.contains("rubric_report_inconsistent"),
        "воспроизводимый отчёт не должен краснеть: {stdout}"
    );
}

/// J2 (ADR-048), критерий успеха 2: балл, поправленный в отчёте руками, ловит
/// составляющая `decision_quality` — отчёт пересобирается из сырых ответов, а
/// расхождение становится находкой `rubric_report_inconsistent` (error).
#[test]
fn gate_flags_hand_edited_score() {
    let home = tempfile::tempdir().expect("tmp");
    let (repo, adr, rubric) = judge_gate_case(home.path());
    judge_report_via_mcp(home.path(), &adr, &rubric, "glm-5.2");
    let report = repo.join("reports/rubric/ADR-001-pilot.json");
    let mut artifact: Value =
        serde_json::from_str(&std::fs::read_to_string(&report).expect("отчёт")).expect("JSON");
    // Поднимаем балл руками: ровно то, что задание называет непроверяемым.
    artifact["scores"][0]["score"] = json!(5);
    artifact["weighted_total"] = json!(5.0);
    std::fs::write(
        &report,
        serde_json::to_string_pretty(&artifact).expect("JSON"),
    )
    .expect("запись отчёта");
    let gate = gate_output(home.path(), &repo);
    let stdout = String::from_utf8_lossy(&gate.stdout);
    assert!(!gate.status.success(), "правка обязана краснеть: {stdout}");
    assert!(
        stdout.contains("rubric_report_inconsistent"),
        "ожидалась находка о несоответствии ответам: {stdout}"
    );
    assert!(
        stdout.contains("критерий 'context'"),
        "находка называет критерий и числа: {stdout}"
    );
}

/// J2 (ADR-048): правка самого сохранённого ответа тоже видна — хэш текста в
/// файле не сходится с записанным в отчёте (`rubric_raw_tampered`, error).
#[test]
fn gate_flags_tampered_raw_answer() {
    let home = tempfile::tempdir().expect("tmp");
    let (repo, adr, rubric) = judge_gate_case(home.path());
    judge_report_via_mcp(home.path(), &adr, &rubric, "glm-5.2");
    let raw = repo.join("reports/rubric/raw/ADR-001-pilot/sample-1.json");
    let mut record: Value =
        serde_json::from_str(&std::fs::read_to_string(&raw).expect("сырой ответ")).expect("JSON");
    record["text"] = json!(
        "{\"scores\":[{\"criterion_id\":\"context\",\"score\":5,\"rationale\":\"Цитата: \\\"Контекст описан явно\\\"\"}],\"verdict\":\"годно\"}"
    );
    std::fs::write(&raw, serde_json::to_string_pretty(&record).expect("JSON"))
        .expect("запись сырого ответа");
    let gate = gate_output(home.path(), &repo);
    let stdout = String::from_utf8_lossy(&gate.stdout);
    assert!(
        !gate.status.success(),
        "подмена ответа обязана краснеть: {stdout}"
    );
    assert!(
        stdout.contains("rubric_raw_tampered"),
        "ожидалась находка о подмене сырого ответа: {stdout}"
    );
}

/// J2 (ADR-048), обратная совместимость: отчёт старого формата (без
/// `provenance`, `scores` и сырых ответов) читается и даёт ПРЕЖНИЕ находки —
/// отсутствие свидетельства не становится находкой, но и не выдаётся за
/// проверку: паспорт называет границу.
#[test]
fn legacy_report_without_provenance_loads_and_passes_as_before() {
    let home = tempfile::tempdir().expect("tmp");
    let (repo, adr, _rubric) = judge_gate_case(home.path());
    let sha = arch_harness::hash::sha256_file(&adr).expect("хэш документа");
    let reports = repo.join("reports/rubric");
    std::fs::create_dir_all(&reports).expect("mkdir reports");
    std::fs::write(
        reports.join("ADR-001-pilot.json"),
        serde_json::to_string_pretty(&json!({
            "schema": "arch-be/rubric-report/v1",
            "rubric": "t-rubric",
            "target": "docs/adr/ADR-001-pilot.md",
            "target_sha256": sha,
            "judge_model": "glm-5.2",
            "author_model": "claude-opus-4",
            "weighted_total": 4.0,
            "verdict": "годно",
            "unstable": false,
            "evidence_not_found": 0,
            "judged_at": "2026-09-20T10:00:00+03:00",
        }))
        .expect("JSON"),
    )
    .expect("запись отчёта");
    let gate = gate_output(home.path(), &repo);
    let stdout = String::from_utf8_lossy(&gate.stdout);
    assert!(
        gate.status.success(),
        "старый отчёт не должен краснеть: {stdout}"
    );
    // Граница проверки называется в паспорте вердикта (блок 2: что зелёный НЕ
    // означает), а не в обычном выводе гейта.
    let explain = arch_cmd(home.path())
        .arg("gate")
        .arg("--repo")
        .arg(repo.as_os_str())
        .arg("--route")
        .arg("fast")
        .arg("--explain")
        .output()
        .expect("запуск arch-be gate --explain");
    let text = String::from_utf8_lossy(&explain.stdout);
    assert!(
        text.contains("невоспроизводима"),
        "граница проверки называется, а не выдаётся за проверку: {text}"
    );
}

/// `arch-be rubric reverify`: отчёт, собранный из своих ответов, проходит;
/// отчёт с поднятым рукой баллом — нет (ненулевой код); отчёт без сырых
/// ответов честно называется невоспроизводимым, но расхождением не считается.
#[test]
fn reverify_reproduces_report_and_flags_edits() {
    let home = tempfile::tempdir().expect("tmp");
    let (repo, adr, rubric) = judge_gate_case(home.path());
    judge_report_via_mcp(home.path(), &adr, &rubric, "glm-5.2");
    let report = repo.join("reports/rubric/ADR-001-pilot.json");
    let reverify = |path: &Path| {
        arch_cmd(home.path())
            .arg("rubric")
            .arg("reverify")
            .arg(path.as_os_str())
            .output()
            .expect("запуск rubric reverify")
    };
    let ok = reverify(&report);
    let stdout = String::from_utf8_lossy(&ok.stdout);
    assert!(ok.status.success(), "свой отчёт должен сходиться: {stdout}");
    assert!(stdout.contains("воспроизводится"), "{stdout}");
    // Правка балла в отчёте: сверка называет расхождение и выходит с кодом 1.
    let mut artifact: Value =
        serde_json::from_str(&std::fs::read_to_string(&report).expect("отчёт")).expect("JSON");
    artifact["scores"][0]["score"] = json!(5);
    std::fs::write(
        &report,
        serde_json::to_string_pretty(&artifact).expect("JSON"),
    )
    .expect("запись отчёта");
    let bad = reverify(&report);
    let stdout = String::from_utf8_lossy(&bad.stdout);
    assert!(
        !bad.status.success(),
        "правка обязана дать ненулевой код: {stdout}"
    );
    assert!(stdout.contains("критерий 'context'"), "{stdout}");
    // Отчёт без сырых ответов: невоспроизводим, но это не расхождение.
    std::fs::remove_dir_all(repo.join("reports/rubric/raw")).expect("удаление сырых ответов");
    let legacy = reverify(&report);
    let stdout = String::from_utf8_lossy(&legacy.stdout);
    assert!(legacy.status.success(), "{stdout}");
    assert!(stdout.contains("невоспроизводим"), "{stdout}");
}

/// J3 (ADR-048): модель-автор берётся из ШАПКИ документа — её значение
/// закоммичено вместе с документом, поэтому автора нельзя «вспомнить» после
/// судейства. Аргумент вызова при этом не отбрасывается: расхождение
/// называется в отчёте (`author_model_declared`) и находкой гейта.
#[test]
fn author_model_is_read_from_adr_header() {
    let home = tempfile::tempdir().expect("tmp");
    let (repo, adr) = provenance_case(home.path());
    // Шапка несёт автора-человека: любая судья-модель от него отлична.
    let text = std::fs::read_to_string(&adr).expect("ADR");
    let text = text.replace(
        "- Дата: 2026-09-20",
        "- Дата: 2026-09-20\n- Модель-автор: human:Архитектор",
    );
    std::fs::write(&adr, text).expect("ADR с автором");
    let rubric = provenance_rubric(home.path());
    let verify = call(
        2,
        "rubric_verify",
        &json!({
            "rubric": rubric,
            "target": adr.display().to_string(),
            "judge_model": "glm-5.2",
            "answers": [provenance_answer()],
        }),
    );
    let responses = mcp_serve_with_args(home.path(), &["--rw"], &batch(&[verify]));
    let verdict = structured(&responses[0], 2);
    // Судья назвал себя — автор из шапки: метки разные, находки нет.
    assert_eq!(verdict["author_model"], "human:Архитектор", "{verdict}");
    assert_eq!(verdict["author_source"], "header", "{verdict}");
    let artifact: Value = serde_json::from_str(
        &std::fs::read_to_string(repo.join("reports/rubric/ADR-001-pilot.json")).expect("отчёт"),
    )
    .expect("JSON отчёта");
    assert_eq!(artifact["author_model"], "human:Архитектор", "{artifact}");
    assert_eq!(artifact["author_source"], "header", "{artifact}");
    assert!(
        artifact.get("author_model_declared").is_none(),
        "{artifact}"
    );
}

/// J3 (ADR-048): когда шапка и аргумент расходятся, в отчёт идёт значение
/// ИЗ ШАПКИ, а расхождение называется — предупреждением гейта
/// `author_model_mismatch`. Так метка, названная в момент судейства, не
/// подменяет автора, записанного в документе.
#[test]
fn header_wins_over_argument_with_warning() {
    let home = tempfile::tempdir().expect("tmp");
    let (repo, adr) = provenance_case(home.path());
    let text = std::fs::read_to_string(&adr).expect("ADR");
    std::fs::write(
        &adr,
        text.replace(
            "- Дата: 2026-09-20",
            "- Дата: 2026-09-20\n- Author-model: claude-opus-4",
        ),
    )
    .expect("ADR с автором");
    let rubric = provenance_rubric(home.path());
    let verify = call(
        2,
        "rubric_verify",
        &json!({
            "rubric": rubric,
            "target": adr.display().to_string(),
            "judge_model": "glm-5.2",
            "author_model": "deepseek-v4-pro",
            "answers": [provenance_answer()],
        }),
    );
    let responses = mcp_serve_with_args(home.path(), &["--rw"], &batch(&[verify]));
    let verdict = structured(&responses[0], 2);
    assert_eq!(
        verdict["author_model"], "claude-opus-4",
        "в отчёт идёт значение из шапки: {verdict}"
    );
    let artifact: Value = serde_json::from_str(
        &std::fs::read_to_string(repo.join("reports/rubric/ADR-001-pilot.json")).expect("отчёт"),
    )
    .expect("JSON отчёта");
    assert_eq!(
        artifact["author_model_declared"], "deepseek-v4-pro",
        "{artifact}"
    );
    // Гейт называет расхождение, но не краснит: метка из вызова — не подлог.
    let enable_gate = home.path().join("arch-harness.toml");
    std::fs::write(
        &enable_gate,
        "[gate.required]\nfast = [\"decision_quality\"]\n",
    )
    .expect("конфиг гейта");
    let gate = arch_cmd(home.path())
        .arg("gate")
        .arg("--repo")
        .arg(repo.as_os_str())
        .arg("--route")
        .arg("fast")
        .output()
        .expect("запуск arch-be gate");
    let stdout = String::from_utf8_lossy(&gate.stdout);
    assert!(
        stdout.contains("author_model_mismatch"),
        "расхождение обязано быть названо: {stdout}"
    );
}

/// J3 (ADR-048): правка метки автора в шапке меняет документ — отчёт от
/// прежней редакции становится устаревшим. Это и есть защита от «вспомнить
/// автора задним числом»: переписать шапку можно, а перенести на неё старую
/// оценку — нет.
#[test]
fn editing_author_header_makes_report_stale() {
    let home = tempfile::tempdir().expect("tmp");
    let (repo, adr, rubric) = judge_gate_case(home.path());
    let text = std::fs::read_to_string(&adr).expect("ADR");
    std::fs::write(
        &adr,
        text.replace(
            "- Дата: 2026-09-20",
            "- Дата: 2026-09-20\n- Модель-автор: claude-opus-4",
        ),
    )
    .expect("ADR с автором");
    judge_report_via_mcp(home.path(), &adr, &rubric, "glm-5.2");
    let gate = gate_output(home.path(), &repo);
    assert!(
        gate.status.success(),
        "отчёт по этой редакции должен проходить: {}",
        String::from_utf8_lossy(&gate.stdout)
    );
    // Меняем метку автора в шапке — редакция документа другая.
    let text = std::fs::read_to_string(&adr).expect("ADR");
    std::fs::write(&adr, text.replace("claude-opus-4", "deepseek-v4-pro")).expect("ADR правленый");
    let gate = gate_output(home.path(), &repo);
    let stdout = String::from_utf8_lossy(&gate.stdout);
    assert!(!gate.status.success(), "отчёт обязан устареть: {stdout}");
    assert!(
        stdout.contains("rubric_report_stale"),
        "ожидалась находка об устаревшем отчёте: {stdout}"
    );
}

/// J3 (ADR-048): `adr_new` пишет метку автора в шапку — иначе автора пришлось
/// бы называть в каждом вызове судьи, а не фиксировать в документе.
#[test]
fn adr_new_writes_author_model() {
    let home = tempfile::tempdir().expect("tmp");
    let dir = home.path().join("adr");
    std::fs::create_dir_all(&dir).expect("mkdir");
    let responses = mcp_serve_with_args(
        home.path(),
        &["--rw"],
        &batch(&[call(
            1,
            "adr_new",
            &json!({
                "title": "Идемпотентность приёма",
                "path": dir.display().to_string(),
                "author_model": "claude-opus-4",
            }),
        )]),
    );
    let verdict = structured(&responses[0], 1);
    let created = verdict["output"]
        .as_str()
        .expect("вывод adr_new")
        .to_string();
    assert!(created.contains("ADR создан"), "{verdict}");
    // Имя файла берём из ответа инструмента: слаг транслитерируется.
    let file = dir.join(created.rsplit('/').next().expect("имя созданного файла"));
    let text = std::fs::read_to_string(&file).expect("новый ADR");
    assert!(
        text.contains("- Модель-автор: claude-opus-4"),
        "шапка нового ADR: {text}"
    );
    assert_eq!(
        arch_harness::adr_registry::author_model_of(&file).as_deref(),
        Some("claude-opus-4"),
        "записанная метка обязана читаться тем же разбором"
    );
}

/// J4 (ADR-048): судья и автор — разные модели ОДНОГО семейства. «Другая
/// модель» не значит «другой взгляд»: слепые зоны у семейства общие, и находка
/// `judge_same_family` называет это предупреждением.
#[test]
fn same_family_is_flagged() {
    let home = tempfile::tempdir().expect("tmp");
    let (repo, adr, rubric) = judge_gate_case(home.path());
    let text = std::fs::read_to_string(&adr).expect("ADR");
    std::fs::write(
        &adr,
        text.replace(
            "- Дата: 2026-09-20",
            "- Дата: 2026-09-20\n- Модель-автор: claude-opus-4",
        ),
    )
    .expect("ADR с автором");
    // Судья — другая модель того же семейства (anthropic).
    judge_report_via_mcp(home.path(), &adr, &rubric, "claude-3-5-sonnet");
    let gate = gate_output(home.path(), &repo);
    let stdout = String::from_utf8_lossy(&gate.stdout);
    assert!(
        stdout.contains("judge_same_family"),
        "одно семейство обязано быть названо: {stdout}"
    );
    assert!(
        stdout.contains("anthropic"),
        "находка называет семейство: {stdout}"
    );
    // По умолчанию это warn: зелёный вердикт сохраняется.
    assert!(
        gate.status.success(),
        "смена модели внутри семейства — не нарушение по умолчанию: {stdout}"
    );
    // Ключ проекта поднимает находку до error.
    std::fs::write(
        home.path().join("arch-harness.toml"),
        "[gate.required]\nfast = [\"decision_quality\"]\n\n[gate.decision_quality]\nrequire_distinct_family = true\n",
    )
    .expect("конфиг со строгим семейством");
    let strict = gate_output(home.path(), &repo);
    let stdout = String::from_utf8_lossy(&strict.stdout);
    assert!(
        !strict.status.success(),
        "require_distinct_family = true обязан краснить: {stdout}"
    );
}

/// J4 (ADR-048): разные НЕИЗВЕСТНЫЕ метки — разные семейства, а не «оба
/// unknown, значит совпали». Иначе судья `my-llm` считался бы роднёй автора
/// `other-llm` только потому, что механика не знает ни того, ни другого.
#[test]
fn unknown_families_do_not_collide() {
    let home = tempfile::tempdir().expect("tmp");
    let (repo, adr, rubric) = judge_gate_case(home.path());
    let text = std::fs::read_to_string(&adr).expect("ADR");
    std::fs::write(
        &adr,
        text.replace(
            "- Дата: 2026-09-20",
            "- Дата: 2026-09-20\n- Модель-автор: my-llm-v1",
        ),
    )
    .expect("ADR с автором");
    judge_report_via_mcp(home.path(), &adr, &rubric, "other-llm-v2");
    let gate = gate_output(home.path(), &repo);
    let stdout = String::from_utf8_lossy(&gate.stdout);
    assert!(
        !stdout.contains("judge_same_family"),
        "разные неизвестные метки не одно семейство: {stdout}"
    );
    assert!(
        gate.status.success(),
        "ложного красного быть не должно: {stdout}"
    );
}

/// J5 (ADR-049): порог независимости. По умолчанию (`none`) поведение 0.3.4
/// сохраняется — отчёт, собранный хостом, зелёный. Проект, поднявший порог до
/// `launched`, получает находку `judge_independence_low` (error): судейство
/// «заявлено», а он требует, чтобы Spine сам запускал судью.
#[test]
fn min_independence_blocks_below_threshold() {
    let home = tempfile::tempdir().expect("tmp");
    let (repo, adr, rubric) = judge_gate_case(home.path());
    judge_report_via_mcp(home.path(), &adr, &rubric, "glm-5.2");
    // Дефолт: порога нет — отчёт, собранный хостом, проходит.
    let gate = gate_output(home.path(), &repo);
    let stdout = String::from_utf8_lossy(&gate.stdout);
    assert!(
        gate.status.success(),
        "дефолт обязан сохранять 0.3.4: {stdout}"
    );
    assert!(
        !stdout.contains("judge_independence_low"),
        "без порога находки нет: {stdout}"
    );
    // Проект требует запуск самим Spine — тот же отчёт краснеет.
    std::fs::write(
        home.path().join("arch-harness.toml"),
        "[gate.required]\nfast = [\"decision_quality\"]\n\n[gate.decision_quality]\nmin_independence = \"launched\"\n",
    )
    .expect("конфиг со строгим порогом");
    let strict = gate_output(home.path(), &repo);
    let stdout = String::from_utf8_lossy(&strict.stdout);
    assert!(!strict.status.success(), "порог обязан краснить: {stdout}");
    assert!(
        stdout.contains("judge_independence_low"),
        "ожидалась находка о пороге независимости: {stdout}"
    );
    assert!(
        stdout.contains("обеспечена запуском"),
        "находка называет требуемый уровень: {stdout}"
    );
}

/// J5 (ADR-049): судейство в рабочей сессии — ПРИМЕЧАНИЕ паспорта, а не
/// находка. Признак косвенный (судья мог видеть контекст автора), и выдавать
/// его за доказательство нельзя — но и молчать нельзя.
#[test]
fn session_not_clean_is_note_not_finding() {
    let home = tempfile::tempdir().expect("tmp");
    let (repo, adr, rubric) = judge_gate_case(home.path());
    judge_report_via_mcp(home.path(), &adr, &rubric, "glm-5.2");
    // Правим происхождение: судейство шло после сорока вызовов в сессии.
    let report = repo.join("reports/rubric/ADR-001-pilot.json");
    let mut artifact: Value =
        serde_json::from_str(&std::fs::read_to_string(&report).expect("отчёт")).expect("JSON");
    artifact["provenance"]["session_calls_before"] = json!(40);
    std::fs::write(
        &report,
        serde_json::to_string_pretty(&artifact).expect("JSON"),
    )
    .expect("запись");
    let gate = gate_output(home.path(), &repo);
    let stdout = String::from_utf8_lossy(&gate.stdout);
    assert!(
        gate.status.success(),
        "рабочая сессия — не нарушение: {stdout}"
    );
    assert!(
        !stdout.contains("judge_session_not_clean"),
        "примечание не должно выглядеть находкой: {stdout}"
    );
    // В паспорте примечание названо, и названо косвенным признаком.
    let explain = arch_cmd(home.path())
        .arg("gate")
        .arg("--repo")
        .arg(repo.as_os_str())
        .arg("--route")
        .arg("fast")
        .arg("--explain")
        .output()
        .expect("gate --explain");
    let text = String::from_utf8_lossy(&explain.stdout);
    assert!(
        text.contains("рабочей сессии"),
        "примечание в паспорте: {text}"
    );
    assert!(
        text.contains("косвенный признак"),
        "признак назван косвенным: {text}"
    );
}

/// J5 (ADR-049): хэши сырых ответов входят в аттестацию вердикта — правка
/// сохранённого ответа меняет аттестацию, потому что отчёт объявлен собранным
/// из этих ответов. У кейса без сырых ответов аттестация не меняется: вход
/// `judge_raw` там честное `absent`.
#[test]
fn envelope_changes_when_raw_answer_changes() {
    let home = tempfile::tempdir().expect("tmp");
    let (repo, adr, rubric) = judge_gate_case(home.path());
    judge_report_via_mcp(home.path(), &adr, &rubric, "glm-5.2");
    let envelope = |home: &Path, repo: &Path| -> Value {
        let out = arch_cmd(home)
            .arg("gate")
            .arg("--repo")
            .arg(repo.as_os_str())
            .arg("--route")
            .arg("fast")
            .arg("--format")
            .arg("json")
            .output()
            .expect("gate --format json");
        serde_json::from_slice(&out.stdout).expect("конверт вердикта")
    };
    let before = envelope(home.path(), &repo);
    assert!(
        before["inputs"]["judge_raw"].is_string(),
        "вход judge_raw назван: {before}"
    );
    let raw = repo.join("reports/rubric/raw/ADR-001-pilot/sample-1.json");
    let mut record: Value =
        serde_json::from_str(&std::fs::read_to_string(&raw).expect("сырой ответ")).expect("JSON");
    record["text"] = json!("{\"scores\":[],\"verdict\":\"иначе\"}");
    std::fs::write(&raw, serde_json::to_string_pretty(&record).expect("JSON")).expect("запись");
    let after = envelope(home.path(), &repo);
    assert_ne!(
        before["attestation"], after["attestation"],
        "правка сырого ответа обязана менять аттестацию"
    );
    assert_ne!(
        before["inputs"]["judge_raw"], after["inputs"]["judge_raw"],
        "вход judge_raw обязан измениться"
    );
}

/// J8 (ADR-048): `arch-be rubric run` читает секцию `[judge]` конфига — раньше
/// CLI подставлял дефолты, а MCP-инструмент брал конфиг, и два пути судили по
/// разным правилам. Судью подменяет фиктивный CLI-провайдер, поэтому живая
/// модель не нужна.
#[test]
fn cli_rubric_run_respects_judge_config() {
    let home = tempfile::tempdir().expect("tmp");
    let (repo, adr, rubric) = judge_gate_case(home.path());
    // Фиктивный CLI-судья: отвечает заготовленным JSON и считает вызовы.
    let counter = home.path().join("calls.txt");
    // Ответ судьи — отдельным файлом: шелл не трогает кавычки внутри JSON.
    let answer = home.path().join("answer.json");
    std::fs::write(
        &answer,
        r#"{"scores":[{"criterion_id":"context","score":4,"rationale":"Цитата: \"Контекст описан явно\""}],"verdict":"годно"}"#,
    )
    .expect("ответ судьи");
    let script = home.path().join("fake-judge.sh");
    std::fs::write(
        &script,
        format!(
            // Пауза после ответа: CLI, выходящий мгновенно, обрывает
            // чтение stdout — ответ доходил не целиком и судья перезапрашивался.
            "#!/bin/sh\ncat > /dev/null\necho x >> {calls}\ncat {answer}\nsleep 0.2\n",
            calls = counter.display(),
            answer = answer.display()
        ),
    )
    .expect("скрипт судьи");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mut perms = std::fs::metadata(&script).expect("stat").permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&script, perms).expect("chmod +x");
    }
    // samples = 5 в секции [judge]: дефолт — 3, поэтому «не прочитал секцию»
    // видно по числу обращений к судье (их будет меньше пяти).
    let assets = home.path().join("assets-test");
    std::fs::write(
        home.path().join("arch-harness.toml"),
        format!(
            "default_model = \"fake-judge\"\n\n[paths]\nassets_dir = \"{}\"\n\n\
             [judge]\nsamples = 5\n\n[models.fake-judge]\nkind = \"cli\"\n\
             command = \"{}\"\nargs = []\n",
            assets.display(),
            script.display()
        ),
    )
    .expect("конфиг");
    let out = arch_cmd(home.path())
        .arg("rubric")
        .arg("run")
        .arg(&rubric)
        .arg(adr.as_os_str())
        .arg("--model")
        .arg("fake-judge")
        .output()
        .expect("rubric run");
    assert!(
        out.status.success(),
        "прогон судьи: {}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let calls = std::fs::read_to_string(&counter).unwrap_or_default();
    // В отчёте — правила, по которым он собран.
    let artifact: Value = serde_json::from_str(
        &std::fs::read_to_string(repo.join("reports/rubric/ADR-001-pilot.json")).expect("отчёт"),
    )
    .expect("JSON отчёта");
    // Считаем не «ровно пять»: провайдер вправе перезапросить сэмпл, ответ
    // которого не разобрался (это его штатное поведение). Дискриминирует
    // нижняя граница: с дефолтными тремя сэмплами обращений было бы меньше.
    assert!(
        calls.lines().count() >= 5,
        "секция [judge] не прочитана: вызовов {}, снимок правил {}",
        calls.lines().count(),
        artifact["judge_config"]
    );
    assert_eq!(artifact["judge_config"]["samples"], 5, "{artifact}");
    assert_eq!(artifact["provenance"]["mode"], "launched", "{artifact}");
    assert_eq!(
        artifact["provenance"]["launcher"]["command"],
        json!(script.display().to_string()),
        "запускатель назван: {artifact}"
    );
    // В шапке этого ADR поля автора нет — источник метки честно `none`.
    assert_eq!(artifact["author_source"], "none", "{artifact}");
}

/// J7 (ADR-048): режим `--rw=reports` открывает запись ТОЛЬКО отчётам рубрики.
/// Отчёт судьи ложится на место (гейт его видит), а пишущие инструменты моста
/// остаются закрытыми — судейскому харнессу не нужны `adr_new` и
/// `delta_propose`, ради которых раньше приходилось открывать весь `--rw`.
#[test]
fn rw_reports_mode_allows_only_rubric_reports() {
    let home = tempfile::tempdir().expect("tmp");
    let (repo, adr, rubric) = judge_gate_case(home.path());
    let verify = call(
        1,
        "rubric_verify",
        &json!({
            "rubric": rubric,
            "target": adr.display().to_string(),
            "judge_model": "glm-5.2",
            "answers": [provenance_answer()],
        }),
    );
    let adr_dir = home.path().join("adr-out");
    std::fs::create_dir_all(&adr_dir).expect("mkdir");
    let new_adr = call(
        2,
        "adr_new",
        &json!({"title": "Новый ADR", "path": adr_dir.display().to_string()}),
    );
    let responses = mcp_serve_with_args(home.path(), &["--rw=reports"], &batch(&[verify, new_adr]));
    // Отчёт записан: режим открывает именно то, ради чего он нужен.
    let verdict = structured(&responses[0], 1);
    assert!(
        verdict["artifact_saved"].as_bool().expect("artifact_saved"),
        "отчёт обязан записаться: {verdict}"
    );
    assert!(
        repo.join("reports/rubric/ADR-001-pilot.json").is_file(),
        "файл отчёта на месте"
    );
    // `adr_new` в этом режиме закрыт.
    let text = responses[1].to_string();
    assert!(
        text.contains("неизвестный инструмент") || text.contains("не разрешён"),
        "adr_new обязан быть закрыт: {text}"
    );
    let created: Vec<_> = std::fs::read_dir(&adr_dir)
        .expect("чтение каталога")
        .flatten()
        .collect();
    assert!(
        created.is_empty(),
        "в режиме rw=reports ADR создавать нельзя: {created:?}"
    );
}

/// J7 (ADR-048), критерий успеха 4: в read-only контуре отчёт больше не теряется
/// молча. Первая строка сводки говорит, что гейт его не увидит и что делать, а
/// готовое содержимое едет в `artifact_json` — хост сохраняет его своими
/// средствами, и такой файл проходит ту же сверку с сырыми ответами.
#[test]
fn readonly_verify_says_report_not_saved_and_returns_artifact_json() {
    let home = tempfile::tempdir().expect("tmp");
    let (repo, adr, rubric) = judge_gate_case(home.path());
    let verify = call(
        1,
        "rubric_verify",
        &json!({
            "rubric": rubric,
            "target": adr.display().to_string(),
            "judge_model": "glm-5.2",
            "author_model": "claude-opus-4",
            "answers": [provenance_answer()],
        }),
    );
    let responses = mcp_serve_with_args(home.path(), &[], &batch(&[verify]));
    let verdict = structured(&responses[0], 1);
    assert_eq!(verdict["artifact_saved"], false, "{verdict}");
    let summary = verdict["summary"].as_str().expect("сводка");
    assert!(
        summary.starts_with("Отчёт НЕ сохранён"),
        "первая строка — про потерянный отчёт: {summary}"
    );
    assert!(summary.contains("--rw=reports"), "назван выход: {summary}");
    assert!(
        !repo.join("reports/rubric/ADR-001-pilot.json").is_file(),
        "read-only контур не пишет в рабочий каталог"
    );
    // Готовый файл: путь и содержимое.
    let path = verdict["artifact_path"].as_str().expect("путь").to_string();
    let json_text = verdict["artifact_json"].as_str().expect("содержимое");
    assert!(
        path.ends_with("reports/rubric/ADR-001-pilot.json"),
        "путь отчёта: {path}"
    );
    let artifact: Value = serde_json::from_str(json_text).expect("JSON отчёта");
    assert_eq!(artifact["schema"], "arch-be/rubric-report/v1", "{artifact}");
    assert_eq!(
        artifact["independence"], "declared_cross_family",
        "{artifact}"
    );
    // Хост сохраняет файл сам — гейт видит отчёт и остаётся зелёным.
    let target = repo.join("reports/rubric");
    std::fs::create_dir_all(&target).expect("mkdir");
    std::fs::write(target.join("ADR-001-pilot.json"), json_text).expect("запись отчёта хостом");
    let gate = gate_output(home.path(), &repo);
    let stdout = String::from_utf8_lossy(&gate.stdout);
    assert!(gate.status.success(), "гейт: {stdout}");
    assert!(
        stdout.contains("[PASS] decision_quality"),
        "сохранённый хостом отчёт засчитан: {stdout}"
    );
}

/// J6 (ADR-048): журнал вызовов несёт идентификатор сессии и хоста, а для
/// `rubric_verify` — мету судейства: цель, рубрику, судью, автора и уровень
/// независимости. Сами ответы судьи в журнал НЕ пишутся (они рядом с отчётом).
#[test]
fn journal_records_judging_meta_without_answers() {
    let home = tempfile::tempdir().expect("tmp");
    let (repo, adr, rubric) = judge_gate_case(home.path());
    let init = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {"clientInfo": {"name": "claude-code", "version": "2.1.278"}},
    })
    .to_string();
    let verify = call(
        2,
        "rubric_verify",
        &json!({
            "rubric": rubric,
            "target": adr.display().to_string(),
            "judge_model": "glm-5.2",
            "author_model": "claude-opus-4",
            "answers": [provenance_answer()],
        }),
    );
    let _ = mcp_serve_with_args(home.path(), &["--rw=reports"], &batch(&[init, verify]));
    // Журнал проектный: он пишется в `.arch-handoff/` рабочего каталога
    // сервера (cwd процесса), а не репозитория документа.
    let _ = &repo;
    let journal = std::fs::read_to_string(home.path().join(".arch-handoff/mcp-calls.jsonl"))
        .expect("журнал вызовов");
    let entry: Value = journal
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .find(|v| v["tool"] == "rubric_verify")
        .expect("запись о судействе");
    assert!(
        entry["session_id"].as_str().is_some_and(|s| !s.is_empty()),
        "сессия в журнале: {entry}"
    );
    assert_eq!(entry["host"], "claude-code", "{entry}");
    let meta = &entry["judging"];
    assert_eq!(meta["rubric"], "t-rubric", "{entry}");
    assert_eq!(meta["judge_model"], "glm-5.2", "{entry}");
    assert_eq!(meta["author_model"], "claude-opus-4", "{entry}");
    assert_eq!(meta["independence"], "declared_cross_family", "{entry}");
    // Ответы судьи в журнал не пишутся: только метки.
    assert!(
        !journal.contains("Цитата"),
        "содержимое ответа судьи не должно попадать в журнал"
    );
}

/// J6 (ADR-048): `arch-be digest` печатает раздел «Судейство» — что оценено,
/// кем, с каким уровнем независимости и сколько оценок пришлось на рабочую
/// сессию. Порог «рабочей сессии» — дефолтный (журнал не знает конфига).
#[test]
fn digest_has_judging_section() {
    let home = tempfile::tempdir().expect("tmp");
    let (repo, _adr, _rubric) = judge_gate_case(home.path());
    let journal = repo.join(".arch-handoff/mcp-calls.jsonl");
    std::fs::create_dir_all(journal.parent().expect("каталог")).expect("mkdir");
    let ts = chrono::Local::now().to_rfc3339();
    std::fs::write(
        &journal,
        format!(
            "{{\"ts\":\"{ts}\",\"tool\":\"rubric_verify\",\"verdict\":\"ok\",\"duration_ms\":5,\"session_id\":\"s1\",\"host\":\"claude-code\",\"judging\":{{\"target\":\"docs/adr/ADR-001-pilot.md\",\"rubric\":\"t-rubric\",\"judge_model\":\"glm-5.2\",\"author_model\":\"claude-opus-4\",\"independence\":\"declared_cross_family\",\"session_calls_before\":1}}}}\n{{\"ts\":\"{ts}\",\"tool\":\"rubric_verify\",\"verdict\":\"ok\",\"duration_ms\":5,\"session_id\":\"s2\",\"host\":\"qwen-code\",\"judging\":{{\"target\":\"docs/adr/ADR-001-pilot.md\",\"rubric\":\"t-rubric\",\"judge_model\":\"glm-5.2\",\"author_model\":\"claude-opus-4\",\"independence\":\"declared_cross_family\",\"session_calls_before\":9}}}}\n"
        ),
    )
    .expect("журнал");
    let out = arch_cmd(home.path())
        .arg("digest")
        .arg("--repo")
        .arg(repo.as_os_str())
        .output()
        .expect("digest");
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("## Судейство"), "{text}");
    assert!(text.contains("Оценок рубриками: 2"), "{text}");
    assert!(
        text.contains("переоценок: 1"),
        "одна цель оценена дважды: {text}"
    );
    assert!(text.contains("glm-5.2"), "судья назван: {text}");
    assert!(
        text.contains("declared_cross_family"),
        "уровень назван: {text}"
    );
    assert!(
        text.contains("рабочей сессии") && text.contains("косвенный"),
        "признак рабочей сессии назван косвенным: {text}"
    );
}
