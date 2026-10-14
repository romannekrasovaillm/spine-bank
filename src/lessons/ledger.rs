//! Реестр уроков: сущность [`Lesson`], append-only журнал событий
//! [`LedgerEvent`] в `.arch/lessons/ledger.jsonl` и детерминированный
//! жизненный цикл по счётчикам доказательств (ADR-085).
//!
//! # Модель
//!
//! Урок рождается из **вердикта** (гейт/fitness/инструмент/ревью), а не из
//! свободного текста исполнителя, и живёт как проекция журнала событий:
//! запись (`record`), применение с успехом (`hit`), применение с провалом
//! (`miss`), промоушен в правило (`promote`), вывод из реестра (`retire`).
//! Проекция (счётчики и статус) пересчитывается свёрткой, поэтому файл
//! журнала остаётся единственным источником истины, а рендер `LESSONS.md` —
//! производной.
//!
//! Счётчики:
//! - `fails` — сколько раз сигнатура наблюдалась как FAIL (по доказательствам);
//! - `hits` — сколько раз урок был в задании и сигнатура прошла (PASS);
//! - `misses` — сколько раз урок был в задании и та же сигнатура снова
//!   провалилась (`consecutive_misses` — серия подряд, сбрасывается `hit`);
//! - `tasks` — число РАЗНЫХ задач среди доказательств (ключ — `task`, а при
//!   его отсутствии `run_id`).
//!
//! Переходы статусов — **только по этим счётчикам** (AD-2: LLM не меняет
//! статус никогда, она может лишь сформулировать `statement`):
//! `observed` (первый FAIL) → `candidate` (FAIL повторился либо есть `hit`)
//! → `confirmed` (`hits ≥ confirm_hits`, `misses ≤ confirm_max_misses`,
//! задач ≥ `confirm_tasks`) → `promoted` (явное событие, L7/ADR-089)
//! → `retired` (серия `misses ≥ retire_misses`, явное событие или истёк
//! `expiry`). Статусы `promoted` и `retired` «липкие»: счётчики их не
//! откатывают, снятие — отдельным событием.
//!
//! # Конкурентная дозапись
//!
//! Журнал — append-only, но дозапись обязана быть безопасной при двух
//! процессах сразу: мутация выполняется под файловой блокировкой
//! (`ledger.jsonl.lock`, атомарное создание эксклюзивного файла), а строка
//! пишется одним `write_all` в дескриптор, открытый на дозапись. Порядок
//! «прочитать → решить → дописать» целиком внутри блокировки, поэтому
//! проекция не опирается на устаревшее чтение. Залежавшаяся блокировка
//! (старше [`LOCK_STALE_SECS`]) считается брошенной и снимается.
//!
//! # Совместимость
//!
//! Событие несёт версию схемы (`v`). Строки чужих/будущих версий не
//! интерпретируются и не затираются — они пропускаются при чтении и
//! учитываются счётчиком `malformed`, чтобы новый код не «съел» данные
//! старого.
//!
//! Ошибки уровня предметной области (урок без доказательств, неизвестный
//! id) возвращаются как [`HarnessError::Control`]: реестр — часть
//! детерминированного контура контроля, а не разбора конфигурации.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::error::{HarnessError, Result};
use crate::hash::sha256_hex;

use super::scope::Scope;

/// Каталог реестра относительно корня репозитория.
pub const LEDGER_DIR: &str = ".arch/lessons";
/// Имя журнала событий.
pub const LEDGER_FILE: &str = "ledger.jsonl";
/// Имя детерминированного рендера.
pub const LESSONS_MD: &str = "LESSONS.md";
/// Имя файла блокировки дозаписи (рядом с журналом).
pub const LOCK_FILE: &str = "ledger.jsonl.lock";
/// Текущая версия схемы события.
pub const LEDGER_VERSION: u32 = 1;
/// Префикс идентификатора урока.
pub const ID_PREFIX: &str = "LSN-";
/// Длина хвоста SHA-256 в идентификаторе.
pub const ID_HASH_CHARS: usize = 8;
/// Потолок символов выдержки вердикта в доказательстве.
pub const MAX_EXCERPT_CHARS: usize = 512;
/// Потолок символов текста правила.
pub const MAX_STATEMENT_CHARS: usize = 2000;
/// Бюджет ожидания файловой блокировки.
pub const LOCK_TIMEOUT: Duration = Duration::from_secs(10);
/// Возраст блокировки, после которого она считается брошенной.
pub const LOCK_STALE_SECS: u64 = 30;

/// Источник урока: откуда пришёл FAIL (ADR-085).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LessonSource {
    /// Вердикт приёмочного гейта.
    Gate,
    /// Fitness-правило реестра ограничений.
    Fitness,
    /// Сбой инструмента (подпись `tool:class`).
    Tool,
    /// Замечание ревью.
    Review,
}

impl LessonSource {
    /// Каноническая строка источника.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Gate => "gate",
            Self::Fitness => "fitness",
            Self::Tool => "tool",
            Self::Review => "review",
        }
    }
}

impl std::fmt::Display for LessonSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for LessonSource {
    type Err = HarnessError;

    /// # Errors
    /// Неизвестный источник — [`HarnessError::Config`].
    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "gate" => Ok(Self::Gate),
            "fitness" => Ok(Self::Fitness),
            "tool" => Ok(Self::Tool),
            "review" => Ok(Self::Review),
            other => Err(HarnessError::Config(format!(
                "реестр уроков: неизвестный источник '{other}' \
                 (допустимо: gate, fitness, tool, review)"
            ))),
        }
    }
}

/// Исход доказательства: чем закончилось применение урока.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    /// Провал по сигнатуре урока.
    Fail,
    /// Проход после применения урока.
    PassAfterApply,
}

impl Outcome {
    /// Каноническая строка исхода.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Fail => "fail",
            Self::PassAfterApply => "pass_after_apply",
        }
    }
}

impl std::fmt::Display for Outcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for Outcome {
    type Err = HarnessError;

    /// # Errors
    /// Неизвестный исход — [`HarnessError::Config`].
    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "fail" => Ok(Self::Fail),
            "pass_after_apply" | "pass" => Ok(Self::PassAfterApply),
            other => Err(HarnessError::Config(format!(
                "реестр уроков: неизвестный исход '{other}' \
                 (допустимо: fail, pass_after_apply)"
            ))),
        }
    }
}

/// Статус урока в детерминированном жизненном цикле.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum LessonStatus {
    /// Первый FAIL с сигнатурой.
    #[default]
    Observed,
    /// FAIL повторился либо урок применён и дал PASS.
    Candidate,
    /// Подтверждён счётчиками: `hits ≥ 2`, `misses = 0`, ≥ 2 разных задачи.
    Confirmed,
    /// Принят fitness-правилом (L7/ADR-089) — явное событие.
    Promoted,
    /// Выведен: серия `misses`, явное событие или истёк `expiry`.
    Retired,
}

impl LessonStatus {
    /// Все статусы в порядке жизненного цикла.
    pub const ALL: [LessonStatus; 5] = [
        LessonStatus::Observed,
        LessonStatus::Candidate,
        LessonStatus::Confirmed,
        LessonStatus::Promoted,
        LessonStatus::Retired,
    ];

    /// Каноническая строка статуса.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Observed => "observed",
            Self::Candidate => "candidate",
            Self::Confirmed => "confirmed",
            Self::Promoted => "promoted",
            Self::Retired => "retired",
        }
    }

    /// «Липкий» статус: счётчики его не откатывают (снятие — событием).
    #[must_use]
    pub fn is_sticky(self) -> bool {
        matches!(self, Self::Promoted | Self::Retired)
    }
}

impl std::fmt::Display for LessonStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for LessonStatus {
    type Err = HarnessError;

