//! `arch-be connect <host>` — подключение Spine к внешнему CLI-агенту.
//!
//! Шаг 3 плана инверсии харнесса (`spine-bank-inversion`): Spine без
//! собственной LLM становится «органом» чужого агента — MCP-сервер
//! (`arch-be mcp serve`, контракт — `mcp_server.rs`), пакет скиллов и хуки
//! жизненного цикла; думает хост (Claude Code, Codex, Qwen Code, Kimi Code).
//!
//! ОТСТУПЛЕНИЕ ОТ ПЛАНА: в плане шаг назван `arch-be export <host>`; имя
//! `export` занято командой экспорта журнала сессии в Word/Excel
//! (`Cmd::Export` в `main.rs`), поэтому команда называется `connect`.
//!
//! Что пишется в проект (`--dir`, по умолчанию — текущий каталог):
//! - `claude`: `.mcp.json` (мердж ключа `mcpServers.spine`, чужие серверы
//!   сохраняются), `.claude/settings.json` (мердж хуков; наши помечены
//!   комментарием `# spine-connect:*` в команде и не дублируются при
//!   повторном запуске), `.claude/skills/<имя>/` (скиллы: источник истины —
//!   библиотека пользователя из `[plugins].dirs` конфига, включая плагины
//!   без манифеста и правки пользователя; встроенные плагины
//!   [`crate::assets::embedded_plugin_files`] — fallback для скиллов,
//!   которых на диске нет, т.е. работает и из релизного бинаря без
//!   `arch-be init`), `CLAUDE.md`
//!   (создаётся краткий либо блок между маркерами
//!   `<!-- SPINE:BEGIN -->`/`<!-- SPINE:END -->`; рукописное не затирается);
//! - `qwen`: `.qwen/settings.json` (мердж `mcpServers`; Qwen Code — форк
//!   gemini-cli, ключ `mcpServers` верхнего уровня подтверждён) + скиллы в
//!   `.qwen/skills/` (нативный project-scope в qwen-code ≥ 0.24; существующий
//!   каталог скиллов не перетирается). Хуки НЕ пишутся: схема хуков Qwen Code
//!   не подтверждена — печатается сниппет-референс;
//! - `gigacode` (`GigaCode` — форк Qwen Code): автоопределение каталога
//!   настроек проекта — существующий `.gigacode/`; иначе существующий
//!   `.qwen/` (совместимость layout форка); иначе создаётся `.gigacode/`.
//!   Дальше механика qwen (мердж `mcpServers.spine` в `<каталог>/settings.json`),
//!   скиллы — как у `claude` (раскладка в `<каталог>/skills/` с обновлением
//!   версий источника), хуки — печать сниппета (как у qwen);
//! - `codex`: MCP у Codex — только пользовательский `~/.codex/config.toml`
//!   (`[mcp_servers.spine]`); молча в дом пользователя не пишем: печать
//!   готового TOML-блока, запись с мерджем — только по явному
//!   `--apply-global` (перед первой перезаписью — бэкап
//!   `*.bak-spine-connect`). AGENTS.md Codex читает нативно: файл сами не
//!   генерируем (наименее инвазивный вариант), при отсутствии —
//!   рекомендация `arch-be agents-md refresh .`;
//! - `kimi`: проектный `.kimi-code/mcp.json` (мердж `mcpServers.spine` по
//!   правилам `.mcp.json` claude; layout подтверждён официальной докой
//!   kimi.com/code/docs: project-уровень перекрывает user-уровень, поле
//!   `cwd` не пишем — сервер наследует cwd харнесса). Плюс печать: блок для
//!   ручной регистрации в user-level `~/.kimi-code/mcp.json` (альтернатива;
//!   запись туда с мерджем и бэкапом — по `--apply-global`) и TOML-блок
//!   хука `[[hooks]]` для `~/.kimi-code/config.toml` (хуки у Kimi Code
//!   бывают только пользовательского уровня; схема `event`/`matcher`/
//!   `command`/`timeout` подтверждена докой, exit 2 = блок) и напоминание
//!   про trust-диалог при первом запуске в каталоге;
//! - `omp` (oh-my-pi): `.mcp.json` (мердж, как у claude — omp дискаверит
//!   проектный файл автоматически, регистрация не нужна). Скиллы omp читает
//!   нативно из `.claude/skills/`: если такого каталога в проекте ещё нет,
//!   скиллы библиотеки раскладываются туда же, как у claude; каталог уже
//!   есть — не трогаем (чужую библиотеку не перетираем). Хуков через
//!   connect нет: механизм хуков omp — TypeScript-расширения, печатается
//!   указание на `omp --hook <file.ts>`;
//! - `generic`: только печать — сниппеты `.mcp.json` и хуков, куда что
//!   вставить вручную.
//!
//! Особые значения host — не агенты, а гейты, не зависящие от хоста
//! (бэклог волны 2, п.8: хуки ненадёжны — у qwen headless-файринг не
//! подтверждён, у Codex lifecycle-хуков нет):
//! - `ci` (`--provider gitlab|github|jenkins`): готовая джоба архитектурного
//!   гейта — `.gitlab-ci.yml` (мердж-блок между маркерами
//!   `# spine-connect:begin/end`, чужое не затирается; нарушения видны в
//!   интерфейсе merge request из артефакта `reports.codequality`),
//!   `.github/workflows/spine-gate.yml` (новый файл; существующий без
//!   маркера — отказ), `Jenkinsfile` (мердж-блок, `junit(...)`). Джоба
//!   запускает `arch-be gate --route auto --format <нативный формат
//!   площадки>`; установка бинаря — curl из релизов (linux-x86_64) или
//!   офлайн-бандл (оба варианта — в комментарии джобы);
//! - `git-hooks`: `.git/hooks/pre-commit` (быстрый `arch-be control check .`)
//!   и `pre-push` (полный `arch-be gate --route auto`); блоки между маркерами,
//!   идемпотентно, чужие хуки не затираются; оба хука fail-soft — при
//!   отсутствии `arch-be` в PATH молча пропускаются, как хуки connect.
//!   Расположение реестра правил в шаблонах не зашито (T-01): его резолвит
//!   бинарь — корень кейса, затем `.arch-handoff/`.
//!
//! Хуки (для `claude` — запись в `settings.json`; для остальных — печать).
//! События Claude Code: `Stop` (дефолт) и `PostToolUse` с matcher
//! `Edit|Write|MultiEdit` (только под `--strict-hooks`). Семантика
//! exit-кодов Claude Code: 0 — ок, 2 — блок с показом stderr агенту.
//! Консервативный дефолт — fail-soft на инфраструктуру, fail-hard на
//! вердикт: хук молча пропускается (exit 0), если `arch-be` не в PATH; блок
//! (exit 2) —
//! когда `arch-be gate --route auto` завершился ненулевым кодом (провал
//! любой составляющей: fitness, delta guard, `rule_weakened`, spine, trace).
//! Так инфраструктурные сбои (нет входа у составляющих гейта — SKIP внутри
//! `arch-be gate`) не останавливают сессию, а реальные нарушения — стопят
//! её. `PostToolUse` в дефолт не входит: гейт на репозитории с правилами
//! `command_succeeds` может гонять сборки/тесты — для каждой правки это
//! слишком дорого.
//!
//! Идемпотентность: повторный запуск даёт тот же результат — JSON
//! смерджен ключ-в-ключ, хуки не дублируются (поиск маркера
//! `HOOK_MARKER`), скиллы побайтово совпадают. `--dry-run` печатает план
//! без единой записи.
//!
//! Разбиение модуля (волна B1): `types` — типы (хост, опции, отчёт); `files` —
//! файловые примитивы (чтение JSON, фиксация записи, бэкап); `mcp` — описание
//! и мердж MCP-конфигов хостов (JSON и TOML Codex); `hooks` — команды и
//! мердж хуков жизненного цикла; `skills` — сбор и раскладка скиллов
//! (диск приоритетен над встроенными); `hosts` — по функции на хост-агента;
//! `ci` — `connect ci` (джобы GitLab/GitHub/Jenkins); `git_hooks` —
//! `connect git-hooks` (pre-commit/pre-push); `render` — печать отчёта;
//! `testkit` — общие фикстуры тестов.

