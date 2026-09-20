//! Дельта-спецификации как state machine (по OpenSpec): change-центричные
//! изменения `propose → (apply) → archive`; дельта описывает только изменение
//! относительно текущей истины (ADDED/MODIFIED/REMOVED).
//!
//! Каталог изменений — `changes/` в репозитории: предложенные на верхнем
//! уровне, заархивированные — в `changes/archive/`.
//!
//! [`guard`] — CI-гейт прямых правок спайна мимо дельты: изменённые файлы под
//! защищёнными путями (по умолчанию `model/`, `ARCHITECTURE-SPINE.md`,
//! `CONSTRAINTS.yaml`) обязаны упоминаться в активной дельте, иначе FAIL.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::control::LintIssue;
use crate::error::{HarnessError, Result};
use crate::llm::ToolSpec;
use crate::tool::{Tool, ToolContext, ToolOutput};

/// Шаблон дельты.
const DELTA_TEMPLATE: &str = "# Дельта: {name}
\
- Route: Fast|Standard (Critical — полный Solutioning, дельты недостаточно)
- Created: {date}

## Проблема

<что и зачем меняем — 2-3 предложения>

## ADDED

- <новые требования с критериями EARS: When <событие>, the <система> shall <реакция>>

## MODIFIED

- <изменяемые требования: было → стало, причина>

## REMOVED

- <удаляемое: что, замена, план миграции потребителей>

## План отката

<как откатываем изменение>

## Критерии приёмки

- [ ] <проверяемый критерий>
";

/// Создаёт каркас дельты `changes/<name>/DELTA.md`.
///
/// # Errors
/// Каталог существует, ошибка записи.
pub fn new(repo: &Path, name: &str) -> Result<PathBuf> {
    let dir = repo.join("changes").join(name);
    if dir.exists() || repo.join("changes/archive").join(name).exists() {
        return Err(HarnessError::Control(format!(
            "дельта '{name}' уже существует (активная или в архиве): {}",
            dir.display()
        )));
    }
    std::fs::create_dir_all(&dir).map_err(|e| HarnessError::io(&dir, e))?;
    let path = dir.join("DELTA.md");
    let content = DELTA_TEMPLATE.replace("{name}", name).replace(
        "{date}",
        &chrono::Local::now().format("%Y-%m-%d").to_string(),
    );
    std::fs::write(&path, content).map_err(|e| HarnessError::io(&path, e))?;
    Ok(path)
}

/// Статус дельты.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeltaStatus {
    /// Предложена (changes/<name>).
    Proposed,
    /// В архиве (changes/archive/<name>).
    Archived,
}

/// Сводка дельты.
#[derive(Debug, Clone)]
pub struct DeltaInfo {
    /// Имя.
    pub name: String,
    /// Статус.
    pub status: DeltaStatus,
    /// Путь к DELTA.md.
    pub path: PathBuf,
}

