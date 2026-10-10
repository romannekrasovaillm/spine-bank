//! Конфигурация харнесса (`config.toml`).
//!
//! Порядок поиска конфига: `--config <path>` → `./arch-harness.toml` →
//! `~/.config/arch-harness/config.toml` → встроенные дефолты.
//! Команда `arch-be init` пишет дефолтный конфиг и ассеты в [`Config::home_dir`].

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{HarnessError, Result};

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

/// Конфигурация одного LLM-провайдера (OpenAI-совместимый API).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ModelConfig {
    /// Базовый URL API, напр. `https://api.deepseek.com/v1`.
    pub base_url: String,
    /// Идентификатор модели, напр. `deepseek-flash`.
    pub model: String,
    /// Род провайдера: None — обычный OpenAI-совместимый endpoint (поведение
    /// прежних версий); Some("cli") — внешний CLI-агент как LLM
    /// (см. [`crate::llm::harness_cli`]): модель вызывается через УЖЕ
    /// авторизованный на машине пользователя CLI-харнесс (Claude Code, Codex,
    /// Qwen Code), собственный API-ключ Spine не нужен — платит подписка
    /// хоста. Неизвестные значения трактуются как обычный провайдер.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// Команда CLI-харнесса (имя/путь бинаря) — только для `kind = "cli"`.
    /// Промпт передаётся процессу через stdin, ответ читается из stdout.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// Аргументы командной строки CLI-харнесса, подставляются как есть —
    /// только для `kind = "cli"` (напр. `["-p", "--output-format", "json"]`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    /// Имя переменной окружения с API-ключом.
    pub api_key_env: String,
    /// Запасной путь к файлу с ключом (`~` раскрывается): читается, если
    /// переменная окружения не задана (кейс theseus: ключ Kimi лежит в
    /// `~/.kimi_api_key`, а процесс харнесса env не видит). Содержимое
    /// никуда не выводится — только путь в тексте ошибки.
    #[serde(default)]
    pub api_key_file: Option<String>,
    /// URL прокси для этого провайдера, напр. `http://127.0.0.1:12080`.
    /// Для loopback-прокси (локальный egress-шлюз) харнесс при выборе модели
    /// проверяет порт и автоматически поднимает шлюз
    /// (`systemctl --user start vpn-egress`, см. [`crate::net`]); внешний
    /// прокси используется как есть. None — прямое соединение (или прокси
    /// из env `HTTP(S)_PROXY`, как раньше).
    #[serde(default)]
    pub proxy: Option<String>,
    /// Максимум токенов ответа.
    pub max_tokens: Option<u32>,
    /// Температура сэмплирования.
    pub temperature: Option<f32>,
    /// Бюджет тишины (сек): ожидание заголовков и пауза между чанками стрима.
    pub timeout_secs: u64,
    /// Окно контекста модели в токенах (если задано): автоматическая
    /// компактификация работает от `min(agent.context_budget_tokens`, этого
    /// окна) — пороги `compact_l1_pct`/`compact_l3_pct` (70%/95%) привязаны
    /// к реальному пределу API, а не к статичному бюджету.
    pub context_limit: Option<usize>,
    /// JSON-объект, сливаемый в тело запроса при включённом ризонинге
    /// (`/think on`): напр. `{"thinking": {"type": "enabled"}}` (`DeepSeek` V4,
    /// GLM-4.x) или `{"reasoning_effort": "max"}` (Kimi K3).
    /// None — переключение ризонинга для модели не настроено.
    pub thinking_on: Option<serde_json::Map<String, serde_json::Value>>,
    /// То же при `/think off`: `{"thinking": {"type": "disabled"}}` или
    /// `{"reasoning_effort": "low"}`.
    pub thinking_off: Option<serde_json::Map<String, serde_json::Value>>,
    /// Значение заголовка `User-Agent` для запросов этого провайдера.
    /// None — дефолт вендора (тонкая фабрика `GigaChat` ставит
    /// `spine-arch/<версия>`; без него API `GigaChat` отвечает 403).
    #[serde(default)]
    pub user_agent: Option<String>,
    /// Путь к PEM-файлу с корневым CA (напр. НУЦ Минцифры для GigaChat):
    /// корень добавляется в root store rustls-клиента. None — системные корни.
    #[serde(default)]
    pub ca_pem_file: Option<String>,
    /// Путь к PEM-файлу с клиентским сертификатом (mTLS-профиль `B2Bank`).
    /// Задаётся парой с `client_key_file`.
    #[serde(default)]
    pub client_cert_file: Option<String>,
    /// Путь к PEM-файлу с ключом клиентского сертификата (mTLS-профиль `B2Bank`).
    #[serde(default)]
    pub client_key_file: Option<String>,
    /// Параметры OAuth2-авторизации (вендоры с токеном вместо статического
    /// ключа — GigaChat): токен запрашивается с Basic-ключом из
    /// `api_key_env`/`api_key_file` и уходит как `Bearer` на вызовы API.
    /// None — OAuth не используется (mTLS-профиль `B2Bank` или статический ключ).
    #[serde(default)]
    pub oauth: Option<OAuthConfig>,
    /// Тариф: цена 1M входных (prompt) токенов в валюте пользователя
    /// (рубли, доллары — что удобно владельцу конфига; единица не
    /// фиксируется харнессом). None — тариф не задан: отчёты стоимости
    /// (`arch-be metrics --cost-report`) показывают только токены.
    #[serde(default)]
    pub price_in_per_1m: Option<f64>,
    /// Тариф: цена 1M выходных (completion) токенов (см. `price_in_per_1m`).
    #[serde(default)]
    pub price_out_per_1m: Option<f64>,
}

