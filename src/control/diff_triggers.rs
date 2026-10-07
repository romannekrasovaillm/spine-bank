//! Архитектурная значимость (ADR-034): 15 канонических триггеров, маршруты
//! Fast/Standard/Critical ([`significance_score`], [`score_with_sources`]) и
//! механический anti-bypass floor (S-1) — вывод триггеров из git-диффа
//! ([`detect_diff_triggers`]).

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::process::{Command, Stdio};

use regex::Regex;

use super::types::{Route, Significance};
use crate::error::{HarnessError, Result};

/// Канонический список из 15 триггеров архитектурной значимости
/// (из обзора AI-Disrupt PDLC, см. `docs/SOURCE_BRIEF.md` §C.3).
pub const SIGNIFICANCE_TRIGGERS: [&str; 15] = [
    "new_component",
    "new_datastore",
    "new_vendor",
    "domain_ownership_change",
    "cross_domain_integration",
    "api_contract_change",
    "data_contract_change",
    "security_boundary_change",
    "trust_zone_change",
    "consistency_model_change",
    "significant_nfr",
    "rto_rpo_targets",
    "irreversible_migration",
    "financial_impact",
    "criticality_or_exception",
];

/// Дефолтный верхний порог маршрута Fast: score ≤ `fast_max` → Fast (ADR-034).
pub const DEFAULT_FAST_MAX: usize = 1;
/// Дефолтный верхний порог маршрута Standard: выше — Critical (ADR-034).
pub const DEFAULT_STANDARD_MAX: usize = 4;

/// Триггеры, форсирующие маршрут Critical независимо от счёта.
/// НЕ конфигурируются (fail-safe, ADR-034): границу безопасности,
/// необратимую миграцию и критичность/exception нельзя «откалибровать вниз».
const FORCING_CRITICAL_TRIGGERS: [&str; 3] = [
    "security_boundary_change",
    "irreversible_migration",
    "criticality_or_exception",
];

/// Триггеры, которые детектор диффа по умолчанию НЕ умеет видеть
/// механически (D1): они зажигаются только заявлением автора
/// (`--trigger`/`declared`). Паспорт вердикта обязан перечислять те из них,
/// что не вошли в прогон, — иначе маршрут выглядит всевидящим, а недобор
/// Critical остаётся невидимым.
///
/// Троица `security`/`trust_zone`/`data_contract` перестаёт быть слепой,
/// когда кейс задаёт глобы `[significance]` (D2): тогда сработавший триггер
/// попадает в маршрут и из этого списка исчезает.
pub const DIFF_BLIND_TRIGGERS: [&str; 7] = [
    "security_boundary_change",
    "criticality_or_exception",
    "trust_zone_change",
    "data_contract_change",
    "domain_ownership_change",
    "consistency_model_change",
    "financial_impact",
];

/// Недетектируемые триггеры, которые НЕ сработали в прогоне (D1): именно их
/// отсутствие в маршруте — риск недобора Critical, и паспорт называет их
/// поимённо. Вход — сработавшие триггеры прогона (из диффа и/или заявленные).
#[must_use]
pub fn blind_triggers_unfired<'a>(fired: impl IntoIterator<Item = &'a str>) -> Vec<&'static str> {
    let fired: std::collections::BTreeSet<&str> = fired.into_iter().collect();
    DIFF_BLIND_TRIGGERS
        .iter()
        .copied()
        .filter(|t| !fired.contains(t))
        .collect()
}

/// Оценивает значимость по карте «триггер → сработал» с дефолтными порогами
/// ([`DEFAULT_FAST_MAX`]/[`DEFAULT_STANDARD_MAX`]) — обратная совместимость.
#[must_use]
pub fn significance_score(answers: &BTreeMap<String, bool>) -> Significance {
    significance_score_with_limits(answers, DEFAULT_FAST_MAX, DEFAULT_STANDARD_MAX)
}

/// Оценивает значимость с явными порогами маршрутизации (ADR-034):
/// score ≤ `fast_max` → Fast, ≤ `standard_max` → Standard, выше → Critical;
/// любой из [`FORCING_CRITICAL_TRIGGERS`] → Critical независимо от счёта.
///
/// Пороги ожидаются валидированными (`fast_max < standard_max`,
/// см. `crate::config::SignificanceConfig::limits`); при невалидных граница
/// Fast пуста (fail-safe в сторону более строгого маршрута).
#[must_use]
pub fn significance_score_with_limits(
    answers: &BTreeMap<String, bool>,
    fast_max: usize,
    standard_max: usize,
) -> Significance {
    // T-04: в счёт идут ТОЛЬКО канонические триггеры. Раньше выдуманное имя
    // («foo») увеличивало score и поднимало маршрут: подсчёт по карте без
    // словаря превращал опечатку в маршрут. Отвергает такие вызовы граница
    // (`control score`, MCP `significance_score`) — здесь же защита от того,
    // чтобы счёт вообще зависел от незнакомого ключа.
    let fired: Vec<String> = answers
        .iter()
        .filter(|(k, v)| **v && SIGNIFICANCE_TRIGGERS.contains(&k.as_str()))
        .map(|(k, _)| k.clone())
        .collect();
    let critical = FORCING_CRITICAL_TRIGGERS
        .iter()
        .any(|t| fired.iter().any(|f| f == t));
    let score = fired.len();
    let route = if critical || score > standard_max {
        Route::Critical
    } else if score > fast_max {
        Route::Standard
    } else {
        Route::Fast
    };
    Significance {
        score,
        fired,
        route,
    }
}

/// Незнакомые имена триггеров в ответе (T-04): ключи карты, которых нет в
/// [`SIGNIFICANCE_TRIGGERS`], — в порядке словаря.
///
/// Считается по КЛЮЧАМ, а не по сработавшим: опечатка со значением `false`
/// (`new_components=false`) означает, что архитектор не отметил настоящий
/// триггер, — молча принять её значит потерять признание значимости.
#[must_use]
pub fn unknown_trigger_names(answers: &BTreeMap<String, bool>) -> Vec<String> {
    answers
        .keys()
        .filter(|k| !SIGNIFICANCE_TRIGGERS.contains(&k.as_str()))
        .cloned()
        .collect()
}

/// Ближайшее каноническое имя триггера для опечатки (T-04).
///
/// Два признака похожести: незавершённый ввод (`security_boundary` →
/// `security_boundary_change` — одно имя префикс другого) и опечатка
/// (`new_components` → `new_component` — расстояние Левенштейна в пределах
/// трети длины). Порог по длине отсекает случайные совпадения: для `foo`
/// (3) допустима правка в один символ, но ни одно каноническое имя так близко
/// не лежит — подсказки нет, и это честнее выдуманной.
#[must_use]
pub fn suggest_trigger(name: &str) -> Option<&'static str> {
    if SIGNIFICANCE_TRIGGERS.contains(&name) {
        return None;
    }
    let limit = (name.chars().count() / 3).max(1);
    SIGNIFICANCE_TRIGGERS
        .iter()
        .map(|t| {
            let prefix = t.starts_with(name) || name.starts_with(*t);
            let dist = levenshtein(name, t);
            (*t, prefix, dist)
        })
        .filter(|(_, prefix, dist)| *prefix || *dist <= limit)
        .min_by_key(|(t, _, dist)| (*dist, t.len()))
        .map(|(t, _, _)| t)
}

