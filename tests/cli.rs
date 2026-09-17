//! Интеграционные тесты CLI-контракта `arch-be` (ревью `SPINE-REVIEW.md`,
//! находка F-7 / задача P0-5; решения — `docs/adr/ADR-005-ci-and-cli-tests.md`).
//!
//! Все тесты детерминированы и офлайн: дом изолирован в tempdir
//! (см. [`common::arch_cmd`]), живые LLM и кодовые харнессы не вызываются.

mod common;

use std::path::{Path, PathBuf};

use predicates::prelude::*;
use predicates::str::contains;

use common::arch_cmd;

/// Пишет исполняемый shell-скрипт фейкового кодового харнесса: печатает
/// в stdout headless JSON-контракт результата (fenced json-блок со
/// `status`) и завершается нулём. Возвращает путь к скрипту.
/// Только сборка `harness` (команда `harness-run` в core отсутствует).
#[cfg(feature = "harness")]
fn write_fake_harness(dir: &Path, contract_json: &str) -> PathBuf {
    let script = dir.join("fake-harness.sh");
    let body = format!("#!/bin/sh\necho '```json'\necho '{contract_json}'\necho '```'\n");
    std::fs::write(&script, body).expect("запись fake-харнесса");
    // +x: без права на исполнение spawn вернёт PermissionDenied.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mut perms = std::fs::metadata(&script)
            .expect("stat скрипта")
            .permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&script, perms).expect("chmod +x");
    }
    script
}

/// Готовая команда `arch-be harness-run fake`: контракт `contract_json`
/// печатает фейковый харнесс, прописанный в тестовом config.toml.
#[cfg(feature = "harness")]
fn harness_run_cmd(home: &Path, contract_json: &str) -> assert_cmd::Command {
    let script = write_fake_harness(home, contract_json);
    // Харнесс-адаптер собирается из конфига: бинарь — наш скрипт, задача
    // уходит в stdin (скрипт её игнорирует), авто-коммит выключен (репо —
    // не git), таймауты малые, чтобы зависший прогон падал быстро.
    let config = home.join("config.toml");
    let text = format!(
        "[harnesses.fake]\nbinary = '{}'\nprompt_mode = 'stdin'\n\
         timeout_secs = 30\nidle_timeout_secs = 0\nauto_commit = false\n",
        script.display()
    );
    std::fs::write(&config, text).expect("запись config.toml");
    let repo = home.join("repo");
    std::fs::create_dir_all(&repo).expect("mkdir repo");
    let mut cmd = arch_cmd(home);
    cmd.arg("--config")
        .arg(config.as_os_str())
        .arg("harness-run")
        .arg("fake")
        .arg("--repo")
        .arg(repo.as_os_str())
        .arg("--task")
        .arg("тестовая задача");
    cmd
}

/// CONSTRAINTS.yaml с одним правилом `file_exists` уровня error.
fn constraints_yaml(path: &str) -> String {
    format!(
        "rules:\n  - name: spine_present\n    type: file_exists\n    path: \"{path}\"\n    severity: error\n"
    )
}

/// Репозиторий-фикстура с `.arch-handoff/CONSTRAINTS.yaml`.
fn repo_with_constraints(home: &Path, constraints: &str) -> PathBuf {
    let repo = home.join("repo");
    let handoff = repo.join(".arch-handoff");
    std::fs::create_dir_all(&handoff).expect("mkdir .arch-handoff");
    std::fs::write(handoff.join("CONSTRAINTS.yaml"), constraints).expect("запись CONSTRAINTS.yaml");
    repo
}

