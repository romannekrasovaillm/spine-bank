//! Дифф DDL-миграций (`.sql`, CD-S01..CD-S05): файл приводится к итоговому
//! состоянию (`CREATE TABLE` + `ALTER TABLE` + `DROP TABLE`), дифф — по
//! состояниям.

use std::collections::BTreeMap;

use super::types::{Finding, escape_segment};

/// Потолок операторов в одном DDL-файле (защита от гигантских дампов;
/// реальные миграции на порядки меньше).
const MAX_DDL_STATEMENTS: usize = 10_000;

/// Колонка в итоговом состоянии DDL.
#[derive(Debug, Clone)]
struct DdlColumn {
    /// Тип (нижний регистр, схлопнутые пробелы).
    typ: String,
    /// `NOT NULL` (или `PRIMARY KEY`).
    not_null: bool,
    /// Есть `DEFAULT`.
    has_default: bool,
}

/// Итоговое состояние схемы после применения всех операторов файла.
#[derive(Debug, Default)]
struct DdlSchema {
    /// Таблицы → колонки по имени.
    tables: BTreeMap<String, BTreeMap<String, DdlColumn>>,
}

/// Нормализация идентификатора SQL: срез кавычек/обратных кавычек/скобок,
/// нижний регистр (неквотированные идентификаторы PostgreSQL складываются
/// в нижний регистр; консервативно — для всех диалектов).
fn sql_ident(raw: &str) -> String {
    raw.trim()
        .trim_matches('"')
        .trim_matches('`')
        .trim_matches(['[', ']'])
        .to_ascii_lowercase()
}

/// Ключевые слова-ограничители в определении колонки (по ним обрезается тип).
const SQL_CONSTRAINT_WORDS: [&str; 10] = [
    "primary",
    "not",
    "null",
    "default",
    "references",
    "unique",
    "check",
    "constraint",
    "collate",
    "generated",
];

/// Разбор определения колонки (`name type [constraints]`).
fn parse_column_def(def: &str) -> Option<(String, DdlColumn)> {
    let tokens: Vec<&str> = def.split_whitespace().collect();
    let name = sql_ident(tokens.first()?);
    if SQL_CONSTRAINT_WORDS.contains(&name.as_str()) {
        return None; // табличный constraint, не колонка
    }
    let mut type_tokens: Vec<&str> = Vec::new();
    for tok in tokens.iter().skip(1) {
        let low = tok.to_ascii_lowercase();
        // Тип кончается первым словом-ограничителем (NOT NULL, DEFAULT, …).
        let stripped = low.trim_end_matches([',', ')']);
        if !type_tokens.is_empty() && SQL_CONSTRAINT_WORDS.contains(&stripped) {
            break;
        }
        type_tokens.push(tok);
    }
    let typ = type_tokens
        .join(" ")
        .to_ascii_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let rest = def.to_ascii_lowercase();
    Some((
        name,
        DdlColumn {
            typ,
            not_null: rest.contains("not null") || rest.contains("primary key"),
            has_default: rest.contains("default"),
        },
    ))
}

/// Разбивает тело `CREATE TABLE (...)` на определения по запятым верхнего
/// уровня (запятые внутри `varchar(…)`/check-скобок не режем).
fn split_top_level_commas(body: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut current = String::new();
    for ch in body.chars() {
        match ch {
            '(' => {
                depth += 1;
                current.push(ch);
            }
            ')' => {
                depth -= 1;
                current.push(ch);
            }
            ',' if depth == 0 => {
                if !current.trim().is_empty() {
                    out.push(current.trim().to_string());
                }
                current.clear();
            }
            _ => current.push(ch),
        }
    }
    if !current.trim().is_empty() {
        out.push(current.trim().to_string());
    }
    out
}

