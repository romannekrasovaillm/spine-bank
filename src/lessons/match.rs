//! Префлайт-подбор уроков под задачу (L4 задачи 0.5.0): детерминированный
//! отбор без LLM.
//!
//! # Зачем
//!
//! Реестр уроков (L1, ADR-085) хранит выученное, блок инъекции (L3, ADR-087)
//! доносит уроки до адресата. Здесь живёт **отбор под конкретную задачу**:
//! перед упаковкой handoff (L5) и перед запуском исполнителя нужно выбрать не
//! «все уроки репозитория», а те, что релевантны задаче, — иначе блок
//! раздувается шумом (TASK §7.3), а неподходящий урок начинает мешать.
//!
//! # Отбор (AD-2: LLM не участвует ни на одной ветке)
//!
//! [`lessons_match`] ранжирует уроки тремя детерминированными ключами:
//! 1. **scope** — урок применим к путям задачи: объединение явных `paths` и
//!    путей, извлечённых из текста ([`paths_from_text`]); пустой список путей
//!    читается как «вся область репозитория» (тот же смысл, что у старта
//!    сессии, ADR-087);
//! 2. **BM25** — текст задачи против `statement` **и** `signature` (тот же
//!    скоринг, что у [`crate::kb::rank_documents`]); вес — строка
//!    `statement + сигнатура`, поэтому совпадение по коду или виду субъекта
//!    тоже поднимает урок;
//! 3. **статус** — `confirmed` > `candidate` > `observed` (промоутированный в
//!    правило урок сильнее `confirmed`; `retired` и уроки с `misses > 0` из
//!    выдачи исключены — они «начали мешать», TASK §7.2).
//!
//! При равных ключах порядок — по `hits`, затем по `id`: повторный вызов на
//! том же входе даёт тот же порядок байт-в-байт (основа воспроизводимости
//! блока L3 и пакета L5).

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::config::Config;
use crate::error::{HarnessError, Result};
use crate::llm::ToolSpec;
use crate::tool::{Tool, ToolContext, ToolOutput};

use super::{Ledger, Lesson, LessonStatus, Thresholds};

/// Максимум путей, извлечённых из текста (защита от раздувания отбора).
const MAX_PATHS: usize = 32;

/// Обрезка длинного пути-токена на входе (длиннее — не путь репозитория).
const MAX_PATH_CHARS: usize = 200;

/// Ширина статуса в ранжировании L4: сильные уроки первыми. `promoted` —
/// урок, уже принятый fitness-правилом (L7), — сильнее `confirmed`; `retired`
/// в выдачу не попадает вовсе (фильтр [`lessons_match`]).
#[must_use]
fn status_rank(status: LessonStatus) -> u8 {
    match status {
        LessonStatus::Promoted => 4,
        LessonStatus::Confirmed => 3,
        LessonStatus::Candidate => 2,
        LessonStatus::Observed => 1,
        LessonStatus::Retired => 0,
    }
}

/// Отбор уроков под задачу (L4): top-K по области, BM25-релевантности и
/// статусу. Чистая функция — один и тот же вход даёт один и тот же порядок.
///
/// Урок попадает в выдачу, если применим хотя бы к одному пути задачи
/// (`paths` ∪ [`paths_from_text`]) **либо** имеет ненулевой BM25-вес против
/// текста задачи. Пустой список путей — вся область репозитория. Из выдачи
/// исключены `retired` и уроки с `misses > 0` (начали мешать).
#[must_use]
pub fn lessons_match(
    lessons: Vec<Lesson>,
    task_text: &str,
    paths: Option<&[String]>,
    k: usize,
) -> Vec<Lesson> {
    // Пути задачи: явные плюс извлечённые из текста (порядок не важен для
    // scope-отбора, дедупликация — только чтобы не гонять лишние глобы).
    let mut all_paths: Vec<String> = paths.map(<[String]>::to_vec).unwrap_or_default();
    for path in paths_from_text(task_text) {
        if !all_paths.contains(&path) {
            all_paths.push(path);
        }
    }

    // BM25 против statement + signature (одна строка-документ на урок).
    let docs: Vec<String> = lessons
        .iter()
        .map(|lesson| format!("{}\n{}", lesson.statement, lesson.signature.describe()))
        .collect();
    let doc_refs: Vec<&str> = docs.iter().map(String::as_str).collect();
    let scores: BTreeMap<usize, f64> = crate::kb::rank_documents(task_text, &doc_refs)
        .into_iter()
        .collect();

    let mut ranked: Vec<(u8, f64, u32, Lesson)> = Vec::new();
    for (idx, lesson) in lessons.into_iter().enumerate() {
        if lesson.status == LessonStatus::Retired || lesson.misses > 0 {
            continue;
        }
        let score = scores.get(&idx).copied().unwrap_or(0.0);
        let in_scope =
            all_paths.is_empty() || all_paths.iter().any(|path| lesson.scope.matches_path(path));
        if !in_scope && score <= 0.0 {
            continue;
        }
        ranked.push((status_rank(lesson.status), score, lesson.hits, lesson));
    }
    // Сильные первыми; при равенстве — по id (детерминизм).
    ranked.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then(b.1.total_cmp(&a.1))
            .then(b.2.cmp(&a.2))
            .then_with(|| a.3.id.cmp(&b.3.id))
    });
    ranked.truncate(k);
    ranked.into_iter().map(|(_, _, _, lesson)| lesson).collect()
}

