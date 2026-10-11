//! Специализированные архитектурные бенчмарки (solution architecture).
//!
//! КОНТРАКТ (владелец: агент `rubric` — общий с rubric.rs):
//! - [`Benchmark`] — YAML-сценарий: имя, описание, постановка задачи
//!   (system+user промпты), ссылка на рубрику оценки, проходной порог;
//! - [`run`] — прогон сценария на модели, оценка ответа рубрикой
//!   ([`crate::rubric::evaluate_with_options`]), запись отчёта в `out_dir` (md+json);
//! - golden-set (`assets/benchmarks/golden/`): синтетические документы
//!   `<имя>.md` + эталонные оценки `<имя>.expected.yaml` ([`GoldenExpectation`]);
//!   [`run_golden`] — прогон судьи по набору, согласие с эталоном — MAE
//!   ([`mean_absolute_error`]); регрессионный порог — на стороне CLI (ADR-004).

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config::JudgeConfig;
use crate::error::{HarnessError, Result};
use crate::llm::{ChatMessage, ChatRequest, LlmProvider};

/// Сценарий бенчмарка.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Benchmark {
    /// Имя сценария.
    pub name: String,
    /// Описание: что измеряет.
    pub description: String,
    /// Системный промпт (роль solution-архитектора).
    pub system_prompt: String,
    /// Постановка задачи.
    pub task: String,
    /// Файл рубрики (относительно assets/rubrics или абсолютный).
    pub rubric: String,
    /// Проходной взвешенный порог (`0..=scale_max`).
    pub pass_threshold: f64,
    /// Теги (integration, adr, nfr, brownfield, …).
    #[serde(default)]
    pub tags: Vec<String>,
}

/// Сводка для списка.
#[derive(Debug, Clone)]
pub struct BenchSummary {
    /// Путь к YAML.
    pub path: PathBuf,
    /// Имя.
    pub name: String,
    /// Описание.
    pub description: String,
    /// Теги.
    pub tags: Vec<String>,
}

/// Отчёт о прогоне.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchReport {
    /// Имя бенчмарка.
    pub bench_name: String,
    /// Модель-испытуемый.
    pub model: String,
    /// Ответ модели (полный текст).
    pub response: String,
    /// Отчёт рубрики (сериализованный).
    pub rubric_report: crate::rubric::RubricReport,
    /// Прошёл ли порог.
    pub passed: bool,
}

/// Ожидаемый балл критерия: точка или диапазон.
///
/// Смысловая рубрика судит **смысл**, и эталон здесь — диапазон, а не точка
/// (ADR-051, S4): «противоречие найдено» это 1..2, и требовать от судьи ровно
/// 1 значило бы мерить не то, что рубрика обещает. Старый формат со скалярами
/// читается: `context: 5` разбирается как точка.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(untagged)]
pub enum GoldenRange {
    /// Точка (формат 0.3.4): один балл.
    Point(u8),
    /// Диапазон включительно.
    Span {
        /// Нижняя граница.
        min: u8,
        /// Верхняя граница.
        max: u8,
    },
}

impl GoldenRange {
    /// Попадает ли балл судьи в диапазон.
    #[must_use]
    pub fn contains(self, score: u8) -> bool {
        match self {
            Self::Point(point) => score == point,
            Self::Span { min, max } => score >= min && score <= max,
        }
    }

    /// Середина диапазона — для MAE, метрики 0.3.4: она остаётся сравнимой с
    /// историей прогонов, а попадание считается отдельно.
    #[must_use]
    pub fn midpoint(self) -> f64 {
        match self {
            Self::Point(point) => f64::from(point),
            Self::Span { min, max } => f64::midpoint(f64::from(min), f64::from(max)),
        }
    }

    /// Верхняя граница — по ней проверяется, не выше ли шкалы эталон.
    #[must_use]
    pub fn upper(self) -> u8 {
        match self {
            Self::Point(point) => point,
            Self::Span { max, .. } => max,
        }
    }
}

/// Метка golden-кейса: засеянный дефект или чистый документ.
///
/// Нужна там, где ошибки судьи несимметричны (ADR-051, S4): ложное обвинение
/// на чистом документе и пропуск дефекта — разные провалы, и в среднем MAE они
/// взаимно гасятся. Без метки кейс оценивается только по MAE.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GoldenKind {
    /// Метка не задана (формат 0.3.4).
    #[default]
    Unlabeled,
    /// Чистый документ: обвинение здесь — ложное.
    Clean,
    /// Засеянный дефект: «всё чисто» здесь — пропуск.
    Defective,
}

/// Эталонные оценки golden-документа (`<имя>.expected.yaml` рядом с `<имя>.md`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoldenExpectation {
    /// Имя рубрики: файл в assets/rubrics (расширение `.yaml` опционально).
    pub rubric: String,
    /// Ожидаемые баллы по критериям (id → точка `1..=scale_max` или диапазон).
    pub scores: BTreeMap<String, GoldenRange>,
    /// Метка кейса — для раздельного счёта ложных обвинений и пропусков.
    #[serde(default)]
    pub kind: GoldenKind,
}

/// Отчёт по одному golden-документу.
#[derive(Debug, Clone)]
pub struct GoldenCaseReport {
    /// Имя файла документа.
    pub doc: String,
    /// MAE по критериям эталона этого документа.
    pub mae: f64,
    /// Сколько пар «судья × эталон» сравнено.
    pub compared: usize,
    /// Сколько пар попало в диапазон эталона (для смысловых кейсов эталон —
    /// диапазон, и попадание считается отдельно от MAE).
    pub hits: usize,
    /// Взвешенный балл судьи (как в отчёте рубрики).
    pub weighted_judge: f64,
    /// Взвешенный балл эталона (те же веса критериев рубрики).
    pub weighted_expected: f64,
    /// Длина документа в символах (для диагностики length bias).
    pub doc_chars: usize,
}

/// MAE по одному критерию рубрики — по всем документам golden-set.
#[derive(Debug, Clone)]
pub struct CriterionMae {
    /// Идентификатор критерия.
    pub criterion_id: String,
    /// MAE по документам, где критерий оценён эталоном.
    pub mae: f64,
    /// Сколько документов сравнено.
    pub compared: usize,
}

/// Диагностика length bias: корреляция Спирмена взвешенного балла
/// с длиной документа (в символах) — отдельно для судьи и эталона.
#[derive(Debug, Clone, Copy)]
pub struct LengthBias {
    /// ρ(взвешенный балл судьи, длина документа).
    pub judge_rho: f64,
    /// ρ(взвешенный балл эталона, длина документа).
    pub expected_rho: f64,
}

/// Отчёт golden-прогона судьи (ADR-004).
#[derive(Debug, Clone)]
pub struct GoldenReport {
    /// Модель-судья.
    pub judge_model: String,
    /// Разбор по документам.
    pub cases: Vec<GoldenCaseReport>,
    /// Итоговый MAE по всем парам «документ × критерий».
    pub mae: f64,
    /// Всего сравненных пар.
    pub compared: usize,
    /// Пар, попавших в диапазон эталона (ADR-051, S4): у смысловых рубрик
    /// эталон — диапазон, и попадание важнее близости к середине.
    pub hits: usize,
    /// Знаменатель [`Self::hits`] — сравненные пары на момент счёта.
    pub hits_compared: usize,
    /// Имена чистых документов, на которых судья выставил обвинение: ложное
    /// обвинение — отдельный провал судьи, и в среднем MAE он гасится
    /// пропусками на дефектных.
    pub false_accusations: Vec<String>,
    /// Чем подтверждено каждое ложное обвинение (главный критерий и балл).
    pub false_accusation_notes: Vec<String>,
    /// Имена дефектных документов, на которых обвинения не было: пропуск.
    pub misses: Vec<String>,
    /// MAE по критериям (по всем документам), отсортировано по id критерия.
    pub criterion_mae: Vec<CriterionMae>,
    /// Length bias (Спирмен ρ балла с длиной документа); `None`, если
    /// документов меньше двух или дисперсия длин/баллов нулевая.
    pub length_bias: Option<LengthBias>,
}

impl GoldenReport {
    /// Текст механической диагностики: MAE по критериям и length bias
    /// (Спирмен ρ взвешенного балла с длиной документа) судьи и эталона.
    #[must_use]
    pub fn diagnostics_text(&self) -> String {
        let mut out = String::new();
        if !self.criterion_mae.is_empty() {
            let _ = writeln!(out, "MAE по критериям:");
            for c in &self.criterion_mae {
                let _ = writeln!(
                    out,
                    "  {:<32} MAE {:.2} ({} документов)",
                    c.criterion_id, c.mae, c.compared
                );
            }
        }
        if let Some(bias) = self.length_bias {
            let _ = writeln!(
                out,
                "  length bias: судья ρ={:.2}, эталон ρ={:.2}",
                bias.judge_rho, bias.expected_rho
            );
        }
        if self.hits_compared > 0 {
            let _ = writeln!(
                out,
                "  попадание в диапазон эталона: {}/{} ({:.0}%)",
                self.hits,
                self.hits_compared,
                100.0 * self.hits as f64 / self.hits_compared as f64
            );
        }
        // Смысловой слой (ADR-051, S4): ошибки судьи несимметричны, и в среднем
        // MAE они гасятся — поэтому считаются раздельно и печатаются всегда,
        // даже нулями: «0 ложных обвинений» — это утверждение, а не отсутствие
        // измерения.
        if !self.false_accusations.is_empty() || !self.misses.is_empty() {
            let _ = writeln!(
                out,
                "  ложных обвинений на чистых: {}; пропусков на дефектных: {}",
                self.false_accusations.len(),
                self.misses.len()
            );
            for note in &self.false_accusation_notes {
                let _ = writeln!(out, "    ложное обвинение — {note}");
            }
            for doc in &self.misses {
                let _ = writeln!(out, "    пропуск — {doc}");
            }
        }
        out
    }
}

/// Загружает бенчмарк из YAML.
///
/// # Errors
/// Файл не читается / не валиден.
pub fn load(path: &Path) -> Result<Benchmark> {
    let text = std::fs::read_to_string(path).map_err(|e| HarnessError::io(path, e))?;
    let bench: Benchmark = serde_yaml_ng::from_str(&text)?;
    Ok(bench)
}

