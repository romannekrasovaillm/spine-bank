//! Конфигурация харнесса (`config.toml`).
//!
//! Порядок поиска конфига: `--config <path>` → `./arch-harness.toml` →
//! `~/.config/arch-harness/config.toml` → встроенные дефолты.
//! Команда `arch-be init` пишет дефолтный конфиг и ассеты в [`Config::home_dir`].
//!
//! Разбиение модуля (волна B1): `models` — LLM-провайдеры
//! ([`ModelConfig`]/[`OAuthConfig`]); `lessons` — контур обучения и гейта
//! ([`LessonsConfig`], [`SignificanceConfig`], [`GateConfig`] и вложенные секции).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{HarnessError, Result};

mod lessons;
mod models;

#[cfg(test)]
pub(crate) use lessons::default_contract_globs;
pub use lessons::{
    ArchDriftConfig, BodySeverity, ConditionSeverity, ConditionsConfig, DEFAULT_EVIDENCE_MIN_BYTES,
    DecisionPolicyConfig, DecisionQualityConfig, DriftConfig, EnvelopeConfig, EvidenceConfig,
    EvidenceSemantics, ExecutableRequired, GateConfig, HumanPolicy, LessonsConfig,
    OpenspecCoverageConfig, OpenspecCoverageScope, OverridesConfig, RequiredRules,
    RuleWeakenedConfig, SecretScope, SecretSeverity, SecretsConfig, SemanticQualityConfig,
    SemanticScope, SignificanceConfig, SurveyConfig, TraceConfig,
};
pub use models::{ModelConfig, OAuthConfig};

/// Корневая конфигурация харнесса.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Имя модели по умолчанию (ключ в [`Config::models`]).
    pub default_model: String,
    /// Настроенные LLM-провайдеры (OpenAI-совместимые endpoint'ы).
    pub models: BTreeMap<String, ModelConfig>,
    /// Параметры агентного цикла.
    pub agent: AgentConfig,
    /// Локальная база знаний (каталоги доменных документов).
    pub knowledge: KnowledgeConfig,
    /// Веб-доступ (поиск, фетч, кураторский список архитектурных сайтов).
    pub web: WebConfig,
    /// Интеграция Archify (Node CLI диаграмм JSON IR → HTML/SVG).
    pub archify: ArchifyConfig,
    /// Адаптеры кодовых харнессов (Claude Code, Qwen Code, `OpenClaw`, …).
    pub harnesses: BTreeMap<String, CodingHarnessConfig>,
    /// Настройки MCP.
    pub mcp: McpSettings,
    /// Каталоги плагинов (скиллы + MCP в одном пакете).
    pub plugins: PluginsConfig,
    /// Политика автономии инструментов (R-уровни).
    pub policy: PolicyConfig,
    /// Хуки жизненного цикла (shell-команды на событиях агента).
    pub hooks: HooksConfig,
    /// Изоляция окружения bash-команд (scrub секретоподобных переменных).
    pub bash: BashConfig,
    /// Периметр файловых инструментов (песочница путей, ADR-083): зона
    /// чтения/записи = рабочий каталог + `fs_allow`.
    pub tools: ToolsConfig,
    /// Настройки планировщика.
    pub cron: CronSettings,
    /// Настройки LLM-судьи рубрик (калибровка, ADR-004).
    pub judge: JudgeConfig,
    /// Порог независимости судьи для метрики доверия (ADR-048).
    pub trust: TrustConfig,
    /// Настройки мутационного прогона redteam (волна E 0.3.14, ADR-065).
    pub redteam: RedteamConfig,
    /// Настройки флота прогонов кодовых харнессов (изоляция и гейт мерджа).
    pub fleet: FleetConfig,
    /// Реестр уроков (Lesson Ledger, ADR-085): пороги жизненного цикла по
    /// счётчикам доказательств и бюджет блока инъекции (L3, ADR-087).
    pub lessons: LessonsConfig,
    /// Пороги маршрутизации значимости (Architecture Significance Score).
    pub significance: SignificanceConfig,
    /// Матрица обязательных составляющих гейта по маршруту (П1 ДКА, ADR-039):
    /// SKIP обязательной составляющей даёт INCOMPLETE и exit 3, а не PASS.
    pub gate: GateConfig,
    /// Семантика артефактов Evidence Bundle (Н1 волны A 0.3.4, ADR-041):
    /// «артефакт есть» ≠ «артефакт написан».
    pub evidence: EvidenceConfig,
    /// Требование исполняемой проверки инвариантов (ADR-050).
    pub trace: TraceConfig,
    /// Проверки дрейфа «модель ↔ код» (волна C: C3 за флагом).
    pub drift: DriftConfig,
    /// Денилист обследования (B8 волны B): пути и хосты, не являющиеся
    /// интеграциями (lock-файлы, лицензии, метаданные чартов, funding-ссылки).
    pub survey: SurveyConfig,
    /// Пути к ассетам, отчётам и сессиям.
    pub paths: PathsConfig,
    /// Откуда конфиг загружен (нужно `harness_run` для горячего
    /// перечитывания адаптеров — правки config.toml подхватываются без
    /// перезапуска сессии). В файл не сериализуется.
    #[serde(skip)]
    pub loaded_from: Option<PathBuf>,
}

/// Параметры агентного цикла.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentConfig {
    /// Максимум итераций «модель ↔ инструменты» за один ход.
    pub max_tool_turns: usize,
    /// Бюджет контекста в токенах (грубая оценка, 4 символа ≈ 1 токен).
    pub context_budget_tokens: usize,
    /// Стримить ответы модели (дельты в TUI/stdout).
    pub stream: bool,
    /// Порог L1-компактификации, % бюджета: маскирование старых
    /// tool-результатов (усечение с пометкой).
    pub compact_l1_pct: usize,
    /// Порог L3-компактификации, % бюджета: LLM-саммари истории в одно
    /// сообщение (последняя user-задача не трогается). >100 — отключено.
    pub compact_l3_pct: usize,
    /// Режим памяти сбоев инструментов (правило «ошибся дважды → урок»,
    /// см. модуль [`crate::failure_memory`]).
    pub failure_memory: FailureMemoryMode,
    /// Стенной бюджет «длинного терпения» на транспортные сбои модели,
    /// секунд: короткие ретраи провайдера (десятки секунд) не переживают
    /// минутные окна DPI/VPN, поэтому сетевую ошибку агент пережидает,
    /// повторяя вызов модели целиком, пока не выйдет этот бюджет. 0 —
    /// выключено (поведение ≤0.3.12: ход падает сразу).
    pub network_retry_budget_secs: u64,
}

/// Режим памяти сбоев инструментов (`[agent] failure_memory`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FailureMemoryMode {
    /// Выключено: подписи сбоев не считаются.
    Off,
    /// Урок предлагается: заметка в транскрипт + предложение в
    /// `paths.state_dir/failure_lessons.md` (просмотр — `/lessons`).
    Propose,
    /// Как `propose`, плюс append урока в AGENTS.md рабочего каталога
    /// (вне зоны `ARCH:GENERATED`; только append, без перезаписи).
    Write,
}

impl FailureMemoryMode {
    /// Строковое имя режима (как в конфиге) — для показа в `/lessons`.
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Propose => "propose",
            Self::Write => "write",
        }
    }
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            max_tool_turns: 4800,
            context_budget_tokens: 6_000_000,
            stream: true,
            compact_l1_pct: 70,
            compact_l3_pct: 95,
            failure_memory: FailureMemoryMode::Propose,
            network_retry_budget_secs: 180,
        }
    }
}

/// Локальная база знаний для `kb search`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct KnowledgeConfig {
    /// Каталоги с доменными документами.
    pub dirs: Vec<PathBuf>,
    /// Расширения файлов для индексации.
    pub extensions: Vec<String>,
}

impl Default for KnowledgeConfig {
    fn default() -> Self {
        let home = dirs::home_dir().unwrap_or_default();
        Self {
            // Нейтральные плейсхолдеры: свои каталоги задаются в config.toml.
            dirs: vec![
                home.join("knowledge/architecture"),
                home.join("knowledge/papers"),
                home.join("knowledge/skills"),
            ],
            extensions: vec!["md".into(), "txt".into(), "rst".into()],
        }
    }
}

/// Кураторский сайт архитектурных знаний.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArchSite {
    /// Короткое имя, напр. `aws-arch`.
    pub name: String,
    /// Базовый URL, напр. `https://docs.aws.amazon.com/architecture/`.
    pub base_url: String,
    /// Домен для site:-ограниченного поиска, напр. `docs.aws.amazon.com`.
    pub domain: String,
    /// Однострочное описание (чем сайт полезен архитектору).
    pub description: String,
}

/// Serde-дефолт для флагов, отсутствующих в конфиге: обратная совместимость —
/// новый ключ без явного значения не меняет поведение (включено).
fn default_true() -> bool {
    true
}

/// Serde-дефолт исполняемого файла Node.js для Archify-раннера.
fn default_node_bin() -> String {
    "node".to_owned()
}

/// Serde-дефолт таймаута вызова Archify CLI, сек.
fn default_archify_timeout() -> u64 {
    crate::archify::DEFAULT_TIMEOUT_SECS
}

/// Настройки интеграции Archify (Node CLI диаграмм JSON IR → HTML/SVG).
///
/// Личные пути машины (`cli_path`, `node_bin`) живут только здесь,
/// в пользовательском конфиге — никогда в коде репозитория (AGENTS).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ArchifyConfig {
    /// Выключатель: false — инструменты `archify_*` не регистрируются в
    /// инструментарии сессии; по умолчанию true.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Путь к `bin/archify.mjs` установленного Archify (пусто — интеграция
    /// не настроена: инструменты отвечают инструкцией по установке).
    pub cli_path: PathBuf,
    /// Исполняемый файл Node.js (>=18).
    #[serde(default = "default_node_bin")]
    pub node_bin: String,
    /// Таймаут одного вызова CLI, секунды.
    #[serde(default = "default_archify_timeout")]
    pub timeout_secs: u64,
}

impl Default for ArchifyConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            cli_path: PathBuf::new(),
            node_bin: default_node_bin(),
            timeout_secs: default_archify_timeout(),
        }
    }
}

/// Настройки веб-доступа.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct WebConfig {
    /// Выключатель веб-канала: false — веб-инструменты не регистрируются в
    /// инструментарии сессии (контур банка, AD-BE5); по умолчанию true.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Endpoint поиска (`DuckDuckGo` HTML).
    pub search_base: String,
    /// User-Agent для запросов.
    pub user_agent: String,
    /// Таймаут запроса, секунды.
    pub timeout_secs: u64,
    /// Максимум символов текста после html→text.
    pub max_fetch_chars: usize,
    /// Кураторский список сайтов для архитектора.
    pub arch_sites: Vec<ArchSite>,
}

