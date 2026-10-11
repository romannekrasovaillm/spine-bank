//! `constraints-harvest` — детерминированный сборщик фактов-ограничений
//! для секции Context ADR (цикл DECIDE) и гейтов GOVERN/COMPLY.
//!
//! КОНТРАКТ (владелец: агент `constraints-harvest`):
//! - [`harvest`] обходит дерево репозитория (без LLM, AD-2) и собирает шесть
//!   независимых секций фактов; каждый факт несёт доказательство
//!   `файл:строка` в стиле `[confirmed]`;
//! - секции: `performance` (нагрузочные тесты, заявленные RPS/latency),
//!   `ownership` (CODEOWNERS/OWNERS), `licenses_eol` (лицензии, платформенные
//!   версии, политики), `lockfiles` (верхние зависимости и диапазонные
//!   запреты), `env` (поверхность переменных окружения с маскировкой
//!   секретов), `invariants` (TODO/@Deprecated/feature-flags);
//! - [`render_md`] — человекочитаемый отчёт (секции + `файл:строка`);
//!   [`render_json`] — машиночитаемый отчёт со схемой [`SCHEMA`];
//! - ДЕТЕРМИНИЗМ: два прогона на одном дереве байт-в-байт идентичны
//!   (обход отсортирован, ни времени, ни случайности в выводе);
//! - ГРАНИЦА: только ФАКТЫ, без интерпретации и приоритизации — сбор
//!   детерминирован, решение о значимости остаётся за архитектором.
//!
//! Маскировка секретов опирается на общий набор правил
//! [`crate::secrets::Redactor`] (та же семантика, что у редакции вывода
//! инструментов): значение секрета в отчёт не попадает никогда.
//!
//! Стек-версии намеренно извлекаются локально (минимальный парсер),
//! а не через приватные механизмы `survey`: чужая зона не правится,
//! граница зафиксирована в ADR-073.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;

use serde::Serialize;
use walkdir::WalkDir;

use crate::error::{HarnessError, Result};
use crate::secrets::Redactor;
use crate::survey::{SKIP_DIRS, is_manifest_file};

/// Версия схемы JSON-отчёта.
pub const SCHEMA: &str = "arch-be/constraints-harvest/v1";

/// Максимальный размер файла, который читается целиком: 1 мебибайт.
const MAX_FILE_BYTES: u64 = 1024 * 1024;
/// Ограничение числа фактов в секции (переполнение — не шум, а сигнал).
const MAX_FACTS_PER_SECTION: usize = 60;
/// Сколько верхних зависимостей печатать из lock-файла.
const MAX_LOCK_DEPS: usize = 12;

/// Один факт-ограничение: утверждение и его доказательство `файл:строка`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Fact {
    /// Текст факта (без интерпретации).
    pub text: String,
    /// Доказательство в виде `путь:строка` (или `путь`).
    pub evidence: String,
}

impl Fact {
    /// Собирает факт.
    fn new(text: impl Into<String>, evidence: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            evidence: evidence.into(),
        }
    }
}

/// Секция отчёта: набор фактов или `gap`, если ничего не найдено.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Section {
    /// Машинный идентификатор секции.
    pub id: &'static str,
    /// Человекочитаемый заголовок.
    pub title: &'static str,
    /// Описание пробела, когда фактов нет (пусто, если факты найдены).
    #[serde(skip_serializing_if = "str_is_empty")]
    pub gap: &'static str,
    /// Собранные факты (отсортированы детерминированно).
    pub facts: Vec<Fact>,
}

/// Предикат сериализации: опускать поле `gap`, когда оно пусто.
fn str_is_empty(s: &&str) -> bool {
    s.is_empty()
}

/// Собирает секцию: контракт «факты ИЛИ один `gap`» — при найденных фактах
/// пробел обнуляется, в JSON ключ `gap` не попадает.
fn section(id: &'static str, title: &'static str, gap: &'static str, facts: Vec<Fact>) -> Section {
    let gap = if facts.is_empty() { gap } else { "" };
    Section {
        id,
        title,
        gap,
        facts,
    }
}

/// Результат сбора: имя репозитория и секции фактов.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Harvest {
    /// Имя корня репозитория (последний компонент пути).
    pub repo: String,
    /// Секции фактов в фиксированном порядке.
    pub sections: Vec<Section>,
}

impl Harvest {
    /// Суммарное число фактов по всем секциям.
    #[must_use]
    pub fn fact_count(&self) -> usize {
        self.sections.iter().map(|s| s.facts.len()).sum()
    }

    /// Число секций без фактов (пробелов).
    #[must_use]
    pub fn gap_count(&self) -> usize {
        self.sections.iter().filter(|s| s.facts.is_empty()).count()
    }
}

/// Пробелы секций (печатаются как `[gap]`).
const GAP_PERF: &str = "нагрузочные тесты и заявленные RPS/latency-ориентиры не найдены";
const GAP_OWNERSHIP: &str = "CODEOWNERS/OWNERS и модели владельцев не найдены";
const GAP_LICENSES: &str = "лицензии, платформенные версии и политики EOL не найдены";
const GAP_LOCKFILES: &str = "файлы блокировки зависимостей не найдены";
const GAP_ENV: &str = "поверхность переменных окружения (compose/k8s/dotenv) не найдена";
const GAP_INVARIANTS: &str = "скрытые инварианты кода (TODO/@Deprecated/feature-flags) не найдены";

