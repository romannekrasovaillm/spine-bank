//! Мутационное тестирование архитектурного пакета (`arch-be redteam`, W2 0.3.4).
//!
//! Отвечает на вопрос, который архитектор не может задать вручную: **насколько
//! мой пакет вообще защищён правилами?** Команда клонирует кейс во временный
//! каталог, засеивает по одному дефекту из каталога [`MUTATORS`], гоняет гейт и
//! печатает карту обнаружения: поймано гейтом / не поймано никем — с итоговой
//! долей (волна E 0.3.14 — раздельно по слоям «документы+модель» и «код», E2).
//!
//! Свойства, которые обязан держать инструмент:
//! - **read-only к исходному кейсу** — работает только с копией во временном
//!   каталоге (удаляется на `Drop`);
//! - **без сети** и без LLM: только детерминированный контур контроля (AD-2);
//! - **детерминированность** — порядок мутаторов фиксирован, вердикт не
//!   зависит от времени и абсолютных путей;
//! - **честность** — дефекты, которые механика не должна ловить (семантика
//!   решения), названы такими в отчёте, а не спрятаны в знаменатель.
//!
//! Разбиение модуля (0.3.14, лимит длины продуктового файла): `mutators` —
//! каталог [`MUTATORS`] и правки кейса-мутанта; `corpus` — внешний корпус
//! патчей (E3, эксперимент за флагом `--corpus`); здесь — типы ожидания и
//! слоя, прогон, отчёты и доли обнаружения.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::control::Route;
use crate::error::{HarnessError, Result};
use crate::gate::{self, GateOptions, GateOutcome, GateReport, GateRequirements, GateStatus};

pub mod corpus;
pub(crate) mod mutators;
pub(crate) use mutators::MUTATORS;

/// Что ожидается от мутатора.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Expectation {
    /// Дефект обязан быть пойман механикой.
    Caught,
    /// Дефект механикой не ловится и не должен — семантика решения
    /// (человеческое ревью, паспорт вердикта), а не проверка правил.
    Semantic,
    /// Контрольный мутатор: вердикт обязан остаться зелёным, но аттестация —
    /// измениться (иначе «зелёный» не привязан к состоянию).
    Control,
}

impl Expectation {
    /// Метка для отчёта.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Caught => "ловится",
            Self::Semantic => "семантика",
            Self::Control => "контроль",
        }
    }
}

/// Правка кейса-мутанта: `Ok(())` — применена, `Err(причина)` — вход не
/// найден (мутатор пропускается, а не считается «пойманным»).
pub type Mutation = fn(&Path) -> std::result::Result<(), String>;

/// Смысловой субъект мутанта: какую рубрику и какое досье брать, чтобы
/// дефект, не ловимымй механикой, измерил судья (ADR-051, S5).
///
/// Субъект задан парой «каталог + префикс имени», а не готовым путём: путь
/// зависит от кейса, а каталог мутанта — копия кейса. Резолвер берёт первый
/// подходящий файл в отсортированном порядке, поэтому прогон детерминирован.
pub struct SemanticSubject {
    /// Имя смысловой рубрики (файл в `assets/rubrics`).
    pub rubric: &'static str,
    /// Вид досье, которым собирается вход судьи.
    pub pack: crate::rubric_pack::PackKind,
    /// Каталог субъекта внутри кейса (`docs/adr`, `model`, `src/legacy`).
    pub dir: &'static str,
    /// Префикс имени файла субъекта (`ADR-`, `CMP-`, `payments.py`).
    pub prefix: &'static str,
}

/// Слой засеянного дефекта (E2, волна E 0.3.14): доля обнаружения считается
/// раздельно для документов+модели и для кода — текстовые правила реестра не
/// должны маскировать слепоту к кодовым дефектам (корпус
/// `experiments/openspec-vs-spine/`: классы нарушений агентов — кодовые).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layer {
    /// Документы и модель (спайн, ADR, DECISION, сущности model/).
    DocsModel,
    /// Код скелета/реализации (D11, D11b, D15, D18 и кодовые классы корпуса).
    Code,
}

impl Layer {
    /// Метка для отчёта.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::DocsModel => "документы+модель",
            Self::Code => "код",
        }
    }

    /// Машинная метка (JSON).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::DocsModel => "docs_model",
            Self::Code => "code",
        }
    }
}

/// Один мутатор: идентификатор, описание, ожидание и правка.
pub struct Mutator {
    /// Идентификатор из red-team набора (`D1`, `D11b`, `R`…).
    pub id: &'static str,
    /// Что засеваем.
    pub title: &'static str,
    /// Каким инструментом обязан ловиться (человеко-читаемая подсказка).
    pub by: &'static str,
    /// Ожидание. У мутаторов с [`Mutator::expected_in`] — дефолт для кейса без
    /// покрывающего правила (документация и статические проверки каталога);
    /// фактическое ожидание прогона вычисляется на мутанте.
    pub expected: Expectation,
    /// Входит ли мутатор в знаменатель доли обнаружения — в набор из 14
    /// позиций раздела 7 ТЗ (D1…D13 + D11b). `R` (ревью `NOT-READY`), `D14`
    /// (контроль аттестации) и `D15` (нарушение инварианта в реализации
    /// скелета) стоят в таблице отдельными строками: их результат виден в карте
    /// обнаружения, но в критерий приёмки «≥ 11 из 14» не входит.
    ///
    /// Для `D15` это не формальность: он проверяет не дефект пакета, а зубы
    /// применённого шаблона (правило `command_succeeds` обязано упасть на
    /// нарушающей реализации). Позиции раздела 7 мерят защищённость пакета;
    /// способность шаблона ловить нарушение — качество реестра, и складывать
    /// одно с другим значило бы менять смысл критерия приёмки.
    ///
    /// Мутаторы волны E (`D18`…`D24`) — тоже отдельные строки: они измеряют
    /// кодовый слой (E2) и классы корпуса `openspec-vs-spine`, а не набор
    /// раздела 7.
    pub in_ratio: bool,
    /// Смысловая рубрика, которой этот класс дефекта ловится (ADR-051, S5):
    /// у `D6`, `D10`, `D11` механика бессильна по построению, и измерение
    /// смыслового слоя — отдельная строка, в долю обнаружения не входящая.
    pub semantic: Option<SemanticSubject>,
    /// Слой дефекта (E2): `Code` — правка кода скелета/реализации.
    pub layer: Layer,
    /// Динамическое ожидание по составу правил кейса (B2/E1): `Some(f)` —
    /// ожидание вычисляется на мутанте после правки (правило класса дефекта
    /// есть в реестре и покрывает файл — `Caught`, иначе — `Semantic`).
    /// Честность по построению: «не пойман» на кейсе без правила класса —
    /// утверждение о реестре, а не о механике.
    pub expected_in: Option<fn(&Path) -> Expectation>,
    /// Правка кейса-мутанта.
    pub apply: Mutation,
}

// ---------------------------------------------------------------------------
// Прогон
// ---------------------------------------------------------------------------

/// Результат одного мутатора.
#[derive(Debug, Clone)]
pub struct Detection {
    /// Идентификатор мутатора.
    pub id: String,
    /// Что засевали.
    pub title: String,
    /// Ожидание (у мутаторов с динамическим ожиданием — вычисленное на
    /// мутанте по составу правил кейса, B2/E1).
    pub expected: Expectation,
    /// Слой засеянного дефекта (E2): документы+модель или код — доли
    /// обнаружения считаются раздельно.
    pub layer: Layer,
    /// Кем поймано: имена проваленных составляющих гейта; `None` — не поймано.
    pub caught_by: Option<String>,
    /// Почему мутатор пропущен (вход не найден) — честная причина вместо
    /// молчаливого «поймано».
    pub skipped: Option<String>,
    /// Каким инструментом ожидался (для строки «НЕ ПОЙМАН»).
    pub expected_by: String,
    /// Входит ли в набор из 14 позиций критерия приёмки.
    pub in_ratio: bool,
}

impl Detection {
    /// Пойман ли дефект.
    #[must_use]
    pub fn caught(&self) -> bool {
        self.caught_by.is_some()
    }
}

/// Отчёт мутационного прогона.
#[derive(Debug, Clone)]
pub struct RedteamReport {
    /// Кейс, который мутировали.
    pub case: PathBuf,
    /// Результаты в порядке каталога.
    pub detections: Vec<Detection>,
    /// Порог доли обнаружения, ниже которого прогон красный.
    pub min_detection: f64,
    /// Порог доли обнаружения КОДОВОГО слоя (E2, `[redteam]
    /// min_code_detection`): `None` — кодовая доля только показывается
    /// (дефолт; порог — решение архитектора, не зашит).
    pub min_code_detection: Option<f64>,
    /// Контрольный мутатор D14: аттестация изменилась при том же вердикте.
    pub control_ok: bool,
    /// Почему контроль не прошёл — с различием двух исходов, у которых разные
    /// выводы: «вердикт изменился» (безвредная правка не должна его менять) и
    /// «аттестация не изменилась» (вердикт не привязан к состоянию дерева).
    /// `None` — контроль пройден.
    pub control_note: Option<String>,
    /// Клоны смысловых мутантов, сохранённые `--keep-semantic` (ADR-051, S5):
    /// в них хост кладёт отчёты судьи, их читает `semantic-score`.
    pub semantic_kept: Vec<PathBuf>,
    /// Сводка корпусного прогона (E3, `redteam --corpus`): `Some` только у
    /// прогона по внешнему корпусу патчей. Позиции корпуса — отдельные
    /// строки вне знаменателя доли обнаружения; поле аддитивное, поведение
    /// стандартного прогона не меняется.
    pub corpus: Option<corpus::CorpusRunInfo>,
}

