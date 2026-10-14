//! Хуки-гейты как подкоманды бинаря (ROADMAP 1.7 п.2): логика `Stop` /
//! `PostToolUse` / `pre-commit` / `pre-push`, раньше зашитая в shell-шаблоны
//! `connect`, версионируется вместе с ядром — обновление `arch-be`
//! обновляет семантику уже установленных хуков без повторного `connect`
//! на каждой машине. Сюда же — хуки инъекции уроков `session-start` и
//! `user-prompt-submit` (ADR-087): они ничего не блокируют, а печатают
//! закреплённый блок реестра в stdout (хосты добавляют stdout хука в контекст
//! модели) — Spine Core без своей LLM получает проверенную гейтом память.
//!
//! Установленные хуки — тонкие shim'ы вида `arch-be hook stop`; в shell
//! остаётся только fail-soft гард `command -v arch-be` (удалённый бинарь не
//! ломает сессии и коммиты). База диффа (якорь основной ветки, stdin git'а
//! для pre-push), запуск гейта и коды выхода — здесь. Семантика кодов по
//! площадкам: Claude/Kimi — exit 2 блок (stderr уходит модели), git-хуки —
//! exit 1; хуки уроков — всегда 0 (память не блокирует работу).

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::config::Config;
use crate::error::{HarnessError, Result};
use crate::lesson_block::{self, Quarantined};

/// Сколько символов промпта уходит в BM25-отбор уроков (хвост отбрасывается:
/// длинный ввод — это вставленный контекст, а не запрос).
const MAX_QUERY_CHARS: usize = 1000;

/// Вид хука — совпадает с именами подкоманд `arch-be hook`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookKind {
    /// Stop-хук хоста (Claude Code, Kimi Code): полный гейт перед концом сессии.
    Stop,
    /// PostToolUse-хук хоста (`--strict-hooks`): полный гейт на каждую правку.
    PostToolUse,
    /// git pre-commit: быстрый fitness-гейт (`control check`).
    PreCommit,
    /// git pre-push: полный гейт с базой из stdin git'а (Н5).
    PrePush,
    /// `SessionStart` хоста: уроки области репозитория в контекст (ADR-087).
    SessionStart,
    /// `UserPromptSubmit` хоста: уроки по упомянутым в промпте путям (ADR-087).
    UserPromptSubmit,
}

impl HookKind {
    /// Разбирает имя подкоманды (`stop`, `post-tool-use`, `pre-commit`,
    /// `pre-push`, `session-start`, `user-prompt-submit`).
    ///
    /// # Errors
    /// Незнакомое имя — перечень допустимых.
    pub fn parse(name: &str) -> Result<Self> {
        match name.trim().to_ascii_lowercase().as_str() {
            "stop" => Ok(Self::Stop),
            "post-tool-use" => Ok(Self::PostToolUse),
            "pre-commit" => Ok(Self::PreCommit),
            "pre-push" => Ok(Self::PrePush),
            "session-start" => Ok(Self::SessionStart),
            "user-prompt-submit" => Ok(Self::UserPromptSubmit),
            other => Err(HarnessError::Config(format!(
                "неизвестный хук '{other}': допустимы stop, post-tool-use, pre-commit, \
                 pre-push, session-start, user-prompt-submit"
            ))),
        }
    }

    /// Хук инъекции уроков (печатает блок, никогда не блокирует).
    #[must_use]
    pub fn is_lessons(self) -> bool {
        matches!(self, Self::SessionStart | Self::UserPromptSubmit)
    }

    /// Читает ли хук stdin (git pre-push и хуки уроков — читают).
    #[must_use]
    pub fn reads_stdin(self) -> bool {
        matches!(
            self,
            Self::PrePush | Self::SessionStart | Self::UserPromptSubmit
        )
    }

    /// Код блокировки по семантике площадки: хостовые хуки (Claude/Kimi) —
    /// 2 (stderr уходит модели), git-хуки — 1, хуки уроков — 0 (не блокируют).
    fn block_code(self) -> i32 {
        match self {
            Self::Stop | Self::PostToolUse => 2,
            Self::PreCommit | Self::PrePush => 1,
            Self::SessionStart | Self::UserPromptSubmit => 0,
        }
    }
}

