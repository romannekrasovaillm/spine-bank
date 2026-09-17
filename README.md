<p align="center">
  <img src="docs/screenshots/00-banner.png" alt="Spine Banking Edition — архитектурный контур для CLI-агентов" width="100%">
</p>

<p align="center">
  <b>Spine без собственной LLM: архитектурный контур — «орган» вашего CLI-агента</b><br>
  <sub>MCP-сервер с детерминированным контролем · 55 архитектурных скиллов · хуки-гейты · судья без API-ключей<br>
  Spine as an organ of your coding agent (Claude Code / Codex / Qwen / Kimi) — MCP server + skills + hooks, no LLM keys required.</sub>
</p>

<p align="center">
  <a href="https://github.com/romannekrasovaillm/spine-bank/actions/workflows/ci.yml"><img src="https://github.com/romannekrasovaillm/spine-bank/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <img src="https://img.shields.io/badge/rust-edition_2024-e43717?logo=rust&logoColor=white" alt="Rust edition 2024">
  <img src="https://img.shields.io/badge/license-MIT-green" alt="License MIT">
  <img src="https://img.shields.io/badge/cases-10-blueviolet" alt="10 cases">
</p>

---

Это форк **Spine Banking Edition** (`arch-be`), перевёрнутый по плану
инверсии: харнесс перестаёт вызывать LLM сам и становится MCP-сервером +
пакетом скиллов + хуками **внутри вашего агента**. Думает ваша подписка
(Claude Code, Codex, Qwen Code, Kimi Code) — Spine проверяет инварианты,
блокирует дрейф и судит документы по рубрикам. Ядро — MIT; банковский слой
(`banking/`) в публикацию не входит.

> Полный тур возможностей исходного харнесса (TUI, флоты субагентов,
> бенчмарки, кейсы в деталях, English version) — в **[README-full.md](README-full.md)**.

## Быстрый старт (2 минуты)

```bash
# 1. Бинарь из раздела Releases (Linux x86_64)
curl -L -o arch-be https://github.com/romannekrasovaillm/spine-bank/releases/latest/download/arch-be-linux-x86_64
chmod +x arch-be && mv arch-be ~/.local/bin/

# 2. В корне проекта, который должен контролировать Spine
cd ~/projects/my-project
arch-be connect claude        # или: qwen / codex / kimi / generic
```

![Установка и подключение](docs/screenshots/connect/01-connect.png)

`connect` пишет в проект (мердж без затирания чужого, идемпотентно,
`--dry-run` для предпросмотра):

| Файл | Что даёт |
|---|---|
| `.mcp.json` | MCP-сервер `spine` (`arch-be mcp serve`) — 20 инструментов контроля |
| `.claude/settings.json` | Stop-хук `arch-be control check .`: агент **не может завершить работу**, пока архитектурный гейт FAIL |
| `.claude/skills/` | 55 скиллов: ADR, fitness-функции, saga/outbox/circuit-breaker, офисные отчёты (docx/pptx/xlsx) |
| `CLAUDE.md` | Точка входа с ссылкой на AGENTS.md (блок между маркерами `SPINE:BEGIN/END`) |

Перезапустите агента и проверьте: `claude mcp list` →
`spine: arch-be mcp serve - ✔ Connected`. При первом запуске Claude Code
спросит разрешение на project-сервер из `.mcp.json` — это штатная защита.

**Подробная инструкция со скриншотами для всех хостов — [docs/CONNECT.md](docs/CONNECT.md).**

## Как это работает в сессии

**Агент проверяет проект сам.** Инструмент `fitness_check` возвращает
машиночитаемый вердикт по вашему `CONSTRAINTS.yaml`; видя FAIL, агент чинит
нарушения и перепроверяет:

![fitness FAIL → fix → PASS](docs/screenshots/connect/04-fitness.png)

**Гейт не отпускает, пока не зелёный.** Stop-хук блокирует завершение хода
при нарушениях (fail-soft на инфраструктуру: нет бинаря или
`CONSTRAINTS.yaml` — молча пропускает):

![Stop-хук блокирует завершение](docs/screenshots/connect/05-stop-hook.png)

**LLM-судья — без единого API-ключа**, двумя путями:

1. `kind = "cli"` в конфиге: `arch-be rubric run …` вызывает ваш
   `claude -p` / `codex exec` подпроцессом — платит подписка хоста;
