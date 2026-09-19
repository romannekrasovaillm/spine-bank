//! Первый зелёный за 15 минут (W3): `arch-be bootstrap`.
//!
//! Исследование дошло от пустого каталога до зелёного Critical примерно за
//! 50 ручных шагов, и почти каждый шаг — «чего ещё не хватает». Проводник
//! делает две вещи: создаёт каркас кейса и после каждого шага называет
//! СЛЕДУЮЩУЮ красную находку с подсказкой, что с ней делать.
//!
//! Каркас намеренно красный. Заглушки бандла написаны так, чтобы их ловила
//! семантика Н1 (`evidence_stub`): проводник, производящий ложнозелёные
//! пакеты, хуже отсутствия проводника — он выдаёт «выпуск разрешён» за
//! состояние «ничего не написано». Тест
//! `bootstrap_stub_skeleton_is_caught_by_evidence_semantics` держит эту
//! границу.
//!
//! Проводник ничего не решает за человека: он не подписывает A3, не пишет
//! решение вместо архитектора и не включает составляющие гейта. Он называет
//! работу и её порядок.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use crate::config::Config;
use crate::control::Route;
use crate::error::{HarnessError, Result};
use crate::gate::{self, GateReport, GateStatus};

/// Стадия «дорожки до зелёного»: человеческое имя и итог по составляющим.
#[derive(Debug, Clone)]
pub struct Stage {
    /// Ключ стадии (`spine`, `rules`, `model`, `bundle`).
    pub key: &'static str,
    /// Человеческое имя для строки прогресса.
    pub title: &'static str,
    /// Метка: `✓`, `✗` либо `7/13` у бандла.
    pub mark: String,
    /// Число находок `error` (для `✗ (N находок)`).
    pub findings: usize,
    /// Пояснение одной строкой (что именно не так).
    pub detail: String,
}

/// Прогресс кейса: четыре стадии и следующий шаг.
#[derive(Debug, Clone)]
pub struct Progress {
    /// Каталог кейса.
    pub dir: PathBuf,
    /// Маршрут, на котором считался прогресс.
    pub route: String,
    /// Стадии в порядке прохождения.
    pub stages: Vec<Stage>,
    /// Следующая красная находка — либо `None`, если зелено.
    pub next: Option<NextStep>,
    /// Итог гейта (`PASS` | `FAIL` | `INCOMPLETE`).
    pub outcome: String,
    /// Что сделал проводник (у `--status` — пусто): «каркас стал git-репозиторием
    /// с одним коммитом» и т.п. Печатается рядом с прогрессом, потому что это
    /// факты о кейсе, а не о вердикте.
    pub notes: Vec<String>,
    /// Аттестация вердикта (для сверки после правок).
    pub attestation: String,
}

/// Следующий шаг: что красное и что с ним делать.
#[derive(Debug, Clone)]
pub struct NextStep {
    /// Стадия, к которой относится шаг.
    pub stage: String,
    /// Составляющая гейта.
    pub component: String,
    /// Текст находки.
    pub problem: String,
    /// Что сделать.
    pub fix_hint: String,
}

impl Progress {
    /// Строка прогресса: `спайн ✓ · правила ✓ · модель ✗ (2 находки) · бандл 7/13`.
    #[must_use]
    pub fn line(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        for s in &self.stages {
            let findings = if s.findings == 0 {
                String::new()
            } else {
                format!(" ({})", plural_findings(s.findings))
            };
            parts.push(format!("{} {}{findings}", s.title, s.mark));
        }
        parts.join(" · ")
    }

    /// Готов ли кейс: все стадии пройдены и следующий шаг отсутствует.
    #[must_use]
    pub fn is_green(&self) -> bool {
        self.outcome == "PASS"
    }
}

impl NextStep {
    /// Печатная форма шага: находка, компонент и подсказка.
    #[must_use]
    pub fn render(&self) -> String {
        format!(
            "Следующий шаг — {} ({})\n  {}\n  → {}",
            self.stage, self.component, self.problem, self.fix_hint
        )
    }
}

/// Стадии «дорожки» и составляющие, по которым считается каждая.
const STAGES: [(&str, &str, &[&str]); 4] = [
    ("spine", "спайн", &["spine_lint"]),
    ("rules", "правила", &["fitness"]),
    ("model", "модель", &["model_validate", "trace_check"]),
    ("bundle", "бандл", &["evidence_verify"]),
];

/// «1 находка · 2 находки · 5 находок» — строка прогресса читается человеком,
/// а «2 находок» режет глаз ровно там, где проводник должен выглядеть
/// аккуратно.
fn plural_findings(n: usize) -> String {
    let tail = n % 100;
    let form = if (11..=14).contains(&tail) {
        "находок"
    } else {
        match n % 10 {
            1 => "находка",
            2..=4 => "находки",
            _ => "находок",
        }
    };
    format!("{n} {form}")
}

