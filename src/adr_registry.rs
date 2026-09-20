//! Глобальный реестр ADR (PR-2, ADR-036): агрегация решений по набору проектов.
//!
//! Ответ на замечание ревью «нет глобального реестра ADR» в рамках позиции
//! single-user (ADR-033): реестр — АГРЕГАЦИЯ ФАЙЛОВ, а не сервис. Команда
//! `arch-be adr registry <ROOT>` сканирует сам `ROOT` и его непосредственные
//! подкаталоги (каждый — проект/репозиторий) и собирает единый индекс из
//! двух источников:
//!
//! - `docs/adr/*.md` — прозаические ADR: номер из заголовка `# ADR-NNN.` /
//!   `# ADR-NNN:`, заголовок — остаток заголовка, `- Date:` и `- Status:`
//!   из шапки;
//! - `model/ADR-*.md` — типизированные сущности (ADR-003): frontmatter
//!   `id`/`title`/`status`/`date` (разбор — [`crate::model::parse_entity`];
//!   файлы сущностей других типов игнорируются — фильтр по префиксу `ADR-`).
//!
//! Проза и типизированная сущность одного проекта с одним номером — это
//! ДВА ПРЕДСТАВЛЕНИЯ ОДНОГО решения, а не два решения: реестр сливает их в
//! одну запись с двумя гранями (`prose`/`model`, поле `source` —
//! `docs/adr+model`). Слияние выполняется только когда в группе
//! (проект, номер) ровно по одной грани каждого вида; иначе это уже дубль
//! внутри представления, и его разбирает `number_collision`.
//!
//! Находки: расхождение граней (проза и модель одного решения разошлись по
//! заголовку, статусу или дате — единственная находка на пару, с обоими
//! значениями), коллизия номеров (два файла с одним `ADR-NNN` внутри одного
//! проекта в ОДНОМ представлении — локальное пространство номеров
//! неоднозначно; либо один `ADR-NNN` в разных проектах с разными
//! заголовками — глобального пространства номеров нет), дубль заголовка
//! (одинаковый заголовок у разных номеров — возможный дубль решения),
//! запись без даты/статуса. Проект без ADR — не находка: просто не попадает
//! в индекс.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::error::{HarnessError, Result};
use crate::llm::ToolSpec;
use crate::tool::{Tool, ToolContext, ToolOutput};

/// Каталоги, которые не считаются проектами при сканировании `ROOT`.
const SKIP_DIRS: [&str; 3] = [".git", "target", "node_modules"];

/// Одна грань решения: файл-источник (проза `docs/adr/*.md` или
/// типизированная сущность `model/ADR-*.md`) и разобранные из него поля.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdrFace {
    /// Файл грани относительно каталога проекта.
    pub file: String,
    /// Заголовок решения в этой грани.
    pub title: String,
    /// Статус (`None` — поле не заполнено).
    #[serde(default)]
    pub status: Option<String>,
    /// Дата (`None` — поле не заполнено).
    #[serde(default)]
    pub date: Option<String>,
    /// Шапка есть, но не распознана ни в одной из понимаемых форм (Н9):
    /// пустой статус тогда означает «не прочитали», а не «не заполнено».
    #[serde(default)]
    pub status_unparsed: bool,
}

/// Одна запись реестра ADR: решение проекта. Если решение описано и прозой,
/// и типизированной сущностью, запись несёт две грани ([`Self::prose`] и
/// [`Self::model`]), а поля верхнего уровня отражают её основную грань
/// (проза, а при её отсутствии — модель).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistryEntry {
    /// Проект (имя непосредственного подкаталога `ROOT`; сам `ROOT` — его
    /// имя каталога).
    pub project: String,
    /// Номер ADR (`ADR-007` → 7).
    pub number: u64,
    /// Заголовок решения (основной грани).
    pub title: String,
    /// Статус (`None` — пропуск обязательного поля, находка).
    pub status: Option<String>,
    /// Дата (`None` — пропуск обязательного поля, находка).
    pub date: Option<String>,
    /// Источник записи: `docs/adr` (проза), `model` (типизированная
    /// сущность ADR-003) или `docs/adr+model` (слитая пара граней).
    pub source: String,
    /// Файл основной грани записи относительно каталога проекта
    /// (для диагностики).
    pub file: String,
    /// Прозаическая грань (`docs/adr/...`), если решение ею описано.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prose: Option<AdrFace>,
    /// Типизированная грань (`model/...`), если решение ею описано.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<AdrFace>,
    /// Шапка прозы не распознана (Н9): живёт на записи, а не только на грани,
    /// потому что `prose` строится ИЗ записи и флаг обязан доехать до неё.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub status_unparsed: bool,
}

impl RegistryEntry {
    /// Идентификатор вида `ADR-007` — для таблиц и находок.
    #[must_use]
    pub fn id(&self) -> String {
        format!("ADR-{:03}", self.number)
    }
}

/// Находка реестра.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistryFinding {
    /// Тип находки: `prose_model_divergence` | `number_collision` |
    /// `title_duplicate` | `missing_fields` | `prose_header_unparsed`.
    pub kind: String,
    /// Описание (проекты, номера, заголовки).
    pub message: String,
}

/// Отчёт реестра ADR (сериализуется в `--json` как `{root, entries, findings}`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistryReport {
    /// Сканированный корень.
    pub root: PathBuf,
    /// Записи индекса (детерминированная сортировка: проект, номер,
    /// источник, файл).
    pub entries: Vec<RegistryEntry>,
    /// Находки (расхождения прозы и модели, коллизии номеров, дубли
    /// заголовков, пропуски полей).
    pub findings: Vec<RegistryFinding>,
}

/// Exit-код команды: `--strict` + любая находка → 1 (гейт для CI);
/// по умолчанию — 0 (реестр — отчёт, а не гейт).
#[must_use]
pub fn exit_code(report: &RegistryReport, strict: bool) -> i32 {
    i32::from(strict && !report.findings.is_empty())
}

/// Разобранный прозаический ADR (заголовок + шапка).
#[derive(Debug)]
struct ProseAdr {
    /// Номер (`ADR-007` → 7).
    number: u64,
    /// Заголовок решения.
    title: String,
    /// Дата из `- Date:` (может отсутствовать).
    date: Option<String>,
    /// Статус из `- Status:` (может отсутствовать).
    status: Option<String>,
    /// В шапке есть строка, похожая на статус, но ни одна форма не разобрана:
    /// это находка `prose_header_unparsed`, а не «расхождение с моделью».
    status_unparsed: bool,
}

/// Имена полей шапки ADR: английские и русские, в любом регистре.
const DATE_NAMES: [&str; 2] = ["Date", "Дата"];
const STATUS_NAMES: [&str; 2] = ["Status", "Статус"];
/// Имена поля шапки ADR с моделью-автором документа (J3, ADR-048): метка
/// автора живёт в самом документе, а не в памяти сессии.
const AUTHOR_NAMES: [&str; 2] = ["Author-model", "Модель-автор"];
/// Сколько строк от начала документа считается шапкой ADR: поля ниже —
/// часть тела и полем шапки не считаются.
const HEADER_WINDOW_LINES: usize = 40;

/// Модель-автор документа из шапки ADR-файла (`- Author-model: …`,
/// `- Модель-автор: …`); `None` — поле не указано.
///
/// Терпимость к разметке — та же, что у `Status`/`Статус` (Н9): понимаются
/// `- **Модель-автор**: X`, `**Author-model**: X` и головное имя со значением
/// следующей строкой. Значение из шапки закоммичено вместе с документом,
/// поэтому его нельзя «вспомнить задним числом»: правка шапки меняет хэш
/// документа и обесценивает отчёт (`rubric_report_stale`).
///
/// `human` и `human:<имя>` — документ написан человеком; любая судья-модель от
/// такого автора отлична (разбор метки — [`crate::judge`]).
#[must_use]
pub fn author_model_of(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    author_model_in(&text)
}