/// Дерево репозитория: относительные пути (отсортированы) и содержимое
/// текстовых файлов в пределах [`MAX_FILE_BYTES`].
struct Tree {
    files: Vec<String>,
    contents: BTreeMap<String, String>,
}

/// Обходит репозиторий, пропуская каталоги из [`SKIP_DIRS`] (кроме корня),
/// и читает текстовые файлы небольшого размера.
///
/// # Errors
/// Каталог нечитаем (ошибка обхода `walkdir`).
fn walk(repo: &Path) -> Result<Tree> {
    let mut files = Vec::new();
    let mut contents = BTreeMap::new();
    let walker = WalkDir::new(repo)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| !is_skipped(entry));
    for entry in walker {
        let entry =
            entry.map_err(|e| HarnessError::Control(format!("обход {}: {e}", repo.display())))?;
        if !entry.file_type().is_file() {
            continue;
        }
        let Ok(rel) = entry.path().strip_prefix(repo) else {
            continue;
        };
        let rel = rel.to_string_lossy().replace('\\', "/");
        files.push(rel.clone());
        if entry.metadata().map_or(0, |m| m.len()) <= MAX_FILE_BYTES {
            // MSRV 1.85: let-цепочки нестабильны — вложенная проверка.
            if let Ok(text) = std::fs::read_to_string(entry.path()) {
                contents.insert(rel, text);
            }
        }
    }
    files.sort();
    Ok(Tree { files, contents })
}

/// Пропускает каталоги из [`SKIP_DIRS`] (корень не прокидывается в filter).
fn is_skipped(entry: &walkdir::DirEntry) -> bool {
    entry.file_type().is_dir() && SKIP_DIRS.contains(&entry.file_name().to_string_lossy().as_ref())
}

/// Имя файла из относительного пути.
fn file_name(rel: &str) -> &str {
    rel.rsplit('/').next().unwrap_or(rel)
}

/// Содержимое текстового файла, если оно было прочитано.
fn content<'a>(tree: &'a Tree, rel: &str) -> Option<&'a str> {
    tree.contents.get(rel).map(String::as_str)
}

/// Доказательство `путь:строка` по 0-based индексу строки.
fn file_line(rel: &str, idx: usize) -> String {
    format!("{rel}:{}", idx + 1)
}

/// Расширение файла без учёта регистра.
fn has_ext(rel: &str, ext: &str) -> bool {
    file_name(rel)
        .rsplit_once('.')
        .is_some_and(|(_, e)| e.eq_ignore_ascii_case(ext))
}

/// Усечение строки по символам с многоточием.
fn truncate(s: &str, max: usize) -> String {
    let mut chars = s.chars();
    let taken: String = chars.by_ref().take(max).collect();
    if chars.next().is_some() {
        format!("{taken}…")
    } else {
        taken
    }
}

/// Обрезает список фактов до [`MAX_FACTS_PER_SECTION`].
fn cap(mut facts: Vec<Fact>) -> Vec<Fact> {
    facts.truncate(MAX_FACTS_PER_SECTION);
    facts
}

/// Имя репозитория (последний компонент пути).
fn repo_name(repo: &Path) -> String {
    repo.file_name().map_or_else(
        || repo.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    )
}

// --- Хелперы коллекторов ----------------------------------------------------

/// Единицы скорости: число РЯДОМ с ними — заявленный ориентир (lowercase).
/// «Рядом» = в пределах [`PERF_RATE_BACK`] символов перед единицей.
const PERF_RATE_UNITS: &[&str] = &[
    "rps",
    "req/s",
    "request/s",
    "requests per second",
    "tps",
    "qps",
    "запрос/с",
];

/// Единицы времени задержки: число ВПЛОТНУЮ перед ними (`200 ms`, `200ms`).
/// Окно малое, чтобы не ловить слова на «-ms» (`systems`, `items`) и не
/// считать число за много слов до единицы. Время засчитывается ориентиром
/// ТОЛЬКО вместе с контекстом задержки ([`PERF_LATENCY_CONTEXT`]) — иначе
/// `10 секунд` в прозе стало бы ложным ориентиром.
const PERF_TIME_UNITS: &[&str] = &["ms", "мс", "µs", "us", "ns", "sec", "сек"];

/// Слова-контекст задержки/пропускной способности: без них число+единица
/// времени — просто длительность в тексте, а не заявленная цель.
const PERF_LATENCY_CONTEXT: &[&str] = &[
    "latency",
    "задержк",
    "отклик",
    "response",
    "бюджет",
    "budget",
    "пропускн",
    "throughput",
    "sla",
    "percentile",
    "перцентил",
    "p50",
    "p75",
    "p90",
    "p95",
    "p99",
];

/// Окно поиска числа перед единицей скорости (символы).
const PERF_RATE_BACK: usize = 7;
/// Окно поиска числа перед единицей времени (символы).
const PERF_TIME_BACK: usize = 2;

