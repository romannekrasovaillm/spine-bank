//! Досье судьи (context pack) — детерминированная сборка входа смысловой
//! рубрики (ADR-051).
//!
//! Смысловая ошибка — противоречие между двумя артефактами (`D6` — ссылка не
//! на ту сущность, `D10` — решение против инварианта, `D11` — код против
//! инварианта), поэтому судье нужен не один документ, а набор источников.
//! Модуль собирает этот набор из репозитория. Модель здесь не вызывается:
//! досье — данные, судит хост (split-judge) — ядро остаётся детерминированным
//! (AD-2).
//!
//! Три свойства, за которые модуль отвечает:
//!
//! - **Адресуемость.** Каждый источник обрамлён маркером с ролью и
//!   относительным путём, а хэш каждого источника попадает в отчёт. Правка
//!   любого источника — в том числе спайна, а не субъекта, — обесценивает
//!   отчёт ([`ContextPack::inputs`]).
//! - **Отсутствие тихого усечения.** Досье не влезает в лимит — явная ошибка
//!   с подсказкой сузить. Для `code_vs_spine` допустимо детерминированное
//!   деление файла на фрагменты по границам строк: каждый фрагмент — отдельный
//!   субъект со своим отчётом.
//! - **Секреты не уезжают судье.** Источник с секретом — досье не собирается
//!   вовсе (`pack_contains_secret`), значение секрета не печатается.
//!
//! Воспроизводимость: тот же репозиторий даёт побайтно тот же текст, а значит
//! и тот же [`ContextPack::sha256`]. Порядок источников стабилен — субъект,
//! затем ссылки по возрастанию ключа.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{HarnessError, Result};
use crate::hash::sha256_hex;
use crate::rubric::MAX_TARGET_CHARS;

/// Начало маркера источника (полная строка — `=== ИСТОЧНИК <роль>: <путь> ===`).
pub const SOURCE_BEGIN: &str = "=== ИСТОЧНИК";
/// Конец маркера источника; строка маркера целиком.
pub const SOURCE_END: &str = "=== КОНЕЦ ИСТОЧНИКА ===";
/// Спайн репозитория — источник инвариантов для досье вида `adr_vs_spine` и
/// `code_vs_spine`.
pub const SPINE_FILE: &str = "ARCHITECTURE-SPINE.md";
/// Каталог модели архитектуры внутри корня досье.
pub const MODEL_DIR: &str = "model";
/// Каталог стандартов корпоративного слоя (E10.1): солюшен-документ
/// сверяется с ними как ссылочными источниками досье. Один файл — один
/// стандарт; идентификатор берётся из шапки (`id: STD-1`) либо из имени файла.
pub const STANDARDS_DIR: &str = "docs/standards";
/// Каталог результатов детекторов внутри репозитория (E7.1). Формат открыт:
/// один JSON на детектор — `{"name": "...", "status": "pass"|"fail", ...}`;
/// остальные поля свободны (в них детектор кладёт свои находки, и судья их
/// читает). Пишет их механический контур проекта или CI, а не Spine: рубрика не
/// исполняет правила, она читает их результат.
pub const DETECTORS_DIR: &str = "reports/detectors";
/// Потолок числа операций контракта, попадающих в досье `entity_links`
/// (перечень операций — ориентация судьи, а не полный контракт).
const MAX_CONTRACT_OPS: usize = 40;
/// Сколько символов начала немашиночитаемого контракта попадает в досье:
/// прозаический контракт (`.md`) судье нужен как ориентир, а не целиком.
const MAX_CONTRACT_HEAD: usize = 4_000;
/// Минимальное число строк, оставляемых фрагменту кода: фрагмент из одной
/// строки бесполезен судье, лучше явная ошибка о слишком длинной строке.
const MIN_FRAGMENT_LINES: usize = 1;
/// Запас бюджета на разметку самого субъекта (два маркера, путь с диапазоном
/// строк и разделитель) — иначе последний фрагмент упирается в лимит целиком.
const SUBJECT_OVERHEAD: usize = 256;

/// Вид досье: какие источники и в каком составе собираются.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PackKind {
    /// ADR против инвариантов спайна (класс `D10`).
    AdrVsSpine,
    /// Карточка сущности против карточек тех, на кого она ссылается (класс `D6`).
    EntityLinks,
    /// NFR против привязанных трассировкой ADR и компонентов.
    NfrMechanism,
    /// Файл кода против инвариантов спайна (класс `D11`).
    CodeVsSpine,
    /// Солюшен-документ против стандартов корпоративного слоя (E10.1, класс
    /// «решение слоя расходится со стандартом ДКА»): субъект — документ
    /// решения, ссылки — стандарты `docs/standards/**`.
    SolutionVsStandards,
}

impl PackKind {
    /// Все виды досье в порядке объявления (реестр CLI/MCP).
    pub const ALL: [Self; 5] = [
        Self::AdrVsSpine,
        Self::EntityLinks,
        Self::NfrMechanism,
        Self::CodeVsSpine,
        Self::SolutionVsStandards,
    ];

    /// Строковое имя вида — как в YAML рубрик, CLI и отчётах.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AdrVsSpine => "adr_vs_spine",
            Self::EntityLinks => "entity_links",
            Self::NfrMechanism => "nfr_mechanism",
            Self::CodeVsSpine => "code_vs_spine",
            Self::SolutionVsStandards => "solution_vs_standards",
        }
    }

    /// Разбирает имя вида досье; неизвестное — ошибка со списком известных.
    ///
    /// # Errors
    /// Имя не совпало ни с одним видом.
    pub fn parse(raw: &str) -> Result<Self> {
        let want = raw.trim();
        Self::ALL
            .into_iter()
            .find(|k| k.as_str() == want)
            .ok_or_else(|| {
                pack_error(
                    "pack_unknown_kind",
                    format!(
                        "неизвестный вид досье '{want}'; известные: {}",
                        Self::ALL
                            .iter()
                            .map(|k| k.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                )
            })
    }

    /// Человекочитаемое ожидание субъекта — для подсказки при ошибке.
    #[must_use]
    pub fn subject_hint(self) -> &'static str {
        match self {
            Self::AdrVsSpine => "путь к ADR относительно корня (`docs/adr/ADR-051-….md`)",
            Self::EntityLinks => "идентификатор сущности модели (`CMP-001`, `INT-002`, …)",
            Self::NfrMechanism => "идентификатор NFR (`NFR-001`)",
            Self::CodeVsSpine => "путь к файлу кода относительно корня (`src/control.rs`)",
            Self::SolutionVsStandards => {
                "путь к солюшен-документу (`docs/solution/SOL-1.md`); стандарты — из `docs/standards/`"
            }
        }
    }
}

/// Роль источника в досье: субъект проверки или ссылочный артефакт, с которым
/// субъект сверяется.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputRole {
    /// Проверяемый артефакт.
    Subject,
    /// Источник, с которым сверяют субъект.
    Reference,
    /// Результат детектора: механического контура (правила, контрактные тесты,
    /// `ArchUnit`), а не суждения модели (E7.1). Детектор — свидетельство
    /// ИЗВНЕ рубрики: судья обязан согласовать с ним вердикт, и «чисто» при
    /// красном детекторе механика называет противоречием (E7.2).
    Detector,
}

impl InputRole {
    /// Строковое имя роли — для маркеров досье и полей отчёта.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Subject => "subject",
            Self::Reference => "reference",
            Self::Detector => "detector",
        }
    }

    /// Разбирает имя роли (регистр не важен) — для полей рубрики
    /// `evidence_roles`.
    ///
    /// # Errors
    /// Роль не `subject` и не `reference`.
    pub fn parse(raw: &str) -> Result<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "subject" => Ok(Self::Subject),
            "detector" => Ok(Self::Detector),
            "reference" => Ok(Self::Reference),
            other => Err(pack_error(
                "pack_unknown_role",
                format!(
                    "неизвестная роль источника '{other}'; известные: subject, reference, detector"
                ),
            )),
        }
    }
}

/// Один источник досье — как он попал в отчёт.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackInput {
    /// Относительный путь источника; для инвариантов спайна — с фрагментом
    /// (`ARCHITECTURE-SPINE.md#AD-1`).
    pub path: String,
    /// SHA-256 текста источника.
    pub sha256: String,
    /// Роль источника в досье.
    pub role: InputRole,
    /// Идентификатор источника для покрытия (`AD-1`, `CMP-002`); `None` —
    /// источник адресуется путём.
    #[serde(default)]
    pub id: Option<String>,
    /// Исход детектора: `pass` / `fail` (E7.1). Поле аддитивное и заполняется
    /// только у источников роли `detector`: по нему механика сверяет вердикт
    /// судьи с механическим контуром (E7.2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
}

impl PackInput {
    /// Ключ, которым судья называет источник в перечне `checked`.
    #[must_use]
    pub fn key(&self) -> &str {
        self.id.as_deref().unwrap_or(&self.path)
    }
}

/// Что именно изменилось в досье после оценки: поимённая причина для человека
/// (`src/control.rs — изменён после оценки`) либо `None`, если источники те же.
///
/// Сверяются путь и хэш каждого источника в обе стороны: правка источника,
/// его исчезновение и появление нового обесценивают отчёт — судья видел другой
/// вход, и записанный балл к нынешнему досье не относится (E1.3, ADR-051).
/// Сравнение по множеству (порядок источников не важен) — перестановка
/// источников без изменения содержимого не считается расхождением.
#[must_use]
pub fn changed_after_judging(recorded: &[PackInput], pack: &ContextPack) -> Option<String> {
    for input in &pack.inputs {
        match recorded.iter().find(|i| i.path == input.path) {
            None => return Some(format!("{} — источник появился после оценки", input.path)),
            Some(was) if was.sha256 != input.sha256 => {
                return Some(format!("{} — изменён после оценки", input.path));
            }
            Some(_) => {}
        }
    }
    for was in recorded {
        if !pack.inputs.iter().any(|i| i.path == was.path) {
            return Some(format!("{} — источник исчез после оценки", was.path));
        }
    }
    None
}

