//! Каталог рубрик (B1): загрузка YAML-рубрики и список рубрик каталога
//! (`*.yaml`/`*.yml`; битые файлы пропускаются).

use std::path::Path;

use crate::error::{HarnessError, Result};

use super::types::{Rubric, RubricSummary};

/// Загружает рубрику из YAML.
///
/// # Errors
/// Файл не читается / не валиден.
pub fn load(path: &Path) -> Result<Rubric> {
    let text = std::fs::read_to_string(path).map_err(|e| HarnessError::io(path, e))?;
    let rubric: Rubric = serde_yaml_ng::from_str(&text)?;
    Ok(rubric)
}

/// Список рубрик каталога (`*.yaml`/`*.yml`); битые файлы пропускаются.
///
/// # Errors
/// Каталог не читается.
pub fn list(dir: &Path) -> Result<Vec<RubricSummary>> {
    let entries = std::fs::read_dir(dir).map_err(|e| HarnessError::io(dir, e))?;
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !is_yaml_file(&path) {
            continue;
        }
        // Битый файл — не ошибка каталога: пропускаем.
        if let Ok(rubric) = load(&path) {
            out.push(RubricSummary {
                path,
                criteria_count: rubric.criteria.len(),
                name: rubric.name,
                description: rubric.description,
            });
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

/// Файл имеет YAML-расширение (`yaml`/`yml`, регистр неважен).
fn is_yaml_file(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("yaml") || e.eq_ignore_ascii_case("yml"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rubric::testkit::*;
    #[test]
    fn load_reads_yaml_and_list_skips_broken() {
        let dir = tempfile::tempdir().expect("tempdir");
        let good = dir.path().join("good.yaml");
        std::fs::write(
            &good,
            serde_yaml_ng::to_string(&sample_rubric()).expect("yaml"),
        )
        .expect("write");
        std::fs::write(dir.path().join("broken.yaml"), "name: [unclosed").expect("write");
        std::fs::write(dir.path().join("notes.txt"), "не yaml").expect("write");

        let loaded = load(&good).expect("load");
        assert_eq!(loaded.name, "adr-quality");

        let items = list(dir.path()).expect("list");
        assert_eq!(
            items.len(),
            1,
            "битый и не-yaml файлы должны быть пропущены"
        );
        assert_eq!(items[0].name, "adr-quality");
        assert_eq!(items[0].criteria_count, 2);
    }

    #[test]
    fn list_errors_on_missing_dir() {
        let missing = Path::new("/nonexistent/rubrics-dir");
        assert!(list(missing).is_err());
    }
}
