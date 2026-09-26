# GigaCode CLI + Spine: быстрое развёртывание силами самого агента

Эта инструкция — для архитектора, который хочет, чтобы **агент GigaCode сам
всё развернул** под свою платформу: выбрал бинарь из релиза, поднял
MCP-сервер, разложил скиллы, подключил хуки-гейты. GigaCode CLI — форк Qwen
Code, поэтому все шаги проверены живьём на qwen-code (0.0.5 и 0.24.0).

> **Версия.** Инструкция сверена с последним релизом на GitHub —
> **v0.3.10** (2026-09-25); в `main` уже идёт 0.3.11, но установочные ссылки
> ниже ведут на `releases/latest`, то есть ровно на опубликованный релиз.
> Релиз публикует **две редакции × четыре платформы** + сводный `SHA256SUMS`
> (см. Часть 0). Что из релиза важно именно этому сценарию:
>
> - **0.3.7** — модель доверия к исполняемым правилам (ADR-053): в MCP-режиме
>   правила `command_succeeds` по умолчанию **не исполняются** (SKIP с
>   находкой `command_untrusted`, гейт `INCOMPLETE`, exit 3). Снятие —
>   `arch-be rules allow` или `ARCH_NO_EXEC=0` в окружении сервера.
> - **0.3.9–0.3.10** — аудиторский след судьи и **архив отчётов рубрики**:
>   и CLI `rubric run`, и MCP `rubric_verify` пишут пару (markdown +
>   JSON-близнец) в `~/.arch-harness/reports/`, в ответе инструмента —
>   поле `archive`. Для записи подключайте сервер как `--rw=reports`.
> - **0.3.10** — `doctor` проверяет задачи cron; `rubric_verify` больше не
>   затирает прошлый отчёт по тому же субъекту.

> **Нативная поддержка.** У `arch-be connect` есть хост `gigacode`
> (алиасы: `giga-code`, `gcode`): каталог настроек определяется
> автоматически (существующий `.gigacode/`; иначе существующий `.qwen/` —
> форк совместим; иначе создаётся `.gigacode/`), скиллы раскладываются в
> `<каталог>/skills/`, проверка — `arch-be doctor --host gigacode`. Ручная
> адаптация путей не нужна; промпт ниже остаётся рабочим путём и для старых
> сборок `arch-be`, где хоста `gigacode` ещё нет (тогда — `connect qwen`).

> **Закрытый контур (без интернета) — офлайн-бандл.** На машине с
> исходниками Spine: `scripts/make_offline_bundle.sh` собирает
> `dist/spine-offline-<версия>-<os>-<arch>.tar.gz`
> (`--edition core|full` — редакция, по умолчанию core; готовый бинарь —
> `--binary ПУТЬ`; каталог — `--out DIR`). Внутри: бинарь, вендоренный
> движок Archify (`vendor/archify/`, BE-22), `SHA256SUMS` и `install.sh`.
> Ассеты (промпты/рубрики/скиллы) встроены в бинарь — `arch-be init`
> работает офлайн. Установка на целевой машине:
> `tar xzf spine-offline-*.tar.gz && cd spine-offline-* && ./install.sh`,
> проверка — `arch-be doctor`. Дальше в корне проекта:
> `arch-be connect gigacode --rw=reports` → `arch-be doctor --host gigacode`.
> Офлайн-бандл — штатный путь для платформ без готового ассета
> (Intel-Mac, Windows arm64, *BSD) и для закрытых контуров.

> **Что получится в итоге:** агент GigaCode сможет вызывать **40
> детерминированных инструментов Spine** read-only (**51** — с `--rw`):
> fitness-гейты, трасса, модель, контракты, NFR, реестры, рубрики; видеть
> **66 архитектурных скиллов**, из них **10 плейбуков** `spine-*` как
> MCP-промпты (слэш-команды хоста), и останавливаться на красном гейте.

## Часть 0. Платформа и редакция — что скачать

