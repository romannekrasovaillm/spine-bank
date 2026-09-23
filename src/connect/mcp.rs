//! MCP-конфиги хостов (B1): описание сервера `spine`, мердж в JSON-конфиги
//! хостов (`.mcp.json`, `settings.json`), TOML-блок и мердж для Codex.

use std::path::Path;

use serde_json::{Value, json};

use super::files::{backup_once, commit_file, read_json_object};
use super::types::ConnectReport;
use crate::error::{HarnessError, Result};

/// Имя нашего MCP-сервера в конфигах хостов.
const MCP_SERVER_NAME: &str = "spine";

/// Описание нашего MCP-сервера для JSON-конфигов хостов.
fn mcp_server_value(rw: bool, rw_reports: bool) -> Value {
    json!({"command": "arch-be", "args": mcp_server_args(rw, rw_reports)})
}

/// Аргументы запуска MCP-сервера: без флага — read-only, `--rw` — полный
/// контур записи, `--rw=reports` — только отчёты рубрики (J7).
fn mcp_server_args(rw: bool, rw_reports: bool) -> Vec<&'static str> {
    if rw_reports {
        vec!["mcp", "serve", "--rw=reports"]
    } else if rw {
        vec!["mcp", "serve", "--rw"]
    } else {
        vec!["mcp", "serve"]
    }
}

/// Режим подключения одним словом — для подсказок и `doctor --host` (J7).
#[must_use]
pub fn mode_label(rw: bool, rw_reports: bool) -> &'static str {
    if rw_reports {
        "rw=reports"
    } else if rw {
        "rw"
    } else {
        "read-only"
    }
}

/// Мердж `mcpServers.spine` в JSON-конфиг хоста (claude/omp `.mcp.json`,
/// qwen `.qwen/settings.json`, kimi `.kimi-code/mcp.json` проекта и
/// `~/.kimi-code/mcp.json`): чужие ключи верхнего уровня и чужие серверы
/// сохраняются, перезаписывается только наш сервер. `backup` — для
/// пользовательских конфигов (`--apply-global`).
// Четыре булевых флага подключения (режим записи, бэкап, dry-run) —
// независимые опции одного шага, а не состояние: их разбор в структуру
// опций — отдельная задача, здесь это ухудшило бы читаемость вызова.
#[allow(clippy::fn_params_excessive_bools)]
pub(super) fn merge_mcp_servers_json(
    path: &Path,
    rw: bool,
    rw_reports: bool,
    backup: bool,
    dry_run: bool,
    report: &mut ConnectReport,
) -> Result<()> {
    let (mut root, old) = read_json_object(path)?;
    let servers_value = root
        .entry("mcpServers".to_string())
        .or_insert_with(|| Value::Object(serde_json::Map::new()));
    let Some(servers) = servers_value.as_object_mut() else {
        return Err(HarnessError::Config(format!(
            "{}: ключ mcpServers не является объектом — не затираю",
            path.display()
        )));
    };
    let foreign: Vec<String> = servers
        .keys()
        .filter(|k| k.as_str() != MCP_SERVER_NAME)
        .cloned()
        .collect();
    servers.insert(
        MCP_SERVER_NAME.to_string(),
        mcp_server_value(rw, rw_reports),
    );
    if !foreign.is_empty() {
        report.notes.push(format!(
            "{}: существующие MCP-серверы сохранены: {}",
            path.display(),
            foreign.join(", ")
        ));
    }
    let new = match serde_json::to_string_pretty(&Value::Object(root)) {
        Ok(text) => format!("{text}\n"),
        Err(e) => return Err(HarnessError::Json(e)),
    };
    if backup {
        if let Some(prev) = old.as_deref() {
            if prev != new {
                backup_once(path, prev, dry_run, report)?;
            }
        }
    }
    commit_file(path, old.as_deref(), &new, dry_run, report)
}