/// То же по уже прочитанному тексту документа.
#[must_use]
pub fn author_model_in(text: &str) -> Option<String> {
    let mut pending = false;
    for line in text.lines().take(HEADER_WINDOW_LINES) {
        if line.trim().is_empty() {
            continue;
        }
        if pending {
            let value = strip_prefix(line).trim_matches('*').trim().to_string();
            if !value.is_empty() {
                return Some(value);
            }
        }
        if let Some(value) = field_value(line, &AUTHOR_NAMES) {
            if !value.is_empty() {
                return Some(value);
            }
        }
        pending = bare_author_field(line);
    }
    None
}

/// Строка — ГОЛОВНОЕ имя поля автора без значения (`## Модель-автор`):
/// значение стоит следующей непустой строкой.
fn bare_author_field(line: &str) -> bool {
    let t = strip_prefix(line);
    let t = t.trim_end_matches(':').trim().trim_matches('*').trim();
    AUTHOR_NAMES.iter().any(|n| t.eq_ignore_ascii_case(n))
}

/// Снимает markdown-разметку начала строки: список, заголовок, жирный.
fn strip_prefix(line: &str) -> String {
    let t = line.trim();
    let t = t.trim_start_matches(['-', '*', '#', ' ']).trim();
    t.trim_start_matches("**").trim().to_string()
}

/// Значение поля шапки из строки: `Status: X`, `**Статус**: X`,
/// `- **Status:** X`. Сравнение имён — без учёта регистра; закрывающие `**`
/// перед двоеточием допускаются. `None` — это не поле (или имя другое).
fn field_value(line: &str, names: &[&str]) -> Option<String> {
    let t = strip_prefix(line);
    for name in names {
        let Some(head) = t.get(..name.len()) else {
            continue;
        };
        if !head.eq_ignore_ascii_case(name) {
            continue;
        }
        let rest = &t[name.len()..];
        let rest = rest.strip_prefix("**").unwrap_or(rest);
        let Some(value) = rest.strip_prefix(':') else {
            continue;
        };
        return Some(value.trim().trim_matches('*').trim().to_string());
    }
    None
}

/// Поле шапки ADR.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HeaderField {
    /// Дата решения.
    Date,
    /// Статус решения.
    Status,
}

/// Имя поля, если строка — ГОЛОВНОЕ имя без значения (`## Статус`): тогда
/// значение стоит следующей непустой строкой.
fn bare_field_name(line: &str) -> Option<HeaderField> {
    let t = strip_prefix(line);
    let t = t.trim_end_matches(':').trim();
    if STATUS_NAMES.iter().any(|n| t.eq_ignore_ascii_case(n)) {
        return Some(HeaderField::Status);
    }
    if DATE_NAMES.iter().any(|n| t.eq_ignore_ascii_case(n)) {
        return Some(HeaderField::Date);
    }
    None
}

/// Похожа ли строка на поле `Status`/`Статус`, которое не удалось разобрать
/// ни в одной из понимаемых форм (`Status = Accepted`, `Статус — Accepted`).
fn looks_like_status_field(line: &str) -> bool {
    let lowered = strip_prefix(line).to_lowercase();
    STATUS_NAMES
        .iter()
        .any(|n| lowered.starts_with(&n.to_lowercase()))
}

/// Парсит прозаический ADR из `docs/adr/*.md`: первый заголовок
/// `# ADR-NNN.` / `# ADR-NNN:`, шапка — `Date`/`Дата` и `Status`/`Статус`.
///
/// Формы шапки, которые понимаются (Н9 волны C 0.3.4): `- Status: Accepted`,
/// `- **Status**: Accepted`, `**Статус**: Accepted`, `Статус: Accepted`,
/// `## Статус` со значением следующей строкой. Раньше понималась ровно одна
/// форма, и документ, написанный по-русски, выглядел «расхождением с моделью»
/// во всех записях проекта — ложная находка вместо честной.
///
/// `Ok(None)` — файл не является ADR (нет заголовка с номером).
fn parse_prose_adr(text: &str) -> Result<Option<ProseAdr>> {
    let heading_re = regex::Regex::new(r"^#{1,3}\s+(ADR-[0-9]+)\s*[:.]\s*(.*?)\s*$")
        .map_err(|e| HarnessError::Model(format!("внутренний regex реестра: {e}")))?;
    let id_re = crate::model::id_re()?;

    let mut parsed: Option<ProseAdr> = None;
    // Незакрытое «головное» поле: `## Статус` без значения на той же строке.
    let mut pending: Option<HeaderField> = None;
    for line in text.lines() {
        if parsed.is_none() {
            if let Some(caps) = heading_re.captures(line) {
                // Номер — через общий regex идентификатора модели (ADR-003).
                let id_token = &caps[1];
                let Some(num_caps) = id_re.captures(id_token) else {
                    continue;
                };
                let Ok(number) = num_caps[2].parse::<u64>() else {
                    continue;
                };
                parsed = Some(ProseAdr {
                    number,
                    title: caps[2].to_string(),
                    date: None,
                    status: None,
                    status_unparsed: false,
                });
            }
            continue;
        }
        if let Some(p) = &mut parsed {
            if line.trim().is_empty() {
                continue;
            }
            // Значение «головного» поля — следующая непустая строка.
            if let Some(field) = pending.take() {
                let value = strip_prefix(line).trim_matches('*').trim().to_string();
                if !value.is_empty() {
                    match field {
                        HeaderField::Status => p.status.get_or_insert(value),
                        HeaderField::Date => p.date.get_or_insert(value),
                    };
                }
                continue;
            }
            if let Some(value) = field_value(line, &STATUS_NAMES) {
                if !value.is_empty() {
                    p.status.get_or_insert(value);
                }
                continue;
            }
            if let Some(value) = field_value(line, &DATE_NAMES) {
                if !value.is_empty() {
                    p.date.get_or_insert(value);
                }
                continue;
            }
            if let Some(field) = bare_field_name(line) {
                pending = Some(field);
            }
        }
    }
    if let Some(p) = &mut parsed {
        p.status_unparsed = p.status.is_none() && unparsed_status_header(text);
    }
    Ok(parsed)
}

/// Есть ли в шапке строка, похожая на поле `Status`/`Статус`, которое не
/// удалось разобрать ни в одной форме.
fn unparsed_status_header(text: &str) -> bool {
    !text
        .lines()
        .take(60)
        .any(|l| field_value(l, &STATUS_NAMES).is_some())
        && text.lines().take(60).any(looks_like_status_field)
}

