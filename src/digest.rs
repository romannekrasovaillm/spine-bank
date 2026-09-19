//! Недельный дайджест outcome-данных MCP-контроля (`arch-be digest`,
//! пункт 9 бэклога волны) и регистр ложных срабатываний
//! (`arch-be control fp mark`).
//!
//! Дайджест заполняет протокол `docs/outcome-metrics.md` из данных, а не
//! вручную: журнал вызовов [`crate::mcp_journal`] (итерации FAIL→PASS,
//! топ нарушаемых правил, длительности), регистр FP
//! `evidence/fp-register.md` (доля ложных срабатываний) и `CONSTRAINTS.yaml`
//! (истекающие overrides и expiry правил).
//!
//! Честность метода (важно для протокола): доля FP — ОРИЕНТИРОВОЧНАЯ.
//! Пометка FP в регистре не привязана к конкретному вызову, а fail-вызов
//! инструмента может нести несколько находок: делим пометки за окно на
//! fail-вызовы за окно — это прокси «помеченные/находки недели», а не
//! точная доля. Цель < 10% — по `docs/outcome-metrics.md` §2.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{HarnessError, Result};
use crate::mcp_journal::{self, JournalEntry};

/// Относительный путь регистра ложных срабатываний в проекте
/// (протокол `docs/outcome-metrics.md` §2).
const FP_REGISTER_REL: &str = "evidence/fp-register.md";

/// Горизонт «истекающих» сроков в дайджесте, дней: overrides (`until`) и
/// expiry правил, наступающие в пределах двух недель (или уже просроченные).
const EXPIRING_HORIZON_DAYS: i64 = 14;

/// Размер топа нарушаемых правил в дайджесте.
const TOP_FAILED_RULES: usize = 10;

/// Окно дайджеста по умолчанию (неделя), дней.
pub const DEFAULT_WINDOW_DAYS: u32 = 7;

// --- Регистр ложных срабатываний (FP) -------------------------------------

/// Запись регистра FP: строка таблицы markdown `| дата | правило | файл | примечание |`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FpMark {
    /// Дата пометки.
    pub date: chrono::NaiveDate,
    /// Имя правила.
    pub rule: String,
    /// Файл (обычно `путь:строка`).
    pub file: String,
    /// Примечание (причина/решение), может быть пустым.
    pub note: String,
}

/// Санитизация поля таблицы markdown: `|` и переводы строк ломают таблицу.
fn md_cell(text: &str) -> String {
    text.replace('|', "/")
        .replace(['\n', '\r'], " ")
        .trim()
        .to_string()
}

/// Добавляет пометку ложного срабатывания в `evidence/fp-register.md`
/// проекта: файл создаётся с шапкой таблицы при отсутствии, запись —
/// append строкой `| дата | правило | файл | примечание |`.
///
/// # Errors
/// Каталог `evidence/` не создаётся, файл не пишется.
pub fn fp_register_mark(
    repo: &Path,
    rule: &str,
    file: &str,
    note: Option<&str>,
) -> Result<PathBuf> {
    let path = repo.join(FP_REGISTER_REL);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| HarnessError::io(dir, e))?;
    }
    if !path.is_file() {
        std::fs::write(
            &path,
            "# Регистр ложных срабатываний правил (FP)\n\n\
             Метод — `docs/outcome-metrics.md` §2: правило сработало на корректный код\n\
             по ошибке эвристики. Пометки добавляет `arch-be control fp mark`.\n\n\
             | Дата | Правило | Файл | Примечание |\n\
             |---|---|---|---|\n",
        )
        .map_err(|e| HarnessError::io(&path, e))?;
    }
    let today = chrono::Local::now().date_naive();
    let row = format!(
        "| {} | {} | {} | {} |\n",
        today,
        md_cell(rule),
        md_cell(file),
        note.map_or_else(|| "—".to_string(), md_cell),
    );
    let mut f = std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .map_err(|e| HarnessError::io(&path, e))?;
    f.write_all(row.as_bytes())
        .map_err(|e| HarnessError::io(&path, e))?;
    Ok(path)
}

