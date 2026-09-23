# SPINE-CORE-API — поверхность будущего ядра `spine-core`

Статус: **предложение** (волна B2 релиза 0.3.7). Документ фиксирует, что
считается стабильной поверхностью будущего выделенного ядра, а что —
внутренним устройством продукта. Ратифицируется вместе с ADR-054; физический
сплит в cargo workspace запланирован на 0.4.0, в 0.3.7 код `src/` не двигается.

## Назначение ядра

Ядро — детерминированный контур архитектурного контроля (AD-1, AD-2):
типизированная модель архитектуры, реестр fitness-правил и их исполнение,
единый гейт с машинным вердиктом, дифф контрактов, рубрики с
evidence-bound судьёй, протокол MCP. Ядро не содержит TUI, агентного цикла
и сетевых LLM-провайдеров: оно пригодно как «орган чужого CLI-агента»
(план инверсии, `docs/INVERSION.md`) и как зависимость доменных редакций
(Banking Edition, AI/ML Edition) без перетаскивания продуктового хвоста.

## Граница сегодня (факт, под гейтом)

Граница уже выражена cargo-фичами: `default = ["harness"]`, слим-профиль
`--no-default-features --features core`. Из 76 модулей верхнего уровня
(`src/lib.rs`) harness-гейтинг несут 12: `agent`, `bench`, `cron`,
`distill`, `eval`, `harness`, `net`, `ralph`, `subagent`, `tui`, `web`,
`worktree`; внутри модулей — сетевые провайдеры `llm::{deepseek, gigachat,
glm, kimi, openai_compat}` и harness-команды CLI.

Дерево зависимостей core-профиля не содержит опциональных harness-крейтов
(`reqwest`, `ratatui`, `crossterm`, `arboard`, `scraper`, `tokio-util`;
`tokio` — неопциональная зависимость, допустима). Это не декларация, а
удерживаемый гейт: правило **C-34 `core_boundary_no_tui_net`**
(`CONSTRAINTS.yaml`) прогоняет `scripts/check_core_deps.sh` в догфуде и CI.

## Что войдёт в `crates/spine-core` (план 0.4.0, ADR-054)

Минимальный контур и его транзитивные зависимости — все перечисленные
модули сегодня собираются в core-профиле (сверено по факту):

- **Модель и графы**: `src/model.rs` + `src/model/` (типизированная модель
  ADR-003, дрейф, валидация, граф, реестр проектов, обмен), `src/landscape.rs`,
  `src/adr_registry.rs`, `src/openspec.rs`, `src/fleet.rs`, `src/export.rs`.
- **Контур контроля**: `src/control/`, `src/gate/`, `src/review.rs`,
  `src/trace.rs`, `src/evidence.rs`, `src/nfr.rs`, `src/delta.rs`,
  `src/redteam.rs`, `src/rehearsal.rs`, `src/trust.rs`, `src/passport.rs`,
  `src/rules_suggest.rs`, `src/rule_templates.rs`, `src/cmd_trust.rs`.
- **Контракты**: `src/contract_diff/`, `src/openapi.rs`, `src/asyncapi.rs`,
  `src/archunit.rs`.
- **Рубрики и оценка**: `src/rubric/`, `src/rubric_pack.rs`, `src/judge.rs`
  (происхождение оценки, ADR-048), `src/stubs.rs`.
- **Протокол MCP**: `src/mcp_server/`, `src/mcp.rs`, `src/mcp_journal.rs`,
  `src/digest.rs`.
- **Handoff и сопровождение**: `src/handoff.rs`, `src/agentsmd.rs`,
  `src/archify.rs`, `src/mermaid.rs`, `src/metrics.rs`, `src/survey.rs`,
  `src/selftest.rs`, `src/doctor.rs`, `src/publish.rs`.
- **Инфраструктура ядра**: `src/error.rs`, `src/config.rs`, `src/hash.rs`,
  `src/proc.rs`, `src/secrets.rs`, `src/retry.rs`, `src/matchers.rs`,
  `src/policy.rs`, `src/hooks.rs`, `src/assets.rs`, `src/bootstrap.rs`,
  `src/plugin.rs`, `src/kb.rs`, `src/memory.rs`, `src/failure_memory.rs`,
  `src/injection.rs`, `src/detectors.rs`, `src/report_fmt.rs`,
  `src/clipboard.rs`, `src/tool.rs`, `src/tools.rs`, `src/llm.rs`
  (трейт `LlmProvider`, реестр, типы сообщений, `llm::harness_cli`).