/// `arch-be init` в изолированном доме создаёт конфиг и ассеты; повторный
/// запуск не затирает пользовательские правки (F-7: идемпотентность init).
#[test]
fn init_creates_config_and_assets_and_preserves_user_edits() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let home = tmp.path();

    arch_cmd(home)
        .arg("init")
        .assert()
        .success()
        .stdout(contains("Инициализация завершена"));

    let config = home.join(".config/arch-harness/config.toml");
    let asset = home.join(".arch-harness/assets/prompts/architect.md");
    assert!(config.is_file(), "конфиг создан: {}", config.display());
    assert!(asset.is_file(), "ассет развёрнут: {}", asset.display());

    // Пользовательские правки: маркер в ассете и смена модели по умолчанию.
    let mut edited = std::fs::read_to_string(&asset).expect("read asset");
    edited.push_str("\n<!-- правка пользователя -->\n");
    std::fs::write(&asset, edited).expect("write asset");
    let cfg_text = std::fs::read_to_string(&config).expect("read config");
    let cfg_text = cfg_text.replace("default_model = \"deepseek\"", "default_model = \"glm\"");
    assert!(
        cfg_text.contains("default_model = \"glm\""),
        "правка конфига применилась до повторного init"
    );
    std::fs::write(&config, cfg_text).expect("write config");

    arch_cmd(home).arg("init").assert().success();

    let asset_after = std::fs::read_to_string(&asset).expect("re-read asset");
    assert!(
        asset_after.contains("<!-- правка пользователя -->"),
        "повторный init затёр правку ассета"
    );
    let cfg_after = std::fs::read_to_string(&config).expect("re-read config");
    assert!(
        cfg_after.contains("default_model = \"glm\""),
        "повторный init потерял пользовательскую модель:\n{cfg_after}"
    );
}

/// `arch-be control check` на падающем CONSTRAINTS.yaml (обязательный файл
/// отсутствует, severity error) → отчёт FAIL и exit 1 (скриптовый гейт
/// fitness-функций).
#[test]
fn control_check_failing_constraints_exits_1() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = repo_with_constraints(tmp.path(), &constraints_yaml("docs/ARCHITECTURE-SPINE.md"));

    let mut cmd = arch_cmd(tmp.path());
    cmd.arg("control").arg("check").arg(repo.as_os_str());
    cmd.assert()
        .code(1)
        .stdout(contains("Итог: FAIL"))
        .stdout(contains("spine_present"));
}

/// Проходящий CONSTRAINTS.yaml (обязательный файл на месте) → PASS, exit 0.
#[test]
fn control_check_passing_constraints_exits_0() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = repo_with_constraints(tmp.path(), &constraints_yaml("docs/ARCHITECTURE-SPINE.md"));
    let docs = repo.join("docs");
    std::fs::create_dir_all(&docs).expect("mkdir docs");
    std::fs::write(docs.join("ARCHITECTURE-SPINE.md"), "# Spine\n").expect("write spine");

    let mut cmd = arch_cmd(tmp.path());
    cmd.arg("control").arg("check").arg(repo.as_os_str());
    cmd.assert().success().stdout(contains("Итог: PASS"));
}

/// `arch-be control spine` на чистом spine-файле → exit 0.
#[test]
fn control_spine_clean_exits_0() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let spine = tmp.path().join("ARCHITECTURE-SPINE.md");
    std::fs::write(
        &spine,
        "# Spine\n\n## AD-1: Первый инвариант\n\n- **Binds**: a ↔ b\n\
         - **Prevents**: дрейф\n- **Rule**: правило. Страж: C-01.\n\
         - **Статус**: [ADOPTED]\n",
    )
    .expect("write spine");

    let mut cmd = arch_cmd(tmp.path());
    cmd.arg("control").arg("spine").arg(spine.as_os_str());
    cmd.assert().success().stdout(contains("нарушений нет"));
}

