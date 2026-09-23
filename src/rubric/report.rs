//! Сборка отчёта рубрики (B1): разбор сырых ответов судьи, механическая сверка
//! цитат-свидетельств (по целому тексту и по ролям источников досье, ADR-051),
//! медиана и разброс сэмплов, взвешенный итог, потолок вердикта (ADR-004).

use std::fmt::Write as _;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::config::JudgeConfig;
use crate::error::{HarnessError, Result};

use super::types::{Criterion, CriterionFlag, CriterionScore, Rubric};

/// Сколько символов ответа модели включается в сообщение об ошибке разбора.
const ERR_FRAGMENT_CHARS: usize = 400;

/// Минимальная длина цитаты-свидетельства в символах: более короткий
/// quoted-span — слово в кавычках, а не свидетельство, и не засчитывается.
const MIN_QUOTE_CHARS: usize = 8;

/// Отчёт по рубрике.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RubricReport {
    /// Имя рубрики.
    pub rubric_name: String,
    /// Модель-судья.
    pub judge_model: String,
    /// Сэмплов судьи на критерий (k из [`JudgeConfig::samples`]).
    #[serde(default)]
    pub judge_samples: usize,
    /// Оценки по критериям.
    pub scores: Vec<CriterionScore>,
    /// Взвешенный итог (`0..=scale_max`) по засчитанным критериям.
    pub weighted_total: f64,
    /// Общий вердикт судьи (из последнего сэмпла).
    pub verdict: String,
    /// Доля сэмплов судьи с неподтверждённой цитатой (Д10): сколько
    /// свидетельств из присланных не подтвердилось оцениваемым текстом.
    /// `0.0` — все цитаты подтверждены (в том числе у отчётов, снятых до 0.3.5:
    /// поле аддитивное, отсутствие читается как ноль).
    #[serde(default)]
    pub evidence_unconfirmed_ratio: f64,
}

impl RubricReport {
    /// Markdown-представление отчёта: заголовок, таблица баллов по критериям
    /// (критерий | вес | балл | метки | обоснование), взвешенный итог со
    /// списком исключённых критериев (`evidence_not_found`), вердикт, имя
    /// судьи с числом сэмплов и дата формирования.
    #[must_use]
    pub fn to_markdown(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(out, "# Оценка по рубрике «{}»\n", self.rubric_name);
        let _ = writeln!(out, "| Критерий | Вес | Балл | Метки | Обоснование |");
        let _ = writeln!(out, "| --- | --- | --- | --- | --- |");
        for s in &self.scores {
            let rationale = s.rationale.replace('|', "\\|").replace(['\n', '\r'], " ");
            let flags = s
                .flags
                .iter()
                .map(|f| match f {
                    CriterionFlag::Unstable => format!("unstable (σ={:.2})", s.stdev),
                    // `evidence_not_found` — критерий исключён из итога;
                    // `evidence_partial` (Д10) — балл засчитан, но часть
                    // сэмплов пришла с неподтверждённой цитатой. Все метки
                    // печатаются своим именем: читателю важна разница.
                    CriterionFlag::EvidenceNotFound
                    | CriterionFlag::EvidencePartial
                    | CriterionFlag::AccusationUnconfirmed
                    | CriterionFlag::CoverageIncomplete => f.as_str().to_string(),
                })
                .collect::<Vec<_>>()
                .join(", ");
            // Балл с неподтверждённой цитатой печатается с маркером ⚠: голая
            // пятёрка не должна читаться глазами как полноценная (штраф уже
            // учтён во взвешенном итоге — см. легенду под таблицей).
            let score_cell = if s.has_flag(CriterionFlag::EvidenceNotFound) {
                format!("{} ⚠", s.score)
            } else {
                s.score.to_string()
            };
            let _ = writeln!(
                out,
                "| {} | {:.2} | {} | {} | {} |",
                s.criterion_id, s.weight, score_cell, flags, rationale
            );
        }
        let _ = writeln!(out, "\n**Взвешенный итог:** {:.2}/5", self.weighted_total);
        let excluded: Vec<&str> = self
            .scores
            .iter()
            .filter(|s| s.has_flag(CriterionFlag::EvidenceNotFound))
            .map(|s| s.criterion_id.as_str())
            .collect();
        if !excluded.is_empty() {
            let _ = writeln!(
                out,
                "**В итог не засчитаны (evidence_not_found):** {}",
                excluded.join(", ")
            );
            let _ = writeln!(
                out,
                "⚠ — оценка с неподтверждённой цитатой: критерий исключён из взвешенного \
                 итога (штраф учтён выше), а вердикт механически ограничен CONCERNS."
            );
        }
        // Д10: часть сэмплов не подтвердилась, но балл засчитан по остальным —
        // отдельная строка: «засчитано» и «подтверждено» здесь расходятся.
        let partial: Vec<String> = self
            .scores
            .iter()
            .filter(|s| s.has_flag(CriterionFlag::EvidencePartial))
            .map(|s| {
                format!(
                    "{} (не подтвердилось {:.0}% сэмплов)",
                    s.criterion_id,
                    s.evidence_unconfirmed_ratio * 100.0
                )
            })
            .collect();
        if !partial.is_empty() {
            let _ = writeln!(
                out,
                "**Свидетельства частично не подтвердились (evidence_partial):** {}",
                partial.join(", ")
            );
        }
        // Смысловые рубрики (ADR-051): два своих повода исключить критерий —
        // и оба читателю важно отличить от выдуманной похвалы выше.
        let by_flag = |flag: CriterionFlag| -> Vec<&str> {
            self.scores
                .iter()
                .filter(|s| s.has_flag(flag))
                .map(|s| s.criterion_id.as_str())
                .collect()
        };
        let accusations = by_flag(CriterionFlag::AccusationUnconfirmed);
        if !accusations.is_empty() {
            let _ = writeln!(
                out,
                "**Обвинение не подтверждено (accusation_unconfirmed):** {}",
                accusations.join(", ")
            );
            let _ = writeln!(
                out,
                "⚠ — низкий балл без подтверждённых цитат по требуемым ролям: критерий \
                 исключён из взвешенного итога, но блокирующей находкой не становится — \
                 выдуманное обвинение наказывает судью, а не документ."
            );
        }
        let uncovered = by_flag(CriterionFlag::CoverageIncomplete);
        if !uncovered.is_empty() {
            let _ = writeln!(
                out,
                "**Покрытие неполно (coverage_incomplete):** {}",
                uncovered.join(", ")
            );
            let _ = writeln!(
                out,
                "⚠ — высокий балл без полного перечня проверенных источников досье: критерий \
                 исключён из итога — «противоречий нет» должно быть названо поимённо."
            );
        }
        let _ = writeln!(out, "**Вердикт:** {}", self.verdict);
        let _ = writeln!(
            out,
            "**Судья:** {} (сэмплов на критерий: {})",
            self.judge_model, self.judge_samples
        );
        let _ = writeln!(
            out,
            "**Дата:** {}",
            chrono::Local::now().format("%Y-%m-%d %H:%M:%S")
        );
        out
    }
}

/// Сырой ответ судьи (JSON). `pub(crate)` — разбор ответов хоста в
/// split-judge (`mcp_server::rubric_verify`); поля закрыты, сборка отчёта —
/// только через [`build_report`].
#[derive(Debug, Deserialize)]
pub(crate) struct JudgeResponse {
    /// Оценки по критериям (могут покрывать не все).
    #[serde(default)]
    scores: Vec<JudgeScore>,
    /// Общий вердикт.
    #[serde(default)]
    verdict: String,
}

/// Сырая оценка одного критерия от судьи.
#[derive(Debug, Deserialize)]
struct JudgeScore {
    /// Идентификатор критерия.
    criterion_id: String,
    /// Балл (терпимо: число или строка с числом).
    #[serde(default, deserialize_with = "de_lenient_f64")]
    score: f64,
    /// Обоснование.
    #[serde(default)]
    rationale: String,
    /// Перечень проверенных источников досье (ADR-051, S3): заполняется
    /// критериями с `coverage`; у остальных пусто.
    #[serde(default)]
    checked: Vec<String>,
}

