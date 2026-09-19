---
id: CMP-009
type: cmp
title: "Возвраты и разбор споров"
status: "designed"
depends_on: [CMP-002, CMP-005]
implements: [CAP-005, REQ-005, AD-005]
availability: 0.995
replicas: 2
rps_per_instance: 300
instances: 2
cost_per_instance_month: 85000
exit_cost: 300000
---

Возвраты по операциям C2B и разбор обращений: опора на журнал, состояние
UNKNOWN как отдельный сценарий, сроки и владелец по каждому обращению.
