# Репродукции ревью форка 0.3.14 (TASK-v2)

Репродукции, выполненные перед фиксом (правило «сначала воспроизведи»).
База: `e3ee160` (v0.3.13).

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