impl Default for ModelConfig {
    fn default() -> Self {
        Self {
            base_url: String::new(),
            model: String::new(),
            kind: None,
            command: None,
            args: Vec::new(),
            api_key_env: String::new(),
            api_key_file: None,
            proxy: None,
            max_tokens: Some(8192),
            temperature: None,
            timeout_secs: 180,
            context_limit: None,
            thinking_on: None,
            thinking_off: None,
            user_agent: None,
            ca_pem_file: None,
            client_cert_file: None,
            client_key_file: None,
            oauth: None,
            price_in_per_1m: None,
            price_out_per_1m: None,
        }
    }
}

/// Параметры OAuth2-авторизации вендора (см. [`ModelConfig::oauth`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OAuthConfig {
    /// URL эндпоинта токена (`POST`, form-urlencoded `scope=…`).
    /// Пустая строка — дефолт профиля вендора (фабрика `GigaChat`
    /// подставляет `https://ngw.devices.sberbank.ru:9443/api/v2/oauth`),
    /// в конфиге дефолт не задаётся.
    #[serde(default)]
    pub token_url: String,
    /// Запрашиваемый scope: для `GigaChat` — `GIGACHAT_API_PERS` (физлица),
    /// `GIGACHAT_API_B2B`/`GIGACHAT_API_CORP` (юрлица) — выбор владельца
    /// инсталляции (от scope зависят лимиты одновременных потоков).
    pub scope: String,
}

impl ModelConfig {
    /// Читает API-ключ из переменной окружения (содержимое не логируется).
    ///
    /// # Errors
    /// Переменная окружения `api_key_env` не установлена.
    pub fn api_key(&self) -> Result<String> {
        std::env::var(&self.api_key_env).map_err(|_| {
            HarnessError::Config(format!(
                "переменная окружения {} не установлена (нужен API-ключ для {})",
                self.api_key_env, self.base_url
            ))
        })
    }
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
}

