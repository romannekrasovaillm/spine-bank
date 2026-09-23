//! Реестр ограничений как наследуемая конфигурация (`docs/corp-spine.md`):
//! резолв `extends` с пинами версий ([`load_constraints_resolved`]), overrides
//! через ADR, анти-ослабление состава правил ([`rule_weakened`], П5) и сверка
//! с git-базой ([`rule_anchor`], [`check_anchored`]).

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::Serialize;

use super::baseline;
use super::exec::check_with_options;
use super::rules::{ConstraintsFile, ConstraintsFileShadow, parse_constraints_file};
use super::types::{
    FitnessReport, FitnessRule, LintIssue, OverrideEntry, OverrideInfo, SkippedUnknownRule,
    normalize_severity,
};
use crate::error::{HarnessError, Result};

/// Переменная окружения с каталогом-реестром родительских
/// constraint-файлов (резолв непутевых ref в `extends`; без сети,
/// `docs/corp-spine.md`).
pub const CONSTRAINTS_REGISTRY_ENV: &str = "ARCH_CONSTRAINTS_REGISTRY";

/// Разрешённый родитель из `extends` (метка источника для вывода).
#[derive(Debug, Clone, Serialize)]
pub struct ResolvedParent {
    /// Запись как в `extends` (`<ref>@<pin>`).
    pub reference: String,
    /// Разрешённый путь к файлу.
    pub path: PathBuf,
    /// Запиненная версия из записи.
    pub pinned: String,
    /// Фактическая версия из поля `version` родителя (`None` — не задана).
    pub actual: Option<String>,
    /// Число унаследованных из этого файла правил (включая транзитивные).
    pub rules: usize,
}

/// Результат полного резолва constraint-файла: правила всех уровней с
/// метками источника, родители, overrides и находки резолва (расхождение
/// пина версии — error-находка, `docs/corp-spine.md`).
#[derive(Debug)]
pub struct ResolvedConstraints {
    /// Слитые правила: родительские (по порядку `extends`, транзитивно)
    /// первыми, затем собственные; собственное правило с именем
    /// унаследованного замещает его (shadowing).
    pub rules: Vec<FitnessRule>,
    /// Непосредственные родители (для отчётности).
    pub parents: Vec<ResolvedParent>,
    /// Overrides всех уровней (родительские + собственные).
    pub overrides: Vec<OverrideEntry>,
    /// Находки резолва (несовпадение пина версии, родитель без `version`,
    /// warn-пропуски правил с неизвестным типом — E8).
    pub findings: Vec<LintIssue>,
    /// Правила, пропущенные из-за неизвестного типа (E8; собственные и
    /// унаследованные — по всем уровням `extends`).
    pub skipped_unknown: Vec<SkippedUnknownRule>,
}

/// Разбирает запись `extends` на (ref, пин версии): разделитель — последний
/// `@` (в путях `@` практически не встречается, в версиях — тем более).
fn parse_extends_ref(entry: &str) -> Result<(&str, &str)> {
    entry
        .rsplit_once('@')
        .filter(|(r, p)| !r.is_empty() && !p.is_empty())
        .ok_or_else(|| {
            HarnessError::Control(format!(
                "extends: запись '{entry}' не вида <ref>@<version> — пин версии обязателен"
            ))
        })
}

/// Признак «ref похож на путь» для `extends`: содержит `/`, начинается с
/// `.` или имеет yaml-расширение (регистр не важен).
fn extends_ref_looks_like_path(reference: &str) -> bool {
    reference.contains('/')
        || reference.starts_with('.')
        || Path::new(reference)
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("yaml") || ext.eq_ignore_ascii_case("yml"))
}

