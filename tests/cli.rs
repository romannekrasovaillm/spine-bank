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

/// Карточный контекст правила (`ad`/`fix_hint`/`skill` из CONSTRAINTS.yaml) виден
/// в текстовом выводе (строкой-отступом) и в `--json` (аддитивные поля).
#[test]
fn control_check_shows_card_context_in_text_and_json() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = repo_with_constraints(
        tmp.path(),
        "rules:\n  - name: spine_present\n    type: file_exists\n    path: \"docs/ARCHITECTURE-SPINE.md\"\n    severity: error\n    ad: AD-9\n    rationale: \"гейт, а не документация задним числом\"\n    fix_hint: \"вернуть spine на место\"\n    skill: spine-invariants\n",
    );

    let mut cmd = arch_cmd(tmp.path());
    cmd.arg("control").arg("check").arg(repo.as_os_str());
    cmd.assert()
        .code(1)
        .stdout(contains("spine_present"))
        .stdout(contains("↳ AD-9"))
        .stdout(contains("как чинить: вернуть spine на место"))
        .stdout(contains("скилл: spine-invariants"));

    let mut cmd = arch_cmd(tmp.path());
    cmd.arg("control")
        .arg("check")
        .arg(repo.as_os_str())
        .arg("--json");
    let output = cmd.assert().code(1).get_output().clone();
    let v: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("--json печатает JSON даже при exit 1");
    let issue = &v["issues"][0];
    assert_eq!(issue["ad"], "AD-9");
    assert_eq!(issue["fix_hint"], "вернуть spine на место");
    assert_eq!(issue["skill"], "spine-invariants");
    assert_eq!(issue["rationale"], "гейт, а не документация задним числом");
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

/// git в каталоге с тестовой идентичностью коммиттера (образец —
/// `src/delta.rs::make_guard_repo`).
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

/// git-репозиторий с `.arch-handoff/CONSTRAINTS.yaml` (два error-правила) и
/// обязательным файлом; один коммит.
fn gate_repo(home: &Path) -> PathBuf {
    let repo = home.join("gate-repo");
    std::fs::create_dir_all(repo.join(".arch-handoff")).expect("mkdir .arch-handoff");
    std::fs::write(
        repo.join(".arch-handoff/CONSTRAINTS.yaml"),
        "rules:\n  - name: spine_present\n    type: file_exists\n    path: \"ARCHITECTURE-SPINE.md\"\n    severity: error\n  - name: no_pan\n    type: must_not_contain\n    glob: \"**/*.py\"\n    pattern: 'PAN'\n    severity: error\n",
    )
    .expect("запись CONSTRAINTS.yaml");
    std::fs::write(repo.join("ARCHITECTURE-SPINE.md"), "# Spine\n").expect("write spine");
    git(&repo, &["init", "-q"]);
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-q", "-m", "init"]);
    repo
}

/// Единый гейт `arch-be gate` на чистом репозитории: все составляющие
/// PASS/SKIP, exit 0 (маршрут auto из пустого диффа → Fast).
#[test]
fn gate_clean_repo_exits_0() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = gate_repo(tmp.path());

    let mut cmd = arch_cmd(tmp.path());
    cmd.arg("gate").arg("--repo").arg(repo.as_os_str());
    cmd.assert()
        .success()
        .stdout(contains("Маршрут: Fast (auto"))
        .stdout(contains("[PASS] fitness"))
        .stdout(contains("[PASS] rule_weakened"))
        .stdout(contains("[SKIP] trace_check"))
        .stdout(contains("Итог: PASS"));
}

/// Анти-ослабление: агент удалил правило `no_pan`, чтобы пройти гейт —
/// `arch-be gate` падает exit 1 с находкой `rule_weakened` (бэклог п.4:
/// детекция по коду возврата, не по строкам).
#[test]
fn gate_fails_when_agent_removes_rule_to_pass() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = gate_repo(tmp.path());
    // «Позеленение»: правило no_pan удалено из реестра.
    std::fs::write(
        repo.join(".arch-handoff/CONSTRAINTS.yaml"),
        "rules:\n  - name: spine_present\n    type: file_exists\n    path: \"ARCHITECTURE-SPINE.md\"\n    severity: error\n",
    )
    .expect("ослабленный CONSTRAINTS.yaml");

    let mut cmd = arch_cmd(tmp.path());
    cmd.arg("gate").arg("--repo").arg(repo.as_os_str());
    cmd.assert()
        .code(1)
        .stdout(contains("[FAIL] rule_weakened"))
        .stdout(contains("no_pan"))
        .stdout(contains("удалено из реестра"))
        .stdout(contains("Итог: FAIL"));
}

