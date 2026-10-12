//! Рендеры отчёта о восстановленной архитектуре (ADR-080, этап 1).
//!
//! [`render_json`] выдаёт машинный контракт `arch-be/report/v1` — сериализацию
//! [`ReportModel`] как есть: порядок полей фиксирован структурой, карты —
//! `BTreeMap`, поэтому вывод стабилен от прогона к прогону (правило 10).
//!
//! [`render_md`] собирает человекочитаемый скелет: все разделы §1–§14
//! (детерминированные наполнены, интерпретационные несут
//! `[Интерпретация Spine]`/`[gap]`), приложения A/C/D/G (B/E/F — этап 2) и
//! лист экспертной оценки. Всё строится только из модели — без часов и
//! случайности.

use std::collections::BTreeSet;
use std::fmt::Write as _;

use crate::error::Result;
use crate::report::model::{
    CheckSeverity, Family, ReportModel, ReportRelation, ReportSection, SectionKind,
};

/// Тире для пустых ячеек таблиц.
const DASH: &str = "—";

/// Машинный контракт `arch-be/report/v1`: сериализация модели (pretty).
///
/// # Errors
/// Ошибка сериализации JSON (практически недостижима для плоских структур).
pub fn render_json(model: &ReportModel) -> Result<String> {
    Ok(serde_json::to_string_pretty(model)?)
}

/// Текстовый отчёт автопроверки: сводка + построчный список находок.
///
/// Единственная точка рендера `check`-формата (AD-11): CLI (`report --format
/// check`) и MCP-мост (`report`) вызывают её — вывод обязан совпадать
/// байт-в-байт.
#[must_use]
pub fn render_check(model: &ReportModel) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "report/check: {} error, {} warn (schema {})",
        model.error_count(),
        model.warn_count(),
        model.schema
    );
    for c in &model.checks {
        let sev = match c.severity {
            CheckSeverity::Error => "error",
            CheckSeverity::Warn => "warn",
        };
        let who = c.element_id.as_deref().unwrap_or("-");
        let _ = writeln!(out, "[{sev}] {}/{}: {}", c.invariant, who, c.message);
    }
    out
}

/// Markdown-скелет отчёта: шапка, §1–§14, приложения и лист эксперта.
#[must_use]
pub fn render_md(model: &ReportModel) -> String {
    let mut out = String::new();
    render_header(&mut out, model);
    for section in &model.sections {
        match section.id.as_str() {
            "2" => render_legend(&mut out, section),
            "4" => {
                render_section(&mut out, model, section);
                render_context_mermaid(&mut out, model);
            }
            "6" => {
                render_section(&mut out, model, section);
                render_component_matrix(&mut out, model);
            }
            "14" => render_uncertainties(&mut out, model, section),
            _ => render_section(&mut out, model, section),
        }
    }
    render_appendices(&mut out, model);
    render_expert_sheet(&mut out, model);
    out
}

/// Шапка отчёта: система, ревизия, статус и сводная автопроверка.
fn render_header(out: &mut String, model: &ReportModel) {
    let m = &model.meta;
    let _ = writeln!(out, "# Отчёт о восстановленной архитектуре — {}", m.system);
    let _ = writeln!(out);
    let _ = writeln!(out, "> **Статус:** {}", m.status);
    let _ = writeln!(out, "> **Репозиторий:** {}", m.repo);
    if let Some(branch) = &m.branch {
        let _ = writeln!(out, "> **Ветка:** {branch}");
    }
    let _ = writeln!(out, "> **Ревизия:** {}", m.rev);
    let _ = writeln!(out, "> **Дата ревизии:** {}", m.generated_at);
    let _ = writeln!(
        out,
        "> **Схема:** `{}` (arch-be {})",
        model.schema, model.arch_be
    );
    let _ = writeln!(
        out,
        "> **Автопроверка (fail-closed, байпас-флага нет):** {} error / {} warn",
        model.error_count(),
        model.warn_count()
    );
    let dist = &m.confidence_distribution;
    let total: usize = dist.values().sum();
    let pct = |k: &str| -> String {
        let n = dist.get(k).copied().unwrap_or(0);
        match (n * 100).checked_div(total) {
            Some(p) => format!("{p}%"),
            None => "0%".to_string(),
        }
    };
    let _ = writeln!(
        out,
        "> **Уверенность A/B/C:** A {}/{} · B {}/{} · C {}/{} (всего {total})",
        dist.get("A").copied().unwrap_or(0),
        pct("A"),
        dist.get("B").copied().unwrap_or(0),
        pct("B"),
        dist.get("C").copied().unwrap_or(0),
        pct("C"),
    );
}