/// Терпимый разбор балла: JSON-число или строка с числом.
fn de_lenient_f64<'de, D>(deserializer: D) -> std::result::Result<f64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    match Value::deserialize(deserializer)? {
        Value::Number(n) => n
            .as_f64()
            .ok_or_else(|| serde::de::Error::custom("балл не число")),
        Value::String(s) => s
            .trim()
            .parse::<f64>()
            .map_err(|_| serde::de::Error::custom("балл не число")),
        _ => Err(serde::de::Error::custom("балл должен быть числом")),
    }
}

/// Извлекает JSON-объект из ответа: от первой `{` до последней `}`
/// (терпимо к ` ```json `-обёрткам и тексту до/после).
fn extract_json_object(text: &str) -> Option<&str> {
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    (end > start).then(|| &text[start..=end])
}

/// Разбирает JSON-ответ судьи (с извлечением объекта из обёртки).
/// `pub(crate)` — split-judge `mcp_server` разбирает ответы хоста тем же
/// парсером, что и встроенный судья.
pub(crate) fn parse_judge_response(text: &str) -> Result<JudgeResponse> {
    let json = extract_json_object(text).ok_or_else(|| {
        HarnessError::Rubric(format!(
            "в ответе судьи нет JSON-объекта: {}",
            fragment(text)
        ))
    })?;
    serde_json::from_str(json)
        .map_err(|e| HarnessError::Rubric(format!("разбор JSON судьи: {e}: {}", fragment(json))))
}

/// Чем проверяются цитаты-свидетельства: один документ или досье с поимёнными
/// источниками по ролям (ADR-051).
///
/// Без разделения по ролям цитата из ADR засчитывалась бы как цитата из спайна:
/// обвинение «решение противоречит инварианту» подтверждалось бы половиной
/// доказательства.
#[derive(Debug, Clone, Copy)]
pub(crate) enum EvidenceScope<'a> {
    /// Один текст (поведение 0.3.4).
    Target(&'a str),
    /// Досье: у каждой роли свой текст, цитата сверяется только со своим.
    Pack(&'a crate::rubric_pack::ContextPack),
}

impl EvidenceScope<'_> {
    /// Текст целиком — для критериев без ролей.
    pub(super) fn whole(&self) -> &str {
        match self {
            Self::Target(t) => t,
            Self::Pack(p) => &p.text,
        }
    }

    /// Тексты источников роли; пусто — роль в досье не представлена.
    fn role_texts(&self, role: crate::rubric_pack::InputRole) -> Vec<&str> {
        match self {
            Self::Target(t) => vec![t],
            Self::Pack(p) => p.role_texts(role),
        }
    }

    /// Идентификаторы ссылочных источников досье (`AD-1`, `CMP-002`, …);
    /// `None` — оценка идёт по документу без досье, и сверять покрытие не с чем.
    fn reference_ids(&self) -> Option<Vec<String>> {
        match self {
            Self::Target(_) => None,
            Self::Pack(p) => Some(p.references().iter().map(|i| i.key().to_string()).collect()),
        }
    }
}

/// С какого балла критерий с `coverage` обязан назвать проверенные источники
/// (ADR-051, S3): «всё чисто» цитатой не докажешь, но перечень проверенного
/// требуется — иначе пятёрку можно поставить не глядя.
pub(super) const COVERAGE_MIN_SCORE: u8 = 4;

/// Назвал ли судья все ссылочные источники досье: сверяется КАЖДЫЙ сэмпл с
/// высоким баллом, а не объединение перечней — иначе источники, названные по
/// одному в разных сэмплах, сошли бы за один полный перечень.
fn coverage_incomplete(
    runs: &[JudgeResponse],
    criterion_id: &str,
    ids: &[String],
    scale_max: u8,
) -> bool {
    runs.iter().any(|run| {
        let Some(sample) = run.scores.iter().find(|s| s.criterion_id == criterion_id) else {
            // Пропуск судьёй — не «не глядя»: балла нет, требовать нечего.
            return false;
        };
        if clamp_score(sample.score, scale_max) < COVERAGE_MIN_SCORE {
            return false;
        }
        ids.iter().any(|id| {
            let want = id.trim().to_lowercase();
            !sample
                .checked
                .iter()
                .any(|got| got.trim().to_lowercase() == want)
        })
    })
}

/// Подтверждены ли все требуемые критерием цитаты в одном обосновании.
///
/// Для критерия без ролей требование одно и проверяется по всему тексту; для
/// критерия с ролями — по цитате на роль, каждая только со своего источника.
/// Метка роли разбирается терпимо (см. ниже), но сама проверка цитаты не
/// смягчается ни в одном из путей.
fn quotes_confirmed(
    rationale: &str,
    roles: &[Option<crate::rubric_pack::InputRole>],
    scope: &EvidenceScope<'_>,
    min_similarity: f64,
) -> bool {
    let mut quoted = quoted_spans(rationale);
    roles.iter().all(|role| match role {
        None => evidence_confirmed(rationale, scope.whole(), min_similarity),
        Some(role) => {
            // Судья обязан пометить цитату ролью, но живые ответы метку
            // склеивают или переставляют («Цитата reference, subject: "…"»,
            // «Цитата subject, reference: "…" / "…"» — живой прогон
            // 2026-09-20). Роль закрывает ПЕРВАЯ цитата, которая
            // подтверждается её источником; использованная цитата выбывает.
            // Послабление только в разборе: каждая роль по-прежнему обязана
            // иметь СВОЮ цитату из СВОЕГО источника, и одна цитата не может
            // закрыть обе роли.
            let labelled = extract_role_quote(rationale, *role).filter(|q| {
                scope
                    .role_texts(*role)
                    .iter()
                    .any(|text| verify_quote(q, text, min_similarity))
            });
            if let Some(q) = labelled {
                // Помеченная цитата тоже выбывает: иначе одна и та же строка
                // закроет обе роли, если она встречается в обоих источниках.
                if let Some(n) = quoted.iter().position(|s| *s == q) {
                    quoted.remove(n);
                }
                return true;
            }
            let found = quoted.iter().position(|q| {
                scope
                    .role_texts(*role)
                    .iter()
                    .any(|text| verify_quote(q, text, min_similarity))
            });
            if let Some(n) = found {
                quoted.remove(n);
                true
            } else {
                false
            }
        }
    })
}

/// Перечень названного судьёй проверенным — объединение по сэмплам, в порядке
/// первого появления (для отчёта: что именно судья перечислил).
fn checked_ids(runs: &[JudgeResponse], criterion_id: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for run in runs {
        let Some(sample) = run.scores.iter().find(|s| s.criterion_id == criterion_id) else {
            continue;
        };
        for id in &sample.checked {
            let id = id.trim();
            if !id.is_empty() && !out.iter().any(|seen| seen.eq_ignore_ascii_case(id)) {
                out.push(id.to_string());
            }
        }
    }
    out
}

/// Все дословные цитаты обоснования в порядке появления.
///
/// Нужны как запасной путь, когда судья склеил роли в одну метку: цитаты всё
/// равно сверяются со своими источниками, поэтому терпимость разбора не
/// ослабляет правило двух цитат.
fn quoted_spans(rationale: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cursor = 0usize;
    while let Some(rel) = rationale
        .get(cursor..)
        .and_then(|t| t.find(['«', '"', '\'']))
    {
        let open = cursor + rel;
        let rest = &rationale[open..];
        let close_ch = rest.chars().next().unwrap_or('"');
        let close = if close_ch == '«' { '»' } else { close_ch };
        let after = &rest[close_ch.len_utf8()..];
        let Some(end) = after.find(close) else { break };
        let span = after[..end].trim();
        if span.chars().count() >= MIN_QUOTE_CHARS {
            out.push(span.to_string());
        }
        cursor = open + close_ch.len_utf8() + end + close.len_utf8();
    }
    out
}

