//! Кандидатные fitness-правила из содержательных пробелов кейса
//! (`arch-be control rules-suggest`, MCP `rules_suggest`).
//!
//! Живой опыт работы архитектора через MCP-сервер: содержательные пробелы
//! кейса находят методики-скиллы (EARS-критерии приёмки, таймауты в
//! контрактах, декомпозиция REQ→работы, RTO/RPO без ADR, аудит операторских
//! действий), а не гейты. Детекторы ниже превращают эти чек-листы в
//! кандидатные правила `CONSTRAINTS.yaml`: где проверка механизируема —
//! готовый YAML-фрагмент (`must_contain`), где нет — честный advisory
//! (`yaml: None`), без механики, которая дала бы ложную уверенность.
//!
//! Все детекторы — read-only эвристики: ничего не пишут в кейс, читают
//! ограниченный набор текстов (лимиты [`MAX_SCAN_FILES`]/[`MAX_FILE_BYTES`])
//! и работают на уровне warn-кандидатов: решение о взятии правила в
//! `CONSTRAINTS.yaml` (и о повышении severity) — за архитектором. Источник
//! методики каждого кандидата — скилл библиотеки плагинов (`source_skill`).

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use regex::Regex;
use serde::Serialize;
use walkdir::WalkDir;

use crate::error::{HarnessError, Result};

/// Потолок файлов, читаемых одним проходом сбора текстов (защита от
/// гигантских деревьев документации: эвристике достаточно первой сотни).
const MAX_SCAN_FILES: usize = 200;

/// Потолок байт читаемого файла; большее анализируется по усечённому
/// префиксу (маркеры детекторов живут в тексте, не в бинарных хвостах).
const MAX_FILE_BYTES: u64 = 512 * 1024;

/// Кандидатное правило: что предложить, почему и откуда методика.
#[derive(Debug, Clone, Serialize)]
pub struct Candidate {
    /// Стабильный id детектора (kebab-case).
    pub id: String,
    /// Обоснование: что найдено в кейсе и почему это пробел.
    pub rationale: String,
    /// Скилл-источник методики (библиотека плагинов).
    pub source_skill: String,
    /// Готовый YAML-фрагмент для `rules:` файла `CONSTRAINTS.yaml`, когда
    /// проверка механизируема; `None` — честный advisory (механики нет).
    pub yaml: Option<String>,
}

/// Отчёт детекторов: кандидаты + однострочная сводка.
#[derive(Debug, Clone, Serialize)]
pub struct SuggestReport {
    /// Кандидаты (порядок — порядок детекторов, стабилен).
    pub candidates: Vec<Candidate>,
    /// Однострочная сводка для CLI/MCP.
    pub summary: String,
}

/// Собирает текстовые файлы каталога (рекурсивно, без следования симлинкам)
/// с заданными расширениями: (путь, содержимое). Лимиты [`MAX_SCAN_FILES`] /
/// [`MAX_FILE_BYTES`]; битые/не-UTF8 файлы пропускаются — эвристика не
/// роняется из-за одного файла. Отсутствующий каталог — пустой список.
/// Порядок стабилен (сортировка по пути).
fn read_texts(dir: &Path, extensions: &[&str]) -> Vec<(PathBuf, String)> {
    let mut out = Vec::new();
    if !dir.is_dir() {
        return out;
    }
    for entry in WalkDir::new(dir)
        .follow_links(false)
        .sort_by_file_name()
        .into_iter()
        .flatten()
    {
        if out.len() >= MAX_SCAN_FILES {
            break;
        }
        let path = entry.path();
        if !entry.file_type().is_file() {
            continue;
        }
        let ext = path
            .extension()
            .map(|e| e.to_string_lossy().to_ascii_lowercase());
        if !ext.is_some_and(|e| extensions.iter().any(|want| *want == e)) {
            continue;
        }
        // Файл пропал между обходом и чтением — пропускаем.
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };
        let cut = bytes.len().min(MAX_FILE_BYTES as usize);
        out.push((
            path.to_path_buf(),
            String::from_utf8_lossy(&bytes[..cut]).to_string(),
        ));
    }
    out
}