/// `arch-be control spine` с error-находкой (дубль AD-id) → exit 1
/// (скриптовый гейт spine-линтера в CI).
#[test]
fn control_spine_error_exits_1() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let spine = tmp.path().join("ARCHITECTURE-SPINE.md");
    std::fs::write(
        &spine,
        "# Spine\n\n## AD-1: Первый\n\n- **Binds**: a\n- **Prevents**: b\n- **Rule**: c.\n\n\
         ## AD-1: Дубль\n\n- **Binds**: a\n- **Prevents**: b\n- **Rule**: c.\n",
    )
    .expect("write spine");

    let mut cmd = arch_cmd(tmp.path());
    cmd.arg("control").arg("spine").arg(spine.as_os_str());
    cmd.assert().code(1).stdout(contains("dup_ad_id"));
}

/// `harness-run` со `status=blocked` в контракте → exit 2 (скриптовый гейт
/// в пайпах, см. `docs/harness_integrations.md`).
#[test]
#[cfg(feature = "harness")]
fn harness_run_blocked_contract_exits_2() {
    let tmp = tempfile::tempdir().expect("tempdir");
    harness_run_cmd(
        tmp.path(),
        r#"{"status": "blocked", "assumptions": [], "open_questions": ["нужен доступ к КШД"], "conflicts_with_prior_decisions": []}"#,
    )
    .assert()
    .code(2)
    .stdout(contains("status=blocked"));
}

/// `harness-run` с непустыми `conflicts_with_prior_decisions` → exit 3
/// (конфликт со spine останавливает интеграцию по контракту).
#[test]
#[cfg(feature = "harness")]
fn harness_run_conflicts_exit_3() {
    let tmp = tempfile::tempdir().expect("tempdir");
    harness_run_cmd(
        tmp.path(),
        r#"{"status": "complete", "conflicts_with_prior_decisions": ["AD-2 запрещает vendor lock-in"]}"#,
    )
    .assert()
    .code(3)
    .stdout(contains("conflicts=1"));
}

/// `harness-run` с чистым `complete` (списки пусты) → exit 0.
#[test]
#[cfg(feature = "harness")]
fn harness_run_complete_exits_0() {
    let tmp = tempfile::tempdir().expect("tempdir");
    harness_run_cmd(tmp.path(), r#"{"status": "complete"}"#)
        .assert()
        .success()
        .stdout(contains("status=complete"));
}

/// `arch-be mermaid` рендерит пример из репозитория без единого ключа
/// (no-LLM смоук из AGENTS.md).
#[test]
fn mermaid_renders_example_without_keys() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let diagram = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/mermaid/flow.mmd");
    let mut cmd = arch_cmd(tmp.path());
    cmd.arg("mermaid").arg(diagram.as_os_str());
    cmd.assert().success().stdout(contains("API Gateway"));
}

/// `arch-be doctor` без единого API-ключа в окружении не падает: отчёт
/// рендерится полностью, без паники и трейса ошибки в stderr. Код 1 —
/// задокументированный контракт (Fail «нет ключа модели по умолчанию»,
/// `src/doctor.rs`; отступление от буквы `DoD` ревью — ADR-005 §7).
/// Контракт полной сборки: в core отсутствие ключей — Warn, см. соседний тест.
#[test]
#[cfg(feature = "harness")]
fn doctor_without_keys_reports_problems_and_exits_1() {
    let tmp = tempfile::tempdir().expect("tempdir");
    arch_cmd(tmp.path())
        .arg("doctor")
        .assert()
        .code(1)
        .stdout(contains("arch-be doctor"))
        .stdout(contains("нет ключа модели по умолчанию"))
        .stdout(contains("Итог:"))
        .stderr(contains("Error:").not());
}

/// Core-сборка: сетевых LLM-провайдеров в бинаре нет, поэтому отсутствие
/// API-ключей — предупреждение, а не ошибка (план «Spine без собственной
/// LLM»: ключи живут у хоста, для судьи годится провайдер `kind = "cli"`).
#[test]
#[cfg(not(feature = "harness"))]
fn doctor_core_missing_keys_are_not_an_error() {
    let tmp = tempfile::tempdir().expect("tempdir");
    arch_cmd(tmp.path())
        .arg("doctor")
        .assert()
        .stdout(contains("arch-be doctor"))
        .stdout(contains("ключи не требуются"))
        .stdout(contains("Итог:"))
        .stdout(contains("нет ключа модели по умолчанию").not())
        .stderr(contains("Error:").not());
}