/// Читает регистр FP: строки таблицы с валидной датой; шапка и битые строки
/// пропускаются. Отсутствующий файл — пустой список (пометок ещё не было).
#[must_use]
pub fn fp_register_read(path: &Path) -> Vec<FpMark> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    text.lines()
        .filter_map(|line| {
            let cells: Vec<&str> = line
                .trim()
                .strip_prefix('|')?
                .trim_end_matches('|')
                .split('|')
                .map(str::trim)
                .collect();
            if cells.len() != 4 {
                return None;
            }
            let date = chrono::NaiveDate::parse_from_str(cells[0], "%Y-%m-%d").ok()?;
            Some(FpMark {
                date,
                rule: cells[1].to_string(),
                file: cells[2].to_string(),
                note: cells[3].to_string(),
            })
        })
        .collect()
}

// --- Дайджест -------------------------------------------------------------

/// Истекающий срок: override (`until`) или expiry правила.
#[derive(Debug, Clone, Serialize)]
pub struct ExpiringItem {
    /// `override` | `expiry`.
    pub kind: String,
    /// Правило.
    pub rule: String,
    /// Дата срока (YYYY-MM-DD; у месячной формы override — последний день
    /// месяца, семантика `control.rs::parse_until`).
    pub until: String,
    /// Дней до срока (отрицательное — уже просрочен).
    pub days_left: i64,
    /// ADR (у override) или владелец (у expiry), если заданы.
    pub detail: Option<String>,
}

/// Дайджест outcome-данных за окно (JSON-контракт `arch-be digest --json`).
#[derive(Debug, Clone, Serialize)]
pub struct DigestReport {
    /// Репозиторий.
    pub repo: PathBuf,
    /// Окно в днях.
    pub window_days: u32,
    /// Начало окна (включительно, RFC 3339).
    pub from: String,
    /// Конец окна (момент построения, RFC 3339).
    pub to: String,
    /// Вызовов MCP в окне.
    pub calls_total: usize,
    /// Вызовы по инструментам.
    pub calls_by_tool: BTreeMap<String, usize>,
    /// Вызовы по вердиктам (pass/fail/ok/error/invalid).
    pub verdicts: BTreeMap<String, usize>,
    /// Итерации FAIL→PASS по инструментам (пары соседних fail→pass
    /// в журнале; только инструменты с ненулевым счётом).
    pub fail_pass_iterations: BTreeMap<String, usize>,
    /// Топ нарушаемых правил (имена из error-находок fail-вызовов).
    pub top_failed_rules: Vec<(String, usize)>,
    /// Пометок FP в регистре за окно.
    pub fp_marks: usize,
    /// Fail-вызовов за окно (знаменатель ориентировочной доли FP).
    pub fail_calls: usize,
    /// Ориентировочная доля FP, % (None — fail-вызовов не было, долю
    /// считать не из чего). Метод — см. документацию модуля.
    pub fp_share_pct: Option<f64>,
    /// Истекающие overrides и expiry (горизонт [`EXPIRING_HORIZON_DAYS`]).
    pub expiring: Vec<ExpiringItem>,
    /// Путь журнала, из которого построен дайджест.
    pub journal_path: PathBuf,
    /// Путь регистра FP.
    pub fp_register_path: PathBuf,
    /// Файл ограничений, из которого прочитаны сроки (None — не найден).
    pub constraints_path: Option<PathBuf>,
    /// Записей журнала пропущено (битый JSON или непарсящийся штамп
    /// времени) — честный счётчик потерь окна.
    pub skipped: usize,
}

