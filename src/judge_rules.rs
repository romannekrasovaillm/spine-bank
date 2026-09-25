//! E7.3: повторяющиеся находки судьи — кандидаты в детерминированные правила.
//!
//! Смысловая рубрика тратит вызов модели на каждый прогон, но её находки
//! повторяются: один и тот же класс дефекта судья называет из отчёта в отчёт
//! (живой архив харнесса: цитата `self._done[idempotency_key] = receipt`
//! встречается в семи прогонах `code_invariant_conformance`). Повтор — сигнал,
//! что класс закрывается механикой: детерминированное правило дешевле,
//! воспроизводимо и не зависит от модели.
//!
//! Модуль читает **историю отчётов рубрик** (архив `paths.reports_dir`,
//! по умолчанию `~/.arch-harness/reports`) и предлагает кандидатов:
//!
//! - **обвинение** (балл ≤ [`ACCUSATION_MAX_SCORE`] с цитатой) в
//!   `min_runs` прогонах, причём хотя бы на двух разных целях, — кандидат
//!   с готовым YAML `must_not_contain`: цитируемая строка становится
//!   запрещённым паттерном;
//! - **метка механики** (`evidence_not_found`, `unstable`, …) на одном
//!   критерии в `min_runs` прогонах — честный advisory без YAML: механика
//!   систематически не подтверждает суждение, но какой именно детектор нужен,
//!   из истории не следует.
//!
//! Источники истории: JSON-близнец отчёта ([`HistoryRecord`], схема
//! [`HISTORY_SCHEMA`], пишется `rubric run` рядом с markdown) и сам markdown
//! (отчёты, снятые до появления близнеца). JSON приоритетнее: в нём есть цель
//! и метки как данные, а не как текст таблицы.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::rules_suggest::{Candidate, SuggestReport};

/// Схема JSON-близнеца отчёта рубрики в архиве истории.
pub const HISTORY_SCHEMA: &str = "judge-history/1";

/// Сколько прогонов делают находку повторяющейся (порог по умолчанию).
pub const MIN_RUNS: usize = 3;

/// Балл, при котором судья утверждает нарушение (шкала 1..=5): ≤ 2 — обвинение,
/// ≥ 4 — «чисто». Между ними — сомнение, детерминировать нечего.
pub const ACCUSATION_MAX_SCORE: u8 = 2;

/// Потолок файлов истории за один проход (защита от гигантского архива).
const MAX_HISTORY_FILES: usize = 2000;

/// Один прогон смысловой рубрики в истории: что судили, чем и с каким исходом.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryRecord {
    /// Схема записи ([`HISTORY_SCHEMA`]).
    pub schema: String,
    /// Имя рубрики.
    pub rubric: String,
    /// Модель-судья (`None` у записей, где поле не сохранилось).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub judge_model: Option<String>,
    /// Когда снят отчёт (`ГГГГММДД-ЧЧММСС`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub judged_at: Option<String>,
    /// Решение рубрики (`pass` / `fail` / `human`), если записано.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision: Option<String>,
    /// Цель прогона (путь субъекта), если известна: markdown-отчёты её не несут.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    /// E8.3: сколько миллисекунд заняли вызовы судьи.
    #[serde(default)]
    pub duration_ms: u64,
    /// E8.3: токены промптов судьи (0 — провайдер не отдаёт `usage`).
    #[serde(default)]
    pub prompt_tokens: u64,
    /// E8.3: токены ответов судьи.
    #[serde(default)]
    pub completion_tokens: u64,
    /// Оценки критериев.
    #[serde(default)]
    pub scores: Vec<HistoryScore>,
}

/// Оценка одного критерия в истории.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryScore {
    /// Идентификатор критерия.
    pub criterion_id: String,
    /// Итоговый балл (шкала 1..=5).
    #[serde(default)]
    pub score: u8,
    /// Метки достоверности как имена (`evidence_not_found`, …).
    #[serde(default)]
    pub flags: Vec<String>,
    /// Обоснование судьи (из него извлекаются цитаты).
    #[serde(default)]
    pub rationale: String,
}

impl HistoryRecord {
    /// Запись истории из живого отчёта рубрики. Цель — путь субъекта досье,
    /// если прогон был по досье; `judged_at` — метка снятия отчёта (та же, что
    /// в имени файла архива).
    #[must_use]
    pub fn from_report(
        report: &crate::rubric::RubricReport,
        subject: Option<String>,
        judged_at: Option<String>,
    ) -> Self {
        Self {
            schema: HISTORY_SCHEMA.to_string(),
            rubric: report.rubric_name.clone(),
            judge_model: Some(report.judge_model.clone()),
            judged_at,
            decision: report.decision.map(|d| d.as_str().to_string()),
            subject,
            duration_ms: report.judge_duration_ms,
            prompt_tokens: report.judge_prompt_tokens,
            completion_tokens: report.judge_completion_tokens,
            scores: report
                .scores
                .iter()
                .map(|s| HistoryScore {
                    criterion_id: s.criterion_id.clone(),
                    score: s.score,
                    flags: s.flags.iter().map(|f| f.as_str().to_string()).collect(),
                    rationale: s.rationale.clone(),
                })
                .collect(),
        }
    }

