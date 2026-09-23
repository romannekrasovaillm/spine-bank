//! Подключение хостов-агентов (B1): по функции на хост (claude, qwen,
//! gigacode, codex, kimi, omp, generic) + блок в CLAUDE.md и автоопределение
//! каталога настроек `GigaCode`.

use std::path::{Path, PathBuf};

use super::files::{commit_file, global_config_path};
use super::hooks::{hooks_snippet, kimi_hooks_toml_block, merge_claude_settings};
use super::mcp::{codex_toml_block, mcp_json_snippet, merge_codex_config, merge_mcp_servers_json};
use super::skills::install_skills;
use super::types::{ConnectOptions, ConnectReport};
use crate::error::Result;

/// Маркер начала блока Spine в CLAUDE.md.
const SPINE_BEGIN: &str = "<!-- SPINE:BEGIN -->";
/// Маркер конца блока Spine в CLAUDE.md.
const SPINE_END: &str = "<!-- SPINE:END -->";

/// Тело блока Spine в CLAUDE.md (между маркерами [`SPINE_BEGIN`]/[`SPINE_END`]).
fn claude_md_block() -> String {
    format!(
        "{SPINE_BEGIN}\n\
         ## Архитектурный контроль (Spine)\n\
         \n\
         В репозитории подключён MCP-сервер `spine` (`arch-be mcp serve`, см. `.mcp.json`):\n\
         - `fitness_check` — прогон CONSTRAINTS.yaml; вызывай ПЕРЕД коммитом: `passed=false` \
         с находками error — откажи изменению и перечисли находки.\n\
         - `spine_lint` / `trace_check` / `significance_score` — линтер спайна, \
         трассируемость, маршрут значимости.\n\
         - Чтение знаний: `kb_search`, `skill_search`, `skill_load`; модель архитектуры: \
         `model_query`.\n\
         \n\
         Точка входа в инструкции репозитория — AGENTS.md (нет файла → \
         `arch-be agents-md refresh .`).\n\
         {SPINE_END}"
    )
}

/// Вставляет блок в существующий файл: замена между маркерами, нет
/// маркеров — дописка в конец. Рукописная зона сохраняется; детерминировано
/// (повторный прогон побайтово совпадает).
fn splice_block(existing: &str, block: &str) -> String {
    let trimmed_block = block.trim_end();
    match (existing.find(SPINE_BEGIN), existing.find(SPINE_END)) {
        (Some(b), Some(e)) if b < e => {
            let pre = existing[..b].trim_end();
            let tail = existing[e + SPINE_END.len()..].trim();
            if tail.is_empty() {
                format!("{pre}\n\n{trimmed_block}\n")
            } else {
                format!("{pre}\n\n{trimmed_block}\n\n{tail}\n")
            }
        }
        _ => format!("{}\n\n{trimmed_block}\n", existing.trim_end()),
    }
}

/// Создаёт CLAUDE.md (краткий: ссылка на AGENTS.md и как звать Spine) или
/// дописывает/обновляет помеченный блок — рукописное не затирается.
fn upsert_claude_md(path: &Path, dry_run: bool, report: &mut ConnectReport) -> Result<()> {
    let block = claude_md_block();
    let old = std::fs::read_to_string(path).ok();
    let new = match old.as_deref() {
        None => format!(
            "# CLAUDE.md\n\n\
             Контекст для Claude Code. Точка входа в инструкции репозитория — AGENTS.md.\n\n\
             {block}\n"
        ),
        Some(existing) => splice_block(existing, &block),
    };
    commit_file(path, old.as_deref(), &new, dry_run, report)
}

/// Финальная строка `next_steps` каждого хоста («первая ценность за
/// 5 минут», C-1): плейбук-промпт MCP-сервера и демо-кейс FAIL→fix→PASS.
const FIRST_VALUE_STEP: &str = "Первая ценность за 5 минут: попросите агента \
     «действуй по плейбуку spine-quickstart»; демо FAIL→fix→PASS — кейс \
     drift-control из репозитория Spine.";

/// `arch-be connect claude`: проектный `.mcp.json`, хуки в
/// `.claude/settings.json`, скиллы в `.claude/skills/`, блок в CLAUDE.md.
pub(super) fn connect_claude(opts: &ConnectOptions, report: &mut ConnectReport) -> Result<()> {
    merge_mcp_servers_json(
        &opts.dir.join(".mcp.json"),
        opts.rw,
        opts.rw_reports,
        false,
        opts.dry_run,
        report,
    )?;
    if opts.hooks {
        merge_claude_settings(
            &opts.dir.join(".claude/settings.json"),
            opts.strict_hooks,
            opts.dry_run,
            report,
        )?;
    }
    if opts.skills {
        install_skills(
            &opts.dir.join(".claude/skills"),
            opts.dry_run,
            report,
            &opts.plugins_dirs,
        )?;
        report.notes.push(
            "субагенты плагинов (agents/*.md) не разворачиваются — при необходимости \
             скопируйте вручную из ~/.arch-harness/plugins после `arch-be init`"
                .into(),
        );
    }
    if opts.agents_md {
        upsert_claude_md(&opts.dir.join("CLAUDE.md"), opts.dry_run, report)?;
    }
    report.next_steps.extend([
        "перезапустите Claude Code (`claude`) в этом каталоге".to_string(),
        "проверьте подключение: `claude mcp list` — в списке сервер «spine»".to_string(),
        format!(
            "инструменты видны агенту как `mcp__spine__*`; режим сервера: {}",
            if opts.rw_reports {
                "rw=reports (запись только отчётов рубрики: rubric_verify → reports/rubric/)"
            } else if opts.rw {
                "rw (разрешены аддитивные записи: handoff_create, adr_new, …)"
            } else {
                "read-only"
            }
        ),
        FIRST_VALUE_STEP.to_string(),
    ]);
    Ok(())
}

