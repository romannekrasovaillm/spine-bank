//! Движок исполнения fitness-правил ([`check`], [`check_with_options`]):
//! glob-обход репозитория, content-правила, `command_succeeds` (таймаут через
//! [`crate::proc`], доверие через [`crate::cmd_trust`], прогонщики через
//! [`crate::rule_templates`]), структурные правила `dependency_direction` /
//! `context_boundary` (ADR-029/030) и `deny_dependency`.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitStatus;
use std::time::{Duration, Instant, SystemTime};

use regex::Regex;
use walkdir::WalkDir;

use super::baseline;
use super::registry::{evaluate_overrides, load_constraints_resolved};
use super::rules::{DEFAULT_MANIFEST_GLOBS, manifest_deps};
use super::types::{
    FitnessReport, FitnessRule, LintIssue, RuleDuration, RuleKind, RulesFingerprint,
    RunnerSkippedRule, SourceCount, UntrustedSkippedRule, normalize_severity,
};
use crate::error::{HarnessError, Result};

/// Вычитает `exclude_glob`'ы правила из набора относительных путей.
fn apply_excludes<T: AsRef<str>>(paths: &mut Vec<T>, excludes: &[String]) {
    if excludes.is_empty() {
        return;
    }
    paths.retain(|p| {
        let s = p.as_ref();
        !excludes.iter().any(|ex| glob_matches(ex, s))
    });
}

/// Дефолтный таймаут `command_succeeds`, секунды.
const COMMAND_TIMEOUT_DEFAULT_SECS: u64 = 60;

/// Прогон fitness functions из `CONSTRAINTS.yaml` по репозиторию.
///
/// `passed = true`, если нет находок с severity `error`. Обход репозитория
/// пропускает каталоги `.git` и `target`; файлы в не-UTF8 кодировке читаются
/// с потерями (content-правила по ним приблизительны). Наследование
/// `extends` резолвится ([`load_constraints_resolved`]): унаследованные
/// правила несут метку источника (отчёт `inherited`), расхождение пина
/// версии — error-находка; активные overrides отключают свои правила
/// (отчёт `overrides`, `docs/corp-spine.md`).
///
/// # Errors
/// `CONSTRAINTS.yaml` не читается/не валиден, репозиторий недоступен,
/// правило некорректно (нет pattern/path/command, невалидный regex/severity),
/// родитель из `extends` не найден, цикл наследования.
pub fn check(repo: &Path, constraints: &Path) -> Result<FitnessReport> {
    check_with_options(repo, constraints, &baseline::CheckOptions::default())
}

/// Полный вариант [`check`] с опциями ([`baseline::CheckOptions`], модуль
/// [`baseline`], `docs/control.md`):
///
/// - `baseline` (путь) — режим ratchet: находки, присутствующие в
///   baseline-файле, — исторический долг (в отчёт `baseline.debt`, гейт не
///   ломают); НОВАЯ error-находка (нет отпечатка в baseline) — ломает гейт;
///   рост счётчика error-находок правила против baseline — error-находка
///   (страховка от коллизий отпечатков). Warn-находки и находки механики
///   (`extends`/`override`) в ratchet не участвуют: первые гейт не ломают
///   никогда, вторые — сломанная конфигурация, а не кодовый долг;
/// - `baseline_update` — перезаписать baseline текущим состоянием
///   ([`baseline::ensure_shrinks`]: принимается только при неухудшении долга,
///   иначе — ошибка, файл не трогается); после принятого обновления весь
///   текущий долг зафиксирован и гейт зелёный (кроме находок механики);
/// - `changed_since` (git-реф) — файловые правила исполняются на срезе
///   изменённых файлов ([`baseline::changed_files_since`]), глобальные
///   пропускаются (отчёт `skipped`); закрытие долга в этом режиме не
///   отслеживается, а `baseline_update` запрещён (срез уничтожил бы записи
///   долга в нетронутых файлах).
///
/// # Errors
/// Те же, что у [`check`]; плюс: baseline-файл не читается/невалиден (в
/// режиме ratchet без обновления — обязан существовать), обновление при
/// выросшем долге, `--baseline-update` в сочетании с `--changed-since`,
/// некорректный git-реф.
pub fn check_with_options(
    repo: &Path,
    constraints: &Path,
    options: &baseline::CheckOptions,
) -> Result<FitnessReport> {
    if !repo.is_dir() {
        return Err(HarnessError::Control(format!(
            "репозиторий недоступен: {}",
            repo.display()
        )));
    }
    // Валидация сочетаний флагов — до любой работы.
    if options.baseline_update && options.changed_since.is_some() {
        return Err(HarnessError::Control(
            "--baseline-update несовместим с --changed-since: обновление baseline требует полного \
             прогона — срез изменённых файлов уничтожил бы записи долга в нетронутых файлах"
                .to_string(),
        ));
    }
    let baseline_path = if options.baseline_update {
        Some(
            options
                .baseline
                .clone()
                .unwrap_or_else(|| repo.join(baseline::DEFAULT_BASELINE_PATH)),
        )
    } else {
        options.baseline.clone()
    };
    let changed: Option<BTreeSet<String>> = match &options.changed_since {
        Some(reference) => Some(baseline::changed_files_since(repo, reference)?),
        None => None,
    };

    let resolved = load_constraints_resolved(constraints)?;
    if resolved.rules.is_empty() {
        // E8: файл, где ВСЕ правила с неизвестными типами, — это не
        // «нет правил», а чужой словарь; сообщение должно это различать.
        if resolved.skipped_unknown.is_empty() {
            return Err(HarnessError::Control(format!(
                "{}: файл не содержит правил — ожидается непустой корень `rules:`/`constraints:` или `extends:`",
                constraints.display()
            )));
        }
        let types: BTreeSet<&str> = resolved
            .skipped_unknown
            .iter()
            .map(|s| s.rule_type.as_str())
            .collect();
        return Err(HarnessError::Control(format!(
            "{}: исполняемых правил нет — все {} записей пропущены из-за неизвестных типов ({}) — словарь другой редакции?",
            constraints.display(),
            resolved.skipped_unknown.len(),
            types.into_iter().collect::<Vec<_>>().join(", ")
        )));
    }
    let (override_infos, override_findings, disabled) =
        evaluate_overrides(&resolved.overrides, &resolved.rules, constraints);

    // П5: отпечаток состава реестра — до перемещения правил.
    let fingerprint = Some(rules_fingerprint_of(&resolved.rules));
    let rules = resolved.rules;
    let skipped_unknown = resolved.skipped_unknown;
    let rule_refs: Vec<&FitnessRule> = rules
        .iter()
        .filter(|r| !disabled.contains(&r.name) && !r.unverifiable)
        .collect();

    let mut issues = resolved.findings;
    issues.extend(override_findings);
    // Находки механики (наследование, overrides) — не кодовый долг: в baseline
    // не зашиваются и ratchet их не прощает.
    let mechanics_len = issues.len();
    let mut durations = Vec::new();
    let mut skipped: Vec<baseline::SkippedRule> = Vec::new();
    let mut runner_skipped: Vec<RunnerSkippedRule> = Vec::new();
    let mut untrusted_skipped: Vec<UntrustedSkippedRule> = Vec::new();
    // A3: решение об исполнении команд реестра — ОДИН снимок на весь прогон
    // (модель доверия, ADR-053): no-exec (флаг/переменная/дефолт MCP) или
    // allow-файл, не совпадающий с отпечатком набора команд. Отпечаток
    // считается по ВСЕМ разрешённым (resolved) command_succeeds-правилам —
    // тем же набором пользуется `arch-be rules allow`. Реестра без команд
    // решение не касается: он не может ничего исполнить.
    let exec = {
        let commands = command_strings(&rules);
        if commands.is_empty() {
            crate::cmd_trust::ExecDecision::Allow
        } else {
            let fingerprint = crate::cmd_trust::commands_fingerprint(commands);
            crate::cmd_trust::resolve(&options.exec, repo, &fingerprint)?
        }
    };
    // Прогонщики внешних команд (pytest/mvn/JDK) — один снимок на весь
    // прогон (A2). Детект — только когда в реестре есть исполняемые правила
    // и исполнение РАЗРЕШЕНО: под запретом доверия команды не прогоняются,
    // и лишний подпроцесс (`python3 -c "import pytest"`) не нужен.
    let runner = if exec.is_deny()
        || !rule_refs
            .iter()
            .any(|r| matches!(r.kind, RuleKind::CommandSucceeds))
    {
        crate::rule_templates::Runner::unavailable()
    } else {
        crate::rule_templates::Runner::detect(None)
    };
    for rule in &rule_refs {
        let started = Instant::now();
        run_rule(
            rule,
            repo,
            &rule_refs,
            changed.as_ref(),
            &mut skipped,
            &mut runner_skipped,
            &mut untrusted_skipped,
            &runner,
            exec,
            &mut issues,
        )?;
        // u128 → u64 с насыщением: переполнение недостижимо практически
        // (584 млн лет), насыщение — страховка вместо паники.
        let ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        durations.push(RuleDuration {
            rule: rule.name.clone(),
            ms,
        });
    }

    // Ratchet: находки правил сверяются с baseline; долг уходит из `issues`
    // в отчёт `baseline`, новые находки и рост счётчиков остаются error'ами.
    let mut baseline_report: Option<baseline::BaselineReport> = None;
    if let Some(path) = &baseline_path {
        let rule_issues = issues.split_off(mechanics_len);
        let mut error_issues = Vec::new();
        let mut warn_issues = Vec::new();
        for issue in rule_issues {
            if issue.severity == "error" {
                error_issues.push(issue);
            } else {
                warn_issues.push(issue);
            }
        }
        if options.baseline_update {
            let today = chrono::Local::now().format("%Y-%m-%d").to_string();
            let old = if path.is_file() {
                Some(baseline::load(path)?)
            } else {
                None
            };
            let new_baseline = baseline::Baseline::from_issues(&error_issues, &today);
            baseline::ensure_shrinks(old.as_ref(), &new_baseline)?;
            baseline::save(path, &new_baseline)?;
            let debt: Vec<baseline::RuleDebt> = new_baseline
                .rules
                .iter()
                .map(baseline::RuleDebt::from_baseline_rule)
                .collect();
            let closed = match &old {
                Some(old) => new_baseline.closed_since(old),
                None => Vec::new(),
            };
            baseline_report = Some(baseline::BaselineReport {
                path: path.clone(),
                updated: true,
                debt_total: debt.iter().map(|d| d.count).sum(),
                closed_total: closed.len(),
                debt,
                closed,
            });
        } else {
            if !path.is_file() {
                return Err(HarnessError::Control(format!(
                    "baseline: файл не найден: {} — сначала зафиксируйте долг: \
                     arch-be control check <repo> --baseline {} --baseline-update",
                    path.display(),
                    path.display()
                )));
            }
            let base = baseline::load(path)?;
            let classification =
                baseline::classify(&error_issues, &base, options.changed_since.is_none());
            for grown in &classification.grown {
                issues.push(LintIssue {
                    file: path.clone(),
                    line: 0,
                    rule: grown.rule.clone(),
                    message: format!(
                        "baseline: нарушений правила '{}' стало {}, было {} — \
                         долг может только убывать (ratchet)",
                        grown.rule, grown.now, grown.was
                    ),
                    severity: "error".to_string(),
                    ..LintIssue::default()
                });
            }
            issues.extend(classification.new_issues);
            baseline_report = Some(baseline::BaselineReport {
                path: path.clone(),
                updated: false,
                debt_total: classification.debt.iter().map(|d| d.count).sum(),
                closed_total: classification.closed.len(),
                debt: classification.debt,
                closed: classification.closed,
            });
        }
        issues.extend(warn_issues);
    }

    issues.sort_by(|a, b| {
        a.file
            .cmp(&b.file)
            .then(a.line.cmp(&b.line))
            .then(a.rule.cmp(&b.rule))
    });

    // Счётчики правил по источникам (наследование видно в выводе).
    let mut by_source: BTreeMap<String, usize> = BTreeMap::new();
    for rule in &rules {
        if let Some(source) = &rule.source {
            *by_source.entry(source.clone()).or_default() += 1;
        }
    }
    let inherited: Vec<SourceCount> = by_source
        .into_iter()
        .map(|(source, rules)| SourceCount { source, rules })
        .collect();

    let errors = issues.iter().filter(|i| i.severity == "error").count();
    let warns = issues.len() - errors;
    let mut summary = format!(
        "Правил: {}, нарушений: {} (error: {errors}, warn: {warns})",
        rule_refs.len(),
        issues.len()
    );
    if let Some(report) = &baseline_report {
        if report.updated {
            let _ = write!(
                summary,
                "; baseline обновлён: долг {} находок",
                report.debt_total
            );
        } else {
            let _ = write!(
                summary,
                "; долг baseline: {} находок (закрыто: {})",
                report.debt_total, report.closed_total
            );
        }
    }
    if let (Some(reference), Some(changed_set)) = (&options.changed_since, &changed) {
        let _ = write!(summary, "; срез {reference}: файлов {}", changed_set.len());
    }
    if !skipped_unknown.is_empty() {
        let types: BTreeSet<&str> = skipped_unknown
            .iter()
            .map(|s| s.rule_type.as_str())
            .collect();
        let _ = write!(
            summary,
            "; пропущено правил: {} (неизвестные типы: {})",
            skipped_unknown.len(),
            types.into_iter().collect::<Vec<_>>().join(", ")
        );
    }
    if !runner_skipped.is_empty() {
        // Не нарушения, но и не проверенные правила: вердикт неполон, и это
        // видно в сводке (в гейте пропуск error-правила — SKIP составляющей).
        let _ = write!(
            summary,
            "; не прогонялись (нет прогонщика): {}",
            runner_skipped.len()
        );
    }
    if !untrusted_skipped.is_empty() {
        // A3: пропуск по решению о доверии — событие политики: в сводке
        // маркером, в гейте — SKIP составляющей при любом severity.
        let _ = write!(
            summary,
            "; не исполнялись ({}): {}",
            crate::cmd_trust::COMMAND_UNTRUSTED,
            untrusted_skipped.len()
        );
    }
    if let Some(fp) = &fingerprint {
        // Запись в String не может завершиться ошибкой — игнор безопасен.
        let _ = write!(
            summary,
            "; реестр: {} правил (error: {}), отпечаток {}",
            fp.rules,
            fp.errors,
            fp.hash.get(..8).unwrap_or(fp.hash.as_str())
        );
    }
    Ok(FitnessReport {
        repo: repo.to_path_buf(),
        passed: errors == 0,
        issues,
        summary,
        durations,
        inherited,
        overrides: override_infos,
        baseline: baseline_report,
        skipped,
        changed_since: options.changed_since.clone(),
        changed_files: changed.as_ref().map(BTreeSet::len),
        skipped_unknown,
        runner_skipped,
        untrusted_skipped,
        fingerprint,
    })
}

