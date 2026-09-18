//! Baseline-режим (ratchet) и срез изменённых файлов для `control check`
//! (бэклог «Baseline / ratchet для brownfield»; `docs/control.md`).
//!
//! Проблема brownfield: на репозитории с сотней исторических нарушений гейт
//! красный всегда — его отключают в первый день. Baseline фиксирует текущее
//! состояние как «исторический долг»: гейт падает только на НОВЫХ нарушениях,
//! а счётчик долга по каждому правилу может лишь убывать (ratchet — храповик):
//!
//! - `control check --baseline <path>`: находки из baseline — долг (в отчёте
//!   `baseline.debt`, гейт не ломают); новая error-находка — FAIL; рост
//!   счётчика error-находок правила против baseline — FAIL (страховка от
//!   коллизий отпечатков);
//! - `control check --baseline <path> --baseline-update`: перезаписывает
//!   baseline текущим состоянием, но ТОЛЬКО при неухудшении долга (рост —
//!   отказ с ошибкой, файл не трогается). Без `--baseline` путь по умолчанию —
//!   `<repo>/.arch-handoff/baseline.json`;
//! - `--changed-since <git-ref>`: проверяются только файлы, изменённые против
//!   рефа (`git diff --name-only <ref>` рабочего дерева + untracked).
//!   Файловые правила (`must_contain`/`must_not_contain`/
//!   `each_file_must_contain`) исполняются на подмножестве; глобальные
//!   (`file_exists`/`dir_must_have_file`/`max_age`/`command_succeeds`/
//!   `dependency_direction`/`context_boundary`/`archunit`/`deny_dependency`)
//!   пропускаются с пометкой в отчёте (`skipped`): на срезе они дают ложные
//!   срабатывания или неоправданно дороги. Полный прогон остаётся истиной:
//!   в режиме среза закрытие долга не отслеживается, а `--baseline-update`
//!   запрещён (срез уничтожил бы записи долга в нетронутых файлах).
//!
//! ## Стабильность отпечатков
//!
//! Номер строки в отпечаток НЕ входит: строки плывут при любых правках файла,
//! и baseline не должен ломаться от сдвига. Отпечаток находки — первые 16
//! hex-символов SHA-256 от `rule \n file \n normalize(message)`:
//!
//! - `file` — путь как в отчёте, разделители нормализованы в `/`;
//! - `normalize(message)` схлопывает пробельные последовательности (включая
//!   переводы строк — у `command_succeeds` многострочные хвосты) в один
//!   пробел и заменяет серии ASCII-цифр на `#`: числа в сниппетах (значения,
//!   id, счётчики) дрейфуют, не делая находку «новой».
//!
//! Страховка от оставшейся нечувствительности (две находки одного правила в
//! одном файле, различающиеся только числами, делят отпечаток) — счётчик
//! находок по правилу: рост против baseline — FAIL независимо от отпечатков.
//!
//! В baseline участвуют только error-находки правил `CONSTRAINTS.yaml`:
//! warn-находки гейт не ломают и долгом не считаются; находки механики
//! (`extends`, `override`) — это сломанная конфигурация губернанса, а не
//! кодовый долг, — в baseline не зашиваются и гейт ломают всегда.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::{Deserialize, Serialize};

use super::LintIssue;
use crate::error::{HarnessError, Result};

/// Версия формата baseline-файла (поле `version`; несовместимые изменения
/// формата её инкрементируют, читатель отвергает чужие версии).
pub const BASELINE_FORMAT_VERSION: u32 = 1;

/// Путь baseline-файла по умолчанию относительно корня репозитория —
/// используется флагом `--baseline-update` без явного `--baseline`.
pub const DEFAULT_BASELINE_PATH: &str = ".arch-handoff/baseline.json";

/// Потолок размера baseline-файла при чтении (8 МиБ): страховка от поданного
/// по ошибке гигантского/бинарного файла; реальный baseline на сотни находок
/// занимает десятки килобайт.
const MAX_BASELINE_FILE_BYTES: u64 = 8 * 1024 * 1024;

