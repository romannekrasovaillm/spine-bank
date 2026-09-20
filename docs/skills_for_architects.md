# Скиллы для архитекторов — обзор библиотеки

Библиотека скиллов — это методики архитектора, упакованные так, что агент
находит их сам (`skill_search`), подгружает в контекст (`skill_load`) и
дальше следует методике: с чек-листами, железными правилами, примерами и
антипаттернами. Механика (поиск, загрузка, layout плагина, доверие, команды)
— в [plugins_and_skills.md](plugins_and_skills.md); этот документ — про
**содержимое**: что внутри и что попробовать в первую очередь.

Всего **66 скиллов в 9 встроенных плагинах** (`assets/plugins/`, MIT) —
раскладываются командой `arch-be init` в `~/.arch-harness/plugins`.

> **Banking Edition** добавляет шесть доменных плагинов `ru-*` (39 скиллов):
> ru-integration, ru-data, ru-compliance, ru-architecture, ru-payments,
> ru-archify — проприетарный слой, в эту публикацию не входит (итого в полной
> редакции 101 скилл в 15 плагинах).

## Карта библиотеки

| Плагин | Скиллов | О чём |
|---|---:|---|
| arch-core | 15 | Ядро методов solution-архитектора: ADR, спайн, маршрутизация, NFR, handoff, рубрики |
| arch-office | 12 | Офисные артефакты: docx/pptx/xlsx с генераторами (отчёты МД, SAD, матрицы, реестр рисков) |
| aws-builders-library | 9 | Дистилляты Amazon Builders' Library: таймауты, load shedding, статическая стабильность |
| spine-workflows | 7 | Плейбуки работы со Spine из чужого харнесса поверх MCP: quickstart, разбор, гейты, судья рубрик, bootstrap, Archify |
| patterns-integration | 5 | Pattern language Ричардсона (microservices.io): сага, outbox, CQRS, strangler+ACL |
| patterns-resilience | 5 | Azure Cloud Design Patterns: circuit breaker, bulkhead, cache-aside, throttling |
| aws-agentic-ai | 5 | Агентные AI-паттерны AWS PG 2026: workflow, сага-оркестрация, reflect-refine |
| arch-governance | 3 | Управление библиотекой правил: каталог fitness-функций, антипаттерны, карта источников |
| spine-be-docs | 1 | Справка по самому продукту из документации репозитория |

## Самое интересное — с чего начать

Выборка скиллов, которые меняют то, как агент делает архитектурную работу,
а не просто добавляют фактов.

### Метод и дисциплина

- **`spine-invariants`** (arch-core) — как писать ARCHITECTURE-SPINE: тест
  принадлежности («независимые исполнители могут разойтись несовместимо?»),
  формат блока `Binds`/`Prevents`/`Rule` на живом примере платёжных
  идентификаторов, наследование инвариантов «по высоте» (корп → домен →
  продукт), антипаттерны (инвариант-пожелание, инвариант-проза).
- **`significance-routing`** (arch-core) — 15 триггеров Architecture
  Significance Score, маршруты Fast/Standard/Critical с контрольными точками
  A0–A5 и детектор «approval theater» (бездумные согласия как метрика).
- **`handoff-packaging`** (arch-core) — сборка пакета для кодового харнесса:
  компиляция epic-context 800–1500 токенов, headless JSON-контракт
  результата, контур контроля после прогона. Методика за механикой
  `handoff_create`/`harness_run`.
- **`adversarial-review`** (arch-core) — состязательное ревью независимым
  контуром: «я не проектировал эту систему — моя работа найти, что сломается».
- **`skill-authoring`** (arch-core) — мета-скилл: анатомия SKILL.md по
  стандарту agent-plugins.org, формула description («Используй этот навык
  ВСЕГДА, когда…»), тело 40–90 строк, когда выносить в references/.
- **`fitness-function-catalog`** (arch-governance) — 20 готовых
  fitness-функций для CONSTRAINTS.yaml с реальными regex (таймауты, ретраи с
  джиттером, OpenAPI/AsyncAPI-контракты, problem+json, идемпотентность), все
  шесть типов проверок движка, три волны внедрения и карточка дистилляции
  источника в правило (7 полей).

