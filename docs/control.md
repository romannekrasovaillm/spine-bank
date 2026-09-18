# Архитектурный контроль

Детерминированный механический слой харнесса (`src/control.rs`): маршрутизация
изменений по значимости, линтер architecture-spine, сенсоры спецификаций,
fitness functions, генератор ADR. Идеи — `docs/SOURCE_BRIEF.md` §C.3
(триггеры) и §A (AI-Disrupt PDLC, AWS AI-DLC).

## Architecture Significance Score

15 канонических триггеров (`SIGNIFICANCE_TRIGGERS`) — ответы «да/нет» на
вопросы об изменении:

| Триггер | Срабатывает, если изменение… |
|---|---|
| `new_component` | вводит новый компонент/сервис в ландшафт |
| `new_datastore` | вводит новое хранилище данных (СУБД, брокер, объектное) |
| `new_vendor` | добавляет нового вендора/внешнюю зависимость |
| `domain_ownership_change` | меняет владельца домена или границы ответственности |
| `cross_domain_integration` | создаёт интеграцию между доменами |
| `api_contract_change` | меняет существующий API-контракт |
| `data_contract_change` | меняет контракт/схему данных (события, таблицы, CDC) |
| `security_boundary_change` | пересекает или сдвигает границу безопасности (аутентификация, авторизация, шифрование) |
| `trust_zone_change` | меняет зоны доверия (контур КИИ, PCI, DMZ) |
| `consistency_model_change` | меняет модель согласованности (strong → eventual и т.п.) |
| `significant_nfr` | существенно меняет NFR: доступность, latency, пропускную способность |
| `rto_rpo_targets` | задаёт или пересматривает целевые RTO/RPO |
| `irreversible_migration` | необратимая миграция (данные, протоколы без пути отката) |
| `financial_impact` | прямое влияние на деньги: проводки, тарифы, лимиты, штрафы |
| `criticality_or_exception` | затрагивает критичный процесс либо просит architecture exception |

### Маршруты (`significance_score`)

Score — число сработавших триггеров:

| Маршрут | Условие | Режим |
|---|---|---|
| `Fast` | score ≤ `fast_max` (дефолт 1) | Дельта-спека + авто-валидация. |
| `Standard` | `fast_max` < score ≤ `standard_max` (дефолт 4) | Контракт Spec→Plan→Tasks + Architecture Fit автоматически. |
| `Critical` | score > `standard_max` **или** любой из критических: `security_boundary_change`, `irreversible_migration`, `criticality_or_exception` | Solutioning + human decision (гейт A3). |

Пороги конфигурируются секцией `[significance]` в `config.toml` (ADR-034;
дефолты 1/4 — эвристика из SOURCE_BRIEF, статус «рабочая гипотеза с
пересмотром при первых данных пилота»):

```toml
[significance]
fast_max = 1       # score ≤ fast_max → Fast
standard_max = 4   # fast_max < score ≤ standard_max → Standard; выше → Critical
```

Валидация мягкая: `fast_max >= standard_max` — ошибка с понятным текстом
при чтении в `control score`/`significance_score`, а не при загрузке конфига.
Форсирующие critical-триггеры НЕ конфигурируются (fail-safe): сработавший
`security_boundary_change`/`irreversible_migration`/`criticality_or_exception`
даёт Critical при любых порогах.

```bash
arch-be control score --trigger new_component=true --trigger trust_zone_change=true
# Score: 2 (new_component, trust_zone_change триггеров) → маршрут Standard
```

В TUI: `/score new_component=true ...` (без аргументов — справка по триггерам).

### Anti-bypass floor: `--from-diff` (S-1, ADR-034)

Заявленные `--trigger` — честность автора изменения; `--from-diff [GIT_REF]`
добавляет механический минимум: триггеры выводятся из git-диффа и
**объединяются** с заявленными (fail-safe — детектор только добавляет,
маршрут считается по объединённому множеству). Без значения — рабочее
дерево против `HEAD` (staged + unstaged + untracked); со значением —
`git diff GIT_REF...HEAD`. Вне git-репозитория — ошибка с понятным текстом.

Детекторы (эвристики, ложное срабатывание лишь расширяет множество):

| Триггер | Механика |
|---|---|
| `new_component` | добавлен каталог верхнего/второго уровня с манифестом (`Cargo.toml`/`pom.xml`/`package.json`/`go.mod`) или `src/` |
| `new_vendor` | в диффе манифеста зависимостей добавлена строка зависимости |
| `api_contract_change` | изменён/добавлен файл с `openapi`/`asyncapi` в имени (без учёта регистра) |
| `irreversible_migration` | в диффе файла миграций (`migrations/` или `*.sql`) есть `DROP TABLE`/`TRUNCATE`/`DROP COLUMN` |
| `new_datastore` | в конфигах добавлены строки подключения `postgres://`/`mysql://`/`kafka`/`mongodb`/`redis://` |

