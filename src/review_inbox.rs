//! Ящик ревью архитектора по флоту (K7, P2, эксперимент — за подкомандой
//! `arch-be review inbox <ROOT>`).
//!
//! Архитектор не может прочитать все PR флота: ящик собирает артефакты
//! `arch-be arch-diff --format json` (контракт `arch-be/arch-diff/v1`,
//! ADR-063), которые CI-джобы репозиториев складывают файлами (как реестры
//! ADR-033: файлы, не сервис), и показывает ОТКРЫТЫЕ изменения с непустым
//! диффом по убыванию маршрута значимости и числа задетых инвариантов.
//! Пустые диффы («архитектуре ревью не нужно») в ящик не попадают — это и
//! есть снятие нагрузки.
//!
//! Границы:
//! - только чтение файлов под ROOT (никаких персональных путей и конфигов
//!   флота: корень передаётся аргументом);
//! - толерантный ридер: схема проверяется по полю `schema` (принимается
//!   ровно `arch-be/arch-diff/v1`), отсутствующие поля читаются как пустые,
//!   битый JSON или чужая схема — предупреждение и пропуск, а не падение
//!   ящика: один битый артефакт не должен прятать остальные;
//! - детерминизм: обход и сортировка фиксированы, повторный прогон на том
//!   же дереве даёт тот же вывод;
//! - инструмент информационный: решений не принимает, exit-код всегда 0
//!   при успешном чтении (как `fleet audit` в режиме отчёта; конвенции
//!   отчёта — по образцу [`crate::fleet`]).

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::arch_diff::ARCH_DIFF_SCHEMA;
use crate::error::{HarnessError, Result};

/// Схема машинного отчёта ящика (`--format json`).
pub const REVIEW_INBOX_SCHEMA: &str = "arch-be/review-inbox/v1";

/// Одна запись ящика: непустой архитектурный дифф из артефакта CI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboxEntry {
    /// Путь к JSON-файлу относительно ROOT (детерминированный адрес записи).
    pub rel: String,
    /// База и голова диффа (как в артефакте; обычно полные sha).
    pub base: String,
    /// Голова диффа.
    pub head: String,
    /// Маршрут значимости строкой (`fast`/`standard`/`critical`, как в JSON).
    pub route: String,
    /// Ранг маршрута для сортировки: critical=3, standard=2, fast=1, прочее=0.
    pub route_rank: u8,
    /// Добавленные/удалённые рёбра и узлы (счётчики массивов артефакта).
    pub added_edges: usize,
    /// Удалённые рёбра.
    pub removed_edges: usize,
    /// Добавленные узлы.
    pub added_nodes: usize,
    /// Удалённые узлы.
    pub removed_nodes: usize,
    /// Задетые инварианты AD.
    pub invariants: usize,
    /// Рёбра вне модели (`model_status == not_in_model` среди добавленных).
    pub undeclared_edges: usize,
    /// Ломающие изменения контрактов (classification breaking|removed).
    pub breaking_contracts: usize,
    /// Затронутые ADR.
    pub adrs: usize,
}

impl InboxEntry {
    /// Вес записи для сортировки «горячее сверху»: ранг маршрута, число
    /// задетых инвариантов, объём изменения (добавленные рёбра+узлы) — всё
    /// по убыванию (Reverse), при равенстве — по пути (детерминизм).
    fn sort_key(
        &self,
    ) -> (
        std::cmp::Reverse<u8>,
        std::cmp::Reverse<usize>,
        std::cmp::Reverse<usize>,
        String,
    ) {
        (
            std::cmp::Reverse(self.route_rank),
            std::cmp::Reverse(self.invariants),
            std::cmp::Reverse(self.added_edges + self.added_nodes),
            self.rel.clone(),
        )
    }
}

/// Файл, пропущенный при сборе ящика (не JSON, чужая схема): предупреждение
/// с причиной, а не молчаливый пропуск и не падение.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkippedFile {
    /// Путь к файлу относительно ROOT.
    pub rel: String,
    /// Причина пропуска (по-русски).
    pub reason: String,
}

/// Отчёт ящика ревью: непустые диффы (отсортированы) + счётчики отбора.
#[derive(Debug, Clone)]
pub struct InboxReport {
    /// Корень набора репозиториев (аргумент команды).
    pub root: PathBuf,
    /// Непустые диффы по убыванию маршрута и числа задетых инвариантов.
    pub entries: Vec<InboxEntry>,
    /// Файлы-кандидаты, пропущенные с предупреждением (битый JSON, чужая
    /// схема).
    pub skipped: Vec<SkippedFile>,
    /// Диффов отфильтровано как пустые (читаются, но ревью не требуют).
    pub empty: usize,
}

