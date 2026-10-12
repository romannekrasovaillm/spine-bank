//! Журнал сессии агента: append-only JSONL в `paths.sessions_dir` —
//! открытие файла, ISO-метки времени, сводки для `/sessions`. Вынесено из
//! `agent.rs` чистым перемещением (C-33 `prod_file_length_limit`); публичный
//! фасад остаётся `crate::agent` (реэкспорт в `agent.rs`).

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use crate::error::{HarnessError, Result};

/// Открывает append-only журнал `session-<yyyymmdd-hhmmss>-<pid>[-n].jsonl` в
/// `dir`. Суффикс pid защищает от коллизии двух процессов, а счётчик `-n` —
/// от повторного открытия в ту же секунду внутри одного процесса
/// (`/new` ротирует журнал): журналы сессий никогда не перемешиваются.
/// Новый файл начинается с «#»-шапки — ASCII-логотип `ArchSpine`
/// (banking edition) и версия; читатели JSONL «#»-строки пропускают.
pub(super) fn open_journal(dir: &Path) -> Result<(PathBuf, std::fs::File)> {
    std::fs::create_dir_all(dir).map_err(|e| HarnessError::io(dir, e))?;
    let base = format!(
        "session-{}-{}",
        chrono::Local::now().format("%Y%m%d-%H%M%S"),
        std::process::id()
    );
    for seq in 0..100u32 {
        let name = if seq == 0 {
            format!("{base}.jsonl")
        } else {
            format!("{base}-{seq}.jsonl")
        };
        let path = dir.join(&name);
        match std::fs::OpenOptions::new()
            .create_new(true)
            .append(true)
            .open(&path)
        {
            Ok(mut file) => {
                // Шапка с ASCII-логотипом ArchSpine: «#»-строки в начале
                // файла — логотип виден первой строкой лога, а читатели
                // JSONL такие строки пропускают. Best effort: журнал
                // валиден и без шапки, поэтому ошибка записи — только
                // предупреждение.
                let mut header = String::new();
                for line in crate::assets::BANNER.lines() {
                    let _ = writeln!(header, "# {line}");
                }
                let _ = writeln!(
                    header,
                    "# ArchSpine (banking edition) v{} · журнал сессии (append-only JSONL)",
                    env!("CARGO_PKG_VERSION")
                );
                if let Err(e) = std::io::Write::write_all(&mut file, header.as_bytes())
                    .and_then(|()| std::io::Write::flush(&mut file))
                {
                    tracing::warn!("шапка журнала не записана: {e}");
                }
                return Ok((path, file));
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(HarnessError::io(&path, e)),
        }
    }
    Err(HarnessError::io(
        dir,
        std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "слишком много журналов за одну секунду",
        ),
    ))
}

/// Текущее время в ISO 8601 с миллисекундами (для журнала).
pub(super) fn now_iso() -> String {
    chrono::Local::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// Краткий итог вывода инструмента: первая строка, не более 120 символов.
pub(super) fn summarize(content: &str) -> String {
    let first = content.lines().next().unwrap_or("").trim();
    let mut out: String = first.chars().take(120).collect();
    if out.len() < first.len() {
        out.push('…');
    }
    out
}

/// Сводка о журнале сессии (для `/sessions`).
#[derive(Debug, Clone)]
pub struct SessionLogInfo {
    /// Путь к JSONL-журналу.
    pub path: PathBuf,
    /// Время модификации (строка для показа).
    pub modified: String,
    /// Первая пользовательская реплика (превью).
    pub first_user_line: String,
    /// Число сообщений user/assistant в журнале.
    pub messages: usize,
}

/// Список журналов сессий каталога (новые первыми).
#[must_use]
pub fn list_session_logs(dir: &Path) -> Vec<SessionLogInfo> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in rd.flatten() {
        let path = entry.path();
        let is_log = path.file_name().is_some_and(|n| {
            let n = n.to_string_lossy();
            n.starts_with("session-") && n.ends_with(".jsonl")
        });
        if !is_log {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let mut first_user_line = String::new();
        let mut messages = 0usize;
        for line in text.lines() {
            let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
                continue;
            };
            let kind = v.get("kind").and_then(|k| k.as_str()).unwrap_or("");
            if matches!(kind, "user" | "assistant") {
                messages += 1;
                if kind == "user" && first_user_line.is_empty() {
                    let content = v.get("content").and_then(|c| c.as_str()).unwrap_or("");
                    first_user_line = summarize(content);
                }
            }
        }
        let modified = entry
            .metadata()
            .and_then(|m| m.modified())
            .map(|t| {
                let dt: chrono::DateTime<chrono::Local> = t.into();
                dt.format("%Y-%m-%d %H:%M:%S").to_string()
            })
            .unwrap_or_default();
        out.push(SessionLogInfo {
            path,
            modified,
            first_user_line,
            messages,
        });
    }
    out.sort_by(|a, b| b.modified.cmp(&a.modified));
    out
}
