//! MCP-серверный режим: `arch-be mcp serve` отдаёт архитектурный контроль
//! наружу кодовым агентам (Claude Code и др.) — verdict в момент написания
//! кода, а не на приёмке пакета (ADR-008, находка F-5 / задача P1-2).
//!
//! КОНТРАКТ (владелец: агент `mcp-serve`):
//! - транспорт stdio, NDJSON: одно сообщение JSON-RPC 2.0 — одна строка
//!   (как у клиента [`crate::mcp`], без `Content-Length`-фрейминга);
//!   stdout — только протокол, логи — stderr (tracing в `main`);
//! - методы: `initialize` (echo известной версии протокола, иначе наша),
//!   `tools/list`, `tools/call`, `prompts/list`, `prompts/get`, `ping`;
//!   `notifications/*` — игнор без ответа; неизвестный метод → `-32601`,
//!   битый JSON → `-32700`, отсутствует `method` → `-32600`, битые
//!   аргументы/инструмент/промпт → `-32602`; `resources/*` не поддержаны
//!   (`-32601`);
//! - промпты (capability `prompts`): семь плейбуков встроенного плагина
//!   spine-workflows как слэш-команды хоста ([`PLAYBOOK_PROMPTS`]) —
//!   хосту не нужно знать формулу «действуй по скиллу …» и то, куда он
//!   кладёт файлы скиллов: `prompts/get` возвращает user-сообщение с
//!   инструкцией и полным текстом SKILL.md. Текст — пользовательская
//!   копия из `plugins.dirs`, если есть (логика `skill_load`), иначе
//!   встроенный ассет: работает из коробки без `arch-be init`;
//! - режимы запуска ([`ServeMode`]): дефолт — строго read-only; флаг
//!   `--rw` (`arch-be mcp serve --rw`) дополнительно открывает белый список
//!   аддитивных записей ([`BRIDGE_READ_WRITE`]: `handoff_create`, `adr_new`,
//!   `agentsmd_generate`, `archify_deliver/show/compare`, `reverse_survey`,
//!   `skill_distill`, `evidence_pack`, `delta_propose`);
//! - инструменты — два слоя. РУЧНЫЕ (оттестированная поверхность ADR-008):
//!   контрольные `spine_lint`, `fitness_check`, `significance_score`,
//!   `significance_from_diff` (маршрут из git-диффа, S-1 anti-bypass),
//!   `trace_check`, `model_query`, `rubric_run` и чтение знаний (T4,
//!   ADR-015): `kb_search`, `skill_search`, `skill_load`, `mermaid_render`;
//!   плюс split-judge без LLM у сервера: `rubric_prompt` (промпты судьи +
//!   JSON-схема ответа) и `rubric_verify` (механическая сборка отчёта из
//!   сырых ответов хоста — медиана, `unstable`, `evidence_not_found`);
//!   плюс `rules_suggest` — кандидатные fitness-правила из пробелов кейса
//!   (EARS, таймауты контрактов, REQ→TASK, RTO/RPO→ADR, аудит операторских
//!   действий; модуль [`crate::rules_suggest`]).
//!   МОСТ: имена из белых списков [`BRIDGE_READ_ONLY`] (+ [`BRIDGE_READ_WRITE`]
//!   под `--rw`), не пересекающиеся с ручными, маршрутизируются в
//!   [`crate::tools::full_registry`] (`dispatch` — с политикой R-уровней;
//!   контекст БЕЗ LLM); спеки генерируются из `Tool::spec()`, annotations —
//!   из членства в списке + [`crate::policy::classify_tool`]. Транш 1
//!   инверсии в мосте: `nfr_check`, `model_validate`, `delta_guard`,
//!   `evidence_verify` (read-only верификаторы, JSON-вердикт
//!   passed/issues/summary в тексте вывода) и под `--rw` — `evidence_pack`,
//!   `delta_propose`. Транш 2: `landscape_report`, `adr_registry`,
//!   `rules_report`, `openspec_coverage`, `model_graph` (read-only отчёты:
//!   счётчики + markdown/mermaid в JSON; `passed=false` только у strict-гейтов
//!   `adr_registry`/`openspec_coverage`). Транш 3: `architect_review`,
//!   `change_impact` (составные инструменты — единое ревью репозитория и
//!   радиус изменения по графу модели; `src/review.rs`). В core-сборке (без
//!   фичи `harness`) домены
//!   `harness`/`distill`/`subagent`/`ralph`/`worktree`/`web` в реестре
//!   отсутствуют — мост их имена из белых списков молча пропускает (спеки
//!   строятся от реестра), `skill_distill` там недоступен;
//!   `handoff_create` — доступен и в core (генерация пакета — чисто
//!   файловая, модуль `crate::handoff`, волна 2 п.10);
//! - НИКОГДА не отдаются (даже под `--rw`) — [`BRIDGE_NEVER`]: write/exec/
//!   веб/субагенты (`bash`, `read_file`/`write_file`/`edit_file`, `glob`,
//!   `grep`, `propose_options`, `screenshot*`, `harness_run`, `subagent_*`,
//!   `ralph_run`, `worktree_new`, `web_*`) — это принадлежность хоста;
//!   `rubric_evaluate`/`rubric_generate` требуют LLM у сервера — вместо них
//!   split-judge. Решение политики Deny/RequireConfirm из `dispatch`
//!   возвращается как isError с текстом причины (подтверждение в
//!   неинтерактивном MCP невозможно → `RequireConfirm` трактуется как отказ);
//! - успешный вызов: `structuredContent` (машиночитаемый verdict) + тот же
//!   объект pretty-JSON в `content[0].text` (мостовые: text — сырой вывод
//!   инструмента, structuredContent — обёртка `{tool, output}`);
//!   контрольные verdict'ы несут `passed: bool` — `false` означает
//!   блокирующую находку, клиентский агент обязан отказать изменению,
//!   нарушающему `AD-*`;
//! - доменный сбой выполнения (файл не читается, сущность не найдена) —
//!   `result` с `isError: true`, не protocol error; сервер не падает ни на
//!   каком вводе, цикл живёт до EOF stdin;
//! - каждый вызов `tools/call` журналируется в проектный append-only журнал
//!   `<cwd сервера>/.arch-handoff/mcp-calls.jsonl` (модуль
//!   [`crate::mcp_journal`]: инструмент, вердикт, длительность, имена правил
//!   error-находок — БЕЗ содержимого аргументов; fail-soft, ротация по
//!   размеру) — источник outcome-данных для `arch-be digest`;
//! - `rubric_run` требует LLM-ключ из конфига: предпроверка доступности
//!   ключа (env задана / файл ключа существует; содержимое не печатается)
//!   → без ключа понятная JSON-RPC ошибка `-32603`; для моделей с
//!   `kind = "cli"` (внешний CLI-харнесс как LLM) ключ не нужен —
//!   предпроверка пропускается.

use std::collections::BTreeMap;
use std::fmt;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Deserialize;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};

use crate::config::{Config, ModelConfig};
use crate::error::Result;
use crate::tool::{ToolContext, ToolRegistry};
use crate::{control, kb, mcp, mermaid, model, plugin, rubric, rules_suggest, trace};

/// Код JSON-RPC «разбор запроса не удался» (невалидный JSON).
const PARSE_ERROR: i64 = -32700;
/// Код JSON-RPC «некорректный запрос» (не объект, нет `method`).
const INVALID_REQUEST: i64 = -32600;
/// Код JSON-RPC «метод не найден».
const METHOD_NOT_FOUND: i64 = -32601;
/// Код JSON-RPC «некорректные параметры» (аргументы, неизвестный инструмент).
const INVALID_PARAMS: i64 = -32602;
/// Код JSON-RPC «внутренняя ошибка» (недоступная capability: нет LLM-ключа).
const INTERNAL_ERROR: i64 = -32603;

/// Максимальная длина одной входной строки (`16 МиБ`). Защита от безграничного
/// роста буфера построчного чтения; самый тяжёлый легальный вход — текст
/// документа для `rubric_run` (лимит рубрики 24k символов), запас ~600×.
const MAX_LINE_BYTES: usize = 16 * 1024 * 1024;

/// Дефолтный лимит хитов `kb_search` (контракт инструмента kb.rs).
const KB_SEARCH_DEFAULT_LIMIT: usize = 10;
/// Дефолтный лимит хитов `skill_search` (контракт инструмента plugin.rs).
const SKILL_SEARCH_DEFAULT_LIMIT: usize = 8;
/// Потолок хитов поисков знаний (общий у kb.rs и plugin.rs).
const KNOWLEDGE_MAX_HITS: usize = 20;
/// Потолок символов тела скилла в ответе `skill_load` (как у агентного
/// инструмента: `ToolOutput::truncated(16_000)` — защита контекста клиента).
const SKILL_TEXT_MAX_CHARS: usize = 16_000;

/// Потолок текстового вывода мостовых инструментов реестра (защита контекста
/// хоста; тот же прецедент 16k, что у `skill_load` и агентных инструментов).
const BRIDGE_OUTPUT_MAX_CHARS: usize = 16_000;

/// Потолок числа ответов хоста в `rubric_verify` (k сэмплов судьи из
/// `[judge]` — единицы; лимит отсекает ошибочные гигантские пачки).
const MAX_VERIFY_ANSWERS: usize = 32;

/// Потолок символов значения аргумента промпта, эхом вставляемого в текст
/// сообщения `prompts/get`: аргумент — короткий путь/имя/предмет, а не
/// документ (длинный ввод хоста не должен раздувать сообщение-команду).
const MAX_PROMPT_ARG_VALUE_CHARS: usize = 500;

/// Статическая карточка плейбука-промпта (MCP prompts): имя (= имя скилла
/// плагина spine-workflows), встроенный текст SKILL.md (запасной источник
/// на машине без `arch-be init`) и объявление аргументов для `prompts/list`.
struct PlaybookPrompt {
    /// Имя промпта (= имя скилла-плейбука).
    name: &'static str,
    /// Встроенный полный текст SKILL.md (embedded-ассет [`crate::assets`]).
    embedded: &'static str,
    /// Аргументы промпта: (имя, описание). Объявляются, только если сценарий
    /// плейбука параметризован (путь/файл/предмет); все необязательные —
    /// слэш-команда обязана работать и без аргументов (цель уточняется из
    /// контекста диалога).
    arguments: &'static [(&'static str, &'static str)],
}

/// Плейбуки плагина spine-workflows как MCP-промпты (слэш-команды хоста,
/// пункт 11 бэклога волны 3). Порядок фиксирован: стабильный `prompts/list`,
/// та же последовательность в `docs/mcp.md`.
const PLAYBOOK_PROMPTS: &[PlaybookPrompt] = &[
    PlaybookPrompt {
        name: "spine-quickstart",
        embedded: crate::assets::PLUGIN_SPINE_WORKFLOWS_SKILLS_SPINE_QUICKSTART_SKILL_MD,
        arguments: &[],
    },
    PlaybookPrompt {
        name: "spine-content-bootstrap",
        embedded: crate::assets::PLUGIN_SPINE_WORKFLOWS_SKILLS_SPINE_CONTENT_BOOTSTRAP_SKILL_MD,
        arguments: &[],
    },
    PlaybookPrompt {
        name: "spine-architect-review",
        embedded: crate::assets::PLUGIN_SPINE_WORKFLOWS_SKILLS_SPINE_ARCHITECT_REVIEW_SKILL_MD,
        arguments: &[],
    },
    PlaybookPrompt {
        name: "spine-adr-judge",
        embedded: crate::assets::PLUGIN_SPINE_WORKFLOWS_SKILLS_SPINE_ADR_JUDGE_SKILL_MD,
        arguments: &[
            (
                "target",
                "Опц.: путь к документу для оценки (ADR, дизайн, спека) — \
                 подставляется в `target` вызовов rubric_prompt/rubric_verify",
            ),
            (
                "rubric",
                "Опц.: имя рубрики (по умолчанию выбирается по `rubric_list`, \
                 обычно `adr_quality`)",
            ),
        ],
    },
    PlaybookPrompt {
        name: "spine-contracts-gate",
        embedded: crate::assets::PLUGIN_SPINE_WORKFLOWS_SKILLS_SPINE_CONTRACTS_GATE_SKILL_MD,
        arguments: &[
            (
                "path",
                "Опц.: путь к файлу контракта (OpenAPI/AsyncAPI) для линта",
            ),
            ("old", "Опц.: путь к старой версии контракта для diff"),
            ("new", "Опц.: путь к новой версии контракта для diff"),
        ],
    },
    PlaybookPrompt {
        name: "spine-archify-viz",
        embedded: crate::assets::PLUGIN_SPINE_WORKFLOWS_SKILLS_SPINE_ARCHIFY_VIZ_SKILL_MD,
        arguments: &[(
            "subject",
            "Опц.: что визуализируем (система/поток/контракты) и имя диаграммы",
        )],
    },
    PlaybookPrompt {
        name: "spine-fitness-gate",
        embedded: crate::assets::PLUGIN_SPINE_WORKFLOWS_SKILLS_SPINE_FITNESS_GATE_SKILL_MD,
        arguments: &[],
    },
    PlaybookPrompt {
        name: "spine-bundle",
        embedded: crate::assets::PLUGIN_SPINE_WORKFLOWS_SKILLS_SPINE_BUNDLE_SKILL_MD,
        arguments: &[
            (
                "path",
                "Опц.: каталог кейса (нужен для `--status` существующего каркаса)",
            ),
            (
                "name",
                "Опц.: имя нового кейса для `arch-be bootstrap <имя>`",
            ),
            (
                "domain",
                "Опц.: домен кейса (payments, …) — подставляется в тексты каркаса",
            ),
        ],
    },
];

/// Белый список read-only моста в реестр инструментов ([`crate::tools::full_registry`]):
/// детерминированный контур контроля и чтения, не покрытый ручными
/// инструментами. Все перечисленные — без записи в рабочий каталог клиента
/// и без LLM. Доступны в обоих режимах [`ServeMode`].
/// (`pub`: справка CLI `mcp serve`/`connect --rw` сверяется со списками
/// реестра тестом в `main.rs` — расхождение справки с реестром падает в CI.)
pub const BRIDGE_READ_ONLY: &[&str] = &[
    "adr_registry",
    "agentsmd_lint",
    "archify_validate",
    "architect_review",
    "asyncapi_lint",
    "change_impact",
    "contract_diff",
    "delta_guard",
    "evidence_verify",
    "fleet_audit",
    "landscape_report",
    "model_drift",
    "model_graph",
    "model_validate",
    "nfr_check",
    "openapi_lint",
    "openspec_coverage",
    "plugin_list",
    "rubric_list",
    "rules_report",
];

/// Исторические имена аргумента-пути: синонимы каноничного `path` (Н8 волны C
/// 0.3.4). Держатся рядом со [`MANUAL_TOOLS`], потому что это поверхность
/// протокола, а не деталь одной функции.
pub const PATH_ARG_ALIASES: [&str; 4] = ["dir", "repo", "case", "change_dir"];

/// Дополнительный белый список режима `--rw` ([`ServeMode::ReadWrite`]):
/// аддитивные записи в рабочий каталог клиента (handoff-пакет, новый ADR,
/// AGENTS.md, HTML-артефакты Archify, карта обследования, дистиллированный
/// скилл, evidence-манифест, скелет дельты). `archify_*`/`skill_distill`/
/// `reverse_survey` классифицируются политикой как `ReadOnly`, но пишут
/// файлы — поэтому только под `--rw` (как и `evidence_pack`/`delta_propose`,
/// для которых политика честно даёт `Mutating`).
/// (`pub`: справка CLI `mcp serve`/`connect --rw` сверяется со списками
/// реестра тестом в `main.rs` — расхождение справки с реестром падает в CI.)
pub const BRIDGE_READ_WRITE: &[&str] = &[
    "adr_new",
    "agentsmd_generate",
    "archify_compare",
    "archify_deliver",
    "archify_show",
    "delta_propose",
    "evidence_pack",
    "handoff_create",
    "reverse_survey",
    "skill_distill",
];

/// Инструменты реестра, которые НЕ отдаются наружу ни в одном режиме:
/// exec/write/веб/субагенты — принадлежность хоста (у Claude Code и др.
/// они свои); `rubric_evaluate`/`rubric_generate` требуют LLM на стороне
/// сервера — в MCP-инверсии её нет, вместо них split-judge
/// (`rubric_prompt`/`rubric_verify`). Охраняется тестом реестра.
const BRIDGE_NEVER: &[&str] = &[
    "bash",
    "read_file",
    "write_file",
    "edit_file",
    "glob",
    "grep",
    "propose_options",
    "screenshot",
    "read_image",
    "harness_run",
    "ralph_run",
    "subagent_run",
    "subagent_list",
    "subagent_result",
    "worktree_new",
    "web_search",
    "web_fetch",
    "web_arch_sites",
    "rubric_evaluate",
    "rubric_generate",
];

/// Имена белых/never-списков моста, чьи домены собираются только под фичей
/// `harness` (кодовые харнессы, субагенты, ralph, worktree, веб, distill).
/// В core-сборке их нет в реестре — мост их молча пропускает (спеки
/// строятся от реестра). Используется тестами согласованности списков.
/// (`handoff_create` здесь намеренно НЕТ: с волны 2 (п.10) генерация
/// пакета — core-модуль `crate::handoff`, инструмент собирается везде.)
#[cfg(test)]
const HARNESS_ONLY_TOOLS: &[&str] = &[
    "skill_distill",
    "harness_run",
    "ralph_run",
    "subagent_run",
    "subagent_list",
    "subagent_result",
    "worktree_new",
    "web_search",
    "web_fetch",
    "web_arch_sites",
];

/// Имена ручных инструментов (нижний слой диспетчера) — мост их не дублирует
/// даже при наличии одноимённых реализаций в реестре (`spine_lint` и др.).
/// (`pub`: справка CLI `mcp serve` сверяется с реестром тестом в `main.rs`.)
pub const MANUAL_TOOLS: &[&str] = &[
    "spine_lint",
    "fitness_check",
    "significance_score",
    "significance_from_diff",
    "trace_check",
    "model_query",
    "rubric_run",
    "rubric_prompt",
    "rubric_verify",
    "kb_search",
    "skill_search",
    "skill_load",
    "mermaid_render",
    "rules_suggest",
    "trust_report",
    "verdict_explain",
];

/// Режим MCP-сервера: какой срез инструментов отдаётся хосту.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ServeMode {
    /// Только чтение (дефолт, поведение до флага `--rw`): ручные инструменты
    /// + read-only мост [`BRIDGE_READ_ONLY`].
    #[default]
    ReadOnly,
    /// `arch-be mcp serve --rw`: дополнительно [`BRIDGE_READ_WRITE`]
    /// (аддитивные записи в рабочий каталог клиента). [`BRIDGE_NEVER`]
    /// остаётся закрытым и в этом режиме.
    ReadWrite,
}

impl ServeMode {
    /// Открыт ли контур записи (режим `--rw`).
    fn allows_write(self) -> bool {
        matches!(self, Self::ReadWrite)
    }
}

/// Ошибка вызова инструмента: на каком уровне протокола отвечать.
#[derive(Debug)]
enum CallError {
    /// Ошибка протокола: ответ error-объектом (`-32602`/`-32603`).
    Protocol {
        /// Код ошибки JSON-RPC.
        code: i64,
        /// Сообщение (рус.).
        message: String,
    },
    /// Доменная ошибка выполнения: `result` с `isError: true` (MCP-стиль,
    /// как у агентских инструментов) — клиент видит причину, сервер жив.
    Execution(String),
}

impl CallError {
    /// Доменная ошибка выполнения с префиксом инструмента.
    fn execution(tool: &str, e: impl fmt::Display) -> Self {
        Self::Execution(format!("{tool}: {e}"))
    }

    /// Ошибка параметров (`-32602`).
    fn invalid_params(message: String) -> Self {
        Self::Protocol {
            code: INVALID_PARAMS,
            message,
        }
    }
}

/// Состояние сервера: конфигурация (нужна `rubric_run` для резолва рубрик
/// и LLM-судьи), режим [`ServeMode`] и один раз построенный полный реестр
/// инструментов (мост белых списков; реестр несёт политику R-уровней из
/// конфига). Путей/«текущего кейса» сервер не хранит — все цели приходят
/// аргументами вызова.
pub struct McpServe {
    cfg: Arc<Config>,
    mode: ServeMode,
    registry: ToolRegistry,
    /// Состояние сессии: хост из рукопожатия, выданный идентификатор, счётчик
    /// вызовов и выданные промпты судьи (ADR-048). Транспорт stdio — один
    /// процесс на сессию, поэтому состояние живёт в сервере, а не в соединении;
    /// `Mutex` нужен лишь потому, что хендлеры берут `&self`.
    session: std::sync::Mutex<crate::judge::SessionState>,
}

/// Исход успешного вызова инструмента: чем заполнить `content`/`structuredContent`.
enum DispatchOutcome {
    /// Машиночитаемый verdict (ручные инструменты): text-дубль — pretty JSON.
    Structured(Value),
    /// Текстовый вывод доменного инструмента реестра (мост): `text` идёт в
    /// `content` как есть (его читает клиент [`crate::mcp`]), structured —
    /// обёртка `{tool, output}` для машиночитаемого контура.
    Text {
        /// Обёртка `{tool, output}` для `structuredContent`.
        structured: Value,
        /// Сырой текстовый вывод инструмента (уже усечённый до лимита).
        text: String,
    },
}

