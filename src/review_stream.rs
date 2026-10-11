//! Потоковый журнал вердиктов ревью (AB-09, «REVIEW gap») — пакетный прогон
//! `arch-be review stream <repo> --last N` по линейной истории коммитов с
//! НАКОПЛЕНИЕМ вердиктов в `.arch-review/stream.jsonl`.
//!
//! Задача, которую решает модуль: поток из десятков PR за несколько сеансов
//! нельзя оценить по одному диффу — вердикт `arch-diff` одноразовый, состояние
//! между сеансами теряется. Журнал вердиктов — память потока (ADR-076):
//! - **пакетный прогон**: для каждого из N последних коммитов первого родителя
//!   считается архитектурный дифф `родитель..коммит` (маршрут, триггеры, дельта
//!   графа) — сводка «сколько fast/standard/critical, какие коммиты подняли
//!   маршрут, значимые по составу против шума»;
//! - **журнал** (`.arch-review/stream.jsonl`, одна строка — один вердикт):
//!   повторный прогон с перекрывающимся диапазоном НЕ дублирует записи
//!   (upsert по sha), старые записи сохраняются — накопление между сеансами;
//! - **кумулятивные метрики эрозии** (`review stream --report`): доля значимых,
//!   динамика маршрутов по окнам, топ-триггеры, кумулятивный счётчик inferred-рёбер
//!   (механика D3, ADR-075);
//! - **ждут подтверждения**: записи журнала старше N дней — вход ящика ревью
//!   ([`crate::review_inbox`]).
//!
//! Границы и правила:
//! - файлы, не сервис (ADR-033): журнал — обычный jsonl в репозитории;
//!   в `.gitignore` его НЕ добавляем осознанно (память потока должна быть
//!   видна в репозитории; решение об исключении — за владельцем репозитория);
//! - детерминизм (правило 10): записи сортируются по (дата, sha); повторный
//!   прогон на том же дереве даёт байт-в-байт тот же журнал;
//! - толерантность к краю: корневой коммит (нет родителя) и битый дифф —
//!   fail-soft (запись в `skipped` с причиной), а не падение всего потока;
//! - только чтение в `--report`; запись — лишь upsert в журнал при прогоне.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::arch_diff::{ArchDiff, ArchDiffInput, Confidence, arch_diff};
use crate::control::DiffGlobs;
use crate::error::{HarnessError, Result};

/// Схема машинного отчёта потока (`--format json`).
pub const REVIEW_STREAM_SCHEMA: &str = "arch-be/review-stream/v1";

/// Путь журнала вердиктов относительно корня репозитория.
pub const STREAM_JOURNAL_PATH: &str = ".arch-review/stream.jsonl";

/// Известное пустое дерево git — база корневого коммита (нет родителя).
const EMPTY_TREE: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";

/// Ранг маршрута `standard` — порог значимости записи (D3).
const STANDARD_RANK: u8 = 2;

/// Порог «ждёт подтверждения» по умолчанию (дней) для ящика ревью.
pub const DEFAULT_AWAITING_DAYS: u64 = 7;

/// Одна запись журнала: вердикт архитектурного диффа по коммиту.
///
/// Поля аддитивны к схеме `v1` (ADR-063/076): добавление поля — минорно.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamVerdict {
    /// Полный sha коммита-головы (ключ upsert).
    pub sha: String,
    /// Дата коммита (ISO-8601, `%cI`) — ось окон и возраста.
    pub date: String,
    /// Маршрут значимости (`fast`/`standard`/`critical`).
    pub route: String,
    /// Счёт значимости (число сработавших триггеров).
    pub score: usize,
    /// Имена сработавших триггеров (отсортированы, без дублей).
    #[serde(default)]
    pub triggers: Vec<String>,
    /// Есть ли архитектурная дельта графа (узлы/рёбра/контракты/NFR).
    pub graph_delta: bool,
    /// Добавленные узлы (вклад в «значимость по составу»).
    #[serde(default)]
    pub added_nodes: usize,
    /// Добавленные рёбра.
    #[serde(default)]
    pub added_edges: usize,
    /// Добавленные рёбра с достоверностью `inferred` (D3): ждут подтверждения.
    #[serde(default)]
    pub inferred_edges: usize,
    /// Хэш вердикта (`sha256:<hex>` по канонической строке полей) — детекция
    /// подмены записи журнала.
    #[serde(default)]
    pub verdict_hash: String,
}

impl StreamVerdict {
    /// Ранг маршрута записи (critical=3, standard=2, fast=1, прочее=0).
    #[must_use]
    pub fn rank(&self) -> u8 {
        route_rank(&self.route)
    }

    /// Значима ли запись: маршрут не ниже `standard` ИЛИ непустая дельта графа.
    #[must_use]
    pub fn significant(&self) -> bool {
        self.rank() >= STANDARD_RANK || self.graph_delta
    }