/// Компилирует внутренний (статически известный) regex детектора; сбой
/// компиляции — доменная ошибка, не паника (конвенция `control.rs`).
fn internal_re(pattern: &str) -> Result<Regex> {
    Regex::new(pattern)
        .map_err(|e| HarnessError::Control(format!("внутренний regex rules-suggest: {e}")))
}

/// YAML-фрагмент кандидатного правила `must_contain` для `rules:`.
fn yaml_must_contain(
    name: &str,
    glob: &str,
    pattern: &str,
    rationale: &str,
    fix_hint: &str,
    skill: &str,
) -> String {
    format!(
        "  - name: {name}\n    \
         type: must_contain\n    \
         glob: \"{glob}\"\n    \
         pattern: \"{pattern}\"\n    \
         severity: warn\n    \
         rationale: \"{rationale}\"\n    \
         fix_hint: \"{fix_hint}\"\n    \
         skill: {skill}"
    )
}

/// Детектор 1 (EARS): спецификации (`docs/spec/**/*.md`) и документы с
/// секцией «Критерии приёмки» есть, а EARS-формулировок (When/While/If/Where
/// в начале строки — нотация скилла readiness-gate) нет ни в одном.
fn detect_ears(case: &Path) -> Result<Option<Candidate>> {
    let mut files = read_texts(&case.join("docs/spec"), &["md"]);
    let acceptance = |t: &str| {
        t.contains("Критерии приёмки") || t.to_ascii_lowercase().contains("acceptance criteria")
    };
    for (path, text) in read_texts(&case.join("docs"), &["md"]) {
        if acceptance(&text) && !files.iter().any(|(p, _)| p == &path) {
            files.push((path, text));
        }
    }
    if files.is_empty() {
        return Ok(None);
    }
    // EARS-паттерн: ключевое слово в начале строки (форма требования), а не
    // слово в середине прозы — ложных срабатываний детектора меньше.
    let ears = internal_re(r"(?m)^\s*(When|While|If|Where)\s")?;
    if files.iter().any(|(_, t)| ears.is_match(t)) {
        return Ok(None);
    }
    Ok(Some(Candidate {
        id: "ears-acceptance-criteria".into(),
        rationale: format!(
            "спецификаций/документов с критериями приёмки: {} — ни одного \
             EARS-требования (When/While/If/Where): критерии приёмки \
             непроверяемы формально",
            files.len()
        ),
        source_skill: "readiness-gate".into(),
        yaml: Some(yaml_must_contain(
            "ears_acceptance_criteria",
            "docs/**/*.md",
            r"(?m)^\s*(When|While|If|Where)\s",
            "критерии приёмки без EARS-формы непроверяемы формально",
            "переписать критерии приёмки в EARS-нотации (When/While/If/Where)",
            "readiness-gate",
        )),
    }))
}

/// Детектор 2 (таймауты контрактов): файлы `docs/contracts/*`
/// (yaml/json/md) есть, а численных timeout/retry/deadline — ни в одном:
/// контракты без временных бюджетов (находка скилла adversarial-review:
/// «стык синхронный, таймаут не задан»).
fn detect_contract_timeouts(case: &Path) -> Result<Option<Candidate>> {
    let files = read_texts(&case.join("docs/contracts"), &["yaml", "yml", "json", "md"]);
    if files.is_empty() {
        return Ok(None);
    }
    // Численный бюджет: слово-маркер и число в одной строке (в любую сторону).
    let numeric = internal_re(
        r"(?i)(timeout|retry|retries|deadline|таймаут|дедлайн|повтор)[^\n]{0,48}\d|\d+\s*(ms|s|sec|seconds|мс|сек)\b",
    )?;
    if files.iter().any(|(_, t)| numeric.is_match(t)) {
        return Ok(None);
    }
    Ok(Some(Candidate {
        id: "contract-timeouts-numeric".into(),
        rationale: format!(
            "контрактов в docs/contracts: {} — ни одного численного \
             timeout/retry/deadline: временные бюджеты стыков не зафиксированы",
            files.len()
        ),
        source_skill: "adversarial-review".into(),
        yaml: Some(yaml_must_contain(
            "contract_timeouts_numeric",
            "docs/contracts/*",
            r"(?i)(timeout|retry|retries|deadline|таймаут|дедлайн|повтор)[^\n]{0,48}\d",
            "контракт без численных таймаутов/ретраев — каскадный сбой при деградации",
            "задать timeout/retry числом в каждом контракте стыка",
            "adversarial-review",
        )),
    }))
}