/// Раздел как заголовок + счётчики + таблица элементов + строки-пункты.
fn render_section(out: &mut String, model: &ReportModel, section: &ReportSection) {
    let kind = match section.kind {
        SectionKind::Deterministic => "детерминированный",
        SectionKind::Interpretive => "интерпретационный",
    };
    let _ = writeln!(out, "\n## §{}. {}", section.id, section.title);
    let _ = writeln!(out, "\n_{kind} раздел_");
    if !section.counts.is_empty() {
        let pairs: Vec<String> = section
            .counts
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect();
        let _ = writeln!(out, "\n`{}`", pairs.join(" · "));
    }
    if !section.element_ids.is_empty() {
        render_element_table(out, model, &section.element_ids);
    }
    render_items(out, &section.items);
    if let Some(ph) = &section.placeholder {
        let _ = writeln!(out, "\n> {ph}");
    }
}

/// Таблица элементов по их report-ID (читает заголовок/уверенность/основания).
fn render_element_table(out: &mut String, model: &ReportModel, ids: &[String]) {
    let _ = writeln!(out, "\n| ID | Заголовок | Уверенность | Основания |");
    let _ = writeln!(out, "|----|-----------|-------------|-----------|");
    for id in ids {
        if let Some(e) = model.element(id) {
            let ev = join_or_dash(&e.evidence);
            let _ = writeln!(
                out,
                "| {} | {} | {} | {} |",
                e.id,
                escape_cell(&e.title),
                e.confidence.as_str(),
                escape_cell(&ev)
            );
        }
    }
}

/// Список строк-пунктов (bullet) со ссылками и пометками.
fn render_items(out: &mut String, items: &[crate::report::model::SectionItem]) {
    for item in items {
        let refs = if item.refs.is_empty() {
            String::new()
        } else {
            format!(" _({})_", item.refs.join(", "))
        };
        let note = item
            .note
            .as_ref()
            .map_or(String::new(), |n| format!(" `[{n}]`"));
        let _ = writeln!(out, "- {}{refs}{note}", item.label);
    }
}

/// §2 Легенда: значения уверенности и условные обозначения (статичная).
fn render_legend(out: &mut String, section: &ReportSection) {
    let _ = writeln!(out, "\n## §{}. {}", section.id, section.title);
    let _ = writeln!(out, "\n_детерминированный раздел_");
    let _ = writeln!(
        out,
        "\n- **A** — элемент опирается на модель (`model_id`) или подтверждённое ребро."
    );
    let _ = writeln!(
        out,
        "- **B** — элемент выведен сканером/эвристикой (манифест, конфиг)."
    );
    let _ = writeln!(
        out,
        "- **C** — пробел (`[gap]`), кандидат в решение или LLM-вывод."
    );
    let _ = writeln!(
        out,
        "- **Основания** — адреса `путь:строка` в дереве ревизии; для A/B обязательны."
    );
    let _ = writeln!(
        out,
        "- **fail-closed** — любая находка `error` блокирует выдачу; байпас-флага нет."
    );
}

/// §14 Неопределённости: 14.1 C-элементы, 14.2 пробелы, 14.3 промпт, 14.4 метод.
fn render_uncertainties(out: &mut String, model: &ReportModel, section: &ReportSection) {
    let _ = writeln!(out, "\n## §{}. {}", section.id, section.title);
    let _ = writeln!(out, "\n_интерпретационный раздел_");
    let _ = writeln!(out, "\n### 14.1. Элементы с уверенностью C");
    let c_items: Vec<crate::report::model::SectionItem> = section.items.clone();
    if c_items.is_empty() {
        let _ = writeln!(out, "\n_{DASH} нет элементов с уверенностью C_");
    } else {
        render_items(out, &c_items);
    }
    let _ = writeln!(out, "\n### 14.2. Пробелы ([gap])");
    let gaps: Vec<&crate::report::model::ReportElement> = model
        .elements
        .iter()
        .filter(|e| e.props.get("source").is_some_and(|s| s == "gap"))
        .collect();
    if gaps.is_empty() {
        let _ = writeln!(out, "\n_{DASH} пробелов не зафиксировано_");
    } else {
        for e in gaps {
            let _ = writeln!(out, "- {}: {}", e.id, e.title);
        }
    }
    let _ = writeln!(out, "\n### 14.3. Вопросы к архитектору");
    let _ = writeln!(
        out,
        "\n> [gap] Заполните: какие внешние системы, компоненты и решения не отражены механически?"
    );
    let _ = writeln!(out, "\n### 14.4. Ограничения метода");
    let _ = writeln!(
        out,
        "\n> Отчёт восстановлен механически из графа `recover/v1`, обследования и сборщика"
    );
    let _ = writeln!(
        out,
        "> фактов; он не верифицирован человеком. Полнота ограничена доступными"
    );
    let _ = writeln!(
        out,
        "> артефактами (манифесты, конфиги, доки). Семантику, скрытые инварианты и"
    );
    let _ = writeln!(out, "> исторические решения уточняет архитектор.");
    if let Some(ph) = &section.placeholder {
        let _ = writeln!(out, "\n> {ph}");
    }
}

