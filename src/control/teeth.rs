//! Измерение зубьев правил реестра (`arch-be rules teeth`, волна B 0.3.14,
//! ADR-065): «поведенческое» означает «с зубьями», а не «нужного типа».
//!
//! `BEHAVIOUR_RULE_KINDS` классифицирует правило по типу, поэтому
//! `command_succeeds` с `command: 'true'` считался проверкой поведения, а
//! `must_contain` — текстом, даже если он демонстрируемо ловит нарушение.
//! Этот модуль замеряет зубья фактически: на КОПИИ кейса во временном
//! каталоге (подход [`crate::redteam`]) каждое правило получает мутацию,
//! которую обязано поймать:
//!
//! - `must_not_contain`: в файл набора вставляется строка, совпадающая с
//!   `pattern` (образец синтезируется из regex [`regex_specimen`] и
//!   **проверяется** скомпилированным шаблоном до записи). Находка обязана
//!   появиться. Под glob нет файлов — статус [`TeethStatus::GlobEmpty`]
//!   (`rule_glob_empty`).
//! - `must_contain` / `each_file_must_contain`: совпадения удаляются из всех
//!   файлов набора. Находка обязана появиться. Совпадений нет — удалять
//!   нечего, статус «не измерено» (а не «с зубьями»).
//! - `file_exists` / `dir_must_have_file`: обязательный файл удаляется.
//! - `command_succeeds`: тривиальная команда (`true`, `:`, `exit 0`, только
//!   `echo`) — [`TeethStatus::Trivial`] (`executable_rule_trivial`): она не
//!   способна упасть. Нетривиальная команда без применённого шаблона с
//!   нарушающей реализацией (`.arch-handoff/rule-templates.lock`, ADR-050) —
//!   [`TeethStatus::Unknown`], а не «с зубьями». С шаблоном — на копии
//!   подставляется нарушающая реализация из библиотеки: правило обязано
//!   покраснеть.
//!
//! Остальные типы (`dependency_direction`, `context_boundary`, `archunit`,
//! `max_age`, `deny_dependency`) мутатором не поддержаны — честный
//! [`TeethStatus::Unknown`], а не молчаливое «с зубьями».
//!
//! Результат сохраняется машиночитаемо в `.arch-handoff/teeth.json`
//! (`--save`; схема `arch-be/rules-teeth/v1`): его читают метрика доверия
//! (`crate::trust`, ступень 3) и отчёт `control rules-report` без пересчёта.
//! Файла нет — честный статус «не измерено». У каждой записи — отпечаток
//! проверяемых полей правила: правка правила после измерения делает запись
//! неприменимой (потребитель считает зубья неизмеренными).

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use regex::Regex;
use serde::{Deserialize, Serialize};

use super::exec::{check, collect_dirs, collect_files, glob_matches};
use super::types::{FitnessRule, RuleKind};
use crate::error::{HarnessError, Result};

/// Путь сохранённого измерения зубьев внутри кейса.
pub const TEETH_RESULT_REL: &str = ".arch-handoff/teeth.json";

/// Схема машиночитаемого файла измерения.
pub const TEETH_SCHEMA: &str = "arch-be/rules-teeth/v1";

/// Куда пишется временный одноправильный реестр внутри копии кейса.
/// `.arch-handoff` исключён из обхода content-правил (`collect_files`), поэтому
/// файл измерения никогда не попадает в набор целей самого правила.
const SINGLE_RULE_REL: &str = ".arch-handoff/teeth-single.yaml";

/// Потолок длины синтезированного образца под regex (страховка от
/// катастрофического `{n}` в шаблоне правила).
const MAX_SPECIMEN_LEN: usize = 512;

/// Статус измерения зубьев одного правила.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TeethStatus {
    /// Мутация дала находку правила — зубья подтверждены измерением.
    Confirmed,
    /// Мутация применена, находки нет — правило беззубое.
    Toothless,
    /// `command_succeeds` с тривиальной командой: упасть она не способна
    /// (`executable_rule_trivial`).
    Trivial,
    /// Под glob правила нет файлов/каталогов — проверять нечего
    /// (`rule_glob_empty`).
    GlobEmpty,
    /// Измерить нечем или вход уже красный — честное «не измерено».
    Unknown,
}

impl TeethStatus {
    /// Машинная метка (как в JSON).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Confirmed => "confirmed",
            Self::Toothless => "toothless",
            Self::Trivial => "trivial",
            Self::GlobEmpty => "glob_empty",
            Self::Unknown => "unknown",
        }
    }

    /// Человеческая метка для отчётов.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Confirmed => "зубья подтверждены",
            Self::Toothless => "беззубое",
            Self::Trivial => "тривиальная команда",
            Self::GlobEmpty => "пустой набор по glob",
            Self::Unknown => "зубья не измерены",
        }
    }

    /// Является ли статус находкой качества реестра (правило не ловит то,
    /// что обязано): влияет на exit-код `rules teeth`.
    #[must_use]
    pub fn is_finding(self) -> bool {
        matches!(self, Self::Toothless | Self::Trivial | Self::GlobEmpty)
    }
}

/// Запись измерения одного правила.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuleTeeth {
    /// Имя правила (код находки в `control check`).
    pub name: String,
    /// Идентификатор (`C-12`), если задан.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Тип правила (`snake_case`, как в YAML).
    pub kind: String,
    /// Статус измерения.
    pub status: TeethStatus,
    /// Отпечаток проверяемых полей правила на момент измерения: правка
    /// тела правила делает запись неприменимой (потребители считают зубья
    /// неизмеренными, а не верят вчерашнему замеру).
    pub fingerprint: String,
    /// Подробность измерения (что мутировали и что вышло).
    pub detail: String,
}

/// Отчёт измерения зубьев реестра (и он же сохраняемая запись,
/// `arch-be/rules-teeth/v1`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeethReport {
    /// Схема файла.
    pub schema: String,
    /// Кейс, на котором измерено.
    pub case: String,
    /// Момент измерения (RFC 3339).
    pub measured_at: String,
    /// Записи по правилам в порядке реестра.
    pub entries: Vec<RuleTeeth>,
}

impl TeethReport {
    /// Правил с подтверждёнными зубьями.
    #[must_use]
    pub fn confirmed(&self) -> usize {
        self.entries
            .iter()
            .filter(|e| e.status == TeethStatus::Confirmed)
            .count()
    }

    /// Записи-находки (беззубые, тривиальные, пустой glob).
    #[must_use]
    pub fn findings(&self) -> Vec<&RuleTeeth> {
        self.entries
            .iter()
            .filter(|e| e.status.is_finding())
            .collect()
    }

    /// Все правила измерены и находок нет.
    #[must_use]
    pub fn passed(&self) -> bool {
        self.findings().is_empty()
    }

    /// Статус правила по имени/идентификатору с проверкой отпечатка: правка
    /// тела правила после измерения обнуляет доверие к записи.
    #[must_use]
    pub fn status_of(&self, rule: &FitnessRule) -> Option<TeethStatus> {
        self.entries
            .iter()
            .find(|e| e.name == rule.name || (e.id.is_some() && e.id == rule.id))
            .filter(|e| e.fingerprint == rule_fingerprint(rule))
            .map(|e| e.status)
    }
}

/// Отпечаток проверяемых полей правила (то, что определяет исход проверки):
/// тип, наборы, шаблон, путь, команда, исключения, таймаут. Карточка
/// (владелец, rationale) и severity на зубья не влияют и в отпечаток не
/// входят.
#[must_use]
pub(crate) fn rule_fingerprint(rule: &FitnessRule) -> String {
    let canon = format!(
        "{}|{}|{}|{}|{}|{}|{}",
        rule.kind.as_str(),
        rule.glob.join("\u{1f}"),
        rule.pattern.as_deref().unwrap_or_default(),
        rule.path.as_deref().unwrap_or_default(),
        rule.command.as_deref().unwrap_or_default(),
        rule.exclude_glob.join("\u{1f}"),
        rule.timeout_secs.map_or(String::new(), |t| t.to_string()),
    );
    crate::hash::sha256_hex(canon.as_bytes())
}

