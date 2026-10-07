//! Evidence Bundle — аудиторский след как условие выпуска (AI-Disrupt PDLC):
//! не «отчёт после», а гейт релиза. `pack` собирает манифест с хэшами
//! артефактов, `verify` проверяет полноту по профилю маршрута
//! (Fast/Standard/Critical) и целостность хэшей.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::control::Route;
use crate::error::{HarnessError, Result};
use crate::llm::ToolSpec;
use crate::tool::{Tool, ToolContext, ToolOutput};

/// Алгоритм хэшей новых бандлов: криптостойкий SHA-256 (П2 ДКА).
pub const HASH_ALG_SHA256: &str = "sha256";
/// Алгоритм бандлов старого формата (FNV-1a 64) — только чтение,
/// с предупреждением о необходимости переупаковки.
pub const HASH_ALG_LEGACY: &str = "fnv1a64";

/// Дефолт поля `hash_alg` для бандлов, собранных до П2: старый алгоритм.
fn default_hash_alg() -> String {
    HASH_ALG_LEGACY.to_string()
}

/// Запись манифеста об одном артефакте.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvidenceItem {
    /// Ключ артефакта (spec, adr, spine, `decision_a3`…).
    pub key: String,
    /// Путь относительно каталога изменения.
    pub path: String,
    /// SHA-256 содержимого на момент упаковки (старые бандлы — FNV-1a).
    pub hash: String,
    /// Размер в байтах.
    pub size: u64,
}

/// Манифест Evidence Bundle (`EVIDENCE.yaml`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvidenceBundle {
    /// Маршрут значимости (профиль полноты).
    pub route: String,
    /// Метка времени упаковки.
    pub packed_at: String,
    /// Алгоритм хэшей `items` (П2): `sha256`; отсутствие поля в старом
    /// бандле означает `fnv1a64`.
    #[serde(default = "default_hash_alg")]
    pub hash_alg: String,
    /// Запись значимости (A2): триггеры, источники, маршрут. Из неё выводится
    /// артефакт `risk_level` — рукописный RISK.md не обязателен. Аддитивное
    /// поле: бандлы прежних версий читаются как `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub significance: Option<SignificanceRecord>,
    /// Артефакты.
    pub items: Vec<EvidenceItem>,
}

/// Сработавший триггер значимости с источником (S-1, ADR-034):
/// `declared` / `diff` / `declared+diff`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TriggerRecord {
    /// Каноническое имя триггера.
    pub name: String,
    /// Источник срабатывания.
    pub source: String,
}

/// Запись значимости в `EVIDENCE.yaml` (A2): уровень риска бандла выводится
/// из этой записи, а не из рукописного RISK.md.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignificanceRecord {
    /// Маршрут, заявленный при упаковке (после ROUTE.lock/флагов).
    pub route: String,
    /// Число сработавших триггеров по диффу рабочего дерева.
    pub score: usize,
    /// Сработавшие триггеры с источниками.
    pub triggers: Vec<TriggerRecord>,
    /// HEAD на момент упаковки (`absent` — не git-репозиторий).
    pub head: String,
    /// Метка времени записи.
    pub recorded_at: String,
}

/// Вычисляет запись значимости для упаковки бандла (A2): триггеры и их
/// источники — из механического диффа рабочего дерева (детекторы S-1), маршрут
/// — тот, с которым пакуют. Вне git-репозитория триггеров нет (HEAD «absent»).
#[must_use]
pub fn significance_record(
    dir: &Path,
    route: Route,
    limits: (usize, usize),
    globs: &crate::control::DiffGlobs,
) -> SignificanceRecord {
    let diff = crate::control::detect_diff_triggers_with(dir, None, globs).unwrap_or_default();
    let scored = crate::control::score_with_sources(
        &std::collections::BTreeMap::new(),
        &diff,
        limits.0,
        limits.1,
    );
    let triggers = scored
        .sources
        .iter()
        .map(|(name, src)| TriggerRecord {
            name: name.clone(),
            source: src.label().to_string(),
        })
        .collect();
    SignificanceRecord {
        route: format!("{route:?}"),
        score: scored.significance.score,
        triggers,
        head: current_head(dir).unwrap_or_else(|| "absent".to_string()),
        recorded_at: chrono::Local::now().to_rfc3339(),
    }
}

/// Артефакт выводится машиной (A2) и потому не пишется руками и не
/// заглушается проводником: `risk_level` — из записи значимости в
/// EVIDENCE.yaml, `rollback_rehearsal` — из REHEARSAL.json гейта A4,
/// отчёты прогонов — из записей `arch-be evidence record` (A1).
#[must_use]
pub(crate) fn is_machine_derived(key: &str) -> bool {
    RecordKind::for_artifact(key).is_some() || matches!(key, "risk_level" | "rollback_rehearsal")
}

/// Находка о СОДЕРЖАНИИ артефакта бандла — третий класс исхода наряду с
/// «отсутствует» и «изменён» (Н1 волны A 0.3.4, ADR-041): файл на месте и
/// хэш сходится, но артефакт не написан.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SemanticFinding {
    /// Ключ артефакта (`decision_a3`, `adversarial_review`, …).
    pub key: String,
    /// Код правила: `evidence_stub`, `review_not_ready`, `a3_not_signed`, …
    pub rule: String,
    /// Критичность (`error` блокирует выпуск, `warn` — нет).
    pub severity: String,
    /// Что именно не так.
    pub message: String,
    /// Что сделать.
    pub fix_hint: String,
}

/// Результат проверки.
#[derive(Debug, Clone)]
pub struct EvidenceVerdict {
    /// Полнота, целостность и содержание подтверждены.
    pub passed: bool,
    /// Отсутствующие обязательные артефакты.
    pub missing: Vec<String>,
    /// Артефакты с изменённым хэшем (подмена/дрейф после упаковки).
    pub tampered: Vec<String>,
    /// Предупреждения, не блокирующие выпуск (бандл старого формата и т.п.).
    pub warnings: Vec<String>,
    /// Находки о содержании артефактов (пустышка/не готов/не подписано).
    pub semantics: Vec<SemanticFinding>,
    /// Заявленное, но механикой НЕ проверяемое: Spine не притворяется, что
    /// удостоверил подпись или смысл — он печатает это архитектору.
    pub not_verified: Vec<String>,
    /// Сводка для отчёта.
    pub summary: String,
}

impl EvidenceVerdict {
    /// Блокирующие находки о содержании (severity `error`).
    #[must_use]
    pub fn blocking_semantics(&self) -> Vec<&SemanticFinding> {
        self.semantics
            .iter()
            .filter(|f| f.severity == "error")
            .collect()
    }
}

/// Обязательные артефакты по маршруту (из обзора AI-Disrupt: объектный минимум
/// разделяют все режимы; Standard/Critical добавляют evidence-проверки).
///
/// `pub(crate)`: тем же списком считается прогресс бандла в проводнике
/// (`crate::bootstrap`) — «7/13» обязано означать тот же профиль, по которому
/// бандл будет проверен, а не похожий.
pub(crate) fn required_artifacts(route: Route) -> Vec<(&'static str, &'static str)> {
    // (ключ, человеко-читаемое описание)
    let mut base = vec![
        ("problem", "формулировка проблемы/гипотезы результата"),
        ("spec_or_delta", "спецификация или дельта"),
        ("risk_level", "уровень риска (significance score)"),
        ("acceptance", "критерии приёмки"),
        ("rollback", "план отката"),
    ];
    match route {
        Route::Fast => {}
        Route::Standard => base.extend([
            ("adr_or_pattern", "ссылка на паттерн или ADR"),
            ("validation", "доказательства валидации (тесты/отчёт)"),
            ("fitness_report", "прогон fitness functions"),
        ]),
        Route::Critical => base.extend([
            ("adr_or_pattern", "ADR с оценкой обратимости"),
            ("spine", "ARCHITECTURE-SPINE с затронутыми инвариантами"),
            (
                "decision_a3",
                "запись человеческого решения A3 (choice/rationale/rejected/expiry)",
            ),
            ("walking_skeleton", "отчёт walking skeleton"),
            (
                "adversarial_review",
                "вердикт состязательного ревью READY/NOT-READY",
            ),
            (
                "rollback_rehearsal",
                "репетиция отката на гейте A4 (.arch-handoff/REHEARSAL.json, PASS)",
            ),
            ("validation", "доказательства валидации (тесты/отчёт)"),
            ("fitness_report", "прогон fitness functions"),
        ]),
    }
    base
}

