//! K2 (контур-бенч 0.4.0): redteam для пары «репозиторий + стандарт».
//!
//! Отличие от [`super::run`]: там мутируется архитектурный ПАКЕТ (кейс с
//! `model/`, `CONSTRAINTS.yaml`, бандлом) и вердикт даёт гейт; здесь
//! засеваются нарушения СТАНДАРТА (`CONSTRAINTS.yaml`) в РЕАЛЬНЫЙ чужой
//! репозиторий кода, а детектор — [контур контроля](crate::control::check)
//! (`arch-be control check <repo> --constraints <standard>`), исполняющий
//! ровно те типы правил, что зафиксированы в стандарте:
//!
//! - **текстовые** — `must_contain`/`must_not_contain`/`each_file_must_contain`/
//!   `dir_must_have_file`/`dependency_direction` (по разбору импортов);
//! - **`ArchUnit`** — правило `type: archunit` (мост к байткоду, ADR-0xx волны S);
//! - **радар** — `deny_dependency` (запрещённые пакеты в манифестах, §2.4).
//!
//! Карта обнаружения строится РАЗДЕЛЬНО по этим трём слоям — так видно, какой
//! слой стандарта слеп: обходы из §2.4 (wildcard, FQN-в-коде, static import,
//! Gradle, scope) ловятся/не ловятся разными слоями неодинаково.
//!
//! Честные границы:
//! - засев = ЧИСТАЯ ДОБАВКА строки в файл (импорт/литерал/зависимость), а не
//!   переписывание кода: детекция считается по ПРИРОСТУ находок относительно
//!   эталонного прогона на неизменённом репозитории (дельта), а не по
//!   абсолютному наличию — иначе предсуществующие нарушения репозитория
//!   засчитывались бы за засев;
//! - цель мутатора, которой в репозитории нет (нет доменного файла, нет
//!   yml-конфига…), — честный пропуск с причиной, а не «пойман»/«не пойман»:
//!   пропуск выходит из знаменателя доли;
//! - засев слоя `ArchUnit`, когда в стандарте НЕТ `type: archunit`, — пропуск
//!   «стандарт без archunit-правил; ловится только мостом» (пропуск, не
//!   провал): текст такие обходы не видит by design;
//! - seed с ожиданием «чисто» (закомментированный импорт, пароль в
//!   комментарии, test-scope) — проба ЛОЖНОГО срабатывания: попал в находки —
//!   ложное срабатывание слоя, а не «пойман»;
//! - прогон информационный: exit-код от доли обнаружения не зависит (порог —
//!   решение архитектора после калибровки), репозиторий read-only (работа идёт
//!   в копии во временном каталоге).

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use walkdir::WalkDir;

use super::Fixture;
use crate::error::{HarnessError, Result};

/// Слой стандарта, который ОБЯЗАН поймать засев (для раздельной карты).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Detector {
    /// Текстовые правила (`must_contain`/`must_not_contain`/
    /// `each_file_must_contain`/`dir_must_have_file`/`dependency_direction`).
    Text,
    /// Правило `type: archunit` (мост к байткоду).
    ArchUnit,
    /// `deny_dependency` (тех-радар по манифестам).
    Radar,
}

impl Detector {
    /// Метка слоя в отчёте.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Detector::Text => "текст",
            Detector::ArchUnit => "ArchUnit",
            Detector::Radar => "радар",
        }
    }
}

/// Что ожидается от засева.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeedExpect {
    /// Нарушение стандарта — обязано попасть в находки.
    Caught,
    /// Чистая правка — находок быть не должно (проба ложного срабатывания).
    Clean,
}

/// Итог одного засева.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SeedOutcome {
    /// Нарушение поймано (появилась новая находка ожидаемого правила).
    Caught,
    /// Нарушение не поймано никем из слоя (новых находок нет).
    Missed,
    /// Чистая правка не дала находок — как и ожидалось.
    Clean,
    /// Чистая правка дала находку — ложное срабатывание слоя.
    FalsePositive,
    /// Засев не состоялся (нет цели / слой неприменим к стандарту) — причина.
    Skipped(String),
}