/// Файл — человекочитаемый документ (README/`.md`/`.txt`) или конфиг.
fn is_doc_or_config(rel: &str) -> bool {
    let name = file_name(rel).to_ascii_lowercase();
    name.starts_with("readme")
        || has_ext(rel, "md")
        || has_ext(rel, "markdown")
        || has_ext(rel, "txt")
        || has_ext(rel, "yaml")
        || has_ext(rel, "yml")
        || has_ext(rel, "toml")
        || has_ext(rel, "properties")
        || has_ext(rel, "json")
}

/// Есть ли число в пределах `back` символов перед вхождением `unit` в `lower`.
/// Границы среза — по символам (единицы `мс`/`µs` многобайтовые) — без паник.
///
/// Единица должна начинаться как отдельный токен: символ перед ней — не буква.
/// Иначе `tps` внутри `https`, `ms` внутри `systems`/`items` давали бы
/// ложные срабатывания (номера портов, URL в тексте).
fn number_before(lower: &str, unit: &str, back: usize) -> bool {
    let mut from = 0;
    while let Some(pos) = lower[from..].find(unit) {
        let abs = from + pos;
        let prev_is_letter = lower[..abs]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_ascii_alphabetic());
        if !prev_is_letter {
            let start = lower[..abs]
                .char_indices()
                .rev()
                .nth(back - 1)
                .map_or(0, |(i, _)| i);
            if lower[start..abs].bytes().any(|b| b.is_ascii_digit()) {
                return true;
            }
        }
        from = abs + unit.len();
    }
    false
}

/// Строка содержит заявленный ориентир производительности: число рядом с
/// единицей скорости/задержки. Узкое правило — чтобы не помечать ориентиром
/// любую строку «есть цифра и слово latency».
///
/// Единица скорости (`rps`, `req/s`) — сильный сигнал сама по себе. Единица
/// времени (`ms`, `sec`) засчитывается только при наличии контекста задержки
/// ([`PERF_LATENCY_CONTEXT`]): иначе длительность в прозе (`10 секунд`,
/// `14-second loop`) стала бы ложным ориентиром.
fn declared_perf_target(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    if PERF_RATE_UNITS
        .iter()
        .any(|u| number_before(&lower, u, PERF_RATE_BACK))
    {
        return true;
    }
    PERF_LATENCY_CONTEXT.iter().any(|c| lower.contains(c))
        && PERF_TIME_UNITS
            .iter()
            .any(|u| number_before(&lower, u, PERF_TIME_BACK))
}

/// Владельцы из строки CODEOWNERS: `шаблон владелец1 владелец2`.
fn split_codeowners(line: &str) -> Option<(String, String)> {
    let mut parts = line.split_whitespace();
    let pattern = parts.next()?;
    let owners: Vec<&str> = parts.collect();
    if owners.is_empty() {
        return None;
    }
    Some((pattern.to_string(), owners.join(" ")))
}

/// Значение XML-тега `<tag>…</tag>` (первое вхождение).
fn xml_tag_value(body: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = body.find(&open)? + open.len();
    let end = body[start..].find(&close)? + start;
    Some(body[start..end].trim().to_string())
}

/// Маркеры лицензий в тексте LICENSE-файла → SPDX-идентификатор.
const LICENSE_MARKERS: &[(&str, &str)] = &[
    ("Apache License", "Apache-2.0"),
    ("MIT License", "MIT"),
    ("Mozilla Public License", "MPL-2.0"),
    ("GNU LESSER GENERAL PUBLIC LICENSE", "LGPL"),
    ("GNU GENERAL PUBLIC LICENSE", "GPL"),
    ("BSD 3-Clause", "BSD-3-Clause"),
    ("BSD 2-Clause", "BSD-2-Clause"),
    ("ISC License", "ISC"),
    ("The Unlicense", "Unlicense"),
];

/// SPDX по тексту лицензии (первое совпадение маркера).
fn detect_license(body: &str) -> Option<&'static str> {
    LICENSE_MARKERS
        .iter()
        .find(|(marker, _)| body.contains(marker))
        .map(|(_, spdx)| *spdx)
}

/// Файл является текстом лицензии по имени.
fn is_license_file(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    upper == "LICENSE"
        || upper == "LICENSE.MD"
        || upper == "LICENSE.TXT"
        || upper == "COPYING"
        || upper == "COPYING.MD"
}

