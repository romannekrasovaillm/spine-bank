# MCP-интеграции

Два режима: **клиент** (`src/mcp.rs`) — подключение внешних MCP-серверов,
их инструменты доступны агенту и из CLI; **сервер** (`src/mcp_server.rs`,
`arch-be mcp serve`) — архитектурный контроль Spine наружу кодовым агентам
(Claude Code и др.) в момент написания кода (ADR-008). Формат конфигурации
клиента — как у Claude Code.

## MCP-сервер: `arch-be mcp serve`

Stdio-сервер JSON-RPC 2.0 (NDJSON, как у клиента): кодовый агент получает
architectural verdict (`passed` + находки) **в момент написания кода**, а не
на приёмке handoff-пакета. Состав read-only режима: **40 инструментов** —
16 ручных (контроль, знания, split-judge, `rules_suggest`) + 24 мостовых из
реестра — и 10 промптов-плейбуков `spine-*` (capability `prompts`).
По умолчанию сервер строго read-only: ничего не
пишет в репозиторий клиента, все цели — аргументами вызова; write/exec-
инструменты агента (`bash`, `write_file`, `harness_run`, `subagent_*`,
`web_*`, …) наружу не отдаются **ни в одном режиме** — это принадлежность
хоста. Флаг `--rw` дополнительно открывает белый список аддитивных записей
в рабочий каталог клиента: `handoff_create`, `adr_new`, `agentsmd_generate`,
`skill_distill`, `archify_deliver`/`archify_show`/`archify_compare`,
`reverse_survey`, `evidence_pack`, `delta_propose` (полный состав —
в разделе «Инструменты» ниже).

Поверх контрольного контура (ADR-008) сервер отдаёт **чтение знаний**
(транш T4, ADR-015): поиск по базе знаний архитектора и библиотеке скиллов
arch-be + рендер mermaid-диаграмм — это read-only-инструменты поверх локальных
каталогов конфига arch-be (`knowledge.dirs`, `plugins.dirs`), не репозитория
клиента. Отдельная capability — **промпты** (`prompts/list`, `prompts/get`):
десять плейбуков spine-workflows как слэш-команды хоста (см. ниже); \
передача судейства и приёмка — `spine-judge-handover`.

