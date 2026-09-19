---
id: CMP-004
type: cmp
title: "Реестр кошельков и связь с клиентом"
status: "designed"
implements: [CAP-002, REQ-001, REQ-002, AD-010]
availability: 0.999
replicas: 3
rps_per_instance: 700
instances: 3
cost_per_instance_month: 110000
exit_cost: 600000
---

Хранит связь клиента с кошельком и минимум персональных данных; проекция
остатка помечена как производная и несёт метку свежести.
