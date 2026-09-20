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
    /// Метка для отчёта. `pub(crate)`: паспорт вердикта (W1) печатает те же
    /// три статуса, что и гейт, — своя таблица меток разошлась бы с гейтом.
    pub(crate) fn label(self) -> &'static str {
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
    /// Заявленное, но механикой НЕ проверяемое: границы собственного
    /// вердикта составляющей (W1, блок 2 паспорта).
    ///
    /// Это не находки и не оправдания: зелёная составляющая обязана назвать,
    /// что именно её зелёный НЕ означает («ссылка разрешена в существующую
    /// сущность» ≠ «ссылка верна»; «балл рубрики выше порога» ≠ «решение
    /// верное»). Строки собираются в паспорт вердикта
    /// ([`crate::passport`]) и в аттестацию не входят: они описывают границу
    /// проверки, а не её результат.
    pub not_verified: Vec<String>,
}

impl GateComponent {
    /// Составляющая пройдена.
    fn pass(name: &'static str, detail: String) -> Self {
        Self {
            name,
            status: GateStatus::Pass,
            detail,
            findings: Vec::new(),
            not_verified: Vec::new(),
        }
    }

    /// Составляющая пройдена, но с неблокирующими находками (warn):
    /// предупреждения обязаны доезжать до читателя, а не пропадать вместе
    /// со статусом PASS (Н7: `judge_is_author`).
    fn pass_with_findings(name: &'static str, detail: String, findings: Vec<GateFinding>) -> Self {
        Self {
            name,
            status: GateStatus::Pass,
            detail,
            findings,
            not_verified: Vec::new(),
        }
    }

    /// Составляющая провалена (находки/сбой) — гейт падает.
    fn fail(name: &'static str, detail: String, findings: Vec<GateFinding>) -> Self {
        Self {
            name,
            status: GateStatus::Fail,
            detail,
            findings,
            not_verified: Vec::new(),
        }
    }

    /// Составляющая пропущена fail-soft (нет входа).
    fn skip(name: &'static str, detail: String) -> Self {
        Self {
            name,
            status: GateStatus::Skip,
            detail,
            findings: Vec::new(),
            not_verified: Vec::new(),
        }
    }

    /// Дополняет составляющую границей её вердикта (W1, блок 2 паспорта).
    fn noting(mut self, notes: Vec<String>) -> Self {
        self.not_verified = notes;
        self
    }
}

/// Итог гейта — три состояния (П1 ДКА): бинарный PASS/FAIL не различал
/// «проверено и чисто» и «проверять было нечего».
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateOutcome {
    /// Все обязательные для маршрута составляющие пройдены, FAIL нет.
    Pass,
    /// Есть FAIL.
    Fail,
    /// FAIL нет, но обязательная для маршрута составляющая в SKIP (нет входа):
    /// «зелёный» не полон, выпускать по нему нельзя.
    Incomplete,
}

impl GateOutcome {
    /// Метка для отчёта и конверта.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Pass => "PASS",
            Self::Fail => "FAIL",
            Self::Incomplete => "INCOMPLETE",
        }
    }

    /// Exit-код канала: 0 — PASS, 1 — FAIL, 3 — INCOMPLETE (отличим в CI).
    #[must_use]
    pub fn exit_code(self) -> i32 {
        match self {
            Self::Pass => 0,
            Self::Fail => 1,
            Self::Incomplete => 3,
        }
    }
}

/// Матрица обязательных составляющих гейта по маршруту (П1 ДКА).
#[derive(Debug, Clone)]
pub struct GateRequirements {
    /// Обязательные составляющие маршрута Fast.
    pub fast: Vec<String>,
    /// Обязательные составляющие маршрута Standard.
    pub standard: Vec<String>,
    /// Обязательные составляющие маршрута Critical.
    pub critical: Vec<String>,
}

impl Default for GateRequirements {
    fn default() -> Self {
        Self::from_config(&crate::config::GateConfig::default())
    }
}

impl GateRequirements {
    /// Из секции `[gate]` конфига.
    #[must_use]
    pub fn from_config(cfg: &crate::config::GateConfig) -> Self {
        Self {
            fast: cfg.required.fast.clone(),
            standard: cfg.required.standard.clone(),
            critical: cfg.required.critical.clone(),
        }
    }

    /// Список обязательных составляющих для маршрута.
    #[must_use]
    pub fn for_route(&self, route: Route) -> &[String] {
        match route {
            Route::Fast => &self.fast,
            Route::Standard => &self.standard,
            Route::Critical => &self.critical,
        }
    }
}

/// Свёртка находок составляющей: SHA-256 отсортированного списка
/// `(код находки, путь)` — привязывает аттестацию к СОДЕРЖАНИЮ вердикта, а не
/// только к имени и статусу составляющей (Н3, ADR-043).
///
/// Берётся только код и путь, не текст сообщения: формулировки — часть
/// представления и могут меняться без смены смысла вердикта.
#[must_use]
fn findings_digest(c: &GateComponent) -> String {
    let mut keys: Vec<(String, String)> = c
        .findings
        .iter()
        .map(|f| {
            (
                f.rule.clone().unwrap_or_else(|| f.severity.clone()),
                f.file.clone().unwrap_or_default(),
            )
        })
        .collect();
    keys.sort();
    let mut canon = String::new();
    for (rule, file) in keys {
        // Запись в String не может завершиться ошибкой — игнор безопасен.
        let _ = writeln!(canon, "{rule}\0{file}");
    }
    crate::hash::sha256_hex(canon.as_bytes())
}

/// Настройки гейта, не выражаемые маршрутом (0.3.4): семантика артефактов
/// бандла (Н1, ADR-041) и порог качества решений (Н7, ADR-042).
///
/// Тесты и встраивающие вызовы без конфига получают [`GateOptions::default`] —
/// ровно те же значения, что в `Config::default()`, поэтому вердикт не
/// зависит от того, читал ли вызывающий `config.toml` (AD-7).
#[derive(Debug, Clone)]
pub struct GateOptions {
    /// Семантика артефактов Evidence Bundle.
    pub evidence: crate::config::EvidenceConfig,
    /// Порог качества архитектурных решений (составляющая `decision_quality`).
    pub decision_quality: crate::config::DecisionQualityConfig,
    /// Глобы детекторов диффа (T-05): что считать контрактом, новым
    /// компонентом и изменением интеграции — из секции `[significance]`.
    pub diff_globs: control::DiffGlobs,
    /// Настройки судьи рубрик (секция `[judge]`): по ним составляющая
    /// `decision_quality` пересобирает отчёт из сырых ответов и сверяет балл
    /// (ADR-048) — теми же порогами, что были при оценке.
    pub judge: crate::config::JudgeConfig,
    /// Каталог рубрик: по нему сверка находит определение рубрики отчёта
    /// (ADR-048), а составляющая `semantic_quality` — рубрики, названные в
    /// конфиге (ADR-052).
    pub rubrics_dir: PathBuf,
    /// Требование исполняемой проверки инвариантов (ADR-050): при
    /// `warn`/`error` составляющая `trace_check` даёт находку `ad-text-only`.
    /// Дефолт `off` — вердикт не меняется без явного решения проекта.
    pub executable_required: crate::config::ExecutableRequired,
    /// Составляющая `semantic_quality` (ADR-052): какие смысловые рубрики
    /// обязательны и на какой области субъектов.
    pub semantic_quality: crate::config::SemanticQualityConfig,
}

impl GateOptions {
    /// Настройки из секций `[evidence]`, `[gate.decision_quality]` и
    /// `[significance]` конфига.
    #[must_use]
    pub fn from_config(cfg: &crate::config::Config) -> Self {
        Self {
            evidence: cfg.evidence.clone(),
            decision_quality: cfg.gate.decision_quality.clone(),
            diff_globs: cfg.significance.diff_globs(),
            judge: cfg.judge.clone(),
            rubrics_dir: cfg.paths.rubrics_dir(),
            executable_required: cfg.trace.executable_required,
            semantic_quality: cfg.gate.semantic_quality.clone(),
        }
    }
}

impl Default for GateOptions {
    /// Настройки по умолчанию (те же дефолты, что у [`crate::config::Config`]).
    fn default() -> Self {
        Self::from_config(&crate::config::Config::default())
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
    /// Итог: PASS / FAIL / INCOMPLETE (П1).
    pub outcome: GateOutcome,
    /// Имена составляющих, обязательных для этого маршрута.
    pub required: Vec<String>,
    /// Обязательные составляющие без входа (SKIP) — честный список
    /// «на что зелёный цвет не распространяется».
    pub not_checked: Vec<String>,
    /// Хэши входов вердикта (П7): реестр правил, файл ограничений.
    pub inputs: Vec<(String, String)>,
    /// SHA-256 канонического конверта вердикта (П7) — вердикт как документ.
    pub attestation: String,
    /// Гейт пройден: все обязательные PASS, FAIL нет (`outcome == Pass`).
    pub passed: bool,
}

impl GateReport {
    /// Пересчитывает производные итога из состава и матрицы обязательности.
    /// Вызывается после сборки и после добавления секций (`architect_review`).
    pub fn recompute(&mut self) {
        let has_fail = self.components.iter().any(|c| c.status == GateStatus::Fail);
        self.not_checked = self
            .required
            .iter()
            .filter(|req| {
                self.components
                    .iter()
                    .any(|c| c.name == req.as_str() && c.status == GateStatus::Skip)
            })
            .cloned()
            .collect();
        self.outcome = if has_fail {
            GateOutcome::Fail
        } else if self.not_checked.is_empty() {
            GateOutcome::Pass
        } else {
            GateOutcome::Incomplete
        };
        self.passed = self.outcome == GateOutcome::Pass;
        self.attestation = self.compute_attestation();
    }

    /// Свёртка входов для `--verify-envelope`: вход → значение, без
    /// абсолютных путей и времени.
    #[must_use]
    pub fn inputs_map(&self) -> BTreeMap<String, String> {
        self.inputs
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }

