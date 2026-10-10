//! Движок исполнения fitness-правил ([`check`], [`check_with_options`]):
//! glob-обход репозитория, content-правила, `command_succeeds` (таймаут через
//! [`crate::proc`], доверие через [`crate::cmd_trust`], прогонщики через
//! [`crate::rule_templates`]), структурные правила `dependency_direction` /
//! `context_boundary` (ADR-029/030) и `deny_dependency`.
//!
//! Разбиение модуля (0.3.15, граница C-33/AD-9): `exec.rs` — оркестрация
//! прогона ([`check`], [`check_with_options`]), реестр правил и `requires`;
//! подмодуль [`runner`] — прогонщики правил; подмодуль [`glob`] — glob-обход
//! и матчинг путей (чистое перемещение с реэкспортами, публичные пути
//! `crate::control::exec::…` не меняются).

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::Path;
use std::time::Instant;

use super::baseline;
use super::registry::{evaluate_overrides, load_constraints_resolved};
use super::types::{
    FitnessReport, FitnessRule, LintIssue, RequiresSkippedRule, RuleDuration, RuleKind,
    RulesFingerprint, RunnerSkippedRule, SourceCount, UntrustedSkippedRule, normalize_severity,
};
use crate::error::{HarnessError, Result};

mod glob;
mod runner;

pub(crate) use glob::{collect_dirs, collect_files, glob_matches};
use runner::run_rule;

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
    // ADR-068 Am.3: схема `requires` — ресурс без зонда (встроенного или
    // объявленного `[gate.requires.<имя>]`) ошибка РЕЕСТРА, а не молчаливый
    // SKIP: опечатка не превращается в вечный пропуск.
    for rule in &resolved.rules {
        options.probes.validate(&rule.name, &rule.requires)?;
    }
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
    // A2: override узаконивает ослабление только настоящим принятым ADR в
    // пределах горизонта — политика (каталог ADR, срок) приходит из опций.
    let adr_policy = crate::control::registry::AdrPolicy::resolve(
        repo,
        options.overrides.adr_dir.as_deref(),
        options.overrides.max_horizon_months,
    );
    let (override_infos, override_findings, disabled) = evaluate_overrides(
        &resolved.overrides,
        &resolved.rules,
        constraints,
        &adr_policy,
    );

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
    let mut requires_skipped: Vec<RequiresSkippedRule> = Vec::new();
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
    // ADR-068: снимок ресурсов среды — ОДИН на прогон и только когда в реестре
    // есть правило с непустым `requires` (детект зондов не нужен реестру без
    // ресурсных правил). Явный снимок из опций (край/тесты) сильнее детекта;
    // иначе — снимок по реестру зондов конфига (ADR-068 Am.3).
    let resources = if rule_refs.iter().any(|r| !r.requires.is_empty()) {
        options
            .resources
            .clone()
            .unwrap_or_else(|| options.probes.snapshot(repo))
    } else {
        crate::control::requires::AvailableResources::all()
    };
    for rule in &rule_refs {
        let started = Instant::now();
        // ADR-068: ресурс недоступен — правило даёт SKIP с причиной (не PASS и
        // не FAIL): проверка не исполняется, в находки не попадает. Ресурс
        // доступен — обычная семантика (SKIP — не лазейка).
        if let Some(missing) = requires_missing(rule, &resources) {
            requires_skipped.push(RequiresSkippedRule {
                rule: rule.name.clone(),
                severity: normalize_severity(&rule.severity, &rule.name)?.to_string(),
                reason: requires_skip_reason(&missing, &options.probes),
                resources: missing,
            });
            continue;
        }
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
    if !requires_skipped.is_empty() {
        // ADR-068: ресурсный пропуск — «проверить не удалось на этой машине»:
        // в сводке маркером с текстом зондов доступности ресурсов, в гейте —
        // SKIP составляющей.
        let names: Vec<&str> = requires_skipped.iter().map(|s| s.rule.as_str()).collect();
        let notes = requires_skip_notes(&requires_skipped, &options.probes);
        let _ = write!(
            summary,
            "; не прогонялись (нет ресурса: {}): {}",
            names.join(", "),
            notes
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
        requires_skipped,
        fingerprint,
    })
}

