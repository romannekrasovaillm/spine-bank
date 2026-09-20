//! Библиотека шаблонов исполняемых правил (`arch-be rules template …`).
//!
//! Задача библиотеки — сократить путь от «инвариант записан» до «инвариант
//! проверяется исполнением». Паспорт вердикта честно говорит, что правило на
//! упоминание зеленеет и когда инвариант соблюдён, и когда о нём просто
//! написали; шаблон даёт готовое правило `command_succeeds` вместе с тестом
//! свойства, фейком внешней системы и эталонной реализацией, на которой тест
//! зелёный из коробки.
//!
//! Три принципа (ADR-050):
//!
//! - **П1. Свойство, а не протокол.** Тест проверяет архитектурное свойство на
//!   фейке внешней системы («после таймаута ровно одна отправка»), а не
//!   конкретный протокол: только так шаблон переносим между кейсами.
//! - **П2. Беззубый тест хуже текстового правила.** У каждого шаблона есть
//!   нарушающая реализация, на которой тест обязан падать, и
//!   [`verify_dir`]/[`verify_all`] это проверяют механикой.
//! - **П3. Spine предлагает, архитектор принимает.** `apply` кладёт файлы и
//!   **печатает** фрагмент правила и строку `verified_by`; реестр правил и
//!   спайн этот модуль не правит — это защищённые файлы, изменение идёт
//!   дельтой.
//!
//! Модуль детерминированный и без LLM: подбор шаблона — счёт совпадений
//! словаря `match` с текстом инварианта, проверка зубов — прогон тестов на
//! эталонной и нарушающей реализациях.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::error::{HarnessError, Result};

/// Каталог шаблонов внутри распакованных ассетов (`~/.arch-harness/assets/`).
pub const TEMPLATES_REL: &str = "assets/rule-templates";

/// Каталог, куда `apply` кладёт файлы шаблона внутри кейса.
pub const TARGET_REL: &str = "skeleton/rule_templates";

/// Файл блокировки применённых шаблонов внутри кейса.
pub const LOCK_REL: &str = ".arch-handoff/rule-templates.lock";

/// Заготовка для нераспознанного паттерна: участвует в подборе как честная
/// пометка «паттерн не распознан», но не является исполняемым шаблоном.
pub const GENERIC_TEMPLATE_ID: &str = "generic-property-test";

/// Имя инварианта, которое `apply` проверяет в спайне: `AD-<n>`.
const AD_PREFIX: &str = "AD-";

/// Потолок прогона python-теста шаблона, если в правиле не задан свой.
const DEFAULT_RUN_TIMEOUT: Duration = Duration::from_secs(120);

/// Потолок прогона java-половины: первая сборка Maven тянет зависимости.
const JAVA_RUN_TIMEOUT: Duration = Duration::from_secs(600);

// ---------------------------------------------------------------------------
// Манифест шаблона
// ---------------------------------------------------------------------------

/// Манифест шаблона (`template.yaml`).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TemplateManifest {
    /// Стабильный id шаблона (совпадает с именем каталога).
    pub id: String,
    /// Версия шаблона; растёт при изменении файлов или свойств.
    #[serde(default = "default_version")]
    pub version: u32,
    /// Заголовок для человека.
    pub title: String,
    /// Описание паттерна словами (что за архитектурная ситуация).
    pub pattern: String,
    /// Словарь подбора: с чем сравнивается текст инварианта.
    #[serde(rename = "match", default)]
    pub match_terms: MatchTerms,
    /// Проверяемые свойства — по одному на тест.
    #[serde(default)]
    pub properties: Vec<String>,
    /// Заготовка правила `CONSTRAINTS.yaml`.
    pub rule: RuleSpec,
    /// Файлы, которые `apply` кладёт в кейс.
    #[serde(default)]
    pub files: Vec<FileSpec>,
    /// Пары «какой файл кейса заменить нарушающей реализацией» (П2).
    #[serde(default)]
    pub violating: Vec<ViolatingSwap>,
    /// Шаблон исполняемый: у него есть тесты свойств и нарушающая реализация.
    /// Заготовка (`generic-property-test`) — `false`: правило печатается
    /// закомментированным, проверка зубов её пропскает.
    #[serde(default = "default_true")]
    pub executable: bool,
}

/// Словарь подбора шаблона: основы слов (подстрока), русские и английские.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct MatchTerms {
    /// Русские основы («идемпотент», «повторн»).
    #[serde(default)]
    pub ru: Vec<String>,
    /// Английские основы («idempot», «dedup»).
    #[serde(default)]
    pub en: Vec<String>,
}

impl MatchTerms {
    /// Все основы словаря в порядке «русские, затем английские».
    fn iter(&self) -> impl Iterator<Item = &String> {
        self.ru.iter().chain(self.en.iter())
    }
}

/// Заготовка правила `command_succeeds` из манифеста.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RuleSpec {
    /// Имя правила (kebab/snake-case, становится кодом находки).
    pub name: String,
    /// Тип правила; библиотека поставляет только `command_succeeds`.
    #[serde(rename = "type")]
    pub kind: String,
    /// Severity заготовки: по умолчанию `error` — нарушение инварианта обязано
    /// краснеть (`control check` → exit 1). Понижает до `warn` архитектор, если
    /// проверка ещё не перенесена на настоящий код.
    #[serde(default = "default_severity")]
    pub severity: String,
    /// Таймаут прогона команды, секунды.
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
    /// Зачем правило: что доказывает исполняемая проверка.
    pub rationale: String,
    /// Что делать, когда правило краснеет.
    pub fix_hint: String,
    /// Скилл-источник методики.
    #[serde(default)]
    pub skill: Option<String>,
    /// Команды по языкам.
    #[serde(default)]
    pub commands: Commands,
}

/// Команды прогона тестов шаблона по языкам.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Commands {
    /// Команда python-половины (pytest).
    #[serde(default)]
    pub python: Option<String>,
    /// Команда java-половины (Maven).
    #[serde(default)]
    pub java: Option<String>,
}

impl Commands {
    /// Команда языка, если она объявлена.
    fn for_lang(&self, lang: &str) -> Option<&str> {
        match lang {
            "java" => self.java.as_deref(),
            _ => self.python.as_deref(),
        }
    }
}

/// Файл шаблона, который `apply` кладёт в кейс.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct FileSpec {
    /// Язык файла: `python`, `java` или `common` (общий).
    #[serde(default = "default_lang")]
    pub lang: String,
    /// Путь внутри каталога шаблона.
    pub from: String,
    /// Путь внутри `skeleton/rule_templates/<id>/` кейса.
    pub to: String,
}

/// Подмена файла кейса нарушающей реализацией (проверка зубов, П2).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ViolatingSwap {
    /// Язык подменяемого файла.
    #[serde(default = "default_lang")]
    pub lang: String,
    /// Путь внутри `skeleton/rule_templates/<id>/` кейса.
    pub to: String,
    /// Путь нарушающей реализации внутри каталога шаблона.
    pub from: String,
}

fn default_version() -> u32 {
    1
}

fn default_true() -> bool {
    true
}

fn default_lang() -> String {
    "common".to_string()
}

fn default_severity() -> String {
    "error".to_string()
}

fn default_timeout() -> u64 {
    DEFAULT_RUN_TIMEOUT.as_secs()
}

/// Шаблон целиком: манифест + содержимое файлов по путям внутри шаблона.
#[derive(Debug, Clone)]
pub struct Template {
    /// Разобранный манифест.
    pub manifest: TemplateManifest,
    /// Файлы шаблона: путь внутри каталога шаблона → содержимое.
    pub files: BTreeMap<String, String>,
}

impl Template {
    /// Содержимое файла шаблона.
    #[must_use]
    pub fn file(&self, rel: &str) -> Option<&str> {
        self.files.get(rel).map(String::as_str)
    }

    /// Файлы, которые нужно положить в кейс для выбранного языка.
    #[must_use]
    pub fn files_for(&self, lang: Lang) -> Vec<&FileSpec> {
        self.manifest
            .files
            .iter()
            .filter(|f| lang.covers(&f.lang))
            .collect()
    }

    /// Подмены нарушающей реализацией для выбранного языка.
    #[must_use]
    pub fn violating_for(&self, lang: Lang) -> Vec<&ViolatingSwap> {
        self.manifest
            .violating
            .iter()
            .filter(|v| lang.covers(&v.lang))
            .collect()
    }

    /// Команда прогона тестов для языка.
    #[must_use]
    pub fn command_for(&self, lang: &str) -> Option<&str> {
        self.manifest.rule.commands.for_lang(lang)
    }
}

/// Язык поставки: какие половины шаблона применяются и проверяются.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    /// Только python + pytest.
    Python,
    /// Только java + `JUnit` 5.
    Java,
    /// Обе половины.
    Both,
}

