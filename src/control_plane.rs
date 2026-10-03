//! Контрольная плоскость handoff-пакета (A3): файлы, которые решают,
//! **работает ли гейт** и с какими порогами, и их пины в `MANIFEST.json`.
//!
//! Модуль — единый источник перечня и снятия пинов сразу для двух сторон:
//! генератора пакета ([`crate::handoff`] пишет пины) и составляющей гейта
//! `control_plane` ([`crate::gate`] сверяет). Правка перечня в одном месте
//! не должна расходиться с проверкой в другом, поэтому список и типы живут
//! здесь, а не в одном из потребителей.
//!
//! Что входит в периметр и почему — `docs/experiments/control-plane-0.3.12.md`.
//! Здесь нет сети, LLM и внешних процессов: только файловый снимок sha256.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Имя файла метаданных пакета — носителя пинов контрольной плоскости.
pub(crate) const MANIFEST_FILE: &str = "MANIFEST.json";

/// Обязательный минимум контрольной плоскости (A3): пинится ВСЕГДА —
/// существующие файлы хэшем, отсутствующие `null` (появление после выдачи
/// пакета — тоже расхождение). В перечне только то, чья правка исполнителем
/// меняет вердикт гейта или выключает его запуск без правки кода.
pub(crate) const REQUIRED: [&str; 7] = [
    ".claude/settings.json",
    "arch-harness.toml",
    ".arch-handoff/CONSTRAINTS.yaml",
    ".arch-handoff/RUBRIC.yaml",
    ".arch-handoff/TASK.md",
    ".arch-handoff/SPEC.md",
    ".arch-handoff/ROLLBACK.yaml",
];

/// Опциональные файлы контрольной плоскости (A3): пинятся по факту наличия —
/// без них кейс легитимен (git-хуки и CI-блоки появляются после
/// `connect`/`bootstrap`). `.arch-handoff/mcp-calls.jsonl` сюда НЕ входит:
/// журнал вызовов растёт легально и конфигом не является.
pub(crate) const OPTIONAL: [&str; 6] = [
    ".git/hooks/pre-commit",
    ".git/hooks/pre-push",
    ".gitlab-ci.yml",
    ".github/workflows/spine-gate.yml",
    "Jenkinsfile",
    ".arch-handoff/rule-templates.lock",
];

/// Пин одного файла контрольной плоскости: `{"sha256": "<hex>"}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Pin {
    /// SHA-256 содержимого файла на момент выдачи пакета.
    pub sha256: String,
}

/// Путь к `MANIFEST.json` пакета.
pub(crate) fn manifest_path(repo: &Path) -> PathBuf {
    repo.join(crate::handoff::HANDOFF_DIR).join(MANIFEST_FILE)
}

/// Снимает пины контрольной плоскости для `MANIFEST.json` (A3).
///
/// Обязательный минимум пинится всегда (отсутствующие — `null`), опциональные
/// — по факту наличия. Снимок делается ПОСЛЕ записи всех файлов пакета
/// (TASK.md включён), иначе пакет сам себе вечное расхождение.
pub(crate) fn collect(repo: &Path) -> BTreeMap<String, Option<Pin>> {
    let mut pins = BTreeMap::new();
    let optional = OPTIONAL
        .iter()
        .copied()
        .filter(|rel| repo.join(rel).is_file());
    for rel in REQUIRED.iter().copied().chain(optional) {
        let pin = crate::hash::sha256_file(&repo.join(rel)).map(|sha256| Pin { sha256 });
        pins.insert(rel.to_string(), pin);
    }
    pins
}

/// Пины контрольной плоскости, прочитанные из `MANIFEST.json`.
pub(crate) enum Pins {
    /// Пакета нет, либо в нём нет поля `control_plane` / оно пусто —
    /// составляющая гейта уходит в честный SKIP (обратная совместимость
    /// со старыми пакетами и кейсами без handoff-контура).
    Absent,
    /// Пины есть.
    Pinned(BTreeMap<String, Option<Pin>>),
    /// `MANIFEST.json` есть, но не читается/не парсится — это не «пинов нет»,
    /// а сломанный вход: молчать нельзя (иначе подмена файла отключала бы
    /// проверку), поэтому потребитель обязан сказать находкой.
    Invalid(String),
}

/// Читает пины контрольной плоскости из `MANIFEST.json` (A3).
pub(crate) fn read(repo: &Path) -> Pins {
    #[derive(Deserialize)]
    struct ManifestPins {
        #[serde(default)]
        control_plane: BTreeMap<String, Option<Pin>>,
    }
    let path = manifest_path(repo);
    if !path.is_file() {
        return Pins::Absent;
    }
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) => return Pins::Invalid(format!("не читается: {e}")),
    };
    match serde_json::from_str::<ManifestPins>(&text) {
        Ok(m) if m.control_plane.is_empty() => Pins::Absent,
        Ok(m) => Pins::Pinned(m.control_plane),
        Err(e) => Pins::Invalid(e.to_string()),
    }
}