/// Сохранённый итог мутационного прогона (`.arch-handoff/redteam.json`,
/// пишет `redteam --save`): метрика доверия (`crate::trust`) читает ИЗМЕРЕННУЮ
/// долю, а не пересказ о ней — пересчитывать прогон при каждом `trust` было бы
/// и медленно, и нечестно (кейс мог измениться после измерения).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RedteamSummary {
    /// Схема файла.
    pub schema: String,
    /// Кейс, на котором измерено (подпись, в какой он был редакции).
    pub case: String,
    /// Момент измерения (RFC 3339).
    pub measured_at: String,
    /// Поймано дефектов (числитель доли).
    pub caught: usize,
    /// Дефектов в знаменателе.
    pub total: usize,
    /// Доля обнаружения `0..=1`.
    pub ratio: f64,
    /// Порог, при котором прогон считался пройденным.
    pub min_detection: f64,
    /// Контрольный мутатор: аттестация изменилась при том же вердикте.
    pub control_ok: bool,
    /// Почему контроль не прошёл (аддитивное поле схемы v1: файлы прежних
    /// редакций читаются, причина просто неизвестна). Метрика доверия
    /// показывает ИМЕННО её, а не свою догадку о причине.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub control_note: Option<String>,
    /// Доля обнаружения слоя «документы+модель» (E2; аддитивные поля схемы
    /// v1: файлы прежних редакций читаются, доли просто неизвестны).
    /// Считается по дефектам слоя, которые обязаны ловиться (ожидание
    /// `Caught`) и не пропущены; `None` — ловимых дефектов слоя не было.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub docs_model_ratio: Option<f64>,
    /// Поймано в слое «документы+модель».
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub docs_model_caught: Option<usize>,
    /// Ловимых дефектов в слое «документы+модель».
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub docs_model_total: Option<usize>,
    /// Доля обнаружения слоя «код» (E2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code_ratio: Option<f64>,
    /// Поймано в слое «код».
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code_caught: Option<usize>,
    /// Ловимых дефектов в слое «код».
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code_total: Option<usize>,
    /// Порог кодовой доли из конфига на момент измерения (`None` — не задан).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_code_detection: Option<f64>,
}

impl RedteamSummary {
    /// Прогон прошёл порог суммарной доли, контроль аттестации и — если
    /// задан — порог кодовой доли (E2): заданный порог при неизмеренной
    /// кодовой доле не проходит (требование не подтверждено).
    #[must_use]
    pub fn passed(&self) -> bool {
        let code_ok = match (self.min_code_detection, self.code_ratio) {
            (Some(min), Some(code)) => code + f64::EPSILON >= min,
            (Some(_), None) => false,
            (None, _) => true,
        };
        self.ratio >= self.min_detection && self.control_ok && code_ok
    }
}

/// Что стало со смысловым мутантом в глазах судьи.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SemanticVerdict {
    /// Главный критерий ≤ 2 и обвинение подтверждено — дефект пойман судьёй.
    Caught,
    /// Главный критерий выше 2: судья противоречия не увидел.
    Missed,
    /// Критерий низкий, но цитат нет — обвинение не подтверждено механикой.
    Unconfirmed,
    /// Отчёта судьи в клоне нет.
    NoReport,
    /// Отчёт есть, но досье изменилось после оценки — судить по нему нельзя.
    Stale,
}

impl SemanticVerdict {
    /// Метка для вывода.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Caught => "пойман",
            Self::Missed => "не пойман",
            Self::Unconfirmed => "обвинение не подтверждено",
            Self::NoReport => "нет отчёта",
            Self::Stale => "отчёт устарел",
        }
    }

    /// Считается ли пойманным (числитель смысловой строки).
    #[must_use]
    pub fn caught(self) -> bool {
        matches!(self, Self::Caught)
    }
}

/// Разбор одного смыслового клона.
#[derive(Debug, Clone)]
pub struct SemanticCaseScore {
    /// Мутант (`D10`).
    pub mutant: String,
    /// Рубрика, которой судили.
    pub rubric: String,
    /// Субъект досье.
    pub subject: String,
    /// Вердикт.
    pub verdict: SemanticVerdict,
    /// Судья (из отчёта), если он есть.
    pub judge: Option<String>,
    /// Судья — автор документа (независимость не подтверждена).
    pub judge_is_author: bool,
}

/// Итог смыслового слоя (ADR-051, S5): отдельная строка, **не** входящая
/// ни в долю обнаружения, ни в порог.
#[derive(Debug, Clone)]
pub struct SemanticScore {
    /// Разбор по клонам.
    pub cases: Vec<SemanticCaseScore>,
}

impl SemanticScore {
    /// Сколько дефектов поймал судья.
    #[must_use]
    pub fn caught(&self) -> usize {
        self.cases.iter().filter(|c| c.verdict.caught()).count()
    }

    /// Строка отчёта: «смысловой слой: поймано k из n; судья: …; независим: …».
    #[must_use]
    pub fn render(&self) -> String {
        let judges: Vec<&str> = self
            .cases
            .iter()
            .filter_map(|c| c.judge.as_deref())
            .collect();
        let judge = if judges.is_empty() {
            "нет".to_string()
        } else {
            let mut uniq: Vec<&str> = judges.clone();
            uniq.sort_unstable();
            uniq.dedup();
            uniq.join(", ")
        };
        // Независимость — общее утверждение, а не по кейсу: если хоть где-то
        // судья совпал с автором, «да» было бы неправдой.
        let independent = !self.cases.is_empty()
            && self
                .cases
                .iter()
                .all(|c| c.judge.is_some() && !c.judge_is_author);
        let mut out = format!(
            "смысловой слой: поймано {} из {}; судья: {judge}; независим: {}",
            self.caught(),
            self.cases.len(),
            if independent { "да" } else { "нет" }
        );
        for c in &self.cases {
            let _ = std::fmt::Write::write_fmt(
                &mut out,
                format_args!(
                    "\n  {} · {} · {} — {}",
                    c.mutant,
                    c.rubric,
                    c.subject,
                    c.verdict.label()
                ),
            );
        }
        out
    }
}

/// Читает отчёты судьи из сохранённых клонов и считает смысловую строку.
///
/// Пойман — главный критерий рубрики (`blocking`) с баллом ≤ 2 **и**
/// подтверждённым обвинением: отчёт без цитат механика сама исключает из
/// итога, и записывать это в поимку значило бы засчитывать выдуманное
/// свидетельство. Отчёт сверяется с досье по хэшу: изменился — «устарел».
///
/// # Errors
/// Каталог недоступен или задание `SEMANTIC-TODO.json` не разбирается.
pub fn semantic_score(dir: &Path, rubrics_dir: &Path) -> Result<SemanticScore> {
    if !dir.is_dir() {
        return Err(HarnessError::Control(format!(
            "каталог смысловых клонов недоступен: {}",
            dir.display()
        )));
    }
    let mut clones: Vec<PathBuf> = std::fs::read_dir(dir)
        .map_err(|e| HarnessError::io(dir, e))?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.join("SEMANTIC-TODO.json").is_file())
        .collect();
    clones.sort();
    let mut cases = Vec::new();
    for clone in &clones {
        let todo_path = clone.join("SEMANTIC-TODO.json");
        let text =
            std::fs::read_to_string(&todo_path).map_err(|e| HarnessError::io(&todo_path, e))?;
        let todo: serde_json::Value = serde_json::from_str(&text).map_err(|e| {
            HarnessError::Control(format!(
                "{}: задание не разбирается: {e}",
                todo_path.display()
            ))
        })?;
        let mutant = todo["mutant"].as_str().unwrap_or("?").to_string();
        let rubric_name = todo["rubric"].as_str().unwrap_or("").to_string();
        let subject = todo["subject"].as_str().unwrap_or("").to_string();
        cases.push(score_one_clone(
            clone,
            &mutant,
            &rubric_name,
            &subject,
            rubrics_dir,
        ));
    }
    if cases.is_empty() {
        return Err(HarnessError::Control(format!(
            "в {} нет сохранённых клонов с заданием SEMANTIC-TODO.json — \
             прогоните `redteam --keep-semantic <каталог>`",
            dir.display()
        )));
    }
    Ok(SemanticScore { cases })
}

/// Разбор одного клона: отчёт судьи по рубрике и субъекту, сверка с досье.
fn score_one_clone(
    clone: &Path,
    mutant: &str,
    rubric_name: &str,
    subject: &str,
    rubrics_dir: &Path,
) -> SemanticCaseScore {
    let base = SemanticCaseScore {
        mutant: mutant.to_string(),
        rubric: rubric_name.to_string(),
        subject: subject.to_string(),
        verdict: SemanticVerdict::NoReport,
        judge: None,
        judge_is_author: false,
    };
    let artifacts = crate::rubric::load_artifacts(clone);
    let Some(artifact) = artifacts
        .iter()
        .rev()
        .find(|a| a.rubric == rubric_name && a.subject.as_deref() == Some(subject))
    else {
        return base;
    };
    let judge = Some(artifact.judge_model.clone());
    let judge_is_author = artifact.author_model.as_deref() == Some(artifact.judge_model.as_str());
    let mut out = SemanticCaseScore {
        judge,
        judge_is_author,
        ..base
    };
    // Отчёт привязан к досье: пересобираем его в клоне и сверяем хэш — иначе
    // отчёт от прежней редакции читался бы как суждение о текущей. Отчёт БЕЗ
    // хэша досье (снят не по досье) устаревшим не объявляется: это не
    // расхождение, а отсутствие привязки, и судить о нём нечем.
    if let Some(want) = artifact.pack_sha256.as_deref() {
        let fresh =
            crate::rubric_pack::PackKind::parse(artifact.pack_kind.as_deref().unwrap_or_default())
                .ok()
                .and_then(|kind| crate::rubric_pack::build(clone, kind, subject).ok())
                .map(|packs| packs.iter().any(|p| p.sha256 == want));
        if fresh == Some(false) {
            out.verdict = SemanticVerdict::Stale;
            return out;
        }
    }
    // Главный критерий рубрики: без него измерять нечего.
    let Ok(rubric) = load_rubric_by_name(rubrics_dir, rubric_name) else {
        out.verdict = SemanticVerdict::NoReport;
        return out;
    };
    let Some(main) = rubric.criteria.iter().find(|c| c.blocking) else {
        out.verdict = SemanticVerdict::NoReport;
        return out;
    };
    out.verdict = match artifact_scores(artifact, &main.id) {
        None => SemanticVerdict::NoReport,
        Some((score, _)) if score > 2 => SemanticVerdict::Missed,
        Some((_, true)) => SemanticVerdict::Unconfirmed,
        Some(_) => SemanticVerdict::Caught,
    };
    out
}

/// Балл главного критерия и признак исключения из отчёта: обвинение без
/// подтверждённых цитат (`accusation_unconfirmed`) или без полного покрытия
/// (`coverage_incomplete`) механика исключает из итога — засчитывать это
/// поимкой значило бы записывать судье в заслугу выдуманное свидетельство.
fn artifact_scores(
    artifact: &crate::rubric::RubricArtifact,
    criterion_id: &str,
) -> Option<(u8, bool)> {
    let score = artifact
        .scores
        .iter()
        .find(|s| s.criterion_id == criterion_id)?;
    let excluded = score.flags.iter().any(|f| f.excludes_from_total());
    Some((score.score, excluded))
}

/// Рубрика по имени из каталога рубрик (ассеты конфига).
fn load_rubric_by_name(dir: &Path, name: &str) -> Result<crate::rubric::Rubric> {
    crate::rubric::load(&dir.join(format!("{name}.yaml")))
}

