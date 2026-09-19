---
id: CMP-013
type: cmp
title: "Арбитраж по спорным исполнениям"
status: "designed"
depends_on: [CMP-005, CMP-011]
implements: [CAP-007, REQ-010, AD-013]
availability: 0.995
replicas: 2
rps_per_instance: 200
instances: 2
cost_per_instance_month: 90000
exit_cost: 250000
---

Приостановка платежа в споре, разбор с опорой на версию условия и журнал
операций, решение человека с обязательной фиксацией оснований.
