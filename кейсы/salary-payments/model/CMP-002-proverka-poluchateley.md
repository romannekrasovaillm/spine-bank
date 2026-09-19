---
id: CMP-002
type: cmp
title: "Проверка получателей и лимитов окна"
status: "designed"
implements: [REQ-001, AD-005]
depends_on: [CMP-003]
availability: 0.999
replicas: 2
rps_per_instance: 300.0
instances: 3
cost_per_instance_month: 38000
exit_cost: 250000
---

Проверяет получателя, стоп-лист и лимит окна до отправки поручения. Никакие
персональные данные не покидают этот компонент в журнал: наружу уходят только
идентификаторы и признак «допущен».