/// Ответ-успех JSON-RPC.
fn ok_response(id: &Value, result: &Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

/// Ответ-ошибка JSON-RPC.
fn error_response(id: &Value, code: i64, message: impl Into<String>) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {"code": code, "message": message.into()},
    })
}

/// Разбирает аргументы инструмента в типизированную структуру;
/// ошибка десериализации → `-32602`.
fn parse_args<T: serde::de::DeserializeOwned>(
    args: Value,
    tool: &str,
) -> std::result::Result<T, CallError> {
    serde_json::from_value(args)
        .map_err(|e| CallError::invalid_params(format!("{tool}: невалидные аргументы: {e}")))
}

/// Аргумент `triggers` инструмента `significance_score`: каноничная карта
/// «триггер → bool» ЛИБО компактный массив строк вида `"name=true"`,
/// `"name=false"` или голое `"name"` (= true) — та же форма, что у CLI
/// `control score --trigger` (агенты-хосты часто копируют её в вызов MCP).
#[derive(Deserialize)]
#[serde(untagged)]
enum TriggersArg {
    /// Карта «триггер → сработал» (каноничная форма).
    Map(BTreeMap<String, bool>),
    /// Массив строк «name[=true|false]»; элемент без `=` — «name=true».
    List(Vec<String>),
}

impl TriggersArg {
    /// Приводит аргумент к карте триггеров; значение после `=`, отличное от
    /// `true`/`false`, и пустое имя — ошибка разбора (`-32602`).
    fn into_map(self) -> std::result::Result<BTreeMap<String, bool>, String> {
        match self {
            Self::Map(map) => Ok(map),
            Self::List(items) => {
                let mut map = BTreeMap::new();
                for item in items {
                    let (name, raw_value) = item.split_once('=').unwrap_or((item.as_str(), "true"));
                    let name = name.trim();
                    if name.is_empty() {
                        return Err(format!("пустое имя триггера в элементе '{item}'"));
                    }
                    let fired = match raw_value.trim() {
                        "true" => true,
                        "false" => false,
                        other => {
                            return Err(format!(
                                "значение '{other}' триггера '{name}' не bool \
                                 (ожидается true/false)"
                            ));
                        }
                    };
                    map.insert(name.to_string(), fired);
                }
                Ok(map)
            }
        }
    }
}

/// Прогоняет синхронную доменную функцию на blocking-пуле (fitness-правила
/// `command_succeeds` и обходы fs не должны держать worker runtime).
async fn blocking<T, F>(tool: &str, f: F) -> std::result::Result<T, CallError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T> + Send + 'static,
{
    match tokio::task::spawn_blocking(f).await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(e)) => Err(CallError::execution(tool, e)),
        Err(e) => Err(CallError::Execution(format!(
            "{tool}: задача прервана: {e}"
        ))),
    }
}

/// Вердикт журнала по структурированному ответу ручного инструмента:
/// `pass`/`fail` — по `passed`, `ok` — успех без `passed`. У `fail`
/// извлекаются имена правил error-находок (источник — verdict, не аргументы).
fn verdict_from_structured(v: &Value) -> (&'static str, Vec<String>) {
    match v.get("passed").and_then(Value::as_bool) {
        Some(true) => ("pass", Vec::new()),
        Some(false) => ("fail", crate::mcp_journal::failed_rule_names(v)),
        None => ("ok", Vec::new()),
    }
}

/// Вердикт журнала по текстовому выводу мостового инструмента: верификаторы
/// реестра несут в text JSON `{passed, issues, summary}` — разбираем его;
/// не-JSON вывод (markdown-отчёты) журналируется как `ok` без разбора.
fn verdict_from_bridge_text(text: &str) -> (&'static str, Vec<String>) {
    match serde_json::from_str::<Value>(text) {
        Ok(v) => verdict_from_structured(&v),
        Err(_) => ("ok", Vec::new()),
    }
}

impl McpServe {
    /// Сервер поверх конфигурации харнесса в режиме read-only (дефолт).
    #[must_use]
    pub fn new(cfg: Arc<Config>) -> Self {
        Self::with_mode(cfg, ServeMode::ReadOnly)
    }

    /// Сервер поверх конфигурации харнесса с явным режимом [`ServeMode`].
    /// Реестр инструментов строится один раз здесь: `dispatch` запросов
    /// моста идёт в разделяемый реестр (политика R-уровней внутри него).
    #[must_use]
    pub fn with_mode(cfg: Arc<Config>, mode: ServeMode) -> Self {
        let registry = crate::tools::full_registry(&cfg);
        Self {
            cfg,
            mode,
            registry,
            session: std::sync::Mutex::new(crate::judge::SessionState::new()),
        }
    }

    /// Состояние сессии. Захват яда мьютекса не ошибка: паника внутри
    /// критической секции не оставляет состояние неконсистентным (только
    /// счётчики и метки), поэтому работа продолжается на восстановленном
    /// значении — отказ сервера из-за этого был бы хуже.
    fn session(&self) -> std::sync::MutexGuard<'_, crate::judge::SessionState> {
        self.session
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Имя разрешено мосту в текущем режиме: только белые списки режима,
    /// с защитным отсечением [`BRIDGE_NEVER`] и ручных имён [`MANUAL_TOOLS`]
    /// (технически списки не пересекаются — проверяется тестом реестра;
    /// отсечение здесь — страховка на будущую правку списков: never-имя
    /// не уйдёт наружу даже при ошибочном добавлении в белый список).
    fn bridge_allowed(&self, name: &str) -> bool {
        if BRIDGE_NEVER.contains(&name) || MANUAL_TOOLS.contains(&name) {
            return false;
        }
        BRIDGE_READ_ONLY.contains(&name)
            || (self.mode.allows_write() && BRIDGE_READ_WRITE.contains(&name))
    }