    /// Канонический конверт вердикта без абсолютных путей и времени: тот же
    /// коммит на той же матрице обязан дать ту же аттестацию во всех каналах.
    fn compute_attestation(&self) -> String {
        let mut canon = String::new();
        // Запись в String не может завершиться ошибкой — игноры безопасны.
        let _ = writeln!(canon, "arch-be/gate-verdict/v1");
        let _ = writeln!(canon, "route={}", self.route);
        let _ = writeln!(canon, "outcome={}", self.outcome.label());
        for (name, hash) in &self.inputs {
            let _ = writeln!(canon, "input:{name}={hash}");
        }
        for c in &self.components {
            let req = self.required.iter().any(|r| r == c.name);
            // Н3 (ADR-043): свёртка находок — два РАЗНЫХ FAIL обязаны быть
            // различимы, а не сливаться в «component=nfr FAIL».
            let _ = writeln!(
                canon,
                "component:{}={} required={req} findings={}",
                c.name,
                c.status.label(),
                findings_digest(c)
            );
        }
        for n in &self.not_checked {
            let _ = writeln!(canon, "not_checked={n}");
        }
        crate::hash::sha256_hex(canon.as_bytes())
    }

    /// Конверт вердикта (П7 ДКА): одна структура, которую отдают все каналы
    /// (`--format json`, MCP, хук, CI). Поле `not_checked` честно говорит, на
    /// что зелёный цвет не распространяется; `attestation` привязывает вердикт
    /// к входам и составу проверок.
    #[must_use]
    pub fn envelope_json(&self) -> serde_json::Value {
        let inputs: BTreeMap<&str, &str> = self
            .inputs
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        let components: Vec<serde_json::Value> = self
            .components
            .iter()
            .map(|c| {
                serde_json::json!({
                    "name": c.name,
                    "status": c.status.label(),
                    "required": self.required.iter().any(|r| r == c.name),
                    "detail": c.detail,
                    "findings_total": c.findings.len(),
                    // Н3 (ADR-043): различимость разных FAIL одной составляющей.
                    "findings_digest": format!("sha256:{}", findings_digest(c)),
                })
            })
            .collect();
        serde_json::json!({
            "schema": "arch-be/gate-verdict/v1",
            "verdict": self.outcome.label(),
            "exit_code": self.outcome.exit_code(),
            "arch_be": env!("CARGO_PKG_VERSION"),
            "route": {
                "effective": self.route.to_string(),
                "auto": self.route_auto,
                "note": self.route_note,
            },
            "inputs": inputs,
            "components": components,
            "not_checked": self.not_checked,
            "required": self.required,
            "attestation": format!("sha256:{}", self.attestation),
        })
    }
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
/// Реализация — единая с диффом ([`control::base_rev`]), чтобы формы базы не
/// разъезжались между составляющими гейта (T-03).
fn base_rev(base: &str) -> &str {
    control::base_rev(base)
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

/// Путь `file` относительно корня git-репозитория (для `<rev>:<path>`).
/// Git резолвит такие пути от toplevel, а не от `-C <dir>`: на кейсе-
/// подкаталоге чужого монорепо только так сравнение идёт с файлом самого
/// кейса, а не с реестром внешнего репозитория (ложные «удалено из реестра»).
fn git_rel_path(repo: &Path, file: &Path) -> Option<String> {
    let top = git_toplevel(repo)?;
    let abs = file.canonicalize().ok()?;
    let rel = abs.strip_prefix(&top).ok()?;
    Some(rel.to_string_lossy().replace('\\', "/"))
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

/// Есть ли в репозитории хоть одна копия реестра правил — корневая или
/// пакетная (T-01). Отличает «пользователь указал не тот файл» от «контура в
/// проекте нет вообще»: во втором случае сообщение обязано назвать оба места
/// и способ создать каркас.
fn has_any_registry(repo: &Path) -> bool {
    repo.join(control::ROOT_CONSTRAINTS_PATH).is_file()
        || repo.join(control::HANDOFF_CONSTRAINTS_PATH).is_file()
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
        let summary = match control::check(repo, &constraints.path) {
            Ok(report) => {
                findings.extend(report.issues.iter().map(GateFinding::lint));
                report.summary
            }
            Err(e) => format!("сбой выполнения: {e}"),
        };
        return GateComponent::fail("fitness", detail(&summary), findings).noting(notes);
    }
    match control::check(repo, &constraints.path) {
        Ok(report) if report.passed => GateComponent::pass(
            "fitness",
            format!("{} — файл: {label}", detail(&report.summary)),
        )
        .noting(notes),
        Ok(report) => GateComponent::fail(
            "fitness",
            format!("{} — файл: {label}", detail(&report.summary)),
            report.issues.iter().map(GateFinding::lint).collect(),
        )
        .noting(notes),
        Err(e) => GateComponent::fail("fitness", format!("сбой выполнения: {e}"), Vec::new()),
    }
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
fn component_trace(repo: &Path, opts: &GateOptions) -> GateComponent {
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

/// Каталоги с EVIDENCE.yaml, подлежащие проверке: активные дельты
/// `changes/<name>/` (архивные — уже выпущены) и **корень репозитория**, куда
/// бандл кладёт `evidence pack` при работе по кейсу (П1 ДКА: раньше корневой
/// бандл был невидим гейту, и `evidence_verify` молча уходил в SKIP).
fn evidence_bundle_dirs(repo: &Path) -> Vec<PathBuf> {
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
fn component_evidence(repo: &Path, cfg: &crate::config::EvidenceConfig) -> GateComponent {
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
fn component_model_validate(repo: &Path, route: Route) -> GateComponent {
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
fn component_decision_quality(repo: &Path, options: &GateOptions, enabled: bool) -> GateComponent {
    let cfg = &options.decision_quality;
    if !enabled {
        return GateComponent::skip(
            "decision_quality",
            "не включена: добавьте 'decision_quality' в [gate.required] нужного маршрута"
                .to_string(),
        );
    }
    let adr_dir = repo.join("docs/adr");
    if !adr_dir.is_dir() {
        return GateComponent::skip("decision_quality", "нет каталога docs/adr".to_string());
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
    let artifacts = crate::rubric::load_artifacts(repo);
    let mut findings = Vec::new();
    let mut judged = 0usize;
    // Отчёты, у которых нет сырых ответов судьи: сверить балл с ответами
    // механика не может (это не находка, а граница проверки — ADR-048).
    let mut not_reproducible = 0usize;
    // Отчёты, собранные в рабочей сессии (косвенный признак, ADR-048).
    let mut session_dirty = 0usize;
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
    if judged == 0 {
        return GateComponent::skip(
            "decision_quality",
            "принятых ADR (Status: Accepted) не найдено".to_string(),
        );
    }
    let errors = findings.iter().filter(|f| f.severity == "error").count();
    let detail = format!(
        "принятых ADR: {judged}, находок: {} (error: {errors}); порог {:.2}",
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
    if errors == 0 {
        GateComponent::pass_with_findings("decision_quality", detail, findings).noting(notes)
    } else {
        GateComponent::fail("decision_quality", detail, findings).noting(notes)
    }
}

/// Субъект смысловой проверки: рубрика и её субъект досье (ADR-052).
#[derive(Debug, Clone)]
struct SemanticSubject {
    /// Субъект досье: путь ADR/файла кода или идентификатор сущности.
    subject: String,
}

/// Потолок числа субъектов смысловой составляющей: перебор всего пакета не
/// должен превращать гейт в утилиту индексации (отсечение честно называется
/// в детали составляющей).
const MAX_SEMANTIC_SUBJECTS: usize = 200;
/// Каталоги, которые не обходятся при поиске файлов кода.
const SEMANTIC_SKIP_DIRS: [&str; 5] = [".git", "target", "node_modules", ".venv", "__pycache__"];

/// Составляющая `semantic_quality` (ADR-052): смысловые рубрики по ОТЧЁТАМ
/// судьи — решение против инварианта, ссылка не на ту сущность, обещание без
/// механизма, код против инварианта.
///
/// LLM в ядро не добавляется: составляющая собирает досье (детерминированно),
/// находит отчёт с тем же хэшем досье и читает его. Модель судит у хоста —
/// `rubric_prompt` → `rubric_verify`. Переоценки «на всякий случай» нет: отчёт
/// с совпавшим хэшем переиспользуется по построению, устаревший — находка, а
/// не повод вызвать модель.
///
/// Включается только через `[gate.required]`: по умолчанию SKIP, иначе
/// ужесточение покраснило бы чужие пайплайны без предупреждения. Пустой
/// список рубрик в конфиге — тоже SKIP: составляющую можно включить раньше,
/// чем завести отчёты.
fn component_semantic_quality(
    repo: &Path,
    cfg: &crate::config::SemanticQualityConfig,
    rubrics_dir: &Path,
    base: Option<&str>,
    git: &GitProbe,
    enabled: bool,
) -> GateComponent {
    if !enabled {
        return GateComponent::skip(
            "semantic_quality",
            "не включена: добавьте 'semantic_quality' в [gate.required] нужного маршрута"
                .to_string(),
        );
    }
    if cfg.rubrics.is_empty() {
        return GateComponent::skip(
            "semantic_quality",
            "список [gate.semantic_quality] rubrics пуст — обязательных смысловых рубрик нет"
                .to_string(),
        );
    }
    let changed = changed_files_for_semantics(repo, base, git, cfg.scope);
    let artifacts = crate::rubric::load_artifacts(repo);
    let mut findings = Vec::new();
    let mut checked = 0usize;
    let mut truncated = false;
    for name in &cfg.rubrics {
        let path = rubrics_dir.join(format!("{name}.yaml"));
        let rubric = match crate::rubric::load(&path) {
            Ok(r) => r,
            Err(e) => {
                findings.push(GateFinding::ruled(
                    "error".to_string(),
                    "semantic_rubric_unknown".to_string(),
                    format!(
                        "рубрика '{name}' не загружается из {}: {e} — составляющая настроена \
                         на рубрику, которой нет",
                        rubrics_dir.display()
                    ),
                ));
                continue;
            }
        };
        let Some(kind) = rubric.pack else {
            findings.push(GateFinding::ruled(
                "error".to_string(),
                "semantic_rubric_without_pack".to_string(),
                format!(
                    "рубрика '{name}' не объявляет вид досье (`pack:`) — собирать вход судьи \
                     нечем; смысловая составляющая работает только с рубриками по досье"
                ),
            ));
            continue;
        };
        let subjects = semantic_subjects(repo, kind, changed.as_ref());
        if subjects.len() > MAX_SEMANTIC_SUBJECTS {
            truncated = true;
        }
        for subject in subjects.into_iter().take(MAX_SEMANTIC_SUBJECTS) {
            let state = semantic_subject_state(repo, &rubric, kind, &subject, &artifacts, cfg);
            checked += usize::from(!matches!(state, SemanticState::Skipped));
            findings.extend(state.into_findings(name, &subject));
        }
    }
    let errors = findings.iter().filter(|f| f.severity == "error").count();
    if checked == 0 && findings.is_empty() {
        return GateComponent::skip(
            "semantic_quality",
            format!(
                "обязательных субъектов не нашлось (рубрик: {}, область: {}); {}",
                cfg.rubrics.len(),
                scope_label(cfg.scope),
                if changed.as_ref().is_some_and(Vec::is_empty) {
                    "диффом не затронут ни один субъект"
                } else {
                    "в пакете нет подходящих субъектов"
                }
            ),
        );
    }
    let mut detail = format!(
        "рубрик: {}, субъектов: {checked}, находок: {} (error: {errors}); порог {:.2}",
        cfg.rubrics.len(),
        findings.len(),
        cfg.min_score
    );
    if truncated {
        let _ = write!(
            detail,
            "; субъектов в пакете больше потолка {MAX_SEMANTIC_SUBJECTS} — проверены первые"
        );
    }
    let notes = vec![
        "балл и обвинение — суждение LLM-судьи по досье; механика сверяет хэш досье, \
         порог и подтверждённость цитат, но не качество суждения"
            .to_string(),
    ];
    if errors == 0 {
        GateComponent::pass_with_findings("semantic_quality", detail, findings).noting(notes)
    } else {
        GateComponent::fail("semantic_quality", detail, findings).noting(notes)
    }
}

/// Область субъектов для вывода.
fn scope_label(scope: crate::config::SemanticScope) -> &'static str {
    match scope {
        crate::config::SemanticScope::Changed => "changed",
        crate::config::SemanticScope::All => "all",
    }
}

/// Изменённые файлы для области `changed`; `None` — область `all` (ограничения
/// по диффу нет).
fn changed_files_for_semantics(
    repo: &Path,
    base: Option<&str>,
    git: &GitProbe,
    scope: crate::config::SemanticScope,
) -> Option<Vec<String>> {
    if scope == crate::config::SemanticScope::All || !git.repo {
        return None;
    }
    delta::guard(repo, base, &[]).ok().map(|r| r.changed_files)
}

/// Состояние субъекта: что о нём говорит отчёт судьи.
enum SemanticState {
    /// Проверять нечего (субъект выпал из области).
    Skipped,
    /// Отчёта нет.
    Missing,
    /// Досье изменилось после оценки.
    Stale(String),
    /// Обвинение подтверждено цитатами по обеим ролям — блокирующая находка.
    Contradiction {
        /// Главный критерий.
        criterion: String,
        /// Балл.
        score: u8,
        /// Цитаты из обоснования (обе стороны).
        quotes: String,
    },
    /// Главный критерий низкий, но обвинение не подтверждено.
    Unconfirmed(String),
    /// Высокий балл без полного перечня проверенного.
    CoverageIncomplete(Vec<String>),
    /// Взвешенный итог ниже порога.
    Low(f64, String),
    /// Судья и автор — одна модель.
    JudgeIsAuthor(String),
    /// Всё в порядке.
    Ok,
}

impl SemanticState {
    /// Находки состояния; пусто — состояние не оставляет следа в отчёте.
    fn into_findings(self, rubric: &str, subject: &SemanticSubject) -> Vec<GateFinding> {
        let who = format!("{} '{}' (рубрика {rubric})", "субъект", subject.subject);
        match self {
            Self::Skipped | Self::Ok => Vec::new(),
            Self::Missing => vec![GateFinding::ruled(
                "error".to_string(),
                "semantic_report_missing".to_string(),
                format!(
                    "{who}: нет отчёта смысловой рубрики — субъект не оценён; прогоните \
                     rubric_prompt → rubric_verify (pack/subject) или `arch-be rubric run`"
                ),
            )],
            Self::Stale(source) => vec![GateFinding::ruled(
                "error".to_string(),
                "semantic_report_stale".to_string(),
                format!(
                    "{who}: отчёт устарел — после оценки изменился источник досье ({source}); \
                     это может быть правка спайна, а не субъекта"
                ),
            )],
            Self::Contradiction {
                criterion,
                score,
                quotes,
            } => vec![GateFinding::ruled(
                "error".to_string(),
                "semantic_contradiction".to_string(),
                format!(
                    "{who}: главный критерий '{criterion}' = {score} — обвинение подтверждено \
                     цитатами: {quotes}"
                ),
            )],
            Self::Unconfirmed(detail) => vec![GateFinding::ruled(
                "warn".to_string(),
                "semantic_accusation_unconfirmed".to_string(),
                format!(
                    "{who}: главный критерий низкий, но цитаты не подтверждены ({detail}) — \
                     критерий исключён из итога, гейт этим не краснеет"
                ),
            )],
            Self::CoverageIncomplete(criteria) => vec![GateFinding::ruled(
                "warn".to_string(),
                "semantic_coverage_incomplete".to_string(),
                format!(
                    "{who}: высокий балл без полного перечня проверенных источников ({}) — \
                     критерии исключены из итога",
                    criteria.join(", ")
                ),
            )],
            Self::Low(total, judge) => vec![GateFinding::ruled(
                // warn, а не error: взвешенный итог смешивает качество документа
                // с дисциплиной цитирования судьи — на чистом контроле живого
                // прогона 2026-09-20 три критерия из четырёх остались без
                // свидетельств, и итог 3.43 стоял в 0.43 от порога. Ошибкой
                // краснело бы честное решение за поведение судьи. Красный —
                // только за обвинение по главному критерию с цитатами.
                "warn".to_string(),
                "semantic_quality_low".to_string(),
                format!("{who}: {total:.2}/5 ниже порога (судья {judge})"),
            )],
            Self::JudgeIsAuthor(judge) => vec![GateFinding::ruled(
                "warn".to_string(),
                "judge_is_author".to_string(),
                format!("{who}: судья и автор — одна модель ({judge}) — оценка не независима"),
            )],
        }
    }
}

/// Что говорит отчёт по субъекту: свежесть досье, главный критерий, порог.
fn semantic_subject_state(
    repo: &Path,
    rubric: &crate::rubric::Rubric,
    kind: crate::rubric_pack::PackKind,
    subject: &SemanticSubject,
    artifacts: &[crate::rubric::RubricArtifact],
    cfg: &crate::config::SemanticQualityConfig,
) -> SemanticState {
    let Ok(packs) = crate::rubric_pack::build(repo, kind, &subject.subject) else {
        // Досье не собирается (секрет, лимит, битый маркер) — это не «нет
        // отчёта», а «судить нечем»; называем причину как пропуск.
        return SemanticState::Skipped;
    };
    let artifact = artifacts.iter().rev().find(|a| {
        a.rubric == rubric.name && a.subject.as_deref() == Some(subject.subject.as_str())
    });
    let Some(artifact) = artifact else {
        return SemanticState::Missing;
    };
    // Отчёт привязан ко ВСЕМ источникам досье (ADR-051, П3): правка спайна
    // обесценивает отчёт о решении, даже если сам ADR не менялся.
    if let Some(want) = artifact.pack_sha256.as_deref() {
        let fresh = packs.iter().find(|p| p.subject == subject.subject);
        match fresh {
            Some(pack) if pack.sha256 == want => {}
            Some(pack) => {
                let changed = changed_source(artifact, pack);
                return SemanticState::Stale(changed);
            }
            None => return SemanticState::Skipped,
        }
    }
    let mut state = SemanticState::Ok;
    // Обвинение по главному критерию.
    if let Some(main) = rubric.criteria.iter().find(|c| c.blocking) {
        if let Some(score) = artifact.scores.iter().find(|s| s.criterion_id == main.id) {
            if score.score <= 2 {
                if score.flags.iter().any(|f| f.excludes_from_total()) {
                    state = SemanticState::Unconfirmed(format!("метки: {:?}", score.flags));
                } else {
                    state = SemanticState::Contradiction {
                        criterion: main.id.clone(),
                        score: score.score,
                        quotes: score.rationale.clone(),
                    };
                }
            }
        }
    }
    // Взвешенный итог ниже порога — факт, подтверждённый счётом: критерий без
    // подтверждённых цитат в итог не входит (`excludes_from_total`), поэтому
    // «обвинение не подтверждено» не отменяет низкий итог и не должно его
    // вытеснять. Живой прогон D11 (2026-09-20): главный критерий 1 с меткой
    // `accusation_unconfirmed` утопил итог 1.00/5 в предупреждение, и гейт
    // перестал бы краснеть на коде, нарушающем инвариант. Порядок силы:
    // противоречие → низкий итог → неподтверждённое обвинение.
    if artifact.weighted_total < cfg.min_score
        && matches!(
            state,
            SemanticState::Ok
                | SemanticState::Unconfirmed(_)
                | SemanticState::CoverageIncomplete(_)
        )
    {
        state = SemanticState::Low(artifact.weighted_total, artifact.judge_model.clone());
    }
    if matches!(state, SemanticState::Ok) {
        let uncovered: Vec<String> = artifact
            .scores
            .iter()
            .filter(|s| s.has_flag(crate::rubric::CriterionFlag::CoverageIncomplete))
            .map(|s| s.criterion_id.clone())
            .collect();
        if !uncovered.is_empty() {
            state = SemanticState::CoverageIncomplete(uncovered);
        }
    }
    // «Автор = судья» — отдельная находка, но она не перекрывает суть
    // (обвинение или пропуск отчёта): печатается только на спокойном итоге.
    if matches!(state, SemanticState::Ok) {
        let author_missing = artifact
            .author_model
            .as_deref()
            .is_none_or(|a| a.trim().is_empty());
        if author_missing || artifact.author_model.as_deref() == Some(artifact.judge_model.as_str())
        {
            state = SemanticState::JudgeIsAuthor(artifact.judge_model.clone());
        }
    }
    state
}

/// Какой источник досье изменился после оценки — поимённо, для сообщения.
fn changed_source(
    artifact: &crate::rubric::RubricArtifact,
    pack: &crate::rubric_pack::ContextPack,
) -> String {
    for input in &pack.inputs {
        let was = artifact
            .inputs
            .iter()
            .find(|i| i.path == input.path)
            .map(|i| i.sha256.as_str());
        match was {
            None => return format!("{} — источник появился после оценки", input.path),
            Some(sha) if sha != input.sha256 => {
                return format!("{} — изменён после оценки", input.path);
            }
            Some(_) => {}
        }
    }
    "состав досье изменился".to_string()
}

/// Субъекты рубрики в пакете; для области `changed` — только те, чьё досье
/// затронуто диффом (сам субъект или любой его источник, включая спайн).
fn semantic_subjects(
    repo: &Path,
    kind: crate::rubric_pack::PackKind,
    changed: Option<&Vec<String>>,
) -> Vec<SemanticSubject> {
    let mut out = Vec::new();
    let mut push = |subject: String| {
        if out.iter().any(|s: &SemanticSubject| s.subject == subject) {
            return;
        }
        out.push(SemanticSubject { subject });
    };
    match kind {
        crate::rubric_pack::PackKind::AdrVsSpine => {
            for adr in accepted_adr_paths(repo) {
                push(adr);
            }
        }
        crate::rubric_pack::PackKind::EntityLinks | crate::rubric_pack::PackKind::NfrMechanism => {
            for id in linked_entities(repo, kind) {
                push(id);
            }
        }
        crate::rubric_pack::PackKind::CodeVsSpine => {
            for file in code_files(repo) {
                push(file);
            }
        }
    }
    let Some(changed) = changed else {
        return out;
    };
    // Область `changed`: субъект остаётся, если затронут он сам или любой
    // источник его досье — правка спайна меняет вердикт о решении, хотя
    // решение не правили.
    out.retain(|s| {
        let Ok(packs) = crate::rubric_pack::build(repo, kind, &s.subject) else {
            return false;
        };
        packs.iter().any(|p| {
            p.inputs
                .iter()
                .any(|i| changed.iter().any(|c| same_path(c, i.path.as_str())))
        })
    });
    out
}

/// Один и тот же файл: пути сравниваются по нормализованной форме (слеши,
/// суффикс `#фрагмент`, ведущее `./`).
fn same_path(a: &str, b: &str) -> bool {
    let norm = |p: &str| {
        p.split('#')
            .next()
            .unwrap_or(p)
            .trim_start_matches("./")
            .replace('\\', "/")
    };
    norm(a) == norm(b)
}

/// Принятые ADR (`docs/adr/ADR-*.md`).
fn accepted_adr_paths(repo: &Path) -> Vec<String> {
    let dir = repo.join("docs/adr");
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut out: Vec<String> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.extension().is_some_and(|x| x.eq_ignore_ascii_case("md"))
                && p.file_name()
                    .is_some_and(|n| n.to_string_lossy().starts_with("ADR-"))
        })
        .filter(|p| std::fs::read_to_string(p).is_ok_and(|text| adr_is_accepted(&text)))
        .map(|p| crate::rubric_pack::relative_path(repo, &p))
        .collect();
    out.sort();
    out
}

/// Сущности модели, которые стоит судить: со связями и (для `nfr_mechanism`)
/// только показатели. Карточка без связей смысловой рубрике не о чём.
fn linked_entities(repo: &Path, kind: crate::rubric_pack::PackKind) -> Vec<String> {
    let Ok(model) = crate::model::load_model_tolerant(&repo.join("model")) else {
        return Vec::new();
    };
    let mut out: Vec<String> = model
        .entities
        .iter()
        .filter(|e| {
            e.depends_on
                .iter()
                .chain(&e.implements)
                .chain(&e.affects)
                .chain(&e.verified_by)
                .count()
                > 0
        })
        .filter(|e| {
            kind != crate::rubric_pack::PackKind::NfrMechanism
                || e.kind == crate::model::EntityKind::Nfr
        })
        .map(|e| e.id.clone())
        .collect();
    out.sort();
    out
}

/// Файлы кода под корнями компонент (`CMP.code_roots`) — субъекты рубрики
/// «код против инварианта».
fn code_files(repo: &Path) -> Vec<String> {
    let Ok(model) = crate::model::load_model_tolerant(&repo.join("model")) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for root in model.entities.iter().flat_map(|e| e.code_roots.iter()) {
        collect_code_files(repo, &repo.join(root), &mut out);
    }
    out.sort();
    out.dedup();
    out
}

/// Рекурсивный обход каталога кода с пропуском служебных каталогов.
fn collect_code_files(repo: &Path, dir: &Path, out: &mut Vec<String>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in rd.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if path.is_dir() {
            if SEMANTIC_SKIP_DIRS.contains(&name.as_str()) || name.starts_with('.') {
                continue;
            }
            collect_code_files(repo, &path, out);
        } else if path.is_file() {
            out.push(crate::rubric_pack::relative_path(repo, &path));
        }
    }
}

/// Вычисляет маршрут из git-диффа (`--route auto`): [`control::detect_diff_triggers`]
/// и [`control::score_with_sources`] с пустым declared (механический минимум
/// S-1, ADR-034). Дифф недоступен (не git-репозиторий, нет HEAD) — fail-safe
/// маршрут Critical с пометкой причины.
fn auto_route(
    repo: &Path,
    base: Option<&str>,
    limits: (usize, usize),
    globs: &control::DiffGlobs,
) -> (Route, String) {
    match control::detect_diff_triggers_with(repo, base, globs) {
        Ok(diff) => {
            let scored = control::score_with_sources(&BTreeMap::new(), &diff, limits.0, limits.1);
            let fired = if scored.significance.fired.is_empty() {
                "триггеров нет".to_string()
            } else {
                scored.significance.fired.join(", ")
            };
            let excluded_note = if diff.excluded.is_empty() {
                String::new()
            } else {
                format!(
                    "; исключено по манифесту connect/.spineignore: {} файлов",
                    diff.excluded.len()
                )
            };
            (
                scored.significance.route,
                format!(
                    "auto: score {} ({fired}){excluded_note}",
                    scored.significance.score
                ),
            )
        }
        Err(e) => (
            Route::Critical,
            format!("auto: дифф недоступен ({e}) — fail-safe маршрут Critical"),
        ),
    }
}

/// Заявленный маршрут репозитория из `.arch-handoff/ROUTE.lock` (П4 ДКА).
#[derive(Debug, Clone)]
struct RouteLock {
    /// Минимальный маршрут контроля для репозитория.
    route: Route,
    /// Кем решён (ожидается ссылка на ADR при понижении).
    decided_by: Option<String>,
}

/// Сырой YAML `ROUTE.lock`.
#[derive(Debug, serde::Deserialize)]
struct RouteLockRaw {
    /// Маршрут строкой (`fast|standard|critical`).
    route: String,
    /// Ссылка на решение (ADR-…).
    #[serde(default)]
    decided_by: Option<String>,
}

/// Ранг маршрута для операции «не ниже»: Fast < Standard < Critical.
fn route_rank(route: Route) -> u8 {
    match route {
        Route::Fast => 0,
        Route::Standard => 1,
        Route::Critical => 2,
    }
}

/// Разбирает `ROUTE.lock`; невалидный YAML/маршрут — `None` (fail-soft:
/// файла нет или он битый не должен валить гейт, но и не поднимает маршрут).
fn parse_route_lock(text: &str) -> Option<RouteLock> {
    let raw: RouteLockRaw = serde_yaml_ng::from_str(text).ok()?;
    let route = raw
        .route
        .trim()
        .to_ascii_lowercase()
        .parse::<Route>()
        .ok()?;
    Some(RouteLock {
        route,
        decided_by: raw.decided_by,
    })
}

/// Путь `ROUTE.lock` в репозитории (пакетный, затем корневой).
fn route_lock_path(repo: &Path) -> Option<PathBuf> {
    let handoff = repo.join(".arch-handoff/ROUTE.lock");
    if handoff.is_file() {
        return Some(handoff);
    }
    let root = repo.join("ROUTE.lock");
    root.is_file().then_some(root)
}

/// Заявленный маршрут репозитория.
fn read_route_lock(repo: &Path) -> Option<RouteLock> {
    let path = route_lock_path(repo)?;
    let text = std::fs::read_to_string(path).ok()?;
    parse_route_lock(&text)
}

/// Составляющая `route_lock` (П4): заявленный маршрут и анти-понижение.
/// Понижение относительно git-базы без `decided_by: ADR-…` — FAIL
/// (по образцу `rule_weakened`).
fn component_route_lock(
    repo: &Path,
    base: Option<&str>,
    git: &GitProbe,
    lock: &RouteLock,
) -> GateComponent {
    let rel = if repo.join(".arch-handoff/ROUTE.lock").is_file() {
        ".arch-handoff/ROUTE.lock"
    } else {
        "ROUTE.lock"
    };
    let decided = lock.decided_by.as_deref().unwrap_or("без ADR");
    let detail = format!(
        "заявленный маршрут: {} ({decided}) — файл: {rel}",
        lock.route
    );
    if !git.repo || !git.head {
        return GateComponent::pass("route_lock", detail);
    }
    let rev = base_rev(base.unwrap_or("HEAD"));
    let Some(git_rel) = git_rel_path(repo, &repo.join(rel)) else {
        return GateComponent::pass("route_lock", detail);
    };
    if !git_rev_exists(repo, rev) || !git_rev_has_path(repo, rev, &git_rel) {
        return GateComponent::pass("route_lock", detail);
    }
    let Ok(base_src) = git_show_file(repo, rev, &git_rel) else {
        return GateComponent::pass("route_lock", detail);
    };
    if let Some(base_lock) = parse_route_lock(&base_src) {
        if route_rank(base_lock.route) > route_rank(lock.route) {
            let has_adr = lock
                .decided_by
                .as_deref()
                .is_some_and(|d| d.trim().to_ascii_uppercase().starts_with("ADR"));
            if !has_adr {
                return GateComponent::fail(
                    "route_lock",
                    format!(
                        "route_lowered: маршрут понижен {} → {} без ADR — файл: {rel}",
                        base_lock.route, lock.route
                    ),
                    vec![GateFinding::text(
                        "error",
                        "понижение заявленного маршрута требует decided_by со ссылкой на ADR"
                            .to_string(),
                    )],
                );
            }
            return GateComponent::pass(
                "route_lock",
                format!(
                    "{detail}; понижение {} → {} подтверждено ADR",
                    base_lock.route, lock.route
                ),
            );
        }
    }
    GateComponent::pass("route_lock", detail)
}

/// Прогоняет единый гейт по репозиторию с матрицей обязательности по
/// умолчанию (П1 ДКА). Полная форма — [`run_with`].
///
/// # Errors
/// Репозиторий недоступен. Провалы составляющих — НЕ ошибка: они в отчёте
/// (`outcome`/`passed = false`), exit-код ставит CLI-край.
pub fn run(
    repo: &Path,
    route_override: Option<Route>,
    base: Option<&str>,
    constraints: Option<&Path>,
    limits: (usize, usize),
) -> Result<GateReport> {
    run_with(
        repo,
        route_override,
        base,
        constraints,
        limits,
        &GateRequirements::default(),
    )
}

/// Полная форма: матрица обязательности + настройки семантики (0.3.4).
///
/// # Errors
/// Репозиторий недоступен. Провалы составляющих — НЕ ошибка.
pub fn run_opts(
    repo: &Path,
    route_override: Option<Route>,
    base: Option<&str>,
    constraints: Option<&Path>,
    limits: (usize, usize),
    requirements: &GateRequirements,
    options: &GateOptions,
) -> Result<GateReport> {
    run_inner(
        repo,
        route_override,
        base,
        constraints,
        limits,
        requirements,
        options,
    )
}

/// Прогоняет единый гейт по репозиторию с заданной матрицей обязательных
/// составляющих (`[gate.required]` конфига, П1 ДКА).
///
/// # Errors
/// Репозиторий недоступен. Провалы составляющих — НЕ ошибка: они в отчёте
/// (`outcome`/`passed = false`), exit-код ставит CLI-край.
pub fn run_with(
    repo: &Path,
    route_override: Option<Route>,
    base: Option<&str>,
    constraints: Option<&Path>,
    limits: (usize, usize),
    requirements: &GateRequirements,
) -> Result<GateReport> {
    run_inner(
        repo,
        route_override,
        base,
        constraints,
        limits,
        requirements,
        &GateOptions::default(),
    )
}

/// Тело гейта: единая точка сборки состава и настроек.
fn run_inner(
    repo: &Path,
    route_override: Option<Route>,
    base: Option<&str>,
    constraints: Option<&Path>,
    limits: (usize, usize),
    requirements: &GateRequirements,
    options: &GateOptions,
) -> Result<GateReport> {
    if !repo.is_dir() {
        return Err(HarnessError::Control(format!(
            "репозиторий недоступен: {}",
            repo.display()
        )));
    }
    let (mut route, route_auto, mut route_note) = if let Some(r) = route_override {
        (r, false, format!("явный --route {r}"))
    } else {
        let (r, note) = auto_route(repo, base, limits, &options.diff_globs);
        (r, true, note)
    };
    // П4: храповик маршрута — эффективный маршрут не ниже заявленного в
    // ROUTE.lock. Критический проект проверяется как Critical даже на чистом
    // дереве и на маленьком MR (раньше auto давал Fast).
    let route_lock = read_route_lock(repo);
    if let Some(lock) = &route_lock {
        if route_rank(lock.route) > route_rank(route) {
            route_note = format!(
                "{route_note}; поднят ROUTE.lock → {} ({})",
                lock.route,
                lock.decided_by.as_deref().unwrap_or("без ADR")
            );
            route = lock.route;
        }
    }
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
        component_trace(repo, options),
        // Н2: целостность модели — часть гейта на ЛЮБОМ маршруте (SKIP без
        // каталога model/); обязательность по маршрутам — в `[gate.required]`.
        component_model_validate(repo, route),
    ];
    if matches!(route, Route::Standard | Route::Critical) {
        components.push(component_sensors(repo));
        components.push(component_nfr(repo));
        components.push(component_evidence(repo, &options.evidence));
    }
    if let Some(lock) = &route_lock {
        components.push(component_route_lock(repo, base, &git, lock));
    }
    // Н7: качество решений — необязательная составляющая; включается только
    // через `[gate.required]` (по умолчанию SKIP, чтобы не краснить чужие
    // пайплайны без предупреждения).
    let required_names = requirements.for_route(route);
    components.push(component_decision_quality(
        repo,
        options,
        required_names.iter().any(|r| r == "decision_quality"),
    ));
    // ADR-052: смысловые рубрики — та же дисциплина, что у `decision_quality`:
    // необязательная составляющая, включается только через `[gate.required]`.
    components.push(component_semantic_quality(
        repo,
        &options.semantic_quality,
        &options.rubrics_dir,
        base,
        &git,
        required_names.iter().any(|r| r == "semantic_quality"),
    ));
    let mut report = GateReport {
        repo: repo.to_path_buf(),
        route,
        route_auto,
        route_note,
        components,
        outcome: GateOutcome::Pass,
        required: requirements.for_route(route).to_vec(),
        not_checked: Vec::new(),
        inputs: collect_inputs(repo, &constraints.path, base, &git),
        attestation: String::new(),
        passed: true,
    };
    report.recompute();
    Ok(report)
}

/// Хэши входов вердикта (П7): чем состояние репозитория отличалось при
/// прогоне — реестр правил, спайн, модель, бандлы доказательств, ROUTE.lock и
/// коммит базы диффа (Н3, ADR-043).
///
/// Только ОТНОСИТЕЛЬНЫЕ пути и никакого времени: тот же коммит, склонированный
/// в другой каталог, обязан дать ту же аттестацию. Отсутствующий вход —
/// честное `absent`, а не пустой хэш.
#[must_use]
fn collect_inputs(
    repo: &Path,
    constraints: &Path,
    base: Option<&str>,
    git: &GitProbe,
) -> Vec<(String, String)> {
    let mut inputs: Vec<(String, String)> = Vec::new();
    let mut push = |name: &str, value: String| inputs.push((name.to_string(), value));

    push(
        "constraints",
        crate::hash::sha256_file(constraints)
            .map_or_else(|| "absent".to_string(), |h| format!("sha256:{h}")),
    );
    // T-02: ПО КАКОМУ реестру судили и сколько в нём правил. Хэш отвечает
    // «тот же файл или нет», но не отвечает «а какой файл-то»: при двух
    // копиях (пакетная приоритетна, корневая — fallback) это первое, что
    // нужно знать читателю вердикта. Путь относительный — аттестация не
    // зависит от каталога, куда склонирован репозиторий.
    push("constraints_path", constraints_label(repo, constraints));
    push(
        "constraints_rules",
        control::load_constraints_resolved(constraints)
            .map_or_else(|_| "unreadable".to_string(), |r| r.rules.len().to_string()),
    );
    // Спайн: оба исторических расположения (как у `spine_lint`).
    let spine = ["ARCHITECTURE-SPINE.md", "docs/ARCHITECTURE-SPINE.md"]
        .iter()
        .map(|p| repo.join(p))
        .find(|p| p.is_file());
    push(
        "spine",
        spine
            .and_then(|p| crate::hash::sha256_file(&p))
            .map_or_else(|| "absent".to_string(), |h| format!("sha256:{h}")),
    );
    push(
        "model",
        crate::hash::sha256_tree(&repo.join("model"))
            .map_or_else(|| "absent".to_string(), |h| format!("sha256:{h}")),
    );
    // Сырые ответы судьи рубрик: отчёт объявлен собранным ИЗ НИХ, поэтому
    // правка сохранённого ответа меняет вердикт о качестве решения — а значит
    // обязана менять и аттестацию (J5, ADR-048). Каталога нет (отчётов нет
    // либо они до появления сырых ответов) — честное `absent`: аттестация
    // существующих кейсов не меняется.
    push(
        "judge_raw",
        crate::hash::sha256_tree(&repo.join(crate::judge::RUBRIC_RAW_DIR))
            .map_or_else(|| "absent".to_string(), |h| format!("sha256:{h}")),
    );
    // Каждый проверенный бандл: правка EVIDENCE.yaml обязана менять аттестацию.
    let bundles = evidence_bundle_dirs(repo);
    push("evidence_bundles", bundles.len().to_string());
    for dir in &bundles {
        let rel = dir.strip_prefix(repo).map_or_else(
            |_| ".".to_string(),
            |p| {
                if p.as_os_str().is_empty() {
                    ".".to_string()
                } else {
                    p.display().to_string()
                }
            },
        );
        push(
            &format!("evidence:{rel}"),
            crate::hash::sha256_file(&dir.join("EVIDENCE.yaml"))
                .map_or_else(|| "absent".to_string(), |h| format!("sha256:{h}")),
        );
    }
    // Отчёты рубрик: правивший отчёт меняет вердикт, и аттестация обязана это
    // видеть — иначе подмена отчёта на «удобный» не отличима от прежнего
    // состояния. Касается и `decision_quality`, и смысловых рубрик (ADR-052):
    // отчёт смысловой рубрики входит в конверт тем же правилом.
    let reports_dir = repo.join(crate::rubric::RUBRIC_REPORTS_DIR);
    let mut reports: Vec<PathBuf> = std::fs::read_dir(&reports_dir)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.path())
                .filter(|p| {
                    p.extension()
                        .is_some_and(|x| x.eq_ignore_ascii_case("json"))
                })
                .collect()
        })
        .unwrap_or_default();
    reports.sort();
    push("rubric_reports", reports.len().to_string());
    for path in &reports {
        let name = path.file_name().map_or_else(
            || "report".to_string(),
            |n| n.to_string_lossy().into_owned(),
        );
        push(
            &format!("rubric_report:{name}"),
            crate::hash::sha256_file(path)
                .map_or_else(|| "absent".to_string(), |h| format!("sha256:{h}")),
        );
    }
    let lock = route_lock_path(repo);
    push(
        "route_lock",
        lock.and_then(|p| crate::hash::sha256_file(&p))
            .map_or_else(|| "absent".to_string(), |h| format!("sha256:{h}")),
    );
    // База диффа — коммитом, а не строкой аргумента: `HEAD~1` и его SHA
    // описывают одно состояние и обязаны дать одну аттестацию.
    // T-03: база приходит и голой ревизией, и диапазоном (`origin/main...HEAD`)
    // — для `git rev-parse` годится только одиночная ревизия, иначе коммит
    // базы молча уезжал в `absent`.
    let base_commit = if git.repo {
        git_resolve(repo, control::base_rev(base.unwrap_or("HEAD")))
    } else {
        None
    };
    push("base", base_commit.unwrap_or_else(|| "absent".to_string()));
    inputs
}

/// Резолвит git-ревизию в полный SHA коммита (None — не резолвится).
fn git_resolve(repo: &Path, rev: &str) -> Option<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["rev-parse", "--verify", &format!("{rev}^{{commit}}")])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let sha = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if sha.is_empty() { None } else { Some(sha) }
}

