# Ожидание: 02-dangling-model-ref

- Что засеяно: `model/INT-006` ссылается на несуществующий контракт `docs/contracts/pcr-conditions-api-v2.md`.
- Команда-проба: `arch-be trace check .` (звено `INT → контракт`)
- Ожидаемая реакция: `[error] int-contract-missing`, exit 1. `model validate` этого не ловит — проверка контракта живёт в трассировке.