    /// # Errors
    /// Неизвестный статус — [`HarnessError::Config`].
    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "observed" => Ok(Self::Observed),
            "candidate" => Ok(Self::Candidate),
            "confirmed" => Ok(Self::Confirmed),
            "promoted" => Ok(Self::Promoted),
            "retired" => Ok(Self::Retired),
            other => Err(HarnessError::Config(format!(
                "реестр уроков: неизвестный статус '{other}' \
                 (допустимо: observed, candidate, confirmed, promoted, retired)"
            ))),
        }
    }
}

/// Пороги детерминированных переходов (секция `[lessons]` конфига).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Thresholds {
    /// Число наблюдённых FAIL, при котором урок становится `candidate`.
    pub candidate_fails: u32,
    /// `hits`, необходимые для `confirmed`.
    pub confirm_hits: u32,
    /// Потолок `misses` для `confirmed` (дефолт — ноль промахов).
    pub confirm_max_misses: u32,
    /// Число разных задач, необходимых для `confirmed`.
    pub confirm_tasks: u32,
    /// Серия `misses` подряд, при которой урок выводится в `retired`.
    pub retire_misses: u32,
}

impl Default for Thresholds {
    fn default() -> Self {
        Self {
            candidate_fails: 2,
            confirm_hits: 2,
            confirm_max_misses: 0,
            confirm_tasks: 2,
            retire_misses: 2,
        }
    }
}

impl From<&crate::config::LessonsConfig> for Thresholds {
    fn from(cfg: &crate::config::LessonsConfig) -> Self {
        Self {
            candidate_fails: cfg.candidate_fails,
            confirm_hits: cfg.confirm_hits,
            confirm_max_misses: cfg.confirm_max_misses,
            confirm_tasks: cfg.confirm_tasks,
            retire_misses: cfg.retire_misses,
        }
    }
}

/// Подпись урока: (источник, код, вид субъекта за пределами задачи).
///
/// Идентификатор `LSN-<sha8>` — производная подписи: одна и та же подпись
/// всегда даёт один и тот же id, перенумерации нет (ADR-085).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Signature {
    /// Источник FAIL.
    pub source: LessonSource,
    /// Код: `G5`, id правила реестра, класс ошибки инструмента.
    pub code: String,
    /// Вид субъекта (L6): «новый публичный символ», «новая миграция», …
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject_kind: Option<String>,
}

impl Signature {
    /// Подпись из источника и кода (без вида субъекта).
    #[must_use]
    pub fn new(source: LessonSource, code: impl Into<String>) -> Self {
        let code = code.into();
        Self {
            source,
            code: normalize_code(&code),
            subject_kind: None,
        }
    }

    /// Подпись с видом субъекта (`subject_kind`).
    #[must_use]
    pub fn with_subject(mut self, subject_kind: impl Into<String>) -> Self {
        let kind = subject_kind.into().trim().to_string();
        self.subject_kind = if kind.is_empty() { None } else { Some(kind) };
        self
    }

    /// Каноническая форма для хэширования: `источник<US>код<US>вид`.
    ///
    /// Разделитель — unit separator (`\u{1f}`): он не встречается в кодах
    /// правил и защищает от склейки (`a`+`b` ≠ `ab`), поэтому разные подписи
    /// не дают одного id.
    #[must_use]
    pub fn canonical(&self) -> String {
        format!(
            "{}\u{1f}{}\u{1f}{}",
            self.source.as_str(),
            self.code,
            self.subject_kind.as_deref().unwrap_or("")
        )
    }

    /// Стабильный id урока: `LSN-<первые 8 hex SHA-256 подписи>`.
    #[must_use]
    pub fn id(&self) -> String {
        let hex = sha256_hex(self.canonical().as_bytes());
        let head: String = hex.chars().take(ID_HASH_CHARS).collect();
        format!("{ID_PREFIX}{head}")
    }

    /// Человекочитаемая форма подписи (`gate · G5 · new_public_symbol`).
    #[must_use]
    pub fn describe(&self) -> String {
        match &self.subject_kind {
            Some(kind) => format!("{} · {} · {kind}", self.source, self.code),
            None => format!("{} · {}", self.source, self.code),
        }
    }
}

/// Нормализация кода: обрезка и сжатие внутренних пробелов (последняя строка
/// вердикта не должна превращать один код в два разных id).
fn normalize_code(code: &str) -> String {
    code.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Доказательство: один зафиксированный исход по сигнатуре урока.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Evidence {
    /// Идентификатор прогона (сессия/задача/запуск харнесса).
    pub run_id: String,
    /// Исполнитель прогона (`claude-code`, `qwen-code`, …).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness: Option<String>,
    /// Выдержка вердикта (усечена [`MAX_EXCERPT_CHARS`]).
    #[serde(default)]
    pub verdict_excerpt: String,
    /// Файл, к которому относится нарушение.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    /// Строка нарушения в файле.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    /// ISO-метка события.
    pub ts: String,
    /// Исход: провал или проход после применения урока.
    pub outcome: Outcome,
    /// Задача, в которой получен исход (ключ «разных задач»); без неё ключом
    /// служит `run_id`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
}

impl Evidence {
    /// Ключ задачи для подсчёта «≥ 2 разных задач» (задача, иначе прогон).
    #[must_use]
    pub fn task_key(&self) -> &str {
        self.task.as_deref().unwrap_or(self.run_id.as_str())
    }

    /// Локация `файл:строка` (для рендера и человека).
    #[must_use]
    pub fn location(&self) -> Option<String> {
        match (&self.file, self.line) {
            (Some(f), Some(l)) => Some(format!("{f}:{l}")),
            (Some(f), None) => Some(f.clone()),
            _ => None,
        }
    }
}

/// Происхождение записи: кто и чем её заверил.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Provenance {
    /// Кто записал: `gate`, `fitness`, `hook`, `agent`, `cli`, `mcp`.
    pub recorder: String,
    /// SHA-256 вердикта, из которого родился урок.
    pub verdict_sha256: String,
    /// Версия arch-be в момент записи.
    pub arch_be: String,
}

impl Provenance {
    /// Провенанс записи по выдержке вердикта (хэш считается от выдержки, если
    /// вызывающий не передал хэш полного вердикта).
    #[must_use]
    pub fn new(recorder: impl Into<String>, verdict_sha256: Option<String>, excerpt: &str) -> Self {
        Self {
            recorder: recorder.into(),
            verdict_sha256: verdict_sha256.unwrap_or_else(|| sha256_hex(excerpt.as_bytes())),
            arch_be: super::arch_be_version().to_string(),
        }
    }
}

/// Урок — сущность реестра (ADR-085).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lesson {
    /// `LSN-<sha8 подписи>`.
    pub id: String,
    /// Подпись (источник + код + вид субъекта).
    pub signature: Signature,
    /// Текст правила; может быть сформулирован LLM (`[inferred]`) — статус от
    /// текста НЕ зависит (AD-2).
    #[serde(default)]
    pub statement: String,
    /// Область действия.
    #[serde(default)]
    pub scope: Scope,
    /// Доказательства (дедуплицированы по `run_id`/`ts`/`outcome`).
    #[serde(default)]
    pub evidence: Vec<Evidence>,
    /// Наблюдённые FAIL по сигнатуре (производная от доказательств).
    #[serde(default)]
    pub fails: u32,
    /// Применений с PASS (производная от доказательств).
    #[serde(default)]
    pub hits: u32,
    /// Применений с FAIL по той же сигнатуре (события `miss`).
    #[serde(default)]
    pub misses: u32,
    /// Серия `misses` подряд (сбрасывается `hit`).
    #[serde(default)]
    pub consecutive_misses: u32,
    /// Файлы промахов (`miss`): локации контрпримеров для отката обобщения
    /// области (L6, ADR-088). Производное от событий `miss`; в рендер не
    /// попадает и при пустом значении не сериализуется (контракт `v1`
    /// аддитивен).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub miss_files: Vec<String>,
    /// Статус жизненного цикла.
    #[serde(default)]
    pub status: LessonStatus,
    /// Происхождение записи.
    pub provenance: Provenance,
    /// Дата истечения (`YYYY-MM-DD`); после неё урок выводится из реестра.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expiry: Option<String>,
    /// Правило реестра, в которое урок промоутирован (L7).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub promoted_rule: Option<String>,
    /// Причина вывода из реестра.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retired_reason: Option<String>,
}

