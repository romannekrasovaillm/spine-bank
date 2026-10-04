//! Количественные NFR поверх типизированной модели (ADR-007).
//!
//! КОНТРАКТ (владелец: агент `nfr`):
//! - четыре проверки над `<case>/model/`: [`budget_check`] — сумма
//!   latency-бюджетов hop'ов `INT-*` против цели p99; [`availability_check`] —
//!   композиция доступности последовательных (`A = ∏Aᵢ`) и параллельных
//!   (`1 − (1−A)ⁿ`) участков против SLA плюс цели RTO/RPO;
//!   [`capacity_check`] — RPS-цель против ёмкости компонентов;
//!   [`cost_check`] — TCO (инстансы × тариф) и цена выхода (Σ `exit_cost`);
//! - находка `error` → exit code 1 (как `control check` / `trace check`),
//!   `warn` не ломает прогон;
//! - все числа — данные frontmatter сущностей (тарифы/ёмкости в коде не
//!   зашиты); проверки полностью детерминированы, LLM не используется;
//! - цепочка для `budget`/`availability`/`capacity` — `affects` NFR-сущности
//!   (порядок hop'ов — порядок сущностей модели, он детерминирован).

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::error::{HarnessError, Result};
use crate::llm::ToolSpec;
use crate::model::{Entity, EntityKind, Model, Severity, load_issues_note, load_model_tolerant};
use crate::tool::{Tool, ToolContext, ToolOutput};

/// Максимум реплик параллельного участка: бóльшие значения не меняют
/// доступность в f64 ((1−A)ⁿ сливается в 0), но защищают расчёт от
/// мусорных данных.
const MAX_REPLICAS: u32 = 1024;

/// Находка количественной проверки NFR.
#[derive(Debug)]
pub struct NfrIssue {
    /// Критичность.
    pub severity: Severity,
    /// Код правила (`budget-exceeded`, `capacity-insufficient`, …).
    pub rule: &'static str,
    /// Сообщение.
    pub message: String,
}

/// Добавляет находку в отчёт.
fn issue(issues: &mut Vec<NfrIssue>, severity: Severity, rule: &'static str, message: String) {
    issues.push(NfrIssue {
        severity,
        rule,
        message,
    });
}

/// Есть ли блокирующие находки (exit code 1).
fn any_errors(issues: &[NfrIssue]) -> bool {
    issues.iter().any(|i| i.severity == Severity::Error)
}

/// Загружает модель кейса (`<case>/model`) толерантно (E3): битые сущности
/// не роняют проверки — о них отчитывается [`push_load_skip_note`]; полный
/// отказ — только когда не загрузилось ничего.
fn load_case_model(case: &Path) -> Result<Model> {
    let dir = case.join("model");
    if !dir.is_dir() {
        return Err(HarnessError::Model(format!(
            "{}: нет каталога модели (ожидается {})",
            case.display(),
            dir.display()
        )));
    }
    load_model_tolerant(&dir)
}

/// Warn-находка о сущностях, пропущенных при толерантной загрузке (E3) —
/// одна агрегированная на отчёт.
fn push_load_skip_note(model: &Model, issues: &mut Vec<NfrIssue>) {
    if let Some(note) = load_issues_note(&model.load_issues) {
        issue(issues, Severity::Warn, "load-skip", note);
    }
}

/// Валидирует неотрицательное числовое поле. Невалидное значение — `error`
/// `bad-value` и `None`: мусорные данные не участвуют в расчёте и не дают
/// уверенный PASS.
fn checked(e: &Entity, field: &str, v: f64, issues: &mut Vec<NfrIssue>) -> Option<f64> {
    if v.is_finite() && v >= 0.0 {
        return Some(v);
    }
    issue(
        issues,
        Severity::Error,
        "bad-value",
        format!(
            "{}: поле `{field}` = {v} — ожидается конечное неотрицательное число",
            e.id
        ),
    );
    None
}

/// Валидирует положительное числовое поле (делитель расчёта).
fn checked_positive(e: &Entity, field: &str, v: f64, issues: &mut Vec<NfrIssue>) -> Option<f64> {
    if v.is_finite() && v > 0.0 {
        return Some(v);
    }
    issue(
        issues,
        Severity::Error,
        "bad-value",
        format!(
            "{}: поле `{field}` = {v} — ожидается конечное положительное число",
            e.id
        ),
    );
    None
}

/// Валидирует долю (0; 1]: доступность/SLA.
fn checked_fraction(e: &Entity, field: &str, v: f64, issues: &mut Vec<NfrIssue>) -> Option<f64> {
    if v.is_finite() && v > 0.0 && v <= 1.0 {
        return Some(v);
    }
    issue(
        issues,
        Severity::Error,
        "bad-value",
        format!(
            "{}: поле `{field}` = {v} — ожидается доля в диапазоне (0; 1]",
            e.id
        ),
    );
    None
}

/// Рендерит список находок и строку итога (общий хвост отчётов).
fn render_issues(out: &mut String, issues: &[NfrIssue], scope: &str) {
    if !issues.is_empty() {
        let _ = writeln!(out, "\n## Находки\n"); // игнорируется: записи в String не падают
        for i in issues {
            let _ = writeln!(out, "- [{}] {}: {}", i.severity, i.rule, i.message);
        }
    }
    let errors = issues
        .iter()
        .filter(|i| i.severity == Severity::Error)
        .count();
    let warns = issues.len() - errors;
    let _ = writeln!(
        out,
        "\nИтог: {} ({scope}, error: {errors}, warn: {warns})",
        if errors > 0 { "FAIL" } else { "PASS" }
    );
}

// ---------------------------------------------------------------------------
// Latency-бюджет
// ---------------------------------------------------------------------------

/// Hop цепочки latency-бюджета.
#[derive(Debug)]
pub struct BudgetHop {
    /// ID интерфейса (`INT-*`).
    pub id: String,
    /// Название.
    pub title: String,
    /// Заявленный бюджет, мс (`None` — не заявлен или невалиден).
    pub budget_ms: Option<f64>,
}

/// Разложение одной цели p99.
#[derive(Debug)]
pub struct BudgetTarget {
    /// ID NFR с целью.
    pub nfr_id: String,
    /// Цель p99, мс.
    pub target_ms: f64,
    /// Hop'и цепочки (`INT-*` из `affects`, порядок модели).
    pub hops: Vec<BudgetHop>,
    /// Сумма заявленных бюджетов, мс.
    pub sum_ms: f64,
}

/// Отчёт `arch-be nfr budget`.
#[derive(Debug)]
pub struct BudgetReport {
    /// Корень кейса.
    pub case: PathBuf,
    /// Разобранные цели p99.
    pub targets: Vec<BudgetTarget>,
    /// `INT-*` с бюджетом вне проверяемых цепочек (warn).
    pub uncovered_hops: Vec<String>,
    /// Находки.
    pub issues: Vec<NfrIssue>,
}

impl BudgetReport {
    /// Есть ли блокирующие находки (exit code 1).
    #[must_use]
    pub fn has_errors(&self) -> bool {
        any_errors(&self.issues)
    }

    /// Markdown-отчёт: таблица hop'ов по каждой цели, находки, итог.
    #[must_use]
    pub fn render(&self) -> String {
        let mut s = String::new();
        let _ = writeln!(s, "# Latency-бюджет: {}\n", self.case.display());
        for t in &self.targets {
            let _ = writeln!(s, "## {} — цель p99 {:.0} мс\n", t.nfr_id, t.target_ms);
            let _ = writeln!(s, "| Hop | Бюджет, мс |");
            let _ = writeln!(s, "|-----|-----------:|");
            for h in &t.hops {
                let budget = h
                    .budget_ms
                    .map_or_else(|| "—".into(), |b| format!("{b:.0}"));
                let _ = writeln!(s, "| {} {} | {} |", h.id, h.title, budget);
            }
            let reserve = t.target_ms - t.sum_ms;
            let status = if reserve >= 0.0 {
                "OK"
            } else {
                "ПРЕВЫШЕНИЕ"
            };
            let _ = writeln!(
                s,
                "\nСумма: {:.0} мс · резерв: {:.0} мс · {status}\n",
                t.sum_ms, reserve
            );
        }
        if self.targets.is_empty() {
            let _ = writeln!(s, "Целей p99 (`p99_target_ms` у NFR) не найдено.");
        }
        render_issues(
            &mut s,
            &self.issues,
            &format!("целей: {}", self.targets.len()),
        );
        s
    }
}

