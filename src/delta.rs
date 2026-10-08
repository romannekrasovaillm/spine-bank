//! Дельта-спецификации как state machine (по `OpenSpec`): change-центричные
//! изменения `propose → (apply) → archive`; дельта описывает только изменение
//! относительно текущей истины (ADDED/MODIFIED/REMOVED).
//!
//! Каталог изменений — `changes/` в репозитории: предложенные на верхнем
//! уровне, заархивированные — в `changes/archive/`.
//!
//! [`guard`] — CI-гейт прямых правок спайна мимо дельты: изменённые файлы под
//! защищёнными путями (по умолчанию `model/`, `ARCHITECTURE-SPINE.md`,
//! `CONSTRAINTS.yaml`) обязаны упоминаться в активной дельте, иначе FAIL.
//!
//! Источники покрытия (F1, ADR-062): дельты Spine (`changes/<name>/DELTA.md`)
//! и changes `OpenSpec` (`openspec/changes/<id>/` — proposal.md, design.md,
//! tasks.md, specs/**/*.md). Состав источников — `[delta] sources` в
//! `arch-harness.toml` кейса; дефолт — оба при наличии `openspec/`.

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

/// Имя дельты свободно: нет ни активной, ни архивной дельты с таким именем.
#[must_use]
pub fn name_free(repo: &Path, name: &str) -> bool {
    !repo.join("changes").join(name).exists() && !repo.join("changes/archive").join(name).exists()
}

/// Создаёт каркас дельты `changes/<name>/DELTA.md`.
///
/// # Errors
/// Каталог существует, ошибка записи.
pub fn new(repo: &Path, name: &str) -> Result<PathBuf> {
    let content = DELTA_TEMPLATE.replace("{name}", name).replace(
        "{date}",
        &chrono::Local::now().format("%Y-%m-%d").to_string(),
    );
    new_with_body(repo, name, &content)
}

/// Создаёт дельту `changes/<name>/DELTA.md` с готовым телом (K5, ADR-064):
/// та же проверка занятости имени, что у [`new`], но содержимое пишет
/// вызывающий (машинное происхождение тела — `arch-diff accept`).
///
/// # Errors
/// Имя занято (активная или архивная дельта), ошибка записи.
pub fn new_with_body(repo: &Path, name: &str, body: &str) -> Result<PathBuf> {
    let dir = repo.join("changes").join(name);
    if !name_free(repo, name) {
        return Err(HarnessError::Control(format!(
            "дельта '{name}' уже существует (активная или в архиве): {}",
            dir.display()
        )));
    }
    std::fs::create_dir_all(&dir).map_err(|e| HarnessError::io(&dir, e))?;
    let path = dir.join("DELTA.md");
    std::fs::write(&path, body).map_err(|e| HarnessError::io(&path, e))?;
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

/// Источник покрытия правок спайна гейтом [`guard`] (F1, ADR-062).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoverageSource {
    /// Дельта Spine (`changes/<name>/DELTA.md`).
    Spine,
    /// Change `OpenSpec` (`openspec/changes/<id>/`: proposal.md, design.md,
    /// tasks.md, specs/**/*.md). Markdown `OpenSpec` Spine не пишет — адаптер
    /// только читает (правило 9, `docs/openspec.md`).
    Openspec,
}

impl CoverageSource {
    /// Имя источника в конфиге и отчётах.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Spine => "spine",
            Self::Openspec => "openspec",
        }
    }
}

/// Секция `[delta]` кейсового конфига `<repo>/arch-harness.toml`. Читается
/// точечно, отдельно от [`crate::config::Config`]: гейт прямых правок обязан
/// видеть источники покрытия и при библиотечном вызове (MCP, тесты), где
/// CLI-конфиг не загружается. Неизвестные ключи секции допустимы (аддитивность
/// формата конфига).
#[derive(Debug, Deserialize)]
struct DeltaCaseConfig {
    /// Секция `[delta]`.
    #[serde(default)]
    delta: Option<DeltaCaseTable>,
}

/// Таблица `[delta]`: состав источников покрытия.
#[derive(Debug, Default, Deserialize)]
struct DeltaCaseTable {
    /// Источники покрытия: подмножество `["spine", "openspec"]`. `None` —
    /// ключ не задан (авто-детект); `Some([])` — ошибка конфигурации.
    #[serde(default)]
    sources: Option<Vec<String>>,
}

/// Источники покрытия `delta guard` для репозитория (F1, ADR-062):
/// - `[delta] sources = [...]` в `<repo>/arch-harness.toml`, если секция
///   с ключом задана; неизвестное имя или пустой список — ошибка (опечатка
///   в конфиге контура не должна молча отключать источник покрытия);
/// - иначе авто-детект: оба источника при наличии каталога `openspec/`,
///   иначе только `spine` (поведение версий до 0.3.14).
///
/// # Errors
/// Конфиг кейса не читается/невалиден; список источников пуст или содержит
/// неизвестное имя.
pub fn coverage_sources(repo: &Path) -> Result<Vec<CoverageSource>> {
    let cfg_path = repo.join("arch-harness.toml");
    if cfg_path.is_file() {
        let text =
            std::fs::read_to_string(&cfg_path).map_err(|e| HarnessError::io(&cfg_path, e))?;
        let parsed: DeltaCaseConfig = toml::from_str(&text).map_err(|e| {
            HarnessError::Control(format!(
                "{}: конфиг кейса не разбирается: {e}",
                cfg_path.display()
            ))
        })?;
        if let Some(sources) = parsed.delta.and_then(|t| t.sources) {
            if sources.is_empty() {
                return Err(HarnessError::Control(format!(
                    "{}: [delta] sources пуст — гейт покрытия не признавал бы ни одного \
                     источника; укажите хотя бы один: \"spine\", \"openspec\"",
                    cfg_path.display()
                )));
            }
            let mut out = Vec::with_capacity(sources.len());
            for name in &sources {
                match name.as_str() {
                    "spine" => out.push(CoverageSource::Spine),
                    "openspec" => out.push(CoverageSource::Openspec),
                    other => {
                        return Err(HarnessError::Control(format!(
                            "{}: неизвестный источник покрытия '{other}' в [delta] sources — \
                             допустимы \"spine\" и \"openspec\"",
                            cfg_path.display()
                        )));
                    }
                }
            }
            return Ok(out);
        }
    }
    // Авто-детект: в репозитории с разметкой OpenSpec change равноправен
    // дельте; без неё — только дельты (поведение прежних версий).
    if repo.join("openspec").is_dir() {
        Ok(vec![CoverageSource::Spine, CoverageSource::Openspec])
    } else {
        Ok(vec![CoverageSource::Spine])
    }
}

/// Подсказка для `delta new`/`delta_propose` в репозитории, где единственный
/// источник покрытия — `OpenSpec` (`[delta] sources = ["openspec"]`): `DELTA.md`
/// не создаётся, изменение оформляется change'ом `OpenSpec` средствами самого
/// `OpenSpec` (его markdown Spine не пишет — правило 9, `docs/openspec.md`).
/// `None` — дельта создаётся как обычно.
///
/// # Errors
/// Ошибка чтения/разбора конфига кейса (см. [`coverage_sources`]).
pub fn openspec_only_hint(repo: &Path) -> Result<Option<String>> {
    if coverage_sources(repo)?.as_slice() == [CoverageSource::Openspec] {
        return Ok(Some(
            "DELTA.md не создан: в этом репозитории источник покрытия — только OpenSpec \
             ([delta] sources = [\"openspec\"]). Оформите изменение change'ом OpenSpec: \
             каталог openspec/changes/<имя>/ с proposal.md и tasks.md (напр. через \
             `openspec new change <имя>`), а правку защищённого файла упомяните в них. \
             Markdown OpenSpec Spine не пишет (docs/openspec.md)"
                .to_string(),
        ));
    }
    Ok(None)
}