/// Потолок длины текста находки, хранимой в baseline (читаемость файла; на
/// отпечаток не влияет — он считается от полного текста находки).
const MAX_BASELINE_MESSAGE_LEN: usize = 240;

/// Опции прогона [`super::check_with_options`]: ratchet по baseline и/или
/// срез изменённых файлов. `Default` — классический полный прогон без
/// baseline (поведение [`super::check`]).
#[derive(Debug, Clone, Default)]
pub struct CheckOptions {
    /// Путь к baseline-файлу долга (JSON): режим ratchet.
    pub baseline: Option<PathBuf>,
    /// Перезаписать baseline текущим состоянием (принимается только при
    /// неухудшении долга; без `baseline` — [`DEFAULT_BASELINE_PATH`]).
    pub baseline_update: bool,
    /// Проверять только файлы, изменённые против этого git-рефа (+untracked).
    pub changed_since: Option<String>,
}

/// Baseline-файл долга: снимок error-находок `control check`, которые гейт
/// «прощает» как исторический долг brownfield-репозитория.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Baseline {
    /// Версия формата ([`BASELINE_FORMAT_VERSION`]).
    pub version: u32,
    /// Дата последнего обновления (`YYYY-MM-DD`).
    pub updated_at: String,
    /// Долг по правилам (только правила с непустым списком находок).
    pub rules: Vec<BaselineRule>,
}

/// Долг одного правила: счётчик + сами находки.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BaselineRule {
    /// Имя правила (`name` из `CONSTRAINTS.yaml`).
    pub name: String,
    /// Владелец правила на момент записи (карточка; информативно).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    /// Число находок — обязано совпадать с `findings.len()` (проверяется при
    /// чтении: baseline — артефакт гейта, ручная правка видна сразу).
    pub count: usize,
    /// Находки-долг.
    pub findings: Vec<BaselineFinding>,
}

/// Одна запись долга в baseline-файле.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BaselineFinding {
    /// Стабильный отпечаток находки (см. [`fingerprint`]).
    pub fingerprint: String,
    /// Файл (как в отчёте, разделители `/`).
    pub file: String,
    /// Строка на момент записи (информативно; в отпечаток НЕ входит).
    pub line: usize,
    /// Текст находки (обрезан до [`MAX_BASELINE_MESSAGE_LEN`]; информативно).
    pub message: String,
}

/// Запись baseline, закрытая в текущем прогоне (была в файле, находки нет).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClosedFinding {
    /// Имя правила.
    pub rule: String,
    /// Отпечаток закрытой находки.
    pub fingerprint: String,
    /// Файл.
    pub file: String,
    /// Строка на момент записи baseline.
    pub line: usize,
}

/// Долг одного правила в отчёте прогона (ratchet-режим).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuleDebt {
    /// Имя правила.
    pub rule: String,
    /// Владелец (из карточки правила; если карточка опустела — из baseline).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    /// Текущее число error-находок-долга правила.
    pub count: usize,
    /// Число находок правила, записанное в baseline.
    pub baseline_count: usize,
    /// Сами находки-долг.
    pub findings: Vec<BaselineFinding>,
}

/// Итог ratchet-сравнения прогона с baseline (аддитивное поле
/// `FitnessReport`, SDK-контракт v1).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BaselineReport {
    /// Путь к baseline-файлу.
    pub path: PathBuf,
    /// `true`, если в этом прогоне baseline перезаписан (`--baseline-update`).
    pub updated: bool,
    /// Текущий долг по правилам (error-находки, присутствующие в baseline).
    pub debt: Vec<RuleDebt>,
    /// Закрытые находки (были в baseline, в текущем прогоне отсутствуют).
    /// В режиме `--changed-since` всегда пусто: срез не показывает закрытие.
    pub closed: Vec<ClosedFinding>,
    /// Всего находок долга.
    pub debt_total: usize,
    /// Всего закрыто с прошлого baseline.
    pub closed_total: usize,
}