/// Расхождение между вердиктом из конверта и текущим состоянием дерева.
#[derive(Debug, Clone)]
pub struct EnvelopeDrift {
    /// Входы, изменившиеся с момента вердикта: `(вход, было, стало)`.
    pub changed: Vec<(String, String, String)>,
    /// Входы, которые были в вердикте, но пропали с дерева.
    pub missing: Vec<String>,
    /// Вердикт относится к текущему состоянию.
    pub same: bool,
}

/// Сверяет конверт вердикта с текущим состоянием репозитория: аттестация
/// привязывает зелёный к ВХОДАМ (Н3, ADR-043), поэтому «воспроизводим по
/// конверту» — это механическая проверка, а не обещание.
///
/// Сравниваются входы (реестр правил, спайн, модель, бандлы, `ROUTE.lock`,
/// коммит базы). Состав находок не пересчитывается: для этого нужен прогон
/// `arch-be gate` с теми же флагами.
///
/// # Errors
/// Файл конверта не читается, не JSON или не конверт `gate-verdict`.
pub fn verify_envelope(
    repo: &Path,
    envelope_path: &Path,
    limits: (usize, usize),
) -> Result<EnvelopeDrift> {
    let text =
        std::fs::read_to_string(envelope_path).map_err(|e| HarnessError::io(envelope_path, e))?;
    let value: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| HarnessError::Config(format!("{}: не JSON ({e})", envelope_path.display())))?;
    let declared = value
        .get("inputs")
        .and_then(|v| v.as_object())
        .ok_or_else(|| {
            HarnessError::Config(format!(
                "{}: нет секции `inputs` — это не конверт arch-be/gate-verdict",
                envelope_path.display()
            ))
        })?;
    // Базу диффа берём из самого конверта, если она там сохранена; иначе —
    // HEAD текущего дерева (конверт и дерево в одном репозитории).
    let base_ref: Option<String> = match value
        .get("inputs")
        .and_then(|i| i.get("base"))
        .and_then(|b| b.as_str())
    {
        Some(sha) if sha != "absent" => Some(sha.to_string()),
        _ => None,
    };
    let current = collect_inputs_for_verify(repo, base_ref.as_deref(), limits);
    let mut changed = Vec::new();
    let mut missing = Vec::new();
    for (name, was) in declared {
        let was = was.as_str().unwrap_or_default().to_string();
        match current.get(name) {
            Some(now) if *now == was => {}
            Some(now) => {
                if was == "absent" {
                    missing.push(name.clone());
                } else {
                    changed.push((name.clone(), was, now.clone()));
                }
            }
            None => missing.push(name.clone()),
        }
    }
    Ok(EnvelopeDrift {
        same: changed.is_empty() && missing.is_empty(),
        changed,
        missing,
    })
}

