//! Метрика доверия к контуру (W4): положение на шкале 1–5 с ЯКОРЯМИ и
//! ДОКАЗАТЕЛЬСТВАМИ, а не число.
//!
//! Число «доверие 4 из 5» бесполезно: непонятно, что именно проверено и чего
//! не хватает. Поэтому каждая ступень — якорь, а у якоря два обязательных
//! поля: чем он подтверждён (файл, счётчик, вердикт) и почему не достигнут.
//! Якорь без доказательства не засчитывается: «правила сопровождаются» без
//! прочитанного реестра — это обещание, а не факт.
//!
//! Метрика ничего не блокирует: она отвечает на вопрос «насколько можно
//! верить зелёному этого контура», а не «можно ли выпускать». Верхняя ступень
//! честно недостижима, пока есть неподписанное A3, судья-автор или измеренная
//! доля обнаружения ниже порога: шкала, у которой верх всегда достижим,
//! измеряет старательность, а не защищённость.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use crate::config::Config;
use crate::error::Result;
use crate::gate;

/// Файл измеренной доли обнаружения, который пишет `redteam --save`.
pub const REDTEAM_RESULT_REL: &str = ".arch-handoff/redteam.json";

/// Один якорь шкалы: что проверено, чем подтверждено и почему не достигнут.
#[derive(Debug, Clone)]
pub struct Anchor {
    /// Номер ступени (1–5).
    pub n: u8,
    /// Имя якоря.
    pub title: &'static str,
    /// Достигнут.
    pub met: bool,
    /// Чем подтверждён (одна строка с числами и путями).
    pub evidence: String,
    /// Почему не достигнут и что закрыть (`None` — достигнут).
    pub why_not: Option<String>,
    /// Предупреждения, не меняющие достижение якоря (схема «warn → error по
    /// флагу», ADR-065): измеренная правда о якоре, которая пока не решает.
    pub warnings: Vec<String>,
}

/// Положение контура на шкале доверия.
#[derive(Debug, Clone)]
pub struct Trust {
    /// Репозиторий.
    pub repo: PathBuf,
    /// Якоря по возрастанию ступени.
    pub anchors: Vec<Anchor>,
    /// Число достигнутых якорей (1–5).
    pub score: u8,
    /// Блокеры верхней ступени: неподписанное A3, судья = автор, доля
    /// обнаружения ниже порога. Пока список непуст, ступень 5 недостижима —
    /// этовидно и в якорях, но называется отдельно: человек читает «5 из 5»
    /// как «всё хорошо», а не как «всё измерено».
    pub blockers: Vec<String>,
}

impl Trust {
    /// Якоря, которые ещё не достигнуты (по возрастанию).
    #[must_use]
    pub fn unmet(&self) -> Vec<&Anchor> {
        self.anchors.iter().filter(|a| !a.met).collect()
    }

    /// Что закрыть, чтобы подняться на ступень выше (первый недостигнутый
    /// якорь), либо `None` на верхней ступени.
    #[must_use]
    pub fn next(&self) -> Option<&Anchor> {
        self.unmet().into_iter().next()
    }

    /// Формулировка положения: `4 из 5 (достигнуто: «пакет защищён измеренно»,
    /// следующий — «вердикт полон и подписан»)`. Высший достигнутый якорь и
    /// следующий недостигнутый называются вместе: без второго число читается
    /// как оценка вообще, а не как место на лестнице.
    #[must_use]
    pub fn label(&self) -> String {
        let reached = self
            .anchors
            .iter()
            .rev()
            .find(|a| a.met)
            .map_or("ни одного".to_string(), |a| {
                format!("«{}»", a.title)
            });
        let next = self.next().map_or_else(
            || "все пройдены".to_string(),
            |a| format!("следующий — «{}»", a.title),
        );
        format!("{} из 5 (достигнуто: {reached}; {next})", self.score)
    }
}

/// Оценивает контур: читает журнал вызовов, реестр правил и регистр FP,
/// результат `redteam`, вердикт гейта и отчёты рубрик.
///
/// # Errors
/// Репозиторий недоступен либо конфигурация маршрутов невалидна.
pub fn assess(repo: &Path, cfg: &Config) -> Result<Trust> {
    assess_with(repo, cfg, &crate::cmd_trust::ExecPolicy::default())
}

/// Полная форма [`assess`] со снимком модели доверия `command_succeeds`
/// (A3, ADR-053): прогон гейта внутри оценки наследует политику края —
/// CLI `trust` передаёт CLI-снимок (флаг/переменная/allow-файл), MCP
/// `trust_report` — серверный (no-exec по умолчанию); библиотечный вызов
/// ([`assess`]) — детерминированный legacy-режим (AD-7).
///
/// # Errors
/// Те же, что у [`assess`].
pub fn assess_with(
    repo: &Path,
    cfg: &Config,
    exec: &crate::cmd_trust::ExecPolicy,
) -> Result<Trust> {
    let limits = cfg
        .significance
        .limits()
        .map_err(|e| crate::error::HarnessError::Config(format!("маршруты значимости: {e}")))?;
    let options = gate::GateOptions {
        exec: exec.clone(),
        ..gate::GateOptions::from_config(cfg)
    };
    let report = gate::run_opts(
        repo,
        None,
        None,
        None,
        limits,
        &gate::GateRequirements::from_config(&cfg.gate),
        &options,
    )?;

    let anchors = vec![
        anchor_journal(repo),
        anchor_gate_was_red(repo),
        anchor_rules(repo, cfg),
        anchor_redteam(repo),
        anchor_verdict(repo, &report, cfg),
    ];
    let blockers = blockers(repo, cfg);
    let score = anchors.iter().filter(|a| a.met).count() as u8;
    Ok(Trust {
        repo: repo.to_path_buf(),
        anchors,
        score,
        blockers,
    })
}

/// Якорь 1: контур подключён и им пользуются — журнал вызовов непуст и в нём
/// больше одного инструмента. Инструмент, который не вызывают, доверия не
/// заслуживает: его вердикты не проверены практикой.
fn anchor_journal(repo: &Path) -> Anchor {
    let path = crate::mcp_journal::journal_path(repo);
    let rel = ".arch-handoff/mcp-calls.jsonl";
    if !path.is_file() {
        return Anchor {
            n: 1,
            title: "Контур подключён",
            met: false,
            evidence: format!("{rel}: файла нет"),
            why_not: Some(
                "подключите MCP-сервер (`arch-be connect <харнесс>`) и поработайте \
                 через него — журнал вызовов и есть доказательство"
                    .to_string(),
            ),
            warnings: Vec::new(),
        };
    }
    let entries = crate::mcp_journal::read_entries(&path);
    let tools: BTreeSet<&str> = entries.iter().map(|e| e.tool.as_str()).collect();
    let met = !entries.is_empty() && tools.len() >= 3;
    Anchor {
        n: 1,
        title: "Контур подключён",
        met,
        evidence: format!(
            "{rel}: {} {}, {} {}",
            entries.len(),
            plural(entries.len(), "вызов", "вызова", "вызовов"),
            tools.len(),
            plural(tools.len(), "инструмент", "инструмента", "инструментов")
        ),
        why_not: (!met).then(|| {
            "в журнале меньше трёх разных инструментов — контур подключён, но им \
             не пользуются (`tools/list` и `tools/call` пишутся в журнал)"
                .to_string()
        }),
        warnings: Vec::new(),
    }
}

