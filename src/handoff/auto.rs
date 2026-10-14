//! Лёгкий автопакет handoff (L5 задачи 0.5.0, закрывает B4): когда в
//! репозитории нет `.arch-handoff/` или пакет старше HEAD, `harness_run`
//! собирает минимальный пакет сам — без вызова явного `handoff_create`.
//!
//! # Зачем
//!
//! `handoff_create` (тяжёлый маршрут) вызывается в ~3% прогонов (B4): знание
//! контура доходит до исполнителя свободным текстом LLM и умирает с сессией.
//! Здесь пакет становится **штатным** побочным продуктом каждого прогона:
//! `TASK.md` с секцией «Правила репозитория (выучено)» (чек-лист уроков из
//! префлайт-подбора L4), требованием прогнать `accept_cmd` перед коммитом и
//! JSON-контрактом результата, плюс `LESSONS.md` (рендер отобранных уроков) и
//! `CONSTRAINTS.yaml` (копия корневого реестра). Так кодер получает гейт,
//! которого у него нет без Spine.
//!
//! # Приоритет тяжёлого маршрута
//!
//! Явный `handoff_create` имеет приоритет: признак тяжёлого пакета — наличие
//! `MANIFEST.json` (его пишет только [`crate::handoff::generate_handoff_opts`]).
//! Автопакет не трогает такой пакет ни в каком случае (даже «устаревший»:
//! свежесть тяжёлого пакета — забота архитектора, который его явно собрал).
//!
//! # Свежесть лёгкого пакета
//!
//! Собранный автопакетом пакет помечается sha `HEAD` в файле
//! [`AUTO_HEAD_FILE`] (для не-git репозитория — маркер [`NO_GIT`]). Следующий
//! прогон сверяет маркер с текущим `HEAD`: пакет старше HEAD пересобирается.
//!
//! # Гигиена (риск 6 TASK)
//!
//! Уроки уровня `user`/`org` (личное знание, не свойство репозитория) в файлы
//! пакета не пишутся без флага `--share` — [`build`] отсекает их по
//! [`crate::lessons::ScopeLevel`]. Текст уроков — недоверенные данные: в
//! `LESSONS.md` и чек-лист попадают только уроки, уже прошедшие карантин
//! префлайт-подбора (L4/L3, `src/injection.rs`).

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use crate::error::{HarnessError, Result};
use crate::lessons::{LESSONS_MD, Lesson, ScopeLevel};

use super::{HANDOFF_DIR, git_out};

/// Имя маркера свежести лёгкого пакета: sha `HEAD` на момент сборки.
pub const AUTO_HEAD_FILE: &str = ".auto-head";

/// Маркер не-git репозитория в [`AUTO_HEAD_FILE`]: свежесть по HEAD невозможна,
/// пакет считается свежим, пока существует.
const NO_GIT: &str = "nogit";

/// Заголовок секции выученных правил в `TASK.md` (байт-в-байт контракт).
pub const RULES_SECTION_HEADER: &str = "## Правила репозитория (выучено)";

/// Состояние пакета перед автоупаковкой (решение `harness_run`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackState {
    /// Пакета нет (`TASK.md` отсутствует) — автопакет нужен.
    Missing,
    /// Явный `handoff_create` (`MANIFEST.json`) — автопакет не трогает.
    ExplicitHandoff,
    /// Лёгкий пакет свеж: маркер [`AUTO_HEAD_FILE`] совпадает с `HEAD`.
    Fresh,
    /// Лёгкий пакет старше `HEAD` — пересборка.
    Stale,
}

impl PackState {
    /// Человекочитаемое описание состояния (для заметки прогона).
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Missing => "пакета нет",
            Self::ExplicitHandoff => "явный handoff_create",
            Self::Fresh => "лёгкий пакет свеж",
            Self::Stale => "пакет старше HEAD",
        }
    }

    /// Автопакет обязан собраться (или пересобраться).
    #[must_use]
    pub fn needs_build(self) -> bool {
        matches!(self, Self::Missing | Self::Stale)
    }
}