/// Измеряет зубья правил реестра кейса на временных копиях.
///
/// `only_rule` — измерить одно правило (по `id` или `name`); `None` — все
/// правила реестра (включая унаследованные через `extends`: для них
/// одноправильный реестр собрать из чужого файла нельзя — честный
/// [`TeethStatus::Unknown`]).
///
/// Исходный кейс не изменяется (read-only): каждое правило меряется на
/// своей свежей копии во временном каталоге.
///
/// # Errors
/// Кейс/реестр недоступны, реестр не читается или пуст; `only_rule` не
/// найден в реестре.
pub fn measure(case: &Path, only_rule: Option<&str>) -> Result<TeethReport> {
    if !case.is_dir() {
        return Err(HarnessError::Control(format!(
            "кейс недоступен (не каталог): {}",
            case.display()
        )));
    }
    let Some(constraints) = super::resolve_constraints_path(case, None) else {
        return Err(HarnessError::Control(format!(
            "реестр правил не найден: ни {} в корне, ни {}",
            super::ROOT_CONSTRAINTS_PATH,
            super::HANDOFF_CONSTRAINTS_PATH
        )));
    };
    let resolved = super::load_constraints_resolved(&constraints)?;
    if resolved.rules.is_empty() {
        return Err(HarnessError::Control(format!(
            "{}: в реестре нет исполняемых правил — зубья измерять не у чего",
            constraints.display()
        )));
    }
    let source_text =
        std::fs::read_to_string(&constraints).map_err(|e| HarnessError::io(&constraints, e))?;
    let source_yaml: serde_yaml_ng::Value = serde_yaml_ng::from_str(&source_text).map_err(|e| {
        HarnessError::Control(format!(
            "{}: YAML не разбирается: {e}",
            constraints.display()
        ))
    })?;

    let mut rules: Vec<&FitnessRule> = resolved.rules.iter().collect();
    if let Some(want) = only_rule {
        rules.retain(|r| r.name == want || r.id.as_deref() == Some(want));
        if rules.is_empty() {
            let known = resolved
                .rules
                .iter()
                .map(|r| r.id.as_deref().unwrap_or(r.name.as_str()))
                .collect::<Vec<_>>()
                .join(", ");
            return Err(HarnessError::Control(format!(
                "правило '{want}' не найдено в реестре (известны: {known})"
            )));
        }
    }

    let sandbox = Sandbox::new()?;
    let mut entries = Vec::new();
    for rule in rules {
        entries.push(measure_rule(&sandbox, case, &source_yaml, rule));
    }
    Ok(TeethReport {
        schema: TEETH_SCHEMA.to_string(),
        case: case.display().to_string(),
        measured_at: chrono::Local::now().to_rfc3339(),
        entries,
    })
}

/// Сохраняет измерение в `<case>/.arch-handoff/teeth.json` (пишет `--save`;
/// читают `trust` и `control rules-report` без пересчёта).
///
/// # Errors
/// Каталог не создаётся либо файл не пишется.
pub fn save(case: &Path, report: &TeethReport) -> Result<PathBuf> {
    let dir = case.join(".arch-handoff");
    std::fs::create_dir_all(&dir).map_err(|e| HarnessError::io(&dir, e))?;
    let path = dir.join("teeth.json");
    let text = serde_json::to_string_pretty(report)
        .map_err(|e| HarnessError::Config(format!("rules teeth: {e}")))?;
    std::fs::write(&path, text).map_err(|e| HarnessError::io(&path, e))?;
    Ok(path)
}

/// Читает сохранённое измерение; нет файла или он не разбирается — `None`
/// (потребитель не имеет права падать на чужом артефакте: честное «не
/// измерено»).
#[must_use]
pub fn load(case: &Path) -> Option<TeethReport> {
    let text = std::fs::read_to_string(case.join(TEETH_RESULT_REL)).ok()?;
    let report: TeethReport = serde_json::from_str(&text).ok()?;
    (report.schema == TEETH_SCHEMA).then_some(report)
}

/// Текстовый рендер отчёта измерения.
#[must_use]
pub fn render(report: &TeethReport) -> String {
    let mut out = String::new();
    // Записи в String не могут завершиться ошибкой — игноры безопасны.
    let _ = writeln!(out, "Зубья правил реестра: {}", report.case);
    let measured = report
        .entries
        .iter()
        .filter(|e| e.status != TeethStatus::Unknown)
        .count();
    let _ = writeln!(
        out,
        "Правил: {}, измерено: {measured}, с подтверждёнными зубьями: {}, не измерено: {}\n",
        report.entries.len(),
        report.confirmed(),
        report.entries.len() - measured
    );
    for e in &report.entries {
        let mark = match e.status {
            TeethStatus::Confirmed => "✓",
            TeethStatus::Unknown => "·",
            _ => "✗",
        };
        let id = e.id.as_deref().unwrap_or("-");
        let _ = writeln!(
            out,
            "  [{mark}] {id} {} ({}) — {}: {}",
            e.name,
            e.kind,
            e.status.label(),
            e.detail
        );
    }
    let findings = report.findings();
    let _ = writeln!(out);
    if findings.is_empty() {
        let _ = writeln!(out, "Итог: PASS — находок нет");
    } else {
        let _ = writeln!(
            out,
            "Итог: FAIL — находок: {} (беззубые/тривиальные/пустой набор):",
            findings.len()
        );
        for f in findings {
            let _ = writeln!(out, "  ! {} ({}) — {}", f.name, f.status.as_str(), f.detail);
        }
    }
    out
}

/// Машиночитаемый отчёт (`--format json`).
#[must_use]
pub fn to_json(report: &TeethReport) -> serde_json::Value {
    serde_json::to_value(report)
        .unwrap_or_else(|_| serde_json::json!({"schema": TEETH_SCHEMA, "entries": []}))
}

// ---------------------------------------------------------------------------
// Измерение одного правила
// ---------------------------------------------------------------------------

/// Песочница измерения: временный каталог, удаляется на `Drop`.
struct Sandbox {
    root: PathBuf,
}

