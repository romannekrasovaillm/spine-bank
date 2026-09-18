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

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
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

/// Шаблон SPEC.md — верифицируемые контракты интерфейсов компонента
/// (модель «5.2»: прозаический ARCHITECTURE.md компонента заменяется spec'ом
/// с контрактами, проверяемыми тестами). Пишется только при отсутствии —
/// заполненный архитектором файл повторная генерация не затирает.
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
/// переданы явно). Не-UTF8 читается с потерями.
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
    let rules = match stack {
        "Rust" => {
            "\
  - name: no-unwrap-in-src
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
            "\
  - name: no-print-in-py
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
            "\
  - name: go-build-passes
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
            "\
  - name: readme-exists
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
            "\
  - name: readme-exists
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

/// Итог генерации handoff-пакета.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HandoffPacket {
    /// Каталог `.arch-handoff/`.
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
    if !repo.is_dir() {
        return Err(HarnessError::Harness(format!(
            "репозиторий недоступен: {}",
            repo.display()
        )));
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
    std::fs::write(
        &task_path,
        render_task_md(task, &rollback_text, qas_section.as_deref()),
    )
    .map_err(|e| HarnessError::io(&task_path, e))?;

    // ARCHITECTURE.md — всегда перезаписывается (компиляция актуальных спек).
    let arch_md = compile_epic_context(spec_files)?;
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

    // CONSTRAINTS.yaml — только при отсутствии (не затирать пользовательские правила).
    // Дефолт — под стек репозитория (Cargo.toml/pyproject.toml/go.mod/package.json).
    let constraints_path = dir.join("CONSTRAINTS.yaml");
    if !constraints_path.exists() {
        std::fs::write(&constraints_path, default_constraints(repo))
            .map_err(|e| HarnessError::io(&constraints_path, e))?;
    }

    // SPEC.md — только при отсутствии (не затирать правки архитектора):
    // с переданными спеками — собирается ИЗ НИХ (полные тексты контрактов,
    // кейс 2026-09-01: исполнители спотыкались о пустой шаблон, когда
    // архитектор передал спеки через --spec); без спек — шаблон.
    let spec_path = dir.join("SPEC.md");
    if !spec_path.exists() {
        let spec_md = if spec_files.is_empty() {
            SPEC_TEMPLATE.to_string()
        } else {
            render_spec_from_sources(spec_files)
        };
        std::fs::write(&spec_path, spec_md).map_err(|e| HarnessError::io(&spec_path, e))?;
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

    // RUBRIC.yaml — только при отсутствии и только если есть якорная рубрика.
    let rubric_path = dir.join("RUBRIC.yaml");
    if !rubric_path.exists() {
        let anchor = cfg.paths.rubrics_dir().join("handoff_quality.yaml");
        if anchor.is_file() {
            std::fs::copy(&anchor, &rubric_path).map_err(|e| HarnessError::io(&rubric_path, e))?;
        }
    }

    // adr/ — копии ADR-файлов; существующие копии не затираем.
    let mut adr_copies = Vec::new();
    for spec in spec_files {
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
/// далее — сжатые рендеры спек; итог усечён до [`EPIC_CONTEXT_MAX_CHARS`].
///
/// Глубина адаптивная: прочие секции рендерятся по [`DEPTH_SHALLOW`] абзацев,
/// но если контекст недобирает до [`EPIC_CONTEXT_MIN_CHARS`] (низ окна рубрики
/// `handoff_quality`, ~800 токенов), спеки перерендериваются глубже
/// ([`DEPTH_DEEP`]) — «реализация без доступа к источникам» требует массы.
///
/// # Errors
/// Спека не читается.
fn compile_epic_context(spec_files: &[PathBuf]) -> Result<String> {
    let mut out = render_epic(spec_files, DEPTH_SHALLOW)?;
    if out.chars().count() < EPIC_CONTEXT_MIN_CHARS {
        out = render_epic(spec_files, DEPTH_DEEP)?;
    }
    if out.chars().count() > EPIC_CONTEXT_MAX_CHARS {
        let notice = "\n\n> **Контекст усечён** до 6000 символов; полные тексты — в файлах-источниках (см. MANIFEST.json).\n";
        let keep = EPIC_CONTEXT_MAX_CHARS.saturating_sub(notice.chars().count());
        let truncated: String = out.chars().take(keep).collect();
        out = truncated;
        out.push_str(notice);
    }
    Ok(out)
}

/// Рендер epic-context на заданной глубине секций (абзацев на прочую секцию;
/// ADR-блоки spine всегда целиком).
fn render_epic(spec_files: &[PathBuf], depth: usize) -> Result<String> {
    let mut out = String::with_capacity(EPIC_CONTEXT_MAX_CHARS);
    out.push_str("# Архитектурный контекст (epic-context)\n\n");
    let _ = write!(out, "Собран: {}\n\n", Utc::now().to_rfc3339());
    out.push_str("Источники:\n");
    for f in spec_files {
        let _ = writeln!(out, "- {}", f.display());
    }
    out.push('\n');
    for f in spec_files {
        let text = std::fs::read_to_string(f).map_err(|e| HarnessError::io(f, e))?;
        let _ = write!(out, "<!-- источник: {} -->\n\n", f.display());
        out.push_str(render_spec(&text, depth).trim_end());
        out.push_str("\n\n");
    }
    Ok(out)
}

/// Рендерит одну спецификацию: секции с полями Binds/Prevents/Rule (ADR-блоки
/// spine) — целиком, прочие секции — заголовок + первые `depth` абзацев.
fn render_spec(text: &str, depth: usize) -> String {
    let mut preamble = String::new();
    let mut sections: Vec<(String, String)> = Vec::new();
    let mut cur: Option<(String, String)> = None;
    let mut in_fence = false;
    for line in text.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_fence = !in_fence;
        }
        if !in_fence && line.starts_with('#') {
            if let Some(s) = cur.take() {
                sections.push(s);
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
        sections.push(s);
    }

    let mut out = String::new();
    if !preamble.trim().is_empty() {
        out.push_str(&first_paragraphs(&preamble, depth));
        out.push_str("\n\n");
    }
    for (heading, body) in &sections {
        out.push_str(heading);
        out.push_str("\n\n");
        if is_adr_block(body) {
            out.push_str(body.trim());
        } else {
            out.push_str(&first_paragraphs(body, depth));
        }
        out.push_str("\n\n");
    }
    out
}

/// Признак ADR-блока spine: секция содержит поля Binds/Prevents/Rule.
fn is_adr_block(body: &str) -> bool {
    ["Binds:", "Prevents:", "Rule:"]
        .iter()
        .any(|m| body.contains(m))
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
                    "repo": {"type": "string", "description": "Корень репозитория (относительно cwd или абсолютный)"},
                    "task": {"type": "string", "description": "Формулировка задачи для кодового харнесса"},
                    "spec": {"type": "array", "items": {"type": "string"}, "description": "Пути к спецификациям/ADR (md), опционально"},
                    "rollback": {"type": "string", "description": "Явный план отката (шаги, сигналы, владелец решения); по умолчанию — откат на baseline-коммит"},
                    "route": {"type": "string", "enum": ["fast", "standard", "critical"], "description": "Маршрут значимости из significance_score: задаёт рекомендованный таймаут прогона (fast=1800с, standard=3600с, critical=7200с); по умолчанию standard"}
                },
                "required": ["repo", "task"]
            }),
        }
    }

    async fn call(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let Some(repo) = args.get("repo").and_then(Value::as_str) else {
            return Ok(ToolOutput::err(
                "handoff_create: обязательный аргумент 'repo' (string) отсутствует",
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
        match generate_handoff(&repo, task, &spec, &self.cfg, rollback, route) {
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
                out.push_str(
                    "\nНапоминание: CONSTRAINTS.yaml — стековая заготовка; перед передачей \
                     перепишите правила под spine-инварианты (AD-n) эпика.",
                );
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
mod tests {
    use super::*;

    /// Конфиг с assets внутри временного каталога (изоляция от ~/.arch-harness).
    fn cfg_in(dir: &Path) -> Config {
        let mut cfg = Config::default();
        cfg.paths.assets_dir = dir.join("assets");
        cfg
    }

    fn write_file(path: &Path, text: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("mkdir");
        }
        std::fs::write(path, text).expect("write");
    }

    const SPINE: &str = "# Spine\n\n\
        ## AD-1: Единый стек\n\n\
        **Binds:** все сервисы — Rust 1.85.\n\n\
        **Prevents:** зоопарк языков в контуре.\n\n\
        **Rule:** в CI закреплён toolchain 1.85.\n\n\
        ## Прочее\n\n\
        Абзац один.\n\n\
        Абзац два.\n\n\
        Абзац три — не должен попасть в контекст.\n";

    /// git-репозиторий с одним baseline-коммитом (явная идентичность —
    /// на CI/в контейнерах user.name/user.email может не быть).
    fn git_repo_with_baseline(dir: &std::path::Path) {
        std::fs::create_dir_all(dir).expect("mkdir repo");
        std::fs::write(dir.join("README.md"), "# baseline\n").expect("readme");
        let git = |args: &[&str]| {
            let out = std::process::Command::new("git")
                .arg("-C")
                .arg(dir)
                .args(args)
                .output()
                .expect("git");
            assert!(out.status.success(), "git {args:?}: {:?}", out.stderr);
        };
        git(&["init", "-q"]);
        git(&["add", "README.md"]);
        git(&[
            "-c",
            "user.name=test",
            "-c",
            "user.email=test@test",
            "commit",
            "-q",
            "-m",
            "baseline",
        ]);
    }

    #[test]
    fn generates_full_packet_and_preserves_user_files() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).expect("mkdir repo");
        // Маркер Rust-стека: дефолтные CONSTRAINTS — cargo-правила.
        write_file(&repo.join("Cargo.toml"), "[package]\nname = \"demo\"\n");
        let cfg = cfg_in(tmp.path());
        write_file(
            &cfg.paths.rubrics_dir().join("handoff_quality.yaml"),
            "# якорная рубрика\n",
        );
        let spine = tmp.path().join("specs/spine.md");
        write_file(&spine, SPINE);
        let adr = tmp.path().join("specs/adr/ADR-001.md");
        write_file(&adr, "# ADR-001\n\nСтатус: Accepted.\n");
        let notes = tmp.path().join("specs/notes.md");
        write_file(&notes, "# Заметки\n\nпервый\n\nвторой\n\nтретий\n");

        let packet = generate_handoff(
            &repo,
            "сделать фичу X",
            &[spine.clone(), adr.clone(), notes.clone()],
            &cfg,
            None,
            Route::Standard,
        )
        .expect("handoff");
        let dir = repo.join(".arch-handoff");
        assert_eq!(packet.dir, dir);

        let task_md = std::fs::read_to_string(dir.join("TASK.md")).expect("TASK.md");
        assert!(task_md.contains("сделать фичу X"));
        // Финализация: контракт требует git-коммита результата (иначе
        // оркестратор работу не увидит — регрессия «агенты без коммита»).
        assert!(task_md.contains("## Финализация (обязательно)"));
        assert!(task_md.contains("git add -A -- . ':!.arch-handoff'"));
        // План отката с якорем baseline (рубрика handoff_quality::rollback_plan).
        assert!(task_md.contains("## План отката"));
        let baseline = packet.baseline.as_deref().expect("baseline-якорь");
        assert!(
            task_md.contains(&format!("git reset --hard {baseline}")),
            "план отката с якорем:\n{task_md}"
        );
        assert!(task_md.contains("Владелец решения об откате"));
        assert!(
            packet.git_initialized,
            "не-git каталог — предгейт делает init"
        );
        assert!(task_md.contains("## Контракт результата"));
        assert!(task_md.contains("\"complete|partial|blocked\""));

        // MANIFEST несёт маршрут и рекомендованный таймаут (подхват harness_run).
        let manifest: Value = serde_json::from_str(
            &std::fs::read_to_string(dir.join("MANIFEST.json")).expect("MANIFEST.json"),
        )
        .expect("manifest json");
        assert_eq!(manifest["route"], "Standard");
        assert_eq!(manifest["recommended_timeout_secs"], 3600);
        assert_eq!(recommended_timeout_secs(&repo), Some(3600));

        let arch = std::fs::read_to_string(dir.join("ARCHITECTURE.md")).expect("ARCHITECTURE.md");
        // ADR-блок включён целиком (все три поля на месте).
        for field in ["**Binds:**", "**Prevents:**", "**Rule:**"] {
            assert!(arch.contains(field), "нет поля {field}");
        }
        // Прочие секции — заголовок + первые абзацы; спека мелкая, поэтому
        // сработала адаптивная глубина (окно рубрики 800–1500 токенов): все
        // три абзаца включены.
        assert!(arch.contains("Абзац два."));
        assert!(arch.contains("Абзац три"), "глубокий рендер:\n{arch}");
        assert!(arch.contains("Источники:"));

        // CONSTRAINTS.yaml создан с дефолтными правилами.
        let constraints = dir.join("CONSTRAINTS.yaml");
        let c = std::fs::read_to_string(&constraints).expect("CONSTRAINTS.yaml");
        for marker in [
            "must_not_contain",
            "unwrap",
            "dbg!",
            "file_exists",
            "command_succeeds",
            "cargo check",
            "timeout_secs: 120",
        ] {
            assert!(c.contains(marker), "CONSTRAINTS.yaml: нет '{marker}'");
        }

        // RUBRIC.yaml — копия якорной рубрики.
        let rubric = std::fs::read_to_string(dir.join("RUBRIC.yaml")).expect("RUBRIC.yaml");
        assert_eq!(rubric, "# якорная рубрика\n");

        // MANIFEST.json — мета пакета.
        let manifest_text =
            std::fs::read_to_string(dir.join("MANIFEST.json")).expect("MANIFEST.json");
        let manifest: Value = serde_json::from_str(&manifest_text).expect("manifest json");
        assert_eq!(manifest["task"], "сделать фичу X");
        assert!(manifest["created_at"].is_string());
        assert_eq!(manifest["sources"].as_array().expect("sources").len(), 3);
        let chars = manifest["epic_context_chars"].as_u64().expect("chars") as usize;
        assert_eq!(chars, arch.chars().count());
        let tokens = manifest["epic_context_tokens"].as_u64().expect("tokens") as usize;
        assert_eq!(tokens, chars / 4);
        assert_eq!(packet.epic_context_tokens, tokens);

        // adr/ — копия ADR-файла.
        assert!(dir.join("adr/ADR-001.md").is_file());
        assert!(packet.files.contains(&dir.join("adr/ADR-001.md")));

        // Повторный прогон: пользовательские CONSTRAINTS/RUBRIC не затираются,
        // TASK.md и MANIFEST.json перезаписываются.
        std::fs::write(&constraints, "# пользовательские правила\n").expect("custom constraints");
        std::fs::write(dir.join("RUBRIC.yaml"), "# пользовательская рубрика\n")
            .expect("custom rubric");
        let packet2 = generate_handoff(
            &repo,
            "другая задача",
            &[spine, adr, notes],
            &cfg,
            None,
            Route::Standard,
        )
        .expect("second handoff");
        assert_eq!(
            std::fs::read_to_string(&constraints).expect("constraints after"),
            "# пользовательские правила\n"
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("RUBRIC.yaml")).expect("rubric after"),
            "# пользовательская рубрика\n"
        );
        assert!(
            std::fs::read_to_string(dir.join("TASK.md"))
                .expect("TASK.md after")
                .contains("другая задача")
        );
        assert!(packet2.files.contains(&constraints));
    }

    #[test]
    fn handoff_includes_spec_template_and_preserves_filled_spec() {
        // SPEC.md — шаблон верифицируемых контрактов интерфейсов (модель
        // «5.2»: контракты вместо прозы ARCHITECTURE.md компонента); пишется
        // один раз и не затирается повторной генерацией, как CONSTRAINTS.yaml.
        let tmp = tempfile::tempdir().expect("tempdir");
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).expect("mkdir repo");
        let cfg = cfg_in(tmp.path());

        let packet =
            generate_handoff(&repo, "задача", &[], &cfg, None, Route::Fast).expect("handoff");
        let spec_path = packet.dir.join("SPEC.md");
        assert!(packet.files.contains(&spec_path), "{:?}", packet.files);
        let spec = std::fs::read_to_string(&spec_path).expect("SPEC.md");
        for section in [
            "## Входы (контракты соседей)",
            "## Выходы (публикуемые контракты)",
            "## Структуры данных",
            "## Границы ошибок",
            "## Критерии верификации (тесты)",
        ] {
            assert!(spec.contains(section), "нет секции «{section}»:\n{spec}");
        }
        // EARS-подсказка на месте.
        assert!(
            spec.contains("When <событие>, the <система> shall"),
            "{spec}"
        );
        // TASK.md несёт пункт чеклиста про SPEC.md.
        let task_md = std::fs::read_to_string(packet.dir.join("TASK.md")).expect("TASK.md");
        assert!(task_md.contains("SPEC.md"), "{task_md}");

        // Заполненный SPEC.md повторная генерация не затирает.
        std::fs::write(&spec_path, "# SPEC\n\nЗаполнено архитектором.\n").expect("fill spec");
        let packet2 =
            generate_handoff(&repo, "задача 2", &[], &cfg, None, Route::Fast).expect("handoff 2");
        assert_eq!(
            std::fs::read_to_string(&spec_path).expect("SPEC.md after"),
            "# SPEC\n\nЗаполнено архитектором.\n"
        );
        assert!(packet2.files.contains(&spec_path));
    }

    #[test]
    fn handoff_fills_spec_from_passed_spec_files() {
        // Кейс 2026-09-01: исполнители (theseus, codewhale) спотыкались о
        // пустой SPEC.md-шаблон, когда архитектор передал контракты через
        // --spec: теперь SPEC.md собирается из полных текстов переданных спек.
        let tmp = tempfile::tempdir().expect("tempdir");
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(repo.join("docs")).expect("mkdir");
        let cfg = cfg_in(tmp.path());
        let spec_src = repo.join("docs/SPEC-meta.md");
        std::fs::write(
            &spec_src,
            "# Спека агента\n\n## Выходы\n\n- карточка ТчВ, идемпотентность по id\n",
        )
        .expect("write spec");
        let packet = generate_handoff(
            &repo,
            "задача",
            std::slice::from_ref(&spec_src),
            &cfg,
            None,
            Route::Fast,
        )
        .expect("handoff");
        let spec = std::fs::read_to_string(packet.dir.join("SPEC.md")).expect("SPEC.md");
        assert!(spec.contains("Источник:"), "{spec}");
        assert!(
            spec.contains("карточка ТчВ, идемпотентность по id"),
            "{spec}"
        );
        assert!(
            !spec.contains("<что компонент потребляет"),
            "шаблон не нужен: {spec}"
        );
        // Правка архитектора поверх собранного файла не затирается.
        let spec_path = packet.dir.join("SPEC.md");
        std::fs::write(&spec_path, "# SPEC\n\nУточнено архитектором.\n").expect("edit");
        generate_handoff(
            &repo,
            "задача 2",
            std::slice::from_ref(&spec_src),
            &cfg,
            None,
            Route::Fast,
        )
        .expect("handoff 2");
        assert_eq!(
            std::fs::read_to_string(&spec_path).expect("SPEC.md after"),
            "# SPEC\n\nУточнено архитектором.\n"
        );
    }

    /// Модель с QAS в `<repo>/model/` для тестов критериев приёмки (ADR-007).
    fn repo_with_qas_model(repo: &Path) {
        write_file(
            &repo.join("model/NFR-001-lat.md"),
            "---\nid: NFR-001\ntype: nfr\ntitle: Latency\nstatus: accepted\nverification: hist\n---\n\np99 < 2s.\n",
        );
        write_file(
            &repo.join("model/QAS-001-peak.md"),
            "---\nid: QAS-001\ntype: qas\ntitle: Пиковая нагрузка\nstatus: accepted\n\
             implements: [NFR-001]\nsource: клиент канала\nstimulus: запрос авторизации в пике 5000 TPS\n\
             artifact: CMP-003 Authorization\nresponse: ответ об авторизации возвращён\n\
             measure: p99 < 2000 мс (NFR-001)\n---\n\nПроза.\n",
        );
    }

    #[test]
    fn handoff_unfolds_qas_into_acceptance_criteria() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).expect("mkdir repo");
        repo_with_qas_model(&repo);
        let cfg = cfg_in(tmp.path());

        generate_handoff(&repo, "задача", &[], &cfg, None, Route::Fast).expect("handoff");
        let task_md = std::fs::read_to_string(repo.join(".arch-handoff/TASK.md")).expect("TASK.md");
        // Секция появилась автоматически, без ручного копирования (DoD P1-1).
        assert!(
            task_md.contains("## Критерии приёмки (QAS из модели)"),
            "{task_md}"
        );
        assert!(task_md.contains("QAS-001"), "{task_md}");
        assert!(
            task_md.contains("запрос авторизации в пике 5000 TPS"),
            "{task_md}"
        );
        assert!(task_md.contains("p99 < 2000 мс (NFR-001)"), "{task_md}");
        // Секция стоит после задачи и до плана отката.
        let task_pos = task_md.find("задача").expect("задача");
        let qas_pos = task_md.find("## Критерии приёмки").expect("секция");
        let rollback_pos = task_md.find("## План отката").expect("откат");
        assert!(task_pos < qas_pos && qas_pos < rollback_pos, "{task_md}");
    }

    #[test]
    fn handoff_without_model_or_qas_has_no_acceptance_section() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).expect("mkdir repo");
        let cfg = cfg_in(tmp.path());
        generate_handoff(&repo, "задача", &[], &cfg, None, Route::Fast).expect("handoff");
        let task_md = std::fs::read_to_string(repo.join(".arch-handoff/TASK.md")).expect("TASK.md");
        assert!(!task_md.contains("Критерии приёмки (QAS"), "{task_md}");

        // Модель есть, но QAS в ней нет — секции тоже нет.
        let repo2 = tmp.path().join("repo2");
        std::fs::create_dir_all(&repo2).expect("mkdir repo2");
        write_file(
            &repo2.join("model/CMP-001-x.md"),
            "---\nid: CMP-001\ntype: cmp\ntitle: X\nstatus: designed\n---\n",
        );
        generate_handoff(&repo2, "задача", &[], &cfg, None, Route::Fast).expect("handoff 2");
        let task_md2 =
            std::fs::read_to_string(repo2.join(".arch-handoff/TASK.md")).expect("TASK.md 2");
        assert!(!task_md2.contains("Критерии приёмки (QAS"), "{task_md2}");
    }

    #[test]
    fn handoff_with_broken_model_fails_loudly() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).expect("mkdir repo");
        write_file(&repo.join("model/broken.md"), "нет frontmatter\n");
        let cfg = cfg_in(tmp.path());
        let err = generate_handoff(&repo, "задача", &[], &cfg, None, Route::Fast)
            .expect_err("битая модель — ошибка, не молчаливый пропуск");
        assert!(err.to_string().contains("QAS"), "{err}");
    }

    #[test]
    fn handoff_git_pregate_is_idempotent() {
        // Предгейт: не-git каталог получает git init + пустой baseline-якорь;
        // повторная генерация якорь не двигает (HEAD — тот же коммит).
        let tmp = tempfile::tempdir().expect("tempdir");
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).expect("mkdir repo");
        let cfg = cfg_in(tmp.path());

        let p1 =
            generate_handoff(&repo, "задача", &[], &cfg, None, Route::Fast).expect("handoff 1");
        assert!(p1.git_initialized);
        let b1 = p1.baseline.clone().expect("baseline 1");
        assert_eq!(recommended_timeout_secs(&repo), Some(1800), "fast → 1800");

        let p2 =
            generate_handoff(&repo, "задача 2", &[], &cfg, None, Route::Fast).expect("handoff 2");
        assert!(!p2.git_initialized, "повторный init не нужен");
        assert_eq!(p2.baseline.as_deref(), Some(b1.as_str()), "якорь стабилен");
        // Baseline — пустой коммит, содержимое каталога не подмётено.
        let count = git_out(&repo, &["log", "--oneline"]).expect("git log");
        assert_eq!(count.lines().count(), 1, "{count}");
    }

    #[test]
    fn handoff_explicit_rollback_and_critical_route() {
        // Явный план отката попадает в TASK.md дословно; маршрут Critical
        // даёт рекомендованный таймаут 7200 (регрессия: Critical-прогон
        // обрывался на дефолтных 30 минутах адаптера). Critical требует
        // epic-context в окне рубрики — даём объёмную спеку.
        let tmp = tempfile::tempdir().expect("tempdir");
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).expect("mkdir repo");
        let cfg = cfg_in(tmp.path());
        let spec = big_spec(tmp.path());

        let packet = generate_handoff(
            &repo,
            "миграция ядра",
            &[spec],
            &cfg,
            Some("Шаг 1: вернуть флаг фичи. Шаг 2: restore из snapshot БД."),
            Route::Critical,
        )
        .expect("handoff");
        assert_eq!(packet.recommended_timeout_secs, 7200);
        assert_eq!(recommended_timeout_secs(&repo), Some(7200));
        let task_md = std::fs::read_to_string(packet.dir.join("TASK.md")).expect("TASK.md");
        assert!(task_md.contains("## План отката"));
        assert!(
            task_md.contains("Шаг 1: вернуть флаг фичи. Шаг 2: restore из snapshot БД."),
            "явный откат дословно:\n{task_md}"
        );
        // Автотекст не подмешивается к явному плану.
        assert!(!task_md.contains("Сигналы отката"));
        // Несуществующий пакет — None (адаптер берёт свой дефолт).
        assert_eq!(recommended_timeout_secs(tmp.path()), None);
    }

    #[test]
    fn critical_route_refuses_thin_epic_context() {
        // Разрыв P2: для Critical контроль нижней границы окна рубрики —
        // отказ на сборке пакета, а не молчаливое предупреждение.
        let tmp = tempfile::tempdir().expect("tempdir");
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).expect("mkdir repo");
        let cfg = cfg_in(tmp.path());
        let err = generate_handoff(&repo, "миграция ядра", &[], &cfg, None, Route::Critical)
            .expect_err("Critical без спек обязан отказывать");
        let msg = err.to_string();
        assert!(msg.contains("ниже окна рубрики"), "{msg}");
        assert!(msg.contains("spec"), "{msg}");
        // Fast/Standard на том же объёме — собираются (Fast молча, Standard с warning).
        generate_handoff(&repo, "фикс", &[], &cfg, None, Route::Fast).expect("Fast ок");
        generate_handoff(&repo, "фикс", &[], &cfg, None, Route::Standard).expect("Standard ок");
    }

    /// Объёмная спека для Critical (epic-context в окне рубрики).
    fn big_spec(tmp: &Path) -> PathBuf {
        let mut big_text = String::from("# Спека миграции\n\n");
        for i in 0..60 {
            let _ = write!(
                big_text,
                "## Блок {i}\n\nИнвариант: AD-{i} — дословное правило интеграции, \
                 проверяемое тестом; детали, стыки и запреты для полноты контекста.\n\n"
            );
        }
        let spec = tmp.join("spec-big.md");
        write_file(&spec, &big_text);
        spec
    }

    #[test]
    fn handoff_generates_machine_readable_rollback_plan() {
        // ROLLBACK.yaml — машиночитаемый план отката (репетиция на гейте A4):
        // baseline-якорь + шаги по умолчанию, парсится rehearsal::load_plan;
        // MANIFEST.json несёт baseline_commit и rollback_plan; правки
        // архитектора повторная генерация не затирает (как CONSTRAINTS.yaml).
        let tmp = tempfile::tempdir().expect("tempdir");
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).expect("mkdir repo");
        let cfg = cfg_in(tmp.path());

        let packet =
            generate_handoff(&repo, "задача", &[], &cfg, None, Route::Fast).expect("handoff");
        let plan_path = packet.dir.join(crate::rehearsal::ROLLBACK_FILE);
        assert!(packet.files.contains(&plan_path), "{:?}", packet.files);
        let baseline = packet.baseline.clone().expect("baseline");
        let plan = crate::rehearsal::load_plan(&packet.dir).expect("план парсится");
        assert_eq!(plan.baseline_commit, baseline);
        assert_eq!(plan.steps.len(), 2, "якорь + откат: {:?}", plan.steps);
        assert!(plan.steps[1].run.contains("git reset --hard"));
        assert!(plan.verify.is_some(), "verify чистоты дерева");

        let manifest: Value = serde_json::from_str(
            &std::fs::read_to_string(packet.dir.join("MANIFEST.json")).expect("MANIFEST.json"),
        )
        .expect("manifest json");
        assert_eq!(manifest["baseline_commit"], baseline.as_str());
        assert!(
            manifest["rollback_plan"]
                .as_str()
                .expect("rollback_plan")
                .contains("Сигналы отката"),
            "дефолтный план в манифесте"
        );

        // Правка архитектора не затирается повторной генерацией.
        std::fs::write(
            &plan_path,
            "baseline_commit: \"\"\nsteps: []\n# пользовательский план\n",
        )
        .expect("custom plan");
        generate_handoff(&repo, "задача 2", &[], &cfg, None, Route::Fast).expect("handoff 2");
        assert!(
            std::fs::read_to_string(&plan_path)
                .expect("plan after")
                .contains("пользовательский план")
        );
    }

    #[test]
    fn critical_route_requires_baseline_and_rollback_plan() {
        // Rollback-first для Critical: пустой явный план отката — ошибка
        // сборки пакета (репетиции на A4 нечего прогонять).
        let tmp = tempfile::tempdir().expect("tempdir");
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).expect("mkdir repo");
        let cfg = cfg_in(tmp.path());
        let spec = big_spec(tmp.path());
        let err = generate_handoff(
            &repo,
            "миграция",
            std::slice::from_ref(&spec),
            &cfg,
            Some("   "),
            Route::Critical,
        )
        .expect_err("Critical с пустым rollback обязан отказывать");
        assert!(err.to_string().contains("rollback"), "{err}");
        // Дефолтный план (откат на baseline) — валиден: baseline от предгейта.
        let packet =
            generate_handoff(&repo, "миграция", &[spec], &cfg, None, Route::Critical).expect("ok");
        assert!(packet.baseline.is_some());
        // Для Fast пустой явный план — не ошибка маршрута (пакет собирается).
        let repo2 = tmp.path().join("repo2");
        std::fs::create_dir_all(&repo2).expect("mkdir repo2");
        generate_handoff(&repo2, "фикс", &[], &cfg, Some(""), Route::Fast).expect("Fast ок");
    }

    #[test]
    fn handoff_warns_on_dirty_tracked_tree() {
        // Хвост предгейта: грязные ОТСЛЕЖИВАЕМЫЕ файлы — откат на baseline
        // их потеряет; предупреждаем при генерации пакета.
        let tmp = tempfile::tempdir().expect("tempdir");
        let repo = tmp.path().join("repo");
        git_repo_with_baseline(&repo);
        let cfg = cfg_in(tmp.path());
        let p = generate_handoff(&repo, "задача", &[], &cfg, None, Route::Fast).expect("handoff");
        assert!(!p.git_dirty_tracked, "чистое дерево — без предупреждения");
        // Модифицируем отслеживаемый файл без коммита.
        std::fs::write(repo.join("README.md"), "# изменено\n").expect("edit");
        let p = generate_handoff(&repo, "задача", &[], &cfg, None, Route::Fast).expect("handoff");
        assert!(p.git_dirty_tracked, "грязное дерево — флаг выставлен");
        // Untracked-файлы грязью не считаются (reset --hard их не трогает).
        let p3_repo = tmp.path().join("repo3");
        git_repo_with_baseline(&p3_repo);
        std::fs::write(p3_repo.join("new-file.py"), "x = 1\n").expect("untracked");
        let p3 =
            generate_handoff(&p3_repo, "задача", &[], &cfg, None, Route::Fast).expect("handoff 3");
        assert!(!p3.git_dirty_tracked, "untracked — не грязь");
    }

    #[test]
    fn long_spec_is_truncated_with_notice() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).expect("mkdir repo");
        let cfg = cfg_in(tmp.path());
        let big = tmp.path().join("big.md");
        let mut text = String::from("# Большая спека\n\n");
        for i in 0..500 {
            let _ = write!(
                text,
                "## Секция {i}\n\nДостаточно длинный абзац, чтобы набрать объём контекста.\n\n"
            );
        }
        write_file(&big, &text);

        let packet = generate_handoff(&repo, "задача", &[big], &cfg, None, Route::Standard)
            .expect("handoff");
        let arch = std::fs::read_to_string(packet.dir.join("ARCHITECTURE.md")).expect("arch");
        assert!(
            arch.chars().count() <= EPIC_CONTEXT_MAX_CHARS,
            "len = {}",
            arch.chars().count()
        );
        assert!(arch.contains("Контекст усечён"));
    }

    #[test]
    fn default_constraints_follow_repo_stack() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).expect("mkdir");

        // Пустой репозиторий — общий минимум.
        let generic = default_constraints(&repo);
        assert!(generic.contains("readme-exists"), "{generic}");
        assert!(generic.contains("Стек: generic"), "{generic}");
        assert!(!generic.contains("cargo check"), "{generic}");

        write_file(&repo.join("requirements.txt"), "pytest\n");
        let py = default_constraints(&repo);
        assert!(py.contains("pytest -q"), "{py}");
        assert!(py.contains("print\\("), "{py}");
        assert!(!py.contains("cargo check"), "{py}");

        std::fs::remove_file(repo.join("requirements.txt")).expect("rm");
        write_file(&repo.join("go.mod"), "module demo\n");
        let go = default_constraints(&repo);
        assert!(go.contains("go build ./..."), "{go}");

        std::fs::remove_file(repo.join("go.mod")).expect("rm");
        write_file(&repo.join("package.json"), "{}\n");
        assert!(default_constraints(&repo).contains("npm test"));

        std::fs::remove_file(repo.join("package.json")).expect("rm");
        write_file(&repo.join("Cargo.toml"), "[package]\nname = \"demo\"\n");
        assert!(default_constraints(&repo).contains("cargo check"));
    }

    #[test]
    fn epic_context_deepens_below_rubric_window() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).expect("mkdir repo");
        let cfg = cfg_in(tmp.path());
        // Секция с пятью абзацами: на мелкой глубине (2) контекст ниже окна
        // рубрики — рендер углубляется, хвост секции доезжает.
        let spec = tmp.path().join("spec.md");
        write_file(
            &spec,
            "# Спека\n\n## Детали\n\nпервый\n\nвторой\n\nтретий\n\nчетвёртый\n\nпятый\n",
        );
        let packet = generate_handoff(&repo, "задача", &[spec], &cfg, None, Route::Standard)
            .expect("handoff");
        let arch = std::fs::read_to_string(packet.dir.join("ARCHITECTURE.md")).expect("arch");
        assert!(
            arch.contains("пятый"),
            "глубокий рендер дотянул хвост:\n{arch}"
        );
    }

    #[tokio::test]
    async fn handoff_create_warns_when_epic_below_window() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).expect("mkdir repo");
        let cfg = cfg_in(tmp.path());
        let tool = HandoffCreateTool::new(cfg.clone());
        let ctx = ToolContext::new(tmp.path().to_path_buf(), Arc::new(cfg));
        // Без спек epic-context ≈ один заголовок — ниже окна рубрики.
        let out = tool
            .call(json!({"repo": "repo", "task": "x"}), &ctx)
            .await
            .expect("call");
        assert!(out.content.contains("ниже окна рубрики"), "{}", out.content);
        assert!(
            out.content.contains("стековая заготовка"),
            "{}",
            out.content
        );
    }

    #[tokio::test]
    async fn handoff_create_tool_reports_summary() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).expect("mkdir repo");
        let cfg = cfg_in(tmp.path());
        let tool = HandoffCreateTool::new(cfg.clone());
        assert_eq!(tool.spec().name, "handoff_create");
        let ctx = ToolContext::new(tmp.path().to_path_buf(), Arc::new(cfg));

        // Нет обязательного аргумента.
        let out = tool.call(json!({"task": "x"}), &ctx).await.expect("call");
        assert!(out.is_error);
        assert!(out.content.contains("'repo'"));

        // Полный вызов (repo относительно cwd).
        let out = tool
            .call(json!({"repo": "repo", "task": "сделать Y"}), &ctx)
            .await
            .expect("call");
        assert!(!out.is_error, "{}", out.content);
        assert!(out.content.contains("Handoff-пакет создан"));
        assert!(repo.join(".arch-handoff/TASK.md").is_file());
    }
}
