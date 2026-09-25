//! Машиночитаемые отчёты рубрик (B1): `reports/rubric/<slug>.json` — запись
//! с происхождением оценки (ADR-048) и субъектом досье (ADR-051), slug'и
//! файлов, загрузка всех отчётов репозитория.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{HarnessError, Result};

use super::report::RubricReport;
use super::types::{CriterionFlag, CriterionScore, JudgeConfigSnapshot};

/// Корень репозитория для целевого документа: ближайший вверх каталог с
/// `.git` или `.arch-handoff`; не найден — каталог самого документа.
///
/// Отчёт обязан лечь туда, откуда его найдёт гейт (`<repo>/reports/rubric/`),
/// а не рядом с документом: `docs/adr/reports/...` гейт не читает.
#[must_use]
pub fn repo_root_of(target: &Path) -> PathBuf {
    let start = if target.is_dir() {
        target.to_path_buf()
    } else {
        target
            .parent()
            .map_or_else(|| PathBuf::from("."), Path::to_path_buf)
    };
    let mut cur: Option<&Path> = Some(&start);
    while let Some(dir) = cur {
        if dir.join(".git").exists() || dir.join(".arch-handoff").is_dir() {
            return dir.to_path_buf();
        }
        cur = dir.parent();
    }
    start
}

/// Каталог машиночитаемых отчётов рубрики внутри репозитория (Н7, ADR-042).
pub const RUBRIC_REPORTS_DIR: &str = "reports/rubric";

/// Схема файла отчёта рубрики.
pub const RUBRIC_REPORT_SCHEMA: &str = "arch-be/rubric-report/v1";

/// Машиночитаемый отчёт рубрики (`reports/rubric/<slug>.json`).
///
/// Зачем файл, а не только вывод команды: оценка качества решения должна
/// переживать сессию и попадать в гейт (составляющая `decision_quality`), не
/// добавляя LLM в ядро. Отчёт привязывает балл к СОДЕРЖИМОМУ документа
/// (`target_sha256`) — правка ADR после оценки обесценивает отчёт
/// (`rubric_report_stale`), а не «переносится» на новую редакцию.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RubricArtifact {
    /// Схема файла.
    pub schema: String,
    /// Имя рубрики.
    pub rubric: String,
    /// Путь оценённого документа относительно репозитория (`None` — текст
    /// без файла: документ не адресуем, гейт его не найдёт).
    #[serde(default)]
    pub target: Option<String>,
    /// SHA-256 содержимого на момент оценки.
    #[serde(default)]
    pub target_sha256: Option<String>,
    /// Модель-судья.
    pub judge_model: String,
    /// Модель-автор документа: `judge_model == author_model` — судья судил
    /// свою же работу (метка `judge_is_author`).
    #[serde(default)]
    pub author_model: Option<String>,
    /// Взвешенный итог `0..=5`.
    pub weighted_total: f64,
    /// Вердикт судьи.
    pub verdict: String,
    /// Разброс сэмплов выше порога (`unstable`).
    #[serde(default)]
    pub unstable: bool,
    /// Число критериев с `evidence_not_found`.
    #[serde(default)]
    pub evidence_not_found: usize,
    /// Правила, по которым собран отчёт (J8): число сэмплов и пороги судьи.
    /// По ним видно, ЧЕМ отчёт отличался бы при другом конфиге, и ими же
    /// пользуется пересборка при сверке. У старых отчётов поля нет.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub judge_config: Option<JudgeConfigSnapshot>,
    /// Уровень независимости судьи (ADR-048): `none`, `declared`,
    /// `declared_cross_family`, `launched`, `launched_cross_family`. Считается
    /// по тому, что механика знает: метки, режим оценки, семейства моделей.
    /// У отчётов до появления поля отсутствует — порог независимости на них
    /// не действует (поведение 0.3.4).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub independence: Option<String>,
    /// Откуда взята метка автора: `header` (поле в шапке документа),
    /// `argument` (аргумент вызова) или `none` (автор не указан) — J3, ADR-048.
    /// У отчётов до появления поля отсутствует; тогда, как и раньше, о метке
    /// известно только её значение.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author_source: Option<String>,
    /// Метка автора, переданная вызовом, если она разошлась с шапкой
    /// документа (J3, ADR-048): в отчёт идёт значение из шапки, а расхождение
    /// называется находкой `author_model_mismatch`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author_model_declared: Option<String>,
    /// Происхождение оценки: как получен отчёт, что из этого удостоверено
    /// механикой и что заявлено (ADR-048). Отсутствует у отчётов до появления
    /// блока — это читается как режим `declared` без деталей.
    #[serde(default)]
    pub provenance: Option<crate::judge::RubricProvenance>,
    /// Вид досье смысловой рубрики (`adr_vs_spine`, …); `None` — отчёт о
    /// документе, а не о досье (ADR-051).
    #[serde(default)]
    pub pack_kind: Option<String>,
    /// Субъект досье: путь или идентификатор; для фрагмента кода — с
    /// диапазоном строк (`src/gate.rs#12-88`).
    #[serde(default)]
    pub subject: Option<String>,
    /// SHA-256 текста собранного досье — привязка отчёта ко ВСЕМ источникам
    /// сразу, а не только к субъекту (ADR-051, П3).
    #[serde(default)]
    pub pack_sha256: Option<String>,
    /// Источники досье с их хэшами: правка любого из них — отчёт устарел.
    #[serde(default)]
    pub inputs: Vec<crate::rubric_pack::PackInput>,
    /// Оценки по критериям — то, по чему гейт (`semantic_quality`) и
    /// `redteam semantic-score` решают про обвинение: агрегата
    /// `weighted_total` для этого мало, а главный критерий нужен поимённо.
    /// Оно же — свидетельство для сверки воспроизводимости отчёта с сырыми
    /// ответами судьи (J2, ADR-048): балл, правленный руками, расходится с
    /// пересборкой. Поле аддитивное: отчёты до 0.3.5 читаются (отсутствие =
    /// пусто, сверка тогда идёт по итогу и меткам).
    #[serde(default)]
    pub scores: Vec<CriterionScore>,
    /// Доля сэмплов судьи с баллом вне шкалы (E3.2) — по ней гейт решает,
    /// отправлять ли решение человеку. Поле аддитивное: отсутствие = 0.
    #[serde(default)]
    pub invalid_samples_ratio: f64,
    /// Единое решение рубрики (E4.1): `pass` / `fail` / `human`. Гейт и CI
    /// читают именно его, а не пересчитывают метки заново. Поле аддитивное:
    /// у отчётов до 0.3.9 его нет (читатель решает сам).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision: Option<crate::rubric::RubricDecision>,
    /// Почему решение такое (E4.4): причины идут в пакет для архитектора.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub decision_reasons: Vec<String>,
    /// Строки входа с паттернами prompt-инъекций (E2). `None` — отчёт записан
    /// до появления детектора (сверка его не штрафует), `Some([])` — вход
    /// сканировали и он чист, `Some([n, …])` — помеченные строки.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_injection_lines: Option<Vec<usize>>,
    /// Метка времени оценки (RFC 3339).
    pub judged_at: String,
}

