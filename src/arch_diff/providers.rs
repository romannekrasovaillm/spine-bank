//! Каталог известных провайдеров внешних систем (R3 волны R).
//!
//! Внешняя система в графе «как построено» — не любой `host:port` из URL
//! (это давало шум: B-метрика «внешние precision» падала до ≤0.21), а только
//! узнанный провайдер: `OpenAI`/`Azure OpenAI`, `Vertex AI`/`Gemini`, `AlloyDB`,
//! платёжные шлюзы. Опознание — по конфигурационным ключам
//! (`spring.ai.openai.*`) и по зависимости/переменным окружения
//! (`OPENAI_API_KEY`, `ALLOYDB_CLUSTER_NAME`).
//!
//! Встроенный каталог — [`crate::assets::RECOVER_PROVIDERS`]; пользователь
//! дополняет его файлом `$ARCH_HOME/assets/recover/providers.yaml` (совпавший
//! по `id` провайдер заменяется, новый добавляется). Формат — подмножество
//! YAML, разбираемое уже имеющимся `serde_yaml_ng` (правило 4: без новых
//! зависимостей).
//!
//! Детерминизм (AD-2): каталог — чистые данные, LLM не участвует; порядок
//! провайдеров фиксирован файлом.

use std::path::PathBuf;

use serde::Deserialize;

/// Провайдер: id узла (`sys:<id>`) и правила опознания.
#[derive(Debug, Clone, Deserialize)]
pub struct ProviderDef {
    /// Идентификатор провайдера (он же имя узла `sys:<id>` и его заголовок).
    pub id: String,
    /// Префиксы конфигурационных ключей (dotted, нижний регистр).
    #[serde(default)]
    pub keys: Vec<String>,
    /// Сырые подстроки содержимого (env-переменные, SDK-маркеры), без учёта
    /// регистра.
    #[serde(default)]
    pub needles: Vec<String>,
}

/// Файл каталога (встроенный или пользовательский).
#[derive(Debug, Deserialize)]
struct ProviderFile {
    /// Список провайдеров.
    #[serde(default)]
    providers: Vec<ProviderDef>,
}

/// Каталог провайдеров: встроенный ∪ пользовательский.
#[derive(Debug, Clone, Default)]
pub struct Providers {
    /// Провайдеры в порядке объявления.
    defs: Vec<ProviderDef>,
}

impl Providers {
    /// Встроенный каталог из `assets/recover/providers.yaml`.
    #[must_use]
    pub fn builtin() -> Self {
        let mut p = Self::default();
        p.extend_from_str(crate::assets::RECOVER_PROVIDERS);
        p
    }

    /// Встроенный каталог, дополненный `$ARCH_HOME/assets/recover/
    /// providers.yaml`. Отсутствующий или испорченный пользовательский файл —
    /// молча встроенный каталог (расширяемость не должна ронять прогон).
    #[must_use]
    pub fn load() -> Self {
        let mut p = Self::builtin();
        if let Ok(text) = std::fs::read_to_string(user_path()) {
            p.extend_from_str(&text);
        }
        p
    }

    /// Дополняет каталог: провайдер с тем же `id` заменяется, новый
    /// добавляется. Невалидный YAML — каталог без изменений.
    fn extend_from_str(&mut self, text: &str) {
        let Ok(file) = serde_yaml_ng::from_str::<ProviderFile>(text) else {
            return;
        };
        for def in file.providers {
            match self.defs.iter_mut().find(|d| d.id == def.id) {
                Some(existing) => *existing = def,
                None => self.defs.push(def),
            }
        }
    }

    /// Провайдер и совпавший ключ каталога по конфигурационному ключу (dotted,
    /// регистр не важен): совпадение префикса ключа (`spring.ai.openai`
    /// покрывает `spring.ai.openai.api-key`), при нескольких — самый длинный.
    /// Возвращаемый ключ — основание условия активации ребра (REC, ADR-075).
    #[must_use]
    pub fn match_key(&self, dotted_key: &str) -> Option<(&ProviderDef, &str)> {
        let key = dotted_key.to_ascii_lowercase();
        self.defs
            .iter()
            .filter_map(|d| {
                let matched = d
                    .keys
                    .iter()
                    .filter(|k| key_matches(&key, k))
                    .max_by_key(|k| k.len())?;
                Some((matched.len(), d, matched.as_str()))
            })
            .max_by_key(|(len, _, _)| *len)
            .map(|(_, d, k)| (d, k))
    }

