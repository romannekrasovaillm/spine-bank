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
use std::path::{Path, PathBuf};
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

// ---------------------------------------------------------------------------
// J2: сырые ответы судьи и воспроизводимость отчёта
// ---------------------------------------------------------------------------

/// Каталог сырых ответов судьи внутри репозитория: `reports/rubric/raw/<slug>/`.
pub const RUBRIC_RAW_DIR: &str = "reports/rubric/raw";

/// Схема файла сырого ответа судьи.
pub const RUBRIC_RAW_SCHEMA: &str = "arch-be/rubric-raw/v1";

/// Каталог сырых ответов конкретного отчёта.
#[must_use]
pub fn raw_dir(repo: &Path, slug: &str) -> PathBuf {
    repo.join(RUBRIC_RAW_DIR).join(slug)
}

/// Имя файла сырого ответа: `sample-<n>.json` (нумерация с 1 — по порядку
/// ответов в вызове).
#[must_use]
pub fn raw_file_name(sample: usize) -> String {
    format!("sample-{sample}.json")
}

/// Ответ судьи на входе сохранения: текст как он пришёл и признак того, что
/// он не разобрался и в расчёт баллов не вошёл.
#[derive(Debug, Clone)]
pub struct RawAnswerInput {
    /// Текст ответа судьи без правок.
    pub text: String,
    /// Ответ не разобран (в медиану не входил).
    pub dropped: bool,
}

/// Сырой ответ судьи на диске: текст, его хэш и служебные метки.
///
/// Хэш считается от `text`: правка текста после сохранения видна сверке
/// (находка `rubric_raw_tampered`), а балл отчёта пересчитывается из ТЕХ ЖЕ
/// ответов — «поправить цифру в отчёте» больше не проходит незамеченным.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawAnswer {
    /// Схема файла.
    pub schema: String,
    /// Имя рубрики, по которой судили.
    pub rubric: String,
    /// Путь оценённого документа относительно репозитория.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    /// SHA-256 документа на момент оценки.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_sha256: Option<String>,
    /// Номер сэмпла (с 1).
    pub sample: usize,
    /// Метка модели-судьи (та же, что в отчёте).
    pub judge_model: String,
    /// SHA-256 текста ответа на момент сохранения.
    pub sha256: String,
    /// Ответ не разобран и в расчёт баллов не вошёл.
    #[serde(default)]
    pub dropped: bool,
    /// Текст ответа как он пришёл.
    pub text: String,
    /// Метка времени сохранения (RFC 3339).
    pub saved_at: String,
}

impl RawAnswer {
    /// Текст ответа не сходится с записанным хэшем — файл правили после
    /// сохранения. Оценка, собранная из подменённых ответов, невоспроизводима.
    #[must_use]
    pub fn tampered(&self) -> bool {
        crate::hash::sha256_hex(self.text.as_bytes()) != self.sha256
    }
}

/// Сохраняет сырые ответы судьи в `reports/rubric/raw/<slug>/` и возвращает
/// их отпечатки для поля `provenance.samples`.
///
/// # Errors
/// Каталог не создаётся или файл не пишется.
pub fn write_raw_answers(
    repo: &Path,
    slug: &str,
    rubric_name: &str,
    target_rel: Option<&str>,
    target_sha: Option<&str>,
    judge_model: &str,
    answers: &[RawAnswerInput],
) -> crate::error::Result<Vec<SampleStamp>> {
    if answers.is_empty() {
        return Ok(Vec::new());
    }
    let dir = raw_dir(repo, slug);
    std::fs::create_dir_all(&dir).map_err(|e| crate::error::HarnessError::io(&dir, e))?;
    let saved_at = chrono::Local::now().to_rfc3339();
    let mut stamps = Vec::with_capacity(answers.len());
    for (idx, answer) in answers.iter().enumerate() {
        let sample = idx + 1;
        let sha256 = crate::hash::sha256_hex(answer.text.as_bytes());
        let record = RawAnswer {
            schema: RUBRIC_RAW_SCHEMA.to_string(),
            rubric: rubric_name.to_string(),
            target: target_rel.map(str::to_string),
            target_sha256: target_sha.map(str::to_string),
            sample,
            judge_model: judge_model.to_string(),
            sha256: sha256.clone(),
            dropped: answer.dropped,
            text: answer.text.clone(),
            saved_at: saved_at.clone(),
        };
        let path = dir.join(raw_file_name(sample));
        let text = serde_json::to_string_pretty(&record).map_err(|e| {
            crate::error::HarnessError::Config(format!("сериализация сырого ответа судьи: {e}"))
        })?;
        std::fs::write(&path, text).map_err(|e| crate::error::HarnessError::io(&path, e))?;
        stamps.push(SampleStamp {
            sha256,
            dropped: answer.dropped,
        });
    }
    Ok(stamps)
}

