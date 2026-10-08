# Воспроизведения находок ревью 0.3.13 → фиксы 0.3.14 (форк spine-bank)

Каждая секция — одна находка TASK-v2: что воспроизводили, результат до фикса,
результат после фикса, как повторить. Репродукции выполнялись перед фиксом
(правило «сначала воспроизведи») на бинаре `arch-be` (v0.3.13, база `e3ee160`
и debug-сборки worktree волн).

## Секция A. Волна A: отчёты прогонов — машинные записи (A1, A2)

### A1. «Рукописный PASS» — главная находка

**Сценарий** (репродукция раздела 3 TASK-v2): бандл Critical, где
`VALIDATION.md`, `reports/fitness.md` и `WALKING-SKELETON.md` написаны рукой
(«Итог: PASS (8 из 8)») без единого прогона.

```bash
AB=./target/debug/arch-be
$AB bootstrap "Рукописный PASS" --dir /tmp/hp --domain платежей
cd /tmp/hp
# Все заглушки заменены содержательной прозой; в трёх отчётах — только:
#   # Отчёт
#   Прогон проверен.
#   Итог: PASS (8 из 8)
# Тесты и fitness НЕ запускались. Репетиция отката — честная
# (control gate A4 --rehearse на baseline=HEAD), она не предмет находки.
$AB evidence pack . --route critical
$AB gate --repo . --route critical
```

**До фикса (e3ee160, v0.3.13):** `evidence_verify` PASS — строки «Итог: PASS»
в прозе достаточно; `Итог: PASS`, **exit 0**. Тест-репродукция:
`evidence::tests::verify_flags_unbound_handwritten_report` (падал до фикса:
находки `evidence_report_unbound` не существовало).

**После фикса (волна A):** каждый рукописный отчёт даёт находку
`[warn] evidence_report_unbound — «строка "Итог: PASS" в тексте не доказывает,
что прогон был» → запустите \`arch-be evidence record <kind>\``, вердикт
остаётся PASS (warn по умолчанию, правило 4). В кейсе с
`[evidence] require_records = true` (его генерирует `arch-be bootstrap` —
bank-профиль на Critical) та же находка — `error`, гейт FAIL, **exit 1**.

Закрытие находки — только прогоном:

```bash
arch-be evidence record fitness                 # дефолт: arch-be control check .
arch-be evidence record tests    --cmd "python3 -m pytest skeleton -q"
arch-be evidence record skeleton --cmd "python3 -m pytest skeleton -q"
arch-be evidence pack . --route critical        # артефактом становится запись
arch-be gate --repo . --route critical          # PASS
```

Правка кода после записи (`echo … >> skeleton/…py`) → находка
`evidence_record_stale` («входы прогона изменились после записи»), гейт FAIL
до повторного `evidence record`. См. ADR-066.

### A2. Выводимые артефакты не пишутся руками

**До фикса:** bootstrap создавал заглушки и для машинных артефактов
(`RISK.md`, `VALIDATION.md`, `reports/fitness.md`, `WALKING-SKELETON.md`,
`.arch-handoff/REHEARSAL.json`), строка прогресса была суммарной
(`бандл 7/13`), а `risk_level` требовал рукописный файл.

**После фикса:**

- `arch-be evidence pack` пишет в `EVIDENCE.yaml` запись значимости
  (маршрут, триггеры с источниками `declared`/`diff`, HEAD) — `risk_level`
  выводится из неё; рукописный RISK.md необязателен (legacy-файл по-прежнему
  принимается, обратная совместимость).
- `arch-be bootstrap` не создаёт заглушек выводимых артефактов: каркас
  содержит 6 заглушек автора (проблема, спека, приёмка, откат, решение A3,
  ревью) вместо 10+; свежий каркас печатает
  `бандл пишет автор 8/8 · выведет машина 1/5` (risk_level уже выведен
  записью значимости при упаковке).
- Тот же раздельный счёт печатает `arch-be evidence pack`
  (`Бандл: пишет автор X/8 · выведет машина Y/5`) — этими двумя числами
  снимается отложенный замер церемонии (файлов, которые человек правит
  руками, до/после).
- Приёмка: на Critical автор пишет ≤ 8 артефактов (тест
  `bootstrap_walks_to_green_when_artifacts_are_written` проходит полный путь
  до PASS при `require_records = true`, записав 7 файлов автора); подмена
  выводимого прозой ловится через A1 (`evidence_report_unbound`).
- Подсказка следующего шага проводника для машинного ключа ведёт к прогону
  (`evidence record` / `control gate A4 --rehearse` / `evidence pack`), а не
  к «создайте файл руками».

### F5. Бандл доказательств из OpenSpec change

**До фикса:** профиль бандла искал проблему/спеку/приёмку только в
`PROBLEM.md`, `SPEC.md`, `ACCEPTANCE.md`, `DELTA.md` — команда на OpenSpec
писала их второй раз.