/// Отчёт гейта прямых правок спайна.
#[derive(Debug, Clone)]
pub struct GuardReport {
    /// База diff, как передана в git.
    pub base: String,
    /// Всего изменённых файлов по diff.
    pub changed: usize,
    /// Изменённые файлы (относительные пути, отсортированы) — тем, кому нужен
    /// не счёт, а состав: составляющая `semantic_quality` (ADR-052) отбирает
    /// по нему субъектов, чьё досье затронуто.
    pub changed_files: Vec<String>,
    /// Изменённые защищённые файлы.
    pub protected_changed: Vec<String>,
    /// Покрытые правки: (файл, метка первого источника покрытия) —
    /// `delta:<name>` или `openspec:<id>` (F1, ADR-062; полный список — в
    /// `mentions`). До 0.3.14 меткой было имя дельты без префикса.
    pub covered: Vec<(String, String)>,
    /// Нарушения: защищённые файлы без упоминания в активных источниках.
    pub violations: Vec<String>,
    /// Правки, чьё покрытие существует, но все его файлы-носители созданы или
    /// изменены в диапазоне прогона исполнителя (правило владения ADR-055):
    /// (файл, метки отклонённых источников). Самоодобрение покрытием не
    /// считается: такие правки — тоже нарушения (`passed = false`), но с
    /// отдельной находкой `self_approved`. Аддитивное поле 0.3.14.
    pub self_approved: Vec<(String, Vec<String>)>,
    /// Гейт пройден (нет непокрытых правок защищённых путей и нет
    /// самоодобренных покрытий).
    pub passed: bool,
    /// Число активных дельт (`changes/<name>/DELTA.md` в статусе Proposed):
    /// контекст честности вывода — нарушение при нуле дельт означает
    /// «правку нечем покрыть», а не «дельта не та».
    pub active_deltas: usize,
    /// Число активных changes `OpenSpec` (`openspec/changes/<id>/` без
    /// `archive/`), участвовавших в покрытии (F1). 0 — источник `openspec`
    /// выключен конфигом или changes нет. Аддитивное поле 0.3.14.
    pub active_changes: usize,
    /// Источники покрытия прогона после резолва (`spine`/`openspec`,
    /// F1, ADR-062). Аддитивное поле 0.3.14.
    pub sources: Vec<String>,
    /// Дельта, заархивированные ВНУТРИ проверяемого диапазона `base..HEAD`
    /// (T-07): архивация до merge — покрытие для правок этого диапазона.
    /// Аддитивное поле: старые читатели JSON его не знают.
    /// С 0.3.14 — метки источников (`delta:<name>`, `openspec:<каталог>`).
    pub archived_in_range: Vec<String>,
    /// Покрытие каждого изменённого защищённого файла: (файл, метки ВСЕХ
    /// легитимных источников, его упоминающих; пустой список — нарушение или
    /// самоодобрение). С 0.3.14 метки — `delta:<name>`/`openspec:<id>`.
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

/// Имена АКТИВНЫХ дельт, упоминающих путь (A3): тот же механизм упоминаний,
/// что в [`guard`] (полный путь / имя файла / стем / id — [`delta_mentions`]),
/// без дублирования его логики. Заархивированные дельты не считаются: они
/// описывают влитую истину, а не правку, сделанную после выдачи пакета.
///
/// Пустой список — путь не покрыт ни одной активной дельтой.
#[must_use]
pub fn mentioning_deltas(repo: &Path, path: &str) -> Vec<String> {
    mentioning_delta_paths(repo, path)
        .into_iter()
        .map(|(name, _)| name)
        .collect()
}

/// То же, что [`mentioning_deltas`], но с путями `DELTA.md`: гейту (A5,
/// ADR-055) нужно проверить, не появилась ли легализующая дельта в диапазоне
/// прогона исполнителя, — имя без пути этот вопрос не решает.
#[must_use]
pub fn mentioning_delta_paths(repo: &Path, path: &str) -> Vec<(String, PathBuf)> {
    let mut out = Vec::new();
    for d in list(repo) {
        if d.status != DeltaStatus::Proposed {
            continue;
        }
        let Ok(body) = std::fs::read_to_string(&d.path) else {
            continue;
        };
        if delta_mentions(&body, path).is_some() {
            out.push((d.name, d.path));
        }
    }
    out
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

/// Изменённые файлы относительно `base` (дефолт `HEAD`: staged + unstaged
/// рабочего дерева), включая `НЕотслеживаемые` (Н4): `git diff` их не показывает,
/// поэтому вердикт не должен зависеть от того, сделан ли `git add`. Пути —
/// относительно корня репозитория, отсортированы и дедуплицированы.
///
/// Одна реализация на двух потребителей — гейт правок спайна ([`guard`]) и
/// сканирование секретов в изменённых файлах (составляющая `secrets`, C3).
///
/// # Errors
/// `git` недоступен или вернул ненулевой код (не репозиторий, плохая база).
pub fn changed_files(repo: &Path, base: Option<&str>) -> Result<Vec<String>> {
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
    Ok(changed)
}

/// Гейт прямых правок спайна мимо дельты (CI-запрет «прямых коммитов в model/
/// мимо changes/»): каждый изменённый защищённый файл обязан упоминаться
/// (путём или именем) в теле хотя бы одного источника покрытия — активной
/// дельты (`changes/<name>/DELTA.md`), активного change `OpenSpec`
/// (`openspec/changes/<id>/`, F1) либо заархивированных ВНУТРИ проверяемого
/// диапазона (`base..HEAD`, T-07). Артефакт, заархивированный до базы, не
/// засчитывается: правок диапазона он не описывает.
///
/// Эквивалент `guard_with` с настройками по умолчанию (источники — из
/// `[delta] sources` кейса или авто-детект; правило владения не применяется).
///
/// Изменённые файлы — `git diff --name-only <base>` (дефолт `HEAD`: staged +
/// unstaged рабочего дерева; untracked-файлы git-diff не показывает — для CI
/// передавайте базу вида `origin/main...HEAD`).
///
/// # Errors
/// `git` недоступен или вернул ненулевой код (не репозиторий, плохая база);
/// конфиг `[delta] sources` кейса невалиден.
pub fn guard(repo: &Path, base: Option<&str>, protect: &[String]) -> Result<GuardReport> {
    guard_with(repo, base, protect, &GuardOptions::default())
}

/// Настройки гейта прямых правок спайна (F1, ADR-062).
#[derive(Debug, Clone, Default)]
pub struct GuardOptions {
    /// Источники покрытия: `None` — `[delta] sources` кейсового
    /// `arch-harness.toml` или авто-детект (оба источника при наличии
    /// `openspec/`). Явный список заменяет конфиг кейса.
    pub sources: Option<Vec<CoverageSource>>,
    /// Диапазон прогона исполнителя (A5, ADR-055): покрытие, чьи файлы-
    /// носители появились или изменились в этом диапазоне, правку НЕ
    /// узаконивает — одинаково для дельт Spine и changes `OpenSpec`
    /// (самоодобрение, находка `self_approved`). `None` (дефолт) — обычный
    /// гейт/CI без проверки происхождения покрытия.
    pub agent_range: Option<crate::control::AgentRange>,
}

/// Единая записись покрытия гейта (F1): дельта Spine или change `OpenSpec`.
struct Cover {
    /// Метка источника: `delta:<name>` или `openspec:<id>`.
    label: String,
    /// Тело, в котором ищется упоминание защищённого файла ([`delta_mentions`]).
    body: String,
    /// Файлы-носители покрытия — маркеры правила владения (ADR-055):
    /// `DELTA.md` дельты; proposal/design/tasks/specs change `OpenSpec`.
    markers: Vec<PathBuf>,
}

/// Собирает покрытия из дельт Spine: активные (`changes/<id>`) и
/// заархивированные ВНУТРИ проверяемого диапазона (T-07). Дельта,
/// заархивированная ДО базы, — влитая истина: правок диапазона она не
/// описывает и покрытием не считается, иначе архив стал бы универсальной
/// отмычкой для любой последующей правки.
fn collect_delta_covers(
    repo: &Path,
    base: &str,
    covers: &mut Vec<Cover>,
    archived_in_range: &mut Vec<String>,
) -> Result<usize> {
    let mut active_deltas = 0_usize;
    for d in list(repo) {
        match d.status {
            DeltaStatus::Proposed => active_deltas += 1,
            DeltaStatus::Archived if archived_within(repo, base, &d.path) => {
                archived_in_range.push(format!("delta:{}", d.name));
            }
            DeltaStatus::Archived => continue,
        }
        let body = std::fs::read_to_string(&d.path).map_err(|e| HarnessError::io(&d.path, e))?;
        covers.push(Cover {
            label: format!("delta:{}", d.name),
            body,
            markers: vec![d.path],
        });
    }
    Ok(active_deltas)
}

/// Файлы change `OpenSpec`, в которых ищется упоминание защищённого пути
/// (F1): `proposal.md`, `design.md`, `tasks.md` и дельты спек `specs/**/*.md`.
/// Тот же набор — маркеры правила владения: изменение любого из них в
/// диапазоне исполнителя делает покрытие самоодобрением (ADR-055).
fn openspec_cover_files(change_dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = ["proposal.md", "design.md", "tasks.md"]
        .iter()
        .map(|f| change_dir.join(f))
        .filter(|p| p.is_file())
        .collect();
    files.extend(crate::openspec::collect_md(&change_dir.join("specs")));
    files.sort();
    files
}

/// Собирает покрытия из changes `OpenSpec` (F1, ADR-062): активные
/// (`openspec/changes/<id>/`, без `archive/`) и заархивированные ВНУТРИ
/// проверяемого диапазона (`openspec/changes/archive/<каталог>/` — та же
/// логика, что у дельт, T-07). Имя каталога архива у `OpenSpec` может нести
/// префикс даты, поэтому идентификатор — имя каталога ЦЕЛИКОМ, без выделения
/// `<id>`. Change без файлов-носителей (ни proposal/design/tasks, ни specs)
/// покрытием не становится — упоминанию нечем быть.
fn collect_openspec_covers(
    repo: &Path,
    base: &str,
    covers: &mut Vec<Cover>,
    archived_in_range: &mut Vec<String>,
) -> Result<usize> {
    let mut active_changes = 0_usize;
    let changes_dir = repo.join("openspec/changes");
    let mut push_change = |dir: &Path, archived: bool| -> Result<()> {
        let files = openspec_cover_files(dir);
        if files.is_empty() {
            return Ok(());
        }
        let id = dir
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let mut body = String::new();
        for f in &files {
            body.push_str(&std::fs::read_to_string(f).map_err(|e| HarnessError::io(f, e))?);
            body.push('\n');
        }
        if archived {
            archived_in_range.push(format!("openspec:{id}"));
        } else {
            active_changes += 1;
        }
        covers.push(Cover {
            label: format!("openspec:{id}"),
            body,
            markers: files,
        });
        Ok(())
    };
    if changes_dir.is_dir() {
        let mut dirs: Vec<PathBuf> = std::fs::read_dir(&changes_dir)
            .map_err(|e| HarnessError::io(&changes_dir, e))?
            .filter_map(std::result::Result::ok)
            .map(|e| e.path())
            .filter(|p| p.is_dir() && p.file_name().is_some_and(|n| n != "archive"))
            .collect();
        dirs.sort();
        for dir in dirs {
            push_change(&dir, false)?;
        }
    }
    let archive_dir = changes_dir.join("archive");
    if archive_dir.is_dir() {
        let mut dirs: Vec<PathBuf> = std::fs::read_dir(&archive_dir)
            .map_err(|e| HarnessError::io(&archive_dir, e))?
            .filter_map(std::result::Result::ok)
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect();
        dirs.sort();
        for dir in dirs {
            // Как у дельт (T-07): засчитывается только архивация ВНУТРИ
            // проверяемого диапазона; признак — каталог change изменён в
            // `base..HEAD` (архив OpenSpec переносит каталог целиком).
            if archived_within(repo, base, &dir) {
                push_change(&dir, true)?;
            }
        }
    }
    Ok(active_changes)
}

/// Гейт прямых правок спайна с явными настройками (F1, ADR-062) — см.
/// [`guard`]; дополнительно к нему: состав источников покрытия и правило
/// владения ADR-055 по диапазону прогона исполнителя.
///
/// # Errors
/// Как у [`guard`]; явный пустой список источников — ошибка конфигурации.
pub fn guard_with(
    repo: &Path,
    base: Option<&str>,
    protect: &[String],
    options: &GuardOptions,
) -> Result<GuardReport> {
    let protected: Vec<String> = if protect.is_empty() {
        DEFAULT_PROTECTED.iter().map(|s| (*s).to_string()).collect()
    } else {
        protect.to_vec()
    };
    let base = base.unwrap_or("HEAD").to_string();
    let changed = changed_files(repo, Some(&base))?;
    let sources = match &options.sources {
        Some(s) if s.is_empty() => {
            return Err(HarnessError::Control(
                "список источников покрытия пуст — гейт не признавал бы ни одного; \
                 укажите хотя бы один: spine, openspec"
                    .to_string(),
            ));
        }
        Some(s) => s.clone(),
        None => coverage_sources(repo)?,
    };

    let mut covers: Vec<Cover> = Vec::new();
    let mut active_deltas = 0_usize;
    let mut active_changes = 0_usize;
    let mut archived_in_range: Vec<String> = Vec::new();
    if sources.contains(&CoverageSource::Spine) {
        active_deltas = collect_delta_covers(repo, &base, &mut covers, &mut archived_in_range)?;
    }
    if sources.contains(&CoverageSource::Openspec) {
        active_changes = collect_openspec_covers(repo, &base, &mut covers, &mut archived_in_range)?;
    }
    archived_in_range.sort();

    // Правило владения (A5, ADR-055): покрытие, чьи файлы-носители появились
    // или изменились в диапазоне прогона исполнителя, — работа исполнителя,
    // а не решение владельца; оно правку не узаконивает. Действует одинаково
    // для дельт Spine и changes OpenSpec, иначе change открывал бы обход,
    // закрытый для самодельной дельты.
    let (in_range, legit): (Vec<Cover>, Vec<Cover>) = match &options.agent_range {
        Some(range) => covers
            .into_iter()
            .partition(|c| c.markers.iter().any(|f| range.contains_file(f))),
        None => (Vec::new(), covers),
    };

    let mut protected_changed = Vec::new();
    let mut covered = Vec::new();
    let mut violations = Vec::new();
    let mut self_approved: Vec<(String, Vec<String>)> = Vec::new();
    let mut mentions = Vec::new();
    let mut reasons = Vec::new();
    for file in changed.iter().filter(|f| is_protected(f, &protected)) {
        protected_changed.push(file.clone());
        let by: Vec<(&Cover, Option<String>)> = legit
            .iter()
            .map(|c| (c, delta_mentions(&c.body, file)))
            .filter(|(_, reason)| reason.is_some())
            .collect();
        let by_labels: Vec<String> = by.iter().map(|(c, _)| c.label.clone()).collect();
        if let Some((cover, reason)) = by.first() {
            covered.push((file.clone(), cover.label.clone()));
            if let Some(reason) = reason {
                reasons.push((file.clone(), reason.clone()));
            }
        } else {
            // Покрытие только из диапазона исполнителя — не «нет покрытия»,
            // а самоодобрение: отдельная находка с именем источника.
            let rejected: Vec<String> = in_range
                .iter()
                .filter(|c| delta_mentions(&c.body, file).is_some())
                .map(|c| c.label.clone())
                .collect();
            if rejected.is_empty() {
                violations.push(file.clone());
            } else {
                self_approved.push((file.clone(), rejected));
            }
        }
        mentions.push((file.clone(), by_labels));
    }
    let passed = violations.is_empty() && self_approved.is_empty();
    Ok(GuardReport {
        base,
        changed: changed.len(),
        changed_files: changed.clone(),
        protected_changed,
        covered,
        violations,
        self_approved,
        passed,
        active_deltas,
        active_changes,
        sources: sources.iter().map(|s| s.as_str().to_string()).collect(),
        archived_in_range,
        mentions,
        reasons,
    })
}

/// Заархивирована ли дельта ВНУТРИ проверяемого диапазона (T-07).
///
/// Порядок работы «merge → archive» — правило процесса, но архивация на ветке
/// ДО merge тоже встречается, и гейт от базы не должен краснеть от того, что
/// работа уже влита в живую истину: правки диапазона описаны этой дельтой.
/// Признак один — файл дельты изменён в диапазоне `base..HEAD`: архивная дельта
/// покрывает правки СВОЕГО диапазона и не покрывает всё, что случится потом.
/// Незакоммиченный перенос в архив покрытием не становится: пока перенос не в
/// истории, «заархивирована» — намерение, а не факт (граница Н4).
fn archived_within(repo: &Path, base: &str, delta: &Path) -> bool {
    let rel = delta.strip_prefix(repo).map_or_else(
        |_| delta.display().to_string(),
        |p| p.to_string_lossy().replace('\\', "/"),
    );
    // Трёхточечная форма базы (`origin/main...HEAD`) — уже диапазон.
    let range = if base.contains("..") {
        base.to_string()
    } else {
        format!("{base}..HEAD")
    };
    std::process::Command::new("git")
        .args(["-c", "core.quotepath=false"])
        .arg("-C")
        .arg(repo)
        .args(["log", "--format=%H", "--max-count=1", &range, "--", &rel])
        .output()
        .is_ok_and(|o| o.status.success() && !o.stdout.is_empty())
}

/// Чем источник покрывает правку: активный или заархивированный внутри
/// проверяемого диапазона (T-07), дельта Spine или change `OpenSpec` (F1).
/// Различия называть обязательно: «покрыто архивным» — не то же самое, что
/// «правка описана изменением в работе», а change `OpenSpec` — не дельта.
fn covering_kind(report: &GuardReport, label: &str) -> String {
    let archived = report.archived_in_range.iter().any(|a| a == label);
    match (label.starts_with("openspec:"), archived) {
        (true, true) => "change OpenSpec, заархивированным в диапазоне".to_string(),
        (true, false) => "активным change OpenSpec".to_string(),
        (false, true) => "дельтой, заархивированной в диапазоне".to_string(),
        (false, false) => "активной дельтой".to_string(),
    }
}

/// Подсказка после `delta archive` на ветке, ещё не влитой в основную (T-07).
///
/// Порядок «merge → archive» — единственный, при котором дельта покрывает свои
/// правки на всём пути: архивация на feature-ветке делает дельту архивной, и
/// гейт от базы видит правки спайна уже без активной дельты. Покрытием дельта
/// засчитывается (правки диапазона она описывает), но правки, попавшие в базу
/// ДО архивации, — нет. `None` — подсказывать нечего: git недоступен, ветка
/// влита в основную, основной ветки нет.
#[must_use]
pub fn archive_order_hint(repo: &Path) -> Option<String> {
    let git_ok = |args: &[&str]| {
        std::process::Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .output()
            .is_ok_and(|o| o.status.success())
    };
    let mut branches: Vec<&str> = Vec::new();
    for branch in ["origin/main", "main", "origin/master", "master"] {
        if git_ok(&["rev-parse", "--verify", "--quiet", branch]) {
            branches.push(branch);
        }
    }
    if branches.is_empty() {
        return None;
    }
    // Влито — если HEAD достижим из любой из основных веток.
    if branches
        .iter()
        .any(|b| git_ok(&["merge-base", "--is-ancestor", "HEAD", b]))
    {
        return None;
    }
    Some(format!(
        "ветка не влита в {}: дельта станет архивной, и гейт от базы (--base {}) \
         увидит правки спайна без активной дельты для того, что попало в базу до \
         архивации. Порядок: merge → archive; границы пересмотра — docs/control.md",
        branches[0], branches[0]
    ))
}

/// Текстовый рендер отчёта гейта (в стиле остальных delta-команд): сводка,
/// по каждому изменённому защищённому файлу — статус его упоминания в
/// источниках покрытия (все поимённо; при полном их отсутствии — честное
/// «покрытий нет», а не обтекаемое «не упоминается»). Метки источников —
/// `delta:<name>`/`openspec:<id>` (F1); покрытие из диапазона исполнителя —
/// отдельный блок `self_approved` (ADR-055).
#[must_use]
pub fn render_guard(report: &GuardReport) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    let openspec_on = report.sources.iter().any(|s| s == "openspec");
    let _ = writeln!(out, "Гейт прямых правок спайна (база: {})", report.base);
    let _ = writeln!(
        out,
        "Изменённых файлов: {}, защищённых среди них: {} (активных дельт: {}{}{})",
        report.changed,
        report.protected_changed.len(),
        report.active_deltas,
        if openspec_on {
            format!(", активных changes OpenSpec: {}", report.active_changes)
        } else {
            String::new()
        },
        if report.archived_in_range.is_empty() {
            String::new()
        } else {
            format!(
                ", заархивировано в диапазоне: {}",
                report.archived_in_range.join(", ")
            )
        }
    );
    let sources_note = |active_zero: bool, archived_empty: bool| match (
        openspec_on,
        active_zero,
        archived_empty,
    ) {
        (false, true, true) => " (активных дельт нет)".to_string(),
        (true, true, true) => " (активных дельт и changes OpenSpec нет)".to_string(),
        (false, false, true) => format!(" (активных дельт: {})", report.active_deltas),
        (true, false, true) => format!(
            " (активных дельт: {}, активных changes OpenSpec: {})",
            report.active_deltas, report.active_changes
        ),
        (_, _, false) => format!(
            " (задействовано источников: {}, заархивировано в диапазоне: {})",
            report.sources.join("+"),
            report.archived_in_range.join(", ")
        ),
    };
    if !report.protected_changed.is_empty() {
        out.push('\n');
        for (file, labels) in &report.mentions {
            if let Some((_, rejected)) = report.self_approved.iter().find(|(f, _)| f == file) {
                let quoted: Vec<String> = rejected.iter().map(|s| format!("'{s}'")).collect();
                let _ = writeln!(
                    out,
                    "[error] {file} — покрытие {} создано или изменено в диапазоне прогона \
                     исполнителя (self_approved)",
                    quoted.join(", ")
                );
                let _ = writeln!(
                    out,
                    "  → правило владения (ADR-055): покрытие из диапазона исполнителя правку не \
                     узаконивает — перенесите оформление изменения в основной репозиторий \
                     решением владельца"
                );
                continue;
            }
            match labels.as_slice() {
                [] => {
                    let _ = writeln!(
                        out,
                        "[error] {file} — не упоминается ни в одном источнике покрытия{}",
                        sources_note(
                            report.active_deltas == 0 && report.active_changes == 0,
                            report.archived_in_range.is_empty()
                        )
                    );
                    let hint = match (report.sources.iter().any(|s| s == "spine"), openspec_on) {
                        (true, false) => "оформите правку дельтой: arch-be delta new <name>, \
                             опишите изменение в changes/<name>/DELTA.md (дельта, \
                             заархивированная до базы, покрытием не считается)"
                            .to_string(),
                        (true, true) => "оформите правку дельтой (arch-be delta new <name>) или \
                             change OpenSpec (openspec/changes/<id>/: proposal.md, tasks.md — \
                             создаётся средствами OpenSpec); источник, заархивированный до базы, \
                             покрытием не считается"
                            .to_string(),
                        (false, true) => "оформите правку change OpenSpec средствами OpenSpec \
                             (openspec/changes/<id>/: proposal.md, tasks.md) — DELTA.md в этом \
                             репозитории покрытием не считается ([delta] sources)"
                            .to_string(),
                        (false, false) => {
                            "источники покрытия не настроены ([delta] sources)".to_string()
                        }
                    };
                    let _ = writeln!(out, "  → {hint}");
                }
                [single] => {
                    let why = report
                        .reasons
                        .iter()
                        .find(|(f, _)| f == file)
                        .map_or(String::new(), |(_, r)| format!(" ({r})"));
                    let _ = writeln!(
                        out,
                        "[ok] {file} — покрыт {} '{single}'{why}",
                        covering_kind(report, single)
                    );
                }
                many => {
                    // Пока все покрытия — активные дельты, формулировка прежняя:
                    // отчёт читается человеком, и лишняя детализация там, где
                    // уточнять нечего, только мешает.
                    let plain_deltas = many.iter().all(|d| {
                        !report.archived_in_range.contains(d) && !d.starts_with("openspec:")
                    });
                    if plain_deltas {
                        let quoted: Vec<String> = many.iter().map(|d| format!("'{d}'")).collect();
                        let _ = writeln!(
                            out,
                            "[ok] {file} — покрыт активными дельтами: {}",
                            quoted.join(", ")
                        );
                    } else {
                        let quoted: Vec<String> = many
                            .iter()
                            .map(|d| format!("'{}' ({})", d, covering_kind(report, d)))
                            .collect();
                        let _ = writeln!(
                            out,
                            "[ok] {file} — покрыт источниками: {}",
                            quoted.join(", ")
                        );
                    }
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
            } else if openspec_on {
                "PASS — все правки спайна покрыты дельтами/changes OpenSpec"
            } else {
                "PASS — все правки спайна покрыты активными дельтами"
            }
        } else if !report.self_approved.is_empty() && report.violations.is_empty() {
            "FAIL — покрытие из диапазона исполнителя (self_approved, ADR-055) (exit 1)"
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
                          дельте changes/<name>/DELTA.md или change OpenSpec \
                          (openspec/changes/<id>/: proposal.md, design.md, tasks.md, \
                          specs/**/*.md — источники задаёт [delta] sources кейса, ADR-062). \
                          Ответ — JSON: passed + violations \
                          (непокрытые правки) + covered + mentions (все источники по каждому \
                          файлу, метки delta:<name>/openspec:<id>) + active_deltas + \
                          active_changes + sources + self_approved (покрытие из диапазона \
                          исполнителя, ADR-055) + summary; passed=false — основание отказать \
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
             непокрытых нарушений {}{} (источники: {}; активных дельт: {}, активных changes \
             OpenSpec: {}, заархивировано в диапазоне: {})",
            report.base,
            report.changed,
            report.protected_changed.len(),
            report.violations.len(),
            if report.self_approved.is_empty() {
                String::new()
            } else {
                format!(", самоодобренных покрытий {}", report.self_approved.len())
            },
            report.sources.join("+"),
            report.active_deltas,
            report.active_changes,
            report.archived_in_range.len()
        );
        let verdict = json!({
            "tool": "delta_guard",
            "passed": report.passed,
            "base": report.base,
            "changed": report.changed,
            "protected_changed": report.protected_changed,
            // Значение — метка источника покрытия `delta:<name>`/`openspec:<id>`
            // (F1, ADR-062; до 0.3.14 — имя дельты без префикса).
            "covered": report.covered.iter().map(|(f, d)| json!({"file": f, "delta": d})).collect::<Vec<_>>(),
            "violations": report.violations,
            // Аддитивные поля (SDK-контракт v1): полный статус упоминания
            // каждого защищённого файла — все активные источники, а не первый.
            "active_deltas": report.active_deltas,
            "archived_in_range": report.archived_in_range,
            "mentions": report.mentions.iter().map(|(f, ds)| json!({"file": f, "deltas": ds})).collect::<Vec<_>>(),
            // Аддитивные поля 0.3.14 (F1): источники прогона, активные changes
            // OpenSpec и покрытия, отклонённые правилом владения (ADR-055).
            "sources": report.sources,
            "active_changes": report.active_changes,
            "self_approved": report.self_approved.iter().map(|(f, ss)| json!({"file": f, "sources": ss})).collect::<Vec<_>>(),
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
        // F1 (ADR-062): при `sources = ["openspec"]` дельта не создаётся —
        // изменение оформляется change'ом OpenSpec (его markdown Spine не
        // пишет, правило 9).
        match openspec_only_hint(&repo) {
            Ok(Some(hint)) => return Ok(ToolOutput::err(format!("delta_propose: {hint}"))),
            Ok(None) => {}
            Err(e) => return Ok(ToolOutput::err(format!("delta_propose: {e}"))),
        }
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
        assert_eq!(
            report.covered,
            [] as [(std::string::String, std::string::String); 0]
        );
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

    /// T-07: дельта, заархивированная ВНУТРИ проверяемого диапазона, покрывает
    /// правки этого диапазона — архивация до merge (обычный порядок на ветке)
    /// не краснит гейт от базы. И обратная граница: дельта, заархивированная ДО
    /// базы, покрытием не становится, а правка без всякой дельты красна.
    #[test]
    fn guard_counts_delta_archived_within_range() {
        let tmp = tempfile::tempdir().expect("tmp");
        let repo = tmp.path().join("repo");
        make_guard_repo(&repo);
        // Основная ветка называется явно: имя ветки по умолчанию зависит от git.
        git(&repo, &["branch", "-M", "main"]);
        git(&repo, &["checkout", "-q", "-b", "feature"]);
        // Работа на ветке: правка спайна + дельта, затем архивация — до merge.
        std::fs::write(repo.join("ARCHITECTURE-SPINE.md"), "# Spine v2\n").expect("edit");
        let path = new(&repo, "spine-v2").expect("new");
        fill_delta(&path);
        let body = std::fs::read_to_string(&path).expect("read");
        std::fs::write(&path, format!("{body}\nПравка ARCHITECTURE-SPINE.md.\n")).expect("mention");
        git(&repo, &["add", "-A"]);
        git(&repo, &["commit", "-q", "-m", "правка спайна дельтой"]);
        archive(&repo, "spine-v2").expect("archive");
        git(&repo, &["add", "-A"]);
        git(&repo, &["commit", "-q", "-m", "архивация до merge"]);
        let report = guard(&repo, Some("main"), &[]).expect("guard");
        assert!(
            report.passed,
            "архивация внутри диапазона обязана покрывать: {report:?}"
        );
        assert_eq!(report.active_deltas, 0, "{report:?}");
        assert_eq!(report.archived_in_range, vec!["delta:spine-v2".to_string()]);
        let text = render_guard(&report);
        assert!(
            text.contains("заархивированной в диапазоне"),
            "отчёт обязан называть, чем покрыто: {text}"
        );
        // Новая правка того же файла после архивации — уже мимо дельты: архив
        // описывает правки СВОЕГО диапазона, а не всё, что случится потом.
        std::fs::write(repo.join("ARCHITECTURE-SPINE.md"), "# Spine v3\n").expect("edit");
        git(&repo, &["add", "-A"]);
        git(&repo, &["commit", "-q", "-m", "правка без дельты"]);
        let report = guard(&repo, Some("HEAD~1"), &[]).expect("guard");
        assert_eq!(
            report.violations,
            vec!["ARCHITECTURE-SPINE.md".to_string()],
            "правка без дельты обязана быть красной: {report:?}"
        );
    }

    /// T-07: архивация на ветке, не влитой в основную, называет порядок
    /// «merge → archive»; на влитой ветке подсказки нет.
    #[test]
    fn archive_hint_names_the_merge_order() {
        let tmp = tempfile::tempdir().expect("tmp");
        let repo = tmp.path().join("repo");
        make_guard_repo(&repo);
        // make_guard_repo коммитит в текущую ветку: она и есть «main».
        git(&repo, &["branch", "-M", "main"]);
        git(&repo, &["checkout", "-q", "-b", "feature"]);
        std::fs::write(repo.join("notes.md"), "ветка в работе\n").expect("write");
        git(&repo, &["add", "-A"]);
        git(&repo, &["commit", "-q", "-m", "работа на ветке"]);
        let hint = archive_order_hint(&repo).expect("подсказка на невлитой ветке");
        assert!(hint.contains("merge → archive"), "{hint}");
        git(&repo, &["checkout", "-q", "main"]);
        git(&repo, &["merge", "-q", "--ff-only", "feature"]);
        assert!(
            archive_order_hint(&repo).is_none(),
            "влитая ветка — подсказывать нечего"
        );
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
                "delta:update-adr-003".to_string()
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
        assert_eq!(report.protected_changed, [] as [std::string::String; 0]);
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
                // Метка источника (F1): `delta:<name>`.
                vec![
                    "delta:adr-003-followup".to_string(),
                    "delta:update-adr-003".to_string()
                ]
            )]
        );
        // Совместимость: covered держит первую дельту.
        assert_eq!(report.covered.len(), 1);
        let text = render_guard(&report);
        assert!(
            text.contains(
                "покрыт активными дельтами: 'delta:adr-003-followup', 'delta:update-adr-003'"
            ),
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

    // --- F1 (ADR-062): change OpenSpec как источник покрытия ----------------

    /// git с выводом stdout (для rev-parse базы диапазона).
    fn git_out(dir: &Path, args: &[&str]) -> String {
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
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// Репо-фикстура «двойного учёта» (F1): git + защищённый model/CMP-001.md,
    /// CONSTRAINTS.yaml и разметка `OpenSpec` (`openspec/specs/payments`).
    /// Один коммит («base»).
    fn make_openspec_repo(dir: &Path) {
        std::fs::create_dir_all(dir).expect("mkdir");
        git(dir, &["init", "-q"]);
        std::fs::create_dir_all(dir.join("model")).expect("mkdir model");
        std::fs::write(
            dir.join("model/CMP-001.md"),
            "---\nid: CMP-001\ntype: cmp\ntitle: Приём платежей\nstatus: active\n---\n",
        )
        .expect("model");
        std::fs::write(dir.join("CONSTRAINTS.yaml"), "constraints: []\n").expect("constraints");
        std::fs::create_dir_all(dir.join("openspec/specs/payments")).expect("mkdir specs");
        std::fs::write(
            dir.join("openspec/specs/payments/spec.md"),
            "# payments\n\n### Requirement: Idempotent intake\n\
             The system SHALL accept a payment at most once per idempotency key.\n",
        )
        .expect("spec");
        git(dir, &["add", "."]);
        git(dir, &["commit", "-q", "-m", "base"]);
    }

    /// Активный change `OpenSpec`, упоминающий `model/CMP-001.md` в proposal.md
    /// и tasks.md (по репродукции «двойного учёта» из задания).
    fn write_openspec_change(dir: &Path, id: &str) {
        let change = dir.join("openspec/changes").join(id);
        std::fs::create_dir_all(&change).expect("mkdir change");
        std::fs::write(
            change.join("proposal.md"),
            "## Why\nНужны лимиты на приём.\n\n## What Changes\n\
             - model/CMP-001.md: компонент получает зависимость от движка лимитов.\n",
        )
        .expect("proposal");
        std::fs::write(
            change.join("tasks.md"),
            "- [ ] 1.1 Обновить model/CMP-001.md\n",
        )
        .expect("tasks");
    }

    /// Правка защищённого `model/CMP-001.md` (зависимость от движка лимитов).
    fn edit_cmp_001(dir: &Path) {
        std::fs::write(
            dir.join("model/CMP-001.md"),
            "---\nid: CMP-001\ntype: cmp\ntitle: Приём платежей\nstatus: active\n---\n\
             depends_on:\n  - CMP-002\n",
        )
        .expect("edit");
    }

    /// F1 (ADR-062), репродукция «двойного учёта» 0.3.13: правка `model/` в
    /// рамках активного change `OpenSpec` засчитывается покрытием — дублировать
    /// описание в `DELTA.md` не нужно. До фикса: FAIL «не упоминается ни в
    /// одной активной дельте — активных дельт нет».
    #[test]
    fn guard_counts_active_openspec_change() {
        let tmp = tempfile::tempdir().expect("tmp");
        let repo = tmp.path().join("repo");
        make_openspec_repo(&repo);
        write_openspec_change(&repo, "add-limits");
        edit_cmp_001(&repo);
        let report = guard(&repo, None, &[]).expect("guard");
        assert!(report.passed, "{report:?}");
        assert_eq!(report.active_deltas, 0);
        assert_eq!(report.active_changes, 1);
        assert_eq!(
            report.sources,
            vec!["spine".to_string(), "openspec".to_string()],
            "авто-детект: оба источника при наличии openspec/"
        );
        assert_eq!(
            report.covered,
            vec![(
                "model/CMP-001.md".to_string(),
                "openspec:add-limits".to_string()
            )],
            "источник покрытия назван в отчёте"
        );
        let text = render_guard(&report);
        assert!(text.contains("openspec:add-limits"), "{text}");
        assert!(text.contains("активным change OpenSpec"), "{text}");
        assert!(text.contains("PASS"), "{text}");
    }

    /// F1: упоминание ищется и в дельтах спек change (`specs/**/*.md`) — той же
    /// функцией `delta_mentions`, тем же правилом совпадения по пути.
    #[test]
    fn guard_finds_mention_in_openspec_specs_delta() {
        let tmp = tempfile::tempdir().expect("tmp");
        let repo = tmp.path().join("repo");
        make_openspec_repo(&repo);
        // Change без proposal/tasks: упоминание только в дельте спеки.
        let specs = repo.join("openspec/changes/add-limits/specs/payments");
        std::fs::create_dir_all(&specs).expect("mkdir delta spec");
        std::fs::write(
            specs.join("spec.md"),
            "# Delta: payments\n\n## MODIFIED Requirements\n\n\
             ### Requirement: Idempotent intake\nЗатрагивает model/CMP-001.md.\n\
             The system SHALL accept a payment at most once.\n",
        )
        .expect("delta spec");
        edit_cmp_001(&repo);
        let report = guard(&repo, None, &[]).expect("guard");
        assert!(report.passed, "{report:?}");
        assert_eq!(
            report.covered,
            vec![(
                "model/CMP-001.md".to_string(),
                "openspec:add-limits".to_string()
            )]
        );
    }

    /// F1 (T-07 для `OpenSpec`): change, заархивированный ВНУТРИ проверяемого
    /// диапазона, покрывает правки диапазона — и метка источника строится по
    /// каталогу архива ЦЕЛИКОМ (с префиксом даты), а не по угаданному `<id>`.
    /// Архив до базы покрытием не является.
    #[test]
    fn guard_counts_openspec_change_archived_within_range_only() {
        let tmp = tempfile::tempdir().expect("tmp");
        let repo = tmp.path().join("repo");
        make_openspec_repo(&repo);
        git(&repo, &["branch", "-M", "main"]);
        git(&repo, &["checkout", "-q", "-b", "feature"]);
        // Работа на ветке: правка модели + change, затем архивация до merge.
        edit_cmp_001(&repo);
        write_openspec_change(&repo, "add-limits");
        git(&repo, &["add", "-A"]);
        git(&repo, &["commit", "-q", "-m", "правка модели change'ом"]);
        let archive = repo.join("openspec/changes/archive");
        std::fs::create_dir_all(&archive).expect("mkdir archive");
        // У архива OpenSpec имя каталога может нести префикс даты.
        std::fs::rename(
            repo.join("openspec/changes/add-limits"),
            archive.join("2026-10-07-add-limits"),
        )
        .expect("archive move");
        git(&repo, &["add", "-A"]);
        git(&repo, &["commit", "-q", "-m", "архивация до merge"]);
        let report = guard(&repo, Some("main"), &[]).expect("guard");
        assert!(
            report.passed,
            "архивация change внутри диапазона обязана покрывать: {report:?}"
        );
        assert_eq!(
            report.archived_in_range,
            vec!["openspec:2026-10-07-add-limits".to_string()],
            "каталог архива сопоставляется целиком, с префиксом даты"
        );
        assert_eq!(
            report.covered,
            vec![(
                "model/CMP-001.md".to_string(),
                "openspec:2026-10-07-add-limits".to_string()
            )]
        );
        let text = render_guard(&report);
        assert!(text.contains("заархивированным в диапазоне"), "{text}");
        // Новая правка того же файла после архивации — уже мимо покрытия:
        // архив описывает правки СВОЕГО диапазона, а не всё, что случится потом.
        std::fs::write(
            repo.join("model/CMP-001.md"),
            "---\nid: CMP-001\ntype: cmp\ntitle: Приём платежей\nstatus: active\n---\n\
             depends_on:\n  - CMP-003\n",
        )
        .expect("edit");
        git(&repo, &["add", "-A"]);
        git(&repo, &["commit", "-q", "-m", "правка без change"]);
        let report = guard(&repo, Some("HEAD~1"), &[]).expect("guard");
        assert_eq!(
            report.violations,
            vec!["model/CMP-001.md".to_string()],
            "правка без покрытия обязана быть красной: {report:?}"
        );
    }

    /// F1 + ADR-055 (обязательный тест задания): change, созданный ВНУТРИ
    /// диапазона прогона исполнителя, правку НЕ узаконивает (`self_approved`);
    /// change, существовавший ДО диапазона, — узаконивает. Иначе F1 открывал
    /// бы обход правила владения, закрытый 0.3.12 для самодельной дельты.
    #[test]
    fn guard_ownership_rejects_change_created_inside_range() {
        // Сценарий A: change создан в диапазоне исполнителя → FAIL по владению.
        let tmp = tempfile::tempdir().expect("tmp");
        let repo = tmp.path().join("repo");
        make_openspec_repo(&repo);
        let base = git_out(&repo, &["rev-parse", "HEAD"]);
        write_openspec_change(&repo, "add-limits");
        edit_cmp_001(&repo);
        git(&repo, &["add", "-A"]);
        git(&repo, &["commit", "-q", "-m", "работа исполнителя"]);
        let range = crate::control::AgentRange::probe(&repo, &base).expect("диапазон");
        let options = GuardOptions {
            sources: None,
            agent_range: Some(range),
        };
        let report = guard_with(&repo, Some(&base), &[], &options).expect("guard");
        assert!(!report.passed, "{report:?}");
        assert!(
            report.violations.is_empty(),
            "это не «нет покрытия», а самоодобрение: {report:?}"
        );
        assert_eq!(
            report.self_approved,
            vec![(
                "model/CMP-001.md".to_string(),
                vec!["openspec:add-limits".to_string()]
            )]
        );
        let text = render_guard(&report);
        assert!(text.contains("self_approved"), "{text}");
        assert!(text.contains("openspec:add-limits"), "{text}");
        assert!(text.contains("ADR-055"), "{text}");

        // Сценарий B: change принят владельцем ДО диапазона → PASS.
        let tmp2 = tempfile::tempdir().expect("tmp2");
        let repo2 = tmp2.path().join("repo");
        make_openspec_repo(&repo2);
        write_openspec_change(&repo2, "add-limits");
        git(&repo2, &["add", "-A"]);
        git(&repo2, &["commit", "-q", "-m", "change владельца"]);
        let base2 = git_out(&repo2, &["rev-parse", "HEAD"]);
        edit_cmp_001(&repo2);
        git(&repo2, &["add", "-A"]);
        git(&repo2, &["commit", "-q", "-m", "правка по change"]);
        let range2 = crate::control::AgentRange::probe(&repo2, &base2).expect("диапазон");
        let options = GuardOptions {
            sources: None,
            agent_range: Some(range2),
        };
        let report = guard_with(&repo2, Some(&base2), &[], &options).expect("guard");
        assert!(
            report.passed,
            "change вне диапазона исполнителя узаконивает правку: {report:?}"
        );
        assert!(report.self_approved.is_empty(), "{report:?}");
    }

    /// F1 + ADR-055, симметрия: самодельная дельта в диапазоне исполнителя —
    /// то же самоодобрение, что и change; без диапазона поведение обычного
    /// CI не изменилось (покрытие засчитывается).
    #[test]
    fn guard_ownership_rejects_delta_created_inside_range() {
        let tmp = tempfile::tempdir().expect("tmp");
        let repo = tmp.path().join("repo");
        make_guard_repo(&repo);
        let base = git_out(&repo, &["rev-parse", "HEAD"]);
        std::fs::write(repo.join("model/adr/ADR-003.md"), "# ADR-003 v2\n").expect("edit");
        let path = new(&repo, "update-adr-003").expect("new");
        let body = std::fs::read_to_string(&path).expect("read");
        std::fs::write(&path, format!("{body}\nЗатронут ADR-003 (таймауты).\n")).expect("mention");
        git(&repo, &["add", "-A"]);
        git(&repo, &["commit", "-q", "-m", "работа исполнителя"]);
        // Без диапазона — обычный гейт: покрытие засчитывается (как прежде).
        let report = guard(&repo, Some(&base), &[]).expect("guard");
        assert!(report.passed, "{report:?}");
        // С диапазоном — самоодобрение.
        let range = crate::control::AgentRange::probe(&repo, &base).expect("диапазон");
        let options = GuardOptions {
            sources: None,
            agent_range: Some(range),
        };
        let report = guard_with(&repo, Some(&base), &[], &options).expect("guard");
        assert!(!report.passed, "{report:?}");
        assert_eq!(
            report.self_approved,
            vec![(
                "model/adr/ADR-003.md".to_string(),
                vec!["delta:update-adr-003".to_string()]
            )]
        );
    }

    /// F1: состав источников — `[delta] sources` кейсового `arch-harness.toml`;
    /// дефолт — авто-детект по каталогу `openspec/`. Опечатка в имени источника
    /// и пустой список — ошибка (конфиг контура не отключается молча).
    #[test]
    fn coverage_sources_from_case_config_and_auto_detect() {
        let tmp = tempfile::tempdir().expect("tmp");
        let repo = tmp.path().join("repo");
        make_openspec_repo(&repo);
        assert_eq!(
            coverage_sources(&repo).expect("sources"),
            vec![CoverageSource::Spine, CoverageSource::Openspec],
            "openspec/ есть — оба источника"
        );
        std::fs::write(
            repo.join("arch-harness.toml"),
            "[delta]\nsources = [\"openspec\"]\n",
        )
        .expect("config");
        assert_eq!(
            coverage_sources(&repo).expect("sources"),
            vec![CoverageSource::Openspec]
        );
        std::fs::write(repo.join("arch-harness.toml"), "[delta]\nsources = []\n").expect("config");
        assert!(
            coverage_sources(&repo).is_err(),
            "пустой список — ошибка конфигурации"
        );
        std::fs::write(
            repo.join("arch-harness.toml"),
            "[delta]\nsources = [\"opensec\"]\n",
        )
        .expect("config");
        let err = coverage_sources(&repo).expect_err("неизвестный источник");
        assert!(err.to_string().contains("opensec"), "{err}");
        // Без openspec/ и без конфига — только spine (поведение до 0.3.14).
        let tmp2 = tempfile::tempdir().expect("tmp2");
        let repo2 = tmp2.path().join("repo");
        make_guard_repo(&repo2);
        assert_eq!(
            coverage_sources(&repo2).expect("sources"),
            vec![CoverageSource::Spine]
        );
    }

    /// F1: `sources = ["spine"]` отключает покрытие от changes `OpenSpec`;
    /// `sources = ["openspec"]` — от дельт (состав задаёт кейс, не гейт).
    #[test]
    fn guard_respects_sources_of_case_config() {
        let tmp = tempfile::tempdir().expect("tmp");
        let repo = tmp.path().join("repo");
        make_openspec_repo(&repo);
        write_openspec_change(&repo, "add-limits");
        edit_cmp_001(&repo);
        // Кейс объявил: покрытие — только дельты Spine.
        std::fs::write(
            repo.join("arch-harness.toml"),
            "[delta]\nsources = [\"spine\"]\n",
        )
        .expect("config");
        let report = guard(&repo, None, &[]).expect("guard");
        assert!(!report.passed, "{report:?}");
        assert_eq!(report.violations, vec!["model/CMP-001.md".to_string()]);
        assert_eq!(report.sources, vec!["spine".to_string()]);
        // Кейс на чистом OpenSpec: дельта не считается, change — считается.
        std::fs::write(
            repo.join("arch-harness.toml"),
            "[delta]\nsources = [\"openspec\"]\n",
        )
        .expect("config");
        let report = guard(&repo, None, &[]).expect("guard");
        assert!(report.passed, "{report:?}");
        // …а DELTA.md при sources = ["openspec"] покрытием не является.
        std::fs::remove_dir_all(repo.join("openspec/changes/add-limits")).expect("remove change");
        let path = new(&repo, "cmp-001-limits").expect("new");
        let body = std::fs::read_to_string(&path).expect("read");
        std::fs::write(&path, format!("{body}\nПравка model/CMP-001.md.\n")).expect("mention");
        let report = guard(&repo, None, &[]).expect("guard");
        assert!(!report.passed, "{report:?}");
        assert_eq!(report.violations, vec!["model/CMP-001.md".to_string()]);
        let text = render_guard(&report);
        assert!(
            text.contains("DELTA.md в этом репозитории покрытием не считается"),
            "подсказка обязана вести к change OpenSpec, а не к delta new: {text}"
        );
        assert!(!text.contains("arch-be delta new"), "{text}");
    }

    /// F1: в репозитории на чистом `OpenSpec` `delta new`/`delta_propose`
    /// подсказывают change средствами `OpenSpec` вместо создания DELTA.md.
    #[tokio::test]
    async fn delta_propose_refused_when_sources_openspec_only() {
        let tmp = tempfile::tempdir().expect("tmp");
        let repo = tmp.path();
        make_openspec_repo(repo);
        // Без секции — подсказки нет (источник spine включён авто-детектом).
        assert_eq!(openspec_only_hint(repo).expect("hint"), None);
        std::fs::write(
            repo.join("arch-harness.toml"),
            "[delta]\nsources = [\"openspec\"]\n",
        )
        .expect("config");
        let hint = openspec_only_hint(repo)
            .expect("hint")
            .expect("есть подсказка");
        assert!(hint.contains("openspec/changes/"), "{hint}");
        assert!(hint.contains("не пишет"), "{hint}");
        // MCP-инструмент — мягкая ошибка с той же подсказкой, DELTA.md нет.
        let ctx = tool_ctx(repo);
        let out = DeltaProposeTool
            .call(json!({"name": "add-limits", "path": "."}), &ctx)
            .await
            .expect("вызов");
        assert!(out.is_error, "{}", out.content);
        assert!(out.content.contains("OpenSpec"), "{}", out.content);
        assert!(!repo.join("changes/add-limits/DELTA.md").exists());
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