/// Канонические расположения артефактов в каталоге изменения.
fn candidate_paths(key: &str) -> Vec<&'static str> {
    match key {
        "problem" => vec!["PROBLEM.md", "SPEC.md", "docs/PROBLEM.md", "DELTA.md"],
        "spec_or_delta" => vec!["SPEC.md", "DELTA.md", "docs/SPEC.md", "docs/specs/SPEC.md"],
        "risk_level" => vec!["RISK.md", "SCORE.md", "docs/RISK.md"],
        "acceptance" => vec!["ACCEPTANCE.md", "SPEC.md", "docs/ACCEPTANCE.md"],
        "rollback" => vec!["ROLLBACK.md", "docs/ROLLBACK.md", "PLAN.md"],
        "rollback_rehearsal" => vec![".arch-handoff/REHEARSAL.json", "REHEARSAL.json"],
        "adr_or_pattern" => vec!["docs/adr", "adr", "ADR.md"],
        "spine" => vec!["docs/ARCHITECTURE-SPINE.md", "ARCHITECTURE-SPINE.md"],
        "decision_a3" => vec!["DECISION.md", "docs/DECISION.md", "A3.md"],
        "walking_skeleton" => vec!["WALKING-SKELETON.md", "docs/WALKING-SKELETON.md"],
        "adversarial_review" => vec!["REVIEW.md", "docs/REVIEW.md", "reports/review.md"],
        "validation" => vec!["VALIDATION.md", "reports/tests.md", "docs/VALIDATION.md"],
        "fitness_report" => vec!["reports/fitness.md", "FITNESS.md", "docs/FITNESS.md"],
        _ => vec![],
    }
}

/// FNV-1a 64 — только чтение бандлов старого формата.
fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        hash ^= u64::from(*b);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// Временный/служебный файл, не влияющий на смысл артефакта.
fn is_transient(name: &str) -> bool {
    if name.ends_with('~') || name == ".DS_Store" {
        return true;
    }
    let ext = Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default();
    ["tmp", "swp", "swo"]
        .iter()
        .any(|e| ext.eq_ignore_ascii_case(e))
}

/// Файлы каталога рекурсивно (П2: правка во вложенном подкаталоге обязана
/// менять хэш), с игнором `.git` и временных файлов. Порядок — по полному
/// пути; нечитаемые каталоги пропускаются (не повод валить проверку).
fn dir_files(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in rd.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if path.is_dir() {
                if name != ".git" {
                    stack.push(path);
                }
            } else if path.is_file() && !is_transient(&name) {
                out.push(path);
            }
        }
    }
    out.sort();
    out
}

/// Хэш содержимого файла выбранным алгоритмом (П2: SHA-256 по умолчанию).
fn hash_file(path: &Path, alg: &str) -> Result<(String, u64)> {
    let bytes = std::fs::read(path).map_err(|e| HarnessError::io(path, e))?;
    let hash = if alg == HASH_ALG_SHA256 {
        crate::hash::sha256_hex(&bytes)
    } else {
        format!("{:016x}", fnv1a64(&bytes))
    };
    Ok((hash, bytes.len() as u64))
}

/// Хэш артефакта: файла — содержимого; каталога — рекурсивно, по
/// относительным путям внутри самого артефакта с разделителем `/`.
///
/// Канонический вход (П2): способ написания пути-аргумента (`.`, абсолютный,
/// с завершающим `/`) на вердикт не влияет — в свёртку идёт путь файла
/// относительно проверяемого артефакта, а не то, как был передан каталог.
fn hash_artifact(path: &Path, alg: &str) -> Result<(String, u64)> {
    if path.is_file() {
        return hash_file(path, alg);
    }
    let mut acc = String::new();
    let mut size = 0u64;
    for file in dir_files(path) {
        let (h, s) = hash_file(&file, alg)?;
        let rel = file.strip_prefix(path).map_or_else(
            |_| file.display().to_string(),
            |p| p.to_string_lossy().replace('\\', "/"),
        );
        // Запись в String не может завершиться ошибкой — игнор безопасен.
        let _ = writeln!(acc, "{rel}\0{h}");
        size += s;
    }
    let digest = if alg == HASH_ALG_SHA256 {
        crate::hash::sha256_hex(acc.as_bytes())
    } else {
        format!("{:016x}", fnv1a64(acc.as_bytes()))
    };
    Ok((digest, size))
}

/// Хэш артефакта алгоритмом старого формата (FNV-1a, нерекурсивно, путь
/// через `display()`): только проверка уже собранных бандлов. Новые бандлы
/// собираются [`hash_artifact`] с SHA-256.
fn hash_artifact_legacy(path: &Path) -> Result<(String, u64)> {
    if path.is_file() {
        let bytes = std::fs::read(path).map_err(|e| HarnessError::io(path, e))?;
        return Ok((format!("{:016x}", fnv1a64(&bytes)), bytes.len() as u64));
    }
    let mut acc = String::new();
    let mut size = 0u64;
    let mut entries: Vec<PathBuf> = std::fs::read_dir(path)
        .map_err(|e| HarnessError::io(path, e))?
        .flatten()
        .map(|e| e.path())
        .collect();
    entries.sort();
    for e in entries {
        if e.is_file() {
            let bytes = std::fs::read(&e).map_err(|err| HarnessError::io(&e, err))?;
            let _ = write!(acc, "{}:{:016x};", e.display(), fnv1a64(&bytes));
            size += bytes.len() as u64;
        }
    }
    Ok((format!("{:016x}", fnv1a64(acc.as_bytes())), size))
}

/// Хэш артефакта алгоритмом бандла: `sha256` — канонический рекурсивный,
/// иначе (старый формат) — legacy-свёртка.
fn hash_with_alg(path: &Path, alg: &str) -> Result<(String, u64)> {
    if alg == HASH_ALG_SHA256 {
        hash_artifact(path, HASH_ALG_SHA256)
    } else {
        hash_artifact_legacy(path)
    }
}

/// Ищет артефакт по каноническим путям (файл или каталог с ≥1 md).
///
/// F5: для ключей `problem` / `spec_or_delta` / `acceptance` после
/// канонических путей ищутся активные `OpenSpec` changes — команда на `OpenSpec`
/// не переписывает проблему, дельту спеки и сценарии приёмки второй раз.
/// Markdown `OpenSpec` только читается: Spine его не пишет и не переписывает
/// (правило 9, docs/openspec.md).
fn find_artifact(change_dir: &Path, key: &str) -> Option<PathBuf> {
    for cand in candidate_paths(key) {
        let p = change_dir.join(cand);
        if p.is_file() {
            return Some(p);
        }
        if p.is_dir() {
            let has_md = std::fs::read_dir(&p).is_ok_and(|rd| {
                rd.flatten()
                    .any(|e| e.path().extension().is_some_and(|x| x == "md"))
            });
            if has_md {
                // Каталог (напр. docs/adr) — хэшируем сводку содержимого.
                return Some(p);
            }
        }
    }
    if matches!(key, "problem" | "spec_or_delta" | "acceptance") {
        return openspec_artifact(change_dir, key);
    }
    None
}

// ---------------------------------------------------------------------------
// F5: артефакты бандла из активных OpenSpec changes (только чтение)
// ---------------------------------------------------------------------------

/// Активные `OpenSpec` changes, видимые из каталога бандла: `openspec/changes/*`
/// самого каталога и до двух предков выше (бандл дельты `changes/<name>/`
/// читает change из корня репозитория); `archive/` пропускается — он уже
/// выпущен. Порядок детерминирован: ближний корень, затем имя change.
fn openspec_change_dirs(change_dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut dir = Some(change_dir);
    let mut hops = 0;
    while let Some(d) = dir {
        if hops > 2 {
            break;
        }
        if let Ok(rd) = std::fs::read_dir(d.join("openspec/changes")) {
            let mut names: Vec<PathBuf> = rd
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.is_dir() && p.file_name().is_some_and(|n| n != "archive"))
                .collect();
            names.sort();
            out.extend(names);
        }
        dir = d.parent();
        hops += 1;
    }
    out
}