/// Записывает итог прогона в `<case>/.arch-handoff/redteam.json`.
///
/// # Errors
/// Каталог не создаётся либо файл не пишется.
pub fn save_summary(case: &Path, report: &RedteamReport) -> Result<PathBuf> {
    // Пишем в ИСХОДНЫЙ кейс, а не в `report.case`: прогон идёт в копии, и
    // сохранение «рядом с измерением» означало бы запись в каталог, который
    // тут же будет удалён. Метрика доверия читает `.arch-handoff/redteam.json`
    // именно исходного кейса — иначе `--save` выглядел бы рабочим, а
    // измерения не было бы ни у кого.
    let dir = case.join(".arch-handoff");
    std::fs::create_dir_all(&dir).map_err(|e| crate::error::HarnessError::io(&dir, e))?;
    let path = dir.join("redteam.json");
    let (docs_model_caught, docs_model_total) = report.layer_counts(Layer::DocsModel);
    let (code_caught, code_total) = report.layer_counts(Layer::Code);
    let summary = RedteamSummary {
        schema: "arch-be/redteam/v1".to_string(),
        case: case.display().to_string(),
        measured_at: chrono::Local::now().to_rfc3339(),
        caught: report.scored_caught(),
        total: report.scored_total(),
        ratio: report.detection_ratio(),
        min_detection: report.min_detection,
        control_ok: report.control_ok,
        control_note: report.control_note.clone(),
        // E2: доли по слоям — отдельно, порог 0.78 применяется к сумме.
        docs_model_ratio: report.layer_ratio(Layer::DocsModel),
        docs_model_caught: (docs_model_total > 0).then_some(docs_model_caught),
        docs_model_total: (docs_model_total > 0).then_some(docs_model_total),
        code_ratio: report.layer_ratio(Layer::Code),
        code_caught: (code_total > 0).then_some(code_caught),
        code_total: (code_total > 0).then_some(code_total),
        min_code_detection: report.min_code_detection,
    };
    let text = serde_json::to_string_pretty(&summary)
        .map_err(|e| crate::error::HarnessError::Config(format!("redteam: {e}")))?;
    std::fs::write(&path, text).map_err(|e| crate::error::HarnessError::io(&path, e))?;
    Ok(path)
}

/// Читает сохранённый итог; нет файла или он не разбирается — `None`
/// (метрика доверия не имеет права падать на чужом артефакте).
#[must_use]
pub fn load_summary(path: &Path) -> Option<RedteamSummary> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

impl RedteamReport {
    /// Дефекты, участвующие в доле обнаружения: входят в набор раздела 7 и
    /// не пропущены из-за отсутствия входа.
    fn scored(&self) -> Vec<&Detection> {
        self.detections
            .iter()
            .filter(|d| d.in_ratio && d.skipped.is_none())
            .collect()
    }

    /// Число дефектов в знаменателе доли.
    #[must_use]
    pub fn scored_total(&self) -> usize {
        self.scored().len()
    }

    /// Число пойманных дефектов (числитель доли): считаются только те, что
    /// обязаны ловиться, — случайно пойманная «семантика» долю не поднимает.
    #[must_use]
    pub fn scored_caught(&self) -> usize {
        self.scored()
            .iter()
            .filter(|d| d.expected == Expectation::Caught && d.caught())
            .count()
    }

    /// Доля обнаружения.
    ///
    /// Мутаторы, не нашедшие вход, из знаменателя исключаются — иначе
    /// неподходящий кейс выглядел бы «незащищённым» вместо «непроверенным».
    #[must_use]
    pub fn detection_ratio(&self) -> f64 {
        let total = self.scored_total();
        if total == 0 {
            return 1.0;
        }
        self.scored_caught() as f64 / total as f64
    }

    /// Числа по слою (E2): (поймано, ловимых) среди дефектов слоя с
    /// ожиданием `Caught` без пропуска входа. Семантические и контрольные
    /// позиции в слоевые доли не входят (они в карте отдельно), поэтому
    /// кодовая доля измеряет именно ловлю кодовых дефектов.
    #[must_use]
    pub fn layer_counts(&self, layer: Layer) -> (usize, usize) {
        let scoped: Vec<&Detection> = self
            .detections
            .iter()
            .filter(|d| {
                d.layer == layer && d.expected == Expectation::Caught && d.skipped.is_none()
            })
            .collect();
        let total = scoped.len();
        let caught = scoped.iter().filter(|d| d.caught()).count();
        (caught, total)
    }

    /// Доля обнаружения слоя; `None` — ловимых дефектов слоя в прогоне не
    /// было (честное «не измерялось», а не 100 %).
    #[must_use]
    pub fn layer_ratio(&self, layer: Layer) -> Option<f64> {
        let (caught, total) = self.layer_counts(layer);
        (total > 0).then(|| caught as f64 / total as f64)
    }

    /// Прогон прошёл порог суммарной доли, контроль аттестации и — если задан
    /// (E2, `[redteam] min_code_detection`) — порог кодовой доли.
    #[must_use]
    pub fn passed(&self) -> bool {
        let code_ok = match (self.min_code_detection, self.layer_ratio(Layer::Code)) {
            (Some(min), Some(code)) => code + f64::EPSILON >= min,
            // Порог задан, а измерения кодового слоя нет — требование не
            // подтверждено: не проходим.
            (Some(_), None) => false,
            (None, _) => true,
        };
        self.detection_ratio() >= self.min_detection && self.control_ok && code_ok
    }

    /// Строки долей по слоям (E2): документы+модель и код раздельно, с
    /// порогом кодовой доли, если он задан конфигом.
    #[must_use]
    pub fn render_layers(&self) -> String {
        let mut out = String::new();
        // Запись в String не может завершиться ошибкой — игноры безопасны.
        for layer in [Layer::DocsModel, Layer::Code] {
            match self.layer_ratio(layer) {
                Some(ratio) => {
                    let (caught, total) = self.layer_counts(layer);
                    let _ = writeln!(
                        out,
                        "  доля по слою «{}»: {caught}/{total} = {:.0}%",
                        layer.label(),
                        ratio * 100.0
                    );
                }
                None => {
                    let _ = writeln!(
                        out,
                        "  доля по слою «{}»: не измерялась (нет ловимых дефектов слоя)",
                        layer.label()
                    );
                }
            }
        }
        match self.min_code_detection {
            Some(min) => {
                let _ = writeln!(
                    out,
                    "  порог кодовой доли ([redteam] min_code_detection): {:.0}%",
                    min * 100.0
                );
            }
            None => {
                let _ = writeln!(
                    out,
                    "  порог кодовой доли: не задан (порог {:.0}% — к сумме, как прежде)",
                    self.min_detection * 100.0
                );
            }
        }
        out
    }

    /// Текстовый рендер «карты обнаружения».
    #[must_use]
    pub fn render(&self) -> String {
        let mut out = String::new();
        // Запись в String не может завершиться ошибкой — игноры безопасны.
        let _ = writeln!(
            out,
            "Мутационное тестирование пакета: {}",
            self.case.display()
        );
        // E3: корпусный прогон — шапка со сводкой патчей вместо строки
        // про каталог мутаторов (позиции корпуса — вне знаменателя доли).
        if let Some(info) = &self.corpus {
            out.push_str(&corpus::render_summary(info));
            out.push('\n');
        } else {
            let _ = writeln!(
                out,
                "Каталог мутаторов: {} (в долю входят только дефекты)",
                self.detections.len()
            );
            out.push('\n');
        }
        for d in &self.detections {
            let (mark, who) = match (&d.caught_by, d.skipped.as_deref()) {
                (_, Some(reason)) => ("—", format!("пропущен: {reason}")),
                (Some(by), None) if d.expected == Expectation::Control => {
                    ("✓", format!("контроль пройден: {by}, вердикт не изменился"))
                }
                (Some(by), None) if d.expected == Expectation::Semantic => {
                    ("!", format!("пойман ({by}) — а не должен: это семантика"))
                }
                (Some(by), None) => ("✓", format!("пойман: {by}")),
                (None, None) if d.expected == Expectation::Semantic => {
                    ("·", "не пойман и не должен — работа ревьюера".to_string())
                }
                (None, None) if d.expected == Expectation::Control => (
                    "✗",
                    format!(
                        "контроль не сработал: {}",
                        self.control_note
                            .as_deref()
                            .unwrap_or("причина не записана (файл прежней редакции)")
                    ),
                ),
                (None, None) => ("✗", format!("НЕ ПОЙМАН (ожидался: {})", d.expected_by)),
            };
            let _ = writeln!(out, "  [{mark}] {:<5} {:<52} {who}", d.id, d.title);
        }
        // E3: у корпусного прогона нет приёмочной доли и вердикта PASS/FAIL
        // (позиции вне знаменателя; порог — решение архитектора после
        // калибровки): вместо итога — доли по слоям корпуса и дисклеймер.
        if self.corpus.is_some() {
            let _ = writeln!(
                out,
                "\nКорпусные позиции в долю обнаружения не входят (отдельные строки, как D15+); \
                 прогон информационный — exit-код от доли поимки корпуса не зависит."
            );
            out.push_str(&self.render_layers());
            return out;
        }
        let _ = writeln!(
            out,
            "\nДоля обнаружения: {}/{} = {:.0}% (порог {:.0}%) · контроль аттестации: {}",
            self.scored_caught(),
            self.scored_total(),
            self.detection_ratio() * 100.0,
            self.min_detection * 100.0,
            if self.control_ok { "да" } else { "нет" }
        );
        // E2: две доли вместо одной — документы+модель и код раздельно.
        out.push_str(&self.render_layers());
        let _ = writeln!(out, "Итог: {}", if self.passed() { "PASS" } else { "FAIL" });
        out
    }

    /// Машинный отчёт (`--format json`).
    #[must_use]
    pub fn to_json(&self) -> serde_json::Value {
        let detections: Vec<serde_json::Value> = self
            .detections
            .iter()
            .map(|d| {
                serde_json::json!({
                    "id": d.id,
                    "title": d.title,
                    "expected": d.expected.label(),
                    "layer": d.layer.as_str(),
                    "caught": d.caught(),
                    "caught_by": d.caught_by,
                    "skipped": d.skipped,
                })
            })
            .collect();
        // E2: доли по слоям — аддитивный ключ; суммарный порог не меняется.
        let layers = [Layer::DocsModel, Layer::Code]
            .into_iter()
            .map(|layer| {
                let (caught, total) = self.layer_counts(layer);
                (
                    layer.as_str().to_string(),
                    serde_json::json!({
                        "caught": caught,
                        "total": total,
                        "ratio": self.layer_ratio(layer),
                    }),
                )
            })
            .collect::<serde_json::Map<String, serde_json::Value>>();
        // E3: сводка корпусного прогона — аддитивный ключ; у стандартного
        // прогона его нет, читатели схемы v1 не ломаются.
        let corpus = self.corpus.as_ref().map(|info| {
            serde_json::json!({
                "dir": info.dir.display().to_string(),
                "found": info.found,
                "applied": info.applied,
                "caught": info.caught,
                "not_applicable": info.not_applicable,
                "rejected": info.rejected,
            })
        });
        serde_json::json!({
            "schema": "arch-be/redteam-report/v1",
            "case": self.case.display().to_string(),
            "detections": detections,
            "detection_ratio": self.detection_ratio(),
            "caught": self.scored_caught(),
            "total": self.scored_total(),
            "min_detection": self.min_detection,
            "layers": layers,
            "min_code_detection": self.min_code_detection,
            "control_ok": self.control_ok,
            "control_note": self.control_note,
            "passed": self.passed(),
            "corpus": corpus,
        })
    }
}

