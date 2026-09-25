//! Типы рубрик архитектурного контроля (B1): критерии с весами и якорями,
//! метки достоверности оценок, снимки конфигурации судьи (ADR-004, ADR-051).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::error::Result;

/// Максимум символов оцениваемого текста: жёсткий лимит промпта судьи.
/// Превышение — явная ошибка ([`check_target_len`]), тихого усечения
/// больше нет (ADR-004). Тот же лимит держит досье смысловой рубрики
/// ([`crate::rubric_pack`], ADR-051) — судья видит ровно один такой объём.
pub const MAX_TARGET_CHARS: usize = 24_000;

/// Когда цитата-свидетельство обязательна (ADR-051, S2).
///
/// В рубрике качества обвинение — это низкий балл, и цитата нужна именно там;
/// в обычной рубрике (поведение 0.3.4) — наоборот, за похвалу.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceOn {
    /// Балл ≥ 2 — цитата за похвалу (поведение 0.3.4, дефолт).
    #[default]
    High,
    /// Балл ≤ 2 — цитата за обвинение.
    Low,
    /// Оба конца шкалы: и похвала, и обвинение.
    Both,
}

impl EvidenceOn {
    /// Строковое имя — как в YAML рубрики.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::High => "high",
            Self::Low => "low",
            Self::Both => "both",
        }
    }

    /// Требуется ли цитата при высоком балле (≥ 2).
    #[must_use]
    pub fn requires_high(self) -> bool {
        matches!(self, Self::High | Self::Both)
    }

    /// Требуется ли цитата при низком балле (≤ 2).
    #[must_use]
    pub fn requires_low(self) -> bool {
        matches!(self, Self::Low | Self::Both)
    }
}

/// Что судья обязан перечислить в ответе при высоком балле (ADR-051, S3).
///
/// Отсутствие не цитируется: «противоречий нет» доказать цитатой нельзя. Вместо
/// цитаты судья называет, что он **проверил**, а механика сверяет список с
/// составом досье — пропуск становится видимым.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Coverage {
    /// Перечислить идентификаторы всех ссылочных источников досье
    /// (`checked: [AD-1, AD-2, …]`).
    ReferenceIds,
}

impl Coverage {
    /// Строковое имя — как в YAML рубрики: строка с одним вариантом, поэтому
    /// опечатка в значении отвергается разбором, а не молча теряет проверку.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ReferenceIds => "reference_ids",
        }
    }
}

/// Критерий рубрики с весом и якорями уровней.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Criterion {
    /// Идентификатор критерия (`snake_case`).
    pub id: String,
    /// Название.
    pub name: String,
    /// Что оценивается.
    pub description: String,
    /// Вес (сумма по рубрике — произвольная, нормируется при подсчёте).
    pub weight: f64,
    /// Якоря уровней: «1» → «критерий отсутствует», «5» → «образцово».
    #[serde(default)]
    pub anchors: std::collections::BTreeMap<u8, String>,
    /// Когда цитата обязательна (ADR-051); дефолт — поведение 0.3.4.
    #[serde(default)]
    pub evidence_on: EvidenceOn,
    /// Роли источников досье, из которых нужны цитаты (ADR-051): цитата из
    /// ADR не засчитывается как цитата из спайна. Пусто — любая часть текста.
    #[serde(default)]
    pub evidence_roles: Vec<String>,
    /// Что судья обязан перечислить при высоком балле (ADR-051, S3);
    /// `None` — покрытие не проверяется (поведение 0.3.4).
    #[serde(default)]
    pub coverage: Option<Coverage>,
    /// Главный критерий рубрики (ADR-052): по нему составляющая гейта
    /// `semantic_quality` строит блокирующую находку `semantic_contradiction`.
    /// Движок рубрик поле не читает — это решение гейта, и рубрика без
    /// главного критерия гейтом отвергается.
    #[serde(default)]
    pub blocking: bool,
}

impl Criterion {
    /// Требуемые роли доказательства: пустой список ролей — одно требование
    /// без роли (проверяется по всему тексту).
    ///
    /// # Errors
    /// Имя роли не из [`crate::rubric_pack::InputRole`].
    pub fn evidence_role_list(&self) -> Result<Vec<Option<crate::rubric_pack::InputRole>>> {
        if self.evidence_roles.is_empty() {
            return Ok(vec![None]);
        }
        self.evidence_roles
            .iter()
            .map(|r| crate::rubric_pack::InputRole::parse(r).map(Some))
            .collect()
    }
}