impl Sandbox {
    fn new() -> Result<Self> {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let root =
            std::env::temp_dir().join(format!("arch-be-teeth-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&root).map_err(|e| HarnessError::io(&root, e))?;
        Ok(Self { root })
    }

    /// Свежий каталог под правило `name`.
    fn slot(&self, name: &str) -> Result<PathBuf> {
        let dest = self.root.join(name);
        if dest.exists() {
            std::fs::remove_dir_all(&dest).map_err(|e| HarnessError::io(&dest, e))?;
        }
        std::fs::create_dir_all(&dest).map_err(|e| HarnessError::io(&dest, e))?;
        Ok(dest)
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        // Игнорируется: песочница во временном каталоге, уборка best-effort.
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// Рекурсивная копия дерева без служебных/тяжёлых каталогов. `.arch-handoff`
/// сохраняется: там живёт lock применённых шаблонов (нужен `command_succeeds`).
fn copy_case(from: &Path, to: &Path) -> Result<()> {
    crate::rule_templates::copy_tree(from, to, &[".git", "target", "node_modules", "__pycache__"])
        .map_err(|e| HarnessError::Control(format!("копия кейса {}: {e}", from.display())))
}

/// Измерение одного правила: своя копия кейса, свой одноправильный реестр.
fn measure_rule(
    sandbox: &Sandbox,
    case: &Path,
    source_yaml: &serde_yaml_ng::Value,
    rule: &FitnessRule,
) -> RuleTeeth {
    let base = RuleTeeth {
        name: rule.name.clone(),
        id: rule.id.clone(),
        kind: rule.kind.as_str().to_string(),
        status: TeethStatus::Unknown,
        fingerprint: rule_fingerprint(rule),
        detail: String::new(),
    };
    let unknown = |detail: String| RuleTeeth {
        status: TeethStatus::Unknown,
        detail,
        ..base.clone()
    };
    if rule.unverifiable {
        return unknown(
            "правило ручного контроля (unverifiable) — движок его не исполняет, зубья не измеряются"
                .to_string(),
        );
    }
    if rule.source.is_some() {
        return unknown(
            "правило унаследовано (extends) — зубья измеряются в репозитории-источнике реестра"
                .to_string(),
        );
    }
    // Одноправильный реестр: узел правила из YAML кейса, без правок текста.
    let single = match single_rule_yaml(source_yaml, &rule.name) {
        Ok(text) => text,
        Err(reason) => return unknown(reason),
    };
    let copy = match sandbox.slot(&rule.name).and_then(|slot| {
        // Контентные правила меряются на разреженной копии (только файлы
        // набора — дёшево на больших репозиториях); `command_succeeds` —
        // на полной (команда может читать что угодно).
        match rule.kind {
            RuleKind::CommandSucceeds => copy_case(case, &slot)?,
            _ => sparse_copy(case, &slot, rule)?,
        }
        Ok(slot)
    }) {
        Ok(slot) => slot,
        Err(e) => return unknown(format!("копия кейса не собрана: {e}")),
    };
    let single_path = copy.join(SINGLE_RULE_REL);
    if let Some(parent) = single_path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            return unknown(format!("{}: {e}", parent.display()));
        }
    }
    if let Err(e) = std::fs::write(&single_path, single) {
        return unknown(format!("{}: {e}", single_path.display()));
    }

    match rule.kind {
        RuleKind::MustNotContain => measure_injection(&copy, &single_path, rule, base),
        RuleKind::MustContain | RuleKind::EachFileMustContain => {
            measure_removal(&copy, &single_path, rule, base)
        }
        RuleKind::FileExists => measure_delete_file(&copy, &single_path, rule, base),
        RuleKind::DirMustHaveFile => measure_delete_dir_file(&copy, &single_path, rule, base),
        RuleKind::CommandSucceeds => measure_command(&copy, &single_path, rule, base),
        other => unknown(format!(
            "тип '{}' мутатором не поддержан — зубья не измерены",
            other.as_str()
        )),
    }
}

/// Итог прогона одного правила на копии: число находок правила и честные
/// пропуски исполнения (нет прогонщика / запрет доверия).
struct RuleRun {
    /// Находок с именем правила.
    findings: usize,
    /// Правило не исполнялось: причина (нет прогонщика / `command_untrusted`).
    not_run: Option<String>,
}

/// Прогоняет одноправильный реестр на копии полным движком `control check`.
fn run_single(
    copy: &Path,
    single_path: &Path,
    rule: &FitnessRule,
) -> std::result::Result<RuleRun, String> {
    let report = check(copy, single_path).map_err(|e| format!("прогон правила: {e}"))?;
    // Правило могло не исполниться: нет прогонщика (A2) или запрет модели
    // доверия (A3) — обе структуры пропуска несут `rule` + `reason`.
    let not_run = report
        .runner_skipped
        .iter()
        .find(|s| s.rule == rule.name)
        .map(|s| s.reason.clone())
        .or_else(|| {
            report
                .untrusted_skipped
                .iter()
                .find(|s| s.rule == rule.name)
                .map(|s| s.reason.clone())
        });
    Ok(RuleRun {
        findings: report.issues.iter().filter(|i| i.rule == rule.name).count(),
        not_run,
    })
}

/// Оценка «мутация → находка»: число находок правила обязано вырасти.
fn verdict(base: RuleTeeth, before: &RuleRun, after: &RuleRun, applied: &str) -> RuleTeeth {
    if let Some(reason) = &after.not_run {
        return RuleTeeth {
            status: TeethStatus::Unknown,
            detail: format!("прогон после мутации не состоялся: {reason}"),
            ..base
        };
    }
    if after.findings > before.findings {
        RuleTeeth {
            status: TeethStatus::Confirmed,
            detail: format!(
                "{applied} — находок правила: {} → {} (зубья подтверждены)",
                before.findings, after.findings
            ),
            ..base
        }
    } else {
        RuleTeeth {
            status: TeethStatus::Toothless,
            detail: format!(
                "{applied} — находок как было, так и осталось ({}) — правило не ловит нарушение",
                after.findings
            ),
            ..base
        }
    }
}

/// Файлы набора правила (glob'ы минус `exclude_glob`) на копии.
fn rule_files(
    copy: &Path,
    rule: &FitnessRule,
) -> std::result::Result<Vec<(String, PathBuf)>, String> {
    let globs: Vec<String> = if rule.glob.is_empty() {
        vec!["**/*".to_string()]
    } else {
        rule.glob.clone()
    };
    let mut files = Vec::new();
    for glob in &globs {
        files.extend(collect_files(copy, glob).map_err(|e| e.to_string())?);
    }
    files.sort_by(|a, b| a.0.cmp(&b.0));
    files.dedup_by(|a, b| a.0 == b.0);
    if !rule.exclude_glob.is_empty() {
        files.retain(|(rel, _)| !rule.exclude_glob.iter().any(|ex| glob_matches(ex, rel)));
    }
    Ok(files)
}

/// Разреженная копия кейса для контентного правила: только файлы набора.
fn sparse_copy(case: &Path, slot: &Path, rule: &FitnessRule) -> Result<()> {
    for (rel, abs) in rule_files(case, rule).map_err(HarnessError::Control)? {
        let dest = slot.join(&rel);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(|e| HarnessError::io(parent, e))?;
        }
        std::fs::copy(&abs, &dest).map_err(|e| HarnessError::io(&dest, e))?;
    }
    // file_exists/dir_must_have_file целятся в путь вне glob-набора — их цель
    // тоже переносим, иначе мутации удалением не на чем играть.
    if let Some(rel) = &rule.path {
        match rule.kind {
            RuleKind::FileExists => {
                let src = case.join(rel);
                if src.is_file() {
                    let dest = slot.join(rel);
                    if let Some(parent) = dest.parent() {
                        std::fs::create_dir_all(parent).map_err(|e| HarnessError::io(parent, e))?;
                    }
                    std::fs::copy(&src, &dest).map_err(|e| HarnessError::io(&dest, e))?;
                }
            }
            RuleKind::DirMustHaveFile => {
                let globs: Vec<String> = if rule.glob.is_empty() {
                    vec!["**/*".to_string()]
                } else {
                    rule.glob.clone()
                };
                for glob in &globs {
                    for dir in collect_dirs(case, glob)? {
                        let src = case.join(&dir).join(rel);
                        if src.is_file() {
                            let dest = slot.join(&dir).join(rel);
                            if let Some(parent) = dest.parent() {
                                std::fs::create_dir_all(parent)
                                    .map_err(|e| HarnessError::io(parent, e))?;
                            }
                            std::fs::copy(&src, &dest).map_err(|e| HarnessError::io(&dest, e))?;
                        }
                    }
                }
            }
            _ => {}
        }
    }
    Ok(())
}

/// `must_not_contain`: вставка строки, совпадающей с `pattern`, в файл набора.
fn measure_injection(
    copy: &Path,
    single_path: &Path,
    rule: &FitnessRule,
    base: RuleTeeth,
) -> RuleTeeth {
    let unknown = |detail: String| RuleTeeth {
        status: TeethStatus::Unknown,
        detail,
        ..base.clone()
    };
    let pattern = rule.pattern.clone().unwrap_or_default();
    let files = match rule_files(copy, rule) {
        Ok(files) => files,
        Err(e) => return unknown(e),
    };
    if files.is_empty() {
        return RuleTeeth {
            status: TeethStatus::GlobEmpty,
            detail: format!(
                "по glob '{}' нет ни одного файла — правилу нечего проверять (`rule_glob_empty`)",
                rule.glob.join(", ")
            ),
            ..base
        };
    }
    let Some(specimen) = regex_specimen(&pattern) else {
        return unknown(format!(
            "не удалось синтезировать строку, совпадающую с pattern '{pattern}' — измерение пропущено"
        ));
    };
    let before = match run_single(copy, single_path, rule) {
        Ok(run) => run,
        Err(e) => return unknown(e),
    };
    // Цель — первый по алфавиту файл набора (детерминизм): вставка туда
    // обязана добавить находку.
    let Some((rel, abs)) = files.first() else {
        return unknown("набор файлов пуст после фильтров".to_string());
    };
    let Ok(content) = std::fs::read_to_string(abs) else {
        return unknown(format!("{rel}: файл не читается как UTF-8"));
    };
    let mutated = format!("{content}\n{specimen}\n");
    if let Err(e) = std::fs::write(abs, mutated) {
        return unknown(format!("{rel}: {e}"));
    }
    let after = match run_single(copy, single_path, rule) {
        Ok(run) => run,
        Err(e) => return unknown(e),
    };
    verdict(
        base,
        &before,
        &after,
        &format!("вставка строки под pattern в {rel}"),
    )
}

/// `must_contain` / `each_file_must_contain`: совпадения удаляются из всех
/// файлов набора — правило обязано покраснеть.
fn measure_removal(
    copy: &Path,
    single_path: &Path,
    rule: &FitnessRule,
    base: RuleTeeth,
) -> RuleTeeth {
    let unknown = |detail: String| RuleTeeth {
        status: TeethStatus::Unknown,
        detail,
        ..base.clone()
    };
    let pattern = rule.pattern.clone().unwrap_or_default();
    let re = match Regex::new(&pattern) {
        Ok(re) => re,
        Err(e) => return unknown(format!("pattern '{pattern}' не компилируется: {e}")),
    };
    let files = match rule_files(copy, rule) {
        Ok(files) => files,
        Err(e) => return unknown(e),
    };
    if files.is_empty() {
        return RuleTeeth {
            status: TeethStatus::GlobEmpty,
            detail: format!(
                "по glob '{}' нет ни одного файла — правилу нечего проверять (`rule_glob_empty`)",
                rule.glob.join(", ")
            ),
            ..base
        };
    }
    let mut matched: Vec<&(String, PathBuf)> = Vec::new();
    for pair in &files {
        let Ok(content) = std::fs::read_to_string(&pair.1) else {
            continue;
        };
        if re.is_match(&content) {
            matched.push(pair);
        }
    }
    if matched.is_empty() {
        return unknown(format!(
            "pattern '{pattern}' не встречается ни в одном файле набора — удалять нечего \
             (правило уже красное либо шаблон нежизнеспособен)"
        ));
    }
    let before = match run_single(copy, single_path, rule) {
        Ok(run) => run,
        Err(e) => return unknown(e),
    };
    let mut touched = Vec::new();
    for (rel, abs) in &matched {
        let content = match std::fs::read_to_string(abs) {
            Ok(content) => content,
            Err(e) => return unknown(format!("{rel}: {e}")),
        };
        let stripped = re.replace_all(&content, "").into_owned();
        if re.is_match(&stripped) {
            return unknown(format!(
                "pattern '{pattern}' после удаления совпадений всё ещё совпадает в {rel} \
                 (нулевая ширина?) — измерение пропущено"
            ));
        }
        if let Err(e) = std::fs::write(abs, stripped) {
            return unknown(format!("{rel}: {e}"));
        }
        touched.push(rel.clone());
    }
    let after = match run_single(copy, single_path, rule) {
        Ok(run) => run,
        Err(e) => return unknown(e),
    };
    verdict(
        base,
        &before,
        &after,
        &format!("удаление совпадений pattern из {}", touched.join(", ")),
    )
}

/// `file_exists`: обязательный файл удаляется — правило обязано покраснеть.
fn measure_delete_file(
    copy: &Path,
    single_path: &Path,
    rule: &FitnessRule,
    base: RuleTeeth,
) -> RuleTeeth {
    let unknown = |detail: String| RuleTeeth {
        status: TeethStatus::Unknown,
        detail,
        ..base.clone()
    };
    let Some(rel) = rule.path.clone() else {
        return unknown("у правила нет path".to_string());
    };
    if !copy.join(&rel).exists() {
        return unknown(format!(
            "файл {rel} уже отсутствует — правило красное до мутации"
        ));
    }
    let before = match run_single(copy, single_path, rule) {
        Ok(run) => run,
        Err(e) => return unknown(e),
    };
    if let Err(e) = std::fs::remove_file(copy.join(&rel)) {
        return unknown(format!("{rel}: {e}"));
    }
    let after = match run_single(copy, single_path, rule) {
        Ok(run) => run,
        Err(e) => return unknown(e),
    };
    verdict(base, &before, &after, &format!("удаление файла {rel}"))
}

/// `dir_must_have_file`: обязательный файл удаляется из первого каталога
/// набора — правило обязано покраснеть.
fn measure_delete_dir_file(
    copy: &Path,
    single_path: &Path,
    rule: &FitnessRule,
    base: RuleTeeth,
) -> RuleTeeth {
    let unknown = |detail: String| RuleTeeth {
        status: TeethStatus::Unknown,
        detail,
        ..base.clone()
    };
    let Some(rel_path) = rule.path.clone() else {
        return unknown("у правила нет path".to_string());
    };
    let globs: Vec<String> = if rule.glob.is_empty() {
        vec!["**/*".to_string()]
    } else {
        rule.glob.clone()
    };
    let mut dirs = Vec::new();
    for glob in &globs {
        match collect_dirs(copy, glob) {
            Ok(found) => dirs.extend(found),
            Err(e) => return unknown(e.to_string()),
        }
    }
    dirs.sort();
    dirs.dedup();
    dirs.retain(|d| !rule.exclude_glob.iter().any(|ex| glob_matches(ex, d)));
    if dirs.is_empty() {
        return RuleTeeth {
            status: TeethStatus::GlobEmpty,
            detail: format!(
                "по glob '{}' нет ни одного каталога — правилу нечего проверять (`rule_glob_empty`)",
                globs.join(", ")
            ),
            ..base
        };
    }
    let Some(target) = dirs.iter().find(|d| copy.join(d).join(&rel_path).is_file()) else {
        return unknown(format!(
            "обязательный файл {rel_path} отсутствует во всех каталогах набора — правило красное до мутации"
        ));
    };
    let before = match run_single(copy, single_path, rule) {
        Ok(run) => run,
        Err(e) => return unknown(e),
    };
    if let Err(e) = std::fs::remove_file(copy.join(target).join(&rel_path)) {
        return unknown(format!("{target}/{rel_path}: {e}"));
    }
    let after = match run_single(copy, single_path, rule) {
        Ok(run) => run,
        Err(e) => return unknown(e),
    };
    verdict(
        base,
        &before,
        &after,
        &format!("удаление {target}/{rel_path}"),
    )
}

/// `command_succeeds`: тривиальная команда — находка `executable_rule_trivial`;
/// нетривиальная измеряется подменой нарушающей реализации из применённого
/// шаблона (ADR-050). Без шаблона — честный `unknown`.
fn measure_command(
    copy: &Path,
    single_path: &Path,
    rule: &FitnessRule,
    base: RuleTeeth,
) -> RuleTeeth {
    let unknown = |detail: String| RuleTeeth {
        status: TeethStatus::Unknown,
        detail,
        ..base.clone()
    };
    let Some(command) = rule.command.clone() else {
        return unknown("у правила нет command".to_string());
    };
    if trivial_command(&command) {
        return RuleTeeth {
            status: TeethStatus::Trivial,
            detail: format!(
                "команда '{command}' не способна упасть (`true`/`:`/`exit 0`/только `echo`) — \
                 правило всегда зелёное (`executable_rule_trivial`)"
            ),
            ..base
        };
    }
    let before = match run_single(copy, single_path, rule) {
        Ok(run) => run,
        Err(e) => return unknown(e),
    };
    if let Some(reason) = &before.not_run {
        return unknown(format!(
            "прогон на немутированной копии не состоялся: {reason}"
        ));
    }
    if before.findings > 0 {
        return unknown(
            "команда уже падает на немутированной копии — зубья не отличить от поломки".to_string(),
        );
    }
    // Шаблон из lock: правило подписано на зубья библиотеки (ADR-050).
    let lock_path = copy.join(crate::rule_templates::LOCK_REL);
    let entries = match crate::rule_templates::read_lock(&lock_path) {
        Ok(entries) => entries,
        Err(e) => return unknown(e.to_string()),
    };
    let entry = entries.iter().find(|e| {
        e.command == command
            || crate::rule_templates::template(&e.id)
                .ok()
                .flatten()
                .is_some_and(|t| t.manifest.rule.name == rule.name)
    });
    let Some(entry) = entry else {
        return unknown(
            "у правила нет применённого шаблона с нарушающей реализацией \
             (.arch-handoff/rule-templates.lock) — зубья не измерены"
                .to_string(),
        );
    };
    let t = match crate::rule_templates::template(&entry.id) {
        Ok(Some(t)) => t,
        Ok(None) => return unknown(format!("шаблона '{}' нет в этой сборке", entry.id)),
        Err(e) => return unknown(e.to_string()),
    };
    if t.manifest.version != entry.version {
        return unknown(format!(
            "применена версия {} шаблона '{}', в сборке {} — нарушающая реализация не та",
            entry.version, entry.id, t.manifest.version
        ));
    }
    if !t.manifest.executable {
        return unknown(format!("шаблон '{}' — заготовка без проверки", entry.id));
    }
    let lang = crate::rule_templates::Lang::parse(&entry.lang)
        .unwrap_or(crate::rule_templates::Lang::Both);
    let swaps = t.violating_for(lang);
    if swaps.is_empty() {
        return unknown(format!(
            "у шаблона '{}' нет нарушающей реализации — зубья не измерены",
            entry.id
        ));
    }
    // Адаптация: файлы кейса разошлись с lock — подмена не доказывает зубья
    // (та же честность, что у `rules template verify`).
    let adapted: Vec<String> = entry
        .files
        .iter()
        .filter_map(|f| match crate::hash::sha256_file(&copy.join(&f.path)) {
            Some(sha) if sha == f.sha256 => None,
            Some(_) => Some(format!("{}: изменён", f.path)),
            None => Some(format!("{}: отсутствует", f.path)),
        })
        .collect();
    if !adapted.is_empty() {
        return unknown(format!(
            "применение адаптировано ({}) — подмена не доказывает зубья, проверьте вручную",
            adapted.join("; ")
        ));
    }
    for swap in &swaps {
        let Some(content) = t.file(&swap.from) else {
            return unknown(format!(
                "шаблон '{}': объявлена нарушающая реализация '{}', которой нет",
                entry.id, swap.from
            ));
        };
        let dest = copy.join(&entry.dir).join(&swap.to);
        if let Some(parent) = dest.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                return unknown(format!("{}: {e}", parent.display()));
            }
        }
        if let Err(e) = std::fs::write(&dest, content) {
            return unknown(format!("{}: {e}", dest.display()));
        }
    }
    let after = match run_single(copy, single_path, rule) {
        Ok(run) => run,
        Err(e) => return unknown(e),
    };
    let applied = format!(
        "подмена реализации на нарушающую из шаблона '{}' ({})",
        entry.id, entry.dir
    );
    verdict(base, &before, &after, &applied)
}