/// Собранное досье — вход судьи смысловой рубрики.
#[derive(Debug, Clone)]
pub struct ContextPack {
    /// Вид досье.
    pub kind: PackKind,
    /// Субъект: путь или идентификатор, как он записан в отчёте (для
    /// фрагмента кода — с диапазоном строк).
    pub subject: String,
    /// Текст досье целиком: субъект и ссылки под маркерами.
    pub text: String,
    /// SHA-256 [`Self::text`].
    pub sha256: String,
    /// Источники досье в порядке следования в тексте.
    pub inputs: Vec<PackInput>,
}

impl ContextPack {
    /// Ссылочные источники (роль `reference`) — то, что судья обязан
    /// перечислить в `checked` при высоком балле.
    #[must_use]
    pub fn references(&self) -> Vec<&PackInput> {
        self.inputs
            .iter()
            .filter(|i| i.role == InputRole::Reference)
            .collect()
    }

    /// Тексты всех источников роли: `evidence_roles` сверяют цитату только с
    /// текстами своего источника, а не со всем досье. Цитата засчитывается,
    /// если подтверждается хотя бы одним источником роли (в `entity_links`
    /// ссылок несколько, и обвинение может опираться на любую из них).
    #[must_use]
    pub fn role_texts(&self, role: InputRole) -> Vec<&str> {
        let mut out = Vec::new();
        let mut cursor = 0usize;
        while let Some(rel) = self.text.get(cursor..).and_then(|t| t.find(SOURCE_BEGIN)) {
            let open = cursor + rel;
            let Some(head_rel) = self.text.get(open..).and_then(|t| t.find('\n')) else {
                break;
            };
            let head = &self.text[open..open + head_rel];
            let body_start = open + head_rel + 1;
            let body_end = self
                .text
                .get(body_start..)
                .and_then(|t| t.find(SOURCE_END))
                .map_or(self.text.len(), |i| body_start + i);
            if head_ends_with_role(head, role) {
                out.push(self.text[body_start..body_end].trim_end_matches('\n'));
            }
            let next = body_end + SOURCE_END.len();
            if next >= self.text.len() {
                break;
            }
            cursor = next;
        }
        out
    }

    /// Источники досье с указателями: путь, роль и текст каждого (E9.1).
    /// Нужны механике цитат: цитата с указателем сверяется с НАЗВАННЫМ
    /// источником, а не со всем досье, и роль берётся из состава досье, а не из
    /// слов судьи. Источники, не найденные в тексте досье, в список не
    /// попадают — цитата на них будет названа неизвестной.
    #[must_use]
    pub fn source_texts(&self) -> Vec<SourceText<'_>> {
        let mut out = Vec::new();
        let mut cursor = 0usize;
        while let Some(rel) = self.text.get(cursor..).and_then(|t| t.find(SOURCE_BEGIN)) {
            let open = cursor + rel;
            let Some(head_rel) = self.text.get(open..).and_then(|t| t.find('\n')) else {
                break;
            };
            let head = &self.text[open..open + head_rel];
            let body_start = open + head_rel + 1;
            let body_end = self
                .text
                .get(body_start..)
                .and_then(|t| t.find(SOURCE_END))
                .map_or(self.text.len(), |i| body_start + i);
            let path = source_path_of(head);
            out.push(SourceText {
                id: self
                    .inputs
                    .iter()
                    .find(|i| i.path == path)
                    .and_then(|i| i.id.clone()),
                path,
                role: InputRole::parse(head_source_role(head).unwrap_or_default())
                    .unwrap_or(InputRole::Subject),
                text: self.text[body_start..body_end].trim_end_matches('\n'),
            });
            let next = body_end + SOURCE_END.len();
            if next >= self.text.len() {
                break;
            }
            cursor = next;
        }
        out
    }
}

/// Источник досье, каким его видит механика цитат (E9.1): путь-указатель,
/// роль из состава досье и текст источника.
#[derive(Debug, Clone)]
pub struct SourceText<'a> {
    /// Путь источника как в маркере (`src/pay.py`, `ARCHITECTURE-SPINE.md#AD-1`).
    pub path: String,
    /// Идентификатор источника для покрытия (`AD-1`), если задан.
    pub id: Option<String>,
    /// Роль источника в досье.
    pub role: InputRole,
    /// Текст источника.
    pub text: &'a str,
}

/// Роль из строки-маркера источника (`=== ИСТОЧНИК reference: … ===`).
fn head_source_role(head: &str) -> Option<&str> {
    let rest = head.strip_prefix(SOURCE_BEGIN)?.trim_start();
    rest.split(':').next().map(str::trim)
}

/// Путь из строки-маркера источника: всё после `<роль>:` без закрывающего
/// ` ===`.
fn source_path_of(head: &str) -> String {
    let rest = head.strip_prefix(SOURCE_BEGIN).unwrap_or(head).trim_start();
    rest.split_once(':')
        .map(|(_, tail)| {
            tail.trim()
                .trim_end_matches("===")
                .trim()
                .trim_end_matches('=')
                .trim()
                .to_string()
        })
        .unwrap_or_default()
}

impl ContextPack {
    /// Разбирает **замороженное** досье из текста: тот же формат маркеров, но
    /// источники не собираются из репозитория, а читаются из текста.
    ///
    /// Зачем: golden-набор смысловых рубрик (ADR-051, S4) хранит досье как
    /// артефакт — иначе прогон судьи зависел бы от состояния кейса, а калибровка
    /// судьи обязана быть воспроизводимой (ADR-004). Проверки цитат по ролям и
    /// сверка покрытия работают по такому досье так же, как по собранному.
    ///
    /// # Errors
    /// В тексте нет ни одного источника или маркеры непарные.
    pub fn from_text(kind: PackKind, subject: &str, text: &str) -> Result<Self> {
        let mut inputs = Vec::new();
        let mut cursor = 0usize;
        while let Some(rel) = text.get(cursor..).and_then(|t| t.find(SOURCE_BEGIN)) {
            let open = cursor + rel;
            let Some(head_rel) = text.get(open..).and_then(|t| t.find('\n')) else {
                return Err(marker_error(
                    subject,
                    "маркер источника без перевода строки",
                ));
            };
            let head = &text[open..open + head_rel];
            let (role, path) = parse_head(head).ok_or_else(|| {
                marker_error(subject, &format!("заголовок маркера не разобран: '{head}'"))
            })?;
            let body_start = open + head_rel + 1;
            let Some(body_rel) = text.get(body_start..).and_then(|t| t.find(SOURCE_END)) else {
                return Err(marker_error(
                    subject,
                    &format!("у источника '{path}' нет закрывающего маркера"),
                ));
            };
            let body = text[body_start..body_start + body_rel].trim_end_matches('\n');
            inputs.push(PackInput {
                path: path.clone(),
                sha256: sha256_hex(body.as_bytes()),
                role,
                id: path
                    .split_once('#')
                    .map(|(_, frag)| frag.to_string())
                    .filter(|f| !f.is_empty()),
                // Замороженное досье (golden-набор) тоже несёт исход детектора:
                // он лежит в теле источника, и без него сверка E7.2 молчала бы
                // ровно там, где досье хранится как текст.
                status: (role == InputRole::Detector)
                    .then(|| detector_status_of(body))
                    .flatten(),
            });
            cursor = body_start + body_rel + SOURCE_END.len();
            if cursor >= text.len() {
                break;
            }
        }
        if inputs.is_empty() {
            return Err(pack_error(
                "pack_text_without_sources",
                format!(
                    "текст по субъекту '{subject}' не содержит ни одного источника \
                     ('{SOURCE_BEGIN} <роль>: <путь> ===') — это не досье"
                ),
            ));
        }
        Ok(Self {
            kind,
            subject: subject.to_string(),
            sha256: sha256_hex(text.as_bytes()),
            text: text.to_string(),
            inputs,
        })
    }
}

/// Заголовок маркера разобранный: роль и путь.
fn parse_head(head: &str) -> Option<(InputRole, String)> {
    let rest = head.strip_prefix(SOURCE_BEGIN)?.trim();
    let (role, path) = rest.split_once(':')?;
    let path = path.trim().trim_end_matches('=').trim();
    if path.is_empty() {
        return None;
    }
    Some((InputRole::parse(role).ok()?, path.to_string()))
}

/// Ошибка разбора замороженного досье: маркеры непарные или битые.
fn marker_error(subject: &str, what: &str) -> HarnessError {
    pack_error("pack_text_malformed", format!("досье '{subject}': {what}"))
}

/// Проверяет, что заголовок маркера объявляет роль `role`
/// (`=== ИСТОЧНИК subject: путь ===`).
fn head_ends_with_role(head: &str, role: InputRole) -> bool {
    let rest = head.strip_prefix(SOURCE_BEGIN).unwrap_or(head);
    rest.trim_start()
        .starts_with(&format!("{}:", role.as_str()))
}

/// Ошибка сборки досье с машинным кодом находки в начале сообщения —
/// код читают вызывающий (MCP) и гейт.
fn pack_error(code: &str, message: impl std::fmt::Display) -> HarnessError {
    HarnessError::Rubric(format!("{code}: {message}"))
}

