//! Юнит-тесты прогона кодовых харнессов (перенесены из `harness.rs`:
//! продовый модуль держится под границей `prod_file_length_limit` — C-33).
//!
//! Тесты видят приватные элементы родителя через `use super::*;`.

use super::*;

/// Конфиг с assets внутри временного каталога (изоляция от ~/.arch-harness).
fn cfg_in(dir: &Path) -> Config {
    let mut cfg = Config::default();
    cfg.paths.assets_dir = dir.join("assets");
    cfg
}

fn write_file(path: &Path, text: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("mkdir");
    }
    std::fs::write(path, text).expect("write");
}

#[test]
fn builds_argv_per_prompt_mode() {
    let cfg = |args: &[&str], mode: PromptMode| CodingHarnessConfig {
        binary: "bin".into(),
        args: args.iter().map(|s| (*s).into()).collect(),
        prompt_mode: mode,
        ..CodingHarnessConfig::default()
    };

    // Positional: задача — позиционный аргумент в конце.
    let (argv, stdin) = build_argv(&cfg(&["-p"], PromptMode::Positional), "TASK");
    assert_eq!(argv, ["-p", "TASK"]);
    assert!(stdin.is_none());

    // Flag с плейсхолдером: подстановка на место {prompt}.
    let (argv, stdin) = build_argv(
        &cfg(&["agent", "--message", "{prompt}"], PromptMode::Flag),
        "TASK",
    );
    assert_eq!(argv, ["agent", "--message", "TASK"]);
    assert!(stdin.is_none());

    // Flag без плейсхолдера: задача добавляется в конец.
    let (argv, stdin) = build_argv(&cfg(&["run", "--task"], PromptMode::Flag), "TASK");
    assert_eq!(argv, ["run", "--task", "TASK"]);
    assert!(stdin.is_none());

    // Stdin: argv без задачи, задача — в stdin.
    let (argv, stdin) = build_argv(&cfg(&["-p"], PromptMode::Stdin), "TASK");
    assert_eq!(argv, ["-p"]);
    assert_eq!(stdin.as_deref(), Some("TASK"));
}

#[tokio::test]
async fn runs_stdin_harness_and_captures_output() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let cfg = CodingHarnessConfig {
        binary: "cat".into(),
        prompt_mode: PromptMode::Stdin,
        timeout_secs: 30,
        ..CodingHarnessConfig::default()
    };
    let run = run_harness("test-cat", &cfg, tmp.path(), "привет, харнесс")
        .await
        .expect("run");
    assert_eq!(run.harness, "test-cat");
    assert_eq!(run.exit_code, Some(0));
    assert_eq!(run.stdout, "привет, харнесс");
    assert_eq!(run.stderr, "");
    assert!(run.duration_secs >= 0.0);
    assert_eq!(run.termination, Termination::Completed);
}

#[tokio::test]
async fn missing_binary_returns_hint() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let cfg = CodingHarnessConfig {
        binary: "definitely-missing-bin".into(),
        timeout_secs: 5,
        ..CodingHarnessConfig::default()
    };
    let err = run_harness("theseus", &cfg, tmp.path(), "задача")
        .await
        .expect_err("должна быть ошибка");
    let msg = err.to_string();
    assert!(msg.contains("definitely-missing-bin"), "{msg}");
    assert!(msg.contains("установите"), "{msg}");
    assert!(msg.contains("[harnesses.theseus]"), "{msg}");
}

#[tokio::test]
async fn absolute_timeout_returns_partial_output() {
    // Регрессия 09-12: раньше таймаут возвращал Err без вывода — модель
    // не видела, что харнесс успел сделать.
    let tmp = tempfile::tempdir().expect("tempdir");
    let cfg = CodingHarnessConfig {
        binary: "sh".into(),
        args: vec!["-c".into(), "echo MARKER; sleep 60".into()],
        prompt_mode: PromptMode::Stdin,
        timeout_secs: 1,
        idle_timeout_secs: 0,
        ..CodingHarnessConfig::default()
    };
    let run = run_harness("slow", &cfg, tmp.path(), "задача")
        .await
        .expect("прерывание — Ok с частичным выводом");
    assert_eq!(run.termination, Termination::AbsoluteTimeout);
    assert!(run.stdout.contains("MARKER"), "stdout: {}", run.stdout);
    assert!(run.duration_secs < 30.0, "{:?}", run.duration_secs);
}

#[tokio::test]
async fn idle_timeout_fires_on_silence() {
    // Молчащий и непишущий процесс убивается по idle, не дожидаясь
    // абсолютного потолка.
    let tmp = tempfile::tempdir().expect("tempdir");
    let cfg = CodingHarnessConfig {
        binary: "sleep".into(),
        args: vec!["60".into()],
        prompt_mode: PromptMode::Stdin,
        timeout_secs: 120,
        idle_timeout_secs: 2,
        ..CodingHarnessConfig::default()
    };
    let run = run_harness("silent", &cfg, tmp.path(), "задача")
        .await
        .expect("run");
    assert_eq!(run.termination, Termination::IdleTimeout);
    assert!(run.duration_secs < 30.0, "{:?}", run.duration_secs);
}

#[tokio::test]
async fn file_activity_resets_idle() {
    // Молчащий, но пишущий файлы процесс (типичный кодовый харнесс)
    // НЕ считается зависшим: heartbeat по mtime репозитория.
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).expect("mkdir repo");
    let cfg = CodingHarnessConfig {
        binary: "sh".into(),
        args: vec![
            "-c".into(),
            "i=0; while [ $i -lt 10 ]; do touch \"f$i\"; i=$((i+1)); sleep 1; done; echo done"
                .into(),
        ],
        prompt_mode: PromptMode::Stdin,
        timeout_secs: 60,
        // Запас против нагрузки параллельного тест-сьюта: gap между
        // touch ~1 с при окне 8 с — флаки не будет.
        idle_timeout_secs: 8,
        ..CodingHarnessConfig::default()
    };
    let run = run_harness("writer", &cfg, &repo, "задача")
        .await
        .expect("run");
    assert_eq!(
        run.termination,
        Termination::Completed,
        "stderr: {}",
        run.stderr
    );
    assert!(run.stdout.contains("done"), "stdout: {}", run.stdout);
    assert!(repo.join("f9").is_file());
}

#[tokio::test]
async fn timeout_kills_whole_process_group() {
    // Регрессия 09-12 (скриншот пользователя): таймаут убивал обёртку,
    // а дочерний процесс харнесса оставался жить сиротой. Теперь группа
    // завершается целиком (TERM → KILL по -pgid).
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).expect("mkdir repo");
    let cfg = CodingHarnessConfig {
        binary: "sh".into(),
        args: vec![
            "-c".into(),
            "sleep 300 & echo $! > child.pid; sleep 300".into(),
        ],
        prompt_mode: PromptMode::Stdin,
        timeout_secs: 1,
        idle_timeout_secs: 0,
        ..CodingHarnessConfig::default()
    };
    let run = run_harness("spawner", &cfg, &repo, "задача")
        .await
        .expect("run");
    assert_eq!(run.termination, Termination::AbsoluteTimeout);
    let pid = std::fs::read_to_string(repo.join("child.pid")).expect("child.pid");
    let pid = pid.trim();
    // Процесс «убит» = /proc нет ИЛИ зомби (Z/X): зомби уже мёртв, его
    // просто ещё не забрал родитель. kill -0 на зомби возвращает success,
    // поэтому проверяем state, а не сам факт ответа.
    let alive = std::fs::read_to_string(format!("/proc/{pid}/stat"))
        .ok()
        .and_then(|s| {
            s.rsplit(')')
                .next()?
                .split_whitespace()
                .next()
                .map(str::to_owned)
        })
        .is_some_and(|state| !matches!(state.as_str(), "Z" | "X" | "x"));
    assert!(!alive, "дочерний процесс {pid} пережил таймаут — сирота");
}

/// git-репозиторий с одним baseline-коммитом (явная идентичность —
/// на CI/в контейнерах user.name/user.email может не быть).
fn git_repo_with_baseline(dir: &std::path::Path) {
    std::fs::create_dir_all(dir).expect("mkdir repo");
    std::fs::write(dir.join("README.md"), "# baseline\n").expect("readme");
    let git = |args: &[&str]| {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .expect("git");
        assert!(out.status.success(), "git {args:?}: {:?}", out.stderr);
    };
    git(&["init", "-q"]);
    git(&["add", "README.md"]);
    git(&[
        "-c",
        "user.name=test",
        "-c",
        "user.email=test@test",
        "commit",
        "-q",
        "-m",
        "baseline",
    ]);
}