/// `arch-be connect qwen`: мердж mcpServers в `.qwen/settings.json`
/// (Qwen Code — форк gemini-cli, ключ подтверждён) + скиллы в
/// `.qwen/skills/` (подтверждено на qwen-code 0.24.0 — нативный project-scope);
/// хуки не пишутся (headless-файринг не подтверждён) — сниппет-референс.
pub(super) fn connect_qwen(opts: &ConnectOptions, report: &mut ConnectReport) -> Result<()> {
    merge_mcp_servers_json(
        &opts.dir.join(".qwen/settings.json"),
        opts.rw,
        opts.rw_reports,
        false,
        opts.dry_run,
        report,
    )?;
    if opts.skills {
        let skills_root = opts.dir.join(".qwen/skills");
        if skills_root.exists() {
            report.notes.push(format!(
                "{} уже существует — скиллы НЕ раскладываются (чужую \
                 библиотеку не перетираем)",
                skills_root.display()
            ));
        } else {
            install_skills(&skills_root, opts.dry_run, report, &opts.plugins_dirs)?;
            report.notes.push(
                "скиллы разложены в .qwen/skills/ — Qwen Code ≥ 0.24 читает этот \
                 каталог нативно (project scope)"
                    .into(),
            );
        }
    }
    if opts.hooks {
        report.notes.push(
            "хуки не записаны: схема хуков Qwen Code не подтверждена — если \
             поддерживается, вставьте блок в .qwen/settings.json вручную"
                .into(),
        );
        report.snippets.push((
            "Хуки (формат Claude Code, референс)".to_string(),
            hooks_snippet(opts.strict_hooks),
        ));
    }
    report.next_steps.extend([
        "перезапустите Qwen Code (`qwen`) в этом каталоге".to_string(),
        "проверьте список MCP-серверов хоста — в нём «spine»".to_string(),
        FIRST_VALUE_STEP.to_string(),
    ]);
    Ok(())
}

/// Исход автоопределения каталога настроек `GigaCode` в проекте.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GigacodeDirOrigin {
    /// В проекте уже есть `.gigacode/` — он и используется.
    ExistingGigacode,
    /// `.gigacode/` нет, но есть `.qwen/`: `GigaCode` — форк Qwen Code,
    /// layout совместим — пишем в него.
    FromQwen,
    /// Ни того ни другого — создаётся `.gigacode/`.
    NewGigacode,
}

/// Автоопределение каталога настроек `GigaCode`: существующий `.gigacode/`
/// (приоритет), иначе существующий `.qwen/`, иначе — новый `.gigacode/`.
/// Используется и `connect gigacode`, и `doctor --host gigacode` (проверка
/// смотрит в тот же каталог, куда писал connect).
#[must_use]
pub fn gigacode_settings_dir(project_dir: &Path) -> (PathBuf, GigacodeDirOrigin) {
    let gigacode = project_dir.join(".gigacode");
    if gigacode.is_dir() {
        return (gigacode, GigacodeDirOrigin::ExistingGigacode);
    }
    let qwen = project_dir.join(".qwen");
    if qwen.is_dir() {
        return (qwen, GigacodeDirOrigin::FromQwen);
    }
    (gigacode, GigacodeDirOrigin::NewGigacode)
}

/// `arch-be connect gigacode` (`GigaCode` — форк Qwen Code): каталог настроек
/// определяется автоматически ([`gigacode_settings_dir`]); в него пишется
/// `settings.json` (мердж `mcpServers.spine`, как у qwen) и раскладываются
/// скиллы в `<каталог>/skills/` (как у claude — с обновлением встроенных
/// версий: целевой хост `GigaCode` освоил project-скиллы по layout Qwen Code
/// 0.24). Хуки не пишутся (подтверждённой схемы файла хуков у форка нет —
/// в qwen-code 0.24 хуки управляются UI `qwen hooks` и в headless не файрят)
/// — печатается сниппет-референс, как у qwen.
pub(super) fn connect_gigacode(opts: &ConnectOptions, report: &mut ConnectReport) -> Result<()> {
    let (settings_dir, origin) = gigacode_settings_dir(&opts.dir);
    match origin {
        GigacodeDirOrigin::ExistingGigacode => report.notes.push(format!(
            "каталог настроек автоопределён: {} (существующий .gigacode/)",
            settings_dir.display()
        )),
        GigacodeDirOrigin::FromQwen => report.notes.push(format!(
            "каталог настроек автоопределён: {} (`.gigacode/` нет, найден `.qwen/` — \
             GigaCode — форк Qwen Code, layout совместим)",
            settings_dir.display()
        )),
        GigacodeDirOrigin::NewGigacode => report.notes.push(format!(
            "каталога настроек в проекте нет — {} (ни .gigacode/, ни .qwen/)",
            if opts.dry_run {
                format!("будет создан {}", settings_dir.display())
            } else {
                format!("создан {}", settings_dir.display())
            }
        )),
    }
    merge_mcp_servers_json(
        &settings_dir.join("settings.json"),
        opts.rw,
        opts.rw_reports,
        false,
        opts.dry_run,
        report,
    )?;
    if opts.skills {
        install_skills(
            &settings_dir.join("skills"),
            opts.dry_run,
            report,
            &opts.plugins_dirs,
        )?;
        report.notes.push(
            "скиллы разложены в <каталог настроек>/skills/ — layout project-скиллов \
             Qwen Code ≥ 0.24, унаследован форком"
                .into(),
        );
    }
    if opts.hooks {
        report.notes.push(
            "хуки не записаны: подтверждённой схемы файла хуков у GigaCode нет \
             (в qwen-code 0.24 хуки управляются через `qwen hooks` (UI) и в \
             headless-режиме не файрят). Информационный вариант из docs/GIGACODE.md — \
             вручную в settings.json: \"hooks\": {\"SessionEnd\": [{\"hooks\": \
             [{\"type\": \"command\", \"command\": \"arch-be gate --route auto 2>&1 | \
             tail -3\"}]}]} (показывает вердикт гейта, но НЕ блокирует завершение). \
             Блокирующие гейты есть у Claude Code/Kimi/omp"
                .into(),
        );
        report.snippets.push((
            "Хуки (формат Claude Code, референс)".to_string(),
            hooks_snippet(opts.strict_hooks),
        ));
    }
    report.next_steps.extend([
        "перезапустите GigaCode в этом каталоге".to_string(),
        "одобрите project-сервер, если хост попросит (в Qwen Code: \
         `qwen mcp approve spine`; в GigaCode — аналог вашей сборки)"
            .to_string(),
        "проверьте подключение: `arch-be doctor --host gigacode`".to_string(),
        FIRST_VALUE_STEP.to_string(),
    ]);
    Ok(())
}

/// `arch-be connect codex`: MCP — пользовательский `~/.codex/config.toml`;
/// по умолчанию только печать TOML-блока (в дом молча не пишем).
pub(super) fn connect_codex(opts: &ConnectOptions, report: &mut ConnectReport) -> Result<()> {
    if opts.apply_global {
        let path = global_config_path(opts, ".codex/config.toml")?;
        merge_codex_config(&path, opts.rw, opts.rw_reports, opts.dry_run, report)?;
    } else {
        report.snippets.push((
            "MCP-сервер для Codex — добавьте в ~/.codex/config.toml:".to_string(),
            codex_toml_block(opts.rw, opts.rw_reports),
        ));
        report.notes.push(
            "в дом пользователя без --apply-global не пишем (перезапуск с флагом \
             вписывает блок с мерджем и бэкапом)"
                .into(),
        );
    }
    if opts.agents_md && !opts.dir.join("AGENTS.md").is_file() {
        report.next_steps.push(
            "Codex нативно читает AGENTS.md: сгенерируйте его командой \
             `arch-be agents-md refresh .` (connect файл не создаёт — наименее \
             инвазивный вариант)"
                .to_string(),
        );
    }
    if opts.hooks {
        report.notes.push(
            "у Codex нет lifecycle-хуков уровня PreToolUse/Stop — архитектурный гейт \
             остаётся ручным (`arch-be gate`) или в CI"
                .into(),
        );
    }
    report.next_steps.extend([
        "перезапустите `codex` в этом каталоге".to_string(),
        "проверьте список MCP-серверов: `codex mcp list` — в нём «spine»".to_string(),
        FIRST_VALUE_STEP.to_string(),
    ]);
    Ok(())
}