/// Ослабление, узаконенное активным override (ADR + срок), гейт пропускает;
/// дельта покрывает правку для delta guard.
#[test]
fn gate_override_legalizes_weakening_exits_0() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = gate_repo(tmp.path());
    std::fs::write(
        repo.join(".arch-handoff/CONSTRAINTS.yaml"),
        "rules:\n  - name: spine_present\n    type: file_exists\n    path: \"ARCHITECTURE-SPINE.md\"\n    severity: error\noverrides:\n  - rule: no_pan\n    adr: ADR-007\n    until: \"2999-01\"\n",
    )
    .expect("CONSTRAINTS.yaml с override");
    let delta = repo.join("changes/drop-pan");
    std::fs::create_dir_all(&delta).expect("mkdir delta");
    std::fs::write(
        delta.join("DELTA.md"),
        "# Дельта\n\nСнимаем no_pan по ADR-007 (правка CONSTRAINTS.yaml).\n",
    )
    .expect("DELTA.md");

    let mut cmd = arch_cmd(tmp.path());
    cmd.arg("gate").arg("--repo").arg(repo.as_os_str());
    cmd.assert()
        .success()
        .stdout(contains("[PASS] rule_weakened"))
        .stdout(contains("Итог: PASS"));
}

/// Fail-soft: каталог без git и без ограничений — составляющие SKIP, exit 0
/// (контракт хуков `arch-be connect`: нет входа — гейт молча пропускается).
#[test]
fn gate_without_git_and_constraints_is_skip_and_exits_0() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let plain = tmp.path().join("plain");
    std::fs::create_dir_all(&plain).expect("mkdir plain");

    let mut cmd = arch_cmd(tmp.path());
    cmd.arg("gate").arg("--repo").arg(plain.as_os_str());
    cmd.assert()
        .success()
        .stdout(contains("[SKIP] fitness"))
        .stdout(contains("[SKIP] delta_guard"))
        .stdout(contains("[SKIP] rule_weakened"))
        .stdout(contains("fail-safe маршрут Critical"))
        .stdout(contains("Итог: PASS"));
}

/// Явный `--route standard` добавляет составляющие nfr/evidence (здесь —
/// SKIP за неимением model/ и бандлов); неизвестный маршрут — ошибка clap.
#[test]
fn gate_explicit_route_adds_standard_components() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = gate_repo(tmp.path());

    let mut cmd = arch_cmd(tmp.path());
    cmd.arg("gate")
        .arg("--repo")
        .arg(repo.as_os_str())
        .arg("--route")
        .arg("standard");
    cmd.assert()
        .success()
        .stdout(contains("Маршрут: Standard (явный --route"))
        .stdout(contains("[SKIP] nfr"))
        .stdout(contains("[SKIP] evidence_verify"));

    let mut cmd = arch_cmd(tmp.path());
    cmd.arg("gate")
        .arg("--repo")
        .arg(repo.as_os_str())
        .arg("--route")
        .arg("ludicrous");
    cmd.assert()
        .failure()
        .stderr(contains("неизвестный маршрут"));
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

// --- Baseline / ratchet для brownfield (бэклог волны 2, п.6) ----------------

/// CONSTRAINTS.yaml legacy-фикстуры: файловое правило `no_pan` (PAN в
/// python-файлах, с owner) и глобальное `arch_doc` (обязательный файл) —
/// оба уровня error, старт репозитория 100% красный.
const LEGACY_CONSTRAINTS: &str = "rules:\n  - name: no_pan\n    type: must_not_contain\n    glob: \"src/**/*.py\"\n    pattern: '\\b\\d{16}\\b'\n    severity: error\n    owner: \"владелец legacy\"\n  - name: arch_doc\n    type: file_exists\n    path: \"docs/ARCH.md\"\n    severity: error\n";

