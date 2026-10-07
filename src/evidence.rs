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
    /// Артефакты.
    pub items: Vec<EvidenceItem>,
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
    None
}

/// Собирает Evidence Bundle: манифест `EVIDENCE.yaml` в каталоге изменения.
///
/// # Errors
/// Каталог недоступен, ошибка записи манифеста.
pub fn pack(change_dir: &Path, route: Route) -> Result<(EvidenceBundle, EvidenceVerdict)> {
    let mut items = Vec::new();
    let mut missing = Vec::new();
    for (key, _desc) in required_artifacts(route) {
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
                    path: path
                        .strip_prefix(change_dir)
                        .map_or_else(|_| path.display().to_string(), |p| p.display().to_string()),
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
    let text = std::fs::read_to_string(change_dir.join("EVIDENCE.yaml")).ok()?;
    let bundle: EvidenceBundle = serde_yaml_ng::from_str(&text).ok()?;
    Some(
        required_artifacts(route)
            .iter()
            .filter(|(key, _)| bundle.items.iter().any(|i| i.key == *key))
            .count(),
    )
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
        if !bundle.items.iter().any(|i| i.key == key) {
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
mod tests {
    use super::*;

    fn put(root: &Path, rel: &str, content: &str) {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().expect("parent")).expect("mkdir");
        std::fs::write(p, content).expect("write");
    }

    #[test]
    fn fast_route_packs_minimal_bundle() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path();
        put(dir, "PROBLEM.md", "# Проблема\n");
        put(
            dir,
            "SPEC.md",
            "## Проблема\n## Критерии приёмки\n## Риски\n",
        );
        put(dir, "RISK.md", "Fast: 0 триггеров\n");
        put(dir, "ROLLBACK.md", "git revert\n");
        let (bundle, verdict) = pack(dir, Route::Fast).expect("pack");
        assert!(verdict.passed, "missing: {:?}", verdict.missing);
        assert!(bundle.items.len() >= 5, "items: {}", bundle.items.len());
        assert!(dir.join("EVIDENCE.yaml").is_file());
        // Проверка чиста сразу после упаковки.
        let v = verify(dir).expect("verify");
        assert!(v.passed, "{:?} {:?}", v.missing, v.tampered);
    }

    #[test]
    fn critical_route_requires_a3_and_spine() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path();
        put(dir, "PROBLEM.md", "x");
        let (_bundle, verdict) = pack(dir, Route::Critical).expect("pack");
        assert!(!verdict.passed);
        assert!(verdict.missing.contains(&"decision_a3".to_string()));
        assert!(verdict.missing.contains(&"spine".to_string()));
        assert!(verdict.missing.contains(&"adversarial_review".to_string()));
        // Critical требует и evidence репетиции отката (гейт A4).
        assert!(verdict.missing.contains(&"rollback_rehearsal".to_string()));
    }

    #[test]
    fn verify_detects_tampering_after_pack() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path();
        put(dir, "PROBLEM.md", "исходная проблема");
        put(dir, "SPEC.md", "спека");
        put(dir, "RISK.md", "r");
        put(dir, "ROLLBACK.md", "rb");
        pack(dir, Route::Fast).expect("pack");
        // Подмена артефакта после упаковки.
        put(dir, "SPEC.md", "ТИХО ПЕРЕПИСАЛИ");
        let v = verify(dir).expect("verify");
        assert!(!v.passed);
        assert!(
            v.tampered.iter().any(|t| t.contains("spec_or_delta")),
            "{:?}",
            v.tampered
        );
    }

    // --- П2: канонический вход, рекурсия, SHA-256, миграция формата --------

    /// Вердикт не зависит от написания пути, каталог хэшируется рекурсивно,
    /// хэши — SHA-256.
    #[test]
    fn hash_is_path_invariant_and_recursive() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path();
        put(dir, "PROBLEM.md", "p");
        put(dir, "SPEC.md", "s\n## Критерии приёмки\n");
        put(dir, "RISK.md", "r");
        put(dir, "ROLLBACK.md", "rb");
        put(dir, "docs/adr/ADR-001.md", "a1");
        put(dir, "docs/adr/archive/ADR-002.md", "a2");
        put(dir, "VALIDATION.md", "v");
        put(dir, "reports/fitness.md", "f");
        let (bundle, verdict) = pack(dir, Route::Standard).expect("pack");
        assert!(verdict.passed, "missing: {:?}", verdict.missing);
        assert_eq!(bundle.hash_alg, HASH_ALG_SHA256);
        assert!(
            bundle.items.iter().all(|i| i.hash.len() == 64),
            "хэши обязаны быть SHA-256 hex: {:?}",
            bundle
                .items
                .iter()
                .map(|i| i.hash.len())
                .collect::<Vec<_>>()
        );
        // Инвариантность к написанию пути (Д2): «.», абсолютный, с «/».
        let trailing = PathBuf::from(format!("{}/", dir.display()));
        let abs = dir.canonicalize().expect("canonicalize");
        let v1 = verify(dir).expect("verify");
        let v2 = verify(&trailing).expect("verify trailing");
        let v3 = verify(&abs).expect("verify abs");
        assert!(v1.passed && v2.passed && v3.passed, "{v1:?} {v2:?} {v3:?}");
        assert_eq!(v1.tampered, v2.tampered);
        assert_eq!(v1.tampered, v3.tampered);
        // Рекурсия: правка во вложенном подкаталоге каталога-артефакта видна.
        put(dir, "docs/adr/archive/ADR-002.md", "a2 ИЗМЕНЁН");
        let v4 = verify(&abs).expect("verify");
        assert!(!v4.passed);
        assert!(
            v4.tampered.iter().any(|t| t.contains("adr_or_pattern")),
            "{:?}",
            v4.tampered
        );
    }

    /// Бандл старого формата (без `hash_alg`) проверяется старой свёрткой
    /// и получает предупреждение о переупаковке.
    #[test]
    fn legacy_bundle_verified_with_old_alg_and_warned() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path();
        put(dir, "PROBLEM.md", "p");
        put(dir, "SPEC.md", "s");
        put(dir, "RISK.md", "r");
        put(dir, "ROLLBACK.md", "rb");
        let (mut bundle, _v) = pack(dir, Route::Fast).expect("pack");
        bundle.hash_alg = HASH_ALG_LEGACY.to_string();
        for item in &mut bundle.items {
            let p = dir.join(&item.path);
            let (h, s) = hash_artifact_legacy(&p).expect("legacy hash");
            item.hash = h;
            item.size = s;
        }
        // Эмулируем старый манифест: поля hash_alg в нём не было.
        let text = serde_yaml_ng::to_string(&bundle).expect("yaml");
        let text = text
            .lines()
            .filter(|l| !l.starts_with("hash_alg:"))
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(dir.join("EVIDENCE.yaml"), text).expect("write");
        let v = verify(dir).expect("verify");
        assert!(
            v.passed,
            "missing: {:?}, tampered: {:?}",
            v.missing, v.tampered
        );
        assert!(
            v.warnings.iter().any(|w| w.contains("старого формата")),
            "{:?}",
            v.warnings
        );
    }

    // --- инструменты evidence_pack / evidence_verify ------------------------

    /// Тестовый контекст без LLM.
    fn tool_ctx(dir: &Path) -> ToolContext {
        ToolContext::new(
            dir.to_path_buf(),
            Arc::new(crate::config::Config::default()),
        )
    }

    #[tokio::test]
    async fn evidence_pack_then_verify_tools_roundtrip_passes() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path();
        // Содержательные артефакты: с 0.3.4 пустышка — находка (Н1, ADR-041),
        // и «чисто сразу после упаковки» проверяется на написанном бандле.
        put(dir, "PROBLEM.md", &body("Проблема"));
        put(dir, "SPEC.md", &body("Спецификация"));
        put(dir, "RISK.md", &body("Риск"));
        put(dir, "ROLLBACK.md", &body("Откат"));
        let ctx = tool_ctx(dir);
        // pack (fast) → manifest записан, полнота ок.
        let out = EvidencePackTool
            .call(json!({"change_dir": ".", "route": "fast"}), &ctx)
            .await
            .expect("вызов");
        assert!(!out.is_error, "{}", out.content);
        let v: Value = serde_json::from_str(&out.content).expect("JSON-вердикт");
        assert_eq!(v["passed"], true, "{v}");
        assert!(dir.join("EVIDENCE.yaml").is_file());
        // verify сразу после pack — чист.
        let out = EvidenceVerifyTool
            .call(json!({"change_dir": "."}), &ctx)
            .await
            .expect("вызов");
        let v: Value = serde_json::from_str(&out.content).expect("JSON-вердикт");
        assert_eq!(v["passed"], true, "{v}");
        assert_eq!(v["issues"], json!([]));

        // Подмена артефакта → verify passed=false, находка kind=tampered.
        put(dir, "SPEC.md", "ПЕРЕПИСАНО");
        let out = EvidenceVerifyTool
            .call(json!({"change_dir": "."}), &ctx)
            .await
            .expect("вызов");
        let v: Value = serde_json::from_str(&out.content).expect("JSON-вердикт");
        assert_eq!(v["passed"], false, "{v}");
        assert!(
            v["issues"]
                .as_array()
                .expect("issues")
                .iter()
                .any(|i| i["kind"] == "tampered"),
            "{v}"
        );
    }

    // --- Н1: семантика артефакта («есть» ≠ «написан», ADR-041) -------------

    /// Содержательное наполнение: длиннее порога 200 б и без маркеров-заглушек.
    fn body(title: &str) -> String {
        format!(
            "# {title}\n\n{}\n",
            "Содержательный раздел решения с обоснованием, альтернативами и \
             последствиями. "
                .repeat(4)
        )
    }

    /// Полный и СОДЕРЖАТЕЛЬНЫЙ бандл маршрута Critical.
    fn put_complete_critical(dir: &Path) {
        put(dir, "PROBLEM.md", &body("Проблема"));
        put(dir, "SPEC.md", &body("Спецификация"));
        put(dir, "RISK.md", &body("Риск"));
        put(dir, "ROLLBACK.md", &body("Откат"));
        put(dir, "docs/adr/ADR-001.md", &body("Решение"));
        put(dir, "ARCHITECTURE-SPINE.md", &body("Инварианты"));
        put(
            dir,
            "DECISION.md",
            &format!(
                "# Решение A3\n\n- **choice**: {}\n- **rationale**: {}\n- \
                 **rejected**: {}\n- **expiry**: 2099-12-31\n- **decided_by**: Архитектор ДКА\n",
                body("вариант"),
                body("обоснование"),
                body("отвергнутое")
            ),
        );
        put(
            dir,
            "WALKING-SKELETON.md",
            &format!(
                "# Walking skeleton\n\n{}\n\nИтог: PASS (8 из 8)\n",
                body("Сквозной прогон")
            ),
        );
        put(
            dir,
            "docs/REVIEW.md",
            &format!(
                "# Ревью\n\nВердикт.\n\nVERDICT: READY\n\nВопросы разобраны: {}",
                body("итог")
            ),
        );
        // Д8: «пройдено» обязано опираться на шаги. Отчёт без шагов (каким его
        // писала заготовка `bootstrap` до 0.3.5) — не аттестация, и держать его
        // в фикстуре «полного бандла» значило бы требовать от гейта слепоты.
        put(
            dir,
            ".arch-handoff/REHEARSAL.json",
            r#"{"kind":"rollback_rehearsal","gate":"A4","passed":true,
                    "baseline_commit":"abc123","rehearsed_at":"2026-09-19T10:00:00Z",
                    "duration_secs":1.5,
                    "steps":[{"name":"якорь-доступен","status":"pass","exit_code":0,
                              "detail":"commit"}],
                    "verify":null,
                    "log":["репетиция отката прошла"]}"#,
        );
        put(
            dir,
            ".arch-handoff/ROLLBACK.yaml",
            "baseline_commit: abc123\nsteps:\n  - name: revert\n    run: git revert --no-edit HEAD\n",
        );
        put(
            dir,
            "VALIDATION.md",
            &format!("# Валидация\n\n{}\n\nИтог: PASS\n", body("Тесты")),
        );
        put(
            dir,
            "reports/fitness.md",
            &format!("# Fitness\n\n{}\n\nИтог: PASS\n", body("Правила")),
        );
    }

    /// Абсолютный регресс 0.3.3: бандл из заглушек проходил как «выпуск разрешён».
    /// A1 (0.3.14): содержательный бандл зелёный, но рукописные отчёты прогонов
    /// по умолчанию названы предупреждением `evidence_report_unbound` — проза не
    /// доказывает прогон (error — только по флагу `require_records`).
    #[test]
    fn verify_passes_on_complete_bundle() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path();
        put_complete_critical(dir);
        let (_b, v) = pack(dir, Route::Critical).expect("pack");
        assert!(v.passed, "missing: {:?}", v.missing);
        let v = verify(dir).expect("verify");
        assert!(
            v.passed,
            "содержательный бандл обязан быть зелёным; находки: {:?}",
            v.semantics
        );
        assert!(
            v.semantics
                .iter()
                .all(|f| f.rule == "evidence_report_unbound" && f.severity == "warn"),
            "кроме предупреждений о рукописных отчётах находок нет: {:?}",
            v.semantics
        );
        // Подлинность подписи A3 — заявленное, но механикой не проверяется.
        assert!(
            v.not_verified
                .iter()
                .any(|n| n.contains("подпись A3: заявлена")),
            "{:?}",
            v.not_verified
        );
    }

    /// Заглушка вместо артефакта: файл есть, содержания нет.
    #[test]
    fn verify_flags_stub_artifact() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path();
        put_complete_critical(dir);
        put(dir, "DECISION.md", "TODO");
        let (_b, _v) = pack(dir, Route::Critical).expect("pack");
        let v = verify(dir).expect("verify");
        assert!(!v.passed, "заглушка на Critical обязана блокировать выпуск");
        assert!(
            v.semantics.iter().any(|f| f.rule == "evidence_stub"
                && f.key == "decision_a3"
                && f.severity == "error"),
            "{:?}",
            v.semantics
        );
        assert!(!v.blocking_semantics().is_empty());
    }

    /// Ревью с вердиктом NOT-READY больше не даёт «выпуск разрешён».
    #[test]
    fn verify_blocks_not_ready_review() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path();
        put_complete_critical(dir);
        put(
            dir,
            "docs/REVIEW.md",
            &format!("# R\n\nVERDICT: NOT-READY\n\n{}", body("замечания")),
        );
        let (_b, _v) = pack(dir, Route::Critical).expect("pack");
        let v = verify(dir).expect("verify");
        assert!(!v.passed);
        assert!(
            v.semantics.iter().any(|f| f.rule == "review_not_ready"),
            "{:?}",
            v.semantics
        );
        // Отсутствие строки вердикта — отдельная находка.
        put(dir, "docs/REVIEW.md", &body("ревью без вердикта"));
        pack(dir, Route::Critical).expect("repack");
        let v = verify(dir).expect("verify");
        assert!(
            v.semantics
                .iter()
                .any(|f| f.rule == "review_verdict_missing"),
            "{:?}",
            v.semantics
        );
    }

    /// Неподписанная запись A3 (пустой `decided_by`) — находка `a3_not_signed`.
    #[test]
    fn verify_flags_unsigned_a3() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path();
        put_complete_critical(dir);
        put(
            dir,
            "DECISION.md",
            "# Решение A3\n\n- **choice**: вариант А — централизованный клиринг\n- **rationale**: снижает операционный риск расчётов и снимает зависимость от ручных сверок\n- **rejected**: вариант Б — распределённый клиринг, отклонён из-за сложности сопровождения\n- **expiry**: 2099-12-31\n- **decided_by**: \n",
        );
        pack(dir, Route::Critical).expect("pack");
        let v = verify(dir).expect("verify");
        assert!(!v.passed);
        assert!(
            v.semantics
                .iter()
                .any(|f| f.rule == "a3_not_signed" && f.message.contains("decided_by")),
            "{:?}",
            v.semantics
        );
        // Прочерк — тот же случай, что пустое поле.
        put(
            dir,
            "DECISION.md",
            "# Решение A3\n\n- **choice**: вариант А — централизованный клиринг\n- **rationale**: снижает операционный риск расчётов и снимает зависимость от ручных сверок\n- **rejected**: вариант Б — распределённый клиринг, отклонён из-за сложности сопровождения\n- **expiry**: 2099-12-31\n- **decided_by**: —\n",
        );
        pack(dir, Route::Critical).expect("pack");
        let v = verify(dir).expect("verify");
        assert!(v.semantics.iter().any(|f| f.rule == "a3_not_signed"));
        assert!(!v.passed);
    }

    /// Просроченный срок пересмотра решения — находка `a3_expired`.
    #[test]
    fn verify_flags_expired_a3() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path();
        put_complete_critical(dir);
        put(
            dir,
            "DECISION.md",
            "# Решение A3\n\n- **choice**: вариант А — централизованный клиринг\n- **rationale**: снижает операционный риск расчётов и снимает зависимость от ручных сверок\n- **rejected**: вариант Б — распределённый клиринг, отклонён из-за сложности сопровождения\n- **expiry**: 2020-01-01\n- **decided_by**: Архитектор\n",
        );
        pack(dir, Route::Critical).expect("pack");
        let v = verify(dir).expect("verify");
        assert!(!v.passed);
        assert!(
            v.semantics.iter().any(|f| f.rule == "a3_expired"),
            "{:?}",
            v.semantics
        );
    }

    /// `semantics = off` возвращает поведение 0.3.3: проверяется только
    /// наличие и целостность, о выключенных проверках сказано честно.
    #[test]
    fn semantics_off_restores_033_behaviour() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path();
        put_complete_critical(dir);
        put(dir, "DECISION.md", "TODO");
        pack(dir, Route::Critical).expect("pack");
        let cfg = crate::config::EvidenceConfig {
            semantics: crate::config::EvidenceSemantics::Off,
            ..crate::config::EvidenceConfig::default()
        };
        let v = verify_with(dir, &cfg).expect("verify");
        assert!(
            v.passed,
            "0.3.3 не смотрела на содержание: {:?}",
            v.semantics
        );
        assert!(v.semantics.is_empty());
        assert!(
            v.not_verified.iter().any(|n| n.contains("выключены")),
            "{:?}",
            v.not_verified
        );
        // На маршруте Standard та же заглушка — warn, выпуск не блокируется.
        let std_dir = tmp.path().join("standard");
        std::fs::create_dir_all(&std_dir).expect("mkdir");
        put(&std_dir, "PROBLEM.md", &body("Проблема"));
        put(&std_dir, "SPEC.md", &body("Спека"));
        put(&std_dir, "RISK.md", &body("Риск"));
        put(&std_dir, "ROLLBACK.md", &body("Откат"));
        put(&std_dir, "VALIDATION.md", "TODO TODO TODO");
        put(&std_dir, "reports/fitness.md", &body("Fitness"));
        put(&std_dir, "docs/adr/ADR-001.md", &body("ADR"));
        pack(&std_dir, Route::Standard).expect("pack");
        let v = verify(&std_dir).expect("verify");
        assert!(v.passed, "Standard — warn, не блокирует: {:?}", v.semantics);
        assert!(
            v.semantics
                .iter()
                .any(|f| f.rule == "evidence_stub" && f.severity == "warn"),
            "{:?}",
            v.semantics
        );
    }

    /// Отчёт с провалом внутри — `evidence_reports_fail`.
    #[test]
    fn verify_flags_failing_report() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path();
        put_complete_critical(dir);
        put(
            dir,
            "VALIDATION.md",
            &format!("{}\n\nИтог: FAIL\n", body("Прогон")),
        );
        pack(dir, Route::Critical).expect("pack");
        let v = verify(dir).expect("verify");
        assert!(!v.passed);
        assert!(
            v.semantics
                .iter()
                .any(|f| f.rule == "evidence_reports_fail"),
            "{:?}",
            v.semantics
        );
    }

    /// Baseline репетиции разошёлся с планом отката — evidence обесценено.
    #[test]
    fn verify_flags_stale_rehearsal_baseline() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path();
        put_complete_critical(dir);
        put(
            dir,
            ".arch-handoff/ROLLBACK.yaml",
            "baseline_commit: deadbeef\nsteps: []\n",
        );
        pack(dir, Route::Critical).expect("pack");
        let v = verify(dir).expect("verify");
        assert!(!v.passed);
        assert!(
            v.semantics
                .iter()
                .any(|f| f.rule == "rehearsal_stale_baseline"),
            "{:?}",
            v.semantics
        );
    }

    /// git-команда в песочнице теста; identity задаётся явно — в окружении CI
    /// её может не быть.
    fn git(dir: &Path, args: &[&str]) -> String {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["-c", "user.email=test@example.com", "-c", "user.name=test"])
            .args(args)
            .output()
            .expect("git запускается");
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// Д8: отчёт, объявленный пройденным, но без единого шага, — не
    /// репетиция. На Critical это блокирующая находка: пустой список шагов
    /// откат не подтверждает, а «passed: true» утверждает обратное.
    #[test]
    fn verify_flags_empty_rehearsal() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path();
        put_complete_critical(dir);
        put(
            dir,
            ".arch-handoff/REHEARSAL.json",
            r#"{"kind":"rollback_rehearsal","gate":"A4","passed":true,
                    "baseline_commit":"abc123","rehearsed_at":"2026-09-19T10:00:00Z",
                    "duration_secs":1.5,"steps":[],"verify":null,
                    "log":["репетиция отката прошла"]}"#,
        );
        pack(dir, Route::Critical).expect("pack");
        let v = verify(dir).expect("verify");
        assert!(!v.passed, "заготовка не имеет права давать зелёный");
        let found = v
            .semantics
            .iter()
            .find(|f| f.rule == "rehearsal_empty")
            .unwrap_or_else(|| panic!("находка rehearsal_empty: {:?}", v.semantics));
        assert_eq!(found.severity, "error", "Critical — блокирует выпуск");
        assert!(!found.fix_hint.is_empty(), "у находки есть подсказка");
    }

    /// Д8: якорь отката, не резолвящийся в коммит ЭТОГО репозитория,
    /// называется явно — но не блокирует: пакет мог быть собран в другой
    /// истории (так живут перенесённые кейсы `кейсы/*`), и обвинять их в
    /// подлоге нечем.
    #[test]
    fn verify_flags_unresolved_baseline() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path();
        put_complete_critical(dir);
        // Репозиторий есть, коммита `abc123` в нём нет.
        git(dir, &["init", "-q"]);
        pack(dir, Route::Critical).expect("pack");
        let v = verify(dir).expect("verify");
        let found = v
            .semantics
            .iter()
            .find(|f| f.rule == "rehearsal_baseline_unresolved")
            .unwrap_or_else(|| panic!("находка о нерезолвящемся якоре: {:?}", v.semantics));
        assert_eq!(found.severity, "warn", "не блокирует: другой репозиторий");
        assert!(found.message.contains("abc123"), "{}", found.message);
        assert!(v.passed, "warn выпуск не блокирует: {:?}", v.semantics);
    }

    /// Обратная сторона Д8: честная репетиция (шаги записаны, якорь
    /// резолвится) проходит без единого замечания о репетиции — усиление не
    /// наказывает того, кто откат действительно отрепетировал.
    #[test]
    fn real_rehearsal_still_passes() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path();
        put_complete_critical(dir);
        git(dir, &["init", "-q"]);
        git(dir, &["add", "-A"]);
        git(dir, &["commit", "-q", "-m", "каркас кейса"]);
        let head = git(dir, &["rev-parse", "HEAD"]);
        put(
            dir,
            ".arch-handoff/ROLLBACK.yaml",
            &format!(
                "baseline_commit: {head}\nsteps:\n  - name: якорь-доступен\n    \
                 run: \"true\"\nverify: \"true\"\n"
            ),
        );
        let report =
            crate::rehearsal::rehearse(dir, &dir.join(".arch-handoff")).expect("репетиция");
        assert!(report.passed, "{:?}", report.log);
        assert!(!report.steps.is_empty(), "шаги обязаны попасть в отчёт");
        pack(dir, Route::Critical).expect("pack");
        let v = verify(dir).expect("verify");
        assert!(
            v.semantics.iter().all(|f| !f.rule.starts_with("rehearsal")),
            "честная репетиция не даёт находок о себе: {:?}",
            v.semantics
        );
        assert!(v.passed, "{:?}", v.semantics);
    }

    #[tokio::test]
    async fn evidence_tools_soft_errors_on_missing_input() {
        let tmp = tempfile::tempdir().expect("tmp");
        let ctx = tool_ctx(tmp.path());
        // Без манифеста — мягкая ошибка verify.
        let out = EvidenceVerifyTool
            .call(json!({"change_dir": "."}), &ctx)
            .await
            .expect("вызов");
        assert!(out.is_error, "{}", out.content);
        // Неизвестный маршрут pack — мягкая ошибка, файл не создан.
        let out = EvidencePackTool
            .call(json!({"change_dir": ".", "route": "ludicrous"}), &ctx)
            .await
            .expect("вызов");
        assert!(out.is_error, "{}", out.content);
        assert!(!tmp.path().join("EVIDENCE.yaml").exists());
    }
    // --- прямые проверки хэшей, служебных файлов и строк отчёта --------------

    /// FNV-1a по эталонным векторам: смещение и простое для каждого байта.
    #[test]
    fn fnv1a64_matches_reference_vectors() {
        assert_eq!(fnv1a64(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a64(b"a"), 0xaf63_dc4c_8601_ec8c);
        assert_ne!(fnv1a64(b"abc"), fnv1a64(b"abd"));
    }

    /// Дефолт алгоритма для бандлов старого формата — именно legacy: иначе
    /// бандл без поля `hash_alg` не получил бы предупреждения о переупаковке.
    #[test]
    fn default_hash_alg_is_legacy() {
        assert_eq!(default_hash_alg(), HASH_ALG_LEGACY);
    }

    /// Временные и служебные файлы: бэкапы, `.DS_Store` и редакторские
    /// черновики — по расширению, без учёта регистра.
    #[test]
    fn is_transient_recognizes_each_form() {
        for name in ["notes.md~", ".DS_Store", "draft.TMP", "x.swp", "y.SWO"] {
            assert!(is_transient(name), "{name} — служебный");
        }
        for name in ["SPEC.md", "NOTES.txt", "data.json"] {
            assert!(!is_transient(name), "{name} — не служебный");
        }
    }

    /// Обход каталога: только файлы, рекурсивно, без `.git` и служебных.
    #[test]
    fn dir_files_returns_only_real_files_recursively() {
        let tmp = tempfile::tempdir().expect("tmp");
        let root = tmp.path();
        put(root, "a.md", "a");
        put(root, "b.md~", "b");
        put(root, "nested/c.md", "c");
        put(root, ".git/config", "git");
        let names: Vec<String> = dir_files(root)
            .iter()
            .map(|p| {
                p.strip_prefix(root)
                    .expect("внутри корня")
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect();
        assert_eq!(names, vec!["a.md", "nested/c.md"], "{names:?}");
    }

    /// Хэш каталога считает суммарный размер всех файлов (а не только
    /// последнего) и даёт sha256-дайджест.
    #[test]
    fn hash_artifact_sums_sizes_and_digests() {
        let tmp = tempfile::tempdir().expect("tmp");
        let root = tmp.path();
        put(root, "one.md", "12345");
        put(root, "two.md", "123");
        let (digest, size) = hash_artifact(root, HASH_ALG_SHA256).expect("хэш каталога");
        assert_eq!(size, 8);
        assert_eq!(digest.len(), 64, "{digest}");
        // Тот же каталог, переданный иначе («.», с завершающим слэшем) — тот же хэш.
        let (digest_dot, _) =
            hash_artifact(&root.join("."), HASH_ALG_SHA256).expect("хэш через точку");
        assert_eq!(digest, digest_dot, "путь-аргумент на вердикт не влияет");
    }

    /// Legacy-свёртка каталога: сверка с формулой по каждому файлу и
    /// накопление размера.
    #[test]
    fn hash_artifact_legacy_matches_formula() {
        use std::fmt::Write as _;
        let tmp = tempfile::tempdir().expect("tmp");
        let root = tmp.path();
        put(root, "a.md", "aa");
        put(root, "b.md", "bbb");
        let (digest, size) = hash_artifact_legacy(root).expect("legacy");
        assert_eq!(size, 5);
        let mut acc = String::new();
        for name in ["a.md", "b.md"] {
            let path = root.join(name);
            let bytes = std::fs::read(&path).expect("read");
            let _ = write!(acc, "{}:{:016x};", path.display(), fnv1a64(&bytes));
        }
        assert_eq!(digest, format!("{:016x}", fnv1a64(acc.as_bytes())));
    }

    /// Хэш одного файла legacy-алгоритмом — свёртка содержимого и его размер.
    #[test]
    fn hash_artifact_legacy_file_matches_content_hash() {
        let tmp = tempfile::tempdir().expect("tmp");
        let path = tmp.path().join("one.md");
        std::fs::write(&path, "content").expect("write");
        let (digest, size) = hash_artifact_legacy(&path).expect("legacy");
        assert_eq!(size, 7);
        assert_eq!(digest, format!("{:016x}", fnv1a64(b"content")));
    }

    /// `stub_file_of` называет проблемный файл: меньше порога — пустышка,
    /// ровно порог с осмысленным текстом — нет; маркер-заглушка ловится и в
    /// большом файле.
    #[test]
    fn stub_file_of_follows_threshold_strictly() {
        let tmp = tempfile::tempdir().expect("tmp");
        let root = tmp.path();
        let min = 10_u64;
        put(root, "exact.md", &"x".repeat(min as usize));
        assert!(
            stub_file_of(root, min).is_none(),
            "ровно порог — не пустышка"
        );
        std::fs::write(root.join("exact.md"), "x".repeat(min as usize - 1)).expect("write");
        assert!(
            stub_file_of(root, min).is_some(),
            "меньше порога — пустышка"
        );
        std::fs::write(
            root.join("exact.md"),
            format!(
                "{} TODO {}",
                "y".repeat(min as usize),
                "z".repeat(min as usize)
            ),
        )
        .expect("write");
        assert!(
            stub_file_of(root, min).is_some(),
            "маркер-заглушка видна и в большом файле"
        );
        // Файл — не каталог: адресного поиска нет.
        let file = root.join("exact.md");
        assert!(stub_file_of(&file, min).is_none());
    }

    /// Пустые значения и маркеры-заглушки поля A3.
    #[test]
    fn field_is_empty_recognizes_empty_and_placeholders() {
        for value in ["", "   ", "—", "–", "-", "?", "TBD", "TODO", "нет", "n/a"] {
            assert!(field_is_empty(value), "«{value}» — пустое");
        }
        assert!(!field_is_empty("Вариант Б: свой шлюз"));
    }

    /// Строка провала — любая из трёх форм, включая английскую.
    #[test]
    fn has_fail_line_recognizes_each_form() {
        assert!(has_fail_line("Итог: FAIL"));
        assert!(has_fail_line("fail"));
        assert!(has_fail_line("FAIL — есть дефекты"));
        assert!(!has_fail_line("PASS (3 из 5)"));
    }

    /// A1: строка «Итог: PASS» в прозе больше не удостоверяет прогон — у
    /// артефактов прогонов её не требуют и ей не верят: прогон доказывает
    /// машинная запись (`evidence_report_unbound`/`evidence_record_*`).
    #[test]
    fn handwritten_result_line_is_not_evidence_anymore() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path();
        let cfg = crate::config::EvidenceConfig::default();
        let report = dir.join("VALIDATION.md");
        std::fs::write(
            &report,
            format!("# Отчёт\n\n{}\n\nИтог: PASS (8 из 8)\n", body("Прогон")),
        )
        .expect("write");
        let (findings, _notes) = semantic_check(dir, "validation", &report, &cfg, "error");
        assert!(
            findings.is_empty(),
            "проза-проверка не краснит написанный отчёт — её роль теперь у записи: {findings:?}"
        );
        // А проверка записи (без markdown-строки итога) называет отсутствие
        // машинного подтверждения — см. verify_flags_unbound_handwritten_report.
    }

    /// Прогресс бандла считается по манифесту: сколько обязательных ключей
    /// маршрута уже описано.
    #[test]
    fn bundle_progress_counts_manifest_keys() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path();
        put(dir, "PROBLEM.md", "проблема");
        let (_bundle, _v) = pack(dir, Route::Fast).expect("pack");
        let required = required_artifacts(Route::Fast);
        // Ожидание считается по тому же манифесту: прогресс — сколько
        // обязательных ключей маршрута в нём описано.
        let text = std::fs::read_to_string(dir.join("EVIDENCE.yaml")).expect("манифест");
        let bundle: EvidenceBundle = serde_yaml_ng::from_str(&text).expect("yaml");
        let expected = required
            .iter()
            .filter(|(key, _)| bundle.items.iter().any(|i| &i.key == key))
            .count();
        assert!(
            expected > 0 && expected < required.len(),
            "фикстура неполная: {expected} из {}",
            required.len()
        );
        assert_eq!(
            bundle_progress(dir, Route::Fast),
            Some(expected),
            "прогресс по манифесту: {} обязательных ключей",
            required.len()
        );
        // Каталог без манифеста — прогресса нет.
        let empty = tmp.path().join("empty");
        std::fs::create_dir_all(&empty).expect("mkdir");
        assert!(bundle_progress(&empty, Route::Fast).is_none());
    }
    /// Порог размера строгий: ровно порог — артефакт написан, на байт меньше —
    /// пустышка.
    #[test]
    fn semantic_size_threshold_is_strict() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path();
        let cfg = crate::config::EvidenceConfig::default();
        let min = cfg.min_bytes as usize;
        let artifact = dir.join("SPEC.md");
        std::fs::write(&artifact, "s".repeat(min)).expect("write");
        let (findings, _notes) = semantic_check(dir, "spec", &artifact, &cfg, "error");
        assert!(
            !findings.iter().any(|f| f.rule == "evidence_stub"),
            "ровно порог — не пустышка: {findings:?}"
        );
        std::fs::write(&artifact, "s".repeat(min - 1)).expect("write");
        let (findings, _notes) = semantic_check(dir, "spec", &artifact, &cfg, "error");
        assert!(
            findings.iter().any(|f| f.rule == "evidence_stub"),
            "меньше порога — пустышка: {findings:?}"
        );
    }

    /// Заглушка A3 не разбирается по полям: у неё один честный диагноз
    /// (не написан), а не пять «поле не заполнено».
    #[test]
    fn semantic_stub_a3_is_not_checked_field_by_field() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path();
        let cfg = crate::config::EvidenceConfig::default();
        let a3 = dir.join("DECISION.md");
        std::fs::write(&a3, "TODO").expect("write");
        let (findings, _notes) = semantic_check(dir, "decision_a3", &a3, &cfg, "error");
        assert!(
            findings.iter().any(|f| f.rule == "evidence_stub"),
            "заглушка названа: {findings:?}"
        );
        assert!(
            !findings.iter().any(|f| f.rule == "a3_not_signed"),
            "поля заглушки не разбираются: {findings:?}"
        );
    }

    /// Подпись A3 называется ровно один раз — по полю `decided_by`, а не по
    /// каждому заполненному полю.
    #[test]
    fn semantic_a3_signature_note_names_only_decided_by() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path();
        let cfg = crate::config::EvidenceConfig::default();
        put_complete_critical(dir);
        let a3 = dir.join("DECISION.md");
        let (findings, notes) = semantic_check(dir, "decision_a3", &a3, &cfg, "error");
        assert!(
            !findings.iter().any(|f| f.rule == "a3_not_signed"),
            "полный A3 подписан: {findings:?}"
        );
        let signatures: Vec<&String> = notes.iter().filter(|n| n.contains("подпись A3")).collect();
        assert_eq!(signatures.len(), 1, "подпись названа один раз: {notes:?}");
    }

    /// Срок A3 «сегодня» — ещё не просрочен: сравнение строгое.
    #[test]
    fn semantic_a3_expiry_today_is_not_expired() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path();
        let cfg = crate::config::EvidenceConfig::default();
        put_complete_critical(dir);
        let today = chrono::Local::now().date_naive();
        let text = std::fs::read_to_string(dir.join("DECISION.md")).expect("read");
        let text = text
            .lines()
            .map(|l| {
                if l.contains("expiry") {
                    format!("- **expiry**: {today}\n")
                } else {
                    format!("{l}\n")
                }
            })
            .collect::<String>();
        std::fs::write(dir.join("DECISION.md"), text).expect("write");
        let a3 = dir.join("DECISION.md");
        let (findings, _notes) = semantic_check(dir, "decision_a3", &a3, &cfg, "error");
        assert!(
            !findings.iter().any(|f| f.rule == "a3_expired"),
            "срок истекает сегодня — не просрочен: {findings:?}"
        );
    }

    /// Отчёт-заглушка без машинной записи: диагноз уже назван («не написан»),
    /// а с A1 рядом стоит `evidence_report_unbound` (на уровне verify) —
    /// требовать ещё и строку итога у прозы — шум, прогон закрывает запись.
    #[test]
    fn semantic_stub_report_does_not_require_result_line() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path();
        let cfg = crate::config::EvidenceConfig::default();
        let report = dir.join("VALIDATION.md");
        std::fs::write(&report, "TODO").expect("write");
        let (findings, _notes) = semantic_check(dir, "validation", &report, &cfg, "warn");
        assert!(
            findings.iter().any(|f| f.rule == "evidence_stub"),
            "заглушка названа: {findings:?}"
        );
        assert!(
            !findings
                .iter()
                .any(|f| f.message.contains("нет строки итога")),
            "строку итога у заглушки не требуем: {findings:?}"
        );
    }

    /// Заглушка репетиции отката не разбирается как отчёт: у неё нет ни
    /// списка шагов, ни якоря, и требовать их — шум.
    #[test]
    fn semantic_stub_rehearsal_is_not_parsed_as_report() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path();
        let cfg = crate::config::EvidenceConfig::default();
        // Рядом лежит отчёт-заготовка: без охранной ветки её разбор дал бы
        // находку `rehearsal_empty` поверх честного «не написан».
        put(
            dir,
            "REHEARSAL.json",
            r#"{"kind":"rollback_rehearsal","gate":"A4","passed":true,
                    "baseline_commit":"abc123","rehearsed_at":"2026-09-19T10:00:00Z",
                    "duration_secs":1.5,"steps":[],"verify":null,
                    "log":["репетиция отката прошла"]}"#,
        );
        let rehearsal = dir.join("ROLLBACK-REHEARSAL.md");
        std::fs::write(&rehearsal, "TODO").expect("write");
        let (findings, _notes) =
            semantic_check(dir, "rollback_rehearsal", &rehearsal, &cfg, "error");
        assert!(
            !findings.iter().any(|f| f.rule == "rehearsal_empty"),
            "заглушку не разбираем как отчёт: {findings:?}"
        );
        assert!(
            findings.iter().any(|f| f.rule == "evidence_stub"),
            "заглушка названа: {findings:?}"
        );
        let extra: Vec<&SemanticFinding> = findings
            .iter()
            .filter(|f| f.rule != "evidence_stub")
            .collect();
        assert!(extra.is_empty(), "лишних требований нет: {extra:?}");
    }

    /// Обязательные артефакты маршрута, которых нет в манифесте, попадают в
    /// `missing` поимённо.
    #[test]
    fn verify_lists_missing_required_artifacts() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path();
        put(dir, "PROBLEM.md", &body("Проблема"));
        pack(dir, Route::Critical).expect("pack");
        let v = verify(dir).expect("verify");
        assert!(!v.passed);
        for key in ["decision_a3", "spine", "adversarial_review"] {
            assert!(
                v.missing.contains(&key.to_string()),
                "{key} назван: {:?}",
                v.missing
            );
        }
    }

    /// A1 (0.3.14, репродукция «рукописный PASS»): отчёт о прогоне, написанный
    /// прозой без машинной записи (`arch-be evidence record`), не доказывает
    /// прогон — обязана быть находка `evidence_report_unbound`.
    #[test]
    fn verify_flags_unbound_handwritten_report() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path();
        put_complete_critical(dir);
        pack(dir, Route::Critical).expect("pack");
        let v = verify(dir).expect("verify");
        for key in ["validation", "fitness_report", "walking_skeleton"] {
            assert!(
                v.semantics
                    .iter()
                    .any(|f| f.rule == "evidence_report_unbound" && f.key == key),
                "рукописный отчёт «{key}» без машинной записи обязан быть назван: {:?}",
                v.semantics
            );
        }
        // По умолчанию — warn (не блокирует), по флагу require_records — error.
        assert!(v.passed, "warn не блокирует выпуск: {:?}", v.semantics);
        let strict = crate::config::EvidenceConfig {
            require_records: true,
            ..crate::config::EvidenceConfig::default()
        };
        let v = verify_with(dir, &strict).expect("verify");
        assert!(
            !v.passed,
            "с require_records=true рукописный PASS блокируется: {:?}",
            v.semantics
        );
        assert!(
            v.semantics
                .iter()
                .any(|f| f.rule == "evidence_report_unbound" && f.severity == "error"),
            "{:?}",
            v.semantics
        );
    }

    /// Запись прогона: команда, exit-код, HEAD, хэш входов, итог — в файле
    /// `.arch-handoff/evidence/<kind>.json`.
    #[test]
    fn record_run_writes_machine_record() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path();
        put(dir, "src/main.py", "print('ok')\n");
        let rec = record_run(dir, RecordKind::Tests, "true", 0).expect("record");
        assert!(rec.passed);
        assert_eq!(rec.exit_code, Some(0));
        assert_eq!(rec.schema, RECORD_SCHEMA);
        assert_eq!(rec.kind, "tests");
        assert_eq!(rec.head, "absent", "не git-репозиторий");
        assert_eq!(rec.inputs_hash.len(), 64, "sha256 hex");
        assert_eq!(rec.attestation.len(), 64);
        let path = record_path(dir, RecordKind::Tests);
        assert!(path.is_file(), "запись записана: {}", path.display());
        let loaded = load_run_record(dir, RecordKind::Tests)
            .expect("load")
            .expect("запись есть");
        assert_eq!(loaded.inputs_hash, rec.inputs_hash);
        // Детерминизм: повторный прогон на неизменном дереве — тот же хэш входов.
        let rec2 = record_run(dir, RecordKind::Tests, "true", 0).expect("record");
        assert_eq!(rec.inputs_hash, rec2.inputs_hash);
    }

    /// Провал прогона — тоже запись (passed=false, честный exit-код).
    #[test]
    fn record_run_failed_command_records_fail() {
        let tmp = tempfile::tempdir().expect("tmp");
        let rec = record_run(tmp.path(), RecordKind::Fitness, "false", 0).expect("record");
        assert!(!rec.passed);
        assert_eq!(rec.exit_code, Some(1));
    }

    /// Битая запись — ошибка разбора, а не молчание (подмена не замалчивается).
    #[test]
    fn load_run_record_rejects_garbage() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path();
        put(dir, ".arch-handoff/evidence/tests.json", "{ не json");
        assert!(load_run_record(dir, RecordKind::Tests).is_err());
    }

    /// Полный цикл A1: запись без markdown закрывает артефакт; правка кода
    /// после записи делает её устаревшей (`evidence_record_stale`).
    #[test]
    fn verify_accepts_fresh_record_and_flags_stale_after_code_edit() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path();
        put_complete_critical(dir);
        // Мир A1: отчёты прогонов — машинные записи, markdown не пишется.
        for rel in ["VALIDATION.md", "reports/fitness.md", "WALKING-SKELETON.md"] {
            std::fs::remove_file(dir.join(rel)).expect("remove");
        }
        for kind in RecordKind::all() {
            record_run(dir, kind, "true", 0).expect("record");
        }
        let (bundle, v) = pack(dir, Route::Critical).expect("pack");
        assert!(v.passed, "missing: {:?}", v.missing);
        // Артефактом стала сама запись, а не проза.
        assert!(
            bundle
                .items
                .iter()
                .any(|i| i.key == "validation" && i.path.ends_with("evidence/tests.json")),
            "{:?}",
            bundle.items.iter().map(|i| &i.path).collect::<Vec<_>>()
        );
        let v = verify(dir).expect("verify");
        assert!(
            v.passed,
            "свежие записи закрывают артефакты без markdown: {:?}",
            v.semantics
        );
        assert!(
            v.semantics
                .iter()
                .all(|f| !f.rule.starts_with("evidence_record")
                    && f.rule != "evidence_report_unbound"),
            "{:?}",
            v.semantics
        );
        // Правка кода после записи → запись устарела (приёмка A1).
        put(dir, "src/new_module.py", "def f(): ...\n");
        let v = verify(dir).expect("verify");
        assert!(
            !v.passed,
            "устаревшая запись обязана блокировать: {:?}",
            v.semantics
        );
        assert!(
            v.semantics
                .iter()
                .any(|f| f.rule == "evidence_record_stale" && f.key == "validation"),
            "{:?}",
            v.semantics
        );
    }

    /// Запись с провалом прогона — находка, даже когда рядом проза говорит PASS.
    #[test]
    fn verify_flags_failed_record_over_prose_pass() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path();
        put_complete_critical(dir);
        record_run(dir, RecordKind::Tests, "false", 0).expect("record");
        pack(dir, Route::Critical).expect("pack");
        let v = verify(dir).expect("verify");
        assert!(
            v.semantics
                .iter()
                .any(|f| f.rule == "evidence_record_failed" && f.key == "validation"),
            "{:?}",
            v.semantics
        );
        assert!(!v.passed);
    }

    /// Ни одна находка по артефактам прогонов не советует дописать текст,
    /// закрывающий её без прогона (приёмка A1): все подсказки ведут к
    /// `arch-be evidence record`.
    #[test]
    fn report_findings_never_teach_prose_bypass() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path();
        put_complete_critical(dir);
        // Разносим типы находок: проза-PASS, заглушка, FAIL в тексте.
        put(dir, "reports/fitness.md", "TODO");
        put(
            dir,
            "WALKING-SKELETON.md",
            &format!("{}\n\nИтог: FAIL\n", body("Скелет")),
        );
        record_run(dir, RecordKind::Fitness, "true", 0).expect("record");
        // Устаревшая запись: код изменился после прогона.
        put(dir, "src/changed.py", "x = 1\n");
        pack(dir, Route::Critical).expect("pack");
        let v = verify(dir).expect("verify");
        let report_findings: Vec<&SemanticFinding> = v
            .semantics
            .iter()
            .filter(|f| RecordKind::for_artifact(&f.key).is_some())
            .collect();
        assert!(
            report_findings.len() >= 4,
            "ожидались находки всех видов: {report_findings:?}"
        );
        for f in report_findings {
            assert!(
                f.fix_hint.contains("evidence record"),
                "подсказка обязана вести к машинной записи, а не к тексту: {f:?}"
            );
            for banned in ["добавьте", "допишите", "строку итога"] {
                assert!(
                    !f.fix_hint.contains(banned),
                    "подсказка не учит обходу («{banned}»): {f:?}"
                );
            }
        }
    }
}
