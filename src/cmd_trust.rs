//! Модель доверия исполняемых правил `command_succeeds` (A3, ADR-053).
//!
//! Правило `command_succeeds` исполняет `bash -c <command>` из
//! `CONSTRAINTS.yaml` проверяемого репозитория. Для своего репозитория это
//! нормально, для чужого — нет (`gate` на PR из форка, MCP-сервер у агента,
//! склонировавшего незнакомый репозиторий, `control check` по кейсу
//! коллеги): исполнение обязано начинаться с решения о доверии, а не с
//! запуска. Аудит путей — `docs/experiments/command-trust.md`.
//!
//! Два механизма (вариант (в) ADR-053):
//!
//! 1. **no-exec** — флаг `--no-exec` у CLI-команд, переменная окружения
//!    [`ENV_NO_EXEC`] (`1` — запретить, явное `0` — разрешить) и дефолт
//!    «вкл» у MCP-сервера. Запрещённые правила не исполняются вообще:
//!    пропуск с причиной [`COMMAND_UNTRUSTED`].
//! 2. **allow-файл** (`~/.arch-harness/trusted.json`) — opt-in TOFU:
//!    `arch-be rules allow` фиксирует SHA-256 канонизированного набора
//!    command-строк реестра. Несовпадение отпечатка при проверке — реестр
//!    менялся после доверия — пропуск [`COMMAND_UNTRUSTED`], пока доверие
//!    не подтвердят повторно. Файла или записи для репозитория нет —
//!    доверие как сегодня (обратная совместимость).
//!
//! no-exec приоритетнее allow-файла: сессионный запрет сильнее постоянного
//! разрешения.
//!
//! Решение — снимок [`ExecPolicy`], строящийся на краях (CLI из флага и
//! окружения, MCP из окружения сервера) и пробрасываемый внутрь как
//! параметр — паттерн снимка `Runner` из A2: библиотека окружение не
//! читает, тесты не мутируют `std::env` (гонки в многопоточном прогоне).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{HarnessError, Result};

/// Переменная окружения запрета исполнения: `1` — запретить, явное `0` —
/// разрешить (у MCP-сервера без значения дефолт — запрет).
pub const ENV_NO_EXEC: &str = "ARCH_NO_EXEC";

/// Маркер-префикс причины пропуска по модели доверия (A3): по нему находку
/// `command_untrusted` видно в текстовом выводе, детали SKIP-составляющей
/// гейта и блоке 3 паспорта вердикта.
pub const COMMAND_UNTRUSTED: &str = "command_untrusted";

/// Версия формата `trusted.json` (инкремент — ломающие изменения формата;
/// чужая версия — ошибка, а не молчаливое неверное прочтение).
const TRUST_FILE_VERSION: u32 = 1;

/// Потолок размера `trusted.json` при чтении (1 МиБ): страховка от
/// поданного по ошибке гигантского/бинарного файла; реальный файл с
/// сотнями репозиториев занимает десятки килобайт.
const MAX_TRUST_FILE_BYTES: u64 = 1024 * 1024;

/// Политика исполнения `command_succeeds`-правил — снимок решения на
/// прогон. `Default` — детерминированный legacy-режим (исполнять,
/// allow-файл не консультируется): библиотечные вызовы без края не зависят
/// от машины (AD-7).
#[derive(Debug, Clone, Default)]
pub struct ExecPolicy {
    /// Исполнение запрещено (флаг `--no-exec`, `ARCH_NO_EXEC=1`, дефолт
    /// MCP-сервера).
    pub no_exec: bool,
    /// Путь к allow-файлу (`trusted.json`); `None` — allow-файл не
    /// консультируется (эквивалент «файла нет» — доверие как сегодня).
    pub trust_file: Option<PathBuf>,
}

impl ExecPolicy {
    /// Политика CLI-края: флаг `--no-exec` ИЛИ переменная [`ENV_NO_EXEC`];
    /// allow-файл — общий ([`default_trust_file`]).
    #[must_use]
    pub fn cli(no_exec_flag: bool) -> Self {
        Self {
            no_exec: no_exec_flag || env_no_exec().unwrap_or(false),
            trust_file: Some(default_trust_file()),
        }
    }