/// Цитата, помеченная ролью: `Цитата subject: "…"` (регистр не важен).
fn extract_role_quote(rationale: &str, role: crate::rubric_pack::InputRole) -> Option<String> {
    let needle = format!("цитата {}", role.as_str());
    let after = find_case_insensitive_end(rationale, &needle)?;
    find_quoted_span(&rationale[after..]).filter(|q| q.chars().count() >= MIN_QUOTE_CHARS)
}

/// Собирает отчёт по k сэмплам судьи (ADR-004): итоговый балл критерия —
/// округлённая медиана сэмплов (пропуск судьёй в сэмпле = 1); σ выше порога —
/// метка `unstable`; балл ≥ 2 без подтверждённой цитаты — `evidence_not_found`
/// и исключение из взвешенного итога. Финальный этап — потолок вердикта:
/// есть `evidence_not_found` → вердикт отчёта не выше CONCERNS
/// ([`cap_verdict_at_concerns`]).
///
/// # Errors
/// Ни один критерий не засчитан (все без подтверждённых свидетельств) или
/// сумма весов засчитанных не положительна.
///
/// `pub(crate)` — split-judge `mcp_server::rubric_verify` собирает отчёт из
/// ответов хоста тем же кодом.
pub(crate) fn build_report(
    rubric: &Rubric,
    judge_model: &str,
    runs: &[JudgeResponse],
    scope: &EvidenceScope<'_>,
    cfg: &JudgeConfig,
) -> Result<RubricReport> {
    let mut scores = Vec::with_capacity(rubric.criteria.len());
    let mut unconfirmed_samples = 0usize;
    let mut counted_samples = 0usize;
    for c in &rubric.criteria {
        let mut samples: Vec<u8> = Vec::with_capacity(runs.len());
        // Д10: цитата сверяется в КАЖДОМ сэмпле, а не только у обоснования,
        // выбранного под медиану. Сэмпл, чья цитата не подтвердилась, в медиану
        // критерия не входит: выдуманное свидетельство не голосует за балл.
        // Балл 1 — «свидетельства нет», цитировать нечего (контракт промпта) —
        // остаётся, как раньше.
        let mut kept: Vec<u8> = Vec::with_capacity(runs.len());
        let mut fabricated = 0usize;
        for run in runs {
            let sample = run.scores.iter().find(|s| s.criterion_id == c.id);
            let value = sample.map_or(1, |s| clamp_score(s.score, rubric.scale_max));
            samples.push(value);
            let confirmed = value < 2
                || sample.is_some_and(|s| {
                    evidence_confirmed(&s.rationale, scope.whole(), cfg.evidence_min_similarity)
                });
            if confirmed {
                kept.push(value);
            } else {
                fabricated += 1;
            }
        }
        let samples_count = samples.len();
        counted_samples += samples_count;
        unconfirmed_samples += fabricated;
        // Подтверждённых сэмплов меньше половины — свидетельств у критерия
        // нет: критерий исключается из взвешенного итога (поведение 0.3.4).
        // Иначе балл считается по подтверждённым.
        let evidence_missing = kept.len() * 2 < samples_count;
        // Медиана значений из 1..=scale_max после округления остаётся в
        // диапазоне — приведение к u8 безопасно. При `evidence_missing` балл
        // считается по всем сэмплам: отчёт обязан показать, что судья ставил.
        let median_score = if evidence_missing {
            median(&samples).round() as u8
        } else {
            debug_assert!(!kept.is_empty(), "иначе evidence_missing был бы истинным");
            median(&kept).round() as u8
        };
        let stdev = stdev(&samples);
        let mut flags = Vec::new();
        if stdev > cfg.unstable_stdev {
            flags.push(CriterionFlag::Unstable);
        }
        let roles = c.evidence_role_list()?;
        let rationale = pick_rationale(
            runs,
            &c.id,
            rubric.scale_max,
            median_score,
            scope.whole(),
            cfg,
        );
        // Д10: сэмплы с неподтверждённой цитатой не голосовали за балл; если
        // подтверждённых меньше половины, свидетельств у критерия нет вовсе.
        if evidence_missing {
            flags.push(CriterionFlag::EvidenceNotFound);
        } else if fabricated > 0 {
            // Балл засчитан по подтверждённым сэмплам, но часть свидетельств
            // не подтвердилась — читателю это нужно знать (Д10).
            flags.push(CriterionFlag::EvidencePartial);
        }
        // Балл ≥ 2 требует подтверждённой цитаты; 1 — это «свидетельство
        // отсутствует», цитировать нечего (контракт промпта). В смысловой
        // рубрике (ADR-051) обвинение — низкий балл, и цитата нужна там:
        // направление доказательства задаёт критерий (`evidence_on`).
        if c.evidence_on.requires_high()
            && median_score >= 2
            && !flags.contains(&CriterionFlag::EvidenceNotFound)
            && !quotes_confirmed(&rationale, &roles, scope, cfg.evidence_min_similarity)
        {
            flags.push(CriterionFlag::EvidenceNotFound);
        }
        if c.evidence_on.requires_low()
            && median_score <= 2
            && !quotes_confirmed(&rationale, &roles, scope, cfg.evidence_min_similarity)
        {
            flags.push(CriterionFlag::AccusationUnconfirmed);
        }
        // Покрытие вместо цитаты (ADR-051, S3): «противоречий нет» цитатой не
        // доказать, поэтому при высоком балле судья называет проверенное, а
        // механика сверяет перечень с составом досье.
        if c.coverage.is_some() {
            let ids = scope.reference_ids().ok_or_else(|| {
                HarnessError::Rubric(format!(
                    "coverage_without_dossier: критерий '{}' требует перечня проверенных \
                     ссылочных источников, а оценка идёт по документу без досье — \
                     вызывайте с pack/subject (ADR-051)",
                    c.id
                ))
            })?;
            if !ids.is_empty() && coverage_incomplete(runs, &c.id, &ids, rubric.scale_max) {
                flags.push(CriterionFlag::CoverageIncomplete);
            }
        }
        // Доля сэмплов с неподтверждённой цитатой (Д10): считается по всем
        // критериям — выдуманное свидетельство не должно голосовать за балл
        // ни в смысловой рубрике, ни в обычной.
        let evidence_unconfirmed_ratio = if samples_count == 0 {
            0.0
        } else {
            fabricated as f64 / samples_count as f64
        };
        let checked = checked_ids(runs, &c.id);
        scores.push(CriterionScore {
            criterion_id: c.id.clone(),
            weight: c.weight,
            score: median_score,
            rationale,
            samples,
            stdev,
            flags,
            evidence_unconfirmed_ratio,
            checked,
        });
    }
    let weighted_total = weighted_total(&rubric.criteria, &scores)?;
    // Потолок вердикта держат оба вида неподтверждённого свидетельства:
    // и выдуманная похвала, и выдуманное обвинение — это оценка, которой
    // механике нечем подтвердить.
    let unconfirmed = scores
        .iter()
        .filter(|s| s.flags.iter().any(|f| f.excludes_from_total()))
        .count();
    let judge_verdict = runs.last().map_or_else(String::new, |r| r.verdict.clone());
    Ok(RubricReport {
        rubric_name: rubric.name.clone(),
        judge_model: judge_model.to_string(),
        judge_samples: runs.len(),
        scores,
        weighted_total,
        verdict: cap_verdict_at_concerns(judge_verdict, unconfirmed),
        evidence_unconfirmed_ratio: if counted_samples == 0 {
            0.0
        } else {
            unconfirmed_samples as f64 / counted_samples as f64
        },
    })
}