/// Legacy-репозиторий со 100% красным стартом: два файла с PAN
/// (2 находки `no_pan`) и отсутствующий docs/ARCH.md (1 находка `arch_doc`).
fn legacy_repo(home: &Path) -> PathBuf {
    let repo = home.join("legacy");
    std::fs::create_dir_all(repo.join(".arch-handoff")).expect("mkdir .arch-handoff");
    std::fs::write(
        repo.join(".arch-handoff/CONSTRAINTS.yaml"),
        LEGACY_CONSTRAINTS,
    )
    .expect("запись CONSTRAINTS.yaml");
    std::fs::create_dir_all(repo.join("src")).expect("mkdir src");
    std::fs::write(repo.join("src/a.py"), "pan = \"4276550012345678\"\n").expect("a.py");
    std::fs::write(repo.join("src/b.py"), "card = \"4276550099990001\"\n").expect("b.py");
    repo
}

/// Ratchet-жизненный цикл на legacy-репозитории (приёмка п.6): красный старт
/// → фиксация baseline → зелёный гейт → новое нарушение FAIL → отказ
/// обновления при выросшем долге → исправление старого уменьшает baseline.
#[test]
fn control_check_baseline_ratchet_lifecycle() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = legacy_repo(tmp.path());
    let baseline = repo.join(".arch-handoff/baseline.json");

    // 1. Старт красный: 3 error-находки, exit 1.
    let mut cmd = arch_cmd(tmp.path());
    cmd.arg("control").arg("check").arg(repo.as_os_str());
    cmd.assert().code(1).stdout(contains("Итог: FAIL"));

    // 2. Фиксация baseline: exit 0, файл записан, долг 3 находки по 2 правилам.
    let mut cmd = arch_cmd(tmp.path());
    cmd.arg("control")
        .arg("check")
        .arg(repo.as_os_str())
        .arg("--baseline")
        .arg(baseline.as_os_str())
        .arg("--baseline-update");
    cmd.assert()
        .success()
        .stdout(contains("Baseline обновлён"))
        .stdout(contains("Итог: PASS"));
    let v: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&baseline).expect("baseline записан"))
            .expect("baseline — JSON");
    let rules = v["rules"].as_array().expect("rules");
    let no_pan = rules
        .iter()
        .find(|r| r["name"] == "no_pan")
        .expect("no_pan в baseline");
    assert_eq!(no_pan["count"], 2);
    assert_eq!(no_pan["owner"], "владелец legacy");

    // 3. Прогон с baseline — зелёный; долг виден по правилам и владельцам.
    let mut cmd = arch_cmd(tmp.path());
    cmd.arg("control")
        .arg("check")
        .arg(repo.as_os_str())
        .arg("--baseline")
        .arg(baseline.as_os_str());
    cmd.assert()
        .success()
        .stdout(contains(
            "долг: no_pan — 2 находок (owner: владелец legacy)",
        ))
        .stdout(contains("долг: arch_doc — 1 находок"))
        .stdout(contains("Итог: PASS"));

    // 3b. --json: долг — в аддитивной секции baseline, issues пуст.
    let mut cmd = arch_cmd(tmp.path());
    cmd.arg("control")
        .arg("check")
        .arg(repo.as_os_str())
        .arg("--baseline")
        .arg(baseline.as_os_str())
        .arg("--json");
    let output = cmd.assert().success().get_output().clone();
    let v: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("--json печатает JSON");
    assert_eq!(v["passed"], true);
    assert_eq!(v["issues"].as_array().expect("issues").len(), 0);
    assert_eq!(v["baseline"]["debt_total"], 3);
    assert_eq!(v["baseline"]["updated"], false);

    // 4. Новое нарушение — гейт FAIL, несмотря на baseline.
    std::fs::write(repo.join("src/c.py"), "pan2 = \"4276550011112222\"\n").expect("c.py");
    let mut cmd = arch_cmd(tmp.path());
    cmd.arg("control")
        .arg("check")
        .arg(repo.as_os_str())
        .arg("--baseline")
        .arg(baseline.as_os_str());
    cmd.assert()
        .code(1)
        .stdout(contains("src/c.py"))
        .stdout(contains("Итог: FAIL"));

    // 5. Обновление при выросшем долге — отказ, baseline не тронут.
    let before = std::fs::read(&baseline).expect("baseline до отказа");
    let mut cmd = arch_cmd(tmp.path());
    cmd.arg("control")
        .arg("check")
        .arg(repo.as_os_str())
        .arg("--baseline")
        .arg(baseline.as_os_str())
        .arg("--baseline-update");
    cmd.assert()
        .failure()
        .stderr(contains("обновление baseline отклонено"))
        .stderr(contains("no_pan"));
    let after = std::fs::read(&baseline).expect("baseline после отказа");
    assert_eq!(before, after, "baseline не перезаписан при отказе");

    // 6. Убираем новое (c.py) и одно старое (b.py) нарушение: обновление
    // принимается, baseline убывает (no_pan 2 → 1).
    std::fs::remove_file(repo.join("src/c.py")).expect("rm c.py");
    std::fs::remove_file(repo.join("src/b.py")).expect("rm b.py");
    let mut cmd = arch_cmd(tmp.path());
    cmd.arg("control")
        .arg("check")
        .arg(repo.as_os_str())
        .arg("--baseline")
        .arg(baseline.as_os_str())
        .arg("--baseline-update");
    cmd.assert().success().stdout(contains("Итог: PASS"));
    let v: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&baseline).expect("baseline")).expect("JSON");
    let no_pan = v["rules"]
        .as_array()
        .expect("rules")
        .iter()
        .find(|r| r["name"] == "no_pan")
        .expect("no_pan");
    assert_eq!(no_pan["count"], 1, "baseline убыл после исправления");
}

