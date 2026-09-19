//! Дрейф «модель ↔ код»: согласованность типизированной модели с
//! репозиторием, в котором она живёт (бэклог «дрейф модели и импорт
//! реестров», волна 3).
//!
//! КОНТРАКТ (владелец: агент `model`):
//! - вход — КОРЕНЬ КЕЙСА (каталог с `model/` внутри, как у `trace check`);
//!   `code_roots` CMP (ADR-030) и `contract` INT (ADR-035) резолвятся от
//!   него же — та же точка резолва, что у `control check`/`trace check`;
//! - находки — [`LintIssue`] (SDK-контракт находок с `ad`/`adr`/`rationale`/
//!   `fix_hint`), коды и критичность звена `INT → контракт` ДОСЛОВНО
//!   повторяют `trace check` (ADR-035), чтобы отчёты инструментов не
//!   расходились;
//! - проверки: `code-root-missing` (error — `code_roots` CMP указывает на
//!   несуществующий путь: модель оторвалась от кода); `uncovered-manifest`
//!   (warn — каталог с манифестом сборки не покрыт ни одним `code_roots`:
//!   код без компонента в модели; набор манифестов — тот же, что у сканера
//!   [`crate::survey`]); звено `INT → контракт` (`int-contract-missing` —
//!   error, `int-without-contract` — warn, ADR-035);
//! - read-only: ничего не пишет, не требует git и сети; инструмент агента
//!   `model_drift` (мост в MCP, read-only), CLI — `arch-be model drift`.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use walkdir::WalkDir;

use crate::control::LintIssue;
use crate::error::Result;
use crate::llm::ToolSpec;
use crate::model::{EntityKind, load_model_tolerant};
use crate::tool::{Tool, ToolContext, ToolOutput};

/// Отчёт проверки дрейфа «модель ↔ код».
#[derive(Debug)]
pub struct DriftReport {
    /// Корень кейса (каталог, из которого прочитана `model/`).
    pub case: PathBuf,
    /// Сущностей в модели.
    pub entities: usize,
    /// CMP с непустым `code_roots`.
    pub components_with_roots: usize,
    /// Каталогов с манифестами сборки, найденных в репозитории.
    pub manifest_dirs: usize,
    /// Находки (отсортированы: file, rule).
    pub issues: Vec<LintIssue>,
}

impl DriftReport {
    /// Есть ли блокирующие находки (error → exit code 1 в CLI).
    #[must_use]
    pub fn has_errors(&self) -> bool {
        self.issues.iter().any(|i| i.severity == "error")
    }

    /// Строка сводки для CLI/инструмента.
    #[must_use]
    pub fn summary(&self) -> String {
        let errors = self.issues.iter().filter(|i| i.severity == "error").count();
        let warns = self.issues.len() - errors;
        format!(
            "Кейс {}: сущностей: {}, CMP с code_roots: {}, каталогов с манифестами: {}, находок: {} (error: {errors}, warn: {warns})",
            self.case.display(),
            self.entities,
            self.components_with_roots,
            self.manifest_dirs,
            self.issues.len(),
        )
    }
}

/// Нормализация `code_roots` в сопоставимую с относительным путём форму:
/// та же обрезка `./`/замыкающего `/`, что в `control::normalize_root`
/// (приватна — дублируется осознанно, чтобы `model` не зависел от
/// внутренностей `control`); дополнительно «.» и «./» схлопываются в
/// пустую строку — обозначение корня репозитория.
fn normalize_root(root: &str) -> String {
    let normalized = root.trim().trim_start_matches("./").trim_end_matches('/');
    if normalized == "." {
        String::new()
    } else {
        normalized.to_string()
    }
}

/// Покрывает ли корень `root` каталог `dir` (префикс по границе сегмента;
/// семантика `module_prefix_match` из `control.rs`). Пустой корень —
/// корень репозитория — покрывает всё.
fn root_covers(root: &str, dir: &str) -> bool {
    root.is_empty()
        || dir == root
        || dir
            .strip_prefix(root)
            .is_some_and(|rest| rest.starts_with('/'))
}