impl Lang {
    /// Разбирает значение CLI-флага.
    ///
    /// # Errors
    /// Неизвестный язык — доменная ошибка с перечнем допустимых.
    pub fn parse(raw: &str) -> Result<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "python" | "py" => Ok(Self::Python),
            "java" => Ok(Self::Java),
            "both" | "all" => Ok(Self::Both),
            other => Err(HarnessError::Control(format!(
                "неизвестный язык '{other}' (допустимы: python, java, both)"
            ))),
        }
    }

    /// Значение для вывода.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Python => "python",
            Self::Java => "java",
            Self::Both => "both",
        }
    }

    /// Входят ли файлы языка `lang` в поставку этого выбора.
    fn covers(self, lang: &str) -> bool {
        match self {
            Self::Both => true,
            Self::Python => lang == "python" || lang == "common",
            Self::Java => lang == "java" || lang == "common",
        }
    }

    /// Половины, которые проверяются проверкой зубов.
    fn halves(self) -> &'static [&'static str] {
        match self {
            Self::Python => &["python"],
            Self::Java => &["java"],
            Self::Both => &["python", "java"],
        }
    }
}

// ---------------------------------------------------------------------------
// Каталог
// ---------------------------------------------------------------------------

/// Все встроенные шаблоны, отсортированные по id.
///
/// # Errors
/// Встроенный ассет бит: нет `template.yaml`, манифест не парсится или его
/// `id` расходится с именем каталога (дефект сборки — обработка как ошибка,
/// а не молчаливый пропуск).
pub fn templates() -> Result<Vec<Template>> {
    let mut by_id: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
    for (rel, content) in crate::assets::embedded_rule_templates() {
        let rest = rel
            .strip_prefix(&format!("{TEMPLATES_REL}/"))
            .ok_or_else(|| {
                HarnessError::Control(format!("ассет шаблона вне {TEMPLATES_REL}/: {rel}"))
            })?;
        let (id, inner) = rest.split_once('/').ok_or_else(|| {
            HarnessError::Control(format!("ассет шаблона без каталога шаблона: {rel}"))
        })?;
        by_id
            .entry(id.to_string())
            .or_default()
            .insert(inner.to_string(), (*content).to_string());
    }
    let mut out = Vec::new();
    for (id, files) in by_id {
        let yaml = files.get("template.yaml").ok_or_else(|| {
            HarnessError::Control(format!("шаблон '{id}': нет файла template.yaml"))
        })?;
        let manifest: TemplateManifest = serde_yaml_ng::from_str(yaml).map_err(|e| {
            HarnessError::Control(format!("шаблон '{id}': манифест не парсится: {e}"))
        })?;
        if manifest.id != id {
            return Err(HarnessError::Control(format!(
                "шаблон '{id}': в манифесте id '{}' — расходится с именем каталога",
                manifest.id
            )));
        }
        out.push(Template { manifest, files });
    }
    Ok(out)
}

/// Шаблон по id.
///
/// # Errors
/// Встроенные ассеты биты (см. [`templates`]).
pub fn template(id: &str) -> Result<Option<Template>> {
    Ok(templates()?.into_iter().find(|t| t.manifest.id == id))
}

/// Текст для `rules template list`.
///
/// # Errors
/// Встроенные ассеты биты (см. [`templates`]).
pub fn render_list() -> Result<String> {
    let all = templates()?;
    let executable = all.iter().filter(|t| t.manifest.executable).count();
    let mut out = String::new();
    let _ = writeln!(
        out,
        "Шаблонов: {} (исполняемых: {executable}, заготовка: {})",
        all.len(),
        all.len() - executable
    );
    for t in &all {
        let mark = if t.manifest.executable { " " } else { "~" };
        let _ = writeln!(
            out,
            " {mark} {:<26} v{:<2} {}",
            t.manifest.id, t.manifest.version, t.manifest.title
        );
    }
    let _ = writeln!(
        out,
        "\n~/ — заготовка для нераспознанного паттерна (правило печатается закомментированным).\n\
         Применить: arch-be rules template apply <id> --ad AD-N --dir <кейс>"
    );
    Ok(out)
}

/// Текст для `rules template show <id>`.
///
/// # Errors
/// Шаблон не найден или встроенные ассеты биты.
pub fn render_show(id: &str) -> Result<String> {
    let t = template(id)?.ok_or_else(|| unknown_template(id))?;
    let m = &t.manifest;
    let mut out = String::new();
    let _ = writeln!(out, "{} (v{})", m.id, m.version);
    let _ = writeln!(out, "{}\n", m.title);
    let _ = writeln!(out, "Паттерн: {}\n", m.pattern);
    let _ = writeln!(out, "Проверяет свойства:");
    for p in &m.properties {
        let _ = writeln!(out, "  - {p}");
    }
    let _ = writeln!(
        out,
        "\nПравило: {} (type: {}, severity: {}, timeout: {}s)",
        m.rule.name, m.rule.kind, m.rule.severity, m.rule.timeout_secs
    );
    for (lang, cmd) in [
        ("python", &m.rule.commands.python),
        ("java", &m.rule.commands.java),
    ] {
        if let Some(cmd) = cmd {
            let _ = writeln!(out, "  {lang}: {cmd}");
        }
    }
    let mut words: Vec<&String> = m.match_terms.iter().collect();
    words.sort();
    let _ = writeln!(
        out,
        "\nПодбор по словам ({}): {}",
        words.len(),
        words
            .iter()
            .map(|w| w.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    );
    let _ = writeln!(out, "\nФайлы ({}):", m.files.len());
    for f in &m.files {
        let _ = writeln!(
            out,
            "  [{}] {} → {TARGET_REL}/{}/{}",
            f.lang, f.from, m.id, f.to
        );
    }
    if !m.violating.is_empty() {
        let _ = writeln!(out, "\nПроверка зубов подменяет:");
        for v in &m.violating {
            let _ = writeln!(
                out,
                "  [{}] {TARGET_REL}/{}/{} ← {}",
                v.lang, m.id, v.to, v.from
            );
        }
    }
    if !m.executable {
        let _ = writeln!(
            out,
            "\nЗаготовка: тест нужно сформулировать самому — исполняемой проверки \
             в шаблоне нет, правило печатается закомментированным."
        );
    }
    Ok(out)
}

/// Ошибка «шаблона нет» с перечнем известных id.
fn unknown_template(id: &str) -> HarnessError {
    let known = templates().map_or_else(
        |_| "каталог недоступен".to_string(),
        |all| {
            all.iter()
                .map(|t| t.manifest.id.clone())
                .collect::<Vec<_>>()
                .join(", ")
        },
    );
    HarnessError::Control(format!("шаблон '{id}' не найден (известны: {known})"))
}

// ---------------------------------------------------------------------------
// Подбор шаблона по тексту инварианта
// ---------------------------------------------------------------------------

/// Результат подбора: шаблон, счёт совпадений и совпавшие слова.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct TemplateMatch {
    /// Id шаблона.
    pub id: String,
    /// Число совпавших слов словаря.
    pub score: usize,
    /// Совпавшие слова (для объяснения выбора).
    pub matched: Vec<String>,
}

/// Подбирает до `limit` шаблонов по тексту инварианта: счёт — число
/// совпавших слов словаря `match` (подстрока по нормализованному тексту).
/// Порядок стабилен: счёт по убыванию, затем id по возрастанию.
///
/// # Errors
/// Встроенные ассеты биты (см. [`templates`]).
pub fn match_templates(text: &str, limit: usize) -> Result<Vec<TemplateMatch>> {
    let hay = normalize(text);
    let mut scored: Vec<TemplateMatch> = Vec::new();
    for t in templates()? {
        if !t.manifest.executable {
            continue;
        }
        let mut matched: Vec<String> = Vec::new();
        for term in t.manifest.match_terms.iter() {
            let needle = normalize(term);
            if !needle.is_empty() && hay.contains(&needle) {
                matched.push(term.clone());
            }
        }
        if !matched.is_empty() {
            scored.push(TemplateMatch {
                id: t.manifest.id,
                score: matched.len(),
                matched,
            });
        }
    }
    scored.sort_by(|a, b| b.score.cmp(&a.score).then_with(|| a.id.cmp(&b.id)));
    scored.truncate(limit);
    Ok(scored)
}

/// Нормализация текста для подбора: нижний регистр и `ё` → `е`.
fn normalize(text: &str) -> String {
    text.to_lowercase().replace('ё', "е")
}

// ---------------------------------------------------------------------------
// Фрагмент правила
// ---------------------------------------------------------------------------

