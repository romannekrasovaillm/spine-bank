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

/// База для хуков сессии (Stop / PostToolUse): точка ответвления от основной
/// ветки — тот же список веток и тот же `merge-base`, что у
/// [`crate::control::default_anchor_base`]. Ветка не найдена — пусто, и хук
/// работает без базы (как 0.3.3). После Н5 хуки обязаны видеть УЖЕ
/// закоммиченное ослабление правила: без базы сравнение шло с `HEAD`.
///
/// T-03: в `--base` уходит ГОЛАЯ ревизия. Раньше шаблоны передавали готовый
/// диапазон `$BASE...HEAD`, гейт дописывал `...HEAD` второй раз, и дифф не
/// вычислялся вовсе — гейт молча уходил в fail-safe Critical.
const ANCHOR_BASE_SNIPPET: &str = "\
BASE=\"\"\n\
for anchor in origin/main main origin/master master; do\n\
\x20 if git rev-parse --verify --quiet \"$anchor\" >/dev/null 2>&1; then\n\
\x20   BASE=$(git merge-base \"$anchor\" HEAD 2>/dev/null || true)\n\
\x20   break\n\
\x20 fi\n\
done\n";

/// Команда Stop-хука Claude Code: единый архитектурный гейт перед завершением
/// сессии. Гард — только `command -v arch-be` (fail-soft на инфраструктуру:
/// нет бинаря — пропуск); блок (exit 2, stderr агенту) — по коду возврата
/// `arch-be gate` (ненулевой = провал хотя бы одной составляющей; строки
/// вывода не разбираются).
///
/// T-01: раньше шаблон сам проверял наличие `.arch-handoff/CONSTRAINTS.yaml`,
/// и на кейсе, собранном `bootstrap` (реестр в КОРНЕ), хук молча пропускал
/// красный гейт — контур молчал там, где обязан остановить. Расположение
/// реестра знает только бинарь (резолвер `control::resolve_constraints_path`:
/// корень, затем `.arch-handoff/`), поэтому в shell его больше нет: нет
/// реестра нигде — `gate` печатает «реестр правил не найден» и отдаёт
/// INCOMPLETE, хук блокирует.
pub(super) fn stop_hook_command() -> String {
    format!(
        "if command -v arch-be >/dev/null 2>&1; then \
         {ANCHOR_BASE_SNIPPET}\
         if [ -n \"$BASE\" ]; then \
         if ! out=$(arch-be gate --route auto --base \"$BASE\" 2>&1); then \
         printf '%s\\n\\n%s\\n' \"$out\" \
         \"{HOOK_MARKER}: архитектурный гейт FAIL — исправьте находки error перед завершением \
         (подробности выше; гейт: arch-be gate)\" >&2; exit 2; fi; \
         elif ! out=$(arch-be gate --route auto 2>&1); then \
         printf '%s\\n\\n%s\\n' \"$out\" \
         \"{HOOK_MARKER}: архитектурный гейт FAIL — исправьте находки error перед завершением \
         (подробности выше; гейт: arch-be gate)\" >&2; exit 2; fi; fi \
         # {HOOK_MARKER}:stop"
    )
}

