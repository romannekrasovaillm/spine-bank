---
id: CMP-003
type: cmp
title: "Адаптер платформы цифрового рубля"
status: "designed"
depends_on: [CMP-010]
implements: [CAP-003, REQ-006, AD-001, AD-004]
availability: 0.999
replicas: 2
rps_per_instance: 1000
instances: 3
cost_per_instance_month: 150000
exit_cost: 1500000
---

Единственная точка обмена с платформой ЦР: нормализация ответов, единая
стратегия таймаутов, различение трёх исходов, владение контуром ключей стыка.
