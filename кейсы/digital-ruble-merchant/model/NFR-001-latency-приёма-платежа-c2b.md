---
id: NFR-001
type: nfr
title: "Latency приёма платежа C2B"
status: "accepted"
verification: "гистограмма merchant_accept_duration_seconds"
affects: [CMP-001, CMP-004, INT-001, INT-003]
p99_target_ms: 1500.0
---

Ответ ТСП на приём поручения: p99 ≤ 1500 мс при доступной платформе. Раздел описывает решение и его границы: какие варианты рассмотрены, почему выбран этот, чем платим за выбор и что произойдёт при отказе. 