/// Якорь 2: гейт останавливал работу — в журнале есть провал, после которого
/// тот же инструмент дал зелёный. Контур, который всегда зелёный, ничего не
/// проверяет.
fn anchor_gate_was_red(repo: &Path) -> Anchor {
    let path = crate::mcp_journal::journal_path(repo);
    let entries = crate::mcp_journal::read_entries(&path);
    // Инструменты с вердиктом: по ним и ищем «краснел → починили».
    let mut reddened: Vec<&str> = Vec::new();
    let mut fixed: Vec<&str> = Vec::new();
    for e in &entries {
        match e.verdict.as_str() {
            "fail" => reddened.push(e.tool.as_str()),
            // Починка засчитывается один раз: важно, что инструмент краснел и
            // после правки позеленел, а не сколько раз он зеленел потом.
            "pass" if reddened.contains(&e.tool.as_str()) && !fixed.contains(&e.tool.as_str()) => {
                fixed.push(e.tool.as_str());
            }
            _ => {}
        }
    }
    let met = !fixed.is_empty();
    Anchor {
        n: 2,
        title: "Гейт останавливал работу",
        met,
        evidence: if entries.is_empty() {
            "журнал пуст — вердиктов нет".to_string()
        } else {
            format!(
                "провалов в журнале: {}; после провала починен: {}",
                reddened.len(),
                if fixed.is_empty() {
                    "ни один".to_string()
                } else {
                    fixed.join(", ")
                }
            )
        },
        why_not: (!met).then(|| {
            "ни один инструмент не краснел и не был починен: либо контур всегда \
             зелёный (тогда он ничего не проверяет), либо журнал ведётся не с \
             начала работы"
                .to_string()
        }),
        warnings: Vec::new(),
    }
}

/// Якорь 3: правила сопровождаются — у каждого владелец и срок, ни одно не
/// просрочено, и хотя бы одно проверяет ПОВЕДЕНИЕ, а не наличие текста
/// (правило на упоминание зеленеет и когда о инварианте просто написали).
///
/// Волна B (B3, ADR-065): «проверяет поведение» по ТИПУ — заявление; по
/// измерению зубьев (`.arch-handoff/teeth.json`, пишет `arch-be rules teeth
/// --save`) — факт. Режим по умолчанию не ломает прежнее условие ступени:
/// измерение (или его отсутствие) показывается предупреждением якоря
/// (`warnings`). По флагу `[trust] require_teeth = true` условие меняется на
/// измеренное: при несущих инвариантах (`load_bearing: true` в модели)
/// каждый обязан быть покрыт правилом с подтверждёнными зубьями; без
/// несущих — доля таких правил не ниже `[trust] behaviour_share_min`
/// (дефолт 0.2; порог — решение архитектора, не зашито).
fn anchor_rules(repo: &Path, cfg: &Config) -> Anchor {
    let Some(path) = crate::control::resolve_constraints_path(repo, None) else {
        return Anchor {
            n: 3,
            title: "Правила сопровождаются",
            met: false,
            evidence: "CONSTRAINTS.yaml не найден".to_string(),
            why_not: Some(
                "заведите реестр правил (`arch-be init` кладёт пример) — без него \
                 контур проверяет не инварианты, а привычки"
                    .to_string(),
            ),
            warnings: Vec::new(),
        };
    };
    let Ok(resolved) = crate::control::load_constraints_resolved(&path) else {
        return Anchor {
            n: 3,
            title: "Правила сопровождаются",
            met: false,
            evidence: format!("{}: реестр не читается", path.display()),
            why_not: Some(
                "исправьте синтаксис реестра: `arch-be control rules-report .` назовёт \
                 находку"
                    .to_string(),
            ),
            warnings: Vec::new(),
        };
    };
    let rules = &resolved.rules;
    let rule_refs: Vec<&crate::control::FitnessRule> = rules.iter().collect();
    let total = rules.len();
    let with_owner = rules.iter().filter(|r| has_text(&r.owner)).count();
    let expired: Vec<&str> = rules
        .iter()
        .filter(|r| {
            r.expiry
                .as_deref()
                .is_some_and(crate::control::expiry_is_past)
        })
        .map(|r| r.name.as_str())
        .collect();
    // Заявлено ТИПОМ правила (поведение прежних версий; волна B: это не факт).
    let behaviour = rules
        .iter()
        .filter(|r| crate::control::BEHAVIOUR_RULE_KINDS.contains(&r.kind.as_str()))
        .count();
    // Измерено (B1): зубья из сохранённого файла, без пересчёта.
    let teeth = crate::control::teeth::load(repo);
    let teeth_groups = crate::control::teeth::groups(&rule_refs, teeth.as_ref());
    let confirmed = teeth_groups.confirmed.len();
    // Покрытие инвариантов: по типу правила (как раньше) и по зубьям.
    let coverage = crate::rule_templates::ad_coverage(repo).ok().flatten();
    let uncovered_ads: Vec<String> = coverage
        .as_ref()
        .map(|c| ads_without_teeth(c, &rule_refs, teeth.as_ref()))
        .unwrap_or_default();
    let load_bearing_uncovered: Vec<String> = coverage
        .as_ref()
        .map(|c| {
            c.entries
                .iter()
                .filter(|e| e.load_bearing && uncovered_ads.contains(&e.ad))
                .map(|e| e.ad.clone())
                .collect()
        })
        .unwrap_or_default();

    // Условие ступени по измерению (B3): при несущих AD — каждый покрыт
    // правилом с подтверждёнными зубьями; без несущих — доля не ниже порога.
    let share_min = cfg.trust.behaviour_share_min;
    let measured_condition: std::result::Result<(), String> = match &teeth {
        None => Err(
            "зубья правил не измерены (`arch-be rules teeth --save`) — «проверяют \
             поведение» заявлено типом правила, а не измерением"
                .to_string(),
        ),
        Some(_) if !load_bearing_uncovered.is_empty() => Err(format!(
            "несущие инварианты без правила с подтверждёнными зубьями: {}",
            load_bearing_uncovered.join(", ")
        )),
        Some(_) if coverage.as_ref().is_some_and(|c| c.load_bearing_defined) => Ok(()),
        Some(_) => {
            let share = if total == 0 {
                0.0
            } else {
                confirmed as f64 / total as f64
            };
            if share + f64::EPSILON >= share_min {
                Ok(())
            } else {
                Err(format!(
                    "правил с подтверждёнными зубьями {confirmed} из {total} ({:.0} %) — \
                     ниже порога [trust] behaviour_share_min = {:.0} %",
                    share * 100.0,
                    share_min * 100.0
                ))
            }
        }
    };
    let strict = cfg.trust.require_teeth;
    let base_met = total > 0 && with_owner == total && expired.is_empty();
    let met = if strict {
        base_met && measured_condition.is_ok()
    } else {
        base_met && behaviour > 0
    };
    // Предупреждения (warn-фаза): измеренная правда, которая пока не решает.
    let mut warnings: Vec<String> = Vec::new();
    if !strict {
        match &measured_condition {
            Err(reason) if teeth.is_some() => warnings.push(format!(
                "замер зубьев: {reason} — в режиме [trust] require_teeth = true \
                 ступень 3 была бы недостигнута"
            )),
            Err(_) => warnings.push(
                "зубья правил не измерены: доля поведенческих заявлена типом правила, \
                 а не измерением (`arch-be rules teeth --save`)"
                    .to_string(),
            ),
            Ok(()) => {}
        }
    }
    let mut evidence = format!(
        "правил: {total}, с владельцем: {with_owner}, просрочено: {}; \
         проверяют поведение (по типу): {behaviour}",
        expired.len()
    );
    // Измеренная доля — рядом с заявленной, иначе «по типу» читалось бы как факт.
    match &teeth {
        Some(measurement) => {
            let _ = write!(
                evidence,
                "; с подтверждёнными зубьями: {confirmed} из {total} (измерение {})",
                measurement.measured_at
            );
        }
        None => {
            let _ = write!(evidence, "; зубья не измерены");
        }
    }
    // Регистр FP — доказательство, что правила ревизуют, а не только завели.
    let marks = crate::digest::fp_register_read(&crate::digest::fp_register_path(repo));
    let _ = write!(evidence, "; пометок FP: {}", marks.len());
    // Покрытие инвариантов исполняемыми проверками (ADR-050): деталь
    // показывает, сколько инвариантов реально проверяется, — иначе «правила
    // сопровождаются» читается как «инварианты проверяются».
    if let Some(c) = &coverage {
        let total_ads = c.total();
        if total_ads > 0 {
            let _ = write!(
                evidence,
                "; инвариантов с проверкой поведения: {} из {total_ads}",
                c.covered().len()
            );
        }
    }
    let why_not = (!met).then(|| {
        let mut reasons: Vec<String> = Vec::new();
        if total == 0 {
            reasons.push("реестр пуст".to_string());
        }
        if with_owner != total {
            reasons.push(format!("у {} правил нет владельца", total - with_owner));
        }
        if !expired.is_empty() {
            reasons.push(format!("просрочены: {}", expired.join(", ")));
        }
        if strict {
            if let Err(reason) = &measured_condition {
                reasons.push(reason.clone());
            }
        } else if behaviour == 0 {
            reasons.push(
                "ни одно правило не проверяет поведение — только наличие текста \
                 (кандидат дают `rules_suggest` и `rules-report`)"
                    .to_string(),
            );
        }
        // Приёмка B3: причина называет непокрытые инварианты поимённо.
        if !uncovered_ads.is_empty() {
            reasons.push(format!(
                "инварианты без правила с подтверждёнными зубьями{}: {}",
                if teeth.is_some() {
                    ""
                } else {
                    " (зубья не измерены)"
                },
                uncovered_ads.join(", ")
            ));
        }
        reasons.join("; ")
    });
    Anchor {
        n: 3,
        title: "Правила сопровождаются",
        met,
        evidence,
        why_not,
        warnings,
    }
}

