---
id: CMP-011
type: cmp
title: "Сервис условий программируемых платежей"
status: "designed"
depends_on: [CMP-007]
implements: [CAP-006, REQ-007, REQ-009, AD-011, AD-012]
availability: 0.999
replicas: 3
rps_per_instance: 700
instances: 3
cost_per_instance_month: 120000
exit_cost: 500000
---

Публикация версий условия, подтверждение сторонами, неизменяемость
опубликованной версии, хранение всех редакций для разбора спора.