/// Разобранный засев: конкретный файл и текст правки.
struct ResolvedSeed {
    id: String,
    title: String,
    group: String,
    detector: Detector,
    expect: SeedExpect,
    /// Имя правила стандарта, которым засев обязан быть пойман.
    rule: String,
    /// Относительный путь цели (для правки и отчёта).
    rel: String,
    /// Правка: `true` — вставка перед первой строкой, `false` — в конец.
    prepend: bool,
    /// Вставляемый текст.
    text: String,
}

/// Строка карты обнаружения (для отчёта/JSON).
#[derive(Debug, Clone)]
pub struct StdDetection {
    /// Идентификатор засева (`HX01`, `SE01`, …).
    pub id: String,
    /// Заголовок.
    pub title: String,
    /// Группа мутаторов.
    pub group: String,
    /// Слой стандарта, обязанный поймать.
    pub detector: Detector,
    /// Ожидание.
    pub expect: SeedExpect,
    /// Имя ожидаемого правила.
    pub rule: String,
    /// Целевой файл (относительный путь), если засев состоялся.
    pub file: Option<String>,
    /// Итог.
    pub outcome: SeedOutcome,
}

/// Доля обнаружения по одному слою.
#[derive(Debug, Clone, Copy, Default)]
struct LayerStat {
    caught: usize,
    total: usize,
}

impl LayerStat {
    fn ratio(self) -> f64 {
        if self.total == 0 {
            0.0
        } else {
            self.caught as f64 / self.total as f64
        }
    }
}

/// Сводка прогона «репозиторий + стандарт».
#[derive(Debug, Clone)]
pub struct StdReport {
    /// Репозиторий.
    pub repo: PathBuf,
    /// Стандарт (`CONSTRAINTS.yaml`).
    pub standard: PathBuf,
    /// Запрошенные группы мутаторов.
    pub groups: Vec<String>,
    /// Находок стандарта на НЕИЗМЕНЁННОМ репозитории (эталон; прозрачность).
    pub baseline_issues: usize,
    /// Число правил в стандарте.
    pub rules: usize,
    /// Есть ли в стандарте правило `type: archunit`.
    pub has_archunit: bool,
    /// Карта обнаружения.
    pub detections: Vec<StdDetection>,
}

impl StdReport {
    fn layer(&self, detector: Detector) -> LayerStat {
        let mut s = LayerStat::default();
        for d in &self.detections {
            if d.detector != detector || d.expect != SeedExpect::Caught {
                continue;
            }
            match d.outcome {
                SeedOutcome::Caught => {
                    s.total += 1;
                    s.caught += 1;
                }
                SeedOutcome::Missed => s.total += 1,
                // Пропуск/ложное срабатывание из доли обнаружения выходят.
                _ => {}
            }
        }
        s
    }

    /// Общая доля обнаружения (по всем слоям; пропуски вне знаменателя).
    #[must_use]
    pub fn detection_ratio(&self) -> f64 {
        let mut s = LayerStat::default();
        for det in [Detector::Text, Detector::ArchUnit, Detector::Radar] {
            let l = self.layer(det);
            s.caught += l.caught;
            s.total += l.total;
        }
        s.ratio()
    }

    /// Доля ложных срабатываний среди «чистых» проб.
    #[must_use]
    pub fn false_positive_ratio(&self) -> f64 {
        let clean: Vec<&StdDetection> = self
            .detections
            .iter()
            .filter(|d| d.expect == SeedExpect::Clean)
            .collect();
        if clean.is_empty() {
            return 0.0;
        }
        let fp = clean
            .iter()
            .filter(|d| d.outcome == SeedOutcome::FalsePositive)
            .count();
        fp as f64 / clean.len() as f64
    }

