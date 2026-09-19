---
id: NFR-011
type: nfr
title: "Latency исполнения условия"
status: "accepted"
verification: "гистограмма cr_condition_execution_duration_seconds"
affects: [CMP-011, CMP-012, INT-003, INT-001, INT-006]
p99_target_ms: 4000
---

Проверка условия и инициирование исполнения — p99 ≤ 4000 мс; бюджет
внешних обращений: каналы 400 + платформа 1500 + реестр условий 900.
