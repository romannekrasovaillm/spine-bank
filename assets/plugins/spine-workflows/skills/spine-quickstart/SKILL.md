---
name: spine-quickstart
description: Подключение Spine (arch-be) к текущему харнессу и проверка, что всё живо. Используй этот навык, когда в проекте появился Spine (MCP-сервер spine, каталоги .claude/skills, .qwen/skills, .kimi-code), когда агент «не видит» инструменты spine, когда нужно проверить подключение или объяснить его коллеге, при вопросах «как подключить Spine», «почему нет инструментов spine», «что за сервер spine в MCP».
---

# Spine Quickstart для агента-хоста

Spine = MCP-сервер `arch-be mcp serve` (stdio JSON-RPC): 20 read-only
инструментов контроля и знаний; с `--rw` — ещё и записывающие
(`adr_new`, `handoff_create`, `agentsmd_generate`, …). Тебе (агенту) они
видны как `mcp__spine__*` (Claude/Kimi) или `spine__*` (OpenClaw) или по
имени сервера в omp/Qwen.

## Проверка подключения (всегда начинай с неё)

1. `fitness_check` с `{"repo": "."}` — самый дешёвый живой вызов. Ответ
   `passed` + `issue_count`. Если ошибка «неизвестный инструмент» — сервер
   не подключён: скажи пользователю запустить `arch-be connect <хост>` в
   корне проекта и перезапустить харнесс (для Qwen/GigaCode ≥ 0.24 ещё и
   `qwen mcp approve spine`).
2. `plugin_list` — сколько плагинов/скиллов зашито в бинарь.

## Границы (не нарушай)

- У Spine НЕТ своей LLM: не проси spine «написать текст» — судьбу по текстам
  делай сам (см. скилл `spine-adr-judge`).
- `bash`, `write_file`, `edit_file`, `read_file`, `glob`, `grep` — у хоста
  свои; Spine их не отдаёт никогда (never-список).
- Инструменты `--rw` пишут файлы — если сервер без `--rw`, вызов `adr_new`
  вернёт отказ; предложи включить `--rw` в конфиге сервера.

## Если инструмента «нет»

- omp: активируй инструмент BM25-поиском (`search_tool_bm25` по имени).
- Kimi: project-сервер молча пропускается в untrusted-каталоге — попроси
  пользователя один раз подтвердить «Trust this folder».
- Qwen/GigaCode: `qwen mcp list` → сервер должен быть «Connected».