/// Markdown-рендер отчёта (`--format markdown`).
#[must_use]
pub fn render_markdown(report: &RedteamReport) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "# Мутационное тестирование пакета\n");
    let _ = writeln!(out, "Кейс: `{}`\n", report.case.display());
    // E3: корпусный прогон — сводка патчей вместо итоговой доли (приёмочной
    // доли у корпуса нет: позиции вне знаменателя).
    if let Some(info) = &report.corpus {
        let _ = writeln!(out, "{}", corpus::render_summary(info));
    }
    // E3: у корпусного прогона приёмочной доли нет — строка о стандартном
    // наборе вводила бы в заблуждение; сводка корпуса напечатана выше.
    if report.corpus.is_none() {
        let _ = writeln!(
            out,
            "**Доля обнаружения: {}/{} = {:.0}%** (порог {:.0}%), контроль аттестации: {}\n",
            report.scored_caught(),
            report.scored_total(),
            report.detection_ratio() * 100.0,
            report.min_detection * 100.0,
            if report.control_ok {
                "пройден"
            } else {
                "не пройден"
            }
        );
    }
    // E2: доли по слоям — отдельными строками, порог суммарной доли не меняется.
    for layer in [Layer::DocsModel, Layer::Code] {
        match report.layer_ratio(layer) {
            Some(ratio) => {
                let (caught, total) = report.layer_counts(layer);
                let _ = writeln!(
                    out,
                    "Доля по слою «{}»: **{caught}/{total} = {:.0}%**\n",
                    layer.label(),
                    ratio * 100.0
                );
            }
            None => {
                let _ = writeln!(
                    out,
                    "Доля по слою «{}»: не измерялась (нет ловимых дефектов слоя)\n",
                    layer.label()
                );
            }
        }
    }
    let _ = writeln!(
        out,
        "| № | Дефект | Ожидание | Результат | Смысловая рубрика |\n|---|---|---|---|---|"
    );
    for d in &report.detections {
        let result = match (&d.caught_by, d.skipped.as_deref()) {
            (_, Some(reason)) => format!("пропущен: {reason}"),
            (Some(by), None) if d.expected == Expectation::Control => {
                format!("контроль пройден: {by}, вердикт не изменился")
            }
            (Some(by), None) if d.expected == Expectation::Semantic => {
                format!("пойман ({by}) — не должен")
            }
            (Some(by), None) => format!("пойман: {by}"),
            (None, None) if d.expected == Expectation::Semantic => {
                "не пойман и не должен".to_string()
            }
            (None, None) if d.expected == Expectation::Control => format!(
                "**контроль не сработал**: {}",
                report
                    .control_note
                    .as_deref()
                    .unwrap_or("причина не записана (файл прежней редакции)")
            ),
            (None, None) => "**не пойман**".to_string(),
        };
        // Смысловая колонка (ADR-051, S5): у классов, где механика бессильна
        // по построению, названа рубрика, которой дефект ловится судьёй, —
        // иначе «не пойман и не должен» читается как приговор без выхода.
        let semantic = MUTATORS
            .iter()
            .find(|m| m.id == d.id)
            .and_then(|m| m.semantic.as_ref())
            .map_or_else(|| "—".to_string(), |s| s.rubric.to_string());
        let _ = writeln!(
            out,
            "| {} | {} | {} | {} | {} |",
            d.id,
            d.title,
            d.expected.label(),
            result,
            semantic
        );
    }
    if !report.semantic_kept.is_empty() {
        let _ = writeln!(
            out,
            "\nСмысловой слой (в долю обнаружения не входит): сохранено клонов — {}. \
             Прогоните судью по заданию `SEMANTIC-TODO.json` в каждом и затем \
             `arch-be redteam semantic-score <каталог>`.",
            report.semantic_kept.len()
        );
    }
    out
}

/// Песочница мутационного прогона: временный каталог, удаляется на `Drop`.
struct Fixture {
    root: PathBuf,
    case: PathBuf,
}