Spine публикует готовые бинари **двух редакций** под **четыре платформы**:
выберите свою и назовите её агенту в промпте (Часть 2) — тогда он скачает
правильный файл, а не «первый попавшийся».

| Ваша платформа | Spine Core — нужен GigaCode | Spine Harness — плюс TUI |
|---|---|---|
| Linux x86_64 | `arch-be-core-linux-x86_64` | `arch-be-linux-x86_64` |
| Linux aarch64 (arm64) | `arch-be-core-linux-aarch64` | `arch-be-linux-aarch64` |
| macOS arm64 (Apple Silicon) | `arch-be-core-macos-arm64` | `arch-be-macos-arm64` |
| Windows x86_64 | `arch-be-core-windows-x86_64.exe` | `arch-be-windows-x86_64.exe` |

**Какая редакция нужна.** В GigaCode думает сам GigaCode, а Spine нужен как
«орган»: MCP-сервер + CLI. Это **Core** — без своей LLM и без TUI (~10 МБ).
Полная редакция **Harness** (~19 МБ) добавляет TUI и собственный агентный
цикл: берите, только если хотите работать и в самом Spine. Обе редакции —
одна кодовая база (различие лишь в том, кто «думает»), версии не расходятся.

**Как определить платформу** (агент сделает это сам, но полезно сверить):

```bash
uname -sm        # Linux/macOS: напр. «Linux x86_64», «Darwin arm64»
```

```powershell
$env:PROCESSOR_ARCHITECTURE   # Windows: AMD64 → windows-x86_64
```

Сопоставление: `Linux x86_64` → `linux-x86_64`; `Linux aarch64`/`arm64` →
`linux-aarch64`; `Darwin arm64` → `macos-arm64`; Windows `AMD64` →
`windows-x86_64.exe`. Для платформ без готового ассета — офлайн-бандл
(выше) или сборка из клона (Часть 2, вариант Б).

**Установка, Linux/macOS** (подставьте своё имя файла из таблицы):

```bash
curl -L -o arch-be https://github.com/romannekrasovaillm/spine-bank/releases/latest/download/arch-be-core-linux-x86_64
chmod +x arch-be && mkdir -p ~/.local/bin && mv arch-be ~/.local/bin/
arch-be --version          # ожидается 0.3.10
```

**Установка, Windows (PowerShell):**

```powershell
curl.exe -L -o arch-be.exe https://github.com/romannekrasovaillm/spine-bank/releases/latest/download/arch-be-core-windows-x86_64.exe
# положите arch-be.exe в каталог из PATH (напр. $env:USERPROFILE\bin)
& "$env:USERPROFILE\bin\arch-be.exe" --version
```

**Целостность.** Скачайте `SHA256SUMS` из того же релиза и сверьте:
`sha256sum --check SHA256SUMS` (Linux), `shasum -a 256 --check SHA256SUMS`
(macOS), `Get-FileHash arch-be.exe -Algorithm SHA256` (Windows).

> **ВАЖНО про пути (только для старых сборок).** Если ваша версия `arch-be`
> ещё не знает хоста `gigacode`, в промпте для агента фигурируют каталоги
> Qwen Code — `.qwen/settings.json` и `.qwen/skills/`. Адаптируйте их под
> фактический каталог вашей сборки GigaCode (`.gigacode/…`). Механика и
> форматы ключей (`mcpServers`, skills, hooks) унаследованы от Qwen Code без
> изменений. В актуальных сборках `connect gigacode` делает это сам.

## Часть 1. Для архитектора (что происходит)

1. GigaCode определяет платформу и ставит бинарь `arch-be` под неё: из
   релиза (Часть 0), офлайн-бандла или из вашего клона.
