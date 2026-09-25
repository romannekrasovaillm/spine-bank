//! Решение архитектора по решению `human` (E4.5).
//!
//! Когда механика говорит `human`, спорное уходит человеку (E4.1/E4.4). Ответ
//! человека — не «галочка в чате», а запись в репозитории: кто, когда, что
//! решил и почему. Запись привязывается к **хэшу файла отчёта**: правка отчёта
//! после решения обесценивает решение — оно относилось к другой ревизии.
//!
//! Хранится рядом с пакетом человека: `reports/human/<slug>.decision.json`.
//! Коммитится подписанным коммитом (`git commit -S`, `Signed-off-by`) — это и
//! есть «подпись» в смысле ревью: запись в истории, а не поле в JSON. Гейт
//! читает запись и снимает эскалацию, если решение — `accept` и хэш отчёта
//! совпал; `reject` оставляет эскалацию (субъект надо оценить заново).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{HarnessError, Result};

use super::artifact::{RUBRIC_REPORTS_DIR, RubricArtifact};

/// Схема файла решения архитектора.
pub const HUMAN_DECISION_SCHEMA: &str = "arch-be/human-decision/v1";

/// Что решил архитектор по суждению судьи.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HumanVerdict {
    /// Суждение принято: эскалация `human` снимается, находка остаётся видимой.
    Accept,
    /// Суждение отклонено: эскалация остаётся, субъект надо оценить заново.
    Reject,
}

impl HumanVerdict {
    /// Строковое имя для JSON и вывода.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Accept => "accept",
            Self::Reject => "reject",
        }
    }
}

/// Запись решения архитектора: кто, когда, что и по какому отчёту.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HumanDecision {
    /// Схема файла.
    pub schema: String,
    /// Рубрика, по которой судили.
    pub rubric: String,
    /// Субъект: путь документа или идентификатор/путь досье (как в отчёте).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    /// Путь к отчёту относительно репозитория.
    pub report: String,
    /// SHA-256 файла отчёта на момент решения: правка отчёта решение
    /// обесценивает (оно относилось к другой ревизии).
    pub report_sha256: String,
    /// Что решил человек.
    pub decision: HumanVerdict,
    /// Кто решил: имя и, при желании, адрес (`Иван Петров <ivan@bank>`).
    pub decided_by: String,
    /// Когда (RFC 3339).
    pub decided_at: String,
    /// Обоснование решения.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reason: String,
}

impl HumanDecision {
    /// Собирает запись по отчёту: идентичность берётся из артефакта, хэш — из
    /// его файла.
    #[must_use]
    pub fn new(
        artifact: &RubricArtifact,
        report: &str,
        report_sha256: &str,
        decision: HumanVerdict,
        decided_by: &str,
        reason: &str,
    ) -> Self {
        Self {
            schema: HUMAN_DECISION_SCHEMA.to_string(),
            rubric: artifact.rubric.clone(),
            subject: artifact.subject.clone().or_else(|| artifact.target.clone()),
            report: report.to_string(),
            report_sha256: report_sha256.to_string(),
            decision,
            decided_by: decided_by.to_string(),
            decided_at: chrono::Local::now().to_rfc3339(),
            reason: reason.to_string(),
        }
    }
}

/// Путь файла решения по slug'у отчёта.
#[must_use]
pub fn decision_path(repo: &Path, slug: &str) -> PathBuf {
    repo.join(super::decision::HUMAN_QUEUE_DIR)
        .join(format!("{slug}.decision.json"))
}

/// Записывает решение архитектора рядом с пакетом человека.
///
/// # Errors
/// Каталог не создаётся или файл не пишется.
pub fn write_decision(repo: &Path, slug: &str, record: &HumanDecision) -> Result<PathBuf> {
    let path = decision_path(repo, slug);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let text = serde_json::to_string_pretty(record)
        .map_err(|e| HarnessError::Rubric(format!("сериализация решения: {e}")))?;
    std::fs::write(&path, text)?;
    Ok(path)
}

/// Читает запись решения; `None` — её нет или она не разбирается (битый файл
/// не должен ронять гейт: решение — свидетельство, а не вход контроля).
#[must_use]
pub fn read_decision(repo: &Path, slug: &str) -> Option<HumanDecision> {
    let text = std::fs::read_to_string(decision_path(repo, slug)).ok()?;
    serde_json::from_str(&text).ok()
}