/// Итог сборки лёгкого пакета.
#[derive(Debug, Clone)]
pub struct AutoPacket {
    /// Каталог `.arch-handoff/`.
    pub dir: PathBuf,
    /// Файлы пакета (TASK.md, LESSONS.md при наличии уроков, CONSTRAINTS.yaml
    /// при наличии корневого реестра).
    pub files: Vec<PathBuf>,
    /// Уроков вошло в `LESSONS.md` и чек-лист `TASK.md` (после фильтра `--share`).
    pub lessons: usize,
    /// Отсечено уроков уровня `user`/`org` (не утекают без `--share`).
    pub shared_excluded: usize,
    /// Что записано в пакетный `CONSTRAINTS.yaml`; `None` — корневого реестра
    /// нет и пакетный не писался.
    pub constraints_action: Option<String>,
    /// `HEAD` на момент сборки (`None` — не-git репозиторий).
    pub head: Option<String>,
}

/// Состояние пакета для автоупаковки: `Missing` (нет `TASK.md`),
/// `ExplicitHandoff` (есть `MANIFEST.json` — тяжёлый пакет), `Fresh`/`Stale`
/// по маркеру [`AUTO_HEAD_FILE`] против текущего `HEAD`.
#[must_use]
pub fn state(repo: &Path) -> PackState {
    let dir = repo.join(HANDOFF_DIR);
    if !dir.join("TASK.md").is_file() {
        return PackState::Missing;
    }
    if dir.join("MANIFEST.json").is_file() {
        return PackState::ExplicitHandoff;
    }
    let recorded = std::fs::read_to_string(dir.join(AUTO_HEAD_FILE))
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    let head = git_head(repo).unwrap_or_else(|| NO_GIT.to_string());
    if recorded == head {
        PackState::Fresh
    } else {
        PackState::Stale
    }
}

/// Собирает лёгкий пакет `.arch-handoff/`: `TASK.md` (задача + «Правила
/// репозитория (выучено)» + приёмка `accept_cmd` + JSON-контракт),
/// `LESSONS.md` (рендер отобранных уроков) и `CONSTRAINTS.yaml` (копия
/// корневого реестра при его наличии), плюс маркер [`AUTO_HEAD_FILE`].
///
/// `lessons` — уроки, отобранные префлайт-подбором L4 под задачу; уроки уровня
/// `user`/`org` отсекаются без `share`. Функция чисто-файловая: LLM, сети и
/// внешних процессов (кроме `git rev-parse`) здесь нет (AD-2, core-сборка).
///
/// # Errors
/// Ошибка создания каталога или записи файла пакета.
pub fn build(
    repo: &Path,
    task: &str,
    lessons: &[Lesson],
    accept_cmd: Option<&str>,
    share: bool,
) -> Result<AutoPacket> {
    let dir = repo.join(HANDOFF_DIR);
    std::fs::create_dir_all(&dir).map_err(|e| HarnessError::io(&dir, e))?;

    // Гигиена (риск 6): уроки user/org-уровня не пишутся в файлы репозитория
    // без --share — личное знание не становится свойством чужого репозитория.
    let (kept, excluded) = repo_scoped(lessons, share);

    let task_md = render_task_md(task, &kept, accept_cmd);
    let task_path = dir.join("TASK.md");
    std::fs::write(&task_path, &task_md).map_err(|e| HarnessError::io(&task_path, e))?;
    let mut files = vec![task_path];

    if !kept.is_empty() {
        let lessons_md = crate::lessons::render::render(&kept);
        let lessons_path = dir.join(LESSONS_MD);
        std::fs::write(&lessons_path, lessons_md)
            .map_err(|e| HarnessError::io(&lessons_path, e))?;
        files.push(lessons_path);
    }

    let constraints_action = copy_root_constraints(repo, &dir, &mut files)?;

    let head = git_head(repo);
    let marker = dir.join(AUTO_HEAD_FILE);
    std::fs::write(&marker, head.as_deref().unwrap_or(NO_GIT))
        .map_err(|e| HarnessError::io(&marker, e))?;

    Ok(AutoPacket {
        dir,
        files,
        lessons: kept.len(),
        shared_excluded: excluded,
        constraints_action,
        head,
    })
}