/// Входы текущего дерева для сверки с конвертом: те же, что у прогона, с тем
/// же резолвом `CONSTRAINTS.yaml` (единый резолвер E2).
fn collect_inputs_for_verify(
    repo: &Path,
    base: Option<&str>,
    _limits: (usize, usize),
) -> BTreeMap<String, String> {
    let constraints = control::resolve_constraints_path_detailed(repo, None)
        .map_or_else(|| repo.join(control::HANDOFF_CONSTRAINTS_PATH), |r| r.path);
    let git = GitProbe::probe(repo);
    collect_inputs(repo, &constraints, base, &git)
        .into_iter()
        .collect()
}

/// Текстовый рендер отчёта гейта: строка маршрута, по каждой составляющей
/// PASS/FAIL/SKIP + краткая причина, находки отступом (с потолком
/// [`MAX_COMPONENT_FINDINGS`]), итоговая строка `Итог: PASS/FAIL/INCOMPLETE`.
#[must_use]
pub fn render(report: &GateReport) -> String {
    let mut out = String::new();
    // Запись в String не может завершиться ошибкой — игноры безопасны.
    let _ = writeln!(out, "Гейт: {}", report.repo.display());
    let _ = writeln!(out, "Маршрут: {} ({})", report.route, report.route_note);
    for c in &report.components {
        let req = if report.required.iter().any(|r| r == c.name) {
            " *"
        } else {
            ""
        };
        let _ = writeln!(
            out,
            "  [{}] {}{req} — {}",
            c.status.label(),
            c.name,
            c.detail
        );
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
        match report.outcome {
            GateOutcome::Pass => "PASS".to_string(),
            GateOutcome::Fail => format!("FAIL — провалено составляющих: {failed} (exit 1)"),
            GateOutcome::Incomplete => format!(
                "INCOMPLETE — обязательные составляющие без входа: {} (exit 3)",
                report.not_checked.join(", ")
            ),
        }
    );
    if !report.not_checked.is_empty() {
        let _ = writeln!(
            out,
            "Не проверено (обязательно для маршрута {}): {}",
            report.route,
            report.not_checked.join(", ")
        );
    }
    if !report.attestation.is_empty() {
        let _ = writeln!(out, "Аттестация вердикта: sha256:{}", report.attestation);
    }
    // Квитанция ценности (аддитивная строка): сумма error-находок всех
    // составляющих — это дефекты, остановленные механикой до ревью.
    if report.outcome == GateOutcome::Fail {
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

    // --- ADR-052: смысловые рубрики как составляющая гейта ------------------

    /// Каталог рубрик с одной смысловой рубрикой (`adr_vs_spine`): берём
    /// встроенную, чтобы тест проверял настоящий контракт рубрики.
    fn semantic_rubrics_dir(dir: &Path) -> PathBuf {
        let rubrics = dir.join("rubrics");
        std::fs::create_dir_all(&rubrics).expect("mkdir rubrics");
        std::fs::write(
            rubrics.join("adr_spine_consistency.yaml"),
            crate::assets::RUBRIC_ADR_SPINE_CONSISTENCY,
        )
        .expect("рубрика");
        rubrics
    }

    /// Настройки составляющей на одну рубрику.
    fn semantic_cfg(scope: crate::config::SemanticScope) -> crate::config::SemanticQualityConfig {
        crate::config::SemanticQualityConfig {
            rubrics: vec!["adr_spine_consistency".to_string()],
            scope,
            min_score: 3.5,
            require_distinct_judge: false,
        }
    }

    /// Требования с включённой смысловой составляющей.
    fn with_semantic(route: Route) -> GateRequirements {
        let mut req = GateRequirements::default();
        let list = match route {
            Route::Fast => &mut req.fast,
            Route::Standard => &mut req.standard,
            Route::Critical => &mut req.critical,
        };
        list.push("semantic_quality".to_string());
        req
    }

    /// Репозиторий с Accepted-ADR и инвариантом спайна — субъект и ссылка
    /// досье `adr_vs_spine`.
    fn make_semantic_repo(dir: &Path) -> PathBuf {
        make_gate_repo(dir);
        std::fs::create_dir_all(dir.join("docs/adr")).expect("mkdir adr");
        let adr = dir.join("docs/adr/ADR-001-reshenie.md");
        std::fs::write(
            &adr,
            "# ADR-001. Решение\n\n- Date: 2026-09-19\n- Status: Accepted\n\n## Context\n\nПричина.\n",
        )
        .expect("adr");
        std::fs::write(
            dir.join("ARCHITECTURE-SPINE.md"),
            "# Spine\n\n## AD-1: Журнал только дописывается\n\n\
             - **Binds**: журнал операций ↔ код записи\n\
             - **Prevents**: правку и удаление строк журнала\n\
             - **Rule**: строки журнала не правятся.\n",
        )
        .expect("spine");
        git(dir, &["add", "."]);
        git(dir, &["commit", "-q", "--allow-empty", "-m", "semantic"]);
        adr
    }

    /// Отчёт смысловой рубрики в репозитории: `pack_sha256` считается по
    /// текущему досье, если не задан иначе.
    fn write_semantic_report(
        dir: &Path,
        subject: &str,
        main_score: u8,
        flags: &[&str],
        total: f64,
        pack_sha256: Option<&str>,
    ) {
        let packs =
            crate::rubric_pack::build(dir, crate::rubric_pack::PackKind::AdrVsSpine, subject)
                .expect("досье");
        let sha = packs[0].sha256.clone();
        let artifact = serde_json::json!({
            "schema": crate::rubric::RUBRIC_REPORT_SCHEMA,
            "rubric": "adr_spine_consistency",
            "judge_model": "judge-x",
            "author_model": "author-y",
            "weighted_total": total,
            "verdict": "CONCERNS",
            "pack_kind": "adr_vs_spine",
            "subject": subject,
            "pack_sha256": pack_sha256.unwrap_or(sha.as_str()),
            "inputs": packs[0].inputs.iter().map(|i| serde_json::json!({
                "path": i.path, "sha256": i.sha256, "role": i.role.as_str(), "id": i.id,
            })).collect::<Vec<_>>(),
            "scores": [{
                "criterion_id": "no_contradiction",
                "weight": 3.0,
                "score": main_score,
                "rationale": "Цитата subject: \"Решение\". Цитата reference: \"Rule\".",
                "samples": [main_score],
                "stdev": 0.0,
                "flags": flags,
                "evidence_unconfirmed_ratio": 0.0,
                "checked": ["AD-1"],
            }],
            "judged_at": "2026-09-20T10:00:00+03:00",
        });
        let reports = dir.join(crate::rubric::RUBRIC_REPORTS_DIR);
        std::fs::create_dir_all(&reports).expect("mkdir reports");
        std::fs::write(
            reports.join("semantic.json"),
            serde_json::to_string_pretty(&artifact).expect("json"),
        )
        .expect("write report");
    }

    /// Прогон гейта с настройками смысловой составляющей.
    fn run_semantic(
        dir: &Path,
        base: Option<&str>,
        cfg: crate::config::SemanticQualityConfig,
        rubrics_dir: &Path,
    ) -> GateReport {
        let options = GateOptions {
            semantic_quality: cfg,
            rubrics_dir: rubrics_dir.to_path_buf(),
            ..GateOptions::default()
        };
        run_inner(
            dir,
            Some(Route::Fast),
            base,
            None,
            (1, 4),
            &with_semantic(Route::Fast),
            &options,
        )
        .expect("гейт")
    }

    /// Находки составляющей по коду правила.
    fn semantic_rules(report: &GateReport) -> Vec<String> {
        report
            .components
            .iter()
            .find(|c| c.name == "semantic_quality")
            .map(|c| c.findings.iter().filter_map(|f| f.rule.clone()).collect())
            .unwrap_or_default()
    }

    /// По умолчанию составляющая — SKIP: включение только через `[gate.required]`.
    #[test]
    fn semantic_quality_is_skip_by_default() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path();
        make_semantic_repo(dir);
        let report = run_with(
            dir,
            Some(Route::Fast),
            None,
            None,
            (1, 4),
            &GateRequirements::default(),
        )
        .expect("гейт");
        assert_eq!(status_of(&report, "semantic_quality"), GateStatus::Skip);
        assert_eq!(report.outcome, GateOutcome::Pass);
    }

    /// Включённая составляющая без отчёта — error: смысловая рубрика обязана
    /// иметь отчёт, иначе «зелёный» означал бы «не смотрели».
    #[test]
    fn semantic_quality_requires_report_when_enabled() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path();
        make_semantic_repo(dir);
        let rubrics = semantic_rubrics_dir(dir);
        let report = run_semantic(
            dir,
            None,
            semantic_cfg(crate::config::SemanticScope::All),
            &rubrics,
        );
        assert_eq!(status_of(&report, "semantic_quality"), GateStatus::Fail);
        assert!(
            semantic_rules(&report).contains(&"semantic_report_missing".to_string()),
            "{:?}",
            semantic_rules(&report)
        );
    }

    /// Совпавший хэш досье — отчёт переиспользуется: находок нет.
    #[test]
    fn semantic_quality_reuses_report_with_same_pack_hash() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path();
        make_semantic_repo(dir);
        let rubrics = semantic_rubrics_dir(dir);
        write_semantic_report(dir, "docs/adr/ADR-001-reshenie.md", 5, &[], 4.5, None);
        let report = run_semantic(
            dir,
            None,
            semantic_cfg(crate::config::SemanticScope::All),
            &rubrics,
        );
        assert_eq!(status_of(&report, "semantic_quality"), GateStatus::Pass);
        assert!(
            semantic_rules(&report).is_empty(),
            "{:?}",
            semantic_rules(&report)
        );
    }

    /// Правка спайна обесценивает отчёт о решении: в сообщении назван
    /// изменившийся источник, а не только субъект.
    #[test]
    fn semantic_quality_stale_when_spine_changes() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path();
        make_semantic_repo(dir);
        let rubrics = semantic_rubrics_dir(dir);
        write_semantic_report(dir, "docs/adr/ADR-001-reshenie.md", 5, &[], 4.5, None);
        // Меняем СПАЙН, а не ADR.
        std::fs::write(
            dir.join("ARCHITECTURE-SPINE.md"),
            "# Spine\n\n## AD-1: Журнал только дописывается\n\n\
             - **Binds**: журнал операций ↔ код записи\n\
             - **Prevents**: правку и удаление строк журнала\n\
             - **Rule**: строки журнала не правятся ничем.\n",
        )
        .expect("spine");
        let report = run_semantic(
            dir,
            None,
            semantic_cfg(crate::config::SemanticScope::All),
            &rubrics,
        );
        assert_eq!(status_of(&report, "semantic_quality"), GateStatus::Fail);
        assert!(
            semantic_rules(&report).contains(&"semantic_report_stale".to_string()),
            "{:?}",
            semantic_rules(&report)
        );
        let detail = report
            .components
            .iter()
            .find(|c| c.name == "semantic_quality")
            .map(|c| {
                c.findings
                    .iter()
                    .map(|f| f.message.clone())
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .unwrap_or_default();
        assert!(
            detail.contains("ARCHITECTURE-SPINE.md"),
            "назван изменившийся источник: {detail}"
        );
    }

    /// Подтверждённое обвинение по главному критерию — блокирующая находка.
    #[test]
    fn semantic_quality_blocks_on_confirmed_contradiction() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path();
        make_semantic_repo(dir);
        let rubrics = semantic_rubrics_dir(dir);
        write_semantic_report(dir, "docs/adr/ADR-001-reshenie.md", 1, &[], 2.0, None);
        let report = run_semantic(
            dir,
            None,
            semantic_cfg(crate::config::SemanticScope::All),
            &rubrics,
        );
        assert_eq!(status_of(&report, "semantic_quality"), GateStatus::Fail);
        let rules = semantic_rules(&report);
        assert!(
            rules.contains(&"semantic_contradiction".to_string()),
            "{rules:?}"
        );
        // Состояние субъекта одно и называет самое важное: обвинение. Порог
        // итога отдельной строкой не дублируется — «противоречие найдено»
        // говорит больше, чем «итог ниже порога».
        assert_eq!(rules.len(), 1, "{rules:?}");
    }

    /// Обвинение без подтверждённых цитат — warn: гейт этим не краснеет
    /// (решение ADR-051: выдуманное обвинение наказывает судью, а не документ).
    #[test]
    fn semantic_quality_unconfirmed_accusation_is_warn() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path();
        make_semantic_repo(dir);
        let rubrics = semantic_rubrics_dir(dir);
        write_semantic_report(
            dir,
            "docs/adr/ADR-001-reshenie.md",
            1,
            &["accusation_unconfirmed"],
            4.0,
            None,
        );
        let report = run_semantic(
            dir,
            None,
            semantic_cfg(crate::config::SemanticScope::All),
            &rubrics,
        );
        assert_eq!(
            status_of(&report, "semantic_quality"),
            GateStatus::Pass,
            "warn не краснит составляющую"
        );
        assert!(
            semantic_rules(&report).contains(&"semantic_accusation_unconfirmed".to_string()),
            "{:?}",
            semantic_rules(&report)
        );
    }

    /// Неподтверждённое обвинение не должно вытеснять подтверждённый низкий
    /// итог: живой прогон D11 (2026-09-20) дал главному критерию 1 с меткой
    /// `accusation_unconfirmed` и взвешенный итог 1.00/5 — при старом порядке
    /// гейт показал бы только «обвинение не подтверждено» и промолчал бы про
    /// итог. Итог сообщается отдельной находкой, но не краснит составляющую:
    /// красный — только за обвинение по главному критерию с цитатами.
    #[test]
    fn semantic_quality_low_total_survives_unconfirmed_accusation() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path();
        make_semantic_repo(dir);
        let rubrics = semantic_rubrics_dir(dir);
        write_semantic_report(
            dir,
            "docs/adr/ADR-001-reshenie.md",
            1,
            &["accusation_unconfirmed"],
            1.0,
            None,
        );
        let report = run_semantic(
            dir,
            None,
            semantic_cfg(crate::config::SemanticScope::All),
            &rubrics,
        );
        assert_eq!(
            status_of(&report, "semantic_quality"),
            GateStatus::Pass,
            "низкий итог — предупреждение, краснит только обвинение по главному критерию"
        );
        let rules = semantic_rules(&report);
        assert!(
            rules.contains(&"semantic_quality_low".to_string()),
            "{rules:?}"
        );
        assert_eq!(
            rules.len(),
            1,
            "состояние одно и называет самое сильное: {rules:?}"
        );
    }

    /// Область `changed`: субъектом становится только тот, чьё досье затронуто
    /// диффом, — иначе поток доработок требовал бы отчётов обо всём пакете.
    #[test]
    fn semantic_quality_scope_changed_limits_subjects() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path();
        make_semantic_repo(dir);
        let rubrics = semantic_rubrics_dir(dir);
        // Второй Accepted-ADR: он не менялся и в область `changed` не попадает.
        std::fs::write(
            dir.join("docs/adr/ADR-002-vtoroe.md"),
            "# ADR-002. Второе\n\n- Date: 2026-09-19\n- Status: Accepted\n\n## Context\n\nДругое.\n",
        )
        .expect("adr2");
        git(dir, &["add", "."]);
        git(dir, &["commit", "-q", "--allow-empty", "-m", "adr2"]);
        // Меняем только первый ADR после коммита.
        std::fs::write(
            dir.join("docs/adr/ADR-001-reshenie.md"),
            "# ADR-001. Решение\n\n- Date: 2026-09-20\n- Status: Accepted\n\n## Context\n\nПричина и следствие.\n",
        )
        .expect("adr1");
        write_semantic_report(dir, "docs/adr/ADR-001-reshenie.md", 5, &[], 4.5, None);

        let report = run_semantic(
            dir,
            Some("HEAD"),
            semantic_cfg(crate::config::SemanticScope::Changed),
            &rubrics,
        );
        assert_eq!(
            status_of(&report, "semantic_quality"),
            GateStatus::Pass,
            "отчёт есть у изменённого субъекта, второй в область не входит: {:?}",
            semantic_rules(&report)
        );
        // А в области `all` второй субъект отчёта не имеет — error.
        let all = run_semantic(
            dir,
            Some("HEAD"),
            semantic_cfg(crate::config::SemanticScope::All),
            &rubrics,
        );
        assert_eq!(status_of(&all, "semantic_quality"), GateStatus::Fail);
        let detail = all
            .components
            .iter()
            .find(|c| c.name == "semantic_quality")
            .map(|c| {
                c.findings
                    .iter()
                    .map(|f| f.message.clone())
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .unwrap_or_default();
        assert!(
            detail.contains("ADR-002-vtoroe.md") && !detail.contains("ADR-001-reshenie.md"),
            "область all называет второго, а не оценённого: {detail}"
        );
    }

    /// Рубрика из конфига, которой нет в каталоге, — явная ошибка настройки,
    /// а не молчаливый SKIP.
    #[test]
    fn semantic_quality_unknown_rubric_is_error() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path();
        make_semantic_repo(dir);
        let rubrics = semantic_rubrics_dir(dir);
        let mut cfg = semantic_cfg(crate::config::SemanticScope::All);
        cfg.rubrics = vec!["net-takoy-rubriki".to_string()];
        let report = run_semantic(dir, None, cfg, &rubrics);
        assert_eq!(status_of(&report, "semantic_quality"), GateStatus::Fail);
        assert!(
            semantic_rules(&report).contains(&"semantic_rubric_unknown".to_string()),
            "{:?}",
            semantic_rules(&report)
        );
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

    // --- Н3: аттестация различает состояния репозитория (ADR-043) ----------

    /// Два РАЗНЫХ FAIL одной составляющей дают разные аттестации.
    #[test]
    fn attestation_differs_for_different_findings() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path();
        make_gate_repo(dir);
        let limits = (1, 4);
        // Два разных нарушения одного и того же правила `no_pan`.
        std::fs::create_dir_all(dir.join("a")).expect("mkdir");
        std::fs::write(dir.join("a/one.py"), "PAN").expect("py");
        git(dir, &["add", "."]);
        git(dir, &["commit", "-q", "-m", "a"]);
        let first = run_with(
            dir,
            Some(Route::Fast),
            None,
            None,
            limits,
            &GateRequirements::default(),
        )
        .expect("gate");
        assert!(!first.passed, "{:?}", first.not_checked);
        std::fs::create_dir_all(dir.join("b")).expect("mkdir");
        std::fs::write(dir.join("b/two.py"), "PAN").expect("py");
        git(dir, &["add", "."]);
        git(dir, &["commit", "-q", "-m", "b"]);
        let second = run_with(
            dir,
            Some(Route::Fast),
            None,
            None,
            limits,
            &GateRequirements::default(),
        )
        .expect("gate");
        assert!(!second.passed);
        // Оба — FAIL составляющей fitness, но находки разные.
        assert_eq!(status_of(&first, "fitness"), GateStatus::Fail);
        assert_eq!(status_of(&second, "fitness"), GateStatus::Fail);
        assert_ne!(
            first.attestation, second.attestation,
            "разные дефекты обязаны давать разные аттестации"
        );
        let d1 = first.envelope_json()["components"]
            .as_array()
            .expect("components")
            .iter()
            .find(|c| c["name"] == "fitness")
            .expect("fitness")["findings_digest"]
            .clone();
        let d2 = second.envelope_json()["components"]
            .as_array()
            .expect("components")
            .iter()
            .find(|c| c["name"] == "fitness")
            .expect("fitness")["findings_digest"]
            .clone();
        assert_ne!(d1, d2, "свёртки находок обязаны различаться");
    }

    /// Правка модели меняет аттестацию — раньше входом был только реестр.
    #[test]
    fn attestation_changes_when_model_changes() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path();
        make_gate_repo(dir);
        write_model(dir, &[("CMP-001", "depends_on: []")]);
        git(dir, &["add", "."]);
        git(dir, &["commit", "-q", "-m", "model"]);
        let limits = (1, 4);
        let before = run_with(
            dir,
            Some(Route::Fast),
            None,
            None,
            limits,
            &GateRequirements::default(),
        )
        .expect("gate");
        // Безвредная правка ТЕКСТА сущности: вердикт тот же, аттестация — нет.
        // Правка коммитится, иначе её поймает delta_guard и сменит не вход, а
        // статус составляющей — сравнение было бы не о том.
        {
            use std::io::Write as _;
            let f = std::fs::OpenOptions::new()
                .append(true)
                .open(dir.join("model/CMP-001.md"))
                .expect("open");
            let mut f = f;
            f.write_all("\n\nУточнение формулировки без смены решения.\n".as_bytes())
                .expect("append");
        }
        git(dir, &["add", "."]);
        git(dir, &["commit", "-q", "-m", "текстовая правка"]);
        let after = run_with(
            dir,
            Some(Route::Fast),
            None,
            None,
            limits,
            &GateRequirements::default(),
        )
        .expect("gate");
        assert_eq!(before.outcome, after.outcome);
        assert_ne!(
            before.attestation, after.attestation,
            "аттестация обязана следовать за состоянием model/"
        );
    }

    /// Аттестация не зависит от того, как записан путь и где лежит репозиторий.
    #[test]
    fn attestation_stable_across_path_spelling() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path();
        make_gate_repo(dir);
        let limits = (1, 4);
        let abs = run_with(
            &dir.canonicalize().expect("canonicalize"),
            Some(Route::Fast),
            None,
            None,
            limits,
            &GateRequirements::default(),
        )
        .expect("gate");
        let trailing = PathBuf::from(format!("{}/", dir.display()));
        let rel = run_with(
            &trailing,
            Some(Route::Fast),
            None,
            None,
            limits,
            &GateRequirements::default(),
        )
        .expect("gate");
        assert_eq!(abs.attestation, rel.attestation);
        assert_eq!(abs.inputs_map(), rel.inputs_map());
    }

    /// `--verify-envelope`: тот же вход — «относится», правка — «изменилось».
    #[test]
    fn verify_envelope_detects_drift() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path();
        make_gate_repo(dir);
        write_model(dir, &[("CMP-001", "depends_on: []")]);
        git(dir, &["add", "."]);
        git(dir, &["commit", "-q", "-m", "model"]);
        let limits = (1, 4);
        let report = run_with(
            dir,
            Some(Route::Fast),
            None,
            None,
            limits,
            &GateRequirements::default(),
        )
        .expect("gate");
        let envelope = dir.join("verdict.json");
        std::fs::write(
            &envelope,
            serde_json::to_string_pretty(&report.envelope_json()).expect("json"),
        )
        .expect("write");
        let same = verify_envelope(dir, &envelope, limits).expect("verify");
        assert!(same.same, "{:?} {:?}", same.changed, same.missing);
        // Правка спайна — состояние разошлось, вход назван.
        std::fs::write(dir.join("ARCHITECTURE-SPINE.md"), "# Spine\n\nAD-1 …\n").expect("spine");
        let drifted = verify_envelope(dir, &envelope, limits).expect("verify");
        assert!(!drifted.same);
        assert!(
            drifted.changed.iter().any(|(name, _, _)| name == "spine"),
            "{:?}",
            drifted.changed
        );
        // Не конверт — честная ошибка оператора, а не «всё совпало».
        std::fs::write(&envelope, "{\"hello\":1}").expect("write");
        assert!(verify_envelope(dir, &envelope, limits).is_err());
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

    /// Пишет минимальную модель: `(id, хвост frontmatter)`.
    fn write_model(dir: &Path, entities: &[(&str, &str)]) {
        let model = dir.join("model");
        std::fs::create_dir_all(&model).expect("mkdir model");
        for (id, body) in entities {
            // Тип выводится из префикса ID — иначе `id-type-mismatch`.
            let kind = id.split('-').next().unwrap_or("cmp").to_ascii_lowercase();
            std::fs::write(
                model.join(format!("{id}.md")),
                format!(
                    "---\nid: {id}\ntype: {kind}\ntitle: \"{id}\"\nstatus: \"designed\"\n{body}\n---\n\n# {id}\n"
                ),
            )
            .expect("write entity");
        }
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
        // Fail-soft на инфраструктуру сохранён (FAIL-составляющих нет), но
        // П1: обязательные для fail-safe Critical составляющие без входа →
        // честный INCOMPLETE, а не зелёный PASS.
        assert!(!report.passed, "{}", render(&report));
        assert_eq!(
            report.outcome,
            GateOutcome::Incomplete,
            "{}",
            render(&report)
        );
        assert!(
            report
                .components
                .iter()
                .all(|c| c.status != GateStatus::Fail),
            "fail-soft: ни одна составляющая не провалена"
        );
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

    /// Паспорт (JSON `inputs`) называет ПУТЬ прочитанного реестра и число
    /// правил: хэш отвечает «тот же файл или нет», но не «какой именно файл».
    #[test]
    fn inputs_name_the_registry_and_rule_count() {
        let tmp = tempfile::tempdir().expect("tmp");
        let repo = tmp.path().join("repo");
        make_gate_repo(&repo);
        let report = run(&repo, None, None, None, (1, 4)).expect("гейт");
        let inputs: std::collections::BTreeMap<String, String> =
            report.inputs.iter().cloned().collect();
        assert_eq!(
            inputs.get("constraints_path").map(String::as_str),
            Some(".arch-handoff/CONSTRAINTS.yaml")
        );
        assert_eq!(
            inputs.get("constraints_rules").map(String::as_str),
            Some("2")
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

    /// П4: `ROUTE.lock` поднимает маршрут на чистом дереве до заявленного,
    /// критические составляющие реально прогоняются.
    #[test]
    fn route_lock_raises_clean_tree_to_critical() {
        let tmp = tempfile::tempdir().expect("tmp");
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).expect("mkdir");
        make_gate_repo(&repo);
        std::fs::write(
            repo.join(".arch-handoff/ROUTE.lock"),
            "route: critical\nreason: \"обработка ЦР, 10/15 триггеров\"\ndecided_by: ADR-012\n",
        )
        .expect("route lock");
        let report = run(&repo, None, None, None, (1, 4)).expect("гейт");
        assert_eq!(report.route, Route::Critical, "{}", render(&report));
        assert!(
            report.components.iter().any(|c| c.name == "nfr"),
            "критические составляющие обязаны попасть в прогон"
        );
        assert!(
            report.route_note.contains("поднят ROUTE.lock"),
            "{}",
            report.route_note
        );
    }

    /// П4: понижение заявленного маршрута без ADR — находка `route_lowered`.
    #[test]
    fn route_lock_lowering_without_adr_fails() {
        let tmp = tempfile::tempdir().expect("tmp");
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).expect("mkdir");
        make_gate_repo(&repo);
        std::fs::write(
            repo.join(".arch-handoff/ROUTE.lock"),
            "route: critical\ndecided_by: ADR-012\n",
        )
        .expect("lock");
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-q", "-m", "route lock"]);
        // Понижаем до fast без ссылки на ADR.
        std::fs::write(repo.join(".arch-handoff/ROUTE.lock"), "route: fast\n").expect("lowered");
        let report = run(&repo, None, None, None, (1, 4)).expect("гейт");
        assert_eq!(
            status_of(&report, "route_lock"),
            GateStatus::Fail,
            "{}",
            render(&report)
        );
        assert_eq!(report.outcome, GateOutcome::Fail);
    }

    /// П7: конверт вердикта стабилен (идемпотентность) и честно называет
    /// непроверенное; exit-код INCOMPLETE — 3.
    #[test]
    fn envelope_attestation_is_stable_and_lists_not_checked() {
        let tmp = tempfile::tempdir().expect("tmp");
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).expect("mkdir");
        make_gate_repo(&repo);
        let first = run(&repo, Some(Route::Standard), None, None, (1, 4)).expect("гейт");
        let second = run(&repo, Some(Route::Standard), None, None, (1, 4)).expect("гейт");
        let a = first.envelope_json();
        let b = second.envelope_json();
        assert_eq!(
            a["attestation"], b["attestation"],
            "тот же вход — та же аттестация"
        );
        assert_eq!(a["verdict"], "INCOMPLETE");
        assert_eq!(a["exit_code"], 3);
        let not_checked = a["not_checked"].as_array().expect("not_checked");
        assert!(
            not_checked.iter().any(|v| v == "trace_check" || v == "nfr"),
            "{a}"
        );
    }
}