/// Синтетический кейс для `arch-be trace check`: `model/` с одним AD,
/// CONSTRAINTS.yaml, spine. `with_rule` — связывает AD с правилом C-001.
fn trace_case(home: &Path, with_rule: bool) -> PathBuf {
    let case = home.join("case");
    let model = case.join("model");
    std::fs::create_dir_all(&model).expect("mkdir model");
    let verified = if with_rule {
        "verified_by: [C-001]"
    } else {
        ""
    };
    std::fs::write(
        model.join("AD-1.md"),
        format!("---\nid: AD-1\ntype: ad\ntitle: Инвариант\nstatus: ADOPTED\n{verified}\n---\n"),
    )
    .expect("write AD");
    std::fs::write(
        case.join("CONSTRAINTS.yaml"),
        "constraints:\n  - id: C-001\n    name: правило\n",
    )
    .expect("write constraints");
    std::fs::write(case.join("ARCHITECTURE-SPINE.md"), "## AD-1: Инвариант\n")
        .expect("write spine");
    case
}

/// `arch-be trace check`: спайн покрыт правилом → PASS, exit 0 (ADR-006).
#[test]
fn trace_check_covered_spine_exits_0() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let case = trace_case(tmp.path(), true);
    let mut cmd = arch_cmd(tmp.path());
    cmd.arg("trace").arg("check").arg(case.as_os_str());
    cmd.assert()
        .success()
        .stdout(contains("AD → fitness-правило | 1/1 | 100%"))
        .stdout(contains("Итог: PASS"));
}

/// `arch-be trace check`: AD без правила и без `unverifiable` → FAIL, exit 1
/// (скриптовый гейт CI флота, ADR-006).
#[test]
fn trace_check_uncovered_ad_exits_1() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let case = trace_case(tmp.path(), false);
    let mut cmd = arch_cmd(tmp.path());
    cmd.arg("trace").arg("check").arg(case.as_os_str());
    cmd.assert()
        .code(1)
        .stdout(contains("ad-not-verified"))
        .stdout(contains("Итог: FAIL"));
}

/// Синтетический кейс для `arch-be nfr`: `model/` с INT-hop'ом и NFR с целью
/// p99. `hop_budget_ms` — бюджет hop'а (None — hop без бюджета).
fn nfr_budget_case(home: &Path, target_ms: u32, hop_budget_ms: Option<u32>) -> PathBuf {
    let case = home.join("nfr-case");
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
    case
}

/// `arch-be nfr budget`: сумма hop'ов в пределах цели p99 → PASS, exit 0 (ADR-007).
#[test]
fn nfr_budget_converging_exits_0() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let case = nfr_budget_case(tmp.path(), 2000, Some(800));
    let mut cmd = arch_cmd(tmp.path());
    cmd.arg("nfr").arg("budget").arg(case.as_os_str());
    cmd.assert()
        .success()
        .stdout(contains("резерв: 1200 мс"))
        .stdout(contains("Итог: PASS"));
}

/// `arch-be nfr budget`: сумма hop'ов выше цели p99 → error с виновными hop'ами,
/// exit 1 (`DoD` P1-1, скриптовый гейт).
#[test]
fn nfr_budget_exceeded_exits_1_with_guilty_hops() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let case = nfr_budget_case(tmp.path(), 2000, Some(3000));
    let mut cmd = arch_cmd(tmp.path());
    cmd.arg("nfr").arg("budget").arg(case.as_os_str());
    cmd.assert()
        .code(1)
        .stdout(contains("budget-exceeded"))
        .stdout(contains("INT-001=3000"))
        .stdout(contains("Итог: FAIL"));
}

