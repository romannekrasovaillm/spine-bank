# Отчёт по плану инверсии (spine-bank-inversion.md)

Статус на 2026-09-17, релиз v0.2.x. План — внешний документ; здесь — что из
него сделано и где смотреть.

| Шаг плана | Статус | Где |
|---|---|---|
| 1. `CliHarnessProvider` (судья через `claude -p`/…, `kind="cli"`) | ✅ | `src/llm/harness_cli.rs`; прогон: `rubric run` через подписку Claude Code — [скриншот](screenshots/connect/06-rubric-cli-judge.png) |
| 2. MCP: итерация по реестру инструментов, read-only дефолт, `--rw`, never-список | ✅ | `src/mcp_server.rs` (мост поверх `tools::full_registry`, белые списки, охранные тесты) |
| 2+. Split-judge (`rubric_prompt`/`rubric_verify`) | ✅ | там же; проверено на Claude Code и Qwen Code 0.24.0 |
| Бэклог волны 3, п. 11: MCP prompts — плейбуки `spine-*` как слэш-команды хоста | ✅ серверная сторона (живые проверки хостов — отдельно) | `src/mcp_server.rs`: capability `prompts`, `prompts/list`/`prompts/get` (7 плейбуков spine-workflows; текст — пользовательская копия из `plugins.dirs`, иначе встроенный ассет); `resources/*` по-прежнему не поддержаны (`-32601`) |
| Бэклог волны 3, п. 13: составные инструменты | ✅ | `src/review.rs`: `architect_review` (гейт + `model_validate` + линт контрактов одним вызовом; CLI `arch-be review`) и `change_impact` (радиус изменения по графу модели: сущности/правила/контракты/владельцы; CLI `arch-be model impact`); оба — read-only мост MCP (`architect_review`, `change_impact`) |
| Бэклог волны 3, п. 14: контракты шире OpenAPI/AsyncAPI | ✅ | `src/contract_diff.rs`: детектор формата (расширение/содержимое, `format=auto\|openapi\|proto\|avro\|jsonschema\|ddl`) + правила protobuf/gRPC (CD-P01..CD-P06, `reserved` узаконивает удаление), Avro (CD-A01..CD-A05, default/промоушены), JSON Schema топиков (CD-J01..CD-J05), DDL-миграций (CD-S01..CD-S05, итоговое состояние файла); «ломающий дифф без смены major — error» где major определим (CD-007 по `info.version`, CD-P06 по суффиксу пакета `.vN`); связка с моделью (`--model` CLI/Tool): `INT.contract` → `change_impact` → потребители/владельцы (ADR-035); CLI `arch-be contract-diff` (breaking → exit 1) |
| 3. `export <host>` → реализовано как **`arch-be connect <host>`** (имя `export` было занято журнальным экспортом) | ✅ | `src/connect.rs`: claude / qwen / kimi / omp / codex / generic |
| 4. Cargo-фичи `core`/`harness` + сторожок в CONSTRAINTS/CI | ✅ | `Cargo.toml` (`default=["harness"]`), CONSTRAINTS.yaml C-29/C-30, CI-джоба `core-build` |
| 5. Регресс кейсов 004–007 | ✅ частично | 006 (drift-control): A exit 1 / B exit 0; 007 (fleet-spine-drift): exit 1. 004/005 — флоты Claude Code не перепрогонялись; вместо них живой E2E на одном агенте ([HARNESSES.md](HARNESSES.md)) |
| Судья, вариант 3 (`McpSamplingProvider`, sampling у хоста) | ⏭ не делали | План помечает его опциональным; поддержка sampling у CLI-хостов пока неровная |

Принципиальные отступления от плана (осознанные):

- **Имя команды**: `connect` вместо `export` (коллизия имён).
- **`archify_show`/`archify_compare`/`reverse_survey`** — в `--rw`, а не в
  read-only: они атомарно пишут файлы, называть их read-only было бы ложью
  клиенту.
- **`nfr`/`delta`/`evidence`** больше не CLI-only (транш 1, 2026-09-18):
  появились Tool-реализации (`nfr_check`, `model_validate`, `delta_guard`,
  `evidence_verify` — read-only мост; `evidence_pack`, `delta_propose` —
  под `--rw`) с JSON-вердиктом `passed`/`issues`/`summary` и политикой
  R-уровней.
- **Отчётный контур** (транш 2, 2026-09-18): `landscape_report`,
  `adr_registry`, `rules_report`, `openspec_coverage`, `model_graph` —
  read-only мост, JSON со счётчиками + markdown/mermaid отчёт; `passed=false`
  только у strict-режимов `adr_registry`/`openspec_coverage` (семантика
  `--strict` CLI), у отчётов ландшафта/правил/графа `passed` не применим.
- **`handoff_create` — в core** (волна 2, п.10, 2026-09-18): генерация
  handoff-пакета вынесена из `src/harness.rs` в core-модуль `src/handoff.rs`
  (чисто файловая, без сети/TUI); инструмент регистрируется в
  `tools::domain_tools` в обеих сборках, core-сборка `arch-be mcp serve --rw`
  отдаёт его мостом. Кейс drift-control воспроизводится core-бинарём в части
  создания пакета (тест в `tests/mcp_serve.rs` на фикстуре кейса 006).
  `harness_run` и адаптеры кодовых харнессов остаются harness-only; CLI
  `arch-be handoff` тоже (его поверхность — связка с адаптерами; core-путь
  создания пакета — MCP `handoff_create`).
- **never-список** шире планового: кроме `bash`/`write_file`/`harness_run`
  наружу не отдаются также `read_file`/`glob`/`grep`/скриншоты/`subagent_*`/
  `ralph_*`/`worktree_new`/`web_*` — это всё принадлежит хосту.

Что теряется (по плану, сознательно): живой mermaid на боковой вкладке TUI,
пикер моделей, индикатор контекста — это UX хоста. Полная сборка
(`--features harness`) их сохраняет.
