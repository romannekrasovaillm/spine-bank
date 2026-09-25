//! Составляющая `semantic_quality` (B1, ADR-052): смысловые рубрики по
//! отчётам судьи поверх досье [`crate::rubric_pack`].

use std::fmt::Write as _;
use std::path::Path;

use super::components::adr_is_accepted;
use super::git::GitProbe;
use super::types::{GateComponent, GateFinding};
use crate::delta;

/// Субъект смысловой проверки: рубрика и её субъект досье (ADR-052).
#[derive(Debug, Clone)]
struct SemanticSubject {
    /// Субъект досье: путь ADR/файла кода или идентификатор сущности.
    subject: String,
}

/// Потолок числа субъектов смысловой составляющей: перебор всего пакета не
/// должен превращать гейт в утилиту индексации (отсечение честно называется
/// в детали составляющей).
const MAX_SEMANTIC_SUBJECTS: usize = 200;
/// Каталоги, которые не обходятся при поиске файлов кода.
const SEMANTIC_SKIP_DIRS: [&str; 5] = [".git", "target", "node_modules", ".venv", "__pycache__"];

/// Составляющая `semantic_quality` (ADR-052): смысловые рубрики по ОТЧЁТАМ
/// судьи — решение против инварианта, ссылка не на ту сущность, обещание без
/// механизма, код против инварианта.
///
/// LLM в ядро не добавляется: составляющая собирает досье (детерминированно),
/// находит отчёт с тем же хэшем досье и читает его. Модель судит у хоста —
/// `rubric_prompt` → `rubric_verify`. Переоценки «на всякий случай» нет: отчёт
/// с совпавшим хэшем переиспользуется по построению, устаревший — находка, а
/// не повод вызвать модель.
///
/// Включается только через `[gate.required]`: по умолчанию SKIP, иначе
/// ужесточение покраснило бы чужие пайплайны без предупреждения. Пустой
/// список рубрик в конфиге — тоже SKIP: составляющую можно включить раньше,
/// чем завести отчёты.
pub(super) fn component_semantic_quality(
    repo: &Path,
    cfg: &crate::config::SemanticQualityConfig,
    rubrics_dir: &Path,
    base: Option<&str>,
    git: &GitProbe,
    enabled: bool,
    human_policy: crate::config::HumanPolicy,
) -> GateComponent {
    if !enabled {
        return GateComponent::skip(
            "semantic_quality",
            "не включена: добавьте 'semantic_quality' в [gate.required] нужного маршрута"
                .to_string(),
        );
    }
    if cfg.rubrics.is_empty() {
        return GateComponent::skip(
            "semantic_quality",
            "список [gate.semantic_quality] rubrics пуст — обязательных смысловых рубрик нет"
                .to_string(),
        );
    }
    let changed = changed_files_for_semantics(repo, base, git, cfg.scope);
    let artifacts = crate::rubric::load_artifacts(repo);
    let mut findings = Vec::new();
    let mut checked = 0usize;
    let mut truncated = false;
    // Субъекты, чей вход помечен детектором инъекций (E2): их суждение
    // подтвердить нельзя, и составляющая уходит в SKIP — вердикт INCOMPLETE.
    let mut injection_suspected = 0usize;
    // E3.2/E3.3: субъекты, чьё суждение механика не подтверждает по качеству
    // ответа судьи (невалидные сэмплы сверх порога, evidence_partial на Critical).
    let mut unconfirmed = 0usize;
    for name in &cfg.rubrics {
        let path = rubrics_dir.join(format!("{name}.yaml"));
        let rubric = match crate::rubric::load(&path) {
            Ok(r) => r,
            Err(e) => {
                findings.push(GateFinding::ruled(
                    "error".to_string(),
                    "semantic_rubric_unknown".to_string(),
                    format!(
                        "рубрика '{name}' не загружается из {}: {e} — составляющая настроена \
                         на рубрику, которой нет",
                        rubrics_dir.display()
                    ),
                ));
                continue;
            }
        };
        let Some(kind) = rubric.pack else {
            findings.push(GateFinding::ruled(
                "error".to_string(),
                "semantic_rubric_without_pack".to_string(),
                format!(
                    "рубрика '{name}' не объявляет вид досье (`pack:`) — собирать вход судьи \
                     нечем; смысловая составляющая работает только с рубриками по досье"
                ),
            ));
            continue;
        };
        let subjects = semantic_subjects(repo, kind, changed.as_ref());
        if subjects.len() > MAX_SEMANTIC_SUBJECTS {
            truncated = true;
        }
        for subject in subjects.into_iter().take(MAX_SEMANTIC_SUBJECTS) {
            let state = semantic_subject_state(
                repo,
                &rubric,
                kind,
                &subject,
                &artifacts,
                cfg,
                human_policy,
            );
            checked += usize::from(!matches!(state, SemanticState::Skipped));
            match &state {
                SemanticState::InjectionSuspected(_) => injection_suspected += 1,
                s if s.escalates(human_policy) => unconfirmed += 1,
                _ => {}
            }
            findings.extend(state.into_findings(name, &subject, human_policy));
        }
    }
    let errors = findings.iter().filter(|f| f.severity == "error").count();
    if checked == 0 && findings.is_empty() {
        return GateComponent::skip(
            "semantic_quality",
            format!(
                "обязательных субъектов не нашлось (рубрик: {}, область: {}); {}",
                cfg.rubrics.len(),
                scope_label(cfg.scope),
                if changed.as_ref().is_some_and(Vec::is_empty) {
                    "диффом не затронут ни один субъект"
                } else {
                    "в пакете нет подходящих субъектов"
                }
            ),
        );
    }
    let mut detail = format!(
        "рубрик: {}, субъектов: {checked}, находок: {} (error: {errors}); порог {:.2}",
        cfg.rubrics.len(),
        findings.len(),
        cfg.min_score
    );
    if truncated {
        let _ = write!(
            detail,
            "; субъектов в пакете больше потолка {MAX_SEMANTIC_SUBJECTS} — проверены первые"
        );
    }
    let notes = vec![
        "балл и обвинение — суждение LLM-судьи по досье; механика сверяет хэш досье, \
         порог и подтверждённость цитат, но не качество суждения"
            .to_string(),
    ];
    if injection_suspected + unconfirmed > 0 {
        // E2/E3: «проверить нельзя», а не «нечего проверять» и не «нарушено».
        // SKIP обязательной составляющей делает вердикт INCOMPLETE (exit 3) —
        // ровно то, что ревью называет решением `human`.
        return GateComponent::skip_with_findings(
            "semantic_quality",
            format!(
                "{detail}; суждений, которые механика не подтверждает: {} \
                 (вход с инъекцией: {injection_suspected}, качество ответа судьи: {unconfirmed}) — \
                 решение за человеком",
                injection_suspected + unconfirmed
            ),
            findings,
        )
        .noting(notes);
    }
    if errors == 0 {
        GateComponent::pass_with_findings("semantic_quality", detail, findings).noting(notes)
    } else {
        GateComponent::fail("semantic_quality", detail, findings).noting(notes)
    }
}

/// Область субъектов для вывода.
fn scope_label(scope: crate::config::SemanticScope) -> &'static str {
    match scope {
        crate::config::SemanticScope::Changed => "changed",
        crate::config::SemanticScope::All => "all",
    }
}

/// Изменённые файлы для области `changed`; `None` — область `all` (ограничения
/// по диффу нет).
fn changed_files_for_semantics(
    repo: &Path,
    base: Option<&str>,
    git: &GitProbe,
    scope: crate::config::SemanticScope,
) -> Option<Vec<String>> {
    if scope == crate::config::SemanticScope::All || !git.repo {
        return None;
    }
    delta::guard(repo, base, &[]).ok().map(|r| r.changed_files)
}

/// Состояние субъекта: что о нём говорит отчёт судьи.
enum SemanticState {
    /// Проверять нечего (субъект выпал из области).
    Skipped,
    /// Отчёта нет.
    Missing,
    /// Досье изменилось после оценки.
    Stale(String),
    /// Обвинение подтверждено цитатами по обеим ролям — блокирующая находка.
    Contradiction {
        /// Главный критерий.
        criterion: String,
        /// Балл.
        score: u8,
        /// Цитаты из обоснования (обе стороны).
        quotes: String,
    },
    /// Главный критерий низкий, но обвинение не подтверждено.
    Unconfirmed(String),
    /// Высокий балл без полного перечня проверенного.
    CoverageIncomplete(Vec<String>),
    /// Взвешенный итог ниже порога.
    Low(f64, String),
    /// Судья и автор — одна модель.
    JudgeIsAuthor(String),
    /// Во входе субъекта есть строки с паттернами prompt-инъекций (E2):
    /// свидетельства оттуда не засчитаны, и суждение по такому входу механика
    /// подтвердить не может — решение за человеком.
    InjectionSuspected(Vec<usize>),
    /// Доля сэмплов судьи с баллом вне шкалы выше порога (E3.2): суждению
    /// верить нельзя — решение за человеком.
    InvalidSamples(f64),
    /// То же, но доля ниже порога: предупреждение, а не эскалация (E3.2).
    InvalidSamplesWarn(f64),
    /// Часть свидетельств судьи не подтвердилась (E3.3): политика маршрута
    /// (E4.2) решает, блокировать ли из-за этого вердикт.
    PartialEvidence(String),
    /// Оговорку судьи разобрал архитектор: решение записано и принято (E4.5) —
    /// эскалации нет, но находка остаётся видимой.
    HumanAccepted(String),
    /// Два независимых судьи разошлись (E5.1): их оценки одного входа не
    /// сошлись — решение человека.
    JudgesDisagreed(String),
    /// Судья не квалифицирован на эталонном наборе, а маршрут блокирующий
    /// (E6.3): «промах» неотличим от «не умеет» — решение человека.
    JudgeUnqualified(String),
    /// Всё в порядке.
    Ok,
}