/// Command-строки всех `command_succeeds`-правил набора (вербатим, в
/// порядке набора): вход отпечатка доверия A3 ([`crate::cmd_trust`]) и
/// подкоманды `arch-be rules allow`.
#[must_use]
pub fn command_strings(rules: &[FitnessRule]) -> Vec<&str> {
    rules
        .iter()
        .filter(|r| matches!(r.kind, RuleKind::CommandSucceeds))
        .filter_map(|r| r.command.as_deref())
        .collect()
}

/// Отпечаток состава реестра правил (П5): SHA-256 отсортированного набора
/// `id|name|severity` + счётчики.
fn rules_fingerprint_of(rules: &[FitnessRule]) -> RulesFingerprint {
    let mut keys: Vec<String> = rules
        .iter()
        .map(|r| {
            format!(
                "{}|{}|{}",
                r.id.as_deref().unwrap_or("-"),
                r.name,
                r.severity
            )
        })
        .collect();
    keys.sort();
    RulesFingerprint {
        hash: crate::hash::sha256_hex(keys.join("\n").as_bytes()),
        rules: rules.len(),
        errors: rules
            .iter()
            .filter(|r| normalize_severity(&r.severity, &r.name).map_or(true, |s| s == "error"))
            .count(),
    }
}

/// Выполняет одно fitness-правило, добавляя находки в `issues`.
///
/// `all_rules` — все правила того же файла: нужны типу `archunit`
/// (ADR-039), который исполняет java-правила всего `CONSTRAINTS.yaml`.
///
/// `changed` — срез режима `--changed-since` (модуль [`baseline`]): при
/// `Some` глобальные правила пропускаются (запись в `skipped`), а файловые
/// исполняются на подмножестве изменённых файлов.
///
/// `runner`/`runner_skipped` — A2: правило `command_succeeds`, чья команда
/// требует отсутствующий в PATH прогонщик (pytest/mvn/JDK), НЕ исполняется и
/// НЕ краснеет: запись уходит в `runner_skipped` с причиной и подсказкой по
/// установке. Гейт переводит такой пропуск error-правила в SKIP
/// составляющей, а не в PASS.
///
/// `exec`/`untrusted_skipped` — A3 (модель доверия, ADR-053): при запрете
/// исполнения (no-exec или несовпадающий allow-файл) правило
/// `command_succeeds` НЕ запускается вообще: запись уходит в
/// `untrusted_skipped` с причиной `command_untrusted`. Проверка доверия
/// предшествует проверке прогонщика: под запретом даже детект окружения
/// не нужен.
#[allow(clippy::too_many_arguments)]
fn run_rule(
    rule: &FitnessRule,
    repo: &Path,
    all_rules: &[&FitnessRule],
    changed: Option<&BTreeSet<String>>,
    skipped: &mut Vec<baseline::SkippedRule>,
    runner_skipped: &mut Vec<RunnerSkippedRule>,
    untrusted_skipped: &mut Vec<UntrustedSkippedRule>,
    runner: &crate::rule_templates::Runner,
    exec: crate::cmd_trust::ExecDecision,
    issues: &mut Vec<LintIssue>,
) -> Result<()> {
    let severity = normalize_severity(&rule.severity, &rule.name)?;
    // Просроченное правило (expiry в прошлом) — warn-находка независимо от
    // исхода проверки: «правило без срока жизни» (антипаттерн библиотеки №5)
    // становится механически видимым. Невалидная дата — не ошибка, поле
    // метаданное.
    if let Some(expiry) = &rule.expiry {
        if let Ok(date) = chrono::NaiveDate::parse_from_str(expiry.trim(), "%Y-%m-%d") {
            if date < chrono::Local::now().date_naive() {
                let owner = rule
                    .owner
                    .as_deref()
                    .map(|o| format!(" (владелец: {o})"))
                    .unwrap_or_default();
                let mut finding = LintIssue {
                    file: PathBuf::from("CONSTRAINTS.yaml"),
                    line: 0,
                    rule: rule.name.clone(),
                    message: format!(
                        "expiry: правило просрочено {expiry}{owner} — пересмотреть, продлить с владельцем или удалить"
                    ),
                    severity: "warn".into(),
                    ..LintIssue::default()
                };
                rule.apply_card(&mut finding);
                issues.push(finding);
            }
        }
    }
    // Режим --changed-since: глобальные и структурные правила на срезе файлов
    // лгут или неоправданно дороги (command_succeeds, archunit) — пропускаются
    // с пометкой в отчёте; полный прогон остаётся истиной гейта.
    if changed.is_some() && !rule.kind.is_file_scoped() {
        skipped.push(baseline::SkippedRule {
            rule: rule.name.clone(),
            reason: "глобальное правило — исполняется в полном прогоне".to_string(),
        });
        return Ok(());
    }
    // Карточный контекст правила (ad/adr/rationale/owner/fix_hint/skill)
    // проставляется в каждую находку — агент видит задетый инвариант и
    // подсказку исправления, а не только имя правила.
    let mut issue = |file: PathBuf, line: usize, message: String| {
        let mut finding = LintIssue {
            file,
            line,
            rule: rule.name.clone(),
            message,
            severity: severity.to_string(),
            ..LintIssue::default()
        };
        rule.apply_card(&mut finding);
        issues.push(finding);
    };
    match rule.kind {
        RuleKind::MustContain => {
            let (re, glob, files) = prep_content_rule(rule, repo)?;
            let Some(files) = scope_files(rule, changed, files, &glob, skipped) else {
                return Ok(());
            };
            let pattern = rule.pattern.as_deref().unwrap_or_default();
            let mut found = false;
            for (_, abs) in &files {
                let bytes = std::fs::read(abs).map_err(|e| HarnessError::io(abs, e))?;
                if re.is_match(&String::from_utf8_lossy(&bytes)) {
                    found = true;
                    break;
                }
            }
            if !found {
                issue(
                    PathBuf::from(&glob),
                    0,
                    format!(
                        "must_contain: паттерн '{pattern}' не найден ни в одном файле по glob '{glob}'"
                    ),
                );
            }
        }
        RuleKind::MustNotContain => {
            let (re, glob, files) = prep_content_rule(rule, repo)?;
            let Some(files) = scope_files(rule, changed, files, &glob, skipped) else {
                return Ok(());
            };
            let pattern = rule.pattern.as_deref().unwrap_or_default();
            for (rel, abs) in &files {
                let bytes = std::fs::read(abs).map_err(|e| HarnessError::io(abs, e))?;
                let content = String::from_utf8_lossy(&bytes);
                for (idx, line) in content.lines().enumerate() {
                    if re.is_match(line) {
                        let snippet: String = line.trim().chars().take(120).collect();
                        issue(
                            PathBuf::from(rel),
                            idx + 1,
                            format!("must_not_contain: запрещённый паттерн '{pattern}': {snippet}"),
                        );
                    }
                }
            }
        }
        RuleKind::EachFileMustContain => {
            let (re, glob, files) = prep_content_rule(rule, repo)?;
            let Some(files) = scope_files(rule, changed, files, &glob, skipped) else {
                return Ok(());
            };
            let pattern = rule.pattern.as_deref().unwrap_or_default();
            if files.is_empty() {
                issue(
                    PathBuf::from(&glob),
                    0,
                    format!(
                        "each_file_must_contain: по glob '{glob}' не найдено ни одного файла — правилу нечего проверять"
                    ),
                );
            }
            for (rel, abs) in &files {
                let bytes = std::fs::read(abs).map_err(|e| HarnessError::io(abs, e))?;
                if !re.is_match(&String::from_utf8_lossy(&bytes)) {
                    issue(
                        PathBuf::from(rel),
                        0,
                        format!(
                            "each_file_must_contain: паттерн '{pattern}' не найден в файле {rel}"
                        ),
                    );
                }
            }
        }
        RuleKind::FileExists => {
            let rel = rule.path.as_deref().ok_or_else(|| {
                HarnessError::Control(format!(
                    "правило '{}': для file_exists нужен path",
                    rule.name
                ))
            })?;
            if !repo.join(rel).exists() {
                issue(
                    PathBuf::from(rel),
                    0,
                    format!("file_exists: файл не найден: {rel}"),
                );
            }
        }
        RuleKind::DirMustHaveFile => {
            let rel_path = rule.path.as_deref().ok_or_else(|| {
                HarnessError::Control(format!(
                    "правило '{}': для dir_must_have_file нужен path",
                    rule.name
                ))
            })?;
            let globs = rule_globs(rule);
            let mut dirs = Vec::new();
            for glob in &globs {
                dirs.extend(collect_dirs(repo, glob)?);
            }
            dirs.sort();
            dirs.dedup();
            apply_excludes(&mut dirs, &rule.exclude_glob);
            if dirs.is_empty() {
                issue(
                    PathBuf::from(globs.join(", ")),
                    0,
                    format!(
                        "dir_must_have_file: по glob '{}' не найдено ни одного каталога — правилу нечего проверять",
                        globs.join(", ")
                    ),
                );
            }
            for dir in &dirs {
                if !repo.join(dir).join(rel_path).exists() {
                    issue(
                        PathBuf::from(dir),
                        0,
                        format!(
                            "dir_must_have_file: в каталоге {dir} не найден обязательный файл {rel_path}"
                        ),
                    );
                }
            }
        }
        RuleKind::MaxAge => {
            let rel = rule.path.as_deref().ok_or_else(|| {
                HarnessError::Control(format!("правило '{}': для max_age нужен path", rule.name))
            })?;
            let max_age_days = rule.max_age_days.ok_or_else(|| {
                HarnessError::Control(format!(
                    "правило '{}': для max_age нужен max_age_days",
                    rule.name
                ))
            })?;
            if let Some(message) = check_max_age(repo, rel, max_age_days, SystemTime::now())? {
                issue(PathBuf::from(rel), 0, message);
            }
        }
        RuleKind::CommandSucceeds => {
            let cmd = rule.command.as_deref().ok_or_else(|| {
                HarnessError::Control(format!(
                    "правило '{}': для command_succeeds нужен command",
                    rule.name
                ))
            })?;
            // A3: исполнение запрещено моделью доверия (no-exec или allow-файл
            // не совпадает с реестром) — команда НЕ запускается: пропуск
            // `command_untrusted` с подсказкой, как разрешить. Проверка
            // доверия — ДО проверки прогонщика (A2): под запретом и детект
            // окружения не нужен.
            if let crate::cmd_trust::ExecDecision::Deny(reason) = exec {
                untrusted_skipped.push(UntrustedSkippedRule {
                    rule: rule.name.clone(),
                    severity: severity.to_string(),
                    reason: crate::cmd_trust::deny_reason_text(reason),
                });
                return Ok(());
            }
            // A2: команда требует внешний прогонщик (pytest/mvn/JDK), которого
            // нет в PATH, — пропуск с причиной, а не провал. Падение прогона
            // (`No module named pytest`) читалось бы как нарушение правила,
            // хотя дефект — в окружении, а не в коде репозитория.
            let missing = crate::rule_templates::missing_runners(cmd, runner);
            if !missing.is_empty() {
                runner_skipped.push(RunnerSkippedRule {
                    rule: rule.name.clone(),
                    severity: severity.to_string(),
                    runners: missing
                        .iter()
                        .map(|kind| kind.label().to_string())
                        .collect(),
                    reason: crate::rule_templates::runners_absent_reason(&missing),
                });
                return Ok(());
            }
            let timeout_secs = rule.timeout_secs.unwrap_or(COMMAND_TIMEOUT_DEFAULT_SECS);
            match run_with_timeout(repo, cmd, Duration::from_secs(timeout_secs))? {
                outcome if outcome.status.is_some_and(|s| s.success()) => {}
                outcome if outcome.status.is_some() => {
                    let code = outcome
                        .status
                        .and_then(|s| s.code())
                        .map_or_else(|| "завершена сигналом".to_string(), |c| format!("код {c}"));
                    let tail = report_tail(&outcome.tail);
                    let detail = if tail.is_empty() {
                        String::new()
                    } else {
                        format!("; хвост вывода:\n{tail}")
                    };
                    issue(
                        repo.to_path_buf(),
                        0,
                        format!(
                            "command_succeeds: команда '{cmd}' завершилась неуспешно ({code}){detail}"
                        ),
                    );
                }
                _ => {
                    issue(
                        repo.to_path_buf(),
                        0,
                        format!(
                            "command_succeeds: команда '{cmd}' превысила таймаут {timeout_secs}s и была убита"
                        ),
                    );
                }
            }
        }
        RuleKind::DependencyDirection => {
            let mode = deps_mode(rule)?;
            let globs = rule_globs(rule);
            let mut files = Vec::new();
            for glob in &globs {
                files.extend(collect_files(repo, glob)?);
            }
            files.sort_by(|a, b| a.0.cmp(&b.0));
            files.dedup_by(|a, b| a.0 == b.0);
            if !rule.exclude_glob.is_empty() {
                files.retain(|(rel, _)| !rule.exclude_glob.iter().any(|ex| glob_matches(ex, rel)));
            }
            if files.is_empty() {
                issue(
                    PathBuf::from(globs.join(", ")),
                    0,
                    format!(
                        "dependency_direction: по glob '{}' не найдено ни одного файла — правилу нечего проверять",
                        globs.join(", ")
                    ),
                );
            }
            for (rel, abs) in &files {
                let bytes = std::fs::read(abs).map_err(|e| HarnessError::io(abs, e))?;
                let content = String::from_utf8_lossy(&bytes);
                for (module, line) in extract_imports(rel, &content)? {
                    // Относительные импорты TS/JS (`./…`) не выражаются в
                    // координатах модулей — их разрешает `context_boundary`.
                    if module.starts_with('.') {
                        continue;
                    }
                    match &mode {
                        DepsMode::Forbid(forbid) => {
                            if let Some(entry) =
                                forbid.iter().find(|e| module_prefix_match(&module, e))
                            {
                                issue(
                                    PathBuf::from(rel),
                                    line,
                                    format!(
                                        "dependency_direction: запрещённая зависимость '{module}' (forbid: '{entry}')"
                                    ),
                                );
                            }
                        }
                        DepsMode::Allow(allow) => {
                            if !allow.iter().any(|e| module_prefix_match(&module, e)) {
                                issue(
                                    PathBuf::from(rel),
                                    line,
                                    format!(
                                        "dependency_direction: зависимость '{module}' вне allow-списка ({})",
                                        allow.join(", ")
                                    ),
                                );
                            }
                        }
                    }
                }
            }
        }
        RuleKind::ContextBoundary => {
            let model_dir = rule.model_dir.as_deref().unwrap_or("model");
            if !repo.join(model_dir).is_dir() {
                issue(
                    PathBuf::from(model_dir),
                    0,
                    format!("context_boundary: каталог модели не найден: {model_dir}"),
                );
                return Ok(());
            }
            let model = crate::model::load_model(&repo.join(model_dir))?;
            check_context_boundary(rule, repo, &model, model_dir, &mut issue)?;
        }
        RuleKind::ArchUnit => {
            // Общий код с `arch-be archunit check` (ADR-039): спек строится
            // из java-правил этого же CONSTRAINTS.yaml. Инфраструктурный
            // сбой (нет java/jar'ов/классов, таймаут) — error-находка
            // (fail-closed), а не молчаливый PASS.
            match crate::archunit::run_control_rule(rule, all_rules, repo) {
                Ok(found) => {
                    // Находки JVM-гейта ссылаются на исходное java-правило по
                    // id (либо имени) — обогащаем их карточкой того правила.
                    for mut finding in found {
                        if let Some(source) = all_rules.iter().find(|r| {
                            r.id.as_deref() == Some(finding.rule.as_str()) || r.name == finding.rule
                        }) {
                            source.apply_card(&mut finding);
                        }
                        issues.push(finding);
                    }
                }
                Err(e) => issue(
                    repo.to_path_buf(),
                    0,
                    format!("archunit: гейт не исполнен (fail-closed): {e}"),
                ),
            }
        }
        RuleKind::DenyDependency => {
            if rule.deny.is_empty() {
                return Err(HarnessError::Control(format!(
                    "правило '{}': для deny_dependency нужен непустой список deny",
                    rule.name
                )));
            }
            let globs = if rule.manifests.is_empty() {
                DEFAULT_MANIFEST_GLOBS
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
            } else {
                rule.manifests.clone()
            };
            let mut files = Vec::new();
            for glob in &globs {
                files.extend(collect_files(repo, glob)?);
            }
            files.sort_by(|a, b| a.0.cmp(&b.0));
            files.dedup_by(|a, b| a.0 == b.0);
            if !rule.exclude_glob.is_empty() {
                files.retain(|(rel, _)| !rule.exclude_glob.iter().any(|ex| glob_matches(ex, rel)));
            }
            let reference = rule
                .reference
                .as_deref()
                .map(|r| format!(" — основание: {r}"))
                .unwrap_or_default();
            for (rel, abs) in &files {
                let bytes = std::fs::read(abs).map_err(|e| HarnessError::io(abs, e))?;
                let content = String::from_utf8_lossy(&bytes);
                for (pkg, line) in manifest_deps(rel, &content)? {
                    if rule.deny.iter().any(|d| d == &pkg) {
                        issue(
                            PathBuf::from(rel),
                            line,
                            format!("deny_dependency: пакет '{pkg}' из deny-списка{reference}"),
                        );
                    }
                }
            }
        }
    }
    Ok(())
}