/// Правило, пропущенное в режиме `--changed-since` (аддитивное поле
/// `FitnessReport`, SDK-контракт v1).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkippedRule {
    /// Имя правила.
    pub rule: String,
    /// Причина пропуска (глобальное правило / пустой срез файлов).
    pub reason: String,
}

/// Правило, у которого счётчик error-находок вырос против baseline
/// (страховка от коллизий отпечатков — FAIL даже при совпавших отпечатках).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrownRule {
    /// Имя правила.
    pub rule: String,
    /// Число находок в baseline.
    pub was: usize,
    /// Число находок в текущем прогоне.
    pub now: usize,
}

/// Итог ratchet-разбора текущих error-находок против baseline.
#[derive(Debug, Default)]
pub struct Classification {
    /// Новые находки (отпечатка нет в baseline) — ломают гейт.
    pub new_issues: Vec<LintIssue>,
    /// Долг по правилам (находки, присутствующие в baseline).
    pub debt: Vec<RuleDebt>,
    /// Закрытые записи baseline (пусто при `track_closed = false`).
    pub closed: Vec<ClosedFinding>,
    /// Правила с выросшим счётчиком.
    pub grown: Vec<GrownRule>,
}

impl Baseline {
    /// Строит baseline из error-находок прогона `control check` (warn-находки
    /// гейт не ломают и в долг не записываются). Правила с пустым списком
    /// находок в файл не попадают; правила и находки сортируются —
    /// детерминированный файл для код-ревью.
    #[must_use]
    pub fn from_issues(issues: &[LintIssue], updated_at: &str) -> Self {
        let mut by_rule: BTreeMap<String, (Option<String>, Vec<BaselineFinding>)> = BTreeMap::new();
        for issue in issues.iter().filter(|i| i.severity == "error") {
            let entry = by_rule.entry(issue.rule.clone()).or_default();
            if entry.0.is_none() {
                entry.0.clone_from(&issue.owner);
            }
            let file = file_key(&issue.file);
            entry.1.push(BaselineFinding {
                fingerprint: fingerprint(&issue.rule, &file, &issue.message),
                file,
                line: issue.line,
                message: truncate_message(&issue.message),
            });
        }
        let rules = by_rule
            .into_iter()
            .map(|(name, (owner, mut findings))| {
                findings.sort_by(|a, b| {
                    a.file
                        .cmp(&b.file)
                        .then(a.line.cmp(&b.line))
                        .then(a.fingerprint.cmp(&b.fingerprint))
                });
                BaselineRule {
                    name,
                    owner,
                    count: findings.len(),
                    findings,
                }
            })
            .collect();
        Self {
            version: BASELINE_FORMAT_VERSION,
            updated_at: updated_at.to_string(),
            rules,
        }
    }

    /// Число находок правила в baseline (0 — правила нет в файле).
    #[must_use]
    pub fn rule_count(&self, name: &str) -> usize {
        self.rules
            .iter()
            .find(|r| r.name == name)
            .map_or(0, |r| r.count)
    }

    /// Записи `old`, которых нет в этом baseline (закрытый обновлением долг).
    #[must_use]
    pub fn closed_since(&self, old: &Baseline) -> Vec<ClosedFinding> {
        closed_findings(old, &self.fingerprint_set())
    }

    /// Множество отпечатков всех записей baseline.
    fn fingerprint_set(&self) -> BTreeSet<String> {
        self.rules
            .iter()
            .flat_map(|r| r.findings.iter().map(|f| f.fingerprint.clone()))
            .collect()
    }
}

impl RuleDebt {
    /// Долг из записи baseline (сразу после принятого обновления: текущее
    /// состояние совпадает с записанным, `count == baseline_count`).
    #[must_use]
    pub fn from_baseline_rule(rule: &BaselineRule) -> Self {
        Self {
            rule: rule.name.clone(),
            owner: rule.owner.clone(),
            count: rule.count,
            baseline_count: rule.count,
            findings: rule.findings.clone(),
        }
    }
}