/// Инварианты модели без правила с подтверждёнными зубьями (B3): по измерению
/// `.arch-handoff/teeth.json`, а без него — по типу правила (тогда список —
/// заявление по типам, а не измеренный факт). Ссылки `verified_by` и `ad:`
/// правила сопоставляются по id/имени (канон `rule_ref_eq`).
fn ads_without_teeth(
    coverage: &crate::rule_templates::AdCoverage,
    rules: &[&crate::control::FitnessRule],
    teeth: Option<&crate::control::teeth::TeethReport>,
) -> Vec<String> {
    let mut out = Vec::new();
    for entry in &coverage.entries {
        let covered = entry.rules.iter().any(|link| {
            let rule = rules.iter().find(|r| {
                crate::rule_templates::rule_ref_eq(
                    r.id.as_deref().unwrap_or(&r.name),
                    &link.reference,
                ) || crate::rule_templates::rule_ref_eq(&r.name, &link.reference)
            });
            match (rule, teeth) {
                (Some(rule), Some(t)) => {
                    t.status_of(rule) == Some(crate::control::teeth::TeethStatus::Confirmed)
                }
                // Без измерения — заявление по типу (поведение прежних версий).
                _ => link
                    .kind
                    .as_deref()
                    .is_some_and(|k| crate::control::BEHAVIOUR_RULE_KINDS.contains(&k)),
            }
        });
        if !covered {
            out.push(entry.ad.clone());
        }
    }
    out
}

/// Якорь 4: пакет защищён ИЗМЕРЕННО — доля обнаружения мутационного прогона
/// не ниже порога и контроль аттестации пройден.
fn anchor_redteam(repo: &Path) -> Anchor {
    let path = repo.join(REDTEAM_RESULT_REL);
    let Some(summary) = crate::redteam::load_summary(&path) else {
        // E6.4: смысловая карта называется и здесь — иначе «результата нет»
        // читалось бы как «судья не проверен», хотя квалификация может быть.
        return Anchor {
            n: 4,
            title: "Пакет защищён измеренно",
            met: false,
            evidence: format!(
                "{REDTEAM_RESULT_REL}: результата нет; {}",
                semantic_detection_map(repo)
            ),
            why_not: Some(
                "измерьте: `arch-be redteam . --save` — доля обнаружения без \
                 измерения не аргумент"
                    .to_string(),
            ),
            warnings: Vec::new(),
        };
    };
    let met = summary.passed();
    // E6.4: карта обнаружения смыслового судьи по классам дефектов — из отчёта
    // квалификации. Она не двигает ступень (допуск судьи — политика проекта,
    // E6.3), но шкала доверия обязана её называть: «доля обнаружения» без неё
    // описывает только механику.
    let semantic = semantic_detection_map(repo);
    Anchor {
        n: 4,
        title: "Пакет защищён измеренно",
        met,
        evidence: format!(
            "{REDTEAM_RESULT_REL}: доля обнаружения {}/{} = {:.0} % (порог {:.0} %), \
             контроль аттестации: {}; {semantic}",
            summary.caught,
            summary.total,
            summary.ratio * 100.0,
            summary.min_detection * 100.0,
            if summary.control_ok { "да" } else { "нет" }
        ),
        why_not: (!met).then(|| {
            if summary.control_ok {
                format!(
                    "доля обнаружения {:.0} % ниже порога {:.0} % — усильте правила \
                     по непойманным мутаторам (`arch-be redteam .`)",
                    summary.ratio * 100.0,
                    summary.min_detection * 100.0
                )
            } else {
                // Причина — из самого измерения, а не догадка метрики: исходов у
                // контроля два, и означают они разное (ADR-047).
                summary.control_note.clone().unwrap_or_else(|| {
                    "контроль аттестации не сработал (причина в файле измерения не \
                     записана — перемерьте: `arch-be redteam . --save`)"
                        .to_string()
                })
            }
        }),
        warnings: Vec::new(),
    }
}