Помимо ручных инструментов работает **мост в реестр** (`tools::full_registry`):
белый список read-only доменных инструментов (20 штук: верификаторы
`nfr_check`/`model_validate`/`delta_guard`/`evidence_verify`, отчёты
`landscape_report`/`adr_registry`/`rules_report`/`openspec_coverage`/`model_graph`,
составные `architect_review`/`change_impact`, `model_drift`, линтеры контрактов
`openapi_lint`/`asyncapi_lint`/`contract_diff` и реестровые
`fleet_audit`/`agentsmd_lint`/`archify_validate`/`rubric_list`/`plugin_list`)
маршрутизируется в `ToolRegistry::dispatch`
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
процесса сервера, который задаёт клиент. Инструменты модели принимают и корень
кейса, и каталог `model/` — модель находится сама (T-13).

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
| `qwen` | `.qwen/settings.json` (мердж `mcpServers`); скиллы в `.qwen/skills/` (project-scope qwen-code ≥ 0.24; существующий каталог не перетирается) | хуки — сниппет-референс (схема файла хуков не подтверждена) |
| `gigacode` | автоопределение каталога настроек: `.gigacode/` → `.qwen/` (форк Qwen Code) → новый `.gigacode/`; в него — `settings.json` (мердж `mcpServers`) и скиллы в `skills/` (как у claude) | хуки — сниппет-референс; следующие шаги (в т.ч. `arch-be doctor --host gigacode`) |
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
| `fitness_check` | `repo`, `constraints?` | прогон CONSTRAINTS.yaml (единый резолвер: явный `constraints` → `<repo>/.arch-handoff/CONSTRAINTS.yaml` → `<repo>/CONSTRAINTS.yaml`): must_contain / must_not_contain / each_file_must_contain / file_exists / dir_must_have_file / max_age / command_succeeds; `passed=false` — правила нарушены. В вердикте также `constraints` (использованный путь) и `drift_note` (пометка «копии реестра различаются…», если обе копии есть и расходятся); правила с неизвестными типами — warn-находки `unknown_rule_type` в `issues` + суффикс сводки (аддитивное поле `skipped_unknown` — в CLI `--json` и у `rules_report`) |
| `significance_score` | `triggers` | маршрут значимости Fast/Standard/Critical по 15 триггерам (информационный, без `passed`); `triggers` — карта «триггер → bool» ЛИБО массив строк `"name=true"` / `"name=false"` / голое `"name"` (= true) |
| `significance_from_diff` | `path?`, `base_ref?`, `declared?` | anti-bypass floor (S-1, ADR-034): триггеры выводятся из git-диффа `path` (без `base_ref` — рабочее дерево против `HEAD`, включая untracked; с `base_ref` — `git diff BASE_REF...HEAD`) и **объединяются** с заявленными `declared` (детектор только добавляет). Ответ: `route`+`score`, `sources` каждого триггера (`declared`/`diff`/`declared+diff`), `undeclared` — найденные диффом, но не заявленные триггеры с файлами-основаниями (`evidence`). Информационный, без `passed`; пороги — из `[significance]` конфига сервера |
| `trace_check` | `case` | позвенная трассируемость `REQ → NFR → AD/ADR → CMP → правило`: AD без правила и без `unverifiable` — error; verdict + `report_markdown` для evidence bundle; толерантная загрузка модели (E3): битые сущности пропускаются, в ответе `load_issues` |
| `model_query` | `dir?`, `id?`, `type?` | список сущностей модели (фильтр по типу) или карточка сущности со связями и обратными ссылками; толерантная загрузка (E3): работа по валидному подмножеству, в ответе `load_issues` (битые файлы с причинами) |
| `rubric_run` | `rubric`, `target` \| `target_text`, `model?`, `cwd?` | оценка документа рубрикой LLM-судьёй (ADR-004; нужен API-ключ из конфига arch-be; для моделей `kind = "cli"` ключ не нужен — судья — внешний CLI-харнесс); относительный `target` резолвится от `cwd` (по умолчанию — cwd процесса сервера) |
| `rubric_prompt` | `rubric`, `target` \| `target_text` (или `pack`+`subject`+`root?`) | split-judge, фаза 1 (без ключа): system+user промпты судьи + JSON-схема ответа + `judge_config` (число сэмплов k). Промпт исполняет модель хоста, ответы идут в `rubric_verify`. Вместо документа можно оценивать **досье** (ADR-051): `pack` — вид (`adr_vs_spine`, `entity_links`, `nfr_mechanism`, `code_vs_spine`, `solution_vs_standards`, `code_vs_scenarios`), `subject` — путь к ADR/файлу кода (`src/gate.rs#12-88` для фрагмента) или идентификатор сущности модели; `pack`/`subject` взаимоисключающи с `target`/`target_text`. Ответ несёт `pack` с хэшем досье и поимёнными хэшами источников |
| `rubric_verify` | `rubric`, `target` \| `target_text` (или `pack`+`subject`+`root?`), `answers`, `model?`, `judge_model?` | split-judge, фаза 2: отчёт рубрики из сырых ответов хоста (медиана, `unstable`, `evidence_not_found`) тем же кодом, что у `rubric_run`; битые ответы отбрасываются со счётчиком `answers.dropped`; `judge_model` — метка фактического судьи (перекрывает `model`, anti-bias «автор = судья»: эхо в поле `judge_model` и строке «Судья: …» markdown-отчёта). Под `--rw` отчёт по досье ложится в `reports/rubric/` с хэшем досье (`pack_sha256`) и списком источников — привязка ко **всем** источникам, а не только к субъекту (ADR-051) |
| `rules_suggest` | `path`, `cwd?` | кандидатные fitness-правила из содержательных пробелов кейса (read-only эвристики): EARS-критерии приёмки, численные таймауты в контрактах, декомпозиция REQ→работы, RTO/RPO без ADR, аудит операторских действий. Ответ: `candidates` (`id`, `rationale`, `source_skill`, `yaml` — готовый фрагмент CONSTRAINTS.yaml или `null` для честного advisory) + `report_markdown`; CLI-эквивалент: `arch-be control rules-suggest <path>`. Информационный, без `passed` |

