//! Объявление кейса (`CASE.yaml`, K1 волны K 0.4.0): маршрут контроля,
//! вес и измеримость.
//!
//! Повод: не всякий кейс кейс-набора — пакет критического маршрута. Кейс
//! внешнего инструмента (`jvm-archunit-gate`) или описи чужого репозитория
//! (`legacy-survey`) по построению не собирается в Critical-пакет, и мерить
//! его мутационным прогоном на Critical — значит мерить сломанное. Такой кейс
//! объявляет свой маршрут здесь, и `redteam` честно меряет его на нём.
//!
//! Формат (все поля необязательны; отсутствие файла — маршрут Critical,
//! как было до 0.4.0):
//!
//! ```yaml
//! route: standard        # fast | standard | critical
//! weight: 1.0            # вес в агрегатах (по умолчанию 1.0)
//! measurable: true       # false — кейс вне контура (причина обязательна)
//! reason: "..."          # почему кейс не измерим
//! ```
//!
//! Fail-closed: невалидный YAML или неизвестный маршрут — ошибка, а не тихий
//! откат к Critical. Иначе опечатка в маршруте молча подняла бы планку и
//! кейс «перестал бы измеряться» без объяснения.

use std::path::Path;

use crate::control::Route;
use crate::error::{HarnessError, Result};

/// Имя файла-объявления кейса.
pub const CASE_MANIFEST: &str = "CASE.yaml";

/// Объявление кейса.
#[derive(Debug, Clone)]
pub struct CaseManifest {
    /// Маршрут контроля, на котором кейс меряется.
    pub route: Route,
    /// Вес кейса в агрегатах (метрика 12). Неотрицательное число.
    pub weight: f64,
    /// Измерим ли кейс мутационным прогоном. `false` — вне контура.
    pub measurable: bool,
    /// Почему кейс не измерим (для `measurable: false`).
    pub reason: Option<String>,
}

impl Default for CaseManifest {
    fn default() -> Self {
        Self {
            route: Route::Critical,
            weight: 1.0,
            measurable: true,
            reason: None,
        }
    }
}

/// Сырой YAML: каждое поле необязательно (толерантное чтение).
#[derive(Debug, serde::Deserialize)]
struct CaseRaw {
    #[serde(default)]
    route: Option<String>,
    #[serde(default)]
    weight: Option<f64>,
    #[serde(default)]
    measurable: Option<bool>,
    #[serde(default)]
    reason: Option<String>,
}

impl CaseManifest {
    /// Разбирает текст `CASE.yaml`.
    ///
    /// # Errors
    /// Битый YAML, неизвестный маршрут или отрицательный вес.
    pub fn parse(text: &str) -> Result<Self> {
        let raw: CaseRaw = serde_yaml_ng::from_str(text)
            .map_err(|e| HarnessError::Config(format!("{CASE_MANIFEST}: {e}")))?;
        let route = match raw.route.as_deref() {
            None => Route::Critical,
            Some(s) => s
                .trim()
                .parse::<Route>()
                .map_err(|e| HarnessError::Config(format!("{CASE_MANIFEST}: {e}")))?,
        };
        let weight = raw.weight.unwrap_or(1.0);
        if !weight.is_finite() || weight < 0.0 {
            return Err(HarnessError::Config(format!(
                "{CASE_MANIFEST}: вес — неотрицательное число, получено {weight}"
            )));
        }
        let measurable = raw.measurable.unwrap_or(true);
        let reason = raw
            .reason
            .map(|r| r.trim().to_string())
            .filter(|r| !r.is_empty());
        if !measurable && reason.is_none() {
            return Err(HarnessError::Config(format!(
                "{CASE_MANIFEST}: measurable: false требует причину (поле reason)"
            )));
        }
        Ok(Self {
            route,
            weight,
            measurable,
            reason,
        })
    }

    /// Читает `CASE.yaml` из каталога кейса; файла нет — маршрут по умолчанию.
    ///
    /// # Errors
    /// Файл есть, но не читается или не разбирается.
    pub fn load(case: &Path) -> Result<Self> {
        let path = case.join(CASE_MANIFEST);
        if !path.is_file() {
            return Ok(Self::default());
        }
        let text = std::fs::read_to_string(&path).map_err(|e| HarnessError::io(&path, e))?;
        Self::parse(&text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Нет файла — маршрут по умолчанию Critical (поведение до 0.4.0).
    #[test]
    fn missing_manifest_defaults_to_critical() {
        let tmp = tempfile::tempdir().expect("tmp");
        let m = CaseManifest::load(tmp.path()).expect("load");
        assert_eq!(m.route, Route::Critical);
        assert_eq!(m.weight, 1.0);
        assert!(m.measurable);
        assert!(m.reason.is_none());
    }

    /// Объявленный маршрут и вес читаются.
    #[test]
    fn parses_route_and_weight() {
        let m = CaseManifest::parse("route: standard\nweight: 0.5\n").expect("parse");
        assert_eq!(m.route, Route::Standard);
        assert_eq!(m.weight, 0.5);
        assert!(m.measurable);
    }

    /// Пустой файл — маршрут по умолчанию (все поля необязательны).
    #[test]
    fn empty_text_defaults() {
        let m = CaseManifest::parse("").expect("parse");
        assert_eq!(m.route, Route::Critical);
    }

    /// Неизвестный маршрут — fail-closed: ошибка, а не тихий откат к Critical.
    #[test]
    fn unknown_route_is_an_error() {
        let err = CaseManifest::parse("route: turbo\n").expect_err("неизвестный маршрут");
        assert!(err.to_string().contains("turbo"), "{err}");
    }

    /// `measurable: false` без причины — ошибка (сводка обязана назвать
    /// причину, а не печатать пустое «не измерен»).
    #[test]
    fn not_measurable_requires_reason() {
        let err = CaseManifest::parse("measurable: false\n").expect_err("нет причины");
        assert!(err.to_string().contains("reason"), "{err}");
        let m = CaseManifest::parse("measurable: false\nreason: \"нет CONSTRAINTS.yaml\"\n")
            .expect("с причиной");
        assert!(!m.measurable);
        assert_eq!(m.reason.as_deref(), Some("нет CONSTRAINTS.yaml"));
    }

    /// Отрицательный/нечисловой вес — ошибка.
    #[test]
    fn negative_weight_is_an_error() {
        assert!(CaseManifest::parse("weight: -1\n").is_err());
    }
}