impl Default for WebConfig {
    fn default() -> Self {
        let site = |name: &str, base: &str, domain: &str, desc: &str| ArchSite {
            name: name.into(),
            base_url: base.into(),
            domain: domain.into(),
            description: desc.into(),
        };
        Self {
            enabled: true,
            search_base: "https://html.duckduckgo.com/html/".into(),
            user_agent: "arch-harness/0.1 (+solution-architect harness)".into(),
            timeout_secs: 30,
            max_fetch_chars: 24_000,
            arch_sites: vec![
                site(
                    "aws-arch",
                    "https://docs.aws.amazon.com/architecture/",
                    "docs.aws.amazon.com",
                    "AWS Architecture Center: reference architectures, Well-Architected",
                ),
                site(
                    "azure-arch",
                    "https://learn.microsoft.com/azure/architecture/",
                    "learn.microsoft.com",
                    "Azure Architecture Center: паттерны, reference architectures",
                ),
                site(
                    "gcp-arch",
                    "https://cloud.google.com/architecture",
                    "cloud.google.com",
                    "Google Cloud Architecture Center",
                ),
                site(
                    "fowler",
                    "https://martinfowler.com/architecture/",
                    "martinfowler.com",
                    "Мартин Фаулер: эссе по архитектуре, микросервисам, эволюционному дизайну",
                ),
                site(
                    "infoq-arch",
                    "https://www.infoq.com/architecture-design/",
                    "infoq.com",
                    "InfoQ Architecture & Design: статьи и тренды",
                ),
                site(
                    "microservices-io",
                    "https://microservices.io/",
                    "microservices.io",
                    "Каталог паттернов микросервисов (Крис Ричардсон)",
                ),
                site(
                    "c4",
                    "https://c4model.com/",
                    "c4model.com",
                    "C4 model: нотация визуализации архитектуры",
                ),
                site(
                    "arc42",
                    "https://docs.arc42.org/",
                    "docs.arc42.org",
                    "arc42: шаблон документирования архитектуры",
                ),
                site(
                    "togaf",
                    "https://pubs.opengroup.org/togaf-standard/",
                    "pubs.opengroup.org",
                    "TOGAF Standard (Open Group)",
                ),
                site(
                    "sei",
                    "https://insights.sei.cmu.edu/library/",
                    "insights.sei.cmu.edu",
                    "SEI/CAD: ATAM, архитектурные тактики, quality attributes",
                ),
                site(
                    "awesome-arch",
                    "https://awesome-architecture.com/",
                    "awesome-architecture.com",
                    "Кураторский список ресурсов по software architecture",
                ),
            ],
        }
    }
}

/// Как адаптер передаёт промпт кодовому харнессу.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PromptMode {
    /// Промпт — позиционный аргумент (`claude "..."`).
    Positional,
    /// Промпт через флаг (`agent --prompt "..."`).
    Flag,
    /// Промпт пишется в stdin.
    Stdin,
}

/// Whitelist окружения по умолчанию для строгого режима (C2, 0.3.12): минимум,
/// без которого процесс кодового агента не стартует, — исполняемый поиск,
/// домашний каталог, локаль, терминал и временный каталог. Секретоподобные
/// переменные сервера (`*_API_KEY`, `*_TOKEN`, `GITHUB_TOKEN`, …) в него не
/// входят: на Critical/bank-profile они не наследуются по умолчанию.
pub const DEFAULT_ENV_ALLOW: [&str; 5] = ["PATH", "HOME", "LANG", "TERM", "TMPDIR"];

/// Режим вызова кодового агента (ADR-057, вариант (а) «конфиг + auto + fallback»).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AcpMode {
    /// Умный выбор (дефолт): секция `[harnesses.<имя>.acp]` задекларирована →
    /// ACP; не задекларирована → headless. Провал инициализации ACP (таймаут
    /// `initialize`, ошибка протокола, ранний exit) — откат на headless с
    /// предупреждением в итоге прогона.
    #[default]
    Auto,
    /// ACP обязателен: провал инициализации — ошибка прогона БЕЗ отката.
    Acp,
    /// Принудительный headless (поведение до ADR-057) — для прогонов, где ACP
    /// не нужен даже при задекларированной секции.
    Prompt,
}

/// Секция `[harnesses.<имя>.acp]` (C1, ADR-057): команда ACP-сервера и таймауты
/// протокола. Наличие секции — единственный признак «ACP задекларирован».
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AcpConfig {
    /// Имя бинаря ACP-адаптера в PATH (например, `claude-code-acp`).
    pub binary: String,
    /// Аргументы ACP-сервера (например, `["--acp"]` для `qwen --acp`).
    pub args: Vec<String>,
    /// Таймаут ответа на `initialize`, секунды (0 — без таймаута).
    pub init_timeout_secs: u64,
    /// Окно graceful-остановки после `session/cancel`, секунды, до SIGKILL
    /// процессной группы (последний рубеж — существующая механика).
    pub cancel_grace_secs: u64,
}

impl Default for AcpConfig {
    fn default() -> Self {
        Self {
            binary: String::new(),
            args: Vec::new(),
            init_timeout_secs: 30,
            cancel_grace_secs: 10,
        }
    }
}

/// Адаптер кодового харнесса (Claude Code, Qwen Code, `OpenClaw`, Hermes, Theseus, `CodeWhale`, Kimi Code).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CodingHarnessConfig {
    /// Имя бинаря в PATH.
    pub binary: String,
    /// Дополнительные аргументы (плейсхолдер `{prompt}` подставляется при `PromptMode::Flag`).
    pub args: Vec<String>,
    /// Режим передачи промпта.
    pub prompt_mode: PromptMode,
    /// Дополнительные переменные окружения.
    pub env: BTreeMap<String, String>,
    /// Whitelist наследуемых переменных окружения: при непустом списке процесс
    /// харнесса стартует с ЧИСТЫМ окружением (`env_clear`) и получает только
    /// перечисленные переменные + `env` адаптера. Пустой список — наследовать
    /// всё окружение процесса (поведение по умолчанию). Закрывает разрыв
    /// «окружение протекает между харнессами» (P1): чужие `*_MODEL`,
    /// прокси-переменные и ключи хоста не утекают в дочерний процесс.
    #[serde(default)]
    pub env_allow: Vec<String>,
    /// Полное наследование окружения вопреки строгому дефолту (C2, 0.3.12):
    /// на маршруте Critical или при `bank_profile = true` процесс исполнителя
    /// по умолчанию получает whitelist [`DEFAULT_ENV_ALLOW`], а не всё
    /// окружение сервера. `true` — осознанный возврат к наследованию: в итог
    /// прогона пишется предупреждение (секреты ключей/токенов доступны
    /// исполнителю). Явно заданный `env_allow` приоритетнее флага.
    #[serde(default)]
    pub env_inherit: bool,
    /// Банковский профиль (C2, 0.3.12): строгий дефолт окружения для ВСЕХ
    /// прогонов этого адаптера, независимо от маршрута пакета — как если бы
    /// каждый прогон был Critical. Включайте в конфиге банка (или в шаблоне,
    /// из которого разворачиваются адаптеры).
    #[serde(default)]
    pub bank_profile: bool,
    /// Таймаут прогона, секунды (абсолютный потолок).
    pub timeout_secs: u64,
    /// Таймаут тишины, секунды: прогон прерывается, если харнесс не пишет
    /// в stdout/stderr И не меняет файлы в репозитории дольше этого срока.
    /// 0 — отключить (только абсолютный потолок). Работающий молча харнесс
    /// (длинный tool-вызов внутри Claude Code и т.п.) при наличии активности
    /// файловой системы НЕ считается зависшим.
    pub idle_timeout_secs: u64,
    /// Авто-коммит незакоммиченных правок после успешного прогона
    /// (`Termination::Completed`): контракт TASK.md требует от исполнителя
    /// финального коммита, но исполнитель может его не сделать — тогда
    /// харнесс сам фиксирует оставшиеся изменения (кроме `.arch-handoff/`
    /// и мусора вида `__pycache__/`). Работа исполнителя всегда оказывается
    /// в git — это точка интеграции параллельных прогонов.
    pub auto_commit: bool,
    /// Пост-гейт (A4): после авто-коммита `run_harness` сам прогоняет
    /// `arch-be gate` по рабочему дереву прогона с базой `baseline_commit`
    /// из `.arch-handoff/MANIFEST.json` — вне окружения исполнителя. Красный
    /// вердикт (FAIL/INCOMPLETE/TIMEOUT) делает итог прогона ошибкой для
    /// оркестратора. `false` — выключен явным решением адаптера
    /// (предупреждение в выводе и журнале остаётся).
    pub post_gate: bool,
    /// Таймаут пост-гейта, секунды (дефолт 600). Гейт не завершился —
    /// вердикт TIMEOUT, итог красный: непроверенный результат не выдаётся
    /// за принятый (RA-5: `dependency_direction` на больших реестрах
    /// патологически дорог — это сигнал, а не блокер механизма).
    pub post_gate_timeout_secs: u64,
    /// Команда приёмки прогона (L2, ADR-086; см. `docs/harness_integrations.md`): shell-строка вне песочницы исполнителя; не задано — дефолт `arch-be gate .` при `CONSTRAINTS.yaml` в корне, пусто — выключено.
    pub accept_cmd: Option<String>,
    /// Режим вызова агента (C1, ADR-057): `auto` (дефолт) | `acp` | `prompt`.
    /// Аддитивно: старые конфиги без поля работают как `auto`, а без секции
    /// `acp` — как headless (поведение до ADR-057).
    #[serde(default)]
    pub mode: AcpMode,
    /// Секция `[harnesses.<имя>.acp]` — ACP-команда адаптера. `None` — ACP не
    /// задекларирован: при `mode = "auto"` прогон идёт headless.
    #[serde(default)]
    pub acp: Option<AcpConfig>,
}

impl Default for CodingHarnessConfig {
    fn default() -> Self {
        Self {
            binary: String::new(),
            args: Vec::new(),
            prompt_mode: PromptMode::Positional,
            env: BTreeMap::new(),
            env_allow: Vec::new(),
            env_inherit: false,
            bank_profile: false,
            timeout_secs: 1800,
            idle_timeout_secs: 600,
            auto_commit: true,
            post_gate: true,
            post_gate_timeout_secs: 600,
            accept_cmd: None,
            mode: AcpMode::Auto,
            acp: None,
        }
    }
}

/// Настройки MCP: путь к файлу серверов (формат как у Claude Code `mcp.json`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct McpSettings {
    /// Путь к `mcp.json` (`{"mcpServers": {name: {command, args, env}}}`).
    pub servers_file: PathBuf,
    /// Подключаться к серверам при старте (иначе — лениво, по первому вызову).
    pub connect_on_start: bool,
    /// Таймаут MCP-вызова, секунды.
    pub timeout_secs: u64,
}

impl Default for McpSettings {
    fn default() -> Self {
        Self {
            servers_file: Config::home_dir().join("mcp.json"),
            connect_on_start: false,
            timeout_secs: 60,
        }
    }
}

