//! Fail-closed автопроверка модели отчёта (ADR-080, этап 1).
//!
//! Три инварианта:
//!
//! - **и1** (основания): у каждого элемента уверенность A/B требует непустого
//!   evidence; пустое — `error` (выдача блокируется).
//! - **и2** (ссылки): каждый report-ID в `relations`/`section.element_ids`
//!   существует в `elements`; элемент без вхождений ни в один раздел —
//!   `warn orphan_element` (не блокирует).
//! - **и3** (адреса): каждая ссылка-доказательство `путь:строка` разрешается —
//!   путь есть в дереве ревизии, а номер строки не выходит за пределы файла;
//!   сломанная ссылка — `error`.
//!
//! Байпас-флага нет: любая находка `error` блокирует рендер (ненулевой код).

use std::collections::BTreeSet;
use std::path::Path;

use crate::arch_diff::Snapshot;
use crate::report::model::{CheckFinding, CheckSeverity, ReportModel};

/// Инвариант и1 — основания уверенности.
const INV_EVIDENCE: &str = "и1";
/// Инвариант и2 — целостность ссылок report-ID.
const INV_REFERENCE: &str = "и2";
/// Инвариант и3 — разрешимость адресов `файл:строка`.
const INV_LOCATION: &str = "и3";

/// Прогон автопроверки модели: находки по инвариантам и1/и2/и3.
///
/// Пустой результат — модель корректна; любая находка [`CheckSeverity::Error`]
/// блокирует выдачу. `snap` — снимок ревизии (источник содержимого для и3),
/// `repo` — рабочее дерево (резерв для файлов вне снимка).
#[must_use]
pub fn check(model: &ReportModel, snap: &Snapshot, repo: &Path) -> Vec<CheckFinding> {
    let mut findings = Vec::new();
    let ids: BTreeSet<&str> = model.elements.iter().map(|e| e.id.as_str()).collect();

    check_elements(model, snap, repo, &mut findings);
    check_relations(model, snap, repo, &ids, &mut findings);
    check_sections(model, &ids, &mut findings);

    findings
}

/// и1 и и3 по элементам: основания и разрешимость адресов.
fn check_elements(
    model: &ReportModel,
    snap: &Snapshot,
    repo: &Path,
    findings: &mut Vec<CheckFinding>,
) {
    for e in &model.elements {
        if e.confidence.requires_evidence() && e.evidence.is_empty() {
            findings.push(CheckFinding {
                invariant: INV_EVIDENCE.to_string(),
                severity: CheckSeverity::Error,
                message: format!(
                    "элемент {} ({}) без основания: уверенность {} требует непустого evidence",
                    e.id,
                    e.title,
                    e.confidence.as_str()
                ),
                element_id: Some(e.id.clone()),
            });
        }
        for r in &e.evidence {
            if let Some(why) = resolve_ref(repo, snap, r) {
                findings.push(CheckFinding {
                    invariant: INV_LOCATION.to_string(),
                    severity: CheckSeverity::Error,
                    message: format!("элемент {}: ссылка `{r}` не разрешается — {why}", e.id),
                    element_id: Some(e.id.clone()),
                });
            }
        }
    }
}

/// и2/и3 по связям: обе стороны существуют, адреса разрешаются.
fn check_relations(
    model: &ReportModel,
    snap: &Snapshot,
    repo: &Path,
    ids: &BTreeSet<&str>,
    findings: &mut Vec<CheckFinding>,
) {
    for rel in &model.relations {
        for (side, id) in [("from", &rel.from), ("to", &rel.to)] {
            if !ids.contains(id.as_str()) {
                findings.push(CheckFinding {
                    invariant: INV_REFERENCE.to_string(),
                    severity: CheckSeverity::Error,
                    message: format!("связь {side}={id} ссылается на несуществующий report-ID"),
                    element_id: None,
                });
            }
        }
        for r in &rel.evidence {
            if let Some(why) = resolve_ref(repo, snap, r) {
                findings.push(CheckFinding {
                    invariant: INV_LOCATION.to_string(),
                    severity: CheckSeverity::Error,
                    message: format!(
                        "связь {}→{}: ссылка `{r}` не разрешается — {why}",
                        rel.from, rel.to
                    ),
                    element_id: None,
                });
            }
        }
    }
}

/// и2 по разделам: ссылки существуют, каждый элемент где-то упомянут.
fn check_sections(model: &ReportModel, ids: &BTreeSet<&str>, findings: &mut Vec<CheckFinding>) {
    let mut referenced: BTreeSet<&str> = BTreeSet::new();
    for s in &model.sections {
        for id in &s.element_ids {
            if ids.contains(id.as_str()) {
                referenced.insert(id.as_str());
            } else {
                findings.push(CheckFinding {
                    invariant: INV_REFERENCE.to_string(),
                    severity: CheckSeverity::Error,
                    message: format!(
                        "раздел §{} ссылается на несуществующий report-ID {id}",
                        s.id
                    ),
                    element_id: Some(id.clone()),
                });
            }
        }
    }
    for e in &model.elements {
        if !referenced.contains(e.id.as_str()) {
            findings.push(CheckFinding {
                invariant: INV_REFERENCE.to_string(),
                severity: CheckSeverity::Warn,
                message: format!(
                    "orphan_element: {} ({}) не упомянут ни в одном разделе",
                    e.id, e.title
                ),
                element_id: Some(e.id.clone()),
            });
        }
    }
}

/// Разрешение ссылки-доказательства `путь:строка` (или `путь/` — каталог).
///
/// Возвращает `None`, если ссылка разрешается, и `Some(причина)`, если она
/// сломана (и3). Проверка: путь есть в снимке ревизии либо в рабочем дереве
/// `repo`; если в снимке доступно содержимое и номер строки распознан — строк
/// в файле должно быть не меньше.
#[must_use]
pub fn resolve_ref(repo: &Path, snap: &Snapshot, reference: &str) -> Option<String> {
    let reference = reference.trim();
    if reference.is_empty() {
        return Some("пустая ссылка".to_string());
    }
    // Каталог: `путь/` — совпадение по префиксу среди файлов снимка.
    if let Some(dir) = reference.strip_suffix('/') {
        let prefix = format!("{dir}/");
        let in_snap = snap.files.iter().any(|f| f.starts_with(&prefix));
        if in_snap || repo.join(dir).is_dir() {
            return None;
        }
        return Some(format!("каталог `{dir}` не найден"));
    }
    // Файл: `путь` или `путь:строка` (строка — только завершающая цифровая группа).
    let (path, line) = split_path_line(reference);
    if path.is_empty() {
        return Some(format!("путь пуст в `{reference}`"));
    }
    let in_snap = snap.files.contains(path);
    let on_disk = repo.join(path).is_file();
    if !in_snap && !on_disk {
        return Some(format!("файл `{path}` не найден"));
    }
    if let (Some(line), Some(content)) = (line, snap.content(path)) {
        let count = content.lines().count();
        if line > count {
            return Some(format!(
                "строка {line} за пределами `{path}` (строк: {count})"
            ));
        }
    }
    None
}

/// Разбор `путь:строка`: строка — только если после последнего `:` идут цифры.
fn split_path_line(reference: &str) -> (&str, Option<usize>) {
    if let Some((head, tail)) = reference.rsplit_once(':') {
        if let Ok(n) = tail.parse::<usize>() {
            return (head, Some(n));
        }
    }
    (reference, None)
}