/// Одноправильный реестр: YAML-узел правила из текста реестра кейса под
/// корнем `rules:`/`constraints:` — тело правила доезжает до движка без
/// переписывания (regex и кавычки сохраняются побитово).
fn single_rule_yaml(
    source: &serde_yaml_ng::Value,
    name: &str,
) -> std::result::Result<String, String> {
    let root_key = ["rules", "constraints"]
        .into_iter()
        .find(|key| {
            source
                .get(*key)
                .and_then(serde_yaml_ng::Value::as_sequence)
                .is_some()
        })
        .ok_or_else(|| "в реестре нет корня `rules:`/`constraints:`".to_string())?;
    let seq = source
        .get(root_key)
        .and_then(serde_yaml_ng::Value::as_sequence)
        .ok_or_else(|| format!("корень `{root_key}` не список"))?;
    let node = seq
        .iter()
        .find(|item| item.get("name").and_then(serde_yaml_ng::Value::as_str) == Some(name))
        .ok_or_else(|| {
            format!("узел правила '{name}' не найден в тексте реестра (унаследовано?)")
        })?;
    let body = serde_yaml_ng::to_string(node).map_err(|e| format!("сериализация узла: {e}"))?;
    let body = body.strip_prefix("---\n").unwrap_or(&body);
    let mut out = String::from("rules:\n");
    let mut first = true;
    for line in body.lines() {
        if first {
            let _ = writeln!(out, "  - {line}");
            first = false;
        } else {
            let _ = writeln!(out, "    {line}");
        }
    }
    Ok(out)
}