/// Скаляр YAML в одинарных кавычках: внутри них нет escape-последовательностей,
/// поэтому regex и пути доезжают до парсера литерально (тот же приём, что в
/// `rules_suggest`: в двойных кавычках `\s` — недопустимая последовательность).
fn yaml_sq(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

/// Фрагмент правила для `CONSTRAINTS.yaml` под ключом `rules:` со свободным
/// `C-NNN` и подставленным `ad`. Для заготовки (`executable: false`) фрагмент
/// печатается закомментированным — механики за ним нет, и делать вид, что она
/// есть, нельзя (П2).
#[must_use]
pub fn rule_fragment(t: &Template, ad: &str, free_id: &str) -> String {
    let m = &t.manifest;
    let command = m.rule.commands.python.as_deref().unwrap_or("");
    let mut body = String::new();
    let _ = writeln!(body, "  - id: {free_id}");
    let _ = writeln!(body, "    name: {}", m.rule.name);
    let _ = writeln!(body, "    type: {}", m.rule.kind);
    let _ = writeln!(body, "    command: {}", yaml_sq(command));
    let _ = writeln!(body, "    timeout_secs: {}", m.rule.timeout_secs);
    let _ = writeln!(body, "    severity: {}", m.rule.severity);
    let _ = writeln!(body, "    ad: {ad}");
    let _ = writeln!(body, "    rationale: {}", yaml_sq(&m.rule.rationale));
    let _ = writeln!(body, "    fix_hint: {}", yaml_sq(&m.rule.fix_hint));
    if let Some(skill) = &m.rule.skill {
        let _ = writeln!(body, "    skill: {skill}");
    }
    if m.executable {
        return body;
    }
    let mut out = String::from(
        "  # Заготовка: исполняемой проверки в шаблоне НЕТ — правило ниже\n\
         \x20 # закомментировано намеренно. Сформулируйте свойство инварианта,\n\
         \x20 # напишите тест и только после этого включайте правило.\n",
    );
    for line in body.lines() {
        let _ = writeln!(out, "  # {line}");
    }
    out
}

/// Фрагмент правила для кандидата детектора: свободный `C-NNN` берётся из
/// реестра кейса, `ad` подставляется из инварианта.
///
/// # Errors
/// Шаблона нет в этой сборке.
pub fn candidate_fragment(case: &Path, template_id: &str, ad: &str) -> Result<String> {
    let t = template(template_id)?.ok_or_else(|| unknown_template(template_id))?;
    Ok(rule_fragment(&t, ad, &free_rule_id(case)))
}

/// Первый свободный `C-NNN` в реестре кейса (для печати фрагмента).
/// Реестра нет — `C-100`.
fn free_rule_id(case: &Path) -> String {
    let mut max: u32 = 99;
    if let Some(resolved) = crate::control::resolve_constraints_path_detailed(case, None) {
        if let Ok(cards) = crate::control::rule_cards(&resolved.path) {
            for card in &cards {
                for raw in card.id.iter().chain(std::iter::once(&card.name)) {
                    if let Some(n) = c_number(raw) {
                        max = max.max(n);
                    }
                }
            }
        }
    }
    format!("C-{:03}", max + 1)
}

/// Номер правила из ссылки `C-<n>` (иначе `None`).
fn c_number(raw: &str) -> Option<u32> {
    let digits = raw.strip_prefix("C-")?;
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

/// Ссылки на правила эквивалентны: `C-2` и `C-002` — одно правило.
#[must_use]
pub fn rule_ref_eq(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    match (c_number(a), c_number(b)) {
        (Some(x), Some(y)) => x == y,
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// Спайн: номер инварианта и его текст
// ---------------------------------------------------------------------------

/// Номер инварианта из ссылки `AD-<n>`: `AD-1`, `AD-001` — оба дают `1`.
fn ad_number(raw: &str) -> Option<u32> {
    let digits = raw.trim().strip_prefix(AD_PREFIX)?;
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

/// Номера инвариантов, объявленных в `ARCHITECTURE-SPINE.md` (`## AD-N. …`).
/// Файла нет — пустой список.
fn spine_ad_numbers(case: &Path) -> Vec<u32> {
    let Ok(text) = std::fs::read_to_string(case.join("ARCHITECTURE-SPINE.md")) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for line in text.lines() {
        let Some(rest) = line.trim_start().strip_prefix("## ") else {
            continue;
        };
        let Some((head, _)) = rest.split_once('.') else {
            continue;
        };
        if let Some(n) = ad_number(head.trim()) {
            out.push(n);
        }
    }
    out
}

/// Текст инварианта в спайне: заголовок и строки `Binds`/`Prevents`/`Rule`
/// до следующего заголовка `## `. Файла или блока нет — `None`.
#[must_use]
pub fn spine_ad_text(case: &Path, ad: &str) -> Option<String> {
    let want = ad_number(ad)?;
    let text = std::fs::read_to_string(case.join("ARCHITECTURE-SPINE.md")).ok()?;
    let mut out = String::new();
    let mut inside = false;
    for line in text.lines() {
        if let Some(rest) = line.trim_start().strip_prefix("## ") {
            if inside {
                break;
            }
            if let Some((head, _)) = rest.split_once('.') {
                inside = ad_number(head.trim()) == Some(want);
            }
            if inside {
                out.push_str(line);
                out.push('\n');
            }
            continue;
        }
        if inside {
            out.push_str(line);
            out.push('\n');
        }
    }
    if out.trim().is_empty() {
        None
    } else {
        Some(out)
    }
}

// ---------------------------------------------------------------------------
// Покрытие инвариантов исполняемыми правилами (общее для детектора, паспорта,
// трассировки и доверия)
// ---------------------------------------------------------------------------

/// Что известно про инвариант и его правила — для отчётов и детектора.
#[derive(Debug, Clone, Serialize)]
pub struct AdRuleKind {
    /// Ссылка на правило (`C-2`) или имя сущности из `verified_by`.
    pub reference: String,
    /// Тип правила, если оно нашлось в реестре (`snake_case`).
    pub kind: Option<String>,
}

/// Инвариант и его покрытие поведением.
#[derive(Debug, Clone, Serialize)]
pub struct AdCoverageEntry {
    /// Id сущности (`AD-003`).
    pub ad: String,
    /// Заголовок инварианта (из модели).
    pub title: String,
    /// Несущий инвариант (`load_bearing: true`).
    pub load_bearing: bool,
    /// Обоснованный отказ от механической проверки ([`crate::model::Entity`]).
    pub unverifiable: bool,
    /// Правила из `verified_by` (и из поля `ad:` реестра) с их типами.
    pub rules: Vec<AdRuleKind>,
    /// Хотя бы одно правило проверяет поведение.
    pub covered: bool,
    /// Заголовок и `Binds`/`Prevents`/`Rule` из спайна (для подбора шаблона).
    pub spine_text: String,
}

/// Покрытие всех инвариантов кейса.
#[derive(Debug, Clone, Serialize)]
pub struct AdCoverage {
    /// Все инварианты модели в порядке файлов.
    pub entries: Vec<AdCoverageEntry>,
    /// Признак `load_bearing` задан хотя бы у одного инварианта.
    pub load_bearing_defined: bool,
}

impl AdCoverage {
    /// Всего инвариантов.
    #[must_use]
    pub fn total(&self) -> usize {
        self.entries.len()
    }

    /// Инварианты с исполняемой проверкой поведения.
    #[must_use]
    pub fn covered(&self) -> Vec<&AdCoverageEntry> {
        self.entries.iter().filter(|e| e.covered).collect()
    }

    /// Инварианты без проверки поведения, несущие первыми, затем по id.
    #[must_use]
    pub fn uncovered(&self) -> Vec<&AdCoverageEntry> {
        let mut out: Vec<&AdCoverageEntry> = self.entries.iter().filter(|e| !e.covered).collect();
        out.sort_by(|a, b| {
            b.load_bearing
                .cmp(&a.load_bearing)
                .then_with(|| a.ad.cmp(&b.ad))
        });
        out
    }

    /// Несущие инварианты, у которых нет проверки поведения.
    #[must_use]
    pub fn load_bearing_uncovered(&self) -> Vec<&AdCoverageEntry> {
        self.uncovered()
            .into_iter()
            .filter(|e| e.load_bearing)
            .collect()
    }
}

/// Собирает покрытие инвариантов кейса: правила из `verified_by` каждой
/// сущности `type: ad` плюс правила реестра с полем `ad:`, типы — из реестра.
///
/// `Ok(None)` — анализа нет: каталога `model/` в кейсе нет ИЛИ модель не
/// читается ни одной сущностью. Второй случай — осознанная терпимость: это
/// представление (детектор-подсказка, строка паспорта, деталь доверия), и
/// ломать из-за него весь отчёт нельзя — о нечитаемой модели скажет
/// `trace_check`, у которого разбор модели предмет самой проверки.
///
/// # Errors
/// Реестр правил есть, но не читается.
pub fn ad_coverage(case: &Path) -> Result<Option<AdCoverage>> {
    let model_dir = case.join("model");
    if !model_dir.is_dir() {
        return Ok(None);
    }
    let Ok(model) = crate::model::load_model_tolerant(&model_dir) else {
        return Ok(None);
    };
    let cards = match crate::control::resolve_constraints_path_detailed(case, None) {
        Some(resolved) => crate::control::rule_cards(&resolved.path)?,
        None => Vec::new(),
    };
    let mut entries = Vec::new();
    let mut load_bearing_defined = false;
    for e in model
        .entities
        .iter()
        .filter(|e| e.kind == crate::model::EntityKind::Ad)
    {
        if e.load_bearing {
            load_bearing_defined = true;
        }
        let mut rules: Vec<AdRuleKind> = Vec::new();
        for raw in &e.verified_by {
            let card = cards.iter().find(|c| {
                rule_ref_eq(c.key(), raw)
                    || rule_ref_eq(&c.name, raw)
                    || c.id.as_deref().is_some_and(|id| rule_ref_eq(id, raw))
            });
            rules.push(AdRuleKind {
                reference: raw.clone(),
                kind: card.map(|c| c.kind.to_string()),
            });
        }
        // Обратная связь реестра: правило само называет задетый инвариант.
        for card in &cards {
            let Some(ad) = card.ad.as_deref() else {
                continue;
            };
            if ad_number(ad) != ad_number(&e.id) {
                continue;
            }
            let key = card.key();
            if rules.iter().any(|r| rule_ref_eq(&r.reference, key)) {
                continue;
            }
            rules.push(AdRuleKind {
                reference: key.to_string(),
                kind: Some(card.kind.to_string()),
            });
        }
        let covered = rules.iter().any(|r| {
            r.kind
                .as_deref()
                .is_some_and(|k| crate::control::BEHAVIOUR_RULE_KINDS.contains(&k))
        });
        entries.push(AdCoverageEntry {
            ad: e.id.clone(),
            title: e.title.clone(),
            load_bearing: e.load_bearing,
            unverifiable: e
                .unverifiable
                .as_deref()
                .is_some_and(|u| !u.trim().is_empty()),
            rules,
            covered,
            spine_text: spine_ad_text(case, &e.id).unwrap_or_else(|| {
                format!("## {} {} \n{}", e.id, e.title, e.body)
                    .trim()
                    .to_string()
            }),
        });
    }
    Ok(Some(AdCoverage {
        entries,
        load_bearing_defined,
    }))
}

// ---------------------------------------------------------------------------
// apply
// ---------------------------------------------------------------------------

/// Итог `rules template apply`.
#[derive(Debug, Clone, Serialize)]
pub struct ApplyReport {
    /// Id шаблона.
    pub template: String,
    /// Версия шаблона.
    pub version: u32,
    /// Инвариант, к которому привязан шаблон.
    pub ad: String,
    /// Язык поставки.
    pub lang: String,
    /// Каталог кейса, куда положены файлы.
    pub target_dir: PathBuf,
    /// Что записано (или было бы записано при `--dry-run`).
    pub written: Vec<PathBuf>,
    /// Файл блокировки.
    pub lock: PathBuf,
    /// Фрагмент правила для `CONSTRAINTS.yaml` (печатается, не вносится).
    pub fragment: String,
    /// Строка `verified_by` для сущности инварианта.
    pub verified_by: String,
    /// Замечания: занятое имя правила, повторное применение, адаптация.
    pub notes: Vec<String>,
    /// Прогон без записи на диск.
    pub dry_run: bool,
}

/// Проверяет `--ad`: форма `AD-<n>` и наличие инварианта в спайне.
///
/// # Errors
/// Форма не та, спайна нет или инварианта в нём нет.
fn validate_ad(case: &Path, ad: &str) -> Result<()> {
    let Some(n) = ad_number(ad) else {
        return Err(HarnessError::Control(format!(
            "--ad: ожидается ссылка вида AD-<номер>, получено '{ad}'"
        )));
    };
    let declared = spine_ad_numbers(case);
    if declared.is_empty() {
        return Err(HarnessError::Control(format!(
            "{}: нет объявленных инвариантов (`## AD-N. …`) — привязывать шаблон не к чему",
            case.join("ARCHITECTURE-SPINE.md").display()
        )));
    }
    if !declared.contains(&n) {
        let known = declared
            .iter()
            .map(|n| format!("AD-{n}"))
            .collect::<Vec<_>>()
            .join(", ");
        return Err(HarnessError::Control(format!(
            "инвариант '{ad}' не найден в спайне кейса (объявлены: {known})"
        )));
    }
    Ok(())
}

/// Кладёт файлы шаблона в кейс, пишет lock-файл и **печатает** фрагмент
/// правила со строкой `verified_by`. Реестр правил и спайн не правит (П3):
/// их изменение — дельта.
///
/// # Errors
/// Шаблон не найден; `--ad` не проходит проверку; файлы назначения уже
/// существуют (перечень конфликтов); файлы/lock не пишутся.
pub fn apply(case: &Path, id: &str, ad: &str, lang: Lang, dry_run: bool) -> Result<ApplyReport> {
    let t = template(id)?.ok_or_else(|| unknown_template(id))?;
    if !case.is_dir() {
        return Err(HarnessError::Control(format!(
            "кейс недоступен (не каталог): {}",
            case.display()
        )));
    }
    validate_ad(case, ad)?;
    let target_dir = case.join(TARGET_REL).join(id);
    let to_write: Vec<&FileSpec> = t.files_for(lang);
    if to_write.is_empty() {
        return Err(HarnessError::Control(format!(
            "шаблон '{id}' не поставляет файлов для языка {}",
            lang.as_str()
        )));
    }
    let mut conflicts = Vec::new();
    for f in &to_write {
        let dest = target_dir.join(&f.to);
        if dest.exists() {
            conflicts.push(dest);
        }
    }
    if !conflicts.is_empty() {
        let list = conflicts
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join("\n  ");
        return Err(HarnessError::Control(format!(
            "файлы уже существуют — ничего не перезаписываю (уберите их или примените \
             шаблон вручную):\n  {list}"
        )));
    }
    let free_id = free_rule_id(case);
    let fragment = rule_fragment(&t, ad, &free_id);
    let verified_by = format!("verified_by: [{free_id}]");
    let mut notes = Vec::new();
    // Что и куда кладём: план строится до записи, поэтому `--dry-run` даёт тот
    // же lock-отпечаток (хэши считаются по содержимому, а не по файлам на диске).
    let mut planned: Vec<(PathBuf, &str)> = Vec::new();
    for f in &to_write {
        let Some(content) = t.file(&f.from) else {
            return Err(HarnessError::Control(format!(
                "шаблон '{id}': в манифесте объявлен файл '{}', которого нет",
                f.from
            )));
        };
        planned.push((target_dir.join(&f.to), content));
    }
    let mut written = Vec::new();
    if !dry_run {
        for (dest, content) in &planned {
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent).map_err(|e| HarnessError::io(parent, e))?;
            }
            std::fs::write(dest, content).map_err(|e| HarnessError::io(dest, e))?;
        }
    }
    written.extend(planned.iter().map(|(dest, _)| dest.clone()));
    let lock = case.join(LOCK_REL);
    let mut lock_entries = read_lock(&lock)?;
    if lock_entries
        .iter()
        .any(|e| e.id == t.manifest.id && rule_ref_eq(&e.ad, ad))
    {
        notes.push(format!(
            "шаблон '{}' уже применён к {ad} — запись в lock обновлена",
            t.manifest.id
        ));
    }
    lock_entries.retain(|e| !(e.id == t.manifest.id && rule_ref_eq(&e.ad, ad)));
    lock_entries.push(lock_entry(case, &t, ad, lang, &planned));
    lock_entries.sort_by(|a, b| a.id.cmp(&b.id).then_with(|| a.ad.cmp(&b.ad)));
    if !dry_run {
        write_lock(&lock, &lock_entries)?;
    }
    // Занятое имя правила — предупреждение, а не отказ: фрагмент только печатается.
    if let Some(resolved) = crate::control::resolve_constraints_path_detailed(case, None) {
        if let Ok(cards) = crate::control::rule_cards(&resolved.path) {
            if let Some(card) = cards.iter().find(|c| c.name == t.manifest.rule.name) {
                notes.push(format!(
                    "имя правила '{}' уже занято ({}) — переименуйте перед вставкой фрагмента",
                    t.manifest.rule.name,
                    card.id.as_deref().unwrap_or(&card.name)
                ));
            }
            if !cards.iter().any(|c| c.name == t.manifest.rule.name) {
                notes.push(format!(
                    "свободный id правила: {free_id} (взято из реестра кейса)"
                ));
            }
        }
    }
    if !t.manifest.executable {
        notes.push(
            "шаблон-заготовка: исполняемой проверки нет, тест нужно написать самому".to_string(),
        );
    }
    notes.push(
        "фрагмент правила и строку verified_by вносит архитектор дельтой \
         (`arch-be delta new <name>`): реестр и спайн — защищённые файлы"
            .to_string(),
    );
    Ok(ApplyReport {
        template: t.manifest.id,
        version: t.manifest.version,
        ad: ad.to_string(),
        lang: lang.as_str().to_string(),
        target_dir,
        written,
        lock,
        fragment,
        verified_by,
        notes,
        dry_run,
    })
}

/// Запись lock-файла: что применено, куда и с какими хэшами.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LockEntry {
    /// Id шаблона.
    pub id: String,
    /// Версия шаблона на момент применения.
    pub version: u32,
    /// Инвариант (`AD-3`).
    pub ad: String,
    /// Язык поставки.
    pub lang: String,
    /// Каталог внутри кейса.
    pub dir: String,
    /// Команда правила, которая запускает тесты шаблона.
    pub command: String,
    /// Файлы с хэшами: подмена или адаптация видны по расхождению.
    pub files: Vec<LockFile>,
}

/// Файл в lock-записи.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LockFile {
    /// Путь относительно корня кейса.
    pub path: String,
    /// SHA-256 содержимого на момент применения.
    pub sha256: String,
}

#[derive(Debug, Deserialize)]
struct Lock {
    #[serde(default)]
    templates: Vec<LockEntry>,
}

fn lock_entry(
    case: &Path,
    t: &Template,
    ad: &str,
    lang: Lang,
    planned: &[(PathBuf, &str)],
) -> LockEntry {
    let mut files = Vec::new();
    for (path, content) in planned {
        let rel = path
            .strip_prefix(case)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");
        files.push(LockFile {
            path: rel,
            sha256: crate::hash::sha256_hex(content.as_bytes()),
        });
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    let python = t.command_for("python").map(str::to_string).unwrap_or_default();
    LockEntry {
        id: t.manifest.id.clone(),
        version: t.manifest.version,
        ad: ad.to_string(),
        lang: lang.as_str().to_string(),
        dir: format!("{TARGET_REL}/{}", t.manifest.id),
        command: python,
        files,
    }
}

fn read_lock(path: &Path) -> Result<Vec<LockEntry>> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Ok(Vec::new());
    };
    let lock: Lock = serde_yaml_ng::from_str(&text).map_err(|e| {
        HarnessError::Control(format!("{}: lock-файл не парсится: {e}", path.display()))
    })?;
    Ok(lock.templates)
}