impl Lesson {
    /// Новый урок из входной записи (статус — `observed`, счётчики нулевые:
    /// проекция пересчитает их из доказательств).
    #[must_use]
    pub fn from_input(input: &RecordInput) -> Self {
        let evidence = input.evidence.clamped();
        Self {
            id: input.signature.id(),
            signature: input.signature.clone(),
            statement: clamp_chars(&input.statement, MAX_STATEMENT_CHARS),
            scope: input.scope.clone(),
            evidence: vec![evidence],
            fails: 0,
            hits: 0,
            misses: 0,
            consecutive_misses: 0,
            miss_files: Vec::new(),
            status: LessonStatus::Observed,
            provenance: Provenance::new(
                input.recorder.clone(),
                input.verdict_sha256.clone(),
                &input.evidence.verdict_excerpt,
            ),
            expiry: input.expiry.clone(),
            promoted_rule: None,
            retired_reason: None,
        }
    }

    /// Число разных задач среди доказательств.
    #[must_use]
    pub fn task_count(&self) -> usize {
        self.evidence
            .iter()
            .map(Evidence::task_key)
            .collect::<std::collections::BTreeSet<_>>()
            .len()
    }

    /// Статус, выведенный ТОЛЬКО из счётчиков (AD-2). Не применяется к
    /// «липким» статусам — их снимает явное событие.
    #[must_use]
    pub fn derived_status(&self, th: &Thresholds) -> LessonStatus {
        if self.hits >= th.confirm_hits
            && self.misses <= th.confirm_max_misses
            && u32::try_from(self.task_count()).unwrap_or(u32::MAX) >= th.confirm_tasks
        {
            LessonStatus::Confirmed
        } else if self.fails >= th.candidate_fails || self.hits >= 1 {
            LessonStatus::Candidate
        } else {
            LessonStatus::Observed
        }
    }

    /// Однострочная сводка (CLI `list`, MCP-ответ).
    #[must_use]
    pub fn summary_line(&self) -> String {
        format!(
            "{id} · {sig} · {status} · hits {hits} · misses {misses} · задач {tasks} · {scope}",
            id = self.id,
            sig = self.signature.describe(),
            status = self.status,
            hits = self.hits,
            misses = self.misses,
            tasks = self.task_count(),
            scope = self.scope.describe(),
        )
    }
}

/// Входная запись урока (`record` из CLI/MCP/гейта).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordInput {
    /// Подпись урока.
    pub signature: Signature,
    /// Текст правила (может быть пустым: сформулирует человек или LLM).
    #[serde(default)]
    pub statement: String,
    /// Область действия.
    #[serde(default)]
    pub scope: Scope,
    /// Доказательство, из которого родилась запись.
    pub evidence: Evidence,
    /// Кто записал (`cli`, `mcp`, `gate`, `hook`, `agent`).
    #[serde(default = "default_recorder")]
    pub recorder: String,
    /// SHA-256 полного вердикта (если вызывающий его знает).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verdict_sha256: Option<String>,
    /// Дата истечения (`YYYY-MM-DD`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expiry: Option<String>,
}

fn default_recorder() -> String {
    "cli".to_string()
}

impl RecordInput {
    /// Запись из подписи и доказательства (рекордер — [`default_recorder`]).
    #[must_use]
    pub fn new(signature: Signature, evidence: Evidence) -> Self {
        Self {
            signature,
            statement: String::new(),
            scope: Scope::repo(),
            evidence,
            recorder: default_recorder(),
            verdict_sha256: None,
            expiry: None,
        }
    }

    /// Текст правила.
    #[must_use]
    pub fn with_statement(mut self, statement: impl Into<String>) -> Self {
        self.statement = statement.into();
        self
    }

    /// Область действия.
    #[must_use]
    pub fn with_scope(mut self, scope: Scope) -> Self {
        self.scope = scope;
        self
    }

    /// Рекордер (`cli`, `mcp`, `gate`, `hook`, `agent`).
    #[must_use]
    pub fn with_recorder(mut self, recorder: impl Into<String>) -> Self {
        self.recorder = recorder.into();
        self
    }

    /// Дата истечения урока.
    #[must_use]
    pub fn with_expiry(mut self, expiry: impl Into<String>) -> Self {
        self.expiry = Some(expiry.into());
        self
    }
}

impl Evidence {
    /// Копия доказательства с усечённой выдержкой вердикта.
    #[must_use]
    pub fn clamped(&self) -> Self {
        let mut copy = self.clone();
        copy.verdict_excerpt = clamp_chars(&self.verdict_excerpt, MAX_EXCERPT_CHARS);
        copy
    }
}

/// Усечение строки по числу символов (границы UTF-8 не рвутся).
#[must_use]
pub fn clamp_chars(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_string();
    }
    text.chars().take(limit).collect()
}

/// Событие журнала реестра: вид + служебные поля версии и времени.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LedgerEvent {
    /// Версия схемы события.
    pub v: u32,
    /// ISO-метка события.
    pub ts: String,
    /// Вид события.
    #[serde(flatten)]
    pub event: EventKind,
}

/// Вид события журнала (append-only).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EventKind {
    /// Запись урока (первая — создаёт, повторная — уточняет формулировку и
    /// добавляет доказательство).
    Record {
        /// Снимок урока (с новым доказательством).
        lesson: Lesson,
    },
    /// Урок был в задании, сигнатура прошла (PASS).
    Hit {
        /// Id урока.
        id: String,
        /// Доказательство прохода.
        evidence: Evidence,
    },
    /// Урок был в задании, та же сигнатура снова провалилась.
    Miss {
        /// Id урока.
        id: String,
        /// Доказательство провала.
        evidence: Evidence,
    },
    /// Урок принят fitness-правилом (L7/ADR-089).
    Promote {
        /// Id урока.
        id: String,
        /// Id правила реестра.
        rule: String,
        /// Дельта, которой правило внесено.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        delta: Option<String>,
    },
    /// Урок выведен из реестра.
    Retire {
        /// Id урока.
        id: String,
        /// Причина вывода.
        reason: String,
    },
}

impl LedgerEvent {
    /// Событие заданного вида с текущей меткой времени.
    #[must_use]
    pub fn now(event: EventKind) -> Self {
        Self {
            v: LEDGER_VERSION,
            ts: now_iso(),
            event,
        }
    }

    /// Id урока, к которому относится событие.
    #[must_use]
    pub fn lesson_id(&self) -> &str {
        match &self.event {
            EventKind::Record { lesson } => &lesson.id,
            EventKind::Hit { id, .. }
            | EventKind::Miss { id, .. }
            | EventKind::Promote { id, .. }
            | EventKind::Retire { id, .. } => id,
        }
    }
}

/// Проекция реестра: уроки, счётчики чтения.
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    /// Уроки по возрастанию id.
    pub lessons: Vec<Lesson>,
    /// Число прочитанных событий.
    pub events: usize,
    /// Число строк, которые не удалось интерпретировать (чужая версия, мусор).
    pub malformed: usize,
}

impl Snapshot {
    /// Урок по id.
    #[must_use]
    pub fn get(&self, id: &str) -> Option<&Lesson> {
        self.lessons.iter().find(|l| l.id == id)
    }

