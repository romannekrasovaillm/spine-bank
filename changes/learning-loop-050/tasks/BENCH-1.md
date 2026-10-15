# Задача BENCH-1 (spine-bank 0.5.0): бенчмарк benchmarks/icl-transfer — обобщение стенда, режим --ci

Прочитай ПЕРВЫМ: этот файл. Далее: TASK-Spine-0.5.0-learning-loop.md (§6 целиком, §1 правила 8–9), benchmarks/icl-transfer/PREREGISTRATION.md, benchmarks/icl-transfer/prereg/pool.yaml, benchmarks/icl-transfer/seed/ (оригинальный стенд icl-lab: run_arm.py, gate/accept.py, tasks.json, template/paylib, shim.py), AGENTS.md, docs/RUST_CONVENTIONS.md.

Объём — ТОЛЬКО режим --ci (live — отдельная задача, не делай):
1. **Генератор пула:** из prereg/pool.yaml (14 конвенций) по семени выбираются 2–3 НЕ-holdout конвенции (holdout.txt читается ТОЛЬКО механизмом исключения по id — содержимое не логируется) и инстанцируются в свежий репозиторий из seed/template/paylib + гейт (обобщение seed/gate/accept.py: конвенции из пула как правила гейта).
2. **Семейства задач:** «модуль» (5 задач — из seed/tasks.json), «CLI-команда» (4), «миграция данных» (3, holdout для GEN). Генерация детерминирована по семени.
3. **Руки --ci (FakeLlm + fake-harness, детерминированно, в cargo test):** FI+L (реестр включён), FI (выключен), X (два fake-исполнителя), GEN (миграция holdout), PAIR (две конвенции одного subject_kind), PROMO (10 задач, L7), FLEET (5 параллельных со сдвигом). C/C+M/SI в --ci не нужны (нет живого кодера).
4. **run_all.sh --ci** прогоняет руки и считает метрики раздела 6/9 TASK; отчёт — benchmarks/icl-transfer/results/.
5. **Анти-Goodhart CI-греп** (правило 9): скрипт проверяет, что маркерные строки пула (marker: из pool.yaml) отсутствуют в src/ и assets/ — входит в тест бенчмарка.
6. Интеграция в `cargo test`: `tests/icl_transfer_ci.rs` (или benches) — детерминированный, быстрый (< 2 мин), без сети.

НЕ делай: --live режим, изменение порогов/пула/семян (заморожены PREREGISTRATION.md), знание конкретных holdout-конвенций в коде бенчмарка (только по id из файла).

ПРОЦЕСС (жёстко): никаких фоновых команд — всё foreground по одной; export CARGO_TARGET_DIR=$PWD/target-local; ВЕРИФИКАЦИЯ TARGETED: cargo fmt --check; cargo clippy --all-targets -- -D warnings; cargo test --test icl_transfer_ci (и смежные); bash benchmarks/icl-transfer/run_all.sh --ci (если применимо). ПОЛНЫЙ cargo test и control check ЗАПРЕЩЕНЫ (оркестратор). Коммит ОДИН: `feat(bench): icl-transfer --ci — генератор пула, руки FI/FI+L/X/GEN/PAIR/PROMO/FLEET, анти-Goodhart греп`. Финал — блок ```json {"status": "complete|partial|blocked", ...}``` обязателен (status строго из перечня).
