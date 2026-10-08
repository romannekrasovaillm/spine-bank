# Дельта: executable-invariants — несущие инварианты под исполняемыми правилами

- Route: Standard (правила и модель кейса; новых компонентов и вендоров нет)
- Created: 2026-10-08
- Волна: TASK v2, B4 («эталонные кейсы едят свою еду»)

## Проблема

Инварианты контура приёма (AD-001…AD-009) охранялись текстовыми правилами
и проверками наличия документов: поведение за ними не исполнялось, а ссылки
`verified_by` в модели указывали на правила по порядковому номеру, а не по
смыслу (AD-001 «идемпотентность» ссылался на C-001 «спайн существует»).
Ступень доверия 3 в строгом режиме (B3, `[trust] require_teeth`) была
недостижима: ни одно правило не проверяло поведение измеримо.

## ADDED

- Правила `C-014`…`C-019` в `CONSTRAINTS.yaml` — исполняемые проверки из
  шаблонов библиотеки (`arch-be rules template apply`, ADR-050) плюс
  контентное правило на карточные данные в любом python-коде:
  - `C-014 idempotency_key_enforced` (шаблон `idempotency-key`, AD-1);
  - `C-015 append_only_journal_enforced` (шаблон `append-only-journal`, AD-2);
  - `C-016 unknown_outcome_no_resend` (шаблон `unknown-outcome-no-resend`, AD-3);
  - `C-017 no_pii_in_logs` (шаблон `no-pii-in-logs`, AD-4);
  - `C-018 validate_before_side_effect` (шаблон `validate-before-side-effect`, AD-6);
  - `C-019 no_card_data_in_code` (`must_not_contain` PAN по `**/*.py`, AD-4) —
    закрывает класс дефекта D11 (ПДн в коде вне `tests/`).
- Эталонные реализации и тесты шаблонов в `skeleton/rule_templates/<id>/`
  (применение зафиксировано в `.arch-handoff/rule-templates.lock`).
- Признак `load_bearing: true` у несущих инвариантов модели: `model/AD-001`,
  `model/AD-002`, `model/AD-003`, `model/AD-004`, `model/AD-006` — ступень
  доверия 3 (B3) опирается на это поле.
- Конфиг кейса `arch-harness.toml` с `[trust] require_teeth = true`: ступень
  3 метрики доверия для этого кейса выдаётся только по измеренным зубьям.

## MODIFIED

- `CONSTRAINTS.yaml`: реестр расширен правилами C-014…C-019; существующие
  правила не ослаблены (severity, glob и pattern прежних правил не менялись).
- `model/AD-001`, `model/AD-002`, `model/AD-003`, `model/AD-004`,
  `model/AD-006`: `load_bearing: true`; `verified_by` переназначен с
  порядковых ссылок (C-001…C-006 по номеру, а не по смыслу) на исполняемые
  правила дельты (C-014…C-018) — звено «инвариант → проверка» теперь
  семантически верное.

## REMOVED

- Ничего не удаляется.

## Чем подтверждается

- `arch-be rules teeth . --save`: у C-014…C-019 зубья подтверждены измерением.
- `arch-be redteam . --save`: D11 пойман `fitness` (C-019), D15/D18 ловятся
  на нарушающей реализации шаблонов; доля обнаружения не ниже прежней.
- `arch-be trust .` (в каталоге кейса, строгий режим) — ступень 3.
- `arch-be gate --repo . --route critical` — PASS.

## Ссылки

ADR-001 (идемпотентность приёма), ADR-002 (append-only журнал),
ADR-003 (разбор неопределённого исхода), ADR-004 (запрет ПДн),
ADR-005 (атомарная проверка лимитов); RISK-002, RISK-003.

## План отката

1. Удалить правила C-014…C-019 из `CONSTRAINTS.yaml`.
2. Удалить `skeleton/rule_templates/`, `.arch-handoff/rule-templates.lock` и
   `arch-harness.toml` кейса.
3. Снять `load_bearing` и вернуть прежние `verified_by` в `model/AD-001…004`,
   `model/AD-006`.

Каждый шаг обратим; артефакты бандла (`EVIDENCE.yaml`) дельта не затрагивает.

## Критерии приёмки

- [x] `arch-be rules teeth . --save` — у правил C-014…C-019 зубья подтверждены
      (измерение 2026-10-08, 19/19 с зубьями).
- [x] `arch-be redteam . --save` — PASS (11/14 = 79 % ≥ 78 %), D11 пойман
      `fitness` (C-019); D15 и D18 пойманы на нарушающей реализации шаблонов.
- [x] `arch-be trust .` (строгий режим, `[trust] require_teeth = true`) —
      ступень 3 (якоря 3, 4, 5); контроль строгости: без `teeth.json`
      ступень падает до 2.
- [x] Гейт кейса на Critical — PASS (19 правил, 0 нарушений).