    /// Ждёт ли запись подтверждения архитектора (D3): есть inferred-рёбра.
    #[must_use]
    pub fn awaiting_confirmation(&self) -> bool {
        self.inferred_edges > 0
    }
}

/// Коммит, пропущенный при прогоне (корневой или битый дифф): fail-soft с
/// причиной, а не падение потока.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkippedCommit {
    /// sha коммита.
    pub sha: String,
    /// Причина пропуска (по-русски).
    pub reason: String,
}

/// Итог пакетного прогона: вердикты + пропущенные коммиты.
#[derive(Debug, Clone)]
pub struct StreamRun {
    /// Вердикты по коммитам (хронологический порядок).
    pub verdicts: Vec<StreamVerdict>,
    /// Пропущенные коммиты с причинами.
    pub skipped: Vec<SkippedCommit>,
}

/// Статистика одного окна журнала (для динамики маршрутов).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowStat {
    /// Номер окна (1-based).
    pub index: usize,
    /// Дата первого коммита окна.
    pub from: String,
    /// Дата последнего коммита окна.
    pub to: String,
    /// Число коммитов окна.
    pub commits: usize,
    /// Распределение маршрутов в окне.
    pub fast: usize,
    /// Standard в окне.
    pub standard: usize,
    /// Critical в окне.
    pub critical: usize,
    /// Значимых в окне.
    pub significant: usize,
    /// Кумулятивный счётчик inferred-рёбер к концу окна (механика D3).
    pub inferred_cumulative: usize,
}

/// Кумулятивный отчёт по журналу (`review stream --report`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamReport {
    /// Всего записей в журнале.
    pub total: usize,
    /// Размер окна (число коммитов).
    pub window: usize,
    /// Распределение маршрутов по всему журналу.
    pub fast: usize,
    /// Standard по журналу.
    pub standard: usize,
    /// Critical по журналу.
    pub critical: usize,
    /// Значимых по журналу (маршрут ≥ standard или дельта графа).
    pub significant: usize,
    /// Записей с непустой дельтой графа.
    pub graph_deltas: usize,
    /// Кумулятивный счётчик inferred-рёбер по журналу.
    pub inferred_total: usize,
    /// Топ-триггеры по частоте (имя, частота), по убыванию частоты.
    pub top_triggers: Vec<(String, usize)>,
    /// Окна (динамика маршрутов).
    pub windows: Vec<WindowStat>,
}

impl StreamReport {
    /// Доля значимых записей, ‰ (промилле) — избегаем float в контракте.
    #[must_use]
    pub fn significant_permille(&self) -> usize {
        if self.total == 0 {
            return 0;
        }
        self.significant * 1000 / self.total
    }
}

/// Запись «ждёт подтверждения»: значимый вердикт журнала старше N дней (для
/// inferred-рёбер — прямое «требует подтверждения архитектора»).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AwaitingEntry {
    /// Репозиторий (путь относительно корня обхода).
    pub repo: String,
    /// sha коммита.
    pub sha: String,
    /// Дата коммита.
    pub date: String,
    /// Маршрут.
    pub route: String,
    /// inferred-рёбра (D3).
    pub inferred_edges: usize,
    /// Возраст записи в днях (на момент обхода).
    pub age_days: i64,
    /// Путь журнала относительно корня обхода.
    pub journal: String,
}

/// Ранг маршрута для сортировки (незнакомый маршрут — ниже fast).
fn route_rank(route: &str) -> u8 {
    match route.trim().to_ascii_lowercase().as_str() {
        "critical" => 3,
        "standard" => 2,
        "fast" => 1,
        _ => 0,
    }
}

/// Путь журнала вердиктов репозитория.
#[must_use]
pub fn journal_path(repo: &Path) -> PathBuf {
    repo.join(STREAM_JOURNAL_PATH)
}

/// Текущее время epoch-секунд (UTC) — для расчёта возраста записей.
#[must_use]
pub fn now_epoch() -> i64 {
    chrono::Utc::now().timestamp()
}

/// Канонический хэш вердикта (`sha256:<hex>`): по полям, кроме самого хэша.
/// Смена любого поля меняет хэш — подмена записи журнала детектируется.
#[must_use]
pub fn verdict_hash(verdict: &StreamVerdict) -> String {
    let canon = format!(
        "{}\0{}\0{}\0{}\0{}\0{}\0{}\0{}\0{}",
        verdict.sha,
        verdict.date,
        verdict.route,
        verdict.score,
        verdict.triggers.join(","),
        verdict.graph_delta,
        verdict.added_nodes,
        verdict.added_edges,
        verdict.inferred_edges,
    );
    format!("sha256:{}", crate::hash::sha256_hex(canon.as_bytes()))
}

/// Upsert записи по sha: та же sha — замена (пересчёт вердикта), иначе
/// добавление. Возвращает `true`, если запись добавлена впервые.
pub fn upsert(records: &mut Vec<StreamVerdict>, incoming: StreamVerdict) -> bool {
    if let Some(slot) = records.iter_mut().find(|r| r.sha == incoming.sha) {
        *slot = incoming;
        false
    } else {
        records.push(incoming);
        true
    }
}

