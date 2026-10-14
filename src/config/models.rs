//! Конфигурация LLM-провайдеров: [`ModelConfig`] (OpenAI-совместимый
//! endpoint или внешний CLI-агент) и OAuth2-параметры [`OAuthConfig`].

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::error::{HarnessError, Result};

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

    /// Доступен ли API-ключ: env `api_key_env` непустая после `trim` ИЛИ файл
    /// `api_key_file` (`~` раскрывается) читается и непуст. Семантика та же,
    /// что у резолва ключа в провайдерах; содержимое ключа никуда не выводится.
    /// Предполётная проверка агентного цикла (ADR-083 п.2) не должна ходить в
    /// сеть — достаточно факта наличия ключа.
    #[must_use]
    pub fn api_key_available(&self) -> bool {
        if std::env::var(&self.api_key_env).is_ok_and(|raw| !raw.trim().is_empty()) {
            return true;
        }
        self.api_key_file.as_deref().is_some_and(|path| {
            let expanded = match path.strip_prefix("~/") {
                Some(rest) => {
                    dirs::home_dir().map_or_else(|| PathBuf::from(path), |h| h.join(rest))
                }
                None => PathBuf::from(path),
            };
            std::fs::read_to_string(&expanded).is_ok_and(|raw| !raw.trim().is_empty())
        })
    }
}