В выводе у каждого сработавшего триггера — источник: `(declared)`,
`(diff)` или `(declared+diff)`. Триггер, найденный диффом, но не заявленный
флагами, попадает в warn-секцию «расхождение: заявлено vs видно по диффу»
(anti-bypass сигнал; на exit code не влияет).

```bash
arch-be control score --trigger new_component=true --from-diff
# Score: 2 (new_component (declared), new_vendor (diff) триггеров) → маршрут Standard
#   diff: new_vendor: зависимость в Cargo.toml: serde = "1.0"
# ВНИМАНИЕ — расхождение: заявлено флагами vs видно по диффу: new_vendor
```

## Линтер spine (`control spine`)

`ARCHITECTURE-SPINE.md` — позвоночник инвариантов: блоки `AD-<n>` с полями
`Binds:` (кого/что связывает), `Prevents:` (какое расхождение предотвращает),
`Rule:` (машинно-проверяемое правило). Определение блока — заголовок
`### AD-<n> …` или строка `AD-<n>:`/`AD-<n>.`; прочие вхождения — ссылки.
Пример — `examples/specs/ARCHITECTURE-SPINE.example.md`.

| Правило | Severity | Что ловит |
|---|---|---|
| `dup_ad_id` | error | Повторное определение того же AD (со ссылкой на первую строку). |
| `empty_field` | error | У AD-блока отсутствует или пусто поле `Binds`/`Prevents`/`Rule`. |
| `stub_marker` | warn | Заглушки `TODO`, `TBD`, `FIXME`, `XXX`, `???` — заполнить до гейта. |
| `unpinned_version` | warn | Непиннутая версия: `latest`, `*`/`"*"` в зависимости. |
| `broken_ad_ref` | warn | Ссылка на AD, не определённый в файле. |

```bash
arch-be control spine examples/specs/ARCHITECTURE-SPINE.example.md
# spine: нарушений нет
```

## Сенсоры спецификаций (`control sensors`)

Прогон по каталогу спек (нерекурсивно, `*.md`), на каждый файл — два сенсора:

- `required_sections` — наличие обязательных заголовков: `## Проблема`,
  `## Критерии приёмки`, `## Риски` (`REQUIRED_SECTIONS`);
- `upstream_coverage` — все относительные md-ссылки `[..](path.md)`
  существуют относительно каталога (внешние URL, `mailto:`, якоря `#`
  пропускаются) — спека обязана ссылаться на свои входы живыми ссылками.

```bash
arch-be control sensors examples/specs
#   [FAIL] required_sections examples/specs/ARCHITECTURE-SPINE.example.md — нет секций: ## Проблема, ## Критерии приёмки, ## Риски
#   [PASS] upstream_coverage examples/specs/ARCHITECTURE-SPINE.example.md — все ссылки валидны (0)
#   [PASS] required_sections examples/specs/SPEC.example.md — все обязательные секции на месте
#   [PASS] upstream_coverage examples/specs/SPEC.example.md — все ссылки валидны (0)
```

(Spine-файл закономерно падает по `required_sections` — он не спека;
сенсоры применяйте к каталогу функциональных спецификаций.)

## Fitness functions (`control check`)

Машинно-проверяемые утверждения о репозитории из `CONSTRAINTS.yaml`
(по умолчанию `<repo>/.arch-handoff/CONSTRAINTS.yaml`, `--constraints` —
другой файл). Итог PASS, если нет находок с severity `error`; **при FAIL —
exit code 1** (годится для CI). Обход пропускает служебные и производные
каталоги — `.git`, `target`, `node_modules`, `dist`, `__pycache__`, `.next`,
`.pytest_cache` и `.arch-handoff`: правила целятся в артефакты реализации,
а не в документы решения (иначе `must_not_contain` срабатывает на цитаты
контракта внутри пакета handoff);
не-UTF8 файлы читаются с потерями.

Схема правила (поля парсера — `src/control.rs::FitnessRule`):

