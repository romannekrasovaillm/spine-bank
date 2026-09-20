---
name: spine-content-bootstrap
description: Наполнение проекта контентом Spine с нуля (без готовой библиотеки) — спайн инвариантов, CONSTRAINTS.yaml, модель сущностей, исполняемые проверки инвариантов шаблонами правил, база знаний, первый ADR. Используй этот навык, когда проект пустой и Spine «не находит» ничего, при вопросах «как завести спайн», «наполни CONSTRAINTS/model/knowledge», «создай архитектурный контур с нуля», «поставь проверку на инвариант».
---

# Наполнение пустого проекта (спайн с нуля)

Порядок (каждый шаг проверяется инструментом):

1. **ARCHITECTURE-SPINE.md** — 2–4 инварианта, формат строго
   `## AD-N: <название>` + тело с Binds/Prevents/Rule. Проверка:
   `spine_lint {"path": "ARCHITECTURE-SPINE.md"}`.
2. **.arch-handoff/CONSTRAINTS.yaml** — правила из инвариантов (и копия в
   корень для `trace_check`):
   ```yaml
   constraints:
     - id: C-01
       name: no_float_for_money
       type: must_not_contain
       glob: "src/**/*.rs"
       pattern: "\\bf64\\b"
       severity: critical
   ```
   Проверка: `fitness_check {"repo": "."}` — FAIL по коду на этом этапе
   НОРМАЛЬНО (гейт уже сторожит).
3. **model/** — сущности с frontmatter: `SYS-001` (type: sys),
   `REQ-001` (req), `NFR-001` (nfr, `verified_by: [C-NNN]`),
   `CMP-001` (cmp, `implements: [REQ-001, AD-1]`), `AD-1` (ad,
   `affects: [CMP-001]`, `verified_by: [C-01]`). Проверка:
   `trace_check {"case": "."}` → PASS; `model_query {"dir": "model"}`.
4. **Исполняемая проверка на каждый несущий инвариант** — пока инвариант
   подтверждён только `must_contain`-правилом, он зеленеет и на упоминании.
   По каждому несущему (и вообще не покрытому поведением) инварианту:
   `arch-be rules suggest .` (MCP `rules_suggest`) → кандидат
   `executable-invariant:AD-N`; затем шаблон
   `arch-be rules template apply <id> --ad AD-N --dir .` — файлы теста, фейка и
   эталона лягут в `skeleton/rule_templates/<id>/`, фрагмент правила со
   свободным `C-NNN` и строкой `verified_by` команда **печатает**: внеси его
   дельтой (`arch-be delta new <name>`), защищённые файлы руками не правим.
   Проверка: `arch-be rules template verify --dir .` — тест обязан падать на
   нарушающей реализации (иначе `executable_rule_toothless`: проверка без зубов
   хуже текстового правила). Пометьте несущие инварианты полем
   `load_bearing: true` во frontmatter — тогда детектор называет их первыми.
5. **knowledge/** — заметки/стандарты проекта (.md) + регистрация в
   `arch-harness.toml`: `[knowledge] dirs = ["knowledge"]`.
   ВАЖНО: `kb_search` — BM25 по дословным токенам: ключевые термины пиши в
   тексте явно (и по-русски, и латиницей: «идемпотентность
   (Idempotency-Key)»). Проверка: `kb_search {"query": "<термин>"}`.
6. **docs/adr/ADR-001-….md** — первый ADR (прозой; через `adr_new` — если
   сервер в `--rw`). Оценить: скилл `spine-adr-judge`.
7. Опционально: `.arch-handoff/RUBRIC.yaml` — своя рубрика проекта
   (образец — `assets/rubrics/` в репо Spine).

Финал — доклад архитектору: что создано, вердикты fitness/trace/kb, что
осознанно оставлено пустым.

## Правило на упоминание — звено трассировки, а не проверка смысла

`must_contain` / `must_not_contain` доказывают, что в документе **написано**
нужное слово. Это ценное звено трассировки (REQ → NFR → AD → CMP → правило),
но оно ничего не говорит о том, что система **делает**: правило зеленеет и
когда инвариант соблюдён, и когда о нём просто упомянули.

Настоящая проверка поведения — `command_succeeds` (тест, линтер, скрипт),
`dependency_direction` / `context_boundary` / `archunit` (структурные гейты).
Держите хотя бы одно исполняемое правило на инвариант, который может быть
нарушен кодом: иначе нарушение найдёт только человек на ревью. Долю таких
правил в реестре печатает `arch-be control rules-report` строкой «Проверяют
поведение: N из M», а `rules-suggest` предлагает кандидата
`executable-invariants`, если в проекте есть тесты, но ни одного
`command_succeeds` нет.