/// Mermaid-диаграмма контекста (§4): внешние системы и их связи.
fn render_context_mermaid(out: &mut String, model: &ReportModel) {
    let ctx_ids: BTreeSet<&str> = model
        .elements_of(Family::Context)
        .iter()
        .map(|e| e.id.as_str())
        .collect();
    let _ = writeln!(out, "\n### Диаграмма контекста");
    if ctx_ids.is_empty() && model.relations.is_empty() {
        let _ = writeln!(out, "\n_{DASH} нет внешних систем и связей_");
        return;
    }
    let _ = writeln!(out, "\n```mermaid");
    let _ = writeln!(out, "flowchart LR");
    let mut declared: BTreeSet<String> = BTreeSet::new();
    for e in model.elements_of(Family::Context) {
        let _ = writeln!(
            out,
            "  {}([\"{}\"])",
            mermaid_id(&e.id),
            mermaid_label(&e.title)
        );
        declared.insert(e.id.clone());
    }
    for r in &model.relations {
        if !(ctx_ids.contains(r.from.as_str()) || ctx_ids.contains(r.to.as_str())) {
            continue;
        }
        for id in [&r.from, &r.to] {
            if declared.insert(id.clone()) {
                if let Some(e) = model.element(id) {
                    let _ = writeln!(
                        out,
                        "  {}([\"{}\"])",
                        mermaid_id(id),
                        mermaid_label(&e.title)
                    );
                }
            }
        }
        let _ = writeln!(
            out,
            "  {} -->|{}| {}",
            mermaid_id(&r.from),
            mermaid_label(&r.kind),
            mermaid_id(&r.to)
        );
    }
    let _ = writeln!(out, "```");
}

/// Матрица import-зависимостей между компонентами (§6).
fn render_component_matrix(out: &mut String, model: &ReportModel) {
    let comps: BTreeSet<&str> = model
        .elements_of(Family::Component)
        .iter()
        .map(|e| e.id.as_str())
        .collect();
    let imports: Vec<&ReportRelation> = model
        .relations
        .iter()
        .filter(|r| comps.contains(r.from.as_str()) && comps.contains(r.to.as_str()))
        .collect();
    let _ = writeln!(out, "\n### Матрица зависимостей компонентов");
    if imports.is_empty() {
        let _ = writeln!(out, "\n_{DASH} нет рёбер между компонентами_");
        return;
    }
    let mut involved: BTreeSet<String> = BTreeSet::new();
    for r in &imports {
        involved.insert(r.from.clone());
        involved.insert(r.to.clone());
    }
    let cols: Vec<String> = involved.into_iter().collect();
    let _ = write!(out, "\n| from \\ to |");
    for c in &cols {
        let _ = write!(out, " {c} |");
    }
    let _ = writeln!(out);
    let _ = write!(out, "|---|");
    for _ in &cols {
        let _ = write!(out, "---|");
    }
    let _ = writeln!(out);
    for f in &cols {
        let _ = write!(out, "| {f} |");
        for t in &cols {
            let mark = if imports.iter().any(|r| r.from == *f && r.to == *t) {
                "•"
            } else {
                ""
            };
            let _ = write!(out, " {mark} |");
        }
        let _ = writeln!(out);
    }
}

