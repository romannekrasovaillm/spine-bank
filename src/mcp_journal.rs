//! Журнал вызовов MCP-сервера (`arch-be mcp serve`): append-only
//! `<cwd сервера>/.arch-handoff/mcp-calls.jsonl` — источник outcome-данных
//! для `arch-be digest` и протокола `docs/outcome-metrics.md` (пункт 9
//! бэклога волны): метрики пилота заполняются из журнала, а не вручную.
//!
//! КОНТРАКТ:
//! - одна строка = один вызов `tools/call`, JSON: `ts` (RFC 3339, локальная
//!   зона), `tool`, `verdict` (`pass`/`fail` — по `passed` структурированного
//!   verdict'а или JSON-вердикта моста; `ok` — успех без `passed`;
//!   `error` — доменный сбой выполнения (`isError: true`);
//!   `invalid` — отказ уровня параметров: неизвестный инструмент, битые
//!   аргументы), `duration_ms`, `rules` (имена правил из error-находок
//!   verdict'а, до [`MAX_JOURNAL_RULES`] — для топа нарушаемых правил
//!   дайджеста). Содержимое аргументов НЕ журналируется;
//! - журнал проектный: каталог `.arch-handoff/` рядом с `CONSTRAINTS.yaml`
//!   (cwd процесса сервера), а не пользовательский — аудит живёт вместе
//!   с репозиторием клиента;
//! - запись строго fail-soft: журнал не должен ломать вызовы — место вызова
//!   глушит ошибку (`let _ = …` с комментарием);
//! - ротация по размеру: при достижении [`MAX_JOURNAL_BYTES`] текущий файл
//!   переименовывается в `mcp-calls.1.jsonl` (прежний `.1` затирается) и
//!   журнал начинается заново — одно поколение истории назад.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{HarnessError, Result};

/// Потолок размера журнала (5 МиБ): при превышении — ротация в `.1`.
/// Строка вызова ~120 байт — потолок покрывает ~40k вызовов на поколение.
const MAX_JOURNAL_BYTES: u64 = 5 * 1024 * 1024;

/// Потолок имён правил в одной записи журнала: verdict с сотнями находок
/// не должен раздувать строку; для топа нарушаемых правил хватает десяти.
const MAX_JOURNAL_RULES: usize = 10;

/// Имя файла журнала внутри `.arch-handoff/`.
const JOURNAL_FILE: &str = "mcp-calls.jsonl";

/// Запись журнала вызовов (одна строка JSONL).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JournalEntry {
    /// Момент вызова (RFC 3339, локальная зона).
    pub ts: String,
    /// Имя инструмента (`tools/call`).
    pub tool: String,
    /// Вердикт: `pass` | `fail` | `ok` | `error` | `invalid`.
    pub verdict: String,
    /// Длительность диспетча, миллисекунды.
    pub duration_ms: u64,
    /// Имена правил из error-находок verdict'а (только у `fail`; источник —
    /// сам verdict, не аргументы вызова). Пустой список не сериализуется.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rules: Vec<String>,
}

impl JournalEntry {
    /// Запись с текущим штампом времени.
    #[must_use]
    pub fn new(
        tool: &str,
        verdict: &str,
        duration: std::time::Duration,
        rules: Vec<String>,
    ) -> Self {
        Self {
            ts: chrono::Local::now().to_rfc3339(),
            tool: tool.to_string(),
            verdict: verdict.to_string(),
            duration_ms: duration.as_millis() as u64,
            rules,
        }
    }
}

/// Путь журнала для сервера с рабочим каталогом `cwd`.
#[must_use]
pub fn journal_path(cwd: &Path) -> PathBuf {
    cwd.join(".arch-handoff").join(JOURNAL_FILE)
}

/// Дописывает запись в журнал (создаёт `.arch-handoff/` при отсутствии).
/// Сама функция честно возвращает ошибку; fail-soft — на месте вызова.
///
/// # Errors
/// Каталог не создаётся, файл не открывается/не пишется, ротация не
/// удалась (переименование поверх существующего `.1`).
pub fn append(cwd: &Path, entry: &JournalEntry) -> Result<()> {
    append_capped(cwd, entry, MAX_JOURNAL_BYTES)
}

/// [`append`] с параметризованным потолком (тесты ротации крутятся на
/// маленьком лимите, без заполнения многомегабайтного файла).
fn append_capped(cwd: &Path, entry: &JournalEntry, max_bytes: u64) -> Result<()> {
    let dir = cwd.join(".arch-handoff");
    std::fs::create_dir_all(&dir).map_err(|e| HarnessError::io(&dir, e))?;
    let path = dir.join(JOURNAL_FILE);
    if std::fs::metadata(&path).map_or(0, |m| m.len()) >= max_bytes {
        let rotated = dir.join("mcp-calls.1.jsonl");
        // Прежнее поколение `.1` затирается — журнал держит одно поколение
        // истории назад (простая ротация по размеру, без архива).
        std::fs::rename(&path, &rotated).map_err(|e| HarnessError::io(&rotated, e))?;
    }
    let mut line = serde_json::to_string(entry)?;
    line.push('\n');
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|e| HarnessError::io(&path, e))?;
    file.write_all(line.as_bytes())
        .map_err(|e| HarnessError::io(&path, e))
}