/// Настройки плагинов: каталоги пакетов «скиллы + MCP»
/// (открытый стандарт agent-plugins.org: `plugin.json` + `skills/*/SKILL.md`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PluginsConfig {
    /// Каталоги с плагинами (каждый прямой потомок — плагин).
    pub dirs: Vec<PathBuf>,
    /// Подхватывать MCP-серверы из плагинов (`mcpServers` в plugin.json
    /// или `.mcp.json` в корне плагина) в общий MCP-пул.
    pub include_mcp: bool,
    /// Исполнять хуки плагинов (`hooks/hooks.json`): shell-команды из
    /// установленных плагинов — включайте только для доверенных библиотек.
    /// Дефолт `true` (как у `include_mcp`).
    pub include_hooks: bool,
    /// Резервный встроенный набор скиллов восстановления (профиль `reverse`,
    /// ADR-083 п.3): когда пользовательская библиотека пуста, `skill_search`/
    /// `skill_load` подхватывают `reverse-discovery`/`recover-review` из ассетов
    /// бинаря с предупреждением в журнал (не молчанием). Дефолт `false` —
    /// обычный прогон видит только поставленную библиотеку.
    pub reverse_fallback: bool,
}

impl Default for PluginsConfig {
    fn default() -> Self {
        Self {
            // По умолчанию — только библиотека харнесса; свои каталоги
            // плагинов добавляются в config.toml.
            dirs: vec![Config::home_dir().join("plugins")],
            include_mcp: true,
            include_hooks: true,
            reverse_fallback: false,
        }
    }
}

/// Хуки жизненного цикла (см. [`crate::hooks`]). Пусто — без хуков.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct HooksConfig {
    /// Спецификации хуков: `[[hooks.specs]]` с `event/tool/command/timeout_secs`.
    pub specs: Vec<crate::hooks::HookSpec>,
}

/// Изоляция окружения bash-команд (defensive pattern `DeepSeek` Harness:
/// «никогда не давай недоверенному выводу ambient-окружение»).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct BashConfig {
    /// Выбрасывать из окружения дочернего процесса переменные с
    /// секретоподобными именами (токены имени: KEY, SECRET, TOKEN,
    /// PASSWORD, PASSWD, PASS, CREDENTIAL(S), AUTH). Дефолт `true`:
    /// команды, которые пишет модель, не видят ключи провайдеров и не
    /// могут отправить их наружу (curl, логи, spill-файлы).
    pub env_scrub: bool,
    /// Точные имена переменных, которые пропускаются НЕСМОТРЯ на scrub
    /// (например `GH_TOKEN` для `gh`). Пусто — исключений нет.
    pub env_allow: Vec<String>,
    /// Изоляция дочерних команд: `"none"` (дефолт — текущее поведение) или
    /// `"bwrap"` (bubblewrap: рабочее дерево rw, остальная ФС ro, сети нет —
    /// egress дочерних процессов закрыт; см. ADR-038). Неизвестное значение —
    /// ошибка конфига при загрузке ([`Config::load`]).
    pub sandbox: String,
}

impl Default for BashConfig {
    fn default() -> Self {
        Self {
            env_scrub: true,
            env_allow: Vec::new(),
            sandbox: "none".into(),
        }
    }
}

/// Режим изоляции дочерних команд bash (`[bash] sandbox`, ADR-038).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BashSandbox {
    /// Без изоляции (поведение по умолчанию).
    None,
    /// bubblewrap: рабочее дерево rw, остальная ФС ro, `--unshare-net`.
    Bwrap,
}

impl BashConfig {
    /// Разобранное значение `sandbox`.
    ///
    /// # Errors
    /// Неизвестное значение: допустимы только `"none"` и `"bwrap"`.
    pub fn sandbox_mode(&self) -> Result<BashSandbox> {
        match self.sandbox.as_str() {
            "none" => Ok(BashSandbox::None),
            "bwrap" => Ok(BashSandbox::Bwrap),
            other => Err(HarnessError::Config(format!(
                "[bash] sandbox: неизвестное значение \"{other}\" \
                 (допустимы: \"none\", \"bwrap\")"
            ))),
        }
    }
}

/// Периметр файловых инструментов (песочница путей, ADR-083 п.1).
///
/// Файловые инструменты (`read_file`/`write_file`/`edit_file`/`glob`/`grep`)
/// не выходят за рабочую зону: рабочий каталог сессии ([`crate::tool::ToolContext::cwd`])
/// плюс каталоги из `fs_allow`. Побег через `..`, абсолютный путь или symlink
/// наружу — отказ (`is_error`) без исполнения. Это НЕ замена bash-песочнице
/// (bwrap, ADR-038): та про дочерние процессы, эта — про путь, который
/// инструмент открывает сам.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ToolsConfig {
    /// Дополнительные каталоги вне `cwd`, куда файловым инструментам
    /// разрешено (абсолютные пути; `~` раскрывается при загрузке). Дефолт —
    /// `~/.arch-harness` (дом ассетов харнесса: скиллы, отчёты, сессии —
    /// инструменты их читают из любой рабочей директории).
    pub fs_allow: Vec<PathBuf>,
    /// Песочница путей включена. `false` — аварийный откат к поведению до
    /// ADR-083 (любой путь без проверки зоны): осознанный обход на случай,
    /// если реальный прогон упрётся в ложное срабатывание. Дефолт `true`.
    pub fs_sandbox: bool,
    /// Каталог записи (профиль `reverse`, ADR-083 п.3): когда задан, пишущие
    /// файловые инструменты (`write_file`/`edit_file`) могут писать только внутрь
    /// `<cwd>/<fs_write_dir>` — исходники доступны на чтение, но не на правку.
    /// `None` (дефолт) — запись в пределах общей зоны `read = write`. Путь
    /// относительный — от рабочего каталога сессии.
    pub fs_write_dir: Option<PathBuf>,
}

impl Default for ToolsConfig {
    fn default() -> Self {
        // Литерал `~/.arch-harness`, а не `Config::home_dir()`: `init`
        // сериализует дефолт в файл, и пользователь должен увидеть ровно
        // `~/.arch-harness` (портабельно, раскрывается `expand_tildes` при
        // загрузке), а не абсолютный путь конкретной машины.
        Self {
            fs_allow: vec![PathBuf::from("~/.arch-harness")],
            fs_sandbox: true,
            fs_write_dir: None,
        }
    }
}

/// Политика автономии инструментов (R-уровни R0–R5, по AI-Disrupt PDLC).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PolicyConfig {
    /// Уровень автономии: R0 (всё через человека) … R5 (полная).
    /// Дефолт R2: чтения и изменения в рабочем каталоге — авто, деструктив —
    /// отказ. R4 — деструктив с подтверждением. R5 не рекомендуется.
    pub autonomy: String,
}

impl Default for PolicyConfig {
    fn default() -> Self {
        Self {
            autonomy: "R2".into(),
        }
    }
}

/// Настройки планировщика md-задач.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CronSettings {
    /// Файл расписания (`cron.toml`).
    pub file: PathBuf,
    /// Каталог отчётов крона (по умолчанию `paths.reports_dir/cron`).
    pub out_dir: Option<PathBuf>,
}

impl Default for CronSettings {
    fn default() -> Self {
        Self {
            file: Config::home_dir().join("cron.toml"),
            out_dir: None,
        }
    }
}

/// Настройки метрики доверия (`arch-be trust`, ADR-048).
///
/// Шкала отвечает, насколько можно верить зелёному контуру; её ступени — не
/// гейт, а измерение. Единственный ключ здесь — порог независимости судьи для
/// верхней ступени.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TrustConfig {
    /// Минимальный уровень независимости судьи, который считается достаточным
    /// для пятой ступени (ADR-048): `declared` (дефолт) — «метки передал хост»
    /// достаточно; `launched` — требуем, чтобы судью запускал сам Spine.
    /// Уровни: `none` < `declared` < `declared_cross_family` < `launched` <
    /// `launched_cross_family`.
    pub min_independence: String,
    /// Минимальная доля правил с подтверждёнными зубьями для ступени 3, когда
    /// в модели нет несущих инвариантов (`load_bearing`) — читается только в
    /// режиме `require_teeth` (B3, ADR-065). Порог — решение архитектора
    /// ([РЕШЕНИЕ ЧЕЛОВЕКА] задания 0.3.14): дефолт 0.2, не зашит в логику.
    pub behaviour_share_min: f64,
    /// Строгий режим ступени 3 (схема «warn → error», ADR-065): `false`
    /// (дефолт) — измерение зубьев показывается предупреждением якоря, условие
    /// ступени прежнее (поведение 0.3.13 не ломается); `true` — условие
    /// измеренное: несущие инварианты покрыты правилами с подтверждёнными
    /// зубьями либо доля таких правил ≥ `behaviour_share_min`.
    pub require_teeth: bool,
}

impl Default for TrustConfig {
    fn default() -> Self {
        Self {
            min_independence: crate::judge::INDEPENDENCE_DECLARED.to_string(),
            behaviour_share_min: DEFAULT_BEHAVIOUR_SHARE_MIN,
            require_teeth: false,
        }
    }
}

/// Дефолт порога доли правил с зубьями для ступени 3 доверия (B3, ADR-065).
pub const DEFAULT_BEHAVIOUR_SHARE_MIN: f64 = 0.2;

/// Настройки мутационного прогона (`arch-be redteam`, волна E 0.3.14).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct RedteamConfig {
    /// Порог доли обнаружения КОДОВЫХ дефектов (слой code: мутаторы D11, D11b,
    /// D15, D18–D24), ниже которого прогон красный (E2, ADR-065). `None`
    /// (дефолт) — кодовая доля только показывается; порог суммарной доли
    /// (`--min-detection`, дефолт 0.78) не меняется. Значение порога —
    /// решение архитектора ([РЕШЕНИЕ ЧЕЛОВЕКА] задания 0.3.14).
    pub min_code_detection: Option<f64>,
}