fn write_lock(path: &Path, entries: &[LockEntry]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| HarnessError::io(parent, e))?;
    }
    let mut out = String::from(
        "# Применённые шаблоны исполняемых правил — сгенерировано `arch-be rules template apply`.\n\
         # Не редактируйте вручную: проверка зубов сверяет хэши файлов с этой записью.\n\
         templates:\n",
    );
    for e in entries {
        let _ = writeln!(out, "  - id: {}", e.id);
        let _ = writeln!(out, "    version: {}", e.version);
        let _ = writeln!(out, "    ad: {}", e.ad);
        let _ = writeln!(out, "    lang: {}", e.lang);
        let _ = writeln!(out, "    dir: {}", e.dir);
        let _ = writeln!(out, "    command: {}", yaml_sq(&e.command));
        let _ = writeln!(out, "    files:");
        for f in &e.files {
            let _ = writeln!(out, "      - path: {}", f.path);
            let _ = writeln!(out, "        sha256: {}", yaml_sq(&f.sha256));
        }
    }
    std::fs::write(path, out).map_err(|e| HarnessError::io(path, e))
}

// ---------------------------------------------------------------------------
// Проверка зубов (П2)
// ---------------------------------------------------------------------------

/// Кто исполняет тесты: чем богато окружение.
#[derive(Debug, Clone)]
pub struct Runner {
    /// `python3` из PATH.
    pub python: Option<PathBuf>,
    /// `mvn` из PATH.
    pub maven: Option<PathBuf>,
    /// JUnit-консоль, переданная флагом `--java-jar`.
    pub java_jar: Option<PathBuf>,
}