2. split-judge для любого MCP-хоста: `rubric_prompt` отдаёт промпт + схему
   ответа, хост судит сам, `rubric_verify` собирает k ответов в отчёт
   (медиана, проверка цитат, флаги `unstable` / `evidence_not_found`).

![Судья через подписку Claude Code](docs/screenshots/connect/06-rubric-cli-judge.png)

## Что внутри MCP-сервера

```bash
arch-be mcp serve         # read-only: 20 инструментов (контроль + знания)
arch-be mcp serve --rw    # + handoff_create, adr_new, agentsmd_generate, …
```

- **Контроль**: `fitness_check`, `spine_lint`, `significance_score`,
  `trace_check`, `model_query`, `contract_diff`, `openapi_lint`,
  `asyncapi_lint`, `fleet_audit`, `archify_validate`, `agentsmd_lint`.
- **Знания**: `kb_search`, `skill_search`, `skill_load`, `rubric_list`,
  `plugin_list`, `mermaid_render`.
- **Судья**: `rubric_run` (через `kind="cli"`), `rubric_prompt` +
  `rubric_verify` (split-judge для любого хоста).
- **Никогда не отдаются наружу** (у хоста свои): `bash`, `write_file`,
  `edit_file`, `harness_run`, `subagent_*`, `web_*` — зашитый never-список,
  охраняется тестами реестра.

## Сборки

| Сборка | Команда | Что внутри |
|---|---|---|
| Полная (по умолчанию) | `cargo build --release` | TUI, агентный цикл, сетевые LLM-провайдеры — всё как в Spine-BE |
| **Core** (рекомендуем коллегам) | `cargo build --release --no-default-features --features core` | Только MCP + CLI: без reqwest/ratatui, без сетевых LLM (release: ~10 МиБ против ~18 МиБ) |

Границу сторожат CI-джоба `core-build` и fitness-правила C-29/C-30 в
`CONSTRAINTS.yaml` — `reqwest`/`ratatui` в core не протекают.

## Кейсы — сквозные прогоны, а не обещания

Каждый кейс — самодостаточный пример с эталонными выводами, реестр и
конвенции — [`кейсы/AGENTS.md`](кейсы/AGENTS.md). В инверсной схеме
механические кейсы воспроизводятся без единой LLM:

| Кейс | Что показывает |
|------|----------------|
| [drift-control](кейсы/drift-control/) | Голая задача → гейт FAIL 2/6; та же задача + handoff-пакет → PASS 6/6 (`arch-be control check` как судья) |
| [fleet-spine-drift](кейсы/fleet-spine-drift/) | Аудит флота: дрейф `CONSTRAINTS.yaml` как exit-код — полностью механически |
| [parallel-epics](кейсы/parallel-epics/) · [fleet-of-ten](кейсы/fleet-of-ten/) | Спайн как клей флота Claude Code: стыки сходятся с первой сборки |
| [legacy-survey](кейсы/legacy-survey/) · [jvm-archunit-gate](кейсы/jvm-archunit-gate/) · [fleet-patterns](кейсы/fleet-patterns/) | Reverse discovery, гейт по байткоду, движок оркестрации — без LLM |

Остальные шесть кейсов (с LLM-прогонами) — в [README-full.md](README-full.md).

## Документация

- **[docs/CONNECT.md](docs/CONNECT.md)** — подключение со скриншотами:
  Claude Code, Qwen, Codex, Kimi, generic; хуки, `--rw`, устранение неполадок.
- [docs/mcp.md](docs/mcp.md) — контракт MCP-сервера, белые списки, split-judge.
- [README-full.md](README-full.md) — полный тур (TUI, флоты, бенчмарки,
  конфигурация, English section).
- Скриншоты гайда регенерируются из сценариев:
  `docs/screenshots/connect/sessions/*.txt` + `scripts/termshot.py`.

## Для разработчиков форка

```bash
cargo build              # полная сборка (фича harness по умолчанию)
cargo test               # ~1000 тестов, офлайн
cargo test --no-default-features --features core   # core-поднабор
arch-be control check . --constraints CONSTRAINTS.yaml   # догфуд-гейт
```

Конвенции — `AGENTS.md`; инварианты — `ARCHITECTURE-SPINE.md` /
`ARCHITECTURE-SPINE-BE.md`; план инверсии, по которому собран этот форк, —
`spine-bank-inversion.md` (внешний документ, его суть покрыта выше).

## Лицензия

Ядро — MIT (`LICENSE`); `LICENSE.banking` относится к слою `banking/`,
который в публичный снапшот не входит. `NOTICE.md` — происхождение форка.
