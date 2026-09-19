---
id: CMP-008
type: cmp
title: "Нотификации клиента"
status: "designed"
depends_on: [CMP-005]
implements: [CAP-001, REQ-003, AD-006]
availability: 0.99
replicas: 2
rps_per_instance: 2000
instances: 2
cost_per_instance_month: 60000
exit_cost: 150000
---

Уведомления о подтверждении, отказе и неопределённом исходе; источник
событий — журнал, повторная обработка восстанавливает пропущенные сообщения.