/// Разбор журнала (строгий): битая строка — ошибка с номером. Пустые строки
/// пропускаются.
///
/// # Errors
/// Строка не разбирается как [`StreamVerdict`].
pub fn parse_journal(text: &str) -> std::result::Result<Vec<StreamVerdict>, String> {
    let mut out = Vec::new();
    for (idx, line) in text.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        match serde_json::from_str::<StreamVerdict>(trimmed) {
            Ok(v) => out.push(v),
            Err(e) => return Err(format!("строка {}: не разобран вердикт ({e})", idx + 1)),
        }
    }
    Ok(out)
}

/// Толерантный разбор журнала: битые строки пропускаются (обход флота не
/// должен падать из-за одного журнала).
#[must_use]
pub fn parse_lines_tolerant(text: &str) -> Vec<StreamVerdict> {
    text.lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                None
            } else {
                serde_json::from_str::<StreamVerdict>(trimmed).ok()
            }
        })
        .collect()
}

/// Чтение журнала репозитория. Нет файла — пустой журнал (не ошибка).
///
/// # Errors
/// Файл есть, но не читается или содержит битую строку.
pub fn load_journal(repo: &Path) -> Result<Vec<StreamVerdict>> {
    let path = journal_path(repo);
    match std::fs::read_to_string(&path) {
        Ok(text) => parse_journal(&text)
            .map_err(|e| HarnessError::Control(format!("журнал потока {}: {e}", path.display()))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(HarnessError::io(&path, e)),
    }
}

/// Запись журнала: сортировка по (дата, sha) для детерминизма, пересчёт
/// хэшей, атомарная замена (temp + rename). Возвращает путь журнала.
///
/// # Errors
/// Каталог журнала не создаётся или файл не пишется.
pub fn save_journal(repo: &Path, records: &mut [StreamVerdict]) -> Result<PathBuf> {
    let path = journal_path(repo);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| HarnessError::io(parent, e))?;
    }
    records.sort_by(|a, b| a.date.cmp(&b.date).then_with(|| a.sha.cmp(&b.sha)));
    for r in records.iter_mut() {
        r.verdict_hash = verdict_hash(r);
    }
    let mut text = String::new();
    for r in records.iter() {
        // Запись в String не может завершиться ошибкой — игнор безопасен.
        let _ = writeln!(text, "{}", serde_json::to_string(r)?);
    }
    let tmp = path.with_extension("jsonl.tmp");
    std::fs::write(&tmp, text.as_bytes()).map_err(|e| HarnessError::io(&tmp, e))?;
    std::fs::rename(&tmp, &path).map_err(|e| HarnessError::io(&path, e))?;
    Ok(path)
}

/// Список коммитов первого родителя (последние `last`), в хронологическом
/// порядке: `(sha, дата ISO-8601)`.
///
/// # Errors
/// Не git-репозиторий или `git log` не отработал.
fn list_first_parent(repo: &Path, last: usize) -> Result<Vec<(String, String)>> {
    let n = format!("-n{}", last.max(1));
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["log", "--first-parent", &n, "--format=%H%x00%cI"])
        .stdin(Stdio::null())
        .output()
        .map_err(|e| HarnessError::Control(format!("review stream: не запустился git ({e})")))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(HarnessError::Control(format!(
            "review stream: git log не отработал в {}: {}",
            repo.display(),
            err.trim()
        )));
    }
    let mut rows: Vec<(String, String)> = Vec::new();
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        if let Some((sha, date)) = line.split_once('\0') {
            rows.push((sha.to_string(), date.to_string()));
        }
    }
    // git log — новейшие сверху; окна и накопление ждут хронологию.
    rows.reverse();
    Ok(rows)
}

/// Первый родитель коммита; у корневого — известное пустое дерево.
fn parent_of(repo: &Path, sha: &str) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["rev-parse", "--verify", "--quiet", &format!("{sha}^")])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output();
    match out {
        Ok(o) if o.status.success() => {
            let parent = String::from_utf8_lossy(&o.stdout).trim().to_string();
            if parent.is_empty() {
                EMPTY_TREE.to_string()
            } else {
                parent
            }
        }
        _ => EMPTY_TREE.to_string(),
    }
}

/// Вердикт по готовому диффу: маршрут, триггеры, дельта графа, inferred-рёбра.
fn verdict_from_diff(sha: &str, date: &str, diff: &ArchDiff) -> StreamVerdict {
    let mut triggers: Vec<String> = diff.route.triggers.iter().map(|t| t.name.clone()).collect();
    triggers.sort();
    triggers.dedup();
    let inferred = diff
        .added_edges
        .iter()
        .filter(|e| e.edge.confidence == Some(Confidence::Inferred))
        .count();
    let mut verdict = StreamVerdict {
        sha: sha.to_string(),
        date: date.to_string(),
        route: diff.route.route.clone(),
        score: diff.route.score,
        triggers,
        graph_delta: !diff.is_empty(),
        added_nodes: diff.added_nodes.len(),
        added_edges: diff.added_edges.len(),
        inferred_edges: inferred,
        verdict_hash: String::new(),
    };
    verdict.verdict_hash = verdict_hash(&verdict);
    verdict
}

