# PROOF.md — OpenSpec → Spine: живое доказательство на фикстуре p2p-core

Дата прогона: 2026-09-05 (MSK). Все команды и выводы — реальные, без имитаций.

## 0. Окружение и версии

| Компонент | Версия / факт |
|---|---|
| OpenSpec CLI (`@fission-ai/openspec`, npx) | 1.12.0 |
| arch-be (Spine BE-харнесс) | 0.1.0 (`~/.local/bin/arch-be` → `/home/user/spine-bank/target/release/arch-be`) |
| Python | 3.11.8 (скрипты — только stdlib) |
| LLM для части 7 | endpoint `https://api.deepseek.com`, запрошенный id `deepseek-chat`, фактический (echo в ответе) — `deepseek-v4-flash`, температура 0 |
| Фикстура | `/tmp/openspec_spine_proof/p2p-core` (Rust-скелет, git) |
| Скрипты | `/tmp/openspec_spine_proof/spine_from_openspec.py`, `anchor/anchor_generate.py`, `anchor/anchor_gate.py`, `p2p-core/archive-gated.sh` |

Факт про expiry (установлен по коду и живым прогоном): **просроченное
правило (`expiry` в прошлом) — это warn-находка, гейт она НЕ ломает**
(`src/control.rs` spine-bank: «Просроченное правило (expiry в прошлом) —
warn-находка независимо от …»; тест `fitness_expired_rule_warns_but_passes_and_metadata_accepted`).
Живое подтверждение — в частях 3 и 6б: правило `legacy_marker_cleanup`
с `expiry: 2025-01-01` даёт `[warn]`, exit-код определяется только error-находками.

---

## Часть 1. Фикстура p2p-core с подсаженными нарушениями

Rust-скелет платёжного ядра: `src/money.rs` (authorize), `src/domain/payment.rs`,
`src/infra/db.rs`, `src/lib.rs`, `Cargo.toml`. Подсажено 5 классов нарушений
орг-стандартов банка (маркеры в комментариях кода):

- C-001: суммы в `f64` (`src/money.rs:9`, `src/domain/payment.rs:10-11`, `src/infra/db.rs:9`);
- C-002: `.unwrap()` в коде ядра (`src/money.rs:19`);
- C-003: ПДн в логах — полный PAN и ФИО (`src/money.rs:11-15`);
- C-004: нет ключа идемпотентности в `authorize`;
- C-005: домен импортирует инфраструктуру (`src/domain/payment.rs:7 use crate::infra::db::PgPool`).

Git-фиксация: `eb88cf9 фикстура p2p-core: подсаженные нарушения C-001..C-005`.

---

## Часть 2. OpenSpec зелёный

`openspec init` выполнен ранее (структура `openspec/` с `config.yaml`,
`changes/`, `specs/`). Создан change `add-p2p-authorize`
(`proposal.md`, `design.md` с 5 решениями, `tasks.md`, дельта
`specs/payment-authorization/spec.md` — 5 требований SHALL/MUST со сценариями).

Команда:

```
$ cd /tmp/openspec_spine_proof/p2p-core && npx openspec validate add-p2p-authorize --strict
Change 'add-p2p-authorize' is valid
exit=0
```

```
$ npx openspec validate --all --strict
- Validating...
✓ change/add-p2p-authorize
Totals: 1 passed, 0 failed (1 items)
exit=0
```

**Вердикт: подтверждено — валидатор OpenSpec зелёный.** При этом код с
нарушениями валидатора не интересует: спека и код живут раздельно,
соответствие никто не проверяет. Именно этот разрыв закрывается в части 3.

Далее change заархивирован штатно (для наполнения главных спек):

```
$ npx openspec archive add-p2p-authorize --yes
Warning: 8 incomplete task(s) found. Continuing due to --yes flag.
Specs to update:
  payment-authorization: create
Applying changes to openspec/specs/payment-authorization/spec.md:
  + 5 added
Specs updated successfully.
Change 'add-p2p-authorize' archived as '2026-09-05-add-p2p-authorize'.
exit=0
```