    /// Обрабатывает одну строку транспорта; `None` — отвечать не нужно
    /// (уведомления по JSON-RPC ответа не имеют).
    async fn handle_line(&self, line: &str) -> Option<Value> {
        let message: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(e) => {
                return Some(error_response(
                    &Value::Null,
                    PARSE_ERROR,
                    format!("невалидный JSON: {e}"),
                ));
            }
        };
        let Some(obj) = message.as_object() else {
            return Some(error_response(
                &Value::Null,
                INVALID_REQUEST,
                "сообщение не является JSON-объектом",
            ));
        };
        let id = obj.get("id").cloned();
        let method = obj.get("method").and_then(Value::as_str);
        match (id, method) {
            // Уведомления (без id) не получают ответа — ни на notifications/*,
            // ни на неизвестные методы-уведомления.
            (None, Some(_)) => None,
            (None, None) => Some(error_response(
                &Value::Null,
                INVALID_REQUEST,
                "нет поля 'method'",
            )),
            (Some(id), None) => Some(error_response(&id, INVALID_REQUEST, "нет поля 'method'")),
            (Some(id), Some(method)) => {
                let params = obj.get("params").cloned().unwrap_or(Value::Null);
                Some(self.handle_request(&id, method, params).await)
            }
        }
    }

    /// Диспетчер запросов (методы с `id`, требующие ответа).
    async fn handle_request(&self, id: &Value, method: &str, params: Value) -> Value {
        match method {
            "initialize" => {
                // Echo известной версии из запроса, иначе — наша текущая:
                // клиент сам решит, устраивает ли его ответная версия.
                let requested = params.get("protocolVersion").and_then(Value::as_str);
                let version = match requested {
                    Some(v)
                        if v == mcp::PROTOCOL_VERSION || v == mcp::PROTOCOL_VERSION_FALLBACK =>
                    {
                        v
                    }
                    _ => mcp::PROTOCOL_VERSION,
                };
                // `clientInfo` — единственное, что хост говорит о себе сам:
                // запоминаем как ЗАЯВЛЕННОЕ имя и версию (ADR-048: это метка
                // хоста, а не удостоверение того, кто отвечал на промпты).
                let host = params.get("clientInfo").and_then(|ci| {
                    let name = ci.get("name").and_then(Value::as_str)?;
                    if name.trim().is_empty() {
                        return None;
                    }
                    Some(crate::judge::HostInfo {
                        name: name.to_string(),
                        version: ci
                            .get("version")
                            .and_then(Value::as_str)
                            .map(str::to_string),
                    })
                });
                let session_id = {
                    let mut session = self.session();
                    if let Some(host) = host {
                        session.set_host(host);
                    }
                    session.id().to_string()
                };
                ok_response(
                    id,
                    &json!({
                        "protocolVersion": version,
                        "sessionId": session_id,
                        "capabilities": {
                            "tools": {"listChanged": false},
                            "prompts": {"listChanged": false},
                        },
                        "serverInfo": {
                            "name": "arch-harness",
                            "version": env!("CARGO_PKG_VERSION"),
                        },
                        "instructions": "Архитектурный контроль Spine для кодового агента: \
                                         перед коммитом изменения вызывайте fitness_check \
                                         (repo + CONSTRAINTS.yaml), trace_check (case) и \
                                         spine_lint (path); passed=false с находками error — \
                                         основание ОТКАЗАТЬ изменению, нарушающему AD-*, \
                                         перечислив находки. significance_score — маршрут \
                                         значимости (fast/standard/critical). model_query — \
                                         карточки и связи сущностей модели. rubric_run — \
                                         LLM-оценка документа рубрикой (нужен API-ключ); \
                                         без ключа — split-judge: rubric_prompt выдаёт промпты \
                                         судьи и JSON-схему ответа, rubric_verify механически \
                                         собирает отчёт из сырых ответов вашей модели. \
                                         Детерминированный контур реестра (openapi_lint, \
                                         asyncapi_lint, contract_diff, fleet_audit, \
                                         agentsmd_lint, archify_validate, rubric_list, \
                                         plugin_list, nfr_check, model_validate, model_drift, \
                                         delta_guard, evidence_verify) доступен напрямую; \
                                         отчёты реестров (landscape_report, adr_registry, \
                                         rules_report, openspec_coverage, model_graph) — \
                                         read-only JSON со счётчиками; составные инструменты: \
                                         architect_review (всё ревью одним вызовом — маршрут, \
                                         контур контроля, модель, контракты) и change_impact \
                                         (что заденет изменение и с кем согласовывать); \
                                         rules_suggest — кандидатные fitness-правила из \
                                         пробелов кейса (EARS, таймауты контрактов, REQ→TASK, \
                                         RTO/RPO→ADR, аудит оператора); аргумент `cwd` — \
                                         рабочий каталог клиента для относительных путей. \
                                         Чтение знаний (read-only): kb_search — поиск по \
                                         базе знаний архитектора; skill_search/skill_load — \
                                         библиотека скиллов; mermaid_render — диаграмма \
                                         mermaid (code/path) в ASCII-арт. Плейбуки spine-* \
                                         (подключение, гейты, разбор, судья рубрик, визуализация) \
                                         доступны как промпты (prompts/list, prompts/get) — \
                                         слэш-команды хоста с полным сценарием в сообщении.",
                    }),
                )
            }
            "ping" => ok_response(id, &json!({})),
            "tools/list" => ok_response(id, &json!({"tools": self.all_tool_specs()})),
            "tools/call" => self.handle_tool_call(id, &params).await,
            "prompts/list" => self.handle_prompts_list(id).await,
            "prompts/get" => self.handle_prompts_get(id, &params).await,
            other => error_response(id, METHOD_NOT_FOUND, format!("неизвестный метод '{other}'")),
        }
    }

    /// `tools/call`: разбор `name`/`arguments`, диспетчер инструментов,
    /// упаковка verdict'а в MCP-ответ.
    async fn handle_tool_call(&self, id: &Value, params: &Value) -> Value {
        let Some(name) = params.get("name").and_then(Value::as_str) else {
            return error_response(id, INVALID_PARAMS, "tools/call: нет строкового поля 'name'");
        };
        let args = params
            .get("arguments")
            .cloned()
            .unwrap_or_else(|| json!({}));
        if !args.is_object() {
            return error_response(
                id,
                INVALID_PARAMS,
                format!("tools/call: 'arguments' должен быть объектом, получено: {args}"),
            );
        }
        let started = std::time::Instant::now();
        let outcome = self.dispatch_tool(name, args).await;
        // Счётчик вызовов сессии растёт ПОСЛЕ вызова: во время обработки
        // `self.session().calls()` — это число уже прошедших вызовов, и отчёт
        // судьи честно называет, сколько работы было в сессии ДО судейства
        // (косвенный признак рабочего контекста автора, ADR-048).
        self.session().note_call();
        self.journal_call(name, started.elapsed(), &outcome);
        match outcome {
            Ok(DispatchOutcome::Structured(structured)) => {
                // Клиент нашего же mcp.rs читает только text-части — дублируем
                // verdict pretty-JSON; structuredContent — для MCP-клиентов.
                let text = serde_json::to_string_pretty(&structured)
                    .unwrap_or_else(|_| structured.to_string());
                ok_response(
                    id,
                    &json!({
                        "content": [{"type": "text", "text": text}],
                        "structuredContent": structured,
                        "isError": false,
                    }),
                )
            }
            Ok(DispatchOutcome::Text { structured, text }) => ok_response(
                id,
                &json!({
                    "content": [{"type": "text", "text": text}],
                    "structuredContent": structured,
                    "isError": false,
                }),
            ),
            Err(CallError::Execution(message)) => ok_response(
                id,
                &json!({
                    "content": [{"type": "text", "text": message}],
                    "isError": true,
                }),
            ),
            Err(CallError::Protocol { code, message }) => error_response(id, code, message),
        }
    }

    /// Журналирует вызов в проектный журнал `.arch-handoff/mcp-calls.jsonl`
    /// (модуль [`crate::mcp_journal`]): инструмент, вердикт, длительность,
    /// имена правил из error-находок — БЕЗ содержимого аргументов. Здесь
    /// проходят и ручные, и мостовые вызовы (единая точка `tools/call`).
    /// Fail-soft: журнал — аудит, а не часть вызова; его сбой (каталог не
    /// создать, ФС только на чтение) не должен ломать инструмент.
    fn journal_call(
        &self,
        name: &str,
        duration: std::time::Duration,
        outcome: &std::result::Result<DispatchOutcome, CallError>,
    ) {
        let (verdict, rules) = match outcome {
            Ok(DispatchOutcome::Structured(v)) => verdict_from_structured(v),
            Ok(DispatchOutcome::Text { text, .. }) => verdict_from_bridge_text(text),
            Err(CallError::Execution(_)) => ("error", Vec::new()),
            Err(CallError::Protocol { .. }) => ("invalid", Vec::new()),
        };
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        // Ошибка записи журнала осознанно глушится (fail-soft по контракту
        // mcp_journal): аудит не должен ломать вызовы инструментов.
        let _ = crate::mcp_journal::append(
            &cwd,
            &crate::mcp_journal::JournalEntry::new(name, verdict, duration, rules),
        );
    }

    /// `prompts/list`: семь плейбуков [`PLAYBOOK_PROMPTS`] с описаниями из
    /// frontmatter и объявлениями аргументов. Пагинация не нужна (список
    /// фиксирован и мал) — `cursor` из params принимается и игнорируется.
    async fn handle_prompts_list(&self, id: &Value) -> Value {
        let dirs = self.cfg.plugins.dirs.clone();
        let listed = blocking("prompts/list", move || -> Result<Value> {
            let prompts: Vec<Value> = PLAYBOOK_PROMPTS
                .iter()
                .map(|pb| {
                    let (_, description) = resolve_playbook(&dirs, pb);
                    json!({
                        "name": pb.name,
                        "description": description,
                        "arguments": pb.arguments.iter().map(|(an, ad)| json!({
                            "name": an,
                            "description": ad,
                            "required": false,
                        })).collect::<Vec<_>>(),
                    })
                })
                .collect();
            Ok(json!({"prompts": prompts}))
        })
        .await;
        match listed {
            Ok(result) => ok_response(id, &result),
            // Доменных сбоев тут нет (fallback встроенный) — только срыв
            // blocking-задачи: внутренняя ошибка сервера.
            Err(CallError::Execution(message)) => error_response(id, INTERNAL_ERROR, message),
            Err(CallError::Protocol { code, message }) => error_response(id, code, message),
        }
    }

    /// `prompts/get`: слэш-команда хоста — одно user-сообщение с инструкцией
    /// «действуй по этому плейбуку», полным текстом SKILL.md и эхом переданных
    /// (объявленных) аргументов. Неизвестное имя и битые аргументы → `-32602`.
    async fn handle_prompts_get(&self, id: &Value, params: &Value) -> Value {
        let Some(name) = params.get("name").and_then(Value::as_str) else {
            return error_response(
                id,
                INVALID_PARAMS,
                "prompts/get: нет строкового поля 'name'",
            );
        };
        let Some(pb) = PLAYBOOK_PROMPTS.iter().find(|p| p.name == name) else {
            return error_response(
                id,
                INVALID_PARAMS,
                format!("prompts/get: неизвестный промпт '{name}'; список — prompts/list"),
            );
        };
        let arguments = params
            .get("arguments")
            .cloned()
            .unwrap_or_else(|| json!({}));
        let Some(arg_obj) = arguments.as_object() else {
            return error_response(
                id,
                INVALID_PARAMS,
                format!(
                    "prompts/get: 'arguments' должен быть объектом «имя → строка», получено: {arguments}"
                ),
            );
        };
        // Эхом подставляются только объявленные аргументы (незнакомые ключи
        // хоста игнорируются — форвард-совместимость); значения — строки по
        // спецификации PromptArgument, иное → -32602.
        let mut provided: Vec<(&str, String)> = Vec::new();
        for (an, _) in pb.arguments {
            if let Some(v) = arg_obj.get(*an) {
                let Some(s) = v.as_str() else {
                    return error_response(
                        id,
                        INVALID_PARAMS,
                        format!("prompts/get: аргумент '{an}' должен быть строкой, получено: {v}"),
                    );
                };
                provided.push((*an, s.chars().take(MAX_PROMPT_ARG_VALUE_CHARS).collect()));
            }
        }
        let dirs = self.cfg.plugins.dirs.clone();
        let built = blocking("prompts/get", move || -> Result<Value> {
            let (mut body, description) = resolve_playbook(&dirs, pb);
            if body.chars().count() > SKILL_TEXT_MAX_CHARS {
                let cut: String = body.chars().take(SKILL_TEXT_MAX_CHARS).collect();
                body = format!("{cut}\n… [усечено: {SKILL_TEXT_MAX_CHARS} символов]");
            }
            let mut text = format!(
                "Действуй по этому плейбуку — скилл `{}` плагина spine-workflows \
                 (MCP-сервер Spine): выполняй его шаги по порядку, вызывая \
                 инструменты этого сервера; вердикты (`passed`, находки) \
                 докладывай архитектору.\n\n{body}",
                pb.name
            );
            if !provided.is_empty() {
                text.push_str("\n\nАргументы запуска (подставь в шаги плейбука):");
                for (an, av) in &provided {
                    // write! в String не падает — игнор результата безопасен.
                    let _ = write!(text, "\n- {an} = \"{av}\"");
                }
            }
            Ok(json!({
                "description": description,
                "messages": [{
                    "role": "user",
                    "content": {"type": "text", "text": text},
                }],
            }))
        })
        .await;
        match built {
            Ok(result) => ok_response(id, &result),
            Err(CallError::Execution(message)) => error_response(id, INTERNAL_ERROR, message),
            Err(CallError::Protocol { code, message }) => error_response(id, code, message),
        }
    }

    /// Отвергает незнакомые аргументы вызова, перечисляя допустимые (Н8).
    ///
    /// Проверяются инструменты с объявленной схемой; у мостовых инструментов
    /// схема живёт в модуле и проверку делает `serde` при разборе — там
    /// перечень печатает сама ошибка десериализации.
    fn reject_unknown_args(name: &str, args: &Value) -> std::result::Result<(), CallError> {
        let Some(map) = args.as_object() else {
            return Ok(());
        };
        if map.is_empty() {
            return Ok(());
        }
        let Some(schema) = tool_specs()
            .into_iter()
            .find(|t| t.get("name").and_then(Value::as_str) == Some(name))
            .and_then(|t| t.get("inputSchema").cloned())
        else {
            return Ok(());
        };
        let Some(props) = schema.get("properties").and_then(Value::as_object) else {
            return Ok(());
        };
        // Исторические имена пути — синонимы каноничного `path` (Н8), а не
        // незнакомые аргументы: старые клиенты и скрипты обязаны работать.
        let path_accepted = props.contains_key("path");
        let known = |k: &String| {
            props.contains_key(k) || (path_accepted && PATH_ARG_ALIASES.contains(&k.as_str()))
        };
        let unknown: Vec<&String> = map.keys().filter(|k| !known(k)).collect();
        if unknown.is_empty() {
            return Ok(());
        }
        let mut allowed: Vec<&str> = props.keys().map(String::as_str).collect();
        allowed.sort_unstable();
        let mut hint = String::new();
        if allowed.contains(&"path") {
            hint.push_str(
                " (путь во всех инструментах называется `path`; исторические `dir`,                  `repo`, `case`, `change_dir` принимаются как синонимы)",
            );
        }
        Err(CallError::invalid_params(format!(
            "{name}: неизвестный аргумент {}; допустимые: {}{hint}",
            unknown
                .iter()
                .map(|k| format!("'{k}'"))
                .collect::<Vec<_>>()
                .join(", "),
            allowed.join(", ")
        )))
    }

    /// Маршрутизация вызова по имени инструмента: сначала ручные
    /// реализации (оттестированная поверхность ADR-008), затем мост в
    /// реестр по белым спискам режима, иначе — `-32602`.
    async fn dispatch_tool(
        &self,
        name: &str,
        args: Value,
    ) -> std::result::Result<DispatchOutcome, CallError> {
        // Н8: неизвестный аргумент — ошибка вызова с ПЕРЕЧНЕМ допустимых, а не
        // молчаливый игнор (serde пропускает незнакомые поля, и опечатка в
        // имени выглядела как «инструмент не сработал»).
        Self::reject_unknown_args(name, &args)?;
        match name {
            "spine_lint" => self
                .tool_spine_lint(args)
                .await
                .map(DispatchOutcome::Structured),
            "fitness_check" => self
                .tool_fitness_check(args)
                .await
                .map(DispatchOutcome::Structured),
            "significance_score" => {
                Self::tool_significance_score(args).map(DispatchOutcome::Structured)
            }
            "significance_from_diff" => self
                .tool_significance_from_diff(args)
                .await
                .map(DispatchOutcome::Structured),
            "trace_check" => self
                .tool_trace_check(args)
                .await
                .map(DispatchOutcome::Structured),
            "model_query" => self
                .tool_model_query(args)
                .await
                .map(DispatchOutcome::Structured),
            "rubric_run" => self
                .tool_rubric_run(args)
                .await
                .map(DispatchOutcome::Structured),
            // Split-judge без LLM у сервера: промпты судьи наружу,
            // механическая сборка отчёта из ответов хоста.
            "rubric_prompt" => self
                .tool_rubric_prompt(args)
                .await
                .map(DispatchOutcome::Structured),
            "rubric_verify" => self
                .tool_rubric_verify(args)
                .await
                .map(DispatchOutcome::Structured),
            // T4 (ADR-015): чтение знаний наружу; тонкие адаптеры к ядру
            // агентных инструментов kb.rs/plugin.rs/mermaid.rs (общая логика
            // живёт там — MCP-слой только парсит аргументы и формирует JSON).
            "kb_search" => self
                .tool_kb_search(args)
                .await
                .map(DispatchOutcome::Structured),
            "skill_search" => self
                .tool_skill_search(args)
                .await
                .map(DispatchOutcome::Structured),
            "skill_load" => self
                .tool_skill_load(args)
                .await
                .map(DispatchOutcome::Structured),
            "mermaid_render" => self
                .tool_mermaid_render(args)
                .await
                .map(DispatchOutcome::Structured),
            // Кандидатные fitness-правила из пробелов кейса (read-only
            // эвристики, src/rules_suggest.rs).
            "rules_suggest" => self
                .tool_rules_suggest(args)
                .await
                .map(DispatchOutcome::Structured),
            // Паспорт вердикта (W1): вердикт гейта + его границы.
            "verdict_explain" => self
                .tool_verdict_explain(args)
                .await
                .map(DispatchOutcome::Structured),
            // Метрика доверия к контуру (W4): место на шкале 1–5 с якорями
            // и доказательствами. Поверх журнала вызовов — того самого,
            // который пишет этот сервер.
            "trust_report" => self
                .tool_trust_report(args)
                .await
                .map(DispatchOutcome::Structured),
            // Мост в реестр инструментов харнесса (белые списки режима).
            other if self.bridge_allowed(other) => self.bridge_dispatch(other, args).await,
            other => Err(CallError::invalid_params(format!(
                "неизвестный инструмент '{other}' (список — tools/list)"
            ))),
        }
    }

    /// Мостовой вызов: маршрутизация в [`ToolRegistry::dispatch`] с
    /// [`ToolContext`] БЕЗ LLM (инструменты, требующие модель, в белые
    /// списки не входят). `cwd` — дополнительный аргумент моста: рабочий
    /// каталог клиента, от которого резолвятся относительные пути
    /// (по умолчанию — cwd процесса сервера, поведение ручных инструментов).
    /// Решение политики R-уровней (Deny/RequireConfirm) приходит из
    /// `dispatch` как `ToolOutput::err` и отдаётся доменным isError
    /// с текстом причины: подтверждение в неинтерактивном MCP невозможно,
    /// `RequireConfirm` трактуется как отказ.
    async fn bridge_dispatch(
        &self,
        name: &str,
        args: Value,
    ) -> std::result::Result<DispatchOutcome, CallError> {
        if self.registry.get(name).is_none() {
            return Err(CallError::invalid_params(format!(
                "{name}: инструмент отключён конфигом сервера (см. [archify]/[web] enabled)"
            )));
        }
        let cwd = args
            .get("cwd")
            .and_then(Value::as_str)
            .map_or_else(|| PathBuf::from("."), PathBuf::from);
        let ctx = ToolContext::new(cwd, Arc::clone(&self.cfg));
        let out = self.registry.dispatch(name, args, &ctx).await;
        if out.is_error {
            return Err(CallError::Execution(out.content));
        }
        let text = out.truncated(BRIDGE_OUTPUT_MAX_CHARS).content;
        let structured = json!({"tool": name, "output": text.clone()});
        Ok(DispatchOutcome::Text { structured, text })
    }

    /// `spine_lint`: линтер ARCHITECTURE-SPINE.md → verdict
    /// (`passed` = нет находок severity error).
    async fn tool_spine_lint(&self, args: Value) -> std::result::Result<Value, CallError> {
        #[derive(Deserialize)]
        struct Args {
            /// Путь к ARCHITECTURE-SPINE.md.
            path: String,
        }
        let args: Args = parse_args(args, "spine_lint")?;
        let path = PathBuf::from(args.path);
        let issues = blocking("spine_lint", move || control::lint_spine(&path)).await?;
        let errors = issues.iter().filter(|i| i.severity == "error").count();
        let warns = issues.len() - errors;
        let summary = if issues.is_empty() {
            "spine: нарушений нет".to_string()
        } else {
            format!(
                "spine: {} находок (error: {errors}, warn: {warns})",
                issues.len()
            )
        };
        Ok(json!({
            "passed": errors == 0,
            "issue_count": issues.len(),
            "error_count": errors,
            "warn_count": warns,
            "issues": issues,
            "summary": summary,
        }))
    }

    /// `fitness_check`: прогон CONSTRAINTS.yaml по репозиторию → verdict
    /// (семантика [`control::check`]: `passed` = нет находок severity error).
    async fn tool_fitness_check(&self, args: Value) -> std::result::Result<Value, CallError> {
        #[derive(Deserialize)]
        struct Args {
            /// Корень репозитория клиента.
            #[serde(alias = "path")]
            repo: String,
            /// Файл ограничений (дефолт `<repo>/.arch-handoff/CONSTRAINTS.yaml`,
            /// иначе `<repo>/CONSTRAINTS.yaml`).
            constraints: Option<String>,
            /// База git для сверки состава правил (П5): по умолчанию —
            /// merge-base с основной веткой, иначе HEAD.
            base: Option<String>,
        }
        let args: Args = parse_args(args, "fitness_check")?;
        let repo = PathBuf::from(args.repo);
        // Единый резолвер реестра (E2): явный путь → пакетная копия →
        // корневой fallback; ни одной копии — канонический дефолт, чтобы
        // ошибка «файл не читается» ссылалась на пакетный путь.
        let resolution = control::resolve_constraints_path_detailed(
            &repo,
            args.constraints.as_deref().map(Path::new),
        );
        let drift_note = resolution
            .as_ref()
            .and_then(control::ConstraintsPathResolution::drift_note);
        let constraints =
            resolution.map_or_else(|| repo.join(control::HANDOFF_CONSTRAINTS_PATH), |r| r.path);
        let constraints_label = constraints.display().to_string();
        let base = args.base;
        // П5: сверка состава правил с git-базой — анти-ослабление доступно
        // не только составному гейту.
        let report = blocking("fitness_check", move || {
            control::check_anchored(
                &repo,
                &constraints,
                &control::baseline::CheckOptions::default(),
                base.as_deref(),
            )
        })
        .await?;
        Ok(json!({
            "passed": report.passed,
            "repo": report.repo,
            "constraints": constraints_label,
            "drift_note": drift_note,
            "issue_count": report.issues.len(),
            "issues": report.issues,
            "fingerprint": report.fingerprint,
            "summary": report.summary,
        }))
    }

    /// `significance_score`: маршрут значимости по 15 триггерам
    /// (информационный инструмент, verdict `passed` не применим).
    /// Не метод: конфиг не нужен (clippy `unused_self` — `&self` осознанно
    /// отсутствует, в отличие от соседних инструментов).
    fn tool_significance_score(args: Value) -> std::result::Result<Value, CallError> {
        #[derive(Deserialize)]
        struct Args {
            /// Триггеры: карта «триггер → сработал» либо массив строк
            /// «name[=true|false]» ([`TriggersArg`]).
            triggers: TriggersArg,
        }
        let args: Args = parse_args(args, "significance_score")?;
        let triggers = args
            .triggers
            .into_map()
            .map_err(|e| CallError::invalid_params(format!("significance_score: {e}")))?;
        // T-04: незнакомое имя триггера — ошибка вызова, а не тихо
        // завышенный маршрут. Раньше «foo» попадал в unknown_triggers и
        // ОДНОВРЕМЕННО в счёт: пять выдуманных имён давали Critical.
        let unknown = control::unknown_trigger_names(&triggers);
        if !unknown.is_empty() {
            return Err(CallError::invalid_params(format!(
                "significance_score: {}",
                control::unknown_triggers_error(&unknown)
            )));
        }
        let s = control::significance_score(&triggers);
        Ok(json!({
            "score": s.score,
            "fired": s.fired,
            "route": s.route,
            "summary": format!("Score: {} → маршрут {}", s.score, s.route),
        }))
    }

    /// `significance_from_diff`: маршрут значимости, выведенный из git-диффа
    /// репозитория (S-1 anti-bypass, ADR-034) в fail-safe объединении с
    /// заявленными триггерами (`declared`) — детектор только добавляет.
    /// Информационный инструмент, verdict `passed` не применим.
    ///
    /// В ответе: `route`/`score` по объединённому множеству, источник каждого
    /// триггера (`sources`: declared/diff/declared+diff) и `undeclared` —
    /// найденные диффом, но не заявленные триггеры с файлами-основаниями
    /// (anti-bypass сигнал «заявлено vs видно по диффу»).
    async fn tool_significance_from_diff(
        &self,
        args: Value,
    ) -> std::result::Result<Value, CallError> {
        #[derive(Deserialize)]
        struct Args {
            /// Корень git-репозитория (дефолт — рабочий каталог процесса
            /// сервера, как у CLI `control score --from-diff`).
            path: Option<String>,
            /// Базовая точка диффа (`git diff BASE_REF...HEAD`); без неё —
            /// рабочее дерево против HEAD (staged + unstaged + untracked).
            base_ref: Option<String>,
            /// Заявленные агентом триггеры (карта «триггер → сработал», как у
            /// `significance_score`); объединяются с найденными по диффу.
            declared: Option<BTreeMap<String, bool>>,
        }
        let args: Args = parse_args(args, "significance_from_diff")?;
        let path = PathBuf::from(args.path.unwrap_or_else(|| ".".to_string()));
        let declared = args.declared.unwrap_or_default();
        // T-04: незнакомое имя в `declared` — ошибка вызова, как и в
        // `significance_score`: иначе «new_components» молча терялся бы, а
        // настоящий триггер остался бы незаявленным.
        let unknown_declared = control::unknown_trigger_names(&declared);
        if !unknown_declared.is_empty() {
            return Err(CallError::invalid_params(format!(
                "significance_from_diff: {}",
                control::unknown_triggers_error(&unknown_declared)
            )));
        }
        // Пороги маршрутов — из конфига сервера ([significance], ADR-034);
        // невалидные границы — понятный доменный сбой, не protocol error.
        let (fast_max, standard_max) = self
            .cfg
            .significance
            .limits()
            .map_err(|e| CallError::execution("significance_from_diff", e))?;
        let base_ref = args.base_ref;
        // T-05: глобы контрактов/компонентов — из секции `[significance]`
        // конфига сервера, а не зашиты в бинарь.
        let globs = self.cfg.significance.diff_globs();
        let diff = blocking("significance_from_diff", move || {
            control::detect_diff_triggers_with(&path, base_ref.as_deref(), &globs)
        })
        .await?;
        let scored = control::score_with_sources(&declared, &diff, fast_max, standard_max);

        let sources: serde_json::Map<String, Value> = scored
            .sources
            .iter()
            .map(|(t, s)| (t.clone(), json!(s.label())))
            .collect();
        // Основания срабатываний — строки вида «<trigger>: <файл-причина>»;
        // имена канонических триггеров не содержат «: », разбиение по первому
        // разделителю однозначно.
        let undeclared: Vec<Value> = scored
            .undeclared
            .iter()
            .map(|t| {
                let evidence: Vec<&str> = diff
                    .evidence
                    .iter()
                    .filter_map(|e| e.split_once(": "))
                    .filter(|(name, _)| name == t)
                    .map(|(_, reason)| reason)
                    .collect();
                json!({"trigger": t, "evidence": evidence})
            })
            .collect();
        let fired: Vec<String> = scored
            .significance
            .fired
            .iter()
            .map(|f| {
                scored
                    .sources
                    .get(f)
                    .map_or_else(|| f.clone(), |s| format!("{f} ({})", s.label()))
            })
            .collect();
        // Незнакомые имена отвергнуты выше — здесь пусто по построению;
        // поле остаётся в ответе для совместимости читателей.
        let unknown: Vec<&str> = Vec::new();
        let undeclared_note = if scored.undeclared.is_empty() {
            String::new()
        } else {
            format!(
                "; ВНИМАНИЕ — не заявлены, но видны по диффу: {}",
                scored.undeclared.join(", ")
            )
        };
        let summary = format!(
            "Score: {} ({}) → маршрут {}{}",
            scored.significance.score,
            if fired.is_empty() {
                "триггеров нет".to_string()
            } else {
                fired.join(", ")
            },
            scored.significance.route,
            undeclared_note,
        );
        Ok(json!({
            "route": scored.significance.route,
            "score": scored.significance.score,
            "fired": scored.significance.fired,
            "sources": sources,
            "undeclared": undeclared,
            "unknown_triggers": unknown,
            "summary": summary,
        }))
    }

    /// `trace_check`: позвенная трассируемость кейса → verdict
    /// (`passed` = нет находок severity error) + markdown-отчёт.
    async fn tool_trace_check(&self, args: Value) -> std::result::Result<Value, CallError> {
        #[derive(Deserialize)]
        struct Args {
            /// Корень кейса (каталог с `model/`).
            #[serde(alias = "path")]
            case: String,
        }
        let args: Args = parse_args(args, "trace_check")?;
        let case = PathBuf::from(args.case);
        let report = blocking("trace_check", move || trace::trace_check(&case)).await?;
        let levels: Vec<Value> = report
            .levels
            .iter()
            .map(|l| {
                json!({
                    "name": l.name,
                    "total": l.total,
                    "covered": l.covered,
                    "unverifiable": l.unverifiable,
                    "orphans": l.orphans,
                    "percent": (l.covered * 100).checked_div(l.total),
                })
            })
            .collect();
        let issues: Vec<Value> = report
            .issues
            .iter()
            .map(|i| {
                json!({
                    "severity": i.severity.to_string(),
                    "rule": i.rule,
                    "message": i.message,
                })
            })
            .collect();
        let errors = report
            .issues
            .iter()
            .filter(|i| i.severity == model::Severity::Error)
            .count();
        let passed = !report.has_errors();
        Ok(json!({
            "passed": passed,
            "entities": report.entities,
            "constraint_rules": report.constraint_rules,
            "spine_ads": report.spine_ads,
            "levels": levels,
            "issue_count": report.issues.len(),
            "error_count": errors,
            "warn_count": report.issues.len() - errors,
            "issues": issues,
            "report_markdown": trace::render_markdown(&report),
            "summary": format!(
                "Итог: {} (error: {errors}, warn: {})",
                if passed { "PASS" } else { "FAIL" },
                report.issues.len() - errors
            ),
        }))
    }

    /// `model_query`: список сущностей модели (с фильтром по типу) либо
    /// карточка сущности по `id` со связями и обратными ссылками.
    async fn tool_model_query(&self, args: Value) -> std::result::Result<Value, CallError> {
        #[derive(Deserialize)]
        struct Args {
            /// Каталог модели (дефолт `model` от cwd процесса сервера).
            #[serde(alias = "path")]
            dir: Option<String>,
            /// ID сущности — карточка (без `id` — список).
            id: Option<String>,
            /// Фильтр списка по типу (`cmp`, `adr`, … или префикс `CMP`).
            #[serde(rename = "type")]
            kind: Option<String>,
        }
        let args: Args = parse_args(args, "model_query")?;
        let kind = match &args.kind {
            Some(raw) => {
                let norm = raw.trim().to_ascii_lowercase();
                model::EntityKind::from_type_str(&norm)
                    .or_else(|| model::EntityKind::from_prefix(&raw.trim().to_ascii_uppercase()))
                    .ok_or_else(|| {
                        CallError::invalid_params(format!(
                            "model_query: неизвестный тип '{raw}' (допустимы: {})",
                            model::EntityKind::type_names().join(", ")
                        ))
                    })
                    .map(Some)?
            }
            None => None,
        };
        let dir = PathBuf::from(args.dir.unwrap_or_else(|| "model".into()));
        let id = args.id;
        blocking("model_query", move || {
            // Толерантная загрузка (E3): ответ по валидному подмножеству +
            // поле `load_issues` в JSON.
            let m = model::load_model_tolerant(&dir)?;
            model_query_value(&m, id.as_deref(), kind, &dir)
        })
        .await
    }

    /// `rubric_run`: оценка документа рубрикой LLM-судьёй (ADR-004).
    ///
    /// Без доступного API-ключа провайдера — JSON-RPC `-32603` с подсказкой,
    /// какой env/файл настроить (содержимое ключа не читается в ответ).
    async fn tool_rubric_run(&self, args: Value) -> std::result::Result<Value, CallError> {
        #[derive(Deserialize)]
        struct Args {
            /// Рубрика: имя в каталоге рубрик (`paths.rubrics_dir`) или путь к YAML.
            rubric: String,
            /// Путь к оцениваемому документу (md/txt).
            target: Option<String>,
            /// Текст документа inline (альтернатива `target`).
            target_text: Option<String>,
            /// Модель-судья (имя из `[models]`, дефолт — `default_model`).
            model: Option<String>,
            /// Рабочий каталог клиента: относительный `target` резолвится от
            /// него (паттерн мостовых инструментов; по умолчанию — cwd
            /// процесса сервера).
            cwd: Option<String>,
        }
        let args: Args = parse_args(args, "rubric_run")?;
        let text = match (args.target, args.target_text) {
            (Some(path), None) => {
                let raw = PathBuf::from(path);
                let path = match &args.cwd {
                    Some(cwd) if !raw.is_absolute() => PathBuf::from(cwd).join(raw),
                    _ => raw,
                };
                blocking("rubric_run", move || {
                    std::fs::read_to_string(&path)
                        .map_err(|e| crate::error::HarnessError::io(&path, e))
                })
                .await?
            }
            (None, Some(text)) => text,
            _ => {
                return Err(CallError::invalid_params(
                    "rubric_run: укажите ровно один из аргументов 'target' / 'target_text'".into(),
                ));
            }
        };
        let model_name = args.model.unwrap_or_else(|| self.cfg.default_model.clone());
        let model_cfg = self.cfg.models.get(&model_name).ok_or_else(|| {
            CallError::invalid_params(format!(
                "rubric_run: модель '{model_name}' не настроена в [models] конфига"
            ))
        })?;
        // kind="cli": судья — внешний CLI-харнесс (Claude Code, Codex, …),
        // уже авторизованный на машине пользователя: собственный API-ключ
        // Spine не нужен, предпроверка пропускается.
        let cli_backed = model_cfg.kind.as_deref() == Some("cli");
        if !cli_backed && !api_key_available(model_cfg) {
            return Err(CallError::Protocol {
                code: INTERNAL_ERROR,
                message: format!(
                    "rubric_run: нет API-ключа провайдера '{model_name}' — установите переменную \
                     окружения '{}' или положите ключ в файл {:?} (см. README «API keys»)",
                    model_cfg.api_key_env, model_cfg.api_key_file
                ),
            });
        }
        let rubric_path = resolve_rubric(&self.cfg.paths.rubrics_dir(), &args.rubric);
        let rub = blocking("rubric_run", move || rubric::load(&rubric_path)).await?;
        let registry = crate::llm::LlmRegistry::from_config(&self.cfg)
            .map_err(|e| CallError::execution("rubric_run", e))?;
        let judge = registry
            .get(&model_name)
            .map_err(|e| CallError::execution("rubric_run", e))?;
        let report = rubric::evaluate_with_options(&rub, &text, judge.as_ref(), &self.cfg.judge)
            .await
            .map_err(|e| CallError::execution("rubric_run", e))?;
        // Отчёт НЕ пишется на диск (read-only-семантика сервера, ADR-008) —
        // markdown возвращается в verdict'е.
        Ok(json!({
            "rubric": report.rubric_name,
            "judge_model": report.judge_model,
            "judge_samples": report.judge_samples,
            "weighted_total": report.weighted_total,
            "verdict": report.verdict,
            "scores": report.scores,
            "report_markdown": report.to_markdown(),
            "summary": format!(
                "Рубрика '{}': {:.2}/5 (судья {})",
                report.rubric_name, report.weighted_total, report.judge_model
            ),
        }))
    }

    /// `rubric_prompt`: split-judge, фаза 1 (без LLM у сервера): промпты
    /// судьи (system+user, те же что у `rubric_run`), JSON-схема ответа,
    /// которую ждёт парсер [`rubric::parse_judge_response`], и параметры
    /// прогона из `[judge]` (k сэмплов). Хост исполняет промпт k раз своей
    /// моделью и возвращает сырые ответы в `rubric_verify`.
    async fn tool_rubric_prompt(&self, args: Value) -> std::result::Result<Value, CallError> {
        #[derive(Deserialize)]
        struct Args {
            /// Рубрика: имя в каталоге рубрик (`paths.rubrics_dir`) или путь к YAML.
            rubric: String,
            /// Путь к оцениваемому документу (md/txt).
            target: Option<String>,
            /// Текст документа inline (альтернатива `target`).
            target_text: Option<String>,
            /// В MCP-режиме не поддерживается: генерация динамической
            /// рубрики требует LLM на стороне сервера.
            dynamic_subject: Option<String>,
        }
        let args: Args = parse_args(args, "rubric_prompt")?;
        if args.dynamic_subject.is_some() {
            return Err(CallError::Execution(
                "rubric_prompt: dynamic_subject требует LLM на стороне сервера — в MCP-режиме \
                 её нет; сгенерируйте динамическую рубрику моделью хоста и передайте путь \
                 к её YAML в аргументе 'rubric'"
                    .into(),
            ));
        }
        let text = rubric_target_text("rubric_prompt", args.target, args.target_text).await?;
        rubric::check_target_len(&text).map_err(|e| CallError::execution("rubric_prompt", e))?;
        let rubric_path = resolve_rubric(&self.cfg.paths.rubrics_dir(), &args.rubric);
        let rub = blocking("rubric_prompt", move || rubric::load(&rubric_path)).await?;
        let samples = self.cfg.judge.samples.max(1);
        // Запоминаем выданный промпт: `rubric_verify` в той же сессии назовёт
        // хэш промпта и счётчик вызовов до судейства (ADR-048). Оценку могли
        // собрать и в другой сессии — тогда `prompt_issued_in_session: false`.
        let system_prompt = rubric::judge_system_prompt(&rub);
        let user_prompt = rubric::judge_user_prompt(&rub, &text);
        let prompt_sha = crate::judge::prompt_sha256(&system_prompt, &user_prompt);
        let session_id = {
            let mut session = self.session();
            session.record_prompt(
                &crate::judge::prompt_key(&rub.name, &text),
                prompt_sha.clone(),
            );
            session.id().to_string()
        };
        Ok(json!({
            "rubric": rub.name,
            "criteria": rub.criteria.len(),
            "system_prompt": system_prompt,
            "user_prompt": user_prompt,
            "response_json_schema": judge_response_schema(&rub),
            "prompt_sha256": prompt_sha,
            "session_id": session_id,
            "judge_config": {
                "samples": samples,
                "thinking": self.cfg.judge.thinking,
                "unstable_stdev": self.cfg.judge.unstable_stdev,
                "evidence_min_similarity": self.cfg.judge.evidence_min_similarity,
            },
            "instructions": "Исполните system+user промпт samples раз независимыми запросами \
                             своей модели; сырые ответы (как есть, без правок) передайте массивом \
                             'answers' в rubric_verify с ТЕМИ ЖЕ rubric и target/target_text.",
            "summary": format!(
                "Промпт судьи по рубрике '{}' собран ({} критериев; нужно независимых ответов: {samples})",
                rub.name,
                rub.criteria.len(),
            ),
        }))
    }

    /// `rubric_verify`: split-judge, фаза 2 (без LLM у сервера): разбор
    /// сырых ответов хоста тем же парсером, что у встроенного судьи, и
    /// сборка отчёта существующим [`rubric::build_report`] — медиана
    /// сэмплов, σ → `unstable`, цитата → `evidence_not_found` (для проверки
    /// цитат нужен тот же target). Битые ответы вызов не роняют: они
    /// считаются в `answers.dropped`; ноль валидных — доменный isError.
    async fn tool_rubric_verify(&self, args: Value) -> std::result::Result<Value, CallError> {
        #[derive(Deserialize)]
        struct Args {
            /// Рубрика: имя в каталоге рубрик или путь к YAML (та же, что судилась).
            rubric: String,
            /// Путь к оцениваемому документу (тот же, что судился).
            target: Option<String>,
            /// Текст документа inline (альтернатива `target`; тот же, что судился).
            target_text: Option<String>,
            /// Сырые ответы модели хоста на промпт `rubric_prompt` (JSON судьи).
            answers: Vec<String>,
            /// Метка судьи для отчёта (имя модели хоста; дефолт — external).
            model: Option<String>,
            /// Метка судьи, перекрывающая `model` (anti-bias «автор = судья»:
            /// ответы судила не дефолтная модель хоста — фиксируйте фактическую;
            /// эхо — строка «Судья: <модель>» в markdown-отчёте).
            judge_model: Option<String>,
            /// Модель-АВТОР документа (Н7, ADR-042): `judge_model == author_model`
            /// — судья судил свою же работу; попадает в отчёт и в находку
            /// `judge_is_author` составляющей гейта `decision_quality`.
            author_model: Option<String>,
        }
        let args: Args = parse_args(args, "rubric_verify")?;
        if args.answers.is_empty() {
            return Err(CallError::invalid_params(
                "rubric_verify: массив 'answers' пуст — нужны сырые ответы модели хоста".into(),
            ));
        }
        if args.answers.len() > MAX_VERIFY_ANSWERS {
            return Err(CallError::invalid_params(format!(
                "rubric_verify: ответов {} при лимите {MAX_VERIFY_ANSWERS} — \
                 судье достаточно k сэмплов из judge_config",
                args.answers.len()
            )));
        }
        let target_path = args.target.clone();
        let text = rubric_target_text("rubric_verify", args.target, args.target_text).await?;
        rubric::check_target_len(&text).map_err(|e| CallError::execution("rubric_verify", e))?;
        let rubric_path = resolve_rubric(&self.cfg.paths.rubrics_dir(), &args.rubric);
        let rub = blocking("rubric_verify", move || rubric::load(&rubric_path)).await?;
        let total = args.answers.len();
        let mut runs = Vec::with_capacity(total);
        let mut dropped = 0usize;
        // Сырые ответы сохраняются как есть — и разобранные, и отброшенные
        // (J2, ADR-048): по ним отчёт пересобирается и сверяется, поэтому
        // «поправить балл в отчёте» перестаёт быть незаметным.
        let mut raw_inputs = Vec::with_capacity(total);
        for raw in &args.answers {
            let parsed = rubric::parse_judge_response(raw);
            let is_dropped = parsed.is_err();
            match parsed {
                Ok(parsed) => runs.push(parsed),
                Err(_) => dropped += 1,
            }
            raw_inputs.push(crate::judge::RawAnswerInput {
                text: raw.clone(),
                dropped: is_dropped,
            });
        }
        if runs.is_empty() {
            return Err(CallError::Execution(format!(
                "rubric_verify: ни один из {total} ответов не разобран как JSON судьи \
                 ({{\"scores\": [{{\"criterion_id\": \"...\", \"score\": 1, \"rationale\": \
                 \"Цитата: \\\"...\\\". ...\"}}], \"verdict\": \"...\"}}) — передайте сырые \
                 ответы модели как есть, без правок"
            )));
        }
        let judge_model = args
            .judge_model
            .or(args.model)
            .unwrap_or_else(|| "external (split-judge)".into());
        let report = rubric::build_report(&rub, &judge_model, &runs, &text, &self.cfg.judge)
            .map_err(|e| CallError::execution("rubric_verify", e))?;
        // Машиночитаемый отчёт (Н7, ADR-042) — то, что читает составляющая
        // гейта `decision_quality`. Пишется только под `--rw`: read-only
        // контур MCP не имеет права оставлять след в рабочем каталоге.
        let mut artifact_note = None;
        let mut provenance_out: Option<crate::judge::RubricProvenance> = None;
        // Автор — из шапки документа, если он там записан: значение из
        // документа сильнее аргумента вызова (J3, ADR-048). Для inline-текста
        // шапки нет, поэтому решает аргумент.
        let target_file = target_path
            .as_deref()
            .map(PathBuf::from)
            .map(|p| p.canonicalize().unwrap_or(p))
            .filter(|p| p.is_file());
        let choice = crate::judge::choose_author(
            target_file
                .as_deref()
                .and_then(crate::adr_registry::author_model_of),
            args.author_model.clone(),
        );
        if let Some(target) = target_path.as_deref() {
            let path = PathBuf::from(target);
            let abs = path.canonicalize().unwrap_or(path);
            if abs.is_file() {
                let repo = crate::rubric::repo_root_of(&abs);
                // Происхождение: заявленные метки, выданный промпт, сессия,
                // оператор из git-конфига — то, что механика знает о судействе
                // хостовой моделью (ADR-048). Какая модель отвечала, она не
                // знает: в отчёте это сказано формулировкой паспорта.
                let issued = self
                    .session()
                    .issued_prompt(&crate::judge::prompt_key(&rub.name, &text));
                let mut provenance = {
                    let session = self.session();
                    let mut prov = crate::judge::RubricProvenance::declared(
                        session.host(),
                        Some(session.id().to_string()),
                    );
                    prov.session_calls_before = issued
                        .as_ref()
                        .map_or_else(|| session.calls(), |i| i.calls_before);
                    prov
                };
                if let Some(issued) = issued {
                    provenance.prompt_sha256 = Some(issued.sha256);
                    provenance.prompt_issued_in_session = true;
                }
                if self.cfg.judge.record_operator {
                    provenance.operator = crate::judge::operator(&repo);
                }
                provenance_out = Some(provenance.clone());
                if self.mode.allows_write() {
                    let extras = crate::rubric::ArtifactExtras {
                        provenance: Some(provenance),
                        author_source: Some(choice.source.clone()),
                        author_model_declared: choice.declared.clone(),
                        raw_answers: raw_inputs.clone(),
                    };
                    match crate::rubric::write_artifact_with(
                        &repo,
                        &report,
                        Some(&abs),
                        choice.author.as_deref(),
                        &extras,
                    ) {
                        Ok(p) => artifact_note = Some(p.display().to_string()),
                        Err(e) => {
                            artifact_note = Some(format!("не записан: {e}"));
                        }
                    }
                } else {
                    artifact_note =
                        Some("не записан: контур MCP только для чтения (нужен `--rw`)".to_string());
                }
            }
        }
        let mut out = json!({
            "rubric": report.rubric_name,
            "judge_model": report.judge_model,
            "author_model": choice.author,
            "author_source": choice.source,
            "artifact": artifact_note,
            "provenance": provenance_out,
            "judge_samples": report.judge_samples,
            "weighted_total": report.weighted_total,
            "verdict": report.verdict,
            "scores": report.scores,
            "report_markdown": report.to_markdown(),
            "answers": {
                "total": total,
                "valid": runs.len(),
                "dropped": dropped,
            },
            "summary": format!(
                "Рубрика '{}': {:.2}/5 (судья {}, валидных ответов {}/{total})",
                report.rubric_name,
                report.weighted_total,
                report.judge_model,
                runs.len(),
            ),
        });
        if dropped > 0 {
            out["warning"] = json!(format!(
                "{dropped} из {total} ответов не разобраны как JSON судьи и отброшены; \
                 отчёт построен по {} валидным",
                runs.len()
            ));
        }
        Ok(out)
    }

    /// `kb_search`: поиск по локальной базе знаний харнесса
    /// (`knowledge.dirs` из конфига arch, а не каталоги репозитория клиента).
    /// Ядро — [`kb::search`] (то же, что у агентного инструмента);
    /// MCP-слой только формирует JSON из хитов.
    async fn tool_kb_search(&self, args: Value) -> std::result::Result<Value, CallError> {
        #[derive(Deserialize)]
        struct Args {
            /// Поисковый запрос: термины через пробел.
            query: String,
            /// Максимум хитов (по умолчанию 10, не больше 20).
            limit: Option<usize>,
        }
        let args: Args = parse_args(args, "kb_search")?;
        let limit = args
            .limit
            .unwrap_or(KB_SEARCH_DEFAULT_LIMIT)
            .min(KNOWLEDGE_MAX_HITS);
        let hits = kb::search(
            &self.cfg.knowledge.dirs,
            &self.cfg.knowledge.extensions,
            &args.query,
            limit,
        )
        .await
        .map_err(|e| CallError::execution("kb_search", e))?;
        let summary = if hits.is_empty() {
            format!(
                "По запросу «{}» в базе знаний ничего не найдено.",
                args.query
            )
        } else {
            format!("По запросу «{}» найдено хитов: {}", args.query, hits.len())
        };
        Ok(json!({
            "query": args.query,
            "count": hits.len(),
            "hits": hits,
            "summary": summary,
        }))
    }

    /// `skill_search`: поиск по библиотеке скиллов (`plugins.dirs` из конфига
    /// arch). Ядро — [`plugin::discover`] + [`plugin::search`] (агентный
    /// инструмент `skill_search` использует те же функции).
    async fn tool_skill_search(&self, args: Value) -> std::result::Result<Value, CallError> {
        #[derive(Deserialize)]
        struct Args {
            /// Поисковый запрос.
            query: String,
            /// Максимум результатов (по умолчанию 8, не больше 20).
            limit: Option<usize>,
        }
        let args: Args = parse_args(args, "skill_search")?;
        let limit = args
            .limit
            .unwrap_or(SKILL_SEARCH_DEFAULT_LIMIT)
            .min(KNOWLEDGE_MAX_HITS);
        let dirs = self.cfg.plugins.dirs.clone();
        let query = args.query;
        blocking("skill_search", move || -> Result<Value> {
            let plugins = plugin::discover(&dirs);
            let total: usize = plugins.iter().map(|p| p.skills.len()).sum();
            let hits = plugin::search(&plugins, &query, limit);
            let summary = if hits.is_empty() {
                format!("по запросу '{query}' ничего не найдено (скиллов в индексе: {total})")
            } else {
                format!("по запросу '{query}' найдено скиллов: {}", hits.len())
            };
            Ok(json!({
                "query": query,
                "count": hits.len(),
                "hits": hits.iter().map(|h| json!({
                    "name": h.meta.name,
                    "plugin": h.meta.plugin,
                    "score": h.score,
                    "description": h.meta.description,
                    "snippet": h.snippet,
                })).collect::<Vec<_>>(),
                "summary": summary,
            }))
        })
        .await
    }

    /// `skill_load`: полный текст скилла по точному имени. Ядро —
    /// [`plugin::skill_by_name`] + [`plugin::load_skill`]; лимит тела —
    /// как у агентного инструмента (`SKILL_TEXT_MAX_CHARS`).
    async fn tool_skill_load(&self, args: Value) -> std::result::Result<Value, CallError> {
        #[derive(Deserialize)]
        struct Args {
            /// Точное имя скилла.
            name: String,
        }
        let args: Args = parse_args(args, "skill_load")?;
        let dirs = self.cfg.plugins.dirs.clone();
        let name = args.name;
        blocking("skill_load", move || -> Result<Value> {
            let plugins = plugin::discover(&dirs);
            let Some(meta) = plugin::skill_by_name(&plugins, &name) else {
                return Err(crate::error::HarnessError::Tool(format!(
                    "скилл '{name}' не найден; сначала skill_search"
                )));
            };
            let mut text = plugin::load_skill(meta)?;
            if text.chars().count() > SKILL_TEXT_MAX_CHARS {
                let cut: String = text.chars().take(SKILL_TEXT_MAX_CHARS).collect();
                text = format!("{cut}\n… [усечено: {SKILL_TEXT_MAX_CHARS} символов]");
            }
            Ok(json!({
                "name": meta.name,
                "plugin": meta.plugin,
                "description": meta.description,
                "path": meta.path,
                "body": text,
                "summary": format!("Скилл '{}' загружен (плагин '{}').", meta.name, meta.plugin),
            }))
        })
        .await
    }

    /// `mermaid_render`: диаграмма mermaid → ASCII-арт. Вход — `code`
    /// (исходник) или `path` (файл относительно cwd сервера, как и пути
    /// контрольных инструментов). Ядро — [`mermaid::render`] /
    /// [`mermaid::read_diagram_source`] (агентный `mermaid_render`).
    async fn tool_mermaid_render(&self, args: Value) -> std::result::Result<Value, CallError> {
        #[derive(Deserialize)]
        struct Args {
            /// Исходный код mermaid-диаграммы.
            code: Option<String>,
            /// Путь к .mmd-файлу (резолвится от cwd сервера).
            path: Option<String>,
        }
        let args: Args = parse_args(args, "mermaid_render")?;
        match args.code {
            Some(code) if !code.trim().is_empty() => {
                blocking("mermaid_render", move || mermaid_render_value(&code)).await
            }
            _ => match args.path {
                Some(path) => {
                    let path = PathBuf::from(path);
                    blocking("mermaid_render", move || {
                        let code = mermaid::read_diagram_source(&path)?;
                        mermaid_render_value(&code)
                    })
                    .await
                }
                None => Err(CallError::Execution(
                    "mermaid_render: нужен аргумент 'code' (исходник) или 'path' (файл)".into(),
                )),
            },
        }
    }

    /// `rules_suggest`: кандидатные fitness-правила из содержательных
    /// пробелов кейса (детекторы [`crate::rules_suggest`]: EARS, таймауты
    /// контрактов, REQ→TASK, RTO/RPO→ADR, аудит операторских действий).
    /// Информационный инструмент (read-only эвристики), verdict `passed`
    /// не применим.
    async fn tool_rules_suggest(&self, args: Value) -> std::result::Result<Value, CallError> {
        #[derive(Deserialize)]
        struct Args {
            /// Корень кейса (каталог с docs/, model/, .arch-handoff/).
            path: String,
            /// Рабочий каталог клиента: относительный `path` резолвится от
            /// него (паттерн мостовых инструментов; по умолчанию — cwd
            /// процесса сервера).
            cwd: Option<String>,
        }
        let args: Args = parse_args(args, "rules_suggest")?;
        let raw = PathBuf::from(args.path);
        let case = match &args.cwd {
            Some(cwd) if !raw.is_absolute() => PathBuf::from(cwd).join(raw),
            _ => raw,
        };
        let case_display = case.display().to_string();
        let report = blocking("rules_suggest", move || rules_suggest::suggest(&case)).await?;
        Ok(json!({
            "case": case_display,
            "candidate_count": report.candidates.len(),
            "candidates": report.candidates,
            "report_markdown": rules_suggest::render_markdown(&report),
            "summary": report.summary,
        }))
    }

    /// Метрика доверия к контуру (W4): шкала 1–5 с якорями и доказательствами.
    /// Ничего не блокирует и ничего не пишет.
    async fn tool_trust_report(&self, args: Value) -> std::result::Result<Value, CallError> {
        #[derive(Deserialize)]
        struct Args {
            /// Репозиторий или кейс (по умолчанию — каталог вызова).
            path: Option<String>,
            /// Рабочий каталог клиента: относительный `path` резолвится от него.
            cwd: Option<String>,
        }
        let args: Args = parse_args(args, "trust_report")?;
        let raw = args.path.map_or_else(|| PathBuf::from("."), PathBuf::from);
        let repo = match &args.cwd {
            Some(cwd) if !raw.is_absolute() => PathBuf::from(cwd).join(raw),
            _ => raw,
        };
        let cfg = self.cfg.clone();
        let repo_for_run = repo.clone();
        let trust = blocking("trust_report", move || {
            crate::trust::assess(&repo_for_run, &cfg)
        })
        .await?;
        let mut out = crate::trust::to_json(&trust);
        out["report_markdown"] = Value::String(crate::trust::render(&trust));
        Ok(out)
    }

    /// Паспорт вердикта (W1): прогон гейта + страница «что зелёный НЕ
    /// означает». Инструмент чтения: ничего не пишет и решения не принимает —
    /// возвращает тот же вердикт, что `arch-be gate`, и его границы.
    async fn tool_verdict_explain(&self, args: Value) -> std::result::Result<Value, CallError> {
        #[derive(Deserialize)]
        struct Args {
            /// Репозиторий (по умолчанию — каталог вызова).
            path: Option<String>,
            /// Маршрут: auto (по умолчанию) | fast | standard | critical.
            route: Option<String>,
            /// База git для диффа и сравнения правил.
            base: Option<String>,
            /// Файл ограничений (по умолчанию <repo>/.arch-handoff/CONSTRAINTS.yaml).
            constraints: Option<String>,
            /// Рабочий каталог клиента: относительный `path` резолвится от него.
            cwd: Option<String>,
        }
        let args: Args = parse_args(args, "verdict_explain")?;
        let raw = args.path.map_or_else(|| PathBuf::from("."), PathBuf::from);
        let repo = match &args.cwd {
            Some(cwd) if !raw.is_absolute() => PathBuf::from(cwd).join(raw),
            _ => raw,
        };
        let route = match args.route.as_deref().unwrap_or("auto").trim() {
            "auto" | "" => None,
            other => Some(
                other
                    .parse::<crate::control::Route>()
                    .map_err(CallError::invalid_params)?,
            ),
        };
        let base = args.base;
        let constraints = args.constraints.map(PathBuf::from);
        let limits = self
            .cfg
            .significance
            .limits()
            .map_err(|e| CallError::Execution(format!("verdict_explain: {e}")))?;
        let requirements = crate::gate::GateRequirements::from_config(&self.cfg.gate);
        let options = crate::gate::GateOptions::from_config(&self.cfg);
        let repo_for_run = repo.clone();
        let report = blocking("verdict_explain", move || {
            crate::gate::run_opts(
                &repo_for_run,
                route,
                base.as_deref(),
                constraints.as_deref(),
                limits,
                &requirements,
                &options,
            )
        })
        .await?;
        let passport = crate::passport::Passport::build(&report, &repo);
        let mut out = passport.to_json();
        // Вердикт рядом с паспортом — тот же прогон, не второй: паспорт без
        // вердикта читался бы как самостоятельное суждение.
        out["verdict_envelope"] = report.envelope_json();
        out["report_markdown"] = Value::String(passport.render());
        out["summary"] = Value::String(crate::passport::summary_line(&passport));
        Ok(out)
    }
}

