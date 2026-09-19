---
id: CMP-002
type: cmp
title: "Оркестратор операций цифрового рубля"
status: "designed"
depends_on: [CMP-003, CMP-005]
implements: [CAP-001, REQ-003, REQ-004, AD-001, AD-005]
availability: 0.999
replicas: 3
rps_per_instance: 900
instances: 4
cost_per_instance_month: 165000
exit_cost: 1200000
---

Ведёт операцию по конечной машине состояний, включая обязательное состояние
UNKNOWN; фиксирует намерение отправки в журнал до вызова платформы; не
разрешает исход UNKNOWN без подтверждения.