/// Есть ли в proposal секция `## Why` — без неё proposal не является
/// формулировкой проблемы.
fn has_why_section(text: &str) -> bool {
    text.lines().any(|l| l.trim_start().starts_with("## Why"))
}

/// Есть ли в дельте спеки сценарии приёмки (`#### Scenario:`).
fn has_scenarios(text: &str) -> bool {
    text.lines()
        .any(|l| l.trim_start().starts_with("#### Scenario:"))
}

/// Дельта-спеки change (`specs/**/spec.md`), порядок детерминирован.
fn change_spec_files(change: &Path) -> Vec<PathBuf> {
    dir_files(&change.join("specs"))
        .into_iter()
        .filter(|p| p.file_name().is_some_and(|n| n == "spec.md"))
        .collect()
}

/// Артефакт бандла из активных `OpenSpec` changes (F5): `problem` —
/// `proposal.md` с секцией Why; `spec_or_delta` — дельта-спека change;
/// `acceptance` — дельта-спека со сценариями `#### Scenario:`. Первый
/// подходящий по детерминированному порядку; `None` — подходящего нет.
fn openspec_artifact(change_dir: &Path, key: &str) -> Option<PathBuf> {
    for change in openspec_change_dirs(change_dir) {
        match key {
            "problem" => {
                let proposal = change.join("proposal.md");
                if proposal.is_file()
                    && std::fs::read_to_string(&proposal).is_ok_and(|t| has_why_section(&t))
                {
                    return Some(proposal);
                }
            }
            "spec_or_delta" => {
                if let Some(spec) = change_spec_files(&change).into_iter().next() {
                    return Some(spec);
                }
            }
            "acceptance" => {
                for spec in change_spec_files(&change) {
                    let ok = std::fs::read_to_string(&spec).is_ok_and(|t| has_scenarios(&t));
                    if ok {
                        return Some(spec);
                    }
                }
            }
            _ => {}
        }
    }
    None
}

/// Путь артефакта относительно каталога бандла (для манифеста): артефакт
/// может лежать ВЫШЕ каталога (`OpenSpec` change живёт в корне репозитория,
/// а бандл — в `changes/<name>/`), тогда путь записывается через `..` —
/// абсолютный путь привязывал бы манифест к машине.
fn rel_artifact_path(change_dir: &Path, path: &Path) -> String {
    if let Ok(rel) = path.strip_prefix(change_dir) {
        return rel.display().to_string();
    }
    let change: Vec<_> = change_dir.components().collect();
    let target: Vec<_> = path.components().collect();
    let common = change
        .iter()
        .zip(target.iter())
        .take_while(|(a, b)| a == b)
        .count();
    let mut rel = PathBuf::new();
    for _ in common..change.len() {
        rel.push("..");
    }
    for comp in &target[common..] {
        rel.push(comp);
    }
    rel.display().to_string()
}

/// Собирает Evidence Bundle: манифест `EVIDENCE.yaml` в каталоге изменения.
///
/// # Errors
/// Каталог недоступен, ошибка записи манифеста.
pub fn pack(change_dir: &Path, route: Route) -> Result<(EvidenceBundle, EvidenceVerdict)> {
    pack_with(change_dir, route, None)
}

/// Собирает Evidence Bundle с записью значимости (A2): при наличии
/// `significance` артефакт `risk_level` выводится из записи и файл RISK.md
/// не требуется (рукописный уровень риска — не evidence).
///
/// # Errors
/// Каталог недоступен, ошибка записи манифеста.
pub fn pack_with(
    change_dir: &Path,
    route: Route,
    significance: Option<SignificanceRecord>,
) -> Result<(EvidenceBundle, EvidenceVerdict)> {
    let mut items = Vec::new();
    let mut missing = Vec::new();
    for (key, _desc) in required_artifacts(route) {
        // A2: уровень риска выводится из записи значимости в манифесте.
        if key == "risk_level" && significance.is_some() {
            continue;
        }
        // A1: для артефактов прогонов артефактом бандла становится машинная
        // запись (`.arch-handoff/evidence/<kind>.json`), когда она есть;
        // markdown-отчёт — legacy/сопровождение.
        let found = RecordKind::for_artifact(key)
            .map(|kind| record_path(change_dir, kind))
            .filter(|p| p.is_file())
            .or_else(|| find_artifact(change_dir, key));
        match found {
            Some(path) => {
                let (hash, size) = hash_artifact(&path, HASH_ALG_SHA256)?;
                items.push(EvidenceItem {
                    key: key.into(),
                    path: rel_artifact_path(change_dir, &path),
                    hash,
                    size,
                });
            }
            None => missing.push(key.to_string()),
        }
    }
    let bundle = EvidenceBundle {
        route: format!("{route:?}"),
        packed_at: chrono::Local::now().to_rfc3339(),
        hash_alg: HASH_ALG_SHA256.to_string(),
        significance,
        items,
    };
    let manifest = change_dir.join("EVIDENCE.yaml");
    let text = serde_yaml_ng::to_string(&bundle)
        .map_err(|e| HarnessError::Config(format!("сериализация EVIDENCE: {e}")))?;
    std::fs::write(&manifest, text).map_err(|e| HarnessError::io(&manifest, e))?;
    let verdict = EvidenceVerdict {
        passed: missing.is_empty(),
        summary: format!(
            "Evidence Bundle ({route:?}): артефактов {}, отсутствует {}",
            bundle.items.len(),
            missing.len()
        ),
        missing,
        tampered: Vec::new(),
        warnings: Vec::new(),
        semantics: Vec::new(),
        not_verified: Vec::new(),
    };
    Ok((bundle, verdict))
}

// ---------------------------------------------------------------------------
// Машинные записи прогонов (A1, 0.3.14): отчёт о прогоне пишет машина,
// а не автор. По образцу `REHEARSAL.json` (`crate::rehearsal`) запись
// `.arch-handoff/evidence/<kind>.json` фиксирует команду, её exit-код,
// время, HEAD и хэш входов; `verify` требует запись и проверяет её свежесть.
// ---------------------------------------------------------------------------

use std::time::{Duration, Instant};

/// Схема машинной записи прогона (версия формата).
pub const RECORD_SCHEMA: &str = "arch-be/evidence-record/v1";

/// Каталог записей прогонов внутри кейса/репозитория.
pub const RECORD_DIR: &str = ".arch-handoff/evidence";

/// Таймаут прогона по умолчанию, секунд: тесты и fitness могут идти минуты;
/// зависший прогон — сам по себе находка (как шаг репетиции, ADR-049).
pub const RECORD_TIMEOUT_SECS: u64 = 900;

/// Хвост вывода прогона, попадающий в запись (символов): диагностика живёт
/// в конце вывода.
const RECORD_LOG_TAIL: usize = 2000;

/// Каталоги и файлы, не входящие в хэш входов прогона: служебные и тяжёлые
/// (результаты сборки/кэши) — иначе сам прогон инвалидировал бы свою запись.
const RECORD_INPUT_SKIP_DIRS: &[&str] = &[
    ".git",
    ".arch-handoff",
    "target",
    "node_modules",
    "__pycache__",
    ".pytest_cache",
];

/// Вид машинной записи прогона (`arch-be evidence record <kind>`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordKind {
    /// Прогон fitness-функций (реестр CONSTRAINTS.yaml).
    Fitness,
    /// Прогон тестов (валидация).
    Tests,
    /// Прогон walking skeleton (сквозной сценарий).
    Skeleton,
}

impl RecordKind {
    /// Имя вида для CLI и имени файла записи.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Fitness => "fitness",
            Self::Tests => "tests",
            Self::Skeleton => "skeleton",
        }
    }

    /// Ключ артефакта бандла, который закрывает запись этого вида.
    #[must_use]
    pub fn artifact_key(self) -> &'static str {
        match self {
            Self::Fitness => "fitness_report",
            Self::Tests => "validation",
            Self::Skeleton => "walking_skeleton",
        }
    }

    /// Вид записи по ключу артефакта (`None` — артефакт пишет автор).
    #[must_use]
    pub fn for_artifact(key: &str) -> Option<Self> {
        match key {
            "fitness_report" => Some(Self::Fitness),
            "validation" => Some(Self::Tests),
            "walking_skeleton" => Some(Self::Skeleton),
            _ => None,
        }
    }

    /// Команда по умолчанию: для fitness — прогон реестра правил; для тестов
    /// и скелета универсальной команды нет — `--cmd` обязателен.
    #[must_use]
    pub fn default_command(self) -> Option<&'static str> {
        match self {
            Self::Fitness => Some("arch-be control check ."),
            Self::Tests | Self::Skeleton => None,
        }
    }

    /// Все виды (для подсказок и разбора CLI).
    #[must_use]
    pub fn all() -> [Self; 3] {
        [Self::Fitness, Self::Tests, Self::Skeleton]
    }
}

