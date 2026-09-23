//! Отчёты и read-only проверки контура: линтер ARCHITECTURE-SPINE.md
//! ([`lint_spine`], [`spine_ad_ids`]), сенсоры спецификаций
//! ([`sensors_check`]), отчёт по реестру правил ([`rules_report`]) и
//! корпоративный отчёт вверх ([`control_report`], `docs/corp-spine.md`).

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use regex::Regex;
use serde::{Deserialize, Serialize};

use super::exec::check;
use super::registry::load_constraints_resolved;
use super::rules::parse_constraints_file;
use super::types::{BEHAVIOUR_RULE_KINDS, FitnessRule, LintIssue};
use crate::error::{HarnessError, Result};

/// Обязательные заголовки спецификации (сенсор `required_sections`).
pub const REQUIRED_SECTIONS: [&str; 3] = ["## Проблема", "## Критерии приёмки", "## Риски"];

/// Номера инвариантов `AD-<n>`, определённых в файле spine.
///
/// Семантика определения — как у [`lint_spine`]: заголовок `### AD-<n> …`
/// либо строка `AD-<n>:` / `AD-<n>.` в начале строки. Используется
/// трассировкой (`crate::trace`) для сверки модели со spine.
///
/// # Errors
/// Файл не читается, некорректный идентификатор AD.
pub fn spine_ad_ids(path: &Path) -> Result<BTreeSet<u64>> {
    let content = std::fs::read_to_string(path).map_err(|e| HarnessError::io(path, e))?;
    let re_def_heading = spine_regex(r"^\s{0,3}#{1,6}\s+AD-(\d+)\b")?;
    let re_def_bare = spine_regex(r"^\s*AD-(\d+)\s*[:.]")?;
    let mut ids = BTreeSet::new();
    for (idx, line) in content.lines().enumerate() {
        if let Some(caps) = re_def_heading
            .captures(line)
            .or_else(|| re_def_bare.captures(line))
        {
            let id: u64 = caps[1].parse().map_err(|_| {
                HarnessError::Control(format!(
                    "{}:{}: некорректный идентификатор AD",
                    path.display(),
                    idx + 1
                ))
            })?;
            ids.insert(id);
        }
    }
    Ok(ids)
}

/// Линтер ARCHITECTURE-SPINE.md.
///
/// Определением AD-блока считается заголовок `### AD-<n> …` либо строка вида
/// `AD-<n>:` / `AD-<n>.` в начале строки; остальные вхождения `AD-<n>` —
/// ссылки. Блок простирается до следующего определения AD (или конца файла).
///
/// # Errors
/// Файл не читается.
pub fn lint_spine(path: &Path) -> Result<Vec<LintIssue>> {
    let content = std::fs::read_to_string(path).map_err(|e| HarnessError::io(path, e))?;
    let lines: Vec<&str> = content.lines().collect();

    let re_def_heading = spine_regex(r"^\s{0,3}#{1,6}\s+AD-(\d+)\b")?;
    let re_def_bare = spine_regex(r"^\s*AD-(\d+)\s*[:.]")?;
    let re_ad_ref = spine_regex(r"\bAD-(\d+)\b")?;
    let re_stub = spine_regex(r"\b(?:TODO|TBD|FIXME|XXX)\b|\?\?\?")?;
    let re_unpinned = spine_regex(r#"\blatest\b|[:=]\s*["']?\*["']?(?:\s|$)"#)?;
    let re_field = spine_regex(r"\b(Binds|Prevents|Rule)\*{0,2}\s*:\s*\*{0,2}\s*(.*)$")?;

    // Первый проход: определения AD-блоков.
    let mut def_lines: Vec<Option<u64>> = vec![None; lines.len()];
    let mut definitions: Vec<(u64, usize)> = Vec::new(); // (id, строка 1-based)
    for (idx, line) in lines.iter().enumerate() {
        let caps = re_def_heading
            .captures(line)
            .or_else(|| re_def_bare.captures(line));
        if let Some(caps) = caps {
            let id: u64 = caps[1].parse().map_err(|_| {
                HarnessError::Control(format!(
                    "{}:{}: некорректный идентификатор AD",
                    path.display(),
                    idx + 1
                ))
            })?;
            def_lines[idx] = Some(id);
            definitions.push((id, idx + 1));
        }
    }
    let defined: BTreeSet<u64> = definitions.iter().map(|(id, _)| *id).collect();

    let mut issues = Vec::new();
    let mut push = |line: usize, rule: &str, severity: &str, message: String| {
        issues.push(LintIssue {
            file: path.to_path_buf(),
            line,
            rule: rule.into(),
            message,
            severity: severity.into(),
            ..LintIssue::default()
        });
    };

    // dup_ad_id: повторное определение того же идентификатора.
    let mut first_seen: BTreeMap<u64, usize> = BTreeMap::new();
    for (id, line_no) in &definitions {
        if let Some(first) = first_seen.get(id) {
            push(
                *line_no,
                "dup_ad_id",
                "error",
                format!("повторное определение AD-{id} (первое — строка {first})"),
            );
        } else {
            first_seen.insert(*id, *line_no);
        }
    }

    // empty_field: у каждого AD-блока должны быть непустые Binds/Prevents/Rule.
    for (pos, (id, def_line)) in definitions.iter().enumerate() {
        let start = def_line - 1;
        let end = definitions
            .get(pos + 1)
            .map_or(lines.len(), |(_, next)| next - 1);
        let block = &lines[start..end];
        for field in ["Binds", "Prevents", "Rule"] {
            let mut found = false;
            for (off, line) in block.iter().enumerate() {
                let Some(caps) = re_field.captures(line) else {
                    continue;
                };
                if &caps[1] != field {
                    continue;
                }
                found = true;
                let value = caps[2].trim().trim_matches('*').trim();
                if value.is_empty() {
                    push(
                        start + off + 1,
                        "empty_field",
                        "error",
                        format!("AD-{id}: пустое обязательное поле '{field}'"),
                    );
                }
            }
            if !found {
                push(
                    *def_line,
                    "empty_field",
                    "error",
                    format!("AD-{id}: отсутствует обязательное поле '{field}'"),
                );
            }
        }
    }

    // Построчные правила: заглушки и непиннутые версии.
    for (idx, line) in lines.iter().enumerate() {
        if let Some(m) = re_stub.find(line) {
            push(
                idx + 1,
                "stub_marker",
                "warn",
                format!("заглушка '{}' — заполнить до гейта", m.as_str()),
            );
        }
        if re_unpinned.is_match(line) {
            let token = if line.contains("latest") {
                "'latest'"
            } else {
                "'*'"
            };
            push(
                idx + 1,
                "unpinned_version",
                "warn",
                format!("непиннутая версия зависимости: {token}"),
            );
        }
    }

    // broken_ad_ref: ссылки на несуществующие в файле AD.
    for (idx, line) in lines.iter().enumerate() {
        let mut reported: BTreeSet<u64> = BTreeSet::new();
        for caps in re_ad_ref.captures_iter(line) {
            let Ok(id) = caps[1].parse::<u64>() else {
                continue;
            };
            // Собственный идентификатор на строке определения — не ссылка.
            if def_lines[idx] == Some(id) || defined.contains(&id) || !reported.insert(id) {
                continue;
            }
            push(
                idx + 1,
                "broken_ad_ref",
                "warn",
                format!("ссылка на несуществующий AD-{id}"),
            );
        }
    }

    issues.sort_by(|a, b| (a.line, &a.rule).cmp(&(b.line, &b.rule)));
    Ok(issues)
}

/// Компилирует статический regex линтера (ошибка компиляции — внутренний дефект).
fn spine_regex(pattern: &str) -> Result<Regex> {
    Regex::new(pattern).map_err(|e| HarnessError::Control(format!("внутренний regex линтера: {e}")))
}

/// Результат сенсора.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SensorResult {
    /// Имя сенсора (`required_sections`, `upstream_coverage`).
    pub sensor: String,
    /// Проверенный файл.
    pub file: PathBuf,
    /// Прошёл ли.
    pub passed: bool,
    /// Детали.
    pub details: String,
}