/// Читает baseline-файл. Строго: неподдерживаемая версия формата, битый JSON
/// и расхождение `count` с числом записей — ошибки (baseline — артефакт
/// гейта, а не черновик).
///
/// # Errors
/// Файл не читается или больше лимита, JSON невалиден, версия формата не
/// поддержана, `count` правила не совпадает с числом его `findings`.
pub fn load(path: &Path) -> Result<Baseline> {
    let meta = std::fs::metadata(path).map_err(|e| HarnessError::io(path, e))?;
    if meta.len() > MAX_BASELINE_FILE_BYTES {
        return Err(HarnessError::Control(format!(
            "baseline: файл {} больше лимита {MAX_BASELINE_FILE_BYTES} байт — это не baseline?",
            path.display()
        )));
    }
    let text = std::fs::read_to_string(path).map_err(|e| HarnessError::io(path, e))?;
    let baseline: Baseline = serde_json::from_str(&text)?;
    if baseline.version != BASELINE_FORMAT_VERSION {
        return Err(HarnessError::Control(format!(
            "baseline: неподдерживаемая версия формата {} в {} (поддерживается {BASELINE_FORMAT_VERSION})",
            baseline.version,
            path.display()
        )));
    }
    for rule in &baseline.rules {
        if rule.count != rule.findings.len() {
            return Err(HarnessError::Control(format!(
                "baseline: у правила '{}' count = {}, а записей {} — файл повреждён или \
                 отредактирован вручную; пересоздайте его --baseline-update",
                rule.name,
                rule.count,
                rule.findings.len()
            )));
        }
    }
    Ok(baseline)
}

/// Пишет baseline-файл (pretty JSON с переводом строки в конце; родительский
/// каталог создаётся при отсутствии).
///
/// # Errors
/// Каталог не создаётся, файл не пишется.
pub fn save(path: &Path, baseline: &Baseline) -> Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|e| HarnessError::io(parent, e))?;
        }
    }
    let mut text = serde_json::to_string_pretty(baseline)?;
    text.push('\n');
    std::fs::write(path, text).map_err(|e| HarnessError::io(path, e))
}

/// Стабильный отпечаток находки: первые 16 hex-символов SHA-256 от
/// `rule \n file \n normalize(message)` (семантика — в документации модуля).
#[must_use]
pub fn fingerprint(rule: &str, file: &str, message: &str) -> String {
    let input = format!("{rule}\n{file}\n{}", normalize_message(message));
    crate::archunit::sha256_hex(input.as_bytes())
        .chars()
        .take(16)
        .collect()
}

/// Нормализация текста находки для отпечатка: пробельные последовательности
/// (включая переводы строк) схлопываются в один пробел, серии ASCII-цифр
/// заменяются на `#` (числа в сниппетах дрейфуют — значения, id, счётчики).
fn normalize_message(message: &str) -> String {
    let collapsed = message.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut out = String::with_capacity(collapsed.len());
    let mut in_digits = false;
    for ch in collapsed.chars() {
        if ch.is_ascii_digit() {
            if !in_digits {
                out.push('#');
            }
            in_digits = true;
        } else {
            in_digits = false;
            out.push(ch);
        }
    }
    out
}

/// Ключ файла для baseline: путь как в отчёте, разделители нормализованы в `/`.
fn file_key(file: &Path) -> String {
    file.to_string_lossy().replace('\\', "/")
}

/// Обрезает текст находки для хранения в baseline
/// ([`MAX_BASELINE_MESSAGE_LEN`], по границе символа, с маркером-многоточием).
fn truncate_message(message: &str) -> String {
    if message.chars().count() <= MAX_BASELINE_MESSAGE_LEN {
        return message.to_string();
    }
    let mut out: String = message.chars().take(MAX_BASELINE_MESSAGE_LEN).collect();
    out.push('…');
    out
}

