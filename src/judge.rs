//! Происхождение оценки рубрики: кто судил, чем и что из этого удостоверено
//! механикой (задание «происхождение оценки», ADR-048).
//!
//! Spine не знает, какая модель отвечала на самом деле, **ни в одном режиме**:
//! при split-judge через MCP метки (`judge_model`, `author_model`) передаёт хост,
//! при запуске судьи самим Spine известна только команда и аргументы запуска.
//! Модуль фиксирует то, что механика действительно знает, и называет то, чего
//! она не знает, — ни одно поле и ни одна формулировка вывода не обещают
//! «подтверждённой независимости судьи».
//!
//! Два режима происхождения:
//! - [`MODE_DECLARED`] — оценку собрал хост (split-judge через `rubric_verify`);
//!   сервер видит только метки и то, сколько вызовов было в сессии до судейства;
//! - [`MODE_LAUNCHED`] — судью запустил Spine (провайдер `kind = "cli"` или
//!   API-модель): известны команда, аргументы и то, что каждый сэмпл — отдельный
//!   процесс/запрос без истории.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

/// Режим происхождения: метки передал хост (split-judge через MCP).
pub const MODE_DECLARED: &str = "declared";
/// Режим происхождения: судью запустил сам Spine (CLI-провайдер или API).
pub const MODE_LAUNCHED: &str = "launched";

/// Порог «чистой сессии» по умолчанию: до стольких вызовов MCP в сессии
/// судейство считается (косвенно) идущим без рабочего контекста автора.
pub const DEFAULT_CLEAN_SESSION_MAX_CALLS: usize = 3;

/// Сколько ждём ответа `<command> --version`, прежде чем считать версию
/// неизвестной: CLI-харнессы отвечают мгновенно, а висящий неизвестный
/// бинарь не должен задерживать судейство.
pub const CLI_VERSION_TIMEOUT_SECS: u64 = 2;

/// Хост-харнесс, назвавшийся в рукопожатии MCP (`initialize.clientInfo`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostInfo {
    /// Имя харнесса (`claude-code`, `qwen-code`, …) — как назвался сам хост.
    pub name: String,
    /// Версия хоста, если он её сообщил.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

/// Запускатель судьи: чем Spine запускал оценку (режим [`MODE_LAUNCHED`]).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Launcher {
    /// Имя модели из `[models]` конфига: и для CLI-провайдера, и для API.
    pub provider: String,
    /// Команда CLI-харнесса (`command` из `[models]`); у API-модели нет.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// Аргументы запуска как есть (значения, похожие на секреты, замаскированы).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    /// Версия CLI (`<command> --version`), если команда ответила.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cli_version: Option<String>,
}

/// Отпечаток одного сырого ответа судьи: хэш текста ответа и признак того,
/// что ответ не разобрался и в отчёт не вошёл (J2).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SampleStamp {
    /// SHA-256 текста сырого ответа.
    pub sha256: String,
    /// Ответ не разобран и отброшен (в расчёт баллов не вошёл).
    #[serde(default)]
    pub dropped: bool,
}

/// Происхождение оценки: что механика знает о том, как получен отчёт.
///
/// Поле аддитивное: у отчётов до появления блока его нет, и это читается как
/// режим [`MODE_DECLARED`] без деталей — старая оценка не становится ни хуже,
/// ни лучше, она просто не несёт происхождения.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RubricProvenance {
    /// Режим: [`MODE_DECLARED`] или [`MODE_LAUNCHED`].
    pub mode: String,
    /// Хост из рукопожатия (только `declared`: при запуске судьи самим Spine
    /// хост — это тот, кто попросил оценить, а не тот, кто судил).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<HostInfo>,
    /// Идентификатор MCP-сессии, выданный сервером на `initialize`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// Вызовов MCP в сессии до выдачи промпта судьи по этой цели — **косвенный**
    /// признак того, что судейство шло в рабочей сессии (судья мог видеть
    /// контекст автора); сам по себе он ничего не доказывает.
    #[serde(default)]
    pub session_calls_before: usize,
    /// SHA-256 system+user промпта, выданного `rubric_prompt` в этой сессии.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_sha256: Option<String>,
    /// true — `rubric_verify` пришёл в ту же сессию, что и `rubric_prompt`.
    /// false — судили в другой сессии (в том числе в чистой): это не ошибка,
    /// просто происхождение промпта механикой не связано.
    #[serde(default)]
    pub prompt_issued_in_session: bool,
    /// Запускатель судьи (только `launched`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launcher: Option<Launcher>,
    /// Отпечатки сырых ответов судьи (пусто — ответы не сохранены, отчёт
    /// невоспроизводим).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub samples: Vec<SampleStamp>,
    /// Кто организовал судейство: git `user.name`/`user.email` репозитория,
    /// best effort. Это **запись из git-конфига, а не подпись**: личность
    /// архитектора механикой не удостоверяется.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operator: Option<String>,
}

