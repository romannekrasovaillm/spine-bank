---
id: NFR-009
type: nfr
title: "Лаг сверки с платформой ЦР"
status: "accepted"
verification: "метрика cr_reconcile_lag_seconds"
affects: [CMP-006, CMP-005]
---

Лаг сверки — ≤ 5 минут (p99). Превышение лага дольше допустимого окна —
основание остановить приём новых поручений.
