---
id: CMP-005
type: cmp
title: "Журнал операций (append-only)"
status: "designed"
implements: [CAP-003, REQ-006, AD-002, AD-005]
availability: 0.999
replicas: 3
rps_per_instance: 1500
instances: 3
cost_per_instance_month: 190000
exit_cost: 1400000
---

Неизменяемый журнал переходов состояний и фактов отправки: источник аудита,
учётных записей, уведомлений и материала сверки.
