//! Политика окружения прогона кодового харнесса (C2 волны C 0.3.12).
//!
//! Вынесено из [`crate::harness`] отдельным модулем: продовый `harness.rs`
//! держится под границей `prod_file_length_limit` (C-33), а решение
//! «что наследует процесс исполнителя» — самостоятельный предмет с собственной
//! таблицей приоритетов (проверяется юнит-тестами без запуска процессов).
//!
//! Угроза T4 (утечка секретов через дочерние процессы): по умолчанию процесс
//! кодового агента наследовал ВСЁ окружение Spine-сервера — ключи LLM,
//! `GITHUB_TOKEN`, прокси (RA-9). Строгий дефолт — deny-by-default whitelist —
//! включается для маршрута Critical из пакета и для банковского профиля; явный
//! `env_allow` адаптера и осознанный `env_inherit = true` остаются рычагами.

use std::path::Path;

use crate::config::{CodingHarnessConfig, DEFAULT_ENV_ALLOW};

/// Политика окружения прогона: что наследует процесс исполнителя.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvPlan {
    /// Имена переменных для whitelist (`env_clear` + выборочное наследование);
    /// `None` — полное наследование окружения сервера.
    pub allow: Option<Vec<String>>,
    /// Заметка в итог прогона: применённый строгий whitelist либо
    /// предупреждение о полном наследовании.
    pub note: Option<String>,
}

/// Выбирает политику окружения (C2): строгий дефолт на Critical/bank-profile,
/// обратно совместимое наследование на Fast/Standard, явный `env_allow` и
/// `env_inherit` — как осознанные решения адаптера.
///
/// Приоритет:
/// 1. непустой `env_allow` — узкое решение человека, применяется всегда;
/// 2. `env_inherit = true` — полное наследование с предупреждением;
/// 3. строгий дефолт ([`DEFAULT_ENV_ALLOW`] + имена `env` адаптера) на
///    маршруте Critical или при `bank_profile`;
/// 4. иначе — наследование (поведение до 0.3.12).
#[must_use]
pub fn select(cfg: &CodingHarnessConfig, route: Option<&crate::control::Route>) -> EnvPlan {
    let strict = cfg.bank_profile || route == Some(&crate::control::Route::Critical);
    if !cfg.env_allow.is_empty() {
        return EnvPlan {
            allow: Some(cfg.env_allow.clone()),
            note: None,
        };
    }
    if cfg.env_inherit {
        let why = if strict {
            "маршрут Critical/bank-profile"
        } else {
            "флаг env_inherit"
        };
        return EnvPlan {
            allow: None,
            note: Some(format!(
                "ПРЕДУПРЕЖДЕНИЕ: env_inherit = true ({why}) — процесс исполнителя \
                 наследует ВСЁ окружение сервера: ключи LLM, GITHUB_TOKEN и прочие \
                 секреты доступны исполнителю"
            )),
        };
    }
    if strict {
        let mut allow: Vec<String> = DEFAULT_ENV_ALLOW.iter().map(|s| (*s).to_string()).collect();
        // Явные env-записи адаптера: их имена тоже разрешены к наследованию,
        // значения подставляет `cmd.envs` (существующая семантика).
        for name in cfg.env.keys() {
            if !allow.contains(name) {
                allow.push(name.clone());
            }
        }
        let why = if cfg.bank_profile {
            "bank_profile"
        } else {
            "маршрут Critical"
        };
        return EnvPlan {
            allow: Some(allow),
            note: Some(format!(
                "{why}: окружение исполнителя ограничено whitelist по умолчанию ({}); \
                 полное наследование — только env_inherit = true",
                DEFAULT_ENV_ALLOW.join(", ")
            )),
        };
    }
    EnvPlan {
        allow: None,
        note: None,
    }
}