/// Харнесс-заглушка: пишет код + интерпретерный мусор, НЕ коммитит
/// (воспроизводит дефект «агенты завершились без финального коммита»).
fn dirty_executor_cfg() -> CodingHarnessConfig {
    CodingHarnessConfig {
        binary: "sh".into(),
        args: vec![
            "-c".into(),
            "mkdir -p spinecalc __pycache__ .arch-handoff; \
                 echo 'def validate_amount(v, l): return True' > spinecalc/amount.py; \
                 echo junk > __pycache__/x.pyc; \
                 echo meta > .arch-handoff/TASK.md; \
                 echo '{\"status\": \"complete\"}'"
                .into(),
        ],
        prompt_mode: PromptMode::Stdin,
        timeout_secs: 30,
        idle_timeout_secs: 0,
        ..CodingHarnessConfig::default()
    }
}

#[tokio::test]
async fn auto_commit_commits_executor_leftovers() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = tmp.path().join("repo");
    git_repo_with_baseline(&repo);
    let run = run_harness(
        "dirty",
        &dirty_executor_cfg(),
        &repo,
        "реализовать модуль amount",
    )
    .await
    .expect("run");
    assert_eq!(run.termination, Termination::Completed);
    let ac = run.auto_commit.expect("харнесс обязан до-коммитить хвост");
    assert_eq!(ac.files, 1, "только код, без мусора: {ac:?}");
    assert!(
        ac.message
            .starts_with("harness(dirty): реализовать модуль amount")
    );
    assert_ne!(ac.hash, "");
    // В истории — baseline + авто-коммит с кодом; физически в дереве
    // остаются лишь некоммитимые служебные/мусорные каталоги.
    let status = git_out(&repo, &["status", "--porcelain"]).expect("status");
    for line in status.lines() {
        assert!(
            line.contains(".arch-handoff/") || line.contains("__pycache__/"),
            "посторонний незакоммиченный путь: {line}"
        );
    }
    let log = git_out(&repo, &["log", "--oneline"]).expect("log");
    assert_eq!(log.lines().count(), 2, "{log}");
    let committed = git_out(&repo, &["show", "--name-only", "--pretty=%s", "HEAD"]).expect("show");
    assert!(committed.contains("spinecalc/amount.py"), "{committed}");
    assert!(!committed.contains("__pycache__"), "{committed}");
    assert!(!committed.contains(".arch-handoff"), "{committed}");
}

#[tokio::test]
async fn auto_commit_disabled_leaves_tree_dirty() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = tmp.path().join("repo");
    git_repo_with_baseline(&repo);
    let cfg = CodingHarnessConfig {
        auto_commit: false,
        ..dirty_executor_cfg()
    };
    let run = run_harness("dirty-off", &cfg, &repo, "задача")
        .await
        .expect("run");
    assert_eq!(run.termination, Termination::Completed);
    assert!(run.auto_commit.is_none());
    let status = git_out(&repo, &["status", "--porcelain"]).expect("status");
    assert!(status.contains("spinecalc/"), "{status}");
}

#[tokio::test]
async fn env_allow_whitelist_isolates_harness_env() {
    // Разрыв P1 «окружение протекает между харнессами»: при непустом
    // env_allow процесс стартует с чистым окружением + whitelist + env
    // адаптера; пустой список — наследование окружения (как раньше).
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).expect("mkdir");
    let probe = |env_allow: Vec<&str>| CodingHarnessConfig {
        binary: "/bin/sh".into(),
        args: vec![
            "-c".into(),
            "echo \"H=${HOME:-EMPTY} P=${PATH:+set} E=${EXTRA:-EMPTY}\"".into(),
        ],
        prompt_mode: PromptMode::Stdin,
        timeout_secs: 30,
        idle_timeout_secs: 0,
        auto_commit: false,
        env_allow: env_allow.iter().map(|s| (*s).into()).collect(),
        env: [("EXTRA".to_string(), "yes".to_string())]
            .into_iter()
            .collect(),
        ..CodingHarnessConfig::default()
    };
    // Наследование по умолчанию: HOME и PATH видны, EXTRA из env — тоже.
    let run = run_harness("probe", &probe(vec![]), &repo, "задача")
        .await
        .expect("run");
    assert!(run.stdout.contains("H=/"), "наследование: {}", run.stdout);
    assert!(run.stdout.contains("P=set E=yes"), "{}", run.stdout);
    // Whitelist без HOME: HOME у процесса пуст, PATH и EXTRA на месте.
    let run = run_harness("probe", &probe(vec!["PATH"]), &repo, "задача")
        .await
        .expect("run");
    assert!(
        run.stdout.contains("H=EMPTY P=set E=yes"),
        "изоляция: {}",
        run.stdout
    );
}

#[tokio::test]
async fn auto_commit_clean_repo_is_noop() {
    // Исполнитель всё закоммитил сам (или ничего не писал) — харнесс
    // не плодит пустых коммитов.
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = tmp.path().join("repo");
    git_repo_with_baseline(&repo);
    let cfg = CodingHarnessConfig {
        binary: "sh".into(),
        args: vec!["-c".into(), "echo ok".into()],
        prompt_mode: PromptMode::Stdin,
        timeout_secs: 30,
        idle_timeout_secs: 0,
        ..CodingHarnessConfig::default()
    };
    let run = run_harness("clean", &cfg, &repo, "задача")
        .await
        .expect("run");
    assert_eq!(run.termination, Termination::Completed);
    assert!(run.auto_commit.is_none(), "пустой коммит не нужен");
    let log = git_out(&repo, &["log", "--oneline"]).expect("log");
    assert_eq!(log.lines().count(), 1, "{log}");
}

/// Конфиг с поддельным харнессом `fake` на бинаре `cat` (stdin → stdout).
fn cfg_with_fake_harness(dir: &Path) -> Config {
    let mut cfg = cfg_in(dir);
    cfg.harnesses.insert(
        "fake".into(),
        CodingHarnessConfig {
            binary: "cat".into(),
            prompt_mode: PromptMode::Stdin,
            timeout_secs: 30,
            ..CodingHarnessConfig::default()
        },
    );
    cfg
}

#[test]
fn parse_result_contract_validates_schema_mechanically() {
    // Валидный полный контракт в последнем fenced-блоке.
    let stdout = "текст\n```json\n{\"status\": \"partial\", \"assumptions\": [\"a\"], \
                      \"open_questions\": [], \"conflicts_with_prior_decisions\": []}\n```\n";
    let ContractParse::Valid(c) = parse_result_contract(stdout) else {
        panic!("контракт найден и валиден");
    };
    assert_eq!(c.status, ContractStatus::Partial);
    assert_eq!(c.assumptions, vec!["a".to_string()]);
    // Списки опциональны: дефолт — пустые.
    let ContractParse::Valid(c) = parse_result_contract("```json\n{\"status\": \"complete\"}\n```")
    else {
        panic!("валиден без списков");
    };
    assert_eq!(c.status, ContractStatus::Complete);
    assert!(c.open_questions.is_empty() && c.conflicts.is_empty());
    // Без поля status — не контракт.
    assert_eq!(
        parse_result_contract("```json\n{\"x\": 1}\n```"),
        ContractParse::Missing
    );
    assert_eq!(parse_result_contract("plain text"), ContractParse::Missing);
    // Битый JSON без status — промах; со status — Invalid (не молчим).
    assert_eq!(
        parse_result_contract("```json\n{oops}\n```"),
        ContractParse::Missing
    );
    let ContractParse::Invalid(reason) =
        parse_result_contract("```json\n{\"status\": \"complete\",\n```")
    else {
        panic!("битый блок со status — Invalid");
    };
    assert!(reason.contains("невалидный JSON"), "{reason}");
    // Схема: status вне перечисления — Invalid.
    let ContractParse::Invalid(reason) =
        parse_result_contract("```json\n{\"status\": \"done\"}\n```")
    else {
        panic!("status вне перечисления — Invalid");
    };
    assert!(reason.contains("done"), "{reason}");
    // Список не массивом — Invalid.
    let ContractParse::Invalid(reason) =
        parse_result_contract("```json\n{\"status\": \"complete\", \"assumptions\": {}}\n```")
    else {
        panic!("assumptions не массив — Invalid");
    };
    assert!(reason.contains("assumptions"), "{reason}");
    // Fence уронен — голый JSON в хвосте подхватывается.
    let ContractParse::Valid(c) = parse_result_contract(
        "проза ответа\n{\"status\": \"blocked\", \"open_questions\": [\"нужен доступ к КШД\"]}",
    ) else {
        panic!("голый JSON в хвосте — валидный контракт");
    };
    assert_eq!(c.status, ContractStatus::Blocked);
    assert_eq!(c.open_questions, vec!["нужен доступ к КШД".to_string()]);
    // Последний из нескольких fenced-блоков со status побеждает.
    let two = "```json\n{\"status\": \"partial\"}\n```\nпромежуток\n```json\n{\"status\": \"complete\"}\n```";
    let ContractParse::Valid(c) = parse_result_contract(two) else {
        panic!("последний блок валиден");
    };
    assert_eq!(c.status, ContractStatus::Complete);
}

