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

/// Единый EARS-паттерн детектора 1 и эмитимого правила (дефект D3 живого
/// отчёта — «два диалекта EARS»: детектор искал голое `When` в начале строки,
/// а собственное предложенное правило и спеки кейсов принимали жирную форму
/// `- **When** …`, и кейс 011 с жирным EARS считался «без EARS»). Формы:
/// `When x`, `- When x`, `**While** y`, `- **When** x` (с отступами).
/// Детектор и правило обязаны говорить на одном паттерне: иначе правило
/// `must_contain` «пропускает» кейс, который детектор считает пробелом (или
/// наоборот — ложный кандидат на кейсе с EARS).
const EARS_PATTERN: &str = r"(?m)^\s*[-*]?\s*\**\s*(When|While|If|Where)\b";

/// Триггеры ручных действий детектора 5 — префиксы словоформ, case-insensitive
/// (дефект D4 живого отчёта: «разблокировка антифрода человеком» кейса
/// digital-ruble не совпала с формулировками словаря — детектор молчал):
/// «разблокировк», «вручную», «ручн(ая|ой|…)», «оператор», «сотрудник»,
/// «дежурн», «человек».
const MANUAL_ACTION_PATTERN: &str =
    r"(?i)(разблокировк|вручную|ручн(ая|ой|ое|ые|ого|ому|ым|ых)|оператор|сотрудник|дежурн|человек)";

/// Маркеры аудит-контура детектора 5 — общие для детектора и эмитимого
/// правила: «аудит» (покрывает и «аудиторский след»), «audit», «журнал»
/// (журнал действий/операций, журналируется). Голое «след» не берём —
/// «следует» давало бы шум. Баланс детектора (read-only эвристика уровня
/// warn-кандидата): пропуск хуже ложного срабатывания, но и фонтан
/// недопустим — словарь шире прежнего (`журнал (действий|операций)`), но
/// ограничен аудит-лексикой.
const AUDIT_TRAIL_PATTERN: &str = r"(?i)(аудит|audit|журнал)";

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
    /// Инвариант спайна, к которому привязан кандидат (`AD-004`). Заполняется
    /// только детектором исполняемых инвариантов; у остальных кандидатов поля
    /// нет вовсе (аддитивное поле, `skip_serializing_if`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ad: Option<String>,
    /// Шаблоны-кандидаты на исполняемую проверку этого инварианта, лучшие
    /// сверху (детектор на каждый инвариант, [`crate::rule_templates`]).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub templates: Vec<crate::rule_templates::TemplateMatch>,
}

impl Candidate {
    /// Кандидат без привязки к инварианту (детекторы 1–5 и легаси-кандидат).
    fn new(id: &str, rationale: String, source_skill: &str, yaml: Option<String>) -> Self {
        Self {
            id: id.to_string(),
            rationale,
            source_skill: source_skill.to_string(),
            yaml,
            ad: None,
            templates: Vec::new(),
        }
    }
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

/// Скаляр YAML в одинарных кавычках: внутри них нет escape-последовательностей,
/// поэтому regex с `\s`/`\d`/`[^\n]` доезжает до парсера литерально — в
/// ДВОЙНЫХ кавычках `\s` недопустимая escape-последовательность, и 2 из 3
/// «готовых фрагментов» отклонялись YAML-парсером (дефект D2 живого отчёта).
/// Единственное экранирование в одинарных кавычках — удвоение самой кавычки.
fn yaml_sq(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

/// YAML-фрагмент кандидатного правила `must_contain` для `rules:`.
/// Скаляры `glob`/`pattern`/`rationale`/`fix_hint` — в одинарных кавычках
/// ([`yaml_sq`]): фрагмент обязан парситься как YAML и загружаться боевой
/// схемой `control::load_fitness_rules` (охранный тест ниже). `name`/`skill`
/// — внутренние kebab-case константы, безопасны в plain-стиле.
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
         glob: {glob}\n    \
         pattern: {pattern}\n    \
         severity: warn\n    \
         rationale: {rationale}\n    \
         fix_hint: {fix_hint}\n    \
         skill: {skill}",
        glob = yaml_sq(glob),
        pattern = yaml_sq(pattern),
        rationale = yaml_sq(rationale),
        fix_hint = yaml_sq(fix_hint),
    )
}