/// Детектор 3 (декомпозиция REQ→работы): в `model/` есть REQ-сущности, а
/// пунктов списка в `.arch-handoff/TASK.md` заметно меньше, чем требований.
/// Честный advisory без YAML: механическая проверка «REQ упомянут в TASK.md»
/// проверяла бы упоминание, а не покрытие (сироты возможны в обе стороны) —
/// ложная уверенность хуже честного ручного слежения.
fn detect_req_task(case: &Path) -> Result<Option<Candidate>> {
    let reqs = read_texts(&case.join("model"), &["md"])
        .into_iter()
        .filter(|(p, _)| {
            p.file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with("REQ-"))
        })
        .count();
    if reqs == 0 {
        return Ok(None);
    }
    let task_path = case.join(".arch-handoff/TASK.md");
    if !task_path.is_file() {
        return Ok(None);
    }
    // Файл пропал между проверкой и чтением — детектор молчит (не гейт).
    let Ok(task_text) = std::fs::read_to_string(&task_path) else {
        return Ok(None);
    };
    let item_re = internal_re(r"(?m)^\s*(?:[-*+]|\d+[.)])\s")?;
    let tasks = item_re.find_iter(&task_text).count();
    if tasks >= reqs {
        return Ok(None);
    }
    Ok(Some(Candidate {
        id: "req-task-decomposition".into(),
        rationale: format!(
            "требований REQ в model/: {reqs}, пунктов списка в \
             .arch-handoff/TASK.md: {tasks} — декомпозиция «требование → \
             работа» неполная. Механического правила нет: must_contain по \
             ссылкам REQ в TASK.md проверял бы упоминание, а не покрытие — \
             следите за прослеживаемостью вручную (поиск «сирот» с обеих \
             сторон, скилл readiness-gate)"
        ),
        source_skill: "readiness-gate".into(),
        yaml: None,
    }))
}

/// Детектор 4 (RTO/RPO → ADR): RTO/RPO упоминаются в документах/модели,
/// но ни один `docs/adr/*.md` их не содержит: целевые показатели
/// восстановления не закреплены архитектурным решением.
fn detect_rto_rpo_adr(case: &Path) -> Result<Option<Candidate>> {
    let rto = internal_re(r"\b(RTO|RPO)\b")?;
    let mut corpus = read_texts(&case.join("docs"), &["md"]);
    corpus.extend(read_texts(&case.join("model"), &["md"]));
    if !corpus.iter().any(|(_, t)| rto.is_match(t)) {
        return Ok(None);
    }
    let adrs = read_texts(&case.join("docs/adr"), &["md"]);
    if adrs.iter().any(|(_, t)| rto.is_match(t)) {
        return Ok(None);
    }
    Ok(Some(Candidate {
        id: "rto-rpo-adr".into(),
        rationale: "RTO/RPO заявлены в документах/модели, но не закреплены ни одним ADR — \
             показатели восстановления без архитектурного решения (при инциденте \
             нечему отвечать)"
            .into(),
        source_skill: "nfr-design".into(),
        yaml: Some(yaml_must_contain(
            "rto_rpo_backed_by_adr",
            "docs/adr/*.md",
            r"RTO|RPO",
            "RTO/RPO из NFR не закреплены ни одним ADR",
            "принять ADR о целевых RTO/RPO и способе восстановления",
            "nfr-design",
        )),
    }))
}