/// Приложения: A (трассируемость), C (связи), D (зависимости), G (параметры);
/// B/E/F — заголовки «этап 2».
fn render_appendices(out: &mut String, model: &ReportModel) {
    let _ = writeln!(out, "\n---\n\n# Приложения");

    let _ = writeln!(out, "\n## Приложение A. Трассируемость");
    let _ = writeln!(
        out,
        "\n| ID | Семейство | graph_id | Уверенность | Основания |"
    );
    let _ = writeln!(
        out,
        "|----|-----------|----------|-------------|-----------|"
    );
    for e in &model.elements {
        let gid = e.graph_id.as_deref().unwrap_or(DASH);
        let _ = writeln!(
            out,
            "| {} | {} | {} | {} | {} |",
            e.id,
            e.family.label(),
            escape_cell(gid),
            e.confidence.as_str(),
            escape_cell(&join_or_dash(&e.evidence))
        );
    }

    let _ = writeln!(out, "\n## Приложение B. Схема данных _(этап 2)_");
    let _ = writeln!(out, "\n> [этап 2]");

    let _ = writeln!(out, "\n## Приложение C. Связи (рёбра → report-ID)");
    if model.relations.is_empty() {
        let _ = writeln!(out, "\n_{DASH} связей нет_");
    } else {
        let _ = writeln!(out, "\n| Из | Вид | В | Основания |");
        let _ = writeln!(out, "|----|-----|---|-----------|");
        for r in &model.relations {
            let _ = writeln!(
                out,
                "| {} | {} | {} | {} |",
                r.from,
                escape_cell(&r.kind),
                r.to,
                escape_cell(&join_or_dash(&r.evidence))
            );
        }
    }

    let _ = writeln!(out, "\n## Приложение D. Зависимости");
    let _ = writeln!(
        out,
        "\nРёбра графа `recover/v1` между элементами; обогащение из lockfiles (`collect_lockfiles`) — этап 2."
    );
    if model.relations.is_empty() {
        let _ = writeln!(out, "\n_{DASH} зависимостей нет_");
    } else {
        for r in &model.relations {
            let _ = writeln!(out, "- {} → {} ({})", r.from, r.to, r.kind);
        }
    }

    let _ = writeln!(out, "\n## Приложение E. Матрица трассируемости _(этап 2)_");
    let _ = writeln!(out, "\n> [этап 2]");
    let _ = writeln!(out, "\n## Приложение F. Сквозной аудит _(этап 2)_");
    let _ = writeln!(out, "\n> [этап 2]");

    let _ = writeln!(out, "\n## Приложение G. Параметры прогона");
    let _ = writeln!(out, "\n| Параметр | Значение |");
    let _ = writeln!(out, "|----------|----------|");
    let m = &model.meta;
    for (k, v) in [
        ("schema", model.schema.as_str()),
        ("arch_be", model.arch_be.as_str()),
        ("system", m.system.as_str()),
        ("repo", m.repo.as_str()),
        ("branch", m.branch.as_deref().unwrap_or(DASH)),
        ("rev", m.rev.as_str()),
        ("generated_at", m.generated_at.as_str()),
        ("status", m.status.as_str()),
    ] {
        let _ = writeln!(out, "| {k} | {} |", escape_cell(v));
    }
    let dist = &m.confidence_distribution;
    let dist_line: Vec<String> = dist.iter().map(|(k, v)| format!("{k}={v}")).collect();
    let _ = writeln!(out, "| confidence | {} |", dist_line.join(" · "));
    let stats: Vec<String> = m
        .statistics
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect();
    if !stats.is_empty() {
        let _ = writeln!(out, "| statistics | {} |", stats.join(" · "));
    }
}

/// Лист экспертной оценки: таблица по разделам + вопросы.
fn render_expert_sheet(out: &mut String, model: &ReportModel) {
    let _ = writeln!(out, "\n---\n\n# Лист экспертной оценки");
    let _ = writeln!(
        out,
        "\nОцените каждый раздел: полнота, корректность, полезность (1–5), ложные ID, пропущенные сущности."
    );
    let _ = writeln!(
        out,
        "\n| Раздел | Полнота | Корректность | Полезность | Ложные (ID) | Пропущенные | Комментарий |"
    );
    let _ = writeln!(
        out,
        "|--------|---------|--------------|-----------|-------------|-------------|-------------|"
    );
    for s in &model.sections {
        let _ = writeln!(out, "| §{}. {} | | | | | | |", s.id, s.title);
    }
    let _ = writeln!(out, "\n### Итоговые вопросы");
    let _ = writeln!(
        out,
        "\n1. Сводка (§1) соответствует вашему пониманию системы?"
    );
    let _ = writeln!(
        out,
        "2. Верны ли границы контекста (§4): что лишнее, что пропущено?"
    );
    let _ = writeln!(
        out,
        "3. Верна ли декомпозиция на контейнеры и компоненты (§5–§6)?"
    );
    let _ = writeln!(
        out,
        "4. Достоверны ли интерфейсы (§8) и автопути сценариев (§9)?"
    );
    let _ = writeln!(out, "5. Какие риски (§13) ложные, а какие упущены?");
    let _ = writeln!(
        out,
        "6. Какие разделы наиболее и наименее полезны для передачи системы?"
    );
}

/// Склеивает ссылки через запятую или отдаёт тире.
fn join_or_dash(values: &[String]) -> String {
    if values.is_empty() {
        DASH.to_string()
    } else {
        values.join(", ")
    }
}

/// Экранирует содержимое ячейки таблицы (`|` и перевод строки).
fn escape_cell(s: &str) -> String {
    s.replace('|', "\\|").replace('\n', " ")
}

/// Идентификатор узла mermaid: только алфанумерика из report-ID.
fn mermaid_id(id: &str) -> String {
    id.chars()
        .filter(|c: &char| c.is_ascii_alphanumeric())
        .collect()
}

/// Метка узла mermaid: кавычки/скобки вычищаются.
fn mermaid_label(s: &str) -> String {
    s.replace('"', "'").replace(['[', ']'], " ")
}