/// Копирует корневой `CONSTRAINTS.yaml` в пакет (гейт читает пакетную копию
/// первой — те же правила, что видит `arch-be gate .`). Без корневого реестра
/// пакетный не пишется: заготовку не фабрикуем (правила — артефакт архитектора).
fn copy_root_constraints(
    repo: &Path,
    dir: &Path,
    files: &mut Vec<PathBuf>,
) -> Result<Option<String>> {
    let root = repo.join(crate::control::ROOT_CONSTRAINTS_PATH);
    if !root.is_file() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(&root).map_err(|e| HarnessError::io(&root, e))?;
    let dest = dir.join(crate::control::ROOT_CONSTRAINTS_PATH);
    std::fs::write(&dest, &text).map_err(|e| HarnessError::io(&dest, e))?;
    files.push(dest);
    Ok(Some(format!(
        "копия корневого {}",
        crate::control::ROOT_CONSTRAINTS_PATH
    )))
}

/// Уроки, пригодные для записи в репозиторий: без `share` отсекаются уроки
/// уровня шире репозитория (`user`/`org`), возвращается число отсечённых.
fn repo_scoped(lessons: &[Lesson], share: bool) -> (Vec<Lesson>, usize) {
    if share {
        return (lessons.to_vec(), 0);
    }
    let mut kept = Vec::new();
    let mut excluded = 0;
    for lesson in lessons {
        if lesson.scope.level.rank() > ScopeLevel::Repo.rank() {
            excluded += 1;
        } else {
            kept.push(lesson.clone());
        }
    }
    (kept, excluded)
}

/// Рендер `TASK.md` лёгкого пакета: задача + чек-лист выученных правил +
/// приёмка `accept_cmd` + финализация (git-коммит) + JSON-контракт результата.
///
/// Функция чистая: тот же вход — тот же текст байт-в-байт (порядок уроков —
/// порядок входа, который у префлайт-подбора детерминирован).
#[must_use]
pub fn render_task_md(task: &str, lessons: &[Lesson], accept_cmd: Option<&str>) -> String {
    let mut s = String::with_capacity(task.len() + 2000);
    s.push_str("# Задача для кодового харнесса\n\n");
    s.push_str(task.trim());
    s.push('\n');
    if !lessons.is_empty() {
        let _ = writeln!(s, "\n{RULES_SECTION_HEADER}\n");
        s.push_str(
            "Правила выведены из вердиктов приёмочного гейта (реестр уроков, ADR-085). \
             Соблюди их в этой задаче — они обязательны к проверке.\n\n",
        );
        for lesson in lessons {
            s.push_str(&checklist_line(lesson));
        }
    }
    if let Some(command) = accept_cmd {
        let _ = writeln!(s, "\n## Приёмка (обязательно)\n");
        s.push_str(
            "Перед финальным коммитом прогнать команду приёмки и добиться её прохождения:\n\n",
        );
        let _ = writeln!(s, "```bash\n{}\n```\n", command.trim());
        s.push_str(
            "- Команду запускает сам исполнитель в корне репозитория, вне песочницы.\n\
             - Непрошедшая приёмка = невыполненная задача: вердикт проверяется оркестратором.\n",
        );
    }
    let _ = writeln!(s, "\n## Финализация (обязательно)\n");
    s.push_str(
        "Результат забирается из git — перед финальным ответом зафиксируй работу коммитом:\n\n\
         ```bash\n\
         git add -A -- . ':!.arch-handoff'\n\
         git commit -m \"<кратко: что реализовано>\"\n\
         ```\n",
    );
    let _ = writeln!(s, "\n## Контракт результата\n");
    s.push_str(
        "Финальный ответ обязан завершаться JSON-объектом (после него — ни символа):\n\n\
         ```json\n\
         {\"status\": \"complete|partial|blocked\", \"assumptions\": [], \"open_questions\": [], \"conflicts_with_prior_decisions\": []}\n\
         ```\n",
    );
    s
}