/// Факт о платформенной версии из манифеста, если она объявлена.
fn platform_version_fact(rel: &str, body: &str) -> Option<Fact> {
    let name = file_name(rel).to_ascii_lowercase();
    let (label, version) = if name == "cargo.toml" {
        let v = body
            .lines()
            .find_map(|l| l.trim().strip_prefix("rust-version"))
            .and_then(|l| l.split('=').nth(1))
            .map(|v| v.trim().trim_matches('"').to_string());
        ("Rust (rust-version)", v?)
    } else if name == "pom.xml" {
        let v = xml_tag_value(body, "java.version")
            .or_else(|| xml_tag_value(body, "maven.compiler.release"))
            .or_else(|| xml_tag_value(body, "maven.compiler.source"));
        ("Java (maven)", v?)
    } else if name.ends_with(".csproj") {
        (
            "TargetFramework (.NET)",
            xml_tag_value(body, "TargetFramework")?,
        )
    } else if name == "go.mod" {
        let v = body
            .lines()
            .find_map(|l| l.trim().strip_prefix("go "))
            .map(|v| v.trim().to_string());
        ("Go (go.mod)", v?)
    } else if name == "pyproject.toml" {
        let v = body
            .lines()
            .find_map(|l| l.trim().strip_prefix("requires-python"))
            .and_then(|l| l.split('=').nth(1))
            .map(|v| v.trim().trim_matches('"').to_string());
        ("Python (requires-python)", v?)
    } else if name == "package.json" {
        let v = json_string_value(body, "node");
        ("Node (engines.node)", v?)
    } else if name == "build.gradle" || name == "build.gradle.kts" {
        let v = body
            .lines()
            .find_map(|l| l.trim().strip_prefix("sourceCompatibility"))
            .and_then(|l| l.split('=').nth(1))
            .map(|v| v.trim().trim_matches('"').to_string());
        ("Java (Gradle sourceCompatibility)", v?)
    } else {
        return None;
    };
    Some(Fact::new(
        format!("платформенная версия: {label} = {version}"),
        rel.to_string(),
    ))
}

/// Строковое значение ключа из JSON-текста (грубый поиск `"key": "value"`).
fn json_string_value(body: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\"");
    let start = body.find(&needle)? + needle.len();
    let rest = &body[start..];
    let colon = rest.find(':')?;
    let after = rest[colon + 1..].trim_start();
    let after = after.strip_prefix('"')?;
    let end = after.find('"')?;
    Some(after[..end].to_string())
}

/// Извлекает версию из строки вида `name = "1.2.3"` после префикса.
fn quoted_after(line: &str, prefix: &str) -> Option<String> {
    let rest = line.trim().strip_prefix(prefix)?;
    let rest = rest.trim().trim_start_matches('=').trim();
    let val = rest.trim_matches(|c| c == '"' || c == '\'');
    if val.is_empty() {
        None
    } else {
        Some(val.to_string())
    }
}

/// Версия-спецификация является «плавающим» диапазоном (диапазонный запрет).
fn is_floating_range(spec: &str) -> bool {
    let s = spec.trim();
    s.is_empty()
        || s == "*"
        || s == "latest"
        || ["^", "~", ">", "<", ">=", ">="]
            .iter()
            .any(|p| s.starts_with(p))
        || s.contains('x')
        || s.contains('X')
}

/// Имя переменной окружения (`UPPER_SNAKE`, минимум два символа).
fn is_env_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_uppercase() => {}
        _ => return false,
    }
    name.len() >= 2
        && name
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
}

/// Разбирает строку окружения `KEY=VALUE` / `KEY: VALUE` на пару.
fn split_env_pair(line: &str) -> Option<(String, String)> {
    let (k, v) = line.split_once('=').or_else(|| line.split_once(':'))?;
    let key = k.trim().trim_start_matches('-').trim().to_string();
    if !is_env_name(&key) {
        return None;
    }
    Some((
        key,
        v.trim().trim_matches('"').trim_matches('\'').to_string(),
    ))
}

/// Добавляет факт о переменной окружения, маскируя значение-секрет.
fn record_env(
    out: &mut Vec<Fact>,
    redactor: &Redactor,
    key: &str,
    value: Option<&str>,
    evidence: String,
) {
    let shown = match value {
        Some(v) if !v.is_empty() && redactor.contains_secret(&format!("{key}={v}")) => {
            format!("{key}=***")
        }
        _ => key.to_string(),
    };
    out.push(Fact::new(format!("env: {shown}"), evidence));
}

/// Маркеры скрытых инвариантов в комментариях.
const INVARIANT_MARKERS: &[&str] = &["TODO", "FIXME", "HACK", "XXX"];

/// Комментарий из строки кода или сама строка, если это комментарий целиком.
fn comment_text(line: &str) -> Option<String> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }
    let marker = trimmed.find("//").or_else(|| trimmed.find('#'))?;
    let text = trimmed[marker..].trim_start_matches(['/', '#']).trim();
    if text.is_empty() {
        None
    } else {
        Some(truncate(text, 100))
    }
}

/// Имя фича-флага Rust на строке `#[cfg(feature = "…")]`.
fn rust_feature_flag(line: &str) -> Option<String> {
    let trimmed = line.trim();
    if !trimmed.starts_with("#[cfg(") || !trimmed.contains("feature") {
        return None;
    }
    let start = trimmed.find("feature")?;
    let rest = &trimmed[start..];
    let eq = rest.find('"').or_else(|| rest.find('\''))?;
    let quote = rest.as_bytes()[eq] as char;
    let body = &rest[eq + 1..];
    let end = body.find(quote)?;
    Some(body[..end].to_string())
}

/// Файл пригоден для текстового сканирования инвариантов.
fn is_scannable(rel: &str) -> bool {
    !is_license_file(file_name(rel))
        && !rel.contains("Cargo.lock")
        && !rel.contains("package-lock.json")
        && !rel.contains("go.sum")
}

// --- Коллекторы -------------------------------------------------------------

