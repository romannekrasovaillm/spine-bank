//! Составные инструменты архитектора (бэклог волны 3, п.13): `architect_review`
//! и `change_impact`.
//!
//! Зачем: у хостов с BM25-активацией инструментов (omp) и у локальных моделей
//! длинный список мелких инструментов режет попадание — один составной вызов
//! с полным ответом надёжнее, чем серия из шести точечных.
//!
//! КОНТРАКТ (владелец: агент `review`):
//! - [`architect_review`] — единое ревью репозитория одним ответом: маршрут
//!   значимости из git-диффа + весь контур [`crate::gate`] (`fitness`,
//!   `delta_guard`, `rule_weakened`, `spine_lint`, `trace_check`; на маршрутах
//!   Standard/Critical — `nfr` и `evidence`) + две дополнительные секции:
//!   `model_validate` (ссылочная целостность `model/`) и `contracts`
//!   (линт контрактов `OpenAPI`/`AsyncAPI` — файлы из полей `contract`
//!   сущностей INT (ADR-035) и из каталога `contracts/`). Каждая секция
//!   fail-soft SKIP при отсутствии входа (как у гейта); провал любой —
//!   итоговый `passed = false`;
//! - [`change_impact`] — «что я задену и с кем согласовывать»: от сущности
//!   (`id`) или от файлов (`paths` → CMP по `code_roots`, ADR-030)
//!   транзитивный обход графа связей модели в ОБЕ стороны (`depends_on` /
//!   `implements` / `affects` / `verified_by` + обратные ссылки) → затронутые
//!   сущности по типам, правила CONSTRAINTS.yaml (`C-NNN` из `verified_by`,
//!   с владельцами из карточек правил), контракты достигнутых INT,
//!   владельцы (достигнутые OWNER). Это отчёт, а не гейт: вердикта
//!   `passed` нет. Неизвестный `id` — честная ошибка; путь без
//!   CMP-покрытия — пометка в `gaps` (не ошибка);
//! - инструменты агента: `architect_review`, `change_impact` ([`tools`]);
//!   CLI: `arch-be review` и `arch-be model impact`; мост MCP — по белому
//!   списку `mcp_server.rs` (read-only).

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::error::{HarnessError, Result};
use crate::gate::{self, GateComponent, GateFinding, GateReport, GateStatus};
use crate::llm::ToolSpec;
use crate::model::{EntityKind, LinkKind, Model, load_model, validate};
use crate::tool::{Tool, ToolContext, ToolOutput};
use crate::{asyncapi, control, openapi};

/// Потолок находок одной секции в JSON/текстовом выводе (остаток —
/// счётчиком): составной ответ читают агент и человек, простыня находок
/// не нужна — полный список дают команды/инструменты составляющих.
const MAX_SECTION_FINDINGS: usize = 20;

/// Потолок контрактных файлов в одном прогоне секции `contracts` (защита
/// от гигантского каталога `contracts/` — линтим детерминированный
/// префикс отсортированного списка).
const MAX_CONTRACT_FILES: usize = 32;

/// Потолок байт для сниффинга формата контракта (маркер `openapi:` /
/// `asyncapi:` живёт в шапке документа).
const MAX_SNIFF_BYTES: usize = 64 * 1024;

/// Отчёт составного ревью [`architect_review`]: отчёт гейта (маршрут,
/// контурные составляющие) + секции `model_validate` и `contracts`,
/// дописанные в конец списка составляющих.
#[derive(Debug)]
pub struct ReviewReport {
    /// Отчёт единого гейта с дополненным списком составляющих и
    /// пересчитанным итогом `passed`.
    pub gate: GateReport,
}

impl ReviewReport {
    /// Итоговая строка сводки (одна строка для агента/CI-лога).
    #[must_use]
    pub fn summary(&self) -> String {
        let failed: Vec<&str> = self
            .gate
            .components
            .iter()
            .filter(|c| c.status == GateStatus::Fail)
            .map(|c| c.name)
            .collect();
        if failed.is_empty() {
            format!(
                "Ревью {}: PASS — маршрут {}, секций: {}",
                self.gate.repo.display(),
                self.gate.route,
                self.gate.components.len()
            )
        } else {
            format!(
                "Ревью {}: FAIL — маршрут {}, провалены: {}",
                self.gate.repo.display(),
                self.gate.route,
                failed.join(", ")
            )
        }
    }
}

/// Метка статуса секции для рендера (`GateStatus::label` приватна в gate).
fn status_label(status: GateStatus) -> &'static str {
    match status {
        GateStatus::Pass => "PASS",
        GateStatus::Fail => "FAIL",
        GateStatus::Skip => "SKIP",
    }
}

/// Секция `model_validate`: ссылочная целостность `model/`
/// ([`crate::model::validate`]). Входа нет — SKIP; модель не разбирается
/// или есть error-находки — FAIL.
fn component_model_validate(repo: &Path) -> GateComponent {
    let model_dir = repo.join("model");
    if !model_dir.is_dir() {
        return GateComponent {
            name: "model_validate",
            status: GateStatus::Skip,
            detail: "нет каталога model/".to_string(),
            findings: Vec::new(),
        };
    }
    let model = match load_model(&model_dir) {
        Ok(m) => m,
        Err(e) => {
            return GateComponent {
                name: "model_validate",
                status: GateStatus::Fail,
                detail: format!("сбой загрузки модели: {e}"),
                findings: Vec::new(),
            };
        }
    };
    let report = validate(&model);
    let errors = report
        .issues
        .iter()
        .filter(|i| i.severity == crate::model::Severity::Error)
        .count();
    let findings: Vec<GateFinding> = report
        .issues
        .iter()
        .map(|i| GateFinding {
            severity: i.severity.to_string(),
            rule: Some(i.rule.to_string()),
            file: Some(i.file.display().to_string()),
            line: None,
            message: i.message.clone(),
        })
        .collect();
    GateComponent {
        name: "model_validate",
        status: if errors == 0 {
            GateStatus::Pass
        } else {
            GateStatus::Fail
        },
        detail: format!(
            "сущностей: {}, находок: {} (error: {errors})",
            report.entities,
            report.issues.len()
        ),
        findings,
    }
}

/// Распознаваемый секцией `contracts` формат контракта.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ContractKind {
    /// `OpenAPI` 3.x (`src/openapi.rs`).
    OpenApi,
    /// `AsyncAPI` 2.x/3.x (`src/asyncapi.rs`).
    AsyncApi,
}

