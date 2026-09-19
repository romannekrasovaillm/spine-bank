---
id: CMP-007
type: cmp
title: "Лимиты и антифрод-мост"
status: "designed"
implements: [CAP-004, REQ-003, REQ-004, AD-009]
availability: 0.999
replicas: 2
rps_per_instance: 1200
instances: 3
cost_per_instance_month: 105000
exit_cost: 350000
---

Проверка лимитов банка и платформы до исполнения, вызов антифрод-скоринга,
блокирующее действие решения stop, журналирование сработавших правил.