/// Список бенчмарков каталога (`*.yaml`/`*.yml`); битые файлы пропускаются.
///
/// # Errors
/// Каталог не читается.
pub fn list(dir: &Path) -> Result<Vec<BenchSummary>> {
    let entries = std::fs::read_dir(dir).map_err(|e| HarnessError::io(dir, e))?;
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let is_yaml = path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("yaml") || e.eq_ignore_ascii_case("yml"));
        if !is_yaml {
            continue;
        }
        // Битый файл — не ошибка каталога: пропускаем.
        if let Ok(bench) = load(&path) {
            out.push(BenchSummary {
                path,
                name: bench.name,
                description: bench.description,
                tags: bench.tags,
            });
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

/// Прогоняет бенчмарк на модели и оценивает рубрикой; пишет отчёты в `out_dir`.
///
/// Ответ модели оценивается тем же провайдером (судья = испытуемая модель),
/// с настройками судьи `judge` (k сэмплов, верификация цитат — ADR-004).
/// Отчёты: `bench-<name>-<model>-<yyyymmdd-hhmmss>.md` (задача, ответ, таблица
/// рубрики, PASS/FAIL) и `.json` (сериализованный [`BenchReport`]).
///
/// # Errors
/// Ошибка модели/судьи/записи.
pub async fn run(
    bench: &Benchmark,
    provider: &dyn LlmProvider,
    rubrics_dir: &Path,
    out_dir: &Path,
    judge: &JudgeConfig,
) -> Result<BenchReport> {
    let request = ChatRequest::chat(vec![
        ChatMessage::system(bench.system_prompt.clone()),
        ChatMessage::user(bench.task.clone()),
    ]);
    let response = provider.complete(request).await?.content;

    let rubric_path = {
        let p = PathBuf::from(&bench.rubric);
        if p.is_absolute() {
            p
        } else {
            rubrics_dir.join(p)
        }
    };
    let rubric = crate::rubric::load(&rubric_path)?;
    // Судья — та же модель, что и испытуемый.
    let rubric_report =
        crate::rubric::evaluate_with_options(&rubric, &response, provider, judge).await?;
    let passed = rubric_report.weighted_total >= bench.pass_threshold;
    let report = BenchReport {
        bench_name: bench.name.clone(),
        model: provider.model().to_string(),
        response,
        rubric_report,
        passed,
    };

    std::fs::create_dir_all(out_dir).map_err(|e| HarnessError::io(out_dir, e))?;
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let base = format!(
        "bench-{}-{}-{stamp}",
        sanitize_file_part(&bench.name),
        sanitize_file_part(provider.model())
    );
    let md_path = out_dir.join(format!("{base}.md"));
    std::fs::write(&md_path, report_markdown(bench, &report))
        .map_err(|e| HarnessError::io(&md_path, e))?;
    let json_path = out_dir.join(format!("{base}.json"));
    let json = serde_json::to_string_pretty(&report)?;
    std::fs::write(&json_path, json).map_err(|e| HarnessError::io(&json_path, e))?;
    Ok(report)
}

/// Загружает golden-set каталога: пары «`<имя>.md` + `<имя>.expected.yaml`».
///
/// В отличие от [`list`], битый эталон — ошибка, а не пропуск: молчаливо
/// потерянный документ завышал бы измеренное согласие судьи с эталоном.
///
/// # Errors
/// Каталог не читается; эталон не парсится / без оценок / с баллом 0;
/// документ к эталону отсутствует.
pub fn load_golden(dir: &Path) -> Result<Vec<(PathBuf, GoldenExpectation)>> {
    let entries = std::fs::read_dir(dir).map_err(|e| HarnessError::io(dir, e))?;
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let Some(stem) = name.strip_suffix(".expected.yaml") else {
            continue;
        };
        let text = std::fs::read_to_string(&path).map_err(|e| HarnessError::io(&path, e))?;
        let expectation: GoldenExpectation = serde_yaml_ng::from_str(&text)
            .map_err(|e| HarnessError::Bench(format!("{}: разбор эталона: {e}", path.display())))?;
        if expectation.scores.is_empty() {
            return Err(HarnessError::Bench(format!(
                "{}: эталон без оценок",
                path.display()
            )));
        }
        if let Some((id, _)) = expectation.scores.iter().find(|(_, s)| s.upper() == 0) {
            return Err(HarnessError::Bench(format!(
                "{}: критерий '{id}' — балл 0 вне шкалы",
                path.display()
            )));
        }
        let doc = dir.join(format!("{stem}.md"));
        if !doc.is_file() {
            return Err(HarnessError::Bench(format!(
                "{}: нет документа к эталону {}",
                doc.display(),
                path.display()
            )));
        }
        out.push((doc, expectation));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

/// Имя golden-документа для отчёта (без каталога).
fn doc_name(doc_path: &Path) -> String {
    doc_path.file_name().map_or_else(
        || doc_path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    )
}

/// Обвинение по главному критерию рубрики — и чем оно подтверждено.
///
/// «Пойман» для смысловой рубрики (ADR-051, S4) — это главный критерий
/// (`blocking`) с баллом ≤ 2 И подтверждёнными цитатами: обвинение без цитат
/// механика сама исключает из итога (`accusation_unconfirmed`), и считать его
/// поимкой значило бы записывать судье в заслугу выдуманное свидетельство.
fn main_accusation(
    rubric: &crate::rubric::Rubric,
    report: &crate::rubric::RubricReport,
) -> Option<String> {
    let main = rubric.criteria.iter().find(|c| c.blocking)?;
    let score = report.scores.iter().find(|s| s.criterion_id == main.id)?;
    if score.score > 2 {
        return None;
    }
    if score.flags.iter().any(|f| f.excludes_from_total()) {
        return None;
    }
    Some(if score.checked.is_empty() {
        format!("'{}' = {} (без перечня проверенного)", main.id, score.score)
    } else {
        format!("'{}' = {}", main.id, score.score)
    })
}

/// Прогоняет судью по golden-set и считает согласие с эталоном (MAE, ADR-004).
///
/// Каждый документ оценивается полным контуром
/// [`crate::rubric::evaluate_with_options`] рубрикой из его эталона; MAE —
/// среднее |судья − эталон| по всем парам «документ × критерий». Эталон,
/// ссылающийся на критерий вне рубрики или на балл выше шкалы, — ошибка
/// данных, а не расхождение судьи. Кроме итогового MAE отчёт несёт
/// механическую диагностику: MAE по критериям и length bias — корреляцию
/// Спирмена взвешенного балла с длиной документа (судья и эталон).
///
/// # Errors
/// Пустой/битый golden-set, ошибка чтения, модели или рубрики.
pub async fn run_golden(
    provider: &dyn LlmProvider,
    rubrics_dir: &Path,
    golden_dir: &Path,
    judge: &JudgeConfig,
) -> Result<GoldenReport> {
    run_golden_filtered(provider, rubrics_dir, golden_dir, judge, None).await
}

/// Прогон golden-набора по одной рубрике (ADR-051, S4): смысловые рубрики
/// лежат в общем наборе (`assets/benchmarks/golden/semantic/<рубрика>/`), и
/// калибруют их поимённо — иначе метрика смешала бы оценки разных судейских
/// задач, а регрессия одной рубрики тонула бы в среднем по всем.
///
/// # Errors
/// Пустой (после фильтра) набор, ошибка чтения, модели или рубрики.
pub async fn run_golden_filtered(
    provider: &dyn LlmProvider,
    rubrics_dir: &Path,
    golden_dir: &Path,
    judge: &JudgeConfig,
    only_rubric: Option<&str>,
) -> Result<GoldenReport> {
    // Смысловые рубрики лежат в подкаталоге `semantic/<рубрика>/` (ADR-051,
    // S4): у каждой свой набор досье, и общий обход верхнего каталога смешал бы
    // их с ADR-набором `adr_quality`.
    let dir = only_rubric.map_or_else(
        || golden_dir.to_path_buf(),
        |name| {
            let sub = golden_dir.join("semantic").join(name);
            if sub.is_dir() {
                sub
            } else {
                golden_dir.to_path_buf()
            }
        },
    );
    let mut cases = load_golden(&dir)?;
    if let Some(name) = only_rubric {
        cases.retain(|(_, e)| e.rubric == name);
    }
    if cases.is_empty() {
        return Err(HarnessError::Bench(format!(
            "golden-set пуст: в {} нет пар <имя>.md + <имя>.expected.yaml{}",
            golden_dir.display(),
            only_rubric.map_or_else(String::new, |n| format!(" для рубрики '{n}'")),
        )));
    }
    let mut all_pairs: Vec<(f64, f64)> = Vec::new();
    let mut criterion_pairs: BTreeMap<String, Vec<(f64, f64)>> = BTreeMap::new();
    let mut case_reports = Vec::with_capacity(cases.len());
    let mut all_hits = 0usize;
    let mut all_compared = 0usize;
    let mut false_accusations: Vec<String> = Vec::new();
    let mut false_accusation_notes: Vec<String> = Vec::new();
    let mut misses: Vec<String> = Vec::new();
    for (doc_path, expectation) in &cases {
        let rubric_path = {
            let direct = rubrics_dir.join(&expectation.rubric);
            if direct.is_file() {
                direct
            } else {
                rubrics_dir.join(format!("{}.yaml", expectation.rubric))
            }
        };
        let rubric = crate::rubric::load(&rubric_path)?;
        let text = std::fs::read_to_string(doc_path).map_err(|e| HarnessError::io(doc_path, e))?;
        // Смысловая рубрика судится по досье (ADR-051, S4): golden-файл — это
        // замороженное досье с маркерами источников, а не документ. Оценка
        // идёт тем же кодом, что у собранного досье, — цитаты сверяются по
        // ролям, покрытие по составу.
        let report = match rubric.pack {
            Some(kind) => {
                let subject = doc_path.file_name().map_or_else(
                    || doc_path.display().to_string(),
                    |n| n.to_string_lossy().into_owned(),
                );
                let pack = crate::rubric_pack::ContextPack::from_text(kind, &subject, &text)?;
                crate::rubric::evaluate_pack(&rubric, &pack, provider, judge).await?
            }
            None => crate::rubric::evaluate_with_options(&rubric, &text, provider, judge).await?,
        };
        let mut pairs = Vec::with_capacity(expectation.scores.len());
        let mut expected_scores = Vec::with_capacity(expectation.scores.len());
        let mut hits = 0usize;
        for (criterion_id, &expected) in &expectation.scores {
            if expected.upper() > rubric.scale_max {
                return Err(HarnessError::Bench(format!(
                    "эталон {}: критерий '{criterion_id}' — балл {} выше шкалы {}",
                    doc_path.display(),
                    expected.upper(),
                    rubric.scale_max
                )));
            }
            let got = report
                .scores
                .iter()
                .find(|s| &s.criterion_id == criterion_id)
                .map(|s| s.score)
                .ok_or_else(|| {
                    HarnessError::Bench(format!(
                        "эталон {} ссылается на критерий '{criterion_id}', которого нет в рубрике {}",
                        doc_path.display(),
                        rubric.name
                    ))
                })?;
            if expected.contains(got) {
                hits += 1;
            }
            let pair = (f64::from(got), expected.midpoint());
            pairs.push(pair);
            criterion_pairs
                .entry(criterion_id.clone())
                .or_default()
                .push(pair);
            expected_scores.push(crate::rubric::CriterionScore {
                citations: Vec::new(),
                evidence_channel: None,
                scenario_verdicts: Vec::new(),
                criterion_id: criterion_id.clone(),
                weight: 0.0,
                score: expected.midpoint().round() as u8,
                rationale: String::new(),
                samples: Vec::new(),
                stdev: 0.0,
                evidence_unconfirmed_ratio: 0.0,
                invalid_samples: 0,
                checked: Vec::new(),
                flags: Vec::new(),
            });
        }
        let case_mae = mean_absolute_error(&pairs).ok_or_else(|| {
            HarnessError::Bench(format!(
                "{}: нет сравненных пар для MAE",
                doc_path.display()
            ))
        })?;
        // Взвешенный эталонный балл — той же формулой, что и итог рубрики.
        let weighted_expected = crate::rubric::weighted_total(&rubric.criteria, &expected_scores)?;
        all_pairs.extend(pairs.iter().copied());
        all_hits += hits;
        all_compared += pairs.len();
        // Раздельный счёт ошибок судьи (ADR-051, S4): на чистом документе
        // обвинение — ложное, на дефектном «всё чисто» — пропуск. В среднем
        // MAE они взаимно гасятся, и без метки кейса провал не виден.
        let main_accused = main_accusation(&rubric, &report);
        match (expectation.kind, &main_accused) {
            (GoldenKind::Clean, Some(reason)) => {
                false_accusations.push(doc_name(doc_path));
                false_accusation_notes
                    .push(format!("{}: главный критерий {reason}", doc_name(doc_path)));
            }
            (GoldenKind::Defective, None) => misses.push(doc_name(doc_path)),
            _ => {}
        }
        case_reports.push(GoldenCaseReport {
            doc: doc_name(doc_path),
            mae: case_mae,
            compared: pairs.len(),
            hits,
            weighted_judge: report.weighted_total,
            weighted_expected,
            doc_chars: text.chars().count(),
        });
    }
    let mae = mean_absolute_error(&all_pairs)
        .ok_or_else(|| HarnessError::Bench("golden-set без сравненных пар для MAE".into()))?;
    let criterion_mae = criterion_pairs
        .iter()
        .filter_map(|(criterion_id, pairs)| {
            mean_absolute_error(pairs).map(|mae| CriterionMae {
                criterion_id: criterion_id.clone(),
                mae,
                compared: pairs.len(),
            })
        })
        .collect();
    // Диагностика length bias: Спирмен ρ взвешенного балла с длиной документа
    // отдельно для судьи и эталона; обе корреляции должны быть определены.
    let lengths: Vec<f64> = case_reports.iter().map(|c| c.doc_chars as f64).collect();
    let judge_totals: Vec<f64> = case_reports.iter().map(|c| c.weighted_judge).collect();
    let expected_totals: Vec<f64> = case_reports.iter().map(|c| c.weighted_expected).collect();
    let length_bias = spearman(&judge_totals, &lengths)
        .zip(spearman(&expected_totals, &lengths))
        .map(|(judge_rho, expected_rho)| LengthBias {
            judge_rho,
            expected_rho,
        });
    Ok(GoldenReport {
        judge_model: provider.model().to_string(),
        cases: case_reports,
        mae,
        compared: all_pairs.len(),
        hits: all_hits,
        hits_compared: all_compared,
        false_accusations,
        false_accusation_notes,
        misses,
        criterion_mae,
        length_bias,
    })
}

/// Средняя абсолютная ошибка по парам (факт, эталон); пустой вход — `None`.
#[must_use]
pub fn mean_absolute_error(pairs: &[(f64, f64)]) -> Option<f64> {
    if pairs.is_empty() {
        return None;
    }
    let sum: f64 = pairs.iter().map(|(got, want)| (got - want).abs()).sum();
    Some(sum / pairs.len() as f64)
}

/// Корреляция Спирмена между двумя векторами: ранжирование с усреднением
/// рангов при связях, затем Пирсон на рангах. Меньше двух наблюдений,
/// разная длина входов или нулевая дисперсия — `None`.
#[must_use]
pub fn spearman(xs: &[f64], ys: &[f64]) -> Option<f64> {
    if xs.len() < 2 || xs.len() != ys.len() {
        return None;
    }
    pearson(&average_ranks(xs), &average_ranks(ys))
}

/// Средние ранги (1-based): связанные значения получают средний ранг группы.
fn average_ranks(xs: &[f64]) -> Vec<f64> {
    let mut order: Vec<usize> = (0..xs.len()).collect();
    order.sort_by(|&a, &b| xs[a].total_cmp(&xs[b]));
    let mut ranks = vec![0.0; xs.len()];
    let mut i = 0;
    while i < order.len() {
        let mut j = i;
        while j + 1 < order.len() && xs[order[j + 1]] == xs[order[i]] {
            j += 1;
        }
        let rank = (i + j) as f64 / 2.0 + 1.0;
        for &idx in &order[i..=j] {
            ranks[idx] = rank;
        }
        i = j + 1;
    }
    ranks
}

/// Корреляция Пирсона двух векторов одинаковой длины; нулевая дисперсия
/// хотя бы одного — `None` (корреляция не определена).
fn pearson(xs: &[f64], ys: &[f64]) -> Option<f64> {
    let n = xs.len() as f64;
    let mean_x = xs.iter().sum::<f64>() / n;
    let mean_y = ys.iter().sum::<f64>() / n;
    let (mut sxy, mut sxx, mut syy) = (0.0, 0.0, 0.0);
    for (&x, &y) in xs.iter().zip(ys) {
        let dx = x - mean_x;
        let dy = y - mean_y;
        sxy += dx * dy;
        sxx += dx * dx;
        syy += dy * dy;
    }
    if sxx <= 0.0 || syy <= 0.0 {
        return None;
    }
    Some(sxy / (sxx * syy).sqrt())
}

/// Строка evidence-журнала golden-прогонов (JSONL, `bench run --golden --record`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoldenRecord {
    /// Дата прогона (`YYYY-MM-DD`).
    pub date: String,
    /// Модель-судья.
    pub model: String,
    /// Итоговый MAE по всем парам «документ × критерий».
    pub mae: f64,
    /// Сравненных пар.
    pub compared: usize,
    /// MAE по критериям.
    pub criterion_mae: BTreeMap<String, f64>,
    /// ρ судьи (length bias); `null`, если корреляция не определена.
    pub judge_rho: Option<f64>,
    /// ρ эталона (length bias); `null`, если корреляция не определена.
    pub expected_rho: Option<f64>,
    /// Число документов набора.
    pub docs: usize,
}

impl GoldenRecord {
    /// Снимок отчёта [`GoldenReport`] на дату `date` (`YYYY-MM-DD`).
    #[must_use]
    pub fn from_report(report: &GoldenReport, date: String) -> Self {
        Self {
            date,
            model: report.judge_model.clone(),
            mae: report.mae,
            compared: report.compared,
            criterion_mae: report
                .criterion_mae
                .iter()
                .map(|c| (c.criterion_id.clone(), c.mae))
                .collect(),
            judge_rho: report.length_bias.map(|b| b.judge_rho),
            expected_rho: report.length_bias.map(|b| b.expected_rho),
            docs: report.cases.len(),
        }
    }
}

/// Дописывает запись строкой JSON в evidence-журнал (M-2); файл создаётся
/// при отсутствии, прежнее содержимое не затирается.
///
/// # Errors
/// Файл не открывается/не пишется; запись не сериализуется.
pub fn record_golden(path: &Path, record: &GoldenRecord) -> Result<()> {
    use std::io::Write as _;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| HarnessError::io(path, e))?;
    let line = serde_json::to_string(record)?;
    writeln!(file, "{line}").map_err(|e| HarnessError::io(path, e))?;
    Ok(())
}