/// Приводит DDL-файл к итоговому состоянию схемы: `CREATE TABLE` +
/// `ALTER TABLE` (add/drop column, type, not null) + `DROP TABLE`.
/// Незнакомые операторы пропускаются (консервативный скелет).
fn parse_ddl(text: &str) -> DdlSchema {
    let create_re = regex::Regex::new(
        r"(?i)^\s*create\s+table\s+(?:if\s+not\s+exists\s+)?([^\s(]+)\s*\((.*)\)\s*$",
    );
    let alter_re =
        regex::Regex::new(r"(?i)^\s*alter\s+table\s+(?:if\s+exists\s+)?([^\s]+)\s+(.*)$");
    let drop_re = regex::Regex::new(r"(?i)^\s*drop\s+table\s+(?:if\s+exists\s+)?(.+)$");
    let (Ok(create_re), Ok(alter_re), Ok(drop_re)) = (create_re, alter_re, drop_re) else {
        return DdlSchema::default();
    };

    let mut schema = DdlSchema::default();
    for stmt in text.split(';').take(MAX_DDL_STATEMENTS) {
        // Однострочные комментарии -- срезаем построчно.
        let cleaned: String = stmt
            .lines()
            .map(|l| l.split("--").next().unwrap_or(""))
            .collect::<Vec<_>>()
            .join(" ");
        let stmt = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
        if stmt.is_empty() {
            continue;
        }
        if let Some(caps) = create_re.captures(&stmt) {
            let table = sql_ident(&caps[1]);
            let columns = schema.tables.entry(table).or_default();
            for def in split_top_level_commas(&caps[2]) {
                if let Some((name, col)) = parse_column_def(&def) {
                    columns.insert(name, col);
                }
            }
            continue;
        }
        if let Some(caps) = drop_re.captures(&stmt) {
            for name in caps[1].split(',') {
                let name = name.trim();
                if name.is_empty() {
                    continue;
                }
                // Отсекаем хвосты cascade/restrict.
                let name = name.split_whitespace().next().unwrap_or(name);
                schema.tables.remove(&sql_ident(name));
            }
            continue;
        }
        if let Some(caps) = alter_re.captures(&stmt) {
            let table = sql_ident(&caps[1]);
            let actions = split_top_level_commas(&caps[2]);
            let Some(columns) = schema.tables.get_mut(&table) else {
                continue; // alter несуществующей таблицы — пропускаем
            };
            for action in actions {
                apply_alter_action(columns, &action);
            }
        }
    }
    schema
}

/// Применяет одно действие `ALTER TABLE` к набору колонок.
fn apply_alter_action(columns: &mut BTreeMap<String, DdlColumn>, action: &str) {
    let low = action.to_ascii_lowercase();
    let tokens: Vec<&str> = action.split_whitespace().collect();
    if low.starts_with("add column ") || low.starts_with("add ") {
        let skip = if low.starts_with("add column ") { 2 } else { 1 };
        let Some(def) = tokens.get(skip..).map(|t| t.join(" ")) else {
            return;
        };
        // «if not exists» в определении пропускаем.
        let def = if def.to_ascii_lowercase().starts_with("if not exists ") {
            def.split_whitespace().skip(3).collect::<Vec<_>>().join(" ")
        } else {
            def
        };
        if let Some((name, col)) = parse_column_def(&def) {
            columns.insert(name, col);
        }
        return;
    }
    if low.starts_with("drop column ") || low.starts_with("drop ") {
        let skip = if low.starts_with("drop column ") {
            2
        } else {
            1
        };
        if let Some(name) = tokens.get(skip) {
            columns.remove(&sql_ident(name));
        }
        return;
    }
    if low.starts_with("alter column ") || low.starts_with("alter ") || low.starts_with("modify ") {
        // «ALTER COLUMN <имя> …» — имя третьим словом, «ALTER <имя> …» и
        // «MODIFY <имя> …» — вторым. До 0.3.11 индекс был жёстко вторым,
        // поэтому каноническая форма Postgres молча игнорировалась: именем
        // становилось слово COLUMN, а такой колонки в таблице нет.
        let after_name = if low.starts_with("alter column ") {
            3
        } else {
            2
        };
        let Some(raw_name) = tokens.get(after_name - 1) else {
            return;
        };
        let name = sql_ident(raw_name);
        let rest = tokens
            .get(after_name..)
            .map(|t| t.join(" "))
            .unwrap_or_default();
        let rest_low = rest.to_ascii_lowercase();
        let Some(col) = columns.get_mut(&name) else {
            return;
        };
        if let Some(pos) = rest_low.find(" type ") {
            // «SET DATA TYPE …» / «TYPE …»: тип до « using » или конца.
            let type_start = pos + " type ".len();
            let tail = &rest[type_start..];
            let tail = tail.split(" using ").next().unwrap_or(tail);
            col.typ = tail.trim().to_ascii_lowercase();
        } else if rest_low.starts_with("set not null") {
            col.not_null = true;
        } else if rest_low.starts_with("drop not null") {
            col.not_null = false;
        } else if low.starts_with("modify ") {
            // MySQL: MODIFY <name> <type> [constraints] — здесь `rest` уже
            // начинается с типа (имя вторым словом).
            let def = rest.clone();
            if let Some((_, new_col)) = parse_column_def(&format!("{name} {def}")) {
                *col = new_col;
            }
        }
    }
}

