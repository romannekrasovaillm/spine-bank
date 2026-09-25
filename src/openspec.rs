//! Адаптер `OpenSpec` (MVP): Spine читает репозиторий с разметкой `OpenSpec`
//! как источник требований и архитектурных решений, а вердикт выносит своим
//! детерминированным ядром. Не форк и не зависимость от `OpenSpec` CLI:
//! адаптер только ЧИТАЕТ их файлы (`openspec/specs/`, `openspec/changes/`)
//! и маппит на свои артефакты (`CONSTRAINTS.yaml`, спайн).
//!
//! Контракт модуля:
//! - [`scan_requirements`] — требования с SHALL/MUST из живых спек
//!   (`openspec/specs/<capability>/spec.md`) и дельт активных changes
//!   (`openspec/changes/<id>/specs/`); стабильный идентификатор
//!   [`requirement_id`] — `openspec:<capability>#<hash8>` от capability и
//!   нормализованного текста требования (заголовки `### Requirement:` в
//!   хэш НЕ входят: их ids нестабильны, переименование заголовка не ломает
//!   связь «правило ← требование»);
//! - [`coverage`] — отчёт покрытия: SHALL всего / покрыто детектором /
//!   unverifiable с owner / без решения. Связь — на своей стороне: правило
//!   `CONSTRAINTS.yaml` перечисляет идентификаторы в поле `covers:`
//!   (markdown `OpenSpec` не трогаем — иначе сломается их `validate --strict`);
//! - [`init`] — генерация скелета `CONSTRAINTS.from-openspec.yaml` (все
//!   найденные SHALL как заглушки `unverifiable: true` с пустым owner и
//!   проставленным `covers:`) и `SPINE.draft.md` (кандидаты из секций
//!   Decisions/Constraints design.md активных changes — в очередь на
//!   подтверждение архитектором, НЕ автоматом; archive — как история);
//! - [`gate_archive`] — гейт архивации change: FAIL (exit 1 на CLI-краю),
//!   если у требований change нет решения (детектор или unverifiable с
//!   owner) или падает `control check`.
//!
//! Чего адаптер НЕ делает (зафиксировано в `docs/openspec.md`): не парсит
//! скиллы/slash-команды `OpenSpec`, не переписывает их файлы (единственные
//! записываемые артефакты — свои `CONSTRAINTS.from-openspec.yaml` и
//! `SPINE.draft.md`), не требует установленного `OpenSpec`. `OpenSpec`
//! необязателен: адаптер живёт за своей подкомандой, ядро о нём не знает.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use walkdir::WalkDir;

use crate::error::{HarnessError, Result};
use crate::llm::ToolSpec;
use crate::tool::{Tool, ToolContext, ToolOutput};

/// Префикс стабильного идентификатора требования `OpenSpec`.
pub const ID_PREFIX: &str = "openspec:";

/// Происхождение требования: живая спека или дельта активного change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ReqSource {
    /// Живая спецификация (`openspec/specs/<capability>/spec.md`).
    Spec,
    /// Дельта активного change (`openspec/changes/<change>/specs/`).
    Change {
        /// Идентификатор change (имя каталога).
        change: String,
    },
}

/// Требование `OpenSpec` с хотя бы одной строкой SHALL/MUST.
#[derive(Debug, Clone, Serialize)]
pub struct Requirement {
    /// Стабильный идентификатор `openspec:<capability>#<hash8>`.
    pub id: String,
    /// Capability (имя каталога спеки).
    pub capability: String,
    /// Заголовок `### Requirement:` (в хэш не входит — может меняться).
    pub title: String,
    /// Строки с SHALL/MUST (нормализованный текст — основа идентификатора).
    pub statements: Vec<String>,
    /// Файл-источник (относительно корня репозитория).
    pub file: PathBuf,
    /// Строка заголовка требования (1-based).
    pub line: usize,
    /// Происхождение (живая спека / дельта change).
    pub source: ReqSource,
}

/// Отчёт сканирования (JSON-контракт `openspec scan --json`).
#[derive(Debug, Clone, Serialize)]
pub struct ScanReport {
    /// Корень репозитория.
    pub root: PathBuf,
    /// Число найденных требований.
    pub total: usize,
    /// Требования (порядок детерминирован: файл, строка).
    pub requirements: Vec<Requirement>,
}