impl Runner {
    /// Определяет доступные прогонщики: `python3`/`mvn` ищутся в `PATH`,
    /// JUnit-консоль — только явным флагом (встраивать бинарный jar в ассеты
    /// нельзя, а тянуть его из сети харнесс не имеет права).
    #[must_use]
    pub fn detect(java_jar: Option<&Path>) -> Self {
        Self {
            python: which("python3"),
            maven: which("mvn"),
            java_jar: java_jar.map(Path::to_path_buf),
        }
    }
}

/// Поиск программы в `PATH`.
fn which(prog: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(prog))
        .find(|candidate| candidate.is_file())
}

/// Одна проверка зубов: шаблон, язык, ожидание и исход.
#[derive(Debug, Clone, Serialize)]
pub struct VerifyCheck {
    /// Id шаблона.
    pub template: String,
    /// Язык половины.
    pub lang: String,
    /// Что проверялось: `reference` (ожидание PASS) или `violating` (FAIL).
    pub stage: &'static str,
    /// Ожидание совпало с исходом.
    pub ok: bool,
    /// Подробность: код возврата и хвост вывода.
    pub detail: String,
}

/// Итог проверки зубов.
#[derive(Debug, Clone, Serialize)]
pub struct VerifyReport {
    /// Проверки в порядке обхода.
    pub checks: Vec<VerifyCheck>,
    /// Пропуски с причиной (нет прогонщика, шаблон адаптирован, заготовка).
    pub skipped: Vec<String>,
    /// Находки: тест остался зелёным на нарушающей реализации.
    pub findings: Vec<String>,
}

impl VerifyReport {
    /// Все проверки прошли и находок нет.
    #[must_use]
    pub fn passed(&self) -> bool {
        self.findings.is_empty() && self.checks.iter().all(|c| c.ok)
    }
}

/// Прогоняет `bash -c <command>` в каталоге `dir` с таймаутом
/// (spawn + опрос `try_wait` + `kill` по истечении; читатели вывода — в
/// отдельных потоках, иначе полный pipe блокирует дочерний процесс).
/// Идиома та же, что у `command_succeeds` в `control`.
fn run_shell(dir: &Path, command: &str, timeout: Duration) -> Result<(Option<i32>, String)> {
    let mut child = Command::new("bash")
        .arg("-c")
        .arg(command)
        .current_dir(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| HarnessError::Control(format!("не удалось запустить bash: {e}")))?;
    let out_task = child
        .stdout
        .take()
        .map(|p| std::thread::spawn(move || drain(p)));
    let err_task = child
        .stderr
        .take()
        .map(|p| std::thread::spawn(move || drain(p)));
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {
                if start.elapsed() >= timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    break None;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(HarnessError::Control(format!(
                    "ошибка ожидания команды '{command}': {e}"
                )));
            }
        }
    };
    let mut captured = Vec::new();
    for task in [out_task, err_task].into_iter().flatten() {
        if let Ok(mut bytes) = task.join() {
            captured.append(&mut bytes);
        }
    }
    let tail = String::from_utf8_lossy(&captured).into_owned();
    let code = status.map(|s| s.code().unwrap_or(-1));
    Ok((code, tail))
}

/// Читает поток целиком (короткий хвост держим в памяти как есть).
fn drain(pipe: impl std::io::Read) -> Vec<u8> {
    let mut buf = Vec::new();
    let mut reader = std::io::BufReader::new(pipe);
    let _ = std::io::Read::read_to_end(&mut reader, &mut buf);
    buf
}

/// Скаляр в одинарных кавычках для `bash`: кавычка внутри — `'\''`
/// (в отличие от YAML, где она удваивается).
fn shell_sq(value: &str) -> String {
    format!("'{}'", value.replace('\'', r"'\''"))
}

/// Команда прогона java-половины через JUnit-консоль (когда `mvn` недоступен
/// или передан `--java-jar`): сборка `javac` + прогон тестов сканированием
/// classpath. Каталог задаётся `cd` перед командой.
fn java_jar_command(jar: &Path) -> String {
    let jar = shell_sq(&jar.display().to_string());
    format!(
        "javac -encoding UTF-8 -cp {jar} -d out src/main/java/*.java src/test/java/*.java && \
         java -jar {jar} execute -cp out --scan-classpath --details=summary"
    )
}

/// Проверяет каждый исполняемый шаблон на эталонной (ожидание PASS) и
/// нарушающей (ожидание FAIL) реализации.
///
/// # Errors
/// Встроенные ассеты биты; временный каталог не создаётся.
pub fn verify_all(runner: &Runner, lang: Lang, require_python: bool) -> Result<VerifyReport> {
    let mut report = VerifyReport {
        checks: Vec::new(),
        skipped: Vec::new(),
        findings: Vec::new(),
    };
    for t in templates()? {
        if !t.manifest.executable {
            report.skipped.push(format!(
                "{}: шаблон-заготовка — исполняемой проверки нет",
                t.manifest.id
            ));
            continue;
        }
        let root = temp_root(&t.manifest.id);
        for half in lang.halves() {
            let files: Vec<&FileSpec> = t
                .files_for(lang)
                .into_iter()
                .filter(|f| f.lang == *half || f.lang == "common")
                .collect();
            if !files.iter().any(|f| f.lang == *half) {
                continue;
            }
            write_files(&root, TARGET_REL, &t.manifest.id, &files, &t)?;
            let Some(command) = command_for(&t, half, &root, None, runner) else {
                report.skipped.push(format!(
                    "{} [{half}]: нет прогонщика — проверка не выполнена",
                    t.manifest.id
                ));
                if *half == "python" && require_python {
                    report.checks.push(VerifyCheck {
                        template: t.manifest.id.clone(),
                        lang: (*half).to_string(),
                        stage: "reference",
                        ok: false,
                        detail: "python3 не найден в PATH, а без него проверка зубов невозможна"
                            .to_string(),
                    });
                }
                continue;
            };
            let timeout = if *half == "java" {
                JAVA_RUN_TIMEOUT
            } else {
                Duration::from_secs(t.manifest.rule.timeout_secs.max(1))
            };
            run_stage(&mut report, &t, half, Stage::Reference, &command, &root, timeout)?;
            // Нарушающая реализация: подменяем объявленные файлы (П2).
            let swaps = t.violating_for(lang);
            let swaps: Vec<&ViolatingSwap> =
                swaps.into_iter().filter(|v| v.lang == *half).collect();
            if swaps.is_empty() {
                report.skipped.push(format!(
                    "{} [{half}]: нарушающая реализация не объявлена — зубы не проверены",
                    t.manifest.id
                ));
                continue;
            }
            apply_swaps(&root, TARGET_REL, &t.manifest.id, &swaps, &t)?;
            run_stage(&mut report, &t, half, Stage::Violating, &command, &root, timeout)?;
        }
        let _ = std::fs::remove_dir_all(&root);
    }
    Ok(report)
}