2. `arch-be connect gigacode [--rw=reports]`: MCP-сервер `spine` в
   `<каталог настроек>/settings.json` + 66 скиллов в
   `<каталог настроек>/skills/` — одной командой; каталог настроек
   определяется автоматически (`.gigacode/` → `.qwen/` → новый `.gigacode/`).
   Режим записи MCP:
   - без флага — строго **read-only**, 40 инструментов;
   - `--rw=reports` — узкая запись: только отчёты рубрики (нужно, чтобы
     `rubric_verify` сохранял архив в `~/.arch-harness/reports/`);
   - `--rw` — полный белый список аддитивных записей (`adr_new`,
     `handoff_create`, `delta_propose`, `evidence_pack`, …), 51 инструмент.
     Never-список (`bash`, `write_file`, `edit_file`, `subagent_*`, `web_*`)
     закрыт навсегда: это принадлежность хоста.
3. Одобряет сервер (`qwen mcp approve spine` или аналог вашей сборки).
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
(проверено прогонами на qwen-code 0.0.5 и 0.24.0 — GigaCode CLI его форк;
 последний релиз Spine на GitHub — v0.3.10)
Репозиторий Spine (если он есть локально): <ПУТЬ_К_КЛОНУ, напр. ~/spine-bank>

ВАЖНО: пути `.qwen/settings.json` и `.qwen/skills/` ниже — от Qwen Code.
В GigaCode CLI подставь фактический каталог конфигурации твоей сборки
(например `.gigacode/settings.json` и `.gigacode/skills/`): проверь, какой
каталог уже есть в проекте или создаётся твоей версией, и пиши туда.
Форматы ключей (mcpServers, skills, hooks) не меняются.

0. Платформа и редакция (сделай до скачивания):
   - Определи ОС и архитектуру: `uname -sm` (Linux/macOS) или
     `$env:PROCESSOR_ARCHITECTURE` (Windows PowerShell). Доложи, что нашёл.
   - Сопоставь файл релиза (Spine Core — то, что нужно GigaCode; полная
     редакция нужна только если просили ещё и TUI):
       Linux x86_64                  → arch-be-core-linux-x86_64
       Linux aarch64/arm64           → arch-be-core-linux-aarch64
       macOS arm64 (Apple Silicon)   → arch-be-core-macos-arm64
       Windows x86_64 (AMD64)        → arch-be-core-windows-x86_64.exe
     Готового ассета нет (Intel-Mac, Windows arm64, *BSD) — не выдумывай
     имя файла: иди вариантом В (офлайн-бандл) или Б (сборка из клона).
   - Скачай SHA256SUMS из того же релиза и сверь хэш бинаря
     (sha256sum / shasum -a 256 / Get-FileHash). Не сходится — стоп и доклад.

1. Бинарь:
   - Если `arch-be` есть в PATH (`command -v arch-be`) — используй его.
   - ВАРИАНТ А (быстрый, готовый бинарь из GitHub Releases):
     curl -L -o arch-be https://github.com/romannekrasovaillm/spine-bank/releases/latest/download/<ФАЙЛ_ИЗ_ШАГА_0>
     && chmod +x arch-be && mkdir -p ~/.local/bin && mv arch-be ~/.local/bin/
     (Windows: curl.exe -L -o arch-be.exe ...\arch-be-core-windows-x86_64.exe
      и положи arch-be.exe в каталог из PATH).
   - ВАРИАНТ Б (сборка из локального клона): cd <ПУТЬ_К_КЛОНУ> &&
     cargo build --release --no-default-features --features core && скопируй
     target/release/arch-be в ~/.local/bin/ (или в каталог из PATH).
   - ВАРИАНТ В (закрытый контур, офлайн-бандл): распакуй
     spine-offline-*.tar.gz и выполни ./install.sh из него (бинарь, движок
     Archify, init — всё офлайн; целостность проверяется по SHA256SUMS).
   - Проверь: `arch-be --version` (ожидается 0.3.10).

