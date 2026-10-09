//! Ресурсы среды, требуемые правилами реестра (поле `requires:` правила,
//! ADR-046 «Stand-bound правила гейта», Am.3 «настраиваемый реестр зондов»).
//!
//! Правило может объявить ресурсы, без которых его проверка бессмысленна или
//! невозможна: `cuda` (GPU), `stand` (тяжёлый прогон) — или любой ресурс
//! проекта (банковский контур: `oracle-client`, `k8s-cluster`,
//! `license-server`). Если требуемый ресурс НЕдоступен, правило даёт **SKIP с
//! причиной** (не PASS и не FAIL): «зелёный» на неполном окружении был бы
//! молчаливой ложью, а «красный» — наказанием за отсутствие среды. Ресурс
//! доступен — обычная семантика PASS/FAIL: SKIP не лазейка.
//!
//! Как ресурс проверяется — задаётся **зондом**: секцией
//! `[gate.requires.<имя>]` конфига либо встроенным дефолтом ([`builtin`]).
//! Ядро (схема правила, SKIP-плумбинг, вывод гейта) не знает про ML: набор
//! видов зонда [`ProbeKind`] общий, а `cuda`/`stand` — лишь удобство,
//! переопределяемое конфигом. Правило с ресурсом, у которого нет ни зонда,
//! ни встроенного дефолта, — **ошибка реестра** (fail-closed): опечатка не
//! превращается в вечный SKIP.
//!
//! Виды зондов (минимальный полный набор):
//! - `file` — существует файл/каталог `path`;
//! - `binary` — исполняемый `name` находится в `PATH`;
//! - `env` — переменная окружения `var` задана и непуста;
//! - `hostname` — hostname машины совпадает с regex `pattern`;
//! - `device` — существует узел устройства `path`;
//! - `command` — команда `cmd` завершается успехом (через [`crate::proc`],
//!   с таймаутом).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use regex::RegexBuilder;
use serde::{Deserialize, Serialize};

use crate::error::{HarnessError, Result};

/// Имя встроенного ресурса GPU.
pub const CUDA: &str = "cuda";
/// Имя встроенного ресурса стенда.
pub const STAND: &str = "stand";

/// Дефолтная строка вывода гейта для встроенного ресурса стенда/GPU: прогон
/// правила, привязанного к стенду/GPU, остаётся за GB10.
pub const STAND_RUN_LINE: &str = "требуется прогон на GB10";

/// Дефолтный таймаут зонда `command`, секунды.
const COMMAND_TIMEOUT_DEFAULT_SECS: u64 = 10;

/// Вид зонда доступности ресурса — общий набор без предметной специфики.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProbeKind {
    /// Существует файл/каталог `path`.
    File,
    /// Исполняемый `name` находится в `PATH`.
    Binary,
    /// Переменная окружения `var` задана и непуста.
    Env,
    /// Hostname машины совпадает с regex `pattern`.
    Hostname,
    /// Существует узел устройства `path`.
    Device,
    /// Команда `cmd` завершается успехом.
    Command,
}

impl ProbeKind {
    /// Имя вида зонда в конфиге (`kind = "..."`).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::File => "file",
            Self::Binary => "binary",
            Self::Env => "env",
            Self::Hostname => "hostname",
            Self::Device => "device",
            Self::Command => "command",
        }
    }

    /// Имя обязательного параметра вида ([`Self::File`]/[`Self::Device`] —
    /// `path` и т.д.).
    const fn param(self) -> &'static str {
        match self {
            Self::File | Self::Device => "path",
            Self::Binary => "name",
            Self::Env => "var",
            Self::Hostname => "pattern",
            Self::Command => "cmd",
        }
    }
}

/// Конфигурация одного зонда — секция `[gate.requires.<имя>]`
/// (ADR-046 Am.3). Поля по смыслу [`ProbeKind`]: обязательный параметр вида
/// плюс необязательный `note` (текст в перечне SKIP).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RequiresProbeConfig {
    /// Вид зонда; незнакомый — ошибка конфига (serde).
    pub kind: ProbeKind,
    /// Текст в перечне SKIP при недоступности ресурса. `None` — нейтральный
    /// дефолт «требуется среда с ресурсом <имя>».
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// `file`/`device`: путь (с `~`/относительный — от корня репозитория).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// `binary`: имя исполняемого файла.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// `env`: имя переменной окружения.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub var: Option<String>,
    /// `hostname`: regex совпадения hostname (без учёта регистра).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pattern: Option<String>,
    /// `command`: команда (через `bash -c`), успешный код — ресурс доступен.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cmd: Option<String>,
}