/// `arch-be connect kimi`: пишет проектный `.kimi-code/mcp.json` (мердж
/// `mcpServers.spine`; layout подтверждён официальной докой Kimi Code —
/// project-уровень действует только на текущий репозиторий и перекрывает
/// одноимённые user-level записи). Поле `cwd` не пишем: сервер наследует
/// рабочий каталог харнесса, как у остальных хостов. Плюс печать: блок для
/// ручной регистрации в user-level `~/.kimi-code/mcp.json` (запись туда с
/// мерджем и бэкапом — по `--apply-global`) и TOML-блок хука `[[hooks]]`
/// для `~/.kimi-code/config.toml` (проектных хуков у Kimi Code нет).
pub(super) fn connect_kimi(opts: &ConnectOptions, report: &mut ConnectReport) -> Result<()> {
    merge_mcp_servers_json(
        &opts.dir.join(".kimi-code/mcp.json"),
        opts.rw,
        opts.rw_reports,
        false,
        opts.dry_run,
        report,
    )?;
    if opts.apply_global {
        let path = global_config_path(opts, ".kimi-code/mcp.json")?;
        merge_mcp_servers_json(&path, opts.rw, opts.rw_reports, true, opts.dry_run, report)?;
        report.notes.push(
            "записаны оба уровня; при совпадении имён проектная запись \
             .kimi-code/mcp.json перекрывает пользовательскую (дока Kimi Code)"
                .into(),
        );
    } else {
        report.snippets.push((
            "MCP-сервер для Kimi Code (альтернатива) — пользовательский уровень \
             ~/.kimi-code/mcp.json, общий для всех проектов (запись с мерджем и \
             бэкапом — перезапуском с --apply-global):"
                .to_string(),
            mcp_json_snippet(opts.rw, opts.rw_reports),
        ));
    }
    if opts.hooks {
        report.snippets.push((
            "Хук-гейт для Kimi Code — добавьте в ~/.kimi-code/config.toml \
             (проектного уровня у хуков нет; схема подтверждена докой Kimi Code):"
                .to_string(),
            kimi_hooks_toml_block(),
        ));
    }
    if opts.skills {
        report.notes.push(
            "скиллы не разворачиваются: у Kimi Code своя библиотека скиллов \
             (extra_skill_dirs в ~/.kimi-code/config.toml) — добавьте каталог \
             ~/.arch-harness/plugins туда вручную при необходимости"
                .into(),
        );
    }
    report.next_steps.extend([
        "перезапустите Kimi Code (`kimi`) в этом каталоге".to_string(),
        "при первом запуске в каталоге Kimi Code покажет trust-диалог со списком \
         project-level MCP-серверов: project MCP не стартует в untrusted-папке \
         (штатная защита) — подтвердите «Trust this folder»"
            .to_string(),
        "проверьте подключение: команда `/mcp` в TUI — в списке сервер «spine»".to_string(),
        format!(
            "режим сервера: {}",
            if opts.rw_reports {
                "rw=reports (запись только отчётов рубрики: rubric_verify → reports/rubric/)"
            } else if opts.rw {
                "rw (разрешены аддитивные записи: handoff_create, adr_new, …)"
            } else {
                "read-only"
            }
        ),
        FIRST_VALUE_STEP.to_string(),
    ]);
    Ok(())
}

/// `arch-be connect omp` (oh-my-pi): проектный `.mcp.json` формата Claude
/// Desktop — omp дискаверит его автоматически, регистрация не нужна.
/// Скиллы omp читает нативно из `.claude/skills/`: если такого каталога в
/// проекте ещё нет, скиллы библиотеки раскладываются туда тем же
/// `install_skills`, что у claude; каталог уже есть — не трогаем, чтобы не
/// перетирать чужую библиотеку (об этом заметка). Хуков через connect нет:
/// механизм хуков omp — TypeScript-расширения (`omp --hook <file.ts>`),
/// печатается указание.
pub(super) fn connect_omp(opts: &ConnectOptions, report: &mut ConnectReport) -> Result<()> {
    merge_mcp_servers_json(
        &opts.dir.join(".mcp.json"),
        opts.rw,
        opts.rw_reports,
        false,
        opts.dry_run,
        report,
    )?;
    report.notes.push(
        "omp автоматически дискаверит проектный .mcp.json — отдельная регистрация \
         сервера не нужна"
            .into(),
    );
    if opts.skills {
        let skills_root = opts.dir.join(".claude/skills");
        if skills_root.exists() {
            report.notes.push(format!(
                "{} уже существует — скиллы НЕ раскладываются (чужую \
                 библиотеку не перетираем; omp читает .claude/skills нативно, \
                 принудительно разложить встроенные может `arch-be connect claude`)",
                skills_root.display()
            ));
        } else {
            install_skills(&skills_root, opts.dry_run, report, &opts.plugins_dirs)?;
            report.notes.push(
                "скиллы разложены в .claude/skills/ — omp читает этот каталог \
                 нативно (тот же layout, что у Claude Code)"
                    .into(),
            );
        }
    }
    if opts.hooks {
        report.notes.push(
            "хуков через connect нет: механизм хуков omp — TypeScript-расширения, \
             подключение: `omp --hook <file.ts>`; команда-гейт для такого \
             расширения: `arch-be gate`"
                .into(),
        );
    }
    report.next_steps.extend([
        "перезапустите omp в этом каталоге — сервер «spine» подхватится из .mcp.json".to_string(),
        format!(
            "инструменты видны агенту как инструменты сервера «spine»; режим: {}",
            if opts.rw_reports {
                "rw=reports (запись только отчётов рубрики: rubric_verify → reports/rubric/)"
            } else if opts.rw {
                "rw (разрешены аддитивные записи: handoff_create, adr_new, …)"
            } else {
                "read-only"
            }
        ),
        FIRST_VALUE_STEP.to_string(),
    ]);
    Ok(())
}