/// FNV-1a 64: детерминированный хэш на stdlib, без новых зависимостей.
/// `DefaultHasher` не подходит: его вывод не гарантированно стабилен
/// между версиями toolchain, а идентификатор живёт в `covers:` правил.
fn fnv1a64(data: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in data.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// Нормализует строку требования для хэширования: схлопывает пробельные
/// последовательности (переносы строк и отступы markdown не влияют на id).
fn normalize_statement(line: &str) -> String {
    line.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Стабильный идентификатор требования: `openspec:<capability>#<hash8>`,
/// где hash8 — младшие 32 бита FNV-1a от capability и нормализованного
/// текста строк SHALL/MUST. Заголовок требования в хэш не входит:
/// переименование `### Requirement:` сохраняет идентификатор, а изменение
/// текста требования его (осознанно) меняет — правило с `covers:` перестаёт
/// покрывать редактированное требование, что видно в отчёте покрытия.
#[must_use]
pub fn requirement_id(capability: &str, statements: &[String]) -> String {
    let normalized = statements
        .iter()
        .map(|s| normalize_statement(s))
        .collect::<Vec<_>>()
        .join("\n");
    let hash = fnv1a64(&format!("{capability}\n{normalized}")) & 0xFFFF_FFFF;
    format!("{ID_PREFIX}{capability}#{hash:08x}")
}

/// Компилирует статический regex адаптера (ошибка компиляции — внутренний дефект).
fn os_regex(pattern: &str) -> Result<Regex> {
    Regex::new(pattern)
        .map_err(|e| HarnessError::Control(format!("внутренний regex openspec-адаптера: {e}")))
}

/// Сырое требование из markdown-файла (до присвоения идентификатора).
struct RawRequirement {
    /// Заголовок `### Requirement:`.
    title: String,
    /// Строка заголовка (1-based).
    line: usize,
    /// Строки с SHALL/MUST.
    statements: Vec<String>,
}

/// Разбирает markdown-файл спеки: блок требования начинается заголовком
/// `### Requirement: <title>` и заканчивается любым заголовком уровня 1–4
/// (включая `#### Scenario:` — сценарии в тело требования не входят).
/// Требования без строк SHALL/MUST отбрасываются.
fn parse_requirements(content: &str) -> Result<Vec<RawRequirement>> {
    let re_req = os_regex(r"^###\s+Requirement:\s*(.+?)\s*$")?;
    let re_heading = os_regex(r"^#{1,4}\s")?;
    let re_shall = os_regex(r"\b(?:SHALL|MUST)\b")?;

    let mut out: Vec<RawRequirement> = Vec::new();
    let mut current: Option<RawRequirement> = None;
    for (idx, line) in content.lines().enumerate() {
        if let Some(caps) = re_req.captures(line) {
            if let Some(done) = current.take() {
                out.push(done);
            }
            current = Some(RawRequirement {
                title: caps[1].to_string(),
                line: idx + 1,
                statements: Vec::new(),
            });
            continue;
        }
        if re_heading.is_match(line) {
            if let Some(done) = current.take() {
                out.push(done);
            }
            continue;
        }
        if let Some(cur) = current.as_mut() {
            if re_shall.is_match(line) {
                cur.statements.push(line.trim().to_string());
            }
        }
    }
    if let Some(done) = current.take() {
        out.push(done);
    }
    out.retain(|r| !r.statements.is_empty());
    Ok(out)
}

/// Сценарий проверки `#### Scenario: <title>` под требованием (E11.1): строки
/// WHEN/THEN — то, что судья проверяет по коду ОТДЕЛЬНО от требования целиком.
#[derive(Debug, Clone, Serialize)]
pub struct Scenario {
    /// Стабильный идентификатор `openspec:<capability>#<hash8>/S<n>`: хэш
    /// требования плюс номер сценария внутри него.
    pub id: String,
    /// Идентификатор требования, к которому сценарий относится.
    pub requirement_id: String,
    /// Заголовок `#### Scenario:`.
    pub title: String,
    /// Тело сценария: строки WHEN/THEN/AND (обрезка, ведущие маркеры сняты).
    pub steps: Vec<String>,
    /// Файл-источник относительно корня репозитория.
    pub file: PathBuf,
    /// Строка заголовка сценария (1-based).
    pub line: usize,
}

/// Требование файла спеки во время разбора сценариев: строки SHALL/MUST и
/// найденные под ним сценарии `(заголовок, строка, шаги)`.
struct Block {
    statements: Vec<String>,
    scenarios: Vec<(String, usize, Vec<String>)>,
}

/// Разбирает сценарии одного файла спеки: каждый привязан к требованию, под
/// которым записан. Требование без строк SHALL/MUST отбрасывается вместе со
/// сценариями — идентификатор требования строится из этих строк, и без них
/// сценарий не к чему привязать.
fn parse_scenarios(content: &str, capability: &str, file: &Path) -> Result<Vec<Scenario>> {
    let re_req = os_regex(r"^###\s+Requirement:\s*(.+?)\s*$")?;
    let re_scenario = os_regex(r"^####\s+Scenario:\s*(.+?)\s*$")?;
    let re_heading = os_regex(r"^#{1,4}\s")?;
    let re_shall = os_regex(r"\b(?:SHALL|MUST)\b")?;

    let mut blocks: Vec<Block> = Vec::new();
    let mut current: Option<Block> = None;
    let mut open_scenario: Option<(String, usize, Vec<String>)> = None;

    let close_scenario = |open: Option<(String, usize, Vec<String>)>,
                          current: &mut Option<Block>| {
        if let (Some(scenario), Some(block)) = (open, current.as_mut()) {
            block.scenarios.push(scenario);
        }
    };

    for (idx, line) in content.lines().enumerate() {
        if re_req.is_match(line) {
            close_scenario(open_scenario.take(), &mut current);
            if let Some(done) = current.take() {
                blocks.push(done);
            }
            current = Some(Block {
                statements: Vec::new(),
                scenarios: Vec::new(),
            });
            continue;
        }
        if let Some(caps) = re_scenario.captures(line) {
            close_scenario(open_scenario.take(), &mut current);
            open_scenario = Some((caps[1].to_string(), idx + 1, Vec::new()));
            continue;
        }
        if re_heading.is_match(line) {
            // Заголовок уровня ≤ 3 закрывает и сценарий, и требование.
            close_scenario(open_scenario.take(), &mut current);
            if let Some(done) = current.take() {
                blocks.push(done);
            }
            continue;
        }
        if let Some((_, _, steps)) = open_scenario.as_mut() {
            let trimmed = line.trim().trim_start_matches(['-', '*', ' ']).trim();
            if !trimmed.is_empty() {
                steps.push(trimmed.to_string());
            }
        } else if let Some(block) = current.as_mut() {
            if re_shall.is_match(line) {
                block.statements.push(line.trim().to_string());
            }
        }
    }
    close_scenario(open_scenario.take(), &mut current);
    if let Some(done) = current.take() {
        blocks.push(done);
    }

    let mut out = Vec::new();
    for block in blocks {
        if block.statements.is_empty() {
            continue; // требование без SHALL/MUST — сценарии к нему не крепятся
        }
        let requirement = requirement_id(capability, &block.statements);
        for (n, (title, line, steps)) in block.scenarios.into_iter().enumerate() {
            out.push(Scenario {
                id: format!("{requirement}/S{}", n + 1),
                requirement_id: requirement.clone(),
                title,
                steps,
                file: file.to_path_buf(),
                line,
            });
        }
    }
    Ok(out)
}

/// Сканирует сценарии проверки (E11.2): те же файлы `OpenSpec`, что у
/// требований, — живые спеки и дельты активных changes. Порядок
/// детерминирован (файл, строка).
///
/// # Errors
/// `openspec/specs` отсутствует, файл спеки не читается.
pub fn scan_scenarios(root: &Path) -> Result<Vec<Scenario>> {
    let specs_dir = root.join("openspec/specs");
    let mut files: Vec<PathBuf> = Vec::new();
    if specs_dir.is_dir() {
        files.extend(collect_md(&specs_dir));
    }
    let changes_dir = root.join("openspec/changes");
    if changes_dir.is_dir() {
        let mut change_dirs: Vec<PathBuf> = std::fs::read_dir(&changes_dir)
            .map_err(|e| HarnessError::io(&changes_dir, e))?
            .filter_map(std::result::Result::ok)
            .map(|e| e.path())
            .filter(|p| p.is_dir() && p.file_name().is_some_and(|n| n != "archive"))
            .collect();
        change_dirs.sort();
        for change in change_dirs {
            files.extend(collect_md(&change.join("specs")));
        }
    }
    if files.is_empty() {
        return Err(HarnessError::Control(format!(
            "openspec/specs не найден в {} — адаптер читает репозиторий с разметкой OpenSpec",
            root.display()
        )));
    }
    let mut out = Vec::new();
    for file in files {
        let content = std::fs::read_to_string(&file).map_err(|e| HarnessError::io(&file, e))?;
        let capability = file
            .parent()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let rel = file.strip_prefix(root).unwrap_or(&file).to_path_buf();
        out.extend(parse_scenarios(&content, &capability, &rel)?);
    }
    out.sort_by(|a, b| a.file.cmp(&b.file).then(a.line.cmp(&b.line)));
    Ok(out)
}

/// Собирает markdown-файлы каталога (рекурсивно, детерминированный порядок).
fn collect_md(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = WalkDir::new(dir)
        .into_iter()
        .filter_map(std::result::Result::ok)
        .filter(|e| e.file_type().is_file())
        .map(walkdir::DirEntry::into_path)
        .filter(|p| p.extension().is_some_and(|ext| ext == "md"))
        .collect();
    files.sort();
    files
}

/// Разбирает один файл спеки и добавляет требования в выходной вектор.
/// `capability` — имя родительского каталога `spec.md`.
fn parse_spec_file(
    path: &Path,
    root: &Path,
    source: &ReqSource,
    out: &mut Vec<Requirement>,
) -> Result<()> {
    let content = std::fs::read_to_string(path).map_err(|e| HarnessError::io(path, e))?;
    let capability = path
        .parent()
        .and_then(|p| p.file_name())
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let rel = path.strip_prefix(root).unwrap_or(path).to_path_buf();
    for raw in parse_requirements(&content)? {
        out.push(Requirement {
            id: requirement_id(&capability, &raw.statements),
            capability: capability.clone(),
            title: raw.title,
            statements: raw.statements,
            file: rel.clone(),
            line: raw.line,
            source: source.clone(),
        });
    }
    Ok(())
}

/// Сканирует репозиторий с разметкой `OpenSpec`: живые спеки
/// (`openspec/specs/**/*.md`) и дельты активных changes
/// (`openspec/changes/<id>/specs/**/*.md`; `changes/archive/` — история,
/// не сканируется). Порядок результата детерминирован (файл, строка).
///
/// # Errors
/// `openspec/specs` отсутствует (репозиторий без разметки `OpenSpec`),
/// файл спеки не читается.
pub fn scan_requirements(root: &Path) -> Result<Vec<Requirement>> {
    let specs_dir = root.join("openspec/specs");
    if !specs_dir.is_dir() {
        return Err(HarnessError::Control(format!(
            "openspec/specs не найден в {} — адаптер читает репозиторий с разметкой OpenSpec",
            root.display()
        )));
    }
    let mut out = Vec::new();
    for file in collect_md(&specs_dir) {
        parse_spec_file(&file, root, &ReqSource::Spec, &mut out)?;
    }
    let changes_dir = root.join("openspec/changes");
    if changes_dir.is_dir() {
        let mut change_dirs: Vec<PathBuf> = std::fs::read_dir(&changes_dir)
            .map_err(|e| HarnessError::io(&changes_dir, e))?
            .filter_map(std::result::Result::ok)
            .map(|e| e.path())
            .filter(|p| p.is_dir() && p.file_name().is_some_and(|n| n != "archive"))
            .collect();
        change_dirs.sort();
        for change in change_dirs {
            let id = change
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            let source = ReqSource::Change { change: id };
            for file in collect_md(&change.join("specs")) {
                parse_spec_file(&file, root, &source, &mut out)?;
            }
        }
    }
    out.sort_by(|a, b| a.file.cmp(&b.file).then(a.line.cmp(&b.line)));
    Ok(out)
}

/// Рендерит отчёт сканирования в markdown.
#[must_use]
pub fn render_scan(root: &Path, requirements: &[Requirement]) -> String {
    let specs = requirements
        .iter()
        .filter(|r| r.source == ReqSource::Spec)
        .count();
    let mut out = String::new();
    let _ = writeln!(out, "# OpenSpec scan: {}", root.display());
    let _ = writeln!(
        out,
        "\nТребований: {} (specs: {specs}, дельты changes: {})",
        requirements.len(),
        requirements.len() - specs
    );
    for r in requirements {
        let _ = writeln!(out, "\n## {} — {}", r.id, r.capability);
        let _ = writeln!(out, "- Требование: {}", r.title);
        let origin = match &r.source {
            ReqSource::Spec => "spec".to_string(),
            ReqSource::Change { change } => format!("change {change}"),
        };
        let _ = writeln!(
            out,
            "- Источник: {}:{} ({origin})",
            r.file.display(),
            r.line
        );
        let _ = writeln!(out, "- SHALL/MUST:");
        for s in &r.statements {
            let _ = writeln!(out, "  - {s}");
        }
    }
    out
}

/// Льготная карточка правила для отчёта покрытия: читает только поля связи
/// (`covers`, `unverifiable`, `owner`) и имя — остальная схема
/// `CONSTRAINTS.yaml` (type/glob/pattern) адаптеру не нужна, а сгенерированный
/// скелет `CONSTRAINTS.from-openspec.yaml` полей `type` вообще не имеет.
#[derive(Debug, Deserialize)]
struct CoverRule {
    /// Имя правила (для ссылок «покрыто правилом …»).
    #[serde(default)]
    name: Option<String>,
    /// Идентификаторы требований, покрываемые правилом.
    #[serde(default)]
    covers: Vec<String>,
    /// Признак заглушки ручного контроля (не детектор).
    #[serde(default)]
    unverifiable: bool,
    /// Владелец ручного контроля.
    #[serde(default)]
    owner: Option<String>,
}

/// Корень файла ограничений (оба допустимых ключа, как в `control.rs`).
#[derive(Debug, Deserialize)]
struct CoversFile {
    /// Канонический корень `rules:`.
    #[serde(default)]
    rules: Vec<CoverRule>,
    /// Альтернативный корень `constraints:`.
    #[serde(default)]
    constraints: Vec<CoverRule>,
}

/// Читает карточки связи из файла ограничений (оба корня — `rules:` и
/// `constraints:`; неизвестные поля правил игнорируются).
///
/// # Errors
/// Файл не читается, YAML невалиден.
fn load_cover_rules(constraints: &Path) -> Result<Vec<CoverRule>> {
    let yaml =
        std::fs::read_to_string(constraints).map_err(|e| HarnessError::io(constraints, e))?;
    let parsed: CoversFile = serde_yaml_ng::from_str(&yaml)?;
    Ok(parsed.rules.into_iter().chain(parsed.constraints).collect())
}

/// Файл ограничений по умолчанию для репозитория: единый резолвер (E2,
/// [`crate::control::resolve_constraints_path`]) — `.arch-handoff/CONSTRAINTS.yaml`,
/// иначе `CONSTRAINTS.yaml` в корне; `None` — ни одного нет.
#[must_use]
pub fn default_constraints(root: &Path) -> Option<PathBuf> {
    crate::control::resolve_constraints_path(root, None)
}

/// Статус покрытия требования решением.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CoverageStatus {
    /// Покрыто детектором: есть правило с `covers:` без признака unverifiable.
    Covered,
    /// Немеханизуемо: заглушка `unverifiable: true` с назначенным owner
    /// (осознанный долг ручного контроля).
    Unverifiable,
    /// Решения нет: ни детектора, ни unverifiable с owner — блокирует
    /// `--strict` и гейт архивации.
    Unresolved,
}

/// Требование с присвоенным статусом покрытия.
#[derive(Debug, Clone, Serialize)]
pub struct CoveredRequirement {
    /// Требование.
    pub requirement: Requirement,
    /// Статус покрытия.
    pub status: CoverageStatus,
    /// Имена правил, перечисляющих требование в `covers:`.
    pub via: Vec<String>,
}

/// Отчёт покрытия требований правилами (JSON-контракт
/// `openspec coverage --json`).
#[derive(Debug, Clone, Serialize)]
pub struct CoverageReport {
    /// Корень репозитория.
    pub root: PathBuf,
    /// Файл ограничений, из которого прочитаны `covers:` (`None` — не найден,
    /// все требования «без решения»).
    pub constraints: Option<PathBuf>,
    /// Вторая копия реестра, отличающаяся от использованной (drift, E2):
    /// существуют обе копии (пакетная и корневая) и они различаются.
    /// Аддитивное поле (не сериализуется, когда `None`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub constraints_drift: Option<PathBuf>,
    /// Уникальных требований SHALL/MUST (дубли по id слиты).
    pub total: usize,
    /// Покрыто детектором.
    pub covered: usize,
    /// Unverifiable с owner.
    pub unverifiable: usize,
    /// Без решения.
    pub unresolved: usize,
    /// Поимённая разбивка.
    pub items: Vec<CoveredRequirement>,
}

impl CoverageReport {
    /// Рендерит отчёт в markdown: сводка + поимённые списки непокрытых
    /// («без решения») и unverifiable.
    #[must_use]
    pub fn to_markdown(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(
            out,
            "# Покрытие требований OpenSpec: {}",
            self.root.display()
        );
        match &self.constraints {
            Some(c) => {
                let _ = writeln!(out, "Файл ограничений: {}", c.display());
                if let Some(d) = &self.constraints_drift {
                    let _ = writeln!(out, "{}", crate::control::constraints_drift_note(c, d));
                }
            }
            None => {
                let _ = writeln!(
                    out,
                    "Файл ограничений: не найден — все требования «без решения»"
                );
            }
        }
        let _ = writeln!(out, "\nSHALL всего: {}", self.total);
        let _ = writeln!(out, "- покрыто детектором: {}", self.covered);
        let _ = writeln!(out, "- unverifiable с owner: {}", self.unverifiable);
        let _ = writeln!(out, "- без решения: {}", self.unresolved);

        let _ = writeln!(out, "\n## Без решения");
        let mut any = false;
        for item in &self.items {
            if item.status == CoverageStatus::Unresolved {
                let r = &item.requirement;
                let _ = writeln!(
                    out,
                    "- {} — {} ({}:{})",
                    r.id,
                    r.title,
                    r.file.display(),
                    r.line
                );
                any = true;
            }
        }
        if !any {
            let _ = writeln!(out, "- нет");
        }

        let _ = writeln!(out, "\n## Unverifiable (ручной контроль, owner назначен)");
        let mut any = false;
        for item in &self.items {
            if item.status == CoverageStatus::Unverifiable {
                let r = &item.requirement;
                let _ = writeln!(
                    out,
                    "- {} — {} ({}:{}, правила: {})",
                    r.id,
                    r.title,
                    r.file.display(),
                    r.line,
                    item.via.join(", ")
                );
                any = true;
            }
        }
        if !any {
            let _ = writeln!(out, "- нет");
        }
        out
    }
}

/// Классифицирует требование по карточкам правил: детектор (хотя бы одно
/// покрывающее правило без `unverifiable`) → unverifiable с owner →
/// без решения. Заглушка `unverifiable` с пустым owner — это ещё НЕ решение
/// (свежий скелет `openspec init`), требование остаётся «без решения».
fn classify(requirement: &Requirement, rules: &[CoverRule]) -> CoveredRequirement {
    let covering: Vec<&CoverRule> = rules
        .iter()
        .filter(|r| r.covers.iter().any(|c| c == &requirement.id))
        .collect();
    let via: Vec<String> = covering
        .iter()
        .map(|r| r.name.clone().unwrap_or_else(|| "<без имени>".to_string()))
        .collect();
    let status = if covering.iter().any(|r| !r.unverifiable) {
        CoverageStatus::Covered
    } else if covering
        .iter()
        .any(|r| r.unverifiable && r.owner.as_deref().is_some_and(|o| !o.trim().is_empty()))
    {
        CoverageStatus::Unverifiable
    } else {
        CoverageStatus::Unresolved
    };
    CoveredRequirement {
        requirement: requirement.clone(),
        status,
        via,
    }
}

/// Дедупликация требований по стабильному id (первое вхождение побеждает —
/// живые спеки сканируются раньше дельт, поэтому источник дубля — spec).
fn unique_by_id(requirements: Vec<Requirement>) -> Vec<Requirement> {
    let mut seen = BTreeSet::new();
    requirements
        .into_iter()
        .filter(|r| seen.insert(r.id.clone()))
        .collect()
}

/// Отчёт покрытия требований `OpenSpec` правилами `CONSTRAINTS.yaml`.
///
/// `constraints`: явный путь к файлу ограничений; `None` — авто-детект
/// ([`default_constraints`]). Если файла нет, все требования классифицируются
/// как «без решения» (это честное состояние, а не ошибка).
///
/// # Errors
/// `openspec/specs` отсутствует, явно заданный файл ограничений не
/// читается/невалиден, файл спеки не читается.
pub fn coverage(root: &Path, constraints: Option<&Path>) -> Result<CoverageReport> {
    let requirements = unique_by_id(scan_requirements(root)?);
    // Единый резолвер (E2): явный путь → пакетная копия → корневая; дрейф
    // двух копий (корневая новее пакетной — частый случай) — пометкой,
    // а не молчаливым покрытием по устаревшей копии.
    let (path, drift) = match constraints {
        Some(p) => (Some(p.to_path_buf()), None),
        None => match crate::control::resolve_constraints_path_detailed(root, None) {
            Some(resolution) => (Some(resolution.path), resolution.drift),
            None => (None, None),
        },
    };
    let rules = match &path {
        Some(p) => load_cover_rules(p)?,
        None => Vec::new(),
    };
    let items: Vec<CoveredRequirement> = requirements.iter().map(|r| classify(r, &rules)).collect();
    let count = |status: CoverageStatus| items.iter().filter(|i| i.status == status).count();
    Ok(CoverageReport {
        root: root.to_path_buf(),
        constraints: path,
        constraints_drift: drift,
        total: items.len(),
        covered: count(CoverageStatus::Covered),
        unverifiable: count(CoverageStatus::Unverifiable),
        unresolved: count(CoverageStatus::Unresolved),
        items,
    })
}

/// Кандидат в спайн из design.md: заголовок секции решений/ограничений.
#[derive(Debug, Clone)]
struct DesignCandidate {
    /// Идентификатор change (имя каталога).
    change: String,
    /// Заголовок секции (текст после `## `).
    heading: String,
}

/// Извлекает из design.md заголовки секций решений и ограничений
/// (`## Decisions`, `## Constraints`, `## Решение N: …` и т.п.).
fn parse_design_candidates(content: &str, change: &str) -> Result<Vec<DesignCandidate>> {
    let re_heading = os_regex(r"^##\s+(.+?)\s*$")?;
    let re_decision = os_regex(r"(?i)(decision|constraint|решени|ограничен)")?;
    let mut out = Vec::new();
    for line in content.lines() {
        if let Some(caps) = re_heading.captures(line) {
            let heading = caps[1].to_string();
            if re_decision.is_match(&heading) {
                out.push(DesignCandidate {
                    change: change.to_string(),
                    heading,
                });
            }
        }
    }
    Ok(out)
}

/// Собирает кандидатов из design.md changes: `archived = false` — активные
/// (`openspec/changes/<id>/`, без `archive`), `true` — архивные
/// (`openspec/changes/archive/<id>/`).
fn collect_design_candidates(root: &Path, archived: bool) -> Result<Vec<DesignCandidate>> {
    let base = if archived {
        root.join("openspec/changes/archive")
    } else {
        root.join("openspec/changes")
    };
    if !base.is_dir() {
        return Ok(Vec::new());
    }
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(&base)
        .map_err(|e| HarnessError::io(&base, e))?
        .filter_map(std::result::Result::ok)
        .map(|e| e.path())
        .filter(|p| p.is_dir() && p.file_name().is_some_and(|n| n != "archive"))
        .collect();
    dirs.sort();
    let mut out = Vec::new();
    for dir in dirs {
        let design = dir.join("design.md");
        if !design.is_file() {
            continue;
        }
        let change = dir
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let content = std::fs::read_to_string(&design).map_err(|e| HarnessError::io(&design, e))?;
        out.extend(parse_design_candidates(&content, &change)?);
    }
    Ok(out)
}

/// Экранирует строку для YAML-скаляра в двойных кавычках (ручная генерация
/// скелета; полноценный сериализатор YAML здесь избыточен).
fn yaml_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "'")
}