/// Разобранный stdin хука хоста: рабочий каталог и текст промпта.
///
/// Claude Code и Kimi Code передают хуку JSON (`cwd`, `prompt`), но контур не
/// полагается на это: неизвестный/битый вход трактуется как текст промпта,
/// пустой — как «ничего не сообщено» (fail-soft, ADR-087).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HookInput {
    /// Рабочий каталог сессии (если хост его сообщил).
    pub cwd: Option<PathBuf>,
    /// Текст пользовательского промпта (если хост его сообщил).
    pub prompt: Option<String>,
}

/// Разбор stdin хука (JSON Claude Code/Kimi Code с фолбэком на сырой текст).
#[must_use]
pub fn parse_hook_input(stdin_text: &str) -> HookInput {
    let trimmed = stdin_text.trim();
    if trimmed.is_empty() {
        return HookInput::default();
    }
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) {
        let cwd = value
            .get("cwd")
            .and_then(serde_json::Value::as_str)
            .map(PathBuf::from);
        let prompt = ["prompt", "user_prompt", "text"]
            .iter()
            .find_map(|key| value.get(*key).and_then(serde_json::Value::as_str))
            .map(str::to_string);
        return HookInput { cwd, prompt };
    }
    // Не JSON — хост отдал сырой текст (например, промпт одной строкой).
    HookInput {
        cwd: None,
        prompt: Some(trimmed.to_string()),
    }
}

/// Итог хука уроков: текст для stdout плюс отсеянные карантином уроки.
#[derive(Debug, Clone, Default)]
pub struct LessonHook {
    /// Блок уроков (пусто — печатать нечего).
    pub block: String,
    /// Уроки, не прошедшие проверку на prompt-injection.
    pub quarantined: Vec<Quarantined>,
}

/// Текст, который хук уроков печатает в stdout (без печати — тестируемо).
///
/// `session-start` — уроки области репозитория (пути не передаются: область
/// сессии — весь репозиторий); `user-prompt-submit` — уроки, релевантные
/// упомянутым в промпте путям (BM25 из `src/kb.rs` плюс scope-отбор).
#[must_use]
pub fn lessons_hook_output(
    kind: HookKind,
    cfg: &Config,
    repo: &Path,
    stdin_text: &str,
) -> LessonHook {
    let input = parse_hook_input(stdin_text);
    let cwd = input.cwd.unwrap_or_else(|| repo.to_path_buf());
    let (paths, query) = if kind == HookKind::UserPromptSubmit {
        let prompt: String = input
            .prompt
            .unwrap_or_default()
            .chars()
            .take(MAX_QUERY_CHARS)
            .collect();
        (lesson_block::paths_from_text(&prompt), Some(prompt))
    } else {
        (Vec::new(), None)
    };
    let report = lesson_block::build_report(cfg, &cwd, &paths, query.as_deref());
    LessonHook {
        block: report.block,
        quarantined: report.quarantined,
    }
}

/// Выполняет хук уроков: блок в stdout, карантин — строкой в stderr (виден
/// человеку; stdout уходит в контекст модели, служебный текст там лишний).
/// Возвращает 0 всегда: память не имеет права блокировать работу хоста.
fn run_lessons_hook(kind: HookKind, cfg: &Config, repo: &Path, stdin_text: &str) -> i32 {
    let hook = lessons_hook_output(kind, cfg, repo, stdin_text);
    if !hook.block.is_empty() {
        print!("{}", hook.block);
    }
    if !hook.quarantined.is_empty() {
        let ids: Vec<&str> = hook.quarantined.iter().map(|q| q.id.as_str()).collect();
        eprintln!(
            "spine-connect: карантин уроков: {} с признаками prompt-injection не попали \
             в контекст ({}) — разберите формулировки (`arch-be lessons show <id>`)",
            ids.len(),
            ids.join(", ")
        );
    }
    0
}