impl Default for PluginsConfig {
    fn default() -> Self {
        Self {
            // По умолчанию — только библиотека харнесса; свои каталоги
            // плагинов добавляются в config.toml.
            dirs: vec![Config::home_dir().join("plugins")],
            include_mcp: true,
            include_hooks: true,
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

/// Пороги маршрутизации Architecture Significance Score (ADR-034).
///
/// Дефолты 1/4 воспроизводят историческую эвристику «0–1 → Fast, 2–4 →
/// Standard, 5+ → Critical» из `SOURCE_BRIEF` (статус — рабочая гипотеза,
/// пересмотр по данным пилота). Форсирующие critical-триггеры
/// (`security_boundary_change`, `irreversible_migration`,
/// `criticality_or_exception`) НЕ конфигурируются — fail-safe.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SignificanceConfig {
    /// Верхняя граница маршрута Fast: score ≤ `fast_max` → Fast.
    pub fast_max: usize,
    /// Верхняя граница маршрута Standard: `fast_max` < score ≤ `standard_max` →
    /// Standard; выше — Critical.
    pub standard_max: usize,
    /// Глобы детектора контрактов (T-05): что считать контрактом по пути.
    #[serde(default = "default_contract_globs")]
    pub contract_globs: Vec<String>,
    /// Глобы детектора новых сущностей модели (T-05): файлы, появление
    /// которых означает новый компонент.
    #[serde(default = "default_component_globs")]
    pub component_globs: Vec<String>,
    /// Глобы детектора изменений интеграций (T-05): файлы сущностей
    /// интеграций модели.
    #[serde(default = "default_integration_globs")]
    pub integration_globs: Vec<String>,
    /// Глобы детектора NFR-сущностей (1.7 п.3): файлы NFR модели, появление
    /// или правка которых поднимает `significant_nfr`.
    #[serde(default = "default_nfr_globs")]
    pub nfr_globs: Vec<String>,
    /// Глобы границы безопасности (D2): auth-конфиги, сетевые политики, IAM —
    /// правка файла под глобом зажигает `security_boundary_change`.
    /// ПУСТ по умолчанию: глобальных эвристик нет, соглашения репозитория не
    /// зашиты в бинарь; без настройки триггер остаётся заявляемым (D1).
    #[serde(default)]
    pub security_globs: Vec<String>,
    /// Глобы зон доверия (D2): правка файла под глобом зажигает
    /// `trust_zone_change`. Пусто по умолчанию, как у `security_globs`.
    #[serde(default)]
    pub trust_zone_globs: Vec<String>,
    /// Глобы контрактов данных (D2): схемы событий/топиков — правка файла
    /// под глобом зажигает `data_contract_change`. Пусто по умолчанию.
    #[serde(default)]
    pub data_contract_globs: Vec<String>,
}

/// Дефолтные глобы контрактов (T-05): каталоги, где контракты лежат по
/// соглашению, — в дополнение к распознаванию по СОДЕРЖИМОМУ файла.
fn default_contract_globs() -> Vec<String> {
    vec!["docs/contracts/**".to_string(), "contracts/**".to_string()]
}

/// Дефолтный глоб новых компонентов модели (T-05).
fn default_component_globs() -> Vec<String> {
    vec!["model/CMP-*".to_string()]
}

/// Дефолтный глоб сущностей интеграций модели (T-05).
fn default_integration_globs() -> Vec<String> {
    vec!["model/INT-*".to_string()]
}

/// Дефолтный глоб NFR-сущностей модели (1.7 п.3).
fn default_nfr_globs() -> Vec<String> {
    vec!["model/NFR-*".to_string()]
}

impl Default for SignificanceConfig {
    fn default() -> Self {
        Self {
            fast_max: crate::control::DEFAULT_FAST_MAX,
            standard_max: crate::control::DEFAULT_STANDARD_MAX,
            contract_globs: default_contract_globs(),
            component_globs: default_component_globs(),
            integration_globs: default_integration_globs(),
            nfr_globs: default_nfr_globs(),
            security_globs: Vec::new(),
            trust_zone_globs: Vec::new(),
            data_contract_globs: Vec::new(),
        }
    }
}

impl SignificanceConfig {
    /// Валидированные пороги `(fast_max, standard_max)`.
    ///
    /// Валидация мягкая — не при загрузке конфига, а при чтении в
    /// `control score`/`significance_score`: `fast_max` обязан быть строго
    /// меньше `standard_max`, иначе граница Fast/Standard неопределённа.
    ///
    /// # Errors
    /// `fast_max >= standard_max` — понятный текст ошибки.
    pub fn limits(&self) -> Result<(usize, usize)> {
        if self.fast_max >= self.standard_max {
            return Err(HarnessError::Config(format!(
                "[significance]: fast_max ({}) обязан быть меньше standard_max ({}) — \
                 иначе граница маршрутов Fast/Standard неопределённа",
                self.fast_max, self.standard_max
            )));
        }
        Ok((self.fast_max, self.standard_max))
    }

    /// Глобы детекторов диффа (T-05) — из секции `[significance]`.
    #[must_use]
    pub fn diff_globs(&self) -> crate::control::DiffGlobs {
        crate::control::DiffGlobs {
            contracts: self.contract_globs.clone(),
            components: self.component_globs.clone(),
            integrations: self.integration_globs.clone(),
            nfr: self.nfr_globs.clone(),
            security: self.security_globs.clone(),
            trust_zone: self.trust_zone_globs.clone(),
            data_contract: self.data_contract_globs.clone(),
        }
    }
}

/// Матрица обязательных составляющих составного гейта по маршруту (П1 ДКА).
///
/// Имена — имена составляющих `arch-be gate`: `fitness`, `delta_guard`,
/// `rule_weakened`, `spine_lint`, `trace_check`, `sensors`, `nfr`,
/// `evidence_verify`, `decision_quality`, `semantic_quality`, `secrets`,
/// `openspec_coverage`. Пустой список = на маршруте обязательных нет.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct GateConfig {
    /// Обязательные составляющие по маршрутам.
    pub required: RequiredRules,
    /// Анти-ослабление реестра (A1): severity находки `BodyChanged`.
    pub rule_weakened: RuleWeakenedConfig,
    /// Override по настоящему принятому ADR (A2): каталог ADR и горизонт.
    pub overrides: OverridesConfig,
    /// Составляющая `decision_quality` (Н7 волны B 0.3.4, ADR-042): качество
    /// архитектурных решений по отчёту рубрики-судьи.
    pub decision_quality: DecisionQualityConfig,
    /// Составляющая `semantic_quality` (ADR-052): смысловые рубрики —
    /// решение против инварианта, ссылка не на ту сущность, обещание без
    /// механизма, код против инварианта — по отчётам судьи.
    pub semantic_quality: SemanticQualityConfig,
    /// Что делать с решением рубрики `human` на каждом маршруте (E4.2).
    pub decision_policy: DecisionPolicyConfig,
    /// Составляющая `secrets` (C3 волны C 0.3.12): литеральные секреты в
    /// исходниках — детекторы [`crate::secrets::builtin_rules`].
    pub secrets: SecretsConfig,
    /// Составляющая `arch_drift` (K6 волны K 0.3.14): рёбра графа «как
    /// построено» против модели. Дорогая (снимок базовой ревизии + обход
    /// рабочего дерева), поэтому по умолчанию SKIP: включается записью
    /// `arch_drift` в `[gate.required]` маршрута (тогда находки блокируют)
    /// либо флагом здесь (находки — warn).
    pub arch_drift: ArchDriftConfig,
    /// Составляющая `openspec_coverage` (F2 волны F 0.3.14, ADR-067): покрытие
    /// требований `OpenSpec` правилами реестра — область проверки.
    #[serde(default)]
    pub openspec_coverage: OpenspecCoverageConfig,
    /// Реестр зондов доступности ресурсов `requires` (ADR-068 Am.3): секции
    /// `[gate.requires.<имя>]`. Зонд задаёт, как проверяется ресурс (файл,
    /// бинарь, переменная, hostname, устройство, команда), и текст SKIP.
    /// Переопределяет одноимённый встроенный зонд (`cuda`/`stand`); новый
    /// ресурс, не объявленный здесь и не встроенный, — ошибка реестра.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub requires: BTreeMap<String, crate::control::requires::RequiresProbeConfig>,
}

/// Настройки составляющей гейта `openspec_coverage` (F2): секция
/// `[gate.openspec_coverage]`. Только область: включение в блокирующие — через
/// `[gate.required]` маршрута (образец — `decision_quality`), ужесточений
/// без явного решения проекта нет.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct OpenspecCoverageConfig {
    /// Область проверки (дефолт `changed` — дешёвый режим потока доработок).
    pub scope: OpenspecCoverageScope,
}