/// Срез `CONSTRAINTS.yaml` для дайджеста: только сроки (expiry правил и
/// until overrides). Дайджест — читатель, не enforce'ер: своя минимальная
/// схема вместо внутренних типов движка (`FitnessRule`), чтобы не тащить
/// в отчётный модуль механику прогона правил.
#[derive(Debug, Deserialize)]
struct ConstraintsSlice {
    /// Корень `rules:`.
    #[serde(default)]
    rules: Vec<RuleSlice>,
    /// Альтернативный корень `constraints:`.
    #[serde(default)]
    constraints: Vec<RuleSlice>,
    /// Исключения `overrides:`.
    #[serde(default)]
    overrides: Vec<OverrideSlice>,
}

/// Срез правила: имя, владелец, срок пересмотра.
#[derive(Debug, Deserialize)]
struct RuleSlice {
    /// Имя правила.
    name: String,
    /// Владелец.
    #[serde(default)]
    owner: Option<String>,
    /// Срок пересмотра (YYYY-MM-DD).
    #[serde(default)]
    expiry: Option<String>,
}

/// Срез override: правило, ADR, срок.
#[derive(Debug, Deserialize)]
struct OverrideSlice {
    /// Имя правила.
    rule: String,
    /// ADR-основание.
    #[serde(default)]
    adr: Option<String>,
    /// Срок (YYYY-MM-DD или YYYY-MM — по месяцу включительно).
    #[serde(default)]
    until: Option<String>,
}

/// Последний день месяца (для месячной формы `until`: срок действует по
/// указанный месяц включительно — как `control.rs::until_expired`).
fn last_day_of_month(y: i32, m: u32) -> Option<chrono::NaiveDate> {
    let (ny, nm) = if m == 12 { (y + 1, 1) } else { (y, m + 1) };
    chrono::NaiveDate::from_ymd_opt(ny, nm, 1).and_then(|d| d.pred_opt())
}

/// Дата срока из строки `YYYY-MM-DD` или `YYYY-MM` (месячная форма —
/// последний день месяца).
fn parse_due_date(raw: &str) -> Option<chrono::NaiveDate> {
    let raw = raw.trim();
    if let Ok(date) = chrono::NaiveDate::parse_from_str(raw, "%Y-%m-%d") {
        return Some(date);
    }
    let (y, m) = raw.split_once('-')?;
    if y.len() != 4 || m.len() != 2 {
        return None;
    }
    last_day_of_month(y.parse().ok()?, m.parse().ok()?)
}

/// Собирает истекающие сроки из `CONSTRAINTS.yaml`: expiry правил и until
/// overrides в горизонте [`EXPIRING_HORIZON_DAYS`] (включая просроченные).
/// Невалидные даты пропускаются (поле метаданное — как в `control.rs`).
fn collect_expiring(
    constraints: &Path,
    today: chrono::NaiveDate,
    horizon_days: i64,
) -> Result<Vec<ExpiringItem>> {
    let yaml =
        std::fs::read_to_string(constraints).map_err(|e| HarnessError::io(constraints, e))?;
    let parsed: ConstraintsSlice = serde_yaml_ng::from_str(&yaml)?;
    let mut out = Vec::new();
    let mut push = |kind: &str, rule: &str, until_raw: &str, detail: Option<String>| {
        let Some(due) = parse_due_date(until_raw) else {
            return;
        };
        let days_left = (due - today).num_days();
        if days_left <= horizon_days {
            out.push(ExpiringItem {
                kind: kind.to_string(),
                rule: rule.to_string(),
                until: due.to_string(),
                days_left,
                detail,
            });
        }
    };
    for rule in parsed.rules.iter().chain(parsed.constraints.iter()) {
        if let Some(expiry) = &rule.expiry {
            push("expiry", &rule.name, expiry, rule.owner.clone());
        }
    }
    for o in &parsed.overrides {
        if let Some(until) = &o.until {
            push("override", &o.rule, until, o.adr.clone());
        }
    }
    out.sort_by(|a, b| {
        a.days_left
            .cmp(&b.days_left)
            .then_with(|| a.rule.cmp(&b.rule))
    });
    Ok(out)
}

