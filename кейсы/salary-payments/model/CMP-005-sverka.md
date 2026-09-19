---
id: CMP-005
type: cmp
title: "Сверка с платформой"
status: "designed"
implements: [REQ-005, AD-007]
depends_on: [CMP-003, CMP-004]
availability: 0.999
replicas: 2
rps_per_instance: 80.0
instances: 2
cost_per_instance_month: 29000
exit_cost: 180000
---

Строит отчёт сверки по снимку журнала и данным платформы, разрешает строки в
разборе, перечисляет расхождения поимённо. Работает после окна, вне пути
выплаты: его недоступность не задерживает окно.