/// Собирает записи проекта `project` из каталога `dir` (два источника).
fn collect_project(project: &str, dir: &Path, entries: &mut Vec<RegistryEntry>) -> Result<()> {
    // Источник 1: прозаические ADR из docs/adr/*.md.
    let docs_dir = dir.join("docs/adr");
    if docs_dir.is_dir() {
        let mut files: Vec<PathBuf> = Vec::new();
        let rd = std::fs::read_dir(&docs_dir).map_err(|e| HarnessError::io(&docs_dir, e))?;
        for entry in rd {
            let entry = entry.map_err(|e| HarnessError::io(&docs_dir, e))?;
            let p = entry.path();
            if p.is_file() && p.extension().is_some_and(|ext| ext == "md") {
                files.push(p);
            }
        }
        files.sort();
        for file in files {
            let text = std::fs::read_to_string(&file).map_err(|e| HarnessError::io(&file, e))?;
            let Some(adr) = parse_prose_adr(&text)? else {
                continue; // не ADR-файл (без заголовка # ADR-NNN.)
            };
            let rel = file
                .strip_prefix(dir)
                .map_or_else(|_| file.clone(), PathBuf::from);
            entries.push(RegistryEntry {
                project: project.to_string(),
                number: adr.number,
                title: adr.title,
                status: adr.status,
                date: adr.date,
                source: "docs/adr".to_string(),
                file: rel.to_string_lossy().replace('\\', "/"),
                prose: None,
                model: None,
                status_unparsed: adr.status_unparsed,
            });
        }
    }

    // Источник 2: типизированные сущности model/ADR-*.md (ADR-003).
    let model_dir = dir.join("model");
    if model_dir.is_dir() {
        let mut files: Vec<PathBuf> = Vec::new();
        let rd = std::fs::read_dir(&model_dir).map_err(|e| HarnessError::io(&model_dir, e))?;
        for entry in rd {
            let entry = entry.map_err(|e| HarnessError::io(&model_dir, e))?;
            let p = entry.path();
            let is_adr = p
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("ADR-"))
                && p.extension().is_some_and(|e| e.eq_ignore_ascii_case("md"));
            if p.is_file() && is_adr {
                files.push(p);
            }
        }
        files.sort();
        for file in files {
            let text = std::fs::read_to_string(&file).map_err(|e| HarnessError::io(&file, e))?;
            // Неразбираемый файл (битый frontmatter) — не сущность модели,
            // пропускаем: её проблемы — зона `arch-be model validate`.
            let Ok(entity) = crate::model::parse_entity(&file, &text) else {
                continue;
            };
            // В model/ живут и другие типы сущностей — берём только ADR-.
            if entity.kind != crate::model::EntityKind::Adr {
                continue;
            }
            let Some((_, number)) = crate::model::parse_id(&entity.id) else {
                continue;
            };
            let rel = file
                .strip_prefix(dir)
                .map_or_else(|_| file.clone(), PathBuf::from);
            entries.push(RegistryEntry {
                project: project.to_string(),
                number,
                title: entity.title,
                status: Some(entity.status),
                date: entity.date,
                source: "model".to_string(),
                status_unparsed: false,
                file: rel.to_string_lossy().replace('\\', "/"),
                prose: None,
                model: None,
            });
        }
    }
    Ok(())
}

/// Каноническая форма статуса для сравнения граней: регистр не важен, а
/// хвостовая оговорка в скобках — пояснение к статусу, а не другой статус
/// («Proposed (A3 — решение архитектора)» ≡ «proposed»). Разные статусные
/// слова («accepted» и «proposed») остаются разными.
fn canonical_status(status: Option<&str>) -> Option<String> {
    let raw = status?.trim();
    let base = raw.split('(').next().unwrap_or_default().trim();
    let base = base.trim_end_matches(['.', ',', ';']).trim();
    (!base.is_empty()).then(|| base.to_lowercase())
}

/// Каноническая форма даты для сравнения граней: значимый непустой текст.
fn canonical_date(date: Option<&str>) -> Option<String> {
    let raw = date?.trim();
    (!raw.is_empty()).then(|| raw.to_string())
}

/// Грань записи по её полям (файл + разобранное решение).
fn face_of(entry: &RegistryEntry) -> AdrFace {
    AdrFace {
        file: entry.file.clone(),
        title: entry.title.clone(),
        status: entry.status.clone(),
        date: entry.date.clone(),
        status_unparsed: entry.status_unparsed,
    }
}

/// Сливает пару «проза + типизированная сущность» одного решения в одну
/// запись с двумя гранями. Основная грань — прозаическая (её поля попадают
/// в поля верхнего уровня для совместимости с прежним видом отчёта), а
/// отсутствующие у неё дата/статус подставляются из типизированной грани.
fn merge_pair(prose: &RegistryEntry, model: &RegistryEntry) -> RegistryEntry {
    let title = if prose.title.trim().is_empty() {
        model.title.clone()
    } else {
        prose.title.clone()
    };
    let status = prose.status.clone().or_else(|| model.status.clone());
    let date = prose.date.clone().or_else(|| model.date.clone());
    RegistryEntry {
        project: prose.project.clone(),
        number: prose.number,
        title,
        status,
        date,
        source: "docs/adr+model".to_string(),
        file: prose.file.clone(),
        status_unparsed: prose.status_unparsed,
        prose: Some(face_of(prose)),
        model: Some(face_of(model)),
    }
}

/// Сливает прозаическую и типизированную грань одного проекта и номера в
/// одну запись. Слияние — только когда в группе (проект, номер) ровно по
/// одной грани каждого вида: иначе соответствие неоднозначно, и группа
/// остаётся как есть — её разбирает `number_collision`.
fn merge_faces(entries: Vec<RegistryEntry>) -> Vec<RegistryEntry> {
    let mut groups: BTreeMap<(String, u64), Vec<RegistryEntry>> = BTreeMap::new();
    for e in entries {
        groups
            .entry((e.project.clone(), e.number))
            .or_default()
            .push(e);
    }
    let mut merged = Vec::new();
    for (_, group) in groups {
        let prose_count = group.iter().filter(|e| e.source == "docs/adr").count();
        let model_count = group.iter().filter(|e| e.source == "model").count();
        if prose_count != 1 || model_count != 1 {
            merged.extend(group);
            continue;
        }
        let mut prose: Option<RegistryEntry> = None;
        let mut model: Option<RegistryEntry> = None;
        for e in group {
            match e.source.as_str() {
                "docs/adr" => prose = Some(e),
                "model" => model = Some(e),
                _ => merged.push(e),
            }
        }
        match (prose, model) {
            (Some(p), Some(m)) => merged.push(merge_pair(&p, &m)),
            // Недостижимо при посчитанных выше счётчиках; подстраховка без паники.
            (p, m) => merged.extend(p.into_iter().chain(m)),
        }
    }
    merged
}