**После фикса:** `problem` читается и из `openspec/changes/<id>/proposal.md`
(обязательна секция `## Why`), `spec_or_delta` — из дельты
`openspec/changes/<id>/specs/**/spec.md`, `acceptance` — из той же дельты,
если в ней есть сценарии `#### Scenario:`. Архив (`changes/archive/`) не
читается; канонические файлы приоритетнее (совместимость). Spine только
читает markdown OpenSpec — не пишет и не переписывает его. Бандл дельты
`changes/<name>/` видит change корня репозитория; путь в манифесте — через
`..` (без привязки к машине).

```bash
# кейс с openspec/changes/add-limits/{proposal.md, specs/payments/spec.md},
# без PROBLEM.md/SPEC.md/ACCEPTANCE.md:
arch-be evidence pack . --route fast
#   + problem        openspec/changes/add-limits/proposal.md
#   + spec_or_delta  openspec/changes/add-limits/specs/payments/spec.md
#   + acceptance     openspec/changes/add-limits/specs/payments/spec.md
arch-be evidence verify .   # Итог: PASS
```

## Волна F — OpenSpec без двойного учёта

### F1. «Двойной учёт»: правка `model/` по активному change OpenSpec краснит `delta_guard`

Воспроизведена 2026-10-07 на бинаре 0.3.13 по сценарию раздела 8 TASK-v2
(`/tmp/os`: `openspec/specs/payments` + активный `openspec/changes/add-limits`,
упоминающий `model/CMP-001.md` в proposal.md и tasks.md; правка `model/CMP-001.md`,
`arch-be gate --repo . --base HEAD`).

До фикса (0.3.13):

```
[FAIL] delta_guard — правки спайна мимо дельты: 1 файлов (активных дельт: 0)
    ↳ model/CMP-001.md — не упоминается ни в одной активной дельте — активных дельт нет
Итог: FAIL — провалено составляющих: 2 (exit 1)     # второй FAIL — fitness, см. ниже
```

и standalone:

```
$ arch-be delta guard --repo . --base HEAD
[error] model/CMP-001.md — не упоминается ни в одной дельте-покрытии (активных дельт нет)
  → оформите правку дельтой: arch-be delta new <name>, …
Итог: FAIL — правки спайна мимо дельты (exit 1)
```

Подсказка при этом учила дублировать описание в `DELTA.md` — то, что команда
уже описала в change OpenSpec.

После фикса (feat(F1)):

```
$ arch-be delta guard --repo . --base HEAD
Изменённых файлов: 3, защищённых среди них: 1 (активных дельт: 0, активных changes OpenSpec: 1)

[ok] model/CMP-001.md — покрыт активным change OpenSpec 'openspec:add-limits' (по пути)

Итог: PASS — все правки спайна покрыты дельтами/changes OpenSpec        # exit 0
```

и в едином гейте: `[PASS] delta_guard — … покрытие: model/CMP-001.md ← 'openspec:add-limits'`.

**Ортогональная находка репродукции (не F1):** файл `constraints: []` из
сценария задания отклоняется составляющей `fitness` («файл не содержит правил —
ожидается непустой корень rules:/constraints:») — поведение существующее,
на вердикт `delta_guard` не влияет; волной F не затрагивается.

Правило владения (ADR-055) проверено тестами и прогоном гейта с диапазоном
исполнителя (`src/gate/components/tests.rs::gate_delta_guard_self_approved_openspec_inside_range`):
change, созданный внутри диапазона, правку не узаконивает — `delta_guard` FAIL
с находкой `self_approved` (error) и именем источника `openspec:add-limits`;
change, существовавший до диапазона, — PASS
(`src/delta.rs::guard_ownership_rejects_change_created_inside_range`, сценарии A/B;
симметрия для дельты — `guard_ownership_rejects_delta_created_inside_range`).

### F4. Осиротевшие `covers:`

Не дефект-репродукция, а новая находка (warn) в `openspec coverage` (коммит
feat(F4)): ссылка `covers:` на исчезнувший id требования (правка текста
требования меняет id) раньше терялась молча. Проверено прогоном: реестр с
`covers: ["openspec:payments#00000000"]` против спеки с переименованным
текстом даёт в отчёте

```
- [warn] covers_orphan — openspec:payments#00000000 (правила: idem_detector) →
  кандидат той же capability без покрытия: openspec:payments#a90fed0f —
  «Idempotent intake v2» (openspec/specs/payments/spec.md:3)…
```

Exit-коды не меняются (0; с `--strict` — 1 только от «без решения», не от
осиротевших ссылок). Тесты: `coverage_classifies_covered_unverifiable_unresolved`
(находка + кандидат + отсутствие ложных срабатываний на живых ссылках),
`coverage_orphan_without_candidate_and_foreign_ids_ignored` (нет кандидата,
чужие префиксы id не проверяются, дубль id у двух правил — одна находка).
