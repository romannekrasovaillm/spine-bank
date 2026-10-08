# Кейс: openspec-bridge — команда на OpenSpec под гейтом Spine

Механический кейс (без LLM): репозиторий, где команда ведёт требования и
изменения в OpenSpec (`openspec/`), а архитектурный контур — в Spine
(`model/`, `ARCHITECTURE-SPINE.md`, `CONSTRAINTS.yaml` со связями `covers:`).
Двойного учёта нет: change OpenSpec засчитывается гейтом `delta_guard` как
источник покрытия наравне с `changes/<name>/DELTA.md` (F1, ADR-062).

## Состав

```
openspec/
  specs/payments/spec.md                       # живая спека: 2 требования (SHALL)
  changes/
    add-limits/                                # АКТИВНЫЙ change: proposal, tasks,
                                               #   design, дельта спеки (+1 требование)
    archive/2026-09-20-add-idempotency/        # реализованный change (история)
model/                                         # CMP-001 (приём), CMP-002 (лимиты),
                                               # AD-001/AD-002 (load_bearing), OWNER
src/intake/, src/limits/                       # python-скелет контура
ARCHITECTURE-SPINE.md                          # AD-1 идемпотентность, AD-2 точные деньги
CONSTRAINTS.yaml                               # C-001…C-003 с covers: openspec:payments#…
e2e.sh                                         # сквозной прогон (см. ниже)
```

## Что показывает e2e (`bash e2e.sh [путь к arch-be]`)

Прогон разворачивает кейс во временный git-репозиторий и проходит жизненный
цикл change:

1. **Чистое дерево** — `arch-be gate --route fast` зелёный.
2. **Правка модели в рамках активного change**: `model/CMP-001` получает
   `depends_on: [CMP-002]` (упомянуто в `proposal.md` и `tasks.md` change
   `add-limits`) → `delta_guard` PASS с источником `openspec:add-limits`.
   Дублировать описание в `DELTA.md` не нужно.
3. **Контроль**: правка `CONSTRAINTS.yaml`, НЕ упомянутая в change, → гейт
   красный, находка называет файл (гард не пустой).
4. **`arch-be openspec gate --archive . add-limits`** — требования дельты
   покрыты правилами с `covers:` → архивация разрешена.
5. **Архивирование** (семантика `openspec archive`): change уезжает в
   `openspec/changes/archive/2026-10-08-add-limits/`, требование сливается в
   живую спеку → гейт на диапазоне, включающем архивацию, PASS: покрытием
   служит архивный change внутри диапазона (`openspec:2026-10-08-add-limits`).
6. **`arch-be openspec coverage . --strict`**: 3/3 требований покрыто и после
   слияния дельты в живую спеку (id требования не меняется — текст не
   трогали).

Фиксированный вывод прогона на коммите введения кейса — в разделе ниже.

## Зафиксированный прогон

```
== Шаг 1 ==  delta_guard PASS — покрытие: model/CMP-001-priyom-platezhey.md ← 'openspec:add-limits'
== Шаг 2 ==  гейт красный (exit 1), находка: CONSTRAINTS.yaml — не упоминается ни в одном активном источнике покрытия
== Шаг 3 ==  openspec gate --archive add-limits → Итог: PASS (требований change: 1, без решения: 0)
== Шаг 5 ==  delta_guard PASS — покрытие: model/CMP-001-priyom-platezhey.md ← 'openspec:2026-10-08-add-limits'
== Шаг 6 ==  coverage --strict: SHALL всего 3, покрыто детектором 3, без решения 0
Итог: e2e openspec-bridge PASS
```

## Что кейс НЕ показывает

- Правило владения (ADR-055, `self_approved`) срабатывает в пост-гейте
  прогона исполнителя (`harness_run`), а не в обычном CI по PR — здесь не
  воспроизводится.
- Правка ТЕКСТА требования ломает связь `covers:` (так задумано) — находка
  `covers_orphan` покрыта тестами адаптера и кейсом не демонстрируется.