/// Каталоги репозитория, содержащие манифест сборки (тот же набор имён,
/// что у сканера обследования [`crate::survey::is_manifest_file`]; тот же
/// контур обхода — без dot-каталогов и служебных [`crate::survey::SKIP_DIRS`]).
/// Возвращает относительные пути каталогов (`""` — манифест в корне).
fn manifest_dirs(case_dir: &Path) -> Result<BTreeSet<String>> {
    let mut dirs = BTreeSet::new();
    let walker = WalkDir::new(case_dir).follow_links(false).into_iter();
    for entry in walker.filter_entry(|e| {
        // Корень обхода (глубина 0) пропускаем всегда: при вызове с «.»
        // его file_name — точка, и фильтр dot-каталогов иначе обрезает весь
        // обход.
        if e.depth() > 0 && e.file_type().is_dir() {
            let name = e.file_name().to_string_lossy();
            !(name.starts_with('.') || crate::survey::SKIP_DIRS.contains(&name.as_ref()))
        } else {
            true
        }
    }) {
        let entry = entry.map_err(|e| {
            crate::error::HarnessError::Model(format!("обход {}: {e}", case_dir.display()))
        })?;
        if !entry.file_type().is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy();
        if !crate::survey::is_manifest_file(&name) {
            continue;
        }
        let rel = entry
            .path()
            .parent()
            .and_then(|p| p.strip_prefix(case_dir).ok())
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .unwrap_or_default();
        dirs.insert(rel);
    }
    Ok(dirs)
}

/// Непустое значение поля `contract` (пустая/пробельная строка =
/// отсутствует; то же правило, что в `trace::contract_path`).
fn contract_path(e: &crate::model::Entity) -> Option<&str> {
    e.contract
        .as_deref()
        .map(str::trim)
        .filter(|c| !c.is_empty())
}

/// Карточка правила дрейфа: архитектурный контекст находки (поля
/// [`LintIssue`]: `adr`, `rationale`, `fix_hint`).
struct RuleCard {
    adr: &'static str,
    rationale: &'static str,
    fix_hint: &'static str,
}

/// Карточка по коду правила.
fn rule_card(rule: &str) -> RuleCard {
    match rule {
        "code-root-missing" => RuleCard {
            adr: "ADR-030",
            rationale: "code_roots — граница контекста для правила context_boundary; \
                        несуществующий путь означает, что модель оторвалась от кода",
            fix_hint: "исправьте путь в code_roots (переименование/перенос каталога), \
                       создайте каталог либо удалите корень из модели",
        },
        "uncovered-manifest" => RuleCard {
            adr: "ADR-030",
            rationale: "каталог с манифестом — корень компонента кода; без CMP с покрывающим \
                        code_roots он невидим для context_boundary и дрейфует незамеченным",
            fix_hint: "добавьте CMP с code_roots, покрывающим этот каталог, либо осознанно \
                       оставьте каталог вне модели (warn останется напоминанием)",
        },
        "int-contract-missing" => RuleCard {
            adr: "ADR-035",
            rationale: "заявленный контракт потерян — разрыв трассировки \
                        «интеграция → контракт»",
            fix_hint: "восстановите контрактный артефакт по этому пути либо поправьте \
                       поле contract",
        },
        _ => RuleCard {
            adr: "ADR-035",
            rationale: "внешняя интеграция без артефакта в репозитории легитимна, но обязана \
                        быть видимой в отчёте",
            fix_hint: "задайте contract: <путь к openapi/asyncapi-артефакту> либо оставьте warn \
                       как осознанный пробел",
        },
    }
}

/// Добавляет находку дрейфа (контекст правила — из карточки [`rule_card`]).
fn push_issue(
    issues: &mut Vec<LintIssue>,
    severity: &str,
    file: PathBuf,
    rule: &str,
    message: String,
) {
    let card = rule_card(rule);
    issues.push(LintIssue {
        file,
        line: 0,
        rule: rule.to_string(),
        message,
        severity: severity.to_string(),
        adr: Some(card.adr.to_string()),
        rationale: Some(card.rationale.to_string()),
        fix_hint: Some(card.fix_hint.to_string()),
        ..LintIssue::default()
    });
}