/// Список дельт (предложенные и архивные).
#[must_use]
pub fn list(repo: &Path) -> Vec<DeltaInfo> {
    let mut out = Vec::new();
    for (dir, status) in [
        (repo.join("changes"), DeltaStatus::Proposed),
        (repo.join("changes/archive"), DeltaStatus::Archived),
    ] {
        if let Ok(rd) = std::fs::read_dir(&dir) {
            for e in rd.flatten() {
                let p = e.path();
                if !p.is_dir() || p.file_name().is_some_and(|n| n == "archive") {
                    continue;
                }
                let delta = p.join("DELTA.md");
                if delta.is_file() {
                    out.push(DeltaInfo {
                        name: e.file_name().to_string_lossy().to_string(),
                        status,
                        path: delta,
                    });
                }
            }
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Валидация структуры дельты: обязательные секции, заглушки, пустые блоки.
///
/// # Errors
/// DELTA.md не читается.
pub fn validate(repo: &Path, name: &str) -> Result<Vec<LintIssue>> {
    let path = find_delta(repo, name)?;
    let text = std::fs::read_to_string(&path).map_err(|e| HarnessError::io(&path, e))?;
    let mut issues = Vec::new();
    for section in [
        "## Проблема",
        "## ADDED",
        "## MODIFIED",
        "## REMOVED",
        "## План отката",
        "## Критерии приёмки",
    ] {
        if !text.contains(section) {
            issues.push(LintIssue {
                file: path.clone(),
                line: 0,
                rule: "missing_section".into(),
                message: format!("нет секции «{section}»"),
                severity: "error".into(),
                ..LintIssue::default()
            });
        }
    }
    // Пустые секции-заглушки из шаблона. Правило вынесено в [`crate::stubs`]
    // (общий знаменатель с семантикой бандла), уровень строгости — тот же,
    // что в 0.3.3: вердикт `delta validate` не меняется.
    for (n, line) in text.lines().enumerate() {
        if crate::stubs::is_template_stub(line) {
            issues.push(LintIssue {
                file: path.clone(),
                line: n + 1,
                rule: "stub_marker".into(),
                message: format!(
                    "незаполненное место: {}",
                    line.trim().chars().take(60).collect::<String>()
                ),
                severity: "warn".into(),
                ..LintIssue::default()
            });
        }
    }
    // Все три блока пустые — дельта ни о чём.
    let has_content = ["## ADDED", "## MODIFIED", "## REMOVED"].iter().any(|s| {
        section_body(&text, s)
            .lines()
            .any(|l| l.trim().starts_with('-') && !l.contains('<'))
    });
    if !has_content {
        issues.push(LintIssue {
            file: path.clone(),
            line: 0,
            rule: "empty_delta".into(),
            message: "ADDED/MODIFIED/REMOVED пусты — дельта без содержания".into(),
            severity: "error".into(),
            ..LintIssue::default()
        });
    }
    Ok(issues)
}

/// Тело секции между заголовком `## X` и следующим `## `.
fn section_body<'a>(text: &'a str, section: &str) -> &'a str {
    let Some(start) = text.find(section) else {
        return "";
    };
    let rest = &text[start + section.len()..];
    let end = rest.find("\n## ").unwrap_or(rest.len());
    &rest[..end]
}

/// Архивирует дельту (после apply): валидация → перенос в `changes/archive/`.
///
/// # Errors
/// Дельта не найдена, не прошла валидацию (error-находки), ошибка переноса.
pub fn archive(repo: &Path, name: &str) -> Result<PathBuf> {
    let issues = validate(repo, name)?;
    let errors: Vec<&LintIssue> = issues.iter().filter(|i| i.severity == "error").collect();
    if !errors.is_empty() {
        return Err(HarnessError::Control(format!(
            "дельта '{name}' не прошла валидацию: {} error-находок",
            errors.len()
        )));
    }
    let src = repo.join("changes").join(name);
    let dst_dir = repo.join("changes/archive");
    std::fs::create_dir_all(&dst_dir).map_err(|e| HarnessError::io(&dst_dir, e))?;
    let dst = dst_dir.join(name);
    if dst.exists() {
        return Err(HarnessError::Control(format!(
            "в архиве уже есть '{name}' — номера/имена не переиспользуются"
        )));
    }
    std::fs::rename(&src, &dst).map_err(|e| HarnessError::io(&dst, e))?;
    Ok(dst.join("DELTA.md"))
}

/// Находит DELTA.md по имени (предложенная или архивная).
fn find_delta(repo: &Path, name: &str) -> Result<PathBuf> {
    for base in [
        repo.join("changes").join(name),
        repo.join("changes/archive").join(name),
    ] {
        let p = base.join("DELTA.md");
        if p.is_file() {
            return Ok(p);
        }
    }
    Err(HarnessError::Control(format!(
        "дельта '{name}' не найдена в {}/changes",
        repo.display()
    )))
}

/// Защищаемые пути по умолчанию гейта прямых правок спайна (`delta guard`):
/// модель архитектуры и корневые spine-артефакты. Любой явный `--protect`
/// заменяет этот список целиком.
pub const DEFAULT_PROTECTED: [&str; 3] = ["model/", "ARCHITECTURE-SPINE.md", "CONSTRAINTS.yaml"];

/// Отчёт гейта прямых правок спайна.
#[derive(Debug, Clone)]
pub struct GuardReport {
    /// База diff, как передана в git.
    pub base: String,
    /// Всего изменённых файлов по diff.
    pub changed: usize,
    /// Изменённые защищённые файлы.
    pub protected_changed: Vec<String>,
    /// Покрытые правки: (файл, имя активной дельты) — первая из упомянувших
    /// (совместимость JSON-вердикта; полный список — в `mentions`).
    pub covered: Vec<(String, String)>,
    /// Нарушения: защищённые файлы без упоминания в активных дельтах.
    pub violations: Vec<String>,
    /// Гейт пройден (нет непокрытых правок защищённых путей).
    pub passed: bool,
    /// Число активных дельт (`changes/<name>/DELTA.md` в статусе Proposed):
    /// контекст честности вывода — нарушение при нуле дельт означает
    /// «правку нечем покрыть», а не «дельта не та».
    pub active_deltas: usize,
    /// Покрытие каждого изменённого защищённого файла: (файл, имена ВСЕХ
    /// активных дельт, его упоминающих; пустой список — нарушение).
    pub mentions: Vec<(String, Vec<String>)>,
    /// Чем именно покрыт файл: (файл, причина) — «по пути», «по id NFR-005».
    /// Аддитивное поле (0.3.4, Н4): архитектору важно видеть, засчитано ли
    /// упоминание по идентификатору или по полному пути.
    pub reasons: Vec<(String, String)>,
}

/// Защищён ли путь: совпадение с записью-файлом или вхождение в каталог-префикс
/// (`model/` матчит `model/adr/ADR-003.md`; запись без слэша трактуется и как
/// каталог: `model` тоже матчит).
fn is_protected(path: &str, protected: &[String]) -> bool {
    protected.iter().any(|entry| {
        let e = entry.trim_end_matches('/');
        path == e || path.starts_with(&format!("{e}/"))
    })
}

/// Вхождение `needle` в `haystack` как ОТДЕЛЬНОГО слова: соседние символы —
/// не буква, не цифра и не дефис. `NFR-0051` не содержит слова `NFR-005`.
fn contains_word(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return false;
    }
    let mut from = 0;
    while let Some(pos) = haystack[from..].find(needle) {
        let start = from + pos;
        let end = start + needle.len();
        let before_ok = haystack[..start]
            .chars()
            .next_back()
            .is_none_or(|c| !(c.is_alphanumeric() || c == '-'));
        let after_ok = haystack[end..]
            .chars()
            .next()
            .is_none_or(|c| !(c.is_alphanumeric() || c == '-'));
        if before_ok && after_ok {
            return true;
        }
        from = end;
    }
    false
}

/// Идентификатор сущности из frontmatter файла модели (`model/NFR-005-*.md` →
/// `NFR-005`): строка `id: <ID>`. Не модель — `None`.
fn entity_id(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    for line in text.lines().take(40) {
        let t = line.trim();
        let Some(rest) = t.strip_prefix("id:") else {
            continue;
        };
        let id = rest.trim().trim_matches('"').trim_matches('\'').trim();
        if !id.is_empty() {
            return Some(id.to_string());
        }
        return None;
    }
    None
}

