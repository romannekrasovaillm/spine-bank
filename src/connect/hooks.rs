//! Хуки жизненного цикла хоста (B1): команды Stop/PostToolUse с гардом,
//! мердж в `.claude/settings.json`, сниппеты для печати (включая TOML-блок
//! для Kimi Code).

use std::path::Path;

use serde_json::{Value, json};

use super::files::{commit_file, read_json_object};
use super::types::ConnectReport;
use crate::error::{HarnessError, Result};

/// Маркер наших хуков: комментарий в конце команды; по нему повторный
/// `connect` узнаёт свои записи и не плодит дубли.
pub(super) const HOOK_MARKER: &str = "spine-connect";
/// Таймаут Stop-хука, секунды (поле `timeout` Claude Code).
const STOP_HOOK_TIMEOUT_SECS: u64 = 150;
/// Таймаут PostToolUse-хука под `--strict-hooks`, секунды.
const POST_TOOL_USE_HOOK_TIMEOUT_SECS: u64 = 300;

/// Команда Stop-хука Claude Code: тонкий shim на бинарь (ROADMAP 1.7 п.2).
/// Вся семантика — якорь базы диффа, запуск `gate`, коды выхода (2 — блок,
/// stderr модели) — живёт в `arch-be hook stop` и версионируется вместе с
/// ядром: исправление хука приезжает с обновлением бинаря, а не повторным
/// `connect` на каждой машине. В shell остаётся только fail-soft гард
/// `command -v arch-be` (удалённый бинарь не ломает завершение сессии).
///
/// T-01: расположения реестра в shim'е нет и быть не может — его знает
/// только бинарь (резолвер `control::resolve_constraints_path`).
pub(crate) fn stop_hook_command() -> String {
    format!(
        "if command -v arch-be >/dev/null 2>&1; then arch-be hook stop; fi \
         # {HOOK_MARKER}:stop"
    )
}

/// Команда PostToolUse-хука (`--strict-hooks`): тот же shim на бинарь —
/// гейт на каждую правку файла (matcher `Edit|Write|MultiEdit`).
pub(super) fn post_tool_use_hook_command() -> String {
    format!(
        "if command -v arch-be >/dev/null 2>&1; then arch-be hook post-tool-use; fi \
         # {HOOK_MARKER}:post-tool-use"
    )
}

/// Есть ли в массиве групп хуков события запись с нашим маркером.
fn has_marked_hook(entries: &[Value]) -> bool {
    entries.iter().any(|group| {
        group
            .get("hooks")
            .and_then(Value::as_array)
            .is_some_and(|cmds| {
                cmds.iter().any(|c| {
                    c.get("command")
                        .and_then(Value::as_str)
                        .is_some_and(|cmd| cmd.contains(HOOK_MARKER))
                })
            })
    })
}

/// Обновляет записи с нашим маркером до канонической формы текущей версии:
/// хук, записанный прежней версией (shell-шаблон с зашитой логикой),
/// заменяется shim'ом на бинарь — дальше семантика приезжает с обновлениями
/// `arch-be`, без повторной правки settings вручную. Matcher групп и чужие
/// поля не трогаются. Возвращает true, если хотя бы одна команда заменена.
fn upgrade_marked_hooks(entries: &mut [Value], canonical_command: &str, timeout_secs: u64) -> bool {
    let mut changed = false;
    for group in entries.iter_mut() {
        let Some(cmds) = group.get_mut("hooks").and_then(Value::as_array_mut) else {
            continue;
        };
        for c in cmds.iter_mut() {
            let is_ours = c
                .get("command")
                .and_then(Value::as_str)
                .is_some_and(|cmd| cmd.contains(HOOK_MARKER));
            if !is_ours {
                continue;
            }
            if c.get("command").and_then(Value::as_str) != Some(canonical_command) {
                c["command"] = json!(canonical_command);
                c["timeout"] = json!(timeout_secs);
                changed = true;
            }
        }
    }
    changed
}

/// Идемпотентная гарантия хука события: записи с маркером нет — вставить;
/// есть — обновить до канонической формы (если отличается). Возвращает
/// `(added, upgraded)`.
///
/// # Errors
/// Существующее значение `hooks[event]` не массив — не затираем чужое.
fn ensure_hook(
    hooks: &mut serde_json::Map<String, Value>,
    event: &str,
    matcher: Option<&str>,
    command: &str,
    timeout_secs: u64,
) -> Result<(bool, bool)> {
    let entries_value = hooks.entry(event.to_string()).or_insert_with(|| json!([]));
    let Some(entries) = entries_value.as_array_mut() else {
        return Err(HarnessError::Config(format!(
            "hooks.{event}: ожидался массив групп хуков — не затираю"
        )));
    };
    if has_marked_hook(entries) {
        return Ok((false, upgrade_marked_hooks(entries, command, timeout_secs)));
    }
    let mut group = json!({
        "hooks": [{"type": "command", "command": command, "timeout": timeout_secs}]
    });
    if let Some(m) = matcher {
        group["matcher"] = json!(m);
    }
    entries.push(group);
    Ok((true, false))
}

