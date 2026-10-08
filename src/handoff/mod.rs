//! Генерация handoff-пакетов (передача архитектуры в код) — чисто-файловое
//! ядро, доступное в обеих сборках (core и harness).
//!
//! КОНТРАКТ (владелец: агент `harness`; модуль выделен из `harness.rs` в
//! волне 2, п.10 — главный доказательный кейс drift-control построен на
//! пакете, и он обязан быть доступен Core-пользователю через MCP
//! `handoff_create`):
//! - [`generate_handoff`] — каталог `<repo>/.arch-handoff/`: TASK.md (задача +
//!   критерии приёмки из QAS-сущностей `<repo>/model/`, ADR-007),
//!   ARCHITECTURE.md (свод спек/спайна), adr/ (копии ADR), CONSTRAINTS.yaml
//!   (fitness-правила под стек репозитория — заготовка, переписывается
//!   архитектором под spine), SPEC.md (шаблон верифицируемых контрактов
//!   интерфейсов: входы/выходы, структуры данных, границы ошибок, критерии
//!   верификации), RUBRIC.yaml (якорная рубрика приёмки), ROLLBACK.yaml
//!   (машиночитаемый план отката — репетируется на гейте A4, см.
//!   `crate::rehearsal`), MANIFEST.json
//!   (мета: дата, модель, источники, маршрут, `baseline_commit`, `rollback_plan`) +
//!   компактный epic-context (800–1500 токенов, по смыслу);
//! - [`tools`] — инструмент `handoff_create` для агентного цикла и моста MCP
//!   (регистрируется в `tools::domain_tools` в обеих сборках).
//!
//! Здесь НЕТ сети, LLM, TUI и запуска внешних процессов-харнессов: прогон
//! пакета кодовым харнессом (`run_harness`, инструмент `harness_run`,
//! адаптеры) остаётся в `crate::harness` (сборка `harness`).

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::config::Config;
use crate::control::Route;
use crate::error::{HarnessError, Result};
use crate::llm::ToolSpec;
use crate::model::{EntityKind, load_model};
use crate::tool::{Tool, ToolContext, ToolOutput};

/// Имя каталога handoff-пакета в корне репозитория.
pub(crate) const HANDOFF_DIR: &str = ".arch-handoff";

/// Максимальный размер epic-context (ARCHITECTURE.md), символов
/// (~1500 токенов при грубой оценке 4 символа ≈ 1 токен).
const EPIC_CONTEXT_MAX_CHARS: usize = 6000;

/// Целевой минимум epic-context, символов (~800 токенов — низ окна рубрики
/// `handoff_quality`). Если на глубине «2 абзаца на секцию» контекст меньше,
/// секции перерендериваются глубже ([`DEPTH_DEEP`]).
const EPIC_CONTEXT_MIN_CHARS: usize = 3200;

/// Глубина рендера прочих секций по умолчанию (абзацев на секцию).
const DEPTH_SHALLOW: usize = 2;
/// Глубина рендера прочих секций при недоборе epic-context (абзацев).
const DEPTH_DEEP: usize = 8;

/// Баннер машинной компиляции — первая строка сгенерированного SPEC.md
/// (находка живого эксперимента: исполнитель принимал машинную компиляцию
/// за авторскую спеку архитектора). Пишется в обеих ветках генерации
/// (шаблон и сборка из переданных спек); пользовательский файл не затирается.
const SPEC_MACHINE_BANNER: &str =
    "> СКОМПИЛИРОВАНО МАШИНОЙ из spine/ADR/NFR — требует авторской правки архитектора.";

/// Шаблон SPEC.md — верифицируемые контракты интерфейсов компонента
/// (модель «5.2»: прозаический ARCHITECTURE.md компонента заменяется spec'ом
/// с контрактами, проверяемыми тестами). Пишется только при отсутствии —
/// заполненный архитектором файл повторная генерация не затирает.
/// Первая строка готового файла — баннер [`SPEC_MACHINE_BANNER`].
const SPEC_TEMPLATE: &str = "# SPEC — контракты интерфейсов компонента\n\
\n\
> Шаблон handoff-пакета (НЕ затирается при повторной генерации). Заполняется\n\
> архитектором ДО передачи: верифицируемые контракты вместо прозы. Требования\n\
> — в духе EARS: When <событие>, the <система> shall <реакция>.\n\
\n\
## Входы (контракты соседей)\n\
\n\
- <что компонент потребляет: API/события/файлы, от кого, формат и инварианты>\n\
\n\
## Выходы (публикуемые контракты)\n\
\n\
- <что компонент публикует: API/события/модели данных, гарантии (идемпотентность, порядок, версии)>\n\
\n\
## Структуры данных\n\
\n\
- <ключевые типы/схемы на границах: поля, единицы, ограничения>\n\
\n\
## Границы ошибок\n\
\n\
- <какие ошибки возвращаются/маппятся, какие эскалируются; коды и семантика повторов>\n\
\n\
## Критерии верификации (тесты)\n\
\n\
- [ ] When <событие>, the <система> shall <реакция> — <каким тестом проверяется>\n\
";

/// Собирает SPEC.md из переданных спек архитектора: полные тексты контрактов
/// с заголовками-источниками (шаблон-заполнитель уже не нужен — контракты
/// переданы явно). Первая строка — баннер машинной компиляции
/// [`SPEC_MACHINE_BANNER`]. Не-UTF8 читается с потерями.
fn render_spec_from_sources(spec_files: &[PathBuf]) -> String {
    let mut out = String::from(
        "# SPEC — контракты интерфейсов компонента\n\n\
         > Собрано автоматически из спек, переданных архитектором (`--spec`),\n\
         > при генерации пакета. НЕ затирается при повторной генерации —\n\
         > правьте под фактические контракты эпика.\n",
    );
    for path in spec_files {
        // Файл уже читался компилятором epic-context — падение маловероятно;
        // lossy-фолбэк вместо второй ошибки чтения.
        let text = std::fs::read(path)
            .map(|b| String::from_utf8_lossy(&b).into_owned())
            .unwrap_or_default();
        let _ = write!(
            out,
            "\n\n---\n\n## Источник: {}\n\n{}\n",
            path.display(),
            text.trim()
        );
    }
    out
}

/// Собирает RUBRIC.yaml пакета с критерием требований change `OpenSpec` (F6,
/// ADR-067): поверх якорной `handoff_quality` (когда она есть) добавляется
/// критерий `openspec_change_requirements` с перечнем id и SHALL-текстов;
/// без якоря (или при нечитаемом якоре) — рубрика из одного этого критерия.
///
/// Якорь разбирается как YAML-документ, а не конкатенируется текстом:
/// ключ `criteria` находится структурно, порядок ключей и комментарии якоря
/// на запись не влияют (пакетная копия — рабочий файл; канонический текст
/// якоря живёт в assets).
///
/// # Errors
/// Якорь невалиден как YAML, либо сгенерированная рубрика не разбирается
/// движком рубрик (самовалидация генератора — как у CONSTRAINTS.yaml).
fn render_rubric_with_change(
    anchor: Option<&str>,
    change_id: &str,
    requirements: &[crate::openspec::Requirement],
) -> Result<String> {
    use std::fmt::Write as _;
    let mut description = format!(
        "Реализация обязана покрыть требования change '{change_id}' (полные тексты — \
         в ARCHITECTURE.md и SPEC.md пакета):\n"
    );
    for r in requirements {
        let _ = writeln!(
            description,
            "- {} — «{}»: {}",
            r.id,
            r.title,
            r.statements.join(" ")
        );
    }
    let anchors = std::collections::BTreeMap::from([
        (
            1u8,
            "Требования change проигнорированы: реализация не покрывает их и не объясняет отступлений"
                .to_string(),
        ),
        (
            3u8,
            "Часть требований change покрыта; отступления названы, но не по каждому с причиной"
                .to_string(),
        ),
        (
            5u8,
            "Каждое требование change покрыто реализацией либо сознательно отклонено с причиной \
             в conflicts_with_prior_decisions"
                .to_string(),
        ),
    ]);
    let criterion = crate::rubric::Criterion {
        id: "openspec_change_requirements".to_string(),
        name: format!("Требования change '{change_id}'"),
        description,
        weight: 3.0,
        anchors,
        evidence_on: crate::rubric::EvidenceOn::default(),
        evidence_roles: Vec::new(),
        coverage: None,
        blocking: false,
    };
    let criterion_value = serde_yaml_ng::to_value(&criterion)
        .map_err(|e| HarnessError::Harness(format!("рубрика change: {e}")))?;
    let anchor_doc = anchor.and_then(|text| serde_yaml_ng::from_str(text).ok());
    let doc = if let Some(serde_yaml_ng::Value::Mapping(mut mapping)) = anchor_doc {
        let key = serde_yaml_ng::Value::from("criteria");
        // Двухшаговая проверка (get → get_mut/insert): условный займ одним
        // match NLL не разрешает (классический случай).
        if mapping
            .get(&key)
            .is_some_and(serde_yaml_ng::Value::is_sequence)
        {
            if let Some(serde_yaml_ng::Value::Sequence(seq)) = mapping.get_mut(&key) {
                seq.push(criterion_value);
            }
        } else {
            // Якорь без списка criteria (или с кривым) — критерий образует
            // список сам.
            mapping.insert(key, serde_yaml_ng::Value::Sequence(vec![criterion_value]));
        }
        serde_yaml_ng::Value::Mapping(mapping)
    } else {
        let rubric = crate::rubric::Rubric {
            name: "handoff_openspec_change".to_string(),
            description: format!(
                "Приёмка handoff-пакета по требованиям change OpenSpec '{change_id}' (F6): \
                 якорной рубрики handoff_quality в assets нет — критерий один"
            ),
            scale_max: 5,
            criteria: vec![criterion],
            origin: "dynamic".to_string(),
            pack: None,
        };
        serde_yaml_ng::to_value(&rubric)
            .map_err(|e| HarnessError::Harness(format!("рубрика change: {e}")))?
    };
    let text = serde_yaml_ng::to_string(&doc)
        .map_err(|e| HarnessError::Harness(format!("рубрика change: {e}")))?;
    // Самовалидация генератора: рубрика обязана читаться движком ДО записи
    // в пакет — битый файл исполнителю недопустим (прецедент — дефект A1
    // CONSTRAINTS.yaml).
    serde_yaml_ng::from_str::<crate::rubric::Rubric>(&text).map_err(|e| {
        HarnessError::Harness(format!(
            "сгенерированная RUBRIC.yaml не разбирается: {e} — дефект генератора, \
             а не данных change"
        ))
    })?;
    Ok(text)
}