/// Что делать с находкой составляющей — по коду составляющей, а не по тексту
/// находки (текст меняется, причина — нет).
fn stage_fix_hint(component: &str) -> &'static str {
    match component {
        "spine_lint" => {
            "допишите инвариант: `Binds` (что связывает), `Prevents` (что \
             предотвращает) и `Rule` (как проверяется) — пустые поля линтер \
             считает недописанным решением"
        }
        "fitness" => {
            "приведите репозиторий в соответствие с правилом либо уточните \
             само правило в CONSTRAINTS.yaml (`arch-be control check .`)"
        }
        "delta_guard" => {
            "опишите правку защищённого пути в changes/<имя>/DELTA.md \
             (`arch-be delta new <имя>`)"
        }
        "rule_weakened" => {
            "верните severity правила либо оформите отступление: ослабление \
             реестра без ADR — дефект, а не сокращение"
        }
        "model_validate" => {
            "исправьте ссылки в model/ или создайте недостающие сущности \
             (`arch-be model validate model`)"
        }
        "trace_check" => {
            "свяжите звено: REQ → NFR → AD/ADR → CMP, у каждого AD — правило \
             в `verified_by` (`arch-be trace check .`)"
        }
        "sensors" => {
            "допишите спецификации в docs/spec: обязательны разделы \
             «## Проблема», «## Критерии приёмки», «## Риски»"
        }
        "nfr" => {
            "поправьте числа в model/: бюджет hop'ов против цели p99, \
             доступность против SLA, ёмкость против RPS (`arch-be nfr check .`)"
        }
        "evidence_verify" => {
            "заполните артефакты бандла содержанием и переупакуйте: \
             `arch-be evidence pack . --route critical`"
        }
        "decision_quality" => {
            "прогоните рубрику судьёй, отличным от автора, и положите отчёт в \
             reports/rubric/ (`arch-be rubric run`)"
        }
        "route_lock" => {
            "согласуйте заявленный маршрут с его изменением: понижение без \
             `decided_by: ADR-…` — дефект (`.arch-handoff/ROUTE.lock`)"
        }
        _ => "разберите находку и повторите прогон гейта",
    }
}

/// Прогоняет гейт на маршруте по умолчанию (auto — с учётом `ROUTE.lock`) и
/// печатает прогресс.
///
/// # Errors
/// Репозиторий недоступен либо конфигурация маршрутов невалидна.
pub fn status(dir: &Path, cfg: &Config) -> Result<Progress> {
    let limits = cfg
        .significance
        .limits()
        .map_err(|e| HarnessError::Config(format!("маршруты значимости: {e}")))?;
    let report = gate::run_opts(
        dir,
        None,
        None,
        None,
        limits,
        &gate::GateRequirements::from_config(&cfg.gate),
        &gate::GateOptions::from_config(cfg),
    )?;
    Ok(progress(dir, &report, cfg))
}

/// Собирает прогресс по готовому отчёту гейта.
fn progress(dir: &Path, report: &GateReport, cfg: &Config) -> Progress {
    let stages: Vec<Stage> = STAGES
        .iter()
        .map(|(key, title, components)| {
            if *key == "bundle" {
                return bundle_stage(dir, report, title);
            }
            let mut findings = 0usize;
            let mut failing: Option<String> = None;
            for name in *components {
                let Some(c) = report.components.iter().find(|c| c.name == *name) else {
                    continue;
                };
                match c.status {
                    GateStatus::Pass => {}
                    GateStatus::Fail => {
                        findings += c.findings.iter().filter(|f| f.severity == "error").count();
                        failing.get_or_insert_with(|| c.detail.clone());
                    }
                    GateStatus::Skip => {
                        failing.get_or_insert_with(|| format!("нет входа: {}", c.detail));
                    }
                }
            }
            Stage {
                key,
                title,
                mark: if failing.is_none() { "✓" } else { "✗" }.to_string(),
                findings,
                detail: failing.unwrap_or_else(|| "готово".to_string()),
            }
        })
        .collect();
    Progress {
        dir: dir.to_path_buf(),
        route: report.route.to_string(),
        stages,
        next: next_step(dir, report, cfg),
        outcome: report.outcome.label().to_string(),
        notes: Vec::new(),
        attestation: report.attestation.clone(),
    }
}

/// Стадия бандла: сколько обязательных артефактов профиля маршрута собрано
/// (`7/13`), а не «есть EVIDENCE.yaml или нет».
fn bundle_stage(dir: &Path, report: &GateReport, title: &'static str) -> Stage {
    let component = report
        .components
        .iter()
        .find(|c| c.name == "evidence_verify");
    let Some(bundle_dir) = bundle_dir(dir) else {
        return Stage {
            key: "bundle",
            title,
            mark: "0/13".to_string(),
            findings: 0,
            detail: "нет EVIDENCE.yaml — бандл не собран (`arch-be evidence pack .`)".to_string(),
        };
    };
    let total = crate::evidence::required_artifacts(report.route).len();
    let present = crate::evidence::bundle_progress(&bundle_dir, report.route).unwrap_or(0);
    let detail = component.map_or_else(|| "бандл собран".to_string(), |c| c.detail.clone());
    let findings = component.map_or(0, |c| {
        c.findings.iter().filter(|f| f.severity == "error").count()
    });
    Stage {
        key: "bundle",
        title,
        mark: if present == total && findings == 0 {
            "✓".to_string()
        } else {
            format!("{present}/{total}")
        },
        findings,
        detail,
    }
}

/// Каталог бандла: корень кейса с `EVIDENCE.yaml` либо активная дельта.
fn bundle_dir(dir: &Path) -> Option<PathBuf> {
    if dir.join("EVIDENCE.yaml").is_file() {
        return Some(dir.to_path_buf());
    }
    let rd = std::fs::read_dir(dir.join("changes")).ok()?;
    let mut found: Vec<PathBuf> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir() && p.join("EVIDENCE.yaml").is_file())
        .collect();
    found.sort();
    found.into_iter().next()
}

