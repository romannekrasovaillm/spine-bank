//! Ресурсы среды, требуемые правилами реестра (поле `requires:` правила,
//! ADR-046 «Stand-bound правила гейта»).
//!
//! Правило может объявить ресурсы, без которых его проверка бессмысленна или
//! невозможна: `cuda` (GPU-инференс) или `stand` (стенд GB10/Spark/Grace —
//! тяжёлые прогоны). Если требуемый ресурс НЕдоступен, правило даёт **SKIP с
//! причиной** (не PASS и не FAIL): «зелёный» на неполном окружении был бы
//! молчаливой ложью, а «красный» — наказанием за отсутствие железа. Ресурс
//! доступен — обычная семантика PASS/FAIL: SKIP не лазейка.
//!
//! Детект детерминированный, без новых крейтов:
//! - `cuda` — существует `/dev/nvidia0` ИЛИ `nvidia-smi -L` перечисляет хотя
//!   бы одно устройство;
//! - `stand` — hostname содержит `gb10`/`spark`/`grace` (детект из ADR-041
//!   Am.2) ИЛИ существует `~/gb10-shared/.locks`.

use std::path::Path;
use std::process::{Command, Stdio};

use crate::error::{HarnessError, Result};

/// Имя ресурса GPU (`/dev/nvidia0` либо `nvidia-smi` с устройством).
pub const CUDA: &str = "cuda";
/// Имя ресурса стенда (hostname `gb10`/`spark`/`grace` либо `~/gb10-shared/.locks`).
pub const STAND: &str = "stand";

/// Известные реестру ресурсы `requires` (валидация схемы): неизвестное имя —
/// ошибка реестра, а не молчаливый SKIP.
pub const KNOWN: [&str; 2] = [CUDA, STAND];

/// Обязательная строка вывода гейта при ресурсном SKIP: прогон правила,
/// привязанного к стенду/GPU, остаётся за GB10.
pub const STAND_RUN_LINE: &str = "требуется прогон на GB10";

/// Снимок доступности ресурсов среды — один на прогон реестра (детект не
/// повторяется на каждое правило и не зависит от порядка обхода).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AvailableResources {
    /// GPU (`cuda`) доступен.
    cuda: bool,
    /// Стенд (`stand`) доступен.
    stand: bool,
}

impl AvailableResources {
    /// Явный снимок (края и тесты).
    #[must_use]
    pub const fn new(cuda: bool, stand: bool) -> Self {
        Self { cuda, stand }
    }

    /// Все ресурсы доступны — пропускать нечего.
    #[must_use]
    pub const fn all() -> Self {
        Self::new(true, true)
    }

    /// Ни один ресурс не доступен.
    #[must_use]
    pub const fn none() -> Self {
        Self::new(false, false)
    }

    /// Ресурс доступен в этом снимке (неизвестное имя — `false`; схема уже
    /// провалидирована при разборе реестра).
    #[must_use]
    pub fn has(self, resource: &str) -> bool {
        match resource {
            CUDA => self.cuda,
            STAND => self.stand,
            _ => false,
        }
    }

    /// Недоступные ресурсы из списка `requires` (порядок объявления сохранён,
    /// дубли убраны). Пусто — правило исполняется.
    #[must_use]
    pub fn missing(self, requires: &[String]) -> Vec<String> {
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
    /// Детерминированный дефолт библиотеки без края: детект среды. `requires`
    /// по природе машинозависим (в этом его смысл), в отличие от политики
    /// доверия `command_succeeds` — здесь снимок ОБЯЗАН отражать окружение.
    fn default() -> Self {
        detect()
    }
}

/// Детект доступности ресурсов текущей среды.
#[must_use]
pub fn detect() -> AvailableResources {
    AvailableResources::new(detect_cuda(), detect_stand())
}

/// GPU доступен: `/dev/nvidia0` существует ИЛИ `nvidia-smi` вернул список
/// устройств.
#[must_use]
pub fn detect_cuda() -> bool {
    if Path::new("/dev/nvidia0").exists() {
        return true;
    }
    match Command::new("nvidia-smi")
        .arg("-L")
        .stdin(Stdio::null())
        .output()
    {
        Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout)
            .lines()
            .any(|l| l.trim_start().starts_with("GPU ")),
        _ => false,
    }
}

