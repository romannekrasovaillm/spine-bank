//! Память сбоев инструментов: правило «ошибся дважды → урок в AGENTS.md»
//! (failure memory из AI-native SDLC playbook Anthropic).
//!
//! На каждый `ToolOutput::err` агентный цикл вычисляет **подпись сбоя** —
//! `(имя инструмента, нормализованный класс ошибки)`: из текста ошибки
//! вырезаются пути и имена файлов, поэтому «файл X не найден» и «файл Y
//! не найден» — одна подпись. Счётчики подписей персистируются в
//! `paths.state_dir/failure_memory.json` с датами первого/последнего случая.
//!
//! Когда подпись повторяется (count достигает 2, строго один раз на
//! подпись — флаг `lesson_fired` хранится в состоянии), формируется урок:
//! что случилось, когда, в каком инструменте. Действие зависит от режима
//! `[agent] failure_memory`:
//! - `off` — учёт отключён;
//! - `propose` (дефолт) — заметка в транскрипт + предложение урока в
//!   `paths.state_dir/failure_lessons.md` (просмотр — `/lessons`);
//! - `write` — как `propose`, плюс append урока в AGENTS.md рабочего
//!   каталога (вне зоны `ARCH:GENERATED`; редактирование только append,
//!   существующее содержимое никогда не перезаписывается).

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{HarnessError, Result};
use crate::lessons as ledger;

/// Имя файла состояния счётчиков в `paths.state_dir`.
pub const STATE_FILE: &str = "failure_memory.json";
/// Имя файла предложенных уроков в `paths.state_dir`.
pub const LESSONS_FILE: &str = "failure_lessons.md";

/// Максимум символов нормализованного класса ошибки в подписи.
const MAX_CLASS_CHARS: usize = 120;
/// Максимум символов примера вывода, сохраняемого в записи.
const MAX_SAMPLE_CHARS: usize = 200;

/// Заголовок автономной секции уроков в AGENTS.md проекта (режим `write`).
const AGENTS_SECTION: &str = "## Уроки failure-memory (авто)";

/// Запись счётчика по одной подписи сбоя.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FailureRecord {
    /// Имя инструмента.
    pub tool: String,
    /// Нормализованный класс ошибки (без путей/имён файлов).
    pub class: String,
    /// Сколько раз подпись встречалась.
    pub count: u32,
    /// ISO-метка первого случая.
    pub first_seen: String,
    /// ISO-метка последнего случая.
    pub last_seen: String,
    /// Первая строка последнего вывода (усечённая, для урока).
    pub sample: String,
    /// Урок по этой подписи уже сформирован (строго один раз).
    pub lesson_fired: bool,
}

/// Сформированный урок по повторной подписи (второй случай сбоя).
#[derive(Debug, Clone)]
pub struct Lesson {
    /// Подпись сбоя (`tool:class`).
    pub signature: String,
    /// Имя инструмента.
    pub tool: String,
    /// Нормализованный класс ошибки.
    pub class: String,
    /// Пример вывода (первая строка последнего случая).
    pub sample: String,
    /// ISO-метка первого случая.
    pub first_seen: String,
    /// ISO-метка последнего случая (момент срабатывания).
    pub last_seen: String,
    /// Число случаев на момент срабатывания (всегда 2).
    pub count: u32,
}

impl Lesson {
    /// Блок урока для `failure_lessons.md` (предложение человеку).
    #[must_use]
    pub fn to_markdown(&self) -> String {
        format!(
            "## {} · `{}` · {}\n\n\
             - Подпись: `{}`\n\
             - Случаев: {} (первый: {}, последний: {})\n\
             - Пример вывода: `{}`\n\
             - Рекомендация: инструмент `{}` повторно падает с одним классом ошибки — \
             разберите причину и зафиксируйте правило обхода в AGENTS.md проекта \
             (режим `failure_memory = \"write\"` делает это сам) или в MEMORY.md.\n",
            self.last_seen,
            self.tool,
            self.class,
            self.signature,
            self.count,
            self.first_seen,
            self.last_seen,
            self.sample,
            self.tool,
        )
    }

    /// Однострочный пункт для AGENTS.md проекта (режим `write`).
    #[must_use]
    pub fn to_agents_bullet(&self) -> String {
        format!(
            "- **{}** · `{}` · класс «{}» · случаев: {} · пример: `{}`",
            self.last_seen, self.tool, self.class, self.count, self.sample
        )
    }
}