/// Следующая красная находка с подсказкой: первая по порядку прогона
/// проваленная составляющая, иначе первая обязательная пропущенная.
fn next_step(dir: &Path, report: &GateReport, cfg: &Config) -> Option<NextStep> {
    for c in &report.components {
        if c.status != GateStatus::Fail {
            continue;
        }
        let stage = stage_of(c.name);
        // Бандл: подсказку даёт сама семантика Н1 — она адресная («замените
        // заглушку в DECISION.md»), а не «заполните бандл».
        if c.name == "evidence_verify" {
            if let Some(bundle) = bundle_dir(dir) {
                if let Ok(verdict) = crate::evidence::verify_with(&bundle, &cfg.evidence) {
                    if let Some(f) = verdict
                        .semantics
                        .iter()
                        .find(|f| f.severity == "error" && !f.fix_hint.is_empty())
                    {
                        return Some(NextStep {
                            stage: stage.to_string(),
                            component: c.name.to_string(),
                            problem: format!(
                                "{} ({}) [{}]: {}",
                                artifact_label(&f.key),
                                f.key,
                                f.rule,
                                f.message
                            ),
                            fix_hint: f.fix_hint.clone(),
                        });
                    }
                    if let Some(m) = verdict.missing.first() {
                        return Some(NextStep {
                            stage: stage.to_string(),
                            component: c.name.to_string(),
                            problem: format!(
                                "отсутствует артефакт бандла: {} ({m})",
                                artifact_label(m)
                            ),
                            fix_hint: format!(
                                "создайте артефакт «{m}» и переупакуйте: \
                                 arch-be evidence pack {} --route {}",
                                dir.display(),
                                report.route
                            ),
                        });
                    }
                }
            }
        }
        let problem = c
            .findings
            .iter()
            .find(|f| f.severity == "error")
            .map_or_else(
                || c.detail.clone(),
                |f| {
                    f.file.as_ref().map_or_else(
                        || f.message.clone(),
                        |file| format!("{file}: {}", f.message),
                    )
                },
            );
        return Some(NextStep {
            stage: stage.to_string(),
            component: c.name.to_string(),
            problem,
            fix_hint: stage_fix_hint(c.name).to_string(),
        });
    }
    // Провалов нет, но обязательная составляющая без входа: зелёного не будет.
    let skipped = report
        .not_checked
        .first()
        .and_then(|name| report.components.iter().find(|c| c.name == name.as_str()))?;
    Some(NextStep {
        stage: stage_of(skipped.name).to_string(),
        component: skipped.name.to_string(),
        problem: format!("составляющая без входа: {}", skipped.detail),
        fix_hint: stage_fix_hint(skipped.name).to_string(),
    })
}

/// Человеческое имя артефакта бандла по его ключу: находка «problem: …» хуже
/// находки «формулировка проблемы/гипотезы результата (problem): …».
fn artifact_label(key: &str) -> String {
    crate::evidence::required_artifacts(Route::Critical)
        .iter()
        .chain(crate::evidence::required_artifacts(Route::Standard).iter())
        .chain(crate::evidence::required_artifacts(Route::Fast).iter())
        .find(|(k, _)| *k == key)
        .map_or_else(|| key.to_string(), |(_, desc)| (*desc).to_string())
}

/// Стадия, к которой относится составляющая (для строки шага).
fn stage_of(component: &str) -> &'static str {
    STAGES
        .iter()
        .find(|(_, _, components)| components.contains(&component))
        .map_or("контур", |(_, title, _)| title)
}

/// Создаёт каркас кейса и печатает дорожку до зелёного.
///
/// # Errors
/// Каталог занят, файлы не пишутся, бандл не упаковывается.
pub fn create(dir: &Path, name: &str, domain: &str, cfg: &Config) -> Result<Progress> {
    if dir.exists() && std::fs::read_dir(dir).is_ok_and(|mut rd| rd.next().is_some()) {
        return Err(HarnessError::Config(format!(
            "каталог {} не пуст — bootstrap создаёт кейс с нуля; \
             для существующего кейса: arch-be bootstrap --status --dir {}",
            dir.display(),
            dir.display()
        )));
    }
    let mut written = Vec::new();
    let mut notes: Vec<String> = Vec::new();
    for (rel, body) in skeleton(name, domain) {
        let path = dir.join(&rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| HarnessError::io(parent, e))?;
        }
        std::fs::write(&path, body).map_err(|e| HarnessError::io(&path, e))?;
        written.push(rel);
    }
    // Дельта-протокол работает от git-базы: вне репозитория `delta_guard`
    // уходит в SKIP, а он обязателен на Critical — «зелёного» не будет вовсе.
    // Поэтому каркас становится репозиторием с одним коммитом; если каталог
    // уже внутри репозитория, коммит — дело вызывающего, и проводник об этом
    // говорит.
    if git_is_repo(dir) {
        notes.push(
            "каталог уже внутри git-репозитория — закоммитьте каркас, иначе \
             `delta guard` (Н4) увидит неотслеживаемые файлы model/ как \
             непокрытую правку спайна"
                .to_string(),
        );
    } else {
        git_init(dir)?;
        notes.push(
            "каркас стал git-репозиторием с одним коммитом: дельта-протокол и \
             `delta_guard` работают от базы"
                .to_string(),
        );
    }
    // Бандл собирает сам инструмент: каркас обязан быть упакован тем же
    // способом, что и настоящий кейс, иначе хэши в манифесте — выдумка.
    crate::evidence::pack(dir, Route::Critical)?;
    // Каркас отвечает не «создано», а «вот что красное и что с ним делать»:
    // иначе проводник заканчивается там, где начинается работа.
    let mut progress = status(dir, cfg)?;
    progress.notes = notes;
    Ok(progress)
}

