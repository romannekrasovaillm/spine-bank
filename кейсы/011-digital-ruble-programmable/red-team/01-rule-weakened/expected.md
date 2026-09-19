# Ожидание: 01-rule-weakened

- Что засеяно: в `.arch-handoff/CONSTRAINTS.yaml` у правила `C-001` severity понижен `error` → `warn` без override.
- Команда-проба: `arch-be gate --repo .` (путь ограничений по умолчанию)
- Ожидаемая реакция: секция `rule_weakened` — FAIL, exit 1.
- Замечание: с явным `--constraints` (в т.ч. абсолютным) проверка уходит в SKIP — см. отчёт.