impl RubricArtifact {
    /// Режим происхождения оценки: у отчёта без блока `provenance` — `declared`
    /// без деталей (оценку собрал хост, но чем именно — отчёт не говорит).
    #[must_use]
    pub fn provenance_mode(&self) -> &str {
        self.provenance.as_ref().map_or(
            crate::judge::MODE_DECLARED,
            crate::judge::RubricProvenance::mode,
        )
    }
}

/// Slug имени файла отчёта: путь документа, обезвреженный до имени файла
/// (`docs/adr/ADR-041-….md` → `ADR-041-…`); пусто — `rubric`.
#[must_use]
pub fn artifact_slug(target: Option<&Path>) -> String {
    let raw = target
        .and_then(|p| p.file_stem())
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let slug: String = raw
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    if slug.trim_matches('-').is_empty() {
        "rubric".to_string()
    } else {
        slug
    }
}

/// Дополнительные сведения отчёта, которых нет в [`RubricReport`]:
/// происхождение оценки (ADR-048). Отдельная структура, а не новые аргументы
/// [`write_artifact`], — вызывающие без происхождения не переписываются.
#[derive(Debug, Clone, Default)]
pub struct ArtifactExtras {
    /// Происхождение оценки: режим, хост, сессия, запускатель, отпечатки
    /// сырых ответов, оператор.
    pub provenance: Option<crate::judge::RubricProvenance>,
    /// Откуда взята метка автора (J3): `header` | `argument` | `none`.
    pub author_source: Option<String>,
    /// Метка автора, переданная вызовом, если она разошлась с шапкой (J3).
    pub author_model_declared: Option<String>,
    /// Семейства моделей (секция `[judge.families]`): по ним считается уровень
    /// независимости (J5). Пусто — работают дефолтные префиксы.
    pub families: BTreeMap<String, String>,
    /// Правила сборки отчёта (`[judge]` на момент оценки) — J8.
    pub judge_config: Option<JudgeConfigSnapshot>,
    /// Сырые ответы судьи: сохраняются рядом с отчётом
    /// (`reports/rubric/raw/<slug>/sample-<n>.json`), их хэши идут в
    /// `provenance.samples` (J2, ADR-048). Пусто — ответы не сохранены.
    pub raw_answers: Vec<crate::judge::RawAnswerInput>,
}