/// Читает evidence-журнал истории golden-прогонов (JSONL).
///
/// Пустые строки пропускаются молча (хвостовой перевод строки), битые —
/// считаются и пропускаются: возвращает `(записи, число битых строк)`.
///
/// # Errors
/// Файл отсутствует или не читается.
pub fn load_golden_history(path: &Path) -> Result<(Vec<GoldenRecord>, usize)> {
    let text = std::fs::read_to_string(path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            HarnessError::Bench(format!(
                "журнал истории {} не найден: сначала прогоните `bench run --golden --record <PATH>`",
                path.display()
            ))
        } else {
            HarnessError::io(path, e)
        }
    })?;
    let mut records = Vec::new();
    let mut broken = 0;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        match serde_json::from_str::<GoldenRecord>(line) {
            Ok(record) => records.push(record),
            // Битая строка — не ошибка журнала: считаем и пропускаем.
            Err(_) => broken += 1,
        }
    }
    Ok((records, broken))
}

/// Markdown-таблица истории golden-прогонов: дата | модель | MAE | docs |
/// судья ρ | эталон ρ; неопределённая корреляция — «—».
#[must_use]
pub fn golden_history_markdown(records: &[GoldenRecord]) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "| Дата | Модель | MAE | Docs | Судья ρ | Эталон ρ |");
    let _ = writeln!(out, "| --- | --- | --- | --- | --- | --- |");
    for r in records {
        let fmt_rho = |rho: Option<f64>| rho.map_or_else(|| "—".to_string(), |v| format!("{v:.2}"));
        let _ = writeln!(
            out,
            "| {} | {} | {:.2} | {} | {} | {} |",
            r.date,
            r.model,
            r.mae,
            r.docs,
            fmt_rho(r.judge_rho),
            fmt_rho(r.expected_rho)
        );
    }
    out
}

/// Согласие по одному golden-документу с человеческими анкетами (J-3).
#[derive(Debug, Clone)]
pub struct DocAgreement {
    /// Имя файла документа.
    pub doc: String,
    /// Эталон golden (критерий → балл).
    pub golden: BTreeMap<String, u8>,
    /// Медиана людей по критериям (критерий → медиана).
    pub medians: BTreeMap<String, f64>,
    /// Оценки участников (участник → критерий → балл).
    pub humans: Vec<(String, BTreeMap<String, u8>)>,
}

/// Отчёт о согласии golden-эталонов с оценками живых архитекторов (J-3).
#[derive(Debug, Clone)]
pub struct HumanAgreementReport {
    /// Разбор по документам, имеющим человеческие анкеты.
    pub docs: Vec<DocAgreement>,
    /// (а) MAE «эталон golden vs медиана людей» по всем парам «документ × критерий».
    pub golden_vs_humans_mae: f64,
    /// (б) Среднее MAE «человек vs медиана людей» — уровень межчеловеческого
    /// согласия, бенчмарк для (а).
    pub inter_human_mae: f64,
    /// Сравненных пар «документ × критерий» в (а).
    pub compared: usize,
    /// Документы golden без человеческих анкет (пропущены).
    pub skipped: Vec<String>,
}

impl HumanAgreementReport {
    /// (в) Вердикт: в пределах ли расхождение эталона с людьми
    /// межчеловеческого разброса.
    #[must_use]
    pub fn verdict(&self) -> &'static str {
        if self.golden_vs_humans_mae <= self.inter_human_mae {
            "расхождение эталона с людьми в пределах межчеловеческого разброса"
        } else {
            "расхождение эталона с людьми ВЫШЕ межчеловеческого разброса — пересмотреть golden-эталоны"
        }
    }

    /// Markdown-представление: таблица «документ × критерий → медиана людей
    /// vs эталон golden», итоговые метрики (а)/(б) и строка-вердикт (в).
    #[must_use]
    pub fn to_markdown(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(
            out,
            "| Документ | Критерий | Медиана людей | Эталон golden | Δ |"
        );
        let _ = writeln!(out, "| --- | --- | --- | --- | --- |");
        for doc in &self.docs {
            for (criterion, &expected) in &doc.golden {
                // Медиана посчитана для каждого критерия эталона; её отсутствие
                // невозможно по построению, но не паникуем на битых данных.
                let Some(&median) = doc.medians.get(criterion) else {
                    continue;
                };
                let _ = writeln!(
                    out,
                    "| {} | {} | {:.1} | {} | {:.1} |",
                    doc.doc,
                    criterion,
                    median,
                    expected,
                    (f64::from(expected) - median).abs()
                );
            }
        }
        let _ = writeln!(
            out,
            "\n(а) MAE эталон golden vs медиана людей: {:.2} ({} пар)",
            self.golden_vs_humans_mae, self.compared
        );
        let _ = writeln!(
            out,
            "(б) Среднее MAE человек vs медиана людей (межчеловеческое согласие): {:.2}",
            self.inter_human_mae
        );
        let _ = writeln!(out, "(в) Вердикт: {}", self.verdict());
        out
    }
}

/// Считает согласие golden-эталонов с человеческими анкетами (J-3).
///
/// Анкеты — файлы `<имя-документа>.<участник>.expected.yaml` в `humans_dir`
/// той же схемы [`GoldenExpectation`]. По каждому документу и критерию —
/// медиана людей; документы golden без анкет пропускаются (список —
/// в [`HumanAgreementReport::skipped`]). Участник с неполным набором
/// критериев — ошибка данных с именем файла.
///
/// # Errors
/// Каталоги не читаются; анкета не парсится, содержит балл 0 или неполный
/// набор критериев; ни один документ не имеет анкет.
pub fn human_agreement(golden_dir: &Path, humans_dir: &Path) -> Result<HumanAgreementReport> {
    let cases = load_golden(golden_dir)?;
    if cases.is_empty() {
        return Err(HarnessError::Bench(format!(
            "golden-set пуст: в {} нет пар <имя>.md + <имя>.expected.yaml",
            golden_dir.display()
        )));
    }
    let human_files = list_human_files(humans_dir)?;
    let mut docs = Vec::new();
    let mut skipped = Vec::new();
    for (doc_path, expectation) in &cases {
        let stem = doc_path
            .file_stem()
            .and_then(|s| s.to_str())
            .ok_or_else(|| {
                HarnessError::Bench(format!("{}: нечитаемое имя документа", doc_path.display()))
            })?;
        let prefix = format!("{stem}.");
        let mut humans: Vec<(String, BTreeMap<String, u8>)> = Vec::new();
        for path in human_files.iter().filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with(&prefix))
        }) {
            humans.push(parse_human_expectation(path, &prefix, expectation)?);
        }
        if humans.is_empty() {
            skipped.push(
                doc_path
                    .file_name()
                    .map_or_else(|| stem.to_string(), |n| n.to_string_lossy().into_owned()),
            );
            continue;
        }
        // Медиана людей по каждому критерию эталона.
        let mut medians = BTreeMap::new();
        for criterion in expectation.scores.keys() {
            let scores: Vec<f64> = humans
                .iter()
                .filter_map(|(_, scores)| scores.get(criterion).map(|&s| f64::from(s)))
                .collect();
            let median = median(&scores).ok_or_else(|| {
                HarnessError::Bench(format!(
                    "{}: критерий '{criterion}' без человеческих оценок",
                    doc_path.display()
                ))
            })?;
            medians.insert(criterion.clone(), median);
        }
        docs.push(DocAgreement {
            doc: doc_path
                .file_name()
                .map_or_else(|| stem.to_string(), |n| n.to_string_lossy().into_owned()),
            golden: expectation
                .scores
                .iter()
                .map(|(id, range)| (id.clone(), range.midpoint().round() as u8))
                .collect(),
            medians,
            humans,
        });
    }
    if docs.is_empty() {
        return Err(HarnessError::Bench(format!(
            "в {} нет анкет ни к одному документу golden-set",
            humans_dir.display()
        )));
    }
    // (а) MAE «эталон golden vs медиана людей» по всем парам.
    let golden_pairs: Vec<(f64, f64)> = docs
        .iter()
        .flat_map(|d| {
            d.golden
                .iter()
                .filter_map(|(c, &expected)| d.medians.get(c).map(|&m| (f64::from(expected), m)))
        })
        .collect();
    let golden_vs_humans_mae = mean_absolute_error(&golden_pairs)
        .ok_or_else(|| HarnessError::Bench("нет пар для MAE эталона с людьми".into()))?;
    // (б) Среднее MAE «человек vs медиана людей».
    let human_pairs: Vec<(f64, f64)> = docs
        .iter()
        .flat_map(|d| {
            d.humans.iter().flat_map(|(_, scores)| {
                scores
                    .iter()
                    .filter_map(|(c, &s)| d.medians.get(c).map(|&m| (f64::from(s), m)))
            })
        })
        .collect();
    let inter_human_mae = mean_absolute_error(&human_pairs)
        .ok_or_else(|| HarnessError::Bench("нет пар для межчеловеческого MAE".into()))?;
    Ok(HumanAgreementReport {
        docs,
        golden_vs_humans_mae,
        inter_human_mae,
        compared: golden_pairs.len(),
        skipped,
    })
}

/// Список файлов анкет каталога (`*.expected.yaml`), отсортированный по имени.
fn list_human_files(humans_dir: &Path) -> Result<Vec<PathBuf>> {
    let entries = std::fs::read_dir(humans_dir).map_err(|e| HarnessError::io(humans_dir, e))?;
    let mut out: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.ends_with(".expected.yaml"))
        })
        .collect();
    out.sort();
    Ok(out)
}

/// Разбирает анкету участника: имя участника — из имени файла (префикс
/// `<документ>.` срезается); проверяет полноту набора критериев эталона.
fn parse_human_expectation(
    path: &Path,
    prefix: &str,
    golden: &GoldenExpectation,
) -> Result<(String, BTreeMap<String, u8>)> {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| HarnessError::Bench(format!("{}: нечитаемое имя анкеты", path.display())))?;
    let participant = name
        .strip_prefix(prefix)
        .and_then(|s| s.strip_suffix(".expected.yaml"))
        .ok_or_else(|| {
            HarnessError::Bench(format!(
                "{}: имя анкеты не по схеме <документ>.<участник>.expected.yaml",
                path.display()
            ))
        })?;
    let text = std::fs::read_to_string(path).map_err(|e| HarnessError::io(path, e))?;
    let expectation: GoldenExpectation = serde_yaml_ng::from_str(&text)
        .map_err(|e| HarnessError::Bench(format!("{}: разбор анкеты: {e}", path.display())))?;
    if let Some((id, _)) = expectation.scores.iter().find(|(_, s)| s.upper() == 0) {
        return Err(HarnessError::Bench(format!(
            "{}: критерий '{id}' — балл 0 вне шкалы",
            path.display()
        )));
    }
    if let Some(missing) = golden
        .scores
        .keys()
        .find(|c| !expectation.scores.contains_key(*c))
    {
        return Err(HarnessError::Bench(format!(
            "{}: неполный набор критериев: нет оценки '{missing}'",
            path.display()
        )));
    }
    // Лишние критерии участника игнорируются: сравнение — только по эталону.
    let scores = expectation
        .scores
        .iter()
        .filter(|(c, _)| golden.scores.contains_key(*c))
        .map(|(c, range)| (c.clone(), range.midpoint().round() as u8))
        .collect();
    Ok((participant.to_string(), scores))
}

/// Медиана выборки; чётное число — среднее двух центральных; пустой вход — `None`.
#[must_use]
pub fn median(xs: &[f64]) -> Option<f64> {
    if xs.is_empty() {
        return None;
    }
    let mut sorted = xs.to_vec();
    sorted.sort_by(f64::total_cmp);
    let mid = sorted.len() / 2;
    if sorted.len() % 2 == 1 {
        Some(sorted[mid])
    } else {
        Some(sorted[mid - 1].midpoint(sorted[mid]))
    }
}