impl RequiresProbeConfig {
    /// Проверяет параметры вида и сводит конфиг к зонду. Ошибка — битая
    /// секция `[gate.requires.<имя>]` (про пустой/отсутствующий параметр вида
    /// или невалидный regex).
    ///
    /// # Errors
    /// Не задан обязательный параметр вида; невалидный `pattern`.
    pub fn to_probe(&self, name: &str) -> Result<Probe> {
        let required = |value: &Option<String>| -> Result<String> {
            value
                .as_ref()
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .ok_or_else(|| {
                    HarnessError::Control(format!(
                        "[gate.requires.{name}] kind = \"{}\": не задано поле '{}'",
                        self.kind.as_str(),
                        self.kind.param()
                    ))
                })
        };
        let mut probe = Probe {
            kind: self.kind,
            note: self.note.clone(),
            path: None,
            name: None,
            var: None,
            pattern: None,
            cmd: None,
        };
        match self.kind {
            ProbeKind::File | ProbeKind::Device => probe.path = Some(required(&self.path)?),
            ProbeKind::Binary => probe.name = Some(required(&self.name)?),
            ProbeKind::Env => probe.var = Some(required(&self.var)?),
            ProbeKind::Hostname => {
                let pattern = required(&self.pattern)?;
                RegexBuilder::new(&pattern)
                    .case_insensitive(true)
                    .build()
                    .map_err(|e| {
                        HarnessError::Control(format!(
                            "[gate.requires.{name}] pattern '{pattern}' невалиден: {e}"
                        ))
                    })?;
                probe.pattern = Some(pattern);
            }
            ProbeKind::Command => probe.cmd = Some(required(&self.cmd)?),
        }
        Ok(probe)
    }
}

/// Зонд доступности одного ресурса: как именно проверяется среда.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Probe {
    /// Вид зонда.
    pub kind: ProbeKind,
    /// Текст в перечне SKIP (`None` — нейтральный дефолт по имени ресурса).
    pub note: Option<String>,
    /// Путь (`file`/`device`).
    pub path: Option<String>,
    /// Имя исполняемого файла (`binary`).
    pub name: Option<String>,
    /// Имя переменной окружения (`env`).
    pub var: Option<String>,
    /// Regex hostname (`hostname`).
    pub pattern: Option<String>,
    /// Команда (`command`).
    pub cmd: Option<String>,
}

impl Probe {
    /// Ресурс доступен в среде `repo` (корень репозитория — база относительных
    /// путей и cwd команды).
    #[must_use]
    pub fn available(&self, repo: &Path) -> bool {
        match self.kind {
            ProbeKind::File | ProbeKind::Device => self
                .path
                .as_deref()
                .is_some_and(|p| resolve_path(repo, p).exists()),
            ProbeKind::Binary => self.name.as_deref().is_some_and(binary_on_path),
            ProbeKind::Env => self
                .var
                .as_deref()
                .is_some_and(|var| std::env::var(var).is_ok_and(|v| !v.trim().is_empty())),
            ProbeKind::Hostname => self.pattern.as_deref().is_some_and(|pattern| {
                let Ok(re) = RegexBuilder::new(pattern).case_insensitive(true).build() else {
                    return false;
                };
                hostname().is_some_and(|h| re.is_match(&h))
            }),
            ProbeKind::Command => self.cmd.as_deref().is_some_and(|cmd| {
                crate::proc::run_shell(
                    repo,
                    "bash",
                    cmd,
                    Duration::from_secs(COMMAND_TIMEOUT_DEFAULT_SECS),
                )
                .is_ok_and(|out| out.status.is_some_and(|s| s.success()))
            }),
        }
    }

    /// Текст в перечне SKIP: настроенный `note` либо нейтральный дефолт
    /// «требуется среда с ресурсом <имя>».
    #[must_use]
    pub fn note(&self, resource: &str) -> String {
        self.note
            .clone()
            .unwrap_or_else(|| format!("требуется среда с ресурсом {resource}"))
    }
}

