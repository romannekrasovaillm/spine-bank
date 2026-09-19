---
id: ADR-002
type: adr
title: "Идемпотентность поручений на каждой точке входа"
status: "Accepted"
date: 2026-09-19
affects: [CMP-001, CMP-002]
implements: [AD-003]
---

Idempotency-Key от канала, дедупликация в одной транзакции с эффектом, повтор
возвращает первый результат.