/// Разложение latency-бюджета: сумма hop'ов `INT-*` против цели p99.
///
/// Ошибки: hop без бюджета (`budget-hop-missing`), цель без цепочки
/// (`budget-no-chain`), сумма выше цели (`budget-exceeded`, с разложением
/// по виновным hop'ам), мусорное число (`bad-value`).
///
/// # Errors
/// Каталог `<case>/model` отсутствует или модель не разбирается.
pub fn budget_check(case: &Path) -> Result<BudgetReport> {
    let model = load_case_model(case)?;
    let mut issues = Vec::new();
    push_load_skip_note(&model, &mut issues);
    let mut targets = Vec::new();
    let mut covered: BTreeSet<&str> = BTreeSet::new();
    for nfr in model.entities.iter().filter(|e| e.kind == EntityKind::Nfr) {
        if let Some(t) = budget_target(&model, nfr, &mut covered, &mut issues) {
            targets.push(t);
        }
    }
    let mut uncovered_hops = Vec::new();
    for e in model
        .entities
        .iter()
        .filter(|e| e.kind == EntityKind::Int && e.latency_budget_ms.is_some())
    {
        if !covered.contains(e.id.as_str()) {
            uncovered_hops.push(e.id.clone());
            issue(
                &mut issues,
                Severity::Warn,
                "budget-hop-uncovered",
                format!(
                    "{}: бюджет заявлен, но ни одна цель p99 его не проверяет",
                    e.id
                ),
            );
        }
    }
    Ok(BudgetReport {
        case: case.to_path_buf(),
        targets,
        uncovered_hops,
        issues,
    })
}

/// Разложение одной цели p99 (`None` — цели у сущности нет или она
/// невалидна). Пополняет `covered` hop'ами, вошедшими в цепочки.
fn budget_target<'m>(
    model: &'m Model,
    nfr: &'m Entity,
    covered: &mut BTreeSet<&'m str>,
    issues: &mut Vec<NfrIssue>,
) -> Option<BudgetTarget> {
    let target = checked(nfr, "p99_target_ms", nfr.p99_target_ms?, issues)?;
    let mut hops = Vec::new();
    let mut sum = 0.0;
    for id in &nfr.affects {
        // Битые ссылки — территория `model validate`, здесь пропускаем.
        let Some(hop) = model.get(id) else {
            continue;
        };
        if hop.kind != EntityKind::Int {
            continue;
        }
        covered.insert(hop.id.as_str());
        let budget = hop
            .latency_budget_ms
            .and_then(|b| checked(hop, "latency_budget_ms", b, issues));
        if hop.latency_budget_ms.is_none() {
            issue(
                issues,
                Severity::Error,
                "budget-hop-missing",
                format!(
                    "{}: hop {} ({}) — нет заявленного бюджета (latency_budget_ms)",
                    nfr.id, hop.id, hop.title
                ),
            );
        }
        sum += budget.unwrap_or(0.0);
        hops.push(BudgetHop {
            id: hop.id.clone(),
            title: hop.title.clone(),
            budget_ms: budget,
        });
    }
    if hops.is_empty() {
        issue(
            issues,
            Severity::Error,
            "budget-no-chain",
            format!(
                "{}: заявлена цель p99 {target:.0} мс, но в affects нет ни одного INT-* — цепочка пуста",
                nfr.id
            ),
        );
    } else if sum > target {
        let guilty = hops
            .iter()
            .map(|h| {
                h.budget_ms
                    .map_or_else(|| format!("{}=—", h.id), |b| format!("{}={b:.0}", h.id))
            })
            .collect::<Vec<_>>()
            .join(", ");
        issue(
            issues,
            Severity::Error,
            "budget-exceeded",
            format!(
                "{}: сумма бюджетов hop'ов {sum:.0} мс превышает цель p99 {target:.0} мс \
                 (превышение {:.0} мс); разложение: {guilty}",
                nfr.id,
                sum - target
            ),
        );
    }
    Some(BudgetTarget {
        nfr_id: nfr.id.clone(),
        target_ms: target,
        hops,
        sum_ms: sum,
    })
}

// ---------------------------------------------------------------------------
// Доступность
// ---------------------------------------------------------------------------

/// Участок цепочки доступности.
#[derive(Debug)]
pub struct AvailabilitySegment {
    /// ID компонента/интерфейса.
    pub id: String,
    /// Название.
    pub title: String,
    /// Заявленная доступность участка (`None` — нет данных, warn).
    pub base: Option<f64>,
    /// Параллельные реплики (дефолт 1).
    pub replicas: u32,
    /// Эффективная доступность участка: `1 − (1−A)^replicas`.
    pub effective: Option<f64>,
}

/// Цепочка доступности одного SLA.
#[derive(Debug)]
pub struct AvailabilityChain {
    /// ID NFR с целью.
    pub nfr_id: String,
    /// Заявленный SLA (доля).
    pub target: f64,
    /// Участки (`CMP`/`INT` из `affects`, порядок модели).
    pub segments: Vec<AvailabilitySegment>,
    /// Скомпонованная доступность (∏ по участкам с данными).
    pub computed: f64,
    /// Все ли участки имеют данные.
    pub complete: bool,
}

/// Цель DR из NFR-сущности.
#[derive(Debug)]
pub struct DrTarget {
    /// ID NFR.
    pub nfr_id: String,
    /// RTO, минут.
    pub rto_minutes: Option<f64>,
    /// RPO, секунд.
    pub rpo_seconds: Option<f64>,
}

/// Отчёт `arch-be nfr availability`.
#[derive(Debug)]
pub struct AvailabilityReport {
    /// Корень кейса.
    pub case: PathBuf,
    /// Цепочки по SLA.
    pub chains: Vec<AvailabilityChain>,
    /// Цели RTO/RPO из NFR.
    pub dr: Vec<DrTarget>,
    /// Находки.
    pub issues: Vec<NfrIssue>,
}

impl AvailabilityReport {
    /// Есть ли блокирующие находки (exit code 1).
    #[must_use]
    pub fn has_errors(&self) -> bool {
        any_errors(&self.issues)
    }

    /// Markdown-отчёт: таблица участков по каждому SLA, цели DR, итог.
    #[must_use]
    pub fn render(&self) -> String {
        let mut s = String::new();
        let _ = writeln!(s, "# Доступность: {}\n", self.case.display());
        for c in &self.chains {
            let _ = writeln!(s, "## {} — SLA {:.4}%\n", c.nfr_id, c.target * 100.0);
            let _ = writeln!(s, "| Участок | A | Реплики | A эфф. |");
            let _ = writeln!(s, "|---------|--:|--------:|-------:|");
            for seg in &c.segments {
                let base = seg.base.map_or_else(|| "—".into(), |a| format!("{a}"));
                // 9 знаков: 0.999999999 в 6 знаках сливается в «1.000000».
                let eff = seg
                    .effective
                    .map_or_else(|| "—".into(), |a| format!("{a:.9}"));
                let _ = writeln!(
                    s,
                    "| {} {} | {} | {} | {} |",
                    seg.id, seg.title, base, seg.replicas, eff
                );
            }
            let downtime = (1.0 - c.computed) * 365.0 * 24.0 * 60.0;
            let allowed = (1.0 - c.target) * 365.0 * 24.0 * 60.0;
            let status = if c.computed >= c.target {
                "OK"
            } else {
                "НИЖЕ SLA"
            };
            let partial = if c.complete {
                ""
            } else {
                " (неполные данные)"
            };
            let _ = writeln!(
                s,
                "\nЦепочка: {:.4}% (простой ~{downtime:.1} мин/год) · SLA {:.4}% (~{allowed:.1} мин/год) · {status}{partial}\n",
                c.computed * 100.0,
                c.target * 100.0
            );
        }
        if self.chains.is_empty() {
            let _ = writeln!(
                s,
                "Целей доступности (`availability_target` у NFR) не найдено."
            );
        }
        if !self.dr.is_empty() {
            let _ = writeln!(s, "\n## Цели DR\n");
            let _ = writeln!(s, "| NFR | RTO, мин | RPO, с |");
            let _ = writeln!(s, "|-----|---------:|-------:|");
            for d in &self.dr {
                let rto = d
                    .rto_minutes
                    .map_or_else(|| "—".into(), |v| format!("{v:.0}"));
                let rpo = d
                    .rpo_seconds
                    .map_or_else(|| "—".into(), |v| format!("{v:.0}"));
                let _ = writeln!(s, "| {} | {} | {} |", d.nfr_id, rto, rpo);
            }
        }
        render_issues(&mut s, &self.issues, &format!("SLA: {}", self.chains.len()));
        s
    }
}

/// Расчёт доступности: последовательные участки перемножаются, участок с
/// `replicas = n` считается параллельным (`1 − (1−A)ⁿ`); сверка с SLA и
/// сбор целей RTO/RPO из NFR-сущностей.
///
/// # Errors
/// Каталог `<case>/model` отсутствует или модель не разбирается.
pub fn availability_check(case: &Path) -> Result<AvailabilityReport> {
    let model = load_case_model(case)?;
    let mut issues = Vec::new();
    push_load_skip_note(&model, &mut issues);
    let mut chains = Vec::new();
    for nfr in model.entities.iter().filter(|e| e.kind == EntityKind::Nfr) {
        if let Some(c) = availability_chain(&model, nfr, &mut issues) {
            chains.push(c);
        }
    }
    let dr = dr_targets(&model, &mut issues);
    if !chains.is_empty() && dr.is_empty() {
        issue(
            &mut issues,
            Severity::Warn,
            "availability-no-dr-targets",
            "заявлены SLA доступности, но в модели нет целей RTO/RPO (rto_minutes/rpo_seconds у NFR)".into(),
        );
    }
    Ok(AvailabilityReport {
        case: case.to_path_buf(),
        chains,
        dr,
        issues,
    })
}