### Артефакты для людей

- **`docx-research-report`** (arch-office) — аналитический отчёт для МД в
  домовом стиле («О документе» → «Резюме» → …), с генератором
  `references/docx_research_report_gen.py` (python-docx) и чек-листом
  приёмки. Рядом в плагине — SAD, концепция для правления, интеграционная
  спецификация, план миграции волнами; pptx для правления и архкомитета;
  xlsx-каталоги, интеграционные и решающие матрицы, реестр рисков 5×5.

### Дистилляты инженерной классики

- **`timeouts-backoff-jitter`, `load-shedding`, `eight-failure-modes`,
  `static-stability`** (aws-builders-library) — Марк Брукер, Дэвид Янацек и
  др.: таймауты по перцентилям, goodput vs throughput и LIFO-очереди, 8
  независимо падающих шагов каждого сетевого вызова, работа при деградации
  зависимостей. В плагине — аннотированный каталог всех ~35 статей
  (`references/catalog.md`).
- **`transactional-outbox`, `idempotent-consumer`, `saga-transactions`**
  (patterns-integration) — канон Ричардсона: атомарная публикация событий с
  бизнес-сущностью, at-least-once без двойных эффектов, согласованность без
  2PC.
- **`circuit-breaker-retry`** (patterns-resilience) — каноническая пара
  устойчивости Azure: автомат closed/open/half-open и ретраи, которые не
  добивают деградирующую зависимость.

### Плейбуки работы со Spine из чужого харнесса (spine-workflows)

Семь плейбуков `spine-*` — это готовые сценарии для агента, который живёт в
кодовом харнессе (GigaCode, Claude Code, Kimi, Qwen, omp) и ходит в Spine
поверх MCP: у самого Spine LLM нет, поэтому выводы формулирует агент-хост,
а Spine даёт детерминированную механику.

- **`spine-quickstart`** — подключение и проверка: сервер `arch-be mcp serve`,
  как увидеть инструменты `spine__*` в своём харнессе, что проверить, когда
  «агент не видит spine».
- **`spine-architect-review`** — read-only разбор проекта: маршрут значимости
  `significance_score` (с честной оговоркой «маршрут — функция заявленных
  триггеров»), модель `model_query` с поиском сирот, трассировка
  `trace_check` по цепочке REQ → NFR → AD/ADR → CMP → правило, линты спайна
  и AGENTS.md; выдача — таблица находок с файлом/ID и предлагаемой чинкой.
- **`spine-fitness-gate`** — цикл «FAIL → починка → PASS»: прочитать каждую
  находку, понять намерение правила по имени (`no_float_for_money` = «деньги
  не в float»), чинить по смыслу минимальным дифом, обязательно перепроверить
  `fitness_check`. Отдельно: Stop-хук, блокирующий завершение, — это гейт,
  а не ошибка харнесса; «отключить хук» не предлагать.
- **`spine-contracts-gate`** — контрактный гейт: линт OpenAPI/AsyncAPI и
  `contract_diff` версий с поимкой ломающих изменений до релиза.
- **`spine-adr-judge`** — оценка ADR и дизайн-документов рубриками без
  API-ключей у Spine: split-judge (`rubric_prompt` → харнесс судит k раз →
  `rubric_verify` собирает медиану и проверяет цитаты) или `rubric_run`
  через CLI-провайдера.
- **`spine-content-bootstrap`** — наполнение пустого проекта с нуля:
  ARCHITECTURE-SPINE.md (2–4 инварианта в строгом формате), CONSTRAINTS.yaml,
  модель сущностей, база знаний, первый ADR — каждый шаг проверяется
  инструментом.
- **`spine-archify-viz`** — визуализация Archify из харнесса: агент пишет
  JSON IR из кода/модели/заметок, итеративно чинит его по диагностикам
  `archify_validate`, получает интерактивный HTML через `archify_show`.

## Полный каталог

### arch-core (15) — методы архитектора

