# Дельта: code-roots — модель привязана к коду скелета

- Route: Standard (модель + реестр правил + раскладка скелета; поведение
  контура не меняется)
- Created: 2026-10-08
- Волна: TASK v2, C4 («эталонные кейсы привязаны»)

## Проблема

У всех шести CMP модели не было `code_roots`: `model_drift` и
`context_boundary` проверяли нечего, а ребро `depends_on` не имело за собой
ни одного импорта — модель описывала код декларативно, без проверки.

## ADDED

- `code_roots` у всех CMP модели (`model/CMP-001`…`model/CMP-006`) на пакеты
  `skeleton/`: orchestrator (CMP-001), recipients (CMP-002), journal
  (CMP-003), platform (CMP-004), reconciliation (CMP-005), registry
  (CMP-006).
- Правило `C-022 context_boundary_skeleton` (`context_boundary`, glob
  `skeleton/**/*.py`) в `CONSTRAINTS.yaml`: импорт через границу контекста
  без объявленного `depends_on` — error-находка.
- Конфиг кейса `arch-harness.toml` с `[gate.required] critical =
  ["model_drift"]` — составляющая вступит в силу после слияния волны C1;
  текущий бинарь неизвестную составляющую игнорирует (проверено прогоном).
- Пакетная раскладка скелета: `skeleton/registry/`, `skeleton/journal/`,
  `skeleton/recipients/`, `skeleton/platform/`, `skeleton/orchestrator/`,
  `skeleton/reconciliation/` (по пакету на CMP).

## MODIFIED

- `skeleton/payouts.py` разложен по пакетам CMP без смены поведения: те же 9
  тестов `tests/test_payouts.py` зелёные (`python3 -m pytest tests/ -q`).
  В `Journal` добавлены методы `append_row`/`applied_row` (приём по строке
  реестра — то, чем оркестратор и так пользовался через `idempotency_key`).
- `model/CMP-001`: `depends_on` дополнен CMP-006 (оркестратор читает строки
  реестра — ребро существовало в коде, в модели отсутствовало).
- `model/CMP-002`: `depends_on` исправлен CMP-003 → CMP-006 (проверка
  получателя работает со строкой реестра, а не с журналом; прежнее ребро не
  имело за собой ни одного импорта — `declared-edge-unused`, C2).
- `model/CMP-004`: `depends_on` исправлен CMP-003 → CMP-006 (адаптер
  платформы работает со строкой реестра).
- `model/CMP-005`: `depends_on` дополнен CMP-006 (сверка читает строки
  реестра).
- `docs/WALKING-SKELETON.md`, `VALIDATION.md`: карта файлов скелета
  переписана на пакетную раскладку; `EVIDENCE.yaml` переупакован
  (`arch-be evidence pack . --route critical`).
- `CONSTRAINTS.yaml`: добавлено правило C-022; существующие правила не
  ослаблены.

## REMOVED

- `skeleton/payouts.py` как единый модуль (содержимое переехало в пакеты
  без изменения логики).

## Чем подтверждается

- `arch-be model drift .` — PASS, 0 находок: все шесть code_roots
  существуют, каждое объявленное ребро `depends_on` подкреплено импортом
  (нет `declared-edge-unused`).
- Мутант «импорт мимо depends_on»: `from skeleton.platform import Platform`
  в `skeleton/recipients/__init__.py` ловится `C-022`
  (`context_boundary: импорт 'skeleton/platform' пересекает границу
  контекста: CMP-002 → CMP-004 без depends_on в модели`, FAIL); мутант
  убран после проверки.
- `arch-be gate --repo . --route critical` — PASS.

## Ссылки

ADR-030 (code_roots), ADR-029 (импорты и границы контекстов), C2 из TASK v2
(мёртвые рёбра модели).

## План отката

1. Удалить правило C-022 и `arch-harness.toml`.
2. Вернуть `skeleton/payouts.py` (содержимое пакетов собирается обратно
   одним файлом), вернуть импорты `tests/test_payouts.py`.
3. Убрать `code_roots` и вернуть прежние `depends_on` в `model/CMP-001…006`.
4. Переупаковать `EVIDENCE.yaml`.

## Критерии приёмки

- [x] `arch-be model drift .` — PASS без находок (43 сущности, 6 CMP с
      code_roots, 0 declared-edge-unused).
- [x] Мутант «импорт мимо depends_on» ловится правилом C-022 (проверено
      руками 2026-10-08: `from skeleton.platform import Platform` в
      `skeleton/recipients/__init__.py` → `[error] context_boundary … CMP-002
      → CMP-004 без depends_on в модели`, FAIL; мутант удалён).
- [x] Гейт кейса на Critical — PASS (22 правила, 0 нарушений).
