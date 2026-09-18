# GigaCode CLI + Spine: быстрое развёртывание силами самого агента

Эта инструкция — для архитектора, который уже **склонировал репозиторий
Spine локально** и хочет, чтобы **агент GigaCode сам всё развернул**:
MCP-сервер, скиллы, хуки-гейты. GigaCode CLI — форк Qwen Code, поэтому все
шаги проверены живьём на qwen-code (0.0.5 и 0.24.0).

> **Нативная поддержка.** Начиная с волны 2 у `arch-be connect` есть хост
> `gigacode`: каталог настроек определяется автоматически (существующий
> `.gigacode/`; иначе существующий `.qwen/` — форк совместим; иначе
> создаётся `.gigacode/`), скиллы раскладываются в `<каталог>/skills/`,
> проверка — `arch-be doctor --host gigacode`. Инструкция ниже с промптом
> для агента остаётся рабочим путём (в т.ч. для старых версий arch-be);
> ручная адаптация путей при `connect gigacode` не нужна.

> **Закрытый контур (без интернета) — офлайн-бандл.** На машине с
> исходниками Spine: `scripts/make_offline_bundle.sh` собирает
> `dist/spine-offline-<версия>-<os>-<arch>.tar.gz` (core-редакция: без сети
> и TUI; `--edition full` — полная; готовый бинарь — `--binary ПУТЬ`).
> Внутри: бинарь, вендоренный движок Archify (`vendor/archify/`, BE-22),
> `SHA256SUMS` и `install.sh`. Ассеты (промпты/рубрики/скиллы) встроены в
> бинарь — `arch-be init` работает офлайн. Установка на целевой машине:
> `tar xzf spine-offline-*.tar.gz && cd spine-offline-* && ./install.sh`,
> проверка — `arch-be doctor`. Дальше в корне проекта:
> `arch-be connect gigacode` → `arch-be doctor --host gigacode`.

> **ВАЖНО про пути.** Ниже в промпте для агента фигурируют каталоги Qwen
> Code — `.qwen/settings.json` и `.qwen/skills/`. В GigaCode CLI они могут
> называться иначе (например `.gigacode/settings.json` и
> `.gigacode/skills/`) — **адаптируйте пути под фактический каталог вашей
> сборки** (посмотрите, какой каталог создаёт GigaCode в проекте, или сверьтесь
> с его документацией). Механика и форматы ключей (`mcpServers`, skills,
> hooks) унаследованы от Qwen Code без изменений.

> Что получится в итоге: агент GigaCode сможет вызывать 20 детерминированных
> инструментов Spine (fitness-гейты, трасса, модель, контракты, рубрики),
> видеть 62 архитектурных скилла и останавливаться на красном гейте.

## Часть 1. Для архитектора (что происходит)

1. GigaCode ставит/собирает бинарь `arch-be` (из релиза, офлайн-бандла или
   из вашего клона).
2. `arch-be connect gigacode` (или `connect qwen` на старых версиях):
   MCP-сервер `spine` в `<каталог настроек>/settings.json` + 62 скилла в
   `<каталог настроек>/skills/` — одной командой; каталог настроек
   определяется автоматически (`.gigacode/` → `.qwen/` → новый `.gigacode/`).
3. Одобряет сервер (`qwen mcp approve spine`).
4. Опционально включает информационный хук гейта (SessionEnd; в headless
   не файрит — блокирующие гейты есть у Claude Code/Kimi/omp/OpenClaw).
5. Проверяет: `arch-be doctor --host gigacode` (механическая проверка:
   бинарь в PATH, settings.json, скиллы, версия хоста), затем вызывает
   `fitness_check` и докладывает вердикт.

Всё это делает сам агент — вы только выдаёте ему промпт из части 2.

## Часть 2. Промпт для агента GigaCode

Откройте GigaCode в корне вашего проекта и вставьте:

```text
Разверни Spine (arch-be) в этом проекте по следующей инструкции.
(проверено прогоном этого же промпта на qwen-code 0.24.0)
Репозиторий Spine уже склонирован локально: <ПУТЬ_К_КЛОНУ, напр. ~/spine-bank>

ВАЖНО: пути `.qwen/settings.json` и `.qwen/skills/` ниже — от Qwen Code.
В GigaCode CLI адаптируй их под фактический каталог конфигурации твоей
сборки (например `.gigacode/settings.json` и `.gigacode/skills/`): проверь,
какой каталог уже есть в проекте или создаётся твоей версией, и пиши туда.
Форматы ключей (mcpServers, skills, hooks) не меняются.

1. Бинарь:
   - Если `arch-be` есть в PATH (`command -v arch-be`) — используй его.
   - ВАРИАНТ А (быстрый, готовый бинарь из GitHub Releases):
     curl -L -o arch-be https://github.com/romannekrasovaillm/spine-bank/releases/latest/download/arch-be-core-linux-x86_64
     && chmod +x arch-be && mv arch-be ~/.local/bin/
     (создай ~/.local/bin при нужде; сверь sha256 с SHA256SUMS.txt из релиза).
   - ВАРИАНТ Б (сборка из локального клона): cd <ПУТЬ_К_КЛОНУ> &&
     cargo build --release --no-default-features --features core && скопируй
     target/release/arch-be в ~/.local/bin/.
   - ВАРИАНТ В (закрытый контур, офлайн-бандл): распакуй
     spine-offline-*.tar.gz и выполни ./install.sh из него (бинарь, движок
     Archify, init — всё офлайн; целостность проверяется по SHA256SUMS).
   - Проверь: `arch-be --version` (ожидается 0.2.x).

2. MCP-сервер + скиллы (project-level, НЕ затирай существующее — мердж):
   - Выполни `arch-be connect gigacode` в корне проекта (если твоя версия
     arch-be его ещё не знает — `arch-be connect qwen` с адаптацией путей
     ниже) — это запишет `<каталог настроек>/settings.json` (mcpServers.spine)
     И разложит 62 скилла в `<каталог настроек>/skills/` (каталог выбирается
     автоматически: существующий `.gigacode/`, иначе `.qwen/`, иначе новый
     `.gigacode/`; project scope skills — как в qwen-code ≥ 0.24).
     Если каталог скиллов уже существовал и остался без встроенных —
     скопируй скиллы вручную из клона: assets/plugins/*/skills/*/.
   - Одобри сервер: `qwen mcp approve spine` (в 0.24 project-серверы требуют
     одобрения) — или подтверди диалог при следующем интерактивном запуске.

3. Хук-гейт (информационный): в .qwen/settings.json можно добавить
   "hooks": {"SessionEnd": [{"hooks": [{"type": "command", "command":
   "arch-be gate --route auto 2>&1 | tail -3"}]}]}
   ВНИМАНИЕ: в qwen-code 0.24 хуки управляются через `qwen hooks` (UI) и
   в headless-режиме не файрят — этот хук показывает вердикт гейта, но НЕ
   блокирует завершение. Если hooks не поддерживаются твоей версией —
   пропусти и доложи. Блокирующие гейты есть у Claude Code/Kimi/omp/OpenClaw.

4. Проверка (обязательна):
   - Механическая: `arch-be doctor --host gigacode` (бинарь в PATH, запись
     mcpServers.spine, скиллы на месте, версия хоста); код выхода 1 — разбор
     находки до продолжения.
   - Вызови MCP-инструмент spine fitness_check с {"repo": "."} и доложи
     вердикт (passed true/false, число находок).
   - Вызови skill_search с {"query": "saga"} и перечисли 3 найденных скилла.
   - Вызови model_query с {"dir": "model"} (если каталога model/ нет —
     скажи об этом, это не ошибка).

5. Финал: доложи одной сводкой — версия arch-be, статус MCP (Connected),
   число разложенных скиллов, статус хука, вердикт fitness_check.
   Ничего не коммить в git. Секреты/ключи не выводи.
```

## Часть 2.5. Наполнение пустого проекта (банковской зоны нет — наполняем локально)

В публичном форке нет слоя `banking/` — спайн, правила, модель и базу знаний
проекта архитектор создаёт под себя. Это тоже делает агент по промпту:

```text
Наполни Spine-контур этого проекта с нуля (шаблоны — в клоне Spine:
кейсы/*/handoff-example/, assets/rubrics/):

1. ARCHITECTURE-SPINE.md — 2-4 инварианта проекта, формат
   «## AD-N: <название>» + Binds/Prevents/Rule.
2. .arch-handoff/CONSTRAINTS.yaml — fitness-правила (must_contain /
   must_not_contain / command_succeeds), и копию в корень (для trace_check).
3. model/ — сущности с frontmatter: SYS-001 (система), REQ-001, NFR-001
   (verified_by: [C-NNN]), CMP-001 (implements: [...]), AD-1… (affects,
   verified_by) — чтобы trace_check давал PASS.
4. knowledge/ — заметки/стандарты проекта (.md); зарегистрируй их в
   arch-harness.toml: [knowledge] dirs = ["knowledge"].
   ВАЖНО: kb_search — BM25 по дословным токенам: пиши ключевые термины
   в тексте явно (русский и английский вариант термина).
5. docs/adr/ — первый ADR (прозой или через adr_new, если сервер в --rw).
6. Проверка: fitness_check, trace_check, kb_search, model_query —
   доложи вердикты.
```

Проверено живьём (qwen-code): агент сам создал спайн, правила и
knowledge-документ; `fitness_check` сразу начал ловить отсутствие кода
(FAIL по `tests_present` — правильно, src/ ещё не было), `kb_search` нашёл
документ по точному термину.