/// Команда PostToolUse-хука (`--strict-hooks`): тот же гейт на каждую
/// правку файла (matcher `Edit|Write|MultiEdit`).
pub(super) fn post_tool_use_hook_command() -> String {
    format!(
        "if command -v arch-be >/dev/null 2>&1; then \
         {ANCHOR_BASE_SNIPPET}\
         if [ -n \"$BASE\" ]; then \
         if ! out=$(arch-be gate --route auto --base \"$BASE\" 2>&1); then \
         printf '%s\\n\\n%s\\n' \"$out\" \
         \"{HOOK_MARKER}: правка не проходит архитектурный гейт (arch-be gate FAIL) — \
         исправьте находки error\" >&2; exit 2; fi; \
         elif ! out=$(arch-be gate --route auto 2>&1); then \
         printf '%s\\n\\n%s\\n' \"$out\" \
         \"{HOOK_MARKER}: правка не проходит архитектурный гейт (arch-be gate FAIL) — \
         исправьте находки error\" >&2; exit 2; fi; fi \
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

/// Добавляет наш хук в массив события `hooks[event]`, если записи с
/// маркером там ещё нет (идемпотентность). Возвращает true, если изменил.
///
/// # Errors
/// Существующее значение `hooks[event]` не массив — не затираем чужое.
fn upsert_hook(
    hooks: &mut serde_json::Map<String, Value>,
    event: &str,
    matcher: Option<&str>,
    command: &str,
    timeout_secs: u64,
) -> Result<bool> {
    let entries_value = hooks.entry(event.to_string()).or_insert_with(|| json!([]));
    let Some(entries) = entries_value.as_array_mut() else {
        return Err(HarnessError::Config(format!(
            "hooks.{event}: ожидался массив групп хуков — не затираю"
        )));
    };
    if has_marked_hook(entries) {
        return Ok(false);
    }
    let mut group = json!({
        "hooks": [{"type": "command", "command": command, "timeout": timeout_secs}]
    });
    if let Some(m) = matcher {
        group["matcher"] = json!(m);
    }
    entries.push(group);
    Ok(true)
}

/// Мердж хуков в `.claude/settings.json`: событие `Stop` всегда,
/// `PostToolUse` (matcher `Edit|Write|MultiEdit`) — под `--strict-hooks`.
/// Чужие ключи и хуки не трогаются.
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
    if upsert_hook(
        hooks,
        "Stop",
        None,
        &stop_hook_command(),
        STOP_HOOK_TIMEOUT_SECS,
    )? {
        added.push("Stop");
    }
    if strict
        && upsert_hook(
            hooks,
            "PostToolUse",
            Some("Edit|Write|MultiEdit"),
            &post_tool_use_hook_command(),
            POST_TOOL_USE_HOOK_TIMEOUT_SECS,
        )?
    {
        added.push("PostToolUse");
    }
    if !added.is_empty() {
        report.notes.push(format!(
            "хуки добавлены: {} (маркер «{HOOK_MARKER}» — повторный connect дублей не плодит)",
            added.join(", ")
        ));
    }
    report.notes.push(
        "семантика хуков: fail-soft на инфраструктуру (нет arch-be — молча \
         пропуск; нет входа у составляющих — SKIP), блок (exit 2) — по ненулевому коду \
         `arch-be gate --route auto` (провал любой составляющей: fitness, \
         delta guard, rule_weakened, spine, trace); PostToolUse-гейт на \
         каждую правку — через --strict-hooks (дорого на репозиториях с \
         command_succeeds-правилами)"
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
    // Свежая карта — upsert заведомо добавляет; ошибка формы невозможна.
    let _ = upsert_hook(
        &mut hooks,
        "Stop",
        None,
        &stop_hook_command(),
        STOP_HOOK_TIMEOUT_SECS,
    );
    if strict {
        let _ = upsert_hook(
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

    /// Команды хуков: гард по бинарю, маркер, exit 2 по коду возврата
    /// `arch-be gate` (без разбора строк вывода).
    ///
    /// T-01: расположения реестра в shell-шаблонах НЕТ — на кейсе `bootstrap`
    /// (реестр в корне) прежний гард `[ -f .arch-handoff/CONSTRAINTS.yaml ]`
    /// глушил красный гейт. Путь резолвит бинарь.
    #[test]
    fn hook_commands_are_guarded_and_marked() {
        for cmd in [stop_hook_command(), post_tool_use_hook_command()] {
            assert!(cmd.contains("command -v arch-be"), "{cmd}");
            assert!(
                !cmd.contains("CONSTRAINTS"),
                "путь к реестру знает только бинарь (T-01): {cmd}"
            );
            assert!(cmd.contains("arch-be gate --route auto"), "{cmd}");
            assert!(
                !cmd.contains("Итог: FAIL"),
                "детекция провала — по exit-коду, не по строке: {cmd}"
            );
            assert!(cmd.contains("exit 2"), "{cmd}");
            assert!(cmd.contains(HOOK_MARKER), "{cmd}");
        }
        assert!(stop_hook_command().contains("# spine-connect:stop"));
        assert!(post_tool_use_hook_command().contains("# spine-connect:post-tool-use"));
    }
}
