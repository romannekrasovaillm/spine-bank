# Cron и баш-пайпы

Принцип харнесса: **задача — это markdown; исполнитель — LLM с ядерными
инструментами (баш-пайпы!); тайминг — cron**. Реализация — `src/cron.rs`;
CLI — `arch-be cron` (`src/main.rs`).

## Расписание `cron.toml`

Путь — `cron.file` (дефолт `~/.arch-harness/cron.toml`; образец —
`cron.example.toml`, `arch-be init` копирует его и md-инструкции):

```toml
[[job]]
name = "kb-digest"                    # уникальное имя (по нему arch-be cron run)
schedule = "30 9 * * *"               # 5 полей: минута час день-месяца месяц день-недели
task_md = "~/.arch-harness/cron/kb_digest.md"   # md-инструкция исполнителю
model = "deepseek"                    # опционально (иначе default_model)
out = "~/.arch-harness/reports/cron"  # опционально (иначе <reports_dir>/cron)
```

Нюанс: тильда в `task_md`/`out` **не раскрывается автоматически** — после
`arch-be init` замените на абсолютные пути (в `cron.toml` самого харнесса
тильда в `cron.file` раскрывается, внутри job-полей — нет).

## Команды

```bash
arch-be cron list                # задачи: имя, расписание, файл инструкции
arch-be cron run kb-digest       # запустить задачу по имени сейчас
arch-be cron tick                # прогнать дюжные задачи (для системного cron)
```

### `tick` и системный crontab

Харнесс не держит собственный демон: периодичность — системный cron,
вызывающий `arch-be cron tick`:

```cron
*/15 * * * * arch-be cron tick >> ~/.arch-harness/reports/cron/tick.log 2>&1
```

- Метка последнего тика — `~/.arch-harness/cron-last-tick` (RFC 3339);
  при первом запуске — «сейчас минус 24 часа».
- Задача дюжная, если ближайшее срабатывание её выражения после метки
  `last` не позже `now` (`cron-parser`); дюжные выполняются последовательно;
  метка перезаписывается после тика.
- Битое cron-выражение (не 5 полей, ошибка разбора) — задача пропускается
  с предупреждением в лог, тик не рвётся. Первая ошибка прогона задачи
  прерывает тик; уже записанные отчёты остаются.

## Прогон задачи и headless JSON-статус

`run_job`: читает `task_md` → локальный цикл «модель ↔ инструменты»
(полный реестр: bash, файлы, kb, web, контроль…; лимит 16 итераций) →
markdown-отчёт `<out>/<name>-<yyyymmdd-HHMMSS>.md`.

Системный промпт исполнителя требует: отчёт в markdown, **последняя строка
— JSON-статус**:

```json
{"status": "complete|partial|blocked", "summary": "…"}
```

Статус извлекается из последней непустой строки (`extract_status`);
не распарсилась — `unknown`. Шапка отчёта: задача, расписание, модель,
дата, длительность, статус. Это тот же headless-контракт, что и у
кодовых харнессов (`docs/harness_integrations.md`): отчёт читается и
человеком, и скриптом (`tail -1 report.md | jq .status`).

### Примеры инструкций (`examples/cron/`)

- **`kb_digest.md`** — дайджест новых материалов базы знаний за сутки:
  `find … -mtime -1` через инструмент `bash`, чтение свежих файлов,
  тематическая группировка, индекс `reports/cron/INDEX.md`; пустые дни
  честно помечаются. Статусы: `partial` — часть каталогов недоступна.
- **`spec_drift.md`** — дрейф-чек спецификаций (гейт A5 по расписанию):
  свежесть `docs/specs/*.md` против свежих коммитов кода; расхождения —
  только со свидетельствами (цитата спеки + файл:строка/хеш коммита);
  классификация `DRIFT`/`STALE-SPEC`/`OK`; прогон `arch-be control spine`.

Пишите свои задачи по тому же образцу: роль дежурного агента, входные
параметры, нумерованные шаги с конкретными командами, формат отчёта,
JSON-статус с правилами выбора `partial`/`blocked`.

## Баш-пайпы

Харнесс дружит с Unix-пайпами — всё headless читает stdin и пишет stdout:

```bash
# Headless-агент на файле: промпт — весь stdin (`-` или просто пайп)
cat docs/specs/payment.md | arch-be run -
cat spec.md | arch-be run --model deepseek-pro --no-stream

# Рендер диаграммы в файл/дальше по пайпу
arch-be mermaid examples/mermaid/flow.mmd > art.txt
cat seq.mmd | arch-be mermaid - | less

# Контроль в CI: exit 1 при FAIL ломает пайплайн
arch-be control check . || exit 1
arch-be control spine docs/ARCHITECTURE-SPINE.md
arch-be control sensors docs/specs

# Отбор отчётов крона по статусу
for f in ~/.arch-harness/reports/cron/*.md; do
  tail -1 "$f" | grep -q '"status": *"blocked"' && echo "BLOCKED: $f"
done
```

Детали: `arch-be run` без аргумента читает stdin, только если stdin — не TTY
(в терминале без пайпа вернёт ошибку «нет промпта»); `--no-stream` печатает
только финальный ответ (удобно в скриптах). В стриминг-режиме stdout несёт
только текст ответа, а прогресс (вызовы инструментов `▶ tool: …` / `✓ …`,
заметки) уходит в stderr — пайп остаётся чистым.

Бюджеты на прогон (страховка CI/cron; превышение таймаута — причина в
stderr и exit 1):

```bash
arch-be run -q --timeout 600 --max-turns 24 "…" > answer.md
```