/// Карта обнаружения смыслового судьи по классам дефектов (E6.4): читает
/// отчёты квалификации (`reports/qualification/*.json`) и называет долю
/// обнаружения по классам с моделью и вердиктом допуска.
///
/// Отчёта нет — так и говорится: «не измерена». Молчаливое умолчание читалось
/// бы как «судья проверен».
fn semantic_detection_map(repo: &Path) -> String {
    let dir = repo.join(crate::rubric::QUALIFICATION_DIR);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return "смысловая доля обнаружения не измерена (`arch-be rubric qualify`)".to_string();
    };
    let mut best: Option<(std::time::SystemTime, crate::rubric::QualificationReport)> = None;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("json"))
        {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(report) = serde_json::from_str::<crate::rubric::QualificationReport>(&text) else {
            continue;
        };
        let time = entry
            .metadata()
            .and_then(|m| m.modified())
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
        if best.as_ref().is_none_or(|(t, _)| time > *t) {
            best = Some((time, report));
        }
    }
    let Some((_, report)) = best else {
        return "смысловая доля обнаружения не измерена (`arch-be rubric qualify`)".to_string();
    };
    let classes: Vec<String> = report
        .by_class
        .iter()
        .filter(|c| c.defective > 0)
        .map(|c| format!("{}: {:.0} %", c.class, c.detection_share() * 100.0))
        .collect();
    format!(
        "смысловая доля обнаружения судьи {}: {} (допуск: {})",
        report.model,
        classes.join(", "),
        if report.passed {
            "пройден"
        } else {
            "не пройден"
        }
    )
}

/// Якорь 5: вердикт полон и подписан — обязательные составляющие имеют вход,
/// запись A3 подписана человеком, судья рубрики не совпадает с автором.
fn anchor_verdict(repo: &Path, report: &gate::GateReport, cfg: &Config) -> Anchor {
    let mut gaps: Vec<String> = Vec::new();
    if !report.not_checked.is_empty() {
        gaps.push(format!(
            "обязательные составляющие без входа: {}",
            report.not_checked.join(", ")
        ));
    }
    let unsigned = unsigned_a3(repo, cfg);
    gaps.extend(unsigned.iter().cloned());
    let judge = judge_is_author(repo);
    gaps.extend(judge.iter().cloned());
    // Уровень независимости: условие ступени не меняется (его задаёт
    // judge_is_author), но требование проекта поднимает планку (ADR-048).
    let weakest = weakest_independence(repo, cfg);
    if let Some((level, min)) = &weakest {
        if crate::judge::independence_rank(level) < crate::judge::independence_rank(min) {
            gaps.push(format!(
                "независимость судьи «{}» ниже порога проекта «{}»",
                crate::judge::independence_label(level),
                crate::judge::independence_label(min),
            ));
        }
    }
    let met = gaps.is_empty();
    Anchor {
        n: 5,
        title: "Вердикт полон и подписан",
        met,
        evidence: format!(
            "вердикт: {} (exit {}), аттестация sha256:{}…; обязательных без входа: {}; \
             минимальная независимость судьи: {}",
            report.outcome.label(),
            report.outcome.exit_code(),
            report.attestation.get(..12).unwrap_or(&report.attestation),
            report.not_checked.len(),
            weakest.map_or_else(
                || "отчётов рубрики нет".to_string(),
                |(level, _)| crate::judge::independence_label(&level).to_string(),
            )
        ),
        why_not: (!met).then(|| gaps.join("; ")),
        warnings: Vec::new(),
    }
}

/// Минимальный уровень независимости по отчётам рубрики и порог проекта:
/// `None` — отчётов с уровнем нет, сравнивать не с чем (ADR-048).
fn weakest_independence(repo: &Path, cfg: &Config) -> Option<(String, String)> {
    let levels: Vec<String> = crate::rubric::load_artifacts(repo)
        .into_iter()
        .filter_map(|a| a.independence)
        .collect();
    let weakest = crate::judge::min_independence(levels.iter().map(String::as_str))?;
    Some((weakest, cfg.trust.min_independence.clone()))
}

/// Блокеры верхней ступени (честное правило W4): неподписанное A3, судья =
/// автор, доля обнаружения ниже порога. Каждый назван с адресом. Источники —
/// те же, что у якоря 5 и якоря 4: два разных мнения об одном состоянии
/// разошлись бы, и метрика начала бы противоречить сама себе.
fn blockers(repo: &Path, cfg: &Config) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    out.extend(unsigned_a3(repo, cfg));
    out.extend(judge_is_author(repo));
    let path = repo.join(REDTEAM_RESULT_REL);
    if let Some(summary) = crate::redteam::load_summary(&path) {
        if !summary.passed() {
            out.push(format!(
                "доля обнаружения redteam {:.0} % ниже порога {:.0} % (или контроль \
                 аттестации не пройден)",
                summary.ratio * 100.0,
                summary.min_detection * 100.0
            ));
        }
    } else {
        out.push("доля обнаружения redteam не измерена".to_string());
    }
    out
}

/// Записи A3 без подписи человека: `decided_by` пуст или прочерк. Источник —
/// семантика бандла (Н1), а не догадка по тексту.
fn unsigned_a3(repo: &Path, cfg: &Config) -> Vec<String> {
    let mut out = Vec::new();
    for dir in bundle_dirs(repo) {
        let Ok(verdict) = crate::evidence::verify_with(&dir, &cfg.evidence) else {
            continue;
        };
        for f in verdict
            .semantics
            .iter()
            .filter(|f| f.rule == "a3_not_signed" && f.message.contains("decided_by"))
        {
            out.push(format!("{}: {}", rel(repo, &dir), f.message));
        }
    }
    out
}