/// Стенд доступен: hostname содержит `gb10`/`spark`/`grace` ИЛИ существует
/// `~/gb10-shared/.locks`.
#[must_use]
pub fn detect_stand() -> bool {
    if let Some(host) = hostname() {
        let host = host.to_ascii_lowercase();
        if host.contains("gb10") || host.contains("spark") || host.contains("grace") {
            return true;
        }
    }
    home_lock_present()
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

/// Наличие маркера общего стенда `~/gb10-shared/.locks` (каталог/файл).
fn home_lock_present() -> bool {
    std::env::var_os("HOME")
        .is_some_and(|home| Path::new(&home).join("gb10-shared/.locks").exists())
}

/// Валидация схемы `requires` правила (ADR-046): неизвестный ресурс — ошибка
/// реестра (опечатка `cudа` молча превратила бы правило в вечный SKIP).
///
/// # Errors
/// В `requires` есть имя вне [`KNOWN`].
pub fn validate(rule: &str, requires: &[String]) -> Result<()> {
    if let Some(unknown) = requires.iter().find(|r| !KNOWN.contains(&r.as_str())) {
        return Err(HarnessError::Control(format!(
            "правило '{rule}': неизвестный ресурс requires '{unknown}' \
             (допустимы: {})",
            KNOWN.join(", ")
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_string()).collect()
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
        // Только cuda доступен.
        let gpu_only = AvailableResources::new(true, false);
        assert!(
            gpu_only.missing(&req(&["cuda"])).is_empty(),
            "cuda доступен"
        );
        assert_eq!(gpu_only.missing(&req(&["stand"])), req(&["stand"]));
        // Смешанный список: недоступен только stand.
        assert_eq!(gpu_only.missing(&req(&["cuda", "stand"])), req(&["stand"]));
    }

    /// Дубли в `requires` не дублируют причину.
    #[test]
    fn missing_dedups_repeated_resources() {
        let none = AvailableResources::none();
        assert_eq!(none.missing(&req(&["cuda", "cuda"])), req(&["cuda"]));
    }

    /// `has` знает оба ресурса; неизвестное имя — `false`, не паника.
    #[test]
    fn has_knows_known_resources_only() {
        let all = AvailableResources::all();
        assert!(all.has(CUDA));
        assert!(all.has(STAND));
        assert!(!all.has("quantum"));
        assert!(!AvailableResources::none().has(CUDA));
    }

    /// Схема: известные ресурсы валидны, неизвестный — ошибка с именем правила
    /// и ресурса.
    #[test]
    fn validate_rejects_unknown_resource() {
        assert!(validate("gpu-rule", &req(&["cuda"])).is_ok());
        assert!(validate("stand-rule", &req(&["stand"])).is_ok());
        assert!(validate("plain", &[]).is_ok());
        let err = validate("typo-rule", &req(&["cudа"])).unwrap_err();
        let text = err.to_string();
        assert!(text.contains("typo-rule"), "{text}");
        assert!(text.contains("cudа"), "{text}");
        assert!(text.contains("cuda"), "{text}");
    }

    /// Детект возвращает снимок без паники (значения — от среды).
    #[test]
    fn detect_returns_snapshot() {
        let snap = detect();
        // Проверяем только согласованность: has отражает поля снимка.
        assert_eq!(snap.has(CUDA), snap.missing(&req(&["cuda"])).is_empty());
        assert_eq!(snap.has(STAND), snap.missing(&req(&["stand"])).is_empty());
    }
}