| Скилл | Что даёт |
|---|---|
| `adr-authoring` | Дисциплина ADR по AI-DLC: запись до реализации, альтернативы, отрицательные последствия, обратимость |
| `adversarial-review` | Состязательное ревью независимым контуром |
| `agents-md-authoring` | AGENTS.md для репозиториев команд: генерация из spine/ADR/CONSTRAINTS, двухзонность |
| `c4-mermaid` | C4-уровни Context→Container→Component с рендером mermaid в терминале |
| `delta-spec` | Дельта-спецификации brownfield (OpenSpec): ADDED/MODIFIED/REMOVED относительно истины |
| `dsh-harness-patterns` | Паттерны устройства агентных харнессов по канону DeepSeek Harness |
| `fitness-functions` | Fitness-функции: CONSTRAINTS.yaml, типы проверок, как писать свои |
| `handoff-packaging` | Handoff-пакет кодовым агентам: epic-context, JSON-контракт, контроль после прогона |
| `nfr-design` | NFR с измеримыми целями, слоистое проектирование functional→NFR→infra |
| `readiness-gate` | Readiness-гейт перед реализацией (BMAD): PASS/CONCERNS/FAIL, поиск «сирот» |
| `reverse-discovery` | Обратное обследование legacy: карта из кода с метками confidence/gaps |
| `rubric-judging` | Якорные и динамические рубрики, LLM-судья со свидетельствами |
| `significance-routing` | 15 триггеров значимости, маршруты Fast/Standard/Critical, A0–A5 |
| `skill-authoring` | Мета-скилл создания скиллов и упаковки в плагины |
| `spine-invariants` | Позвоночник инвариантов для параллельных исполнителей |

Субагенты плагина: `repo-scout`, `adr-reviewer`, `nfr-auditor`. Хуки: блок
force-push, напоминание о spine/ADR после правок. MCP: `arch-memory`.

### arch-office (12) — офисные артефакты с генераторами

| Скилл | Что даёт |
|---|---|
| `docx-research-report` | Аналитический отчёт для МД (домовой стиль серии отчётов) |
| `docx-solution-design` | SAD / Architecture Definition Document (TOGAF) |
| `docx-architecture-vision` | Концепция для правления (TOGAF Phase A) |
| `docx-integration-spec` | Интеграционная спецификация — контракт стыка |
| `docx-current-state-assessment` | Аудит текущего состояния с находками-свидетельствами |
| `docx-migration-roadmap` | План миграции волнами с зависимостями |
| `pptx-board-deck` | Дек для правления: проблема → варианты → рекомендация → экономика |
| `pptx-architecture-review` | Защита решения на архкомитете |
| `xlsx-system-catalog` | Каталог/инвентарь систем |
| `xlsx-integration-matrix` | Матрица стыков с атрибутами + сводка формулами |
| `xlsx-decision-matrix` | Взвешенная матрица выбора с анализом чувствительности |
| `xlsx-risk-register` | Реестр рисков 5×5 с условным форматированием |

У каждого — генератор `references/*_gen.py` (python-docx / python-pptx /
openpyxl), корпоративные стили и чек-лист приёмки. Субагент
`report-proofreader` — корректура отчётов.

### aws-builders-library (9) — Amazon Builders' Library

| Скилл | Что даёт |
|---|---|
| `timeouts-backoff-jitter` | Таймауты по перцентилям latency, ретраи с джиттером (Брукер) |
| `avoiding-fallback` | Почему fallback хуже отказа (Габриэлсон) |
| `control-data-plane` | Разнесение плоскостей, конфигурация через файлы (Маджеррамов) |
| `load-shedding` | Goodput vs throughput, дешёвый отказ, LIFO (Янацек) |
| `static-stability` | Работа при деградации зависимостей, изоляция AZ (Вайсс, Фурр) |
| `leader-election` | Leases, грабли heartbeat и GC-пауз (Брукер) |
| `eight-failure-modes` | 8 независимо падающих шагов сетевого вызова (Габриэлсон) |
| `queue-backlogs` | Бимодальность очередей, AgeOfFirstAttempt, shuffle-sharding (Янацек) |
| `fairness-admission-control` | Квоты на тенанта, token bucket, честность в мультитенантности (Янацек) |

### spine-workflows (7) — плейбуки поверх MCP