#[tokio::test]
async fn harness_run_tool_validates_args() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let cfg = cfg_with_fake_harness(tmp.path());
    let tool = HarnessRunTool { cfg: cfg.clone() };
    assert_eq!(tool.spec().name, "harness_run");
    let ctx = ToolContext::new(tmp.path().to_path_buf(), Arc::new(cfg));

    let out = tool.call(json!({"repo": "."}), &ctx).await.expect("call");
    assert!(
        out.is_error && out.content.contains("'harness'"),
        "{}",
        out.content
    );

    // path — имя из схемы; repo — исторический алиас. До фикса обработчик
    // читал только `repo`, а required ссылался на несуществующее свойство:
    // агент, передающий path по схеме, получал ложный отказ (инцидент
    // «обязательный аргумент 'repo' отсутствует» при корректном вызове).
    let out = tool
        .call(json!({"harness": "nope", "path": "."}), &ctx)
        .await
        .expect("call");
    assert!(out.is_error, "{}", out.content);
    assert!(
        out.content.contains("не настроен"),
        "path резолвится как repo: {}",
        out.content
    );

    let out = tool
        .call(json!({"harness": "nope", "repo": "."}), &ctx)
        .await
        .expect("call");
    assert!(out.is_error, "{}", out.content);
    assert!(out.content.contains("не настроен"), "{}", out.content);
    assert!(
        out.content.contains("claude-code"),
        "список известных: {}",
        out.content
    );

    // Нет task и нет TASK.md — понятная ошибка с подсказкой.
    let out = tool
        .call(json!({"harness": "fake", "repo": "."}), &ctx)
        .await
        .expect("call");
    assert!(out.is_error, "{}", out.content);
    assert!(out.content.contains("handoff_create"), "{}", out.content);
}

#[tokio::test]
async fn harness_run_background_registers_and_completes() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let mut cfg = cfg_with_fake_harness(tmp.path());
    cfg.paths.reports_dir = tmp.path().join("reports");
    let tool = HarnessRunTool { cfg: cfg.clone() };
    let registry = crate::subagent::SubagentRegistry::new();
    let ctx =
        ToolContext::new(tmp.path().to_path_buf(), Arc::new(cfg)).with_subagents(registry.clone());

    // Фон: инструмент возвращается сразу, задача hr-* в реестре.
    let out = tool
        .call(
            json!({"harness": "fake", "repo": ".", "task": "фон", "background": true}),
            &ctx,
        )
        .await
        .expect("call");
    assert!(!out.is_error, "{}", out.content);
    assert!(out.content.contains("hr-"), "id задачи: {}", out.content);
    assert_eq!(registry.running(), 1, "прогон числится запущенным");

    // cat завершается мгновенно — дожидаемся финиша задачи.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let task = registry
            .list()
            .into_iter()
            .find(|t| t.id.starts_with("hr-"))
            .expect("задача hr-* в реестре");
        if task.status != crate::subagent::TaskStatus::Running {
            assert_eq!(
                task.status,
                crate::subagent::TaskStatus::Done,
                "отчёт: {}",
                task.report
            );
            assert!(task.report.contains("завершился"), "отчёт: {}", task.report);
            assert!(task.finished_at.is_some());
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "фоновый прогон не завершился за 10 с"
        );
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    // Полный лог лежит файлом reports/harness/<id>.log.
    let logs = std::fs::read_dir(tmp.path().join("reports/harness")).expect("logs dir");
    assert_eq!(logs.count(), 1, "один лог-файл фонового прогона");
}

#[tokio::test]
async fn harness_run_background_without_registry_is_clear_error() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let cfg = cfg_with_fake_harness(tmp.path());
    let tool = HarnessRunTool { cfg: cfg.clone() };
    // Без with_subagents — реестр не подключён (headless).
    let ctx = ToolContext::new(tmp.path().to_path_buf(), Arc::new(cfg));
    let out = tool
        .call(
            json!({"harness": "fake", "repo": ".", "task": "фон", "background": true}),
            &ctx,
        )
        .await
        .expect("call");
    assert!(out.is_error, "{}", out.content);
    assert!(out.content.contains("реестр"), "{}", out.content);
}

#[tokio::test]
async fn harness_run_hot_reloads_adapter_config() {
    // Регрессия: агент исправил [harnesses.*] в config.toml в ходе сессии,
    // а прогон шёл со снапшота конфига, загруженного при старте процесса
    // (дефолтный `-p` для hermes — «unrecognized arguments»). Теперь
    // адаптер перечитывается из файла на каждый вызов.
    let tmp = tempfile::tempdir().expect("tempdir");
    let cfg_path = tmp.path().join("config.toml");
    let write_cfg = |marker: &str| {
        std::fs::write(
            &cfg_path,
            format!(
                "default_model = \"deepseek\"\n[harnesses.fake]\nbinary = \"sh\"\n\
                     args = [\"-c\", \"echo {marker}\"]\nprompt_mode = \"stdin\"\n\
                     timeout_secs = 30\nidle_timeout_secs = 0\nauto_commit = false\n"
            ),
        )
        .expect("write config");
    };
    write_cfg("ADAPTER_V1");
    let cfg = Config::load(Some(&cfg_path)).expect("load");
    assert_eq!(cfg.loaded_from.as_deref(), Some(cfg_path.as_path()));
    let tool = HarnessRunTool { cfg: cfg.clone() };
    let ctx = ToolContext::new(tmp.path().to_path_buf(), Arc::new(cfg));

    let out = tool
        .call(
            json!({"harness": "fake", "repo": ".", "task": "задача"}),
            &ctx,
        )
        .await
        .expect("call 1");
    assert!(out.content.contains("ADAPTER_V1"), "{}", out.content);

    // Правка файла между вызовами — без пересоздания инструмента.
    write_cfg("ADAPTER_V2");
    let out = tool
        .call(
            json!({"harness": "fake", "repo": ".", "task": "задача"}),
            &ctx,
        )
        .await
        .expect("call 2");
    assert!(out.content.contains("ADAPTER_V2"), "{}", out.content);
    assert!(!out.content.contains("ADAPTER_V1"), "{}", out.content);
}

#[tokio::test]
async fn harness_run_reads_task_md_and_extracts_contract() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = tmp.path().join("repo");
    write_file(
        &repo.join(".arch-handoff/TASK.md"),
        "Сделай фичу\n\n```json\n{\"status\": \"complete\", \"assumptions\": [], \
             \"open_questions\": [\"q1\"], \"conflicts_with_prior_decisions\": []}\n```\n",
    );
    let cfg = cfg_with_fake_harness(tmp.path());
    let tool = HarnessRunTool { cfg };
    let ctx = ToolContext::new(tmp.path().to_path_buf(), Arc::new(Config::default()));

    // cat вернёт TASK.md в stdout — контракт извлекается в сводку.
    let out = tool
        .call(json!({"harness": "fake", "repo": "repo"}), &ctx)
        .await
        .expect("call");
    assert!(!out.is_error, "{}", out.content);
    assert!(out.content.contains("код 0"), "{}", out.content);
    assert!(out.content.contains("status=complete"), "{}", out.content);
    assert!(out.content.contains("open_questions: 1"), "{}", out.content);
    assert!(out.content.contains("Сделай фичу"), "{}", out.content);
}

#[tokio::test]
async fn harness_run_warns_when_contract_missing() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let mut cfg = cfg_in(tmp.path());
    cfg.harnesses.insert(
        "fake".into(),
        CodingHarnessConfig {
            binary: "echo".into(),
            args: vec!["нет контракта".into()],
            prompt_mode: PromptMode::Stdin,
            timeout_secs: 30,
            ..CodingHarnessConfig::default()
        },
    );
    let tool = HarnessRunTool { cfg };
    let ctx = ToolContext::new(tmp.path().to_path_buf(), Arc::new(Config::default()));
    // echo не читает stdin, печатает строку без контракта, код 0.
    let out = tool
        .call(
            json!({"harness": "fake", "repo": ".", "task": "задача"}),
            &ctx,
        )
        .await
        .expect("call");
    assert!(!out.is_error, "{}", out.content);
    assert!(
        out.content.contains("контракт результата"),
        "{}",
        out.content
    );
    assert!(out.content.contains("не найден"), "{}", out.content);
}

#[tokio::test]
async fn harness_run_raises_tiny_timeout_to_floor() {
    // Модель оптимистично просит 30 с — поднимаем до 600 и честно
    // сообщаем об этом в сводке (ранний обрыв оставлял репо полусобранным).
    let tmp = tempfile::tempdir().expect("tempdir");
    let cfg = cfg_with_fake_harness(tmp.path());
    let tool = HarnessRunTool { cfg };
    let ctx = ToolContext::new(tmp.path().to_path_buf(), Arc::new(Config::default()));
    let out = tool
        .call(
            json!({"harness": "fake", "repo": ".", "task": "задача", "timeout_secs": 30}),
            &ctx,
        )
        .await
        .expect("call");
    assert!(!out.is_error, "{}", out.content);
    assert!(out.content.contains("поднят до 600"), "{}", out.content);
    // Явный разумный таймаут не трогаем.
    let out = tool
        .call(
            json!({"harness": "fake", "repo": ".", "task": "задача", "timeout_secs": 900}),
            &ctx,
        )
        .await
        .expect("call");
    assert!(!out.content.contains("поднят"), "{}", out.content);
}

