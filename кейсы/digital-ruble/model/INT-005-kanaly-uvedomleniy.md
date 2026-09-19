---
id: INT-005
type: int
title: "Каналы уведомлений (пуш, SMS)"
status: "accepted"
latency_budget_ms: 150
availability: 0.99
contract: docs/contracts/notification-gateway.md
---

Доставка уведомлений клиенту. Вне критического пути исполнения: сбой доставки
не меняет состояние операции и восстанавливается повторной обработкой журнала.
