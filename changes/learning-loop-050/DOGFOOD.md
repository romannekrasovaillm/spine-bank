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

## 2026-10-14 · LSN-черновик 3 · source: tool (из прогонов L2)
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

(продолжение — по мере прогонов L1/L2/L3: провалы clippy, control check, тестов)