/// Дефолтные fitness-правила под стек репозитория (по маркерным файлам):
/// Cargo.toml → Rust; pyproject.toml/requirements.txt/setup.py → Python;
/// go.mod → Go; package.json → Node; иначе — минимальный общий набор.
/// Пишутся только при отсутствии пользовательского CONSTRAINTS.yaml и всегда
/// остаются заготовкой: перед передачей архитектор переписывает их под
/// spine-инварианты (AD-n) эпика.
fn default_constraints(repo: &Path) -> String {
    let stack = if repo.join("Cargo.toml").is_file() {
        "Rust"
    } else if ["pyproject.toml", "requirements.txt", "setup.py"]
        .iter()
        .any(|m| repo.join(m).is_file())
    {
        "Python"
    } else if repo.join("go.mod").is_file() {
        "Go"
    } else if repo.join("package.json").is_file() {
        "Node"
    } else {
        "generic"
    };
    // Контент каждого шаблона начинается сразу после открывающей кавычки на
    // той же строке: форма `"\<перевод строки>` съедала бы перевод строки И
    // ведущие пробелы первой строки, ломая отступы YAML (дефект A1 живого
    // эксперимента — исполнителю уезжал нечитаемый CONSTRAINTS.yaml).
    // Отступы консистентны с корневым CONSTRAINTS.yaml репозитория: пункты
    // списка — 2 пробела под `rules:`, ключи правила — 4 пробела.
    let rules = match stack {
        "Rust" => {
            "  - name: no-unwrap-in-src
    type: must_not_contain
    glob: \"src/**\"
    pattern: 'unwrap\\('
    severity: warn
  - name: no-dbg-macro
    type: must_not_contain
    glob: \"src/**\"
    pattern: 'dbg!'
    severity: error
  - name: readme-exists
    type: file_exists
    path: README.md
    severity: warn
  - name: cargo-check-passes
    type: command_succeeds
    command: 'cargo check'
    timeout_secs: 120
    severity: error
"
        }
        "Python" => {
            "  - name: no-print-in-py
    type: must_not_contain
    glob: \"**/*.py\"
    pattern: 'print\\('
    severity: warn
  - name: readme-exists
    type: file_exists
    path: README.md
    severity: warn
  - name: pytest-passes
    type: command_succeeds
    command: 'pytest -q'
    timeout_secs: 180
    severity: error
"
        }
        "Go" => {
            "  - name: go-build-passes
    type: command_succeeds
    command: 'go build ./...'
    timeout_secs: 180
    severity: error
  - name: go-vet-passes
    type: command_succeeds
    command: 'go vet ./...'
    timeout_secs: 180
    severity: warn
  - name: readme-exists
    type: file_exists
    path: README.md
    severity: warn
"
        }
        "Node" => {
            "  - name: readme-exists
    type: file_exists
    path: README.md
    severity: warn
  - name: npm-test-passes
    type: command_succeeds
    command: 'npm test'
    timeout_secs: 300
    severity: warn
"
        }
        _ => {
            "  - name: readme-exists
    type: file_exists
    path: README.md
    severity: warn
"
        }
    };
    format!(
        "# Fitness-правила для `arch-be control check` (схема control::check).\n\
         # Стек: {stack} (детектирован по маркерным файлам). Заготовка генератора\n\
         # handoff (файл НЕ затирается при повторной генерации): перед передачей\n\
         # перепишите правила под spine-инварианты (AD-n) эпика.\n\
         rules:\n{rules}"
    )
}

/// Самовалидация генератора: сгенерированный текст CONSTRAINTS.yaml обязан
/// парситься как YAML ДО записи в пакет — битый файл лучше отклонить здесь,
/// чем выдать исполнителю нечитаемый (дефект A1: шаблоны теряли отступ
/// первой строки и выдавали YAML с `ScannerError`).
///
/// # Errors
/// Текст не парсится как YAML — это дефект генератора, а не данных репозитория.
fn validate_constraints_text(text: &str) -> Result<()> {
    serde_yaml_ng::from_str::<serde_yaml_ng::Value>(text)
        .map(|_| ())
        .map_err(|e| {
            HarnessError::Harness(format!(
                "сгенерированный CONSTRAINTS.yaml невалиден как YAML: {e} — пакет не \
                 собирается (это дефект шаблонов генератора, а не данных репозитория)"
            ))
        })
}

/// Итог генерации handoff-пакета.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HandoffPacket {
    /// Каталог `.arch-handoff/`.
    #[serde(alias = "path")]
    pub dir: PathBuf,
    /// Файлы пакета (включая сохранённые пользовательские CONSTRAINTS.yaml/SPEC.md/RUBRIC.yaml).
    pub files: Vec<PathBuf>,
    /// Оценка размера epic-context в токенах.
    pub epic_context_tokens: usize,
    /// Baseline-коммит (якорь отката) на момент генерации пакета.
    #[serde(default)]
    pub baseline: Option<String>,
    /// Git-репозиторий был инициализирован предгейтом (`git init`).
    #[serde(default)]
    pub git_initialized: bool,
    /// На момент генерации есть незакоммиченные изменения отслеживаемых
    /// файлов (откат на baseline их потеряет).
    #[serde(default)]
    pub git_dirty_tracked: bool,
    /// Рекомендованный таймаут прогона по маршруту значимости, секунд.
    #[serde(default)]
    pub recommended_timeout_secs: u64,
    /// Детерминированные предупреждения готовности пакета (тонкая
    /// декомпозиция REQ → задачи и т.п.); не блокируют сборку.
    #[serde(default)]
    pub warnings: Vec<String>,
    /// Что сделано с пакетным реестром правил (T-02): копия корневого
    /// `CONSTRAINTS.yaml`, заготовка под стек или «существующий файл не
    /// тронут». `None` — реестр в пакете не писался.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub constraints_action: Option<String>,
    /// Change `OpenSpec`, из которого собран пакет (F6, ADR-067).
    /// Аддитивное поле 0.3.14.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub openspec_change: Option<String>,
    /// Требований change вписано в рубрику пакета (0 — change не задан или
    /// требований нет). Аддитивное поле 0.3.14.
    #[serde(default)]
    pub openspec_requirements: usize,
}

/// Опции генерации пакета (T-02).
#[derive(Debug, Clone, Default)]
pub struct HandoffOptions {
    /// Перезаписать пакетную копию реестра правил (`--refresh-constraints`):
    /// по умолчанию существующий файл пакета не трогается — правки
    /// архитектора сохраняются.
    pub refresh_constraints: bool,
    /// Change `OpenSpec` как источник пакета (F6, ADR-067): `proposal.md`,
    /// `design.md`, `tasks.md` и дельты спек change кладутся в пакет как
    /// `--spec` (первыми: лесенка усечения epic-context режет прозу с хвоста,
    /// и предмет задачи не должен попасть под сокращение), требования change —
    /// критерием рубрики пакета. Markdown `OpenSpec` только читается (правило
    /// 9, `docs/openspec.md`).
    pub openspec_change: Option<String>,
}

/// Метаданные пакета (`MANIFEST.json`).
#[derive(Serialize)]
struct Manifest<'a> {
    /// Дата создания, ISO 8601 (UTC).
    created_at: String,
    /// Формулировка задачи.
    task: &'a str,
    /// Модель по умолчанию из конфига.
    model: &'a str,
    /// Файлы-источники спецификаций.
    sources: Vec<String>,
    /// Размер epic-context, символов.
    epic_context_chars: usize,
    /// Оценка размера epic-context, токенов (~chars/4).
    epic_context_tokens: usize,
    /// Маршрут значимости (Fast/Standard/Critical).
    route: String,
    /// Рекомендованный таймаут прогона по маршруту, секунд.
    recommended_timeout_secs: u64,
    /// Baseline-коммит (якорь отката) на момент генерации пакета.
    #[serde(skip_serializing_if = "Option::is_none")]
    baseline_commit: Option<String>,
    /// План отката (текст раздела «План отката» TASK.md).
    rollback_plan: &'a str,
    /// Пины контрольной плоскости (A3): sha256 каждого файла из перечня,
    /// решающего, работает ли гейт; `null` — файла не было при выдаче пакета
    /// (появление после выдачи — тоже расхождение). Сверяет составляющая
    /// `control_plane`. Аддитивное поле: старые пакеты (без него) гейт
    /// пропускает — обратная совместимость.
    control_plane: BTreeMap<String, Option<crate::control_plane::Pin>>,
    /// Change `OpenSpec` — источник пакета (F6, ADR-067); аддитивное поле.
    #[serde(skip_serializing_if = "Option::is_none")]
    openspec_change: Option<&'a str>,
}