/// Сниффинг формата контракта по шапке файла (до [`MAX_SNIFF_BYTES`]):
/// маркеры `openapi`/`asyncapi` в JSON- (`"openapi":`) и YAML-форме
/// (строка `openapi:` в начале строки). Остальные форматы (proto, avsc,
/// sql, JSON Schema) секцией не линтуются — для них есть `contract_diff`.
fn sniff_contract(path: &Path) -> Option<ContractKind> {
    let bytes = std::fs::read(path).ok()?;
    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(MAX_SNIFF_BYTES)]);
    let json_openapi = head.contains("\"openapi\"");
    let json_asyncapi = head.contains("\"asyncapi\"");
    let mut yaml_openapi = false;
    let mut yaml_asyncapi = false;
    for line in head.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("openapi:") {
            yaml_openapi = true;
        }
        if trimmed.starts_with("asyncapi:") {
            yaml_asyncapi = true;
        }
    }
    if json_openapi || yaml_openapi {
        Some(ContractKind::OpenApi)
    } else if json_asyncapi || yaml_asyncapi {
        Some(ContractKind::AsyncApi)
    } else {
        None
    }
}

/// Собирает контрактные файлы репозитория: пути из полей `contract`
/// сущностей INT (`model/`, ADR-035) + файлы `contracts/*.{yaml,yml,json}`
/// верхнего уровня. Возвращает (отсортированный дедуплицированный список
/// существующих файлов, число заявленных, но не найденных путей).
///
/// Модель, не загружающаяся, здесь не валит секцию — её ошибку покажет
/// `model_validate`; заявленные INT-пути тогда просто не читаются.
fn contract_candidates(repo: &Path) -> (Vec<PathBuf>, usize) {
    let mut files: BTreeSet<PathBuf> = BTreeSet::new();
    let mut missing = 0usize;
    let model_dir = repo.join("model");
    if model_dir.is_dir() {
        // Ошибку загрузки модели осознанно игнорируем: её отразит секция
        // model_validate, а секция контрактов работает с тем, что есть.
        if let Ok(model) = load_model(&model_dir) {
            for e in &model.entities {
                if e.kind != EntityKind::Int {
                    continue;
                }
                let Some(contract) = e.contract.as_deref().map(str::trim) else {
                    continue;
                };
                if contract.is_empty() {
                    continue;
                }
                let path = repo.join(contract);
                if path.is_file() {
                    files.insert(path);
                } else {
                    // Битый путь контракта — error звена «INT → контракт»
                    // (trace_check); здесь только счётчик в сводке.
                    missing += 1;
                }
            }
        }
    }
    let contracts_dir = repo.join("contracts");
    if let Ok(rd) = std::fs::read_dir(&contracts_dir) {
        let mut listed: Vec<PathBuf> = rd
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.is_file()
                    && p.extension()
                        .is_some_and(|ext| ext == "yaml" || ext == "yml" || ext == "json")
            })
            .collect();
        listed.sort();
        files.extend(listed);
    }
    (
        files.into_iter().take(MAX_CONTRACT_FILES).collect(),
        missing,
    )
}

/// Секция `contracts`: линт контрактов OpenAPI/AsyncAPI
/// ([`openapi::lint_openapi`], [`asyncapi::lint_asyncapi`]) по файлам из
/// [`contract_candidates`]. error-находка любого линтера или сбой разбора
/// распознанного файла — FAIL; файлов нет — SKIP.
fn component_contracts(repo: &Path) -> GateComponent {
    let (files, missing) = contract_candidates(repo);
    if files.is_empty() {
        return GateComponent {
            name: "contracts",
            status: GateStatus::Skip,
            detail: if missing == 0 {
                "нет контрактных файлов (ни INT.contract, ни contracts/*.{yaml,yml,json})"
                    .to_string()
            } else {
                format!("заявленные INT.contract не найдены ({missing}) — см. trace_check")
            },
            findings: Vec::new(),
        };
    }
    let mut findings: Vec<GateFinding> = Vec::new();
    let mut errors = 0usize;
    let mut linted_openapi = 0usize;
    let mut linted_asyncapi = 0usize;
    let mut unrecognized = 0usize;
    for path in &files {
        let rel = path
            .strip_prefix(repo)
            .unwrap_or(path)
            .display()
            .to_string();
        let Some(kind) = sniff_contract(path) else {
            unrecognized += 1;
            continue;
        };
        // У линтеров разные типы Finding — нормализуем в строки находок
        // по месте вызова, без промежуточного общего вектора.
        match kind {
            ContractKind::OpenApi => {
                linted_openapi += 1;
                match openapi::lint_openapi(path) {
                    Ok(found) => {
                        for f in &found {
                            if f.severity == "error" {
                                errors += 1;
                            }
                            findings.push(GateFinding {
                                severity: f.severity.clone(),
                                rule: Some(f.rule.clone()),
                                file: Some(rel.clone()),
                                line: None,
                                message: f.message.clone(),
                            });
                        }
                    }
                    // Файл распознан по маркеру, но линтер его не принял
                    // (например, Swagger 2.0) — это находка, а не сбой.
                    Err(e) => {
                        errors += 1;
                        findings.push(GateFinding {
                            severity: "error".to_string(),
                            rule: Some("contract-lint".to_string()),
                            file: Some(rel.clone()),
                            line: None,
                            message: e.to_string(),
                        });
                    }
                }
            }
            ContractKind::AsyncApi => {
                linted_asyncapi += 1;
                match asyncapi::lint_asyncapi(path) {
                    Ok(found) => {
                        for f in &found {
                            if f.severity == "error" {
                                errors += 1;
                            }
                            findings.push(GateFinding {
                                severity: f.severity.clone(),
                                rule: Some(f.rule.clone()),
                                file: Some(rel.clone()),
                                line: None,
                                message: f.message.clone(),
                            });
                        }
                    }
                    Err(e) => {
                        errors += 1;
                        findings.push(GateFinding {
                            severity: "error".to_string(),
                            rule: Some("contract-lint".to_string()),
                            file: Some(rel.clone()),
                            line: None,
                            message: e.to_string(),
                        });
                    }
                }
            }
        }
    }
    let detail = format!(
        "файлов: {} (openapi: {linted_openapi}, asyncapi: {linted_asyncapi}, не распознано: {unrecognized}, потеряно: {missing}), находок: {} (error: {errors})",
        files.len(),
        findings.len()
    );
    GateComponent {
        name: "contracts",
        status: if errors == 0 {
            GateStatus::Pass
        } else {
            GateStatus::Fail
        },
        detail,
        findings,
    }
}