/// Проверяет свежесть файла для правила `max_age`:
/// PASS ⇔ файл существует И его mtime не старше `now − max_age_days`.
///
/// `now` инъецируется параметром ради тестируемости: std не умеет выставлять
/// mtime, `unsafe` запрещён, новых зависимостей не добавляем — юнит-тесты
/// сдвигают `now`, а не время файла. Публичный вызов передаёт
/// [`SystemTime::now`].
///
/// Возвращает `Ok(None)` при прохождении; `Ok(Some(message))` при нарушении:
/// файл отсутствует (текст как у `file_exists`) либо устарел (возраст в днях
/// против лимита).
fn check_max_age(
    repo: &Path,
    rel: &str,
    max_age_days: u64,
    now: SystemTime,
) -> Result<Option<String>> {
    /// Секунд в сутках (перевод возраста файла в дни).
    const DAY_SECS: u64 = 86_400;
    let abs = repo.join(rel);
    if !abs.exists() {
        return Ok(Some(format!("file_exists: файл не найден: {rel}")));
    }
    let meta = std::fs::metadata(&abs).map_err(|e| HarnessError::io(&abs, e))?;
    let mtime = meta.modified().map_err(|e| HarnessError::io(&abs, e))?;
    // mtime в будущем (дрейф часов) — файл считаем свежим, возраст 0.
    let age = now.duration_since(mtime).unwrap_or(Duration::ZERO);
    // saturating_mul: абсурдно большой max_age_days не паникует, а просто
    // делает лимит практически бесконечным.
    let limit = Duration::from_secs(max_age_days.saturating_mul(DAY_SECS));
    if age > limit {
        let age_days = age.as_secs() / DAY_SECS;
        return Ok(Some(format!(
            "max_age: файл {rel} устарел: возраст {age_days} дн. при лимите {max_age_days} дн."
        )));
    }
    Ok(None)
}

/// Режим правила `dependency_direction`: чёрный или белый список модулей.
enum DepsMode<'a> {
    /// Запрещённые префиксы модульных путей.
    Forbid(&'a [String]),
    /// Разрешённые префиксы (пустой список — запрет всех внутренних
    /// зависимостей: листовые модули).
    Allow(&'a [String]),
}

/// Валидирует и возвращает режим `dependency_direction`: ровно одно из
/// `forbid`/`allow`.
///
/// # Errors
/// Оба списка заданы или оба отсутствуют.
fn deps_mode(rule: &FitnessRule) -> Result<DepsMode<'_>> {
    match (&rule.forbid, &rule.allow) {
        (Some(forbid), None) => Ok(DepsMode::Forbid(forbid)),
        (None, Some(allow)) => Ok(DepsMode::Allow(allow)),
        _ => Err(HarnessError::Control(format!(
            "правило '{}': для dependency_direction нужно ровно одно из forbid/allow",
            rule.name
        ))),
    }
}

/// Совпадение модуля с записью списка по префиксу пути с границей сегмента:
/// `agent/slash` совпадает с `agent`, `agentworld` — нет.
fn module_prefix_match(module: &str, entry: &str) -> bool {
    module == entry
        || module
            .strip_prefix(entry)
            .is_some_and(|rest| rest.starts_with('/'))
}

/// Извлечённый импорт: модуль в координатах `/` и номер строки (1-based).
type ImportEdge = (String, usize);