2. MCP-сервер + скиллы (project-level, НЕ затирай существующее — мердж):
   - Выполни `arch-be connect gigacode --rw=reports` в корне проекта (если
     твоя версия arch-be его ещё не знает — `arch-be connect qwen
     --rw=reports` с адаптацией путей ниже) — это запишет
     `<каталог настроек>/settings.json` (mcpServers.spine)
     И разложит 66 скиллов в `<каталог настроек>/skills/` (каталог
     выбирается автоматически: существующий `.gigacode/`, иначе `.qwen/`,
     иначе новый `.gigacode/`; project scope skills — как в qwen-code ≥ 0.24).
     `--rw=reports` включает узкую запись отчётов рубрики; без флага сервер
     строго read-only, а широкий `--rw` открывает ещё одиннадцать пишущих
     инструментов — выбирай осознанно и доложи, какой режим записал.
     Если каталог скиллов уже существовал и остался без встроенных —
     скопируй скиллы вручную из клона: assets/plugins/*/skills/*/.
   - Одобри сервер: `qwen mcp approve spine` (в 0.24 project-серверы требуют
     одобрения) — или подтверди диалог при следующем интерактивном запуске.

3. Хук-гейт (информационный): в <каталог настроек>/settings.json можно
   добавить "hooks": {"SessionEnd": [{"hooks": [{"type": "command",
   "command": "arch-be gate --route auto 2>&1 | tail -3"}]}]}
   ВНИМАНИЕ: в qwen-code 0.24 хуки управляются через `qwen hooks` (UI) и
   в headless-режиме не файрят — этот хук показывает вердикт гейта, но НЕ
   блокирует завершение. Если hooks не поддерживаются твоей версией —
   пропусти и доложи. Блокирующие гейты есть у Claude Code/Kimi/omp/OpenClaw.

4. Проверка (обязательна):
   - Механическая: `arch-be doctor --host gigacode` (бинарь в PATH, запись
     mcpServers.spine, скиллы на месте, версия хоста, наличие
     python3/pytest/java/mvn для применённых шаблонов, задачи cron); код
     выхода 1 — разбор находки до продолжения.
   - Вызови MCP-инструмент spine fitness_check с {"repo": "."} и доложи
     вердикт (passed true/false, число находок).
     Если в вердикте есть `untrusted_skipped`/`command_untrusted` — это
     ожидаемо: в MCP-режиме правила command_succeeds не исполняются, пока
     доверие не подтверждено (`arch-be rules allow` в этом репозитории либо
     ARCH_NO_EXEC=0 в окружении сервера). Скажи об этом человеку и не
     выдавай SKIP за PASS.
   - Вызови skill_search с {"query": "saga"} и перечисли 3 найденных скилла.
   - Вызови model_query с {"dir": "model"} (если каталога model/ нет —
     скажи об этом, это не ошибка).

5. Финал: доложи одной сводкой — платформа и выбранный файл релиза, версия
   arch-be, режим записи MCP (read-only / rw=reports / rw), статус MCP
   (Connected), число разложенных скиллов, статус хука, вердикт fitness_check.
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

Помимо инструментов MCP, `connect gigacode` разложил в
`.gigacode/skills/` (или `.qwen/skills/` — см. заметку про пути выше)
**десять плейбуков** — скиллов, которые учат самого агента правильно водить
Spine. MCP даёт инструменты; плейбук даёт МЕТОД: в каком порядке вызывать,
на что смотреть в ответах, какие оговорки делать, где границы (read-only,
never-список).

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
| `spine-semantic-judge` | Смысловая рубрика по коду: субъекты → досье → k ответов → verify → гейт |
| `spine-judge-handover` | Передача судейства второму судье и приёмка отчёта |
| `spine-contracts-gate` | Перед релизом API: линт OpenAPI/AsyncAPI + diff версий |
| `spine-fitness-gate` | Работа с кодом под гейтом: чтение находок, починка, перепроверка |
| `spine-bundle` | Сборка Evidence Bundle под маршрут изменения |
| `spine-archify-viz` | «Нарисуй архитектуру»: JSON IR → 9 проверок → интерактивный HTML |

Их же можно читать как документацию: это обычные файлы
`.gigacode/skills/spine-*/SKILL.md`.

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

