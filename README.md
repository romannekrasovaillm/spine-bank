<p align="center">
  <img src="docs/screenshots/00-banner.png" alt="Spine — архитектурный контур для CLI-агентов" width="100%">
</p>

<p align="center">
  <b>Spine: архитектурный контур — для вашего CLI-агента или как самостоятельный харнесс</b><br>
  <sub>MCP-сервер с детерминированным контролем · 55 архитектурных скиллов · хуки-гейты · судья без API-ключей<br>
  Spine as an organ of your coding agent (Claude Code / Kimi / Qwen / omp / OpenClaw) — or a standalone architect harness (TUI + own LLM).</sub>
</p>

<p align="center">
  <a href="https://github.com/romannekrasovaillm/spine-bank/actions/workflows/ci.yml"><img src="https://github.com/romannekrasovaillm/spine-bank/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <img src="https://img.shields.io/badge/rust-edition_2024-e43717?logo=rust&logoColor=white" alt="Rust edition 2024">
  <img src="https://img.shields.io/badge/license-MIT-green" alt="License MIT">
  <img src="https://img.shields.io/badge/harnesses-5 ✓-blueviolet" alt="5 harnesses verified">
</p>

---

## Две версии — какая вам нужна?

<p align="center">
  <img src="docs/screenshots/editions.png" alt="Spine Core (для вашего агента, без своей LLM) vs Spine Harness (самостоятельный TUI-харнесс со своей LLM)" width="96%">
</p>

| | **Spine Core** | **Spine Harness (TUI)** |
|---|---|---|
| Для кого | У вас уже есть кодовый агент (Claude Code, Kimi, Qwen, omp, OpenClaw) | Вы — архитектор и работаете сами, без внешнего агента |
| Что это | «Орган» чужого харнесса: MCP-сервер + скиллы + хуки | Полный харнесс архитектора: TUI + агентный цикл + то же ядро |
| LLM | **Не нужна**: думает ваш агент; судья — через `kind="cli"` или split-judge | Своя: DeepSeek / GLM / Kimi / GigaChat / локальная платформа |
| Бинарь (релиз) | `arch-be-core-linux-x86_64` (~10 МБ) | `arch-be-linux-x86_64` (~19 МБ) |
| Сборка из исходников | `cargo build --release --no-default-features --features core` | `cargo build --release` |

Это форк **Spine Banking Edition**, перевёрнутый по плану инверсии: харнесс
перестаёт вызывать LLM сам и становится MCP-сервером + пакетом скиллов +
хуками **внутри вашего агента**. Ядро — MIT; банковский слой (`banking/`)
в публикацию не входит. Полный тур исходного харнесса —
в [README-full.md](README-full.md).

## Быстрый старт Spine Core (2 минуты)