/// Результат генерации `openspec init`.
#[derive(Debug)]
pub struct InitOutcome {
    /// Путь к сгенерированному скелету ограничений.
    pub constraints_path: PathBuf,
    /// Путь к черновику спайна.
    pub spine_path: PathBuf,
    /// Число правил-заглушек в скелете.
    pub rules: usize,
    /// Число кандидатов в спайн (активные changes).
    pub candidates: usize,
    /// Число записей истории (archive).
    pub history: usize,
    /// Отчёт покрытия ПО сгенерированному скелету (все заглушки без owner —
    /// честный стартовый долг «без решения»).
    pub coverage: CoverageReport,
}

/// Генерирует по репозиторию `OpenSpec`:
/// - `CONSTRAINTS.from-openspec.yaml` — все найденные SHALL как
///   правила-заглушки `unverifiable: true` с пустым owner и проставленным
///   `covers:` (скелет — черновик для вливания в основной `CONSTRAINTS.yaml`;
///   `control check` заглушки пропускает как записи ручного контроля —
///   заполнить детектор (type/glob/pattern) или owner);
/// - `SPINE.draft.md` — кандидаты из секций Decisions/Constraints design.md
///   активных changes (очередь на подтверждение, НЕ автоматом) и история
///   решений из archive (источник для expiry).
///
/// Существующие файлы не затираются без `force = true` (проверяются оба
/// артефакта до записи любого из них). Содержимое детерминировано:
/// повторная генерация с `--force` даёт байт-в-байт тот же файл.
///
/// # Errors
/// `openspec/specs` отсутствует, целевой файл существует без `force`,
/// ошибки чтения/записи.
pub fn init(root: &Path, out_dir: &Path, force: bool) -> Result<InitOutcome> {
    let requirements = unique_by_id(scan_requirements(root)?);
    let constraints_path = out_dir.join("CONSTRAINTS.from-openspec.yaml");
    let spine_path = out_dir.join("SPINE.draft.md");
    for path in [&constraints_path, &spine_path] {
        if path.exists() && !force {
            return Err(HarnessError::Control(format!(
                "файл уже существует: {} — перезапись только с --force",
                path.display()
            )));
        }
    }
    std::fs::create_dir_all(out_dir).map_err(|e| HarnessError::io(out_dir, e))?;

    // --- Скелет ограничений ---
    let mut yaml = String::new();
    let _ = writeln!(
        yaml,
        "# CONSTRAINTS.from-openspec.yaml — СКЕЛЕТ, сгенерирован `arch-be openspec init`."
    );
    let _ = writeln!(
        yaml,
        "# Источник: openspec/ репозитория {}. Все правила — заглушки",
        root.display()
    );
    let _ = writeln!(
        yaml,
        "# `unverifiable: true` с пустым owner: заполнить детектор (type/glob/pattern)"
    );
    let _ = writeln!(
        yaml,
        "# или owner (ручной контроль) и влить в основной CONSTRAINTS.yaml."
    );
    let _ = writeln!(yaml, "constraints:");
    let mut sorted = requirements.clone();
    sorted.sort_by(|a, b| {
        a.capability
            .cmp(&b.capability)
            .then(a.file.cmp(&b.file))
            .then(a.line.cmp(&b.line))
    });
    for (idx, r) in sorted.iter().enumerate() {
        let name = crate::control::kebab_slug(&r.title)
            .chars()
            .take(64)
            .collect::<String>();
        let _ = writeln!(yaml, "  - id: OS-{:03}", idx + 1);
        let _ = writeln!(yaml, "    name: {name}");
        let _ = writeln!(yaml, "    unverifiable: true");
        let _ = writeln!(yaml, "    covers: [\"{}\"]", r.id);
        let _ = writeln!(yaml, "    owner: \"\"");
        let _ = writeln!(
            yaml,
            "    rationale: \"OpenSpec: {} ({}:{})\"",
            yaml_escape(&r.title),
            r.file.display(),
            r.line
        );
        let _ = writeln!(yaml);
    }

    // --- Черновик спайна ---
    let candidates = collect_design_candidates(root, false)?;
    let history = collect_design_candidates(root, true)?;
    let mut spine = String::new();
    let _ = writeln!(
        spine,
        "# SPINE.draft.md — ЧЕРНОВИК, сгенерирован `arch-be openspec init`."
    );
    let _ = writeln!(
        spine,
        "# Кандидаты в спайн из design.md changes OpenSpec (секции Decisions/Constraints)."
    );
    let _ = writeln!(
        spine,
        "# Кандидаты — очередь на подтверждение архитектором, в спайн НЕ автоматом."
    );
    let _ = writeln!(spine, "\n## Кандидаты (активные changes)\n");
    if candidates.is_empty() {
        let _ = writeln!(
            spine,
            "- нет (у активных changes нет design.md с секциями решений)"
        );
    }
    for c in &candidates {
        let _ = writeln!(
            spine,
            "- **{}** (источник: changes/{}, design.md)",
            c.heading, c.change
        );
    }
    let _ = writeln!(
        spine,
        "\n## История (changes/archive — уже принято, источник для expiry)\n"
    );
    if history.is_empty() {
        let _ = writeln!(spine, "- нет");
    }
    for c in &history {
        let _ = writeln!(
            spine,
            "- **{}** (источник: changes/archive/{}, design.md)",
            c.heading, c.change
        );
    }

    std::fs::write(&constraints_path, yaml).map_err(|e| HarnessError::io(&constraints_path, e))?;
    std::fs::write(&spine_path, spine).map_err(|e| HarnessError::io(&spine_path, e))?;

    let coverage = coverage(root, Some(&constraints_path))?;
    Ok(InitOutcome {
        constraints_path,
        spine_path,
        rules: sorted.len(),
        candidates: candidates.len(),
        history: history.len(),
        coverage,
    })
}