    /// Политика MCP-сервера: no-exec=вкл по умолчанию (сервер обслуживает
    /// агента на чужом репозитории — безопасный дефолт без действий
    /// оператора); снимается явным `ARCH_NO_EXEC=0` в окружении сервера.
    #[must_use]
    pub fn mcp() -> Self {
        Self::mcp_with(std::env::var(ENV_NO_EXEC).ok().as_deref())
    }

    /// [`mcp`] с явным значением переменной — тесты без мутаций окружения.
    #[must_use]
    pub fn mcp_with(value: Option<&str>) -> Self {
        Self {
            no_exec: parse_no_exec(value).unwrap_or(true),
            trust_file: Some(default_trust_file()),
        }
    }
}

/// Причина запрета исполнения.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DenyReason {
    /// Активен no-exec (флаг, переменная, серверный дефолт).
    NoExec,
    /// allow-файл существует, но отпечаток реестра не совпадает с записью:
    /// реестр менялся после подтверждения доверия.
    StaleTrust,
}

/// Решение об исполнении `command_succeeds`-правил прогона.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecDecision {
    /// Исполнять (дефолт CLI на своём репозитории).
    Allow,
    /// Не исполнять: правила уходят в пропуск с причиной
    /// [`COMMAND_UNTRUSTED`].
    Deny(DenyReason),
}

impl ExecDecision {
    /// Запрет ли это.
    #[must_use]
    pub fn is_deny(self) -> bool {
        matches!(self, Self::Deny(_))
    }
}

/// Путь allow-файла по умолчанию: `~/.arch-harness/trusted.json`
/// (`$ARCH_HOME/trusted.json` при выставленном `ARCH_HOME`).
#[must_use]
pub fn default_trust_file() -> PathBuf {
    crate::config::Config::home_dir().join("trusted.json")
}

/// Значение [`ENV_NO_EXEC`] из окружения процесса (только края: CLI и
/// старт MCP-сервера; библиотека окружение не читает).
#[must_use]
fn env_no_exec() -> Option<bool> {
    parse_no_exec(std::env::var(ENV_NO_EXEC).ok().as_deref())
}

/// Разбор значения [`ENV_NO_EXEC`]: `1/true/yes/on` — запрет,
/// `0/false/no/off` — разрешение (регистронезависимо). Прочее и
/// отсутствующее значение — `None` (мнения нет, действует дефолт края).
#[must_use]
fn parse_no_exec(value: Option<&str>) -> Option<bool> {
    match value?.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

/// SHA-256 канонизированного набора command-строк `command_succeeds`-правил
/// реестра (канонизация зафиксирована ADR-053): строки `command:` вербатим
/// (без тримминга — менялся любой байт = менялся реестр), сортировка
/// байтово-лексикографическая, склейка разделителем `\n`, хэш —
/// [`crate::hash::sha256_hex`]. Пустой набор — валидный отпечаток (хэш
/// пустой строки): добавление первого command-правила после такого
/// доверия тоже даёт несовпадение.
#[must_use]
pub fn commands_fingerprint<'a>(commands: impl IntoIterator<Item = &'a str>) -> String {
    let mut cmds: Vec<&str> = commands.into_iter().collect();
    cmds.sort_unstable();
    crate::hash::sha256_hex(cmds.join("\n").as_bytes())
}

/// Запись доверия одного репозитория в allow-файле.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrustEntry {
    /// Отпечаток набора команд ([`commands_fingerprint`]).
    pub sha256: String,
    /// Число command-строк на момент подтверждения (информативно).
    #[serde(default)]
    pub commands: usize,
    /// Когда подтверждено (RFC 3339, локальное время; информативно).
    #[serde(default)]
    pub allowed_at: String,
    /// Метка файла ограничений, из которого взят набор (информативно).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub constraints: Option<String>,
}

/// Allow-файл `trusted.json`: записи доверия по канонизированному пути
/// репозитория.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrustFile {
    /// Версия формата ([`TRUST_FILE_VERSION`]).
    pub version: u32,
    /// `канонизированный путь репозитория → запись доверия`.
    #[serde(default)]
    pub repos: BTreeMap<String, TrustEntry>,
}

impl Default for TrustFile {
    fn default() -> Self {
        Self {
            version: TRUST_FILE_VERSION,
            repos: BTreeMap::new(),
        }
    }
}

