# Дельта: agent-modes-acp

- Route: Critical (significance: new_component + api_contract_change +
  significant_nfr + criticality_or_exception — run_harness несёт
  эшелонированную защиту 0.3.12)
- Created: 2026-10-03
- ADR: docs/adr/ADR-057-rezhimy-vyzova-kodovogo-agenta-headless-i-acp-s-umnym-vyborom.md
  (Accepted, вариант (а) «конфиг + auto + fallback», выбор владельца 2026-10-03)

## Проблема

Кодовый агент вызывается единственным способом — headless (промпт
позиционным аргументом / флагом / stdin, вывод — сырой текст). Idle-детект
опирается на эвристику «stderr/файлы», долгий молчащий tool-вызов внутри
агента неотличим от зависания; отмена — только SIGKILL; подключение нового
агента — зоопарк флагов. На хосте 5 из 7 агентов говорят по ACP (Agent
Client Protocol: JSON-RPC 2.0 поверх stdio — initialize/session/new/
session/prompt + стрим session/update, request_permission, cancel), но
харнесс этот протокол не использует.

## ADDED

- When у адаптера `[harnesses.<имя>]` задекларирована секция `[harnesses.<имя>.acp]`
  (binary, args, init_timeout_secs), the `run_harness` shall вызывать агента
  по ACP: initialize (client capabilities fs=false, terminal=false,
  elicitation=false), session/new (cwd = каталог прогона), session/prompt
  (задача), чтение стрима session/update; финальный текст ответа — из
  агрегата agent_message_chunk (как «stdout» для разбора JSON-контракта
  результата).
- When агент в ACP-сессии присылает `session/request_permission`, the клиент
  shall автоматически выбирать первую опцию вида allow-once и журналировать
  запрос (tool + опции) — эквивалент сегодняшнего skip-permissions/yolo;
  изменение политики допуска — вне этой дельты.
- When агент вызывает fs/terminal/elicit-методы, the клиент shall отвечать
  JSON-RPC error -32601 (method not found): capabilities объявлены false.
- When режим `mode = "auto"` (дефолт) и ACP-инициализация провалилась
  (таймаут initialize, ошибка протокола, ранний exit процесса), the
  `run_harness` shall откатиться на headless-вызов с предупреждением в
  итоге прогона (какое ACP-исключение, какая попытка); при `mode = "acp"`
  провал = ошибка прогона без отката; `mode = "prompt"` — принудительный
  headless.
- When прогон прерывается по таймауту в ACP-режиме, the клиент shall
  отправить `session/cancel` и дать агенту окно на graceful-остановку до
  SIGKILL процессной группы.
- When ACP-режим активен, the idle-детект shall учитывать события протокола
  (session/update любого вида) как активность наравне с файловой системой
  (stderr-эвристика остаётся для headless).
- Конфиг-развёртка на агентов хоста: acp-секции для claude-code
  (`claude-code-acp`), qwen-code (`qwen --acp`), kimi-code (`kimi acp`),
  openclaw (`openclaw acp`), hermes (`hermes acp`); theseus/codewhale —
  без acp-секции (headless), комментарий «ACP не поддержан (проверено
  --help 2026-10-03)».

## MODIFIED

- `CodingHarnessConfig`: новые поля `mode` (auto|acp|prompt, дефолт auto) и
  `acp: Option<AcpConfig>` — аддитивно, старые конфиги работают без правок
  (serde default). Контракт JSON-результата прогона не меняется.
- `execute_run`/`run_harness`: выбор режима до spawn; ACP-путь — новый
  модуль (src/harness/acp.rs или эквивалент) без изменения headless-пути.

## REMOVED

- Ничего.

## План отката

Дельта обратима: дефолт `auto` без acp-секций ≡ прежнее поведение;
выключение — `mode = "prompt"` или удаление acp-секций; код откатывается
`git revert` (один коммит `feat(acp)` + один `chore(config)`). Данных и
миграций нет; журнал прогонов расширяется полями режима — старые записи
читаются без изменений.

## Критерии приёмки

- [ ] Юнит-тесты: ACP-клиент против фикстуры-«агента» (python/node-скрипт
      в тестах): initialize→new→prompt→стрим→финал; request_permission
      авто-allow; -32601 на fs-метод; cancel по таймауту; fallback auto при
      провале init; mode="acp" — ошибка без отката.
- [ ] Интеграционный тест (sandbox): прогон через реальный ACP-способный
      агент хоста (минимальная задача: touch файла + JSON-контракт) в обоих
      режимах; theseus/codewhale — headless как раньше.
- [ ] Idle-детект: тест с агентом, который молча «работает» (пауза между
      session/update дольше idle_timeout) → прерывание; активный стрим не
      прерывается.
- [ ] Существующие тесты harness_run (worktree-изоляция, env_clear, пост-гейт,
      авто-коммит) зелёные в обоих режимах — режим не ослабляет изоляцию.
- [ ] Конфиг-файл хоста дополнен acp-секциями (5 агентов), theseus/codewhale
      помечены; конфиг перечитывается на каждый вызов (без рестарта сервера).
- [ ] Журнал прогона фиксирует режим (acp|prompt|fallback), версию
      claude-code-acp, количество request_permission.
- [ ] CHANGELOG: раздел «Что может покраснеть» — поведение auto/fallback.