/// Находки по собранному индексу.
fn find_issues(entries: &[RegistryEntry]) -> Vec<RegistryFinding> {
    let mut findings = Vec::new();

    // Расхождение граней одного решения: проза и типизированная сущность
    // одного проекта и номера описывают решение по-разному. Это не коллизия
    // номера (номер как раз общий), а дрейф: архитектору нужно свести
    // источники к одному тексту. Значения обеих граней — в сообщении.
    for e in entries {
        let (Some(prose), Some(model)) = (&e.prose, &e.model) else {
            continue;
        };
        let mut drift: Vec<String> = Vec::new();
        if prose.title.trim() != model.title.trim() {
            drift.push(format!(
                "заголовок: проза «{}» ≠ модель «{}»",
                prose.title.trim(),
                model.title.trim()
            ));
        }
        // Шапку не разобрали — это НЕ расхождение с моделью: неизвестно, чему
        // она равна. Отдельная находка с примером ожидаемого формата (Н9).
        if prose.status_unparsed {
            findings.push(RegistryFinding {
                kind: "prose_header_unparsed".to_string(),
                message: format!(
                    "{} {} — шапка прозы не распознана: строка статуса не в одной из \
                     понимаемых форм (файл: {}). Ожидается `- Status: Accepted` или \
                     `- **Статус**: Accepted`; поле, которое не удалось прочитать, \
                     НЕ считается расхождением с моделью",
                    e.project,
                    e.id(),
                    prose.file
                ),
            });
        } else if canonical_status(prose.status.as_deref())
            != canonical_status(model.status.as_deref())
        {
            drift.push(format!(
                "статус: проза «{}» ≠ модель «{}»",
                prose.status.as_deref().unwrap_or("—"),
                model.status.as_deref().unwrap_or("—")
            ));
        }
        if canonical_date(prose.date.as_deref()) != canonical_date(model.date.as_deref()) {
            drift.push(format!(
                "дата: проза «{}» ≠ модель «{}»",
                prose.date.as_deref().unwrap_or("—"),
                model.date.as_deref().unwrap_or("—")
            ));
        }
        if !drift.is_empty() {
            findings.push(RegistryFinding {
                kind: "prose_model_divergence".to_string(),
                message: format!(
                    "{} {} — расхождение прозы и модели: {} (файлы: {}, {})",
                    e.project,
                    e.id(),
                    drift.join("; "),
                    prose.file,
                    model.file
                ),
            });
        }
    }

    // Коллизия номеров ВНУТРИ одного проекта: два файла с одним ADR-NNN.
    // В отличие от межпроектной коллизии заголовки роли не играют: дубль
    // номера в локальном пространстве делает ссылку на него неоднозначной
    // и при совпадающем заголовке (копия файла под другим именем).
    let mut by_project_number: BTreeMap<(&str, u64), Vec<&RegistryEntry>> = BTreeMap::new();
    for e in entries {
        by_project_number
            .entry((e.project.as_str(), e.number))
            .or_default()
            .push(e);
    }
    for ((project, number), group) in &by_project_number {
        if group.len() < 2 {
            continue;
        }
        let detail = group
            .iter()
            .map(|e| format!("{} «{}»", e.file, e.title))
            .collect::<Vec<_>>()
            .join(", ");
        let distinct_titles: std::collections::BTreeSet<&str> =
            group.iter().map(|e| e.title.as_str()).collect();
        let nuance = if distinct_titles.len() > 1 {
            "разные заголовки — конфликт решений под одним номером"
        } else {
            "тот же заголовок — вероятная копия файла"
        };
        let sources: std::collections::BTreeSet<&str> =
            group.iter().map(|e| e.source.as_str()).collect();
        let reps = sources.into_iter().collect::<Vec<_>>().join(" + ");
        findings.push(RegistryFinding {
            kind: "number_collision".to_string(),
            message: format!(
                "ADR-{number:03} дублируется в проекте {project} ({reps}): {detail} \
                 ({nuance}; номер обязан быть уникальным внутри одного \
                 представления — ссылки на номер неоднозначны)"
            ),
        });
    }

    // Коллизия номеров: один ADR-NNN в РАЗНЫХ проектах с РАЗНЫМИ заголовками.
    let mut by_number: BTreeMap<u64, Vec<&RegistryEntry>> = BTreeMap::new();
    for e in entries {
        by_number.entry(e.number).or_default().push(e);
    }
    for (number, group) in &by_number {
        let mut projects: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        for e in group {
            projects
                .entry(e.project.as_str())
                .or_default()
                .push(e.title.as_str());
        }
        let distinct_titles: std::collections::BTreeSet<&str> =
            group.iter().map(|e| e.title.as_str()).collect();
        // Коллизия — только если разные проекты И разные заголовки.
        if projects.len() > 1 && distinct_titles.len() > 1 {
            let detail = projects
                .iter()
                .map(|(p, titles)| {
                    let mut t = titles.clone();
                    t.sort_unstable();
                    t.dedup();
                    format!("{p}: «{}»", t.join("», «"))
                })
                .collect::<Vec<_>>()
                .join("; ");
            findings.push(RegistryFinding {
                kind: "number_collision".to_string(),
                message: format!(
                    "ADR-{number:03} в разных проектах с разными заголовками: {detail} \
                     (глобального пространства номеров нет — ссылки на номер неоднозначны)"
                ),
            });
        }
    }

    // Дубль заголовка: одинаковый заголовок у разных номеров.
    let mut by_title: BTreeMap<String, Vec<&RegistryEntry>> = BTreeMap::new();
    for e in entries {
        let key = e.title.trim().to_lowercase();
        if !key.is_empty() {
            by_title.entry(key).or_default().push(e);
        }
    }
    for (title, group) in &by_title {
        let mut ids: Vec<String> = group
            .iter()
            .map(|e| format!("{} ({})", e.id(), e.project))
            .collect();
        ids.sort();
        ids.dedup();
        if ids.len() > 1 {
            // В сообщении — заголовок в исходном регистре (первой записи группы).
            let original = group.first().map_or(title.as_str(), |e| e.title.trim());
            findings.push(RegistryFinding {
                kind: "title_duplicate".to_string(),
                message: format!(
                    "заголовок «{original}» у разных номеров: {}",
                    ids.join(", ")
                ),
            });
        }
    }

    // Пропуск обязательных полей: нет даты или статуса.
    for e in entries {
        let missing: Vec<&str> = [
            e.date.is_none().then_some("дата"),
            e.status.is_none().then_some("статус"),
        ]
        .into_iter()
        .flatten()
        .collect();
        if !missing.is_empty() {
            findings.push(RegistryFinding {
                kind: "missing_fields".to_string(),
                message: format!(
                    "{} {} ({}): нет поля «{}»",
                    e.project,
                    e.id(),
                    e.file,
                    missing.join("», «")
                ),
            });
        }
    }
    findings
}

/// Строит реестр ADR по `ROOT`: сам каталог + непосредственные подкаталоги
/// (каждый — проект). Каталоги из [`SKIP_DIRS`] пропускаются; проект без
/// ADR молча не попадает в индекс.
///
/// # Errors
/// `ROOT` не каталог; нечего регистрировать — ни в самом `ROOT`, ни в
/// непосредственных подкаталогах не найдено ни одного ADR.
pub fn build_registry(root: &Path) -> Result<RegistryReport> {
    if !root.is_dir() {
        return Err(HarnessError::Model(format!(
            "реестр ADR: корень недоступен: {}",
            root.display()
        )));
    }
    let root_name = root
        .canonicalize()
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_else(|| ".".to_string());

    let mut entries = Vec::new();
    collect_project(&root_name, root, &mut entries)?;

    let mut projects: Vec<PathBuf> = Vec::new();
    let rd = std::fs::read_dir(root).map_err(|e| HarnessError::io(root, e))?;
    for entry in rd {
        let entry = entry.map_err(|e| HarnessError::io(root, e))?;
        let p = entry.path();
        if !p.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if SKIP_DIRS.contains(&name.as_str()) {
            continue;
        }
        projects.push(p);
    }
    projects.sort();
    for p in &projects {
        let name = p
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        collect_project(&name, p, &mut entries)?;
    }

    if entries.is_empty() {
        return Err(HarnessError::Model(format!(
            "реестр ADR: нечего регистрировать — ни в {}, ни в его непосредственных \
             подкаталогах не найдено ADR (источники: docs/adr/*.md, model/ADR-*.md)",
            root.display()
        )));
    }

    let mut entries = merge_faces(entries);
    entries.sort_by(|a, b| {
        (&a.project, a.number, &a.source, &a.file).cmp(&(&b.project, b.number, &b.source, &b.file))
    });
    let findings = find_issues(&entries);
    Ok(RegistryReport {
        root: root.to_path_buf(),
        entries,
        findings,
    })
}

/// Рендер реестра в markdown: таблица индекса + секция находок.
#[must_use]
pub fn render_markdown(report: &RegistryReport) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "# Реестр ADR: {}\n", report.root.display());
    let _ = writeln!(
        out,
        "| Проект | ADR | Заголовок | Статус | Дата | Источник |"
    );
    let _ = writeln!(out, "|---|---|---|---|---|---|");
    for e in &report.entries {
        let dash = "—";
        let _ = writeln!(
            out,
            "| {} | {} | {} | {} | {} | {} |",
            e.project,
            e.id(),
            e.title,
            e.status.as_deref().unwrap_or(dash),
            e.date.as_deref().unwrap_or(dash),
            e.source
        );
    }
    let _ = writeln!(out, "\n## Находки\n");
    if report.findings.is_empty() {
        let _ = writeln!(out, "- нет");
    } else {
        for f in &report.findings {
            let _ = writeln!(out, "- [{}] {}", f.kind, f.message);
        }
    }
    let _ = write!(
        out,
        "\nИтого: {} записей, {} находок",
        report.entries.len(),
        report.findings.len()
    );
    out
}