#[test]
fn harness_run_tool_timeout_covers_longest_run() {
    // Регрессия 11-24: агентный цикл обрывал вызов на жёстких 300 с
    // (TOOL_TIMEOUT_SECS), пока адаптер ждал 1800. Таймаут инструмента
    // обязан покрывать потолок аргумента (7200) плюс запас.
    let tool = HarnessRunTool {
        cfg: Config::default(),
    };
    assert!(tool.timeout_secs() >= 7200 + 120, "{}", tool.timeout_secs());
    let mut cfg = Config::default();
    cfg.harnesses.insert(
        "long".into(),
        CodingHarnessConfig {
            binary: "true".into(),
            timeout_secs: 8000,
            ..CodingHarnessConfig::default()
        },
    );
    assert_eq!(HarnessRunTool { cfg }.timeout_secs(), 8000 + 120);
}

#[tokio::test]
async fn enforce_run_worktree_disabled_by_default() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let cfg = cfg_in(tmp.path());
    // require_worktree = false (дефолт) — enforcement выключен.
    let res = enforce_run_worktree(&cfg, tmp.path(), "fake")
        .await
        .expect("enforce");
    assert!(res.is_none());
}

#[tokio::test]
async fn enforce_run_worktree_creates_worktree_and_copies_handoff() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = tmp.path().join("repo");
    git_repo_with_baseline(&repo);
    // Handoff-пакет по контракту НЕ коммитится — в свежем worktree его нет,
    // enforcement обязан его скопировать.
    write_file(&repo.join(".arch-handoff/TASK.md"), "задача из пакета\n");
    let mut cfg = cfg_in(tmp.path());
    cfg.paths.reports_dir = tmp.path().join("reports");
    cfg.fleet.require_worktree = true;

    let (dir, run_id) = enforce_run_worktree(&cfg, &repo, "Claude Code")
        .await
        .expect("enforce")
        .expect("worktree создан");
    // Имя — kebab-case «<слаг>-<timestamp>» (слаг санитизирован).
    assert!(run_id.starts_with("claude-code-"), "{run_id}");
    assert!(dir.join("README.md").is_file(), "worktree имеет базу");
    let task = std::fs::read_to_string(dir.join(".arch-handoff/TASK.md")).expect("TASK.md");
    assert_eq!(task, "задача из пакета\n");
    // Основное дерево не тронуто: ветка worktree в списке фабрики.
    let infos = crate::worktree::list(&repo).await.expect("list");
    assert_eq!(infos.len(), 1, "{infos:?}");
    assert_eq!(infos[0].name, run_id);
}

#[tokio::test]
async fn enforce_run_worktree_requires_git_repo() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let mut cfg = cfg_in(tmp.path());
    cfg.fleet.require_worktree = true;
    // Не git-репозиторий — понятная ошибка ДО запуска харнесса.
    let err = enforce_run_worktree(&cfg, tmp.path(), "fake")
        .await
        .expect_err("не git-репозиторий");
    assert!(err.to_string().contains("не git-репозиторий"), "{err}");
}

#[tokio::test]
async fn harness_run_tool_enforces_worktree_when_required() {
    // Сквозной путь: [fleet] require_worktree = true → harness_run прогоняет
    // харнесс в worktree, основное дерево остаётся чистым, в сводке —
    // run-id и указание на гейт владельца.
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = tmp.path().join("repo");
    git_repo_with_baseline(&repo);
    let mut cfg = cfg_with_fake_harness(tmp.path());
    cfg.paths.reports_dir = tmp.path().join("reports");
    cfg.fleet.require_worktree = true;
    // Исполнитель пишет файл и НЕ коммитит (auto_commit выключен, чтобы
    // работа осталась незакоммиченной в worktree — main всё равно чист).
    cfg.harnesses.get_mut("fake").expect("fake").auto_commit = false;
    cfg.harnesses.insert(
        "writer".into(),
        CodingHarnessConfig {
            binary: "sh".into(),
            args: vec![
                "-c".into(),
                "echo код > feature.txt; echo '{\"status\": \"complete\"}'".into(),
            ],
            prompt_mode: PromptMode::Stdin,
            timeout_secs: 30,
            idle_timeout_secs: 0,
            auto_commit: false,
            ..CodingHarnessConfig::default()
        },
    );
    let tool = HarnessRunTool { cfg };
    let ctx = ToolContext::new(tmp.path().to_path_buf(), Arc::new(Config::default()));
    let out = tool
        .call(
            json!({"harness": "writer", "repo": "repo", "task": "сделать фичу"}),
            &ctx,
        )
        .await
        .expect("call");
    assert!(!out.is_error, "{}", out.content);
    assert!(out.content.contains("run-id 'writer-"), "{}", out.content);
    assert!(
        out.content.contains("arch fleet merge"),
        "указание на гейт: {}",
        out.content
    );
    // Основное дерево чисто — работа осталась в worktree.
    let status = git_out(&repo, &["status", "--porcelain"]).expect("status");
    assert!(status.trim().is_empty(), "main не тронут: {status}");
    assert!(!repo.join("feature.txt").is_file());
    let infos = crate::worktree::list(&repo).await.expect("list");
    assert_eq!(infos.len(), 1, "{infos:?}");
    assert!(infos[0].path.join("feature.txt").is_file());
}

// --- A4: пост-гейт прогона ---------------------------------------------