/// Отчёт гейта архивации change.
#[derive(Debug)]
pub struct GateReport {
    /// Идентификатор change.
    pub change: String,
    /// Требований в дельте change.
    pub total: usize,
    /// Требования без решения (блокируют архивацию).
    pub uncovered: Vec<CoveredRequirement>,
    /// Итог `control check` (`None` — файл ограничений не найден, проверка
    /// не запускалась).
    pub control: Option<crate::control::FitnessReport>,
    /// Гейт пройден.
    pub passed: bool,
}

impl GateReport {
    /// Рендерит отчёт гейта в markdown.
    #[must_use]
    pub fn to_markdown(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(out, "# Гейт архивации OpenSpec: change '{}'", self.change);
        let _ = writeln!(
            out,
            "\nТребований change: {}, без решения: {}",
            self.total,
            self.uncovered.len()
        );
        for item in &self.uncovered {
            let r = &item.requirement;
            let _ = writeln!(
                out,
                "- {} — {} ({}:{})",
                r.id,
                r.title,
                r.file.display(),
                r.line
            );
        }
        match &self.control {
            Some(report) => {
                let _ = writeln!(
                    out,
                    "\ncontrol check: {} ({})",
                    if report.passed { "PASS" } else { "FAIL" },
                    report.summary
                );
            }
            None => {
                let _ = writeln!(
                    out,
                    "\ncontrol check: не запускался (файл ограничений не найден)"
                );
            }
        }
        let _ = writeln!(out, "\nИтог: {}", if self.passed { "PASS" } else { "FAIL" });
        out
    }
}