## Часть 3. Что дальше — работа архитектора

### Плейбуки `spine-*`: зачем и как запускать

Помимо инструментов MCP, `connect qwen` разложил в `.qwen/skills/` (или
`.gigacode/skills/` — см. заметку про пути выше) семь **плейбуков** —
скиллов, которые учат самого агента правильно водить Spine. MCP даёт
инструменты; плейбук даёт МЕТОД: в каком порядке вызывать, на что смотреть
в ответах, какие оговорки делать, где границы (read-only, never-список).

Запуск — просто просите агента по имени скилла, например:

```text
Действуй по скиллу spine-architect-review: разбери этот проект.
```

Проверено живьём (qwen-code 0.24.0): агент прочитал скилл, выполнил разбор
по его шагам (маршрут → модель → трасса → спайн) и отчитался, какие шаги
закрыл.

Те же плейбуки сервер отдаёт и как **MCP-промпты** (capability `prompts`,
`prompts/list`) — тогда ревью запускается командой хоста из меню (в Claude
Code — `/mcp__spine__spine-architect-review`; qwen-code 0.24 промпты
поддерживает). Формулу «действуй по скиллу…» и расположение файлов скиллов
помнить не нужно: текст плейбука встроен в бинарь сервера и приезжает по
`prompts/get` (пользовательская копия в `plugins.dirs`, если есть, в
приоритете).

| Плейбук | Когда звать |
|---|---|
| `spine-quickstart` | Подключение/диагностика MCP: «не видит инструменты», approve, trust |
| `spine-content-bootstrap` | Пустой проект: создать спайн, CONSTRAINTS.yaml, model/, knowledge/ |
| `spine-architect-review` | Разбор проекта: маршрут значимости, модель, трассировка |
| `spine-adr-judge` | Оценка ADR/документа рубрикой (split-judge, без ключей у Spine) |
| `spine-contracts-gate` | Перед релизом API: линт OpenAPI/AsyncAPI + diff версий |
| `spine-archify-viz` | «Нарисуй архитектуру»: JSON IR → 9 проверок → интерактивный HTML |
| `spine-fitness-gate` | Работа с кодом под гейтом: чтение находок, починка, перепроверка |

Их же можно читать как документацию: это обычные файлы
`.qwen/skills/spine-*/SKILL.md`.

### Свободные запросы

Можно и без плейбука, например:

- «Проверь проект через spine `fitness_check`; если FAIL — исправь и
  перепроверь» (агент сам чинит нарушения CONSTRAINTS.yaml);
- «Оцени маршрут изменения: `significance_score` с триггером
  `new_component=true`»;
- «Покажи модель: `model_query` по `model/`»; «Проверь трассировку:
  `trace_check`»;
- «Оцени ADR по рубрике: `rubric_prompt` → дай 2–3 ответа судьи →
  `rubric_verify`» (split-judge — судит сам GigaCode, ключей Spine не нужно);
- «Проверь контракты: `openapi_lint`, `contract_diff`»;
- «Визуализируй архитектуру: напиши Archify IR и провалидируй
  `archify_validate`, потом `archify_show`» (настройка — ниже).

### Визуализация архитектуры (Archify) из харнесса

1. Движок вендорен в репо Spine: `vendor/archify/` (нужен Node.js ≥ 18).
2. Одна строка в `arch-harness.toml` проекта:

   ```toml
   [archify]
   cli_path = "<путь-к-клону-spine>/vendor/archify/bin/archify.mjs"
   ```

3. Дальше агент сам: пишет JSON IR (`diagrams/*.architecture.json`),
   валидирует через `archify_validate` (9 проверок + supportedFixes для
   точечного ремонта), показывает через `archify_show` — интерактивный HTML
   (guided views, легенда, карточки потоков). Пример результата: [qwen-archify-html.png](screenshots/harnesses/qwen-archify-html.png).

Полная матрица проверенных харнессов и ограничения — в
[docs/HARNESSES.md](HARNESSES.md). Подробное подключение всех хостов —
в [docs/CONNECT.md](CONNECT.md).

## Если что-то пошло не так

| Симптом | Лечение |
|---|---|
| `spine` в статусе Pending approval | `qwen mcp approve spine` (или интерактивный запуск и одобрение) |
| Агент «не видит» инструменты | `qwen mcp list` → должен быть `Connected`; проверьте `command -v arch-be` |
| Сервер одобрен, но в текущей сессии инструменты не привязались | `approve` действует со СЛЕДУЮЩЕЙ сессии — перезапустите GigaCode (в headless — это просто следующий вызов) |
| 404 по модели | Для openai-совместимого бэкенда модель задаётся через `OPENAI_MODEL`, не флагом `-m` |
| Хук не срабатывает | Версия GigaCode может не иметь hooks — проверьте `qwen hooks` / документацию своей сборки |