/// Скалярные совместимые расширения типов SQL (warn).
const SQL_SCALAR_WIDENINGS: &[(&str, &str)] = &[
    ("smallint", "int"),
    ("smallint", "integer"),
    ("smallint", "bigint"),
    ("int", "bigint"),
    ("integer", "bigint"),
    ("real", "double precision"),
    ("float", "double precision"),
    ("varchar", "text"),
];

/// Совместимые расширения типов SQL (warn): `int→bigint`, `smallint→int/bigint`,
/// `real→double precision`, `varchar(N→M≥N)`, `char(N→M≥N)`,
/// `numeric(p,s)→numeric(p'≥p,s'≥s)`.
fn sql_type_widening(old: &str, new: &str) -> bool {
    if old == new {
        return true;
    }
    if SQL_SCALAR_WIDENINGS.contains(&(old, new)) {
        return true;
    }
    // Параметризованные типы: name(a[, b]).
    if let (Some((on, op)), Some((nn, np))) = (sql_type_params(old), sql_type_params(new)) {
        if on != nn
            || !matches!(
                on,
                "varchar" | "char" | "character varying" | "numeric" | "decimal"
            )
        {
            return false;
        }
        return op.len() == np.len() && op.iter().zip(np.iter()).all(|(o, n)| n >= o);
    }
    false
}

/// Разбор параметризованного типа `name(a[, b])` → (имя, параметры).
fn sql_type_params(t: &str) -> Option<(&str, Vec<u64>)> {
    let (name, rest) = t.split_once('(')?;
    let rest = rest.trim_end_matches(')');
    let nums: Option<Vec<u64>> = rest
        .split(',')
        .map(|p| p.trim().parse::<u64>().ok())
        .collect();
    Some((name.trim(), nums?))
}