/// Идентификатор сущности, выведенный ИЗ ИМЕНИ файла: `NFR-005-recovery.md` →
/// `NFR-005` (префикс вида `<ЛАТИНИЦА>-<ЦИФРЫ>`). `None` — имя не в этой форме.
fn id_from_stem(stem: &str) -> Option<String> {
    let (head, tail) = stem.split_once('-')?;
    if head.is_empty() || !head.chars().all(|c| c.is_ascii_alphabetic()) {
        return None;
    }
    let digits: String = tail.chars().take_while(char::is_ascii_digit).collect();
    if digits.is_empty() {
        return None;
    }
    Some(format!("{head}-{digits}"))
}

/// Чем текст дельты покрывает файл: `None` — не упоминает.
///
/// Признаётся (в порядке убывания точности): полный относительный путь; имя
/// файла; стем (от 4 символов); путь без расширения и слага-суффикса
/// (`model/NFR-005`); идентификатор сущности из frontmatter или имени файла
/// (`NFR-005`) — последние два как ОТДЕЛЬНОЕ слово, иначе `NFR-0051` «покрывал»
/// бы `NFR-005`.
fn delta_mentions(body: &str, path: &str) -> Option<String> {
    if body.contains(path) {
        return Some("по пути".to_string());
    }
    let p = Path::new(path);
    let name = p.file_name().map(|n| n.to_string_lossy().into_owned());
    if let Some(name) = &name {
        if !name.is_empty() && body.contains(name.as_str()) {
            return Some("по имени файла".to_string());
        }
    }
    let stem = p.file_stem().map(|s| s.to_string_lossy().into_owned());
    if let Some(stem) = &stem {
        // Стем — тоже идентификатор: `NFR-0051` не должен «покрывать»
        // файл `NFR-005.md` (Н4б).
        if stem.chars().count() >= 4 && contains_word(body, stem) {
            return Some(format!("по стему '{stem}'"));
        }
    }
    // Идентификатор сущности: сначала из frontmatter (истина), затем из имени.
    let id = p
        .is_file()
        .then(|| entity_id(p))
        .flatten()
        .or_else(|| stem.as_deref().and_then(id_from_stem));
    if let Some(id) = id {
        if contains_word(body, &id) {
            return Some(format!("по id {id}"));
        }
        // Путь со слагом: `model/NFR-005-…` → `model/NFR-005`.
        if let Some(parent) = p.parent() {
            let short = parent.join(&id);
            let short = short.to_string_lossy().replace('\\', "/");
            if contains_word(body, &short) {
                return Some(format!("по пути '{short}' (id {id})"));
            }
        }
    }
    None
}

/// Первая непустая строка stderr git без префикса «fatal:» — краткая причина
/// для ошибки команды. Сырой stderr целиком не проксируем (D9): там
/// многострочная справка использования.
fn git_stderr_reason(stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr);
    let reason = text.lines().map(str::trim).find(|l| !l.is_empty()).map_or(
        "git завершился с ошибкой без сообщения",
        |l| l.strip_prefix("fatal:").map_or(l, str::trim),
    );
    reason.chars().take(160).collect()
}

/// Гейт прямых правок спайна мимо дельты (CI-запрет «прямых коммитов в model/
/// мимо changes/»): каждый изменённый защищённый файл обязан упоминаться
/// (путём или именем) в теле хотя бы одной АКТИВНОЙ дельты
/// `changes/<name>/DELTA.md`; архивные дельты не засчитываются.
///
/// Изменённые файлы — `git diff --name-only <base>` (дефолт `HEAD`: staged +
/// unstaged рабочего дерева; untracked-файлы git-diff не показывает — для CI
/// передавайте базу вида `origin/main...HEAD`).
///
/// # Errors
/// `git` недоступен или вернул ненулевой код (не репозиторий, плохая база).
pub fn guard(repo: &Path, base: Option<&str>, protect: &[String]) -> Result<GuardReport> {
    let protected: Vec<String> = if protect.is_empty() {
        DEFAULT_PROTECTED.iter().map(|s| (*s).to_string()).collect()
    } else {
        protect.to_vec()
    };
    let base = base.unwrap_or("HEAD").to_string();
    let out = std::process::Command::new("git")
        // `core.quotepath=false`: имена сущностей в кейсах русские, а git по
        // умолчанию отдаёт такие пути экранированными (`"model/CMP-001-\320…"`)
        // — путь перестаёт начинаться с `model/`, и правка мимо дельты
        // выглядела как «защищённых среди них: 0».
        .args(["-c", "core.quotepath=false"])
        .arg("-C")
        .arg(repo)
        .args(["diff", "--name-only", &base])
        .output()
        .map_err(|e| HarnessError::Control(format!("git не запустился: {e}")))?;
    if !out.status.success() {
        return Err(HarnessError::Control(format!(
            "git diff --name-only {base}: {}",
            git_stderr_reason(&out.stderr)
        )));
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    let mut changed: Vec<String> = stdout
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_owned)
        .collect();
    // Н4: `git diff` не показывает НЕотслеживаемые файлы, поэтому вердикт
    // зависел от того, сделан ли `git add` — до него guard пропускал новый
    // `model/AD-009-*.md`, после — краснел. Вердикт обязан совпадать.
    if let Ok(untracked) = std::process::Command::new("git")
        .args(["-c", "core.quotepath=false"])
        .arg("-C")
        .arg(repo)
        .args(["ls-files", "--others", "--exclude-standard"])
        .output()
    {
        if untracked.status.success() {
            changed.extend(
                String::from_utf8_lossy(&untracked.stdout)
                    .lines()
                    .map(str::trim)
                    .filter(|l| !l.is_empty())
                    .map(str::to_owned),
            );
        }
    }
    changed.sort();
    changed.dedup();

    // Тела активных дельт (архив — уже влитая истина, покрытием не считается).
    let mut active: Vec<(String, String)> = Vec::new();
    for d in list(repo) {
        if d.status != DeltaStatus::Proposed {
            continue;
        }
        let body = std::fs::read_to_string(&d.path).map_err(|e| HarnessError::io(&d.path, e))?;
        active.push((d.name, body));
    }

    let mut protected_changed = Vec::new();
    let mut covered = Vec::new();
    let mut violations = Vec::new();
    let mut mentions = Vec::new();
    let mut reasons = Vec::new();
    for file in changed.iter().filter(|f| is_protected(f, &protected)) {
        protected_changed.push(file.clone());
        let by: Vec<(String, Option<String>)> = active
            .iter()
            .map(|(name, body)| (name.clone(), delta_mentions(body, file)))
            .filter(|(_, reason)| reason.is_some())
            .collect();
        let by_names: Vec<String> = by.iter().map(|(name, _)| name.clone()).collect();
        match by.first() {
            Some((name, reason)) => {
                covered.push((file.clone(), name.clone()));
                if let Some(reason) = reason {
                    reasons.push((file.clone(), reason.clone()));
                }
            }
            None => violations.push(file.clone()),
        }
        mentions.push((file.clone(), by_names));
    }
    let passed = violations.is_empty();
    Ok(GuardReport {
        base,
        changed: changed.len(),
        protected_changed,
        covered,
        violations,
        passed,
        active_deltas: active.len(),
        mentions,
        reasons,
    })
}