/// Область покрытия требований `OpenSpec` в составляющей гейта (F2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OpenspecCoverageScope {
    /// Требования дельт активных changes, затронутых диффом `base..дерево`,
    /// плюс требования живых спек, чьи файлы изменены (дефолт).
    #[default]
    Changed,
    /// Все требования — как `openspec coverage`.
    All,
}

impl OpenspecCoverageScope {
    /// Метка области для детали составляющей.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Changed => "changed",
            Self::All => "all",
        }
    }
}

/// Severity находки `BodyChanged` анти-ослабления реестра (A1).
///
/// `warn` по умолчанию: изменение тела правила — сигнал, а не приговор
/// (у `glob`/`pattern` нет формальной семантики «ужесточения», поэтому любое
/// изменение — находка, и краснить ею чужой пайплайн без явного решения
/// нельзя). Проект, готовый к строгому режиму, ставит
/// `[gate.rule_weakened] body = "error"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BodySeverity {
    /// Предупреждение (дефолт): находка видна, вердикт не меняется.
    #[default]
    Warn,
    /// Блок: изменение тела правила валит составляющую `rule_weakened`.
    Error,
}

impl BodySeverity {
    /// Нормализованная метка severity (`warn`/`error`).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Warn => "warn",
            Self::Error => "error",
        }
    }
}

/// Настройки анти-ослабления реестра (A1): секция `[gate.rule_weakened]`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct RuleWeakenedConfig {
    /// Severity находки `BodyChanged` (дефолт `warn`).
    pub body: BodySeverity,
}

/// Severity находки `secret_literal` составляющей `secrets` (C3).
///
/// `warn` по умолчанию: детекторы работают по форматам и дают ложные
/// срабатывания на длинных hex (git-хэш, sha256) — краснить ими чужой
/// пайплайн без явного решения проекта нельзя. Проект, готовый к строгому
/// режиму, ставит `[gate.secrets] severity = "error"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SecretSeverity {
    /// Предупреждение (дефолт): находка видна, вердикт не меняется.
    #[default]
    Warn,
    /// Блок: литеральный секрет валит составляющую `secrets`.
    Error,
}