/// Сырые ответы, сохранённые для отчёта (по возрастанию номера сэмпла).
/// Нечитаемый или чужой JSON пропускается — это свидетельство, а не гейт.
#[must_use]
pub fn load_raw_answers(repo: &Path, slug: &str) -> Vec<RawAnswer> {
    let dir = raw_dir(repo, slug);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut out: Vec<RawAnswer> = entries
        .flatten()
        .filter(|e| {
            e.path()
                .extension()
                .is_some_and(|x| x.eq_ignore_ascii_case("json"))
        })
        .filter_map(|e| std::fs::read_to_string(e.path()).ok())
        .filter_map(|t| serde_json::from_str::<RawAnswer>(&t).ok())
        .collect();
    out.sort_by_key(|a| a.sample);
    out
}

/// Slug отчёта: имя файла отчёта, посчитанное от цели так же, как при записи.
#[must_use]
pub fn artifact_slug_of(artifact: &crate::rubric::RubricArtifact) -> String {
    crate::rubric::artifact_slug(artifact.target.as_deref().map(Path::new))
}

/// Итог пересборки отчёта из сырых ответов (`arch-be rubric reverify` и
/// составляющая гейта `decision_quality`).
#[derive(Debug, Default)]
pub struct Reverify {
    /// Сырые ответы найдены (иначе отчёт невоспроизводим в принципе).
    pub raw_saved: bool,
    /// Файлы сырых ответов, чей текст не сходится с записанным хэшем.
    pub tampered: Vec<String>,
    /// Пересобранный из сырых ответов отчёт (`None` — пересобрать нельзя).
    pub rebuilt: Option<crate::rubric::RubricReport>,
    /// Почему пересборка невозможна (рубрика не найдена, документ недоступен,
    /// нет валидных ответов). Это НЕ расхождение: механика не смогла
    /// проверить, а не нашла подлог.
    pub unavailable: Option<String>,
    /// Расхождения записанного отчёта с пересобранным.
    pub differences: Vec<String>,
}

impl Reverify {
    /// Отчёт воспроизводится из своих сырых ответов: сверка прошла, расхождений
    /// нет. Отчёт без сохранённых ответов этот признак НЕ получает — «нечего
    /// проверять» не то же самое, что «проверено».
    #[must_use]
    pub fn reproduced(&self) -> bool {
        self.raw_saved
            && self.tampered.is_empty()
            && self.unavailable.is_none()
            && self.differences.is_empty()
    }
}

/// Пересобирает отчёт из сохранённых сырых ответов тем же [`crate::rubric::build_report`]
/// и сверяет с записанным: баллы по критериям, метки, взвешенный итог, вердикт.
///
/// Дёшево по устройству (разбор JSON и медианы, без LLM) — поэтому это может
/// делать и гейт. Сверка не «удостоверяет качество суждения»: она отвечает
/// ровно на один вопрос — соответствует ли записанный балл ответам, из которых
/// он объявлен собранным.
#[must_use]
pub fn reverify(
    repo: &Path,
    artifact: &crate::rubric::RubricArtifact,
    rubrics_dir: &Path,
    cfg: &crate::config::JudgeConfig,
) -> Reverify {
    let slug = artifact_slug_of(artifact);
    let raw = load_raw_answers(repo, &slug);
    let mut out = Reverify {
        raw_saved: !raw.is_empty(),
        tampered: raw
            .iter()
            .filter(|a| a.tampered())
            .map(|a| raw_file_name(a.sample))
            .collect(),
        ..Reverify::default()
    };
    if !out.raw_saved {
        return out;
    }
    let rebuilt = match rebuild(repo, artifact, &raw, rubrics_dir, cfg) {
        Ok(report) => report,
        Err(reason) => {
            out.unavailable = Some(reason);
            return out;
        }
    };
    out.differences = compare(artifact, &rebuilt);
    out.rebuilt = Some(rebuilt);
    out
}

