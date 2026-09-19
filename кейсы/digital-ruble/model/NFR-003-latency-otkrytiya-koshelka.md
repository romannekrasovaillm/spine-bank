---
id: NFR-003
type: nfr
title: "Latency открытия кошелька ЦР"
status: "accepted"
verification: "гистограмма cr_wallet_open_duration_seconds"
affects: [CMP-001, CMP-004, INT-003, INT-001, INT-002]
p99_target_ms: 5000
---

Открытие кошелька включает обращение к платформе и сведения из учётного ядра:
p99 ≤ 5000 мс.