/// Расстояние Левенштейна по символам (без зависимостей).
fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0usize; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        cur[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            cur[j + 1] = (prev[j] + cost).min(prev[j + 1] + 1).min(cur[j] + 1);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

/// Текст ошибки о незнакомых триггерах (T-04) — единый для CLI и MCP:
/// перечисляет лишние имена, ближайшее каноническое к каждому и весь
/// канонический список (спрашивать «а какие есть» второй раз незачем).
#[must_use]
pub fn unknown_triggers_error(unknown: &[String]) -> String {
    let named: Vec<String> = unknown
        .iter()
        .map(|u| match suggest_trigger(u) {
            Some(s) => format!("'{u}' (ближайшее каноническое: '{s}')"),
            None => format!("'{u}' (похожего канонического нет)"),
        })
        .collect();
    format!(
        "неизвестные триггеры: {} — в счёт они не идут. Канонические ({}): {}",
        named.join(", "),
        SIGNIFICANCE_TRIGGERS.len(),
        SIGNIFICANCE_TRIGGERS.join(", ")
    )
}

// --- S-1: механический anti-bypass floor (триггеры из git-диффа, ADR-034) ---

/// Манифесты зависимостей, отслеживаемые детекторами
/// `new_component`/`new_vendor`.
const DEP_MANIFESTS: [&str; 4] = ["Cargo.toml", "pom.xml", "package.json", "go.mod"];

/// Расширения конфиг-файлов для детектора `new_datastore` (S-1).
const CONFIG_EXTS: [&str; 9] = [
    "yaml",
    "yml",
    "toml",
    "json",
    "ini",
    "conf",
    "config",
    "properties",
    "env",
];

/// Результат механического сканирования git-диффа (S-1).
#[derive(Debug, Clone, Default)]
pub struct DiffTriggers {
    /// Сработавшие триггеры (канонические имена из [`SIGNIFICANCE_TRIGGERS`]).
    pub triggers: BTreeSet<String>,
    /// Основания срабатываний («триггер: файл») — для отчёта и аудита.
    pub evidence: Vec<String>,
    /// Пути, исключённые из детекторов манифестом `connect`/`.spineignore`
    /// (П3: исключение видимо, а не молчаливо).
    pub excluded: Vec<String>,
}

impl DiffTriggers {
    /// Фиксирует срабатывание триггера с основанием.
    fn fire(&mut self, trigger: &str, evidence: &str) {
        if self.triggers.insert(trigger.to_string()) {
            self.evidence.push(format!("{trigger}: {evidence}"));
        }
    }
}

/// stdout `git diff` в репозитории; ошибка — не git-репозиторий, git
/// недоступен или некорректный диапазон (понятный текст для CLI).
fn git_diff_out(repo: &Path, diff_args: &[String], extra: &[&str]) -> Result<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .arg("diff")
        .args(diff_args)
        .args(extra)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| {
            HarnessError::Control(format!(
                "anti-bypass: не удалось запустить git ({e}) — дифф-сканирование невозможно"
            ))
        })?;
    if !out.status.success() {
        // Сырой stderr git в отчёт не проксируем (D9): после первой строки
        // там многострочная справка использования («Используйте «--» для
        // отделения путей от редакций…»), засорявшая строку маршрута гейта
        // на репозитории без коммитов. Причина — первая непустая строка
        // без префикса «fatal:»; fail-safe семантика сохраняется.
        let stderr = String::from_utf8_lossy(&out.stderr);
        let reason = stderr
            .lines()
            .map(str::trim)
            .find(|l| !l.is_empty())
            .map_or(
                "git завершился с ошибкой без сообщения",
                |l| l.strip_prefix("fatal:").map_or(l, str::trim),
            );
        let detail: String = reason.chars().take(160).collect();
        return Err(HarnessError::Control(format!(
            "anti-bypass: {} — база диффа недоступна: {detail} \
             (не git-репозиторий, нет базового коммита или некорректный GIT_REF)",
            repo.display()
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Похожа ли добавленная строка манифеста на новую зависимость
/// (эвристика по типу манифеста; fail-safe — ложное срабатывание лишь
/// расширяет множество триггеров).
fn looks_like_dependency_line(manifest: &str, line: &str) -> bool {
    let l = line.trim();
    match manifest {
        // serde = "1.0" / serde = { version = "1.0" }
        "Cargo.toml" => {
            let name_ok = l.split(['=', ' ']).next().is_some_and(|n| {
                !n.is_empty()
                    && n.chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
            });
            name_ok && l.contains('=') && l.contains('"')
        }
        // "left-pad": "^1.3.0"
        "package.json" => l.starts_with('"') && l.contains("\":"),
        // <dependency> / <groupId>… / <artifactId>…
        "pom.xml" => {
            l.contains("<dependency>") || l.contains("<groupId>") || l.contains("<artifactId>")
        }
        // require github.com/foo/bar v1.2.3 (и строки в блоке require)
        "go.mod" => {
            l.starts_with("require ")
                || l.split_whitespace().nth(1).is_some_and(|v| {
                    v.starts_with('v') && v[1..].chars().next().is_some_and(|c| c.is_ascii_digit())
                })
        }
        _ => false,
    }
}

/// Файл — конфиг по эвристике: расширение из списка или «config»/«application»/
/// «settings» в имени (для детектора `new_datastore`).
fn looks_like_config(path: &str) -> bool {
    let name = Path::new(path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let ext = Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    CONFIG_EXTS.contains(&ext.as_str())
        || name.contains("config")
        || name.contains("application")
        || name.contains("settings")
}

/// Компилирует статический regex детекторов (константные паттерны —
/// ошибка компиляции означает внутренний дефект).
pub(super) fn diff_regex(pattern: &str) -> Result<Regex> {
    Regex::new(pattern)
        .map_err(|e| HarnessError::Control(format!("внутренний regex anti-bypass: {e}")))
}

/// Манифест установки `connect` (П3): провенанс того, что положил сам Spine.
pub const CONNECT_MANIFEST_PATH: &str = ".arch-handoff/connect-manifest.json";

/// Файл ручных исключений детекторов (синтаксис, близкий к gitignore).
pub const SPINEIGNORE_PATH: &str = ".spineignore";

/// Паттерны исключения путей из детекторов значимости: манифест `connect`
/// (установленное самим Spine не должно менять маршрут проекта, Д3) плюс
/// ручной `.spineignore`.
fn exclusion_patterns(repo: &Path) -> Vec<String> {
    let mut patterns: Vec<String> = Vec::new();
    if let Ok(text) = std::fs::read_to_string(repo.join(CONNECT_MANIFEST_PATH)) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
            if let Some(arr) = v.get("paths").and_then(serde_json::Value::as_array) {
                patterns.extend(arr.iter().filter_map(|x| x.as_str()).map(str::to_string));
            }
        }
    }
    if let Ok(text) = std::fs::read_to_string(repo.join(SPINEIGNORE_PATH)) {
        for line in text.lines() {
            let l = line.trim();
            if l.is_empty() || l.starts_with('#') {
                continue;
            }
            patterns.push(l.to_string());
        }
    }
    patterns
}

/// Путь исключён хотя бы одним паттерном.
fn path_excluded(patterns: &[String], path: &str) -> bool {
    let path = path.trim_start_matches("./");
    patterns
        .iter()
        .any(|p| glob_match(p.trim_start_matches('/').trim_end_matches('/'), path))
}

/// Простое сопоставление пути с паттерном-исключением: `dir/**` — весь
/// подкаталог, `*` — любой фрагмент внутри сегмента, иначе точное имя.
fn glob_match(pattern: &str, path: &str) -> bool {
    if pattern.is_empty() {
        return false;
    }
    if let Some(prefix) = pattern.strip_suffix("/**") {
        return path == prefix || path.starts_with(&format!("{prefix}/"));
    }
    if let Some(prefix) = pattern.strip_suffix('/') {
        return path.starts_with(prefix);
    }
    if let Some(prefix) = pattern
        .split("**")
        .next()
        .filter(|_| pattern.contains("**"))
    {
        return path.starts_with(prefix);
    }
    let p: Vec<&str> = pattern.split('/').collect();
    let s: Vec<&str> = path.split('/').collect();
    p.len() == s.len() && p.iter().zip(s.iter()).all(|(pp, ss)| segment_match(pp, ss))
}

/// Сопоставление одного сегмента пути с сегментом паттерна (`*` — glob).
fn segment_match(pat: &str, seg: &str) -> bool {
    if pat == "*" {
        return true;
    }
    if !pat.contains('*') {
        return pat == seg;
    }
    let parts: Vec<&str> = pat.split('*').collect();
    let mut rest = seg;
    for (i, part) in parts.iter().enumerate() {
        if part.is_empty() {
            continue;
        }
        if i == 0 {
            if !rest.starts_with(part) {
                return false;
            }
            rest = &rest[part.len()..];
        } else if i == parts.len() - 1 {
            return rest.ends_with(part);
        } else if let Some(pos) = rest.find(part) {
            rest = &rest[pos + part.len()..];
        } else {
            return false;
        }
    }
    true
}

/// База диффа в форме, которую понимает git (T-03).
///
/// `--base` приходит из трёх источников с разными привычками: человек пишет
/// `origin/main` или `origin/main...HEAD` (так подсказывает `--help`), шаблоны
/// `connect` писали уже готовый диапазон `rev...HEAD`, CI — третью форму.
/// Дописывание `...HEAD` к готовому диапазону давало `rev...HEAD...HEAD`:
/// git отказывал, дифф не вычислялся, и гейт молча уходил в fail-safe Critical
/// — то есть строгость зависела от формы записи базы, а не от изменения.
///
/// Правило одно: значение с `..` — это уже диапазон, используется как есть;
/// голая ревизия получает `...HEAD`.
#[must_use]
pub fn normalize_base_range(base: &str) -> String {
    if base.contains("..") {
        base.to_string()
    } else {
        format!("{base}...HEAD")
    }
}

/// Одиночная ревизия из базы диффа: `origin/main...HEAD` → `origin/main`
/// (для `git show`/`git rev-parse`, которые диапазон не принимают).
#[must_use]
pub fn base_rev(base: &str) -> &str {
    match base.split_once("...") {
        Some((left, _)) => left,
        None => base.split_once("..").map_or(base, |(left, _)| left),
    }
}

/// Глобы детекторов значимости (T-05): задаются секцией `[significance]`
/// (`arch-harness.toml` проекта или `config.toml` пользователя), чтобы
/// соглашения репозитория («контракты лежат в `docs/contracts/`») не были
/// зашиты в бинарь.
/// Структура глобов детекторов диффа (T-05): что считать контрактом,
/// компонентом, интеграцией и NFR-сущностью модели.
#[derive(Debug, Clone)]
pub struct DiffGlobs {
    /// Пути/глобы контрактов.
    pub contracts: Vec<String>,
    /// Файлы сущностей модели, появление которых — новый компонент.
    pub components: Vec<String>,
    /// Файлы сущностей модели-интеграций.
    pub integrations: Vec<String>,
    /// Файлы NFR-сущностей модели (1.7 п.3).
    pub nfr: Vec<String>,
    /// Глобы границы безопасности (D2): auth-конфиги, сетевые политики,
    /// IAM. ПУСТ по умолчанию — без явной настройки кейса поведение прежнее
    /// (триггер `security_boundary_change` остаётся заявляемым); глобальных
    /// эвристик нет, соглашения не зашиваются в бинарь.
    pub security: Vec<String>,
    /// Глобы зон доверия (D2): границы контуров, mesh-политики. Пусто по
    /// умолчанию, как у [`DiffGlobs::security`].
    pub trust_zone: Vec<String>,
    /// Глобы контрактов данных (D2): схемы событий, топиков, Avro-схемы.
    /// Пусто по умолчанию, как у [`DiffGlobs::security`].
    pub data_contract: Vec<String>,
}

impl Default for DiffGlobs {
    fn default() -> Self {
        Self {
            contracts: vec!["docs/contracts/**".to_string(), "contracts/**".to_string()],
            components: vec!["model/CMP-*".to_string()],
            integrations: vec!["model/INT-*".to_string()],
            nfr: vec!["model/NFR-*".to_string()],
            security: Vec::new(),
            trust_zone: Vec::new(),
            data_contract: Vec::new(),
        }
    }
}

/// Механический вывод триггеров значимости из git-диффа (S-1, ADR-034)
/// с дефолтными глобами (T-05) — обратная совместимость.
///
/// # Errors
/// Как у [`detect_diff_triggers_with`].
pub fn detect_diff_triggers(repo: &Path, git_ref: Option<&str>) -> Result<DiffTriggers> {
    detect_diff_triggers_with(repo, git_ref, &DiffGlobs::default())
}

/// Механический вывод триггеров значимости из git-диффа (S-1, ADR-034).
///
/// Диапазон: `git_ref = None` — рабочее дерево против `HEAD` (staged +
/// unstaged + untracked); `Some(r)` — `git diff` по [`normalize_base_range`]
/// (`r...HEAD` для голой ревизии, диапазон как есть). Детекторы
/// (эвристики, fail-safe — только расширяют множество):
///
/// - `new_component` — добавлен каталог верхнего/второго уровня с манифестом
///   (Cargo.toml/pom.xml/package.json/go.mod), каталог `src/` или сущность
///   модели по глобу [`DiffGlobs::components`];
/// - `new_vendor` — в диффе манифеста зависимостей добавлена строка
///   зависимости;
/// - `api_contract_change` — изменён, добавлен или УДАЛЁН контракт: по
///   содержимому (`openapi:`/`asyncapi:`/`swagger:` ключом верхнего уровня,
///   расширение `.proto`) либо по глобу [`DiffGlobs::contracts`] (T-05:
///   раньше — только по `openapi`/`asyncapi` в имени файла, из-за чего
///   `docs/contracts/wallet-api.v1.yaml` в дельте был невидим);
/// - `cross_domain_integration` — появилась или изменена сущность интеграции
///   модели по глобу [`DiffGlobs::integrations`];
/// - `significant_nfr` — появилась или изменена NFR-сущность модели по глобу
///   [`DiffGlobs::nfr`] (1.7 п.3: новая NFR — новое требование к системе,
///   правка — изменение целевого показателя);
/// - `rto_rpo_targets` — в добавленных строках файлов `model/` изменились
///   цели RTO/RPO: поля `rto_minutes:`/`rpo_seconds:` (канон) или инлайн-формы
///   «RTO ≤ 15»/«RPO = 0» (1.7 п.3: правка цели 4ч → 15мин по имени файла
///   не видна);
/// - `irreversible_migration` — в диффе файла миграций (каталог `migrations/`
///   или `*.sql`) есть `DROP TABLE`/`TRUNCATE`/`DROP COLUMN`;
/// - `new_datastore` — в конфигах добавлены строки подключения
///   (`postgres://`/`postgresql://`/`mysql://`/`mongodb://`/`redis://`/
///   `kafka://`/`bootstrap.servers`), а не голое слово.
/// - `security_boundary_change` / `trust_zone_change` /
///   `data_contract_change` (D2) — изменён файл под глобом кейса
///   [`DiffGlobs::security`] / [`DiffGlobs::trust_zone`] /
///   [`DiffGlobs::data_contract`]. Глобы по умолчанию пусты: без настройки
///   `[significance] *_globs` эти триггеры детектор не видит (слепая зона
///   паспорта, D1) — глобальные эвристики намеренно не вводятся.
///
/// # Errors
/// Не git-репозиторий, git недоступен, некорректный `GIT_REF`.
pub fn detect_diff_triggers_with(
    repo: &Path,
    git_ref: Option<&str>,
    globs: &DiffGlobs,
) -> Result<DiffTriggers> {
    let range: Vec<String> = match git_ref {
        None => vec!["HEAD".to_string()],
        Some(r) => vec![normalize_base_range(r)],
    };
    let name_status = git_diff_out(repo, &range, &["--name-status"])?;
    let patch_text = git_diff_out(repo, &range, &[])?;

    // Изменённые файлы: (статус, путь). Rename/copy — берём новый путь.
    let mut files: Vec<(char, String)> = Vec::new();
    for line in name_status.lines() {
        let mut parts = line.split('\t');
        let Some(status) = parts.next() else {
            continue;
        };
        let code = status.chars().next().unwrap_or(' ');
        let path = match code {
            'R' | 'C' => parts.nth(1),
            _ => parts.next(),
        };
        if let Some(p) = path {
            files.push((code, p.to_string()));
        }
    }

    // Добавленные строки по файлам (разбор unified diff).
    let mut added: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut current: Option<String> = None;
    for line in patch_text.lines() {
        if let Some(rest) = line.strip_prefix("+++ b/") {
            current = Some(rest.to_string());
        } else if line.starts_with("+++") {
            current = None; // +++ /dev/null — удалённый файл
        } else if let Some(cur) = &current {
            if let Some(l) = line.strip_prefix('+') {
                added.entry(cur.clone()).or_default().push(l.to_string());
            }
        }
    }

    // В режиме рабочего дерева untracked-файлы git-diff не видит — а новый
    // компонент почти всегда начинается с untracked. Дочитываем их как
    // добавленные (весь файл — «добавленные строки»).
    if git_ref.is_none() {
        let out = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(["ls-files", "--others", "--exclude-standard"])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output();
        if let Ok(out) = out {
            if out.status.success() {
                for path in String::from_utf8_lossy(&out.stdout).lines() {
                    let path = path.trim();
                    if path.is_empty() {
                        continue;
                    }
                    files.push(('A', path.to_string()));
                    // Нечитаемый/бинарный файл — без контентных детекторов.
                    if let Ok(content) = std::fs::read_to_string(repo.join(path)) {
                        added
                            .entry(path.to_string())
                            .or_default()
                            .extend(content.lines().map(str::to_string));
                    }
                }
            }
        }
    }

    // П3 (Д3): пути, установленные самим Spine (`connect`), и явные
    // исключения `.spineignore` не участвуют в оценке значимости изменения
    // проекта — иначе подключение инструмента завышает маршрут.
    let patterns = exclusion_patterns(repo);
    let mut excluded: Vec<String> = Vec::new();
    if !patterns.is_empty() {
        files.retain(|(_, p)| {
            if path_excluded(&patterns, p) {
                excluded.push(p.clone());
                false
            } else {
                true
            }
        });
        added.retain(|k, _| !path_excluded(&patterns, k));
        excluded.sort();
        excluded.dedup();
    }

    let re_migration = diff_regex(r"(?i)\b(?:drop\s+table|truncate|drop\s+column)\b")?;
    // Строки подключения, а не голое слово (ДКА: `kafka` без схемы ловило
    // любое упоминание в YAML/JSON и давало ложный `new_datastore`).
    let re_datastore = diff_regex(
        r"(?i)(?:postgres(?:ql)?://|mysql://|mongodb(?:\+srv)?://|redis://|kafka://|bootstrap\.servers)",
    )?;
    // Цели RTO/RPO (1.7 п.3): frontmatter-поля `rto_minutes:` / `rpo_seconds:`
    // (канон типизированной модели) и инлайн-формы «RTO ≤ 15», «RPO = 0».
    let re_rto_rpo_field = diff_regex(r"(?i)^\s*(?:rto|rpo)_\w*\s*:")?;
    let re_rto_rpo_target = diff_regex(r"(?i)\b(?:rto|rpo)\s*[≤<:=]\s*\d")?;

    let mut found = DiffTriggers::default();
    for (code, path) in &files {
        let segs: Vec<&str> = path.split('/').collect();
        let file_name = segs.last().copied().unwrap_or_default();
        let lower_name = file_name.to_ascii_lowercase();
        let lower_path = path.to_ascii_lowercase();

        // new_component: добавлен каталог 1-го/2-го уровня с манифестом, src/
        // или сущность модели по глобу (T-05).
        if *code == 'A' {
            if (2..=3).contains(&segs.len()) && DEP_MANIFESTS.contains(&file_name) {
                found.fire("new_component", &format!("добавлен манифест {path}"));
            }
            if segs.len() >= 2 && (segs[0] == "src" || (segs.len() >= 3 && segs[1] == "src")) {
                found.fire("new_component", &format!("добавлены исходники {path}"));
            }
            if globs
                .components
                .iter()
                .any(|g| glob_match(g, path.as_str()))
            {
                found.fire("new_component", &format!("новая сущность модели {path}"));
            }
        }

        // cross_domain_integration: появилась или изменилась сущность
        // интеграции модели (T-05). Новая интеграция — появление связи;
        // правка существующей — изменение интерфейса, и оба случая значимы
        // (детектор только расширяет множество, ADR-034).
        if (*code == 'A' || *code == 'M')
            && globs
                .integrations
                .iter()
                .any(|g| glob_match(g, path.as_str()))
        {
            found.fire(
                "cross_domain_integration",
                &format!(
                    "{} сущность интеграции {path}",
                    if *code == 'A' {
                        "новая"
                    } else {
                        "изменена"
                    }
                ),
            );
        }

        // significant_nfr (1.7 п.3): новая NFR — новое требование к системе;
        // правка существующей — изменение целевого показателя (p99,
        // доступность, ёмкость). Та же семантика «появление/правка», что у
        // интеграций выше.
        if (*code == 'A' || *code == 'M') && globs.nfr.iter().any(|g| glob_match(g, path.as_str()))
        {
            found.fire(
                "significant_nfr",
                &format!(
                    "{} NFR-сущность {path}",
                    if *code == 'A' {
                        "новая"
                    } else {
                        "изменена"
                    }
                ),
            );
        }

        // rto_rpo_targets (1.7 п.3): правка цели восстановления (4ч → 15мин)
        // видна только по содержимому сущности модели. Область — только
        // `model/`: упоминание RTO/RPO в ADR или прозе — не цель системы.
        if *code != 'D' && lower_path.starts_with("model/") {
            if let Some(lines) = added.get(path.as_str()) {
                if let Some(l) = lines
                    .iter()
                    .find(|l| re_rto_rpo_field.is_match(l) || re_rto_rpo_target.is_match(l))
                {
                    found.fire(
                        "rto_rpo_targets",
                        &format!(
                            "цели RTO/RPO в {path}: {}",
                            l.trim().chars().take(80).collect::<String>()
                        ),
                    );
                }
            }
        }

        // new_vendor: строка зависимости в диффе манифеста.
        if *code != 'D' && DEP_MANIFESTS.contains(&file_name) {
            if let Some(lines) = added.get(path.as_str()) {
                if let Some(l) = lines
                    .iter()
                    .find(|l| looks_like_dependency_line(file_name, l))
                {
                    found.fire(
                        "new_vendor",
                        &format!(
                            "зависимость в {path}: {}",
                            l.trim().chars().take(80).collect::<String>()
                        ),
                    );
                }
            }
        }

        // api_contract_change (T-05): контракт по СОДЕРЖИМОМУ, по каталогу
        // контрактов или по имени файла. Раньше — только имя: правка
        // `docs/contracts/wallet-api.v1.yaml` (внутри `openapi: 3.0.3`, а в
        // имени слова «openapi» нет) была для детектора невидима, и дельта с
        // новым компонентом оценивалась как Fast. Удаление контракта —
        // ломающее изменение по определению, поэтому тоже срабатывание.
        let by_name = lower_name.contains("openapi") || lower_name.contains("asyncapi");
        let by_glob = globs.contracts.iter().any(|g| glob_match(g, path.as_str()));
        let by_content = *code != 'D' && file_looks_like_contract(&repo.join(path));
        if by_name || by_glob || by_content {
            let how = if by_content && !by_name && !by_glob {
                "по содержимому"
            } else if by_glob && !by_name {
                "в каталоге контрактов"
            } else {
                "по имени файла"
            };
            found.fire(
                "api_contract_change",
                &format!(
                    "{} контракт {path} ({how})",
                    if *code == 'D' {
                        "удалён"
                    } else {
                        "изменён или добавлен"
                    }
                ),
            );
        }

        // irreversible_migration: DDL разрушения в файлах миграций.
        let is_migration = lower_path.split('/').any(|s| s == "migrations")
            || Path::new(path)
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("sql"));
        if *code != 'D' && is_migration {
            if let Some(lines) = added.get(path.as_str()) {
                if let Some(l) = lines.iter().find(|l| re_migration.is_match(l)) {
                    found.fire(
                        "irreversible_migration",
                        &format!(
                            "разрушающий DDL в {path}: {}",
                            l.trim().chars().take(80).collect::<String>()
                        ),
                    );
                }
            }
        }

        // new_datastore: строки подключения в конфигах.
        if *code != 'D' && looks_like_config(path) {
            if let Some(lines) = added.get(path.as_str()) {
                if let Some(l) = lines.iter().find(|l| re_datastore.is_match(l)) {
                    found.fire(
                        "new_datastore",
                        &format!(
                            "строка подключения в {path}: {}",
                            l.trim().chars().take(80).collect::<String>()
                        ),
                    );
                }
            }
        }

        // Глобы кейса для недетектируемых триггеров (D2): изменение файла
        // под явно настроенным глобом зажигает триггер (любой статус правки:
        // добавление, изменение, удаление файла границы безопасности значимо
        // одинаково). Глобальных эвристик нет: без `[significance] *_globs`
        // в конфиге кейса эти детекторы молчат, и триггеры остаются
        // заявляемыми (слепая зона паспорта, D1).
        for (key_globs, trigger, what) in [
            (
                &globs.security,
                "security_boundary_change",
                "границы безопасности",
            ),
            (&globs.trust_zone, "trust_zone_change", "зоны доверия"),
            (
                &globs.data_contract,
                "data_contract_change",
                "контракта данных",
            ),
        ] {
            if key_globs.iter().any(|g| glob_match(g, path.as_str())) {
                let verb = match code {
                    'A' => "добавлен",
                    'D' => "удалён",
                    'R' | 'C' => "переименован",
                    _ => "изменён",
                };
                found.fire(trigger, &format!("{verb} файл {what} {path} (глоб кейса)"));
            }
        }
    }
    found.excluded = excluded;
    Ok(found)
}

/// Похож ли файл на контракт по содержимому (T-05): ключ верхнего уровня
/// `openapi:`/`asyncapi:`/`swagger:` в первых строках (комментарии и
/// документные разделители YAML пропускаются) либо расширение `.proto`.
///
/// Читаются только первые [`CONTRACT_PROBE_BYTES`] байт: контракт опознаётся
/// по заголовку, а тянуть в память многомегабайтный файл ради одной строки
/// незачем. Нечитаемый файл (удалён, бинарный, нет прав) — не контракт:
/// молчаливая догадка хуже пропуска, а имя/каталог всё равно проверяются.
fn file_looks_like_contract(path: &Path) -> bool {
    if path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("proto"))
    {
        return true;
    }
    let Ok(mut file) = std::fs::File::open(path) else {
        return false;
    };
    let mut buf = vec![0u8; CONTRACT_PROBE_BYTES];
    let Ok(n) = std::io::Read::read(&mut file, &mut buf) else {
        return false;
    };
    let head = String::from_utf8_lossy(&buf[..n]);
    // Ключ верхнего уровня: без отступа, `openapi:`/`asyncapi:`/`swagger:`
    // (вложенные `openapi:` внутри схем и JSON-поля не в счёт).
    head.lines().any(|line| {
        let line = line.trim_end();
        !line.starts_with([' ', '\t', '#'])
            && ["openapi:", "asyncapi:", "swagger:"]
                .iter()
                .any(|k| line.starts_with(k))
    })
}

/// Сколько байт файла читается при опознании контракта по содержимому (T-05).
const CONTRACT_PROBE_BYTES: usize = 4096;

/// Источник срабатывания триггера значимости (anti-bypass отчёт, S-1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TriggerSource {
    /// Заявлен флагом `--trigger`.
    Declared,
    /// Найден механически по git-диффу.
    Diff,
    /// Заявлен флагом и подтверждён диффом.
    Both,
}