/// Инструменты домена: `adr_registry`.
#[must_use]
pub fn tools() -> Vec<Arc<dyn Tool>> {
    vec![Arc::new(AdrRegistryTool)]
}

/// Инструмент `adr_registry`: глобальный реестр ADR по набору проектов
/// (ADR-036) — JSON-вердикт со счётчиками и находками (мост в MCP,
/// транш 2 инверсии; read-only).
pub struct AdrRegistryTool;

#[derive(Debug, Deserialize)]
struct AdrRegistryArgs {
    /// Корневой каталог набора проектов.
    path: String,
    /// Строгий режим: находки делают `passed: false` (гейт; как `--strict`
    /// на CLI: exit-код 1 при любой находке). По умолчанию реестр — отчёт,
    /// `passed` всегда true.
    strict: Option<bool>,
}

#[async_trait]
impl Tool for AdrRegistryTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "adr_registry".into(),
            description: "Глобальный реестр ADR по набору проектов (ADR-036): скан самого ROOT \
                          и непосредственных подкаталогов, источники — docs/adr/*.md (проза) и \
                          model/ADR-*.md (типизированные сущности). Проза и сущность одного \
                          проекта с одним номером сливаются в одну запись с двумя гранями. \
                          Ответ — JSON: passed + счётчики entries/findings + находки \
                          (расхождение прозы и модели, коллизия номеров, дубль заголовка, \
                          пропуск даты/статуса) + report_markdown. Реестр — отчёт, \
                          а не гейт: passed=false только в strict-режиме при наличии находок"
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Корневой каталог набора проектов"},
                    "strict": {
                        "type": "boolean",
                        "description": "Гейт: passed=false при любой находке (по умолчанию false — отчёт)"
                    }
                },
                "required": ["path"]
            }),
        }
    }

    async fn call(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let args: AdrRegistryArgs = match serde_json::from_value(args) {
            Ok(a) => a,
            Err(e) => {
                return Ok(ToolOutput::err(format!(
                    "adr_registry: невалидные аргументы: {e}"
                )));
            }
        };
        let root = ctx.resolve(&args.path);
        let strict = args.strict.unwrap_or(false);
        let report = match build_registry(&root) {
            Ok(r) => r,
            Err(e) => return Ok(ToolOutput::err(format!("adr_registry: {e}"))),
        };
        let findings: Vec<Value> = report
            .findings
            .iter()
            .map(|f| json!({"kind": f.kind, "message": f.message}))
            .collect();
        let summary = format!(
            "Реестр ADR {}: {} записей, {} находок{}",
            report.root.display(),
            report.entries.len(),
            report.findings.len(),
            if strict { " (strict)" } else { "" }
        );
        let verdict = json!({
            "tool": "adr_registry",
            "passed": exit_code(&report, strict) == 0,
            "strict": strict,
            "entries": report.entries.len(),
            "findings": findings,
            "finding_count": report.findings.len(),
            "summary": summary,
            "report_markdown": render_markdown(&report),
        });
        // Сериализация собранного объекта не падает; запасной вариант — компактная форма.
        let text = serde_json::to_string_pretty(&verdict).unwrap_or_else(|_| verdict.to_string());
        Ok(ToolOutput::ok(text))
    }
}

#[cfg(test)]
mod author_tests {
    use super::*;

    /// Поле автора понимается в тех же формах, что `Status`: английское и
    /// русское имя, звёздочки, регистр, головное имя со значением следующей
    /// строкой (J3, ADR-048).
    #[test]
    fn author_field_forms_are_understood() {
        for (text, expected) in [
            (
                "# ADR-001\n\n- Author-model: claude-opus-4\n",
                "claude-opus-4",
            ),
            ("# ADR-001\n\n- Модель-автор: glm-5.2\n", "glm-5.2"),
            (
                "# ADR-001\n\n- **Модель-автор**: human:Иван\n",
                "human:Иван",
            ),
            ("# ADR-001\n\n**Author-model**: qwen3-max\n", "qwen3-max"),
            (
                "# ADR-001\n\n## Модель-автор\n\nclaude-opus-4\n",
                "claude-opus-4",
            ),
        ] {
            assert_eq!(
                author_model_in(text).as_deref(),
                Some(expected),
                "форма не разобрана: {text:?}"
            );
        }
    }

    /// Поля автора в документе нет — это не ошибка: старые ADR его не несут,
    /// и выдумывать автора механика не станет.
    #[test]
    fn missing_author_field_is_none_not_guess() {
        assert!(author_model_in("# ADR-001. Решение\n\n- Status: Accepted\n").is_none());
        // Тело документа ниже шапки полем не считается.
        let deep = format!(
            "# ADR-001. Решение\n\n- Status: Accepted\n{}\n- Модель-автор: glm-5.2\n",
            "текст\n".repeat(50)
        );
        assert!(author_model_in(&deep).is_none(), "поле найдено вне шапки");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Пишет файл в каталог и возвращает его путь.
    fn write_file(dir: &Path, name: &str, content: &str) -> PathBuf {
        let p = dir.join(name);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&p, content).unwrap();
        p
    }

    /// Фикстура: ROOT с собственным ADR и тремя «проектами»:
    /// p1 (docs/adr, один ADR без даты), p2 (model/: ADR + CMP, который
    /// обязан игнорироваться), p3 (без ADR — молча пропускается).
    fn fixture(dir: &Path) -> PathBuf {
        let root = dir.join("root");
        write_file(
            &root,
            "docs/adr/ADR-009-korn.md",
            "# ADR-009. Корневое решение\n\n- Date: 2026-01-01\n- Status: Accepted\n",
        );
        write_file(
            &root,
            "p1/docs/adr/ADR-001-outbox.md",
            "# ADR-001. Outbox pattern\n\n- Date: 2026-02-01\n- Status: Accepted\n",
        );
        write_file(
            &root,
            "p1/docs/adr/ADR-002-shina.md",
            "# ADR-002: Единая шина событий\n\n- Status: Proposed\n",
        );
        // Не ADR-файл в docs/adr — игнорируется.
        write_file(&root, "p1/docs/adr/README.md", "# О каталоге\n");
        write_file(
            &root,
            "p2/model/ADR-002-shina.md",
            "---\nid: ADR-002\ntype: adr\ntitle: Другая шина\nstatus: adopted\ndate: 2026-03-01\n---\nТело.\n",
        );
        write_file(
            &root,
            "p2/model/ADR-005-outbox.md",
            "---\nid: ADR-005\ntype: adr\ntitle: Outbox pattern\nstatus: proposed\n---\nТело.\n",
        );
        // Другой тип сущности в model/ — игнорируется.
        write_file(
            &root,
            "p2/model/CMP-001-core.md",
            "---\nid: CMP-001\ntype: cmp\ntitle: Core\nstatus: adopted\n---\nТело.\n",
        );
        // Проект без ADR.
        write_file(&root, "p3/README.md", "пустой проект\n");
        // Служебные каталоги не считаются проектами.
        write_file(
            &root,
            ".git/docs/adr/ADR-099-sleep.md",
            "# ADR-099. Спящий\n\n- Date: 2020-01-01\n- Status: Draft\n",
        );
        root
    }