/// Детектор 1 (EARS): спецификации (`docs/spec/**/*.md`) и документы с
/// секцией «Критерии приёмки» есть, а EARS-формулировок (When/While/If/Where
/// в начале строки — голой или жирной markdown-форме, [`EARS_PATTERN`]) нет
/// ни в одном.
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
    // слово в середине прозы — ложных срабатываний детектора меньше. Тот же
    // паттерн уезжает в предложенное правило — единый диалект (дефект D3).
    let ears = internal_re(EARS_PATTERN)?;
    if files.iter().any(|(_, t)| ears.is_match(t)) {
        return Ok(None);
    }
    Ok(Some(Candidate::new(
        "ears-acceptance-criteria",
        format!(
            "спецификаций/документов с критериями приёмки: {} — ни одного \
             EARS-требования (When/While/If/Where): критерии приёмки \
             непроверяемы формально",
            files.len()
        ),
        "readiness-gate",
        Some(yaml_must_contain(
            "ears_acceptance_criteria",
            "docs/**/*.md",
            EARS_PATTERN,
            "критерии приёмки без EARS-формы непроверяемы формально",
            "переписать критерии приёмки в EARS-нотации (When/While/If/Where)",
            "readiness-gate",
        )),
    )))
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
    // Численный бюджет: слово-маркер и число в одной строке (в любую сторону)
    // либо число с единицей времени. Детектор осознанно ШИРЕ эмитимого ниже
    // правила (там только «маркер и число»): подавлять кандидата может и
    // голая цифра с единицей, а правило-напоминание требует явного маркера.
    let numeric = internal_re(
        r"(?i)(timeout|retry|retries|deadline|таймаут|дедлайн|повтор)[^\n]{0,48}\d|\d+\s*(ms|s|sec|seconds|мс|сек)\b",
    )?;
    if files.iter().any(|(_, t)| numeric.is_match(t)) {
        return Ok(None);
    }
    Ok(Some(Candidate::new(
        "contract-timeouts-numeric",
        format!(
            "контрактов в docs/contracts: {} — ни одного численного \
             timeout/retry/deadline: временные бюджеты стыков не зафиксированы",
            files.len()
        ),
        "adversarial-review",
        Some(yaml_must_contain(
            "contract_timeouts_numeric",
            "docs/contracts/*",
            r"(?i)(timeout|retry|retries|deadline|таймаут|дедлайн|повтор)[^\n]{0,48}\d",
            "контракт без численных таймаутов/ретраев — каскадный сбой при деградации",
            "задать timeout/retry числом в каждом контракте стыка",
            "adversarial-review",
        )),
    )))
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
    Ok(Some(Candidate::new(
        "req-task-decomposition",
        format!(
            "требований REQ в model/: {reqs}, пунктов списка в \
             .arch-handoff/TASK.md: {tasks} — декомпозиция «требование → \
             работа» неполная. Механического правила нет: must_contain по \
             ссылкам REQ в TASK.md проверял бы упоминание, а не покрытие — \
             следите за прослеживаемостью вручную (поиск «сирот» с обеих \
             сторон, скилл readiness-gate)"
        ),
        "readiness-gate",
        None,
    )))
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
    Ok(Some(Candidate::new(
        "rto-rpo-adr",
        "RTO/RPO заявлены в документах/модели, но не закреплены ни одним ADR — \
             показатели восстановления без архитектурного решения (при инциденте \
             нечему отвечать)"
            .to_string(),
        "nfr-design",
        Some(yaml_must_contain(
            "rto_rpo_backed_by_adr",
            "docs/adr/*.md",
            r"RTO|RPO",
            "RTO/RPO из NFR не закреплены ни одним ADR",
            "принять ADR о целевых RTO/RPO и способе восстановления",
            "nfr-design",
        )),
    )))
}

/// Детектор 5 (аудит операторских действий): тексты упоминают ручные
/// действия (разблокировка/вручную/ручная/оператор/сотрудник/дежурный/
/// человек — [`MANUAL_ACTION_PATTERN`]), но нет аудит-формулировок
/// ([`AUDIT_TRAIL_PATTERN`]): ручные вмешательства в эксплуатацию без
/// требования журналирования.
fn detect_operator_audit(case: &Path) -> Result<Option<Candidate>> {
    let docs = read_texts(&case.join("docs"), &["md"]);
    if docs.is_empty() {
        return Ok(None);
    }
    let manual = internal_re(MANUAL_ACTION_PATTERN)?;
    if !docs.iter().any(|(_, t)| manual.is_match(t)) {
        return Ok(None);
    }
    let audit = internal_re(AUDIT_TRAIL_PATTERN)?;
    if docs.iter().any(|(_, t)| audit.is_match(t)) {
        return Ok(None);
    }
    Ok(Some(Candidate::new(
        "operator-actions-audit",
        "документы упоминают ручные действия (разблокировка/вручную/оператор/\
             сотрудник/дежурный), но аудит-формулировок (аудит/журнал действий) нет — \
             операторские вмешательства не журналируются, расследовать инциденты будет не по чему"
            .to_string(),
        "fitness-functions",
        Some(yaml_must_contain(
            "operator_actions_audited",
            "docs/**/*.md",
            AUDIT_TRAIL_PATTERN,
            "ручные действия оператора без требования аудита — вмешательства нерасследуемы",
            "добавить требование журналирования операторских действий в эксплуатационные документы",
            "fitness-functions",
        )),
    )))
}