/// Механический потолок вердикта отчёта (находка живого прогона 0.3.0 на
/// кейсе цифрового рубля): хотя бы один критерий помечен
/// `evidence_not_found` → итоговый вердикт НЕ ВЫШЕ CONCERNS, независимо от
/// вердикта судьи (судья может прислать PASS с выдуманной цитатой — метка
/// одного критерия не должна тонуть в отчёте). Штраф к итогу (исключение
/// критерия из взвешенного) не меняется — только потолок вердикта.
/// `unconfirmed` — число критериев с `evidence_not_found`.
fn cap_verdict_at_concerns(judge_verdict: String, unconfirmed: usize) -> String {
    if unconfirmed == 0 {
        return judge_verdict;
    }
    let trimmed = judge_verdict.trim();
    if trimmed.eq_ignore_ascii_case("pass") {
        return format!(
            "CONCERNS — вердикт судьи PASS понижен до CONCERNS: неподтверждённые цитаты ({unconfirmed})"
        );
    }
    if trimmed.is_empty() {
        return format!("CONCERNS (механический потолок: неподтверждённые цитаты — {unconfirmed})");
    }
    // Свободный вердикт судьи сохраняется после маркера потолка (FAIL и
    // прочие не поднимаются — потолок только прижимает к CONCERNS сверху).
    format!("CONCERNS (потолок: неподтверждённые цитаты — {unconfirmed}): {trimmed}")
}

/// Медиана баллов сэмплов; для чётного k — среднее двух центральных.
fn median(samples: &[u8]) -> f64 {
    debug_assert!(!samples.is_empty(), "число сэмплов клэмпится в ≥ 1");
    let mut sorted = samples.to_vec();
    sorted.sort_unstable();
    let mid = sorted.len() / 2;
    if sorted.len() % 2 == 1 {
        f64::from(sorted[mid])
    } else {
        f64::midpoint(f64::from(sorted[mid - 1]), f64::from(sorted[mid]))
    }
}

/// Population-σ баллов сэмплов (деление на n: консервативнее sample-σ при
/// малом k — флаг `unstable` реже ложный, ADR-004); один сэмпл → 0.
fn stdev(samples: &[u8]) -> f64 {
    if samples.len() < 2 {
        return 0.0;
    }
    let mean = samples.iter().map(|s| f64::from(*s)).sum::<f64>() / samples.len() as f64;
    let var = samples
        .iter()
        .map(|s| (f64::from(*s) - mean).powi(2))
        .sum::<f64>()
        / samples.len() as f64;
    var.sqrt()
}

/// Обоснование для отчёта: из первого сэмпла, чей (клэмпнутый) балл совпал с
/// итоговым медианным И цитата подтверждена; иначе — первое подтверждённое,
/// иначе первое непустое (тогда критерий уже помечен `evidence_not_found` или
/// `evidence_partial`); судья ни разу не оценил — явная пометка.
///
/// Предпочтение подтверждённой цитаты — часть Д10: показывать в отчёте
/// выдуманное свидетельство, когда рядом есть подтверждённое, значит вводить
/// читателя в заблуждение ровно тем, что проверка и ловит.
fn pick_rationale(
    runs: &[JudgeResponse],
    criterion_id: &str,
    scale_max: u8,
    final_score: u8,
    target: &str,
    cfg: &JudgeConfig,
) -> String {
    let mut first_confirmed: Option<&str> = None;
    let mut first_non_empty: Option<&str> = None;
    for run in runs {
        let Some(s) = run.scores.iter().find(|s| s.criterion_id == criterion_id) else {
            continue;
        };
        if s.rationale.trim().is_empty() {
            continue;
        }
        if first_non_empty.is_none() {
            first_non_empty = Some(s.rationale.as_str());
        }
        let score = clamp_score(s.score, scale_max);
        let confirmed =
            score < 2 || evidence_confirmed(&s.rationale, target, cfg.evidence_min_similarity);
        if !confirmed {
            continue;
        }
        if first_confirmed.is_none() {
            first_confirmed = Some(s.rationale.as_str());
        }
        if score == final_score {
            return s.rationale.clone();
        }
    }
    first_confirmed
        .or(first_non_empty)
        .map_or_else(|| "судья не оценил".to_string(), str::to_string)
}

/// Цитата из rationale подтверждена оцениваемым текстом: цитата извлекается
/// и находится в тексте (точный substring либо fuzzy-матч по порогу).
fn evidence_confirmed(rationale: &str, target: &str, min_similarity: f64) -> bool {
    extract_quote(rationale).is_some_and(|q| verify_quote(&q, target, min_similarity))
}

/// Извлекает цитату-свидетельство из rationale: первый quoted-span
/// («…», "…", '…') длиной ≥ [`MIN_QUOTE_CHARS`] после маркера «цитата»
/// (регистр неважен); без маркера — первый такой span во всём rationale.
fn extract_quote(rationale: &str) -> Option<String> {
    let after_marker = find_case_insensitive_end(rationale, "цитата");
    after_marker
        .and_then(|i| find_quoted_span(&rationale[i..]))
        .or_else(|| find_quoted_span(rationale))
        .filter(|q| q.chars().count() >= MIN_QUOTE_CHARS)
}

/// Байтовая позиция КОНЦА первого вхождения `needle` в `haystack` без учёта
/// регистра. Работает посимвольно — безопасна для любого (модельного) ввода.
fn find_case_insensitive_end(haystack: &str, needle: &str) -> Option<usize> {
    let needle_chars: Vec<char> = needle.chars().flat_map(char::to_lowercase).collect();
    let n = needle_chars.len();
    let h: Vec<(usize, char)> = haystack.char_indices().collect();
    if h.len() < n {
        return None;
    }
    h.windows(n).find_map(|w| {
        let matches = w
            .iter()
            .flat_map(|&(_, c)| c.to_lowercase())
            .eq(needle_chars.iter().copied());
        let (end_i, end_c) = w[n - 1];
        matches.then(|| end_i + end_c.len_utf8())
    })
}

/// Первый quoted-span («…», "…", '…') с непустым содержимым.
fn find_quoted_span(text: &str) -> Option<String> {
    let pairs = [('«', '»'), ('"', '"'), ('\'', '\'')];
    let mut best: Option<(usize, usize)> = None; // (начало, конец) содержимого
    for (open, close) in pairs {
        let mut rest = text;
        let mut offset = 0usize;
        while let Some(i) = rest.find(open) {
            let after = &rest[i + open.len_utf8()..];
            let Some(j) = after.find(close) else { break };
            if !after[..j].trim().is_empty() {
                let abs = offset + i + open.len_utf8();
                if best.is_none_or(|(bs, _)| abs < bs) {
                    best = Some((abs, abs + j));
                }
                break; // первый непустой span этой пары кавычек — достаточно
            }
            // Пустой span ("") — пропускаем и ищем следующий.
            let step = i + open.len_utf8() + j + close.len_utf8();
            offset += step;
            rest = &rest[step..];
        }
    }
    best.map(|(s, e)| text[s..e].trim().to_string())
}

/// Цитата подтверждена: точный substring после нормализации, иначе fuzzy
/// (лучшее скользящее окно по словам) с порогом `min_similarity`.
fn verify_quote(quote: &str, target: &str, min_similarity: f64) -> bool {
    let q = normalize_for_match(&unescape_quote_ws(quote));
    let t = normalize_for_match(target);
    if q.is_empty() || t.is_empty() {
        return false;
    }
    if t.contains(&q) {
        return true;
    }
    quote_similarity(&q, &t) >= min_similarity.clamp(0.0, 1.0)
}