/// Прогон сенсоров по каталогу спецификаций (нерекурсивно, только `*.md`).
///
/// Сенсоры:
/// - `required_sections` — наличие заголовков из [`REQUIRED_SECTIONS`];
/// - `upstream_coverage` — относительные ссылки `[..](path.md)` существуют
///   относительно каталога спецификаций.
///
/// # Errors
/// Каталог не читается.
pub fn sensors_check(spec_dir: &Path) -> Result<Vec<SensorResult>> {
    let mut files: Vec<PathBuf> = Vec::new();
    let rd = std::fs::read_dir(spec_dir).map_err(|e| HarnessError::io(spec_dir, e))?;
    for entry in rd {
        let entry = entry.map_err(|e| HarnessError::io(spec_dir, e))?;
        let p = entry.path();
        if p.is_file() && p.extension().is_some_and(|ext| ext == "md") {
            files.push(p);
        }
    }
    files.sort();

    let re_link = Regex::new(r"\[[^\]]*\]\(\s*([^)\s]+)[^)]*\)")
        .map_err(|e| HarnessError::Control(format!("внутренний regex сенсоров: {e}")))?;

    let mut results = Vec::new();
    for file in files {
        let content = std::fs::read_to_string(&file).map_err(|e| HarnessError::io(&file, e))?;

        let missing: Vec<&str> = REQUIRED_SECTIONS
            .iter()
            .copied()
            .filter(|sec| !content.lines().any(|l| l.trim_start().starts_with(sec)))
            .collect();
        results.push(SensorResult {
            sensor: "required_sections".into(),
            file: file.clone(),
            passed: missing.is_empty(),
            details: if missing.is_empty() {
                "все обязательные секции на месте".into()
            } else {
                format!("нет секций: {}", missing.join(", "))
            },
        });

        let mut total = 0usize;
        let mut broken: Vec<String> = Vec::new();
        for caps in re_link.captures_iter(&content) {
            let raw = &caps[1];
            if raw.starts_with("http://")
                || raw.starts_with("https://")
                || raw.starts_with("mailto:")
                || raw.starts_with('#')
            {
                continue;
            }
            let target = raw.split('#').next().unwrap_or(raw);
            if target.is_empty() {
                continue;
            }
            total += 1;
            let p = Path::new(target);
            let full = if p.is_absolute() {
                p.to_path_buf()
            } else {
                spec_dir.join(p)
            };
            if !full.exists() {
                broken.push(target.to_string());
            }
        }
        results.push(SensorResult {
            sensor: "upstream_coverage".into(),
            file,
            passed: broken.is_empty(),
            details: if broken.is_empty() {
                format!("все ссылки валидны ({total})")
            } else {
                format!("битые ссылки: {}", broken.join(", "))
            },
        });
    }
    Ok(results)
}

/// Число коммитов за последние 90 дней, трогавших файл ограничений —
/// git-прокси стоимости сопровождения реестра правил. `None` — не
/// git-репозиторий или git недоступен: отчёт показывает «недоступно»,
/// это не ошибка.
fn constraint_churn_90d(repo: &Path, constraints: &Path) -> Option<usize> {
    let abs = constraints.canonicalize().ok()?;
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["log", "--since=90 days ago", "--oneline", "--"])
        .arg(&abs)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter(|l| !l.trim().is_empty())
            .count(),
    )
}

