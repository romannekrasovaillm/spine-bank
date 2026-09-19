---
id: NFR-002
type: nfr
title: "Latency оплаты C2B по QR"
status: "accepted"
verification: "гистограмма cr_qr_payment_duration_seconds"
affects: [CMP-001, CMP-002, CMP-003, INT-003, INT-001]
p99_target_ms: 3000
---

Операция оплаты по QR в точке продаж: p99 ≤ 3000 мс, включая вызов платформы
(бюджет 1500 мс) и канал (400 мс).