/// Выполняет хук и возвращает код выхода для процесса (0 — пропуск).
///
/// `stdin_text` — содержимое stdin (pre-push читает строки git'а; для
/// остальных видов игнорируется).
///
/// # Errors
/// Ошибка запуска гейта/fitness (не путать с провалом: провал — это
/// ненулевой КОД, а не Err).
pub fn run_hook(kind: HookKind, cfg: &Config, repo: &Path, stdin_text: &str) -> Result<i32> {
    if kind == HookKind::PreCommit {
        return run_fitness_hook(cfg, repo);
    }
    if kind.is_lessons() {
        return Ok(run_lessons_hook(kind, cfg, repo, stdin_text));
    }
    let base = match kind {
        HookKind::PrePush => pre_push_base(repo, stdin_text),
        _ => crate::control::default_anchor_base(repo),
    };
    let code = run_gate(cfg, repo, base.as_deref())?;
    if code == 0 {
        return Ok(0);
    }
    let note = match kind {
        HookKind::Stop => {
            "архитектурный гейт FAIL — исправьте находки error \
                            перед завершением (подробности выше; гейт: arch-be gate)"
        }
        HookKind::PostToolUse => {
            "правка не проходит архитектурный гейт \
                                  (arch-be gate FAIL) — исправьте находки error"
        }
        HookKind::PrePush => "pre-push FAIL — arch-be gate не пройден (находки выше)",
        HookKind::PreCommit => unreachable!("pre-commit обрабатывается выше"),
        HookKind::SessionStart | HookKind::UserPromptSubmit => {
            unreachable!("хуки уроков обрабатываются выше и не блокируют")
        }
    };
    eprintln!("\nspine-connect: {note}");
    Ok(kind.block_code())
}

/// Прогон единого гейта для хука (маршрут auto): опции и требования — как у
/// CLI-края `arch-be gate` (дефолтная политика доверия exec, составляющие
/// из конфига). Возвращает exit-код гейта (0/1/2/3); отчёт печатается в
/// stderr только при ненулевом коде — зелёный хук молчит.
fn run_gate(cfg: &Config, repo: &Path, base: Option<&str>) -> Result<i32> {
    let limits = cfg.significance.limits()?;
    let options = crate::gate::GateOptions {
        exec: crate::cmd_trust::ExecPolicy::cli(false),
        ..crate::gate::GateOptions::from_config(cfg)
    };
    let requirements = crate::gate::GateRequirements::from_config(&cfg.gate);
    let report = crate::gate::run_opts(repo, None, base, None, limits, &requirements, &options)?;
    let code = report.outcome.exit_code();
    if code != 0 {
        eprint!("{}", crate::gate::render(&report));
    }
    Ok(code)
}

/// pre-commit: быстрый fitness-гейт (`control check`). Расположение реестра
/// резолвит бинарь (корень, затем `.arch-handoff/`, T-01); нет реестра
/// нигде — понятный текст в stderr и код блока (молчаливый пропуск красного
/// гейта был той самой дырой).
fn run_fitness_hook(cfg: &Config, repo: &Path) -> Result<i32> {
    let Some(constraints) = crate::control::resolve_constraints_path(repo, None) else {
        eprintln!(
            "spine-connect: реестр правил не найден: ни {} в корне, ни {} — \
             создайте каркас: `arch-be bootstrap`",
            crate::control::ROOT_CONSTRAINTS_PATH,
            crate::control::HANDOFF_CONSTRAINTS_PATH
        );
        return Ok(HookKind::PreCommit.block_code());
    };
    let options = crate::control::baseline::CheckOptions {
        baseline: None,
        baseline_update: false,
        changed_since: None,
        exec: crate::cmd_trust::ExecPolicy::cli(false),
        overrides: crate::control::baseline::OverrideSettings {
            adr_dir: cfg.gate.overrides.adr_dir.clone(),
            max_horizon_months: cfg.gate.overrides.max_horizon_months,
        },
        // ADR-068: ресурсы среды — детект по требованию правил (`None`).
        resources: None,
        // ADR-068 Am.3: реестр зондов — из `[gate.requires.<имя>]`.
        probes: crate::control::requires::ProbeRegistry::from_config(&cfg.gate.requires)?,
        // S6: доступность JDK — детект (`None`).
        jdk: None,
    };
    let report = crate::control::check_anchored(repo, &constraints, &options, None)?;
    if !report.passed {
        println!("{}", report.summary);
        for i in &report.issues {
            println!(
                "  [{}] {}:{} {} — {}",
                i.severity,
                i.file.display(),
                i.line,
                i.rule,
                i.message
            );
        }
        eprintln!("spine-connect: pre-commit FAIL — исправьте находки error (отчёт выше)");
    }
    Ok(if report.passed {
        0
    } else {
        HookKind::PreCommit.block_code()
    })
}

