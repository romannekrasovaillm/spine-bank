# ADR-092. Переносимый резолв бинаря в command_succeeds: ${CARGO_TARGET_DIR:-./target} вместо жёсткого ./target (правило rule_templates_have_teeth)

- Date: 2026-10-14
- Status: Accepted
- Модель-автор: spine-arch (оркестратор), по дог-фуд уроку №2 волны 0.5.0

## Context

Правило `rule_templates_have_teeth` (CONSTRAINTS.yaml) вызывало бинарь жёстким
относительным путём `./target/debug/arch-be`. В флоте параллельных исполнителей
0.5.0 каждый агент собирает в приватный `CARGO_TARGET_DIR` (дог-фуд урок №5:
общий target даёт подмену бинарей и ложное «running 0 tests») — и правило падало
с кодом 127 не из-за беззубых шаблонов, а из-за отсутствия файла (ложный
FAIL гейта, зафиксировано в дог-фуд уроке №2 волны learning-loop-050).

## Decision

Команда правила резолвит бинарь переносимо:
`${CARGO_TARGET_DIR:-./target}/debug/arch-be rules template verify --all --require-python`.
Без `CARGO_TARGET_DIR` поведение байт-в-байт прежнее (CI, одиночная разработка);
с приватным target-каталогом — бинарь находится там.

## Alternatives Considered

| Вариант | Плюсы | Минусы |
|---------|-------|--------|
| **A. ${CARGO_TARGET_DIR:-./target} (выбран)** | Нулевая стоимость, обратная совместимость | Требует shell-экспансии в command_succeeds (она есть — команды идут через sh) |
| B. Симлинк `target` в каждом worktree | Ноль правок реестра | Операционный ритуал, забывается, не коммитится — хрупко (именно так и падало) |
| C. Резолв через current_exe внутри движка правил | Канонически правильно | Правка движка правил ради одной команды; остальные command_succeeds не выигрывают |
| D. Статус-кво | — | Ложный FAIL на каждом флот-прогоне |

## Consequences

### Positive

- Дог-фуд гейт работает из любого CARGO_TARGET_DIR; урок №2 закрыт на уровне правила.

### Negative

- Изменение тела error-правила включает страж `rule_weakened` (warn) — гасится
  override с этим ADR и сроком пересмотра; само срабатывание стража — штатная
  работа дисциплины (изменение правила видимо и формализовано).

## Reversibility

reversible: возврат команды к `./target/...` — одна строка (со сработкой стража).

## References

- changes/learning-loop-050/DOGFOOD.md (уроки №2, №5); ADR-085 (контур уроков);
  CI dogfood-джоба (.github/workflows/ci.yml).