/// Прогоняет составное архитектурное ревью репозитория: контур
/// [`gate::run`] (маршрут `auto` из диффа) + секции `model_validate` и
/// `contracts`.
///
/// `base`: git-ref базы для диффа/delta guard/сравнения правил (`None` —
/// рабочее дерево против HEAD). `constraints`: файл ограничений (дефолт
/// `<repo>/.arch-handoff/CONSTRAINTS.yaml`). `limits` — пороги маршрутов
/// `(fast_max, standard_max)` из конфига (`[significance]`, ADR-034).
///
/// # Errors
/// Репозиторий недоступен. Провалы секций — НЕ ошибка: они в отчёте
/// (`passed = false`), exit-код ставит CLI-край.
pub fn architect_review(
    repo: &Path,
    base: Option<&str>,
    constraints: Option<&Path>,
    limits: (usize, usize),
) -> Result<ReviewReport> {
    let mut gate_report = gate::run(repo, None, base, constraints, limits)?;
    gate_report.components.push(component_model_validate(repo));
    gate_report.components.push(component_contracts(repo));
    gate_report.passed = gate_report
        .components
        .iter()
        .all(|c| c.status != GateStatus::Fail);
    Ok(ReviewReport { gate: gate_report })
}

/// Текстовый рендер отчёта ревью: строка маршрута, по каждой секции
/// PASS/FAIL/SKIP + краткая сводка, находки отступом (с потолком
/// [`MAX_SECTION_FINDINGS`]), итоговая строка «Итог: PASS/FAIL».
#[must_use]
pub fn render_review(report: &ReviewReport) -> String {
    let gate = &report.gate;
    let mut out = String::new();
    // Запись в String не может завершиться ошибкой — игноры безопасны.
    let _ = writeln!(out, "Ревью: {}", gate.repo.display());
    let _ = writeln!(out, "Маршрут: {} ({})", gate.route, gate.route_note);
    for c in &gate.components {
        let _ = writeln!(
            out,
            "  [{}] {} — {}",
            status_label(c.status),
            c.name,
            c.detail
        );
        for f in c.findings.iter().take(MAX_SECTION_FINDINGS) {
            let _ = writeln!(out, "      ↳ {f}");
        }
        if c.findings.len() > MAX_SECTION_FINDINGS {
            let _ = writeln!(
                out,
                "      ↳ … и ещё {} находок (полный список — командами составляющих)",
                c.findings.len() - MAX_SECTION_FINDINGS
            );
        }
    }
    let failed = gate
        .components
        .iter()
        .filter(|c| c.status == GateStatus::Fail)
        .count();
    let _ = writeln!(
        out,
        "Итог: {}",
        if gate.passed {
            "PASS".to_string()
        } else {
            format!("FAIL — провалено секций: {failed} (exit 1)")
        }
    );
    out
}

/// JSON-форма отчёта ревью (инструмент и `arch-be review --json`).
/// Находки секций ограничены [`MAX_SECTION_FINDINGS`] с счётчиком
/// `findings_total`.
#[must_use]
pub fn review_json(report: &ReviewReport) -> Value {
    let gate = &report.gate;
    let components: Vec<Value> = gate
        .components
        .iter()
        .map(|c| {
            json!({
                "name": c.name,
                "status": status_label(c.status),
                "detail": c.detail,
                "findings": c.findings.iter().take(MAX_SECTION_FINDINGS).collect::<Vec<_>>(),
                "findings_total": c.findings.len(),
            })
        })
        .collect();
    json!({
        "tool": "architect_review",
        "passed": gate.passed,
        "repo": gate.repo.display().to_string(),
        "route": gate.route.to_string(),
        "route_auto": gate.route_auto,
        "route_note": gate.route_note,
        "components": components,
        "summary": report.summary(),
    })
}

/// Затронутая сущность в отчёте [`change_impact`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AffectedEntity {
    /// Идентификатор (`CMP-001`).
    pub id: String,
    /// Тип (`cmp`, `sys`, … — `EntityKind::type_str`).
    pub kind: &'static str,
    /// Заголовок.
    pub title: String,
}

/// Правило CONSTRAINTS.yaml, затронутое изменением (через `verified_by`
/// достигнутых сущностей).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImpactRule {
    /// Идентификатор правила (`C-001`).
    pub id: String,
    /// Имя правила из карточки (`None` — карточка не найдена: правило
    /// известно только по ссылке).
    pub name: Option<String>,
    /// Владелец из карточки правила.
    pub owner: Option<String>,
}

/// Отчёт [`change_impact`] — «радиус взрыва» изменения.
#[derive(Debug)]
pub struct ImpactReport {
    /// Корень кейса (каталог с `model/`).
    pub case: PathBuf,
    /// Сущности-источники обхода (ID).
    pub seeds: Vec<String>,
    /// Пути из `paths`, не покрытые ни одним `code_roots` CMP (разрыв
    /// покрытия модели — сигнал дописать модель, ADR-030).
    pub gaps: Vec<String>,
    /// Затронутые сущности (транзитивно, включая источники), по типам и ID.
    pub affected: Vec<AffectedEntity>,
    /// Затронутые правила CONSTRAINTS.yaml (по `verified_by` C-NNN).
    pub rules: Vec<ImpactRule>,
    /// Контрактные файлы достигнутых INT (поле `contract`, ADR-035).
    pub contracts: Vec<String>,
    /// Достигнутые владельцы (`OWNER-* · заголовок`) — с кем согласовывать.
    pub owners: Vec<String>,
    /// Сводка одной строкой.
    pub summary: String,
}