/// Собирает досье из репозитория; для `code_vs_spine` файл, не влезающий в
/// лимит, делится на фрагменты — на каждый свой [`ContextPack`].
///
/// # Errors
/// Неизвестный субъект; спайн без инвариантов; источник содержит строку
/// маркера; в источнике секрет; текст не влезает в [`MAX_TARGET_CHARS`] и
/// фрагментация невозможна или недостаточна.
pub fn build(repo: &Path, kind: PackKind, subject: &str) -> Result<Vec<ContextPack>> {
    let sources = collector(kind, repo, subject)?;
    let (subject_src, refs) = sources.split_first().ok_or_else(|| {
        pack_error(
            "pack_empty",
            "досье без источников не собирается".to_string(),
        )
    })?;
    check_no_marker(subject_src, refs)?;
    check_no_secret(std::iter::once(subject_src).chain(refs.iter()))?;
    let refs_len = sources_len(refs);
    let budget = MAX_TARGET_CHARS
        .checked_sub(refs_len)
        .and_then(|b| b.checked_sub(SUBJECT_OVERHEAD))
        .ok_or_else(|| over_limit_error(kind, subject, refs_len, MAX_TARGET_CHARS))?;
    // Субъект с диапазоном строк адресует конкретный фрагмент: дробить нечего,
    // остаётся проверить, что он сам влезает в лимит.
    let already_addressed = split_fragment(subject)?.1.is_some();
    if kind != PackKind::CodeVsSpine || already_addressed {
        let pack = assemble(kind, subject.to_string(), &sources);
        check_limit(&pack)?;
        return Ok(vec![pack]);
    }
    fragments(kind, subject, subject_src, refs, budget)
}

/// Собирает источники досье в порядке «субъект, затем ссылки» — без проверок
/// лимита и секретов (их применяет [`build`]).
fn collector(kind: PackKind, repo: &Path, subject: &str) -> Result<Vec<Source>> {
    match kind {
        PackKind::AdrVsSpine => adr_vs_spine(repo, subject),
        PackKind::EntityLinks => entity_links(repo, subject),
        PackKind::NfrMechanism => nfr_mechanism(repo, subject),
        PackKind::CodeVsSpine => code_vs_spine(repo, subject),
        PackKind::SolutionVsStandards => solution_vs_standards(repo, subject),
    }
}

/// Источник досье до сборки текста.
#[derive(Debug, Clone)]
struct Source {
    /// Путь (для ссылок спайна — с фрагментом `#AD-n`).
    path: String,
    /// Текст источника.
    text: String,
    /// Роль.
    role: InputRole,
    /// Идентификатор для покрытия.
    id: Option<String>,
    /// Исход детектора (`pass`/`fail`); у прочих ролей — `None`.
    status: Option<String>,
}

impl Source {
    /// Источник-субъект.
    fn subject(path: String, text: String) -> Self {
        Self {
            path,
            text,
            role: InputRole::Subject,
            id: None,
            status: None,
        }
    }

    /// Источник-детектор: результат механического контура (E7.1).
    fn detector(path: String, text: String, id: Option<String>, status: Option<String>) -> Self {
        Self {
            path,
            text,
            role: InputRole::Detector,
            id,
            status,
        }
    }

    /// Источник-ссылка с идентификатором для перечня `checked`.
    fn reference(path: String, text: String, id: Option<String>) -> Self {
        Self {
            path,
            text,
            role: InputRole::Reference,
            id,
            status: None,
        }
    }
}

/// Суммарная длина текстов источников в символах.
fn sources_len(sources: &[Source]) -> usize {
    sources.iter().map(|s| s.text.chars().count() + 80).sum()
}

/// Собирает текст досье из источников: маркеры роли и пути, стабильные
/// разделители, детерминированный порядок.
fn assemble(kind: PackKind, subject: String, sources: &[Source]) -> ContextPack {
    let mut text = String::new();
    let mut inputs = Vec::with_capacity(sources.len());
    for s in sources {
        let trimmed = s.text.trim_end_matches(['\n', '\r', ' ', '\t']);
        if !text.is_empty() {
            text.push('\n');
        }
        let _ = writeln!(text, "{SOURCE_BEGIN} {}: {} ===", s.role.as_str(), s.path); // игнорируется: записи в String не падают
        let _ = writeln!(text, "{trimmed}"); // игнорируется: записи в String не падают
        let _ = writeln!(text, "{SOURCE_END}"); // игнорируется: записи в String не падают
        inputs.push(PackInput {
            path: s.path.clone(),
            sha256: sha256_hex(trimmed.as_bytes()),
            role: s.role,
            id: s.id.clone(),
            status: s.status.clone(),
        });
    }
    let sha256 = sha256_hex(text.as_bytes());
    ContextPack {
        kind,
        subject,
        text,
        sha256,
        inputs,
    }
}

/// Строка маркера внутри источника — ошибка сборки, а не экранирование:
/// экранирование сделало бы текст источника неотличимым от разметки досье.
fn check_no_marker(subject: &Source, refs: &[Source]) -> Result<()> {
    for s in std::iter::once(subject).chain(refs.iter()) {
        for line in s.text.lines() {
            let t = line.trim_start();
            if t.starts_with(SOURCE_BEGIN) || t.trim_end() == SOURCE_END {
                return Err(pack_error(
                    "pack_marker_in_source",
                    format!(
                        "источник '{}' содержит строку маркера досье ('{}…') — \
                         досье с таким источником не собирается: разметка досье \
                         перестала бы отличаться от данных",
                        s.path,
                        t.chars().take(24).collect::<String>()
                    ),
                ));
            }
        }
    }
    Ok(())
}

/// Источник с секретом — досье не выдаётся; в сообщении только путь, без
/// значения (AD-3).
///
/// Проверка идёт встроенными правилами [`crate::secrets::Redactor`] без
/// значений окружения: сборка досье обязана быть одинаковой на любой машине
/// (AD-7), а env-значения делали бы её машинозависимой.
fn check_no_secret<'a>(sources: impl Iterator<Item = &'a Source>) -> Result<()> {
    let redactor = crate::secrets::Redactor::with_builtin_rules();
    for s in sources {
        if redactor.contains_secret(&s.text) {
            return Err(pack_error(
                "pack_contains_secret",
                format!(
                    "источник '{}' содержит секрет — досье судье не выдаётся; \
                     уберите секрет из артефакта (он не должен жить в репозитории)",
                    s.path
                ),
            ));
        }
    }
    Ok(())
}

/// Превышение лимита досье — явная ошибка с подсказкой, что сузить.
fn over_limit_error(kind: PackKind, subject: &str, got: usize, limit: usize) -> HarnessError {
    pack_error(
        "pack_over_limit",
        format!(
            "досье '{}' по субъекту '{subject}': ссылочная часть {got} символов при лимите \
             {limit} — судья не увидит текст целиком; сузьте ссылки (для кода помогает \
             автоматическое деление на фрагменты, но при таком объёме ссылок оно не спасает)",
            kind.as_str()
        ),
    )
}

/// Проверяет лимит собранного досье тем же правилом, что и обычный target:
/// тихого усечения нет.
fn check_limit(pack: &ContextPack) -> Result<()> {
    let len = pack.text.chars().count();
    if len > MAX_TARGET_CHARS {
        return Err(pack_error(
            "pack_over_limit",
            format!(
                "досье по субъекту '{}' длиннее лимита: {len} символов при {MAX_TARGET_CHARS}; \
                 сузьте субъект или оценивайте его по частям отдельными вызовами",
                pack.subject
            ),
        ));
    }
    Ok(())
}

/// Относительный путь к файлу от корня репозитория в слешевой форме.
///
/// Публичная: тем же правилом пользуется составляющая гейта `semantic_quality`
/// (ADR-052), когда называет субъектов, — иначе один путь печатался бы в
/// отчётах в двух разных формах и сверка путей расходилась бы.
#[must_use]
pub fn relative_path(repo: &Path, path: &Path) -> String {
    relative(repo, path)
}

/// Относительный путь к файлу от корня репозитория в слешевой форме.
fn relative(repo: &Path, path: &Path) -> String {
    path.strip_prefix(repo)
        .unwrap_or(path)
        .display()
        .to_string()
        .replace('\\', "/")
}

/// Разбирает субъект-файл на путь и необязательный диапазон строк `#a-b`
/// (так адресуется фрагмент кода: файл целиком в досье не влезает).
///
/// # Errors
/// Диапазон записан не как `#<начало>-<конец>` из положительных чисел.
fn split_fragment(subject: &str) -> Result<(&str, Option<(usize, usize)>)> {
    let Some((path, frag)) = subject.split_once('#') else {
        return Ok((subject, None));
    };
    let (a, b) = frag
        .split_once('-')
        .ok_or_else(|| fragment_subject_error(subject))?;
    let start = a
        .trim()
        .parse::<usize>()
        .map_err(|_| fragment_subject_error(subject))?;
    let end = b
        .trim()
        .parse::<usize>()
        .map_err(|_| fragment_subject_error(subject))?;
    if start == 0 || end < start {
        return Err(fragment_subject_error(subject));
    }
    Ok((path, Some((start, end))))
}

/// Ошибка записи фрагмента в субъекте досье.
fn fragment_subject_error(subject: &str) -> HarnessError {
    pack_error(
        "pack_bad_fragment",
        format!(
            "субъект '{subject}': диапазон строк записывается как 'путь#<начало>-<конец>' \
             (нумерация с 1, начало не больше конца)"
        ),
    )
}

/// Путь субъекта-файла: относительный, если он внутри репозитория; абсолютный
/// путь вне репозитория — ошибка (отчёт привязан к путям репозитория).
fn resolve_subject_path(repo: &Path, subject: &str, kind: PackKind) -> Result<PathBuf> {
    let (subject, _) = split_fragment(subject)?;
    let candidate = Path::new(subject);
    let path = if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        repo.join(candidate)
    };
    if !path.is_file() {
        return Err(pack_error(
            "pack_subject_not_found",
            format!(
                "субъект '{subject}' для досье '{}' не найден: ожидается {}",
                kind.as_str(),
                kind.subject_hint()
            ),
        ));
    }
    if path.strip_prefix(repo).is_err() {
        return Err(pack_error(
            "pack_subject_outside_repo",
            format!(
                "субъект '{subject}' лежит вне корня '{}' — досье собирается только \
                 из репозитория",
                repo.display()
            ),
        ));
    }
    Ok(path)
}