/// Текст и описание плейбука-промпта: сначала пользовательская копия из
/// `plugins.dirs` (та же логика, что у `skill_load`, — правки пользователя
/// в силе), иначе встроенный ассет (чистая машина без `arch-be init`).
/// Ошибка чтения пользовательской копии не фатальна — откат на встроенный
/// текст: сервер не падает из-за одного битого файла.
fn resolve_playbook(dirs: &[PathBuf], pb: &PlaybookPrompt) -> (String, String) {
    let plugins = plugin::discover(dirs);
    if let Some(meta) = plugin::skill_by_name(&plugins, pb.name) {
        if let Ok(text) = plugin::load_skill(meta) {
            return (text, meta.description.clone());
        }
    }
    let description = plugin::parse_frontmatter_text(pb.embedded)
        .map(|(_, d)| d)
        .unwrap_or_default();
    (pb.embedded.to_string(), description)
}

/// Рендерит код диаграммы в JSON-ответ `mermaid_render` (общий для inline-кода
/// и файла): арт + вид диаграммы. Ошибки парсера — [`HarnessError::Mermaid`]
/// с номером строки, как у агентного инструмента.
fn mermaid_render_value(code: &str) -> Result<Value> {
    let art = mermaid::render(code)?;
    let kind = match mermaid::diagram_kind(code) {
        mermaid::DiagramKind::Flowchart => "flowchart",
        mermaid::DiagramKind::Sequence => "sequenceDiagram",
        mermaid::DiagramKind::Er => "erDiagram",
        mermaid::DiagramKind::C4 => "c4",
        mermaid::DiagramKind::C4Unsupported | mermaid::DiagramKind::Unknown => {
            // `render` выше уже отклонил эти виды — ветка недостижима, но
            // DiagramKind не знает об успехе; держимся консервативно.
            "unknown"
        }
    };
    Ok(json!({
        "kind": kind,
        "art": art,
        "summary": format!("Диаграмма ({kind}) отрендерена в ASCII-арт."),
    }))
}