```bash
# 1. Бинарь из раздела Releases (Linux x86_64)
curl -L -o arch-be https://github.com/romannekrasovaillm/spine-bank/releases/latest/download/arch-be-core-linux-x86_64
chmod +x arch-be && mv arch-be ~/.local/bin/

# 2. В корне проекта, который должен контролировать Spine
cd ~/projects/my-project
arch-be connect claude        # или: kimi / qwen / omp / codex / generic
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

## Проверено на пяти харнессах (живые прогоны, не моки)

Каждый харнесс подключался к Spine, получал проект с нарушением
fitness-правил, **сам** чинил его и перепроверял. Полная матрица со всеми
скриншотами и ограничениями — **[docs/HARNESSES.md](docs/HARNESSES.md)**.

| Харнесс | MCP | FAIL→PASS | Скиллы | Хуки |
|---|---|---|---|---|
| Claude Code | ✅ | ✅ | ✅ 55 | ✅ Stop-гейт |
| Kimi Code | ✅ | ✅ | ✅ Project scope | ✅ Stop (user-level) |
| Qwen Code | ✅ | ✅ (локальная qwen3.8) | ⚠️ через MCP | ❌ нет событий |
| omp (oh-my-pi) | ✅ авто-дискавери `.mcp.json` | ✅ | ✅ нативно | ✅ TS-хук block |
| OpenClaw | ✅ `mcp add` | ✅ | ✅ 55/55 ready | ✅ плагин `before_agent_finalize` |

Скиллы Spine видны агенту нативно — например, Claude Code и omp читают
`.claude/skills` напрямую:

<p align="center">
  <img src="docs/screenshots/harnesses/claude-skills.png" alt="Claude Code видит 55 скиллов Spine" width="47%">
  <img src="docs/screenshots/harnesses/omp-skills.png" alt="omp видит 55 скиллов Spine через discovery .claude/skills" width="47%">
</p>

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
   (медиана, проверка цитат, флаги `unstable` / `evidence_not_found`) —
   проверено на Claude Code (итог 2.70/5 по двум ответам судьи).

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

## Spine Harness (TUI)

Полная сборка — это исходный харнесс архитектора: TUI на ratatui (Tokyo
Night), агентный цикл с компактификацией, пикер моделей, флоты субагентов,
бенчмарки, экспорт сессий в docx/xlsx. Скриншоты и полный тур —
в [README-full.md](README-full.md).

<p align="center">
  <img src="docs/screenshots/02-chat-mermaid.png" alt="TUI Spine Harness" width="72%">
</p>

## Кейсы — сквозные прогоны, а не обещания

| Кейс | Что показывает |
|------|----------------|
| [drift-control](кейсы/drift-control/) | Голая задача → гейт FAIL 2/6; та же задача + handoff-пакет → PASS 6/6 |
| [fleet-spine-drift](кейсы/fleet-spine-drift/) | Аудит флота: дрейф `CONSTRAINTS.yaml` как exit-код — полностью механически |
| [parallel-epics](кейсы/parallel-epics/) · [fleet-of-ten](кейсы/fleet-of-ten/) | Спайн как клей флота Claude Code: стыки сходятся с первой сборки |
| [legacy-survey](кейсы/legacy-survey/) · [jvm-archunit-gate](кейсы/jvm-archunit-gate/) · [fleet-patterns](кейсы/fleet-patterns/) | Reverse discovery, гейт по байткоду, движок оркестрации — без LLM |

Реестр и конвенции — [`кейсы/AGENTS.md`](кейсы/AGENTS.md); ещё шесть кейсов —
в [README-full.md](README-full.md).

## Документация

- **[docs/CONNECT.md](docs/CONNECT.md)** — подключение со скриншотами:
  claude / kimi / qwen / omp / codex / generic; хуки, `--rw`, неполадки.
- **[docs/HARNESSES.md](docs/HARNESSES.md)** — матрица реальных прогонов
  пяти харнессов: MCP, скиллы, хуки, ограничения.
- [docs/mcp.md](docs/mcp.md) — контракт MCP-сервера, белые списки, split-judge.
- [README-full.md](README-full.md) — полный тур (TUI, флоты, бенчмарки, EN).
- Скриншоты регенерируются из сценариев:
  `docs/screenshots/{connect,harnesses}/sessions/*.txt` + `scripts/termshot.py`.

## Для разработчиков форка

```bash
cargo build              # полная сборка (фича harness по умолчанию)
cargo test               # ~1000 тестов, офлайн
cargo test --no-default-features --features core   # core-поднабор
arch-be control check . --constraints CONSTRAINTS.yaml   # догфуд-гейт
```

Конвенции — `AGENTS.md`; инварианты — `ARCHITECTURE-SPINE.md` /
`ARCHITECTURE-SPINE-BE.md`.

## Лицензия

Ядро — MIT (`LICENSE`); `LICENSE.banking` относится к слою `banking/`,
который в публичный снапшот не входит. `NOTICE.md` — происхождение форка.
