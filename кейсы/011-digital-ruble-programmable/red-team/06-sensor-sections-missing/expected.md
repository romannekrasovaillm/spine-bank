# Ожидание: 06-sensor-sections-missing

- Что засеяно: из `docs/spec/programmable-payments.md` удалена секция `## Критерии приёмки`.
- Команда-проба: `arch-be control sensors docs/spec`
- Ожидаемая реакция: сенсор `required_sections` сообщает FAIL, команда завершается с **exit 1** (строка «Итог: FAIL — сенсоров: N, провалено: M»).
- Дополнительно: составляющая `sensors` входит в контур `gate`/`review` на маршрутах Standard/Critical — секция FAIL валит весь гейт (исторический разрыв «сенсор печатает FAIL, но exit 0 и вне контура» закрыт волной DB-гейта).