/// Цепочка доступности одного SLA (`None` — цели у сущности нет или она
/// невалидна).
fn availability_chain(
    model: &Model,
    nfr: &Entity,
    issues: &mut Vec<NfrIssue>,
) -> Option<AvailabilityChain> {
    let target = checked_fraction(nfr, "availability_target", nfr.availability_target?, issues)?;
    let mut segments = Vec::new();
    let mut computed = 1.0;
    let mut complete = true;
    for id in &nfr.affects {
        // Битые ссылки — территория `model validate`, здесь пропускаем.
        let Some(seg) = model.get(id) else {
            continue;
        };
        if !matches!(seg.kind, EntityKind::Cmp | EntityKind::Int) {
            continue;
        }
        let base = seg
            .availability
            .and_then(|a| checked_fraction(seg, "availability", a, issues));
        if seg.availability.is_none() {
            issue(
                issues,
                Severity::Warn,
                "availability-no-data",
                format!(
                    "{}: участок {} ({}) без данных доступности (availability) — исключён из расчёта",
                    nfr.id, seg.id, seg.title
                ),
            );
        }
        let replicas = seg.replicas.unwrap_or(1).min(MAX_REPLICAS);
        let effective =
            base.map(|a| 1.0 - (1.0 - a).powi(i32::try_from(replicas).unwrap_or(i32::MAX)));
        if let Some(eff) = effective {
            computed *= eff;
        } else {
            complete = false;
        }
        segments.push(AvailabilitySegment {
            id: seg.id.clone(),
            title: seg.title.clone(),
            base,
            replicas,
            effective,
        });
    }
    if segments.is_empty() {
        issue(
            issues,
            Severity::Error,
            "availability-no-chain",
            format!(
                "{}: заявлен SLA {:.4}%, но в affects нет ни одного CMP-*/INT-* — цепочка пуста",
                nfr.id,
                target * 100.0
            ),
        );
    } else if computed < target {
        // Добавление недостающих участков только снизит произведение —
        // расхождение с SLA уже доказано.
        issue(
            issues,
            Severity::Error,
            "availability-below-target",
            format!(
                "{}: расчётная доступность {:.6} ниже SLA {:.6}",
                nfr.id, computed, target
            ),
        );
    }
    Some(AvailabilityChain {
        nfr_id: nfr.id.clone(),
        target,
        segments,
        computed,
        complete,
    })
}

/// Цели RTO/RPO из NFR-сущностей.
fn dr_targets(model: &Model, issues: &mut Vec<NfrIssue>) -> Vec<DrTarget> {
    let mut dr = Vec::new();
    for nfr in model.entities.iter().filter(|e| e.kind == EntityKind::Nfr) {
        if nfr.rto_minutes.is_none() && nfr.rpo_seconds.is_none() {
            continue;
        }
        let rto = nfr
            .rto_minutes
            .and_then(|v| checked(nfr, "rto_minutes", v, issues));
        let rpo = nfr
            .rpo_seconds
            .and_then(|v| checked(nfr, "rpo_seconds", v, issues));
        dr.push(DrTarget {
            nfr_id: nfr.id.clone(),
            rto_minutes: rto,
            rpo_seconds: rpo,
        });
    }
    dr
}

// ---------------------------------------------------------------------------
// Пропускная способность
// ---------------------------------------------------------------------------

/// Ёмкость компонента против RPS-цели.
#[derive(Debug)]
pub struct ComponentCapacity {
    /// ID компонента.
    pub id: String,
    /// Название.
    pub title: String,
    /// RPS на инстанс (`None` — нет данных, warn).
    pub rps_per_instance: Option<f64>,
    /// Заявлено инстансов (`None` — не заявлено, warn).
    pub instances: Option<u32>,
    /// Требуемое число инстансов под цель (`None` — нет данных).
    pub required_instances: Option<u64>,
}

/// RPS-цель с разбором компонентов.
#[derive(Debug)]
pub struct CapacityTarget {
    /// ID NFR с целью.
    pub nfr_id: String,
    /// Цель, RPS.
    pub rps: f64,
    /// Компоненты цепочки (`CMP` из `affects`).
    pub components: Vec<ComponentCapacity>,
}

/// Отчёт `arch-be nfr capacity`.
#[derive(Debug)]
pub struct CapacityReport {
    /// Корень кейса.
    pub case: PathBuf,
    /// Разобранные RPS-цели.
    pub targets: Vec<CapacityTarget>,
    /// Находки.
    pub issues: Vec<NfrIssue>,
}

impl CapacityReport {
    /// Есть ли блокирующие находки (exit code 1).
    #[must_use]
    pub fn has_errors(&self) -> bool {
        any_errors(&self.issues)
    }

    /// Markdown-отчёт: ёмкость компонентов по каждой цели, находки, итог.
    #[must_use]
    pub fn render(&self) -> String {
        let mut s = String::new();
        let _ = writeln!(s, "# Пропускная способность: {}\n", self.case.display());
        for t in &self.targets {
            let _ = writeln!(s, "## {} — цель {:.0} RPS\n", t.nfr_id, t.rps);
            let _ = writeln!(
                s,
                "| Компонент | RPS/инстанс | Инстансов | Ёмкость | Требуется |"
            );
            let _ = writeln!(
                s,
                "|-----------|------------:|----------:|--------:|----------:|"
            );
            for c in &t.components {
                let per = c
                    .rps_per_instance
                    .map_or_else(|| "—".into(), |v| format!("{v:.0}"));
                let inst = c.instances.map_or_else(|| "—".into(), |v| format!("{v}"));
                let have = match (c.rps_per_instance, c.instances) {
                    (Some(p), Some(n)) => format!("{:.0}", p * f64::from(n)),
                    _ => "—".into(),
                };
                let req = c
                    .required_instances
                    .map_or_else(|| "—".into(), |v| format!("{v}"));
                let _ = writeln!(
                    s,
                    "| {} {} | {} | {} | {} | {} |",
                    c.id, c.title, per, inst, have, req
                );
            }
            let _ = writeln!(s);
        }
        if self.targets.is_empty() {
            let _ = writeln!(s, "RPS-целей (`rps_target` у NFR) не найдено.");
        }
        render_issues(
            &mut s,
            &self.issues,
            &format!("целей: {}", self.targets.len()),
        );
        s
    }
}

/// Проверка пропускной способности: каждый компонент цепочки обязан держать
/// RPS-цель (`instances × rps_per_instance ≥ target`); требуемое число
/// инстансов — `⌈target / rps_per_instance⌉`.
///
/// # Errors
/// Каталог `<case>/model` отсутствует или модель не разбирается.
pub fn capacity_check(case: &Path) -> Result<CapacityReport> {
    let model = load_case_model(case)?;
    let mut issues = Vec::new();
    push_load_skip_note(&model, &mut issues);
    let mut targets = Vec::new();
    for nfr in model.entities.iter().filter(|e| e.kind == EntityKind::Nfr) {
        let Some(raw_target) = nfr.rps_target else {
            continue;
        };
        let Some(target) = checked_positive(nfr, "rps_target", raw_target, &mut issues) else {
            continue;
        };
        let mut components = Vec::new();
        for id in &nfr.affects {
            let Some(cmp) = model.get(id) else {
                continue;
            };
            if cmp.kind != EntityKind::Cmp {
                continue;
            }
            let per = cmp
                .rps_per_instance
                .and_then(|v| checked_positive(cmp, "rps_per_instance", v, &mut issues));
            if cmp.rps_per_instance.is_none() {
                issue(
                    &mut issues,
                    Severity::Warn,
                    "capacity-no-data",
                    format!(
                        "{}: компонент {} ({}) без ёмкости инстанса (rps_per_instance)",
                        nfr.id, cmp.id, cmp.title
                    ),
                );
            }
            if cmp.instances.is_none() {
                issue(
                    &mut issues,
                    Severity::Warn,
                    "capacity-no-instances",
                    format!(
                        "{}: компонент {} ({}) без числа инстансов (instances)",
                        nfr.id, cmp.id, cmp.title
                    ),
                );
            }
            let required = per.map(|p| {
                // ceil положительного частного двух валидированных положительных
                // чисел: насыщающий каст безопасен (RPS/ёмкости ≪ u64::MAX).
                #[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let v = (target / p).ceil() as u64;
                v
            });
            if let (Some(p), Some(n)) = (per, cmp.instances) {
                if f64::from(n) * p < target {
                    issue(
                        &mut issues,
                        Severity::Error,
                        "capacity-insufficient",
                        format!(
                            "{}: {} держит {:.0} RPS ({n} инст. × {p:.0}), цель {target:.0} RPS — требуется {} инст.",
                            nfr.id,
                            cmp.id,
                            f64::from(n) * p,
                            required.unwrap_or(0)
                        ),
                    );
                }
            }
            components.push(ComponentCapacity {
                id: cmp.id.clone(),
                title: cmp.title.clone(),
                rps_per_instance: per,
                instances: cmp.instances,
                required_instances: required,
            });
        }
        if components.is_empty() {
            issue(
                &mut issues,
                Severity::Error,
                "capacity-no-chain",
                format!(
                    "{}: заявлена цель {target:.0} RPS, но в affects нет ни одного CMP-* — цепочка пуста",
                    nfr.id
                ),
            );
        }
        targets.push(CapacityTarget {
            nfr_id: nfr.id.clone(),
            rps: target,
            components,
        });
    }
    Ok(CapacityReport {
        case: case.to_path_buf(),
        targets,
        issues,
    })
}