/// git-команда в песочнице с фиксированной идентичностью коммиттера
/// (на CI/в контейнерах user.name/user.email может не быть).
fn git_sandbox(dir: &Path, args: &[&str]) {
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
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Песочница A4 (сценарий RA-6): git-репо с handoff-пакетом — реестром
/// `no_pan_in_logs`, Stop-хуком в `.claude/settings.json`, спайном и
/// `MANIFEST.json` с пинами контрольной плоскости и `baseline_commit`.
///
/// `spine_tracked = false` — спайн создан ПОСЛЕ базового коммита
/// (untracked): удаление такого файла исполнителем не попадает в
/// git-diff и не валит `delta_guard` — красный обязан прийти именно от
/// `spine_lint` (INCOMPLETE), как в тесте «удаление спайна».
///
/// Возвращает sha baseline-коммита (базу пост-гейта).
fn a4_sandbox(repo: &Path, spine_tracked: bool) -> String {
    write_file(&repo.join("src/a.rs"), "fn main() {}\n");
    write_file(
        &repo.join(".claude/settings.json"),
        "{\"hooks\":{\"Stop\":[{\"hooks\":[{\"type\":\"command\",\
             \"command\":\"arch-be gate --base HEAD\"}]}]}}\n",
    );
    write_file(
        &repo.join(".arch-handoff/CONSTRAINTS.yaml"),
        "rules:\n  - id: X-1\n    name: no_pan_in_logs\n    type: must_not_contain\n    \
             glob: \"src/**/*.rs\"\n    pattern: \"PAN=\"\n    severity: error\n",
    );
    if spine_tracked {
        write_file(&repo.join("ARCHITECTURE-SPINE.md"), "# Spine\n");
    }
    git_sandbox(repo, &["init", "-q"]);
    git_sandbox(repo, &["config", "user.name", "t"]);
    git_sandbox(repo, &["config", "user.email", "t@t"]);
    git_sandbox(repo, &["add", "."]);
    git_sandbox(repo, &["commit", "-q", "-m", "baseline"]);
    let sha = git_out(repo, &["rev-parse", "HEAD"])
        .expect("rev-parse")
        .trim()
        .to_string();
    if !spine_tracked {
        write_file(&repo.join("ARCHITECTURE-SPINE.md"), "# Spine\n");
    }
    // Пины снимаются ПОСЛЕ всех файлов пакета — иначе пакет сам себе
    // вечное расхождение (тот же порядок, что у handoff_create).
    let pins = crate::control_plane::collect(repo);
    let manifest = serde_json::json!({
        "baseline_commit": sha,
        "model": "stub-model",
        "control_plane": pins,
    });
    write_file(
        &repo.join(".arch-handoff/MANIFEST.json"),
        &serde_json::to_string_pretty(&manifest).expect("manifest json"),
    );
    sha
}

/// Заглушка-исполнитель RA-6: удаляет Stop-хук, пишет PAN в код, коммитит.
fn a4_toxic_cfg() -> CodingHarnessConfig {
    CodingHarnessConfig {
        binary: "sh".into(),
        args: vec![
            "-c".into(),
            "printf '{}' > .claude/settings.json; \
                 echo 'log(\"PAN=4111111111111111\")' > src/a.rs; \
                 git add -A; git commit -qm toxic; \
                 echo '{\"status\": \"complete\"}'"
                .into(),
        ],
        prompt_mode: PromptMode::Stdin,
        timeout_secs: 30,
        idle_timeout_secs: 0,
        ..CodingHarnessConfig::default()
    }
}

/// Заглушка-исполнитель чистого прогона: коммитит безобидный файл.
fn a4_clean_cfg() -> CodingHarnessConfig {
    CodingHarnessConfig {
        binary: "sh".into(),
        args: vec![
            "-c".into(),
            "echo feature > feature.txt; git add -A; git commit -qm feature; \
                 echo '{\"status\": \"complete\"}'"
                .into(),
        ],
        prompt_mode: PromptMode::Stdin,
        timeout_secs: 30,
        idle_timeout_secs: 0,
        ..CodingHarnessConfig::default()
    }
}

/// Заглушка-исполнитель: удаляет спайн (untracked) и коммитит безобидное.
fn a4_drop_spine_cfg() -> CodingHarnessConfig {
    CodingHarnessConfig {
        binary: "sh".into(),
        args: vec![
            "-c".into(),
            "rm ARCHITECTURE-SPINE.md; echo ok > notes.md; git add -A; \
                 git commit -qm notes; echo '{\"status\": \"complete\"}'"
                .into(),
        ],
        prompt_mode: PromptMode::Stdin,
        timeout_secs: 30,
        idle_timeout_secs: 0,
        ..CodingHarnessConfig::default()
    }
}

/// Заглушка-исполнитель, подменяющая `MANIFEST.json`: своя база и пустые
/// пины `control_plane` под своё состояние (A4.1b). Пишет безобидный код —
/// прочие составляющие гейта PASS, красный обязан прийти от сверки.
fn a4_tamper_manifest_cfg() -> CodingHarnessConfig {
    CodingHarnessConfig {
        binary: "sh".into(),
        args: vec![
            "-c".into(),
            "printf '%s' '{\"baseline_commit\":\"deadbeefdeadbeef\",\"model\":\"stub\",\
                 \"control_plane\":{}}' > .arch-handoff/MANIFEST.json; \
                 echo feature > feature.txt; git add -A; git commit -qm sneaky; \
                 echo '{\"status\": \"complete\"}'"
                .into(),
        ],
        prompt_mode: PromptMode::Stdin,
        timeout_secs: 30,
        idle_timeout_secs: 0,
        ..CodingHarnessConfig::default()
    }
}

/// Заглушка-исполнитель, создающая `MANIFEST.json` в прогоне (на старте
/// его не было): база не назначена выдачей — пост-гейт обязан остаться
/// SKIP, а не поверить манифесту исполнителя (A4.1b).
fn a4_add_manifest_cfg() -> CodingHarnessConfig {
    CodingHarnessConfig {
        binary: "sh".into(),
        args: vec![
            "-c".into(),
            "printf '%s' '{\"baseline_commit\":\"cafecafecafecafe\",\"model\":\"stub\",\
                 \"control_plane\":{}}' > .arch-handoff/MANIFEST.json; \
                 echo feature > feature.txt; git add -A; git commit -qm add-manifest; \
                 echo '{\"status\": \"complete\"}'"
                .into(),
        ],
        prompt_mode: PromptMode::Stdin,
        timeout_secs: 30,
        idle_timeout_secs: 0,
        ..CodingHarnessConfig::default()
    }
}

/// RA-6 после фикса: заглушка удаляет Stop-хук и пишет PAN — пост-гейт
/// красный (`control_plane_tampered` + `no_pan_in_logs`), итог прогона
/// ошибка, хотя исполнитель вернул код 0 / `status=complete`.
#[tokio::test]
async fn post_gate_red_on_toxic_run_ra6() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = tmp.path().join("repo");
    let base = a4_sandbox(&repo, true);
    let cfg = a4_toxic_cfg();
    let run = run_harness("stub", &cfg, &repo, "задача")
        .await
        .expect("run");
    assert_eq!(run.exit_code, Some(0), "исполнитель завершился кодом 0");
    let pg = run
        .post_gate
        .expect("пост-гейт обязан отработать (пакет есть)");
    assert_eq!(pg.verdict, PostGateVerdict::Fail, "{pg:?}");
    assert_eq!(pg.exit_code, Some(1), "exit-код гейта: {pg:?}");
    assert_eq!(pg.base.as_deref(), Some(base.as_str()));
    let joined = pg.findings.join("\n");
    assert!(joined.contains("control_plane_tampered"), "{joined}");
    assert!(joined.contains("no_pan_in_logs"), "{joined}");
    // Итог для оркестратора — ошибка, хотя исполнитель «успешен».
    let out = execute_run("stub", &cfg, &repo, "задача", String::new()).await;
    assert!(out.is_error, "{}", out.content);
    assert!(out.content.contains("Пост-гейт: FAIL"), "{}", out.content);
    assert!(
        out.content.contains("control_plane_tampered"),
        "{}",
        out.content
    );
    println!("RA-6 после фикса:\n{}", out.content);
}

/// Чистый прогон: пост-гейт PASS, ошибка — только по коду исполнителя.
#[tokio::test]
async fn post_gate_passes_on_clean_run() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = tmp.path().join("repo");
    a4_sandbox(&repo, true);
    let cfg = a4_clean_cfg();
    let run = run_harness("stub", &cfg, &repo, "задача")
        .await
        .expect("run");
    let pg = run.post_gate.expect("пост-гейт отработал");
    assert_eq!(pg.verdict, PostGateVerdict::Pass, "{pg:?}");
    assert_eq!(pg.exit_code, Some(0), "{pg:?}");
    assert!(pg.findings.is_empty(), "{:?}", pg.findings);
    assert!(!pg.verdict.is_red());
    let out = execute_run("stub", &cfg, &repo, "задача", String::new()).await;
    assert!(!out.is_error, "{}", out.content);
    assert!(out.content.contains("Пост-гейт: PASS"), "{}", out.content);
}

/// A4.1b: заглушка переписывает `MANIFEST.json` (своя база + пины под своё
/// состояние) при безобидном коде — пост-гейт красный с `manifest_tampered`
/// независимо от того, что остальные составляющие PASS. Проверяемый не
/// управляет базой проверки.
#[tokio::test]
async fn post_gate_red_on_manifest_tampering() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = tmp.path().join("repo");
    a4_sandbox(&repo, true);
    let cfg = a4_tamper_manifest_cfg();
    let run = run_harness("stub", &cfg, &repo, "задача")
        .await
        .expect("run");
    assert_eq!(run.exit_code, Some(0), "исполнитель завершился кодом 0");
    let pg = run
        .post_gate
        .expect("пост-гейт обязан отработать (пакет был на старте)");
    assert_eq!(pg.verdict, PostGateVerdict::Fail, "{pg:?}");
    assert!(pg.verdict.is_red());
    assert!(
        pg.summary.contains("MANIFEST.json"),
        "сводка о подмене: {}",
        pg.summary
    );
    let joined = pg.findings.join("\n");
    assert!(joined.contains("manifest_tampered"), "{joined}");
    assert!(joined.contains("пересоздать handoff"), "{joined}");

    // Итог для оркестратора — ошибка. Свежая песочница: повторный прогон
    // той же заглушки перезаписал бы манифест идентично, и подмена не
    // отличалась бы от снимка.
    let repo2 = tmp.path().join("repo2");
    a4_sandbox(&repo2, true);
    let out = execute_run("stub", &cfg, &repo2, "задача", String::new()).await;
    assert!(out.is_error, "{}", out.content);
    assert!(out.content.contains("Пост-гейт: FAIL"), "{}", out.content);
    assert!(out.content.contains("manifest_tampered"), "{}", out.content);
}

/// A4.1b: заглушка не трогает `MANIFEST.json` — находки `manifest_tampered`
/// нет, вердикт прежний (без тамперинга поведение не изменилось).
#[tokio::test]
async fn post_gate_no_tamper_finding_when_manifest_untouched() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = tmp.path().join("repo");
    a4_sandbox(&repo, true);
    let cfg = a4_clean_cfg();
    let run = run_harness("stub", &cfg, &repo, "задача")
        .await
        .expect("run");
    let pg = run.post_gate.expect("пост-гейт отработал");
    assert_eq!(pg.verdict, PostGateVerdict::Pass, "{pg:?}");
    assert!(
        !pg.findings.iter().any(|f| f.contains("manifest_tampered")),
        "ложная находка о подмене: {:?}",
        pg.findings
    );
}