fn collect_performance(tree: &Tree) -> Vec<Fact> {
    let mut facts = Vec::new();
    for rel in &tree.files {
        let name = file_name(rel);
        let lower = name.to_ascii_lowercase();
        if lower.starts_with("locustfile") {
            facts.push(Fact::new(
                format!("нагрузочный сценарий Locust: {name}"),
                rel.clone(),
            ));
        } else if has_ext(rel, "jmx") {
            facts.push(Fact::new(
                format!("нагрузочный план JMeter: {name}"),
                rel.clone(),
            ));
        } else if lower.ends_with("simulation.scala") || lower.ends_with("simulation.java") {
            facts.push(Fact::new(format!("сценарий Gatling: {name}"), rel.clone()));
        } else if has_ext(rel, "js")
            && content(tree, rel).is_some_and(|c| c.contains("k6") || c.contains("from 'k6"))
        {
            facts.push(Fact::new(
                format!("нагрузочный сценарий k6: {name}"),
                rel.clone(),
            ));
        } else if rel.contains("/benches/") || has_ext(rel, "rs") && rel.starts_with("benches/") {
            facts.push(Fact::new(
                format!("бенчмарк criterion: {name}"),
                rel.clone(),
            ));
        }
    }
    // Заявленные ориентиры из документов и конфигов.
    for rel in &tree.files {
        if !is_doc_or_config(rel) {
            continue;
        }
        let Some(body) = content(tree, rel) else {
            continue;
        };
        for (idx, line) in body.lines().enumerate() {
            if declared_perf_target(line) {
                facts.push(Fact::new(
                    format!("заявленный ориентир: {}", truncate(line.trim(), 120)),
                    file_line(rel, idx),
                ));
            }
        }
    }
    facts
}

fn collect_ownership(tree: &Tree) -> Vec<Fact> {
    let mut facts = Vec::new();
    for rel in &tree.files {
        let name = file_name(rel);
        let lower = name.to_ascii_lowercase();
        if lower == "codeowners" {
            let Some(body) = content(tree, rel) else {
                continue;
            };
            for (idx, line) in body.lines().enumerate() {
                let trimmed = line.trim();
                if trimmed.is_empty() || trimmed.starts_with('#') {
                    continue;
                }
                if let Some((pattern, owners)) = split_codeowners(trimmed) {
                    facts.push(Fact::new(
                        format!("владелец `{pattern}` → {owners}"),
                        file_line(rel, idx),
                    ));
                }
            }
        } else if lower == "owners" || lower == "owners.yaml" || lower == "owners.yml" {
            let Some(body) = content(tree, rel) else {
                continue;
            };
            for (idx, line) in body.lines().enumerate() {
                let trimmed = line.trim().trim_start_matches('-').trim();
                if trimmed.is_empty() || trimmed.starts_with('#') {
                    continue;
                }
                facts.push(Fact::new(
                    format!("владелец: {}", truncate(trimmed, 80)),
                    file_line(rel, idx),
                ));
            }
        } else if rel.contains("model/OWNER") || lower.starts_with("owner-") {
            facts.push(Fact::new(format!("модель владельцев: {name}"), rel.clone()));
        }
    }
    facts
}

fn collect_licenses(tree: &Tree) -> Vec<Fact> {
    let mut facts = Vec::new();
    for rel in &tree.files {
        let name = file_name(rel);
        // MSRV 1.85: let-цепочки нестабильны — вложенные проверки.
        if is_license_file(name) {
            if let Some(body) = content(tree, rel) {
                if let Some(spdx) = detect_license(body) {
                    facts.push(Fact::new(format!("лицензия: {spdx}"), rel.clone()));
                }
            }
        }
        if is_manifest_file(name) {
            if let Some(body) = content(tree, rel) {
                if let Some(fact) = platform_version_fact(rel, body) {
                    facts.push(fact);
                }
            }
            // Лицензия из манифестов пакетных менеджеров.
            if name == "package.json" {
                if let Some(body) = content(tree, rel) {
                    if let Some(lic) = json_string_value(body, "license") {
                        facts.push(Fact::new(format!("лицензия: {lic}"), rel.clone()));
                    }
                }
            }
            if name == "pyproject.toml" || name == "deny.toml" {
                if let Some(body) = content(tree, rel) {
                    if let Some(idx) = body.lines().position(|l| l.trim().starts_with("license")) {
                        let line = body.lines().nth(idx).unwrap_or("").trim();
                        facts.push(Fact::new(
                            format!("политика лицензий: {}", truncate(line, 100)),
                            file_line(rel, idx),
                        ));
                    }
                }
            }
        }
    }
    facts
}