/// Встроенный зонд ресурса `cuda` (удобство, переопределяется конфигом):
/// узел устройства `/dev/nvidia0`.
#[must_use]
pub fn builtin_cuda() -> Probe {
    Probe {
        kind: ProbeKind::Device,
        note: Some(STAND_RUN_LINE.to_string()),
        path: Some("/dev/nvidia0".to_string()),
        name: None,
        var: None,
        pattern: None,
        cmd: None,
    }
}

/// Встроенный зонд ресурса `stand` (удобство, переопределяется конфигом):
/// hostname содержит `gb10`/`spark`/`grace`.
#[must_use]
pub fn builtin_stand() -> Probe {
    Probe {
        kind: ProbeKind::Hostname,
        note: Some(STAND_RUN_LINE.to_string()),
        path: None,
        name: None,
        var: None,
        // Regex чередования: машина стенда — GB10/Spark/Grace (ADR-041 Am.2).
        pattern: Some("gb10|spark|grace".to_string()),
        cmd: None,
    }
}

/// Встроенный набор зондов — дефолты, переопределяемые конфигом.
#[must_use]
pub fn builtin() -> BTreeMap<String, Probe> {
    BTreeMap::from([
        (CUDA.to_string(), builtin_cuda()),
        (STAND.to_string(), builtin_stand()),
    ])
}

/// Реестр зондов: имя ресурса → зонд. Строится из встроенных дефолтов и
/// секций `[gate.requires.<имя>]` конфига (конфиг переопределяет одноимённый
/// встроенный зонд).
#[derive(Debug, Clone)]
pub struct ProbeRegistry {
    probes: BTreeMap<String, Probe>,
}

impl Default for ProbeRegistry {
    /// Библиотечный дефолт — встроенные зонды (`cuda`/`stand`): край заменяет
    /// их реестром из конфига (`[gate.requires.<имя>]`).
    fn default() -> Self {
        Self::new()
    }
}

impl ProbeRegistry {
    /// Реестр встроенных дефолтов без конфига.
    #[must_use]
    pub fn new() -> Self {
        Self { probes: builtin() }
    }

    /// Реестр из конфига: встроенные дефолты, поверх — секции
    /// `[gate.requires.<имя>]` (переопределение и новые ресурсы). Битая секция
    /// — ошибка конфига.
    ///
    /// # Errors
    /// Секция зонда без обязательного параметра вида или с невалидным regex.
    pub fn from_config(config: &BTreeMap<String, RequiresProbeConfig>) -> Result<Self> {
        let mut registry = Self::new();
        for (name, cfg) in config {
            registry.probes.insert(name.clone(), cfg.to_probe(name)?);
        }
        Ok(registry)
    }

    /// Ресурс известен реестру (встроенный либо объявленный конфигом).
    #[must_use]
    pub fn contains(&self, name: &str) -> bool {
        self.probes.contains_key(name)
    }

    /// Имена ресурсов реестра (детерминированный порядок).
    #[must_use]
    pub fn names(&self) -> Vec<&str> {
        self.probes.keys().map(String::as_str).collect()
    }

    /// Текст в перечне SKIP для ресурса: `note` зонда либо нейтральный дефолт.
    /// Неизвестный ресурс — нейтральный дефолт (схема уже провалидирована).
    #[must_use]
    pub fn note(&self, resource: &str) -> String {
        self.probes.get(resource).map_or_else(
            || format!("требуется среда с ресурсом {resource}"),
            |p| p.note(resource),
        )
    }

    /// Валидация схемы `requires` правила (ADR-046 Am.3): ресурс без зонда —
    /// ошибка реестра (fail-closed): опечатка не превращается в вечный SKIP.
    ///
    /// # Errors
    /// В `requires` есть имя, которому не соответствует ни встроенный зонд,
    /// ни секция `[gate.requires.<имя>]`.
    pub fn validate(&self, rule: &str, requires: &[String]) -> Result<()> {
        if let Some(unknown) = requires.iter().find(|r| !self.contains(r)) {
            let known = self.names().join(", ");
            return Err(HarnessError::Control(format!(
                "правило '{rule}': неизвестный ресурс requires '{unknown}' — \
                 объявите зонд [gate.requires.{unknown}] в конфиге \
                 (встроенные ресурсы: {known})"
            )));
        }
        Ok(())
    }

