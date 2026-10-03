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
