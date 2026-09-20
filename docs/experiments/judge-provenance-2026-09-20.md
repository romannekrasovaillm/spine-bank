# Происхождение оценки рубрики: что сделано (0.3.5, срез J1–J5, J7, J8)

Ветка `feat/judge-provenance` (worktree `/home/roman/spine-bank-034-jp`, база
`a960a17` = `release/0.3.5`). Задание — `CLAUDE-TASK-Spine-Core-judge-provenance.md`
(«кто судил, чем и можно ли это проверить»). Коммиты: `7b5fbdc` (J1),
`3579834` (J2), `b42b3bd` (J3), `5f48aa0` (J4), `4708444` (J5), `f39866f` (J8),
`9c1a68a` (J7).

## Почему работа шла в отдельном дереве

В основном дереве параллельно работали ещё четыре сеанса по своим заданиям
(0.3.5-defects, semantic-rubrics, executable-invariants и др.), и файлы
пересекались: `src/rubric.rs`, `src/mcp_server.rs`, `src/gate.rs`,
`src/config.rs`. Выяснилось, что `git commit -- <путь>` коммитит **всё
содержимое файла**, поэтому «коммит своими путями» захватывал бы незакоммиченные
хунки соседей (и незаконченные: в тот момент дерево не собиралось из-за чужого
`src/rule_templates.rs`). Работа перенесена в обособленный worktree; свои хунки
из общего дерева сняты, оно снова собирается. Номера ADR согласованы:
соседи заняли 046, 047, 049, 050, 051 — этот срез занял 048.

## Что сделано

### J1 — блок `provenance` в отчёте (ADR-048)

Отчёт рубрики несёт аддитивное `provenance`:

- режим `declared` (оценку собрал хост): `host` из `clientInfo` рукопожатия MCP,
  `session_id`, `session_calls_before`, `prompt_sha256`,
  `prompt_issued_in_session`;
- режим `launched` (судью запустил Spine): `launcher` — имя модели из `[models]`,
  а для `kind = "cli"` ещё команда, аргументы (значения-секреты маскируются
  `src/secrets.rs`) и версия по `<command> --version`;
- в обоих режимах: `samples` (хэши сырых ответов) и `operator` — git
  `user.name`/`user.email` репозитория (ключ `[judge] record_operator`, дефолт
  `true`).

Сервер MCP получил состояние сессии (`SessionState` под `Mutex`): `initialize`
запоминает `clientInfo` и выдаёт `sessionId`, `rubric_prompt` запоминает хэш
выданного промпта и число **завершённых** вызовов, `rubric_verify` пишет это в
отчёт. Верификация без предшествующего промпта в этой сессии — не ошибка:
`prompt_issued_in_session: false`.

### J2 — воспроизводимость отчёта (ADR-048)

Сырые ответы судьи сохраняются рядом с отчётом
(`reports/rubric/raw/<slug>/sample-<n>.json`: текст как он пришёл, его SHA-256,
признак «не разобран», метка судьи, хэш документа на момент оценки); отчёт несёт
`scores` — баллы и метки по критериям. Из этих свидетельств отчёт
**пересобирается** тем же `build_report`, без LLM:

- `arch-be rubric reverify <отчёт|каталог>` — расхождение даёт код 1 и называет
  каждое расхождение с числами;
- составляющая `decision_quality` — находки `rubric_report_inconsistent`
  (error: балл правили руками) и `rubric_raw_tampered` (error: правили
  сохранённый ответ);
- нет сырых ответов (старый отчёт) — примечание «отчёт невоспроизводим», без
  находки.

### J3 — автор из шапки документа

`adr_registry` читает поле `- Author-model:` / `- Модель-автор:` той же терпимой
логикой, что `Status`/`Статус`. Автор берётся **из шапки** (значение из
документа сильнее аргумента вызова), расхождение с аргументом называется полем
`author_model_declared` и предупреждением гейта `author_model_mismatch`. В отчёт
идёт `author_source: header | argument | none`. Правка шапки меняет документ →
отчёт устаревает (`rubric_report_stale`): автора нельзя «вспомнить» задним
числом. `adr_new` принимает `author_model` и пишет строку в шапку; шаблоны ADR в
скилле `adr-authoring` и в каркасе `bootstrap` получили строку.