impl SecretSeverity {
    /// Нормализованная метка severity (`warn`/`error`).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Warn => "warn",
            Self::Error => "error",
        }
    }
}

/// Область сканирования составляющей `secrets` (C3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SecretScope {
    /// Только файлы диффа `base..HEAD` (как `rule_weakened`, дефолт) — дешёвый
    /// режим для потока доработок.
    #[default]
    Changed,
    /// Весь репозиторий (кроме служебных и сборочных каталогов).
    All,
}

/// Настройки составляющей `secrets` (C3): секция `[gate.secrets]`.
///
/// Сканирование исходников на литеральные секреты — эвристика известных
/// форматов, поэтому дефолт `warn`/`changed`: чужие пайплайны не краснеют,
/// включение строгости (`error`) и полной области (`all`) — явное решение.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SecretsConfig {
    /// Severity находки `secret_literal` (дефолт `warn`).
    pub severity: SecretSeverity,
    /// Область сканирования (дефолт `changed`).
    pub scope: SecretScope,
}

/// Настройки составляющей `arch_drift` (K6): секция `[gate.arch_drift]`.
///
/// Составляющая строит граф «как построено» дважды (базовая ревизия git +
/// рабочее дерево) и потому дорогая: по умолчанию она SKIP и включение —
/// осознанное решение проекта (обратная совместимость, правило 4).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ArchDriftConfig {
    /// Включить `arch_drift` вне `[gate.required]`: находки о рёбрах вне
    /// модели и отклонённых рёбрах в коде — warn. `false` (дефолт) —
    /// составляющая не прогоняется, если не названа в `[gate.required]`.
    pub enabled: bool,
    /// Потолок числа читаемых в снимок файлов (ADR-068 Am.2). `None` —
    /// прежний дефолт [`crate::arch_diff::MAX_SNAPSHOT_CONTENT_FILES`] (2000).
    /// Большие кейсы (легаси с `env/` на тысячи файлов) поднимают лимит, не
    /// ломая границу bounded-работы для остальных.
    pub max_files: Option<usize>,
    /// Подстроки путей-исключений из снимка (ADR-068 Am.2): файлы, чей путь
    /// содержит подстроку, не попадают ни в список, ни в содержимое. Пусто
    /// (дефолт) — ничего не исключается.
    pub ignore: Vec<String>,
}

/// Настройки проверки overrides (A2): секция `[gate.overrides]`.
///
/// Override узаконивает ослабление правила только тогда, когда ADR с его
/// номером существует и принят, а срок `until` не уходит за горизонт. Так
/// «override на выдуманный ADR-999 с until 2099» перестаёт зеленеть гейт.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct OverridesConfig {
    /// Каталог ADR кейса: относительный — от корня репозитория, абсолютный —
    /// как есть. `None` — `docs/adr`.
    pub adr_dir: Option<String>,
    /// Максимальный горизонт `until`, месяцев (дефолт 12): override — срочное
    /// исключение, а не вечное отключение правила.
    pub max_horizon_months: u32,
}

impl Default for OverridesConfig {
    fn default() -> Self {
        Self {
            adr_dir: None,
            max_horizon_months: crate::control::baseline::DEFAULT_MAX_HORIZON_MONTHS,
        }
    }
}

/// Что делает гейт с решением рубрики `human` (E4.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HumanPolicy {
    /// Предупреждение: находка видна, вердикт не меняется.
    #[default]
    Warn,
    /// Блок: составляющая уходит в SKIP, вердикт INCOMPLETE — решение за
    /// человеком (очередь `reports/human/`, E4.4).
    Human,
}

/// Политика решения `human` по маршрутам значимости (E4.2; раздел 5.2 ревью).
///
/// По умолчанию блокирует только Critical: на Fast/Standard решение модели —
/// повод для предупреждения и задачи архитектору, а не остановка потока.
/// Инъекция во входе и невалидные сэмплы этой политике **не подчиняются** —
/// они эскалируются всегда (E2/E3.2): доверие к суждению подорвано, и
/// «понизить до warn» это не настройка, а отключение страховки.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DecisionPolicyConfig {
    /// Маршрут Fast: по умолчанию предупреждение.
    pub fast: HumanPolicy,
    /// Маршрут Standard: по умолчанию предупреждение и задача архитектору.
    pub standard: HumanPolicy,
    /// Маршрут Critical: по умолчанию блок до решения архитектора.
    pub critical: HumanPolicy,
}

impl Default for DecisionPolicyConfig {
    fn default() -> Self {
        Self {
            fast: HumanPolicy::Warn,
            standard: HumanPolicy::Warn,
            critical: HumanPolicy::Human,
        }
    }
}

