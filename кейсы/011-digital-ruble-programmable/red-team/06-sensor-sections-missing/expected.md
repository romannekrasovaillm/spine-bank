# Ожидание: 06-sensor-sections-missing

- Что засеяно: из `docs/spec/programmable-payments.md` удалена секция `## Критерии приёмки`.
- Команда-проба: `arch-be control sensors docs/spec`
- Ожидаемая реакция: сенсор `required_sections` сообщает FAIL.
- **Наблюдаемый разрыв**: сенсор печатает FAIL, но не меняет exit-код (exit 0), и секции сенсоров нет в составных `gate`/`review` — дефект проходит весь контур. См. отчёт, находка R-3.
