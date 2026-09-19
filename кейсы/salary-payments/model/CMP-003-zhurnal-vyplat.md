---
id: CMP-003
type: cmp
title: "Журнал выплат"
status: "designed"
implements: [REQ-003, AD-002]
depends_on: [CMP-006]
availability: 0.9995
replicas: 3
rps_per_instance: 500.0
instances: 3
cost_per_instance_month: 64000
exit_cost: 900000
---

Append-only журнал: одна строка реестра — одна неизменяемая запись о переходе
состояния. Исправление — компенсирующая запись со ссылкой на исходную.
Журнал — источник отчёта сверки и доказательство при обращении получателя.