/// Пакетный прогон: вердикт архитектурного диффа `родитель..коммит` для
/// каждого из `last` последних коммитов первого родителя.
///
/// # Errors
/// Не git-репозиторий или `git log` не отработал (ошибки отдельных диффов —
/// fail-soft в `skipped`).
pub fn run_stream(
    repo: &Path,
    last: usize,
    limits: (usize, usize),
    globs: &DiffGlobs,
) -> Result<StreamRun> {
    let rows = list_first_parent(repo, last)?;
    let mut verdicts = Vec::with_capacity(rows.len());
    let mut skipped = Vec::new();
    for (sha, date) in rows {
        let base = parent_of(repo, &sha);
        if base == EMPTY_TREE {
            skipped.push(SkippedCommit {
                sha,
                reason: "корневой коммит — базовая ревизия недоступна".to_string(),
            });
            continue;
        }
        let input = ArchDiffInput {
            base: &base,
            head: Some(&sha),
            declared: BTreeMap::new(),
            limits,
            globs,
        };
        match arch_diff(repo, &input) {
            Ok(diff) => verdicts.push(verdict_from_diff(&sha, &date, &diff)),
            Err(e) => skipped.push(SkippedCommit {
                sha,
                reason: e.to_string(),
            }),
        }
    }
    Ok(StreamRun { verdicts, skipped })
}

/// Кумулятивный отчёт по журналу: значимость, динамика маршрутов по окнам,
/// топ-триггеры, кумулятивный счётчик inferred-рёбер.
#[must_use]
pub fn report(records: &[StreamVerdict], window: usize) -> StreamReport {
    let window = window.max(1);
    let mut ordered: Vec<&StreamVerdict> = records.iter().collect();
    ordered.sort_by(|a, b| a.date.cmp(&b.date).then_with(|| a.sha.cmp(&b.sha)));

    let mut fast = 0;
    let mut standard = 0;
    let mut critical = 0;
    let mut significant = 0;
    let mut graph_deltas = 0;
    let mut inferred_total = 0usize;
    let mut trigger_freq: BTreeMap<String, usize> = BTreeMap::new();
    for v in &ordered {
        match route_rank(&v.route) {
            3 => critical += 1,
            2 => standard += 1,
            1 => fast += 1,
            _ => {}
        }
        if v.significant() {
            significant += 1;
        }
        if v.graph_delta {
            graph_deltas += 1;
        }
        inferred_total += v.inferred_edges;
        for t in &v.triggers {
            *trigger_freq.entry(t.clone()).or_insert(0) += 1;
        }
    }

    let mut windows = Vec::new();
    let mut cumulative = 0usize;
    for (i, chunk) in ordered.chunks(window).enumerate() {
        let mut w = WindowStat {
            index: i + 1,
            from: chunk.first().map_or_else(String::new, |v| v.date.clone()),
            to: chunk.last().map_or_else(String::new, |v| v.date.clone()),
            commits: chunk.len(),
            fast: 0,
            standard: 0,
            critical: 0,
            significant: 0,
            inferred_cumulative: 0,
        };
        for v in chunk {
            match route_rank(&v.route) {
                3 => w.critical += 1,
                2 => w.standard += 1,
                1 => w.fast += 1,
                _ => {}
            }
            if v.significant() {
                w.significant += 1;
            }
            cumulative += v.inferred_edges;
        }
        w.inferred_cumulative = cumulative;
        windows.push(w);
    }

    let mut top_triggers: Vec<(String, usize)> = trigger_freq.into_iter().collect();
    // Частота по убыванию, при равенстве — имя по возрастанию (детерминизм).
    top_triggers.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

    StreamReport {
        total: ordered.len(),
        window,
        fast,
        standard,
        critical,
        significant,
        graph_deltas,
        inferred_total,
        top_triggers,
        windows,
    }
}

/// Возраст записи в днях (`None` — дата не разобрана).
fn age_days(date: &str, now_epoch: i64) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(date)
        .ok()
        .map(|d| (now_epoch - d.timestamp()).div_euclid(86_400))
}

