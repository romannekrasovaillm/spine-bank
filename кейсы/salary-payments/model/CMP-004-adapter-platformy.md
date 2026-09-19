---
id: CMP-004
type: cmp
title: "Адаптер платформы цифрового рубля"
status: "designed"
implements: [REQ-003, REQ-004, AD-003]
depends_on: [CMP-003]
availability: 0.999
replicas: 2
rps_per_instance: 150.0
instances: 3
cost_per_instance_month: 41000
exit_cost: 300000
---

Изолирует протокол платформы: идемпотентная отправка поручения, чтение статуса
по ключу, различение «отказ» и «неизвестно». Таймаут платформы не превращается
в повторную отправку — только в состояние разбора.