// ---------------------------------------------------------------------------
// Стоимость
// ---------------------------------------------------------------------------

/// Строка стоимости компонента.
#[derive(Debug)]
pub struct ComponentCost {
    /// ID компонента.
    pub id: String,
    /// Название.
    pub title: String,
    /// Инстансов.
    pub instances: u32,
    /// Тариф за инстанс в месяц.
    pub cost_per_instance_month: f64,
    /// Месячная стоимость.
    pub monthly: f64,
    /// Разовая цена выхода (`None` — не заявлена).
    pub exit_cost: Option<f64>,
}

/// Отчёт `arch-be nfr cost`.
#[derive(Debug)]
pub struct CostReport {
    /// Корень кейса.
    pub case: PathBuf,
    /// Валюта (поле `currency` ёмкостного NFR, дефолт «у.е.»).
    pub currency: String,
    /// Позиции стоимости.
    pub components: Vec<ComponentCost>,
    /// TCO в месяц.
    pub monthly_total: f64,
    /// Суммарная цена выхода.
    pub exit_total: f64,
    /// Находки.
    pub issues: Vec<NfrIssue>,
}

impl CostReport {
    /// Есть ли блокирующие находки (exit code 1).
    #[must_use]
    pub fn has_errors(&self) -> bool {
        any_errors(&self.issues)
    }

    /// TCO в год.
    #[must_use]
    pub fn annual_total(&self) -> f64 {
        self.monthly_total * 12.0
    }

    /// Markdown-отчёт: позиции, TCO, цена выхода, итог.
    #[must_use]
    pub fn render(&self) -> String {
        let mut s = String::new();
        let _ = writeln!(s, "# Стоимость: {}\n", self.case.display());
        let _ = writeln!(s, "Валюта: {}\n", self.currency);
        if self.components.is_empty() {
            let _ = writeln!(
                s,
                "Тарифных данных (cost_per_instance_month у CMP) не найдено."
            );
        } else {
            let _ = writeln!(
                s,
                "| Компонент | Инстансов | Тариф/мес | В месяц | Цена выхода |"
            );
            let _ = writeln!(
                s,
                "|-----------|----------:|----------:|--------:|------------:|"
            );
            for c in &self.components {
                let exit = c
                    .exit_cost
                    .map_or_else(|| "—".into(), |v| format!("{v:.0}"));
                let _ = writeln!(
                    s,
                    "| {} {} | {} | {:.0} | {:.0} | {} |",
                    c.id, c.title, c.instances, c.cost_per_instance_month, c.monthly, exit
                );
            }
            let _ = writeln!(
                s,
                "\nTCO: {1:.0} {0}/мес · {2:.0} {0}/год",
                self.currency,
                self.monthly_total,
                self.annual_total()
            );
            let annual = self.annual_total();
            if annual > 0.0 {
                let share = self.exit_total / annual * 100.0;
                let _ = writeln!(
                    s,
                    "Цена выхода: {:.0} {} ({share:.0}% годового TCO)",
                    self.exit_total, self.currency
                );
            } else {
                let _ = writeln!(s, "Цена выхода: {:.0} {}", self.exit_total, self.currency);
            }
        }
        render_issues(
            &mut s,
            &self.issues,
            &format!("позиций: {}", self.components.len()),
        );
        s
    }
}

/// Стоимость: TCO = Σ `instances × cost_per_instance_month`, цена выхода =
/// Σ `exit_cost` (данные сущностей, не хардкод). Неполные тарифные данные —
/// `warn`, позиция исключается из суммы.
///
/// # Errors
/// Каталог `<case>/model` отсутствует или модель не разбирается.
pub fn cost_check(case: &Path) -> Result<CostReport> {
    let model = load_case_model(case)?;
    let mut issues = Vec::new();
    push_load_skip_note(&model, &mut issues);
    let currency = model
        .entities
        .iter()
        .filter(|e| e.kind == EntityKind::Nfr)
        .find_map(|e| e.currency.clone())
        .unwrap_or_else(|| "у.е.".into());
    let mut components = Vec::new();
    for cmp in model.entities.iter().filter(|e| e.kind == EntityKind::Cmp) {
        match (cmp.cost_per_instance_month, cmp.instances) {
            (Some(raw), Some(n)) => {
                let Some(tariff) = checked(cmp, "cost_per_instance_month", raw, &mut issues) else {
                    continue;
                };
                let exit = cmp
                    .exit_cost
                    .and_then(|v| checked(cmp, "exit_cost", v, &mut issues));
                components.push(ComponentCost {
                    id: cmp.id.clone(),
                    title: cmp.title.clone(),
                    instances: n,
                    cost_per_instance_month: tariff,
                    monthly: f64::from(n) * tariff,
                    exit_cost: exit,
                });
            }
            (Some(_), None) => issue(
                &mut issues,
                Severity::Warn,
                "cost-no-instances",
                format!(
                    "{}: тариф заявлен, но нет числа инстансов (instances) — исключён из TCO",
                    cmp.id
                ),
            ),
            (None, Some(_)) => issue(
                &mut issues,
                Severity::Warn,
                "cost-no-tariff",
                format!(
                    "{}: инстансы заявлены, но нет тарифа (cost_per_instance_month) — исключён из TCO",
                    cmp.id
                ),
            ),
            (None, None) => {}
        }
    }
    if components.is_empty() {
        issue(
            &mut issues,
            Severity::Warn,
            "cost-no-data",
            "ни у одного CMP нет полных тарифных данных (cost_per_instance_month + instances)"
                .into(),
        );
    }
    let monthly_total = components.iter().map(|c| c.monthly).sum();
    let exit_total = components.iter().filter_map(|c| c.exit_cost).sum();
    Ok(CostReport {
        case: case.to_path_buf(),
        currency,
        components,
        monthly_total,
        exit_total,
        issues,
    })
}

// ---------------------------------------------------------------------------
// Runtime fitness: сверка фактических метрик с бюджетами/SLA модели (D4)
// ---------------------------------------------------------------------------

/// Операция из файла метрик (p99, мс).
#[derive(Debug, Deserialize)]
struct OperationMetric {
    /// Id операции (обычно id hop'а `INT-*`).
    #[serde(default)]
    id: Option<String>,
    /// Или название hop'а (альтернатива id).
    #[serde(default)]
    name: Option<String>,
    /// Измеренный p99, мс.
    p99_ms: f64,
}

/// Файл фактических метрик (JSON; D4, ADR-059): p99 по операциям, общая
/// доступность и частота ошибок. Источник — файл (экспорт Prometheus — будущая
/// работа), сеть в ядре не используется (Core-граница C-34).
#[derive(Debug, Deserialize)]
struct MetricsFile {
    /// Окно измерения (метка, свободный текст).
    #[serde(default)]
    window: Option<String>,
    /// Метрики по операциям.
    #[serde(default)]
    operations: Vec<OperationMetric>,
    /// Измеренная доступность (доля).
    #[serde(default)]
    availability: Option<f64>,
    /// Измеренная частота ошибок (доля).
    #[serde(default)]
    error_rate: Option<f64>,
    /// Явный бюджет частоты ошибок (иначе — из SLA: `1 − SLA`).
    #[serde(default)]
    error_rate_target: Option<f64>,
}

/// Строка сверки одной метрики с бюджетом/целью модели.
#[derive(Debug)]
pub struct MetricComparison {
    /// Субъект сверки (id hop'а `INT-*` либо `availability`/`error_rate`).
    pub subject: String,
    /// Что измерено.
    pub measured: f64,
    /// Бюджет/цель из модели (`None` — сверять не с чем).
    pub budget: Option<f64>,
    /// В пределах бюджета (или бюджета нет — не сравнивалось).
    pub within: bool,
}

/// Отчёт `arch-be nfr verify --metrics <файл>` (D4, ADR-059).
#[derive(Debug)]
pub struct MetricsReport {
    /// Корень кейса.
    pub case: PathBuf,
    /// Файл метрик.
    pub metrics: PathBuf,
    /// Окно измерения (метка).
    pub window: Option<String>,
    /// Строки сверки.
    pub comparisons: Vec<MetricComparison>,
    /// Находки.
    pub issues: Vec<NfrIssue>,
}

impl MetricsReport {
    /// Есть ли блокирующие находки (exit code 1).
    #[must_use]
    pub fn has_errors(&self) -> bool {
        any_errors(&self.issues)
    }

