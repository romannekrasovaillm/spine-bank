//! Единый архитектурный гейт репозитория (`arch-be gate`) — одна команда,
//! прогоняющая детерминированный контур контроля (AD-2, AD-9) целиком и
//! сводящая исходы в один exit-код: провал ЛЮБОЙ составляющей → exit 1
//! (механически, без разбора строк вывода — строки для людей, код для CI
//! и хуков `arch-be connect`).
//!
//! Составляющие (на любом маршруте): fitness (`control check`), гейт прямых
//! правок спайна (`delta guard`), анти-ослабление реестра правил
//! ([`control::rule_weakened`]), линтер спайна (`control spine`), трассировка
//! (`trace check`). На маршрутах Standard/Critical добавляются сенсоры
//! спецификаций ([`control::sensors_check`] по `<repo>/docs/spec`),
//! количественные NFR (все четыре проверки `nfr`) и проверка evidence-бандлов
//! активных дельт (`changes/<name>/EVIDENCE.yaml`).
//!
//! Fail-soft (статус SKIP, не падение): у составляющей нет входа — нет
//! `CONSTRAINTS.yaml`, не git-репозиторий, нет `model/`, нет активных
//! бандлов. Сбой выполнения при НАЛИЧИИ входа (битый YAML, нерабочее правило)
//! — FAIL с причиной: гейт, молча пропускающий поломку собственной
//! конфигурации, не гейт (антикейс бэклога: агент под давлением «зеленеет»
//! правкой `CONSTRAINTS.yaml` — `rule_weakened` это ловит).
//!
//! Маршрут: `--route auto` (дефолт) вычисляет маршрут механически из
//! git-диффа ([`control::detect_diff_triggers`] + [`control::score_with_sources`]
//! с пустым declared — тот же anti-bypass floor S-1, что у MCP
//! `significance_from_diff`); дифф недоступен (не git, нет HEAD) — fail-safe
//! маршрут Critical. Явный `--route fast|standard|critical` переопределяет
//! авто-режим.
//!
//! Вывод: текстовый рендер — [`render`]; машинные форматы для CI
//! (`--format sarif|junit|gitlab-codequality|markdown`, вывод в stdout для
//! редиректа в файл-артефакт) — модуль [`crate::report_fmt`] поверх
//! структурированных находок [`GateFinding`].

use std::collections::BTreeMap;
use std::fmt::{self, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::control::{self, Route};
use crate::error::{HarnessError, Result};
use crate::{delta, evidence, nfr, trace};

/// Потолок строк находок, печатаемых под FAIL-составляющей (остаток —
/// счётчиком): вывод гейта читают люди и агенты в хуках, простыня находок
/// там не нужна — полный список дают команды составляющих.
const MAX_COMPONENT_FINDINGS: usize = 20;

/// Статус составляющей гейта.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateStatus {
    /// Пройдена.
    Pass,
    /// Провалена (находки или сбой выполнения при наличии входа) — итоговый
    /// exit 1.
    Fail,
    /// Пропущена fail-soft: нет входа (артефактов) для проверки.
    Skip,
}

impl GateStatus {
    /// Метка для отчёта.
    fn label(self) -> &'static str {
        match self {
            Self::Pass => "PASS",
            Self::Fail => "FAIL",
            Self::Skip => "SKIP",
        }
    }
}

/// Структурированная находка составляющей гейта: [`fmt::Display`] воспроизводит
/// каноничную строку текстового вывода (форматы составляющих различаются —
/// см. конструкторы), а поля отдают адрес и правило машинным форматам
/// (`src/report_fmt.rs`: SARIF/JUnit/GitLab Code Quality) без разбора строк.
#[derive(Debug, Clone, serde::Serialize)]
pub struct GateFinding {
    /// Критичность (`error` | `warn`).
    pub severity: String,
    /// Код правила/проверки (`None` — чисто текстовая находка).
    pub rule: Option<String>,
    /// Файл (`None` — находка без адреса).
    pub file: Option<String>,
    /// Строка (`None` — без адреса; у `LintIssue` «файл целиком» — это 0).
    pub line: Option<usize>,
    /// Сообщение.
    pub message: String,
}