/// Мердж хуков в `.claude/settings.json`: событие `Stop` всегда,
/// `PostToolUse` (matcher `Edit|Write|MultiEdit`) — под `--strict-hooks`.
/// Чужие ключи и хуки не трогаются; записи прежних версий (shell-шаблоны с
/// зашитой логикой) обновляются до shim'а на бинарь.
pub(super) fn merge_claude_settings(
    path: &Path,
    strict: bool,
    dry_run: bool,
    report: &mut ConnectReport,
) -> Result<()> {
    let (mut root, old) = read_json_object(path)?;
    let hooks_value = root.entry("hooks".to_string()).or_insert_with(|| json!({}));
    let Some(hooks) = hooks_value.as_object_mut() else {
        return Err(HarnessError::Config(format!(
            "{}: ключ hooks не является объектом — не затираю",
            path.display()
        )));
    };
    let mut added: Vec<&str> = Vec::new();
    let mut upgraded: Vec<&str> = Vec::new();
    let (a, u) = ensure_hook(
        hooks,
        "Stop",
        None,
        &stop_hook_command(),
        STOP_HOOK_TIMEOUT_SECS,
    )?;
    if a {
        added.push("Stop");
    }
    if u {
        upgraded.push("Stop");
    }
    if strict {
        let (a, u) = ensure_hook(
            hooks,
            "PostToolUse",
            Some("Edit|Write|MultiEdit"),
            &post_tool_use_hook_command(),
            POST_TOOL_USE_HOOK_TIMEOUT_SECS,
        )?;
        if a {
            added.push("PostToolUse");
        }
        if u {
            upgraded.push("PostToolUse");
        }
    }
    if !added.is_empty() {
        report.notes.push(format!(
            "хуки добавлены: {} (маркер «{HOOK_MARKER}» — повторный connect дублей не плодит)",
            added.join(", ")
        ));
    }
    if !upgraded.is_empty() {
        report.notes.push(format!(
            "хуки обновлены до shim'а на бинарь: {} — далее семантика приезжает с обновлением \
             arch-be, повторный connect не нужен",
            upgraded.join(", ")
        ));
    }
    report.notes.push(
        "семантика хуков: shim `arch-be hook stop|post-tool-use` — логика в бинаре \
         (версионируется с ядром); fail-soft на инфраструктуру (нет arch-be — молча пропуск; \
         нет входа у составляющих — SKIP), блок (exit 2) — по ненулевому коду гейта \
         (провал любой составляющей: fitness, delta guard, rule_weakened, spine, trace); \
         PostToolUse-гейт на каждую правку — через --strict-hooks (дорого на репозиториях \
         с command_succeeds-правилами)"
            .into(),
    );
    let new = match serde_json::to_string_pretty(&Value::Object(root)) {
        Ok(text) => format!("{text}\n"),
        Err(e) => return Err(HarnessError::Json(e)),
    };
    commit_file(path, old.as_deref(), &new, dry_run, report)
}

/// Сниппет хуков для печати (формат Claude Code settings.json): те же
/// команды, что пишутся мерджем, — у хостов без записи виден точный текст.
pub(super) fn hooks_snippet(strict: bool) -> String {
    let mut hooks = serde_json::Map::new();
    // Свежая карта — ensure заведомо добавляет; ошибка формы невозможна.
    let _ = ensure_hook(
        &mut hooks,
        "Stop",
        None,
        &stop_hook_command(),
        STOP_HOOK_TIMEOUT_SECS,
    );
    if strict {
        let _ = ensure_hook(
            &mut hooks,
            "PostToolUse",
            Some("Edit|Write|MultiEdit"),
            &post_tool_use_hook_command(),
            POST_TOOL_USE_HOOK_TIMEOUT_SECS,
        );
    }
    let v = json!({"hooks": Value::Object(hooks)});
    match serde_json::to_string_pretty(&v) {
        Ok(text) => text,
        // Сериализация Value не падает; запасной вариант — компактная форма.
        Err(_) => v.to_string(),
    }
}