/// Рубрика оценки (якорная — из YAML, динамическая — сгенерированная).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rubric {
    /// Имя рубрики.
    pub name: String,
    /// Описание назначения.
    pub description: String,
    /// Максимум шкалы (обычно 5).
    pub scale_max: u8,
    /// Критерии с весами.
    pub criteria: Vec<Criterion>,
    /// Пометка происхождения: anchor|dynamic.
    pub origin: String,
    /// Вид досье, которым собирается вход этой рубрики (ADR-051, волна B);
    /// `None` — рубрика оценивает один документ (поведение 0.3.4).
    ///
    /// Соответствие «рубрика → досье» живёт в рубрике, а не в коде гейта:
    /// иначе третья смысловая рубрика потребовала бы правки механики.
    #[serde(default)]
    pub pack: Option<crate::rubric_pack::PackKind>,
}

/// Сводная строка списка рубрик.
#[derive(Debug, Clone)]
pub struct RubricSummary {
    /// Путь к YAML.
    pub path: PathBuf,
    /// Имя.
    pub name: String,
    /// Описание.
    pub description: String,
    /// Число критериев.
    pub criteria_count: usize,
}

/// Метка достоверности оценки критерия (ADR-004).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CriterionFlag {
    /// Разброс баллов по сэмплам судьи выше порога [`JudgeConfig::unstable_stdev`].
    Unstable,
    /// Цитата-свидетельство из rationale не подтверждена оцениваемым текстом;
    /// критерий исключён из взвешенного итога.
    EvidenceNotFound,
    /// Часть сэмплов судьи пришла с неподтверждённой цитатой (Д10): в медиану
    /// критерия они не вошли, но подтверждённых сэмплов хватило — балл
    /// засчитан с оговоркой. **Из итога не исключает** — в отличие от
    /// `accusation_unconfirmed` ниже.
    EvidencePartial,
    /// Обвинение (низкий балл смысловой рубрики) не подтверждено цитатами по
    /// требуемым ролям (ADR-051): критерий исключён из взвешенного итога, но
    /// блокирующей находкой **не становится** — выдуманное обвинение наказывает
    /// судью потерей критерия, а не репозиторий.
    AccusationUnconfirmed,
    /// Высокий балл выставлен без полного перечня проверенных ссылочных
    /// источников (ADR-051, S3): критерий исключён из взвешенного итога —
    /// «5 не глядя» не должно читаться как проверка.
    CoverageIncomplete,
    /// Свидетельство критерия опирается на строку входа с паттерном
    /// prompt-инъекции (E2, ADR-038): такая цитата не засчитывается
    /// доказательством — она честно есть в тексте, но её подсунул не автор
    /// решения, а тот, кто хотел управлять судьёй. Критерий, оставшийся без
    /// подтверждённых свидетельств, исключается из итога как обычно.
    InjectionQuote,
    /// Балл сэмпла вне шкалы рубрики (E3.1): `9` при `scale_max: 5` — не
    /// «пятёрка после обрезки», а невалидный сэмпл. Он не голосует за балл,
    /// а его доля (`invalid_samples_ratio`) видна в отчёте и в гейте.
    InvalidSamples,
}

impl CriterionFlag {
    /// Строковое имя для отчётов и журналов.
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Unstable => "unstable",
            Self::EvidenceNotFound => "evidence_not_found",
            Self::EvidencePartial => "evidence_partial",
            Self::AccusationUnconfirmed => "accusation_unconfirmed",
            Self::CoverageIncomplete => "coverage_incomplete",
            Self::InjectionQuote => "injection_quote",
            Self::InvalidSamples => "invalid_samples",
        }
    }

    /// Исключает ли метка критерий из взвешенного итога рубрики.
    #[must_use]
    pub fn excludes_from_total(self) -> bool {
        matches!(
            self,
            Self::EvidenceNotFound | Self::AccusationUnconfirmed | Self::CoverageIncomplete
        )
    }
}