    /// Провайдер и совпавший needle по сырой подстроке строки (регистр не
    /// важен): при нескольких — самый длинный needle (специфичный провайдер
    /// важнее общего). Возвращаемый needle — основание условия активации ребра
    /// (REC, ADR-075).
    #[must_use]
    pub fn match_needle(&self, line: &str) -> Option<(&ProviderDef, &str)> {
        let upper = line.to_ascii_uppercase();
        self.defs
            .iter()
            .filter_map(|d| {
                let matched = d
                    .needles
                    .iter()
                    .filter(|n| upper.contains(&n.to_ascii_uppercase()))
                    .max_by_key(|n| n.len())?;
                Some((matched.len(), d, matched.as_str()))
            })
            .max_by_key(|(len, _, _)| *len)
            .map(|(_, d, n)| (d, n))
    }

    /// Провайдеры каталога (диагностика и тесты).
    #[cfg(test)]
    #[must_use]
    pub fn defs(&self) -> &[ProviderDef] {
        &self.defs
    }
}

/// Путь пользовательского каталога: `$ARCH_HOME/assets/recover/providers.yaml`.
#[must_use]
pub fn user_path() -> PathBuf {
    crate::config::Config::home_dir()
        .join("assets")
        .join("recover")
        .join("providers.yaml")
}

/// `key` — это `prefix` или начинается на `prefix.` (граница по точке, чтобы
/// `spring.ai.openai` не «съел» `spring.ai.openai2`).
fn key_matches(key: &str, prefix: &str) -> bool {
    let prefix = prefix.to_ascii_lowercase();
    key == prefix
        || key
            .strip_prefix(&prefix)
            .is_some_and(|r| r.starts_with('.'))
}

/// Разворачивает вложенный YAML-подобный конфиг в плоские dotted-ключи с
/// номерами строк (1-based). Комментарии и пустые строки пропускаются;
/// отступ задаёт иерархию. Ключ — всё до первого `:`, за которым пробел или
/// конец строки (так `- name: server` из списка не даёт ключа: `- name`).
#[must_use]
pub fn flatten_keys(content: &str) -> Vec<(String, usize)> {
    let mut out = Vec::new();
    let mut stack: Vec<(usize, String)> = Vec::new(); // (indent, key)
    for (idx, raw) in content.lines().enumerate() {
        let line = strip_comment(raw);
        if line.trim().is_empty() {
            continue;
        }
        let indent = line.len() - line.trim_start().len();
        let trimmed = line.trim_start();
        let Some((key, value)) = split_key(trimmed) else {
            continue;
        };
        if key.is_empty() || key.contains(char::is_whitespace) {
            continue;
        }
        while stack.last().is_some_and(|(ind, _)| *ind >= indent) {
            stack.pop();
        }
        let mut dotted = String::new();
        for (_, parent) in &stack {
            dotted.push_str(parent);
            dotted.push('.');
        }
        dotted.push_str(key);
        out.push((dotted.clone(), idx + 1));
        if value.is_empty() {
            stack.push((indent, key.to_string()));
        }
    }
    out
}

/// Срезает комментарий: `#` в начале строки или после пробела (внутри
/// значения `a#b` не трогаем — консервативно).
fn strip_comment(line: &str) -> &str {
    let bytes = line.as_bytes();
    for i in 0..bytes.len() {
        if bytes[i] == b'#' && (i == 0 || bytes[i - 1].is_ascii_whitespace()) {
            return &line[..i];
        }
    }
    line
}