impl std::str::FromStr for RecordKind {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "fitness" => Ok(Self::Fitness),
            "tests" => Ok(Self::Tests),
            "skeleton" => Ok(Self::Skeleton),
            other => Err(format!(
                "неизвестный вид записи '{other}' (допустимы: fitness, tests, skeleton)"
            )),
        }
    }
}

/// Машинная запись прогона (`.arch-handoff/evidence/<kind>.json`).
///
/// Свежесть записи (A1): запись действительна, пока совпадают и HEAD, и хэш
/// входов. Совпадения только HEAD недостаточно — незакоммиченная правка кода
/// обязана инвалидировать запись; вне git-репозитория (HEAD «absent») судят
/// только входы.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunRecord {
    /// Схема формата ([`RECORD_SCHEMA`]).
    pub schema: String,
    /// Вид записи (`fitness`/`tests`/`skeleton`).
    pub kind: String,
    /// Команда прогона (как запускалась).
    pub command: String,
    /// Exit-код прогона (`None` — таймаут, процесс убит).
    pub exit_code: Option<i32>,
    /// Прогон завершился кодом 0 до таймаута.
    pub passed: bool,
    /// Метка времени записи (локальная, RFC 3339).
    pub recorded_at: String,
    /// Длительность прогона, секунды.
    pub duration_secs: f64,
    /// HEAD репозитория на момент прогона (`absent` — не git-репозиторий).
    pub head: String,
    /// Хэш входов прогона (дерево без служебных/тяжёлых каталогов; для
    /// `tests`/`skeleton` — без markdown-прозы; для `fitness` — всё дерево,
    /// отпечаток реестра и команда).
    pub inputs_hash: String,
    /// Человеко-читаемое описание того, что вошло в хэш входов.
    pub inputs_note: String,
    /// Отпечаток реестра правил (SHA-256 файла CONSTRAINTS.yaml) — только
    /// для `fitness`; `None` у остальных видов или когда реестр не найден.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registry_fingerprint: Option<String>,
    /// Аттестация вердикта прогона: SHA-256 свёртка
    /// `вид\0команда\0exit\0входы\0реестр` — привязывает итог к входам.
    pub attestation: String,
    /// Хвост вывода прогона (диагностика для читающего запись).
    pub output_tail: String,
}

/// Имя файла записи вида (`fitness.json`).
#[must_use]
pub fn record_file_name(kind: RecordKind) -> String {
    format!("{}.json", kind.as_str())
}

/// Путь записи вида в каталоге `dir` (без проверки существования).
#[must_use]
pub fn record_path(dir: &Path, kind: RecordKind) -> PathBuf {
    dir.join(RECORD_DIR).join(record_file_name(kind))
}

/// HEAD репозитория, которому принадлежит `dir` (`None` — не git-репозиторий).
fn current_head(dir: &Path) -> Option<String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["rev-parse", "HEAD"])
        .stdin(std::process::Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let head = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!head.is_empty()).then_some(head)
}

/// Файлы-входы прогона: дерево `root` без служебных/тяжёлых каталогов и
/// манифеста бандла; `exclude_markdown` — проза бандла не вход прогона
/// тестов/скелета (правка отчёта не инвалидирует прогон тестов).
fn record_input_files(root: &Path, exclude_markdown: bool) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in rd.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if path.is_dir() {
                if !RECORD_INPUT_SKIP_DIRS.contains(&name.as_str()) {
                    stack.push(path);
                }
            } else if path.is_file()
                && !is_transient(&name)
                && name != "EVIDENCE.yaml"
                && !(exclude_markdown
                    && Path::new(&name)
                        .extension()
                        .and_then(|e| e.to_str())
                        .is_some_and(|e| e.eq_ignore_ascii_case("md")))
            {
                out.push(path);
            }
        }
    }
    out.sort();
    out
}

/// Отпечаток реестра правил кейса (для записи `fitness`): SHA-256 файла
/// CONSTRAINTS.yaml, который резолвит движок (`control check`).
fn registry_fingerprint(dir: &Path) -> Option<String> {
    let resolved = crate::control::resolve_constraints_path_detailed(dir, None)?;
    crate::hash::sha256_file(&resolved.path)
}

/// Хэш входов прогона: каноническая свёртка дерева входов, команды и (для
/// fitness) отпечатка реестра.
fn inputs_fingerprint(dir: &Path, kind: RecordKind, command: &str) -> (String, String) {
    let exclude_md = kind != RecordKind::Fitness;
    let mut acc = String::new();
    let mut count = 0usize;
    for file in record_input_files(dir, exclude_md) {
        let Some(h) = crate::hash::sha256_file(&file) else {
            continue;
        };
        let rel = file.strip_prefix(dir).map_or_else(
            |_| file.display().to_string(),
            |p| p.to_string_lossy().replace('\\', "/"),
        );
        // Запись в String не может завершиться ошибкой — игнор безопасен.
        let _ = writeln!(acc, "{rel}\0{h}");
        count += 1;
    }
    let registry = if kind == RecordKind::Fitness {
        registry_fingerprint(dir)
    } else {
        None
    };
    let canonical = format!(
        "{}\0{command}\0{}",
        crate::hash::sha256_hex(acc.as_bytes()),
        registry.as_deref().unwrap_or("-")
    );
    let note = match kind {
        RecordKind::Fitness => format!(
            "всё дерево кейса ({count} файлов, без {}) + отпечаток реестра + команда",
            RECORD_INPUT_SKIP_DIRS.join("/")
        ),
        _ => format!(
            "код и конфиги кейса ({count} файлов, без {} и *.md) + команда",
            RECORD_INPUT_SKIP_DIRS.join("/")
        ),
    };
    (crate::hash::sha256_hex(canonical.as_bytes()), note)
}

/// Прогоняет команду и пишет машинную запись `.arch-handoff/evidence/<kind>.json`
/// (A1). Запись создаётся и при провале прогона: FAIL-запись — честное
/// evidence неуспеха, а не отсутствие записи.
///
/// # Errors
/// Каталог недоступен, команда не запустилась, запись не пишется.
pub fn record_run(
    dir: &Path,
    kind: RecordKind,
    command: &str,
    timeout_secs: u64,
) -> Result<RunRecord> {
    let timeout = Duration::from_secs(if timeout_secs == 0 {
        RECORD_TIMEOUT_SECS
    } else {
        timeout_secs
    });
    let (inputs_hash, inputs_note) = inputs_fingerprint(dir, kind, command);
    let head = current_head(dir).unwrap_or_else(|| "absent".to_string());
    let started = Instant::now();
    let outcome = crate::proc::run_shell(dir, "bash", command, timeout)?;
    let duration = started.elapsed().as_secs_f64();
    let (exit_code, passed) = match outcome.status {
        Some(s) => (s.code(), s.success()),
        None => (None, false),
    };
    let mut output = String::from_utf8_lossy(&outcome.stdout).into_owned();
    if !outcome.stderr.is_empty() {
        output.push_str(&String::from_utf8_lossy(&outcome.stderr));
    }
    let output_tail: String = output
        .chars()
        .rev()
        .take(RECORD_LOG_TAIL)
        .collect::<Vec<char>>()
        .into_iter()
        .rev()
        .collect();
    let registry = if kind == RecordKind::Fitness {
        registry_fingerprint(dir)
    } else {
        None
    };
    let attestation = crate::hash::sha256_hex(
        format!(
            "{}\0{}\0{}\0{}\0{}",
            kind.as_str(),
            command,
            exit_code.map_or_else(|| "timeout".to_string(), |c| c.to_string()),
            inputs_hash,
            registry.as_deref().unwrap_or("-")
        )
        .as_bytes(),
    );
    let record = RunRecord {
        schema: RECORD_SCHEMA.to_string(),
        kind: kind.as_str().to_string(),
        command: command.to_string(),
        exit_code,
        passed,
        recorded_at: chrono::Local::now().to_rfc3339(),
        duration_secs: duration,
        head,
        inputs_hash,
        inputs_note,
        registry_fingerprint: registry,
        attestation,
        output_tail: output_tail.trim().to_string(),
    };
    let path = record_path(dir, kind);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| HarnessError::io(parent, e))?;
    }
    let text = serde_json::to_string_pretty(&record)?;
    std::fs::write(&path, format!("{text}\n")).map_err(|e| HarnessError::io(&path, e))?;
    Ok(record)
}

