//! Машинные форматы отчётов контроля (`--format`) — чистые рендеры поверх
//! отчётов `gate`, `control check`, `trace check` и `contract-diff`
//! (бэклог волны 2, п.8): хуки хостов ненадёжны (у qwen headless-файринг не
//! подтверждён, у Codex lifecycle-хуков нет), поэтому CI и git-хуки —
//! единственный гейт, не зависящий от хоста, и ему нужны нативные форматы
//! площадок, а не только текст и `--json`.
//!
//! Каналы вывода (конвенция существующего `--json`, `docs/headless.md`):
//! машинный отчёт — строго в stdout (годен для редиректа в файл-артефакт CI),
//! человеческий текст туда не подмешивается; exit-коды команд не меняются:
//! красный гейт — это данные отчёта (отчёт печатается полностью, exit 1),
//! а не сбой исполнения.
//!
//! Форматы ([`ReportFormat`]):
//! - `sarif` — SARIF 2.1.0 (`runs[0].tool.driver.rules` + `results` с
//!   `level` error/warning, `locations` при наличии адреса и стабильными
//!   `partialFingerprints`) — code scanning `GitHub` и импортёр сторонних
//!   сканеров в `GitLab`;
//! - `junit` — `JUnit` XML: `testsuite` на группу (составляющая гейта/правило),
//!   `testcase` на находку, `failure` — только у error-находок (warn не
//!   ломает гейт — и в `JUnit` не ломает тест), SKIP-группы — `<skipped/>`;
//! - `gitlab-codequality` — JSON-массив Code Quality (`description`,
//!   `check_name`, `fingerprint` по правилу+файлу+строке, `severity`,
//!   `location.path`/`lines.begin`): артефакт `reports.codequality` показывает
//!   нарушения в интерфейсе merge request без ручной настройки;
//! - `markdown` — таблица находок + сводка (job summary / комментарий MR).
//!
//! Нормализация: разноформатные отчёты (`GateReport`, `FitnessReport`,
//! `TraceReport`, `Vec<contract_diff::Finding>`) приводятся к общему виду
//! [`FmtReport`] → группы [`FmtGroup`] → находки [`Finding`]; рендеры
//! работают только с нормализованным видом и от источников не зависят.
//!
//! Ограничения форматов площадок: у GitLab Code Quality `location.path` и
//! `lines.begin` обязательны — находки без адреса получают путь-заглушку
//! [`NO_LOCATION_PATH`] (видны в полном отчёте пайплайна); находок в одном
//! отчёте не больше [`MAX_FINDINGS`] (страховка артефактов CI от раздувания).

use std::collections::BTreeMap;
use std::fmt::Write as _;

use crate::contract_diff;
use crate::control::FitnessReport;
use crate::gate::{GateReport, GateStatus};
use crate::trace::TraceReport;

/// Потолок находок в одном машинном отчёте: простыня из тысяч записей
/// раздувает артефакты CI и валит загрузку виджетов; полный список всегда
/// доступен текстовым выводом соответствующей команды.
const MAX_FINDINGS: usize = 1000;

/// Путь-заглушка для находок без адреса (GitLab Code Quality требует
/// `location.path` у каждой записи).
const NO_LOCATION_PATH: &str = "(repository)";

/// Формат машинного отчёта (`--format`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReportFormat {
    /// Текстовый вывод команды (дефолт; рендерится самой командой, не здесь).
    Text,
    /// SARIF 2.1.0.
    Sarif,
    /// `JUnit` XML.
    Junit,
    /// `GitLab` Code Quality (JSON-массив).
    GitlabCodeQuality,
    /// Markdown-таблица + сводка.
    Markdown,
}

impl ReportFormat {
    /// Разбор значения CLI: `text` | `sarif` | `junit` | `gitlab-codequality`
    /// (алиасы `gitlab`, `codequality`) | `markdown` (алиас `md`).
    ///
    /// # Errors
    /// Неизвестный формат — сообщение со списком допустимых.
    pub fn parse(raw: &str) -> std::result::Result<Self, String> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "text" => Ok(Self::Text),
            "sarif" => Ok(Self::Sarif),
            "junit" => Ok(Self::Junit),
            "gitlab-codequality" | "gitlab" | "codequality" => Ok(Self::GitlabCodeQuality),
            "markdown" | "md" => Ok(Self::Markdown),
            other => Err(format!(
                "неизвестный формат '{other}' (допустимы: text, sarif, junit, gitlab-codequality, markdown)"
            )),
        }
    }
}

/// Критичность находки в нормализованном виде (error ломает гейт, warn — нет).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    /// Блокирующая находка.
    Error,
    /// Предупреждение.
    Warn,
}

