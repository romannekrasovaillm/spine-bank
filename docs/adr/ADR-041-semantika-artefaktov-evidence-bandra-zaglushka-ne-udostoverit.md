# ADR-041. Семантика артефактов Evidence Bundle: «есть» ≠ «написан»

- Date: 2026-09-19
- Status: Accepted
- Reversibility: обратимо (флаг `[evidence] semantics = off` возвращает
  поведение 0.3.3 без отката кода)

## Context

ADR-040 (0.3.3) научил гейт не зеленеть, когда у проверки **нет входа**:
обязательная составляющая в `SKIP` даёт `INCOMPLETE` и exit 3. Исследование
2026-09-19 на кейсе «Приём оплаты ЦР для ТСП» показало ту же болезнь уровнем
глубже: **гейт зеленеет, когда вход есть, но пустой.**

Воспроизведение на 0.3.3 (коммит `9486d79`), маршрут Critical:

```bash
echo TODO > docs/DECISION.md
echo TODO > WALKING-SKELETON.md
printf '# R\n\nVERDICT: NOT-READY\n' > docs/REVIEW.md
arch-be evidence pack . --route critical
arch-be evidence verify .
# → «Итог: PASS — выпуск разрешён», exit 0
```

Тринадцать обязательных артефактов «на месте», хэши сходятся, полнота
подтверждена — и зелёный вердикт удостоверяет **наличие файла**, а не
**записанное решение**. Ревьюер прямо написал `NOT-READY`, а механика ответила
«выпуск разрешён».

Причина: `required_artifacts` + `hash != item.hash` — это два класса исхода
(«отсутствует», «изменён»). Третьего — «пустышка/не готов» — не существовало,
хотя исходный смысл Evidence Bundle по AI-Disrupt PDLC ровно обратный: не
«отчёт после», а гейт релиза.

## Decision

**1. Третий класс исхода.** `EvidenceVerdict` получает поле
`semantics: Vec<SemanticFinding>` (`src/evidence.rs`): ключ артефакта, код
правила, критичность, сообщение и обязательный `fix_hint`. `passed` требует
отсутствия блокирующих находок наряду с полнотой и целостностью.

**2. Детектор заглушек — один на дельту и бандл.** `src/stubs.rs` — общий
модуль с двумя уровнями строгости:

- `is_template_stub` — историческое правило `delta validate` (строка целиком
  `<…>` либо `TODO`/`TBD`); поведение 0.3.3 сохраняется байт-в-байт, вердикт
  `delta validate` не меняется;
- `is_stub_line` — то же плюс `<…>`-вставка внутри строки (`- **choice**:
  <выбор>`); HTML-теги (`<br>`, `<strong>`) внутри строки не ловятся.

**3. Правила содержания по ключам.** Общие для всех: размер меньше
`[evidence] min_bytes` (дефолт 200 б) либо наличие маркера-заглушки →
`evidence_stub`. Специфичные:

| Ключ | Правило | Код находки |
|---|---|---|
| `adversarial_review` | нет строки вердикта (`VERDICT`/`ВЕРДИКТ`, регистр свободен) | `review_verdict_missing` |
| `adversarial_review` | `NOT-READY` — выпуск не подтверждён ревьюером | `review_not_ready` |
| `decision_a3` | пусто/`—`/`-`/`TBD` в `choice`, `rationale`, `rejected`, `expiry`, `decided_by` | `a3_not_signed` |
| `decision_a3` | `expiry` в прошлом | `a3_expired` |
| `decision_a3` | `expiry` не дата | `a3_expiry_invalid` |
| `rollback_rehearsal` | репетиция не PASS | `rehearsal_not_passed` |
| `rollback_rehearsal` | baseline репетиции ≠ baseline плана | `rehearsal_stale_baseline` |
| `rollback_rehearsal` | `REHEARSAL.json` не разбирается | `rehearsal_invalid` |
| `validation`, `fitness_report`, `walking_skeleton` | в отчёте нет строки итога | `evidence_stub` |
| `validation`, `fitness_report`, `walking_skeleton` | в отчёте итог FAIL | `evidence_reports_fail` |

