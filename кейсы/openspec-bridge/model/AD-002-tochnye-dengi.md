---
id: AD-002
type: ad
title: "Денежные суммы — в minor units, без плавающей точки"
status: "ADOPTED"
affects: [CMP-001, CMP-002]
verified_by: [C-001]
load_bearing: true
---

- **Binds**: Приём платежей, Движок лимитов
- **Prevents**: дрейф округления в денежном пути
- **Rule**: суммы хранятся и сравниваются целыми копейками; `float` для денег
  запрещён