/// Читает запись прогона из каталога `dir` (`None` — записи нет).
///
/// # Errors
/// Файл есть, но не разбирается: подмена/дрейф evidence не замалчивается
/// (тот же принцип, что у [`crate::rehearsal::load_report`]).
pub fn load_run_record(dir: &Path, kind: RecordKind) -> Result<Option<RunRecord>> {
    let path = record_path(dir, kind);
    if !path.is_file() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(&path).map_err(|e| HarnessError::io(&path, e))?;
    let record = serde_json::from_str(&text).map_err(|e| {
        HarnessError::Control(format!(
            "{}: невалидная запись прогона ({e}) — перезапишите: \
             `arch-be evidence record {}`",
            path.display(),
            kind.as_str()
        ))
    })?;
    Ok(Some(record))
}

/// Находит запись прогона для бандла: в каталоге изменения, затем в
/// родителях (бандл дельты `changes/<name>/` читает записи корня
/// репозитория). Возвращает запись и корень, от которого она писалась
/// (свежесть входов считается от него).
fn find_run_record(change_dir: &Path, kind: RecordKind) -> Result<Option<(RunRecord, PathBuf)>> {
    let mut dir = Some(change_dir);
    let mut hops = 0;
    while let Some(d) = dir {
        if hops > 2 {
            break;
        }
        if let Some(record) = load_run_record(d, kind)? {
            return Ok(Some((record, d.to_path_buf())));
        }
        dir = d.parent();
        hops += 1;
    }
    Ok(None)
}

/// Свежесть записи прогона: причины устаревания (пусто — запись свежая).
/// Запись устаревает, когда сместился HEAD ИЛИ изменились входы: незакоммиченная
/// правка кода обязана инвалидировать запись (приёмка A1). Вне git («absent»)
/// судят только входы.
fn record_stale_reasons(record: &RunRecord, kind: RecordKind, root: &Path) -> Vec<String> {
    let mut why = Vec::new();
    if record.head != "absent" {
        if let Some(head) = current_head(root) {
            if head != record.head {
                why.push(format!(
                    "HEAD сместился ({}… ≠ {} записи)",
                    head.chars().take(12).collect::<String>(),
                    record.head.chars().take(12).collect::<String>()
                ));
            }
        }
    }
    let (now, _) = inputs_fingerprint(root, kind, &record.command);
    if now != record.inputs_hash {
        why.push("входы прогона изменились после записи".to_string());
    }
    why
}

// ---------------------------------------------------------------------------
// Семантика артефакта: «есть» ≠ «написан» (Н1 волны A 0.3.4, ADR-041)
// ---------------------------------------------------------------------------

/// Собирает находку о содержании.
fn finding(
    key: &str,
    rule: &str,
    severity: &str,
    message: String,
    fix_hint: &str,
) -> SemanticFinding {
    SemanticFinding {
        key: key.to_string(),
        rule: rule.to_string(),
        severity: severity.to_string(),
        message,
        fix_hint: fix_hint.to_string(),
    }
}

/// Текст артефакта для проверки содержания: файл целиком либо конкатенация
/// его `md`-файлов (для каталогов вроде `docs/adr/`).
fn artifact_text(path: &Path) -> Option<String> {
    if path.is_file() {
        return std::fs::read_to_string(path).ok();
    }
    let mut out = String::new();
    for f in dir_files(path) {
        if let Ok(text) = std::fs::read_to_string(&f) {
            out.push_str(&text);
            out.push('\n');
        }
    }
    Some(out)
}

/// Имя самого проблемного файла каталога-артефакта (для адресного сообщения).
fn stub_file_of(path: &Path, min_bytes: u64) -> Option<String> {
    if path.is_file() {
        return None;
    }
    dir_files(path).into_iter().find_map(|f| {
        let text = std::fs::read_to_string(&f).ok()?;
        let too_small = std::fs::metadata(&f).map_or(0, |m| m.len()) < min_bytes;
        if too_small || crate::stubs::find_stub(&text, true).is_some() {
            Some(f.display().to_string())
        } else {
            None
        }
    })
}

/// Значение поля записи решения A3: понимает `- **choice**: X`, `**choice**: X`
/// и `choice: X` (плюс кириллические имена вроде `выбор`).
fn decision_field(text: &str, name: &str) -> Option<String> {
    for line in text.lines() {
        let t = line.trim().trim_start_matches(['-', '*', ' ']).trim();
        let Some(head) = t.get(..name.len()) else {
            continue;
        };
        if !head.eq_ignore_ascii_case(name) {
            continue;
        }
        // Шаблон записи — `- **choice**: …`: закрывающие `**` стоят до двоеточия.
        let rest = t[name.len()..]
            .strip_prefix("**")
            .unwrap_or(&t[name.len()..]);
        let Some(value) = rest.strip_prefix(':') else {
            continue;
        };
        return Some(
            value
                .trim()
                .trim_start_matches('*')
                .trim()
                .trim_end_matches('*')
                .trim()
                .to_string(),
        );
    }
    None
}

/// Значение поля пустое или является маркером-заглушкой.
fn field_is_empty(value: &str) -> bool {
    let v = value.trim();
    v.is_empty()
        || matches!(v, "—" | "–" | "-" | "?" | "TBD" | "TODO" | "нет" | "n/a")
        || crate::stubs::has_angle_placeholder(v)
        || crate::stubs::is_template_stub(v)
}

/// Строка итога отчёта с провалом.
fn has_fail_line(text: &str) -> bool {
    text.lines().any(|l| {
        let t = l.trim();
        t.contains("Итог: FAIL") || t.eq_ignore_ascii_case("fail") || t.starts_with("FAIL —")
    })
}

/// Подсказка находки по отчёту о прогоне (A1): прогон закрывается машинной
/// записью, а не дописыванием текста — подсказка обязана вести к прогону,
/// иначе она учит обходу («добавьте строку» и зелёный — так 0.3.13 принимал
/// рукописный PASS).
fn record_hint(key: &str) -> String {
    let kind = RecordKind::for_artifact(key).map_or("<kind>", RecordKind::as_str);
    format!(
        "запустите `arch-be evidence record {kind} [--cmd …]`: факт и свежесть \
         прогона фиксирует машинная запись, markdown остаётся сопровождением для человека"
    )
}

/// Вердикт состязательного ревью: `READY` / `NOT-READY` (регистр и кириллица
/// `ВЕРДИКТ` допустимы).
///
/// Возвращает `None`, если строки вердикта нет.
fn review_verdict(text: &str) -> Option<bool> {
    for line in text.lines() {
        let t = line.trim().trim_start_matches(['-', '*', '#', ' ']).trim();
        let t = t.trim_start_matches("**").trim();
        let upper = t.to_uppercase();
        let Some(rest) = upper
            .strip_prefix("VERDICT")
            .or_else(|| upper.strip_prefix("ВЕРДИКТ"))
        else {
            continue;
        };
        // Голое «VERDICT» без «:» — упоминание слова, а не вердикт.
        let Some(rest) = rest.trim().strip_prefix(':') else {
            continue;
        };
        let rest = rest.trim();
        if rest.starts_with("NOT-READY") || rest.starts_with("NOT READY") {
            return Some(false);
        }
        if rest.starts_with("READY") {
            return Some(true);
        }
    }
    None
}

/// Разбирает `expiry` как дату `ГГГГ-ММ-ДД`.
fn expiry_date(value: &str) -> Option<chrono::NaiveDate> {
    let head = value.trim().trim_matches('*').trim();
    let head: String = head.chars().take(10).collect();
    chrono::NaiveDate::parse_from_str(&head, "%Y-%m-%d").ok()
}