impl RubricProvenance {
    /// Происхождение для отчёта, собранного хостом (split-judge через MCP).
    #[must_use]
    pub fn declared(host: Option<HostInfo>, session_id: Option<String>) -> Self {
        Self {
            mode: MODE_DECLARED.to_string(),
            host,
            session_id,
            ..Self::default()
        }
    }

    /// Происхождение для отчёта, судью которого запустил сам Spine.
    #[must_use]
    pub fn launched(launcher: Launcher) -> Self {
        Self {
            mode: MODE_LAUNCHED.to_string(),
            launcher: Some(launcher),
            ..Self::default()
        }
    }

    /// Режим происхождения: у отчёта без блока — `declared` без деталей.
    #[must_use]
    pub fn mode(&self) -> &str {
        if self.mode.is_empty() {
            MODE_DECLARED
        } else {
            self.mode.as_str()
        }
    }

    /// Запущен ли судья самим Spine.
    #[must_use]
    pub fn is_launched(&self) -> bool {
        self.mode() == MODE_LAUNCHED
    }
}

/// Один выданный промпт судьи в сессии: хэш промпта и счётчик вызовов на
/// момент выдачи (для `session_calls_before`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssuedPrompt {
    /// SHA-256 текста system+user промпта.
    pub sha256: String,
    /// Вызовов MCP в сессии до выдачи промпта.
    pub calls_before: usize,
}

/// Состояние MCP-сессии: хост из рукопожатия, выданный идентификатор, счётчик
/// вызовов и выданные промпты судьи по ключу «рубрика + цель».
///
/// Сервер stdio живёт ровно одну сессию (один процесс — один клиент), поэтому
/// состояние принадлежит серверу, а не соединению: переподключение хоста
/// начинает новую сессию с новым идентификатором.
#[derive(Debug, Default)]
pub struct SessionState {
    id: String,
    host: Option<HostInfo>,
    calls: usize,
    prompts: BTreeMap<String, IssuedPrompt>,
}

impl SessionState {
    /// Новая сессия с выданным идентификатором.
    #[must_use]
    pub fn new() -> Self {
        Self {
            id: new_session_id(),
            ..Self::default()
        }
    }

    /// Идентификатор сессии (выдан на `initialize`).
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Запоминает `clientInfo` рукопожатия (повторный `initialize` перезаписывает).
    pub fn set_host(&mut self, host: HostInfo) {
        self.host = Some(host);
    }

    /// Хост, назвавшийся в рукопожатии.
    #[must_use]
    pub fn host(&self) -> Option<HostInfo> {
        self.host.clone()
    }

    /// Отмечает завершённый вызов инструмента: счётчик растёт ПОСЛЕ вызова,
    /// поэтому во время обработки `calls()` — число уже прошедших вызовов.
    pub fn note_call(&mut self) {
        self.calls = self.calls.saturating_add(1);
    }

    /// Вызовов в сессии на текущий момент.
    #[must_use]
    pub fn calls(&self) -> usize {
        self.calls
    }

    /// Запоминает выданный промпт судьи для пары «рубрика + цель».
    pub fn record_prompt(&mut self, key: &str, sha256: String) {
        self.prompts.insert(
            key.to_string(),
            IssuedPrompt {
                sha256,
                calls_before: self.calls,
            },
        );
    }

    /// Выданный в этой сессии промпт по паре «рубрика + цель», если он был.
    #[must_use]
    pub fn issued_prompt(&self, key: &str) -> Option<IssuedPrompt> {
        self.prompts.get(key).cloned()
    }
}