/// Маршрут значимости из `MANIFEST.json` пакета (`None` — пакета нет,
/// манифест старого формата или значение не разбирается). Тем же полем
/// пользуется пост-гейт A4 для рекомендации таймаута.
#[must_use]
pub fn manifest_route(repo: &Path) -> Option<crate::control::Route> {
    #[derive(serde::Deserialize)]
    struct ManifestRoute {
        #[serde(default)]
        route: Option<String>,
    }
    let text =
        std::fs::read_to_string(repo.join(crate::handoff::HANDOFF_DIR).join("MANIFEST.json"))
            .ok()?;
    let m = serde_json::from_str::<ManifestRoute>(&text).ok()?;
    m.route?.trim().parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::Route;
    use std::collections::BTreeMap;

    fn cfg(env_allow: &[&str], env_inherit: bool, bank_profile: bool) -> CodingHarnessConfig {
        CodingHarnessConfig {
            env_allow: env_allow.iter().map(|s| (*s).into()).collect(),
            env_inherit,
            bank_profile,
            env: BTreeMap::new(),
            ..CodingHarnessConfig::default()
        }
    }

    /// Fast/Standard без явных флагов — наследование, как до 0.3.12.
    #[test]
    fn fast_route_inherits_by_default() {
        let plan = select(&cfg(&[], false, false), Some(&Route::Fast));
        assert_eq!(plan.allow, None);
        assert_eq!(plan.note, None);
        // Critical без bank_profile — строгий whitelist с заметкой.
        let plan = select(&cfg(&[], false, false), Some(&Route::Critical));
        let allow = plan.allow.expect("whitelist на Critical");
        assert_eq!(allow, DEFAULT_ENV_ALLOW.to_vec());
        assert!(plan.note.expect("заметка").contains("маршрут Critical"));
    }

    /// `bank_profile` — строгий дефолт независимо от маршрута.
    #[test]
    fn bank_profile_is_strict_on_any_route() {
        let plan = select(&cfg(&[], false, true), Some(&Route::Fast));
        assert_eq!(plan.allow.expect("whitelist"), DEFAULT_ENV_ALLOW.to_vec());
        assert!(plan.note.expect("заметка").contains("bank_profile"));
    }

    /// Явный `env_allow` приоритетнее и строгого дефолта, и `env_inherit`.
    #[test]
    fn explicit_allow_wins() {
        let plan = select(
            &cfg(&["PATH", "GH_TOKEN"], true, true),
            Some(&Route::Critical),
        );
        assert_eq!(
            plan.allow.expect("явный whitelist"),
            vec!["PATH".to_string(), "GH_TOKEN".to_string()]
        );
        assert_eq!(plan.note, None, "предупреждения о наследовании нет");
    }

    /// `env_inherit = true` — наследование возвращается, но с предупреждением.
    #[test]
    fn env_inherit_inherits_with_warning() {
        let plan = select(&cfg(&[], true, false), Some(&Route::Critical));
        assert_eq!(plan.allow, None);
        assert!(
            plan.note
                .expect("предупреждение")
                .contains("env_inherit = true")
        );
        // На Fast флаг тоже срабатывает (осознанное решение) — но не молчит.
        let plan = select(&cfg(&[], true, false), Some(&Route::Fast));
        assert_eq!(plan.allow, None);
        assert!(
            plan.note
                .expect("предупреждение")
                .contains("env_inherit = true")
        );
    }

    /// Имена env-записей адаптера попадают в whitelist строгого режима.
    #[test]
    fn adapter_env_names_are_whitelisted() {
        let mut c = cfg(&[], false, true);
        c.env.insert("PAYMENTS_TOKEN".into(), "value".into());
        let allow = select(&c, Some(&Route::Standard)).allow.expect("whitelist");
        assert!(allow.contains(&"PAYMENTS_TOKEN".to_string()), "{allow:?}");
    }

    /// Маршрут читается из MANIFEST.json пакета; отсутствие/мусор — None.
    #[test]
    fn manifest_route_reads_package_field() {
        let tmp = tempfile::tempdir().expect("tmp");
        assert_eq!(manifest_route(tmp.path()), None, "пакета нет");
        let handoff = tmp.path().join(crate::handoff::HANDOFF_DIR);
        std::fs::create_dir_all(&handoff).expect("mkdir");
        std::fs::write(handoff.join("MANIFEST.json"), r#"{"route": "critical"}"#).expect("write");
        assert_eq!(manifest_route(tmp.path()), Some(Route::Critical));
        std::fs::write(handoff.join("MANIFEST.json"), r#"{"route": "??? "}"#).expect("write");
        assert_eq!(
            manifest_route(tmp.path()),
            None,
            "неизвестный маршрут — None"
        );
    }
}