Мостовые read-only инструменты реестра (спеки — из `Tool::spec()`, плюс
опциональный `cwd`): `openapi_lint` (`path`), `asyncapi_lint` (`path`),
`contract_diff` (`old`, `new`, опц. `format`, `model` — форматы
OpenAPI/proto/Avro/JSON Schema/DDL и связка с моделью по `INT.contract`,
ADR-035), `fleet_audit`, `agentsmd_lint` (`repo`),
`archify_validate` (`type`, `path`), `rubric_list`, `plugin_list`,
`nfr_check` (`path`, `kind`), `model_validate`/`model_drift` (`path` — корень
кейса или каталог `model/`),
`delta_guard` (`path`, `base`, `protect`), `evidence_verify` (`change_dir`),
`architect_review` (`path`, `base`), `change_impact` (`path`, `id` | `paths`).

Под `--rw` добавляются: `handoff_create`, `adr_new`, `agentsmd_generate`,
`skill_distill`, `archify_deliver`, `archify_show`, `archify_compare`,
`reverse_survey`, `evidence_pack` (`change_dir`, `route`),
`delta_propose` (`name`, `path`) (у mutating-инструментов `readOnlyHint:
false`; `evidence_pack`/`delta_propose` политика R-уровней классифицирует
как `Mutating` — авто с R2, на R0/R1 вызов отклоняется с пояснением).

Верификаторы транша 1 (`nfr_check`, `model_validate`, `delta_guard`,
`evidence_verify`) и `model_drift` возвращают в `content[0].text` (и в
`structuredContent.output` моста) JSON-вердикт `{passed, issues, summary}` —
тот же контракт, что у ручных контрольных инструментов: `passed: false` —
основание отказать изменению, перечислив находки. У модельных инструментов
(`nfr_check`, `model_validate`, `model_drift`) вердикт несёт аддитивное поле
`load_issues` (E3): сущности `model/`, пропущенные из-за ошибок разбора
(у `model_validate` они же — error-находки `load-error`).

Мостовой ответ несёт **разобранный** объект (T-12): поля вердикта
(`passed`, `summary`, `findings`, …) лежат в `structuredContent` рядом с
`tool` и строковым `output` — читать JSON из строки не нужно. Строка `output`
сохранена для клиентов, написанных до этой правки; у инструментов, чей текст —
человеко-читаемый отчёт (`contract_diff`), разобранный вердикт приходит из
`data` инструмента, у остальных — разбором JSON-текста.

Отчёты транша 2 (read-only; JSON со счётчиками в том же контуре моста):

