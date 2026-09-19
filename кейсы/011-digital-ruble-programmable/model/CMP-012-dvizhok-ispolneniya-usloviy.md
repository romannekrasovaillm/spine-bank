---
id: CMP-012
type: cmp
title: "Движок исполнения условий"
status: "designed"
depends_on: [CMP-003, CMP-005, CMP-011]
implements: [CAP-006, REQ-008, REQ-009, AD-011, AD-014]
availability: 0.999
replicas: 2
rps_per_instance: 1600
instances: 5
cost_per_instance_month: 210000
exit_cost: 900000
---

Проверка выполнения условия, инициирование поручения в ПЦР, различение
подтверждённого, отклонённого и неопределённого исхода; состояние условия не
подменяет состояние операции.
