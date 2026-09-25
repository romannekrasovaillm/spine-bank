//! Составляющая `semantic_quality` (B1, ADR-052): смысловые рубрики по
//! отчётам судьи поверх досье [`crate::rubric_pack`].

use std::fmt::Write as _;
use std::path::Path;

use super::components::adr_is_accepted;
use super::git::GitProbe;
use super::types::{GateComponent, GateFinding};
use crate::control::Route;
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
    route: Option<Route>,
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
            let state =
                semantic_subject_state(repo, &rubric, kind, &subject, &artifacts, cfg, route);
            checked += usize::from(!matches!(state, SemanticState::Skipped));
            match &state {
                SemanticState::InjectionSuspected(_) => injection_suspected += 1,
                SemanticState::InvalidSamples(_) | SemanticState::PartialOnCritical(_) => {
                    unconfirmed += 1;
                }
                _ => {}
            }
            findings.extend(state.into_findings(name, &subject));
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
    /// Часть свидетельств судьи не подтвердилась, а маршрут — Critical (E3.3):
    /// оговорку принимает человек.
    PartialOnCritical(String),
    /// Всё в порядке.
    Ok,
}

impl SemanticState {
    /// Находки состояния; пусто — состояние не оставляет следа в отчёте.
    fn into_findings(self, rubric: &str, subject: &SemanticSubject) -> Vec<GateFinding> {
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
                "warn".to_string(),
                "semantic_accusation_unconfirmed".to_string(),
                format!(
                    "{who}: главный критерий низкий, но цитаты не подтверждены ({detail}) — \
                     критерий исключён из итога, гейт этим не краснеет"
                ),
            )],
            Self::CoverageIncomplete(criteria) => vec![GateFinding::ruled(
                "warn".to_string(),
                "semantic_coverage_incomplete".to_string(),
                format!(
                    "{who}: высокий балл без полного перечня проверенных источников ({}) — \
                     критерии исключены из итога",
                    criteria.join(", ")
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
            Self::PartialOnCritical(criteria) => vec![GateFinding::ruled(
                "error".to_string(),
                "semantic_evidence_partial".to_string(),
                format!(
                    "{who}: часть свидетельств судьи не подтвердилась ({criteria}), а маршрут \
                     Critical — решение за человеком"
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
    route: Option<Route>,
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
    // E3.3: оговорка судьи (`evidence_partial`) на Critical — человеку.
    if route == Some(Route::Critical) {
        let partial: Vec<String> = artifact
            .scores
            .iter()
            .filter(|s| s.has_flag(crate::rubric::CriterionFlag::EvidencePartial))
            .map(|s| s.criterion_id.clone())
            .collect();
        if !partial.is_empty() {
            return SemanticState::PartialOnCritical(partial.join(", "));
        }
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
                    state = SemanticState::Unconfirmed(format!("метки: {:?}", score.flags));
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
            state = SemanticState::CoverageIncomplete(uncovered);
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
        assert_eq!(
            report.outcome,
            GateOutcome::Incomplete,
            "{}",
            crate::gate::render(&report)
        );
    }
}