    /// Запись читается схемой этого модуля.
    fn is_history_schema(&self) -> bool {
        self.schema == HISTORY_SCHEMA
    }
}

/// Одна находка истории: критерий + балл + цитаты обоснования.
#[derive(Debug, Clone)]
struct Finding {
    run: usize,
    rubric: String,
    subject: Option<String>,
    criterion: String,
    score: u8,
    flags: Vec<String>,
    quotes: Vec<String>,
}

/// Уникальная основа имени отчёта в архиве истории: `rubric-<рубрика>-<метка>`,
/// а если такая пара уже есть (два прогона в одну секунду — метка времени
/// секундная), к метке добавляется счётчик `-2`, `-3`, … Иначе второй прогон
/// молча затирал бы первый, и история (E7.3) и стоимость (E8.3) теряли ревью.
#[must_use]
pub fn unique_history_stem(dir: &Path, rubric: &str, stamp: &str) -> String {
    let base = format!("rubric-{rubric}-{stamp}");
    let taken = |stem: &str| {
        dir.join(format!("{stem}.md")).exists() || dir.join(format!("{stem}.json")).exists()
    };
    if !taken(&base) {
        return base;
    }
    for n in 2..1000 {
        let candidate = format!("{base}-{n}");
        if !taken(&candidate) {
            return candidate;
        }
    }
    base
}

/// Пишет JSON-близнец отчёта в архив истории (`rubric-<рубрика>-<метка>.json`).
/// Рядом с markdown-отчётом живёт машиночитаемая запись: у неё цель и метки —
/// данные, а не текст таблицы, поэтому `rules suggest --from-judge` читает её
/// без разбора markdown.
///
/// # Errors
///
/// Ошибка создания каталога или записи файла.
pub fn write_history_twin(
    dir: &Path,
    stem: &str,
    judged_at: &str,
    report: &crate::rubric::RubricReport,
    subject: Option<String>,
) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join(format!("{stem}.json"));
    let record = HistoryRecord::from_report(report, subject, Some(judged_at.to_string()));
    std::fs::write(&path, serde_json::to_vec_pretty(&record)?)?;
    Ok(path)
}

/// Каталог истории по умолчанию — архив отчётов харнесса (`$ARCH_HOME/reports`).
#[must_use]
pub fn default_history_dir() -> PathBuf {
    crate::config::Config::home_dir().join("reports")
}

/// Читает историю отчётов из каталога: JSON-близнецы (`rubric-*.json`) и
/// markdown (`rubric-*.md`). Порядок стабилен (по имени файла), битые записи
/// пропускаются — история не роняется из-за одного файла.
#[must_use]
pub fn read_history(dir: &Path) -> Vec<HistoryRecord> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    let mut paths: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file())
        .collect();
    paths.sort();
    // Один прогон пишет пару «markdown + JSON-близнец»: считаем его один раз.
    // Близнец приоритетнее (цель и метки — данные), markdown остаётся для
    // отчётов, снятых до его появления.
    let twins: BTreeSet<String> = paths
        .iter()
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .filter_map(|p| p.file_stem().map(|s| s.to_string_lossy().to_string()))
        .collect();
    for path in paths {
        if out.len() >= MAX_HISTORY_FILES {
            break;
        }
        let Some(name) = path.file_name().map(|n| n.to_string_lossy().to_string()) else {
            continue;
        };
        if !name.starts_with("rubric-") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let kind = path
            .extension()
            .map(|e| e.to_string_lossy().to_ascii_lowercase());
        let record = match kind.as_deref() {
            Some("json") => serde_json::from_str::<HistoryRecord>(&text)
                .ok()
                .filter(HistoryRecord::is_history_schema),
            Some("md") => {
                // У отчёта есть JSON-близнец — он уже прочитан этой же парой.
                let stem = path
                    .file_stem()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_default();
                if twins.contains(&stem) {
                    continue;
                }
                parse_report(&text)
            }
            _ => None,
        };
        if let Some(record) = record {
            out.push(record);
        }
    }
    out
}