impl Fixture {
    fn new(case: &Path) -> Result<Self> {
        if !case.is_dir() {
            return Err(HarnessError::Control(format!(
                "кейс недоступен: {}",
                case.display()
            )));
        }
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let root =
            std::env::temp_dir().join(format!("arch-be-redteam-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&root).map_err(|e| HarnessError::io(&root, e))?;
        Ok(Self {
            root,
            case: case.to_path_buf(),
        })
    }

    /// Свежая копия кейса под мутанта `id`.
    fn mutant(&self, id: &str) -> Result<PathBuf> {
        let dest = self.root.join(id);
        if dest.exists() {
            std::fs::remove_dir_all(&dest).map_err(|e| HarnessError::io(&dest, e))?;
        }
        std::fs::create_dir_all(&dest).map_err(|e| HarnessError::io(&dest, e))?;
        copy_tree(&self.case, &dest)?;
        Ok(dest)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        // Игнорируем: песочница во временном каталоге, уборка best-effort.
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// Копирует дерево, пропуская `.git` (мутанту делается свой репозиторий).
fn copy_tree(from: &Path, to: &Path) -> Result<()> {
    let rd = std::fs::read_dir(from).map_err(|e| HarnessError::io(from, e))?;
    for entry in rd.flatten() {
        let name = entry.file_name();
        if name == ".git" {
            continue;
        }
        let src = entry.path();
        let dst = to.join(&name);
        if src.is_dir() {
            std::fs::create_dir_all(&dst).map_err(|e| HarnessError::io(&dst, e))?;
            copy_tree(&src, &dst)?;
        } else if src.is_file() {
            std::fs::copy(&src, &dst).map_err(|e| HarnessError::io(&dst, e))?;
        }
    }
    Ok(())
}

/// Первый подходящий субъект смысловой рубрики в клоне — относительный путь.
/// Порядок сортировки делает выбор детерминированным.
fn resolve_semantic_subject(root: &Path, subject: &SemanticSubject) -> Option<String> {
    let rd = std::fs::read_dir(root.join(subject.dir)).ok()?;
    let mut names: Vec<String> = rd
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with(subject.prefix))
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    let name = names.into_iter().next()?;
    // Досье по сущностям адресуется ИДЕНТИФИКАТОРОМ (`CMP-001`), а не путём к
    // файлу: сборщик досье ищет сущность в модели и путь не разбирает (живой
    // прогон 2026-09-20 — `pack_subject_not_found` на файле).
    match subject.pack {
        crate::rubric_pack::PackKind::EntityLinks | crate::rubric_pack::PackKind::NfrMechanism => {
            let id: Vec<&str> = name.split('-').take(2).collect();
            (id.len() == 2).then(|| id.join("-"))
        }
        _ => Some(format!("{}/{}", subject.dir, name)),
    }
}

/// Копирует клон смыслового мутанта в `dest_root/<D-n>` и кладёт рядом
/// `SEMANTIC-TODO.json` — задание хосту (ADR-051, S5).
///
/// Смысловой дефект ловит не харнесс, а модель хоста, и ей нужен не только
/// клон, но и точный вход: вид досье и субъект. Задание собирается из
/// метаданных мутатора, а не пишется человеком, — иначе прогон судьи и
/// измерение разъезжались бы.
fn keep_semantic_clone(
    root: &Path,
    dest_root: &Path,
    m: &Mutator,
    subject: &SemanticSubject,
) -> Result<PathBuf> {
    let resolved = resolve_semantic_subject(root, subject).ok_or_else(|| {
        HarnessError::Control(format!(
            "{}: субъект смысловой рубрики '{}' не найден в клоне ({}/{}*) — \
             измерять нечего",
            m.id, subject.rubric, subject.dir, subject.prefix
        ))
    })?;
    let dest = dest_root.join(m.id);
    if dest.exists() {
        std::fs::remove_dir_all(&dest).map_err(|e| HarnessError::io(&dest, e))?;
    }
    std::fs::create_dir_all(&dest).map_err(|e| HarnessError::io(&dest, e))?;
    copy_tree(root, &dest)?;
    let todo = serde_json::json!({
        "schema": "arch-be/semantic-todo/v1",
        "mutant": m.id,
        "title": m.title,
        "expected": m.expected.label(),
        "rubric": subject.rubric,
        "pack": subject.pack.as_str(),
        "subject": resolved,
        "clone": dest.display().to_string(),
        "instructions": "Судит модель хоста (в ядре LLM нет): rubric_prompt с pack/subject/root \
                         → k независимых ответов → rubric_verify под --rw (отчёт ляжет в \
                         reports/rubric/ клона). Затем `arch-be redteam semantic-score <каталог>`. \
                         Смысловая строка в долю обнаружения не входит (ADR-051).",
    });
    let path = dest.join("SEMANTIC-TODO.json");
    let text = serde_json::to_string_pretty(&todo)
        .map_err(|e| HarnessError::Config(format!("redteam: задание семантики: {e}")))?;
    std::fs::write(&path, text).map_err(|e| HarnessError::io(&path, e))?;
    Ok(dest)
}

/// git-команда мутанта (идентичность коммиттера задаём явно: CI без
/// `user.email` иначе падает на `git commit`).
fn git(dir: &Path, args: &[&str]) -> std::result::Result<(), String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_AUTHOR_NAME", "redteam")
        .env("GIT_AUTHOR_EMAIL", "redteam@example.invalid")
        .env("GIT_COMMITTER_NAME", "redteam")
        .env("GIT_COMMITTER_EMAIL", "redteam@example.invalid")
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("git не запустился: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

/// Имя временной дельты, которой прикрывается контрольный мутант (D14).
const CONTROL_DELTA: &str = "redteam-control";

/// Прикрывает безвредную правку контрольного мутанта временной дельтой.
///
/// D14 — контроль: безвредная правка ТЕЛА сущности обязана оставить вердикт
/// зелёным, а аттестацию — изменить. Но правка защищённого пути без активной
/// дельты краснит `delta_guard` (и правильно краснит: это его работа), и на
/// кейсе, где все дельты заархивированы, контроль падал бы не на вердикте, а
/// на отсутствии дельты в свежей копии мутанта: «контроль аттестации: нет»
/// при доле выше порога и падение `trust` на ступень. Причина при этом не в
/// пакете, а в том, что мутант поставлен вне процесса дельт — любая настоящая
/// доработка приходит ВМЕСТЕ с дельтой (ADR-047).
///
/// Поэтому мутант ставится в положение обычной доработки: правка + дельта,
/// покрывающая изменённые защищённые пути. Дельта временная — живёт в копии
/// мутанта, гейту её достаточно, и она не касается разбора самих недостатков.
fn cover_control_mutation(root: &Path) -> std::result::Result<(), String> {
    let report = crate::delta::guard(root, Some("HEAD"), &[])
        .map_err(|e| format!("контроль: перечень защищённых правок: {e}"))?;
    if report.protected_changed.is_empty() {
        return Ok(());
    }
    let dir = root.join("changes").join(CONTROL_DELTA);
    std::fs::create_dir_all(&dir).map_err(|e| format!("контроль: {}: {e}", dir.display()))?;
    let mut body = String::from(
        "# Дельта: redteam-control\n\n\
         Временная дельта мутационного прогона (D14): покрывает безвредную \
         правку, чтобы вердикт мерил правку, а не отсутствие дельты. \
         К делу не относится и в исходный кейс не попадает.\n\n\
         ## MODIFIED\n\n",
    );
    for file in &report.protected_changed {
        let _ = writeln!(body, "- {file}");
    }
    let path = dir.join("DELTA.md");
    std::fs::write(&path, body).map_err(|e| format!("контроль: {}: {e}", path.display()))
}

/// Готовит мутанта: свой git-репозиторий, базовый коммит, правка, коммит
/// правки. База `HEAD~1` нужна, чтобы анти-ослабление правил видело правку
/// реестра, уже лежащую в коммите (Н5).
fn prepare(root: &Path) -> std::result::Result<(), String> {
    git(root, &["init", "-q"])?;
    git(root, &["add", "-A"])?;
    git(root, &["commit", "-q", "-m", "baseline"])?;
    Ok(())
}

/// Отчёт гейта по мутанту (для контроля D14 нужен вердикт и аттестация).
fn gate_report(root: &Path, decision_quality: bool) -> Result<GateReport> {
    let mut requirements = GateRequirements::default();
    if decision_quality {
        requirements.critical.push("decision_quality".to_string());
    }
    gate::run_opts(
        root,
        Some(Route::Critical),
        Some("HEAD~1"),
        None,
        (1, 4),
        &requirements,
        &GateOptions::default(),
    )
}

/// Мутационный прогон по кейсу.
///
/// # Errors
/// Кейс недоступен, git недоступен, гейт не смог отработать на эталоне.
/// Параметры мутационного прогона.
pub struct RedteamOptions {
    /// Порог доли обнаружения, ниже которого прогон красный.
    pub min_detection: f64,
    /// Порог доли обнаружения кодового слоя (E2): `None` — кодовая доля
    /// только показывается. CLI заполняет из `[redteam] min_code_detection`.
    pub min_code_detection: Option<f64>,
    /// Учитывать ли составляющую `decision_quality` при прогоне гейта.
    pub decision_quality: bool,
    /// Куда сохранить клоны смысловых мутантов (ADR-051, S5): `None` — клоны
    /// удаляются, как раньше.
    pub keep_semantic: Option<PathBuf>,
    /// Режим «герметичного контура» (B7): эталон, красный ТОЛЬКО из-за
    /// отсутствующего прогонщика (pytest/mvn/JDK), не считается сломанным, а
    /// мутаторы, которым нужен этот прогонщик, выходят из знаменателя как
    /// «пропущен: неизмеримо без прогонщика» — по образцу «нет входа».
    pub hermetic: bool,
}

impl Default for RedteamOptions {
    fn default() -> Self {
        Self {
            min_detection: 0.78,
            min_code_detection: None,
            decision_quality: true,
            keep_semantic: None,
            hermetic: false,
        }
    }
}

/// Прогон с параметрами по умолчанию (поведение 0.3.4).
///
/// # Errors
/// Кейс недоступен, не зелёный на маршруте Critical, git или гейт отказали.
pub fn run(case: &Path, min_detection: f64, decision_quality: bool) -> Result<RedteamReport> {
    run_with_options(
        case,
        &RedteamOptions {
            min_detection,
            decision_quality,
            ..RedteamOptions::default()
        },
    )
}

/// Причины «эталон не зелёный» для сообщения preflight: проваленные
/// составляющие и — отдельно — обязательные, оставшиеся без входа (SKIP на
/// Critical даёт INCOMPLETE: без них список был бы пустым, а причина отказа —
/// непонятной; случай hermetic-контура без pytest, B4 0.3.14).
pub(crate) fn not_green_reasons(reference: &crate::gate::GateReport) -> String {
    let mut parts: Vec<String> = reference
        .components
        .iter()
        .filter(|c| c.status == GateStatus::Fail)
        .map(|c| format!("провалена {}", c.name))
        .collect();
    parts.extend(reference.not_checked.iter().map(|name| {
        // B7: причина SKIP (недостающий прогонщик и команда установки) лежит
        // в `detail` составляющей; без неё сообщение глухо называет лишь имя —
        // «pytest не найден» архитектор видел только в `doctor`.
        let detail = reference
            .components
            .iter()
            .find(|c| c.name == name.as_str())
            .map(|c| c.detail.trim())
            .filter(|d| !d.is_empty());
        match detail {
            Some(d) => format!("обязательная {name} без входа (SKIP): {d}"),
            None => format!("обязательная {name} без входа (SKIP)"),
        }
    }));
    if parts.is_empty() {
        return format!("итог {:?} без названных составляющих", reference.outcome);
    }
    parts.join(", ")
}

/// Причина пропуска из-за отсутствующего прогонщика, извлечённая из `detail`
/// составляющей: от префикса [`crate::rule_templates::RUNNER_ABSENT_PREFIX`]
/// до служебного хвоста « — файл …». `None` — среди SKIP-составляющих нет
/// пропуска по прогонщику (пропуск иной природы).
pub(crate) fn runner_skip_note(report: &crate::gate::GateReport) -> Option<String> {
    report.components.iter().find_map(|c| {
        if c.status != GateStatus::Skip {
            return None;
        }
        let from = &c.detail[c.detail.find(crate::rule_templates::RUNNER_ABSENT_PREFIX)?..];
        Some(
            from.split(" — файл")
                .next()
                .unwrap_or(from)
                .trim()
                .to_string(),
        )
    })
}

/// Эталон красный ТОЛЬКО из-за отсутствующего прогонщика: исход INCOMPLETE,
/// ни одной FAIL-составляющей, и каждая непроверенная обязательная
/// составляющая — SKIP с причиной «нет прогонщика». Именно такой эталон
/// `--hermetic` (B7) вправе мерить: слепота окружения, а не сломанный кейс.
pub(crate) fn runner_only_incomplete(report: &crate::gate::GateReport) -> bool {
    if report.outcome != GateOutcome::Incomplete || report.not_checked.is_empty() {
        return false;
    }
    if report
        .components
        .iter()
        .any(|c| c.status == GateStatus::Fail)
    {
        return false;
    }
    report.not_checked.iter().all(|name| {
        report
            .components
            .iter()
            .find(|c| c.name == name.as_str())
            .is_some_and(|c| {
                c.status == GateStatus::Skip
                    && c.detail
                        .contains(crate::rule_templates::RUNNER_ABSENT_PREFIX)
            })
    })
}

/// Причина «пропущен: неизмеримо без прогонщика» для мутатора, который под
/// `--hermetic` не поймал ничего (`failed_empty`) из-за отсутствующего
/// прогонщика. `None` — обычный путь: режим выключен, дефект поймали или
/// вердикт красный не из-за прогонщика.
pub(crate) fn hermetic_skip(
    report: &crate::gate::GateReport,
    enabled: bool,
    failed_empty: bool,
) -> Option<String> {
    if !enabled || !failed_empty {
        return None;
    }
    runner_skip_note(report).map(|note| format!("неизмеримо без прогонщика: {note}"))
}

/// Мутационный прогон с опциями.
///
/// # Errors
/// Кейс недоступен, не зелёный на маршруте Critical, git или гейт отказали.
pub fn run_with_options(case: &Path, options: &RedteamOptions) -> Result<RedteamReport> {
    let min_detection = options.min_detection;
    let decision_quality = options.decision_quality;
    let fixture = Fixture::new(case)?;
    // Эталон: кейс без правок. Нужен и как проверка «кейс вообще зелёный»
    // (иначе доля обнаружения мерила бы сломанный кейс), и как база для
    // контроля D14.
    let reference_root = fixture.mutant("reference")?;
    prepare(&reference_root).map_err(HarnessError::Control)?;
    // Второй коммит обязателен: база прогона — `HEAD~1`, а на репозитории с
    // одним коммитом git отказывает, и delta_guard краснеет не по делу.
    let _ = crate::evidence::pack(&reference_root, Route::Critical);
    if let Err(e) = git(&reference_root, &["add", "-A"])
        .and_then(|()| git(&reference_root, &["commit", "-q", "-m", "reference"]))
    {
        return Err(HarnessError::Control(format!("эталон: коммит: {e}")));
    }
    let reference = gate_report(&reference_root, decision_quality)?;
    // B7: в `--hermetic` эталон, красный ИСКЛЮЧИТЕЛЬНО из-за отсутствующего
    // прогонщика, — не «сломанный пакет», а слепое окружение. Мерить его
    // можно; мутаторы без прогонщика выйдут из знаменателя ниже.
    let hermetic_runner_skip = options.hermetic && runner_only_incomplete(&reference);
    if reference.outcome != GateOutcome::Pass && !hermetic_runner_skip {
        return Err(HarnessError::Control(format!(
            "кейс {} не зелёный на маршруте Critical — мутационный прогон мерил бы \
             сломанный пакет ({}); красноглазый эталон не даёт отличить \
             «дефект пойман» от «пакет уже сломан»",
            case.display(),
            not_green_reasons(&reference)
        )));
    }
    let mut detections = Vec::new();
    let mut control_ok = true;
    let mut control_note: Option<String> = None;
    let mut kept: Vec<PathBuf> = Vec::new();
    for m in &MUTATORS {
        let root = fixture.mutant(m.id)?;
        if let Err(e) = prepare(&root) {
            return Err(HarnessError::Control(format!(
                "{}: подготовка мутанта: {e}",
                m.id
            )));
        }
        if let Err(reason) = (m.apply)(&root) {
            detections.push(Detection {
                id: m.id.to_string(),
                title: m.title.to_string(),
                expected: m.expected,
                layer: m.layer,
                caught_by: None,
                skipped: Some(reason),
                expected_by: m.by.to_string(),
                in_ratio: m.in_ratio,
            });
            continue;
        }
        // B2/E1: ожидание по составу правил кейса — вычисляется на мутанте
        // после правки (правило класса дефекта покрывает файл → Caught).
        let expected = m.expected_in.map_or(m.expected, |f| f(&root));
        // Бандл переупаковывается ПОСЛЕ правки: иначе любая правка удостоверенного
        // файла краснила бы evidence_verify как «изменён после упаковки», и
        // дефект ловился бы не тем инструментом, который проверяется.
        if m.expected == Expectation::Control {
            if let Err(e) = cover_control_mutation(&root) {
                return Err(HarnessError::Control(format!("{}: {e}", m.id)));
            }
        }
        let _ = crate::evidence::pack(&root, Route::Critical);
        if let Err(e) = git(&root, &["add", "-A"])
            .and_then(|()| git(&root, &["commit", "-q", "-m", &format!("mutant {}", m.id)]))
        {
            return Err(HarnessError::Control(format!("{}: коммит: {e}", m.id)));
        }
        // Смысловой мутант: клон сохраняется для хоста вместе с заданием
        // (ADR-051, S5) — судить его будет модель хоста, а не харнесс.
        if let (Some(subject), Some(dest_root)) = (&m.semantic, options.keep_semantic.as_deref()) {
            let dest = keep_semantic_clone(&root, dest_root, m, subject)?;
            kept.push(dest);
        }
        let report = gate_report(&root, decision_quality)?;
        let failed: Vec<String> = report
            .components
            .iter()
            .filter(|c| c.status == GateStatus::Fail)
            .map(|c| c.name.to_string())
            .collect();
        if m.expected == Expectation::Control {
            // Контроль: вердикт обязан остаться зелёным, аттестация — смениться.
            // Два исхода различимы и означают разное, поэтому и текст разный
            // (раньше «изменился вердикт» показывался как «аттестация X → Y»).
            let same_verdict = report.outcome == reference.outcome;
            let changed = report.attestation != reference.attestation;
            control_ok = control_ok && same_verdict && changed;
            let caught_by = if !same_verdict {
                let why = if failed.is_empty() {
                    "причина не в составляющих — сверьте вердикты".to_string()
                } else {
                    failed.join(", ")
                };
                control_note = Some(format!(
                    "безвредная правка изменила вердикт ({} → {}) — правка не должна \
                     менять вердикт: проверьте составляющие {why}",
                    reference.outcome.label(),
                    report.outcome.label()
                ));
                Some(format!("вердикт изменился: {why}"))
            } else if changed {
                Some(format!(
                    "аттестация {} → {}",
                    &reference.attestation[..12],
                    &report.attestation[..12]
                ))
            } else {
                control_note = Some(
                    "аттестация не изменилась при том же вердикте — вердикт не привязан \
                     к состоянию дерева (Н3)"
                        .to_string(),
                );
                None
            };
            detections.push(Detection {
                id: m.id.to_string(),
                title: m.title.to_string(),
                expected: m.expected,
                layer: m.layer,
                expected_by: m.by.to_string(),
                in_ratio: m.in_ratio,
                caught_by,
                skipped: None,
            });
            continue;
        }
        // B7: в hermetic-режиме мутатор, не поймавший ничего ИЗ-ЗА
        // отсутствующего прогонщика, честно выходит из знаменателя —
        // «пропущен: неизмеримо без прогонщика …», а не глухое «не пойман».
        if let Some(reason) = hermetic_skip(&report, options.hermetic, failed.is_empty()) {
            detections.push(Detection {
                id: m.id.to_string(),
                title: m.title.to_string(),
                expected,
                layer: m.layer,
                expected_by: m.by.to_string(),
                in_ratio: m.in_ratio,
                caught_by: None,
                skipped: Some(reason),
            });
            continue;
        }
        detections.push(Detection {
            id: m.id.to_string(),
            title: m.title.to_string(),
            expected,
            layer: m.layer,
            expected_by: m.by.to_string(),
            in_ratio: m.in_ratio,
            caught_by: if failed.is_empty() {
                None
            } else {
                Some(failed.join(", "))
            },
            skipped: None,
        });
    }
    Ok(RedteamReport {
        case: case.to_path_buf(),
        detections,
        min_detection,
        min_code_detection: options.min_code_detection,
        control_ok,
        control_note,
        semantic_kept: kept,
        corpus: None,
    })
}

#[cfg(test)]
mod tests {
    /// Н8 (T-08): контрольный мутант прикрыт временной дельтой. Без неё
    /// безвредная правка защищённого пути краснит `delta_guard` (его работа!),
    /// вердикт контрольного мутанта меняется — и контроль падает не на
    /// вердикте, а на отсутствии дельты в свежей копии мутанта: на кейсе с
    /// заархивированными дельтами «контроль аттестации: нет» при доле выше
    /// порога.
    #[test]
    fn control_mutation_is_covered_by_a_temporary_delta() {
        let tmp = tempfile::tempdir().expect("tmp");
        let repo = tmp.path().join("mutant");
        std::fs::create_dir_all(repo.join("model")).expect("mkdir");
        let entity = repo.join("model/CMP-001-оркестратор-операций.md");
        std::fs::write(&entity, "---\nid: CMP-001\n---\n\nкарточка\n").expect("write");
        git(&repo, &["init", "-q"]).expect("init");
        git(&repo, &["add", "-A"]).expect("add");
        git(&repo, &["commit", "-q", "-m", "baseline"]).expect("commit");
        // Безвредная правка ТЕЛА сущности — то, что делает D14.
        std::fs::write(
            &entity,
            "---\nid: CMP-001\n---\n\nкарточка\n\nУточнение формулировки без смены решения.\n",
        )
        .expect("edit");
        let before = crate::delta::guard(&repo, Some("HEAD"), &[]).expect("guard");
        assert_eq!(
            before.violations.len(),
            1,
            "до прикрытия — нарушение: {before:?}"
        );
        // Вход не найден (нет защищённых правок) — прикрытие не нужно.
        let clean = tmp.path().join("clean");
        std::fs::create_dir_all(clean.join("model")).expect("mkdir");
        std::fs::write(
            clean.join("model/CMP-001-карточка.md"),
            "---\nid: CMP-001\n---\n",
        )
        .expect("write");
        git(&clean, &["init", "-q"]).expect("init");
        git(&clean, &["add", "-A"]).expect("add");
        git(&clean, &["commit", "-q", "-m", "baseline"]).expect("commit");
        cover_control_mutation(&clean).expect("cover");
        assert!(
            !clean.join("changes").exists(),
            "пустая дельта в кейсе без правок — мусор"
        );
        // Прикрытие: правка приходит вместе с дельтой, как обычная доработка.
        cover_control_mutation(&repo).expect("cover");
        let after = crate::delta::guard(&repo, Some("HEAD"), &[]).expect("guard");
        assert!(after.passed, "прикрытие обязано снять нарушение: {after:?}");
        assert_eq!(after.protected_changed.len(), 1, "{after:?}");
        assert!(
            repo.join("changes")
                .join(CONTROL_DELTA)
                .join("DELTA.md")
                .is_file()
        );
        // Прикрытие не «прощает» правку вообще: без упоминания файла —
        // нарушение на месте (дельта покрывает только то, что названо).
        std::fs::write(
            repo.join("changes").join(CONTROL_DELTA).join("DELTA.md"),
            "# Дельта: redteam-control\n\n## MODIFIED\n\n",
        )
        .expect("rewrite");
        let bare = crate::delta::guard(&repo, Some("HEAD"), &[]).expect("guard");
        assert_eq!(bare.violations.len(), 1, "{bare:?}");
    }

    /// W2×W4: `save_summary` пишет в УКАЗАННЫЙ каталог, а не в `report.case`
    /// (прогон идёт в копии — измерение принадлежит исходному кейсу).
    #[test]
    fn save_summary_writes_into_the_given_case() {
        let tmp = tempfile::tempdir().expect("tmp");
        let case = tmp.path().join("case");
        std::fs::create_dir_all(&case).expect("mkdir");
        let measured = tmp.path().join("копия");
        let report = RedteamReport {
            case: measured,
            detections: Vec::new(),
            min_detection: 0.78,
            min_code_detection: None,
            control_ok: true,
            control_note: None,
            semantic_kept: Vec::new(),
            corpus: None,
        };
        let path = save_summary(&case, &report).expect("save");
        assert!(
            path.starts_with(&case),
            "запись в исходный кейс: {}",
            path.display()
        );
        let back = load_summary(&path).expect("load");
        assert_eq!(back.case, case.display().to_string());
        assert!(back.control_ok);
    }

    use super::*;

    #[test]
    fn expectation_labels_are_stable() {
        assert_eq!(Expectation::Caught.label(), "ловится");
        assert_eq!(Expectation::Semantic.label(), "семантика");
        assert_eq!(Expectation::Control.label(), "контроль");
    }

    /// T-02: D7 ослабляет ТОТ реестр, который читает гейт. При двух копиях
    /// (пакетной и корневой) резолвер выбирает пакетную — если мутант правит
    /// корневую, гейт ослабления не видит, и D7 числится непойманным на
    /// ровном месте: «дыра в защите», которой нет.
    #[test]
    fn d7_weakens_the_registry_the_gate_reads() {
        let tmp = tempfile::tempdir().expect("tmp");
        let case = tmp.path().join("case");
        std::fs::create_dir_all(case.join(".arch-handoff")).expect("mkdir");
        let strong = "rules:\n  - name: spine_present\n    type: file_exists\n    path: \"ARCHITECTURE-SPINE.md\"\n    severity: error\n";
        std::fs::write(case.join(".arch-handoff/CONSTRAINTS.yaml"), strong).expect("реестр пакета");
        std::fs::write(
            case.join("CONSTRAINTS.yaml"),
            "rules:\n  - name: readme_exists\n    type: file_exists\n    path: \"README.md\"\n    severity: error\n  - name: no_pan\n    type: must_not_contain\n    glob: \"**/*.py\"\n    pattern: 'PAN'\n    severity: error\n",
        )
        .expect("корневой реестр");
        std::fs::write(case.join("ARCHITECTURE-SPINE.md"), "# Spine\n").expect("spine");

        mutators::mutate_d7(&case).expect("мутант D7");
        let packet =
            std::fs::read_to_string(case.join(".arch-handoff/CONSTRAINTS.yaml")).expect("пакет");
        assert!(
            packet.contains("severity: warn"),
            "ослабление обязано быть в реестре, который читает гейт: {packet}"
        );
        let root = std::fs::read_to_string(case.join("CONSTRAINTS.yaml")).expect("корень");
        assert!(
            root.contains("severity: error"),
            "корневая копия мутантом не трогается — её гейт не читает: {root}"
        );
    }

    #[test]
    fn catalog_covers_the_red_team_set() {
        let ids: Vec<&str> = MUTATORS.iter().map(|m| m.id).collect();
        for expected in [
            "D1", "D2", "D3", "D4", "D5", "D6", "D7", "D8", "D9", "D10", "D11", "D11b", "D12",
            "D13", "R", "D14", "D15",
        ] {
            assert!(ids.contains(&expected), "в каталоге нет {expected}");
        }
        // Доля обнаружения считается по 14 дефектам раздела 7 (D1…D13 + D11b);
        // R, D14 и D15 — отдельные строки. D15 добавлен к таблице, а не к
        // знаменателю: он проверяет зубы применённого шаблона (обязан ли
        // краснеть `command_succeeds` на нарушающей реализации), а не
        // защищённость пакета от дефекта раздела 7. Включи он себя в долю —
        // критерий приёмки «≥ 11 из 14» перестал бы сравниваться с ТЗ.
        let scored = MUTATORS.iter().filter(|m| m.in_ratio).count();
        assert_eq!(scored, 14, "в наборе обязано быть 14 позиций раздела 7");
        // Обязаны ловиться 11: D1–D5, D7, D8, D9, D11b, D12, D13.
        let must_catch = MUTATORS
            .iter()
            .filter(|m| m.in_ratio && m.expected == Expectation::Caught)
            .count();
        assert_eq!(must_catch, 11, "критерий приёмки — 11 из 14");
        // Не ловятся и не должны: D6, D10, D11.
        let semantic = MUTATORS
            .iter()
            .filter(|m| m.in_ratio && m.expected == Expectation::Semantic)
            .count();
        assert_eq!(semantic, 3);
    }

    #[test]
    fn ratio_excludes_skipped_and_semantic() {
        let report = RedteamReport {
            case: PathBuf::from("case"),
            detections: vec![
                Detection {
                    id: "D1".into(),
                    title: "ловится".into(),
                    expected: Expectation::Caught,
                    layer: Layer::DocsModel,
                    caught_by: Some("nfr".into()),
                    skipped: None,
                    expected_by: "nfr".into(),
                    in_ratio: true,
                },
                Detection {
                    id: "D2".into(),
                    title: "пропущен".into(),
                    expected: Expectation::Caught,
                    layer: Layer::DocsModel,
                    caught_by: None,
                    skipped: Some("нет входа".into()),
                    expected_by: "nfr".into(),
                    in_ratio: true,
                },
                Detection {
                    id: "D6".into(),
                    title: "семантика".into(),
                    expected: Expectation::Semantic,
                    layer: Layer::DocsModel,
                    caught_by: None,
                    skipped: None,
                    expected_by: "—".into(),
                    in_ratio: true,
                },
            ],
            min_detection: 0.5,
            min_code_detection: None,
            control_ok: true,
            control_note: None,
            semantic_kept: Vec::new(),
            corpus: None,
        };
        // Пропущенный (нет входа) выпадает из знаменателя, семантический —
        // остаётся: он обязан НЕ ловиться, и доля это учитывает.
        assert_eq!(report.scored_total(), 2);
        assert_eq!(report.scored_caught(), 1);
        assert!((report.detection_ratio() - 0.5).abs() < f64::EPSILON);
        assert!(report.passed());
        // Семантический дефект, пойманный по ошибке, долю не поднимает.
        let mut over = report.clone();
        over.detections[2].caught_by = Some("model_validate".into());
        assert_eq!(over.scored_caught(), 1);
        // Красный, когда поймано меньше порога.
        let mut low = report.clone();
        low.min_detection = 0.6;
        assert!(!low.passed());
        // Контроль аттестации краснит прогон сам по себе.
        let mut ncontrol = report.clone();
        ncontrol.control_ok = false;
        assert!(!ncontrol.passed());
    }

    // --- S5 (ADR-051): смысловой слой отдельной строкой ---------------------

    /// Мутатор по идентификатору из каталога.
    fn mutator(id: &str) -> &'static Mutator {
        MUTATORS.iter().find(|m| m.id == id).expect("мутатор")
    }

    /// Клон смыслового мутанта сохраняется вместе с заданием хосту: в клоне
    /// лежит правленый субъект, рядом — `SEMANTIC-TODO.json` с рубрикой, видом
    /// досье и субъектом. Без задания хост не знал бы, чем судить.
    #[test]
    fn semantic_clone_keeps_subject_and_todo() {
        let tmp = tempfile::tempdir().expect("tmp");
        let root = tmp.path().join("clone");
        std::fs::create_dir_all(root.join("model")).expect("mkdir");
        std::fs::write(
            root.join("model/CMP-001-jurnal.md"),
            "---\nid: CMP-001\ntype: cmp\ntitle: Журнал\nstatus: designed\n---\n\nтело\n",
        )
        .expect("write");
        let m = mutator("D6");
        let subject = m.semantic.as_ref().expect("D6 — смысловой мутант");
        assert_eq!(subject.rubric, "model_link_semantics");

        let dest_root = tmp.path().join("kept");
        let dest = keep_semantic_clone(&root, &dest_root, m, subject).expect("клон");
        assert_eq!(dest, dest_root.join("D6"));
        assert!(
            dest.join("model/CMP-001-jurnal.md").is_file(),
            "правленый субъект в клоне"
        );
        let todo: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(dest.join("SEMANTIC-TODO.json")).expect("todo"),
        )
        .expect("json");
        assert_eq!(todo["mutant"], "D6");
        assert_eq!(todo["rubric"], "model_link_semantics");
        assert_eq!(todo["pack"], "entity_links");
        assert_eq!(
            todo["subject"], "CMP-001",
            "досье по сущностям адресуется идентификатором, а не путём"
        );
        assert!(
            todo["instructions"]
                .as_str()
                .expect("инструкция")
                .contains("rubric_prompt"),
            "задание говорит, чем судить: {todo}"
        );
    }

    /// Резолвер субъекта берёт первый файл в отсортированном порядке — прогон
    /// детерминирован независимо от порядка файловой системы.
    #[test]
    fn semantic_subject_resolution_is_sorted() {
        let tmp = tempfile::tempdir().expect("tmp");
        let root = tmp.path().join("clone");
        std::fs::create_dir_all(root.join("docs/adr")).expect("mkdir");
        for name in ["ADR-002-b.md", "ADR-001-a.md"] {
            std::fs::write(root.join("docs/adr").join(name), "x").expect("write");
        }
        let m = mutator("D10");
        let subject = m.semantic.as_ref().expect("D10 — смысловой мутант");
        assert_eq!(
            resolve_semantic_subject(&root, subject).as_deref(),
            Some("docs/adr/ADR-001-a.md")
        );
    }

    /// Досье по сущностям адресуется идентификатором, а не путём к файлу:
    /// иначе `rubric run --pack entity_links` не находит субъекта.
    #[test]
    fn semantic_subject_for_entities_is_an_id() {
        let tmp = tempfile::tempdir().expect("tmp");
        let root = tmp.path().join("clone");
        std::fs::create_dir_all(root.join("model")).expect("mkdir");
        std::fs::write(
            root.join("model/CMP-001-orchestrator-vyplat.md"),
            "---\nid: CMP-001\ntype: cmp\ntitle: Оркестратор\nstatus: designed\n---\n\nтело\n",
        )
        .expect("write");
        let m = mutator("D6");
        let subject = m.semantic.as_ref().expect("D6 — смысловой мутант");
        assert_eq!(
            resolve_semantic_subject(&root, subject).as_deref(),
            Some("CMP-001")
        );
    }

    /// Что писать в отчёт судьи клона (для тестов смысловой строки).
    struct ReportSpec<'a> {
        main_score: u8,
        flags: &'a [&'a str],
        pack_sha256: Option<&'a str>,
        author: Option<&'a str>,
    }

