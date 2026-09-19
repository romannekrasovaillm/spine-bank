---
id: CMP-001
type: cmp
title: "Оркестратор выплат"
status: "designed"
implements: [REQ-001, REQ-002, REQ-003, AD-001]
depends_on: [CMP-002, CMP-003, CMP-004]
availability: 0.9995
replicas: 2
rps_per_instance: 250.0
instances: 4
cost_per_instance_month: 52000
exit_cost: 400000
---

Ведёт реестр от приёма до закрытия окна: распределяет строки по проверке,
отправляет поручения, фиксирует статусы. Числа карточки — вход проверок NFR:
доступность, реплики, ёмкость, стоимость и цена выхода.