### J4 — семейства моделей

`family_key`/`family_of` по префиксам меток: `claude`→anthropic,
`gpt|o1|o3|codex`→openai, `gemini`→google, `glm`→zhipu, `deepseek`, `qwen`,
`kimi`→moonshot, `gigachat`→sber; секция `[judge.families]` конфига дополняет и
переопределяет таблицу по самому длинному подошедшему префиксу. Префикс
совпадает только как отдельное слово (`claude-opus-4` — да, `claudette` — нет).
Две разные **неизвестные** метки остаются разными семействами. Находка
`judge_same_family` (warn), ключ `require_distinct_family = true` поднимает её до
error.

### J5 — уровень независимости (ADR-048; отдельный ADR-049 не оформлен)

Отчёт несёт `independence`: `none` | `declared` | `declared_cross_family` |
`launched` | `launched_cross_family`. Ключ
`[gate.decision_quality] min_independence` (дефолт `none` — поведение 0.3.4)
даёт находку `judge_independence_low` (error) ниже порога. Рабочая сессия
(`declared` и больше `[judge] clean_session_max_calls` вызовов до промпта) —
**примечание** паспорта, а не находка. Паспорт, блок 2, различает «заявлена»
(метки передал хост, с хостом и версией) и «обеспечена запуском» (судью запускал
Spine; какая модель отвечала — механикой не проверяется). `trust`: условие
пятой ступени не изменено, в доказательство добавлен минимальный уровень, ключ
`[trust] min_independence` (дефолт `declared`). В конверт вердикта добавлен вход
`judge_raw` (хэш каталога сырых ответов): правка сохранённого ответа меняет
аттестацию; у кейсов без сырых ответов вход `absent`, аттестация не меняется.

### J7 — отчёт не теряется молча; `--rw=reports`

Режим `--rw=reports` (у `mcp serve` и у `connect <хост>`) разрешает запись
только отчётам рубрики. В read-only при заданном `target` ответ несёт
`artifact_saved: false`, `artifact_json` и `artifact_path`, а первая строка
`summary` говорит «Отчёт НЕ сохранён: гейт его не увидит» и что делать. Файл,
сохранённый хостом, проходит ту же сверку и ту же составляющую гейта.
`doctor --host` печатает режим тремя значениями.

### J8 — CLI читает `[judge]`

`arch-be rubric run` передаёт `cfg.judge` (раньше подставлял дефолты, а MCP брал
конфиг). Отчёт несёт снимок `judge_config` (samples, порог нестабильности, порог
сходства цитат). Мёртвый `rubric::evaluate` без опций удалён.

## Пример отчёта

`declared` (split-judge через MCP, `--rw=reports`):

```jsonc
"independence": "declared_cross_family",
"author_source": "header",
"author_model": "claude-opus-4",
"judge_config": {"samples": 2, "unstable_stdev": 1.0, "evidence_min_similarity": 0.8},
"provenance": {
  "mode": "declared",
  "host": {"name": "claude-code", "version": "2.1.278"},
  "session_id": "f491de32cccb747f",
  "session_calls_before": 0,
  "prompt_sha256": "…",
  "prompt_issued_in_session": true,
  "samples": [{"sha256": "…", "dropped": false}],
  "operator": "Тест Архитектор <arch@example.invalid>"
}
```

`launched` (`arch-be rubric run --model judge-cli`):

```jsonc
"independence": "launched_cross_family",
"provenance": {
  "mode": "launched",
  "launcher": {"provider": "judge-cli", "command": "claude", "args": ["-p"], "cli_version": "…"}
}
```

Паспорт, блок 2 — **до** этого среза: «независимость судьи не подтверждена: судья
совпадает с автором либо автор не указан». **После**: по каждому встреченному
уровню — «независимость судьи **заявлена**: метки (glm-5.2) переданы хостом
(claude-code 2.1.278), механикой не проверяются — судить могли в рабочей сессии
автора» либо «**обеспечена запуском**: Spine сам запускал судью (claude), каждый
сэмпл — отдельный процесс/запрос; какая модель отвечала, механикой не
проверяется».