## Уточнения протокола ACP (сверка со скиллом acp-integration, 2026-10-04)

Выжимка скилла acp-integration (agentclientprotocol.com, индекс /llms.txt)
уточняет обязательные детали клиента. Дизайн (вариант «а») не меняется —
уточняются контракты хода и завершения. Обязательны к приёмке.

### Хендшейк

- `initialize`: клиент шлёт `protocolVersion` (целое = MAJOR),
  `clientCapabilities` (fs=false, terminal=false, elicitation=false),
  `clientInfo`. Агент отвечает СВОЕЙ поддерживаемой версией; версия,
  которую клиент не поддерживает, → закрыть соединение: auto → fallback
  headless с причиной «версия протокола не согласована», mode=acp →
  ошибка прогона.
- Ответ `initialize` несёт `agentCapabilities`, `agentInfo`, `authMethods`:
  `agentInfo` (имя/версия адаптера) — в журнал прогона; непустые
  `authMethods`/cap authenticate — клиент аутентификацию не поддерживает →
  auto-fallback / ошибка (не притворяться, что «работает»).
- Пропущенная capability = UNSUPPORTED: опциональные методы (loadSession,
  resume, session/close) не зовутся без cap.

### Ход и финальный ответ

- `session/prompt`: `prompt` — массив ContentBlock:
  `[{ "type": "text", "text": <задача> }]` (baseline Text).
- Завершение хода — ОТВЕТ на `session/prompt` со `stopReason`
  ∈ `end_turn|max_tokens|max_turn_requests|refusal|cancelled` (НЕ выход
  процесса):
  - `end_turn` → ход завершён (Completed);
  - `cancelled` → прогон отменён (таймаут/abort) — не «ложная ошибка»;
  - `max_tokens` / `max_turn_requests` → ход оборван по лимиту: частичный
    ответ, предупреждение в итоге прогона — не тихий успех;
  - `refusal` → ошибка прогона (агент отказался выполнять).
- Агрегация финального текста — по `messageId`: один id = чанки одного
  сообщения, смена id = новое сообщение; финальный ответ = ПОСЛЕДНЕЕ
  агентское сообщение (уточнение формулировки ADR-057 «после последнего
  tool_call» — эвристика остаётся фолбэком при отсутствии messageId).
- `session/update` — notification: ответа нет и не ждём; неизвестные
  варианты `sessionUpdate` (`usage_update`, `current_mode_update`,
  `available_commands_update`, `config_option_update`,
  `session_info_update`, …) — журналируются и игнорируются
  (расширяемость протокола), ошибкой не считаются.

### Разрешения и отмена

- Ответ на `session/request_permission` — строго
  `{ "outcome": { "outcome": "selected", "optionId": <первая allow-опция> } }`;
  при отмене прогона с висящим permission-запросом —
  `{ "outcome": { "outcome": "cancelled" } }` (агент не висит).
- `session/cancel` — notification: после отправки клиент ПРОДОЛЖАЕТ читать
  `session/update` (tool_call_update и др.) и ждёт ответ `session/prompt`
  со `stopReason="cancelled"` в пределах graceful-окна; только потом
  SIGKILL процессной группы (лестница TERM→KILL — последний рубеж).

### Завершение сессии

- После `end_turn`: при cap `sessionCapabilities.close` → `session/close`
  (освобождение ресурсов), затем закрытие stdin → ожидание выхода
  процесса → TERM/KILL по лестнице. Без cap — сразу stdin/лестница.

### Конвенции (чек-лист ревью)

- JSON-ключи camelCase; дискриминаторы значений (`sessionUpdate`) —
  snake_case; пути абсолютные (cwd в `session/new` — канонизированный
  абсолютный путь worktree/репо, `mcpServers: []`); номера строк 1-based.
- Клиентский MUST `session/request_permission` — реализован (auto-allow +
  журнал); опциональные клиентские методы (fs/*, terminal/*,
  elicitation/*) не заявлены → -32601.

### Дополнительные критерии приёмки

- [ ] `stopReason` обработан всеми значениями: фикстуры end_turn /
      cancelled / max_tokens / refusal (отражение в итог прогона).
- [ ] Агрегация по `messageId`: фикстура с двумя агентскими сообщениями и
      tool_call между ними — финал = второе сообщение.
- [ ] Неизвестный `sessionUpdate` от фикстуры — прогон не рушится,
      событие в журнале.
- [ ] Несогласованная версия протокола (фикстура отвечает чужой MAJOR) —
      auto → fallback с причиной; mode=acp → ошибка.
- [ ] Непустые `authMethods` (фикстура) → auto-fallback / ошибка.
- [ ] Висящий `request_permission` при отмене → outcome=cancelled, агент
      не зависает (фикстура).
- [ ] Интеграционный тест живого агента: initialize → session/new →
      session/prompt → session/cancel, отчёт «что именно проверено»;
      референсный клиент среды для ручной сверки —
      `openclaw acp client --server "<cmd>" --server-args <args...>`.
- [ ] Опционально [ТРЕБУЕТ ПРОВЕРКИ]: TCK
      (github.com/agentclientprotocol/acp-tck, экспериментальный) против
      тестовой фикстуры — подтверждает, что фикстура — честный ACP-агент;
      ключи/опции — только из README репозитория.
