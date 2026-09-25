//! Составляющие гейта (B1): `fitness`, `delta_guard`, `rule_weakened`,
//! `spine_lint`, `trace_check`, `sensors`, `nfr`, `evidence_verify`,
//! `model_validate`, `decision_quality`.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use super::git::{
    ConstraintsPath, GitProbe, base_rev, canonical_rel, constraints_label, git_rel_path,
    git_rev_exists, git_rev_has_path, git_show_file, has_any_registry,
};
use super::types::{GateComponent, GateFinding, GateOptions};
use crate::control::{self, Route};
use crate::{delta, evidence, nfr, trace};

/// Составляющая `fitness`: прогон `CONSTRAINTS.yaml` ([`control::check`]).
///
/// `exec` — снимок модели доверия `command_succeeds` (A3, ADR-053):
/// пропущенные по no-exec/untrusted правила переводят составляющую в SKIP
/// (см. [`exec_skip_detail`]) — «зелёный при неисполненных командах» был бы
/// молчаливой ложью.
pub(super) fn component_fitness(
    repo: &Path,
    constraints: &ConstraintsPath,
    exec: &crate::cmd_trust::ExecPolicy,
) -> GateComponent {
    if !constraints.path.is_file() {
        // T-01: реестра нет НИГДЕ (резолвер пробует корень, затем
        // `.arch-handoff/`) — это не «нечего прогонять» по недосмотру, а
        // отсутствующий вход контура. У обязательной составляющей он даёт
        // INCOMPLETE, а Stop-хук и CI — блокировку с ВНЯТНОЙ причиной, а не
        // молчаливый зелёный (раньше хук сам проверял `.arch-handoff/` и на
        // кейсе `bootstrap` — с реестром в корне — пропускал красный гейт).
        let message = if constraints.explicit || has_any_registry(repo) {
            format!(
                "нет файла ограничений {} — нечего прогонять",
                constraints.path.display()
            )
        } else {
            format!(
                "реестр правил не найден: ни {} в корне, ни {} — создайте каркас: `arch-be bootstrap`",
                control::ROOT_CONSTRAINTS_PATH,
                control::HANDOFF_CONSTRAINTS_PATH
            )
        };
        return GateComponent::skip("fitness", message);
    }
    let label = constraints_label(repo, &constraints.path);
    // T-02: расхождение двух копий реестра — не пометка в тексте, а находка.
    // Пакетная копия приоритетна (резолвер E2), поэтому расхождение означает:
    // гейт проверяет НЕ тот реестр, что лежит в корне проекта, и правила
    // корня в вердикте не участвуют вовсе. Раньше это был PASS с припиской
    // «копии реестра различаются» — то есть зелёный там, где контур проверяет
    // не то, что написал архитектор (и где `redteam` D7 не ловил ослабления).
    let divergence = registry_divergence(repo, constraints);
    let notes = mention_rule_notes(repo, &constraints.path);
    // A3: политика исполнения команд реестра — из опций гейта (CLI/MCP-край
    // выставил; библиотечный дефолт — legacy, AD-7).
    let check_options = control::baseline::CheckOptions {
        exec: exec.clone(),
        ..control::baseline::CheckOptions::default()
    };
    let detail = |summary: &str| {
        summary.to_string()
            + &constraints.drift.as_ref().map_or_else(String::new, |d| {
                format!(
                    "; {}",
                    control::constraints_drift_note(&constraints.path, d)
                )
            })
    };
    if let Some(finding) = divergence {
        let mut findings = vec![finding];
        let summary = match control::check_with_options(repo, &constraints.path, &check_options) {
            Ok(report) => {
                findings.extend(report.issues.iter().map(GateFinding::lint));
                report.summary
            }
            Err(e) => format!("сбой выполнения: {e}"),
        };
        return GateComponent::fail("fitness", detail(&summary), findings).noting(notes);
    }
    match control::check_with_options(repo, &constraints.path, &check_options) {
        Ok(report) if report.passed => {
            // Пропуски исполняемых правил — не PASS («нарушений нет»), а SKIP:
            // вердикт неполон, обязательная составляющая даёт INCOMPLETE.
            // A2: отсутствующий прогонщик (error-правила); A3: запрет доверия
            // (любое severity — пропуск по решению политики).
            if let Some(skip_detail) = exec_skip_detail(&report) {
                return GateComponent::skip(
                    "fitness",
                    format!("{} — файл: {label}", detail(&skip_detail)),
                )
                .noting(notes);
            }
            GateComponent::pass(
                "fitness",
                format!("{} — файл: {label}", detail(&report.summary)),
            )
            .noting(notes)
        }
        Ok(report) => GateComponent::fail(
            "fitness",
            format!("{} — файл: {label}", detail(&report.summary)),
            report.issues.iter().map(GateFinding::lint).collect(),
        )
        .noting(notes),
        Err(e) => GateComponent::fail("fitness", format!("сбой выполнения: {e}"), Vec::new()),
    }
}

/// Деталь составляющей `fitness`, когда исполняемые правила не прогонялись.
///
/// Два вида пропусков с разной блокирующей семантикой:
///
/// - **A2** (`runner_skipped`, нет прогонщика pytest/mvn/JDK): блокирующими
///   считаются пропуски error-правил — составляющая обязана уйти в SKIP
///   («проверить не удалось»), а не в PASS. Пропуски warn-правил вердикт не
///   меняют (их находки гейт и раньше не печатал).
/// - **A3** (`untrusted_skipped`, запрет модели доверия — no-exec или
///   несовпадающий allow-файл, ADR-053): блокирующий пропуск при ЛЮБОМ
///   severity. Пропуск по решению о доверии — событие политики, а не разрыв
///   окружения: зелёный PASS при неисполненных по политике правилах был бы
///   молчаливой ложью, а блок 3 паспорта обязан перечислить такие правила
///   (находка `command_untrusted`).
///
/// `None` — блокирующих пропусков нет.
fn exec_skip_detail(report: &control::FitnessReport) -> Option<String> {
    let blocking_runners: Vec<&control::RunnerSkippedRule> = report
        .runner_skipped
        .iter()
        .filter(|s| s.severity == "error")
        .collect();
    if blocking_runners.is_empty() && report.untrusted_skipped.is_empty() {
        return None;
    }
    let mut names: Vec<&str> = blocking_runners.iter().map(|s| s.rule.as_str()).collect();
    let mut reasons: Vec<&str> = blocking_runners.iter().map(|s| s.reason.as_str()).collect();
    for skip in &report.untrusted_skipped {
        names.push(skip.rule.as_str());
        reasons.push(skip.reason.as_str());
    }
    reasons.dedup();
    Some(format!(
        "исполняемые правила не прогонялись ({}) — {}",
        names.join(", "),
        reasons.join("; ")
    ))
}

/// Находка `registry_diverged` (T-02): в проекте две копии реестра правил, и
/// они различаются. Гейт читает пакетную (`.arch-handoff/`), значит правила
/// корневой копии — те, что видит архитектор, — в вердикте не участвуют.
///
/// Числа правил в тексте нужны, чтобы расхождение было действием, а не
/// диагнозом: «3 правила против 1» сразу говорит, какая копия устарела.
fn registry_divergence(repo: &Path, constraints: &ConstraintsPath) -> Option<GateFinding> {
    let other = constraints.drift.as_ref()?;
    let used = constraints_label(repo, &constraints.path);
    let other_label = constraints_label(repo, other);
    let count = |path: &Path| {
        control::load_constraints_resolved(path).map_or_else(
            |_| "реестр не читается".to_string(),
            |r| format!("{} правил", r.rules.len()),
        )
    };
    Some(GateFinding {
        severity: "error".into(),
        rule: Some("registry_diverged".into()),
        file: Some(used.clone()),
        line: Some(0),
        message: format!(
            "копии реестра различаются: гейт прочитал {used} ({used_count}), \
             {other_label} ({other_count}) — правила второй копии в вердикте не \
             участвуют. Синхронизируйте копии: `cp {other_label} {used}` (или \
             пересоберите пакет: `arch-be handoff … --refresh-constraints`)",
            used_count = count(&constraints.path),
            other_count = count(other),
        ),
    })
}

/// Граница вердикта `fitness` (W1, блок 2 паспорта): доля правил реестра,
/// которые доказывают НАЛИЧИЕ текста, а не поведение системы.
///
/// `must_contain`/`must_not_contain`/`each_file_must_contain` — звено
/// трассировки: они зеленеют и когда инвариант соблюдён, и когда о нём просто
/// упомянули (Н10, D11 red-team). Считается по реестру; нечитаемый реестр —
/// пустой список (составляющая и так ответит своей находкой).
fn mention_rule_notes(repo: &Path, constraints: &Path) -> Vec<String> {
    let Ok(resolved) = control::load_constraints_resolved(constraints) else {
        return Vec::new();
    };
    let total = resolved.rules.len();
    if total == 0 {
        return Vec::new();
    }
    let behaviour = resolved
        .rules
        .iter()
        .filter(|r| control::BEHAVIOUR_RULE_KINDS.contains(&r.kind.as_str()))
        .count();
    let mention = total - behaviour;
    if mention == 0 {
        return Vec::new();
    }
    let mut notes = vec![format!(
        "правил, судящих по ТЕКСТУ файла (наличие/запрет слова), — {mention} из \
         {total}; они зеленеют и когда инвариант соблюдён, и когда о нём просто \
         написали (исполняемых проверок поведения: {behaviour})"
    )];
    if let Some(line) = ads_without_behaviour(repo) {
        notes.push(line);
    }
    notes
}

/// Потолок имён инвариантов в строке блока 2 паспорта (W1/ADR-050): дальше —
/// счётчик. Полный список всегда доступен `arch-be trace`.
const MAX_AD_NAMES: usize = 8;

/// Блок 2 паспорта, вторая строка: инварианты модели, ни одно правило которых
/// не проверяет ПОВЕДЕНИЕ (несущие первыми). Модели нет — строки нет; это
/// представление, вердикт не меняется.
fn ads_without_behaviour(repo: &Path) -> Option<String> {
    let coverage = crate::rule_templates::ad_coverage(repo).ok().flatten()?;
    let uncovered = coverage.uncovered();
    if uncovered.is_empty() {
        return None;
    }
    let mut names: Vec<String> = uncovered
        .iter()
        .take(MAX_AD_NAMES)
        .map(|e| {
            if e.load_bearing {
                format!("{} (несущий)", e.ad)
            } else {
                e.ad.clone()
            }
        })
        .collect();
    let rest = uncovered.len().saturating_sub(names.len());
    if rest > 0 {
        names.push(format!("и ещё {rest}"));
    }
    Some(format!(
        "инварианты без проверки поведения: {} — их правила судят по тексту, а не по \
         поведению системы (несущие первыми; шаблон: `arch-be rules template list`)",
        names.join(", ")
    ))
}

/// Потолок записей покрытия «файл ← дельты» в детали составляющей
/// `delta_guard`: строка детали одна, полный список всегда доступен
/// `arch-be delta guard`.
const MAX_COVERAGE_NOTE: usize = 3;

/// Однострочная сводка покрытия защищённых файлов дельтами:
/// `file ← 'delta1', 'delta2'` через запятую (с потолком [`MAX_COVERAGE_NOTE`]).
fn coverage_note(report: &delta::GuardReport) -> String {
    let mut parts: Vec<String> = Vec::new();
    for (file, deltas) in report.mentions.iter().take(MAX_COVERAGE_NOTE) {
        if deltas.is_empty() {
            continue;
        }
        let quoted: Vec<String> = deltas.iter().map(|d| format!("'{d}'")).collect();
        parts.push(format!("{file} ← {}", quoted.join(", ")));
    }
    let covered = report
        .mentions
        .iter()
        .filter(|(_, d)| !d.is_empty())
        .count();
    if covered > MAX_COVERAGE_NOTE {
        parts.push(format!("… и ещё {}", covered - MAX_COVERAGE_NOTE));
    }
    parts.join("; ")
}

/// Составляющая `delta_guard`: гейт прямых правок спайна ([`delta::guard`]).
pub(super) fn component_delta_guard(
    repo: &Path,
    base: Option<&str>,
    git: &GitProbe,
) -> GateComponent {
    if !git.repo {
        return GateComponent::skip(
            "delta_guard",
            "не git-репозиторий — дифф защищённых путей недоступен".to_string(),
        );
    }
    if base.is_none() && !git.head {
        return GateComponent::skip(
            "delta_guard",
            "нет базового коммита (HEAD не существует) — дифф недоступен".to_string(),
        );
    }
    match delta::guard(repo, base, &[]) {
        Ok(report) if report.passed => {
            let mut detail = format!(
                "изменённых файлов: {}, защищённых среди них: {}",
                report.changed,
                report.protected_changed.len()
            );
            // Отчёт, а не галочка (D8): какие защищённые пути изменены и
            // какой дельтой каждый покрыт.
            if !report.protected_changed.is_empty() {
                // Запись в String не может завершиться ошибкой — игнор безопасен.
                let _ = write!(detail, " — покрытие: {}", coverage_note(&report));
            }
            GateComponent::pass("delta_guard", detail)
        }
        Ok(report) => GateComponent::fail(
            "delta_guard",
            format!(
                "правки спайна мимо дельты: {} файлов (активных дельт: {})",
                report.violations.len(),
                report.active_deltas
            ),
            report
                .violations
                .iter()
                .map(|v| {
                    GateFinding::file_only(
                        "error",
                        v.clone(),
                        if report.active_deltas == 0 {
                            "не упоминается ни в одной активной дельте — активных дельт нет"
                                .to_string()
                        } else {
                            format!(
                                "не упоминается ни в одной из {} активных дельт",
                                report.active_deltas
                            )
                        },
                    )
                })
                .collect(),
        ),
        Err(e) => GateComponent::fail("delta_guard", format!("сбой выполнения: {e}"), Vec::new()),
    }
}