    /// Урок по подписи.
    #[must_use]
    pub fn find_by_signature(&self, signature: &Signature) -> Option<&Lesson> {
        let id = signature.id();
        self.get(&id)
    }

    /// Число уроков в каждом статусе (для сводок и рендера).
    #[must_use]
    pub fn count_by_status(&self) -> BTreeMap<LessonStatus, usize> {
        let mut counts: BTreeMap<LessonStatus, usize> = BTreeMap::new();
        for status in LessonStatus::ALL {
            counts.insert(status, 0);
        }
        for lesson in &self.lessons {
            *counts.entry(lesson.status).or_default() += 1;
        }
        counts
    }
}

/// Результат мутации реестра.
#[derive(Debug, Clone)]
pub struct Mutation {
    /// Урок после применения события.
    pub lesson: Lesson,
    /// Урок создан этим событием (а не уточнён).
    pub created: bool,
    /// Статус до мутации (`None` — урок создан).
    pub previous_status: Option<LessonStatus>,
    /// Записанные события (в порядке дозаписи).
    pub events: Vec<LedgerEvent>,
}

impl Mutation {
    /// Статус изменился этой мутацией.
    #[must_use]
    pub fn status_changed(&self) -> bool {
        self.previous_status != Some(self.lesson.status)
    }
}

/// Реестр уроков: путь к журналу плюс операции над ним.
///
/// Структура не держит состояния в памяти: каждое чтение поднимает проекцию
/// из журнала, каждая мутация — «блокировка → чтение → решение → дозапись».
#[derive(Debug, Clone)]
pub struct Ledger {
    path: PathBuf,
}