/// Заменяет символы, небезопасные в имени файла, на `-`.
fn sanitize_file_part(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '-'
            }
        })
        .collect()
}

/// Markdown-отчёт прогона: задача, ответ модели, таблица рубрики, PASS/FAIL.
fn report_markdown(bench: &Benchmark, report: &BenchReport) -> String {
    let mut out = String::new();
    let verdict = if report.passed { "PASS" } else { "FAIL" };
    let _ = writeln!(out, "# Бенчмарк «{}» — {}\n", report.bench_name, verdict);
    let _ = writeln!(out, "**Модель:** {}", report.model);
    let _ = writeln!(
        out,
        "**Дата:** {}",
        chrono::Local::now().format("%Y-%m-%d %H:%M:%S")
    );
    let _ = writeln!(
        out,
        "**Взвешенный итог:** {:.2}/5 (порог {:.2})\n",
        report.rubric_report.weighted_total, bench.pass_threshold
    );
    let _ = writeln!(out, "## Задача\n\n{}\n", bench.task);
    let _ = writeln!(out, "## Ответ модели\n\n{}\n", report.response);
    let _ = writeln!(
        out,
        "## Оценка по рубрике\n\n{}\n",
        report.rubric_report.to_markdown()
    );
    let cmp = if report.passed { ">=" } else { "<" };
    let _ = writeln!(
        out,
        "## Результат\n\n**{}**: взвешенный итог {:.2} {} порог {:.2}.",
        verdict, report.rubric_report.weighted_total, cmp, bench.pass_threshold
    );
    out
}

// ── Метрика 13: arch-diff-replay (волна D, ADR-072) ──────────────────────────
//
// Replay значимых диффов и окон-шума по замороженной разметке
// `benchmarks/architect-value/labels/arch-diff-replay.yaml` (приложение B.3
// задачи). Семантика зеркалит прогонщик
// `benchmarks/architect-value/runners/replay.py` (заморожен вместе с входами):
//   recall значимых   — доля значимых коммитов, где «по составу» всплыл хотя
//                       бы один ожидаемый токен РЁБЕР/ВНЕШНИХ систем;
//                       компонент-узел сам по себе не «состав»;
//   precision состава — macro-F1 множества всплывших состав-токенов (рёбра +
//                       внешние) против ожидаемых того же вида;
//   шум               — доля окон-шума с НЕПУСТЫМ диффом (порог ≤ 10 %).
//
// Нормализация токенов (label): срезается префикс `dir:`/`sys:`/`svc:`/
// `node:`/`edge:`, берётся последний сегмент пути, lower-case; ребро —
// нормализованные концы через `->`.

/// Префиксы id узлов, срезаемые при нормализации токена (заморожено).
const REPLAY_PREFIXES: [&str; 5] = ["dir:", "sys:", "svc:", "node:", "edge:"];

/// Списки узлов диффа (для «всплывших» токенов).
const REPLAY_NODE_LISTS: [&str; 2] = ["added_nodes", "removed_nodes"];

/// Списки рёбер диффа.
const REPLAY_EDGE_LISTS: [&str; 2] = ["added_edges", "removed_edges"];

/// Поля диффа, непустота любого из которых = «дифф непустой» (заморожено;
/// шире [`ArchDiff::is_empty`] — включает предложения/ADR/объявленные рёбра).
const REPLAY_NONEMPTY_FIELDS: [&str; 10] = [
    "added_nodes",
    "removed_nodes",
    "added_edges",
    "removed_edges",
    "contract_changes",
    "nfr_shifts",
    "invariants_touched",
    "proposals",
    "declared_unused",
    "adrs_touched",
];

/// Нормализация токена метки: `dir:src/x` → `x`; `sys:tracing-server:9411` →
/// `tracing-server:9411` (срез одного префикса, последний сегмент пути, lower).
#[must_use]
pub fn replay_norm_token(tok: &str) -> String {
    let mut t = tok.trim().to_lowercase();
    for p in REPLAY_PREFIXES {
        if let Some(rest) = t.strip_prefix(p) {
            t = rest.to_string();
            break;
        }
    }
    match t.rsplit_once('/') {
        Some((_, last)) => last.to_string(),
        None => t,
    }
}

/// Нормализованное ребро: `<from>-><to>`.
#[must_use]
pub fn replay_norm_edge(from: &str, to: &str) -> String {
    format!("{}->{}", replay_norm_token(from), replay_norm_token(to))
}

/// Разметка метрики 13 (`labels/arch-diff-replay.yaml`).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ReplayLabels {
    /// Значения по умолчанию (base-шаблон, пороги).
    #[serde(default)]
    pub defaults: ReplayDefaults,
    /// Значимые коммиты.
    #[serde(default)]
    pub significant: Vec<ReplaySignificant>,
    /// Окна-шума.
    #[serde(default)]
    pub noise: Vec<ReplayNoise>,
}

/// Значения по умолчанию разметки.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ReplayDefaults {
    /// Шаблон базы диффа (`{sha}~1`); `{sha}` подставляется sha коммита.
    #[serde(default)]
    pub base: Option<String>,
    /// Формат диффа (заморожен `json`; поле читается для совместимости).
    #[serde(default)]
    pub format: Option<String>,
    /// Пороги метрики.
    #[serde(default)]
    pub thresholds: ReplayThresholds,
}

/// Пороги метрики 13 (заморожены в PREREGISTRATION.md).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplayThresholds {
    /// Порог recall значимых (≥).
    pub recall_significant: f64,
    /// Порог macro-F1 состава (≥).
    pub composition_f1: f64,
    /// Порог доли шума (≤).
    pub noise_ratio: f64,
}

impl Default for ReplayThresholds {
    fn default() -> Self {
        Self {
            recall_significant: 0.80,
            composition_f1: 0.80,
            noise_ratio: 0.10,
        }
    }
}

/// Значимый коммит разметки.
#[derive(Debug, Clone, Deserialize)]
pub struct ReplaySignificant {
    /// Логическое имя репо (`petclinic`/`boutique`/…).
    pub repo: String,
    /// Полный sha коммита.
    pub sha: String,
    /// Человекочитаемый заголовок.
    #[serde(default)]
    pub title: String,
    /// Ожидаемые компоненты-токены (для справки; в F1 не входят).
    #[serde(default)]
    pub expect_components: Vec<String>,
    /// Ожидаемые внешние системы-токены.
    #[serde(default)]
    pub expect_external: Vec<String>,
    /// Ожидаемые рёбра — пары `[from, to]`.
    #[serde(default)]
    pub expect_edges: Vec<Vec<String>>,
}

/// Окно-шума разметки (эталон «архитектурных изменений нет»).
#[derive(Debug, Clone, Deserialize)]
pub struct ReplayNoise {
    /// Логическое имя репо.
    pub repo: String,
    /// Полный sha коммита.
    pub sha: String,
    /// Человекочитаемый заголовок.
    #[serde(default)]
    pub title: String,
    /// Вид окна (dependency/ci/ui/…).
    #[serde(default)]
    pub kind: String,
}

/// Результат по одному значимому коммиту.
#[derive(Debug, Clone, Serialize)]
pub struct ReplaySigResult {
    /// Логическое имя репо.
    pub repo: String,
    /// sha коммита.
    pub sha: String,
    /// Заголовок.
    pub title: String,
    /// Всплыл ли ≥1 ожидаемый состав-токен.
    pub hit: bool,
    /// F1 состава этого коммита.
    pub f1: f64,
    /// Всплывшие состав-токены (рёбра + внешние), отсортированы.
    pub surf_comp: Vec<String>,
    /// Ожидаемые состав-токены (рёбра + внешние), отсортированы.
    pub exp_comp: Vec<String>,
}

/// Результат по одному окну-шума.
#[derive(Debug, Clone, Serialize)]
pub struct ReplayNoiseResult {
    /// Логическое имя репо.
    pub repo: String,
    /// sha коммита.
    pub sha: String,
    /// Вид окна.
    pub kind: String,
    /// Непустой ли дифф (ложное срабатывание).
    pub nonempty: bool,
}

/// Отчёт метрики 13.
#[derive(Debug, Clone, Serialize)]
pub struct ReplayReport {
    /// recall значимых агрегированный.
    pub recall_significant: f64,
    /// macro-F1 состава.
    pub composition_f1: f64,
    /// Доля непустых окон-шума.
    pub noise_ratio: f64,
    /// Сработавших значимых.
    pub sig_hits: usize,
    /// Измеренных значимых.
    pub sig_measured: usize,
    /// Непустых окон-шума.
    pub noise_fp: usize,
    /// Измеренных окон-шума.
    pub noise_measured: usize,
    /// Пороги из разметки.
    pub thresholds: ReplayThresholds,
    /// По-коммитные результаты значимых.
    pub significant: Vec<ReplaySigResult>,
    /// По-коммитные результаты окон-шума.
    pub noise: Vec<ReplayNoiseResult>,
    /// Пропущенные метки (нет клона/коммита/ошибка диффа) с причиной.
    pub skipped: Vec<String>,
}

impl ReplayReport {
    /// Сборка отчёта из по-коммитных результатов (чистая агрегация).
    #[must_use]
    pub fn from_results(
        significant: Vec<ReplaySigResult>,
        noise: Vec<ReplayNoiseResult>,
        thresholds: ReplayThresholds,
        skipped: Vec<String>,
    ) -> Self {
        let sig_measured = significant.len();
        let sig_hits = significant.iter().filter(|r| r.hit).count();
        let f1_sum: f64 = significant.iter().map(|r| r.f1).sum();
        let noise_measured = noise.len();
        let noise_fp = noise.iter().filter(|r| r.nonempty).count();
        #[allow(clippy::cast_precision_loss)]
        let recall = if sig_measured == 0 {
            0.0
        } else {
            sig_hits as f64 / sig_measured as f64
        };
        #[allow(clippy::cast_precision_loss)]
        let macro_f1 = if sig_measured == 0 {
            0.0
        } else {
            f1_sum / sig_measured as f64
        };
        #[allow(clippy::cast_precision_loss)]
        let noise_ratio = if noise_measured == 0 {
            0.0
        } else {
            noise_fp as f64 / noise_measured as f64
        };
        Self {
            recall_significant: recall,
            composition_f1: macro_f1,
            noise_ratio,
            sig_hits,
            sig_measured,
            noise_fp,
            noise_measured,
            thresholds,
            significant,
            noise,
            skipped,
        }
    }

    /// Прошёл ли прогон пороги метрики 13.
    #[must_use]
    pub fn passed(&self) -> bool {
        self.recall_significant >= self.thresholds.recall_significant
            && self.composition_f1 >= self.thresholds.composition_f1
            && self.noise_ratio <= self.thresholds.noise_ratio
    }

    /// Однострочная сводка метрики 13.
    #[must_use]
    pub fn summary(&self) -> String {
        format!(
            "recall_significant={}/{}={:.2} (порог≥{:.2}); composition_F1={:.2} \
             (порог≥{:.2}); noise={}/{}={:.2} (порог≤{:.2}) — {}",
            self.sig_hits,
            self.sig_measured,
            self.recall_significant,
            self.thresholds.recall_significant,
            self.composition_f1,
            self.thresholds.composition_f1,
            self.noise_fp,
            self.noise_measured,
            self.noise_ratio,
            self.thresholds.noise_ratio,
            if self.passed() { "PASS" } else { "FAIL" }
        )
    }

    /// Многострочный текстовый отчёт (для `--format text`).
    #[must_use]
    pub fn to_text(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(out, "arch-diff-replay — метрика 13");
        for r in &self.significant {
            let _ = writeln!(
                out,
                "  significant [{}:{}] hit={} f1={:.2} surf={} exp={} — {}",
                r.repo,
                &r.sha[..r.sha.len().min(7)],
                if r.hit { "да" } else { "нет" },
                r.f1,
                join_or(&r.surf_comp, "нет"),
                join_or(&r.exp_comp, "нет"),
                r.title
            );
        }
        for r in &self.noise {
            if r.nonempty {
                let _ = writeln!(
                    out,
                    "  noise [{}:{}] непустой дифф ({})",
                    r.repo,
                    &r.sha[..r.sha.len().min(7)],
                    r.kind
                );
            }
        }
        for s in &self.skipped {
            let _ = writeln!(out, "  skip: {s}");
        }
        let _ = writeln!(out, "  {}", self.summary());
        out
    }
}

/// Список токенов через запятую либо `dash` при пустоте.
fn join_or(items: &[String], dash: &str) -> String {
    if items.is_empty() {
        dash.to_string()
    } else {
        items.join(", ")
    }
}

/// Непустой ли массив по ключу (пустой/отсутствующий — нет).
fn field_nonempty(doc: &serde_json::Value, key: &str) -> bool {
    doc.get(key)
        .and_then(serde_json::Value::as_array)
        .is_some_and(|a| !a.is_empty())
}

/// «Всплывшие» состав-токены диффа: (внешние системы, рёбра).
#[must_use]
pub fn replay_surfaced(doc: &serde_json::Value) -> (BTreeSet<String>, BTreeSet<String>) {
    let mut externals = BTreeSet::new();
    let mut edges = BTreeSet::new();
    for key in REPLAY_NODE_LISTS {
        if let Some(arr) = doc.get(key).and_then(serde_json::Value::as_array) {
            for n in arr {
                let nid = n
                    .get("id")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("");
                if nid.starts_with("sys:") {
                    externals.insert(replay_norm_token(nid));
                }
            }
        }
    }
    for key in REPLAY_EDGE_LISTS {
        if let Some(arr) = doc.get(key).and_then(serde_json::Value::as_array) {
            for e in arr {
                let from = e
                    .get("from")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("");
                let to = e
                    .get("to")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("");
                edges.insert(replay_norm_edge(from, to));
            }
        }
    }
    (externals, edges)
}

