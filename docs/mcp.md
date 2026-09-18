# MCP-интеграции

Два режима: **клиент** (`src/mcp.rs`) — подключение внешних MCP-серверов,
их инструменты доступны агенту и из CLI; **сервер** (`src/mcp_server.rs`,
`arch-be mcp serve`) — архитектурный контроль Spine наружу кодовым агентам
(Claude Code и др.) в момент написания кода (ADR-008). Формат конфигурации
клиента — как у Claude Code.

## MCP-сервер: `arch-be mcp serve`

Stdio-сервер JSON-RPC 2.0 (NDJSON, как у клиента): кодовый агент получает
architectural verdict (`passed` + находки) **в момент написания кода**, а не
на приёмке handoff-пакета. По умолчанию сервер строго read-only: ничего не
пишет в репозиторий клиента, все цели — аргументами вызова; write/exec-
инструменты агента (`bash`, `write_file`, `harness_run`, `subagent_*`,
`web_*`, …) наружу не отдаются **ни в одном режиме** — это принадлежность
хоста. Флаг `--rw` дополнительно открывает белый список аддитивных записей
в рабочий каталог клиента: `handoff_create`, `adr_new`, `agentsmd_generate`,
`skill_distill`, `archify_deliver`/`archify_show`/`archify_compare`,
`reverse_survey`.

Поверх контрольного контура (ADR-008) сервер отдаёт **чтение знаний**
(транш T4, ADR-015): поиск по базе знаний архитектора и библиотеке скиллов
arch-be + рендер mermaid-диаграмм — это read-only-инструменты поверх локальных
каталогов конфига arch-be (`knowledge.dirs`, `plugins.dirs`), не репозитория
клиента.

Помимо ручных инструментов работает **мост в реестр** (`tools::full_registry`):
белый список read-only доменных инструментов (`openapi_lint`, `asyncapi_lint`,
`contract_diff`, `fleet_audit`, `agentsmd_lint`, `archify_validate`,
`rubric_list`, `plugin_list`) маршрутизируется в `ToolRegistry::dispatch`
с политикой R-уровней из конфига. Спеки этих инструментов генерируются из
`Tool::spec()`; каждый принимает дополнительный аргумент `cwd` — рабочий
каталог клиента для резолва относительных путей (по умолчанию — cwd процесса
сервера). Решение политики `Deny`/`RequireConfirm` возвращается как
`isError` с текстом причины (подтверждение в неинтерактивном MCP невозможно —
`RequireConfirm` трактуется как отказ).

### Подключение (Claude Code)

```bash
claude mcp add arch-spine -- arch-be mcp serve
```

либо вручную в `~/.claude.json` / `.mcp.json` проекта (тот же формат, что
и у нашего клиента):

```json
{
  "mcpServers": {
    "arch-spine": {
      "command": "arch-be",
      "args": ["mcp", "serve"]
    }
  }
}
```

Запуск из каталога целевого проекта: относительные пути аргументов
(`model`, `model/` для `model_query` по умолчанию) резолвятся от cwd
процесса сервера, который задаёт клиент.

### Подключение одной командой: `arch-be connect <host>`

`arch-be connect` раскладывает всё, что нужно хосту: конфиг MCP-сервера
(ключ `spine` → `arch-be mcp serve`, `--rw` добавляет флаг), пакет скиллов
из встроенных плагинов и хуки жизненного цикла. Повторный запуск
идемпотентен: JSON мержится ключ-в-ключ (чужие серверы и хуки
сохраняются — об этом заметка в выводе), наши хуки помечены маркером
`# spine-connect:*` и не дублируются; `--dry-run` печатает план без записи.
Флаги: `--dir <путь>` (дефолт — текущий каталог), `--no-skills`,
`--no-hooks`, `--no-agents-md`, `--strict-hooks`, `--apply-global`.