/// Нормализация для сопоставления цитат: нижний регистр + схлопывание
/// пробельных последовательностей в один пробел.
fn normalize_for_match(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// Приводит литеральные escape-последовательности (`\n`, `\r`, `\t` — два
/// символа) в цитате судьи к обычному пробелу. Судья нередко передаёт
/// перевод строки документа в escape-форме (`"- Date: …\n- Status: …"`),
/// и без этого шага кросс-строчная цитата не находится даже после
/// нормализации пробелов — ложный `evidence_not_found` на реальных строках.
/// Применяется только к цитате: в документе (target) `\n` может быть
/// содержимым (примеры кода), там подмена создала бы ложные подтверждения.
fn unescape_quote_ws(quote: &str) -> String {
    quote
        .replace("\\n", " ")
        .replace("\\r", " ")
        .replace("\\t", " ")
}

/// Максимум `similar::TextDiff::ratio` по скользящему окну слов размером в
/// цитату: ratio окна той же длины — доля совпавших по порядку слов, цитата
/// с парой искажённых слов остаётся выше порога 0.8.
fn quote_similarity(quote: &str, target: &str) -> f64 {
    let q_words = quote.split(' ').count();
    let t_words: Vec<&str> = target.split(' ').collect();
    if q_words == 0 || t_words.is_empty() {
        return 0.0;
    }
    if t_words.len() <= q_words {
        return f64::from(similar::TextDiff::from_words(target, quote).ratio());
    }
    let mut best = 0.0_f64;
    for window in t_words.windows(q_words) {
        let candidate = window.join(" ");
        let ratio = f64::from(similar::TextDiff::from_words(candidate.as_str(), quote).ratio());
        if ratio > best {
            best = ratio;
            if best >= 1.0 {
                break;
            }
        }
    }
    best
}

/// Взвешенный итог: Σ(score·weight)/Σweight по засчитанным критериям;
/// критерии с меткой `evidence_not_found` исключаются (ADR-004).
///
/// Публична: golden-прогон (`bench::run_golden`) считает ей взвешенный
/// эталонный балл для диагностики length bias.
///
/// # Errors
/// Сумма весов засчитанных критериев не положительна (все отклонены или
/// веса рубрики нулевые).
pub fn weighted_total(criteria: &[Criterion], scores: &[CriterionScore]) -> Result<f64> {
    let mut sum = 0.0;
    let mut weights = 0.0;
    for c in criteria {
        let score = scores.iter().find(|s| s.criterion_id == c.id);
        // Свидетельство не подтверждено (evidence_not_found) или обвинение не
        // подтверждено цитатами по ролям (accusation_unconfirmed) — балл не
        // засчитывается.
        if score.is_some_and(|s| s.flags.iter().any(|f| f.excludes_from_total())) {
            continue;
        }
        sum += f64::from(score.map_or(1, |s| s.score)) * c.weight;
        weights += c.weight;
    }
    if weights <= 0.0 {
        return Err(HarnessError::Rubric(
            "нет засчитанных критериев: все оценки без подтверждённых свидетельств \
             (evidence_not_found, accusation_unconfirmed) или сумма весов рубрики \
             не положительна"
                .into(),
        ));
    }
    Ok(sum / weights)
}

/// Клэмпит сырой балл судьи в `1..=scale_max`.
fn clamp_score(raw: f64, scale_max: u8) -> u8 {
    let max = f64::from(scale_max.max(1));
    // После clamp+round значение гарантированно в 1..=scale_max, усечения не будет.
    raw.clamp(1.0, max).round() as u8
}

/// Первые [`ERR_FRAGMENT_CHARS`] символов текста для сообщений об ошибках.
pub(super) fn fragment(text: &str) -> String {
    let mut s: String = text.chars().take(ERR_FRAGMENT_CHARS).collect();
    if text.chars().count() > ERR_FRAGMENT_CHARS {
        s.push('…');
    }
    s
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};

    use super::*;
    use crate::rubric::testkit::*;
    use crate::rubric::types::{Coverage, EvidenceOn};

    #[test]
    fn weighted_total_math() {
        let rubric = sample_rubric();
        let scores = vec![
            plain_score("context", 1.0, 4),
            plain_score("alternatives", 3.0, 2),
        ];
        // (4*1 + 2*3) / (1+3) = 2.5
        let total = weighted_total(&rubric.criteria, &scores).expect("total");
        assert!((total - 2.5).abs() < 1e-9, "ожидали 2.5, получили {total}");
    }

    #[test]
    fn weighted_total_rejects_zero_weights() {
        let mut rubric = sample_rubric();
        for c in &mut rubric.criteria {
            c.weight = 0.0;
        }
        assert!(weighted_total(&rubric.criteria, &[]).is_err());
    }

    #[test]
    fn weighted_total_skips_evidence_not_found() {
        let rubric = sample_rubric();
        let mut rejected = plain_score("context", 1.0, 5);
        rejected.flags.push(CriterionFlag::EvidenceNotFound);
        let scores = vec![rejected, plain_score("alternatives", 3.0, 2)];
        // context отклонён: итог — только alternatives: 2*3/3 = 2.0
        let total = weighted_total(&rubric.criteria, &scores).expect("total");
        assert!((total - 2.0).abs() < 1e-9, "ожидали 2.0, получили {total}");

        // Все критерии отклонены — честная ошибка, а не нулевой итог.
        let all_rejected: Vec<CriterionScore> = scores
            .into_iter()
            .map(|mut s| {
                s.flags.push(CriterionFlag::EvidenceNotFound);
                s
            })
            .collect();
        let err = weighted_total(&rubric.criteria, &all_rejected).expect_err("все отклонены");
        assert!(err.to_string().contains("evidence_not_found"), "{err}");
    }

    #[test]
    fn median_and_stdev_math() {
        assert_eq!(median(&[3]), 3.0);
        assert_eq!(median(&[2, 4, 5]), 4.0);
        assert_eq!(median(&[2, 5]), 3.5, "чётное k — среднее центральных");
        assert_eq!(median(&[1, 1, 5]), 1.0, "медиана устойчива к выбросу");

        assert_eq!(stdev(&[4]), 0.0, "один сэмпл — без разброса");
        assert_eq!(stdev(&[3, 3, 3]), 0.0);
        // population-σ [2,4,5]: mean 11/3, var (2.789+0.111+1.778)/3 ≈ 1.556
        let sd = stdev(&[2, 4, 5]);
        assert!((sd - 1.247).abs() < 0.01, "ожидали ≈1.247, получили {sd}");
    }

    #[test]
    fn extracts_json_from_fence_and_preamble() {
        let fenced = "Вот оценка:\n```json\n{\"scores\": [], \"verdict\": \"ok\"}\n```\nГотово.";
        let json = extract_json_object(fenced).expect("json");
        assert!(json.starts_with('{') && json.ends_with('}'));
        assert!(json.contains("\"verdict\""));

        let bare = "преамбула {\"a\": 1} послесловие";
        assert_eq!(extract_json_object(bare), Some("{\"a\": 1}"));

        assert!(extract_json_object("нет json вообще").is_none());
    }

    #[test]
    fn clamp_score_bounds() {
        assert_eq!(clamp_score(0.0, 5), 1);
        assert_eq!(clamp_score(99.0, 5), 5);
        assert_eq!(clamp_score(3.0, 5), 3);
        assert_eq!(clamp_score(4.0, 0), 1, "scale_max=0 не должен паниковать");
    }

    #[test]
    fn quote_extraction_variants() {
        // Маркер + ёлочки.
        assert_eq!(
            extract_quote("Цитата: «вендор уходит с рынка» — сила названа").as_deref(),
            Some("вендор уходит с рынка")
        );
        // Маркер без учёта регистра + прямые кавычки.
        assert_eq!(
            extract_quote("цитата: \"миграция платёжного шлюза\" — ok").as_deref(),
            Some("миграция платёжного шлюза")
        );
        // Без маркера — первый длинный span в кавычках.
        assert_eq!(
            extract_quote("обоснование с опорой на «честные причины отказа»").as_deref(),
            Some("честные причины отказа")
        );
        // Короткий span — не свидетельство.
        assert_eq!(extract_quote("Цитата: \"да\""), None);
        // Кавычек нет вовсе.
        assert_eq!(extract_quote("контекст описан хорошо"), None);
    }

    #[test]
    fn quote_verification_substring_and_fuzzy() {
        let target = "Контекст: миграция платёжного шлюза завершится в мае. Риски: двойная запись.";
        // Точное вхождение (с нормализацией регистра/пробелов).
        assert!(verify_quote("Миграция платёжного   шлюза", target, 0.8));
        // Одно искажённое слово — fuzzy выше порога 0.8.
        assert!(verify_quote(
            "миграция платёжного шлюза завершится в июне",
            target,
            0.8
        ));
        // Выдуманная цитата не подтверждается.
        assert!(!verify_quote(
            "этой фразы нет в документе вообще",
            target,
            0.8
        ));
        // Пустые входы не паникуют и не подтверждаются.
        assert!(!verify_quote("", target, 0.8));
        assert!(!verify_quote("что-то длинное", "", 0.8));
    }

    #[test]
    fn cross_line_quote_confirmed() {
        // Шапка ADR из отчёта полигона (E5): цитата судьи пересекает
        // перевод строки документа, строки реальны.
        let target = "# ADR-001. Решение\n\n- Date: 2026-09-19\n- Status: Accepted\n\n## Context\n\nТекст.\n";
        // Реальный перевод строки в цитате — подтверждается.
        assert!(verify_quote(
            "- Date: 2026-09-19\n- Status: Accepted",
            target,
            0.8
        ));
        // Литеральная escape-форма (backslash + n двумя символами) — тоже.
        assert!(verify_quote(
            "- Date: 2026-09-19\\n- Status: Accepted",
            target,
            0.8
        ));
        // Полный путь через rationale: маркер + кавычки + escape-форма.
        let rationale =
            "Балл 4. Цитата: \"- Date: 2026-09-19\\n- Status: Accepted\" — шапка на месте.";
        assert!(evidence_confirmed(rationale, target, 0.8));
        // Выдуманная кросс-строчная цитата (правдоподобная структура,
        // выдуманное содержимое) — метка остаётся.
        assert!(!verify_quote(
            "- Owner: команда-платформа\\n- Review: ежеквартально",
            target,
            0.8
        ));
        assert!(!evidence_confirmed(
            "Цитата: \"- Owner: команда-платформа\\n- Review: ежеквартально\".",
            target,
            0.8
        ));
        // Escape-подмена не создаёт ложных подтверждений из кода в target:
        // литеральный `\n` в документе остаётся содержимым, а не пробелом.
        let code_target = "пример: print(\"a\\nb\") в коде";
        assert!(!verify_quote("a b", code_target, 0.8));
    }

    /// Сэмпл судьи с заданным баллом по `context` и цитатой; `alternatives`
    /// всегда подтверждён.
    fn judge_run(context: u8, rationale: &str) -> JudgeResponse {
        JudgeResponse {
            scores: vec![
                JudgeScore {
                    criterion_id: "context".into(),
                    score: f64::from(context),
                    rationale: rationale.into(),
                    checked: Vec::new(),
                },
                JudgeScore {
                    criterion_id: "alternatives".into(),
                    score: 3.0,
                    rationale: "Цитата: \"альтернативы перечислены\" — частично".into(),
                    checked: Vec::new(),
                },
            ],
            verdict: "ok".into(),
        }
    }

    /// Д10: выдуманная цитата в ОДНОМ сэмпле из трёх не проходит незамеченной.
    /// До 0.3.5 цитата сверялась только у обоснования, выбранного под медиану:
    /// сэмпл с баллом 5 и выдуманным свидетельством не влиял ни на балл, ни на
    /// метку — `[3, 5, 3]` давало 3 без флага.
    #[test]
    fn single_fabricated_quote_is_flagged_and_excluded() {
        let runs = vec![
            judge_run(3, "Цитата: \"контекст описан кратко\" — средне"),
            judge_run(
                5,
                "Цитата: \"этой фразы в документе нет вообще\" — якобы образцово",
            ),
            judge_run(3, "Цитата: \"контекст описан кратко\" — средне"),
        ];
        let report = build_report(
            &sample_rubric(),
            "fake",
            &runs,
            &EvidenceScope::Target("контекст описан кратко; альтернативы перечислены"),
            &three_samples(),
        )
        .expect("отчёт");
        let context = &report.scores[0];
        assert_eq!(context.samples, vec![3, 5, 3], "сырые баллы сохранены");
        assert_eq!(context.score, 3, "медиана считается по подтверждённым");
        assert!(
            context.has_flag(CriterionFlag::EvidencePartial),
            "неподтверждённый сэмпл помечен: {:?}",
            context.flags
        );
        assert!(
            !context.has_flag(CriterionFlag::EvidenceNotFound),
            "подтверждённых большинство — критерий засчитан"
        );
        assert!(
            (context.evidence_unconfirmed_ratio - 1.0 / 3.0).abs() < 1e-9,
            "доля неподтверждённых: {}",
            context.evidence_unconfirmed_ratio
        );
        assert!(
            context.rationale.contains("средне"),
            "в отчёт идёт подтверждённое обоснование: {}",
            context.rationale
        );
        assert!(report.evidence_unconfirmed_ratio > 0.0);
        // Критерий засчитан — вердикт механическим потолком не прижат.
        assert_eq!(report.verdict, "ok", "{}", report.verdict);
    }

    /// Д10: выдумка в большинстве сэмплов — свидетельств у критерия нет.
    /// Критерий исключается из взвешенного итога, как и раньше при одной
    /// выдуманной цитате, а в балле отчёта видно, что ставил судья.
    #[test]
    fn majority_fabricated_is_evidence_not_found() {
        let runs = vec![
            judge_run(3, "Цитата: \"контекст описан кратко\" — средне"),
            judge_run(5, "Цитата: \"выдуманная фраза раз\" — якобы образцово"),
            judge_run(5, "Цитата: \"выдуманная фраза два\" — якобы образцово"),
        ];
        let report = build_report(
            &sample_rubric(),
            "fake",
            &runs,
            &EvidenceScope::Target("контекст описан кратко; альтернативы перечислены"),
            &three_samples(),
        )
        .expect("отчёт");
        let context = &report.scores[0];
        assert!(context.has_flag(CriterionFlag::EvidenceNotFound));
        assert!(!context.has_flag(CriterionFlag::EvidencePartial));
        assert_eq!(
            context.score, 5,
            "балл в отчёте — то, что ставил судья (медиана всех сэмплов)"
        );
        // Итог — только alternatives: 3*3/3 = 3.0 (context исключён).
        assert!((report.weighted_total - 3.0).abs() < 1e-9);
    }

    /// Д10 (обратная сторона): эталонный набор даёт прежние баллы, когда
    /// цитаты настоящие. Проверка каждого сэмпла не должна сдвигать оценки —
    /// иначе «усиление» превратилось бы в переоценку всех прошлых отчётов.
    ///
    /// Прогон офлайновый: вместо вызова судьи берутся объявленные эталоном
    /// баллы, а обоснования цитируют реальные фрагменты документа.
    #[test]
    fn golden_scores_unchanged() {
        #[derive(serde::Deserialize)]
        struct Expected {
            #[serde(default)]
            rubric: String,
            scores: BTreeMap<String, u8>,
        }
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/benchmarks/golden");
        let rubric: Rubric = serde_yaml_ng::from_str(crate::assets::RUBRIC_ADR_QUALITY)
            .expect("рубрика adr_quality из ассетов");
        let mut checked = 0usize;
        let mut entries: Vec<PathBuf> = std::fs::read_dir(&dir)
            .expect("каталог golden")
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|e| e == "yaml"))
            .collect();
        entries.sort();
        for expected_path in entries {
            let stem = expected_path
                .file_stem()
                .and_then(|s| s.to_str())
                .expect("имя")
                .trim_end_matches(".expected")
                .to_string();
            let doc_path = dir.join(format!("{stem}.md"));
            if !doc_path.is_file() {
                continue;
            }
            let expected: Expected = serde_yaml_ng::from_str(
                &std::fs::read_to_string(&expected_path).expect("эталон читается"),
            )
            .expect("эталон разбирается");
            assert_eq!(
                expected.rubric, rubric.name,
                "{stem}: эталон снят с другой рубрики"
            );
            let target = std::fs::read_to_string(&doc_path).expect("документ читается");
            // Цитата — реальный фрагмент документа (первая достаточно длинная
            // строка): свидетельство подтверждается точным вхождением.
            let fragment = target
                .lines()
                .map(str::trim)
                .find(|l| l.chars().count() >= MIN_QUOTE_CHARS)
                .map(|l| l.chars().take(60).collect::<String>())
                .expect("в документе есть строка для цитаты");
            let runs: Vec<JudgeResponse> = (0..3)
                .map(|_| JudgeResponse {
                    scores: expected
                        .scores
                        .iter()
                        .map(|(id, score)| JudgeScore {
                            criterion_id: id.clone(),
                            score: f64::from(*score),
                            rationale: format!("Цитата: \"{fragment}\" — по тексту"),
                            checked: Vec::new(),
                        })
                        .collect(),
                    verdict: "ok".into(),
                })
                .collect();
            let report = build_report(
                &rubric,
                "fake",
                &runs,
                &EvidenceScope::Target(&target),
                &three_samples(),
            )
            .expect("отчёт");
            for (id, want) in &expected.scores {
                let got = report
                    .scores
                    .iter()
                    .find(|s| s.criterion_id == *id)
                    .unwrap_or_else(|| panic!("{stem}: критерий {id} в отчёте"));
                assert_eq!(got.score, *want, "{stem}/{id}: эталонный балл не сдвинулся");
                assert!(
                    got.flags.is_empty(),
                    "{stem}/{id}: подтверждённые цитаты не дают меток: {:?}",
                    got.flags
                );
            }
            assert_eq!(
                report.evidence_unconfirmed_ratio, 0.0,
                "{stem}: все цитаты подтверждены"
            );
            checked += 1;
        }
        assert!(checked >= 8, "эталонных документов проверено: {checked}");
    }

    #[test]
    fn markdown_contains_table_total_verdict_judge() {
        let report = RubricReport {
            rubric_name: "adr-quality".into(),
            judge_model: "fake-judge-1".into(),
            judge_samples: 3,
            scores: vec![CriterionScore {
                criterion_id: "context".into(),
                weight: 1.0,
                score: 4,
                rationale: "по тексту".into(),
                samples: vec![4, 4, 4],
                stdev: 0.0,
                flags: Vec::new(),
                evidence_unconfirmed_ratio: 0.0,
                checked: Vec::new(),
            }],
            weighted_total: 4.0,
            verdict: "годно".into(),
            evidence_unconfirmed_ratio: 0.0,
        };
        let md = report.to_markdown();
        assert!(md.contains("# Оценка по рубрике «adr-quality»"));
        assert!(md.contains("| Критерий | Вес | Балл | Метки | Обоснование |"));
        assert!(md.contains("| context | 1.00 | 4 |  | по тексту |"));
        assert!(md.contains("**Взвешенный итог:** 4.00/5"));
        assert!(md.contains("**Вердикт:** годно"));
        assert!(md.contains("**Судья:** fake-judge-1 (сэмплов на критерий: 3)"));
        assert!(md.contains("**Дата:**"));
    }

    // --- S2 (ADR-051): направление доказательства и роли источников ---------

    /// Обвинение (низкий балл) без цитат: критерий исключён из итога и помечен
    /// `accusation_unconfirmed`, но блокирующей находкой не становится (S2).
    #[test]
    fn low_score_without_quotes_is_unconfirmed() {
        let rubric = rubric_of(vec![
            criterion("no_contradiction", 3.0, EvidenceOn::Low, &[]),
            criterion("context", 1.0, EvidenceOn::High, &[]),
        ]);
        let cfg = one_sample();
        let scope = EvidenceScope::Target("текст документа без противоречий");
        // Обвинение без цитаты — выдуманное свидетельство; context оценён
        // подтверждённой цитатой.
        let both = parse_judge_response(
            r#"{"scores": [
                 {"criterion_id": "no_contradiction", "score": 1, "rationale": "найдено противоречие"},
                 {"criterion_id": "context", "score": 5, "rationale": "Цитата: \"текст документа без противоречий\". контекст полон"}
               ], "verdict": "v"}"#,
        )
        .expect("ответ судьи");
        let report = build_report(&rubric, "judge-x", &[both], &scope, &cfg).expect("отчёт");
        let acc = report
            .scores
            .iter()
            .find(|s| s.criterion_id == "no_contradiction")
            .expect("критерий");
        assert!(
            acc.has_flag(CriterionFlag::AccusationUnconfirmed),
            "флаг обвинения: {:?}",
            acc.flags
        );
        assert_eq!(
            report.weighted_total, 5.0,
            "исключённый критерий не тянет итог вниз: остался context"
        );
        assert!(
            report.verdict.contains("CONCERNS"),
            "вердикт под потолком: {}",
            report.verdict
        );

        // Цитата подтверждена — обвинение засчитано.
        let report = build_report(
            &rubric,
            "judge-x",
            &[run_of(
                "no_contradiction",
                1,
                "Цитата: \"текст документа без противоречий\". прямое противоречие",
            )],
            &scope,
            &cfg,
        )
        .expect("отчёт");
        let acc = report
            .scores
            .iter()
            .find(|s| s.criterion_id == "no_contradiction")
            .expect("критерий");
        assert!(
            !acc.has_flag(CriterionFlag::AccusationUnconfirmed),
            "подтверждённое обвинение засчитано: {:?}",
            acc.flags
        );
    }

    /// Цитата из чужого источника роль не закрывает: обвинение «ADR против
    /// спайна» не подтверждается половиной доказательства (S2).
    #[test]
    fn quote_from_wrong_role_is_rejected() {
        let rubric = rubric_of(vec![criterion(
            "no_contradiction",
            3.0,
            EvidenceOn::Low,
            &["subject", "reference"],
        )]);
        let cfg = one_sample();
        let pack = two_source_pack();
        let scope = EvidenceScope::Pack(&pack);

        // Обе цитаты со своих источников — обвинение подтверждено.
        let both = run_of(
            "no_contradiction",
            1,
            "Цитата subject: \"Решение: контроль слоя построен без LLM в гейте.\". \
             Цитата reference: \"Rule: механика контроля без LLM.\". противоречие",
        );
        let report = build_report(&rubric, "judge-x", &[both], &scope, &cfg).expect("отчёт");
        assert!(
            !report.scores[0].has_flag(CriterionFlag::AccusationUnconfirmed),
            "две цитаты по ролям: {:?}",
            report.scores[0].flags
        );

        // Живая форма ответа (прогон 2026-09-20): роли склеены в одну метку,
        // цитаты идут по порядку — каждая сверяется со своим источником.
        let merged = run_of(
            "no_contradiction",
            1,
            "Цитата subject, reference: \"Решение: контроль слоя построен без LLM в гейте.\" / \
             \"Rule: механика контроля без LLM.\". противоречие",
        );
        let report = build_report(&rubric, "judge-x", &[merged], &scope, &cfg).expect("отчёт");
        assert!(
            !report.scores[0].has_flag(CriterionFlag::AccusationUnconfirmed),
            "склеенная метка не ломает сверку: {:?} — {}",
            report.scores[0].flags,
            report.scores[0].rationale
        );

        // Живая форма ответа (прогон D11, 2026-09-20): склеенная метка ещё и
        // переставлена — первой идёт цитата СУБЪЕКТА. Роль закрывает первая
        // цитата, которую подтверждает её источник, а не та, что названа.
        let reversed = run_of(
            "no_contradiction",
            1,
            "Цитата reference, subject: \"Решение: контроль слоя построен без LLM в гейте.\" / \
             \"Rule: механика контроля без LLM.\". противоречие",
        );
        let report = build_report(&rubric, "judge-x", &[reversed], &scope, &cfg).expect("отчёт");
        assert!(
            !report.scores[0].has_flag(CriterionFlag::AccusationUnconfirmed),
            "переставленная метка не ломает сверку: {:?} — {}",
            report.scores[0].flags,
            report.scores[0].rationale
        );

        // Цитата спайна подставлена в роль субъекта — не засчитывается.
        let swapped = run_of(
            "no_contradiction",
            1,
            "Цитата subject: \"Rule: механика контроля без LLM.\". \
             Цитата reference: \"Rule: механика контроля без LLM.\". противоречие",
        );
        let report = build_report(&rubric, "judge-x", &[swapped], &scope, &cfg)
            .expect_err("единственный критерий исключён — итог не собрать");
        assert!(
            err_text(&report).contains("accusation_unconfirmed"),
            "цитата из чужого источника не закрывает роль: {report}"
        );
    }

    /// Одна цитата не закрывает обе роли, даже когда строка встречается в
    /// обоих источниках: иначе терпимость разбора превратилась бы в «одно
    /// доказательство на две роли» (S2, живой прогон 2026-09-20).
    #[test]
    fn one_quote_cannot_close_both_roles() {
        let rubric = rubric_of(vec![criterion(
            "no_contradiction",
            3.0,
            EvidenceOn::Low,
            &["subject", "reference"],
        )]);
        let cfg = one_sample();
        let mut pack = two_source_pack();
        // Общая строка в обоих источниках — искушение для судьи.
        let shared = "Решение: контроль слоя построен без LLM в гейте.";
        pack.text = pack
            .text
            .replace("Rule: механика контроля без LLM.", shared);
        pack.sha256 = crate::hash::sha256_hex(pack.text.as_bytes());
        let scope = EvidenceScope::Pack(&pack);
        let one = run_of(
            "no_contradiction",
            1,
            &format!("Цитата subject, reference: \"{shared}\". противоречие"),
        );
        let report = build_report(&rubric, "judge-x", &[one], &scope, &cfg)
            .expect_err("одной цитаты на две роли мало");
        assert!(
            err_text(&report).contains("accusation_unconfirmed"),
            "одна цитата не заменяет две роли: {report}"
        );
    }

    /// Цитаты сверяются в КАЖДОМ сэмпле: доля неподтверждённых видна в отчёте
    /// (S2 и Д10 — одно поле: механику писали два задания параллельно и имена
    /// свели намеренно). Цитата требуется там, где балл что-то утверждает:
    /// балл 1 — это «свидетельства нет», цитировать нечего.
    #[test]
    fn unconfirmed_ratio_counts_every_sample() {
        let rubric = rubric_of(vec![criterion(
            "no_contradiction",
            1.0,
            EvidenceOn::Low,
            &[],
        )]);
        let cfg = JudgeConfig {
            samples: 2,
            ..JudgeConfig::default()
        };
        let evidence = EvidenceScope::Target("в документе нет противоречий инварианту");
        let runs = [
            run_of(
                "no_contradiction",
                4,
                "Цитата: \"в документе нет противоречий инварианту\". всё чисто",
            ),
            run_of("no_contradiction", 4, "всё чисто, цитаты не привожу"),
        ];
        let report = build_report(&rubric, "judge-x", &runs, &evidence, &cfg).expect("отчёт");
        let score = &report.scores[0];
        assert_eq!(
            score.evidence_unconfirmed_ratio, 0.5,
            "один сэмпл из двух без подтверждённой цитаты: {score:?}"
        );
        assert!(
            score.has_flag(CriterionFlag::EvidencePartial),
            "подтверждённых сэмплов хватило на балл — метка о частичной \
             подтверждённости: {:?}",
            score.flags
        );
    }

    // --- S3 (ADR-051): покрытие вместо цитаты для «всё чисто» ---------------

    /// Критерий с требованием покрытия ссылочных источников.
    fn covering_criterion(id: &str, on: EvidenceOn) -> Criterion {
        let mut c = criterion(id, 3.0, on, &[]);
        c.coverage = Some(Coverage::ReferenceIds);
        c
    }

    /// Высокий балл с полным перечнем проверенного — критерий засчитан;
    /// пропуск хотя бы одного источника — флаг и исключение из итога (S3).
    #[test]
    fn high_score_with_partial_coverage_is_flagged() {
        let rubric = rubric_of(vec![
            covering_criterion("no_contradiction", EvidenceOn::Low),
            criterion("context", 1.0, EvidenceOn::High, &[]),
        ]);
        let cfg = one_sample();
        let pack = two_source_pack();
        let scope = EvidenceScope::Pack(&pack);

        // Полный перечень: единственный ссылочный источник досье — AD-2.
        let full = parse_judge_response(
            r#"{"scores": [
                 {"criterion_id": "no_contradiction", "score": 5,
                  "rationale": "противоречий нет",
                  "checked": ["AD-2"]},
                 {"criterion_id": "context", "score": 4,
                  "rationale": "Цитата: \"Решение: контроль слоя построен без LLM в гейте.\". ок"}
               ], "verdict": "v"}"#,
        )
        .expect("ответ судьи");
        let report = build_report(&rubric, "judge-x", &[full], &scope, &cfg).expect("отчёт");
        let main = &report.scores[0];
        assert!(
            !main.has_flag(CriterionFlag::CoverageIncomplete),
            "полный перечень: {:?}",
            main.flags
        );
        assert_eq!(main.checked, vec!["AD-2".to_string()], "перечень в отчёте");

        // Пропуск ссылочного источника — «5 не глядя».
        let partial = parse_judge_response(
            r#"{"scores": [
                 {"criterion_id": "no_contradiction", "score": 5,
                  "rationale": "противоречий нет", "checked": []},
                 {"criterion_id": "context", "score": 4,
                  "rationale": "Цитата: \"Решение: контроль слоя построен без LLM в гейте.\". ок"}
               ], "verdict": "v"}"#,
        )
        .expect("ответ судьи");
        let report = build_report(&rubric, "judge-x", &[partial], &scope, &cfg).expect("отчёт");
        let main = &report.scores[0];
        assert!(
            main.has_flag(CriterionFlag::CoverageIncomplete),
            "неполное покрытие: {:?}",
            main.flags
        );
        assert!(
            report.to_markdown().contains("coverage_incomplete"),
            "метка видна в отчёте"
        );
        assert!(
            report.verdict.contains("CONCERNS"),
            "вердикт под потолком: {}",
            report.verdict
        );

        // Низкий балл покрытия не требует: обвинение доказывается цитатами.
        let low = parse_judge_response(
            r#"{"scores": [
                 {"criterion_id": "no_contradiction", "score": 2,
                  "rationale": "Цитата subject: \"Решение: контроль слоя построен без LLM в гейте.\". Цитата reference: \"Rule: механика контроля без LLM.\". слабое место"},
                 {"criterion_id": "context", "score": 4,
                  "rationale": "Цитата: \"Решение: контроль слоя построен без LLM в гейте.\". ок"}
               ], "verdict": "v"}"#,
        )
        .expect("ответ судьи");
        let mut low_rubric = rubric_of(vec![
            covering_criterion("no_contradiction", EvidenceOn::Low),
            criterion("context", 1.0, EvidenceOn::High, &[]),
        ]);
        low_rubric.criteria[0].evidence_roles = vec!["subject".into(), "reference".into()];
        let report = build_report(&low_rubric, "judge-x", &[low], &scope, &cfg).expect("отчёт");
        assert!(
            !report.scores[0].has_flag(CriterionFlag::CoverageIncomplete),
            "при балле ниже {COVERAGE_MIN_SCORE} покрытие не требуется: {:?}",
            report.scores[0].flags
        );
    }

    /// Критерий с покрытием без досье — явная ошибка: сверять перечень не с
    /// чем, и молча пропустить проверку нельзя (S3).
    #[test]
    fn coverage_without_dossier_is_explicit_error() {
        let rubric = rubric_of(vec![covering_criterion(
            "no_contradiction",
            EvidenceOn::Low,
        )]);
        let scope = EvidenceScope::Target("просто документ");
        let err = build_report(
            &rubric,
            "judge-x",
            &[run_of("no_contradiction", 5, "противоречий нет")],
            &scope,
            &one_sample(),
        )
        .expect_err("покрытие без досье");
        let msg = err.to_string();
        assert!(msg.contains("coverage_without_dossier"), "{msg}");
        assert!(msg.contains("pack/subject"), "подсказка что делать: {msg}");
    }
}