/// Команда прогона половины: python — из манифеста (или записанная в lock),
/// java — JUnit-консолью (если передан jar), иначе Maven’ом из манифеста.
/// `None` — прогонщика в окружении нет.
fn command_for(
    t: &Template,
    half: &str,
    base: &Path,
    python_command: Option<&str>,
    runner: &Runner,
) -> Option<String> {
    if half == "java" {
        if runner.maven.is_none() {
            let jar = runner.java_jar.as_ref()?;
            let dir = base.join(TARGET_REL).join(&t.manifest.id).join("java");
            return Some(format!(
                "cd {} && {}",
                shell_sq(&dir.display().to_string()),
                java_jar_command(jar)
            ));
        }
        return t.command_for("java").map(str::to_string);
    }
    runner.python.as_ref()?;
    python_command
        .map(str::to_string)
        .or_else(|| t.command_for("python").map(str::to_string))
}

/// Стадия проверки зубов: эталон (ожидание PASS) или нарушение (ожидание FAIL).
#[derive(Debug, Clone, Copy)]
enum Stage {
    /// Эталонная реализация: тест обязан быть зелёным.
    Reference,
    /// Нарушающая реализация: тест обязан упасть.
    Violating,
}

impl Stage {
    /// Подпись стадии для отчёта.
    fn label(self) -> &'static str {
        match self {
            Self::Reference => "reference",
            Self::Violating => "violating",
        }
    }

    /// Исход, который считается успехом стадии.
    fn expect_pass(self) -> bool {
        matches!(self, Self::Reference)
    }
}

/// Прогоняет одну стадию (эталонную или нарушающую) и записывает проверку.
fn run_stage(
    report: &mut VerifyReport,
    t: &Template,
    half: &str,
    stage: Stage,
    command: &str,
    root: &Path,
    timeout: Duration,
) -> Result<()> {
    let (code, tail) = run_shell(root, command, timeout)?;
    let expect_pass = stage.expect_pass();
    let ok = match code {
        Some(0) => expect_pass,
        Some(_) => !expect_pass,
        None => false,
    };
    let detail = match code {
        Some(c) => format!("код {c}: {}", tail_tail(&tail)),
        None => format!("таймаут {}s: {}", timeout.as_secs(), tail_tail(&tail)),
    };
    report.checks.push(VerifyCheck {
        template: t.manifest.id.clone(),
        lang: half.to_string(),
        stage: stage.label(),
        ok,
        detail,
    });
    if !ok && matches!(stage, Stage::Violating) && code == Some(0) {
        report.findings.push(format!(
            "{} [{half}]: тест остался зелёным на нарушающей реализации — правило беззубое \
             (`executable_rule_toothless`)",
            t.manifest.id
        ));
    }
    Ok(())
}

/// Последние непустые строки вывода (для detail-строки отчёта).
fn tail_tail(text: &str) -> String {
    // Баннер JUnit-консоли в хвост не берём: он одинаков на успехе и провале
    // и вытесняет единственную полезную строку (число пройденных тестов).
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.contains("Thanks for using JUnit"))
        .collect();
    let tail = lines
        .iter()
        .rev()
        .take(3)
        .rev()
        .copied()
        .collect::<Vec<_>>()
        .join(" | ");
    if tail.chars().count() > 240 {
        tail.chars().take(240).collect::<String>() + "…"
    } else {
        tail
    }
}

/// Временный каталог прогона.
fn temp_root(id: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let root = std::env::temp_dir().join(format!("arch-be-rt-{}-{id}-{nanos}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    root
}

/// Раскладывает файлы шаблона в `<root>/<TARGET_REL>/<id>/…`.
fn write_files(
    root: &Path,
    target_rel: &str,
    id: &str,
    files: &[&FileSpec],
    t: &Template,
) -> Result<()> {
    for f in files {
        let Some(content) = t.file(&f.from) else {
            return Err(HarnessError::Control(format!(
                "шаблон '{id}': в манифесте объявлен файл '{}', которого нет",
                f.from
            )));
        };
        let dest = root.join(target_rel).join(id).join(&f.to);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(|e| HarnessError::io(parent, e))?;
        }
        std::fs::write(&dest, content).map_err(|e| HarnessError::io(&dest, e))?;
    }
    Ok(())
}

/// Подменяет файлы нарушающими реализациями.
fn apply_swaps(
    root: &Path,
    target_rel: &str,
    id: &str,
    swaps: &[&ViolatingSwap],
    t: &Template,
) -> Result<()> {
    for v in swaps {
        let Some(content) = t.file(&v.from) else {
            return Err(HarnessError::Control(format!(
                "шаблон '{id}': объявлена нарушающая реализация '{}', которой нет",
                v.from
            )));
        };
        let dest = root.join(target_rel).join(id).join(&v.to);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(|e| HarnessError::io(parent, e))?;
        }
        std::fs::write(&dest, content).map_err(|e| HarnessError::io(&dest, e))?;
    }
    Ok(())
}

/// Проверка зубов на уровне кейса: по `.arch-handoff/rule-templates.lock`
/// для каждого применённого шаблона во временной копии кейса эталонная
/// реализация подменяется нарушающей и тест прогоняется. Тест остался
/// зелёным — находка `executable_rule_toothless` (в гейт не входит).
///
/// # Errors
/// Lock-файла нет или он битый; временный каталог не создаётся.
pub fn verify_dir(case: &Path, runner: &Runner, lang: Lang) -> Result<VerifyReport> {
    let lock = case.join(LOCK_REL);
    let entries = read_lock(&lock)?;
    let mut report = VerifyReport {
        checks: Vec::new(),
        skipped: Vec::new(),
        findings: Vec::new(),
    };
    if entries.is_empty() {
        return Err(HarnessError::Control(format!(
            "{}: применённых шаблонов нет (`arch-be rules template apply` в этом кейсе не вызывался)",
            lock.display()
        )));
    }
    for entry in entries {
        if !lang.halves().contains(&entry.lang.as_str()) && entry.lang != "both" {
            report.skipped.push(format!(
                "{} ({}, {}) — язык вне фильтра {}",
                entry.id,
                entry.ad,
                entry.lang,
                lang.as_str()
            ));
            continue;
        }
        let Some(t) = template(&entry.id)? else {
            report.skipped.push(format!(
                "{} ({}): шаблона нет в этой сборке — проверка зубов неприменима",
                entry.id, entry.ad
            ));
            continue;
        };
        if t.manifest.version != entry.version {
            report.skipped.push(format!(
                "{} ({}): применена версия {}, в сборке {} — проверка зубов неприменима",
                entry.id, entry.ad, entry.version, t.manifest.version
            ));
            continue;
        }
        if !t.manifest.executable {
            report
                .skipped
                .push(format!("{} ({}): заготовка", entry.id, entry.ad));
            continue;
        }
        // Адаптация: файлы кейса разошлись с lock — нарушающую реализацию
        // подставлять некуда, проверять нечего.
        let mut adapted = Vec::new();
        for f in &entry.files {
            let path = case.join(&f.path);
            match crate::hash::sha256_file(&path) {
                Some(sha) if sha == f.sha256 => {}
                Some(_) => adapted.push(format!("{}: изменён", f.path)),
                None => adapted.push(format!("{}: отсутствует", f.path)),
            }
        }
        if adapted.is_empty() {
            adapted.extend(rule_command_changed(case, &t, &entry));
        }
        if !adapted.is_empty() {
            report.skipped.push(format!(
                "{} ({}): адаптирован — проверка зубов неприменима, проверьте вручную ({})",
                entry.id,
                entry.ad,
                adapted.join("; ")
            ));
            continue;
        }
        let halves: Vec<&str> = if entry.lang == "both" {
            vec!["python", "java"]
        } else {
            vec![entry.lang.as_str()]
        };
        for half in halves {
            let Some(command) = command_for(&t, half, case, Some(&entry.command), runner) else {
                report.skipped.push(format!(
                    "{} ({}) [{half}]: нет прогонщика — проверка не выполнена",
                    entry.id, entry.ad
                ));
                continue;
            };
            let root = case_copy(case, &entry.id, &entry.ad);
            let swaps: Vec<&ViolatingSwap> = t
                .violating_for(Lang::parse(&entry.lang).unwrap_or(Lang::Both))
                .into_iter()
                .filter(|v| v.lang == half)
                .collect();
            apply_swaps(&root, TARGET_REL, &entry.id, &swaps, &t)?;
            let timeout = if half == "java" {
                JAVA_RUN_TIMEOUT
            } else {
                Duration::from_secs(t.manifest.rule.timeout_secs.max(1))
            };
            run_stage(&mut report, &t, half, Stage::Violating, &command, &root, timeout)?;
            let _ = std::fs::remove_dir_all(&root);
        }
    }
    Ok(report)
}