/// Разрешает ref из `extends` в путь к файлу (без сети,
/// `docs/corp-spine.md`): ref с `/`, ведущей `.` или расширением
/// `.yaml`/`.yml` — путь относительно каталога текущего файла; «голое» имя
/// — поиск `<ref>.yaml`/`<ref>.yml` в реестре: каталог из
/// [`CONSTRAINTS_REGISTRY_ENV`], иначе `constraints.d/` рядом с текущим
/// файлом.
fn resolve_extends_path(reference: &str, current_file: &Path) -> Result<PathBuf> {
    let base_dir = current_file.parent().unwrap_or_else(|| Path::new("."));
    if extends_ref_looks_like_path(reference) {
        let path = base_dir.join(reference);
        return path.is_file().then_some(path).ok_or_else(|| {
            HarnessError::Control(format!(
                "extends: родительский файл не найден: {reference} (от {})",
                current_file.display()
            ))
        });
    }
    let mut roots = Vec::new();
    if let Ok(registry) = std::env::var(CONSTRAINTS_REGISTRY_ENV) {
        roots.push(PathBuf::from(registry));
    }
    roots.push(base_dir.join("constraints.d"));
    for root in &roots {
        for ext in ["yaml", "yml"] {
            let candidate = root.join(format!("{reference}.{ext}"));
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
    }
    Err(HarnessError::Control(format!(
        "extends: '{reference}' не найден в реестре ({} или {}/constraints.d)",
        CONSTRAINTS_REGISTRY_ENV,
        base_dir.display()
    )))
}

/// Метка источника для правил родителя: `<имя>@<версия>`, где имя — ref как
/// записан (для путевого ref — имя файла без расширения), версия —
/// фактическая из родителя (иначе пин, иначе `?`).
fn source_label(reference: &str, path: &Path, pinned: &str, actual: Option<&str>) -> String {
    let name = if extends_ref_looks_like_path(reference) {
        path.file_stem().map_or_else(
            || reference.to_string(),
            |s| s.to_string_lossy().to_string(),
        )
    } else {
        reference.to_string()
    };
    format!("{name}@{}", actual.unwrap_or(pinned))
}

/// Рекурсивный резолв constraint-файла; `stack` — цепочка канонических
/// путей от корневого файла (детектор циклов `extends`).
fn resolve_constraints(path: &Path, stack: &mut Vec<PathBuf>) -> Result<ResolvedConstraints> {
    let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    if stack.contains(&canonical) {
        let mut cycle: Vec<String> = stack.iter().map(|p| p.display().to_string()).collect();
        cycle.push(canonical.display().to_string());
        return Err(HarnessError::Control(format!(
            "extends: цикл наследования: {}",
            cycle.join(" → ")
        )));
    }
    stack.push(canonical);

    let yaml = std::fs::read_to_string(path).map_err(|e| HarnessError::io(path, e))?;
    let (parsed, skipped) = parse_constraints_file(&yaml, path)?;

    let mut rules = Vec::new();
    let mut parents = Vec::new();
    let mut overrides = Vec::new();
    let mut findings = Vec::new();
    let mut skipped_unknown = Vec::new();

    // Неизвестный тип правила (словарь другой редакции, E8) — громкая
    // warn-находка, а не падение резолва и не тихий пропуск.
    for s in skipped {
        findings.push(LintIssue {
            file: path.to_path_buf(),
            line: 0,
            rule: "unknown_rule_type".to_string(),
            message: format!(
                "правило '{}': неизвестный тип '{}' — пропущено (словарь другой редакции?)",
                s.name, s.rule_type
            ),
            severity: "warn".to_string(),
            ..LintIssue::default()
        });
        skipped_unknown.push(s);
    }

    for entry in &parsed.extends {
        let (reference, pinned) = parse_extends_ref(entry)?;
        let parent_path = resolve_extends_path(reference, path)?;
        let parent = resolve_constraints(&parent_path, stack)?;
        // Пин версии — «PR с diff»: расхождение — error-находка, а не
        // молчаливая поломка и не тихий пропуск обновления родителя.
        let actual = parent_actual_version(&parent_path)?;
        match &actual {
            Some(actual) if actual != pinned => {
                findings.push(LintIssue {
                    file: path.to_path_buf(),
                    line: 0,
                    rule: "extends".to_string(),
                    message: format!(
                        "extends: родитель обновился: {reference}@{pinned} → {actual} — перепиновать осознанно"
                    ),
                    severity: "error".to_string(),
                    ..LintIssue::default()
                });
            }
            Some(_) => {}
            None => {
                findings.push(LintIssue {
                    file: path.to_path_buf(),
                    line: 0,
                    rule: "extends".to_string(),
                    message: format!(
                        "extends: у родителя {reference} нет поля version — пин {pinned} проверить нельзя"
                    ),
                    severity: "error".to_string(),
                    ..LintIssue::default()
                });
            }
        }
        let label = source_label(reference, &parent_path, pinned, actual.as_deref());
        let mut inherited = parent.rules;
        let inherited_count = inherited.len();
        for rule in &mut inherited {
            if rule.source.is_none() {
                rule.source = Some(label.clone());
            }
        }
        parents.push(ResolvedParent {
            reference: entry.clone(),
            path: parent_path,
            pinned: pinned.to_string(),
            actual,
            rules: inherited_count,
        });
        rules.extend(inherited);
        overrides.extend(parent.overrides);
        findings.extend(parent.findings);
        skipped_unknown.extend(parent.skipped_unknown);
    }

    let own_rules: Vec<FitnessRule> = parsed.rules.into_iter().chain(parsed.constraints).collect();
    // Shadowing: собственное правило с тем же именем замещает унаследованное.
    let own_names: BTreeSet<&str> = own_rules.iter().map(|r| r.name.as_str()).collect();
    rules.retain(|r| !own_names.contains(r.name.as_str()));
    rules.extend(own_rules);
    overrides.extend(parsed.overrides);

    stack.pop();
    Ok(ResolvedConstraints {
        rules,
        parents,
        overrides,
        findings,
        skipped_unknown,
    })
}

/// Версия constraint-файла (поле верхнего уровня `version`) — читается
/// родителем дочернего файла при проверке пина. Разбор — тенью (E8):
/// неизвестные типы правил родителя проверке пина не мешают.
fn parent_actual_version(path: &Path) -> Result<Option<String>> {
    let yaml = std::fs::read_to_string(path).map_err(|e| HarnessError::io(path, e))?;
    let parsed: ConstraintsFileShadow = serde_yaml_ng::from_str(&yaml)?;
    Ok(parsed.version)
}

/// Полный резолв `CONSTRAINTS.yaml`: наследование `extends` (транзитивно,
/// с метками источника и проверкой пинов версий), слияние overrides.
///
/// # Errors
/// Файл не читается/невалиден, родитель из `extends` не найден, цикл
/// наследования, запись `extends` без пина версии.
pub fn load_constraints_resolved(constraints: &Path) -> Result<ResolvedConstraints> {
    resolve_constraints(constraints, &mut Vec::new())
}

/// Разбирает срок override: `YYYY-MM-DD` либо `YYYY-MM` (год-месяц; срок
/// действует по указанный месяц включительно). Возвращает (год, месяц,
/// день); день 0 — форма YYYY-MM.
fn parse_until(raw: &str) -> Option<(i32, u32, u32)> {
    use chrono::Datelike as _;
    let raw = raw.trim();
    if let Ok(date) = chrono::NaiveDate::parse_from_str(raw, "%Y-%m-%d") {
        return Some((date.year(), date.month(), date.day()));
    }
    let (y, m) = raw.split_once('-')?;
    if m.len() != 2 || y.len() != 4 {
        return None;
    }
    Some((y.parse().ok()?, m.parse().ok()?, 0)).filter(|(y, m, _)| {
        *m >= 1 && *m <= 12 && chrono::NaiveDate::from_ymd_opt(*y, *m, 1).is_some()
    })
}

/// Истёк ли срок override на сегодня (для YYYY-MM — по месяцу: истёк, когда
/// текущий месяц ПОЗЖЕ указанного).
fn until_expired(y: i32, m: u32, d: u32) -> bool {
    use chrono::Datelike as _;
    let today = chrono::Local::now().date_naive();
    if d > 0 {
        chrono::NaiveDate::from_ymd_opt(y, m, d).is_some_and(|date| date < today)
    } else {
        (i64::from(y) * 12 + i64::from(m))
            < (i64::from(today.year()) * 12 + i64::from(today.month()))
    }
}

/// Срок правила `expiry` уже прошёл (та же логика дат, что у overrides:
/// `YYYY-MM-DD` либо `YYYY-MM` — срок действует по указанный месяц
/// включительно). Неразбираемая дата — «не просрочен»: за формат отвечает
/// отдельная находка `check`.
#[must_use]
pub fn expiry_is_past(raw: &str) -> bool {
    parse_until(raw.trim()).is_some_and(|(y, m, d)| until_expired(y, m, d))
}

/// Оценивает overrides против итогового набора правил: статусы для отчёта,
/// находки (неполный override — error; просроченный или на несуществующее
/// правило — warn) и множество отключённых активными overrides имён правил.
pub(super) fn evaluate_overrides(
    overrides: &[OverrideEntry],
    rules: &[FitnessRule],
    file: &Path,
) -> (Vec<OverrideInfo>, Vec<LintIssue>, BTreeSet<String>) {
    let mut infos = Vec::new();
    let mut findings = Vec::new();
    let mut disabled = BTreeSet::new();
    let finding = |severity: &str, message: String| LintIssue {
        file: file.to_path_buf(),
        line: 0,
        rule: "override".to_string(),
        message,
        severity: severity.to_string(),
        ..LintIssue::default()
    };
    for entry in overrides {
        let rule = entry.rule.as_deref().unwrap_or("").trim();
        let adr = entry.adr.as_deref().unwrap_or("").trim();
        let until = entry.until.as_deref().unwrap_or("").trim();
        let mut info = OverrideInfo {
            rule: rule.to_string(),
            adr: adr.to_string(),
            until: until.to_string(),
            status: "invalid".to_string(),
            note: String::new(),
        };
        // Гейт «override только через ADR»: все три поля обязательны.
        let missing: Vec<&str> = [
            rule.is_empty().then_some("rule"),
            adr.is_empty().then_some("adr"),
            until.is_empty().then_some("until"),
        ]
        .into_iter()
        .flatten()
        .collect();
        if !missing.is_empty() {
            info.note = format!("неполный override (нет {})", missing.join(", "));
            findings.push(finding(
                "error",
                format!(
                    "override: неполная запись для '{rule}' — требуются rule+adr+until ({})",
                    info.note
                ),
            ));
            infos.push(info);
            continue;
        }
        let Some((y, m, d)) = parse_until(until) else {
            info.note = format!("некорректный until '{until}' (ожидается YYYY-MM или YYYY-MM-DD)");
            findings.push(finding("error", format!("override {rule}: {}", info.note)));
            infos.push(info);
            continue;
        };
        let targets: Vec<&FitnessRule> = rules
            .iter()
            .filter(|r| r.name == rule || r.id.as_deref() == Some(rule))
            .collect();
        if targets.is_empty() {
            info.note = format!("правило '{rule}' не найдено (ни id, ни name)");
            findings.push(finding(
                "warn",
                format!("override на несуществующее правило '{rule}' (adr {adr}) — игнорируется"),
            ));
            infos.push(info);
            continue;
        }
        if until_expired(y, m, d) {
            info.status = "expired".to_string();
            info.note = format!("override истёк {until} — правило снова действует");
            findings.push(finding(
                "warn",
                format!("override '{rule}' (adr {adr}) истёк {until} — правило снова действует"),
            ));
            infos.push(info);
            continue;
        }
        info.status = "active".to_string();
        info.note = format!("правило отключено до {until} (adr {adr})");
        for target in targets {
            disabled.insert(target.name.clone());
        }
        infos.push(info);
    }
    (infos, findings, disabled)
}

/// Итог сверки состава правил с git-базой (П5).
#[derive(Debug, Clone)]
pub struct RuleAnchor {
    /// База сверки (ревизия git).
    pub base: Option<String>,
    /// Сверка выполнена.
    pub checked: bool,
    /// Причина, если сверка не выполнена (честная строка, а не молчание).
    pub note: Option<String>,
    /// Находки `rule_weakened`.
    pub issues: Vec<LintIssue>,
}

/// База сверки по умолчанию: точка ответвления от основной ветки
/// (`merge-base HEAD origin/main|main|origin/master|master`), иначе `HEAD`.
/// `None` — git-база недоступна.
#[must_use]
pub fn default_anchor_base(repo: &Path) -> Option<String> {
    for branch in ["origin/main", "main", "origin/master", "master"] {
        let out = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(["merge-base", "HEAD", branch])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output();
        if let Ok(out) = out {
            if out.status.success() {
                let rev = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if !rev.is_empty() {
                    return Some(rev);
                }
            }
        }
    }
    let head_ok = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["rev-parse", "--verify", "--quiet", "HEAD"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success());
    head_ok.then(|| "HEAD".to_string())
}

/// Корень git-репозитория для `repo` (`git rev-parse --show-toplevel`),
/// канонизированный. `None` — git недоступен или каталог вне репозитория.
fn git_toplevel(repo: &Path) -> Option<PathBuf> {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["rev-parse", "--show-toplevel"])
        .stdin(Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8(out.stdout).ok()?;
    PathBuf::from(text.trim()).canonicalize().ok()
}

/// Сверка состава правил с git-базой (П5, Д4): анти-ослабление доступно не
/// только составному гейту. Никогда не падает: недоступность базы — честный
/// `note`, а не молчаливый PASS.
#[must_use]
pub fn rule_anchor(repo: &Path, constraints: &Path, base: Option<&str>) -> RuleAnchor {
    let Some(rev) = base
        .map(str::to_string)
        .or_else(|| default_anchor_base(repo))
    else {
        return RuleAnchor {
            base: None,
            checked: false,
            note: Some("git-база недоступна — сверка состава правил не выполнена".into()),
            issues: Vec::new(),
        };
    };
    if !constraints.is_file() {
        return RuleAnchor {
            base: Some(rev),
            checked: false,
            note: Some("нет файла ограничений — сверять нечего".into()),
            issues: Vec::new(),
        };
    }
    let (Ok(abs_repo), Ok(abs_constraints)) = (repo.canonicalize(), constraints.canonicalize())
    else {
        return RuleAnchor {
            base: Some(rev),
            checked: false,
            note: Some("путь репозитория/реестра не канонизируется".into()),
            issues: Vec::new(),
        };
    };
    let Ok(rel_to_repo) = abs_constraints.strip_prefix(&abs_repo) else {
        return RuleAnchor {
            base: Some(rev),
            checked: false,
            note: Some("реестр вне репозитория — сравнение невозможно".into()),
            issues: Vec::new(),
        };
    };
    // `git show <rev>:<path>` считает путь от КОРНЯ git-репозитория, а не от
    // переданного каталога. Для реестра в подкаталоге (`arch-be control check
    // кейсы/x`) путь от каталога указывал бы на корневой CONSTRAINTS.yaml, и
    // каждое правило корня выглядело бы «удалённым» — ложный `rule_weakened`
    // на ровном месте. Поэтому путь достраивается до корня репозитория.
    let rel = match git_toplevel(abs_repo.as_path()) {
        Some(top) => match abs_constraints.strip_prefix(&top) {
            Ok(r) => r.to_path_buf(),
            Err(_) => rel_to_repo.to_path_buf(),
        },
        None => rel_to_repo.to_path_buf(),
    };
    let git_rel = rel.to_string_lossy().replace('\\', "/");
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .arg("show")
        .arg(format!("{rev}:{git_rel}"))
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output();
    match out {
        Ok(out) if out.status.success() => {
            let base_src = String::from_utf8_lossy(&out.stdout).into_owned();
            let current_src = match std::fs::read_to_string(constraints) {
                Ok(text) => text,
                Err(e) => {
                    return RuleAnchor {
                        base: Some(rev),
                        checked: false,
                        note: Some(format!("сбой чтения реестра: {e}")),
                        issues: Vec::new(),
                    };
                }
            };
            match rule_weakened(&current_src, &base_src, constraints) {
                Ok(issues) => RuleAnchor {
                    base: Some(rev),
                    checked: true,
                    note: None,
                    issues,
                },
                Err(e) => RuleAnchor {
                    base: Some(rev),
                    checked: false,
                    note: Some(format!("сбой сравнения: {e}")),
                    issues: Vec::new(),
                },
            }
        }
        _ => {
            let note = format!("в базе '{rev}' файла {git_rel} нет — новый реестр");
            RuleAnchor {
                base: Some(rev),
                checked: false,
                note: Some(note),
                issues: Vec::new(),
            }
        }
    }
}

/// [`check_with_options`] + сверка состава правил с git-базой (П5): «правило
/// выполняется» дополняется вопросом «правило ещё существует». Вызывается
/// каналами `control check` и MCP `fitness_check`.
///
/// # Errors
/// Те же, что у [`check_with_options`].
pub fn check_anchored(
    repo: &Path,
    constraints: &Path,
    options: &baseline::CheckOptions,
    base: Option<&str>,
) -> Result<FitnessReport> {
    let mut report = check_with_options(repo, constraints, options)?;
    let anchor = rule_anchor(repo, constraints, base);
    if anchor.checked {
        if anchor.issues.is_empty() {
            let _ = write!(
                report.summary,
                "; сверка состава правил с {}: ослаблений нет",
                anchor.base.as_deref().unwrap_or("?")
            );
        } else {
            let base = anchor.base.clone().unwrap_or_else(|| "?".into());
            let count = anchor.issues.len();
            report.issues.extend(anchor.issues);
            let _ = write!(
                report.summary,
                "; сверка состава правил с {base}: ослаблений {count}"
            );
            // Итог пересчитывается по полному набору находок.
            report.passed = !report
                .issues
                .iter()
                .any(|i| i.severity.eq_ignore_ascii_case("error"));
        }
    } else if let Some(note) = anchor.note {
        let _ = write!(
            report.summary,
            "; сверка состава правил не выполнена: {note}"
        );
    }
    Ok(report)
}

// --- Анти-ослабление гейта (находка `rule_weakened`, `arch-be gate`) -------

/// Тип ослабления правила — расшифровка находки `rule_weakened`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WeakenedKind {
    /// Правило удалено из реестра (было в базовой версии, нет в текущей).
    Removed,
    /// У правила появился или расширился `exclude_glob` (новые исключения
    /// из набора проверяемых файлов).
    ExcludeWidened,
    /// Severity понижен (нормализованное error → warn; сами нормализованные
    /// значения сравниваются — `critical`/`high`/`block` ≡ error).
    SeverityLowered,
}