/// Человеко-читаемая карта созданного каркаса (для вывода команды).
#[must_use]
pub fn created_files() -> Vec<String> {
    skeleton("", "").into_iter().map(|(rel, _)| rel).collect()
}

/// Содержимое каркаса: путь → текст. Один источник и для создания, и для
/// справки — каркас не может разойтись с тем, что о нём написано.
#[must_use]
pub fn skeleton(name: &str, domain: &str) -> Vec<(String, String)> {
    let name = if name.is_empty() { "кейс" } else { name };
    let domain = if domain.is_empty() {
        "payments"
    } else {
        domain
    };
    let slug = slugify(name);
    let mut files: Vec<(String, String)> = Vec::new();
    let mut push = |rel: &str, body: String| files.push((rel.to_string(), body));

    push(
        "README.md",
        format!(
            "# {name}\n\nКаркас кейса, созданный `arch-be bootstrap`. Он КРАСНЫЙ: \
             артефакты бандла — заглушки.\n\nДорожка до зелёного:\n\n```bash\n\
             arch-be bootstrap --status --dir .   # что ещё не так и что делать\n\
             arch-be gate --repo . --explain      # паспорт вердикта: чего зелёный не означает\n\
             ```\n\nДомен: {domain}. Порядок шагов — спайн → правила → модель → бандл.\n"
        ),
    );
    push("ARCHITECTURE-SPINE.md", spine(name, domain));
    push("CONSTRAINTS.yaml", constraints(domain));
    push(
        ".arch-handoff/ROUTE.lock",
        "route: critical\ndecided_by: bootstrap\n".to_string(),
    );
    push(".arch-handoff/ROLLBACK.yaml", rollback_plan(&slug));
    push(".arch-handoff/REHEARSAL.json", rehearsal(&slug));
    for (rel, body) in model(name, domain) {
        push(&rel, body);
    }
    push(&format!("docs/adr/ADR-001-{slug}.md"), adr(name, domain));
    push(
        &format!("docs/spec/{slug}-acceptance.md"),
        spec(name, domain),
    );
    // Артефакты бандла: явные заглушки. Их обязана ловить семантика Н1 —
    // проводник, производящий ложнозелёные пакеты, хуже отсутствия проводника.
    for (rel, title) in [
        ("PROBLEM.md", "Проблема"),
        ("docs/SPEC.md", "Спецификация"),
        ("RISK.md", "Риск"),
        ("ACCEPTANCE.md", "Критерии приёмки"),
        ("ROLLBACK.md", "План отката"),
        ("DECISION.md", "Человеческое решение A3"),
        ("WALKING-SKELETON.md", "Walking skeleton"),
        ("VALIDATION.md", "Валидация"),
        ("reports/fitness.md", "Прогон fitness-функций"),
    ] {
        push(rel, stub_artifact(title, domain));
    }
    push(
        "docs/REVIEW.md",
        stub_artifact("Состоятельное ревью", domain),
    );
    files
}

/// Текст заглушки артефакта: он обязан быть пойман семантикой Н1, поэтому
/// содержит и незаполненное место шаблона, и незакрытый маркер.
fn stub_artifact(title: &str, domain: &str) -> String {
    format!(
        "# {title}\n\nTODO: заполнить содержанием. Пока здесь каркас: раздел о \
         домене {domain} не написан.\n\n> Шаблон: <что именно решили и почему>\n"
    )
}

/// Текст каркаса: спайн с двумя примерами инвариантов (валиден для
/// `spine_lint`, иначе первым шагом было бы «почините то, что создал
/// проводник»).
fn spine(name: &str, domain: &str) -> String {
    format!(
        "# ARCHITECTURE-SPINE — {name}\n\n\
         Инварианты, действующие на все компоненты контура. Нарушение любого — \
         не «долг», а дефект выпуска.\n\n\
         ## AD-1. Каждая операция домена {domain} идемпотентна по ключу\n\n\
         - **Binds**: {name}, Ядро {domain}\n\
         - **Prevents**: повторный эффект при повторной доставке запроса\n\
         - **Rule**: ключ идемпотентности обязателен на каждой точке входа; \
         дедупликация выполняется в одной транзакции с эффектом\n\n\
         ## AD-2. Журнал операций append-only\n\n\
         - **Binds**: Журнал операций, Ядро {domain}\n\
         - **Prevents**: потерю следа операции и подмену истории при разборе \
         расхождения\n\
         - **Rule**: журнал только дополняется; корректировка — компенсирующей \
         записью со ссылкой на исходную\n"
    )
}

/// Два правила — по одному на пример инварианта, плюс правило на форму ADR
/// (третий пример; его же требует первый ADR каркаса).
fn constraints(domain: &str) -> String {
    format!(
        "# Реестр правил кейса: {domain}\n\
         rules:\n\
         \x20 - id: C-001\n\
         \x20   name: spine_present\n\
         \x20   type: file_exists\n\
         \x20   path: \"ARCHITECTURE-SPINE.md\"\n\
         \x20   severity: error\n\
         \x20   owner: OWNER-001\n\
         \x20   expiry: 2027-12-31\n\
         \x20   effort_hours: 1\n\
         \x20 - id: C-002\n\
         \x20   name: journal_append_only\n\
         \x20   type: must_contain\n\
         \x20   glob: \"ARCHITECTURE-SPINE.md\"\n\
         \x20   pattern: 'append-only'\n\
         \x20   severity: error\n\
         \x20   owner: OWNER-001\n\
         \x20   expiry: 2027-12-31\n\
         \x20   effort_hours: 1\n\
         \x20 - id: C-003\n\
         \x20   name: adr_alternatives\n\
         \x20   type: each_file_must_contain\n\
         \x20   glob: \"docs/adr/ADR-*.md\"\n\
         \x20   pattern: 'Alternatives'\n\
         \x20   severity: error\n\
         \x20   owner: OWNER-001\n\
         \x20   expiry: 2027-12-31\n\
         \x20   effort_hours: 1\n"
    )
}