/// Записывает отчёт рубрики в `<repo>/reports/rubric/<slug>.json` без
/// происхождения (поведение до ADR-048).
///
/// # Errors
/// Каталог отчётов не создаётся или файл не пишется.
pub fn write_artifact(
    repo: &Path,
    report: &RubricReport,
    target: Option<&Path>,
    author_model: Option<&str>,
) -> Result<PathBuf> {
    write_artifact_for_subject(repo, report, &ArtifactSubject::Target(target), author_model)
}

/// Записывает отчёт рубрики вместе с происхождением оценки (J1–J5, ADR-048).
///
/// # Errors
/// Каталог отчётов не создаётся или файл не пишется.
pub fn write_artifact_with(
    repo: &Path,
    report: &RubricReport,
    target: Option<&Path>,
    author_model: Option<&str>,
    extras: &ArtifactExtras,
) -> Result<PathBuf> {
    write_artifact_for_subject_with(
        repo,
        report,
        &ArtifactSubject::Target(target),
        author_model,
        extras,
    )
}

/// Содержимое отчёта, которое записал бы [`write_artifact_with`], — БЕЗ записи
/// (J7): контур только для чтения не оставляет следов в рабочем каталоге, но
/// возвращает хосту готовый файл, чтобы тот сохранил его своими средствами и
/// гейт увидел отчёт.
///
/// Сырые ответы судьи при этом НЕ сохраняются: read-only контур не пишет
/// ничего, а хэши ответов остаются в самом отчёте (`provenance.samples`).
///
/// # Errors
/// Отчёт не сериализуется.
pub fn artifact_json(
    repo: &Path,
    report: &RubricReport,
    target: Option<&Path>,
    author_model: Option<&str>,
    extras: &ArtifactExtras,
) -> Result<(PathBuf, String)> {
    artifact_json_for_subject(
        repo,
        report,
        &ArtifactSubject::Target(target),
        author_model,
        extras,
    )
}

/// Уровень независимости оценки — один расчёт на все входы (J5, ADR-048):
/// запись отчёта и ответ `rubric_verify` берут его отсюда, чтобы уровень в
/// журнале и в файле не мог разойтись.
#[must_use]
pub fn independence_for(
    author_model: Option<&str>,
    judge_model: &str,
    provenance: Option<&crate::judge::RubricProvenance>,
    families: &BTreeMap<String, String>,
) -> String {
    let mode = provenance.map_or(
        crate::judge::MODE_DECLARED,
        crate::judge::RubricProvenance::mode,
    );
    crate::judge::independence_of(author_model, judge_model, mode, families)
}

/// Путь файла относительно корня репозитория в слешевой форме — аттестация не
/// должна зависеть от того, где склонирован репозиторий; файл вне корня
/// остаётся абсолютным.
fn relative_to(repo: &Path, path: &Path) -> String {
    path.strip_prefix(repo).map_or_else(
        |_| path.display().to_string().replace('\\', "/"),
        |r| r.display().to_string().replace('\\', "/"),
    )
}