/// A4.1b: манифеста не было ДО запуска, исполнитель создал его в прогоне —
/// это не тамперинг и не база: пост-гейт остаётся SKIP (нет доверенной
/// базы сверки), пометка о подмене не нужна.
#[tokio::test]
async fn post_gate_skips_when_manifest_appears_mid_run() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = tmp.path().join("repo");
    a4_sandbox(&repo, true);
    std::fs::remove_file(repo.join(".arch-handoff/MANIFEST.json")).expect("rm manifest");
    let cfg = a4_add_manifest_cfg();
    let run = run_harness("stub", &cfg, &repo, "задача")
        .await
        .expect("run");
    assert!(
        repo.join(".arch-handoff/MANIFEST.json").is_file(),
        "исполнитель создал манифест в прогоне"
    );
    let pg = run.post_gate.expect("вердикт с пометкой SKIP");
    assert_eq!(pg.verdict, PostGateVerdict::Skipped, "{pg:?}");
    assert!(!pg.verdict.is_red());
    assert!(
        !pg.findings.iter().any(|f| f.contains("manifest_tampered")),
        "появление манифеста в прогоне — не тамперинг: {:?}",
        pg.findings
    );
    // Итог прогона — не ошибка (SKIP не красный). Свежая песочница:
    // во второй прогон той же заглушки манифест уже был бы на старте.
    let repo2 = tmp.path().join("repo2");
    a4_sandbox(&repo2, true);
    std::fs::remove_file(repo2.join(".arch-handoff/MANIFEST.json")).expect("rm manifest");
    let out = execute_run("stub", &cfg, &repo2, "задача", String::new()).await;
    assert!(!out.is_error, "{}", out.content);
}

/// Отключение адаптером: вердикта нет (None), предупреждение — в выводе.
#[tokio::test]
async fn post_gate_disabled_by_adapter_warns() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = tmp.path().join("repo");
    a4_sandbox(&repo, true);
    let cfg = CodingHarnessConfig {
        post_gate: false,
        ..a4_clean_cfg()
    };
    let run = run_harness("stub", &cfg, &repo, "задача")
        .await
        .expect("run");
    assert!(run.post_gate.is_none(), "отключён — вердикта нет");
    let out = execute_run("stub", &cfg, &repo, "задача", String::new()).await;
    assert!(!out.is_error, "{}", out.content);
    assert!(
        out.content
            .contains("пост-гейт отключён адаптером post_gate = false"),
        "{}",
        out.content
    );
}

/// Нет handoff-пакета: пост-гейт SKIP с пометкой, не ошибка.
#[tokio::test]
async fn post_gate_skips_without_package() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = tmp.path().join("repo");
    a4_sandbox(&repo, true);
    std::fs::remove_file(repo.join(".arch-handoff/MANIFEST.json")).expect("rm manifest");
    let cfg = a4_clean_cfg();
    let run = run_harness("stub", &cfg, &repo, "задача")
        .await
        .expect("run");
    let pg = run.post_gate.expect("вердикт с пометкой SKIP");
    assert_eq!(pg.verdict, PostGateVerdict::Skipped, "{pg:?}");
    assert!(pg.summary.contains("нет handoff-пакета"), "{}", pg.summary);
    assert!(!pg.verdict.is_red());
    let out = execute_run("stub", &cfg, &repo, "задача", String::new()).await;
    assert!(!out.is_error, "{}", out.content);
}

/// Пакет старого формата (нет `baseline_commit`): SKIP с пометкой.
#[tokio::test]
async fn post_gate_skips_old_manifest_without_baseline() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = tmp.path().join("repo");
    a4_sandbox(&repo, true);
    write_file(
        &repo.join(".arch-handoff/MANIFEST.json"),
        "{\"model\": \"stub-model\"}\n",
    );
    let cfg = a4_clean_cfg();
    let run = run_harness("stub", &cfg, &repo, "задача")
        .await
        .expect("run");
    let pg = run.post_gate.expect("вердикт с пометкой SKIP");
    assert_eq!(pg.verdict, PostGateVerdict::Skipped, "{pg:?}");
    assert!(pg.summary.contains("baseline_commit"), "{}", pg.summary);
    assert!(!pg.verdict.is_red());
}

/// Удаление спайна заглушкой: `spine_lint` без входа → INCOMPLETE →
/// красный итог для прогона с пакетом (обязательная составляющая SKIP).
#[tokio::test]
async fn post_gate_red_when_spine_removed_incomplete() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = tmp.path().join("repo");
    a4_sandbox(&repo, false);
    let cfg = a4_drop_spine_cfg();
    let run = run_harness("stub", &cfg, &repo, "задача")
        .await
        .expect("run");
    let pg = run.post_gate.expect("пост-гейт отработал");
    assert_eq!(pg.verdict, PostGateVerdict::Incomplete, "{pg:?}");
    assert!(pg.summary.contains("spine_lint"), "{}", pg.summary);
    assert!(pg.verdict.is_red());
    let out = execute_run("stub", &cfg, &repo, "задача", String::new()).await;
    assert!(out.is_error, "{}", out.content);
}

/// Таймаут пост-гейта: медленный реестр (`command_succeeds: sleep`) не
/// укладывается в 1 с — вердикт TIMEOUT, итог красный (RA-5).
#[tokio::test]
async fn post_gate_timeout_is_red() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = tmp.path().join("repo");
    a4_sandbox(&repo, true);
    write_file(
        &repo.join(".arch-handoff/CONSTRAINTS.yaml"),
        "rules:\n  - id: X-1\n    name: no_pan_in_logs\n    type: must_not_contain\n    \
             glob: \"src/**/*.rs\"\n    pattern: \"PAN=\"\n    severity: error\n  \
             - id: X-2\n    name: slow\n    type: command_succeeds\n    command: \"sleep 5\"\n    \
             severity: error\n",
    );
    let cfg = CodingHarnessConfig {
        post_gate_timeout_secs: 1,
        ..a4_clean_cfg()
    };
    let run = run_harness("stub", &cfg, &repo, "задача")
        .await
        .expect("run");
    let pg = run.post_gate.expect("пост-гейт отработал");
    assert_eq!(pg.verdict, PostGateVerdict::Timeout, "{pg:?}");
    assert!(pg.verdict.is_red());
    let out = execute_run("stub", &cfg, &repo, "задача", String::new()).await;
    assert!(out.is_error, "{}", out.content);
    assert!(
        out.content.contains("Пост-гейт: TIMEOUT"),
        "{}",
        out.content
    );
}

/// Тот же красный прогон через инструмент `harness_run` (путь MCP):
/// вывод инструмента обязан нести вердикт и находки, а `is_error` — быть
/// истинным одинаково в синхронном и фоновом каналах.
#[tokio::test]
async fn post_gate_red_visible_through_harness_run_tool() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = tmp.path().join("repo");
    a4_sandbox(&repo, true);
    let mut cfg = cfg_in(tmp.path());
    cfg.harnesses.insert("stub".into(), a4_toxic_cfg());
    let tool = HarnessRunTool { cfg: cfg.clone() };
    let ctx = ToolContext::new(tmp.path().to_path_buf(), Arc::new(cfg));
    let out = tool
        .call(
            json!({"harness": "stub", "path": "repo", "task": "задача"}),
            &ctx,
        )
        .await
        .expect("call");
    assert!(out.is_error, "{}", out.content);
    assert!(out.content.contains("Пост-гейт: FAIL"), "{}", out.content);
    assert!(
        out.content.contains("control_plane_tampered"),
        "{}",
        out.content
    );
    assert!(out.content.contains("no_pan_in_logs"), "{}", out.content);
}

/// Фоновый прогон: карточка задачи `hr-*` показывает тот же вывод —
/// статус Failed и отчёт с вердиктом пост-гейта (отдельного TUI-кода не
/// нужно: карточка рендерит `content` инструмента).
#[tokio::test]
async fn post_gate_red_marks_background_task_failed() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = tmp.path().join("repo");
    a4_sandbox(&repo, true);
    let mut cfg = cfg_in(tmp.path());
    cfg.paths.reports_dir = tmp.path().join("reports");
    cfg.harnesses.insert("stub".into(), a4_toxic_cfg());
    let tool = HarnessRunTool { cfg: cfg.clone() };
    let registry = crate::subagent::SubagentRegistry::new();
    let ctx =
        ToolContext::new(tmp.path().to_path_buf(), Arc::new(cfg)).with_subagents(registry.clone());
    let out = tool
        .call(
            json!({"harness": "stub", "path": "repo", "task": "задача", "background": true}),
            &ctx,
        )
        .await
        .expect("call");
    assert!(!out.is_error, "{}", out.content);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        let task = registry
            .list()
            .into_iter()
            .find(|t| t.id.starts_with("hr-"))
            .expect("задача hr-* в реестре");
        if task.status != crate::subagent::TaskStatus::Running {
            assert_eq!(
                task.status,
                crate::subagent::TaskStatus::Failed,
                "отчёт: {}",
                task.report
            );
            assert!(
                task.report.contains("Пост-гейт: FAIL"),
                "отчёт: {}",
                task.report
            );
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "фоновый прогон не завершился за 30 с"
        );
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}