/// ADR-068: недоступные ресурсы правила (`requires`). `None` — правило не
/// привязано к среде либо все ресурсы доступны (обычная семантика).
fn requires_missing(
    rule: &FitnessRule,
    resources: &crate::control::requires::AvailableResources,
) -> Option<Vec<String>> {
    if rule.requires.is_empty() {
        return None;
    }
    let missing = resources.missing(&rule.requires);
    (!missing.is_empty()).then_some(missing)
}

/// Причина ресурсного SKIP: перечисляет недоступные ресурсы и тексты их
/// зондов (note; для встроенных `cuda`/`stand` — обязательная строка прогона
/// на стенде).
fn requires_skip_reason(
    missing: &[String],
    probes: &crate::control::requires::ProbeRegistry,
) -> String {
    let notes = unique_notes(missing, probes);
    format!(
        "недоступны ресурсы среды: {} — {}",
        missing.join(", "),
        notes
    )
}

/// Уникальные тексты зондов для списка недоступных ресурсов (порядок
/// объявления, без повторов — один note на разные ресурсы печатается раз).
fn unique_notes(resources: &[String], probes: &crate::control::requires::ProbeRegistry) -> String {
    let mut notes: Vec<String> = Vec::new();
    for resource in resources {
        let note = probes.note(resource);
        if !notes.contains(&note) {
            notes.push(note);
        }
    }
    notes.join("; ")
}