/// Кандидат `executable-invariants` (Н10 волны C 0.3.4): в репозитории есть
/// исполняемые тесты, но ни одно правило реестра не запускает поведение —
/// весь контроль сводится к «документ содержит слово».
///
/// Правило на упоминание — звено ТРАССИРОВКИ, а не проверка смысла: оно
/// доказывает, что текст написан, и ничего не говорит о том, что код делает.
/// Пока в реестре нет ни одного `command_succeeds`, инварианты,
/// сформулированные в спайне, не проверяются исполнением.
fn detect_executable_invariants(case_dir: &Path) -> Option<Candidate> {
    // Исполняемые тесты: каталог с тестами или скелет с прогоном.
    let has_tests = ["tests", "test", "skeleton", "src/test"]
        .iter()
        .any(|rel| case_dir.join(rel).is_dir());
    if !has_tests {
        return None;
    }
    // Правила реестра: есть ли хоть одно, запускающее команду.
    let constraints = crate::control::resolve_constraints_path_detailed(case_dir, None);
    let resolved = constraints?;
    let text = std::fs::read_to_string(&resolved.path).ok()?;
    if text.contains("command_succeeds") {
        return None;
    }
    Some(Candidate::new(
        "executable-invariants",
        "в репозитории есть исполняемые тесты, но в CONSTRAINTS.yaml нет ни одного \
             правила `command_succeeds`: реестр проверяет только наличие слов в документах, \
             а инварианты спайна — не исполнением. Правило на упоминание — звено \
             трассировки, а не проверка смысла; добавьте хотя бы одно исполняемое правило"
            .to_string(),
        "fitness-functions",
        Some(
            "# Требуется решение архитектора: какую команду считать исполняемой проверкой.\n\
             # Пример формы (замените command на реальную команду проекта):\n\
             #   - id: C-100\n\
             #     name: invariants_executable\n\
             #     type: command_succeeds\n\
             #     command: \"cargo test --quiet\"\n\
             #     timeout_secs: 600\n\
             #     severity: error\n\
             #     owner: OWNER-001\n\
             #     fix_hint: \"сформулируйте инвариант спайна как тест, который падает при нарушении\"\n"
                .to_string(),
        ),
    ))
}