/// Признак «команда правила переключена со скелета на настоящий код»: правило
/// с именем шаблона есть в реестре кейса, но запускает другую команду.
fn rule_command_changed(case: &Path, t: &Template, entry: &LockEntry) -> Vec<String> {
    let Some(resolved) = crate::control::resolve_constraints_path_detailed(case, None) else {
        return vec!["реестр правил не найден".to_string()];
    };
    let Ok(cards) = crate::control::rule_cards(&resolved.path) else {
        return vec!["реестр правил не читается".to_string()];
    };
    let Some(card) = cards.iter().find(|c| c.name == t.manifest.rule.name) else {
        return vec![format!("правила '{}' нет в реестре", t.manifest.rule.name)];
    };
    match card.command.as_deref() {
        Some(cmd) if cmd == entry.command => Vec::new(),
        Some(_) => vec![format!(
            "команда правила '{}' отличается от команды шаблона — проверьте вручную",
            t.manifest.rule.name
        )],
        None => vec![format!("у правила '{}' нет команды", t.manifest.rule.name)],
    }
}

/// Копия кейса во временном каталоге (без `.git`/`target`/`node_modules`).
fn case_copy(case: &Path, id: &str, ad: &str) -> PathBuf {
    let root = temp_root(&format!("{id}-{ad}-dir"));
    let _ = copy_tree(
        case,
        &root,
        &[".git", "target", "node_modules", "__pycache__"],
    );
    root
}

/// Рекурсивное копирование дерева с пропуском служебных каталогов.
fn copy_tree(from: &Path, to: &Path, skip: &[&str]) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if skip.contains(&name.as_ref()) {
            continue;
        }
        let src = entry.path();
        let dst = to.join(name.as_ref());
        if entry.file_type()?.is_dir() {
            copy_tree(&src, &dst, skip)?;
        } else {
            std::fs::copy(&src, &dst)?;
        }
    }
    Ok(())
}

/// Текст отчёта проверки зубов для CLI.
#[must_use]
pub fn render_verify(report: &VerifyReport) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "# Проверка зубов шаблонов\n");
    let _ = writeln!(
        out,
        "Проверок: {} (провалено: {}), пропусков: {}, находок: {}\n",
        report.checks.len(),
        report.checks.iter().filter(|c| !c.ok).count(),
        report.skipped.len(),
        report.findings.len()
    );
    for c in &report.checks {
        let mark = if c.ok { "✓" } else { "✗" };
        let expect = if c.stage == "reference" {
            "ожидание PASS"
        } else {
            "ожидание FAIL"
        };
        let _ = writeln!(
            out,
            "{mark} {} [{}] {} ({expect}): {}",
            c.template, c.lang, c.stage, c.detail
        );
    }
    for s in &report.skipped {
        let _ = writeln!(out, "· пропуск — {s}");
    }
    for f in &report.findings {
        let _ = writeln!(out, "! находка — {f}");
    }
    out
}

// ---------------------------------------------------------------------------
// Инструменты реестра (MCP-мост отдаёт их по белым спискам `mcp_server.rs`:
// list/show — read-only, apply — только под `--rw`)
// ---------------------------------------------------------------------------

/// Инструменты шаблонов исполняемых правил.
#[must_use]
pub fn tools() -> Vec<std::sync::Arc<dyn crate::tool::Tool>> {
    vec![
        std::sync::Arc::new(RuleTemplateListTool),
        std::sync::Arc::new(RuleTemplateShowTool),
        std::sync::Arc::new(RuleTemplateApplyTool),
    ]
}

/// `rule_template_list`: библиотека шаблонов.
struct RuleTemplateListTool;

#[async_trait::async_trait]
impl crate::tool::Tool for RuleTemplateListTool {
    fn spec(&self) -> crate::llm::ToolSpec {
        crate::llm::ToolSpec {
            name: "rule_template_list".into(),
            description: "Список шаблонов исполняемых fitness-правил: id, версия, что проверяет \
                          каждый шаблон. Применение — `rule_template_apply`."
                .into(),
            parameters: serde_json::json!({"type": "object", "properties": {}}),
        }
    }

    async fn call(
        &self,
        _args: serde_json::Value,
        _ctx: &crate::tool::ToolContext,
    ) -> Result<crate::tool::ToolOutput> {
        Ok(crate::tool::ToolOutput::ok(render_list()?))
    }
}

/// `rule_template_show`: один шаблон целиком.
struct RuleTemplateShowTool;

#[async_trait::async_trait]
impl crate::tool::Tool for RuleTemplateShowTool {
    fn spec(&self) -> crate::llm::ToolSpec {
        crate::llm::ToolSpec {
            name: "rule_template_show".into(),
            description: "Шаблон исполняемого правила: проверяемые свойства, словарь подбора, \
                          состав файлов, команды прогона и проверка зубов."
                .into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "id": {"type": "string", "description": "Id шаблона (см. rule_template_list)"}
                },
                "required": ["id"]
            }),
        }
    }

    async fn call(
        &self,
        args: serde_json::Value,
        _ctx: &crate::tool::ToolContext,
    ) -> Result<crate::tool::ToolOutput> {
        let Some(id) = args.get("id").and_then(serde_json::Value::as_str) else {
            return Ok(crate::tool::ToolOutput::err("нужен аргумент 'id'"));
        };
        match render_show(id) {
            Ok(text) => Ok(crate::tool::ToolOutput::ok(text)),
            Err(e) => Ok(crate::tool::ToolOutput::err(e.to_string())),
        }
    }
}

/// `rule_template_apply`: файлы шаблона в кейс + печать фрагмента правила.
struct RuleTemplateApplyTool;