/// База диффа pre-push из stdin git'а (строки `<local ref> <local sha>
/// <remote ref> <remote sha>`). Remote sha из нулей — новая ветка: база —
/// точка ответвления от основной ветки (`merge-base` с local sha), тот же
/// список веток, что у [`crate::control::default_anchor_base`]. Иначе база —
/// remote sha (анти-ослабление правил видит УЖЕ закоммиченное ослабление,
/// Н5). Пустой вход — None (гейт без базы, как 0.3.3).
#[must_use]
pub fn pre_push_base(repo: &Path, stdin_text: &str) -> Option<String> {
    for line in stdin_text.lines() {
        let mut fields = line.split_whitespace();
        let (Some(_local_ref), Some(local_sha), Some(_remote_ref), Some(remote_sha)) =
            (fields.next(), fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        if remote_sha.is_empty() {
            continue;
        }
        if remote_sha.chars().all(|c| c == '0') {
            for anchor in ["origin/main", "main", "origin/master", "master"] {
                if git_ok(repo, &["rev-parse", "--verify", "--quiet", anchor]) {
                    if let Some(base) = git_stdout(repo, &["merge-base", anchor, local_sha]) {
                        return Some(base);
                    }
                }
            }
        } else {
            return Some(remote_sha.to_string());
        }
    }
    None
}

/// `git <args>` в репозитории — успех ли (тихо, без stdin/stderr).
fn git_ok(repo: &Path, args: &[&str]) -> bool {
    Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .is_ok_and(|out| out.status.success())
}

/// stdout `git <args>` (trim) при успехе, иначе None.
fn git_stdout(repo: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!text.is_empty()).then_some(text)
}

/// Читает stdin pre-push, НЕ блокируясь на терминале: при ручном запуске
/// `arch-be hook pre-push` stdin — TTY, и ждать EOF там нечего.
#[must_use]
pub fn read_stdin_if_piped() -> String {
    use std::io::IsTerminal as _;
    if std::io::stdin().is_terminal() {
        return String::new();
    }
    std::io::read_to_string(std::io::stdin()).unwrap_or_default()
}

/// Канонический путь к shim-команде хука для connect/докторских проверок:
/// `arch-be hook <имя>` (логика — в бинаре).
#[must_use]
pub fn hook_subcommand_name(kind: HookKind) -> &'static str {
    match kind {
        HookKind::Stop => "stop",
        HookKind::PostToolUse => "post-tool-use",
        HookKind::PreCommit => "pre-commit",
        HookKind::PrePush => "pre-push",
        HookKind::SessionStart => "session-start",
        HookKind::UserPromptSubmit => "user-prompt-submit",
    }
}