impl Severity {
    /// Из строки источника (`error`/`warn`; прочее — warn: неизвестное не
    /// должно делать отчёт краснее, чем посчитал движок).
    fn from_raw(raw: &str) -> Self {
        match raw {
            "error" => Self::Error,
            _ => Self::Warn,
        }
    }

    /// Метка для вывода.
    fn label(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warn => "warn",
        }
    }
}

/// Нормализованная находка для машинных форматов.
#[derive(Debug, Clone)]
pub struct Finding {
    /// Группа (составляющая гейта, имя fitness-правила, код CD-…).
    pub group: String,
    /// Код правила (находки без правила не бывает: rule-less группа даёт
    /// синтетическую находку с rule = имя группы).
    pub rule: String,
    /// Критичность.
    pub severity: Severity,
    /// Сообщение (одной строкой).
    pub message: String,
    /// Файл (как в отчёте источника), если находка адресная.
    pub file: Option<String>,
    /// Строка (`None` — без адреса / файл целиком, у источников это line 0).
    pub line: Option<usize>,
}

/// Статус группы находок (составляющей гейта / правила).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupStatus {
    /// Пройдена (находок нет).
    Pass,
    /// Провалена (есть error-находки или сбой выполнения).
    Fail,
    /// Пропущена fail-soft (нет входа) — в `JUnit` это `<skipped/>`.
    Skip,
}

/// Группа находок: составляющая гейта или правило.
#[derive(Debug)]
pub struct FmtGroup {
    /// Имя группы (`fitness`, `no_pan`, `CD-001`, …).
    pub name: String,
    /// Статус.
    pub status: GroupStatus,
    /// Краткая сводка группы (для pass/skip-кейсов `JUnit` и markdown).
    pub detail: String,
    /// Находки группы.
    pub findings: Vec<Finding>,
}

/// Позиция группы `rule` в `groups`: группа создаётся при первом обращении
/// (pass-пустая), индекс строится по ходу (порядок групп — порядок находок
/// источника).
fn group_pos<'r>(
    groups: &mut Vec<FmtGroup>,
    index: &mut BTreeMap<&'r str, usize>,
    rule: &'r str,
) -> usize {
    if let Some(&pos) = index.get(rule) {
        return pos;
    }
    groups.push(FmtGroup {
        name: rule.to_string(),
        status: GroupStatus::Pass,
        detail: String::new(),
        findings: Vec::new(),
    });
    index.insert(rule, groups.len() - 1);
    groups.len() - 1
}

/// Выставляет группам статус Fail при error-находках и дефолтный detail.
fn finalize_groups(groups: &mut [FmtGroup]) {
    for g in groups {
        if g.findings.iter().any(|f| f.severity == Severity::Error) {
            g.status = GroupStatus::Fail;
        }
        if g.detail.is_empty() {
            g.detail = format!("находок: {}", g.findings.len());
        }
    }
}

/// Нормализованный отчёт: единый вход всех машинных рендеров.
#[derive(Debug)]
pub struct FmtReport {
    /// Имя инструмента (`arch-be gate`, `arch-be control check`, …).
    pub tool: &'static str,
    /// Человеческая сводка одной строкой (заголовок markdown).
    pub summary: String,
    /// Итог прогона (false — команда завершится exit 1).
    pub passed: bool,
    /// Группы в порядке обхода источника.
    pub groups: Vec<FmtGroup>,
}

impl FmtReport {
    /// Нормализация отчёта гейта: группа на составляющую; FAIL-составляющая
    /// без находок (сбой выполнения) даёт синтетическую находку с текстом
    /// причины — иначе она была бы невидима в машинных форматах.
    #[must_use]
    pub fn from_gate(report: &GateReport) -> Self {
        let mut groups = Vec::new();
        for c in &report.components {
            let mut findings: Vec<Finding> = c
                .findings
                .iter()
                .map(|f| Finding {
                    group: c.name.to_string(),
                    rule: f.rule.clone().unwrap_or_else(|| c.name.to_string()),
                    severity: Severity::from_raw(&f.severity),
                    message: f.message.clone(),
                    file: f.file.clone(),
                    line: f.line,
                })
                .collect();
            if c.status == GateStatus::Fail && findings.is_empty() {
                findings.push(Finding {
                    group: c.name.to_string(),
                    rule: c.name.to_string(),
                    severity: Severity::Error,
                    message: c.detail.clone(),
                    file: None,
                    line: None,
                });
            }
            groups.push(FmtGroup {
                name: c.name.to_string(),
                status: match c.status {
                    GateStatus::Pass => GroupStatus::Pass,
                    GateStatus::Fail => GroupStatus::Fail,
                    GateStatus::Skip => GroupStatus::Skip,
                },
                detail: c.detail.clone(),
                findings,
            });
        }
        Self {
            tool: "arch-be gate",
            summary: format!(
                "Гейт: {} · маршрут {} ({}) · итог {}",
                report.repo.display(),
                report.route,
                report.route_note,
                if report.passed { "PASS" } else { "FAIL" }
            ),
            passed: report.passed,
            groups,
        }
    }