/// Действующее решение по отчёту: рубрика совпадает, а хэш файла отчёта сходится
/// с записанным. Правка отчёта после решения — решения нет.
#[must_use]
pub fn decision_for(repo: &Path, artifact: &RubricArtifact) -> Option<HumanDecision> {
    let slug = crate::judge::artifact_slug_of(artifact);
    let record = read_decision(repo, &slug)?;
    if record.rubric != artifact.rubric {
        return None;
    }
    let dir = repo.join(RUBRIC_REPORTS_DIR);
    // Отчёт обычно назван по slug'у (`write_artifact`). Если проект назвал файл
    // иначе — ищем среди отчётов тот, чья личность совпадает: решение
    // привязывается к содержимому, а не к имени файла.
    let canonical = dir.join(format!("{slug}.json"));
    if canonical.is_file() {
        return (crate::hash::sha256_file(&canonical)? == record.report_sha256).then_some(record);
    }
    for entry in std::fs::read_dir(&dir).ok()?.flatten() {
        let path = entry.path();
        if !path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("json"))
        {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(other) = serde_json::from_str::<RubricArtifact>(&text) else {
            continue;
        };
        if other.rubric == artifact.rubric
            && other.subject == artifact.subject
            && other.target == artifact.target
            && other.pack_sha256 == artifact.pack_sha256
            && other.target_sha256 == artifact.target_sha256
            && crate::hash::sha256_hex(text.as_bytes()) == record.report_sha256
        {
            return Some(record);
        }
    }
    None
}

/// Относительный путь отчёта внутри репозитория — для записи в решение.
#[must_use]
pub fn report_rel_path(repo: &Path, report: &Path) -> String {
    report.strip_prefix(repo).map_or_else(
        |_| report.display().to_string(),
        |p| p.display().to_string().replace('\\', "/"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rubric::testkit::*;

    /// Отчёт в репозитории: настоящий путь записи, чтобы решение привязывалось
    /// к тому же файлу, что читает гейт.
    fn report_in(repo: &Path) -> (PathBuf, RubricArtifact) {
        std::fs::create_dir_all(repo).expect("repo");
        let doc = repo.join("doc.md");
        std::fs::write(&doc, "контекст описан подробно").expect("doc");
        let runs = vec![
            crate::rubric::parse_judge_response(
                "{\"scores\":[{\"criterion_id\":\"context\",\"score\":4,\
                 \"rationale\":\"Цитата: \\\"контекст описан подробно\\\"\"}],\"verdict\":\"ok\"}",
            )
            .expect("ответ судьи"),
        ];
        let report = crate::rubric::build_report(
            &sample_rubric(),
            "judge-x",
            &runs,
            &crate::rubric::EvidenceScope::Target("контекст описан подробно"),
            &one_sample(),
        )
        .expect("отчёт");
        let path = crate::rubric::write_artifact(repo, &report, Some(&doc), None).expect("отчёт");
        let artifact: RubricArtifact =
            serde_json::from_str(&std::fs::read_to_string(&path).expect("файл")).expect("JSON");
        (path, artifact)
    }

    fn record_for(
        repo: &Path,
        path: &Path,
        artifact: &RubricArtifact,
        verdict: HumanVerdict,
    ) -> HumanDecision {
        let text = std::fs::read_to_string(path).expect("отчёт");
        HumanDecision::new(
            artifact,
            &report_rel_path(repo, path),
            &crate::hash::sha256_hex(text.as_bytes()),
            verdict,
            "Иван Петров <ivan@bank>",
            "риск принят осознанно",
        )
    }

    /// E4.5: решение записывается, читается и действует только для той ревизии
    /// отчёта, по которой принято: правка отчёта решение обесценивает.
    #[test]
    fn decision_is_bound_to_the_report_revision() {
        let tmp = tempfile::tempdir().expect("tmp");
        let repo = tmp.path();
        let (path, artifact) = report_in(repo);
        let slug = crate::judge::artifact_slug_of(&artifact);
        let record = record_for(repo, &path, &artifact, HumanVerdict::Accept);
        let written = write_decision(repo, &slug, &record).expect("решение");
        assert!(written.is_file(), "{}", written.display());
        assert!(
            written.to_string_lossy().contains("reports/human"),
            "{}",
            written.display()
        );
        let read = read_decision(repo, &slug).expect("решение читается");
        assert_eq!(read.decision, HumanVerdict::Accept);
        assert_eq!(read.decided_by, "Иван Петров <ivan@bank>");
        assert_eq!(read.report, "reports/rubric/doc.json");
        assert!(
            decision_for(repo, &artifact).is_some(),
            "решение относится к этой ревизии"
        );
        // Правка отчёта после решения — решения нет: оно про другую ревизию.
        std::fs::write(&path, "{\"schema\":\"arch-be/rubric-report/v1\"}").expect("правка");
        assert!(
            decision_for(repo, &artifact).is_none(),
            "решение не переносится на правленый отчёт"
        );
    }

    /// E4.5: `reject` читается как отклонение — гейт по нему эскалацию не снимает.
    #[test]
    fn reject_is_recorded_as_rejection() {
        let tmp = tempfile::tempdir().expect("tmp");
        let repo = tmp.path();
        let (path, artifact) = report_in(repo);
        let slug = crate::judge::artifact_slug_of(&artifact);
        let record = record_for(repo, &path, &artifact, HumanVerdict::Reject);
        write_decision(repo, &slug, &record).expect("решение");
        let read = decision_for(repo, &artifact).expect("решение действует");
        assert_eq!(read.decision, HumanVerdict::Reject);
    }
}