/// Путь по умолчанию для репозитория хука (текущий каталог) — единая точка
/// для CLI (clap `default_value` не хранит логику).
#[must_use]
pub fn default_repo() -> PathBuf {
    PathBuf::from(".")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// git в каталоге с тестовой идентичностью коммиттера (изоляция AD-7).
    fn git(dir: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
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
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    /// Репо: main с коммитом и ветка feature с ещё одним коммитом.
    /// Возвращает (путь, sha base-коммита main, sha вершины feature).
    fn repo_with_feature(dir: &Path) -> (PathBuf, String, String) {
        let repo = dir.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]);
        std::fs::write(repo.join("f.txt"), "base\n").unwrap();
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-q", "-m", "base"]);
        let base_sha = git(&repo, &["rev-parse", "HEAD"]).trim().to_string();
        git(&repo, &["checkout", "-q", "-b", "feature"]);
        std::fs::write(repo.join("f.txt"), "base\nfeature\n").unwrap();
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-q", "-m", "feature"]);
        let local_sha = git(&repo, &["rev-parse", "HEAD"]).trim().to_string();
        (repo, base_sha, local_sha)
    }

    #[test]
    fn hook_kind_parse_names_and_rejects_unknown() {
        assert_eq!(HookKind::parse("stop").expect("stop"), HookKind::Stop);
        assert_eq!(
            HookKind::parse("post-tool-use").expect("ptu"),
            HookKind::PostToolUse
        );
        assert_eq!(
            HookKind::parse("pre-commit").expect("pc"),
            HookKind::PreCommit
        );
        assert_eq!(HookKind::parse("pre-push").expect("pp"), HookKind::PrePush);
        assert_eq!(
            HookKind::parse("session-start").expect("ss"),
            HookKind::SessionStart
        );
        assert_eq!(
            HookKind::parse("user-prompt-submit").expect("ups"),
            HookKind::UserPromptSubmit
        );
        let err = HookKind::parse("stop-hook").expect_err("неизвестный");
        assert!(err.to_string().contains("допустимы"), "{err}");
        assert!(err.to_string().contains("session-start"), "{err}");
        // Хуки уроков: не блокируют, stdin читают, имя подкоманды — канон.
        for kind in [HookKind::SessionStart, HookKind::UserPromptSubmit] {
            assert!(kind.is_lessons(), "{kind:?}");
            assert!(kind.reads_stdin(), "{kind:?}");
            assert_eq!(kind.block_code(), 0, "{kind:?}");
            assert!(hook_subcommand_name(kind).contains('-'), "{kind:?}");
        }
        assert!(!HookKind::Stop.is_lessons() && !HookKind::Stop.reads_stdin());
    }

    /// Хук `session-start` без реестра (дефолтный конфиг: `[lessons] enabled =
    /// false`) — пустой блок и код 0: fail-soft, никакой блокировки сессии.
    #[test]
    fn session_start_hook_without_registry_is_silent_and_soft() {
        let tmp = tempfile::tempdir().expect("tmp");
        let cfg = Config::default();
        assert!(!cfg.lessons.enabled, "дефолт — выключено");
        let hook = lessons_hook_output(HookKind::SessionStart, &cfg, tmp.path(), "");
        assert!(hook.block.is_empty(), "{}", hook.block);
        assert_eq!(hook.quarantined.len(), 0);
        assert_eq!(
            run_hook(HookKind::SessionStart, &cfg, tmp.path(), "").expect("хук"),
            0,
            "хук уроков не блокирует"
        );
    }

    /// Хук `session-start` с непустым реестром отдаёт блок уроков; ввод хоста
    /// (JSON с `cwd`) переносит область отбора в рабочий каталог сессии.
    #[test]
    fn session_start_hook_emits_block_from_registry() {
        let tmp = tempfile::tempdir().expect("tmp");
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).expect("repo");
        let mut cfg = Config::default();
        cfg.lessons.enabled = true;
        cfg.lessons.user_dir = Some(tmp.path().join("user-lessons"));
        let th = crate::lessons::Thresholds::default();
        let ledger = crate::lessons::Ledger::repo(&repo);
        let input = crate::lessons::RecordInput::new(
            crate::lessons::Signature::new(crate::lessons::LessonSource::Gate, "G5"),
            crate::lessons::Evidence {
                run_id: "run-1".into(),
                harness: Some("claude-code".into()),
                verdict_excerpt: "[G5] нет __all__".into(),
                file: Some("src/lib.rs".into()),
                line: Some(7),
                ts: "2026-10-14T10:00:00.000Z".into(),
                outcome: crate::lessons::Outcome::Fail,
                task: Some("T1".into()),
            },
        )
        .with_statement("Новый публичный символ обязан попасть в __all__ пакета");
        ledger.record(&input, &th, "2026-10-14").expect("record");

        let cwd_json = format!(
            "{{\"cwd\": \"{}\", \"source\": \"startup\"}}",
            repo.display()
        );
        let hook = lessons_hook_output(HookKind::SessionStart, &cfg, &repo, &cwd_json);
        assert!(hook.block.contains(lesson_block::HEADER), "{}", hook.block);
        assert!(hook.block.contains("__all__"), "{}", hook.block);
        // Хук никогда не отдаёт ненулевой код — даже с непустым реестром.
        assert_eq!(
            run_hook(HookKind::SessionStart, &cfg, &repo, &cwd_json).expect("хук"),
            0
        );
    }

    /// Хук `user-prompt-submit`: блок отбирается по путям и тексту промпта,
    /// а битый stdin не роняет хук (fail-soft в пользу хоста).
    #[test]
    fn user_prompt_submit_hook_reads_paths_and_survives_garbage() {
        let tmp = tempfile::tempdir().expect("tmp");
        let mut cfg = Config::default();
        cfg.lessons.enabled = true;
        cfg.lessons.user_dir = Some(tmp.path().join("user-lessons"));
        let input = parse_hook_input("{\"prompt\": \"поправь src/a.rs\"}");
        assert_eq!(input.prompt.as_deref(), Some("поправь src/a.rs"));
        assert_eq!(input.cwd, None);
        let raw = parse_hook_input("просто текст без JSON");
        assert_eq!(raw.prompt.as_deref(), Some("просто текст без JSON"));
        assert_eq!(parse_hook_input("  "), HookInput::default());
        // Битый JSON — как текст промпта, не паника.
        assert!(parse_hook_input("{не json").prompt.is_some());
        assert_eq!(
            run_hook(HookKind::UserPromptSubmit, &cfg, tmp.path(), "{не json").expect("хук"),
            0
        );
    }

    #[test]
    fn pre_push_base_takes_remote_sha_when_set() {
        let tmp = tempfile::tempdir().expect("tmp");
        let (repo, _base, _local) = repo_with_feature(tmp.path());
        let stdin = format!(
            "refs/heads/f {} refs/heads/f {}",
            "a".repeat(40),
            "b".repeat(40)
        );
        assert_eq!(
            pre_push_base(&repo, &stdin),
            Some("b".repeat(40)),
            "существующая ветка — база = remote sha"
        );
    }

    #[test]
    fn pre_push_base_new_branch_falls_back_to_merge_base() {
        let tmp = tempfile::tempdir().expect("tmp");
        let (repo, base_sha, local_sha) = repo_with_feature(tmp.path());
        let zeros = "0".repeat(40);
        let stdin = format!("refs/heads/feature {local_sha} refs/heads/feature {zeros}");
        assert_eq!(
            pre_push_base(&repo, &stdin),
            Some(base_sha),
            "новая ветка — точка ответвления от main"
        );
    }

    #[test]
    fn pre_push_base_empty_or_malformed_stdin_is_none() {
        let tmp = tempfile::tempdir().expect("tmp");
        let (repo, _base, _local) = repo_with_feature(tmp.path());
        assert_eq!(pre_push_base(&repo, ""), None);
        assert_eq!(pre_push_base(&repo, "мусор\n"), None);
        // Строка с пустым remote sha — пропускается.
        assert_eq!(
            pre_push_base(&repo, "refs/heads/a sha refs/heads/a \n"),
            None
        );
    }

    /// pre-commit на репо с запрещённым маркером — блок; на чистом — пропуск.
    #[test]
    fn pre_commit_hook_blocks_failing_fitness_and_passes_clean() {
        let tmp = tempfile::tempdir().expect("tmp");
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]);
        std::fs::write(
            repo.join("CONSTRAINTS.yaml"),
            "rules:\n  - id: X-1\n    name: no_pan\n    type: must_not_contain\n    glob: \"**/*.py\"\n    pattern: \"PAN=\"\n    severity: critical\n",
        )
        .unwrap();
        std::fs::write(repo.join("a.py"), "print('ok')\n").unwrap();
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-q", "-m", "base"]);
        let cfg = Config::default();

        // Чистое дерево — пропуск.
        assert_eq!(
            run_hook(HookKind::PreCommit, &cfg, &repo, "").expect("hook"),
            0
        );

        // Нарушение правила — блок (код 1, git-площадка).
        std::fs::write(repo.join("a.py"), "print('PAN=4111')\n").unwrap();
        assert_eq!(
            run_hook(HookKind::PreCommit, &cfg, &repo, "").expect("hook"),
            1
        );
    }

    /// pre-commit без реестра — понятная ошибка и блок, а не молчаливый
    /// пропуск (дыра T-01: гард по `.arch-handoff/CONSTRAINTS.yaml` глушил
    /// красный гейт на кейсе с реестром в корне — здесь реестра нет вовсе).
    #[test]
    fn pre_commit_hook_without_constraints_blocks_loudly() {
        let tmp = tempfile::tempdir().expect("tmp");
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]);
        std::fs::write(repo.join("a.py"), "print('ok')\n").unwrap();
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-q", "-m", "base"]);
        let cfg = Config::default();
        assert_eq!(
            run_hook(HookKind::PreCommit, &cfg, &repo, "").expect("hook"),
            1,
            "нет реестра — блок с текстом, а не тихий пропуск"
        );
    }
}