/// Читает журнал целиком; битые строки пропускаются (журнал append-only,
/// хвост мог быть обрезан ротацией/сбоем — это не ошибка читателя).
/// Отсутствующий файл — пустой список (вызовов ещё не было).
#[must_use]
pub fn read_entries(path: &Path) -> Vec<JournalEntry> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    text.lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

/// Извлекает из структурированного verdict'а (или JSON-текста моста) имена
/// правил error-находок: источник данных дайджеста «топ нарушаемых правил».
/// Дедупликация, потолок [`MAX_JOURNAL_RULES`].
#[must_use]
pub fn failed_rule_names(verdict: &serde_json::Value) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let Some(issues) = verdict.get("issues").and_then(|v| v.as_array()) else {
        return out;
    };
    for issue in issues {
        let is_error = issue.get("severity").and_then(|s| s.as_str()) == Some("error");
        let rule = issue.get("rule").and_then(|r| r.as_str());
        if let (true, Some(rule)) = (is_error, rule) {
            if !out.iter().any(|r| r == rule) {
                out.push(rule.to_string());
            }
        }
        if out.len() >= MAX_JOURNAL_RULES {
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Запись читается обратно: `ts`/`tool`/`verdict`/`duration_ms`/`rules` на месте.
    #[test]
    fn append_then_read_roundtrip() {
        let tmp = tempfile::tempdir().expect("tmp");
        let entry = JournalEntry::new(
            "fitness_check",
            "fail",
            std::time::Duration::from_millis(42),
            vec!["no-pan".to_string()],
        );
        append(tmp.path(), &entry).expect("append");
        let entries = read_entries(&journal_path(tmp.path()));
        assert_eq!(entries.len(), 1);
        let e = &entries[0];
        assert_eq!(e.tool, "fitness_check");
        assert_eq!(e.verdict, "fail");
        assert_eq!(e.duration_ms, 42);
        assert_eq!(e.rules, vec!["no-pan"]);
        assert!(!e.ts.is_empty());
        // Вторая запись — append, а не перезапись.
        append(
            tmp.path(),
            &JournalEntry::new("spine_lint", "pass", std::time::Duration::ZERO, Vec::new()),
        )
        .expect("append 2");
        assert_eq!(read_entries(&journal_path(tmp.path())).len(), 2);
    }

    /// Ротация: при превышении потолка журнал уходит в `.1`, новый файл
    /// начинается заново и содержит только свежую запись.
    #[test]
    fn rotation_moves_full_journal_to_generation_one() {
        let tmp = tempfile::tempdir().expect("tmp");
        let big = JournalEntry::new("tool", "ok", std::time::Duration::ZERO, Vec::new());
        // Потолок в один байт: вторая запись обязана вызвать ротацию.
        append_capped(tmp.path(), &big, 1).expect("первая запись");
        append_capped(tmp.path(), &big, 1).expect("вторая запись ротирует");
        let fresh = read_entries(&journal_path(tmp.path()));
        assert_eq!(fresh.len(), 1, "после ротации журнал начат заново");
        let rotated = tmp.path().join(".arch-handoff/mcp-calls.1.jsonl");
        assert!(rotated.is_file(), "старое поколение — в .1");
        let old = read_entries(&rotated);
        assert_eq!(old.len(), 1, "прежняя запись сохранена в .1");
    }

    /// Fail-soft — забота вызывающего: append честно возвращает Err, когда
    /// `.arch-handoff` нельзя создать (на его месте — обычный файл;
    /// детерминированная имитация RO-ФС без игр с правами, которые под
    /// root не работают).
    #[test]
    fn append_reports_error_when_dir_not_creatable() {
        let tmp = tempfile::tempdir().expect("tmp");
        std::fs::write(tmp.path().join(".arch-handoff"), "не каталог").expect("файл-заглушка");
        let entry = JournalEntry::new("tool", "ok", std::time::Duration::ZERO, Vec::new());
        assert!(append(tmp.path(), &entry).is_err());
    }

    /// Чтение отсутствующего журнала — пусто; битые строки пропускаются.
    #[test]
    fn read_entries_tolerates_missing_and_broken_lines() {
        let tmp = tempfile::tempdir().expect("tmp");
        assert!(read_entries(&journal_path(tmp.path())).is_empty());
        let path = journal_path(tmp.path());
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        std::fs::write(
            &path,
            "{\"ts\":\"t\",\"tool\":\"a\",\"verdict\":\"ok\",\"duration_ms\":1}\nне-json\n",
        )
        .expect("запись");
        let entries = read_entries(&path);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].tool, "a");
    }

    /// Имена правил — только из error-находок, с дедупликацией и потолком.
    #[test]
    fn failed_rule_names_picks_error_rules_only() {
        let verdict = serde_json::json!({
            "passed": false,
            "issues": [
                {"severity": "error", "rule": "no-pan"},
                {"severity": "warn", "rule": "slow-rule"},
                {"severity": "error", "rule": "no-pan"},
                {"severity": "error", "rule": "msrv_pinned"},
                {"severity": "error"}
            ]
        });
        assert_eq!(failed_rule_names(&verdict), vec!["no-pan", "msrv_pinned"]);
        assert!(failed_rule_names(&serde_json::json!({"passed": true})).is_empty());
    }
}