Зафиксированная реальная структура OpenSpec 1.12 после archive:
`openspec/specs/<capability>/spec.md` (актуальные спеки) +
`openspec/changes/archive/2026-09-05-add-p2p-authorize/{proposal,design,tasks}.md`
и `specs/<capability>/spec.md` (архив change'я).

---

## Часть 3. Spine gate FAIL + факт про expiry

Создан `/tmp/openspec_spine_proof/p2p-core/CONSTRAINTS.yaml` — 6 правил
(C-001…C-005 против подсаженных нарушений + C-006 `legacy_marker_cleanup`
с `expiry: "2025-01-01"` для проверки семантики expiry).

Команда и реальный вывод:

```
$ cd /tmp/openspec_spine_proof/p2p-core && arch-be control check . --constraints CONSTRAINTS.yaml
Правил: 6, нарушений: 13 (error: 12, warn: 1)
  [warn] CONSTRAINTS.yaml:0 legacy_marker_cleanup — expiry: правило просрочено 2025-01-01 (владелец: @payments-arch) — пересмотреть, продлить с владельцем или удалить
  [error] src/domain/payment.rs:7 domain_no_infra_import — must_not_contain: запрещённый паттерн 'use crate::infra': use crate::infra::db::PgPool;
  [error] src/domain/payment.rs:10 no_f64_money — must_not_contain: запрещённый паттерн '\bf64\b': pub amount: f64,
  [error] src/domain/payment.rs:11 no_f64_money — must_not_contain: запрещённый паттерн '\bf64\b': pub fee: f64,
  [error] src/infra/db.rs:9 no_f64_money — must_not_contain: запрещённый паттерн '\bf64\b': pub fn save(&self, amount: f64) {
  [error] src/money.rs:0 idempotency_key_required — must_contain: паттерн 'idempotency_key' не найден ни в одном файле по glob 'src/money.rs'
  [error] src/money.rs:9 no_f64_money — ... pub fn authorize(amount: f64, from_card: &str, to_card: &str) -> Payment {
  [error] src/money.rs:12 no_pii_in_logs — ... "authorize: card={} holder=Иванов Иван Иванович amount={}",
  [error] src/money.rs:15 no_pii_in_logs — ... log::info!("payment to card {}", to_card);
  [error] src/money.rs:19 no_f64_money — ... parse::<f64>().unwrap();
  [error] src/money.rs:19 no_unwrap_in_core — ... parse::<f64>().unwrap();
Итог: FAIL
exit=1
```

**Вердикт: подтверждено.** OpenSpec-валидатор зелёный (часть 2) при 12
error-нарушениях в коде — разрыв «спека ≠ код» закрывается гейтом Spine.
Expiry-факт подтверждён живьём: `expiry: 2025-01-01` → `[warn]`, а не error.

---

## Часть 4. Сводная таблица

| Проверка | Команда | Итог | exit |
|---|---|---|---|
| OpenSpec validate (спека) | `npx openspec validate --all --strict` | 1 passed, 0 failed | 0 |
| Spine fitness-гейт (код) | `arch-be control check . --constraints CONSTRAINTS.yaml` | FAIL: 12 error + 1 warn | 1 |
| Переход OpenSpec→Spine | `python3 spine_from_openspec.py p2p-core` | 5/5 требований → черновые правила, 5 инвариантов → SPINE.draft.md | 0 |
| Archive при нарушениях | `./archive-gated.sh add-p2p-callback` | GATE FAIL → архивация ОТКЛОНЕНА | 1 |
| Archive после исправлений | `./archive-gated.sh add-p2p-callback` | GATE PASS (0 error, 1 warn) → archived | 0 |
| Якорный пилот (9 генераций) | `anchor_generate.py` + `anchor_gate.py` | A: 23 наруш. → B: 5 → C: 2 | — |

---

## Часть 5. Скрипт перехода `spine-from-openspec`

Скрипт `/tmp/openspec_spine_proof/spine_from_openspec.py` (python3, только stdlib):

- парсит `openspec/specs/**/*.md` (актуальные спеки OpenSpec 1.12 после archive);
- извлекает требования по заголовкам `### Requirement:` и строкам с SHALL/MUST
  (для классификации берётся всё тело требования — ключевые слова детекторов
  часто стоят на строках-продолжениях без SHALL/MUST);
- на механизуемое требование генерирует черновик правила
  (`must_contain`/`must_not_contain` с TODO-паттерном, severity/owner — TODO),
  на немеханизуемое — запись в разделе `unverifiable:` с `owner: "TODO"`;
- из `design.md` активных и архивных change'ей (`changes/*/design.md`,
  `changes/archive/*/design.md`) извлекает заголовки решений в черновик
  `SPINE.draft.md` — накопительный слой инвариантов.

