//! Прогонщики правил (волна B1, декомпозиция `exec.rs` под C-33): исполнение
//! одного правила ([`run_rule`]), `command_succeeds` (таймаут через
//! [`crate::proc`], доверие через [`crate::cmd_trust`], прогонщики через
//! [`crate::rule_templates`]), `max_age`, `dependency_direction` (ADR-029),
//! `context_boundary` (ADR-030) и подготовка content-правил.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::ExitStatus;
use std::time::{Duration, SystemTime};

use regex::Regex;

use super::baseline;
use super::glob::{collect_dirs, collect_files, glob_matches};
use crate::control::rules::{DEFAULT_MANIFEST_GLOBS, manifest_deps};
use crate::control::types::{
    FitnessRule, LintIssue, RuleKind, RunnerSkippedRule, UntrustedSkippedRule, normalize_severity,
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
pub(super) fn run_rule(
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
                for edge in crate::imports::extract_imports(rel, &content)? {
                    let module = &edge.module;
                    // Относительные импорты TS/JS (`./…`) не выражаются в
                    // координатах модулей — их разрешает `context_boundary`.
                    if module.starts_with('.') {
                        continue;
                    }
                    match &mode {
                        DepsMode::Forbid(forbid) => {
                            if let Some(entry) = forbid
                                .iter()
                                .find(|e| crate::imports::module_prefix_match(module, e))
                            {
                                issue(
                                    PathBuf::from(rel),
                                    edge.line,
                                    format!(
                                        "dependency_direction: запрещённая зависимость '{module}' (forbid: '{entry}')"
                                    ),
                                );
                            }
                        }
                        DepsMode::Allow(allow) => {
                            if !allow
                                .iter()
                                .any(|e| crate::imports::module_prefix_match(module, e))
                            {
                                issue(
                                    PathBuf::from(rel),
                                    edge.line,
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
pub(super) fn check_max_age(
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
        .map(|e| {
            (
                e,
                e.code_roots
                    .iter()
                    .map(|r| crate::imports::normalize_root(r))
                    .collect(),
            )
        })
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
                    if crate::imports::module_prefix_match(ra, rb)
                        || crate::imports::module_prefix_match(rb, ra)
                    {
                        return Err(HarnessError::Control(format!(
                            "правило '{}': code_roots пересекаются: {} ('{ra}') и {} ('{rb}')",
                            rule.name, a.id, b.id
                        )));
                    }
                }
            }
        }
    }
    let bases = crate::imports::candidate_bases(repo);
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
        let Some(source) = crate::imports::owner_by_path(&contexts, rel).copied() else {
            continue;
        };
        let bytes = std::fs::read(abs).map_err(|e| HarnessError::io(abs, e))?;
        let content = String::from_utf8_lossy(&bytes);
        for edge in crate::imports::extract_imports(rel, &content)? {
            let module = &edge.module;
            let Some(target) =
                crate::imports::resolve_import_owner_fs(repo, &contexts, &bases, rel, module)
                    .copied()
            else {
                continue;
            };
            if target.id == source.id || source.depends_on.contains(&target.id) {
                continue;
            }
            issue(
                PathBuf::from(rel),
                edge.line,
                format!(
                    "context_boundary: импорт '{module}' пересекает границу контекста: {} → {} ({}) без depends_on в модели",
                    source.id, target.id, target.title
                ),
            );
        }
    }
    Ok(())
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

/// Запускает `bash -c <command>` в `repo` с ручным таймаутом.
/// `Ok(None)` в статусе — команда превысила таймаут и была убита.
/// Механика (процессная группа, читатели с дедлайном) — [`crate::proc`]:
/// таймаут убивает группу целиком, внуки не держат гейт (A1).
pub(super) fn run_with_timeout(
    repo: &Path,
    command: &str,
    timeout: Duration,
) -> Result<CommandOutcome> {
    let outcome = crate::proc::run_shell(repo, "bash", command, timeout)?;
    let mut captured = outcome.stdout;
    captured.extend_from_slice(&outcome.stderr);
    Ok(CommandOutcome {
        status: outcome.status,
        tail: String::from_utf8_lossy(&captured).into_owned(),
    })
}

/// Итог прогона `command_succeeds`-команды: статус и хвост вывода для отчёта.
pub(super) struct CommandOutcome {
    /// `Some`, если команда завершилась сама (иначе — убита по таймауту).
    pub(super) status: Option<ExitStatus>,
    /// Последние байты stdout+stderr (обрезаны до [`crate::proc::MAX_CAPTURE_BYTES`]).
    pub(super) tail: String,
}

/// Сколько последних строк хвоста включаем в отчёт об упавшей команде.
pub(super) const REPORT_TAIL_LINES: usize = 15;

/// Хвост вывода упавшей команды для отчёта: последние [`REPORT_TAIL_LINES`]
/// строк, пропущенные через редактор секретов (AD-3: вывод может содержать
/// значения переменных окружения и токены).
pub(super) fn report_tail(raw: &str) -> String {
    let redacted = crate::secrets::Redactor::new(crate::secrets::builtin_rules()).redact(raw);
    let lines: Vec<&str> = redacted.lines().collect();
    let skip = lines.len().saturating_sub(REPORT_TAIL_LINES);
    lines[skip..].join("\n")
}
