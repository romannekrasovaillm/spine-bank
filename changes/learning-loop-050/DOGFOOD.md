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

(продолжение — по мере прогонов L1/L2/L3: провалы clippy, control check, тестов)