/// Режим `--changed-since`: проверяются только затронутые файлы (изменённые
/// против ref + untracked), глобальные правила пропускаются; нетронутый файл
/// с долгом остаётся долгом, новое нарушение в untracked-файле — FAIL.
#[test]
fn control_check_changed_since_scopes_check_to_touched_files() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = legacy_repo(tmp.path());
    git(&repo, &["init", "-q"]);
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-q", "-m", "init"]);
    let baseline = repo.join(".arch-handoff/baseline.json");

    // Фиксация baseline на полном прогоне (долг: no_pan ×2, arch_doc ×1);
    // baseline.json коммитим, чтобы сам не попадал в срез untracked.
    let mut cmd = arch_cmd(tmp.path());
    cmd.arg("control")
        .arg("check")
        .arg(repo.as_os_str())
        .arg("--baseline")
        .arg(baseline.as_os_str())
        .arg("--baseline-update");
    cmd.assert().success();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-q", "-m", "baseline"]);

    // Чистое дерево: срез пуст — файловые правила тоже пропускаются.
    let mut cmd = arch_cmd(tmp.path());
    cmd.arg("control")
        .arg("check")
        .arg(repo.as_os_str())
        .arg("--baseline")
        .arg(baseline.as_os_str())
        .arg("--changed-since")
        .arg("HEAD");
    cmd.assert()
        .success()
        .stdout(contains("изменённых файлов 0"))
        .stdout(contains("skip: no_pan"))
        .stdout(contains("skip: arch_doc"))
        .stdout(contains("Итог: PASS"));

    // Правка a.py строкой-комментарием сверху (сдвиг строки, сниппет PAN не
    // изменился) — находка остаётся долгом: отпечаток не содержит строку.
    std::fs::write(
        repo.join("src/a.py"),
        "# touched\npan = \"4276550012345678\"\n",
    )
    .expect("a.py v2");
    let mut cmd = arch_cmd(tmp.path());
    cmd.arg("control")
        .arg("check")
        .arg(repo.as_os_str())
        .arg("--baseline")
        .arg(baseline.as_os_str())
        .arg("--changed-since")
        .arg("HEAD");
    cmd.assert()
        .success()
        .stdout(contains("изменённых файлов 1"))
        .stdout(contains("долг: no_pan — 1 находок"))
        .stdout(contains("skip: arch_doc"))
        .stdout(contains("Итог: PASS"));

    // Новое нарушение в untracked-файле ловится и в режиме среза.
    std::fs::write(repo.join("src/new.py"), "pan = \"4276550022223333\"\n").expect("new.py");
    let mut cmd = arch_cmd(tmp.path());
    cmd.arg("control")
        .arg("check")
        .arg(repo.as_os_str())
        .arg("--baseline")
        .arg(baseline.as_os_str())
        .arg("--changed-since")
        .arg("HEAD");
    cmd.assert()
        .code(1)
        .stdout(contains("src/new.py"))
        .stdout(contains("Итог: FAIL"));

    // --baseline-update с --changed-since — отказ (срез уничтожил бы долг).
    let mut cmd = arch_cmd(tmp.path());
    cmd.arg("control")
        .arg("check")
        .arg(repo.as_os_str())
        .arg("--baseline")
        .arg(baseline.as_os_str())
        .arg("--baseline-update")
        .arg("--changed-since")
        .arg("HEAD");
    cmd.assert().failure().stderr(contains("несовместим"));
}