/// Модель-автор кода из контракта передачи (E5.3): `MANIFEST.json` пакета
/// называет модель, назначенную для реализации задачи. Это единственное
/// машинное свидетельство авторства кода до того, как кто-то назовёт его
/// аргументом; по нему считается независимость судьи (E5.2) и ловится
/// «судья судил свой же код».
///
/// Пустое значение и отсутствие пакета — `None`: догадки вместо свидетельства
/// не подставляются, и в отчёте честно остаётся «автор не указан».
#[must_use]
pub fn author_model_from_contract(repo: &Path) -> Option<String> {
    #[derive(serde::Deserialize)]
    struct ManifestAuthor {
        #[serde(default)]
        model: Option<String>,
    }
    let text = std::fs::read_to_string(crate::control_plane::manifest_path(repo)).ok()?;
    serde_json::from_str::<ManifestAuthor>(&text)
        .ok()?
        .model
        .filter(|m| !m.trim().is_empty())
}

/// Генерирует handoff-пакет в репозиторий.
///
/// Создаёт `<repo>/.arch-handoff/` с TASK.md, ARCHITECTURE.md, MANIFEST.json,
/// adr/ (копии ADR) и, при отсутствии, CONSTRAINTS.yaml, SPEC.md и RUBRIC.yaml.
/// Перезаписываются только TASK.md, ARCHITECTURE.md и MANIFEST.json —
/// пользовательские правки CONSTRAINTS.yaml/SPEC.md/RUBRIC.yaml сохраняются.
///
/// Предгейт: гарантирует git-репозиторий и baseline-коммит-якорь отката
/// ([`ensure_git_baseline`]); `rollback` — явный план отката в TASK.md
/// (по умолчанию — откат на baseline с сигналами и владельцем решения);
/// `route` задаёт рекомендованный таймаут прогона (MANIFEST.json подхватывает
/// `harness_run`, когда `timeout_secs` не задан явно).
///
/// Маршрут Critical (rollback-first): пакет не собирается без git-якоря
/// отката (`baseline_commit`) и непустого плана отката — репетиция на гейте A4
/// (`crate::rehearsal`) требует обоих.
///
/// # Errors
/// Репозиторий недоступен, спека не читается, ошибка записи.
pub fn generate_handoff(
    repo: &Path,
    task: &str,
    spec_files: &[PathBuf],
    cfg: &Config,
    rollback: Option<&str>,
    route: Route,
) -> Result<HandoffPacket> {
    generate_handoff_opts(
        repo,
        task,
        spec_files,
        cfg,
        rollback,
        route,
        HandoffOptions::default(),
    )
}

/// [`generate_handoff`] с опциями (T-02).
///
/// # Errors
/// Как у [`generate_handoff`].
pub fn generate_handoff_opts(
    repo: &Path,
    task: &str,
    spec_files: &[PathBuf],
    cfg: &Config,
    rollback: Option<&str>,
    route: Route,
    opts: HandoffOptions,
) -> Result<HandoffPacket> {
    if !repo.is_dir() {
        return Err(HarnessError::Harness(format!(
            "репозиторий недоступен: {}",
            repo.display()
        )));
    }
    // Опции читаются через поля (значение потребляется деструктурированием —
    // HandoffOptions перестал быть Copy с полем `openspec_change`, F6).
    let HandoffOptions {
        refresh_constraints,
        openspec_change,
    } = opts;
    // F6 (ADR-067): change OpenSpec как источник пакета — proposal/design/
    // tasks и дельты спек change идут как --spec (ПЕРВЫМИ: лесенка усечения
    // epic-context режет прозу с хвоста, и предмет задачи не должен попасть
    // под сокращение), требования change — в рубрику пакета (ниже). Порог
    // контекста Critical считается уже по объединённому набору.
    let mut spec_files: Vec<PathBuf> = spec_files.to_vec();
    let mut change_requirements: Vec<crate::openspec::Requirement> = Vec::new();
    if let Some(change_id) = &openspec_change {
        let mut merged = crate::openspec::change_packet_files(repo, change_id)?;
        change_requirements = crate::openspec::change_requirements(repo, change_id)?;
        merged.append(&mut spec_files);
        // Дедуп по пути с сохранением порядка: один и тот же файл, переданный
        // и через --spec, и через change, не должен давать две секции.
        let mut seen = std::collections::BTreeSet::new();
        merged.retain(|p| seen.insert(p.clone()));
        spec_files = merged;
    }
    let baseline = ensure_git_baseline(repo);
    let rollback_text =
        rollback.map_or_else(|| default_rollback(baseline.hash.as_deref()), str::to_owned);
    // Rollback-first для маршрута Critical: пакет обязан нести якорь отката
    // (baseline_commit) и непустой план отката — без них репетиция на гейте
    // A4 невозможна, а откат превращается в импровизацию на инциденте.
    if route == Route::Critical {
        if baseline.hash.is_none() {
            return Err(HarnessError::Harness(
                "маршрут Critical требует baseline_commit (git-якорь отката), но git \
                 в репозитории недоступен — пакет не собирается"
                    .into(),
            ));
        }
        if rollback_text.trim().is_empty() {
            return Err(HarnessError::Harness(
                "маршрут Critical требует непустой план отката (rollback_plan): передайте \
                 --rollback с шагами отката или опустите его — дефолт откатит на baseline"
                    .into(),
            ));
        }
    }
    let timeout = recommended_timeout(route);
    let dir = repo.join(HANDOFF_DIR);
    let adr_dir = dir.join("adr");
    std::fs::create_dir_all(&adr_dir).map_err(|e| HarnessError::io(&adr_dir, e))?;

    // TASK.md — всегда перезаписывается (задача новая на каждый прогон).
    // Критерии приёмки разворачиваются из QAS-сущностей модели репозитория
    // (ADR-007): нет model/ или нет QAS — секции нет; битая модель — ошибка.
    let qas_section = qas_acceptance_section(repo)?;
    let task_path = dir.join("TASK.md");
    let task_md = render_task_md(task, &rollback_text, qas_section.as_deref());
    std::fs::write(&task_path, &task_md).map_err(|e| HarnessError::io(&task_path, e))?;

    // Детерминированные предупреждения готовности пакета (не блокируют
    // сборку): тонкая декомпозиция REQ → задачи TASK.md.
    let mut warnings = Vec::new();
    if let Some(w) = req_decomposition_warning(repo, &task_md) {
        warnings.push(w);
    }

    // ARCHITECTURE.md — всегда перезаписывается (компиляция актуальных спек).
    let arch_md = compile_epic_context(&spec_files)?;
    let arch_path = dir.join("ARCHITECTURE.md");
    std::fs::write(&arch_path, &arch_md).map_err(|e| HarnessError::io(&arch_path, e))?;
    let epic_chars = arch_md.chars().count();
    let epic_tokens = epic_chars / 4;

    // Маршрут Critical требует полного epic-context: ниже окна рубрики
    // (800 токенов) пакет не собирается — «реализация без доступа к
    // источникам» на пустом контексте означает архитектурные изобретения
    // исполнителя (разрыв P2: Fast-окно молча прошло бы и для Critical).
    if route == Route::Critical && epic_tokens < EPIC_CONTEXT_MIN_CHARS / 4 {
        return Err(HarnessError::Harness(format!(
            "epic-context ~{epic_tokens} токенов — ниже окна рубрики handoff_quality \
             ({}); для маршрута Critical пакет не собирается: передайте спеки через \
             `spec`/`--spec` (spine с AD-инвариантами, затронутые ADR, NFR) или \
             понизьте маршрут осознанно",
            EPIC_CONTEXT_MIN_CHARS / 4
        )));
    }

    // CONSTRAINTS.yaml — только при отсутствии (не затирать пользовательские
    // правила), если не передан `--refresh-constraints`.
    //
    // T-02: пакетная копия ПРИОРИТЕТНА для резолвера гейта. Заготовка из
    // одного правила, положенная в пакет поверх корневого реестра из
    // шестнадцати, молча переключала гейт на себя: «Правил: 1, PASS» вместо
    // прогона реальных правил проекта. Поэтому при существующем корневом
    // реестре в пакет идёт его КОПИЯ — то, что написал архитектор, и то, что
    // прочитает гейт, обязаны совпадать.
    let constraints_path = dir.join("CONSTRAINTS.yaml");
    let root_registry = repo.join(crate::control::ROOT_CONSTRAINTS_PATH);
    let mut constraints_action: Option<String> = None;
    if !constraints_path.exists() || refresh_constraints {
        let (text, action) = if root_registry.is_file() {
            let text = std::fs::read_to_string(&root_registry)
                .map_err(|e| HarnessError::io(&root_registry, e))?;
            (
                text,
                format!("копия корневого {}", crate::control::ROOT_CONSTRAINTS_PATH),
            )
        } else {
            (
                default_constraints(repo),
                "заготовка под стек (корневого реестра в репозитории нет)".to_string(),
            )
        };
        // Самовалидация генератора до записи: падение здесь — дефект шаблонов,
        // а не данных репозитория (лучше ошибка, чем битый файл исполнителю).
        validate_constraints_text(&text)?;
        std::fs::write(&constraints_path, text)
            .map_err(|e| HarnessError::io(&constraints_path, e))?;
        constraints_action = Some(action);
    }

    // SPEC.md — только при отсутствии (не затирать правки архитектора):
    // с переданными спеками — собирается ИЗ НИХ (полные тексты контрактов,
    // кейс 2026-09-01: исполнители спотыкались о пустой шаблон, когда
    // архитектор передал спеки через --spec); без спек — шаблон.
    // В обеих ветках генерации первая строка — баннер машинной компиляции:
    // исполнитель обязан отличать скомпилированную спеку от авторской
    // (находка живого эксперимента — компиляция принималась за авторскую).
    let spec_path = dir.join("SPEC.md");
    if !spec_path.exists() {
        let spec_body = if spec_files.is_empty() {
            SPEC_TEMPLATE.to_string()
        } else {
            render_spec_from_sources(&spec_files)
        };
        std::fs::write(&spec_path, format!("{SPEC_MACHINE_BANNER}\n{spec_body}"))
            .map_err(|e| HarnessError::io(&spec_path, e))?;
    }

    // ROLLBACK.yaml — машиночитаемый план отката для репетиции на гейте A4;
    // только при отсутствии: архитектор переписывает шаги под фактический
    // план эпика, повторная генерация его правки не затирает.
    let rollback_path = dir.join(crate::rehearsal::ROLLBACK_FILE);
    if !rollback_path.exists() {
        std::fs::write(
            &rollback_path,
            render_rollback_yaml(baseline.hash.as_deref()),
        )
        .map_err(|e| HarnessError::io(&rollback_path, e))?;
    }

    // RUBRIC.yaml — только при отсутствии; с change OpenSpec (F6) требования
    // change идут в рубрику критерием `openspec_change_requirements` (поверх
    // якорной `handoff_quality`, а без неё — рубрикой из одного критерия).
    // Существующая рубрика архитектора не затирается: требования не
    // вписываются, и это — предупреждение пакета, а не молчание.
    let rubric_path = dir.join("RUBRIC.yaml");
    if rubric_path.exists() {
        if let Some(change_id) = openspec_change.as_deref() {
            if !change_requirements.is_empty() {
                warnings.push(format!(
                    "RUBRIC.yaml пакета уже существует и не затирается — требования change \
                     '{change_id}' не вписаны; добавьте критерий openspec_change_requirements \
                     вручную по списку из ARCHITECTURE.md"
                ));
            }
        }
    } else {
        let anchor = cfg.paths.rubrics_dir().join("handoff_quality.yaml");
        match (&openspec_change, change_requirements.is_empty()) {
            (Some(change_id), false) => {
                let anchor_text = std::fs::read_to_string(&anchor).ok();
                let text = render_rubric_with_change(
                    anchor_text.as_deref(),
                    change_id,
                    &change_requirements,
                )?;
                std::fs::write(&rubric_path, text)
                    .map_err(|e| HarnessError::io(&rubric_path, e))?;
            }
            (Some(change_id), true) => {
                warnings.push(format!(
                    "change '{change_id}' не несёт требований (в specs/ нет SHALL/MUST) — \
                     в рубрику пакета ничего не добавлено"
                ));
                if anchor.is_file() {
                    std::fs::copy(&anchor, &rubric_path)
                        .map_err(|e| HarnessError::io(&rubric_path, e))?;
                }
            }
            _ => {
                if anchor.is_file() {
                    std::fs::copy(&anchor, &rubric_path)
                        .map_err(|e| HarnessError::io(&rubric_path, e))?;
                }
            }
        }
    }

    // adr/ — копии ADR-файлов; существующие копии не затираем.
    let mut adr_copies = Vec::new();
    for spec in &spec_files {
        if is_adr_file(spec) {
            let Some(name) = spec.file_name() else {
                continue;
            };
            let dest = adr_dir.join(name);
            if !dest.exists() {
                std::fs::copy(spec, &dest).map_err(|e| HarnessError::io(&dest, e))?;
            }
            adr_copies.push(dest);
        }
    }

    // A3: пины контрольной плоскости снимаются здесь — ПОСЛЕ записи всех
    // файлов пакета (TASK.md/SPEC.md/RUBRIC.yaml/ROLLBACK.yaml включены),
    // иначе пакет сам себе создаёт вечное расхождение первым же прогоном.
    let control_plane = crate::control_plane::collect(repo);

    // MANIFEST.json — всегда перезаписывается.
    let manifest = Manifest {
        created_at: Utc::now().to_rfc3339(),
        task,
        model: &cfg.default_model,
        sources: spec_files.iter().map(|p| p.display().to_string()).collect(),
        epic_context_chars: epic_chars,
        epic_context_tokens: epic_tokens,
        route: route.to_string(),
        recommended_timeout_secs: timeout,
        baseline_commit: baseline.hash.clone(),
        rollback_plan: &rollback_text,
        control_plane,
        openspec_change: openspec_change.as_deref(),
    };
    let manifest_path = dir.join("MANIFEST.json");
    let manifest_text = serde_json::to_string_pretty(&manifest)?;
    std::fs::write(&manifest_path, format!("{manifest_text}\n"))
        .map_err(|e| HarnessError::io(&manifest_path, e))?;

    let mut files = vec![task_path, arch_path, manifest_path];
    if constraints_path.exists() {
        files.push(constraints_path);
    }
    if spec_path.exists() {
        files.push(spec_path);
    }
    if rollback_path.exists() {
        files.push(rollback_path);
    }
    if rubric_path.exists() {
        files.push(rubric_path);
    }
    files.extend(adr_copies);

    Ok(HandoffPacket {
        dir,
        files,
        epic_context_tokens: epic_tokens,
        baseline: baseline.hash,
        git_initialized: baseline.initialized,
        git_dirty_tracked: baseline.dirty_tracked,
        recommended_timeout_secs: timeout,
        warnings,
        constraints_action,
        openspec_change,
        openspec_requirements: change_requirements.len(),
    })
}

