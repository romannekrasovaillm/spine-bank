# Ожидание: 01-rule-weakened

- Что засеяно: в корневом `CONSTRAINTS.yaml` у правила `C-001` severity понижен `error` → `warn` без override.
- Команда-проба: `arch-be gate --repo . --constraints CONSTRAINTS.yaml`
- Ожидаемая реакция: секция `rule_weakened` — FAIL, exit 1 (анти-ослабление реестра правил относительно git-базы).