    #[test]
    fn registry_collects_index_and_findings() {
        let dir = tempfile::tempdir().unwrap();
        let root = fixture(dir.path());
        let report = build_registry(&root).unwrap();

        // Индекс: root (1) + p1 (2) + p2 (2) = 5 записей; p3 и .git — мимо.
        assert_eq!(report.entries.len(), 5, "{:?}", report.entries);
        assert!(
            report.entries.iter().all(|e| e.project != "p3"),
            "проект без ADR молча пропускается"
        );
        assert!(
            report.entries.iter().all(|e| e.number != 99),
            ".git не сканируется"
        );
        // Детерминированная сортировка: проект, затем номер.
        let order: Vec<(String, u64)> = report
            .entries
            .iter()
            .map(|e| (e.project.clone(), e.number))
            .collect();
        let mut sorted = order.clone();
        sorted.sort();
        assert_eq!(order, sorted, "{order:?}");
        // Источники распознаны.
        let p2 = |n: u64| {
            report
                .entries
                .iter()
                .find(|e| e.project == "p2" && e.number == n)
                .unwrap()
        };
        assert_eq!(p2(2).source, "model");
        assert_eq!(p2(2).status.as_deref(), Some("adopted"));
        assert_eq!(p2(2).date.as_deref(), Some("2026-03-01"));
        assert_eq!(p2(5).date, None, "дата опциональна в модели");
        let prose = report
            .entries
            .iter()
            .find(|e| e.project == "p1" && e.number == 2)
            .unwrap();
        assert_eq!(prose.source, "docs/adr");
        assert_eq!(prose.title, "Единая шина событий");
        assert_eq!(prose.status.as_deref(), Some("Proposed"));
        assert_eq!(prose.date, None, "в прозе даты нет");

        // Находки: коллизия номеров (ADR-002: p1 ≠ p2), дубль заголовка
        // (Outbox pattern: ADR-001/p1 и ADR-005/p2), пропуски полей.
        let kinds = |k: &str| {
            report
                .findings
                .iter()
                .filter(|f| f.kind == k)
                .collect::<Vec<_>>()
        };
        let collisions = kinds("number_collision");
        assert_eq!(collisions.len(), 1, "{collisions:?}");
        assert!(collisions[0].message.contains("ADR-002"), "{collisions:?}");
        let dupes = kinds("title_duplicate");
        assert_eq!(dupes.len(), 1, "{dupes:?}");
        assert!(dupes[0].message.contains("Outbox pattern"), "{dupes:?}");
        let missing = kinds("missing_fields");
        assert!(
            missing.iter().any(|f| f.message.contains("p1 ADR-002")),
            "{missing:?}"
        );
        assert!(
            missing.iter().any(|f| f.message.contains("p2 ADR-005")),
            "{missing:?}"
        );
    }

    #[test]
    fn registry_detects_within_project_duplicate_numbers() {
        // Фикстура из отчёта полигона (E1): в одном docs/adr два дубля —
        // ADR-001 с другим заголовком и ADR-002 с идентичным (копия файла).
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root");
        let hdr = |n: u64, title: &str| {
            format!("# ADR-{n:03}. {title}\n\n- Date: 2026-09-19\n- Status: Accepted\n")
        };
        write_file(
            &root,
            "docs/adr/ADR-001-payment-state-machine.md",
            &hdr(1, "Платёж как конечный автомат"),
        );
        write_file(
            &root,
            "docs/adr/ADR-001-duplicate-number.md",
            &hdr(1, "Дубликат номера для проверки реестра"),
        );
        write_file(
            &root,
            "docs/adr/ADR-002-inbox.md",
            &hdr(2, "In-memory inbox"),
        );
        write_file(
            &root,
            "docs/adr/ADR-002-same-title-copy.md",
            &hdr(2, "In-memory inbox"),
        );
        let report = build_registry(&root).unwrap();
        assert_eq!(report.entries.len(), 4, "{:?}", report.entries);
        let collisions: Vec<_> = report
            .findings
            .iter()
            .filter(|f| f.kind == "number_collision")
            .collect();
        // Ровно две находки — по одной на дублированный номер, с именами файлов.
        assert_eq!(collisions.len(), 2, "{:?}", report.findings);
        assert!(
            collisions
                .iter()
                .any(|f| f.message.contains("ADR-001-duplicate-number.md")
                    && f.message.contains("ADR-001-payment-state-machine.md")),
            "{collisions:?}"
        );
        assert!(
            collisions
                .iter()
                .any(|f| f.message.contains("ADR-002-same-title-copy.md")
                    && f.message.contains("ADR-002-inbox.md")),
            "{collisions:?}"
        );
        // Копия с тем же заголовком — тоже коллизия номера, а не тишина;
        // дубль-заголовок при этом не дублирует находку (номер тот же).
        assert!(
            report.findings.iter().all(|f| f.kind != "title_duplicate"),
            "{:?}",
            report.findings
        );
        // strict: любая находка → гейт падает.
        assert_eq!(exit_code(&report, true), 1);
    }

    /// Фикстура «две грани одного решения»: прозаический ADR-001 в
    /// `docs/adr/` и типизированный ADR-001 в `model/` одного проекта.
    /// `prose` — шапка прозы, `front` — строки frontmatter модели
    /// (`title`/`status`/`date`); возвращает корень реестра.
    fn faces_fixture(dir: &Path, prose: &str, front: &str) -> PathBuf {
        let root = dir.join("root");
        write_file(&root, "docs/adr/ADR-001-x.md", prose);
        write_file(
            &root,
            "model/ADR-001-x.md",
            &format!("---\nid: ADR-001\ntype: adr\n{front}\n---\nТело.\n"),
        );
        root
    }

    /// Идентичная пара проза+модель — ОДНА запись с двумя гранями и без
    /// находок: это два представления одного решения, а не коллизия.
    #[test]
    fn registry_merges_identical_faces_without_finding() {
        let dir = tempfile::tempdir().unwrap();
        let root = faces_fixture(
            dir.path(),
            "# ADR-001. Outbox pattern\n\n- Date: 2026-02-01\n- Status: Accepted\n",
            "title: Outbox pattern\nstatus: Accepted\ndate: 2026-02-01",
        );
        let report = build_registry(&root).unwrap();
        assert_eq!(report.entries.len(), 1, "{:?}", report.entries);
        let e = &report.entries[0];
        assert_eq!(e.source, "docs/adr+model");
        assert_eq!(e.title, "Outbox pattern");
        let prose = e.prose.as_ref().expect("прозаическая грань");
        let model = e.model.as_ref().expect("типизированная грань");
        assert_eq!(prose.file, "docs/adr/ADR-001-x.md");
        assert_eq!(model.file, "model/ADR-001-x.md");
        assert!(report.findings.is_empty(), "{:?}", report.findings);
        assert_eq!(exit_code(&report, true), 0, "идентичная пара — не гейт");
    }

    /// Скобочная оговорка к статусу — пояснение, а не другой статус:
    /// «Proposed (A3 …)» в прозе и «Proposed» в модели не расходятся
    /// (кейс digital-ruble, ADR-009).
    #[test]
    fn registry_status_note_in_parentheses_is_not_divergence() {
        let dir = tempfile::tempdir().unwrap();
        let root = faces_fixture(
            dir.path(),
            "# ADR-001. Решение\n\n- Date: 2026-09-19\n- Status: Proposed (A3 — решение архитектора)\n",
            "title: Решение\nstatus: Proposed\ndate: 2026-09-19",
        );
        let report = build_registry(&root).unwrap();
        assert_eq!(report.entries.len(), 1, "{:?}", report.entries);
        assert!(report.findings.is_empty(), "{:?}", report.findings);
    }