Команда и реальный вывод на фикстуре:

```
$ cd /tmp/openspec_spine_proof && python3 spine_from_openspec.py p2p-core --out /tmp/openspec_spine_proof/from_openspec
Репозиторий: /tmp/openspec_spine_proof/p2p-core
Требований SHALL/MUST извлечено: 5
Покрыто детекторами (черновые правила): 5
Долг (unverifiable, owner TODO): 0
  [rule]  R-001 must_not_contain   Точное представление денежных сумм
  [rule]  R-002 must_contain       Обязательный ключ идемпотентности
  [rule]  R-003 must_not_contain   Обработка ошибок без паник
  [rule]  R-004 must_not_contain   Защита персональных данных в логах
  [rule]  R-005 must_not_contain   Слоистая архитектура ядра
Архитектурных утверждений в SPINE.draft.md: 5
Записано: /tmp/openspec_spine_proof/from_openspec/CONSTRAINTS.from-openspec.yaml
Записано: /tmp/openspec_spine_proof/from_openspec/SPINE.draft.md
exit=0
```

Артефакты: `from_openspec/CONSTRAINTS.from-openspec.yaml` (5 черновых правил
R-001…R-005 с rationale «OpenSpec: <требование> (<путь к спеке>)») и
`from_openspec/SPINE.draft.md` (5 инвариантов «Решение 1: Деньги — minor units,
не f64» … «Решение 5: Слоистость infra -> domain» с источником
`changes 2026-09-05-add-p2p-authorize, design.md`).

Ветка «долг» (unverifiable) на фикстуре не сработала — все 5 требований
механизуемы. Демонстрация ветки на синтетической спеке
(`/tmp/openspec_spine_proof/synth_repo`, требование «Удобство операторского
интерфейса» — без машинных детекторов):

```
$ python3 spine_from_openspec.py synth_repo --out /tmp/openspec_spine_proof/synth_out
Требований SHALL/MUST извлечено: 2
Покрыто детекторами (черновые правила): 1
Долг (unverifiable, owner TODO): 1
  [rule]  R-001 must_not_contain   Ошибки без unwrap
  [debt]  R-002 unverifiable       Удобство операторского интерфейса
```

**Вердикт: подтверждено.** Тезис для слайда: «переход стоит одну команду,
а не миграцию» — скелет CONSTRAINTS.yaml и черновик SPINE.md генерируются
из живого OpenSpec-репозитория за один запуск; немеханизуемое становится
видимым долгом (unverifiable + owner TODO), а не молчаливой дырой.

---

## Часть 6. Гейт на archive (обёртка `archive-gated.sh`)

Обёртка `/tmp/openspec_spine_proof/p2p-core/archive-gated.sh`: сначала
`arch-be control check . --constraints CONSTRAINTS.yaml`; exit≠0 → отказ
архивации с диагностикой; 0 → `npx openspec archive <change> --yes`.
Создан валидный change `add-p2p-callback`
(`npx openspec validate add-p2p-callback --strict` → «Change 'add-p2p-callback'
is valid», exit=0).

### 6а. Нарушения в коде → gate FAIL → archive ОТКЛОНЁН

```
$ cd /tmp/openspec_spine_proof/p2p-core && ./archive-gated.sh add-p2p-callback
== gate: arch-be control check . --constraints CONSTRAINTS.yaml ==
Правил: 6, нарушений: 13 (error: 12, warn: 1)
  [warn] CONSTRAINTS.yaml:0 legacy_marker_cleanup — expiry: правило просрочено 2025-01-01 ...
  [error] src/domain/payment.rs:7 domain_no_infra_import — ...
  [error] src/money.rs:0 idempotency_key_required — must_contain: паттерн 'idempotency_key' не найден ...
  ... (12 error всего)
Итог: FAIL

== GATE FAIL (exit 1): архивация change 'add-p2p-callback' ОТКЛОНЕНА ==
   Диагностика выше: устраните error-нарушения и повторите.
exit=1

$ npx openspec list
Changes:
  add-p2p-callback     0/3 tasks     just now      # change НЕ заархивирован
```