impl InboxReport {
    /// Прочитано валидных артефактов `arch-diff/v1` (непустые + пустые).
    #[must_use]
    pub fn read_ok(&self) -> usize {
        self.entries.len() + self.empty
    }
}

/// Ранг маршрута для сортировки (незнакомый маршрут — ниже fast).
fn route_rank(route: &str) -> u8 {
    match route.trim().to_ascii_lowercase().as_str() {
        "critical" => 3,
        "standard" => 2,
        "fast" => 1,
        _ => 0,
    }
}

/// Длина массива JSON (нет поля или не массив — 0: толерантный ридер).
fn arr_len(v: &Value, key: &str) -> usize {
    v.get(key).and_then(Value::as_array).map_or(0, Vec::len)
}

/// Пуст ли дифф по смыслу `ArchDiff::is_empty` (узлы/рёбра/контракты/NFR).
/// Поля читаются толерантно: отсутствующее поле — пустой массив.
fn is_empty_diff(v: &Value) -> bool {
    [
        "added_nodes",
        "removed_nodes",
        "added_edges",
        "removed_edges",
        "contract_changes",
        "nfr_shifts",
    ]
    .iter()
    .all(|key| arr_len(v, key) == 0)
}

/// Разбор одного артефакта `arch-diff/v1` в запись ящика. `None` — дифф
/// пуст (в ящик не попадает).
fn parse_entry(rel: &str, v: &Value) -> Option<InboxEntry> {
    if is_empty_diff(v) {
        return None;
    }
    let route = v
        .get("route")
        .and_then(|r| r.get("route"))
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_string();
    let added = v.get("added_edges").and_then(Value::as_array);
    let undeclared = added.map_or(0, |edges| {
        edges
            .iter()
            .filter(|e| e.get("model_status").and_then(Value::as_str) == Some("not_in_model"))
            .count()
    });
    let breaking = v
        .get("contract_changes")
        .and_then(Value::as_array)
        .map_or(0, |changes| {
            changes
                .iter()
                .filter(|c| {
                    matches!(
                        c.get("classification").and_then(Value::as_str),
                        Some("breaking" | "removed")
                    )
                })
                .count()
        });
    Some(InboxEntry {
        rel: rel.to_string(),
        base: v
            .get("base")
            .and_then(Value::as_str)
            .unwrap_or("?")
            .to_string(),
        head: v
            .get("head")
            .and_then(Value::as_str)
            .unwrap_or("?")
            .to_string(),
        route_rank: route_rank(&route),
        route,
        added_edges: arr_len(v, "added_edges"),
        removed_edges: arr_len(v, "removed_edges"),
        added_nodes: arr_len(v, "added_nodes"),
        removed_nodes: arr_len(v, "removed_nodes"),
        invariants: arr_len(v, "invariants_touched"),
        undeclared_edges: undeclared,
        breaking_contracts: breaking,
        adrs: arr_len(v, "adrs_touched"),
    })
}