    /// Текстовый отчёт: шапка + карта по слоям + засевы.
    #[must_use]
    pub fn render(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(
            out,
            "Контур-бенч (K2): репозиторий {} · стандарт {}",
            self.repo.display(),
            self.standard.display()
        );
        let _ = writeln!(
            out,
            "Группы мутаторов: {} · правил в стандарте: {} · archunit-слой: {}",
            self.groups.join(", "),
            self.rules,
            if self.has_archunit {
                "есть"
            } else {
                "нет"
            }
        );
        let _ = writeln!(
            out,
            "Эталон (неизменённый репозиторий): {} находок стандарта — \
             они не считаются засевом (детекция по дельте)",
            self.baseline_issues
        );
        for det in [Detector::Text, Detector::ArchUnit, Detector::Radar] {
            let s = self.layer(det);
            if s.total == 0 {
                continue;
            }
            let _ = writeln!(
                out,
                "  слой {:<8}: {}/{} ({:.2})",
                det.label(),
                s.caught,
                s.total,
                s.ratio()
            );
        }
        let _ = writeln!(
            out,
            "Итог по слоям: доля обнаружения {:.2} · ложных срабатываний {:.2} \
             (информационно: порог не задан)",
            self.detection_ratio(),
            self.false_positive_ratio()
        );
        for d in &self.detections {
            let outcome = match &d.outcome {
                SeedOutcome::Caught => "пойман".to_string(),
                SeedOutcome::Missed => "НЕ пойман".to_string(),
                SeedOutcome::Clean => "чисто".to_string(),
                SeedOutcome::FalsePositive => "ЛОЖНОЕ СРАБАТЫВАНИЕ".to_string(),
                SeedOutcome::Skipped(r) => format!("пропущен: {r}"),
            };
            let _ = writeln!(
                out,
                "  [{}] {} · {} · {} · {}",
                d.id,
                d.title,
                d.detector.label(),
                d.file.as_deref().unwrap_or("—"),
                outcome
            );
        }
        out
    }

    /// Отчёт JSON (схема `arch-be/stdcomply/v1`) `serde_json::Value`.
    #[must_use]
    pub fn to_json(&self) -> serde_json::Value {
        let layer_json = |det: Detector| {
            let s = self.layer(det);
            serde_json::json!({ "caught": s.caught, "total": s.total, "ratio": s.ratio() })
        };
        let seeds: Vec<serde_json::Value> = self
            .detections
            .iter()
            .map(|d| {
                serde_json::json!({
                    "id": d.id,
                    "title": d.title,
                    "group": d.group,
                    "detector": d.detector.label(),
                    "expect": match d.expect { SeedExpect::Caught => "caught", SeedExpect::Clean => "clean" },
                    "rule": d.rule,
                    "file": d.file,
                    "outcome": match &d.outcome {
                        SeedOutcome::Caught => "caught",
                        SeedOutcome::Missed => "missed",
                        SeedOutcome::Clean => "clean",
                        SeedOutcome::FalsePositive => "false_positive",
                        SeedOutcome::Skipped(_) => "skipped",
                    },
                    "skip_reason": match &d.outcome { SeedOutcome::Skipped(r) => Some(r.clone()), _ => None },
                })
            })
            .collect();
        serde_json::json!({
            "schema": "arch-be/stdcomply/v1",
            "repo": self.repo.display().to_string(),
            "standard": self.standard.display().to_string(),
            "groups": self.groups,
            "rules": self.rules,
            "has_archunit": self.has_archunit,
            "baseline_issues": self.baseline_issues,
            "detection": {
                "text": layer_json(Detector::Text),
                "archunit": layer_json(Detector::ArchUnit),
                "radar": layer_json(Detector::Radar),
                "overall_ratio": self.detection_ratio(),
            },
            "false_positive_ratio": self.false_positive_ratio(),
            "seeds": seeds,
            "note": "информационный прогон K2: доля обнаружения не применяет порог",
        })
    }
}

/// Разобранный стандарт: имена правил и наличие archunit-слоя.
#[derive(Debug, Deserialize)]
struct StdRaw {
    #[serde(default)]
    rules: Vec<serde_yaml_ng::Value>,
}

/// Читает стандарт и считает правила/archunit-слой.
fn load_standard(path: &Path) -> Result<(usize, bool)> {
    let text = std::fs::read_to_string(path).map_err(|e| HarnessError::io(path, e))?;
    let raw: StdRaw = serde_yaml_ng::from_str(&text)
        .map_err(|e| HarnessError::Control(format!("стандарт {}: {e}", path.display())))?;
    let rules = raw.rules.len();
    let mut has_archunit = false;
    for r in &raw.rules {
        if r.get("type").and_then(serde_yaml_ng::Value::as_str) == Some("archunit") {
            has_archunit = true;
        }
    }
    Ok((rules, has_archunit))
}

/// Найденные цели засева в репозитории (относительные пути, детерминированно).
struct Targets {
    domain: Option<String>,
    web_adapter: Option<String>,
    yml: Option<String>,
    pom: Option<String>,
    gradle: Option<String>,
    base_package: Option<String>,
}