    /// Markdown-отчёт: таблица сверки, находки, итог.
    #[must_use]
    pub fn render(&self) -> String {
        let mut s = String::new();
        let _ = writeln!(s, "# Runtime fitness: {}", self.case.display());
        let _ = writeln!(s, "\nМетрики: {}", self.metrics.display());
        if let Some(w) = &self.window {
            let _ = writeln!(s, "Окно: {w}");
        }
        if self.comparisons.is_empty() {
            let _ = writeln!(s, "\nСверок нет (нет операций/метрик или целей в модели).");
        } else {
            let _ = writeln!(s, "\n| Субъект | Измерено | Бюджет/цель | Статус |");
            let _ = writeln!(s, "|---------|---------:|------------:|--------|");
            for c in &self.comparisons {
                let budget = c.budget.map_or_else(|| "—".into(), |b| format!("{b:.6}"));
                let status = match (c.within, c.budget.is_some()) {
                    (_, false) => "нет цели",
                    (true, _) => "OK",
                    (false, _) => "РАСХОЖДЕНИЕ",
                };
                let _ = writeln!(
                    s,
                    "| {} | {:.6} | {} | {status} |",
                    c.subject, c.measured, budget
                );
            }
        }
        render_issues(
            &mut s,
            &self.issues,
            &format!("сверок: {}", self.comparisons.len()),
        );
        s
    }
}

/// Разрешает hop по id или названию (только `INT-*` — носители
/// `latency_budget_ms`).
fn resolve_hop<'m>(model: &'m Model, key: &str) -> Option<&'m Entity> {
    model
        .entities
        .iter()
        .find(|e| e.kind == EntityKind::Int && (e.id == key || e.title == key))
}

/// Проверяет конечность/неотрицательность метрики (иначе — `bad-value`).
fn metric_value(name: &str, v: f64, source: &Path, issues: &mut Vec<NfrIssue>) -> Option<f64> {
    if v.is_finite() && v >= 0.0 {
        return Some(v);
    }
    issue(
        issues,
        Severity::Error,
        "bad-value",
        format!(
            "{}: метрика `{name}` = {v} — ожидается конечное неотрицательное число",
            source.display()
        ),
    );
    None
}

/// Сверка фактических метрик (файл JSON) с бюджетами/SLA модели: p99 операций
/// против `latency_budget_ms` hop'ов `INT-*`, доступность против минимума
/// `availability_target` из NFR, частота ошибок против явного
/// `error_rate_target` или бюджета `1 − SLA`. Расхождение — `error`-находка с
/// виновным hop/звеном; сети нет (источник — файл).
///
/// # Errors
/// Каталог `<case>/model` отсутствует/не разбирается; файл метрик не читается
/// или не является валидным JSON.
pub fn metrics_verify(case: &Path, metrics: &Path) -> Result<MetricsReport> {
    let model = load_case_model(case)?;
    let text = std::fs::read_to_string(metrics).map_err(|e| HarnessError::io(metrics, e))?;
    let file: MetricsFile = serde_json::from_str(&text).map_err(|e| {
        HarnessError::Config(format!(
            "метрики {}: невалидный JSON: {e}",
            metrics.display()
        ))
    })?;
    let mut issues = Vec::new();
    push_load_skip_note(&model, &mut issues);
    let mut comparisons = Vec::new();

    // p99 по операциям: сверять нечего без бюджета hop'а — warn, не error.
    for op in &file.operations {
        let key = op
            .id
            .clone()
            .or_else(|| op.name.clone())
            .unwrap_or_default();
        let Some(measured) = metric_value("p99_ms", op.p99_ms, metrics, &mut issues) else {
            continue;
        };
        let Some(hop) = resolve_hop(&model, &key) else {
            issue(
                &mut issues,
                Severity::Warn,
                "runtime-unknown-operation",
                format!("операция '{key}' не найдена среди INT-* модели — сверка p99 пропущена"),
            );
            continue;
        };
        let budget = hop
            .latency_budget_ms
            .and_then(|b| checked(hop, "latency_budget_ms", b, &mut issues));
        if budget.is_none() {
            issue(
                &mut issues,
                Severity::Warn,
                "runtime-no-budget",
                format!(
                    "{} ({}) без заявленного бюджета (latency_budget_ms) — p99 {measured:.0} мс не с чем сверять",
                    hop.id, hop.title
                ),
            );
        }
        let within = budget.is_none_or(|b| measured <= b);
        if let Some(b) = budget {
            if measured > b {
                issue(
                    &mut issues,
                    Severity::Error,
                    "runtime-latency-exceeded",
                    format!(
                        "{} ({}) — фактический p99 {measured:.0} мс превышает бюджет {b:.0} мс",
                        hop.id, hop.title
                    ),
                );
            }
        }
        comparisons.push(MetricComparison {
            subject: hop.id.clone(),
            measured,
            budget,
            within,
        });
    }

    // Доступность: строжайший SLA из NFR (минимум availability_target).
    let sla = model
        .entities
        .iter()
        .filter(|e| e.kind == EntityKind::Nfr)
        .filter_map(|e| {
            e.availability_target
                .and_then(|a| checked_fraction(e, "availability_target", a, &mut issues))
                .map(|a| (e.id.clone(), a))
        })
        .min_by(|a, b| a.1.total_cmp(&b.1));
    if let Some(measured) = file.availability {
        if let Some(measured) = metric_value("availability", measured, metrics, &mut issues) {
            if let Some((nfr_id, target)) = &sla {
                let within = measured >= *target;
                if !within {
                    issue(
                        &mut issues,
                        Severity::Error,
                        "runtime-availability-below-sla",
                        format!(
                            "фактическая доступность {measured:.6} ниже SLA {target:.6} ({nfr_id})"
                        ),
                    );
                }
                comparisons.push(MetricComparison {
                    subject: "availability".to_string(),
                    measured,
                    budget: Some(*target),
                    within,
                });
            } else {
                issue(
                    &mut issues,
                    Severity::Warn,
                    "runtime-no-sla",
                    "измерена доступность, но в модели нет цели (availability_target у NFR)".into(),
                );
                comparisons.push(MetricComparison {
                    subject: "availability".to_string(),
                    measured,
                    budget: None,
                    within: true,
                });
            }
        }
    }

    // Частота ошибок: явный бюджет или дополнение SLA.
    if let Some(measured) = file.error_rate {
        if let Some(measured) = metric_value("error_rate", measured, metrics, &mut issues) {
            let allowed = file
                .error_rate_target
                .and_then(|t| metric_value("error_rate_target", t, metrics, &mut issues))
                .or_else(|| sla.as_ref().map(|(_, target)| 1.0 - target));
            if let Some(allowed) = allowed {
                let within = measured <= allowed;
                if !within {
                    issue(
                        &mut issues,
                        Severity::Error,
                        "runtime-error-rate-exceeded",
                        format!(
                            "фактическая частота ошибок {measured:.6} превышает бюджет {allowed:.6}"
                        ),
                    );
                }
                comparisons.push(MetricComparison {
                    subject: "error_rate".to_string(),
                    measured,
                    budget: Some(allowed),
                    within,
                });
            } else {
                issue(
                    &mut issues,
                    Severity::Warn,
                    "runtime-no-error-budget",
                    "измерена частота ошибок, но нет ни error_rate_target, ни SLA для бюджета"
                        .into(),
                );
                comparisons.push(MetricComparison {
                    subject: "error_rate".to_string(),
                    measured,
                    budget: None,
                    within: true,
                });
            }
        }
    }

    Ok(MetricsReport {
        case: case.to_path_buf(),
        metrics: metrics.to_path_buf(),
        window: file.window.clone(),
        comparisons,
        issues,
    })
}

// ---------------------------------------------------------------------------
// Агентный инструмент `nfr_check` (мост в MCP, транш 1 инверсии)
// ---------------------------------------------------------------------------

/// Инструменты домена: `nfr_check`.
#[must_use]
pub fn tools() -> Vec<Arc<dyn Tool>> {
    vec![Arc::new(NfrCheckTool)]
}

/// Инструмент `nfr_check`: количественные NFR поверх модели кейса —
/// JSON-вердикт `{passed, issues, summary}` (машиночитаемый контракт для
/// MCP-моста: агент хоста получает verdict без вызова bash).
pub struct NfrCheckTool;

#[derive(Debug, Deserialize)]
struct NfrCheckArgs {
    /// Корень кейса (каталог с `model/`).
    path: String,
    /// Проверка: budget | availability | capacity | cost | all (дефолт all).
    kind: Option<String>,
}

/// Находка NFR-проверки в JSON (с меткой проверки-источника).
fn issue_json(check: &str, i: &NfrIssue) -> Value {
    json!({
        "check": check,
        "severity": i.severity.to_string(),
        "rule": i.rule,
        "message": i.message,
    })
}