/// Рендерит TASK.md: задача + критерии приёмки из QAS (при наличии) +
/// план отката + финализация (git-коммит) + контракт результата
/// (headless JSON-статус).
fn render_task_md(task: &str, rollback: &str, acceptance: Option<&str>) -> String {
    let mut s = String::with_capacity(task.len() + rollback.len() + 2000);
    s.push_str("# Задача для кодового харнесса\n\n");
    s.push_str(task.trim());
    s.push('\n');
    if let Some(acceptance) = acceptance {
        s.push('\n');
        s.push_str(acceptance.trim());
        s.push('\n');
    }
    s.push_str("\n## План отката\n\n");
    s.push_str(rollback.trim());
    s.push('\n');
    s.push_str("\n## Финализация (обязательно)\n\n");
    s.push_str("Результат забирается из git, поэтому перед финальным ответом зафиксируй работу коммитом:\n\n");
    s.push_str("```bash\ngit add -A -- . ':!.arch-handoff'\ngit commit -m \"<кратко: что реализовано>\"\ngit status --short   # пусто, кроме .arch-handoff/\n```\n\n");
    s.push_str(
        "- Коммитится код и тесты; служебный каталог `.arch-handoff/` в коммит не входит.\n",
    );
    s.push_str("- Работа без коммита считается невыполненной: оркестратор увидит её только через git log.\n");
    s.push_str("\n## Контракт результата\n\n");
    s.push_str("Финальный ответ обязан завершаться JSON-объектом (после него — ни символа):\n\n");
    s.push_str("```json\n{\"status\": \"complete|partial|blocked\", \"assumptions\": [], \"open_questions\": [], \"conflicts_with_prior_decisions\": []}\n```\n\n");
    s.push_str("- `status`: `complete` — выполнено полностью; `partial` — частично; `blocked` — заблокировано.\n");
    s.push_str("- `assumptions`: допущения, принятые при реализации.\n");
    s.push_str("- `open_questions`: вопросы к архитектору.\n");
    s.push_str(
        "- `conflicts_with_prior_decisions`: расхождения с принятыми ранее решениями (ADR, spine).\n\n",
    );
    s.push_str("Архитектурный контекст — `ARCHITECTURE.md`, ограничения — `CONSTRAINTS.yaml`, рубрика приёмки — `RUBRIC.yaml` (при наличии).\n\n");
    s.push_str("## Чеклист перед финальным ответом\n\n");
    s.push_str("- [ ] `SPEC.md` (контракты интерфейсов: входы/выходы, структуры данных, границы ошибок, критерии верификации) заполнен архитектором — сверь реализацию с ним; расхождения фиксируй в `conflicts_with_prior_decisions`, а не молчаливым отступлением.\n");
    s
}

/// Минимум REQ-сущностей модели, с которого проверяется декомпозиция
/// REQ → задачи TASK.md (мелкие эпики не обязаны дробиться в список).
const REQ_DECOMP_MIN_REQS: usize = 3;

/// Порог предупреждения о тонкой декомпозиции: REQ-сущностей больше, чем
/// в [`REQ_DECOMP_RATIO`] раз, числа пунктов задач в TASK.md.
const REQ_DECOMP_RATIO: usize = 2;