| Инструмент | Аргументы | Возвращает |
|---|---|---|
| `landscape_report` | `path`, `format?` (`markdown`\|`mermaid`) | ландшафт систем набора проектов (EA-3, ADR-037): счётчики `systems`/`edges`/`findings` + `report` (markdown-отчёт или mermaid `graph TD`) + `load_issues` (битые сущности, E3). Отчёт, не гейт — `passed` не применим |
| `adr_registry` | `path`, `strict?` | реестр ADR по набору проектов (ADR-036): `entries`, `findings` (коллизия номеров — в т.ч. **внутри одного проекта**: ≥2 файлов с одним ADR-NNN, с нюансом «разные заголовки — конфликт решений»/«тот же заголовок — вероятная копия»; дубли заголовков у разных номеров; пропуски полей), `report_markdown`; `passed=false` только при `strict: true` и наличии находок |
| `rules_report` | `path` (репозиторий), `constraints?` | инвентарь правил CONSTRAINTS.yaml (единый резолвер пути, как у `fitness_check`): `rules_total`, `by_kind`, `by_severity`, `constraints` (использованный путь), `drift_note` (дрейф двух копий), `skipped_unknown` (неизвестные типы правил), `report_markdown` (карточки owner/expiry/effort_hours, находки, git-прокси). Отчёт — `passed` всегда true |
| `openspec_coverage` | `path`, `constraints?`, `strict?` | покрытие требований OpenSpec правилами (`covers:`): `total`/`covered`/`unverifiable`/`unresolved` + `unresolved_items` поимённо + `constraints` (использованный путь реестра) + `drift_note` (пометка дрейфа второй копии, если расходится) + `report_markdown`; `passed=false` только при `strict: true` и требованиях «без решения» |
| `model_graph` | `dir?`, `format?` (`text`\|`mermaid`) | граф связей модели: `entities`/`edges` + `graph` (список или flowchart LR, совместим с `mermaid_render`) + `load_issues` (битые сущности, E3). Отчёт, не гейт — `passed` не применим |

Составные инструменты (транш 3, read-only):

| Инструмент | Аргументы | Возвращает |
|---|---|---|
| `architect_review` | `path?`, `base?` | единое ревью репозитория одним вызовом: маршрут значимости из git-диффа + весь контур гейта (fitness, delta_guard, rule_weakened, spine_lint, trace_check; на Standard/Critical — nfr, evidence, sensors) + секции `model_validate` и `contracts` (линт OpenAPI/AsyncAPI из `INT.contract` и `contracts/`). JSON: `passed` + `route` + `components` (status/detail/findings) + `summary`; `passed=false` — основание отказать изменению |
| `change_impact` | `path?`, `id` \| `paths` | радиус изменения по графу модели: `seeds`, `affected` (сущности по типам), `rules` (C-NNN с владельцами), `contracts`, `owners` (достигнутые `OWNER-*` и владельцы карточек затронутых правил; реестр ищется и в `.arch-handoff/`), `owners_note` (почему список пуст), `gaps` (пути без CMP-покрытия), `summary` + `load_issues` (битые сущности, E3). Отчёт, не гейт — `passed` не применим; неизвестный `id` — `isError` |

Чтение знаний (транш T4, ADR-015; все — read-only, без verdict `passed`):

| Инструмент | Аргументы | Возвращает |
|---|---|---|
| `kb_search` | `query`, `limit?` | хиты по базе знаний архитектора (`knowledge.dirs` конфига arch-be): `path`, `line`, `score`, `snippet` с контекстом (максимум 20, по умолчанию 10) |
| `skill_search` | `query`, `limit?` | скиллы из библиотеки плагинов arch-be (`plugins.dirs`) и каталогов подключённых харнессов (`.claude/skills` и др., куда их кладёт `connect`): `name`, `plugin`, `score`, `description`, `snippet` (максимум 20, по умолчанию 8) |
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

### Промпты: плейбуки `spine-*` как слэш-команды хоста

Помимо инструментов сервер отдаёт capability **`prompts`**: десять плейбуков
встроенного плагина spine-workflows доступны хосту как готовые команды —
не нужно помнить формулу «действуй по скиллу …», и не важно, куда конкретный
хост кладёт файлы скиллов: сценарий приезжает прямо от сервера.

- `prompts/list` — список промптов (`name`, `description` из frontmatter
  SKILL.md, объявленные `arguments`). Пагинации нет: список фиксирован.
- `prompts/get` (`name`, опц. `arguments`) → `description` + `messages`:
  одно user-сообщение с инструкцией «действуй по этому плейбуку», полным
  текстом SKILL.md и эхом переданных аргументов (`- имя = "значение"`;
  эхом подставляются только объявленные аргументы, значения — строки).
  Неизвестное имя → `-32602`.

