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
    FitnessReport, FitnessRule, LintIssue, NotApplicableRule, RequiresSkippedRule, RuleDuration,
    RuleKind, RulesFingerprint, RunnerSkippedRule, SourceCount, UntrustedSkippedRule,
    normalize_severity,
};
use crate::error::{HarnessError, Result};

mod glob;
mod runner;

pub(crate) use glob::{collect_dirs, collect_files, glob_matches};
pub(crate) use runner::collect_content_files;
use runner::{not_applicable, run_rule};

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

/// S6 (0.4.0): подсказка о байткод-слое `ArchUnit`. `Some(находка)` — в
/// репозитории есть скомпилированные классы и доступен JDK, JVM-правило слоёв
/// маппится в спек `ArchUnit`, но правила `type: archunit` в реестре нет:
/// текстовая проверка остаётся слепой к FQN-обходам (C.3: текст видит 1/5,
/// байткод — 5/5). Иначе `None`. Уровень warn — гейт не краснит, но молчаливо
/// зелёный текстовый слой становится видимым (ADR-039, ADR-068).
#[allow(clippy::question_mark)] // S6: ранний выход по is_none — не Result-ветка
fn archunit_available_hint(
    repo: &Path,
    rules: &[&FitnessRule],
    options: &baseline::CheckOptions,
) -> Option<LintIssue> {
    // Байткод-слой уже подключён (`type: archunit`) — предлагать нечего.
    if rules.iter().any(|r| matches!(r.kind, RuleKind::ArchUnit)) {
        return None;
    }
    // Нечего маппить: JVM-правил слоёв (dependency_direction/context_boundary
    // по `**/*.java`) в реестре нет — подсказка адресована им.
    if crate::archunit::spec_from_constraints(repo, rules, None)
        .rules
        .is_empty()
    {
        return None;
    }
    // Байткод-слою нечего анализировать без скомпилированных классов.
    if crate::archunit::find_classes_dir(repo).is_none() {
        return None;
    }
    // Байткод-слой не запустить без JDK. Явный снимок из опций (край/тесты)
    // сильнее детекта инструментария.
    let jdk = options.jdk.unwrap_or_else(|| {
        crate::rule_templates::runner_available(crate::rule_templates::RunnerKind::Java)
    });
    if !jdk {
        return None;
    }
    Some(LintIssue {
        file: std::path::PathBuf::from("CONSTRAINTS.yaml"),
        line: 0,
        rule: "rule_archunit_available".to_string(),
        message: "найдены скомпилированные классы и JDK — включите `type: archunit` \
                  для байткод-проверки: текстовый слой видит 1/5 классов обходов, \
                  байткод — 5/5 (C.3, ADR-039)"
            .to_string(),
        severity: "warn".to_string(),
        ..LintIssue::default()
    })
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
    // P0-1: если пустота реестра вызвана error-находками разбора (например,
    // `rule_type_missing` — запись без `type` отброшена fail-closed), не
    // прячем причину за общей ошибкой «нет правил»: отчёт с error-находкой
    // краснит гейт и называет виновника.
    let has_error_finding = resolved.findings.iter().any(|f| f.severity == "error");
    if resolved.rules.is_empty() && !has_error_finding {
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
    let (override_infos, override_findings, disabled, scoped_overrides) = evaluate_overrides(
        &resolved.overrides,
        &resolved.rules,
        constraints,
        &adr_policy,
    );

    // П5: отпечаток состава реестра — до перемещения правил.
    let fingerprint = Some(rules_fingerprint_of(&resolved.rules));
    let mut rules = resolved.rules;
    // 0.4.8: узкий override — доливаем глобы в `exclude_glob` правила, чтобы
    // та же механика исключения погасила проверку лишь на матчащихся файлах.
    // Полный override (без глобов) идёт прежним путём — через `disabled`.
    for rule in &mut rules {
        if let Some(globs) = scoped_overrides.get(&rule.name) {
            rule.exclude_glob.extend(globs.iter().cloned());
        }
    }
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
    // Волна S (ADR-070): правила со статусом N/A — объявленные условия
    // `applies_to` не выполнены, прогон пропущен. НЕ находка, гейт не ломает.
    let mut not_applicable_rules: Vec<NotApplicableRule> = Vec::new();
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
        // Волна S (ADR-070): правило, чьи объявленные условия `applies_to` не
        // выполнены, получает статус N/A — прогон пропускается, статус идёт в
        // отчёт покрытия (не PASS и не FAIL), гейт не ломается.
        if let Some(reason) = not_applicable(repo, rule)? {
            not_applicable_rules.push(NotApplicableRule {
                rule: rule.name.clone(),
                severity: normalize_severity(&rule.severity, &rule.name)?.to_string(),
                reason,
            });
            continue;
        }
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

    // S6 (0.4.0): подсказка о байткод-слое — ПОСЛЕ ratchet, чтобы не попасть
    // в долг и не потеряться в любом режиме baseline. Уровень warn: гейт не
    // краснит (passed считается от error-находок ниже).
    if let Some(hint) = archunit_available_hint(repo, rule_refs.as_slice(), options) {
        issues.push(hint);
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
    if !not_applicable_rules.is_empty() {
        // Волна S (ADR-070): N/A — не нарушение и не проверка: правило не
        // относится к этому репозиторию. В сводке отдельным счётчиком, гейт
        // не ломает.
        let _ = write!(
            summary,
            "; неприменимо (N/A): {}",
            not_applicable_rules.len()
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
        not_applicable: not_applicable_rules,
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
mod command_capture_tests;
#[cfg(test)]
mod tests;