    /// Нормализация `FitnessReport` (`control check`): группа на правило
    /// (правила без находок — pass-группы по реестру `durations`).
    #[must_use]
    pub fn from_fitness(report: &FitnessReport) -> Self {
        let mut groups: Vec<FmtGroup> = Vec::new();
        let mut index: BTreeMap<&str, usize> = BTreeMap::new();
        for i in &report.issues {
            let pos = group_pos(&mut groups, &mut index, i.rule.as_str());
            groups[pos].findings.push(Finding {
                group: i.rule.clone(),
                rule: i.rule.clone(),
                severity: Severity::from_raw(&i.severity),
                message: i.message.clone(),
                file: Some(i.file.display().to_string()),
                line: (i.line > 0).then_some(i.line),
            });
        }
        // Правила без находок — видимые pass-группы (по ним JUnit показывает,
        // что проверка реально прогонялась, а не отсутствует).
        for d in &report.durations {
            if !index.contains_key(d.rule.as_str()) {
                groups.push(FmtGroup {
                    name: d.rule.clone(),
                    status: GroupStatus::Pass,
                    detail: format!("пройдено за {} мс", d.ms),
                    findings: Vec::new(),
                });
            }
        }
        finalize_groups(&mut groups);
        Self {
            tool: "arch-be control check",
            summary: report.summary.clone(),
            passed: report.passed,
            groups,
        }
    }

    /// Нормализация `TraceReport` (`trace check`): группа на правило
    /// трассировки; пустой список находок — одна pass-группа `trace`.
    #[must_use]
    pub fn from_trace(report: &TraceReport) -> Self {
        let mut groups: Vec<FmtGroup> = Vec::new();
        let mut index: BTreeMap<&str, usize> = BTreeMap::new();
        for i in &report.issues {
            let pos = group_pos(&mut groups, &mut index, i.rule);
            groups[pos].findings.push(Finding {
                group: i.rule.to_string(),
                rule: i.rule.to_string(),
                severity: Severity::from_raw(&i.severity.to_string()),
                message: i.message.clone(),
                file: None,
                line: None,
            });
        }
        finalize_groups(&mut groups);
        if groups.is_empty() {
            groups.push(FmtGroup {
                name: "trace".to_string(),
                status: GroupStatus::Pass,
                detail: format!(
                    "сущностей: {}, звеньев: {}, находок: 0",
                    report.entities,
                    report.levels.len()
                ),
                findings: Vec::new(),
            });
        }
        let (errors, warns) = report.issues.iter().fold((0usize, 0usize), |(e, w), i| {
            if i.severity == crate::model::Severity::Error {
                (e + 1, w)
            } else {
                (e, w + 1)
            }
        });
        Self {
            tool: "arch-be trace check",
            summary: format!(
                "Трассируемость: {} · сущностей {}, звеньев {} · итог {} (error: {errors}, warn: {warns})",
                report.case.display(),
                report.entities,
                report.levels.len(),
                if report.has_errors() { "FAIL" } else { "PASS" }
            ),
            passed: !report.has_errors(),
            groups,
        }
    }

    /// Нормализация находок `contract-diff`: группа на правило CD-001..CD-006;
    /// пустой дифф — одна pass-группа `contract_diff`.
    #[must_use]
    pub fn from_contract_diff(findings: &[contract_diff::Finding]) -> Self {
        let mut groups: Vec<FmtGroup> = Vec::new();
        let mut index: BTreeMap<&str, usize> = BTreeMap::new();
        for f in findings {
            let pos = group_pos(&mut groups, &mut index, f.rule.as_str());
            groups[pos].findings.push(Finding {
                group: f.rule.clone(),
                rule: f.rule.clone(),
                severity: Severity::from_raw(&f.severity),
                message: format!("{} — {}", f.location, f.message),
                file: None,
                line: None,
            });
        }
        finalize_groups(&mut groups);
        if groups.is_empty() {
            groups.push(FmtGroup {
                name: "contract_diff".to_string(),
                status: GroupStatus::Pass,
                detail: "изменений нет".to_string(),
                findings: Vec::new(),
            });
        }
        let breaking = findings.iter().filter(|f| f.severity == "error").count();
        Self {
            tool: "arch-be contract-diff",
            summary: format!(
                "contract_diff: {} изменений (breaking: {breaking}, non-breaking: {}) · итог {}",
                findings.len(),
                findings.len() - breaking,
                if breaking == 0 { "PASS" } else { "FAIL" }
            ),
            passed: breaking == 0,
            groups,
        }
    }
}

