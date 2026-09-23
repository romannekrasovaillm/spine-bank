//! Типы и константы протокола MCP-сервера (разбиение B1): коды ошибок
//! JSON-RPC, лимиты, карточка плейбуков и [`PLAYBOOK_PROMPTS`], белые списки
//! моста ([`BRIDGE_READ_ONLY`]/[`BRIDGE_READ_WRITE`]/[`MANUAL_TOOLS`]/
//! [`PATH_ARG_ALIASES`]), режим [`ServeMode`], ошибка вызова [`CallError`],
//! состояние сервера [`McpServe`] (конструкторы, сессия, допуск моста),
//! ответы ok/error, разбор аргументов, blocking-пул и вердикты журнала.

use std::fmt;
use std::sync::Arc;

use serde_json::{Value, json};

use crate::config::Config;
use crate::error::Result;
use crate::tool::ToolRegistry;

/// Код JSON-RPC «разбор запроса не удался» (невалидный JSON).
pub(super) const PARSE_ERROR: i64 = -32700;
/// Код JSON-RPC «некорректный запрос» (не объект, нет `method`).
pub(super) const INVALID_REQUEST: i64 = -32600;
/// Код JSON-RPC «метод не найден».
pub(super) const METHOD_NOT_FOUND: i64 = -32601;
/// Код JSON-RPC «некорректные параметры» (аргументы, неизвестный инструмент).
pub(super) const INVALID_PARAMS: i64 = -32602;
/// Код JSON-RPC «внутренняя ошибка» (недоступная capability: нет LLM-ключа).
pub(super) const INTERNAL_ERROR: i64 = -32603;

/// Максимальная длина одной входной строки (`16 МиБ`). Защита от безграничного
/// роста буфера построчного чтения; самый тяжёлый легальный вход — текст
/// документа для `rubric_run` (лимит рубрики 24k символов), запас ~600×.
pub(super) const MAX_LINE_BYTES: usize = 16 * 1024 * 1024;

/// Дефолтный лимит хитов `kb_search` (контракт инструмента kb.rs).
pub(super) const KB_SEARCH_DEFAULT_LIMIT: usize = 10;
/// Дефолтный лимит хитов `skill_search` (контракт инструмента plugin.rs).
pub(super) const SKILL_SEARCH_DEFAULT_LIMIT: usize = 8;
/// Потолок хитов поисков знаний (общий у kb.rs и plugin.rs).
pub(super) const KNOWLEDGE_MAX_HITS: usize = 20;
/// Потолок символов тела скилла в ответе `skill_load` (как у агентного
/// инструмента: `ToolOutput::truncated(16_000)` — защита контекста клиента).
pub(super) const SKILL_TEXT_MAX_CHARS: usize = 16_000;

/// Потолок текстового вывода мостовых инструментов реестра (защита контекста
/// хоста; тот же прецедент 16k, что у `skill_load` и агентных инструментов).
pub(super) const BRIDGE_OUTPUT_MAX_CHARS: usize = 16_000;

/// Потолок числа ответов хоста в `rubric_verify` (k сэмплов судьи из
/// `[judge]` — единицы; лимит отсекает ошибочные гигантские пачки).
pub(super) const MAX_VERIFY_ANSWERS: usize = 32;

/// Потолок символов значения аргумента промпта, эхом вставляемого в текст
/// сообщения `prompts/get`: аргумент — короткий путь/имя/предмет, а не
/// документ (длинный ввод хоста не должен раздувать сообщение-команду).
pub(super) const MAX_PROMPT_ARG_VALUE_CHARS: usize = 500;