/// Тривиальная команда (`executable_rule_trivial`): все сегменты (по `&&`,
/// `||`, `;`, `|`) — из набора «не умеет падать»: `true`, `:`, `exit 0`,
/// `echo …`, пустой/комментарий. Такая команда зелёная при любом состоянии
/// кода, и называть её проверкой поведения нельзя.
#[must_use]
pub(crate) fn trivial_command(command: &str) -> bool {
    let mut segments = 0usize;
    for raw in command.split(['&', '|', ';']) {
        // Комментарий shell (# до конца строки) на решение не влияет.
        let seg = raw.split('#').next().unwrap_or_default().trim();
        if seg.is_empty() {
            continue;
        }
        segments += 1;
        let trivial = seg == "true"
            || seg == ":"
            || seg == "exit 0"
            || seg == "echo"
            || seg.starts_with("echo ");
        if !trivial {
            return false;
        }
    }
    segments > 0 || command.trim().is_empty()
}

// ---------------------------------------------------------------------------
// Образец строки под regex (вставка для must_not_contain, комментарий D18)
// ---------------------------------------------------------------------------

/// Синтезирует строку, совпадающую с `pattern`: разбирает подмножество
/// синтаксиса regex (литералы, `\d \w \s` и обратные, классы `[…]` с диапазонами
/// и отрицанием, группы `(…)`/`(?:…)`/флаги `(?i)`, повторы `* + ? {n[,m]}`,
/// альтернативу `|` — берётся первая ветка) и **проверяет** результат
/// скомпилированным шаблоном. Неподдержанная конструкция или проверка не
/// сошлась — `None` (измерение честно пропускается, а не угадывает).
///
/// Результат однострочный: `\n`/`\r` в шаблоне делают синтез неприменимым
/// (content-правила проверяют построчно/по файлу, многострочный образец —
/// отдельная механика).
#[must_use]
pub(crate) fn regex_specimen(pattern: &str) -> Option<String> {
    let chars: Vec<char> = pattern.chars().collect();
    let mut parser = SpecimenGen {
        chars: &chars,
        pos: 0,
        out: String::new(),
    };
    parser.sequence(0)?;
    let specimen = parser.out;
    if specimen.chars().count() > MAX_SPECIMEN_LEN {
        return None;
    }
    let re = Regex::new(pattern).ok()?;
    re.is_match(&specimen).then_some(specimen)
}

/// Генератор образца: позиционный разбор regex с записью результата.
struct SpecimenGen<'a> {
    /// Шаблон посимвольно.
    chars: &'a [char],
    /// Текущая позиция разбора.
    pos: usize,
    /// Накопленный образец.
    out: String,
}

impl SpecimenGen<'_> {
    /// Текущий символ.
    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    /// Текущий символ со сдвигом.
    fn take(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.pos += 1;
        Some(c)
    }

    /// Последовательность атомов до `)` или `|` на глубине `depth`;
    /// альтернатива: генерируется первая ветка, остальные пропускаются.
    /// Конец ввода — нормальное завершение (шаблон без групп исчерпан).
    /// На верхнем уровне (depth 0) `|` означает просто конец генерации:
    /// первая ветка уже сгенерирована, остаток шаблона не читается.
    fn sequence(&mut self, depth: usize) -> Option<()> {
        loop {
            let Some(c) = self.peek() else {
                return Some(());
            };
            match c {
                ')' if depth > 0 => return Some(()),
                '|' => {
                    if depth > 0 {
                        self.skip_to_group_end(depth)?;
                    }
                    return Some(());
                }
                _ => self.atom_with_quantifier()?,
            }
        }
    }

    /// Пропускает оставшиеся ветки альтернативы до закрывающей `)` группы.
    /// Сама `)` НЕ потребляется — её забирает `group_body`.
    fn skip_to_group_end(&mut self, depth: usize) -> Option<()> {
        let mut level = depth;
        while let Some(c) = self.peek() {
            match c {
                '(' => {
                    self.pos += 1;
                    level += 1;
                }
                ')' => {
                    if level == depth {
                        return Some(());
                    }
                    self.pos += 1;
                    level -= 1;
                }
                '\\' => {
                    self.pos += 1;
                    self.take()?;
                }
                _ => {
                    self.pos += 1;
                }
            }
        }
        None
    }

    /// Атом и его квантификатор.
    fn atom_with_quantifier(&mut self) -> Option<()> {
        let mark = self.out.len();
        self.atom()?;
        self.quantifier(mark)
    }

    /// Квантификатор после атома: повторяет или отбрасывает сгенерированное.
    fn quantifier(&mut self, mark: usize) -> Option<()> {
        let piece = self.out.get(mark..)?.to_string();
        match self.peek() {
            Some('*') => {
                self.pos += 1;
                self.out.truncate(mark); // ноль повторов — минимальный образец
            }
            Some('+') => {
                self.pos += 1; // один экземпляр уже сгенерирован
            }
            Some('?') => {
                self.pos += 1;
                self.out.truncate(mark); // необязательный элемент опускаем
            }
            Some('{') => {
                let (n, consumed) = self.parse_brace()?;
                if consumed {
                    self.out.truncate(mark);
                    for _ in 0..n {
                        self.out.push_str(&piece);
                    }
                }
            }
            _ => return Some(()),
        }
        // Ленивый суффикс (`*?`, `+?`, …) на образец не влияет.
        if self.peek() == Some('?') {
            self.pos += 1;
        }
        Some(())
    }

    /// `{n}` / `{n,}` / `{n,m}` → (n, был ли это квантификатор). Не `{` —
    /// (0, false), позиция не двигается.
    fn parse_brace(&mut self) -> Option<(usize, bool)> {
        let save = self.pos;
        self.pos += 1; // '{'
        let mut digits = String::new();
        while let Some(c) = self.peek() {
            if c.is_ascii_digit() {
                digits.push(c);
                self.pos += 1;
            } else {
                break;
            }
        }
        if digits.is_empty() {
            self.pos = save;
            return Some((0, false));
        }
        if self.peek() == Some(',') {
            self.pos += 1;
            while let Some(c) = self.peek() {
                if c.is_ascii_digit() {
                    self.pos += 1;
                } else {
                    break;
                }
            }
        }
        if self.take() != Some('}') {
            self.pos = save;
            return Some((0, false));
        }
        let n = digits.parse().ok()?;
        Some((n, true))
    }

    /// Один атом шаблона.
    fn atom(&mut self) -> Option<()> {
        match self.take()? {
            '(' => self.group(),
            '[' => self.class(),
            '\\' => self.escape(),
            '^' | '$' => Some(()),
            '.' => {
                self.out.push('x');
                Some(())
            }
            c => {
                self.out.push(c);
                Some(())
            }
        }
    }

    /// Группа: `(?:…)`, `(?flags)`, `(?flags:…)`, `(?P<name>…)`, обычная.
    fn group(&mut self) -> Option<()> {
        if self.peek() == Some('?') {
            self.pos += 1;
            match self.take()? {
                ':' => return self.group_body(),
                'P' => {
                    // Именованная группа (?P<name>…) — имя пропускаем.
                    if self.take() != Some('<') {
                        return None;
                    }
                    while let Some(c) = self.take() {
                        if c == '>' {
                            break;
                        }
                    }
                    return self.group_body();
                }
                _flag => {
                    // Строка флагов до ')' (глобальные) или ':' (скоуп).
                    let mut scoped = false;
                    while let Some(c) = self.take() {
                        match c {
                            ')' => return Some(()),
                            ':' => {
                                scoped = true;
                                break;
                            }
                            c if c.is_ascii_alphabetic() || c == '-' => {}
                            _ => return None,
                        }
                    }
                    return if scoped { self.group_body() } else { Some(()) };
                }
            }
        }
        self.group_body()
    }

    /// Тело группы до закрывающей `)`.
    fn group_body(&mut self) -> Option<()> {
        self.sequence(1)?;
        if self.take() != Some(')') {
            return None;
        }
        Some(())
    }

    /// Экранированный символ/класс.
    fn escape(&mut self) -> Option<()> {
        match self.take()? {
            'd' => self.out.push('7'),
            'D' | 'W' | 'S' => self.out.push('!'),
            'w' => self.out.push('x'),
            's' => self.out.push(' '),
            't' => self.out.push('\t'),
            // Многострочные конструкции (образец обязан остаться одной строкой),
            // прокси-классы и hex — не поддерживаем.
            'n' | 'r' | 'f' | 'v' | 'p' | 'P' | 'x' | 'u' | '0'..='9' => return None,
            // Якоря нулевой ширины: ничего не излучают.
            'b' | 'B' | 'A' | 'z' | 'Z' => {}
            // Экранированный литерал: `\.`, `\(`, `\\` и т.п.
            other => self.out.push(other),
        }
        Some(())
    }

    /// Класс `[…]`: первый подходящий представитель; для отрицания `[^…]` —
    /// первый из кандидатов вне набора.
    fn class(&mut self) -> Option<()> {
        let negated = self.peek() == Some('^');
        if negated {
            self.pos += 1;
        }
        // `[]]` — `]` литерал на первой позиции.
        let mut first = true;
        let mut items: Vec<ClassItem> = Vec::new();
        loop {
            let c = self.take()?;
            if c == ']' && !first {
                break;
            }
            first = false;
            if c == '\\' {
                match self.take()? {
                    d @ ('d' | 'D' | 'w' | 'W' | 's' | 'S') => items.push(ClassItem::Meta(d)),
                    'n' => items.push(ClassItem::Char('\n')),
                    't' => items.push(ClassItem::Char('\t')),
                    other => items.push(ClassItem::Char(other)),
                }
                continue;
            }
            // Диапазон `a-z` (но не `a-]`, где `-` литерал перед закрытием).
            if self.peek() == Some('-') && self.chars.get(self.pos + 1).is_some_and(|&n| n != ']') {
                self.pos += 1; // '-'
                let hi = self.take()?;
                items.push(ClassItem::Range(c, hi));
                continue;
            }
            if c == ']' {
                // Одинокое `]` не на первой позиции — конец класса (выше);
                // сюда попадает только первое `]` — литерал.
                items.push(ClassItem::Char(']'));
                continue;
            }
            items.push(ClassItem::Char(c));
        }
        let pick = if negated {
            ['x', '7', ' ', '_', '!', 'a', '0', '-']
                .into_iter()
                .find(|c| !items.iter().any(|i| i.contains(*c)))?
        } else {
            items.first()?.representative()?
        };
        self.out.push(pick);
        Some(())
    }
}

