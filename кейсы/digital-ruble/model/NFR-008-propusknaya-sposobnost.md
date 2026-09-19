---
id: NFR-008
type: nfr
title: "Пропускная способность приёма поручений"
status: "accepted"
verification: "нагрузочный прогон, метрика cr_gateway_rps"
affects: [CMP-001, CMP-002, CMP-003]
rps_target: 2400
currency: RUB
---

Пиковая нагрузка приёма поручений — ≥ 2400 rps с запасом на отказ одной
реплики каждого участка.