impl Ledger {
    /// Реестр по явному пути к журналу.
    #[must_use]
    pub fn at(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// Репозиторный реестр: `<repo>/.arch/lessons/ledger.jsonl`.
    #[must_use]
    pub fn repo(repo: &Path) -> Self {
        Self::at(repo_ledger_path(repo))
    }

    /// Пользовательский реестр: `$ARCH_HOME/lessons/ledger.jsonl`.
    #[must_use]
    pub fn user() -> Self {
        Self::at(user_ledger_path())
    }

    /// Путь к журналу событий.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Каталог реестра (там же лежат `LESSONS.md`, `ledger.jsonl.lock`).
    #[must_use]
    pub fn dir(&self) -> PathBuf {
        self.path
            .parent()
            .map_or_else(|| PathBuf::from("."), Path::to_path_buf)
    }

    /// Путь к детерминированному рендеру.
    #[must_use]
    pub fn lessons_md_path(&self) -> PathBuf {
        self.dir().join(LESSONS_MD)
    }

    /// Путь к файлу блокировки дозаписи.
    #[must_use]
    pub fn lock_path(&self) -> PathBuf {
        self.dir().join(LOCK_FILE)
    }

    /// Проекция реестра (толерантное чтение: битые строки считаются, не роняют).
    ///
    /// # Errors
    /// Ошибка чтения журнала.
    pub fn snapshot(&self, th: &Thresholds) -> Result<Snapshot> {
        let (events, malformed) = read_events(&self.path)?;
        let lessons = fold(&events, th);
        Ok(Snapshot {
            lessons,
            events: events.len(),
            malformed,
        })
    }

    /// Записывает исход по подписи.
    ///
    /// Отображение исхода в событие (AD-2 — счётчики, не текст):
    /// - подпись незнакома — `record`: урок рождается в `observed`;
    /// - знакомый урок ещё НЕ был в задании (`hits = misses = 0`) и снова
    ///   FAIL — `record`: это повторное НАБЛЮДЕНИЕ («FAIL повторился»),
    ///   переводящее урок в `candidate`, а не промах применения;
    /// - знакомый урок был в задании и снова FAIL — `miss` (промах);
    /// - PASS после применения — `hit`.
    ///
    /// Просроченные уроки выводятся (`retire` по `expiry`) этой же мутацией,
    /// поэтому реестр не «стареет» втихую.
    ///
    /// # Errors
    /// Ошибка журнала (создание каталога, блокировка, запись).
    pub fn record(&self, input: &RecordInput, th: &Thresholds, today: &str) -> Result<Mutation> {
        let id = input.signature.id();
        let mut created = false;
        let mut previous = None;
        let outcome = self.mutate(th, |lessons| {
            let mut events = expire_events(lessons, today);
            if let Some(lesson) = lessons.iter().find(|l| l.id == id) {
                previous = Some(lesson.status);
                let evidence = input.evidence.clamped();
                let in_task = lesson.hits > 0 || lesson.misses > 0;
                events.push(match (evidence.outcome, in_task) {
                    (Outcome::PassAfterApply, _) => LedgerEvent::now(EventKind::Hit {
                        id: id.clone(),
                        evidence,
                    }),
                    (Outcome::Fail, true) => LedgerEvent::now(EventKind::Miss {
                        id: id.clone(),
                        evidence,
                    }),
                    (Outcome::Fail, false) => LedgerEvent::now(EventKind::Record {
                        lesson: Lesson::from_input(input),
                    }),
                });
            } else {
                created = true;
                events.push(LedgerEvent::now(EventKind::Record {
                    lesson: Lesson::from_input(input),
                }));
            }
            Ok(events)
        })?;
        let lesson = lesson_by_id(&outcome.lessons, &id)?;
        Ok(Mutation {
            lesson,
            created,
            previous_status: previous,
            events: outcome.fresh,
        })
    }

    /// Принимает готовый урок извне (импорт бандла): дописывает событие
    /// `record` с его доказательствами.
    ///
    /// Статус из бандла **не принимается** — проекция выводит его из счётчиков
    /// (AD-2): чужой урок начинается с того, что подтвердили своим реестром.
    /// Урок без доказательств отклоняется ([`validate_imported`]).
    ///
    /// # Errors
    /// Урок без доказательств, несоответствие id и подписи либо ошибка журнала.
    pub fn adopt(&self, lesson: &Lesson, th: &Thresholds, today: &str) -> Result<Mutation> {
        validate_imported(lesson)?;
        let id = lesson.id.clone();
        let mut created = false;
        let mut previous = None;
        let outcome = self.mutate(th, |lessons| {
            let mut events = expire_events(lessons, today);
            previous = lessons.iter().find(|l| l.id == id).map(|l| l.status);
            created = previous.is_none();
            events.push(LedgerEvent::now(EventKind::Record {
                lesson: lesson.clone(),
            }));
            Ok(events)
        })?;
        Ok(Mutation {
            lesson: lesson_by_id(&outcome.lessons, &id)?,
            created,
            previous_status: previous,
            events: outcome.fresh,
        })
    }

    /// Переводит урок в `retired` явным событием.
    ///
    /// # Errors
    /// Урок не найден или ошибка журнала.
    pub fn retire(&self, id: &str, reason: &str, th: &Thresholds) -> Result<Mutation> {
        let id_owned = id.to_string();
        let mut previous = None;
        let outcome = self.mutate(th, |lessons| {
            let lesson = lessons.iter().find(|l| l.id == id_owned).ok_or_else(|| {
                HarnessError::Control(format!(
                    "реестр уроков: урок {id_owned} не найден — нечего выводить"
                ))
            })?;
            previous = Some(lesson.status);
            if lesson.status == LessonStatus::Retired {
                return Ok(Vec::new());
            }
            Ok(vec![LedgerEvent::now(EventKind::Retire {
                id: id_owned.clone(),
                reason: reason.to_string(),
            })])
        })?;
        Ok(Mutation {
            lesson: lesson_by_id(&outcome.lessons, id)?,
            created: false,
            previous_status: previous,
            events: outcome.fresh,
        })
    }

    /// Промоутирует урок в fitness-правило (L7/ADR-089).
    ///
    /// # Errors
    /// Урок не найден или ошибка журнала.
    pub fn promote(
        &self,
        id: &str,
        rule: &str,
        delta: Option<&str>,
        th: &Thresholds,
    ) -> Result<Mutation> {
        let id_owned = id.to_string();
        let mut previous = None;
        let outcome = self.mutate(th, |lessons| {
            let lesson = lessons.iter().find(|l| l.id == id_owned).ok_or_else(|| {
                HarnessError::Control(format!(
                    "реестр уроков: урок {id_owned} не найден — нечего промоутировать"
                ))
            })?;
            previous = Some(lesson.status);
            Ok(vec![LedgerEvent::now(EventKind::Promote {
                id: id_owned.clone(),
                rule: rule.to_string(),
                delta: delta.map(str::to_string),
            })])
        })?;
        Ok(Mutation {
            lesson: lesson_by_id(&outcome.lessons, id)?,
            created: false,
            previous_status: previous,
            events: outcome.fresh,
        })
    }

    /// Выводит из реестра уроки с истёкшим `expiry` относительно даты `today`
    /// (`YYYY-MM-DD`). Возвращает число выведенных уроков.
    ///
    /// # Errors
    /// Ошибка журнала.
    pub fn refresh(&self, today: &str, th: &Thresholds) -> Result<usize> {
        let outcome = self.mutate(th, |lessons| Ok(expire_events(lessons, today)))?;
        Ok(outcome.fresh.len())
    }

    /// Мутация под файловой блокировкой: чтение → решение → дозапись.
    fn mutate<F>(&self, th: &Thresholds, decide: F) -> Result<MutateOutcome>
    where
        F: FnOnce(&[Lesson]) -> Result<Vec<LedgerEvent>>,
    {
        let dir = self.dir();
        std::fs::create_dir_all(&dir).map_err(|e| HarnessError::io(&dir, e))?;
        let _guard = FileLock::acquire(&self.lock_path())?;
        let (events, _) = read_events(&self.path)?;
        let lessons = fold(&events, th);
        let fresh = decide(&lessons)?;
        if !fresh.is_empty() {
            append_events(&self.path, &fresh)?;
        }
        let mut all = events;
        all.extend(fresh.iter().cloned());
        Ok(MutateOutcome {
            lessons: fold(&all, th),
            fresh,
        })
    }
}

/// Итог мутации внутри блокировки.
struct MutateOutcome {
    /// Проекция после дозаписи.
    lessons: Vec<Lesson>,
    /// События, дописанные этой мутацией.
    fresh: Vec<LedgerEvent>,
}

/// Урок по id из проекции (урок обязан существовать после мутации).
fn lesson_by_id(lessons: &[Lesson], id: &str) -> Result<Lesson> {
    lessons.iter().find(|l| l.id == id).cloned().ok_or_else(|| {
        HarnessError::Control(format!(
            "реестр уроков: урок {id} отсутствует в проекции после записи"
        ))
    })
}

/// Путь к репозиторному журналу.
#[must_use]
pub fn repo_ledger_path(repo: &Path) -> PathBuf {
    repo.join(LEDGER_DIR).join(LEDGER_FILE)
}

/// Каталог пользовательского реестра (`$ARCH_HOME/lessons`).
#[must_use]
pub fn user_dir() -> PathBuf {
    crate::config::Config::home_dir().join("lessons")
}

/// Путь к пользовательскому журналу.
#[must_use]
pub fn user_ledger_path() -> PathBuf {
    user_dir().join(LEDGER_FILE)
}

/// Проверка урока, пришедшего извне (импорт бандла, L10).
///
/// Требования ADR-085/TASK §7.6: урок без доказательств не принимается (иначе
/// чужой реестр становится каналом инъекции текста без проверки гейтом), а id
/// обязан соответствовать подписи — иначе в реестр попадёт урок с чужой
/// «личностью» и счётчики склеятся с чужими.
///
/// # Errors
/// [`HarnessError::Control`] с причиной отказа.
pub fn validate_imported(lesson: &Lesson) -> Result<()> {
    if lesson.evidence.is_empty() {
        return Err(HarnessError::Control(format!(
            "реестр уроков: урок {} без evidence — импорт отклонён \
             (нужно хотя бы одно доказательство с run_id, ts и исходом)",
            lesson.id
        )));
    }
    let expected = lesson.signature.id();
    if lesson.id != expected {
        return Err(HarnessError::Control(format!(
            "реестр уроков: id {} не соответствует сигнатуре (ожидался {expected})",
            lesson.id
        )));
    }
    Ok(())
}

/// События вывода просроченных уроков (`retire` по `expiry`).
fn expire_events(lessons: &[Lesson], today: &str) -> Vec<LedgerEvent> {
    lessons
        .iter()
        .filter(|l| {
            !l.status.is_sticky()
                && l.expiry
                    .as_deref()
                    .is_some_and(|expiry| !expiry.is_empty() && today > expiry)
        })
        .map(|l| {
            LedgerEvent::now(EventKind::Retire {
                id: l.id.clone(),
                reason: format!("истёк expiry: {}", l.expiry.as_deref().unwrap_or_default()),
            })
        })
        .collect()
}

/// Свёртка журнала в проекцию (уроки по возрастанию id).
///
/// Счётчики `fails`/`hits` — производные от доказательств; `misses` и серия
/// промахов — от событий `miss`/`hit`. Статусы `promoted`/`retired` липкие.
#[must_use]
pub fn fold(events: &[LedgerEvent], th: &Thresholds) -> Vec<Lesson> {
    let mut index: BTreeMap<String, Lesson> = BTreeMap::new();
    for event in events {
        match &event.event {
            EventKind::Record { lesson } => {
                let entry = index.entry(lesson.id.clone()).or_insert_with(|| {
                    let mut fresh = lesson.clone();
                    fresh.misses = 0;
                    fresh.consecutive_misses = 0;
                    // Локации промахов — производное только от событий `miss`;
                    // снимок записи (и импортированного бандла) их не задаёт.
                    fresh.miss_files.clear();
                    fresh.promoted_rule = None;
                    fresh.retired_reason = None;
                    fresh.status = LessonStatus::Observed;
                    fresh
                });
                // Повторная запись добавляет доказательства и уточняет
                // метаданные: пустое значение НЕ затирает уже известное
                // (повторный FAIL — доказательство, а не новая область
                // действия или сброс формулировки). Статус из снимка не
                // принимается вовсе — его выводит только счётчик.
                if !lesson.statement.is_empty() {
                    entry.statement.clone_from(&lesson.statement);
                }
                if lesson.scope.is_specified() {
                    entry.scope.clone_from(&lesson.scope);
                }
                entry.provenance.clone_from(&lesson.provenance);
                if lesson.expiry.is_some() {
                    entry.expiry.clone_from(&lesson.expiry);
                }
                for evidence in &lesson.evidence {
                    push_unique(&mut entry.evidence, evidence.clamped());
                }
            }
            EventKind::Hit { id, evidence } => {
                if let Some(lesson) = index.get_mut(id) {
                    push_unique(&mut lesson.evidence, evidence.clamped());
                    lesson.consecutive_misses = 0;
                }
            }
            EventKind::Miss { id, evidence } => {
                if let Some(lesson) = index.get_mut(id) {
                    push_unique(&mut lesson.evidence, evidence.clamped());
                    if let Some(file) = evidence.file.as_deref() {
                        if !lesson.miss_files.iter().any(|f| f == file) {
                            lesson.miss_files.push(file.to_string());
                        }
                    }
                    lesson.misses = lesson.misses.saturating_add(1);
                    lesson.consecutive_misses = lesson.consecutive_misses.saturating_add(1);
                    if lesson.consecutive_misses >= th.retire_misses && !lesson.status.is_sticky() {
                        lesson.status = LessonStatus::Retired;
                        lesson.retired_reason =
                            Some(format!("misses подряд: {}", lesson.consecutive_misses));
                    }
                }
            }
            EventKind::Promote { id, rule, .. } => {
                if let Some(lesson) = index.get_mut(id) {
                    lesson.status = LessonStatus::Promoted;
                    lesson.promoted_rule = Some(rule.clone());
                }
            }
            EventKind::Retire { id, reason } => {
                if let Some(lesson) = index.get_mut(id) {
                    lesson.status = LessonStatus::Retired;
                    lesson.retired_reason = Some(reason.clone());
                }
            }
        }
    }
    let mut lessons: Vec<Lesson> = index.into_values().collect();
    for lesson in &mut lessons {
        let fails = lesson
            .evidence
            .iter()
            .filter(|e| e.outcome == Outcome::Fail)
            .count();
        let hits = lesson
            .evidence
            .iter()
            .filter(|e| e.outcome == Outcome::PassAfterApply)
            .count();
        lesson.fails = u32::try_from(fails).unwrap_or(u32::MAX);
        lesson.hits = u32::try_from(hits).unwrap_or(u32::MAX);
        if !lesson.status.is_sticky() {
            lesson.status = lesson.derived_status(th);
        }
    }
    lessons.sort_by(|a, b| a.id.cmp(&b.id));
    lessons
}

/// Добавляет доказательство, если такого ещё нет (идемпотентность записи).
fn push_unique(evidence: &mut Vec<Evidence>, candidate: Evidence) {
    let duplicate = evidence.iter().any(|e| {
        e.run_id == candidate.run_id && e.ts == candidate.ts && e.outcome == candidate.outcome
    });
    if !duplicate {
        evidence.push(candidate);
    }
}

/// Читает журнал (толерантно): `(события, число нераспознанных строк)`.
///
/// # Errors
/// Ошибка чтения файла.
pub fn read_events(path: &Path) -> Result<(Vec<LedgerEvent>, usize)> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok((Vec::new(), 0)),
        Err(e) => return Err(HarnessError::io(path, e)),
    };
    let mut events = Vec::new();
    let mut malformed = 0;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        match serde_json::from_str::<LedgerEvent>(line) {
            Ok(event) if event.v == LEDGER_VERSION => events.push(event),
            Ok(other) => {
                // Чужая (будущая) версия схемы: строка сохраняется в файле, но
                // в проекцию не попадает — новый код не «съедает» данные старого.
                malformed += 1;
                tracing::warn!(
                    "{}: событие версии {} пропущено (текущая {})",
                    path.display(),
                    other.v,
                    LEDGER_VERSION
                );
            }
            Err(e) => {
                malformed += 1;
                tracing::warn!("{}: строка журнала не разобрана: {e}", path.display());
            }
        }
    }
    Ok((events, malformed))
}