/// `arch-be connect generic`: ничего не пишет — печатает сниппеты и
/// инструкцию ручной установки.
pub(super) fn connect_generic(opts: &ConnectOptions, report: &mut ConnectReport) {
    report.snippets.push((
        "MCP-сервер (формат `.mcp.json` Claude Code — его понимает большинство хостов):"
            .to_string(),
        mcp_json_snippet(opts.rw, opts.rw_reports),
    ));
    if opts.hooks {
        report.snippets.push((
            "Хуки (формат settings.json Claude Code; exit 2 = блок, stderr уходит агенту):"
                .to_string(),
            hooks_snippet(opts.strict_hooks),
        ));
    }
    if opts.skills {
        report.snippets.push((
            "Скиллы".to_string(),
            "встроенные скиллы раскладываются в ~/.arch-harness/plugins командой \
             `arch-be init` — скопируйте нужные `skills/<имя>/SKILL.md` в каталог \
             скиллов вашего хоста"
                .to_string(),
        ));
    }
    if opts.agents_md {
        report.snippets.push((
            "AGENTS.md".to_string(),
            "сгенерируйте точку входа агента: `arch-be agents-md refresh .` \
             (рукописная зона сохраняется)"
                .to_string(),
        ));
    }
    report
        .notes
        .push("generic ничего не записывает — только печать".into());
    report.next_steps.extend([
        "вставьте сниппеты выше в конфигурацию вашего агента".to_string(),
        "перезапустите агента и проверьте, что MCP-сервер «spine» поднялся".to_string(),
        FIRST_VALUE_STEP.to_string(),
    ]);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connect::Host;
    use crate::connect::connect;
    use crate::connect::testkit::{count_marked_hooks, read, snapshot};
    use serde_json::{Value, json};

    /// Финальной строкой `next_steps` каждого хоста идёт «первая ценность
    /// за 5 минут» (C-1): плейбук spine-quickstart + демо-кейс drift-control.
    #[test]
    fn every_host_next_steps_end_with_first_value() {
        let tmp = tempfile::tempdir().expect("tmp");
        for (i, host) in [
            Host::Claude,
            Host::Qwen,
            Host::GigaCode,
            Host::Codex,
            Host::Kimi,
            Host::Omp,
            Host::Generic,
        ]
        .into_iter()
        .enumerate()
        {
            let dir = tmp.path().join(format!("proj-{i}"));
            std::fs::create_dir_all(&dir).expect("mkdir");
            let report = connect(&ConnectOptions::new(host, dir)).expect("connect");
            let last = report
                .next_steps
                .last()
                .unwrap_or_else(|| panic!("{host:?}: next_steps пуст"));
            assert_eq!(
                last, FIRST_VALUE_STEP,
                "{host:?}: финальная строка — «первая ценность за 5 минут»"
            );
            assert!(
                last.contains("spine-quickstart") && last.contains("drift-control"),
                "{host:?}: {last}"
            );
        }
    }

    /// (a) connect claude в пустой каталог: .mcp.json, settings.json,
    /// CLAUDE.md и скиллы на месте.
    #[test]
    fn claude_scaffolds_empty_dir() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path().join("proj");
        let report = connect(&ConnectOptions::new(Host::Claude, dir.clone())).expect("connect");

        let mcp: Value = serde_json::from_str(&read(&dir.join(".mcp.json"))).expect("json");
        assert_eq!(mcp["mcpServers"]["spine"]["command"], "arch-be");
        assert_eq!(mcp["mcpServers"]["spine"]["args"], json!(["mcp", "serve"]));

        let settings: Value =
            serde_json::from_str(&read(&dir.join(".claude/settings.json"))).expect("json");
        assert_eq!(count_marked_hooks(&settings, "Stop"), 1, "один Stop-хук");
        assert!(
            settings["hooks"].get("PostToolUse").is_none(),
            "PostToolUse — только под --strict-hooks"
        );

        let claude_md = read(&dir.join("CLAUDE.md"));
        assert!(claude_md.contains(SPINE_BEGIN));
        assert!(claude_md.contains("AGENTS.md"));

        let skill = dir.join(".claude/skills/adr-authoring/SKILL.md");
        assert!(skill.is_file(), "скилл разложен: {}", skill.display());
        assert!(read(&skill).starts_with("---\n"), "frontmatter на месте");
        // Скилл с references/ переносится целиком.
        assert!(
            dir.join(".claude/skills/adr-authoring/references/adr-template.md")
                .is_file(),
            "references скилла переносятся"
        );
        assert!(
            report.skills.len() >= 40,
            "скиллов: {}",
            report.skills.len()
        );
        assert!(
            report.skills_skipped.is_empty(),
            "{:?}",
            report.skills_skipped
        );
        assert!(
            report.created.iter().any(|p| p.ends_with(".mcp.json")),
            "{:?}",
            report.created
        );
    }

    /// (b) Повторный запуск: дерево побайтово идентично, хуки не дублируются.
    #[test]
    fn claude_second_run_is_byte_identical() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path().join("proj");
        let opts = ConnectOptions::new(Host::Claude, dir.clone());
        connect(&opts).expect("первый прогон");
        let first = snapshot(&dir);
        let report = connect(&opts).expect("второй прогон");
        let second = snapshot(&dir);
        assert_eq!(first, second, "повторный запуск изменил файлы");
        assert!(
            !report.unchanged.is_empty(),
            "второй прогон — всё «без изменений»"
        );
        assert!(report.merged.is_empty() && report.created.is_empty());

        let settings: Value =
            serde_json::from_str(&read(&dir.join(".claude/settings.json"))).expect("json");
        assert_eq!(count_marked_hooks(&settings, "Stop"), 1, "без дублей хуков");

        // Эскалация до --strict-hooks добавляет PostToolUse и не трогает Stop.
        let strict = ConnectOptions {
            strict_hooks: true,
            ..ConnectOptions::new(Host::Claude, dir.clone())
        };
        connect(&strict).expect("strict прогон");
        let settings: Value =
            serde_json::from_str(&read(&dir.join(".claude/settings.json"))).expect("json");
        assert_eq!(count_marked_hooks(&settings, "Stop"), 1);
        assert_eq!(count_marked_hooks(&settings, "PostToolUse"), 1);
        connect(&strict).expect("повторный strict");
        let settings: Value =
            serde_json::from_str(&read(&dir.join(".claude/settings.json"))).expect("json");
        assert_eq!(
            count_marked_hooks(&settings, "PostToolUse"),
            1,
            "дубль strict"
        );
    }

    /// (c) Мердж: чужой сервер в .mcp.json и чужой хук в settings.json
    /// сохраняются, наши добавляются.
    #[test]
    fn merge_preserves_foreign_servers_and_hooks() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path().join("proj");
        std::fs::create_dir_all(dir.join(".claude")).expect("mkdir");
        std::fs::write(
            dir.join(".mcp.json"),
            "{\n  \"mcpServers\": {\n    \"other\": {\"command\": \"uvx\", \"args\": [\"x\"]}\n  },\n  \"foreignTop\": true\n}\n",
        )
        .expect("write .mcp.json");
        std::fs::write(
            dir.join(".claude/settings.json"),
            "{\n  \"model\": \"opus\",\n  \"hooks\": {\n    \"Stop\": [\n      {\"hooks\": [{\"type\": \"command\", \"command\": \"echo mine\"}]}\n    ]\n  }\n}\n",
        )
        .expect("write settings.json");

        let report = connect(&ConnectOptions::new(Host::Claude, dir.clone())).expect("connect");

        let mcp: Value = serde_json::from_str(&read(&dir.join(".mcp.json"))).expect("json");
        assert_eq!(
            mcp["mcpServers"]["other"]["command"], "uvx",
            "чужой сервер цел"
        );
        assert_eq!(mcp["foreignTop"], json!(true), "чужой верхний ключ цел");
        assert_eq!(mcp["mcpServers"]["spine"]["command"], "arch-be");

        let settings: Value =
            serde_json::from_str(&read(&dir.join(".claude/settings.json"))).expect("json");
        assert_eq!(settings["model"], "opus", "чужой ключ settings цел");
        let stop = settings["hooks"]["Stop"].as_array().expect("массив Stop");
        assert_eq!(stop.len(), 2, "чужой хук + наш: {stop:?}");
        assert_eq!(stop[0]["hooks"][0]["command"], "echo mine", "чужой хук цел");
        assert_eq!(count_marked_hooks(&settings, "Stop"), 1);
        assert!(
            report
                .notes
                .iter()
                .any(|n| n.contains("сохранены") && n.contains("other")),
            "заметка о чужих серверах: {:?}",
            report.notes
        );
        assert!(
            report.merged.iter().any(|p| p.ends_with(".mcp.json")),
            "{:?}",
            report.merged
        );
    }

    /// Битый существующий JSON — отказ без затирания.
    #[test]
    fn broken_existing_json_refuses_overwrite() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path().join("proj");
        std::fs::create_dir_all(&dir).expect("mkdir");
        std::fs::write(dir.join(".mcp.json"), "{это не json").expect("write");
        let err = connect(&ConnectOptions::new(Host::Claude, dir.clone()))
            .expect_err("должна быть ошибка");
        assert!(err.to_string().contains("не затираю"), "{err}");
        assert_eq!(read(&dir.join(".mcp.json")), "{это не json", "файл цел");
    }

    /// (d) --dry-run: только план, файловая система не трогается.
    #[test]
    fn dry_run_writes_nothing() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path().join("proj");
        let opts = ConnectOptions {
            dry_run: true,
            ..ConnectOptions::new(Host::Claude, dir.clone())
        };
        let report = connect(&opts).expect("connect dry-run");
        assert!(!report.created.is_empty(), "план непустой");
        assert!(!dir.exists(), "dry-run даже каталог не создаёт");

        // И на существующем дереве — ни одной правки.
        connect(&ConnectOptions::new(Host::Claude, dir.clone())).expect("реальный прогон");
        let before = snapshot(&dir);
        let opts = ConnectOptions {
            dry_run: true,
            strict_hooks: true, // план шире реальности — всё равно ничего не пишет
            ..ConnectOptions::new(Host::Claude, dir.clone())
        };
        connect(&opts).expect("dry-run поверх");
        assert_eq!(before, snapshot(&dir), "dry-run что-то записал");
    }

    /// (e) codex/generic без --apply-global — только печать: ни в проект,
    /// ни в дом ничего не пишется (kimi с подтверждённым проектным layout
    /// таким больше не является — см. `kimi_writes_project_mcp_json`).
    #[test]
    fn codex_generic_are_print_only_by_default() {
        for host in [Host::Codex, Host::Generic] {
            let tmp = tempfile::tempdir().expect("tmp");
            let dir = tmp.path().join("proj");
            let home = tmp.path().join("home");
            std::fs::create_dir_all(&dir).expect("mkdir");
            let opts = ConnectOptions {
                home: Some(home.clone()),
                ..ConnectOptions::new(host, dir.clone())
            };
            let report = connect(&opts).expect("connect");
            assert_eq!(
                snapshot(&dir),
                Vec::new(),
                "{}: в проект ничего не пишется",
                host.name()
            );
            assert!(!home.exists(), "{}: дом не трогается", host.name());
            assert!(
                !report.snippets.is_empty(),
                "{}: печатаются сниппеты",
                host.name()
            );
        }
        // generic печатает и mcp-сниппет, и хуки.
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path().join("proj");
        let report = connect(&ConnectOptions::new(Host::Generic, dir)).expect("connect");
        let titles: Vec<&str> = report.snippets.iter().map(|(t, _)| t.as_str()).collect();
        assert!(titles.iter().any(|t| t.contains("MCP")), "{titles:?}");
        assert!(titles.iter().any(|t| t.contains("Хуки")), "{titles:?}");
    }

    /// (f) --rw добавляет "--rw" в args MCP-сервера (запись и сниппеты).
    #[test]
    fn rw_flag_extends_mcp_args() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path().join("proj");
        let opts = ConnectOptions {
            rw: true,
            ..ConnectOptions::new(Host::Claude, dir.clone())
        };
        connect(&opts).expect("connect");
        let mcp: Value = serde_json::from_str(&read(&dir.join(".mcp.json"))).expect("json");
        assert_eq!(
            mcp["mcpServers"]["spine"]["args"],
            json!(["mcp", "serve", "--rw"])
        );
        // kimi: --rw в проектном .kimi-code/mcp.json и в user-level сниппете.
        let kimi_dir = tmp.path().join("proj-kimi");
        let opts = ConnectOptions {
            rw: true,
            ..ConnectOptions::new(Host::Kimi, kimi_dir.clone())
        };
        let report = connect(&opts).expect("kimi");
        let mcp: Value =
            serde_json::from_str(&read(&kimi_dir.join(".kimi-code/mcp.json"))).expect("json");
        assert_eq!(
            mcp["mcpServers"]["spine"]["args"],
            json!(["mcp", "serve", "--rw"])
        );
        assert!(
            report.snippets.iter().any(|(_, s)| s.contains("--rw")),
            "{:?}",
            report.snippets
        );
        // omp: --rw в .mcp.json.
        let omp_dir = tmp.path().join("proj-omp");
        let opts = ConnectOptions {
            rw: true,
            ..ConnectOptions::new(Host::Omp, omp_dir.clone())
        };
        connect(&opts).expect("omp");
        let mcp: Value = serde_json::from_str(&read(&omp_dir.join(".mcp.json"))).expect("json");
        assert_eq!(
            mcp["mcpServers"]["spine"]["args"],
            json!(["mcp", "serve", "--rw"])
        );
        // В сниппетах codex тоже.
        let opts = ConnectOptions {
            rw: true,
            ..ConnectOptions::new(Host::Codex, tmp.path().join("p2"))
        };
        let report = connect(&opts).expect("codex");
        assert!(
            report.snippets.iter().any(|(_, s)| s.contains("--rw")),
            "{:?}",
            report.snippets
        );
    }

    /// qwen: .qwen/settings.json (мердж mcpServers) + скиллы в
    /// .qwen/skills/ (0.24+); хуки — сниппет-референс.
    #[test]
    fn qwen_writes_settings_json_only() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path().join("proj");
        let report = connect(&ConnectOptions::new(Host::Qwen, dir.clone())).expect("connect");
        let settings: Value =
            serde_json::from_str(&read(&dir.join(".qwen/settings.json"))).expect("json");
        assert_eq!(settings["mcpServers"]["spine"]["command"], "arch-be");
        assert!(!dir.join(".claude").exists(), "каталога .claude нет");
        assert!(
            report.notes.iter().any(|n| n.contains(".qwen/skills")),
            "заметка про скиллы .qwen/skills: {:?}",
            report.notes
        );
        // Идемпотентность.
        connect(&ConnectOptions::new(Host::Qwen, dir.clone())).expect("повтор");
        let again: Value =
            serde_json::from_str(&read(&dir.join(".qwen/settings.json"))).expect("json");
        assert_eq!(settings, again);
    }

    /// --apply-global: codex — мердж в ~/.codex/config.toml с бэкапом и без
    /// дублей; kimi — мердж в ~/.kimi-code/mcp.json.
    #[test]
    fn apply_global_merges_home_configs_with_backup() {
        let tmp = tempfile::tempdir().expect("tmp");
        let home = tmp.path().join("home");
        std::fs::create_dir_all(home.join(".codex")).expect("mkdir");
        let cfg = home.join(".codex/config.toml");
        std::fs::write(
            &cfg,
            "model = \"gpt-5\"\n\n[mcp_servers.other]\ncommand = \"uvx\"\n",
        )
        .expect("write config.toml");
        let opts = ConnectOptions {
            apply_global: true,
            home: Some(home.clone()),
            ..ConnectOptions::new(Host::Codex, tmp.path().join("proj"))
        };
        connect(&opts).expect("apply codex");
        let text = read(&cfg);
        assert!(text.contains("[mcp_servers.spine]"), "{text}");
        assert!(
            text.contains("[mcp_servers.other]"),
            "чужая секция цела: {text}"
        );
        assert!(text.contains("model = \"gpt-5\""), "чужой ключ цел: {text}");
        let backup = home.join(".codex/config.toml.bak-spine-connect");
        assert!(backup.is_file(), "бэкап создан");
        assert!(read(&backup).contains("gpt-5"), "бэкап — прежнее");
        // Повтор — без дублей и без нового бэкапа.
        let first = read(&cfg);
        connect(&opts).expect("повтор codex");
        assert_eq!(first, read(&cfg), "повторный apply_global идемпотентен");

        // kimi: JSON-мердж в ~/.kimi-code/mcp.json + проектный файл тоже
        // записывается (проектный уровень перекрывает пользовательский).
        let kimi_proj = tmp.path().join("proj-kimi");
        let opts = ConnectOptions {
            apply_global: true,
            home: Some(home.clone()),
            ..ConnectOptions::new(Host::Kimi, kimi_proj.clone())
        };
        connect(&opts).expect("apply kimi");
        let mcp: Value =
            serde_json::from_str(&read(&home.join(".kimi-code/mcp.json"))).expect("json");
        assert_eq!(mcp["mcpServers"]["spine"]["command"], "arch-be");
        let mcp: Value =
            serde_json::from_str(&read(&kimi_proj.join(".kimi-code/mcp.json"))).expect("json");
        assert_eq!(mcp["mcpServers"]["spine"]["command"], "arch-be");
    }

    /// CLAUDE.md: блок дописывается в чужой файл, при повторе — заменяется,
    /// рукописное цело.
    #[test]
    fn claude_md_block_splices_and_preserves_handwritten() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path().join("proj");
        std::fs::create_dir_all(&dir).expect("mkdir");
        std::fs::write(
            dir.join("CLAUDE.md"),
            "# Наш проект\n\nРукописные договорённости команды.\n",
        )
        .expect("write");
        connect(&ConnectOptions::new(Host::Claude, dir.clone())).expect("первый");
        let text = read(&dir.join("CLAUDE.md"));
        assert!(text.contains("Рукописные договорённости"), "{text}");
        assert_eq!(text.matches(SPINE_BEGIN).count(), 1, "{text}");

        // Правка внутри блока затирается, снаружи — нет.
        let modified = text.replace(SPINE_END, "шум внутри блока\n<!-- SPINE:END -->");
        std::fs::write(dir.join("CLAUDE.md"), modified).expect("правка блока");
        connect(&ConnectOptions::new(Host::Claude, dir.clone())).expect("второй");
        let text = read(&dir.join("CLAUDE.md"));
        assert!(!text.contains("шум внутри блока"), "{text}");
        assert!(text.contains("Рукописные договорённости"), "{text}");
        assert_eq!(text.matches(SPINE_BEGIN).count(), 1);
    }

    /// kimi: пишется проектный `.kimi-code/mcp.json` (без поля cwd), дом не
    /// трогается без --apply-global; печатаются user-level сниппет, TOML-блок
    /// хука и напоминание про trust-диалог. Мердж чужих серверов, отказ на
    /// битом JSON, идемпотентность — как у claude `.mcp.json`.
    #[test]
    fn kimi_writes_project_mcp_json_with_merge_and_idempotency() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path().join("proj");
        let home = tmp.path().join("home");
        std::fs::create_dir_all(dir.join(".kimi-code")).expect("mkdir");
        std::fs::write(
            dir.join(".kimi-code/mcp.json"),
            "{\n  \"mcpServers\": {\n    \"other\": {\"command\": \"uvx\", \"args\": [\"x\"]}\n  }\n}\n",
        )
        .expect("write mcp.json");
        let opts = ConnectOptions {
            home: Some(home.clone()),
            ..ConnectOptions::new(Host::Kimi, dir.clone())
        };
        let report = connect(&opts).expect("connect");

        let mcp: Value =
            serde_json::from_str(&read(&dir.join(".kimi-code/mcp.json"))).expect("json");
        assert_eq!(mcp["mcpServers"]["spine"]["command"], "arch-be");
        assert_eq!(mcp["mcpServers"]["spine"]["args"], json!(["mcp", "serve"]));
        assert!(
            mcp["mcpServers"]["spine"].get("cwd").is_none(),
            "поле cwd не пишем — сервер наследует cwd харнесса: {mcp}"
        );
        assert_eq!(
            mcp["mcpServers"]["other"]["command"], "uvx",
            "чужой сервер цел"
        );
        assert!(
            report
                .notes
                .iter()
                .any(|n| n.contains("сохранены") && n.contains("other")),
            "заметка о чужих серверах: {:?}",
            report.notes
        );
        assert!(!home.exists(), "дом не трогается без --apply-global");
        // Печать: user-level альтернатива, TOML-блок хука, trust-диалог.
        let all_snippets = report
            .snippets
            .iter()
            .map(|(t, s)| format!("{t}\n{s}"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            all_snippets.contains("~/.kimi-code/mcp.json"),
            "user-level сниппет: {:?}",
            report.snippets
        );
        assert!(all_snippets.contains("[[hooks]]"), "TOML-блок хука");
        assert!(all_snippets.contains("event = \"Stop\""), "{all_snippets}");
        assert!(
            all_snippets.contains("~/.kimi-code/config.toml"),
            "{all_snippets}"
        );
        assert!(
            all_snippets.contains("arch-be gate --route auto"),
            "{all_snippets}"
        );
        assert!(
            report.next_steps.iter().any(|s| s.contains("trust")),
            "напоминание про trust-диалог: {:?}",
            report.next_steps
        );

        // Идемпотентность: повторный запуск побайтово тот же, без дублей.
        let first = snapshot(&dir);
        let report = connect(&opts).expect("повтор");
        assert_eq!(first, snapshot(&dir), "повторный запуск изменил файлы");
        assert!(report.merged.is_empty() && report.created.is_empty());

        // Битый JSON — отказ без затирания.
        std::fs::write(dir.join(".kimi-code/mcp.json"), "{это не json").expect("write");
        let err = connect(&opts).expect_err("должна быть ошибка");
        assert!(err.to_string().contains("не затираю"), "{err}");
        assert_eq!(
            read(&dir.join(".kimi-code/mcp.json")),
            "{это не json",
            "файл цел"
        );
    }

    /// kimi --dry-run: ни проектный файл, ни дом не трогаются.
    #[test]
    fn kimi_dry_run_writes_nothing() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path().join("proj");
        let opts = ConnectOptions {
            dry_run: true,
            home: Some(tmp.path().join("home")),
            ..ConnectOptions::new(Host::Kimi, dir.clone())
        };
        let report = connect(&opts).expect("dry-run");
        assert!(!report.created.is_empty(), "план непустой");
        assert!(!dir.exists(), "dry-run ничего не записал");
        assert!(!tmp.path().join("home").exists(), "дом не трогается");
    }

    /// omp: пишется только `.mcp.json` + скиллы в `.claude/skills/` (omp
    /// читает его нативно); повторный запуск без дублей; существующий
    /// каталог `.claude/skills` не трогается; --dry-run ничего не пишет.
    #[test]
    fn omp_writes_mcp_json_and_skills() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path().join("proj");
        let report = connect(&ConnectOptions::new(Host::Omp, dir.clone())).expect("connect");

        let mcp: Value = serde_json::from_str(&read(&dir.join(".mcp.json"))).expect("json");
        assert_eq!(mcp["mcpServers"]["spine"]["command"], "arch-be");
        assert!(
            dir.join(".claude/skills/adr-authoring/SKILL.md").is_file(),
            "скиллы разложены в .claude/skills"
        );
        assert!(
            !dir.join(".claude/settings.json").exists(),
            "хуки не пишутся"
        );
        assert!(!dir.join("CLAUDE.md").exists(), "CLAUDE.md не создаётся");
        assert!(
            report.notes.iter().any(|n| n.contains("дискаверит")),
            "заметка про автодискавери: {:?}",
            report.notes
        );
        assert!(
            report.notes.iter().any(|n| n.contains("omp --hook")),
            "указание на TS-хуки: {:?}",
            report.notes
        );

        // Повтор: дерево побайтово то же (каталог скиллов уже есть —
        // скиллы не перетираются, .mcp.json мержится ключ-в-ключ).
        let first = snapshot(&dir);
        let report = connect(&ConnectOptions::new(Host::Omp, dir.clone())).expect("повтор");
        assert_eq!(first, snapshot(&dir), "повторный запуск изменил файлы");
        assert!(report.created.is_empty() && report.merged.is_empty());
        assert!(
            report
                .notes
                .iter()
                .any(|n| n.contains("уже существует") && n.contains("НЕ раскладываются")),
            "скип существующего каталога скиллов: {:?}",
            report.notes
        );

        // Существующий (даже пустой) .claude/skills не наполняется.
        let dir2 = tmp.path().join("proj2");
        std::fs::create_dir_all(dir2.join(".claude/skills")).expect("mkdir");
        let report = connect(&ConnectOptions::new(Host::Omp, dir2.clone())).expect("connect");
        assert!(report.skills.is_empty(), "скиллы не раскладывались");
        assert_eq!(
            std::fs::read_dir(dir2.join(".claude/skills"))
                .expect("read dir")
                .count(),
            0,
            "чужой каталог скиллов не тронут"
        );
        assert!(dir2.join(".mcp.json").is_file(), ".mcp.json записан");

        // --dry-run: ничего не пишется.
        let dir3 = tmp.path().join("proj3");
        let opts = ConnectOptions {
            dry_run: true,
            ..ConnectOptions::new(Host::Omp, dir3.clone())
        };
        let report = connect(&opts).expect("dry-run");
        assert!(!report.created.is_empty(), "план непустой");
        assert!(!report.skills.is_empty(), "план по скиллам непустой");
        assert!(!dir3.exists(), "dry-run ничего не записал");
    }

    /// qwen: `.qwen/settings.json` + скиллы в `.qwen/skills/` (0.24+);
    /// существующий каталог скиллов не перетирается; dry-run пустой.
    #[test]
    fn qwen_writes_settings_and_qwen_skills() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path().join("proj");
        let report = connect(&ConnectOptions::new(Host::Qwen, dir.clone())).expect("connect");

        assert_eq!(report.skills.len(), 66, "в отчёте все 66 скиллов");
        let settings: Value =
            serde_json::from_str(&read(&dir.join(".qwen/settings.json"))).expect("json");
        assert_eq!(settings["mcpServers"]["spine"]["command"], "arch-be");
        assert!(
            dir.join(".qwen/skills/adr-authoring/SKILL.md").is_file(),
            "скиллы разложены в .qwen/skills"
        );

        // Существующий (даже пустой) .qwen/skills не наполняется.
        let dir2 = tmp.path().join("proj2");
        std::fs::create_dir_all(dir2.join(".qwen/skills")).expect("mkdir");
        let report = connect(&ConnectOptions::new(Host::Qwen, dir2.clone())).expect("connect");
        assert!(report.skills.is_empty(), "скиллы не раскладывались");
        assert!(dir2.join(".qwen/settings.json").is_file());

        // --dry-run: ничего не пишется.
        let dir3 = tmp.path().join("proj3");
        let opts = ConnectOptions {
            dry_run: true,
            ..ConnectOptions::new(Host::Qwen, dir3.clone())
        };
        let report = connect(&opts).expect("dry-run");
        assert!(!report.skills.is_empty(), "план по скиллам непустой");
        assert!(!dir3.exists(), "dry-run ничего не записал");
    }

    /// gigacode: автоопределение каталога настроек — приоритет существующего
    /// `.gigacode/`, затем `.qwen/` (форк), иначе новый `.gigacode/`.
    #[test]
    fn gigacode_autodetect_priorities() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path().join("proj");
        // Ни одного каталога — новый .gigacode/.
        let (path, origin) = gigacode_settings_dir(&dir);
        assert_eq!(origin, GigacodeDirOrigin::NewGigacode);
        assert_eq!(path, dir.join(".gigacode"));
        // Есть только .qwen/ — пишем в него.
        std::fs::create_dir_all(dir.join(".qwen")).expect("mkdir .qwen");
        let (path, origin) = gigacode_settings_dir(&dir);
        assert_eq!(origin, GigacodeDirOrigin::FromQwen);
        assert_eq!(path, dir.join(".qwen"));
        // Есть оба — приоритет .gigacode/.
        std::fs::create_dir_all(dir.join(".gigacode")).expect("mkdir .gigacode");
        let (path, origin) = gigacode_settings_dir(&dir);
        assert_eq!(origin, GigacodeDirOrigin::ExistingGigacode);
        assert_eq!(path, dir.join(".gigacode"));
    }

    /// gigacode в пустом проекте: создаётся `.gigacode/settings.json` (мердж
    /// mcpServers.spine), скиллы — в `.gigacode/skills/` (как у claude, с
    /// обновлением встроенных); хуки — только сниппет; заметка про создание
    /// каталога; повтор идемпотентен.
    #[test]
    fn gigacode_scaffolds_new_settings_dir() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path().join("proj");
        std::fs::create_dir_all(&dir).expect("mkdir proj");
        let report = connect(&ConnectOptions::new(Host::GigaCode, dir.clone())).expect("connect");

        let settings: Value =
            serde_json::from_str(&read(&dir.join(".gigacode/settings.json"))).expect("json");
        assert_eq!(settings["mcpServers"]["spine"]["command"], "arch-be");
        assert_eq!(
            settings["mcpServers"]["spine"]["args"],
            json!(["mcp", "serve"])
        );
        assert!(
            dir.join(".gigacode/skills/adr-authoring/SKILL.md")
                .is_file(),
            "скиллы разложены в .gigacode/skills"
        );
        assert!(!dir.join(".qwen").exists(), "каталога .qwen не появилось");
        assert!(
            report
                .notes
                .iter()
                .any(|n| n.contains("создан") && n.contains(".gigacode")),
            "заметка про создание каталога: {:?}",
            report.notes
        );
        assert!(
            report.snippets.iter().any(|(t, _)| t.contains("Хуки")),
            "сниппет хуков напечатан: {:?}",
            report.snippets
        );
        assert!(
            report
                .next_steps
                .iter()
                .any(|s| s.contains("doctor --host gigacode")),
            "{:?}",
            report.next_steps
        );

        // Идемпотентность: повторный прогон побайтово тот же.
        let first = snapshot(&dir);
        let report = connect(&ConnectOptions::new(Host::GigaCode, dir.clone())).expect("повтор");
        assert_eq!(first, snapshot(&dir), "повторный запуск изменил файлы");
        assert!(report.created.is_empty() && report.merged.is_empty());
    }

    /// gigacode в проекте с существующим `.qwen/`: пишет в него (чужой сервер
    /// сохранён), `.gigacode/` не создаётся; существующий чужой скилл в
    /// `.qwen/skills/` не трогается, встроенные докладываются рядом.
    #[test]
    fn gigacode_reuses_existing_qwen_dir() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path().join("proj");
        std::fs::create_dir_all(dir.join(".qwen/skills/foreign")).expect("mkdir");
        std::fs::write(
            dir.join(".qwen/settings.json"),
            "{\n  \"mcpServers\": {\n    \"other\": {\"command\": \"uvx\", \"args\": [\"x\"]}\n  }\n}\n",
        )
        .expect("write settings");
        std::fs::write(
            dir.join(".qwen/skills/foreign/SKILL.md"),
            "---\nname: foreign\ndescription: чужой\n---\n",
        )
        .expect("write foreign skill");

        let report = connect(&ConnectOptions::new(Host::GigaCode, dir.clone())).expect("connect");

        let settings: Value =
            serde_json::from_str(&read(&dir.join(".qwen/settings.json"))).expect("json");
        assert_eq!(settings["mcpServers"]["spine"]["command"], "arch-be");
        assert_eq!(
            settings["mcpServers"]["other"]["command"], "uvx",
            "чужой сервер цел"
        );
        assert!(
            !dir.join(".gigacode").exists(),
            ".gigacode не создаётся при живом .qwen"
        );
        assert!(
            report
                .notes
                .iter()
                .any(|n| n.contains(".qwen/") && n.contains("совместим")),
            "заметка про наследование .qwen: {:?}",
            report.notes
        );
        assert_eq!(
            read(&dir.join(".qwen/skills/foreign/SKILL.md")),
            "---\nname: foreign\ndescription: чужой\n---\n",
            "чужой скилл не тронут"
        );
        assert!(
            dir.join(".qwen/skills/adr-authoring/SKILL.md").is_file(),
            "встроенные скиллы доложены в .qwen/skills"
        );
    }

    /// gigacode --dry-run: только план, ничего не создаётся.
    #[test]
    fn gigacode_dry_run_writes_nothing() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path().join("proj");
        std::fs::create_dir_all(&dir).expect("mkdir proj");
        let opts = ConnectOptions {
            dry_run: true,
            ..ConnectOptions::new(Host::GigaCode, dir.clone())
        };
        let report = connect(&opts).expect("dry-run");
        assert!(!report.created.is_empty(), "план непустой");
        assert!(!report.skills.is_empty(), "план по скиллам непустой");
        assert!(
            report.notes.iter().any(|n| n.contains("будет создан")),
            "заметка про будущее создание каталога: {:?}",
            report.notes
        );
        assert_eq!(
            std::fs::read_dir(&dir).expect("read dir").count(),
            0,
            "dry-run ничего не записал"
        );
    }
}