/// Ключ записи доверия: канонизированный абсолютный путь репозитория
/// (тот же репозиторий по другому написанию пути — та же запись; перенос
/// каталога — новая запись, старая просто не срабатывает).
#[must_use]
fn repo_key(repo: &Path) -> String {
    repo.canonicalize()
        .unwrap_or_else(|_| repo.to_path_buf())
        .to_string_lossy()
        .into_owned()
}

/// Читает allow-файл: `Ok(None)` — файла нет (доверие как сегодня).
///
/// # Errors
/// Файл есть, но не читается, больше [`MAX_TRUST_FILE_BYTES`], невалидный
/// JSON или чужая версия формата: молчаливая деградация модели доверия в
/// «исполнять» хуже, чем громкая ошибка с адресом (fail-closed).
pub fn load_trust_file(path: &Path) -> Result<Option<TrustFile>> {
    if !path.is_file() {
        return Ok(None);
    }
    let meta = std::fs::metadata(path).map_err(|e| HarnessError::io(path, e))?;
    if meta.len() > MAX_TRUST_FILE_BYTES {
        return Err(HarnessError::Control(format!(
            "{}: allow-файл больше {} МиБ — исправьте или удалите его (доверие не подтвердить)",
            path.display(),
            MAX_TRUST_FILE_BYTES / 1024 / 1024
        )));
    }
    let text = std::fs::read_to_string(path).map_err(|e| HarnessError::io(path, e))?;
    let file: TrustFile = serde_json::from_str(&text).map_err(|e| {
        HarnessError::Control(format!(
            "{}: allow-файл не читается ({e}) — исправьте или удалите его: \
             до этого доверие подтвердить нельзя",
            path.display()
        ))
    })?;
    if file.version != TRUST_FILE_VERSION {
        return Err(HarnessError::Control(format!(
            "{}: версия формата {} не поддерживается (ожидается {}) — \
             удалите файл или обновите arch-be",
            path.display(),
            file.version,
            TRUST_FILE_VERSION
        )));
    }
    Ok(Some(file))
}

/// Подтверждает доверие репозиторию (`arch-be rules allow`): записывает в
/// allow-файл `path` отпечаток набора команд реестра. Записи других
/// репозиториев сохраняются. Файл создаётся с правами 0600 (unix): это не
/// секрет, но часть модели доверия — чужая правка недопустима.
///
/// `constraints_label` — метка файла ограничений (для читаемости записи).
///
/// # Errors
/// Существующий файл не читается ([`load_trust_file`]), каталог не
/// создаётся, файл не пишется.
pub fn record_allow(
    path: &Path,
    repo: &Path,
    fingerprint: &str,
    commands: usize,
    constraints_label: &str,
) -> Result<TrustEntry> {
    let mut file = load_trust_file(path)?.unwrap_or_default();
    let entry = TrustEntry {
        sha256: fingerprint.to_string(),
        commands,
        allowed_at: chrono::Local::now().to_rfc3339(),
        constraints: Some(constraints_label.to_string()),
    };
    file.repos.insert(repo_key(repo), entry.clone());
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| HarnessError::io(parent, e))?;
    }
    let text = serde_json::to_string_pretty(&file)
        .map_err(|e| HarnessError::Control(format!("allow-файл не сериализуется: {e}")))?;
    std::fs::write(path, format!("{text}\n")).map_err(|e| HarnessError::io(path, e))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .map_err(|e| HarnessError::io(path, e))?;
    }
    Ok(entry)
}

/// Вычисляет решение об исполнении для репозитория с отпечатком реестра
/// `fingerprint` ([`commands_fingerprint`]).
///
/// Порядок: no-exec сильнее всего (файл даже не читается); allow-файла или
/// записи для репозитория нет — исполнять (обратная совместимость);
/// запись не совпадает — запрет `StaleTrust`.
///
/// # Errors
/// allow-файл есть, но не читается ([`load_trust_file`]).
pub fn resolve(policy: &ExecPolicy, repo: &Path, fingerprint: &str) -> Result<ExecDecision> {
    if policy.no_exec {
        return Ok(ExecDecision::Deny(DenyReason::NoExec));
    }
    let Some(path) = &policy.trust_file else {
        return Ok(ExecDecision::Allow);
    };
    let Some(file) = load_trust_file(path)? else {
        return Ok(ExecDecision::Allow);
    };
    match file.repos.get(&repo_key(repo)) {
        None => Ok(ExecDecision::Allow),
        Some(entry) if entry.sha256 == fingerprint => Ok(ExecDecision::Allow),
        Some(_) => Ok(ExecDecision::Deny(DenyReason::StaleTrust)),
    }
}