impl GateFinding {
    /// Находка из `LintIssue` (fitness, `rule_weakened`, `spine_lint`):
    /// `[severity] file:line rule — message`.
    fn lint(i: &control::LintIssue) -> Self {
        Self {
            severity: i.severity.clone(),
            rule: Some(i.rule.clone()),
            file: Some(i.file.display().to_string()),
            line: Some(i.line),
            message: i.message.clone(),
        }
    }

    /// Адресная находка без строки (`delta_guard`): `file — message`.
    fn file_only(severity: &str, file: String, message: String) -> Self {
        Self {
            severity: severity.to_string(),
            rule: None,
            file: Some(file),
            line: None,
            message,
        }
    }

    /// Находка с правилом без адреса (`trace_check`, nfr): `[severity] rule — message`.
    fn ruled(severity: String, rule: String, message: String) -> Self {
        Self {
            severity,
            rule: Some(rule),
            file: None,
            line: None,
            message,
        }
    }

    /// Свободный текст (`evidence_verify`): печатается как есть.
    fn text(severity: &str, message: String) -> Self {
        Self {
            severity: severity.to_string(),
            rule: None,
            file: None,
            line: None,
            message,
        }
    }
}

impl fmt::Display for GateFinding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (&self.file, self.line, &self.rule) {
            (Some(file), Some(line), Some(rule)) => {
                write!(
                    f,
                    "[{}] {}:{} {} — {}",
                    self.severity, file, line, rule, self.message
                )
            }
            (Some(file), _, None) => write!(f, "{} — {}", file, self.message),
            (Some(file), _, Some(rule)) => {
                write!(
                    f,
                    "[{}] {} {} — {}",
                    self.severity, file, rule, self.message
                )
            }
            (None, _, Some(rule)) => write!(f, "[{}] {} — {}", self.severity, rule, self.message),
            (None, _, None) => write!(f, "{}", self.message),
        }
    }
}

/// Итог одной составляющей гейта.
#[derive(Debug)]
pub struct GateComponent {
    /// Имя составляющей (`fitness`, `delta_guard`, …).
    pub name: &'static str,
    /// Статус.
    pub status: GateStatus,
    /// Краткая причина/сводка одной строкой.
    pub detail: String,
    /// Находки (печатаются отступом под строкой FAIL-составляющей).
    pub findings: Vec<GateFinding>,
}

impl GateComponent {
    /// Составляющая пройдена.
    fn pass(name: &'static str, detail: String) -> Self {
        Self {
            name,
            status: GateStatus::Pass,
            detail,
            findings: Vec::new(),
        }
    }

    /// Составляющая провалена (находки/сбой) — гейт падает.
    fn fail(name: &'static str, detail: String, findings: Vec<GateFinding>) -> Self {
        Self {
            name,
            status: GateStatus::Fail,
            detail,
            findings,
        }
    }

    /// Составляющая пропущена fail-soft (нет входа).
    fn skip(name: &'static str, detail: String) -> Self {
        Self {
            name,
            status: GateStatus::Skip,
            detail,
            findings: Vec::new(),
        }
    }
}

/// Отчёт гейта `arch-be gate`.
#[derive(Debug)]
pub struct GateReport {
    /// Репозиторий.
    pub repo: PathBuf,
    /// Маршрут прогона.
    pub route: Route,
    /// Маршрут вычислен автоматически из диффа (`--route auto`) или задан явно.
    pub route_auto: bool,
    /// Заметка о маршруте: score и триггеры из диффа либо причина fail-safe.
    pub route_note: String,
    /// Составляющие в порядке прогона.
    pub components: Vec<GateComponent>,
    /// Гейт пройден (нет FAIL ни в одной составляющей).
    pub passed: bool,
}

/// Предпроверка git-окружения репозитория (один раз на прогон).
struct GitProbe {
    /// Это git-репозиторий (`git rev-parse --git-dir` успешен).
    repo: bool,
    /// Есть HEAD (`git rev-parse --verify HEAD`): в репозитории без единого
    /// коммита базы для диффа/сравнения нет.
    head: bool,
}