    /// Снимок доступности всех ресурсов реестра (детерминированный, без
    /// повторного детекта на каждое правило).
    #[must_use]
    pub fn snapshot(&self, repo: &Path) -> AvailableResources {
        AvailableResources::detected(
            self.probes
                .iter()
                .map(|(name, probe)| (name.clone(), probe.available(repo)))
                .collect(),
        )
    }
}

/// Снимок доступности ресурсов среды — один на прогон реестра (детект не
/// повторяется на каждое правило и не зависит от порядка обхода).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AvailableResources {
    snapshot: Snapshot,
}

/// Внутреннее представление снимка: явные все/ничего (край, тесты) либо
/// покартовое отображение «имя ресурса → доступен».
#[derive(Debug, Clone, PartialEq, Eq)]
enum Snapshot {
    /// Все ресурсы доступны — пропускать нечего.
    All,
    /// Ни один ресурс не доступен.
    None,
    /// Покартовое отображение (детект среды либо явный набор).
    Map(BTreeMap<String, bool>),
}

impl AvailableResources {
    /// Все ресурсы доступны — пропускать нечего.
    #[must_use]
    pub const fn all() -> Self {
        Self {
            snapshot: Snapshot::All,
        }
    }

    /// Ни один ресурс не доступен.
    #[must_use]
    pub const fn none() -> Self {
        Self {
            snapshot: Snapshot::None,
        }
    }

    /// Явный покартовый снимок (край и тесты).
    #[must_use]
    pub fn detected(map: BTreeMap<String, bool>) -> Self {
        Self {
            snapshot: Snapshot::Map(map),
        }
    }

    /// Ресурс доступен в этом снимке (неизвестное имя — `false`; схема уже
    /// провалидирована при разборе реестра).
    #[must_use]
    pub fn has(&self, resource: &str) -> bool {
        match &self.snapshot {
            Snapshot::All => true,
            Snapshot::None => false,
            Snapshot::Map(map) => map.get(resource).copied().unwrap_or(false),
        }
    }

    /// Недоступные ресурсы из списка `requires` (порядок объявления сохранён,
    /// дубли убраны). Пусто — правило исполняется.
    #[must_use]
    pub fn missing(&self, requires: &[String]) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for resource in requires {
            if !self.has(resource) && !out.iter().any(|r| r == resource) {
                out.push(resource.clone());
            }
        }
        out
    }
}

impl Default for AvailableResources {
    /// Детерминированный дефолт библиотеки без края: детект по встроенному
    /// реестру зондов. `requires` по природе машинозависим (в этом его смысл),
    /// в отличие от политики доверия `command_succeeds` — здесь снимок ОБЯЗАН
    /// отражать окружение.
    fn default() -> Self {
        detect()
    }
}

/// Детект доступности ресурсов текущей среды по встроенному реестру зондов.
#[must_use]
pub fn detect() -> AvailableResources {
    ProbeRegistry::new().snapshot(Path::new("."))
}

/// Hostname машины: `$HOSTNAME`, иначе `/proc/sys/kernel/hostname`, иначе
/// команда `hostname -s` (детерминированный порядок без крейтов).
fn hostname() -> Option<String> {
    if let Ok(h) = std::env::var("HOSTNAME") {
        if !h.trim().is_empty() {
            return Some(h.trim().to_string());
        }
    }
    if let Ok(h) = std::fs::read_to_string("/proc/sys/kernel/hostname") {
        let h = h.trim();
        if !h.is_empty() {
            return Some(h.to_string());
        }
    }
    let out = Command::new("hostname")
        .arg("-s")
        .stdin(Stdio::null())
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
        .filter(|h| !h.is_empty())
}

/// Разрешает путь зонда: `~/...` — от `$HOME`, абсолютный — как есть,
/// относительный — от корня репозитория.
fn resolve_path(repo: &Path, raw: &str) -> PathBuf {
    if raw == "~" {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home);
        }
    }
    if let Some(rest) = raw.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return Path::new(&home).join(rest);
        }
    }
    let path = Path::new(raw);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        repo.join(path)
    }
}

