---
id: NFR-001
type: nfr
title: "Latency перевода C2C"
status: "accepted"
verification: "гистограмма cr_transfer_duration_seconds"
affects: [CMP-001, CMP-002, CMP-003, INT-003, INT-001, INT-004]
p99_target_ms: 3000
---

Ответ клиенту об исполнении или о неопределённом исходе — p99 ≤ 3000 мс.
Бюджет hop'ов: канал 400 мс, платформа 1500 мс, антифрод 250 мс, остальное —
внутренние компоненты.