// --- A5: самоодобрение изнутри диапазона прогона (ADR-055) --------------

/// Заглушка A5: файлы сценария уже разложены в песочнице — исполнитель
/// фиксирует их одним коммитом (так работа и попадает в диапазон
/// `baseline_commit..HEAD` пост-гейта).
fn a5_commit_cfg() -> CodingHarnessConfig {
    CodingHarnessConfig {
        binary: "sh".into(),
        args: vec![
            "-c".into(),
            "git add -A; git commit -qm ra7; echo '{\"status\": \"complete\"}'".into(),
        ],
        prompt_mode: PromptMode::Stdin,
        timeout_secs: 30,
        idle_timeout_secs: 0,
        ..CodingHarnessConfig::default()
    }
}

/// Дата override в пределах горизонта (12 мес.): сегодня + 6 месяцев.
fn a5_within_horizon() -> String {
    (chrono::Local::now() + chrono::Duration::days(182))
        .format("%Y-%m-%d")
        .to_string()
}

/// A5 (ADR-055), сценарий RA-7 дословно через пост-гейт: исполнитель
/// создаёт НАСТОЯЩИЙ Accepted ADR-900, override (горизонт в пределах),
/// ослабляет пакетный реестр (error→warn), пишет PAN в код и свою дельту
/// `relax3`. Пост-гейт (диапазон `baseline..HEAD`) красный независимо от
/// кода возврата исполнителя: `self_approved` + `rule_weakened`; итог —
/// ошибка. До фикса тот же сценарий был зелёным (оба канала
/// самоодобрения открыты).
#[tokio::test]
async fn post_gate_self_approved_on_real_adr_and_override_ra7() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = tmp.path().join("repo");
    let base = a4_sandbox(&repo, true);
    write_file(
        &repo.join("docs/adr/ADR-900-real.md"),
        "# ADR-900. Настоящее решение\n\n- Status: Accepted\n\n## Context\n\n\
             создан исполнителем\n",
    );
    write_file(
        &repo.join(".arch-handoff/CONSTRAINTS.yaml"),
        &format!(
            "rules:\n  - id: X-1\n    name: no_pan_in_logs\n    type: must_not_contain\n    \
                 glob: \"src/**/*.rs\"\n    pattern: \"PAN=\"\n    severity: warn\n\
                 overrides:\n  - rule: X-1\n    adr: ADR-900\n    until: \"{}\"\n",
            a5_within_horizon()
        ),
    );
    write_file(
        &repo.join("changes/relax3/DELTA.md"),
        "# Дельта relax3\n\nПравим .arch-handoff/CONSTRAINTS.yaml: ослабление по ADR-900.\n",
    );
    write_file(
        &repo.join("src/a.rs"),
        "fn main() { log(\"PAN=4111111111111111\"); }\n",
    );
    let cfg = a5_commit_cfg();
    let run = run_harness("stub", &cfg, &repo, "задача")
        .await
        .expect("run");
    assert_eq!(run.exit_code, Some(0), "исполнитель завершился кодом 0");
    let pg = run
        .post_gate
        .expect("пост-гейт обязан отработать (пакет есть)");
    assert_eq!(pg.verdict, PostGateVerdict::Fail, "{pg:?}");
    assert_eq!(pg.base.as_deref(), Some(base.as_str()));
    let joined = pg.findings.join("\n");
    assert!(joined.contains("self_approved"), "{joined}");
    assert!(joined.contains("rule_weakened"), "{joined}");
    let out = execute_run("stub", &cfg, &repo, "задача", String::new()).await;
    assert!(out.is_error, "{}", out.content);
    assert!(out.content.contains("self_approved"), "{}", out.content);
    println!("RA-7 после фикса:\n{}", out.content);
}

/// A5 (ADR-055), канал дельт: исполнитель правит пакетный реестр после
/// выдачи пакета и оформляет правку СВОЕЙ дельтой, упоминающей файл. С
/// диапазоном дельта не легализует расхождение контрольной плоскости:
/// `control_plane_tampered` + `self_approved` (имя артефакта — дельта).
#[tokio::test]
async fn post_gate_self_approved_on_delta_ra7() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = tmp.path().join("repo");
    a4_sandbox(&repo, true);
    // Ослабления нет — меняется только запись файла (комментарий): красный
    // обязан прийти от канала дельт, а не от rule_weakened.
    write_file(
        &repo.join(".arch-handoff/CONSTRAINTS.yaml"),
        "rules:\n  - id: X-1\n    name: no_pan_in_logs\n    type: must_not_contain\n    \
             glob: \"src/**/*.rs\"\n    pattern: \"PAN=\"\n    severity: error\n\
             # правка после выдачи пакета\n",
    );
    write_file(
        &repo.join("changes/relax3/DELTA.md"),
        "# Дельта relax3\n\nПравим .arch-handoff/CONSTRAINTS.yaml: безобидный комментарий.\n",
    );
    let cfg = a5_commit_cfg();
    let run = run_harness("stub", &cfg, &repo, "задача")
        .await
        .expect("run");
    let pg = run
        .post_gate
        .expect("пост-гейт обязан отработать (пакет есть)");
    assert_eq!(pg.verdict, PostGateVerdict::Fail, "{pg:?}");
    let joined = pg.findings.join("\n");
    assert!(joined.contains("control_plane_tampered"), "{joined}");
    assert!(joined.contains("self_approved"), "{joined}");
    assert!(joined.contains("дельта relax3"), "{joined}");
}

/// Путь к фикстуре-«агенту» ACP (та же, что у юнит-тестов `harness::acp`).
#[cfg(feature = "harness")]
fn acp_fixture(mode: &str) -> crate::config::AcpConfig {
    crate::config::AcpConfig {
        binary: "python3".into(),
        args: vec![
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/acp_agent.py")
                .to_string_lossy()
                .into_owned(),
            mode.to_string(),
        ],
        init_timeout_secs: 3,
        cancel_grace_secs: 1,
    }
}

/// C3: `mode = "auto"` без acp-секции — headless (поведение до ADR-057).
#[tokio::test]
async fn auto_without_acp_section_runs_headless() {
    let tmp = tempfile::tempdir().expect("tmp");
    let cfg = CodingHarnessConfig {
        binary: "cat".into(),
        prompt_mode: PromptMode::Stdin,
        timeout_secs: 30,
        idle_timeout_secs: 0,
        ..CodingHarnessConfig::default()
    };
    let run = run_harness("fake", &cfg, tmp.path(), "задача")
        .await
        .expect("run");
    assert_eq!(run.mode, HarnessMode::Prompt);
    assert!(run.acp.is_none(), "headless-путь не несёт ACP-метаданных");
    assert_eq!(run.stdout, "задача");
}

/// C3: `mode = "prompt"` принудительно игнорирует задекларированную acp-секцию.
#[tokio::test]
async fn explicit_prompt_ignores_acp_section() {
    let tmp = tempfile::tempdir().expect("tmp");
    let cfg = CodingHarnessConfig {
        binary: "cat".into(),
        prompt_mode: PromptMode::Stdin,
        timeout_secs: 30,
        idle_timeout_secs: 0,
        mode: AcpMode::Prompt,
        acp: Some(acp_fixture("ok")),
        ..CodingHarnessConfig::default()
    };
    let run = run_harness("fake", &cfg, tmp.path(), "задача")
        .await
        .expect("run");
    assert_eq!(
        run.mode,
        HarnessMode::Prompt,
        "prompt — принудительный headless"
    );
    assert_eq!(run.stdout, "задача", "шёл headless-путь (cat), не ACP");
}

/// C3: `mode = "auto"` + провал ACP-инициализации → откат на headless
/// с предупреждением в итоге.
#[tokio::test]
async fn auto_falls_back_to_headless_on_acp_init_failure() {
    let tmp = tempfile::tempdir().expect("tmp");
    let cfg = CodingHarnessConfig {
        binary: "cat".into(),
        prompt_mode: PromptMode::Stdin,
        timeout_secs: 30,
        idle_timeout_secs: 0,
        acp: Some(acp_fixture("init-exit")),
        ..CodingHarnessConfig::default()
    };
    let run = run_harness("fake", &cfg, tmp.path(), "задача")
        .await
        .expect("откат — не ошибка");
    assert_eq!(run.mode, HarnessMode::Fallback);
    assert_eq!(run.stdout, "задача", "откат выполнил headless-путь");
    let note = run.mode_note.expect("предупреждение об откате");
    assert!(note.contains("ACP"), "какое исключение: {note}");
    assert!(note.contains("initialize"), "{note}");
}