/// Предупреждение о тонкой декомпозиции REQ → задачи (детерминированное):
/// если модель репозитория несёт [`REQ_DECOMP_MIN_REQS`]+ REQ-сущностей, а
/// рабочая область TASK.md (формулировка задачи + критерии приёмки QAS — всё
/// до раздела «План отката»; служебные секции шаблона не считаются) содержит
/// существенно меньше пунктов списка (REQ > [`REQ_DECOMP_RATIO`]× задач),
/// декомпозиция выглядит неполной (находка живого эксперимента: исполнитель
/// получал TASK.md, чей список задач недопокрывал REQ-множество).
///
/// `None` — нет model/, нет REQ-* или декомпозиция достаточная.
fn req_decomposition_warning(repo: &Path, task_md: &str) -> Option<String> {
    let model_dir = repo.join("model");
    if !model_dir.is_dir() {
        return None;
    }
    // Ошибку разбора модели здесь безопасно игнорировать: битая модель уже
    // упала в qas_acceptance_section выше по generate_handoff (ошибка, а не
    // молчаливый пропуск) — до этой точки исполнение просто не доходит.
    let model = load_model(&model_dir).ok()?;
    let reqs = model
        .entities
        .iter()
        .filter(|e| e.kind == EntityKind::Req)
        .count();
    if reqs < REQ_DECOMP_MIN_REQS {
        return None;
    }
    let work_region = task_md.split("\n## План отката").next().unwrap_or(task_md);
    let tasks = count_task_items(work_region);
    if reqs > REQ_DECOMP_RATIO * tasks {
        Some(format!(
            "в модели {reqs} REQ-сущностей, а в TASK.md — {tasks} пункт(ов) задач: \
             декомпозиция REQ → задачи выглядит неполной (порог REQ > {REQ_DECOMP_RATIO}× задач). \
             Как закрыть: добавьте в TASK.md по пункту на каждую непокрытую REQ \
             (`## Задачи` — список `- [ ] …`), либо пометьте REQ как отложенные \
             в самой сущности (`unverifiable: \"отложено до …\"`), либо снизьте \
             детализацию модели, если часть REQ — надзадача для остальных."
        ))
    } else {
        None
    }
}

/// Число пунктов списка/чекбоксов в markdown-тексте вне кодовых блоков:
/// маркированные `- `/`* ` (включая чекбоксы `- [ ]`) и нумерованные `1. `/`1) `.
fn count_task_items(markdown: &str) -> usize {
    let mut in_fence = false;
    let mut count = 0;
    for line in markdown.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        let is_bullet = trimmed.starts_with("- ") || trimmed.starts_with("* ");
        let is_ordered = trimmed.find(['.', ')']).is_some_and(|pos| {
            pos > 0
                && pos <= 3
                && trimmed[..pos].bytes().all(|b| b.is_ascii_digit())
                && trimmed[pos + 1..].starts_with(' ')
        });
        if is_bullet || is_ordered {
            count += 1;
        }
    }
    count
}

/// Секция «Критерии приёмки» из QAS-сущностей модели репозитория (ADR-007).
///
/// `None` — каталога `<repo>/model/` нет или в нём нет `QAS-*`; сценарии
/// рендерятся в порядке модели (детерминированном), незаполненное поле
/// помечается `—`.
///
/// # Errors
/// Каталог `model/` есть, но модель не разбирается: молчаливый пропуск
/// превратил бы «критерии попадают автоматически» в «иногда попадают».
fn qas_acceptance_section(repo: &Path) -> Result<Option<String>> {
    let model_dir = repo.join("model");
    if !model_dir.is_dir() {
        return Ok(None);
    }
    let model = load_model(&model_dir).map_err(|e| {
        HarnessError::Model(format!(
            "{}: модель для QAS-критериев приёмки не разбирается: {e}",
            model_dir.display()
        ))
    })?;
    let scenarios: Vec<&crate::model::Entity> = model
        .entities
        .iter()
        .filter(|e| e.kind == EntityKind::Qas)
        .collect();
    if scenarios.is_empty() {
        return Ok(None);
    }
    let mut s = String::new();
    s.push_str("## Критерии приёмки (QAS из модели)\n\n");
    s.push_str("Сценарии атрибутов качества из `model/` — обязательная часть приёмки:\n\n");
    for q in scenarios {
        let field = |v: &Option<String>| {
            v.as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .unwrap_or("—")
                .to_string()
        };
        let _ = writeln!(
            s,
            "- **{}** ({}): при {} от «{}» к «{}» → {}. Мера: {}.",
            q.id,
            q.title,
            field(&q.stimulus),
            field(&q.source),
            field(&q.artifact),
            field(&q.response),
            field(&q.measure)
        );
    }
    Ok(Some(s))
}

/// План отката по умолчанию (рубрика `handoff_quality::rollback_plan` требует
/// шаги, сигналы-триггеры и владельца решения): точка отката — baseline-коммит,
/// созданный предгейтом [`ensure_git_baseline`].
fn default_rollback(baseline: Option<&str>) -> String {
    let anchor = match baseline {
        Some(h) => format!(
            "Откат: `git reset --hard {h}` (baseline — последний коммит до работы исполнителя; вся его работа приходит одним коммитом поверх).\n"
        ),
        None => "Откат: удалить коммит(ы) исполнителя (`git log` → `git reset --hard <до-исполнителя>`); если репозиторий не под git — удалить созданные за прогон файлы.\n".into(),
    };
    format!(
        "{anchor}\
         Сигналы отката: провал fitness-гейта (`arch-be control check`), непустой \
         `conflicts_with_prior_decisions`, статус `blocked`.\n\
         Владелец решения об откате — solution-архитектор; исполнитель откат не \
         выполняет и не маскирует проблему обходным редизайном.\n\
         Обратимость: полная — единая точка изменений, коммит исполнителя."
    )
}

/// Рендерит `ROLLBACK.yaml` — машиночитаемый план отката для репетиции на
/// гейте A4 ([`crate::rehearsal`]). Дефолт соответствует [`default_rollback`]:
/// проверка якоря + откат на baseline + verify чистоты дерева. Без git-якоря —
/// пустой план с комментарием (репетиция такого пакета падает с диагностикой).
fn render_rollback_yaml(baseline: Option<&str>) -> String {
    let header = "# План отката handoff-пакета (машиночитаемый) — репетируется на гейте A4:\n\
                  # `arch control gate A4 <repo> --rehearse`. Файл НЕ затирается повторной\n\
                  # генерацией: перед передачей перепишите шаги под фактический план отката\n\
                  # эпика (должен соответствовать разделу «План отката» TASK.md). Шаги с\n\
                  # внешними/деструктивными эффектами репетиция отклоняет — docs/control.md.\n";
    match baseline {
        Some(h) => format!(
            "{header}\
             baseline_commit: \"{h}\"\n\
             steps:\n\
             \x20 - name: якорь-доступен\n\
             \x20   run: git cat-file -t {h}\n\
             \x20 - name: откат-на-baseline\n\
             \x20   run: git reset --hard {h}\n\
             verify: test -z \"$(git status --porcelain --untracked-files=no)\"\n"
        ),
        None => format!(
            "{header}\
             # git недоступен на момент генерации — якоря нет; впишите baseline_commit\n\
             # и шаги вручную, иначе репетиция на A4 упадёт с диагностикой.\n\
             baseline_commit: \"\"\n\
             steps: []\n"
        ),
    }
}

/// Рекомендованный таймаут прогона по маршруту значимости: Critical-эпик
/// (walking skeleton из нескольких модулей) в 30-минутный дефолт адаптера
/// не влезает — прогон обрывался посередине.
fn recommended_timeout(route: Route) -> u64 {
    match route {
        Route::Fast => 1800,
        Route::Standard => 3600,
        Route::Critical => 7200,
    }
}

/// Итог предгейта git: якорь отката и факт инициализации репозитория.
#[derive(Debug, Clone, Default)]
struct GitBaseline {
    /// Короткий хеш baseline-коммита (HEAD на момент генерации пакета).
    hash: Option<String>,
    /// Репозиторий был создан этим вызовом (`git init`).
    initialized: bool,
    /// Есть незакоммиченные изменения ОТСЛЕЖИВАЕМЫХ файлов: откат на
    /// baseline (`reset --hard`) их потеряет (untracked он не трогает).
    dirty_tracked: bool,
}

/// Предгейт handoff: гарантирует git-репозиторий и baseline-коммит-якорь.
///
/// Без git контракт «финальный коммит» невыполним, авто-коммит прогона не
/// работает, а откату не за что зацепиться — поэтому репозиторий
/// инициализируется (`git init`), а при отсутствии коммитов создаётся пустой
/// baseline (`--allow-empty`, идентичность spine-harness). Содержимое каталога
/// в baseline НЕ добавляется осознанно: это дело исполнителя/пользователя.
/// Git недоступен — пакет всё равно собирается, просто без якоря.
fn ensure_git_baseline(repo: &Path) -> GitBaseline {
    let mut initialized = false;
    if git_out(repo, &["rev-parse", "--git-dir"]).is_none() {
        if git_out(repo, &["init", "-q"]).is_none() {
            return GitBaseline::default();
        }
        initialized = true;
    }
    if let Some(head) = git_out(repo, &["rev-parse", "--short", "HEAD"]) {
        let dirty = git_out(repo, &["status", "--porcelain", "--untracked-files=no"])
            .is_some_and(|s| !s.trim().is_empty());
        return GitBaseline {
            hash: Some(head.trim().to_string()),
            initialized,
            dirty_tracked: dirty,
        };
    }
    // Репозиторий без единого коммита — создаём пустой якорь отката.
    let commit = git_out(
        repo,
        &[
            "-c",
            "user.name=spine-harness",
            "-c",
            "user.email=spine-harness@localhost",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "baseline: якорь отката handoff",
        ],
    );
    let hash = commit.and_then(|_| {
        git_out(repo, &["rev-parse", "--short", "HEAD"]).map(|h| h.trim().to_string())
    });
    GitBaseline {
        hash,
        initialized,
        dirty_tracked: false,
    }
}