/// Текстовый рендер отчёта гейта (в стиле остальных delta-команд): сводка,
/// по каждому изменённому защищённому файлу — статус его упоминания в
/// активных дельтах (все дельты поимённо; при полном их отсутствии — честное
/// «активных дельт нет», а не обтекаемое «не упоминается»).
#[must_use]
pub fn render_guard(report: &GuardReport) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    let _ = writeln!(out, "Гейт прямых правок спайна (база: {})", report.base);
    let _ = writeln!(
        out,
        "Изменённых файлов: {}, защищённых среди них: {} (активных дельт: {})",
        report.changed,
        report.protected_changed.len(),
        report.active_deltas
    );
    if !report.protected_changed.is_empty() {
        out.push('\n');
        for (file, deltas) in &report.mentions {
            match deltas.as_slice() {
                [] => {
                    let _ = writeln!(
                        out,
                        "[error] {file} — не упоминается ни в одной активной дельте{}",
                        if report.active_deltas == 0 {
                            " (активных дельт нет)".to_string()
                        } else {
                            format!(" (активных дельт: {})", report.active_deltas)
                        }
                    );
                    let _ = writeln!(
                        out,
                        "  → оформите правку дельтой: arch-be delta new <name>, опишите изменение \
                         в changes/<name>/DELTA.md (архивные дельты не засчитываются)"
                    );
                }
                [single] => {
                    let why = report
                        .reasons
                        .iter()
                        .find(|(f, _)| f == file)
                        .map_or(String::new(), |(_, r)| format!(" ({r})"));
                    let _ = writeln!(out, "[ok] {file} — покрыт активной дельтой '{single}'{why}");
                }
                many => {
                    let quoted: Vec<String> = many.iter().map(|d| format!("'{d}'")).collect();
                    let _ = writeln!(
                        out,
                        "[ok] {file} — покрыт активными дельтами: {}",
                        quoted.join(", ")
                    );
                }
            }
        }
    }
    let _ = writeln!(
        out,
        "\nИтог: {}",
        if report.passed {
            if report.protected_changed.is_empty() {
                "PASS — защищённые пути не затронуты"
            } else {
                "PASS — все правки спайна покрыты активными дельтами"
            }
        } else {
            "FAIL — правки спайна мимо дельты (exit 1)"
        }
    );
    out
}

// ---------------------------------------------------------------------------
// Агентные инструменты `delta_guard` / `delta_propose` (мост в MCP, транш 1)
// ---------------------------------------------------------------------------

/// Инструменты домена: `delta_guard` (read-only гейт), `delta_propose`
/// (создание скелета дельты — пишущий, в MCP только под `--rw`).
#[must_use]
pub fn tools() -> Vec<Arc<dyn Tool>> {
    vec![Arc::new(DeltaGuardTool), Arc::new(DeltaProposeTool)]
}

/// Инструмент `delta_guard`: гейт прямых правок спайна мимо дельты —
/// JSON-вердикт `{passed, violations, covered, summary}`.
pub struct DeltaGuardTool;

#[derive(Debug, Deserialize)]
struct DeltaGuardArgs {
    /// Репозиторий (дефолт — текущий каталог).
    path: Option<String>,
    /// База diff (по умолчанию HEAD — staged+unstaged рабочего дерева).
    base: Option<String>,
    /// Защищаемые пути/префиксы (заменяют дефолт [`DEFAULT_PROTECTED`]).
    protect: Option<Vec<String>>,
}