mod ci;
mod files;
mod git_hooks;
mod hooks;
mod hosts;
mod mcp;
mod render;
mod skills;
#[cfg(test)]
mod testkit;
mod types;

pub use ci::{CiProvider, ci_placeholder_present, connect_ci};
pub use git_hooks::connect_git_hooks;
pub use hosts::{GigacodeDirOrigin, gigacode_settings_dir};
pub use mcp::mode_label;
pub use render::{render_plan, render_report};
pub use types::{ConnectOptions, ConnectReport, Host, SkillAction, SkillOutcome};

use hosts::{
    connect_claude, connect_codex, connect_generic, connect_gigacode, connect_kimi, connect_omp,
    connect_qwen,
};
use serde_json::{Value, json};
use std::path::Path;

use crate::error::{HarnessError, Result};

/// Выполняет подключение (или планирует его при `dry_run`).
///
/// # Errors
/// Существующий целевой JSON/TOML не разбирается или имеет неожиданную
/// форму (чужое не затираем — разбор вручную); ошибки чтения/записи файлов;
/// `--apply-global` без определимого домашнего каталога.
pub fn connect(opts: &ConnectOptions) -> Result<ConnectReport> {
    let mut report = ConnectReport {
        dry_run: opts.dry_run,
        ..ConnectReport::default()
    };
    if !opts.dir.is_dir() {
        if opts.dry_run {
            report
                .notes
                .push(format!("каталог {} будет создан", opts.dir.display()));
        } else {
            std::fs::create_dir_all(&opts.dir).map_err(|e| HarnessError::io(&opts.dir, e))?;
        }
    }
    match opts.host {
        Host::Claude => connect_claude(opts, &mut report)?,
        Host::Qwen => connect_qwen(opts, &mut report)?,
        Host::GigaCode => connect_gigacode(opts, &mut report)?,
        Host::Codex => connect_codex(opts, &mut report)?,
        Host::Kimi => connect_kimi(opts, &mut report)?,
        Host::Omp => connect_omp(opts, &mut report)?,
        Host::Generic => connect_generic(opts, &mut report),
    }
    write_connect_manifest(opts, &mut report)?;
    Ok(report)
}