/// О чём отчёт: о документе (поведение 0.3.4) или о досье смысловой рубрики.
#[derive(Debug, Clone, Copy)]
pub enum ArtifactSubject<'a> {
    /// Документ репозитория; `None` — текст без файла.
    Target(Option<&'a Path>),
    /// Досье: субъект, хэш собранного текста и источники (ADR-051).
    Pack(&'a crate::rubric_pack::ContextPack),
}

/// Записывает отчёт с указанием субъекта — общий путь для документа и досье
/// без происхождения (поведение до ADR-048; тонкая обёртка над
/// [`write_artifact_for_subject_with`]).
///
/// # Errors
/// Каталог отчётов не создаётся или файл не пишется.
pub fn write_artifact_for_subject(
    repo: &Path,
    report: &RubricReport,
    subject: &ArtifactSubject<'_>,
    author_model: Option<&str>,
) -> Result<PathBuf> {
    write_artifact_for_subject_with(
        repo,
        report,
        subject,
        author_model,
        &ArtifactExtras::default(),
    )
}

/// Записывает отчёт с указанием субъекта и происхождением оценки — общий путь
/// для документа и досье (F1, ADR-051: досье пишет сырые ответы и происхождение
/// так же, как документ, иначе `rubric reverify` называет его невоспроизводимым).
///
/// # Errors
/// Каталог отчётов не создаётся или файл не пишется.
pub fn write_artifact_for_subject_with(
    repo: &Path,
    report: &RubricReport,
    subject: &ArtifactSubject<'_>,
    author_model: Option<&str>,
    extras: &ArtifactExtras,
) -> Result<PathBuf> {
    let (path, artifact) = build_artifact(repo, report, subject, author_model, extras, true)?;
    write_artifact_file(repo, &path, &artifact)?;
    Ok(path)
}

/// Содержимое отчёта по указанному субъекту БЕЗ записи (J7) — read-only
/// вариант [`write_artifact_for_subject_with`] для документа и досье: контур
/// не оставляет следов в рабочем каталоге, но возвращает хосту готовый файл.
///
/// Сырые ответы судьи не сохраняются (контур не пишет ничего), а их хэши
/// остаются в отчёте (`provenance.samples`) — по ним видно, из чего он собран.
///
/// # Errors
/// Отчёт не сериализуется.
pub fn artifact_json_for_subject(
    repo: &Path,
    report: &RubricReport,
    subject: &ArtifactSubject<'_>,
    author_model: Option<&str>,
    extras: &ArtifactExtras,
) -> Result<(PathBuf, String)> {
    let (path, artifact) = build_artifact(repo, report, subject, author_model, extras, false)?;
    let text = serde_json::to_string_pretty(&artifact)
        .map_err(|e| HarnessError::Config(format!("сериализация отчёта рубрики: {e}")))?;
    Ok((path, text))
}

/// Кладёт собранный отчёт в `reports/rubric/` — один способ записи на все
/// входы (документ, досье, путь с происхождением), чтобы формат не расходился.
fn write_artifact_file(repo: &Path, path: &Path, artifact: &RubricArtifact) -> Result<()> {
    let dir = repo.join(RUBRIC_REPORTS_DIR);
    std::fs::create_dir_all(&dir).map_err(|e| HarnessError::io(&dir, e))?;
    let text = serde_json::to_string_pretty(artifact)
        .map_err(|e| HarnessError::Config(format!("сериализация отчёта рубрики: {e}")))?;
    std::fs::write(path, text).map_err(|e| HarnessError::io(path, e))?;
    Ok(())
}

/// Собирает артефакт отчёта и путь, по которому он лёг бы. `save_raw` —
/// сохранять ли сырые ответы судьи: при сборке «на возврат» (read-only контур)
/// следов в рабочем каталоге не остаётся.
fn build_artifact(
    repo: &Path,
    report: &RubricReport,
    subject: &ArtifactSubject<'_>,
    author_model: Option<&str>,
    extras: &ArtifactExtras,
    save_raw: bool,
) -> Result<(PathBuf, RubricArtifact)> {
    let (target, sha, file_stem, pack_kind, pack_subject, pack_sha256, inputs) = match subject {
        ArtifactSubject::Target(t) => {
            let target = t.map(|p| relative_to(repo, p));
            let sha = match t {
                Some(p) if p.is_file() => crate::hash::sha256_file(p),
                _ => None,
            };
            (target, sha, artifact_slug(*t), None, None, None, Vec::new())
        }
        ArtifactSubject::Pack(pack) => (
            None,
            None,
            pack_artifact_slug(pack.kind.as_str(), &pack.subject),
            Some(pack.kind.as_str().to_string()),
            Some(pack.subject.clone()),
            Some(pack.sha256.clone()),
            pack.inputs.clone(),
        ),
    };
    // Сырые ответы судьи — рядом с отчётом: отчёт обязан быть воспроизводим из
    // ответов, на которых он объявлен собранным (J2, ADR-048). Их хэши попадают
    // в происхождение независимо от записи файлов: в read-only контуре
    // (save_raw == false, J7) файлов нет, но отчёт, который хост сохранит
    // своими средствами, обязан нести связь с ответами (F1). У досье роль
    // «документа» играет субъект досье с его хэшем — сверка та же.
    let mut provenance = extras.provenance.clone();
    if !extras.raw_answers.is_empty() {
        let stamps = if save_raw {
            crate::judge::write_raw_answers(
                repo,
                &file_stem,
                &report.rubric_name,
                pack_subject.as_deref().or(target.as_deref()),
                pack_sha256.as_deref().or(sha.as_deref()),
                &report.judge_model,
                &extras.raw_answers,
            )?
        } else {
            extras
                .raw_answers
                .iter()
                .map(|answer| crate::judge::SampleStamp {
                    sha256: crate::hash::sha256_hex(answer.text.as_bytes()),
                    dropped: answer.dropped,
                })
                .collect()
        };
        let prov =
            provenance.get_or_insert_with(|| crate::judge::RubricProvenance::declared(None, None));
        prov.samples = stamps;
    }
    // Уровень независимости считается ЗДЕСЬ, а не вызывающим: иначе один из
    // двух путей (CLI и MCP) мог бы писать отчёт без уровня, и порог
    // независимости молча не действовал бы (J5, ADR-048).
    let independence = independence_for(
        author_model,
        &report.judge_model,
        provenance.as_ref(),
        &extras.families,
    );
    let artifact = RubricArtifact {
        schema: RUBRIC_REPORT_SCHEMA.to_string(),
        rubric: report.rubric_name.clone(),
        target,
        target_sha256: sha,
        judge_model: report.judge_model.clone(),
        author_model: author_model.map(str::to_string),
        weighted_total: report.weighted_total,
        verdict: report.verdict.clone(),
        unstable: report
            .scores
            .iter()
            .any(|s| s.has_flag(CriterionFlag::Unstable)),
        evidence_not_found: report
            .scores
            .iter()
            .filter(|s| s.has_flag(CriterionFlag::EvidenceNotFound))
            .count(),
        independence: Some(independence),
        judge_config: extras.judge_config.clone(),
        author_source: extras.author_source.clone(),
        author_model_declared: extras.author_model_declared.clone(),
        provenance,
        pack_kind,
        subject: pack_subject,
        pack_sha256,
        inputs,
        scores: report.scores.clone(),
        invalid_samples_ratio: report.invalid_samples_ratio,
        decision: report.decision,
        decision_reasons: report.decision_reasons.clone(),
        // E2: пометки инъекций входа переезжают в отчёт для гейта. `None` —
        // отчёт записан до появления детектора: отсутствие поля означает «не
        // сканировали», и сверка его не штрафует (как provenance и scores).
        input_injection_lines: (!report.input_injections.is_empty()).then(|| {
            report
                .input_injections
                .iter()
                .map(|i| i.line)
                .collect::<Vec<_>>()
        }),
        judged_at: chrono::Local::now().to_rfc3339(),
    };
    let dir = repo.join(RUBRIC_REPORTS_DIR);
    Ok((dir.join(format!("{file_stem}.json")), artifact))
}

/// Slug файла отчёта по досье: имя файла субъекта + вид досье + диапазон
/// строк фрагмента.
///
/// Отдельный slug обязателен: отчёт о качестве документа (`adr_quality` по
/// ADR-051) и отчёт о согласованности того же документа со спайном
/// (`adr_spine_consistency`) — разные отчёты об одном файле, и общий slug
/// заставлял бы их затирать друг друга.
#[must_use]
pub fn pack_artifact_slug(kind: &str, subject: &str) -> String {
    let (path, frag) = subject
        .split_once('#')
        .map_or((subject, None), |(p, f)| (p, Some(f)));
    let base = artifact_slug(Some(Path::new(path)));
    let frag_slug = frag.map(|f| {
        let s: String = f
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || c == '-' {
                    c
                } else {
                    '-'
                }
            })
            .collect();
        s.trim_matches('-').to_string()
    });
    match frag_slug {
        Some(f) if !f.is_empty() => format!("{base}--{kind}--{f}"),
        _ => format!("{base}--{kind}"),
    }
}

/// Все машиночитаемые отчёты рубрик репозитория (`reports/rubric/*.json`);
/// нечитаемый или чужой JSON пропускается — отчёт, а не гейт.
#[must_use]
pub fn load_artifacts(repo: &Path) -> Vec<RubricArtifact> {
    let dir = repo.join(RUBRIC_REPORTS_DIR);
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut out: Vec<RubricArtifact> = rd
        .flatten()
        .filter(|e| {
            e.path()
                .extension()
                .is_some_and(|x| x.eq_ignore_ascii_case("json"))
        })
        .filter_map(|e| std::fs::read_to_string(e.path()).ok())
        .filter_map(|t| serde_json::from_str::<RubricArtifact>(&t).ok())
        .collect();
    out.sort_by(|a, b| a.judged_at.cmp(&b.judged_at));
    out
}