/// Детектор 7 (инвариант → исполняемое правило): по каждой сущности
/// `type: ad` модели — есть ли у неё хоть одно правило, проверяющее
/// ПОВЕДЕНИЕ ([`BEHAVIOUR_RULE_KINDS`]).
///
/// Легаси-кандидат [`detect_executable_invariants`] говорит о кейсе в целом и
/// замолкает после первого `command_succeeds`; этот — про каждый инвариант
/// поимённо и называет шаблон, которым пробел закрывается. Свойства:
///
/// - инвариант с обоснованным `unverifiable` — не пробел (решение архитектора);
/// - ни одного правила в `BEHAVIOUR_RULE_KINDS` → кандидат
///   `executable-invariant:AD-00N` с шаблонами по тексту спайна;
/// - паттерн не распознан → шаблон-заготовка и честная пометка;
/// - каталога `model/` нет → детектор молчит (инвариантов не видно).
///
/// # Errors
/// Внутренние регулярные выражения, реестр и модель — см.
/// [`crate::rule_templates::ad_coverage`].
fn detect_executable_invariant_per_ad(case_dir: &Path) -> Result<Vec<Candidate>> {
    use crate::rule_templates as rt;
    let Some(coverage) = rt::ad_coverage(case_dir)? else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    // Порядок задаёт `uncovered()`: несущие первыми, затем по id инварианта.
    for entry in coverage.uncovered() {
        if entry.unverifiable {
            continue;
        }
        let matched = rt::match_templates(&entry.spine_text, 2)?;
        let unrecognized = matched.is_empty();
        let templates = if unrecognized {
            vec![rt::TemplateMatch {
                id: rt::GENERIC_TEMPLATE_ID.to_string(),
                score: 0,
                matched: Vec::new(),
            }]
        } else {
            matched
        };
        let best = templates.first().map_or_else(
            || rt::GENERIC_TEMPLATE_ID.to_string(),
            |t| t.id.clone(),
        );
        let rules = if entry.rules.is_empty() {
            "правил в `verified_by` нет вовсе".to_string()
        } else {
            let named: Vec<String> = entry
                .rules
                .iter()
                .map(|r| match r.kind.as_deref() {
                    Some(kind) => format!("{} ({kind})", r.reference),
                    None => format!("{} (нет в реестре)", r.reference),
                })
                .collect();
            format!(
                "все {} его правил проверяют наличие текста: {}",
                named.len(),
                named.join(", ")
            )
        };
        let templates_note = if unrecognized {
            format!(
                "паттерн не распознан — предложена заготовка `{best}`: сформулируйте свойство \
                 и напишите тест"
            )
        } else {
            let names: Vec<String> = templates
                .iter()
                .map(|t| format!("{} (совпало: {})", t.id, t.matched.join(", ")))
                .collect();
            format!("шаблоны исполняемой проверки: {}", names.join("; "))
        };
        let bearing = if entry.load_bearing {
            "несущий инвариант — "
        } else {
            ""
        };
        let rationale = format!(
            "{bearing}{} «{}» не проверяется поведением: {rules}. Правило на упоминание — \
             звено трассировки, а не проверка смысла. {templates_note}. Применение шаблона: \
             `arch-be rules template apply <id> --ad {} --dir .`",
            entry.ad, entry.title, entry.ad
        );
        let yaml = rt::candidate_fragment(case_dir, &best, &entry.ad).ok();
        out.push(Candidate {
            id: format!("executable-invariant:{}", entry.ad),
            rationale,
            source_skill: "fitness-functions".to_string(),
            yaml,
            ad: Some(entry.ad.clone()),
            templates,
        });
    }
    Ok(out)
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
        detect_executable_invariants(case_dir),
    ]
    .into_iter()
    .flatten()
    {
        candidates.push(candidate);
    }
    // Детектор на каждый инвариант (Н10/executable-invariants): идёт после
    // общих детекторов, порядок внутри — несущие первыми, затем по id.
    candidates.extend(detect_executable_invariant_per_ad(case_dir)?);
    let mechanizable = candidates.iter().filter(|c| c.yaml.is_some()).count();
    let summary = if candidates.is_empty() {
        "Кандидатов нет: пробелов по 7 детекторам не найдено.".to_string()
    } else {
        format!(
            "Кандидатов: {} (с готовым YAML: {mechanizable}, advisory: {}; по инвариантам: {}). \
             Взять правило в CONSTRAINTS.yaml и назначить severity — решение архитектора.",
            candidates.len(),
            candidates.len() - mechanizable,
            candidates.iter().filter(|c| c.ad.is_some()).count()
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
             RTO/RPO→ADR, аудит операторских действий, исполняемые правила, \
             инвариант→исполняемое правило) не найдено."
        );
        return out;
    }
    for c in &report.candidates {
        let _ = writeln!(out, "\n## {}\n", c.id);
        let _ = writeln!(out, "- Источник методики: скилл `{}`", c.source_skill);
        if let Some(ad) = &c.ad {
            let _ = writeln!(out, "- Инвариант: {ad}");
        }
        let _ = writeln!(out, "- Обоснование: {}", c.rationale);
        if !c.templates.is_empty() {
            let list: Vec<String> = c
                .templates
                .iter()
                .map(|t| {
                    if t.score == 0 {
                        format!("`{}` (паттерн не распознан)", t.id)
                    } else {
                        format!("`{}` (совпадений: {})", t.id, t.score)
                    }
                })
                .collect();
            let _ = writeln!(out, "- Шаблоны исполняемой проверки: {}", list.join(", "));
            let _ = writeln!(
                out,
                "  Применить: `arch-be rules template apply {} --ad {} --dir <кейс>`",
                c.templates[0].id,
                c.ad.as_deref().unwrap_or("AD-N")
            );
        }
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

    #[test]
    fn generated_yaml_fragments_parse_and_load_into_control_schema() {
        // Дефект D2: «готовые фрагменты» с pattern в двойных кавычках YAML не
        // парсились (`\s` — недопустимая escape-последовательность): 2 из 3
        // кандидатов отклонялись парсером. Каждый сгенерированный фрагмент
        // всех детекторов обязан парситься serde_yaml_ng И приниматься боевой
        // схемой `control::load_fitness_rules` (прецедент — тест
        // `default_constraints_yaml_is_valid_for_every_stack` в handoff.rs).
        let tmp = tempfile::tempdir().expect("tmp");
        let case = gap_case(tmp.path());
        let report = suggest(&case).expect("suggest");
        let mut checked = 0_usize;
        for c in &report.candidates {
            let Some(yaml) = &c.yaml else {
                continue;
            };
            checked += 1;
            let doc = format!("rules:\n{yaml}\n");
            let parsed: serde_yaml_ng::Value = serde_yaml_ng::from_str(&doc)
                .unwrap_or_else(|e| panic!("{}: фрагмент не парсится: {e}\n{doc}", c.id));
            let rules = parsed["rules"]
                .as_sequence()
                .unwrap_or_else(|| panic!("{}: нет списка rules:\n{doc}", c.id));
            assert_eq!(rules.len(), 1, "{}:\n{doc}", c.id);
            let rule = &rules[0];
            for key in ["name", "type", "severity"] {
                assert!(rule[key].is_string(), "{}: нет '{key}':\n{doc}", c.id);
            }
            // Скаляр pattern доезжает до regex-движка без потерь: обратная
            // косая в одинарных кавычках YAML — литерал.
            let pattern = rule["pattern"]
                .as_str()
                .unwrap_or_else(|| panic!("{}: pattern не строка:\n{doc}", c.id));
            Regex::new(pattern)
                .unwrap_or_else(|e| panic!("{}: pattern не компилируется: {e}\n{pattern}", c.id));
            // Боевая схема (control::check): файл читается загрузчиком правил.
            let path = tmp.path().join(format!("CONSTRAINTS-{}.yaml", c.id));
            std::fs::write(&path, &doc).expect("write");
            let loaded = crate::control::load_fitness_rules(&path)
                .unwrap_or_else(|e| panic!("{}: схема не принимает: {e}\n{doc}", c.id));
            assert_eq!(loaded.len(), 1, "{}", c.id);
        }
        assert!(
            checked >= 4,
            "механизируемых кандидатов минимум 4 (EARS, таймауты, RTO/RPO, аудит): {checked}"
        );
        // Паттерн EARS-правила после парсинга — в точности детекторный (D3).
        let ears = report
            .candidates
            .iter()
            .find(|c| c.id == "ears-acceptance-criteria")
            .expect("ears-кандидат");
        let doc = format!("rules:\n{}\n", ears.yaml.as_deref().expect("yaml"));
        let parsed: serde_yaml_ng::Value = serde_yaml_ng::from_str(&doc).expect("parse");
        assert_eq!(
            parsed["rules"][0]["pattern"].as_str(),
            Some(EARS_PATTERN),
            "pattern искажён кавычками:\n{doc}"
        );
    }

    #[test]
    fn yaml_sq_escapes_single_quotes() {
        // Одинарная кавычка внутри значения удваивается — фрагмент остаётся
        // валидным YAML со скаляром без искажений.
        let doc = format!(
            "rules:\n{}\n",
            yaml_must_contain(
                "quoted",
                "docs/**/*.md",
                "don't panic",
                "rationale с 'кавычкой'",
                "fix",
                "skill-x",
            )
        );
        let parsed: serde_yaml_ng::Value = serde_yaml_ng::from_str(&doc).expect("parse");
        assert_eq!(
            parsed["rules"][0]["pattern"].as_str(),
            Some("don't panic"),
            "{doc}"
        );
        assert_eq!(
            parsed["rules"][0]["rationale"].as_str(),
            Some("rationale с 'кавычкой'"),
            "{doc}"
        );
    }

    #[test]
    fn ears_detector_recognizes_all_markdown_dialects() {
        // D3: единый диалект — детектор и эмитимое правило говорят на одном
        // паттерне; формы из реальных спек (кейс 011) узнаются.
        let re = internal_re(EARS_PATTERN).expect("regex");
        for form in [
            "When платёж принят, the шлюз shall подтвердить за 200 мс.",
            "- When платёж принят, the шлюз shall подтвердить.",
            "- **When** сторона публикует версию условия, **the** сервис **shall** …",
            "**While** платёж находится в состоянии `DISPUTED`, **the** движок **shall** …",
            "   - **Where** применяется растепливание, **the** система **shall** …",
            "  * If платформа не подтвердила исполнение, **then** …",
        ] {
            assert!(re.is_match(form), "форма не узнана: {form}");
        }
        for non in [
            "Система корректно обрабатывает платежи.",
            "Когда платёж принят, шлюз подтверждает.",
            "the шлюз shall подтвердить за 200 мс.",
            "Заметка о When в середине строки не считается.",
        ] {
            assert!(!re.is_match(non), "ложное срабатывание: {non}");
        }
    }

    #[test]
    fn bold_ears_case_yields_no_ears_candidate() {
        // Регрессия D3 по кейсу 011: EARS только в жирной форме — детектор
        // молчит (раньше считал такой кейс «без EARS» и давал ложного
        // кандидата, а собственное правило кейс проходил бы).
        let tmp = tempfile::tempdir().expect("tmp");
        let case = tmp.path().join("bold-ears");
        std::fs::create_dir_all(case.join("docs/spec")).expect("spec");
        std::fs::write(
            case.join("docs/spec/payments.md"),
            "# Спека\n\n## Критерии приёмки\n\n- **When** платёж принят, **the** шлюз **shall** \
             подтвердить за 200 мс.\n- **While** платёж в `DISPUTED`, **the** движок **shall** \
             приостановить исполнение.\n",
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
    fn operator_audit_gap_yields_candidate_and_audited_case_does_not() {
        // D4: «разблокировка человеком» без аудит-формулировок → кандидат;
        // те же ручные действия с контуром аудита → кандидата нет.
        let tmp = tempfile::tempdir().expect("tmp");
        let case = tmp.path().join("manual-no-audit");
        std::fs::create_dir_all(case.join("docs")).expect("docs");
        std::fs::write(
            case.join("docs/antifraud.md"),
            "# Антифрод\n\nРешение «стоп» блокирующее: разблокировка возможна только человеком.\n",
        )
        .expect("doc");
        let report = suggest(&case).expect("suggest");
        assert!(
            ids(&report).contains(&"operator-actions-audit"),
            "{:?}",
            ids(&report)
        );

        std::fs::write(
            case.join("docs/audit.md"),
            "# Аудит\n\nДействия оператора журналируются: кто, когда и что разблокировал — \
             аудиторский след вмешательств.\n",
        )
        .expect("doc");
        let report = suggest(&case).expect("suggest");
        assert!(
            !ids(&report).contains(&"operator-actions-audit"),
            "{:?}",
            ids(&report)
        );
    }

    #[test]
    fn operator_audit_dictionary_covers_word_forms() {
        // Словари D4: префиксы словоформ триггеров ручных действий и
        // маркеров аудита; «следует» и «выследить» шумом не становятся.
        let manual = internal_re(MANUAL_ACTION_PATTERN).expect("manual");
        for form in [
            "разблокировка антифрода человеком",
            "перезапуск выполняет дежурный администратор",
            "Сотрудник back-office подтверждает операцию",
            "ручная сверка выписок",
            "Ручного разбора не предусмотрено",
            "операция проводится вручную",
            "Решение принимает оператор",
        ] {
            assert!(manual.is_match(form), "триггер не узнан: {form}");
        }
        assert!(!manual.is_match("автоматическая сверка по расписанию"));
        let audit = internal_re(AUDIT_TRAIL_PATTERN).expect("audit");
        for form in [
            "журнал действий оператора",
            "аудиторский след вмешательств",
            "immutable audit trail",
            "каждое действие журналируется",
        ] {
            assert!(audit.is_match(form), "маркер не узнан: {form}");
        }
        assert!(
            !audit.is_match("следует выполнять сверку"),
            "«следует» — шум"
        );
    }
}

#[cfg(test)]
mod tests_executable_invariant_per_ad {
    //! Детектор «инвариант → исполняемое правило» (E4, ADR-050).

    use super::*;

    /// Кейс: спайн с инвариантами, модель и реестр правил.
    /// `ads` — (id, заголовок, дополнительные строки frontmatter).
    fn case_with(root: &Path, ads: &[(&str, &str, &str)], registry: &str) -> PathBuf {
        let case = root.join("case");
        std::fs::create_dir_all(case.join("model")).expect("model");
        let mut spine = String::from("# Спайн\n\n");
        for (id, title, extra) in ads {
            let num = id.trim_start_matches("AD-").trim_start_matches('0');
            std::fs::write(
                case.join("model").join(format!("{id}.md")),
                format!(
                    "---\nid: {id}\ntype: ad\ntitle: \"{title}\"\nstatus: \"ADOPTED\"\n{extra}\n\
                     ---\n\n- **Binds**: Приём\n- **Prevents**: потерю\n- **Rule**: правило\n"
                ),
            )
            .expect("сущность");
            let _ = write!(
                spine,
                "## AD-{num}. {title}\n\n- **Binds**: Приём\n- **Prevents**: потерю\n\
                 - **Rule**: правило\n\n"
            );
        }
        std::fs::write(case.join("ARCHITECTURE-SPINE.md"), spine).expect("спайн");
        std::fs::write(case.join("CONSTRAINTS.yaml"), registry).expect("реестр");
        case
    }

    /// Реестр: одно правило на упоминание (`must_contain`).
    const TEXT_ONLY: &str = "rules:\n  - id: C-002\n    name: word_in_doc\n    type: must_contain\n    \
         glob: 'docs/**/*.md'\n    pattern: 'ключ'\n    severity: error\n";
    /// Реестр: то же плюс исполняемое правило для второго инварианта.
    const WITH_BEHAVIOUR: &str = "rules:\n  - id: C-002\n    name: word_in_doc\n    type: must_contain\n    \
         glob: 'docs/**/*.md'\n    pattern: 'ключ'\n    severity: error\n  \
         - id: C-003\n    name: behaviour\n    type: command_succeeds\n    command: 'true'\n    \
         severity: error\n    ad: AD-002\n";

    /// Id кандидатов отчёта (хелпер сообщений).
    fn ids(report: &SuggestReport) -> Vec<&str> {
        report.candidates.iter().map(|c| c.id.as_str()).collect()
    }

    /// Кандидаты по инвариантам (без легаси-кандидата про кейс в целом).
    fn per_ad(report: &SuggestReport) -> Vec<&Candidate> {
        report
            .candidates
            .iter()
            .filter(|c| c.ad.is_some())
            .collect()
    }

    /// Все правила текстовые — на каждый инвариант по кандидату с шаблоном и
    /// незакомментированным фрагментом правила.
    #[test]
    fn suggests_per_ad_when_all_rules_textual() {
        let tmp = tempfile::tempdir().expect("tmp");
        let case = case_with(
            tmp.path(),
            &[
                (
                    "AD-001",
                    "Идемпотентность по ключу: повторная доставка",
                    "verified_by: [C-002]",
                ),
                (
                    "AD-002",
                    "Журнал только на дозапись",
                    "verified_by: [C-002]",
                ),
            ],
            TEXT_ONLY,
        );
        let report = suggest(&case).expect("suggest");
        let ads = per_ad(&report);
        assert_eq!(ads.len(), 2, "{:?}", ids(&report));
        assert_eq!(ads[0].id, "executable-invariant:AD-001");
        for c in &ads {
            assert_eq!(
                c.ad.as_deref(),
                Some(c.id.trim_start_matches("executable-invariant:"))
            );
            assert!(!c.templates.is_empty(), "шаблон предложен: {c:?}");
            assert!(c.templates[0].score > 0, "паттерн распознан: {c:?}");
            let yaml = c.yaml.as_deref().expect("фрагмент правила");
            assert!(yaml.contains("type: command_succeeds"), "{yaml}");
            assert!(
                !yaml.trim_start().starts_with('#'),
                "фрагмент обязан быть незакомментированным: {yaml}"
            );
            assert!(
                yaml.contains(&format!("ad: {}", c.ad.as_deref().unwrap())),
                "{yaml}"
            );
            // Свободный id — из реестра кейса: занятый C-002 не переиспользуем,
            // ниже сотни не занимаем (запас на библиотеку корпоративных правил).
            assert!(yaml.contains("id: C-100"), "{yaml}");
            assert!(!yaml.contains("id: C-002"), "{yaml}");
        }
    }

    /// Инвариант, у которого есть правило поведения, — не пробел.
    #[test]
    fn silent_for_ad_with_behaviour_rule() {
        let tmp = tempfile::tempdir().expect("tmp");
        let case = case_with(
            tmp.path(),
            &[
                ("AD-001", "Идемпотентность по ключу", "verified_by: [C-002]"),
                (
                    "AD-002",
                    "Журнал только на дозапись",
                    "verified_by: [C-003]",
                ),
            ],
            WITH_BEHAVIOUR,
        );
        let report = suggest(&case).expect("suggest");
        let ads: Vec<&str> = per_ad(&report)
            .iter()
            .map(|c| c.ad.as_deref().unwrap())
            .collect();
        assert_eq!(ads, vec!["AD-001"], "{:?}", ids(&report));
    }

    /// Обоснованный отказ от проверки (`unverifiable`) — решение архитектора,
    /// а не пробел: детектор по такому инварианту молчит.
    #[test]
    fn silent_for_unverifiable_ad() {
        let tmp = tempfile::tempdir().expect("tmp");
        let case = case_with(
            tmp.path(),
            &[(
                "AD-001",
                "Идемпотентность по ключу",
                "verified_by: [C-002]\nunverifiable: \"внешняя система не даёт наблюдаемости\"",
            )],
            TEXT_ONLY,
        );
        let report = suggest(&case).expect("suggest");
        assert!(per_ad(&report).is_empty(), "{:?}", ids(&report));
    }

    /// Подбор по тексту спайна: инвариант про идемпотентность получает
    /// `idempotency-key`, а не «первый по алфавиту».
    #[test]
    fn matches_idempotency_template_by_spine_text() {
        let tmp = tempfile::tempdir().expect("tmp");
        let case = case_with(
            tmp.path(),
            &[(
                "AD-001",
                "Выплата идемпотентна по ключу (реестр, строка)",
                "verified_by: [C-002]",
            )],
            TEXT_ONLY,
        );
        let report = suggest(&case).expect("suggest");
        let c = per_ad(&report)[0];
        assert_eq!(c.templates[0].id, "idempotency-key", "{:?}", c.templates);
        assert!(
            c.templates[0]
                .matched
                .iter()
                .any(|m| m.starts_with("идемпотент"))
        );
    }

    /// Паттерн не распознан — заготовка и честная пометка, а не выдуманный
    /// шаблон; правило в заготовке закомментировано (механики за ним нет).
    #[test]
    fn unknown_pattern_gets_generic_template() {
        let tmp = tempfile::tempdir().expect("tmp");
        let case = case_with(
            tmp.path(),
            &[(
                "AD-001",
                "Космический лифт: расписание запусков",
                "verified_by: [C-002]",
            )],
            TEXT_ONLY,
        );
        let report = suggest(&case).expect("suggest");
        let c = per_ad(&report)[0];
        assert_eq!(
            c.templates[0].id,
            crate::rule_templates::GENERIC_TEMPLATE_ID,
            "{:?}",
            c.templates
        );
        assert_eq!(c.templates[0].score, 0);
        assert!(
            c.rationale.contains("паттерн не распознан"),
            "{}",
            c.rationale
        );
        let yaml = c.yaml.as_deref().expect("фрагмент");
        assert!(
            yaml.contains("# Заготовка") && yaml.contains("#   - id:"),
            "фрагмент заготовки обязан быть закомментирован: {yaml}"
        );
    }

    /// Несущие инварианты идут первыми (ADR-050), затем — по id.
    #[test]
    fn load_bearing_ads_come_first() {
        let tmp = tempfile::tempdir().expect("tmp");
        let case = case_with(
            tmp.path(),
            &[
                ("AD-001", "Идемпотентность по ключу", "verified_by: [C-002]"),
                (
                    "AD-002",
                    "Журнал только на дозапись",
                    "load_bearing: true\nverified_by: [C-002]",
                ),
            ],
            TEXT_ONLY,
        );
        let report = suggest(&case).expect("suggest");
        let ads = per_ad(&report);
        assert_eq!(ads[0].ad.as_deref(), Some("AD-002"), "несущий — первым");
        assert!(ads[0].rationale.contains("несущий"), "{}", ads[0].rationale);
    }

    /// Нет каталога `model/` — детектор молчит (инвариантов не видно).
    #[test]
    fn silent_without_model_dir() {
        let tmp = tempfile::tempdir().expect("tmp");
        let case = tmp.path().join("no-model");
        std::fs::create_dir_all(&case).expect("кейс");
        std::fs::write(case.join("CONSTRAINTS.yaml"), TEXT_ONLY).expect("реестр");
        let report = suggest(&case).expect("suggest");
        assert!(per_ad(&report).is_empty());
    }

    /// Легаси-кандидат `executable-invariants` живёт на прежних условиях:
    /// он про кейс в целом и привязки к инварианту не несёт.
    #[test]
    fn legacy_candidate_unchanged() {
        let tmp = tempfile::tempdir().expect("tmp");
        let case = case_with(
            tmp.path(),
            &[("AD-001", "Идемпотентность по ключу", "verified_by: [C-002]")],
            TEXT_ONLY,
        );
        std::fs::create_dir_all(case.join("tests")).expect("tests");
        let report = suggest(&case).expect("suggest");
        let legacy = report
            .candidates
            .iter()
            .find(|c| c.id == "executable-invariants")
            .expect("легаси-кандидат на месте (тесты есть, command_succeeds нет)");
        assert!(legacy.ad.is_none(), "легаси не привязан к инварианту");
        assert!(legacy.templates.is_empty(), "и шаблонов не предлагает");
    }

    /// Снимок на эталонном кейсе: детектор называет КАЖДЫЙ инвариант, а для
    /// пяти инвариантов `кейсы/salary-payments` ожидаемый шаблон задан
    /// заданием и сверяется поимённо.
    #[test]
    fn salary_payments_snapshot_names_every_ad_with_its_template() {
        let case = Path::new(env!("CARGO_MANIFEST_DIR")).join("кейсы/salary-payments");
        let report = suggest(&case).expect("suggest");
        let mut got: Vec<(String, String)> = per_ad(&report)
            .iter()
            .map(|c| (c.ad.clone().unwrap_or_default(), c.templates[0].id.clone()))
            .collect();
        got.sort();
        let expect = [
            ("AD-001", "idempotency-key"),
            ("AD-002", "append-only-journal"),
            ("AD-003", "unknown-outcome-no-resend"),
            ("AD-004", "no-pii-in-logs"),
            ("AD-005", "validate-before-side-effect"),
            ("AD-006", "unknown-outcome-no-resend"),
            ("AD-007", "append-only-journal"),
        ];
        assert_eq!(got.len(), expect.len(), "названы все инварианты: {got:?}");
        for (ad, want) in expect {
            assert!(
                got.iter().any(|(a, t)| a == ad && t == want),
                "{ad} → {want}, получено {got:?}"
            );
        }
    }

    /// Снимок второго эталонного кейса: девять инвариантов, у одного паттерн
    /// честно не распознан (интеграция через адаптер — не из восьми паттернов).
    #[test]
    fn digital_ruble_merchant_snapshot_names_every_ad() {
        let case = Path::new(env!("CARGO_MANIFEST_DIR")).join("кейсы/digital-ruble-merchant");
        let report = suggest(&case).expect("suggest");
        let ads = per_ad(&report);
        assert_eq!(
            ads.len(),
            9,
            "девять инвариантов названы: {:?}",
            ids(&report)
        );
        let unrecognized = ads
            .iter()
            .filter(|c| c.templates[0].id == crate::rule_templates::GENERIC_TEMPLATE_ID)
            .count();
        assert_eq!(unrecognized, 1, "ровно один паттерн не распознан");
        for c in &ads {
            assert!(c.yaml.is_some(), "{} без фрагмента", c.id);
        }
    }
}

#[cfg(test)]
mod tests_executable_invariants {
    //! Н10 волны C 0.3.4: кандидат `executable-invariants`.

    use super::*;

    /// Кейс с тестами и реестром из одного правила на упоминание.
    fn case_with(root: &Path, constraints: &str, with_tests: bool) -> PathBuf {
        let case = root.join(if with_tests { "with-tests" } else { "no-tests" });
        let _ = std::fs::remove_dir_all(&case);
        std::fs::create_dir_all(case.join(".arch-handoff")).expect("handoff");
        if with_tests {
            std::fs::create_dir_all(case.join("tests")).expect("tests");
            std::fs::write(
                case.join("tests/a_test.py"),
                "def test_x():\n    assert 1\n",
            )
            .expect("test");
        }
        std::fs::write(case.join(".arch-handoff/CONSTRAINTS.yaml"), constraints)
            .expect("constraints");
        case
    }

    /// Тесты есть, `command_succeeds` нет — кандидат предлагается с YAML-заготовкой.
    #[test]
    fn suggests_when_tests_exist_and_no_command_rule() {
        let tmp = tempfile::tempdir().expect("tmp");
        let case = case_with(
            tmp.path(),
            "rules:\n  - id: C-001\n    name: doc_word\n    type: must_contain\n    \
             glob: \"docs/**/*.md\"\n    pattern: 'журнал'\n    severity: error\n",
            true,
        );
        let report = suggest(&case).expect("suggest");
        let hit = report
            .candidates
            .iter()
            .find(|c| c.id == "executable-invariants")
            .unwrap_or_else(|| panic!("нет кандидата: {:?}", report.candidates));
        assert!(hit.yaml.is_some(), "кандидат обязан нести YAML-заготовку");
        assert!(hit.rationale.contains("трассировки"));
    }

    /// Есть исполняемое правило — кандидат не предлагается.
    #[test]
    fn silent_when_command_rule_present() {
        let tmp = tempfile::tempdir().expect("tmp");
        let case = case_with(
            tmp.path(),
            "rules:\n  - id: C-001\n    name: tests\n    type: command_succeeds\n    \
             command: \"cargo test\"\n    severity: error\n",
            true,
        );
        let report = suggest(&case).expect("suggest");
        assert!(
            !report
                .candidates
                .iter()
                .any(|c| c.id == "executable-invariants"),
            "{:?}",
            report.candidates
        );
    }

    /// Тестов нет — предлагать исполняемое правило нечего.
    #[test]
    fn silent_without_executable_tests() {
        let tmp = tempfile::tempdir().expect("tmp");
        let case = case_with(
            tmp.path(),
            "rules:\n  - id: C-001\n    name: doc_word\n    type: must_contain\n    \
             glob: \"docs/**/*.md\"\n    pattern: 'журнал'\n    severity: error\n",
            false,
        );
        let report = suggest(&case).expect("suggest");
        assert!(
            !report
                .candidates
                .iter()
                .any(|c| c.id == "executable-invariants"),
            "{:?}",
            report.candidates
        );
    }
}