impl SemanticState {
    /// Блокирует ли состояние вердикт при такой политике маршрута (E4.2).
    /// Инъекция во входе и невалидные сэмплы не понижаются: это не настройка,
    /// а отказ доверия к суждению (E2/E3.2). `HumanAccepted` (решение
    /// архитектора уже принято, E4.5) и остальные состояния не блокируют.
    fn escalates(&self, policy: crate::config::HumanPolicy) -> bool {
        match self {
            Self::InjectionSuspected(_) | Self::InvalidSamples(_) | Self::JudgeUnqualified(_) => {
                true
            }
            Self::PartialEvidence(_)
            | Self::Unconfirmed(_)
            | Self::CoverageIncomplete(_)
            | Self::JudgesDisagreed(_) => policy == crate::config::HumanPolicy::Human,
            _ => false,
        }
    }

    /// Находки состояния; пусто — состояние не оставляет следа в отчёте.
    fn into_findings(
        self,
        rubric: &str,
        subject: &SemanticSubject,
        policy: crate::config::HumanPolicy,
    ) -> Vec<GateFinding> {
        let escalated = self.escalates(policy);
        let who = format!("{} '{}' (рубрика {rubric})", "субъект", subject.subject);
        match self {
            Self::Skipped | Self::Ok => Vec::new(),
            Self::Missing => vec![GateFinding::ruled(
                "error".to_string(),
                "semantic_report_missing".to_string(),
                format!(
                    "{who}: нет отчёта смысловой рубрики — субъект не оценён; прогоните \
                     rubric_prompt → rubric_verify (pack/subject) или `arch-be rubric run`"
                ),
            )],
            Self::Stale(source) => vec![GateFinding::ruled(
                "error".to_string(),
                "semantic_report_stale".to_string(),
                format!(
                    "{who}: отчёт устарел — после оценки изменился источник досье ({source}); \
                     это может быть правка спайна, а не субъекта"
                ),
            )],
            Self::Contradiction {
                criterion,
                score,
                quotes,
            } => vec![GateFinding::ruled(
                "error".to_string(),
                "semantic_contradiction".to_string(),
                format!(
                    "{who}: главный критерий '{criterion}' = {score} — обвинение подтверждено \
                     цитатами: {quotes}"
                ),
            )],
            Self::Unconfirmed(detail) => vec![GateFinding::ruled(
                if escalated { "error" } else { "warn" }.to_string(),
                "semantic_accusation_unconfirmed".to_string(),
                format!(
                    "{who}: главный критерий низкий, но цитаты не подтверждены ({detail}) — \
                     критерий исключён из итога{}",
                    if escalated {
                        "; на этом маршруте решение за человеком"
                    } else {
                        ", гейт этим не краснеет"
                    }
                ),
            )],
            Self::CoverageIncomplete(criteria) => vec![GateFinding::ruled(
                if escalated { "error" } else { "warn" }.to_string(),
                "semantic_coverage_incomplete".to_string(),
                format!(
                    "{who}: высокий балл без полного перечня проверенных источников ({}) — \
                     критерии исключены из итога{}",
                    criteria.join(", "),
                    if escalated {
                        "; на этом маршруте решение за человеком"
                    } else {
                        ""
                    }
                ),
            )],
            Self::Low(total, judge) => vec![GateFinding::ruled(
                // warn, а не error: взвешенный итог смешивает качество документа
                // с дисциплиной цитирования судьи — на чистом контроле живого
                // прогона 2026-09-20 три критерия из четырёх остались без
                // свидетельств, и итог 3.43 стоял в 0.43 от порога. Ошибкой
                // краснело бы честное решение за поведение судьи. Красный —
                // только за обвинение по главному критерию с цитатами.
                "warn".to_string(),
                "semantic_quality_low".to_string(),
                format!("{who}: {total:.2}/5 ниже порога (судья {judge})"),
            )],
            Self::JudgeIsAuthor(judge) => vec![GateFinding::ruled(
                "warn".to_string(),
                "judge_is_author".to_string(),
                format!("{who}: судья и автор — одна модель ({judge}) — оценка не независима"),
            )],
            Self::InvalidSamples(ratio) => vec![GateFinding::ruled(
                "error".to_string(),
                "semantic_invalid_samples".to_string(),
                format!(
                    "{who}: доля сэмплов судьи с баллом вне шкалы {:.0}% выше порога — суждению \
                     верить нельзя, решение за человеком",
                    ratio * 100.0
                ),
            )],
            Self::InvalidSamplesWarn(ratio) => vec![GateFinding::ruled(
                "warn".to_string(),
                "semantic_invalid_samples".to_string(),
                format!(
                    "{who}: {:.0}% сэмплов судьи пришли с баллом вне шкалы — в расчёт не вошли",
                    ratio * 100.0
                ),
            )],
            Self::JudgeUnqualified(reason) => vec![GateFinding::ruled(
                "error".to_string(),
                "judge_unqualified".to_string(),
                format!("{who}: {reason} — `arch-be rubric qualify`"),
            )],
            Self::JudgesDisagreed(detail) => vec![GateFinding::ruled(
                if escalated { "error" } else { "warn" }.to_string(),
                "judge_disagreement".to_string(),
                format!(
                    "{who}: два независимых судьи разошлись ({detail}){}",
                    if escalated {
                        " — решение человека"
                    } else {
                        ""
                    }
                ),
            )],
            Self::HumanAccepted(decision) => vec![GateFinding::ruled(
                "warn".to_string(),
                "human_decision_accepted".to_string(),
                format!(
                    "{who}: оговорку судьи разобрал архитектор — {decision}; эскалации нет, \
                     находка остаётся видимой"
                ),
            )],
            Self::PartialEvidence(criteria) => vec![GateFinding::ruled(
                if escalated { "error" } else { "warn" }.to_string(),
                "semantic_evidence_partial".to_string(),
                format!(
                    "{who}: часть свидетельств судьи не подтвердилась ({criteria}){}",
                    if escalated {
                        " — на этом маршруте решение за человеком"
                    } else {
                        ""
                    }
                ),
            )],
            Self::InjectionSuspected(lines) => vec![GateFinding::ruled(
                // error: это не «нарушение субъекта», а недействительность
                // суждения о нём. Составляющая при этой находке — SKIP, и
                // вердикт гейта становится INCOMPLETE (exit 3): молча зелёным
                // такой вход не проходит, а решение принимает человек.
                "error".to_string(),
                "semantic_input_injection".to_string(),
                format!(
                    "{who}: во входе досье есть строки с паттернами prompt-инъекций ({lines:?}) — \
                     цитаты оттуда свидетельствами не засчитаны, суждение по этому входу \
                     механика подтвердить не может"
                ),
            )],
        }
    }
}