/// Дифф двух DDL-файлов (итоговых состояний): CD-S01..CD-S05.
pub(crate) fn diff_ddl(old: &str, new: &str) -> Vec<Finding> {
    let old_schema = parse_ddl(old);
    let new_schema = parse_ddl(new);
    let mut out = Vec::new();

    // CD-S01 (error): удалённая таблица; CD-S05 (warn): добавленная.
    for table in old_schema.tables.keys() {
        if !new_schema.tables.contains_key(table) {
            out.push(Finding {
                severity: "error".into(),
                rule: "CD-S01".into(),
                location: format!("#/ddl/table/{}", escape_segment(table)),
                message: format!("удалена таблица «{table}»"),
            });
        }
    }
    for (table, columns) in &new_schema.tables {
        if !old_schema.tables.contains_key(table) {
            out.push(Finding {
                severity: "warn".into(),
                rule: "CD-S05".into(),
                location: format!("#/ddl/table/{}", escape_segment(table)),
                message: format!("добавлена таблица «{table}» ({} колонок)", columns.len()),
            });
        }
    }

    for (table, old_cols) in &old_schema.tables {
        let Some(new_cols) = new_schema.tables.get(table) else {
            continue;
        };
        let loc = format!("#/ddl/table/{}", escape_segment(table));
        for (col, old_col) in old_cols {
            let cloc = format!("{loc}/column/{}", escape_segment(col));
            match new_cols.get(col) {
                // CD-S02 (error): удалённая колонка.
                None => out.push(Finding {
                    severity: "error".into(),
                    rule: "CD-S02".into(),
                    location: cloc,
                    message: format!("удалена колонка «{table}.{col}»"),
                }),
                Some(new_col) => {
                    // CD-S03 (error): несовместимая смена типа; расширение — warn.
                    if old_col.typ != new_col.typ {
                        if sql_type_widening(&old_col.typ, &new_col.typ) {
                            out.push(Finding {
                                severity: "warn".into(),
                                rule: "CD-S05".into(),
                                location: cloc.clone(),
                                message: format!(
                                    "колонка «{table}.{col}»: тип расширен {} → {}",
                                    old_col.typ, new_col.typ
                                ),
                            });
                        } else {
                            out.push(Finding {
                                severity: "error".into(),
                                rule: "CD-S03".into(),
                                location: cloc.clone(),
                                message: format!(
                                    "колонка «{table}.{col}»: несовместимая смена типа {} → {}",
                                    old_col.typ, new_col.typ
                                ),
                            });
                        }
                    }
                    // CD-S04 (error): стала NOT NULL без default; с default — warn.
                    if !old_col.not_null && new_col.not_null {
                        if new_col.has_default {
                            out.push(Finding {
                                severity: "warn".into(),
                                rule: "CD-S05".into(),
                                location: cloc.clone(),
                                message: format!(
                                    "колонка «{table}.{col}» стала NOT NULL (с DEFAULT — существующие строки заполнятся)"
                                ),
                            });
                        } else {
                            out.push(Finding {
                                severity: "error".into(),
                                rule: "CD-S04".into(),
                                location: cloc.clone(),
                                message: format!(
                                    "колонка «{table}.{col}» стала NOT NULL без DEFAULT — существующие NULL не пройдут"
                                ),
                            });
                        }
                    }
                    if old_col.not_null && !new_col.not_null {
                        out.push(Finding {
                            severity: "warn".into(),
                            rule: "CD-S05".into(),
                            location: cloc,
                            message: format!("колонка «{table}.{col}»: снято NOT NULL"),
                        });
                    }
                }
            }
        }
        for (col, new_col) in new_cols {
            if old_cols.contains_key(col) {
                continue;
            }
            let cloc = format!("{loc}/column/{}", escape_segment(col));
            // CD-S04 (error): добавлена NOT NULL колонка без default;
            // nullable/с default — CD-S05 (warn).
            if new_col.not_null && !new_col.has_default {
                out.push(Finding {
                    severity: "error".into(),
                    rule: "CD-S04".into(),
                    location: cloc,
                    message: format!(
                        "добавлена обязательная колонка «{table}.{col}» без DEFAULT — вставка в существующие строки невозможна"
                    ),
                });
            } else {
                out.push(Finding {
                    severity: "warn".into(),
                    rule: "CD-S05".into(),
                    location: cloc,
                    message: format!("добавлена колонка «{table}.{col}»"),
                });
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::diff_ddl;
    use crate::contract_diff::testkit::diff_text;
    use crate::tool::ToolOutput;

    const DDL_V1: &str = "CREATE TABLE charges (
  id varchar(36) NOT NULL,
  amount_minor bigint NOT NULL,
  note text
);

ALTER TABLE charges ADD COLUMN currency varchar(3);
";

    async fn diff_ddl_text(old: &str, new: &str) -> ToolOutput {
        diff_text(old, new, "old.sql", "new.sql").await
    }

    #[tokio::test]
    async fn ddl_identical_passes_clean() {
        let out = diff_ddl_text(DDL_V1, DDL_V1).await;
        assert!(!out.is_error, "{}", out.content);
        assert!(out.content.contains("Формат: ddl"), "{}", out.content);
        assert!(out.content.contains("Итог: PASS"), "{}", out.content);
    }

    #[tokio::test]
    async fn ddl_drop_column_and_table_are_breaking() {
        // Колонка note удалена из CREATE TABLE.
        let new = DDL_V1.replace(",\n  note text\n", "\n");
        let out = diff_ddl_text(DDL_V1, &new).await;
        assert!(out.content.contains("CD-S02"), "{}", out.content);
        assert!(out.content.contains("Итог: FAIL"), "{}", out.content);

        let new = "DROP TABLE charges;\n";
        let out = diff_ddl_text(DDL_V1, new).await;
        assert!(
            out.content.contains("[error] #/ddl/table/charges CD-S01"),
            "{}",
            out.content
        );
    }

    #[tokio::test]
    async fn ddl_not_null_without_default_breaking_with_default_warn() {
        // Существующая nullable-колонка стала NOT NULL без DEFAULT — error.
        let new = DDL_V1.replace("  note text\n", "  note text NOT NULL\n");
        let out = diff_ddl_text(DDL_V1, &new).await;
        assert!(out.content.contains("CD-S04"), "{}", out.content);
        assert!(out.content.contains("Итог: FAIL"), "{}", out.content);
        // С DEFAULT — warn.
        let new = DDL_V1.replace("  note text\n", "  note text NOT NULL DEFAULT ''\n");
        let out = diff_ddl_text(DDL_V1, &new).await;
        assert!(!out.content.contains("CD-S04"), "{}", out.content);
        assert!(out.content.contains("Итог: PASS"), "{}", out.content);
        // Новая обязательная колонка без DEFAULT — error.
        let new =
            format!("{DDL_V1}\nALTER TABLE charges ADD COLUMN trace_id varchar(36) NOT NULL;\n");
        let out = diff_ddl_text(DDL_V1, &new).await;
        assert!(out.content.contains("CD-S04"), "{}", out.content);
    }

    #[tokio::test]
    async fn ddl_type_change_incompatible_breaking_widening_warn() {
        // varchar(36) → int: несовместимо.
        let new = DDL_V1.replace("id varchar(36) NOT NULL", "id int NOT NULL");
        let out = diff_ddl_text(DDL_V1, &new).await;
        assert!(out.content.contains("CD-S03"), "{}", out.content);
        assert!(out.content.contains("Итог: FAIL"), "{}", out.content);
        // varchar(36) → varchar(64) и ALTER TYPE bigint→bigint через alter.
        let new = DDL_V1
            .replace("id varchar(36) NOT NULL", "id varchar(64) NOT NULL")
            .replace(
                "ADD COLUMN currency varchar(3);",
                "ADD COLUMN currency varchar(8);",
            );
        let out = diff_ddl_text(DDL_V1, &new).await;
        assert!(!out.content.contains("CD-S03"), "{}", out.content);
        assert!(out.content.contains("Итог: PASS"), "{}", out.content);
        // ALTER COLUMN через отдельный оператор: расширение того же типа
        // безопасно (varchar(36) → varchar(64)).
        let new =
            format!("{DDL_V1}\nALTER TABLE charges ALTER COLUMN id SET DATA TYPE varchar(64);\n");
        let out = diff_ddl_text(DDL_V1, &new).await;
        assert!(!out.content.contains("CD-S03"), "{}", out.content);
        // А несовместимая смена через тот же оператор ВИДНА: до 0.3.11 имя
        // колонки читалось вторым словом («COLUMN»), оператор игнорировался,
        // и такой дифф молча проходил зелёным.
        let new = format!(
            "{DDL_V1}\nALTER TABLE charges ALTER COLUMN amount_minor SET DATA TYPE varchar(4);\n"
        );
        let out = diff_ddl_text(DDL_V1, &new).await;
        assert!(out.content.contains("CD-S03"), "{}", out.content);
    }

    /// Поиск CD-S05 по конкретному сообщению (CD-S05 встречается и у
    /// добавленной таблицы, и у снятого NOT NULL).
    fn has_message(findings: &[crate::contract_diff::types::Finding], needle: &str) -> bool {
        findings.iter().any(|f| f.message.contains(needle))
    }

    /// Существующая в обеих версиях таблица не считается добавленной, а
    /// неизменившиеся колонки не дают «снято NOT NULL».
    #[test]
    fn unchanged_tables_and_columns_produce_no_findings() {
        let findings = diff_ddl(DDL_V1, DDL_V1);
        assert!(findings.is_empty(), "{findings:?}");
        // Таблица есть в обеих версиях — «добавлена таблица» не печатается.
        assert!(!has_message(&findings, "добавлена таблица"), "{findings:?}");
        assert!(!has_message(&findings, "снято NOT NULL"), "{findings:?}");
    }

    /// Добавленная nullable-колонка без DEFAULT — не CD-S04: ошибка только
    /// у обязательной колонки без значения по умолчанию.
    #[test]
    fn added_nullable_column_without_default_is_not_breaking() {
        let new = format!("{DDL_V1}\nALTER TABLE charges ADD COLUMN trace_id varchar(36);\n");
        let findings = diff_ddl(DDL_V1, &new);
        assert!(
            !findings.iter().any(|f| f.rule == "CD-S04"),
            "nullable-колонка без DEFAULT не ломает: {findings:?}"
        );
        assert!(
            has_message(&findings, "trace_id"),
            "колонка названа: {findings:?}"
        );
    }

    /// `ALTER COLUMN … SET DATA TYPE` действительно меняет тип: несовместимый
    /// тип даёт CD-S03, а не «тихое» игнорирование операнда.
    #[test]
    fn alter_column_set_data_type_is_applied() {
        let new = format!(
            "{DDL_V1}\nALTER TABLE charges ALTER COLUMN amount_minor SET DATA TYPE varchar(4);\n"
        );
        let findings = diff_ddl(DDL_V1, &new);
        assert!(
            findings.iter().any(|f| f.rule == "CD-S03"),
            "bigint → varchar(4) несовместимо: {findings:?}"
        );
        // Расширение через тот же оператор — не ошибка.
        let widen =
            format!("{DDL_V1}\nALTER TABLE charges ALTER COLUMN id SET DATA TYPE varchar(64);\n");
        let findings = diff_ddl(DDL_V1, &widen);
        assert!(
            !findings.iter().any(|f| f.rule == "CD-S03"),
            "varchar(36) → varchar(64) — расширение: {findings:?}"
        );
    }

    /// Параметризованные типы: разные имена типов — не расширение, даже если
    /// параметры совпали; разное число параметров — тоже не расширение.
    #[test]
    fn parameterized_type_widening_requires_same_name_and_params() {
        // varchar(5) → char(5): другое имя типа при тех же параметрах.
        let old = "CREATE TABLE t (\n  c varchar(5)\n);\n";
        let new = "CREATE TABLE t (\n  c char(5)\n);\n";
        let findings = diff_ddl(old, new);
        assert!(
            findings.iter().any(|f| f.rule == "CD-S03"),
            "смена имени типа — не widening: {findings:?}"
        );
        // numeric(5) → numeric(5,2): параметров стало больше.
        let old = "CREATE TABLE t (\n  c numeric(5)\n);\n";
        let new = "CREATE TABLE t (\n  c numeric(5,2)\n);\n";
        let findings = diff_ddl(old, new);
        assert!(
            findings.iter().any(|f| f.rule == "CD-S03"),
            "разное число параметров — не widening: {findings:?}"
        );
        // numeric(5,2) → numeric(9,2): расширение того же типа.
        let old = "CREATE TABLE t (\n  c numeric(5,2)\n);\n";
        let new = "CREATE TABLE t (\n  c numeric(9,2)\n);\n";
        let findings = diff_ddl(old, new);
        assert!(
            !findings.iter().any(|f| f.rule == "CD-S03"),
            "numeric(5,2) → numeric(9,2) — расширение: {findings:?}"
        );
    }

    /// Короткие формы действий `ALTER TABLE` без слова COLUMN: `ADD <col>`,
    /// `DROP <col>`, `ALTER <col> …` — каждая распознаётся сама по себе.
    #[test]
    fn short_alter_actions_without_column_keyword_are_applied() {
        // ADD <колонка> <тип> — колонка появляется.
        let added = format!("{DDL_V1}\nALTER TABLE charges ADD currency_code varchar(3);\n");
        let findings = diff_ddl(DDL_V1, &added);
        assert!(
            has_message(&findings, "currency_code"),
            "короткий ADD применён: {findings:?}"
        );
        // DROP <колонка> — колонка исчезает (CD-S02).
        let dropped = format!("{DDL_V1}\nALTER TABLE charges DROP note;\n");
        let findings = diff_ddl(DDL_V1, &dropped);
        assert!(
            findings
                .iter()
                .any(|f| f.rule == "CD-S02" && f.message.contains("note")),
            "короткий DROP применён: {findings:?}"
        );
        // ALTER <колонка> SET DATA TYPE — тип меняется.
        let altered =
            format!("{DDL_V1}\nALTER TABLE charges ALTER amount_minor SET DATA TYPE varchar(4);\n");
        let findings = diff_ddl(DDL_V1, &altered);
        assert!(
            findings.iter().any(|f| f.rule == "CD-S03"),
            "короткий ALTER применён: {findings:?}"
        );
    }
}
