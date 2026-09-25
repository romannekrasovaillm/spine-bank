//! Типы вердикта гейта (B1): статусы, находки, составляющие, матрица
//! обязательности, настройки и отчёт с аттестацией (П7, ADR-043).

use std::collections::BTreeMap;
use std::fmt::{self, Write as _};
use std::path::PathBuf;

use crate::control::{self, Route};

/// Потолок строк находок, печатаемых под FAIL-составляющей (остаток —
/// счётчиком): вывод гейта читают люди и агенты в хуках, простыня находок
/// там не нужна — полный список дают команды составляющих.
pub(super) const MAX_COMPONENT_FINDINGS: usize = 20;

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
    pub(super) fn lint(i: &control::LintIssue) -> Self {
        Self {
            severity: i.severity.clone(),
            rule: Some(i.rule.clone()),
            file: Some(i.file.display().to_string()),
            line: Some(i.line),
            message: i.message.clone(),
        }
    }

    /// Адресная находка без строки (`delta_guard`): `file — message`.
    pub(super) fn file_only(severity: &str, file: String, message: String) -> Self {
        Self {
            severity: severity.to_string(),
            rule: None,
            file: Some(file),
            line: None,
            message,
        }
    }

    /// Находка с правилом без адреса (`trace_check`, nfr): `[severity] rule — message`.
    pub(super) fn ruled(severity: String, rule: String, message: String) -> Self {
        Self {
            severity,
            rule: Some(rule),
            file: None,
            line: None,
            message,
        }
    }

    /// Свободный текст (`evidence_verify`): печатается как есть.
    pub(super) fn text(severity: &str, message: String) -> Self {
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
    pub(super) fn pass(name: &'static str, detail: String) -> Self {
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
    pub(super) fn pass_with_findings(
        name: &'static str,
        detail: String,
        findings: Vec<GateFinding>,
    ) -> Self {
        Self {
            name,
            status: GateStatus::Pass,
            detail,
            findings,
            not_verified: Vec::new(),
        }
    }

    /// Составляющая провалена (находки/сбой) — гейт падает.
    pub(super) fn fail(name: &'static str, detail: String, findings: Vec<GateFinding>) -> Self {
        Self {
            name,
            status: GateStatus::Fail,
            detail,
            findings,
            not_verified: Vec::new(),
        }
    }

    /// Составляющая пропущена fail-soft (нет входа).
    pub(super) fn skip(name: &'static str, detail: String) -> Self {
        Self {
            name,
            status: GateStatus::Skip,
            detail,
            findings: Vec::new(),
            not_verified: Vec::new(),
        }
    }

    /// Составляющая пропущена, но не «нечего проверять», а «проверить нельзя»:
    /// вход требует человека (E2 — prompt-инъекция в досье). Находка остаётся
    /// в отчёте, статус SKIP делает вердикт INCOMPLETE: пропуск обязательной
    /// составляющей не зеленеет (П1), и релиз не выдаётся молча.
    pub(super) fn skip_with_findings(
        name: &'static str,
        detail: String,
        findings: Vec<GateFinding>,
    ) -> Self {
        Self {
            name,
            status: GateStatus::Skip,
            detail,
            findings,
            not_verified: Vec::new(),
        }
    }

    /// Дополняет составляющую границей её вердикта (W1, блок 2 паспорта).
    pub(super) fn noting(mut self, notes: Vec<String>) -> Self {
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
    /// Модель доверия `command_succeeds` (A3, ADR-053): снимок решения
    /// «исполнять ли команды реестра» для составляющей `fitness`. `Default` —
    /// детерминированный legacy-режим (исполнять, allow-файл не
    /// консультируется): библиотека без края не зависит от машины (AD-7).
    pub exec: crate::cmd_trust::ExecPolicy,
    /// Политика решения `human` по маршрутам (E4.2): от неё зависит, блокирует
    /// ли гейт оговорку судьи или предупреждает.
    pub decision_policy: crate::config::DecisionPolicyConfig,
    /// Эффективный маршрут прогона — после храповика `ROUTE.lock` (E3.3):
    /// от него зависит политика оговорок отчёта (`evidence_partial` на
    /// Critical уходит человеку). `None` — библиотечный вызов без маршрута:
    /// поведение как на Fast (warn).
    pub route: Option<Route>,
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
            decision_policy: cfg.gate.decision_policy.clone(),
            exec: crate::cmd_trust::ExecPolicy::default(),
            route: None,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::Route;

    fn component(
        name: &'static str,
        status: GateStatus,
        findings: Vec<GateFinding>,
    ) -> GateComponent {
        GateComponent {
            name,
            status,
            detail: format!("детали {name}"),
            findings,
            not_verified: Vec::new(),
        }
    }

    fn report(required: Vec<String>, inputs: Vec<(String, String)>) -> GateReport {
        GateReport {
            repo: std::path::PathBuf::from("."),
            route: Route::Fast,
            route_auto: false,
            route_note: "auto".to_string(),
            components: vec![
                component("fitness", GateStatus::Pass, Vec::new()),
                component("delta_guard", GateStatus::Pass, Vec::new()),
            ],
            outcome: GateOutcome::Pass,
            required,
            not_checked: Vec::new(),
            inputs,
            attestation: String::new(),
            passed: true,
        }
    }

    /// `inputs_map` отдаёт ровно те пары «вход → значение», что записаны в
    /// вердикте: сверка конверта (`--verify-envelope`) сравнивает их поимённо.
    #[test]
    fn inputs_map_pairs_names_with_values() {
        let map = report(
            Vec::new(),
            vec![
                ("base".to_string(), "sha256:aaa".to_string()),
                ("constraints".to_string(), "absent".to_string()),
            ],
        )
        .inputs_map();
        assert_eq!(map.len(), 2, "{map:?}");
        assert_eq!(map.get("base").map(String::as_str), Some("sha256:aaa"));
        assert_eq!(map.get("constraints").map(String::as_str), Some("absent"));
    }

    /// Вердикт с одной составляющей: нужен, чтобы флаг `required` был виден
    /// и в вырожденном случае — «обязательна» против «не обязательна».
    fn single_report(required: Vec<String>) -> GateReport {
        let mut r = report(required, Vec::new());
        r.components = vec![component("fitness", GateStatus::Pass, Vec::new())];
        r
    }

    /// Аттестация различает состав обязательных составляющих: свёртка «ничего
    /// не обязательно» и «обязательно всё» не имеет права совпасть, иначе
    /// конверт с другим маршрутом выглядел бы тем же вердиктом.
    #[test]
    fn attestation_distinguishes_required_composition() {
        // Вырожденный случай: одна составляющая, флаг «обязательна» меняется.
        assert_ne!(
            single_report(vec!["fitness".to_string()]).compute_attestation(),
            single_report(Vec::new()).compute_attestation(),
            "единственная обязательная составляющая меняет аттестацию"
        );
        let none_required = report(Vec::new(), Vec::new()).compute_attestation();
        let all_required = report(
            vec!["fitness".to_string(), "delta_guard".to_string()],
            Vec::new(),
        )
        .compute_attestation();
        assert_ne!(
            none_required, all_required,
            "разный состав обязательных — разная аттестация"
        );
        // Тот же вход и тот же состав — та же аттестация (идемпотентность).
        assert_eq!(
            none_required,
            report(Vec::new(), Vec::new()).compute_attestation()
        );
        // Входы входят в свёртку: другой реестр — другая аттестация.
        assert_ne!(
            none_required,
            report(
                Vec::new(),
                vec![("constraints".to_string(), "sha256:aaa".to_string())],
            )
            .compute_attestation()
        );
    }
}