#[async_trait::async_trait]
impl crate::tool::Tool for RuleTemplateApplyTool {
    fn spec(&self) -> crate::llm::ToolSpec {
        crate::llm::ToolSpec {
            name: "rule_template_apply".into(),
            description: "Положить файлы шаблона исполняемого правила в кейс \
                          (`skeleton/rule_templates/<id>/`), записать \
                          `.arch-handoff/rule-templates.lock` и вернуть фрагмент правила для \
                          CONSTRAINTS.yaml со свободным C-NNN и строкой verified_by. Реестр и \
                          спайн не правит: фрагмент вносит архитектор дельтой."
                .into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "id": {"type": "string", "description": "Id шаблона"},
                    "ad": {"type": "string", "description": "Инвариант спайна (AD-3)"},
                    "path": {"type": "string", "description": "Корень кейса (по умолчанию — рабочий каталог)"},
                    "lang": {"type": "string", "description": "python | java | both (по умолчанию python)"},
                    "dry_run": {"type": "boolean", "description": "Показать план, ничего не записывая"}
                },
                "required": ["id", "ad"]
            }),
        }
    }

    async fn call(
        &self,
        args: serde_json::Value,
        ctx: &crate::tool::ToolContext,
    ) -> Result<crate::tool::ToolOutput> {
        let Some(id) = args.get("id").and_then(serde_json::Value::as_str) else {
            return Ok(crate::tool::ToolOutput::err("нужен аргумент 'id'"));
        };
        let Some(ad) = args.get("ad").and_then(serde_json::Value::as_str) else {
            return Ok(crate::tool::ToolOutput::err("нужен аргумент 'ad' (AD-3)"));
        };
        let case = args
            .get("path")
            .and_then(serde_json::Value::as_str)
            .map_or_else(|| ctx.cwd.clone(), |p| ctx.resolve(p));
        let lang = match args.get("lang").and_then(serde_json::Value::as_str) {
            Some(raw) => match Lang::parse(raw) {
                Ok(lang) => lang,
                Err(e) => return Ok(crate::tool::ToolOutput::err(e.to_string())),
            },
            None => Lang::Python,
        };
        let dry_run = args
            .get("dry_run")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        match apply(&case, id, ad, lang, dry_run) {
            Ok(report) => {
                let mut out = String::new();
                let _ = writeln!(
                    out,
                    "Шаблон {} v{} → {} (файлов: {}, язык: {})",
                    report.template,
                    report.version,
                    report.target_dir.display(),
                    report.written.len(),
                    report.lang
                );
                let _ = writeln!(out, "\nФрагмент для CONSTRAINTS.yaml:\n{}", report.fragment);
                let _ = writeln!(out, "\nСтрока инварианта: {}", report.verified_by);
                for note in &report.notes {
                    let _ = writeln!(out, "- {note}");
                }
                Ok(crate::tool::ToolOutput::ok(out))
            }
            Err(e) => Ok(crate::tool::ToolOutput::err(e.to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_case() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tmp");
        std::fs::write(
            dir.path().join("ARCHITECTURE-SPINE.md"),
            "# Спайн\n\n## AD-1. Идемпотентность по ключу\n\n- **Binds**: Приём, Журнал\n\
             - **Prevents**: двойную выплату\n- **Rule**: ключ идемпотентности на входе\n\n\
             ## AD-2. Журнал append-only\n\n- **Binds**: Журнал\n- **Prevents**: подмену истории\n\
             - **Rule**: журнал только дополняется\n",
        )
        .expect("spine");
        dir
    }

    #[test]
    fn templates_are_embedded_and_manifests_valid() {
        let all = templates().expect("каталог");
        assert!(all.len() >= 9, "шаблонов минимум 9: {}", all.len());
        for t in &all {
            assert!(t.files.contains_key("template.yaml"), "{}", t.manifest.id);
            assert!(!t.manifest.title.trim().is_empty(), "{}", t.manifest.id);
            assert!(
                t.manifest.rule.kind == "command_succeeds",
                "{}: тип {}",
                t.manifest.id,
                t.manifest.rule.kind
            );
            assert!(
                t.manifest.rule.commands.python.is_some(),
                "{}: нет python-команды",
                t.manifest.id
            );
            if t.manifest.executable {
                assert!(
                    !t.manifest.violating.is_empty(),
                    "{}: исполняемый шаблон без нарушающей реализации (П2)",
                    t.manifest.id
                );
                assert!(
                    t.manifest.match_terms.iter().next().is_some(),
                    "{}: пустой словарь подбора",
                    t.manifest.id
                );
            }
            // Каждый объявленный файл существует во встроенных ассетах.
            for f in &t.manifest.files {
                assert!(
                    t.file(&f.from).is_some(),
                    "{}: нет файла {}",
                    t.manifest.id,
                    f.from
                );
            }
            for v in &t.manifest.violating {
                assert!(
                    t.file(&v.from).is_some(),
                    "{}: нет {}",
                    t.manifest.id,
                    v.from
                );
            }
        }
    }

    #[test]
    fn matching_prefers_the_idempotency_template() {
        let hits = match_templates("Выплата идемпотентна по ключу: повторная доставка", 2)
            .expect("подбор");
        assert_eq!(hits[0].id, "idempotency-key", "{hits:?}");
        assert!(hits[0].score >= 2, "счёт: {}", hits[0].score);
        assert!(hits[0].matched.iter().any(|m| m.starts_with("идемпотент")));
    }

    #[test]
    fn unknown_text_yields_no_match() {
        let hits = match_templates("Космический лифт и расписание запусков", 2).expect("подбор");
        assert!(hits.is_empty(), "{hits:?}");
    }

    #[test]
    fn spine_ad_text_extracts_only_its_block() {
        let case = tmp_case();
        let text = spine_ad_text(case.path(), "AD-1").expect("блок");
        assert!(text.contains("Идемпотентность по ключу"), "{text}");
        assert!(text.contains("Binds"), "{text}");
        assert!(!text.contains("append-only"), "захвачен чужой блок: {text}");
        assert!(spine_ad_text(case.path(), "AD-9").is_none(), "нет AD-9");
    }

    #[test]
    fn ad_refs_are_equivalent_by_number() {
        assert!(rule_ref_eq("C-2", "C-002"));
        assert!(rule_ref_eq("C-7", "C-7"));
        assert!(!rule_ref_eq("C-7", "C-8"));
        assert!(!rule_ref_eq("C-7", "OWNER-1"));
    }

    #[test]
    fn apply_writes_files_and_lock_and_is_dry_run_safe() {
        let case = tmp_case();
        let report =
            apply(case.path(), "idempotency-key", "AD-1", Lang::Python, true).expect("dry-run");
        assert!(report.dry_run);
        assert!(
            !case.path().join(LOCK_REL).exists(),
            "dry-run пишет на диск"
        );
        assert!(
            report.fragment.contains("type: command_succeeds"),
            "{}",
            report.fragment
        );
        assert!(report.fragment.contains("ad: AD-1"), "{}", report.fragment);

        let report =
            apply(case.path(), "idempotency-key", "AD-1", Lang::Python, false).expect("apply");
        assert!(report.written.iter().all(|p| p.exists()));
        assert!(case.path().join(LOCK_REL).is_file());
        let lock = read_lock(&case.path().join(LOCK_REL)).expect("lock");
        assert_eq!(lock.len(), 1);
        assert_eq!(lock[0].ad, "AD-1");
        assert!(!lock[0].files.is_empty());
        // Повторное применение — конфликт, а не тихая перезапись.
        let err = apply(case.path(), "idempotency-key", "AD-1", Lang::Python, false)
            .expect_err("конфликт");
        assert!(err.to_string().contains("уже существуют"), "{err}");
    }

    #[test]
    fn apply_rejects_unknown_ad_and_missing_spine() {
        let case = tmp_case();
        let err = apply(case.path(), "idempotency-key", "AD-9", Lang::Python, false)
            .expect_err("нет такого инварианта");
        assert!(err.to_string().contains("не найден в спайне"), "{err}");
        let err =
            apply(case.path(), "idempotency-key", "1", Lang::Python, false).expect_err("форма");
        assert!(err.to_string().contains("AD-<номер>"), "{err}");
        let empty = tempfile::tempdir().expect("tmp");
        let err = apply(empty.path(), "idempotency-key", "AD-1", Lang::Python, false)
            .expect_err("нет спайна");
        assert!(
            err.to_string().contains("нет объявленных инвариантов"),
            "{err}"
        );
    }

    #[test]
    fn apply_does_not_touch_registry_or_spine() {
        let case = tmp_case();
        std::fs::write(
            case.path().join("CONSTRAINTS.yaml"),
            "rules:\n  - id: C-001\n    name: doc_word\n    type: must_contain\n    \
             glob: 'docs/**/*.md'\n    pattern: 'журнал'\n    severity: error\n",
        )
        .expect("registry");
        let before = std::fs::read_to_string(case.path().join("CONSTRAINTS.yaml")).expect("read");
        let spine_before =
            std::fs::read_to_string(case.path().join("ARCHITECTURE-SPINE.md")).expect("read");
        apply(case.path(), "idempotency-key", "AD-1", Lang::Python, false).expect("apply");
        assert_eq!(
            std::fs::read_to_string(case.path().join("CONSTRAINTS.yaml")).expect("read"),
            before,
            "реестр правил изменён"
        );
        assert_eq!(
            std::fs::read_to_string(case.path().join("ARCHITECTURE-SPINE.md")).expect("read"),
            spine_before,
            "спайн изменён"
        );
    }

    #[test]
    fn coverage_sees_text_only_rules_and_load_bearing() {
        let case = tmp_case();
        std::fs::create_dir_all(case.path().join("model")).expect("model");
        std::fs::write(
            case.path().join("model/AD-001-idempotentnost.md"),
            "---\nid: AD-001\ntype: ad\ntitle: \"Идемпотентность\"\nstatus: \"ADOPTED\"\n\
             load_bearing: true\nverified_by: [C-002]\n---\n\n- **Binds**: Приём\n",
        )
        .expect("ad1");
        std::fs::write(
            case.path().join("model/AD-002-zhurnal.md"),
            "---\nid: AD-002\ntype: ad\ntitle: \"Журнал\"\nstatus: \"ADOPTED\"\n\
             verified_by: [C-009]\n---\n\n- **Binds**: Журнал\n",
        )
        .expect("ad2");
        std::fs::write(
            case.path().join("CONSTRAINTS.yaml"),
            "rules:\n  - id: C-002\n    name: idempotency_key_in_code\n    type: must_contain\n    \
             glob: 'skeleton/**/*.py'\n    pattern: 'idempotency_key'\n    severity: error\n    \
             ad: AD-1\n  - id: C-009\n    name: tests_run\n    type: command_succeeds\n    \
             command: 'true'\n    severity: error\n",
        )
        .expect("registry");
        let cov = ad_coverage(case.path())
            .expect("coverage")
            .expect("model есть");
        assert_eq!(cov.total(), 2);
        assert!(cov.load_bearing_defined);
        let uncovered = cov.uncovered();
        assert_eq!(uncovered.len(), 1, "покрыт только AD-002: {uncovered:?}");
        assert_eq!(uncovered[0].ad, "AD-001");
        assert!(uncovered[0].load_bearing, "несущий идёт первым и помечен");
        assert_eq!(uncovered[0].rules[0].kind.as_deref(), Some("must_contain"));
        assert_eq!(cov.covered()[0].ad, "AD-002");
    }

    #[test]
    fn coverage_is_silent_without_model_dir() {
        let case = tmp_case();
        assert!(ad_coverage(case.path()).expect("нет модели").is_none());
    }

    #[test]
    fn package_runs_the_bash_command_and_reports_code() {
        let dir = tempfile::tempdir().expect("tmp");
        let (code, _) = run_shell(dir.path(), "exit 0", Duration::from_secs(10)).expect("прогон");
        assert_eq!(code, Some(0));
        let (code, _) = run_shell(dir.path(), "exit 3", Duration::from_secs(10)).expect("прогон");
        assert_eq!(code, Some(3));
        let (code, _) =
            run_shell(dir.path(), "sleep 30", Duration::from_millis(200)).expect("таймаут");
        assert_eq!(code, None, "команда должна быть убита по таймауту");
    }
}