/// Гейт архивации change (точка CI перед `openspec archive`): FAIL, если
/// хотя бы одно требование дельты `changes/<change_id>/specs/` — «без
/// решения» (нет ни детектора, ни unverifiable с owner), либо падает
/// `control check` по файлу ограничений.
///
/// # Errors
/// Change не найден (или уже в архиве), `openspec/specs` отсутствует,
/// файл ограничений не читается/невалиден, правила `control check`
/// некорректны.
pub fn gate_archive(
    root: &Path,
    change_id: &str,
    constraints: Option<&Path>,
) -> Result<GateReport> {
    let change_dir = root.join("openspec/changes").join(change_id);
    if !change_dir.is_dir() {
        let archived = root.join("openspec/changes/archive").join(change_id);
        let hint = if archived.is_dir() {
            " — change уже в архиве (openspec/changes/archive/)"
        } else {
            ""
        };
        return Err(HarnessError::Control(format!(
            "change '{change_id}' не найден: {}{hint}",
            change_dir.display()
        )));
    }
    // Дельта change парсится напрямую: scan_requirements требует
    // openspec/specs, а у репозитория могут быть только changes.
    let mut delta_reqs = Vec::new();
    let source = ReqSource::Change {
        change: change_id.to_string(),
    };
    for file in collect_md(&change_dir.join("specs")) {
        parse_spec_file(&file, root, &source, &mut delta_reqs)?;
    }

    let path = match constraints {
        Some(p) => Some(p.to_path_buf()),
        None => default_constraints(root),
    };
    let rules = match &path {
        Some(p) => load_cover_rules(p)?,
        None => Vec::new(),
    };
    let items: Vec<CoveredRequirement> = unique_by_id(delta_reqs)
        .iter()
        .map(|r| classify(r, &rules))
        .collect();
    let total = items.len();
    let uncovered: Vec<CoveredRequirement> = items
        .into_iter()
        .filter(|i| i.status == CoverageStatus::Unresolved)
        .collect();

    let control = match &path {
        Some(p) => Some(crate::control::check(root, p)?),
        None => None,
    };
    let control_ok = control.as_ref().is_none_or(|r| r.passed);
    let passed = uncovered.is_empty() && control_ok;
    Ok(GateReport {
        change: change_id.to_string(),
        total,
        uncovered,
        control,
        passed,
    })
}

