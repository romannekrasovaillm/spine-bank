---
id: CMP-010
type: cmp
title: "Криптоконтур и ключи"
status: "designed"
implements: [CAP-001, AD-004, AD-008]
availability: 0.999
replicas: 2
rps_per_instance: 3000
instances: 2
cost_per_instance_month: 260000
exit_cost: 2100000
---

Владение ключевым материалом стыка: HSM или vault, регламентная ротация,
применение ключа только внутри контура; ключи не попадают в код, логи и
трассировки.