Текст плейбука — пользовательская копия из `plugins.dirs`, если она есть
(правки в силе, как у `skill_load`), иначе встроенный в бинарь ассет:
работает из коробки, без `arch-be init`. Как это выглядит у хостов:
**Claude Code** отдаёт MCP-промпты как слэш-команды (`/mcp__spine__<имя>`
или выбор в меню `/`); **qwen-code 0.24** поддерживает prompts (layout
каталога скиллов для `connect qwen` по-прежнему помечен неподтверждённым —
для промптов он и не нужен).

| Промпт | Аргументы (все опциональны) | Сценарий |
|---|---|---|
| `spine-quickstart` | — | проверка подключения MCP и границ сервера |
| `spine-content-bootstrap` | — | наполнение пустого проекта: спайн, CONSTRAINTS.yaml, model/, knowledge/, первый ADR |
| `spine-architect-review` | — | архитектурный разбор: маршрут значимости, модель, трассировка, инвентаризация |
| `spine-adr-judge` | `target`, `rubric` | split-judge оценка документа рубрикой без API-ключей у Spine |
| `spine-contracts-gate` | `path`, `old`, `new` | контрактный гейт: линт OpenAPI/AsyncAPI + diff версий |
| `spine-archify-viz` | `subject` | визуализация Archify: JSON IR → валидация → интерактивный HTML |
| `spine-fitness-gate` | — | работа под fitness-гейтом: чтение находок, починка, перепроверка |

`resources/*` сервер не отдаёт (`-32601`): чтение знаний идёт инструментами
`kb_search`/`skill_search`/`skill_load`, сценарии — промптами.

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

`unknown_triggers` всегда пуст: незнакомое имя в `declared` (или в триггерах
`significance_score`) — ошибка вызова `-32602` с перечнем 15 канонических
триггеров и ближайшим совпадением для каждого лишнего (T-04: раньше выдуманное
имя попадало и в `unknown_triggers`, и в счёт — пять несуществующих триггеров
давали Critical).

### Журнал вызовов (`mcp-calls.jsonl`)

Каждый вызов `tools/call` (и ручной, и мостовой) журналируется в проектный
append-only журнал `<cwd сервера>/.arch-handoff/mcp-calls.jsonl` — рядом с
`CONSTRAINTS.yaml` проекта, а не в доме пользователя. Журнал — источник
outcome-данных для `arch-be digest` (протокол `docs/outcome-metrics.md`
заполняется из него, а не вручную).

Строка = один вызов, JSON:

```json
{"ts":"2026-09-18T12:00:00+03:00","tool":"fitness_check","verdict":"fail","duration_ms":42,"rules":["no-pan"]}
```

- `verdict`: `pass`/`fail` — по `passed` структурированного verdict'а (для
  мостовых верификаторов — по JSON `{passed,…}` в текстовом выводе);
  `ok` — успех без `passed`; `error` — доменный сбой (`isError: true`);
  `invalid` — отказ уровня параметров (`-32602`: неизвестный инструмент,
  битые аргументы).
- `rules` — имена правил из error-находок verdict'а (до 10, только у
  `fail`): топ нарушаемых правил в дайджесте. **Содержимое аргументов не
  журналируется.**
- Запись fail-soft: сбой журнала (каталог не создать, ФС только на чтение)
  не ломает вызов инструмента.
- Ротация по размеру: при 5 МиБ файл переименовывается в
  `mcp-calls.1.jsonl` (прежний `.1` затирается) — одно поколение истории.

### Ошибки

- Битая JSON-строка → `-32700`; неизвестный метод → `-32601`; битые
  аргументы / неизвестный инструмент / неизвестный промпт → `-32602`.
  Сервер не падает ни на каком вводе, цикл живёт до EOF stdin.
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
