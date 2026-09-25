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
mod tests;