/// Память сбоев: счётчики подписей + пути файлов состояния.
pub struct FailureMemory {
    /// Путь к файлу состояния (`failure_memory.json`).
    state_path: PathBuf,
    /// Путь к файлу предложенных уроков (`failure_lessons.md`).
    lessons_path: PathBuf,
    /// Счётчики по подписям.
    records: BTreeMap<String, FailureRecord>,
}

impl FailureMemory {
    /// Загружает состояние из `state_dir` (файл `failure_memory.json`).
    /// Отсутствие файла — пустое состояние; битый JSON — тоже (warn в
    /// tracing): память сбоев вспомогательна и не должна ронять сессию.
    #[must_use]
    pub fn load(state_dir: &Path) -> Self {
        let state_path = state_dir.join(STATE_FILE);
        let records = match std::fs::read_to_string(&state_path) {
            Ok(text) => serde_json::from_str(&text).unwrap_or_else(|e| {
                tracing::warn!(
                    "{}: битый JSON состояния ({e}) — начинаем с нуля",
                    state_path.display()
                );
                BTreeMap::new()
            }),
            Err(_) => BTreeMap::new(),
        };
        Self {
            state_path,
            lessons_path: state_dir.join(LESSONS_FILE),
            records,
        }
    }

    /// Путь к файлу предложенных уроков.
    #[must_use]
    pub fn lessons_path(&self) -> &Path {
        &self.lessons_path
    }

    /// Счётчики подписей (для `/lessons`).
    #[must_use]
    pub fn records(&self) -> &BTreeMap<String, FailureRecord> {
        &self.records
    }

    /// Учитывает сбой инструмента: подпись → счётчик → сохранение.
    /// Возвращает [`Lesson`] ровно один раз на подпись — при втором случае
    /// (count достиг 2 и урок ещё не формировался). Ошибки записи состояния
    /// — warn в tracing, ход агента не прерывают.
    pub fn note_failure(&mut self, tool: &str, output: &str) -> Option<Lesson> {
        let signature = signature(tool, output);
        let class = error_class(output);
        let sample: String = output
            .lines()
            .next()
            .unwrap_or("")
            .trim()
            .chars()
            .take(MAX_SAMPLE_CHARS)
            .collect();
        let now = now_iso();
        let rec = self
            .records
            .entry(signature.clone())
            .or_insert_with(|| FailureRecord {
                tool: tool.to_string(),
                class: class.clone(),
                count: 0,
                first_seen: now.clone(),
                last_seen: now.clone(),
                sample: String::new(),
                lesson_fired: false,
            });
        rec.count += 1;
        rec.last_seen.clone_from(&now);
        rec.sample.clone_from(&sample);
        let fire = rec.count >= 2 && !rec.lesson_fired;
        if fire {
            rec.lesson_fired = true;
        }
        // Значения для урока снимаем до save() (конфликт заимствований).
        let lesson = fire.then(|| Lesson {
            signature,
            tool: tool.to_string(),
            class,
            sample,
            first_seen: rec.first_seen.clone(),
            last_seen: now,
            count: rec.count,
        });
        self.save();
        lesson
    }

    /// Сохраняет состояние (best-effort: сбой — warn в tracing).
    fn save(&self) {
        if let Some(parent) = self.state_path.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                tracing::warn!("failure_memory: {}: {e}", parent.display());
                return;
            }
        }
        match serde_json::to_string_pretty(&self.records) {
            Ok(text) => {
                if let Err(e) = std::fs::write(&self.state_path, text) {
                    tracing::warn!("failure_memory: {}: {e}", self.state_path.display());
                }
            }
            Err(e) => tracing::warn!("failure_memory: сериализация: {e}"),
        }
    }
}

/// Подпись сбоя: `имя_инструмента:нормализованный_класс`.
#[must_use]
pub fn signature(tool: &str, error: &str) -> String {
    format!("{tool}:{}", error_class(error))
}

/// Источник сбоя, порождающего урок (ADR-085: источник — часть подписи).
///
/// Память сбоев поймала только ошибки *инструментов* (B1): FAIL приёмочного
/// гейта и нарушение fitness-правила проходили мимо. Здесь эти источники
/// получают ту же форму подписи, что и реестр уроков, — один сбой одного рода
/// даёт один `LSN-…` независимо от того, кто его заметил.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FailureSource {
    /// Сбой инструмента (подпись `tool:class`).
    Tool,
    /// FAIL приёмочного гейта (код вида `G5`).
    Gate,
    /// Нарушение fitness-правила реестра ограничений (id правила).
    Fitness,
}