### 6б. Исправления → gate PASS → archive прошёл

Код исправлен: суммы → `i64` minor units, добавлен обязательный
`idempotency_key`, ошибки через `Result<Payment, CoreError>`, логи — только
`mask_card()` (BIN+last4), домен очищен от `infra` (порт `PaymentStore`,
реализация в `infra/db.rs`).

Промежуточный честный эпизод: первый прогон после исправлений дал 1 error —
правило C-003 (`card \{\}`) поймало ложное срабатывание на уже маскированном
логе `log::info!("payment to card {}", mask_card(to_card))`. Паттерн уточнён
до сырого логирования переменных карты:
`holder=|ФИО|card \{\}", (from_card|to_card)`. Это нормальная жизнь
fitness-правил, зафиксирована, не скрыта.

```
$ ./archive-gated.sh add-p2p-callback
== gate: arch-be control check . --constraints CONSTRAINTS.yaml ==
Правил: 6, нарушений: 1 (error: 0, warn: 1)
  [warn] CONSTRAINTS.yaml:0 legacy_marker_cleanup — expiry: правило просрочено 2025-01-01 ...
Итог: PASS

== GATE PASS: архивация change 'add-p2p-callback' разрешена ==
Specs to update:
  payment-callback: create
Specs updated successfully.
Change 'add-p2p-callback' archived as '2026-09-05-add-p2p-callback'.
exit=0

$ npx openspec list --specs
Specs:
  payment-authorization     requirements 5
  payment-callback          requirements 1
```

Обратите внимание: просроченное expiry-правило осталось `[warn]` и НЕ
заблокировало архивацию — второе живое подтверждение expiry-семантики.

**Вердикт: подтверждено в обоих сценариях.** Разрыв OpenSpec («archive не
смотрит в код») закрыт гейтом: при нарушениях архивация отклонена, после
исправлений — прошла.

### Фиксация состояния фикстуры

Нарушения возвращены коммитом, фикстура осталась демонстрационной:

```
a616a80 Revert "fix: устранены нарушения C-001..C-005 ..."   <- рабочее дерево: нарушения на месте
a1a4d4e fix: устранены нарушения C-001..C-005 (minor units, idempotency_key, Result, маска карт, порт PaymentStore)
2d2d0a0 change add-p2p-callback + archive-gated.sh + CONSTRAINTS.yaml
eb88cf9 фикстура p2p-core: подсаженные нарушения C-001..C-005
```

(Реверт вернул и исходный вариант паттерна C-003 — уточнение паттерна живёт
в коммите a1a4d4e.)

---

## Часть 7. Якорь (пилот): спайн-контекст vs нарушения на входе

**Статус: пилотный сигнал, не статистика.** Доступ к API реальный: ключ взят
из env `DEEPSEEK_API_KEY` (не печатался), endpoint `https://api.deepseek.com`.
Пробный вызов: id `deepseek-chat` — принят (в ответе `model: deepseek-v4-flash`;
вариант `deepseek-v4-flash` перебирать не понадобился, первый id сработал).
Температура 0, `max_tokens: 4000`, по 1 прогону на ячейку.

Методика: 3 задачи × 3 варианта контекста = 9 генераций одного Rust-файла.
Каждая генерация сохранена (`anchor/cells/<T>_<V>/{prompt.txt,raw.md,src/main.rs}`),
затем прогнана ОДНИМ И ТЕМ ЖЕ гейтом:
`arch-be control check <cell> --constraints anchor/CONSTRAINTS.anchor.yaml --json`
(4 правила: `no-f64-money`, `idempotency-key` (must_contain), `no-unwrap`,
`no-pii-logs`). Гейт зафиксирован ДО генераций и не менялся.

Задачи (сокращённо): T1 — функция `authorize` p2p-платежа (сумма, две карты,
комиссия 1%, логирование); T2 — модуль счёта: баланс, зачисление/списание,
печать выписки с контрагентами; T3 — обработчик колбэка платёжной системы
(id платежа, карта, сумма, статус → хранилище + лог).

Варианты контекста (сокращённо; полные тексты — в `anchor/cells/*/prompt.txt`):