#[async_trait]
impl Tool for NfrCheckTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "nfr_check".into(),
            description: "Количественные NFR поверх типизированной модели кейса (ADR-007): \
                          budget (сумма latency-бюджетов hop'ов INT против цели p99), \
                          availability (композиция доступности против SLA + RTO/RPO), \
                          capacity (RPS-цель против ёмкости компонентов), cost (TCO и цена \
                          выхода). kind=all — все четыре, агрегат. Ответ — JSON: passed + \
                          issues (расхождения с виновными hop'ами/звеньями, поля check/\
                          severity/rule/message) + summary; passed=false — основание отказать \
                          изменению"
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Корень кейса (каталог с model/)"},
                    "kind": {
                        "type": "string",
                        "description": "Проверка: budget | availability | capacity | cost | all (по умолчанию all — все четыре)",
                        "enum": ["budget", "availability", "capacity", "cost", "all"]
                    }
                },
                "required": ["path"]
            }),
        }
    }

    async fn call(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let args: NfrCheckArgs = match serde_json::from_value(args) {
            Ok(a) => a,
            Err(e) => {
                return Ok(ToolOutput::err(format!(
                    "nfr_check: невалидные аргументы: {e}"
                )));
            }
        };
        let case = ctx.resolve(&args.path);
        let kind = args
            .kind
            .as_deref()
            .unwrap_or("all")
            .trim()
            .to_ascii_lowercase();
        let kinds: Vec<&str> = match kind.as_str() {
            "all" => vec!["budget", "availability", "capacity", "cost"],
            single @ ("budget" | "availability" | "capacity" | "cost") => vec![single],
            other => {
                return Ok(ToolOutput::err(format!(
                    "nfr_check: неизвестный вид проверки '{other}' (допустимы: budget, \
                     availability, capacity, cost, all)"
                )));
            }
        };
        let mut issues: Vec<Value> = Vec::new();
        let mut errors = 0usize;
        let mut warns = 0usize;
        // Warn-пометка E3 (`load-skip`) одна и та же во всех четырёх
        // проверках — при kind=all в JSON идёт один раз.
        let mut seen_load_skips = BTreeSet::new();
        for name in &kinds {
            // Отчёты проверок — разных типов; объединяет их только `issues`.
            let found = match *name {
                "budget" => budget_check(&case).map(|r| r.issues),
                "availability" => availability_check(&case).map(|r| r.issues),
                "capacity" => capacity_check(&case).map(|r| r.issues),
                _ => cost_check(&case).map(|r| r.issues),
            };
            match found {
                Ok(found) => {
                    for i in &found {
                        if i.rule == "load-skip" && !seen_load_skips.insert(i.message.clone()) {
                            continue;
                        }
                        match i.severity {
                            Severity::Error => errors += 1,
                            Severity::Warn => warns += 1,
                        }
                        issues.push(issue_json(name, i));
                    }
                }
                Err(e) => return Ok(ToolOutput::err(format!("nfr_check/{name}: {e}"))),
            }
        }
        let passed = errors == 0;
        let summary = format!(
            "NFR ({kind}): проверок {}, находок {} (error: {errors}, warn: {warns})",
            kinds.len(),
            issues.len()
        );
        let verdict = json!({
            "tool": "nfr_check",
            "passed": passed,
            "checks": kinds,
            "issues": issues,
            "summary": summary,
        });
        // Сериализация собранного объекта не падает; запасной вариант — компактная форма.
        let text = serde_json::to_string_pretty(&verdict).unwrap_or_else(|_| verdict.to_string());
        Ok(ToolOutput::ok(text))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Пишет сущность в `<case>/model/<name>`; `frontmatter` — без маркеров `---`.
    fn entity(case: &Path, name: &str, frontmatter: &str) {
        let dir = case.join("model");
        std::fs::create_dir_all(&dir).expect("model dir");
        std::fs::write(
            dir.join(name),
            format!("---\n{frontmatter}\n---\n\nТело.\n"),
        )
        .expect("фикстура");
    }

    /// INT с бюджетом.
    fn int(case: &Path, n: u32, budget_ms: Option<f64>) {
        let budget = budget_ms.map_or(String::new(), |b| format!("latency_budget_ms: {b}\n"));
        entity(
            case,
            &format!("INT-{n:03}-hop.md"),
            &format!("id: INT-{n:03}\ntype: int\ntitle: Hop {n}\nstatus: accepted\n{budget}"),
        );
    }

    /// NFR с произвольными количественными полями и affects.
    fn nfr(case: &Path, n: u32, fields: &str, affects: &str) {
        entity(
            case,
            &format!("NFR-{n:03}-t.md"),
            &format!(
                "id: NFR-{n:03}\ntype: nfr\ntitle: NFR {n}\nstatus: accepted\n\
                 verification: v\n{fields}affects: [{affects}]"
            ),
        );
    }

    /// CMP с произвольными полями.
    fn cmp(case: &Path, n: u32, fields: &str) {
        entity(
            case,
            &format!("CMP-{n:03}-c.md"),
            &format!("id: CMP-{n:03}\ntype: cmp\ntitle: Cmp {n}\nstatus: designed\n{fields}"),
        );
    }

    // --- budget ------------------------------------------------------------

    /// Битая сущность E3 (`availability_target` строкой) рядом с валидными.
    fn broken_entity(case: &Path) {
        std::fs::write(
            case.join("model").join("NFR-777-broken.md"),
            "---\nid: NFR-777\ntype: nfr\ntitle: SLA\nstatus: accepted\navailability_target: \"99.9\"\n---\n",
        )
        .expect("битая сущность");
    }

    #[test]
    fn budget_tolerates_broken_entity_with_load_skip_warn() {
        let tmp = tempfile::tempdir().expect("tmp");
        let case = tmp.path();
        int(case, 1, Some(800.0));
        int(case, 2, Some(300.0));
        nfr(case, 1, "p99_target_ms: 2000\n", "INT-001, INT-002");
        broken_entity(case);
        let report = budget_check(case).expect("отчёт");
        assert!(!report.has_errors(), "{:?}", report.issues);
        let skip = report
            .issues
            .iter()
            .find(|i| i.rule == "load-skip")
            .expect("warn-находка пропуска");
        assert_eq!(skip.severity, Severity::Warn);
        assert!(skip.message.contains("1 сущностей пропущено"), "{skip:?}");
        // Расчёт по валидному подмножеству не пострадал.
        assert_eq!(report.targets.len(), 1);
        assert!((report.targets[0].sum_ms - 1100.0).abs() < f64::EPSILON);
    }

    /// `nfr_check` (kind=all): пометка `load-skip` идёт в JSON один раз.
    #[tokio::test]
    async fn nfr_check_tool_dedups_load_skip_note() {
        let tmp = tempfile::tempdir().expect("tmp");
        let case = tmp.path();
        int(case, 1, Some(800.0));
        nfr(case, 1, "p99_target_ms: 2000\n", "INT-001");
        broken_entity(case);
        let ctx = ToolContext::new(
            case.to_path_buf(),
            std::sync::Arc::new(crate::config::Config::default()),
        );
        let out = NfrCheckTool
            .call(json!({"path": "."}), &ctx)
            .await
            .expect("вызов");
        assert!(!out.is_error, "{}", out.content);
        let v: Value = serde_json::from_str(&out.content).expect("JSON");
        let skips = v["issues"]
            .as_array()
            .expect("issues")
            .iter()
            .filter(|i| i["rule"] == "load-skip")
            .count();
        assert_eq!(skips, 1, "{v}");
    }

    #[test]
    fn budget_converges_with_reserve() {
        let tmp = tempfile::tempdir().expect("tmp");
        let case = tmp.path();
        int(case, 1, Some(800.0));
        int(case, 2, Some(300.0));
        nfr(case, 1, "p99_target_ms: 2000\n", "INT-001, INT-002");
        let report = budget_check(case).expect("отчёт");
        assert!(!report.has_errors(), "{:?}", report.issues);
        assert_eq!(report.targets.len(), 1);
        let t = &report.targets[0];
        assert_eq!(t.hops.len(), 2);
        assert!((t.sum_ms - 1100.0).abs() < f64::EPSILON);
        let text = report.render();
        assert!(text.contains("резерв: 900 мс"), "{text}");
        assert!(text.contains("Итог: PASS"), "{text}");
    }

    #[test]
    fn budget_exceeded_lists_guilty_hops() {
        let tmp = tempfile::tempdir().expect("tmp");
        let case = tmp.path();
        int(case, 1, Some(1500.0));
        int(case, 2, Some(800.0));
        nfr(case, 1, "p99_target_ms: 2000\n", "INT-001, INT-002");
        let report = budget_check(case).expect("отчёт");
        assert!(report.has_errors());
        let e = report
            .issues
            .iter()
            .find(|i| i.rule == "budget-exceeded")
            .expect("budget-exceeded");
        // Виновные hop'ы поимённо, с бюджетами (DoD P1-1).
        assert!(e.message.contains("INT-001=1500"), "{}", e.message);
        assert!(e.message.contains("INT-002=800"), "{}", e.message);
        assert!(e.message.contains("2300"), "{}", e.message);
        assert!(report.render().contains("Итог: FAIL"));
    }

    #[test]
    fn budget_missing_hop_budget_is_error() {
        let tmp = tempfile::tempdir().expect("tmp");
        let case = tmp.path();
        int(case, 1, Some(800.0));
        int(case, 2, None);
        nfr(case, 1, "p99_target_ms: 2000\n", "INT-001, INT-002");
        let report = budget_check(case).expect("отчёт");
        assert!(report.has_errors());
        let e = report
            .issues
            .iter()
            .find(|i| i.rule == "budget-hop-missing")
            .expect("budget-hop-missing");
        assert!(e.message.contains("INT-002"), "{}", e.message);
    }

    #[test]
    fn budget_target_without_chain_is_error() {
        let tmp = tempfile::tempdir().expect("tmp");
        let case = tmp.path();
        cmp(case, 1, "");
        nfr(case, 1, "p99_target_ms: 2000\n", "CMP-001");
        let report = budget_check(case).expect("отчёт");
        assert!(report.has_errors());
        assert!(
            report.issues.iter().any(|i| i.rule == "budget-no-chain"),
            "{:?}",
            report.issues
        );
    }

    #[test]
    fn budget_uncovered_hop_is_warn_not_error() {
        let tmp = tempfile::tempdir().expect("tmp");
        let case = tmp.path();
        int(case, 1, Some(800.0));
        int(case, 2, Some(300.0));
        nfr(case, 1, "p99_target_ms: 2000\n", "INT-001");
        let report = budget_check(case).expect("отчёт");
        assert!(!report.has_errors(), "{:?}", report.issues);
        assert_eq!(report.uncovered_hops, ["INT-002"]);
        let w = report
            .issues
            .iter()
            .find(|i| i.rule == "budget-hop-uncovered")
            .expect("warn");
        assert_eq!(w.severity, Severity::Warn);
    }

    #[test]
    fn budget_bad_value_is_error() {
        let tmp = tempfile::tempdir().expect("tmp");
        let case = tmp.path();
        int(case, 1, Some(-5.0));
        nfr(case, 1, "p99_target_ms: 2000\n", "INT-001");
        let report = budget_check(case).expect("отчёт");
        assert!(report.has_errors());
        assert!(
            report.issues.iter().any(|i| i.rule == "bad-value"),
            "{:?}",
            report.issues
        );
    }

    // --- availability ------------------------------------------------------

    #[test]
    fn availability_serial_chain_below_target_is_error() {
        let tmp = tempfile::tempdir().expect("tmp");
        let case = tmp.path();
        cmp(case, 1, "availability: 0.999\n");
        cmp(case, 2, "availability: 0.999\n");
        nfr(case, 4, "availability_target: 0.9995\n", "CMP-001, CMP-002");
        nfr(case, 6, "rto_minutes: 15\n", "CMP-001");
        let report = availability_check(case).expect("отчёт");
        // Последовательно: 0.999 × 0.999 = 0.998001 < 0.9995.
        let c = &report.chains[0];
        assert!((c.computed - 0.998_001).abs() < 1e-9, "{}", c.computed);
        assert!(report.has_errors());
        let e = report
            .issues
            .iter()
            .find(|i| i.rule == "availability-below-target")
            .expect("below-target");
        assert!(e.message.contains("NFR-004"), "{}", e.message);
    }

    #[test]
    fn availability_parallel_replicas_lift_segment() {
        let tmp = tempfile::tempdir().expect("tmp");
        let case = tmp.path();
        // Участок 0.99 в трёх репликах: 1 − 0.01³ = 0.999999.
        cmp(case, 1, "availability: 0.99\nreplicas: 3\n");
        cmp(case, 2, "availability: 0.999\n");
        nfr(case, 4, "availability_target: 0.998\n", "CMP-001, CMP-002");
        nfr(case, 6, "rto_minutes: 15\n", "CMP-001");
        let report = availability_check(case).expect("отчёт");
        let c = &report.chains[0];
        let eff = 1.0 - 0.01_f64.powi(3);
        assert!((c.segments[0].effective.expect("eff") - eff).abs() < 1e-12);
        assert!((c.computed - eff * 0.999).abs() < 1e-12, "{}", c.computed);
        assert!(!report.has_errors(), "{:?}", report.issues);
        assert!(c.complete);
        assert!(report.render().contains("Итог: PASS"));
    }

    #[test]
    fn availability_missing_segment_data_is_warn_excluded() {
        let tmp = tempfile::tempdir().expect("tmp");
        let case = tmp.path();
        cmp(case, 1, "availability: 0.999\n");
        cmp(case, 2, "");
        nfr(case, 4, "availability_target: 0.99\n", "CMP-001, CMP-002");
        nfr(case, 6, "rto_minutes: 15\n", "CMP-001");
        let report = availability_check(case).expect("отчёт");
        assert!(!report.has_errors(), "{:?}", report.issues);
        assert!(
            report
                .issues
                .iter()
                .any(|i| i.rule == "availability-no-data" && i.severity == Severity::Warn),
            "{:?}",
            report.issues
        );
        let c = &report.chains[0];
        assert!(!c.complete, "неполные данные помечены");
        assert!((c.computed - 0.999).abs() < 1e-12);
    }

    #[test]
    fn availability_dr_targets_listed_and_absent_warns() {
        let tmp = tempfile::tempdir().expect("tmp");
        let case = tmp.path();
        cmp(case, 1, "availability: 0.999\n");
        nfr(case, 4, "availability_target: 0.99\n", "CMP-001");
        nfr(case, 6, "rto_minutes: 15\n", "CMP-001");
        nfr(case, 7, "rpo_seconds: 0\n", "CMP-001");
        let report = availability_check(case).expect("отчёт");
        assert_eq!(report.dr.len(), 2);
        assert_eq!(report.dr[0].rto_minutes, Some(15.0));
        assert_eq!(report.dr[1].rpo_seconds, Some(0.0));
        assert!(
            !report
                .issues
                .iter()
                .any(|i| i.rule == "availability-no-dr-targets"),
            "{:?}",
            report.issues
        );

        // SLA без DR-целей — warn.
        let tmp2 = tempfile::tempdir().expect("tmp");
        cmp(tmp2.path(), 1, "availability: 0.999\n");
        nfr(tmp2.path(), 4, "availability_target: 0.99\n", "CMP-001");
        let report2 = availability_check(tmp2.path()).expect("отчёт");
        assert!(
            report2
                .issues
                .iter()
                .any(|i| i.rule == "availability-no-dr-targets"),
            "{:?}",
            report2.issues
        );
    }

    #[test]
    fn availability_target_out_of_range_is_error() {
        let tmp = tempfile::tempdir().expect("tmp");
        let case = tmp.path();
        cmp(case, 1, "availability: 0.999\n");
        nfr(case, 4, "availability_target: 1.5\n", "CMP-001");
        let report = availability_check(case).expect("отчёт");
        assert!(report.has_errors());
        assert!(report.issues.iter().any(|i| i.rule == "bad-value"));
    }

    // --- capacity ----------------------------------------------------------

    #[test]
    fn capacity_sufficient_computes_required() {
        let tmp = tempfile::tempdir().expect("tmp");
        let case = tmp.path();
        cmp(case, 1, "rps_per_instance: 2000\ninstances: 3\n");
        nfr(case, 3, "rps_target: 5000\n", "CMP-001");
        let report = capacity_check(case).expect("отчёт");
        assert!(!report.has_errors(), "{:?}", report.issues);
        let c = &report.targets[0].components[0];
        // ceil(5000 / 2000) = 3.
        assert_eq!(c.required_instances, Some(3));
        assert!(report.render().contains("Итог: PASS"));
    }

    #[test]
    fn capacity_insufficient_is_error() {
        let tmp = tempfile::tempdir().expect("tmp");
        let case = tmp.path();
        cmp(case, 1, "rps_per_instance: 1500\ninstances: 2\n");
        nfr(case, 3, "rps_target: 5000\n", "CMP-001");
        let report = capacity_check(case).expect("отчёт");
        assert!(report.has_errors());
        let e = report
            .issues
            .iter()
            .find(|i| i.rule == "capacity-insufficient")
            .expect("insufficient");
        // 2 × 1500 = 3000 < 5000, требуется ceil(5000/1500) = 4.
        assert!(e.message.contains("CMP-001"), "{}", e.message);
        assert!(e.message.contains("4 инст."), "{}", e.message);
    }

    #[test]
    fn capacity_missing_data_is_warn() {
        let tmp = tempfile::tempdir().expect("tmp");
        let case = tmp.path();
        cmp(case, 1, "");
        nfr(case, 3, "rps_target: 5000\n", "CMP-001");
        let report = capacity_check(case).expect("отчёт");
        assert!(!report.has_errors(), "{:?}", report.issues);
        assert!(
            report.issues.iter().any(|i| i.rule == "capacity-no-data"),
            "{:?}",
            report.issues
        );
        assert!(
            report
                .issues
                .iter()
                .any(|i| i.rule == "capacity-no-instances"),
            "{:?}",
            report.issues
        );
    }

    // --- cost --------------------------------------------------------------

    #[test]
    fn cost_tco_and_exit_price() {
        let tmp = tempfile::tempdir().expect("tmp");
        let case = tmp.path();
        cmp(
            case,
            1,
            "cost_per_instance_month: 45000\ninstances: 4\nexit_cost: 300000\n",
        );
        cmp(case, 2, "cost_per_instance_month: 55000\ninstances: 2\n");
        nfr(case, 3, "currency: RUB\n", "CMP-001");
        let report = cost_check(case).expect("отчёт");
        assert!(!report.has_errors(), "{:?}", report.issues);
        assert_eq!(report.currency, "RUB");
        // 4×45000 + 2×55000 = 290000/мес, год 3 480 000, выход 300 000.
        assert!((report.monthly_total - 290_000.0).abs() < f64::EPSILON);
        assert!((report.annual_total() - 3_480_000.0).abs() < f64::EPSILON);
        assert!((report.exit_total - 300_000.0).abs() < f64::EPSILON);
        let text = report.render();
        assert!(text.contains("290000"), "{text}");
        assert!(text.contains("RUB"), "{text}");
    }

    #[test]
    fn cost_incomplete_data_is_warn_excluded() {
        let tmp = tempfile::tempdir().expect("tmp");
        let case = tmp.path();
        cmp(case, 1, "cost_per_instance_month: 45000\ninstances: 4\n");
        cmp(case, 2, "cost_per_instance_month: 1000\n"); // без instances
        cmp(case, 3, "instances: 2\n"); // без тарифа
        let report = cost_check(case).expect("отчёт");
        assert!(!report.has_errors(), "{:?}", report.issues);
        assert_eq!(report.components.len(), 1, "полная позиция одна");
        assert!(
            report.issues.iter().any(|i| i.rule == "cost-no-instances"),
            "{:?}",
            report.issues
        );
        assert!(
            report.issues.iter().any(|i| i.rule == "cost-no-tariff"),
            "{:?}",
            report.issues
        );
    }

    #[test]
    fn cost_without_any_tariffs_is_warn() {
        let tmp = tempfile::tempdir().expect("tmp");
        let case = tmp.path();
        cmp(case, 1, "");
        let report = cost_check(case).expect("отчёт");
        assert!(!report.has_errors());
        assert_eq!(report.currency, "у.е.");
        assert!(report.issues.iter().any(|i| i.rule == "cost-no-data"));
    }

    // --- общее -------------------------------------------------------------

    #[test]
    fn case_without_model_dir_is_error() {
        let tmp = tempfile::tempdir().expect("tmp");
        let err = budget_check(tmp.path()).expect_err("нет model/");
        assert!(err.to_string().contains("model"), "{err}");
    }

    #[test]
    fn error_drives_exit_code_contract() {
        // Контракт exit code: has_errors() == true → CLI делает exit(1).
        let tmp = tempfile::tempdir().expect("tmp");
        let case = tmp.path();
        int(case, 1, Some(3000.0));
        nfr(case, 1, "p99_target_ms: 2000\n", "INT-001");
        assert!(budget_check(case).expect("отчёт").has_errors());
        int(case, 1, Some(1000.0));
        assert!(!budget_check(case).expect("отчёт").has_errors());
    }

    #[test]
    fn reports_render_deterministically() {
        let tmp = tempfile::tempdir().expect("tmp");
        let case = tmp.path();
        int(case, 1, Some(800.0));
        nfr(case, 1, "p99_target_ms: 2000\n", "INT-001");
        let a = budget_check(case).expect("отчёт").render();
        let b = budget_check(case).expect("отчёт").render();
        assert_eq!(a, b);
    }

    // --- инструмент nfr_check ----------------------------------------------

    /// Тестовый контекст без LLM.
    fn tool_ctx(case: &Path) -> ToolContext {
        ToolContext::new(
            case.to_path_buf(),
            Arc::new(crate::config::Config::default()),
        )
    }

    /// Разбирает JSON-вердикт из текстового вывода инструмента.
    fn verdict_json(out: &ToolOutput) -> Value {
        assert!(!out.is_error, "{}", out.content);
        serde_json::from_str(&out.content).expect("вывод nfr_check — JSON-вердикт")
    }

    #[tokio::test]
    async fn nfr_check_all_passes_converging_case() {
        let tmp = tempfile::tempdir().expect("tmp");
        let case = tmp.path();
        int(case, 1, Some(800.0));
        nfr(case, 1, "p99_target_ms: 2000\n", "INT-001");
        let out = NfrCheckTool
            .call(json!({"path": "."}), &tool_ctx(case))
            .await
            .expect("вызов");
        let v = verdict_json(&out);
        assert_eq!(v["passed"], true, "{v}");
        assert_eq!(
            v["checks"],
            json!(["budget", "availability", "capacity", "cost"])
        );
        assert!(
            v["summary"]
                .as_str()
                .expect("summary")
                .contains("NFR (all)")
        );
    }

    #[tokio::test]
    async fn nfr_check_budget_fails_with_guilty_hops_in_issues() {
        let tmp = tempfile::tempdir().expect("tmp");
        let case = tmp.path();
        int(case, 1, Some(3000.0));
        nfr(case, 1, "p99_target_ms: 2000\n", "INT-001");
        let out = NfrCheckTool
            .call(json!({"path": ".", "kind": "budget"}), &tool_ctx(case))
            .await
            .expect("вызов");
        let v = verdict_json(&out);
        assert_eq!(v["passed"], false, "{v}");
        let issue = &v["issues"][0];
        assert_eq!(issue["rule"], "budget-exceeded");
        assert_eq!(issue["check"], "budget");
        // Виновный hop — в машиночитаемой находке (DoD P1-1 сохранён в MCP).
        assert!(
            issue["message"]
                .as_str()
                .expect("msg")
                .contains("INT-001=3000")
        );
    }

    #[tokio::test]
    async fn nfr_check_bad_kind_and_missing_model_are_soft_errors() {
        let tmp = tempfile::tempdir().expect("tmp");
        let out = NfrCheckTool
            .call(
                json!({"path": ".", "kind": "latency"}),
                &tool_ctx(tmp.path()),
            )
            .await
            .expect("вызов");
        assert!(out.is_error, "{}", out.content);
        assert!(out.content.contains("неизвестный вид"), "{}", out.content);
        // Кейса без model/ — мягкая ошибка, не паника.
        let out = NfrCheckTool
            .call(json!({"path": "."}), &tool_ctx(tmp.path()))
            .await
            .expect("вызов");
        assert!(out.is_error, "{}", out.content);
        assert!(out.content.contains("model"), "{}", out.content);
    }

    // --- runtime fitness: nfr verify --metrics (D4, ADR-059) ---------------

    /// Пишет JSON-файл метрик в кейс.
    fn metrics_file(case: &Path, json: &str) -> PathBuf {
        let p = case.join("metrics.json");
        std::fs::write(&p, json).expect("metrics");
        p
    }

    /// Расхождение p99 и доступности → error-находки с виновным hop/звеном.
    #[test]
    fn metrics_verify_flags_latency_and_availability_with_guilty_hop() {
        let tmp = tempfile::tempdir().expect("tmp");
        let case = tmp.path();
        int(case, 1, Some(800.0));
        nfr(case, 4, "availability_target: 0.9995\n", "");
        let metrics = metrics_file(
            case,
            r#"{"operations": [{"id": "INT-001", "p99_ms": 1200}], "availability": 0.997}"#,
        );
        let report = metrics_verify(case, &metrics).expect("отчёт");
        assert!(report.has_errors(), "{:?}", report.issues);
        let lat = report
            .issues
            .iter()
            .find(|i| i.rule == "runtime-latency-exceeded")
            .expect("latency");
        assert!(lat.message.contains("INT-001"), "{}", lat.message);
        let av = report
            .issues
            .iter()
            .find(|i| i.rule == "runtime-availability-below-sla")
            .expect("availability");
        assert!(av.message.contains("NFR-004"), "{}", av.message);
        assert!(report.render().contains("Итог: FAIL"));
    }

    /// Метрики в пределах бюджетов/SLA — PASS, сверки перечислены.
    #[test]
    fn metrics_verify_passes_when_metrics_within_budgets() {
        let tmp = tempfile::tempdir().expect("tmp");
        let case = tmp.path();
        int(case, 1, Some(1500.0));
        nfr(case, 4, "availability_target: 0.999\n", "");
        let metrics = metrics_file(
            case,
            r#"{
                "window": "2026-10-04T00:00:00Z/PT1H",
                "operations": [{"id": "INT-001", "p99_ms": 1200}],
                "availability": 0.9995,
                "error_rate": 0.0001
            }"#,
        );
        let report = metrics_verify(case, &metrics).expect("отчёт");
        assert!(!report.has_errors(), "{:?}", report.issues);
        assert_eq!(report.comparisons.len(), 3, "{:?}", report.comparisons);
        assert!(report.render().contains("Итог: PASS"));
        assert_eq!(report.window.as_deref(), Some("2026-10-04T00:00:00Z/PT1H"));
    }

    /// Операция, которой нет в модели, — warn (не блокирует сверку остальных).
    #[test]
    fn metrics_verify_unknown_operation_is_warn() {
        let tmp = tempfile::tempdir().expect("tmp");
        let case = tmp.path();
        int(case, 1, Some(800.0));
        let metrics = metrics_file(
            case,
            r#"{"operations": [{"id": "INT-999", "p99_ms": 100}]}"#,
        );
        let report = metrics_verify(case, &metrics).expect("отчёт");
        assert!(!report.has_errors(), "{:?}", report.issues);
        assert!(
            report
                .issues
                .iter()
                .any(|i| i.rule == "runtime-unknown-operation" && i.severity == Severity::Warn),
            "{:?}",
            report.issues
        );
    }

    /// Битый JSON метрик — ошибка чтения, не молчаливый PASS.
    #[test]
    fn metrics_verify_bad_json_is_error() {
        let tmp = tempfile::tempdir().expect("tmp");
        let case = tmp.path();
        int(case, 1, Some(800.0));
        let metrics = metrics_file(case, "{ not json");
        assert!(metrics_verify(case, &metrics).is_err());
    }
}
