# AGENTS.md — Spine Banking Edition

Guidance for AI agents (and humans) working on this repository.
**Reading/exploring the repo instead? See `AGENTS-READERS.md`.**

**Spine Banking Edition** is a *domain agent harness for solution
architectors* (banking): a thin, Rust-built terminal agent with
architecture-specific tooling — ADRs, architecture-spine invariants,
rubrics with an evidence-bound LLM judge, fitness functions, handoff
packages for coding harnesses, a skills/plugins library, background
sub-agents, and governance. One binary, `arch-be`: TUI + CLI + library.
See `README.md` (bilingual RU/EN) for the full feature tour.

> **Product fork.** This repository is Spine Banking Edition: a product
> fork of Spine (`NOTICE.md`, ADR-010). Core is MIT; the `banking/` layer
> is proprietary (`LICENSE.banking`). Product invariants: `ARCHITECTURE-SPINE-BE.md`
> (AD-BE1…BE4) — license boundary, model matrix (GigaChat + self-hosted
> open-source inside the bank perimeter, no YandexGPT), upstream patch
> discipline, product positioning.

## Install & run (one-minute setup)

```bash
cargo build --release          # binary: target/release/arch-be
ln -sf "$PWD/target/release/arch-be" ~/.local/bin/arch-be   # one-word launch: `arch-be`
arch-be init                      # config + assets into ~/.arch-harness and
                               # ~/.config/arch-harness/config.toml

arch-be                           # interactive TUI (default command)
arch-be run -q "draft an ADR for saga adoption" > adr.md   # strict headless
arch-be doctor                    # environment check (keys, dirs, plugins, MCP)
```

API keys come from the environment or key files — never from the config
(values are never stored there):

```bash
export DEEPSEEK_API_KEY=...    # deepseek (v4-flash, default), deepseek-pro (v4-pro)
export ZHIPU_API_KEY=...       # glm (glm-5.2 + budget 4.7/air/flash)
export KIMI_API_KEY=...        # kimi (k3, coding surface) or file ~/.kimi_api_key
```

No-LLM smoke: `arch-be mermaid examples/mermaid/flow.mmd`,
`arch-be control score --trigger new_component=true`, `arch-be doctor`.

## Commands for development

- Build: `cargo build` / fast check: `cargo check`
- Tests: `cargo test` (live-LLM tests are `#[ignore]`d — they need keys and network)
- Lint: `cargo clippy --all-targets` (pedantic warnings are tolerated for now)
- Release: `cargo build --release`
- README screenshots regenerate from code: `ARCH_GEN_SHOTS=1 cargo test gen_readme_screenshots`

## Architecture map