#[async_trait]
impl Tool for DeltaGuardTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "delta_guard".into(),
            description: "Гейт прямых правок спайна мимо дельты (модель 5.2): каждый изменённый \
                          файл под защищёнными путями (по умолчанию model/, \
                          ARCHITECTURE-SPINE.md, CONSTRAINTS.yaml) обязан упоминаться в активной \
                          дельте changes/<name>/DELTA.md. Ответ — JSON: passed + violations \
                          (непокрытые правки) + covered + mentions (все дельты по каждому \
                          файлу) + active_deltas + summary; passed=false — основание отказать \
                          изменению"
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Репозиторий (по умолчанию — текущий каталог)"},
                    "base": {"type": "string", "description": "База diff (по умолчанию HEAD; для CI — напр. origin/main...HEAD)"},
                    "protect": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "Защищаемые пути/префиксы (непустой список заменяет дефолт целиком)"
                    }
                }
            }),
        }
    }

    async fn call(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let args: DeltaGuardArgs = match serde_json::from_value(args) {
            Ok(a) => a,
            Err(e) => {
                return Ok(ToolOutput::err(format!(
                    "delta_guard: невалидные аргументы: {e}"
                )));
            }
        };
        let repo = ctx.resolve(args.path.as_deref().unwrap_or("."));
        let protect = args.protect.unwrap_or_default();
        let report = match guard(&repo, args.base.as_deref(), &protect) {
            Ok(r) => r,
            Err(e) => return Ok(ToolOutput::err(format!("delta_guard: {e}"))),
        };
        let summary = format!(
            "Гейт прямых правок спайна (база: {}): изменённых файлов {}, защищённых {}, \
             непокрытых нарушений {} (активных дельт: {})",
            report.base,
            report.changed,
            report.protected_changed.len(),
            report.violations.len(),
            report.active_deltas
        );
        let verdict = json!({
            "tool": "delta_guard",
            "passed": report.passed,
            "base": report.base,
            "changed": report.changed,
            "protected_changed": report.protected_changed,
            "covered": report.covered.iter().map(|(f, d)| json!({"file": f, "delta": d})).collect::<Vec<_>>(),
            "violations": report.violations,
            // Аддитивные поля (SDK-контракт v1): полный статус упоминания
            // каждого защищённого файла — все активные дельты, а не первая.
            "active_deltas": report.active_deltas,
            "mentions": report.mentions.iter().map(|(f, ds)| json!({"file": f, "deltas": ds})).collect::<Vec<_>>(),
            "summary": summary,
        });
        // Сериализация собранного объекта не падает; запасной вариант — компактная форма.
        let text = serde_json::to_string_pretty(&verdict).unwrap_or_else(|_| verdict.to_string());
        Ok(ToolOutput::ok(text))
    }
}

/// Инструмент `delta_propose`: скелет новой дельты `changes/<name>/DELTA.md`
/// (пишущий: в MCP-режиме отдаётся только под `--rw`).
pub struct DeltaProposeTool;

#[derive(Debug, Deserialize)]
struct DeltaProposeArgs {
    /// Имя изменения (kebab-case).
    name: String,
    /// Репозиторий (дефолт — текущий каталог).
    path: Option<String>,
}