/// Ratchet-разбор текущих error-находок правил против baseline: новые
/// (ломают гейт), долг (в отчёт), закрытые, правила с выросшим счётчиком.
///
/// `current` — только error-находки правил `CONSTRAINTS.yaml` (находки
/// механики отфильтровывает вызывающий). `track_closed = false` — в режиме
/// среза `--changed-since`: непроверенные файлы не считаются закрытым долгом.
#[must_use]
pub fn classify(current: &[LintIssue], baseline: &Baseline, track_closed: bool) -> Classification {
    let mut out = Classification::default();
    // Группировка текущих находок по правилам (порядок — по имени правила).
    let mut by_rule: BTreeMap<&str, Vec<&LintIssue>> = BTreeMap::new();
    for issue in current {
        by_rule.entry(issue.rule.as_str()).or_default().push(issue);
    }
    let mut current_fps: BTreeSet<String> = BTreeSet::new();
    for (name, issues) in &by_rule {
        let base_rule = baseline.rules.iter().find(|r| r.name == *name);
        let base_fps: BTreeSet<&str> = base_rule
            .map(|r| r.findings.iter().map(|f| f.fingerprint.as_str()).collect())
            .unwrap_or_default();
        let base_count = base_rule.map_or(0, |r| r.count);
        let mut debt_findings = Vec::new();
        for issue in issues {
            let file = file_key(&issue.file);
            let fp = fingerprint(name, &file, &issue.message);
            current_fps.insert(fp.clone());
            if base_fps.contains(fp.as_str()) {
                debt_findings.push(BaselineFinding {
                    fingerprint: fp,
                    file,
                    line: issue.line,
                    message: truncate_message(&issue.message),
                });
            } else {
                out.new_issues.push((*issue).clone());
            }
        }
        if issues.len() > base_count {
            out.grown.push(GrownRule {
                rule: (*name).to_string(),
                was: base_count,
                now: issues.len(),
            });
        }
        if !debt_findings.is_empty() {
            let owner = issues
                .iter()
                .find_map(|i| i.owner.clone())
                .or_else(|| base_rule.and_then(|r| r.owner.clone()));
            out.debt.push(RuleDebt {
                rule: (*name).to_string(),
                owner,
                count: debt_findings.len(),
                baseline_count: base_count,
                findings: debt_findings,
            });
        }
    }
    if track_closed {
        out.closed = closed_findings(baseline, &current_fps);
    }
    out
}

/// Записи baseline, которых нет среди текущих отпечатков (закрытый долг).
fn closed_findings(baseline: &Baseline, current_fps: &BTreeSet<String>) -> Vec<ClosedFinding> {
    let mut out = Vec::new();
    for rule in &baseline.rules {
        for finding in &rule.findings {
            if !current_fps.contains(&finding.fingerprint) {
                out.push(ClosedFinding {
                    rule: rule.name.clone(),
                    fingerprint: finding.fingerprint.clone(),
                    file: finding.file.clone(),
                    line: finding.line,
                });
            }
        }
    }
    out.sort_by(|a, b| {
        a.rule
            .cmp(&b.rule)
            .then(a.file.cmp(&b.file))
            .then(a.line.cmp(&b.line))
    });
    out
}

/// Гейт ratchet на обновление: новый baseline принимается, только если долг
/// не вырос НИ ПО ОДНОМУ правилу (новое правило с находками — рост от 0).
/// При отказе файл вызывающий не пишет.
///
/// # Errors
/// Долг вырос хотя бы по одному правилу (перечень — в тексте ошибки).
pub fn ensure_shrinks(old: Option<&Baseline>, new: &Baseline) -> Result<()> {
    let Some(old) = old else {
        return Ok(()); // первый baseline репозитория — эталон долга
    };
    let grew: Vec<String> = new
        .rules
        .iter()
        .filter_map(|rule| {
            let was = old.rule_count(&rule.name);
            (rule.count > was).then(|| format!("'{}': было {was}, стало {}", rule.name, rule.count))
        })
        .collect();
    if grew.is_empty() {
        return Ok(());
    }
    Err(HarnessError::Control(format!(
        "обновление baseline отклонено — долг вырос ({}); ratchet разрешает только убывание: \
         исправьте новые нарушения, а не зашивайте их в baseline",
        grew.join(", ")
    )))
}