/// Пересобирает отчёт из сырых ответов; `Err` — причина, по которой это
/// невозможно (не расхождение).
fn rebuild(
    repo: &Path,
    artifact: &crate::rubric::RubricArtifact,
    raw: &[RawAnswer],
    rubrics_dir: &Path,
    cfg: &crate::config::JudgeConfig,
) -> std::result::Result<crate::rubric::RubricReport, String> {
    let rubric_path = resolve_rubric_path(rubrics_dir, &artifact.rubric).ok_or_else(|| {
        format!(
            "рубрика '{}' не найдена в {}",
            artifact.rubric,
            rubrics_dir.display()
        )
    })?;
    let rubric =
        crate::rubric::load(&rubric_path).map_err(|e| format!("рубрика не читается: {e}"))?;
    let target_rel = artifact
        .target
        .as_deref()
        .ok_or_else(|| "в отчёте нет пути документа — текст не восстановить".to_string())?;
    let path = repo.join(target_rel);
    let text = std::fs::read_to_string(&path)
        .map_err(|e| format!("документ {} не читается: {e}", path.display()))?;
    let runs: Vec<_> = raw
        .iter()
        .filter(|a| !a.dropped)
        .filter_map(|a| crate::rubric::parse_judge_response(&a.text).ok())
        .collect();
    if runs.is_empty() {
        return Err("ни один сырой ответ не разобран — пересобрать отчёт не из чего".to_string());
    }
    crate::rubric::build_report(&rubric, &artifact.judge_model, &runs, &text, cfg)
        .map_err(|e| format!("пересборка отчёта: {e}"))
}