/// Разбирает markdown-отчёт рубрики (отчёты, снятые до появления JSON-близнеца).
/// Читает заголовок, строку решения, строку судьи и таблицу критериев; цитаты
/// берутся из обоснований. Нераспознанный текст — `None`.
#[must_use]
pub fn parse_report(text: &str) -> Option<HistoryRecord> {
    let rubric = text
        .lines()
        .find_map(|l| l.strip_prefix("# Оценка по рубрике «")?.strip_suffix('»'))
        .map(str::to_string)?;
    let judge_model = text.lines().find_map(|l| {
        l.strip_prefix("**Судья:** ")
            .and_then(|rest| rest.split(" (сэмплов").next())
            .map(str::to_string)
    });
    let decision = text.lines().find_map(|l| {
        l.strip_prefix("**Решение:** ")
            .and_then(|rest| rest.split('`').nth(1))
            .map(str::to_string)
    });
    let mut scores = Vec::new();
    for line in text.lines() {
        let Some(row) = line.strip_prefix("| ").and_then(|l| l.strip_suffix(" |")) else {
            continue;
        };
        // Заголовок и разделитель таблицы отсекаются по форме балла.
        let cells: Vec<&str> = row.split(" | ").collect();
        if cells.len() != 5 {
            continue;
        }
        let criterion_id = cells[0].trim();
        let Ok(score) = cells[2].trim().parse::<u8>() else {
            continue;
        };
        if criterion_id.is_empty() || criterion_id == "Критерий" {
            continue;
        }
        let flags = cells[3]
            .split(", ")
            .filter_map(|f| f.split(" (").next())
            .map(str::trim)
            .filter(|f| !f.is_empty())
            .map(str::to_string)
            .collect();
        scores.push(HistoryScore {
            criterion_id: criterion_id.to_string(),
            score,
            flags,
            rationale: cells[4].replace("\\|", "|"),
        });
    }
    if scores.is_empty() {
        return None;
    }
    Some(HistoryRecord {
        schema: HISTORY_SCHEMA.to_string(),
        rubric,
        judge_model,
        judged_at: None,
        decision,
        subject: None,
        duration_ms: 0,
        prompt_tokens: 0,
        completion_tokens: 0,
        scores,
    })
}

/// Цитаты из обоснования судьи: формы `Цитата subject: "…"`, `Цитата
/// reference: "…"` (берём только субъекта: детерминировать можно нарушение в
/// коде, а не формулировку эталона) и `Цитата: "…"`.
fn quotes_of(rationale: &str) -> Vec<String> {
    const LEAD: [&str; 3] = ["Цитата subject: \"", "Цитата код: \"", "Цитата: \""];
    let mut out = Vec::new();
    for lead in LEAD {
        let mut rest = rationale;
        while let Some(pos) = rest.find(lead) {
            let tail = &rest[pos + lead.len()..];
            if let Some(end) = tail.find('"') {
                let quote = normalize_quote(&tail[..end]);
                if !quote.is_empty() && !out.contains(&quote) {
                    out.push(quote);
                }
            }
            rest = tail;
        }
    }
    out
}

