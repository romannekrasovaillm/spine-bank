---
id: INT-001
type: int
title: "Платформа цифрового рубля Банка России"
status: "accepted"
latency_budget_ms: 400.0
availability: 0.995
contract: "docs/contracts/pcr-payout.md"
---

Внешняя граница: отправка поручения и чтение статуса по ключу идемпотентности.
Режим деградации — строка в разбор, окно продолжается для остальных строк.