/// Путь к рубрике по имени из отчёта: имя в каталоге рубрик (`.yaml`/`.yml`)
/// либо путь как он записан.
fn resolve_rubric_path(dir: &Path, name: &str) -> Option<std::path::PathBuf> {
    let direct = Path::new(name);
    if direct.is_file() {
        return Some(direct.to_path_buf());
    }
    for ext in ["yaml", "yml"] {
        let candidate = dir.join(format!("{name}.{ext}"));
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// Расхождения записанного отчёта с пересобранным — по одному пункту на
/// расхождение, с числами (человеку нужно видеть, что именно правили).
fn compare(
    artifact: &crate::rubric::RubricArtifact,
    rebuilt: &crate::rubric::RubricReport,
) -> Vec<String> {
    let mut out = Vec::new();
    if artifact.judge_model != rebuilt.judge_model {
        out.push(format!(
            "судья: в отчёте '{}', в сырых ответах '{}'",
            artifact.judge_model, rebuilt.judge_model
        ));
    }
    for score in &rebuilt.scores {
        let Some(recorded) = artifact
            .scores
            .iter()
            .find(|s| s.criterion_id == score.criterion_id)
        else {
            out.push(format!(
                "критерий '{}': в отчёте не записан (пересборка дала {})",
                score.criterion_id, score.score
            ));
            continue;
        };
        if recorded.score != score.score {
            out.push(format!(
                "критерий '{}': в отчёте {}, из сырых ответов {}",
                score.criterion_id, recorded.score, score.score
            ));
        }
        let mut recorded_flags: Vec<&str> = recorded
            .flags
            .iter()
            .map(crate::rubric::CriterionFlag::as_str)
            .collect();
        let mut rebuilt_flags: Vec<&str> = score
            .flags
            .iter()
            .map(crate::rubric::CriterionFlag::as_str)
            .collect();
        recorded_flags.sort_unstable();
        rebuilt_flags.sort_unstable();
        if recorded_flags != rebuilt_flags {
            out.push(format!(
                "критерий '{}': метки в отчёте {recorded_flags:?}, из сырых ответов {rebuilt_flags:?}",
                score.criterion_id
            ));
        }
    }
    let unstable = rebuilt
        .scores
        .iter()
        .any(|s| s.has_flag(crate::rubric::CriterionFlag::Unstable));
    if artifact.unstable != unstable {
        out.push(format!(
            "метка unstable: в отчёте {}, из сырых ответов {unstable}",
            artifact.unstable
        ));
    }
    let evidence_not_found = rebuilt
        .scores
        .iter()
        .filter(|s| s.has_flag(crate::rubric::CriterionFlag::EvidenceNotFound))
        .count();
    if artifact.evidence_not_found != evidence_not_found {
        out.push(format!(
            "evidence_not_found: в отчёте {}, из сырых ответов {evidence_not_found}",
            artifact.evidence_not_found
        ));
    }
    if (artifact.weighted_total - rebuilt.weighted_total).abs() > 1e-9 {
        out.push(format!(
            "взвешенный итог: в отчёте {:.4}, из сырых ответов {:.4}",
            artifact.weighted_total, rebuilt.weighted_total
        ));
    }
    if artifact.verdict != rebuilt.verdict {
        out.push(format!(
            "вердикт: в отчёте '{}', из сырых ответов '{}'",
            artifact.verdict, rebuilt.verdict
        ));
    }
    out
}

// ---------------------------------------------------------------------------
// J3: автор документа — из шапки, а не со слов в момент судейства
// ---------------------------------------------------------------------------

/// Приводит метку модели к сравнимому виду: регистр, пробелы и типовые
/// разделители (`Claude-Opus`, `claude opus`, `Claude_Opus` → `claude-opus`).
///
/// Сравнение меток побайтово ошибочно в обе стороны: `Claude-Opus` и
/// `claude-opus` — одна модель, а `opus` и `sonnet` — разные метки одного
/// семейства, то есть судья и автор с общими слепыми зонами.
#[must_use]
pub fn normalize_label(label: &str) -> String {
    let mut out = String::with_capacity(label.len());
    let mut last_dash = true;
    for ch in label.trim().chars() {
        if ch.is_alphanumeric() {
            out.extend(ch.to_lowercase());
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    out.trim_matches('-').to_string()
}

/// Совпадают ли две метки после нормализации.
#[must_use]
pub fn same_label(left: &str, right: &str) -> bool {
    normalize_label(left) == normalize_label(right)
}

/// Источник метки автора в отчёте: значение поля `author_source`.
pub const AUTHOR_SOURCE_HEADER: &str = "header";
/// Источник метки автора: аргумент вызова (в документе поля нет).
pub const AUTHOR_SOURCE_ARGUMENT: &str = "argument";
/// Источник метки автора: автора нет ни в документе, ни в вызове.
pub const AUTHOR_SOURCE_NONE: &str = "none";

/// Кем выбран автор документа и откуда взята метка (J3, ADR-048).
#[derive(Debug, Clone, Default)]
pub struct AuthorChoice {
    /// Метка автора, которая идёт в отчёт.
    pub author: Option<String>,
    /// Источник метки: [`AUTHOR_SOURCE_HEADER`], [`AUTHOR_SOURCE_ARGUMENT`]
    /// или [`AUTHOR_SOURCE_NONE`].
    pub source: String,
    /// Метка, переданная вызовом, если она разошлась с шапкой (иначе `None`).
    pub declared: Option<String>,
}

impl AuthorChoice {
    /// Метка автора пришла из шапки документа.
    #[must_use]
    pub fn from_header(&self) -> bool {
        self.source == AUTHOR_SOURCE_HEADER
    }
}

/// Выбирает автора документа: **значение из шапки документа сильнее**
/// аргумента вызова (J3, ADR-048).
///
/// Почему так: шапка закоммичена вместе с документом, и её правка меняет хэш
/// документа — отчёт от новой редакции становится устаревшим
/// (`rubric_report_stale`). Метка, переданная аргументом, живёт только в
/// вызове: разрешив ей перекрывать шапку, автор «вспоминался бы задним
/// числом» — то есть ровно то, от чего происхождение и защищает.
/// Расхождение аргумента с шапкой не отбрасывается, а называется.
#[must_use]
pub fn choose_author(header: Option<String>, declared: Option<String>) -> AuthorChoice {
    let header = header.filter(|a| !a.trim().is_empty());
    let declared = declared.filter(|a| !a.trim().is_empty());
    match (header, declared) {
        (Some(header), Some(declared)) => {
            let mismatch = !same_label(&header, &declared);
            AuthorChoice {
                author: Some(header),
                source: AUTHOR_SOURCE_HEADER.to_string(),
                declared: mismatch.then_some(declared),
            }
        }
        (Some(header), None) => AuthorChoice {
            author: Some(header),
            source: AUTHOR_SOURCE_HEADER.to_string(),
            declared: None,
        },
        (None, Some(declared)) => AuthorChoice {
            author: Some(declared),
            source: AUTHOR_SOURCE_ARGUMENT.to_string(),
            declared: None,
        },
        (None, None) => AuthorChoice {
            author: None,
            source: AUTHOR_SOURCE_NONE.to_string(),
            declared: None,
        },
    }
}

/// Человек ли автор документа: `human` или `human:<имя>` (J3, ADR-048).
/// Документ, написанный человеком, отличен от любой судьи-модели — это не
/// «независимость подтверждена», а факт другого рода автора.
#[must_use]
pub fn is_human_author(label: &str) -> bool {
    let normalized = normalize_label(label);
    normalized == "human" || normalized.starts_with("human-") || normalized.starts_with("human:")
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

    /// Метки сравниваются после нормализации: регистр, пробелы и разделители
    /// не делают одну модель двумя, а разные модели одного семейства —
    /// независимыми судьями (J3/J4, ADR-048).
    #[test]
    fn labels_are_normalized_before_comparison() {
        assert_eq!(normalize_label("Claude-Opus"), "claude-opus");
        assert_eq!(normalize_label("  claude  opus "), "claude-opus");
        assert_eq!(normalize_label("GLM_5.2"), "glm-5-2");
        assert!(same_label("Claude-Opus", "claude opus"));
        assert!(!same_label("claude-opus", "claude-sonnet"));
    }

    /// Автор документа — из шапки: значение из документа сильнее аргумента
    /// вызова, а расхождение не отбрасывается, а называется (J3, ADR-048).
    #[test]
    fn author_choice_prefers_document_header() {
        let from_header = choose_author(Some("claude-opus-4".into()), None);
        assert_eq!(from_header.author.as_deref(), Some("claude-opus-4"));
        assert_eq!(from_header.source, AUTHOR_SOURCE_HEADER);
        assert!(from_header.declared.is_none());

        let from_argument = choose_author(None, Some("glm-5.2".into()));
        assert_eq!(from_argument.source, AUTHOR_SOURCE_ARGUMENT);
        assert_eq!(from_argument.author.as_deref(), Some("glm-5.2"));

        // Одна и та же модель, названная по-разному, расхождением не считается.
        let same = choose_author(Some("Claude-Opus".into()), Some("claude opus".into()));
        assert!(same.declared.is_none(), "{same:?}");
        assert_eq!(same.author.as_deref(), Some("Claude-Opus"));

        // Разные метки: в отчёт идёт шапка, расхождение названо.
        let mismatch = choose_author(Some("claude-opus-4".into()), Some("glm-5.2".into()));
        assert_eq!(mismatch.author.as_deref(), Some("claude-opus-4"));
        assert!(mismatch.from_header());
        assert_eq!(mismatch.declared.as_deref(), Some("glm-5.2"));

        let none = choose_author(None, None);
        assert_eq!(none.source, AUTHOR_SOURCE_NONE);
        assert!(none.author.is_none());
    }

    /// `human` и `human:<имя>` — автор-человек: он отличен от любой
    /// судьи-модели, и это факт другого рода, а не «независимость» (J3).
    #[test]
    fn human_author_is_recognized() {
        assert!(is_human_author("human"));
        assert!(is_human_author("HUMAN"));
        assert!(is_human_author("human:Иван Петров"));
        assert!(!is_human_author("claude-opus-4"));
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
