---
id: INT-004
type: int
title: "Антифрод-платформа банка"
status: "accepted"
latency_budget_ms: 250
availability: 0.999
contract: docs/contracts/antifraud-api.md
---

Скоринг перевода до отправки поручения: pass, step_up, stop. Недоступность
платформы не трактуется как pass — действует консервативное правило политики.
