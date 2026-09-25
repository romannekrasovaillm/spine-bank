//! E8.2: кэш вердикта судьи по хэшу досье и версии модели.
//!
//! Повторное ревью неизменённого кода не должно стоить вызова модели: вердикт —
//! чистая функция от того, что судье показали (досье), чем судили (модель) и по
//! каким правилам (рубрика и настройки `[judge]`). Ключ кэша — хэш ровно этих
//! четырёх вещей, поэтому правка кода, смена модели, правка рубрики или порогов
//! промахивается мимо записи, а не отдаёт старый вердикт.
//!
//! Что кэш НЕ делает:
//!
//! - не подменяет происхождение: запись кэша видна в отчёте
//!   (`provenance.cache.key` / `judged_at` / `hits`), и человек читает «вердикт
//!   снят 2026-09-25, повторно использован 3 раза», а не «судья отработал»;
//! - не кэширует маршрут `declared` (split-judge через MCP): там платит хост, и
//!   вызов модели делает он же — экономить Spine'у нечего;
//! - не живёт вечно: файл ограничен [`MAX_ENTRIES`] записями (вытесняются самые
//!   старые по времени оценки), испорченный файл читается как пустой кэш, а не
//!   роняет прогон.
//!
//! Сырые ответы судьи лежат в записи вместе с отчётом: повторное использование
//! не отменяет `rubric reverify` — отчёт остаётся проверяемым из того, из чего
//! он собран.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::rubric::JudgeConfigSnapshot;
use crate::rubric::RubricReport;

/// Версия формата кэша: несовпадение — записи не читаются (лучше промах, чем
/// вердикт, собранный по другой схеме).
pub const CACHE_VERSION: u32 = 1;

/// Потолок записей в файле кэша (лишние вытесняются по времени оценки).
pub const MAX_ENTRIES: usize = 200;

/// Версия промптов судьи — часть ключа: правка формулировок меняет то, что
/// видит модель, и старые вердикты к ней не относятся. Поднимать вместе с
/// правкой `judge_system_prompt`/`judge_user_prompt`.
pub const PROMPT_VERSION: u32 = 1;

/// Отметка попадания в кэш: чем именно воспользовались и когда вердикт снят.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheHit {
    /// Ключ записи (хэш досье + модели + рубрики + настроек).
    pub key: String,
    /// Когда вердикт был снят моделью (не когда использован).
    pub judged_at: String,
    /// Сколько раз вердикт предъявлен (включая текущий).
    pub hits: u64,
}

/// Запись кэша: вердикт и то, из чего он собран.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheEntry {
    /// Версия формата ([`CACHE_VERSION`]).
    #[serde(default)]
    pub version: u32,
    /// Ключ (дублирует ключ словаря — для чтения файла глазами).
    pub key: String,
    /// Имя рубрики.
    pub rubric: String,
    /// Модель-судья.
    pub model: String,
    /// Когда вердикт снят.
    pub judged_at: String,
    /// Сколько раз запись использована.
    #[serde(default)]
    pub hits: u64,
    /// Отчёт судьи целиком.
    pub report: RubricReport,
    /// Сырые ответы судьи: без них отчёт нечем сверить (`rubric reverify`).
    #[serde(default)]
    pub raw: Vec<String>,
}

/// Файл кэша: словарь «ключ → запись».
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct JudgeCache {
    /// Записи по ключу.
    #[serde(default)]
    pub entries: BTreeMap<String, CacheEntry>,
}

impl JudgeCache {
    /// Читает кэш. Отсутствующий, испорченный или чужой по версии файл — пустой
    /// кэш: промах дешевле падения прогона и безопаснее старого вердикта.
    #[must_use]
    pub fn load(path: &Path) -> Self {
        let Ok(text) = std::fs::read_to_string(path) else {
            return Self::default();
        };
        let Ok(cache) = serde_json::from_str::<Self>(&text) else {
            return Self::default();
        };
        let mut cache = cache;
        cache
            .entries
            .retain(|_, entry| entry.version == CACHE_VERSION);
        cache
    }