impl WeakenedKind {
    /// Русская метка для текста находки.
    fn label(self) -> &'static str {
        match self {
            Self::Removed => "удалено из реестра",
            Self::ExcludeWidened => "появился/расширился exclude_glob",
            Self::SeverityLowered => "severity понижен",
        }
    }
}

/// Имена/идентификаторы правил с АКТИВНЫМИ overrides (`rule`+`adr`+`until`,
/// дата корректна и не просрочена — та же логика, что у
/// [`evaluate_overrides`]). Активный override узаконивает ослабление своего
/// правила: находка `rule_weakened` по нему подавляется.
fn active_override_keys(overrides: &[OverrideEntry]) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for entry in overrides {
        let (Some(rule), Some(adr), Some(until)) = (&entry.rule, &entry.adr, &entry.until) else {
            continue; // неполный override — не активен (отдельная error-находка `check`)
        };
        let rule = rule.trim();
        if rule.is_empty() || adr.trim().is_empty() {
            continue;
        }
        let Some((y, m, d)) = parse_until(until.trim()) else {
            continue; // некорректная дата — override не активен
        };
        if until_expired(y, m, d) {
            continue;
        }
        out.insert(rule.to_string());
    }
    out
}

/// Снимок правила для сравнения версий: `exclude_glob` и нормализованный
/// severity (id — для матчинга overrides, которые могут ссылаться на правило
/// по id).
struct RuleSnapshot {
    /// Идентификатор правила (`id: C-NNN`), если задан.
    id: Option<String>,
    /// Множество `exclude_glob`.
    excludes: BTreeSet<String>,
    /// Нормализованный severity (`error`/`warn`).
    severity: &'static str,
}