/// Инварианты и отложенные решения спайна как источники-ссылки.
fn spine_sources(repo: &Path) -> Result<Vec<Source>> {
    let spine = repo.join(SPINE_FILE);
    if !spine.is_file() {
        return Err(pack_error(
            "pack_spine_missing",
            format!(
                "спайн '{SPINE_FILE}' не найден в '{}' — сверять не с чем; \
                 досье этого вида требует инвариантов",
                repo.display()
            ),
        ));
    }
    let mut out = Vec::new();
    for inv in crate::agentsmd::parse_spine_invariants(&spine) {
        out.push(Source::reference(
            format!("{SPINE_FILE}#{}", inv.id),
            format!("{}: {}\nRule: {}", inv.id, inv.title, inv.rule),
            Some(inv.id),
        ));
    }
    for def in crate::agentsmd::parse_spine_deferred(&spine) {
        out.push(Source::reference(
            format!("{SPINE_FILE}#{}", def.id),
            format!(
                "{}: {}\nСтатус: отложено (Deferred — решение принято не будет, пока \
                 не выполнено условие возврата)\n{}",
                def.id, def.title, def.rule
            ),
            Some(def.id),
        ));
    }
    if out.is_empty() {
        return Err(pack_error(
            "pack_spine_without_invariants",
            format!(
                "в '{SPINE_FILE}' нет блоков AD-n/DEF-n — линтер спайна такое считает \
                 дефектом; досье без инвариантов не собирается"
            ),
        ));
    }
    // Ссылки по возрастанию ключа: `AD-9` до `AD-10` — числовой порядок
    // номера, а не лексикографический (иначе порядок зависел бы от разрядности).
    out.sort_by_key(ref_sort_key);
    Ok(out)
}

/// Ключ сортировки ссылок: (базовый путь, префикс идентификатора, числовой
/// номер, исходный путь). Префикс в ключе обязателен: без него `DEF-1` вставал
/// бы между `AD-2` и `AD-10` по номеру, разрывая группу инвариантов, а номер
/// читается числом — иначе `AD-10` шёл бы перед `AD-2`.
fn ref_sort_key(s: &Source) -> (String, String, u64, String) {
    let (base, frag) = s.path.split_once('#').unwrap_or((s.path.as_str(), ""));
    let (prefix, num) = frag
        .split_once('-')
        .map_or((frag.to_string(), u64::MAX), |(p, n)| {
            (p.to_string(), n.parse::<u64>().unwrap_or(u64::MAX))
        });
    (base.to_string(), prefix, num, s.path.clone())
}

/// Досье `adr_vs_spine`: ADR целиком + инварианты спайна и отложенные решения.
fn adr_vs_spine(repo: &Path, subject: &str) -> Result<Vec<Source>> {
    let path = resolve_subject_path(repo, subject, PackKind::AdrVsSpine)?;
    let text = std::fs::read_to_string(&path).map_err(|e| HarnessError::io(&path, e))?;
    let mut out = vec![Source::subject(relative(repo, &path), text)];
    out.extend(spine_sources(repo)?);
    Ok(out)
}

/// Досье `solution_vs_standards` (E10.1): солюшен-документ + стандарты
/// корпоративного слоя. Стандарты читаются из [`STANDARDS_DIR`] (рекурсивно,
/// отсортированы по пути), идентификатор — из шапки `id:` или из имени файла.
/// Нет ни одного стандарта — явная ошибка: сверять документ не с чем, а молча
/// пустое досье дало бы «нарушений нет» на ровном месте.
fn solution_vs_standards(repo: &Path, subject: &str) -> Result<Vec<Source>> {
    let path = resolve_subject_path(repo, subject, PackKind::SolutionVsStandards)?;
    let text = std::fs::read_to_string(&path).map_err(|e| HarnessError::io(&path, e))?;
    let mut out = vec![Source::subject(relative(repo, &path), text)];
    let standards = standards_sources(repo);
    if standards.is_empty() {
        return Err(pack_error(
            "pack_no_standards",
            format!(
                "в {STANDARDS_DIR} нет стандартов слоя — сверять документ не с чем;                  положите стандарты файлами (`{STANDARDS_DIR}/STD-1.md`)"
            ),
        ));
    }
    out.extend(standards);
    Ok(out)
}

/// Стандарты слоя как ссылочные источники: `docs/standards/**` (рекурсивно),
/// отсортированы по пути; идентификатор — шапка `id:` либо имя файла.
fn standards_sources(repo: &Path) -> Vec<Source> {
    let dir = repo.join(STANDARDS_DIR);
    if !dir.is_dir() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for entry in walk_files(&dir) {
        let Ok(text) = std::fs::read_to_string(&entry) else {
            continue;
        };
        let rel = relative(repo, &entry);
        let id = standards_id(&text, &entry);
        out.push(Source {
            path: rel,
            text,
            role: InputRole::Reference,
            id: Some(id),
            status: None,
        });
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

/// Идентификатор стандарта: первое поле `id:` шапки, иначе имя файла без
/// расширения. Шапка — первые строки файла (`id: STD-1`), как в model-картах.
fn standards_id(text: &str, path: &Path) -> String {
    for line in text.lines().take(12) {
        let trimmed = line.trim().trim_start_matches("- ").trim();
        if let Some(rest) = trimmed.strip_prefix("id:") {
            let value = rest.trim().trim_matches(['"', '\'', '*']).trim();
            if !value.is_empty() {
                return value.to_string();
            }
        }
    }
    path.file_stem()
        .map_or_else(|| "STD".to_string(), |s| s.to_string_lossy().into_owned())
}

/// Рекурсивный обход каталога файлами (без следования симлинкам), отсортирован.
fn walk_files(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&current) else {
            continue;
        };
        let mut paths: Vec<std::path::PathBuf> = entries.flatten().map(|e| e.path()).collect();
        paths.sort();
        for path in paths {
            if path.is_dir() {
                stack.push(path);
            } else if path.is_file() {
                out.push(path);
            }
        }
    }
    out.sort();
    out
}

/// Досье `code_vs_spine`: файл кода + инварианты спайна. Субъект с диапазоном
/// строк (`src/gate.rs#12-88`) даёт досье ровно об этом фрагменте — так
/// вызывающий адресует уже нарезанные фрагменты поимённо.
/// Разбор файла детектора (E7.1): обязателен только `status`, остальное —
/// свободные поля с находками.
#[derive(serde::Deserialize)]
struct Detector {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    status: Option<String>,
}

/// Исход детектора из тела его источника: `{"status": "fail"}`. Не JSON и нет
/// поля — `None` («неизвестно»), а не догадка.
fn detector_status_of(body: &str) -> Option<String> {
    #[derive(serde::Deserialize)]
    struct Status {
        #[serde(default)]
        status: Option<String>,
    }
    serde_json::from_str::<Status>(body).ok()?.status
}

/// Результаты детекторов как источники досье (E7.1): механический контур
/// (правила, контрактные тесты, `ArchUnit`) судья видит наравне с кодом и
/// спайном, а их хэши привязывают отчёт к этому срезу измерений — правка
/// результата обесценивает отчёт так же, как правка кода.
fn detector_sources(repo: &Path) -> Result<Vec<Source>> {
    let dir = repo.join(DETECTORS_DIR);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Ok(Vec::new());
    };
    let mut paths: Vec<std::path::PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.extension()
                .is_some_and(|x| x.eq_ignore_ascii_case("json"))
        })
        .collect();
    paths.sort();
    let mut out = Vec::new();
    for path in paths {
        let text = std::fs::read_to_string(&path).map_err(|e| HarnessError::io(&path, e))?;
        let parsed: Detector = serde_json::from_str(&text).map_err(|e| {
            pack_error(
                "pack_detector_malformed",
                format!(
                    "детектор {}: не разбирается как JSON ({e})",
                    relative(repo, &path)
                ),
            )
        })?;
        let id = parsed
            .name
            .clone()
            .or_else(|| path.file_stem().map(|s| s.to_string_lossy().into_owned()));
        out.push(Source::detector(
            relative(repo, &path),
            text.trim_end().to_string(),
            id,
            parsed.status.clone(),
        ));
    }
    Ok(out)
}

fn code_vs_spine(repo: &Path, subject: &str) -> Result<Vec<Source>> {
    let (_, range) = split_fragment(subject)?;
    let path = resolve_subject_path(repo, subject, PackKind::CodeVsSpine)?;
    let text = std::fs::read_to_string(&path).map_err(|e| HarnessError::io(&path, e))?;
    let slice = match range {
        None => text,
        Some((start, end)) => {
            let lines = text.lines().count();
            if end > lines {
                return Err(pack_error(
                    "pack_fragment_out_of_range",
                    format!(
                        "субъект '{subject}': конец диапазона {end} за пределами файла \
                         (строк {lines})"
                    ),
                ));
            }
            slice_lines(&text, start, end)
        }
    };
    let mut out = vec![Source::subject(relative(repo, &path), slice)];
    // E7.1: результаты механического контура — отдельная роль источника.
    out.extend(detector_sources(repo)?);
    out.extend(spine_sources(repo)?);
    Ok(out)
}

/// Модель архитектуры репозитория (каталог `model/`).
fn load_repo_model(repo: &Path) -> Result<crate::model::Model> {
    let dir = repo.join(MODEL_DIR);
    if !dir.is_dir() {
        return Err(pack_error(
            "pack_model_missing",
            format!(
                "каталог модели '{}' не найден в '{}' — сущности, на которые ссылается \
                 субъект, взять негде",
                MODEL_DIR,
                repo.display()
            ),
        ));
    }
    crate::model::load_model_tolerant(&dir)
}