/// Извлекает импорты из исходного файла по его расширению (ADR-029).
///
/// Поддерживаемые формы:
/// - Rust (`.rs`): `use crate::…` и инлайн-пути `crate::…::` (`::` → `/`);
///   внешние крейты (`use std::…`) не извлекаются;
/// - Python (`.py`): `import a.b`, `from a.b import …` (`.` → `/`);
/// - Java/Kotlin (`.java`, `.kt`): `import a.b.C;` (`.` → `/`);
/// - TS/JS (`.ts`, `.tsx`, `.js`, `.jsx`, `.mjs`): `from '…'`, `import '…'`,
///   `require('…')`; относительные пути (`./…`) сохраняются как есть —
///   их разрешает `context_boundary`, а `dependency_direction` пропускает.
///
/// Строки-комментарии (`//`, `///`, `//!`, `#`) игнорируются; блочные
/// комментарии и строковые литералы не разбираются — эвристика
/// документированно приблизительна (ложное срабатывание возможно на
/// `crate::…` внутри строки). Для файлов неподдерживаемых расширений
/// возвращается пустой список.
///
/// # Errors
/// Внутренний regex не компилируется (инвариант кода; практически
/// недостижимо — паттерны константны).
fn extract_imports(rel: &str, content: &str) -> Result<Vec<ImportEdge>> {
    let ext = Path::new(rel)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default();
    let comment_prefix = match ext {
        "rs" | "java" | "kt" | "ts" | "tsx" | "js" | "jsx" | "mjs" => "//",
        "py" => "#",
        _ => return Ok(Vec::new()),
    };
    let compile = |pat: &str| {
        Regex::new(pat)
            .map_err(|e| HarnessError::Control(format!("внутренний regex импортов '{pat}': {e}")))
    };
    let mut out = Vec::new();
    for (idx, line) in content.lines().enumerate() {
        if line.trim_start().starts_with(comment_prefix) {
            continue;
        }
        let lineno = idx + 1;
        match ext {
            "rs" => {
                let re =
                    compile(r"\bcrate::([A-Za-z_][A-Za-z0-9_]*(?:::[A-Za-z_][A-Za-z0-9_]*)*)")?;
                out.extend(
                    re.captures_iter(line)
                        .map(|c| (c[1].replace("::", "/"), lineno)),
                );
            }
            "py" => {
                let re_import = compile(r"^\s*import\s+([A-Za-z_][\w.]*)")?;
                let re_from = compile(r"^\s*from\s+([A-Za-z_][\w.]*)\s+import\b")?;
                out.extend(
                    re_import
                        .captures(line)
                        .into_iter()
                        .chain(re_from.captures(line))
                        .map(|c| (c[1].replace('.', "/"), lineno)),
                );
            }
            "java" | "kt" => {
                let re = compile(r"^\s*import\s+(?:static\s+)?([A-Za-z_][\w.]*)\s*;")?;
                if let Some(c) = re.captures(line) {
                    out.push((c[1].replace('.', "/"), lineno));
                }
            }
            _ => {
                // ts/tsx/js/jsx/mjs: путь сохраняется сырым (включая `./…`).
                let re_from = compile(r#"\bfrom\s+['"]([^'"]+)['"]"#)?;
                let re_import = compile(r#"^\s*import\s+['"]([^'"]+)['"]"#)?;
                let re_require = compile(r#"\brequire\(\s*['"]([^'"]+)['"]\s*\)"#)?;
                out.extend(
                    re_from
                        .captures_iter(line)
                        .chain(re_import.captures_iter(line))
                        .chain(re_require.captures_iter(line))
                        .map(|c| (c[1].to_string(), lineno)),
                );
            }
        }
    }
    Ok(out)
}

/// Проверка `context_boundary` (ADR-030): импорты файлов не пересекают
/// границы контекстов (CMP с `code_roots`) без объявленного `depends_on`.
///
/// Контекст файла определяется по префиксу пути среди `code_roots`;
/// контекст импорта — сначала прямым префиксным совпадением в координатах
/// импортов (Python/Java: путь пакета == путь файла), затем разрешением
/// модуля в существующий файл (Rust `crate::…` против `src/`, TS-относительные
/// против каталога файла). Неразрешённый импорт — внешняя зависимость,
/// пропускается. Пересекающиеся `code_roots` разных CMP — ошибка
/// конфигурации правила.
fn check_context_boundary(
    rule: &FitnessRule,
    repo: &Path,
    model: &crate::model::Model,
    model_dir: &str,
    issue: &mut impl FnMut(PathBuf, usize, String),
) -> Result<()> {
    let contexts: Vec<(&crate::model::Entity, Vec<String>)> = model
        .entities
        .iter()
        .filter(|e| e.kind == crate::model::EntityKind::Cmp && !e.code_roots.is_empty())
        .map(|e| (e, e.code_roots.iter().map(|r| normalize_root(r)).collect()))
        .collect();
    if contexts.is_empty() {
        issue(
            PathBuf::from(model_dir),
            0,
            "context_boundary: в модели нет CMP с code_roots — правилу нечего проверять"
                .to_string(),
        );
        return Ok(());
    }
    // Пересечение корней двух CMP делает владение файлом неоднозначным.
    for (i, (a, roots_a)) in contexts.iter().enumerate() {
        for (b, roots_b) in &contexts[i + 1..] {
            for ra in roots_a {
                for rb in roots_b {
                    if module_prefix_match(ra, rb) || module_prefix_match(rb, ra) {
                        return Err(HarnessError::Control(format!(
                            "правило '{}': code_roots пересекаются: {} ('{ra}') и {} ('{rb}')",
                            rule.name, a.id, b.id
                        )));
                    }
                }
            }
        }
    }
    let bases = candidate_bases(repo);
    let globs = rule_globs(rule);
    let mut files = Vec::new();
    for glob in &globs {
        files.extend(collect_files(repo, glob)?);
    }
    files.sort_by(|a, b| a.0.cmp(&b.0));
    files.dedup_by(|a, b| a.0 == b.0);
    if !rule.exclude_glob.is_empty() {
        files.retain(|(rel, _)| !rule.exclude_glob.iter().any(|ex| glob_matches(ex, rel)));
    }
    for (rel, abs) in &files {
        let Some(source) = owner_by_path(&contexts, rel) else {
            continue;
        };
        let bytes = std::fs::read(abs).map_err(|e| HarnessError::io(abs, e))?;
        let content = String::from_utf8_lossy(&bytes);
        for (module, line) in extract_imports(rel, &content)? {
            let Some(target) = resolve_import_owner(repo, &contexts, &bases, rel, &module) else {
                continue;
            };
            if target.id == source.id || source.depends_on.contains(&target.id) {
                continue;
            }
            issue(
                PathBuf::from(rel),
                line,
                format!(
                    "context_boundary: импорт '{module}' пересекает границу контекста: {} → {} ({}) без depends_on в модели",
                    source.id, target.id, target.title
                ),
            );
        }
    }
    Ok(())
}

/// Контекст (CMP), которому принадлежит путь `path` по префиксу `code_roots`.
fn owner_by_path<'m>(
    contexts: &[(&'m crate::model::Entity, Vec<String>)],
    path: &str,
) -> Option<&'m crate::model::Entity> {
    contexts
        .iter()
        .find(|(_, roots)| roots.iter().any(|r| module_prefix_match(path, r)))
        .map(|(e, _)| *e)
}

/// Разрешает импорт `module` (координаты `/`) во владеющий им контекст.
///
/// Порядок: TS-относительный путь — от каталога файла; прямое префиксное
/// совпадение с `code_roots`; разрешение в существующий файл репозитория
/// (базы `candidate_bases` + модульные суффиксы). `None` — внешняя или
/// неразрешённая зависимость.
fn resolve_import_owner<'m>(
    repo: &Path,
    contexts: &[(&'m crate::model::Entity, Vec<String>)],
    bases: &[String],
    from_rel: &str,
    module: &str,
) -> Option<&'m crate::model::Entity> {
    if let Some(stripped) = module.strip_prefix('.') {
        // TS/JS-относительный импорт: `./foo`, `../bar` — от каталога файла.
        let dir = Path::new(from_rel)
            .parent()
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .unwrap_or_default();
        let resolved = normalize_rel_path(&format!("{dir}/{stripped}"));
        return owner_by_path(contexts, &resolved);
    }
    // Прямое совпадение в координатах импортов (Python/Java-пакеты).
    if let Some(owner) = owner_by_path(contexts, module) {
        return Some(owner);
    }
    // Разрешение модуля в файл (Rust `crate::…`, смешанные монорепо).
    // Путь импорта включает имя элемента (`beta/core/Engine`), поэтому
    // пробуем префиксы от длинного к короткому: `beta/core/Engine` →
    // `beta/core` → `beta`.
    let segments: Vec<&str> = module.split('/').collect();
    for len in (1..=segments.len()).rev() {
        let prefix = segments[..len].join("/");
        for base in bases {
            for cand in candidate_paths(base, &prefix) {
                if repo.join(&cand).is_file() {
                    return owner_by_path(contexts, &cand);
                }
            }
        }
    }
    None
}

/// Базовые каталоги для разрешения модуля в файл: корень, типовые корни
/// исходников и крейты Rust-workspace (`crates/*`).
fn candidate_bases(repo: &Path) -> Vec<String> {
    let mut bases = vec![
        String::new(),
        "src/".to_string(),
        "src/main/java/".to_string(),
        "src/main/kotlin/".to_string(),
    ];
    // Отсутствующий `crates/` — не ошибка, просто нет дополнительных баз.
    if let Ok(rd) = std::fs::read_dir(repo.join("crates")) {
        for entry in rd.flatten() {
            if entry.path().is_dir() {
                bases.push(format!("crates/{}/", entry.file_name().to_string_lossy()));
            }
        }
    }
    bases
}

/// Кандидатные пути файла для модуля `module` под базой `base`.
fn candidate_paths(base: &str, module: &str) -> Vec<String> {
    [
        "{m}.rs",
        "{m}/mod.rs",
        "{m}.py",
        "{m}/__init__.py",
        "{m}.java",
        "{m}.kt",
        "{m}.ts",
        "{m}/index.ts",
        "{m}.js",
        "{m}/index.js",
    ]
    .iter()
    .map(|pat| format!("{base}{}", pat.replace("{m}", module)))
    .collect()
}

/// Нормализует `code_root`: срезает пробелы, ведущий `./` и хвостовые `/`.
fn normalize_root(root: &str) -> String {
    root.trim()
        .trim_start_matches("./")
        .trim_end_matches('/')
        .to_string()
}

/// Нормализует относительный путь: раскрывает `.` и `..` по сегментам.
fn normalize_rel_path(path: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    for seg in path.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                out.pop();
            }
            s => out.push(s),
        }
    }
    out.join("/")
}

/// Подготовленное content-правило: скомпилированный regex, glob-шаблон
/// и набор файлов (относительный путь, абсолютный путь).
type PreparedContentRule = (Regex, String, Vec<(String, PathBuf)>);

/// Общая подготовка content-правил: компилированный regex, glob, набор файлов.
fn prep_content_rule(rule: &FitnessRule, repo: &Path) -> Result<PreparedContentRule> {
    let pattern = rule.pattern.as_deref().ok_or_else(|| {
        HarnessError::Control(format!(
            "правило '{}': для {:?} нужен pattern",
            rule.name, rule.kind
        ))
    })?;
    let re = Regex::new(pattern).map_err(|e| {
        HarnessError::Control(format!(
            "правило '{}': невалидный regex '{pattern}': {e}",
            rule.name
        ))
    })?;
    let globs = rule_globs(rule);
    let mut files = Vec::new();
    for glob in &globs {
        files.extend(collect_files(repo, glob)?);
    }
    files.sort_by(|a, b| a.0.cmp(&b.0));
    files.dedup_by(|a, b| a.0 == b.0);
    if !rule.exclude_glob.is_empty() {
        files.retain(|(rel, _)| !rule.exclude_glob.iter().any(|ex| glob_matches(ex, rel)));
    }
    Ok((re, globs.join(", "), files))
}

/// Срез `--changed-since` для файловых правил (модуль [`baseline`]):
/// оставляет только изменённые файлы. Пустой срез — НЕ находка (в отличие от
/// пустого полного набора у `each_file_must_contain`), а пропуск правила с
/// пометкой в отчёте: нетронутые файлы проверит полный прогон.
///
/// `None` у `changed` — полный прогон: набор возвращается как есть.
fn scope_files(
    rule: &FitnessRule,
    changed: Option<&BTreeSet<String>>,
    mut files: Vec<(String, PathBuf)>,
    glob: &str,
    skipped: &mut Vec<baseline::SkippedRule>,
) -> Option<Vec<(String, PathBuf)>> {
    let Some(changed) = changed else {
        return Some(files);
    };
    files.retain(|(rel, _)| changed.contains(rel));
    if files.is_empty() {
        skipped.push(baseline::SkippedRule {
            rule: rule.name.clone(),
            reason: format!("нет изменённых файлов по glob '{glob}'"),
        });
        return None;
    }
    Some(files)
}

/// Glob'ы правила с дефолтом `**/*`.
fn rule_globs(rule: &FitnessRule) -> Vec<String> {
    if rule.glob.is_empty() {
        vec!["**/*".to_string()]
    } else {
        rule.glob.clone()
    }
}