/// Разведка целей: обход копии репозитория без `.git`, первые совпадения по
/// сортировке путей (детерминизм).
fn discover(root: &Path) -> Targets {
    let mut java_domain: Vec<String> = Vec::new();
    let mut java_web: Vec<String> = Vec::new();
    let mut ymls: Vec<String> = Vec::new();
    let mut poms: Vec<String> = Vec::new();
    let mut gradles: Vec<String> = Vec::new();
    for entry in WalkDir::new(root)
        .into_iter()
        .filter_map(std::result::Result::ok)
    {
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        let rel = path.strip_prefix(root).unwrap_or(path);
        let rel = rel.to_string_lossy().replace('\\', "/");
        if rel.starts_with(".git/") || rel.contains("/.git/") {
            continue;
        }
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if ext == "java" {
            if rel.contains("/application/domain/") {
                java_domain.push(rel.clone());
            } else if rel.contains("/adapter/in/") {
                java_web.push(rel.clone());
            }
        } else if (ext == "yml" || ext == "yaml") && name.starts_with("application") {
            ymls.push(rel.clone());
        }
        if name == "pom.xml" {
            poms.push(rel.clone());
        }
        if name == "build.gradle" || name == "build.gradle.kts" {
            gradles.push(rel.clone());
        }
    }
    for v in [
        &mut java_domain,
        &mut java_web,
        &mut ymls,
        &mut poms,
        &mut gradles,
    ] {
        v.sort();
    }
    let base_package = java_domain.first().and_then(|p| base_package(p));
    Targets {
        domain: java_domain.into_iter().next(),
        web_adapter: java_web.into_iter().next(),
        yml: ymls.into_iter().next(),
        pom: poms.into_iter().next(),
        gradle: gradles.into_iter().next(),
        base_package,
    }
}

/// Базовый пакет из пути доменного файла: сегменты между `java` и `domain`,
/// с отброшенным хвостом `application` (слой гексагона, не часть пакета).
fn base_package(rel: &str) -> Option<String> {
    let segs: Vec<&str> = rel.split('/').collect();
    let java = segs.iter().position(|s| *s == "java")?;
    let domain = segs.iter().position(|s| *s == "domain")?;
    if domain <= java + 1 {
        return None;
    }
    let mut base: Vec<&str> = segs[java + 1..domain].to_vec();
    if base.last() == Some(&"application") {
        base.pop();
    }
    if base.is_empty() {
        None
    } else {
        Some(base.join("."))
    }
}

/// Известные группы мутаторов.
const KNOWN_GROUPS: [&str; 4] = ["java-hex", "secrets", "radar", "config"];