/// Скелет модели: минимальный связный набор, на котором звено трассировки
/// проходит (REQ → NFR → AD/ADR → CMP, у каждого AD — правило).
fn model(name: &str, domain: &str) -> Vec<(String, String)> {
    let body = |text: &str| format!("\n{text}\n");
    vec![
        (
            "model/SYS-001-servis.md".to_string(),
            format!(
                "---\nid: SYS-001\ntype: sys\ntitle: \"Сервис {domain}\"\n\
                 status: \"designed\"\nimplements: [AD-001, AD-002]\n---\n{}",
                body(&format!(
                    "Контур {domain}: что делает система, где её границы и что \
                     произойдёт при отказе. Здесь же — почему выбран этот \
                     вариант, а не <альтернатива>, и чем за выбор платим."
                ))
            ),
        ),
        (
            "model/CAP-001-osnovnoy-scenariy.md".to_string(),
            format!(
                "---\nid: CAP-001\ntype: cap\ntitle: \"{name}\"\nstatus: \"designed\"\n---\n{}",
                body(&format!(
                    "Основной сценарий домена {domain} — то, ради чего кейс \
                     существует, в одном абзаце без деталей реализации."
                ))
            ),
        ),
        (
            "model/REQ-001-bazovyy-scenariy.md".to_string(),
            format!(
                "---\nid: REQ-001\ntype: req\ntitle: \"Базовый сценарий {domain}\"\n\
                 status: \"accepted\"\n---\n{}",
                body(&format!(
                    "When клиент отправляет запрос домена {domain}, the сервис \
                     shall зафиксировать операцию и вернуть результат."
                ))
            ),
        ),
        (
            "model/NFR-001-latency.md".to_string(),
            format!(
                "---\nid: NFR-001\ntype: nfr\ntitle: \"Латентность отклика {domain}\"\n\
                 status: \"accepted\"\nverification: \"гистограмма \
                 {domain}_operation_duration_seconds\"\naffects: [CMP-001, INT-001]\n\
                 p99_target_ms: 1500.0\n---\n{}",
                body(&format!(
                    "Отклик по домену {domain}: p99 ≤ 1500 мс при доступной \
                     платформе. Способ проверки — в поле `verification`: цель \
                     без способа проверки на критическом маршруте — пожелание."
                ))
            ),
        ),
        (
            "model/INT-001-platforma.md".to_string(),
            format!(
                "---\nid: INT-001\ntype: int\ntitle: \"Вызов платформы домена {domain}\"\n\
                 status: \"designed\"\nlatency_budget_ms: 300.0\n---\n{}",
                body(&format!(
                    "Внешний вызов контура {domain}: `latency_budget_ms` — вклад этого \
                     перехода в бюджет цели p99 из NFR-001. Цепочка hop'ов нужна \
                     не для красоты: без неё бюджет цели не с чем сверить."
                ))
            ),
        ),
        (
            "model/AD-001-idempotentnost.md".to_string(),
            format!(
                "---\nid: AD-001\ntype: ad\ntitle: \"Идемпотентность операции по ключу\"\n\
                 status: \"ADOPTED\"\naffects: [CMP-001]\nverified_by: [C-001]\n---\n{}",
                body(&format!(
                    "- **Binds**: контур {domain}, ядро\n- **Prevents**: повторный \
                     эффект при повторной доставке\n- **Rule**: ключ идемпотентности \
                     на каждой точке входа"
                ))
            ),
        ),
        (
            "model/AD-002-append-only.md".to_string(),
            format!(
                "---\nid: AD-002\ntype: ad\ntitle: \"Журнал операций append-only\"\n\
                 status: \"ADOPTED\"\naffects: [CMP-001]\nverified_by: [C-002]\n---\n{}",
                body(
                    "- **Binds**: журнал операций, ядро\n- **Prevents**: подмену \
                     истории при разборе расхождения\n- **Rule**: журнал только \
                     дополняется; корректировка — компенсирующей записью",
                )
            ),
        ),
        (
            "model/ADR-001-idempotentnost.md".to_string(),
            format!(
                "---\nid: ADR-001\ntype: adr\ntitle: \"Идемпотентность на каждой \
                 точке входа\"\nstatus: \"Accepted\"\ndate: \"{today}\"\n\
                 affects: [CMP-001]\nimplements: [AD-001]\n---\n{}",
                body(&format!(
                    "Решение домена {domain} с оценкой обратимости: что рассмотрели, \
                     что выбрали, чем платим и как откатываем."
                )),
                today = chrono::Local::now().date_naive()
            ),
        ),
        (
            "model/CMP-001-yadro.md".to_string(),
            format!(
                "---\nid: CMP-001\ntype: cmp\ntitle: \"Ядро {domain}\"\n\
                 status: \"designed\"\nimplements: [REQ-001, AD-001]\n\
                 availability: 0.999\nreplicas: 2\nrps_per_instance: 200.0\n\
                 instances: 3\ncost_per_instance_month: 45000\nexit_cost: 300000\n---\n{}",
                body(&format!(
                    "Компонент контура {domain}: числа (`availability`, `replicas`, \
                     `rps_per_instance`, `instances`, стоимость и цена выхода) — \
                     вход количественных проверок NFR, а не украшение карточки."
                ))
            ),
        ),
        (
            "model/OWNER-001-vladelec.md".to_string(),
            format!(
                "---\nid: OWNER-001\ntype: owner\ntitle: \"Владелец контура {domain}\"\n\
                 status: \"accepted\"\n---\n{}",
                body(
                    "Кто отвечает за решения домена и к кому идти за изменением \
                     правила. Правила без владельца не ревизуются."
                )
            ),
        ),
    ]
}