/// Цель ссылки — правило CONSTRAINTS.yaml (`C-` + номер)? Дубль семантики
/// `trace::is_constraint_ref` (та приватна): ссылки на правила — единственный
/// не-сущностный вид целей `verified_by`.
fn is_constraint_ref(raw: &str) -> bool {
    let rest = raw.strip_prefix("C-");
    rest.is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

/// Нормализация пути для сопоставления с `code_roots`: срез `./`, `/` на
/// конце и дублирующиеся слэши.
fn normalize_path(raw: &str) -> String {
    let mut p = raw.trim().replace('\\', "/");
    while let Some(rest) = p.strip_prefix("./") {
        p = rest.to_string();
    }
    while p.ends_with('/') && p.len() > 1 {
        p.pop();
    }
    p
}

/// CMP, покрывающие путь `path` своими `code_roots` (ADR-030): корень —
/// префикс пути по границе сегмента. Побеждает самый длинный корень;
/// при равной длине — все CMP с ним.
fn cmps_for_path<'m>(model: &'m Model, path: &str) -> Vec<&'m str> {
    let mut best_len = 0usize;
    let mut out: Vec<&str> = Vec::new();
    for e in model.entities.iter().filter(|e| e.kind == EntityKind::Cmp) {
        for root in &e.code_roots {
            let root = normalize_path(root);
            if root.is_empty() {
                continue;
            }
            let covered = path == root || path.starts_with(&format!("{root}/"));
            if !covered {
                continue;
            }
            match root.len().cmp(&best_len) {
                std::cmp::Ordering::Greater => {
                    best_len = root.len();
                    out.clear();
                    out.push(&e.id);
                }
                std::cmp::Ordering::Equal => out.push(&e.id),
                std::cmp::Ordering::Less => {}
            }
        }
    }
    out.sort_unstable();
    out.dedup();
    out
}

/// Карточки правил CONSTRAINTS.yaml кейса: ключ (id, иначе name) →
/// (name, owner). Файла нет или не парсится — `None` (fail-soft: правила
/// остаются в отчёте голыми ссылками, имена/владельцы не резолвятся).
fn rule_cards(case: &Path) -> Option<BTreeMap<String, (String, Option<String>)>> {
    let path = case.join("CONSTRAINTS.yaml");
    if !path.is_file() {
        return None;
    }
    let rules = control::load_fitness_rules(&path).ok()?;
    let mut cards = BTreeMap::new();
    for rule in rules {
        let key = rule.id.clone().unwrap_or_else(|| rule.name.clone());
        cards.insert(key, (rule.name.clone(), rule.owner.clone()));
    }
    Some(cards)
}

/// Загружает модель кейса (`<case>/model/`) с честной ошибкой при
/// отсутствии каталога.
fn load_case_model(case: &Path) -> Result<Model> {
    let model_dir = case.join("model");
    if !model_dir.is_dir() {
        return Err(HarnessError::Model(format!(
            "нет каталога модели {} — обходу нечего читать",
            model_dir.display()
        )));
    }
    load_model(&model_dir)
}

/// Обход «что я задену»: от `id` (сущность модели) или `paths` (файлы →
/// CMP по `code_roots`) транзитивно по всем видам связей в обе стороны.
///
/// Семантика радиуса: меняя сущность, вы затрагиваете и тех, кто на неё
/// ссылается (обратные ссылки), и тех, на кого ссылается она (зависимости,
/// инварианты, владельцы) — ненаправленный обход по рёбрам `depends_on` /
/// `implements` / `affects` / `verified_by` между сущностями.
///
/// # Errors
/// Нет каталога `model/`; модель не разбирается; не заданы ни `id`, ни
/// `paths`; `id` неизвестен модели.
pub fn change_impact(case: &Path, id: Option<&str>, paths: &[String]) -> Result<ImpactReport> {
    let model =
        load_case_model(case).map_err(|e| HarnessError::Model(format!("change_impact: {e}")))?;
    if id.is_none() && paths.is_empty() {
        return Err(HarnessError::Model(
            "change_impact: задайте источник — id сущности или paths файлов".to_string(),
        ));
    }

    let mut seeds: BTreeSet<String> = BTreeSet::new();
    let mut gaps: Vec<String> = Vec::new();
    if let Some(raw_id) = id {
        let clean = raw_id.trim();
        if model.get(clean).is_none() {
            return Err(HarnessError::Model(format!(
                "change_impact: сущность '{clean}' не найдена (всего сущностей: {})",
                model.entities.len()
            )));
        }
        seeds.insert(clean.to_string());
    }
    for raw in paths {
        let norm = normalize_path(raw);
        let cmps = cmps_for_path(&model, &norm);
        if cmps.is_empty() {
            gaps.push(norm);
        } else {
            seeds.extend(cmps.into_iter().map(str::to_string));
        }
    }
    if seeds.is_empty() {
        return Err(HarnessError::Model(format!(
            "change_impact: ни один путь не покрыт code_roots CMP — источников нет \
             (gaps: {})",
            gaps.join(", ")
        )));
    }
    Ok(impact_core(case, &model, seeds, gaps))
}

/// Обход от нескольких сущностей-источников сразу (используется связкой
/// `contract_diff --model`: все INT, чьё поле `contract` совпало с путём
/// диффа — ломающее изменение сразу возвращает потребителей и владельцев).
///
/// # Errors
/// Нет каталога `model/`; модель не разбирается; список пуст; какой-то из
/// ID неизвестен модели.
pub fn impact_from_ids(case: &Path, ids: &[String]) -> Result<ImpactReport> {
    let model =
        load_case_model(case).map_err(|e| HarnessError::Model(format!("impact_from_ids: {e}")))?;
    if ids.is_empty() {
        return Err(HarnessError::Model(
            "impact_from_ids: пустой список источников".to_string(),
        ));
    }
    let mut seeds: BTreeSet<String> = BTreeSet::new();
    for raw in ids {
        let clean = raw.trim();
        if model.get(clean).is_none() {
            return Err(HarnessError::Model(format!(
                "impact_from_ids: сущность '{clean}' не найдена (всего сущностей: {})",
                model.entities.len()
            )));
        }
        seeds.insert(clean.to_string());
    }
    Ok(impact_core(case, &model, seeds, Vec::new()))
}