| Скилл | Что даёт |
|---|---|
| `spine-quickstart` | Подключение Spine (MCP) к текущему харнессу и проверка, что инструменты живы |
| `spine-architect-review` | Read-only архитектурный разбор: маршрут значимости, модель, трассировка, линты |
| `spine-fitness-gate` | Работа с fitness-гейтом: чтение находок, починка по смыслу правила, перепроверка |
| `spine-contracts-gate` | Контрактный гейт: линт OpenAPI/AsyncAPI, diff версий, ломающие изменения до релиза |
| `spine-adr-judge` | Оценка ADR/документов рубриками: split-judge без API-ключей у Spine или rubric_run |
| `spine-content-bootstrap` | Наполнение пустого проекта: спайн, CONSTRAINTS.yaml, модель, база знаний, первый ADR |
| `spine-archify-viz` | Диаграммы Archify из харнесса: JSON IR → validate → интерактивный HTML |

### patterns-integration (5) — microservices.io

| Скилл | Что даёт |
|---|---|
| `saga-transactions` | Согласованность без 2PC: последовательность локальных транзакций |
| `transactional-outbox` | Атомарная запись сущности и события в одной транзакции |
| `cqrs-api-composition` | CQRS vs API Composition — чтения в микросервисах |
| `strangler-acl` | Strangler Fig + Anti-Corruption Layer для миграции с legacy |
| `idempotent-consumer` | Идемпотентный потребитель при at-least-once доставке |

### patterns-resilience (5) — Azure Patterns

| Скилл | Что даёт |
|---|---|
| `circuit-breaker-retry` | Автомат closed/open/half-open + ретраи без добивания зависимости |
| `bulkhead` | Изоляция ресурсов: пулы, семафоры, отдельные БД |
| `queue-load-leveling` | Очередь-буфер против пиков + конкурирующие потребители |
| `cache-aside` | Чтение через кэш: инвалидация, TTL, защита от stampede |
| `rate-limiting-throttling` | Лимиты по tenant/клиенту, токен-бакет, честные 429 |

### aws-agentic-ai (5) — агентные паттерны AWS PG 2026

| Скилл | Что даёт |
|---|---|
| `agent-patterns-overview` | Таксономия 10 паттернов: от basic reasoning до multi-agent |
| `llm-workflow-patterns` | Пять workflow-паттернов: chaining, routing, parallelization, orchestration, reflect-refine |
| `saga-orchestration-agents` | Оркестратор-агент (supervisor): декомпозиция и компенсации |
| `multi-agent-collaboration` | Пиринговая координация равных агентов vs workflow-оркестрация |
| `reflect-refine-loops` | Генератор + независимый оценщик, структурированная обратная связь |

### arch-governance (3) — библиотека правил

| Скилл | Что даёт |
|---|---|
| `fitness-function-catalog` | 20 готовых fitness-функций с regex, три волны внедрения, карточка дистилляции |
| `rule-library-antipatterns` | 8 антипаттернов расширения + фильтр включения из пяти форм правила |
| `architecture-sources-map` | Навигация по 15 блокам источников (168 шт.): что встроено, что кандидат |

### spine-be-docs (1)

| Скилл | Что даёт |
|---|---|
| `check-spine-be-docs` | Ответы о Spine-BE строго по документации репозитория, а не по памяти |

## Как этим пользоваться

- **Агент сам**: в агентном цикле модель зовёт `skill_search` → `skill_load`
  по теме задачи; скиллы не раздувают системный промпт, пока не нужны.
- **Вручную в TUI**: `/skills [запрос]` — список/поиск, `/skill <имя>` —
  загрузить скилл в контекст текущей сессии, `/plugins` — состав плагинов.
- **Из CLI/скриптов**: `arch-be skills list | search <q> | show <name>`,
  `arch-be plugins list | show <name>`.
- **Пополнение из практики**: `/distill` (или инструмент `skill_distill`)
  превращает статью/транскрипт сессии в скилл managed-зоны `arch-distilled`;
  методика авторинга — скилл `skill-authoring`, рецепт «свой плагин за 5
  минут» — в [plugins_and_skills.md](plugins_and_skills.md).

Живой пример цикла «поиск → загрузка → план по методике → ADR» — на
скриншоте в [README-full.md](../README-full.md#русский)
(`docs/screenshots/07-skills.png`).