impl FailureSource {
    /// Каноническая строка источника (как в реестре уроков).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Tool => "tool",
            Self::Gate => "gate",
            Self::Fitness => "fitness",
        }
    }

    /// Источник урока реестра (ADR-085) для этого класса сбоя.
    #[must_use]
    pub fn lesson_source(self) -> ledger::LessonSource {
        match self {
            Self::Tool => ledger::LessonSource::Tool,
            Self::Gate => ledger::LessonSource::Gate,
            Self::Fitness => ledger::LessonSource::Fitness,
        }
    }
}

/// Подпись урока реестра для сбоя источника (ADR-085/ADR-087).
///
/// `detail` — код гейта (`G5`), id правила или класс сбоя инструмента (в форме
/// [`signature`]: `read_file:io: <путь> not found`). Идентификатор `LSN-…`
/// стабилен: один и тот же сбой всегда даёт один и тот же урок.
#[must_use]
pub fn lesson_signature(source: FailureSource, detail: &str) -> ledger::Signature {
    ledger::Signature::new(source.lesson_source(), detail)
}

/// Разобранный урок файла `failure_lessons.md` (для миграции в реестр).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigratedLesson {
    /// Подпись сбоя (`tool:class`) — она же `run_id` доказательства.
    pub signature: String,
    /// Имя инструмента.
    pub tool: String,
    /// Нормализованный класс ошибки.
    pub class: String,
    /// Пример вывода (первая строка последнего случая).
    pub sample: String,
    /// ISO-метка последнего случая.
    pub last_seen: String,
    /// Число случаев на момент формирования урока.
    pub count: u32,
}

impl MigratedLesson {
    /// Формулировка правила для реестра (без LLM: факт сбоя плюс действие).
    #[must_use]
    pub fn statement(&self) -> String {
        format!(
            "Инструмент «{}» повторно падает с одним классом ошибки «{}» (случаев: {}) — \
             обходите причину, а не повторяйте вызов.",
            self.tool, self.class, self.count
        )
    }
}

/// Разбирает `failure_lessons.md` (формат [`Lesson::to_markdown`]) в записи
/// миграции. Битые/неполные блоки пропускаются — файл историчен и мог быть
/// дописан вручную.
#[must_use]
pub fn parse_lessons_md(text: &str) -> Vec<MigratedLesson> {
    let mut out = Vec::new();
    // Блок начинается заголовком `## `; файл может начинаться с него же —
    // тогда ведущего перевода строки нет, и первый блок иначе потерялся бы.
    let normalized = if text.starts_with("## ") {
        format!("\n{text}")
    } else {
        text.to_string()
    };
    for block in normalized.split("\n## ").skip(1) {
        let mut lines = block.lines();
        let Some(header) = lines.next() else {
            continue;
        };
        // Заголовок: `{ts} · `{tool}` · {class}`.
        let parts: Vec<&str> = header.split(" · ").collect();
        if parts.len() < 3 {
            continue;
        }
        let last_seen = parts[0].trim().to_string();
        let tool = parts[1].trim().trim_matches('`').to_string();
        let class = parts[2].trim().to_string();
        let mut signature = String::new();
        let mut sample = String::new();
        let mut count = 0u32;
        for line in lines {
            let line = line.trim();
            if let Some(rest) = line.strip_prefix("- Подпись: ") {
                signature = rest.trim().trim_matches('`').to_string();
            } else if let Some(rest) = line.strip_prefix("- Случаев: ") {
                count = rest
                    .split_whitespace()
                    .next()
                    .and_then(|n| n.parse::<u32>().ok())
                    .unwrap_or(0);
            } else if let Some(rest) = line.strip_prefix("- Пример вывода: ") {
                sample = rest.trim().trim_matches('`').to_string();
            }
        }
        if signature.is_empty() || tool.is_empty() {
            continue;
        }
        out.push(MigratedLesson {
            signature,
            tool,
            class,
            sample,
            last_seen,
            count,
        });
    }
    out
}