/// Составляющая `rule_weakened`: анти-ослабление реестра правил относительно
/// git-базы ([`control::rule_weakened`]). Активные overrides с ADR
/// узаконивают ослабление.
///
/// Fail-closed (D6): явный `--constraints` ВНУТРИ репозитория сравнивается
/// по относительному пути (раньше абсолютный путь улетал в SKIP «вне
/// репозитория» — защита молча отключалась); путь ВНЕ репозитория — FAIL
/// с причиной, а не SKIP: анти-ослабление невозможно честно, и гейт обязан
/// это сказать. Репозиторий без базовой ревизии (нет коммитов) — честный
/// SKIP: сравнивать не с чем, это не поломка и не ослабление.
pub(super) fn component_rule_weakened(
    repo: &Path,
    constraints: &ConstraintsPath,
    base: &str,
    git: &GitProbe,
) -> GateComponent {
    if !git.repo {
        return GateComponent::skip(
            "rule_weakened",
            "не git-репозиторий — сравнение с базой недоступно".to_string(),
        );
    }
    if !constraints.path.is_file() {
        return GateComponent::skip(
            "rule_weakened",
            "нет файла ограничений — нечего сравнивать".to_string(),
        );
    }
    let rev = base_rev(base);
    if !git_rev_exists(repo, rev) {
        return GateComponent::skip(
            "rule_weakened",
            format!("базовая ревизия '{rev}' не существует (нет коммитов?) — сравнивать не с чем"),
        );
    }
    let Some(rel) = canonical_rel(repo, &constraints.path) else {
        if constraints.explicit {
            return GateComponent::fail(
                "rule_weakened",
                format!(
                    "анти-ослабление невозможно: файл ограничений {} вне репозитория {} — \
                     git-сравнение недоступно; держите реестр правил внутри репозитория \
                     (или снимите явный --constraints)",
                    constraints.path.display(),
                    repo.display()
                ),
                Vec::new(),
            );
        }
        // Дефолтный путь строится из repo.join(...) и вне репозитория
        // оказаться не может; ветка — страховка от рассинхрона резолва.
        return GateComponent::skip(
            "rule_weakened",
            format!(
                "файл ограничений {} вне репозитория — git-сравнение невозможно",
                constraints.path.display()
            ),
        );
    };
    let rel = rel.to_string_lossy();
    // Путь для `<rev>:<path>` — от корня git-репозитория (см. git_rel_path):
    // на кейсе-подкаталоге монорепо сравнение идёт с файлом самого кейса,
    // иначе читается реестр внешнего репозитория (ложные «удалено из реестра»).
    let Some(git_rel) = git_rel_path(repo, &constraints.path) else {
        return GateComponent::skip(
            "rule_weakened",
            format!(
                "файл ограничений {} вне git-репозитория — сравнение с базой недоступно",
                constraints.path.display()
            ),
        );
    };
    if !git_rev_has_path(repo, rev, &git_rel) {
        return GateComponent::skip(
            "rule_weakened",
            format!("в базе '{rev}' файла {rel} нет (новый реестр) — сравнивать не с чем"),
        );
    }
    let base_src = match git_show_file(repo, rev, &git_rel) {
        Ok(text) => text,
        Err(e) => {
            return GateComponent::fail(
                "rule_weakened",
                format!("сбой чтения базовой версии: {e}"),
                Vec::new(),
            );
        }
    };
    let current_src = match std::fs::read_to_string(&constraints.path) {
        Ok(text) => text,
        Err(e) => {
            return GateComponent::fail(
                "rule_weakened",
                format!("сбой чтения {}: {e}", constraints.path.display()),
                Vec::new(),
            );
        }
    };
    match control::rule_weakened(&current_src, &base_src, &constraints.path) {
        Ok(issues) if issues.is_empty() => GateComponent::pass(
            "rule_weakened",
            format!("реестр правил не ослаблен относительно {rev} — файл: {rel}"),
        ),
        Ok(issues) => GateComponent::fail(
            "rule_weakened",
            format!(
                "ослаблений правил относительно {rev}: {} — файл: {rel}",
                issues.len()
            ),
            issues.iter().map(GateFinding::lint).collect(),
        ),
        Err(e) => GateComponent::fail("rule_weakened", format!("сбой сравнения: {e}"), Vec::new()),
    }
}

/// Составляющая `spine_lint`: линтер `ARCHITECTURE-SPINE.md` в корне
/// репозитория ([`control::lint_spine`]); error-находки валят гейт.
pub(super) fn component_spine_lint(repo: &Path) -> GateComponent {
    let spine = repo.join("ARCHITECTURE-SPINE.md");
    if !spine.is_file() {
        return GateComponent::skip("spine_lint", "нет ARCHITECTURE-SPINE.md".to_string());
    }
    match control::lint_spine(&spine) {
        Ok(issues) => {
            let errors = issues.iter().filter(|i| i.severity == "error").count();
            if errors == 0 {
                GateComponent::pass(
                    "spine_lint",
                    format!("находок: {} (error: 0)", issues.len()),
                )
            } else {
                GateComponent::fail(
                    "spine_lint",
                    format!("находок: {} (error: {errors})", issues.len()),
                    issues.iter().map(GateFinding::lint).collect(),
                )
            }
        }
        Err(e) => GateComponent::fail("spine_lint", format!("сбой выполнения: {e}"), Vec::new()),
    }
}

/// Составляющая `trace_check`: позвенная трассируемость кейса
/// ([`trace::trace_check`]). Контракт `trace check` требует `model/` И
/// `CONSTRAINTS.yaml` в корне кейса — без любого из них SKIP (не падение).
pub(super) fn component_trace(repo: &Path, opts: &GateOptions) -> GateComponent {
    if !repo.join("model").is_dir() {
        return GateComponent::skip("trace_check", "нет каталога model/".to_string());
    }
    if !repo.join("CONSTRAINTS.yaml").is_file() {
        return GateComponent::skip(
            "trace_check",
            "нет CONSTRAINTS.yaml в корне — по контракту trace check звено fitness не проверить"
                .to_string(),
        );
    }
    match trace::trace_check_with(repo, opts.executable_required) {
        Ok(report) if !report.has_errors() => GateComponent::pass(
            "trace_check",
            format!(
                "сущностей: {}, звеньев: {}, error: 0",
                report.entities,
                report.levels.len()
            ),
        ),
        Ok(report) => {
            let errors = report
                .issues
                .iter()
                .filter(|i| i.severity == crate::model::Severity::Error)
                .count();
            GateComponent::fail(
                "trace_check",
                format!(
                    "находок: {} (error: {errors}, warn: {})",
                    report.issues.len(),
                    report.issues.len() - errors
                ),
                report
                    .issues
                    .iter()
                    .map(|i| {
                        GateFinding::ruled(
                            i.severity.to_string(),
                            i.rule.to_string(),
                            i.message.clone(),
                        )
                    })
                    .collect(),
            )
        }
        Err(e) => GateComponent::fail("trace_check", format!("сбой выполнения: {e}"), Vec::new()),
    }
}

/// Составляющая `sensors` (маршруты Standard/Critical): сенсоры спецификаций
/// [`control::sensors_check`] по `<repo>/docs/spec` — обязательные секции
/// (`required_sections`) и живость относительных ссылок (`upstream_coverage`).
///
/// Маршрутность — как у соседних nfr/evidence (Standard/Critical): сенсоры
/// проверяют СОДЕРЖАНИЕ решения (форму спецификаций), а не механику
/// протокола гейта; на маршруте Fast контур намеренно лёгкий (ADR-034) —
/// мелкая правка не должна блокироваться неполной спекой, полнота
/// обязательна со Standard. Каталога нет или он пуст — SKIP (fail-soft на
/// инфраструктуру, как у соседних составляющих); провал любого сенсора —
/// FAIL, и с ним весь гейт (класс red-team 06: удалённая секция спеки
/// раньше проходила весь контур незамеченной).
pub(super) fn component_sensors(repo: &Path) -> GateComponent {
    let spec_dir = repo.join("docs/spec");
    if !spec_dir.is_dir() {
        return GateComponent::skip(
            "sensors",
            "нет каталога docs/spec — спецификаций для сенсоров нет".to_string(),
        );
    }
    match control::sensors_check(&spec_dir) {
        Ok(results) if results.is_empty() => GateComponent::skip(
            "sensors",
            "в docs/spec нет *.md — спецификаций для сенсоров нет".to_string(),
        ),
        Ok(results) => {
            let failed: Vec<&control::SensorResult> =
                results.iter().filter(|r| !r.passed).collect();
            if failed.is_empty() {
                GateComponent::pass(
                    "sensors",
                    format!("сенсоров прогнано: {}, провалов нет", results.len()),
                )
            } else {
                GateComponent::fail(
                    "sensors",
                    format!(
                        "сенсоров прогнано: {}, провалено: {}",
                        results.len(),
                        failed.len()
                    ),
                    failed
                        .iter()
                        .map(|r| GateFinding {
                            severity: "error".to_string(),
                            rule: Some(r.sensor.clone()),
                            file: Some(r.file.display().to_string()),
                            line: None,
                            message: r.details.clone(),
                        })
                        .collect(),
                )
            }
        }
        Err(e) => GateComponent::fail("sensors", format!("сбой выполнения: {e}"), Vec::new()),
    }
}

/// Собирает находки одной NFR-проверки в общий список гейта; возвращает
/// (error, warn) этой проверки.
fn collect_nfr(
    check: &'static str,
    issues: &[nfr::NfrIssue],
    findings: &mut Vec<GateFinding>,
) -> (usize, usize) {
    let mut errors = 0;
    for i in issues {
        if i.severity == crate::model::Severity::Error {
            errors += 1;
        }
        findings.push(GateFinding::ruled(
            i.severity.to_string(),
            format!("{check}/{}", i.rule),
            i.message.clone(),
        ));
    }
    (errors, issues.len() - errors)
}

/// Составляющая `nfr` (маршруты Standard/Critical): все четыре количественные
/// проверки поверх модели ([`nfr::budget_check`], [`nfr::availability_check`],
/// [`nfr::capacity_check`], [`nfr::cost_check`]).
pub(super) fn component_nfr(repo: &Path) -> GateComponent {
    if !repo.join("model").is_dir() {
        return GateComponent::skip("nfr", "нет каталога model/ — нечего считать".to_string());
    }
    let mut findings = Vec::new();
    let mut errors = 0usize;
    let mut warns = 0usize;
    let mut ran = 0usize;
    // Отчёты проверок — разных типов, объединяет их только `issues`:
    // прогон выписан явно, без массива (тип кортежа иначе не сойдётся).
    let budget = nfr::budget_check(repo);
    let availability = nfr::availability_check(repo);
    let capacity = nfr::capacity_check(repo);
    let cost = nfr::cost_check(repo);
    for (name, issues) in [
        ("budget", budget.map(|r| r.issues)),
        ("availability", availability.map(|r| r.issues)),
        ("capacity", capacity.map(|r| r.issues)),
        ("cost", cost.map(|r| r.issues)),
    ] {
        match issues {
            Ok(issues) => {
                ran += 1;
                let (e, w) = collect_nfr(name, &issues, &mut findings);
                errors += e;
                warns += w;
            }
            Err(e) => {
                return GateComponent::fail(
                    "nfr",
                    format!("{name}: сбой выполнения: {e}"),
                    findings,
                );
            }
        }
    }
    if errors == 0 {
        GateComponent::pass(
            "nfr",
            format!("проверок: {ran}, находок: {warns} (error: 0)"),
        )
    } else {
        GateComponent::fail(
            "nfr",
            format!(
                "проверок: {ran}, находок: {} (error: {errors})",
                errors + warns
            ),
            findings,
        )
    }
}

/// Каталоги с EVIDENCE.yaml, подлежащие проверке: активные дельты
/// `changes/<name>/` (архивные — уже выпущены) и **корень репозитория**, куда
/// бандл кладёт `evidence pack` при работе по кейсу (П1 ДКА: раньше корневой
/// бандл был невидим гейту, и `evidence_verify` молча уходил в SKIP).
pub(super) fn evidence_bundle_dirs(repo: &Path) -> Vec<PathBuf> {
    let mut bundles: Vec<PathBuf> = Vec::new();
    if repo.join("EVIDENCE.yaml").is_file() {
        bundles.push(repo.to_path_buf());
    }
    let changes = repo.join("changes");
    if let Ok(rd) = std::fs::read_dir(&changes) {
        for entry in rd.flatten() {
            let dir = entry.path();
            if !dir.is_dir() || entry.file_name() == "archive" {
                continue;
            }
            if dir.join("EVIDENCE.yaml").is_file() {
                bundles.push(dir);
            }
        }
    }
    bundles.sort();
    bundles.dedup();
    bundles
}