/// Оценка одного критерия.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CriterionScore {
    /// Идентификатор критерия.
    pub criterion_id: String,
    /// Вес критерия в рубрике (копия для отчётной таблицы [`RubricReport::to_markdown`]).
    #[serde(default)]
    pub weight: f64,
    /// Итоговый балл `1..=scale_max` (округлённая медиана сэмплов).
    pub score: u8,
    /// Обоснование судьи (из сэмпла с медианным баллом, иначе первое непустое).
    pub rationale: String,
    /// Баллы всех сэмплов судьи (длина = числу сэмплов оценки).
    #[serde(default)]
    pub samples: Vec<u8>,
    /// Population-σ сэмплов (0 при одном сэмпле).
    #[serde(default)]
    pub stdev: f64,
    /// Метки достоверности: `unstable`, `evidence_not_found`,
    /// `evidence_partial`, `accusation_unconfirmed`, `coverage_incomplete`,
    /// `injection_quote`, `invalid_samples`.
    #[serde(default)]
    pub flags: Vec<CriterionFlag>,
    /// Доля сэмплов судьи с неподтверждённой цитатой (Д10; ADR-051, S2).
    /// Такие сэмплы в медиану не входят; у критериев без направления
    /// доказательства — ноль. Поле аддитивное: отчёты, снятые раньше,
    /// читаются (отсутствие = ноль).
    #[serde(default)]
    pub evidence_unconfirmed_ratio: f64,
    /// Сколько сэмплов критерия пришло с баллом вне шкалы (E3.1). Такой сэмпл
    /// не голосует за балл, а в `samples` виден приведённым к шкале — счётчик
    /// рядом объясняет, почему. Поле аддитивное (отсутствие = ноль).
    #[serde(default)]
    pub invalid_samples: usize,
    /// Что судья назвал проверенным (ADR-051, S3) — объединение перечней
    /// `checked` по сэмплам; пусто у критериев без `coverage`.
    #[serde(default)]
    pub checked: Vec<String>,
}

impl CriterionScore {
    /// Признак наличия метки достоверности.
    #[must_use]
    pub fn has_flag(&self, flag: CriterionFlag) -> bool {
        self.flags.contains(&flag)
    }
}

/// Правила сборки отчёта: то, что было в секции `[judge]` в момент оценки.
///
/// Зачем в отчёте: `arch-be rubric run` и MCP `rubric_run` читают `[judge]`, но
/// отчёт об этом молчал — по двум отчётам нельзя было понять, почему у одного
/// три сэмпла, а у другого пять. Поле аддитивное: у старых отчётов его нет.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JudgeConfigSnapshot {
    /// Сэмплов судьи на критерий.
    pub samples: usize,
    /// Порог population-σ сэмплов для метки `unstable`.
    pub unstable_stdev: f64,
    /// Порог сходства цитаты-свидетельства.
    pub evidence_min_similarity: f64,
}

/// Балл критерия на момент сборки отчёта — то, по чему сверяется
/// воспроизводимость отчёта из сырых ответов судьи (J2, ADR-048).
///
/// У отчётов до появления поля его нет: сверка тогда идёт по взвешенному
/// итогу, метке `unstable` и числу `evidence_not_found` — старый отчёт
/// не становится подозрительным из-за отсутствия поля.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CriterionSnapshot {
    /// Идентификатор критерия.
    pub criterion_id: String,
    /// Итоговый балл `1..=scale_max`.
    pub score: u8,
    /// Метки достоверности критерия.
    #[serde(default)]
    pub flags: Vec<CriterionFlag>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rubric::testkit::*;
    #[test]
    fn rubric_yaml_roundtrip() {
        let rubric = sample_rubric();
        let yaml = serde_yaml_ng::to_string(&rubric).expect("serialize");
        let back: Rubric = serde_yaml_ng::from_str(&yaml).expect("deserialize");
        assert_eq!(back.name, rubric.name);
        assert_eq!(back.scale_max, 5);
        assert_eq!(back.criteria.len(), 2);
        assert_eq!(back.criteria[0].weight, 1.0);
        assert!(back.criteria[0].anchors.contains_key(&5));
        assert_eq!(back.origin, "anchor");
    }
}