/// Пути, упомянутые в тексте: токены вида `src/a.rs`, `tests/**`, `docs/`.
///
/// Извлекаются только относительные пути репозитория (абсолютные пути и URL
/// отбрасываются); дубликаты схлопываются, порядок — как в тексте, не более
/// [`MAX_PATHS`]. Разбор детерминирован — LLM не участвует.
#[must_use]
pub fn paths_from_text(text: &str) -> Vec<String> {
    let mut paths: Vec<String> = Vec::new();
    for raw in text.split(|c: char| {
        c.is_whitespace()
            || matches!(
                c,
                '`' | '"' | '\'' | '(' | ')' | '[' | ']' | '{' | '}' | ',' | ';' | '<' | '>' | '='
            )
    }) {
        let token = raw.trim_end_matches(['.', ':', ',', ';', ')', ']', '}', '"', '\'', '»']);
        if !is_path_like(token) {
            continue;
        }
        let normalized = token.trim_start_matches("./").replace('\\', "/");
        if !paths.contains(&normalized) {
            paths.push(normalized);
        }
        if paths.len() >= MAX_PATHS {
            break;
        }
    }
    paths
}

/// Похож ли токен на относительный путь репозитория.
fn is_path_like(token: &str) -> bool {
    let len = token.chars().count();
    if !(3..=MAX_PATH_CHARS).contains(&len) || token.starts_with('/') || token.contains("://") {
        return false;
    }
    if !token
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '.' | '_' | '-' | '*' | '?'))
    {
        return false;
    }
    // Путь — это слэш либо расширение файла (`TASK.md`); прозаические слова
    // без того и другого (ADR-087, `gate`) путями не считаются.
    token.contains('/')
        || token.rsplit_once('.').is_some_and(|(stem, ext)| {
            !stem.is_empty()
                && !ext.is_empty()
                && ext.len() <= 8
                && ext.chars().all(|c| c.is_ascii_alphanumeric())
        })
}

/// Уроки обоих реестров (репозиторный и пользовательский), дедуплицированные
/// по `id`: локальный урок приоритетнее пользовательского с тем же id.
///
/// Fail-soft: нет реестра, нет файла, битый журнал — предупреждение в tracing
/// и пустая/частичная выдача. Реестр — вспомогательная память, он не должен
/// ронять ход агента, инструмент или хук хоста.
#[must_use]
pub fn load_lessons(cfg: &Config, repo: &Path) -> Vec<Lesson> {
    let th = Thresholds::from(&cfg.lessons);
    let sources = [
        (super::repo_ledger_path(repo), "репозиторный"),
        (cfg.lessons.user_ledger_path(), "пользовательский"),
    ];
    let mut seen: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let mut out: Vec<Lesson> = Vec::new();
    for (path, label) in sources {
        match Ledger::at(path.clone()).snapshot(&th) {
            Ok(snapshot) => {
                for lesson in snapshot.lessons {
                    if seen.insert(lesson.id.clone()) {
                        out.push(lesson);
                    }
                }
            }
            Err(e) => tracing::warn!("блок уроков: {label} реестр {}: {e}", path.display()),
        }
    }
    out
}

/// Инструменты домена: `lessons_match`.
#[must_use]
pub fn tools() -> Vec<Arc<dyn Tool>> {
    vec![Arc::new(LessonsMatchTool)]
}

/// Аргументы инструмента `lessons_match`.
#[derive(Debug, Deserialize)]
struct LessonsMatchArgs {
    /// Текст задачи: отбор против `statement` и `signature`, пути — из текста.
    task: String,
    /// Явные пути задачи (объединяются с путями из текста).
    #[serde(default)]
    paths: Vec<String>,
    /// Корень репозитория с реестром (по умолчанию — cwd инструмента).
    path: Option<String>,
    /// Максимум уроков в выдаче (по умолчанию — `[lessons] k`).
    k: Option<usize>,
}