/// Инструменты домена: `openspec_coverage`.
#[must_use]
pub fn tools() -> Vec<Arc<dyn Tool>> {
    vec![Arc::new(OpenspecCoverageTool)]
}

/// Инструмент `openspec_coverage`: покрытие требований `OpenSpec` правилами
/// `CONSTRAINTS.yaml` — JSON-вердикт со счётчиками (мост в MCP, транш 2
/// инверсии; read-only).
pub struct OpenspecCoverageTool;

#[derive(Debug, Deserialize)]
struct OpenspecCoverageArgs {
    /// Корень репозитория с разметкой `OpenSpec`.
    path: String,
    /// Файл ограничений (дефолт — авто-детект [`default_constraints`]:
    /// `<root>/.arch-handoff/CONSTRAINTS.yaml`, иначе `<root>/CONSTRAINTS.yaml`;
    /// нет файла — все требования «без решения»).
    constraints: Option<String>,
    /// Строгий режим: требования «без решения» делают `passed: false`
    /// (как `--strict` на CLI: exit 1). По умолчанию отчёт — `passed` true.
    strict: Option<bool>,
}

#[async_trait]
impl Tool for OpenspecCoverageTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "openspec_coverage".into(),
            description: "Покрытие требований OpenSpec (openspec/specs/ + активные changes) \
                          правилами CONSTRAINTS.yaml (связь — поле covers: правила): SHALL \
                          всего / покрыто детектором / unverifiable с owner / без решения, \
                          непокрытые поимённо. Ответ — JSON: passed + счётчики \
                          total/covered/unverifiable/unresolved + unresolved_items + \
                          report_markdown. passed=false только в strict-режиме при \
                          требованиях «без решения»"
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Корень репозитория с разметкой OpenSpec"},
                    "constraints": {
                        "type": "string",
                        "description": "Файл ограничений (по умолчанию авто-детект: <root>/.arch-handoff/CONSTRAINTS.yaml, иначе <root>/CONSTRAINTS.yaml)"
                    },
                    "strict": {
                        "type": "boolean",
                        "description": "Гейт: passed=false при требованиях «без решения» (по умолчанию false — отчёт)"
                    }
                },
                "required": ["path"]
            }),
        }
    }

    async fn call(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let args: OpenspecCoverageArgs = match serde_json::from_value(args) {
            Ok(a) => a,
            Err(e) => {
                return Ok(ToolOutput::err(format!(
                    "openspec_coverage: невалидные аргументы: {e}"
                )));
            }
        };
        let root = ctx.resolve(&args.path);
        let constraints = args.constraints.map(|c| ctx.resolve(c));
        let strict = args.strict.unwrap_or(false);
        let report = match coverage(&root, constraints.as_deref()) {
            Ok(r) => r,
            Err(e) => return Ok(ToolOutput::err(format!("openspec_coverage: {e}"))),
        };
        let unresolved_items: Vec<Value> = report
            .items
            .iter()
            .filter(|i| i.status == CoverageStatus::Unresolved)
            .map(|i| {
                json!({
                    "id": i.requirement.id,
                    "title": i.requirement.title,
                    "file": i.requirement.file.display().to_string(),
                    "line": i.requirement.line,
                })
            })
            .collect();
        let summary = format!(
            "OpenSpec-покрытие {}: SHALL {}, покрыто {}, unverifiable {}, без решения {}{}",
            report.root.display(),
            report.total,
            report.covered,
            report.unverifiable,
            report.unresolved,
            if strict { " (strict)" } else { "" }
        );
        let drift_note = match (&report.constraints, &report.constraints_drift) {
            (Some(c), Some(d)) => Some(crate::control::constraints_drift_note(c, d)),
            _ => None,
        };
        let verdict = json!({
            "tool": "openspec_coverage",
            "passed": !(strict && report.unresolved > 0),
            "strict": strict,
            "constraints": report.constraints.as_ref().map(|p| p.display().to_string()),
            "drift_note": drift_note,
            "total": report.total,
            "covered": report.covered,
            "unverifiable": report.unverifiable,
            "unresolved": report.unresolved,
            "unresolved_items": unresolved_items,
            "summary": summary,
            "report_markdown": report.to_markdown(),
        });
        // Сериализация собранного объекта не падает; запасной вариант — компактная форма.
        let text = serde_json::to_string_pretty(&verdict).unwrap_or_else(|_| verdict.to_string());
        Ok(ToolOutput::ok(text))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Пишет файл фикстуры (родительские каталоги создаются).
    fn write(path: &Path, content: &str) {
        std::fs::create_dir_all(path.parent().expect("родитель фикстуры")).expect("mkdir фикстуры");
        std::fs::write(path, content).expect("запись фикстуры");
    }

    /// Спека с двумя требованиями: одно с SHALL/MUST, одно без ключевых слов.
    const SPEC_MD: &str = "# payments Specification\n\
        \n\
        ## Purpose\n\
        Спека платежей.\n\
        \n\
        ## Requirements\n\
        \n\
        ### Requirement: Точные деньги\n\
        Система SHALL хранить суммы в minor units.\n\
        Продолжение без ключевых слов.\n\
        И MUST NOT использовать f64.\n\
        \n\
        #### Scenario: Округление\n\
        - **WHEN** сумма дробная\n\
        - **THEN** система SHALL округлять по правилу\n\
        \n\
        ### Requirement: Описание без модальных глаголов\n\
        Просто проза без требований.\n\
        \n\
        ### Requirement: Идемпотентность\n\
        Система SHALL требовать ключ идемпотентности.\n";

    // --- E11.1/E11.2: сценарии проверки --------------------------------------

    /// Сценарии привязываются к требованию, получают стабильный идентификатор
    /// `…/S<n>` и тело WHEN/THEN; требование без SHALL/MUST сценариев не даёт.
    #[test]
    fn scenarios_are_bound_to_requirements_with_stable_ids() {
        let scenarios = parse_scenarios(
            SPEC_MD,
            "payments",
            Path::new("openspec/specs/payments/spec.md"),
        )
        .expect("сценарии разбираются");
        assert_eq!(scenarios.len(), 1, "{scenarios:?}");
        let scenario = &scenarios[0];
        assert_eq!(scenario.title, "Округление");
        assert!(
            scenario.id.ends_with("/S1"),
            "номер сценария в идентификаторе: {}",
            scenario.id
        );
        assert!(
            scenario.id.starts_with(&scenario.requirement_id),
            "идентификатор сценария строится от требования"
        );
        assert_eq!(scenario.steps.len(), 2, "{:?}", scenario.steps);
        assert!(scenario.steps[0].contains("WHEN"), "{:?}", scenario.steps);
        assert!(scenario.steps[1].contains("THEN"), "{:?}", scenario.steps);
        assert_eq!(scenario.line, 13, "строка заголовка сценария");
        assert_eq!(
            scenario.file,
            PathBuf::from("openspec/specs/payments/spec.md")
        );
    }

    /// Требование без сценариев сценариев не даёт; несколько сценариев под
    /// одним требованием нумеруются по порядку.
    #[test]
    fn scenarios_are_numbered_per_requirement() {
        let content = "# Spec\n\n### Requirement: Одно\nСистема SHALL делать.\n\n\
                       #### Scenario: Первый\n- **WHEN** раз\n- **THEN** два\n\n\
                       #### Scenario: Второй\n- **WHEN** три\n- **THEN** четыре\n\n\
                       ### Requirement: Без сценариев\nСистема SHALL молчать.\n";
        let scenarios = parse_scenarios(content, "cap", Path::new("spec.md")).expect("разбор");
        assert_eq!(scenarios.len(), 2);
        assert!(scenarios[0].id.ends_with("/S1"), "{}", scenarios[0].id);
        assert_eq!(scenarios[1].title, "Второй");
        assert!(scenarios[1].id.ends_with("/S2"), "{}", scenarios[1].id);
        assert_eq!(
            scenarios[0].requirement_id, scenarios[1].requirement_id,
            "оба сценария — одного требования"
        );
    }

    /// Изменение текста сценария в `OpenSpec` меняет досье Spine без ручной
    /// работы: сценарии читаются из спеки при каждой сборке (E11.2).
    #[test]
    fn scan_scenarios_reads_specs_and_tracks_changes() {
        let dir = tempfile::tempdir().expect("tmp");
        let root = dir.path();
        let spec = root.join("openspec/specs/payments/spec.md");
        write(&spec, SPEC_MD);
        let first = scan_scenarios(root).expect("сканирование");
        assert_eq!(first.len(), 1);
        // Правка сценария в спеке видна в следующем сканировании.
        write(
            &spec,
            &SPEC_MD.replace("по правилу", "по правилу банка, с округлением вниз"),
        );
        let second = scan_scenarios(root).expect("сканирование");
        assert_eq!(second.len(), 1);
        assert_ne!(
            first[0].steps, second[0].steps,
            "правка OpenSpec видна без ручной работы"
        );
    }

    /// Нет разметки `OpenSpec` — понятная ошибка, а не пустой список.
    #[test]
    fn scan_scenarios_without_openspec_is_an_explicit_error() {
        let dir = tempfile::tempdir().expect("tmp");
        let err = scan_scenarios(dir.path()).expect_err("нет openspec/specs");
        assert!(err.to_string().contains("openspec/specs"), "{err}");
    }

    /// Дельта активного change.
    const DELTA_SPEC_MD: &str = "# Delta: payments\n\
        \n\
        ## ADDED Requirements\n\
        \n\
        ### Requirement: Колбэки идемпотентны\n\
        Повторный колбэк MUST NOT менять состояние платежа.\n";

    /// design.md активного change.
    const DESIGN_MD: &str = "# Design: add-callback\n\
        \n\
        ## Контекст\n\
        Контекст change.\n\
        \n\
        ## Решение 1: Деньги — minor units\n\
        Обоснование решения.\n\
        \n\
        ## Constraints\n\
        - без новых зависимостей\n";

    /// Репозиторий-фикстура с разметкой `OpenSpec`.
    fn fixture_repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        write(&root.join("openspec/specs/payments/spec.md"), SPEC_MD);
        write(
            &root.join("openspec/changes/add-callback/specs/payments/spec.md"),
            DELTA_SPEC_MD,
        );
        write(
            &root.join("openspec/changes/add-callback/design.md"),
            DESIGN_MD,
        );
        write(
            &root.join("openspec/changes/archive/2026-01-01-add-auth/design.md"),
            "# Design: add-auth\n\n## Решение 1: Токены короткоживущие\nТекст.\n",
        );
        dir
    }

    /// Идентификатор первого требования живой спеки фикстуры.
    fn fixture_money_id() -> String {
        requirement_id(
            "payments",
            &[
                "Система SHALL хранить суммы в minor units.".to_string(),
                "И MUST NOT использовать f64.".to_string(),
            ],
        )
    }

    #[test]
    fn parser_extracts_shall_and_skips_scenarios() {
        let reqs = parse_requirements(SPEC_MD).expect("парсинг спеки");
        assert_eq!(reqs.len(), 2, "требование без SHALL отбрасывается");
        let money = &reqs[0];
        assert_eq!(money.title, "Точные деньги");
        assert_eq!(money.line, 8);
        assert_eq!(
            money.statements,
            vec![
                "Система SHALL хранить суммы в minor units.".to_string(),
                "И MUST NOT использовать f64.".to_string(),
            ],
            "SHALL внутри #### Scenario не входит в тело требования"
        );
    }

    #[test]
    fn requirement_id_stable_and_title_independent() {
        let statements = vec!["Система   SHALL хранить\nсуммы.".to_string()];
        let a = requirement_id("payments", &statements);
        let b = requirement_id("payments", &["Система SHALL хранить суммы.".to_string()]);
        assert_eq!(a, b, "пробелы/переносы нормализуются");
        assert!(a.starts_with("openspec:payments#"));
        assert_eq!(a.len(), "openspec:payments#".len() + 8);
        let other_cap = requirement_id("billing", &statements);
        assert_ne!(a, other_cap, "capability входит в хэш");
        let other_text = requirement_id("payments", &["Система SHALL иное.".to_string()]);
        assert_ne!(a, other_text, "текст требования входит в хэш");
    }

    #[test]
    fn scan_finds_specs_and_change_deltas() {
        let dir = fixture_repo();
        let reqs = scan_requirements(dir.path()).expect("scan");
        assert_eq!(reqs.len(), 3);
        let specs = reqs.iter().filter(|r| r.source == ReqSource::Spec).count();
        assert_eq!(specs, 2);
        let delta = reqs
            .iter()
            .find(|r| matches!(&r.source, ReqSource::Change { change } if change == "add-callback"))
            .expect("требование дельты");
        assert_eq!(delta.capability, "payments");
        assert_eq!(
            delta.file,
            PathBuf::from("openspec/changes/add-callback/specs/payments/spec.md")
        );
    }

    #[test]
    fn scan_requires_openspec_specs() {
        let dir = tempfile::tempdir().expect("tempdir");
        let err = scan_requirements(dir.path()).expect_err("без openspec/specs — ошибка");
        assert!(err.to_string().contains("openspec/specs не найден"));
    }

    #[test]
    fn coverage_classifies_covered_unverifiable_unresolved() {
        let dir = fixture_repo();
        let root = dir.path();
        let covered_id = fixture_money_id();
        let unverifiable_id = requirement_id(
            "payments",
            &["Система SHALL требовать ключ идемпотентности.".to_string()],
        );
        let constraints = format!(
            "rules:\n\
             \x20 - name: detector_rule\n\
             \x20   type: must_contain\n\
             \x20   glob: \"src/**/*.rs\"\n\
             \x20   pattern: \"x\"\n\
             \x20   covers: [\"{covered_id}\"]\n\
             \x20 - name: manual_rule\n\
             \x20   unverifiable: true\n\
             \x20   owner: \"@arch\"\n\
             \x20   covers: [\"{unverifiable_id}\"]\n\
             \x20 - name: ownerless_stub\n\
             \x20   unverifiable: true\n\
             \x20   owner: \"\"\n\
             \x20   covers: [\"openspec:payments#deadbeef\"]\n"
        );
        write(&root.join("CONSTRAINTS.yaml"), &constraints);
        let report = coverage(root, None).expect("coverage");
        assert_eq!(report.total, 3);
        assert_eq!(report.covered, 1);
        assert_eq!(report.unverifiable, 1);
        assert_eq!(report.unresolved, 1, "третье требование без covers");
        let unresolved = report
            .items
            .iter()
            .find(|i| i.status == CoverageStatus::Unresolved)
            .expect("есть непокрытое");
        assert_eq!(unresolved.requirement.title, "Колбэки идемпотентны");
        let md = report.to_markdown();
        assert!(md.contains("без решения: 1"));
        assert!(md.contains("Колбэки идемпотентны"));
    }

    #[test]
    fn coverage_missing_constraints_marks_all_unresolved() {
        let dir = fixture_repo();
        let report = coverage(dir.path(), None).expect("coverage без constraints");
        assert_eq!(report.constraints, None);
        assert_eq!(report.unresolved, report.total);
        assert_eq!(report.covered, 0);
    }

    /// E2: обе копии реестра различаются — покрытие по пакетной + пометка
    /// дрейфа (а не молчаливый расчёт по устаревшей копии).
    #[test]
    fn coverage_notes_drift_of_two_copies() {
        let dir = fixture_repo();
        let root = dir.path();
        let covered_id = fixture_money_id();
        // Корневая копия «новее»: в ней есть covers, в пакетной — нет.
        write(
            &root.join("CONSTRAINTS.yaml"),
            &format!(
                "rules:\n  - name: detector_rule\n    type: must_contain\n    covers: [\"{covered_id}\"]\n"
            ),
        );
        write(
            &root.join(".arch-handoff/CONSTRAINTS.yaml"),
            "rules:\n  - name: detector_rule\n    type: must_contain\n",
        );
        let report = coverage(root, None).expect("coverage");
        assert_eq!(
            report.constraints,
            Some(root.join(".arch-handoff/CONSTRAINTS.yaml")),
            "используется пакетная копия"
        );
        assert_eq!(
            report.constraints_drift,
            Some(root.join("CONSTRAINTS.yaml"))
        );
        let md = report.to_markdown();
        assert!(md.contains("Файл ограничений: "), "{md}");
        assert!(md.contains("копии реестра различаются"), "{md}");
        assert!(md.contains("отличается (drift)"), "{md}");
        // Копии одинаковы — пометки нет.
        write(
            &root.join("CONSTRAINTS.yaml"),
            "rules:\n  - name: detector_rule\n    type: must_contain\n",
        );
        let report = coverage(root, None).expect("coverage");
        assert_eq!(report.constraints_drift, None);
        assert!(!report.to_markdown().contains("drift"));
    }

    #[test]
    fn init_generates_skeleton_and_is_force_idempotent() {
        let dir = fixture_repo();
        let root = dir.path();
        let outcome = init(root, root, false).expect("init");
        assert_eq!(outcome.rules, 3);
        assert_eq!(
            outcome.candidates, 2,
            "Решение 1 + Constraints активного change"
        );
        assert_eq!(outcome.history, 1, "архивный design.md — история");
        // Свежий скелет: все заглушки без owner → «без решения» (честный долг).
        assert_eq!(outcome.coverage.unresolved, 3);

        let yaml = std::fs::read_to_string(&outcome.constraints_path).expect("скелет");
        assert!(yaml.contains("unverifiable: true"));
        assert!(yaml.contains(&format!("covers: [\"{}\"]", fixture_money_id())));
        assert!(yaml.contains("owner: \"\""));
        let spine = std::fs::read_to_string(&outcome.spine_path).expect("спайн-черновик");
        assert!(spine.contains("Решение 1: Деньги — minor units"));
        assert!(spine.contains("changes/archive/2026-01-01-add-auth"));
        assert!(
            !spine.contains("Контекст change"),
            "секция контекста не кандидат"
        );

        // Без --force существующие файлы не затираются.
        let err = init(root, root, false).expect_err("без --force — ошибка");
        assert!(err.to_string().contains("--force"));
        // С --force — регенерация байт-в-байт идентична (детерминизм).
        let again = init(root, root, true).expect("init --force");
        let yaml2 = std::fs::read_to_string(&again.constraints_path).expect("скелет заново");
        assert_eq!(yaml, yaml2);
    }

    #[test]
    fn gate_archive_fails_on_uncovered_and_passes_with_covers() {
        let dir = fixture_repo();
        let root = dir.path();
        // Без covers у требований change гейт падает.
        let report = gate_archive(root, "add-callback", None).expect("гейт");
        assert!(!report.passed);
        assert_eq!(report.total, 1);
        assert_eq!(report.uncovered.len(), 1);
        let md = report.to_markdown();
        assert!(md.contains("Итог: FAIL"));

        // Покрываем требование дельты детектором — гейт проходит
        // (control check по файлу с одним валидным правилом PASS).
        let delta_id = requirement_id(
            "payments",
            &["Повторный колбэк MUST NOT менять состояние платежа.".to_string()],
        );
        write(&root.join("marker.txt"), "idempotency_key\n");
        let constraints = format!(
            "rules:\n\
             \x20 - name: callback_detector\n\
             \x20   type: must_contain\n\
             \x20   glob: \"marker.txt\"\n\
             \x20   pattern: \"idempotency_key\"\n\
             \x20   covers: [\"{delta_id}\"]\n"
        );
        write(&root.join("CONSTRAINTS.yaml"), &constraints);
        let report = gate_archive(root, "add-callback", None).expect("гейт с covers");
        assert!(report.passed, "отчёт: {}", report.to_markdown());
        assert_eq!(report.uncovered.len(), 0);
        assert!(report.control.as_ref().is_some_and(|c| c.passed));
    }

    #[test]
    fn gate_archive_unknown_change_errors() {
        let dir = fixture_repo();
        let err = gate_archive(dir.path(), "no-such-change", None).expect_err("нет change");
        assert!(err.to_string().contains("не найден"));
        let err = gate_archive(dir.path(), "2026-01-01-add-auth", None)
            .expect_err("архивный change — не активен");
        assert!(err.to_string().contains("уже в архиве"));
    }

    /// Инструмент `openspec_coverage`: счётчики и поимённые непокрытые на
    /// фикстуре; по умолчанию — отчёт, strict — гейт по «без решения».
    #[tokio::test]
    async fn openspec_coverage_tool_verdict_and_strict() {
        let dir = fixture_repo();
        let root = dir.path();
        // Правило покрывает одно требование живой спеки; остальные — «без решения».
        write(
            &root.join("CONSTRAINTS.yaml"),
            &format!(
                "rules:\n  - name: money-detector\n    type: must_contain\n    glob: 'src/**'\n    pattern: 'minor_units'\n    covers: [\"{}\"]\n",
                fixture_money_id()
            ),
        );
        let ctx = ToolContext::new(
            root.to_path_buf(),
            Arc::new(crate::config::Config::default()),
        );
        let out = OpenspecCoverageTool
            .call(json!({"path": "."}), &ctx)
            .await
            .expect("вызов");
        assert!(!out.is_error, "{}", out.content);
        let v: Value = serde_json::from_str(&out.content).expect("JSON-вердикт");
        assert_eq!(v["tool"], "openspec_coverage");
        assert_eq!(v["total"], 3, "{v}");
        assert_eq!(v["covered"], 1, "{v}");
        assert_eq!(v["unresolved"], 2, "{v}");
        assert_eq!(v["passed"], true, "отчёт, не гейт: {v}");
        let unresolved = v["unresolved_items"].as_array().expect("items");
        assert_eq!(unresolved.len(), 2, "{v}");
        assert!(
            unresolved[0]["id"]
                .as_str()
                .expect("id")
                .starts_with(ID_PREFIX)
        );
        assert!(
            v["report_markdown"]
                .as_str()
                .expect("md")
                .contains("# Покрытие требований OpenSpec"),
            "{v}"
        );
        // strict: требования «без решения» → passed=false (семантика --strict CLI).
        let out = OpenspecCoverageTool
            .call(json!({"path": ".", "strict": true}), &ctx)
            .await
            .expect("вызов");
        let v: Value = serde_json::from_str(&out.content).expect("JSON");
        assert_eq!(v["passed"], false, "{v}");
        // Битый файл ограничений (явный) — мягкая ошибка инструмента.
        let out = OpenspecCoverageTool
            .call(json!({"path": ".", "constraints": "нет-такого.yaml"}), &ctx)
            .await
            .expect("вызов");
        assert!(out.is_error, "{}", out.content);
    }
}