/// Строит карту «имя правила → снимок» из разобранного constraint-файла.
/// Невалидный severity — ошибка разбора (ту же ошибку дал бы и `check`).
fn rules_snapshot(parsed: &ConstraintsFile) -> Result<BTreeMap<String, RuleSnapshot>> {
    let mut out = BTreeMap::new();
    for rule in parsed.all_rules() {
        out.insert(
            rule.name.clone(),
            RuleSnapshot {
                id: rule.id.clone(),
                excludes: rule.exclude_glob.iter().cloned().collect(),
                severity: normalize_severity(&rule.severity, &rule.name)?,
            },
        );
    }
    Ok(out)
}

/// Анти-ослабление fitness-гейта (находка `rule_weakened`): сравнивает
/// текущий `CONSTRAINTS.yaml` с версией из git-базы (обе версии — текстами,
/// git-разрешение делает вызывающий — [`crate::gate`]). Ослабления, дающие
/// error-находку с именем правила:
///
/// - правило из базы исчезло из текущего файла (по именам);
/// - у правила появился/расширился `exclude_glob` (новые glob'ы исключений);
/// - severity понижен (error → warn, нормализация [`normalize_severity`]).
///
/// Ослабление УЗАКОНЕНО (находки нет), если в текущем файле есть активный
/// override на это правило (по имени или id) с ADR — гейт «только через ADR»
/// (`docs/corp-spine.md`).
///
/// Сравнение — по плоскому разбору этого файла (оба корня `rules:`/
/// `constraints:`), наследование `extends` не разворачивается: правке через
/// смену пина родителя соответствует отдельная error-находка резолва.
///
/// # Errors
/// YAML любой из версий невалиден, severity правила вне допустимых значений.
pub fn rule_weakened(current_src: &str, base_src: &str, file: &Path) -> Result<Vec<LintIssue>> {
    // Толерантный разбор (E8): записи с неизвестными типами в сравнении не
    // участвуют (сравнивать не с чем — они не из нашего словаря); их
    // warn-пропуск покажет `check`.
    let (current, _) = parse_constraints_file(current_src, file)?;
    let (base, _) = parse_constraints_file(base_src, file)?;
    let current_rules = rules_snapshot(&current)?;
    let base_rules = rules_snapshot(&base)?;
    let legalized = active_override_keys(&current.overrides);

    let mut issues = Vec::new();
    let push = |kind: WeakenedKind,
                name: &str,
                id: Option<&str>,
                detail: String,
                issues: &mut Vec<LintIssue>| {
        // Активный override по имени или id правила узаконивает ослабление.
        if legalized.contains(name) || id.is_some_and(|i| legalized.contains(i)) {
            return;
        }
        issues.push(LintIssue {
            file: file.to_path_buf(),
            line: 0,
            rule: "rule_weakened".to_string(),
            message: format!(
                "правило '{name}': {} — {detail}; ослабление гейта требует активный override \
                 с ADR (overrides: rule+adr+until)",
                kind.label()
            ),
            severity: "error".to_string(),
            ..LintIssue::default()
        });
    };

    for (name, base_rule) in &base_rules {
        let Some(current_rule) = current_rules.get(name) else {
            push(
                WeakenedKind::Removed,
                name,
                base_rule.id.as_deref(),
                "правило присутствовало в базовой версии и удалено".to_string(),
                &mut issues,
            );
            continue;
        };
        let added: Vec<String> = current_rule
            .excludes
            .difference(&base_rule.excludes)
            .cloned()
            .collect();
        if !added.is_empty() {
            push(
                WeakenedKind::ExcludeWidened,
                name,
                current_rule.id.as_deref(),
                format!("новые исключения: {}", added.join(", ")),
                &mut issues,
            );
        }
        if base_rule.severity == "error" && current_rule.severity == "warn" {
            push(
                WeakenedKind::SeverityLowered,
                name,
                current_rule.id.as_deref(),
                "error → warn".to_string(),
                &mut issues,
            );
        }
    }
    Ok(issues)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::{FitnessRule, LintIssue, check};

    /// Пишет файл в каталог и возвращает его путь.
    fn write_file(dir: &Path, name: &str, content: &str) -> PathBuf {
        let p = dir.join(name);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&p, content).unwrap();
        p
    }
    // --- Анти-ослабление гейта (rule_weakened) ------------------------------

    /// Базовая версия CONSTRAINTS.yaml для тестов `rule_weakened`: два
    /// правила уровня error, у `no-pan` есть id для матчинга overrides.
    const WEAK_BASE: &str = "rules:\n\
         - name: no-pan\n  \
         id: C-01\n  \
         type: must_not_contain\n  \
         glob: \"src/**\"\n  \
         pattern: 'PAN'\n  \
         severity: error\n\
         - name: spine-present\n  \
         type: file_exists\n  \
         path: \"ARCHITECTURE-SPINE.md\"\n  \
         severity: error\n";

    /// Н12: реестр правил в ПОДКАТАЛОГЕ сверяется со своей базовой версией, а
    /// не с корневым реестром репозитория. Иначе каждое правило корня
    /// выглядело бы «удалённым» — ложный `rule_weakened` на реестре кейса,
    /// который существует ровно в одном экземпляре.
    #[test]
    fn rule_anchor_reads_the_subdirectory_registry_from_the_git_root() {
        let tmp = tempfile::tempdir().expect("tmp");
        let repo = tmp.path();
        let case = repo.join("кейсы/x");
        std::fs::create_dir_all(&case).expect("mkdir");
        std::fs::write(
            repo.join("CONSTRAINTS.yaml"),
            "rules:\n  - name: root_rule\n    type: file_exists\n    path: \"A\"\n    severity: error\n",
        )
        .expect("root registry");
        let case_registry = case.join("CONSTRAINTS.yaml");
        std::fs::write(
            &case_registry,
            "rules:\n  - name: case_rule\n    type: file_exists\n    path: \"B\"\n    severity: error\n",
        )
        .expect("case registry");
        git(repo, &["init", "-q"]);
        git(repo, &["add", "-A"]);
        git(repo, &["commit", "-q", "-m", "init"]);

        let anchor = rule_anchor(&case, &case_registry, None);
        assert!(
            anchor.issues.is_empty(),
            "корневое правило не имеет отношения к реестру кейса: {:?}",
            anchor.issues
        );

        // Ослабление СВОЕГО правила по-прежнему видно.
        std::fs::write(&case_registry, "rules: []\n").expect("weakened");
        let anchor = rule_anchor(&case, &case_registry, None);
        assert!(
            anchor.issues.iter().any(|i| i.rule == "rule_weakened"),
            "ослабление реестра кейса обязано остаться видимым: {:?}",
            anchor.issues
        );
    }

    /// git в каталоге с тестовой идентичностью коммиттера.
    fn git(dir: &Path, args: &[&str]) {
        let out = std::process::Command::new("git")
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

    /// Текущая версия, идентичная базовой (без ослаблений).
    #[test]
    fn rule_weakened_clean_when_unchanged_or_strengthened() {
        let file = Path::new("CONSTRAINTS.yaml");
        let issues = rule_weakened(WEAK_BASE, WEAK_BASE, file).expect("сравнение");
        assert!(issues.is_empty(), "{issues:?}");
        // Усиление (новое правило, поднятие severity, добавление glob) —
        // не ослабление: находок нет.
        let stronger = "rules:\n\
             - name: no-pan\n  \
             id: C-01\n  \
             type: must_not_contain\n  \
             glob: \"src/**\"\n  \
             pattern: 'PAN'\n  \
             severity: critical\n\
             - name: spine-present\n  \
             type: file_exists\n  \
             path: \"ARCHITECTURE-SPINE.md\"\n  \
             severity: error\n\
             - name: no-dbg\n  \
             type: must_not_contain\n  \
             glob: \"src/**\"\n  \
             pattern: 'dbg!'\n  \
             severity: warn\n";
        let issues = rule_weakened(stronger, WEAK_BASE, file).expect("сравнение");
        assert!(issues.is_empty(), "{issues:?}");
    }

    /// Удаление правила (агент «позеленил» гейт) — error-находка с именем.
    #[test]
    fn rule_weakened_flags_removed_rule() {
        let current = "rules:\n\
             - name: spine-present\n  \
             type: file_exists\n  \
             path: \"ARCHITECTURE-SPINE.md\"\n  \
             severity: error\n";
        let issues =
            rule_weakened(current, WEAK_BASE, Path::new("CONSTRAINTS.yaml")).expect("сравнение");
        assert_eq!(issues.len(), 1, "{issues:?}");
        let i = &issues[0];
        assert_eq!(i.rule, "rule_weakened");
        assert_eq!(i.severity, "error");
        assert!(i.message.contains("no-pan"), "{}", i.message);
        assert!(i.message.contains("удалено из реестра"), "{}", i.message);
    }

    /// Появление `exclude_glob` у правила — error-находка.
    #[test]
    fn rule_weakened_flags_new_exclude_glob() {
        let current = WEAK_BASE.replace(
            "pattern: 'PAN'\n  severity: error",
            "pattern: 'PAN'\n  exclude_glob: [\"src/legacy/**\"]\n  severity: error",
        );
        assert_ne!(current, WEAK_BASE, "подстановка сработала");
        let issues =
            rule_weakened(&current, WEAK_BASE, Path::new("CONSTRAINTS.yaml")).expect("сравнение");
        assert_eq!(issues.len(), 1, "{issues:?}");
        assert!(
            issues[0].message.contains("exclude_glob"),
            "{}",
            issues[0].message
        );
        assert!(
            issues[0].message.contains("src/legacy/**"),
            "{}",
            issues[0].message
        );
    }

    /// Понижение severity error → warn без override — error-находка;
    /// нормализованные эквиваленты (critical → error) ослаблением не считаются.
    #[test]
    fn rule_weakened_flags_severity_downgrade_only_when_real() {
        let lowered = WEAK_BASE.replace(
            "path: \"ARCHITECTURE-SPINE.md\"\n  severity: error",
            "path: \"ARCHITECTURE-SPINE.md\"\n  severity: warn",
        );
        assert_ne!(lowered, WEAK_BASE, "подстановка сработала");
        let issues =
            rule_weakened(&lowered, WEAK_BASE, Path::new("CONSTRAINTS.yaml")).expect("сравнение");
        assert_eq!(issues.len(), 1, "{issues:?}");
        assert!(
            issues[0].message.contains("spine-present")
                && issues[0].message.contains("severity понижен"),
            "{}",
            issues[0].message
        );
        // critical → error: оба нормализуются в error — не ослабление.
        let base_critical = WEAK_BASE.replacen("severity: error", "severity: critical", 1);
        let current_error = WEAK_BASE;
        let issues = rule_weakened(current_error, &base_critical, Path::new("CONSTRAINTS.yaml"))
            .expect("сравнение");
        assert!(issues.is_empty(), "{issues:?}");
    }

    /// Ослабление с АКТИВНЫМ override (rule+adr+until в будущем) — узаконено:
    /// находок нет; просроченный override ослабление не легализует.
    #[test]
    fn rule_weakened_active_override_legalizes_weakening() {
        // Удаление no-pan + активный override по id правила (C-01).
        let legal = "rules:\n\
             - name: spine-present\n  \
             type: file_exists\n  \
             path: \"ARCHITECTURE-SPINE.md\"\n  \
             severity: error\n\
             overrides:\n\
             - rule: C-01\n  \
             adr: ADR-042\n  \
             until: \"2999-01\"\n";
        let issues =
            rule_weakened(legal, WEAK_BASE, Path::new("CONSTRAINTS.yaml")).expect("сравнение");
        assert!(
            issues.is_empty(),
            "активный override узаконивает: {issues:?}"
        );

        // Тот же override, но просроченный, — ослабление снова находка.
        let expired = legal.replace("2999-01", "2001-01");
        let issues =
            rule_weakened(&expired, WEAK_BASE, Path::new("CONSTRAINTS.yaml")).expect("сравнение");
        assert_eq!(issues.len(), 1, "{issues:?}");
        assert!(
            issues[0].message.contains("no-pan"),
            "{}",
            issues[0].message
        );

        // Override на ДРУГОЕ правило ослабление no-pan не легализует.
        let alien = legal.replace("rule: C-01", "rule: spine-present");
        let issues =
            rule_weakened(&alien, WEAK_BASE, Path::new("CONSTRAINTS.yaml")).expect("сравнение");
        assert_eq!(issues.len(), 1, "{issues:?}");
    }

    /// Невалидный YAML любой из версий — ошибка разбора, не паника.
    #[test]
    fn rule_weakened_reports_broken_yaml() {
        assert!(rule_weakened("{битый", WEAK_BASE, Path::new("C.yaml")).is_err());
        assert!(rule_weakened(WEAK_BASE, "{битый", Path::new("C.yaml")).is_err());
    }

    // --- Корп-спайн: extends / deny_dependency / severity / overrides / report ---

    /// Корп-родитель для фикстур наследования (docs/corp-spine.md).
    const CORP_YAML: &str = "version: \"2026.3\"\n\
        rules:\n\
        \x20 - id: C-CORP-001\n\
        \x20   name: no_hold_crates\n\
        \x20   type: deny_dependency\n\
        \x20   deny: [left-pad, openssl-sys]\n\
        \x20   reference: \"Техкомитет 2026-03, протокол №12\"\n\
        \x20   severity: block\n\
        \x20 - id: C-CORP-002\n\
        \x20   name: corp_readme_present\n\
        \x20   type: file_exists\n\
        \x20   path: \"README.md\"\n\
        \x20   severity: warn\n";

    /// Пишет корп-родителя и продуктовый файл с extends в tempdir;
    /// возвращает (tempdir, путь к продуктовому CONSTRAINTS.yaml).
    fn extends_fixture(corp_version: &str, pin: &str) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        write_file(
            dir.path(),
            "corp/CONSTRAINTS.corp.yaml",
            &CORP_YAML.replace("2026.3", corp_version),
        );
        let product = write_file(
            dir.path(),
            "product/CONSTRAINTS.yaml",
            &format!(
                "extends: [\"../corp/CONSTRAINTS.corp.yaml@{pin}\"]\n\
                 rules:\n\
                 \x20 - name: own_rule\n\
                 \x20   type: must_contain\n\
                 \x20   glob: \"marker.txt\"\n\
                 \x20   pattern: \"ok\"\n"
            ),
        );
        write_file(dir.path(), "product/marker.txt", "ok\n");
        write_file(dir.path(), "product/README.md", "# product\n");
        (dir, product)
    }

    #[test]
    fn extends_inherits_rules_with_source_label() {
        let (dir, product) = extends_fixture("2026.3", "2026.3");
        let resolved = load_constraints_resolved(&product).unwrap();
        assert_eq!(resolved.rules.len(), 3);
        assert_eq!(resolved.findings.len(), 0, "{:?}", resolved.findings);
        let inherited: Vec<&FitnessRule> = resolved
            .rules
            .iter()
            .filter(|r| r.source.is_some())
            .collect();
        assert_eq!(inherited.len(), 2);
        assert_eq!(
            inherited[0].source.as_deref(),
            Some("CONSTRAINTS.corp@2026.3")
        );
        assert!(resolved.rules.iter().any(|r| r.source.is_none()));

        let report = check(&dir.path().join("product"), &product).unwrap();
        assert!(report.passed, "{:?}", report.issues);
        assert_eq!(report.inherited.len(), 1);
        assert_eq!(report.inherited[0].source, "CONSTRAINTS.corp@2026.3");
        assert_eq!(report.inherited[0].rules, 2);
    }

    #[test]
    fn extends_version_mismatch_is_error_finding() {
        let (dir, product) = extends_fixture("2026.4", "2026.3");
        let report = check(&dir.path().join("product"), &product).unwrap();
        assert!(!report.passed, "расхождение пина ломает гейт");
        let finding = report
            .issues
            .iter()
            .find(|i| i.rule == "extends")
            .expect("находка extends");
        assert_eq!(finding.severity, "error");
        assert!(
            finding.message.contains("2026.3 → 2026.4"),
            "{}",
            finding.message
        );
    }

    #[test]
    fn extends_cycle_is_error() {
        let dir = tempfile::tempdir().unwrap();
        write_file(
            dir.path(),
            "a.yaml",
            "version: \"1\"\nextends: [\"b.yaml@1\"]\nrules: []\n",
        );
        write_file(
            dir.path(),
            "b.yaml",
            "version: \"1\"\nextends: [\"a.yaml@1\"]\nrules: []\n",
        );
        let err = load_constraints_resolved(&dir.path().join("a.yaml")).unwrap_err();
        assert!(err.to_string().contains("цикл"), "{err}");
    }

    #[test]
    fn extends_without_pin_is_error() {
        let dir = tempfile::tempdir().unwrap();
        let file = write_file(
            dir.path(),
            "p.yaml",
            "extends: [\"corp.yaml@\"]\nrules: []\n",
        );
        let err = load_constraints_resolved(&file).unwrap_err();
        assert!(err.to_string().contains("<ref>@<version>"), "{err}");
    }

    #[test]
    fn deny_dependency_hits_all_manifest_formats() {
        let dir = tempfile::tempdir().unwrap();
        write_file(
            dir.path(),
            "Cargo.toml",
            "[package]\nname = \"x\"\n\n[dependencies]\nserde = \"1\"\nleft-pad = \"0.1\"\n\n[dev-dependencies]\n# comment\ntempfile = \"3\"\n",
        );
        write_file(
            dir.path(),
            "service/pom.xml",
            "<project>\n  <artifactId>my-service</artifactId>\n  <dependency>\n    <artifactId>openssl-sys</artifactId>\n  </dependency>\n</project>\n",
        );
        write_file(
            dir.path(),
            "ml/requirements.txt",
            "# комментарий\nrequests>=2.0\nleft-pad == 1.0  # pinned\n-r base.txt\n",
        );
        let constraints = write_file(dir.path(), "CONSTRAINTS.yaml", CORP_YAML);
        let report = check(dir.path(), &constraints).unwrap();
        assert!(!report.passed);
        let deny: Vec<&LintIssue> = report
            .issues
            .iter()
            .filter(|i| i.rule == "no_hold_crates")
            .collect();
        // Cargo.toml left-pad, pom openssl-sys, requirements left-pad.
        assert_eq!(deny.len(), 3, "{deny:?}");
        assert!(
            deny.iter()
                .all(|i| i.message.contains("Техкомитет 2026-03"))
        );
        assert!(
            deny.iter()
                .any(|i| i.file == Path::new("Cargo.toml") && i.line == 6)
        );
        assert!(deny.iter().any(|i| i.file == Path::new("service/pom.xml")));
        assert!(
            deny.iter()
                .any(|i| i.file == Path::new("ml/requirements.txt") && i.line == 3)
        );
        // serde/requests не в deny — находок нет.
        assert!(!deny.iter().any(|i| i.message.contains("serde")));
    }

    #[test]
    fn deny_dependency_empty_deny_is_config_error() {
        let dir = tempfile::tempdir().unwrap();
        let constraints = write_file(
            dir.path(),
            "CONSTRAINTS.yaml",
            "rules:\n  - name: bad\n    type: deny_dependency\n    deny: []\n",
        );
        let err = check(dir.path(), &constraints).unwrap_err();
        assert!(err.to_string().contains("deny"), "{err}");
    }

    #[test]
    fn severity_warn_rule_does_not_break_gate() {
        let dir = tempfile::tempdir().unwrap();
        // C-CORP-002 (file_exists README.md, severity warn) не находит README —
        // warn-находка, гейт PASS; deny-правилу нечего найти (манифестов нет).
        let constraints = write_file(dir.path(), "CONSTRAINTS.yaml", CORP_YAML);
        let report = check(dir.path(), &constraints).unwrap();
        assert!(report.passed, "{:?}", report.issues);
        let warn = report
            .issues
            .iter()
            .find(|i| i.rule == "corp_readme_present")
            .expect("warn-находка");
        assert_eq!(warn.severity, "warn");
        assert_eq!(report.summary.matches("error: 0").count(), 1);
    }

    /// Продуктовый файл с override-фикстурой (тело overrides — параметр).
    fn override_fixture(overrides_yaml: &str) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        write_file(dir.path(), "corp/CONSTRAINTS.corp.yaml", CORP_YAML);
        write_file(dir.path(), "product/README.md", "# product\n");
        let product = write_file(
            dir.path(),
            "product/CONSTRAINTS.yaml",
            &format!(
                "extends: [\"../corp/CONSTRAINTS.corp.yaml@2026.3\"]\n\
                 overrides:\n{overrides_yaml}\n\
                 rules:\n\
                 \x20 - name: own_rule\n\
                 \x20   type: file_exists\n\
                 \x20   path: \"README.md\"\n"
            ),
        );
        (dir, product)
    }

    #[test]
    fn override_active_disables_rule() {
        // Cargo.toml содержит deny-пакет, но правило отключено активным override.
        let (dir, product) =
            override_fixture("  - rule: C-CORP-001\n    adr: ADR-041\n    until: \"2999-01\"\n");
        write_file(
            dir.path(),
            "product/Cargo.toml",
            "[dependencies]\nleft-pad = \"0.1\"\n",
        );
        let report = check(&dir.path().join("product"), &product).unwrap();
        assert!(report.passed, "{:?}", report.issues);
        assert!(!report.issues.iter().any(|i| i.rule == "no_hold_crates"));
        let info = &report.overrides[0];
        assert_eq!(info.status, "active");
        assert_eq!(info.rule, "C-CORP-001");
    }

    #[test]
    fn override_incomplete_is_error_and_rule_stays() {
        let (dir, product) = override_fixture("  - rule: C-CORP-001\n    until: \"2999-01\"\n");
        write_file(
            dir.path(),
            "product/Cargo.toml",
            "[dependencies]\nleft-pad = \"0.1\"\n",
        );
        let report = check(&dir.path().join("product"), &product).unwrap();
        assert!(!report.passed);
        // Неполный override — error-находка, правило НЕ отключено (deny-hit есть).
        assert!(
            report
                .issues
                .iter()
                .any(|i| i.rule == "override" && i.severity == "error")
        );
        assert!(report.issues.iter().any(|i| i.rule == "no_hold_crates"));
        assert_eq!(report.overrides[0].status, "invalid");
    }

    #[test]
    fn override_expired_warns_and_rule_applies_again() {
        let (dir, product) =
            override_fixture("  - rule: C-CORP-001\n    adr: ADR-041\n    until: \"2020-01\"\n");
        write_file(
            dir.path(),
            "product/Cargo.toml",
            "[dependencies]\nleft-pad = \"0.1\"\n",
        );
        let report = check(&dir.path().join("product"), &product).unwrap();
        assert!(!report.passed, "правило снова действует → deny-hit");
        assert!(report.issues.iter().any(|i| i.rule == "no_hold_crates"));
        let expired = report
            .issues
            .iter()
            .find(|i| i.rule == "override" && i.message.contains("истёк"))
            .expect("warn «override истёк»");
        assert_eq!(expired.severity, "warn");
        assert_eq!(report.overrides[0].status, "expired");
    }

    #[test]
    fn override_unknown_rule_is_warn() {
        let (dir, product) =
            override_fixture("  - rule: C-CORP-999\n    adr: ADR-041\n    until: \"2999-01\"\n");
        let report = check(&dir.path().join("product"), &product).unwrap();
        assert!(report.passed, "warn не ломает гейт: {:?}", report.issues);
        assert!(report.issues.iter().any(|i| i.rule == "override"
            && i.severity == "warn"
            && i.message.contains("несуществующее правило")));
        assert_eq!(report.overrides[0].status, "invalid");
    }

    /// П5: отпечаток состава реестра меняется при удалении правила.
    #[test]
    fn rules_fingerprint_tracks_composition() {
        let tmp = tempfile::tempdir().expect("tmp");
        let repo = tmp.path();
        let c = repo.join("CONSTRAINTS.yaml");
        std::fs::write(
            &c,
            "rules:\n  - id: C-1\n    name: a\n    type: file_exists\n    path: \"A\"\n    severity: error\n  - id: C-2\n    name: b\n    type: file_exists\n    path: \"B\"\n    severity: warn\n",
        )
        .expect("write");
        let first = check(repo, &c)
            .expect("check")
            .fingerprint
            .expect("отпечаток");
        assert_eq!(first.rules, 2);
        assert_eq!(first.errors, 1);
        std::fs::write(
            &c,
            "rules:\n  - id: C-2\n    name: b\n    type: file_exists\n    path: \"B\"\n    severity: warn\n",
        )
        .expect("write");
        let second = check(repo, &c)
            .expect("check")
            .fingerprint
            .expect("отпечаток");
        assert_eq!(second.rules, 1);
        assert_ne!(
            first.hash, second.hash,
            "состав реестра изменился — отпечаток обязан измениться"
        );
    }

    /// П5 (Д4): `control check`-канал ловит исчезновение правила через
    /// сверку с git-базой, а не только составной гейт.
    #[test]
    fn check_anchored_flags_removed_rule() {
        let tmp = tempfile::tempdir().expect("tmp");
        let repo = tmp.path();
        let c = repo.join("CONSTRAINTS.yaml");
        let two = "rules:\n  - id: C-1\n    name: a\n    type: file_exists\n    path: \"A\"\n    severity: error\n  - id: C-2\n    name: b\n    type: file_exists\n    path: \"B\"\n    severity: error\n";
        std::fs::write(&c, two).expect("write");
        let git = |args: &[&str]| {
            let status = Command::new("git")
                .arg("-C")
                .arg(repo)
                .args(args)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .expect("git");
            assert!(status.success(), "git {args:?}");
        };
        git(&["init", "-q"]);
        git(&["add", "CONSTRAINTS.yaml"]);
        git(&[
            "-c",
            "user.email=t@example.invalid",
            "-c",
            "user.name=test",
            "commit",
            "-q",
            "-m",
            "init",
        ]);
        // Правило C-1 удалено из рабочего дерева (антикейс «зеленения»).
        std::fs::write(
            &c,
            "rules:\n  - id: C-2\n    name: b\n    type: file_exists\n    path: \"B\"\n    severity: error\n",
        )
        .expect("write");
        let report =
            check_anchored(repo, &c, &baseline::CheckOptions::default(), None).expect("check");
        assert!(!report.passed, "{}", report.summary);
        assert!(
            report.issues.iter().any(|i| i.rule == "rule_weakened"),
            "находки: {:?}",
            report.issues.iter().map(|i| &i.rule).collect::<Vec<_>>()
        );
    }
}