/// Собирает ящик ревью по корню набора репозиториев: рекурсивно ищет
/// файлы `arch-diff*.json` (кроме служебных каталогов — как у обхода
/// [`crate::survey`]), читает толерантно и отбирает непустые диффы.
///
/// # Errors
/// Корень недоступен или не читается (ошибки отдельных файлов — не фатальны:
/// они попадают в `skipped` с причиной).
pub fn scan(root: &Path) -> Result<InboxReport> {
    if !root.is_dir() {
        return Err(HarnessError::Control(format!(
            "review inbox: корень недоступен или не каталог: {}",
            root.display()
        )));
    }
    let mut candidates: Vec<PathBuf> = Vec::new();
    let walker = walkdir::WalkDir::new(root).follow_links(false).into_iter();
    for entry in walker.filter_entry(|e| {
        // Служебные каталоги не читаем: dot-каталоги (там живёт журнал
        // решений arch-diff-decisions.json — он НЕ артефакт диффа) и
        // сборочные SKIP_DIRS.
        if e.depth() > 0 && e.file_type().is_dir() {
            let name = e.file_name().to_string_lossy();
            !(name.starts_with('.') || crate::survey::SKIP_DIRS.contains(&name.as_ref()))
        } else {
            true
        }
    }) {
        let entry = entry.map_err(|e| {
            HarnessError::Control(format!("review inbox: обход {}: {e}", root.display()))
        })?;
        if !entry.file_type().is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy();
        let is_json = entry
            .path()
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("json"));
        if name.to_ascii_lowercase().starts_with("arch-diff") && is_json {
            candidates.push(entry.path().to_path_buf());
        }
    }
    candidates.sort();

    let mut report = InboxReport {
        root: root.to_path_buf(),
        entries: Vec::new(),
        skipped: Vec::new(),
        empty: 0,
    };
    for path in &candidates {
        let rel = path
            .strip_prefix(root)
            .map_or_else(|_| path.display().to_string(), |p| p.display().to_string());
        let parsed = std::fs::read_to_string(path)
            .map_err(|e| format!("не читается: {e}"))
            .and_then(|text| {
                serde_json::from_str::<Value>(&text).map_err(|e| format!("не JSON: {e}"))
            });
        let value = match parsed {
            Ok(v) => v,
            Err(reason) => {
                report.skipped.push(SkippedFile { rel, reason });
                continue;
            }
        };
        // Схема — по полю `schema`: принимаем ровно arch-diff/v1, остальное
        // (включая журнал решений и отчёты других инструментов) — пропуск.
        let schema = value.get("schema").and_then(Value::as_str);
        if schema != Some(ARCH_DIFF_SCHEMA) {
            report.skipped.push(SkippedFile {
                rel,
                reason: format!(
                    "поле schema — {}, а не {ARCH_DIFF_SCHEMA}",
                    schema.map_or_else(|| "отсутствует".to_string(), |s| format!("«{s}»"))
                ),
            });
            continue;
        }
        match parse_entry(&rel, &value) {
            Some(entry) => report.entries.push(entry),
            None => report.empty += 1,
        }
    }
    // Горячее сверху: маршрут, инварианты, объём; при равенстве — по пути
    // (ключ кэшируется: он не меняется при сортировке).
    report.entries.sort_by_cached_key(InboxEntry::sort_key);
    Ok(report)
}

/// sha для вывода: короткая форма (12 знаков), как в рендерах arch-diff.
fn short_rev(rev: &str) -> &str {
    let n = rev.len().min(12);
    &rev[..n]
}

/// Текстовый рендер ящика: сводка отбора + по строке на непустой дифф.
#[must_use]
pub fn render(report: &InboxReport) -> String {
    let mut out = String::new();
    // Запись в String не может завершиться ошибкой — игноры безопасны.
    let _ = writeln!(out, "Ящик ревью архитектора: {}", report.root.display());
    let _ = writeln!(
        out,
        "Непустых диффов: {} · пустых отфильтровано: {} · пропущено с предупреждением: {}\n",
        report.entries.len(),
        report.empty,
        report.skipped.len()
    );
    if report.entries.is_empty() {
        let _ = writeln!(
            out,
            "Открытых архитектурных изменений нет — ревью не требуется."
        );
    }
    for e in &report.entries {
        let mut flags: Vec<String> = vec![format!(
            "рёбра +{}/−{} · узлы +{}/−{}",
            e.added_edges, e.removed_edges, e.added_nodes, e.removed_nodes
        )];
        if e.invariants > 0 {
            flags.push(format!("инварианты: {}", e.invariants));
        }
        if e.undeclared_edges > 0 {
            flags.push(format!("рёбер вне модели: {}", e.undeclared_edges));
        }
        if e.breaking_contracts > 0 {
            flags.push(format!("ломающих контрактов: {}", e.breaking_contracts));
        }
        if e.adrs > 0 {
            flags.push(format!("ADR: {}", e.adrs));
        }
        let _ = writeln!(
            out,
            "  {:<8} {}..{} · {}\n           {}",
            e.route,
            short_rev(&e.base),
            short_rev(&e.head),
            flags.join(" · "),
            e.rel
        );
    }
    if !report.skipped.is_empty() {
        let _ = writeln!(out, "\nПропущены (предупреждения):");
        for s in &report.skipped {
            let _ = writeln!(out, "  [warn] {} — {}", s.rel, s.reason);
        }
    }
    out
}