    /// Отчёт судьи в клоне: пишем минимальный валидный артефакт рубрики.
    fn write_report(clone: &Path, rubric: &str, subject: &str, spec: &ReportSpec<'_>) {
        let ReportSpec {
            main_score,
            flags,
            pack_sha256,
            author,
        } = *spec;
        let dir = clone.join(crate::rubric::RUBRIC_REPORTS_DIR);
        std::fs::create_dir_all(&dir).expect("mkdir reports");
        let artifact = serde_json::json!({
            "schema": crate::rubric::RUBRIC_REPORT_SCHEMA,
            "rubric": rubric,
            "judge_model": "judge-x",
            "author_model": author,
            "weighted_total": 2.0,
            "verdict": "CONCERNS",
            "pack_kind": "adr_vs_spine",
            "subject": subject,
            "pack_sha256": pack_sha256,
            "scores": [{
                "criterion_id": "no_contradiction",
                "weight": 3.0,
                "score": main_score,
                "rationale": "Цитата subject: \"a\". Цитата reference: \"b\".",
                "samples": [main_score],
                "stdev": 0.0,
                "flags": flags,
                "evidence_unconfirmed_ratio": 0.0,
                "checked": ["AD-1"],
            }],
            "judged_at": "2026-09-20T10:00:00+03:00",
        });
        std::fs::write(
            dir.join("report.json"),
            serde_json::to_string_pretty(&artifact).expect("json"),
        )
        .expect("write report");
    }