/// Детектор 5 (аудит операторских действий): тексты упоминают ручные
/// действия (разблокировка/оператор/вручную), но нет аудит-формулировок:
/// ручные вмешательства в эксплуатацию без требования журналирования.
fn detect_operator_audit(case: &Path) -> Result<Option<Candidate>> {
    let docs = read_texts(&case.join("docs"), &["md"]);
    if docs.is_empty() {
        return Ok(None);
    }
    let manual = internal_re(r"(?i)(разблокиров|оператор|вручную)")?;
    if !docs.iter().any(|(_, t)| manual.is_match(t)) {
        return Ok(None);
    }
    let audit = internal_re(r"(?i)(аудит|audit|журнал (действий|операций))")?;
    if docs.iter().any(|(_, t)| audit.is_match(t)) {
        return Ok(None);
    }
    Ok(Some(Candidate {
        id: "operator-actions-audit".into(),
        rationale: "документы упоминают ручные действия (разблокировка/оператор/вручную), \
             но аудит-формулировок (аудит/журнал действий) нет — операторские \
             вмешательства не журналируются, расследовать инциденты будет не по чему"
            .into(),
        source_skill: "fitness-functions".into(),
        yaml: Some(yaml_must_contain(
            "operator_actions_audited",
            "docs/**/*.md",
            r"(?i)(аудит|audit|журнал (действий|операций))",
            "ручные действия оператора без требования аудита — вмешательства нерасследуемы",
            "добавить требование журналирования операторских действий в эксплуатационные документы",
            "fitness-functions",
        )),
    }))
}

/// Прогоняет все детекторы по кейсу (корень — каталог с `docs/`, `model/`,
/// `.arch-handoff/`; читается read-only).
///
/// # Errors
/// Кейс недоступен (не каталог); внутренний regex детектора не скомпилировался.
pub fn suggest(case_dir: &Path) -> Result<SuggestReport> {
    if !case_dir.is_dir() {
        return Err(HarnessError::Control(format!(
            "rules-suggest: кейс недоступен (не каталог): {}",
            case_dir.display()
        )));
    }
    let mut candidates = Vec::new();
    for candidate in [
        detect_ears(case_dir)?,
        detect_contract_timeouts(case_dir)?,
        detect_req_task(case_dir)?,
        detect_rto_rpo_adr(case_dir)?,
        detect_operator_audit(case_dir)?,
    ]
    .into_iter()
    .flatten()
    {
        candidates.push(candidate);
    }
    let mechanizable = candidates.iter().filter(|c| c.yaml.is_some()).count();
    let summary = if candidates.is_empty() {
        "Кандидатов нет: пробелов по 5 детекторам не найдено.".to_string()
    } else {
        format!(
            "Кандидатов: {} (с готовым YAML: {mechanizable}, advisory: {}). \
             Взять правило в CONSTRAINTS.yaml и назначить severity — решение архитектора.",
            candidates.len(),
            candidates.len() - mechanizable
        )
    };
    Ok(SuggestReport {
        candidates,
        summary,
    })
}

