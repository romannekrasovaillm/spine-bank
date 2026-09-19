---
id: NFR-004
type: nfr
title: "Доступность приёма поручений через каналы"
status: "accepted"
verification: "доля успешных запросов шлюза, метрика cr_gateway_requests_total"
affects: [CMP-001, CMP-002]
availability_target: 0.995
---

Доступность собственного контура приёма поручений: ≥ 0,995. Внешние
зависимости учитываются отдельно в NFR-005.