/// Настройки LLM-судьи рубрик (калибровка и верификация, ADR-004).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct JudgeConfig {
    /// Сэмплов судьи на критерий: независимые прогоны, итоговый балл —
    /// медиана сэмплов (1 — одиночная оценка, поведение до ADR-004).
    pub samples: usize,
    /// Порог population-σ баллов сэмплов: выше — критерий помечается
    /// флагом `unstable` (разброс судьи, к итогу балл допускается).
    pub unstable_stdev: f64,
    /// Минимальное сходство цитаты-свидетельства с оцениваемым текстом
    /// (0..=1; точный substring-матч засчитывается всегда). Цитата слабее
    /// порога при балле ≥ 2 → флаг `evidence_not_found`, критерий
    /// исключается из взвешенного итога.
    pub evidence_min_similarity: f64,
    /// Максимально допустимый MAE golden-прогона (`arch-be bench run --golden`):
    /// итог выше порога → exit code 1 (регрессионный гейт качества судьи).
    pub golden_max_mae: f64,
    /// Ризонинг для запросов судьи: `Some(true)` — слить карту `thinking_on`
    /// модели (вернуть старое поведение); `Some(false)`/`None` (дефолт) —
    /// судья работает БЕЗ ризонинга: судья — задача структурной экстракции
    /// (JSON с баллами и цитатами), а thinking-токены входят в бюджет
    /// `max_tokens` провайдера и обрывали JSON посреди ответа
    /// (кейс 2026-09-01, deepseek-v4-flash).
    pub thinking: Option<bool>,
    /// Семейства моделей по префиксам метки (`claude = "anthropic"`): судья и
    /// автор из одного семейства делят слепые зоны, и «независимость» между
    /// ними — только по названию (ADR-048). Дополняет дефолтную таблицу
    /// [`crate::judge::DEFAULT_FAMILIES`] и переопределяет её по самому
    /// длинному подошедшему префиксу.
    pub families: BTreeMap<String, String>,
    /// Порог «чистой сессии» (ADR-048): режим `declared` и больше стольких
    /// вызовов MCP в сессии до выдачи промпта судьи — примечание
    /// `judge_session_not_clean` в паспорте («судейство шло в рабочей сессии:
    /// судья мог видеть контекст автора»). Это КОСВЕННЫЙ признак, а не находка.
    pub clean_session_max_calls: usize,
    /// Писать в отчёт рубрики, кто организовал судейство: git `user.name` и
    /// `user.email` репозитория (поле `provenance.operator`, ADR-048).
    /// Это запись из git-конфига, а не подпись: личность механики не
    /// удостоверяется, а имя с адресом попадают в коммитимый JSON отчёта —
    /// поэтому ключ существует (`false` выключает запись).
    pub record_operator: bool,
    /// E8.2: кэш вердикта судьи по хэшу досье, модели, рубрики и настроек.
    /// `true` — повторное ревью неизменённого кода не вызывает модель; запись
    /// кэша видна в отчёте (`provenance.cache`), обход — `rubric run --no-cache`.
    /// `false` (дефолт) — каждый прогон спрашивает модель.
    pub cache: bool,
    /// E8.1: адаптивное число сэмплов — `true` спрашивает судью один раз, когда
    /// первый ответ однозначен (все баллы на краях шкалы), и добирает сэмплы до
    /// `samples`, когда балл в зоне сомнения (`2..scale_max-1`) или ответ
    /// неразбираем. `false` (дефолт) — прежнее поведение: всегда `samples`.
    /// Число сэмплов видно в отчёте, поэтому экономия не прячется от аудита.
    pub adaptive_samples: bool,
}

impl Default for JudgeConfig {
    fn default() -> Self {
        Self {
            samples: 3,
            unstable_stdev: 1.0,
            evidence_min_similarity: 0.8,
            golden_max_mae: 1.0,
            thinking: None,
            families: BTreeMap::new(),
            clean_session_max_calls: crate::judge::DEFAULT_CLEAN_SESSION_MAX_CALLS,
            record_operator: true,
            cache: false,
            adaptive_samples: false,
        }
    }
}

/// Настройки флота прогонов кодовых харнессов: enforced-изоляция работы
/// агента и гейт интеграции (мотив — AI-native SDLC playbook Anthropic:
/// «агент не аппрувит свой код и не имеет пути в main» — enforced платформой,
/// а не дисциплиной промптов).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct FleetConfig {
    /// true — прогон кодового харнесса (`harness_run`, `arch harness-run`)
    /// обязан идти в изолированном git worktree (ветка `arch/<харнесс>-<timestamp>`,
    /// фабрика [`crate::worktree`]), а не в основном рабочем дереве; каталог
    /// не git-репозиторий — понятная ошибка ДО запуска харнесса.
    /// Дефолт `false` — поведение прежних версий (прогон в дереве как есть).
    pub require_worktree: bool,
    /// Гейт мерджа результата прогона в основную ветку (`arch fleet merge`):
    /// `owner` (дефолт) — мерж только с явным подтверждением владельца
    /// (`--owner-approve`); без флага печатается сводка прогона (diff stat,
    /// коммиты, статус контракта из evidence) и мерж отклоняется с exit 1.
    /// `none` — без гейта. Неизвестные значения трактуются как `owner`
    /// (безопасная интерпретация).
    pub merge_gate: String,
}

impl Default for FleetConfig {
    fn default() -> Self {
        Self {
            require_worktree: false,
            merge_gate: "owner".into(),
        }
    }
}

/// Пути к ассетам и данным харнесса.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PathsConfig {
    /// Корень ассетов (prompts/, rubrics/, benchmarks/, ascii/).
    pub assets_dir: PathBuf,
    /// Каталог отчётов (рубрики, бенчи, контроль).
    pub reports_dir: PathBuf,
    /// Каталог сессий (append-only журналы агента).
    pub sessions_dir: PathBuf,
    /// Каталог состояния харнесса (персистентные счётчики и артефакты
    /// вроде `failure_memory.json`/`failure_lessons.md` модуля
    /// [`crate::failure_memory`]).
    pub state_dir: PathBuf,
    /// Глобальная md-память харнесса: содержимое дописывается в системный
    /// промпт каждой сессии (см. модуль `memory`).
    pub memory_file: PathBuf,
}

impl Default for PathsConfig {
    fn default() -> Self {
        let home = Config::home_dir();
        Self {
            assets_dir: home.join("assets"),
            reports_dir: home.join("reports"),
            sessions_dir: home.join("sessions"),
            state_dir: home.join("state"),
            memory_file: home.join("MEMORY.md"),
        }
    }
}

impl PathsConfig {
    /// Каталог библиотеки промптов.
    #[must_use]
    pub fn prompts_dir(&self) -> PathBuf {
        self.assets_dir.join("prompts")
    }

    /// Каталог якорных рубрик.
    #[must_use]
    pub fn rubrics_dir(&self) -> PathBuf {
        self.assets_dir.join("rubrics")
    }

    /// Каталог бенчмарков.
    #[must_use]
    pub fn benchmarks_dir(&self) -> PathBuf {
        self.assets_dir.join("benchmarks")
    }

    /// Каталог библиотеки шаблонов исполняемых правил
    /// (`arch-be rules template …`, [`crate::rule_templates`]).
    #[must_use]
    pub fn rule_templates_dir(&self) -> PathBuf {
        self.assets_dir.join("rule-templates")
    }

    /// Каталог JSON-отчётов eval-сьютов: `evals/` в корне данных харнесса
    /// (по умолчанию `~/.arch-harness/evals`; следует за переопределением
    /// `reports_dir` — корнем считается его родитель).
    #[must_use]
    pub fn evals_dir(&self) -> PathBuf {
        self.reports_dir.parent().map_or_else(
            || Config::home_dir().join("evals"),
            |root| root.join("evals"),
        )
    }
}

/// JSON-карта параметра ризонинга (`thinking: {type: ...}`) для тела запроса.
type ThinkingMap = Option<serde_json::Map<String, serde_json::Value>>;

/// Карты ризонинга в стиле `thinking: {type: enabled/disabled}`
/// (`DeepSeek` V4, GLM-4.x/5.x): возвращает (on, off).
fn thinking_type_maps() -> (ThinkingMap, ThinkingMap) {
    let make = |kind: &str| {
        let mut inner = serde_json::Map::new();
        inner.insert("type".into(), kind.into());
        let mut outer = serde_json::Map::new();
        outer.insert("thinking".into(), inner.into());
        Some(outer)
    };
    (make("enabled"), make("disabled"))
}