/// Записи журнала, ждущие подтверждения: значимые (или с inferred-рёбрами)
/// и старше `older_than_days` дней. Сортировка «старейшие сверху», затем sha.
///
/// # Errors
/// Отсутствуют: функция чистая (ошибки дат поглощаются как «не ждёт»).
#[must_use]
pub fn awaiting(
    records: &[StreamVerdict],
    older_than_days: u64,
    now_epoch: i64,
) -> Vec<AwaitingEntry> {
    let threshold = older_than_days.min(i64::MAX as u64) as i64;
    let mut out: Vec<AwaitingEntry> = Vec::new();
    for v in records {
        if !(v.significant() || v.awaiting_confirmation()) {
            continue;
        }
        let Some(days) = age_days(&v.date, now_epoch) else {
            continue;
        };
        if days < threshold {
            continue;
        }
        out.push(AwaitingEntry {
            repo: String::new(),
            sha: v.sha.clone(),
            date: v.date.clone(),
            route: v.route.clone(),
            inferred_edges: v.inferred_edges,
            age_days: days,
            journal: String::new(),
        });
    }
    out.sort_by(|a, b| b.age_days.cmp(&a.age_days).then_with(|| a.sha.cmp(&b.sha)));
    out
}

/// Обход корня флота в поисках журналов `.arch-review/stream.jsonl` и сбор
/// записей, ждущих подтверждения (толерантно: чужие/битые журналы пропускаются).
///
/// # Errors
/// Корень недоступен или не каталог.
pub fn awaiting_under(
    root: &Path,
    older_than_days: u64,
    now_epoch: i64,
) -> Result<Vec<AwaitingEntry>> {
    if !root.is_dir() {
        return Err(HarnessError::Control(format!(
            "review stream: корень недоступен или не каталог: {}",
            root.display()
        )));
    }
    let mut paths: Vec<PathBuf> = Vec::new();
    let walker = walkdir::WalkDir::new(root).follow_links(false).into_iter();
    for entry in walker.filter_entry(|e| {
        // Пропускаем только сборочные каталоги и `.git`; `.arch-review` —
        // dot-каталог, в который как раз нужно заходить.
        if e.depth() > 0 && e.file_type().is_dir() {
            let name = e.file_name().to_string_lossy();
            !(name == ".git" || crate::survey::SKIP_DIRS.contains(&name.as_ref()))
        } else {
            true
        }
    }) {
        let entry = entry.map_err(|e| {
            HarnessError::Control(format!("review stream: обход {}: {e}", root.display()))
        })?;
        if entry.file_type().is_file() && entry.path().ends_with(STREAM_JOURNAL_PATH) {
            paths.push(entry.path().to_path_buf());
        }
    }
    paths.sort();

    let mut out = Vec::new();
    for path in &paths {
        let Ok(text) = std::fs::read_to_string(path) else {
            continue;
        };
        let rel = path
            .strip_prefix(root)
            .map_or_else(|_| path.display().to_string(), |p| p.display().to_string());
        let repo_dir = path.parent().and_then(Path::parent);
        let repo = repo_dir.map_or_else(
            || ".".to_string(),
            |p| {
                p.strip_prefix(root)
                    .map_or_else(|_| p.display().to_string(), |r| r.display().to_string())
            },
        );
        let repo = if repo.is_empty() {
            ".".to_string()
        } else {
            repo
        };
        for mut entry in awaiting(&parse_lines_tolerant(&text), older_than_days, now_epoch) {
            entry.repo.clone_from(&repo);
            entry.journal.clone_from(&rel);
            out.push(entry);
        }
    }
    // Старейшие сверху по всему флоту; при равенстве — репозиторий, затем sha.
    out.sort_by(|a, b| {
        b.age_days
            .cmp(&a.age_days)
            .then_with(|| a.repo.cmp(&b.repo))
            .then_with(|| a.sha.cmp(&b.sha))
    });
    Ok(out)
}

/// Текстовый рендер итога прогона: маршруты, значимое против шума, окна.
#[must_use]
pub fn render_run(run: &StreamRun, window: usize) -> String {
    let rep = report(&run.verdicts, window);
    let mut out = String::new();
    let _ = writeln!(
        out,
        "Поток ревью: {} коммитов · fast {} · standard {} · critical {} · значимых {}",
        rep.total, rep.fast, rep.standard, rep.critical, rep.significant
    );
    let noise = rep.total.saturating_sub(rep.significant);
    let _ = writeln!(
        out,
        "Значимое: {} ({}‰) · шум (без архитектурной дельты): {} · дельт графа: {} · \
         inferred-рёбер: {}",
        rep.significant,
        rep.significant_permille(),
        noise,
        rep.graph_deltas,
        rep.inferred_total
    );
    render_windows(&mut out, &rep);
    if !rep.top_triggers.is_empty() {
        let _ = writeln!(out, "\nТоп-триггеры:");
        for (name, freq) in rep.top_triggers.iter().take(10) {
            let _ = writeln!(out, "  {freq:>4}  {name}");
        }
    }
    if !run.skipped.is_empty() {
        let _ = writeln!(out, "\nПропущены коммиты (fail-soft):");
        for s in &run.skipped {
            let _ = writeln!(out, "  [warn] {} — {}", short_sha(&s.sha), s.reason);
        }
    }
    out
}