/// Тексты зондов всех ресурсных пропусков прогона (для строки сводки).
fn requires_skip_notes(
    skipped: &[RequiresSkippedRule],
    probes: &crate::control::requires::ProbeRegistry,
) -> String {
    let mut notes: Vec<String> = Vec::new();
    for skip in skipped {
        for resource in &skip.resources {
            let note = probes.note(resource);
            if !notes.contains(&note) {
                notes.push(note);
            }
        }
    }
    notes.join("; ")
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

#[cfg(test)]
mod tests {
    use super::glob::*;
    use super::runner::*;
    use super::*;
    use crate::control::load_fitness_rules_with_skips;
    use std::collections::BTreeMap;
    use std::path::PathBuf;
    use std::time::{Duration, SystemTime};

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
        let errors: Vec<_> = report
            .issues
            .iter()
            .filter(|i| i.severity == "error")
            .collect();
        assert_eq!(errors.len(), 1, "{:?}", report.issues);
        let i = errors[0];
        assert_eq!(i.rule, "llm_no_agent");
        assert_eq!(i.line, 4, "инлайн-путь на 4-й строке: {i:?}");
        assert!(i.file.ends_with("src/llm/engine.rs"));
        assert!(i.message.contains("crate-путь 'agent'") || i.message.contains("'agent'"));
        assert!(
            !i.message.contains("tui"),
            "комментарий с упоминанием tui-модуля игнорируется: {i:?}"
        );
        // B4: `tui` не встречается среди импортов репозитория — префикс
        // вакуумный, ожидается warn (не красит гейт).
        assert!(
            report
                .issues
                .iter()
                .any(|i| i.rule == "rule_vacuous_prefix" && i.message.contains("'tui'")),
            "ожидался warn о вакуумном префиксе 'tui': {:?}",
            report.issues
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
        let errors: Vec<_> = report
            .issues
            .iter()
            .filter(|i| i.severity == "error")
            .collect();
        assert_eq!(errors.len(), 1, "{:?}", report.issues);
        assert!(
            errors[0].message.contains("вне allow-списка"),
            "{:?}",
            errors[0]
        );
        // B4: `error` не встречается среди импортов репозитория — вакуумный
        // префикс даёт warn, но гейт не красит.
        assert!(
            report
                .issues
                .iter()
                .any(|i| i.rule == "rule_vacuous_prefix" && i.message.contains("'error'")),
            "ожидался warn о вакуумном префиксе 'error': {:?}",
            report.issues
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

    /// B4 (0.4.0): привычная Java/Kotlin-нотация префикса (`io.reflectoring`)
    /// должна совпадать с координатами импорта (`io/reflectoring/...`). До
    /// фикса правило зеленело при любом коде: точечный префикс не совпадал
    /// с слэш-координатой никогда.
    #[test]
    fn fitness_dependency_direction_java_notation_prefix_matches() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        write_file(
            &repo,
            "src/main/java/io/reflectoring/domain/DomainService.java",
            "package io.reflectoring.domain;\n\
             import io.reflectoring.persistence.JpaEntity;\n",
        );
        let constraints = write_file(
            dir.path(),
            "CONSTRAINTS.yaml",
            "rules:\n\
             \x20 - name: domain_pure\n\
             \x20   type: dependency_direction\n\
             \x20   glob: '**/*.java'\n\
             \x20   forbid: ['io.reflectoring.persistence']\n",
        );
        let report = check(&repo, &constraints).unwrap();
        let errors: Vec<_> = report
            .issues
            .iter()
            .filter(|i| i.severity == "error")
            .collect();
        assert_eq!(errors.len(), 1, "{:?}", report.issues);
        assert!(
            errors[0]
                .message
                .contains("io/reflectoring/persistence/JpaEntity"),
            "{:?}",
            errors[0]
        );
    }

    /// B4 (0.4.0): префикс, которому не совпал ни один импорт репозитория, —
    /// признак неверной нотации. Находка `rule_vacuous_prefix` уровня warn (не
    /// краснит гейт), но делает молчаливое зелёное правило видимым.
    #[test]
    fn fitness_dependency_direction_vacuous_prefix_warns() {
        let dir = tempfile::tempdir().unwrap();
        let repo = deps_repo(dir.path());
        let constraints = write_file(
            dir.path(),
            "CONSTRAINTS.yaml",
            "rules:\n\
             \x20 - name: llm_no_ghost\n\
             \x20   type: dependency_direction\n\
             \x20   glob: 'src/llm/**'\n\
             \x20   forbid: ['ghost_module']\n",
        );
        let report = check(&repo, &constraints).unwrap();
        assert!(report.passed, "warn не краснит гейт: {:?}", report.issues);
        let w = report
            .issues
            .iter()
            .find(|i| i.rule == "rule_vacuous_prefix")
            .unwrap_or_else(|| {
                panic!("ожидалась находка rule_vacuous_prefix: {:?}", report.issues)
            });
        assert_eq!(w.severity, "warn");
        assert!(w.message.contains("'ghost_module'"), "{w:?}");
    }

    /// B5 (0.4.0): для JVM текстовая проверка импортов честно слепа к
    /// FQN-обращениям без `import` (например `new io.reflectoring.persistence.X()`).
    /// Вакуумный префикс JVM-правила обязан назвать это и указать выход —
    /// ArchUnit-мост (`type: archunit`), который видит байткод/типы, а не текст.
    #[test]
    fn fitness_dependency_direction_jvm_vacuous_suggests_archunit() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        write_file(
            &repo,
            "src/main/java/io/reflectoring/domain/DomainService.java",
            "package io.reflectoring.domain;\nimport java.util.List;\n",
        );
        let constraints = write_file(
            dir.path(),
            "CONSTRAINTS.yaml",
            "rules:\n\
             \x20 - name: domain_pure\n\
             \x20   type: dependency_direction\n\
             \x20   glob: '**/*.java'\n\
             \x20   forbid: ['io.reflectoring.persistence']\n",
        );
        let report = check(&repo, &constraints).unwrap();
        let w = report
            .issues
            .iter()
            .find(|i| i.rule == "rule_vacuous_prefix")
            .unwrap_or_else(|| panic!("ожидался rule_vacuous_prefix: {:?}", report.issues));
        assert!(
            w.message.contains("ArchUnit"),
            "FQN-слепота JVM-проверки не названа: {w:?}"
        );
        assert!(
            w.message.contains("io.reflectoring.persistence"),
            "префикс не назван: {w:?}"
        );
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

    // --- ADR-068: requires — SKIP при недоступном ресурсе, не PASS/FAIL ---

    /// Ресурс недоступен — правило даёт SKIP с причиной (в `requires_skipped`),
    /// команда/проверка НЕ исполняется и в находки не попадает.
    #[test]
    fn requires_absent_resource_skips_rule() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        // Правило без прав доступа: файла нет, значит при исполнении оно бы
        // дало error. Но ресурс недоступен — правила касаться нельзя.
        let constraints = write_file(
            repo,
            "CONSTRAINTS.yaml",
            "rules:\n  - name: gpu_rule\n    type: file_exists\n    path: nope\n    requires: [cuda]\n",
        );
        let opts = baseline::CheckOptions {
            resources: Some(crate::control::requires::AvailableResources::none()),
            ..baseline::CheckOptions::default()
        };
        let report = check_with_options(repo, &constraints, &opts).unwrap();
        assert_eq!(
            report.requires_skipped.len(),
            1,
            "{:?}",
            report.requires_skipped
        );
        let skip = &report.requires_skipped[0];
        assert_eq!(skip.rule, "gpu_rule");
        assert_eq!(skip.resources, vec!["cuda".to_string()]);
        assert!(skip.reason.contains("cuda"), "{}", skip.reason);
        assert!(
            skip.reason
                .contains(crate::control::requires::STAND_RUN_LINE),
            "{}",
            skip.reason
        );
        // SKIP — не находка: правило не краснеет.
        assert!(report.passed, "{}", report.summary);
        assert!(
            report.issues.iter().all(|i| i.rule != "gpu_rule"),
            "{:?}",
            report.issues
        );
    }

    /// Ресурс доступен — обычная семантика: правило исполняется (файла нет →
    /// error-находка), `requires_skipped` пуст.
    #[test]
    fn requires_present_resource_runs_rule_normally() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        let constraints = write_file(
            repo,
            "CONSTRAINTS.yaml",
            "rules:\n  - name: gpu_rule\n    type: file_exists\n    path: nope\n    requires: [cuda]\n",
        );
        let opts = baseline::CheckOptions {
            resources: Some(crate::control::requires::AvailableResources::all()),
            ..baseline::CheckOptions::default()
        };
        let report = check_with_options(repo, &constraints, &opts).unwrap();
        assert!(
            report.requires_skipped.is_empty(),
            "{:?}",
            report.requires_skipped
        );
        assert!(!report.passed, "{}", report.summary);
        assert!(
            report.issues.iter().any(|i| i.rule == "gpu_rule"),
            "{:?}",
            report.issues
        );
    }

    /// Конфиг зонда с одним заданным параметром (остальные — `None`).
    fn probe_config(
        kind: crate::control::requires::ProbeKind,
        param: &str,
        value: &str,
        note: Option<&str>,
    ) -> crate::control::requires::RequiresProbeConfig {
        use crate::control::requires::RequiresProbeConfig;
        let mut cfg = RequiresProbeConfig {
            kind,
            note: note.map(str::to_string),
            path: None,
            name: None,
            var: None,
            pattern: None,
            cmd: None,
        };
        match param {
            "path" => cfg.path = Some(value.to_string()),
            "name" => cfg.name = Some(value.to_string()),
            "var" => cfg.var = Some(value.to_string()),
            "pattern" => cfg.pattern = Some(value.to_string()),
            "cmd" => cfg.cmd = Some(value.to_string()),
            _ => unreachable!("неизвестный параметр зонда"),
        }
        cfg
    }

    /// Реестр зондов из конфига.
    fn probes(
        cfg: &BTreeMap<String, crate::control::requires::RequiresProbeConfig>,
    ) -> crate::control::requires::ProbeRegistry {
        crate::control::requires::ProbeRegistry::from_config(cfg).expect("реестр из конфига")
    }

    /// (а) Произвольный НЕ-ML ресурс через конфиг: недоступен — SKIP с
    /// настроенным note; доступен — правило исполняется обычной семантикой.
    #[test]
    fn config_declared_resource_skips_with_note_and_runs_when_available() {
        use crate::control::requires::ProbeKind;
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        std::fs::write(repo.join("present"), b"x").unwrap();
        let constraints = write_file(
            repo,
            "CONSTRAINTS.yaml",
            "rules:\n  - name: db_rule\n    type: file_exists\n    path: present\n    requires: [oracle-client]\n",
        );

        // Ресурс недоступен (бинаря нет) — SKIP с настроенным note.
        let absent = probes(&BTreeMap::from([(
            "oracle-client".to_string(),
            probe_config(
                ProbeKind::Binary,
                "name",
                "arch-requires-absent-tool-xyz",
                Some("нужен доступ к Оракулу"),
            ),
        )]));
        let opts = baseline::CheckOptions {
            probes: absent,
            ..baseline::CheckOptions::default()
        };
        let report = check_with_options(repo, &constraints, &opts).unwrap();
        assert_eq!(
            report.requires_skipped.len(),
            1,
            "{:?}",
            report.requires_skipped
        );
        let skip = &report.requires_skipped[0];
        assert_eq!(skip.rule, "db_rule");
        assert_eq!(skip.resources, vec!["oracle-client".to_string()]);
        assert!(
            skip.reason.contains("нужен доступ к Оракулу"),
            "{}",
            skip.reason
        );
        assert!(report.passed && report.issues.is_empty());

        // Ресурс доступен (`sh` есть в PATH) — правило исполняется и проходит.
        let available = probes(&BTreeMap::from([(
            "oracle-client".to_string(),
            probe_config(ProbeKind::Binary, "name", "sh", None),
        )]));
        let opts = baseline::CheckOptions {
            probes: available,
            ..baseline::CheckOptions::default()
        };
        let report = check_with_options(repo, &constraints, &opts).unwrap();
        assert!(
            report.requires_skipped.is_empty(),
            "{:?}",
            report.requires_skipped
        );
        assert!(report.passed, "{}", report.summary);
    }

    /// (б) Ресурс без зонда (ни встроенного, ни конфига) — ошибка РЕЕСТРА с
    /// подсказкой объявить `[gate.requires.<имя>]` (fail-closed).
    #[test]
    fn unknown_requires_resource_is_registry_error() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        let constraints = write_file(
            repo,
            "CONSTRAINTS.yaml",
            "rules:\n  - name: typo_rule\n    type: file_exists\n    path: a\n    requires: [quantum]\n",
        );
        let err = check_with_options(repo, &constraints, &baseline::CheckOptions::default())
            .expect_err("неизвестный ресурс");
        let text = err.to_string();
        assert!(text.contains("typo_rule"), "{text}");
        assert!(text.contains("quantum"), "{text}");
        assert!(text.contains("gate.requires.quantum"), "{text}");
    }

    /// (в) Встроенный `cuda` переопределяется конфигом: конфиг-зонд побеждает
    /// встроенный (датчик ресурса — файл-маркер, а не `/dev/nvidia0`).
    #[test]
    fn builtin_resource_is_overridden_by_config() {
        use crate::control::requires::ProbeKind;
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        let constraints = write_file(
            repo,
            "CONSTRAINTS.yaml",
            "rules:\n  - name: gpu_rule\n    type: file_exists\n    path: present\n    requires: [cuda]\n",
        );
        std::fs::write(repo.join("present"), b"x").unwrap();
        // Конфиг переопределяет cuda: доступность = наличие маркера.
        let overridden = probes(&BTreeMap::from([(
            "cuda".to_string(),
            probe_config(ProbeKind::File, "path", "gpu-marker", None),
        )]));
        // Маркера нет — SKIP (встроенный /dev/nvidia0 не консультируется).
        let opts = baseline::CheckOptions {
            probes: overridden.clone(),
            ..baseline::CheckOptions::default()
        };
        let report = check_with_options(repo, &constraints, &opts).unwrap();
        assert_eq!(
            report.requires_skipped.len(),
            1,
            "{:?}",
            report.requires_skipped
        );
        // Дефолтный note встроенного cuda заменён конфигом без note →
        // нейтральный текст с именем ресурса.
        assert!(
            report.requires_skipped[0]
                .reason
                .contains("требуется среда с ресурсом cuda"),
            "{}",
            report.requires_skipped[0].reason
        );
        // Маркер появился — ресурс доступен, правило исполняется.
        std::fs::write(repo.join("gpu-marker"), b"x").unwrap();
        let opts = baseline::CheckOptions {
            probes: overridden,
            ..baseline::CheckOptions::default()
        };
        let report = check_with_options(repo, &constraints, &opts).unwrap();
        assert!(
            report.requires_skipped.is_empty(),
            "{:?}",
            report.requires_skipped
        );
        assert!(report.passed, "{}", report.summary);
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
    use super::runner::*;
    use std::time::Duration;

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