    /// Клон с заданием и скелетом досье (`docs/adr` + спайн), чтобы отчёт
    /// можно было пересчитать и сверить хэш.
    fn score_clone(root: &Path) -> PathBuf {
        let clone = root.join("D10");
        std::fs::create_dir_all(clone.join("docs/adr")).expect("mkdir");
        std::fs::write(
            clone.join(ARCHITECTURE_SPINE_FOR_TEST),
            "## AD-1: Журнал только дописывается\n\n- **Rule**: строки журнала не правятся.\n",
        )
        .expect("spine");
        std::fs::write(
            clone.join("docs/adr/ADR-001-x.md"),
            "# ADR-001\n\nрешение\n",
        )
        .expect("adr");
        std::fs::write(
            clone.join("SEMANTIC-TODO.json"),
            serde_json::to_string_pretty(&serde_json::json!({
                "mutant": "D10",
                "rubric": "adr_spine_consistency",
                "pack": "adr_vs_spine",
                "subject": "docs/adr/ADR-001-x.md",
            }))
            .expect("json"),
        )
        .expect("todo");
        clone
    }

    /// Имя спайна в клоне — как у сборщика досье.
    const ARCHITECTURE_SPINE_FOR_TEST: &str = "ARCHITECTURE-SPINE.md";

