# PLAN — spine-bank 0.5.0 «контур, который учится»

База: ветка `arch/learning-loop-050` от `554dac7` (0.4.11; базовый коммит задачи `641b46a` — предок, расхождение 0.4.10→0.4.11 учтено в DEVIATIONS.md).
Нумерация ADR: в задаче указаны ADR-084…090, но ADR-084 уже занят (порт санитайзера tool-пар). Сдвиг: **L1→ADR-085, L2→ADR-086, L3→ADR-087, L6→ADR-088, L7→ADR-089, L9→ADR-090, L10→ADR-091**. Зафиксировано в DEVIATIONS.md.
Порт в spine-aiml (arch-ml): после приёмки волн 1–2 в spine-bank, нумерация ADR там — со следующего свободного (≥ ADR-057).

## Волна 0 — воспроизведение и пререгистрация (оркестратор, без кодового харнесса: только md/конфиги/перенос стенда)

| Шаг | Файлы | Коммит |
|---|---|---|
| Стенд → seed | `benchmarks/icl-transfer/seed/**` (26 файлов из icl-lab.tar.gz) | `chore(L0): перенос стенда icl-lab` |
| Пререгистрация | `benchmarks/icl-transfer/PREREGISTRATION.md`, `prereg_hashes.txt`, `prereg/{pool.yaml,seeds.txt,holdout.txt}` | `chore(L0): пререгистрация` |
| Воспроизведение B1–B9 | `docs/experiments/live-0.5.0-repro.md` (команды/грепы по src/, статус каждой находки) | `docs(L0): repro B1–B9` |
| ADR волны 1 | `docs/adr/ADR-085..087` | `docs(adr): ADR-085..087` |

## Волна 1 — фундамент (кодовый харнесс, `[lessons] enabled=false`)

| Пункт | ADR | Файлы (создать/тронуть) | Тесты | Приёмка |
|---|---|---|---|---|
| **L1 Lesson Ledger** [Общий] | ADR-085 | `src/lessons/{mod,ledger,render,scope}.rs`; CLI `src/cli/lessons.rs`; MCP `lessons_query/lessons_record` (`src/mcp*`); конфиг `[lessons]` | табличные переходы статусов; детерминизм рендера байт-в-байт; конкурентная дозапись 2 процессов; import без evidence отклоняется | `arch-be lessons list` на стенде после руки S показывает G5/G6 confirmed с run_id |
| **L2 Гейт в петле** [Harness+Core-хук] | ADR-086 | `src/harness.rs` (accept/accept_cmd, verdict first line, событие gate_verdict, вызов lessons_record), `src/hook.rs`, `src/connect/hooks.rs` | fake-harness + fake-гейт exit 1 со строкой `[G5]`: событие в журнале, запись в реестре, первая строка результата; без accept — байт-в-байт как 0.4.10 | рука S без подсказки «зови гейт через bash» — Spine видит вердикт |
| **L3 Инъекция и защита от компактификации** [Общий] | ADR-087 | `src/agent.rs` (закреплённый блок, пересборка после L1/L3), `src/failure_memory.rs` (сигнатуры gate/fitness, миграция failure_lessons.md), `src/injection.rs` (карантин), `src/subagent.rs`, `src/ralph.rs` (блок уроков в свежий контекст), Core-хуки session-start/user-prompt-submit | блок присутствует после compact_now; лимит токенов; «ignore previous instructions» → quarantine; session-start без реестра = пустой fail-soft вывод | синтетический тест B5: 50 ходов шума после урока → блок цел |
| **L11 Метрики обучения** [Общий] | — (без ADR, аддитивно) | `src/agent/journal.rs`, `src/llm/openai_compat.rs` (usage при stream=false), `src/cli/metrics.rs` (--learning, learning-metrics/v1) | usage пишется при stream=false; формат learning-metrics/v1 валиден | B7 закрыт: `arch-be metrics` показывает реальные токены |

Порядок: L1 → (L2, L3 параллельно в отдельных worktree) → L11. Каждый пункт — отдельный коммит `feat(Ln): …` с тестами.

## Волна 2 — от урока к знанию (`enabled=false`)

| Пункт | ADR | Файлы | Критерии |
|---|---|---|---|
| **L4 lessons_match** | — | `src/lessons/match.rs` (scope glob + BM25 из `src/kb.rs` + приоритет статусов), событие `prevented` | руки GEN/PAIR в --ci |
| **L5 Автоупаковка handoff** | — | `src/harness.rs`, `src/handoff*.rs` (лёгкий пакет: TASK.md + LESSONS.md + accept_cmd, приоритет явного handoff_create) | снапшот TASK.md; уроки user-уровня не утекают без флага |
| **L6 Обобщение/композиция** | ADR-088 | `src/lessons/{generalize,compose,subject.rs}` (классификатор по диффу: recover/arch_diff) | E3, E4; табличные сценарии расширения/отката scope |
| **L7 Промоушен в fitness** | ADR-089 | `src/lessons/promote.rs`, `rule_templates`, redteam-засев, интеграция с delta_propose и pre-commit хуками connect | E5; без подписи на Critical — exit 1 |

## Волна 3 — масштаб (`enabled=true`, профиль orchestrate дефолтом для `arch-be run`)

| Пункт | ADR | Файлы | Критерии |
|---|---|---|---|
| **L8 Стигмергия флота** | — | `src/worktree.rs`, реестр фоновых hr-*, атомарная перерисовка LESSONS.md | E6: поздно стартовавшие получают урок, завершённые не трогаются |
| **L9 Ленивые инструменты** | ADR-090 | `src/tools.rs`, `src/agent.rs`, `tool_search`, профили | первый запрос ≤ 25 000 симв.; architect-value не хуже 0.4.10 |
| **L10 Экспорт/импорт/org** | ADR-091 | `src/lessons/{export,import}.rs`, lessons-bundle/v1 + SHA-256 | импорт понижает статус; битый хэш отклоняется |

## Бенчмарк и финал

- `benchmarks/icl-transfer/`: обобщение seed → генератор пула (≥12 конвенций, 2–3 на прогон по семени), семейства задач (модуль×5, CLI×4, миграция×3 — миграция holdout), руки C/C+M/FI/SI/FI+L/X/GEN/PAIR/PROMO/FLEET, `--ci` (FakeLlm+fake-harness, в `cargo test`) и `--live`.
- Анти-Goodhart grep src/ assets/ по строкам пула — в CI бенчмарка.
- Финал: таблица раздела 9 TASK заполнена живым прогоном `run_all.sh`; CHANGELOG; DEVIATIONS.md; отчёт о дог-фуде (≥3 confirmed урока в собственном реестре).

## Стоп-условия (эскалация человеку)

Порог не достигнут после двух итераций; нужно менять `*/v1`; промоушен правила требует подписи; live-прогон невозможен без ключей и шим не работает; пункт помечен [РЕШЕНИЕ ЧЕЛОВЕКА].