/// Ключ «рубрика + цель» для сопоставления `rubric_prompt` и `rubric_verify`:
/// целью служит SHA-256 текста документа, а не написание пути, — тот же
/// документ, названный по-разному (относительный путь, абсолютный, inline-текст),
/// даёт тот же ключ, а текст в ключ не попадает.
#[must_use]
pub fn prompt_key(rubric: &str, target_text: &str) -> String {
    format!(
        "{rubric}|{}",
        crate::hash::sha256_hex(target_text.as_bytes())
    )
}

/// SHA-256 промпта судьи (system + user) — то, что именно было выдано хосту.
#[must_use]
pub fn prompt_sha256(system: &str, user: &str) -> String {
    crate::hash::sha256_hex(format!("{system}\n{user}").as_bytes())
}

/// Идентификатор сессии: SHA-256 от времени в наносекундах, pid и счётчика
/// процесса — 16 hex-символов. Своя криптография не нужна: идентификатор
/// различает сессии, а не удостоверяет личность (ADR-048: подписи нет).
#[must_use]
pub fn new_session_id() -> String {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let canon = format!("{nanos}:{}:{seq}", std::process::id());
    let hex = crate::hash::sha256_hex(canon.as_bytes());
    hex[..16.min(hex.len())].to_string()
}

/// Кто организовал судейство: git `user.name` и `user.email` **репозитория**
/// (best effort — без настроенного пользователя `None`).
///
/// Вне git-репозитория git отдаёт глобальную настройку пользователя: это тот же
/// оператор, и запись остаётся честной («кто зафиксирован в конфиге»), а не
/// удостоверением личности.
#[must_use]
pub fn operator(repo: &Path) -> Option<String> {
    let name = git_config(repo, "user.name");
    let email = git_config(repo, "user.email");
    match (name, email) {
        (Some(name), Some(email)) => Some(format!("{name} <{email}>")),
        (Some(name), None) => Some(name),
        (None, email) => email,
    }
}

/// Значение одной настройки git-конфига репозитория (`None` — нет настройки,
/// нет git или команда не ответила).
fn git_config(repo: &Path, key: &str) -> Option<String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["config", "--get", key])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let value = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if value.is_empty() { None } else { Some(value) }
}

/// Запускатель судьи по конфигу модели: CLI-провайдер несёт команду и
/// аргументы запуска (значения, похожие на секреты, замаскированы), API —
/// только имя модели из `[models]`.
///
/// Известна именно КОМАНДА запуска, а не то, какая модель отвечала: API-ключ
/// и маршрутизация провайдера — вне контроля Spine (ADR-048).
#[must_use]
pub fn launcher_for(cfg: &crate::config::Config, model_name: &str) -> Launcher {
    let model = cfg.models.get(model_name);
    let is_cli = model.is_some_and(|m| m.kind.as_deref() == Some("cli"));
    if !is_cli {
        return Launcher {
            provider: model_name.to_string(),
            ..Launcher::default()
        };
    }
    let command = model.and_then(|m| m.command.clone());
    let args = model.map(|m| m.args.clone()).unwrap_or_default();
    Launcher {
        provider: model_name.to_string(),
        command: command.clone(),
        args: redact_args(&args),
        cli_version: command.as_deref().and_then(cli_version),
    }
}

/// Аргументы запуска с маскировкой значений, похожих на секреты: ключи не
/// попадают в коммитимый отчёт (AD-3, `src/secrets.rs`).
#[must_use]
pub fn redact_args(args: &[String]) -> Vec<String> {
    let redactor = crate::secrets::Redactor::with_builtin_rules();
    args.iter().map(|a| redactor.redact(a)).collect()
}

