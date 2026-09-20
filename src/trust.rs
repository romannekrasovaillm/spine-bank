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
    let limits = cfg
        .significance
        .limits()
        .map_err(|e| crate::error::HarnessError::Config(format!("маршруты значимости: {e}")))?;
    let report = gate::run_opts(
        repo,
        None,
        None,
        None,
        limits,
        &gate::GateRequirements::from_config(&cfg.gate),
        &gate::GateOptions::from_config(cfg),
    )?;

    let anchors = vec![
        anchor_journal(repo),
        anchor_gate_was_red(repo),
        anchor_rules(repo),
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
    }
}

/// Якорь 3: правила сопровождаются — у каждого владелец и срок, ни одно не
/// просрочено, и хотя бы одно проверяет ПОВЕДЕНИЕ, а не наличие текста
/// (правило на упоминание зеленеет и когда о инварианте просто написали).
fn anchor_rules(repo: &Path) -> Anchor {
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
        };
    };
    let rules = &resolved.rules;
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
    let behaviour = rules
        .iter()
        .filter(|r| crate::control::BEHAVIOUR_RULE_KINDS.contains(&r.kind.as_str()))
        .count();
    let met = total > 0 && with_owner == total && expired.is_empty() && behaviour > 0;
    let mut evidence = format!(
        "правил: {total}, с владельцем: {with_owner}, просрочено: {}; \
         проверяют поведение: {behaviour}",
        expired.len()
    );
    // Регистр FP — доказательство, что правила ревизуют, а не только завели.
    let marks = crate::digest::fp_register_read(&crate::digest::fp_register_path(repo));
    let _ = write!(evidence, "; пометок FP: {}", marks.len());
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
        if behaviour == 0 {
            reasons.push(
                "ни одно правило не проверяет поведение — только наличие текста \
                 (кандидат дают `rules_suggest` и `rules-report`)"
                    .to_string(),
            );
        }
        reasons.join("; ")
    });
    Anchor {
        n: 3,
        title: "Правила сопровождаются",
        met,
        evidence,
        why_not,
    }
}

/// Якорь 4: пакет защищён ИЗМЕРЕННО — доля обнаружения мутационного прогона
/// не ниже порога и контроль аттестации пройден.
fn anchor_redteam(repo: &Path) -> Anchor {
    let path = repo.join(REDTEAM_RESULT_REL);
    let Some(summary) = crate::redteam::load_summary(&path) else {
        return Anchor {
            n: 4,
            title: "Пакет защищён измеренно",
            met: false,
            evidence: format!("{REDTEAM_RESULT_REL}: результата нет"),
            why_not: Some(
                "измерьте: `arch-be redteam . --save` — доля обнаружения без \
                 измерения не аргумент"
                    .to_string(),
            ),
        };
    };
    let met = summary.passed();
    Anchor {
        n: 4,
        title: "Пакет защищён измеренно",
        met,
        evidence: format!(
            "{REDTEAM_RESULT_REL}: доля обнаружения {}/{} = {:.0} % (порог {:.0} %), \
             контроль аттестации: {}",
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
    }
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
    let met = gaps.is_empty();
    Anchor {
        n: 5,
        title: "Вердикт полон и подписан",
        met,
        evidence: format!(
            "вердикт: {} (exit {}), аттестация sha256:{}…; обязательных без входа: {}",
            report.outcome.label(),
            report.outcome.exit_code(),
            report.attestation.get(..12).unwrap_or(&report.attestation),
            report.not_checked.len()
        ),
        why_not: (!met).then(|| gaps.join("; ")),
    }
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
            format!(
                "{}: судья и автор — {} (оценка не независима)",
                a.target.as_deref().unwrap_or("документ без пути"),
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