/// Элемент класса `[…]` для подбора представителя.
enum ClassItem {
    /// Литеральный символ.
    Char(char),
    /// Диапазон `c1-c2`.
    Range(char, char),
    /// Мета-класс (`\d`, `\w`, `\s` и обратные).
    Meta(char),
}

impl ClassItem {
    /// Представитель для позитивного класса.
    fn representative(&self) -> Option<char> {
        match self {
            Self::Char(c) => Some(*c),
            Self::Range(lo, _) => Some(*lo),
            Self::Meta('d') => Some('7'),
            Self::Meta('w' | 'S') => Some('x'),
            Self::Meta('s') => Some(' '),
            Self::Meta('D' | 'W') => Some('!'),
            Self::Meta(_) => None,
        }
    }

    /// Входит ли символ в элемент (для отрицаемого класса).
    fn contains(&self, c: char) -> bool {
        match self {
            Self::Char(x) => *x == c,
            Self::Range(lo, hi) => (*lo..=*hi).contains(&c),
            Self::Meta('d') => c.is_ascii_digit(),
            Self::Meta('D') => !c.is_ascii_digit(),
            Self::Meta('w') => c.is_alphanumeric() || c == '_',
            Self::Meta('W') => !(c.is_alphanumeric() || c == '_'),
            Self::Meta('s') => c.is_whitespace(),
            Self::Meta('S') => !c.is_whitespace(),
            Self::Meta(_) => false,
        }
    }
}

// ---------------------------------------------------------------------------
// Раздел «зубья» для отчётов (rules-report, trust): три группы по измерению
// ---------------------------------------------------------------------------

/// Делит правила реестра на три группы по сохранённому измерению зубьев
/// (B1): «проверяют поведение (зубья подтверждены)», «текст» (измерены и
/// поведение НЕ подтверждено: беззубые/тривиальные/пустой glob), «зубья не
/// измерены» (статус unknown, нет записи, отпечаток не сошёлся либо файла
/// измерения нет вовсе — честное «не измерено», а не «с зубьями»).
#[derive(Debug)]
pub struct TeethGroups<'a> {
    /// Правила с подтверждёнными зубьями.
    pub confirmed: Vec<&'a str>,
    /// Измерены, поведение не подтверждено (по сути — текст/мертвецы).
    pub text: Vec<&'a str>,
    /// Не измерены.
    pub unmeasured: Vec<&'a str>,
}

/// Раскладывает правила по группам зубьев. `None` — измерения нет вообще:
/// ВСЕ правила уходят в «не измерены» вызывающей стороной (она сама решит,
/// как это показать).
#[must_use]
pub fn groups<'a>(rules: &'a [&FitnessRule], teeth: Option<&TeethReport>) -> TeethGroups<'a> {
    let mut confirmed = Vec::new();
    let mut text = Vec::new();
    let mut unmeasured = Vec::new();
    for rule in rules {
        match teeth.and_then(|t| t.status_of(rule)) {
            Some(TeethStatus::Confirmed) => confirmed.push(rule.name.as_str()),
            Some(status) if status.is_finding() => text.push(rule.name.as_str()),
            _ => unmeasured.push(rule.name.as_str()),
        }
    }
    TeethGroups {
        confirmed,
        text,
        unmeasured,
    }
}

/// Первый (по реестру, затем по алфавиту файлов) `must_contain` реестра, чей
/// pattern встречается в файле набора С КОДОМ (лексический мутатор D18
/// redteam, B2: удаляется проверка в КОДЕ скелета — правила на документы
/// лексического обхода не показывают, а их правка ловится другими
/// составляющими не по делу). При пустом `want_rel` — первый файл набора с
/// совпадением; иначе — только он. Возвращает (относительный путь файла,
/// pattern правила).
#[must_use]
pub(crate) fn first_must_contain_match(root: &Path, want_rel: &str) -> Option<(String, String)> {
    let constraints = super::resolve_constraints_path(root, None)?;
    let resolved = super::load_constraints_resolved(&constraints).ok()?;
    for rule in &resolved.rules {
        if !matches!(rule.kind, RuleKind::MustContain) {
            continue;
        }
        let Some(pattern) = rule.pattern.clone() else {
            continue;
        };
        let re = Regex::new(&pattern).ok()?;
        let globs: Vec<String> = if rule.glob.is_empty() {
            vec!["**/*".to_string()]
        } else {
            rule.glob.clone()
        };
        for glob in &globs {
            let files = collect_files(root, glob).ok()?;
            for (rel, abs) in files {
                if rule.exclude_glob.iter().any(|ex| glob_matches(ex, &rel)) {
                    continue;
                }
                if !want_rel.is_empty() && rel != want_rel {
                    continue;
                }
                if want_rel.is_empty() && !is_code_file(&rel) {
                    continue;
                }
                let Ok(content) = std::fs::read_to_string(&abs) else {
                    continue;
                };
                if re.is_match(&content) {
                    return Some((rel, pattern));
                }
            }
        }
    }
    None
}

/// Файл — код скелета/реализации (для D18): по расширению или по месту
/// (`skeleton/`, `src/`). Документы и модель — не цель лексического обхода.
fn is_code_file(rel: &str) -> bool {
    if rel.starts_with("skeleton/") || rel.starts_with("src/") {
        return true;
    }
    matches!(
        rel.rsplit('.').next(),
        Some("py" | "rs" | "go" | "java" | "kt" | "ts" | "js" | "c" | "cpp" | "h" | "sh")
    )
}

/// Доля правил с подтверждёнными зубьями (числитель, знаменатель) по
/// сохранённому измерению. Без измерения — (0, все): «не измерено» ≠
/// «с зубьями» (волна B: тип правила — не доказательство).
#[must_use]
pub fn confirmed_share(rules: &[&FitnessRule], teeth: Option<&TeethReport>) -> (usize, usize) {
    let g = groups(rules, teeth);
    (g.confirmed.len(), rules.len())
}

