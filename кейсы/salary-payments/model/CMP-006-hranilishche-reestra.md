---
id: CMP-006
type: cmp
title: "Хранилище реестров и снимков журнала"
status: "designed"
implements: [REQ-001, AD-002]
availability: 0.9999
replicas: 3
rps_per_instance: 400.0
instances: 3
cost_per_instance_month: 33000
exit_cost: 150000
---

Хранит принятые реестры и снимки журнала на момент закрытия окна. Снимок —
основание отчёта сверки: он неизменяем, поэтому отчёт воспроизводим.