/// Собирает файлы репозитория по простому glob-шаблону (`**` — любая глубина,
/// `*` — внутри сегмента, `?` — один символ). Возвращает (относительный путь,
/// абсолютный путь), отсортированные по относительному пути.
///
/// Служебные и производные каталоги исключены всегда: `.git`, `target`,
/// `node_modules`, `dist`, `__pycache__`, `.next`, `.pytest_cache` и
/// `.arch-handoff` — fitness-правила целятся в АРТЕФАКТЫ РЕАЛИЗАЦИИ, а не в
/// документы решения: пакет handoff содержит текст spine/TASK.md, и правило
/// `must_not_contain` срабатывало на собственные цитаты контракта (кейс 1).
fn collect_files(repo: &Path, glob: &str) -> Result<Vec<(String, PathBuf)>> {
    const SKIP: [&str; 8] = [
        ".git",
        "target",
        "node_modules",
        "dist",
        "__pycache__",
        ".next",
        ".pytest_cache",
        ".arch-handoff",
    ];
    let mut out = Vec::new();
    let walker = WalkDir::new(repo).follow_links(false).into_iter();
    for entry in walker.filter_entry(|e| {
        let name = e.file_name().to_string_lossy();
        !(e.file_type().is_dir() && SKIP.contains(&name.as_ref()))
    }) {
        let entry = entry.map_err(|e| {
            HarnessError::Control(format!("обход репозитория {}: {e}", repo.display()))
        })?;
        if !entry.file_type().is_file() {
            continue;
        }
        let rel = entry.path().strip_prefix(repo).map_err(|e| {
            HarnessError::Control(format!(
                "относительный путь {}: {e}",
                entry.path().display()
            ))
        })?;
        let rel_str = rel.to_string_lossy().replace('\\', "/");
        if glob_matches(glob, &rel_str) {
            out.push((rel_str, entry.path().to_path_buf()));
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

/// Собирает КАТАЛОГИ репозитория по glob-шаблону (для `dir_must_have_file`).
/// Возвращает относительные пути каталогов (с `/`-разделителями),
/// отсортированные. Служебные каталоги исключены тем же списком, что и в
/// [`collect_files`]; корень репозитория в выборку не входит.
fn collect_dirs(repo: &Path, glob: &str) -> Result<Vec<String>> {
    const SKIP: [&str; 8] = [
        ".git",
        "target",
        "node_modules",
        "dist",
        "__pycache__",
        ".next",
        ".pytest_cache",
        ".arch-handoff",
    ];
    let mut out = Vec::new();
    let walker = WalkDir::new(repo).follow_links(false).into_iter();
    for entry in walker.filter_entry(|e| {
        let name = e.file_name().to_string_lossy();
        !(e.file_type().is_dir() && SKIP.contains(&name.as_ref()))
    }) {
        let entry = entry.map_err(|e| {
            HarnessError::Control(format!("обход репозитория {}: {e}", repo.display()))
        })?;
        if !entry.file_type().is_dir() || entry.path() == repo {
            continue;
        }
        let rel = entry.path().strip_prefix(repo).map_err(|e| {
            HarnessError::Control(format!(
                "относительный путь {}: {e}",
                entry.path().display()
            ))
        })?;
        let rel_str = rel.to_string_lossy().replace('\\', "/");
        if glob_matches(glob, &rel_str) {
            out.push(rel_str);
        }
    }
    out.sort();
    Ok(out)
}

/// Матч одного сегмента пути по glob-шаблону (`*` — любые символы, `?` — один).
fn segment_matches(pattern: &str, name: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let n: Vec<char> = name.chars().collect();
    let (mut pi, mut ni) = (0usize, 0usize);
    let mut star: Option<(usize, usize)> = None; // (позиция '*' в шаблоне, позиция в имени)
    while ni < n.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == n[ni]) {
            pi += 1;
            ni += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some((pi, ni));
            pi += 1;
        } else if let Some((sp, sn)) = star {
            pi = sp + 1;
            ni = sn + 1;
            star = Some((sp, sn + 1));
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

/// Матч относительного пути по glob-шаблону с поддержкой `**` (любая глубина,
/// включая ноль сегментов: `**/*.rs` матчит и `main.rs`).
///
/// `pub(crate)`: разделяется с аудитом флота (`crate::fleet`, флаг `--include`).
pub(crate) fn glob_matches(pattern: &str, path: &str) -> bool {
    let pat: Vec<&str> = pattern.split('/').collect();
    let parts: Vec<&str> = path.split('/').collect();
    match_glob_segments(&pat, &parts)
}

fn match_glob_segments(pat: &[&str], parts: &[&str]) -> bool {
    if pat.is_empty() {
        return parts.is_empty();
    }
    if pat[0] == "**" {
        return (0..=parts.len()).any(|skip| match_glob_segments(&pat[1..], &parts[skip..]));
    }
    if parts.is_empty() {
        return false;
    }
    segment_matches(pat[0], parts[0]) && match_glob_segments(&pat[1..], &parts[1..])
}

/// Запускает `bash -c <command>` в `repo` с ручным таймаутом.
/// `Ok(None)` в статусе — команда превысила таймаут и была убита.
/// Механика (процессная группа, читатели с дедлайном) — [`crate::proc`]:
/// таймаут убивает группу целиком, внуки не держат гейт (A1).
fn run_with_timeout(repo: &Path, command: &str, timeout: Duration) -> Result<CommandOutcome> {
    let outcome = crate::proc::run_shell(repo, "bash", command, timeout)?;
    let mut captured = outcome.stdout;
    captured.extend_from_slice(&outcome.stderr);
    Ok(CommandOutcome {
        status: outcome.status,
        tail: String::from_utf8_lossy(&captured).into_owned(),
    })
}

/// Итог прогона `command_succeeds`-команды: статус и хвост вывода для отчёта.
struct CommandOutcome {
    /// `Some`, если команда завершилась сама (иначе — убита по таймауту).
    status: Option<ExitStatus>,
    /// Последние байты stdout+stderr (обрезаны до [`crate::proc::MAX_CAPTURE_BYTES`]).
    tail: String,
}

/// Сколько последних строк хвоста включаем в отчёт об упавшей команде.
const REPORT_TAIL_LINES: usize = 15;

/// Хвост вывода упавшей команды для отчёта: последние [`REPORT_TAIL_LINES`]
/// строк, пропущенные через редактор секретов (AD-3: вывод может содержать
/// значения переменных окружения и токены).
fn report_tail(raw: &str) -> String {
    let redacted = crate::secrets::Redactor::new(crate::secrets::builtin_rules()).redact(raw);
    let lines: Vec<&str> = redacted.lines().collect();
    let skip = lines.len().saturating_sub(REPORT_TAIL_LINES);
    lines[skip..].join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::load_fitness_rules_with_skips;

    /// Пишет файл в каталог и возвращает его путь.
    fn write_file(dir: &Path, name: &str, content: &str) -> PathBuf {
        let p = dir.join(name);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&p, content).unwrap();
        p
    }
    #[test]
    fn fitness_all_rule_types() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        write_file(&repo, "src/main.rs", "fn main() { println!(\"hi\"); }\n");
        write_file(
            &repo,
            "src/lib.rs",
            "pub fn f() {}\n// unsafe тут запрещён\n",
        );
        write_file(&repo, "Cargo.toml", "[package]\nname = \"x\"\n");
        let constraints = write_file(
            dir.path(),
            "CONSTRAINTS.yaml",
            "rules:\n\
             \x20 - name: has_main\n\
             \x20   type: must_contain\n\
             \x20   glob: '**/*.rs'\n\
             \x20   pattern: 'fn main'\n\
             \x20 - name: no_such_token\n\
             \x20   type: must_contain\n\
             \x20   glob: '**/*.rs'\n\
             \x20   pattern: 'zzqwxv_never'\n\
             \x20 - name: no_unsafe\n\
             \x20   type: must_not_contain\n\
             \x20   glob: '**/*.rs'\n\
             \x20   pattern: '\\bunsafe\\b'\n\
             \x20   severity: warn\n\
             \x20 - name: cargo_toml_exists\n\
             \x20   type: file_exists\n\
             \x20   path: Cargo.toml\n\
             \x20 - name: readme_exists\n\
             \x20   type: file_exists\n\
             \x20   path: README.md\n\
             \x20 - name: cmd_true\n\
             \x20   type: command_succeeds\n\
             \x20   command: 'true'\n\
             \x20 - name: cmd_false\n\
             \x20   type: command_succeeds\n\
             \x20   command: 'false'\n",
        );
        let report = check(&repo, &constraints).unwrap();
        assert!(
            report
                .summary
                .starts_with("Правил: 7, нарушений: 4 (error: 3, warn: 1)"),
            "{}",
            report.summary
        );
        assert!(!report.passed);
        let by_rule = |name: &str| report.issues.iter().find(|i| i.rule == name).unwrap();
        assert_eq!(by_rule("no_such_token").line, 0);
        assert_eq!(by_rule("no_such_token").severity, "error");
        let unsafe_issue = by_rule("no_unsafe");
        assert_eq!(unsafe_issue.severity, "warn");
        assert_eq!(unsafe_issue.line, 2, "unsafe на второй строке lib.rs");
        assert!(unsafe_issue.file.ends_with("src/lib.rs"));
        assert_eq!(by_rule("readme_exists").severity, "error");
        assert!(by_rule("cmd_false").message.contains("неуспешно"));
        assert!(report.issues.iter().all(|i| i.rule != "has_main"
            && i.rule != "cargo_toml_exists"
            && i.rule != "cmd_true"));
    }

    #[test]
    fn fitness_report_json_matches_sdk_contract_v1() {
        // SDK-контракт v1 (sdk/CONTRACT.md §2): однострочный JSON FitnessReport
        // с ключами repo/passed/summary/issues и полями находки
        // file/line/rule/message/severity.
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        write_file(&repo, "src/main.py", "pan = \"4276550012345678\"\n");
        let constraints = write_file(
            dir.path(),
            "CONSTRAINTS.yaml",
            "constraints:\n\
             \x20 - id: BANK-01\n\
             \x20   name: no_pan\n\
             \x20   type: must_not_contain\n\
             \x20   glob: 'src/**/*.py'\n\
             \x20   pattern: '\\b\\d{16}\\b'\n\
             \x20   severity: critical\n",
        );
        let report = check(&repo, &constraints).unwrap();
        let v: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&report).unwrap()).unwrap();
        assert_eq!(v["passed"], false);
        assert!(v["repo"].is_string() && v["summary"].is_string());
        let issue = &v["issues"][0];
        for key in ["file", "line", "rule", "message", "severity"] {
            assert!(issue.get(key).is_some(), "нет ключа {key} в issue");
        }
        assert_eq!(issue["rule"], "no_pan");
        assert_eq!(issue["severity"], "error");
        assert_eq!(issue["line"], 1);
    }

    #[test]
    fn fitness_issue_carries_rule_card_context() {
        // Находки несут архитектурный контекст карточки правила (ad/adr/
        // rationale/owner/fix_hint/skill): видно задетый инвариант и скилл
        // исправления, а не только имя правила. Контракт аддитивный: у
        // правила без карточки полей в JSON нет (skip_serializing_if).
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        write_file(&repo, "src/main.rs", "fn main() {}\n");
        let constraints = write_file(
            dir.path(),
            "CONSTRAINTS.yaml",
            "rules:\n\
             \x20 - name: no_unsafe\n\
             \x20   type: must_not_contain\n\
             \x20   glob: 'src/**/*.rs'\n\
             \x20   pattern: 'unsafe'\n\
             \x20   ad: AD-6\n\
             \x20   adr: ADR-012\n\
             \x20   rationale: безопасный Rust без unsafe\n\
             \x20   owner: архитектор контура\n\
             \x20   fix_hint: убрать unsafe-блок\n\
             \x20   skill: fitness-functions\n\
             \x20 - name: bare_rule\n\
             \x20   type: must_contain\n\
             \x20   glob: 'src/**/*.rs'\n\
             \x20   pattern: 'never-found-marker'\n",
        );
        // Нарушение собирается конкатенацией строк: цельный литерал в
        // исходнике теста сам попал бы под догфуд-правило C-01 (no_unsafe_code).
        write_file(&repo, "src/lib.rs", concat!("un", "safe fn f() {}\n"));
        let report = check(&repo, &constraints).unwrap();
        assert!(!report.passed);
        let issue = report
            .issues
            .iter()
            .find(|i| i.rule == "no_unsafe")
            .expect("находка no_unsafe");
        assert_eq!(issue.ad.as_deref(), Some("AD-6"));
        assert_eq!(issue.adr.as_deref(), Some("ADR-012"));
        assert_eq!(
            issue.rationale.as_deref(),
            Some("безопасный Rust без unsafe")
        );
        assert_eq!(issue.owner.as_deref(), Some("архитектор контура"));
        assert_eq!(issue.fix_hint.as_deref(), Some("убрать unsafe-блок"));
        assert_eq!(issue.skill.as_deref(), Some("fitness-functions"));

        let v: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&report).unwrap()).unwrap();
        let issues = v["issues"].as_array().expect("issues");
        let with_card = issues
            .iter()
            .find(|i| i["rule"] == "no_unsafe")
            .expect("no_unsafe в JSON");
        assert_eq!(with_card["ad"], "AD-6");
        assert_eq!(with_card["skill"], "fitness-functions");
        assert_eq!(with_card["fix_hint"], "убрать unsafe-блок");
        let bare = issues
            .iter()
            .find(|i| i["rule"] == "bare_rule")
            .expect("bare_rule в JSON");
        for key in ["ad", "adr", "rationale", "owner", "fix_hint", "skill"] {
            assert!(bare.get(key).is_none(), "у находки без карточки нет {key}");
        }
        // Обратная совместимость: старый JSON без новых полей десериализуется.
        let legacy: LintIssue = serde_json::from_str(
            r#"{"file":"src/x.rs","line":1,"rule":"r","message":"m","severity":"error"}"#,
        )
        .expect("legacy JSON без карточных полей");
        assert!(legacy.ad.is_none() && legacy.fix_hint.is_none() && legacy.skill.is_none());
    }

    #[test]
    fn fitness_each_file_must_contain_per_file_issues() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        write_file(&repo, "services/a/config.yaml", "timeout: 5s\n");
        write_file(&repo, "services/b/config.yaml", "retries: 3\n");
        let constraints = write_file(
            dir.path(),
            "CONSTRAINTS.yaml",
            "rules:\n\
             \x20 - name: every_client_has_timeout\n\
             \x20   type: each_file_must_contain\n\
             \x20   glob: 'services/*/config.yaml'\n\
             \x20   pattern: 'timeout'\n\
             \x20 - name: glob_miss\n\
             \x20   type: each_file_must_contain\n\
             \x20   glob: 'services/*/settings.yaml'\n\
             \x20   pattern: 'timeout'\n",
        );
        let report = check(&repo, &constraints).unwrap();
        assert!(!report.passed);
        let by_rule = |name: &str| {
            report
                .issues
                .iter()
                .filter(|i| i.rule == name)
                .collect::<Vec<_>>()
        };
        let timeout_issues = by_rule("every_client_has_timeout");
        assert_eq!(
            timeout_issues.len(),
            1,
            "только services/b: {timeout_issues:?}"
        );
        assert!(timeout_issues[0].file.ends_with("services/b/config.yaml"));
        assert_eq!(by_rule("glob_miss").len(), 1, "пустой набор — находка");
    }

    #[test]
    fn fitness_dir_must_have_file_per_dir_issues() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        write_file(&repo, "services/a/openapi.yaml", "openapi: 3.0.0\n");
        write_file(&repo, "services/b/main.rs", "fn main() {}\n");
        // Служебный каталог не должен попадать в выборку даже при широком glob.
        write_file(&repo, "node_modules/pkg/package.json", "{}\n");
        let constraints = write_file(
            dir.path(),
            "CONSTRAINTS.yaml",
            "rules:\n\
             \x20 - name: every_service_has_contract\n\
             \x20   type: dir_must_have_file\n\
             \x20   glob: 'services/*'\n\
             \x20   path: openapi.yaml\n\
             \x20 - name: glob_miss\n\
             \x20   type: dir_must_have_file\n\
             \x20   glob: 'apps/*'\n\
             \x20   path: openapi.yaml\n",
        );
        let report = check(&repo, &constraints).unwrap();
        assert!(!report.passed);
        let by_rule = |name: &str| {
            report
                .issues
                .iter()
                .filter(|i| i.rule == name)
                .collect::<Vec<_>>()
        };
        let contract_issues = by_rule("every_service_has_contract");
        assert_eq!(
            contract_issues.len(),
            1,
            "только services/b: {contract_issues:?}"
        );
        assert!(contract_issues[0].file.ends_with("services/b"));
        assert!(contract_issues[0].message.contains("openapi.yaml"));
        assert_eq!(
            by_rule("glob_miss").len(),
            1,
            "пустой набор каталогов — находка"
        );
        assert!(
            report
                .issues
                .iter()
                .all(|i| !i.file.to_string_lossy().contains("node_modules"))
        );
    }

    #[test]
    fn fitness_dir_must_have_file_passes_when_all_dirs_have_it() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        write_file(&repo, "services/a/openapi.yaml", "openapi: 3.0.0\n");
        write_file(&repo, "services/b/openapi.yaml", "openapi: 3.0.0\n");
        let constraints = write_file(
            dir.path(),
            "CONSTRAINTS.yaml",
            "rules:\n\
             \x20 - name: every_service_has_contract\n\
             \x20   type: dir_must_have_file\n\
             \x20   glob: 'services/*'\n\
             \x20   path: openapi.yaml\n",
        );
        let report = check(&repo, &constraints).unwrap();
        assert!(report.passed, "{:?}", report.issues);
    }

    #[test]
    fn fitness_max_age_fresh_file_passes() {
        // (а) свежий файл + реальное now → pass: возраст файла (микросекунды)
        // несопоставим с лимитом 3650 дней.
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        write_file(&repo, "evidence/dr-report.md", "учения: 2026-08\n");
        assert_eq!(
            check_max_age(&repo, "evidence/dr-report.md", 3650, SystemTime::now()).unwrap(),
            None
        );
    }

    #[test]
    fn fitness_max_age_stale_file_fails_with_age() {
        // (б) тот же файл + now, сдвинутый на max_age_days + запас → fail
        // с возрастом в днях против лимита (mtime не трогаем — std не умеет).
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        let evidence = write_file(&repo, "evidence/dr-report.md", "учения: 2026-08\n");
        let mtime = std::fs::metadata(&evidence).unwrap().modified().unwrap();
        let stale_now = mtime + Duration::from_secs((3650 + 2) * 86_400);
        let message = check_max_age(&repo, "evidence/dr-report.md", 3650, stale_now)
            .unwrap()
            .expect("файл старше лимита — нарушение");
        assert!(message.contains("устарел"), "{message}");
        assert!(message.contains("3652 дн."), "{message}");
        assert!(message.contains("лимите 3650"), "{message}");
    }

    #[test]
    fn fitness_max_age_missing_file_fails() {
        // (в) отсутствующий файл → fail с сообщением как у file_exists.
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let message = check_max_age(&repo, "evidence/missing.md", 3650, SystemTime::now())
            .unwrap()
            .expect("отсутствующий файл — нарушение");
        assert_eq!(message, "file_exists: файл не найден: evidence/missing.md");
    }

    #[test]
    fn fitness_max_age_yaml_parses_and_backward_compat() {
        // (г) YAML: правило max_age (path + max_age_days) парсится и работает;
        // старые типы рядом парсятся без изменений (обратная совместимость).
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        write_file(&repo, "evidence/dr-report.md", "учения: 2026-08\n");
        write_file(&repo, "src/main.rs", "fn main() {}\n");
        let constraints = write_file(
            dir.path(),
            "CONSTRAINTS.yaml",
            "rules:\n\
             \x20 - name: dr_report_fresh\n\
             \x20   type: max_age\n\
             \x20   path: evidence/dr-report.md\n\
             \x20   max_age_days: 365\n\
             \x20 - name: legacy_file_exists\n\
             \x20   type: file_exists\n\
             \x20   path: evidence/dr-report.md\n\
             \x20 - name: legacy_must_contain\n\
             \x20   type: must_contain\n\
             \x20   glob: 'src/**/*.rs'\n\
             \x20   pattern: 'fn main'\n\
             \x20 - name: legacy_command\n\
             \x20   type: command_succeeds\n\
             \x20   command: 'true'\n",
        );
        let report = check(&repo, &constraints).unwrap();
        assert!(
            report.passed,
            "max_age и старые типы парсятся и проходят: {:?}",
            report.issues
        );
        assert!(
            report
                .summary
                .starts_with("Правил: 4, нарушений: 0 (error: 0, warn: 0)"),
            "{}",
            report.summary
        );
        // max_age без обязательных полей — ошибка парсинга правила.
        let no_days = write_file(
            dir.path(),
            "no-days.yaml",
            "rules:\n\
             \x20 - name: x\n\
             \x20   type: max_age\n\
             \x20   path: a.txt\n",
        );
        assert!(
            check(&repo, &no_days).is_err(),
            "max_age без max_age_days — ошибка"
        );
        let no_path = write_file(
            dir.path(),
            "no-path.yaml",
            "rules:\n\
             \x20 - name: x\n\
             \x20   type: max_age\n\
             \x20   max_age_days: 365\n",
        );
        assert!(check(&repo, &no_path).is_err(), "max_age без path — ошибка");
    }

    #[test]
    fn fitness_max_age_medium_severity_warns() {
        // (д) severity medium наследуется общим правилом: medium → warn,
        // итог остаётся PASS. max_age_days: 0 — любой файл «старше» лимита
        // (mtime всегда раньше now), детерминированно без трюков с mtime.
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        write_file(&repo, "evidence/stale.md", "отчёт прошлого года\n");
        let constraints = write_file(
            dir.path(),
            "CONSTRAINTS.yaml",
            "rules:\n\
             \x20 - name: stale_evidence\n\
             \x20   type: max_age\n\
             \x20   path: evidence/stale.md\n\
             \x20   max_age_days: 0\n\
             \x20   severity: medium\n",
        );
        let report = check(&repo, &constraints).unwrap();
        assert!(report.passed, "medium → warn не ломает итог");
        assert_eq!(report.issues.len(), 1);
        assert_eq!(report.issues[0].severity, "warn");
        assert!(report.issues[0].message.contains("устарел"));
        assert!(
            report
                .summary
                .starts_with("Правил: 1, нарушений: 1 (error: 0, warn: 1)"),
            "{}",
            report.summary
        );
    }

    #[test]
    fn fitness_exclude_glob_string_and_list_forms() {
        // Мотив — живой кейс флота 2026-09-01: архитектурный агент писал
        // exclude_glob в CONSTRAINTS.yaml, а движок поле игнорировал.
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        // mart.py — легитимное место JOIN (сборщик витрины), исключён из запрета.
        write_file(&repo, "poc/meta_agent/mart.py", "SELECT a JOIN b\n");
        write_file(&repo, "poc/meta_agent/api.py", "SELECT a JOIN b\n");
        write_file(&repo, "services/a/openapi.yaml", "openapi: 3.0.0\n");
        write_file(&repo, "services/b/openapi.yaml", "openapi: 3.0.0\n");
        let constraints = write_file(
            dir.path(),
            "CONSTRAINTS.yaml",
            "rules:\n\
             \x20 - name: no_runtime_join\n\
             \x20   type: must_not_contain\n\
             \x20   glob: 'poc/**/*.py'\n\
             \x20   pattern: 'JOIN'\n\
             \x20   exclude_glob: 'poc/meta_agent/mart.py'\n\
             \x20 - name: contracts_except_legacy\n\
             \x20   type: dir_must_have_file\n\
             \x20   glob: 'services/*'\n\
             \x20   path: asyncapi.yaml\n\
             \x20   exclude_glob: ['services/a', 'services/b']\n",
        );
        let report = check(&repo, &constraints).unwrap();
        let join_issues: Vec<_> = report
            .issues
            .iter()
            .filter(|i| i.rule == "no_runtime_join")
            .collect();
        assert_eq!(join_issues.len(), 1, "только api.py: {join_issues:?}");
        assert!(join_issues[0].file.ends_with("api.py"));
        // Оба каталога исключены списком — набор пуст → одна находка о пустом наборе.
        let dir_issues: Vec<_> = report
            .issues
            .iter()
            .filter(|i| i.rule == "contracts_except_legacy")
            .collect();
        assert_eq!(dir_issues.len(), 1, "{dir_issues:?}");
        assert!(
            dir_issues[0]
                .message
                .contains("не найдено ни одного каталога")
        );
    }

    #[test]
    fn fitness_expired_rule_warns_but_passes_and_metadata_accepted() {
        // Карточка правила (trigger/rationale/owner/expiry из шаблона
        // дистилляции) — опциональные метаданные; expiry в прошлом —
        // warn-находка, итог не ломает.
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        write_file(&repo, "src/main.rs", "fn main() {}\n");
        let constraints = write_file(
            dir.path(),
            "CONSTRAINTS.yaml",
            "rules:\n\
             \x20 - name: old_rule\n\
             \x20   type: file_exists\n\
             \x20   path: src/main.rs\n\
             \x20   trigger: пока стек X\n\
             \x20   rationale: исторический запрет\n\
             \x20   owner: архитектор контура\n\
             \x20   expiry: '2020-01-01'\n\
             \x20 - name: fresh_rule\n\
             \x20   type: file_exists\n\
             \x20   path: src/main.rs\n\
             \x20   expiry: '2999-01-01'\n",
        );
        let report = check(&repo, &constraints).unwrap();
        assert!(report.passed, "warn не ломает итог: {:?}", report.issues);
        let expired: Vec<_> = report
            .issues
            .iter()
            .filter(|i| i.rule == "old_rule")
            .collect();
        assert_eq!(expired.len(), 1, "{expired:?}");
        assert_eq!(expired[0].severity, "warn");
        assert!(expired[0].message.contains("просрочено"));
        assert!(
            report.issues.iter().all(|i| i.rule != "fresh_rule"),
            "будущая дата — без находки: {:?}",
            report.issues
        );
    }

    #[test]
    fn fitness_skips_handoff_packet_and_junk_dirs() {
        // Разрыв P2 «fitness целится в документ решения»: правило с широким
        // glob срабатывало на текст spine внутри пакета. Служебные каталоги
        // (.arch-handoff, node_modules, __pycache__, …) исключены из обхода.
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        write_file(&repo, "src/bad.py", "print('hi')\n");
        write_file(
            &repo,
            ".arch-handoff/TASK.md",
            "Контекст: print( запрещён в проде\n",
        );
        write_file(&repo, "node_modules/pkg/index.js", "print('junk')\n");
        let constraints = write_file(
            dir.path(),
            "CONSTRAINTS.yaml",
            "rules:\n\
             \x20 - name: no_print\n\
             \x20   type: must_not_contain\n\
             \x20   glob: '**/*'\n\
             \x20   pattern: 'print\\('\n",
        );
        let report = check(&repo, &constraints).unwrap();
        assert_eq!(
            report.issues.len(),
            1,
            "только код, не пакет: {:?}",
            report.issues
        );
        assert!(report.issues[0].file.ends_with("src/bad.py"));
    }

    #[test]
    fn fitness_passes_when_all_rules_hold() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        write_file(&repo, "src/main.rs", "fn main() {}\n");
        let constraints = write_file(
            dir.path(),
            "CONSTRAINTS.yaml",
            "rules:\n\
             \x20 - name: has_main\n\
             \x20   type: must_contain\n\
             \x20   glob: '**/*.rs'\n\
             \x20   pattern: 'fn main'\n\
             \x20 - name: cmd_ok\n\
             \x20   type: command_succeeds\n\
             \x20   command: 'true'\n\
             \x20   timeout_secs: 5\n",
        );
        let report = check(&repo, &constraints).unwrap();
        assert!(report.passed);
        assert!(report.issues.is_empty());
        assert!(
            report
                .summary
                .starts_with("Правил: 2, нарушений: 0 (error: 0, warn: 0)"),
            "{}",
            report.summary
        );
    }

    #[test]
    fn fitness_command_timeout_fails_rule() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let constraints = write_file(
            dir.path(),
            "CONSTRAINTS.yaml",
            "rules:\n\
             \x20 - name: slow\n\
             \x20   type: command_succeeds\n\
             \x20   command: 'sleep 5'\n\
             \x20   timeout_secs: 1\n",
        );
        let started = std::time::Instant::now();
        let report = check(&repo, &constraints).unwrap();
        assert!(!report.passed);
        assert_eq!(report.issues.len(), 1);
        assert!(report.issues[0].message.contains("таймаут"));
        assert!(
            started.elapsed() < Duration::from_secs(4),
            "таймаут должен убить команду раньше её завершения"
        );
    }

    #[test]
    fn fitness_rejects_invalid_constraints() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let bad_yaml = write_file(dir.path(), "bad.yaml", "rules: [\n");
        assert!(check(&repo, &bad_yaml).is_err(), "битый YAML — ошибка");
        let bad_severity = write_file(
            dir.path(),
            "sev.yaml",
            "rules:\n\
             \x20 - name: x\n\
             \x20   type: file_exists\n\
             \x20   path: a.txt\n\
             \x20   severity: fatal\n",
        );
        assert!(
            check(&repo, &bad_severity).is_err(),
            "неизвестный severity — ошибка"
        );
        assert!(
            check(&repo, &repo.join("missing.yaml")).is_err(),
            "несуществующий constraints — ошибка"
        );
    }

    #[test]
    fn fitness_accepts_constraints_root() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::write(repo.join("ok.go"), "package main\n").unwrap();
        // Корень `constraints:` (стиль кейсов/handoff-пакетов) читается наравне
        // с каноническим `rules:`.
        let file = write_file(
            dir.path(),
            "constraints.yaml",
            "constraints:\n\
             \x20 - id: C-001\n\
             \x20   name: go-file\n\
             \x20   type: must_contain\n\
             \x20   glob: \"**/*.go\"\n\
             \x20   pattern: package\n",
        );
        let report = check(&repo, &file).unwrap();
        assert!(report.passed, "правило из корня constraints: выполняется");
        let failing = write_file(
            dir.path(),
            "constraints-fail.yaml",
            "constraints:\n\
             \x20 - id: C-001\n\
             \x20   name: no-todo\n\
             \x20   type: must_not_contain\n\
             \x20   glob: \"**/*.go\"\n\
             \x20   pattern: package\n\
             \x20   severity: critical\n",
        );
        let report = check(&repo, &failing).unwrap();
        assert!(!report.passed, "нарушение из корня constraints: ловится");
        assert!(
            report.issues.iter().all(|i| i.severity == "error"),
            "critical маппится в блокирующий error"
        );
    }

    #[test]
    fn fitness_rejects_constraints_file_without_rules() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        // Опечатка в корне (`rulez:`) не должна давать тихий PASS.
        let typo = write_file(dir.path(), "typo.yaml", "rulez:\n  - name: x\n");
        assert!(
            check(&repo, &typo).is_err(),
            "файл без правил rules:/constraints: — ошибка, а не тихий PASS"
        );
        let empty = write_file(dir.path(), "empty.yaml", "rules: []\n");
        assert!(
            check(&repo, &empty).is_err(),
            "пустой список правил — ошибка"
        );
    }

    #[test]
    fn glob_matcher_cases() {
        assert!(glob_matches("**/*.rs", "src/main.rs"));
        assert!(
            glob_matches("**/*.rs", "main.rs"),
            "** матчит ноль сегментов"
        );
        assert!(glob_matches("*.md", "a.md"));
        assert!(!glob_matches("*.md", "docs/a.md"));
        assert!(glob_matches("docs/**", "docs/a/b.txt"));
        assert!(glob_matches("src/*/mod.rs", "src/foo/mod.rs"));
        assert!(!glob_matches("src/*/mod.rs", "src/foo/bar/mod.rs"));
        assert!(glob_matches("plain/path.txt", "plain/path.txt"));
        assert!(!glob_matches("plain/path.txt", "plain/other.txt"));
        assert!(segment_matches("f?o.rs", "foo.rs"));
        assert!(!segment_matches("f?o.rs", "fo.rs"));
        assert!(segment_matches("*", "anything"));
    }

    // --- dependency_direction (ADR-029) -----------------------------------

    /// Мини-репозиторий для слоевых тестов: `src/llm/engine.rs` импортирует
    /// `crate::config` (use) и `crate::agent` (инлайн-путь), комментарий
    /// упоминает `crate::tui` (должен игнорироваться).
    fn deps_repo(dir: &Path) -> PathBuf {
        let repo = dir.join("repo");
        write_file(
            &repo,
            "src/llm/engine.rs",
            "use crate::config::Config;\n\
             \n\
             pub fn f() {\n\
             \x20   let _ = crate::agent::run();\n\
             }\n\
             // crate::tui::render — упоминание в комментарии\n",
        );
        write_file(&repo, "src/agent.rs", "pub fn run() {}\n");
        repo
    }

    #[test]
    fn fitness_dependency_direction_forbid_violation_and_pass() {
        let dir = tempfile::tempdir().unwrap();
        let repo = deps_repo(dir.path());
        let constraints = write_file(
            dir.path(),
            "CONSTRAINTS.yaml",
            "rules:\n\
             \x20 - name: llm_no_agent\n\
             \x20   type: dependency_direction\n\
             \x20   glob: 'src/llm/**'\n\
             \x20   forbid: ['agent', 'tui']\n",
        );
        let report = check(&repo, &constraints).unwrap();
        assert!(!report.passed);
        assert_eq!(report.issues.len(), 1, "{:?}", report.issues);
        let i = &report.issues[0];
        assert_eq!(i.rule, "llm_no_agent");
        assert_eq!(i.line, 4, "инлайн-путь на 4-й строке: {i:?}");
        assert!(i.file.ends_with("src/llm/engine.rs"));
        assert!(i.message.contains("crate-путь 'agent'") || i.message.contains("'agent'"));
        assert!(
            !i.message.contains("tui"),
            "комментарий с упоминанием tui-модуля игнорируется: {i:?}"
        );

        // Чистый вариант: forbid только tui — нарушений нет.
        let constraints = write_file(
            dir.path(),
            "CONSTRAINTS2.yaml",
            "rules:\n\
             \x20 - name: llm_no_tui\n\
             \x20   type: dependency_direction\n\
             \x20   glob: 'src/llm/**'\n\
             \x20   forbid: ['tui']\n",
        );
        let report = check(&repo, &constraints).unwrap();
        assert!(report.passed, "{:?}", report.issues);
    }

    #[test]
    fn fitness_dependency_direction_allow_list() {
        let dir = tempfile::tempdir().unwrap();
        let repo = deps_repo(dir.path());
        let constraints = write_file(
            dir.path(),
            "CONSTRAINTS.yaml",
            "rules:\n\
             \x20 - name: llm_allow\n\
             \x20   type: dependency_direction\n\
             \x20   glob: ['src/llm.rs', 'src/llm/**']\n\
             \x20   allow: ['config', 'error']\n",
        );
        let report = check(&repo, &constraints).unwrap();
        assert!(!report.passed);
        assert_eq!(report.issues.len(), 1, "{:?}", report.issues);
        assert!(
            report.issues[0].message.contains("вне allow-списка"),
            "{:?}",
            report.issues[0]
        );
    }

    #[test]
    fn fitness_dependency_direction_empty_allow_forbids_all() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        write_file(&repo, "src/error.rs", "pub struct E;\n");
        write_file(&repo, "src/secrets.rs", "use crate::error::E;\n");
        let constraints = write_file(
            dir.path(),
            "CONSTRAINTS.yaml",
            "rules:\n\
             \x20 - name: leaves\n\
             \x20   type: dependency_direction\n\
             \x20   glob: ['src/error.rs', 'src/secrets.rs']\n\
             \x20   allow: []\n",
        );
        let report = check(&repo, &constraints).unwrap();
        assert!(!report.passed);
        assert_eq!(report.issues.len(), 1, "{:?}", report.issues);
        assert!(report.issues[0].file.ends_with("src/secrets.rs"));
    }

    #[test]
    fn fitness_dependency_direction_requires_exactly_one_mode() {
        let dir = tempfile::tempdir().unwrap();
        let repo = deps_repo(dir.path());
        for (name, extra) in [
            ("both", "forbid: ['agent']\n   allow: ['config']\n"),
            ("none", ""),
        ] {
            let constraints = write_file(
                dir.path(),
                &format!("C-{name}.yaml"),
                &format!(
                    "rules:\n - name: bad\n   type: dependency_direction\n   glob: 'src/**'\n   {extra}"
                ),
            );
            let err = check(&repo, &constraints).unwrap_err();
            assert!(
                err.to_string().contains("ровно одно из forbid/allow"),
                "{name}: {err}"
            );
        }
    }

    #[test]
    fn fitness_dependency_direction_empty_file_set_is_issue() {
        let dir = tempfile::tempdir().unwrap();
        let repo = deps_repo(dir.path());
        let constraints = write_file(
            dir.path(),
            "CONSTRAINTS.yaml",
            "rules:\n\
             \x20 - name: dead_glob\n\
             \x20   type: dependency_direction\n\
             \x20   glob: 'src/ghost/**'\n\
             \x20   forbid: ['agent']\n",
        );
        let report = check(&repo, &constraints).unwrap();
        assert!(!report.passed);
        assert!(
            report.issues[0]
                .message
                .contains("правилу нечего проверять"),
            "{:?}",
            report.issues[0]
        );
    }

    #[test]
    fn fitness_dependency_direction_python_imports() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        write_file(
            &repo,
            "services/api/main.py",
            "import os\nfrom services.legacy.db import connect\n# from services.legacy.x import y\n",
        );
        let constraints = write_file(
            dir.path(),
            "CONSTRAINTS.yaml",
            "rules:\n\
             \x20 - name: no_legacy\n\
             \x20   type: dependency_direction\n\
             \x20   glob: 'services/**/*.py'\n\
             \x20   forbid: ['services/legacy']\n",
        );
        let report = check(&repo, &constraints).unwrap();
        assert!(!report.passed);
        assert_eq!(report.issues.len(), 1, "{:?}", report.issues);
        assert_eq!(report.issues[0].line, 2);
        assert!(report.issues[0].message.contains("services/legacy/db"));
    }

    // --- context_boundary (ADR-030) ----------------------------------------

    /// Репозиторий с моделью из двух контекстов и python-кодом:
    /// `services/alpha` импортирует `services.beta` (через границу).
    fn contexts_repo(dir: &Path, alpha_depends_on_beta: bool) -> PathBuf {
        let repo = dir.join("repo");
        let depends = if alpha_depends_on_beta {
            "depends_on: [CMP-002]\n"
        } else {
            ""
        };
        write_file(
            &repo,
            "model/CMP-001-alpha.md",
            &format!(
                "---\nid: CMP-001\ntype: cmp\ntitle: Alpha\nstatus: adopted\n{depends}code_roots: [services/alpha]\n---\nКонтекст A.\n"
            ),
        );
        write_file(
            &repo,
            "model/CMP-002-beta.md",
            "---\nid: CMP-002\ntype: cmp\ntitle: Beta\nstatus: adopted\ncode_roots: [services/beta]\n---\nКонтекст B.\n",
        );
        write_file(
            &repo,
            "services/alpha/main.py",
            "from services.beta.core import run\nfrom services.alpha.util import helper\n",
        );
        write_file(&repo, "services/beta/core.py", "def run(): pass\n");
        write_file(&repo, "services/alpha/util.py", "def helper(): pass\n");
        write_file(
            &repo,
            "tools/free.py",
            "from services.beta.core import run\n",
        );
        repo
    }

    #[test]
    fn fitness_context_boundary_detects_crossing() {
        let dir = tempfile::tempdir().unwrap();
        let repo = contexts_repo(dir.path(), false);
        let constraints = write_file(
            dir.path(),
            "CONSTRAINTS.yaml",
            "rules:\n\
             \x20 - name: contexts\n\
             \x20   type: context_boundary\n",
        );
        let report = check(&repo, &constraints).unwrap();
        assert!(!report.passed);
        assert_eq!(report.issues.len(), 1, "{:?}", report.issues);
        let i = &report.issues[0];
        assert_eq!(i.line, 1, "только импорт beta; свой alpha и tools/ — мимо");
        assert!(i.file.ends_with("services/alpha/main.py"));
        assert!(
            i.message.contains("CMP-001") && i.message.contains("CMP-002"),
            "{i:?}"
        );
    }

    #[test]
    fn fitness_context_boundary_depends_on_allows_crossing() {
        let dir = tempfile::tempdir().unwrap();
        let repo = contexts_repo(dir.path(), true);
        let constraints = write_file(
            dir.path(),
            "CONSTRAINTS.yaml",
            "rules:\n\
             \x20 - name: contexts\n\
             \x20   type: context_boundary\n",
        );
        let report = check(&repo, &constraints).unwrap();
        assert!(report.passed, "{:?}", report.issues);
    }

    #[test]
    fn fitness_context_boundary_resolves_rust_crate_paths() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        write_file(
            &repo,
            "model/CMP-001-alpha.md",
            "---\nid: CMP-001\ntype: cmp\ntitle: Alpha\nstatus: adopted\ncode_roots: [src/alpha]\n---\nA.\n",
        );
        write_file(
            &repo,
            "model/CMP-002-beta.md",
            "---\nid: CMP-002\ntype: cmp\ntitle: Beta\nstatus: adopted\ncode_roots: [src/beta]\n---\nB.\n",
        );
        write_file(
            &repo,
            "src/alpha/lib.rs",
            "use crate::beta::core::Engine;\n",
        );
        write_file(&repo, "src/beta/core.rs", "pub struct Engine;\n");
        let constraints = write_file(
            dir.path(),
            "CONSTRAINTS.yaml",
            "rules:\n\
             \x20 - name: contexts\n\
             \x20   type: context_boundary\n",
        );
        let report = check(&repo, &constraints).unwrap();
        assert!(!report.passed);
        assert_eq!(report.issues.len(), 1, "{:?}", report.issues);
        assert!(report.issues[0].file.ends_with("src/alpha/lib.rs"));
    }

    #[test]
    fn fitness_context_boundary_missing_model_dir_is_issue() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        write_file(&repo, "src/main.rs", "fn main() {}\n");
        let constraints = write_file(
            dir.path(),
            "CONSTRAINTS.yaml",
            "rules:\n\
             \x20 - name: contexts\n\
             \x20   type: context_boundary\n",
        );
        let report = check(&repo, &constraints).unwrap();
        assert!(!report.passed);
        assert!(
            report.issues[0]
                .message
                .contains("каталог модели не найден"),
            "{:?}",
            report.issues[0]
        );
    }

    #[test]
    fn fitness_context_boundary_overlapping_roots_is_config_error() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        write_file(
            &repo,
            "model/CMP-001-a.md",
            "---\nid: CMP-001\ntype: cmp\ntitle: A\nstatus: adopted\ncode_roots: [services/x]\n---\nA.\n",
        );
        write_file(
            &repo,
            "model/CMP-002-b.md",
            "---\nid: CMP-002\ntype: cmp\ntitle: B\nstatus: adopted\ncode_roots: [services/x/sub]\n---\nB.\n",
        );
        write_file(&repo, "services/x/sub/m.py", "pass\n");
        let constraints = write_file(
            dir.path(),
            "CONSTRAINTS.yaml",
            "rules:\n\
             \x20 - name: contexts\n\
             \x20   type: context_boundary\n",
        );
        let err = check(&repo, &constraints).unwrap_err();
        assert!(err.to_string().contains("code_roots пересекаются"), "{err}");
    }

    #[test]
    fn fitness_context_boundary_without_code_roots_is_issue() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        write_file(
            &repo,
            "model/CMP-001-a.md",
            "---\nid: CMP-001\ntype: cmp\ntitle: A\nstatus: adopted\n---\nA.\n",
        );
        write_file(&repo, "src/main.rs", "fn main() {}\n");
        let constraints = write_file(
            dir.path(),
            "CONSTRAINTS.yaml",
            "rules:\n\
             \x20 - name: contexts\n\
             \x20   type: context_boundary\n",
        );
        let report = check(&repo, &constraints).unwrap();
        assert!(!report.passed);
        assert!(
            report.issues[0].message.contains("нет CMP с code_roots"),
            "{:?}",
            report.issues[0]
        );
    }

    // --- M-1a: per-rule timing ------------------------------------------------

    #[test]
    fn fitness_report_carries_per_rule_durations() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        write_file(&repo, "src/main.rs", "fn main() {}\n");
        let constraints = write_file(
            dir.path(),
            "CONSTRAINTS.yaml",
            "rules:\n\
             \x20 - name: has_main\n\
             \x20   type: must_contain\n\
             \x20   glob: '**/*.rs'\n\
             \x20   pattern: 'fn main'\n\
             \x20 - name: main_exists\n\
             \x20   type: file_exists\n\
             \x20   path: src/main.rs\n",
        );
        let report = check(&repo, &constraints).unwrap();
        // Длительность замеряется для КАЖДОГО правила (порядок — как в файле).
        let names: Vec<&str> = report.durations.iter().map(|d| d.rule.as_str()).collect();
        assert_eq!(names, ["has_main", "main_exists"], "{:?}", report.durations);
        // SDK-контракт v1: durations — аддитивный ключ рядом с каноническими.
        let v: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&report).unwrap()).unwrap();
        assert!(v.get("durations").is_some(), "аддитивный ключ durations");
        assert_eq!(v["durations"][0]["rule"], "has_main");
        assert!(v["durations"][0]["ms"].is_number());
        for key in ["repo", "passed", "summary", "issues"] {
            assert!(v.get(key).is_some(), "канонический ключ {key} на месте");
        }
        // Десериализация старого JSON без durations — serde default (обратная
        // совместимость для SDK-клиентов, читающих отчёт в структуру).
        let legacy: FitnessReport =
            serde_json::from_str(r#"{"repo":".","passed":true,"issues":[],"summary":"s"}"#)
                .unwrap();
        assert!(legacy.durations.is_empty());
    }

    /// A2: правило `command_succeeds`, чья команда требует отсутствующий в
    /// PATH прогонщик, — пропуск с причиной (`runner_skipped`), а не
    /// error-находка; самодостаточная команда исполняется как раньше (и
    /// падает, как раньше). Отсутствие раннера имитируется снимком
    /// `Runner::unavailable()` — детерминированно, без троганья окружения.
    #[test]
    fn command_succeeds_without_runner_is_skipped_not_failed() {
        let dir = tempfile::tempdir().unwrap();
        let constraints = write_file(
            dir.path(),
            "CONSTRAINTS.yaml",
            "rules:\n  - name: needs_pytest\n    type: command_succeeds\n    \
             command: 'python3 -m pytest -q tests/x.py'\n    severity: error\n\
             \x20 - name: needs_maven\n    type: command_succeeds\n    \
             command: 'mvn -q test'\n    severity: warn\n\
             \x20 - name: plain_fail\n    type: command_succeeds\n    \
             command: 'false'\n    severity: error\n\
             \x20 - name: plain_ok\n    type: command_succeeds\n    \
             command: 'true'\n    severity: error\n",
        );
        let (rules, _unknown) = load_fitness_rules_with_skips(&constraints).unwrap();
        let refs: Vec<&FitnessRule> = rules.iter().collect();
        let mut skipped = Vec::new();
        let mut runner_skipped = Vec::new();
        let mut untrusted_skipped = Vec::new();
        let mut issues = Vec::new();
        for rule in &refs {
            run_rule(
                rule,
                dir.path(),
                &refs,
                None,
                &mut skipped,
                &mut runner_skipped,
                &mut untrusted_skipped,
                &crate::rule_templates::Runner::unavailable(),
                crate::cmd_trust::ExecDecision::Allow,
                &mut issues,
            )
            .unwrap();
        }
        // Правила с раннерами — пропущены, а не провалены.
        assert_eq!(runner_skipped.len(), 2, "{runner_skipped:?}");
        let pytest_skip = runner_skipped
            .iter()
            .find(|s| s.rule == "needs_pytest")
            .expect("пропуск needs_pytest");
        assert_eq!(pytest_skip.severity, "error");
        assert_eq!(pytest_skip.runners, vec!["pytest".to_string()]);
        assert!(
            pytest_skip
                .reason
                .starts_with(crate::rule_templates::RUNNER_ABSENT_PREFIX),
            "{}",
            pytest_skip.reason
        );
        assert!(
            pytest_skip.reason.contains("pytest:"),
            "{}",
            pytest_skip.reason
        );
        // Уточнение причины средо-зависимое: «python3 есть, модуля pytest
        // нет» → подсказка `pip install pytest`; «python3 нет вообще»
        // (hermetic-контейнер CI без python3) → «нет `python3` в PATH».
        // Обе формы — честный SKIP про нужный раннер, а не ✗.
        assert!(
            pytest_skip.reason.contains("pip install pytest")
                || pytest_skip.reason.contains("нет `python3` в PATH"),
            "{}",
            pytest_skip.reason
        );
        let maven_skip = runner_skipped
            .iter()
            .find(|s| s.rule == "needs_maven")
            .expect("пропуск needs_maven");
        assert_eq!(maven_skip.severity, "warn");
        // Самодостаточные команды исполняются: `false` — находка, `true` — чисто.
        assert!(issues.iter().any(|i| i.rule == "plain_fail"), "{issues:?}");
        assert!(issues.iter().all(|i| i.rule != "plain_ok"), "{issues:?}");
        // Пропущенные правила в находки не попали: пропуск — не провал.
        assert!(
            issues
                .iter()
                .all(|i| i.rule != "needs_pytest" && i.rule != "needs_maven"),
            "{issues:?}"
        );
        assert!(skipped.is_empty(), "{skipped:?}");
    }

    /// A3: запрет исполнения (no-exec) — правило `command_succeeds` НЕ
    /// запускается вообще: пропуск `command_untrusted` с подсказкой, а
    /// файл-маяк, который создала бы команда, отсутствует. Решение
    /// инжектируется снимком `ExecDecision` — без мутаций окружения (A2-паттерн).
    #[test]
    fn command_succeeds_under_no_exec_is_skipped_and_never_runs() {
        let dir = tempfile::tempdir().unwrap();
        let constraints = write_file(
            dir.path(),
            "CONSTRAINTS.yaml",
            "rules:\n  - name: touched\n    type: command_succeeds\n    \
             command: 'touch marker.txt'\n    severity: error\n",
        );
        let (rules, _unknown) = load_fitness_rules_with_skips(&constraints).unwrap();
        let refs: Vec<&FitnessRule> = rules.iter().collect();
        let mut skipped = Vec::new();
        let mut runner_skipped = Vec::new();
        let mut untrusted_skipped = Vec::new();
        let mut issues = Vec::new();
        for rule in &refs {
            run_rule(
                rule,
                dir.path(),
                &refs,
                None,
                &mut skipped,
                &mut runner_skipped,
                &mut untrusted_skipped,
                &crate::rule_templates::Runner::unavailable(),
                crate::cmd_trust::ExecDecision::Deny(crate::cmd_trust::DenyReason::NoExec),
                &mut issues,
            )
            .unwrap();
        }
        assert_eq!(untrusted_skipped.len(), 1, "{untrusted_skipped:?}");
        let skip = &untrusted_skipped[0];
        assert_eq!(skip.rule, "touched");
        assert_eq!(skip.severity, "error");
        assert!(
            skip.reason.starts_with(crate::cmd_trust::COMMAND_UNTRUSTED),
            "{}",
            skip.reason
        );
        assert!(
            skip.reason.contains("ARCH_NO_EXEC=0"),
            "подсказка по разрешению: {}",
            skip.reason
        );
        assert!(
            !dir.path().join("marker.txt").exists(),
            "команда НЕ исполнялась: файл-маяк отсутствует"
        );
        assert!(issues.is_empty(), "пропуск — не находка: {issues:?}");
        assert!(runner_skipped.is_empty(), "{runner_skipped:?}");

        // Контроль маяка: при разрешении та же команда исполняется.
        let mut issues = Vec::new();
        let mut untrusted_skipped = Vec::new();
        for rule in &refs {
            run_rule(
                rule,
                dir.path(),
                &refs,
                None,
                &mut skipped,
                &mut runner_skipped,
                &mut untrusted_skipped,
                &crate::rule_templates::Runner::unavailable(),
                crate::cmd_trust::ExecDecision::Allow,
                &mut issues,
            )
            .unwrap();
        }
        assert!(dir.path().join("marker.txt").exists(), "Allow: маяк создан");
        assert!(untrusted_skipped.is_empty());
    }

    /// A3 на уровне прогона: политика no-exec — все command_succeeds-правила
    /// в пропуске `command_untrusted`, `passed` не страдает (пропуск — не
    /// находка), маяк не создан, сводка несёт маркер. Политика инжектируется
    /// через `CheckOptions.exec` — окружение не трогается.
    #[test]
    fn check_with_no_exec_policy_skips_commands_without_failing() {
        let dir = tempfile::tempdir().unwrap();
        let constraints = write_file(
            dir.path(),
            "CONSTRAINTS.yaml",
            "rules:\n  - name: touched\n    type: command_succeeds\n    \
             command: 'touch marker.txt'\n    severity: error\n  - name: doc_present\n    \
             type: file_exists\n    path: \"README.md\"\n    severity: error\n",
        );
        std::fs::write(dir.path().join("README.md"), "x").unwrap();
        let options = baseline::CheckOptions {
            exec: crate::cmd_trust::ExecPolicy {
                no_exec: true,
                trust_file: None,
            },
            ..baseline::CheckOptions::default()
        };
        let report = check_with_options(dir.path(), &constraints, &options).unwrap();
        assert!(report.passed, "{}", report.summary);
        assert_eq!(report.untrusted_skipped.len(), 1, "{report:?}");
        assert_eq!(report.untrusted_skipped[0].rule, "touched");
        assert!(
            report.summary.contains(crate::cmd_trust::COMMAND_UNTRUSTED),
            "{}",
            report.summary
        );
        assert!(
            !dir.path().join("marker.txt").exists(),
            "команда не запускалась"
        );
        assert!(report.issues.is_empty(), "{:?}", report.issues);
    }

    /// A3, allow-файл: записи нет — исполнение (обратная совместимость);
    /// `record_allow` → совпадение отпечатка — исполнение (маяк создан);
    /// изменение реестра — SKIP `command_untrusted` (`StaleTrust`, маяк НЕ
    /// создан). allow-файл живёт в tempdir — окружение и дом не трогаются.
    #[test]
    fn check_allow_file_matrix() {
        let dir = tempfile::tempdir().unwrap();
        let trust = dir.path().join("trusted.json");
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let v1 = "rules:\n  - name: touched\n    type: command_succeeds\n    \
             command: 'touch marker.txt'\n    severity: error\n";
        let v2 = "rules:\n  - name: touched\n    type: command_succeeds\n    \
             command: 'touch other.txt'\n    severity: error\n";
        let constraints = write_file(&repo, "CONSTRAINTS.yaml", v1);
        let options = baseline::CheckOptions {
            exec: crate::cmd_trust::ExecPolicy {
                no_exec: false,
                trust_file: Some(trust.clone()),
            },
            ..baseline::CheckOptions::default()
        };
        // Файла нет — исполнение, как сегодня.
        let report = check_with_options(&repo, &constraints, &options).unwrap();
        assert!(
            report.passed && report.untrusted_skipped.is_empty(),
            "{}",
            report.summary
        );
        assert!(repo.join("marker.txt").exists(), "маяк создан");
        // Подтверждение доверия тем же каноном отпечатка, что у прогона.
        let fp = crate::cmd_trust::commands_fingerprint(["touch marker.txt"]);
        crate::cmd_trust::record_allow(&trust, &repo, &fp, 1, "CONSTRAINTS.yaml").unwrap();
        std::fs::remove_file(repo.join("marker.txt")).unwrap();
        let report = check_with_options(&repo, &constraints, &options).unwrap();
        assert!(
            report.untrusted_skipped.is_empty(),
            "совпадение — исполнение: {}",
            report.summary
        );
        assert!(repo.join("marker.txt").exists(), "маяк создан снова");
        // Реестр изменился после доверия — SKIP StaleTrust, команда не запускалась.
        write_file(&repo, "CONSTRAINTS.yaml", v2);
        let report = check_with_options(&repo, &constraints, &options).unwrap();
        assert_eq!(report.untrusted_skipped.len(), 1, "{}", report.summary);
        assert!(
            report.untrusted_skipped[0].reason.contains("rules allow"),
            "подсказка переподтверждения: {}",
            report.untrusted_skipped[0].reason
        );
        assert!(
            !repo.join("other.txt").exists(),
            "команда изменённого реестра не запускалась"
        );
        assert!(report.passed, "пропуск — не провал: {}", report.summary);
    }
}