/// Проверяет дрейф «модель ↔ код» для кейса `case_dir`.
///
/// Толерантная загрузка модели (E3): битые сущности — warn-находки
/// `model-load-skip`, проверки идут по валидному подмножеству; полный отказ
/// — только когда не загрузилось ничего.
///
/// # Errors
/// Каталог недоступен, `model/` внутри него отсутствует, ни один файл
/// модели не разбирается, ошибка обхода репозитория.
pub fn drift_check(case_dir: &Path) -> Result<DriftReport> {
    let model_dir = case_dir.join("model");
    let model = load_model_tolerant(&model_dir)?;
    let mut issues = Vec::new();
    // E3: пропущенные при загрузке сущности — warn-находки, дрейф считается
    // по валидному подмножеству.
    for li in &model.load_issues {
        issues.push(LintIssue {
            file: li.file.clone(),
            line: 0,
            rule: "model-load-skip".to_string(),
            message: format!("сущность пропущена из-за ошибки разбора: {}", li.reason),
            severity: "warn".to_string(),
            ..LintIssue::default()
        });
    }

    // 1. CMP → код: каждый корень code_roots обязан существовать.
    let mut covered_roots: Vec<String> = Vec::new();
    let mut components_with_roots = 0usize;
    for e in &model.entities {
        if e.kind != EntityKind::Cmp || e.code_roots.is_empty() {
            continue;
        }
        components_with_roots += 1;
        for raw in &e.code_roots {
            let root = normalize_root(raw);
            let exists = if root.is_empty() {
                true // «.» — корень репозитория, существует по построению
            } else {
                // Недоступен (права, обрыв ссылки) — считаем отсутствующим:
                // дрейф-находка с этим путём видна и проверяется человеком.
                case_dir.join(&root).try_exists().unwrap_or_default()
            };
            if exists {
                covered_roots.push(root);
            } else {
                push_issue(
                    &mut issues,
                    "error",
                    e.file.clone(),
                    "code-root-missing",
                    format!(
                        "{}: code_roots '{raw}' — путь не существует относительно корня кейса",
                        e.id
                    ),
                );
            }
        }
    }
    covered_roots.sort();
    covered_roots.dedup();

    // 2. Код → модель: каталог с манифестом без покрывающего CMP.
    let manifest_dirs = manifest_dirs(case_dir)?;
    for dir in &manifest_dirs {
        let covered = covered_roots.iter().any(|r| root_covers(r, dir));
        if !covered {
            push_issue(
                &mut issues,
                "warn",
                case_dir.join(dir),
                "uncovered-manifest",
                format!(
                    "каталог '{dir}' содержит манифест сборки, но не покрыт ни одним \
                     code_roots компонента (код вне модели)"
                ),
            );
        }
    }

    // 3. Звено INT → контракт (ADR-035): коды и критичность — как в trace check.
    for e in &model.entities {
        if e.kind != EntityKind::Int {
            continue;
        }
        match contract_path(e) {
            Some(c) => {
                let exists = case_dir.join(c).try_exists().unwrap_or(false);
                if !exists {
                    push_issue(
                        &mut issues,
                        "error",
                        e.file.clone(),
                        "int-contract-missing",
                        format!(
                            "{}: контрактный артефакт '{c}' не найден (путь относительно \
                             корня кейса)",
                            e.id
                        ),
                    );
                }
            }
            None => push_issue(
                &mut issues,
                "warn",
                e.file.clone(),
                "int-without-contract",
                format!(
                    "{}: интеграция без контрактного артефакта (поле contract не задано)",
                    e.id
                ),
            ),
        }
    }

    issues.sort_by(|a, b| {
        a.file
            .cmp(&b.file)
            .then(a.rule.cmp(&b.rule))
            .then(a.message.cmp(&b.message))
    });
    Ok(DriftReport {
        case: case_dir.to_path_buf(),
        entities: model.entities.len(),
        components_with_roots,
        manifest_dirs: manifest_dirs.len(),
        issues,
    })
}

/// JSON-вердикт инструмента: тот же конверт `{passed, issues, summary}`,
/// что у `model_validate` (issues — сериализованные [`LintIssue`]).
#[must_use]
pub fn verdict_json(report: &DriftReport) -> Value {
    json!({
        "tool": "model_drift",
        "passed": !report.has_errors(),
        "issues": report.issues,
        "summary": report.summary(),
    })
}