/// F2: `auto` + `authMethods` БЕЗ authenticate-cap — информационный список,
/// ACP-сессия продолжается (без отката на headless), журнал информационный.
#[tokio::test]
async fn auto_keeps_acp_when_auth_methods_informational() {
    let tmp = tempfile::tempdir().expect("tmp");
    let cfg = CodingHarnessConfig {
        binary: "cat".into(),
        prompt_mode: PromptMode::Stdin,
        timeout_secs: 30,
        idle_timeout_secs: 0,
        acp: Some(acp_fixture("auth")),
        ..CodingHarnessConfig::default()
    };
    let run = run_harness("fake", &cfg, tmp.path(), "задача")
        .await
        .expect("ACP-сессия");
    assert_eq!(
        run.mode,
        HarnessMode::Acp,
        "authMethods без cap не фатальны"
    );
    assert!(
        run.stdout.contains("\"status\": \"complete\""),
        "сессия дошла до end_turn: {}",
        run.stdout
    );
    let info = run.acp.expect("ACP-метаданные");
    assert!(
        info.journal
            .iter()
            .any(|j| j.contains("authMethods") && j.contains("информационно")),
        "информационная запись в журнале: {:?}",
        info.journal
    );
}

/// F2: `auto` + агент заявил authenticate-cap при непустых `authMethods` →
/// откат на headless с честной причиной (клиент аутентификацию не поддерживает).
#[tokio::test]
async fn auto_falls_back_on_required_auth() {
    let tmp = tempfile::tempdir().expect("tmp");
    let cfg = CodingHarnessConfig {
        binary: "cat".into(),
        prompt_mode: PromptMode::Stdin,
        timeout_secs: 30,
        idle_timeout_secs: 0,
        acp: Some(acp_fixture("auth-required")),
        ..CodingHarnessConfig::default()
    };
    let run = run_harness("fake", &cfg, tmp.path(), "задача")
        .await
        .expect("откат — не ошибка");
    assert_eq!(run.mode, HarnessMode::Fallback);
    assert_eq!(run.stdout, "задача", "откат выполнил headless-путь");
    let note = run.mode_note.expect("предупреждение об откате");
    assert!(note.contains("authMethods"), "{note}");
}

/// F2: `mode = "acp"` + authenticate-cap при непустых `authMethods` —
/// ошибка прогона без отката.
#[tokio::test]
async fn explicit_acp_errors_on_required_auth() {
    let tmp = tempfile::tempdir().expect("tmp");
    let cfg = CodingHarnessConfig {
        binary: "cat".into(),
        prompt_mode: PromptMode::Stdin,
        timeout_secs: 30,
        idle_timeout_secs: 0,
        mode: AcpMode::Acp,
        acp: Some(acp_fixture("auth-required")),
        ..CodingHarnessConfig::default()
    };
    let err = run_harness("fake", &cfg, tmp.path(), "задача")
        .await
        .expect_err("mode=acp: authenticate-cap + authMethods = ошибка без отката");
    assert!(err.to_string().contains("authMethods"), "{err}");
}

/// C3: `mode = "acp"` — провал инициализации БЕЗ отката (ошибка прогона).
#[tokio::test]
async fn explicit_acp_does_not_fall_back() {
    let tmp = tempfile::tempdir().expect("tmp");
    let cfg = CodingHarnessConfig {
        binary: "cat".into(),
        prompt_mode: PromptMode::Stdin,
        timeout_secs: 30,
        idle_timeout_secs: 0,
        mode: AcpMode::Acp,
        acp: Some(acp_fixture("init-exit")),
        ..CodingHarnessConfig::default()
    };
    let err = run_harness("fake", &cfg, tmp.path(), "задача")
        .await
        .expect_err("mode=acp: провал = ошибка без отката");
    assert!(err.to_string().contains("ACP"), "{err}");
}

/// C3/C5: `auto` + рабочий ACP-агент → режим `acp`, метаданные адаптера.
#[tokio::test]
async fn auto_uses_acp_when_declared_and_working() {
    let tmp = tempfile::tempdir().expect("tmp");
    let cfg = CodingHarnessConfig {
        binary: "cat".into(),
        prompt_mode: PromptMode::Stdin,
        timeout_secs: 30,
        idle_timeout_secs: 0,
        acp: Some(acp_fixture("ok")),
        ..CodingHarnessConfig::default()
    };
    let run = run_harness("fake", &cfg, tmp.path(), "задача")
        .await
        .expect("run");
    assert_eq!(run.mode, HarnessMode::Acp);
    let info = run.acp.expect("ACP-метаданные");
    assert_eq!(info.adapter.as_deref(), Some("fixture-agent 0.0.1"));
    assert_eq!(info.permissions, 0);
    assert!(
        run.stdout.contains("\"status\": \"complete\""),
        "финальный текст ACP подан как stdout: {}",
        run.stdout
    );
    // Контракт разобран механически — из ACP-текста, не из сырого вывода.
    assert!(matches!(run.contract, ContractParse::Valid(_)));
}

// --- Срез «полный stopReason» (дельта agent-modes-acp) ------------------

/// ACP-ход, оборванный по лимиту токенов, — `TurnLimit`: частичный ответ
/// разобран на JSON-контракт, пост-гейт и авто-коммит работают как для
/// `Completed`, итог — предупреждение (не ошибка исполнения).
#[tokio::test]
async fn acp_turn_limit_keeps_partial_contract_and_runs_post_gate() {
    let tmp = tempfile::tempdir().expect("tmp");
    let repo = tmp.path().join("repo");
    a4_sandbox(&repo, true);
    let cfg = CodingHarnessConfig {
        binary: "cat".into(),
        prompt_mode: PromptMode::Stdin,
        timeout_secs: 30,
        idle_timeout_secs: 0,
        mode: AcpMode::Acp,
        acp: Some(acp_fixture("limit-tokens")),
        ..CodingHarnessConfig::default()
    };
    let run = run_harness("fake", &cfg, &repo, "задача")
        .await
        .expect("run");
    assert_eq!(
        run.termination,
        Termination::TurnLimit(TurnLimitReason::MaxTokens)
    );
    let ContractParse::Valid(c) = &run.contract else {
        panic!("частичный контракт обязан разобраться: {:?}", run.contract);
    };
    assert_eq!(c.status, ContractStatus::Partial, "{c:?}");
    let pg = run
        .post_gate
        .expect("пост-гейт обязан отработать (пакет есть)");
    assert_eq!(pg.verdict, PostGateVerdict::Pass, "{pg:?}");
    let out = execute_run("fake", &cfg, &repo, "задача", String::new()).await;
    assert!(
        !out.is_error,
        "TurnLimit — не ошибка исполнения: {}",
        out.content
    );
    assert!(
        out.content
            .contains("ход оборван по лимиту (max_tokens), ответ частичный"),
        "{}",
        out.content
    );
    assert!(out.content.contains("Пост-гейт: PASS"), "{}", out.content);
}

/// Мягкая отмена, подтверждённая агентом после нашего `session/cancel`, —
/// `Cancelled`: итог «прерван», не ошибка прогона.
#[tokio::test]
async fn acp_cancelled_after_cancel_is_soft_not_error() {
    let tmp = tempfile::tempdir().expect("tmp");
    let cfg = CodingHarnessConfig {
        binary: "cat".into(),
        prompt_mode: PromptMode::Stdin,
        timeout_secs: 2,
        idle_timeout_secs: 0,
        mode: AcpMode::Acp,
        acp: Some(acp_fixture("cancelled-after-cancel")),
        ..CodingHarnessConfig::default()
    };
    let run = run_harness("fake", &cfg, tmp.path(), "задача")
        .await
        .expect("run");
    assert_eq!(run.termination, Termination::Cancelled);
    let out = execute_run("fake", &cfg, tmp.path(), "задача", String::new()).await;
    assert!(!out.is_error, "мягкая отмена — не ошибка: {}", out.content);
    assert!(out.content.contains("ПРЕРВАН МЯГКО"), "{}", out.content);
}

/// Отказ агента (`refusal`) — ошибка прогона; текст отказа сохранён в выводе.
#[tokio::test]
async fn acp_refusal_is_run_error() {
    let tmp = tempfile::tempdir().expect("tmp");
    let cfg = CodingHarnessConfig {
        binary: "cat".into(),
        prompt_mode: PromptMode::Stdin,
        timeout_secs: 30,
        idle_timeout_secs: 0,
        mode: AcpMode::Acp,
        acp: Some(acp_fixture("refusal")),
        ..CodingHarnessConfig::default()
    };
    let run = run_harness("fake", &cfg, tmp.path(), "задача")
        .await
        .expect("run");
    assert_eq!(run.termination, Termination::Refused);
    let out = execute_run("fake", &cfg, tmp.path(), "задача", String::new()).await;
    assert!(out.is_error, "отказ — ошибка прогона: {}", out.content);
    assert!(out.content.contains("ОТКАЗАЛСЯ"), "{}", out.content);
    assert!(
        out.content.contains("отказываюсь выполнять"),
        "текст агента сохранён: {}",
        out.content
    );
}