/// `arch-be nfr budget`: hop без заявленного бюджета → error, exit 1.
#[test]
fn nfr_budget_missing_hop_budget_exits_1() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let case = nfr_budget_case(tmp.path(), 2000, None);
    let mut cmd = arch_cmd(tmp.path());
    cmd.arg("nfr").arg("budget").arg(case.as_os_str());
    cmd.assert()
        .code(1)
        .stdout(contains("budget-hop-missing"))
        .stdout(contains("INT-001"))
        .stdout(contains("Итог: FAIL"));
}

/// `arch-be run --max-turns 0` — лимит итераций не бывает нулевым
/// (`value_parser` range `1..`, код 2, без обращения к LLM).
#[test]
#[cfg(feature = "harness")]
fn run_max_turns_zero_rejected_exits_2() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let mut cmd = arch_cmd(tmp.path());
    cmd.arg("run").arg("--max-turns").arg("0").arg("пинг");
    cmd.assert().code(2).stderr(contains("--max-turns"));
}

/// git-репозиторий `home/<name>` с baseline-коммитом и handoff-пакетом
/// (MANIFEST.json + ROLLBACK.yaml; `{BASELINE}` в плане подменяется на хеш).
fn repo_with_rollback_plan(home: &Path, name: &str, route: &str, plan_yaml: &str) -> PathBuf {
    let repo = home.join(name);
    std::fs::create_dir_all(&repo).expect("mkdir repo");
    let git = |args: &[&str]| {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args(args)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .output()
            .expect("git");
        assert!(out.status.success(), "git {args:?}: {:?}", out.stderr);
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    git(&["init", "-q", "-b", "main"]);
    std::fs::write(repo.join("README.md"), "base\n").expect("readme");
    git(&["add", "."]);
    git(&["commit", "-q", "-m", "baseline"]);
    let baseline = git(&["rev-parse", "HEAD"]).trim().to_string();
    let packet = repo.join(".arch-handoff");
    std::fs::create_dir_all(&packet).expect("mkdir packet");
    std::fs::write(
        packet.join("MANIFEST.json"),
        format!("{{\"route\": \"{route}\"}}\n"),
    )
    .expect("manifest");
    std::fs::write(
        packet.join("ROLLBACK.yaml"),
        plan_yaml.replace("{BASELINE}", &baseline),
    )
    .expect("plan");
    repo
}

/// Безопасный план отката: проверка якоря + откат на baseline + verify.
const SAFE_PLAN: &str = "baseline_commit: \"{BASELINE}\"\n\
     steps:\n\
     \x20 - name: якорь-доступен\n\
     \x20   run: git cat-file -t {BASELINE}\n\
     \x20 - name: откат-на-baseline\n\
     \x20   run: git reset --hard {BASELINE}\n\
     verify: test -z \"$(git status --porcelain --untracked-files=no)\"\n";

/// `arch control gate A4 <repo> --rehearse`: безопасный план репетируется во
/// временном worktree → PASS, exit 0, evidence REHEARSAL.json записан.
#[test]
fn control_gate_a4_rehearse_pass_exits_0() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = repo_with_rollback_plan(tmp.path(), "repo-gate", "Critical", SAFE_PLAN);

    let mut cmd = arch_cmd(tmp.path());
    cmd.arg("control")
        .arg("gate")
        .arg("A4")
        .arg(repo.as_os_str())
        .arg("--rehearse");
    cmd.assert()
        .success()
        .stdout(contains("Репетиция отката"))
        .stdout(contains("отрепетирован"))
        .stdout(contains("Итог: PASS"));
    assert!(
        repo.join(".arch-handoff/REHEARSAL.json").is_file(),
        "evidence репетиции записан в пакет"
    );
}