/// Машинный отчёт ящика (`--format json`), схема `arch-be/review-inbox/v1`.
#[must_use]
pub fn to_json(report: &InboxReport) -> Value {
    let entries: Vec<Value> = report
        .entries
        .iter()
        .map(|e| {
            json!({
                "file": e.rel,
                "base": e.base,
                "head": e.head,
                "route": e.route,
                "added_edges": e.added_edges,
                "removed_edges": e.removed_edges,
                "added_nodes": e.added_nodes,
                "removed_nodes": e.removed_nodes,
                "invariants_touched": e.invariants,
                "undeclared_edges": e.undeclared_edges,
                "breaking_contracts": e.breaking_contracts,
                "adrs_touched": e.adrs,
            })
        })
        .collect();
    let skipped: Vec<Value> = report
        .skipped
        .iter()
        .map(|s| json!({"file": s.rel, "reason": s.reason}))
        .collect();
    json!({
        "schema": REVIEW_INBOX_SCHEMA,
        "root": report.root.display().to_string(),
        "non_empty": report.entries.len(),
        "empty_filtered": report.empty,
        "skipped": skipped,
        "entries": entries,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Пишет файл фикстуры (родители создаются).
    fn write(root: &Path, rel: &str, text: &str) {
        let p = root.join(rel);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).expect("mkdir");
        }
        std::fs::write(p, text).expect("write");
    }

    /// Минимальный артефакт arch-diff/v1: пустой либо с одним добавленным
    /// ребром (route/invariants — по параметрам).
    fn artifact(route: &str, invariants: usize, undeclared: bool) -> String {
        let edges = if undeclared {
            json!([{"from": "CMP-001", "to": "CMP-004", "kind": "import",
                    "evidence": ["skeleton/intake/writer.py:2"],
                    "model_status": "not_in_model"}])
        } else {
            json!([])
        };
        let inv: Vec<Value> = (0..invariants)
            .map(|i| {
                json!({"ad_id": format!("AD-{i}"), "ad_title": "t",
                            "via_components": [], "rules": []})
            })
            .collect();
        serde_json::to_string_pretty(&json!({
            "schema": ARCH_DIFF_SCHEMA,
            "arch_be": "0.3.14",
            "base": "aaaa1111bbbb2222cccc3333dddd4444",
            "head": "eeee5555ffff66667777888899990000",
            "added_nodes": [],
            "removed_nodes": [],
            "added_edges": edges,
            "removed_edges": [],
            "declared_unused": [],
            "invariants_touched": inv,
            "adrs_touched": [],
            "contract_changes": [],
            "nfr_shifts": [],
            "route": {"route": route, "score": 1, "triggers": [], "evidence": [], "undeclared": []},
            "proposals": [],
        }))
        .expect("json")
    }

    /// Ящик: пустой дифф отфильтрован, непустые отсортированы по маршруту и
    /// числу инвариантов, битый JSON — предупреждение и пропуск, чужая схема
    /// — пропуск, журнал решений в dot-каталоге не подхватывается.
    #[test]
    fn inbox_filters_sorts_and_tolerates_broken_files() {
        let tmp = tempfile::tempdir().expect("tmp");
        let root = tmp.path();
        // Непустые: standard (1 инвариант) и critical (2 инварианта).
        write(
            root,
            "repo-a/arch-diff.json",
            &artifact("standard", 1, true),
        );
        write(
            root,
            "repo-b/arch-diff-pr7.json",
            &artifact("critical", 2, true),
        );
        // Пустой дифф — в ящик не попадает.
        write(root, "repo-c/arch-diff.json", &artifact("fast", 0, false));
        // Битый JSON — предупреждение и пропуск.
        write(root, "repo-d/arch-diff.json", "{битый");
        // Чужая схема (журнал решений K5) — пропуск с причиной.
        write(
            root,
            "repo-e/.arch-handoff/arch-diff-decisions.json",
            "{\"schema\": \"arch-be/arch-diff-decisions/v1\", \"decisions\": []}",
        );
        // Посторонний JSON — не кандидат по имени, игнорируется молча.
        write(root, "repo-f/report.json", "{\"schema\": \"other\"}");

        let report = scan(root).expect("scan");
        assert_eq!(report.entries.len(), 2, "{:?}", report.entries);
        assert_eq!(report.empty, 1, "пустой дифф отфильтрован");
        assert_eq!(report.skipped.len(), 1, "битый JSON — единственный пропуск");
        assert!(
            report.skipped[0].reason.contains("не JSON"),
            "{:?}",
            report.skipped
        );
        // Сортировка: critical выше standard.
        assert_eq!(report.entries[0].route, "critical");
        assert_eq!(report.entries[1].route, "standard");
        assert_eq!(report.entries[0].invariants, 2);
        assert_eq!(report.entries[1].undeclared_edges, 1);
        // dot-каталоги не читаются: журнал решений не попал ни в записи,
        // ни в пропуски.
        assert!(
            !report
                .skipped
                .iter()
                .any(|s| s.rel.contains("arch-diff-decisions")),
            "{:?}",
            report.skipped
        );
    }

    /// Равный маршрут и инварианты — порядок по относительному пути
    /// (детерминизм повторного прогона).
    #[test]
    fn inbox_tie_breaks_by_path() {
        let tmp = tempfile::tempdir().expect("tmp");
        let root = tmp.path();
        write(root, "b/arch-diff.json", &artifact("standard", 1, true));
        write(root, "a/arch-diff.json", &artifact("standard", 1, true));
        let report = scan(root).expect("scan");
        let rels: Vec<&str> = report.entries.iter().map(|e| e.rel.as_str()).collect();
        assert_eq!(rels, vec!["a/arch-diff.json", "b/arch-diff.json"]);
    }

    /// Файл схемы v1, но с минимумом полей (толерантный ридер: отсутствующие
    /// поля — пустые/нулевые), непустой по одному ребру — попадает в ящик.
    #[test]
    fn tolerant_reader_defaults_missing_fields() {
        let tmp = tempfile::tempdir().expect("tmp");
        let root = tmp.path();
        write(
            root,
            "repo/arch-diff.json",
            "{\"schema\": \"arch-be/arch-diff/v1\", \"added_edges\": [{}]}",
        );
        let report = scan(root).expect("scan");
        assert_eq!(report.entries.len(), 1);
        let e = &report.entries[0];
        assert_eq!(e.route, "unknown");
        assert_eq!(e.route_rank, 0);
        assert_eq!(e.base, "?");
        assert_eq!(e.added_edges, 1);
        assert_eq!(e.invariants, 0);
    }

    /// Корень не каталог — явная ошибка (fail-closed), а не пустой ящик.
    #[test]
    fn missing_root_is_an_error() {
        let tmp = tempfile::tempdir().expect("tmp");
        let err = scan(&tmp.path().join("нет")).expect_err("ошибка");
        assert!(err.to_string().contains("недоступен"), "{err}");
    }

    /// Рендер: сводка отбора, строка записи с флагами, предупреждения.
    #[test]
    fn render_shows_summary_entries_and_warnings() {
        let tmp = tempfile::tempdir().expect("tmp");
        let root = tmp.path();
        write(
            root,
            "repo-a/arch-diff.json",
            &artifact("critical", 2, true),
        );
        write(root, "repo-b/arch-diff.json", "{битый");
        let report = scan(root).expect("scan");
        let text = render(&report);
        assert!(text.contains("Непустых диффов: 1"), "{text}");
        assert!(text.contains("пустых отфильтровано: 0"), "{text}");
        assert!(text.contains("пропущено с предупреждением: 1"), "{text}");
        assert!(text.contains("critical"), "{text}");
        assert!(text.contains("инварианты: 2"), "{text}");
        assert!(text.contains("рёбер вне модели: 1"), "{text}");
        assert!(
            text.contains("[warn] repo-b/arch-diff.json — не JSON"),
            "{text}"
        );
        // Пустой ящик — честное «ревью не требуется».
        let empty = scan(tempfile::tempdir().expect("tmp").path()).expect("scan");
        assert!(render(&empty).contains("ревью не требуется"));
    }

    /// JSON-отчёт: схема, счётчики, запись с полями.
    #[test]
    fn json_report_carries_schema_and_counts() {
        let tmp = tempfile::tempdir().expect("tmp");
        let root = tmp.path();
        write(
            root,
            "repo-a/arch-diff.json",
            &artifact("standard", 1, true),
        );
        write(root, "repo-b/arch-diff.json", &artifact("fast", 0, false));
        let report = scan(root).expect("scan");
        let v = to_json(&report);
        assert_eq!(v["schema"], REVIEW_INBOX_SCHEMA);
        assert_eq!(v["non_empty"], 1);
        assert_eq!(v["empty_filtered"], 1);
        let entry = &v["entries"][0];
        assert_eq!(entry["route"], "standard");
        assert_eq!(entry["invariants_touched"], 1);
        assert_eq!(entry["undeclared_edges"], 1);
        assert_eq!(entry["file"], "repo-a/arch-diff.json");
    }
}