/// Ожидаемые состав-токены метки: (внешние, рёбра).
#[must_use]
pub fn replay_expected(label: &ReplaySignificant) -> (BTreeSet<String>, BTreeSet<String>) {
    let ext = label
        .expect_external
        .iter()
        .map(|s| replay_norm_token(s))
        .collect();
    let edges = label
        .expect_edges
        .iter()
        .filter_map(|pair| match pair.as_slice() {
            [from, to] => Some(replay_norm_edge(from, to)),
            _ => None,
        })
        .collect();
    (ext, edges)
}

/// F1 множеств (surf против exp) и размер пересечения.
#[must_use]
pub fn replay_f1(surf: &BTreeSet<String>, exp: &BTreeSet<String>) -> (f64, usize) {
    let inter = surf.intersection(exp).count();
    if surf.is_empty() || exp.is_empty() {
        return (0.0, inter);
    }
    #[allow(clippy::cast_precision_loss)]
    let p = inter as f64 / surf.len() as f64;
    #[allow(clippy::cast_precision_loss)]
    let r = inter as f64 / exp.len() as f64;
    let f = if p + r > 0.0 {
        2.0 * p * r / (p + r)
    } else {
        0.0
    };
    (f, inter)
}

/// «Дифф непустой» (замороженная семантика): любое из полей состава/
/// контрактов/NFR/предложений/ADR непусто ИЛИ сработали триггеры/счёт маршрута.
#[must_use]
pub fn replay_is_nonempty(doc: &serde_json::Value) -> bool {
    if REPLAY_NONEMPTY_FIELDS
        .iter()
        .any(|k| field_nonempty(doc, k))
    {
        return true;
    }
    let route = doc.get("route");
    let has_triggers = route
        .and_then(|r| r.get("triggers"))
        .and_then(serde_json::Value::as_array)
        .is_some_and(|a| !a.is_empty());
    let score = route
        .and_then(|r| r.get("score"))
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    has_triggers || score > 0
}

/// Сверка одного значимого коммита с эталоном.
#[must_use]
pub fn replay_evaluate_significant(
    label: &ReplaySignificant,
    doc: &serde_json::Value,
) -> ReplaySigResult {
    let (ext, edges) = replay_surfaced(doc);
    let mut surf_comp: BTreeSet<String> = ext;
    surf_comp.extend(edges);
    let (exp_ext, exp_edges) = replay_expected(label);
    let mut exp_comp: BTreeSet<String> = exp_ext;
    exp_comp.extend(exp_edges);
    let (f1, inter) = replay_f1(&surf_comp, &exp_comp);
    ReplaySigResult {
        repo: label.repo.clone(),
        sha: label.sha.clone(),
        title: label.title.clone(),
        hit: inter >= 1,
        f1,
        surf_comp: surf_comp.into_iter().collect(),
        exp_comp: exp_comp.into_iter().collect(),
    }
}

/// Оценка одного окна-шума.
#[must_use]
pub fn replay_evaluate_noise(label: &ReplayNoise, doc: &serde_json::Value) -> ReplayNoiseResult {
    ReplayNoiseResult {
        repo: label.repo.clone(),
        sha: label.sha.clone(),
        kind: label.kind.clone(),
        nonempty: replay_is_nonempty(doc),
    }
}

/// Каталог клона по логическому имени репо (как `avlib.REPO_CLONES`).
#[must_use]
fn replay_clone_name(repo: &str) -> &str {
    match repo {
        "boutique" => "microservices-demo",
        "petclinic" => "spring-petclinic-microservices",
        "buckpal" => "buckpal",
        "library" => "library",
        other => other,
    }
}

/// Подставляет `{sha}` в шаблон базы (по умолчанию `{sha}~1`).
fn replay_base_ref(defaults: &ReplayDefaults, sha: &str) -> String {
    defaults
        .base
        .clone()
        .unwrap_or_else(|| "{sha}~1".to_string())
        .replace("{sha}", sha)
}

/// Прогон метрики 13: дифф каждой метки (`base` → `head=sha`) по клону репо и
/// сверка с эталоном. `repo_filter` — `None`/`Some("all")` = все репо. `last` —
/// рассматривать не более N последних меток каждого списка (после фильтра).
///
/// Отсутствие клона/коммита и ошибка диффа — честный `skipped`, не паника
/// (как замороженный `replay.py`).
///
/// # Errors
/// Не читается/не разбирается файл разметки.
pub fn replay_arch_diff(
    labels_path: &Path,
    repos_dir: &Path,
    repo_filter: Option<&str>,
    last: Option<usize>,
    limits: (usize, usize),
    globs: &crate::control::DiffGlobs,
) -> Result<ReplayReport> {
    let text = std::fs::read_to_string(labels_path).map_err(|e| {
        HarnessError::Config(format!(
            "метрика 13: не читается разметка {}: {e}",
            labels_path.display()
        ))
    })?;
    let labels: ReplayLabels = serde_yaml_ng::from_str(&text).map_err(|e| {
        HarnessError::Config(format!(
            "метрика 13: не разобрана разметка {}: {e}",
            labels_path.display()
        ))
    })?;
    let thresholds = labels.defaults.thresholds.clone();
    let keep = |repo: &str| match repo_filter {
        None | Some("all") => true,
        Some(want) => repo == want,
    };
    let cap = |n: usize| -> usize { last.map_or(n, |l| l.min(n)) };

    let mut skipped: Vec<String> = Vec::new();
    let mut significant: Vec<ReplaySigResult> = Vec::new();
    let mut noise: Vec<ReplayNoiseResult> = Vec::new();

    // Значимые: F1 состава важнее половины — замеряем все.
    let sig_labels: Vec<&ReplaySignificant> = labels
        .significant
        .iter()
        .filter(|s| keep(&s.repo))
        .collect();
    let sig_take = cap(sig_labels.len());
    for label in sig_labels.into_iter().take(sig_take) {
        let short = &label.sha[..label.sha.len().min(7)];
        let tag = format!("[{}:{short}]", label.repo);
        let Some(clone) = replay_clone(repos_dir, &label.repo) else {
            skipped.push(format!(
                "metric 13 significant {tag}: клон {} отсутствует",
                label.repo
            ));
            continue;
        };
        match replay_run_diff(
            &clone,
            &replay_base_ref(&labels.defaults, &label.sha),
            &label.sha,
            limits,
            globs,
        ) {
            Ok(doc) => significant.push(replay_evaluate_significant(label, &doc)),
            Err(e) => skipped.push(format!("metric 13 significant {tag}: {e}")),
        }
    }

    // Окна-шума: доля непустых диффов.
    let noise_labels: Vec<&ReplayNoise> = labels.noise.iter().filter(|n| keep(&n.repo)).collect();
    let noise_take = cap(noise_labels.len());
    for label in noise_labels.into_iter().take(noise_take) {
        let short = &label.sha[..label.sha.len().min(7)];
        let tag = format!("[{}:{short}]", label.repo);
        let Some(clone) = replay_clone(repos_dir, &label.repo) else {
            skipped.push(format!(
                "metric 13 noise {tag}: клон {} отсутствует",
                label.repo
            ));
            continue;
        };
        match replay_run_diff(
            &clone,
            &replay_base_ref(&labels.defaults, &label.sha),
            &label.sha,
            limits,
            globs,
        ) {
            Ok(doc) => noise.push(replay_evaluate_noise(label, &doc)),
            Err(e) => skipped.push(format!("metric 13 noise {tag}: {e}")),
        }
    }

    Ok(ReplayReport::from_results(
        significant,
        noise,
        thresholds,
        skipped,
    ))
}

/// Дифф одной метки: `arch_diff` → JSON → разбор в [`serde_json::Value`].
fn replay_run_diff(
    clone: &Path,
    base: &str,
    head: &str,
    limits: (usize, usize),
    globs: &crate::control::DiffGlobs,
) -> Result<serde_json::Value> {
    let declared = BTreeMap::new();
    let diff = crate::arch_diff::arch_diff(
        clone,
        &crate::arch_diff::ArchDiffInput {
            base,
            head: Some(head),
            declared,
            limits,
            globs,
        },
    )
    .map_err(|e| HarnessError::Config(format!("ошибка диффа: {e}")))?;
    let json = crate::arch_diff::render_json(&diff)
        .map_err(|e| HarnessError::Config(format!("ошибка рендера JSON: {e}")))?;
    serde_json::from_str(&json).map_err(|e| HarnessError::Config(format!("дифф не JSON: {e}")))
}

/// Каталог клона репо, если он есть (`<repos_dir>/<имя-клона>`).
fn replay_clone(repos_dir: &Path, repo: &str) -> Option<PathBuf> {
    let cand = repos_dir.join(replay_clone_name(repo));
    cand.is_dir().then_some(cand)
}

