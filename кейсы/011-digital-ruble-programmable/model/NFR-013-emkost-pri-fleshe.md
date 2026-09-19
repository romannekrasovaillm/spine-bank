---
id: NFR-013
type: nfr
title: "Ёмкость движка при флеш-нагрузке"
status: "accepted"
verification: "нагрузочный прогон флеш-профиля, метрика cr_condition_rps"
affects: [CMP-012]
rps_target: 5000
currency: RUB
---

Флеш-профиль (массовое срабатывание условий по расписанию) — ≥ 5000 rps на
движке условий без деградации идемпотентности.