/// Отчёты рубрик, где судья совпадает с автором документа или автор не указан:
/// такая оценка не независима, и опираться на неё как на доказательство нельзя.
fn judge_is_author(repo: &Path) -> Vec<String> {
    crate::rubric::load_artifacts(repo)
        .into_iter()
        .filter(|a| {
            a.author_model
                .as_deref()
                .is_none_or(|author| author.trim().is_empty() || author == a.judge_model)
        })
        .map(|a| {
            // У отчёта по досье (ADR-051) `target` пуст, а субъект назван в
            // `subject`: без этого пятая ступень говорила бы «документ без
            // пути» вместо того, чей именно отчёт не независим.
            let what = a
                .target
                .as_deref()
                .or(a.subject.as_deref())
                .unwrap_or("документ без пути");
            format!(
                "{what}: судья и автор — {} (оценка не независима)",
                a.judge_model
            )
        })
        .collect()
}

/// Каталоги бандлов: корень и активные дельты (как у составляющей гейта).
fn bundle_dirs(repo: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if repo.join("EVIDENCE.yaml").is_file() {
        out.push(repo.to_path_buf());
    }
    if let Ok(rd) = std::fs::read_dir(repo.join("changes")) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir()
                && p.file_name().is_some_and(|n| n != "archive")
                && p.join("EVIDENCE.yaml").is_file()
            {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

/// Путь каталога относительно репозитория (для читаемой ссылки в доказательстве).
fn rel(repo: &Path, dir: &Path) -> String {
    dir.strip_prefix(repo).map_or_else(
        |_| dir.display().to_string(),
        |p| {
            if p.as_os_str().is_empty() {
                ".".to_string()
            } else {
                p.display().to_string()
            }
        },
    )
}

/// Русское число с существительным: `1 вызов · 4 вызова · 12 вызовов`.
fn plural(n: usize, one: &'static str, few: &'static str, many: &'static str) -> &'static str {
    let tail = n % 100;
    if (11..=14).contains(&tail) {
        return many;
    }
    match n % 10 {
        1 => one,
        2..=4 => few,
        _ => many,
    }
}

/// Непустое поле правила.
fn has_text(field: &Option<String>) -> bool {
    field.as_deref().is_some_and(|v| !v.trim().is_empty())
}

/// Печатная форма отчёта: якоря с доказательствами, потолок и следующий шаг.
#[must_use]
pub fn render(trust: &Trust) -> String {
    let mut out = String::new();
    // Запись в String не может завершиться ошибкой — игноры безопасны.
    let _ = writeln!(out, "Доверие к контуру: {}", trust.label());
    let _ = writeln!(out, "Репозиторий: {}", trust.repo.display());
    let _ = writeln!(out);
    for a in &trust.anchors {
        let mark = if a.met { "✓" } else { "✗" };
        let _ = writeln!(out, "  [{mark}] {}. {} — {}", a.n, a.title, a.evidence);
        if let Some(why) = &a.why_not {
            let _ = writeln!(out, "        → {why}");
        }
        for warning in &a.warnings {
            // warn-фаза (ADR-065): видно, но не решает.
            let _ = writeln!(out, "        ⚠ {warning}");
        }
    }
    if !trust.blockers.is_empty() {
        let _ = writeln!(out);
        let _ = writeln!(
            out,
            "Ступень 5 недостижима, пока не сняты блокеры: {}",
            trust.blockers.join("; ")
        );
    }
    let _ = writeln!(out);
    match trust.next() {
        Some(a) => {
            let _ = writeln!(
                out,
                "Следующая ступень — {}: {}",
                a.n,
                a.why_not.as_deref().unwrap_or(a.title)
            );
        }
        None => {
            let _ = writeln!(out, "Все якоря достигнуты.");
        }
    }
    let _ = writeln!(
        out,
        "Метрика ничего не блокирует: она говорит, насколько можно верить \
         зелёному этого контура."
    );
    out
}

/// Машиночитаемая форма (`--format json`).
#[must_use]
pub fn to_json(trust: &Trust) -> serde_json::Value {
    serde_json::json!({
        "schema": "arch-be/trust/v1",
        "repo": trust.repo.display().to_string(),
        "score": trust.score,
        "scale_max": 5,
        "label": trust.label(),
        "top_reachable": trust.blockers.is_empty(),
        "blockers": trust.blockers,
        "anchors": trust.anchors.iter().map(|a| serde_json::json!({
            "n": a.n,
            "title": a.title,
            "met": a.met,
            "evidence": a.evidence,
            "why_not": a.why_not,
            "warnings": a.warnings,
        })).collect::<Vec<_>>(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::mcp_journal::{self, JournalEntry};

    /// Репозиторий-песочница с реестром правил (владелец и срок задаются
    /// вызывающим: именно их отсутствие и должен называть якорь 3).
    fn repo_with_rules(dir: &Path, owner: bool) {
        std::fs::create_dir_all(dir.join(".arch-handoff")).expect("mkdir");
        let owner_line = if owner { "\n    owner: OWNER-001" } else { "" };
        std::fs::write(
            dir.join(".arch-handoff/CONSTRAINTS.yaml"),
            format!(
                "rules:\n  - name: spine_present\n    type: file_exists\n    \
                 path: \"ARCHITECTURE-SPINE.md\"\n    severity: error{owner_line}\n    \
                 expiry: 2099-12-31\n"
            ),
        )
        .expect("constraints");
        std::fs::write(dir.join("ARCHITECTURE-SPINE.md"), "# Spine\n").expect("spine");
    }

    /// Отчёт рубрики с уровнем независимости: пятая ступень говорит, какой
    /// уровень достигнут и какого требует проект (ADR-048).
    fn report_with_independence(dir: &Path, level: &str, judge: &str, author: &str) {
        let reports = dir.join("reports/rubric");
        std::fs::create_dir_all(&reports).expect("mkdir");
        std::fs::write(
            reports.join("ADR-001-demo.json"),
            format!(
                "{{\n  \"schema\": \"arch-be/rubric-report/v1\",\n  \"rubric\": \"adr_quality\",\n                   \"target\": \"docs/adr/ADR-001-demo.md\",\n  \"judge_model\": \"{judge}\",\n                   \"author_model\": \"{author}\",\n  \"weighted_total\": 4.0,\n  \"verdict\": \"годно\",\n                   \"judged_at\": \"2026-09-20T10:00:00+03:00\",\n  \"independence\": \"{level}\"\n}}\n"
            ),
        )
        .expect("отчёт");
    }

    // --- B3 (волна B 0.3.14, ADR-065): ступень 3 по измеренным зубьям --------

    /// Реестр с владельцем/сроком у каждого правила (значения — в тексте
    /// вызывающего): `extra_rules` — тела правил YAML.
    fn repo_with_registry(dir: &Path, rules_yaml: &str) {
        std::fs::create_dir_all(dir.join(".arch-handoff")).expect("mkdir");
        std::fs::write(
            dir.join(".arch-handoff/CONSTRAINTS.yaml"),
            format!("rules:\n{rules_yaml}"),
        )
        .expect("constraints");
        std::fs::write(dir.join("ARCHITECTURE-SPINE.md"), "# Spine\n").expect("spine");
    }

    /// Модель с одним инвариантом: `load_bearing` и ссылка `verified_by` —
    /// параметры фикстуры.
    fn model_with_ad(dir: &Path, load_bearing: bool, verified_by: &str) {
        std::fs::create_dir_all(dir.join("model")).expect("model");
        std::fs::write(
            dir.join("model/AD-001-idempotentnost.md"),
            format!(
                "---\nid: AD-001\ntype: ad\ntitle: \"Идемпотентность\"\nstatus: \"ADOPTED\"\n\
                 load_bearing: {load_bearing}\nverified_by: [{verified_by}]\n---\n\n\
                 - **Binds**: Приём\n- **Prevents**: дубли\n- **Rule**: ключ обязателен\n"
            ),
        )
        .expect("ad");
    }

    /// Замерить зубья фикстуры и сохранить `.arch-handoff/teeth.json` (как
    /// `arch-be rules teeth --save`).
    fn measure_and_save_teeth(dir: &Path) -> crate::control::teeth::TeethReport {
        let report = crate::control::teeth::measure(dir, None).expect("измерение");
        crate::control::teeth::save(dir, &report).expect("сохранение");
        report
    }

    /// Режим по умолчанию (warn-фаза ADR-065): без измерения зубьев условие
    /// ступени прежнее (тип правила), но якорь предупреждает, что доля
    /// «поведенческих» заявлена типом, а не измерением.
    #[test]
    fn anchor_three_warns_but_keeps_legacy_semantics_without_measurement() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path();
        repo_with_registry(
            dir,
            "  - id: C-001\n    name: ci_green\n    type: command_succeeds\n    \
             command: 'true'\n    severity: error\n    owner: OWNER-1\n    expiry: '2099-01-01'\n",
        );
        let anchor = anchor_rules(dir, &Config::default());
        // `command: 'true'` по типу «поведенческое» — наследие 0.3.13: без
        // измерения ступень не ломается (warn → error только по флагу).
        assert!(anchor.met, "наследие не ломается: {anchor:?}");
        assert!(
            anchor.warnings.iter().any(|w| w.contains("не измерены")),
            "предупреждение про отсутствие измерения: {:?}",
            anchor.warnings
        );
        assert!(anchor.evidence.contains("зубья не измерены"), "{anchor:?}");
    }

    /// Строгий режим (`[trust] require_teeth = true`): тривиальная команда не
    /// даёт зубьев — ступень 3 недостижима и причина называет долю и порог.
    #[test]
    fn anchor_three_strict_mode_counts_only_confirmed_teeth() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path();
        repo_with_registry(
            dir,
            "  - id: C-001\n    name: ci_green\n    type: command_succeeds\n    \
             command: 'true'\n    severity: error\n    owner: OWNER-1\n    expiry: '2099-01-01'\n  \
             - id: C-002\n    name: no_pan\n    type: must_not_contain\n    \
             glob: 'skeleton/**/*.py'\n    pattern: 'PAN'\n    severity: error\n    \
             owner: OWNER-1\n    expiry: '2099-01-01'\n",
        );
        let mut strict = Config::default();
        strict.trust.require_teeth = true;
        // Без измерения: недостигнута с честной причиной.
        let anchor = anchor_rules(dir, &strict);
        assert!(!anchor.met, "без измерения — не достигнута");
        let why = anchor.why_not.clone().expect("причина");
        assert!(why.contains("не измерены"), "{why}");

        // Измерение: 'true' тривиальна, no_pan с пустым набором — glob_empty:
        // подтверждённых 0 из 2 — ниже дефолтного порога 20 %.
        measure_and_save_teeth(dir);
        let anchor = anchor_rules(dir, &strict);
        assert!(
            !anchor.met,
            "тривиальная команда зубьев не даёт: {anchor:?}"
        );
        let why = anchor.why_not.clone().expect("причина");
        assert!(why.contains("behaviour_share_min"), "{why}");
        assert!(why.contains("0 из 2"), "{why}");
        // А в режиме по умолчанию тот же замер — предупреждение, не вердикт.
        let soft = anchor_rules(dir, &Config::default());
        assert!(soft.met, "warn-фаза не ломает: {soft:?}");
        assert!(
            soft.warnings.iter().any(|w| w.contains("require_teeth")),
            "предупреждение зовёт включить строгий режим: {:?}",
            soft.warnings
        );
    }

    /// Несущий инвариант (`load_bearing: true`) обязан быть покрыт правилом с
    /// ПОДТВЕРЖДЁННЫМИ зубьями: замер подтверждает — ступень взята; набор
    /// правила пуст — ступень недостижима и причина называет инвариант.
    #[test]
    fn anchor_three_load_bearing_ads_need_confirmed_teeth() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path();
        let registry = "  - id: C-100\n    name: no_pan\n    type: must_not_contain\n    \
             glob: 'skeleton/**/*.py'\n    pattern: 'PAN'\n    severity: error\n    \
             owner: OWNER-1\n    expiry: '2099-01-01'\n  \
             - id: C-101\n    name: ci_green\n    type: command_succeeds\n    \
             command: 'true'\n    severity: error\n    owner: OWNER-1\n    expiry: '2099-01-01'\n";
        repo_with_registry(dir, registry);
        model_with_ad(dir, true, "C-100");
        // Файл набора есть — мутация вставкой даст находку: зубья подтверждены.
        std::fs::create_dir_all(dir.join("skeleton")).expect("skeleton");
        std::fs::write(dir.join("skeleton/pay.py"), "def pay():\n    return 1\n").expect("py");
        let report = measure_and_save_teeth(dir);
        assert_eq!(report.confirmed(), 1, "{:?}", report.entries);

        let mut strict = Config::default();
        strict.trust.require_teeth = true;
        let anchor = anchor_rules(dir, &strict);
        assert!(
            anchor.met,
            "несущий AD покрыт правилом с зубьями — ступень взята: {}",
            anchor.why_not.clone().unwrap_or_default()
        );

        // Вариант-контраст: файлов под glob нет — зубья не подтверждены,
        // причина называет несущий инвариант поимённо.
        std::fs::remove_file(dir.join("skeleton/pay.py")).expect("remove");
        let report = measure_and_save_teeth(dir);
        assert_eq!(report.confirmed(), 0, "{:?}", report.entries);
        let anchor = anchor_rules(dir, &strict);
        assert!(!anchor.met, "{anchor:?}");
        let why = anchor.why_not.clone().expect("причина");
        assert!(why.contains("AD-001"), "несущий AD назван: {why}");
        assert!(why.contains("несущие инварианты"), "{why}");
    }

    /// Без несущих инвариантов в строгом режиме действует доля; в причине при
    /// недостаче — непокрытые инварианты поимённо (приёмка B3 на эталонном
    /// кейсе: текстовый реестр + модель с AD).
    #[test]
    fn anchor_three_names_uncovered_ads() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path();
        // Текстовый реестр (ни одного поведенческого типа) — как эталонные
        // кейсы волны B: ступень 3 не выдаётся и AD названы в причине.
        repo_with_registry(
            dir,
            "  - id: C-001\n    name: spine_binds\n    type: must_contain\n    \
             glob: 'ARCHITECTURE-SPINE.md'\n    pattern: 'Binds'\n    severity: error\n    \
             owner: OWNER-1\n    expiry: '2099-01-01'\n",
        );
        model_with_ad(dir, false, "C-001");
        let anchor = anchor_rules(dir, &Config::default());
        assert!(!anchor.met, "текстовый реестр — ступень 3 не выдаётся");
        let why = anchor.why_not.clone().expect("причина");
        assert!(why.contains("AD-001"), "инвариант назван в причине: {why}");
    }

    /// Пятая ступень называет минимальный уровень независимости по отчётам, а
    /// требование проекта поднимает планку: при пороге `launched` отчёт
    /// уровня `declared` делает ступень недостижимой (ADR-048).
    #[test]
    fn trust_detail_shows_min_independence() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path();
        repo_with_rules(dir, true);
        journal(dir, &[("spine_lint", "fail"), ("spine_lint", "pass")]);
        report_with_independence(dir, "declared_cross_family", "glm-5.2", "claude-opus-4");

        let cfg = Config::default();
        let report = crate::gate::run(dir, Some(crate::control::Route::Fast), None, None, (50, 50))
            .expect("gate");
        let anchor = anchor_verdict(dir, &report, &cfg);
        assert!(
            anchor.evidence.contains("заявлена"),
            "уровень назван в доказательстве: {}",
            anchor.evidence
        );

        // Порог проекта: требовать обеспеченную запуском независимость.
        let mut strict = Config::default();
        strict.trust.min_independence = crate::judge::INDEPENDENCE_LAUNCHED.to_string();
        let anchor = anchor_verdict(dir, &report, &strict);
        assert!(!anchor.met, "порог не достигнут — ступень не взята");
        let why = anchor.why_not.expect("причина");
        assert!(
            why.contains("ниже порога проекта"),
            "причина называет порог: {why}"
        );
    }

    /// E6.4: карта обнаружения смыслового судьи по классам читается из отчёта
    /// квалификации и попадает в доказательство якоря 4; без отчёта шкала
    /// честно говорит «не измерена», а не молчит.
    #[test]
    fn semantic_detection_map_is_read_from_qualification() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path();
        assert!(
            semantic_detection_map(dir).contains("не измерена"),
            "без отчёта шкала говорит прямо"
        );
        let qdir = dir.join(crate::rubric::QUALIFICATION_DIR);
        std::fs::create_dir_all(&qdir).expect("mkdir");
        let class = |name: &str, caught: usize, missed: usize| {
            serde_json::json!({
                "class": name, "total": 6, "defective": 3, "clean": 3,
                "caught": caught, "missed": missed, "cleared": 3,
                "false_accusations": 0, "human": 0
            })
        };
        let report = serde_json::json!({
            "schema": crate::rubric::QUALIFICATION_SCHEMA,
            "rubric": "code_invariant_conformance",
            "model": "deepseek-flash",
            "set": "/набор",
            "set_sha256": "a".repeat(64),
            "judged_at": "2026-09-25T00:00:00+00:00",
            "samples": 1,
            "by_class": [class("ignored_key", 3, 0), class("no_return", 1, 2)],
            "totals": {"class": "итого", "total": 12, "defective": 6, "clean": 6,
                       "caught": 4, "missed": 2, "cleared": 6, "false_accusations": 0,
                       "human": 0},
            "human_share": 0.0,
            "thresholds": {"min_completeness": 0.8, "min_accuracy": 0.8,
                           "max_human_share": 0.5},
            "passed": false,
            "failures": ["полнота 0.67 ниже порога 0.80"],
            "cases": []
        });
        std::fs::write(
            qdir.join("code_invariant_conformance--deepseek-flash.json"),
            serde_json::to_string_pretty(&report).expect("json"),
        )
        .expect("write");
        let map = semantic_detection_map(dir);
        assert!(map.contains("deepseek-flash"), "{map}");
        assert!(map.contains("ignored_key: 100 %"), "{map}");
        assert!(map.contains("no_return: 33 %"), "{map}");
        assert!(map.contains("не пройден"), "{map}");
    }

    /// Запись журнала MCP-вызовов.
    fn journal(dir: &Path, entries: &[(&str, &str)]) {
        let path = mcp_journal::journal_path(dir);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        for (tool, verdict) in entries {
            mcp_journal::append(
                dir,
                &JournalEntry::new(tool, verdict, std::time::Duration::ZERO, Vec::new()),
            )
            .expect("journal");
        }
    }

    /// Якорь 1 без журнала не засчитывается и называет файл, которого нет.
    #[test]
    fn anchor_one_needs_a_journal() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path();
        repo_with_rules(dir, true);
        let trust = assess(dir, &Config::default()).expect("trust");
        let first = &trust.anchors[0];
        assert!(!first.met, "{first:?}");
        assert!(first.evidence.contains("mcp-calls.jsonl"), "{first:?}");
        assert!(first.why_not.is_some());
    }

    /// Якорь 1 засчитывается по журналу, якорь 2 — только когда после провала
    /// тот же инструмент дал зелёный. Контур, который всегда зелёный, ничего
    /// не проверяет.
    #[test]
    fn anchor_two_needs_a_fail_that_was_fixed() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path();
        repo_with_rules(dir, true);
        journal(
            dir,
            &[
                ("fitness_check", "pass"),
                ("spine_lint", "pass"),
                ("trace_check", "pass"),
            ],
        );
        let always_green = assess(dir, &Config::default()).expect("trust");
        assert!(
            always_green.anchors[0].met,
            "журнал есть — якорь 1 достигнут"
        );
        assert!(
            !always_green.anchors[1].met,
            "зелёный журнал без провалов якорь 2 не даёт"
        );

        let tmp2 = tempfile::tempdir().expect("tmp2");
        let dir2 = tmp2.path();
        repo_with_rules(dir2, true);
        journal(
            dir2,
            &[
                ("fitness_check", "pass"),
                ("spine_lint", "pass"),
                ("trace_check", "pass"),
                ("fitness_check", "fail"),
                ("fitness_check", "pass"),
            ],
        );
        let fixed = assess(dir2, &Config::default()).expect("trust");
        assert!(fixed.anchors[1].met, "{}", fixed.anchors[1].evidence);
        assert!(fixed.anchors[1].evidence.contains("fitness_check"));
    }

    /// Якорь 3 называет недостачу: правило без владельца — не правило, а
    /// пожелание (сопровождать его некому).
    #[test]
    fn anchor_three_names_missing_owner() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path();
        repo_with_rules(dir, false);
        let trust = assess(dir, &Config::default()).expect("trust");
        let third = &trust.anchors[2];
        assert!(!third.met);
        let why = third.why_not.as_deref().unwrap_or_default();
        assert!(why.contains("владельца"), "{why}");
    }

    /// Якорь 4 читает ИЗМЕРЕННУЮ долю из `.arch-handoff/redteam.json`, а не
    /// верит на слово: без файла нет и якоря.
    #[test]
    fn anchor_four_reads_the_saved_measurement() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path();
        repo_with_rules(dir, true);
        let trust = assess(dir, &Config::default()).expect("trust");
        assert!(!trust.anchors[3].met);
        assert!(trust.blockers.iter().any(|b| b.contains("не измерена")));

        std::fs::write(
            dir.join(REDTEAM_RESULT_REL),
            "{\"schema\":\"arch-be/redteam/v1\",\"case\":\"кейс\",\
             \"measured_at\":\"2026-09-19T00:00:00+00:00\",\"caught\":11,\"total\":14,\
             \"ratio\":0.79,\"min_detection\":0.78,\"control_ok\":true}",
        )
        .expect("redteam.json");
        let trust = assess(dir, &Config::default()).expect("trust");
        assert!(trust.anchors[3].met, "{}", trust.anchors[3].evidence);
        assert!(trust.anchors[3].evidence.contains("11/14"));
    }

    /// Контроль аттестации — часть якоря: без него измерение не значит ничего
    /// (вердикт не привязан к состоянию дерева).
    #[test]
    fn anchor_four_requires_the_attestation_control() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path();
        repo_with_rules(dir, true);
        std::fs::write(
            dir.join(REDTEAM_RESULT_REL),
            "{\"schema\":\"arch-be/redteam/v1\",\"case\":\"кейс\",\
             \"measured_at\":\"2026-09-19T00:00:00+00:00\",\"caught\":14,\"total\":14,\
             \"ratio\":1.0,\"min_detection\":0.78,\"control_ok\":false}",
        )
        .expect("redteam.json");
        let trust = assess(dir, &Config::default()).expect("trust");
        assert!(
            !trust.anchors[3].met,
            "100 % без контроля — не доказательство"
        );
        assert!(
            trust.anchors[3]
                .why_not
                .as_deref()
                .unwrap_or_default()
                .contains("контроль")
        );
    }

    /// Якорь 4 показывает причину отказа ИЗ ИЗМЕРЕНИЯ, а не свою догадку:
    /// исходов у контроля два, и «вердикт изменился» — не то же самое, что
    /// «аттестация не изменилась» (T-08). Метрика, называющая не ту причину,
    /// отправляет архитектора чинить несуществующее.
    #[test]
    fn anchor_four_shows_the_measured_reason() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path();
        repo_with_rules(dir, true);
        std::fs::write(
            dir.join(REDTEAM_RESULT_REL),
            "{\"schema\":\"arch-be/redteam/v1\",\"case\":\"кейс\",\
             \"measured_at\":\"2026-09-20T00:00:00+00:00\",\"caught\":11,\"total\":14,\
             \"ratio\":0.79,\"min_detection\":0.78,\"control_ok\":false,\
             \"control_note\":\"безвредная правка изменила вердикт (PASS → FAIL) — \
             правка не должна менять вердикт: проверьте составляющие delta_guard\"}",
        )
        .expect("redteam.json");
        let trust = assess(dir, &Config::default()).expect("trust");
        let why = trust.anchors[3].why_not.clone().unwrap_or_default();
        assert!(
            why.contains("delta_guard"),
            "причина обязана прийти из измерения: {why}"
        );
        assert!(
            !why.contains("не изменила аттестацию"),
            "догадка метрики не подменяет измеренную причину: {why}"
        );
    }

    /// Верхняя ступень недостижима, пока есть блокер, и блокер назван с
    /// адресом: судья-автор — это не «независимая оценка».
    #[test]
    fn top_anchor_is_unreachable_while_blockers_stand() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path();
        repo_with_rules(dir, true);
        let adr_dir = dir.join("docs/adr");
        std::fs::create_dir_all(&adr_dir).expect("mkdir");
        let adr = adr_dir.join("ADR-001-demo.md");
        std::fs::write(&adr, "# ADR-001\n\n## Alternatives\n\nБ.\n").expect("adr");
        let sha = crate::hash::sha256_file(&adr).expect("sha");
        std::fs::create_dir_all(dir.join(crate::rubric::RUBRIC_REPORTS_DIR)).expect("mkdir");
        std::fs::write(
            dir.join(crate::rubric::RUBRIC_REPORTS_DIR)
                .join("ADR-001-demo.json"),
            format!(
                "{{\"schema\":\"arch-be/rubric-report/v1\",\"rubric\":\"adr_quality\",\
                 \"target\":\"docs/adr/ADR-001-demo.md\",\"target_sha256\":\"{sha}\",\
                 \"judge_model\":\"gpt-5\",\"author_model\":\"gpt-5\",\
                 \"weighted_total\":4.0,\"verdict\":\"accept\",\"judged_at\":\
                 \"2026-09-19T00:00:00+00:00\"}}"
            ),
        )
        .expect("rubric report");

        let trust = assess(dir, &Config::default()).expect("trust");
        assert!(
            trust.blockers.iter().any(|b| b.contains("судья и автор")),
            "блокер назван: {:?}",
            trust.blockers
        );
        assert!(
            !trust.anchors[4].met,
            "якорь 5 не достигнут при судье-авторе"
        );
        let page = render(&trust);
        assert!(page.contains("Ступень 5 недостижима"), "{page}");
    }

    /// У каждого якоря есть доказательство — даже у недостигнутого: «нет
    /// данных» это тоже факт, а пустая строка — нет.
    #[test]
    fn every_anchor_carries_evidence() {
        let tmp = tempfile::tempdir().expect("tmp");
        let trust = assess(tmp.path(), &Config::default()).expect("trust");
        assert_eq!(trust.anchors.len(), 5);
        for a in &trust.anchors {
            assert!(
                !a.evidence.trim().is_empty(),
                "якорь {} без доказательства",
                a.n
            );
            assert_eq!(a.met, a.why_not.is_none(), "якорь {} несогласован", a.n);
        }
        assert!(trust.score <= 5);
    }

    /// JSON несёт те же якоря: каналы не имеют права расходиться.
    #[test]
    fn json_carries_the_same_anchors() {
        let tmp = tempfile::tempdir().expect("tmp");
        let trust = assess(tmp.path(), &Config::default()).expect("trust");
        let json = to_json(&trust);
        assert_eq!(json["schema"], "arch-be/trust/v1");
        assert_eq!(json["anchors"].as_array().map(Vec::len), Some(5));
        assert_eq!(json["score"].as_u64(), Some(u64::from(trust.score)));
    }
}