    /// Расхождение заголовка — ровно одна находка `prose_model_divergence`
    /// (не `number_collision`), в сообщении оба значения и оба файла.
    #[test]
    fn registry_reports_title_divergence_once() {
        let dir = tempfile::tempdir().unwrap();
        let root = faces_fixture(
            dir.path(),
            "# ADR-001. Журнал — единственный источник аудита\n\n- Date: 2026-09-19\n- Status: Accepted\n",
            "title: Журнал — источник аудита\nstatus: Accepted\ndate: 2026-09-19",
        );
        let report = build_registry(&root).unwrap();
        assert_eq!(report.entries.len(), 1, "{:?}", report.entries);
        let div: Vec<_> = report
            .findings
            .iter()
            .filter(|f| f.kind == "prose_model_divergence")
            .collect();
        assert_eq!(div.len(), 1, "{:?}", report.findings);
        assert!(div[0].message.contains("заголовок"), "{div:?}");
        assert!(
            div[0].message.contains("единственный источник аудита")
                && div[0].message.contains("Журнал — источник аудита"),
            "{div:?}"
        );
        assert!(
            div[0].message.contains("docs/adr/ADR-001-x.md")
                && div[0].message.contains("model/ADR-001-x.md"),
            "{div:?}"
        );
        assert!(
            report.findings.iter().all(|f| f.kind != "number_collision"),
            "слитая пара — не коллизия: {:?}",
            report.findings
        );
    }

    /// Расхождение статуса — ровно одна находка, оба значения в сообщении.
    #[test]
    fn registry_reports_status_divergence_once() {
        let dir = tempfile::tempdir().unwrap();
        let root = faces_fixture(
            dir.path(),
            "# ADR-001. Решение\n\n- Date: 2026-09-19\n- Status: Accepted\n",
            "title: Решение\nstatus: proposed\ndate: 2026-09-19",
        );
        let report = build_registry(&root).unwrap();
        assert_eq!(report.entries.len(), 1, "{:?}", report.entries);
        let div: Vec<_> = report
            .findings
            .iter()
            .filter(|f| f.kind == "prose_model_divergence")
            .collect();
        assert_eq!(div.len(), 1, "{:?}", report.findings);
        assert!(div[0].message.contains("статус"), "{div:?}");
        assert!(
            div[0].message.contains("Accepted") && div[0].message.contains("proposed"),
            "{div:?}"
        );
    }

    /// Расхождение даты — ровно одна находка, оба значения в сообщении.
    #[test]
    fn registry_reports_date_divergence_once() {
        let dir = tempfile::tempdir().unwrap();
        let root = faces_fixture(
            dir.path(),
            "# ADR-001. Решение\n\n- Date: 2026-01-01\n- Status: Accepted\n",
            "title: Решение\nstatus: Accepted\ndate: 2026-02-02",
        );
        let report = build_registry(&root).unwrap();
        assert_eq!(report.entries.len(), 1, "{:?}", report.entries);
        let div: Vec<_> = report
            .findings
            .iter()
            .filter(|f| f.kind == "prose_model_divergence")
            .collect();
        assert_eq!(div.len(), 1, "{:?}", report.findings);
        assert!(div[0].message.contains("дата"), "{div:?}");
        assert!(
            div[0].message.contains("2026-01-01") && div[0].message.contains("2026-02-02"),
            "{div:?}"
        );
    }

    /// Заголовок, статус и дата разошлись одновременно — всё равно РОВНО
    /// одна находка на пару, а не три.
    #[test]
    fn registry_one_divergence_finding_per_pair() {
        let dir = tempfile::tempdir().unwrap();
        let root = faces_fixture(
            dir.path(),
            "# ADR-001. Первое\n\n- Date: 2026-01-01\n- Status: Accepted\n",
            "title: Второе\nstatus: Proposed\ndate: 2026-02-02",
        );
        let report = build_registry(&root).unwrap();
        let div: Vec<_> = report
            .findings
            .iter()
            .filter(|f| f.kind == "prose_model_divergence")
            .collect();
        assert_eq!(div.len(), 1, "{:?}", report.findings);
        for needle in ["заголовок", "статус", "дата"] {
            assert!(div[0].message.contains(needle), "{div:?}");
        }
    }

    /// Настоящий дубль внутри ОДНОГО представления (две прозы с одним
    /// номером) остаётся `number_collision` — это не расхождение граней.
    #[test]
    fn registry_same_representation_duplicate_is_number_collision() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root");
        let hdr = "# ADR-001. Копия\n\n- Date: 2026-01-01\n- Status: Accepted\n";
        write_file(&root, "docs/adr/ADR-001-a.md", hdr);
        write_file(&root, "docs/adr/ADR-001-b.md", hdr);
        let report = build_registry(&root).unwrap();
        assert_eq!(report.entries.len(), 2, "{:?}", report.entries);
        let collisions: Vec<_> = report
            .findings
            .iter()
            .filter(|f| f.kind == "number_collision")
            .collect();
        assert_eq!(collisions.len(), 1, "{:?}", report.findings);
        assert!(
            collisions[0].message.contains("docs/adr/ADR-001-a.md")
                && collisions[0].message.contains("docs/adr/ADR-001-b.md"),
            "{collisions:?}"
        );
        assert!(
            report
                .findings
                .iter()
                .all(|f| f.kind != "prose_model_divergence"),
            "обе записи — проза, расхождения граней нет: {:?}",
            report.findings
        );
    }

    #[test]
    fn registry_clean_docs_adr_has_no_findings() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root");
        write_file(
            &root,
            "docs/adr/ADR-001-one.md",
            "# ADR-001. Первое\n\n- Date: 2026-09-19\n- Status: Accepted\n",
        );
        write_file(
            &root,
            "docs/adr/ADR-002-two.md",
            "# ADR-002. Второе\n\n- Date: 2026-09-19\n- Status: Proposed\n",
        );
        let report = build_registry(&root).unwrap();
        assert_eq!(report.entries.len(), 2, "{:?}", report.entries);
        assert!(report.findings.is_empty(), "{:?}", report.findings);
        assert_eq!(exit_code(&report, true), 0);
    }

    #[test]
    fn registry_same_title_same_number_across_projects_is_not_collision() {
        // Одинаковый номер + одинаковый заголовок в разных проектах — это
        // скопированное решение, а не коллизия пространства номеров.
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root");
        write_file(
            &root,
            "a/docs/adr/ADR-001-x.md",
            "# ADR-001. Одно и то же\n\n- Date: 2026-01-01\n- Status: Accepted\n",
        );
        write_file(
            &root,
            "b/docs/adr/ADR-001-x.md",
            "# ADR-001. Одно и то же\n\n- Date: 2026-01-02\n- Status: Accepted\n",
        );
        let report = build_registry(&root).unwrap();
        assert!(
            report.findings.iter().all(|f| f.kind != "number_collision"),
            "{:?}",
            report.findings
        );
    }

    #[test]
    fn registry_strict_exit_code_and_json() {
        let dir = tempfile::tempdir().unwrap();
        let root = fixture(dir.path());
        let report = build_registry(&root).unwrap();
        // --strict: любая находка → exit 1; без --strict — отчёт, exit 0.
        assert_eq!(exit_code(&report, true), 1);
        assert_eq!(exit_code(&report, false), 0);
        // Чистый индекс: --strict не ломает.
        let clean_root = dir.path().join("clean");
        write_file(
            &clean_root,
            "docs/adr/ADR-001-ok.md",
            "# ADR-001. Чистое\n\n- Date: 2026-01-01\n- Status: Accepted\n",
        );
        let clean = build_registry(&clean_root).unwrap();
        assert!(clean.findings.is_empty(), "{:?}", clean.findings);
        assert_eq!(exit_code(&clean, true), 0);
        // --json: единый объект {entries, findings}.
        let v: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&report).unwrap()).unwrap();
        assert!(v["entries"].is_array(), "{v}");
        assert!(v["findings"].is_array(), "{v}");
        assert_eq!(v["entries"][0]["project"], "p1");
        assert!(v["entries"][0]["number"].is_number());
        assert!(
            v["findings"]
                .as_array()
                .unwrap()
                .iter()
                .any(|f| f["kind"] == "number_collision")
        );
    }

    #[test]
    fn registry_empty_root_is_clear_error() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("empty");
        std::fs::create_dir_all(root.join("p3")).unwrap();
        let err = build_registry(&root).unwrap_err();
        assert!(err.to_string().contains("нечего регистрировать"), "{err}");
        assert!(
            build_registry(&root.join("missing")).is_err(),
            "несуществующий корень — ошибка"
        );
    }

    #[test]
    fn registry_markdown_renders_table_and_findings() {
        let dir = tempfile::tempdir().unwrap();
        let root = fixture(dir.path());
        let report = build_registry(&root).unwrap();
        let md = render_markdown(&report);
        assert!(
            md.contains("| Проект | ADR | Заголовок | Статус | Дата | Источник |"),
            "{md}"
        );
        assert!(
            md.contains("| p1 | ADR-001 | Outbox pattern | Accepted | 2026-02-01 | docs/adr |"),
            "{md}"
        );
        assert!(
            md.contains("| p2 | ADR-002 | Другая шина | adopted | 2026-03-01 | model |"),
            "{md}"
        );
        assert!(md.contains("[number_collision]"), "{md}");
        assert!(md.contains("## Находки"), "{md}");
        assert!(md.contains("Итого: 5 записей"), "{md}");
    }

    /// Инструмент `adr_registry`: счётчики и markdown на фикстуре; по
    /// умолчанию — отчёт (`passed: true` при находках), strict — гейт.
    #[tokio::test]
    async fn adr_registry_tool_counts_and_strict_gate() {
        let dir = tempfile::tempdir().unwrap();
        let root = fixture(dir.path());
        let ctx = ToolContext::new(root, Arc::new(crate::config::Config::default()));
        let out = AdrRegistryTool
            .call(json!({"path": "."}), &ctx)
            .await
            .expect("вызов");
        assert!(!out.is_error, "{}", out.content);
        let v: Value = serde_json::from_str(&out.content).expect("JSON-вердикт");
        assert_eq!(v["entries"], 5, "{v}");
        assert!(v["finding_count"].as_u64().expect("findings") >= 1, "{v}");
        assert_eq!(v["passed"], true, "отчёт, не гейт: {v}");
        assert!(
            v["report_markdown"]
                .as_str()
                .expect("md")
                .contains("# Реестр ADR"),
            "{v}"
        );
        // strict: находки → passed=false (семантика exit_code --strict).
        let out = AdrRegistryTool
            .call(json!({"path": ".", "strict": true}), &ctx)
            .await
            .expect("вызов");
        let v: Value = serde_json::from_str(&out.content).expect("JSON");
        assert_eq!(v["passed"], false, "{v}");
        assert_eq!(v["strict"], true, "{v}");
        // Пустой корень без ADR — мягкая ошибка («нечего регистрировать»).
        let empty = tempfile::tempdir().unwrap();
        let ctx = ToolContext::new(
            empty.path().to_path_buf(),
            Arc::new(crate::config::Config::default()),
        );
        let out = AdrRegistryTool
            .call(json!({"path": "."}), &ctx)
            .await
            .expect("вызов");
        assert!(out.is_error, "{}", out.content);
    }
}