| Хост | Что пишется | Что печатается |
|---|---|---|
| `claude` | `.mcp.json` (мердж `mcpServers.spine`), `.claude/settings.json` (мердж хуков), `.claude/skills/<имя>/`, `CLAUDE.md` (блок между `<!-- SPINE:BEGIN/END -->`) | следующие шаги (`claude mcp list`) |
| `qwen` | `.qwen/settings.json` (мердж `mcpServers`) | скиллы и хуки — сниппеты (layout Qwen Code не подтверждён) |
| `codex` | только с `--apply-global`: `~/.codex/config.toml` (мердж `[mcp_servers.spine]`, бэкап `*.bak-spine-connect`) | TOML-блок; рекомендация `arch-be agents-md refresh .` |
| `kimi` | `.kimi-code/mcp.json` проекта (мердж `mcpServers.spine`, чужие серверы сохраняются; project-уровень перекрывает user-level); с `--apply-global` — также `~/.kimi-code/mcp.json` (мердж, бэкап) | JSON-блок для user-level (альтернатива), TOML-блок хука `[[hooks]]` для `~/.kimi-code/config.toml` (проектных хуков у Kimi Code нет), напоминание про trust-диалог (project MCP не стартует в untrusted-папке) |
| `omp` | `.mcp.json` (мердж, как у claude — omp дискаверит проектный файл автоматически); скиллы в `.claude/skills/`, только если такого каталога ещё нет (omp читает его нативно) | заметки: автодискавери `.mcp.json`; хуков нет — TS-расширения через `omp --hook <file.ts>` |
| `generic` | ничего | все сниппеты для ручной установки |

Хуки Claude Code: `Stop` → `arch-be gate --route auto` (гард: только если
`arch-be` в PATH и есть `.arch-handoff/CONSTRAINTS.yaml`; единый гейт —
состав составляющих и SKIP-семантика в `docs/control.md`). Дефолт —
fail-soft на инфраструктуру (нет бинаря/правил, нет входа у составляющих —
молча exit 0) и fail-hard на вердикт (ненулевой код возврата гейта →
exit 2, stderr уходит агенту; строки вывода хук не разбирает). `PostToolUse`
(matcher `Edit|Write|MultiEdit`) — только под `--strict-hooks`: на
репозиториях с правилами `command_succeeds` гейт может гонять сборки — для
каждой правки это дорого.

### Инструменты

| Инструмент | Аргументы | Verdict |
|---|---|---|
| `spine_lint` | `path` | линтер ARCHITECTURE-SPINE.md: `passed=false` при находках error (дубли AD-id, пустые Binds/Prevents/Rule, заглушки, непиннутые версии, битые ссылки AD) |
| `fitness_check` | `repo`, `constraints?` | прогон CONSTRAINTS.yaml (дефолт `<repo>/.arch-handoff/CONSTRAINTS.yaml`): must_contain / must_not_contain / each_file_must_contain / file_exists / dir_must_have_file / max_age / command_succeeds; `passed=false` — правила нарушены |
| `significance_score` | `triggers` | маршрут значимости Fast/Standard/Critical по 15 триггерам (информационный, без `passed`) |
| `significance_from_diff` | `path?`, `base_ref?`, `declared?` | anti-bypass floor (S-1, ADR-034): триггеры выводятся из git-диффа `path` (без `base_ref` — рабочее дерево против `HEAD`, включая untracked; с `base_ref` — `git diff BASE_REF...HEAD`) и **объединяются** с заявленными `declared` (детектор только добавляет). Ответ: `route`+`score`, `sources` каждого триггера (`declared`/`diff`/`declared+diff`), `undeclared` — найденные диффом, но не заявленные триггеры с файлами-основаниями (`evidence`). Информационный, без `passed`; пороги — из `[significance]` конфига сервера |
| `trace_check` | `case` | позвенная трассируемость `REQ → NFR → AD/ADR → CMP → правило`: AD без правила и без `unverifiable` — error; verdict + `report_markdown` для evidence bundle |
| `model_query` | `dir?`, `id?`, `type?` | список сущностей модели (фильтр по типу) или карточка сущности со связями и обратными ссылками |
| `rubric_run` | `rubric`, `target` \| `target_text`, `model?` | оценка документа рубрикой LLM-судьёй (ADR-004; нужен API-ключ из конфига arch-be; для моделей `kind = "cli"` ключ не нужен — судья — внешний CLI-харнесс) |
| `rubric_prompt` | `rubric`, `target` \| `target_text` | split-judge, фаза 1 (без ключа): system+user промпты судьи + JSON-схема ответа + `judge_config` (число сэмплов k). Промпт исполняет модель хоста, ответы идут в `rubric_verify` |
| `rubric_verify` | `rubric`, `target` \| `target_text`, `answers`, `model?` | split-judge, фаза 2: отчёт рубрики из сырых ответов хоста (медиана, `unstable`, `evidence_not_found`) тем же кодом, что у `rubric_run`; битые ответы отбрасываются со счётчиком `answers.dropped` |

