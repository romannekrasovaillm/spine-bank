# Дельта: executable-invariants — несущие инварианты под исполняемыми правилами

- Route: Standard (правила и модель кейса; новых компонентов и вендоров нет)
- Created: 2026-10-08
- Волна: TASK v2, B4 («эталонные кейсы едят свою еду»)

## Проблема

Инварианты кейса (AD-001…AD-007) охранялись только текстовыми правилами
(`must_contain`/`must_not_contain`): слово в коде есть, а поведение за ним
никто не исполняет. Мутация D18 («слово на месте, логики нет») такой контур
не ловит, а ступень доверия 3 по несущим AD (B3) опирается на правила с
подтверждёнными зубьями.

## ADDED

- Правила `C-016`…`C-021` в `CONSTRAINTS.yaml` — исполняемые проверки из
  шаблонов библиотеки (`arch-be rules template apply`, ADR-050) плюс
  контентное правило на карточные данные в любом python-коде:
  - `C-016 idempotency_key_enforced` (шаблон `idempotency-key`, AD-1);
  - `C-017 append_only_journal_enforced` (шаблон `append-only-journal`, AD-2);
  - `C-018 unknown_outcome_no_resend` (шаблон `unknown-outcome-no-resend`, AD-3);
  - `C-019 no_pii_in_logs` (шаблон `no-pii-in-logs`, AD-4);
  - `C-020 validate_before_side_effect` (шаблон `validate-before-side-effect`, AD-5);
  - `C-021 no_card_data_in_code` (`must_not_contain` PAN по `**/*.py`, AD-4) —
    закрывает класс дефекта D11 (ПДн в коде вне `tests/`).
- Эталонные реализации и тесты шаблонов в `skeleton/rule_templates/<id>/`
  (применение зафиксировано в `.arch-handoff/rule-templates.lock`).
- Признак `load_bearing: true` у несущих инвариантов модели: `model/AD-001`,
  `model/AD-002`, `model/AD-003`, `model/AD-004`, `model/AD-005` — ступень
  доверия 3 (B3, `[trust] require_teeth`) опирается на это поле.

## MODIFIED

- `CONSTRAINTS.yaml`: реестр расширен правилами C-016…C-021; существующие
  правила не ослаблены (severity, glob и pattern прежних правил не менялись).
- `model/AD-001-idempotentnost.md`, `model/AD-002-append-only.md`,
  `model/AD-003-sverka-vmesto-povtora.md`, `model/AD-004-pdn-v-zaprete.md`,
  `model/AD-005-proverka-do-otpravki.md`: `load_bearing: true` и
  `verified_by` дополнен ссылкой на исполняемое правило (C-016…C-020);
  прежние ссылки на контентные правила сохранены.

## REMOVED

- Ничего не удаляется.

## Чем подтверждается

- `arch-be rules teeth . --save`: у C-016…C-021 зубья подтверждены измерением
  (подмена нарушающей реализации из шаблона / вставка PAN в код).
- `arch-be redteam . --no-decision-quality`: D11 пойман `fitness` (C-021),
  D15 и D18 ловятся на нарушающей реализации шаблона.
- `arch-be gate --repo . --route critical` — PASS.

## Ссылки

ADR-001 (ключ идемпотентности), ADR-004 (построчная атомарность),
AD-001…AD-005, RISK-001 (двойная выплата), RISK-003 (утечка ПДн).

## План отката

1. Удалить правила C-016…C-021 из `CONSTRAINTS.yaml`.
2. Удалить `skeleton/rule_templates/` и `.arch-handoff/rule-templates.lock`.
3. Снять `load_bearing` и новые ссылки `verified_by` в `model/AD-001…005`.

Каждый шаг обратим; на код скелета (`skeleton/payouts.py`) дельта не влияет.

## Критерии приёмки

- [x] `arch-be rules teeth . --save` — у правил C-016…C-021 зубья подтверждены
      (измерение 2026-10-08, 21/21 с зубьями).
- [x] `arch-be redteam . --no-decision-quality` — D11 пойман `fitness`
      (C-021); D15 и D18 пойманы на нарушающей реализации шаблонов.
- [x] Гейт кейса на Critical — PASS (21 правило, 0 нарушений).