/// Разбирает `key: value`: двоеточие, за которым пробел или конец строки.
fn split_key(trimmed: &str) -> Option<(&str, &str)> {
    let bytes = trimmed.as_bytes();
    for i in 0..bytes.len() {
        if bytes[i] == b':' && (i + 1 == bytes.len() || bytes[i + 1] == b' ') {
            let key = trimmed[..i].trim();
            let value = if i + 1 < bytes.len() {
                trimmed[i + 1..].trim()
            } else {
                ""
            };
            return Some((key, value));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_catalog_has_expected_providers() {
        let p = Providers::builtin();
        let ids: Vec<&str> = p.defs().iter().map(|d| d.id.as_str()).collect();
        for expected in ["openai", "azure-openai", "vertex-ai", "alloydb", "stripe"] {
            assert!(ids.contains(&expected), "{ids:?}");
        }
    }

    #[test]
    fn dotted_key_prefix_matches_longest_provider() {
        let p = Providers::builtin();
        assert_eq!(
            p.match_key("spring.ai.openai.api-key")
                .map(|(d, _)| d.id.as_str()),
            Some("openai")
        );
        assert_eq!(
            p.match_key("spring.ai.azure.openai.api-key")
                .map(|(d, _)| d.id.as_str()),
            Some("azure-openai")
        );
        assert_eq!(
            p.match_key("spring.ai.gemini.project-id")
                .map(|(d, _)| d.id.as_str()),
            Some("vertex-ai")
        );
        assert!(p.match_key("server.port").is_none());
        // Граница по точке: `spring.ai.openai2` — не openai.
        assert!(p.match_key("spring.ai.openai2.key").is_none());
        // Совпавший ключ каталога — основание условия активации (ADR-075).
        assert_eq!(
            p.match_key("spring.ai.openai.api-key").map(|(_, k)| k),
            Some("spring.ai.openai")
        );
    }

    #[test]
    fn needle_matches_env_markers() {
        let p = Providers::builtin();
        assert_eq!(
            p.match_needle("  api-key: ${OPENAI_API_KEY:demo}")
                .map(|(d, _)| d.id.as_str()),
            Some("openai")
        );
        assert_eq!(
            p.match_needle("value: \"ALLOYDB_CLUSTER_NAME\"")
                .map(|(d, _)| d.id.as_str()),
            Some("alloydb")
        );
        assert_eq!(
            p.match_needle("GOOGLE_API_KEY: ${GOOGLE_API_KEY}")
                .map(|(d, _)| d.id.as_str()),
            Some("vertex-ai")
        );
        assert!(p.match_needle("ordinary: value").is_none());
        // Совпавший needle каталога — основание условия активации (ADR-075).
        assert_eq!(
            p.match_needle("value: \"ALLOYDB_CLUSTER_NAME\"")
                .map(|(_, n)| n),
            Some("ALLOYDB_CLUSTER_NAME")
        );
    }

    #[test]
    fn flatten_keys_expands_nested_yaml_with_line_numbers() {
        let content = "# comment\nspring:\n  ai:\n    azure:\n      openai:\n        api-key: ${AZURE_OPENAI_KEY}\n";
        let keys = flatten_keys(content);
        assert!(
            keys.iter()
                .any(|(k, l)| k == "spring.ai.azure.openai" && *l == 5),
            "{keys:?}"
        );
        assert!(
            keys.iter()
                .any(|(k, l)| k == "spring.ai.azure.openai.api-key" && *l == 6),
            "{keys:?}"
        );
        // Комментарий не даёт ключа.
        assert!(!keys.iter().any(|(k, _)| k.contains("comment")), "{keys:?}");
    }

    #[test]
    fn user_overlay_replaces_same_id_and_appends_new() {
        let mut p = Providers::default();
        p.extend_from_str(crate::assets::RECOVER_PROVIDERS);
        let before = p.defs().len();
        p.extend_from_str(
            "version: 1\nproviders:\n  - id: openai\n    keys: [custom.openai]\n  - id: internal-bank\n    needles: [BANK_GATEWAY_URL]\n",
        );
        assert_eq!(p.defs().len(), before + 1, "новый провайдер добавлен");
        let openai = p.defs().iter().find(|d| d.id == "openai").expect("openai");
        assert_eq!(
            openai.keys,
            vec!["custom.openai".to_string()],
            "замена по id"
        );
        assert!(
            p.match_needle("BANK_GATEWAY_URL").is_some(),
            "пользовательский провайдер виден"
        );
    }

    #[test]
    fn corrupted_user_catalog_is_ignored() {
        let mut p = Providers::builtin();
        let before = p.defs().len();
        p.extend_from_str("{ это не yaml providers:");
        assert_eq!(p.defs().len(), before, "мусор не ломает каталог");
    }
}