/// Шаг с деструктивной командой (`rm -rf`) отклоняется с диагностикой →
/// FAIL, exit 1; основной репозиторий не тронут.
#[test]
fn control_gate_a4_refuses_destructive_step_exits_1() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let plan = "baseline_commit: \"{BASELINE}\"\n\
                steps:\n\
                \x20 - name: снести-всё\n\
                \x20   run: rm -rf README.md\n";
    let repo = repo_with_rollback_plan(tmp.path(), "repo-gate", "Critical", plan);

    let mut cmd = arch_cmd(tmp.path());
    cmd.arg("control")
        .arg("gate")
        .arg("A4")
        .arg(repo.as_os_str())
        .arg("--rehearse");
    cmd.assert()
        .code(1)
        .stdout(contains("REFUSED"))
        .stdout(contains("не репетируется"))
        .stdout(contains("Итог: FAIL"));
    assert!(repo.join("README.md").is_file(), "шаг не выполнялся");
}

/// Critical без успешной репетиции гейт A4 не проходит (exit 1); маршрут Fast
/// при дефолтном пороге — advisory (exit 0 без репетиции).
#[test]
fn control_gate_a4_requires_rehearsal_only_for_critical_by_default() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = repo_with_rollback_plan(tmp.path(), "repo-gate", "Critical", SAFE_PLAN);
    let mut cmd = arch_cmd(tmp.path());
    cmd.arg("control")
        .arg("gate")
        .arg("A4")
        .arg(repo.as_os_str());
    cmd.assert()
        .code(1)
        .stdout(contains("--rehearse"))
        .stdout(contains("Итог: FAIL"));

    let fast2 = repo_with_rollback_plan(tmp.path(), "repo-gate-fast", "Fast", SAFE_PLAN);
    let mut cmd = arch_cmd(tmp.path());
    cmd.arg("control")
        .arg("gate")
        .arg("A4")
        .arg(fast2.as_os_str());
    cmd.assert()
        .success()
        .stdout(contains("не обязательна"))
        .stdout(contains("Итог: PASS"));
}

/// Нереализованный гейт — понятная ошибка.
#[test]
fn control_gate_unknown_gate_errors() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = repo_with_rollback_plan(tmp.path(), "repo-gate", "Critical", SAFE_PLAN);
    let mut cmd = arch_cmd(tmp.path());
    cmd.arg("control")
        .arg("gate")
        .arg("A3")
        .arg(repo.as_os_str());
    cmd.assert().failure().stderr(contains("только A4"));
}

/// `arch eval run`: встроенный сьют agent-config прогоняется герметично
/// (временный дом, без ключей и сети), гейт 100% проходит, JSON-отчёт
/// пишется в `<дом>/evals/` (docs/evals.md).
#[test]
#[cfg(feature = "harness")]
fn eval_builtin_suite_passes_offline_and_writes_report() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let mut cmd = arch_cmd(tmp.path());
    cmd.arg("eval").arg("run");
    cmd.assert()
        .success()
        .stdout(contains("Eval-сьют 'agent-config': задач 8, пропущено 0"))
        .stdout(contains("✓ mermaid-render"))
        .stdout(contains("✓ policy-denies-destructive"))
        .stdout(contains("Pass-rate: 100.0% (8/8), гейт 100.0% — PASS"));

    // JSON-отчёт: <tempdir>/.arch-harness/evals/eval-agent-config-*.json.
    let evals = tmp.path().join(".arch-harness/evals");
    let reports: Vec<PathBuf> = std::fs::read_dir(&evals)
        .expect("каталог evals создан")
        .flatten()
        .map(|e| e.path())
        .collect();
    assert_eq!(reports.len(), 1, "ровно один отчёт: {reports:?}");
    let text = std::fs::read_to_string(&reports[0]).expect("read json");
    assert!(text.contains("\"gate_passed\": true"), "{text}");
    assert!(text.contains("\"pass_rate\": 100.0"), "{text}");
}