/// Статическая карточка плейбука-промпта (MCP prompts): имя (= имя скилла
/// плагина spine-workflows), встроенный текст SKILL.md (запасной источник
/// на машине без `arch-be init`) и объявление аргументов для `prompts/list`.
pub(super) struct PlaybookPrompt {
    /// Имя промпта (= имя скилла-плейбука).
    pub(super) name: &'static str,
    /// Встроенный полный текст SKILL.md (embedded-ассет [`crate::assets`]).
    pub(super) embedded: &'static str,
    /// Аргументы промпта: (имя, описание). Объявляются, только если сценарий
    /// плейбука параметризован (путь/файл/предмет); все необязательные —
    /// слэш-команда обязана работать и без аргументов (цель уточняется из
    /// контекста диалога).
    pub(super) arguments: &'static [(&'static str, &'static str)],
}

/// Плейбуки плагина spine-workflows как MCP-промпты (слэш-команды хоста,
/// пункт 11 бэклога волны 3). Порядок фиксирован: стабильный `prompts/list`,
/// та же последовательность в `docs/mcp.md`.
pub(super) const PLAYBOOK_PROMPTS: &[PlaybookPrompt] = &[
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
        name: "spine-semantic-judge",
        embedded: crate::assets::PLUGIN_SPINE_WORKFLOWS_SKILLS_SPINE_SEMANTIC_JUDGE_SKILL_MD,
        arguments: &[(
            "subject",
            "субъект досье: путь к ADR или файлу кода либо идентификатор сущности модели",
        )],
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
    PlaybookPrompt {
        name: "spine-judge-handover",
        embedded: crate::assets::PLUGIN_SPINE_WORKFLOWS_SKILLS_SPINE_JUDGE_HANDOVER_SKILL_MD,
        arguments: &[
            ("path", "Опц.: каталог кейса, судейство которого передаём"),
            (
                "rubric",
                "Опц.: рубрика оценки решений (по умолчанию adr_quality)",
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
    "rubric_accept",
    "rubric_handover",
    "rubric_list",
    "rule_template_list",
    "rule_template_show",
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
    "rule_template_apply",
    "skill_distill",
];

/// Инструменты реестра, которые НЕ отдаются наружу ни в одном режиме:
/// exec/write/веб/субагенты — принадлежность хоста (у Claude Code и др.
/// они свои); `rubric_evaluate`/`rubric_generate` требуют LLM на стороне
/// сервера — в MCP-инверсии её нет, вместо них split-judge
/// (`rubric_prompt`/`rubric_verify`). Охраняется тестом реестра.
pub(super) const BRIDGE_NEVER: &[&str] = &[
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
pub(super) const HARNESS_ONLY_TOOLS: &[&str] = &[
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
    /// `arch-be mcp serve --rw=reports`: запись разрешена ТОЛЬКО отчётам
    /// рубрики (`rubric_verify` → `reports/rubric/`). Судейскому харнессу не
    /// нужны `adr_new`, `delta_propose` и `handoff_create`, а широкий `--rw`
    /// открывал их все разом (J7, ADR-048).
    Reports,
    /// `arch-be mcp serve --rw`: дополнительно [`BRIDGE_READ_WRITE`]
    /// (аддитивные записи в рабочий каталог клиента). [`BRIDGE_NEVER`]
    /// остаётся закрытым и в этом режиме.
    ReadWrite,
}

impl ServeMode {
    /// Открыт ли контур записи (режимы `--rw` и `--rw=reports`).
    pub(super) fn allows_write(self) -> bool {
        matches!(self, Self::ReadWrite | Self::Reports)
    }

    /// Запись ограничена отчётами рубрики (режим `--rw=reports`): мостовые
    /// пишущие инструменты закрыты.
    pub(super) fn reports_only(self) -> bool {
        matches!(self, Self::Reports)
    }

    /// Человекочитаемый режим подключения — для `doctor --host` (J7).
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::ReadOnly => "read-only",
            Self::Reports => "rw=reports",
            Self::ReadWrite => "rw",
        }
    }

    /// Разбор значения флага `--rw`: без значения (или `full`) — полный rw,
    /// `reports` — узкий; иное — ошибка с перечнем допустимых.
    ///
    /// # Errors
    /// Значение флага не распознано.
    pub fn parse_rw(value: Option<&str>) -> std::result::Result<Self, String> {
        match value.map(str::trim) {
            // Флаг НЕ передан — строго read-only: `None` здесь означает
            // «нет --rw», а не «--rw без значения» (то даёт default_missing_value
            // clap'а — строку `full`). Спутать эти два случая значило бы
            // открывать запись там, где её не просили.
            None => Ok(Self::ReadOnly),
            Some("" | "full" | "true") => Ok(Self::ReadWrite),
            Some("reports") => Ok(Self::Reports),
            Some(other) => Err(format!(
                "неизвестный режим записи '{other}': ожидается `--rw` (полный) или `--rw=reports`                  (только отчёты рубрики)"
            )),
        }
    }
}

/// Ошибка вызова инструмента: на каком уровне протокола отвечать.
#[derive(Debug)]
pub(super) enum CallError {
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
    pub(super) fn execution(tool: &str, e: impl fmt::Display) -> Self {
        Self::Execution(format!("{tool}: {e}"))
    }

    /// Ошибка параметров (`-32602`).
    pub(super) fn invalid_params(message: String) -> Self {
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
    pub(super) cfg: Arc<Config>,
    pub(super) mode: ServeMode,
    pub(super) registry: ToolRegistry,
    /// Модель доверия `command_succeeds` (A3, ADR-053): серверный снимок,
    /// вычисленный один раз при старте из `ARCH_NO_EXEC` — по умолчанию
    /// no-exec=вкл (сервер обслуживает агента на потенциально чужом
    /// репозитории), снятие — явным `ARCH_NO_EXEC=0`. Наследуется всеми
    /// инструментами, доходящими до исполнения правил реестра (ручными —
    /// напрямую, мостовыми — через `ToolContext.exec`).
    pub(super) exec: crate::cmd_trust::ExecPolicy,
    /// Состояние сессии: хост из рукопожатия, выданный идентификатор, счётчик
    /// вызовов и выданные промпты судьи (ADR-048). Транспорт stdio — один
    /// процесс на сессию, поэтому состояние живёт в сервере, а не в соединении;
    /// `Mutex` нужен лишь потому, что хендлеры берут `&self`.
    pub(super) session: std::sync::Mutex<crate::judge::SessionState>,
}

/// Ответ-успех JSON-RPC.
pub(super) fn ok_response(id: &Value, result: &Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

/// Ответ-ошибка JSON-RPC.
pub(super) fn error_response(id: &Value, code: i64, message: impl Into<String>) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {"code": code, "message": message.into()},
    })
}

/// Разбирает аргументы инструмента в типизированную структуру;
/// ошибка десериализации → `-32602`.
pub(super) fn parse_args<T: serde::de::DeserializeOwned>(
    args: Value,
    tool: &str,
) -> std::result::Result<T, CallError> {
    serde_json::from_value(args)
        .map_err(|e| CallError::invalid_params(format!("{tool}: невалидные аргументы: {e}")))
}

/// Прогоняет синхронную доменную функцию на blocking-пуле (fitness-правила
/// `command_succeeds` и обходы fs не должны держать worker runtime).
pub(super) async fn blocking<T, F>(tool: &str, f: F) -> std::result::Result<T, CallError>
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
pub(super) fn verdict_from_structured(v: &Value) -> (&'static str, Vec<String>) {
    match v.get("passed").and_then(Value::as_bool) {
        Some(true) => ("pass", Vec::new()),
        Some(false) => ("fail", crate::mcp_journal::failed_rule_names(v)),
        None => ("ok", Vec::new()),
    }
}

/// Вердикт журнала по текстовому выводу мостового инструмента: верификаторы
/// реестра несут в text JSON `{passed, issues, summary}` — разбираем его;
/// не-JSON вывод (markdown-отчёты) журналируется как `ok` без разбора.
pub(super) fn verdict_from_bridge_text(text: &str) -> (&'static str, Vec<String>) {
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
    /// Политика исполнения `command_succeeds` — серверный снимок
    /// [`crate::cmd_trust::ExecPolicy::mcp`] (A3): no-exec=вкл, пока
    /// окружение сервера не задаст явное `ARCH_NO_EXEC=0`.
    #[must_use]
    pub fn with_mode(cfg: Arc<Config>, mode: ServeMode) -> Self {
        let registry = crate::tools::full_registry(&cfg);
        Self {
            cfg,
            mode,
            registry,
            exec: crate::cmd_trust::ExecPolicy::mcp(),
            session: std::sync::Mutex::new(crate::judge::SessionState::new()),
        }
    }

    /// Подменяет политику исполнения `command_succeeds` явным снимком —
    /// для тестов без мутаций окружения (многопоточный прогон, `std::env`
    /// трогать нельзя) и для встраивающих вызовов с собственным решением.
    #[must_use]
    pub fn with_exec_policy(mut self, exec: crate::cmd_trust::ExecPolicy) -> Self {
        self.exec = exec;
        self
    }

    /// Состояние сессии. Захват яда мьютекса не ошибка: паника внутри
    /// критической секции не оставляет состояние неконсистентным (только
    /// счётчики и метки), поэтому работа продолжается на восстановленном
    /// значении — отказ сервера из-за этого был бы хуже.
    pub(super) fn session(&self) -> std::sync::MutexGuard<'_, crate::judge::SessionState> {
        self.session
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Имя разрешено мосту в текущем режиме: только белые списки режима,
    /// с защитным отсечением [`BRIDGE_NEVER`] и ручных имён [`MANUAL_TOOLS`]
    /// (технически списки не пересекаются — проверяется тестом реестра;
    /// отсечение здесь — страховка на будущую правку списков: never-имя
    /// не уйдёт наружу даже при ошибочном добавлении в белый список).
    pub(super) fn bridge_allowed(&self, name: &str) -> bool {
        if BRIDGE_NEVER.contains(&name) || MANUAL_TOOLS.contains(&name) {
            return false;
        }
        BRIDGE_READ_ONLY.contains(&name)
            || (self.mode.allows_write()
                && !self.mode.reports_only()
                && BRIDGE_READ_WRITE.contains(&name))
    }
}

#[cfg(test)]
mod serve_mode_tests {
    use super::*;

    /// Разбор `--rw`: отсутствие флага — строго read-only. Спутать «нет флага»
    /// с «флаг без значения» значило бы открывать запись там, где её не просили
    /// (J7, ADR-048).
    #[test]
    fn rw_flag_parsing_keeps_default_read_only() {
        assert_eq!(
            ServeMode::parse_rw(None).expect("None"),
            ServeMode::ReadOnly
        );
        assert_eq!(
            ServeMode::parse_rw(Some("full")).expect("full"),
            ServeMode::ReadWrite
        );
        assert_eq!(
            ServeMode::parse_rw(Some("")).expect("пустое значение"),
            ServeMode::ReadWrite
        );
        assert_eq!(
            ServeMode::parse_rw(Some("reports")).expect("reports"),
            ServeMode::Reports
        );
        assert!(
            ServeMode::parse_rw(Some("всё")).is_err(),
            "чужое значение — ошибка"
        );
        // Режимы записи и их подписи для `doctor --host`.
        assert!(!ServeMode::ReadOnly.allows_write());
        assert!(ServeMode::Reports.allows_write() && ServeMode::Reports.reports_only());
        assert!(ServeMode::ReadWrite.allows_write() && !ServeMode::ReadWrite.reports_only());
        assert_eq!(ServeMode::ReadOnly.label(), "read-only");
        assert_eq!(ServeMode::Reports.label(), "rw=reports");
        assert_eq!(ServeMode::ReadWrite.label(), "rw");
    }
}