/// Проверки содержания одного артефакта: находки + строки «не проверяется».
fn semantic_check(
    change_dir: &Path,
    key: &str,
    path: &Path,
    cfg: &crate::config::EvidenceConfig,
    severity: &str,
) -> (Vec<SemanticFinding>, Vec<String>) {
    let mut out = Vec::new();
    let mut notes = Vec::new();
    let Some(text) = artifact_text(path) else {
        // Нечитаемый артефакт уже виден как «изменён»; дублировать нечем.
        return (out, notes);
    };
    let size = if path.is_file() {
        std::fs::metadata(path).map_or(0, |m| m.len())
    } else {
        text.len() as u64
    };
    // Общие для всех ключей: размер-пустышка и маркеры-заглушки. У артефактов
    // прогонов (A1) подсказка ведёт к машинной записи, а не к дописыванию
    // текста: «заполните отчёт» у рукописного PASS — инструкция обхода.
    let record_stub_hint = RecordKind::for_artifact(key).map(|_| record_hint(key));
    let mut stub_flagged = false;
    if size < cfg.min_bytes {
        let where_ = stub_file_of(path, cfg.min_bytes)
            .map_or_else(|| path.display().to_string(), |f| f.clone());
        out.push(finding(
            key,
            "evidence_stub",
            severity,
            format!(
                "артефакт не написан: {size} б < порога {} б ({where_})",
                cfg.min_bytes
            ),
            record_stub_hint
                .as_deref()
                .unwrap_or("заполните артефакт: заглушка в бандле не удостоверяет ничего"),
        ));
        stub_flagged = true;
    } else if let Some(file) = stub_file_of(path, cfg.min_bytes) {
        out.push(finding(
            key,
            "evidence_stub",
            severity,
            format!("артефакт содержит незаполненное место: {file}"),
            record_stub_hint.as_deref().unwrap_or(
                "уберите маркеры-заглушки (TODO/TBD/<…>) — они попадут в аудиторский след",
            ),
        ));
        stub_flagged = true;
    } else if let Some((line, frag)) = crate::stubs::find_stub(&text, true) {
        out.push(finding(
            key,
            "evidence_stub",
            severity,
            format!("строка {line}: незаполненное место «{frag}»"),
            record_stub_hint
                .as_deref()
                .unwrap_or("замените заглушку содержанием: артефакт обязан быть написан"),
        ));
        stub_flagged = true;
    }
    match key {
        "adversarial_review" => match review_verdict(&text) {
            None => out.push(finding(
                key,
                "review_verdict_missing",
                severity,
                "в ревью нет строки вердикта".to_string(),
                "добавьте строку «VERDICT: READY» или «VERDICT: NOT-READY»",
            )),
            Some(false) => out.push(finding(
                key,
                "review_not_ready",
                severity,
                "ревью поставило NOT-READY — выпуск не подтверждён ревьюером".to_string(),
                "устраните замечания ревью и получите вердикт READY",
            )),
            Some(true) => {}
        },
        "decision_a3" if stub_flagged => {
            notes.push(
                "семантика решения A3 (адекватность выбора) механикой не проверяется — это работа ревьюера"
                    .to_string(),
            );
        }
        "decision_a3" => {
            for (field, why) in [
                ("choice", "не выбран вариант"),
                ("rationale", "нет обоснования выбора"),
                ("rejected", "не перечислены отвергнутые варианты"),
                ("expiry", "нет срока пересмотра решения"),
                ("decided_by", "нет подписанта"),
            ] {
                let value = decision_field(&text, field);
                let empty = value.as_deref().is_none_or(field_is_empty);
                if empty {
                    out.push(finding(
                        key,
                        "a3_not_signed",
                        severity,
                        format!("поле «{field}» записи A3 не заполнено: {why}"),
                        "заполните запись человеческого решения A3 (choice/rationale/rejected/expiry/decided_by)",
                    ));
                } else if field == "decided_by" {
                    if let Some(v) = value {
                        notes.push(format!(
                            "подпись A3: заявлена ({v}), подлинность механикой не проверяется"
                        ));
                    }
                }
            }
            if let Some(v) = decision_field(&text, "expiry") {
                match expiry_date(&v) {
                    Some(d) if d < chrono::Local::now().date_naive() => out.push(finding(
                        key,
                        "a3_expired",
                        severity,
                        format!(
                            "решение A3 просрочено: срок пересмотра {d}, сегодня {}",
                            chrono::Local::now().date_naive()
                        ),
                        "продлите срок пересмотра или примите решение заново",
                    )),
                    Some(_) => {}
                    None => out.push(finding(
                        key,
                        "a3_expiry_invalid",
                        severity,
                        format!("поле «expiry» не дата: «{v}»"),
                        "укажите срок в формате ГГГГ-ММ-ДД",
                    )),
                }
            }
            notes.push(
                "семантика решения A3 (адекватность выбора) механикой не проверяется — это работа ревьюера"
                    .to_string(),
            );
        }
        "rollback_rehearsal" if stub_flagged => {}
        "rollback_rehearsal" => {
            let packet = path.parent().unwrap_or(change_dir);
            match crate::rehearsal::load_report(packet) {
                Ok(Some(report)) if report.passed => {
                    // Д8: «пройдено» с пустым списком шагов — не репетиция, а
                    // отчёт-заготовка: подтверждать нечем, а гейт A4 на каркасе
                    // молчал, потому что видел `passed: true`.
                    if report.steps.is_empty() {
                        out.push(finding(
                            key,
                            "rehearsal_empty",
                            severity,
                            "репетиция объявлена пройденной, но не содержит ни одного шага"
                                .to_string(),
                            "прогоните `arch-be rehearsal run`: пустой список шагов откат не подтверждает",
                        ));
                    }
                    // Д8: baseline — якорь отката. Отчёт, объявленный пройденным
                    // на коммите, которого нет, — ложная аттестация; проверка
                    // смотрит только на `passed`, потому что настоящая репетиция
                    // такого отчёта произвести не может (`rehearsal::rehearse`
                    // падает на нерезолвящемся якоре), а непройденная уже
                    // заблокирована `rehearsal_not_passed`.
                    // Severity — всегда `warn`, в отличие от остальных находок:
                    // нерезолвящийся якорь не доказывает подлога. Пакет мог быть
                    // собран в ДРУГОМ репозитории (кейс, перенесённый в чужую
                    // историю, — так живут `кейсы/*`: их baseline принадлежит
                    // истории кейса, а не репозитория-носителя). Сказать об этом
                    // обязательно, блокировать выпуск — нет.
                    let baseline = report.baseline_commit.trim();
                    if !baseline.is_empty()
                        && crate::rehearsal::baseline_resolves(packet, baseline) == Some(false)
                    {
                        out.push(finding(
                            key,
                            "rehearsal_baseline_unresolved",
                            "warn",
                            format!(
                                "baseline репетиции «{baseline}» не резолвится в коммит этого \
                                 репозитория — якорь отката здесь не проверить"
                            ),
                            "если репетиция шла в другом репозитории, это ожидаемо; иначе укажите \
                             существующий коммит",
                        ));
                    }
                    if let Ok(plan) = crate::rehearsal::load_plan(packet) {
                        if !plan.baseline_commit.trim().is_empty()
                            && plan.baseline_commit.trim() != report.baseline_commit.trim()
                        {
                            out.push(finding(
                                key,
                                "rehearsal_stale_baseline",
                                severity,
                                format!(
                                    "baseline репетиции ({}) ≠ baseline плана ({})",
                                    report.baseline_commit, plan.baseline_commit
                                ),
                                "прогоните репетицию заново после правки ROLLBACK.yaml",
                            ));
                        }
                    }
                }
                Ok(Some(_)) => out.push(finding(
                    key,
                    "rehearsal_not_passed",
                    severity,
                    "репетиция отката не PASS".to_string(),
                    "прогоните `arch-be control gate A4` и добейтесь PASS",
                )),
                Ok(None) => {}
                Err(e) => out.push(finding(
                    key,
                    "rehearsal_invalid",
                    severity,
                    format!("REHEARSAL.json не разбирается: {e}"),
                    "пересоберите evidence репетиции командой `arch-be rehearsal run`",
                )),
            }
        }
        // A1: строка итога в прозе больше не удостоверяет прогон (рукописный
        // PASS 0.3.13) — прогон доказывает машинная запись (проверяется в
        // `record_checks`). Markdown остаётся сопровождением для человека;
        // единственная проза-проверка — честность итога (FAIL в тексте
        // против записи PASS — противоречие, которое надо разрешить прогоном).
        "validation" | "fitness_report" | "walking_skeleton" if has_fail_line(&text) => {
            out.push(finding(
                key,
                "evidence_reports_fail",
                severity,
                "отчёт содержит итог FAIL".to_string(),
                &record_hint(key),
            ));
        }
        _ => {}
    }
    (out, notes)
}