impl TriggerSource {
    /// Метка для вывода: `(declared)` / `(diff)` / `(declared+diff)`.
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Self::Declared => "declared",
            Self::Diff => "diff",
            Self::Both => "declared+diff",
        }
    }
}

/// Результат fail-safe объединения заявленных и найденных по диффу триггеров.
#[derive(Debug, Clone)]
pub struct ScoredTriggers {
    /// Оценка по объединённому множеству.
    pub significance: Significance,
    /// Сработавший триггер → источник.
    pub sources: BTreeMap<String, TriggerSource>,
    /// Найдены диффом, но не заявлены флагами (расхождение «заявлено vs
    /// видно по диффу» — anti-bypass сигнал; на маршрут влияет через
    /// объединённое множество, на exit code не влияет).
    pub undeclared: Vec<String>,
}

/// Fail-safe объединение (S-1, ADR-034): детектор ТОЛЬКО добавляет триггеры
/// к заявленным `--trigger` флагами (объединение множеств) — маршрут
/// считается по объединённому множеству с порогами `fast_max`/`standard_max`.
#[must_use]
pub fn score_with_sources(
    answers: &BTreeMap<String, bool>,
    diff: &DiffTriggers,
    fast_max: usize,
    standard_max: usize,
) -> ScoredTriggers {
    let mut merged = answers.clone();
    let mut sources = BTreeMap::new();
    for (name, fired) in answers {
        if *fired {
            sources.insert(name.clone(), TriggerSource::Declared);
        }
    }
    let mut undeclared = Vec::new();
    for t in &diff.triggers {
        if let Some(s) = sources.get_mut(t) {
            *s = TriggerSource::Both;
        } else {
            sources.insert(t.clone(), TriggerSource::Diff);
            undeclared.push(t.clone());
        }
        merged.insert(t.clone(), true);
    }
    ScoredTriggers {
        significance: significance_score_with_limits(&merged, fast_max, standard_max),
        sources,
        undeclared,
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn significance_routes_by_count() {
        let mut answers = BTreeMap::new();
        assert_eq!(significance_score(&answers).route, Route::Fast);
        answers.insert("new_component".into(), true);
        answers.insert("new_vendor".into(), true);
        assert_eq!(significance_score(&answers).route, Route::Standard);
        answers.insert("security_boundary_change".into(), true);
        assert_eq!(significance_score(&answers).route, Route::Critical);
    }

    /// Пишет файл в каталог и возвращает его путь.
    fn write_file(dir: &Path, name: &str, content: &str) -> PathBuf {
        let p = dir.join(name);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&p, content).unwrap();
        p
    }

    // --- S-1: anti-bypass floor (триггеры из диффа) -----------------------------

    /// git в каталоге с тестовой идентичностью коммиттера (изоляция AD-7).
    fn git_in(dir: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .output()
            .expect("git");
        assert!(
            out.status.success(),
            "git {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    /// Git-репозиторий с baseline-коммитом.
    fn git_repo(dir: &Path) -> PathBuf {
        let repo = dir.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        git_in(&repo, &["init", "-q", "-b", "main"]);
        write_file(&repo, "README.md", "base\n");
        write_file(&repo, "Cargo.toml", "[package]\nname = \"x\"\n");
        git_in(&repo, &["add", "."]);
        git_in(&repo, &["commit", "-q", "-m", "baseline"]);
        repo
    }

    #[test]
    fn diff_detectors_fire_on_committed_changes() {
        let dir = tempfile::tempdir().unwrap();
        let repo = git_repo(dir.path());
        // Новый компонент (манифест + src/), новый контракт, миграция с
        // DROP TABLE, конфиг со строкой подключения, зависимость в манифесте.
        write_file(
            &repo,
            "billing/Cargo.toml",
            "[package]\nname = \"billing\"\n",
        );
        write_file(&repo, "billing/src/main.rs", "fn main() {}\n");
        write_file(&repo, "api/OpenAPI.yaml", "openapi: 3.0.0\n");
        write_file(
            &repo,
            "migrations/001_init.sql",
            "CREATE TABLE t (id int);\n",
        );
        write_file(
            &repo,
            "migrations/002_drop.sql",
            "ALTER TABLE t ADD COLUMN x int;\nDROP TABLE t;\n",
        );
        write_file(&repo, "config/app.yaml", "db: postgres://localhost/x\n");
        let cargo = std::fs::read_to_string(repo.join("Cargo.toml")).unwrap();
        write_file(
            &repo,
            "Cargo.toml",
            &format!("{cargo}\n[dependencies]\nserde = \"1.0\"\n"),
        );
        git_in(&repo, &["add", "."]);
        git_in(&repo, &["commit", "-q", "-m", "feature"]);

        let found = detect_diff_triggers(&repo, Some("HEAD~1")).unwrap();
        for t in [
            "new_component",
            "new_vendor",
            "api_contract_change",
            "irreversible_migration",
            "new_datastore",
        ] {
            assert!(found.triggers.contains(t), "нет {t}: {:?}", found.triggers);
        }
        assert_eq!(found.triggers.len(), 5, "{:?}", found.triggers);
        assert_eq!(found.evidence.len(), 5, "{:?}", found.evidence);

        // Чистый дифф (HEAD против самого себя) — триггеров нет.
        let clean = detect_diff_triggers(&repo, None).unwrap();
        assert!(clean.triggers.is_empty(), "{:?}", clean.triggers);
    }

    /// 1.7 п.3: NFR-сущности и цели RTO/RPO поднимают маршрут. Ужесточение
    /// `rto_minutes: 240 → 15` в существующей NFR раньше уходило по Fast:
    /// дифф видел CMP/INT/контракты, но не цели восстановления и новые NFR.
    #[test]
    fn diff_detector_sees_nfr_entities_and_rto_rpo_targets() {
        let dir = tempfile::tempdir().unwrap();
        let repo = git_repo(dir.path());
        write_file(
            &repo,
            "model/NFR-006-rto.md",
            "---\nid: NFR-006\ntype: nfr\ntitle: RTO\nstatus: accepted\nrto_minutes: 240\n---\n\nЦель: 240 минут.\n",
        );
        write_file(
            &repo,
            "model/CMP-002-core.md",
            "---\nid: CMP-002\ntype: cmp\ntitle: Ядро\nstatus: accepted\n---\n\nТело.\n",
        );
        write_file(
            &repo,
            "docs/adr/ADR-001-dr.md",
            "# ADR-001\n\nРезерв: RTO ≤ 240, RPO ≤ 300.\n",
        );
        git_in(&repo, &["add", "."]);
        git_in(&repo, &["commit", "-q", "-m", "model"]);

        // Ужесточение цели восстановления в существующей NFR.
        write_file(
            &repo,
            "model/NFR-006-rto.md",
            "---\nid: NFR-006\ntype: nfr\ntitle: RTO\nstatus: accepted\nrto_minutes: 15\n---\n\nЦель: 15 минут.\n",
        );
        // Новая NFR-сущность.
        write_file(
            &repo,
            "model/NFR-013-dr-drill-rpo.md",
            "---\nid: NFR-013\ntype: nfr\ntitle: RPO\nstatus: accepted\nrpo_seconds: 60\n---\n\nПотеря данных ≤ 60 секунд.\n",
        );
        // RTO/RPO в ДОКУМЕНТЕ (не сущность модели) — не цель системы.
        write_file(
            &repo,
            "docs/adr/ADR-001-dr.md",
            "# ADR-001\n\nРезерв: RTO ≤ 240, RPO ≤ 60.\n",
        );
        // Обычная правка CMP без целей — NFR-триггеров не даёт.
        write_file(
            &repo,
            "model/CMP-002-core.md",
            "---\nid: CMP-002\ntype: cmp\ntitle: Ядро\nstatus: accepted\n---\n\nТело шире.\n",
        );
        git_in(&repo, &["add", "."]);
        git_in(&repo, &["commit", "-q", "-m", "nfr"]);

        let found = detect_diff_triggers(&repo, Some("HEAD~1")).unwrap();
        assert_eq!(
            found.triggers.len(),
            2,
            "ровно NFR-детекторы: {:?}",
            found.triggers
        );
        assert!(
            found.triggers.contains("significant_nfr"),
            "{:?}",
            found.triggers
        );
        assert!(
            found.triggers.contains("rto_rpo_targets"),
            "{:?}",
            found.triggers
        );
        // Доказательства называют сущности модели и поле цели, а не ADR
        // (evidence пишется по первому срабатыванию триггера — fire()).
        let ev = found.evidence.join("\n");
        assert!(ev.contains("NFR-сущность model/NFR-006"), "{ev}");
        assert!(ev.contains("rto_minutes"), "{ev}");
        assert!(
            !ev.contains("ADR-001"),
            "упоминание в прозе — не цель: {ev}"
        );
        // Маршрут не ниже Standard (score 2 > fast_max 1): правка RTO не Fast.
        let scored = score_with_sources(
            &BTreeMap::new(),
            &found,
            DEFAULT_FAST_MAX,
            DEFAULT_STANDARD_MAX,
        );
        assert_eq!(scored.significance.route, Route::Standard);
    }

    /// Только добавление NFR — «новая NFR-сущность» в основании.
    #[test]
    fn diff_detector_marks_new_nfr_entity() {
        let dir = tempfile::tempdir().unwrap();
        let repo = git_repo(dir.path());
        write_file(
            &repo,
            "model/NFR-016-error-budget.md",
            "---\nid: NFR-016\ntype: nfr\ntitle: Error budget\nstatus: accepted\n---\n\nБюджет 43 минуты/мес.\n",
        );
        git_in(&repo, &["add", "."]);
        git_in(&repo, &["commit", "-q", "-m", "nfr"]);

        let found = detect_diff_triggers(&repo, Some("HEAD~1")).unwrap();
        assert!(
            found.triggers.contains("significant_nfr"),
            "{:?}",
            found.triggers
        );
        let ev = found.evidence.join("\n");
        assert!(ev.contains("новая NFR-сущность model/NFR-016"), "{ev}");
        // Поля RTO/RPO в файле нет — второй NFR-детектор молчит.
        assert!(!found.triggers.contains("rto_rpo_targets"), "{ev}");
    }

    /// T-03: форма базы не меняет вердикт. Голая ревизия и готовый диапазон
    /// обязаны давать один и тот же набор триггеров: раньше диапазон получал
    /// второй `...HEAD`, git отказывал, и гейт молча уходил в fail-safe
    /// Critical — то есть строгость зависела от записи базы.
    /// T-04: выдуманные триггеры не поднимают маршрут, а называются ошибкой.
    /// Раньше счёт шёл по всей карте: `foo=true` был равен каноническому.
    #[test]
    fn unknown_triggers_do_not_score() {
        let mut fake = BTreeMap::new();
        for name in ["foo", "bar", "baz", "qux", "quux"] {
            fake.insert(name.to_string(), true);
        }
        let s = significance_score(&fake);
        assert_eq!(s.score, 0, "выдуманные имена не считаются: {:?}", s.fired);
        assert!(s.fired.is_empty(), "{:?}", s.fired);
        assert_eq!(s.route, Route::Fast);

        // Смесь: считается только каноническое, лишние называются поимённо.
        let mut mixed = BTreeMap::new();
        mixed.insert("new_component".to_string(), true);
        mixed.insert("new_components".to_string(), true);
        mixed.insert("new_datastore".to_string(), false);
        let unknown = unknown_trigger_names(&mixed);
        assert_eq!(unknown, vec!["new_components".to_string()]);
        assert_eq!(significance_score(&mixed).score, 1);
        assert_eq!(suggest_trigger("new_components"), Some("new_component"));
        assert_eq!(
            suggest_trigger("security_boundary"),
            Some("security_boundary_change")
        );
        assert_eq!(suggest_trigger("foo"), None);
        let text = unknown_triggers_error(&unknown);
        assert!(text.contains("'new_components'"), "{text}");
        assert!(text.contains("'new_component'"), "{text}");
        assert!(text.contains("Канонические (15)"), "{text}");

        // Опечатка со значением false — тоже незнакомая (архитектор не
        // отметил настоящий триггер, и молча это принять нельзя).
        let mut off = BTreeMap::new();
        off.insert("new_components".to_string(), false);
        assert_eq!(
            unknown_trigger_names(&off),
            vec!["new_components".to_string()]
        );
    }

    /// T-05: контракт опознаётся по СОДЕРЖИМОМУ и по каталогу, а не только по
    /// имени файла. Дельта с новым компонентом модели и правкой
    /// `docs/contracts/wallet-api.v1.yaml` (внутри `openapi: 3.0.3`, а в имени
    /// слова «openapi» нет) раньше оценивалась как Fast — спасал только
    /// храповик ROUTE.lock.
    #[test]
    fn diff_detector_sees_contracts_by_content_and_model_entities() {
        let dir = tempfile::tempdir().unwrap();
        let repo = git_repo(dir.path());
        write_file(&repo, "README.md", "# Кейс\n");
        git_in(&repo, &["add", "."]);
        git_in(&repo, &["commit", "-q", "-m", "baseline"]);

        // Дельта autotopup: новый компонент модели, правка контракта без
        // «openapi» в имени, новая интеграция.
        write_file(
            &repo,
            "model/CMP-008-autotopup.md",
            "---\nid: CMP-008\ntype: cmp\ntitle: Автопополнение\nstatus: designed\n---\n\nТело.\n",
        );
        write_file(
            &repo,
            "model/INT-004-autotopup-api.md",
            "---\nid: INT-004\ntype: int\ntitle: API автопополнения\nstatus: accepted\ncontract: docs/contracts/wallet-api.v1.yaml\n---\n\nТело.\n",
        );
        write_file(
            &repo,
            "docs/contracts/wallet-api.v1.yaml",
            "openapi: 3.0.3\ninfo:\n  title: Wallet API\n  version: 1.0.0\npaths: {}\n",
        );
        git_in(&repo, &["add", "."]);
        git_in(&repo, &["commit", "-q", "-m", "autotopup"]);

        let found = detect_diff_triggers(&repo, Some("HEAD~1")).unwrap();
        for t in [
            "new_component",
            "api_contract_change",
            "cross_domain_integration",
        ] {
            assert!(found.triggers.contains(t), "нет {t}: {:?}", found.triggers);
        }
        // Маршрут не ниже Standard (score 3 > fast_max 1).
        let scored = score_with_sources(
            &BTreeMap::new(),
            &found,
            DEFAULT_FAST_MAX,
            DEFAULT_STANDARD_MAX,
        );
        assert_eq!(scored.significance.route, Route::Standard);

        // Контракт, лежащий ВНЕ каталогов и БЕЗ ключа `openapi:` — не контракт
        // для детектора (иначе «любой .yaml» поднимал бы маршрут).
        let dir2 = tempfile::tempdir().unwrap();
        let repo2 = git_repo(dir2.path());
        write_file(&repo2, "config/app.yaml", "db: postgres://localhost/x\n");
        git_in(&repo2, &["add", "."]);
        git_in(&repo2, &["commit", "-q", "-m", "baseline"]);
        write_file(&repo2, "config/other.yaml", "key: value\n");
        git_in(&repo2, &["add", "."]);
        git_in(&repo2, &["commit", "-q", "-m", "правка конфига"]);
        let clean = detect_diff_triggers(&repo2, Some("HEAD~1")).unwrap();
        assert!(
            !clean.triggers.contains("api_contract_change"),
            "{:?}",
            clean.triggers
        );

        // Удаление контракта — тоже срабатывание: это ломающее изменение.
        std::fs::remove_file(repo.join("docs/contracts/wallet-api.v1.yaml")).unwrap();
        git_in(&repo, &["add", "-A"]);
        git_in(&repo, &["commit", "-q", "-m", "удаление контракта"]);
        let removed = detect_diff_triggers(&repo, Some("HEAD~1")).unwrap();
        assert!(
            removed
                .evidence
                .iter()
                .any(|e| e.starts_with("api_contract_change: удалён контракт")),
            "{:?}",
            removed.evidence
        );
    }

    /// T-05: глобы настраиваются — соглашение репозитория не зашито в бинарь.
    #[test]
    fn diff_detector_globs_are_configurable() {
        let dir = tempfile::tempdir().unwrap();
        let repo = git_repo(dir.path());
        write_file(&repo, "README.md", "# Кейс\n");
        git_in(&repo, &["add", "."]);
        git_in(&repo, &["commit", "-q", "-m", "baseline"]);
        write_file(&repo, "spec/wallet.proto", "syntax = \"proto3\";\n");
        write_file(&repo, "model/CMP-009.md", "---\nid: CMP-009\n---\n");
        git_in(&repo, &["add", "."]);
        git_in(&repo, &["commit", "-q", "-m", "delta"]);

        // `.proto` — контракт по расширению даже вне каталогов контрактов;
        // компонент модели виден по дефолтному глобу.
        let found = detect_diff_triggers(&repo, Some("HEAD~1")).unwrap();
        assert!(found.triggers.contains("api_contract_change"), "{found:?}");
        assert!(found.triggers.contains("new_component"), "{found:?}");

        // Свой глоб заменяет дефолтный: `model/**` вместо `model/CMP-*`.
        let globs = DiffGlobs {
            components: vec!["model/**".to_string()],
            ..DiffGlobs::default()
        };
        write_file(&repo, "model/REQ-002.md", "---\nid: REQ-002\n---\n");
        git_in(&repo, &["add", "."]);
        git_in(&repo, &["commit", "-q", "-m", "требование"]);
        let custom = detect_diff_triggers_with(&repo, Some("HEAD~1"), &globs).unwrap();
        assert!(custom.triggers.contains("new_component"), "{custom:?}");
        let default_globs = detect_diff_triggers(&repo, Some("HEAD~1")).unwrap();
        assert!(
            !default_globs.triggers.contains("new_component"),
            "дефолтный глоб REQ-* не покрывает: {default_globs:?}"
        );
    }

    /// D2: глобы кейса зажигают ранее недетектируемые триггеры —
    /// `security_boundary_change`, `trust_zone_change`, `data_contract_change`.
    /// Без настройки (`DiffGlobs::default()` — эти списки пусты) те же файлы
    /// триггеров не дают: глобальных эвристик не введено, поведение прежнее.
    #[test]
    fn case_globs_fire_formerly_blind_triggers() {
        let dir = tempfile::tempdir().unwrap();
        let repo = git_repo(dir.path());
        write_file(&repo, "auth/policy.yaml", "allow: all\n");
        write_file(&repo, "deploy/mesh/peer-authentication.yaml", "mtls: {}\n");
        write_file(&repo, "schemas/events/payment.yaml", "type: record\n");
        git_in(&repo, &["add", "."]);
        git_in(&repo, &["commit", "-q", "-m", "контур безопасности"]);

        let globs = DiffGlobs {
            security: vec!["auth/**".to_string()],
            trust_zone: vec!["deploy/mesh/**".to_string()],
            data_contract: vec!["schemas/events/**".to_string()],
            ..DiffGlobs::default()
        };
        let found = detect_diff_triggers_with(&repo, Some("HEAD~1"), &globs).unwrap();
        for t in [
            "security_boundary_change",
            "trust_zone_change",
            "data_contract_change",
        ] {
            assert!(found.triggers.contains(t), "нет {t}: {:?}", found.triggers);
        }
        let ev = found.evidence.join("\n");
        assert!(ev.contains("глоб кейса"), "{ev}");
        // security_boundary_change — форсирующий: маршрут Critical.
        let scored = score_with_sources(
            &BTreeMap::new(),
            &found,
            DEFAULT_FAST_MAX,
            DEFAULT_STANDARD_MAX,
        );
        assert_eq!(scored.significance.route, Route::Critical);

        // Дефолт: списки пусты — те же файлы слепой зоны не зажигают.
        let default_globs = DiffGlobs::default();
        assert_eq!(default_globs.security, Vec::<String>::new());
        assert_eq!(default_globs.trust_zone, Vec::<String>::new());
        assert_eq!(default_globs.data_contract, Vec::<String>::new());
        let plain = detect_diff_triggers(&repo, Some("HEAD~1")).unwrap();
        for t in [
            "security_boundary_change",
            "trust_zone_change",
            "data_contract_change",
        ] {
            assert!(
                !plain.triggers.contains(t),
                "без глобов {t} не зажигается: {:?}",
                plain.triggers
            );
        }
    }

    /// D2: зажигает ЛЮБАЯ правка файла под глобом — не только добавление:
    /// изменение и удаление auth-конфига значимы так же, как его появление.
    #[test]
    fn case_globs_fire_on_modification_and_deletion() {
        let dir = tempfile::tempdir().unwrap();
        let repo = git_repo(dir.path());
        write_file(&repo, "auth/policy.yaml", "allow: all\n");
        git_in(&repo, &["add", "."]);
        git_in(&repo, &["commit", "-q", "-m", "baseline auth"]);
        let globs = DiffGlobs {
            security: vec!["auth/**".to_string()],
            ..DiffGlobs::default()
        };

        write_file(&repo, "auth/policy.yaml", "allow: none\n");
        git_in(&repo, &["add", "."]);
        git_in(&repo, &["commit", "-q", "-m", "ужесточение политики"]);
        let modified = detect_diff_triggers_with(&repo, Some("HEAD~1"), &globs).unwrap();
        assert!(
            modified.triggers.contains("security_boundary_change"),
            "правка под глобом: {:?}",
            modified.triggers
        );
        let ev = modified.evidence.join("\n");
        assert!(ev.contains("изменён файл границы безопасности"), "{ev}");

        std::fs::remove_file(repo.join("auth/policy.yaml")).unwrap();
        git_in(&repo, &["add", "-A"]);
        git_in(&repo, &["commit", "-q", "-m", "удаление политики"]);
        let deleted = detect_diff_triggers_with(&repo, Some("HEAD~1"), &globs).unwrap();
        assert!(
            deleted.triggers.contains("security_boundary_change"),
            "удаление под глобом: {:?}",
            deleted.triggers
        );
        let ev = deleted.evidence.join("\n");
        assert!(ev.contains("удалён файл границы безопасности"), "{ev}");
    }

    #[test]
    fn diff_base_forms_are_equivalent() {
        let dir = tempfile::tempdir().unwrap();
        let repo = git_repo(dir.path());
        write_file(
            &repo,
            "billing/Cargo.toml",
            "[package]\nname = \"billing\"\n",
        );
        write_file(&repo, "billing/src/main.rs", "fn main() {}\n");
        git_in(&repo, &["add", "."]);
        git_in(&repo, &["commit", "-q", "-m", "feature"]);

        let bare = detect_diff_triggers(&repo, Some("HEAD~1")).unwrap();
        let range = detect_diff_triggers(&repo, Some("HEAD~1...HEAD")).unwrap();
        assert_eq!(bare.triggers, range.triggers, "формы базы разошлись");
        assert!(
            bare.triggers.contains("new_component"),
            "{:?}",
            bare.triggers
        );
        // Ни в одной форме в выводе не появляется двойной `...HEAD`.
        assert!(!bare.evidence.iter().any(|e| e.contains("HEAD...HEAD")));
        assert_eq!(normalize_base_range("origin/main"), "origin/main...HEAD");
        assert_eq!(
            normalize_base_range("origin/main...HEAD"),
            "origin/main...HEAD"
        );
        assert_eq!(normalize_base_range("HEAD~1..HEAD"), "HEAD~1..HEAD");
        assert_eq!(base_rev("origin/main...HEAD"), "origin/main");
        assert_eq!(base_rev("HEAD~1"), "HEAD~1");
    }

    #[test]
    fn diff_detector_worktree_mode_sees_untracked_new_component() {
        let dir = tempfile::tempdir().unwrap();
        let repo = git_repo(dir.path());
        // Без коммита: новый компонент — untracked-файлы рабочего дерева.
        write_file(&repo, "fraud/package.json", "{\"name\": \"fraud\"}\n");
        write_file(&repo, "fraud/src/index.js", "console.log(1);\n");
        let found = detect_diff_triggers(&repo, None).unwrap();
        assert!(
            found.triggers.contains("new_component"),
            "{:?}",
            found.triggers
        );
    }

    #[test]
    fn diff_detector_non_git_repo_is_control_error() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("plain");
        std::fs::create_dir_all(&repo).unwrap();
        let err = detect_diff_triggers(&repo, None).unwrap_err();
        assert!(err.to_string().contains("anti-bypass"), "{err}");
    }

    #[test]
    fn score_with_sources_unions_and_flags_undeclared() {
        let mut answers = BTreeMap::new();
        answers.insert("new_component".to_string(), true);
        answers.insert("new_vendor".to_string(), false);
        let mut diff = DiffTriggers::default();
        diff.fire("new_vendor", "зависимость в Cargo.toml: serde");
        diff.fire("new_component", "подтверждение диффом");

        let scored = score_with_sources(&answers, &diff, 1, 4);
        // Fail-safe: детектор добавил new_vendor поверх declared=false.
        assert_eq!(scored.significance.score, 2);
        assert_eq!(scored.significance.route, Route::Standard);
        assert_eq!(
            scored.sources["new_component"],
            TriggerSource::Both,
            "заявлен и подтверждён"
        );
        assert_eq!(scored.sources["new_vendor"], TriggerSource::Diff);
        // Расхождение — только new_vendor (заявлен false, найден диффом).
        assert_eq!(scored.undeclared, ["new_vendor"]);
        // Пороги параметризуются: тем же множеством при standard_max=1 — Critical.
        let strict = score_with_sources(&answers, &diff, 0, 1);
        assert_eq!(strict.significance.route, Route::Critical);
    }

    // --- S-2: пороги значимости (ADR-034) ---------------------------------------

    #[test]
    fn significance_with_limits_defaults_match_legacy_behavior() {
        // Дефолты 1/4 воспроизводят исторические маршруты 0–1/2–4/5+.
        for (count, want) in [
            (0, Route::Fast),
            (1, Route::Fast),
            (2, Route::Standard),
            (4, Route::Standard),
            (5, Route::Critical),
        ] {
            // T-04: имена обязаны быть каноническими — иначе в счёт не идут.
            let answers: BTreeMap<String, bool> = SIGNIFICANCE_TRIGGERS
                .iter()
                .take(count)
                .map(|t| ((*t).to_string(), true))
                .collect();
            assert_eq!(
                significance_score(&answers).route,
                significance_score_with_limits(&answers, DEFAULT_FAST_MAX, DEFAULT_STANDARD_MAX)
                    .route,
                "дефолты = старое поведение при score={count}"
            );
            let legacy = match count {
                0 | 1 => Route::Fast,
                2..=4 => Route::Standard,
                _ => Route::Critical,
            };
            assert_eq!(want, legacy, "таблица теста самосогласована");
            assert_eq!(significance_score(&answers).route, want);
        }
    }

    #[test]
    fn significance_with_limits_custom_thresholds_change_route() {
        // T-04: в счёт идут только канонические имена — тест порогов берёт их.
        let answers: BTreeMap<String, bool> = ["new_component", "new_vendor", "new_datastore"]
            .iter()
            .map(|t| ((*t).to_string(), true))
            .collect();
        // fast_max=2, standard_max=5: те же 3 триггера — Standard, не Fast.
        assert_eq!(
            significance_score_with_limits(&answers, 2, 5).route,
            Route::Standard
        );
        assert_eq!(
            significance_score_with_limits(&answers, 3, 5).route,
            Route::Fast
        );
        assert_eq!(
            significance_score_with_limits(&answers, 0, 2).route,
            Route::Critical
        );
    }

    #[test]
    fn significance_forcing_critical_triggers_not_configurable() {
        // security_boundary_change форсирует Critical при ЛЮБЫХ порогах
        // (fail-safe): «откалибровать вниз» его нельзя.
        let mut answers = BTreeMap::new();
        answers.insert("security_boundary_change".to_string(), true);
        assert_eq!(
            significance_score_with_limits(&answers, 10, 20).route,
            Route::Critical
        );
    }
}