impl GitProbe {
    /// Снимает состояние git для `repo`. Любой сбой запуска git трактуется
    /// как «не git» (fail-soft: git-составляющие получат SKIP).
    fn probe(repo: &Path) -> Self {
        let is_repo = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(["rev-parse", "--git-dir"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success());
        let head = is_repo
            && Command::new("git")
                .arg("-C")
                .arg(repo)
                .args(["rev-parse", "--verify", "--quiet", "HEAD"])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|s| s.success());
        Self {
            repo: is_repo,
            head,
        }
    }
}

/// Левая сторона диапазона диффа как одиночная ревизия для `git show`
/// (`origin/main...HEAD` → `origin/main`): `git show` диапазон не принимает.
fn base_rev(base: &str) -> &str {
    match base.split_once("...") {
        Some((left, _)) => left,
        None => base.split_once("..").map_or(base, |(left, _)| left),
    }
}

/// Ревизия существует (`git rev-parse --verify <rev>^{commit}`)?
fn git_rev_exists(repo: &Path, rev: &str) -> bool {
    Command::new("git")
        .arg("-C")
        .arg(repo)
        .arg("rev-parse")
        .arg("--verify")
        .arg("--quiet")
        .arg(format!("{rev}^{{commit}}"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// Путь присутствует в ревизии (`git cat-file -e <rev>:<rel>`)? Ревизия
/// обязана существовать (проверяется [`git_rev_exists`]).
fn git_rev_has_path(repo: &Path, rev: &str, rel: &str) -> bool {
    Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["cat-file", "-e"])
        .arg(format!("{rev}:{rel}"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// Первая непустая строка stderr git без префикса «fatal:» — краткая причина
/// для отчёта. Сырой stderr целиком в отчёт не проксируем (D9): там
/// многострочная справка использования, засорявшая вывод гейта.
fn git_stderr_reason(stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr);
    let reason = text.lines().map(str::trim).find(|l| !l.is_empty()).map_or(
        "git завершился с ошибкой без сообщения",
        |l| l.strip_prefix("fatal:").map_or(l, str::trim),
    );
    reason.chars().take(160).collect()
}

/// Содержимое файла в ревизии (`git show <rev>:<rel>`).
///
/// # Errors
/// `git` недоступен, ревизия плохая, содержимое не UTF-8.
fn git_show_file(repo: &Path, rev: &str, rel: &str) -> Result<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .arg("show")
        .arg(format!("{rev}:{rel}"))
        .stdin(Stdio::null())
        .output()
        .map_err(|e| HarnessError::Control(format!("git show не запустился: {e}")))?;
    if !out.status.success() {
        return Err(HarnessError::Control(format!(
            "git show {rev}:{rel}: {}",
            git_stderr_reason(&out.stderr)
        )));
    }
    String::from_utf8(out.stdout)
        .map_err(|_| HarnessError::Control(format!("{rev}:{rel}: содержимое не UTF-8")))
}

/// Разрешённый путь к файлу ограничений гейта: явный `--constraints` либо
/// дефолт с fallback'ом на корневой `CONSTRAINTS.yaml` (D6); резолв —
/// единый [`control::resolve_constraints_path_detailed`] (E2).
struct ConstraintsPath {
    /// Файл ограничений (может не существовать — составляющие дадут SKIP).
    path: PathBuf,
    /// Путь задан явно флагом `--constraints`: тогда путь вне репозитория
    /// валит `rule_weakened` (fail-closed), а не молча отключает
    /// анти-ослабление (раньше — SKIP «git-сравнение невозможно»).
    explicit: bool,
    /// Вторая копия реестра, отличающаяся от использованной (drift, E2) —
    /// пометка в детали составляющей `fitness`.
    drift: Option<PathBuf>,
}

/// Путь к файлу ограничений для отчёта: относительный к репозиторию, когда
/// файл внутри него (иначе — как передан).
fn constraints_label(repo: &Path, constraints: &Path) -> String {
    constraints
        .strip_prefix(repo)
        .unwrap_or(constraints)
        .display()
        .to_string()
}

/// Относительный путь `file` внутри `repo` по канонизированным путям
/// (`None` — файл вне репозитория или канонизация не удалась). Сравнение
/// канонизированно: явный `--constraints` бывает абсолютным или с `./`,
/// тогда как `repo` — `.` (раньше такой путь улетал в SKIP).
fn canonical_rel(repo: &Path, file: &Path) -> Option<PathBuf> {
    let abs_file = file.canonicalize().ok()?;
    let abs_repo = repo.canonicalize().ok()?;
    abs_file.strip_prefix(&abs_repo).ok().map(Path::to_path_buf)
}

/// Составляющая `fitness`: прогон `CONSTRAINTS.yaml` ([`control::check`]).
fn component_fitness(repo: &Path, constraints: &ConstraintsPath) -> GateComponent {
    if !constraints.path.is_file() {
        return GateComponent::skip(
            "fitness",
            format!(
                "нет файла ограничений {} — нечего прогонять",
                constraints.path.display()
            ),
        );
    }
    let label = constraints_label(repo, &constraints.path);
    // Пометка дрейфа двух копий реестра (E2): обе существуют и различаются.
    let drift_note = constraints.drift.as_ref().map_or_else(String::new, |d| {
        format!(
            "; {}",
            control::constraints_drift_note(&constraints.path, d)
        )
    });
    match control::check(repo, &constraints.path) {
        Ok(report) if report.passed => GateComponent::pass(
            "fitness",
            format!("{} — файл: {label}{drift_note}", report.summary),
        ),
        Ok(report) => GateComponent::fail(
            "fitness",
            format!("{} — файл: {label}{drift_note}", report.summary),
            report.issues.iter().map(GateFinding::lint).collect(),
        ),
        Err(e) => GateComponent::fail("fitness", format!("сбой выполнения: {e}"), Vec::new()),
    }
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
fn component_delta_guard(repo: &Path, base: Option<&str>, git: &GitProbe) -> GateComponent {
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
fn component_rule_weakened(
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
    if !git_rev_has_path(repo, rev, &rel) {
        return GateComponent::skip(
            "rule_weakened",
            format!("в базе '{rev}' файла {rel} нет (новый реестр) — сравнивать не с чем"),
        );
    }
    let base_src = match git_show_file(repo, rev, &rel) {
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
fn component_spine_lint(repo: &Path) -> GateComponent {
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
fn component_trace(repo: &Path) -> GateComponent {
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
    match trace::trace_check(repo) {
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
fn component_sensors(repo: &Path) -> GateComponent {
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
fn component_nfr(repo: &Path) -> GateComponent {
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

/// Составляющая `evidence_verify` (маршруты Standard/Critical): полнота и
/// целостность хэшей evidence-бандлов активных дельт
/// (`changes/<name>/EVIDENCE.yaml`; архивные — уже выпущенные, не гейтуются).
fn component_evidence(repo: &Path) -> GateComponent {
    let changes = repo.join("changes");
    let mut bundles: Vec<PathBuf> = Vec::new();
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
    if bundles.is_empty() {
        return GateComponent::skip(
            "evidence_verify",
            "нет активных change-dir с EVIDENCE.yaml".to_string(),
        );
    }
    let mut failed = Vec::new();
    for dir in &bundles {
        match evidence::verify(dir) {
            Ok(verdict) if verdict.passed => {}
            Ok(verdict) => {
                failed.push(GateFinding::text(
                    "error",
                    format!("{}: {}", dir.display(), verdict.summary),
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
    if failed.is_empty() {
        GateComponent::pass(
            "evidence_verify",
            format!("бандлов проверено: {}", bundles.len()),
        )
    } else {
        GateComponent::fail(
            "evidence_verify",
            format!("бандлов: {}, не прошли: {}", bundles.len(), failed.len()),
            failed,
        )
    }
}

/// Вычисляет маршрут из git-диффа (`--route auto`): [`control::detect_diff_triggers`]
/// и [`control::score_with_sources`] с пустым declared (механический минимум
/// S-1, ADR-034). Дифф недоступен (не git-репозиторий, нет HEAD) — fail-safe
/// маршрут Critical с пометкой причины.
fn auto_route(repo: &Path, base: Option<&str>, limits: (usize, usize)) -> (Route, String) {
    match control::detect_diff_triggers(repo, base) {
        Ok(diff) => {
            let scored = control::score_with_sources(&BTreeMap::new(), &diff, limits.0, limits.1);
            let fired = if scored.significance.fired.is_empty() {
                "триггеров нет".to_string()
            } else {
                scored.significance.fired.join(", ")
            };
            (
                scored.significance.route,
                format!("auto: score {} ({fired})", scored.significance.score),
            )
        }
        Err(e) => (
            Route::Critical,
            format!("auto: дифф недоступен ({e}) — fail-safe маршрут Critical"),
        ),
    }
}

/// Прогоняет единый гейт по репозиторию.
///
/// `route_override`: `None` — маршрут вычисляется из диффа (`--route auto`).
/// `base`: git-ref базы для диффа, delta guard и сравнения правил (`None` —
/// рабочее дерево против HEAD). `constraints`: файл ограничений (дефолт
/// `<repo>/.arch-handoff/CONSTRAINTS.yaml`, как у `control check`; D6 —
/// fallback: если его нет, но есть `<repo>/CONSTRAINTS.yaml`, берётся он —
/// и для fitness, и для анти-ослабления).
/// `limits` — пороги маршрутов `(fast_max, standard_max)` из конфига
/// (`[significance]`, ADR-034).
///
/// # Errors
/// Репозиторий недоступен. Провалы составляющих — НЕ ошибка: они в отчёте
/// (`passed = false`), exit-код ставит CLI-край.
pub fn run(
    repo: &Path,
    route_override: Option<Route>,
    base: Option<&str>,
    constraints: Option<&Path>,
    limits: (usize, usize),
) -> Result<GateReport> {
    if !repo.is_dir() {
        return Err(HarnessError::Control(format!(
            "репозиторий недоступен: {}",
            repo.display()
        )));
    }
    let (route, route_auto, route_note) = if let Some(r) = route_override {
        (r, false, format!("явный --route {r}"))
    } else {
        let (r, note) = auto_route(repo, base, limits);
        (r, true, note)
    };
    let constraints = if let Some(path) = constraints {
        ConstraintsPath {
            path: path.to_path_buf(),
            explicit: true,
            drift: None,
        }
    } else {
        // Единый резолвер (E2): пакетная копия → корневой fallback (D6);
        // ни одной копии — дефолтный путь, составляющие дадут SKIP (раньше
        // на кейсе без handoff-пакета гейт зеленел «из-за пропусков»).
        match control::resolve_constraints_path_detailed(repo, None) {
            Some(resolution) => ConstraintsPath {
                path: resolution.path,
                explicit: false,
                drift: resolution.drift,
            },
            None => ConstraintsPath {
                path: repo.join(control::HANDOFF_CONSTRAINTS_PATH),
                explicit: false,
                drift: None,
            },
        }
    };
    let git = GitProbe::probe(repo);

    let mut components = vec![
        component_fitness(repo, &constraints),
        component_delta_guard(repo, base, &git),
        component_rule_weakened(repo, &constraints, base.unwrap_or("HEAD"), &git),
        component_spine_lint(repo),
        component_trace(repo),
    ];
    if matches!(route, Route::Standard | Route::Critical) {
        components.push(component_sensors(repo));
        components.push(component_nfr(repo));
        components.push(component_evidence(repo));
    }
    let passed = components.iter().all(|c| c.status != GateStatus::Fail);
    Ok(GateReport {
        repo: repo.to_path_buf(),
        route,
        route_auto,
        route_note,
        components,
        passed,
    })
}

/// Текстовый рендер отчёта гейта: строка маршрута, по каждой составляющей
/// PASS/FAIL/SKIP + краткая причина, находки отступом (с потолком
/// [`MAX_COMPONENT_FINDINGS`]), итоговая строка «Итог: PASS/FAIL».
#[must_use]
pub fn render(report: &GateReport) -> String {
    let mut out = String::new();
    // Запись в String не может завершиться ошибкой — игноры безопасны.
    let _ = writeln!(out, "Гейт: {}", report.repo.display());
    let _ = writeln!(out, "Маршрут: {} ({})", report.route, report.route_note);
    for c in &report.components {
        let _ = writeln!(out, "  [{}] {} — {}", c.status.label(), c.name, c.detail);
        for f in c.findings.iter().take(MAX_COMPONENT_FINDINGS) {
            let _ = writeln!(out, "      ↳ {f}");
        }
        if c.findings.len() > MAX_COMPONENT_FINDINGS {
            let _ = writeln!(
                out,
                "      ↳ … и ещё {} находок (полный список — командами составляющих)",
                c.findings.len() - MAX_COMPONENT_FINDINGS
            );
        }
    }
    let failed = report
        .components
        .iter()
        .filter(|c| c.status == GateStatus::Fail)
        .count();
    let _ = writeln!(
        out,
        "Итог: {}",
        if report.passed {
            "PASS".to_string()
        } else {
            format!("FAIL — провалено составляющих: {failed} (exit 1)")
        }
    );
    // Квитанция ценности (аддитивная строка): сумма error-находок всех
    // составляющих — это дефекты, остановленные механикой до ревью.
    if !report.passed {
        let caught = report
            .components
            .iter()
            .flat_map(|c| &c.findings)
            .filter(|f| f.severity == "error")
            .count();
        let _ = writeln!(
            out,
            "Гейт поймал {caught} нарушений до ревью — исправьте и перепроверьте"
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// git в каталоге с тестовой идентичностью коммиттера (образец —
    /// `src/delta.rs::make_guard_repo`).
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

    /// Репо-фикстура гейта: git + `.arch-handoff/CONSTRAINTS.yaml` с двумя
    /// error-правилами и файл, который они требуют. Один коммит.
    fn make_gate_repo(dir: &Path) {
        std::fs::create_dir_all(dir.join(".arch-handoff")).expect("mkdir handoff");
        std::fs::write(
            dir.join(".arch-handoff/CONSTRAINTS.yaml"),
            "rules:\n  - name: spine_present\n    type: file_exists\n    path: \"ARCHITECTURE-SPINE.md\"\n    severity: error\n  - name: no_pan\n    type: must_not_contain\n    glob: \"**/*.py\"\n    pattern: 'PAN'\n    severity: error\n",
        )
        .expect("constraints");
        std::fs::write(dir.join("ARCHITECTURE-SPINE.md"), "# Spine\n").expect("spine");
        git(dir, &["init", "-q"]);
        git(dir, &["add", "."]);
        git(dir, &["commit", "-q", "-m", "init"]);
    }

    /// Статус составляющей по имени.
    fn status_of(report: &GateReport, name: &str) -> GateStatus {
        report
            .components
            .iter()
            .find(|c| c.name == name)
            .unwrap_or_else(|| panic!("нет составляющей {name}"))
            .status
    }

    #[test]
    fn gate_passes_on_clean_repo_and_skips_without_inputs() {
        let tmp = tempfile::tempdir().expect("tmp");
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).expect("mkdir");
        make_gate_repo(&repo);
        let report = run(&repo, None, None, None, (1, 4)).expect("гейт");
        assert!(report.passed, "чистый репо: {}", render(&report));
        assert!(report.route_auto, "маршрут из диффа");
        // Дифф пуст → score 0 → Fast → nfr/evidence вне прогона.
        assert_eq!(report.route, Route::Fast);
        for name in ["fitness", "delta_guard", "rule_weakened", "spine_lint"] {
            assert_eq!(status_of(&report, name), GateStatus::Pass, "{name}");
        }
        assert_eq!(status_of(&report, "trace_check"), GateStatus::Skip);
        assert!(
            !report.components.iter().any(|c| c.name == "nfr"),
            "маршрут Fast — nfr вне гейта"
        );
        let text = render(&report);
        assert!(text.contains("Итог: PASS"), "{text}");
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

    #[test]
    fn gate_non_git_repo_is_fail_soft() {
        let tmp = tempfile::tempdir().expect("tmp");
        let repo = tmp.path().join("plain");
        std::fs::create_dir_all(repo.join(".arch-handoff")).expect("mkdir");
        std::fs::write(
            repo.join(".arch-handoff/CONSTRAINTS.yaml"),
            "rules:\n  - name: spine_present\n    type: file_exists\n    path: \"ARCHITECTURE-SPINE.md\"\n    severity: error\n",
        )
        .expect("constraints");
        std::fs::write(repo.join("ARCHITECTURE-SPINE.md"), "# Spine\n").expect("spine");
        // Не git: delta guard и rule_weakened — SKIP; auto-маршрут — fail-safe.
        let report = run(&repo, None, None, None, (1, 4)).expect("гейт");
        assert!(report.passed, "{}", render(&report));
        assert_eq!(report.route, Route::Critical, "fail-safe без диффа");
        assert_eq!(status_of(&report, "delta_guard"), GateStatus::Skip);
        assert_eq!(status_of(&report, "rule_weakened"), GateStatus::Skip);
        assert_eq!(status_of(&report, "fitness"), GateStatus::Pass);
    }

    #[test]
    fn gate_standard_route_adds_nfr_and_evidence_components() {
        let tmp = tempfile::tempdir().expect("tmp");
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).expect("mkdir");
        make_gate_repo(&repo);
        let report = run(&repo, Some(Route::Standard), None, None, (1, 4)).expect("гейт");
        assert!(!report.route_auto, "явный маршрут");
        assert_eq!(status_of(&report, "nfr"), GateStatus::Skip, "нет model/");
        assert_eq!(
            status_of(&report, "evidence_verify"),
            GateStatus::Skip,
            "нет активных бандлов"
        );
    }

    #[test]
    fn gate_route_note_lists_diff_triggers() {
        let tmp = tempfile::tempdir().expect("tmp");
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).expect("mkdir");
        make_gate_repo(&repo);
        // Новый компонент в рабочем дереве (untracked): auto поднимает score.
        std::fs::create_dir_all(repo.join("services/risk/src")).expect("mkdir svc");
        std::fs::write(
            repo.join("services/risk/Cargo.toml"),
            "[package]\nname = \"risk\"\nversion = \"0.1.0\"\n",
        )
        .expect("manifest");
        let report = run(&repo, None, None, None, (1, 4)).expect("гейт");
        assert!(
            matches!(report.route, Route::Standard | Route::Critical),
            "маршрут из диффа: {}",
            report.route_note
        );
        assert!(
            report.route_note.contains("new_component"),
            "{}",
            report.route_note
        );
    }

    #[test]
    fn gate_missing_repo_is_error_not_report() {
        let tmp = tempfile::tempdir().expect("tmp");
        let err = run(&tmp.path().join("ghost"), None, None, None, (1, 4))
            .expect_err("несуществующий репозиторий");
        assert!(err.to_string().contains("недоступен"), "{err}");
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
        assert!(report.passed, "{}", render(&report));
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
        assert!(report.passed, "{}", render(&report));
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

    /// git-репозиторий без единого коммита: `.arch-handoff/CONSTRAINTS.yaml`
    /// и spine на месте, базы для диффа/сравнения нет.
    fn make_uncommitted_repo(repo: &Path) {
        std::fs::create_dir_all(repo.join(".arch-handoff")).expect("mkdir");
        std::fs::write(
            repo.join(".arch-handoff/CONSTRAINTS.yaml"),
            "rules:\n  - name: spine_present\n    type: file_exists\n    path: \"ARCHITECTURE-SPINE.md\"\n    severity: error\n",
        )
        .expect("constraints");
        std::fs::write(repo.join("ARCHITECTURE-SPINE.md"), "# Spine\n").expect("spine");
        git(repo, &["init", "-q"]);
    }

    #[test]
    fn gate_route_note_is_clean_when_diff_base_unavailable() {
        let tmp = tempfile::tempdir().expect("tmp");
        let repo = tmp.path().join("repo");
        make_uncommitted_repo(&repo);
        let report = run(&repo, None, None, None, (1, 4)).expect("гейт");
        assert_eq!(report.route, Route::Critical, "fail-safe без диффа");
        // D9: сырой stderr git (многострочная справка «Используйте «--»…»)
        // в отчёт не протекает — только чистое однострочное сообщение.
        assert!(
            report.route_note.contains("база диффа недоступна"),
            "{}",
            report.route_note
        );
        assert!(
            !report.route_note.contains('\n'),
            "однострочная заметка: {}",
            report.route_note
        );
        for junk in ["Используйте", "Use '--'", "separate paths", "fatal:"] {
            assert!(
                !report.route_note.contains(junk),
                "в заметке маршрута сырой stderr git ({junk}): {}",
                report.route_note
            );
        }
        assert!(
            report.route_note.contains("fail-safe маршрут Critical"),
            "{}",
            report.route_note
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
}