/// Что говорит отчёт по субъекту: свежесть досье, главный критерий, порог.
fn semantic_subject_state(
    repo: &Path,
    rubric: &crate::rubric::Rubric,
    kind: crate::rubric_pack::PackKind,
    subject: &SemanticSubject,
    artifacts: &[crate::rubric::RubricArtifact],
    cfg: &crate::config::SemanticQualityConfig,
    human_policy: crate::config::HumanPolicy,
) -> SemanticState {
    let Ok(packs) = crate::rubric_pack::build(repo, kind, &subject.subject) else {
        // Досье не собирается (секрет, лимит, битый маркер) — это не «нет
        // отчёта», а «судить нечем»; называем причину как пропуск.
        return SemanticState::Skipped;
    };
    let artifact = artifacts.iter().rev().find(|a| {
        a.rubric == rubric.name && a.subject.as_deref() == Some(subject.subject.as_str())
    });
    let Some(artifact) = artifact else {
        return SemanticState::Missing;
    };
    // E2: вход с инъекцией обесценивает суждение целиком — проверяется раньше
    // свежести: судья мог подчиниться строке из досье, и «свежий» отчёт об
    // этом ничего не говорит. Решение по такому субъекту — за человеком.
    if let Some(lines) = artifact
        .input_injection_lines
        .as_ref()
        .filter(|l| !l.is_empty())
    {
        return SemanticState::InjectionSuspected(lines.clone());
    }
    // E3.2: качество ответа судьи — невалидные сэмплы. Выше порога решение
    // механике не подтвердить, ниже — предупреждение.
    if artifact.invalid_samples_ratio > cfg.max_invalid_samples_ratio {
        return SemanticState::InvalidSamples(artifact.invalid_samples_ratio);
    }
    if artifact.invalid_samples_ratio > 0.0 {
        return SemanticState::InvalidSamplesWarn(artifact.invalid_samples_ratio);
    }
    // E6.3: на блокирующем маршруте судья обязан быть квалифицирован на
    // эталонном наборе. Проверка первая: остальное о необученном судье
    // говорит мало.
    if human_policy == crate::config::HumanPolicy::Human && cfg.require_qualified_judge {
        let state = crate::rubric::qualification(repo, &artifact.rubric, &artifact.judge_model);
        if !matches!(state, crate::rubric::Qualification::Qualified(_)) {
            return SemanticState::JudgeUnqualified(crate::rubric::refusal_reason(
                &state,
                &artifact.judge_model,
            ));
        }
    }
    // E5.1: два независимых судьи разошлись — суждение не принято, решает
    // человек (политика маршрута решает, блокирует ли это вердикт).
    if let Some(second) = artifact.second_judge.as_ref().filter(|s| !s.agreement) {
        return SemanticState::JudgesDisagreed(format!(
            "второй судья {}: {}",
            second.model,
            if second.differences.is_empty() {
                "оценки не сошлись".to_string()
            } else {
                second.differences.join("; ")
            }
        ));
    }
    // E4.5: решение архитектора по этому отчёту снимает эскалацию оговорок
    // судьи (инъекции и невалидные сэмплы оно не снимает — они выше).
    let decided = crate::rubric::decision_for(repo, artifact)
        .filter(|record| record.decision == crate::rubric::HumanVerdict::Accept);
    let resolved = |state: SemanticState| -> SemanticState {
        match &decided {
            Some(record) => SemanticState::HumanAccepted(format!(
                "{} ({}){}",
                record.decided_by,
                record.decided_at,
                if record.reason.is_empty() {
                    String::new()
                } else {
                    format!(": {}", record.reason)
                }
            )),
            None => state,
        }
    };
    // E3.3/E4.2: оговорка судьи (`evidence_partial`) — состояние, а блокирует
    // ли она вердикт, решает политика маршрута.
    let partial: Vec<String> = artifact
        .scores
        .iter()
        .filter(|s| s.has_flag(crate::rubric::CriterionFlag::EvidencePartial))
        .map(|s| s.criterion_id.clone())
        .collect();
    if !partial.is_empty() {
        return resolved(SemanticState::PartialEvidence(partial.join(", ")));
    }
    // Отчёт привязан ко ВСЕМ источникам досье (ADR-051, П3): правка спайна
    // обесценивает отчёт о решении, даже если сам ADR не менялся.
    if let Some(want) = artifact.pack_sha256.as_deref() {
        let fresh = packs.iter().find(|p| p.subject == subject.subject);
        match fresh {
            Some(pack) if pack.sha256 == want => {}
            Some(pack) => {
                let changed = changed_source(artifact, pack);
                return SemanticState::Stale(changed);
            }
            None => return SemanticState::Skipped,
        }
    }
    let mut state = SemanticState::Ok;
    // Обвинение по главному критерию.
    if let Some(main) = rubric.criteria.iter().find(|c| c.blocking) {
        if let Some(score) = artifact.scores.iter().find(|s| s.criterion_id == main.id) {
            if score.score <= 2 {
                if score.flags.iter().any(|f| f.excludes_from_total()) {
                    state = resolved(SemanticState::Unconfirmed(format!(
                        "метки: {:?}",
                        score.flags
                    )));
                } else {
                    state = SemanticState::Contradiction {
                        criterion: main.id.clone(),
                        score: score.score,
                        quotes: score.rationale.clone(),
                    };
                }
            }
        }
    }
    // Взвешенный итог ниже порога — факт, подтверждённый счётом: критерий без
    // подтверждённых цитат в итог не входит (`excludes_from_total`), поэтому
    // «обвинение не подтверждено» не отменяет низкий итог и не должно его
    // вытеснять. Живой прогон D11 (2026-09-20): главный критерий 1 с меткой
    // `accusation_unconfirmed` утопил итог 1.00/5 в предупреждение, и гейт
    // перестал бы краснеть на коде, нарушающем инвариант. Порядок силы:
    // противоречие → низкий итог → неподтверждённое обвинение.
    if artifact.weighted_total < cfg.min_score
        && matches!(
            state,
            SemanticState::Ok
                | SemanticState::Unconfirmed(_)
                | SemanticState::CoverageIncomplete(_)
        )
    {
        state = SemanticState::Low(artifact.weighted_total, artifact.judge_model.clone());
    }
    if matches!(state, SemanticState::Ok) {
        let uncovered: Vec<String> = artifact
            .scores
            .iter()
            .filter(|s| s.has_flag(crate::rubric::CriterionFlag::CoverageIncomplete))
            .map(|s| s.criterion_id.clone())
            .collect();
        if !uncovered.is_empty() {
            state = resolved(SemanticState::CoverageIncomplete(uncovered));
        }
    }
    // «Автор = судья» — отдельная находка, но она не перекрывает суть
    // (обвинение или пропуск отчёта): печатается только на спокойном итоге.
    if matches!(state, SemanticState::Ok) {
        let author_missing = artifact
            .author_model
            .as_deref()
            .is_none_or(|a| a.trim().is_empty());
        if author_missing || artifact.author_model.as_deref() == Some(artifact.judge_model.as_str())
        {
            state = SemanticState::JudgeIsAuthor(artifact.judge_model.clone());
        }
    }
    state
}

/// Какой источник досье изменился после оценки — поимённо, для сообщения.
fn changed_source(
    artifact: &crate::rubric::RubricArtifact,
    pack: &crate::rubric_pack::ContextPack,
) -> String {
    for input in &pack.inputs {
        let was = artifact
            .inputs
            .iter()
            .find(|i| i.path == input.path)
            .map(|i| i.sha256.as_str());
        match was {
            None => return format!("{} — источник появился после оценки", input.path),
            Some(sha) if sha != input.sha256 => {
                return format!("{} — изменён после оценки", input.path);
            }
            Some(_) => {}
        }
    }
    "состав досье изменился".to_string()
}

/// Субъекты рубрики в пакете; для области `changed` — только те, чьё досье
/// затронуто диффом (сам субъект или любой его источник, включая спайн).
fn semantic_subjects(
    repo: &Path,
    kind: crate::rubric_pack::PackKind,
    changed: Option<&Vec<String>>,
) -> Vec<SemanticSubject> {
    let mut out = Vec::new();
    let mut push = |subject: String| {
        if out.iter().any(|s: &SemanticSubject| s.subject == subject) {
            return;
        }
        out.push(SemanticSubject { subject });
    };
    match kind {
        crate::rubric_pack::PackKind::AdrVsSpine => {
            for adr in accepted_adr_paths(repo) {
                push(adr);
            }
        }
        crate::rubric_pack::PackKind::EntityLinks | crate::rubric_pack::PackKind::NfrMechanism => {
            for id in linked_entities(repo, kind) {
                push(id);
            }
        }
        crate::rubric_pack::PackKind::CodeVsSpine => {
            for file in code_files(repo) {
                push(file);
            }
        }
        crate::rubric_pack::PackKind::SolutionVsStandards => {
            // E10.1: смысловая рубрика слоя ДКА судит солюшен-документы.
            for doc in solution_docs(repo) {
                push(doc);
            }
        }
        crate::rubric_pack::PackKind::CodeVsScenarios => {
            // E11.1: сценарии OpenSpec проверяются по файлам кода модели.
            for file in code_files(repo) {
                push(file);
            }
        }
    }
    let Some(changed) = changed else {
        return out;
    };
    // Область `changed`: субъект остаётся, если затронут он сам или любой
    // источник его досье — правка спайна меняет вердикт о решении, хотя
    // решение не правили.
    out.retain(|s| {
        let Ok(packs) = crate::rubric_pack::build(repo, kind, &s.subject) else {
            return false;
        };
        packs.iter().any(|p| {
            p.inputs
                .iter()
                .any(|i| changed.iter().any(|c| same_path(c, i.path.as_str())))
        })
    });
    out
}

/// Один и тот же файл: пути сравниваются по нормализованной форме (слеши,
/// суффикс `#фрагмент`, ведущее `./`).
fn same_path(a: &str, b: &str) -> bool {
    let norm = |p: &str| {
        p.split('#')
            .next()
            .unwrap_or(p)
            .trim_start_matches("./")
            .replace('\\', "/")
    };
    norm(a) == norm(b)
}

/// Принятые ADR (`docs/adr/ADR-*.md`).
fn accepted_adr_paths(repo: &Path) -> Vec<String> {
    let dir = repo.join("docs/adr");
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut out: Vec<String> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.extension().is_some_and(|x| x.eq_ignore_ascii_case("md"))
                && p.file_name()
                    .is_some_and(|n| n.to_string_lossy().starts_with("ADR-"))
        })
        .filter(|p| std::fs::read_to_string(p).is_ok_and(|text| adr_is_accepted(&text)))
        .map(|p| crate::rubric_pack::relative_path(repo, &p))
        .collect();
    out.sort();
    out
}

/// Сущности модели, которые стоит судить: со связями и (для `nfr_mechanism`)
/// только показатели. Карточка без связей смысловой рубрике не о чём.
fn linked_entities(repo: &Path, kind: crate::rubric_pack::PackKind) -> Vec<String> {
    let Ok(model) = crate::model::load_model_tolerant(&repo.join("model")) else {
        return Vec::new();
    };
    let mut out: Vec<String> = model
        .entities
        .iter()
        .filter(|e| {
            e.depends_on
                .iter()
                .chain(&e.implements)
                .chain(&e.affects)
                .chain(&e.verified_by)
                .count()
                > 0
        })
        .filter(|e| {
            kind != crate::rubric_pack::PackKind::NfrMechanism
                || e.kind == crate::model::EntityKind::Nfr
        })
        .map(|e| e.id.clone())
        .collect();
    out.sort();
    out
}

/// Файлы кода под корнями компонент (`CMP.code_roots`) — субъекты рубрики
/// «код против инварианта».
/// Солюшен-документы слоя ДКА (E10.1): `docs/solution/**/*.md` (рекурсивно).
fn solution_docs(repo: &Path) -> Vec<String> {
    let mut out = Vec::new();
    collect_markdown(repo, &repo.join("docs/solution"), &mut out);
    out.sort();
    out.dedup();
    out
}