/// Карточка сущности как источник досье.
///
/// Абсолютный путь в строке `Файл:` заменяется относительным: иначе хэш досье
/// зависел бы от места чекаута, и отчёт, снятый в одном клоне, становился бы
/// «устаревшим» в другом на том же содержимом — а гейт сравнивает именно хэш
/// (ADR-051, П3).
fn card_source(
    model: &crate::model::Model,
    repo: &Path,
    e: &crate::model::Entity,
    role: InputRole,
) -> Source {
    let rel = relative(repo, &e.file);
    let card = crate::model::card(model, e).replace(&e.file.display().to_string(), &rel);
    Source {
        path: rel,
        text: card,
        role,
        id: Some(e.id.clone()),
        status: None,
    }
}

/// Карточка сущности по идентификатору как источник-ссылка.
fn entity_source(model: &crate::model::Model, repo: &Path, id: &str) -> Option<Source> {
    let e = model.get(id)?;
    Some(card_source(model, repo, e, InputRole::Reference))
}

/// Досье `entity_links`: карточка субъекта + карточки всех сущностей, на
/// которые она ссылается; для `INT` — ещё и очерк файла контракта.
fn entity_links(repo: &Path, subject: &str) -> Result<Vec<Source>> {
    let model = load_repo_model(repo)?;
    let id = subject.trim();
    let entity = model.get(id).ok_or_else(|| {
        pack_error(
            "pack_subject_not_found",
            format!(
                "сущность '{id}' не найдена в каталоге '{MODEL_DIR}'; ожидается {}",
                PackKind::EntityLinks.subject_hint()
            ),
        )
    })?;
    let mut out = vec![card_source(&model, repo, entity, InputRole::Subject)];
    // Цели всех видов связей в детерминированном порядке; повтор не дублируется,
    // несуществующая цель пропускается (это дефект `model_validate`, а не досье).
    let mut ids: BTreeSet<String> = BTreeSet::new();
    for kind in crate::model::LinkKind::ALL {
        ids.extend(entity.link_targets(kind).iter().cloned());
    }
    for target in &ids {
        if let Some(src) = entity_source(&model, repo, target) {
            out.push(src);
        }
    }
    // Ссылки `verified_by` ведут и на правила fitness-реестра (`C-09`), а не
    // только на сущности: без карточки правила судья не может судить, относится
    // ли проверка к формулировке инварианта (ADR-051, R2 `rule_invariant_fit`).
    out.extend(rule_sources(repo, &ids));
    if let Some(contract) = &entity.contract {
        let path = repo.join(contract);
        if path.is_file() {
            let text = std::fs::read_to_string(&path).map_err(|e| HarnessError::io(&path, e))?;
            out.push(Source::reference(
                contract.clone(),
                contract_outline(&text),
                Some(contract.clone()),
            ));
        }
    }
    out.sort_by_key(|s| {
        if s.role == InputRole::Subject {
            (String::new(), String::new(), 0_u64, String::new())
        } else {
            ref_sort_key(s)
        }
    });
    Ok(out)
}

/// Карточки правил fitness-реестра для целей, названных в связях сущности
/// (`C-09`, `module-exists`, …): в модель правила не входят, поэтому берутся из
/// активного `CONSTRAINTS.yaml`. Нет реестра или нет совпадений — источников
/// нет: пустых карточек досье не выдумывает.
fn rule_sources(repo: &Path, ids: &BTreeSet<String>) -> Vec<Source> {
    let Some(path) = crate::control::resolve_constraints_path(repo, None) else {
        return Vec::new();
    };
    let Ok(rules) = crate::control::load_fitness_rules(&path) else {
        return Vec::new();
    };
    let file = relative(repo, &path);
    let mut out = Vec::new();
    for rule in &rules {
        let key = rule.id.clone().unwrap_or_else(|| rule.name.clone());
        if !ids.contains(&key) && !ids.contains(&rule.name) {
            continue;
        }
        out.push(Source::reference(
            format!("{file}#{key}"),
            rule.card(),
            Some(key),
        ));
    }
    out
}

/// Досье `nfr_mechanism`: карточка NFR + ADR и компоненты, привязанные к нему
/// трассировкой (и те, что ссылаются на NFR, и те, на что ссылается он).
fn nfr_mechanism(repo: &Path, subject: &str) -> Result<Vec<Source>> {
    let model = load_repo_model(repo)?;
    let id = subject.trim();
    let entity = model.get(id).ok_or_else(|| {
        pack_error(
            "pack_subject_not_found",
            format!(
                "сущность '{id}' не найдена в каталоге '{MODEL_DIR}'; ожидается {}",
                PackKind::NfrMechanism.subject_hint()
            ),
        )
    })?;
    if entity.kind != crate::model::EntityKind::Nfr {
        return Err(pack_error(
            "pack_subject_wrong_kind",
            format!(
                "субъект '{id}' — не NFR; досье 'nfr_mechanism' собирается только для \
                 показателей: ожидается {}",
                PackKind::NfrMechanism.subject_hint()
            ),
        ));
    }
    let mut out = vec![card_source(&model, repo, entity, InputRole::Subject)];
    let mut ids: BTreeSet<String> = BTreeSet::new();
    for kind in crate::model::LinkKind::ALL {
        ids.extend(entity.link_targets(kind).iter().cloned());
    }
    ids.extend(
        model
            .referents(&entity.id)
            .into_iter()
            .map(|(e, _)| e.id.clone()),
    );
    for target in &ids {
        if let Some(src) = entity_source(&model, repo, target) {
            out.push(src);
        }
    }
    out.sort_by_key(|s| {
        if s.role == InputRole::Subject {
            (String::new(), String::new(), 0_u64, String::new())
        } else {
            ref_sort_key(s)
        }
    });
    Ok(out)
}

/// Очерк файла контракта: шапка и перечень операций — без полного разбора
/// YAML/JSON (досье нужно ориентировать судью, а не заменить линтер
/// контрактов). Для `OpenAPI` операции живут под `paths:`, для `AsyncAPI` — под
/// `channels:`; берутся ключи первого уровня вложенности.
#[must_use]
pub fn contract_outline(text: &str) -> String {
    /// Раздел верхнего уровня: шапка (эмитится вместе с детьми) или операции.
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Section {
        Nothing,
        Header,
        Ops,
    }
    let mut out = String::new();
    let mut ops = 0usize;
    let mut section = Section::Nothing;
    for line in text.lines() {
        let indent = line.len() - line.trim_start().len();
        let t = line.trim_start();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        if indent == 0 {
            let head = t.split(':').next().unwrap_or(t).trim();
            section = match head {
                "paths" | "channels" => Section::Ops,
                "openapi" | "asyncapi" | "swagger" | "info" | "servers" | "version" | "title" => {
                    Section::Header
                }
                _ => Section::Nothing,
            };
            if section != Section::Nothing {
                let _ = writeln!(out, "{t}"); // игнорируется: записи в String не падают
            }
            continue;
        }
        if !t.contains(':') {
            continue;
        }
        match section {
            Section::Header => {
                let _ = writeln!(out, "{t}"); // игнорируется: записи в String не падают
            }
            Section::Ops if ops < MAX_CONTRACT_OPS => {
                ops += 1;
                let _ = writeln!(out, "{t}"); // игнорируется: записи в String не падают
            }
            _ => {}
        }
    }
    if ops >= MAX_CONTRACT_OPS {
        let _ = writeln!(
            out,
            "# перечень операций показан не полностью (потолок {MAX_CONTRACT_OPS}) — \
             полный контракт смотрите линтером контрактов"
        ); // игнорируется: записи в String не падают
    }
    if out.trim().is_empty() {
        // Контракт не машиночитаемый (например, прозаический `.md`): пустой
        // источник вводил бы судью в заблуждение — «контракт пуст». Отдаём
        // ограниченное начало файла с честной пометкой об усечении.
        let head: String = text.chars().take(MAX_CONTRACT_HEAD).collect();
        let _ = writeln!(
            out,
            "# контракт без машиночитаемой шапки (нет `openapi:`/`asyncapi:`/`paths:`); \
             показано начало файла, первые {MAX_CONTRACT_HEAD} символов"
        ); // игнорируется: записи в String не падают
        let _ = writeln!(out, "{}", head.trim_end()); // игнорируется: записи в String не падают
    }
    out
}

/// Фрагментация кода: файл, не влезающий в бюджет, делится по границам строк
/// на детерминированные фрагменты — каждый со своим отчётом.
fn fragments(
    kind: PackKind,
    subject: &str,
    subject_src: &Source,
    refs: &[Source],
    budget: usize,
) -> Result<Vec<ContextPack>> {
    let ranges = split_code(&subject_src.text, budget)?;
    if ranges.len() == 1 {
        let pack = assemble(
            kind,
            subject.to_string(),
            &collect_sources(subject_src, refs),
        );
        check_limit(&pack)?;
        return Ok(vec![pack]);
    }
    let mut out = Vec::with_capacity(ranges.len());
    for (start, end) in ranges {
        let slice = slice_lines(&subject_src.text, start, end);
        let frag_subject = format!("{subject}#{start}-{end}");
        let frag_src = Source::subject(subject_src.path.clone(), slice);
        let pack = assemble(kind, frag_subject, &collect_sources(&frag_src, refs));
        check_limit(&pack)?;
        out.push(pack);
    }
    Ok(out)
}

/// Собирает источники «субъект + ссылки» в один вектор (клонирование ссылок —
/// они одинаковы во всех фрагментах).
fn collect_sources(subject: &Source, refs: &[Source]) -> Vec<Source> {
    let mut out = Vec::with_capacity(refs.len() + 1);
    out.push(subject.clone());
    out.extend(refs.iter().cloned());
    out
}