/// Рендерит отчёт в markdown (пользовательский вывод
/// `arch-be control rules-suggest`, поле `report_markdown` MCP-вердикта).
#[must_use]
pub fn render_markdown(report: &SuggestReport) -> String {
    let mut out = String::new();
    // Запись в String не падает — игноры результата безопасны.
    let _ = writeln!(out, "# Кандидатные fitness-правила (rules-suggest)\n");
    let _ = writeln!(out, "{}\n", report.summary);
    if report.candidates.is_empty() {
        let _ = writeln!(
            out,
            "Пробелов по детекторам (EARS, таймауты контрактов, REQ→TASK, \
             RTO/RPO→ADR, аудит операторских действий) не найдено."
        );
        return out;
    }
    for c in &report.candidates {
        let _ = writeln!(out, "\n## {}\n", c.id);
        let _ = writeln!(out, "- Источник методики: скилл `{}`", c.source_skill);
        let _ = writeln!(out, "- Обоснование: {}", c.rationale);
        match &c.yaml {
            Some(yaml) => {
                let _ = writeln!(
                    out,
                    "\nФрагмент для `.arch-handoff/CONSTRAINTS.yaml` (под `rules:`):\n\n\
                     ```yaml\n{yaml}\n```"
                );
            }
            None => {
                let _ = writeln!(
                    out,
                    "\nМеханизируемого правила нет — advisory: отслеживать вручную \
                     (честный вариант вместо проверки, дающей ложную уверенность)."
                );
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Пустой кейс-фикстура без пробелов: docs/spec с EARS-критериями,
    /// контракт с таймаутом, ADR с RTO, документ с аудитом оператора.
    fn clean_case(root: &Path) -> PathBuf {
        let case = root.join("clean");
        std::fs::create_dir_all(case.join("docs/spec")).expect("spec");
        std::fs::create_dir_all(case.join("docs/contracts")).expect("contracts");
        std::fs::create_dir_all(case.join("docs/adr")).expect("adr");
        std::fs::create_dir_all(case.join(".arch-handoff")).expect("handoff");
        std::fs::write(
            case.join("docs/spec/payments.md"),
            "# Спека\n\n## Критерии приёмки\n\nWhen платёж принят, the шлюз shall \
             подтвердить за 200 мс.\n",
        )
        .expect("spec");
        std::fs::write(
            case.join("docs/contracts/tsp-api.yaml"),
            "info:\n  version: 1.0.0\n# timeout: 500 ms, retries: 3\n",
        )
        .expect("contract");
        std::fs::write(
            case.join("docs/adr/ADR-001.md"),
            "# ADR-001. Восстановление\n\nЦелевые RTO 15 мин и RPO 0 закрепляем.\n",
        )
        .expect("adr");
        std::fs::write(
            case.join("docs/runbook.md"),
            "# Ранбук\n\nРазблокировка вручную оператором; журнал действий оператора \
             ведётся (аудит).\n",
        )
        .expect("runbook");
        case
    }

    /// Кейс с пробелами по всем механизируемым детекторам: спека без EARS,
    /// контракт без таймаутов, RTO в NFR без ADR, ручные действия без аудита.
    fn gap_case(root: &Path) -> PathBuf {
        let case = root.join("gaps");
        std::fs::create_dir_all(case.join("docs/spec")).expect("spec");
        std::fs::create_dir_all(case.join("docs/contracts")).expect("contracts");
        std::fs::create_dir_all(case.join("docs/adr")).expect("adr");
        std::fs::create_dir_all(case.join("model")).expect("model");
        std::fs::create_dir_all(case.join(".arch-handoff")).expect("handoff");
        std::fs::write(
            case.join("docs/spec/payments.md"),
            "# Спека\n\n## Критерии приёмки\n\nСистема корректно обрабатывает платежи.\n",
        )
        .expect("spec");
        std::fs::write(
            case.join("docs/contracts/tsp-api.md"),
            "# Контракт мерчант-API\n\nСинхронный вызов, подтверждение приёма.\n",
        )
        .expect("contract");
        std::fs::write(
            case.join("docs/adr/ADR-001.md"),
            "# ADR-001. Брокер\n\nИспользуем брокер сообщений.\n",
        )
        .expect("adr");
        std::fs::write(
            case.join("docs/nfr.md"),
            "# NFR\n\nДоступность 99,9 %, RTO — по согласованию.\n",
        )
        .expect("nfr");
        std::fs::write(
            case.join("docs/runbook.md"),
            "# Ранбук\n\nОператор выполняет разблокировку вручную.\n",
        )
        .expect("runbook");
        std::fs::write(case.join("model/REQ-001.md"), "# REQ-001 Приём платежа\n").expect("req1");
        std::fs::write(case.join("model/REQ-002.md"), "# REQ-002 Возврат платежа\n").expect("req2");
        std::fs::write(case.join("model/REQ-003.md"), "# REQ-003 Сверка\n").expect("req3");
        std::fs::write(
            case.join(".arch-handoff/TASK.md"),
            "# Задача\n\n- Реализовать приём платежа\n",
        )
        .expect("task");
        case
    }

    /// Id кандидатов отчёта (хелпер).
    fn ids(report: &SuggestReport) -> Vec<&str> {
        report.candidates.iter().map(|c| c.id.as_str()).collect()
    }

    #[test]
    fn gap_case_yields_all_five_detectors() {
        let tmp = tempfile::tempdir().expect("tmp");
        let case = gap_case(tmp.path());
        let report = suggest(&case).expect("suggest");
        let ids = ids(&report);
        for want in [
            "ears-acceptance-criteria",
            "contract-timeouts-numeric",
            "req-task-decomposition",
            "rto-rpo-adr",
            "operator-actions-audit",
        ] {
            assert!(ids.contains(&want), "нет детектора {want}: {ids:?}");
        }
        // Механизируемые несут готовый YAML с именем правила и warn-severity;
        // advisory (REQ→TASK) — честный None.
        for c in &report.candidates {
            if c.id == "req-task-decomposition" {
                assert_eq!(c.yaml, None, "advisory без механики");
                assert!(c.rationale.contains("упоминание, а не покрытие"));
            } else {
                let yaml = c.yaml.as_deref().expect("yaml у механизируемого");
                assert!(yaml.contains("type: must_contain"), "{yaml}");
                assert!(yaml.contains("severity: warn"), "{yaml}");
            }
            assert!(!c.source_skill.is_empty(), "скилл-источник указан");
        }
        let md = render_markdown(&report);
        assert!(md.contains("## ears-acceptance-criteria"), "{md}");
        assert!(md.contains("```yaml"), "{md}");
        assert!(md.contains("advisory"), "{md}");
        assert!(md.contains("readiness-gate"), "{md}");
    }

    #[test]
    fn clean_case_yields_no_candidates() {
        let tmp = tempfile::tempdir().expect("tmp");
        let case = clean_case(tmp.path());
        let report = suggest(&case).expect("suggest");
        assert!(report.candidates.is_empty(), "{:?}", ids(&report));
        let md = render_markdown(&report);
        assert!(md.contains("Кандидатов нет"), "{md}");
    }

    #[test]
    fn partial_ears_presence_suppresses_detector() {
        // Один из двух документов уже с EARS — детектор молчит (правило
        // must_contain тоже прошло бы: искать надо хотя бы в одном файле).
        let tmp = tempfile::tempdir().expect("tmp");
        let case = clean_case(tmp.path());
        std::fs::write(
            case.join("docs/spec/extra.md"),
            "# Ещё спека\n\n## Критерии приёмки\n\nВсё корректно обрабатывается.\n",
        )
        .expect("spec");
        let report = suggest(&case).expect("suggest");
        assert!(
            !ids(&report).contains(&"ears-acceptance-criteria"),
            "{:?}",
            ids(&report)
        );
    }

    #[test]
    fn missing_case_dir_is_error_not_panic() {
        let tmp = tempfile::tempdir().expect("tmp");
        let err = suggest(&tmp.path().join("ghost")).expect_err("не каталог");
        assert!(err.to_string().contains("недоступен"), "{err}");
    }

    #[test]
    fn empty_case_yields_no_candidates() {
        // Пустой каталог — честный ноль (ни одного входа у детекторов).
        let tmp = tempfile::tempdir().expect("tmp");
        let report = suggest(tmp.path()).expect("suggest");
        assert!(report.candidates.is_empty());
    }
}