/// Нормализация цитаты для сравнения между прогонами: пробелы сжимаются,
/// края обрезаются. Регистр сохраняется — код регистрозависим.
fn normalize_quote(raw: &str) -> String {
    raw.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Кандидаты, собранные из истории.
///
/// `suggest` — чистая функция от прочитанной истории: одна и та же история
/// даёт один и тот же список (порядок детерминирован), поэтому кандидатов
/// можно сравнивать между прогонами и класть в ревью.
#[must_use]
pub fn suggest(history: &[HistoryRecord], min_runs: usize) -> SuggestReport {
    let min_runs = min_runs.max(2);
    let findings = collect_findings(history);
    let mut candidates = accusation_candidates(&findings, min_runs);
    candidates.extend(flag_candidates(history, min_runs));
    let summary = if candidates.is_empty() {
        format!(
            "Повторяющихся находок судьи не найдено: {} прогонов истории, порог {}.",
            history.len(),
            min_runs
        )
    } else {
        format!(
            "Из истории отчётов судьи ({} прогонов, {} оценок критериев): кандидатов {} — \
             механизируемых {}, advisory {}.",
            history.len(),
            findings.len(),
            candidates.len(),
            candidates.iter().filter(|c| c.yaml.is_some()).count(),
            candidates.iter().filter(|c| c.yaml.is_none()).count()
        )
    };
    SuggestReport {
        candidates,
        summary,
        empty_note: Some(
            "История не даёт кандидатов: находки единичны или ниже порога повторяемости. \
             Прогоните рубрику на большем числе целей или снизьте --min-runs."
                .to_string(),
        ),
    }
}

/// Все находки истории: по одной на критерий каждого прогона, с цитатами.
fn collect_findings(history: &[HistoryRecord]) -> Vec<Finding> {
    let mut out = Vec::new();
    for (run, record) in history.iter().enumerate() {
        for score in &record.scores {
            out.push(Finding {
                run,
                rubric: record.rubric.clone(),
                subject: record.subject.clone(),
                criterion: score.criterion_id.clone(),
                score: score.score,
                flags: score.flags.clone(),
                quotes: quotes_of(&score.rationale),
            });
        }
    }
    out
}

/// Обвинения, повторяющиеся в прогонах: цитата-нарушение становится
/// запрещённым паттерном (YAML `must_not_contain`).
fn accusation_candidates(findings: &[Finding], min_runs: usize) -> Vec<Candidate> {
    // Ключ — (критерий, цитата): одна и та же строка, названная нарушением
    // в разных прогонах. Значения — прогоны, цели и рубрики этой группы.
    struct Group {
        runs: BTreeSet<usize>,
        subjects: BTreeSet<String>,
        rubrics: BTreeSet<String>,
    }
    let mut groups: BTreeMap<(String, String), Group> = BTreeMap::new();
    for f in findings {
        if f.score > ACCUSATION_MAX_SCORE {
            continue;
        }
        // Обвинение с неподтверждённой цитатой правилом не становится:
        // механика не подтвердила, что цитата вообще есть в субъекте, и правило
        // `must_not_contain` кодировало бы ошибку модели, а не дефект кода.
        // Такие находки видны человеку отдельно — advisory по метке
        // ([`flag_candidates`]).
        if f.flags
            .iter()
            .any(|flag| flag == "accusation_unconfirmed" || flag == "evidence_not_found")
        {
            continue;
        }
        for quote in &f.quotes {
            let entry = groups
                .entry((f.criterion.clone(), quote.clone()))
                .or_insert_with(|| Group {
                    runs: BTreeSet::new(),
                    subjects: BTreeSet::new(),
                    rubrics: BTreeSet::new(),
                });
            entry.runs.insert(f.run);
            entry.rubrics.insert(f.rubric.clone());
            if let Some(subject) = &f.subject {
                entry.subjects.insert(subject.clone());
            }
        }
    }
    let mut ready: Vec<(String, String, Group)> = groups
        .into_iter()
        .filter(|(_, g)| {
            // Повтор в разных прогонах; если цели известны, они должны быть
            // разными — иначе это один и тот же файл, пересуженный много раз.
            g.runs.len() >= min_runs && (g.subjects.is_empty() || g.subjects.len() >= 2)
        })
        .map(|((criterion, quote), group)| (criterion, quote, group))
        .collect();
    // Больше повторов — выше в списке; дальше по критерию и цитате (стабильно).
    ready.sort_by(|a, b| {
        b.2.runs
            .len()
            .cmp(&a.2.runs.len())
            .then_with(|| a.0.cmp(&b.0))
            .then_with(|| a.1.cmp(&b.1))
    });
    let mut counters: BTreeMap<String, usize> = BTreeMap::new();
    let mut out = Vec::new();
    for (criterion, quote, group) in ready {
        let runs = group.runs.len();
        let subjects: Vec<String> = group.subjects.into_iter().collect();
        let rubrics: Vec<String> = group.rubrics.into_iter().collect();
        let n = counters.entry(criterion.clone()).or_insert(0);
        *n += 1;
        let name = format!("judge_{}_{}", slug(&criterion), n);
        let glob = glob_for(&subjects);
        let rationale = format!(
            "Судья называл нарушением критерий «{criterion}» рубрики «{}» в {runs} прогонах \
             (повторяющаяся цитата: `{quote}`). Класс закрывается механикой: \
             запрещённый паттерн воспроизводим, не зависит от модели и не стоит вызова.",
            rubrics.join("», «"),
        );
        let fix_hint = format!(
            "Убрать паттерн из кода или, если он законен, заменить кандидата на \
             контрактный тест, проверяющий поведение «{criterion}»."
        );
        let yaml = crate::rules_suggest::yaml_must_not_contain(
            &name,
            &glob,
            &regex_escape(&quote),
            &rationale,
            &fix_hint,
            "fitness-functions",
        );
        let candidate = Candidate::from_history(
            &format!("judge-accusation-{}-{n}", slug(&criterion)),
            format!(
                "{} Цели: {}.",
                rationale,
                if subjects.is_empty() {
                    "не записаны в отчётах".to_string()
                } else {
                    subjects.join(", ")
                }
            ),
            "fitness-functions",
            Some(yaml),
        );
        out.push(candidate);
    }
    out
}

/// Метки механики, повторяющиеся на одном критерии: честный advisory без YAML
/// (какой детерминированный детектор нужен — из истории не следует).
fn flag_candidates(history: &[HistoryRecord], min_runs: usize) -> Vec<Candidate> {
    let mut groups: BTreeMap<(String, String, String), BTreeSet<usize>> = BTreeMap::new();
    for (run, record) in history.iter().enumerate() {
        for score in &record.scores {
            for flag in &score.flags {
                groups
                    .entry((
                        record.rubric.clone(),
                        score.criterion_id.clone(),
                        flag.clone(),
                    ))
                    .or_default()
                    .insert(run);
            }
        }
    }
    let mut ready: Vec<(String, String, String, usize)> = groups
        .into_iter()
        .filter(|(_, runs)| runs.len() >= min_runs)
        .map(|((rubric, criterion, flag), runs)| (rubric, criterion, flag, runs.len()))
        .collect();
    ready.sort_by(|a, b| {
        b.3.cmp(&a.3)
            .then_with(|| a.0.cmp(&b.0))
            .then_with(|| a.1.cmp(&b.1))
            .then_with(|| a.2.cmp(&b.2))
    });
    ready
        .into_iter()
        .map(|(rubric, criterion, flag, runs)| {
            Candidate::from_history(
                &format!("judge-flag-{flag}-{}", slug(&criterion)),
                format!(
                    "Метка `{flag}` на критерии «{criterion}» рубрики «{rubric}» в {runs} \
                     прогонах: механика систематически не подтверждает суждение судьи. \
                     Нужен детерминированный детектор этого класса или пересмотр критерия — \
                     автоматического правила из истории не следует."
                ),
                "adversarial-review",
                None,
            )
        })
        .collect()
}

/// Glob набора файлов по расширениям целей: одно расширение — `**/*.py`,
/// иначе `**/*` (история не даёт оснований сузить набор).
fn glob_for(subjects: &[String]) -> String {
    let extensions: BTreeSet<String> = subjects
        .iter()
        .filter_map(|s| {
            Path::new(s)
                .extension()
                .map(|e| e.to_string_lossy().to_string())
        })
        .collect();
    match extensions.len() {
        1 => format!(
            "**/*.{}",
            extensions.iter().next().unwrap_or(&String::new())
        ),
        _ => "**/*".to_string(),
    }
}

/// kebab-case из идентификатора критерия: `no_violation` → `no-violation`.
fn slug(raw: &str) -> String {
    raw.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}

/// Экранирование regex-метасимволов цитаты: паттерн правила — регулярное
/// выражение, а цитата из кода — литерал.
fn regex_escape(raw: &str) -> String {
    regex::escape(raw)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(
        rubric: &str,
        subject: &str,
        score: u8,
        flag: Option<&str>,
        rationale: &str,
    ) -> HistoryRecord {
        HistoryRecord {
            schema: HISTORY_SCHEMA.to_string(),
            rubric: rubric.to_string(),
            judge_model: Some("test-judge".to_string()),
            judged_at: Some("20260925-120000".to_string()),
            decision: Some("fail".to_string()),
            subject: Some(subject.to_string()),
            duration_ms: 0,
            prompt_tokens: 0,
            completion_tokens: 0,
            scores: vec![HistoryScore {
                criterion_id: "no_violation".to_string(),
                score,
                flags: flag.map(|f| vec![f.to_string()]).unwrap_or_default(),
                rationale: rationale.to_string(),
            }],
        }
    }

    fn accusation(subject: &str) -> HistoryRecord {
        record(
            "code_invariant_conformance",
            subject,
            1,
            None,
            "Цитата subject: \"self.charged.append(amount_minor)\". Цитата reference: \"AD-1\". Нарушение.",
        )
    }

    #[test]
    fn repeated_accusation_becomes_forbidden_pattern_rule() {
        let history = vec![
            accusation("src/pay.py"),
            accusation("src/ledger.py"),
            accusation("src/order.py"),
        ];
        let report = suggest(&history, MIN_RUNS);
        assert_eq!(report.candidates.len(), 1, "ожидается один кандидат");
        let c = &report.candidates[0];
        assert!(
            c.id.starts_with("judge-accusation-no-violation"),
            "id кандидата: {}",
            c.id
        );
        let yaml = c.yaml.as_deref().expect("обвинение механизируемо");
        assert!(yaml.contains("type: must_not_contain"), "yaml: {yaml}");
        assert!(
            yaml.contains("self\\.charged\\.append\\(amount_minor\\)"),
            "yaml: {yaml}"
        );
        assert!(yaml.contains("glob: '**/*.py'"), "yaml: {yaml}");
    }

    #[test]
    fn single_file_reviewed_many_times_is_not_a_rule() {
        let history = vec![
            accusation("src/pay.py"),
            accusation("src/pay.py"),
            accusation("src/pay.py"),
        ];
        let report = suggest(&history, MIN_RUNS);
        assert!(
            report.candidates.is_empty(),
            "один и тот же файл — не повторяемость класса: {:?}",
            report.candidates
        );
    }

    #[test]
    fn below_threshold_history_gives_no_candidates() {
        let history = vec![accusation("src/pay.py"), accusation("src/ledger.py")];
        let report = suggest(&history, MIN_RUNS);
        assert!(report.candidates.is_empty());
        assert!(report.summary.contains("не найдено"), "{}", report.summary);
    }

    #[test]
    fn clean_verdicts_never_become_rules() {
        let history: Vec<HistoryRecord> = ["src/a.py", "src/b.py", "src/c.py"]
            .iter()
            .map(|s| {
                record(
                    "code_invariant_conformance",
                    s,
                    5,
                    None,
                    "Цитата subject: \"def authorize\". Нарушений нет.",
                )
            })
            .collect();
        assert!(suggest(&history, MIN_RUNS).candidates.is_empty());
    }

    #[test]
    fn repeated_mechanics_flag_is_advisory_without_yaml() {
        let history: Vec<HistoryRecord> = ["src/a.py", "src/b.py", "src/c.py"]
            .iter()
            .map(|s| {
                record(
                    "adr_quality",
                    s,
                    3,
                    Some("evidence_not_found"),
                    "Нарушений не вижу.",
                )
            })
            .collect();
        let report = suggest(&history, MIN_RUNS);
        assert_eq!(report.candidates.len(), 1);
        let c = &report.candidates[0];
        assert!(c.yaml.is_none(), "advisory не выдумывает механику");
        assert!(
            c.rationale.contains("evidence_not_found"),
            "{}",
            c.rationale
        );
        assert_eq!(c.source_skill, "adversarial-review");
    }

    #[test]
    fn min_runs_below_two_is_clamped() {
        let history = vec![accusation("src/pay.py")];
        // Один прогон с порогом 0 не становится правилом: повтор — минимум два.
        assert!(suggest(&history, 0).candidates.is_empty());
    }

    /// Два прогона в одну секунду не затирают друг друга (метка времени
    /// секундная): основа имени получает счётчик.
    #[test]
    fn history_stem_survives_two_runs_in_one_second() {
        let dir = std::env::temp_dir().join(format!("judge-stem-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("временный каталог");
        let first = unique_history_stem(&dir, "code_vs_spine", "20260925-120000");
        assert_eq!(first, "rubric-code_vs_spine-20260925-120000");
        std::fs::write(dir.join(format!("{first}.md")), "x").expect("первый отчёт");
        let second = unique_history_stem(&dir, "code_vs_spine", "20260925-120000");
        assert_eq!(second, "rubric-code_vs_spine-20260925-120000-2");
        std::fs::write(dir.join(format!("{second}.json")), "x").expect("близнец второго");
        assert_eq!(
            unique_history_stem(&dir, "code_vs_spine", "20260925-120000"),
            "rubric-code_vs_spine-20260925-120000-3"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Пара «markdown + JSON-близнец» одного прогона считается одним прогоном:
    /// иначе порог повторяемости достигался бы вдвое меньшим числом прогонов.
    #[test]
    fn markdown_twin_pair_counts_as_one_run() {
        let dir = std::env::temp_dir().join(format!("judge-rules-pair-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("временный каталог");
        let record = accusation("src/pay.py");
        let md = "# Оценка по рубрике «code_invariant_conformance»\n\n\
                  | Критерий | Вес | Балл | Метки | Обоснование |\n\
                  | --- | --- | --- | --- | --- |\n\
                  | no_violation | 3.00 | 1 |  | Цитата subject: \"self.charged.append(amount_minor)\". |\n";
        std::fs::write(
            dir.join("rubric-code_invariant_conformance-20260925-120000.md"),
            md,
        )
        .expect("markdown");
        std::fs::write(
            dir.join("rubric-code_invariant_conformance-20260925-120000.json"),
            serde_json::to_string(&record).expect("json"),
        )
        .expect("близнец");
        let history = read_history(&dir);
        assert_eq!(history.len(), 1, "пара — один прогон");
        assert_eq!(
            history[0].subject.as_deref(),
            Some("src/pay.py"),
            "читается близнец"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Обвинение с неподтверждённой цитатой не механизируется: правило
    /// `must_not_contain` из несуществующей строки — это закреплённая ошибка
    /// модели. Повтор таких обвинений виден advisory-кандидатом по метке.
    #[test]
    fn unconfirmed_accusation_never_becomes_a_rule() {
        let history: Vec<HistoryRecord> = ["src/a.py", "src/b.py", "src/c.py"]
            .iter()
            .map(|s| {
                record(
                    "code_invariant_conformance",
                    s,
                    1,
                    Some("accusation_unconfirmed"),
                    "Цитата subject: \"self.charged.append(amount_minor)\". Нарушение.",
                )
            })
            .collect();
        let report = suggest(&history, MIN_RUNS);
        assert!(
            report.candidates.iter().all(|c| c.yaml.is_none()),
            "механизируемых правил из неподтверждённых обвинений нет: {:?}",
            report.candidates
        );
        assert!(
            report
                .candidates
                .iter()
                .any(|c| c.id.contains("judge-flag-accusation_unconfirmed")),
            "повтор неподтверждённых обвинений остаётся видимым advisory: {:?}",
            report.candidates
        );
    }

    /// Охранный тест E7.3: каждый сгенерированный фрагмент `must_not_contain`
    /// обязан парситься YAML, компилироваться как regex и приниматься боевой
    /// схемой реестра (`control::load_fitness_rules`) — иначе кандидат красив,
    /// но не загружается.
    #[test]
    fn generated_forbidden_pattern_fragment_loads_into_control_schema() {
        let history = vec![
            accusation("src/pay.py"),
            accusation("src/ledger.py"),
            accusation("src/order.py"),
        ];
        let report = suggest(&history, MIN_RUNS);
        let candidate = report
            .candidates
            .iter()
            .find(|c| c.yaml.is_some())
            .expect("механизируемый кандидат");
        let yaml = candidate.yaml.as_deref().expect("yaml");
        let doc = format!("rules:\n{yaml}\n");
        let parsed: serde_yaml_ng::Value =
            serde_yaml_ng::from_str(&doc).unwrap_or_else(|e| panic!("не парсится: {e}\n{doc}"));
        let rule = &parsed["rules"][0];
        assert_eq!(rule["type"].as_str(), Some("must_not_contain"));
        let pattern = rule["pattern"].as_str().expect("pattern — строка");
        regex::Regex::new(pattern).unwrap_or_else(|e| panic!("regex не компилируется: {e}"));
        let tmp = tempfile::tempdir().expect("tmp");
        let path = tmp.path().join("CONSTRAINTS.yaml");
        std::fs::write(&path, &doc).expect("write");
        let loaded = crate::control::load_fitness_rules(&path)
            .unwrap_or_else(|e| panic!("боевая схема не принимает: {e}\n{doc}"));
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].kind.as_str(), "must_not_contain");
    }

    /// Охранный тест парсера: markdown, снятый живым отчётом, читается обратно
    /// без потерь по критериям, меткам и цитатам. Формат отчёта — контракт
    /// между `rubric run` (пишет) и `rules suggest` (читает историю).
    #[test]
    fn live_report_markdown_roundtrips_into_history() {
        use crate::rubric::{CriterionFlag, RubricDecision, RubricReport};
        let score = |id: &str, score: u8, flags: Vec<CriterionFlag>, rationale: &str| {
            crate::rubric::CriterionScore {
                criterion_id: id.to_string(),
                weight: 1.0,
                score,
                rationale: rationale.to_string(),
                samples: vec![score],
                stdev: 0.0,
                flags,
                evidence_unconfirmed_ratio: 0.0,
                invalid_samples: 0,
                checked: Vec::new(),
            }
        };
        let report = RubricReport {
            rubric_name: "code_invariant_conformance".to_string(),
            judge_model: "deepseek-chat".to_string(),
            judge_samples: 3,
            scores: vec![
                score(
                    "no_violation",
                    1,
                    Vec::new(),
                    "Цитата subject: \"self.charged.append(amount_minor)\". Нарушение.",
                ),
                score(
                    "bypass_paths",
                    3,
                    vec![CriterionFlag::EvidencePartial],
                    "Цитата: \"def authorize\". Частично.",
                ),
            ],
            weighted_total: 2.0,
            verdict: "тест".to_string(),
            evidence_unconfirmed_ratio: 0.0,
            input_injections: Vec::new(),
            invalid_samples_ratio: 0.0,
            decision: Some(RubricDecision::Fail),
            decision_reasons: vec!["подтверждённое нарушение".to_string()],
            judge_duration_ms: 0,
            judge_prompt_tokens: 0,
            judge_completion_tokens: 0,
        };
        let parsed = parse_report(&report.to_markdown()).expect("markdown разбирается");
        assert_eq!(parsed.rubric, report.rubric_name);
        assert_eq!(parsed.judge_model.as_deref(), Some("deepseek-chat"));
        assert_eq!(parsed.decision.as_deref(), Some("fail"));
        assert_eq!(parsed.scores.len(), 2);
        assert_eq!(parsed.scores[0].score, 1);
        assert_eq!(parsed.scores[1].flags, vec!["evidence_partial".to_string()]);
        assert_eq!(
            quotes_of(&parsed.scores[0].rationale),
            vec!["self.charged.append(amount_minor)".to_string()]
        );
        // Запись из живого отчёта несёт цель и решение как данные (JSON-близнец).
        let twin = HistoryRecord::from_report(
            &report,
            Some("src/pay.py".to_string()),
            Some("20260925-120000".to_string()),
        );
        assert_eq!(twin.subject.as_deref(), Some("src/pay.py"));
        let json = serde_json::to_string(&twin).expect("сериализация близнеца");
        let back: HistoryRecord = serde_json::from_str(&json).expect("чтение близнеца");
        assert_eq!(back.scores[0].criterion_id, "no_violation");
    }

    #[test]
    fn parses_legacy_markdown_report() {
        let text = "\
# Оценка по рубрике «code_invariant_conformance»

**Решение:** не годно (`fail`, код выхода 1)
- критерий 'no_violation' = 1 при подтверждённых цитатах

| Критерий | Вес | Балл | Метки | Обоснование |
| --- | --- | --- | --- | --- |
| no_violation | 3.00 | 1 |  | Цитата subject: \"self.charged.append(amount_minor)\". Нарушение. |
| invariant_enforced | 2.00 | 3 | evidence_partial | Цитата: \"def authorize\". Частично. |

**Судья:** deepseek-chat (сэмплов на критерий: 3)
**Дата:** 2026-09-25 10:56:49
";
        let record = parse_report(text).expect("отчёт разбирается");
        assert_eq!(record.rubric, "code_invariant_conformance");
        assert_eq!(record.decision.as_deref(), Some("fail"));
        assert_eq!(record.judge_model.as_deref(), Some("deepseek-chat"));
        assert_eq!(record.scores.len(), 2);
        assert_eq!(record.scores[0].score, 1);
        assert!(
            record.scores[1]
                .flags
                .contains(&"evidence_partial".to_string())
        );
        let quotes = quotes_of(&record.scores[0].rationale);
        assert_eq!(
            quotes,
            vec!["self.charged.append(amount_minor)".to_string()]
        );
    }

    #[test]
    fn markdown_history_is_read_and_grouped() {
        let dir = std::env::temp_dir().join(format!("judge-rules-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("временный каталог");
        for (i, subject) in ["a", "b", "c"].iter().enumerate() {
            let text = format!(
                "# Оценка по рубрике «code_invariant_conformance»\n\n\
                 **Решение:** не годно (`fail`, код выхода 1)\n\n\
                 | Критерий | Вес | Балл | Метки | Обоснование |\n\
                 | --- | --- | --- | --- | --- |\n\
                 | no_violation | 3.00 | 1 |  | Цитата subject: \"self.charged.append(amount_minor)\". {subject} |\n\n\
                 **Судья:** m (сэмплов на критерий: 1)\n"
            );
            std::fs::write(
                dir.join(format!(
                    "rubric-code_invariant_conformance-2026092{i}-120000.md"
                )),
                text,
            )
            .expect("запись отчёта");
        }
        let history = read_history(&dir);
        assert_eq!(history.len(), 3, "три markdown-отчёта");
        let report = suggest(&history, MIN_RUNS);
        assert_eq!(
            report.candidates.len(),
            1,
            "markdown-история без целей даёт кандидата по числу прогонов: {:?}",
            report.candidates
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn json_twin_roundtrips_through_history() {
        let dir = std::env::temp_dir().join(format!("judge-rules-json-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("временный каталог");
        let record = accusation("src/pay.py");
        std::fs::write(
            dir.join("rubric-code_invariant_conformance-20260925-120000.json"),
            serde_json::to_string(&record).expect("сериализация"),
        )
        .expect("запись близнеца");
        let history = read_history(&dir);
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].subject.as_deref(), Some("src/pay.py"));
        assert_eq!(history[0].scores[0].score, 1);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn foreign_json_in_archive_is_skipped() {
        let dir = std::env::temp_dir().join(format!("judge-rules-foreign-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("временный каталог");
        std::fs::write(
            dir.join("rubric-bogus-20260925-120000.json"),
            "{\"schema\": \"other/9\", \"rubric\": \"x\"}",
        )
        .expect("запись");
        assert!(read_history(&dir).is_empty(), "чужая схема не читается");
        std::fs::remove_dir_all(&dir).ok();
    }
}