/// Baseline-файл обязан существовать для ratchet-прогона (fail-closed):
/// отсутствующий файл — понятная ошибка с подсказкой про --baseline-update.
#[test]
fn control_check_baseline_missing_file_errors_with_hint() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = legacy_repo(tmp.path());
    let mut cmd = arch_cmd(tmp.path());
    cmd.arg("control")
        .arg("check")
        .arg(repo.as_os_str())
        .arg("--baseline")
        .arg(repo.join(".arch-handoff/baseline.json").as_os_str());
    cmd.assert()
        .failure()
        .stderr(contains("baseline: файл не найден"))
        .stderr(contains("--baseline-update"));
}

/// `control fp mark` пишет регистр ложных срабатываний: файл
/// `evidence/fp-register.md` создаётся с шапкой таблицы, пометка — строкой
/// с датой/правилом/файлом/примечанием (пункт 9, docs/outcome-metrics.md §2).
#[test]
fn control_fp_mark_appends_to_register() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let mut cmd = arch_cmd(tmp.path());
    cmd.arg("control")
        .arg("fp")
        .arg("mark")
        .arg("no-pan")
        .arg("src/a.rs:10")
        .arg("--note")
        .arg("crate::… в строковом литерале");
    cmd.assert()
        .success()
        .stdout(contains("Пометка FP записана"));

    let register = tmp.path().join("evidence/fp-register.md");
    let text = std::fs::read_to_string(&register).expect("регистр создан");
    assert!(
        text.contains("| Дата | Правило | Файл | Примечание |"),
        "шапка таблицы: {text}"
    );
    assert!(
        text.contains("| no-pan | src/a.rs:10 | crate::… в строковом литерале |"),
        "строка пометки: {text}"
    );
    // Вторая пометка — append, шапка не дублируется.
    let mut cmd = arch_cmd(tmp.path());
    cmd.arg("control")
        .arg("fp")
        .arg("mark")
        .arg("msrv")
        .arg("Cargo.toml");
    cmd.assert().success();
    let text = std::fs::read_to_string(&register).expect("регистр на месте");
    assert_eq!(text.matches("| Дата |").count(), 1, "шапка одна: {text}");
    assert!(text.contains("| msrv | Cargo.toml | — |"), "{text}");
}

/// `arch-be digest` на фикстуре журнала: итерации FAIL→PASS, топ правил,
/// истекающие overrides; `--json` — машиночитаемый контракт (пункт 9).
#[test]
fn digest_reads_journal_and_register() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let handoff = tmp.path().join(".arch-handoff");
    std::fs::create_dir_all(&handoff).expect("mkdir .arch-handoff");
    // Штампы «сейчас» (дайджест в бинаре берёт Local::now): fail → pass.
    let ts = chrono::Local::now().to_rfc3339();
    std::fs::write(
        handoff.join("mcp-calls.jsonl"),
        format!(
            "{{\"ts\":\"{ts}\",\"tool\":\"fitness_check\",\"verdict\":\"fail\",\"duration_ms\":7,\"rules\":[\"no-pan\"]}}\n\
             {{\"ts\":\"{ts}\",\"tool\":\"fitness_check\",\"verdict\":\"pass\",\"duration_ms\":5}}\n"
        ),
    )
    .expect("журнал-фикстура");
    // Override истекает завтра — должен попасть в дайджест.
    let tomorrow = (chrono::Local::now() + chrono::Duration::days(1))
        .format("%Y-%m-%d")
        .to_string();
    std::fs::write(
        handoff.join("CONSTRAINTS.yaml"),
        format!(
            "rules:\n  - name: no-pan\n    type: must_not_contain\n    glob: \"src/**\"\n    pattern: 'PAN'\n    severity: error\n\
             overrides:\n  - rule: no-pan\n    adr: ADR-001\n    until: {tomorrow}\n"
        ),
    )
    .expect("constraints");

    let mut cmd = arch_cmd(tmp.path());
    cmd.arg("digest");
    cmd.assert()
        .success()
        .stdout(contains("## Итерации FAIL→PASS"))
        .stdout(contains("fitness_check: 1"))
        .stdout(contains("no-pan: 1"))
        .stdout(contains("истекает через 1 дн."));

    // Машиночитаемый контракт.
    let mut cmd = arch_cmd(tmp.path());
    cmd.arg("digest").arg("--json");
    let out = cmd.assert().success();
    let json: serde_json::Value =
        serde_json::from_slice(&out.get_output().stdout).expect("stdout — валидный JSON");
    assert_eq!(json["calls_total"], 2);
    assert_eq!(json["fail_pass_iterations"]["fitness_check"], 1);
    assert_eq!(
        json["top_failed_rules"][0],
        serde_json::json!(["no-pan", 1])
    );
    assert_eq!(json["expiring"][0]["rule"], "no-pan");
}