#[cfg(test)]
mod tests_h9 {
    //! Н9 волны C 0.3.4: шапка прозы ADR в формах, которые пишет человек.

    use super::*;

    fn write(root: &Path, rel: &str, text: &str) {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }

    /// Прогон реестра по каталогу с одним проектом и возвращает находки.
    fn findings_for(root: &Path) -> Vec<RegistryFinding> {
        let report = build_registry(root).expect("registry");
        report.findings
    }

    /// Русская форма `- **Статус**: Accepted` читается как Accepted, и
    /// расхождения с моделью НЕТ — это и была находка Н9.
    #[test]
    fn russian_status_header_is_parsed() {
        let tmp = tempfile::tempdir().expect("tmp");
        let root = tmp.path();
        write(
            root,
            "p/docs/adr/ADR-001-resh.md",
            "# ADR-001. Решение\n\n- **Дата**: 2026-09-19\n- **Статус**: Accepted\n\nТело.\n",
        );
        write(
            root,
            "p/model/ADR-001-resh.md",
            "---\nid: ADR-001\ntype: adr\ntitle: Решение\nstatus: Accepted\ndate: 2026-09-19\n---\nТело.\n",
        );
        let findings = findings_for(root);
        assert!(
            !findings.iter().any(|f| f.kind == "prose_model_divergence"),
            "русская шапка обязана читаться: {findings:?}"
        );
        assert!(
            !findings.iter().any(|f| f.kind == "prose_header_unparsed"),
            "{findings:?}"
        );
    }

    /// Секционная форма `## Статус` со значением следующей строкой.
    #[test]
    fn section_status_header_is_parsed() {
        let tmp = tempfile::tempdir().expect("tmp");
        let root = tmp.path();
        write(
            root,
            "p/docs/adr/ADR-001-resh.md",
            "# ADR-001. Решение\n\n## Статус\n\nAccepted\n\n## Контекст\n\nТело.\n",
        );
        write(
            root,
            "p/model/ADR-001-resh.md",
            "---\nid: ADR-001\ntype: adr\ntitle: Решение\nstatus: Accepted\n---\nТело.\n",
        );
        let findings = findings_for(root);
        assert!(
            !findings
                .iter()
                .any(|f| f.kind == "prose_model_divergence" || f.kind == "prose_header_unparsed"),
            "{findings:?}"
        );
    }

    /// Непонятная форма — находка «шапка не распознана», а НЕ «расхождение»:
    /// неизвестно, чему равно поле, поэтому сравнивать нечего.
    #[test]
    fn unparsed_status_is_not_reported_as_divergence() {
        let tmp = tempfile::tempdir().expect("tmp");
        let root = tmp.path();
        write(
            root,
            "p/docs/adr/ADR-001-resh.md",
            "# ADR-001. Решение\n\nСтатус = Accepted\n\nТело.\n",
        );
        write(
            root,
            "p/model/ADR-001-resh.md",
            "---\nid: ADR-001\ntype: adr\ntitle: Решение\nstatus: Accepted\n---\nТело.\n",
        );
        let findings = findings_for(root);
        let unparsed: Vec<&RegistryFinding> = findings
            .iter()
            .filter(|f| f.kind == "prose_header_unparsed")
            .collect();
        assert_eq!(unparsed.len(), 1, "{findings:?}");
        assert!(
            unparsed[0].message.contains("- **Статус**: Accepted"),
            "подсказка обязана называть ожидаемый формат: {}",
            unparsed[0].message
        );
        assert!(
            !findings.iter().any(|f| f.kind == "prose_model_divergence"),
            "нераспознанная шапка не может быть расхождением: {findings:?}"
        );
    }

    /// Настоящее расхождение по-прежнему ловится.
    #[test]
    fn real_divergence_still_reported() {
        let tmp = tempfile::tempdir().expect("tmp");
        let root = tmp.path();
        write(
            root,
            "p/docs/adr/ADR-001-resh.md",
            "# ADR-001. Решение\n\n- Status: Accepted\n\nТело.\n",
        );
        write(
            root,
            "p/model/ADR-001-resh.md",
            "---\nid: ADR-001\ntype: adr\ntitle: Решение\nstatus: Proposed\n---\nТело.\n",
        );
        let findings = findings_for(root);
        assert!(
            findings.iter().any(|f| f.kind == "prose_model_divergence"),
            "{findings:?}"
        );
    }
}