impl DecisionPolicyConfig {
    /// Политика маршрута; библиотечный вызов без маршрута — как на Fast
    /// (мягко): эскалация требует явного решения проекта.
    #[must_use]
    pub fn for_route(&self, route: Option<crate::control::Route>) -> HumanPolicy {
        match route {
            Some(crate::control::Route::Critical) => self.critical,
            Some(crate::control::Route::Standard) => self.standard,
            Some(crate::control::Route::Fast) | None => self.fast,
        }
    }
}

/// На что смотрит составляющая `semantic_quality` (ADR-052).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SemanticScope {
    /// Субъекты, чьё досье затронуто диффом от базы или активной дельтой, —
    /// дешёвый режим для потока доработок (дефолт).
    #[default]
    Changed,
    /// Весь пакет: все ADR, сущности модели и файлы кода под корнями компонент.
    All,
}

/// Составляющая `semantic_quality` (ADR-052).
///
/// Пустой список `rubrics` — составляющая ничего не требует, даже будучи
/// включённой в `[gate.required]`: включать её можно раньше, чем заводить
/// отчёты. Оценка дорогая (k сэмплов судьи на субъект), поэтому гейт **не
/// оценивает** — он читает отчёты, которые положил хост, и переиспользует
/// отчёт с совпавшим хэшем досье (ADR-051).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SemanticQualityConfig {
    /// Имена рубрик из каталога рубрик (`paths.rubrics_dir`), обязательных к
    /// отчёту. Пусто — ничего не требуется.
    #[serde(default)]
    pub rubrics: Vec<String>,
    /// Область субъектов.
    #[serde(default)]
    pub scope: SemanticScope,
    /// Минимальный взвешенный итог отчёта.
    #[serde(default = "default_semantic_min_score")]
    pub min_score: f64,
    /// Требовать от судьи модель, отличную от автора документа: `true` —
    /// `judge_is_author` становится error, `false` — warn.
    #[serde(default)]
    pub require_distinct_judge: bool,
    /// Порог доли невалидных сэмплов (балл вне шкалы) для смысловых рубрик
    /// (E3.2): выше порога составляющая уходит в SKIP — вердикт INCOMPLETE.
    #[serde(default = "default_max_invalid_ratio")]
    pub max_invalid_samples_ratio: f64,
    /// Требовать от судьи пройденную квалификацию (E6.3): на блокирующем
    /// маршруте находка `judge_unqualified` → INCOMPLETE. Дефолт `false`.
    #[serde(default)]
    pub require_qualified_judge: bool,
}

/// Дефолт порога доли невалидных сэмплов (E3.2): половина и больше.
fn default_max_invalid_ratio() -> f64 {
    0.5
}

/// Порог взвешенного итога смысловой рубрики по умолчанию.
///
/// `3.0`, а не `3.5` как у `decision_quality`: порог смысловой рубрики
/// откалиброван по живому прогону 2026-09-20, а не по аналогии. На четырёх
/// прогонах `adr_spine_consistency`/`model_link_semantics`/
/// `code_invariant_conformance` чистый субъект дал 3.43, засеянные дефекты —
/// 2.12, 2.00 и 1.00. При `3.5` краснел бы единственный чистый контроль
/// (3.43 < 3.5), то есть порог был бы заведомо ложным; `3.0` разделяет те же
/// четыре точки с запасом 0.43 снизу и 0.88 сверху. Замер — в
/// `docs/experiments/semantic-rubrics-2026-09-20.md`; порог настраивается
/// (`min_score`), потому что выборка мала.
fn default_semantic_min_score() -> f64 {
    3.0
}

impl Default for SemanticQualityConfig {
    fn default() -> Self {
        Self {
            rubrics: Vec::new(),
            scope: SemanticScope::Changed,
            min_score: default_semantic_min_score(),
            require_distinct_judge: false,
            max_invalid_samples_ratio: default_max_invalid_ratio(),
            require_qualified_judge: false,
        }
    }
}