/// Собирает разобранные засевы по группам из доступных в репозитории целей.
fn catalog(t: &Targets, groups: &[String]) -> Vec<ResolvedSeed> {
    let want = |g: &str| groups.iter().any(|x| x == g);
    let mut seeds: Vec<ResolvedSeed> = Vec::new();
    let mut push = |id: &str,
                    title: &str,
                    group: &str,
                    detector: Detector,
                    expect: SeedExpect,
                    rule: &str,
                    rel: &str,
                    prepend: bool,
                    text: String| {
        seeds.push(ResolvedSeed {
            id: id.to_string(),
            title: title.to_string(),
            group: group.to_string(),
            detector,
            expect,
            rule: rule.to_string(),
            rel: rel.to_string(),
            prepend,
            text,
        });
    };

    // --- java-hex: зависимости домена и входящего адаптера ---
    if want("java-hex") {
        if let Some(rel) = &t.domain {
            // HX01: прямой импорт фреймворка в домен (JSS-006, текст).
            push(
                "HX01",
                "импорт фреймворка в домен",
                "java-hex",
                Detector::Text,
                SeedExpect::Caught,
                "s6_domain_independent",
                rel,
                true,
                "import org.springframework.stereotype.Component;".to_string(),
            );
            // HX02: FQN-тип в теле без импорта — текст слеп, ловит только мост.
            push(
                "HX02",
                "FQN-тип в домене без импорта",
                "java-hex",
                Detector::ArchUnit,
                SeedExpect::Caught,
                "archunit:domain-fqn",
                rel,
                false,
                "\n    private org.springframework.stereotype.Component fqnSpring;\n".to_string(),
            );
            // HX03: wildcard-импорт фреймворка (§2.4) — текст ловит.
            push(
                "HX03",
                "wildcard-импорт фреймворка в домен",
                "java-hex",
                Detector::Text,
                SeedExpect::Caught,
                "s6_domain_independent",
                rel,
                true,
                "import org.springframework.*;".to_string(),
            );
            // HX04: static import (§2.4) — текст ловит.
            push(
                "HX04",
                "static import фреймворка в домен",
                "java-hex",
                Detector::Text,
                SeedExpect::Caught,
                "s6_domain_independent",
                rel,
                true,
                "import static org.springframework.util.Assert.notNull;".to_string(),
            );
            // HX05: jakarta.persistence в домене — текст ловит.
            push(
                "HX05",
                "jakarta.persistence в домене",
                "java-hex",
                Detector::Text,
                SeedExpect::Caught,
                "s6_domain_independent",
                rel,
                true,
                "import jakarta.persistence.Entity;".to_string(),
            );
            // HX06: spring-web в домене — текст ловит.
            push(
                "HX06",
                "spring-web в домене",
                "java-hex",
                Detector::Text,
                SeedExpect::Caught,
                "s6_domain_independent",
                rel,
                true,
                "import org.springframework.web.bind.annotation.RestController;".to_string(),
            );
        }
        if let (Some(rel), Some(base)) = (&t.web_adapter, &t.base_package) {
            // HX07: прямой импорт исходящего адаптера во входящем (JSS-007, текст).
            push(
                "HX07",
                "исходящий адаптер во входящем",
                "java-hex",
                Detector::Text,
                SeedExpect::Caught,
                "s7_inbound_no_outbound",
                rel,
                true,
                format!("import {base}.adapter.out.PersistenceAdapter;"),
            );
            // HX08: `new <FQN>` входящего адаптера на исходящий — только мост.
            push(
                "HX08",
                "new FQN исходящего адаптера во входящем",
                "java-hex",
                Detector::ArchUnit,
                SeedExpect::Caught,
                "archunit:inbound-new-out",
                rel,
                false,
                format!("\n    Object fqnOut = new {base}.adapter.out.PersistenceAdapter();\n"),
            );
        }
        if let Some(rel) = &t.web_adapter {
            // HX09: `.class`-ссылка на тип мимо импорта — только мост.
            push(
                "HX09",
                "FxClient.class без импорта",
                "java-hex",
                Detector::ArchUnit,
                SeedExpect::Caught,
                "archunit:class-ref",
                rel,
                false,
                "\n    Class<?> fqnClass = org.springframework.stereotype.Component.class;\n"
                    .to_string(),
            );
        }
    }

    // --- secrets: литерал секрета в конфиге ---
    if want("secrets") {
        if let Some(rel) = &t.yml {
            push(
                "SE01",
                "пароль литералом в конфиге",
                "secrets",
                Detector::Text,
                SeedExpect::Caught,
                "s8_no_literal_password",
                rel,
                false,
                "  password: secret123\n".to_string(),
            );
        }
    }

    // --- radar: запрещённые пакеты в манифестах ---
    if want("radar") {
        let pom_rel = t.pom.clone().unwrap_or_else(|| "pom.xml".to_string());
        let pom_text = |artifact: &str, scope: &str| {
            format!(
                "\n<dependency><groupId>com.example</groupId><artifactId>{artifact}</artifactId>\
                 <scope>{scope}</scope></dependency>\n"
            )
        };
        push(
            "RD01",
            "h2 в pom (runtime)",
            "radar",
            Detector::Radar,
            SeedExpect::Caught,
            "s1_no_embedded_db_in_prod",
            &pom_rel,
            false,
            pom_text("h2", "runtime"),
        );
        push(
            "RD02",
            "hsqldb в pom (runtime)",
            "radar",
            Detector::Radar,
            SeedExpect::Caught,
            "s1_no_embedded_db_in_prod",
            &pom_rel,
            false,
            pom_text("hsqldb", "runtime"),
        );
        // RD05: h2 в test-scope — радар §2.4 слеп к scope (проба ложного срабатывания).
        push(
            "RD05",
            "h2 в pom (test-scope) — радар слеп к scope",
            "radar",
            Detector::Radar,
            SeedExpect::Clean,
            "s1_no_embedded_db_in_prod",
            &pom_rel,
            false,
            pom_text("h2", "test"),
        );
        // RD03/RD04: Gradle — радар не разбирает build.gradle (§2.4, обход).
        let gradle_rel = t
            .gradle
            .clone()
            .unwrap_or_else(|| "build.gradle".to_string());
        push(
            "RD03",
            "h2 в build.gradle (обход радара)",
            "radar",
            Detector::Radar,
            SeedExpect::Caught,
            "s1_no_embedded_db_in_prod",
            &gradle_rel,
            false,
            "\nimplementation 'com.h2database:h2:2.2.224'\n".to_string(),
        );
        push(
            "RD04",
            "hsqldb в build.gradle (обход радара)",
            "radar",
            Detector::Radar,
            SeedExpect::Caught,
            "s1_no_embedded_db_in_prod",
            &gradle_rel,
            false,
            "\nimplementation 'org.hsqldb:hsqldb:2.7.2'\n".to_string(),
        );
    }

    // --- config: чистые правки — проба ложных срабатываний ---
    if want("config") {
        if let Some(rel) = &t.domain {
            push(
                "CF01",
                "закомментированный импорт в домене",
                "config",
                Detector::Text,
                SeedExpect::Clean,
                "",
                rel,
                true,
                "// import org.springframework.stereotype.Component;".to_string(),
            );
        }
        if let Some(rel) = &t.yml {
            push(
                "CF02",
                "пароль в комментарии yml",
                "config",
                Detector::Text,
                SeedExpect::Clean,
                "",
                rel,
                false,
                "# password: secret123\n".to_string(),
            );
        }
    }

    // Пометка «слой ArchUnit неприменим к стандарту» делается на этапе прогона
    // (в `run_stdcomply`): здесь карта строится только по доступным целям.
    seeds
}