Точный состав замыкается при исполнении ADR-054: модуль входит в ядро,
если он нужен перечисленному контуру по построению и не тянет
harness-зависимостей (проверяемо C-34 уже сегодня).

## Что остаётся продуктовым (`crates/arch-be`)

- **TUI** (`src/tui.rs`, `src/tui/`) и буфер обмена как UX продукта.
- **Агентный цикл** (`src/agent.rs`, `src/agent/`), слеш-команды, компакция.
- **Сетевые LLM-провайдеры** (`src/llm/{deepseek,gigachat,glm,kimi,
  openai_compat}.rs`) и сетевой слой (`src/net.rs`, `src/web.rs`).
- **Продуктовые контуры**: `connect` (подключение к хост-агентам), `bench`,
  `cron`, `distill`, `eval`, `harness` (прогон кодовых харнессов), `ralph`,
  `subagent`, `worktree`.
- **Банковский профиль**: зона `banking/` (проприетарная надстройка по
  ADR-013, в публичный снапшот не входит) и BE-правила реестра.
- Точка входа `src/main.rs` + проводка CLI (`src/cli/`): бинарь `arch-be`
  собирает продукт поверх ядра.

## Точки расширения доменов — трейты, а не правка ядра

Доменные редакции (banking, aiml) расширяют ядро через устойчивые швы,
не редактируя его механику:

- **Трейт `Tool`** (`src/tool.rs:216`): доменный инструмент =
  `impl Tool` (`spec` + `call`) + регистрация в реестре; ядро не знает
  о домене.
- **Инжекция `&dyn LlmProvider`** в судье рубрик (`src/rubric/judge.rs`):
  судья работает с любым провайдером — внутрипериметровым GigaChat,
  внешним CLI (`llm::harness_cli`) или будущей доменной реализацией.
  Ядро задаёт контракт, домен подставляет реализацию.
- **Шаблоны правил** (`src/rule_templates.rs`): библиотека исполняемых
  шаблонов живёт в ассетах; `apply` печатает фрагмент правила, а реестр и
  спайн правит дельта с решением архитектора (П3 ADR-050).
- **Триггеры значимости** (`src/control/diff_triggers.rs`): 15 канонических
  триггеров с механическим anti-bypass floor; пороги маршрутов — в конфиге
  (ADR-034), домен калибрует маршрут без правки механики.
- **Плагины и скиллы** (AD-10): доменное знание (рубрики, промпты, хуки)
  поставляется плагинами и подключается без перекомпиляции ядра.

## Политика стабильности

- **pub API ядра — semver**: после выделения `crates/spine-core` ломающие
  изменения публичных типов и функций ядра — только с major-версией.
- **Машинные контракты как протокол**: схема вердикта `arch-be/gate-verdict/v1`
  (`src/gate/types.rs`) и exit-коды CLI (0 — PASS, 1 — FAIL, 3 — INCOMPLETE;
  ADR-040) — контракт для SDK и CI. Аддитивные поля — minor-эволюция;
  переименование или смена семантики — новая версия схемы (`v2`).
- **Внутреннее — не контракт**: устройство модулей, не перечисленных в
  разделе «Что войдёт», человеко-читаемый текст отчётов и раскладка
  подмодулей внутри семей (`control/`, `gate/`, `rubric/`…) могут меняться
  без бампа major, пока публичные пути и машинные контракты сохранены.
- До физического сплита политика носит характер обязательства: граница
  удерживается фичей `core` и правилом C-34; с выходом 0.4.0 она становится
  границей crate и начинает версионироваться.

## Ссылки

ADR-054 (план сплита), ADR-010 (продуктовый форк), ADR-013 (open-core),
ADR-034 (маршрут значимости), ADR-040 (трёхзначный вердикт), ADR-050
(библиотека шаблонов), ADR-053 (модель доверия `command_succeeds`),
`docs/INVERSION.md` (план инверсии, шаг 4 — cargo-фичи),
`scripts/check_core_deps.sh`, правило C-34 в `CONSTRAINTS.yaml`.
