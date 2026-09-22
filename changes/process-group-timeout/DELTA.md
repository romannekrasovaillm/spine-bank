# Дельта: таймаут fitness-правил убивает процессную группу (A1)

## Что меняется

- `src/proc.rs` — новый модуль, единая точка механики запуска shell-команд:
  `shell_command`/`shell_command_tokio` (spawn в собственной процессной
  группе, unix), `run_shell` (таймаут + читатели вывода с дедлайном),
  `kill_process_group_sync` (TERM → 1 с → KILL) и async `kill_process_group`
  (перенесена из `src/harness.rs` без изменения семантики: TERM → 10 ×
  300 мс → KILL), `drain_tail`/`MAX_CAPTURE_BYTES` (перенесены из
  `src/control.rs`).
- `src/lib.rs` — регистрация модуля `proc` (доступен и в фиче `core`;
  async-функции gated `harness` — их потребители только там).
- `src/control.rs` — `run_with_timeout` делегирует `proc::run_shell`;
  сигнатура и `CommandOutcome { status, tail }` сохранены.
- `src/rehearsal.rs` — `run_step` переведён на `shell_command` +
  `kill_process_group_sync` (вывод по-прежнему в лог-файл worktree).
- `src/rule_templates.rs` — `run_shell` делегирует `proc::run_shell`;
  хвост вывода теперь ограничен 16 КБ на поток (было — без лимита).
- `src/hooks.rs` — `run_hook` переведён на `shell_command` +
  `kill_process_group_sync` + `PipeReader` (stdout с дедлайном; лимит
  4 КБ символов на выходе сохранён, для вывода > 16 КБ теперь берётся
  хвост, а не голова).
- `src/eval.rs` — `run_command` переведён на `shell_command_tokio` +
  `kill_process_group`; ожидание читателей ограничено дедлайном
  `READER_JOIN_TIMEOUT`.
- `src/harness.rs` — импортирует `kill_process_group` из `crate::proc`.
- `CONSTRAINTS.yaml` — новое правило `C-32 no_direct_shell_spawn`
  (`must_not_contain`, critical): запрет прямого запуска bash/sh в
  `src/**/*.rs` в обход `src/proc.rs`.

## Зачем

Прежняя идиома «spawn + `child.kill()` по таймауту» убивала только
интерпретатор: внуки (`pytest` с воркерами, `./gradlew`, `server &`)
оставались сиротами и, держа унаследованные stdout/stderr, подвешивали
потоки-читатели на EOF. Замер до исправления: правило `sleep 15; true` с
`timeout_secs: 2` реально выполнялось 15,0 с; правило `(sleep 30 &); sleep 30`
подвешивало гейт бессрочно (убито внешним `timeout 10`). Гейт обязан быть
живучее проверяемого кода: таймаут должен означать таймаут.

## Чем подтверждается

- Тесты `src/proc.rs`: `timeout_kills_sleeping_command_within_grace`,
  `timeout_kills_background_grandchildren_within_grace` (оба — завершение
  < 3 с при таймауте 1 с; до исправления падали с 15,0 с и висли
  бессрочно), `timeout_leaves_no_orphan_processes` (`kill -0` по pid
  фонового потомка — процесса нет), регрессии хвоста вывода.
- Регрессии мигрированных точек: `control::command_capture_tests`,
  `hooks::tests::timeout_does_not_hang`,
  `rule_templates::tests::package_runs_the_bash_command_and_reports_code`,
  `harness::tests::timeout_kills_whole_process_group`.
- Догфуд: `arch-be control check . --constraints CONSTRAINTS.yaml` — PASS
  (включая C-32); репродукция из задания — FAIL за ≤ 3 с.

## Ссылки

Задание A1 релиза 0.3.7; AD-9 (гейт, а не документация задним числом);
комментарий о разделителе `--` у procps kill — `src/proc.rs`.