```yaml
rules:
  - name: no-dbg-macro          # имя → код находки (обязательно)
    type: must_not_contain      # тип проверки (обязательно; опускается у unverifiable-записей)
    glob: "src/**"              # набор файлов — строка или список (дефолт **/*)
    exclude_glob: "src/gen/**"  # исключения из набора — строка или список
                                # (опционально; легитимные точечные отступления)
    pattern: 'dbg!'             # regex (content-правила)
    forbid: ['tui']             # dependency_direction: запрещённые модули
    allow: ['config', 'error']  # …или разрешённые (ровно одно из двух)
    model_dir: model            # context_boundary: каталог модели (дефолт model)
    classes_dir: target/classes # archunit: каталог классов (дефолт — авто-детект)
    jar_dir: vendor/archunit    # archunit: jar'ы (дефолт — ARCHUNIT_HOME/кэш)
    deny: [left-pad]            # deny_dependency: запрещённые пакеты
    reference: "протокол №12"   # deny_dependency: основание запрета → в текст находки
    severity: error             # error | block (синонимы) | warn (дефолт error)
    # Карточка правила (опциональные метаданные, движок не enforce'ит):
    trigger: "признак применимости"
    rationale: "какой отказ предотвращается"      # → в находки (issues[].rationale)
    evidence: "артефакт после проверки"
    reversibility: "обратимо | дорого | необратимо"
    owner: "владелец правила"                     # → в находки (issues[].owner)
    ad: "AD-6"                     # задетый инвариант spine → в находки (issues[].ad)
    adr: "ADR-012"                 # связанное решение → в находки (issues[].adr)
    fix_hint: "что сделать вместо нарушения"      # → в находки (issues[].fix_hint)
    skill: "fitness-functions"     # скилл исправления (skill_load) → в находки (issues[].skill)
    expiry: "2027-01-01"         # дата пересмотра; просроченное правило — warn-находка
    effort_hours: 4.5            # оценка стоимости сопровождения (чел.-часы);
                                 # метаданные — суммируется в rules-report
  - name: cargo-check-passes
    type: command_succeeds
    command: 'cargo check'
    timeout_secs: 120           # дефолт 60
```

Одиннадцать типов правил (включая `max_age` — свежесть evidence-артефактов,
ADR-019 §2, структурные `dependency_direction`/`context_boundary` —
ADR-029/ADR-030, JVM-гейт `archunit` — ADR-039 и `deny_dependency` —
детектор тех-радара корп-спайна, `docs/corp-spine.md`):