/// Ядро обхода: BFS от `seeds` по ненаправленной смежности модели + сборка
/// отчёта (сущности по типам, правила C-NNN, контракты INT, владельцы).
fn impact_core(
    case: &Path,
    model: &Model,
    seeds: BTreeSet<String>,
    gaps: Vec<String>,
) -> ImpactReport {
    // Ненаправленная смежность: исходящие связи + обратные ссылки, только
    // между существующими сущностями (битые ссылки — забота validate).
    let mut adjacency: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for e in &model.entities {
        for kind in LinkKind::ALL {
            for target in e.link_targets(kind) {
                if model.get(target).is_none() {
                    continue;
                }
                adjacency.entry(e.id.as_str()).or_default().insert(target);
                adjacency
                    .entry(target.as_str())
                    .or_default()
                    .insert(e.id.as_str());
            }
        }
    }
    // BFS от источников.
    let mut reached: BTreeSet<String> = seeds.clone();
    let mut queue: VecDeque<String> = seeds.iter().cloned().collect();
    while let Some(cur) = queue.pop_front() {
        let Some(neighbors) = adjacency.get(cur.as_str()) else {
            continue;
        };
        for next in neighbors {
            if reached.insert((*next).to_string()) {
                queue.push_back((*next).to_string());
            }
        }
    }

    // Затронутые сущности: стабильный порядок (тип, затем ID).
    let mut affected: Vec<AffectedEntity> = reached
        .iter()
        .filter_map(|rid| model.get(rid))
        .map(|e| AffectedEntity {
            id: e.id.clone(),
            kind: e.kind.type_str(),
            title: e.title.clone(),
        })
        .collect();
    affected.sort_by(|a, b| {
        let ka = EntityKind::from_type_str(a.kind);
        let kb = EntityKind::from_type_str(b.kind);
        ka.cmp(&kb).then_with(|| a.id.cmp(&b.id))
    });

    // Правила: C-NNN из verified_by достигнутых сущностей + карточки.
    let cards = rule_cards(case);
    let mut rules: Vec<ImpactRule> = Vec::new();
    let mut rule_ids: BTreeSet<String> = BTreeSet::new();
    for rid in &reached {
        let Some(e) = model.get(rid) else {
            continue;
        };
        for target in &e.verified_by {
            if is_constraint_ref(target) {
                rule_ids.insert(target.clone());
            }
        }
    }
    for rid in rule_ids {
        let card = cards.as_ref().and_then(|c| c.get(&rid));
        rules.push(ImpactRule {
            id: rid,
            name: card.map(|(name, _)| name.clone()),
            owner: card.and_then(|(_, owner)| owner.clone()),
        });
    }

    // Контракты достигнутых INT (поле contract, ADR-035).
    let contracts: Vec<String> = reached
        .iter()
        .filter_map(|rid| model.get(rid))
        .filter(|e| e.kind == EntityKind::Int)
        .filter_map(|e| {
            e.contract
                .as_deref()
                .map(str::trim)
                .filter(|c| !c.is_empty())
                .map(str::to_string)
        })
        .collect();

    // Владельцы: достигнутые сущности OWNER.
    let owners: Vec<String> = affected
        .iter()
        .filter(|a| a.kind == EntityKind::Owner.type_str())
        .map(|a| format!("{} · {}", a.id, a.title))
        .collect();

    let mut by_kind: BTreeMap<&str, usize> = BTreeMap::new();
    for a in &affected {
        *by_kind.entry(a.kind).or_default() += 1;
    }
    let kinds_text = by_kind
        .iter()
        .map(|(k, n)| format!("{k}: {n}"))
        .collect::<Vec<_>>()
        .join(", ");
    let mut summary = format!(
        "Изменение «{}» затрагивает сущностей: {} ({kinds_text}); правил: {}; контрактов: {}; владельцев: {}",
        seeds.iter().cloned().collect::<Vec<_>>().join(", "),
        affected.len(),
        rules.len(),
        contracts.len(),
        owners.len()
    );
    if !gaps.is_empty() {
        let _ = write!(summary, "; путей без CMP-покрытия (gap): {}", gaps.len());
    }
    ImpactReport {
        case: case.to_path_buf(),
        seeds: seeds.into_iter().collect(),
        gaps,
        affected,
        rules,
        contracts,
        owners,
        summary,
    }
}

/// Текстовый рендер отчёта обхода: источники, gaps, сущности по типам,
/// правила (с владельцами), контракты, «с кем согласовывать».
#[must_use]
pub fn render_impact(report: &ImpactReport) -> String {
    let mut out = String::new();
    // Запись в String не может завершиться ошибкой — игноры безопасны.
    let _ = writeln!(out, "Радиус изменения: {}", report.case.display());
    let _ = writeln!(out, "Источники: {}", report.seeds.join(", "));
    if !report.gaps.is_empty() {
        let _ = writeln!(
            out,
            "Пути без CMP-покрытия (gap — допишите code_roots в модель, ADR-030):"
        );
        for g in &report.gaps {
            let _ = writeln!(out, "  {g}");
        }
    }
    let _ = writeln!(out, "Затронутые сущности ({}):", report.affected.len());
    for a in &report.affected {
        let _ = writeln!(out, "  {:<10} {:<5} {}", a.id, a.kind, a.title);
    }
    if !report.rules.is_empty() {
        let _ = writeln!(out, "Правила CONSTRAINTS.yaml:");
        for r in &report.rules {
            let name = r.name.as_deref().unwrap_or("?");
            match &r.owner {
                Some(owner) => {
                    let _ = writeln!(out, "  {} ({name}; владелец: {owner})", r.id);
                }
                None => {
                    let _ = writeln!(out, "  {} ({name})", r.id);
                }
            }
        }
    }
    if !report.contracts.is_empty() {
        let _ = writeln!(out, "Контракты INT (проверьте эволюцию — contract_diff):");
        for c in &report.contracts {
            let _ = writeln!(out, "  {c}");
        }
    }
    if !report.owners.is_empty() {
        let _ = writeln!(out, "Согласовать с владельцами:");
        for o in &report.owners {
            let _ = writeln!(out, "  {o}");
        }
    }
    let _ = writeln!(out, "Итог: {}", report.summary);
    out
}

/// JSON-форма отчёта обхода (инструмент и `arch-be model impact --json`).
#[must_use]
pub fn impact_json(report: &ImpactReport) -> Value {
    json!({
        "tool": "change_impact",
        "case": report.case.display().to_string(),
        "seeds": report.seeds,
        "gaps": report.gaps,
        "affected": report.affected.iter().map(|a| json!({
            "id": a.id,
            "kind": a.kind,
            "title": a.title,
        })).collect::<Vec<_>>(),
        "rules": report.rules.iter().map(|r| json!({
            "id": r.id,
            "name": r.name,
            "owner": r.owner,
        })).collect::<Vec<_>>(),
        "contracts": report.contracts,
        "owners": report.owners,
        "summary": report.summary,
    })
}

/// Инструменты домена: `architect_review`, `change_impact`.
#[must_use]
pub fn tools() -> Vec<Arc<dyn Tool>> {
    vec![Arc::new(ArchitectReviewTool), Arc::new(ChangeImpactTool)]
}

/// Инструмент `architect_review`: составное ревью репозитория одним вызовом
/// (мост в MCP, транш 3 инверсии; read-only).
pub struct ArchitectReviewTool;

#[derive(Debug, Deserialize)]
struct ArchitectReviewArgs {
    /// Репозиторий (дефолт — текущий каталог).
    path: Option<String>,
    /// База git для диффа (дефолт — рабочее дерево против HEAD).
    base: Option<String>,
}

