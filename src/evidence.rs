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

/// Результат проверки.
#[derive(Debug, Clone)]
pub struct EvidenceVerdict {
    /// Полнота и целостность подтверждены.
    pub passed: bool,
    /// Отсутствующие обязательные артефакты.
    pub missing: Vec<String>,
    /// Артефакты с изменённым хэшем (подмена/дрейф после упаковки).
    pub tampered: Vec<String>,
    /// Предупреждения, не блокирующие выпуск (бандл старого формата и т.п.).
    pub warnings: Vec<String>,
    /// Сводка для отчёта.
    pub summary: String,
}

/// Обязательные артефакты по маршруту (из обзора AI-Disrupt: объектный минимум
/// разделяют все режимы; Standard/Critical добавляют evidence-проверки).
fn required_artifacts(route: Route) -> Vec<(&'static str, &'static str)> {
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
    name.ends_with('~')
        || name.ends_with(".tmp")
        || name.ends_with(".swp")
        || name.ends_with(".swo")
        || name == ".DS_Store"
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
        let _ = write!(acc, "{rel}\0{h}\n");
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
        match find_artifact(change_dir, key) {
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
    };
    Ok((bundle, verdict))
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
    let passed = missing.is_empty() && tampered.is_empty();
    Ok(EvidenceVerdict {
        passed,
        summary: format!(
            "Проверка bundle ({}): артефактов {}, отсутствует {}, изменено {}",
            bundle.route,
            bundle.items.len(),
            missing.len(),
            tampered.len()
        ),
        missing,
        tampered,
        warnings,
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
                    "change_dir": {"type": "string", "description": "Каталог изменения с EVIDENCE.yaml"}
                },
                "required": ["change_dir"]
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
        let verdict = match verify(&dir) {
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
            .collect();
        let out = json!({
            "tool": "evidence_verify",
            "passed": verdict.passed,
            "issues": issues,
            "warnings": verdict.warnings,
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
                    "change_dir": {"type": "string", "description": "Каталог изменения"},
                    "route": {
                        "type": "string",
                        "description": "Маршрут: fast | standard | critical (по умолчанию standard)",
                        "enum": ["fast", "standard", "critical"]
                    }
                },
                "required": ["change_dir"]
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
        put(dir, "PROBLEM.md", "# Проблема\n");
        put(
            dir,
            "SPEC.md",
            "## Проблема\n## Критерии приёмки\n## Риски\n",
        );
        put(dir, "RISK.md", "Fast: 0\n");
        put(dir, "ROLLBACK.md", "git revert\n");
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
}