/// Корень клонов бенчмарка: `ARCH_VALUE_REPOS` / `~/bench-repos`.
#[must_use]
pub fn replay_default_repos_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("ARCH_VALUE_REPOS") {
        return PathBuf::from(dir);
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join("bench-repos")
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;

    /// Рубрика-пример для прогонов.
    const RUBRIC_YAML: &str = "\
name: core
description: Базовая рубрика
scale_max: 5
origin: anchor
criteria:
  - id: context
    name: Контекст
    description: Описан контекст
    weight: 1.0
    anchors:
      1: нет
      3: частично
      5: полный
  - id: risks
    name: Риски
    description: Названы риски
    weight: 1.0
";

    /// Бенчмарк-пример, ссылающийся на [`RUBRIC_YAML`] по имени файла.
    const BENCH_YAML: &str = "\
name: adr-basic
description: Написать ADR
system_prompt: Ты solution-архитектор.
task: Напиши ADR миграции платёжного шлюза.
rubric: core.yaml
pass_threshold: 3.0
tags:
  - adr
";

    /// Фейк-провайдер: на задачу отвечает `answer`, на судейский промпт — `judge`.
    #[derive(Debug)]
    struct FakeLlm {
        answer: String,
        judge: String,
    }

    #[async_trait]
    impl LlmProvider for FakeLlm {
        fn name(&self) -> &'static str {
            "fake"
        }
        fn model(&self) -> &'static str {
            "fake-model"
        }
        async fn complete(&self, req: ChatRequest) -> Result<ChatMessage> {
            let is_judge = req
                .messages
                .iter()
                .any(|m| m.content.contains("архитектурный судья"));
            let content = if is_judge {
                self.judge.clone()
            } else {
                self.answer.clone()
            };
            Ok(ChatMessage::assistant(content, Vec::new()))
        }
    }

    #[test]
    fn benchmark_yaml_roundtrip() {
        let bench: Benchmark = serde_yaml_ng::from_str(BENCH_YAML).expect("parse");
        let yaml = serde_yaml_ng::to_string(&bench).expect("serialize");
        let back: Benchmark = serde_yaml_ng::from_str(&yaml).expect("reparse");
        assert_eq!(back.name, "adr-basic");
        assert_eq!(back.system_prompt, "Ты solution-архитектор.");
        assert_eq!(back.rubric, "core.yaml");
        assert_eq!(back.tags, vec!["adr".to_string()]);
        assert!((back.pass_threshold - 3.0).abs() < 1e-9);
    }

    #[test]
    fn list_skips_broken_files() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("good.yaml"), BENCH_YAML).expect("write");
        std::fs::write(dir.path().join("broken.yml"), "{{{").expect("write");
        std::fs::write(dir.path().join("README.md"), "не бенч").expect("write");
        let items = list(dir.path()).expect("list");
        assert_eq!(
            items.len(),
            1,
            "битый и не-yaml файлы должны быть пропущены"
        );
        assert_eq!(items[0].name, "adr-basic");
        assert_eq!(items[0].tags, vec!["adr".to_string()]);
    }

    #[test]
    fn sanitize_replaces_unsafe_chars() {
        assert_eq!(sanitize_file_part("openai/gpt 5"), "openai-gpt-5");
        assert_eq!(sanitize_file_part("deepseek-chat"), "deepseek-chat");
    }

    /// Полный прогон бенчмарка на фейковой модели: файлы во временном каталоге.
    async fn run_case(judge: &str) -> (tempfile::TempDir, BenchReport, PathBuf) {
        let dir = tempfile::tempdir().expect("tempdir");
        let rubrics = dir.path().join("rubrics");
        std::fs::create_dir_all(&rubrics).expect("mkdir");
        std::fs::write(rubrics.join("core.yaml"), RUBRIC_YAML).expect("write rubric");
        let bench: Benchmark = serde_yaml_ng::from_str(BENCH_YAML).expect("bench");
        let llm = FakeLlm {
            answer: "ADR-001: мигрируем платёжный шлюз. Контекст: вендор уходит. \
                     Риски: двойная запись, откат."
                .into(),
            judge: judge.into(),
        };
        let out = dir.path().join("out");
        let report = run(&bench, &llm, &rubrics, &out, &JudgeConfig::default())
            .await
            .expect("run");
        (dir, report, out)
    }

    #[tokio::test]
    async fn run_writes_reports_and_passes_above_threshold() {
        // Цитаты в rationale — дословные фрагменты ответа модели (ADR-004).
        let judge = "{\"scores\": [\
             {\"criterion_id\": \"context\", \"score\": 5, \"rationale\": \"Цитата: \\\"Контекст: вендор уходит\\\" — контекст назван\"}, \
             {\"criterion_id\": \"risks\", \"score\": 4, \"rationale\": \"Цитата: \\\"Риски: двойная запись, откат\\\" — риски перечислены\"}], \
             \"verdict\": \"годно\"}";
        let (_dir, report, out) = run_case(judge).await;
        assert!(report.passed, "4.5 >= 3.0");
        assert_eq!(report.bench_name, "adr-basic");
        assert_eq!(report.model, "fake-model");
        assert!((report.rubric_report.weighted_total - 4.5).abs() < 1e-9);

        // На диске — md и json с контрактным именем.
        let files: Vec<PathBuf> = std::fs::read_dir(&out)
            .expect("read_dir")
            .flatten()
            .map(|e| e.path())
            .collect();
        let md = files
            .iter()
            .find(|p| p.extension().is_some_and(|e| e == "md"))
            .expect("md отчёт");
        let json_path = files
            .iter()
            .find(|p| p.extension().is_some_and(|e| e == "json"))
            .expect("json отчёт");
        let file_name = md.file_name().expect("имя файла").to_string_lossy();
        assert!(
            file_name.starts_with("bench-adr-basic-fake-model-"),
            "имя отчёта: {file_name}"
        );
        let md_text = std::fs::read_to_string(md).expect("read md");
        assert!(md_text.contains("PASS"));
        assert!(md_text.contains("ADR-001"), "ответ модели в отчёте");
        assert!(md_text.contains("| Критерий | Вес | Балл | Метки | Обоснование |"));
        let parsed: BenchReport =
            serde_json::from_str(&std::fs::read_to_string(json_path).expect("read json"))
                .expect("parse json");
        assert!(parsed.passed);
        assert_eq!(parsed.response, report.response);
        assert_eq!(parsed.rubric_report.scores.len(), 2);
    }

    #[tokio::test]
    async fn run_fails_below_threshold() {
        let judge = "{\"scores\": [\
             {\"criterion_id\": \"context\", \"score\": 2, \"rationale\": \"Цитата: \\\"Контекст: вендор уходит\\\" — слабо\"}, \
             {\"criterion_id\": \"risks\", \"score\": 2, \"rationale\": \"Цитата: \\\"Риски: двойная запись, откат\\\" — слабо\"}], \
             \"verdict\": \"плохо\"}";
        let (_dir, report, _out) = run_case(judge).await;
        assert!(!report.passed, "2.0 < 3.0");
        assert!(report.rubric_report.weighted_total < 3.0);
    }

    #[test]
    fn mean_absolute_error_math() {
        assert_eq!(mean_absolute_error(&[]), None, "пустой вход — None");
        let mae = mean_absolute_error(&[(4.0, 5.0), (5.0, 5.0), (1.0, 2.0)]).expect("mae");
        assert!((mae - 2.0 / 3.0).abs() < 1e-9, "MAE: {mae}");
    }

    /// Документ golden-фикстуры без свидетельств (эталон — единицы).
    const GOLDEN_BAD_MD: &str = "Проект важен. Дедлайн скоро.";
    /// Эталон к [`GOLDEN_BAD_MD`].
    const GOLDEN_BAD_YAML: &str = "rubric: core\nscores:\n  context: 1\n  risks: 1\n";
    /// Документ golden-фикстуры со свидетельствами (эталон — пятёрки).
    const GOLDEN_GOOD_MD: &str = "Контекст описан полностью. Риски названы и разобраны подробно.";
    /// Эталон к [`GOLDEN_GOOD_MD`].
    const GOLDEN_GOOD_YAML: &str = "rubric: core\nscores:\n  context: 5\n  risks: 5\n";

    /// Фейк-судья с очередью ответов (по одному на прогон оценки).
    #[derive(Debug)]
    struct QueueLlm {
        replies: std::sync::Mutex<std::collections::VecDeque<String>>,
    }

    impl QueueLlm {
        fn new(replies: &[String]) -> Self {
            Self {
                replies: std::sync::Mutex::new(replies.iter().cloned().collect()),
            }
        }
    }

    #[async_trait]
    impl LlmProvider for QueueLlm {
        fn name(&self) -> &'static str {
            "fake"
        }
        fn model(&self) -> &'static str {
            "fake-queue"
        }
        async fn complete(&self, _req: ChatRequest) -> Result<ChatMessage> {
            let reply = self
                .replies
                .lock()
                .expect("mutex poisoned")
                .pop_front()
                .unwrap_or_default();
            Ok(ChatMessage::assistant(reply, Vec::new()))
        }
    }

    /// Диапазоны и раздельный счёт ошибок (ADR-051, S4): `--rubric` ищет набор
    /// в подкаталоге `semantic/<рубрика>`, досье читается как досье (маркеры
    /// источников), эталон-диапазон считается попаданием, а ложное обвинение
    /// на чистом кейсе — отдельным провалом, не тонущим в MAE.
    #[tokio::test]
    async fn golden_semantic_rubric_counts_hits_and_false_accusations() {
        let tmp = tempfile::tempdir().expect("tmp");
        let rubrics = tmp.path().join("rubrics");
        let golden = tmp.path().join("golden/semantic/adr_spine_consistency");
        std::fs::create_dir_all(&rubrics).expect("mkdir");
        std::fs::create_dir_all(&golden).expect("mkdir");
        std::fs::write(
            rubrics.join("adr_spine_consistency.yaml"),
            crate::assets::RUBRIC_ADR_SPINE_CONSISTENCY,
        )
        .expect("rubric");
        let dossier = |adr: &str| {
            format!(
                "=== ИСТОЧНИК subject: docs/adr/ADR-001.md ===\n{adr}\n\
                 === КОНЕЦ ИСТОЧНИКА ===\n\
                 === ИСТОЧНИК reference: ARCHITECTURE-SPINE.md#AD-1 ===\n\
                 AD-1: Журнал только дописывается\nRule: строки журнала не правятся.\n\
                 === КОНЕЦ ИСТОЧНИКА ===\n"
            )
        };
        std::fs::write(
            golden.join("defect.md"),
            dossier("Прямое противоречие инварианту."),
        )
        .expect("doc");
        std::fs::write(
            golden.join("defect.expected.yaml"),
            "rubric: adr_spine_consistency\nkind: defective\nscores:\n  no_contradiction: {min: 1, max: 2}\n",
        )
        .expect("exp");
        std::fs::write(golden.join("clean.md"), dossier("Противоречий нет.")).expect("doc");
        std::fs::write(
            golden.join("clean.expected.yaml"),
            "rubric: adr_spine_consistency\nkind: clean\nscores:\n  no_contradiction: {min: 4, max: 5}\n",
        )
        .expect("exp");
        // Судья: на дефектном кейсе обвинение, на чистом — похвала с полным
        // перечнем проверенного (иначе критерий исключается как непокрытый).
        let accuse = r#"{"scores":[{"criterion_id":"no_contradiction","score":1,"rationale":"Цитата subject: \"Прямое противоречие инварианту.\". Цитата reference: \"Rule: строки журнала не правятся.\". противоречие","checked":["AD-1"]}],"verdict":"CONCERNS"}"#;
        let praise = r#"{"scores":[{"criterion_id":"no_contradiction","score":5,"rationale":"Цитата subject: \"Противоречий нет.\". Цитата reference: \"Rule: строки журнала не правятся.\". чисто","checked":["AD-1"]}],"verdict":"PASS"}"#;
        // Кейсы отсортированы по имени файла: `clean.md` идёт первым.
        let llm = QueueLlm::new(&[praise.to_string(), accuse.to_string()]);
        let cfg = JudgeConfig {
            samples: 1,
            ..JudgeConfig::default()
        };
        let report = run_golden_filtered(
            &llm,
            &rubrics,
            &tmp.path().join("golden"),
            &cfg,
            Some("adr_spine_consistency"),
        )
        .await
        .expect("прогон");
        assert_eq!(report.cases.len(), 2, "набор найден в подкаталоге рубрики");
        assert_eq!(report.hits, 2, "оба балла попали в диапазон: {report:?}");
        assert_eq!(report.hits_compared, 2);
        assert!(
            report.false_accusations.is_empty() && report.misses.is_empty(),
            "ложных обвинений и пропусков нет: {report:?}"
        );
        assert!(report.diagnostics_text().contains("попадание в диапазон"));
    }

    /// Пишет golden-set (два документа + эталоны) и рубрику во временный каталог.
    fn golden_dirs() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let dir = tempfile::tempdir().expect("tempdir");
        let rubrics = dir.path().join("rubrics");
        let golden = dir.path().join("golden");
        std::fs::create_dir_all(&rubrics).expect("mkdir");
        std::fs::create_dir_all(&golden).expect("mkdir");
        std::fs::write(rubrics.join("core.yaml"), RUBRIC_YAML).expect("rubric");
        std::fs::write(golden.join("bad.md"), GOLDEN_BAD_MD).expect("doc");
        std::fs::write(golden.join("bad.expected.yaml"), GOLDEN_BAD_YAML).expect("exp");
        std::fs::write(golden.join("good.md"), GOLDEN_GOOD_MD).expect("doc");
        std::fs::write(golden.join("good.expected.yaml"), GOLDEN_GOOD_YAML).expect("exp");
        (dir, rubrics, golden)
    }

    /// JSON-ответ судьи для golden-фикстуры.
    fn judge_reply(
        context: u8,
        risks: u8,
        context_rationale: &str,
        risks_rationale: &str,
    ) -> String {
        format!(
            "{{\"scores\": [\
             {{\"criterion_id\": \"context\", \"score\": {context}, \"rationale\": \"{context_rationale}\"}}, \
             {{\"criterion_id\": \"risks\", \"score\": {risks}, \"rationale\": \"{risks_rationale}\"}}], \
             \"verdict\": \"ok\"}}"
        )
    }

    #[test]
    fn load_golden_reads_sorted_pairs() {
        let (_dir, _rubrics, golden) = golden_dirs();
        let cases = load_golden(&golden).expect("load_golden");
        assert_eq!(cases.len(), 2);
        assert!(cases[0].0.ends_with("bad.md"), "сортировка по имени файла");
        assert!(cases[0].1.scores["context"].contains(1));
        assert_eq!(cases[1].1.rubric, "core");
        assert_eq!(cases[1].1.scores.len(), 2);
    }

    /// Диапазоны эталона (ADR-051, S4): смысловая оценка не точка, но старый
    /// формат со скалярами обязан читаться как прежде.
    #[test]
    fn golden_range_parses_point_and_span() {
        let doc = "rubric: r\nkind: clean\nscores:\n  a: 4\n  b: {min: 1, max: 2}\n";
        let exp: GoldenExpectation = serde_yaml_ng::from_str(doc).expect("эталон");
        assert_eq!(exp.kind, GoldenKind::Clean);
        assert!(exp.scores["a"].contains(4) && !exp.scores["a"].contains(3));
        assert!(exp.scores["b"].contains(1) && exp.scores["b"].contains(2));
        assert!(!exp.scores["b"].contains(3));
        assert!((exp.scores["b"].midpoint() - 1.5).abs() < f64::EPSILON);
        // Метка по умолчанию — «без метки»: старые эталоны читаются.
        let plain: GoldenExpectation =
            serde_yaml_ng::from_str("rubric: r\nscores:\n  a: 5\n").expect("эталон");
        assert_eq!(plain.kind, GoldenKind::Unlabeled);
    }

    #[test]
    fn load_golden_rejects_broken_inputs() {
        let dir = tempfile::tempdir().expect("tempdir");
        let golden = dir.path().join("golden");
        std::fs::create_dir_all(&golden).expect("mkdir");
        // Эталон без документа — ошибка, а не пропуск.
        std::fs::write(golden.join("orphan.expected.yaml"), GOLDEN_BAD_YAML).expect("write");
        let err = load_golden(&golden).expect_err("эталон-сирота");
        assert!(err.to_string().contains("нет документа"), "{err}");
        // Балл 0 вне шкалы.
        std::fs::write(golden.join("orphan.md"), "текст").expect("write");
        std::fs::write(
            golden.join("zero.expected.yaml"),
            "rubric: core\nscores:\n  context: 0\n",
        )
        .expect("write");
        std::fs::write(golden.join("zero.md"), "текст").expect("write");
        let err = load_golden(&golden).expect_err("балл 0");
        assert!(err.to_string().contains("вне шкалы"), "{err}");
        // Битый YAML.
        std::fs::remove_file(golden.join("zero.expected.yaml")).expect("rm");
        std::fs::remove_file(golden.join("orphan.expected.yaml")).expect("rm");
        std::fs::write(golden.join("broken.expected.yaml"), "rubric: [unclosed").expect("write");
        std::fs::write(golden.join("broken.md"), "текст").expect("write");
        assert!(load_golden(&golden).is_err(), "битый yaml — ошибка");
    }

    #[tokio::test]
    async fn run_golden_computes_mae_against_expectations() {
        let (_dir, rubrics, golden) = golden_dirs();
        let llm = QueueLlm::new(&[
            // bad.md идёт первым по сортировке: судья совпал с эталоном.
            judge_reply(
                1,
                1,
                "свидетельство отсутствует",
                "свидетельство отсутствует",
            ),
            // good.md: судья занизил context на балл, risks угадал.
            judge_reply(
                4,
                5,
                "Цитата: \\\"Контекст описан полностью\\\" — почти полный",
                "Цитата: \\\"Риски названы и разобраны подробно\\\" — разобраны",
            ),
        ]);
        let cfg = JudgeConfig {
            samples: 1,
            ..JudgeConfig::default()
        };
        let report = run_golden(&llm, &rubrics, &golden, &cfg)
            .await
            .expect("golden");
        assert_eq!(report.judge_model, "fake-queue");
        assert_eq!(report.compared, 4, "2 документа × 2 критерия");
        assert_eq!(report.cases.len(), 2);
        assert_eq!(report.cases[0].doc, "bad.md");
        assert!(
            (report.cases[0].mae - 0.0).abs() < 1e-9,
            "точное совпадение"
        );
        assert_eq!(report.cases[1].doc, "good.md");
        assert!(
            (report.cases[1].mae - 0.5).abs() < 1e-9,
            "(|4−5| + |5−5|) / 2"
        );
        assert!((report.mae - 0.25).abs() < 1e-9, "итоговый MAE по 4 парам");
        // MAE по критериям: context — (|1−1| + |4−5|)/2, risks — 0.
        assert_eq!(report.criterion_mae.len(), 2);
        assert_eq!(report.criterion_mae[0].criterion_id, "context");
        assert!((report.criterion_mae[0].mae - 0.5).abs() < 1e-9);
        assert_eq!(report.criterion_mae[0].compared, 2);
        assert_eq!(report.criterion_mae[1].criterion_id, "risks");
        assert!((report.criterion_mae[1].mae - 0.0).abs() < 1e-9);
        // Взвешенные баллы: bad — 1.0/1.0, good — 4.5/5.0.
        assert!((report.cases[0].weighted_judge - 1.0).abs() < 1e-9);
        assert!((report.cases[0].weighted_expected - 1.0).abs() < 1e-9);
        assert!((report.cases[1].weighted_judge - 4.5).abs() < 1e-9);
        assert!((report.cases[1].weighted_expected - 5.0).abs() < 1e-9);
        assert_eq!(report.cases[0].doc_chars, GOLDEN_BAD_MD.chars().count());
        // Два документа разной длины, баллы растут с длиной: обе ρ = 1.0.
        let bias = report.length_bias.expect("length bias при 2 документах");
        assert!(
            (bias.judge_rho - 1.0).abs() < 1e-9,
            "ρ судьи: {}",
            bias.judge_rho
        );
        assert!(
            (bias.expected_rho - 1.0).abs() < 1e-9,
            "ρ эталона: {}",
            bias.expected_rho
        );
        // Диагностика попадает в текст отчёта.
        let text = report.diagnostics_text();
        assert!(text.contains("MAE по критериям:"), "{text}");
        assert!(text.contains("context"), "{text}");
        assert!(
            text.contains("length bias: судья ρ=1.00, эталон ρ=1.00"),
            "{text}"
        );
    }

    #[tokio::test]
    async fn run_golden_length_bias_none_for_single_doc() {
        let dir = tempfile::tempdir().expect("tempdir");
        let rubrics = dir.path().join("rubrics");
        let golden = dir.path().join("golden");
        std::fs::create_dir_all(&rubrics).expect("mkdir");
        std::fs::create_dir_all(&golden).expect("mkdir");
        std::fs::write(rubrics.join("core.yaml"), RUBRIC_YAML).expect("rubric");
        std::fs::write(golden.join("solo.md"), GOLDEN_BAD_MD).expect("doc");
        std::fs::write(golden.join("solo.expected.yaml"), GOLDEN_BAD_YAML).expect("exp");
        let llm = QueueLlm::new(&[judge_reply(
            1,
            1,
            "свидетельство отсутствует",
            "свидетельство отсутствует",
        )]);
        let cfg = JudgeConfig {
            samples: 1,
            ..JudgeConfig::default()
        };
        let report = run_golden(&llm, &rubrics, &golden, &cfg)
            .await
            .expect("golden");
        assert_eq!(report.cases.len(), 1);
        assert!(
            report.length_bias.is_none(),
            "один документ — корреляция не определена"
        );
        assert!(
            !report.diagnostics_text().contains("length bias"),
            "строка length bias не печатается"
        );
        // MAE по критериям при этом считается и по одному документу.
        assert_eq!(report.criterion_mae.len(), 2);
    }

    #[test]
    fn spearman_math() {
        // Монотонная зависимость → 1.0, обратная → −1.0.
        let xs = [1.0, 2.0, 3.0, 4.0];
        let up = [10.0, 20.0, 30.0, 40.0];
        let down = [40.0, 30.0, 20.0, 10.0];
        assert!((spearman(&xs, &up).expect("ρ") - 1.0).abs() < 1e-9);
        assert!((spearman(&xs, &down).expect("ρ") + 1.0).abs() < 1e-9);
        // Связи усредняют ранги: xs ранжируется в [1.5, 1.5, 3, 4].
        let tied = [1.0, 1.0, 2.0, 3.0];
        let rho = spearman(&tied, &up).expect("ρ со связями");
        let expected = 4.5 / (4.5_f64 * 5.0).sqrt();
        assert!(
            (rho - expected).abs() < 1e-9,
            "ρ: {rho}, ожидалось {expected}"
        );
        // Неопределённые случаи — None.
        assert_eq!(spearman(&[1.0], &[2.0]), None, "n < 2");
        assert_eq!(spearman(&[], &[]), None, "пустые входы");
        assert_eq!(spearman(&xs, &up[..3]), None, "разная длина");
        assert_eq!(
            spearman(&[2.0, 2.0, 2.0], &up[..3]),
            None,
            "нулевая дисперсия"
        );
    }

    #[tokio::test]
    async fn run_golden_rejects_unknown_criterion_in_expectation() {
        let (_dir, rubrics, golden) = golden_dirs();
        // Ломаем эталон good.md: критерий, которого нет в рубрике core.
        std::fs::write(
            golden.join("good.expected.yaml"),
            "rubric: core\nscores:\n  context: 5\n  ghost: 4\n",
        )
        .expect("write");
        let llm = QueueLlm::new(&[
            judge_reply(
                1,
                1,
                "свидетельство отсутствует",
                "свидетельство отсутствует",
            ),
            judge_reply(
                1,
                1,
                "свидетельство отсутствует",
                "свидетельство отсутствует",
            ),
        ]);
        let cfg = JudgeConfig {
            samples: 1,
            ..JudgeConfig::default()
        };
        let err = run_golden(&llm, &rubrics, &golden, &cfg)
            .await
            .expect_err("критерий-призрак — ошибка данных");
        assert!(err.to_string().contains("ghost"), "{err}");
    }

    /// Live-прогон golden-set репозитория: `cargo test -- --ignored golden_live`.
    #[tokio::test]
    #[ignore = "нужен API-ключ и сеть: живой прогон судьи по golden-set репозитория"]
    async fn golden_live_repo_set() {
        let cfg = crate::config::Config::load(None).expect("config");
        let registry = crate::llm::LlmRegistry::from_config(&cfg).expect("registry");
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let report = run_golden(
            registry.default().as_ref(),
            &root.join("assets/rubrics"),
            &root.join("assets/benchmarks/golden"),
            &cfg.judge,
        )
        .await
        .expect("golden run");
        assert!(
            report.compared >= 30,
            "пар документ×критерий: {}",
            report.compared
        );
        eprintln!("golden MAE = {:.2}", report.mae);
    }

    /// Детерминированный golden-прогон фикстуры (судья: bad — точно,
    /// good — context занижен на балл).
    async fn golden_fixture_report() -> (tempfile::TempDir, GoldenReport) {
        let (dir, rubrics, golden) = golden_dirs();
        let llm = QueueLlm::new(&[
            judge_reply(
                1,
                1,
                "свидетельство отсутствует",
                "свидетельство отсутствует",
            ),
            judge_reply(
                4,
                5,
                "Цитата: \\\"Контекст описан полностью\\\" — почти полный",
                "Цитата: \\\"Риски названы и разобраны подробно\\\" — разобраны",
            ),
        ]);
        let cfg = JudgeConfig {
            samples: 1,
            ..JudgeConfig::default()
        };
        let report = run_golden(&llm, &rubrics, &golden, &cfg)
            .await
            .expect("golden");
        (dir, report)
    }

    #[tokio::test]
    async fn record_golden_appends_valid_json_lines() {
        let (_dir, report) = golden_fixture_report().await;
        let journal = tempfile::NamedTempFile::new().expect("journal");
        let first = GoldenRecord::from_report(&report, "2026-09-01".to_string());
        let second = GoldenRecord::from_report(&report, "2026-09-04".to_string());
        record_golden(journal.path(), &first).expect("record 1");
        record_golden(journal.path(), &second).expect("record 2");
        let text = std::fs::read_to_string(journal.path()).expect("read");
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2, "append не затирает прежние записи");
        let parsed: GoldenRecord = serde_json::from_str(lines[0]).expect("валидный JSON");
        assert_eq!(parsed.date, "2026-09-01");
        assert_eq!(parsed.model, "fake-queue");
        assert!((parsed.mae - 0.25).abs() < 1e-9);
        assert_eq!(parsed.compared, 4);
        assert_eq!(parsed.docs, 2);
        assert!((parsed.criterion_mae["context"] - 0.5).abs() < 1e-9);
        assert!((parsed.criterion_mae["risks"] - 0.0).abs() < 1e-9);
        assert!((parsed.judge_rho.expect("rho судьи") - 1.0).abs() < 1e-9);
        assert!((parsed.expected_rho.expect("rho эталона") - 1.0).abs() < 1e-9);
        // Контрактные ключи JSON-строки.
        let raw: serde_json::Value = serde_json::from_str(lines[1]).expect("json");
        for key in [
            "date",
            "model",
            "mae",
            "compared",
            "criterion_mae",
            "judge_rho",
            "expected_rho",
            "docs",
        ] {
            assert!(raw.get(key).is_some(), "нет ключа '{key}': {raw}");
        }
        assert_eq!(raw["date"], "2026-09-04");
    }

    #[tokio::test]
    async fn record_golden_writes_null_rho_when_bias_undefined() {
        let dir = tempfile::tempdir().expect("tempdir");
        let rubrics = dir.path().join("rubrics");
        let golden = dir.path().join("golden");
        std::fs::create_dir_all(&rubrics).expect("mkdir");
        std::fs::create_dir_all(&golden).expect("mkdir");
        std::fs::write(rubrics.join("core.yaml"), RUBRIC_YAML).expect("rubric");
        std::fs::write(golden.join("solo.md"), GOLDEN_BAD_MD).expect("doc");
        std::fs::write(golden.join("solo.expected.yaml"), GOLDEN_BAD_YAML).expect("exp");
        let llm = QueueLlm::new(&[judge_reply(
            1,
            1,
            "свидетельство отсутствует",
            "свидетельство отсутствует",
        )]);
        let cfg = JudgeConfig {
            samples: 1,
            ..JudgeConfig::default()
        };
        let report = run_golden(&llm, &rubrics, &golden, &cfg)
            .await
            .expect("golden");
        let entry = GoldenRecord::from_report(&report, "2026-09-04".to_string());
        let raw = serde_json::to_string(&entry).expect("json");
        assert!(raw.contains("\"judge_rho\":null"), "{raw}");
        assert!(raw.contains("\"expected_rho\":null"), "{raw}");
    }

    #[tokio::test]
    async fn load_golden_history_reads_and_skips_broken_lines() {
        let (_dir, report) = golden_fixture_report().await;
        let journal = tempfile::NamedTempFile::new().expect("journal");
        let good = serde_json::to_string(&GoldenRecord::from_report(
            &report,
            "2026-09-01".to_string(),
        ))
        .expect("json");
        std::fs::write(
            journal.path(),
            format!("{good}\n{{битая строка\n\n{good}\n"),
        )
        .expect("write");
        let (records, broken) = load_golden_history(journal.path()).expect("history");
        assert_eq!(records.len(), 2, "пустая строка молча пропущена");
        assert_eq!(broken, 1, "одна битая строка");
        // Отсутствующий файл — понятная ошибка.
        let missing = journal.path().with_file_name("no-such-journal.jsonl");
        let err = load_golden_history(&missing).expect_err("нет файла");
        assert!(err.to_string().contains("не найден"), "{err}");
    }

    #[test]
    fn golden_history_markdown_renders_table() {
        let records = vec![
            GoldenRecord {
                date: "2026-09-01".into(),
                model: "glm-5.3".into(),
                mae: 0.42,
                compared: 45,
                criterion_mae: BTreeMap::new(),
                judge_rho: Some(0.93),
                expected_rho: Some(0.71),
                docs: 9,
            },
            GoldenRecord {
                date: "2026-09-04".into(),
                model: "deepseek-v4-flash".into(),
                mae: 0.5,
                compared: 10,
                criterion_mae: BTreeMap::new(),
                judge_rho: None,
                expected_rho: None,
                docs: 1,
            },
        ];
        let md = golden_history_markdown(&records);
        assert!(
            md.contains("| Дата | Модель | MAE | Docs | Судья ρ | Эталон ρ |"),
            "{md}"
        );
        assert!(
            md.contains("| 2026-09-01 | glm-5.3 | 0.42 | 9 | 0.93 | 0.71 |"),
            "{md}"
        );
        assert!(
            md.contains("| 2026-09-04 | deepseek-v4-flash | 0.50 | 1 | — | — |"),
            "неопределённая ρ — прочерк: {md}"
        );
    }

    /// Пишет анкеты двух участников к двум документам golden-фикстуры.
    fn human_dirs(dir: &Path) -> PathBuf {
        let humans = dir.join("humans");
        std::fs::create_dir_all(&humans).expect("mkdir");
        let write = |name: &str, context: u8, risks: u8| {
            std::fs::write(
                humans.join(name),
                format!("rubric: core\nscores:\n  context: {context}\n  risks: {risks}\n"),
            )
            .expect("анкета");
        };
        write("bad.ivanov.expected.yaml", 1, 2);
        write("bad.petrov.expected.yaml", 2, 1);
        write("good.ivanov.expected.yaml", 5, 4);
        write("good.petrov.expected.yaml", 4, 5);
        humans
    }

    #[test]
    fn human_agreement_computes_medians_mae_and_verdict() {
        let (dir, _rubrics, golden) = golden_dirs();
        let humans = human_dirs(dir.path());
        let report = human_agreement(&golden, &humans).expect("agreement");
        assert_eq!(report.docs.len(), 2);
        assert_eq!(report.skipped, [] as [std::string::String; 0]);
        // Медианы: bad — 1.5/1.5, good — 4.5/4.5.
        let bad = &report.docs[0];
        assert_eq!(bad.doc, "bad.md");
        assert!((bad.medians["context"] - 1.5).abs() < 1e-9);
        assert!((bad.medians["risks"] - 1.5).abs() < 1e-9);
        assert_eq!(bad.humans.len(), 2);
        let good = &report.docs[1];
        assert!((good.medians["context"] - 4.5).abs() < 1e-9);
        // (а): (0.5 + 0.5 + 0.5 + 0.5) / 4 = 0.5.
        assert_eq!(report.compared, 4);
        assert!((report.golden_vs_humans_mae - 0.5).abs() < 1e-9);
        // (б): все восемь пар |человек − медиана| = 0.5 → 0.5.
        assert!((report.inter_human_mae - 0.5).abs() < 1e-9);
        // (в): 0.5 <= 0.5 — в пределах разброса.
        assert!(
            report.verdict().contains("в пределах"),
            "{}",
            report.verdict()
        );
        let md = report.to_markdown();
        assert!(
            md.contains("| Документ | Критерий | Медиана людей | Эталон golden | Δ |"),
            "{md}"
        );
        assert!(md.contains("| bad.md | context | 1.5 | 1 | 0.5 |"), "{md}");
        assert!(
            md.contains("(а) MAE эталон golden vs медиана людей: 0.50 (4 пар)"),
            "{md}"
        );
        assert!(md.contains("(в) Вердикт:"), "{md}");
    }

    #[test]
    fn human_agreement_verdict_flags_divergence_above_spread() {
        let (dir, _rubrics, golden) = golden_dirs();
        let humans = human_dirs(dir.path());
        // Эталон bad завышен (3/3 при медиане людей 1.5/1.5): расхождение
        // эталона с людьми выше межчеловеческого разброса.
        std::fs::write(
            golden.join("bad.expected.yaml"),
            "rubric: core\nscores:\n  context: 3\n  risks: 3\n",
        )
        .expect("exp");
        let report = human_agreement(&golden, &humans).expect("agreement");
        // (а): bad даёт 1.5+1.5, good 0.5+0.5 → 4/4 = 1.0; (б) = 0.5.
        assert!((report.golden_vs_humans_mae - 1.0).abs() < 1e-9);
        assert!(report.verdict().contains("ВЫШЕ"), "{}", report.verdict());
    }

    #[test]
    fn human_agreement_skips_doc_without_humans() {
        let (dir, _rubrics, golden) = golden_dirs();
        let humans = human_dirs(dir.path());
        std::fs::remove_file(humans.join("good.ivanov.expected.yaml")).expect("rm");
        std::fs::remove_file(humans.join("good.petrov.expected.yaml")).expect("rm");
        let report = human_agreement(&golden, &humans).expect("agreement");
        assert_eq!(report.docs.len(), 1, "good.md без анкет — пропущен");
        assert_eq!(report.skipped, vec!["good.md".to_string()]);
    }

    #[test]
    fn human_agreement_rejects_incomplete_participant() {
        let (dir, _rubrics, golden) = golden_dirs();
        let humans = human_dirs(dir.path());
        // Иванов не оценил risks к bad.md — ошибка данных с именем файла.
        std::fs::write(
            humans.join("bad.ivanov.expected.yaml"),
            "rubric: core\nscores:\n  context: 1\n",
        )
        .expect("анкета");
        let err = human_agreement(&golden, &humans).expect_err("неполная анкета");
        assert!(
            err.to_string().contains("bad.ivanov.expected.yaml"),
            "{err}"
        );
        assert!(err.to_string().contains("неполный набор"), "{err}");
    }

    #[test]
    fn median_math() {
        assert_eq!(median(&[]), None);
        assert!((median(&[3.0, 1.0, 2.0]).expect("медиана") - 2.0).abs() < 1e-9);
        assert!((median(&[1.0, 2.0]).expect("медиана") - 1.5).abs() < 1e-9);
    }

    // ── Метрика 13: arch-diff-replay (волна D, ADR-072) ──────────────────────

    #[test]
    fn replay_norm_token_strips_one_prefix_and_keeps_last_segment() {
        // Срезается ровно ОДИН префикс, затем берётся последний сегмент пути.
        assert_eq!(replay_norm_token("dir:src/x"), "x");
        assert_eq!(replay_norm_token("svc:api-gateway"), "api-gateway");
        assert_eq!(
            replay_norm_token("sys:tracing-server:9411"),
            "tracing-server:9411",
            "двоеточие в имени системы сохраняется"
        );
        assert_eq!(
            replay_norm_token("dir:spring-petclinic-genai-service"),
            "spring-petclinic-genai-service"
        );
        assert_eq!(replay_norm_token("node:Foo/Bar"), "bar");
        // Отступ по краям срезается, но пробел ПОСЛЕ префикса сохраняется —
        // как в замороженном `replay.py` (`strip().lower()`, затем срез префикса).
        assert_eq!(replay_norm_token("  EDGE:API-Gateway "), "api-gateway");
        assert_eq!(replay_norm_token("edge: x"), " x");
        // Без префикса — последний сегмент и lower-case.
        assert_eq!(replay_norm_token("A/B/C"), "c");
        assert_eq!(replay_norm_token(""), "");
    }

    #[test]
    fn replay_norm_edge_joins_normalized_ends() {
        assert_eq!(
            replay_norm_edge("dir:a/api-gateway", "svc:genai-service"),
            "api-gateway->genai-service"
        );
    }

    #[test]
    fn replay_f1_math() {
        let b = |xs: &[&str]| xs.iter().map(|s| (*s).to_string()).collect::<BTreeSet<_>>();
        // Идеальное совпадение.
        let (f, i) = replay_f1(&b(&["a", "b"]), &b(&["a", "b"]));
        assert!((f - 1.0).abs() < 1e-9 && i == 2);
        // Половина всплывших лишние: surf 4, exp 2, inter 2 → p=0.5, r=1 → F1=2/3.
        let (f, i) = replay_f1(&b(&["a", "b", "x", "y"]), &b(&["a", "b"]));
        assert!((f - 2.0 / 3.0).abs() < 1e-9 && i == 2);
        // Пустая сторона → 0.
        assert_eq!(replay_f1(&b(&[]), &b(&["a"])), (0.0, 0));
        assert_eq!(replay_f1(&b(&["a"]), &b(&[])), (0.0, 0));
    }

    #[test]
    fn replay_is_nonempty_semantics() {
        // Пустой дифф, нулевой маршрут → непустой = нет.
        let empty = serde_json::json!({ "route": { "score": 0, "triggers": [] } });
        assert!(!replay_is_nonempty(&empty));
        // Непустой список узлов.
        let with_node = serde_json::json!({ "added_nodes": [{ "id": "svc:x" }] });
        assert!(replay_is_nonempty(&with_node));
        // Список, но пустой — всё ещё пусто.
        let empty_list = serde_json::json!({ "added_edges": [] });
        assert!(!replay_is_nonempty(&empty_list));
        // Сработал триггер маршрута.
        let trig =
            serde_json::json!({ "route": { "score": 0, "triggers": [{ "id": "new_component" }] } });
        assert!(replay_is_nonempty(&trig));
        // Счёт маршрута > 0.
        let score = serde_json::json!({ "route": { "score": 2, "triggers": [] } });
        assert!(replay_is_nonempty(&score));
        // Предложения/ADR также считаются.
        let adr = serde_json::json!({ "adrs_touched": ["ADR-001"] });
        assert!(replay_is_nonempty(&adr));
    }

    #[test]
    fn replay_surfaced_extracts_externals_and_edges() {
        let doc = serde_json::json!({
            "added_nodes": [
                { "id": "svc:genai-service" },
                { "id": "sys:openai" },
                { "id": "sys:azure-openai" }
            ],
            "added_edges": [
                { "from": "svc:api-gateway", "to": "svc:genai-service" }
            ]
        });
        let (ext, edges) = replay_surfaced(&doc);
        // Только `sys:`-узлы попадают во «внешние»; компоненты — нет.
        assert_eq!(
            ext.into_iter().collect::<Vec<_>>(),
            vec!["azure-openai".to_string(), "openai".to_string()]
        );
        assert_eq!(
            edges.into_iter().collect::<Vec<_>>(),
            vec!["api-gateway->genai-service".to_string()]
        );
    }

    #[test]
    fn replay_evaluate_significant_petclinic_genai() {
        // Синтетический дифф, имитирующий 79b527a после нормализации id узлов к
        // коротким именам (`svc:`): 5 ожидаемых состав-токенов совпали, но 2
        // ребра лишние → p=5/7, r=1 → F1 = 10/12 ≈ 0.8333 (≥ порога 0.80).
        let label = ReplaySignificant {
            repo: "petclinic".into(),
            sha: "79b527a35d2141f619386bb94a9e18355aab7880".into(),
            title: "genai".into(),
            expect_components: vec!["genai-service".into()],
            expect_external: vec!["openai".into(), "azure-openai".into()],
            expect_edges: vec![
                vec!["api-gateway".into(), "genai-service".into()],
                vec!["genai-service".into(), "vets-service".into()],
                vec!["genai-service".into(), "customers-service".into()],
            ],
        };
        let doc = serde_json::json!({
            "added_nodes": [
                { "id": "svc:genai-service" },
                { "id": "sys:openai" },
                { "id": "sys:azure-openai" }
            ],
            "added_edges": [
                { "from": "svc:api-gateway", "to": "svc:genai-service" },
                { "from": "svc:genai-service", "to": "svc:vets-service" },
                { "from": "svc:genai-service", "to": "svc:customers-service" },
                { "from": "svc:api-gateway", "to": "svc:vets-service" },
                { "from": "svc:api-gateway", "to": "svc:customers-service" }
            ]
        });
        let r = replay_evaluate_significant(&label, &doc);
        assert!(r.hit, "≥1 ожидаемый токен должен всплыть");
        assert!((r.f1 - 10.0 / 12.0).abs() < 1e-9, "f1={}", r.f1);
        assert_eq!(r.surf_comp.len(), 7);
        assert_eq!(r.exp_comp.len(), 5);
    }

    #[test]
    fn replay_report_aggregates_and_gates() {
        let mk_sig = |hit: bool, f1: f64| ReplaySigResult {
            repo: "r".into(),
            sha: "s".into(),
            title: String::new(),
            hit,
            f1,
            surf_comp: vec![],
            exp_comp: vec![],
        };
        let mk_noise = |nonempty: bool| ReplayNoiseResult {
            repo: "r".into(),
            sha: "s".into(),
            kind: "dependency".into(),
            nonempty,
        };
        // 1/2 recall, macro-F1 = (1.0+0.0)/2 = 0.5, шум 0/2.
        let report = ReplayReport::from_results(
            vec![mk_sig(true, 1.0), mk_sig(false, 0.0)],
            vec![mk_noise(false), mk_noise(false)],
            ReplayThresholds::default(),
            vec![],
        );
        assert!((report.recall_significant - 0.5).abs() < 1e-9);
        assert!((report.composition_f1 - 0.5).abs() < 1e-9);
        assert!((report.noise_ratio - 0.0).abs() < 1e-9);
        assert!(!report.passed(), "recall 0.5 < 0.80 → FAIL");
        assert!(report.summary().contains("FAIL"));

        // Проходной набор: recall 2/2, F1 (0.9+0.9)/2, шум 0/10.
        let noise: Vec<_> = (0..10).map(|_| mk_noise(false)).collect();
        let pass = ReplayReport::from_results(
            vec![mk_sig(true, 0.9), mk_sig(true, 0.9)],
            noise,
            ReplayThresholds::default(),
            vec![],
        );
        assert!(pass.passed());
        assert!(pass.summary().contains("PASS"));
    }

    #[test]
    fn replay_labels_yaml_parses_seq_of_seqs_and_thresholds() {
        let yaml = "\
version: \"2026.10\"
defaults:
  base: \"{sha}~1\"
  format: json
  thresholds:
    recall_significant: 0.80
    composition_f1: 0.80
    noise_ratio: 0.10
significant:
  - repo: petclinic
    sha: 79b527a35d2141f619386bb94a9e18355aab7880
    title: genai
    expect_components: [genai-service]
    expect_edges:
      - [api-gateway, genai-service]
      - [genai-service, vets-service]
    expect_external: [openai, azure-openai]
noise:
  - repo: petclinic
    sha: abc1234
    title: bump
    kind: dependency
";
        let labels: ReplayLabels = serde_yaml_ng::from_str(yaml).expect("разметка разобрана");
        assert_eq!(labels.significant.len(), 1);
        assert_eq!(labels.noise.len(), 1);
        assert_eq!(labels.significant[0].expect_edges.len(), 2);
        assert_eq!(
            labels.significant[0].expect_edges[0],
            vec!["api-gateway", "genai-service"]
        );
        assert!((labels.defaults.thresholds.composition_f1 - 0.80).abs() < 1e-9);
        assert_eq!(replay_base_ref(&labels.defaults, "deadbeef"), "deadbeef~1");
    }
}