impl Default for Config {
    fn default() -> Self {
        // Модели сверены с официальной документацией (август 2026):
        // - DeepSeek: deepseek-chat/reasoner сняты 2026-07-24 → v4-flash/v4-pro;
        //   deepseek-v4-flash выведена → deepseek-flash (V4.1-Flash, тариф Flash);
        //   ризонинг — `thinking: {type: enabled/disabled}` (api-docs.deepseek.com/
        //   guides/thinking_mode); с tools требуется эхо reasoning_content (см. llm.rs);
        // - Kimi: kimi-k2 снят 2025-05-25 → kimi-k3 (1M ctx), ризонинг —
        //   `reasoning_effort: low/high/max` (platform.kimi.ai/docs/models);
        // - GLM: glm-4.6 → glm-4.7, ризонинг — `thinking: {type: ...}` (docs.z.ai);
        //   glm-5.3-flash — окно 1.3M, thinking не отключается (см. ниже);
        // - GigaChat (сентябрь 2026, ADR-021): GigaChat-2/3, контекст 128K,
        //   OAuth2 вместо статического ключа (developers.sber.ru/docs/ru/gigachat).
        let mut models = BTreeMap::new();
        let (think_on, think_off) = thinking_type_maps();
        models.insert(
            "deepseek".into(),
            ModelConfig {
                base_url: "https://api.deepseek.com/v1".into(),
                model: "deepseek-flash".into(),
                api_key_env: "DEEPSEEK_API_KEY".into(),
                context_limit: Some(1_000_000),
                thinking_on: think_on.clone(),
                thinking_off: think_off.clone(),
                ..ModelConfig::default()
            },
        );
        models.insert(
            "deepseek-pro".into(),
            ModelConfig {
                base_url: "https://api.deepseek.com/v1".into(),
                model: "deepseek-v4-pro".into(),
                api_key_env: "DEEPSEEK_API_KEY".into(),
                timeout_secs: 300,
                context_limit: Some(1_000_000),
                thinking_on: think_on.clone(),
                thinking_off: think_off,
                ..ModelConfig::default()
            },
        );
        models.insert(
            "kimi".into(),
            ModelConfig {
                // Официальная coding-поверхность Kimi Code (доки kimi.com/code):
                // модели k3 / k3-256k; старая /v1 → 404. thinking-параметры
                // поверхность НЕ принимает (400 без reasoning_content в истории
                // — его харнесс эхом возвращает); temperature — только 1,
                // поэтому не шлём вовсе. Ключ — env KIMI_API_KEY или файл.
                base_url: "https://api.kimi.com/coding/v1".into(),
                model: "k3".into(),
                api_key_env: "KIMI_API_KEY".into(),
                api_key_file: Some("~/.kimi_api_key".into()),
                // K3 ризонит всегда: цепочка рассуждений съедает бюджет до
                // начала видимого ответа — дефолтные 8192 обрывали генерацию
                // (finish_reason=length, кейс 2026-10-14). Стартуем с 32K;
                // при length харнесс эскалирует max_tokens автоматически
                // (llm::escalated_max_tokens, потолок 65536).
                max_tokens: Some(32768),
                context_limit: Some(1_000_000),
                ..ModelConfig::default()
            },
        );
        let (glm_on, glm_off) = thinking_type_maps();
        let glm_entry = |model: &str, timeout: u64, context_limit: usize| ModelConfig {
            // Международная площадка Z.AI (для Китая — open.bigmodel.cn).
            base_url: "https://api.z.ai/api/paas/v4".into(),
            model: model.into(),
            api_key_env: "ZHIPU_API_KEY".into(),
            timeout_secs: timeout,
            context_limit: Some(context_limit),
            thinking_on: glm_on.clone(),
            thinking_off: glm_off.clone(),
            ..ModelConfig::default()
        };
        models.insert("glm".into(), glm_entry("glm-5.2", 180, 1_000_000));
        models.insert("glm-4.7".into(), glm_entry("glm-4.7", 180, 204_800));
        models.insert("glm-air".into(), glm_entry("glm-4.5-air", 120, 131_072));
        models.insert("glm-flash".into(), glm_entry("glm-4.7-flash", 120, 204_800));
        // GLM-5.3-Flash: окно 1.3M; thinking НЕ отключается (HTTP 1210 на
        // thinking.type=disabled, проверено 28.08) — «off» эмулируем картой
        // enabled + reasoning_effort=low (допустимы low|high|max).
        let glm53f_off = {
            let mut m = glm_on.clone().unwrap_or_default();
            m.insert("reasoning_effort".into(), "low".into());
            Some(m)
        };
        models.insert(
            "glm-5.3-flash".into(),
            ModelConfig {
                thinking_off: glm53f_off,
                ..glm_entry("glm-5.3-flash", 180, 1_310_720)
            },
        );
        // GigaChat (ADR-021, developers.sber.ru/docs/ru/gigachat): OAuth2 вместо
        // статического ключа — Basic-ключ из GIGACHAT_API_KEY идёт на
        // oauth.token_url (дефолт фабрики ngw.devices.sberbank.ru), далее Bearer
        // (TTL 30 мин, упреждающий рефреш). base_url не задан → пресет фабрики
        // https://api.giga.chat/v1; User-Agent обязателен (без него 403) —
        // ставит фабрика. thinking в API нет — карты ризонинга не задаём.
        // ca_pem_file в дефолт не включаем: путь к PEM корня НУЦ Минцифры —
        // машинно-зависимый (живой профиль — в config.example.toml).
        // scope — выбор владельца инсталляции (PERS — 1 поток; B2B/CORP — 10).
        let gigachat_entry = |model: &str, timeout: u64| ModelConfig {
            model: model.into(),
            api_key_env: "GIGACHAT_API_KEY".into(),
            timeout_secs: timeout,
            context_limit: Some(128_000),
            oauth: Some(OAuthConfig {
                token_url: String::new(),
                scope: "GIGACHAT_API_PERS".into(),
            }),
            ..ModelConfig::default()
        };
        models.insert("gigachat".into(), gigachat_entry("GigaChat-2-Pro", 180));
        models.insert("gigachat-max".into(), gigachat_entry("GigaChat-2-Max", 300));
        // GigaChat-3-Ultra: только физлица (Freemium; на платных тарифах и для
        // юрлиц недоступна — developers.sber.ru/docs/ru/gigachat/models/
        // gigachat-3-ultra, обновлено 17.07.2026). Согласуется со scope PERS.
        models.insert(
            "gigachat-ultra".into(),
            gigachat_entry("GigaChat-3-Ultra", 300),
        );
        // Корпоративные ключи (юрлица, scope B2B/CORP — 10 потоков) дефолтными
        // строками пикера не дублируем: подключение — копией любой секции
        // gigachat* в config.toml с заменой api_key_env (напр. на
        // GIGACHAT_API_KEY_CORP) и oauth.scope (шаблон в config.example.toml).

        let harness = |binary: &str, args: &[&str], mode: PromptMode| CodingHarnessConfig {
            binary: binary.into(),
            args: args.iter().map(|s| (*s).into()).collect(),
            prompt_mode: mode,
            ..CodingHarnessConfig::default()
        };
        let mut harnesses = BTreeMap::new();
        // Claude Code в headless (`-p` без TTY) на файловых операциях встаёт на
        // permission-промпте и ждёт вечно → --dangerously-skip-permissions
        // обязателен для unattended-прогонов. Права процесса ограничены
        // каталогом репозитория; уберите флаг в config.toml для интерактива.
        harnesses.insert(
            "claude-code".into(),
            harness(
                "claude",
                &["-p", "--dangerously-skip-permissions"],
                PromptMode::Stdin,
            ),
        );
        harnesses.insert("qwen-code".into(), harness("qwen", &[], PromptMode::Stdin));
        // Флаги валидированы живыми прогонами флота (бенч 04_payment-idempotency,
        // 2026-08): неверные режимы/флаги давали код 2 на argparse.
        harnesses.insert(
            "openclaw".into(),
            harness(
                "openclaw",
                &["agent", "--agent", "main", "--message", "{prompt}"],
                PromptMode::Flag,
            ),
        );
        harnesses.insert(
            "hermes".into(),
            harness("hermes", &["-z", "{prompt}"], PromptMode::Flag),
        );
        harnesses.insert(
            "theseus".into(),
            harness("theseus", &["-p", "{prompt}"], PromptMode::Flag),
        );
        harnesses.insert(
            "codewhale".into(),
            harness("codewhale", &["-p", "{prompt}"], PromptMode::Flag),
        );
        // Kimi Code: headless — `kimi -p PROMPT`. Permission-флаг НЕ нужен и
        // недопустим: в `-p`-режиме regular-инструменты всегда исполняются под
        // auto-политикой (без интерактивных промптов), а `--yolo`/`--auto` с
        // `--prompt` несовместимы — запуск отклоняется (документация
        // kimi-command, 2026-08). Модель — `-m <alias>` (по умолчанию
        // `default_model` из конфига kimi).
        harnesses.insert(
            "kimi-code".into(),
            harness("kimi", &["-p", "{prompt}"], PromptMode::Flag),
        );

        Self {
            default_model: "deepseek".into(),
            models,
            agent: AgentConfig::default(),
            knowledge: KnowledgeConfig::default(),
            web: WebConfig::default(),
            archify: ArchifyConfig::default(),
            harnesses,
            mcp: McpSettings::default(),
            plugins: PluginsConfig::default(),
            policy: PolicyConfig::default(),
            hooks: HooksConfig::default(),
            bash: BashConfig::default(),
            tools: ToolsConfig::default(),
            cron: CronSettings::default(),
            judge: JudgeConfig::default(),
            trust: TrustConfig::default(),
            redteam: RedteamConfig::default(),
            fleet: FleetConfig::default(),
            lessons: LessonsConfig::default(),
            significance: SignificanceConfig::default(),
            gate: GateConfig::default(),
            evidence: EvidenceConfig::default(),
            trace: TraceConfig::default(),
            drift: DriftConfig::default(),
            survey: SurveyConfig::default(),
            paths: PathsConfig::default(),
            loaded_from: None,
        }
    }
}

impl Config {
    /// Домашний каталог харнесса (`~/.arch-harness` или `$ARCH_HOME`).
    pub fn home_dir() -> PathBuf {
        std::env::var_os("ARCH_HOME")
            .map(PathBuf::from)
            .or_else(|| dirs::home_dir().map(|h| h.join(".arch-harness")))
            .unwrap_or_else(|| PathBuf::from(".arch-harness"))
    }