/// Исполняемый файл `name` находится в `PATH`.
fn binary_on_path(name: &str) -> bool {
    let Some(paths) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&paths).any(|dir| {
        let candidate = dir.join(name);
        candidate.is_file() && is_executable(&candidate)
    })
}

/// Файл имеет бит исполнения (Unix); на прочих платформах — наличие файла.
fn is_executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        path.metadata()
            .is_ok_and(|m| m.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_string()).collect()
    }

    fn registry(cfg: &BTreeMap<String, RequiresProbeConfig>) -> ProbeRegistry {
        ProbeRegistry::from_config(cfg).expect("реестр из конфига")
    }

    fn conf(kind: ProbeKind, field: (&str, &str)) -> RequiresProbeConfig {
        let mut cfg = RequiresProbeConfig {
            kind,
            note: None,
            path: None,
            name: None,
            var: None,
            pattern: None,
            cmd: None,
        };
        match field.0 {
            "path" => cfg.path = Some(field.1.to_string()),
            "name" => cfg.name = Some(field.1.to_string()),
            "var" => cfg.var = Some(field.1.to_string()),
            "pattern" => cfg.pattern = Some(field.1.to_string()),
            "cmd" => cfg.cmd = Some(field.1.to_string()),
            _ => unreachable!("неизвестное поле зонда"),
        }
        cfg
    }

    /// Ресурс есть — пропускать нечего; ресурса нет — он в `missing`.
    #[test]
    fn missing_lists_only_unavailable_resources() {
        let all = AvailableResources::all();
        assert!(
            all.missing(&req(&["cuda", "stand"])).is_empty(),
            "все ресурсы доступны"
        );
        let none = AvailableResources::none();
        assert_eq!(none.missing(&req(&["cuda"])), req(&["cuda"]));
        assert_eq!(none.missing(&req(&["stand"])), req(&["stand"]));
    }

    /// Покартовый снимок: только недоступные попадают в `missing`.
    #[test]
    fn detected_snapshot_reports_missing_per_name() {
        let snap = AvailableResources::detected(BTreeMap::from([
            ("cuda".to_string(), true),
            ("stand".to_string(), false),
        ]));
        assert_eq!(snap.missing(&req(&["cuda"])), Vec::<String>::new());
        assert_eq!(snap.missing(&req(&["cuda", "stand"])), req(&["stand"]));
        assert_eq!(
            snap.missing(&req(&["oracle-client"])),
            req(&["oracle-client"])
        );
    }

    /// Дубли в `requires` не дублируют причину.
    #[test]
    fn missing_dedups_repeated_resources() {
        let none = AvailableResources::none();
        assert_eq!(none.missing(&req(&["cuda", "cuda"])), req(&["cuda"]));
    }

    /// `has` знает встроенные ресурсы; неизвестное имя — `false` (all) и
    /// `false` (none).
    #[test]
    fn has_knows_builtin_resources() {
        let all = AvailableResources::all();
        assert!(all.has(CUDA));
        assert!(all.has(STAND));
        assert!(!AvailableResources::none().has(CUDA));
    }

    /// Встроенный реестр содержит cuda и stand; конфиг добавляет произвольный.
    #[test]
    fn registry_merges_builtins_and_config() {
        let mut cfg = BTreeMap::new();
        cfg.insert(
            "oracle-client".to_string(),
            conf(ProbeKind::Binary, ("name", "sqlplus")),
        );
        let reg = registry(&cfg);
        assert!(reg.contains(CUDA));
        assert!(reg.contains(STAND));
        assert!(reg.contains("oracle-client"));
    }

    /// Схема: встроенные ресурсы валидны, неизвестный — ошибка с именем
    /// правила, ресурса и подсказкой объявить `[gate.requires.X]`.
    #[test]
    fn validate_rejects_unknown_resource() {
        let reg = ProbeRegistry::new();
        assert!(reg.validate("gpu-rule", &req(&["cuda"])).is_ok());
        assert!(reg.validate("stand-rule", &req(&["stand"])).is_ok());
        assert!(reg.validate("plain", &[]).is_ok());
        let err = reg.validate("typo-rule", &req(&["cudа"])).unwrap_err();
        let text = err.to_string();
        assert!(text.contains("typo-rule"), "{text}");
        assert!(text.contains("cudа"), "{text}");
        assert!(text.contains("cuda"), "перечень известных: {text}");
        assert!(
            text.contains("[gate.requires.quantum]") || text.contains("gate.requires"),
            "{text}"
        );
    }

    /// Конфиг объявляет неизвестный реестру ресурс — правило валидно.
    #[test]
    fn validate_accepts_config_declared_resource() {
        let mut cfg = BTreeMap::new();
        cfg.insert(
            "oracle-client".to_string(),
            conf(ProbeKind::Binary, ("name", "sqlplus")),
        );
        let reg = registry(&cfg);
        assert!(reg.validate("db-rule", &req(&["oracle-client"])).is_ok());
    }

    /// Секция без обязательного параметра вида — ошибка конфига.
    #[test]
    fn config_without_required_param_is_error() {
        let mut cfg = BTreeMap::new();
        cfg.insert(
            "oracle-client".to_string(),
            RequiresProbeConfig {
                kind: ProbeKind::Binary,
                note: None,
                path: None,
                name: None,
                var: None,
                pattern: None,
                cmd: None,
            },
        );
        let err = ProbeRegistry::from_config(&cfg).unwrap_err();
        let text = err.to_string();
        assert!(text.contains("oracle-client"), "{text}");
        assert!(text.contains("name"), "{text}");
    }

    /// Незнакомый `kind` — ошибка конфига (serde).
    #[test]
    fn unknown_kind_is_config_error() {
        let err = toml::from_str::<RequiresProbeConfig>("kind = \"gpu\"\n").unwrap_err();
        assert!(err.to_string().contains("gpu"), "{err}");
    }

    /// Дефолтный note нейтрален и называет ресурс; настроенный — как задан.
    #[test]
    fn note_defaults_neutral_and_configurable() {
        let mut cfg = BTreeMap::new();
        cfg.insert(
            "oracle-client".to_string(),
            conf(ProbeKind::Binary, ("name", "sqlplus")),
        );
        let reg = registry(&cfg);
        assert_eq!(
            reg.note("oracle-client"),
            "требуется среда с ресурсом oracle-client"
        );
        // Встроенный stand несёт свой note (прогон на GB10).
        assert_eq!(reg.note(STAND), STAND_RUN_LINE);

        cfg.get_mut("oracle-client").unwrap().note = Some("нужен доступ к Ораклу".to_string());
        let reg = registry(&cfg);
        assert_eq!(reg.note("oracle-client"), "нужен доступ к Ораклу");
    }

    /// Зонд `file`: относительный путь — от корня репозитория, `~/` — от HOME.
    #[test]
    fn file_probe_resolves_relative_and_home() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("marker"), b"x").unwrap();
        let file = Probe {
            kind: ProbeKind::File,
            note: None,
            path: Some("marker".to_string()),
            name: None,
            var: None,
            pattern: None,
            cmd: None,
        };
        assert!(file.available(dir.path()), "файл есть — ресурс доступен");
        let absent = Probe {
            path: Some("nope".to_string()),
            ..file
        };
        assert!(!absent.available(dir.path()), "файла нет — недоступен");
    }

    /// Зонд `env`: заданная непустая переменная — доступен, отсутствующая —
    /// нет. `PATH` есть в любом тестовом окружении (без мутаций env).
    #[test]
    fn env_probe_checks_variable() {
        let probe = Probe {
            kind: ProbeKind::Env,
            note: None,
            path: None,
            name: None,
            var: Some("PATH".to_string()),
            pattern: None,
            cmd: None,
        };
        assert!(probe.available(Path::new(".")));
        let absent = Probe {
            var: Some("ARCH_REQUIRES_PROBE_ABSENT_XYZ".to_string()),
            ..probe
        };
        assert!(!absent.available(Path::new(".")));
    }

    /// Детект по встроенному реестру возвращает снимок без паники и
    /// согласован по has/missing.
    #[test]
    fn detect_returns_snapshot() {
        let snap = detect();
        assert_eq!(snap.has(CUDA), snap.missing(&req(&["cuda"])).is_empty());
        assert_eq!(snap.has(STAND), snap.missing(&req(&["stand"])).is_empty());
    }
}