/// Карта «правило × файл → число находок» для дельта-сравнения (устойчива к
/// сдвигу номеров строк при вставке).
fn issue_map(report: &crate::control::FitnessReport) -> BTreeMap<(String, String), usize> {
    let mut out: BTreeMap<(String, String), usize> = BTreeMap::new();
    for issue in &report.issues {
        let key = (issue.rule.clone(), issue.file.to_string_lossy().to_string());
        *out.entry(key).or_insert(0) += 1;
    }
    out
}

/// Новые правила, появившиеся относительно эталона (прирост счётчика).
fn new_rules(
    before: &BTreeMap<(String, String), usize>,
    after: &BTreeMap<(String, String), usize>,
) -> Vec<String> {
    let mut rules: Vec<String> = Vec::new();
    for (key, count) in after {
        let prev = before.get(key).copied().unwrap_or(0);
        if *count > prev {
            rules.push(key.0.clone());
        }
    }
    rules.sort();
    rules.dedup();
    rules
}

/// Прогон K2: «репозиторий + стандарт» → карта обнаружения по слоям.
///
/// # Errors
/// Репозиторий/стандарт недоступны, стандарт не разбирается как YAML,
/// неизвестная группа мутаторов, копирование репозитория или исполнение
/// контроля отказали.
pub fn run_stdcomply(repo: &Path, standard: &Path, groups: &[String]) -> Result<StdReport> {
    if !repo.is_dir() {
        return Err(HarnessError::Control(format!(
            "репозиторий недоступен: {}",
            repo.display()
        )));
    }
    if !standard.is_file() {
        return Err(HarnessError::Control(format!(
            "стандарт недоступен: {}",
            standard.display()
        )));
    }
    // Группы: пустой запрос → все; незнакомая — ошибка (не молчание).
    let groups: Vec<String> = if groups.is_empty() {
        KNOWN_GROUPS.iter().map(|s| (*s).to_string()).collect()
    } else {
        groups.to_vec()
    };
    for g in &groups {
        if !KNOWN_GROUPS.contains(&g.as_str()) {
            return Err(HarnessError::Control(format!(
                "неизвестная группа мутаторов `{g}` (доступны: {})",
                KNOWN_GROUPS.join(", ")
            )));
        }
    }

    let (rules, has_archunit) = load_standard(standard)?;
    // Одна копия репозитория на весь прогон (read-only к исходнику), .git
    // пропускается — правки идут по файлам рабочей копии и откатываются.
    let fixture = Fixture::new(repo)?;
    let work = fixture.mutant("work")?;

    let baseline = crate::control::check(&work, standard)?;
    let before = issue_map(&baseline);
    let baseline_issues = baseline.issues.len();

    let targets = discover(&work);
    let seeds = catalog(&targets, &groups);

    let mut detections: Vec<StdDetection> = Vec::new();
    for seed in &seeds {
        let abs = work.join(&seed.rel);
        let outcome = if !abs.is_file() {
            SeedOutcome::Skipped(format!("цель не найдена в репозитории: {}", seed.rel))
        } else if seed.detector == Detector::ArchUnit && !has_archunit {
            SeedOutcome::Skipped("стандарт без archunit-правил; ловится только мостом".to_string())
        } else {
            let original = std::fs::read(&abs).map_err(|e| HarnessError::io(&abs, e))?;
            apply(&abs, seed)?;
            let after_report = crate::control::check(&work, standard)?;
            let after = issue_map(&after_report);
            let new = new_rules(&before, &after);
            // Откат правки до вердикта о результате — независимо от исхода.
            std::fs::write(&abs, &original).map_err(|e| HarnessError::io(&abs, e))?;
            if seed.expect == SeedExpect::Clean {
                if new.is_empty() {
                    SeedOutcome::Clean
                } else {
                    SeedOutcome::FalsePositive
                }
            } else if new.contains(&seed.rule) {
                SeedOutcome::Caught
            } else {
                SeedOutcome::Missed
            }
        };
        detections.push(StdDetection {
            id: seed.id.clone(),
            title: seed.title.clone(),
            group: seed.group.clone(),
            detector: seed.detector,
            expect: seed.expect,
            rule: seed.rule.clone(),
            file: Some(seed.rel.clone()),
            outcome,
        });
    }

    Ok(StdReport {
        repo: repo.to_path_buf(),
        standard: standard.to_path_buf(),
        groups,
        baseline_issues,
        rules,
        has_archunit,
        detections,
    })
}