/// TOML-блок хука-гейта для `~/.kimi-code/config.toml` (Kimi Code): событие
/// `Stop`, та же команда с гардом, что пишется Claude Code. Схема
/// подтверждена официальной докой Kimi Code (customization/hooks):
/// массив `[[hooks]]` с полями `event`/`matcher`/`command`/`timeout`,
/// только пользовательский уровень; семантика exit-кодов совпадает с
/// Claude Code — 0 пропуск, 2 блок (stderr уходит модели), прочие сбои
/// fail-open; рабочий каталог команды — каталог проекта сессии.
pub(super) fn kimi_hooks_toml_block() -> String {
    format!(
        "# Семантика: exit 0 — пропустить, exit 2 — блок (stderr уходит модели),\n\
         # прочие ошибки — fail-open. Команда выполняется в каталоге проекта.\n\
         [[hooks]]\n\
         event = \"Stop\"\n\
         command = \"{}\"\n\
         timeout = {}\n",
        toml_escape_basic(&stop_hook_command()),
        STOP_HOOK_TIMEOUT_SECS
    )
}

/// Экранирует строку для встраивания в TOML basic string (печать сниппета):
/// обратный слэш и двойная кавычка.
fn toml_escape_basic(text: &str) -> String {
    text.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Команды хуков — тонкие shim'ы на бинарь (ROADMAP 1.7 п.2): гард по
    /// бинарю, маркер, вызов `arch-be hook <имя>`; логика (база диффа,
    /// запуск гейта, exit-коды) — в бинаре, в shell её нет.
    ///
    /// T-01: расположения реестра в shim'ах НЕТ — путь резолвит бинарь.
    #[test]
    fn hook_commands_are_guarded_and_marked() {
        for cmd in [stop_hook_command(), post_tool_use_hook_command()] {
            assert!(cmd.contains("command -v arch-be"), "{cmd}");
            assert!(
                !cmd.contains("CONSTRAINTS"),
                "путь к реестру знает только бинарь (T-01): {cmd}"
            );
            assert!(cmd.contains("arch-be hook "), "shim на бинарь: {cmd}");
            assert!(
                !cmd.contains("arch-be gate") && !cmd.contains("merge-base"),
                "логика переехала в бинарь: {cmd}"
            );
            assert!(cmd.contains(HOOK_MARKER), "{cmd}");
        }
        assert!(stop_hook_command().contains("arch-be hook stop"));
        assert!(stop_hook_command().contains("# spine-connect:stop"));
        assert!(post_tool_use_hook_command().contains("arch-be hook post-tool-use"));
        assert!(post_tool_use_hook_command().contains("# spine-connect:post-tool-use"));
    }

    /// Прежний хук (shell-шаблон с зашитой логикой) обновляется до shim'а
    /// тем же connect: маркер один, чужие записи и matcher не тронуты.
    #[test]
    fn merge_upgrades_legacy_fat_hook_to_shim() {
        let tmp = tempfile::tempdir().expect("tmp");
        let path = tmp.path().join("settings.json");
        let legacy = "if command -v arch-be >/dev/null 2>&1; then BASE=x; arch-be gate \
                      --route auto; exit 2; fi # spine-connect:stop";
        std::fs::write(
            &path,
            format!(
                "{{\"hooks\": {{\"Stop\": [{{\"matcher\": \"m\", \"hooks\": [\
                 {{\"type\": \"command\", \"command\": \"{legacy}\", \"timeout\": 150}},\
                 {{\"type\": \"command\", \"command\": \"echo чужой\", \"timeout\": 1}}\
                 ]}}]}}}}"
            ),
        )
        .expect("settings");
        let mut report = ConnectReport::default();
        merge_claude_settings(&path, false, false, &mut report).expect("merge");
        let text = std::fs::read_to_string(&path).expect("read");
        assert!(text.contains("arch-be hook stop"), "shim: {text}");
        assert!(
            !text.contains("arch-be gate --route auto"),
            "старое убрано: {text}"
        );
        assert!(text.contains("echo чужой"), "чужой хук цел: {text}");
        assert!(
            text.contains("\"matcher\": \"m\""),
            "matcher группы цел: {text}"
        );
        assert_eq!(text.matches("spine-connect:stop").count(), 1, "маркер один");
        assert!(
            report.notes.iter().any(|n| n.contains("обновлены")),
            "заметка об обновлении: {:?}",
            report.notes
        );
        // Повторный connect — уже канон, ничего не меняет.
        let mut report2 = ConnectReport::default();
        merge_claude_settings(&path, false, false, &mut report2).expect("повтор");
        assert_eq!(text, std::fs::read_to_string(&path).expect("read2"));
    }
}