/// Причина пропуска с префиксом [`COMMAND_UNTRUSTED`] и подсказкой, как
/// разрешить исполнение, — для поля `untrusted_skipped` отчёта и детали
/// SKIP-составляющей гейта.
#[must_use]
pub fn deny_reason_text(reason: DenyReason) -> String {
    match reason {
        DenyReason::NoExec => format!(
            "{COMMAND_UNTRUSTED}: исполнение команд реестра запрещено (no-exec: флаг \
             --no-exec, {ENV_NO_EXEC}=1 или дефолт MCP-сервера). Разрешить: снимите \
             --no-exec / задайте {ENV_NO_EXEC}=0 (для MCP — в окружении сервера)"
        ),
        DenyReason::StaleTrust => format!(
            "{COMMAND_UNTRUSTED}: реестр команд менялся после подтверждения доверия \
             (trusted.json). Переподтвердите состав: `arch-be rules allow`"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Разбор переменной: документированные значения, регистронезависимость,
    /// мусор — без мнения.
    #[test]
    fn parse_no_exec_values() {
        assert_eq!(parse_no_exec(Some("1")), Some(true));
        assert_eq!(parse_no_exec(Some("true")), Some(true));
        assert_eq!(parse_no_exec(Some("YES")), Some(true));
        assert_eq!(parse_no_exec(Some("On")), Some(true));
        assert_eq!(parse_no_exec(Some("0")), Some(false));
        assert_eq!(parse_no_exec(Some("false")), Some(false));
        assert_eq!(parse_no_exec(Some("No")), Some(false));
        assert_eq!(parse_no_exec(Some("off")), Some(false));
        assert_eq!(parse_no_exec(Some(" 1 ")), Some(true));
        assert_eq!(parse_no_exec(Some("2")), None);
        assert_eq!(parse_no_exec(Some("")), None);
        assert_eq!(parse_no_exec(None), None);
    }

    /// MCP-дефолт: без значения — запрет; явный `0` — разрешение;
    /// `1` — как дефолт (ADR-053, п. 2 спецификации).
    #[test]
    fn mcp_policy_defaults_to_no_exec() {
        assert!(ExecPolicy::mcp_with(None).no_exec);
        assert!(ExecPolicy::mcp_with(Some("1")).no_exec);
        assert!(!ExecPolicy::mcp_with(Some("0")).no_exec);
        assert!(!ExecPolicy::mcp_with(Some("false")).no_exec);
    }

    /// Канонизация отпечатка: порядок команд не влияет, состав — влияет,
    /// разделитель виден (склейка без `\n` дала бы коллизии соседей).
    #[test]
    fn fingerprint_is_order_independent_and_content_sensitive() {
        let a = commands_fingerprint(["cargo fmt --check", "cargo test"]);
        let b = commands_fingerprint(["cargo test", "cargo fmt --check"]);
        assert_eq!(a, b, "сортировка: порядок правил не влияет");
        let c = commands_fingerprint(["cargo test", "cargo clippy"]);
        assert_ne!(a, c, "смена команды — другой отпечаток");
        let d = commands_fingerprint(["cargo test "]);
        assert_ne!(
            commands_fingerprint(["cargo test"]),
            d,
            "вербатим: хвостовой пробел — тоже изменение реестра"
        );
        assert_eq!(
            commands_fingerprint([] as [&str; 0]),
            crate::hash::sha256_hex(b""),
            "пустой набор — хэш пустой строки"
        );
    }

    /// Запись доверия: файл создаётся (0600 на unix), перечитывается,
    /// записи других репозиториев сохраняются.
    #[test]
    fn record_allow_writes_and_preserves_other_repos() {
        let tmp = tempfile::tempdir().expect("tmp");
        let path = tmp.path().join("nested/trusted.json");
        let repo_a = tmp.path().join("repo-a");
        let repo_b = tmp.path().join("repo-b");
        std::fs::create_dir_all(&repo_a).expect("mkdir");
        std::fs::create_dir_all(&repo_b).expect("mkdir");
        let entry_a = record_allow(&path, &repo_a, "aaa", 2, "CONSTRAINTS.yaml").expect("allow a");
        assert_eq!(entry_a.sha256, "aaa");
        assert_eq!(entry_a.commands, 2);
        assert!(!entry_a.allowed_at.is_empty());
        record_allow(&path, &repo_b, "bbb", 0, ".arch-handoff/CONSTRAINTS.yaml").expect("allow b");
        let file = load_trust_file(&path)
            .expect("читается")
            .expect("файл есть");
        assert_eq!(file.repos.len(), 2, "обе записи на месте");
        assert_eq!(file.repos[&repo_key(&repo_a)].sha256, "aaa");
        assert_eq!(file.repos[&repo_key(&repo_b)].sha256, "bbb");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(&path).expect("stat").permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "allow-файл только для владельца");
        }
    }

    /// Решение: no-exec сильнее всего; без файла/записи — как сегодня;
    /// совпадение — исполнять; несовпадение — `StaleTrust`.
    #[test]
    fn resolve_matrix() {
        let tmp = tempfile::tempdir().expect("tmp");
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).expect("mkdir");
        let trust = tmp.path().join("trusted.json");

        // no-exec бьёт даже совпадающую запись (и не читает файл вовсе).
        record_allow(&trust, &repo, "fp1", 1, "c").expect("allow");
        let policy = ExecPolicy {
            no_exec: true,
            trust_file: Some(trust.clone()),
        };
        assert_eq!(
            resolve(&policy, &repo, "fp1").expect("resolve"),
            ExecDecision::Deny(DenyReason::NoExec)
        );

        // Без no-exec: совпадение — исполнять, несовпадение — StaleTrust.
        let policy = ExecPolicy {
            no_exec: false,
            trust_file: Some(trust.clone()),
        };
        assert_eq!(
            resolve(&policy, &repo, "fp1").expect("resolve"),
            ExecDecision::Allow
        );
        assert_eq!(
            resolve(&policy, &repo, "fp2").expect("resolve"),
            ExecDecision::Deny(DenyReason::StaleTrust)
        );

        // Записи для репозитория нет — доверие как сегодня (TOFU).
        let stranger = tmp.path().join("stranger");
        std::fs::create_dir_all(&stranger).expect("mkdir");
        assert_eq!(
            resolve(&policy, &stranger, "whatever").expect("resolve"),
            ExecDecision::Allow
        );

        // Файла нет — доверие как сегодня; trust_file None — то же.
        let missing = ExecPolicy {
            no_exec: false,
            trust_file: Some(tmp.path().join("no-such.json")),
        };
        assert_eq!(
            resolve(&missing, &repo, "fp2").expect("resolve"),
            ExecDecision::Allow
        );
        assert_eq!(
            resolve(&ExecPolicy::default(), &repo, "fp2").expect("resolve"),
            ExecDecision::Allow
        );
    }

    /// Битый allow-файл — громкая ошибка (fail-closed), а не молчаливое
    /// «исполнять»: файл — часть модели доверия.
    #[test]
    fn corrupt_trust_file_is_loud_error() {
        let tmp = tempfile::tempdir().expect("tmp");
        let path = tmp.path().join("trusted.json");
        std::fs::write(&path, "{битый").expect("write");
        let err = load_trust_file(&path).expect_err("битый JSON — ошибка");
        assert!(err.to_string().contains("не читается"), "{err}");
        std::fs::write(&path, "{\"version\": 99, \"repos\": {}}").expect("write");
        let err = load_trust_file(&path).expect_err("чужая версия — ошибка");
        assert!(err.to_string().contains("версия"), "{err}");
        // Отсутствующий файл — не ошибка.
        assert!(
            load_trust_file(&tmp.path().join("none.json"))
                .expect("ok")
                .is_none()
        );
    }

    /// Тексты причин несут маркер `command_untrusted` и подсказку по
    /// разрешению для каждого случая.
    #[test]
    fn deny_reasons_carry_marker_and_hint() {
        let no_exec = deny_reason_text(DenyReason::NoExec);
        assert!(no_exec.starts_with(COMMAND_UNTRUSTED), "{no_exec}");
        assert!(no_exec.contains("ARCH_NO_EXEC=0"), "{no_exec}");
        let stale = deny_reason_text(DenyReason::StaleTrust);
        assert!(stale.starts_with(COMMAND_UNTRUSTED), "{stale}");
        assert!(stale.contains("rules allow"), "{stale}");
    }
}