/// Применяет правку засева к файлу (вставка строки в начало или в конец).
///
/// Хвостовой засев гарантирует перевод строки перед собой: реальные файлы
/// часто не заканчиваются `\n`, и без разделителя засев приклеился бы к
/// последней строке — строчное правило (напр. `must_not_contain` с `^`) его
/// не увидело бы, и дефект «не пойман» был бы артефактом вставки, а не слепотой
/// слоя.
fn apply(abs: &Path, seed: &ResolvedSeed) -> Result<()> {
    let original = std::fs::read_to_string(abs).map_err(|e| HarnessError::io(abs, e))?;
    let text = if seed.prepend {
        format!("{}\n{original}", seed.text)
    } else {
        let sep = if original.is_empty() || original.ends_with('\n') {
            ""
        } else {
            "\n"
        };
        format!("{original}{sep}{}", seed.text)
    };
    std::fs::write(abs, text).map_err(|e| HarnessError::io(abs, e))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Стандарт «Java-сервисы» из репозитория (cwd теста — корень крейта).
    fn standard() -> PathBuf {
        PathBuf::from("benchmarks/architect-value/standard/CONSTRAINTS.java-services.yaml")
    }

    /// Синтетический гексагональный репозиторий под стандарт: домен,
    /// входящий адаптер, yml-конфиг, pom.
    fn synthetic_repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let write = |rel: &str, text: &str| {
            let p = root.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, text).unwrap();
        };
        write(
            "src/main/java/com/example/app/application/domain/user/User.java",
            "package com.example.app.application.domain.user;\n\npublic class User {\n}\n",
        );
        write(
            "src/main/java/com/example/app/adapter/in/web/UserController.java",
            "package com.example.app.adapter.in.web;\n\npublic class UserController {\n}\n",
        );
        write(
            "src/main/resources/application.yml",
            "server:\n  port: 8080\n",
        );
        write(
            "pom.xml",
            "<project><modelVersion>4.0.0</modelVersion></project>\n",
        );
        write("build.gradle", "dependencies {\n}\n");
        dir
    }

    fn find<'a>(report: &'a StdReport, id: &str) -> &'a StdDetection {
        report
            .detections
            .iter()
            .find(|d| d.id == id)
            .unwrap_or_else(|| panic!("нет засева {id}"))
    }

    #[test]
    fn base_package_derivation() {
        assert_eq!(
            base_package("src/main/java/com/example/app/application/domain/user/User.java"),
            Some("com.example.app".to_string())
        );
        assert_eq!(base_package("User.java"), None);
    }

    /// Хвостовой засев ставится на отдельную строку, даже если исходный файл не
    /// заканчивался переводом строки (иначе строчное `^`-правило его не увидит).
    #[test]
    fn tail_seed_starts_a_new_line_without_trailing_newline() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("application.yml");
        std::fs::write(&file, "buckpal:\n  transferThreshold: 10000").unwrap();
        let seed = ResolvedSeed {
            id: "SE01".into(),
            title: "пароль".into(),
            group: "secrets".into(),
            detector: Detector::Text,
            expect: SeedExpect::Caught,
            rule: "s8_no_literal_password".into(),
            rel: "application.yml".into(),
            prepend: false,
            text: "  password: secret123\n".into(),
        };
        apply(&file, &seed).expect("apply");
        let content = std::fs::read_to_string(&file).unwrap();
        assert!(
            content
                .lines()
                .any(|l| l.trim_start().starts_with("password:")),
            "засев не на своей строке: {content:?}"
        );
    }

    #[test]
    fn stdcomply_maps_detection_by_layer() {
        let dir = synthetic_repo();
        let groups: Vec<String> = KNOWN_GROUPS.iter().map(|s| (*s).to_string()).collect();
        let report = run_stdcomply(dir.path(), &standard(), &groups).expect("run");
        // Текстовые обходы домена ловятся.
        assert_eq!(find(&report, "HX01").outcome, SeedOutcome::Caught);
        assert_eq!(find(&report, "HX03").outcome, SeedOutcome::Caught);
        assert_eq!(find(&report, "HX04").outcome, SeedOutcome::Caught);
        assert_eq!(find(&report, "HX06").outcome, SeedOutcome::Caught);
        // Исходящий адаптер во входящем — JSS-007 (специфичен для buckpal-пакета;
        // базовый пакет синтетики — com.example.app, правило не срабатывает).
        assert_eq!(find(&report, "HX07").outcome, SeedOutcome::Missed);
        // FQN-обходы без импорта — только мост ArchUnit; в этом стандарте нет.
        assert!(matches!(
            find(&report, "HX02").outcome,
            SeedOutcome::Skipped(_)
        ));
        // Секрет литералом ловится, пароль в комментарии — чисто.
        assert_eq!(find(&report, "SE01").outcome, SeedOutcome::Caught);
        assert_eq!(find(&report, "CF02").outcome, SeedOutcome::Clean);
        // Закомментированный импорт — чисто.
        assert_eq!(find(&report, "CF01").outcome, SeedOutcome::Clean);
        // Радар: h2/hsqldb в pom ловятся; Gradle — обход; test-scope — ложное.
        assert_eq!(find(&report, "RD01").outcome, SeedOutcome::Caught);
        assert_eq!(find(&report, "RD02").outcome, SeedOutcome::Caught);
        assert_eq!(find(&report, "RD03").outcome, SeedOutcome::Missed);
        assert_eq!(find(&report, "RD05").outcome, SeedOutcome::FalsePositive);
        // Доля обнаружения и доля ложных срабатываний считаются.
        assert!(report.detection_ratio() > 0.0);
        assert!(report.false_positive_ratio() > 0.0);
    }

    #[test]
    fn stdcomply_rejects_unknown_group() {
        let dir = synthetic_repo();
        let err = run_stdcomply(dir.path(), &standard(), &["nope".to_string()]).unwrap_err();
        assert!(format!("{err}").contains("неизвестная группа"));
    }

    #[test]
    fn stdcomply_group_filter_selects_only_radar() {
        let dir = synthetic_repo();
        let report = run_stdcomply(dir.path(), &standard(), &["radar".to_string()]).expect("run");
        assert!(report.detections.iter().all(|d| d.group == "radar"));
        assert_eq!(find(&report, "RD01").outcome, SeedOutcome::Caught);
    }
}