- **A** — только задача, без контекста.
- **B** — проза в стиле design.md OpenSpec (~170 слов): «…практика представления
  сумм числами с плавающей точкой признана ошибочной… все денежные величины —
  только в minor units (i64)… каждая денежная операция обязана быть
  идемпотентной: вызов сопровождается idempotency_key… ошибки — исключительно
  через Result: паники, unwrap и expect запрещены… в логи попадает только
  маска карты (BIN и последние четыре цифры)…».
- **C** — спайн, 4 инварианта: «1. ЗАПРЕЩЕНО f64/f32 для денег. Проверка: только
  i64 minor units или Decimal. 2. ЗАПРЕЩЁН вызов денежной операции без ключа
  идемпотентности. Проверка: idempotency_key — обязательный параметр.
  3. ЗАПРЕЩЕНЫ unwrap/expect/panic!. Проверка: только Result. 4. ЗАПРЕЩЕНО
  логировать полный номер карты или ФИО. Проверка: только маска BIN+last4.»

### Таблица: задача × вариант → error-нарушения (реальный вывод `anchor_gate.py`)

```
== ТАБЛИЦА: задача x вариант -> error-нарушения ==
задача | A (без контекста) | B (design.md) | C (спайн)
T1 | 6 | 0 | 0
T2 | 11 | 3 | 0
T3 | 6 | 2 | 2
```

Суммы по вариантам: **A = 23, B = 5, C = 2**.

Характерные реальные находки гейта:

- T1_A: `pub amount: f64` + `idempotency-key … не найден ни в одном файле` (6 error);
- T2_A: 7× `f64`, 4× `.unwrap()` в демо-вызовах, нет идемпотентности (11 error);
- T3_A: `storage.lock().unwrap()` ×3, `f64`, нет идемпотентности (6 error);
- T2_B: деньги и ПДн исправлены, но 3× `.unwrap()` остались (3 error);
- T3_B: `self.data.lock().unwrap()` + нет `idempotency` (2 error);
- T3_C: `store.payments.lock().unwrap()` + нет `idempotency` (2 error).

Чтение сигнала: без контекста модель системно нарушает все 4 орг-правила
(f64-деньги в 3/3, нет идемпотентности в 3/3, unwrap в 2/3). Проза-контекст
(B) убирает f64 и ПДн полностью, но «недожимает» unwrap (Mutex `.lock().unwrap()`
— устойчивая идиома) и идемпотентность в T3. Спайн (C) — минимум нарушений;
оба остаточных случая — та же идиома `lock().unwrap()` и пропуск
идемпотентности в колбэке.

### Честные оговорки

- N=3 задачи, 1 прогон на ячейку, одна модель, температура 0 — это сигнал,
  не доказательство; дисперсия между прогонами не измерена.
- Гейт — regex-правила: считаются лексические совпадения, не семантика
  (напр., `.unwrap()` на `Mutex::lock` формально нарушение, хотя не «денежный
  путь»; must_contain `idempotency` засчитывает само слово).
- Правила C/B-контекстов и гейт написаны одной рукой — возможна подгонка
  лексики (правила спрятаны в прозе B теми же словами, что в инвариантах C).
- Часть находок T2_A — в демонстрационном `main()` сгенерированного файла,
  а не в библиотечной логике.
- Полный эксперимент: пилот 2×2 (±проза-контекст × ±спайн-гейт), N≥10 задач,
  ≥3 прогона на ячейку, 2+ модели, семантические детекторы поверх regex.

---

## Итоговые вердикты

| Часть | Вердикт |
|---|---|
| 1. Фикстура с нарушениями | подтверждено (5 классов нарушений, git eb88cf9) |
| 2. OpenSpec validate зелёный | подтверждено (`--strict`, exit 0) |
| 3. Spine gate FAIL + expiry | подтверждено (12 error, exit 1; expiry → warn, не error) |
| 4. Сводная таблица | выше |
| 5. spine-from-openspec | подтверждено (5/5 требований → правила, 5 решений → SPINE.draft.md; ветка долга показана на синтетике) |
| 6. Archive-гейт | подтверждено в обоих сценариях (FAIL→отказ, PASS→archived) |
| 7. Якорный пилот | выполнен на живых прогонах: A=23, B=5, C=2 нарушения; пилотный сигнал в пользу тезиса, оговорки выше |