/// Настройки составляющей гейта `decision_quality` (Н7, ADR-042).
///
/// Составляющая по умолчанию **не обязательна** ни на одном маршруте: SKIP
/// обязательной составляющей даёт INCOMPLETE, и включение порога качества
/// должно быть осознанным решением проекта, а не сюрпризом после обновления.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DecisionQualityConfig {
    /// Минимальный взвешенный итог рубрики `adr_quality` для Accepted-ADR.
    /// Порог `3.5` — как в чек-листе скилла `adr-authoring`.
    pub min_score: f64,
    /// Требовать от судьи модель, отличную от автора документа: `true` —
    /// `judge_is_author` становится error, `false` — warn.
    pub require_distinct_judge: bool,
    /// Требовать судью из другого СЕМЕЙСТВА моделей (ADR-048): `true` —
    /// `judge_same_family` становится error, `false` (дефолт) — warn.
    /// По умолчанию warn: смена модели внутри одного семейства — обычная
    /// практика, а не нарушение; ужесточение — осознанный выбор проекта.
    pub require_distinct_family: bool,
    /// Минимальный уровень независимости судьи (ADR-048): ниже порога —
    /// находка `judge_independence_low` (error). Дефолт `none` — поведение
    /// 0.3.4: независимость заявлена, порога нет. Уровни:
    /// `none` < `declared` < `declared_cross_family` < `launched` <
    /// `launched_cross_family`.
    pub min_independence: String,
    /// Порог доли сэмплов судьи с баллом вне шкалы (E3.2). Выше порога
    /// суждению верить нельзя: составляющая уходит в SKIP — вердикт
    /// INCOMPLETE, решение за человеком. Дефолт `0.5`: половина сэмплов и
    /// больше — не «шум одного ответа». Любая ненулевая доля ниже порога
    /// оставляет warn-находку, поэтому невалидный сэмпл не бывает невидимым.
    pub max_invalid_samples_ratio: f64,
    /// Требовать от судьи пройденную квалификацию на эталонном наборе (E6.3):
    /// на блокирующем маршруте находка `judge_unqualified` → INCOMPLETE.
    /// Дефолт `false` — поведение 0.3.8: допуск включает проект осознанно,
    /// прогнав `arch-be rubric qualify` (иначе ужесточение покраснило бы
    /// чужие пайплайны без предупреждения).
    #[serde(default)]
    pub require_qualified_judge: bool,
}

impl Default for DecisionQualityConfig {
    fn default() -> Self {
        Self {
            min_score: 3.5,
            require_distinct_judge: false,
            require_distinct_family: false,
            min_independence: crate::judge::INDEPENDENCE_NONE.to_string(),
            max_invalid_samples_ratio: 0.5,
            require_qualified_judge: false,
        }
    }
}

/// Семантика артефактов Evidence Bundle (Н1 волны A 0.3.4, ADR-041).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EvidenceSemantics {
    /// По маршруту: Critical — `error`, Standard/Fast — `warn` (дефолт).
    #[default]
    Auto,
    /// Проверки содержания выключены (поведение 0.3.3).
    Off,
    /// Все находки о содержании — предупреждения, выпуск не блокируют.
    Warn,
    /// Все находки о содержании блокируют выпуск на любом маршруте.
    Error,
}

/// Секция `[evidence]`: семантика артефактов бандла (Н1, ADR-041).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct EvidenceConfig {
    /// Минимальный размер артефакта в байтах: меньше — «пустышка».
    pub min_bytes: u64,
    /// Строгость находок о содержании артефактов.
    pub semantics: EvidenceSemantics,
    /// Требование машинных записей прогонов (A1, 0.3.14): `true` — отчёт
    /// `validation`/`fitness_report`/`walking_skeleton`, написанный прозой без
    /// записи `.arch-handoff/evidence/<kind>.json` (`arch-be evidence record`),
    /// блокирует выпуск (`evidence_report_unbound` — `error`); `false`
    /// (дефолт) — та же находка остаётся предупреждением. Схема «warn → error
    /// по флагу» (правило 4): поведение прежних версий не меняется без явного
    /// решения проекта.
    #[serde(default)]
    pub require_records: bool,
}

/// Дефолтный порог «пустышки»: 200 байт (стартовое предложение ТЗ 0.3.4,
/// обоснование — ADR-041).
pub const DEFAULT_EVIDENCE_MIN_BYTES: u64 = 200;

impl Default for EvidenceConfig {
    fn default() -> Self {
        Self {
            min_bytes: DEFAULT_EVIDENCE_MIN_BYTES,
            semantics: EvidenceSemantics::Auto,
            require_records: false,
        }
    }
}

impl EvidenceConfig {
    /// Строгость находок о содержании для маршрута (`Auto` разворачивается
    /// по маршруту: Critical — блокирует выпуск).
    #[must_use]
    pub fn severity_for(&self, route: crate::control::Route) -> Option<&'static str> {
        match self.semantics {
            EvidenceSemantics::Off => None,
            EvidenceSemantics::Warn => Some("warn"),
            EvidenceSemantics::Error => Some("error"),
            EvidenceSemantics::Auto => Some(if route == crate::control::Route::Critical {
                "error"
            } else {
                "warn"
            }),
        }
    }
}

/// Строгость требования «у инварианта есть исполняемая проверка» (ADR-050).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExecutableRequired {
    /// Требования нет: звено «AD → исполняемое правило» информационное (дефолт).
    #[default]
    Off,
    /// Находка `ad-text-only` уровня warn: сигнал, вердикт не меняется.
    Warn,
    /// Находка `ad-text-only` уровня error: гейт краснеет.
    Error,
}