/// Строит JSON-ответ `model_query`: список сущностей либо карточка по `id`.
fn model_query_value(
    m: &model::Model,
    id: Option<&str>,
    kind: Option<model::EntityKind>,
    dir: &Path,
) -> Result<Value> {
    // E3: сущности, пропущенные при толерантной загрузке (пусто — модель
    // разобралась целиком).
    let load_issues = json!(m.load_issues);
    if let Some(id) = id {
        let e = m.get(id).ok_or_else(|| {
            crate::error::HarnessError::Model(format!(
                "model_query: сущность '{id}' не найдена (всего сущностей: {})",
                m.entities.len()
            ))
        })?;
        let mut links = serde_json::Map::new();
        for lk in model::LinkKind::ALL {
            let targets = e.link_targets(lk);
            if !targets.is_empty() {
                links.insert(lk.field_name().to_string(), json!(targets));
            }
        }
        let referents: Vec<Value> = m
            .referents(&e.id)
            .iter()
            .map(|(src, lk)| json!({"id": src.id, "title": src.title, "via": lk.field_name()}))
            .collect();
        return Ok(json!({
            "entity": {
                "id": e.id,
                "kind": e.kind.type_str(),
                "kind_ru": e.kind.title_ru(),
                "status": e.status,
                "title": e.title,
                "date": e.date,
                "verification": e.verification,
                "file": e.file,
                "links": links,
                "referents": referents,
                "body": e.body,
            },
            "card": model::card(m, e),
            "load_issues": load_issues,
        }));
    }
    let entities: Vec<Value> = m
        .entities
        .iter()
        .filter(|e| kind.is_none_or(|k| e.kind == k))
        .map(|e| {
            let links: usize = model::LinkKind::ALL
                .iter()
                .map(|k| e.link_targets(*k).len())
                .sum();
            json!({
                "id": e.id,
                "kind": e.kind.type_str(),
                "status": e.status,
                "title": e.title,
                "links": links,
            })
        })
        .collect();
    Ok(json!({
        "dir": dir,
        "total": entities.len(),
        "entities": entities,
        "load_issues": load_issues,
    }))
}

/// Резолвит рубрику: существующий путь → как есть; имя в каталоге рубрик →
/// `<dir>/<name>` или `<dir>/<name>.yaml` (семантика `resolve_asset` из CLI).
fn resolve_rubric(dir: &Path, name: &str) -> PathBuf {
    let as_path = PathBuf::from(name);
    if as_path.is_file() {
        return as_path;
    }
    let in_dir = dir.join(name);
    if in_dir.is_file() {
        return in_dir;
    }
    dir.join(format!("{name}.yaml"))
}

/// Разбор пары `target`/`target_text` инструментов рубрик (подход
/// `rubric_run`): ровно один из двух; путь читается на blocking-пуле.
async fn rubric_target_text(
    tool: &str,
    target: Option<String>,
    target_text: Option<String>,
) -> std::result::Result<String, CallError> {
    match (target, target_text) {
        (Some(path), None) => {
            let path = PathBuf::from(path);
            blocking(tool, move || {
                std::fs::read_to_string(&path).map_err(|e| crate::error::HarnessError::io(&path, e))
            })
            .await
        }
        (None, Some(text)) => Ok(text),
        _ => Err(CallError::invalid_params(format!(
            "{tool}: укажите ровно один из аргументов 'target' / 'target_text'"
        ))),
    }
}

/// JSON-схема ответа судьи, как её ждёт [`rubric::parse_judge_response`]
/// (split-judge: хост подставляет её в структурированный вывод своей модели;
/// парсер терпимо принимает балл и строкой — схема фиксирует канону).
fn judge_response_schema(rubric: &rubric::Rubric) -> Value {
    let ids: Vec<&str> = rubric.criteria.iter().map(|c| c.id.as_str()).collect();
    json!({
        "type": "object",
        "properties": {
            "scores": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "criterion_id": {"type": "string", "enum": ids},
                        "score": {
                            "type": "integer",
                            "minimum": 1,
                            "maximum": rubric.scale_max,
                        },
                        "rationale": {
                            "type": "string",
                            "description": "При балле ≥ 2 начинается с «Цитата: \"<дословный фрагмент текста>\"» — цитата проверяется механически",
                        },
                    },
                    "required": ["criterion_id", "score", "rationale"],
                },
            },
            "verdict": {"type": "string"},
        },
        "required": ["scores", "verdict"],
    })
}

/// Доступен ли API-ключ провайдера (та же семантика, что у резолва ключа
/// в `llm::openai_compat`: env непустая после trim; файл с `~`-раскрытием
/// читается и непуст). Содержимое ключа в ответы/логи не попадает.
fn api_key_available(mc: &ModelConfig) -> bool {
    if let Ok(raw) = std::env::var(&mc.api_key_env) {
        if !raw.trim().is_empty() {
            return true;
        }
    }
    if let Some(path) = &mc.api_key_file {
        let expanded = match path.strip_prefix("~/") {
            Some(rest) => dirs::home_dir().map_or_else(|| PathBuf::from(path), |h| h.join(rest)),
            None => PathBuf::from(path),
        };
        if let Ok(raw) = std::fs::read_to_string(&expanded) {
            if !raw.trim().is_empty() {
                return true;
            }
        }
    }
    false
}

/// Спецификации ручных инструментов для `tools/list` (имена и аргументы —
/// ADR-008; порядок первых десяти зафиксирован тестами).
// Декларативная таблица: дробление на fn-по-инструменту ухудшит обзорность.
#[expect(clippy::too_many_lines, reason = "декларативная таблица спецификаций")]
/// Схемы ручных инструментов. Правило поверхности: поле, которое разбирает
/// реализация, ОБЯЗАНО быть в схеме — иначе `reject_unknown_args` (Н8)
/// отвергнет вызов, который инструмент умеет обслужить, и поле станет
/// недостижимым для всех клиентов (Н13: `rubric_verify.author_model`).
/// Заодно тестами зафиксирован обратный край: объявленный обязательный
/// аргумент обязан разбираться.
fn tool_specs() -> Vec<Value> {
    let read_only = json!({
        "readOnlyHint": true,
        "destructiveHint": false,
        "idempotentHint": true,
        "openWorldHint": false,
    });
    vec![
        json!({
            "name": "spine_lint",
            "description": "Линтер ARCHITECTURE-SPINE.md: дубли AD-id, пустые/отсутствующие \
                            Binds/Prevents/Rule, заглушки (TODO/TBD), непиннутые версии, ссылки \
                            на несуществующие AD. Verdict: passed=false (есть находки error) — \
                            spine нарушен, отказать изменению с перечнем находок",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Путь к ARCHITECTURE-SPINE.md"}
                },
                "required": ["path"],
            },
            "annotations": read_only,
        }),
        json!({
            "name": "fitness_check",
            "description": "Fitness-контроль репозитория по CONSTRAINTS.yaml: must_contain / \
                            must_not_contain (regex по glob), file_exists, command_succeeds \
                            (с таймаутом). Вызывать ПЕРЕД коммитом: verdict passed=false — \
                            изменение нарушает архитектурные правила (AD-*), отказать и \
                            перечислить находки",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Корень репозитория"},
                    "constraints": {
                        "type": "string",
                        "description": "Путь к CONSTRAINTS.yaml (по умолчанию <repo>/.arch-handoff/CONSTRAINTS.yaml)",
                    },
                    "base": {
                        "type": "string",
                        "description": "База git для сверки состава правил (анти-ослабление, П5): по умолчанию merge-base с основной веткой, иначе HEAD",
                    },
                },
                "required": ["path"],
            },
            "annotations": read_only,
        }),
        json!({
            "name": "significance_score",
            "description": "Architecture Significance Score по 15 триггерам → маршрут изменения: \
                            Fast (0–1), Standard (2–4), Critical (5+ или критические триггеры \
                            security_boundary_change / irreversible_migration / \
                            criticality_or_exception)",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "triggers": {
                        "oneOf": [
                            {"type": "object", "additionalProperties": {"type": "boolean"}},
                            {"type": "array", "items": {"type": "string"}},
                        ],
                        "description": "Триггеры: карта «триггер → true/false» (ключи — из 15 канонических) ЛИБО массив строк \"name=true\" / \"name=false\" / голое \"name\" (= true). Незнакомое имя — ошибка вызова (-32602) с перечнем канонических триггеров и ближайшим совпадением: выдуманный триггер не поднимает маршрут",
                    }
                },
                "required": ["triggers"],
            },
            "annotations": read_only,
        }),
        json!({
            "name": "trace_check",
            "description": "Трассируемость как fitness-функция: покрытие звеньев REQ → NFR → \
                            AD/ADR → CMP → правило CONSTRAINTS.yaml, поимённые сироты, сверка \
                            модели с ARCHITECTURE-SPINE.md. AD без правила и без unverifiable — \
                            error. Verdict: passed=false — отказать изменению; report_markdown \
                            пригоден для evidence bundle",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Корень кейса (каталог с model/)"}
                },
                "required": ["path"],
            },
            "annotations": read_only,
        }),
        json!({
            "name": "model_query",
            "description": "Запрос к типизированной модели архитектуры (каталог model/): \
                            карточка сущности по id со связями и обратными ссылками, либо \
                            список сущностей (фильтр по типу: cap, sys, cmp, int, nfr, req, \
                            ad, adr, risk, owner)",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Каталог модели (по умолчанию model от cwd сервера)"},
                    "id": {"type": "string", "description": "ID сущности (ADR-001, CMP-002, …): карточка со связями"},
                    "type": {"type": "string", "description": "Фильтр списка по типу (cmp, adr, …)"},
                },
            },
            "annotations": read_only,
        }),
        json!({
            "name": "rubric_run",
            "description": "Оценка документа рубрикой архитектурного контроля через LLM-судью \
                            (evidence-bound, ADR-004; требует API-ключ провайдера из конфига \
                            arch — без ключа понятная JSON-RPC ошибка). Укажите ровно один из \
                            target / target_text",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "rubric": {"type": "string", "description": "Рубрика: имя в каталоге рубрик arch или путь к YAML"},
                    "target": {"type": "string", "description": "Путь к оцениваемому документу (md/txt)"},
                    "target_text": {"type": "string", "description": "Текст документа inline (альтернатива target)"},
                    "model": {"type": "string", "description": "Модель-судья (имя из [models]; по умолчанию — дефолтная)"},
                    "cwd": {"type": "string", "description": "Рабочий каталог клиента: относительный target резолвится от него (по умолчанию — cwd процесса сервера)"},
                },
                "required": ["rubric"],
            },
            "annotations": {
                "readOnlyHint": true,
                "destructiveHint": false,
                "idempotentHint": false,
                "openWorldHint": true,
            },
        }),
        // T4 (ADR-015): чтение знаний наружу. Только чтение — write/exec
        // инструменты агенту НЕ отдаются (см. тест write_tools_are_not_exposed).
        json!({
            "name": "kb_search",
            "description": "Поиск по локальной базе знаний архитектора (каталоги \
                            knowledge.dirs из конфига arch): ранжированные хиты — \
                            путь, строка, сниппет с контекстом. Вызывай, когда \
                            нужен доменный материал (ADRs, заметки, статьи)",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": {"type": "string", "description": "Поисковый запрос: термины через пробел"},
                    "limit": {"type": "integer", "description": "Максимум хитов (по умолчанию 10, не больше 20)"},
                },
                "required": ["query"],
            },
            "annotations": read_only,
        }),
        json!({
            "name": "skill_search",
            "description": "Поиск по библиотеке архитектурных скиллов (плагины \
                            arch: навыки, MCP, субагенты). Вызывай, когда нужна \
                            методика по теме (ADR, saga, NFR, рубрики…)",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": {"type": "string", "description": "Поисковый запрос"},
                    "limit": {"type": "integer", "description": "Максимум результатов (по умолчанию 8, не больше 20)"},
                },
                "required": ["query"],
            },
            "annotations": read_only,
        }),
        json!({
            "name": "skill_load",
            "description": "Загрузить полный текст архитектурного скилла по точному \
                            имени (после skill_search). Скилл — методика: приёмы, \
                            чек-листы, антипаттерны",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": {"type": "string", "description": "Точное имя скилла"},
                },
                "required": ["name"],
            },
            "annotations": read_only,
        }),
        json!({
            "name": "mermaid_render",
            "description": "Рендерит mermaid-диаграмму (flowchart, sequenceDiagram, \
                            erDiagram или C4Context/C4Container/C4Component) в \
                            ASCII-арт. Вход: 'code' (исходник) или 'path' (путь к \
                            .mmd-файлу относительно cwd сервера)",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "code": {"type": "string", "description": "Исходный код mermaid-диаграммы"},
                    "path": {"type": "string", "description": "Путь к .mmd-файлу (резолвится от cwd сервера)"},
                },
            },
            "annotations": read_only,
        }),
        // Split-judge (механический судья без LLM у сервера): хост исполняет
        // промпт своей моделью, сервер собирает отчёт тем же кодом, что
        // у встроенного судьи rubric_run (медиана, unstable, evidence_not_found).
        json!({
            "name": "rubric_prompt",
            "description": "Split-judge, фаза 1 (без API-ключа): собирает system+user промпты \
                            архитектурного судьи по рубрике и целевому документу + JSON-схему \
                            ответа + judge_config (число сэмплов k). Выполните промпт k раз \
                            СВОЕЙ моделью и передайте сырые ответы массивом 'answers' в \
                            rubric_verify с теми же rubric и target/target_text",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "rubric": {"type": "string", "description": "Рубрика: имя в каталоге рубрик arch или путь к YAML"},
                    "target": {"type": "string", "description": "Путь к оцениваемому документу (md/txt)"},
                    "target_text": {"type": "string", "description": "Текст документа inline (альтернатива target)"},
                    "dynamic_subject": {"type": "string", "description": "НЕ поддерживается в MCP-режиме (нужен LLM у сервера): сгенерируйте рубрику своей моделью и передайте путь в rubric"},
                },
                "required": ["rubric"],
            },
            "annotations": read_only,
        }),
        json!({
            "name": "rubric_verify",
            "description": "Split-judge, фаза 2 (без API-ключа): принимает сырые ответы вашей \
                            модели на промпт rubric_prompt (массив строк 'answers') и строит \
                            отчёт рубрики: медиана баллов по сэмплам, метки unstable (разброс) \
                            и evidence_not_found (цитата не подтверждена target'ом). Битые \
                            ответы отбрасываются со счётчиком в answers.dropped",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "rubric": {"type": "string", "description": "Та же рубрика, что в rubric_prompt"},
                    "target": {"type": "string", "description": "Тот же документ (путь), что судился — для проверки цитат"},
                    "target_text": {"type": "string", "description": "Тот же текст inline (альтернатива target)"},
                    "answers": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "Сырые ответы модели хоста (каждый — JSON судьи по response_json_schema)",
                    },
                    "model": {"type": "string", "description": "Опц.: метка судьи для отчёта (имя модели хоста)"},
                    "judge_model": {"type": "string", "description": "Опц.: метка судьи, перекрывает model — фиксируйте фактическую модель-судью (anti-bias «автор = судья»: судья ДОЛЖЕН отличаться от модели-автора документа)"},
                    "author_model": {"type": "string", "description": "Опц.: модель-АВТОР документа — если совпадает с судьёй, отчёт помечается «судья судил свою работу» (Н7, ADR-042); попадает в отчёт рубрики и в находку judge_is_author составляющей гейта decision_quality"},
                },
                "required": ["rubric", "answers"],
            },
            "annotations": read_only,
        }),
        // Anti-bypass floor (S-1, ADR-034): маршрут из механики диффа, а не из
        // самооценки агента. В конце vec — порядок первых 12 ручных
        // инструментов зафиксирован тестами.
        json!({
            "name": "significance_from_diff",
            "description": "Маршрут значимости Fast/Standard/Critical, выведенный из git-диффа \
                            репозитория (anti-bypass S-1, ADR-034): детекторы new_component / \
                            new_vendor / api_contract_change / irreversible_migration / \
                            new_datastore объединяются с заявленными 'declared' (детектор \
                            только добавляет). Ответ: route+score, sources каждого триггера \
                            (declared/diff/declared+diff), undeclared — найденные диффом, но \
                            не заявленные триггеры с файлами-основаниями. Информационный \
                            инструмент (без passed)",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Корень git-репозитория (по умолчанию — рабочий каталог процесса сервера)",
                    },
                    "base_ref": {
                        "type": "string",
                        "description": "Опц.: база диффа — голая ревизия (git diff BASE_REF...HEAD) или готовый диапазон A...HEAD как есть; без неё — рабочее дерево против HEAD (staged + unstaged + untracked)",
                    },
                    "declared": {
                        "type": "object",
                        "description": "Опц.: заявленные триггеры («триггер → true/false», ключи — из 15 канонических, как у significance_score). Незнакомое имя — ошибка вызова (-32602): опечатка молча оставила бы настоящий триггер незаявленным",
                        "additionalProperties": {"type": "boolean"},
                    },
                },
            },
            "annotations": read_only,
        }),
        // Кандидатные fitness-правила из пробелов кейса (src/rules_suggest.rs).
        // В конце vec — порядок первых 12 ручных инструментов зафиксирован
        // тестами.
        json!({
            "name": "rules_suggest",
            "description": "Кандидатные fitness-правила из содержательных пробелов кейса \
                            (read-only эвристики): EARS-критерии приёмки, численные таймауты \
                            в контрактах, декомпозиция REQ→работы, RTO/RPO без ADR, аудит \
                            операторских действий. Ответ: candidates (id, rationale, \
                            source_skill, yaml — готовый фрагмент CONSTRAINTS.yaml или null \
                            для честного advisory) + report_markdown. Информационный \
                            инструмент, без passed",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Корень кейса (каталог с docs/, model/, .arch-handoff/)"},
                    "cwd": {"type": "string", "description": "Рабочий каталог клиента: относительный path резолвится от него (по умолчанию — cwd процесса сервера)"},
                },
                "required": ["path"],
            },
            "annotations": read_only,
        }),
        json!({
            "name": "trust_report",
            "description": "Метрика доверия к контуру: положение на шкале 1–5 с ЯКОРЯМИ и \
                            ДОКАЗАТЕЛЬСТВАМИ — контур подключён (журнал вызовов), гейт \
                            останавливал работу (fail → починка), правила сопровождаются \
                            (владелец, срок, проверка поведения), пакет защищён измеренно \
                            (доля обнаружения redteam), вердикт полон и подписан. У каждого \
                            якоря: чем подтверждён и почему не достигнут. Ничего не \
                            блокирует: отвечает, насколько можно верить зелёному контура",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Репозиторий или кейс (по умолчанию — каталог вызова)"},
                    "cwd": {"type": "string", "description": "Рабочий каталог клиента: относительный path резолвится от него (по умолчанию — cwd процесса сервера)"},
                },
            },
            "annotations": read_only,
        }),
        json!({
            "name": "verdict_explain",
            "description": "Паспорт вердикта: прогон гейта + страница «что зелёный НЕ означает» \
                            в трёх блоках — проверено (составляющие с числами), заявлено, но \
                            механикой не проверяется (подпись A3, семантика ссылок, независимость \
                            судьи и ревьюера, адекватность решения), не проверено (SKIP с \
                            причиной). Отвечает тем же вердиктом, что `arch-be gate`, и его \
                            границами; решения не принимает",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Репозиторий (по умолчанию — каталог вызова)"},
                    "route": {"type": "string", "description": "auto (по умолчанию) | fast | standard | critical"},
                    "base": {"type": "string", "description": "База git для диффа (по умолчанию — рабочее дерево против HEAD)"},
                    "constraints": {"type": "string", "description": "Файл ограничений (по умолчанию <repo>/.arch-handoff/CONSTRAINTS.yaml)"},
                    "cwd": {"type": "string", "description": "Рабочий каталог клиента: относительный path резолвится от него (по умолчанию — cwd процесса сервера)"},
                },
            },
            "annotations": read_only,
        }),
    ]
}