/// Текстовый рендер отчёта для CLI: находки + сводка + итог.
#[must_use]
pub fn render_text(report: &DriftReport) -> String {
    let mut out = String::new();
    for i in &report.issues {
        let _ = writeln!(
            out,
            "[{}] {}: {} — {}",
            i.severity,
            i.file.display(),
            i.rule,
            i.message
        );
        if let Some(hint) = &i.fix_hint {
            let _ = writeln!(out, "  исправление: {hint}");
        }
    }
    let _ = writeln!(out, "{}", report.summary());
    let _ = writeln!(
        out,
        "Итог: {}",
        if report.has_errors() { "FAIL" } else { "PASS" }
    );
    out
}

/// Инструмент `model_drift`: дрейф «модель ↔ код» — JSON-вердикт
/// `{passed, issues, summary}` (мост в MCP, read-only).
pub struct ModelDriftTool;

#[derive(Debug, Deserialize)]
struct ModelDriftArgs {
    /// Корень кейса (каталог с `model/` внутри; дефолт — текущий каталог).
    dir: Option<String>,
}

#[async_trait]
impl Tool for ModelDriftTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "model_drift".into(),
            description: "Дрейф «модель ↔ код» (корень кейса с model/): CMP с несуществующими \
                          code_roots (ADR-030) — error; каталог с манифестом сборки без \
                          покрывающего CMP — warn; INT с битым путём contract — error, без \
                          поля contract — warn (ADR-035, семантика trace check). Ответ — JSON: \
                          passed + issues (severity/rule/file/message/fix_hint) + summary; \
                          passed=false — основание отказать изменению"
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "dir": {"type": "string", "description": "Корень кейса — каталог с model/ (по умолчанию текущий)"}
                }
            }),
        }
    }

    async fn call(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let args: ModelDriftArgs = match serde_json::from_value(args) {
            Ok(a) => a,
            Err(e) => {
                return Ok(ToolOutput::err(format!(
                    "model_drift: невалидные аргументы: {e}"
                )));
            }
        };
        let dir = ctx.resolve(args.dir.as_deref().unwrap_or("."));
        let report = match drift_check(&dir) {
            Ok(r) => r,
            Err(e) => return Ok(ToolOutput::err(format!("model_drift: {e}"))),
        };
        let verdict = verdict_json(&report);
        // Сериализация собранного объекта не падает; запасной вариант — компактная форма.
        let text = serde_json::to_string_pretty(&verdict).unwrap_or_else(|_| verdict.to_string());
        Ok(ToolOutput::ok(text))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    /// Пишет файл фикстуры (родители создаются).
    fn write_file(dir: &Path, name: &str, content: &str) -> PathBuf {
        let p = dir.join(name);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).expect("mkdir");
        }
        std::fs::write(&p, content).expect("write");
        p
    }

    /// Базовый кейс: модель с CMP (`code_roots` на существующий каталог
    /// `services/billing` с Cargo.toml) и INT с живым контрактом.
    fn fixture_case(dir: &Path) -> PathBuf {
        let case = dir.join("case");
        write_file(
            &case,
            "model/CMP-001-billing.md",
            "---\nid: CMP-001\ntype: cmp\ntitle: Billing\nstatus: adopted\ncode_roots: [services/billing]\n---\nКомпонент.\n",
        );
        write_file(
            &case,
            "model/INT-001-rail.md",
            "---\nid: INT-001\ntype: int\ntitle: Рельс\nstatus: accepted\ncontract: contracts/rail.yaml\n---\nИнтеграция.\n",
        );
        write_file(&case, "contracts/rail.yaml", "asyncapi: 3.0.0\n");
        write_file(
            &case,
            "services/billing/Cargo.toml",
            "[package]\nname = \"billing\"\n",
        );
        case
    }

    #[test]
    fn clean_case_has_no_issues() {
        let dir = tempfile::tempdir().expect("tmp");
        let case = fixture_case(dir.path());
        let report = drift_check(&case).expect("drift");
        assert!(report.issues.is_empty(), "{}", report.summary());
        assert!(!report.has_errors());
        assert_eq!(report.entities, 2);
        assert_eq!(report.components_with_roots, 1);
        assert_eq!(report.manifest_dirs, 1);
        let text = render_text(&report);
        assert!(text.contains("Итог: PASS"), "{text}");
    }

    /// E3: битая сущность — warn-находка `model-load-skip`, дрейф валидного
    /// подмножества считается, инструмент не падает.
    #[test]
    fn broken_entity_is_warn_and_valid_subset_checked() {
        let dir = tempfile::tempdir().expect("tmp");
        let case = fixture_case(dir.path());
        write_file(
            &case,
            "model/NFR-001-broken.md",
            "---\nid: NFR-001\ntype: nfr\ntitle: SLA\nstatus: accepted\navailability_target: \"99.9\"\n---\n",
        );
        let report = drift_check(&case).expect("drift");
        let skip = report
            .issues
            .iter()
            .find(|i| i.rule == "model-load-skip")
            .expect("warn-находка пропуска");
        assert_eq!(skip.severity, "warn");
        assert!(skip.file.ends_with("NFR-001-broken.md"));
        assert!(!report.has_errors(), "{}", report.summary());
        assert_eq!(report.entities, 2, "валидные сущности посчитаны");
        let v = verdict_json(&report);
        assert_eq!(v["passed"], true, "{v}");
        let text = render_text(&report);
        assert!(text.contains("Итог: PASS"), "{text}");
    }

    #[test]
    fn missing_code_root_is_error() {
        let dir = tempfile::tempdir().expect("tmp");
        let case = fixture_case(dir.path());
        write_file(
            &case,
            "model/CMP-002-ghost.md",
            "---\nid: CMP-002\ntype: cmp\ntitle: Ghost\nstatus: adopted\ncode_roots: [services/ghost, \"./\"]\n---\nПризрак.\n",
        );
        let report = drift_check(&case).expect("drift");
        let issue = report
            .issues
            .iter()
            .find(|i| i.rule == "code-root-missing")
            .expect("error-находка");
        assert_eq!(issue.severity, "error");
        assert!(
            issue.message.contains("services/ghost"),
            "{}",
            issue.message
        );
        assert_eq!(issue.adr.as_deref(), Some("ADR-030"));
        assert!(issue.fix_hint.is_some(), "fix_hint заполнен");
        // «./» — корень репозитория — существует всегда, находкой не стал.
        assert_eq!(
            report
                .issues
                .iter()
                .filter(|i| i.rule == "code-root-missing")
                .count(),
            1,
            "{}",
            report.summary()
        );
        assert!(report.has_errors());
        let text = render_text(&report);
        assert!(text.contains("Итог: FAIL"), "{text}");
    }

    #[test]
    fn uncovered_manifest_is_warn_nested_covered() {
        let dir = tempfile::tempdir().expect("tmp");
        let case = fixture_case(dir.path());
        // Новый каталог с манифестом вне code_roots.
        write_file(
            &case,
            "services/notify/package.json",
            "{\"name\": \"notify\"}\n",
        );
        // Вложенный манифест ВНУТРИ покрытого корня — не находка.
        write_file(&case, "services/billing/inner/go.mod", "module inner\n");
        let report = drift_check(&case).expect("drift");
        let uncovered: Vec<&LintIssue> = report
            .issues
            .iter()
            .filter(|i| i.rule == "uncovered-manifest")
            .collect();
        assert_eq!(uncovered.len(), 1, "{}", report.summary());
        assert_eq!(uncovered[0].severity, "warn");
        assert!(
            uncovered[0].message.contains("services/notify"),
            "{}",
            uncovered[0].message
        );
        assert!(!report.has_errors(), "warn не ломает итог");
        // Корневой манифест кейса (dir "") не покрыт — отдельная warn-находка.
        write_file(&case, "pom.xml", "<project/>\n");
        let report = drift_check(&case).expect("drift");
        assert!(
            report
                .issues
                .iter()
                .any(|i| i.rule == "uncovered-manifest" && i.message.contains("''")),
            "{}",
            report.summary()
        );
        // CMP с code_roots ["."] покрывает всё, включая корневой манифест.
        write_file(
            &case,
            "model/CMP-009-root.md",
            "---\nid: CMP-009\ntype: cmp\ntitle: Root\nstatus: adopted\ncode_roots: [\".\"]\n---\nКорневой.\n",
        );
        let report = drift_check(&case).expect("drift");
        assert!(
            !report.issues.iter().any(|i| i.rule == "uncovered-manifest"),
            "{}",
            report.summary()
        );
    }

    #[test]
    fn int_contract_semantics_match_trace() {
        let dir = tempfile::tempdir().expect("tmp");
        let case = fixture_case(dir.path());
        // Битый путь contract — error int-contract-missing (как trace check).
        write_file(
            &case,
            "model/INT-002-broken.md",
            "---\nid: INT-002\ntype: int\ntitle: Битый\nstatus: accepted\ncontract: contracts/ghost.yaml\n---\nИнтеграция.\n",
        );
        // Без поля contract — warn int-without-contract.
        write_file(
            &case,
            "model/INT-003-bare.md",
            "---\nid: INT-003\ntype: int\ntitle: Голый\nstatus: accepted\n---\nИнтеграция.\n",
        );
        // Пустое значение contract == отсутствие поля.
        write_file(
            &case,
            "model/INT-004-empty.md",
            "---\nid: INT-004\ntype: int\ntitle: Пустой\nstatus: accepted\ncontract: \"  \"\n---\nИнтеграция.\n",
        );
        let report = drift_check(&case).expect("drift");
        let missing: Vec<&LintIssue> = report
            .issues
            .iter()
            .filter(|i| i.rule == "int-contract-missing")
            .collect();
        assert_eq!(missing.len(), 1, "{}", report.summary());
        assert_eq!(missing[0].severity, "error");
        assert!(
            missing[0].message.contains("INT-002"),
            "{}",
            missing[0].message
        );
        assert_eq!(missing[0].adr.as_deref(), Some("ADR-035"));
        let without: Vec<&LintIssue> = report
            .issues
            .iter()
            .filter(|i| i.rule == "int-without-contract")
            .collect();
        assert_eq!(without.len(), 2, "INT-003 и INT-004: {}", report.summary());
        assert!(without.iter().all(|i| i.severity == "warn"));
        assert!(report.has_errors());
    }

    #[test]
    fn missing_model_dir_is_error() {
        let dir = tempfile::tempdir().expect("tmp");
        assert!(drift_check(dir.path()).is_err());
    }

    #[tokio::test]
    async fn model_drift_tool_json_contract() {
        let dir = tempfile::tempdir().expect("tmp");
        let case = fixture_case(dir.path());
        let ctx = ToolContext::new(case.clone(), Arc::new(crate::config::Config::default()));
        let tool = ModelDriftTool;
        let out = tool.call(json!({"dir": "."}), &ctx).await.expect("вызов");
        assert!(!out.is_error, "{}", out.content);
        let v: Value = serde_json::from_str(&out.content).expect("JSON-вердикт");
        assert_eq!(v["tool"], "model_drift");
        assert_eq!(v["passed"], true, "{v}");
        assert!(v["issues"].as_array().expect("issues").is_empty());

        // Находка попадает в JSON с полями LintIssue (rule/severity/fix_hint).
        write_file(
            &case,
            "model/CMP-002-ghost.md",
            "---\nid: CMP-002\ntype: cmp\ntitle: Ghost\nstatus: adopted\ncode_roots: [ghost]\n---\nПризрак.\n",
        );
        let out = tool.call(json!({}), &ctx).await.expect("вызов");
        let v: Value = serde_json::from_str(&out.content).expect("JSON-вердикт");
        assert_eq!(v["passed"], false, "{v}");
        let issue = &v["issues"][0];
        assert_eq!(issue["rule"], "code-root-missing");
        assert_eq!(issue["severity"], "error");
        assert!(issue["fix_hint"].is_string(), "{issue}");

        // Несуществующий кейс — мягкая ошибка инструмента.
        let out = tool
            .call(json!({"dir": "ghost-case"}), &ctx)
            .await
            .expect("вызов");
        assert!(out.is_error, "{}", out.content);
    }
}