| `type` | Семантика | Обязательные поля |
|---|---|---|
| `must_contain` | `pattern` обязан найтись хотя бы в одном файле по `glob` | `glob`, `pattern` |
| `must_not_contain` | `pattern` не должен встречаться; находка на каждое вхождение (файл:строка + сниппет 120 символов) | `glob`, `pattern` |
| `each_file_must_contain` | `pattern` обязан найтись в КАЖДОМ файле по `glob`; находка на каждый файл без совпадения; пустой набор файлов — находка | `glob`, `pattern` |
| `file_exists` | Файл/каталог существует относительно корня репо | `path` |
| `dir_must_have_file` | В КАЖДОМ каталоге по `glob` существует файл `path` (например, у каждого сервиса `services/*` есть `openapi.yaml`); пустой набор каталогов — находка | `glob`, `path` |
| `max_age` | Файл существует И его mtime не старше `now − max_age_days` (свежесть: дата последних учений, ежегодный pentest); отсутствующий файл — нарушение как у `file_exists` | `path`, `max_age_days` |
| `command_succeeds` | `bash -c <command>` в корне репо завершается кодом 0 до `timeout_secs` (по таймауту процесс убивается) | `command` |
| `dependency_direction` | Направление зависимостей (ADR-029): импорты каждого файла набора против `forbid` (запрещённые) или `allow` (разрешённые; пустой — запрет всех внутренних) префиксов модульных путей. Извлечение по расширению: Rust (`crate::…`, включая инлайн-пути), Python, Java/Kotlin, TS/JS (относительные `./…` пропускаются); строки-комментарии игнорируются, строковые литералы и блочные комментарии не разбираются. Пустой набор файлов — находка | `glob`, ровно одно из `forbid`/`allow` |
| `context_boundary` | Границы контекстов (ADR-030): импорты файлов не пересекают `code_roots` чужих CMP-сущностей модели `model_dir` (дефолт `model`), если целевой CMP не в `depends_on` исходного. Разрешение импорта в контекст: префикс в координатах импортов (Python/Java), от каталога файла (TS-относительные), в существующий файл (`src/`, `crates/*` и др.); неразрешённый — внешняя зависимость. Пересекающиеся `code_roots` — ошибка конфигурации | `model_dir` в модели есть CMP с `code_roots` |
| `archunit` | JVM-гейт настоящим ArchUnit (ADR-039): java-правила этого же CONSTRAINTS.yaml (`dependency_direction`/`context_boundary` с glob `**/*.java`) исполняются по байткоду на скомпилированных классах (общий код с `arch-be archunit check`). Находки — с id и severity исходного правила; unsupported — warn; инфраструктурный сбой (нет java/jar'ов/классов, таймаут) — error-находка (fail-closed). Требует JDK и jar'ы (`arch-be archunit fetch`); `timeout_secs` дефолт 300 | — (опционально `classes_dir`, `jar_dir`) |
| `deny_dependency` | Запрещённые пакеты в манифестах зависимостей (тех-радар, `docs/corp-spine.md`): построчный разбор `Cargo.toml` (секции *dependencies), `pom.xml` (`<artifactId>`), `requirements.txt`; пакет из `deny` — находка со ссылкой `reference` на решение техкомитета. `manifests` — glob'ы, дефолт auto (три формата) | `deny` |

Наследование корпоративного спайна (`extends`), overrides через ADR,
`severity: block|warn`, записи ручного контроля (`unverifiable: true`) и
отчёт вверх `arch-be control report --level corp [--json]` — в
`docs/corp-spine.md`.

Примеры:

```yaml
rules:
  - name: no-direct-db-from-api
    type: must_not_contain
    glob: "services/api/**/*.rs"
    pattern: "sqlx::|diesel::"
    severity: error
  - name: spec-has-acceptance
    type: must_contain
    glob: "docs/specs/*.md"
    pattern: "(?m)^## Критерии приёмки$"
    severity: error
  - name: spine-present
    type: file_exists
    path: "docs/ARCHITECTURE-SPINE.md"
    severity: warn
  - name: every-service-has-contract
    type: dir_must_have_file
    glob: "services/*"
    path: "openapi.yaml"
    severity: error
  - name: every-spec-has-acceptance
    type: each_file_must_contain
    glob: "docs/specs/*.md"
    pattern: "(?m)^## Критерии приёмки$"
    severity: error
  - name: dr-drill-evidence-fresh
    type: max_age
    path: "evidence/dr-drill-report.md"
    max_age_days: 365
    severity: error
  - name: unit-tests-pass
    type: command_succeeds
    command: "cargo test --quiet"
    timeout_secs: 900
    severity: error
  - name: llm-layer-isolation          # ADR-029: слои в коде, не в прозе
    type: dependency_direction
    glob: ["src/llm.rs", "src/llm/**"] # glob — строка или список
    allow: [config, error, llm]        # белый список: новая зависимость ломает сборку
    severity: error
  - name: no-legacy-imports
    type: dependency_direction
    glob: "services/**/*.py"
    forbid: [services/legacy]          # префикс с границей сегмента
    severity: error
  - name: context-boundaries           # ADR-030: границы из модели, depends_on — пропуск
    type: context_boundary
    model_dir: model                   # дефолт model; CMP с code_roots обязаны быть
    severity: error
  - name: jvm-archunit-gate            # ADR-039: те же java-правила — настоящим ArchUnit
    type: archunit                     # спек — из dependency_direction/context_boundary
    timeout_secs: 300                  # этого же файла с glob **/*.java
    severity: error
```

> **Структурные правила — эвристика, не AST.** `dependency_direction` и
> `context_boundary` извлекают импорты текстово: `crate::…` в строковом
> литерале даст ложное срабатывание, блочные комментарии `/* … */` не
> вырезаются, экзотические source roots и path-алиасы bundler'ов не
> разрешаются. Для таких layout'ов подключайте стек-нативный инструмент
> (ArchUnit, dependency-cruiser, cargo-modules) через `command_succeeds` —
> паттерн карточки GEN-16; эталонный скрипт для Rust —
> `scripts/deps-check.py` (та же семантика, что у нативного типа). Для
> JVM есть мост первого класса: тип `archunit` и команды
> `arch-be archunit gen|check|fetch` исполняют те же java-правила
> настоящим ArchUnit по байткоду — см. `docs/archunit.md` (ADR-039).

> **Якоря в regex: `^`/`$` — против ВСЕГО файла.** Content-правила матчат
> `pattern` против всего содержимого файла, а не построчно (как grep):
> голое `^` означает начало файла. Для строчной семантики используйте
> флаг `(?m)` — пример выше (кейс тестирования 2026-09-01: правило
> `'^\s*burst'` молча не находило значение на второй строке конфига).

Glob — простой: `**` — любая глубина (включая ноль сегментов), `*` — внутри
сегмента, `?` — один символ. Поле `glob` принимает строку или список строк
(ADR-029: `src/x.rs` + `src/x/**` одним правилом; наборы объединяются).
Больше примеров — `examples/CONSTRAINTS.example.yaml`
(поля схемы — `name`/`type`, как показано выше). Необязательное поле
`id: C-NNN` связывает правило с трассировкой (`verified_by: C-NNN` в AD,
см. `arch-be trace check`); fitness-парсер его игнорирует.

```bash
arch-be control check ~/work/payment-svc
# Правил: 4, нарушений: 0 (error: 0, warn: 0)
# Итог: PASS
```

Per-rule timing (M-1a): длительность каждого правила замеряется и попадает
в отчёт (`durations`); если есть правила медленнее 1 с, текстовый вывод
после сводки показывает топ-5 самых медленных:

```
Самые медленные правила:
  12.3s cargo-test-all-targets
  2.1s cargo-clippy-deny-warnings
```

**Находки с архитектурным смыслом.** Если у сработавшего правила в карточке
заполнены `ad`/`adr`/`rationale`/`owner`/`fix_hint`/`skill`, движок переносит
их в находку: видно задетый инвариант и путь исправления, а не только имя
правила. В текстовом выводе контекст печатается одной строкой-отступом под
находкой (только при наличии `rationale`/`fix_hint`):

```
  [error] src/main.rs:12 no_unsafe — must_not_contain: запрещённый паттерн 'unsafe\s*(\{|fn|impl)': …
      ↳ AD-6 · зачем: unsafe снимает гарантии памяти · как чинить: убрать unsafe-блок · скилл: fitness-functions
```

### Машинный вывод `--json` (SDK-контракт v1)

Флаг `--json` печатает в stdout одну строку JSON — сериализацию
`FitnessReport` (поля — `src/control.rs::FitnessReport`; назначение —
SDK и CI, см. `docs/sdk.md` и `sdk/CONTRACT.md` §2). Живой прогон
2026-09-03 на фикстуре `banking/demos/cli-from-claude-code/scenario3-gate/fixtures`:

```bash
arch-be control check banking/demos/cli-from-claude-code/scenario3-gate/fixtures \
  --constraints banking/demos/cli-from-claude-code/scenario3-gate/fixtures/CONSTRAINTS.yaml --json
```

```json
{"repo":"banking/demos/cli-from-claude-code/scenario3-gate/fixtures","passed":true,"issues":[],"summary":"Правил: 3, нарушений: 0 (error: 0, warn: 0)"}
```

Красный прогон (временная копия фикстуры + `src/hotfix.py` с 16-значным PAN):

```json
{"repo":"/tmp/…/repo","passed":false,"issues":[{"file":"src/hotfix.py","line":1,"rule":"no_pan_in_code","message":"must_not_contain: запрещённый паттерн '\\b\\d{16}\\b': pan = \"4276550012345678\"","severity":"error"}],"summary":"Правил: 3, нарушений: 1 (error: 1, warn: 0)"}
```

Схема: `repo` (путь как передан), `passed`, `summary` (та же строка, что в
текстовом выводе), `issues[]` — `file`, `line` (0 — находка на файл целиком),
`rule` (имя правила), `message` (тип проверки + сниппет), `severity`
(`"error"` | `"warn"`). Аддитивное поле `durations[]` — `{rule, ms}` на каждое
правило (per-rule timing; добавление полей контракт v1 не ломает, клиенты без
него работают как раньше). Аддитивные карточные поля `issues[]` — `ad`,
`adr`, `rationale`, `owner`, `fix_hint`, `skill` — присутствуют, только если
заполнены в карточке правила (`skip_serializing_if`; у находок линтера
spine/наследования/overrides их нет). Те же поля несёт и MCP-инструмент
`fitness_check` (`structuredContent.issues[]`).

**Семантика exit-кодов**: 0 — `passed=true`; 1 — `passed=false`, при этом
**JSON всё равно напечатан** (красный гейт — это данные отчёта, а не сбой
инструмента); ошибка запуска (нет репозитория/файла ограничений) — ненулевой
exit **без** JSON, причина в stderr. Потребитель (SDK, CI-скрипт) обязан
различать второй и третий случаи: парсить JSON даже при exit 1.

### Форматы CI (`--format`): SARIF / JUnit / GitLab Code Quality / markdown

Флаг `--format sarif|junit|gitlab-codequality|markdown` (дефолт `text`;
несовместим с `--json`) печатает отчёт в нативном формате площадок CI —
тем же контрактом каналов, что `--json`: машинный отчёт строго в stdout
(годен для редиректа в файл-артефакт), exit-коды не меняются (FAIL — отчёт
напечатан полностью, exit 1). Рендеры — чистые функции над отчётами,
`src/report_fmt.rs`; тот же флаг есть у `arch-be gate`, `arch-be trace check`
и `arch-be contract-diff`:

| Формат | Стандарт | Назначение |
|---|---|---|
| `sarif` | SARIF 2.1.0 (`rules` + `results` с `level` error/warning, `locations`, стабильные `partialFingerprints`) | code scanning GitHub, импорт сторонних сканеров в GitLab |
| `junit` | JUnit XML (`testsuite` на составляющую/правило, `testcase` на находку, `failure` только у error; SKIP — `<skipped/>`) | Jenkins `junit(...)`, виджеты тестов площадок |
| `gitlab-codequality` | GitLab Code Quality JSON (`description`, `check_name`, `fingerprint` по правилу+файлу+строке, `severity` error→major/warn→minor, `location`) | артефакт `reports.codequality` — нарушения в интерфейсе merge request без ручной настройки |
| `markdown` | таблица находок + сводка + статусы групп | job summary, комментарий к MR |

Ограничения: у GitLab Code Quality `location.path`/`lines.begin` обязательны —
находки без адреса (трассировка, составляющие гейта) получают путь-заглушку
`(repository)` и строку 1; потолок находок в машинном отчёте — 1000 (полный
список — текстовым выводом). Fingerprint стабилен между прогонами (FNV-1a по
правилу+пути+строке, без текста сообщения — правка формулировки не плодит
«новые» находки в MR). Готовые джобы под площадки раскладывает
`arch-be connect ci --provider gitlab|github|jenkins` (`docs/CONNECT.md`).

```bash
arch-be gate --format gitlab-codequality > codequality-spine.json   # артефакт MR
arch-be control check . --format junit > fitness.xml                # junit(...) в Jenkins
arch-be trace check кейсы/legacy-survey --format markdown           # сводка в job summary
```


## Реестр правил (`control rules-report`)

Отчёт по карточкам CONSTRAINTS.yaml (markdown в stdout; M-1b/C-3) — реестр
как объект сопровождения: сколько правил, чьих, с какой ценой:

```bash
arch-be control rules-report . --constraints CONSTRAINTS.yaml
# # Отчёт по правилам: CONSTRAINTS.yaml
#
# Всего правил: 13
# По типам: command_succeeds 3, file_exists 1, must_contain 5, must_not_contain 4
# По severity: error 13
#
# | Правило | Тип | Severity | Owner | Expiry | exclude_glob | effort_hours |
# |---|---|---|---|---|---|---|
# | no-pan | must_not_contain | error | Иванов | 2027-01-01 | 0 | 4.5 |
# ...
# ## Находки
# ### Правила без owner/expiry
# - bare-rule (нет owner, expiry)
# ### Просроченные правила (expiry в прошлом)
# - old-rule — expiry 2020-01-01 (владелец: Петров)
# ### Правила с exclude_glob (прокси отступлений/ложных срабатываний)
# - contracts-except-legacy: services/a, services/b
# ### Стоимость сопровождения (git-прокси)
# Коммитов за последние 90 дней, трогавших файл ограничений: 7
#
# правил 13, суммарный effort_hours 6.5 (покрыто 2 правил)
```

- `--constraints` — как у `check` (дефолт `<repo>/.arch-handoff/CONSTRAINTS.yaml`);
- просрочка expiry — та же логика даты, что у warn-находки в `check`;
- git-прокси — `git log --since="90 days ago" --oneline -- <файл>`; вне
  git-репозитория (или без git) — строка «недоступно», не ошибка;
- `effort_hours` из карточек суммируется в итоговой строке (метаданные,
  движок не enforce'ит).

## Гейт A4: репетиция отката (`control gate`)

Rollback-first по мотивам AI-native SDLC (у Anthropic rollback — «самый
отрепетированный путь»): для маршрута Critical гейт A4 (conformance evidence)
требует, чтобы план отката был не просто написан, а **отрепетирован**.

Handoff-пакет несёт машиночитаемый план `.arch-handoff/ROLLBACK.yaml`
(генерируется `handoff_create`/`arch handoff` вместе с пакетом, повторная
генерация не затирает правки архитектора). Схема (`src/rehearsal.rs::RollbackPlan`):

```yaml
baseline_commit: "a1b2c3d"        # якорь отката (обязателен для Critical)
steps:                            # shell-шаги отката (хотя бы один для Critical)
  - name: якорь-доступен
    run: git cat-file -t a1b2c3d
  - name: откат-на-baseline
    run: git reset --hard a1b2c3d
verify: test -z "$(git status --porcelain --untracked-files=no)"   # опционально
```

Для Critical пакет вообще не собирается без git-якоря и непустого плана
отката (валидация на генерации handoff).

```bash
arch control gate A4 <repo> --rehearse
# Репетиция отката (baseline a1b2c3d):
#   [PASS] якорь-доступен — commit
#   [PASS] откат-на-baseline — ...
#   [PASS] verify — ...
# A4: маршрут Critical — откат отрепетирован на a1b2c3d (...)
# Итог: PASS
```

Механика репетиции: шаги выполняются во **временном git-worktree на
baseline_commit** (detached, каталог в `/tmp`, убирается автоматически) —
рабочее дерево и история основного репозитория не трогаются; `git reset --hard
<baseline>` внутри одноразового worktree безопасен. Шаги — fail-fast, как в
боевом откате. Шаги с внешними/деструктивными/недетерминированными эффектами
**отклоняются до запуска** (REFUSED с диагностикой): denylist `git push`,
`git clean`, `rm` с флагами, сетевые команды (curl/ssh/rsync/…), `sudo`,
управление процессами/сервисами хоста, docker/kubectl/helm/terraform,
публикация артефактов, `DROP DATABASE/TABLE/SCHEMA` — такой шаг надо
переписать верификационной командой или вынести из репетируемого плана.

Результат (PASS/FAIL + лог шагов) пишется в evidence пакета —
`.arch-handoff/REHEARSAL.json`; для маршрута Critical этот артефакт входит и
в Evidence Bundle (`arch evidence pack`). Гейт можно оценивать и без
`--rehearse` — по существующему evidence; если план изменился после репетиции
(baseline в `ROLLBACK.yaml` ≠ baseline в evidence), evidence считается
протухшим и гейт падает.

**Порог обязательности** — `--require-rehearsal fast|standard|critical|never`
(дефолт `critical`: Fast/Standard — advisory, Critical без успешной репетиции
не проходит). **Exit-коды**: FAIL гейта → **exit 1** (годится для CI);
нереализованный гейт (A0–A3, A5) — ошибка.

Ограничение: репетиция проверяет **применимость** шагов на baseline (якорь
резолвится, команды завершаются нулём, дерево возвращается чистым), а не
фактический откат коммита исполнителя — его в момент гейта ещё не существует.

## Аудит флота worktree (`fleet audit`)

SSOT-контроль модели «5.2 + дельта-протокол»: при параллельной агентной
разработке во флоте git-worktree каждый исполнитель несёт копию
архитектурного спайна — копии расползаются (точные дубли документации +
дрейф содержимого). `arch-be fleet audit` измеряет это механически: сканирует
документацию (`**/*.md|yaml|yml|json`, без `.git`/`target`/`node_modules`/
`.arch-handoff`) по набору каталогов-worktree и считает:

- всего файлов, точные дубли (есть идентичная копия того же пути в другом
  worktree) и их долю;
- файлы-«ядро» (присутствуют во ВСЕХ worktree);
- дрейф: пути с разным содержимым у владельцев; канон — majority-версия
  (при равенстве голосов — детерминированно меньший хэш), отступники
  называются поимённо; сводка per-worktree «N/M файлов отличаются от канона».

```bash
arch-be fleet audit кейсы/fleet-spine-drift/fleet/wt-*   # пример — кейс 007
#   точные дубли: 8 (66.7%) · ядро: 3 · файлов с дрейфом: 1
#   CONSTRAINTS.yaml — отступники: wt-c
# Итог: DRIFT — копии спайна разошлись (exit 1)
```

Worktree можно перечислить из git: `--repo <path>` (берутся из
`git worktree list --porcelain`, добавляются к позиционным путям).
`--include <glob>` (повторяемый) сужает сканирование (напр. `model/**`),
`--format json` — машинный вывод. **Exit-коды**: дрейф хотя бы одного файла →
**exit 1** (гейт для CI); независимый второй триггер — `--fail-on-dupes <pct>`
(exit 1, если доля дублей выше порога). Агентный вызов — инструмент
`fleet_audit` (та же сводка текстом или JSON).

## Гейт прямых правок спайна (`delta guard`)

Вторая опора модели 5.2: изменения спайна идут только дельтами `changes/<id>`
(OpenSpec-протокол, см. `arch-be delta new/validate/archive`). `arch-be delta guard`
— CI-запрет «прямых коммитов в model/ мимо changes/»: файлы, изменённые по
`git diff --name-only <base>` и попадающие под защищённые пути, обязаны
упоминаться (путём или именем, напр. `model/adr/ADR-003.md` или `ADR-003`)
в теле хотя бы одной АКТИВНОЙ дельты `changes/*/DELTA.md` (архивные не
засчитываются — влитая истина не освобождает от протокола для новых правок).

- Защищаемые пути по умолчанию: `model/`, `ARCHITECTURE-SPINE.md`,
  `CONSTRAINTS.yaml`; повторяемый `--protect <префикс>` заменяет дефолт
  целиком (задан хотя бы один — дефолт не действует).
- База diff по умолчанию `HEAD` (staged+unstaged рабочего дерева;
  untracked-файлы git-diff не видит). Для CI: `--base origin/main...HEAD` —
  трёхточечную форму разбирает сам git.
- **Exit-коды**: непокрытая правка защищённого файла → **exit 1** со списком
  нарушений и подсказкой «оформите правку дельтой: arch-be delta new <name>»;
  нет изменений защищённых путей или все покрыты дельтами → PASS, exit 0.

Пример CI-использования (оба гейта в пайплайне):

```bash
arch-be delta guard --base origin/main...HEAD          # спайн меняется только дельтами
arch-be fleet audit --repo . --fail-on-dupes 50        # флот worktree без дрейфа и дубль-распада
```

Живой мини-кейс обоих гейтов — `кейсы/fleet-spine-drift/` (007).

## Единый гейт (`arch-be gate`)

Одна команда прогоняет весь детерминированный контур контроля репозитория и
сводит исходы в один exit-код: **провал любой составляющей → exit 1**
(механически, по кодам возврата составляющих — без разбора строк вывода;
это контракт для CI и для хуков `arch-be connect`, которые вызывают именно
`arch-be gate`).

```bash
arch-be gate [--repo <path>] [--route auto|fast|standard|critical] [--base <git-ref>] [--constraints <file>] [--format text|sarif|junit|gitlab-codequality|markdown]
```

`--format` (волна 2, п.8) — машинные форматы для CI в stdout (см. «Форматы CI»
выше): для GitLab merge request — `gitlab-codequality` (артефакт
`reports.codequality`), для Jenkins — `junit`, для GitHub — `sarif`, сводка в
job summary — `markdown`. Текстовый вывод не меняется; exit-код общий: провал
любой составляющей → exit 1. Готовые джобы — `arch-be connect ci --provider …`.

Составляющие (на любом маршруте):

| Составляющая | Что прогоняет | FAIL, когда |
|---|---|---|
| `fitness` | `control check` по `CONSTRAINTS.yaml` (дефолт `<repo>/.arch-handoff/CONSTRAINTS.yaml`, `--constraints` — другой файл) | находки severity error; файл есть, но не читается/не валиден |
| `delta_guard` | `delta guard` (защищённые пути: `model/`, `ARCHITECTURE-SPINE.md`, `CONSTRAINTS.yaml`) | правки защищённых файлов без активной дельты |
| `rule_weakened` | анти-ослабление реестра правил относительно git-базы (см. ниже) | правило удалено / `exclude_glob` расширен / severity понижен без активного override |
| `spine_lint` | `control spine ARCHITECTURE-SPINE.md` | error-находки линтера |
| `trace_check` | `trace check` (нужны `model/` и `CONSTRAINTS.yaml` в корне) | error-находки трассировки |

На маршрутах **Standard/Critical** добавляются:

| Составляющая | Что прогоняет | FAIL, когда |
|---|---|---|
| `nfr` | все четыре проверки `nfr` (budget/availability/capacity/cost) | error-находка хотя бы одной |
| `evidence_verify` | `evidence verify` по каждому активному change-dir `changes/<name>/EVIDENCE.yaml` | бандл неполон или хэш сошёлся с дрейфом |

**Fail-soft (SKIP, не падение):** у составляющей нет входа — нет
`CONSTRAINTS.yaml`, не git-репозиторий, нет `model/`, нет активных бандлов.
Сбой выполнения при наличии входа (битый YAML, неработающее правило) — FAIL
с причиной: гейт, молча пропускающий поломку собственной конфигурации,
не гейт.

**Маршрут.** `--route auto` (дефолт) вычисляет маршрут механически из
git-диффа (`detect_diff_triggers` + `score_with_sources` с пустым declared —
тот же anti-bypass floor S-1, что у `control score --from-diff` и MCP
`significance_from_diff`; `--base` задаёт git-ref, без него — рабочее дерево
против HEAD). Дифф недоступен (не git-репозиторий, нет HEAD) — fail-safe
маршрут Critical с пометкой причины в строке «Маршрут:». Явный
`--route fast|standard|critical` переопределяет авто-режим.

```bash
arch-be gate --repo ~/work/payment-svc
# Гейт: ~/work/payment-svc
# Маршрут: Fast (auto: score 0 (триггеров нет))
#   [PASS] fitness — Правил: 13, нарушений: 0 (error: 0, warn: 0)
#   [PASS] delta_guard — изменённых файлов: 2, защищённых среди них: 0
#   [PASS] rule_weakened — реестр правил не ослаблен относительно HEAD
#   [PASS] spine_lint — находок: 0 (error: 0)
#   [SKIP] trace_check — нет каталога model/
# Итог: PASS
```

### Находка `rule_weakened` (анти-ослабление гейта)

Сравнение текущего `CONSTRAINTS.yaml` с версией в git-базе (`--base`, дефолт
HEAD). Error-находка с именем правила — за каждое из:

- правило из базы **удалено** из текущего файла (по именам);
- у правила появился или расширился **`exclude_glob`** (новые исключения);
- **severity понижен** (error → warn; эквиваленты `critical`/`high`/`block` ≡
  error ослаблением не считаются).

Ослабление **узаконено** (находки нет), если в текущем файле есть активный
override на это правило (по имени или `id`) с ADR — гейт «только через ADR»
(`docs/corp-spine.md`): `overrides: [{rule, adr, until}]`, срок не истёк.
Сравнение — по плоскому разбору файла (оба корня `rules:`/`constraints:`),
`extends` не разворачивается. Входа нет (не git, файла нет в базовой
ревизии, реестр новый) — SKIP.

Типовой антикейс: агент под давлением красного гейта «чинит» его удалением
правила — `rule_weakened` валит прогон, пока ослабление не оформлено через
ADR-override.

## ADR-шаблон (`control adr`)

```bash
arch-be control adr "Оркестрация платежа — отдельный сервис" --dir docs/adr
# ADR создан: docs/adr/ADR-003--------.md
```

Номер — `max(существующие ADR-NNN-*) + 1`; каталог создаётся при отсутствии.
Имя файла — `ADR-NNN-kebab-title.md` (не-ASCII, включая кириллицу, → `-`,
без транслитерации). Шаблон AI-DLC с placeholder-комментариями:

```
# ADR-003. <title>
- Date: <дата>
- Status: Proposed
## Context          — силы, ограничения, цена бездействия
## Decision         — одно явно сформулированное решение
## Alternatives Considered   — таблица «вариант | плюсы | минусы»
## Consequences     — ### Positive / ### Negative (обязательно!)
## Reversibility    — reversible | costly | irreversible + обоснование
## References       — ссылки на spine (AD-n), спеки, обсуждения
```

В TUI: `/adr new <title>` (в `docs/adr` рабочего каталога). Качество
заполненного ADR оценивается рубрикой `adr_quality`
(`docs/rubrics_and_benchmarks.md`).
