# Реестр уроков контура (Lesson Ledger)

> Файл собран детерминированно из `.arch/lessons/ledger.jsonl` (ADR-085):
> правьте журнал событий, а не этот файл. Уроков в статусе `retired`
> в блок инъекции не попадает. Сортировка — по `id`.

Уроков: 6 (observed 0, candidate 1, confirmed 5, promoted 0, retired 0)

## LSN-1e36136a — tool · unrecognized-model · harness_launch · confirmed

- **Правило:** (формулировка не зафиксирована — статус от текста не зависит)
- **Область:** repo
- **Счётчики:** hits 2 · misses 0 (подряд 0) · fails 1 · задач 3
- **Провенанс:** записал cli · arch-be 0.4.11 · sha256 вердикта 3ce4ee4958d4
- **Доказательства:**
  - 2026-10-15T00:03:20.851+03:00 · fail · l11-135849 · claude-code-ps · `unrecognized_model deepseek-flash (sdk) — тишина 1800с`
  - 2026-10-15T00:03:20.890+03:00 · pass_after_apply · l5-211904 · claude-code-ps
  - 2026-10-15T00:03:20.928+03:00 · pass_after_apply · l9-223414 · claude-code-ps

## LSN-20878f67 — review · log-scan-first-match · verification · candidate

- **Правило:** При разборе логов тестов искать ВСЕ вхождения FAILED/failures:/error[, а не первое: grep -E 'FAILED|failures:|error\[' по всему логу, затем сводка 'test result' по каждому сьюту
- **Область:** repo
- **Счётчики:** hits 1 · misses 0 (подряд 0) · fails 1 · задач 2
- **Провенанс:** записал cli · arch-be 0.4.11 · sha256 вердикта 0acdcc31b13f
- **Доказательства:**
  - 2026-10-15T01:00:02.382+03:00 · fail · orchestrator-wave3 · orchestrator · `греп лога по паттерну первой находки (cli) — пропущен регресс mcp_serve; нашёлся пост-гейтом BENCH-1`
  - 2026-10-15T01:00:02.403+03:00 · pass_after_apply · fix2-dispatch · orchestrator · `FIX-2 сформирован по полному списку failures из лога пост-гейта`

## LSN-4f62b693 — tool · shared-cargo-target · fleet_run · confirmed

- **Правило:** (формулировка не зафиксирована — статус от текста не зависит)
- **Область:** repo
- **Счётчики:** hits 2 · misses 0 (подряд 0) · fails 1 · задач 3
- **Провенанс:** записал cli · arch-be 0.4.11 · sha256 вердикта 7098ad2daece
- **Доказательства:**
  - 2026-10-15T00:03:20.747+03:00 · fail · l2-135701 · claude-code-ps · `running 0 tests при непустом модуле — подмена бинаря соседним прогоном`
  - 2026-10-15T00:03:20.787+03:00 · pass_after_apply · l3fin-163548 · claude-code-ps
  - 2026-10-15T00:03:20.818+03:00 · pass_after_apply · l7-211938 · claude-code-ps

## LSN-6bf06d5e — gate · C-33 · config_growth · confirmed

- **Правило:** Следи за лимитом C-33 (3000 строк) при добавлении полей в config.rs; декомпозиция на подмодули заранее (src/config/{lessons,models}.rs)
- **Область:** repo
- **Счётчики:** hits 2 · misses 0 (подряд 0) · fails 1 · задач 3
- **Провенанс:** записал cli · arch-be 0.4.11 · sha256 вердикта 6c8c4f70fd17
- **Доказательства:**
  - 2026-10-15T00:03:20.562+03:00 · fail · merge-L1L2L3-1845 · claude-code-ps · src/config.rs · `prod_file_length_limit: src/config.rs 3020 > 3000`
  - 2026-10-15T00:03:20.600+03:00 · pass_after_apply · cfg1-184739 · claude-code-ps · `check_file_length.sh зелёный после декомпозиции`
  - 2026-10-15T00:03:20.632+03:00 · pass_after_apply · merge-wave23 · orchestrator · `C-33 зелёный после мерджей L4-L10`

## LSN-c5dc9860 — fitness · rule_templates_have_teeth · new_worktree · confirmed

- **Правило:** (формулировка не зафиксирована — статус от текста не зависит)
- **Область:** repo
- **Счётчики:** hits 2 · misses 0 (подряд 0) · fails 1 · задач 3
- **Провенанс:** записал cli · arch-be 0.4.11 · sha256 вердикта f36eb7ecb8db
- **Доказательства:**
  - 2026-10-15T00:03:20.659+03:00 · fail · l1-121311 · claude-code-ps · `код 127: ./target/debug/arch-be не найден при CARGO_TARGET_DIR`
  - 2026-10-15T00:03:20.691+03:00 · pass_after_apply · cfg1-184739 · claude-code-ps · `после фикса ./target — зелёный`
  - 2026-10-15T00:03:20.718+03:00 · pass_after_apply · orchestrator-verify · orchestrator · `verify --all зелёный с приватным target`

## LSN-e8a96219 — tool · background-task-silence · harness_run · confirmed

- **Правило:** (формулировка не зафиксирована — статус от текста не зависит)
- **Область:** repo
- **Счётчики:** hits 2 · misses 0 (подряд 0) · fails 1 · задач 3
- **Провенанс:** записал cli · arch-be 0.4.11 · sha256 вердикта 8476a6b5b556
- **Доказательства:**
  - 2026-10-15T00:03:20.962+03:00 · fail · l3z-160020 · claude-code-ps · `фоновый cargo test + 30 мин тишины → kill по idle`
  - 2026-10-15T00:03:20.991+03:00 · pass_after_apply · l3fin-163548 · claude-code-ps
  - 2026-10-15T00:03:21.023+03:00 · pass_after_apply · l10-223431 · claude-code-ps