/// Дозапись событий в журнал (вызывается только под [`FileLock`]).
fn append_events(path: &Path, events: &[LedgerEvent]) -> Result<()> {
    let mut buffer = String::new();
    for event in events {
        let line = serde_json::to_string(event)?;
        let _ = writeln!(buffer, "{line}");
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| HarnessError::io(path, e))?;
    file.write_all(buffer.as_bytes())
        .map_err(|e| HarnessError::io(path, e))?;
    file.flush().map_err(|e| HarnessError::io(path, e))
}

/// Файловая блокировка дозаписи: эксклюзивный файл-замок рядом с журналом.
///
/// Замок — не «честный» `flock` (в std его нет), а атомарное создание файла:
/// эксклюзивность даёт `create_new`. Брошенный замок (процесс убит) снимается
/// по возрасту — [`LOCK_STALE_SECS`].
struct FileLock {
    path: PathBuf,
}

impl FileLock {
    fn acquire(path: &Path) -> Result<Self> {
        let deadline = Instant::now() + LOCK_TIMEOUT;
        loop {
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path)
            {
                Ok(mut file) => {
                    let _ = write!(file, "{}", std::process::id());
                    return Ok(Self {
                        path: path.to_path_buf(),
                    });
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    if is_stale(path) {
                        let _ = std::fs::remove_file(path);
                        continue;
                    }
                    if Instant::now() >= deadline {
                        return Err(HarnessError::Control(format!(
                            "реестр уроков: блокировка {} не освободилась за {} с",
                            path.display(),
                            LOCK_TIMEOUT.as_secs()
                        )));
                    }
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(e) => return Err(HarnessError::io(path, e)),
            }
        }
    }
}

/// Замок старше [`LOCK_STALE_SECS`] считается брошенным (процесс убит).
fn is_stale(path: &Path) -> bool {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|mtime| mtime.elapsed().ok())
        .is_some_and(|age| age.as_secs() >= LOCK_STALE_SECS)
}