/// Одна строка чек-листа урока: `- [ ] LSN-… · подпись — формулировка`.
///
/// Формулировка схлопывается в одну строку; пустая честно помечается — статус
/// урока от текста не зависит (AD-2), поэтому формулировка не обязательна.
fn checklist_line(lesson: &Lesson) -> String {
    let statement = if lesson.statement.trim().is_empty() {
        "(формулировка не зафиксирована — статус от текста не зависит)".to_string()
    } else {
        lesson
            .statement
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    };
    format!(
        "- [ ] **{id}** · {signature} — {statement}\n",
        id = lesson.id,
        signature = lesson.signature.describe(),
    )
}

/// Полный sha текущего `HEAD`; `None` — не-git репозиторий или нет коммитов.
fn git_head(repo: &Path) -> Option<String> {
    git_out(repo, &["rev-parse", "HEAD"]).map(|h| h.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lessons::{
        Evidence, LessonSource, Outcome, RecordInput, Scope, ScopeLevel, Signature,
    };

    /// Урок реестра заданного уровня области (конструктор тестов).
    fn lesson(code: &str, statement: &str, level: ScopeLevel) -> Lesson {
        let signature = Signature::new(LessonSource::Gate, code);
        let input = RecordInput::new(
            signature,
            Evidence {
                run_id: format!("run-{code}"),
                harness: Some("claude-code".to_string()),
                verdict_excerpt: format!("[{code}] нарушение"),
                file: Some("src/lib.rs".to_string()),
                line: Some(1),
                ts: "2026-10-14T10:00:00.000Z".to_string(),
                outcome: Outcome::Fail,
                task: Some(format!("T-{code}")),
            },
        )
        .with_statement(statement)
        .with_scope(Scope::new(level, Vec::<String>::new()));
        Lesson::from_input(&input)
    }

    /// Снапшот TASK.md автопакета: точный байт-в-байт рендер задачи с
    /// чек-листом, приёмкой, финализацией и JSON-контрактом.
    #[test]
    fn task_md_render_is_byte_stable() {
        let lessons = vec![
            lesson(
                "G5",
                "Новый публичный символ обязан попасть в __all__ пакета",
                ScopeLevel::Repo,
            ),
            lesson("G6", "Миграция обязана быть обратимой", ScopeLevel::Module),
        ];
        let id5 = &lessons[0].id;
        let id6 = &lessons[1].id;
        let text = render_task_md(
            "Реализуй модуль paylib.\n\nПодробности в спеке.",
            &lessons,
            Some("arch-be gate ."),
        );
        let expected = format!(
            "# Задача для кодового харнесса\n\n\
             Реализуй модуль paylib.\n\n\
             Подробности в спеке.\n\n\
             ## Правила репозитория (выучено)\n\n\
             Правила выведены из вердиктов приёмочного гейта (реестр уроков, ADR-085). Соблюди их в этой задаче — они обязательны к проверке.\n\n\
             - [ ] **{id5}** · gate · G5 — Новый публичный символ обязан попасть в __all__ пакета\n\
             - [ ] **{id6}** · gate · G6 — Миграция обязана быть обратимой\n\n\
             ## Приёмка (обязательно)\n\n\
             Перед финальным коммитом прогнать команду приёмки и добиться её прохождения:\n\n\
             ```bash\n\
             arch-be gate .\n\
             ```\n\n\
             - Команду запускает сам исполнитель в корне репозитория, вне песочницы.\n\
             - Непрошедшая приёмка = невыполненная задача: вердикт проверяется оркестратором.\n\n\
             ## Финализация (обязательно)\n\n\
             Результат забирается из git — перед финальным ответом зафиксируй работу коммитом:\n\n\
             ```bash\n\
             git add -A -- . ':!.arch-handoff'\n\
             git commit -m \"<кратко: что реализовано>\"\n\
             ```\n\n\
             ## Контракт результата\n\n\
             Финальный ответ обязан завершаться JSON-объектом (после него — ни символа):\n\n\
             ```json\n\
             {{\"status\": \"complete|partial|blocked\", \"assumptions\": [], \"open_questions\": [], \"conflicts_with_prior_decisions\": []}}\n\
             ```\n"
        );
        assert_eq!(text, expected);
        // Повторный рендер — байт-в-байт тот же (детерминизм, AD-2).
        assert_eq!(
            render_task_md(
                "Реализуй модуль paylib.\n\nПодробности в спеке.",
                &lessons,
                Some("arch-be gate .")
            ),
            expected
        );
    }

    /// Без уроков и приёмки — только задача, финализация и контракт: пустых
    /// секций нет, лишнего текста исполнителю не добавляется.
    #[test]
    fn task_md_without_lessons_and_accept_has_no_empty_sections() {
        let text = render_task_md("Простая задача", &[], None);
        assert!(
            text.starts_with("# Задача для кодового харнесса\n\nПростая задача\n"),
            "{text}"
        );
        assert!(!text.contains(RULES_SECTION_HEADER), "{text}");
        assert!(!text.contains("## Приёмка"), "{text}");
        assert!(text.contains("## Финализация"), "{text}");
        assert!(text.contains("conflicts_with_prior_decisions"), "{text}");
    }

    /// Гигиена (риск 6): уроки уровня `user`/`org` отсекаются без `--share`,
    /// с `share` — проходят в пакет целиком.
    #[test]
    fn user_and_org_lessons_do_not_leak_without_share() {
        let repo = lesson("G5", "репозиторное", ScopeLevel::Repo);
        let user = lesson("U1", "личное", ScopeLevel::User);
        let org = lesson("O1", "организационное", ScopeLevel::Org);
        let all = vec![repo.clone(), user.clone(), org.clone()];

        let (kept, excluded) = repo_scoped(&all, false);
        assert_eq!(excluded, 2, "user+org отсечены: {excluded}");
        assert_eq!(kept, vec![repo.clone()]);

        let (kept, excluded) = repo_scoped(&all, true);
        assert_eq!(excluded, 0);
        assert_eq!(kept.len(), 3, "--share пропускает всё");
    }

    /// `state()`: нет `TASK.md` → `Missing`; есть `MANIFEST.json` →
    /// `ExplicitHandoff`; маркер не совпадает с `HEAD` → `Stale`; совпадает →
    /// `Fresh`.
    #[test]
    fn pack_state_distinguishes_missing_explicit_fresh_and_stale() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).expect("mkdir");
        assert_eq!(state(&repo), PackState::Missing);

        // Явный тяжёлый пакет: MANIFEST.json — приоритет handoff_create.
        let dir = repo.join(HANDOFF_DIR);
        std::fs::create_dir_all(&dir).expect("mkdir");
        std::fs::write(dir.join("TASK.md"), "# задача\n").expect("task");
        std::fs::write(dir.join("MANIFEST.json"), "{}").expect("manifest");
        assert_eq!(state(&repo), PackState::ExplicitHandoff);

        // Лёгкий пакет без маркера в не-git репозитории: запись «nogit» vs
        // пустой маркер → Stale; после записи маркера «nogit» → Fresh.
        std::fs::remove_file(dir.join("MANIFEST.json")).expect("rm manifest");
        assert_eq!(state(&repo), PackState::Stale, "пустой маркер не совпадает");
        std::fs::write(dir.join(AUTO_HEAD_FILE), NO_GIT).expect("marker");
        assert_eq!(state(&repo), PackState::Fresh);
    }

    /// Свежесть по git HEAD: после коммита пакет, собранный на старом HEAD,
    /// пересобирается (Stale), на актуальном — свеж (Fresh).
    #[test]
    fn pack_state_tracks_git_head() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).expect("mkdir repo");
        let git = |args: &[&str]| {
            let out = std::process::Command::new("git")
                .arg("-C")
                .arg(&repo)
                .args(args)
                .output()
                .expect("git");
            assert!(out.status.success(), "git {args:?}: {:?}", out.stderr);
        };
        git(&["init", "-q"]);
        git(&[
            "-c",
            "user.name=test",
            "-c",
            "user.email=test@test",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "baseline",
        ]);
        let first_head = git_head(&repo).expect("HEAD");
        std::fs::create_dir_all(repo.join(HANDOFF_DIR)).expect("mkdir handoff");
        std::fs::write(repo.join(HANDOFF_DIR).join("TASK.md"), "# задача\n").expect("task");
        std::fs::write(repo.join(HANDOFF_DIR).join(AUTO_HEAD_FILE), &first_head).expect("marker");
        assert_eq!(state(&repo), PackState::Fresh, "маркер == HEAD");

        git(&[
            "-c",
            "user.name=test",
            "-c",
            "user.email=test@test",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "второй коммит",
        ]);
        assert_eq!(
            state(&repo),
            PackState::Stale,
            "HEAD ушёл вперёд — пакет старше"
        );
    }

    /// `build()` пишет TASK.md (+ секцию правил), LESSONS.md с отобранными
    /// уроками и копию корневого CONSTRAINTS.yaml; маркер — HEAD.
    #[test]
    fn build_writes_light_package_files() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).expect("mkdir");
        std::fs::write(repo.join("CONSTRAINTS.yaml"), "# корневой реестр\n").expect("constraints");

        let lessons = vec![lesson(
            "G5",
            "Новый публичный символ обязан попасть в __all__",
            ScopeLevel::Repo,
        )];
        let packet =
            build(&repo, "задача", &lessons, Some("arch-be gate ."), false).expect("build");
        assert_eq!(packet.lessons, 1);
        assert_eq!(packet.shared_excluded, 0);

        let dir = repo.join(HANDOFF_DIR);
        let task = std::fs::read_to_string(dir.join("TASK.md")).expect("task");
        assert!(task.contains(RULES_SECTION_HEADER), "{task}");
        assert!(task.contains("arch-be gate ."), "{task}");
        assert!(task.contains("conflicts_with_prior_decisions"), "{task}");
        let lessons_md = std::fs::read_to_string(dir.join(LESSONS_MD)).expect("lessons");
        assert!(lessons_md.contains("gate · G5"), "{lessons_md}");
        let constraints =
            std::fs::read_to_string(dir.join("CONSTRAINTS.yaml")).expect("constraints");
        assert_eq!(constraints, "# корневой реестр\n");
        assert_eq!(
            packet.constraints_action.as_deref(),
            Some("копия корневого CONSTRAINTS.yaml")
        );
        // Маркер свежести: не-git репозиторий → NO_GIT.
        assert_eq!(
            std::fs::read_to_string(dir.join(AUTO_HEAD_FILE)).expect("marker"),
            NO_GIT
        );
    }

    /// Без корневого реестра пакетный CONSTRAINTS.yaml не пишется, а LESSONS.md
    /// не создаётся при пустом списке уроков (без шума в пакете).
    #[test]
    fn build_skips_constraints_and_empty_lessons() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).expect("mkdir");
        let packet = build(&repo, "задача", &[], None, false).expect("build");
        assert!(packet.constraints_action.is_none());
        assert_eq!(packet.lessons, 0);
        let dir = repo.join(HANDOFF_DIR);
        assert!(dir.join("TASK.md").is_file());
        assert!(
            !dir.join(LESSONS_MD).exists(),
            "пустой LESSONS.md не пишется"
        );
        assert!(!dir.join("CONSTRAINTS.yaml").exists());
    }
}
