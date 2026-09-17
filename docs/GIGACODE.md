# GigaCode CLI + Spine: быстрое развёртывание силами самого агента

Эта инструкция — для архитектора, который уже **склонировал репозиторий
Spine локально** и хочет, чтобы **агент GigaCode сам всё развернул**:
MCP-сервер, скиллы, хуки-гейты. GigaCode CLI — форк Qwen Code, поэтому все
шаги проверены живьём на qwen-code (0.0.5 и 0.24.0).

> Что получится в итоге: агент GigaCode сможет вызывать 20 детерминированных
> инструментов Spine (fitness-гейты, трасса, модель, контракты, рубрики),
> видеть 55 архитектурных скиллов и останавливаться на красном гейте.

## Часть 1. Для архитектора (что происходит)

1. GigaCode ставит/собирает бинарь `arch-be` (из релиза или из вашего клона).
2. Регистрирует MCP-сервер `spine` в проекте (`.qwen/settings.json`).
3. Раскладывает 55 скиллов Spine в `.qwen/skills/` проекта.
4. Включает хук-гейт (SessionEnd → `arch-be control check .`).
5. Проверяет: вызывает `fitness_check` и докладывает вердикт.

Всё это делает сам агент — вы только выдаёте ему промпт из части 2.

## Часть 2. Промпт для агента GigaCode

Откройте GigaCode в корне вашего проекта и вставьте:

```text
Разверни Spine (arch-be) в этом проекте по следующей инструкции.
Репозиторий Spine уже склонирован локально: <ПУТЬ_К_КЛОНУ, напр. ~/spine-bank>

1. Бинарь:
   - Если `arch-be` есть в PATH (`command -v arch-be`) — используй его.
   - ВАРИАНТ А (быстрый, готовый бинарь из GitHub Releases):
     curl -L -o arch-be https://github.com/romannekrasovaillm/spine-bank/releases/latest/download/arch-be-core-linux-x86_64
     && chmod +x arch-be && mv arch-be ~/.local/bin/
     (создай ~/.local/bin при нужде; сверь sha256 с SHA256SUMS.txt из релиза).
   - ВАРИАНТ Б (сборка из локального клона): cd <ПУТЬ_К_КЛОНУ> &&
     cargo build --release --no-default-features --features core && скопируй
     target/release/arch-be в ~/.local/bin/.
   - Проверь: `arch-be --version` (ожидается 0.2.x).

2. MCP-сервер (project-level, НЕ затирай существующие ключи файла — мердж):
   - Предпочтительно: выполни `arch-be connect qwen` в корне проекта.
   - Или вручную в .qwen/settings.json добавь:
     {"mcpServers": {"spine": {"command": "arch-be", "args": ["mcp", "serve"]}}}
   - Одобри сервер: `qwen mcp approve spine` (в 0.24 project-серверы требуют
     одобрения) — или подтверди диалог при следующем интерактивном запуске.

3. Скиллы (55 шт.): скопируй из клона каталоги
   assets/plugins/*/skills/*/  в  .qwen/skills/<имя>/SKILL.md
   (структура: .qwen/skills/saga-transactions/SKILL.md и т.д.).
   Не копируй файлы больше 200 КБ — доложи о пропущенных.

4. Хук-гейт (если поддерживается версией): в .qwen/settings.json добавь
   "hooks": {"SessionEnd": [{"hooks": [{"type": "command", "command":
   "arch-be control check . 2>&1 | tail -3"}]}]}
   Если ключ hooks не поддерживается твоей версией — пропусти и доложи это.

5. Проверка (обязательна):
   - Вызови MCP-инструмент spine fitness_check с {"repo": "."} и доложи
     вердикт (passed true/false, число находок).
   - Вызови skill_search с {"query": "saga"} и перечисли 3 найденных скилла.
   - Вызови model_query с {"dir": "model"} (если каталога model/ нет —
     скажи об этом, это не ошибка).

6. Финал: доложи одной сводкой — версия arch-be, статус MCP (Connected),
   число разложенных скиллов, статус хука, вердикт fitness_check.
   Ничего не коммить в git. Секреты/ключи не выводи.
```

