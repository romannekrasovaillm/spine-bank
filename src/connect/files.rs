//! Файловые примитивы `connect` (B1): чтение JSON-объекта, фиксация записи
//! в отчёте, разовый бэкап, путь к домашнему конфигу хоста.

use std::path::{Path, PathBuf};

use serde_json::Value;

use super::types::{ConnectOptions, ConnectReport};
use crate::error::{HarnessError, Result};

/// Читает JSON-объект из файла: (объект, исходный текст, если файл был).
/// Отсутствующий файл — пустой объект; битый JSON или не-объект верхнего
/// уровня — ошибка (чужой файл не затираем).
pub(super) fn read_json_object(
    path: &Path,
) -> Result<(serde_json::Map<String, Value>, Option<String>)> {
    match std::fs::read_to_string(path) {
        Ok(text) => {
            let value: Value = serde_json::from_str(&text).map_err(|e| {
                HarnessError::Config(format!(
                    "{}: существующий файл не разбирается как JSON ({e}) — \
                     не затираю; разберите вручную",
                    path.display()
                ))
            })?;
            let obj = value.as_object().cloned().ok_or_else(|| {
                HarnessError::Config(format!(
                    "{}: ожидался JSON-объект верхнего уровня — не затираю",
                    path.display()
                ))
            })?;
            Ok((obj, Some(text)))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok((serde_json::Map::new(), None)),
        Err(e) => Err(HarnessError::io(path, e)),
    }
}

/// Фиксирует новое содержимое файла: пишет (при dry-run — только планирует)
/// и классифицирует в отчёте. `old = None` — файл ранее не существовал.
pub(super) fn commit_file(
    path: &Path,
    old: Option<&str>,
    new: &str,
    dry_run: bool,
    report: &mut ConnectReport,
) -> Result<()> {
    if old == Some(new) {
        report.unchanged.push(path.to_path_buf());
        return Ok(());
    }
    if !dry_run {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| HarnessError::io(parent, e))?;
        }
        std::fs::write(path, new).map_err(|e| HarnessError::io(path, e))?;
    }
    if old.is_some() {
        report.merged.push(path.to_path_buf());
    } else {
        report.created.push(path.to_path_buf());
    }
    Ok(())
}

/// Разовый бэкап файла перед перезаписью (только `--apply-global`):
/// `<имя>.bak-spine-connect` создаётся один раз, повторные прогоны его
/// не трогают.
pub(super) fn backup_once(
    path: &Path,
    old: &str,
    dry_run: bool,
    report: &mut ConnectReport,
) -> Result<()> {
    let name = path.file_name().map_or_else(
        || "config.bak-spine-connect".to_string(),
        |n| format!("{}.bak-spine-connect", n.to_string_lossy()),
    );
    let backup = path.with_file_name(name);
    if backup.exists() || dry_run {
        return Ok(());
    }
    std::fs::write(&backup, old).map_err(|e| HarnessError::io(&backup, e))?;
    report
        .notes
        .push(format!("бэкап прежнего конфига: {}", backup.display()));
    Ok(())
}

/// Домашний конфиг хоста для `--apply-global` или ошибка.
pub(super) fn global_config_path(opts: &ConnectOptions, rel: &str) -> Result<PathBuf> {
    opts.home.as_ref().map(|h| h.join(rel)).ok_or_else(|| {
        HarnessError::Config(
            "--apply-global: не удалось определить домашний каталог пользователя".into(),
        )
    })
}