Мостовые read-only инструменты реестра (спеки — из `Tool::spec()`, плюс
опциональный `cwd`): `openapi_lint` (`path`), `asyncapi_lint` (`path`),
`contract_diff`, `fleet_audit`, `agentsmd_lint` (`repo`),
`archify_validate` (`type`, `path`), `rubric_list`, `plugin_list`,
`nfr_check` (`path`, `kind`), `model_validate` (`dir`),
`delta_guard` (`path`, `base`, `protect`), `evidence_verify` (`change_dir`).
Под `--rw` добавляются: `handoff_create`, `adr_new`, `agentsmd_generate`,
`skill_distill`, `archify_deliver`, `archify_show`, `archify_compare`,
`reverse_survey`, `evidence_pack` (`change_dir`, `route`),
`delta_propose` (`name`, `path`) (у mutating-инструментов `readOnlyHint:
false`; `evidence_pack`/`delta_propose` политика R-уровней классифицирует
как `Mutating` — авто с R2, на R0/R1 вызов отклоняется с пояснением).

Верификаторы транша 1 (`nfr_check`, `model_validate`, `delta_guard`,
`evidence_verify`) возвращают в `content[0].text` (и в
`structuredContent.output` моста) JSON-вердикт `{passed, issues, summary}` —
тот же контракт, что у ручных контрольных инструментов: `passed: false` —
основание отказать изменению, перечислив находки.

Чтение знаний (транш T4, ADR-015; все — read-only, без verdict `passed`):

| Инструмент | Аргументы | Возвращает |
|---|---|---|
| `kb_search` | `query`, `limit?` | хиты по базе знаний архитектора (`knowledge.dirs` конфига arch-be): `path`, `line`, `score`, `snippet` с контекстом (максимум 20, по умолчанию 10) |
| `skill_search` | `query`, `limit?` | скиллы из библиотеки плагинов arch-be (`plugins.dirs`): `name`, `plugin`, `score`, `description`, `snippet` (максимум 20, по умолчанию 8) |
| `skill_load` | `name` | полный текст скилла по точному имени (после `skill_search`); неизвестное имя — `isError` |
| `mermaid_render` | `code` \| `path` | mermaid-диаграмма (flowchart, sequenceDiagram, erDiagram, C4) в ASCII-арт; `path` — `.mmd`-файл относительно cwd сервера |

Контракт ответа контрольных инструментов (`structuredContent` и тот же
объект pretty-JSON в `content[0].text`):

```json
{
  "passed": false,
  "issue_count": 2,
  "error_count": 2,
  "warn_count": 0,
  "issues": [{"severity": "error", "rule": "spine_present", "file": "…", "line": 0, "message": "…"}],
  "summary": "Правил: 1, нарушений: 2 (error: 2, warn: 0)"
}
```

У находок `fitness_check` могут быть аддитивные карточные поля `ad`, `adr`,
`rationale`, `owner`, `fix_hint`, `skill` — архитектурный контекст из карточки
правила `CONSTRAINTS.yaml` (какой инвариант задет, как чинить, какой скилл
загрузить через `skill_load`); присутствуют, только если заполнены в карточке
(см. `docs/control.md` — формат находки).

`passed: false` — основание **отказать изменению**, нарушающему `AD-*`,
перечислив находки (эта инструкция отдаётся клиенту и в `initialize.instructions`).

Контракт ответа `significance_from_diff` (информационный, `passed` не
применим):

```json
{
  "route": "Standard",
  "score": 2,
  "fired": ["new_component", "new_vendor"],
  "sources": {"new_component": "declared+diff", "new_vendor": "diff"},
  "undeclared": [
    {"trigger": "new_vendor", "evidence": ["зависимость в services/risk/Cargo.toml: serde = \"1.0\""]}
  ],
  "unknown_triggers": [],
  "summary": "Score: 2 (new_component (declared+diff), new_vendor (diff)) → маршрут Standard; ВНИМАНИЕ — не заявлены, но видны по диффу: new_vendor"
}
```

`undeclared` — anti-bypass сигнал «заявлено агентом vs видно по диффу»:
на маршрут влияет через объединённое множество (детектор только добавляет),
блокирующего verdict нет — решение остаётся за гейтом маршрута.

### Ошибки

- Битая JSON-строка → `-32700`; неизвестный метод → `-32601`; битые
  аргументы / неизвестный инструмент → `-32602`. Сервер не падает ни на
  каком вводе, цикл живёт до EOF stdin.
- Доменный сбой выполнения (файл не читается, сущность не найдена) —
  `result` с `isError: true` и текстом причины (не protocol error).
- `rubric_run` без API-ключа провайдера — понятная JSON-RPC ошибка
  `-32603` (какой env/файл настроить; содержимое ключа не читается).
  Исключение — модели `kind = "cli"`: предпроверка ключа пропускается,
  модель вызывается через авторизованный на машине CLI-харнесс.