/// Составляющая `evidence_verify` (маршруты Standard/Critical): полнота и
/// целостность хэшей evidence-бандлов активных дельт и корня репозитория.
pub(super) fn component_evidence(
    repo: &Path,
    cfg: &crate::config::EvidenceConfig,
) -> GateComponent {
    let bundles = evidence_bundle_dirs(repo);
    if bundles.is_empty() {
        return GateComponent::skip(
            "evidence_verify",
            "нет EVIDENCE.yaml ни в корне, ни в активных change-dir".to_string(),
        );
    }
    let mut failed = Vec::new();
    // Границы вердикта бандла (W1): «подпись заявлена», «семантика решения —
    // работа ревьюера». Собираются и на зелёном: именно там они и нужны.
    let mut notes: Vec<String> = Vec::new();
    for dir in &bundles {
        match evidence::verify_with(dir, cfg) {
            Ok(verdict) if verdict.passed => {
                notes.extend(verdict.not_verified.iter().cloned());
            }
            Ok(verdict) => {
                notes.extend(verdict.not_verified.iter().cloned());
                let label = dir.strip_prefix(repo).map_or_else(
                    |_| dir.display().to_string(),
                    |p| {
                        if p.as_os_str().is_empty() {
                            ".".to_string()
                        } else {
                            p.display().to_string()
                        }
                    },
                );
                failed.push(GateFinding::text(
                    "error",
                    format!("{label}: {}", verdict.summary),
                ));
                failed.extend(
                    verdict
                        .missing
                        .iter()
                        .map(|m| GateFinding::text("error", format!("  отсутствует: {m}"))),
                );
                failed.extend(
                    verdict
                        .tampered
                        .iter()
                        .map(|t| GateFinding::text("error", format!("  изменён: {t}"))),
                );
                // Н1 (ADR-041): содержание артефакта — третий класс исхода.
                failed.extend(verdict.semantics.iter().map(|f| {
                    GateFinding::ruled(
                        f.severity.clone(),
                        f.rule.clone(),
                        format!("{}: {} → {}", f.key, f.message, f.fix_hint),
                    )
                }));
            }
            Err(e) => {
                return GateComponent::fail(
                    "evidence_verify",
                    format!("{}: сбой проверки: {e}", dir.display()),
                    failed,
                );
            }
        }
    }
    notes.sort();
    notes.dedup();
    if failed.is_empty() {
        GateComponent::pass(
            "evidence_verify",
            format!("бандлов проверено: {}", bundles.len()),
        )
        .noting(notes)
    } else {
        GateComponent::fail(
            "evidence_verify",
            format!("бандлов: {}, не прошли: {}", bundles.len(), failed.len()),
            failed,
        )
        .noting(notes)
    }
}

/// Составляющая `model_validate` (Н2 волны A 0.3.4): ссылочная целостность
/// типизированной модели `model/`.
///
/// Живёт в гейте, а не только в составном ревью: без неё битая ссылка модели
/// проходила `gate`, pre-push и Stop-хук, тогда как `model validate` и
/// `review` её видели — вердикт зависел от способа вызова, а не от состояния
/// репозитория (`review` = gate + контракты, секция не считается дважды).
///
/// Нет каталога `model/` — SKIP (fail-soft: нет входа).
///
/// На маршруте Critical находка `nfr-without-verification` повышается с `warn`
/// до `error`: NFR без способа проверки на критическом маршруте не цель, а
/// пожелание.
pub(super) fn component_model_validate(repo: &Path, route: Route) -> GateComponent {
    let model_dir = repo.join("model");
    if !model_dir.is_dir() {
        return GateComponent::skip("model_validate", "нет каталога model/".to_string());
    }
    let model = match crate::model::load_model_tolerant(&model_dir) {
        Ok(m) => m,
        Err(e) => {
            return GateComponent::fail(
                "model_validate",
                format!("сбой загрузки модели: {e}"),
                Vec::new(),
            );
        }
    };
    let report = crate::model::validate(&model);
    let promoted = route == Route::Critical;
    let errors = report
        .issues
        .iter()
        .filter(|i| {
            i.severity == crate::model::Severity::Error
                || (promoted && i.rule == "nfr-without-verification")
        })
        .count();
    let findings: Vec<GateFinding> = report
        .issues
        .iter()
        .map(|i| {
            let severity = if promoted && i.rule == "nfr-without-verification" {
                "error".to_string()
            } else {
                i.severity.to_string()
            };
            GateFinding {
                severity,
                rule: Some(i.rule.to_string()),
                file: Some(i.file.display().to_string()),
                line: None,
                message: i.message.clone(),
            }
        })
        .collect();
    let detail = format!(
        "сущностей: {}, находок: {} (error: {errors}){}",
        report.entities,
        report.issues.len(),
        if promoted {
            "; Critical: nfr-without-verification → error"
        } else {
            ""
        }
    );
    // W1: зелёный здесь означает «ссылки разрешаются», а не «ссылки верны».
    // Ссылка на существующую, но не ту сущность (D6 red-team) механикой не
    // ловится и не должна — это блок 2 паспорта и состязательное ревью.
    let notes = vec![
        "ссылка разрешается в СУЩЕСТВУЮЩУЮ сущность; верна ли она по смыслу \
         (та ли это сущность) — не проверяется"
            .to_string(),
    ];
    if errors == 0 {
        GateComponent::pass("model_validate", detail).noting(notes)
    } else {
        GateComponent::fail("model_validate", detail, findings).noting(notes)
    }
}

/// Статус ADR в прозе: `- Status: Accepted` в любой из принятых форм
/// (`**Статус**:`, `## Статус`). Толерантность намеренная: ошибка разбора
/// формата не должна выглядеть как решение архитектора (Н9).
/// `pub(crate)`: тем же признаком паспорт вердикта (W1) отличает решения,
/// о которых вердикт вообще ничего не говорит.
pub(crate) fn adr_is_accepted(text: &str) -> bool {
    text.lines().take(40).any(|l| {
        let t = l.trim().trim_start_matches(['-', '*', '#', ' ']).trim();
        let lowered = t.to_lowercase();
        (lowered.starts_with("status") || lowered.starts_with("статус"))
            && lowered.contains("accepted")
    })
}

