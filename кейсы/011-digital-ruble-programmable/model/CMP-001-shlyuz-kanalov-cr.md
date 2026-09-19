---
id: CMP-001
type: cmp
title: "Шлюз каналов цифрового рубля"
status: "designed"
depends_on: [CMP-002, CMP-007]
implements: [CAP-001, REQ-001, REQ-003, REQ-004, AD-003, AD-008, AD-009]
availability: 0.999
replicas: 3
rps_per_instance: 800
instances: 4
cost_per_instance_month: 145000
exit_cost: 900000
---

Единая точка входа для мобильного банка и интернет-банка: аутентификация
запроса, требование Idempotency-Key, схема валидации, режим деградации
(отказ изменяющих методов при недоступности платформы).
