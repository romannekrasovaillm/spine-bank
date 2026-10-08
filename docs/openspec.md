# Адаптер OpenSpec (`arch-be openspec`)

Spine читает репозиторий с разметкой [OpenSpec](https://openspec.ai) как
**источник** требований и архитектурных решений, а вердикт выносит своим
детерминированным ядром (`src/openspec.rs`). Это адаптер, а не форк и не
зависимость от OpenSpec CLI: установленный OpenSpec не нужен, файлы читаются
при каждом прогоне (`--watch` не требуется).

## Что адаптер читает

| Источник | Что извлекается |
|---|---|
| `openspec/specs/<capability>/spec.md` | Требования: блоки `### Requirement:` со строками SHALL/MUST (живая истина) |
| `openspec/changes/<id>/specs/**/spec.md` | Требования дельт активных changes (контекст гейта архивации) |
| `openspec/changes/<id>/design.md` | Кандидаты в спайн: секции Decisions/Constraints — в очередь на подтверждение, **не автоматом** |
| `openspec/changes/archive/<id>/design.md` | История решений (откуда правило/решение — источник для expiry) |
| `CONSTRAINTS.yaml` (свой) | Поле `covers:` правил — связь «правило ← требование» |

Блок требования начинается заголовком `### Requirement: <title>` и
заканчивается любым заголовком уровня 1–4 (сценарии `#### Scenario:` в тело
не входят). Требования без строк SHALL/MUST отбрасываются.

## Стабильный идентификатор требования

`openspec:<capability>#<hash8>` — младшие 32 бита FNV-1a от capability и
нормализованного текста строк SHALL/MUST. Заголовок `### Requirement:` в хэш
**не входит** (заголовки OpenSpec нестабильны): переименование требования не
ломает связь, а редактирование текста требования — осознанно ломает (правило
перестаёт его покрывать, что видно в отчёте).

## Связь требование ↔ правило: `covers:`

Markdown OpenSpec адаптер **не трогает** (иначе сломается их
`openspec validate --strict`). Связь живёт на своей стороне — в поле `covers:`
правила `CONSTRAINTS.yaml`:

```yaml
constraints:
  - id: C-001
    name: no_f64_money
    type: must_not_contain
    glob: "src/**/*.rs"
    pattern: "\\bf64\\b"
    severity: critical
    covers: ["openspec:payment-authorization#1a2b3c4d"]
```

Поле опционально и обратно совместимо (`serde default`, схема —
`FitnessRule` в `src/control.rs`); движок находок его не использует — только
отчёт покрытия и гейты адаптера.

## Команды

### `arch-be openspec scan <ROOT> [--json]`

Список требований: стабильный id, capability, заголовок, строки SHALL/MUST,
источник (файл:строка, spec или change). Markdown по умолчанию, `--json` —
`ScanReport`.

### `arch-be openspec coverage <ROOT> [--constraints <PATH>] [--json] [--strict]`

Отчёт покрытия:

- **SHALL всего** — уникальных требований (specs + дельты активных changes,
  дубли по id слиты);
- **покрыто детектором** — есть правило с `covers:` без признака
  `unverifiable`; число расщепляется по доказательности (F2, связка с
  волной B): **с подтверждёнными зубьями** (запись `confirmed` в
  `.arch-handoff/teeth.json`, отпечаток правила сошёлся) и **покрыто
  текстом** (зубья не подтверждены: не измерялись — честное «не
  проверялось», либо измерены беззубыми) — покрытие требований не должно
  быть формальным;
- **unverifiable с owner** — только заглушки `unverifiable: true` с
  назначенным owner (осознанный долг ручного контроля);
- **без решения** — ни детектора, ни unverifiable с owner; список поимённо.

Заглушка `unverifiable` с **пустым** owner — это ещё не решение (свежий
скелет `openspec init`), требование остаётся «без решения».

Находка `covers_orphan` (warn, F4): ссылка `covers:` правила на идентификатор
требования, которого больше нет (правка текста требования меняет его id — так
задумано). Подсказка называет кандидата — непокрытое требование той же
capability с изменённым хэшем: правка текста видна как «правило потеряло
цель», а не теряется молча. На счётчики и exit-коды не влияет.

Файл ограничений: `--constraints`, иначе авто-детект
`<ROOT>/.arch-handoff/CONSTRAINTS.yaml` → `<ROOT>/CONSTRAINTS.yaml`; если
файла нет — все требования «без решения» (честное состояние, не ошибка).
Если существуют обе копии и они различаются, покрытие считается по пакетной,
а в отчёт (markdown и JSON-поле `drift_note`) добавляется пометка
«копии реестра различаются: используется X; Y отличается (drift)» — покрытие
по устаревшей копии молча не засчитывается (волна E).

Exit code: 0 всегда, кроме `--strict` — тогда 1 при наличии требований
«без решения» (гейт CI).

### `arch-be openspec init <ROOT> [--out <DIR>] [--force]`

Генерирует (= `init --from-openspec` из рекомендаций):

- `CONSTRAINTS.from-openspec.yaml` — все найденные SHALL как правила-заглушки
  `unverifiable: true` с пустым owner и проставленным `covers:`. Это черновик
  для вливания в основной `CONSTRAINTS.yaml`: заглушки исполнять нечего —
  `control check` их пропускает как записи ручного контроля
  (`docs/corp-spine.md`), но покрытие в отчёте `openspec coverage` появится
  только после заполнения детектора (type/glob/pattern) или owner;
- `SPINE.draft.md` — кандидаты из секций Decisions/Constraints design.md
  активных changes (очередь на подтверждение архитектором) и история решений
  из `changes/archive/`;
- печатает отчёт покрытия по сгенерированному скелету.

Существующие файлы не затираются без `--force`; регенерация с `--force`
детерминирована (байт-в-байт то же содержимое).

### `arch-be openspec gate --archive <ROOT> <change-id> [--constraints <PATH>]`

Гейт архивации change (точка CI перед `openspec archive`): **exit 1**, если
хотя бы одно требование дельты `changes/<change-id>/specs/` — «без решения»,
либо падает `control check` по файлу ограничений. Иначе PASS, exit 0.

### `arch-be openspec gate --change <ID> <ROOT> [--constraints <PATH>] [--base <REF>]` (F3, ADR-067)

Гейт активного change — для MR, реализующего конкретный change. Одним
вызовом:

1. **покрытие требований дельты change** (F2 в области change): требование
   без решения — `requirement_uncovered`, exit 1; покрытие текстом (зубья
   правил не подтверждены) показывается отдельным счётчиком;
2. **`delta_guard` с этим change как источником** (F1): правки защищённых
   путей (`model/`, `ARCHITECTURE-SPINE.md`, `CONSTRAINTS.yaml`) обязаны
   упоминаться в `proposal.md`/`design.md`/`tasks.md`/`specs/**` активного
   change;
3. **`control check`** по реестру правил;
4. **маршрут значимости по диффу** `base..HEAD` (детектор триггеров, как у
   `gate --route auto`): печатается в шапке отчёта; `--base` — для CI
   (напр. `origin/main...HEAD`), по умолчанию `HEAD` (рабочее дерево).

Без git-репозитория `delta_guard` и маршрут честно помечаются недоступными
(не притворяются пройденными), вердикт решают покрытие и `control check`.
Провал любой части — **exit 1**.

## Составляющая `openspec_coverage` единого гейта (F2, ADR-067)

Покрытие требований OpenSpec — часть `arch-be gate`: составляющая
прогоняется на любом маршруте; без каталога `openspec/` — SKIP с явной
пометкой (паспорт вердикта показывает её в блоке «не проверено»).

- **Блокировка — решением проекта**: находка `requirement_uncovered`
  (требование без решения) — `error`, если `openspec_coverage` входит в
  `[gate.required]` маршрута в `arch-harness.toml`/`config.toml`, иначе
  `warn`. Пример: `[gate.required] critical = [..., "openspec_coverage"]`.
- **Область** (`[gate.openspec_coverage] scope`): `changed` (дефолт) —
  требования дельт активных changes, затронутых диффом `base..дерево`, плюс
  требования живых спек, чьи файлы изменены; `all` — всё, как
  `openspec coverage`.
- **Зубья покрытия**: правило, покрывающее требование, обязано иметь
  подтверждённые зубья (волна B, `arch-be rules teeth --save`); без них
  требование засчитывается «покрыто текстом» — отдельной строкой детали.
  Правила, измеренные беззубыми, дают warn-находку `requirement_text_only`.
  Нет файла измерения — «зубья не измерены» в границах вердикта (блок 2
  паспорта), а не находка.
- Осиротевшие `covers:` (F4) видны и в гейте — warn-находка `covers_orphan`.

## Handoff из change (F6, ADR-067)

`arch-be handoff … --openspec-change <id>`: в пакет кладутся `proposal.md`,
`design.md`, `tasks.md` и дельты спек change (`openspec/changes/<id>/`) — как
`--spec` (контент попадает в `ARCHITECTURE.md` и собранный `SPEC.md`, ссылки —
в `MANIFEST.json`, поле `openspec_change`). Файлы change идут первыми:
лесенка усечения epic-context режет прозу с хвоста, и предмет задачи не
должен попасть под сокращение. Порог контекста маршрута Critical считается с
их учётом. Требования change добавляются в `RUBRIC.yaml` пакета критерием
`openspec_change_requirements` (id + SHALL-тексты) — приёмка по требованиям,
а не по пересказу. Существующая `RUBRIC.yaml` пакета не затирается:
требования не вписываются, и пакет предупреждает. Markdown OpenSpec только
читается — Spine его не пишет (правило 9).

То же в MCP-инструменте `handoff_create` (параметр `openspec_change`).

## Change OpenSpec — источник покрытия `delta_guard` (F1, ADR-062)

Гейт прямых правок спайна (`arch-be delta guard`, составляющая `delta_guard`
единого гейта) засчитывает активный change OpenSpec наравне с дельтой Spine:
правка защищённого файла (`model/`, `ARCHITECTURE-SPINE.md`,
`CONSTRAINTS.yaml`) обязана упоминаться в `proposal.md`, `design.md`,
`tasks.md` или `specs/**/*.md` активного change (`openspec/changes/<id>/`).
Дублировать описание в `changes/<name>/DELTA.md` не нужно — двойной учёт
снят. Источник покрытия назван в отчёте меткой `openspec:<id>` (дельты —
`delta:<name>`).

- **Архив**: change из `openspec/changes/archive/<каталог>/` засчитывается,
  только если архивация вошла в проверяемый диапазон `base..HEAD` (как у
  дельт, T-07); имя каталога архива сопоставляется целиком, с префиксом даты.
- **Правило владения (ADR-055)**: в пост-гейте `harness_run` покрытие (change
  или дельта), созданное/изменённое в диапазоне прогона исполнителя, правку не
  узаконивает — находка `self_approved`. Обычный CI по PR не меняется.
- **Состав источников** — `[delta] sources` в `arch-harness.toml` кейса:
  `["spine", "openspec"]`, либо один из них. Дефолт — оба при наличии
  `openspec/`, иначе `spine`. Неизвестное имя/пустой список — ошибка конфига.
- **`arch-be delta new` при `sources = ["openspec"]`** не создаёт `DELTA.md` и
  подсказывает оформить change средствами OpenSpec: markdown OpenSpec Spine не
  пишет (см. ниже «Чего адаптер НЕ делает»).

## Команда на OpenSpec: что ставить и что не писать дважды

Образец — эталонный кейс `кейсы/openspec-bridge/` (e2e: `bash e2e.sh`).

**Что ставить (контур Spine поверх OpenSpec):**

- Свой `CONSTRAINTS.yaml` со связью `covers:` на стабильные id требований
  (`openspec:<capability>#<hash8>` — печатает `arch-be openspec scan`).
  Это единственная точка связи: markdown OpenSpec Spine не пишет, поэтому
  `openspec validate --strict` у команды не ломается.
- Свою `model/` и `ARCHITECTURE-SPINE.md` — как обычно; гейт их защищает,
  а правки узакониваются активным change (без дублирующего `DELTA.md`).
- `arch-be openspec gate --archive <id>` — точкой CI перед
  `openspec archive`: требования дельты без покрытия останавливают
  архивацию.
- При желании — явный `[delta] sources = ["spine", "openspec"]` в
  `arch-harness.toml` (дефолт и так оба при наличии `openspec/`); при
  `["openspec"]` `arch-be delta new` подскажет оформить change средствами
  OpenSpec вместо создания `DELTA.md`.

**Что НЕ писать дважды:**

- `changes/<name>/DELTA.md` под изменение, уже оформленное change'ом:
  `delta_guard` ищет упоминание защищённого файла в `proposal.md`,
  `design.md`, `tasks.md` и `specs/**/*.md` активного change и называет
  источник в отчёте (`openspec:<id>`). Дубль в `DELTA.md` не нужен и
  создаёт два расходящихся описания одного изменения.
- Проблему и критерии приёмки в артефактах бандла: `proposal.md` (секция
  Why) и сценарии `#### Scenario:` дельты подхватываются evidence-профилем
  (F5) — переписывать их в `PROBLEM.md`/`ACCEPTANCE.md` не требуется.
- Текст требования в `covers:`-комментарии: связь по id, а не по цитате —
  правка текста требования меняет id, и `covers_orphan` (F4) покажет
  правило, потерявшее цель, с кандидатом на перелинковку.

**Границы доверия, которые важно знать:**

- Архивный change засчитывается покрытием, только если архивация вошла в
  проверяемый диапазон `base..HEAD` (как у дельт, T-07); имя каталога
  архива сопоставляется целиком, с префиксом даты.
- Правило владения (ADR-055) действует одинаково для дельт и changes:
  покрытие, созданное внутри диапазона прогона исполнителя, правку не
  узаконивает (`self_approved`, пост-гейт `harness_run`).
- Гейт по-прежнему не читает смысл change: соответствие proposal реальной
  правке — зона ревьюера (составляющая `semantic_quality`).

## Чего адаптер НЕ делает

- Не парсит скиллы и slash-команды OpenSpec, не исполняет их CLI.
- Не переписывает файлы OpenSpec — единственные записываемые артефакты:
  свои `CONSTRAINTS.from-openspec.yaml` и `SPINE.draft.md`.
- Не синкает спайн из design.md автоматически — кандидаты только в черновик.
- Не требует OpenSpec: адаптер живёт за подкомандой `openspec`, ядро
  (`control`, `trace`, `model`) о нём не знает и без разметки OpenSpec
  работает как раньше.

## Roadmap (за пределами MVP)

- **sync**: спайн поверх design.md — подтверждённые кандидаты из
  `SPINE.draft.md` в `ARCHITECTURE-SPINE.md` с обратной ссылкой на change.
- **gate --expiry**: правила, порождённые из archived changes, получают
  expiry/owner из истории archive; просроченные — в отчёт.
- **config.yaml rules → черновики правил**: маппинг per-artifact rules
  OpenSpec в `must_contain` по артефактам.