/// Кейс-фикстура для составного ревью и радиуса изменения (бэклог волны 3,
/// п.13): git-репо (один коммит), model/ с цепочкой CMP→INT→AD→OWNER,
/// корневой CONSTRAINTS.yaml (карточка C-001 с владельцем), spine,
/// контракт contracts/api.yaml и .arch-handoff/CONSTRAINTS.yaml.
fn review_case(home: &Path) -> PathBuf {
    let case = home.join("review-case");
    let model = case.join("model");
    std::fs::create_dir_all(&model).expect("mkdir model");
    for (name, fm) in [
        (
            "AD-1.md",
            "---\nid: AD-1\ntype: ad\ntitle: Точные деньги\nstatus: ADOPTED\nverified_by: [C-001]\n---\n\nПравило.\n",
        ),
        (
            "CMP-001.md",
            "---\nid: CMP-001\ntype: cmp\ntitle: Платёжный шлюз\nstatus: designed\nimplements: [AD-1]\ndepends_on: [INT-001]\naffects: [OWNER-1]\ncode_roots: [services/pay]\n---\n\nТело.\n",
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
        std::fs::write(model.join(name), fm).expect("сущность");
    }
    std::fs::write(
        case.join("CONSTRAINTS.yaml"),
        "constraints:\n  - id: C-001\n    name: no_float_money\n    owner: Команда платежей\n",
    )
    .expect("constraints");
    std::fs::write(
        case.join("ARCHITECTURE-SPINE.md"),
        "# Spine\n\n### AD-1. Точные деньги\n- Binds: денежные суммы\n- Prevents: потеря копеек\n- Rule: суммы в minor units\n",
    )
    .expect("spine");
    std::fs::create_dir_all(case.join("contracts")).expect("mkdir contracts");
    std::fs::write(
        case.join("contracts/api.yaml"),
        "openapi: 3.0.3\ninfo:\n  title: Processing API\n  version: 1.0.0\npaths:\n  /v1/charges:\n    get:\n      operationId: listCharges\n      responses:\n        '200':\n          description: ok\n",
    )
    .expect("контракт");
    std::fs::create_dir_all(case.join(".arch-handoff")).expect("mkdir handoff");
    std::fs::write(
        case.join(".arch-handoff/CONSTRAINTS.yaml"),
        "rules:\n  - name: spine_present\n    type: file_exists\n    path: \"ARCHITECTURE-SPINE.md\"\n    severity: error\n",
    )
    .expect("handoff constraints");
    git(&case, &["init", "-q"]);
    git(&case, &["add", "."]);
    git(&case, &["commit", "-q", "-m", "init"]);
    case
}

/// `arch-be review <dir>`: единое ревью одной командой — все секции
/// PASS, exit 0; `--json` — валидный JSON-вердикт с составом секций.
#[test]
fn review_clean_case_exits_0_text_and_json() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let case = review_case(tmp.path());

    let mut cmd = arch_cmd(tmp.path());
    cmd.arg("review").arg(case.as_os_str());
    cmd.assert()
        .success()
        .stdout(contains("Ревью:"))
        .stdout(contains("Маршрут: Fast (auto"))
        .stdout(contains("[PASS] fitness"))
        .stdout(contains("[PASS] trace_check"))
        .stdout(contains("[PASS] model_validate"))
        .stdout(contains("[PASS] contracts"))
        .stdout(contains("Итог: PASS"));

    let mut cmd = arch_cmd(tmp.path());
    cmd.arg("review").arg(case.as_os_str()).arg("--json");
    let out = cmd.assert().success();
    let json: serde_json::Value =
        serde_json::from_slice(&out.get_output().stdout).expect("stdout — валидный JSON");
    assert_eq!(json["passed"], true);
    assert_eq!(json["route"], "Fast");
    let names: Vec<&str> = json["components"]
        .as_array()
        .expect("components")
        .iter()
        .filter_map(|c| c["name"].as_str())
        .collect();
    for want in ["fitness", "trace_check", "model_validate", "contracts"] {
        assert!(names.contains(&want), "нет секции {want}: {names:?}");
    }
}