## Существующие отчёты в `кейсы/`

Отчёты (8 штук в `кейсы/digital-ruble-merchant` и `кейсы/salary-payments`) не
несут ни `provenance`, ни `independence`: режим читается как `declared` без
деталей, порог независимости на них не действует, новых error по умолчанию нет —
поведение 0.3.4 сохраняется. Это проверяется тестом
`legacy_report_without_provenance_loads_and_passes_as_before` (и примечанием
«отчёт невоспроизводим» в паспорте).

## Сквозные сценарии

- **Запуск судьи самим Spine.** Тест `cli_rubric_run_respects_judge_config`:
  фиктивный CLI-судья, `[judge] samples = 5`, отчёт получает
  `provenance.mode = launched`, названный запускатель и снимок правил;
  `min_independence = "launched"` даёт зелёный гейт
  (`min_independence_blocks_below_threshold` проверяет и красный случай для
  `declared`).
- **Передача судейства во второй харнесс** — НЕ ПРОВЕДЁН: пункт J9 (`rubric
  handover` / `rubric accept` / плейбук `spine-judge-handover`) не реализован.
  Из его частей готово то, на что он опирается: `--rw=reports`, `artifact_json`
  в read-only, узкий белый список, уровни независимости.

## Что НЕ сделано (честно)

- **J6** — `JournalEntry.session_id`/`host`, блок `judging` в журнале, раздел
  «Судейство» в `arch-be digest`.
- **J9** — `rubric handover` / `rubric accept`, MCP-инструменты
  `rubric_handover`/`rubric_accept`, плейбук `spine-judge-handover`, порядок
  предпочтения в скиллах `spine-adr-judge`/`rubric-judging`, проверка судьи в
  `doctor`, команды `rubric run --samples/--all-accepted`.
- **§8** — два мутатора красного угла (поднятый рукой балл, подменённая метка
  автора в шапке) не добавлены; засев этих дефектов возможен уже сейчас, но в
  карту обнаружения они не внесены.
- **Документация** — `docs/CONNECT.md` (два способа передачи + `--rw=reports`),
  `docs/SPINE-BE-GUIDE.md`, `docs/evals.md`, `docs/HARNESSES.md`, README,
  `ROADMAP.md`. Обновлены только `docs/control.md`, `docs/tools.md`,
  `docs/rubrics_and_benchmarks.md` и ADR-048.
- **CHANGELOG.md** — записи для 0.3.5 не внесены намеренно: секцию `[0.3.5]`
  ведёт срез дефектов, и правка с двух сторон дала бы конфликт. Записи для
  вливания (в `### Added`): «происхождение оценки рубрики: режимы
  declared/launched, хэши промпта и сырых ответов, оператор (ADR-048)»;
  «воспроизводимость отчёта: `rubric reverify`, находки
  `rubric_report_inconsistent`/`rubric_raw_tampered»»; «автор из шапки ADR,
  семейства моделей, уровень независимости (ADR-048)»; «`--rw=reports` и
  `artifact_json` в read-only». В `### Fixed`: «`rubric run` читает секцию
  `[judge]` (J8)».

## Открытые решения

Из четырёх вопросов, помеченных в задании «[РЕШЕНИЕ ЧЕЛОВЕКА]», ответы получены
до работы: `record_operator` — дефолт `true` (имя и адрес); `[trust]
min_independence` — ключ введён с дефолтом `declared`; мутанты §8 — отдельными
строками вне знаменателя (не реализовано, решение принято). Осталось
нереализованным ни одно открытое решение — открытым остаётся только объём.

## Проверки

Все прогоны — в обособленном дереве, свой target:

```
cargo test        # 1433 теста: 1325 lib + 2 + 59 cli + 46 mcp_serve + 1 doctest, 0 упавших
cargo clippy --all-targets -- -D warnings   # чисто
cargo fmt --check (по своим файлам)         # чисто
```