fn collect_lockfiles(tree: &Tree) -> Vec<Fact> {
    let mut facts = Vec::new();
    // Cargo.lock: верхние зависимости name/version.
    if let Some(body) = tree.contents.get("Cargo.lock") {
        let mut deps = Vec::new();
        let mut cur: Option<(Option<String>, Option<String>)> = None;
        for line in body.lines() {
            if line.trim() == "[[package]]" {
                if let Some((Some(n), v)) = cur.take() {
                    deps.push((n, v.unwrap_or_default()));
                }
                cur = Some((None, None));
            } else if let Some((name, version)) = cur.as_mut() {
                // MSRV 1.85: let-цепочки нестабильны — вложенные проверки.
                if name.is_none() {
                    if let Some(n) = quoted_after(line, "name") {
                        *name = Some(n);
                    }
                }
                if version.is_none() {
                    if let Some(v) = quoted_after(line, "version") {
                        *version = Some(v);
                    }
                }
            }
        }
        if let Some((Some(n), v)) = cur.take() {
            deps.push((n, v.unwrap_or_default()));
        }
        for (name, version) in deps.iter().take(MAX_LOCK_DEPS) {
            facts.push(Fact::new(
                format!("зависимость Cargo.lock: {name} {version}"),
                "Cargo.lock".to_string(),
            ));
        }
    }
    // package.json: диапазонные (плавающие) зависимости.
    if let Some(body) = tree.contents.get("package.json") {
        for (idx, line) in body.lines().enumerate() {
            let trimmed = line.trim();
            let Some((key, rest)) = trimmed.split_once(':') else {
                continue;
            };
            let dep = key.trim().trim_matches('"');
            if dep.is_empty() || dep.starts_with('{') {
                continue;
            }
            let spec = rest.trim().trim_matches(',').trim().trim_matches('"');
            if spec.is_empty() || spec.starts_with('{') || spec.starts_with('[') {
                continue;
            }
            if is_floating_range(spec) {
                facts.push(Fact::new(
                    format!("диапазонный запрет: {dep} {spec}"),
                    file_line("package.json", idx),
                ));
            }
        }
    }
    // requirements.txt: пинованные версии.
    for rel in &tree.files {
        // MSRV 1.85: let-цепочки нестабильны — вложенная проверка.
        if file_name(rel) == "requirements.txt" {
            if let Some(body) = content(tree, rel) {
                for (idx, line) in body.lines().enumerate() {
                    let trimmed = line.trim();
                    if trimmed.is_empty() || trimmed.starts_with('#') {
                        continue;
                    }
                    facts.push(Fact::new(
                        format!("зависимость: {}", truncate(trimmed, 80)),
                        file_line(rel, idx),
                    ));
                }
            }
        }
    }
    facts
}

fn collect_env(tree: &Tree, redactor: &Redactor) -> Vec<Fact> {
    let mut facts = Vec::new();
    for rel in &tree.files {
        let name = file_name(rel).to_ascii_lowercase();
        let Some(body) = content(tree, rel) else {
            continue;
        };
        if name == ".env" || name.starts_with(".env.") {
            for (idx, line) in body.lines().enumerate() {
                let trimmed = line.trim();
                if trimmed.is_empty() || trimmed.starts_with('#') {
                    continue;
                }
                if let Some((key, value)) = split_env_pair(trimmed) {
                    record_env(
                        &mut facts,
                        redactor,
                        &key,
                        Some(&value),
                        file_line(rel, idx),
                    );
                }
            }
        } else if has_ext(rel, "yml") || has_ext(rel, "yaml") || name == "dockerfile" {
            for (idx, line) in body.lines().enumerate() {
                let trimmed = line.trim();
                let candidate = trimmed
                    .strip_prefix("- ")
                    .or_else(|| trimmed.strip_prefix("ENV "))
                    .or_else(|| trimmed.strip_prefix("ARG "))
                    .or_else(|| trimmed.strip_prefix("name: "))
                    .unwrap_or(trimmed);
                if let Some((key, value)) = split_env_pair(candidate) {
                    // Интересны только `K: V`/`K=V` в контексте env-блоков.
                    if !is_env_name(&key) {
                        continue;
                    }
                    record_env(
                        &mut facts,
                        redactor,
                        &key,
                        Some(&value),
                        file_line(rel, idx),
                    );
                }
            }
        }
    }
    facts
}

fn collect_invariants(tree: &Tree) -> Vec<Fact> {
    let mut facts = Vec::new();
    for rel in &tree.files {
        if !is_scannable(rel) {
            continue;
        }
        let Some(body) = content(tree, rel) else {
            continue;
        };
        for (idx, line) in body.lines().enumerate() {
            if let Some(flag) = rust_feature_flag(line) {
                facts.push(Fact::new(
                    format!("feature-flag: {flag}"),
                    file_line(rel, idx),
                ));
                continue;
            }
            if line.contains("@Deprecated") || line.contains("#[deprecated") {
                facts.push(Fact::new(
                    format!("устаревание: {}", truncate(line.trim(), 100)),
                    file_line(rel, idx),
                ));
                continue;
            }
            // MSRV 1.85: let-цепочки нестабильны — вложенная проверка.
            if let Some(text) = comment_text(line) {
                if INVARIANT_MARKERS.iter().any(|m| text.contains(m)) {
                    facts.push(Fact::new(
                        format!("маркер: {}", truncate(&text, 100)),
                        file_line(rel, idx),
                    ));
                }
            }
        }
    }
    facts
}

// --- Публичный API ----------------------------------------------------------