/// Гейт ломает код выхода: пользовательский сьют с заведомо красной
/// задачей → exit 1 (регрессионный контракт для CI).
#[test]
#[cfg(feature = "harness")]
fn eval_gate_below_pass_rate_exits_1() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let suite = tmp.path().join("suite");
    std::fs::create_dir_all(&suite).expect("mkdir");
    std::fs::write(
        suite.join("01-red.yaml"),
        "id: red-task\ntitle: Заведомо красная задача\ncommand: \"echo тихо\"\nchecks:\n  - type: must_contain\n    pattern: \"отсутствует\"\n",
    )
    .expect("write suite");
    std::fs::write(
        suite.join("02-green.yaml"),
        "id: green-task\ntitle: Зелёная задача\ncommand: \"echo тихо\"\nchecks:\n  - type: must_contain\n    pattern: \"тихо\"\n",
    )
    .expect("write suite");
    let mut cmd = arch_cmd(tmp.path());
    cmd.arg("eval")
        .arg("run")
        .arg("--suite")
        .arg(suite.as_os_str());
    cmd.assert()
        .code(1)
        .stdout(contains("✗ red-task"))
        .stdout(contains("Pass-rate: 50.0% (1/2), гейт 100.0% — FAIL"));

    // Пониженный гейт пропускает тот же сьют.
    let mut cmd = arch_cmd(tmp.path());
    cmd.arg("eval")
        .arg("run")
        .arg("--suite")
        .arg(suite.as_os_str())
        .arg("--gate")
        .arg("50");
    cmd.assert().success().stdout(contains("гейт 50.0% — PASS"));
}

/// `arch-be connect claude --dir <проект>` одной командой раскладывает
/// MCP-конфиг (.mcp.json), хуки (.claude/settings.json), скиллы
/// (.claude/skills/) и CLAUDE.md; повторный запуск идемпотентен
/// («Без изменений», без дублей хуков).
#[test]
fn connect_claude_scaffolds_host_integration() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let proj = tmp.path().join("proj");
    std::fs::create_dir_all(&proj).expect("mkdir proj");

    let mut cmd = arch_cmd(tmp.path());
    cmd.arg("connect")
        .arg("claude")
        .arg("--dir")
        .arg(proj.as_os_str());
    cmd.assert()
        .success()
        .stdout(contains("Подключение Spine к хосту «claude»"))
        .stdout(contains("claude mcp list"));

    assert!(proj.join(".mcp.json").is_file(), ".mcp.json создан");
    assert!(
        proj.join(".claude/settings.json").is_file(),
        "settings.json с хуками создан"
    );
    assert!(proj.join("CLAUDE.md").is_file(), "CLAUDE.md создан");
    assert!(
        proj.join(".claude/skills/adr-authoring/SKILL.md").is_file(),
        "скиллы разложены"
    );

    let mut again = arch_cmd(tmp.path());
    again
        .arg("connect")
        .arg("claude")
        .arg("--dir")
        .arg(proj.as_os_str());
    again.assert().success().stdout(contains("Без изменений"));
}

/// `arch-be connect generic --dry-run` — только печать сниппетов: ни одного
/// файла в проекте не появляется.
#[test]
fn connect_generic_dry_run_writes_nothing() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let proj = tmp.path().join("proj");
    std::fs::create_dir_all(&proj).expect("mkdir proj");

    let mut cmd = arch_cmd(tmp.path());
    cmd.arg("connect")
        .arg("generic")
        .arg("--dir")
        .arg(proj.as_os_str())
        .arg("--dry-run");
    cmd.assert()
        .success()
        .stdout(contains("dry-run"))
        .stdout(contains("mcpServers"));
    assert_eq!(
        std::fs::read_dir(&proj).expect("read dir").count(),
        0,
        "dry-run ничего не записал"
    );
}

/// Неизвестный хост — понятная ошибка (список допустимых в stderr).
#[test]
fn connect_unknown_host_errors() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let mut cmd = arch_cmd(tmp.path());
    cmd.arg("connect").arg("cursor");
    cmd.assert()
        .failure()
        .stderr(contains("неизвестный хост"))
        .stderr(contains("claude"));
}

