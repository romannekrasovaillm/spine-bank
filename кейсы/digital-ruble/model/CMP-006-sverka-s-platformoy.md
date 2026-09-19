---
id: CMP-006
type: cmp
title: "Сверка с платформой цифрового рубля"
status: "designed"
depends_on: [CMP-003, CMP-005]
implements: [CAP-003, REQ-006, AD-001, AD-007]
availability: 0.995
replicas: 2
rps_per_instance: 200
instances: 2
cost_per_instance_month: 95000
exit_cost: 400000
---

Сопоставляет журнал с выписками платформы с измеримым лагом, разрешает
операции в UNKNOWN, классифицирует расхождения и ставит их в очередь разбора
без автоматической правки.