/// Рекурсивный сбор markdown-файлов относительно корня.
fn collect_markdown(repo: &Path, dir: &Path, out: &mut Vec<String>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in rd.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_markdown(repo, &path, out);
        } else if path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("md"))
        {
            if let Ok(rel) = path.strip_prefix(repo) {
                out.push(rel.to_string_lossy().into_owned());
            }
        }
    }
}

fn code_files(repo: &Path) -> Vec<String> {
    let Ok(model) = crate::model::load_model_tolerant(&repo.join("model")) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for root in model.entities.iter().flat_map(|e| e.code_roots.iter()) {
        collect_code_files(repo, &repo.join(root), &mut out);
    }
    out.sort();
    out.dedup();
    out
}

/// Рекурсивный обход каталога кода с пропуском служебных каталогов.
fn collect_code_files(repo: &Path, dir: &Path, out: &mut Vec<String>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in rd.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if path.is_dir() {
            if SEMANTIC_SKIP_DIRS.contains(&name.as_str()) || name.starts_with('.') {
                continue;
            }
            collect_code_files(repo, &path, out);
        } else if path.is_file() {
            out.push(crate::rubric_pack::relative_path(repo, &path));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::Route;
    use crate::gate::testkit::*;
    use crate::gate::verdict::run_inner;
    use crate::gate::{
        GateOptions, GateOutcome, GateReport, GateRequirements, GateStatus, run_with,
    };
    use std::path::PathBuf;

    // --- ADR-052: смысловые рубрики как составляющая гейта ------------------

    /// Каталог рубрик с одной смысловой рубрикой (`adr_vs_spine`): берём
    /// встроенную, чтобы тест проверял настоящий контракт рубрики.
    fn semantic_rubrics_dir(dir: &Path) -> PathBuf {
        let rubrics = dir.join("rubrics");
        std::fs::create_dir_all(&rubrics).expect("mkdir rubrics");
        std::fs::write(
            rubrics.join("adr_spine_consistency.yaml"),
            crate::assets::RUBRIC_ADR_SPINE_CONSISTENCY,
        )
        .expect("рубрика");
        rubrics
    }

    /// Настройки составляющей на одну рубрику.
    fn semantic_cfg(scope: crate::config::SemanticScope) -> crate::config::SemanticQualityConfig {
        crate::config::SemanticQualityConfig {
            rubrics: vec!["adr_spine_consistency".to_string()],
            scope,
            min_score: 3.5,
            require_distinct_judge: false,
            max_invalid_samples_ratio: 0.5,
            // E6.3: тесты составляющей включают допуск судьи осознанно — в
            // конфиге проекта он по умолчанию выключен.
            require_qualified_judge: true,
        }
    }

    /// Требования с включённой смысловой составляющей.
    fn with_semantic(route: Route) -> GateRequirements {
        let mut req = GateRequirements::default();
        let list = match route {
            Route::Fast => &mut req.fast,
            Route::Standard => &mut req.standard,
            Route::Critical => &mut req.critical,
        };
        list.push("semantic_quality".to_string());
        req
    }

    /// Репозиторий с Accepted-ADR и инвариантом спайна — субъект и ссылка
    /// досье `adr_vs_spine`.
    fn make_semantic_repo(dir: &Path) -> PathBuf {
        make_gate_repo(dir);
        std::fs::create_dir_all(dir.join("docs/adr")).expect("mkdir adr");
        let adr = dir.join("docs/adr/ADR-001-reshenie.md");
        std::fs::write(
            &adr,
            "# ADR-001. Решение\n\n- Date: 2026-09-19\n- Status: Accepted\n\n## Context\n\nПричина.\n",
        )
        .expect("adr");
        std::fs::write(
            dir.join("ARCHITECTURE-SPINE.md"),
            "# Spine\n\n## AD-1: Журнал только дописывается\n\n\
             - **Binds**: журнал операций ↔ код записи\n\
             - **Prevents**: правку и удаление строк журнала\n\
             - **Rule**: строки журнала не правятся.\n",
        )
        .expect("spine");
        git(dir, &["add", "."]);
        git(dir, &["commit", "-q", "--allow-empty", "-m", "semantic"]);
        adr
    }

    /// Отчёт смысловой рубрики в репозитории: `pack_sha256` считается по
    /// текущему досье, если не задан иначе.
    fn write_semantic_report(
        dir: &Path,
        subject: &str,
        main_score: u8,
        flags: &[&str],
        total: f64,
        pack_sha256: Option<&str>,
    ) {
        let packs =
            crate::rubric_pack::build(dir, crate::rubric_pack::PackKind::AdrVsSpine, subject)
                .expect("досье");
        let sha = packs[0].sha256.clone();
        let artifact = serde_json::json!({
            "schema": crate::rubric::RUBRIC_REPORT_SCHEMA,
            "rubric": "adr_spine_consistency",
            "judge_model": "judge-x",
            "author_model": "author-y",
            "weighted_total": total,
            "verdict": "CONCERNS",
            "pack_kind": "adr_vs_spine",
            "subject": subject,
            "pack_sha256": pack_sha256.unwrap_or(sha.as_str()),
            "inputs": packs[0].inputs.iter().map(|i| serde_json::json!({
                "path": i.path, "sha256": i.sha256, "role": i.role.as_str(), "id": i.id,
            })).collect::<Vec<_>>(),
            "scores": [{
                "criterion_id": "no_contradiction",
                "weight": 3.0,
                "score": main_score,
                "rationale": "Цитата subject: \"Решение\". Цитата reference: \"Rule\".",
                "samples": [main_score],
                "stdev": 0.0,
                "flags": flags,
                "evidence_unconfirmed_ratio": 0.0,
                "checked": ["AD-1"],
            }],
            "judged_at": "2026-09-20T10:00:00+03:00",
        });
        let reports = dir.join(crate::rubric::RUBRIC_REPORTS_DIR);
        std::fs::create_dir_all(&reports).expect("mkdir reports");
        std::fs::write(
            reports.join("semantic.json"),
            serde_json::to_string_pretty(&artifact).expect("json"),
        )
        .expect("write report");
    }

    /// Прогон гейта с настройками смысловой составляющей.
    fn run_semantic(
        dir: &Path,
        base: Option<&str>,
        cfg: crate::config::SemanticQualityConfig,
        rubrics_dir: &Path,
    ) -> GateReport {
        let options = GateOptions {
            semantic_quality: cfg,
            rubrics_dir: rubrics_dir.to_path_buf(),
            ..GateOptions::default()
        };
        run_inner(
            dir,
            Some(Route::Fast),
            base,
            None,
            (1, 4),
            &with_semantic(Route::Fast),
            &options,
        )
        .expect("гейт")
    }

    /// Находки составляющей по коду правила.
    fn semantic_rules(report: &GateReport) -> Vec<String> {
        report
            .components
            .iter()
            .find(|c| c.name == "semantic_quality")
            .map(|c| c.findings.iter().filter_map(|f| f.rule.clone()).collect())
            .unwrap_or_default()
    }
    /// По умолчанию составляющая — SKIP: включение только через `[gate.required]`.
    #[test]
    fn semantic_quality_is_skip_by_default() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path();
        make_semantic_repo(dir);
        let report = run_with(
            dir,
            Some(Route::Fast),
            None,
            None,
            (1, 4),
            &GateRequirements::default(),
        )
        .expect("гейт");
        assert_eq!(status_of(&report, "semantic_quality"), GateStatus::Skip);
        assert_eq!(report.outcome, GateOutcome::Pass);
    }

    /// Включённая составляющая без отчёта — error: смысловая рубрика обязана
    /// иметь отчёт, иначе «зелёный» означал бы «не смотрели».
    #[test]
    fn semantic_quality_requires_report_when_enabled() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path();
        make_semantic_repo(dir);
        let rubrics = semantic_rubrics_dir(dir);
        let report = run_semantic(
            dir,
            None,
            semantic_cfg(crate::config::SemanticScope::All),
            &rubrics,
        );
        assert_eq!(status_of(&report, "semantic_quality"), GateStatus::Fail);
        assert!(
            semantic_rules(&report).contains(&"semantic_report_missing".to_string()),
            "{:?}",
            semantic_rules(&report)
        );
    }

    /// Совпавший хэш досье — отчёт переиспользуется: находок нет.
    #[test]
    fn semantic_quality_reuses_report_with_same_pack_hash() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path();
        make_semantic_repo(dir);
        let rubrics = semantic_rubrics_dir(dir);
        write_semantic_report(dir, "docs/adr/ADR-001-reshenie.md", 5, &[], 4.5, None);
        let report = run_semantic(
            dir,
            None,
            semantic_cfg(crate::config::SemanticScope::All),
            &rubrics,
        );
        assert_eq!(status_of(&report, "semantic_quality"), GateStatus::Pass);
        assert!(
            semantic_rules(&report).is_empty(),
            "{:?}",
            semantic_rules(&report)
        );
    }

    /// Правка спайна обесценивает отчёт о решении: в сообщении назван
    /// изменившийся источник, а не только субъект.
    #[test]
    fn semantic_quality_stale_when_spine_changes() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path();
        make_semantic_repo(dir);
        let rubrics = semantic_rubrics_dir(dir);
        write_semantic_report(dir, "docs/adr/ADR-001-reshenie.md", 5, &[], 4.5, None);
        // Меняем СПАЙН, а не ADR.
        std::fs::write(
            dir.join("ARCHITECTURE-SPINE.md"),
            "# Spine\n\n## AD-1: Журнал только дописывается\n\n\
             - **Binds**: журнал операций ↔ код записи\n\
             - **Prevents**: правку и удаление строк журнала\n\
             - **Rule**: строки журнала не правятся ничем.\n",
        )
        .expect("spine");
        let report = run_semantic(
            dir,
            None,
            semantic_cfg(crate::config::SemanticScope::All),
            &rubrics,
        );
        assert_eq!(status_of(&report, "semantic_quality"), GateStatus::Fail);
        assert!(
            semantic_rules(&report).contains(&"semantic_report_stale".to_string()),
            "{:?}",
            semantic_rules(&report)
        );
        let detail = report
            .components
            .iter()
            .find(|c| c.name == "semantic_quality")
            .map(|c| {
                c.findings
                    .iter()
                    .map(|f| f.message.clone())
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .unwrap_or_default();
        assert!(
            detail.contains("ARCHITECTURE-SPINE.md"),
            "назван изменившийся источник: {detail}"
        );
    }

    /// Подтверждённое обвинение по главному критерию — блокирующая находка.
    #[test]
    fn semantic_quality_blocks_on_confirmed_contradiction() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path();
        make_semantic_repo(dir);
        let rubrics = semantic_rubrics_dir(dir);
        write_semantic_report(dir, "docs/adr/ADR-001-reshenie.md", 1, &[], 2.0, None);
        let report = run_semantic(
            dir,
            None,
            semantic_cfg(crate::config::SemanticScope::All),
            &rubrics,
        );
        assert_eq!(status_of(&report, "semantic_quality"), GateStatus::Fail);
        let rules = semantic_rules(&report);
        assert!(
            rules.contains(&"semantic_contradiction".to_string()),
            "{rules:?}"
        );
        // Состояние субъекта одно и называет самое важное: обвинение. Порог
        // итога отдельной строкой не дублируется — «противоречие найдено»
        // говорит больше, чем «итог ниже порога».
        assert_eq!(rules.len(), 1, "{rules:?}");
    }

    /// Обвинение без подтверждённых цитат — warn: гейт этим не краснеет
    /// (решение ADR-051: выдуманное обвинение наказывает судью, а не документ).
    #[test]
    fn semantic_quality_unconfirmed_accusation_is_warn() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path();
        make_semantic_repo(dir);
        let rubrics = semantic_rubrics_dir(dir);
        write_semantic_report(
            dir,
            "docs/adr/ADR-001-reshenie.md",
            1,
            &["accusation_unconfirmed"],
            4.0,
            None,
        );
        let report = run_semantic(
            dir,
            None,
            semantic_cfg(crate::config::SemanticScope::All),
            &rubrics,
        );
        assert_eq!(
            status_of(&report, "semantic_quality"),
            GateStatus::Pass,
            "warn не краснит составляющую"
        );
        assert!(
            semantic_rules(&report).contains(&"semantic_accusation_unconfirmed".to_string()),
            "{:?}",
            semantic_rules(&report)
        );
    }

    /// Неподтверждённое обвинение не должно вытеснять подтверждённый низкий
    /// итог: живой прогон D11 (2026-09-20) дал главному критерию 1 с меткой
    /// `accusation_unconfirmed` и взвешенный итог 1.00/5 — при старом порядке
    /// гейт показал бы только «обвинение не подтверждено» и промолчал бы про
    /// итог. Итог сообщается отдельной находкой, но не краснит составляющую:
    /// красный — только за обвинение по главному критерию с цитатами.
    #[test]
    fn semantic_quality_low_total_survives_unconfirmed_accusation() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path();
        make_semantic_repo(dir);
        let rubrics = semantic_rubrics_dir(dir);
        write_semantic_report(
            dir,
            "docs/adr/ADR-001-reshenie.md",
            1,
            &["accusation_unconfirmed"],
            1.0,
            None,
        );
        let report = run_semantic(
            dir,
            None,
            semantic_cfg(crate::config::SemanticScope::All),
            &rubrics,
        );
        assert_eq!(
            status_of(&report, "semantic_quality"),
            GateStatus::Pass,
            "низкий итог — предупреждение, краснит только обвинение по главному критерию"
        );
        let rules = semantic_rules(&report);
        assert!(
            rules.contains(&"semantic_quality_low".to_string()),
            "{rules:?}"
        );
        assert_eq!(
            rules.len(),
            1,
            "состояние одно и называет самое сильное: {rules:?}"
        );
    }

    /// Область `changed`: субъектом становится только тот, чьё досье затронуто
    /// диффом, — иначе поток доработок требовал бы отчётов обо всём пакете.
    #[test]
    fn semantic_quality_scope_changed_limits_subjects() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path();
        make_semantic_repo(dir);
        let rubrics = semantic_rubrics_dir(dir);
        // Второй Accepted-ADR: он не менялся и в область `changed` не попадает.
        std::fs::write(
            dir.join("docs/adr/ADR-002-vtoroe.md"),
            "# ADR-002. Второе\n\n- Date: 2026-09-19\n- Status: Accepted\n\n## Context\n\nДругое.\n",
        )
        .expect("adr2");
        git(dir, &["add", "."]);
        git(dir, &["commit", "-q", "--allow-empty", "-m", "adr2"]);
        // Меняем только первый ADR после коммита.
        std::fs::write(
            dir.join("docs/adr/ADR-001-reshenie.md"),
            "# ADR-001. Решение\n\n- Date: 2026-09-20\n- Status: Accepted\n\n## Context\n\nПричина и следствие.\n",
        )
        .expect("adr1");
        write_semantic_report(dir, "docs/adr/ADR-001-reshenie.md", 5, &[], 4.5, None);

        let report = run_semantic(
            dir,
            Some("HEAD"),
            semantic_cfg(crate::config::SemanticScope::Changed),
            &rubrics,
        );
        assert_eq!(
            status_of(&report, "semantic_quality"),
            GateStatus::Pass,
            "отчёт есть у изменённого субъекта, второй в область не входит: {:?}",
            semantic_rules(&report)
        );
        // А в области `all` второй субъект отчёта не имеет — error.
        let all = run_semantic(
            dir,
            Some("HEAD"),
            semantic_cfg(crate::config::SemanticScope::All),
            &rubrics,
        );
        assert_eq!(status_of(&all, "semantic_quality"), GateStatus::Fail);
        let detail = all
            .components
            .iter()
            .find(|c| c.name == "semantic_quality")
            .map(|c| {
                c.findings
                    .iter()
                    .map(|f| f.message.clone())
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .unwrap_or_default();
        assert!(
            detail.contains("ADR-002-vtoroe.md") && !detail.contains("ADR-001-reshenie.md"),
            "область all называет второго, а не оценённого: {detail}"
        );
    }

    /// Рубрика из конфига, которой нет в каталоге, — явная ошибка настройки,
    /// а не молчаливый SKIP.
    #[test]
    fn semantic_quality_unknown_rubric_is_error() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path();
        make_semantic_repo(dir);
        let rubrics = semantic_rubrics_dir(dir);
        let mut cfg = semantic_cfg(crate::config::SemanticScope::All);
        cfg.rubrics = vec!["net-takoy-rubriki".to_string()];
        let report = run_semantic(dir, None, cfg, &rubrics);
        assert_eq!(status_of(&report, "semantic_quality"), GateStatus::Fail);
        assert!(
            semantic_rules(&report).contains(&"semantic_rubric_unknown".to_string()),
            "{:?}",
            semantic_rules(&report)
        );
    }

    /// Пройденная квалификация судьи `judge-x` на рубрике
    /// `adr_spine_consistency`: на Critical её требует E6.3, поэтому тесты про
    /// оговорки судьи задают это условие явно (как это сделал бы проект).
    fn write_passing_qualification(dir: &Path, rubrics: &Path) {
        let rubric =
            crate::rubric::load(&rubrics.join("adr_spine_consistency.yaml")).expect("рубрика");
        let set = crate::rubric::QualificationSet {
            dir: PathBuf::from("/набор"),
            cases: Vec::new(),
            sha256: "d".repeat(64),
        };
        let outcome = |truth: crate::rubric::Truth| crate::rubric::CaseOutcome {
            file: "code/x.py".to_string(),
            class: "ignored_key".to_string(),
            truth,
            decision: Some(match truth {
                crate::rubric::Truth::Defective => crate::rubric::RubricDecision::Fail,
                crate::rubric::Truth::Clean => crate::rubric::RubricDecision::Pass,
            }),
            weighted_total: 4.0,
            flags: Vec::new(),
            error: None,
        };
        let report = crate::rubric::build_qualification_report(
            &rubric,
            "judge-x",
            &set,
            vec![
                outcome(crate::rubric::Truth::Defective),
                outcome(crate::rubric::Truth::Clean),
            ],
            1,
        );
        assert!(report.passed, "{:?}", report.failures);
        crate::rubric::write_qualification(dir, &report).expect("квалификация");
    }

    /// Прогон гейта на заданном маршруте и с заданной политикой решения (E4.2).
    fn run_semantic_on(
        dir: &Path,
        route: Route,
        policy: crate::config::DecisionPolicyConfig,
        cfg: crate::config::SemanticQualityConfig,
        rubrics_dir: &Path,
    ) -> GateReport {
        let options = GateOptions {
            semantic_quality: cfg,
            rubrics_dir: rubrics_dir.to_path_buf(),
            decision_policy: policy,
            ..GateOptions::default()
        };
        run_inner(
            dir,
            Some(route),
            None,
            None,
            (1, 4),
            &with_semantic(route),
            &options,
        )
        .expect("гейт")
    }

    /// E6.3: на Critical судья без пройденной квалификации к гейту не
    /// допускается — SKIP с находкой `judge_unqualified`; после квалификации
    /// проходит обычным путём.
    #[test]
    fn unqualified_judge_blocks_on_critical() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path();
        make_semantic_repo(dir);
        let rubrics = semantic_rubrics_dir(dir);
        write_semantic_report(dir, "docs/adr/ADR-001-reshenie.md", 5, &[], 4.6, None);
        let policy = crate::config::DecisionPolicyConfig::default();
        let before = run_semantic_on(
            dir,
            Route::Critical,
            policy.clone(),
            semantic_cfg(crate::config::SemanticScope::All),
            &rubrics,
        );
        assert_eq!(
            status_of(&before, "semantic_quality"),
            GateStatus::Skip,
            "{}",
            crate::gate::render(&before)
        );
        assert!(
            semantic_rules(&before).contains(&"judge_unqualified".to_string()),
            "{:?}",
            semantic_rules(&before)
        );
        // Квалификация судьи `judge-x` на этой рубрике пройдена.
        let rubric =
            crate::rubric::load(&rubrics.join("adr_spine_consistency.yaml")).expect("рубрика");
        let set = crate::rubric::QualificationSet {
            dir: std::path::PathBuf::from("/набор"),
            cases: Vec::new(),
            sha256: "c".repeat(64),
        };
        let outcome = |truth| crate::rubric::CaseOutcome {
            file: "code/x.py".to_string(),
            class: "ignored_key".to_string(),
            truth,
            decision: Some(match truth {
                crate::rubric::Truth::Defective => crate::rubric::RubricDecision::Fail,
                crate::rubric::Truth::Clean => crate::rubric::RubricDecision::Pass,
            }),
            weighted_total: 4.0,
            flags: Vec::new(),
            error: None,
        };
        let qualification = crate::rubric::build_qualification_report(
            &rubric,
            "judge-x",
            &set,
            vec![
                outcome(crate::rubric::Truth::Defective),
                outcome(crate::rubric::Truth::Clean),
            ],
            1,
        );
        assert!(qualification.passed, "{:?}", qualification.failures);
        crate::rubric::write_qualification(dir, &qualification).expect("квалификация");
        let after = run_semantic_on(
            dir,
            Route::Critical,
            policy,
            semantic_cfg(crate::config::SemanticScope::All),
            &rubrics,
        );
        assert_eq!(
            status_of(&after, "semantic_quality"),
            GateStatus::Pass,
            "квалифицированный судья проходит: {}",
            crate::gate::render(&after)
        );
    }

    /// E5.1: расхождение двух судей — решение человека. На Critical оговорка
    /// блокирует вердикт (SKIP → INCOMPLETE), на Fast остаётся предупреждением.
    #[test]
    fn judge_disagreement_follows_route_policy() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path();
        make_semantic_repo(dir);
        let rubrics = semantic_rubrics_dir(dir);
        write_semantic_report(dir, "docs/adr/ADR-001-reshenie.md", 5, &[], 4.5, None);
        // Второй судья разошёлся с первым: отметка в отчёте (E5.1).
        let path = dir
            .join(crate::rubric::RUBRIC_REPORTS_DIR)
            .join("semantic.json");
        let mut artifact: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).expect("отчёт")).expect("JSON");
        artifact["second_judge"] = serde_json::json!({
            "model": "judge-2",
            "report": "reports/rubric/semantic--second.json",
            "decision": "fail",
            "weighted_total": 1.0,
            "agreement": false,
            "differences": ["решения разошлись: pass и fail"],
        });
        std::fs::write(
            &path,
            serde_json::to_string_pretty(&artifact).expect("json"),
        )
        .expect("write report");
        write_passing_qualification(dir, &rubrics);
        let policy = crate::config::DecisionPolicyConfig::default();
        let critical = run_semantic_on(
            dir,
            Route::Critical,
            policy.clone(),
            semantic_cfg(crate::config::SemanticScope::All),
            &rubrics,
        );
        assert_eq!(
            status_of(&critical, "semantic_quality"),
            GateStatus::Skip,
            "{}",
            crate::gate::render(&critical)
        );
        assert!(
            semantic_rules(&critical).contains(&"judge_disagreement".to_string()),
            "{:?}",
            semantic_rules(&critical)
        );
        let fast = run_semantic_on(
            dir,
            Route::Fast,
            policy,
            semantic_cfg(crate::config::SemanticScope::All),
            &rubrics,
        );
        assert_eq!(status_of(&fast, "semantic_quality"), GateStatus::Pass);
        assert!(
            semantic_rules(&fast).contains(&"judge_disagreement".to_string()),
            "на Fast находка видна предупреждением: {:?}",
            semantic_rules(&fast)
        );
    }

    /// E4.5: записанное решение архитектора (`accept`) снимает эскалацию
    /// оговорки судьи на Critical. Находка остаётся видимой, но вердикт
    /// составляющей — PASS: спорное уже разобрано человеком.
    #[test]
    fn human_decision_accept_resolves_escalation() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path();
        make_semantic_repo(dir);
        let rubrics = semantic_rubrics_dir(dir);
        write_semantic_report(
            dir,
            "docs/adr/ADR-001-reshenie.md",
            1,
            &["accusation_unconfirmed"],
            4.0,
            None,
        );
        write_passing_qualification(dir, &rubrics);
        let policy = crate::config::DecisionPolicyConfig::default();
        let before = run_semantic_on(
            dir,
            Route::Critical,
            policy.clone(),
            semantic_cfg(crate::config::SemanticScope::All),
            &rubrics,
        );
        assert_eq!(
            status_of(&before, "semantic_quality"),
            GateStatus::Skip,
            "без решения архитектора оговорка блокирует: {}",
            crate::gate::render(&before)
        );
        // Решение архитектора: принято.
        let path = dir
            .join(crate::rubric::RUBRIC_REPORTS_DIR)
            .join("semantic.json");
        let text = std::fs::read_to_string(&path).expect("отчёт");
        let artifact: crate::rubric::RubricArtifact =
            serde_json::from_str(&text).expect("JSON отчёта");
        let slug = crate::judge::artifact_slug_of(&artifact);
        let record = crate::rubric::HumanDecision::new(
            &artifact,
            "reports/rubric/semantic.json",
            &crate::hash::sha256_hex(text.as_bytes()),
            crate::rubric::HumanVerdict::Accept,
            "Архитектор <arch@bank>",
            "риск принят осознанно",
        );
        crate::rubric::write_decision(dir, &slug, &record).expect("решение");
        let after = run_semantic_on(
            dir,
            Route::Critical,
            policy,
            semantic_cfg(crate::config::SemanticScope::All),
            &rubrics,
        );
        assert_eq!(
            status_of(&after, "semantic_quality"),
            GateStatus::Pass,
            "решение архитектора снимает блок: {}",
            crate::gate::render(&after)
        );
        assert!(
            semantic_rules(&after).contains(&"human_decision_accepted".to_string()),
            "{:?}",
            semantic_rules(&after)
        );
    }

    /// E4.3: находки судьи доезжают до SARIF — с идентификатором правила и
    /// цитатами, которыми подтверждено обвинение. Интерфейс ревью кода читает
    /// машинный формат, а не markdown отчёта, и без этого «находки судьи в
    /// ревью» остались бы только словами.
    #[test]
    fn semantic_findings_reach_sarif_with_quotes() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path();
        make_semantic_repo(dir);
        let rubrics = semantic_rubrics_dir(dir);
        write_semantic_report(dir, "docs/adr/ADR-001-reshenie.md", 1, &[], 2.0, None);
        let report = run_semantic(
            dir,
            None,
            semantic_cfg(crate::config::SemanticScope::All),
            &rubrics,
        );
        let fmt = crate::report_fmt::FmtReport::from_gate(&report);
        let sarif = crate::report_fmt::render(crate::report_fmt::ReportFormat::Sarif, &fmt);
        assert!(
            sarif.contains("semantic_contradiction"),
            "правило в SARIF: {sarif}"
        );
        assert!(
            sarif.contains("Цитата subject") && sarif.contains("Цитата reference"),
            "цитаты обеих ролей в SARIF: {sarif}"
        );
        assert!(
            sarif.contains("docs/adr/ADR-001-reshenie.md"),
            "файл-субъект в SARIF: {sarif}"
        );
    }

    /// E4.2: «главный критерий низкий, но цитаты не подтверждены» на Critical —
    /// решение человека (SKIP → INCOMPLETE), а на Fast — предупреждение с
    /// вердиктом PASS. Та же механика, разная политика маршрута.
    #[test]
    fn human_policy_follows_route_for_unconfirmed_accusation() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path();
        make_semantic_repo(dir);
        let rubrics = semantic_rubrics_dir(dir);
        write_semantic_report(
            dir,
            "docs/adr/ADR-001-reshenie.md",
            1,
            &["accusation_unconfirmed"],
            4.0,
            None,
        );
        write_passing_qualification(dir, &rubrics);
        let policy = crate::config::DecisionPolicyConfig::default();
        // Critical: блок до решения архитектора.
        let critical = run_semantic_on(
            dir,
            Route::Critical,
            policy.clone(),
            semantic_cfg(crate::config::SemanticScope::All),
            &rubrics,
        );
        assert_eq!(
            status_of(&critical, "semantic_quality"),
            GateStatus::Skip,
            "{}",
            crate::gate::render(&critical)
        );
        assert_eq!(
            critical.outcome,
            GateOutcome::Incomplete,
            "{}",
            crate::gate::render(&critical)
        );
        // Fast: предупреждение, вердикт не меняется.
        let fast = run_semantic_on(
            dir,
            Route::Fast,
            policy,
            semantic_cfg(crate::config::SemanticScope::All),
            &rubrics,
        );
        assert_eq!(status_of(&fast, "semantic_quality"), GateStatus::Pass);
        assert_eq!(fast.outcome, GateOutcome::Pass);
    }

    /// E4.2: политику можно переопределить в конфиге — `critical = "warn"`
    /// снимает блокировку, и то же состояние даёт PASS с предупреждением.
    #[test]
    fn human_policy_is_configurable_in_config() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path();
        make_semantic_repo(dir);
        let rubrics = semantic_rubrics_dir(dir);
        write_semantic_report(
            dir,
            "docs/adr/ADR-001-reshenie.md",
            1,
            &["accusation_unconfirmed"],
            4.0,
            None,
        );
        let policy = crate::config::DecisionPolicyConfig {
            critical: crate::config::HumanPolicy::Warn,
            ..crate::config::DecisionPolicyConfig::default()
        };
        let report = run_semantic_on(
            dir,
            Route::Critical,
            policy,
            semantic_cfg(crate::config::SemanticScope::All),
            &rubrics,
        );
        assert_eq!(
            status_of(&report, "semantic_quality"),
            GateStatus::Pass,
            "политика проекта сильнее дефолта: {}",
            crate::gate::render(&report)
        );
        // Находка остаётся видимой, но предупреждением, а не блоком: итог
        // гейта здесь определяют другие обязательные составляющие Critical.
        let comp = report
            .components
            .iter()
            .find(|c| c.name == "semantic_quality")
            .expect("comp");
        assert!(
            comp.findings.iter().any(|f| f.rule.as_deref()
                == Some("semantic_accusation_unconfirmed")
                && f.severity == "warn"),
            "{:?}",
            comp.findings
        );
    }

    /// E3.2 (смысловая составляющая, путь досье): доля сэмплов судьи с баллом
    /// вне шкалы выше порога — SKIP с находкой `semantic_invalid_samples` и
    /// вердикт INCOMPLETE. Тот же контур, что у `decision_quality`, но по досье.
    #[test]
    fn semantic_invalid_samples_above_threshold_escalates() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path();
        make_semantic_repo(dir);
        let rubrics = semantic_rubrics_dir(dir);
        write_semantic_report(dir, "docs/adr/ADR-001-reshenie.md", 5, &[], 4.6, None);
        let path = dir
            .join(crate::rubric::RUBRIC_REPORTS_DIR)
            .join("semantic.json");
        let mut artifact: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).expect("отчёт")).expect("JSON");
        artifact["invalid_samples_ratio"] = serde_json::json!(0.75);
        std::fs::write(
            &path,
            serde_json::to_string_pretty(&artifact).expect("json"),
        )
        .expect("write report");
        let report = run_semantic(
            dir,
            None,
            semantic_cfg(crate::config::SemanticScope::All),
            &rubrics,
        );
        assert_eq!(
            status_of(&report, "semantic_quality"),
            GateStatus::Skip,
            "суждению верить нельзя: {}",
            crate::gate::render(&report)
        );
        assert!(
            semantic_rules(&report).contains(&"semantic_invalid_samples".to_string()),
            "{:?}",
            semantic_rules(&report)
        );
        assert_eq!(
            report.outcome,
            GateOutcome::Incomplete,
            "{}",
            crate::gate::render(&report)
        );
    }

    /// E2: вход с prompt-инъекцией — «проверить нельзя, нужен человек».
    /// Составляющая уходит в SKIP с находкой `semantic_input_injection`, и
    /// вердикт гейта становится INCOMPLETE (exit 3): молчаливым PASS такой
    /// вход не проходит, а FAIL не объявляется — субъект ничего не нарушил.
    #[test]
    fn semantic_input_injection_makes_gate_incomplete() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path();
        make_semantic_repo(dir);
        let rubrics = semantic_rubrics_dir(dir);
        write_semantic_report(dir, "docs/adr/ADR-001-reshenie.md", 5, &[], 4.6, None);
        // Пометка входа (E2.1) — поле новой схемы отчёта.
        let path = dir
            .join(crate::rubric::RUBRIC_REPORTS_DIR)
            .join("semantic.json");
        let mut artifact: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).expect("отчёт")).expect("JSON");
        artifact["input_injection_lines"] = serde_json::json!([2]);
        std::fs::write(
            &path,
            serde_json::to_string_pretty(&artifact).expect("json"),
        )
        .expect("write report");
        let report = run_semantic(
            dir,
            None,
            semantic_cfg(crate::config::SemanticScope::All),
            &rubrics,
        );
        assert_eq!(
            status_of(&report, "semantic_quality"),
            GateStatus::Skip,
            "суждение по такому входу не подтверждается: {}",
            crate::gate::render(&report)
        );
        assert!(
            semantic_rules(&report).contains(&"semantic_input_injection".to_string()),
            "{:?}",
            semantic_rules(&report)
        );
        // Счётчик называет именно инъекцию, а не «качество ответа судьи»:
        // это разные причины неподтверждаемости, и решение человека по ним
        // принимается по-разному.
        let comp = report
            .components
            .iter()
            .find(|c| c.name == "semantic_quality")
            .expect("составляющая");
        assert!(
            comp.detail.contains("вход с инъекцией: 1"),
            "{}",
            comp.detail
        );
        assert!(
            comp.detail.contains("качество ответа судьи: 0"),
            "{}",
            comp.detail
        );
        assert_eq!(
            report.outcome,
            GateOutcome::Incomplete,
            "{}",
            crate::gate::render(&report)
        );
    }

    /// Потолок числа субъектов в пакете: ровно потолок — без пометки о
    /// срезе, потолок + 1 — с пометкой (сравнение строгое).
    #[test]
    fn semantic_subject_cap_marks_truncation_exactly_above_limit() {
        let make = |count: usize| {
            let tmp = tempfile::tempdir().expect("tmp");
            let dir = tmp.path();
            make_semantic_repo(dir);
            for k in 1..=count {
                std::fs::write(
                    dir.join(format!("docs/adr/ADR-{k:03}-reshenie.md")),
                    format!(
                        "# ADR-{k:03}

- Status: Accepted
"
                    ),
                )
                .expect("adr");
            }
            let rubrics = semantic_rubrics_dir(dir);
            let report = run_semantic(
                dir,
                None,
                semantic_cfg(crate::config::SemanticScope::All),
                &rubrics,
            );
            let comp = report
                .components
                .iter()
                .find(|c| c.name == "semantic_quality")
                .expect("составляющая");
            comp.detail.clone()
        };
        let at_limit = make(MAX_SEMANTIC_SUBJECTS);
        assert!(
            !at_limit.contains("больше потолка"),
            "на потолке среза нет: {at_limit}"
        );
        let over_limit = make(MAX_SEMANTIC_SUBJECTS + 1);
        assert!(
            over_limit.contains("больше потолка"),
            "сверх потолка — пометка о срезе: {over_limit}"
        );
    }

    /// Метка области субъектов — ровно `changed`/`all`: пустая строка или
    /// заглушка в вердикте назвали бы область неверно.
    #[test]
    fn scope_label_names_both_scopes() {
        assert_eq!(
            scope_label(crate::config::SemanticScope::Changed),
            "changed"
        );
        assert_eq!(scope_label(crate::config::SemanticScope::All), "all");
    }

    /// Принятые ADR — только `ADR-*.md` со статусом Accepted: прочий markdown
    /// в каталоге ADR и непринятые решения в выборку не попадают.
    #[test]
    fn accepted_adr_paths_select_only_accepted_adr_markdown() {
        let tmp = tempfile::tempdir().expect("tmp");
        let repo = tmp.path();
        let adr_dir = repo.join("docs/adr");
        std::fs::create_dir_all(&adr_dir).expect("mkdir");
        std::fs::write(
            adr_dir.join("ADR-001-accepted.md"),
            "# ADR-001\n\n- Status: Accepted\n\nРешение.\n",
        )
        .expect("accepted");
        std::fs::write(
            adr_dir.join("ADR-002-proposed.md"),
            "# ADR-002\n\n- Status: Proposed\n\nЧерновик.\n",
        )
        .expect("proposed");
        std::fs::write(
            adr_dir.join("notes.md"),
            "# Заметки\n\n- Status: Accepted\n\nНе ADR.\n",
        )
        .expect("notes");
        std::fs::write(adr_dir.join("readme.txt"), "Status: Accepted\n").expect("txt");
        let found = accepted_adr_paths(repo);
        assert_eq!(
            found,
            vec!["docs/adr/ADR-001-accepted.md".to_string()],
            "{found:?}"
        );
    }

    /// Субъекты смысловой рубрики — только сущности со связями; для
    /// `nfr_mechanism` дополнительно только сами NFR.
    #[test]
    fn linked_entities_require_links_and_filter_nfr_by_pack() {
        let tmp = tempfile::tempdir().expect("tmp");
        let repo = tmp.path();
        let model = repo.join("model");
        std::fs::create_dir_all(&model).expect("mkdir model");
        // Связная CMP и связный NFR.
        std::fs::write(
            model.join("CMP-001.md"),
            "---\nid: CMP-001\ntype: cmp\ntitle: \"Шлюз\"\nstatus: designed\ndepends_on: [INT-001]\n---\n\nТело.\n",
        )
        .expect("cmp");
        std::fs::write(
            model.join("NFR-001.md"),
            "---\nid: NFR-001\ntype: nfr\ntitle: \"Доступность\"\nstatus: accepted\nverified_by: [C-001]\n---\n\nТело.\n",
        )
        .expect("nfr");
        // Сущность без связей — субъектом не становится.
        std::fs::write(
            model.join("CMP-002.md"),
            "---\nid: CMP-002\ntype: cmp\ntitle: \"Сид\"\nstatus: designed\n---\n\nТело.\n",
        )
        .expect("orphan");
        let all = linked_entities(repo, crate::rubric_pack::PackKind::CodeVsSpine);
        assert!(all.contains(&"CMP-001".to_string()), "{all:?}");
        assert!(all.contains(&"NFR-001".to_string()), "{all:?}");
        assert!(
            !all.contains(&"CMP-002".to_string()),
            "связная сущность без связей не субъект: {all:?}"
        );
        let nfr_only = linked_entities(repo, crate::rubric_pack::PackKind::NfrMechanism);
        assert_eq!(nfr_only, vec!["NFR-001".to_string()], "{nfr_only:?}");
        // Модели нет — пустой список, а не паника.
        assert!(
            linked_entities(
                &repo.join("nope"),
                crate::rubric_pack::PackKind::CodeVsSpine
            )
            .is_empty()
        );
    }

    /// Доля невалидных сэмплов ровно на пороге — предупреждение, а не
    /// эскалация: строгое «больше порога» отделяет шум от сломанного судьи.
    #[test]
    fn semantic_invalid_ratio_at_threshold_is_warning_not_escalation() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path();
        make_semantic_repo(dir);
        let rubrics = semantic_rubrics_dir(dir);
        write_semantic_report(dir, "docs/adr/ADR-001-reshenie.md", 5, &[], 4.6, None);
        let path = dir
            .join(crate::rubric::RUBRIC_REPORTS_DIR)
            .join("semantic.json");
        let mut artifact: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).expect("отчёт")).expect("JSON");
        artifact["invalid_samples_ratio"] = serde_json::json!(0.5);
        std::fs::write(
            &path,
            serde_json::to_string_pretty(&artifact).expect("json"),
        )
        .expect("write report");
        let report = run_semantic(
            dir,
            None,
            semantic_cfg(crate::config::SemanticScope::All),
            &rubrics,
        );
        assert_ne!(
            status_of(&report, "semantic_quality"),
            GateStatus::Skip,
            "на пороге решение механике ещё подтверждаемо: {}",
            crate::gate::render(&report)
        );
        let invalid: Vec<&crate::gate::GateFinding> = report
            .components
            .iter()
            .find(|c| c.name == "semantic_quality")
            .expect("составляющая")
            .findings
            .iter()
            .filter(|f| f.rule.as_deref() == Some("semantic_invalid_samples"))
            .collect();
        assert!(
            !invalid.is_empty(),
            "предупреждение о невалидных сэмплах обязано быть: {}",
            crate::gate::render(&report)
        );
        assert!(
            invalid.iter().all(|f| f.severity == "warn"),
            "{}",
            crate::gate::render(&report)
        );
    }

    /// Допуск судьи включается проектом: при `require_qualified_judge = false`
    /// неквалифицированный судья на блокирующем маршруте не останавливает
    /// вердикт (проверка включается осознанно, E6.3).
    #[test]
    fn semantic_qualification_is_not_required_when_flag_is_off() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path();
        make_semantic_repo(dir);
        let rubrics = semantic_rubrics_dir(dir);
        write_semantic_report(dir, "docs/adr/ADR-001-reshenie.md", 5, &[], 4.6, None);
        let mut cfg = semantic_cfg(crate::config::SemanticScope::All);
        cfg.require_qualified_judge = false;
        let report = run_semantic_on(
            dir,
            Route::Critical,
            crate::config::DecisionPolicyConfig::default(),
            cfg,
            &rubrics,
        );
        assert!(
            !semantic_rules(&report).contains(&"judge_unqualified".to_string()),
            "флаг выключен — допуск не требуется: {:?}",
            semantic_rules(&report)
        );
    }

    /// На неблокирующем маршруте (политика без человека) допуск судьи не
    /// спрашивается даже при включённом флаге: останавливать нечего.
    #[test]
    fn semantic_qualification_is_not_checked_under_warn_policy() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path();
        make_semantic_repo(dir);
        let rubrics = semantic_rubrics_dir(dir);
        write_semantic_report(dir, "docs/adr/ADR-001-reshenie.md", 5, &[], 4.6, None);
        let report = run_semantic(
            dir,
            None,
            semantic_cfg(crate::config::SemanticScope::All),
            &rubrics,
        );
        assert!(
            !semantic_rules(&report).contains(&"judge_unqualified".to_string()),
            "на Fast политика без человека: {:?}",
            semantic_rules(&report)
        );
    }

    /// Отчёт судьи ищется по паре «рубрика + субъект»: отчёт другой рубрики о
    /// том же документе не подменяет суждение (иначе вердикт шёл бы по чужой
    /// шкале критериев).
    #[test]
    fn semantic_artifact_lookup_requires_rubric_and_subject() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path();
        make_semantic_repo(dir);
        let rubrics = semantic_rubrics_dir(dir);
        write_semantic_report(dir, "docs/adr/ADR-001-reshenie.md", 5, &[], 4.6, None);
        let path = dir
            .join(crate::rubric::RUBRIC_REPORTS_DIR)
            .join("semantic.json");
        let mut artifact: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).expect("отчёт")).expect("JSON");
        artifact["rubric"] = serde_json::json!("adr_quality");
        std::fs::write(
            &path,
            serde_json::to_string_pretty(&artifact).expect("json"),
        )
        .expect("write report");
        let report = run_semantic(
            dir,
            None,
            semantic_cfg(crate::config::SemanticScope::All),
            &rubrics,
        );
        assert!(
            semantic_rules(&report).contains(&"semantic_report_missing".to_string()),
            "отчёт чужой рубрики не считается отчётом по нашей: {:?}",
            semantic_rules(&report)
        );
    }

    /// Итог ровно на пороге — не «ниже порога»: сравнение строгое.
    #[test]
    fn semantic_total_exactly_at_threshold_is_not_low() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path();
        make_semantic_repo(dir);
        let rubrics = semantic_rubrics_dir(dir);
        write_semantic_report(dir, "docs/adr/ADR-001-reshenie.md", 4, &[], 3.5, None);
        let report = run_semantic(
            dir,
            None,
            semantic_cfg(crate::config::SemanticScope::All),
            &rubrics,
        );
        assert!(
            !semantic_rules(&report).contains(&"semantic_quality_low".to_string()),
            "на пороге итог не «низкий»: {:?}",
            semantic_rules(&report)
        );
    }

    /// Судья, совпавший с автором, назван: это отдельная находка, а не тишина.
    #[test]
    fn semantic_judge_is_author_is_reported_for_named_author() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path();
        make_semantic_repo(dir);
        let rubrics = semantic_rubrics_dir(dir);
        write_semantic_report(dir, "docs/adr/ADR-001-reshenie.md", 5, &[], 4.6, None);
        let path = dir
            .join(crate::rubric::RUBRIC_REPORTS_DIR)
            .join("semantic.json");
        let mut artifact: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).expect("отчёт")).expect("JSON");
        artifact["author_model"] = artifact["judge_model"].clone();
        std::fs::write(
            &path,
            serde_json::to_string_pretty(&artifact).expect("json"),
        )
        .expect("write report");
        let report = run_semantic(
            dir,
            None,
            semantic_cfg(crate::config::SemanticScope::All),
            &rubrics,
        );
        assert!(
            semantic_rules(&report).contains(&"judge_is_author".to_string()),
            "автор = судья обязан быть назван: {:?}",
            semantic_rules(&report)
        );
    }

    /// Солюшен-документы собираются рекурсивно из `docs/solution`, и только
    /// markdown: это субъекты рубрики «решение против стандартов».
    #[test]
    fn semantic_solution_docs_are_collected_recursively() {
        let tmp = tempfile::tempdir().expect("tmp");
        let repo = tmp.path();
        std::fs::create_dir_all(repo.join("docs/solution/nested")).expect("mkdir");
        std::fs::write(repo.join("docs/solution/top.md"), "# Решение\n").expect("md");
        std::fs::write(repo.join("docs/solution/nested/deep.md"), "# Вложенное\n").expect("md");
        std::fs::write(repo.join("docs/solution/notes.txt"), "не markdown\n").expect("txt");
        let found = solution_docs(repo);
        assert_eq!(
            found,
            vec![
                "docs/solution/nested/deep.md".to_string(),
                "docs/solution/top.md".to_string()
            ],
            "{found:?}"
        );
    }

    /// Файлы кода — из корней `code_roots` модели, со пропуском служебных и
    /// скрытых каталогов: `target/` и `.git/` не субъекты смыслового ревью.
    #[test]
    fn semantic_code_files_follow_roots_and_skip_service_dirs() {
        let tmp = tempfile::tempdir().expect("tmp");
        let repo = tmp.path();
        write_model(repo, &[("CMP-001", "code_roots: [src]\ndepends_on: []")]);
        std::fs::create_dir_all(repo.join("src/target")).expect("mkdir target");
        std::fs::create_dir_all(repo.join("src/.hidden")).expect("mkdir hidden");
        std::fs::create_dir_all(repo.join("src/node_modules")).expect("mkdir modules");
        std::fs::write(repo.join("src/main.rs"), "fn main() {}\n").expect("main");
        std::fs::write(repo.join("src/target/build.rs"), "// сборка\n").expect("target");
        std::fs::write(repo.join("src/.hidden/secret.rs"), "// скрытое\n").expect("hidden");
        std::fs::write(repo.join("src/node_modules/dep.rs"), "// зависимость\n").expect("dep");
        let found = code_files(repo);
        assert_eq!(found, vec!["src/main.rs".to_string()], "{found:?}");
    }
}