/// `arch-be review` на кейсе с битой ссылкой модели: секция `model_validate`
/// FAIL, exit 1 (гейт-семантика единого ревью).
#[test]
fn review_broken_model_exits_1() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let case = review_case(tmp.path());
    std::fs::write(
        case.join("model/ADR-002-bad.md"),
        "---\nid: ADR-002\ntype: adr\ntitle: Битое\nstatus: Accepted\naffects: [CMP-999]\n---\n\nТело.\n",
    )
    .expect("битый ADR");

    let mut cmd = arch_cmd(tmp.path());
    cmd.arg("review").arg(case.as_os_str());
    cmd.assert()
        .code(1)
        .stdout(contains("[FAIL] model_validate"))
        .stdout(contains("broken-link"))
        .stdout(contains("Итог: FAIL"));
}

/// `arch-be model impact <dir> --id`: радиус изменения — затронутые сущности,
/// правило с владельцем, контракт INT, «с кем согласовывать»; exit 0
/// (отчёт, не гейт). `--json` — машиночитаемая форма.
#[test]
fn model_impact_by_id_lists_rules_contracts_owners() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let case = review_case(tmp.path());

    let mut cmd = arch_cmd(tmp.path());
    cmd.arg("model")
        .arg("impact")
        .arg(case.as_os_str())
        .arg("--id")
        .arg("CMP-001");
    cmd.assert()
        .success()
        .stdout(contains("Радиус изменения:"))
        .stdout(contains("CMP-001"))
        .stdout(contains("INT-001"))
        .stdout(contains("OWNER-1"))
        .stdout(contains(
            "C-001 (no_float_money; владелец: Команда платежей)",
        ))
        .stdout(contains("contracts/api.yaml"))
        .stdout(contains("Согласовать с владельцами:"));

    let mut cmd = arch_cmd(tmp.path());
    cmd.arg("model")
        .arg("impact")
        .arg(case.as_os_str())
        .arg("--paths")
        .arg("services/pay/src/main.rs")
        .arg("--paths")
        .arg("docs/notes.md")
        .arg("--json");
    let out = cmd.assert().success();
    let json: serde_json::Value =
        serde_json::from_slice(&out.get_output().stdout).expect("stdout — валидный JSON");
    assert_eq!(json["seeds"], serde_json::json!(["CMP-001"]));
    assert_eq!(json["gaps"], serde_json::json!(["docs/notes.md"]));
    assert_eq!(json["contracts"], serde_json::json!(["contracts/api.yaml"]));
}

/// `arch-be model impact` с неизвестным id — честная ошибка (exit != 0),
/// без источника — тоже.
#[test]
fn model_impact_unknown_id_and_no_source_fail() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let case = review_case(tmp.path());

    let mut cmd = arch_cmd(tmp.path());
    cmd.arg("model")
        .arg("impact")
        .arg(case.as_os_str())
        .arg("--id")
        .arg("CMP-999");
    cmd.assert().failure().stderr(contains("не найдена"));

    let mut cmd = arch_cmd(tmp.path());
    cmd.arg("model").arg("impact").arg(case.as_os_str());
    cmd.assert()
        .failure()
        .stderr(contains("id сущности или paths"));
}