/// Файл ограничений проекта: `.arch-handoff/CONSTRAINTS.yaml`, иначе
/// корневой `CONSTRAINTS.yaml`; `None` — ни одного нет (сроки не читаем,
/// это честное состояние, а не ошибка).
fn project_constraints(repo: &Path) -> Option<PathBuf> {
    let handoff = repo.join(".arch-handoff/CONSTRAINTS.yaml");
    if handoff.is_file() {
        return Some(handoff);
    }
    let root = repo.join("CONSTRAINTS.yaml");
    root.is_file().then_some(root)
}

/// Строит дайджест за `days` дней, отсчитанных от `now`.
///
/// # Errors
/// `repo` не каталог; файл ограничений не читается/невалиден.
fn build_at(repo: &Path, days: u32, now: chrono::DateTime<chrono::Local>) -> Result<DigestReport> {
    if !repo.is_dir() {
        return Err(HarnessError::Control(format!(
            "дайджест: репозиторий недоступен: {}",
            repo.display()
        )));
    }
    let from = now - chrono::Duration::days(i64::from(days));
    let journal_path = mcp_journal::journal_path(repo);
    let mut entries: Vec<(chrono::DateTime<chrono::FixedOffset>, JournalEntry)> = Vec::new();
    let mut skipped = 0usize;
    for entry in mcp_journal::read_entries(&journal_path) {
        match chrono::DateTime::parse_from_rfc3339(&entry.ts) {
            Ok(ts) if ts >= from => entries.push((ts, entry)),
            // Вне окна или битый штамп — в счётчик потерь, не в метрики.
            _ => skipped += 1,
        }
    }
    // Журнал append-only и почти монотонен; стабильная сортировка — страховка
    // от записей «назад во времени» (часы машины, внешние правки файла).
    entries.sort_by_key(|a| a.0);

    let mut calls_by_tool: BTreeMap<String, usize> = BTreeMap::new();
    let mut verdicts: BTreeMap<String, usize> = BTreeMap::new();
    let mut rule_counts: BTreeMap<String, usize> = BTreeMap::new();
    let mut last_verdict: BTreeMap<String, &str> = BTreeMap::new();
    let mut fail_pass: BTreeMap<String, usize> = BTreeMap::new();
    let mut fail_calls = 0usize;
    for (_, entry) in &entries {
        *calls_by_tool.entry(entry.tool.clone()).or_default() += 1;
        *verdicts.entry(entry.verdict.clone()).or_default() += 1;
        if entry.verdict == "fail" {
            fail_calls += 1;
            for rule in &entry.rules {
                *rule_counts.entry(rule.clone()).or_default() += 1;
            }
        }
        // Пара «соседний fail → pass» для этого инструмента = одна итерация
        // «починил находки и перепроверил».
        if entry.verdict == "pass" && last_verdict.get(entry.tool.as_str()) == Some(&"fail") {
            *fail_pass.entry(entry.tool.clone()).or_default() += 1;
        }
        last_verdict.insert(entry.tool.clone(), entry.verdict.as_str());
    }
    let mut top_failed_rules: Vec<(String, usize)> = rule_counts.into_iter().collect();
    top_failed_rules.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    top_failed_rules.truncate(TOP_FAILED_RULES);

    let fp_register_path = repo.join(FP_REGISTER_REL);
    let (from_date, to_date) = (from.date_naive(), now.date_naive());
    let fp_marks = fp_register_read(&fp_register_path)
        .into_iter()
        .filter(|m| m.date >= from_date && m.date <= to_date)
        .count();
    let fp_share_pct = (fail_calls > 0).then(|| (fp_marks as f64 / fail_calls as f64) * 100.0);

    let constraints_path = project_constraints(repo);
    let expiring = match &constraints_path {
        Some(path) => collect_expiring(path, to_date, EXPIRING_HORIZON_DAYS)?,
        None => Vec::new(),
    };

    Ok(DigestReport {
        repo: repo.to_path_buf(),
        window_days: days,
        from: from.to_rfc3339(),
        to: now.to_rfc3339(),
        calls_total: entries.len(),
        calls_by_tool,
        verdicts,
        fail_pass_iterations: fail_pass,
        top_failed_rules,
        fp_marks,
        fail_calls,
        fp_share_pct,
        expiring,
        journal_path,
        fp_register_path,
        constraints_path,
        skipped,
    })
}