/// Собирает факты-ограничения по дереву репозитория (детерминированно).
///
/// # Errors
/// Дерево нечитаемо (ошибка обхода каталога).
pub fn harvest(repo: &Path) -> Result<Harvest> {
    let tree = walk(repo)?;
    let redactor = Redactor::with_builtin_rules();
    let sections = vec![
        section(
            "performance",
            "Производительность и нагрузочные ориентиры",
            GAP_PERF,
            cap(collect_performance(&tree)),
        ),
        section(
            "ownership",
            "Владение и точки эскалации",
            GAP_OWNERSHIP,
            cap(collect_ownership(&tree)),
        ),
        section(
            "licenses_eol",
            "Лицензии, платформенные версии и EOL",
            GAP_LICENSES,
            cap(collect_licenses(&tree)),
        ),
        section(
            "lockfiles",
            "Зависимости и диапазонные запреты",
            GAP_LOCKFILES,
            cap(collect_lockfiles(&tree)),
        ),
        section(
            "env",
            "Поверхность переменных окружения",
            GAP_ENV,
            cap(collect_env(&tree, &redactor)),
        ),
        section(
            "invariants",
            "Скрытые инварианты кода",
            GAP_INVARIANTS,
            cap(collect_invariants(&tree)),
        ),
    ];
    Ok(Harvest {
        repo: repo_name(repo),
        sections,
    })
}

/// Рендерит Markdown-отчёт: секции с фактами `[confirmed] (файл:строка)`
/// и пробелами `[gap]`. Без времени и случайности — вывод детерминирован.
#[must_use]
pub fn render_md(harvest: &Harvest) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "# Ограничения: {}", harvest.repo);
    let _ = writeln!(out);
    let _ = writeln!(out, "Схема: {SCHEMA}");
    for section in &harvest.sections {
        let _ = writeln!(out, "\n## {}", section.title);
        if section.facts.is_empty() {
            let _ = writeln!(out, "- [gap] {}", section.gap);
        } else {
            for fact in &section.facts {
                let _ = writeln!(out, "- [confirmed] {} ({})", fact.text, fact.evidence);
            }
        }
    }
    out
}

/// Обёртка JSON-отчёта: схема + развёрнутый [`Harvest`].
#[derive(Serialize)]
struct HarvestJson<'a> {
    schema: &'static str,
    #[serde(flatten)]
    harvest: &'a Harvest,
}