    /// Записывает кэш, вытесняя лишнее. Ошибка записи — не ошибка ревью:
    /// кэш ускоряет, а не решает.
    ///
    /// # Errors
    ///
    /// Ошибка создания каталога или записи файла.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, serde_json::to_vec_pretty(self)?)
    }

    /// Запись по ключу.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&CacheEntry> {
        self.entries.get(key)
    }

    /// Кладёт запись и вытесняет самые старые по времени оценки сверх
    /// [`MAX_ENTRIES`]. Повторная запись того же ключа (например, после
    /// `--no-cache` прогона) заменяет старую и сохраняет счётчик попаданий.
    pub fn put(&mut self, entry: CacheEntry) {
        let hits = self
            .entries
            .get(&entry.key)
            .map_or(entry.hits, |old| old.hits.max(entry.hits));
        let mut entry = entry;
        entry.hits = hits;
        self.entries.insert(entry.key.clone(), entry);
        while self.entries.len() > MAX_ENTRIES {
            let Some(oldest) = self
                .entries
                .values()
                .min_by(|a, b| a.judged_at.cmp(&b.judged_at))
                .map(|e| e.key.clone())
            else {
                break;
            };
            self.entries.remove(&oldest);
        }
    }

    /// Отмечает использование записи: счётчик попаданий и возврат отметки для
    /// отчёта. Отметка описывает вердикт ДО инкремента (`hits` — сколько раз
    /// вердикт предъявлен включая текущий).
    pub fn touch(&mut self, key: &str) -> Option<CacheHit> {
        let entry = self.entries.get_mut(key)?;
        entry.hits += 1;
        Some(CacheHit {
            key: key.to_string(),
            judged_at: entry.judged_at.clone(),
            hits: entry.hits,
        })
    }
}

/// Ключ кэша: хэш рубрики, модели, оцениваемого текста и настроек судьи.
/// Всё, что меняет ответ модели, входит в ключ — иначе кэш отдавал бы вердикт
/// о другом коде или по другим правилам.
#[must_use]
pub fn key_for(
    rubric: &crate::rubric::Rubric,
    model: &str,
    input_sha256: &str,
    snapshot: &JudgeConfigSnapshot,
) -> String {
    let rubric_json = serde_json::to_string(rubric).unwrap_or_default();
    let config = format!(
        "v{PROMPT_VERSION}|samples={}|unstable={}|evidence={}|adaptive={}",
        snapshot.samples,
        snapshot.unstable_stdev,
        snapshot.evidence_min_similarity,
        snapshot.adaptive_samples,
    );
    let mut parts = vec![
        format!("cache={CACHE_VERSION}"),
        rubric_json,
        model.to_string(),
        input_sha256.to_string(),
        config,
    ];
    parts.insert(0, "judge-cache".to_string());
    crate::hash::sha256_hex(parts.join("\u{1f}").as_bytes())
}