/// Первый ADR по шаблону — с секцией альтернатив, иначе первым же шагом
/// стало бы «допишите Alternatives» (правило C-003).
fn adr(name: &str, domain: &str) -> String {
    format!(
        "# ADR-001. Идемпотентность на каждой точке входа\n\n\
         - **Статус**: Proposed\n- **Дата**: {today}\n\
         - **Контекст**: {name} — домен {domain}.\n\n\
         ## Context\n\nЗапрос может быть доставлен повторно; без дедупликации это \
         даёт повторный эффект.\n\n\
         ## Alternatives\n\n- Вариант А: ключ идемпотентности на каждой точке \
         входа (выбран).\n- Вариант Б: дедупликация только на шлюзе — не покрывает \
         внутренние повторы.\n- Вариант В: ничего не делать — цена ошибки выше \
         стоимости решения.\n\n\
         ## Consequences\n\nПлюс: повторная доставка безопасна. Минус: хранилище \
         ключей и точка отказа. Обратимость: решение обратимо, стоимость отката — \
         <оценка>.\n",
        today = chrono::Local::now().date_naive()
    )
}

/// Спецификация с обязательными разделами сенсоров.
fn spec(name: &str, domain: &str) -> String {
    format!(
        "# Спецификация: {name}\n\n\
         ## Проблема\n\nКлиент домена {domain} не может выполнить операцию \
         предсказуемо: результат зависит от ручных шагов и не проверяется машиной.\n\n\
         ## Критерии приёмки\n\n\
         - When клиент отправляет запрос, the сервис shall зафиксировать операцию \
         и вернуть результат за p99 ≤ 1500 мс.\n\
         - When запрос доставлен повторно, the сервис shall вернуть тот же \
         результат, не выполняя эффект дважды.\n\n\
         ## Риски\n\nПовторная доставка запроса при таймауте ответа: без \
         идемпотентности возможен повторный эффект. Механизм — AD-1 спайна.\n"
    )
}

/// План отката в формате, который читает `rehearsal`.
fn rollback_plan(slug: &str) -> String {
    format!(
        "baseline_commit: {slug}-1\n\
         steps:\n\
         \x20 - name: вернуть предыдущую версию сервиса\n\
         \x20   run: \"true\"\n\
         \x20 - name: переключить трафик\n\
         \x20   run: \"true\"\n\
         verify: \"true\"\n"
    )
}

/// Отчёт репетиции отката: PASS, иначе бандл краснеет по `rehearsal_not_passed`.
fn rehearsal(slug: &str) -> String {
    format!(
        "{{\n  \"kind\": \"rollback_rehearsal\",\n  \"gate\": \"A4\",\n  \
         \"passed\": true,\n  \"baseline_commit\": \"{slug}-1\",\n  \
         \"rehearsed_at\": \"{now}\",\n  \"duration_secs\": 1.0,\n  \
         \"steps\": [],\n  \"verify\": null,\n  \
         \"log\": [\"каркас: репетиция отката пройдена на baseline {slug}-1\"]\n}}\n",
        now = chrono::Local::now().to_rfc3339()
    )
}