/// Читает рекомендованный таймаут прогона из MANIFEST.json пакета
/// (None — пакета нет или манифест старый, без поля).
#[must_use]
pub fn recommended_timeout_secs(repo: &Path) -> Option<u64> {
    #[derive(Deserialize)]
    struct ManifestMeta {
        #[serde(default)]
        recommended_timeout_secs: Option<u64>,
    }
    let text = std::fs::read_to_string(repo.join(HANDOFF_DIR).join("MANIFEST.json")).ok()?;
    serde_json::from_str::<ManifestMeta>(&text)
        .ok()?
        .recommended_timeout_secs
}

/// Компилирует epic-context из спецификаций: заголовок с датой и источниками,
/// далее — сжатые рендеры спек; итог удерживается в [`EPIC_CONTEXT_MAX_CHARS`].
///
/// Глубина адаптивная: прочие секции рендерятся по [`DEPTH_SHALLOW`] абзацев,
/// но если контекст недобирает до [`EPIC_CONTEXT_MIN_CHARS`] (низ окна рубрики
/// `handoff_quality`, ~800 токенов), спеки перерендериваются глубже
/// ([`DEPTH_DEEP`]) — «реализация без доступа к источникам» требует массы.
///
/// При переполнении работает лесенка деградации (дефект A2 живого эксперимента:
/// тупое усечение по символам обрезало инвариант AD-010 на полуслове):
/// DEEP → SHALLOW (если мелкий рендер в окне рубрики) → прозаические секции
/// с хвоста сокращаются до заголовков → прозаические секции с хвоста
/// выкидываются целиком. **ADR-блоки spine не режутся никогда** — ценой
/// превышения лимита, если документ состоит из одних инвариантов. ADR-блоком
/// считается секция с полями Binds/Prevents/Rule в любой markdown-форме
/// (`Binds:`, `- Binds:`, `**Binds**:`, `- **Binds**:` — см.
/// [`ADR_FIELD_PATTERN`]) или с заголовком `AD-<n>`/`ADR-<n>` (подстраховка
/// полевого детектора, дефект D1 живого отчёта: спайн кейса digital-ruble
/// пишет поля как `- **Binds**:`, и инварианты резались как проза). Сноска об
/// усечении перечисляет сокращённые и выкинутые секции поимённо и проверяет
/// факт перед утверждением о дословности инвариантов (см. [`cut_ad_sections`]).
///
/// # Errors
/// Спека не читается.
fn compile_epic_context(spec_files: &[PathBuf]) -> Result<String> {
    let mut depth = ProseDepth::Paragraphs(DEPTH_SHALLOW);
    let mut render = render_epic_structured(spec_files, depth)?;
    if render.chars_len() < EPIC_CONTEXT_MIN_CHARS {
        depth = ProseDepth::Paragraphs(DEPTH_DEEP);
        render = render_epic_structured(spec_files, depth)?;
    }
    if render.chars_len() <= EPIC_CONTEXT_MAX_CHARS {
        return Ok(render.assemble(None));
    }
    // Шаг 1 лесенки: глубокий рендер (недобор до окна рубрики) переполнен —
    // пробуем умолчательный мелкий. Если он в окне [MIN, MAX], это дефолтная
    // глубина без всякого усечения — сноски не нужно. Если мелкий тоже
    // переполнен, деградируем его (он компактнее); если недобирает до окна,
    // остаёмся на глубоком и деградируем его (иначе контекст провалится под
    // окно рубрики и убьёт маршрут Critical).
    if depth == ProseDepth::Paragraphs(DEPTH_DEEP) {
        let shallow = render_epic_structured(spec_files, ProseDepth::Paragraphs(DEPTH_SHALLOW))?;
        if shallow.chars_len() >= EPIC_CONTEXT_MIN_CHARS
            && shallow.chars_len() <= EPIC_CONTEXT_MAX_CHARS
        {
            return Ok(shallow.assemble(None));
        }
        if shallow.chars_len() > EPIC_CONTEXT_MAX_CHARS {
            render = shallow;
        }
    }
    // Шаг 2: прозаические секции с хвоста сокращаются до заголовков
    // (ADR-блоки пропускаются и остаются дословными).
    let mut dropped: Vec<String> = Vec::new();
    for pos in (0..render.sections.len()).rev() {
        let notice = truncation_notice(&render, &dropped);
        if render.chars_len() + notice.chars().count() <= EPIC_CONTEXT_MAX_CHARS {
            break;
        }
        let section = &mut render.sections[pos];
        if !section.is_adr && !section.body.is_empty() {
            section.body.clear();
            section.shortened_to_heading = true;
        }
    }
    // Шаг 3: всё ещё переполнение — выкидываем прозаические секции с хвоста
    // целиком. ADR-блоки не выкидываются никогда: если остались только они,
    // документ уходит за лимит дословным (честнее, чем инвариант на полуслове).
    loop {
        let notice = truncation_notice(&render, &dropped);
        if render.chars_len() + notice.chars().count() <= EPIC_CONTEXT_MAX_CHARS {
            break;
        }
        let Some(pos) = render.sections.iter().rposition(|s| !s.is_adr) else {
            break;
        };
        dropped.push(render.sections.remove(pos).title);
    }
    let notice = truncation_notice(&render, &dropped);
    Ok(render.assemble(Some(&notice)))
}

/// Режим рендера прозаических (не-ADR) секций epic-context: заголовок +
/// первые N абзацев тела секции. ADR-блоки spine рендерятся целиком в любом
/// режиме — они не сокращаются никогда (сокращение прозы до заголовков делает
/// шаг 2 лесенки в [`compile_epic_context`], очищая тела секций, а не
/// перерендером).
#[derive(Clone, Copy, PartialEq, Eq)]
enum ProseDepth {
    /// Заголовок + первые N абзацев тела секции.
    Paragraphs(usize),
}

/// Секция epic-context в структурном рендере (для лесенки деградации).
struct EpicSection {
    /// Название секции для сноски об усечении (заголовок без маркеров `#`;
    /// у преамбулы — «вводная часть»).
    title: String,
    /// Заголовок секции (у преамбулы — пусто); сюда же вклеивается маркер
    /// `<!-- источник: ... -->` первой секции спеки.
    heading: String,
    /// Тело секции на текущей глубине (у ADR-блоков — всегда дословное;
    /// обрезается шагом 2 лесенки).
    body: String,
    /// ADR-блок spine: не сокращается и не выкидывается никогда.
    is_adr: bool,
    /// Тело сокращено до заголовка шагом 2 лесенки — секция попадает в
    /// сноску об усечении (пока не выкинута шагом 3).
    shortened_to_heading: bool,
}

impl EpicSection {
    /// Длина секции в собранном документе (символов, без разделителя).
    fn chars_len(&self) -> usize {
        let mut n = self.heading.chars().count();
        if !self.body.is_empty() {
            n += 2 + self.body.chars().count();
        }
        n
    }

    /// Дописывает секцию в документ.
    fn render_into(&self, out: &mut String) {
        out.push_str(&self.heading);
        if !self.body.is_empty() {
            out.push_str("\n\n");
            out.push_str(&self.body);
        }
    }
}

/// Структурный рендер epic-context: неизменная шапка + секции по порядку.
struct EpicRender {
    /// Шапка: заголовок, дата сборки, список источников (не усечается).
    header: String,
    /// Секции всех спек в порядке обхода файлов.
    sections: Vec<EpicSection>,
}

impl EpicRender {
    /// Длина собранного документа без сноски об усечении (символов).
    fn chars_len(&self) -> usize {
        self.header.chars().count()
            + self
                .sections
                .iter()
                .map(|s| s.chars_len() + 2)
                .sum::<usize>()
    }

    /// Собирает документ: шапка + секции через пустую строку + сноска (если есть).
    fn assemble(&self, notice: Option<&str>) -> String {
        let mut out = self.header.clone();
        for s in &self.sections {
            s.render_into(&mut out);
            out.push_str("\n\n");
        }
        if let Some(notice) = notice {
            out.push_str(notice);
        }
        out
    }
}

/// Структурный рендер epic-context на заданной глубине прозаических секций
/// (ADR-блоки spine всегда целиком).
fn render_epic_structured(spec_files: &[PathBuf], depth: ProseDepth) -> Result<EpicRender> {
    // Полевой regex ADR-блоков компилируется один раз на рендер и раздаётся
    // разбору каждой спеки.
    let adr_re = epic_re(ADR_FIELD_PATTERN)?;
    let mut header = String::with_capacity(512);
    header.push_str("# Архитектурный контекст (epic-context)\n\n");
    let _ = write!(header, "Собран: {}\n\n", Utc::now().to_rfc3339());
    header.push_str("Источники:\n");
    for f in spec_files {
        let _ = writeln!(header, "- {}", f.display());
    }
    header.push('\n');
    let mut sections = Vec::new();
    for f in spec_files {
        let text = std::fs::read_to_string(f).map_err(|e| HarnessError::io(f, e))?;
        let marker = format!("<!-- источник: {} -->", f.display());
        let mut spec_sections = render_spec_structured(&text, depth, &adr_re);
        if let Some(first) = spec_sections.first_mut() {
            // Маркер источника привязывается к первой секции спеки.
            first.heading = format!("{marker}\n\n{}", first.heading);
        } else {
            sections.push(EpicSection {
                title: format!("источник {}", f.display()),
                heading: marker,
                body: String::new(),
                is_adr: false,
                shortened_to_heading: false,
            });
        }
        sections.extend(spec_sections);
    }
    Ok(EpicRender { header, sections })
}