/// Полный список спецификаций `tools/list`: ручные инструменты (см.
/// [`tool_specs`]) + мостовые по белым спискам текущего режима.
impl McpServe {
    /// `tools/list` текущего режима: ручные + мостовые (сортированы по имени).
    fn all_tool_specs(&self) -> Vec<Value> {
        let mut specs = tool_specs();
        specs.extend(self.bridge_tool_specs());
        specs
    }

    /// MCP-спеки мостовых инструментов: генерируются из `Tool::spec()`
    /// реестра (name/description/parameters) + annotations по членству в
    /// списках и классу риска [`crate::policy::classify_tool`]. В схему
    /// каждого добавляется опциональный аргумент `cwd` моста. Инструменты,
    /// отключённые конфигом (напр. `[archify].enabled = false`), пропускаются.
    fn bridge_tool_specs(&self) -> Vec<Value> {
        let mut names: Vec<&str> = Vec::new();
        names.extend_from_slice(BRIDGE_READ_ONLY);
        if self.mode.allows_write() {
            names.extend_from_slice(BRIDGE_READ_WRITE);
        }
        names.sort_unstable();
        let mut out = Vec::with_capacity(names.len());
        for name in names {
            let Some(tool) = self.registry.get(name) else {
                continue;
            };
            let spec = tool.spec();
            let mut parameters = spec.parameters;
            if let Some(props) = parameters
                .get_mut("properties")
                .and_then(Value::as_object_mut)
            {
                props.insert(
                    "cwd".into(),
                    json!({
                        "type": "string",
                        "description": "Рабочий каталог клиента: относительные пути вызова \
                                        резолвятся от него (по умолчанию — cwd процесса сервера)",
                    }),
                );
            }
            out.push(json!({
                "name": spec.name,
                "description": spec.description,
                "inputSchema": parameters,
                "annotations": bridge_annotations(&spec.name),
            }));
        }
        out
    }
}

/// Аннотации MCP для мостового инструмента: `readOnlyHint` — по членству в
/// rw-списке (честно о записи: rw-инструменты пишут в рабочий каталог
/// клиента, даже если политика считает их `ReadOnly`), `destructiveHint` — по
/// классу риска из [`crate::policy::classify_tool`] (`Mutating`+ → true;
/// аддитивные записи вроде `skill_distill`/`archify_*` — false).
fn bridge_annotations(name: &str) -> Value {
    let class = crate::policy::classify_tool(name, &Value::Null);
    let mutating = !BRIDGE_READ_ONLY.contains(&name);
    json!({
        "readOnlyHint": !mutating,
        "destructiveHint": class != crate::policy::RiskClass::ReadOnly,
        "idempotentHint": !mutating,
        "openWorldHint": false,
    })
}

/// Цикл сервера поверх произвольных AsyncRead/AsyncWrite: строка → ответ
/// (или молчание на уведомление), flush на каждый ответ, выход по EOF
/// либо по ошибке чтения (транспорт мёртв — сервер завершается чисто).
async fn run_loop<R, W>(server: &McpServe, reader: R, mut writer: W) -> Result<()>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut lines = BufReader::new(reader).lines();
    loop {
        match lines.next_line().await {
            Ok(Some(line)) => {
                if line.trim().is_empty() {
                    continue; // пустые строки — не сообщения, пропускаем молча
                }
                let response = if line.len() > MAX_LINE_BYTES {
                    Some(error_response(
                        &Value::Null,
                        INVALID_REQUEST,
                        format!("строка длиннее лимита {MAX_LINE_BYTES} байт"),
                    ))
                } else {
                    server.handle_line(&line).await
                };
                if let Some(response) = response {
                    let mut payload = response.to_string();
                    payload.push('\n');
                    writer.write_all(payload.as_bytes()).await?;
                    writer.flush().await?;
                }
            }
            Ok(None) => break,
            Err(e) => {
                tracing::warn!(error = %e, "mcp-serve: ошибка чтения stdin, завершение");
                break;
            }
        }
    }
    writer.flush().await?;
    Ok(())
}

/// Точка входа `arch-be mcp serve`: цикл на stdin/stdout процесса,
/// read-only режим (поведение по умолчанию).
///
/// # Errors
/// Запись в stdout оборвалась (клиент умер) — сервер завершается с ошибкой
/// транспорта; входной мусор ошибкой не является (ответ `-32700` и дальше).
pub async fn serve(cfg: Arc<Config>) -> Result<()> {
    serve_with_mode(cfg, ServeMode::ReadOnly).await
}