#[async_trait]
impl Tool for DeltaProposeTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "delta_propose".into(),
            description: "Создать скелет дельты changes/<name>/DELTA.md (шаблон: Проблема, \
                          ADDED/MODIFIED/REMOVED, План отката, Критерии приёмки) — начало \
                          change-центричного изменения спайна. Пишет в рабочий каталог"
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "name": {"type": "string", "description": "Имя изменения (kebab-case)"},
                    "path": {"type": "string", "description": "Репозиторий (по умолчанию — текущий каталог)"}
                },
                "required": ["name"]
            }),
        }
    }

    async fn call(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let args: DeltaProposeArgs = match serde_json::from_value(args) {
            Ok(a) => a,
            Err(e) => {
                return Ok(ToolOutput::err(format!(
                    "delta_propose: невалидные аргументы: {e}"
                )));
            }
        };
        let repo = ctx.resolve(args.path.as_deref().unwrap_or("."));
        match new(&repo, &args.name) {
            Ok(path) => {
                let verdict = json!({
                    "tool": "delta_propose",
                    "created": path.display().to_string(),
                    "summary": format!("Дельта '{}' создана: {}", args.name, path.display()),
                });
                // Сериализация собранного объекта не падает; запасной вариант — компактная форма.
                let text =
                    serde_json::to_string_pretty(&verdict).unwrap_or_else(|_| verdict.to_string());
                Ok(ToolOutput::ok(text))
            }
            Err(e) => Ok(ToolOutput::err(format!("delta_propose: {e}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Заполняет дельту рабочим содержимым (все обязательные секции + буллеты).
    fn fill_delta(path: &Path) {
        std::fs::write(
            path,
            "# Дельта\n\n## Проблема\nТаймаут велик.\n\n## ADDED\n- Требование: таймаут авторизации 500 мс. Критерий: When запрос, the оркестратор shall ответить ≤ 500 мс\n\n## MODIFIED\n\n## REMOVED\n\n## План отката\nrevert флага\n\n## Критерии приёмки\n- [ ] тест таймаута\n",
        )
        .expect("fill");
    }

    #[test]
    fn delta_full_cycle_new_validate_archive_and_name_guard() {
        let tmp = tempfile::tempdir().expect("tmp");
        let repo = tmp.path();
        // new: шаблон создан, висит в Proposed.
        let path = new(repo, "saga-pilot").expect("new");
        assert!(path.is_file());
        let infos = list(repo);
        assert_eq!(infos.len(), 1);
        assert_eq!(infos[0].status, DeltaStatus::Proposed);
        // Дубликат активного имени — отказ.
        assert!(new(repo, "saga-pilot").is_err(), "активное имя занято");
        // Свежий шаблон не валиден (пустые блоки ADDED/MODIFIED/REMOVED).
        let issues = validate(repo, "saga-pilot").expect("validate");
        assert!(
            issues
                .iter()
                .any(|i| i.rule == "empty_delta" && i.severity == "error"),
            "issues: {issues:?}"
        );
        // Архивация невалидной дельты — отказ.
        assert!(
            archive(repo, "saga-pilot").is_err(),
            "error-находки блокируют архив"
        );
        // Заполняем — валидация чиста (error-находок нет), архивируется.
        fill_delta(&path);
        let issues = validate(repo, "saga-pilot").expect("validate2");
        assert!(
            issues.iter().all(|i| i.severity != "error"),
            "остались error: {issues:?}"
        );
        let archived = archive(repo, "saga-pilot").expect("archive");
        assert!(archived.is_file());
        let infos = list(repo);
        assert_eq!(infos[0].status, DeltaStatus::Archived);
        // Имя в архиве занято навсегда: ни new, ни повторный archive.
        let err = new(repo, "saga-pilot").expect_err("имя в архиве защищено");
        assert!(err.to_string().contains("архиве"), "{err}");
        assert!(archive(repo, "saga-pilot").is_err());
        // Несуществующая дельта — внятная ошибка.
        assert!(validate(repo, "ghost").is_err());
    }

    /// git в каталоге с тестовой идентичностью коммиттера.
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

    /// Репо-фикстура гейта: git init + защищённые файлы и код, один коммит.
    fn make_guard_repo(dir: &Path) {
        std::fs::create_dir_all(dir).expect("mkdir");
        git(dir, &["init", "-q"]);
        std::fs::create_dir_all(dir.join("model/adr")).expect("mkdir model");
        std::fs::write(dir.join("model/adr/ADR-003.md"), "# ADR-003\n").expect("adr");
        std::fs::write(dir.join("ARCHITECTURE-SPINE.md"), "# Spine\n").expect("spine");
        std::fs::create_dir_all(dir.join("src")).expect("mkdir src");
        std::fs::write(dir.join("src/main.rs"), "fn main() {}\n").expect("src");
        git(dir, &["add", "."]);
        git(dir, &["commit", "-q", "-m", "init"]);
    }

    #[test]
    fn guard_flags_protected_change_without_delta() {
        let tmp = tempfile::tempdir().expect("tmp");
        let repo = tmp.path().join("repo");
        make_guard_repo(&repo);
        // Прямая правка model/ без дельты (незакоммиченная — база HEAD ловит).
        std::fs::write(repo.join("model/adr/ADR-003.md"), "# ADR-003 v2\n").expect("edit");
        let report = guard(&repo, None, &[]).expect("guard");
        assert!(!report.passed);
        assert_eq!(report.violations, vec!["model/adr/ADR-003.md".to_string()]);
        assert!(report.covered.is_empty());
        let text = render_guard(&report);
        assert!(text.contains("arch-be delta new"), "{text}");
        assert!(text.contains("FAIL"), "{text}");
    }

    /// Н4а: вердикт guard не зависит от того, сделан ли `git add`.
    /// `git diff --name-only HEAD` неотслеживаемые файлы не показывает.
    #[test]
    fn guard_sees_untracked_protected_files() {
        let tmp = tempfile::tempdir().expect("tmp");
        let repo = tmp.path().join("repo");
        make_guard_repo(&repo);
        // Новый защищённый файл, ещё не в индексе.
        std::fs::write(repo.join("model/AD-009-tehnologii.md"), "# AD-009\n").expect("write");
        let before = guard(&repo, None, &[]).expect("guard");
        assert!(
            before
                .violations
                .contains(&"model/AD-009-tehnologii.md".to_string()),
            "до git add: {:?}",
            before.violations
        );
        assert!(!before.passed);
        git(&repo, &["add", "-A"]);
        let after = guard(&repo, None, &[]).expect("guard");
        assert_eq!(
            before.violations, after.violations,
            "вердикт обязан совпадать до и после git add"
        );
        assert_eq!(before.passed, after.passed);
    }

    /// Н4б: упоминание по идентификатору сущности засчитывается, и отчёт
    /// говорит, чем именно покрыт файл.
    #[test]
    fn guard_accepts_entity_id_mention() {
        let tmp = tempfile::tempdir().expect("tmp");
        let repo = tmp.path().join("repo");
        make_guard_repo(&repo);
        let file = repo.join("model/NFR-005-recovery-rto-rpo.md");
        std::fs::write(
            &file,
            "---\nid: NFR-005\ntype: nfr\ntitle: \"RTO\"\nstatus: \"accepted\"\n---\n\nRTO 15 мин.\n",
        )
        .expect("write");
        git(&repo, &["add", "-A"]);
        git(&repo, &["commit", "-q", "-m", "nfr"]);
        std::fs::write(&file, "---\nid: NFR-005\ntype: nfr\ntitle: \"RTO\"\nstatus: \"accepted\"\n---\n\nRTO 10 мин.\n")
            .expect("edit");
        let path = new(&repo, "recovery-rto-rpo").expect("new");
        let body = std::fs::read_to_string(&path).expect("read");
        std::fs::write(
            &path,
            format!("{body}\nМеняем NFR-005 (срок восстановления).\n"),
        )
        .expect("mention");
        let report = guard(&repo, None, &[]).expect("guard");
        assert!(report.passed, "{report:?}");
        assert!(
            report
                .reasons
                .iter()
                .any(|(_, r)| r.contains("по id NFR-005")),
            "{:?}",
            report.reasons
        );
        let text = render_guard(&report);
        assert!(text.contains("по id NFR-005"), "{text}");
    }

    /// Н4б-граница: `NFR-0051` не покрывает `NFR-005` (совпадение слова).
    #[test]
    fn guard_does_not_match_id_substring() {
        assert!(!contains_word("правим NFR-0051", "NFR-005"));
        assert!(contains_word("правим NFR-005, срок", "NFR-005"));
        assert!(contains_word("(NFR-005)", "NFR-005"));
        assert!(!contains_word("AD-0091", "AD-009"));
        let tmp = tempfile::tempdir().expect("tmp");
        let repo = tmp.path().join("repo");
        make_guard_repo(&repo);
        std::fs::write(repo.join("model/NFR-005.md"), "---\nid: NFR-005\n---\n").expect("write");
        git(&repo, &["add", "-A"]);
        git(&repo, &["commit", "-q", "-m", "nfr"]);
        std::fs::write(
            repo.join("model/NFR-005.md"),
            "---\nid: NFR-005\n---\n\nv2\n",
        )
        .expect("edit");
        let path = new(&repo, "nfr-005").expect("new");
        let body = std::fs::read_to_string(&path).expect("read");
        std::fs::write(&path, format!("{body}\nЗатронут NFR-0051 (соседний).\n")).expect("mention");
        let report = guard(&repo, None, &[]).expect("guard");
        assert!(
            report.violations.contains(&"model/NFR-005.md".to_string()),
            "NFR-0051 не покрывает NFR-005: {:?}",
            report.covered
        );
    }

    /// Защищённый путь с кириллицей в имени: имена сущностей в кейсах русские
    /// (`model/CMP-001-оркестратор-операций.md`), и такой файл обязан быть
    /// виден гарду. `git diff --name-only` по умолчанию экранирует не-ASCII
    /// (`"model/CMP-001-\320\276…"`), из-за чего путь не начинался с `model/`
    /// и правка мимо дельты выглядела как «защищённых среди них: 0».
    #[test]
    fn guard_sees_protected_path_with_cyrillic_name() {
        let tmp = tempfile::tempdir().expect("tmp");
        let repo = tmp.path().join("repo");
        make_guard_repo(&repo);
        let file = repo.join("model/CMP-001-оркестратор-операций.md");
        std::fs::write(&file, "---\nid: CMP-001\n---\n\nкарточка\n").expect("write");
        git(&repo, &["add", "-A"]);
        git(&repo, &["commit", "-q", "-m", "cyr"]);
        std::fs::write(&file, "---\nid: CMP-001\n---\n\nкарточка v2\n").expect("edit");
        let report = guard(&repo, None, &[]).expect("guard");
        assert_eq!(
            report.protected_changed,
            vec!["model/CMP-001-оркестратор-операций.md".to_string()],
            "кириллический защищённый путь обязан быть виден: {report:?}"
        );
        assert!(!report.passed, "{report:?}");
    }

    #[test]
    fn guard_passes_when_active_delta_mentions_file() {
        let tmp = tempfile::tempdir().expect("tmp");
        let repo = tmp.path().join("repo");
        make_guard_repo(&repo);
        std::fs::write(repo.join("model/adr/ADR-003.md"), "# ADR-003 v2\n").expect("edit");
        // Активная дельта упоминает файл стемом (ADR-003) — засчитывается.
        let path = new(&repo, "update-adr-003").expect("new");
        let body = std::fs::read_to_string(&path).expect("read");
        std::fs::write(&path, format!("{body}\nЗатронут ADR-003 (таймауты).\n")).expect("mention");
        let report = guard(&repo, None, &[]).expect("guard");
        assert!(report.passed, "{report:?}");
        assert_eq!(
            report.covered,
            vec![(
                "model/adr/ADR-003.md".to_string(),
                "update-adr-003".to_string()
            )]
        );
        assert!(render_guard(&report).contains("PASS"));
    }

    #[test]
    fn guard_ignores_archived_delta() {
        let tmp = tempfile::tempdir().expect("tmp");
        let repo = tmp.path().join("repo");
        make_guard_repo(&repo);
        std::fs::write(repo.join("model/adr/ADR-003.md"), "# ADR-003 v2\n").expect("edit");
        let path = new(&repo, "update-adr-003").expect("new");
        fill_delta(&path);
        let body = std::fs::read_to_string(&path).expect("read");
        std::fs::write(&path, format!("{body}\nПравка model/adr/ADR-003.md.\n")).expect("mention");
        archive(&repo, "update-adr-003").expect("archive");
        // Архивная дельта — уже влитая истина, покрытием не считается.
        let report = guard(&repo, None, &[]).expect("guard");
        assert!(!report.passed, "архив не покрывает: {report:?}");
        assert_eq!(report.violations.len(), 1);
    }

    #[test]
    fn guard_passes_on_unprotected_change_and_protect_override() {
        let tmp = tempfile::tempdir().expect("tmp");
        let repo = tmp.path().join("repo");
        make_guard_repo(&repo);
        // Правка незащищённого файла — гейт молчит.
        std::fs::write(repo.join("src/main.rs"), "fn main() { println!(\"x\"); }\n").expect("edit");
        let report = guard(&repo, None, &[]).expect("guard");
        assert!(report.passed, "{report:?}");
        assert!(report.protected_changed.is_empty());
        assert!(render_guard(&report).contains("не затронуты"));
        // Явный --protect заменяет дефолт: теперь src/ под защитой → нарушение.
        let report = guard(&repo, None, &["src/".to_string()]).expect("guard override");
        assert!(!report.passed);
        assert_eq!(report.violations, vec!["src/main.rs".to_string()]);
    }

    #[test]
    fn guard_reports_git_errors() {
        let tmp = tempfile::tempdir().expect("tmp");
        // Не репозиторий — внятная ошибка, не паника.
        let err = guard(tmp.path(), None, &[]).expect_err("не git");
        assert!(err.to_string().contains("git diff"), "{err}");
    }

    #[test]
    fn guard_lists_all_mentioning_deltas_in_report() {
        let tmp = tempfile::tempdir().expect("tmp");
        let repo = tmp.path().join("repo");
        make_guard_repo(&repo);
        std::fs::write(repo.join("model/adr/ADR-003.md"), "# ADR-003 v2\n").expect("edit");
        // ДВЕ активные дельты упоминают файл — отчёт обязан показать обе,
        // а не первую попавшуюся (D8: отчёт вместо галочки).
        for name in ["update-adr-003", "adr-003-followup"] {
            let path = new(&repo, name).expect("new");
            let body = std::fs::read_to_string(&path).expect("read");
            std::fs::write(&path, format!("{body}\nЗатронут ADR-003.\n")).expect("mention");
        }
        let report = guard(&repo, None, &[]).expect("guard");
        assert!(report.passed, "{report:?}");
        assert_eq!(report.active_deltas, 2);
        assert_eq!(
            report.mentions,
            vec![(
                "model/adr/ADR-003.md".to_string(),
                // Порядок — по имени дельты (list сортирует), детерминирован.
                vec!["adr-003-followup".to_string(), "update-adr-003".to_string()]
            )]
        );
        // Совместимость: covered держит первую дельту.
        assert_eq!(report.covered.len(), 1);
        let text = render_guard(&report);
        assert!(
            text.contains("покрыт активными дельтами: 'adr-003-followup', 'update-adr-003'"),
            "{text}"
        );
        assert!(text.contains("активных дельт: 2"), "{text}");
    }

    #[test]
    fn guard_is_honest_when_no_active_deltas_at_all() {
        let tmp = tempfile::tempdir().expect("tmp");
        let repo = tmp.path().join("repo");
        make_guard_repo(&repo);
        std::fs::write(repo.join("model/adr/ADR-003.md"), "# ADR-003 v2\n").expect("edit");
        // Дельт нет вообще: честно «активных дельт нет», а не обтекаемое
        // «не упоминается ни в одной» (D8).
        let report = guard(&repo, None, &[]).expect("guard");
        assert!(!report.passed);
        assert_eq!(report.active_deltas, 0);
        assert_eq!(
            report.mentions,
            vec![("model/adr/ADR-003.md".to_string(), Vec::new())]
        );
        let text = render_guard(&report);
        assert!(text.contains("(активных дельт нет)"), "{text}");
        assert!(text.contains("arch-be delta new"), "{text}");
    }

    // --- инструменты delta_guard / delta_propose ----------------------------

    /// Тестовый контекст без LLM.
    fn tool_ctx(dir: &Path) -> crate::tool::ToolContext {
        crate::tool::ToolContext::new(
            dir.to_path_buf(),
            Arc::new(crate::config::Config::default()),
        )
    }

    #[tokio::test]
    async fn delta_guard_tool_verdict_pass_and_violation() {
        let tmp = tempfile::tempdir().expect("tmp");
        let repo = tmp.path().join("repo");
        make_guard_repo(&repo);
        let ctx = tool_ctx(&repo);
        // Чистое дерево — PASS-вердикт JSON.
        let out = DeltaGuardTool
            .call(json!({"path": "."}), &ctx)
            .await
            .expect("вызов");
        assert!(!out.is_error, "{}", out.content);
        let v: Value = serde_json::from_str(&out.content).expect("JSON-вердикт");
        assert_eq!(v["passed"], true, "{v}");
        assert_eq!(v["violations"], json!([]));

        // Прямая правка model/ без дельты — passed=false, файл в violations.
        std::fs::write(repo.join("model/adr/ADR-003.md"), "# ADR-003 v2\n").expect("edit");
        let out = DeltaGuardTool
            .call(json!({"path": "."}), &ctx)
            .await
            .expect("вызов");
        let v: Value = serde_json::from_str(&out.content).expect("JSON-вердикт");
        assert_eq!(v["passed"], false, "{v}");
        assert_eq!(v["violations"], json!(["model/adr/ADR-003.md"]));
        // Аддитивные поля D8: полный статус упоминания и счётчик дельт.
        assert_eq!(v["active_deltas"], 0, "{v}");
        assert_eq!(
            v["mentions"],
            json!([{"file": "model/adr/ADR-003.md", "deltas": []}]),
            "{v}"
        );

        // Не git-репозиторий — мягкая ошибка инструмента.
        let out = DeltaGuardTool
            .call(json!({"path": "."}), &tool_ctx(tmp.path()))
            .await
            .expect("вызов");
        assert!(out.is_error, "{}", out.content);
    }

    #[tokio::test]
    async fn delta_propose_tool_creates_skeleton_and_refuses_duplicate() {
        let tmp = tempfile::tempdir().expect("tmp");
        let ctx = tool_ctx(tmp.path());
        let out = DeltaProposeTool
            .call(json!({"name": "saga-pilot", "path": "."}), &ctx)
            .await
            .expect("вызов");
        assert!(!out.is_error, "{}", out.content);
        let v: Value = serde_json::from_str(&out.content).expect("JSON-вердикт");
        let created = v["created"].as_str().expect("created");
        assert!(
            created.ends_with("changes/saga-pilot/DELTA.md"),
            "{created}"
        );
        assert!(tmp.path().join("changes/saga-pilot/DELTA.md").is_file());

        // Повторное создание — мягкая ошибка (имя занято).
        let out = DeltaProposeTool
            .call(json!({"name": "saga-pilot", "path": "."}), &ctx)
            .await
            .expect("вызов");
        assert!(out.is_error, "{}", out.content);
        assert!(out.content.contains("уже существует"), "{}", out.content);
    }
}