/// Рендерит нормализованный отчёт в машинный формат.
///
/// [`ReportFormat::Text`] здесь не рендерится (текст — удел самих команд);
/// попадание сюда Text трактуется как markdown (защита от рассинхрона
/// вызовов, не часть контракта).
#[must_use]
pub fn render(format: ReportFormat, report: &FmtReport) -> String {
    match format {
        ReportFormat::Sarif => render_sarif(report),
        ReportFormat::Junit => render_junit(report),
        ReportFormat::GitlabCodeQuality => render_gitlab_code_quality(report),
        ReportFormat::Text | ReportFormat::Markdown => render_markdown(report),
    }
}

/// Собирает плоский список находок всех групп с потолком [`MAX_FINDINGS`].
fn all_findings(report: &FmtReport) -> Vec<&Finding> {
    report
        .groups
        .iter()
        .flat_map(|g| &g.findings)
        .take(MAX_FINDINGS)
        .collect()
}

/// FNV-1a (64 бита) — стабильный отпечаток находки без новых зависимостей:
/// детерминирован между прогонами и версиями (GitLab сопоставляет находки
/// merge request'ов по fingerprint, SARIF-потребители — по partialFingerprints).
fn fingerprint(parts: &[&str]) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for part in parts {
        for b in part.as_bytes() {
            hash ^= u64::from(*b);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        // Разделитель полей, чтобы конкатенации «ab|c» и «a|bc» различались.
        hash ^= 0xff;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

/// Экранирование текста для XML (`JUnit`).
fn xml_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// Экранирование ячейки markdown-таблицы: `|` и переводы строк.
fn md_cell(text: &str) -> String {
    text.replace('|', "\\|")
        .replace('\n', " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Адрес находки одной строкой (`file:line`, `file`, `—`).
fn location_label(f: &Finding) -> String {
    match (&f.file, f.line) {
        (Some(file), Some(line)) => format!("{file}:{line}"),
        (Some(file), None) => file.clone(),
        (None, _) => "—".to_string(),
    }
}

/// SARIF 2.1.0: rules + results с level error/warning, locations при наличии
/// адреса и partialFingerprints по правилу+адресу+сообщению.
fn render_sarif(report: &FmtReport) -> String {
    let findings = all_findings(report);
    let mut rule_ids: Vec<&str> = findings.iter().map(|f| f.rule.as_str()).collect();
    rule_ids.sort_unstable();
    rule_ids.dedup();
    let rules: Vec<serde_json::Value> = rule_ids
        .iter()
        .map(|id| {
            serde_json::json!({
                "id": id,
                "shortDescription": { "text": id },
            })
        })
        .collect();
    let results: Vec<serde_json::Value> = findings
        .iter()
        .map(|f| {
            let mut result = serde_json::json!({
                "ruleId": f.rule,
                "level": match f.severity {
                    Severity::Error => "error",
                    Severity::Warn => "warning",
                },
                "message": { "text": format!("[{}] {}", f.group, f.message) },
                "partialFingerprints": {
                    "spine/v1": fingerprint(&[
                        report.tool,
                        &f.rule,
                        f.file.as_deref().unwrap_or(""),
                        &f.line.map_or(String::new(), |l| l.to_string()),
                        &f.message,
                    ]),
                },
            });
            if let Some(file) = &f.file {
                let mut location = serde_json::json!({
                    "physicalLocation": {
                        "artifactLocation": { "uri": file },
                    }
                });
                if let Some(line) = f.line {
                    location["physicalLocation"]["region"] = serde_json::json!({
                        "startLine": line,
                    });
                }
                result["locations"] = serde_json::json!([location]);
            }
            result
        })
        .collect();
    let doc = serde_json::json!({
        "$schema": "https://json.schemastore.org/sarif-2.1.0.json",
        "version": "2.1.0",
        "runs": [{
            "tool": {
                "driver": {
                    "name": report.tool,
                    "version": env!("CARGO_PKG_VERSION"),
                    "rules": rules,
                }
            },
            "results": results,
        }],
    });
    // Сериализация собранного вручную JSON не падает; запасной вариант —
    // компактная форма (недостижима, но без unwrap).
    match serde_json::to_string_pretty(&doc) {
        Ok(text) => format!("{text}\n"),
        Err(_) => format!("{doc}\n"),
    }
}

/// `JUnit` XML: `testsuite` на группу, `testcase` на находку; `failure` —
/// только у error-находок (warn-находки — зелёные тесты с текстом в
/// `system-out`-стиле: предупреждение не ломает гейт и не должно валить
/// джобу Jenkins поверх вердикта самого `arch-be`).
fn render_junit(report: &FmtReport) -> String {
    let mut out = String::new();
    // Запись в String не может завершиться ошибкой — игноры безопасны.
    let total: usize = report.groups.iter().map(|g| g.findings.len().max(1)).sum();
    let failures: usize = all_findings(report)
        .iter()
        .filter(|f| f.severity == Severity::Error)
        .count();
    let skipped: usize = report
        .groups
        .iter()
        .filter(|g| g.status == GroupStatus::Skip)
        .count();
    let _ = writeln!(out, r#"<?xml version="1.0" encoding="UTF-8"?>"#);
    let _ = writeln!(
        out,
        r#"<testsuites name="{}" tests="{total}" failures="{failures}" skipped="{skipped}">"#,
        xml_escape(report.tool)
    );
    for g in &report.groups {
        let group_failures = g
            .findings
            .iter()
            .filter(|f| f.severity == Severity::Error)
            .count();
        let tests = g.findings.len().max(1);
        let _ = writeln!(
            out,
            r#"  <testsuite name="{}" tests="{tests}" failures="{group_failures}" skipped="{}">"#,
            xml_escape(&g.name),
            usize::from(g.status == GroupStatus::Skip)
        );
        if g.findings.is_empty() {
            // Группа без находок: один testcase, отражающий исход
            // (pass — зелёный; skip — <skipped/>; fail без находок не бывает —
            // адаптер синтезирует находку из detail).
            let name = format!("{}: {}", g.name, g.detail);
            if g.status == GroupStatus::Skip {
                let _ = writeln!(
                    out,
                    r#"    <testcase classname="{}" name="{}"><skipped message="{}"/></testcase>"#,
                    xml_escape(report.tool),
                    xml_escape(&name),
                    xml_escape(&g.detail)
                );
            } else {
                let _ = writeln!(
                    out,
                    r#"    <testcase classname="{}" name="{}"/>"#,
                    xml_escape(report.tool),
                    xml_escape(&name)
                );
            }
        }
        for f in g.findings.iter().take(MAX_FINDINGS) {
            let name = format!("{} — {}", f.rule, location_label(f));
            match f.severity {
                Severity::Error => {
                    let _ = writeln!(
                        out,
                        r#"    <testcase classname="{}" name="{}">"#,
                        xml_escape(&g.name),
                        xml_escape(&name)
                    );
                    let _ = writeln!(
                        out,
                        r#"      <failure message="{}">{}</failure>"#,
                        xml_escape(&f.message),
                        xml_escape(&format!("{}: {}", location_label(f), f.message))
                    );
                    let _ = writeln!(out, "    </testcase>");
                }
                Severity::Warn => {
                    let _ = writeln!(
                        out,
                        r#"    <testcase classname="{}" name="[{}] {}">"#,
                        xml_escape(&g.name),
                        f.severity.label(),
                        xml_escape(&name)
                    );
                    let _ = writeln!(
                        out,
                        r"      <system-out>{}</system-out>",
                        xml_escape(&f.message)
                    );
                    let _ = writeln!(out, "    </testcase>");
                }
            }
        }
        let _ = writeln!(out, "  </testsuite>");
    }
    let _ = writeln!(out, "</testsuites>");
    out
}

/// GitLab Code Quality: массив записей `{description, check_name, fingerprint,
/// severity, location}`; fingerprint стабилен по правилу+пути+строке (GitLab
/// сопоставляет находки между MR), severity error→major / warn→minor.
fn render_gitlab_code_quality(report: &FmtReport) -> String {
    let findings = all_findings(report);
    let entries: Vec<serde_json::Value> = findings
        .iter()
        .map(|f| {
            let path = f
                .file
                .clone()
                .unwrap_or_else(|| NO_LOCATION_PATH.to_string());
            serde_json::json!({
                "description": format!("[{}] {}", f.rule, f.message),
                "check_name": format!("{}: {}", report.tool, f.rule),
                "fingerprint": fingerprint(&[
                    report.tool,
                    &f.rule,
                    &path,
                    &f.line.map_or(String::new(), |l| l.to_string()),
                ]),
                "severity": match f.severity {
                    Severity::Error => "major",
                    Severity::Warn => "minor",
                },
                "location": {
                    "path": path,
                    "lines": { "begin": f.line.unwrap_or(1) },
                },
            })
        })
        .collect();
    // Сериализация собранного вручную JSON не падает; запасной вариант —
    // компактная форма.
    let doc = serde_json::Value::Array(entries);
    match serde_json::to_string_pretty(&doc) {
        Ok(text) => format!("{text}\n"),
        Err(_) => format!("{doc}\n"),
    }
}

/// Markdown: сводка + таблица находок + статусы групп без находок
/// (для job summary GitHub/GitLab и комментариев к MR).
fn render_markdown(report: &FmtReport) -> String {
    let mut out = String::new();
    // Запись в String не может завершиться ошибкой — игноры безопасны.
    let _ = writeln!(out, "## Отчёт: {}\n", report.tool);
    let _ = writeln!(
        out,
        "**{}**\n",
        if report.passed {
            "Итог: PASS".to_string()
        } else {
            "Итог: FAIL".to_string()
        }
    );
    let _ = writeln!(out, "{}\n", md_cell(&report.summary));
    let findings = all_findings(report);
    if findings.is_empty() {
        let _ = writeln!(out, "Находок нет.\n");
    } else {
        let _ = writeln!(
            out,
            "| Группа | Severity | Правило | Локация | Находка |\n|---|---|---|---|---|"
        );
        for f in &findings {
            let _ = writeln!(
                out,
                "| {} | {} | {} | {} | {} |",
                md_cell(&f.group),
                f.severity.label(),
                md_cell(&f.rule),
                md_cell(&location_label(f)),
                md_cell(&f.message)
            );
        }
        let total: usize = report.groups.iter().map(|g| g.findings.len()).sum();
        if total > MAX_FINDINGS {
            let _ = writeln!(
                out,
                "\n… и ещё {} находок (потолок машинного отчёта {MAX_FINDINGS}; полный список — текстовым выводом команды)\n",
                total - MAX_FINDINGS
            );
        }
    }
    // Сводка статусов — только по группам без находок (группы с находками
    // уже видны в таблице; pass-группа с warn-находками не «чистая»).
    let mut pass = Vec::new();
    let mut skip = Vec::new();
    for g in &report.groups {
        if !g.findings.is_empty() {
            continue;
        }
        match g.status {
            GroupStatus::Pass => pass.push(g.name.as_str()),
            GroupStatus::Skip => skip.push(g.name.as_str()),
            GroupStatus::Fail => {}
        }
    }
    if !pass.is_empty() {
        let _ = writeln!(out, "PASS: {}", pass.join(", "));
    }
    if !skip.is_empty() {
        let _ = writeln!(out, "SKIP: {} (нет входа)", skip.join(", "));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::Route;
    use crate::gate::{GateComponent, GateFinding, GateReport, GateStatus};

    /// Отчёт-фикстура гейта: одна FAIL-составляющая с адресной находкой,
    /// одна PASS, одна SKIP.
    fn gate_report() -> GateReport {
        GateReport {
            repo: std::path::PathBuf::from("."),
            route: Route::Fast,
            route_auto: true,
            route_note: "auto: score 0 (триггеров нет)".to_string(),
            components: vec![
                GateComponent {
                    name: "fitness",
                    status: GateStatus::Fail,
                    detail: "Правил: 2, нарушений: 1 (error: 1, warn: 0)".to_string(),
                    findings: vec![GateFinding {
                        severity: "error".to_string(),
                        rule: Some("no_pan".to_string()),
                        file: Some("src/hotfix.py".to_string()),
                        line: Some(3),
                        message: "must_not_contain: запрещённый паттерн 'PAN'".to_string(),
                    }],
                },
                GateComponent {
                    name: "delta_guard",
                    status: GateStatus::Pass,
                    detail: "изменённых файлов: 1, защищённых среди них: 0".to_string(),
                    findings: Vec::new(),
                },
                GateComponent {
                    name: "trace_check",
                    status: GateStatus::Skip,
                    detail: "нет каталога model/".to_string(),
                    findings: Vec::new(),
                },
            ],
            passed: false,
        }
    }

    /// FAIL-группа без находок (сбой выполнения) даёт синтетическую находку.
    #[test]
    fn gate_fail_without_findings_yields_synthetic_finding() {
        let mut report = gate_report();
        report.components[0].findings.clear();
        let norm = FmtReport::from_gate(&report);
        let group = &norm.groups[0];
        assert_eq!(group.status, GroupStatus::Fail);
        assert_eq!(group.findings.len(), 1, "синтетическая находка из detail");
        assert_eq!(group.findings[0].rule, "fitness");
        assert!(
            group.findings[0].message.contains("нарушений"),
            "{:?}",
            group.findings[0]
        );
    }

    /// SARIF: валидный JSON, версия 2.1.0, rules+results, level, locations,
    /// partialFingerprints.
    #[test]
    fn sarif_is_valid_json_with_rules_results_and_locations() {
        let norm = FmtReport::from_gate(&gate_report());
        let text = render(ReportFormat::Sarif, &norm);
        let doc: serde_json::Value = serde_json::from_str(&text).expect("валидный JSON");
        assert_eq!(doc["version"], "2.1.0");
        assert_eq!(
            doc["$schema"],
            "https://json.schemastore.org/sarif-2.1.0.json"
        );
        let run = &doc["runs"][0];
        assert_eq!(run["tool"]["driver"]["name"], "arch-be gate");
        let rules = run["tool"]["driver"]["rules"].as_array().expect("rules");
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0]["id"], "no_pan");
        let results = run["results"].as_array().expect("results");
        assert_eq!(results.len(), 1, "skip/pass-группы — не results");
        let r = &results[0];
        assert_eq!(r["ruleId"], "no_pan");
        assert_eq!(r["level"], "error");
        assert_eq!(
            r["locations"][0]["physicalLocation"]["artifactLocation"]["uri"],
            "src/hotfix.py"
        );
        assert_eq!(
            r["locations"][0]["physicalLocation"]["region"]["startLine"],
            3
        );
        let fp = r["partialFingerprints"]["spine/v1"]
            .as_str()
            .expect("fingerprint");
        assert_eq!(fp.len(), 16, "hex u64: {fp}");
        // Детерминизм: повторный рендер даёт тот же отпечаток.
        let text2 = render(ReportFormat::Sarif, &norm);
        let doc2: serde_json::Value = serde_json::from_str(&text2).expect("валидный JSON");
        assert_eq!(
            doc2["runs"][0]["results"][0]["partialFingerprints"]["spine/v1"],
            fp
        );
    }

    /// `JUnit`: валидный каркас XML (баланс тегов), testsuite на составляющую,
    /// failure у error-находки, skipped у SKIP-составляющей.
    #[test]
    fn junit_has_testsuite_per_component_failure_and_skipped() {
        let norm = FmtReport::from_gate(&gate_report());
        let text = render(ReportFormat::Junit, &norm);
        assert!(
            text.starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\"?>"),
            "{text}"
        );
        assert!(
            text.contains(r#"<testsuite name="fitness" tests="1" failures="1""#),
            "{text}"
        );
        assert!(
            text.contains(r#"<testcase classname="fitness" name="no_pan — src/hotfix.py:3">"#),
            "{text}"
        );
        assert!(text.contains("<failure message="), "{text}");
        assert!(
            text.contains(r#"<testsuite name="trace_check" tests="1" failures="0" skipped="1">"#),
            "{text}"
        );
        assert!(text.contains("<skipped"), "{text}");
        assert!(text.contains(r#"<testsuite name="delta_guard""#), "{text}");
        assert_xml_balanced(&text);
    }

    /// XML с опасными символами в сообщении экранируется.
    #[test]
    fn junit_escapes_xml_specials() {
        let mut norm = FmtReport::from_gate(&gate_report());
        norm.groups[0].findings[0].message = "тег <b> & \"кавычки\"".to_string();
        let text = render(ReportFormat::Junit, &norm);
        assert!(
            text.contains("&lt;b&gt; &amp; &quot;кавычки&quot;"),
            "{text}"
        );
        assert!(!text.contains("<b>"), "{text}");
        assert_xml_balanced(&text);
    }

    /// GitLab Code Quality: валидный JSON-массив, обязательные поля, mapping
    /// severity, стабильный fingerprint по правилу+пути+строке.
    #[test]
    fn gitlab_cq_entries_have_required_fields_and_stable_fingerprints() {
        let norm = FmtReport::from_gate(&gate_report());
        let text = render(ReportFormat::GitlabCodeQuality, &norm);
        let doc: serde_json::Value = serde_json::from_str(&text).expect("валидный JSON");
        let arr = doc.as_array().expect("массив верхнего уровня");
        assert_eq!(arr.len(), 1);
        let e = &arr[0];
        assert_eq!(e["severity"], "major");
        assert_eq!(e["location"]["path"], "src/hotfix.py");
        assert_eq!(e["location"]["lines"]["begin"], 3);
        assert!(
            e["description"]
                .as_str()
                .expect("description")
                .contains("no_pan")
        );
        assert!(
            e["check_name"]
                .as_str()
                .expect("check_name")
                .contains("arch-be gate")
        );
        let fp = e["fingerprint"].as_str().expect("fingerprint");
        // Сообщение в fingerprint не входит: правка формулировки не плодит
        // «новые» находки в GitLab.
        let mut norm2 = FmtReport::from_gate(&gate_report());
        norm2.groups[0].findings[0].message = "другая формулировка".to_string();
        let text2 = render(ReportFormat::GitlabCodeQuality, &norm2);
        let doc2: serde_json::Value = serde_json::from_str(&text2).expect("валидный JSON");
        assert_eq!(doc2[0]["fingerprint"], fp, "fingerprint стабилен");
        // А сдвиг строки — новая находка (GitLab-семантика).
        let mut norm3 = FmtReport::from_gate(&gate_report());
        norm3.groups[0].findings[0].line = Some(4);
        let text3 = render(ReportFormat::GitlabCodeQuality, &norm3);
        let doc3: serde_json::Value = serde_json::from_str(&text3).expect("валидный JSON");
        assert_ne!(doc3[0]["fingerprint"], fp);
    }

    /// GitLab CQ: находка без адреса получает путь-заглушку и строку 1
    /// (обязательные поля схемы).
    #[test]
    fn gitlab_cq_file_less_finding_gets_stub_path() {
        let mut norm = FmtReport::from_gate(&gate_report());
        let f = &mut norm.groups[0].findings[0];
        f.file = None;
        f.line = None;
        let text = render(ReportFormat::GitlabCodeQuality, &norm);
        let doc: serde_json::Value = serde_json::from_str(&text).expect("валидный JSON");
        assert_eq!(doc[0]["location"]["path"], NO_LOCATION_PATH);
        assert_eq!(doc[0]["location"]["lines"]["begin"], 1);
    }

    /// Markdown: таблица находок, сводка, статусы групп, экранирование `|`.
    #[test]
    fn markdown_has_table_summary_and_group_statuses() {
        let mut report = gate_report();
        report.components[0].findings[0].message =
            "паттерн 'PAN' | сработал\nв две строки".to_string();
        let norm = FmtReport::from_gate(&report);
        let text = render(ReportFormat::Markdown, &norm);
        assert!(text.contains("## Отчёт: arch-be gate"), "{text}");
        assert!(text.contains("**Итог: FAIL**"), "{text}");
        assert!(
            text.contains("| Группа | Severity | Правило | Локация | Находка |"),
            "{text}"
        );
        assert!(
            text.contains("| fitness | error | no_pan | src/hotfix.py:3 |"),
            "{text}"
        );
        assert!(
            text.contains("паттерн 'PAN' \\| сработал в две строки"),
            "{text}"
        );
        assert!(text.contains("PASS: delta_guard"), "{text}");
        assert!(text.contains("SKIP: trace_check"), "{text}");
    }

    /// Разбор формата: алиасы и ошибка со списком допустимых.
    #[test]
    fn report_format_parse_accepts_aliases_and_rejects_unknown() {
        assert_eq!(ReportFormat::parse("text"), Ok(ReportFormat::Text));
        assert_eq!(ReportFormat::parse("SARIF"), Ok(ReportFormat::Sarif));
        assert_eq!(ReportFormat::parse("junit"), Ok(ReportFormat::Junit));
        assert_eq!(
            ReportFormat::parse("gitlab-codequality"),
            Ok(ReportFormat::GitlabCodeQuality)
        );
        assert_eq!(
            ReportFormat::parse("gitlab"),
            Ok(ReportFormat::GitlabCodeQuality)
        );
        assert_eq!(ReportFormat::parse("markdown"), Ok(ReportFormat::Markdown));
        let err = ReportFormat::parse("pdf").expect_err("неизвестный формат");
        assert!(err.contains("sarif"), "{err}");
    }

    /// Проверка сбалансированности тегов XML (без внешнего парсера): стек
    /// открывающих/закрывающих тегов, самозакрывающиеся `<…/>` допустимы.
    fn assert_xml_balanced(text: &str) {
        let mut stack: Vec<&str> = Vec::new();
        let mut rest = text;
        while let Some(open) = rest.find('<') {
            let close = rest[open..]
                .find('>')
                .map(|i| open + i)
                .expect("тег закрывается");
            let tag = &rest[open + 1..close];
            if tag.starts_with('?') || tag.starts_with('!') {
                // prolog/комментарий — не тег.
            } else if let Some(name) = tag.strip_prefix('/') {
                let name = name.trim();
                assert_eq!(stack.pop(), Some(name), "несбалансированный XML: {text}");
            } else if !tag.ends_with('/') {
                let name = tag.split_whitespace().next().expect("имя тега");
                stack.push(name);
            }
            rest = &rest[close + 1..];
        }
        assert!(stack.is_empty(), "незакрытые теги {stack:?}: {text}");
    }
}