/// Проверки машинной записи прогона (A1): для артефактов `validation` /
/// `fitness_report` / `walking_skeleton` прогон доказывает запись
/// `.arch-handoff/evidence/<kind>.json`, а не проза.
///
/// - записи нет, а markdown-отчёт есть → `evidence_report_unbound` (warn;
///   error при `[evidence] require_records = true`): проза не доказывает прогон;
/// - запись есть, итог не PASS → `evidence_record_failed`;
/// - запись устарела (HEAD или входы сместились) → `evidence_record_stale`;
/// - запись не разбирается → `evidence_record_invalid` (подмена не замалчивается).
fn record_checks(
    change_dir: &Path,
    kind: RecordKind,
    has_markdown: bool,
    cfg: &crate::config::EvidenceConfig,
    severity: &str,
) -> (Vec<SemanticFinding>, Vec<String>) {
    let key = kind.artifact_key();
    let mut out = Vec::new();
    let mut notes = Vec::new();
    let found = match find_run_record(change_dir, kind) {
        Ok(found) => found,
        Err(e) => {
            out.push(finding(
                key,
                "evidence_record_invalid",
                severity,
                format!("запись прогона «{}» не разбирается: {e}", kind.as_str()),
                &format!(
                    "перезапишите запись прогоном: `arch-be evidence record {}`",
                    kind.as_str()
                ),
            ));
            return (out, notes);
        }
    };
    let Some((record, root)) = found else {
        if has_markdown {
            // Правило 4 (warn → error по флагу): по умолчанию находка видна,
            // но не блокирует; проект/bank-профиль включает требование записей.
            let sev = if cfg.require_records { "error" } else { "warn" };
            out.push(finding(
                key,
                "evidence_report_unbound",
                sev,
                format!(
                    "отчёт «{key}» написан прозой, машинной записи прогона нет — \
                     строка «Итог: PASS» в тексте не доказывает, что прогон был"
                ),
                &record_hint(key),
            ));
        }
        return (out, notes);
    };
    notes.push(format!(
        "запись «{}» удостоверяет факт и свежесть прогона ({}), а не его \
         достаточность — что прогонять, решает автор команды",
        kind.as_str(),
        record.command
    ));
    if !record.passed {
        out.push(finding(
            key,
            "evidence_record_failed",
            severity,
            format!(
                "запись «{}» зафиксировала провал прогона (exit {}): {}",
                kind.as_str(),
                record
                    .exit_code
                    .map_or_else(|| "таймаут".to_string(), |c| c.to_string()),
                record.output_tail.lines().next().unwrap_or_default()
            ),
            &format!(
                "добейтесь зелёного прогона и повторите `arch-be evidence record {}`",
                kind.as_str()
            ),
        ));
        return (out, notes);
    }
    let stale = record_stale_reasons(&record, kind, &root);
    if !stale.is_empty() {
        out.push(finding(
            key,
            "evidence_record_stale",
            severity,
            format!(
                "запись прогона «{}» устарела: {}",
                kind.as_str(),
                stale.join("; ")
            ),
            &format!(
                "повторите `arch-be evidence record {}` на текущем состоянии и переупакуйте бандл",
                kind.as_str()
            ),
        ));
    }
    (out, notes)
}

/// Сколько обязательных артефактов профиля маршрута уже есть в каталоге
/// изменения. `None` — манифеста нет или он не читается (проводник скажет
/// «бандл не собран»); «есть файл» и «есть запись в манифесте» намеренно
/// различаются: прогресс считается по манифесту, как и сама проверка.
#[must_use]
pub fn bundle_progress(change_dir: &Path, route: Route) -> Option<usize> {
    let split = bundle_progress_split(change_dir, route)?;
    Some(split.author_done + split.machine_done)
}

/// Прогресс бандла по происхождению артефактов (A2): раздельный счёт «что
/// пишет автор» и «что выведет машина» — этой парой чисел снимается замер
/// церемонии (сколько файлов человек правит руками, до/после).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BundleProgress {
    /// Артефакты автора: есть в манифесте / всего по профилю.
    pub author_done: usize,
    /// Всего артефактов автора по профилю маршрута.
    pub author_total: usize,
    /// Машинные артефакты: есть в манифесте / всего по профилю.
    pub machine_done: usize,
    /// Всего машинных артефактов по профилю маршрута.
    pub machine_total: usize,
}

/// Раздельный прогресс по манифесту: машинный ключ засчитан, когда его
/// артефакт описан в манифесте; `risk_level` — когда в манифесте есть запись
/// значимости (A2), даже без файла-артефакта.
#[must_use]
pub fn bundle_progress_split(change_dir: &Path, route: Route) -> Option<BundleProgress> {
    let text = std::fs::read_to_string(change_dir.join("EVIDENCE.yaml")).ok()?;
    let bundle: EvidenceBundle = serde_yaml_ng::from_str(&text).ok()?;
    let mut progress = BundleProgress {
        author_done: 0,
        author_total: 0,
        machine_done: 0,
        machine_total: 0,
    };
    for (key, _) in required_artifacts(route) {
        let present = bundle.items.iter().any(|i| i.key == *key)
            || (key == "risk_level" && bundle.significance.is_some());
        if is_machine_derived(key) {
            progress.machine_total += 1;
            progress.machine_done += usize::from(present);
        } else {
            progress.author_total += 1;
            progress.author_done += usize::from(present);
        }
    }
    Some(progress)
}

/// Есть ли у бандла вход для проверки содержания артефакта (файл на месте).
fn existing_artifact(change_dir: &Path, key: &str) -> Option<PathBuf> {
    find_artifact(change_dir, key)
}

/// Проверяет bundle: обязательные артефакты на месте, хэши совпадают.
///
/// Старый формат (`hash_alg` отсутствует или `fnv1a64`) проверяется старой
/// свёрткой с предупреждением о переупаковке — вердикт не блокируется только
/// из-за формата.
///
/// # Errors
/// Манифест отсутствует/не валиден.
pub fn verify(change_dir: &Path) -> Result<EvidenceVerdict> {
    verify_with(change_dir, &crate::config::EvidenceConfig::default())
}