#[async_trait]
impl Tool for ArchitectReviewTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "architect_review".into(),
            description: "Составное архитектурное ревью (review) репозитория одним вызовом: \
                          маршрут значимости из git-диффа + весь контур контроля (fitness \
                          CONSTRAINTS.yaml, гейт правок спайна, анти-ослабление правил, линт \
                          spine, трассировка; на Standard/Critical — NFR и evidence) + \
                          целостность модели + линт контрактов OpenAPI/AsyncAPI. Ответ — JSON: \
                          passed + route + components (секции со статусами PASS/FAIL/SKIP и \
                          находками) + summary; passed=false — основание отказать изменению"
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Репозиторий (по умолчанию текущий каталог)"},
                    "base": {"type": "string", "description": "База git для диффа (по умолчанию — рабочее дерево против HEAD; для CI — напр. origin/main...HEAD)"}
                }
            }),
        }
    }

    async fn call(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let args: ArchitectReviewArgs = match serde_json::from_value(args) {
            Ok(a) => a,
            Err(e) => {
                return Ok(ToolOutput::err(format!(
                    "architect_review: невалидные аргументы: {e}"
                )));
            }
        };
        let repo = ctx.resolve(args.path.as_deref().unwrap_or("."));
        let limits = match ctx.config.significance.limits() {
            Ok(l) => l,
            Err(e) => {
                return Ok(ToolOutput::err(format!(
                    "architect_review: пороги маршрутов: {e}"
                )));
            }
        };
        let report = match architect_review(&repo, args.base.as_deref(), None, limits) {
            Ok(r) => r,
            Err(e) => return Ok(ToolOutput::err(format!("architect_review: {e}"))),
        };
        let verdict = review_json(&report);
        // Сериализация собранного объекта не падает; запасной вариант — компактная форма.
        let text = serde_json::to_string_pretty(&verdict).unwrap_or_else(|_| verdict.to_string());
        Ok(ToolOutput::ok(text))
    }
}

/// Инструмент `change_impact`: радиус взрыва изменения по графу модели
/// (мост в MCP, транш 3 инверсии; read-only).
pub struct ChangeImpactTool;

#[derive(Debug, Deserialize)]
struct ChangeImpactArgs {
    /// Корень кейса (каталог с `model/`, дефолт — текущий каталог).
    path: Option<String>,
    /// ID сущности-источника.
    id: Option<String>,
    /// Файлы изменения (источники — CMP по `code_roots`).
    paths: Option<Vec<String>>,
}

#[async_trait]
impl Tool for ChangeImpactTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "change_impact".into(),
            description: "Что заденет изменение и с кем согласовывать: от сущности (id) или \
                          файлов (paths → компоненты по code_roots) транзитивный обход графа \
                          связей модели → затронутые CMP/SYS/INT/AD/ADR/NFR/QAS, правила \
                          CONSTRAINTS.yaml (C-NNN с владельцами), контракты интеграций, \
                          владельцы OWNER. Ответ — JSON: seeds + affected (по типам) + rules + \
                          contracts + owners + gaps (пути без покрытия моделью) + summary; \
                          отчёт, а не гейт — passed не применим"
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Корень кейса (каталог с model/; по умолчанию текущий каталог)"},
                    "id": {"type": "string", "description": "ID сущности-источника (CMP-001, INT-002, …)"},
                    "paths": {"type": "array", "items": {"type": "string"}, "description": "Файлы изменения (источники — CMP по code_roots, ADR-030)"}
                }
            }),
        }
    }

    async fn call(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let args: ChangeImpactArgs = match serde_json::from_value(args) {
            Ok(a) => a,
            Err(e) => {
                return Ok(ToolOutput::err(format!(
                    "change_impact: невалидные аргументы: {e}"
                )));
            }
        };
        let case = ctx.resolve(args.path.as_deref().unwrap_or("."));
        let paths = args.paths.unwrap_or_default();
        let report = match change_impact(&case, args.id.as_deref(), &paths) {
            Ok(r) => r,
            Err(e) => return Ok(ToolOutput::err(format!("change_impact: {e}"))),
        };
        let verdict = impact_json(&report);
        // Сериализация собранного объекта не падает; запасной вариант — компактная форма.
        let text = serde_json::to_string_pretty(&verdict).unwrap_or_else(|_| verdict.to_string());
        Ok(ToolOutput::ok(text))
    }
}

#[cfg(test)]
mod tests {
    use std::process::Command;

    use serde_json::json;

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