/// Мигрирует предложенные уроки памяти сбоев (`failure_lessons.md`) в реестр
/// уроков (ADR-087; закрывает B2 — файл, который никто не читал).
///
/// Каждый разобранный урок записывается доказательством `fail` с источником
/// `tool`; `run_id` — подпись сбоя, `ts` — последний случай. Повторный прогон
/// доказательств не плодит (дедупликация реестра по `run_id`/`ts`/`outcome`),
/// поэтому миграцию можно звать идемпотентно. Отсутствие файла — не ошибка:
/// мигрировать нечего.
///
/// # Errors
/// Ошибка чтения файла уроков (кроме «нет файла») или записи в реестр.
pub fn migrate_to_ledger(
    lessons_file: &Path,
    ledger: &ledger::Ledger,
    th: &ledger::Thresholds,
    today: &str,
) -> Result<usize> {
    let text = match std::fs::read_to_string(lessons_file) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(HarnessError::io(lessons_file, e)),
    };
    let parsed = parse_lessons_md(&text);
    for lesson in &parsed {
        let input = ledger::RecordInput::new(
            lesson_signature(FailureSource::Tool, &lesson.signature),
            ledger::Evidence {
                run_id: lesson.signature.clone(),
                harness: None,
                verdict_excerpt: lesson.sample.clone(),
                file: None,
                line: None,
                ts: lesson.last_seen.clone(),
                outcome: ledger::Outcome::Fail,
                task: Some(lesson.tool.clone()),
            },
        )
        .with_statement(lesson.statement())
        .with_recorder("failure-memory");
        ledger.record(&input, th, today)?;
    }
    Ok(parsed.len())
}

/// Нормализованный класс ошибки: первая строка вывода, где токены-пути
/// (`/a/b/c`, `C:\x`) схлопнуты в `<путь>`, имена файлов (`report.docx`) —
/// в `<файл>`, подряд идущие дубли плейсхолдеров сжаты в один. Поэтому
/// «файл X не найден» и «файл Y не найден» дают один класс.
#[must_use]
pub fn error_class(error: &str) -> String {
    let first = error.lines().next().unwrap_or("").trim();
    let mut tokens: Vec<String> = Vec::new();
    for raw in first.split_whitespace() {
        let norm = normalize_token(raw);
        if norm.is_empty() {
            continue;
        }
        // Сжатие подряд идущих одинаковых плейсхолдеров: «<путь> <путь>» → «<путь>».
        if tokens.last() == Some(&norm) {
            continue;
        }
        tokens.push(norm);
    }
    let class: String = tokens.join(" ").chars().take(MAX_CLASS_CHARS).collect();
    if class.is_empty() {
        "(пустая ошибка)".to_string()
    } else {
        class
    }
}

/// Нормализация одного токена: обрамляющие кавычки/скобки срезаются,
/// пути и имена файлов заменяются плейсхолдерами.
fn normalize_token(raw: &str) -> String {
    let t = raw.trim_matches(|c: char| {
        matches!(
            c,
            '"' | '\'' | '`' | '«' | '»' | '(' | ')' | '[' | ']' | '{' | '}' | ',' | ';'
        )
    });
    if t.is_empty() {
        return String::new();
    }
    if t.contains('/') || t.contains('\\') {
        return "<путь>".to_string();
    }
    if looks_like_filename(t) {
        return "<файл>".to_string();
    }
    t.to_string()
}

/// Эвристика имени файла: есть точка, суффикс — 1..=8 ascii-алфавитно-цифровых
/// символов с хотя бы одной буквой (числа-версии вида `1.5` не трогаем).
fn looks_like_filename(token: &str) -> bool {
    let Some((stem, ext)) = token.rsplit_once('.') else {
        return false;
    };
    !stem.is_empty()
        && !ext.is_empty()
        && ext.len() <= 8
        && ext.chars().all(|c| c.is_ascii_alphanumeric())
        && ext.chars().any(|c| c.is_ascii_alphabetic())
}

/// Дописывает блок урока в `failure_lessons.md` (файл и родительский
/// каталог создаются; существующее содержимое не трогается — только append).
///
/// # Errors
/// Ошибка создания каталога или записи файла.
pub fn append_lesson(path: &Path, lesson_md: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|e| HarnessError::io(parent, e))?;
        }
    }
    let mut block = String::new();
    // Разделитель перед блоком, если файл уже не пуст (читаем только ради
    // длины — содержимое никогда не перезаписывается).
    let not_empty = std::fs::metadata(path).is_ok_and(|m| m.len() > 0);
    if not_empty {
        block.push('\n');
    }
    block.push_str(lesson_md);
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| HarnessError::io(path, e))?;
    std::io::Write::write_all(&mut file, block.as_bytes()).map_err(|e| HarnessError::io(path, e))
}