/// Провенанс установки (П3 ДКА): `connect` записывает
/// `.arch-handoff/connect-manifest.json` — какие пути положил сам Spine,
/// с версией и хэшами. Детекторы значимости ([`crate::control`]) исключают
/// эти пути, поэтому подключение инструмента не меняет маршрут проекта.
///
/// # Errors
/// Ошибка записи манифеста (нет прав на `.arch-handoff/`).
fn write_connect_manifest(opts: &ConnectOptions, report: &mut ConnectReport) -> Result<()> {
    if opts.dry_run {
        return Ok(());
    }
    let rel = |p: &Path| {
        p.strip_prefix(&opts.dir)
            .ok()
            .map(|r| r.to_string_lossy().replace('\\', "/"))
    };
    let path = opts.dir.join(crate::control::CONNECT_MANIFEST_PATH);
    // Манифест — про установленное СОСТОЯНИЕ, а не про текущий прогон: слияние
    // с предыдущим делает повтор `connect` байт-в-байт идемпотентным и не
    // «теряет» пути, которые в этом прогоне уже были без изменений.
    let prev: Option<Value> = std::fs::read_to_string(&path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok());
    let mut paths: Vec<String> = prev
        .as_ref()
        .and_then(|v| v.get("paths"))
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    let mut sha256: serde_json::Map<String, Value> = prev
        .as_ref()
        .and_then(|v| v.get("sha256"))
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    for p in report.created.iter().chain(report.merged.iter()) {
        if let Some(r) = rel(p) {
            paths.push(r.clone());
            if p.is_file() {
                if let Ok(bytes) = std::fs::read(p) {
                    sha256.insert(r, json!(crate::hash::sha256_hex(&bytes)));
                }
            }
        }
    }
    // Каталоги скиллов — паттерном (файлов много, исключается весь каталог).
    for root in [
        ".claude/skills",
        ".qwen/skills",
        ".gigacode/skills",
        ".kimi-code/skills",
    ] {
        if opts.dir.join(root).is_dir() {
            paths.push(format!("{root}/**"));
        }
    }
    paths.sort();
    paths.dedup();
    // Пути, которых больше нет, из манифеста выпадают.
    paths.retain(|p| p.ends_with("/**") || opts.dir.join(p).exists());
    sha256.retain(|k, _| paths.contains(k));
    if paths.is_empty() {
        return Ok(());
    }
    let manifest = json!({
        "arch_be": env!("CARGO_PKG_VERSION"),
        "host": format!("{:?}", opts.host).to_ascii_lowercase(),
        "installed_at": chrono::Local::now().to_rfc3339(),
        "paths": paths,
        "sha256": Value::Object(sha256),
    });
    // Идемпотентность: неизменившийся манифест не перезаписывается (иначе
    // `installed_at` ломает байт-в-байт повтор `connect`).
    let unchanged = prev.as_ref().is_some_and(|p| {
        p.get("paths") == manifest.get("paths")
            && p.get("sha256") == manifest.get("sha256")
            && p.get("host") == manifest.get("host")
            && p.get("arch_be") == manifest.get("arch_be")
    });
    if unchanged {
        report.notes.push(format!(
            "провенанс установки: {} (без изменений)",
            crate::control::CONNECT_MANIFEST_PATH
        ));
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| HarnessError::io(parent, e))?;
    }
    let text = serde_json::to_string_pretty(&manifest)
        .map_err(|e| HarnessError::Config(format!("сериализация connect-manifest: {e}")))?;
    std::fs::write(&path, text).map_err(|e| HarnessError::io(&path, e))?;
    report.notes.push(format!(
        "провенанс установки: {} ({} путей исключены из детекторов значимости)",
        crate::control::CONNECT_MANIFEST_PATH,
        manifest["paths"].as_array().map_or(0, Vec::len)
    ));
    Ok(())
}