/// Проверяет bundle с настройками семантики из `[evidence]` конфига
/// (Н1 волны A 0.3.4, ADR-041).
///
/// Полнота и целостность проверяются всегда; проверки СОДЕРЖАНИЯ включаются
/// `[evidence] semantics` (дефолт `auto`: Critical — `error`, Standard/Fast —
/// `warn`). `passed` требует, чтобы среди находок о содержании не было
/// блокирующих.
///
/// # Errors
/// Манифест отсутствует/не валиден.
pub fn verify_with(
    change_dir: &Path,
    cfg: &crate::config::EvidenceConfig,
) -> Result<EvidenceVerdict> {
    let manifest = change_dir.join("EVIDENCE.yaml");
    let text = std::fs::read_to_string(&manifest).map_err(|e| HarnessError::io(&manifest, e))?;
    let bundle: EvidenceBundle = serde_yaml_ng::from_str(&text)?;
    let route = match bundle.route.as_str() {
        "Standard" => Route::Standard,
        "Critical" => Route::Critical,
        _ => Route::Fast,
    };
    let alg = if bundle.hash_alg == HASH_ALG_SHA256 {
        HASH_ALG_SHA256
    } else {
        HASH_ALG_LEGACY
    };
    let mut warnings = Vec::new();
    if alg == HASH_ALG_LEGACY {
        warnings.push(format!(
            "бандл старого формата ({}): переупакуйте (`arch-be evidence pack`) — \
             новые бандлы используют SHA-256",
            bundle.hash_alg
        ));
    }
    let mut missing = Vec::new();
    let mut tampered = Vec::new();
    for (key, _desc) in required_artifacts(route) {
        let present = bundle.items.iter().any(|i| i.key == key)
            // A2: уровень риска выводится из записи значимости манифеста.
            || (key == "risk_level" && bundle.significance.is_some());
        if !present {
            missing.push(key.to_string());
        }
    }
    for item in &bundle.items {
        let path = change_dir.join(&item.path);
        if !path.exists() {
            tampered.push(format!("{} (удалён: {})", item.key, item.path));
            continue;
        }
        let (hash, _) = hash_with_alg(&path, alg)?;
        if hash != item.hash {
            tampered.push(format!(
                "{} (изменён после упаковки: {})",
                item.key, item.path
            ));
        }
    }
    // Содержание артефактов: третий класс исхода (Н1, ADR-041).
    let mut semantics = Vec::new();
    let mut not_verified = Vec::new();
    if let Some(severity) = cfg.severity_for(route) {
        for (key, _desc) in required_artifacts(route) {
            // A1: у артефактов прогонов машинная запись закрывает ключ и без
            // markdown — проверяется и в этом случае.
            if let Some(kind) = RecordKind::for_artifact(key) {
                let md = existing_artifact(change_dir, key);
                let (found, rec_notes) =
                    record_checks(change_dir, kind, md.is_some(), cfg, severity);
                semantics.extend(found);
                for n in rec_notes {
                    if !not_verified.contains(&n) {
                        not_verified.push(n);
                    }
                }
                let Some(path) = md else { continue };
                let (found, notes) = semantic_check(change_dir, key, &path, cfg, severity);
                semantics.extend(found);
                for n in notes {
                    if !not_verified.contains(&n) {
                        not_verified.push(n);
                    }
                }
                continue;
            }
            let Some(path) = existing_artifact(change_dir, key) else {
                continue;
            };
            let (found, notes) = semantic_check(change_dir, key, &path, cfg, severity);
            semantics.extend(found);
            for n in notes {
                if !not_verified.contains(&n) {
                    not_verified.push(n);
                }
            }
        }
    } else if crate::config::EvidenceSemantics::Off == cfg.semantics {
        not_verified.push(
            "проверки содержания артефактов выключены ([evidence] semantics = off) — \
             зелёный вердикт удостоверяет только наличие и целостность файлов"
                .to_string(),
        );
    }
    let blocking = semantics.iter().filter(|f| f.severity == "error").count();
    let passed = missing.is_empty() && tampered.is_empty() && blocking == 0;
    let summary = format!(
        "Проверка bundle ({}): артефактов {}, отсутствует {}, изменено {}, \
         содержание — находок {} (блокирующих {blocking})",
        bundle.route,
        bundle.items.len(),
        missing.len(),
        tampered.len(),
        semantics.len()
    );
    Ok(EvidenceVerdict {
        passed,
        missing,
        tampered,
        warnings,
        semantics,
        not_verified,
        summary,
    })
}

// ---------------------------------------------------------------------------
// Агентные инструменты `evidence_verify` / `evidence_pack` (мост в MCP,
// транш 1 инверсии)
// ---------------------------------------------------------------------------

/// Инструменты домена: `evidence_verify` (read-only проверка бандла),
/// `evidence_pack` (сборка манифеста — пишущий, в MCP только под `--rw`).
#[must_use]
pub fn tools() -> Vec<Arc<dyn Tool>> {
    vec![Arc::new(EvidenceVerifyTool), Arc::new(EvidencePackTool)]
}

/// Инструмент `evidence_verify`: проверка Evidence Bundle —
/// JSON-вердикт `{passed, issues, summary}`.
pub struct EvidenceVerifyTool;

#[derive(Debug, Deserialize)]
struct EvidenceDirArgs {
    /// Каталог изменения (с EVIDENCE.yaml для verify).
    #[serde(alias = "path")]
    change_dir: String,
}

#[async_trait]
impl Tool for EvidenceVerifyTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "evidence_verify".into(),
            description: "Проверить Evidence Bundle (EVIDENCE.yaml в каталоге изменения): \
                          полнота по профилю маршрута (Fast/Standard/Critical) + целостность \
                          хэшей артефактов (подмена/дрейф после упаковки). Ответ — JSON: \
                          passed + issues (missing/tampered) + summary; passed=false — \
                          выпуск заблокирован"
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Каталог изменения с EVIDENCE.yaml"}
                },
                "required": ["path"]
            }),
        }
    }

    async fn call(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let args: EvidenceDirArgs = match serde_json::from_value(args) {
            Ok(a) => a,
            Err(e) => {
                return Ok(ToolOutput::err(format!(
                    "evidence_verify: невалидные аргументы: {e}"
                )));
            }
        };
        let dir = ctx.resolve(&args.change_dir);
        let verdict = match verify_with(&dir, &ctx.config.evidence) {
            Ok(v) => v,
            Err(e) => return Ok(ToolOutput::err(format!("evidence_verify: {e}"))),
        };
        let issues: Vec<Value> = verdict
            .missing
            .iter()
            .map(|m| json!({"kind": "missing", "artifact": m}))
            .chain(
                verdict
                    .tampered
                    .iter()
                    .map(|t| json!({"kind": "tampered", "artifact": t})),
            )
            .chain(verdict.semantics.iter().map(|s| {
                json!({
                    "kind": "semantic",
                    "artifact": s.key,
                    "rule": s.rule,
                    "severity": s.severity,
                    "message": s.message,
                    "fix_hint": s.fix_hint,
                })
            }))
            .collect();
        let out = json!({
            "tool": "evidence_verify",
            "passed": verdict.passed,
            "issues": issues,
            "warnings": verdict.warnings,
            "not_verified": verdict.not_verified,
            "summary": verdict.summary,
        });
        // Сериализация собранного объекта не падает; запасной вариант — компактная форма.
        let text = serde_json::to_string_pretty(&out).unwrap_or_else(|_| out.to_string());
        Ok(ToolOutput::ok(text))
    }
}

/// Инструмент `evidence_pack`: сборка Evidence Bundle (манифест
/// `EVIDENCE.yaml` в каталоге изменения; пишущий — в MCP под `--rw`).
pub struct EvidencePackTool;

#[derive(Debug, Deserialize)]
struct EvidencePackArgs {
    /// Каталог изменения.
    #[serde(alias = "path")]
    change_dir: String,
    /// Маршрут: fast|standard|critical (дефолт standard).
    route: Option<String>,
}

#[async_trait]
impl Tool for EvidencePackTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "evidence_pack".into(),
            description: "Собрать Evidence Bundle: манифест EVIDENCE.yaml с SHA-256 хэшами \
                          артефактов каталога изменения по профилю маршрута (fast/standard/\
                          critical). Пишет манифест в рабочий каталог; вердикт полноты — \
                          в ответе (passed=false — не хватает обязательных артефактов)"
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Каталог изменения"},
                    "route": {
                        "type": "string",
                        "description": "Маршрут: fast | standard | critical (по умолчанию standard)",
                        "enum": ["fast", "standard", "critical"]
                    }
                },
                "required": ["path"]
            }),
        }
    }

    async fn call(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let args: EvidencePackArgs = match serde_json::from_value(args) {
            Ok(a) => a,
            Err(e) => {
                return Ok(ToolOutput::err(format!(
                    "evidence_pack: невалидные аргументы: {e}"
                )));
            }
        };
        let route = match args
            .route
            .as_deref()
            .unwrap_or("standard")
            .trim()
            .to_ascii_lowercase()
            .as_str()
        {
            "fast" => Route::Fast,
            "standard" => Route::Standard,
            "critical" => Route::Critical,
            other => {
                return Ok(ToolOutput::err(format!(
                    "evidence_pack: неизвестный маршрут '{other}' (допустимы: fast, standard, critical)"
                )));
            }
        };
        let dir = ctx.resolve(&args.change_dir);
        let (bundle, verdict) = match pack(&dir, route) {
            Ok(pair) => pair,
            Err(e) => return Ok(ToolOutput::err(format!("evidence_pack: {e}"))),
        };
        let out = json!({
            "tool": "evidence_pack",
            "passed": verdict.passed,
            "manifest": dir.join("EVIDENCE.yaml").display().to_string(),
            "route": bundle.route,
            "hash_alg": bundle.hash_alg,
            "items": bundle.items.len(),
            "missing": verdict.missing,
            "summary": verdict.summary,
        });
        // Сериализация собранного объекта не падает; запасной вариант — компактная форма.
        let text = serde_json::to_string_pretty(&out).unwrap_or_else(|_| out.to_string());
        Ok(ToolOutput::ok(text))
    }
}

#[cfg(test)]
mod tests;