/// Строит дайджест за последние `days` дней (от текущего момента).
///
/// # Errors
/// Как у [`build_at`].
pub fn build(repo: &Path, days: u32) -> Result<DigestReport> {
    build_at(repo, days, chrono::Local::now())
}

/// Рендерит дайджест в markdown (пользовательский вывод `arch-be digest`).
#[must_use]
pub fn render_markdown(report: &DigestReport) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "# Дайджест MCP-контроля: {} → {} ({} дн.)\n",
        report.from, report.to, report.window_days
    );

    let _ = writeln!(out, "## Вызовы MCP\n");
    if report.calls_total == 0 {
        let _ = writeln!(
            out,
            "Вызовов за окно нет (журнал {} пуст или отсутствует).",
            report.journal_path.display()
        );
    } else {
        let verdicts = report
            .verdicts
            .iter()
            .map(|(v, n)| format!("{v} {n}"))
            .collect::<Vec<_>>()
            .join(", ");
        let _ = writeln!(out, "Всего вызовов: {} ({verdicts})", report.calls_total);
        let tools = report
            .calls_by_tool
            .iter()
            .map(|(t, n)| format!("{t} {n}"))
            .collect::<Vec<_>>()
            .join(", ");
        let _ = writeln!(out, "По инструментам: {tools}");
    }
    if report.skipped > 0 {
        let _ = writeln!(
            out,
            "\nПропущено записей журнала (вне окна или битый штамп): {}",
            report.skipped
        );
    }

    // Секция для управления: квитанция ценности гейта — дефекты, которые
    // механика остановила до ревью (fail-вердикты журнала), с разложением
    // по правилам и ориентировочной долей FP (метод — §2 протокола).
    let _ = writeln!(out, "\n## Дефекты, не дошедшие до ревью\n");
    if report.fail_calls == 0 {
        let _ = writeln!(
            out,
            "Fail-вызовов за окно не было — данных о пойманных дефектах нет \
             (это отсутствие сигнала, а не доказанный «ноль дефектов»)."
        );
    } else {
        let fixed: usize = report.fail_pass_iterations.values().sum();
        let _ = writeln!(
            out,
            "Гейт остановил до ревью fail-вызовов: {}; из них исправлены и \
             перепроверены (итерации FAIL→PASS): {fixed}.",
            report.fail_calls
        );
        if !report.top_failed_rules.is_empty() {
            let rules = report
                .top_failed_rules
                .iter()
                .map(|(r, n)| format!("{r} {n}"))
                .collect::<Vec<_>>()
                .join(", ");
            let _ = writeln!(out, "Fail-вердикты по правилам: {rules}.");
        }
        match report.fp_share_pct {
            Some(pct) => {
                let _ = writeln!(
                    out,
                    "Ориентировочная доля FP среди них: {pct:.1}% (цель < 10% — \
                     метод и оговорки в секции «Ложные срабатывания» ниже)."
                );
            }
            None => {
                let _ = writeln!(out, "Доля FP не считается (нет знаменателя).");
            }
        }
    }

    let _ = writeln!(out, "\n## Итерации FAIL→PASS\n");
    if report.fail_pass_iterations.is_empty() {
        let _ = writeln!(out, "- нет");
    } else {
        for (tool, n) in &report.fail_pass_iterations {
            let _ = writeln!(out, "- {tool}: {n}");
        }
    }

    let _ = writeln!(
        out,
        "\n## Топ нарушаемых правил (error-находки fail-вызовов)\n"
    );
    if report.top_failed_rules.is_empty() {
        let _ = writeln!(out, "- нет");
    } else {
        for (rule, n) in &report.top_failed_rules {
            let _ = writeln!(out, "- {rule}: {n}");
        }
    }

    let _ = writeln!(
        out,
        "\n## Ложные срабатывания (регистр {})\n",
        report.fp_register_path.display()
    );
    match report.fp_share_pct {
        Some(pct) => {
            let _ = writeln!(
                out,
                "Пометок FP за окно: {}; fail-вызовов за окно: {} → ориентировочная доля FP: {pct:.1}%",
                report.fp_marks, report.fail_calls
            );
            let _ = writeln!(
                out,
                "Метод: пометки регистра / fail-вызовы (один вызов может нести несколько находок, \
                 пометка может относиться к находке вне окна — оценка грубая; цель < 10% \
                 по docs/outcome-metrics.md §2)."
            );
        }
        None => {
            let _ = writeln!(
                out,
                "Пометок FP за окно: {}; fail-вызовов не было — доля не считается (нет знаменателя).",
                report.fp_marks
            );
        }
    }

    let _ = writeln!(
        out,
        "\n## Истекающие overrides и expiry (горизонт {EXPIRING_HORIZON_DAYS} дн.)\n"
    );
    if report.constraints_path.is_none() {
        let _ = writeln!(out, "- файл ограничений не найден — сроки не читались");
    } else if report.expiring.is_empty() {
        let _ = writeln!(out, "- нет");
    } else {
        for item in &report.expiring {
            let when = if item.days_left < 0 {
                format!("просрочен {} дн. назад", -item.days_left)
            } else {
                format!("истекает через {} дн.", item.days_left)
            };
            let detail = item
                .detail
                .as_deref()
                .map_or_else(String::new, |d| format!(" ({d})"));
            let _ = writeln!(
                out,
                "- [{}] {} — {}, {when}{detail}",
                item.kind, item.rule, item.until
            );
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Штамп RFC 3339 для фикстур журнала.
    fn ts(day: u32, hour: u32) -> String {
        format!("2026-09-{day:02}T{hour:02}:00:00+03:00")
    }

    /// Строка записи журнала.
    fn line(ts: &str, tool: &str, verdict: &str, rules: &[&str]) -> String {
        let rules = rules
            .iter()
            .map(|r| format!("\"{r}\""))
            .collect::<Vec<_>>()
            .join(",");
        format!(
            "{{\"ts\":\"{ts}\",\"tool\":\"{tool}\",\"verdict\":\"{verdict}\",\"duration_ms\":5,\"rules\":[{rules}]}}"
        )
    }

    /// Пишет журнал фикстуры в `<repo>/.arch-handoff/mcp-calls.jsonl`.
    fn write_journal(repo: &Path, lines: &[String]) {
        let dir = repo.join(".arch-handoff");
        std::fs::create_dir_all(&dir).expect("mkdir");
        std::fs::write(dir.join("mcp-calls.jsonl"), lines.join("\n") + "\n").expect("журнал");
    }

    /// Фиксированный «сейчас» дайджеста: 2026-09-18 12:00 +03:00.
    fn fixed_now() -> chrono::DateTime<chrono::Local> {
        chrono::DateTime::parse_from_rfc3339("2026-09-18T12:00:00+03:00")
            .expect("валидный штамп")
            .with_timezone(&chrono::Local)
    }

    #[test]
    fn fp_mark_creates_register_and_appends_rows() {
        let tmp = tempfile::tempdir().expect("tmp");
        let path = fp_register_mark(
            tmp.path(),
            "no-pan",
            "src/a.rs:10",
            Some("строковый литерал"),
        )
        .expect("первая пометка");
        fp_register_mark(tmp.path(), "msrv_pinned", "Cargo.toml", None).expect("вторая пометка");
        let marks = fp_register_read(&path);
        assert_eq!(marks.len(), 2);
        assert_eq!(marks[0].rule, "no-pan");
        assert_eq!(marks[0].file, "src/a.rs:10");
        assert_eq!(marks[0].note, "строковый литерал");
        assert_eq!(marks[1].note, "—");
        // Поля с `|` и переводами строк не ломают таблицу.
        fp_register_mark(tmp.path(), "r|x", "f\ny", None).expect("санитизация");
        let marks = fp_register_read(&path);
        assert_eq!(marks.len(), 3);
        assert_eq!(marks[2].rule, "r/x");
        assert_eq!(marks[2].file, "f y");
    }

    #[test]
    fn digest_counts_iterations_rules_and_fp_share() {
        let tmp = tempfile::tempdir().expect("tmp");
        write_journal(
            tmp.path(),
            &[
                line(&ts(12, 9), "fitness_check", "fail", &["no-pan"]),
                line(
                    &ts(12, 10),
                    "fitness_check",
                    "fail",
                    &["no-pan", "msrv_pinned"],
                ),
                line(&ts(12, 11), "fitness_check", "pass", &[]),
                line(&ts(13, 9), "spine_lint", "fail", &["no-pan"]),
                line(&ts(13, 10), "spine_lint", "pass", &[]),
                line(&ts(14, 9), "kb_search", "ok", &[]),
                line(&ts(14, 10), "fitness_check", "error", &[]),
                // Вне недельного окна (9 сентября) — не должен попасть.
                line(&ts(9, 9), "fitness_check", "fail", &["old-rule"]),
            ],
        );
        let report = build_at(tmp.path(), 7, fixed_now()).expect("дайджест");
        assert_eq!(report.calls_total, 7);
        assert_eq!(report.calls_by_tool.get("fitness_check"), Some(&4));
        assert_eq!(report.fail_pass_iterations.get("fitness_check"), Some(&1));
        assert_eq!(report.fail_pass_iterations.get("spine_lint"), Some(&1));
        // no-pan: 3 попадания (две fail fitness + одна fail spine_lint).
        assert_eq!(
            report.top_failed_rules.first(),
            Some(&("no-pan".to_string(), 3))
        );
        assert!(!report.top_failed_rules.iter().any(|(r, _)| r == "old-rule"));
        assert_eq!(report.fail_calls, 3);
        // Две пометки FP: одна в окне, одна вне. Даты пишем фиксированные —
        // окно дайджеста закрыто fixed_now() (2026-09-18 12:00), поэтому
        // реальное «сегодня» из fp_register_mark в это окно попадает не
        // всегда (тест обязан быть воспроизводим в любой день).
        let dir = tmp.path().join("evidence");
        std::fs::create_dir_all(&dir).expect("mkdir");
        let marks_path = tmp.path().join(FP_REGISTER_REL);
        std::fs::write(
            &marks_path,
            "| Дата | Правило | Файл | Примечание |\n\
             |---|---|---|---|\n\
             | 2026-09-18 | no-pan | src/a.rs:1 | — |\n\
             | 2026-09-01 | old-rule | src/b.rs | вне окна |\n",
        )
        .expect("fp register");
        let report = build_at(tmp.path(), 7, fixed_now()).expect("дайджест 2");
        assert_eq!(report.fp_marks, 1);
        let share = report.fp_share_pct.expect("доля есть — fail-вызовы были");
        assert!((share - 100.0 / 3.0).abs() < 1e-9, "{share}");
        assert!(
            report.skipped >= 1,
            "запись вне окна посчитана: {}",
            report.skipped
        );
    }

    #[test]
    fn digest_collects_expiring_overrides_and_expiry() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path().join(".arch-handoff");
        std::fs::create_dir_all(&dir).expect("mkdir");
        std::fs::write(
            dir.join("CONSTRAINTS.yaml"),
            "rules:\n\
             - name: soon-expiry\n  type: file_exists\n  path: X\n  expiry: 2026-09-20\n  owner: Иванов\n\
             - name: far-expiry\n  type: file_exists\n  path: Y\n  expiry: 2027-01-01\n\
             - name: stale\n  type: file_exists\n  path: Z\n  expiry: 2026-09-01\n\
             overrides:\n\
             - rule: no-pan\n  adr: ADR-001\n  until: 2026-09-19\n\
             - rule: month-form\n  adr: ADR-002\n  until: 2026-09\n",
        )
        .expect("constraints");
        let report = build_at(tmp.path(), 7, fixed_now()).expect("дайджест");
        let kinds: Vec<(&str, &str)> = report
            .expiring
            .iter()
            .map(|e| (e.kind.as_str(), e.rule.as_str()))
            .collect();
        assert!(kinds.contains(&("override", "no-pan")), "{kinds:?}");
        assert!(kinds.contains(&("override", "month-form")), "{kinds:?}");
        assert!(kinds.contains(&("expiry", "soon-expiry")), "{kinds:?}");
        // Просроченное — тоже показываем (с отрицательным days_left).
        assert!(kinds.contains(&("expiry", "stale")), "{kinds:?}");
        // Далёкое — нет.
        assert!(!kinds.contains(&("expiry", "far-expiry")), "{kinds:?}");
        // Месячная форма — последний день месяца.
        let month = report
            .expiring
            .iter()
            .find(|e| e.rule == "month-form")
            .expect("month-form");
        assert_eq!(month.until, "2026-09-30");
        // Сортировка по срочности: просроченные первыми.
        assert_eq!(report.expiring[0].rule, "stale");
    }

    #[test]
    fn digest_empty_repo_is_honest_zeroes() {
        let tmp = tempfile::tempdir().expect("tmp");
        let report = build_at(tmp.path(), 7, fixed_now()).expect("дайджест");
        assert_eq!(report.calls_total, 0);
        assert_eq!(report.fp_marks, 0);
        assert_eq!(report.fp_share_pct, None);
        assert!(report.constraints_path.is_none());
        let md = render_markdown(&report);
        assert!(md.contains("Вызовов за окно нет"), "{md}");
        assert!(md.contains("файл ограничений не найден"), "{md}");
    }

    #[test]
    fn digest_render_contains_all_sections() {
        let tmp = tempfile::tempdir().expect("tmp");
        write_journal(
            tmp.path(),
            &[
                line(&ts(12, 9), "fitness_check", "fail", &["no-pan"]),
                line(&ts(12, 10), "fitness_check", "pass", &[]),
            ],
        );
        let report = build_at(tmp.path(), 7, fixed_now()).expect("дайджест");
        let md = render_markdown(&report);
        for section in [
            "## Вызовы MCP",
            "## Дефекты, не дошедшие до ревью",
            "## Итерации FAIL→PASS",
            "## Топ нарушаемых правил",
            "## Ложные срабатывания",
            "## Истекающие overrides и expiry",
        ] {
            assert!(md.contains(section), "нет секции {section}: {md}");
        }
        assert!(md.contains("fitness_check: 1"), "{md}");
        assert!(md.contains("no-pan: 1"), "{md}");
        // Управленческая квитанция: fail-вызовы по правилам + честная оговорка
        // пустого состояния.
        assert!(
            md.contains("Гейт остановил до ревью fail-вызовов: 1"),
            "{md}"
        );
        assert!(md.contains("Fail-вердикты по правилам: no-pan 1"), "{md}");
    }

    #[test]
    fn digest_render_defects_section_is_honest_when_no_fail_calls() {
        let tmp = tempfile::tempdir().expect("tmp");
        write_journal(
            tmp.path(),
            &[
                line(&ts(12, 9), "kb_search", "ok", &[]),
                line(&ts(12, 10), "fitness_check", "pass", &[]),
            ],
        );
        let report = build_at(tmp.path(), 7, fixed_now()).expect("дайджест");
        let md = render_markdown(&report);
        assert!(
            md.contains("Fail-вызовов за окно не было"),
            "пустое состояние — честная оговорка: {md}"
        );
        assert!(md.contains("не доказанный «ноль дефектов»"), "{md}");
    }
}