/// Дописывает урок в AGENTS.md рабочего каталога (режим `write`).
///
/// Строго append: файл читается только для проверки наличия секции
/// [`AGENTS_SECTION`], существующее содержимое никогда не перезаписывается.
/// Дописка идёт в конец файла — снаружи зоны `ARCH:GENERATED`…`ARCH:END`
/// (зона непрерывна между маркерами, поэтому хвост файла всегда в рукописной
/// зоне и переживает перегенерацию). Файл отсутствует — создаётся с секцией.
///
/// # Errors
/// Ошибка чтения/записи AGENTS.md.
pub fn append_to_agents_md(repo_root: &Path, lesson: &Lesson) -> Result<PathBuf> {
    let path = repo_root.join("AGENTS.md");
    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    let mut chunk = String::new();
    if !existing.trim().is_empty() {
        chunk.push('\n');
    }
    if !existing.contains(AGENTS_SECTION) {
        let _ = writeln!(chunk, "{AGENTS_SECTION}\n");
        let _ = writeln!(
            chunk,
            "<!-- Дописывается харнессом при повторных сбоях инструментов \
             ([agent] failure_memory = \"write\"). Выученные правила переносите \
             выше, в рукописные разделы. -->\n"
        );
    }
    let _ = writeln!(chunk, "{}", lesson.to_agents_bullet());
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|e| HarnessError::io(&path, e))?;
    std::io::Write::write_all(&mut file, chunk.as_bytes())
        .map_err(|e| HarnessError::io(&path, e))?;
    Ok(path)
}

