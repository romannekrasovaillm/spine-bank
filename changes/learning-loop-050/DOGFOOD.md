# Дог-фуд журнал (сырьё для .arch/lessons/ledger.jsonl после приземления L1)

Формат: черновые записи уроков, полученные в работе над TASK-Spine-0.5.0. После L1
каждая запись оформляется через `arch-be lessons record` (источник gate|tool), чтобы
выполнить критерий «≥ 3 confirmed урока из собственной работы» (TASK §6, дог-фуд).

## 2026-10-14 · LSN-черновик 1 · source: tool
- Факт: harness_run через адаптер claude-code (ACP mode=auto) упал на session/prompt:
  -32603 «Failed to authenticate. API Error: 403 Request not allowed» (hr-20261014-120923-00).
- Урок: при 403 от шлюза на ACP-входе переключаться на prompt-only адаптер
  (claude-code-ps, секция без .acp); держать такой вариант в config.toml постоянно.
- evidence: reports/harness/hr-20261014-120923-00.log; фикс — коммит конфига
  ~/.config/arch-harness/config.toml (вне репозитория).

## 2026-10-14 · LSN-черновик 2 · source: tool (из прогона L1, отчёт исполнителя)
- Факт: правило `rule_templates_have_teeth` вызывает `./target/debug/arch-be`
  относительным путём — при CARGO_TARGET_DIR на другой чекаут дог-фуд гейт падает
  с кодом 127. Обход — симлинк `target → <общий target>` (вне git).
- Урок-кандидат: правило обязано резолвить бинарь через CARGO_TARGET_DIR/current_exe,
  а не через относительный путь. Зафиксировать отдельным пунктом (улучшение fitness,
  не входит в L1–L11 — кандидат в BACKLOG).


(продолжение — по мере прогонов L1/L2/L3: провалы clippy, control check, тестов)

## 2026-10-14 · LSN-черновик 3 · source: tool (прогон L11, hr-20261014-135849-04)
- Факт: claude-code повис по таймауту тишины 1800 с; stderr —
  `[claude-code:unrecognized_model] {"model":"deepseek-flash","query_source":"sdk"}`.
  Корень: `~/.claude/settings.json` несёт протухшие имена моделей, и для sdk-субагентов
  они перекрывают env адаптера. L1 выжил (не звал субагентов), L11 — нет.
- Урок: env адаптера НЕ побеждает settings.json пользователя; фикс — флаг
  `--settings /home/roman/.arch-harness/claude-harness-settings.json` (приоритет
  командной строки над user-настройками) в адаптерах claude-code*.
- evidence: отчёт hr-20261014-135849-04; фикс — ~/.config/arch-harness/config.toml
  (вне репозитория) + ~/.arch-harness/claude-harness-settings.json.

## 2026-10-14 · LSN-черновик 4 · source: tool (прогон L3-завершения, hr-20261014-160020-07)
- Факт: исполнитель запустил `cargo test` ФОНОМ (task-нотификации claude) и молча ждал;
  CARGO_TARGET_DIR вне worktree → 30 мин без вывода и без изменений файлов репозитория →
  убит по таймауту тишины 1800 с, хотя работал (4,2M входных токенов). Тот же механизм
  убил первый L11.
- Урок: watchdog тишины смотрит на stdout и файлы РЕПОЗИТОРИЯ. Исполнителю запрещать
  фоновые длинные команды; приватный CARGO_TARGET_DIR класть ВНУТРЬ worktree
  (сборка сама подтверждает живость изменениями файлов).
- evidence: транскрипт сессии 058f74aa (entrypoint sdk-cli, background task
  «Run full test suite» killed, totalDuration ≈ 1800 с).

## 2026-10-14 · LSN-черновик 5 · source: tool (из прогона L2)
- Факт: общий `CARGO_TARGET_DIR=/home/roman/spine-bank/target` у параллельных
  worktree флота даёт одинаковые имена артефактов. Сборка соседнего исполнителя
  (L11, `cargo test --lib learning_metrics`) подменила тестовый бинарь, и
  `cargo test --lib gate_loop::` отрапортовал «running 0 tests» при непустом
  модуле — ложно-зелёный прогон; полный `cargo test` тем же механизмом мог бы
  пройти на бинаре чужого дерева (и `-C extra-filename` совпал: -3656f21866e5b917).
- Урок-кандидат: проверки в прогоне флота выполнять с ПРИВАТНЫМ target
  (свой на worktree), общий target — только для артефакта дог-фуд-гейта
  (`./target/debug/arch-be`); «0 tests» вместо ожидаемого числа — сигнал подмены
  бинаря, а не отсутствия тестов.
- evidence: сессия L2 2026-10-14; после `CARGO_TARGET_DIR=<приватный>` те же
  11 тестов модуля зелёные (а полный `cargo test` — 2377+).