impl Drop for FileLock {
    fn drop(&mut self) {
        // Снятие замка best-effort: неудача (файла уже нет) не влияет на
        // корректность — брошенный замок снимет проверка возраста.
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Текущее время в ISO 8601 с миллисекундами (метки событий).
#[must_use]
pub fn now_iso() -> String {
    chrono::Local::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// Сегодняшняя дата в UTC (`YYYY-MM-DD`) — сравнение с `expiry`.
#[must_use]
pub fn today() -> String {
    chrono::Utc::now().date_naive().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn evidence(run: &str, outcome: Outcome) -> Evidence {
        Evidence {
            run_id: run.to_string(),
            harness: Some("claude-code".to_string()),
            verdict_excerpt: format!("[{run}] вердикт"),
            file: Some("src/lib.rs".to_string()),
            line: Some(12),
            ts: format!("2026-10-14T10:00:{:02}Z", run.len()),
            outcome,
            task: Some(run.to_string()),
        }
    }

    fn record_input(code: &str, run: &str, outcome: Outcome) -> RecordInput {
        RecordInput::new(
            Signature::new(LessonSource::Gate, code),
            evidence(run, outcome),
        )
        .with_statement("правило")
    }

    #[test]
    fn id_is_stable_and_signature_separated() {
        let a = Signature::new(LessonSource::Gate, "G5");
        let b = Signature::new(LessonSource::Gate, "G5");
        assert_eq!(a.id(), b.id(), "id — производная подписи");
        assert!(a.id().starts_with(ID_PREFIX));
        assert_eq!(a.id().len(), ID_PREFIX.len() + ID_HASH_CHARS);
        // Склейка кодов не даёт коллизии (unit separator в канонической форме).
        assert_ne!(
            Signature::new(LessonSource::Gate, "G5X").id(),
            Signature::new(LessonSource::Gate, "G5")
                .with_subject("X")
                .id()
        );
        assert_ne!(
            Signature::new(LessonSource::Gate, "G5").id(),
            Signature::new(LessonSource::Fitness, "G5").id()
        );
        // Нормализация пробелов: «G  5» и «G 5» — одна подпись.
        assert_eq!(
            Signature::new(LessonSource::Gate, "G   5").id(),
            Signature::new(LessonSource::Gate, "G 5").id()
        );
        assert_eq!(a.describe(), "gate · G5");
        assert_eq!(
            a.clone().with_subject("new_public_symbol").describe(),
            "gate · G5 · new_public_symbol"
        );
    }

    /// Табличный тест жизненного цикла: только эти переходы допустимы.
    #[test]
    fn status_transitions_table() {
        // (fails, hits, misses, серия, задач, ожидаемый статус, комментарий)
        let cases: &[(u32, u32, u32, u32, u32, LessonStatus, &str)] = &[
            (1, 0, 0, 0, 1, LessonStatus::Observed, "первый FAIL"),
            (2, 0, 0, 0, 1, LessonStatus::Candidate, "FAIL повторился"),
            (1, 1, 0, 0, 1, LessonStatus::Candidate, "применён и PASS"),
            (
                2,
                2,
                0,
                0,
                1,
                LessonStatus::Candidate,
                "две hits, но одна задача",
            ),
            (
                2,
                2,
                0,
                0,
                2,
                LessonStatus::Confirmed,
                "hits≥2, misses=0, 2 задачи",
            ),
            (
                3,
                3,
                0,
                0,
                3,
                LessonStatus::Confirmed,
                "больше доказательств",
            ),
            (
                2,
                2,
                1,
                1,
                2,
                LessonStatus::Candidate,
                "miss откатывает confirmed",
            ),
            (
                0,
                0,
                0,
                0,
                0,
                LessonStatus::Observed,
                "пустой урок — observed",
            ),
        ];
        let th = Thresholds::default();
        for (fails, hits, misses, consecutive, tasks, want, why) in cases {
            let mut lesson = Lesson::from_input(&record_input("G5", "run-1", Outcome::Fail));
            lesson.fails = *fails;
            lesson.hits = *hits;
            lesson.misses = *misses;
            lesson.consecutive_misses = *consecutive;
            lesson.evidence = (0..*tasks)
                .map(|i| evidence(&format!("task{i}"), Outcome::Fail))
                .collect();
            assert_eq!(
                lesson.derived_status(&th),
                *want,
                "fails={fails} hits={hits} misses={misses} tasks={tasks}: {why}"
            );
        }
    }

    #[test]
    fn fold_walks_the_lifecycle_observed_to_confirmed() {
        let th = Thresholds::default();
        let signature = Signature::new(LessonSource::Gate, "G5");
        let id = signature.id();
        let dir = tempfile::tempdir().expect("tempdir");
        let ledger = Ledger::at(dir.path().join(LEDGER_FILE));

        // 1. Первый FAIL — урок рождается в observed.
        let first = ledger
            .record(
                &record_input("G5", "task1", Outcome::Fail),
                &th,
                "2026-10-14",
            )
            .expect("record 1");
        assert!(first.created);
        assert_eq!(first.lesson.status, LessonStatus::Observed);
        assert_eq!(first.lesson.id, id);
        assert_eq!(first.lesson.fails, 1);
        assert_eq!(
            first.lesson.provenance.arch_be,
            super::super::arch_be_version()
        );
        assert_eq!(first.lesson.provenance.verdict_sha256.len(), 64);

        // 2. Повторный FAIL той же подписи — candidate (не второй урок).
        let second = ledger
            .record(
                &record_input("G5", "task2", Outcome::Fail),
                &th,
                "2026-10-14",
            )
            .expect("record 2");
        assert!(!second.created, "подпись та же — урок не размножается");
        assert_eq!(second.lesson.status, LessonStatus::Candidate);
        assert!(second.status_changed());

        // 3–4. Урок применён и дал PASS в двух разных задачах — confirmed.
        ledger
            .record(
                &record_input("G5", "task3", Outcome::PassAfterApply),
                &th,
                "2026-10-14",
            )
            .expect("hit 1");
        let confirmed = ledger
            .record(
                &record_input("G5", "task4", Outcome::PassAfterApply),
                &th,
                "2026-10-14",
            )
            .expect("hit 2");
        assert_eq!(confirmed.lesson.status, LessonStatus::Confirmed);
        assert_eq!(confirmed.lesson.hits, 2);
        assert_eq!(confirmed.lesson.misses, 0);
        assert_eq!(confirmed.lesson.task_count(), 4);

        // Проекция с диска совпадает с возвращённой (журнал — источник истины).
        let snapshot = ledger.snapshot(&th).expect("snapshot");
        assert_eq!(snapshot.lessons.len(), 1);
        assert_eq!(snapshot.lessons[0].status, LessonStatus::Confirmed);
        assert_eq!(snapshot.malformed, 0);
    }

    #[test]
    fn two_consecutive_misses_retire_the_lesson() {
        let th = Thresholds::default();
        let dir = tempfile::tempdir().expect("tempdir");
        let ledger = Ledger::at(dir.path().join(LEDGER_FILE));
        ledger
            .record(
                &record_input("R1", "t1", Outcome::PassAfterApply),
                &th,
                "2026-10-14",
            )
            .expect("hit 1");
        ledger
            .record(
                &record_input("R1", "t2", Outcome::PassAfterApply),
                &th,
                "2026-10-14",
            )
            .expect("hit 2");
        // Первый промах — откат в candidate, второй подряд — retired.
        let missed = ledger
            .record(&record_input("R1", "t3", Outcome::Fail), &th, "2026-10-14")
            .expect("miss 1");
        assert_eq!(missed.lesson.status, LessonStatus::Candidate);
        let retired = ledger
            .record(&record_input("R1", "t4", Outcome::Fail), &th, "2026-10-14")
            .expect("miss 2");
        assert_eq!(retired.lesson.status, LessonStatus::Retired);
        assert!(
            retired
                .lesson
                .retired_reason
                .as_deref()
                .unwrap_or("")
                .contains("misses подряд")
        );

        // «Липкость»: последующий PASS не воскрешает урок без явного события.
        let after = ledger
            .record(
                &record_input("R1", "t5", Outcome::PassAfterApply),
                &th,
                "2026-10-14",
            )
            .expect("hit после retired");
        assert_eq!(after.lesson.status, LessonStatus::Retired);
    }

    #[test]
    fn hit_resets_the_consecutive_miss_streak() {
        let th = Thresholds::default();
        let dir = tempfile::tempdir().expect("tempdir");
        let ledger = Ledger::at(dir.path().join(LEDGER_FILE));
        ledger
            .record(&record_input("R2", "t1", Outcome::Fail), &th, "2026-10-14")
            .expect("f1");
        ledger
            .record(&record_input("R2", "t2", Outcome::Fail), &th, "2026-10-14")
            .expect("f2");
        ledger
            .record(
                &record_input("R2", "t3", Outcome::PassAfterApply),
                &th,
                "2026-10-14",
            )
            .expect("h1");
        let last = ledger
            .record(&record_input("R2", "t4", Outcome::Fail), &th, "2026-10-14")
            .expect("f3");
        assert_eq!(last.lesson.consecutive_misses, 1, "hit сбросил серию");
        assert_eq!(last.lesson.misses, 1, "misses — счётчик применений");
        assert_eq!(last.lesson.fails, 3, "fails — наблюдённые FAIL");
    }

    #[test]
    fn duplicate_evidence_does_not_inflate_counters() {
        let th = Thresholds::default();
        let dir = tempfile::tempdir().expect("tempdir");
        let ledger = Ledger::at(dir.path().join(LEDGER_FILE));
        let input = record_input("R3", "t1", Outcome::Fail);
        let first = ledger.record(&input, &th, "2026-10-14").expect("record");
        let again = ledger.record(&input, &th, "2026-10-14").expect("record");
        assert_eq!(first.lesson.evidence.len(), 1);
        assert_eq!(
            again.lesson.evidence.len(),
            1,
            "то же доказательство (run_id+ts+исход) не дублируется"
        );
        assert_eq!(again.lesson.fails, 1);
    }

    #[test]
    fn repeat_record_keeps_statement_and_scope() {
        // Повторный FAIL без явной области не затирает область/формулировку,
        // вычисленные при первой записи (иначе урок «расплывался» бы к repo).
        let th = Thresholds::default();
        let dir = tempfile::tempdir().expect("tempdir");
        let ledger = Ledger::at(dir.path().join(LEDGER_FILE));
        let first = RecordInput::new(
            Signature::new(LessonSource::Gate, "G5"),
            evidence("t1", Outcome::Fail),
        )
        .with_statement("новый публичный символ требует __all__")
        .with_scope(Scope::new(
            super::super::ScopeLevel::Module,
            vec!["paylib/**".to_string()],
        ));
        ledger.record(&first, &th, "2026-10-14").expect("record 1");

        // Вторая запись: ни формулировки, ни области (дефолт repo).
        let bare = RecordInput::new(
            Signature::new(LessonSource::Gate, "G5"),
            evidence("t2", Outcome::Fail),
        );
        let second = ledger.record(&bare, &th, "2026-10-14").expect("record 2");
        assert_eq!(second.lesson.scope.describe(), "module (globs: paylib/**)");
        assert_eq!(
            second.lesson.statement,
            "новый публичный символ требует __all__"
        );
        assert_eq!(second.lesson.fails, 2, "доказательства накопились");
        assert_eq!(second.lesson.status, LessonStatus::Candidate);
    }

    #[test]
    fn expiry_retires_the_lesson_on_refresh() {
        let th = Thresholds::default();
        let dir = tempfile::tempdir().expect("tempdir");
        let ledger = Ledger::at(dir.path().join(LEDGER_FILE));
        let input = record_input("R4", "t1", Outcome::PassAfterApply).with_expiry("2026-01-01");
        ledger.record(&input, &th, "2026-01-01").expect("record");
        let snapshot = ledger.snapshot(&th).expect("snapshot");
        assert_eq!(
            snapshot.lessons[0].status,
            LessonStatus::Candidate,
            "срок ещё не истёк"
        );

        let retired = ledger.refresh("2026-01-02", &th).expect("refresh");
        assert_eq!(retired, 1, "просроченный урок выведен");
        let snapshot = ledger.snapshot(&th).expect("snapshot");
        assert_eq!(snapshot.lessons[0].status, LessonStatus::Retired);
        assert!(
            snapshot.lessons[0]
                .retired_reason
                .as_deref()
                .unwrap_or("")
                .contains("expiry")
        );
        // Повторный refresh идемпотентен — вторых событий вывода нет.
        assert_eq!(ledger.refresh("2026-01-02", &th).expect("refresh"), 0);
    }

    #[test]
    fn unknown_id_is_an_error() {
        let th = Thresholds::default();
        let dir = tempfile::tempdir().expect("tempdir");
        let ledger = Ledger::at(dir.path().join(LEDGER_FILE));
        let err = ledger
            .retire("LSN-deadbeef", "нет правила", &th)
            .expect_err("отказ");
        assert!(err.to_string().contains("не найден"), "{err}");
    }

    #[test]
    fn malformed_and_foreign_lines_do_not_break_the_projection() {
        let th = Thresholds::default();
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join(LEDGER_FILE);
        let ledger = Ledger::at(path.clone());
        ledger
            .record(&record_input("G5", "t1", Outcome::Fail), &th, "2026-10-14")
            .expect("record");
        // Мусор и строка чужой версии схемы дописываются мимо API.
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .expect("open");
        writeln!(file, "{{это не json}}").expect("мусор");
        writeln!(
            file,
            r#"{{"v":99,"ts":"2027-01-01T00:00:00Z","kind":"record","lesson":{{}}}}"#
        )
        .expect("чужая версия");
        let snapshot = ledger.snapshot(&th).expect("snapshot");
        assert_eq!(snapshot.lessons.len(), 1, "проекция цела");
        assert_eq!(snapshot.malformed, 2);
        assert_eq!(snapshot.events, 1);
    }

    #[test]
    fn promote_is_sticky_and_recorded() {
        let th = Thresholds::default();
        let dir = tempfile::tempdir().expect("tempdir");
        let ledger = Ledger::at(dir.path().join(LEDGER_FILE));
        let input = record_input("R5", "t1", Outcome::PassAfterApply);
        let lesson = ledger
            .record(&input, &th, "2026-10-14")
            .expect("record")
            .lesson;
        let promoted = ledger
            .promote(&lesson.id, "C-99-check", Some("changes/x"), &th)
            .expect("promote");
        assert_eq!(promoted.lesson.status, LessonStatus::Promoted);
        assert_eq!(promoted.lesson.promoted_rule.as_deref(), Some("C-99-check"));
        // Промах не откатывает промоутированный урок (решение зафиксировано).
        let after = ledger
            .record(&record_input("R5", "t2", Outcome::Fail), &th, "2026-10-14")
            .expect("miss");
        assert_eq!(after.lesson.status, LessonStatus::Promoted);
    }

    #[test]
    fn evidence_keys_deserialize_without_optional_fields() {
        // Контракт совместимости: минимальное доказательство (только обязательные
        // поля) читается, опциональные не падают.
        let json = r#"{"run_id":"r1","ts":"2026-10-14T10:00:00Z","outcome":"fail"}"#;
        let evidence: Evidence = serde_json::from_str(json).expect("разбор");
        assert_eq!(evidence.outcome, Outcome::Fail);
        assert_eq!(evidence.task_key(), "r1", "без задачи ключ — run_id");
        assert!(evidence.location().is_none());
    }

    #[test]
    fn event_roundtrip_keeps_schema() {
        let dir = tempfile::tempdir().expect("tempdir");
        let ledger = Ledger::at(dir.path().join(LEDGER_FILE));
        let th = Thresholds::default();
        let lesson = ledger
            .record(&record_input("G7", "t1", Outcome::Fail), &th, "2026-10-14")
            .expect("record")
            .lesson;
        let (events, malformed) = read_events(ledger.path()).expect("read");
        assert_eq!(malformed, 0);
        assert_eq!(events.len(), 1);
        let line = serde_json::to_string(&events[0]).expect("serde");
        assert!(
            line.contains(r#""kind":"record""#),
            "вид события в строке: {line}"
        );
        assert!(line.contains(r#""v":1"#), "версия схемы в строке: {line}");
        let back: LedgerEvent = serde_json::from_str(&line).expect("обратный разбор");
        assert_eq!(back, events[0]);
        match &back.event {
            EventKind::Record { lesson: parsed } => {
                assert_eq!(parsed.id, lesson.id);
                assert_eq!(parsed.signature.code, "G7");
            }
            other => panic!("ожидалось record, получено {other:?}"),
        }
    }

    #[test]
    fn concurrent_mutations_from_threads_keep_jsonl_valid() {
        // Внутрипроцессная проверка блокировки (межпроцессная — в
        // tests/lessons_ledger.rs): восемь потоков пишут в один журнал.
        let th = Thresholds::default();
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join(LEDGER_FILE);
        let mut handles = Vec::new();
        for worker in 0..8 {
            let path = path.clone();
            handles.push(std::thread::spawn(move || {
                let ledger = Ledger::at(path);
                for i in 0..5 {
                    let code = format!("W{worker}-{i}");
                    ledger
                        .record(
                            &record_input(&code, &format!("t{worker}-{i}"), Outcome::Fail),
                            &th,
                            "2026-10-14",
                        )
                        .expect("record");
                }
            }));
        }
        for handle in handles {
            handle.join().expect("поток");
        }
        let (events, malformed) = read_events(&path).expect("read");
        assert_eq!(malformed, 0, "все строки журнала валидны");
        assert_eq!(events.len(), 40, "ни одна запись не потеряна");
        let text = std::fs::read_to_string(&path).expect("read");
        for line in text.lines() {
            serde_json::from_str::<LedgerEvent>(line).expect("каждая строка — валидный JSON");
        }
    }

    #[test]
    fn clamp_chars_respects_unicode_boundaries() {
        assert_eq!(clamp_chars("абв", 2), "аб");
        assert_eq!(clamp_chars("abc", 10), "abc");
        let long = "я".repeat(MAX_EXCERPT_CHARS + 10);
        let clamped = clamp_chars(&long, MAX_EXCERPT_CHARS);
        assert_eq!(clamped.chars().count(), MAX_EXCERPT_CHARS);
    }
}