1. Движок вендорен в репо Spine: `vendor/archify/` (нужен Node.js ≥ 18;
   в офлайн-бандле он уже внутри архива).
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

## Нюансы релиза 0.3.7–0.3.10, важные при работе из GigaCode

- **`command_succeeds` под MCP не исполняется по умолчанию** (0.3.7,
  ADR-053). `fitness_check` вернёт `untrusted_skipped`, а гейт — `INCOMPLETE`
  (exit 3). Это не поломка и не PASS: подтвердите доверие в репозитории
  (`arch-be rules allow` — отпечаток набора команд в
  `~/.arch-harness/trusted.json`; правка реестра требует повторного доверия)
  либо задайте `ARCH_NO_EXEC=0` в окружении MCP-сервера. CLI по умолчанию
  работает как прежде (`--no-exec` — выключить).
- **Пропуск вместо провала** (0.3.7): правило с недоступным раннером
  (`pytest`, `mvn`) даёт SKIP с подсказкой, а не `✗`; в `gate` пропуск
  error-правила — `INCOMPLETE`. `doctor` показывает наличие
  `python3`/`pytest`/`java`/`mvn` (WARN, никогда FAIL).
- **Отчёт судьи не сохранится в read-only** (0.3.6–0.3.10): при обычном
  `mcp serve` ответ `rubric_verify` несёт `artifact_saved: false`. Для
  аудиторского следа подключайте сервер как `--rw=reports` — тогда оба
  пути (CLI и MCP) кладут архивную пару в `~/.arch-harness/reports/`, а в
  ответе появляется поле `archive`.
- **Допуск судьи к гейту — политика проекта** (0.3.9): при
  `require_qualified_judge = true` судья без пройденной квалификации даёт
  `judge_unqualified` и вердикт `INCOMPLETE`. По умолчанию флаг выключен;
  квалификация — `arch-be rubric qualify <рубрика> --set <набор>`.
- **`doctor` проверяет cron** (0.3.10): называет задачи `cron.toml`, файлов
  которых нет, а не ограничивается наличием файла расписания.

## Если что-то пошло не так

| Симптом | Лечение |
|---|---|
| Скачали не тот файл релиза (чужой `os`/`arch`) | Сверьтесь с таблицей Части 0; `arch-be --version` на неверном бинаре падает или не запускается. Либо отдайте выбор агенту (шаг 0 промпта) |
| `releases/latest/download/...` отдаёт 404 | Проверьте имя файла из таблицы Части 0; для платформ без ассета — офлайн-бандл или сборка из клона |
| SHA256 не сходится | Не запускайте бинарь: перекачайте файл и `SHA256SUMS` из одного и того же релиза, сверьте ещё раз |
| Windows: `arch-be` не находится | Положите `arch-be.exe` в каталог из `PATH` и вызывайте `arch-be.exe` (или добавьте каталог в `PATH`) |
| `spine` в статусе Pending approval | `qwen mcp approve spine` (или интерактивный запуск и одобрение) |
| Агент «не видит» инструменты | `qwen mcp list` → должен быть `Connected`; проверьте `command -v arch-be` |
| Сервер одобрен, но в текущей сессии инструменты не привязались | `approve` действует со СЛЕДУЮЩЕЙ сессии — перезапустите GigaCode (в headless — это просто следующий вызов) |
| Гейт: `INCOMPLETE`, находки `command_untrusted` | MCP-режим не исполняет `command_succeeds`: `arch-be rules allow` в репозитории или `ARCH_NO_EXEC=0` в окружении сервера |
| `rubric_report_missing`, хотя оценка прошла | Хост подключён read-only: переподключите с `--rw=reports` (или заберите `artifact_json` из ответа `rubric_verify`) |
| 404 по модели | Для openai-совместимого бэкенда модель задаётся через `OPENAI_MODEL`, не флагом `-m` |
| Хук не срабатывает | Версия GigaCode может не иметь hooks — проверьте `qwen hooks` / документацию своей сборки |