/// Строки `start..=end` (1-based) как текст с завершающим переводом.
fn slice_lines(text: &str, start: usize, end: usize) -> String {
    let mut out = String::new();
    for line in text.lines().skip(start - 1).take(end + 1 - start) {
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// Границы фрагментов кода: `(первая, последняя строка)` включительно, в
/// порядке следования. Резать предпочитаем по «швам» — пустой строке или
/// началу объявления верхнего уровня; если шва не было, режем по текущей
/// строке; строка длиннее бюджета — явная ошибка.
fn split_code(text: &str, budget: usize) -> Result<Vec<(usize, usize)>> {
    let lines: Vec<&str> = text.lines().collect();
    if lines.is_empty() {
        return Ok(vec![(1, 1)]);
    }
    let total: usize = lines.iter().map(|l| l.chars().count() + 1).sum();
    if total <= budget {
        return Ok(vec![(1, lines.len())]);
    }
    let mut out = Vec::new();
    let mut start = 0usize; // индекс с нуля
    let mut acc = 0usize;
    let mut seam: Option<usize> = None;
    for (i, line) in lines.iter().enumerate() {
        let len = line.chars().count() + 1;
        if acc + len > budget {
            if i == start {
                // Одна строка длиннее бюджета фрагмента: резать негде.
                return Err(fragment_error(start + 1, i + 1));
            }
            let cut = seam.unwrap_or(i - 1);
            out.push((start + 1, cut + 1));
            start = cut + 1;
            seam = None;
            // Остаток текущего фрагмента (строки cut+1..=i) начинает новый.
            acc = lines[start..=i].iter().map(|l| l.chars().count() + 1).sum();
            if acc > budget {
                return Err(fragment_error(start + 1, i + 1));
            }
            continue;
        }
        acc += len;
        if i > start && is_seam(line) {
            seam = Some(i);
        }
    }
    if start < lines.len() {
        out.push((start + 1, lines.len()));
    }
    if out.is_empty()
        || out
            .iter()
            .any(|(s, e)| e < s || e - s + 1 < MIN_FRAGMENT_LINES)
    {
        return Err(fragment_error(1, lines.len()));
    }
    Ok(out)
}

/// Ошибка фрагментации: строка или шов не позволили нарезать файл в бюджет.
fn fragment_error(start: usize, end: usize) -> HarnessError {
    pack_error(
        "pack_fragment_impossible",
        format!(
            "файл не удаётся нарезать в лимит досье: участок строк {start}..{end} \
             не имеет шва (пустая строка или начало объявления) и содержит строку \
             длиннее бюджета; сократите файл или оценивайте его по частям вручную"
        ),
    )
}

/// Начала объявлений верхнего уровня — швы для нарезки фрагментов кода.
const SEAM_STARTS: [&str; 10] = [
    "fn ",
    "pub fn ",
    "pub(crate) fn ",
    "impl ",
    "struct ",
    "pub struct ",
    "enum ",
    "pub enum ",
    "trait ",
    "mod ",
];

/// Шов для нарезки фрагментов: пустая строка или начало объявления верхнего
/// уровня (нулевой отступ).
fn is_seam(line: &str) -> bool {
    if line.trim().is_empty() {
        return true;
    }
    if line.len() != line.trim_start().len() {
        return false;
    }
    SEAM_STARTS.iter().any(|s| line.starts_with(s))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Тестовый репозиторий: спайн с двумя инвариантами и отложенным решением,
    /// каталог модели и каталог для файлов кода.
    fn repo_with_spine() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().expect("tmp");
        let root = tmp.path();
        std::fs::create_dir_all(root.join("docs/adr")).expect("mkdir adr");
        std::fs::create_dir_all(root.join("src")).expect("mkdir src");
        std::fs::write(
            root.join(SPINE_FILE),
            "# ARCHITECTURE-SPINE\n\n\
             ## AD-2: Детерминированный слой контроля — без LLM\n\n\
             - **Binds**: контроль ↔ CI\n\
             - **Rule**: механика контроля детерминирована; LLM только в судье рубрик.\n\
             - **Статус**: [ADOPTED]\n\n\
             ## AD-10: Плагин — единица распространения знаний\n\n\
             - **Rule**: знания живут в плагинах.\n\n\
             ## Deferred (с условиями возврата)\n\n\
             ### DEF-1: Интерактивное подтверждение на R4\n\n\
             Возврат: когда TUI получит диалоговую шину.\n",
        )
        .expect("spine");
        tmp
    }

    fn small_adr(root: &Path, name: &str, body: &str) -> String {
        let rel = format!("docs/adr/{name}");
        std::fs::write(root.join(&rel), format!("# {name}\n\n{body}\n")).expect("adr");
        rel
    }

    /// E9.1: источники досье отдаются с указателями — путь, роль, текст.
    #[test]
    fn source_texts_expose_path_role_and_text() {
        let text = format!(
            "{SOURCE_BEGIN} subject: docs/adr/ADR-001.md ===\n\
             Решение: контроль слоя построен без LLM в гейте.\n\
             {SOURCE_END}\n\
             {SOURCE_BEGIN} reference: ARCHITECTURE-SPINE.md#AD-2 ===\n\
             AD-2: Детерминированный слой контроля\nRule: механика контроля без LLM.\n\
             {SOURCE_END}\n"
        );
        let pack = ContextPack::from_text(PackKind::AdrVsSpine, "docs/adr/ADR-001.md", &text)
            .expect("досье из текста");
        let sources = pack.source_texts();
        assert_eq!(sources.len(), 2);
        assert_eq!(sources[0].path, "docs/adr/ADR-001.md");
        assert_eq!(sources[0].role, InputRole::Subject);
        assert!(sources[0].text.contains("контроль слоя построен без LLM"));
        assert_eq!(sources[1].path, "ARCHITECTURE-SPINE.md#AD-2");
        assert_eq!(sources[1].role, InputRole::Reference);
        assert_eq!(sources[1].id.as_deref(), Some("AD-2"));
        assert!(sources[1].text.contains("Rule: механика контроля без LLM"));
    }

    // --- E10.1: досье «солюшен против стандартов слоя ДКА» -------------------

    /// Стандарт слоя с шапкой `id:` и без неё: идентификатор берётся из шапки,
    /// иначе — из имени файла.
    #[test]
    fn solution_pack_pairs_document_with_standards() {
        let tmp = repo_with_spine();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("docs/solution")).expect("mkdir solution");
        std::fs::create_dir_all(root.join("docs/standards/nested")).expect("mkdir standards");
        std::fs::write(
            root.join("docs/solution/SOL-1.md"),
            "# Решение: платежный шлюз\n\nОбработка синхронная, ретраев нет.\n",
        )
        .expect("документ");
        std::fs::write(
            root.join("docs/standards/STD-1.md"),
            "id: STD-1\n\n# Стандарт: асинхронные интеграции\n\nВнешние вызовы — только асинхронно.\n",
        )
        .expect("стандарт 1");
        std::fs::write(
            root.join("docs/standards/nested/retry.md"),
            "# Стандарт: ретраи\n\nРетраи обязательны для внешних вызовов.\n",
        )
        .expect("стандарт 2");

        let packs = build(
            root,
            PackKind::SolutionVsStandards,
            "docs/solution/SOL-1.md",
        )
        .expect("досье собирается");
        assert_eq!(packs.len(), 1, "документ не дробится");
        let pack = &packs[0];
        assert_eq!(pack.subject, "docs/solution/SOL-1.md");
        assert_eq!(
            pack.inputs
                .iter()
                .map(|i| i.path.clone())
                .collect::<Vec<_>>(),
            vec![
                "docs/solution/SOL-1.md",
                "docs/standards/STD-1.md",
                "docs/standards/nested/retry.md"
            ],
            "субъект, затем стандарты по пути"
        );
        assert_eq!(pack.inputs[1].role, InputRole::Reference);
        assert_eq!(pack.inputs[1].id.as_deref(), Some("STD-1"), "id из шапки");
        assert_eq!(
            pack.inputs[2].id.as_deref(),
            Some("retry"),
            "id из имени файла"
        );
        assert!(pack.text.contains("STD-1"), "текст стандарта в досье");
        assert_eq!(pack.sha256.len(), 64);
        // Источники с указателями (E9.1) видны механике цитат.
        let sources = pack.source_texts();
        assert_eq!(sources.len(), 3);
        assert_eq!(sources[1].role, InputRole::Reference);
        assert_eq!(sources[1].id.as_deref(), Some("STD-1"));
    }

    /// Нет стандартов — явная ошибка: пустое досье дало бы «нарушений нет».
    #[test]
    fn solution_pack_without_standards_is_an_explicit_error() {
        let tmp = repo_with_spine();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("docs/solution")).expect("mkdir solution");
        std::fs::write(root.join("docs/solution/SOL-1.md"), "# Решение\n").expect("документ");
        let err = build(
            root,
            PackKind::SolutionVsStandards,
            "docs/solution/SOL-1.md",
        )
        .expect_err("без стандартов досье не собирается");
        let msg = err.to_string();
        assert!(msg.contains("pack_no_standards"), "{msg}");
        assert!(msg.contains(STANDARDS_DIR), "подсказка где искать: {msg}");
    }

    /// Рубрика слоя ДКА объявляет ровно этот вид досье и требует цитаты на
    /// обе роли — иначе нарушение не подкрепить стандартом (E10.1).
    #[test]
    fn solution_rubric_demands_document_and_standard_citations() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("assets/rubrics/solution_standards.yaml");
        let rubric = crate::rubric::load(&path).expect("рубрика грузится");
        assert_eq!(
            rubric.pack.map(PackKind::as_str),
            Some("solution_vs_standards")
        );
        let blocking = rubric
            .criteria
            .iter()
            .find(|c| c.blocking)
            .expect("блокирующий критерий есть");
        assert_eq!(blocking.id, "standard_compliance");
        assert!(blocking.evidence_on.requires_low());
        assert_eq!(
            blocking
                .evidence_role_list()
                .expect("роли разбираются")
                .len(),
            2,
            "цитата на документ и на стандарт"
        );
        assert!(
            blocking.coverage.is_some(),
            "покрытие стандартов проверяется"
        );
    }

    #[test]
    fn pack_is_deterministic() {
        let tmp = repo_with_spine();
        let rel = small_adr(tmp.path(), "ADR-001-x.md", "Решение: контроль без LLM.");
        let a = build(tmp.path(), PackKind::AdrVsSpine, &rel).expect("pack");
        let b = build(tmp.path(), PackKind::AdrVsSpine, &rel).expect("pack");
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].text, b[0].text, "текст досье побайтно воспроизводим");
        assert_eq!(a[0].sha256, b[0].sha256);
        assert_eq!(
            a[0].inputs
                .iter()
                .map(|i| i.path.clone())
                .collect::<Vec<_>>(),
            vec![
                "docs/adr/ADR-001-x.md",
                "ARCHITECTURE-SPINE.md#AD-2",
                "ARCHITECTURE-SPINE.md#AD-10",
                "ARCHITECTURE-SPINE.md#DEF-1",
            ],
            "порядок: субъект, затем ссылки по возрастанию номера"
        );
    }

    #[test]
    fn pack_hash_changes_when_reference_changes() {
        let tmp = repo_with_spine();
        let rel = small_adr(tmp.path(), "ADR-001-x.md", "Решение: контроль без LLM.");
        let before = build(tmp.path(), PackKind::AdrVsSpine, &rel).expect("pack");
        // Меняется ИНВАРИАНТ, а не субъект: хэш досье обязан измениться —
        // это ответ на П3 (правка спайна обесценивает отчёт о согласованности).
        let spine = tmp.path().join(SPINE_FILE);
        let text = std::fs::read_to_string(&spine).expect("read");
        std::fs::write(&spine, text.replace("знания живут", "знания живут только")).expect("write");
        let after = build(tmp.path(), PackKind::AdrVsSpine, &rel).expect("pack");
        assert_ne!(before[0].sha256, after[0].sha256, "хэш досье изменился");
        assert_ne!(
            before[0].inputs[2].sha256, after[0].inputs[2].sha256,
            "хэш изменившегося источника виден поимённо"
        );
        assert_eq!(
            before[0].inputs[0].sha256, after[0].inputs[0].sha256,
            "хэш неизменившегося субъекта прежний"
        );
    }

    #[test]
    fn pack_rejects_marker_in_source() {
        let tmp = repo_with_spine();
        let rel = small_adr(
            tmp.path(),
            "ADR-001-x.md",
            &format!("{SOURCE_BEGIN} subject: подделка ===\nтекст"),
        );
        let err = build(tmp.path(), PackKind::AdrVsSpine, &rel).expect_err("маркер в источнике");
        assert!(
            err.to_string().contains("pack_marker_in_source"),
            "код находки в сообщении: {err}"
        );
    }

    #[test]
    fn pack_over_limit_is_explicit_error() {
        let tmp = repo_with_spine();
        let rel = small_adr(
            tmp.path(),
            "ADR-001-x.md",
            &"я".repeat(MAX_TARGET_CHARS + 10),
        );
        let err = build(tmp.path(), PackKind::AdrVsSpine, &rel).expect_err("лимит");
        let msg = err.to_string();
        assert!(msg.contains("pack_over_limit"), "{msg}");
        assert!(msg.contains("сузьте"), "подсказка что делать: {msg}");
    }

    #[test]
    fn pack_with_secret_is_refused() {
        let tmp = repo_with_spine();
        let rel = small_adr(
            tmp.path(),
            "ADR-001-x.md",
            "ключ провайдера: DEEPSEEK_API_KEY=sk-abcdef1234567890xyz",
        );
        let err = build(tmp.path(), PackKind::AdrVsSpine, &rel).expect_err("секрет");
        let msg = err.to_string();
        assert!(msg.contains("pack_contains_secret"), "{msg}");
        assert!(
            !msg.contains("sk-abcdef1234567890xyz"),
            "значение секрета не печатается: {msg}"
        );
    }

    #[test]
    fn pack_requires_spine_with_invariants() {
        let tmp = tempfile::tempdir().expect("tmp");
        std::fs::create_dir_all(tmp.path().join("docs/adr")).expect("mkdir");
        std::fs::write(tmp.path().join(SPINE_FILE), "# Спайн\n\nпрозы нет\n").expect("spine");
        let rel = small_adr(tmp.path(), "ADR-001-x.md", "текст");
        let err = build(tmp.path(), PackKind::AdrVsSpine, &rel).expect_err("нет инвариантов");
        assert!(
            err.to_string().contains("pack_spine_without_invariants"),
            "{err}"
        );
    }

    #[test]
    fn pack_unknown_kind_names_the_known_ones() {
        let err = PackKind::parse("adr_vs_model").expect_err("неизвестный вид");
        let msg = err.to_string();
        assert!(msg.contains("pack_unknown_kind"), "{msg}");
        assert!(
            PackKind::ALL.iter().all(|k| msg.contains(k.as_str())),
            "перечислены все известные виды: {msg}"
        );
    }

    #[test]
    fn role_text_separates_sources() {
        let tmp = repo_with_spine();
        let rel = small_adr(tmp.path(), "ADR-001-x.md", "уникальный текст субъекта");
        let packs = build(tmp.path(), PackKind::AdrVsSpine, &rel).expect("pack");
        let pack = &packs[0];
        let subject = pack.role_texts(InputRole::Subject);
        let subject = subject.first().copied().expect("текст субъекта");
        assert!(subject.contains("уникальный текст субъекта"), "{subject}");
        assert!(
            !subject.contains("Rule:"),
            "текст субъекта не втягивает ссылки: {subject}"
        );
    }

    #[test]
    fn entity_links_includes_cards_of_targets() {
        let tmp = tempfile::tempdir().expect("tmp");
        let root = tmp.path();
        std::fs::create_dir_all(root.join("model")).expect("mkdir model");
        std::fs::write(
            root.join(SPINE_FILE),
            "## AD-2: Контроль\n\n- **Rule**: без LLM.\n",
        )
        .expect("spine");
        std::fs::write(
            root.join("model/CMP-001-core.md"),
            "---\nid: CMP-001\ntype: cmp\ntitle: Ядро\nstatus: ADOPTED\n---\n\nядро системы\n",
        )
        .expect("cmp");
        std::fs::write(
            root.join("model/INT-002-notify.md"),
            "---\nid: INT-002\ntype: int\ntitle: Уведомления\nstatus: ADOPTED\ndepends_on: [CMP-001]\n---\n\nинтеграция\n",
        )
        .expect("int");
        let packs = build(root, PackKind::EntityLinks, "INT-002").expect("pack");
        let pack = &packs[0];
        assert_eq!(pack.inputs[0].role, InputRole::Subject);
        assert_eq!(pack.inputs[0].id.as_deref(), Some("INT-002"));
        assert!(
            pack.inputs
                .iter()
                .any(|i| i.id.as_deref() == Some("CMP-001")),
            "карточка цели ссылки в досье: {:?}",
            pack.inputs
        );
        assert!(pack.text.contains("ядро системы"), "текст карточки цели");
    }

    /// Досье по модели не зависит от места чекаута: карточка печатает путь
    /// файла, и абсолютный путь сделал бы хэш досье машинозависимым — отчёт,
    /// снятый в одном клоне, «устаревал» бы в другом на том же содержимом.
    #[test]
    fn pack_is_independent_of_checkout_path() {
        let tmp = tempfile::tempdir().expect("tmp");
        let root = tmp.path();
        std::fs::write(
            root.join(SPINE_FILE),
            "## AD-2: Контроль\n\n- **Rule**: без LLM.\n",
        )
        .expect("spine");
        std::fs::create_dir_all(root.join("model")).expect("mkdir model");
        std::fs::write(
            root.join("model/CMP-001-core.md"),
            "---\nid: CMP-001\ntype: cmp\ntitle: Ядро\nstatus: ADOPTED\n---\n\nядро\n",
        )
        .expect("cmp");

        // Второй клон: то же содержимое по другому пути.
        let other = tempfile::tempdir().expect("tmp");
        let root2 = other.path();
        for rel in [SPINE_FILE, "model/CMP-001-core.md"] {
            std::fs::create_dir_all(root2.join(rel).parent().expect("parent")).expect("mkdir");
            std::fs::copy(root.join(rel), root2.join(rel)).expect("copy");
        }
        let a = build(root, PackKind::EntityLinks, "CMP-001").expect("досье");
        let b = build(root2, PackKind::EntityLinks, "CMP-001").expect("досье");
        assert_eq!(
            a[0].text, b[0].text,
            "текст досье не зависит от корня чекаута"
        );
        assert_eq!(a[0].sha256, b[0].sha256, "и хэш тоже");
        assert!(
            !a[0].text.contains(&root.display().to_string()),
            "абсолютный путь в досье не печатается: {}",
            a[0].text
        );
    }

    /// Ссылка сущности на правило fitness-реестра приносит в досье карточку
    /// правила: без неё нечем судить, относится ли проверка к формулировке
    /// инварианта (рубрика `model_link_semantics`).
    #[test]
    fn entity_links_includes_rule_cards() {
        let tmp = tempfile::tempdir().expect("tmp");
        let root = tmp.path();
        std::fs::create_dir_all(root.join("model")).expect("mkdir model");
        std::fs::write(
            root.join(SPINE_FILE),
            "## AD-2: Контроль\n\n- **Rule**: без LLM.\n",
        )
        .expect("spine");
        std::fs::write(
            root.join("CONSTRAINTS.yaml"),
            "rules:\n  - id: C-009\n    name: journal_append_only\n    type: must_contain\n    \
             glob: \"src/**/*.rs\"\n    pattern: \"append_only\"\n    severity: error\n    \
             ad: AD-2\n",
        )
        .expect("constraints");
        std::fs::write(
            root.join("model/AD-002-append.md"),
            "---\nid: AD-002\ntype: ad\ntitle: Append-only журнал\nstatus: ADOPTED\n\
             verified_by: [C-009]\n---\n\nжурнал только дописывается\n",
        )
        .expect("ad");

        let packs = build(root, PackKind::EntityLinks, "AD-002").expect("досье");
        let pack = &packs[0];
        assert!(
            pack.inputs.iter().any(|i| i.id.as_deref() == Some("C-009")),
            "карточка правила в источниках: {:?}",
            pack.inputs
        );
        assert!(
            pack.text.contains("Шаблон (regex): append_only"),
            "карточка правила говорит, что именно проверяется: {}",
            pack.text
        );
    }

    #[test]
    fn code_vs_spine_splits_long_file_deterministically() {
        let tmp = repo_with_spine();
        let mut code = String::new();
        for i in 0..600 {
            let _ = writeln!(
                code,
                "pub fn handler_{i}(x: u32) -> u32 {{\n    x + {i}\n}}\n"
            );
        }
        std::fs::write(tmp.path().join("src/big.rs"), code).expect("code");
        let packs = build(tmp.path(), PackKind::CodeVsSpine, "src/big.rs").expect("packs");
        assert!(
            packs.len() > 1,
            "файл поделён на фрагменты: {}",
            packs.len()
        );
        for p in &packs {
            assert!(
                p.subject.starts_with("src/big.rs#"),
                "субъект фрагмента называет диапазон: {}",
                p.subject
            );
            assert!(p.text.chars().count() <= MAX_TARGET_CHARS);
            assert!(
                p.inputs.iter().any(|i| i.id.as_deref() == Some("AD-2")),
                "во фрагменте есть инварианты"
            );
        }
        let again = build(tmp.path(), PackKind::CodeVsSpine, "src/big.rs").expect("packs");
        assert_eq!(
            packs.iter().map(|p| p.subject.clone()).collect::<Vec<_>>(),
            again.iter().map(|p| p.subject.clone()).collect::<Vec<_>>(),
            "нарезка детерминирована"
        );
    }

    #[test]
    fn addressed_fragment_is_not_split_again() {
        let tmp = repo_with_spine();
        let mut code = String::new();
        for i in 0..600 {
            let _ = writeln!(code, "pub fn f_{i}() {{}}\n");
        }
        std::fs::write(tmp.path().join("src/big.rs"), code).expect("code");
        let packs = build(tmp.path(), PackKind::CodeVsSpine, "src/big.rs#10-20").expect("pack");
        assert_eq!(packs.len(), 1, "адресованный фрагмент не дробится повторно");
        assert_eq!(packs[0].subject, "src/big.rs#10-20");
        assert!(
            packs[0].text.contains("pub fn f_9()"),
            "взят диапазон строк"
        );
        let err = build(tmp.path(), PackKind::CodeVsSpine, "src/big.rs#1-99999")
            .expect_err("диапазон за файлом");
        assert!(
            err.to_string().contains("pack_fragment_out_of_range"),
            "{err}"
        );
    }

    #[test]
    fn bad_fragment_subject_is_explicit_error() {
        let tmp = repo_with_spine();
        std::fs::write(tmp.path().join("src/x.rs"), "fn a() {}\n").expect("code");
        let err =
            build(tmp.path(), PackKind::CodeVsSpine, "src/x.rs#abc").expect_err("кривой диапазон");
        assert!(err.to_string().contains("pack_bad_fragment"), "{err}");
    }

    #[test]
    fn contract_outline_lists_operations() {
        let text = "openapi: 3.0.3\ninfo:\n  title: Кошелёк\n  version: 1.0.0\npaths:\n  /wallets:\n    get:\n      summary: список\n  /wallets/{id}/top-up:\n    post:\n      summary: пополнение\ncomponents:\n  schemas:\n    Wallet:\n      type: object\n";
        let out = contract_outline(text);
        assert!(out.contains("/wallets"), "{out}");
        assert!(out.contains("/wallets/{id}/top-up"), "{out}");
        assert!(out.contains("title: Кошелёк"), "шапка в очерке: {out}");
        assert!(
            !out.contains("Wallet:"),
            "схемы не подмешиваются к перечню операций: {out}"
        );
    }

    #[test]
    fn contract_outline_falls_back_to_head_for_prose() {
        let text = "# Контракт интеграции ЦР\n\nПотребитель: платформа ЦР. Поставщик: реестр.\n";
        let out = contract_outline(text);
        assert!(
            out.contains("Потребитель: платформа ЦР"),
            "прозаический контракт не отдаётся пустым: {out}"
        );
        assert!(
            out.contains("без машиночитаемой шапки"),
            "обрезание названо честно: {out}"
        );
    }

    /// E7.1: результаты детекторов — отдельная роль источника с исходом и
    /// хэшем: правка результата обесценивает отчёт так же, как правка кода.
    #[test]
    fn detectors_are_dossier_sources_with_status() {
        let tmp = repo_with_spine();
        let repo = tmp.path();
        std::fs::create_dir_all(repo.join("src")).expect("src");
        std::fs::write(
            repo.join("src/pay.py"),
            "def charge(key):\n    return key\n",
        )
        .expect("code");
        let before = build(repo, PackKind::CodeVsSpine, "src/pay.py")
            .expect("досье")
            .into_iter()
            .next()
            .expect("досье");
        assert!(
            before.inputs.iter().all(|i| i.role != InputRole::Detector),
            "без результатов детекторов их в досье нет"
        );
        std::fs::create_dir_all(repo.join(DETECTORS_DIR)).expect("detectors");
        std::fs::write(
            repo.join(DETECTORS_DIR).join("fitness.json"),
            "{\"name\": \"fitness\", \"status\": \"fail\", \"findings\": [\"C-005: ключ не проверяется\"]}",
        )
        .expect("detector");
        let with = build(repo, PackKind::CodeVsSpine, "src/pay.py")
            .expect("досье")
            .into_iter()
            .next()
            .expect("досье");
        let detector = with
            .inputs
            .iter()
            .find(|i| i.role == InputRole::Detector)
            .expect("источник-детектор");
        assert_eq!(detector.id.as_deref(), Some("fitness"));
        assert_eq!(detector.status.as_deref(), Some("fail"));
        assert_eq!(detector.sha256.len(), 64, "хэш результата записан");
        assert!(
            with.text
                .contains("=== ИСТОЧНИК detector: reports/detectors/fitness.json ==="),
            "роль видна судье в маркере: {}",
            with.text
        );
        assert_ne!(
            before.sha256, with.sha256,
            "срез измерений привязан к отчёту"
        );
        // Правка результата детектора меняет хэш досье.
        std::fs::write(
            repo.join(DETECTORS_DIR).join("fitness.json"),
            "{\"name\": \"fitness\", \"status\": \"pass\", \"findings\": []}",
        )
        .expect("detector 2");
        let after = build(repo, PackKind::CodeVsSpine, "src/pay.py")
            .expect("досье")
            .into_iter()
            .next()
            .expect("досье");
        assert_ne!(with.sha256, after.sha256);
        // Замороженное досье тоже несёт исход: он в теле источника.
        let frozen = ContextPack::from_text(
            PackKind::CodeVsSpine,
            "src/pay.py",
            &format!(
                "{SOURCE_BEGIN} detector: reports/detectors/fitness.json ===\n\
                 {{\"status\": \"fail\"}}\n{SOURCE_END}\n"
            ),
        )
        .expect("замороженное досье");
        assert_eq!(
            frozen.inputs[0].status.as_deref(),
            Some("fail"),
            "из тела источника"
        );
    }

    /// E1.3: сверка досье после оценки называет изменившийся источник поимённо,
    /// ловит и исчезновение, и появление источника, а перестановку источников
    /// без правки содержимого расхождением не считает (порядок в тексте досье
    /// стабилен, но сравнение идёт по составу, а не по номерам строк).
    #[test]
    fn changed_after_judging_names_the_changed_source() {
        let input = |path: &str, sha: &str| PackInput {
            path: path.to_string(),
            sha256: sha.to_string(),
            role: InputRole::Reference,
            id: None,
            status: None,
        };
        let pack = ContextPack {
            kind: PackKind::CodeVsSpine,
            subject: "src/control.rs".to_string(),
            text: "текст досье".to_string(),
            sha256: "a".repeat(64),
            inputs: vec![
                input("src/control.rs", &"b".repeat(64)),
                input("ARCHITECTURE-SPINE.md#AD-1", &"c".repeat(64)),
            ],
        };
        assert_eq!(
            changed_after_judging(&pack.inputs, &pack),
            None,
            "тот же состав источников — не расхождение"
        );
        let mut edited = pack.clone();
        edited.inputs[0].sha256 = "d".repeat(64);
        let reason = changed_after_judging(&pack.inputs, &edited).expect("правка названа");
        assert!(
            reason.contains("src/control.rs") && reason.contains("изменён"),
            "{reason}"
        );
        let mut dropped = pack.clone();
        dropped.inputs.remove(1);
        let reason = changed_after_judging(&pack.inputs, &dropped).expect("исчезновение названо");
        assert!(
            reason.contains("ARCHITECTURE-SPINE.md#AD-1") && reason.contains("исчез"),
            "{reason}"
        );
        let mut added = pack.clone();
        added
            .inputs
            .push(input("model/REQ-002.md", &"e".repeat(64)));
        let reason = changed_after_judging(&pack.inputs, &added).expect("появление названо");
        assert!(
            reason.contains("model/REQ-002.md") && reason.contains("появился"),
            "{reason}"
        );
        let mut reordered = pack.clone();
        reordered.inputs.reverse();
        assert_eq!(
            changed_after_judging(&pack.inputs, &reordered),
            None,
            "перестановка источников без правки содержимого — не расхождение"
        );
    }
}
