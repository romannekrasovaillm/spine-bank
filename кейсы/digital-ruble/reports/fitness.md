# Отчёт фитнес-функций

Сгенерирован прогоном гейтов (все ключи моделей вырезаны из окружения).
Дата: 2026-09-19T07:41:07+03:00

```text
$ arch-be control spine ARCHITECTURE-SPINE.md
spine: нарушений нет
exit=0

$ arch-be control check . --constraints CONSTRAINTS.yaml
Правил: 15, нарушений: 0 (error: 0, warn: 0)
Итог: PASS
exit=0

$ arch-be model validate model
Сущностей: 65, находок: 0 (error: 0, warn: 0)
Итог: PASS
exit=0

$ arch-be trace check .
# Трассируемость: .

Сущностей: 65; правил CONSTRAINTS.yaml: 15; инвариантов spine: 10.

| Звено | Покрыто | Доля | Сироты |
|---|---|---|---|
| REQ → дизайн | 6/6 | 100% | — |
| NFR → дизайн | 10/10 | 100% | — |
| AD → fitness-правило | 10/10 | 100% | — |
| ADR → CMP | 9/9 | 100% | — |
| CMP → fitness | 10/10 | 100% | — |
| INT → контракт | 5/5 | 100% | — |

Итог: PASS (error: 0, warn: 0)
exit=0

$ arch-be nfr budget .
## Находки

- [warn] budget-hop-uncovered: INT-005: бюджет заявлен, но ни одна цель p99 его не проверяет

Итог: PASS (целей: 3, error: 0, warn: 1)
exit=0
$ arch-be nfr availability .
| NFR-007 | — | 60 |

Итог: PASS (SLA: 2, error: 0, warn: 0)
exit=0
$ arch-be nfr capacity .


Итог: PASS (целей: 1, error: 0, warn: 0)
exit=0
$ arch-be nfr cost .
Цена выхода: 8900000 RUB (19% годового TCO)

Итог: PASS (позиций: 10, error: 0, warn: 0)
exit=0

$ arch-be control sensors docs/spec
  [PASS] required_sections docs/spec/limits-and-antifraud.md — все обязательные секции на месте
  [PASS] upstream_coverage docs/spec/limits-and-antifraud.md — все ссылки валидны (0)
  [PASS] required_sections docs/spec/operation-state-machine.md — все обязательные секции на месте
  [PASS] upstream_coverage docs/spec/operation-state-machine.md — все ссылки валидны (0)
exit=0
```