    /// Каталог конфигурации (`~/.config/arch-harness`).
    #[must_use]
    pub fn config_dir() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("arch-harness")
    }

    /// Предполёт агентного цикла: провайдер должен быть ЯВНО работоспособен
    /// (усиление UX-6 0.4.9 до уровня запуска, ADR-083 п.2). Без настроенного
    /// провайдера цикл не стартует молча с дефолтом `DeepSeek` — возвращается
    /// отказ с перечнем вариантов конфигурации.
    ///
    /// «Работоспособен» = запись есть в `[models]` и либо `kind = "cli"`
    /// (внешний CLI-харнесс, собственный ключ Spine не нужен), либо доступен
    /// API-ключ ([`ModelConfig::api_key_available`]).
    ///
    /// # Errors
    /// `[models]` пуст; модель `requested`/`default_model` не настроена; у
    /// провайдера нет ни `kind = "cli"`, ни доступного API-ключа.
    pub fn ensure_agent_provider(&self, requested: Option<&str>) -> Result<()> {
        let name = requested.unwrap_or(&self.default_model);
        if self.models.is_empty() {
            return Err(HarnessError::Config(
                "агентный цикл без провайдера: в [models] нет ни одной записи. \
                 Варианты: 1) добавьте [models.<имя>] в config.toml \
                 (см. README «API keys»); 2) задайте kind = \"cli\" — внешний \
                 CLI-харнесс (Claude Code, Codex) как LLM, без API-ключа; \
                 3) проверьте путь к конфигу (--config)."
                    .into(),
            ));
        }
        let Some(mc) = self.models.get(name) else {
            let mut names: Vec<&str> = self.models.keys().map(String::as_str).collect();
            names.sort_unstable();
            return Err(HarnessError::Config(format!(
                "агентный цикл: модель '{name}' не настроена в [models]. \
                 Настроены: {}. Укажите --model <имя> или default_model в config.toml.",
                names.join(", ")
            )));
        };
        if mc.kind.as_deref() == Some("cli") || mc.api_key_available() {
            return Ok(());
        }
        Err(HarnessError::Config(format!(
            "агентный цикл: провайдер '{name}' не готов — нет API-ключа и kind ≠ \"cli\". \
             Варианты: 1) задайте переменную окружения '{}' или положите ключ в файл '{}' \
             (см. README «API keys»); 2) настройте kind = \"cli\" — внешний CLI-харнесс \
             (Claude Code, Codex) как LLM, без API-ключа.",
            mc.api_key_env,
            mc.api_key_file.as_deref().unwrap_or("(не задан)")
        )))
    }

    /// Включает профиль `reverse` (ADR-083 п.3) на этой конфигурации:
    /// восстанавливающий прогон по чужому репозиторию — только чтение кода,
    /// запись в выходной каталог, без веб-канала. Идемпотентно.
    ///
    /// Состав: `[web].enabled = false` (веб-инструменты исключаются из реестра
    /// сессии на уровне регистрации, см. [`crate::tools::full_registry`]);
    /// `[tools].fs_write_dir = out` (пишущие файловые инструменты — только
    /// внутрь `out/`, чтение исходников остаётся); `[plugins].reverse_fallback`
    /// (встроенный набор `reverse-discovery`/`recover-review` при пустой
    /// библиотеке, с предупреждением в журнал).
    pub fn apply_reverse_profile(&mut self) {
        self.web.enabled = false;
        self.tools.fs_write_dir = Some(PathBuf::from("out"));
        self.plugins.reverse_fallback = true;
    }

    /// Загружает конфиг: `--config` → `./arch-harness.toml` →
    /// `~/.config/arch-harness/config.toml` → дефолты.
    ///
    /// Явный путь (`--config`) — **fail-closed** (ADR-083 п.2): если файл не
    /// существует, не читается или не разбирается — ошибка с путём и
    /// причиной, БЕЗ отката в дефолты. Иначе опечатка в `--config` молча
    /// подменяла бы конфиг дефолтами. Неявная цепочка (`cli_path == None`)
    /// сохраняет прежнее поведение: первый существующий файл, иначе дефолты.
    ///
    /// # Errors
    /// `HarnessError::Config` с путём и причиной, если явный путь задан, но
    /// недоступен/не разбирается/содержит невалидные значения.
    pub fn load(cli_path: Option<&Path>) -> Result<Self> {
        if let Some(p) = cli_path {
            let text = std::fs::read_to_string(p).map_err(|e| {
                HarnessError::Config(format!(
                    "{}: не прочитать конфиг (--config): {e}",
                    p.display()
                ))
            })?;
            return Self::from_toml(p, &text);
        }
        Self::load_first_existing(vec![
            PathBuf::from("arch-harness.toml"),
            Self::config_dir().join("config.toml"),
        ])
    }

    /// Неявная цепочка: первый существующий файл, иначе дефолты (расширенные
    /// `~`). Отсутствие всех кандидатов — НЕ ошибка (обычный старт без
    /// конфига); явный путь обрабатывается в [`Config::load`] отдельно.
    fn load_first_existing(candidates: Vec<PathBuf>) -> Result<Self> {
        for path in candidates {
            if path.is_file() {
                let text =
                    std::fs::read_to_string(&path).map_err(|e| HarnessError::io(&path, e))?;
                return Self::from_toml(&path, &text);
            }
        }
        let mut cfg = Config::default();
        cfg.expand_tildes();
        Ok(cfg)
    }

    /// Разбирает текст конфига из файла `path` с fail-fast валидацией полей,
    /// которые serde не проверяет (например, `[bash] sandbox`). Ошибки несут
    /// путь к файлу-виновнику.
    fn from_toml(path: &Path, text: &str) -> Result<Self> {
        let mut cfg: Config = toml::from_str(text).map_err(|e| {
            HarnessError::Config(format!("{}: не разобрать конфиг: {e}", path.display()))
        })?;
        cfg.bash.sandbox_mode().map_err(|e| {
            // Внутренняя ошибка уже помечена `config:`; не дублируем префикс,
            // но добавляем путь к файлу-виновнику.
            let msg = match e {
                HarnessError::Config(m) => m,
                other => other.to_string(),
            };
            HarnessError::Config(format!("{}: {msg}", path.display()))
        })?;
        cfg.expand_tildes();
        cfg.loaded_from = Some(path.to_path_buf());
        Ok(cfg)
    }

    /// Сохраняет конфиг в `~/.config/arch-harness/config.toml`.
    ///
    /// # Errors
    /// Ошибка создания каталога или записи файла.
    pub fn save_default(&self) -> Result<PathBuf> {
        let path = Self::config_dir().join("config.toml");
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| HarnessError::io(parent, e))?;
        }
        let text = toml::to_string_pretty(self)
            .map_err(|e| HarnessError::Config(format!("сериализация конфига: {e}")))?;
        std::fs::write(&path, text).map_err(|e| HarnessError::io(&path, e))?;
        Ok(path)
    }

    /// Подставляет `~` в начале путей (toml не раскрывает тильду).
    fn expand_tildes(&mut self) {
        let expand = |p: &mut PathBuf| {
            if let Ok(s) = p.clone().into_os_string().into_string() {
                if let Some(rest) = s.strip_prefix("~/") {
                    if let Some(home) = dirs::home_dir() {
                        *p = home.join(rest);
                    }
                }
            }
        };
        for d in &mut self.knowledge.dirs {
            expand(d);
        }
        expand(&mut self.mcp.servers_file);
        for d in &mut self.plugins.dirs {
            expand(d);
        }
        for d in &mut self.tools.fs_allow {
            expand(d);
        }
        expand(&mut self.cron.file);
        if let Some(d) = &mut self.cron.out_dir {
            expand(d);
        }
        expand(&mut self.paths.assets_dir);
        expand(&mut self.paths.reports_dir);
        expand(&mut self.paths.sessions_dir);
        expand(&mut self.paths.state_dir);
        expand(&mut self.paths.memory_file);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_has_three_providers() {
        let cfg = Config::default();
        for name in ["deepseek", "kimi", "glm"] {
            assert!(cfg.models.contains_key(name), "нет модели {name}");
        }
        assert!(cfg.harnesses.contains_key("claude-code"));
        assert!(cfg.harnesses.contains_key("codewhale"));
        assert!(cfg.harnesses.contains_key("kimi-code"));
        assert!(cfg.web.arch_sites.len() >= 8);
    }

    #[test]
    fn default_adapters_carry_validated_flags() {
        // Дефолты валидированы живыми прогонами флота (2026-08): неверный
        // флаг/режим давал argparse-код 2 (hermes: «unrecognized arguments: -p»).
        let cfg = Config::default();
        let h = &cfg.harnesses;
        let flags = |name: &str| (h[name].args.clone(), h[name].prompt_mode);
        assert_eq!(
            flags("hermes"),
            (
                vec!["-z".to_string(), "{prompt}".to_string()],
                PromptMode::Flag
            )
        );
        assert_eq!(
            flags("theseus"),
            (
                vec!["-p".to_string(), "{prompt}".to_string()],
                PromptMode::Flag
            )
        );
        assert_eq!(
            flags("codewhale"),
            (
                vec!["-p".to_string(), "{prompt}".to_string()],
                PromptMode::Flag
            )
        );
        // kimi-code: flag-режим `-p {prompt}`; permission-флагов нет осознанно —
        // `--yolo`/`--auto` несовместимы с `-p` (headless сам под auto-политикой).
        assert_eq!(
            flags("kimi-code"),
            (
                vec!["-p".to_string(), "{prompt}".to_string()],
                PromptMode::Flag
            )
        );
        assert!(!h["kimi-code"].args.iter().any(|a| a.contains("yolo")));
        assert_eq!(
            flags("openclaw"),
            (
                vec![
                    "agent".to_string(),
                    "--agent".to_string(),
                    "main".to_string(),
                    "--message".to_string(),
                    "{prompt}".to_string()
                ],
                PromptMode::Flag
            )
        );
    }

    #[test]
    fn load_remembers_config_path_for_hot_reload() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("config.toml");
        std::fs::write(&path, "default_model = \"glm\"\n").expect("write");
        let cfg = Config::load(Some(&path)).expect("load");
        assert_eq!(cfg.loaded_from.as_deref(), Some(path.as_path()));
        assert_eq!(cfg.default_model, "glm");
        // Снапшот дефолтов без файла — loaded_from пуст (горячего источника нет).
        assert!(Config::default().loaded_from.is_none());
        // loaded_from не утекает в сериализацию конфига.
        let text = toml::to_string_pretty(&cfg).expect("serialize");
        assert!(!text.contains("loaded_from"), "{text}");
    }

    #[test]
    fn flagship_models_default_to_1m_context() {
        let cfg = Config::default();
        for name in ["deepseek", "deepseek-pro", "kimi", "glm"] {
            let limit = cfg.models[name].context_limit.expect("context_limit задан");
            assert_eq!(limit, 1_000_000, "окно {name}");
        }
        // Бюджетные GLM остаются на своих окнах.
        assert_eq!(cfg.models["glm-air"].context_limit, Some(131_072));
    }

    #[test]
    fn kimi_default_starts_with_roomy_max_tokens() {
        // K3 ризонит всегда: цепочка рассуждений съедает бюджет до начала
        // видимого ответа — дефолтные 8192 обрывали генерацию (кейс
        // 2026-10-14). Старт с 32K + автоэскалация при length.
        let cfg = Config::default();
        assert_eq!(cfg.models["kimi"].max_tokens, Some(32768));
    }

    /// glm-5.3-flash в дефолтах: окно 1.3M; «off» ризонинга эмулируется
    /// enabled+low (провайдер не даёт отключить thinking — HTTP 1210).
    #[test]
    fn glm53_flash_defaults_with_emulated_thinking_off() {
        let cfg = Config::default();
        let m = &cfg.models["glm-5.3-flash"];
        assert_eq!(m.model, "glm-5.3-flash");
        assert_eq!(m.context_limit, Some(1_310_720));
        assert_eq!(m.api_key_env, "ZHIPU_API_KEY");
        let on = m.thinking_on.as_ref().expect("thinking_on задан");
        assert_eq!(on["thinking"]["type"], "enabled");
        let off = m.thinking_off.as_ref().expect("thinking_off задан");
        assert_eq!(off["thinking"]["type"], "enabled");
        assert_eq!(off["reasoning_effort"], "low");
    }

    #[test]
    fn config_roundtrips_through_toml() {
        let cfg = Config::default();
        let text = toml::to_string_pretty(&cfg).expect("serialize");
        let back: Config = toml::from_str(&text).expect("deserialize");
        assert_eq!(back.default_model, cfg.default_model);
        assert_eq!(back.models.len(), cfg.models.len());
    }

    /// Периметр файловых инструментов в дефолтах (ADR-083 п.1): песочница
    /// включена, allowlist — литерал `~/.arch-harness` (init сериализует его
    /// как есть; `expand_tildes` раскрывает при загрузке).
    #[test]
    fn tools_default_sandbox_and_allowlist() {
        let cfg = Config::default();
        assert!(cfg.tools.fs_sandbox, "песочница включена по умолчанию");
        assert_eq!(cfg.tools.fs_allow, vec![PathBuf::from("~/.arch-harness")]);
        // init-файл несёт литерал (портабельно), а не путь конкретной машины.
        let text = toml::to_string_pretty(&cfg).expect("serialize");
        assert!(text.contains("[tools]"), "{text}");
        assert!(text.contains("~/.arch-harness"), "{text}");
        // Загрузка раскрывает `~` в абсолютный путь (если дом известен).
        let mut loaded = Config::default();
        loaded.expand_tildes();
        if dirs::home_dir().is_some() {
            assert!(
                loaded.tools.fs_allow[0].is_absolute(),
                "expand_tildes раскрывает fs_allow: {:?}",
                loaded.tools.fs_allow
            );
        }
    }

    #[test]
    fn model_config_parses_optional_proxy() {
        // Поле задано — URL прокси попадает в конфиг провайдера.
        let mc: ModelConfig = toml::from_str(
            "base_url = \"https://openrouter.ai/api/v1\"\n\
             model = \"stealth/ox-alpha\"\n\
             api_key_env = \"OPENROUTER_API_KEY\"\n\
             proxy = \"http://127.0.0.1:12080\"\n",
        )
        .expect("deserialize");
        assert_eq!(mc.proxy.as_deref(), Some("http://127.0.0.1:12080"));
        // Без поля — None (serde default): прямое соединение, как раньше.
        let plain: ModelConfig = toml::from_str(
            "base_url = \"https://api.deepseek.com/v1\"\n\
             model = \"deepseek-v4-flash\"\n\
             api_key_env = \"DEEPSEEK_API_KEY\"\n",
        )
        .expect("deserialize");
        assert!(plain.proxy.is_none());
        assert!(ModelConfig::default().proxy.is_none());
    }

    #[test]
    fn old_style_model_config_without_new_fields_still_parses() {
        // Обратная совместимость (ADR-021): старые конфиги без user_agent,
        // ca_pem_file, client_cert_file/client_key_file и oauth читаются
        // без изменений — новые поля optional, serde default.
        let text = "[models.legacy]\n\
            base_url = \"https://api.example.test/v1\"\n\
            model = \"legacy-model\"\n\
            api_key_env = \"LEGACY_KEY\"\n";
        let cfg: Config = toml::from_str(text).expect("parse");
        let mc = &cfg.models["legacy"];
        assert_eq!(mc.model, "legacy-model");
        assert_eq!(mc.timeout_secs, 180, "дефолты на месте");
        assert!(mc.user_agent.is_none());
        assert!(mc.ca_pem_file.is_none());
        assert!(mc.client_cert_file.is_none());
        assert!(mc.client_key_file.is_none());
        assert!(mc.oauth.is_none());
        assert!(mc.proxy.is_none());
    }

    #[test]
    fn model_config_cli_fields_parse_and_default() {
        // kind = "cli": провайдер — внешний CLI-агент, свои ключи не нужны.
        let mc: ModelConfig = toml::from_str(
            "kind = \"cli\"\n\
             command = \"claude\"\n\
             args = [\"-p\", \"--output-format\", \"json\"]\n\
             timeout_secs = 240\n",
        )
        .expect("deserialize");
        assert_eq!(mc.kind.as_deref(), Some("cli"));
        assert_eq!(mc.command.as_deref(), Some("claude"));
        assert_eq!(mc.args, vec!["-p", "--output-format", "json"]);
        assert_eq!(mc.timeout_secs, 240);
        // Старые конфиги без новых полей: None/пусто — поведение не меняется.
        let plain: ModelConfig = toml::from_str(
            "base_url = \"https://api.deepseek.com/v1\"\n\
             model = \"deepseek-flash\"\n\
             api_key_env = \"DEEPSEEK_API_KEY\"\n",
        )
        .expect("deserialize");
        assert!(plain.kind.is_none());
        assert!(plain.command.is_none());
        assert_eq!(plain.args, [] as [std::string::String; 0]);
        assert!(ModelConfig::default().kind.is_none());
    }

    #[test]
    fn oauth_config_parses_with_vendor_default_token_url() {
        // oauth-блок с пустым/отсутствующим token_url валиден: дефолт URL
        // профиля задаёт фабрика вендора (GigaChat), не конфиг.
        let text = "[models.gigachat]\n\
            model = \"GigaChat-2-Pro\"\n\
            api_key_env = \"GIGACHAT_KEY\"\n\
            [models.gigachat.oauth]\n\
            scope = \"GIGACHAT_API_PERS\"\n";
        let cfg: Config = toml::from_str(text).expect("parse");
        let oauth = cfg.models["gigachat"].oauth.as_ref().expect("oauth");
        assert_eq!(oauth.scope, "GIGACHAT_API_PERS");
        assert!(oauth.token_url.is_empty(), "дефолт задаст фабрика");
    }

    #[test]
    fn agent_failure_memory_mode_defaults_and_parses() {
        // Дефолт — propose: урок предлагается, AGENTS.md не трогается.
        let cfg = Config::default();
        assert_eq!(cfg.agent.failure_memory, FailureMemoryMode::Propose);
        // Разбор всех режимов из toml + круговой обход сериализации.
        for (text, want) in [
            ("off", FailureMemoryMode::Off),
            ("propose", FailureMemoryMode::Propose),
            ("write", FailureMemoryMode::Write),
        ] {
            let cfg: Config = toml::from_str(&format!("[agent]\nfailure_memory = \"{text}\"\n"))
                .expect("deserialize");
            assert_eq!(cfg.agent.failure_memory, want, "режим {text}");
            assert_eq!(cfg.agent.failure_memory.as_str(), text);
        }
        // Неизвестный режим — ошибка разбора, не молчаливый дефолт.
        assert!(toml::from_str::<Config>("[agent]\nfailure_memory = \"yolo\"\n").is_err());
        // state_dir — в домашнем каталоге харнесса.
        assert!(cfg.paths.state_dir.ends_with("state"));
    }

    #[test]
    fn judge_config_defaults_match_adr004() {
        let cfg = Config::default();
        assert_eq!(cfg.judge.samples, 3, "k сэмплов судьи по умолчанию");
        assert!((cfg.judge.unstable_stdev - 1.0).abs() < 1e-9);
        assert!((cfg.judge.evidence_min_similarity - 0.8).abs() < 1e-9);
        assert!((cfg.judge.golden_max_mae - 1.0).abs() < 1e-9);
        assert!(
            !cfg.judge.cache,
            "E8.2 кэш вердикта — opt-in: дефолт каждый раз спрашивает модель"
        );
        assert!(
            !cfg.judge.adaptive_samples,
            "E8.1 адаптивные сэмплы — opt-in: дефолт спрашивает судью `samples` раз"
        );
        // Пустая секция [judge] в toml даёт те же дефолты (serde default).
        let back: Config = toml::from_str("[judge]\n").expect("deserialize");
        assert_eq!(back.judge.samples, 3);
    }

    #[test]
    fn fleet_config_defaults_and_parse() {
        // Дефолты: enforcement выключен (поведение прежних версий), гейт — owner.
        let cfg = Config::default();
        assert!(!cfg.fleet.require_worktree);
        assert_eq!(cfg.fleet.merge_gate, "owner");
        // Секция отсутствует в toml — serde default даёт те же значения.
        let bare: Config = toml::from_str("").expect("deserialize empty");
        assert!(!bare.fleet.require_worktree);
        assert_eq!(bare.fleet.merge_gate, "owner");
        // Явная секция парсится.
        let on: Config =
            toml::from_str("[fleet]\nrequire_worktree = true\nmerge_gate = \"none\"\n")
                .expect("deserialize");
        assert!(on.fleet.require_worktree);
        assert_eq!(on.fleet.merge_gate, "none");
        // Round-trip через сериализацию.
        let text = toml::to_string_pretty(&on).expect("serialize");
        let back: Config = toml::from_str(&text).expect("deserialize");
        assert!(back.fleet.require_worktree);
        assert_eq!(back.fleet.merge_gate, "none");
    }

    #[test]
    fn web_defaults_to_enabled_when_key_missing() {
        // Обратная совместимость: без ключа enabled веб-канал включён, как раньше.
        let back: Config =
            toml::from_str("[web]\nsearch_base = \"http://127.0.0.1:9/\"\n").expect("deserialize");
        assert!(back.web.enabled);
        assert_eq!(back.web.search_base, "http://127.0.0.1:9/");
        // Полностью пустая секция [web] — тоже дефолт true (serde default).
        let empty: Config = toml::from_str("[web]\n").expect("deserialize");
        assert!(empty.web.enabled);
    }

    #[test]
    fn web_enabled_false_parses() {
        let back: Config = toml::from_str("[web]\nenabled = false\n").expect("deserialize");
        assert!(
            !back.web.enabled,
            "флаг [web].enabled=false должен читаться"
        );
    }

    #[test]
    fn significance_config_defaults_parse_and_validate() {
        // Дефолты 1/4 — историческая эвристика SOURCE_BRIEF (ADR-034):
        // обратная совместимость с поведением до введения секции.
        let cfg = Config::default();
        assert_eq!(cfg.significance.limits().expect("дефолты валидны"), (1, 4));
        // Секция отсутствует в toml — serde default даёт те же значения.
        let bare: Config = toml::from_str("").expect("deserialize empty");
        assert_eq!(bare.significance.limits().expect("валидны"), (1, 4));
        // Кастомные пороги парсятся и валидируются.
        let custom: Config =
            toml::from_str("[significance]\nfast_max = 2\nstandard_max = 6\n").expect("parse");
        assert_eq!(custom.significance.limits().expect("валидны"), (2, 6));
        // fast_max >= standard_max — мягкая ошибка при чтении (не при загрузке).
        let bad: Config =
            toml::from_str("[significance]\nfast_max = 4\nstandard_max = 4\n").expect("parse");
        let err = bad.significance.limits().expect_err("границы совпали");
        assert!(err.to_string().contains("fast_max"), "{err}");
    }

    /// D2: глобы слепой зоны (`security`/`trust_zone`/`data_contract`) по
    /// умолчанию ПУСТЫ (поведение прежнее, глобальных эвристик нет), а из
    /// `[significance]` конфига кейса доезжают до `DiffGlobs` детектора.
    #[test]
    fn significance_case_globs_default_empty_and_parse() {
        let bare: Config = toml::from_str("").expect("deserialize empty");
        let globs = bare.significance.diff_globs();
        assert!(globs.security.is_empty(), "дефолт пуст: {globs:?}");
        assert!(globs.trust_zone.is_empty(), "дефолт пуст: {globs:?}");
        assert!(globs.data_contract.is_empty(), "дефолт пуст: {globs:?}");

        let custom: Config = toml::from_str(
            "[significance]\nsecurity_globs = [\"auth/**\"]\ntrust_zone_globs = [\"deploy/mesh/**\"]\ndata_contract_globs = [\"schemas/events/**\"]\n",
        )
        .expect("parse");
        let globs = custom.significance.diff_globs();
        assert_eq!(globs.security, vec!["auth/**".to_string()]);
        assert_eq!(globs.trust_zone, vec!["deploy/mesh/**".to_string()]);
        assert_eq!(globs.data_contract, vec!["schemas/events/**".to_string()]);
        // Остальные глобы — дефолтные (T-05 не сломан).
        assert_eq!(globs.contracts, default_contract_globs());
    }

    #[test]
    fn bash_sandbox_defaults_parse_and_validate() {
        // Дефолт — "none": поведение прежних версий (без изоляции).
        let cfg = Config::default();
        assert_eq!(
            cfg.bash.sandbox_mode().expect("дефолт валиден"),
            BashSandbox::None
        );
        // Секция отсутствует в toml — serde default даёт то же значение.
        let bare: Config = toml::from_str("").expect("deserialize empty");
        assert_eq!(bare.bash.sandbox, "none");
        // bwrap парсится.
        let bwrap: Config = toml::from_str("[bash]\nsandbox = \"bwrap\"\n").expect("deserialize");
        assert_eq!(
            bwrap.bash.sandbox_mode().expect("валиден"),
            BashSandbox::Bwrap
        );
        // Неизвестное значение — понятная ошибка конфига, не молчаливый дефолт.
        let bad: Config = toml::from_str("[bash]\nsandbox = \"firejail\"\n").expect("deserialize");
        let err = bad.bash.sandbox_mode().expect_err("неизвестное значение");
        let text = err.to_string();
        assert!(text.contains("firejail"), "{text}");
        assert!(text.contains("none") && text.contains("bwrap"), "{text}");
    }

    #[test]
    fn load_rejects_unknown_bash_sandbox() {
        // Ошибка всплывает при загрузке файла конфига (fail-fast), а не при
        // первом запуске команды.
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[bash]\nsandbox = \"nsjail\"\n").expect("write");
        let err = Config::load(Some(&path)).expect_err("загрузка должна упасть");
        assert!(err.to_string().contains("nsjail"), "{err}");
        // Путь файла-виновника присутствует (ADR-083 п.2).
        assert!(err.to_string().contains("config.toml"), "{err}");
    }

    /// ADR-083 п.2: явный `--config` — fail-closed. Отсутствие файла НЕ
    /// откатывается в дефолты (опечатка в пути должна падать, а не молча
    /// подменяться дефолтным конфигом).
    #[test]
    fn load_explicit_missing_path_is_fail_closed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("нет-такого-конфига.toml");
        let err = Config::load(Some(&path)).expect_err("нет файла → ошибка, не дефолт");
        let text = err.to_string();
        assert!(
            text.contains("нет-такого-конфига.toml"),
            "путь в ошибке: {text}"
        );
        assert!(text.contains("--config"), "речь о явном пути: {text}");
    }

    /// Разбираемый, но битый TOML по явному пути — ошибка с путём (а не
    /// «transparent»-ошибка serde без указания файла).
    #[test]
    fn load_explicit_broken_toml_names_path() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("битый.toml");
        std::fs::write(&path, "default_model = \n").expect("write");
        let err = Config::load(Some(&path)).expect_err("битый toml → ошибка");
        assert!(err.to_string().contains("битый.toml"), "{err}");
    }

    /// Каталог вместо файла по явному пути — тоже fail-closed.
    #[test]
    fn load_explicit_directory_is_fail_closed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let err = Config::load(Some(dir.path())).expect_err("каталог не файл → ошибка");
        assert!(err.to_string().contains("не прочитать"), "{err}");
    }

    /// Неявная цепочка без файлов-кандидатов → дефолты (не ошибка); прежнее
    /// поведение обычного старта без конфига сохранено.
    #[test]
    fn load_implicit_chain_defaults_when_no_files() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cfg = Config::load_first_existing(vec![
            dir.path().join("nope-1.toml"),
            dir.path().join("nope-2.toml"),
        ])
        .expect("дефолты без файлов");
        assert!(cfg.loaded_from.is_none(), "горячего источника нет");
        assert!(cfg.tools.fs_sandbox, "дефолты на месте");
    }

    /// `api_key_available`: env-ветку не трогаем (`set_var` unsafe в 2024),
    /// проверяем файловую ветку — непустой файл даёт `true`, пустой/пробельный
    /// и несуществующий — `false`.
    #[test]
    fn api_key_available_uses_env_or_file() {
        // Имя env уникально и не задаётся — ветка env гарантированно промах.
        let dir = tempfile::tempdir().expect("tempdir");

        // Файла нет → ключа нет.
        let mut mc = ModelConfig {
            api_key_env: "ARCH_HARNESS_TEST_KEY_ABSENT_UNIQUE".into(),
            api_key_file: Some(dir.path().join("нет-такого").display().to_string()),
            ..Default::default()
        };
        assert!(!mc.api_key_available(), "отсутствующий файл → нет ключа");

        // Пустой/пробельный файл → нет ключа (как и env после trim).
        let blank = dir.path().join("blank");
        std::fs::write(&blank, "   \n\t").expect("write");
        mc.api_key_file = Some(blank.display().to_string());
        assert!(!mc.api_key_available(), "пробельный файл → нет ключа");

        // Непустой файл → ключ есть.
        let key = dir.path().join("key");
        std::fs::write(&key, "секрет\n").expect("write");
        mc.api_key_file = Some(key.display().to_string());
        assert!(mc.api_key_available(), "непустой файл → ключ есть");

        // Файл не задан и env пуст → нет ключа.
        mc.api_key_file = None;
        assert!(!mc.api_key_available(), "ни env, ни файла → нет ключа");
    }

    /// ADR-083 п.2: предполёт провайдера агентного цикла — цикл не стартует
    /// молча с дефолтом. `kind = "cli"` и доступный ключ проходят; иначе —
    /// отказ с вариантами; пустой `[models]` и неизвестное имя — свои отказы.
    #[test]
    fn ensure_agent_provider_fail_closed_variants() {
        let dir = tempfile::tempdir().expect("tempdir");

        let mut cfg = Config::default();

        // 1) kind="cli" — проходит без API-ключа (внешний CLI-харнесс).
        cfg.models.insert(
            "cli-модель".into(),
            ModelConfig {
                kind: Some("cli".into()),
                ..Default::default()
            },
        );
        cfg.ensure_agent_provider(Some("cli-модель"))
            .expect("kind=cli без ключа ок");

        // 2) ключ через api_key_file — проходит.
        let key = dir.path().join("kimi.key");
        std::fs::write(&key, "секрет\n").expect("write");
        cfg.models.insert(
            "с-ключом".into(),
            ModelConfig {
                api_key_env: "ARCH_HARNESS_TEST_KEY_FILE_UNIQUE".into(),
                api_key_file: Some(key.display().to_string()),
                ..Default::default()
            },
        );
        cfg.ensure_agent_provider(Some("с-ключом"))
            .expect("ключ из файла ок");

        // 3) ни ключа, ни cli → отказ с перечнем вариантов.
        cfg.models.insert(
            "без-ключа".into(),
            ModelConfig {
                api_key_env: "ARCH_HARNESS_TEST_KEY_MISSING_UNIQUE".into(),
                ..Default::default()
            },
        );
        let err = cfg
            .ensure_agent_provider(Some("без-ключа"))
            .expect_err("без ключа → отказ");
        let text = err.to_string();
        assert!(text.contains("агентный цикл"), "{text}");
        assert!(
            text.contains("ARCH_HARNESS_TEST_KEY_MISSING_UNIQUE"),
            "{text}"
        );
        assert!(
            text.contains("kind = \"cli\""),
            "вариант cli предложен: {text}"
        );

        // 4) неизвестная модель → перечень настроенных имён.
        let err = cfg
            .ensure_agent_provider(Some("нет-такой"))
            .expect_err("нет модели → отказ");
        let text = err.to_string();
        assert!(text.contains("нет-такой"), "{text}");
        assert!(text.contains("cli-модель"), "перечень настроенных: {text}");

        // 5) пустой [models] → отказ с вариантами.
        let empty = Config {
            models: BTreeMap::new(),
            ..Config::default()
        };
        let err = empty
            .ensure_agent_provider(None)
            .expect_err("нет моделей → отказ");
        assert!(err.to_string().contains("[models]"), "{err}");
    }

    #[test]
    fn evals_dir_follows_reports_dir_root() {
        // Дефолт: ~/.arch-harness/evals (корень — родитель reports_dir).
        let cfg = Config::default();
        assert_eq!(
            cfg.paths.evals_dir(),
            Config::home_dir().join("evals"),
            "evals рядом с корнем данных"
        );
        // Переопределение reports_dir тащит evals в тот же корень
        // (детерминизм тестов и пользовательских макетов).
        let mut cfg = Config::default();
        cfg.paths.reports_dir = PathBuf::from("/tmp/x/reports");
        assert_eq!(cfg.paths.evals_dir(), PathBuf::from("/tmp/x/evals"));
    }

    /// C1 (ADR-057): секция `[harnesses.<имя>.acp]` парсится, таймауты — дефолтные.
    #[test]
    fn acp_section_is_parsed() {
        let c: CodingHarnessConfig = toml::from_str(
            "binary = 'claude-code-acp'\nprompt_mode = 'stdin'\n\
             [acp]\nbinary = 'claude-code-acp'\nargs = ['--acp']\n",
        )
        .expect("CONFIG");
        assert_eq!(c.mode, AcpMode::Auto, "дефолт режима — auto");
        let acp = c.acp.expect("секция acp");
        assert_eq!(acp.binary, "claude-code-acp");
        assert_eq!(acp.args, vec!["--acp".to_string()]);
        assert_eq!(acp.init_timeout_secs, 30, "дефолт init_timeout_secs");
        assert_eq!(acp.cancel_grace_secs, 10, "дефолт cancel_grace_secs");

        // Явные значения перекрывают дефолты.
        let c: CodingHarnessConfig = toml::from_str(
            "binary = 'x'\nmode = 'acp'\n\
             [acp]\nbinary = 'x'\ninit_timeout_secs = 5\ncancel_grace_secs = 2\n",
        )
        .expect("CONFIG");
        assert_eq!(c.mode, AcpMode::Acp);
        let acp = c.acp.expect("секция acp");
        assert_eq!(acp.init_timeout_secs, 5);
        assert_eq!(acp.cancel_grace_secs, 2);
    }

    /// C1: старый конфиг без `mode`/`acp` работает как headless (аддитивность).
    #[test]
    fn legacy_harness_config_without_acp_still_loads() {
        let c: CodingHarnessConfig =
            toml::from_str("binary = 'claude'\nargs = ['-p']\nprompt_mode = 'stdin'\n")
                .expect("CONFIG");
        assert_eq!(c.mode, AcpMode::Auto, "старый конфиг читается как auto");
        assert!(c.acp.is_none(), "ACP не задекларирован → headless");
        assert_eq!(c.binary, "claude");
        assert_eq!(c.prompt_mode, PromptMode::Stdin);
    }

    /// C1: `mode` принимает auto|acp|prompt; неизвестное значение — ошибка.
    #[test]
    fn acp_mode_parses_three_values() {
        let mode = |m: &str| -> AcpMode {
            toml::from_str::<CodingHarnessConfig>(&format!("binary = 'x'\nmode = '{m}'\n"))
                .expect("CONFIG")
                .mode
        };
        assert_eq!(mode("auto"), AcpMode::Auto);
        assert_eq!(mode("acp"), AcpMode::Acp);
        assert_eq!(mode("prompt"), AcpMode::Prompt);
        assert!(
            toml::from_str::<CodingHarnessConfig>("binary = 'x'\nmode = 'yolo'\n").is_err(),
            "неизвестный режим обязан падать, а не молча дефолтиться"
        );
    }

    /// ADR-068 Am.2: `[gate.arch_drift]` принимает `max_files` и `ignore`;
    /// пустая секция — прежние дефолты (лимит подставляет сканер, ignore пуст).
    #[test]
    fn gate_arch_drift_max_files_and_ignore_parse() {
        let c: Config = toml::from_str(
            "[gate.arch_drift]\nenabled = true\nmax_files = 5000\nignore = [\"env/\", \"vendor/\"]\n",
        )
        .expect("parse");
        assert!(c.gate.arch_drift.enabled);
        assert_eq!(c.gate.arch_drift.max_files, Some(5000));
        assert_eq!(
            c.gate.arch_drift.ignore,
            vec!["env/".to_string(), "vendor/".to_string()]
        );
        let bare: Config = toml::from_str("[gate.arch_drift]\n").expect("parse");
        assert_eq!(bare.gate.arch_drift.max_files, None);
        assert!(
            bare.gate.arch_drift.ignore.is_empty(),
            "ignore по умолчанию пуст"
        );
    }
}