/// `arch-be connect kimi --dir <проект>` пишет проектный
/// `.kimi-code/mcp.json` (без поля cwd), печатает user-level сниппет,
/// TOML-блок хука и напоминание про trust-диалог; повторный запуск
/// идемпотентен; `--rw` добавляет флаг в args.
#[test]
fn connect_kimi_writes_project_mcp_json() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let proj = tmp.path().join("proj");
    std::fs::create_dir_all(&proj).expect("mkdir proj");

    let mut cmd = arch_cmd(tmp.path());
    cmd.arg("connect")
        .arg("kimi")
        .arg("--dir")
        .arg(proj.as_os_str());
    cmd.assert()
        .success()
        .stdout(contains("Подключение Spine к хосту «kimi»"))
        .stdout(contains("~/.kimi-code/mcp.json"))
        .stdout(contains("[[hooks]]"))
        .stdout(contains("~/.kimi-code/config.toml"))
        .stdout(contains("trust"));

    let mcp_path = proj.join(".kimi-code/mcp.json");
    let text = std::fs::read_to_string(&mcp_path).expect("read mcp.json");
    assert!(text.contains("\"command\": \"arch-be\""), "{text}");
    assert!(
        !text.contains("\"cwd\""),
        "поле cwd не пишем — сервер наследует cwd харнесса: {text}"
    );

    // Повторный запуск — «Без изменений», дублей нет.
    let mut again = arch_cmd(tmp.path());
    again
        .arg("connect")
        .arg("kimi")
        .arg("--dir")
        .arg(proj.as_os_str());
    again.assert().success().stdout(contains("Без изменений"));
    assert_eq!(
        std::fs::read_to_string(&mcp_path).expect("read"),
        text,
        "повторный запуск изменил файл"
    );

    // --rw добавляет флаг.
    let mut rw = arch_cmd(tmp.path());
    rw.arg("connect")
        .arg("kimi")
        .arg("--dir")
        .arg(proj.as_os_str())
        .arg("--rw");
    rw.assert().success();
    let text = std::fs::read_to_string(&mcp_path).expect("read");
    assert!(text.contains("\"--rw\""), "{text}");
}

/// `arch-be connect omp --dir <проект>` пишет `.mcp.json` и раскладывает
/// встроенные скиллы в `.claude/skills/` (omp читает его нативно); повтор —
/// «Без изменений»; `--dry-run` ничего не записывает.
#[test]
fn connect_omp_writes_mcp_json_and_skills() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let proj = tmp.path().join("proj");
    std::fs::create_dir_all(&proj).expect("mkdir proj");

    let mut cmd = arch_cmd(tmp.path());
    cmd.arg("connect")
        .arg("omp")
        .arg("--dir")
        .arg(proj.as_os_str());
    cmd.assert()
        .success()
        .stdout(contains("Подключение Spine к хосту «omp»"))
        .stdout(contains("omp --hook"));

    assert!(proj.join(".mcp.json").is_file(), ".mcp.json создан");
    assert!(
        proj.join(".claude/skills/adr-authoring/SKILL.md").is_file(),
        "скиллы разложены"
    );

    let mut again = arch_cmd(tmp.path());
    again
        .arg("connect")
        .arg("omp")
        .arg("--dir")
        .arg(proj.as_os_str());
    again.assert().success().stdout(contains("Без изменений"));

    // --dry-run на новом каталоге: ничего не появляется.
    let proj2 = tmp.path().join("proj2");
    std::fs::create_dir_all(&proj2).expect("mkdir proj2");
    let mut dry = arch_cmd(tmp.path());
    dry.arg("connect")
        .arg("omp")
        .arg("--dir")
        .arg(proj2.as_os_str())
        .arg("--dry-run");
    dry.assert().success().stdout(contains("dry-run"));
    assert_eq!(
        std::fs::read_dir(&proj2).expect("read dir").count(),
        0,
        "dry-run ничего не записал"
    );
}