/// Секция отчёта о трёх группах зубьев (для `rules_report`).
#[must_use]
pub fn render_groups(rules: &[&FitnessRule], teeth: Option<&TeethReport>) -> String {
    let g = groups(rules, teeth);
    let total = rules.len();
    let mut out = String::from("\n## Зубья правил (B1)\n");
    match teeth {
        None => {
            let _ = writeln!(
                out,
                "Измерения нет — прогоните `arch-be rules teeth --save`: зубья не измерены ни у одного из {total} правил."
            );
            let _ = writeln!(
                out,
                "Доля «проверяют поведение» без измерения не заявляется: тип правила — не доказательство."
            );
        }
        Some(report) => {
            let _ = writeln!(
                out,
                "Измерение: {} ({})",
                report.measured_at, TEETH_RESULT_REL
            );
            let _ = writeln!(
                out,
                "Проверяют поведение (зубья подтверждены): {} из {total}",
                g.confirmed.len()
            );
            for name in &g.confirmed {
                let _ = writeln!(out, "- {name}");
            }
            let _ = writeln!(out, "Текст (зубья не подтверждены): {}", g.text.len());
            for name in &g.text {
                let _ = writeln!(out, "- {name}");
            }
            let _ = writeln!(out, "Зубья не измерены: {}", g.unmeasured.len());
            for name in &g.unmeasured {
                let _ = writeln!(out, "- {name}");
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Пишет файл в кейс и возвращает его путь.
    fn write_file(case: &Path, name: &str, content: &str) {
        let p = case.join(name);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).expect("mkdir");
        }
        std::fs::write(&p, content).expect("write");
    }

    /// Кейс с реестром `constraints_yaml` в корне.
    fn case_with(constraints_yaml: &str) -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tmp");
        write_file(dir.path(), "CONSTRAINTS.yaml", constraints_yaml);
        dir
    }

    /// Запись измерения по имени правила.
    fn entry<'a>(report: &'a TeethReport, name: &str) -> &'a RuleTeeth {
        report
            .entries
            .iter()
            .find(|e| e.name == name)
            .unwrap_or_else(|| panic!("нет записи о правиле {name}: {:?}", report.entries))
    }

    // --- приёмка B1: четыре класса ситуаций ---------------------------------

    /// `command: 'true'` — тривиальная команда: находка `executable_rule_trivial`,
    /// а НЕ «правило с зубьями» (до фикса тип `command_succeeds` считался
    /// поведенческим по определению — репродукция B1).
    #[test]
    fn trivial_true_command_is_a_finding_not_teeth() {
        let case = case_with(
            "rules:\n  - id: C-001\n    name: ci_green\n    type: command_succeeds\n    \
             command: 'true'\n    severity: error\n",
        );
        let report = measure(case.path(), None).expect("измерение");
        let e = entry(&report, "ci_green");
        assert_eq!(e.status, TeethStatus::Trivial, "{e:?}");
        assert!(e.detail.contains("executable_rule_trivial"), "{e:?}");
        assert!(!report.passed(), "тривиальное правило — находка качества");
        for cmd in [
            "true",
            ":",
            "exit 0",
            "echo ok",
            "true && echo done",
            " : ",
            "true # ok",
        ] {
            assert!(trivial_command(cmd), "{cmd} тривиальна");
        }
        for cmd in [
            "false",
            "exit 1",
            "pytest -q",
            "grep -q x f.py",
            "true && false",
        ] {
            assert!(!trivial_command(cmd), "{cmd} нетривиальна");
        }
    }

    /// Glob по несуществующему пути: находка `rule_glob_empty` — правилу
    /// нечего проверять, и это видно как дефект реестра, а не как зелёный.
    #[test]
    fn empty_glob_set_is_rule_glob_empty() {
        let case = case_with(
            "rules:\n  - id: C-002\n    name: no_pan\n    type: must_not_contain\n    \
             glob: 'src/**/*.rs'\n    pattern: 'PAN'\n    severity: error\n",
        );
        // Ни одного .rs-файла в кейсе нет.
        let report = measure(case.path(), None).expect("измерение");
        let e = entry(&report, "no_pan");
        assert_eq!(e.status, TeethStatus::GlobEmpty, "{e:?}");
        assert!(e.detail.contains("rule_glob_empty"), "{e:?}");
        assert!(!report.passed());
    }

    /// `pattern: 'x^'` ничего не совпадает: для `must_contain` удалять нечего —
    /// честный `unknown`, а не «зубья подтверждены» и не беззубое.
    #[test]
    fn never_matching_pattern_is_honestly_unmeasurable() {
        let case = case_with(
            "rules:\n  - id: C-003\n    name: has_marker\n    type: must_contain\n    \
             glob: 'src/**/*.py'\n    pattern: 'x^'\n    severity: error\n",
        );
        write_file(case.path(), "src/main.py", "x = 1\n");
        let report = measure(case.path(), None).expect("измерение");
        let e = entry(&report, "has_marker");
        assert_eq!(e.status, TeethStatus::Unknown, "{e:?}");
        assert!(e.detail.contains("удалять нечего"), "{e:?}");
        // Неизмеренное — не находка качества реестра: прогон не красный.
        assert!(report.passed(), "{:?}", report.entries);
    }

    /// Честное правило: вставка строки под pattern в файл набора даёт находку —
    /// зубья подтверждены измерением.
    #[test]
    fn honest_must_not_contain_rule_is_confirmed() {
        let case = case_with(
            "rules:\n  - id: C-004\n    name: no_pan\n    type: must_not_contain\n    \
             glob: 'src/**/*.py'\n    pattern: 'PAN'\n    severity: error\n",
        );
        write_file(case.path(), "src/main.py", "def charge():\n    return 1\n");
        let report = measure(case.path(), None).expect("измерение");
        let e = entry(&report, "no_pan");
        assert_eq!(e.status, TeethStatus::Confirmed, "{e:?}");
        assert!(e.detail.contains("вставка"), "{e:?}");
        // Исходный кейс не тронут: read-only.
        let content = std::fs::read_to_string(case.path().join("src/main.py")).expect("read");
        assert!(!content.contains("PAN"), "кейс не мутирован: {content}");
    }

    /// `must_contain`: удаление совпадений обязано покраснить правило.
    #[test]
    fn must_contain_is_confirmed_by_removal() {
        let case = case_with(
            "rules:\n  - id: C-005\n    name: idem\n    type: must_contain\n    \
             glob: 'skeleton/**/*.py'\n    pattern: 'idempotency_key'\n    severity: error\n",
        );
        write_file(
            case.path(),
            "skeleton/api.py",
            "def take(idempotency_key):\n    return idempotency_key\n",
        );
        let report = measure(case.path(), None).expect("измерение");
        assert_eq!(entry(&report, "idem").status, TeethStatus::Confirmed);
    }

    /// `file_exists`: удаление обязательного файла обязано покраснить правило.
    #[test]
    fn file_exists_is_confirmed_by_deletion() {
        let case = case_with(
            "rules:\n  - id: C-006\n    name: spine_present\n    type: file_exists\n    \
             path: 'ARCHITECTURE-SPINE.md'\n    severity: error\n",
        );
        write_file(case.path(), "ARCHITECTURE-SPINE.md", "# Spine\n");
        let report = measure(case.path(), None).expect("измерение");
        assert_eq!(
            entry(&report, "spine_present").status,
            TeethStatus::Confirmed
        );
    }

    /// `command_succeeds` без применённого шаблона — честный `unknown`,
    /// а не «с зубьями» (волна B: шаблон с нарушающей реализацией — единственный
    /// способ измерить зубья команды).
    #[test]
    fn command_without_template_is_unknown_not_toothy() {
        let case = case_with(
            "rules:\n  - id: C-007\n    name: tests_run\n    type: command_succeeds\n    \
             command: 'grep -q x README.md'\n    severity: error\n",
        );
        write_file(case.path(), "README.md", "x\n");
        let report = measure(case.path(), None).expect("измерение");
        let e = entry(&report, "tests_run");
        assert_eq!(e.status, TeethStatus::Unknown, "{e:?}");
        assert!(e.detail.contains("rule-templates.lock"), "{e:?}");
    }

    /// Кейс с применённым шаблоном `idempotency-key` и правилом-грепом по
    /// проверяемой конструкции: на эталоне зелёный, после подмены нарушающей
    /// реализацией — красный. Зубья подтверждены без внешних прогонщиков.
    fn case_with_template_rule(command: &str) -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tmp");
        let case = dir.path();
        let t = crate::rule_templates::template("idempotency-key")
            .expect("сборка")
            .expect("шаблон в сборке");
        let tdir = format!("{}/idempotency-key", crate::rule_templates::TARGET_REL);
        let mut files_yaml = String::new();
        for f in t.files_for(crate::rule_templates::Lang::Python) {
            let content = t.file(&f.from).expect("файл шаблона");
            let rel = format!("{tdir}/{}", f.to);
            write_file(case, &rel, content);
            let _ = writeln!(
                files_yaml,
                "      - path: {rel}\n        sha256: '{}'",
                crate::hash::sha256_hex(content.as_bytes())
            );
        }
        let lock = format!(
            "# тест\ntemplates:\n  - id: idempotency-key\n    version: {}\n    ad: AD-1\n    \
             lang: python\n    dir: {tdir}\n    command: '{command}'\n    files:\n{files_yaml}",
            t.manifest.version
        );
        write_file(case, crate::rule_templates::LOCK_REL, &lock);
        let constraints = format!(
            "rules:\n  - id: C-008\n    name: idempotency_key_enforced\n    type: command_succeeds\n    \
             command: '{command}'\n    severity: error\n"
        );
        write_file(case, "CONSTRAINTS.yaml", &constraints);
        dir
    }

    /// Правило из шаблона: нарушающая реализация краснит команду — зубья
    /// подтверждены. Беззубая команда (конструкция есть в обеих реализациях) —
    /// находка `executable_rule_toothless`-уровня (статус toothless).
    #[test]
    fn template_backed_command_is_measured_by_the_violating_impl() {
        // Зубастая команда: следит за дедупликацией (её нет в нарушающей
        // реализации) — после подмены падает.
        let case = case_with_template_rule(
            "grep -q _answers skeleton/rule_templates/idempotency-key/reference_impl.py",
        );
        let report = measure(case.path(), None).expect("измерение");
        let e = entry(&report, "idempotency_key_enforced");
        assert_eq!(e.status, TeethStatus::Confirmed, "{e:?}");
        assert!(e.detail.contains("нарушающую"), "{e:?}");

        // Беззубая команда: маркер есть в обеих реализациях — подмена не
        // меняет исхода, правило не ловит нарушение.
        let case = case_with_template_rule(
            "grep -q PaymentService skeleton/rule_templates/idempotency-key/reference_impl.py",
        );
        let report = measure(case.path(), None).expect("измерение");
        let e = entry(&report, "idempotency_key_enforced");
        assert_eq!(e.status, TeethStatus::Toothless, "{e:?}");
        assert!(!report.passed());
    }

    /// Сохранение и чтение измерения: потребители (trust, rules-report)
    /// читают файл без пересчёта; правка тела правила обнуляет доверие к
    /// записи (отпечаток не сошёлся — «не измерено»).
    #[test]
    fn save_load_roundtrip_and_staleness_by_fingerprint() {
        let case = case_with(
            "rules:\n  - id: C-009\n    name: no_pan\n    type: must_not_contain\n    \
             glob: 'src/**/*.py'\n    pattern: 'PAN'\n    severity: error\n",
        );
        write_file(case.path(), "src/main.py", "x = 1\n");
        let report = measure(case.path(), None).expect("измерение");
        assert_eq!(entry(&report, "no_pan").status, TeethStatus::Confirmed);
        let path = save(case.path(), &report).expect("сохранение");
        assert_eq!(path, case.path().join(TEETH_RESULT_REL));
        let back = load(case.path()).expect("чтение сохранённого");
        assert_eq!(back.schema, TEETH_SCHEMA);
        assert_eq!(back.confirmed(), 1);

        // Потребительская проверка: статус по СВЕЖЕМУ правилу совпадает.
        let resolved =
            super::super::load_constraints_resolved(&case.path().join("CONSTRAINTS.yaml"))
                .expect("реестр");
        let rule = &resolved.rules[0];
        assert_eq!(back.status_of(rule), Some(TeethStatus::Confirmed));

        // Правка тела правила (другой pattern) — запись неприменима.
        write_file(
            case.path(),
            "CONSTRAINTS.yaml",
            "rules:\n  - id: C-009\n    name: no_pan\n    type: must_not_contain\n    \
             glob: 'src/**/*.py'\n    pattern: 'CVV'\n    severity: error\n",
        );
        let resolved =
            super::super::load_constraints_resolved(&case.path().join("CONSTRAINTS.yaml"))
                .expect("реестр");
        let rule = &resolved.rules[0];
        assert_eq!(
            back.status_of(rule),
            None,
            "правка правила после измерения обнуляет запись"
        );
    }

    /// Фильтр `--rule`: измеряется одно правило; неизвестное имя — ошибка с
    /// перечнем известных.
    #[test]
    fn measure_filters_by_rule_id_or_name() {
        let case = case_with(
            "rules:\n  - id: C-010\n    name: a_rule\n    type: file_exists\n    \
             path: 'A.md'\n  - id: C-011\n    name: b_rule\n    type: file_exists\n    \
             path: 'B.md'\n",
        );
        write_file(case.path(), "A.md", "a\n");
        write_file(case.path(), "B.md", "b\n");
        let report = measure(case.path(), Some("C-011")).expect("по id");
        assert_eq!(report.entries.len(), 1);
        assert_eq!(report.entries[0].name, "b_rule");
        let report = measure(case.path(), Some("a_rule")).expect("по имени");
        assert_eq!(report.entries[0].id.as_deref(), Some("C-010"));
        let err = measure(case.path(), Some("C-999")).expect_err("нет такого");
        assert!(err.to_string().contains("C-010"), "{err}");
    }

    /// Три группы зубьев: подтверждённые / текст / не измерены; без файла
    /// измерения — все «не измерены» (а не «с зубьями»).
    #[test]
    fn groups_split_rules_by_measured_teeth() {
        let case = case_with(
            "rules:\n  - id: C-012\n    name: toothy\n    type: must_not_contain\n    \
             glob: 'src/**/*.py'\n    pattern: 'PAN'\n    severity: error\n  \
             - id: C-013\n    name: trivial\n    type: command_succeeds\n    \
             command: 'true'\n    severity: error\n  \
             - id: C-014\n    name: structural\n    type: file_exists\n    \
             path: 'missing-dir/x.md'\n    severity: error\n",
        );
        write_file(case.path(), "src/main.py", "x = 1\n");
        let report = measure(case.path(), None).expect("измерение");
        save(case.path(), &report).expect("сохранение");
        let resolved =
            super::super::load_constraints_resolved(&case.path().join("CONSTRAINTS.yaml"))
                .expect("реестр");
        let rules: Vec<&FitnessRule> = resolved.rules.iter().collect();
        let teeth = load(case.path());
        let g = groups(&rules, teeth.as_ref());
        assert_eq!(g.confirmed, ["toothy"]);
        assert_eq!(g.text, ["trivial"], "тривиальная команда — «текст»");
        assert_eq!(g.unmeasured, ["structural"], "{g:?}");
        // Без измерения: все не измерены.
        let g = groups(&rules, None);
        assert!(g.confirmed.is_empty() && g.text.is_empty());
        assert_eq!(g.unmeasured.len(), 3);
        let text = render_groups(&rules, None);
        assert!(text.contains("Измерения нет"), "{text}");
        assert!(text.contains("rules teeth --save"), "{text}");
        let text = render_groups(&rules, teeth.as_ref());
        assert!(
            text.contains("Проверяют поведение (зубья подтверждены): 1 из 3"),
            "{text}"
        );
    }

    // --- генератор образцов regex ---------------------------------------------

    /// Образец совпадает со шаблоном: литералы, классы, повторы, альтернативы,
    /// флаги — включая боевые шаблоны корпуса `experiments/openspec-vs-spine`.
    #[test]
    fn specimen_matches_the_pattern_it_was_built_from() {
        let cases = [
            "PAN",
            r"\b\d{16}\b",
            r"\bf(64|32)\b",
            "(?i)idempotenc",
            r"enum\s+\w*[Ee]rror",
            r"static\s+mut",
            r"\.unwrap\(\)|\.expect\(",
            "Err\\(\\s*\"",
            "Binds",
            "(?i)(password|api_key|secret_key|secret|token)\\s*(:\\s*&str)?\\s*=\\s*\"[^\"]{6,}\"",
            "(?i)(println!|log::(info|debug)!)[^\\n]*(card_number|\\bpan\\b)",
            "Idempotency-Key",
            r"^\s{0,3}#{1,6}\s+AD-(\d+)\b",
        ];
        for pattern in cases {
            let Some(specimen) = regex_specimen(pattern) else {
                panic!("образец не синтезирован для {pattern}");
            };
            let re = Regex::new(pattern).expect("шаблон");
            assert!(re.is_match(&specimen), "{pattern} vs {specimen:?}");
        }
    }

    /// Несинтезируемые шаблоны — честный None: несовпадаемый `x^` (якорь
    /// после литерала), многострочные `\n`, обратные ссылки и прокси-классы.
    #[test]
    fn specimen_declines_unsupported_or_unsatisfiable_patterns() {
        assert_eq!(regex_specimen("x^"), None, "x^ ничего не совпадает");
        assert_eq!(regex_specimen(r"a\nb"), None, "образец однострочный");
        assert_eq!(
            regex_specimen(r"\p{L}+"),
            None,
            "прокси-классы не поддержаны"
        );
    }
}