/// Разбирает одну спецификацию в секции epic-context: преамбула (если есть) +
/// секции по markdown-заголовкам. Секции с полями Binds/Prevents/Rule (в любой
/// markdown-форме, см. [`ADR_FIELD_PATTERN`]) или с заголовком `AD-<n>`/
/// `ADR-<n>` помечаются `is_adr` и рендерятся дословно; проза — по глубине
/// `depth`.
fn render_spec_structured(text: &str, depth: ProseDepth, adr_re: &Regex) -> Vec<EpicSection> {
    let mut preamble = String::new();
    let mut raw_sections: Vec<(String, String)> = Vec::new();
    let mut cur: Option<(String, String)> = None;
    let mut in_fence = false;
    for line in text.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_fence = !in_fence;
        }
        if !in_fence && line.starts_with('#') {
            if let Some(s) = cur.take() {
                raw_sections.push(s);
            }
            cur = Some((line.trim_end().to_string(), String::new()));
        } else if let Some((_, body)) = cur.as_mut() {
            body.push_str(line);
            body.push('\n');
        } else {
            preamble.push_str(line);
            preamble.push('\n');
        }
    }
    if let Some(s) = cur.take() {
        raw_sections.push(s);
    }

    let mut sections = Vec::new();
    if !preamble.trim().is_empty() {
        let is_adr = is_adr_block(adr_re, &preamble);
        sections.push(EpicSection {
            title: "вводная часть".to_string(),
            heading: String::new(),
            body: render_section_body(&preamble, depth, is_adr),
            is_adr,
            shortened_to_heading: false,
        });
    }
    for (heading, body) in &raw_sections {
        let title = heading.trim_start_matches('#').trim().to_string();
        // Классификация ADR-блока: поля Binds/Prevents/Rule в теле ИЛИ
        // заголовок AD/ADR-<n> — подстраховка на случай формы полей, которую
        // полевой regex не узнал: тело инварианта не должно резаться из-за
        // орфографии полей (дефект D1 — потерянные AD-008…AD-010 кейса).
        let is_adr = is_adr_block(adr_re, body) || is_ad_title(&title);
        sections.push(EpicSection {
            title,
            heading: heading.clone(),
            body: render_section_body(body, depth, is_adr),
            is_adr,
            shortened_to_heading: false,
        });
    }
    sections
}

/// Тело секции на заданной глубине: ADR-блок — дословно, проза — первые N
/// абзацев.
fn render_section_body(body: &str, depth: ProseDepth, is_adr: bool) -> String {
    if is_adr {
        return body.trim().to_string();
    }
    match depth {
        ProseDepth::Paragraphs(n) => first_paragraphs(body, n),
    }
}

/// Бюджет перечня секций в сноске об усечении (символов): длинный список
/// заменяется компактной формой «первые, …, последняя (всего N)» — сноска
/// не должна сама съедать лимит epic-context.
const NOTICE_LIST_MAX_CHARS: usize = 240;

/// Сколько первых имён секций показывается в компактной форме перечня сноски.
const NOTICE_LIST_HEAD: usize = 3;

/// Компактное перечисление секций в сноске об усечении: полный список, а при
/// превышении [`NOTICE_LIST_MAX_CHARS`] — первые [`NOTICE_LIST_HEAD`] и
/// последняя секция + счётчик.
fn compact_section_list(names: &[String]) -> String {
    let joined = names.join(", ");
    if joined.chars().count() <= NOTICE_LIST_MAX_CHARS || names.len() <= NOTICE_LIST_HEAD + 1 {
        return joined;
    }
    format!(
        "{}, …, {} (всего {})",
        names[..NOTICE_LIST_HEAD].join(", "),
        names[names.len() - 1],
        names.len()
    )
}

/// Сноска об усечении epic-context: честно перечисляет, какие секции
/// сокращены до заголовков и какие выкинуты с хвоста; инварианты spine
/// подчёркнуто дословны — но только после проверки факта: если AD/ADR-секция
/// (по заголовку) осталась без тела или выкинута, сноска называет её, а не
/// декларирует дословность (дефект D1 живого отчёта: сноска писала «приведены
/// дословно и не сокращались» про AD-008…AD-010, вырезанные до заголовков).
/// Маркер «Контекст усечён» сохраняется для потребителей (рубрики,
/// регрессионные проверки).
fn truncation_notice(render: &EpicRender, dropped: &[String]) -> String {
    let shortened: Vec<String> = render
        .sections
        .iter()
        .filter(|s| s.shortened_to_heading)
        .map(|s| s.title.clone())
        .collect();
    let mut notice = String::from("\n\n> **Контекст усечён**");
    let mut parts: Vec<String> = Vec::new();
    if !shortened.is_empty() {
        parts.push(format!(
            "прозаические секции сокращены до заголовков: {}",
            compact_section_list(&shortened)
        ));
    }
    if !dropped.is_empty() {
        parts.push(format!(
            "выкинуты прозаические секции с хвоста: {}",
            compact_section_list(dropped)
        ));
    }
    if parts.is_empty() {
        notice
            .push_str(" — лимит превышен документом из инвариантов spine, которые не сокращаются");
    } else {
        let _ = write!(notice, ": {}", parts.join("; "));
    }
    let ad_cut = cut_ad_sections(render, dropped);
    if ad_cut.is_empty() {
        notice.push_str(
            ". Инварианты spine (AD-блоки) приведены дословно и не сокращались; полные тексты — \
             в файлах-источниках (см. MANIFEST.json).\n",
        );
    } else {
        let _ = writeln!(
            notice,
            ". ВНИМАНИЕ: секции инвариантов без текста (урезаны лесенкой или пусты в источнике): \
             {} — дословность AD-блоков НЕ гарантируется; полные тексты — в файлах-источниках \
             (см. MANIFEST.json).",
            compact_section_list(&ad_cut)
        );
    }
    notice
}

/// AD/ADR-секции (по заголовку, см. [`is_ad_title`]), оставшиеся в выходе без
/// тела (урезаны лесенкой или пусты в источнике), плюс выкинутые AD-секции.
/// Проверка факта для сноски об усечении: непустой список запрещает сноске
/// утверждать дословность инвариантов (дефект D1).
fn cut_ad_sections(render: &EpicRender, dropped: &[String]) -> Vec<String> {
    let mut cut: Vec<String> = render
        .sections
        .iter()
        .filter(|s| is_ad_title(&s.title) && s.body.trim().is_empty())
        .map(|s| s.title.clone())
        .collect();
    cut.extend(dropped.iter().filter(|t| is_ad_title(t)).cloned());
    cut
}

/// Regex полей ADR-блока spine (Binds/Prevents/Rule), толерантный к
/// markdown-формам: `Binds:`, `- Binds:`, `**Binds**:`, `- **Binds**:`,
/// `**Binds:**`. Семантика повторяет полевой regex штатного линтера spine
/// (`control.rs::lint_spine` — `re_field` = `\b(Binds|Prevents|Rule)\*{0,2}\s*:`,
/// там же извлекается значение поля; здесь нужен только факт наличия).
/// Держать синхронно с линтером, чтобы форматы не расходились в третий раз:
/// дефект D1 живого отчёта — спайн кейса digital-ruble пишет поля как
/// `- **Binds**:`, подстроки `Binds:` там нет, и AD-блоки классифицировались
/// прозой и резались лесенкой до заголовков.
const ADR_FIELD_PATTERN: &str = r"\b(?:Binds|Prevents|Rule)\*{0,2}\s*:";

/// Компилирует статический regex epic-context; сбой компиляции — доменная
/// ошибка, не паника (конвенция `control.rs::spine_regex`).
fn epic_re(pattern: &str) -> Result<Regex> {
    Regex::new(pattern).map_err(|e| HarnessError::Harness(format!("внутренний regex handoff: {e}")))
}

/// Признак ADR-блока spine: секция содержит поля Binds/Prevents/Rule в любой
/// markdown-форме (см. [`ADR_FIELD_PATTERN`]).
fn is_adr_block(adr_re: &Regex, body: &str) -> bool {
    adr_re.is_match(body)
}

/// Признак заголовка AD/ADR-секции (`AD-8 …`, `ADR-012 …` — сразу после
/// префикса идёт номер). Подстраховка полевого детектора [`is_adr_block`]:
/// секция с таким заголовком считается инвариантом, даже если её тело
/// записано в форме, которую полевой regex не узнал, — тело AD-блока не
/// должно резаться из-за орфографии полей (так спайн кейса digital-ruble
/// потерял AD-008…AD-010, дефект D1). Проверка строковая (без regex), чтобы
/// вызываться и из сноски об усечении без обработки ошибок компиляции.
fn is_ad_title(title: &str) -> bool {
    let t = title.trim_start();
    for prefix in ["AD-", "ADR-"] {
        if let Some(rest) = t.strip_prefix(prefix) {
            return rest.chars().next().is_some_and(|c| c.is_ascii_digit());
        }
    }
    false
}