/// Слаг каталога из человеческого имени: кириллица транслитерируется,
/// остальное обезвреживается до `[a-z0-9-]`. Одна и та же строка даёт одно
/// и то же имя каталога — иначе повторный bootstrap создавал бы копии.
#[must_use]
pub fn slugify(name: &str) -> String {
    let mut out = String::new();
    for ch in name.chars() {
        if let Some(lat) = translit(ch) {
            out.push_str(lat);
            continue;
        }
        let lower = ch.to_ascii_lowercase();
        if lower.is_ascii_alphanumeric() {
            out.push(lower);
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    let trimmed = out.trim_matches('-').to_string();
    if trimmed.is_empty() {
        "case".to_string()
    } else {
        trimmed
    }
}

/// Кириллица → латиница (ГОСТ-подобная практичная таблица).
fn translit(ch: char) -> Option<&'static str> {
    let lower = ch.to_lowercase().next().unwrap_or(ch);
    let base = match lower {
        'а' => "a",
        'б' => "b",
        'в' => "v",
        'г' => "g",
        'д' => "d",
        'е' | 'ё' | 'э' => "e",
        'ж' => "zh",
        'з' => "z",
        'и' => "i",
        'й' | 'ы' => "y",
        'к' => "k",
        'л' => "l",
        'м' => "m",
        'н' => "n",
        'о' => "o",
        'п' => "p",
        'р' => "r",
        'с' => "s",
        'т' => "t",
        'у' => "u",
        'ф' => "f",
        'х' => "h",
        'ц' => "c",
        'ч' => "ch",
        'ш' => "sh",
        'щ' => "sch",
        'ъ' | 'ь' => "",
        'ю' => "yu",
        'я' => "ya",
        _ => return None,
    };
    // Имя каталога — строчное: регистр исходного имени сохраняется в титулах.
    Some(base)
}

/// Каталог внутри git-репозитория (своё ли это репо — неважно: коммит в
/// чужое дерево проводник не делает).
fn git_is_repo(dir: &Path) -> bool {
    std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["rev-parse", "--git-dir"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// `git init` + один коммит каркаса. Личность коммиттера задаётся флагами
/// `-c`: глобальный конфиг машины проводник не трогает.
fn git_init(dir: &Path) -> Result<()> {
    let run = |args: &[&str]| -> Result<()> {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .env("GIT_AUTHOR_NAME", "arch-be bootstrap")
            .env("GIT_AUTHOR_EMAIL", "bootstrap@arch-be.invalid")
            .env("GIT_COMMITTER_NAME", "arch-be bootstrap")
            .env("GIT_COMMITTER_EMAIL", "bootstrap@arch-be.invalid")
            .output()
            .map_err(|e| HarnessError::Config(format!("git {}: {e}", args.join(" "))))?;
        if !out.status.success() {
            return Err(HarnessError::Config(format!(
                "git {}: {}",
                args.join(" "),
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
        Ok(())
    };
    run(&["init", "-q"])?;
    run(&["add", "-A"])?;
    run(&["commit", "-q", "-m", "chore(bootstrap): каркас кейса"])?;
    Ok(())
}

/// Печать прогресса в stdout-форме (общая для `create` и `--status`).
#[must_use]
pub fn render(progress: &Progress) -> String {
    let mut out = String::new();
    // Запись в String не может завершиться ошибкой — игноры безопасны.
    let _ = writeln!(out, "Кейс: {}", progress.dir.display());
    let _ = writeln!(
        out,
        "Маршрут: {} · итог: {}",
        progress.route, progress.outcome
    );
    for n in &progress.notes {
        let _ = writeln!(out, "· {n}");
    }
    let _ = writeln!(out, "{}", progress.line());
    for s in &progress.stages {
        if s.mark != "✓" {
            let _ = writeln!(out, "  {} — {}", s.title, s.detail);
        }
    }
    let _ = writeln!(out);
    if let Some(step) = &progress.next {
        let _ = writeln!(out, "{}", step.render());
    } else {
        let _ = writeln!(
            out,
            "Зелёный. Дальше — паспорт вердикта: arch-be gate --repo {} --explain",
            progress.dir.display()
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Прогон проводника на пустом каталоге во временной песочнице.
    fn bootstrapped(dir: &Path, name: &str) -> Progress {
        create(dir, name, "payments", &Config::default()).expect("bootstrap")
    }

    /// Каркас красный и говорит, что делать: три стадии пройдены, четвёртая
    /// ждёт работы, следующий шаг назван с подсказкой.
    #[test]
    fn bootstrap_creates_a_red_skeleton_with_a_next_step() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path().join("case");
        let progress = bootstrapped(&dir, "Зарплатные выплаты");

        assert_eq!(progress.outcome, "FAIL", "каркас обязан быть красным");
        assert!(!progress.is_green());
        let marks: Vec<(&str, &str)> = progress
            .stages
            .iter()
            .map(|s| (s.key, s.mark.as_str()))
            .collect();
        assert_eq!(
            marks,
            vec![
                ("spine", "✓"),
                ("rules", "✓"),
                ("model", "✓"),
                ("bundle", "13/13")
            ],
            "спайн, правила и модель каркаса обязаны быть валидны — иначе первым \
             шагом было бы «почините то, что создал проводник»"
        );
        let next = progress.next.expect("следующий шаг назван");
        assert!(
            !next.fix_hint.is_empty(),
            "шаг без подсказки — не дорожка, а констатация: {next:?}"
        );
        assert_eq!(next.component, "evidence_verify");
    }

    /// ГРАНИЦА ПРОВОДНИКА: заглушки каркаса обязаны ловиться семантикой Н1.
    /// Проводник, производящий ложнозелёные пакеты, хуже отсутствия
    /// проводника — он выдаёт «выпуск разрешён» за «ничего не написано».
    #[test]
    fn bootstrap_stub_skeleton_is_caught_by_evidence_semantics() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path().join("case");
        bootstrapped(&dir, "Зарплатные выплаты");

        let verdict = crate::evidence::verify_with(&dir, &crate::config::EvidenceConfig::default())
            .expect("бандл каркаса читается");
        assert!(
            !verdict.passed,
            "каркас не имеет права быть зелёным: {}",
            verdict.summary
        );
        let stubs: Vec<&crate::evidence::SemanticFinding> = verdict
            .semantics
            .iter()
            .filter(|f| f.rule == "evidence_stub" && f.severity == "error")
            .collect();
        assert!(
            stubs.len() >= 10,
            "заглушки каркаса обязаны быть пойманы все (десять артефактов-заглушек              плюс незаполненное место в шаблоне ADR), пойманы: {}",
            stubs.len()
        );
        assert!(
            stubs.iter().all(|f| !f.fix_hint.is_empty()),
            "у каждой находки есть подсказка"
        );
    }

    /// Проводник не перезаписывает чужой каталог и говорит, что делать с
    /// существующим кейсом.
    #[test]
    fn bootstrap_refuses_a_non_empty_directory() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path().join("case");
        std::fs::create_dir_all(&dir).expect("mkdir");
        std::fs::write(dir.join("README.md"), "уже есть").expect("write");
        let err = create(&dir, "Кейс", "payments", &Config::default())
            .expect_err("занятый каталог — отказ");
        let text = err.to_string();
        assert!(text.contains("не пуст"), "{text}");
        assert!(
            text.contains("--status"),
            "отказ обязан называть выход для существующего кейса: {text}"
        );
    }

    /// `--status` идемпотентен: два прогона на неизменённом дереве дают одну
    /// строку прогресса и одну аттестацию (иначе «дорожка» дышит сама).
    #[test]
    fn bootstrap_status_is_idempotent() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path().join("case");
        bootstrapped(&dir, "Зарплатные выплаты");
        let first = status(&dir, &Config::default()).expect("status");
        let second = status(&dir, &Config::default()).expect("status");
        assert_eq!(first.line(), second.line());
        assert_eq!(first.attestation, second.attestation);
    }

    /// Заполненные артефакты уводят каркас к зелёному — проводник доводит до
    /// конца, а не заканчивается на «создано».
    #[test]
    fn bootstrap_walks_to_green_when_artifacts_are_written() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path().join("case");
        bootstrapped(&dir, "Зарплатные выплаты");
        // Заменяем заглушки настоящим содержанием: то же, что сделает
        // архитектор, идя по шагам проводника.
        for (rel, _) in [
            ("PROBLEM.md", ""),
            ("docs/SPEC.md", ""),
            ("RISK.md", ""),
            ("ACCEPTANCE.md", ""),
            ("ROLLBACK.md", ""),
            ("DECISION.md", ""),
            ("WALKING-SKELETON.md", ""),
            ("VALIDATION.md", ""),
            ("reports/fitness.md", ""),
            ("docs/REVIEW.md", ""),
        ] {
            std::fs::write(dir.join(rel), written_artifact(rel)).expect("write artifact");
        }
        // Шаблон ADR — тоже с незаполненным местом (`<оценка>`), и его ловит
        // та же семантика: шаблон не становится решением оттого, что лежит в
        // docs/adr.
        let adr_dir = dir.join("docs/adr");
        for entry in std::fs::read_dir(&adr_dir).expect("adr dir").flatten() {
            let path = entry.path();
            let text = std::fs::read_to_string(&path).expect("adr");
            std::fs::write(&path, text.replace("<оценка>", "две недели и один релиз"))
                .expect("adr filled");
        }
        crate::evidence::pack(&dir, Route::Critical).expect("pack");
        let progress = status(&dir, &Config::default()).expect("status");
        assert_eq!(
            progress.outcome,
            "PASS",
            "заполненный каркас обязан дойти до зелёного: {}",
            progress.line()
        );
        assert!(progress.next.is_none(), "шагов не осталось");
    }

    /// Правдоподобное содержание артефакта — по его роли в бандле. Заглушек
    /// нет, строка итога у отчётов есть, A3 подписан.
    fn written_artifact(rel: &str) -> &'static str {
        match rel {
            "DECISION.md" => {
                "# Человеческое решение A3\n\n- **choice**: вариант А — ключ идемпотентности на каждой точке входа\n- **rationale**: повторная доставка запроса иначе даёт повторный эффект, а разбор расхождения дороже ключа\n- **rejected**: вариант Б — дедупликация только на шлюзе, не покрывает внутренние повторы\n- **expiry**: 2099-12-31\n- **decided_by**: архитектор контура\n\nРешение принято человеком; механика проверяет только заполненность полей.\n"
            }
            "docs/REVIEW.md" => {
                "# Состязательное ревью\n\nРазобраны сценарии повторной доставки, отказ платформы и разбор расхождения.\n\nVERDICT: READY\n"
            }
            "VALIDATION.md" | "reports/fitness.md" | "WALKING-SKELETON.md" => {
                "# Отчёт\n\nПрогон на тестовом контуре: сценарии повторной доставки и отказа платформы проверены.\n\nИтог: PASS (8 из 8)\n"
            }
            "RISK.md" => {
                "# Риск\n\nУровень: critical. Повторная доставка запроса без дедупликации даёт повторный эффект; отказ платформы оставляет операцию в неопределённом состоянии.\n"
            }
            "ACCEPTANCE.md" => {
                "# Критерии приёмки\n\n- When запрос доставлен повторно, the сервис shall вернуть тот же результат, не выполняя эффект дважды.\n- When платформа не ответила, the сервис shall перевести операцию в разбор.\n"
            }
            "ROLLBACK.md" => {
                "# План отката\n\nBaseline: предыдущая версия сервиса. Шаги: вернуть версию, переключить трафик, проверить сверкой. Учения проведены, отчёт — в .arch-handoff/REHEARSAL.json.\n"
            }
            _ => {
                "# Раздел\n\nКлиент домена не может выполнить операцию предсказуемо: результат зависит от ручных шагов. Решение — ключ идемпотентности на каждой точке входа и append-only журнал; альтернативы рассмотрены и отклонены по цене сопровождения.\n"
            }
        }
    }

    /// Слаг каталога: кириллица транслитерируется, повтор даёт то же имя.
    #[test]
    fn slugify_is_stable_and_transliterates() {
        assert_eq!(
            slugify("Зарплатные и социальные выплаты"),
            "zarplatnye-i-socialnye-vyplaty"
        );
        assert_eq!(
            slugify("Зарплатные и социальные выплаты"),
            slugify("Зарплатные и социальные выплаты")
        );
        assert_eq!(slugify("!!!"), "case");
    }

    /// В справке о каркасе — те же файлы, что он создаёт.
    #[test]
    fn created_files_matches_the_skeleton() {
        assert_eq!(created_files().len(), skeleton("к", "payments").len());
    }
}