/// Секция `[trace]`: требование исполняемой проверки инвариантов (ADR-050).
///
/// Дефолт `off` — обратная совместимость: до 0.3.5 находки `ad-text-only` не
/// существовало, и включение требования — осознанное решение проекта.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TraceConfig {
    /// Строгость находки `ad-text-only` по инвариантам без исполняемой проверки.
    pub executable_required: ExecutableRequired,
}

impl Default for TraceConfig {
    fn default() -> Self {
        Self {
            executable_required: ExecutableRequired::Off,
        }
    }
}

/// Секция `[drift]`: проверки дрейфа «модель ↔ код» (волна C 0.3.14).
///
/// Дефолт off — обратная совместимость: находка `nfr-metric-missing`
/// появляется только при осознанном включении (`[drift] nfr_metric_check =
/// true` в конфиге кейса).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct DriftConfig {
    /// C3: метрика из `verification` NFR (`snake_case` с суффиксом
    /// `_seconds`/`_total`/`_bytes`/`_ratio`) обязана встречаться в коде CMP,
    /// реализующих NFR (`implements`), иначе warn-находка
    /// `nfr-metric-missing`.
    pub nfr_metric_check: bool,
}

/// Секция `[survey]`: денилист обследования (B8 волны B).
///
/// Списки **дополняют** встроенный набор (lock-файлы, лицензии, метаданные
/// чартов, funding-хосты), а не заменяют его: пользователь может добавить
/// свои пути/хосты, не теряя защиты по умолчанию. Совпадение пути — по
/// имени файла или glob-суффиксу (`*.lock`); совпадение хоста — по
/// равенству или dot-суффиксу (`example.com` покрывает `www.example.com`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SurveyConfig {
    /// Дополнительные хосты, которые не считаются интеграциями.
    pub ignore_hosts: Vec<String>,
    /// Дополнительные пути/имена файлов, которые не сканируются как
    /// источники интеграций.
    pub ignore_paths: Vec<String>,
}

impl ExecutableRequired {
    /// Severity находки `ad-text-only` (`None` — находки нет).
    #[must_use]
    pub fn severity(self) -> Option<&'static str> {
        match self {
            Self::Off => None,
            Self::Warn => Some("warn"),
            Self::Error => Some("error"),
        }
    }
}

/// Списки обязательных составляющих для Fast / Standard / Critical.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct RequiredRules {
    /// Маршрут Fast.
    pub fast: Vec<String>,
    /// Маршрут Standard.
    pub standard: Vec<String>,
    /// Маршрут Critical.
    pub critical: Vec<String>,
}

impl Default for RequiredRules {
    fn default() -> Self {
        let base = vec!["fitness".to_string(), "spine_lint".to_string()];
        let mut standard = base.clone();
        standard.extend(
            ["trace_check", "rule_weakened", "nfr", "model_validate"]
                .iter()
                .map(|s| (*s).to_string()),
        );
        let mut critical = standard.clone();
        critical.extend(
            ["delta_guard", "sensors", "evidence_verify"]
                .iter()
                .map(|s| (*s).to_string()),
        );
        Self {
            fast: base,
            standard,
            critical,
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
            cron: CronSettings::default(),
            judge: JudgeConfig::default(),
            trust: TrustConfig::default(),
            redteam: RedteamConfig::default(),
            fleet: FleetConfig::default(),
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

    /// Загружает конфиг: `--config` → `./arch-harness.toml` →
    /// `~/.config/arch-harness/config.toml` → дефолты.
    ///
    /// # Errors
    /// Ошибка чтения/разбора файла, если явный путь задан и недоступен.
    pub fn load(cli_path: Option<&Path>) -> Result<Self> {
        let candidates: Vec<PathBuf> = match cli_path {
            Some(p) => vec![p.to_path_buf()],
            None => vec![
                PathBuf::from("arch-harness.toml"),
                Self::config_dir().join("config.toml"),
            ],
        };
        for path in &candidates {
            if path.is_file() {
                let text = std::fs::read_to_string(path).map_err(|e| HarnessError::io(path, e))?;
                let mut cfg: Config = toml::from_str(&text)?;
                // Fail-fast валидация значений, которые serde не проверяет
                // (поле-строка с ограниченным набором значений).
                cfg.bash.sandbox_mode()?;
                cfg.expand_tildes();
                cfg.loaded_from = Some(path.clone());
                return Ok(cfg);
            }
        }
        let mut cfg = Config::default();
        cfg.expand_tildes();
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