    /// Полный кейс-фикстура: git-репо (один коммит, чистое дерево → маршрут
    /// Fast), model/ с цепочкой CMP→INT→SYS→AD→OWNER (+ REQ/NFR/ADR/QAS/CMP-2),
    /// корневой CONSTRAINTS.yaml (карточка C-001 с владельцем), spine с AD-1,
    /// контракт contracts/api.yaml и .arch-handoff/CONSTRAINTS.yaml
    /// (`file_exists` на спайн). Все звенья покрыты — ревью зелёное.
    fn make_case(dir: &Path) {
        let model = dir.join("model");
        std::fs::create_dir_all(&model).expect("mkdir model");
        let entities = [
            (
                "REQ-001.md",
                "---\nid: REQ-001\ntype: req\ntitle: Приём платежа\nstatus: accepted\n---\n\nТело.\n",
            ),
            (
                "NFR-001.md",
                "---\nid: NFR-001\ntype: nfr\ntitle: Латентность\nstatus: accepted\nverification: histogram\naffects: [CMP-001]\n---\n\nТело.\n",
            ),
            (
                "AD-1.md",
                "---\nid: AD-1\ntype: ad\ntitle: Точные деньги\nstatus: ADOPTED\nverified_by: [C-001]\n---\n\nПравило.\n",
            ),
            (
                "ADR-001.md",
                "---\nid: ADR-001\ntype: adr\ntitle: Minor units\nstatus: Accepted\naffects: [CMP-001]\nimplements: [AD-1]\n---\n\nТело.\n",
            ),
            (
                "CMP-001.md",
                "---\nid: CMP-001\ntype: cmp\ntitle: Платёжный шлюз\nstatus: designed\nimplements: [AD-1, REQ-001]\ndepends_on: [INT-001]\ncode_roots: [services/pay]\n---\n\nТело.\n",
            ),
            (
                "CMP-002.md",
                "---\nid: CMP-002\ntype: cmp\ntitle: Витрина\nstatus: designed\nimplements: [AD-1]\ndepends_on: [CMP-001]\n---\n\nТело.\n",
            ),
            (
                "INT-001.md",
                "---\nid: INT-001\ntype: int\ntitle: Рельс процессинга\nstatus: accepted\ndepends_on: [SYS-001]\ncontract: contracts/api.yaml\n---\n\nТело.\n",
            ),
            (
                "SYS-001.md",
                "---\nid: SYS-001\ntype: sys\ntitle: Процессинг\nstatus: active\naffects: [OWNER-1]\n---\n\nТело.\n",
            ),
            (
                "OWNER-1.md",
                "---\nid: OWNER-1\ntype: owner\ntitle: Команда процессинга\nstatus: active\n---\n\nТело.\n",
            ),
            (
                "QAS-001.md",
                "---\nid: QAS-001\ntype: qas\ntitle: Пик нагрузки\nstatus: accepted\nimplements: [NFR-001]\nsource: канал\nstimulus: пик 5000 TPS\nartifact: шлюз\nresponse: ответ\nmeasure: p99 < 2000 мс\n---\n\nТело.\n",
            ),
        ];
        for (name, text) in entities {
            std::fs::write(model.join(name), text).expect("сущность");
        }
        std::fs::write(
            dir.join("CONSTRAINTS.yaml"),
            "constraints:\n  - id: C-001\n    name: no_float_money\n    owner: Команда платежей\n",
        )
        .expect("constraints");
        std::fs::write(
            dir.join("ARCHITECTURE-SPINE.md"),
            "# Spine\n\n### AD-1. Точные деньги\n- Binds: денежные суммы\n- Prevents: потеря копеек\n- Rule: суммы в minor units\n",
        )
        .expect("spine");
        let contracts = dir.join("contracts");
        std::fs::create_dir_all(&contracts).expect("mkdir contracts");
        std::fs::write(
            contracts.join("api.yaml"),
            "openapi: 3.0.3\ninfo:\n  title: Processing API\n  version: 1.0.0\npaths:\n  /v1/charges:\n    get:\n      operationId: listCharges\n      responses:\n        '200':\n          description: ok\n",
        )
        .expect("контракт");
        std::fs::create_dir_all(dir.join(".arch-handoff")).expect("mkdir handoff");
        std::fs::write(
            dir.join(".arch-handoff/CONSTRAINTS.yaml"),
            "rules:\n  - name: spine_present\n    type: file_exists\n    path: \"ARCHITECTURE-SPINE.md\"\n    severity: error\n",
        )
        .expect("handoff constraints");
        git(dir, &["init", "-q"]);
        git(dir, &["add", "."]);
        git(dir, &["commit", "-q", "-m", "init"]);
    }

    #[test]
    fn review_passes_on_clean_case_and_covers_all_sections() {
        let tmp = tempfile::tempdir().expect("tmp");
        let repo = tmp.path().join("case");
        std::fs::create_dir_all(&repo).expect("mkdir");
        make_case(&repo);
        let report = architect_review(&repo, None, None, (1, 4)).expect("ревью");
        assert!(report.gate.passed, "{}", render_review(&report));
        let names: Vec<&str> = report.gate.components.iter().map(|c| c.name).collect();
        // Контур гейта + две новые секции в конце.
        for want in [
            "fitness",
            "delta_guard",
            "rule_weakened",
            "spine_lint",
            "trace_check",
            "model_validate",
            "contracts",
        ] {
            assert!(names.contains(&want), "нет секции {want}: {names:?}");
        }
        for c in &report.gate.components {
            assert_ne!(
                c.status,
                GateStatus::Fail,
                "{}: {}",
                c.name,
                render_review(&report)
            );
        }
        // model/ и контракт есть — обе новые секции реально прогнались.
        let model = report
            .gate
            .components
            .iter()
            .find(|c| c.name == "model_validate")
            .expect("секция");
        assert_eq!(model.status, GateStatus::Pass);
        let contracts = report
            .gate
            .components
            .iter()
            .find(|c| c.name == "contracts")
            .expect("секция");
        assert_eq!(contracts.status, GateStatus::Pass, "{}", contracts.detail);
        assert!(
            contracts.detail.contains("openapi: 1"),
            "{}",
            contracts.detail
        );
        let text = render_review(&report);
        assert!(text.contains("Итог: PASS"), "{text}");
        assert!(text.contains("Ревью:"), "{text}");
        let json = review_json(&report);
        assert_eq!(json["passed"], true, "{json}");
        assert_eq!(json["route"], "Fast", "{json}");
    }

    #[test]
    fn review_fails_on_broken_model_link() {
        let tmp = tempfile::tempdir().expect("tmp");
        let repo = tmp.path().join("case");
        std::fs::create_dir_all(&repo).expect("mkdir");
        make_case(&repo);
        // ADR ссылается на несуществующий CMP → model_validate FAIL, а с ним
        // и всё ревью (trace тоже упадёт на orphan-ADR — warn, не error).
        std::fs::write(
            repo.join("model/ADR-002-bad.md"),
            "---\nid: ADR-002\ntype: adr\ntitle: Битое\nstatus: Accepted\naffects: [CMP-999]\n---\n\nТело.\n",
        )
        .expect("битый ADR");
        let report = architect_review(&repo, None, None, (1, 4)).expect("ревью");
        assert!(!report.gate.passed, "{}", render_review(&report));
        let model = report
            .gate
            .components
            .iter()
            .find(|c| c.name == "model_validate")
            .expect("секция");
        assert_eq!(model.status, GateStatus::Fail);
        assert!(
            model
                .findings
                .iter()
                .any(|f| f.rule.as_deref() == Some("broken-link")),
            "{:?}",
            model.findings
        );
        let text = render_review(&report);
        assert!(text.contains("Итог: FAIL"), "{text}");
    }

    #[test]
    fn review_fails_on_lint_error_of_declared_contract() {
        let tmp = tempfile::tempdir().expect("tmp");
        let repo = tmp.path().join("case");
        std::fs::create_dir_all(&repo).expect("mkdir");
        make_case(&repo);
        // Контракт без версии (OA-001) и без /vN-префикса пути (OA-002).
        std::fs::write(
            repo.join("contracts/api.yaml"),
            "openapi: 3.0.3\ninfo:\n  title: Processing API\n  version: \"1\"\npaths:\n  /charges:\n    get:\n      operationId: list\n      responses:\n        '200':\n          description: ok\n",
        )
        .expect("контракт");
        let report = architect_review(&repo, None, None, (1, 4)).expect("ревью");
        assert!(!report.gate.passed, "{}", render_review(&report));
        let contracts = report
            .gate
            .components
            .iter()
            .find(|c| c.name == "contracts")
            .expect("секция");
        assert_eq!(contracts.status, GateStatus::Fail);
        assert!(
            contracts
                .findings
                .iter()
                .any(|f| f.rule.as_deref() == Some("OA-001")),
            "{:?}",
            contracts.findings
        );
    }