## Часть 2.5. Наполнение пустого проекта (банковской зоны нет — наполняем локально)

В публичном форке нет слоя `banking/` — спайн, правила, модель и базу знаний
проекта архитектор создаёт под себя. Это тоже делает агент по промпту:

```text
Наполни Spine-контур этого проекта с нуля (шаблоны — в клоне Spine:
кейсы/*/handoff-example/, assets/rubrics/):

1. ARCHITECTURE-SPINE.md — 2-4 инварианта проекта, формат
   «## AD-N: <название>» + Binds/Prevents/Rule.
2. .arch-handoff/CONSTRAINTS.yaml — fitness-правила (must_contain /
   must_not_contain / command_succeeds), и копию в корень (для trace_check).
3. model/ — сущности с frontmatter: SYS-001 (система), REQ-001, NFR-001
   (verified_by: [C-NNN]), CMP-001 (implements: [...]), AD-1… (affects,
   verified_by) — чтобы trace_check давал PASS.
4. knowledge/ — заметки/стандарты проекта (.md); зарегистрируй их в
   arch-harness.toml: [knowledge] dirs = ["knowledge"].
   ВАЖНО: kb_search — BM25 по дословным токенам: пиши ключевые термины
   в тексте явно (русский и английский вариант термина).
5. docs/adr/ — первый ADR (прозой или через adr_new, если сервер в --rw).
6. Проверка: fitness_check, trace_check, kb_search, model_query —
   доложи вердикты.
```

Проверено живьём (qwen-code): агент сам создал спайн, правила и
knowledge-документ; `fitness_check` сразу начал ловить отсутствие кода
(FAIL по `tests_present` — правильно, src/ ещё не было), `kb_search` нашёл
документ по точному термину.

## Часть 3. Что дальше — работа архитектора

После развёртывания просто просите агента, например:

- «Проверь проект через spine `fitness_check`; если FAIL — исправь и
  перепроверь» (агент сам чинит нарушения CONSTRAINTS.yaml);
- «Оцени маршрут изменения: `significance_score` с триггером
  `new_component=true`»;
- «Покажи модель: `model_query` по `model/`»; «Проверь трассировку:
  `trace_check`»;
- «Оцени ADR по рубрике: `rubric_prompt` → дай 2–3 ответа судьи →
  `rubric_verify`» (split-judge — судит сам GigaCode, ключей Spine не нужно);
- «Проверь контракты: `openapi_lint`, `contract_diff`»;
- «Визуализируй архитектуру: напиши Archify IR и провалидируй
  `archify_validate`, потом `archify_show`» (настройка — ниже).

### Визуализация архитектуры (Archify) из харнесса

1. Движок вендорен в репо Spine: `vendor/archify/` (нужен Node.js ≥ 18).
2. Одна строка в `arch-harness.toml` проекта:

   ```toml
   [archify]
   cli_path = "<путь-к-клону-spine>/vendor/archify/bin/archify.mjs"
   ```

3. Дальше агент сам: пишет JSON IR (`diagrams/*.architecture.json`),
   валидирует через `archify_validate` (9 проверок + supportedFixes для
   точечного ремонта), показывает через `archify_show` — интерактивный HTML
   (guided views, легенда, карточки потоков). Пример результата:
   `docs/screenshots/harnesses/qwen-archify-html.png`.

Полная матрица проверенных харнессов и ограничения — в
[docs/HARNESSES.md](HARNESSES.md). Подробное подключение всех хостов —
в [docs/CONNECT.md](CONNECT.md).

## Если что-то пошло не так

| Симптом | Лечение |
|---|---|
| `spine` в статусе Pending approval | `qwen mcp approve spine` (или интерактивный запуск и одобрение) |
| Агент «не видит» инструменты | `qwen mcp list` → должен быть `Connected`; проверьте `command -v arch-be` |
| 404 по модели | Для openai-совместимого бэкенда модель задаётся через `OPENAI_MODEL`, не флагом `-m` |
| Хук не срабатывает | Версия GigaCode может не иметь hooks — проверьте `qwen hooks` / документацию своей сборки |