/// Текстовый рендер кумулятивного отчёта по журналу.
#[must_use]
pub fn render_report(rep: &StreamReport) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "Кумулятивный отчёт потока (журнал): {} записей · fast {} · standard {} · critical {}",
        rep.total, rep.fast, rep.standard, rep.critical
    );
    let noise = rep.total.saturating_sub(rep.significant);
    let _ = writeln!(
        out,
        "Доля значимых: {} ({}‰) · шум: {} · дельт графа: {} · inferred-рёбер всего: {}",
        rep.significant,
        rep.significant_permille(),
        noise,
        rep.graph_deltas,
        rep.inferred_total
    );
    render_windows(&mut out, rep);
    if !rep.top_triggers.is_empty() {
        let _ = writeln!(out, "\nТоп-триггеры:");
        for (name, freq) in rep.top_triggers.iter().take(10) {
            let _ = writeln!(out, "  {freq:>4}  {name}");
        }
    }
    out
}

/// Таблица окон (общая для рендера прогона и отчёта).
fn render_windows(out: &mut String, rep: &StreamReport) {
    if rep.windows.is_empty() {
        return;
    }
    let _ = writeln!(
        out,
        "\nОкна по {} коммитов (динамика маршрутов):",
        rep.window
    );
    let _ = writeln!(
        out,
        "  {:>4}  {:<10} {:<10}  {:>3} {:>3} {:>3}  {:>4} {:>5}",
        "окно", "от", "до", "f", "s", "c", "знач", "inf∑"
    );
    for w in &rep.windows {
        let _ = writeln!(
            out,
            "  {:>4}  {:<10} {:<10}  {:>3} {:>3} {:>3}  {:>4} {:>5}",
            w.index,
            short_date(&w.from),
            short_date(&w.to),
            w.fast,
            w.standard,
            w.critical,
            w.significant,
            w.inferred_cumulative
        );
    }
}

/// Первые 10 знаков даты (ISO-8601) — компактность таблицы.
fn short_date(date: &str) -> &str {
    let n = date.len().min(10);
    &date[..n]
}

/// Короткая форма sha (12 знаков).
fn short_sha(sha: &str) -> &str {
    let n = sha.len().min(12);
    &sha[..n]
}

/// Машинный отчёт прогона (`--format json`), схема `arch-be/review-stream/v1`,
/// `mode: "run"`.
#[must_use]
pub fn run_json(run: &StreamRun, window: usize) -> Value {
    let rep = report(&run.verdicts, window);
    let skipped: Vec<Value> = run
        .skipped
        .iter()
        .map(|s| json!({"commit": s.sha, "reason": s.reason}))
        .collect();
    json!({
        "schema": REVIEW_STREAM_SCHEMA,
        "mode": "run",
        "total": rep.total,
        "routes": {"fast": rep.fast, "standard": rep.standard, "critical": rep.critical},
        "significant": rep.significant,
        "significant_permille": rep.significant_permille(),
        "graph_deltas": rep.graph_deltas,
        "inferred_edges": rep.inferred_total,
        "top_triggers": rep.top_triggers.iter().map(|(n, f)| json!({"name": n, "count": f})).collect::<Vec<_>>(),
        "windows": windows_json(&rep),
        "verdicts": run.verdicts.iter().map(verdict_json).collect::<Vec<_>>(),
        "skipped": skipped,
    })
}

/// Машинный кумулятивный отчёт по журналу, `mode: "report"`.
#[must_use]
pub fn report_json(rep: &StreamReport) -> Value {
    json!({
        "schema": REVIEW_STREAM_SCHEMA,
        "mode": "report",
        "total": rep.total,
        "routes": {"fast": rep.fast, "standard": rep.standard, "critical": rep.critical},
        "significant": rep.significant,
        "significant_permille": rep.significant_permille(),
        "graph_deltas": rep.graph_deltas,
        "inferred_edges": rep.inferred_total,
        "top_triggers": rep.top_triggers.iter().map(|(n, f)| json!({"name": n, "count": f})).collect::<Vec<_>>(),
        "windows": windows_json(rep),
    })
}

/// JSON-представление вердикта (для `run_json`).
fn verdict_json(v: &StreamVerdict) -> Value {
    json!({
        "sha": v.sha,
        "date": v.date,
        "route": v.route,
        "score": v.score,
        "triggers": v.triggers,
        "graph_delta": v.graph_delta,
        "added_nodes": v.added_nodes,
        "added_edges": v.added_edges,
        "inferred_edges": v.inferred_edges,
        "verdict_hash": v.verdict_hash,
    })
}