/// Отчёт по реестру правил `CONSTRAINTS.yaml` (markdown в stdout).
///
/// Секции: сводка (всего / по типам / по severity), таблица карточек
/// (name, type, severity, owner, expiry, число `exclude_glob`, `effort_hours`),
/// находки (правила без owner/expiry; просроченные expiry — та же логика
/// даты, что в [`run_rule`]; правила с `exclude_glob` — прокси отступлений/FP),
/// git-прокси стоимости сопровождения (коммиты за 90 дней по файлу
/// ограничений; «недоступно» вне git-репозитория) и итоговая строка
/// «правил N, суммарный `effort_hours` X (покрыто Y правил)».
///
/// Правила с неизвестным `type` (словарь другой редакции, E8) не роняют
/// отчёт: перечисляются отдельным разделом «Пропущено (неизвестный тип)».
///
/// # Errors
/// Те же, что у [`check`]: репозиторий/файл недоступны, YAML невалиден,
/// правил нет.
pub fn rules_report(repo: &Path, constraints: &Path) -> Result<String> {
    if !repo.is_dir() {
        return Err(HarnessError::Control(format!(
            "репозиторий недоступен: {}",
            repo.display()
        )));
    }
    let yaml =
        std::fs::read_to_string(constraints).map_err(|e| HarnessError::io(constraints, e))?;
    let (parsed, skipped_unknown) = parse_constraints_file(&yaml, constraints)?;
    let rules: Vec<&FitnessRule> = parsed.all_rules().collect();
    if rules.is_empty() && skipped_unknown.is_empty() {
        return Err(HarnessError::Control(format!(
            "{}: файл не содержит правил — ожидается непустой корень `rules:` или `constraints:`",
            constraints.display()
        )));
    }

    let mut out = String::new();
    let _ = writeln!(out, "# Отчёт по правилам: {}", constraints.display());
    let _ = writeln!(out, "\nВсего правил: {}", rules.len());
    if !skipped_unknown.is_empty() {
        let types: BTreeSet<&str> = skipped_unknown
            .iter()
            .map(|s| s.rule_type.as_str())
            .collect();
        let _ = writeln!(
            out,
            "Пропущено правил: {} (неизвестные типы: {})",
            skipped_unknown.len(),
            types.into_iter().collect::<Vec<_>>().join(", ")
        );
    }

    let mut by_kind: BTreeMap<&str, usize> = BTreeMap::new();
    let mut by_severity: BTreeMap<&str, usize> = BTreeMap::new();
    for r in &rules {
        *by_kind.entry(r.kind.as_str()).or_default() += 1;
        *by_severity.entry(r.severity.as_str()).or_default() += 1;
    }
    let join_counts = |m: &BTreeMap<&str, usize>| {
        m.iter()
            .map(|(k, n)| format!("{k} {n}"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let _ = writeln!(out, "По типам: {}", join_counts(&by_kind));
    let _ = writeln!(out, "По severity: {}", join_counts(&by_severity));

    // Доля правил, проверяющих ПОВЕДЕНИЕ (Н10 волны C 0.3.4): правило на
    // упоминание — звено трассировки, а не проверка смысла. Метрика отвечает
    // на вопрос «сколько в реестре настоящих проверок», который иначе
    // приходится считать глазами по таблице типов.
    let behaviour = rules
        .iter()
        .filter(|r| BEHAVIOUR_RULE_KINDS.contains(&r.kind.as_str()))
        .count();
    let total = rules.len();
    let share = if total == 0 {
        0.0
    } else {
        behaviour as f64 / total as f64 * 100.0
    };
    let _ = writeln!(
        out,
        "Проверяют поведение: {behaviour} из {total} ({share:.0}%) — типы {};          остальные проверяют наличие текста (звено трассировки, а не смысл)",
        BEHAVIOUR_RULE_KINDS.join(", ")
    );

    let _ = writeln!(
        out,
        "\n| Правило | Тип | Severity | Owner | Expiry | exclude_glob | effort_hours |"
    );
    let _ = writeln!(out, "|---|---|---|---|---|---|---|");
    let dash = "—";
    for r in &rules {
        let _ = writeln!(
            out,
            "| {} | {} | {} | {} | {} | {} | {} |",
            r.name,
            r.kind.as_str(),
            r.severity,
            r.owner.as_deref().unwrap_or(dash),
            r.expiry.as_deref().unwrap_or(dash),
            r.exclude_glob.len(),
            r.effort_hours
                .map_or_else(|| dash.to_string(), |h| h.to_string()),
        );
    }

    let _ = writeln!(out, "\n## Находки\n");

    let _ = writeln!(out, "### Правила без owner/expiry");
    let mut any = false;
    for r in &rules {
        let missing: Vec<&str> = [
            (r.owner.is_none()).then_some("owner"),
            (r.expiry.is_none()).then_some("expiry"),
        ]
        .into_iter()
        .flatten()
        .collect();
        if !missing.is_empty() {
            let _ = writeln!(out, "- {} (нет {})", r.name, missing.join(", "));
            any = true;
        }
    }
    if !any {
        let _ = writeln!(out, "- нет");
    }

    let _ = writeln!(out, "\n### Просроченные правила (expiry в прошлом)");
    let today = chrono::Local::now().date_naive();
    let mut any = false;
    for r in &rules {
        let Some(expiry) = &r.expiry else {
            continue;
        };
        // Невалидная дата — не ошибка: поле метаданное (как в run_rule).
        if let Ok(date) = chrono::NaiveDate::parse_from_str(expiry.trim(), "%Y-%m-%d") {
            if date < today {
                let owner = r
                    .owner
                    .as_deref()
                    .map(|o| format!(" (владелец: {o})"))
                    .unwrap_or_default();
                let _ = writeln!(out, "- {} — expiry {expiry}{owner}", r.name);
                any = true;
            }
        }
    }
    if !any {
        let _ = writeln!(out, "- нет");
    }

    let _ = writeln!(
        out,
        "\n### Правила с exclude_glob (прокси отступлений/ложных срабатываний)"
    );
    let mut any = false;
    for r in &rules {
        if !r.exclude_glob.is_empty() {
            let _ = writeln!(out, "- {}: {}", r.name, r.exclude_glob.join(", "));
            any = true;
        }
    }
    if !any {
        let _ = writeln!(out, "- нет");
    }

    let _ = writeln!(out, "\n### Стоимость сопровождения (git-прокси)");
    match constraint_churn_90d(repo, constraints) {
        Some(n) => {
            let _ = writeln!(
                out,
                "Коммитов за последние 90 дней, трогавших файл ограничений: {n}"
            );
        }
        None => {
            let _ = writeln!(
                out,
                "Коммитов за последние 90 дней, трогавших файл ограничений: недоступно \
                 (не git-репозиторий или git не найден)"
            );
        }
    }

    // Зубы применённых шаблонов (П2): находка `executable_rule_toothless` —
    // «правило есть» ≠ «правило проверяет». В вердикт гейта находка не входит
    // (это не нарушение архитектуры, а качество реестра), но архитектор обязан
    // видеть её там, где смотрит на реестр. Отчёт остаётся отчётом: раздел не
    // меняет ни вердикт, ни код возврата.
    if let Some(section) = teeth_section(repo, &crate::rule_templates::Runner::detect(None)) {
        let _ = write!(out, "{section}");
    }

    let covered = rules.iter().filter(|r| r.effort_hours.is_some()).count();
    // Sum для f64 на пустом множестве даёт -0.0 (особенность std) — +0.0
    // нормализует к «0» в выводе.
    let total: f64 = rules.iter().filter_map(|r| r.effort_hours).sum::<f64>() + 0.0;
    let _ = writeln!(
        out,
        "\nправил {}, суммарный effort_hours {total} (покрыто {covered} правил)",
        rules.len()
    );

    if !skipped_unknown.is_empty() {
        let _ = writeln!(
            out,
            "\n## Пропущено (неизвестный тип — словарь другой редакции?)"
        );
        for s in &skipped_unknown {
            let _ = writeln!(out, "- `{}` — неизвестный тип `{}`", s.name, s.rule_type);
        }
    }
    Ok(out)
}

/// Раздел отчёта о зубах применённых шаблонов исполняемых правил (П2).
///
/// `None` — в кейсе нет `.arch-handoff/rule-templates.lock`: шаблоны не
/// применялись, и отчёт не прибавляет ни строки (кейс, никогда не видевший
/// `rules template apply`, не должен платить за чужую механику). Кейс с локом
/// получает раздел; сорвавшаяся проверка (нет прогонщика, нечитаемый lock)
/// даёт одну строку с причиной, а не падение отчёта — в отличие от
/// `rules template verify`, где это провал по существу.
///
/// Проверяется python-половина: java требует Maven или JUnit-консоли и минуты
/// компиляции, а отчёт обязан оставаться дешёвым. Непроверенное при этом
/// видно числом пропусков, а не молчанием.
fn teeth_section(case: &Path, runner: &crate::rule_templates::Runner) -> Option<String> {
    let lock = case.join(crate::rule_templates::LOCK_REL);
    if !lock.is_file() {
        return None;
    }
    let applied = applied_templates_count(&lock);
    let report = match crate::rule_templates::verify_dir(
        case,
        runner,
        crate::rule_templates::Lang::Python,
    ) {
        Ok(report) => report,
        Err(e) => {
            return Some(format!(
                "\n## Зубы применённых шаблонов (П2)\nПроверка не выполнена: {e}\n"
            ));
        }
    };
    let with_teeth = report.checks.iter().filter(|c| c.ok).count();
    let mut out = String::from("\n## Зубы применённых шаблонов (П2)\n");
    let _ = writeln!(
        out,
        "Применённых шаблонов: {applied}; с зубами: {with_teeth}, беззубых: {} \
         (`executable_rule_toothless`), проверок пропущено: {}",
        report.findings.len(),
        report.skipped.len()
    );
    for finding in &report.findings {
        let _ = writeln!(out, "- {finding}");
    }
    Some(out)
}

/// Число записей `templates:` в lock-файле применённых шаблонов (0, если файл
/// не читается или не парсится — причину назовёт сама проверка зубов).
fn applied_templates_count(lock: &Path) -> usize {
    let Ok(text) = std::fs::read_to_string(lock) else {
        return 0;
    };
    let Ok(value) = serde_yaml_ng::from_str::<serde_yaml_ng::Value>(&text) else {
        return 0;
    };
    value
        .get("templates")
        .and_then(serde_yaml_ng::Value::as_sequence)
        .map_or(0, Vec::len)
}

/// Просрочено ли правило (expiry в прошлом; невалидная дата — не просрочка,
/// поле метаданное). Общая логика даты с `run_rule`/`rules_report`.
fn rule_expiry_in_past(expiry: &str) -> bool {
    chrono::NaiveDate::parse_from_str(expiry.trim(), "%Y-%m-%d")
        .is_ok_and(|date| date < chrono::Local::now().date_naive())
}

/// Запись overrides в отчёте `control report` (SDK-контракт, аддитивные поля).
#[derive(Debug, Clone, Serialize)]
pub struct OverrideReportEntry {
    /// Правило.
    pub rule: String,
    /// ADR.
    pub adr: String,
    /// Срок.
    pub until: String,
    /// active|expired|invalid.
    pub status: String,
    /// Пояснение.
    pub note: String,
}

/// Отчёт вверх по корпоративному контуру (`arch-be control report`,
/// `docs/corp-spine.md`): агрегат по текущему репо — покрытие корп-правил,
/// overrides со статусами, просроченные правила.
#[derive(Debug, Clone, Serialize)]
pub struct ControlReport {
    /// Репозиторий.
    pub repo: PathBuf,
    /// Файл ограничений.
    pub constraints: PathBuf,
    /// Уровень отчёта: `corp` (только унаследованные правила) | `all`.
    pub level: String,
    /// Правил в скоупе уровня.
    pub rules_total: usize,
    /// Собственных правил в скоупе.
    pub own: usize,
    /// Унаследованные правила по источникам (`<ref>@<version>` → число).
    pub inherited: BTreeMap<String, usize>,
    /// Правил без находок.
    pub pass: usize,
    /// Правил с error-находками.
    pub fail: usize,
    /// Правил только с warn-находками.
    pub warn: usize,
    /// Итог гейта (`control check`).
    pub passed: bool,
    /// Overrides со статусами.
    pub overrides: Vec<OverrideReportEntry>,
    /// Просроченные правила (expiry в прошлом).
    pub expired_rules: Vec<String>,
    /// Правила ручного контроля (`unverifiable: true` — не исполняются,
    /// осознанный долг с owner).
    pub unverifiable_rules: Vec<String>,
    /// Расхождения пинов версий `extends` (текст находок).
    pub version_mismatches: Vec<String>,
}

/// Отчёт `control report --level corp|all` по текущему репо: прогоняет
/// [`check`] и группирует находки по правилам в скоупе уровня (`corp` —
/// только унаследованные через `extends` правила, `all` — все).
///
/// # Errors
/// Неизвестный уровень; те же, что у [`check`].
pub fn control_report(repo: &Path, constraints: &Path, level: &str) -> Result<ControlReport> {
    if level != "corp" && level != "all" {
        return Err(HarnessError::Control(format!(
            "неизвестный уровень отчёта '{level}' (допустимы: corp, all)"
        )));
    }
    let resolved = load_constraints_resolved(constraints)?;
    let fitness = check(repo, constraints)?;

    let in_scope = |rule: &&FitnessRule| level == "all" || rule.source.is_some();
    let scoped: Vec<&FitnessRule> = resolved.rules.iter().filter(in_scope).collect();

    // Худший исход правила по его находкам (error > warn > pass); находки
    // механики (extends/override) правилам не принадлежат и не группируются.
    // Правила ручного контроля (unverifiable) не исполняются — в исходы не
    // входят, перечисляются отдельно.
    let mut pass = 0usize;
    let mut fail = 0usize;
    let mut warn = 0usize;
    let mut inherited: BTreeMap<String, usize> = BTreeMap::new();
    let mut own = 0usize;
    let mut unverifiable_rules = Vec::new();
    for rule in &scoped {
        match &rule.source {
            Some(source) => *inherited.entry(source.clone()).or_default() += 1,
            None => own += 1,
        }
        if rule.unverifiable {
            unverifiable_rules.push(rule.name.clone());
            continue;
        }
        let has_error = fitness
            .issues
            .iter()
            .any(|i| i.rule == rule.name && i.severity == "error");
        let has_warn = fitness
            .issues
            .iter()
            .any(|i| i.rule == rule.name && i.severity == "warn");
        if has_error {
            fail += 1;
        } else if has_warn {
            warn += 1;
        } else {
            pass += 1;
        }
    }

    let expired_rules: Vec<String> = scoped
        .iter()
        .filter(|r| r.expiry.as_deref().is_some_and(rule_expiry_in_past))
        .map(|r| r.name.clone())
        .collect();
    let version_mismatches: Vec<String> = fitness
        .issues
        .iter()
        .filter(|i| i.rule == "extends")
        .map(|i| i.message.clone())
        .collect();
    let overrides: Vec<OverrideReportEntry> = fitness
        .overrides
        .iter()
        .map(|o| OverrideReportEntry {
            rule: o.rule.clone(),
            adr: o.adr.clone(),
            until: o.until.clone(),
            status: o.status.clone(),
            note: o.note.clone(),
        })
        .collect();

    Ok(ControlReport {
        repo: repo.to_path_buf(),
        constraints: constraints.to_path_buf(),
        level: level.to_string(),
        rules_total: scoped.len(),
        own,
        inherited,
        pass,
        fail,
        warn,
        passed: fitness.passed,
        overrides,
        expired_rules,
        unverifiable_rules,
        version_mismatches,
    })
}

/// Рендерит [`ControlReport`] в markdown (человекочитаемый режим
/// `arch-be control report`).
#[must_use]
pub fn render_control_report(report: &ControlReport) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "# Отчёт контроля (уровень {}): {}",
        report.level,
        report.repo.display()
    );
    let _ = writeln!(out, "Файл ограничений: {}", report.constraints.display());
    let _ = writeln!(
        out,
        "\nПравил в скоупе: {} (собственных: {}, унаследованных: {})",
        report.rules_total,
        report.own,
        report.inherited.values().sum::<usize>()
    );
    for (source, count) in &report.inherited {
        let _ = writeln!(out, "- {source}: {count} правил");
    }
    let _ = writeln!(
        out,
        "\nИсходы правил: pass {}, fail {}, warn {}",
        report.pass, report.fail, report.warn
    );
    let _ = writeln!(
        out,
        "Итог гейта: {}",
        if report.passed { "PASS" } else { "FAIL" }
    );

    let _ = writeln!(out, "\n## Overrides");
    if report.overrides.is_empty() {
        let _ = writeln!(out, "- нет");
    }
    for o in &report.overrides {
        let _ = writeln!(
            out,
            "- {} (adr {}, until {}): {} — {}",
            o.rule, o.adr, o.until, o.status, o.note
        );
    }

    let _ = writeln!(out, "\n## Просроченные правила (expiry)");
    if report.expired_rules.is_empty() {
        let _ = writeln!(out, "- нет");
    }
    for name in &report.expired_rules {
        let _ = writeln!(out, "- {name}");
    }

    let _ = writeln!(out, "\n## Ручной контроль (unverifiable)");
    if report.unverifiable_rules.is_empty() {
        let _ = writeln!(out, "- нет");
    }
    for name in &report.unverifiable_rules {
        let _ = writeln!(out, "- {name}");
    }

    let _ = writeln!(out, "\n## Расхождения версий extends");
    if report.version_mismatches.is_empty() {
        let _ = writeln!(out, "- нет");
    }
    for m in &report.version_mismatches {
        let _ = writeln!(out, "- {m}");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::check;

    /// Пишет файл в каталог и возвращает его путь.
    fn write_file(dir: &Path, name: &str, content: &str) -> PathBuf {
        let p = dir.join(name);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&p, content).unwrap();
        p
    }
    /// Корп-родитель для фикстур наследования (docs/corp-spine.md).
    const CORP_YAML: &str = "version: \"2026.3\"\n\
        rules:\n\
        \x20 - id: C-CORP-001\n\
        \x20   name: no_hold_crates\n\
        \x20   type: deny_dependency\n\
        \x20   deny: [left-pad, openssl-sys]\n\
        \x20   reference: \"Техкомитет 2026-03, протокол №12\"\n\
        \x20   severity: block\n\
        \x20 - id: C-CORP-002\n\
        \x20   name: corp_readme_present\n\
        \x20   type: file_exists\n\
        \x20   path: \"README.md\"\n\
        \x20   severity: warn\n";
    /// Продуктовый файл с override-фикстурой (тело overrides — параметр).
    fn override_fixture(overrides_yaml: &str) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        write_file(dir.path(), "corp/CONSTRAINTS.corp.yaml", CORP_YAML);
        write_file(dir.path(), "product/README.md", "# product\n");
        let product = write_file(
            dir.path(),
            "product/CONSTRAINTS.yaml",
            &format!(
                "extends: [\"../corp/CONSTRAINTS.corp.yaml@2026.3\"]\n\
                 overrides:\n{overrides_yaml}\n\
                 rules:\n\
                 \x20 - name: own_rule\n\
                 \x20   type: file_exists\n\
                 \x20   path: \"README.md\"\n"
            ),
        );
        (dir, product)
    }
    #[test]
    fn spine_detects_all_rule_kinds() {
        let dir = tempfile::tempdir().unwrap();
        let spine = write_file(
            dir.path(),
            "ARCHITECTURE-SPINE.md",
            "# ARCHITECTURE-SPINE\n\
             \n\
             ### AD-1. Единый брокер сообщений\n\
             - Binds: интеграционный контур\n\
             - Prevents: point-to-point связность\n\
             - Rule: все события — через брокер\n\
             \n\
             ### AD-2. Хранилище\n\
             - Binds:\n\
             - Rule: см. AD-99\n\
             \n\
             ### AD-1. Повторное определение\n\
             - Binds: x\n\
             - Prevents: y\n\
             - Rule: z\n\
             \n\
             ### AD-3. Версии зависимостей\n\
             - Binds: стек\n\
             - Prevents: дрейф версий\n\
             - Rule: все зависимости пинуются\n\
             - kafka-client = \"latest\"  # TODO запиновать\n",
        );
        let issues = lint_spine(&spine).unwrap();
        let count = |rule: &str| issues.iter().filter(|i| i.rule == rule).count();
        assert_eq!(
            count("dup_ad_id"),
            1,
            "должен найти повтор AD-1: {issues:?}"
        );
        assert_eq!(
            count("empty_field"),
            2,
            "AD-2: пустой Binds + нет Prevents: {issues:?}"
        );
        assert_eq!(count("stub_marker"), 1, "TODO: {issues:?}");
        assert_eq!(count("unpinned_version"), 1, "latest: {issues:?}");
        assert_eq!(count("broken_ad_ref"), 1, "AD-99 не определён: {issues:?}");
        assert!(
            issues
                .iter()
                .all(|i| i.severity == "error" || i.severity == "warn")
        );
        assert!(
            issues
                .iter()
                .any(|i| i.rule == "broken_ad_ref" && i.message.contains("AD-99"))
        );
        assert!(issues.iter().all(|i| i.line > 0));
    }

    #[test]
    fn spine_clean_file_has_no_issues() {
        let dir = tempfile::tempdir().unwrap();
        let spine = write_file(
            dir.path(),
            "SPINE.md",
            "### AD-1. Брокер\n\
             - Binds: контур\n\
             - Prevents: хаос\n\
             - Rule: только через брокер\n\
             \n\
             ### AD-2. Хранилище\n\
             - Binds: данные\n\
             - Prevents: дубли\n\
             - Rule: одно хранилище, см. AD-1\n",
        );
        let issues = lint_spine(&spine).unwrap();
        assert!(issues.is_empty(), "чистый spine: {issues:?}");
    }

    #[test]
    fn sensors_pass_and_fail_per_file() {
        let dir = tempfile::tempdir().unwrap();
        write_file(
            dir.path(),
            "good.md",
            "# Спека\n\
             \n\
             ## Проблема\n\
             текст\n\
             \n\
             ## Критерии приёмки\n\
             текст\n\
             \n\
             ## Риски\n\
             текст\n\
             \n\
             Связано: [вторая спека](bad.md), [сайт](https://example.com), [якорь](#риски)\n",
        );
        write_file(
            dir.path(),
            "bad.md",
            "# Плохая спека\n\
             \n\
             ## Проблема\n\
             только проблема, ссылка [битая](missing.md)\n",
        );
        write_file(
            dir.path(),
            "ignored.txt",
            "## Проблема\nне md — игнорируем\n",
        );

        let results = sensors_check(dir.path()).unwrap();
        assert_eq!(results.len(), 4, "2 md-файла × 2 сенсора: {results:?}");
        let find = |file: &str, sensor: &str| {
            results
                .iter()
                .find(|r| r.file.ends_with(file) && r.sensor == sensor)
                .unwrap()
        };
        assert!(find("good.md", "required_sections").passed);
        assert!(find("good.md", "upstream_coverage").passed);
        let bad_sections = find("bad.md", "required_sections");
        assert!(!bad_sections.passed);
        assert!(bad_sections.details.contains("## Риски"));
        assert!(bad_sections.details.contains("## Критерии приёмки"));
        let bad_links = find("bad.md", "upstream_coverage");
        assert!(!bad_links.passed);
        assert!(bad_links.details.contains("missing.md"));
    }

    #[test]
    fn spine_ad_ids_heading_and_bare_forms() {
        // Заголовки `## AD-<n>:` и «голые» строки `AD-<n>.` — определения;
        // вхождения AD-<n> в тексте — ссылки, не определения (ADR-006).
        let dir = tempfile::tempdir().unwrap();
        let spine = dir.path().join("ARCHITECTURE-SPINE.md");
        std::fs::write(
            &spine,
            "# Spine\n\n## AD-1: Первый\n\nСсылаемся на AD-2 в тексте.\n\nAD-2. Второй (bare-форма)\n\n- **Rule**: AD-1 применяется.\n",
        )
        .unwrap();
        let ids = spine_ad_ids(&spine).unwrap();
        assert_eq!(ids, BTreeSet::from([1, 2]), "только определения");
    }

    // --- M-1b/C-3: rules_report ------------------------------------------------

    #[test]
    fn rules_report_sections_and_effort_hours() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let constraints = write_file(
            dir.path(),
            "CONSTRAINTS.yaml",
            "rules:\n\
             \x20 - name: no-pan\n\
             \x20   type: must_not_contain\n\
             \x20   glob: 'src/**/*.py'\n\
             \x20   pattern: '\\b\\d{16}\\b'\n\
             \x20   owner: Иванов\n\
             \x20   expiry: '2999-01-01'\n\
             \x20   effort_hours: 4.5\n\
             \x20 - name: old-rule\n\
             \x20   type: file_exists\n\
             \x20   path: README.md\n\
             \x20   owner: Петров\n\
             \x20   expiry: '2020-01-01'\n\
             \x20   effort_hours: 2\n\
             \x20 - name: contracts-except-legacy\n\
             \x20   type: dir_must_have_file\n\
             \x20   glob: 'services/*'\n\
             \x20   path: openapi.yaml\n\
             \x20   exclude_glob: ['services/a', 'services/b']\n\
             \x20   severity: warn\n\
             \x20 - name: bare-rule\n\
             \x20   type: command_succeeds\n\
             \x20   command: 'true'\n",
        );
        let report = rules_report(&repo, &constraints).unwrap();
        // Сводка.
        assert!(report.contains("Всего правил: 4"), "{report}");
        assert!(report.contains("must_not_contain 1"), "{report}");
        assert!(report.contains("По severity: error 3, warn 1"), "{report}");
        // Таблица карточек.
        assert!(
            report
                .contains("| no-pan | must_not_contain | error | Иванов | 2999-01-01 | 0 | 4.5 |"),
            "{report}"
        );
        assert!(
            report.contains(
                "| contracts-except-legacy | dir_must_have_file | warn | — | — | 2 | — |"
            ),
            "{report}"
        );
        // Находки: без owner/expiry (нет обоих полей у двух правил).
        let no_meta = report.split("### Правила без owner/expiry").nth(1).unwrap();
        let no_meta = no_meta.split("###").next().unwrap();
        assert!(
            no_meta.contains("contracts-except-legacy (нет owner, expiry)"),
            "{no_meta}"
        );
        assert!(
            no_meta.contains("bare-rule (нет owner, expiry)"),
            "{no_meta}"
        );
        assert!(
            !no_meta.contains("no-pan"),
            "owner+expiry заданы: {no_meta}"
        );
        // Просроченные — та же логика даты, что в run_rule.
        let expired = report.split("### Просроченные правила").nth(1).unwrap();
        let expired = expired.split("###").next().unwrap();
        assert!(
            expired.contains("old-rule — expiry 2020-01-01 (владелец: Петров)"),
            "{expired}"
        );
        assert!(
            !expired.contains("no-pan"),
            "дата в будущем — не просрочено: {expired}"
        );
        // exclude_glob — прокси отступлений.
        assert!(
            report.contains("- contracts-except-legacy: services/a, services/b"),
            "{report}"
        );
        // Без git-репозитория — ветка «недоступно», не ошибка.
        assert!(report.contains("недоступно"), "{report}");
        // Итоговая строка: effort_hours парсится и суммируется.
        assert!(
            report.contains("правил 4, суммарный effort_hours 6.5 (покрыто 2 правил)"),
            "{report}"
        );
    }

    // --- E7: зубы применённых шаблонов в отчёте реестра (П2) --------------------

    /// Прогонщик с непустым полем `python` (команды в локе тривиальные —
    /// `true`/`false`, — им прогонщик и не нужен: `verify_dir` смотрит
    /// требование по тексту команды, A2). Прогон детерминирован и дешёв.
    fn fake_runner() -> crate::rule_templates::Runner {
        crate::rule_templates::Runner {
            python: Some(PathBuf::from("python3")),
            ..crate::rule_templates::Runner::unavailable()
        }
    }

    /// Раскладывает применённый шаблон библиотеки в кейс и возвращает его
    /// запись для лока: файлы с хэшами (`apply` пишет те же) и команду. Команду
    /// тест подменяет нарочно — проверяется раздел отчёта, а не шаблон.
    fn applied_template_entry(case: &Path, id: &str, command: &str) -> String {
        let t = crate::rule_templates::template(id)
            .unwrap()
            .expect("шаблон есть в этой сборке");
        let dir = format!("{}/{id}", crate::rule_templates::TARGET_REL);
        let mut files = Vec::new();
        for f in t.files_for(crate::rule_templates::Lang::Python) {
            let content = t.file(&f.from).expect("файл шаблона есть в сборке");
            let rel = format!("{dir}/{}", f.to);
            write_file(case, &rel, content);
            files.push((rel, crate::hash::sha256_hex(content.as_bytes())));
        }
        let mut out = format!(
            "  - id: {id}\n    version: {}\n    ad: AD-1\n    lang: python\n    \
             dir: {dir}\n    command: '{command}'\n    files:\n",
            t.manifest.version
        );
        for (path, sha) in files {
            let _ = write!(out, "      - path: {path}\n        sha256: {sha}\n");
        }
        out
    }

    /// Без `.arch-handoff/rule-templates.lock` раздел о зубах не появляется:
    /// отчёт кейса, не применявшего шаблоны, не обрастает чужой механикой.
    #[test]
    fn rules_report_without_lock_has_no_teeth_section() {
        let dir = tempfile::tempdir().unwrap();
        let case = dir.path().join("case");
        std::fs::create_dir_all(&case).unwrap();
        let constraints = write_file(
            dir.path(),
            "CONSTRAINTS.yaml",
            "rules:\n  - name: r\n    type: file_exists\n    path: README.md\n",
        );
        let report = rules_report(&case, &constraints).unwrap();
        assert!(!report.contains("Зубы применённых шаблонов"), "{report}");
        assert!(!report.contains("executable_rule_toothless"), "{report}");
        // Раздел сторожит именно лок-файл, а не «нет шаблонов в кейсе».
        assert!(teeth_section(&case, &fake_runner()).is_none());
    }

    /// Лок есть: раздел показывает число применённых шаблонов, зубы и дословные
    /// находки `executable_rule_toothless`; тело отчёта при этом не меняется —
    /// отчёт остаётся отчётом.
    #[test]
    fn rules_report_prints_teeth_section_from_lock() {
        let dir = tempfile::tempdir().unwrap();
        let case = dir.path().join("case");
        std::fs::create_dir_all(&case).unwrap();
        // Две применённые позиции: команда `true` упасть не может — правило
        // беззубое; команда `false` пройти не может — зубы на месте.
        let toothless = applied_template_entry(&case, "idempotency-key", "true");
        let toothy = applied_template_entry(&case, "no-pii-in-logs", "false");
        write_file(
            &case,
            crate::rule_templates::LOCK_REL,
            &format!("# применённые шаблоны\ntemplates:\n{toothless}{toothy}"),
        );
        // Реестр кейса: имена правил как в шаблонах и те же команды, иначе
        // `verify_dir` сочтёт применение адаптированным и пропустит проверку.
        let constraints = write_file(
            &case,
            ".arch-handoff/CONSTRAINTS.yaml",
            "rules:\n  - name: idempotency_key_enforced\n    type: command_succeeds\n    \
             command: 'true'\n  - name: no_pii_in_logs\n    type: command_succeeds\n    \
             command: 'false'\n",
        );
        let report = rules_report(&case, &constraints).unwrap();
        let (before, teeth) = report
            .split_once("## Зубы применённых шаблонов (П2)")
            .unwrap_or_else(|| panic!("раздел о зубах отсутствует: {report}"));
        assert!(
            teeth.contains(
                "Применённых шаблонов: 2; с зубами: 1, беззубых: 1 \
                 (`executable_rule_toothless`), проверок пропущено: 0"
            ),
            "{teeth}"
        );
        assert!(
            teeth.contains(
                "- idempotency-key [python]: тест остался зелёным на нарушающей реализации — \
                 правило беззубое (`executable_rule_toothless`)"
            ),
            "{teeth}"
        );
        assert!(
            !teeth.contains("no-pii-in-logs ["),
            "зубастая позиция находкой не помечается: {teeth}"
        );
        // Тело отчёта до раздела не изменилось: сводка реестра на месте.
        assert!(before.contains("Всего правил: 2"), "{before}");
    }

    #[test]
    fn control_report_json_shape_corp_level() {
        let (dir, product) =
            override_fixture("  - rule: C-CORP-001\n    adr: ADR-041\n    until: \"2999-01\"\n");
        let product_dir = dir.path().join("product");
        let report = control_report(&product_dir, &product, "corp").unwrap();
        assert_eq!(report.rules_total, 2, "только унаследованные");
        assert_eq!(report.own, 0);
        assert_eq!(report.inherited.get("CONSTRAINTS.corp@2026.3"), Some(&2));
        // C-CORP-001 отключён override'ом (находок нет → pass), C-CORP-002 —
        // README на месте → pass.
        assert_eq!(report.pass, 2);
        assert_eq!(report.fail, 0);
        assert_eq!(report.overrides.len(), 1);
        assert_eq!(report.overrides[0].status, "active");
        assert!(report.expired_rules.is_empty());
        assert!(report.version_mismatches.is_empty());
        assert!(report.passed);
        // JSON-контракт: ключевые поля сериализуются.
        let json = serde_json::to_value(&report).unwrap();
        for key in [
            "rules_total",
            "inherited",
            "pass",
            "fail",
            "warn",
            "overrides",
            "expired_rules",
        ] {
            assert!(json.get(key).is_some(), "нет поля {key}");
        }

        let all = control_report(&product_dir, &product, "all").unwrap();
        assert_eq!(all.rules_total, 3);
        assert_eq!(all.own, 1);
    }

    #[test]
    fn control_report_rejects_unknown_level() {
        let dir = tempfile::tempdir().unwrap();
        let constraints = write_file(dir.path(), "CONSTRAINTS.yaml", CORP_YAML);
        let err = control_report(dir.path(), &constraints, "domain").unwrap_err();
        assert!(err.to_string().contains("corp"), "{err}");
    }

    #[test]
    fn unverifiable_rule_parses_without_type_and_is_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let constraints = write_file(
            dir.path(),
            "CONSTRAINTS.yaml",
            "rules:\n\
             \x20 - id: C-CORP-900\n\
             \x20   name: manual_observability\n\
             \x20   unverifiable: true\n\
             \x20   owner: \"@corp-arch\"\n\
             \x20 - name: own_rule\n\
             \x20   type: file_exists\n\
             \x20   path: \"README.md\"\n",
        );
        // Правило без type парсится, не исполняется; гейт — по own_rule (fail:
        // README нет), unverifiable не даёт находок.
        let report = check(dir.path(), &constraints).unwrap();
        assert!(!report.passed);
        assert_eq!(report.durations.len(), 1, "исполнено только own_rule");
        assert!(
            !report
                .issues
                .iter()
                .any(|i| i.rule == "manual_observability")
        );

        let corp = control_report(dir.path(), &constraints, "all").unwrap();
        assert_eq!(corp.unverifiable_rules, vec!["manual_observability"]);
        assert_eq!(corp.fail, 1, "только own_rule в исходах");
        assert_eq!(corp.pass, 0);
    }
    /// Реестр «другой редакции»: два неизвестных типа + одно валидное правило.
    const ALIEN_CONSTRAINTS: &str = "rules:\n\
         \x20 - name: readme\n\
         \x20   type: file_exists\n\
         \x20   path: README.md\n\
         \x20 - name: skill-gate\n\
         \x20   type: skill_contract\n\
         \x20   glob: 'skills/**'\n\
         \x20 - name: addr-trace\n\
         \x20   type: address_trace\n";

    #[test]
    fn rules_report_lists_unknown_types_section() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        let c = write_file(repo, "CONSTRAINTS.yaml", ALIEN_CONSTRAINTS);
        let md = rules_report(repo, &c).expect("отчёт");
        assert!(md.contains("Всего правил: 1"), "{md}");
        assert!(
            md.contains("Пропущено правил: 2 (неизвестные типы: address_trace, skill_contract)"),
            "{md}"
        );
        assert!(md.contains("## Пропущено (неизвестный тип"), "{md}");
        assert!(
            md.contains("`skill-gate` — неизвестный тип `skill_contract`"),
            "{md}"
        );
        // Валидное правило — в таблице.
        assert!(md.contains("| readme | file_exists |"), "{md}");
    }
}