/// Составляющая `decision_quality` (Н7 волны B 0.3.4, ADR-042): качество
/// архитектурных решений по ОТЧЁТАМ рубрики-судьи.
///
/// LLM в ядро не добавляется: составляющая читает уже собранный отчёт
/// (`reports/rubric/<slug>.json`, пишут `rubric run` и MCP `rubric_verify`)
/// и сверяет записанный балл с порогом. Отчёт привязан к содержимому
/// документа своим `target_sha256` — правка ADR после оценки даёт
/// `rubric_report_stale`, а не «перенос» балла на новую редакцию.
///
/// Составляющая включается только через `[gate.required]`: по умолчанию она
/// SKIP, иначе ужесточение покраснило бы чужие пайплайны без предупреждения.
pub(super) fn component_decision_quality(
    repo: &Path,
    options: &GateOptions,
    enabled: bool,
) -> GateComponent {
    let cfg = &options.decision_quality;
    if !enabled {
        return GateComponent::skip(
            "decision_quality",
            "не включена: добавьте 'decision_quality' в [gate.required] нужного маршрута"
                .to_string(),
        );
    }
    let adr_dir = repo.join("docs/adr");
    let artifacts = crate::rubric::load_artifacts(repo);
    // E4.2: политика маршрута — одна на обе ветки (документы и досье).
    let human_policy = options.decision_policy.for_route(options.route);
    // E1.4: смысловые рубрики по досье судятся и без каталога ADR — иначе
    // решение о коде выпадало бы из decision_quality целиком.
    if !adr_dir.is_dir() && !artifacts.iter().any(|a| a.pack_kind.is_some()) {
        return GateComponent::skip(
            "decision_quality",
            "нет каталога docs/adr и отчётов по досье".to_string(),
        );
    }
    let mut adrs: Vec<PathBuf> = std::fs::read_dir(&adr_dir)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.path())
                .filter(|p| {
                    p.extension().is_some_and(|x| x.eq_ignore_ascii_case("md"))
                        && p.file_name()
                            .is_some_and(|n| n.to_string_lossy().starts_with("ADR-"))
                })
                .collect()
        })
        .unwrap_or_default();
    adrs.sort();
    let mut findings = Vec::new();
    let mut judged = 0usize;
    // Отчёты, у которых нет сырых ответов судьи: сверить балл с ответами
    // механика не может (это не находка, а граница проверки — ADR-048).
    let mut not_reproducible = 0usize;
    // Отчёты, собранные в рабочей сессии (косвенный признак, ADR-048).
    let mut session_dirty = 0usize;
    // Отчёты, чей вход помечен детектором инъекций (E2): суждение по ним
    // подтвердить нельзя — составляющая уходит в SKIP (вердикт INCOMPLETE).
    let mut injection_suspected = 0usize;
    // E3.2: доля невалидных сэмплов судьи выше порога — тоже «не подтверждено».
    let mut invalid_over_threshold = 0usize;
    // E3.3: `evidence_partial` на Critical — оговорка, которую проект не
    // принимает молча.
    let mut partial_on_critical = 0usize;
    // E5.1: два независимых судьи разошлись — решение человека.
    let mut judges_disagreed = 0usize;
    // E6.3: судья не квалифицирован на эталонном наборе, а маршрут блокирующий.
    let mut judge_unqualified = 0usize;
    for adr in &adrs {
        let Ok(text) = std::fs::read_to_string(adr) else {
            continue;
        };
        if !adr_is_accepted(&text) {
            continue;
        }
        judged += 1;
        let rel = adr.strip_prefix(repo).map_or_else(
            |_| adr.display().to_string(),
            |p| p.display().to_string().replace('\\', "/"),
        );
        let sha = crate::hash::sha256_file(adr);
        let same_target = |a: &crate::rubric::RubricArtifact| {
            a.target
                .as_deref()
                .is_some_and(|t| t == rel || t.ends_with(&rel) || rel.ends_with(t))
        };
        let by_sha = artifacts
            .iter()
            .find(|a| sha.is_some() && a.target_sha256.is_some() && a.target_sha256 == sha);
        let by_path = artifacts.iter().find(|a| same_target(a));
        let Some(artifact) = by_sha.or(by_path) else {
            findings.push(GateFinding::ruled(
                "error".to_string(),
                "rubric_report_missing".to_string(),
                format!(
                    "{rel}: нет отчёта рубрики — решение не оценено; \
                     прогоните rubric_prompt → rubric_verify (или `arch-be rubric run`)"
                ),
            ));
            continue;
        };
        // E2: вход с инъекцией обесценивает суждение о документе так же, как о
        // досье: судья мог подчиниться строке из самого документа. Составляющая
        // уйдёт в SKIP — вердикт INCOMPLETE, решение за человеком.
        if let Some(lines) = artifact
            .input_injection_lines
            .as_ref()
            .filter(|l| !l.is_empty())
        {
            injection_suspected += 1;
            findings.push(GateFinding::ruled(
                "error".to_string(),
                "rubric_input_injection".to_string(),
                format!(
                    "{rel}: во входе есть строки с паттернами prompt-инъекций ({lines:?}) — \
                     цитаты оттуда свидетельствами не засчитаны, суждение по этому входу \
                     механика подтвердить не может"
                ),
            ));
            continue;
        }
        // E3.2: невалидные сэмплы судьи. Выше порога — суждению верить нельзя,
        // решение за человеком; ниже — предупреждение, но не молчание.
        if artifact.invalid_samples_ratio > cfg.max_invalid_samples_ratio {
            invalid_over_threshold += 1;
            findings.push(GateFinding::ruled(
                "error".to_string(),
                "rubric_invalid_samples".to_string(),
                format!(
                    "{rel}: доля сэмплов судьи с баллом вне шкалы {:.0}% выше порога {:.0}% — \
                     суждению верить нельзя, решение за человеком",
                    artifact.invalid_samples_ratio * 100.0,
                    cfg.max_invalid_samples_ratio * 100.0
                ),
            ));
            continue;
        }
        if artifact.invalid_samples_ratio > 0.0 {
            findings.push(GateFinding::ruled(
                "warn".to_string(),
                "rubric_invalid_samples".to_string(),
                format!(
                    "{rel}: {:.0}% сэмплов судьи пришли с баллом вне шкалы — в расчёт не вошли",
                    artifact.invalid_samples_ratio * 100.0
                ),
            ));
        }
        // E3.3: часть свидетельств не подтвердилась. На Critical это решение
        // человека, на остальных маршрутах — предупреждение.
        let partial = artifact
            .scores
            .iter()
            .any(|s| s.has_flag(crate::rubric::CriterionFlag::EvidencePartial));
        // E4.2: политика маршрута решает, блокирует ли оговорка судьи
        // (по умолчанию — только на Critical).
        if partial && human_policy == crate::config::HumanPolicy::Human {
            // E4.5: решение архитектора по этому отчёту снимает эскалацию —
            // спорное уже разобрано человеком, и повторно звать его незачем.
            match crate::rubric::decision_for(repo, artifact) {
                Some(record) if record.decision == crate::rubric::HumanVerdict::Accept => {
                    findings.push(GateFinding::ruled(
                        "warn".to_string(),
                        "rubric_evidence_partial".to_string(),
                        format!(
                            "{rel}: часть свидетельств судьи не подтвердилась (evidence_partial), \
                             но решение архитектора принято — {} ({}){}",
                            record.decided_by,
                            record.decided_at,
                            if record.reason.is_empty() {
                                String::new()
                            } else {
                                format!(": {}", record.reason)
                            }
                        ),
                    ));
                }
                other => {
                    partial_on_critical += 1;
                    let why = match other {
                        Some(record) => format!(
                            "архитектор решение отклонил — {} ({})",
                            record.decided_by, record.decided_at
                        ),
                        None => "решение за человеком".to_string(),
                    };
                    findings.push(GateFinding::ruled(
                        "error".to_string(),
                        "rubric_evidence_partial".to_string(),
                        format!(
                            "{rel}: часть свидетельств судьи не подтвердилась (evidence_partial) — \
                             {why}"
                        ),
                    ));
                }
            }
            continue;
        }
        if partial {
            findings.push(GateFinding::ruled(
                "warn".to_string(),
                "rubric_evidence_partial".to_string(),
                format!(
                    "{rel}: часть свидетельств судьи не подтвердилась (evidence_partial) — \
                     на этом маршруте это предупреждение"
                ),
            ));
        }
        // Привязка к содержанию: отчёт обязан относиться к ЭТОЙ редакции.
        if let (Some(want), Some(got)) = (&sha, &artifact.target_sha256) {
            if want != got && same_target(artifact) {
                findings.push(GateFinding::ruled(
                    "error".to_string(),
                    "rubric_report_stale".to_string(),
                    format!(
                        "{rel}: отчёт устарел — документ изменён после оценки \
                         (было sha256:{}, стало sha256:{want})",
                        &got[..12.min(got.len())]
                    ),
                ));
                continue;
            }
        }
        if artifact.weighted_total < cfg.min_score {
            findings.push(GateFinding::ruled(
                "error".to_string(),
                "decision_quality_low".to_string(),
                format!(
                    "{rel}: {:.2}/5 ниже порога {:.2} (судья {})",
                    artifact.weighted_total, cfg.min_score, artifact.judge_model
                ),
            ));
        }
        // Воспроизводимость отчёта: балл обязан сходиться с ответами, из
        // которых он объявлен собранным (J2, ADR-048). Сверка дешёвая —
        // разбор JSON и медианы, без LLM. Правка цифры в отчёте руками даёт
        // `rubric_report_inconsistent`, правка сохранённого ответа —
        // `rubric_raw_tampered`. Нет сырых ответов (отчёт до 0.3.5) — сверка
        // невозможна, и это честно называется, а не выдаётся за проверку.
        let check = crate::judge::reverify(repo, artifact, &options.rubrics_dir, &options.judge);
        if !check.raw_saved {
            not_reproducible += 1;
        } else if !check.tampered.is_empty() {
            findings.push(GateFinding::ruled(
                "error".to_string(),
                "rubric_raw_tampered".to_string(),
                format!(
                    "{rel}: сохранённые ответы судьи правили после записи ({}) — \
                     отчёт собран из подменённых ответов",
                    check.tampered.join(", ")
                ),
            ));
        } else if !check.differences.is_empty() {
            findings.push(GateFinding::ruled(
                "error".to_string(),
                "rubric_report_inconsistent".to_string(),
                format!(
                    "{rel}: отчёт не соответствует ответам судьи — {}",
                    check.differences.join("; ")
                ),
            ));
        }
        // Уровень независимости ниже порога проекта (ADR-048). Дефолт порога —
        // `none`: находка не появляется, пока проект сам не попросит строже.
        let independence = artifact.independence.as_deref().unwrap_or_default();
        if !independence.is_empty() {
            let min = crate::judge::independence_rank(&cfg.min_independence);
            if crate::judge::independence_rank(independence) < min {
                findings.push(GateFinding::ruled(
                    "error".to_string(),
                    "judge_independence_low".to_string(),
                    format!(
                        "{rel}: независимость судьи «{}» ниже порога «{}» — поднимите её \
                         запуском судьи самим Spine (`arch-be rubric run`) или судьёй \
                         другого семейства",
                        crate::judge::independence_label(independence),
                        crate::judge::independence_label(&cfg.min_independence),
                    ),
                ));
            }
        }
        // Судейство шло в рабочей сессии: судья мог видеть контекст автора.
        // Это КОСВЕННЫЙ признак и примечание паспорта, а не находка (ADR-048).
        if let Some(prov) = &artifact.provenance {
            if prov.mode() == crate::judge::MODE_DECLARED
                && prov.session_calls_before > options.judge.clean_session_max_calls
            {
                session_dirty += 1;
            }
        }
        // Судья и автор — разные модели одного семейства: «другая модель» не
        // значит «другой взгляд» — слепые зоны у семейства общие (ADR-048).
        // Сравнение — по нормализованным меткам и семействам; две разные
        // НЕИЗВЕСТНЫЕ метки разными семействами и остаются.
        let author_for_family = artifact.author_model.as_deref().unwrap_or_default();
        if !author_for_family.trim().is_empty()
            && !crate::judge::same_label(author_for_family, &artifact.judge_model)
        {
            let judge_family =
                crate::judge::family_of(&artifact.judge_model, &options.judge.families);
            let same = crate::judge::family_key(author_for_family, &options.judge.families)
                == crate::judge::family_key(&artifact.judge_model, &options.judge.families);
            if same {
                let severity = if cfg.require_distinct_family {
                    "error"
                } else {
                    "warn"
                };
                let message = if judge_family == crate::judge::FAMILY_UNKNOWN {
                    format!(
                        "{rel}: семейство судьи и автора не опознано (метки '{}' и '{}') —                          судья независим только по названию",
                        artifact.judge_model, author_for_family
                    )
                } else {
                    format!(
                        "{rel}: судья и автор — разные модели одного семейства '{judge_family}'                          ({} и {}) — слепые зоны у семейства общие",
                        artifact.judge_model, author_for_family
                    )
                };
                findings.push(GateFinding::ruled(
                    severity.to_string(),
                    "judge_same_family".to_string(),
                    message,
                ));
            }
        }
        // Метка автора из вызова разошлась с шапкой документа: в отчёт пошло
        // значение из шапки (оно закоммичено вместе с документом), но само
        // расхождение читателю назвать нужно — это признак того, что автора
        // «вспоминали» уже после написания (J3, ADR-048).
        if let ("header", Some(declared)) = (
            artifact.author_source.as_deref().unwrap_or_default(),
            artifact.author_model_declared.as_deref(),
        ) {
            findings.push(GateFinding::ruled(
                "warn".to_string(),
                "author_model_mismatch".to_string(),
                format!(
                    "{rel}: вызов назвал автора '{declared}', в шапке документа '{}' —                      в отчёт пошло значение из шапки",
                    artifact.author_model.as_deref().unwrap_or("не указан")
                ),
            ));
        }
        // «Автор = судья»: вердикт судьи о своей же работе не независим.
        let author_missing = artifact
            .author_model
            .as_deref()
            .is_none_or(|a| a.trim().is_empty());
        if author_missing || artifact.author_model.as_deref() == Some(&artifact.judge_model) {
            findings.push(GateFinding::ruled(
                if cfg.require_distinct_judge && !author_missing {
                    "error".to_string()
                } else {
                    "warn".to_string()
                },
                "judge_is_author".to_string(),
                if author_missing {
                    format!(
                        "{rel}: author_model в отчёте не указан — независимость судьи не подтверждена \
                         (судья {})",
                        artifact.judge_model
                    )
                } else {
                    format!(
                        "{rel}: судья и автор — одна модель ({}) — оценка не независима",
                        artifact.judge_model
                    )
                },
            ));
        }
    }
    // E1.4: отчёты смысловых рубрик по досье (`code_vs_spine` и другие) —
    // такой же предмет аудита, как ADR: правка кода или инварианта после
    // оценки (`rubric_report_stale`), подмена сохранённого ответа судьи
    // (`rubric_raw_tampered`) и правка балла руками (`rubric_report_inconsistent`)
    // обязаны быть видны гейту. Отчёт по досье без сырых ответов — warn-находка
    // `rubric_report_unreproducible`: read-only контур MCP их не пишет, и делать
    // это провалом значило бы краснить настройку, а не дефект решения.
    let mut pack_judged = 0usize;
    let mut pack_unreproducible = 0usize;
    for artifact in artifacts.iter().filter(|a| a.pack_kind.is_some()) {
        let Some(subject) = artifact.subject.as_deref() else {
            continue;
        };
        pack_judged += 1;
        let what = format!("{subject} [{}]", artifact.rubric);
        // E6.3: на блокирующем маршруте судья обязан быть квалифицирован на
        // эталонном наборе — иначе «промах» неотличим от «не умеет».
        if human_policy == crate::config::HumanPolicy::Human && cfg.require_qualified_judge {
            let state = crate::rubric::qualification(repo, &artifact.rubric, &artifact.judge_model);
            if !matches!(state, crate::rubric::Qualification::Qualified(_)) {
                judge_unqualified += 1;
                findings.push(GateFinding::ruled(
                    "error".to_string(),
                    "judge_unqualified".to_string(),
                    format!(
                        "{what}: {} — `arch-be rubric qualify`",
                        crate::rubric::refusal_reason(&state, &artifact.judge_model)
                    ),
                ));
                continue;
            }
        }
        // E5.1: два независимых судьи разошлись — механика не выбирает, кому
        // верить, и отправляет решение человеку.
        if let Some(second) = artifact.second_judge.as_ref().filter(|s| !s.agreement) {
            judges_disagreed += 1;
            findings.push(GateFinding::ruled(
                "error".to_string(),
                "judge_disagreement".to_string(),
                format!(
                    "{what}: второй судья {} разошёлся с первым ({}); решение человека",
                    second.model,
                    if second.differences.is_empty() {
                        "оценки не сошлись".to_string()
                    } else {
                        second.differences.join("; ")
                    }
                ),
            ));
            continue;
        }
        let check = crate::judge::reverify(repo, artifact, &options.rubrics_dir, &options.judge);
        if !check.raw_saved {
            pack_unreproducible += 1;
            findings.push(GateFinding::ruled(
                "warn".to_string(),
                "rubric_report_unreproducible".to_string(),
                format!(
                    "{what}: сырые ответы судьи не сохранены — сверить балл с ответами механика \
                     не может; прогоните рубрику самим Spine (`arch-be rubric run --pack …`)"
                ),
            ));
            continue;
        }
        // Порядок силы: подмена сохранённого ответа — прямое свидетельство
        // правки следа, и она называется раньше устаревания досье: оба могут
        // быть истинны одновременно (ответ правили, а источник уже изменился).
        if !check.tampered.is_empty() {
            findings.push(GateFinding::ruled(
                "error".to_string(),
                "rubric_raw_tampered".to_string(),
                format!(
                    "{what}: сохранённые ответы судьи правили после записи ({}) — отчёт собран \
                     из подменённых ответов",
                    check.tampered.join(", ")
                ),
            ));
        } else if let Some(reason) = &check.stale {
            findings.push(GateFinding::ruled(
                "error".to_string(),
                "rubric_report_stale".to_string(),
                format!(
                    "{what}: досье изменено после оценки — {reason}; оцените субъект заново \
                     (`arch-be rubric run --pack …`)"
                ),
            ));
        } else if !check.differences.is_empty() {
            findings.push(GateFinding::ruled(
                "error".to_string(),
                "rubric_report_inconsistent".to_string(),
                format!(
                    "{what}: отчёт не соответствует ответам судьи — {}",
                    check.differences.join("; ")
                ),
            ));
        }
    }
    if judged == 0 && pack_judged == 0 {
        return GateComponent::skip(
            "decision_quality",
            "принятых ADR (Status: Accepted) и отчётов по досье не найдено".to_string(),
        );
    }
    let errors = findings.iter().filter(|f| f.severity == "error").count();
    let detail = format!(
        "принятых ADR: {judged}, отчётов по досье: {pack_judged}, находок: {} (error: {errors}); порог {:.2}",
        findings.len(),
        cfg.min_score
    );
    // W1: балл — это суждение LLM-судьи, а не свойство решения. Механика
    // сверяет число с порогом и свежесть отчёта; качество самого суждения и
    // верность решения она не подтверждает (D10 red-team — блок 2 паспорта).
    let mut notes = vec![
        "балл рубрики — суждение LLM-судьи; механика сверяет число с порогом \
         и привязку отчёта к редакции документа, но не качество суждения и не \
         верность самого решения"
            .to_string(),
    ];
    if findings
        .iter()
        .any(|f| f.rule.as_deref() == Some("judge_is_author"))
    {
        notes.push(
            "независимость судьи не подтверждена: для части документов судья \
             совпадает с автором либо автор в отчёте не указан (judge_is_author)"
                .to_string(),
        );
    }
    if session_dirty > 0 {
        notes.push(format!(
            "судейство части отчётов ({session_dirty}) шло в рабочей сессии: до выдачи \
             промпта судьи в ней было больше {max} вызовов — судья мог видеть контекст \
             автора. Это косвенный признак, а не доказательство: сам по себе он ничего \
             не значит",
            max = options.judge.clean_session_max_calls
        ));
    }
    if not_reproducible > 0 {
        notes.push(format!(
            "часть отчётов невоспроизводима: сырые ответы судьи не сохранены ({not_reproducible}) \
             — сверить балл с ответами механика не может, она сверяет только число с порогом"
        ));
    }
    if pack_unreproducible > 0 {
        notes.push(format!(
            "часть отчётов по досье невоспроизводима: сырые ответы судьи не сохранены \
             ({pack_unreproducible}) — read-only контур MCP файлов не пишет, и в отчёте \
             остаются одни хэши"
        ));
    }
    let unconfirmed = injection_suspected
        + invalid_over_threshold
        + partial_on_critical
        + judges_disagreed
        + judge_unqualified;
    if unconfirmed > 0 {
        // E2/E3: «проверить нельзя», а не «нарушено» и не «нечего проверять» —
        // SKIP обязательной составляющей даёт вердикт INCOMPLETE (exit 3).
        return GateComponent::skip_with_findings(
            "decision_quality",
            format!(
                "{detail}; решений, которые механика не подтверждает: {unconfirmed} \
                 (вход с инъекцией: {injection_suspected}, невалидных сэмплов сверх порога: \
                 {invalid_over_threshold}, evidence_partial на Critical: {partial_on_critical}, \
                 расхождение судей: {judges_disagreed}, судья не квалифицирован: \
                 {judge_unqualified}) — решение за человеком"
            ),
            findings,
        )
        .noting(notes);
    }
    if errors == 0 {
        GateComponent::pass_with_findings("decision_quality", detail, findings).noting(notes)
    } else {
        GateComponent::fail("decision_quality", detail, findings).noting(notes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gate::testkit::*;

    use crate::gate::{
        GateOptions, GateOutcome, GateReport, GateRequirements, GateStatus, render, run, run_opts,
        run_with,
    };

    // --- Н7: качество решений как составляющая гейта (ADR-042) -------------

    /// Репозиторий с одним Accepted-ADR и (опционально) отчётом рубрики.
    fn make_quality_repo(dir: &Path, score: Option<f64>, author: Option<&str>) {
        make_gate_repo(dir);
        std::fs::create_dir_all(dir.join("docs/adr")).expect("mkdir adr");
        let adr = dir.join("docs/adr/ADR-001-reshenie.md");
        std::fs::write(
            &adr,
            "# ADR-001. Решение\n\n- Date: 2026-09-19\n- Status: Accepted\n\n## Context\n\nПричина.\n\n## Alternatives\n\nВариант Б.\n\n## Consequences\n\nЦена.\n",
        )
        .expect("adr");
        git(dir, &["add", "."]);
        // `--allow-empty`: тест может пересобрать фикстуру в том же каталоге.
        git(dir, &["commit", "-q", "--allow-empty", "-m", "adr"]);
        if let Some(total) = score {
            let sha = crate::hash::sha256_file(&adr).expect("sha");
            let artifact = serde_json::json!({
                "schema": crate::rubric::RUBRIC_REPORT_SCHEMA,
                "rubric": "adr_quality",
                "target": "docs/adr/ADR-001-reshenie.md",
                "target_sha256": sha,
                "judge_model": "judge-x",
                "author_model": author,
                "weighted_total": total,
                "verdict": "OK",
                "unstable": false,
                "evidence_not_found": 0,
                "judged_at": "2026-09-19T10:00:00+00:00",
            });
            let reports = dir.join(crate::rubric::RUBRIC_REPORTS_DIR);
            std::fs::create_dir_all(&reports).expect("mkdir reports");
            std::fs::write(
                reports.join("ADR-001-reshenie.json"),
                serde_json::to_string_pretty(&artifact).expect("json"),
            )
            .expect("write report");
        }
    }
    /// С включённой составляющей требования передаются явно.
    fn with_quality(route: Route) -> GateRequirements {
        let mut req = GateRequirements::default();
        let list = match route {
            Route::Fast => &mut req.fast,
            Route::Standard => &mut req.standard,
            Route::Critical => &mut req.critical,
        };
        list.push("decision_quality".to_string());
        req
    }

    /// По умолчанию составляющая — SKIP: включение только через `[gate.required]`.
    #[test]
    fn decision_quality_is_skip_by_default() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path();
        make_quality_repo(dir, Some(1.0), Some("judge-x"));
        let report = run_with(
            dir,
            Some(Route::Fast),
            None,
            None,
            (1, 4),
            &GateRequirements::default(),
        )
        .expect("gate");
        assert_eq!(
            status_of(&report, "decision_quality"),
            GateStatus::Skip,
            "ADR с низким баллом не краснит гейт без явного включения"
        );
        assert_eq!(report.outcome, GateOutcome::Pass);
    }

    /// Слабый ADR (картонный: секции есть, содержания нет) — ниже порога.
    #[test]
    fn decision_quality_fails_below_threshold() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path();
        make_quality_repo(dir, Some(1.30), Some("judge-x"));
        let report = run_with(
            dir,
            Some(Route::Fast),
            None,
            None,
            (1, 4),
            &with_quality(Route::Fast),
        )
        .expect("gate");
        assert_eq!(status_of(&report, "decision_quality"), GateStatus::Fail);
        let findings = &report
            .components
            .iter()
            .find(|c| c.name == "decision_quality")
            .expect("comp")
            .findings;
        assert!(
            findings
                .iter()
                .any(|f| f.rule.as_deref() == Some("decision_quality_low")),
            "{findings:?}"
        );
        // Сильный ADR (3.90) — тот же порог пройден.
        make_quality_repo(dir, Some(3.90), Some("judge-x"));
        let ok = run_with(
            dir,
            Some(Route::Fast),
            None,
            None,
            (1, 4),
            &with_quality(Route::Fast),
        )
        .expect("gate");
        assert_eq!(status_of(&ok, "decision_quality"), GateStatus::Pass);
    }

    /// Отчёт, снятый с прежней редакции ADR, обесценивается.
    #[test]
    fn decision_quality_flags_stale_report() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path();
        make_quality_repo(dir, Some(4.5), Some("judge-x"));
        // Правка документа после оценки — при том же пути.
        let adr = dir.join("docs/adr/ADR-001-reshenie.md");
        let mut text = std::fs::read_to_string(&adr).expect("read");
        text.push_str("\nДописано после оценки.\n");
        std::fs::write(&adr, text).expect("write");
        let report = run_with(
            dir,
            Some(Route::Fast),
            None,
            None,
            (1, 4),
            &with_quality(Route::Fast),
        )
        .expect("gate");
        assert_eq!(status_of(&report, "decision_quality"), GateStatus::Fail);
        let findings = &report
            .components
            .iter()
            .find(|c| c.name == "decision_quality")
            .expect("comp")
            .findings;
        assert!(
            findings
                .iter()
                .any(|f| f.rule.as_deref() == Some("rubric_report_stale")),
            "{findings:?}"
        );
    }

    /// Судья = автор (или автор не указан) — отдельная находка.
    #[test]
    fn decision_quality_warns_when_judge_is_author() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path();
        for author in [Some("judge-x"), None] {
            make_quality_repo(dir, Some(4.5), author);
            let report = run_with(
                dir,
                Some(Route::Fast),
                None,
                None,
                (1, 4),
                &with_quality(Route::Fast),
            )
            .expect("gate");
            let findings = &report
                .components
                .iter()
                .find(|c| c.name == "decision_quality")
                .expect("comp")
                .findings;
            let comp = report
                .components
                .iter()
                .find(|c| c.name == "decision_quality")
                .expect("comp");
            let hit = findings
                .iter()
                .find(|f| f.rule.as_deref() == Some("judge_is_author"))
                .unwrap_or_else(|| {
                    panic!(
                        "нет judge_is_author для {author:?}: {:?} / {}",
                        findings, comp.detail
                    )
                });
            assert_eq!(hit.severity, "warn", "{findings:?}");
            // warn не краснит составляющую: балл выше порога.
            assert_eq!(status_of(&report, "decision_quality"), GateStatus::Pass);
        }
    }

    /// Нет отчёта вовсе — решение не оценено (дефект D9: «картонный» ADR).
    #[test]
    fn decision_quality_requires_report_when_enabled() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path();
        make_quality_repo(dir, None, None);
        let report = run_with(
            dir,
            Some(Route::Fast),
            None,
            None,
            (1, 4),
            &with_quality(Route::Fast),
        )
        .expect("gate");
        assert_eq!(status_of(&report, "decision_quality"), GateStatus::Fail);
        assert!(
            report
                .components
                .iter()
                .find(|c| c.name == "decision_quality")
                .expect("comp")
                .findings
                .iter()
                .any(|f| f.rule.as_deref() == Some("rubric_report_missing")),
            "ожидалась rubric_report_missing"
        );
    }
    /// Н2: битая ссылка модели краснит гейт — без каталога `model/` секция
    /// честно SKIP, вердикт тот же, что у `model validate`.
    #[test]
    fn gate_fails_on_broken_model_link() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path();
        make_gate_repo(dir);
        write_model(
            dir,
            &[
                ("CMP-001", "depends_on: [CMP-002]"),
                ("CMP-002", "depends_on: []"),
            ],
        );
        // Модель коммитится: с Н4 delta guard видит и неотслеживаемые файлы,
        // и незакоммиченная модель — это правка спайна без дельты.
        git(dir, &["add", "."]);
        git(dir, &["commit", "-q", "-m", "model"]);
        let limits = (1, 4);
        let clean = run_with(
            dir,
            Some(Route::Standard),
            None,
            None,
            limits,
            &GateRequirements::default(),
        )
        .expect("gate");
        assert_eq!(status_of(&clean, "model_validate"), GateStatus::Pass);
        assert_ne!(clean.outcome, GateOutcome::Fail);
        // Конверт вердикта содержит составляющую (П7).
        let envelope = clean.envelope_json();
        assert!(
            envelope["components"]
                .as_array()
                .expect("components")
                .iter()
                .any(|c| c["name"] == "model_validate"),
            "{envelope}"
        );
        // Ссылка на несуществующую сущность — гейт краснеет.
        write_model(
            dir,
            &[
                ("CMP-001", "depends_on: [CMP-099]"),
                ("CMP-002", "depends_on: []"),
            ],
        );
        git(dir, &["add", "."]);
        git(dir, &["commit", "-q", "-m", "broken link"]);
        let broken = run_with(
            dir,
            Some(Route::Standard),
            None,
            None,
            limits,
            &GateRequirements::default(),
        )
        .expect("gate");
        assert_eq!(status_of(&broken, "model_validate"), GateStatus::Fail);
        assert!(!broken.passed);
        assert_eq!(broken.outcome, GateOutcome::Fail);
    }

    /// Без каталога `model/` составляющая пропускается fail-soft, и на
    /// маршруте, где она НЕ обязательна, это не даёт INCOMPLETE.
    #[test]
    fn gate_skips_model_validate_without_model_dir() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path();
        make_gate_repo(dir);
        let report = run_with(
            dir,
            Some(Route::Fast),
            None,
            None,
            (1, 4),
            &GateRequirements::default(),
        )
        .expect("gate");
        assert_eq!(status_of(&report, "model_validate"), GateStatus::Skip);
        assert_eq!(
            report.outcome,
            GateOutcome::Pass,
            "{:?}",
            report.not_checked
        );
        // А на Standard она обязательна: SKIP даёт INCOMPLETE (exit 3).
        let standard = run_with(
            dir,
            Some(Route::Standard),
            None,
            None,
            (1, 4),
            &GateRequirements::default(),
        )
        .expect("gate");
        assert_eq!(standard.outcome, GateOutcome::Incomplete);
        assert!(
            standard.not_checked.contains(&"model_validate".to_string()),
            "{:?}",
            standard.not_checked
        );
    }

    /// На маршруте Critical NFR без способа проверки — error, а не warn.
    #[test]
    fn critical_route_promotes_nfr_without_verification() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path();
        make_gate_repo(dir);
        write_model(
            dir,
            &[
                ("CMP-001", "depends_on: []"),
                ("NFR-001", "verification: \"\"\naffects: [CMP-001]"),
            ],
        );
        git(dir, &["add", "."]);
        git(dir, &["commit", "-q", "-m", "model"]);
        let standard = run_with(
            dir,
            Some(Route::Standard),
            None,
            None,
            (1, 4),
            &GateRequirements::default(),
        )
        .expect("gate");
        let comp = standard
            .components
            .iter()
            .find(|c| c.name == "model_validate")
            .expect("comp");
        assert_eq!(
            status_of(&standard, "model_validate"),
            GateStatus::Pass,
            "{:?}",
            comp.findings
        );
        let critical = run_with(
            dir,
            Some(Route::Critical),
            None,
            None,
            (1, 4),
            &GateRequirements::default(),
        )
        .expect("gate");
        assert_eq!(status_of(&critical, "model_validate"), GateStatus::Fail);
    }
    #[test]
    fn gate_fails_when_rule_removed_to_green_the_gate() {
        let tmp = tempfile::tempdir().expect("tmp");
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).expect("mkdir");
        make_gate_repo(&repo);
        // Агент удалил правило no_pan, чтобы пройти гейт.
        std::fs::write(
            repo.join(".arch-handoff/CONSTRAINTS.yaml"),
            "rules:\n  - name: spine_present\n    type: file_exists\n    path: \"ARCHITECTURE-SPINE.md\"\n    severity: error\n",
        )
        .expect("ослабленный constraints");
        let report = run(&repo, None, None, None, (1, 4)).expect("гейт");
        assert!(!report.passed, "ослабление обязано валить гейт");
        assert_eq!(status_of(&report, "rule_weakened"), GateStatus::Fail);
        // delta guard молчит: его дефолт защищает корневой CONSTRAINTS.yaml,
        // а правка — в .arch-handoff/ (ослабление ловит именно rule_weakened).
        assert_eq!(status_of(&report, "delta_guard"), GateStatus::Pass);
        let text = render(&report);
        assert!(text.contains("rule_weakened"), "{text}");
        assert!(text.contains("no_pan"), "{text}");
        assert!(text.contains("Итог: FAIL"), "{text}");
        assert!(
            text.contains("Гейт поймал 1 нарушений до ревью — исправьте и перепроверьте"),
            "квитанция ценности при FAIL: {text}"
        );
    }

    #[test]
    fn gate_active_override_legalizes_rule_removal() {
        let tmp = tempfile::tempdir().expect("tmp");
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).expect("mkdir");
        make_gate_repo(&repo);
        // Дельта покрывает правку CONSTRAINTS.yaml, override узаконивает
        // удаление правила (гейт «только через ADR»).
        std::fs::write(
            repo.join(".arch-handoff/CONSTRAINTS.yaml"),
            "rules:\n  - name: spine_present\n    type: file_exists\n    path: \"ARCHITECTURE-SPINE.md\"\n    severity: error\noverrides:\n  - rule: no_pan\n    adr: ADR-007\n    until: \"2999-01\"\n",
        )
        .expect("constraints с override");
        let delta_dir = repo.join("changes/drop-pan");
        std::fs::create_dir_all(&delta_dir).expect("mkdir delta");
        std::fs::write(
            delta_dir.join("DELTA.md"),
            "# Дельта\n\nСнимаем правило no_pan по ADR-007: CONSTRAINTS.yaml.\n",
        )
        .expect("delta");
        let report = run(&repo, None, None, None, (1, 4)).expect("гейт");
        assert_eq!(
            status_of(&report, "rule_weakened"),
            GateStatus::Pass,
            "активный override узаконивает: {}",
            render(&report)
        );
        assert_eq!(status_of(&report, "delta_guard"), GateStatus::Pass);
        assert!(report.passed, "{}", render(&report));
    }

    #[test]
    fn gate_fails_on_broken_constraints_yaml() {
        let tmp = tempfile::tempdir().expect("tmp");
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).expect("mkdir");
        make_gate_repo(&repo);
        // Битый YAML при наличии входа — FAIL, а не молчаливый пропуск.
        std::fs::write(repo.join(".arch-handoff/CONSTRAINTS.yaml"), "{битый yaml")
            .expect("битый constraints");
        let report = run(&repo, None, None, None, (1, 4)).expect("гейт");
        assert!(!report.passed);
        assert_eq!(status_of(&report, "fitness"), GateStatus::Fail);
        assert_eq!(status_of(&report, "rule_weakened"), GateStatus::Fail);
    }

    #[test]
    fn gate_fails_on_fitness_violation() {
        let tmp = tempfile::tempdir().expect("tmp");
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).expect("mkdir");
        make_gate_repo(&repo);
        // Удаляем обязательный файл: fitness FAIL, ослаблений правил нет.
        std::fs::remove_file(repo.join("ARCHITECTURE-SPINE.md")).expect("remove spine");
        let report = run(&repo, None, None, None, (1, 4)).expect("гейт");
        assert!(!report.passed);
        assert_eq!(status_of(&report, "fitness"), GateStatus::Fail);
        // spine_lint пропущен: файла нет — входа нет (fail-soft).
        assert_eq!(status_of(&report, "spine_lint"), GateStatus::Skip);
    }

    /// A2: пропуск error-правила из-за отсутствия прогонщика — составляющая
    /// fitness уходит в SKIP (деталь для `GateComponent::skip`), warn-пропуск
    /// вердикт не меняет. Детерминировано: отчёт собран руками, окружение не
    /// участвует.
    #[test]
    fn runner_skip_detail_only_for_error_severity() {
        let skip = |rule: &str, severity: &str| control::RunnerSkippedRule {
            rule: rule.to_string(),
            severity: severity.to_string(),
            runners: vec!["pytest".to_string()],
            reason: "нет прогонщика pytest: `python3` есть, но нет модуля pytest \
                     (`python3 -m pip install pytest`)"
                .to_string(),
        };
        let report = |runner_skipped: Vec<control::RunnerSkippedRule>| control::FitnessReport {
            repo: PathBuf::from("."),
            passed: true,
            issues: Vec::new(),
            summary: String::new(),
            durations: Vec::new(),
            inherited: Vec::new(),
            overrides: Vec::new(),
            baseline: None,
            skipped: Vec::new(),
            changed_since: None,
            changed_files: None,
            skipped_unknown: Vec::new(),
            runner_skipped,
            untrusted_skipped: Vec::new(),
            fingerprint: None,
        };
        let detail = exec_skip_detail(&report(vec![
            skip("r_err", "error"),
            skip("r_warn", "warn"),
        ]))
        .expect("error-пропуск блокирует");
        assert!(detail.contains("r_err"), "{detail}");
        assert!(
            !detail.contains("r_warn"),
            "warn-пропуск не блокирует: {detail}"
        );
        assert!(detail.contains("pip install pytest"), "{detail}");
        assert!(
            exec_skip_detail(&report(vec![skip("r_warn", "warn")])).is_none(),
            "warn-пропуски не меняют вердикт"
        );
        assert!(exec_skip_detail(&report(Vec::new())).is_none());
    }

    /// A3: пропуск по модели доверия (no-exec/untrusted) блокирует при ЛЮБОМ
    /// severity — иначе блок 3 паспорта не увидел бы warn-правила, не
    /// исполненные по решению политики. Отчёт собран руками (детерминированно).
    #[test]
    fn untrusted_skip_detail_blocks_at_any_severity() {
        let report =
            |untrusted_skipped: Vec<control::UntrustedSkippedRule>| control::FitnessReport {
                repo: PathBuf::from("."),
                passed: true,
                issues: Vec::new(),
                summary: String::new(),
                durations: Vec::new(),
                inherited: Vec::new(),
                overrides: Vec::new(),
                baseline: None,
                skipped: Vec::new(),
                changed_since: None,
                changed_files: None,
                skipped_unknown: Vec::new(),
                runner_skipped: Vec::new(),
                untrusted_skipped,
                fingerprint: None,
            };
        let skip = |rule: &str, severity: &str| control::UntrustedSkippedRule {
            rule: rule.to_string(),
            severity: severity.to_string(),
            reason: crate::cmd_trust::deny_reason_text(crate::cmd_trust::DenyReason::NoExec),
        };
        let detail = exec_skip_detail(&report(vec![skip("warn_rule", "warn")]))
            .expect("warn-пропуск по доверию блокирует (A3)");
        assert!(detail.contains("warn_rule"), "{detail}");
        assert!(
            detail.contains(crate::cmd_trust::COMMAND_UNTRUSTED),
            "маркер находки в детали: {detail}"
        );
        assert!(exec_skip_detail(&report(Vec::new())).is_none());
    }

    /// A3 в гейте целиком: реестр с command-правилом при no-exec — составляющая
    /// `fitness` SKIP с маркером `command_untrusted` и именем правила, вердикт
    /// INCOMPLETE (fitness обязательна на всех маршрутах), а блок 3 паспорта
    /// перечисляет пропущенное с причиной. Команда при этом НЕ исполняется
    /// (маяк не создан). Политика инжектируется опцией — окружение не трогается.
    #[test]
    fn gate_no_exec_skips_fitness_and_passport_lists_it() {
        let tmp = tempfile::tempdir().expect("tmp");
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(repo.join(".arch-handoff")).expect("mkdir");
        std::fs::write(
            repo.join(".arch-handoff/CONSTRAINTS.yaml"),
            "rules:\n  - name: touched\n    type: command_succeeds\n    \
             command: 'touch marker.txt'\n    severity: error\n  - name: spine_present\n    \
             type: file_exists\n    path: \"ARCHITECTURE-SPINE.md\"\n    severity: error\n",
        )
        .expect("constraints");
        std::fs::write(repo.join("ARCHITECTURE-SPINE.md"), "# Spine\n").expect("spine");
        let options = GateOptions {
            exec: crate::cmd_trust::ExecPolicy {
                no_exec: true,
                trust_file: None,
            },
            ..GateOptions::default()
        };
        let report = run_opts(
            &repo,
            Some(crate::control::Route::Fast),
            None,
            None,
            (50, 50),
            &GateRequirements::default(),
            &options,
        )
        .expect("гейт");
        let fitness = report
            .components
            .iter()
            .find(|c| c.name == "fitness")
            .expect("составляющая fitness");
        assert_eq!(fitness.status, GateStatus::Skip, "{}", fitness.detail);
        assert!(
            fitness.detail.contains(crate::cmd_trust::COMMAND_UNTRUSTED),
            "{}",
            fitness.detail
        );
        assert!(fitness.detail.contains("touched"), "{}", fitness.detail);
        assert_eq!(
            report.outcome,
            GateOutcome::Incomplete,
            "обязательная составляющая в SKIP — зелёный неполон"
        );
        assert!(!repo.join("marker.txt").exists(), "команда не исполнялась");
        // Паспорт (блок 3): пропущенное по доверию перечислено с причиной.
        let passport = crate::passport::Passport::build(&report, &repo);
        let fitness_nc = passport
            .not_checked
            .iter()
            .find(|n| n.name == "fitness")
            .expect("fitness в блоке 3 паспорта");
        assert!(
            fitness_nc
                .reason
                .contains(crate::cmd_trust::COMMAND_UNTRUSTED),
            "{}",
            fitness_nc.reason
        );
        assert!(
            fitness_nc.reason.contains("touched"),
            "{}",
            fitness_nc.reason
        );
        assert!(fitness_nc.required, "fitness обязательна для Fast");
    }
    // --- составляющая `sensors` (D5) ----------------------------------------
    /// Пишет спецификацию в `<repo>/docs/spec/<name>`.
    fn write_spec(repo: &Path, name: &str, text: &str) {
        let dir = repo.join("docs/spec");
        std::fs::create_dir_all(&dir).expect("mkdir spec");
        std::fs::write(dir.join(name), text).expect("spec");
    }

    #[test]
    fn gate_sensors_fail_when_spec_lost_required_section() {
        let tmp = tempfile::tempdir().expect("tmp");
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).expect("mkdir");
        make_gate_repo(&repo);
        // Red-team 06: из спеки удалена секция «## Критерии приёмки».
        write_spec(
            &repo,
            "payments.md",
            "# Спека\n\n## Проблема\nТекст.\n\n## Риски\nТекст.\n",
        );
        // Маршрут Standard: sensors в контуре (на Fast её нет — см. ниже).
        let report = run(&repo, Some(Route::Standard), None, None, (1, 4)).expect("гейт");
        assert!(!report.passed, "{}", render(&report));
        assert_eq!(status_of(&report, "sensors"), GateStatus::Fail);
        let text = render(&report);
        assert!(text.contains("required_sections"), "{text}");
        assert!(text.contains("## Критерии приёмки"), "{text}");
        // На маршруте Fast составляющей sensors нет вовсе (лёгкий контур).
        let fast = run(&repo, Some(Route::Fast), None, None, (1, 4)).expect("гейт fast");
        assert!(
            !fast.components.iter().any(|c| c.name == "sensors"),
            "маршрут Fast — sensors вне гейта"
        );
    }

    #[test]
    fn gate_sensors_skip_without_docs_spec_and_pass_on_full_spec() {
        let tmp = tempfile::tempdir().expect("tmp");
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).expect("mkdir");
        make_gate_repo(&repo);
        // Без docs/spec — честный SKIP (fail-soft на инфраструктуру).
        let report = run(&repo, Some(Route::Standard), None, None, (1, 4)).expect("гейт");
        assert_eq!(status_of(&report, "sensors"), GateStatus::Skip);
        assert_eq!(
            report.outcome,
            GateOutcome::Incomplete,
            "{}",
            render(&report)
        );
        // Полная спека (все секции REQUIRED_SECTIONS, ссылок нет) — PASS.
        write_spec(
            &repo,
            "payments.md",
            "# Спека\n\n## Проблема\nТекст.\n\n## Критерии приёмки\n- [ ] тест.\n\n## Риски\nТекст.\n",
        );
        let report = run(&repo, Some(Route::Standard), None, None, (1, 4)).expect("гейт");
        assert_eq!(
            status_of(&report, "sensors"),
            GateStatus::Pass,
            "{}",
            render(&report)
        );
        // Требование к sensors выполнено: его нет в «не проверено». Итог всё
        // ещё INCOMPLETE — trace_check/nfr обязательны на Standard, а model/
        // в этом репозитории нет.
        assert!(!report.not_checked.iter().any(|n| n == "sensors"));
        assert!(report.not_checked.iter().any(|n| n == "trace_check"));
        assert_eq!(
            report.outcome,
            GateOutcome::Incomplete,
            "{}",
            render(&report)
        );
    }
    // --- пути ограничений и fail-closed `rule_weakened` (D6) -----------------

    /// Ослабленный реестр: правило `no_pan` удалено (антикейс «зеленения» гейта).
    const WEAKENED_CONSTRAINTS: &str = "rules:\n  - name: spine_present\n    type: file_exists\n    path: \"ARCHITECTURE-SPINE.md\"\n    severity: error\n";

    #[test]
    fn gate_explicit_constraints_inside_repo_keeps_weakened_protection() {
        let tmp = tempfile::tempdir().expect("tmp");
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).expect("mkdir");
        make_gate_repo(&repo);
        // Явный --constraints АБСОЛЮТНЫМ путём внутри репозитория (red-team,
        // наблюдение 1): раньше секция уходила в SKIP «вне репозитория».
        let explicit = repo.join(".arch-handoff/CONSTRAINTS.yaml");
        std::fs::write(&explicit, WEAKENED_CONSTRAINTS).expect("ослабленный constraints");
        let report = run(&repo, None, None, Some(&explicit), (1, 4)).expect("гейт");
        assert!(!report.passed, "{}", render(&report));
        assert_eq!(status_of(&report, "rule_weakened"), GateStatus::Fail);
        let text = render(&report);
        assert!(text.contains("no_pan"), "{text}");
    }

    #[test]
    fn gate_explicit_constraints_outside_repo_fails_closed() {
        let tmp = tempfile::tempdir().expect("tmp");
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).expect("mkdir");
        make_gate_repo(&repo);
        // Файл ограничений ВНЕ репозитория: анти-ослабление невозможно —
        // FAIL с причиной, а не молчаливый SKIP.
        let outside_dir = tmp.path().join("outside");
        std::fs::create_dir_all(&outside_dir).expect("mkdir outside");
        let outside = outside_dir.join("CONSTRAINTS.yaml");
        std::fs::write(&outside, WEAKENED_CONSTRAINTS).expect("внешний constraints");
        let report = run(&repo, None, None, Some(&outside), (1, 4)).expect("гейт");
        assert!(!report.passed, "{}", render(&report));
        assert_eq!(status_of(&report, "rule_weakened"), GateStatus::Fail);
        let component = report
            .components
            .iter()
            .find(|c| c.name == "rule_weakened")
            .expect("составляющая");
        assert!(
            component.detail.contains("анти-ослабление невозможно")
                && component.detail.contains("вне репозитория"),
            "{}",
            component.detail
        );
        // Fitness при этом честно прогоняет внешний файл (вход есть).
        assert_eq!(status_of(&report, "fitness"), GateStatus::Pass);
    }

    #[test]
    fn gate_on_repo_subdirectory_compares_against_case_file_not_outer_registry() {
        // Регрессия D6b: кейс-подкаталог внутри чужого монорепо (как кейсы/
        // внутри spine-core). `<rev>:<path>` резолвится git'ом от toplevel —
        // сравнение обязано идти с файлом кейса, а не с реестром внешнего репо.
        let tmp = tempfile::tempdir().expect("tmp");
        let outer = tmp.path().join("outer");
        let case = outer.join("cases").join("demo");
        std::fs::create_dir_all(&case).expect("mkdir case");
        // Ловушка: реестр внешнего репозитория с правилом, которого нет у кейса.
        std::fs::write(
            outer.join("CONSTRAINTS.yaml"),
            "rules:\n  - name: outer_only_rule\n    type: file_exists\n    path: \"OUTER.md\"\n    severity: error\n",
        )
        .expect("outer constraints");
        std::fs::write(
            case.join("CONSTRAINTS.yaml"),
            "rules:\n  - name: spine_present\n    type: file_exists\n    path: \"ARCHITECTURE-SPINE.md\"\n    severity: error\n  - name: no_pan\n    type: must_not_contain\n    glob: \"**/*.py\"\n    pattern: 'PAN'\n    severity: error\n",
        )
        .expect("case constraints");
        std::fs::write(case.join("ARCHITECTURE-SPINE.md"), "# Spine\n").expect("spine");
        git(&outer, &["init", "-q"]);
        git(&outer, &["add", "."]);
        git(&outer, &["commit", "-q", "-m", "init"]);
        // Ослабление реестра кейса в рабочем дереве: правило no_pan удалено.
        std::fs::write(case.join("CONSTRAINTS.yaml"), WEAKENED_CONSTRAINTS)
            .expect("ослабленный constraints");
        let report = run(&case, None, None, None, (1, 4)).expect("гейт");
        assert_eq!(
            status_of(&report, "rule_weakened"),
            GateStatus::Fail,
            "{}",
            render(&report)
        );
        let text = render(&report);
        assert!(text.contains("no_pan"), "{text}");
        assert!(
            !text.contains("outer_only_rule"),
            "сравнение с реестром внешнего репо даёт ложные находки: {text}"
        );
    }

    #[test]
    fn gate_falls_back_to_root_constraints_yaml() {
        let tmp = tempfile::tempdir().expect("tmp");
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).expect("mkdir");
        // Кейс без handoff-пакета: реестр правил — КОРНЕВОЙ CONSTRAINTS.yaml
        // (как кейс 011 и сам этот репозиторий).
        std::fs::write(
            repo.join("CONSTRAINTS.yaml"),
            "rules:\n  - name: spine_present\n    type: file_exists\n    path: \"ARCHITECTURE-SPINE.md\"\n    severity: error\n  - name: no_pan\n    type: must_not_contain\n    glob: \"**/*.py\"\n    pattern: 'PAN'\n    severity: error\n",
        )
        .expect("constraints");
        std::fs::write(repo.join("ARCHITECTURE-SPINE.md"), "# Spine\n").expect("spine");
        git(&repo, &["init", "-q"]);
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-q", "-m", "init"]);
        // До ослабления: fitness прогоняется по корневому файлу (не SKIP),
        // rule_weakened сравнивает по корневому пути.
        let report = run(&repo, None, None, None, (1, 4)).expect("гейт");
        assert!(report.passed, "{}", render(&report));
        assert_eq!(status_of(&report, "fitness"), GateStatus::Pass);
        let fitness = report
            .components
            .iter()
            .find(|c| c.name == "fitness")
            .expect("составляющая");
        assert!(
            fitness.detail.contains("файл: CONSTRAINTS.yaml"),
            "секция печатает использованный путь: {}",
            fitness.detail
        );
        let weakened = report
            .components
            .iter()
            .find(|c| c.name == "rule_weakened")
            .expect("составляющая");
        assert_eq!(weakened.status, GateStatus::Pass);
        assert!(
            weakened.detail.contains("CONSTRAINTS.yaml"),
            "{}",
            weakened.detail
        );
        // Ослабление корневого реестра ловится тем же анти-ослаблением.
        std::fs::write(repo.join("CONSTRAINTS.yaml"), WEAKENED_CONSTRAINTS)
            .expect("ослабленный constraints");
        let report = run(&repo, None, None, None, (1, 4)).expect("гейт");
        assert!(!report.passed, "{}", render(&report));
        assert_eq!(status_of(&report, "rule_weakened"), GateStatus::Fail);
    }

    #[test]
    fn gate_repo_without_commits_skips_weakened_honestly() {
        let tmp = tempfile::tempdir().expect("tmp");
        let repo = tmp.path().join("repo");
        make_uncommitted_repo(&repo);
        let report = run(&repo, None, None, None, (1, 4)).expect("гейт");
        // Базы для сравнения нет: rule_weakened честно SKIP; П1 — INCOMPLETE,
        // потому что маршрут Critical требует эту составляющую.
        assert!(!report.passed, "{}", render(&report));
        assert_eq!(report.outcome, GateOutcome::Incomplete);
        assert_eq!(status_of(&report, "rule_weakened"), GateStatus::Skip);
        let component = report
            .components
            .iter()
            .find(|c| c.name == "rule_weakened")
            .expect("составляющая");
        assert!(
            component.detail.contains("не существует"),
            "{}",
            component.detail
        );
    }

    // --- T-02: две копии реестра правил ----------------------------------

    /// Расхождение двух копий реестра — находка `registry_diverged` (error), а
    /// не пометка в тексте. Гейт читает пакетную копию первой, поэтому
    /// расхождение означает: правила корневой копии — те, что написал
    /// архитектор, — в вердикте не участвуют вовсе. Раньше `fitness` при этом
    /// оставался PASS с припиской «копии реестра различаются».
    #[test]
    fn diverged_registries_are_a_finding_not_a_note() {
        let tmp = tempfile::tempdir().expect("tmp");
        let repo = tmp.path().join("repo");
        make_gate_repo(&repo);
        let packet_registry = repo.join(".arch-handoff/CONSTRAINTS.yaml");
        let packet_rules = std::fs::read_to_string(&packet_registry).expect("реестр пакета");
        // Корневая копия — другой реестр (в жизни так делает `bootstrap`:
        // реестр в корне, а `handoff` кладёт в пакет заготовку).
        std::fs::write(
            repo.join("CONSTRAINTS.yaml"),
            "rules:\n  - id: C-001\n    name: readme_exists\n    type: file_exists\n    path: \"README.md\"\n    severity: error\n",
        )
        .expect("корневой реестр");
        std::fs::write(repo.join("README.md"), "# Проект\n").expect("readme");

        let report = run(&repo, None, None, None, (1, 4)).expect("гейт");
        let fitness = report
            .components
            .iter()
            .find(|c| c.name == "fitness")
            .expect("составляющая");
        assert_eq!(status_of(&report, "fitness"), GateStatus::Fail);
        let finding = fitness
            .findings
            .iter()
            .find(|f| f.rule.as_deref() == Some("registry_diverged"))
            .unwrap_or_else(|| panic!("нет находки registry_diverged: {}", render(&report)));
        assert_eq!(finding.severity, "error");
        // Текст — действие, а не диагноз: названы оба пути и оба числа правил.
        assert!(finding.message.contains("2 правил"), "{}", finding.message);
        assert!(finding.message.contains("1 правил"), "{}", finding.message);
        assert!(finding.message.contains("cp "), "{}", finding.message);

        // Синхронизация копий снимает находку.
        std::fs::copy(repo.join("CONSTRAINTS.yaml"), &packet_registry).expect("синхронизация");
        let report = run(&repo, None, None, None, (1, 4)).expect("гейт");
        assert_eq!(status_of(&report, "fitness"), GateStatus::Pass);
        assert!(
            !render(&report).contains("registry_diverged"),
            "{}",
            render(&report)
        );

        // Приоритет копий виден в числах первой проверки: «прочитано 2»
        // относится к ПАКЕТНОЙ копии (`make_gate_repo`), а не к корневой.
        assert_eq!(
            packet_rules.lines().filter(|l| l.contains("name:")).count(),
            2
        );
    }
    #[test]
    fn gate_delta_guard_detail_shows_coverage() {
        let tmp = tempfile::tempdir().expect("tmp");
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).expect("mkdir");
        make_gate_repo(&repo);
        // Защищённая правка, покрытая активной дельтой: деталь секции —
        // отчёт «что изменено и чем покрыто», а не голая галочка (D8).
        std::fs::write(repo.join("ARCHITECTURE-SPINE.md"), "# Spine v2\n").expect("edit");
        let delta_dir = repo.join("changes/spine-update");
        std::fs::create_dir_all(&delta_dir).expect("mkdir delta");
        std::fs::write(
            delta_dir.join("DELTA.md"),
            "# Дельта\n\nПравим ARCHITECTURE-SPINE.md (v2).\n",
        )
        .expect("delta");
        let report = run(&repo, None, None, None, (1, 4)).expect("гейт");
        assert_eq!(
            status_of(&report, "delta_guard"),
            GateStatus::Pass,
            "{}",
            render(&report)
        );
        let component = report
            .components
            .iter()
            .find(|c| c.name == "delta_guard")
            .expect("составляющая");
        assert!(
            component
                .detail
                .contains("покрытие: ARCHITECTURE-SPINE.md ← 'spine-update'"),
            "{}",
            component.detail
        );
    }
    // --- П1/П4/П7: честный зелёный, храповик маршрута, конверт вердикта ------

    /// П1 (Д1): evidence-бандл в КОРНЕ репозитория виден гейту — раньше он
    /// искался только в `changes/<имя>/` и составляющая молча уходила в SKIP.
    #[test]
    fn gate_finds_evidence_bundle_in_repo_root() {
        let tmp = tempfile::tempdir().expect("tmp");
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).expect("mkdir");
        make_gate_repo(&repo);
        // Неполный бандл Critical в корне: обязательных артефактов нет.
        std::fs::write(
            repo.join("EVIDENCE.yaml"),
            "route: Critical\npacked_at: \"2026-09-19T00:00:00+00:00\"\nitems: []\n",
        )
        .expect("bundle");
        let report = run(&repo, Some(Route::Critical), None, None, (1, 4)).expect("гейт");
        assert_eq!(
            status_of(&report, "evidence_verify"),
            GateStatus::Fail,
            "корневой бандл обязан проверяться: {}",
            render(&report)
        );
        assert_eq!(report.outcome, GateOutcome::Fail, "{}", render(&report));
    }

    // --- E1.4: отчёты по досье в decision_quality --------------------------

    /// Репозиторий с отчётом смысловой рубрики по досье (`code_vs_spine`) и,
    /// опционально, сырым ответом судьи под slug'ом досье. Каталога `docs/adr`
    /// здесь нет намеренно: составляющая обязана судить решение о коде и без
    /// принятых ADR.
    fn write_code_pack_report(dir: &Path, raw: Option<(&str, &str)>) {
        make_gate_repo(dir);
        let reports = dir.join(crate::rubric::RUBRIC_REPORTS_DIR);
        std::fs::create_dir_all(&reports).expect("mkdir reports");
        let artifact = serde_json::json!({
            "schema": crate::rubric::RUBRIC_REPORT_SCHEMA,
            "rubric": "code_invariant_conformance",
            "judge_model": "judge-x",
            "author_model": "agent-y",
            "weighted_total": 4.5,
            "verdict": "OK",
            "unstable": false,
            "evidence_not_found": 0,
            "judged_at": "2026-09-25T10:00:00+00:00",
            "pack_kind": "code_vs_spine",
            "subject": "src/control.rs",
            "pack_sha256": "a".repeat(64),
            "inputs": [{"path": "src/control.rs", "sha256": "b".repeat(64), "role": "subject"}],
        });
        std::fs::write(
            reports.join("control--code_vs_spine.json"),
            serde_json::to_string_pretty(&artifact).expect("json"),
        )
        .expect("write pack report");
        if let Some((text, recorded_sha)) = raw {
            let raw_dir = dir
                .join(crate::judge::RUBRIC_RAW_DIR)
                .join("control--code_vs_spine");
            std::fs::create_dir_all(&raw_dir).expect("mkdir raw");
            let answer = serde_json::json!({
                "schema": crate::judge::RUBRIC_RAW_SCHEMA,
                "rubric": "code_invariant_conformance",
                "sample": 1,
                "judge_model": "judge-x",
                "sha256": recorded_sha,
                "dropped": false,
                "text": text,
                "saved_at": "2026-09-25T10:00:00+00:00",
            });
            std::fs::write(
                raw_dir.join(crate::judge::raw_file_name(1)),
                serde_json::to_string_pretty(&answer).expect("json"),
            )
            .expect("write raw");
        }
    }

    /// E1.4: отчёт по досье без сырых ответов судьи — находка гейта
    /// (`rubric_report_unreproducible`, warn: read-only контур MCP файлов не
    /// пишет, и провалом это краснило бы настройку, а не решение).
    #[test]
    fn decision_quality_flags_unreproducible_code_report() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path();
        write_code_pack_report(dir, None);
        let report = run_with(
            dir,
            Some(Route::Fast),
            None,
            None,
            (1, 4),
            &with_quality(Route::Fast),
        )
        .expect("gate");
        let comp = report
            .components
            .iter()
            .find(|c| c.name == "decision_quality")
            .expect("comp");
        assert!(
            comp.detail.contains("отчётов по досье: 1"),
            "отчёт по досье посчитан: {}",
            comp.detail
        );
        assert!(
            comp.findings
                .iter()
                .any(|f| f.rule.as_deref() == Some("rubric_report_unreproducible")),
            "{:?}",
            comp.findings
        );
        assert_eq!(
            status_of(&report, "decision_quality"),
            GateStatus::Pass,
            "warn не краснит составляющую: {}",
            render(&report)
        );
    }

    /// E1.4: подмена сохранённого ответа судьи по рубрике кода — находка
    /// гейта с провалом составляющей, как и у отчёта по документу.
    #[test]
    fn decision_quality_flags_tampered_code_raw_answer() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path();
        let text = "{\"scores\": [], \"verdict\": \"ok\"}";
        write_code_pack_report(dir, Some((text, &"f".repeat(64))));
        let report = run_with(
            dir,
            Some(Route::Fast),
            None,
            None,
            (1, 4),
            &with_quality(Route::Fast),
        )
        .expect("gate");
        let comp = report
            .components
            .iter()
            .find(|c| c.name == "decision_quality")
            .expect("comp");
        assert!(
            comp.findings
                .iter()
                .any(|f| f.rule.as_deref() == Some("rubric_raw_tampered")),
            "{:?}",
            comp.findings
        );
        assert_eq!(status_of(&report, "decision_quality"), GateStatus::Fail);
    }

    /// E2: вход документа помечен prompt-инъекцией — «проверить нельзя, нужен
    /// человек». Составляющая уходит в SKIP с находкой `rubric_input_injection`,
    /// вердикт гейта — INCOMPLETE (exit 3): ни PASS (судья мог подчиниться
    /// строке), ни FAIL (документ ничего не нарушил).
    #[test]
    fn decision_quality_input_injection_makes_gate_incomplete() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path();
        make_quality_repo(dir, Some(4.5), Some("judge-x"));
        let path = dir
            .join(crate::rubric::RUBRIC_REPORTS_DIR)
            .join("ADR-001-reshenie.json");
        let mut artifact: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).expect("отчёт")).expect("JSON");
        artifact["input_injection_lines"] = serde_json::json!([2]);
        std::fs::write(
            &path,
            serde_json::to_string_pretty(&artifact).expect("json"),
        )
        .expect("write report");
        let report = run_with(
            dir,
            Some(Route::Fast),
            None,
            None,
            (1, 4),
            &with_quality(Route::Fast),
        )
        .expect("gate");
        let comp = report
            .components
            .iter()
            .find(|c| c.name == "decision_quality")
            .expect("comp");
        assert_eq!(comp.status, GateStatus::Skip, "{}", render(&report));
        assert!(
            comp.findings
                .iter()
                .any(|f| f.rule.as_deref() == Some("rubric_input_injection")),
            "{:?}",
            comp.findings
        );

        assert_eq!(
            report.outcome,
            GateOutcome::Incomplete,
            "{}",
            render(&report)
        );
    }

    /// Патч отчёта `make_quality_repo` полями E3: доля невалидных сэмплов и
    /// метки критериев. Возвращает путь отчёта.
    fn patch_quality_report(dir: &Path, invalid_ratio: Option<f64>, flags: &[&str]) -> PathBuf {
        let path = dir
            .join(crate::rubric::RUBRIC_REPORTS_DIR)
            .join("ADR-001-reshenie.json");
        let mut artifact: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).expect("отчёт")).expect("JSON");
        if let Some(ratio) = invalid_ratio {
            artifact["invalid_samples_ratio"] = serde_json::json!(ratio);
        }
        if !flags.is_empty() {
            artifact["scores"] = serde_json::json!([{
                "criterion_id": "context",
                "weight": 1.0,
                "score": 4,
                "rationale": "цитата",
                "samples": [4],
                "stdev": 0.0,
                "flags": flags,
                "evidence_unconfirmed_ratio": 0.0,
                "invalid_samples": 0,
                "checked": [],
            }]);
        }
        std::fs::write(
            &path,
            serde_json::to_string_pretty(&artifact).expect("json"),
        )
        .expect("write report");
        path
    }

    /// Пройденная квалификация судьи `judge-x` на рубрике
    /// `code_invariant_conformance` (рубрика по досье): на блокирующем маршруте
    /// её требует E6.3, когда проект включил `require_qualified_judge`.
    fn write_passing_qualification(dir: &Path) {
        let rubric = crate::rubric::load(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("assets/rubrics/code_invariant_conformance.yaml"),
        )
        .expect("рубрика code_invariant_conformance");
        let set = crate::rubric::QualificationSet {
            dir: std::path::PathBuf::from("/набор"),
            cases: Vec::new(),
            sha256: "e".repeat(64),
        };
        let outcome = |truth: crate::rubric::Truth| crate::rubric::CaseOutcome {
            file: "code/x.py".to_string(),
            class: "ignored_key".to_string(),
            truth,
            decision: Some(match truth {
                crate::rubric::Truth::Defective => crate::rubric::RubricDecision::Fail,
                crate::rubric::Truth::Clean => crate::rubric::RubricDecision::Pass,
            }),
            weighted_total: 4.0,
            flags: Vec::new(),
            error: None,
        };
        let report = crate::rubric::build_qualification_report(
            &rubric,
            "judge-x",
            &set,
            vec![
                outcome(crate::rubric::Truth::Defective),
                outcome(crate::rubric::Truth::Clean),
            ],
            1,
        );
        assert!(report.passed, "{:?}", report.failures);
        crate::rubric::write_qualification(dir, &report).expect("квалификация");
    }

    /// Прогон гейта с настройками допуска судьи (E6.3).
    fn run_quality_on(dir: &Path, route: Route, require_qualified: bool) -> GateReport {
        let mut options = GateOptions::default();
        options.decision_quality.require_qualified_judge = require_qualified;
        options.route = Some(route);
        crate::gate::verdict::run_inner(
            dir,
            Some(route),
            None,
            None,
            (1, 4),
            &with_quality(route),
            &options,
        )
        .expect("гейт")
    }

    /// E6.3: с включённым допуском судья без квалификации на Critical не
    /// проходит (SKIP → INCOMPLETE), после пройденной — проходит; с выключенным
    /// флагом проверки нет (поведение 0.3.8).
    #[test]
    fn decision_quality_requires_qualification_when_enabled() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path();
        // Отчёт по досье рубрики кода: эталонный набор существует именно для неё.
        write_code_pack_report(dir, None);
        let before = run_quality_on(dir, Route::Critical, true);
        assert_eq!(
            status_of(&before, "decision_quality"),
            GateStatus::Skip,
            "{}",
            render(&before)
        );
        let comp = before
            .components
            .iter()
            .find(|c| c.name == "decision_quality")
            .expect("comp");
        assert!(
            comp.findings
                .iter()
                .any(|f| f.rule.as_deref() == Some("judge_unqualified")),
            "{:?}",
            comp.findings
        );
        write_passing_qualification(dir);
        let after = run_quality_on(dir, Route::Critical, true);
        assert_eq!(
            status_of(&after, "decision_quality"),
            GateStatus::Pass,
            "{}",
            render(&after)
        );
        let off = run_quality_on(dir, Route::Critical, false);
        assert_eq!(
            status_of(&off, "decision_quality"),
            GateStatus::Pass,
            "без флага проверки нет: {}",
            render(&off)
        );
    }

    /// E3.2: доля сэмплов судьи с баллом вне шкалы выше порога — суждению
    /// верить нельзя: SKIP с находкой `rubric_invalid_samples`, вердикт
    /// INCOMPLETE (exit 3), решение за человеком.
    #[test]
    fn decision_quality_invalid_samples_above_threshold_escalates() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path();
        make_quality_repo(dir, Some(4.5), Some("judge-x"));
        patch_quality_report(dir, Some(0.8), &[]);
        let report = run_with(
            dir,
            Some(Route::Fast),
            None,
            None,
            (1, 4),
            &with_quality(Route::Fast),
        )
        .expect("gate");
        let comp = report
            .components
            .iter()
            .find(|c| c.name == "decision_quality")
            .expect("comp");
        assert_eq!(comp.status, GateStatus::Skip, "{}", render(&report));
        assert!(
            comp.findings
                .iter()
                .any(|f| f.rule.as_deref() == Some("rubric_invalid_samples")
                    && f.severity == "error"),
            "{:?}",
            comp.findings
        );
        assert_eq!(
            report.outcome,
            GateOutcome::Incomplete,
            "{}",
            render(&report)
        );
    }

    /// E3.2, контрпроба: доля ниже порога — только предупреждение, вердикт
    /// остаётся PASS: один сбойный сэмпл не повод блокировать решение.
    #[test]
    fn decision_quality_invalid_samples_below_threshold_is_warn() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path();
        make_quality_repo(dir, Some(4.5), Some("judge-x"));
        patch_quality_report(dir, Some(0.25), &[]);
        let report = run_with(
            dir,
            Some(Route::Fast),
            None,
            None,
            (1, 4),
            &with_quality(Route::Fast),
        )
        .expect("gate");
        let comp = report
            .components
            .iter()
            .find(|c| c.name == "decision_quality")
            .expect("comp");
        assert_eq!(comp.status, GateStatus::Pass, "{}", render(&report));
        assert!(
            comp.findings.iter().any(
                |f| f.rule.as_deref() == Some("rubric_invalid_samples") && f.severity == "warn"
            ),
            "{:?}",
            comp.findings
        );
        assert_eq!(report.outcome, GateOutcome::Pass);
    }

    /// E3.3: `evidence_partial` на Critical — решение за человеком (SKIP →
    /// INCOMPLETE), на Fast — предупреждение с вердиктом PASS.
    #[test]
    fn decision_quality_evidence_partial_follows_route() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path();
        make_quality_repo(dir, Some(4.5), Some("judge-x"));
        write_passing_qualification(dir);
        write_passing_qualification(dir);
        patch_quality_report(dir, None, &["evidence_partial"]);
        // Critical: оговорку судьи принимает человек.
        let critical = run_with(
            dir,
            Some(Route::Critical),
            None,
            None,
            (1, 4),
            &with_quality(Route::Critical),
        )
        .expect("gate critical");
        let comp = critical
            .components
            .iter()
            .find(|c| c.name == "decision_quality")
            .expect("comp");
        assert_eq!(comp.status, GateStatus::Skip, "{}", render(&critical));
        assert!(
            comp.findings
                .iter()
                .any(|f| f.rule.as_deref() == Some("rubric_evidence_partial")),
            "{:?}",
            comp.findings
        );
        assert_eq!(
            critical.outcome,
            GateOutcome::Incomplete,
            "{}",
            render(&critical)
        );
        // Fast: то же состояние — предупреждение, вердикт PASS.
        let fast = run_with(
            dir,
            Some(Route::Fast),
            None,
            None,
            (1, 4),
            &with_quality(Route::Fast),
        )
        .expect("gate fast");
        let comp = fast
            .components
            .iter()
            .find(|c| c.name == "decision_quality")
            .expect("comp");
        assert_eq!(comp.status, GateStatus::Pass, "{}", render(&fast));
        assert_eq!(fast.outcome, GateOutcome::Pass, "{}", render(&fast));
    }
}