#[cfg(test)]
mod command_capture_tests {
    use super::*;

    #[test]
    fn run_with_timeout_captures_output_tail_on_failure() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let outcome = run_with_timeout(
            tmp.path(),
            "echo marker-строка-вывода; echo ошибка-в-стдошибку >&2; exit 3",
            Duration::from_secs(10),
        )
        .expect("прогон команды");
        assert_eq!(outcome.status.and_then(|s| s.code()), Some(3));
        assert!(outcome.tail.contains("marker-строка-вывода"));
        assert!(outcome.tail.contains("ошибка-в-стдошибку"));
    }

    #[test]
    fn report_tail_keeps_last_lines_and_redacts_secrets() {
        use std::fmt::Write as _;
        let mut raw = String::new();
        for i in 1..=20 {
            writeln!(raw, "строка {i}").expect("запись в String не падает");
        }
        raw.push_str("ключ DEEPSEEK_API_KEY=sk-0123456789abcdef0123456789 в тексте\n");
        let tail = report_tail(&raw);
        assert_eq!(tail.lines().count(), REPORT_TAIL_LINES);
        assert!(tail.contains("строка 20"));
        assert!(!tail.contains("строка 5"), "старые строки обрезаны: {tail}");
        assert!(
            !tail.contains("sk-0123456789abcdef0123456789"),
            "секрет обязан быть замаскирован: {tail}"
        );
    }
}