| Area | Files | Role |
|---|---|---|
| Agent loop | `src/agent.rs`, `src/agent/{slash,prompts}.rs` | turn loop, tool dispatch, compaction (L1/prune/L3), session journal (append-only JSONL), failure memory (`src/failure_memory.rs`: повторные сбои → уроки), slash commands |
| LLM | `src/llm.rs`, `src/llm/openai_compat.rs`, `{deepseek,kimi,glm}.rs` | OpenAI-compatible client (SSE streaming, retries, stream-break recovery, `reasoning_content` echo, thinking maps) |
| Tools | `src/tool.rs`, `src/tools/{bash,fs,ask}.rs`, `src/tools.rs` | registry, policy gate (R0–R5), bash with env-scrub + orthogonal outcome markers, file ops with fuzzy edit |
| Domain tools | `src/{bench,model,trace,agentsmd,evidence,metrics,delta,handoff,worktree,subagent,ralph,distill,harness,kb,web,mcp,mcp_journal,mermaid,plugin,eval,review,digest,report_fmt,openapi,asyncapi,adr_registry,landscape,archunit,survey,nfr,fleet,openspec,publish,export,doctor,rehearsal}.rs`, `src/control/{mod,types,rules,registry,exec,report,diff_triggers,templates,tools,baseline}.rs`, `src/gate/{mod,types,git,components,semantic,route,verdict,attest,explain,testkit}.rs`, `src/mcp_server/{mod,types,protocol,testkit,tools/{mod,fitness,model,rubric,knowledge,insight}}.rs`, `src/connect/{mod,types,files,mcp,hooks,skills,hosts,ci,git_hooks,render,testkit}.rs`, `src/rubric/{mod,types,catalog,judge,report,artifact,tools,testkit}.rs`, `src/contract_diff/{mod,types,detect,diffbase,openapi,proto,avro,jsonschema,ddl,report,tools,testkit}.rs`, `src/model/{drift,registry,graph,validate,project,exchange,parse}.rs` | architect-specific tooling (see README); `handoff.rs` (генерация пакета) — core, `harness.rs` (прогон) — harness-only; `gate/` — единый гейт (`arch-be gate`), `review.rs` — составное ревью, `digest.rs` + `mcp_journal.rs` — outcome-данные MCP; `control` разбит на подмодули (B1: types/rules/registry/exec/report/diff_triggers/templates/tools + baseline), публичные пути — через реэкспорты `control/mod.rs`; `gate` разбит на подмодули (B1: types/git/components/semantic/route/verdict/attest/explain + testkit), публичные пути — через реэкспорты `gate/mod.rs`; `mcp_server` разбит на подмодули (B1: types — типы/константы протокола, protocol — JSON-RPC костяк и диспетчер, tools/* — по файлу на семейство ручных инструментов, testkit — фикстуры тестов), публичные пути — через реэкспорты `mcp_server/mod.rs`; `connect` разбит на подмодули (B1: types — типы, files/mcp/hooks — примитивы записи конфигов и хуков, skills — доставка скиллов, hosts — по файлу на хост-агента, ci/git_hooks — гейты без хоста, render — печать отчёта, testkit — фикстуры тестов), публичные пути — через реэкспорты `connect/mod.rs`; `rubric` разбит на подмодули (B1: types — типы рубрики и оценок, catalog — загрузка и список YAML-рубрик, judge — LLM-судья (промпты, сэмплы, динамическая генерация), report — разбор ответов судьи и сборка отчёта со сверкой цитат, artifact — машиночитаемые отчёты `reports/rubric/`, tools — агентные инструменты, testkit — фикстуры тестов), публичные пути — через реэкспорты `rubric/mod.rs`; `contract_diff` разбит на подмодули (B1: types — типы и примитивы локации находок, detect — детектор формата, diffbase — точка входа/правило major/связка с моделью ADR-035, openapi/proto/avro/jsonschema/ddl — по файлу на формат контракта, report — рендер отчёта, tools — агентный инструмент, testkit — общие фикстуры тестов), публичные пути — через реэкспорты `contract_diff/mod.rs` |
| TUI | `src/tui.rs`, `src/tui/{app,input,render,text,theme}.rs` | ratatui Tokyo Night; ask-modal, model picker, tabs, fullscreen viewer; `input.rs` — состояние строки ввода (B1: вынос из `app.rs` под порог 3000 строк) |
| Config | `src/config.rs` | `~/.config/arch-harness/config.toml` (all personal paths live HERE, never in code) |
| Assets | `assets/`, `src/assets.rs` | embedded prompts/rubrics/benchmarks/plugins, deployed by `arch-be init` |
| Entry | `src/main.rs` | CLI (clap) + wiring |

## Conventions (enforced)

- **No `unsafe`**, no `unwrap`/`expect` outside tests. Doc comments are in
  Russian (`///`); user-facing text is Russian; code/identifiers English.
- Errors: `HarnessError`/`Result` (thiserror) in the library; `anyhow` with
  `.context()` at the CLI edge. A tool failure is `ToolOutput::err`, never a panic.
- Tests are deterministic and self-contained: `tempfile` + `Config::default()`
  with overridden `paths.*`; no network, no real home dir, no real plugin
  libraries (fixtures set `plugins.include_hooks = false`).
- **Secrets**: never print, log, or commit key material; keys resolve lazily
  via `api_key_env`/`api_key_file`; tool output and journals pass through the
  redactor (`src/secrets.rs`); spawned commands get a scrubbed environment
  (`[bash] env_scrub`).
- **No personal paths in the repo** — machine-specific directories
  (knowledge bases, plugin libraries) belong to the user config only
  (see README “Configuring personal paths”). This is checked before every push.
- Orthogonal outcomes are reported independently (exit code, signal, timeout,
  truncation — separate markers, never nested).
- Swallowed errors (`let _ = …`) carry a comment naming what is ignored and
  why it is safe. Numeric limits are named `MAX_*` constants with docs.

## How to extend

- **New agent tool**: implement `Tool` (`spec` + `call`), register in
  `tools::domain_tools`, add a doc row in `docs/tools.md`, add tests.
- **New model**: add `[models.<name>]` to the config (base_url, model,
  api_key_env/api_key_file, `thinking_on/off` maps, `context_limit`,
  optional `proxy` — per-provider egress proxy, loopback gateways are
  auto-started via `src/net.rs`).
  Any OpenAI-compatible endpoint works out of the box.
- **New skill/plugin**: a directory under a `[plugins] dirs` entry —
  `plugin.json` + `skills/<name>/SKILL.md` (+ optional `mcp.json`,
  `agents/*.md`, `hooks/hooks.json`). The plugin is the only install unit;
  skills never install separately.
- **New slash command**: `src/agent/slash.rs` — `execute()` arm + `catalog()`
  entry + a test; update `docs/slash_commands.md`.

## Definition of done for a change

1. `cargo test` green (incl. new tests for the change) and
   `cargo build --release` clean.
2. Docs touched by the change updated (`docs/*.md`, README when user-facing).
3. No secrets or personal paths added (run a grep gate before pushing).
4. Session journal facts: user-visible behavior changes are reflected in
   `docs/architecture.md` when the loop contract moves.

## Бенчмарки

- `benchmarks/platformv-arch-bench/` — бенчмарк из 24 архитектурных задач по
  документации Platform V (СберТех): выбор модели, регрессионный гейт фич,
  сравнение кодовых харнессов для handoff. Документация:
  `docs/platformv-benchmark.md`. Тяжёлые прогоны (`runs/`) в git не входят.

<!-- ARCH:GENERATED hash=7663710e594c057f ts="2026-09-02 16:27" -->
> Сгенерировано харнессом `arch` (`arch agents-md refresh`). Не редактируйте
> внутри маркеров — правьте источники (spine, CONSTRAINTS.yaml) или зону снаружи.

## Команды

- Сборка: `cargo build`
- Тесты: `cargo test`
- Линт: `cargo clippy --all-targets`
- CI: GitHub Actions

## Инварианты архитектуры (нарушать нельзя)

- **AD-1 Тонкое ядро — механика в коде, знания в плагинах**
- **AD-2 Детерминированный слой контроля — без LLM**
- **AD-3 Секреты — только через окружение**
- **AD-4 Единый OpenAI-совместимый провайдерный слой**
- **AD-5 Журнал — единственный источник аудита**
- **AD-6 Безопасный Rust — без unsafe, с пином MSRV**
- **AD-7 Тесты и git-операции — изолированы от машины**
- **AD-8 Handoff кодовым харнессам — механический контракт**
- **AD-9 Spine и трассировка — гейт, а не документация задним числом**
- **AD-10 Плагин — единица распространения знаний**

Полный текст: `ARCHITECTURE-SPINE.md`

## Запреты и fitness-правила

- `module-exists` (must_contain, error) 
- `tool-registered` (must_contain, error) 
- `registry-test-updated` (must_contain, error) 
- `ruleset-o001-o005` (must_contain, error) 
- `ruleset-o003-idempotency` (must_contain, error) 
- `ruleset-rfc7807` (must_contain, error) 
- `no-banking-deps-in-core` (must_not_contain, error) 
- `no-new-dependencies` (must_not_contain, error) 
- `docs-tools-row` (must_contain, error) 
- `tests-present` (must_contain, error) 
- `cargo-fmt-clean` (command_succeeds, error) 
- `cargo-clippy-deny-warnings` (command_succeeds, error) 
- `cargo-test-all-targets` (command_succeeds, error) 

Проверка: `arch control check .` — источник `.arch-handoff/CONSTRAINTS.yaml`

## Карта репозитория

- Стек: rust
- Каталоги: assets, banking, benches, docs, examples, src, tests, кейсы
- ADR: `docs/adr` (24 шт.) — решения читаем ДО изменения затронутых мест

## Стоп-условия: когда остановиться и эскалировать архитектору

Прекратите работу и запросите решение архитектора (A3), если изменение затрагивает:
- API/data-контракт, схему данных, security boundary / trust zone;
- новый компонент/хранилище/вендора, cross-domain интеграцию;
- необратимую миграцию, RTO/RPO, финансово значимые потоки.
Маршрут значимости: `arch control score --trigger …` (Fast/Standard/Critical).
<!-- ARCH:END -->