    #[test]
    fn impact_from_id_reaches_chain_and_collects_everything() {
        let tmp = tempfile::tempdir().expect("tmp");
        let case = tmp.path().join("case");
        std::fs::create_dir_all(&case).expect("mkdir");
        make_case(&case);
        let report = change_impact(&case, Some("CMP-001"), &[]).expect("impact");
        assert_eq!(report.seeds, vec!["CMP-001"]);
        // Вся цепочка в обе стороны: CMP-001 → AD-1/REQ-001/INT-001 →
        // SYS-001 → OWNER-1; обратно: NFR-001/ADR-001/CMP-002; QAS через NFR.
        let ids: Vec<&str> = report.affected.iter().map(|a| a.id.as_str()).collect();
        for want in [
            "CMP-001", "CMP-002", "INT-001", "SYS-001", "AD-1", "ADR-001", "REQ-001", "NFR-001",
            "QAS-001", "OWNER-1",
        ] {
            assert!(ids.contains(&want), "нет {want} в {ids:?}");
        }
        assert_eq!(report.affected.len(), 10, "{ids:?}");
        // Правило C-001 из verified_by AD-1, с картой владельца.
        assert_eq!(report.rules.len(), 1);
        assert_eq!(report.rules[0].id, "C-001");
        assert_eq!(report.rules[0].name.as_deref(), Some("no_float_money"));
        assert_eq!(report.rules[0].owner.as_deref(), Some("Команда платежей"));
        assert_eq!(report.contracts, vec!["contracts/api.yaml"]);
        assert_eq!(
            report.owners,
            vec!["OWNER-1 · Команда процессинга".to_string()]
        );
        let text = render_impact(&report);
        assert!(text.contains("Согласовать с владельцами"), "{text}");
        assert!(text.contains("C-001 (no_float_money"), "{text}");
        let json = impact_json(&report);
        assert_eq!(json["affected"].as_array().expect("affected").len(), 10);
    }

    #[test]
    fn impact_from_paths_maps_code_roots_and_marks_gaps() {
        let tmp = tempfile::tempdir().expect("tmp");
        let case = tmp.path().join("case");
        std::fs::create_dir_all(&case).expect("mkdir");
        make_case(&case);
        let paths = vec![
            "services/pay/src/main.rs".to_string(),
            "./services/pay/Cargo.toml".to_string(),
            "docs/notes.md".to_string(),
        ];
        let report = change_impact(&case, None, &paths).expect("impact");
        assert_eq!(report.seeds, vec!["CMP-001"]);
        assert_eq!(report.gaps, vec!["docs/notes.md"]);
        assert!(report.summary.contains("gap"), "{}", report.summary);
        let text = render_impact(&report);
        assert!(text.contains("docs/notes.md"), "{text}");
    }

    #[test]
    fn impact_unknown_id_and_empty_source_are_honest_errors() {
        let tmp = tempfile::tempdir().expect("tmp");
        let case = tmp.path().join("case");
        std::fs::create_dir_all(&case).expect("mkdir");
        make_case(&case);
        let err = change_impact(&case, Some("CMP-999"), &[]).expect_err("неизвестный id");
        assert!(err.to_string().contains("не найдена"), "{err}");
        let err = change_impact(&case, None, &[]).expect_err("нет источника");
        assert!(err.to_string().contains("id сущности или paths"), "{err}");
        // Пути только-gap — тоже ошибка (источников нет).
        let err =
            change_impact(&case, None, &["docs/x.md".to_string()]).expect_err("все пути — gap");
        assert!(err.to_string().contains("code_roots"), "{err}");
        // Нет model/ — честная ошибка, не паника.
        let empty = tmp.path().join("empty");
        std::fs::create_dir_all(&empty).expect("mkdir");
        assert!(change_impact(&empty, Some("CMP-001"), &[]).is_err());
    }

    #[tokio::test]
    async fn tools_review_and_impact_over_registry_shape() {
        let tmp = tempfile::tempdir().expect("tmp");
        let case = tmp.path().join("case");
        std::fs::create_dir_all(&case).expect("mkdir");
        make_case(&case);
        let ctx = ToolContext::new(
            tmp.path().to_path_buf(),
            Arc::new(crate::config::Config::default()),
        );
        let out = ArchitectReviewTool
            .call(json!({"path": "case"}), &ctx)
            .await
            .expect("вызов");
        assert!(!out.is_error, "{}", out.content);
        let v: Value = serde_json::from_str(&out.content).expect("JSON-вердикт");
        assert_eq!(v["tool"], "architect_review");
        assert_eq!(v["passed"], true, "{v}");
        let names: Vec<&str> = v["components"]
            .as_array()
            .expect("components")
            .iter()
            .filter_map(|c| c["name"].as_str())
            .collect();
        assert!(names.contains(&"contracts"), "{names:?}");
        assert!(names.contains(&"model_validate"), "{names:?}");

        let out = ChangeImpactTool
            .call(json!({"path": "case", "id": "INT-001"}), &ctx)
            .await
            .expect("вызов");
        assert!(!out.is_error, "{}", out.content);
        let v: Value = serde_json::from_str(&out.content).expect("JSON");
        assert_eq!(v["tool"], "change_impact");
        assert_eq!(v["contracts"], json!(["contracts/api.yaml"]), "{v}");
        // Из INT-001 радиус добирается до всей модели (связный граф).
        assert_eq!(v["affected"].as_array().expect("affected").len(), 10);

        // Мягкие ошибки: неизвестный id, кейс без модели.
        let out = ChangeImpactTool
            .call(json!({"path": "case", "id": "CMP-999"}), &ctx)
            .await
            .expect("вызов");
        assert!(out.is_error, "{}", out.content);
        let out = ChangeImpactTool
            .call(json!({"path": ".", "id": "CMP-001"}), &ctx)
            .await
            .expect("вызов");
        assert!(out.is_error, "{}", out.content);
        // Битые аргументы.
        let out = ArchitectReviewTool
            .call(json!({"path": 42}), &ctx)
            .await
            .expect("вызов");
        assert!(out.is_error, "{}", out.content);
    }
}
