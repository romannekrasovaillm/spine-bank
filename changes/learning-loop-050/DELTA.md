# Дельта: learning-loop-050
- Route: Critical (significance 7: new_component, new_datastore, api_contract_change, consistency_model_change, security_boundary_change, cross_domain_integration, significant_nfr). Человеческое решение A3 — дано TASK-Spine-0.5.0-learning-loop.md (пакет с порогами и [РЕШЕНИЕ ЧЕЛОВЕКА]-точками).
- Created: 2026-10-14
- Источник истины: `TASK-Spine-0.5.0-learning-loop.md` (корень репозитория). План: `changes/learning-loop-050/PLAN.md`. Пререгистрация: `benchmarks/icl-transfer/PREREGISTRATION.md`.

## Проблема

Живые прогоны 0.4.10 (36 сессий, 108 запусков исполнителя) показали: обучение в контексте на уровне оркестратора существует (0/17 повторов скрытой ошибки с контекстом против 8/8 без), но держится на свободном тексте LLM, умирает с сессией и компактификацией, не ловится `failure_memory`, не попадает в пакеты исполнителям. Цель 0.5.0 — сделать обучение свойством архитектуры: урок переживает сессию/модель/исполнителя, обобщается, превращается в детерминированное fitness-правило, измеряется пререгистрированным бенчмарком.

## ADDED

- When гейт/fitness возвращает FAIL с кодом правила, the arch-be shall записать урок (signature, evidence) в append-only реестр `.arch/lessons/ledger.jsonl` (источник `gate|fitness|tool|review`).
- When статус урока меняется, the arch-be shall менять его только по счётчикам доказательств (observed→candidate→confirmed→promoted→retired), без участия LLM в переходах.
- When начинается сессия или вызывается harness_run, the arch-be shall подмешать закреплённый блок «Уроки контура» (top-K по scope/BM25, лимит токенов из конфига), пересобираемый из реестра после компактификации.
- When harness_run завершается, the arch-be shall прогнать `accept`-команду вне песочницы исполнителя, привести результат к gate-verdict/v1, выдать вердикт первой строкой результата и записать событием `gate_verdict` в журнал.
- When урок confirmed и выразим шаблоном rule_templates, the arch-be shall предложить fitness-правило через дельту после redteam-засева (ловит исходные нарушения, не краснит зелёный HEAD); на Standard/Critical — только с подписью человека.
- When работают параллельные фоновые исполнители флота, the arch-be shall перерисовывать LESSONS.md во всех активных worktree при записи нового урока.
- MCP: `lessons_query`, `lessons_record` (Core); CLI: `arch-be lessons list|show|record|retire|export|import`.
- `arch-be metrics --learning` (learning-metrics/v1): repeat_failure_rate, prevented, dispatches_per_task, статусы уроков, promoted_rules, доля блока уроков, токены на задачу.
- Ленивая загрузка инструментов: ядро + `tool_search`, профили orchestrate/architect/reverse/full.
- Бенчмарк `benchmarks/icl-transfer/` (--ci детерминированный, --live нативный), руки C, C+M, FI, SI, FI+L, X, GEN, PAIR, PROMO, FLEET.

## MODIFIED

- `harness_run`: аддитивный параметр `accept` и `[harnesses.x].accept_cmd`; без них вывод байт-в-байт как в 0.4.10 (совместимость).
- `failure_memory`: сигнатуры источников `gate`/`fitness`; `failure_lessons.md` мигрирует в реестр уроков.
- Хуки Core: добавляются `session-start`, `user-prompt-submit` (connect для Claude Code и Kimi Code, fail-soft).
- `post_gate` без манифеста: не «пропускается молча», если задан `accept` (B3).

## REMOVED

- Ничего не удаляется. `harness_run` без новых обязательных параметров, gate-verdict/v1, exit-коды 0/1/2/3, смысл CONSTRAINTS.yaml и `failure_memory = off|propose|write` сохраняются. Новое поведение — за `[lessons] enabled` (по умолчанию false до волны 3).

## План отката

- Всё новое — за флагом `[lessons] enabled=false` (волны 1–2); откат = выключение флага либо revert коммитов ветки `arch/learning-loop-050` на baseline 554dac7.
- Реестр уроков — новые файлы `.arch/lessons/`, удаление не трогает существующие данные.
- Промоутированные правила снимаются обычной дельтой CONSTRAINTS.yaml (анти-ослабление действует в обе стороны).

## Критерии приёмки

- [ ] Волна 0: стенд в `benchmarks/icl-transfer/seed/`, `PREREGISTRATION.md` + `prereg_hashes.txt` заморожены до первого прогона новой механики; `docs/experiments/live-0.5.0-repro.md` — воспроизведение B1–B9.
- [ ] Волна 1 (L1, L2, L3, L11): руки FI+L и X проходят в `--ci`; тесты переходов статусов, детерминизм рендера, конкурентная дозапись, карантин инъекций, блок уроков после compact_now.
- [ ] Волна 2 (L4–L7): руки GEN, PAIR, PROMO в `--ci`; redteam-засев правил; подпись человека на Critical.
- [ ] Волна 3 (L8–L10): полный `--live`-прогон, таблица «до → после» раздела 9 TASK заполнена; ≥ 3 критерия E1–E6; `benchmarks/architect-value` не хуже 0.4.10 по каждой строке.
- [ ] Дог-фуд: ≥ 3 урока confirmed в `.arch/lessons/ledger.jsonl` этого репозитория из реальных провалов работы над заданием.
- [ ] Каждый пункт — отдельный коммит с тестами; `cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test && cargo test --no-default-features --features core` + дог-фуд `control check` зелёные.