/// Сниппет `.mcp.json` для печати (generic-потоки).
pub(super) fn mcp_json_snippet(rw: bool, rw_reports: bool) -> String {
    let v = json!({"mcpServers": {MCP_SERVER_NAME: mcp_server_value(rw, rw_reports)}});
    match serde_json::to_string_pretty(&v) {
        Ok(text) => text,
        // Сериализация Value не падает; запасной вариант — компактная форма.
        Err(_) => v.to_string(),
    }
}

/// TOML-блок `[mcp_servers.spine]` для Codex (`~/.codex/config.toml`).
pub(super) fn codex_toml_block(rw: bool, rw_reports: bool) -> String {
    let args = if rw_reports {
        "[\"mcp\", \"serve\", \"--rw=reports\"]"
    } else if rw {
        "[\"mcp\", \"serve\", \"--rw\"]"
    } else {
        "[\"mcp\", \"serve\"]"
    };
    format!("[mcp_servers.spine]\ncommand = \"arch-be\"\nargs = {args}\n")
}

/// Мердж `[mcp_servers.spine]` в `~/.codex/config.toml` (`--apply-global`):
/// чужие секции сохраняются, перезаписывается только наша таблица; перед
/// первой перезаписью — бэкап. Пере-сериализация не сохраняет комментарии
/// и порядок ключей чужого файла — об этом заметка в отчёте.
pub(super) fn merge_codex_config(
    path: &Path,
    rw: bool,
    rw_reports: bool,
    dry_run: bool,
    report: &mut ConnectReport,
) -> Result<()> {
    let old = match std::fs::read_to_string(path) {
        Ok(text) => Some(text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(HarnessError::io(path, e)),
    };
    let mut doc: toml::Value = match old.as_deref() {
        None => toml::Value::Table(toml::map::Map::new()),
        Some(text) => toml::from_str(text).map_err(|e| {
            HarnessError::Config(format!(
                "{}: существующий config.toml не разбирается ({e}) — не затираю; \
                 разберите вручную",
                path.display()
            ))
        })?,
    };
    let Some(root) = doc.as_table_mut() else {
        return Err(HarnessError::Config(format!(
            "{}: верхний уровень не TOML-таблица — не затираю",
            path.display()
        )));
    };
    if !root.contains_key("mcp_servers") {
        root.insert(
            "mcp_servers".to_string(),
            toml::Value::Table(toml::map::Map::new()),
        );
    }
    let Some(servers) = root
        .get_mut("mcp_servers")
        .and_then(toml::Value::as_table_mut)
    else {
        return Err(HarnessError::Config(format!(
            "{}: секция mcp_servers не TOML-таблица — не затираю",
            path.display()
        )));
    };
    let foreign: Vec<String> = servers
        .keys()
        .filter(|k| k.as_str() != MCP_SERVER_NAME)
        .cloned()
        .collect();
    let mut spine = toml::map::Map::new();
    spine.insert(
        "command".to_string(),
        toml::Value::String("arch-be".to_string()),
    );
    // Одна функция аргументов на все каналы: кодовая копия логики режима
    // разошлась бы с остальными при первой же правке (J7).
    let args = mcp_server_args(rw, rw_reports);
    spine.insert(
        "args".to_string(),
        toml::Value::Array(
            args.iter()
                .map(|a| toml::Value::String((*a).to_string()))
                .collect(),
        ),
    );
    servers.insert(MCP_SERVER_NAME.to_string(), toml::Value::Table(spine));
    if !foreign.is_empty() {
        report.notes.push(format!(
            "{}: существующие MCP-серверы сохранены: {}",
            path.display(),
            foreign.join(", ")
        ));
    }
    let new = toml::to_string_pretty(&doc)
        .map_err(|e| HarnessError::Config(format!("сериализация TOML: {e}")))?;
    if let Some(prev) = old.as_deref() {
        if prev != new {
            backup_once(path, prev, dry_run, report)?;
            report.notes.push(format!(
                "{} переписан сериализатором TOML: комментарии/порядок ключей не \
                 сохраняются (прежняя версия — в бэкапе)",
                path.display()
            ));
        }
    }
    commit_file(path, old.as_deref(), &new, dry_run, report)
}