### Проверка без клиента

```bash
printf '%s\n' \
  '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"0"}}}' \
  '{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}' \
  '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"fitness_check","arguments":{"repo":"/path/to/repo"}}}' \
  | arch-be mcp serve
```

## MCP-клиент: подключение внешних серверов

## Файл серверов `mcp.json`

Путь — `mcp.servers_file` (дефолт `~/.arch-harness/mcp.json`; `arch-be init`
кладёт образец). Формат:

```json
{
  "mcpServers": {
    "filesystem": {
      "command": "npx",
      "args": ["-y", "@modelcontextprotocol/server-filesystem", "~/Документы"]
    },
    "fetch": {
      "command": "uvx",
      "args": ["mcp-server-fetch"]
    },
    "memory": {
      "command": "npx",
      "args": ["-y", "@modelcontextprotocol/server-memory"]
    }
  }
}
```

(это `examples/mcp.example.json`: filesystem — файловое дерево, fetch —
загрузка URL, memory — граф памяти). На сервер: `command` + опциональные
`args` и `env`.

## Транспорт и протокол

- **stdio**: сервер запускается дочерним процессом (`tokio::process`),
  обмен — **NDJSON**: сообщения JSON-RPC 2.0 строками в stdin/stdout, без
  `Content-Length`-фрейминга (стиль MCP).
- Handshake: `initialize` (`protocolVersion: "2025-06-18"`, clientInfo
  `arch-harness`; при протокольной ошибке — один fallback на `"2024-11-05"`)
  + `notifications/initialized`; далее `tools/list` и `tools/call`.
- Подключения и опросы серверов — конкурентно, лимит 4
  (`CONNECT_CONCURRENCY`); сбой одного сервера не роняет остальные.
- Таймаут — `mcp.timeout_secs` (дефолт 60 с) на вызов/подключение.
- `McpManager::shutdown` — graceful: `kill` дочерних процессов.

## Именование и вызов

Инструменты всех серверов сливаются с составными именами **`server__tool`**
(например, `filesystem__read_file`, `fetch__fetch`):

```bash
arch-be mcp list                                   # серверы + инструменты
arch-be mcp call fetch__fetch '{"url": "https://example.com/spec"}'
```

В TUI: `/mcp list` — подключение по требованию и список; сбой (нет файла,
сервер не поднялся) показывается текстом, не роняя сессию.

## Ленивый режим (`connect_on_start`)

Дефолт — `connect_on_start = false` (`src/config.rs::McpSettings`):

- **старт TUI не ждёт серверы** — никаких пауз на `npx`/`uvx`-загрузки;
- `/mcp list` подключается по требованию, показывает инструменты и отключается;
- **инструменты MCP доступны модели только при `connect_on_start = true`**:
  тогда при старте TUI `McpManager` поднимается, и каждый MCP-инструмент
  регистрируется в реестре агента через `McpToolAdapter`
  (`src/mcp.rs` → `Tool`) с именем `server__tool`.

```toml
[mcp]
servers_file = "~/.arch-harness/mcp.json"
connect_on_start = true    # модель увидит MCP-инструменты; старт TUI медленнее
timeout_secs = 60
```

Компромисс: `true` — MCP в агентном цикле ценой задержки старта;
`false` — мгновенный старт, MCP только по явному `/mcp list`/`arch-be mcp call`.

## Свой MCP-сервер

Сервер — любой процесс, говорящий JSON-RPC 2.0 строками по stdio:

1. На `initialize` ответить возможностями (`tools` — если есть инструменты);
   принять `notifications/initialized`.
2. На `tools/list` вернуть список `{name, description, inputSchema}`.
3. На `tools/call` — выполнить и вернуть `content: [{type: "text", text: ...}]`
   (клиент разбирает text-части).

Спецификация: <https://modelcontextprotocol.io> (transport «stdio»).
Готовые серверы — пакеты `@modelcontextprotocol/server-*` (npx) и
`mcp-server-*` (uvx/pip). Проверка без TUI:

```bash
arch-be mcp list
arch-be mcp call myserver__mytool '{"arg": 1}'
```

Замечания:

- Файл отсутствует — ошибка с подсказкой создать `~/.arch-harness/mcp.json`
  по образцу `examples/mcp.example.json`.
- Таймауты сетевых MCP-серверов регулируйте `timeout_secs`; тяжёлые
  `npx -y`-первые-запуски могут не уложиться в 60 с — поднимите до 120–180.
- Ключи/токены серверам передавайте через `env` в `mcp.json`; сам файл
  не коммитьте наружу, если в нём секреты.