/// Рендерит JSON-отчёт со схемой [`SCHEMA`] (детерминированный порядок ключей).
///
/// # Errors
/// Ошибка сериализации (практически недостижима для плоской структуры).
pub fn render_json(harvest: &Harvest) -> Result<String> {
    let payload = HarvestJson {
        schema: SCHEMA,
        harvest,
    };
    serde_json::to_string_pretty(&payload).map_err(HarnessError::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Создаёт временное дерево из набора `путь → содержимое`.
    fn fixture(files: &[(&str, &str)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tmp");
        for (path, body) in files {
            let full = dir.path().join(path);
            if let Some(parent) = full.parent() {
                std::fs::create_dir_all(parent).expect("mkdir");
            }
            std::fs::write(&full, body).expect("write");
        }
        dir
    }

    /// Факты секции по идентификатору.
    fn facts<'a>(harvest: &'a Harvest, id: &str) -> &'a [Fact] {
        &harvest
            .sections
            .iter()
            .find(|s| s.id == id)
            .expect("секция")
            .facts
    }

    #[test]
    fn performance_finds_locust_and_declared_target() {
        let dir = fixture(&[
            ("locustfile.py", "from locust import HttpUser\n"),
            (
                "README.md",
                "# Сервис\n\n## Производительность\n\n- Цель: 500 rps при p99 < 200 ms\n",
            ),
        ]);
        let h = harvest(dir.path()).expect("harvest");
        let perf = facts(&h, "performance");
        assert!(
            perf.iter().any(|f| f.evidence.contains("locustfile.py")),
            "Locust найден: {perf:?}"
        );
        assert!(
            perf.iter().any(|f| f.text.contains("500 rps")),
            "заявленный ориентир найден: {perf:?}"
        );
    }

    #[test]
    fn declared_target_is_number_adjacent_to_unit_not_any_keyword() {
        // Ориентир: число вплотную к единице скорости/задержки.
        assert!(declared_perf_target("- Цель: 500 rps при p99 < 200 ms"));
        assert!(declared_perf_target("latency budget: 250ms"));
        assert!(declared_perf_target("throughput 1000 req/s"));
        // Не ориентир: цифра и слово «latency» в разных смыслах (шум
        // golden-фикстур: ссылка на NFR, где число — это токен, не цель).
        assert!(!declared_perf_target(
            "← affects от NFR-001 (Latency перевода C2C)"
        ));
        // Не ориентир: число далеко от единицы.
        assert!(!declared_perf_target("12 систем, latency описана ниже"));
        // Не ориентир: слова на «ms» без числа вплотную.
        assert!(!declared_perf_target("набор items в каталоге"));
        // Не ориентир: единица внутри слова (граница токена).
        // `tps` внутри `https`, `ms` внутри `systems` — не единицы.
        assert!(!declared_perf_target(
            "Prometheus, порт 7011 (HTTP) / 7012 (HTTPS)."
        ));
        assert!(!declared_perf_target(
            "`https://web.archive.org/web/2024/…`"
        ));
        assert!(!declared_perf_target("разбор systems и items в каталоге"));
    }

    #[test]
    fn ownership_parses_codeowners() {
        let dir = fixture(&[(
            ".github/CODEOWNERS",
            "# владельцы\n*.rs @backend-team\n/services/ @platform\n",
        )]);
        let h = harvest(dir.path()).expect("harvest");
        let own = facts(&h, "ownership");
        assert!(
            own.iter().any(|f| f.text.contains("@backend-team")),
            "владелец rust: {own:?}"
        );
        assert!(
            own.iter().any(|f| f.text.contains("@platform")),
            "владелец сервисов: {own:?}"
        );
    }

    #[test]
    fn licenses_detects_spdx_and_java_version() {
        let dir = fixture(&[
            ("LICENSE", "MIT License\n\nCopyright (c) 2026\n"),
            (
                "pom.xml",
                "<project><properties><java.version>17</java.version></properties></project>\n",
            ),
        ]);
        let h = harvest(dir.path()).expect("harvest");
        let lic = facts(&h, "licenses_eol");
        assert!(
            lic.iter().any(|f| f.text.contains("MIT")),
            "лицензия MIT: {lic:?}"
        );
        assert!(
            lic.iter().any(|f| f.text.contains("17")),
            "версия Java: {lic:?}"
        );
    }

    #[test]
    fn lockfiles_reads_cargo_lock_and_json_range() {
        let dir = fixture(&[
            (
                "Cargo.lock",
                "version = 4\n\n[[package]]\nname = \"serde\"\nversion = \"1.0.200\"\n",
            ),
            (
                "package.json",
                "{\n  \"dependencies\": {\n    \"lodash\": \"^4.17.0\"\n  }\n}\n",
            ),
        ]);
        let h = harvest(dir.path()).expect("harvest");
        let locks = facts(&h, "lockfiles");
        assert!(
            locks.iter().any(|f| f.text.contains("serde")),
            "зависимость из Cargo.lock: {locks:?}"
        );
        assert!(
            locks
                .iter()
                .any(|f| f.text.contains("lodash") && f.text.to_lowercase().contains("диапазон")),
            "диапазонный запрет: {locks:?}"
        );
    }

    #[test]
    fn env_lists_names_and_masks_password() {
        let dir = fixture(&[(".env", "DB_PASSWORD=hunter2secret\nPORT=8080\n")]);
        let h = harvest(dir.path()).expect("harvest");
        let env = facts(&h, "env");
        assert!(
            env.iter().any(|f| f.text.contains("DB_PASSWORD=***")),
            "секрет замаскирован: {env:?}"
        );
        assert!(
            env.iter().any(|f| f.text.contains("PORT")),
            "несекретное имя видно: {env:?}"
        );
        assert!(
            env.iter().all(|f| !f.text.contains("hunter2secret")),
            "значение секрета не протекло: {env:?}"
        );
    }

    #[test]
    fn invariants_find_todo_deprecated_and_flag() {
        let dir = fixture(&[(
            "src/a.rs",
            "// TODO: пересмотреть границы модуля\n#[deprecated(note = \"legacy\")]\nfn old() {}\n#[cfg(feature = \"beta\")]\nfn new() {}\n",
        )]);
        let h = harvest(dir.path()).expect("harvest");
        let inv = facts(&h, "invariants");
        assert!(inv.iter().any(|f| f.text.contains("TODO")), "TODO: {inv:?}");
        assert!(
            inv.iter().any(|f| f.text.contains("deprecated")),
            "deprecated: {inv:?}"
        );
        assert!(
            inv.iter().any(|f| f.text.contains("beta")),
            "feature-flag: {inv:?}"
        );
    }

    #[test]
    fn json_has_schema_and_is_deterministic() {
        let dir = fixture(&[
            (".env", "PORT=8080\n"),
            ("README.md", "# X\n\n- latency 50 ms\n"),
        ]);
        let a = harvest(dir.path()).expect("harvest a");
        let b = harvest(dir.path()).expect("harvest b");
        assert_eq!(a, b, "структура детерминирована");
        let ja = render_json(&a).expect("json a");
        let jb = render_json(&b).expect("json b");
        assert_eq!(ja, jb, "JSON байт-в-байт");
        assert!(ja.contains(SCHEMA), "схема в отчёте: {ja}");
        assert_eq!(render_md(&a), render_md(&b), "Markdown байт-в-байт");
        // Контракт секции: факты ИЛИ один `gap` — взаимоисключающе.
        let v: serde_json::Value = serde_json::from_str(&ja).expect("json value");
        let sections = v["sections"].as_array().expect("sections");
        let env = sections.iter().find(|s| s["id"] == "env").expect("env");
        assert!(env.get("gap").is_none(), "секция с фактами не несёт gap");
        let own = sections
            .iter()
            .find(|s| s["id"] == "ownership")
            .expect("own");
        assert!(own["gap"].is_string(), "пустая секция несёт gap: {own}");
    }

    #[test]
    fn empty_repo_reports_all_gaps() {
        let dir = fixture(&[]);
        let h = harvest(dir.path()).expect("harvest");
        assert_eq!(h.fact_count(), 0, "фактов нет");
        assert_eq!(h.gap_count(), 6, "шесть пробелов");
        let md = render_md(&h);
        assert_eq!(md.matches("[gap]").count(), 6, "шесть [gap]: {md}");
    }
}