/// Точка входа `arch-be mcp serve [--rw]`: цикл на stdin/stdout процесса
/// в явном режиме [`ServeMode`].
///
/// # Errors
/// Запись в stdout оборвалась (клиент умер) — сервер завершается с ошибкой
/// транспорта; входной мусор ошибкой не является (ответ `-32700` и дальше).
pub async fn serve_with_mode(cfg: Arc<Config>, mode: ServeMode) -> Result<()> {
    let server = McpServe::with_mode(cfg, mode);
    run_loop(&server, tokio::io::stdin(), tokio::io::stdout()).await
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Сервер на дефолтном конфиге (без ключей и реального дома).
    fn server() -> McpServe {
        McpServe::new(Arc::new(Config::default()))
    }

    /// Прогоняет пачку входных строк через in-memory цикл и возвращает
    /// разобранные ответы (по одному на строку вывода). Сервер — дефолтный.
    async fn run_lines(input: &[&str]) -> Vec<Value> {
        run_lines_on(server(), input).await
    }

    /// Вариант [`run_lines`] на сервере с заданным конфигом (например,
    /// с временными knowledge/plugins-каталогами).
    async fn run_lines_on(server: McpServe, input: &[&str]) -> Vec<Value> {
        // spawn требует 'static: пачка клонируется в owned-строки заранее.
        let owned: Vec<String> = input.iter().map(|s| (*s).to_string()).collect();
        let (read_end, mut write_end) = tokio::io::duplex(64 * 1024);
        let (out_read, out_write) = tokio::io::duplex(64 * 1024);
        let writer_task = tokio::spawn(async move {
            for line in owned {
                write_end.write_all(line.as_bytes()).await.expect("запись");
                write_end.write_all(b"\n").await.expect("запись nl");
            }
            // Закрытие write_end → EOF на read_end → цикл завершается.
        });
        run_loop(&server, read_end, out_write).await.expect("цикл");
        writer_task.await.expect("писатель");
        let mut responses = Vec::new();
        let mut lines = BufReader::new(out_read).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            responses.push(serde_json::from_str(&line).expect("валидный JSON ответа"));
        }
        responses
    }

    #[tokio::test]
    async fn initialize_echoes_known_protocol_and_advertises_tools() {
        for (asked, want) in [
            ("2025-06-18", "2025-06-18"),
            ("2024-11-05", "2024-11-05"),
            ("1999-01-01", "2025-06-18"),
        ] {
            let responses = run_lines(&[&format!(
                r#"{{"jsonrpc":"2.0","id":1,"method":"initialize","params":{{"protocolVersion":"{asked}","capabilities":{{}},"clientInfo":{{"name":"t","version":"0"}}}}}}"#
            )])
            .await;
            assert_eq!(responses.len(), 1);
            let result = &responses[0]["result"];
            assert_eq!(result["protocolVersion"], want, "версия для {asked}");
            assert_eq!(result["serverInfo"]["name"], "arch-harness");
            assert!(result["capabilities"]["tools"].is_object());
            assert!(
                result["capabilities"]["prompts"].is_object(),
                "capability prompts (плейбуки spine-* как слэш-команды)"
            );
        }
    }

    #[tokio::test]
    async fn notifications_and_empty_lines_get_no_response() {
        let responses = run_lines(&[
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
            r#"{"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":1}}"#,
            r#"{"jsonrpc":"2.0","method":"unknown_notification"}"#,
            "",
            "   ",
            r#"{"jsonrpc":"2.0","id":7,"method":"ping"}"#,
        ])
        .await;
        assert_eq!(responses.len(), 1, "ответ только на ping: {responses:?}");
        assert_eq!(responses[0]["id"], 7);
        assert_eq!(responses[0]["result"], json!({}));
    }

    #[tokio::test]
    async fn broken_json_gives_32700_and_loop_continues() {
        let responses = run_lines(&[
            "{это не json",
            r#"{"jsonrpc":"2.0","id":2,"method":"ping"}"#,
        ])
        .await;
        assert_eq!(responses.len(), 2, "сервер пережил битую строку");
        assert_eq!(responses[0]["error"]["code"], PARSE_ERROR);
        assert_eq!(responses[0]["id"], Value::Null);
        assert_eq!(responses[1]["result"], json!({}));
    }

    #[tokio::test]
    async fn unknown_method_gives_32601_with_any_id_type() {
        let responses = run_lines(&[
            r#"{"jsonrpc":"2.0","id":"abc-1","method":"resources/list"}"#,
            "[1,2,3]",
            r#"{"jsonrpc":"2.0","id":3}"#,
        ])
        .await;
        assert_eq!(responses[0]["error"]["code"], METHOD_NOT_FOUND);
        assert_eq!(responses[0]["id"], "abc-1", "строковый id эхом");
        assert_eq!(responses[1]["error"]["code"], INVALID_REQUEST);
        assert_eq!(responses[2]["error"]["code"], INVALID_REQUEST);
        assert_eq!(responses[2]["id"], 3);
    }

    #[tokio::test]
    async fn tools_list_read_only_mode_manual_first_then_bridge() {
        let responses =
            run_lines(&[r#"{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}"#]).await;
        let tools = responses[0]["result"]["tools"].as_array().expect("tools");
        let names: Vec<&str> = tools
            .iter()
            .map(|t| t["name"].as_str().expect("имя"))
            .collect();
        // Первые 10 — ручные инструменты в зафиксированном порядке (ADR-008);
        // далее — split-judge (ручные) и мостовые read-only.
        assert_eq!(
            names[..10],
            [
                "spine_lint",
                "fitness_check",
                "significance_score",
                "trace_check",
                "model_query",
                "rubric_run",
                "kb_search",
                "skill_search",
                "skill_load",
                "mermaid_render"
            ]
        );
        assert_eq!(names[10..12], ["rubric_prompt", "rubric_verify"]);
        for bridged in BRIDGE_READ_ONLY {
            assert!(
                names.contains(bridged),
                "мостовой read-only '{bridged}' обязан быть в tools/list: {names:?}"
            );
        }
        assert_eq!(
            names.len(),
            MANUAL_TOOLS.len() + BRIDGE_READ_ONLY.len(),
            "ro-режим: ручные + split-judge + read-only мост"
        );
        for t in tools {
            assert_eq!(t["annotations"]["readOnlyHint"], true, "{}", t["name"]);
            assert!(t["inputSchema"].is_object(), "{}", t["name"]);
        }
        // У мостовых спек есть дополнительный аргумент моста `cwd`.
        let openapi = tools
            .iter()
            .find(|t| t["name"] == "openapi_lint")
            .expect("openapi_lint");
        assert!(
            openapi["inputSchema"]["properties"]["cwd"].is_object(),
            "аргумент cwd в мостовой спеке: {openapi}"
        );
        assert!(
            openapi["inputSchema"]["properties"]["path"].is_object(),
            "схема инструмента из Tool::spec(): {openapi}"
        );
    }

    #[tokio::test]
    async fn write_and_exec_tools_are_not_exposed() {
        // Read-only дисциплина (T4, ADR-015): сервер отдаёт только чтение —
        // ни одного write/exec-инструмента агентного реестра наружу.
        let responses =
            run_lines(&[r#"{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}"#]).await;
        let names: Vec<&str> = responses[0]["result"]["tools"]
            .as_array()
            .expect("tools")
            .iter()
            .map(|t| t["name"].as_str().expect("имя"))
            .collect();
        for forbidden in BRIDGE_NEVER {
            assert!(
                !names.contains(forbidden),
                "write/exec-инструмент '{forbidden}' не должен отдаваться наружу: {names:?}"
            );
        }
        // rw-контур в read-only режиме закрыт: ни в списке, ни вызовом.
        for rw in BRIDGE_READ_WRITE {
            assert!(
                !names.contains(rw),
                "rw-инструмент '{rw}' не должен отдаваться без --rw: {names:?}"
            );
        }
    }

    #[tokio::test]
    async fn unknown_tool_and_bad_arguments_give_32602() {
        let responses = run_lines(&[
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"ghost","arguments":{}}}"#,
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"spine_lint","arguments":{}}}"#,
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"fitness_check","arguments":{"repo":42}}}"#,
            r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"ping"}}"#,
            r#"{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"arguments":{}}}"#,
        ])
        .await;
        for (i, r) in responses.iter().enumerate() {
            assert_eq!(r["error"]["code"], INVALID_PARAMS, "ответ {}: {r}", i + 1);
        }
        assert!(
            responses[0]["error"]["message"]
                .as_str()
                .expect("сообщение")
                .contains("ghost")
        );
    }

    /// T-04: незнакомое имя триггера — ошибка вызова, а не завышенный
    /// маршрут. Раньше выдуманные имена попадали в `unknown_triggers` И в
    /// счёт: пять несуществующих триггеров давали Critical.
    #[tokio::test]
    async fn significance_score_routes_and_rejects_unknown() {
        let responses = run_lines(&[
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"significance_score","arguments":{"triggers":{"new_component":true,"security_boundary_change":true}}}}"#,
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"significance_score","arguments":{"triggers":{}}}}"#,
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"significance_score","arguments":{"triggers":{"alien_trigger":true}}}}"#,
            r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"significance_score","arguments":{"triggers":{"new_components":true,"new_datastore":true}}}}"#,
        ])
        .await;
        let sc = &responses[0]["result"]["structuredContent"];
        assert_eq!(sc["route"], "Critical");
        assert_eq!(sc["score"], 2);
        assert_eq!(responses[1]["result"]["structuredContent"]["route"], "Fast");
        let alien = &responses[2]["error"];
        assert_eq!(alien["code"], INVALID_PARAMS, "{alien}");
        assert!(
            alien["message"]
                .as_str()
                .expect("сообщение")
                .contains("Канонические (15)")
        );
        // Опечатка в настоящем имени: названо ближайшее каноническое, маршрута
        // нет — иначе `new_components` тихо занизил бы значимость.
        let typo = &responses[3]["error"];
        assert_eq!(typo["code"], INVALID_PARAMS, "{typo}");
        let text = typo["message"].as_str().expect("сообщение");
        assert!(text.contains("'new_components'"), "{text}");
        assert!(text.contains("'new_component'"), "{text}");
        // text-дубль verdict'а — валидный JSON (его разбирает клиент mcp.rs).
        let text = responses[0]["result"]["content"][0]["text"]
            .as_str()
            .expect("text");
        let parsed: Value = serde_json::from_str(text).expect("text — JSON");
        assert_eq!(parsed["route"], "Critical");
    }

    #[tokio::test]
    async fn significance_score_accepts_trigger_list_form() {
        // Массивная форма (`control score --trigger` стиль): "name=true",
        // "name=false", голое "name" (= true). Незнакомое имя отвергается (T-04).
        let responses = run_lines(&[
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"significance_score","arguments":{"triggers":["new_component=true","security_boundary_change"]}}}"#,
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"significance_score","arguments":{"triggers":["new_component=false"]}}}"#,
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"significance_score","arguments":{"triggers":["alien_trigger=true"]}}}"#,
            r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"significance_score","arguments":{"triggers":["new_component=да"]}}}"#,
        ])
        .await;
        let sc = &responses[0]["result"]["structuredContent"];
        assert_eq!(sc["route"], "Critical", "{sc}");
        assert_eq!(sc["score"], 2, "{sc}");
        // "new_component=false" — не сработал: пустое множество → Fast.
        let off = &responses[1]["result"]["structuredContent"];
        assert_eq!(off["route"], "Fast", "{off}");
        assert_eq!(off["fired"], json!([]), "{off}");
        // T-04: незнакомое имя в массивной форме — та же ошибка вызова.
        let alien = &responses[2]["error"];
        assert_eq!(alien["code"], INVALID_PARAMS, "{alien}");
        // Не-bool значение после '=' — понятная ошибка разбора (-32602).
        assert_eq!(responses[3]["error"]["code"], INVALID_PARAMS);
        assert!(
            responses[3]["error"]["message"]
                .as_str()
                .expect("сообщение")
                .contains("не bool"),
            "{}",
            responses[3]
        );
    }

    #[tokio::test]
    async fn rules_suggest_in_tools_list_and_finds_gap_candidates() {
        // Кейс с пробелом: контракт без численных таймаутов + спека без EARS.
        let dir = tempfile::tempdir().expect("tmp");
        std::fs::create_dir_all(dir.path().join("docs/contracts")).expect("mkdir");
        std::fs::write(
            dir.path().join("docs/contracts/api.md"),
            "# Контракт\n\nСинхронный вызов.\n",
        )
        .expect("contract");
        let case = dir.path().display().to_string();
        let owned = [
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}"#.to_string(),
            format!(
                r#"{{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{{"name":"rules_suggest","arguments":{{"path":"{case}"}}}}}}"#
            ),
        ];
        let refs: Vec<&str> = owned.iter().map(String::as_str).collect();
        let responses = run_lines(&refs).await;
        // Инструмент объявлен в tools/list (read-only режим).
        let tools = responses[0]["result"]["tools"].as_array().expect("tools");
        let spec = tools
            .iter()
            .find(|t| t["name"] == "rules_suggest")
            .expect("rules_suggest в tools/list");
        assert_eq!(spec["annotations"]["readOnlyHint"], true, "{spec}");
        assert!(
            spec["inputSchema"]["properties"]["cwd"].is_object(),
            "аргумент cwd в спеке: {spec}"
        );
        // Вызов находит кандидатов; у механизируемых — готовый YAML.
        let sc = &responses[1]["result"]["structuredContent"];
        let candidates = sc["candidates"].as_array().expect("candidates");
        let ids: Vec<&str> = candidates.iter().filter_map(|c| c["id"].as_str()).collect();
        assert!(
            ids.contains(&"contract-timeouts-numeric"),
            "{ids:?} (case: {case})"
        );
        let timeouts = candidates
            .iter()
            .find(|c| c["id"] == "contract-timeouts-numeric")
            .expect("кандидат");
        assert!(
            timeouts["yaml"]
                .as_str()
                .expect("yaml")
                .contains("must_contain"),
            "{timeouts}"
        );
        assert_eq!(timeouts["source_skill"], "adversarial-review");
        assert!(
            sc["report_markdown"]
                .as_str()
                .expect("markdown")
                .contains("Кандидатные fitness-правила"),
            "{sc}"
        );
        // Чистый кейс (пустой каталог) — честный ноль кандидатов.
        let empty = tempfile::tempdir().expect("tmp");
        let call = format!(
            r#"{{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{{"name":"rules_suggest","arguments":{{"path":"{}"}}}}}}"#,
            empty.path().display()
        );
        let responses = run_lines(&[&call]).await;
        let sc = &responses[0]["result"]["structuredContent"];
        assert_eq!(sc["candidate_count"], 0, "{sc}");
    }

    #[tokio::test]
    async fn rubric_run_resolves_target_against_cwd() {
        // Относительный target резолвится от аргумента `cwd` (рабочий каталог
        // клиента), а не от cwd процесса сервера.
        let dir = tempfile::tempdir().expect("tmp");
        std::fs::create_dir_all(dir.path().join("docs")).expect("mkdir");
        std::fs::write(dir.path().join("docs/adr.md"), "# ADR\n\nРешение.\n").expect("doc");
        let cwd = dir.path().display().to_string();
        let call = |id: u64, args: &str| {
            format!(
                r#"{{"jsonrpc":"2.0","id":{id},"method":"tools/call","params":{{"name":"rubric_run","arguments":{args}}}}}"#
            )
        };
        let owned = [
            // target есть в cwd клиента: чтение проходит, падение — позже, на
            // резолве несуществующей модели (-32602) — доказательство, что
            // файл прочитан (иначе — isError io раньше).
            call(
                1,
                &format!(
                    r#"{{"rubric":"x","target":"docs/adr.md","cwd":"{cwd}","model":"ghost-model"}}"#
                ),
            ),
            // Без cwd относительный путь ищется от cwd сервера — io-сбой.
            call(
                2,
                r#"{"rubric":"x","target":"docs/adr.md","model":"ghost-model"}"#,
            ),
            // Абсолютный target cwd игнорирует.
            call(
                3,
                &format!(
                    r#"{{"rubric":"x","target":"{}/docs/adr.md","cwd":"/tmp","model":"ghost-model"}}"#,
                    dir.path().display()
                ),
            ),
        ];
        let refs: Vec<&str> = owned.iter().map(String::as_str).collect();
        let responses = run_lines(&refs).await;
        assert_eq!(
            responses[0]["error"]["code"], INVALID_PARAMS,
            "{}",
            responses[0]
        );
        assert!(
            responses[0]["error"]["message"]
                .as_str()
                .expect("сообщение")
                .contains("ghost-model"),
            "{}",
            responses[0]
        );
        assert_eq!(
            responses[1]["result"]["isError"], true,
            "без cwd файл не находится: {}",
            responses[1]
        );
        assert!(
            responses[1]["result"]["content"][0]["text"]
                .as_str()
                .expect("текст")
                .contains("io:"),
            "{}",
            responses[1]
        );
        assert_eq!(
            responses[2]["error"]["code"], INVALID_PARAMS,
            "{}",
            responses[2]
        );
    }

    #[tokio::test]
    async fn spine_lint_verdict_marks_violations_and_clean() {
        let dir = tempfile::tempdir().expect("tmp");
        let bad = dir.path().join("BAD-SPINE.md");
        std::fs::write(
            &bad,
            "### AD-1. Брокер\n- Binds: контур\n- Prevents: хаос\n- Rule: только брокер\n\n\
             ### AD-1. Дубль\n- Binds: x\n- Prevents: y\n- Rule: z\n",
        )
        .expect("spine");
        let good = dir.path().join("GOOD-SPINE.md");
        std::fs::write(
            &good,
            "### AD-1. Брокер\n- Binds: контур\n- Prevents: хаос\n- Rule: только брокер\n",
        )
        .expect("spine");
        let call = |id: u64, path: &Path| {
            format!(
                r#"{{"jsonrpc":"2.0","id":{id},"method":"tools/call","params":{{"name":"spine_lint","arguments":{{"path":"{}"}}}}}}"#,
                path.display()
            )
        };
        let owned = [call(1, &bad), call(2, &good)];
        let refs: Vec<&str> = owned.iter().map(String::as_str).collect();
        let responses = run_lines(&refs).await;
        let bad_v = &responses[0]["result"]["structuredContent"];
        assert_eq!(bad_v["passed"], false, "{bad_v}");
        assert!(bad_v["issue_count"].as_u64().expect("число") >= 1);
        assert!(
            bad_v["issues"]
                .as_array()
                .expect("issues")
                .iter()
                .any(|i| i["rule"] == "dup_ad_id")
        );
        let good_v = &responses[1]["result"]["structuredContent"];
        assert_eq!(good_v["passed"], true, "{good_v}");
        assert_eq!(good_v["issue_count"], 0);
    }

    #[tokio::test]
    async fn domain_failure_is_is_error_result_not_crash() {
        let responses = run_lines(&[
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"spine_lint","arguments":{"path":"/нет/такого/spine.md"}}}"#,
            r#"{"jsonrpc":"2.0","id":2,"method":"ping"}"#,
        ])
        .await;
        assert_eq!(responses[0]["result"]["isError"], true);
        let text = responses[0]["result"]["content"][0]["text"]
            .as_str()
            .expect("text");
        assert!(text.contains("spine_lint"), "{text}");
        assert_eq!(responses[1]["result"], json!({}), "сервер жив после сбоя");
    }

    #[tokio::test]
    async fn rubric_run_validates_target_pair_and_key() {
        // Конфиг с моделью, чей ключ гарантированно отсутствует в окружении.
        let mut cfg = Config::default();
        cfg.models.insert(
            "nokey".into(),
            ModelConfig {
                base_url: "http://127.0.0.1:9".into(),
                model: "stub".into(),
                api_key_env: "ARCH_HARNESS_TEST_MISSING_KEY_XYZ".into(),
                api_key_file: None,
                ..ModelConfig::default()
            },
        );
        cfg.default_model = "nokey".into();
        let server = McpServe::new(Arc::new(cfg));
        // Оба target сразу → -32602.
        let both = server
            .handle_line(
                r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"rubric_run","arguments":{"rubric":"x","target":"a.md","target_text":"текст"}}}"#,
            )
            .await
            .expect("ответ");
        assert_eq!(both["error"]["code"], INVALID_PARAMS);
        // Ни одного target → -32602.
        let none_ = server
            .handle_line(
                r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"rubric_run","arguments":{"rubric":"x"}}}"#,
            )
            .await
            .expect("ответ");
        assert_eq!(none_["error"]["code"], INVALID_PARAMS);
        // Ключ недоступен → понятная -32603 (ДО обращения к рубрике/LLM).
        let no_key = server
            .handle_line(
                r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"rubric_run","arguments":{"rubric":"x","target_text":"текст"}}}"#,
            )
            .await
            .expect("ответ");
        assert_eq!(no_key["error"]["code"], INTERNAL_ERROR, "{no_key}");
        let msg = no_key["error"]["message"].as_str().expect("сообщение");
        assert!(msg.contains("ARCH_HARNESS_TEST_MISSING_KEY_XYZ"), "{msg}");
        assert!(!msg.contains("sk-"), "секретов в сообщении нет: {msg}");
        // Неизвестная модель → -32602.
        let bad_model = server
            .handle_line(
                r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"rubric_run","arguments":{"rubric":"x","target_text":"т","model":"ghost"}}}"#,
            )
            .await
            .expect("ответ");
        assert_eq!(bad_model["error"]["code"], INVALID_PARAMS);
    }

    /// Сервер на конфиге с временными каталогами знаний и плагинов
    /// (реального дома тест не касается).
    fn server_with_dirs(kb_dir: &Path, plugins_dir: &Path) -> McpServe {
        let mut cfg = Config::default();
        cfg.knowledge.dirs = vec![kb_dir.to_path_buf()];
        cfg.knowledge.extensions = vec!["md".into(), "txt".into()];
        cfg.plugins.dirs = vec![plugins_dir.to_path_buf()];
        McpServe::new(Arc::new(cfg))
    }

    /// Файлы тестовой базы знаний: документ про Kafka и нейтральный readme.
    fn kb_fixture(dir: &Path) {
        std::fs::write(
            dir.join("adr-042-kafka.md"),
            "# ADR-042\n\nБрокер сообщений.\nKafka выбран как шина событий.\nИтог: kafka в проде.\n",
        )
        .expect("adr");
        std::fs::write(dir.join("readme.md"), "# Общее\n\nНичего про брокеров.\n").expect("readme");
    }

    /// Плагин `mine` со скиллом `arch-core` (каталог плагина — носитель
    /// `skills/`, манифест синтезируется из имени каталога).
    fn plugin_fixture(dir: &Path) {
        let skill_md = dir.join("mine/skills/arch-core/SKILL.md");
        std::fs::create_dir_all(skill_md.parent().expect("parent")).expect("dirs");
        std::fs::write(
            &skill_md,
            "---\nname: arch-core\ndescription: Архитектурное ядро банка: каркас решений по ADR.\n---\n\
             # Arch Core\n\nМетодика принятия решений: фиксируйте ADR в каталоге model/adr.\n",
        )
        .expect("SKILL.md");
    }

    #[tokio::test]
    async fn kb_search_returns_hits_from_test_knowledge_catalog() {
        let tmp = tempfile::tempdir().expect("tmp");
        kb_fixture(tmp.path());
        let kb_dir = tmp.path().join("kb");
        std::fs::create_dir(&kb_dir).expect("kb dir");
        std::fs::write(
            kb_dir.join("notes-kafka.md"),
            "# Заметки\n\nKafka как шина событий в проде.\n",
        )
        .expect("notes");
        // Каталог знаний — только kb_dir (в tmp лежат и файлы фикстуры).
        let mut cfg = Config::default();
        cfg.knowledge.dirs = vec![kb_dir.clone()];
        cfg.knowledge.extensions = vec!["md".into()];
        let server = McpServe::new(Arc::new(cfg));
        let call = |id: u64, query: &str| {
            format!(
                r#"{{"jsonrpc":"2.0","id":{id},"method":"tools/call","params":{{"name":"kb_search","arguments":{{"query":"{query}"}}}}}}"#
            )
        };
        let responses = run_lines_on(server, &[&call(1, "kafka")]).await;
        let result = &responses[0]["result"];
        assert_eq!(result["isError"], false, "{result}");
        let sc = &result["structuredContent"];
        assert!(sc["count"].as_u64().expect("count") >= 1, "{sc}");
        let hits = sc["hits"].as_array().expect("hits");
        let top = &hits[0];
        assert!(
            top["path"]
                .as_str()
                .expect("path")
                .ends_with("notes-kafka.md"),
            "верхний хит — файл с kafka в имени: {top}"
        );
        assert!(top["score"].as_f64().expect("score") > 0.0);
        assert!(
            top["snippet"].as_str().expect("snippet").contains("Kafka"),
            "сниппет с матчем: {top}"
        );
        // Текст-дубль — валидный JSON (его разбирает клиент mcp.rs).
        let text = result["content"][0]["text"].as_str().expect("text");
        let parsed: Value = serde_json::from_str(text).expect("text — JSON");
        assert_eq!(parsed["count"], sc["count"]);
    }

    #[tokio::test]
    async fn kb_search_empty_result_is_not_an_error() {
        let tmp = tempfile::tempdir().expect("tmp");
        kb_fixture(tmp.path());
        let server = server_with_dirs(tmp.path(), tmp.path());
        let call = r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"kb_search","arguments":{"query":"никогданесуществующийтермин"}}}"#;
        let responses = run_lines_on(server, &[call]).await;
        let result = &responses[0]["result"];
        assert_eq!(result["isError"], false, "{result}");
        let sc = &result["structuredContent"];
        assert_eq!(sc["count"], 0);
        assert!(
            sc["summary"]
                .as_str()
                .expect("summary")
                .contains("ничего не найдено")
        );
    }

    #[tokio::test]
    async fn skill_search_finds_arch_core_and_load_returns_body() {
        let tmp = tempfile::tempdir().expect("tmp");
        plugin_fixture(tmp.path());
        let server = server_with_dirs(tmp.path(), tmp.path());
        let responses = run_lines_on(
            server,
            &[
                r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"skill_search","arguments":{"query":"arch"}}}"#,
                r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"skill_load","arguments":{"name":"arch-core"}}}"#,
                r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"skill_load","arguments":{"name":"нет-такого"}}}"#,
            ],
        )
        .await;
        // skill_search находит скилл arch-core.
        let search = &responses[0]["result"]["structuredContent"];
        let hits = search["hits"].as_array().expect("hits");
        assert!(
            hits.iter().any(|h| {
                h["name"] == "arch-core"
                    && h["plugin"] == "mine"
                    && h["score"].as_f64().expect("score") > 0.0
            }),
            "arch-core в хитах: {search}"
        );
        // skill_load отдаёт полный текст скилла.
        let loaded = &responses[1]["result"];
        assert_eq!(loaded["isError"], false, "{loaded}");
        let sc = &loaded["structuredContent"];
        assert_eq!(sc["name"], "arch-core");
        assert_eq!(sc["plugin"], "mine");
        assert!(
            sc["body"]
                .as_str()
                .expect("body")
                .contains("Методика принятия решений"),
            "тело скилла: {sc}"
        );
        // Неизвестное имя — доменная ошибка isError, а не protocol error.
        let missing = &responses[2]["result"];
        assert_eq!(missing["isError"], true, "{missing}");
        let text = missing["content"][0]["text"].as_str().expect("text");
        assert!(
            text.contains("не найден") && text.contains("нет-такого"),
            "{text}"
        );
    }

    #[tokio::test]
    async fn prompts_list_has_eight_playbooks_with_frontmatter_descriptions() {
        // Пустой plugins-каталог → встроенные ассеты (чистая машина).
        let tmp = tempfile::tempdir().expect("tmp");
        let server = server_with_dirs(tmp.path(), tmp.path());
        let responses = run_lines_on(
            server,
            &[r#"{"jsonrpc":"2.0","id":1,"method":"prompts/list","params":{}}"#],
        )
        .await;
        let prompts = responses[0]["result"]["prompts"]
            .as_array()
            .expect("prompts");
        let names: Vec<&str> = prompts
            .iter()
            .map(|p| p["name"].as_str().expect("name"))
            .collect();
        assert_eq!(
            names,
            [
                "spine-quickstart",
                "spine-content-bootstrap",
                "spine-architect-review",
                "spine-adr-judge",
                "spine-contracts-gate",
                "spine-archify-viz",
                "spine-fitness-gate",
                "spine-bundle",
            ],
            "восемь плейбуков в зафиксированном порядке"
        );
        for p in prompts {
            assert!(
                p["description"].as_str().is_some_and(|d| !d.is_empty()),
                "description из frontmatter: {p}"
            );
            assert!(p["arguments"].is_array(), "arguments — массив: {p}");
        }
        // Аргументы объявлены только у параметризованных плейбуков.
        let arg_count = |name: &str| {
            prompts.iter().find(|p| p["name"] == name).expect("промпт")["arguments"]
                .as_array()
                .expect("args")
                .len()
        };
        assert_eq!(arg_count("spine-quickstart"), 0);
        assert_eq!(arg_count("spine-adr-judge"), 2, "target + rubric");
        assert_eq!(arg_count("spine-contracts-gate"), 3, "path + old + new");
        assert_eq!(arg_count("spine-archify-viz"), 1, "subject");
    }

    #[tokio::test]
    async fn prompts_get_renders_every_embedded_playbook() {
        let tmp = tempfile::tempdir().expect("tmp");
        for pb in PLAYBOOK_PROMPTS {
            let server = server_with_dirs(tmp.path(), tmp.path());
            let line = format!(
                r#"{{"jsonrpc":"2.0","id":1,"method":"prompts/get","params":{{"name":"{}"}}}}"#,
                pb.name
            );
            let responses = run_lines_on(server, &[&line]).await;
            let result = &responses[0]["result"];
            assert!(
                result["description"]
                    .as_str()
                    .is_some_and(|d| !d.is_empty()),
                "{}: description из frontmatter",
                pb.name
            );
            let messages = result["messages"].as_array().expect("messages");
            assert_eq!(messages.len(), 1, "{}: одно user-сообщение", pb.name);
            assert_eq!(messages[0]["role"], "user", "{}", pb.name);
            assert_eq!(messages[0]["content"]["type"], "text", "{}", pb.name);
            let text = messages[0]["content"]["text"].as_str().expect("text");
            assert!(
                text.starts_with("Действуй по этому плейбуку"),
                "{}: инструкция-команда",
                pb.name
            );
            assert!(
                text.contains(pb.embedded),
                "{}: полный текст встроенного SKILL.md в сообщении",
                pb.name
            );
        }
    }

    #[tokio::test]
    async fn prompts_get_prefers_user_copy_and_echoes_declared_arguments() {
        let tmp = tempfile::tempdir().expect("tmp");
        // Пользовательская копия плейбука в plugins.dirs (логика skill_load).
        let skill_md = tmp
            .path()
            .join("spine-workflows/skills/spine-fitness-gate/SKILL.md");
        std::fs::create_dir_all(skill_md.parent().expect("parent")).expect("dirs");
        std::fs::write(
            &skill_md,
            "---\nname: spine-fitness-gate\ndescription: ПОЛЬЗОВАТЕЛЬСКИЙ плейбук гейта.\n---\n\n\
             # Мой гейт\n\nТело пользователя.\n",
        )
        .expect("SKILL.md");
        let server = server_with_dirs(tmp.path(), tmp.path());
        let responses = run_lines_on(
            server,
            &[
                r#"{"jsonrpc":"2.0","id":1,"method":"prompts/get","params":{"name":"spine-fitness-gate"}}"#,
                r#"{"jsonrpc":"2.0","id":2,"method":"prompts/get","params":{"name":"spine-adr-judge","arguments":{"target":"docs/adr/0001.md","rubric":"adr_quality","чужой":"игнор"}}}"#,
            ],
        )
        .await;
        // Пользовательская копия побеждает встроенную.
        let first = &responses[0]["result"];
        assert_eq!(first["description"], "ПОЛЬЗОВАТЕЛЬСКИЙ плейбук гейта.");
        let text = first["messages"][0]["content"]["text"]
            .as_str()
            .expect("text");
        assert!(text.contains("Тело пользователя."), "{text}");
        // Эхо — только объявленных аргументов, в порядке объявления.
        let text2 = responses[1]["result"]["messages"][0]["content"]["text"]
            .as_str()
            .expect("text");
        assert!(text2.contains("- target = \"docs/adr/0001.md\""), "{text2}");
        assert!(text2.contains("- rubric = \"adr_quality\""), "{text2}");
        assert!(!text2.contains("чужой"), "{text2}");
    }

    #[tokio::test]
    async fn prompts_get_invalid_params_and_resources_stay_guarded() {
        // Имена валидны только в id=3/4, но ошибки параметров ловятся до
        // резолва текста — тест не зависит от реального дома.
        let responses = run_lines(&[
            r#"{"jsonrpc":"2.0","id":1,"method":"prompts/get","params":{}}"#,
            r#"{"jsonrpc":"2.0","id":2,"method":"prompts/get","params":{"name":"ghost"}}"#,
            r#"{"jsonrpc":"2.0","id":3,"method":"prompts/get","params":{"name":"spine-quickstart","arguments":["x"]}}"#,
            r#"{"jsonrpc":"2.0","id":4,"method":"prompts/get","params":{"name":"spine-adr-judge","arguments":{"target":42}}}"#,
            r#"{"jsonrpc":"2.0","id":5,"method":"resources/list","params":{}}"#,
            r#"{"jsonrpc":"2.0","id":6,"method":"resources/templates/list","params":{}}"#,
        ])
        .await;
        for (i, resp) in responses.iter().enumerate() {
            if i < 4 {
                assert_eq!(resp["error"]["code"], INVALID_PARAMS, "ответ {i}: {resp}");
            } else {
                assert_eq!(resp["error"]["code"], METHOD_NOT_FOUND, "ответ {i}: {resp}");
            }
        }
        let msg = responses[1]["error"]["message"].as_str().expect("message");
        assert!(msg.contains("ghost"), "{msg}");
    }

    #[tokio::test]
    async fn mermaid_render_returns_ascii_art_for_flowchart() {
        let server = server();
        let responses = run_lines_on(
            server,
            &[
                r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"mermaid_render","arguments":{"code":"graph LR\nA --> B"}}}"#,
                r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"mermaid_render","arguments":{}}}"#,
            ],
        )
        .await;
        let ok = &responses[0]["result"];
        assert_eq!(ok["isError"], false, "{ok}");
        let sc = &ok["structuredContent"];
        assert_eq!(sc["kind"], "flowchart");
        let art = sc["art"].as_str().expect("art");
        assert!(
            art.contains("│ A │") && art.contains("│ B │") && art.contains('▶'),
            "LR-цепочка из двух узлов:\n{art}"
        );
        assert!(
            sc["summary"]
                .as_str()
                .expect("summary")
                .contains("flowchart"),
            "{}",
            sc["summary"]
        );
        // Без code/path — вежливая доменная ошибка, не protocol error.
        let no_args = &responses[1]["result"];
        assert_eq!(no_args["isError"], true, "{no_args}");
        assert!(
            no_args["content"][0]["text"]
                .as_str()
                .expect("text")
                .contains("нужен аргумент"),
            "{no_args}"
        );
    }

    #[tokio::test]
    async fn mermaid_render_reads_mmd_file_by_path() {
        let tmp = tempfile::tempdir().expect("tmp");
        std::fs::write(tmp.path().join("flow.mmd"), "graph TD\nA --> B\n").expect("mmd");
        let server = server();
        let call = format!(
            r#"{{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{{"name":"mermaid_render","arguments":{{"path":"{}"}}}}}}"#,
            tmp.path().join("flow.mmd").display()
        );
        let responses = run_lines_on(server, &[&call]).await;
        let result = &responses[0]["result"];
        assert_eq!(result["isError"], false, "{result}");
        let sc = &result["structuredContent"];
        assert_eq!(sc["kind"], "flowchart");
        let art = sc["art"].as_str().expect("art");
        assert!(
            art.contains("│ A │") && art.contains("│ B │") && art.contains('▼'),
            "TD-цепочка из файла:\n{art}"
        );
    }

    /// Сервер в режиме `--rw` на дефолтном конфиге.
    fn rw_server() -> McpServe {
        McpServe::with_mode(Arc::new(Config::default()), ServeMode::ReadWrite)
    }

    /// Мини-репозиторий для `agentsmd_generate` (по образцу фикстуры
    /// `agentsmd::tests`): манифест, спайн, каталог ADR, `.arch-handoff/`.
    fn agentsmd_repo(root: &Path) -> PathBuf {
        let repo = root.join("repo");
        std::fs::create_dir_all(repo.join("docs/adr")).expect("adr");
        std::fs::create_dir_all(repo.join(".arch-handoff")).expect("handoff");
        std::fs::write(repo.join("Cargo.toml"), "[package]\nname=\"r\"\n").expect("cargo");
        std::fs::write(
            repo.join("docs/ARCHITECTURE-SPINE.md"),
            "# Spine\n\n## AD-1. Формат id\nBinds: все\nPrevents: рассинхрон\nRule: правило\n",
        )
        .expect("spine");
        repo
    }

    /// YAML-фикстура рубрики из двух критериев (context вес 1, alternatives вес 3).
    fn rubric_fixture(root: &Path) -> PathBuf {
        let path = root.join("adr-quality.yaml");
        std::fs::write(
            &path,
            "name: adr-quality\n\
             description: Качество ADR\n\
             scale_max: 5\n\
             origin: anchor\n\
             criteria:\n  \
             - id: context\n    \
             name: Контекст\n    \
             description: Описан контекст и проблема\n    \
             weight: 1.0\n  \
             - id: alternatives\n    \
             name: Альтернативы\n    \
             description: Рассмотрены альтернативы\n    \
             weight: 3.0\n",
        )
        .expect("рубрика");
        path
    }

    #[test]
    fn bridge_lists_partition_registry() {
        // Белые списки, ручные имена и never-список попарно не пересекаются.
        for name in BRIDGE_READ_ONLY.iter().chain(BRIDGE_READ_WRITE) {
            assert!(
                !BRIDGE_NEVER.contains(name),
                "{name} и в белом списке, и в never"
            );
            assert!(
                !MANUAL_TOOLS.contains(name),
                "{name} — ручной и мостовой одновременно"
            );
        }
        for name in BRIDGE_NEVER {
            assert!(!MANUAL_TOOLS.contains(name), "{name} — ручной и в never");
        }
        // Все имена списков — реальные члены полного реестра дефолтного
        // конфига (страховка от переименований инструментов доменов).
        let registry = crate::tools::full_registry(&Config::default());
        for name in BRIDGE_READ_ONLY.iter().chain(BRIDGE_READ_WRITE) {
            if HARNESS_ONLY_TOOLS.contains(name) {
                // Домены сборки `harness` (кодовые харнессы, дистилляция): в
                // core-сборке их нет в реестре — мост пропускает их молча.
                if cfg!(feature = "harness") {
                    assert!(
                        registry.get(name).is_some(),
                        "{name} из белого списка отсутствует в full_registry"
                    );
                } else {
                    assert!(
                        registry.get(name).is_none(),
                        "{name} — инструмент сборки harness, в core его быть не должно"
                    );
                }
                continue;
            }
            assert!(
                registry.get(name).is_some(),
                "{name} из белого списка отсутствует в full_registry"
            );
        }
        for name in BRIDGE_NEVER {
            if HARNESS_ONLY_TOOLS.contains(name) {
                if cfg!(feature = "harness") {
                    assert!(
                        registry.get(name).is_some(),
                        "{name} из never-списка отсутствует в full_registry — список протух?"
                    );
                } else {
                    assert!(
                        registry.get(name).is_none(),
                        "{name} — инструмент сборки harness, в core его быть не должно"
                    );
                }
                continue;
            }
            assert!(
                registry.get(name).is_some(),
                "{name} из never-списка отсутствует в full_registry — список протух?"
            );
        }
    }

    #[tokio::test]
    async fn rw_mode_lists_bridge_write_tools_but_never_never() {
        let responses = run_lines_on(
            rw_server(),
            &[r#"{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}"#],
        )
        .await;
        let tools = responses[0]["result"]["tools"].as_array().expect("tools");
        let names: Vec<&str> = tools
            .iter()
            .map(|t| t["name"].as_str().expect("имя"))
            .collect();
        // Инструменты доменов сборки `harness` (skill_distill и др.) в
        // core-сборке в реестре отсутствуют — мост их пропускает.
        let expected_rw: Vec<&str> = BRIDGE_READ_WRITE
            .iter()
            .copied()
            .filter(|n| cfg!(feature = "harness") || !HARNESS_ONLY_TOOLS.contains(n))
            .collect();
        for rw in &expected_rw {
            assert!(
                names.contains(rw),
                "rw-инструмент '{rw}' нужен в --rw: {names:?}"
            );
        }
        // В core-сборке harness-инструментов нет и в выдаче.
        for rw in BRIDGE_READ_WRITE {
            if !cfg!(feature = "harness") && HARNESS_ONLY_TOOLS.contains(rw) {
                assert!(
                    !names.contains(rw),
                    "harness-инструмент '{rw}' не должен собираться в core: {names:?}"
                );
            }
        }
        for forbidden in BRIDGE_NEVER {
            assert!(
                !names.contains(forbidden),
                "never-инструмент '{forbidden}' закрыт и под --rw: {names:?}"
            );
        }
        assert_eq!(
            names.len(),
            MANUAL_TOOLS.len() + BRIDGE_READ_ONLY.len() + expected_rw.len(),
            "rw-режим: ручные + оба белых списка (в core — без harness-доменов)"
        );
        // Аннотации: mutating по классификации политики → destructiveHint.
        // handoff_create — в обеих сборках (core-модуль crate::handoff).
        let handoff = tools
            .iter()
            .find(|t| t["name"] == "handoff_create")
            .expect("handoff_create");
        assert_eq!(handoff["annotations"]["readOnlyHint"], false);
        assert_eq!(handoff["annotations"]["destructiveHint"], true);
        // Аддитивная запись (политика — ReadOnly): readOnlyHint=false по
        // членству в rw-списке, destructiveHint=false по классу риска.
        // (skill_distill живёт в домене сборки `harness`.)
        #[cfg(feature = "harness")]
        {
            let distill = tools
                .iter()
                .find(|t| t["name"] == "skill_distill")
                .expect("skill_distill");
            assert_eq!(distill["annotations"]["readOnlyHint"], false);
            assert_eq!(distill["annotations"]["destructiveHint"], false);
        }
    }

    #[tokio::test]
    async fn rw_tools_callable_only_in_rw_mode() {
        let tmp = tempfile::tempdir().expect("tmp");
        let repo = agentsmd_repo(tmp.path());
        let repo_str = repo.display().to_string();
        let call = |id: u64| {
            format!(
                r#"{{"jsonrpc":"2.0","id":{id},"method":"tools/call","params":{{"name":"agentsmd_generate","arguments":{{"repo":"{repo_str}"}}}}}}"#
            )
        };
        // ro-режим: rw-инструмент недоступен совсем (-32602), запись не идёт.
        let responses = run_lines(&[&call(1)]).await;
        assert_eq!(responses[0]["error"]["code"], INVALID_PARAMS);
        assert!(
            !repo.join("AGENTS.md").exists(),
            "в ro-режиме ничего не создаётся"
        );
        // rw-режим: вызов проходит через реестр, AGENTS.md создан.
        let responses = run_lines_on(rw_server(), &[&call(1)]).await;
        let result = &responses[0]["result"];
        assert_eq!(result["isError"], false, "{result}");
        assert_eq!(result["structuredContent"]["tool"], "agentsmd_generate");
        assert!(
            repo.join("AGENTS.md").is_file(),
            "AGENTS.md создан мостовым вызовом"
        );
    }

    #[tokio::test]
    async fn bridge_openapi_lint_runs_via_registry_with_cwd() {
        let tmp = tempfile::tempdir().expect("tmp");
        std::fs::write(
            tmp.path().join("api.yaml"),
            "openapi: 3.0.3\n\
             info:\n  \
             title: Pet Store API\n  \
             version: 1.0.0\n\
             paths:\n  \
             /v1/pets:\n    \
             get:\n      \
             operationId: listPets\n      \
             responses:\n        \
             '200':\n          \
             description: ok\n",
        )
        .expect("контракт");
        // Относительный путь резолвится от аргумента моста `cwd`.
        let call = format!(
            r#"{{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{{"name":"openapi_lint","arguments":{{"path":"api.yaml","cwd":"{}"}}}}}}"#,
            tmp.path().display()
        );
        let responses = run_lines(&[&call]).await;
        let result = &responses[0]["result"];
        assert_eq!(result["isError"], false, "{result}");
        let sc = &result["structuredContent"];
        assert_eq!(sc["tool"], "openapi_lint");
        let output = sc["output"].as_str().expect("output");
        assert!(output.contains("openapi:"), "отчёт инструмента: {output}");
        // content[0].text моста — сырой вывод инструмента, не JSON-обёртка.
        let text = result["content"][0]["text"].as_str().expect("text");
        assert_eq!(text, output);
    }

    #[tokio::test]
    async fn bridge_policy_require_confirm_becomes_is_error() {
        // Политика R1: agentsmd_generate классифицируется Mutating →
        // RequireConfirm; в неинтерактивном MCP это отказ с пояснением
        // (isError), файл не создаётся.
        let tmp = tempfile::tempdir().expect("tmp");
        let repo = agentsmd_repo(tmp.path());
        let mut cfg = Config::default();
        cfg.policy.autonomy = "R1".into();
        let server = McpServe::with_mode(Arc::new(cfg), ServeMode::ReadWrite);
        let call = format!(
            r#"{{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{{"name":"agentsmd_generate","arguments":{{"repo":"{}"}}}}}}"#,
            repo.display()
        );
        let responses = run_lines_on(server, &[&call]).await;
        let result = &responses[0]["result"];
        assert_eq!(result["isError"], true, "{result}");
        let text = result["content"][0]["text"].as_str().expect("text");
        assert!(
            text.contains("ТРЕБУЕТСЯ ПОДТВЕРЖДЕНИЕ"),
            "причина отказа политики: {text}"
        );
        assert!(
            !repo.join("AGENTS.md").exists(),
            "при RequireConfirm запись не идёт"
        );
    }

    #[tokio::test]
    async fn rubric_prompt_emits_prompts_schema_and_judge_config() {
        let tmp = tempfile::tempdir().expect("tmp");
        let rub = rubric_fixture(tmp.path());
        let server = server();
        let prompt_call = format!(
            r#"{{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{{"name":"rubric_prompt","arguments":{{"rubric":"{}","target_text":"контекст описан подробно"}}}}}}"#,
            rub.display()
        );
        let dyn_call = format!(
            r#"{{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{{"name":"rubric_prompt","arguments":{{"rubric":"{}","target_text":"текст","dynamic_subject":"ADR миграции"}}}}}}"#,
            rub.display()
        );
        let responses = run_lines_on(server, &[&prompt_call, &dyn_call]).await;
        let result = &responses[0]["result"];
        assert_eq!(result["isError"], false, "{result}");
        let sc = &result["structuredContent"];
        assert_eq!(sc["rubric"], "adr-quality");
        assert_eq!(sc["criteria"], 2);
        let system = sc["system_prompt"].as_str().expect("system");
        assert!(
            system.contains("НАЧАЛО ОЦЕНИВАЕМОГО ТЕКСТА"),
            "маркеры изоляции в системном промпте: {system}"
        );
        let user = sc["user_prompt"].as_str().expect("user");
        assert!(
            user.contains("контекст описан подробно"),
            "целевой текст в user-промпте: {user}"
        );
        let schema = &sc["response_json_schema"];
        assert_eq!(
            schema["properties"]["scores"]["items"]["properties"]["score"]["maximum"],
            5
        );
        assert_eq!(sc["judge_config"]["samples"], 3, "k сэмплов из [judge]");
        // dynamic_subject требует LLM у сервера — вежливая доменная ошибка.
        let dyn_result = &responses[1]["result"];
        assert_eq!(dyn_result["isError"], true, "{dyn_result}");
        let text = dyn_result["content"][0]["text"].as_str().expect("text");
        assert!(text.contains("dynamic_subject"), "{text}");
    }

    #[tokio::test]
    async fn rubric_verify_builds_report_and_counts_dropped() {
        let tmp = tempfile::tempdir().expect("tmp");
        let rub = rubric_fixture(tmp.path());
        let answers = [
            r#"{"scores":[{"criterion_id":"context","score":4,"rationale":"Цитата: \"контекст описан подробно\" — да"},{"criterion_id":"alternatives","score":2,"rationale":"Цитата: \"контекст описан подробно\" — альтернативы слабо"}],"verdict":"v1"}"#,
            r#"{"scores":[{"criterion_id":"context","score":2,"rationale":"Цитата: \"контекст описан подробно\" — слабо"}],"verdict":"v2"}"#,
            "это вообще не json судьи",
        ];
        let answers_json = serde_json::to_string(&answers).expect("json");
        let call = format!(
            r#"{{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{{"name":"rubric_verify","arguments":{{"rubric":"{}","target_text":"контекст описан подробно","answers":{answers_json},"model":"host-model-x"}}}}}}"#,
            rub.display()
        );
        let responses = run_lines(&[&call]).await;
        let result = &responses[0]["result"];
        assert_eq!(result["isError"], false, "{result}");
        let sc = &result["structuredContent"];
        assert_eq!(sc["judge_model"], "host-model-x");
        assert_eq!(sc["judge_samples"], 2, "битый ответ отброшен");
        assert_eq!(sc["answers"], json!({"total": 3, "valid": 2, "dropped": 1}));
        assert!(
            sc["warning"]
                .as_str()
                .expect("warning")
                .contains("отброшены"),
            "предупреждение о доле отброшенных: {sc}"
        );
        // context: сэмплы [4,2] → медиана 3.0 → балл 3, цитата из текста →
        // без флагов; alternatives: оценён один раз (2), пропуск = 1 →
        // сэмплы [2,1] → медиана 1.5 → балл 2 (round half away from zero),
        // цитата подтверждена → засчитан.
        let scores = sc["scores"].as_array().expect("scores");
        let context = &scores[0];
        assert_eq!(context["criterion_id"], "context");
        assert_eq!(context["score"], 3, "медиана [4,2]: {context}");
        assert_eq!(context["samples"], json!([4, 2]));
        assert_eq!(context["flags"], json!([]), "цитата подтверждена");
        let alternatives = &scores[1];
        assert_eq!(alternatives["samples"], json!([2, 1]));
        // Итог: (3*1 + 2*3) / (1+3) = 2.25.
        assert!(
            (sc["weighted_total"].as_f64().expect("итог") - 2.25).abs() < 1e-9,
            "{}",
            sc["weighted_total"]
        );
        assert_eq!(
            sc["verdict"], "v2",
            "вердикт — из последнего валидного сэмпла"
        );
        assert!(
            sc["report_markdown"]
                .as_str()
                .expect("markdown")
                .contains("# Оценка по рубрике «adr-quality»"),
            "markdown-отчёт как у rubric_run"
        );
    }

    #[tokio::test]
    async fn rubric_verify_judge_model_overrides_model_label() {
        // Anti-bias «автор = судья»: фактическая модель-судья фиксируется в
        // отчёте; `judge_model` перекрывает метку `model`, эхо — в markdown.
        let tmp = tempfile::tempdir().expect("tmp");
        let rub = rubric_fixture(tmp.path());
        let answer = r#"{"scores":[{"criterion_id":"context","score":4,"rationale":"Цитата: \"контекст описан подробно\" — да"},{"criterion_id":"alternatives","score":2,"rationale":"Цитата: \"контекст описан подробно\" — слабо"}],"verdict":"v"}"#;
        let answers = [answer, answer];
        let answers_json = serde_json::to_string(&answers).expect("json");
        let call = |id: u64, extra: &str| {
            format!(
                r#"{{"jsonrpc":"2.0","id":{id},"method":"tools/call","params":{{"name":"rubric_verify","arguments":{{"rubric":"{}","target_text":"контекст описан подробно","answers":{answers_json}{extra}}}}}}}"#,
                rub.display()
            )
        };
        let owned = [
            call(1, r#","judge_model":"claude-opus-4-8""#),
            call(2, r#","model":"host-default","judge_model":"glm-5-3""#),
            call(3, ""),
        ];
        let refs: Vec<&str> = owned.iter().map(String::as_str).collect();
        let responses = run_lines(&refs).await;
        let sc = &responses[0]["result"]["structuredContent"];
        assert_eq!(sc["judge_model"], "claude-opus-4-8", "{sc}");
        let md = sc["report_markdown"].as_str().expect("markdown");
        assert!(
            md.contains("**Судья:** claude-opus-4-8"),
            "эхо судьи в человекочитаемом отчёте: {md}"
        );
        // При конфликте меток побеждает judge_model (фактический судья).
        let sc2 = &responses[1]["result"]["structuredContent"];
        assert_eq!(sc2["judge_model"], "glm-5-3", "{sc2}");
        // Без меток — дефолт split-judge.
        let sc3 = &responses[2]["result"]["structuredContent"];
        assert_eq!(sc3["judge_model"], "external (split-judge)", "{sc3}");
    }

    #[tokio::test]
    async fn rubric_verify_flags_fabricated_quote_and_survives_partial_evidence() {
        let tmp = tempfile::tempdir().expect("tmp");
        let rub = rubric_fixture(tmp.path());
        // context: балл 5 с выдуманной цитатой (в обоих сэмплах) →
        // evidence_not_found и исключение из итога; alternatives: балл 1
        // («свидетельство отсутствует», цитата не нужна) → засчитан.
        let answer = r#"{"scores":[{"criterion_id":"context","score":5,"rationale":"Цитата: \"выдуманная фраза вне текста\" — якобы есть"},{"criterion_id":"alternatives","score":1,"rationale":"свидетельство отсутствует"}],"verdict":"спорно"}"#;
        let answers = [answer, answer];
        let answers_json = serde_json::to_string(&answers).expect("json");
        let call = format!(
            r#"{{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{{"name":"rubric_verify","arguments":{{"rubric":"{}","target_text":"контекст описан кратко","answers":{answers_json}}}}}}}"#,
            rub.display()
        );
        let responses = run_lines(&[&call]).await;
        let result = &responses[0]["result"];
        assert_eq!(result["isError"], false, "{result}");
        let sc = &result["structuredContent"];
        let context = &sc["scores"][0];
        assert_eq!(context["flags"], json!(["evidence_not_found"]), "{context}");
        // Из итога context исключён: только alternatives (1*3/3 = 1.0).
        assert!(
            (sc["weighted_total"].as_f64().expect("итог") - 1.0).abs() < 1e-9,
            "{}",
            sc["weighted_total"]
        );
    }

    #[tokio::test]
    async fn rubric_verify_all_broken_or_empty_answers_is_error() {
        let tmp = tempfile::tempdir().expect("tmp");
        let rub = rubric_fixture(tmp.path());
        let all_broken = format!(
            r#"{{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{{"name":"rubric_verify","arguments":{{"rubric":"{}","target_text":"текст","answers":["мусор","ещё мусор"]}}}}}}"#,
            rub.display()
        );
        let empty = format!(
            r#"{{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{{"name":"rubric_verify","arguments":{{"rubric":"{}","target_text":"текст","answers":[]}}}}}}"#,
            rub.display()
        );
        let too_many = format!(
            r#"{{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{{"name":"rubric_verify","arguments":{{"rubric":"{}","target_text":"текст","answers":{}}}}}}}"#,
            rub.display(),
            serde_json::to_string(&vec!["x"; MAX_VERIFY_ANSWERS + 1]).expect("json")
        );
        let responses = run_lines(&[&all_broken, &empty, &too_many]).await;
        // Все ответы битые — доменная ошибка isError (не protocol error).
        let broken = &responses[0]["result"];
        assert_eq!(broken["isError"], true, "{broken}");
        let text = broken["content"][0]["text"].as_str().expect("text");
        assert!(text.contains("ни один из 2 ответов"), "{text}");
        // Пустой массив и превышение лимита — ошибки параметров -32602.
        assert_eq!(responses[1]["error"]["code"], INVALID_PARAMS);
        assert_eq!(responses[2]["error"]["code"], INVALID_PARAMS);
    }

    #[tokio::test]
    async fn rubric_run_cli_model_skips_api_key_precheck() {
        // kind="cli": судья — внешний CLI-харнесс, уже авторизованный на
        // машине, — собственный API-ключ Spine не нужен, предпроверка ключа
        // пропускается. Провайдер cli здесь не настроен → вызов доходит до
        // исполнения и падает доменной ошибкой (isError), а НЕ protocol
        // error -32603 про отсутствующий ключ.
        let tmp = tempfile::tempdir().expect("tmp");
        let rub = rubric_fixture(tmp.path());
        let mut cfg = Config::default();
        cfg.models.insert(
            "cli-judge".into(),
            ModelConfig {
                base_url: "http://127.0.0.1:9".into(),
                model: "stub".into(),
                kind: Some("cli".into()),
                command: Some("definitely-missing-cli-harness-binary".into()),
                api_key_env: "ARCH_HARNESS_TEST_MISSING_KEY_XYZ".into(),
                api_key_file: None,
                ..ModelConfig::default()
            },
        );
        cfg.default_model = "cli-judge".into();
        let server = McpServe::new(Arc::new(cfg));
        let call = format!(
            r#"{{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{{"name":"rubric_run","arguments":{{"rubric":"{}","target_text":"текст"}}}}}}"#,
            rub.display()
        );
        let response = server.handle_line(&call).await.expect("ответ");
        assert!(
            response.get("error").is_none(),
            "предпроверка ключа (-32603) должна быть пропущена для kind=cli: {response}"
        );
        assert_eq!(
            response["result"]["isError"], true,
            "исполнение без настроенного cli-провайдера — доменная ошибка: {response}"
        );
    }
}