/// Инструмент `lessons_match`: префлайт-подбор уроков реестра под задачу
/// (read-only, без LLM — детерминированный отбор L4).
struct LessonsMatchTool;

#[async_trait]
impl Tool for LessonsMatchTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "lessons_match".into(),
            description: "Подобрать уроки реестра (Lesson Ledger, ADR-085) под задачу — \
                детерминированный префлайт L4 без LLM: отбор по области действия \
                (глобы путей задачи и путей из текста), BM25 текста задачи против \
                формулировки и сигнатуры урока, приоритет статусов \
                confirmed > candidate > observed. Возвращает top-K уроков с рендером в \
                Markdown. Read-only: файлов не создаёт. Используй перед упаковкой \
                handoff или запуском исполнителя, чтобы отобрать релевантные правила \
                вместо «всех уроков репозитория»."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "task": {"type": "string", "description": "Текст задачи: подбор против statement и signature, пути извлекаются из текста"},
                    "paths": {"type": "array", "items": {"type": "string"}, "description": "Явные пути задачи (глобы или файлы); объединяются с путями из текста"},
                    "path": {"type": "string", "description": "Корень репозитория с реестром (по умолчанию — рабочий каталог)"},
                    "k": {"type": "integer", "description": "Максимум уроков в выдаче (по умолчанию — [lessons] k конфига)"}
                },
                "required": ["task"]
            }),
        }
    }

    async fn call(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let args: LessonsMatchArgs = serde_json::from_value(args)
            .map_err(|e| HarnessError::Tool(format!("lessons_match: невалидные аргументы: {e}")))?;
        let repo = match args.path {
            Some(raw) if !Path::new(&raw).is_absolute() => ctx.cwd.join(raw),
            Some(raw) => Path::new(&raw).to_path_buf(),
            None => ctx.cwd.clone(),
        };
        let k = args.k.unwrap_or(ctx.config.lessons.k);
        let lessons = load_lessons(&ctx.config, &repo);
        let matched = lessons_match(lessons, &args.task, Some(&args.paths), k);
        if matched.is_empty() {
            return Ok(ToolOutput::ok(
                "По задаче не подобрано ни одного урока: реестр пуст, уроки вне \
                 области задачи или нерелевантны тексту.",
            ));
        }
        let report = super::render::render(&matched);
        let mut buf = String::new();
        buf.push_str(&report);
        Ok(ToolOutput::ok(buf))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lessons::{
        Evidence, LessonSource, Outcome, RecordInput, Scope, ScopeLevel, Signature,
    };

    /// Урок реестра заданного статуса/счётчиков (конструктор тестов).
    fn lesson(
        code: &str,
        statement: &str,
        scope: Scope,
        fails: u32,
        hits: u32,
        misses: u32,
    ) -> Lesson {
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
        .with_scope(scope);
        let mut lesson = Lesson::from_input(&input);
        lesson.fails = fails;
        lesson.hits = hits;
        lesson.misses = misses;
        lesson.status = lesson.derived_status(&Thresholds::default());
        lesson
    }

    fn module_scope(glob: &str) -> Scope {
        Scope::new(ScopeLevel::Module, vec![glob.to_string()])
    }

    /// Отбор по scope: урок модуля `src/**` не попадает в задачу про `docs/`,
    /// а урок уровня репозитория попадает всегда.
    #[test]
    fn scope_selects_module_vs_repo() {
        let lessons = vec![
            lesson("G5", "про __all__", module_scope("src/**"), 1, 0, 0),
            lesson("G6", "про docs", module_scope("docs/**"), 1, 0, 0),
            lesson("G7", "репозиторное правило", Scope::repo(), 1, 0, 0),
        ];
        let src = lessons_match(lessons.clone(), "поправь src/lessons/ledger.rs", None, 5);
        let codes: Vec<String> = src.iter().map(|l| l.signature.code.clone()).collect();
        assert!(codes.contains(&"G5".to_string()), "{codes:?}");
        assert!(codes.contains(&"G7".to_string()), "{codes:?}");
        assert!(!codes.contains(&"G6".to_string()), "{codes:?}");

        // Пустой список путей (и пустой текст) — вся область репозитория.
        let all = lessons_match(lessons, "", None, 5);
        assert_eq!(all.len(), 3, "без путей область — весь репозиторий");
    }

    /// Ровно K результатов и стабильный порядок (сильные статусы первыми,
    /// при равенстве — по id), независимо от порядка входа.
    #[test]
    fn exactly_k_results_in_stable_status_order() {
        let weak = lesson("G9", "наблюдение", module_scope("src/**"), 1, 0, 0);
        let candidate = lesson("G6", "кандидат", module_scope("src/**"), 2, 0, 0);
        let mut strong = lesson("G5", "подтверждённый", module_scope("src/**"), 2, 2, 0);
        strong.status = LessonStatus::Confirmed;

        let first = lessons_match(
            vec![weak.clone(), candidate.clone(), strong.clone()],
            "любая задача",
            None,
            5,
        );
        let second = lessons_match(
            vec![strong.clone(), candidate.clone(), weak.clone()],
            "любая задача",
            None,
            5,
        );
        assert_eq!(first, second, "порядок входа не влияет на порядок выдачи");
        assert_eq!(
            first[0].status,
            LessonStatus::Confirmed,
            "подтверждённый первым"
        );
        assert_eq!(first[1].status, LessonStatus::Candidate, "кандидат вторым");
        assert_eq!(first[2].status, LessonStatus::Observed);

        // Ровно K: обрезка до двух.
        let two = lessons_match(vec![weak, candidate, strong], "любая задача", None, 2);
        assert_eq!(two.len(), 2);
    }

    /// Детерминизм при повторе: тот же вход — та же выдача байт-в-байт.
    #[test]
    fn matching_is_deterministic_on_repeat() {
        let lessons = vec![
            lesson("G5", "правило A", module_scope("src/**"), 1, 0, 0),
            lesson("G6", "правило B", module_scope("src/**"), 1, 0, 0),
        ];
        let a = lessons_match(lessons.clone(), "задача про src/a.rs", None, 5);
        let b = lessons_match(lessons.clone(), "задача про src/a.rs", None, 5);
        assert_eq!(a, b);
    }

    /// BM25 против signature: урок с пустой формулировкой, но чей код (`G5`)
    /// встречается в тексте задачи, поднимается в выдачу даже вне области.
    #[test]
    fn signature_match_rescues_statementless_lesson() {
        let code_lesson = lesson("G5", "", module_scope("other/**"), 1, 0, 0);
        let scoped = lesson(
            "G6",
            "миграция обязана быть обратимой",
            module_scope("src/**"),
            1,
            0,
            0,
        );
        // Пути ограничены `src/**`, поэтому code-урок вне области; текст
        // задачи упоминает `G5` — урок поднимается по сигнатуре и идёт первым
        // (у scoped-урока BM25-вес нулевой).
        let matched = lessons_match(
            vec![code_lesson, scoped],
            "про гейт G5 поправь src/a.rs",
            None,
            5,
        );
        assert_eq!(
            matched[0].signature.code, "G5",
            "урок по коду сигнатуры первым: {matched:?}"
        );
    }

    /// Выведенные и «начавшие мешать» уроки из выдачи исключены.
    #[test]
    fn retired_and_missing_lessons_are_excluded() {
        let mut retired = lesson("G5", "правило", module_scope("src/**"), 3, 0, 2);
        retired.status = LessonStatus::Retired;
        let noisy = lesson("G6", "шумное", module_scope("src/**"), 2, 0, 1);
        let clean = lesson("G7", "честное", module_scope("src/**"), 1, 0, 0);
        let matched = lessons_match(vec![retired, noisy, clean], "задача", None, 5);
        let codes: Vec<String> = matched.iter().map(|l| l.signature.code.clone()).collect();
        assert_eq!(codes, vec!["G7".to_string()], "{codes:?}");
    }

    /// Пути из текста: относительные пути и глобы — да, URL/абсолютные/проза — нет.
    #[test]
    fn paths_from_text_extracts_repo_paths_only() {
        let text = "Поправь `src/lessons/ledger.rs` и tests/cli.rs, см. \
                    https://example.com/x/y.rs и /etc/passwd; TASK.md не трогай. \
                    Ещё src/** и docs/adr/ADR-087.md.";
        let paths = paths_from_text(text);
        for want in [
            "src/lessons/ledger.rs",
            "tests/cli.rs",
            "TASK.md",
            "src/**",
            "docs/adr/ADR-087.md",
        ] {
            assert!(paths.contains(&want.to_string()), "нет {want}: {paths:?}");
        }
        assert!(
            !paths.iter().any(|p| p.contains("example.com")),
            "{paths:?}"
        );
        assert!(!paths.iter().any(|p| p.starts_with('/')), "{paths:?}");
        assert!(!paths.contains(&"и".to_string()), "{paths:?}");
    }
}