/// Файл кэша по умолчанию: `$ARCH_HOME/state/judge-cache.json`.
#[must_use]
pub fn default_cache_path() -> PathBuf {
    crate::config::Config::home_dir()
        .join("state")
        .join("judge-cache.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rubric() -> crate::rubric::Rubric {
        crate::rubric::Rubric {
            name: "adr-quality".to_string(),
            description: "Качество ADR".to_string(),
            scale_max: 5,
            criteria: vec![crate::rubric::Criterion {
                id: "context".to_string(),
                name: "Контекст".to_string(),
                description: "Описан контекст".to_string(),
                weight: 1.0,
                anchors: BTreeMap::new(),
                evidence_on: crate::rubric::EvidenceOn::High,
                evidence_roles: Vec::new(),
                coverage: None,
                blocking: false,
            }],
            origin: "anchor".to_string(),
            pack: None,
        }
    }

    fn snapshot(samples: usize) -> JudgeConfigSnapshot {
        JudgeConfigSnapshot {
            samples,
            unstable_stdev: 1.0,
            evidence_min_similarity: 0.8,
            adaptive_samples: false,
        }
    }

    fn report() -> RubricReport {
        RubricReport {
            rubric_name: "adr-quality".to_string(),
            judge_model: "judge-1".to_string(),
            judge_samples: 1,
            scores: vec![crate::rubric::CriterionScore {
                criterion_id: "context".to_string(),
                weight: 1.0,
                score: 5,
                rationale: "Цитата: \"контекст\".".to_string(),
                samples: vec![5],
                stdev: 0.0,
                flags: Vec::new(),
                evidence_unconfirmed_ratio: 0.0,
                invalid_samples: 0,
                checked: Vec::new(),
            }],
            weighted_total: 5.0,
            verdict: "v".to_string(),
            evidence_unconfirmed_ratio: 0.0,
            input_injections: Vec::new(),
            invalid_samples_ratio: 0.0,
            decision: Some(crate::rubric::RubricDecision::Pass),
            decision_reasons: Vec::new(),
        }
    }

    fn entry(key: &str, judged_at: &str) -> CacheEntry {
        CacheEntry {
            version: CACHE_VERSION,
            key: key.to_string(),
            rubric: "adr-quality".to_string(),
            model: "judge-1".to_string(),
            judged_at: judged_at.to_string(),
            hits: 0,
            report: report(),
            raw: vec!["сырой ответ".to_string()],
        }
    }

    #[test]
    fn key_covers_model_rubric_input_and_settings() {
        let base_rubric = rubric();
        let base = key_for(&base_rubric, "judge-1", "pack-hash", &snapshot(3));
        assert_eq!(
            base,
            key_for(&base_rubric, "judge-1", "pack-hash", &snapshot(3))
        );
        assert_ne!(
            base,
            key_for(&base_rubric, "judge-2", "pack-hash", &snapshot(3))
        );
        assert_ne!(
            base,
            key_for(&base_rubric, "judge-1", "other-hash", &snapshot(3))
        );
        assert_ne!(
            base,
            key_for(&base_rubric, "judge-1", "pack-hash", &snapshot(5))
        );
        let mut other = rubric();
        other.criteria[0].weight = 2.0;
        assert_ne!(
            base,
            key_for(&other, "judge-1", "pack-hash", &snapshot(3)),
            "правка рубрики меняет ключ"
        );
    }
    #[test]
    fn put_get_and_hits_survive_roundtrip() {
        let dir = tempfile::tempdir().expect("tmp");
        let path = dir.path().join("judge-cache.json");
        let mut cache = JudgeCache::default();
        cache.put(entry("k1", "20260925-120000"));
        let hit = cache.touch("k1").expect("попадание");
        assert_eq!(hit.hits, 1);
        cache.save(&path).expect("запись кэша");
        let back = JudgeCache::load(&path);
        assert_eq!(
            back.get("k1").expect("запись").raw,
            vec!["сырой ответ".to_string()]
        );
        assert_eq!(back.get("k1").expect("запись").hits, 1);
    }

    #[test]
    fn stale_and_foreign_records_are_dropped() {
        let dir = tempfile::tempdir().expect("tmp");
        let path = dir.path().join("judge-cache.json");
        let mut cache = JudgeCache::default();
        cache.put(entry("k1", "20260925-120000"));
        cache.save(&path).expect("запись кэша");
        // Чужая версия схемы — запись не читается.
        let text = std::fs::read_to_string(&path).expect("чтение");
        let text = text.replace(&format!("\"version\": {CACHE_VERSION}"), "\"version\": 99");
        std::fs::write(&path, text).expect("правка версии");
        assert!(JudgeCache::load(&path).entries.is_empty());
    }

    #[test]
    fn corrupted_cache_reads_as_empty_instead_of_failing() {
        let dir = tempfile::tempdir().expect("tmp");
        let path = dir.path().join("judge-cache.json");
        std::fs::write(&path, "{ это не json").expect("мусор");
        assert!(JudgeCache::load(&path).entries.is_empty());
        assert!(
            JudgeCache::load(&dir.path().join("нет-файла.json"))
                .entries
                .is_empty()
        );
    }

    #[test]
    fn oldest_entries_are_evicted_over_the_limit() {
        let mut cache = JudgeCache::default();
        for i in 0..MAX_ENTRIES + 3 {
            cache.put(entry(&format!("k{i}"), &format!("20260925-12{i:04}")));
        }
        assert_eq!(cache.entries.len(), MAX_ENTRIES);
        assert!(cache.get("k0").is_none(), "самая старая запись вытеснена");
        assert!(cache.get(&format!("k{}", MAX_ENTRIES + 2)).is_some());
    }
}