/// Первые `n` абзацев текста (абзацы разделены пустыми строками).
fn first_paragraphs(text: &str, n: usize) -> String {
    let mut paras: Vec<String> = Vec::new();
    let mut cur: Vec<&str> = Vec::new();
    for line in text.lines() {
        if line.trim().is_empty() {
            if !cur.is_empty() {
                paras.push(cur.join("\n"));
                cur.clear();
                if paras.len() >= n {
                    break;
                }
            }
        } else {
            cur.push(line);
        }
    }
    if paras.len() < n && !cur.is_empty() {
        paras.push(cur.join("\n"));
    }
    paras.join("\n\n")
}

/// Признак ADR-файла: md, чьё имя содержит `ADR` или путь содержит `/adr/`.
fn is_adr_file(path: &Path) -> bool {
    if !path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("md"))
    {
        return false;
    }
    let name_hit = path
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.contains("ADR"));
    let path_hit = path.to_string_lossy().contains("/adr/");
    name_hit || path_hit
}

/// Выполняет git-команду в репозитории; None — команда упала или stderr.
/// Общий хелпер с `crate::harness` (авто-коммит прогона): генерации пакета
/// нужен baseline-предгейт, прогону — фиксация хвоста исполнителя.
pub(crate) fn git_out(repo: &Path, args: &[&str]) -> Option<String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Инструмент `handoff_create`: генерация handoff-пакета из агентного цикла
/// и моста MCP (в отличие от `harness_run` доступен и в core-сборке —
/// создание пакета чисто файловое, без сети и внешних харнессов).
pub struct HandoffCreateTool {
    /// Конфигурация (пути к рубрикам, модель по умолчанию).
    cfg: Config,
}

impl HandoffCreateTool {
    /// Инструмент поверх конфигурации харнесса.
    #[must_use]
    pub fn new(cfg: Config) -> Self {
        Self { cfg }
    }
}

#[async_trait]
impl Tool for HandoffCreateTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "handoff_create".into(),
            description: "Сгенерировать handoff-пакет (.arch-handoff/: TASK.md, ARCHITECTURE.md, CONSTRAINTS.yaml, SPEC.md — шаблон верифицируемых контрактов интерфейсов, MANIFEST.json, adr/) для передачи задачи кодовому харнессу. Предгейт: гарантирует git-репозиторий и baseline-коммит (якорь отката); TASK.md включает план отката и требование финального git-коммита; MANIFEST несёт рекомендованный таймаут прогона по маршруту значимости (подхватывает harness_run)".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Корень репозитория (относительно cwd или абсолютный); историческое имя `repo` принимается. T-02: при существующем корневом CONSTRAINTS.yaml пакетный реестр — его КОПИЯ, а не заготовка: гейт читает пакетную копию первой, и расхождение копий он называет находкой registry_diverged"},
                    "task": {"type": "string", "description": "Формулировка задачи для кодового харнесса"},
                    "spec": {"type": "array", "items": {"type": "string"}, "description": "Пути к спецификациям/ADR (md), опционально"},
                    "rollback": {"type": "string", "description": "Явный план отката (шаги, сигналы, владелец решения); по умолчанию — откат на baseline-коммит"},
                    "route": {"type": "string", "enum": ["fast", "standard", "critical"], "description": "Маршрут значимости из significance_score: задаёт рекомендованный таймаут прогона (fast=1800с, standard=3600с, critical=7200с); по умолчанию standard"},
                    "refresh_constraints": {"type": "boolean", "description": "Перезаписать существующий пакетный CONSTRAINTS.yaml (T-02): без флага файл пакета не трогается — правки архитектора сохраняются"},
                    "openspec_change": {"type": "string", "description": "Change OpenSpec как источник пакета (F6): proposal.md/design.md/tasks.md и дельты спек openspec/changes/<id>/ кладутся в пакет как spec, требования change — критерием openspec_change_requirements в RUBRIC.yaml; порог контекста Critical считается с их учётом"}
                },
                "required": ["task"]
            }),
        }
    }

    async fn call(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput> {
        // Каноничное имя пути — `path` (Н8); `repo` держится синонимом, чтобы
        // старые клиенты и скрипты продолжали работать (T-02: раньше схема
        // объявляла `path`, а инструмент требовал `repo` — вызов по
        // объявленному имени падал).
        let Some(repo) = args
            .get("path")
            .or_else(|| args.get("repo"))
            .and_then(Value::as_str)
        else {
            return Ok(ToolOutput::err(
                "handoff_create: обязательный аргумент 'path' (string; историческое имя 'repo') отсутствует",
            ));
        };
        let Some(task) = args.get("task").and_then(Value::as_str) else {
            return Ok(ToolOutput::err(
                "handoff_create: обязательный аргумент 'task' (string) отсутствует",
            ));
        };
        let spec: Vec<PathBuf> = args
            .get("spec")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(|s| ctx.resolve(s))
                    .collect()
            })
            .unwrap_or_default();
        let repo = ctx.resolve(repo);
        let rollback = args.get("rollback").and_then(Value::as_str);
        let route = match args.get("route").and_then(Value::as_str) {
            Some(r) => match r.parse::<Route>() {
                Ok(route) => route,
                Err(e) => return Ok(ToolOutput::err(format!("handoff_create: {e}"))),
            },
            None => Route::Standard,
        };
        let opts = HandoffOptions {
            refresh_constraints: args
                .get("refresh_constraints")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            // F6: change OpenSpec как источник пакета (ADR-067).
            openspec_change: args
                .get("openspec_change")
                .and_then(Value::as_str)
                .map(str::to_string),
        };
        match generate_handoff_opts(&repo, task, &spec, &self.cfg, rollback, route, opts) {
            Ok(packet) => {
                let files = packet
                    .files
                    .iter()
                    .map(|f| format!("- {}", f.display()))
                    .collect::<Vec<_>>()
                    .join("\n");
                let mut out = format!(
                    "Handoff-пакет создан: {}\nEpic-context: ~{} токенов.\nФайлы:\n{files}",
                    packet.dir.display(),
                    packet.epic_context_tokens
                );
                // Предгейт git: якорь отката и факт инициализации.
                match &packet.baseline {
                    Some(h) if packet.git_initialized => {
                        let _ = write!(
                            out,
                            "\nGit: репозиторий инициализирован, baseline-коммит {h} (якорь отката в TASK.md)."
                        );
                    }
                    Some(h) => {
                        let _ = write!(out, "\nGit: baseline-коммит {h} (якорь отката в TASK.md).");
                    }
                    None => {
                        out.push_str(
                            "\nВНИМАНИЕ: git недоступен — якоря отката нет; контракт \
                             финального коммита и авто-коммит прогона работать не будут.",
                        );
                    }
                }
                let _ = write!(
                    out,
                    "\nМаршрут: {route} → рекомендованный timeout_secs={} \
                     (harness_run подхватит из MANIFEST.json, если не задан явно).",
                    packet.recommended_timeout_secs
                );
                // F6: источник пакета — change OpenSpec (требования — в рубрике).
                if let Some(change) = &packet.openspec_change {
                    let _ = write!(
                        out,
                        "\nOpenSpec change: {change} (требований в рубрике пакета: {}).",
                        packet.openspec_requirements
                    );
                }
                if packet.git_dirty_tracked {
                    out.push_str(
                        "\nВНИМАНИЕ: есть незакоммиченные изменения отслеживаемых файлов — \
                         откат на baseline (`git reset --hard`) их потеряет: закоммитьте \
                         заранее или осознанно включите в задачу.",
                    );
                }
                // Окно рубрики handoff_quality — 800–1500 токенов.
                if packet.epic_context_tokens < EPIC_CONTEXT_MIN_CHARS / 4 {
                    let _ = write!(
                        out,
                        "\nВНИМАНИЕ: epic-context ~{} токенов — ниже окна рубрики (800–1500). \
                         Сценарий «реализация без доступа к источникам» не выполняется: \
                         добавьте спеки через 'spec' или расширьте источники.",
                        packet.epic_context_tokens
                    );
                }
                // Детерминированные предупреждения готовности пакета.
                for w in &packet.warnings {
                    let _ = write!(out, "\nВНИМАНИЕ: {w}");
                }
                // T-02: что стало с пакетным реестром — часть результата, а не
                // деталь: гейт читает ПАКЕТНУЮ копию первой, поэтому «пакет
                // собран» без ответа на вопрос «а какие правила он применит»
                // оставлял бы исполнителя с чужим реестром.
                match &packet.constraints_action {
                    Some(action) => {
                        let _ = write!(
                            out,
                            "\nРеестр правил пакета: {action} — гейт читает именно пакетную копию \
                             (пакетная приоритетна, корневая — fallback); перед передачей \
                             перепишите правила под spine-инварианты (AD-n) эпика."
                        );
                    }
                    None => out.push_str(
                        "\nРеестр правил пакета: существующий файл не тронут \
                         (`--refresh-constraints` перезапишет его копией корневого).",
                    ),
                }
                Ok(ToolOutput::ok(out))
            }
            Err(e) => Ok(ToolOutput::err(format!("handoff_create: {e}"))),
        }
    }
}

/// Инструменты домена: `handoff_create` (регистрируется в
/// `tools::domain_tools` в обеих сборках — core и harness).
#[must_use]
pub fn tools(cfg: &Config) -> Vec<Arc<dyn Tool>> {
    vec![Arc::new(HandoffCreateTool::new(cfg.clone()))]
}

#[cfg(test)]
mod tests;