Записи A3 читаются в формах `- **choice**: …`, `**choice**: …` и `choice: …`.
Разбор репетиции отката переиспользует `rehearsal::load_report`/`load_plan`, а
не свою копию правила.

**4. Строгость по маршруту (обратная совместимость).** Ключ
`[evidence] semantics = auto|off|warn|error`, дефолт `auto`:

- `Critical` → находки содержания `error` — выпуск блокируется (там выпуск и
  так под гейтом);
- `Standard`/`Fast` → `warn` — чужой зелёный пайплайн не краснеет, но
  архитектор видит предупреждения;
- `off` → проверки содержания не выполняются, и об этом печатается честная
  строка в блоке «не проверяется механикой» (поведение 0.3.3, обратимость).

**5. Spine не притворяется.** Подпись A3 механикой не проверяется: при
заполненном `decided_by` печатается «подпись A3: заявлена (<значение>),
подлинность механикой не проверяется», и то же — в блоке `not_verified`.
Адекватность выбранного варианта остаётся работой ревьюера (паспорт вердикта,
W1).

## Alternatives

**А. Ничего не менять, оставить «артефакт есть = артефакт готов».** Отклонено:
это и есть дефект; зелёный вердикт при `NOT-READY` в бандле — прямая ложь
архитектору, ради которой выпускается 0.3.4.

**Б. Эвристики по смыслу текста (TF-IDF, ключевые слова, LLM-судья в ядре).**
Отклонено: нарушает AD-2 (детерминированный слой контроля без LLM) и правило
ТЗ «без LLM в ядре». Семантику решения механика не проверяет и делать вид,
что проверяет, не должна — её место в состязательном ревью и в
`decision_quality` (ADR-042).

**В. Сразу `error` на всех маршрутах.** Отклонено по правилу поэтапного
ужесточения (ТЗ 1.5): может покраснить существующий зелёный пайплайн на
Fast/Standard без предупреждения. Схема «Critical сразу error, остальные warn
в 0.3.4» оставляет миграционный коридор; переход к `error` везде —
`[evidence] semantics = error` в конфиге проекта.

**Г. Свой детектор заглушек только в `evidence.rs`.** Отклонено: два правила
про одно и то же разойдутся (ровно этот класс дефектов — находка ДКА о
дублировании правил). Вынос в `src/stubs.rs` фиксирует одно правило на обоих
потребителей.

## Consequences

**Что может покраснеть после обновления.** На маршруте Critical бандл, где
`DECISION.md`/`REVIEW.md`/`ROLLBACK`-репетиция/отчёты являлись заглушками,
перестаёт быть зелёным. На Standard/Fast те же случаи дают предупреждения без
смены exit-кода. Полный возврат к поведению 0.3.3 — `[evidence] semantics =
off`.

**Цена ложного срабатывания.** Порог 200 байт — стартовое предложение, не
измеренный оптимум: короткий, но честный артефакт (например, ссылка на
внешний документ) будет помечен. Порог настраивается `[evidence] min_bytes`.
Слишком короткие файлы в существующих тестах (`hash_is_path_invariant_*`,
`fast_route_packs_minimal_bundle`) остаются зелёными, потому что на Fast и
Standard находки — `warn`.

**Проверяемость.** Тесты `verify_passes_on_complete_bundle`,
`verify_flags_stub_artifact`, `verify_blocks_not_ready_review`,
`verify_flags_unsigned_a3`, `verify_flags_expired_a3`,
`verify_flags_failing_report`, `verify_flags_stale_rehearsal_baseline`,
`semantics_off_restores_033_behaviour`. На red-team наборе это закрывает
дефекты D13 (`DECISION.md` = «TODO») и R (ревью `NOT-READY`).

## References

- `src/stubs.rs`, `src/evidence.rs`, `src/config.rs` (`[evidence]`),
  `src/gate.rs` (`component_evidence`)
- ADR-040 — трёхзначный вердикт гейта (предыдущий уровень той же болезни)
- `CLAUDE-TASK-Spine-Core-0.3.4.md`, раздел 3, находка Н1