/// Текущее время в ISO 8601 с миллисекундами (метки первого/последнего случая).
fn now_iso() -> String {
    chrono::Local::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_class_collapses_paths_and_filenames() {
        let a =
            error_class("read_file: io: /alpha/beta/x.txt: No such file or directory (os error 2)");
        let b = error_class("read_file: io: /gamma/y.txt: No such file or directory (os error 2)");
        assert_eq!(a, b, "разные пути — одна подпись: {a} vs {b}");
        assert!(a.contains("<путь>"), "путь схлопнут: {a}");
        assert!(!a.contains("alpha"), "путь не утекает в класс: {a}");

        // «файл X не найден» и «файл Y не найден» — один класс.
        let x = error_class("fs: файл «report.docx» не найден");
        let y = error_class("fs: файл «plan.md» не найден");
        assert_eq!(x, y, "{x} vs {y}");
        assert_eq!(x, "fs: файл <файл> не найден");

        // Код ошибки сохраняется: exit code 1 и exit code 2 — разные классы.
        assert_ne!(
            error_class("bash: команда завершилась с кодом 1"),
            error_class("bash: команда завершилась с кодом 2"),
            "коды — часть класса"
        );
        // Подряд идущие плейсхолдеры сжимаются.
        assert_eq!(
            error_class("cp /a/x /b/y: нет такого"),
            "cp <путь> нет такого"
        );
    }

    #[test]
    fn error_class_handles_empty_and_multiline() {
        assert_eq!(error_class(""), "(пустая ошибка)");
        assert_eq!(error_class("\n\n"), "(пустая ошибка)");
        // Класс — только по первой строке.
        assert_eq!(error_class("первая /p/q\nвторая строка"), "первая <путь>");
    }

    #[test]
    fn note_failure_fires_lesson_exactly_once_and_persists() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path().join("state");
        let mut mem = FailureMemory::load(&dir);
        // Первый случай — только счётчик.
        assert!(
            mem.note_failure("read_file", "io: /a/x.txt: not found")
                .is_none()
        );
        // Второй (другой путь, та же подпись) — урок.
        let lesson = mem
            .note_failure("read_file", "io: /b/y.txt: not found")
            .expect("второй случай — урок");
        assert_eq!(lesson.count, 2);
        assert!(lesson.signature.starts_with("read_file:"));
        assert!(lesson.class.contains("<путь>"));
        // Третий — урок НЕ повторяется.
        assert!(
            mem.note_failure("read_file", "io: /c/z.txt: not found")
                .is_none()
        );

        // Перезагрузка с диска: счётчик и флаг урока персистентны.
        let mut reloaded = FailureMemory::load(&dir);
        let rec = &reloaded.records()[&lesson.signature];
        assert_eq!(rec.count, 3, "счётчик пережил перезапуск");
        assert!(rec.lesson_fired);
        assert!(!rec.first_seen.is_empty() && !rec.last_seen.is_empty());
        assert!(
            reloaded
                .note_failure("read_file", "io: /d/w.txt: not found")
                .is_none(),
            "урок строго один раз на подпись"
        );
        // Другая подпись (другой инструмент, тот же класс по путям) — свой цикл.
        assert!(
            reloaded
                .note_failure("bash", "команда /usr/bin/git не найдена")
                .is_none()
        );
        assert!(
            reloaded
                .note_failure("bash", "команда /usr/local/bin/hg не найдена")
                .is_some()
        );
    }

    /// Источники `gate` и `fitness` дают те же подписи реестра, что источник
    /// `tool` (ADR-087): FAIL гейта и нарушение правила больше не «невидимы».
    #[test]
    fn gate_and_fitness_sources_produce_ledger_signatures() {
        assert_eq!(FailureSource::Tool.as_str(), "tool");
        assert_eq!(FailureSource::Gate.as_str(), "gate");
        assert_eq!(FailureSource::Fitness.as_str(), "fitness");
        assert_eq!(
            FailureSource::Gate.lesson_source(),
            ledger::LessonSource::Gate
        );
        assert_eq!(
            FailureSource::Fitness.lesson_source(),
            ledger::LessonSource::Fitness
        );
        let gate = lesson_signature(FailureSource::Gate, "G5");
        assert_eq!(gate.source, ledger::LessonSource::Gate);
        assert_eq!(gate.code, "G5");
        assert_eq!(gate.id(), lesson_signature(FailureSource::Gate, "G5").id());
        let fitness = lesson_signature(FailureSource::Fitness, "RULE-17");
        assert_eq!(fitness.source, ledger::LessonSource::Fitness);
        assert_ne!(gate.id(), fitness.id(), "разные источники — разные уроки");
        // Подпись инструмента — та же форма, что у памяти сбоев.
        let tool = lesson_signature(FailureSource::Tool, &signature("read_file", "io: нет"));
        assert_eq!(tool.source, ledger::LessonSource::Tool);
        assert!(tool.code.starts_with("read_file:"), "{}", tool.code);
    }

    /// Формат `failure_lessons.md` разбирается обратно в записи (нужно для
    /// миграции: файл писался человеку, а читает его теперь реестр).
    #[test]
    fn lessons_md_roundtrip_parses_to_migration_records() {
        let lesson = Lesson {
            signature: "read_file:io: <путь> not found".into(),
            tool: "read_file".into(),
            class: "io: <путь> not found".into(),
            sample: "io: /a/x.txt: not found".into(),
            first_seen: "2026-10-13T10:00:00.000Z".into(),
            last_seen: "2026-10-14T10:00:00.000Z".into(),
            count: 2,
        };
        let parsed = parse_lessons_md(&lesson.to_markdown());
        assert_eq!(parsed.len(), 1, "{parsed:?}");
        let rec = &parsed[0];
        assert_eq!(rec.signature, "read_file:io: <путь> not found");
        assert_eq!(rec.tool, "read_file");
        assert_eq!(rec.class, "io: <путь> not found");
        assert_eq!(rec.count, 2);
        assert_eq!(rec.last_seen, "2026-10-14T10:00:00.000Z");
        assert!(rec.statement().contains("read_file"));

        // Мусор и пустой файл — пусто, без паники.
        assert_eq!(parse_lessons_md("").len(), 0, "пустой файл");
        assert_eq!(
            parse_lessons_md("## не заголовок урока\n\nтекст\n").len(),
            0,
            "неполный блок пропущен"
        );
    }

    /// Миграция: уроки из `failure_lessons.md` попадают в реестр, повторный
    /// прогон идемпотентен, отсутствие файла — не ошибка (B1, B2).
    #[test]
    fn migration_moves_failure_lessons_into_registry() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let lesson = Lesson {
            signature: "bash:код 2".into(),
            tool: "bash".into(),
            class: "код 2".into(),
            sample: "exit 2".into(),
            first_seen: "2026-10-13T10:00:00.000Z".into(),
            last_seen: "2026-10-14T10:00:00.000Z".into(),
            count: 2,
        };
        let file = tmp.path().join("failure_lessons.md");
        append_lesson(&file, &lesson.to_markdown()).expect("append");

        let ledger_handle = ledger::Ledger::at(tmp.path().join("ledger.jsonl"));
        let th = ledger::Thresholds::default();
        assert_eq!(
            migrate_to_ledger(&file, &ledger_handle, &th, "2026-10-14").expect("миграция"),
            1
        );
        let snapshot = ledger_handle.snapshot(&th).expect("snapshot");
        assert_eq!(snapshot.lessons.len(), 1, "урок в реестре");
        let migrated = &snapshot.lessons[0];
        assert_eq!(migrated.signature.source, ledger::LessonSource::Tool);
        assert_eq!(migrated.signature.code, "bash:код 2");
        assert_eq!(migrated.provenance.recorder, "failure-memory");
        assert!(migrated.statement.contains("баш") || migrated.statement.contains("bash"));

        // Идемпотентность: доказательства не дублируются.
        let before = snapshot.lessons[0].evidence.len();
        migrate_to_ledger(&file, &ledger_handle, &th, "2026-10-14").expect("повтор");
        let again = ledger_handle.snapshot(&th).expect("snapshot 2");
        assert_eq!(
            again.lessons[0].evidence.len(),
            before,
            "доказательства не растут"
        );

        // Нет файла — мигрировать нечего, и это не ошибка.
        assert_eq!(
            migrate_to_ledger(
                &tmp.path().join("нет-такого.md"),
                &ledger_handle,
                &th,
                "2026-10-14"
            )
            .expect("нет файла"),
            0
        );
    }

    #[test]
    fn append_lesson_appends_without_overwriting() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("nested/failure_lessons.md");
        append_lesson(&path, "## урок 1\n").expect("append 1");
        append_lesson(&path, "## урок 2\n").expect("append 2");
        let text = std::fs::read_to_string(&path).expect("read");
        assert_eq!(text, "## урок 1\n\n## урок 2\n", "append с разделителем");
    }

    #[test]
    fn append_to_agents_md_stays_outside_generated_zone() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("AGENTS.md");
        std::fs::write(
            &path,
            "# Рукописное\n\nПравила команды.\n\n<!-- ARCH:GENERATED hash=1 -->\nзона\n<!-- ARCH:END -->\n",
        )
        .expect("write");
        let lesson = Lesson {
            signature: "read_file:io: <путь> not found".into(),
            tool: "read_file".into(),
            class: "io: <путь> not found".into(),
            sample: "io: /a/x.txt: not found".into(),
            first_seen: "2026-08-26T00:00:00.000+03:00".into(),
            last_seen: "2026-08-26T00:01:00.000+03:00".into(),
            count: 2,
        };
        append_to_agents_md(tmp.path(), &lesson).expect("append 1");
        append_to_agents_md(tmp.path(), &lesson).expect("append 2");
        let text = std::fs::read_to_string(&path).expect("read");
        // Рукописная и сгенерированная зоны целы, дописка — после маркеров.
        assert!(text.contains("# Рукописное"));
        assert!(text.contains("<!-- ARCH:END -->"));
        let end_pos = text.find("<!-- ARCH:END -->").expect("маркер конца");
        let section_pos = text.find(AGENTS_SECTION).expect("секция уроков");
        assert!(section_pos > end_pos, "уроки вне зоны ARCH:GENERATED");
        assert_eq!(
            text.matches(AGENTS_SECTION).count(),
            1,
            "секция добавлена один раз"
        );
        assert_eq!(text.matches("read_file").count(), 2, "два пункта-урока");
    }

    #[test]
    fn append_to_agents_md_creates_missing_file() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let lesson = Lesson {
            signature: "bash:код 2".into(),
            tool: "bash".into(),
            class: "код 2".into(),
            sample: "exit 2".into(),
            first_seen: "t1".into(),
            last_seen: "t2".into(),
            count: 2,
        };
        let path = append_to_agents_md(tmp.path(), &lesson).expect("append");
        let text = std::fs::read_to_string(&path).expect("read");
        assert!(text.contains(AGENTS_SECTION));
        assert!(text.contains("`bash`"));
        assert!(path.ends_with("AGENTS.md"));
    }
}