/// Множество файлов, изменённых против git-рефа: `git diff --name-only
/// <ref>` (рабочее дерево против рефа — покрывает и закоммиченное после
/// рефа, и незакоммиченное) плюс untracked-файлы (`git ls-files --others
/// --exclude-standard`). Механика — как у [`super::detect_diff_triggers`].
///
/// # Errors
/// Не git-репозиторий, git недоступен, некорректный `<ref>`.
pub fn changed_files_since(repo: &Path, git_ref: &str) -> Result<BTreeSet<String>> {
    let diff = git_stdout(repo, &["diff", "--name-only", git_ref], git_ref)?;
    let mut files: BTreeSet<String> = diff
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect();
    let untracked = git_stdout(
        repo,
        &["ls-files", "--others", "--exclude-standard"],
        git_ref,
    )?;
    files.extend(
        untracked
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(str::to_string),
    );
    Ok(files)
}

/// stdout git-команды в репозитории; ошибка — с контекстом `--changed-since`.
fn git_stdout(repo: &Path, args: &[&str], git_ref: &str) -> Result<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| {
            HarnessError::Control(format!("--changed-since: не удалось запустить git ({e})"))
        })?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let detail = stderr.trim().chars().take(200).collect::<String>();
        return Err(HarnessError::Control(format!(
            "--changed-since: {} не git-репозиторий или некорректный реф '{git_ref}' ({detail})",
            repo.display()
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Текстовая секция baseline для вывода `control check`: долг по правилам и
/// владельцам + закрытые находки.
#[must_use]
pub fn render_baseline_section(report: &BaselineReport) -> String {
    let mut out = String::new();
    if report.updated {
        let _ = writeln!(
            out,
            "Baseline обновлён: {} — долг зафиксирован: {} находок",
            report.path.display(),
            report.debt_total
        );
    } else {
        let _ = writeln!(out, "Baseline (ratchet): {}", report.path.display());
    }
    if report.debt.is_empty() {
        let _ = writeln!(out, "Долг: нет");
    }
    for debt in &report.debt {
        let owner = debt
            .owner
            .as_deref()
            .map(|o| format!(" (owner: {o})"))
            .unwrap_or_default();
        let drift = if debt.count == debt.baseline_count {
            String::new()
        } else {
            format!(", было {}", debt.baseline_count)
        };
        let _ = writeln!(
            out,
            "  долг: {} — {} находок{}{drift}",
            debt.rule, debt.count, owner
        );
    }
    if !report.closed.is_empty() {
        let _ = writeln!(out, "Закрыто с прошлого baseline: {}", report.closed_total);
        for closed in &report.closed {
            let _ = writeln!(
                out,
                "  закрыто: {} {}:{}",
                closed.rule, closed.file, closed.line
            );
        }
    }
    out
}

/// Текстовая секция режима `--changed-since`: размер среза и пропущенные
/// правила с причинами.
#[must_use]
pub fn render_scope_section(
    changed_since: &str,
    files_count: usize,
    skipped: &[SkippedRule],
) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "Срез --changed-since {changed_since}: изменённых файлов {files_count} \
         (полный прогон остаётся истиной гейта)"
    );
    if !skipped.is_empty() {
        let _ = writeln!(out, "Пропущены правила ({}):", skipped.len());
        for skipped_rule in skipped {
            let _ = writeln!(
                out,
                "  skip: {} — {}",
                skipped_rule.rule, skipped_rule.reason
            );
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Находка уровня error для тестов ratchet.
    fn issue(rule: &str, file: &str, line: usize, message: &str) -> LintIssue {
        LintIssue {
            file: PathBuf::from(file),
            line,
            rule: rule.to_string(),
            message: message.to_string(),
            severity: "error".to_string(),
            ..LintIssue::default()
        }
    }

    /// git в каталоге с тестовой идентичностью коммиттера (образец —
    /// `crate::delta` тестовый `make_guard_repo`).
    fn git(dir: &Path, args: &[&str]) {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .output()
            .expect("git");
        assert!(
            out.status.success(),
            "git {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr)
        );
    }

    #[test]
    fn fingerprint_stable_against_line_and_digit_drift() {
        let base = fingerprint("no_pan", "src/a.py", "must_not_contain: x = \"123\"");
        // Номер строки в отпечаток не входит (он параметр вызывающего, не
        // хэша) — сдвиг строк не делает находку новой.
        let digits = fingerprint("no_pan", "src/a.py", "must_not_contain: x = \"999\"");
        assert_eq!(base, digits, "дрейф чисел нормализуется");
        let whitespace = fingerprint("no_pan", "src/a.py", "must_not_contain:  x\n=   \"123\"");
        assert_eq!(base, whitespace, "пробелы/переводы строк схлопываются");
        assert_ne!(
            base,
            fingerprint("no_pan", "src/b.py", "must_not_contain: x = \"123\""),
            "другой файл — другой отпечаток"
        );
        assert_ne!(
            base,
            fingerprint("other_rule", "src/a.py", "must_not_contain: x = \"123\""),
            "другое правило — другой отпечаток"
        );
        assert_eq!(base.len(), 16, "отпечаток — 16 hex-символов");
    }

    #[test]
    fn from_issues_keeps_only_errors_and_counts() {
        let mut err = issue("no_pan", "src/a.py", 3, "m1");
        err.owner = Some("владелец".to_string());
        let warn = LintIssue {
            severity: "warn".to_string(),
            ..issue("no_pan", "src/b.py", 1, "m2")
        };
        let baseline = Baseline::from_issues(&[warn, err], "2026-09-18");
        assert_eq!(baseline.version, BASELINE_FORMAT_VERSION);
        assert_eq!(baseline.updated_at, "2026-09-18");
        assert_eq!(baseline.rules.len(), 1, "warn в baseline не входит");
        let rule = &baseline.rules[0];
        assert_eq!(rule.name, "no_pan");
        assert_eq!(rule.owner.as_deref(), Some("владелец"));
        assert_eq!(rule.count, 1);
        assert_eq!(rule.count, rule.findings.len());
        assert_eq!(rule.findings[0].file, "src/a.py");
        assert_eq!(rule.findings[0].line, 3);
    }

    #[test]
    fn save_load_roundtrip_and_strict_validation() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("sub/dir/baseline.json");
        let baseline = Baseline::from_issues(&[issue("no_pan", "src/a.py", 1, "m")], "2026-09-18");
        save(&path, &baseline).expect("запись (каталоги создаются)");
        let loaded = load(&path).expect("чтение");
        assert_eq!(loaded.rules.len(), 1);
        assert_eq!(loaded.rules[0].findings, baseline.rules[0].findings);

        // Ручная правка count — ошибка (baseline — артефакт гейта).
        let text = std::fs::read_to_string(&path).expect("read");
        std::fs::write(&path, text.replace("\"count\": 1", "\"count\": 5")).expect("write");
        let err = load(&path).expect_err("count ≠ findings — отказ");
        assert!(err.to_string().contains("повреждён"), "{err}");

        // Чужая версия формата — ошибка.
        std::fs::write(
            &path,
            r#"{"version": 99, "updated_at": "2026-09-18", "rules": []}"#,
        )
        .expect("write");
        let err = load(&path).expect_err("версия 99 — отказ");
        assert!(err.to_string().contains("версия"), "{err}");

        // Отсутствующий файл — ошибка чтения, не паника.
        assert!(load(&tmp.path().join("missing.json")).is_err());
    }

    #[test]
    fn classify_splits_new_debt_closed_and_growth() {
        let old_issues = [
            issue("no_pan", "src/a.py", 1, "pan = \"4276550012345678\""),
            issue("no_pan", "src/b.py", 2, "pan = \"4276550099990001\""),
        ];
        let baseline = Baseline::from_issues(&old_issues, "2026-09-18");

        // Те же находки (строки сдвинуты) + одна новая: долг 2, новая 1,
        // счётчик вырос 2 → 3.
        let current = vec![
            issue("no_pan", "src/a.py", 40, "pan = \"4276550012345678\""),
            issue("no_pan", "src/b.py", 50, "pan = \"4276550099990001\""),
            issue("no_pan", "src/c.py", 1, "pan = \"4276550011112222\""),
        ];
        let c = classify(&current, &baseline, true);
        assert_eq!(c.debt.len(), 1);
        assert_eq!(c.debt[0].count, 2);
        assert_eq!(c.debt[0].baseline_count, 2);
        assert_eq!(c.new_issues.len(), 1);
        assert_eq!(c.new_issues[0].file, PathBuf::from("src/c.py"));
        assert!(c.closed.is_empty());
        assert_eq!(
            c.grown,
            vec![GrownRule {
                rule: "no_pan".to_string(),
                was: 2,
                now: 3
            }]
        );

        // Все нарушения исправлены: долга нет, обе записи закрыты.
        let c = classify(&[], &baseline, true);
        assert!(c.debt.is_empty() && c.new_issues.is_empty() && c.grown.is_empty());
        assert_eq!(c.closed.len(), 2);

        // Без отслеживания закрытия (режим среза) закрытые не считаются.
        let c = classify(&[], &baseline, false);
        assert!(c.closed.is_empty());
    }

    #[test]
    fn ensure_shrinks_allows_only_decrease() {
        let old = Baseline::from_issues(
            &[
                issue("no_pan", "src/a.py", 1, "m"),
                issue("no_pan", "src/b.py", 2, "m"),
            ],
            "2026-09-18",
        );
        // Первый baseline — всегда принимается.
        ensure_shrinks(None, &old).expect("первый baseline");
        // Тот же долг — принимается.
        ensure_shrinks(Some(&old), &old.clone()).expect("без изменений");
        // Убывание — принимается.
        let smaller = Baseline::from_issues(&[issue("no_pan", "src/a.py", 1, "m")], "2026-09-18");
        ensure_shrinks(Some(&old), &smaller).expect("убывание");
        // Рост — отказ с перечнем.
        let bigger = Baseline::from_issues(
            &[
                issue("no_pan", "src/a.py", 1, "m"),
                issue("no_pan", "src/b.py", 2, "m"),
                issue("no_pan", "src/c.py", 3, "m"),
            ],
            "2026-09-18",
        );
        let err = ensure_shrinks(Some(&old), &bigger).expect_err("рост — отказ");
        assert!(err.to_string().contains("отклонено"), "{err}");
        assert!(err.to_string().contains("no_pan"), "{err}");
        // Новое правило с находками — тоже рост (от нуля).
        let new_rule = Baseline::from_issues(&[issue("dbg", "src/x.py", 1, "m")], "2026-09-18");
        assert!(ensure_shrinks(Some(&old), &new_rule).is_err());
    }

    #[test]
    fn changed_files_since_tracks_modified_and_untracked() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).expect("mkdir");
        git(&repo, &["init", "-q"]);
        std::fs::write(repo.join("a.rs"), "fn a() {}\n").expect("a.rs");
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-q", "-m", "init"]);

        // Чистое дерево — пустой срез.
        let files = changed_files_since(&repo, "HEAD").expect("diff");
        assert!(files.is_empty(), "{files:?}");

        // Правка закоммиченного + untracked-файл попадают в срез.
        std::fs::write(repo.join("a.rs"), "fn a() { todo!() }\n").expect("edit");
        std::fs::write(repo.join("b.rs"), "fn b() {}\n").expect("b.rs");
        let files = changed_files_since(&repo, "HEAD").expect("diff");
        assert_eq!(
            files,
            BTreeSet::from(["a.rs".to_string(), "b.rs".to_string()])
        );

        // Не git-репозиторий — понятная ошибка.
        let err = changed_files_since(tmp.path(), "HEAD").expect_err("не git");
        assert!(err.to_string().contains("--changed-since"), "{err}");
    }
}