    /// Каталог рубрик с одной смысловой рубрикой (главный критерий — `low`).
    fn rubrics_dir(root: &Path) -> PathBuf {
        let dir = root.join("rubrics");
        std::fs::create_dir_all(&dir).expect("mkdir rubrics");
        std::fs::write(
            dir.join("adr_spine_consistency.yaml"),
            "name: adr_spine_consistency\ndescription: d\nscale_max: 5\norigin: anchor\n\
             pack: adr_vs_spine\ncriteria:\n  - id: no_contradiction\n    name: n\n    \
             description: d\n    weight: 3.0\n    blocking: true\n    evidence_on: low\n",
        )
        .expect("rubric");
        dir
    }

    /// Смысловая строка: пойман — главный критерий ≤ 2 с подтверждённым
    /// обвинением; выше — пропуск; с меткой исключения — обвинение не
    /// подтверждено; с чужим хэшем досье — отчёт устарел.
    #[test]
    fn semantic_score_reads_verdicts_from_reports() {
        let cases: [(&str, ReportSpec<'_>, SemanticVerdict); 4] = [
            (
                "пойман",
                ReportSpec {
                    main_score: 1,
                    flags: &[],
                    pack_sha256: None,
                    author: Some("author-y"),
                },
                SemanticVerdict::Caught,
            ),
            (
                "пропуск",
                ReportSpec {
                    main_score: 4,
                    flags: &[],
                    pack_sha256: None,
                    author: Some("author-y"),
                },
                SemanticVerdict::Missed,
            ),
            (
                "не подтверждено",
                ReportSpec {
                    main_score: 1,
                    flags: &["accusation_unconfirmed"],
                    pack_sha256: None,
                    author: Some("author-y"),
                },
                SemanticVerdict::Unconfirmed,
            ),
            (
                "устарел",
                ReportSpec {
                    main_score: 1,
                    flags: &[],
                    pack_sha256: Some(
                        "0000000000000000000000000000000000000000000000000000000000000000",
                    ),
                    author: Some("author-y"),
                },
                SemanticVerdict::Stale,
            ),
        ];
        for (name, spec, want) in cases {
            let tmp = tempfile::tempdir().expect("tmp");
            let clone = score_clone(tmp.path());
            write_report(
                &clone,
                "adr_spine_consistency",
                "docs/adr/ADR-001-x.md",
                &spec,
            );
            let scored = semantic_score(tmp.path(), &rubrics_dir(tmp.path())).expect("score");
            assert_eq!(scored.cases.len(), 1, "{name}");
            assert_eq!(scored.cases[0].verdict, want, "{name}: {scored:?}");
            // Человеческая строка называет и судью, и независимость.
            let text = scored.render();
            if want == SemanticVerdict::Caught {
                assert!(text.contains("смысловой слой: поймано 1 из 1"), "{text}");
                assert!(text.contains("судья: judge-x"), "{text}");
                assert!(text.contains("независим: да"), "{text}");
            }
        }
    }

    /// Клон без отчёта судьи — «нет отчёта», а не «не пойман»: разница
    /// принципиальна, иначе непрогнанный кейс считался бы провалом судьи.
    #[test]
    fn semantic_score_without_report_says_so() {
        let tmp = tempfile::tempdir().expect("tmp");
        let _ = score_clone(tmp.path());
        let scored = semantic_score(tmp.path(), &rubrics_dir(tmp.path())).expect("score");
        assert_eq!(scored.cases[0].verdict, SemanticVerdict::NoReport);
        assert_eq!(scored.caught(), 0);
        assert!(scored.render().contains("независим: нет"));
    }

    /// Пустой каталог — явная ошибка с подсказкой, а не «поймано 0 из 0».
    #[test]
    fn semantic_score_empty_dir_is_error() {
        let tmp = tempfile::tempdir().expect("tmp");
        let err = semantic_score(tmp.path(), &rubrics_dir(tmp.path())).expect_err("пусто");
        assert!(err.to_string().contains("keep-semantic"), "{err}");
    }

    /// Смысловая колонка в карте обнаружения называет рубрику: без неё
    /// «не пойман и не должен» читается как приговор без выхода.
    #[test]
    fn detection_map_names_semantic_rubric() {
        let report = RedteamReport {
            case: PathBuf::from("case"),
            detections: vec![Detection {
                id: "D10".into(),
                title: "решение противоречит инварианту".into(),
                expected: Expectation::Semantic,
                layer: Layer::DocsModel,
                caught_by: None,
                skipped: None,
                expected_by: "—".into(),
                in_ratio: true,
            }],
            min_detection: 0.78,
            min_code_detection: None,
            control_ok: true,
            control_note: None,
            semantic_kept: Vec::new(),
            corpus: None,
        };
        let md = render_markdown(&report);
        assert!(
            md.contains("| D10 |") && md.contains("adr_spine_consistency"),
            "колонка смысловой рубрики: {md}"
        );
        assert!(md.contains("не пойман и не должен"), "{md}");
    }

    /// Составляющая гейта для сборки отчёта в тестах B7 (конструкторы
    /// `GateComponent` — `pub(super)` в `gate::types`, поэтому соберём литералом).
    fn gate_component(
        name: &'static str,
        status: GateStatus,
        detail: &str,
    ) -> crate::gate::GateComponent {
        crate::gate::GateComponent {
            name,
            status,
            detail: detail.to_string(),
            findings: Vec::new(),
            not_verified: Vec::new(),
        }
    }

    /// Отчёт гейта из состава: `recompute()` выводит `not_checked`/`outcome`.
    fn report_with(
        components: Vec<crate::gate::GateComponent>,
        required: Vec<&str>,
    ) -> crate::gate::GateReport {
        let mut report = crate::gate::GateReport {
            repo: PathBuf::from("repo"),
            route: Route::Critical,
            route_auto: false,
            route_note: String::new(),
            route_triggers: Vec::new(),
            components,
            outcome: GateOutcome::Pass,
            required: required.into_iter().map(str::to_string).collect(),
            not_checked: Vec::new(),
            inputs: Vec::new(),
            attestation: String::new(),
            passed: false,
        };
        report.recompute();
        report
    }

    /// Эталон, красный ровно из-за отсутствующего прогонщика: `fitness` ушёл в
    /// SKIP с причиной «нет прогонщика pytest: … (`python3 -m pip install pytest`)».
    fn runnerless_report() -> crate::gate::GateReport {
        report_with(
            vec![gate_component(
                "fitness",
                GateStatus::Skip,
                "исполняемые правила не прогонялись (r_no_pytest) — \
                 нет прогонщика pytest: `python3 -m pip install pytest` \
                 — файл: CONSTRAINTS.yaml",
            )],
            vec!["fitness"],
        )
    }

    /// B7: неизмеримый эталон обязан назвать НЕДОСТАЮЩИЙ прогонщик и команду
    /// установки. Без этого причина («pytest не найден») видна только в
    /// `doctor`, а `redteam` краснеет глухим «обязательная fitness без входа».
    #[test]
    fn b7_not_green_reasons_names_missing_runner_and_install_hint() {
        let report = runnerless_report();
        let reason = not_green_reasons(&report);
        assert!(
            reason.contains("обязательная fitness без входа (SKIP)"),
            "{reason}"
        );
        assert!(
            reason.contains("нет прогонщика"),
            "причина пропуска обязана называть прогонщика: {reason}"
        );
        assert!(
            reason.contains("pip install pytest"),
            "нужна команда установки: {reason}"
        );
    }

    /// B7: `runner_skip_note` вырезает из detail только причину по прогонщику
    /// (до служебного « — файл»), а на пропуске иной природы отвечает `None`.
    #[test]
    fn b7_runner_skip_note_extracts_hint_only() {
        assert_eq!(
            runner_skip_note(&runnerless_report()).as_deref(),
            Some("нет прогонщика pytest: `python3 -m pip install pytest`")
        );
        let other = report_with(
            vec![gate_component(
                "fitness",
                GateStatus::Skip,
                "нет каталога model/ — файл: CONSTRAINTS.yaml",
            )],
            vec!["fitness"],
        );
        assert_eq!(runner_skip_note(&other), None, "пропуск не по прогонщику");
    }

    /// B7: `--hermetic` прощает ТОЛЬКО эталон, красный исключительно из-за
    /// отсутствующего прогонщика. FAIL-составляющая или пропуск иной природы
    /// (нет model/, нет ADR…) режимом не прощаются — иначе он маскировал бы
    /// реальные провалы.
    #[test]
    fn b7_hermetic_excuses_only_runner_incomplete_reference() {
        assert!(
            runner_only_incomplete(&runnerless_report()),
            "эталон только из-за прогонщика"
        );
        let failed = report_with(
            vec![gate_component("fitness", GateStatus::Fail, "нарушений: 3")],
            vec!["fitness"],
        );
        // FAIL перекрывает: исход Fail — не прощается.
        assert!(!runner_only_incomplete(&failed), "FAIL не прощается");
        let other_skip = report_with(
            vec![gate_component(
                "fitness",
                GateStatus::Skip,
                "нет каталога model/",
            )],
            vec!["fitness"],
        );
        assert!(
            !runner_only_incomplete(&other_skip),
            "пропуск не по прогонщику не прощается"
        );
        let green = report_with(
            vec![gate_component("fitness", GateStatus::Pass, "ок")],
            vec!["fitness"],
        );
        assert!(
            !runner_only_incomplete(&green),
            "зелёный — не предмет режима"
        );
    }

    /// B7: `hermetic_skip` даёт причину «неизмеримо без прогонщика» только
    /// когда режим включён, дефект не пойман и причина именно в прогонщике.
    #[test]
    fn b7_hermetic_skip_only_when_runner_missing_and_nothing_caught() {
        let report = runnerless_report();
        let reason = hermetic_skip(&report, true, true).expect("пропуск по прогонщику");
        assert!(reason.contains("неизмеримо без прогонщика"), "{reason}");
        assert!(reason.contains("pip install pytest"), "{reason}");
        assert_eq!(hermetic_skip(&report, false, true), None, "режим выключен");
        assert_eq!(hermetic_skip(&report, true, false), None, "дефект пойман");
        let other = report_with(
            vec![gate_component("fitness", GateStatus::Skip, "нет model/")],
            vec!["fitness"],
        );
        assert_eq!(
            hermetic_skip(&other, true, true),
            None,
            "пропуск не по прогонщику"
        );
    }
}