/// Версия CLI-харнесса: `<command> --version`, если команда ответила за
/// [`CLI_VERSION_TIMEOUT_SECS`]. Best effort — неизвестная версия не ошибка,
/// а честное «не знаем».
///
/// Проба идёт в отдельном потоке, потому что у `std::process::Command` нет
/// таймаута: если команда не ответит, поток остаётся ждать её и завершится
/// вместе с процессом Spine (висящий бинарь уже не наш контур).
#[must_use]
pub fn cli_version(command: &str) -> Option<String> {
    let (tx, rx) = std::sync::mpsc::channel();
    let command = command.to_string();
    std::thread::spawn(move || {
        let out = std::process::Command::new(&command)
            .arg("--version")
            .stdin(std::process::Stdio::null())
            .output();
        let version = out
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .filter(|s| !s.is_empty());
        let _ = tx.send(version);
    });
    rx.recv_timeout(std::time::Duration::from_secs(CLI_VERSION_TIMEOUT_SECS))
        .ok()
        .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Идентификаторы сессий различаются и остаются коротким hex: это метка
    /// сессии, а не секрет (ADR-048).
    #[test]
    fn session_ids_differ_and_are_short_hex() {
        let first = new_session_id();
        let second = new_session_id();
        assert_ne!(first, second, "идентификаторы сессий совпали");
        assert_eq!(first.len(), 16, "{first}");
        assert!(
            first.chars().all(|c| c.is_ascii_hexdigit()),
            "не hex: {first}"
        );
    }

    /// Аргументы запуска судьи маскируются: ключ провайдера не попадает в
    /// коммитимый отчёт (AD-3).
    #[test]
    fn redacts_secret_looking_launcher_args() {
        let args = vec![
            "-p".to_string(),
            "--api-key=sk-abcdef0123456789abcdef0123456789".to_string(),
            "DEEPSEEK_API_KEY=abcdef0123456789".to_string(),
        ];
        let red = redact_args(&args);
        assert_eq!(red[0], "-p", "обычный флаг не меняется: {red:?}");
        assert!(
            !red[1].contains("0123456789abcdef"),
            "ключ остался в аргументе: {red:?}"
        );
        assert!(red[1].contains("***"), "{red:?}");
        assert!(
            !red[2].contains("abcdef0123456789"),
            "значение переменной осталось: {red:?}"
        );
    }

    /// API-провайдер (не `kind = "cli"`) в запускателе несёт только имя
    /// модели: команды запуска у него нет.
    #[test]
    fn api_provider_launcher_has_only_name() {
        let cfg = crate::config::Config::default();
        let launcher = launcher_for(&cfg, "deepseek-v4-pro");
        assert_eq!(launcher.provider, "deepseek-v4-pro");
        assert!(launcher.command.is_none(), "{launcher:?}");
        assert!(launcher.args.is_empty(), "{launcher:?}");
        assert!(launcher.cli_version.is_none(), "{launcher:?}");
    }

    /// CLI-провайдер в запускателе называет команду и аргументы; недоступная
    /// команда не роняет судейство — версия просто неизвестна.
    #[test]
    fn cli_provider_launcher_names_command_and_args() {
        let mut cfg = crate::config::Config::default();
        cfg.models.insert(
            "judge-cli".to_string(),
            crate::config::ModelConfig {
                kind: Some("cli".to_string()),
                command: Some("definitely-not-a-real-cli-xyz".to_string()),
                args: vec!["exec".to_string()],
                ..crate::config::ModelConfig::default()
            },
        );
        let launcher = launcher_for(&cfg, "judge-cli");
        assert_eq!(
            launcher.command.as_deref(),
            Some("definitely-not-a-real-cli-xyz")
        );
        assert_eq!(launcher.args, vec!["exec".to_string()]);
        assert!(
            launcher.cli_version.is_none(),
            "несуществующая команда не может назвать версию: {launcher:?}"
        );
    }

    /// Оператор — запись из git-конфига РЕПОЗИТОРИЯ: имя и адрес как есть,
    /// без попытки что-то о них утверждать (это не подпись).
    #[test]
    fn operator_reads_repository_git_config() {
        let dir = tempfile::tempdir().expect("tmp");
        let git = |args: &[&str]| {
            let status = std::process::Command::new("git")
                .arg("-C")
                .arg(dir.path())
                .args(args)
                .status()
                .expect("git доступен");
            assert!(status.success(), "git {args:?}");
        };
        git(&["init", "-q"]);
        git(&["config", "user.name", "Тест Архитектор"]);
        git(&["config", "user.email", "arch@example.invalid"]);
        assert_eq!(
            operator(dir.path()).as_deref(),
            Some("Тест Архитектор <arch@example.invalid>")
        );
    }

    /// Ключ промпта не зависит от написания пути: он считается по тексту
    /// документа, а разные документы дают разные ключи.
    #[test]
    fn prompt_key_depends_on_text_not_path() {
        let first = prompt_key("adr_quality", "контекст описан");
        let second = prompt_key("adr_quality", "контекст описан");
        let other = prompt_key("adr_quality", "другой текст");
        assert_eq!(first, second);
        assert_ne!(first, other);
        assert!(
            !first.contains("контекст"),
            "текст не попадает в ключ: {first}"
        );
    }
}