/// `arch-be contract-diff` (бэклог волны 3, п.14): .proto с ломающим диффом
/// (удалено поле без reserved) → exit 1; с `--model` в выводе — потребители
/// и владельцы из радиуса изменения (`INT.contract` → `change_impact`, ADR-035).
#[test]
fn contract_diff_proto_breaking_exits_1_with_consumers() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let case = tmp.path().join("contract-case");
    let model = case.join("model");
    std::fs::create_dir_all(&model).expect("mkdir model");
    for (name, fm) in [
        (
            "CMP-001.md",
            "---\nid: CMP-001\ntype: cmp\ntitle: Платёжный шлюз\nstatus: designed\ndepends_on: [INT-001]\n---\n\nТело.\n",
        ),
        (
            "INT-001.md",
            "---\nid: INT-001\ntype: int\ntitle: Рельс процессинга\nstatus: accepted\ncontract: contracts/pay-new.proto\naffects: [OWNER-1]\n---\n\nТело.\n",
        ),
        (
            "OWNER-1.md",
            "---\nid: OWNER-1\ntype: owner\ntitle: Команда процессинга\nstatus: active\n---\n\nТело.\n",
        ),
    ] {
        std::fs::write(model.join(name), fm).expect("сущность");
    }
    let contracts = case.join("contracts");
    std::fs::create_dir_all(&contracts).expect("mkdir contracts");
    let proto_v1 = "syntax = \"proto3\";\n\npackage acme.payments.v1;\n\nmessage ChargeRequest {\n  string id = 1;\n  int64 amount_minor = 2;\n  optional string currency = 3;\n}\n";
    let proto_v2 = proto_v1.replace("  optional string currency = 3;\n", "");
    std::fs::write(contracts.join("pay-old.proto"), proto_v1).expect("old");
    std::fs::write(contracts.join("pay-new.proto"), proto_v2).expect("new");

    // Без --model: breaking → exit 1, находки CD-P02/CD-P06.
    let mut cmd = arch_cmd(tmp.path());
    cmd.arg("contract-diff")
        .arg(contracts.join("pay-old.proto").as_os_str())
        .arg(contracts.join("pay-new.proto").as_os_str());
    cmd.assert()
        .code(1)
        .stdout(contains("Формат: proto"))
        .stdout(contains("CD-P02"))
        .stdout(contains("CD-P06"))
        .stdout(contains("Итог: FAIL"));

    // С --model: та же ломающая пара отдаёт потребителя и владельца.
    let mut cmd = arch_cmd(tmp.path());
    cmd.arg("contract-diff")
        .arg(contracts.join("pay-old.proto").as_os_str())
        .arg(contracts.join("pay-new.proto").as_os_str())
        .arg("--model")
        .arg(case.as_os_str());
    cmd.assert()
        .code(1)
        .stdout(contains("Связь с моделью: INT-001"))
        .stdout(contains("CMP-001 · Платёжный шлюз"))
        .stdout(contains("OWNER-1 · Команда процессинга"));

    // Не-breaking пара (добавлено поле) — exit 0.
    let proto_v3 = proto_v1.replace(
        "  optional string currency = 3;\n",
        "  optional string currency = 3;\n  string trace_id = 4;\n",
    );
    std::fs::write(contracts.join("pay-v3.proto"), proto_v3).expect("v3");
    let mut cmd = arch_cmd(tmp.path());
    cmd.arg("contract-diff")
        .arg(contracts.join("pay-old.proto").as_os_str())
        .arg(contracts.join("pay-v3.proto").as_os_str())
        .arg("--json");
    let out = cmd.assert().success();
    let json: serde_json::Value =
        serde_json::from_slice(&out.get_output().stdout).expect("stdout — валидный JSON");
    assert_eq!(json["passed"], true);
    assert_eq!(json["format"], "proto");
    assert_eq!(json["breaking"], 0);

    // Неизвестный формат — ошибка clap-края (exit 2) или anyhow (exit 1).
    let mut cmd = arch_cmd(tmp.path());
    cmd.arg("contract-diff")
        .arg(contracts.join("pay-old.proto").as_os_str())
        .arg(contracts.join("pay-new.proto").as_os_str())
        .arg("--format")
        .arg("xml");
    cmd.assert().failure();
}