/// JSON-представление окон.
fn windows_json(rep: &StreamReport) -> Vec<Value> {
    rep.windows
        .iter()
        .map(|w| {
            json!({
                "index": w.index,
                "from": w.from,
                "to": w.to,
                "commits": w.commits,
                "fast": w.fast,
                "standard": w.standard,
                "critical": w.critical,
                "significant": w.significant,
                "inferred_cumulative": w.inferred_cumulative,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Вердикт для тестов: путь через публичные поля (журнал не требует git).
    fn verdict(sha: &str, date: &str, route: &str, graph_delta: bool) -> StreamVerdict {
        let mut v = StreamVerdict {
            sha: sha.to_string(),
            date: date.to_string(),
            route: route.to_string(),
            score: route_rank(route) as usize,
            triggers: vec!["contract".to_string()],
            graph_delta,
            added_nodes: usize::from(graph_delta),
            added_edges: usize::from(graph_delta),
            inferred_edges: 0,
            verdict_hash: String::new(),
        };
        v.verdict_hash = verdict_hash(&v);
        v
    }

    #[test]
    fn upsert_replaces_by_sha_and_preserves_order() {
        let mut records = vec![
            verdict("aa", "2026-01-01T00:00:00+00:00", "fast", false),
            verdict("bb", "2026-01-02T00:00:00+00:00", "standard", true),
        ];
        // Новая sha — добавление.
        let added = upsert(
            &mut records,
            verdict("cc", "2026-01-03T00:00:00+00:00", "critical", true),
        );
        assert!(added);
        assert_eq!(records.len(), 3);
        // Повторная sha — замена, не дубликат (и позиция сохраняется).
        let added = upsert(
            &mut records,
            verdict("bb", "2026-01-02T00:00:00+00:00", "fast", false),
        );
        assert!(!added);
        assert_eq!(records.len(), 3);
        assert_eq!(records[1].route, "fast");
        assert_eq!(records[1].sha, "bb");
    }

    #[test]
    fn accumulation_survives_overlapping_ranges() {
        // Сеанс 1: два коммита.
        let mut journal: Vec<StreamVerdict> = Vec::new();
        for sha in ["a1", "a2"] {
            upsert(
                &mut journal,
                verdict(sha, "2026-02-01T00:00:00+00:00", "fast", false),
            );
        }
        assert_eq!(journal.len(), 2);
        // Сеанс 2: перекрывающийся диапазон + новый коммит — старые не теряются.
        for sha in ["a2", "a3"] {
            upsert(
                &mut journal,
                verdict(sha, "2026-02-02T00:00:00+00:00", "standard", true),
            );
        }
        assert_eq!(
            journal.len(),
            3,
            "перекрытие не дублирует, новые добавляются"
        );
        assert!(journal.iter().any(|v| v.sha == "a1"));
        assert!(journal.iter().any(|v| v.sha == "a3"));
    }

    #[test]
    fn report_windows_and_inferred_cumulative() {
        let mut records = Vec::new();
        for (i, route) in ["fast", "standard", "critical", "fast", "standard"]
            .iter()
            .enumerate()
        {
            let mut v = verdict(
                &format!("s{i}"),
                &format!("2026-03-{:02}T00:00:00+00:00", i + 1),
                route,
                route != &"fast",
            );
            v.inferred_edges = usize::from(route == &"standard");
            records.push(v);
        }
        let rep = report(&records, 2);
        assert_eq!(rep.total, 5);
        assert_eq!(rep.window, 2);
        assert_eq!((rep.fast, rep.standard, rep.critical), (2, 2, 1));
        assert_eq!(rep.windows.len(), 3);
        assert_eq!(rep.windows[0].commits, 2);
        assert_eq!(rep.windows[2].commits, 1);
        // Кумулятивный счётчик inferred не убывает и доходит до суммы.
        // inferred по окнам: (fast,standard)=1, (critical,fast)=0, (standard)=1.
        assert_eq!(rep.windows[0].inferred_cumulative, 1);
        assert_eq!(rep.windows[1].inferred_cumulative, 1);
        assert_eq!(rep.windows[2].inferred_cumulative, 2);
        assert_eq!(rep.inferred_total, 2);
        // Топ-триггеры присутствуют (у всех записей один и тот же триггер).
        assert_eq!(
            rep.top_triggers.first().map(|(n, _)| n.as_str()),
            Some("contract")
        );
    }

    #[test]
    fn significant_counts_route_and_graph_delta() {
        let a = verdict("a", "2026-01-01T00:00:00+00:00", "fast", false);
        let b = verdict("b", "2026-01-02T00:00:00+00:00", "fast", true);
        let c = verdict("c", "2026-01-03T00:00:00+00:00", "standard", false);
        assert!(!a.significant(), "fast без дельты — шум");
        assert!(b.significant(), "дельта графа значима даже на fast");
        assert!(c.significant(), "standard значим");
        // Промилле: 2 из 3 → 666‰.
        let rep = report(&[a, b, c], 10);
        assert_eq!(rep.significant, 2);
        assert_eq!(rep.significant_permille(), 666);
    }

    #[test]
    fn journal_roundtrip_sorted_and_hashed() {
        let dir = tempfile::tempdir().expect("tmp");
        let repo = dir.path();
        let mut records = vec![
            verdict("z", "2026-05-02T00:00:00+00:00", "fast", false),
            verdict("a", "2026-05-01T00:00:00+00:00", "standard", true),
        ];
        save_journal(repo, &mut records).expect("save");
        // Порядок после save — по (дата, sha).
        assert_eq!(records[0].sha, "a");
        assert_eq!(records[1].sha, "z");
        assert!(
            records
                .iter()
                .all(|v| v.verdict_hash.starts_with("sha256:"))
        );
        let back = load_journal(repo).expect("load");
        assert_eq!(back, records, "журнал читается байт-в-байт");
    }

    #[test]
    fn load_journal_missing_is_empty() {
        let dir = tempfile::tempdir().expect("tmp");
        assert_eq!(load_journal(dir.path()).expect("load").len(), 0);
    }

    #[test]
    fn parse_journal_strict_on_broken_line() {
        let text = "{\"sha\":\"a\",\"date\":\"2026-01-01T00:00:00+00:00\",\"route\":\"fast\",\"score\":0,\"graph_delta\":false}\n{oops}\n";
        assert!(parse_journal(text).is_err());
        // Толерантный режим — та же строка просто пропускается.
        let tolerant = parse_lines_tolerant(text);
        assert_eq!(tolerant.len(), 1);
        assert_eq!(tolerant[0].sha, "a");
    }

    #[test]
    fn awaiting_selects_old_significant_records() {
        let now = chrono::DateTime::parse_from_rfc3339("2026-06-30T00:00:00+00:00")
            .expect("ts")
            .timestamp();
        let old_sig = verdict("old", "2026-06-01T00:00:00+00:00", "standard", true);
        let old_noise = verdict("noise", "2026-06-01T00:00:00+00:00", "fast", false);
        let mut old_inf = verdict("inf", "2026-06-10T00:00:00+00:00", "standard", false);
        old_inf.inferred_edges = 2;
        let fresh_sig = verdict("fresh", "2026-06-29T00:00:00+00:00", "critical", true);
        let records = vec![old_sig, old_noise, old_inf, fresh_sig];
        let await_list = awaiting(&records, 7, now);
        let shas: Vec<&str> = await_list.iter().map(|e| e.sha.as_str()).collect();
        assert!(shas.contains(&"old"), "старый значимый ждёт");
        assert!(shas.contains(&"inf"), "inferred ждёт подтверждения");
        assert!(!shas.contains(&"noise"), "шум не ждёт");
        assert!(!shas.contains(&"fresh"), "свежее не ждёт");
        // Старейшее сверху.
        assert_eq!(await_list.first().map(|e| e.sha.as_str()), Some("old"));
        assert!(await_list.iter().all(|e| e.age_days >= 7));
    }

    #[test]
    fn awaiting_under_walks_fleet_journals() {
        let dir = tempfile::tempdir().expect("tmp");
        let root = dir.path();
        let repo_a = root.join("svc-a");
        let repo_b = root.join("svc-b");
        std::fs::create_dir_all(&repo_a).expect("mkdir a");
        std::fs::create_dir_all(&repo_b).expect("mkdir b");
        // Журнал репозитория A: старая значимая запись.
        let mut a = vec![verdict("aa", "2026-01-01T00:00:00+00:00", "standard", true)];
        save_journal(&repo_a, &mut a).expect("save a");
        // Журнал репозитория B: битая строка + свежая запись.
        let b_journal = journal_path(&repo_b);
        std::fs::create_dir_all(b_journal.parent().expect("parent")).expect("mkdir .arch-review");
        std::fs::write(
            &b_journal,
            "not-json\n{\"sha\":\"bb\",\"date\":\"2026-06-20T00:00:00+00:00\",\"route\":\"critical\",\"score\":1,\"graph_delta\":true}\n",
        )
        .expect("write b");
        let now = chrono::DateTime::parse_from_rfc3339("2026-06-30T00:00:00+00:00")
            .expect("ts")
            .timestamp();
        let found = awaiting_under(root, 7, now).expect("awaiting");
        assert_eq!(found.len(), 2, "по одной записи из каждого журнала");
        assert_eq!(found[0].repo, "svc-a", "старейшее сверху");
        assert_eq!(found[0].sha, "aa");
        assert!(
            found
                .iter()
                .all(|e| e.journal.ends_with(STREAM_JOURNAL_PATH))
        );
    }

    #[test]
    fn report_json_has_schema_and_mode() {
        let rep = report(
            &[verdict("a", "2026-01-01T00:00:00+00:00", "standard", true)],
            10,
        );
        let v = report_json(&rep);
        assert_eq!(v["schema"], REVIEW_STREAM_SCHEMA);
        assert_eq!(v["mode"], "report");
        assert_eq!(v["significant"], 1);
        assert_eq!(v["routes"]["standard"], 1);
    }
}
