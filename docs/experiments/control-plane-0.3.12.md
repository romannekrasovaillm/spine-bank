# Аудит контрольной плоскости 0.3.12 (A3, этап «без кода»)

Контрольная плоскость — файлы, которые решают, **работает ли гейт**: что
проверяется, с какими порогами, кем и когда запускается, по какому
реестру. Критерий включения в перечень: правка файла исполнителем
меняет вердикт гейта или выключает его запуск — без правки кода.

Код-источник каждой записи указан; все пути проверены по `src/` на
`f4edf3d`. Задание: TASK-Spine-Core-0.3.12.md §A3; репродукции ослабления —
`fork-repro-0.3.12.md` (RA-1).

## 1. Перечень

### 1.1 Хуки хоста: `.claude/settings.json`

- **Stop** — всегда при `connect claude` (`src/connect/hooks.rs:135`):
  единый гейт перед завершением сессии, exit 2 агенту при ненулевом
  `arch-be gate` (`hooks.rs:39–67`).
- **PostToolUse** (matcher `Edit|Write|MultiEdit`) — только под
  `--strict-hooks` (`hooks.rs:69–90`).
- Гард fail-soft: `command -v arch-be` — нет бинаря в PATH → молчаливый
  пропуск (PATH — окружение исполнителя).
- Маркер принадлежности: `# spine-connect:stop` / `:post-tool-use`.

### 1.2 Git-хуки: `.git/hooks/pre-commit`, `.git/hooks/pre-push`

`connect git-hooks` (`src/connect/git_hooks.rs`): pre-commit — быстрый
`arch-be control check .`, pre-push — полный гейт. **Каталог `.git/hooks`
не версионируется git** — правка/удаление хуков невидимы ни git-diff, ни
delta guard, ни какому-либо review-процессу.

### 1.3 CI-файлы площадок

`connect ci --provider …` (`src/connect/ci.rs:57–59`):

| Провайдер | Файл | Блок |
|---|---|---|
| GitLab | `.gitlab-ci.yml` | `spine-connect:begin/end`, артефакт codequality |
| GitHub | `.github/workflows/spine-gate.yml` | отдельный workflow, SARIF |
| Jenkins | `Jenkinsfile` | `spine-connect:begin/end`, junit |

Блок идемпотентен по маркерам (`ci.rs:13–15`). Удаление блока/файла
отключает гейт на мерже — текущий `rule_weakened`/`delta guard` этого не
видят (файлы не в `DEFAULT_PROTECTED`).

### 1.4 Конфиг кейса: `arch-harness.toml` (корень)

Пишется `bootstrap` (`src/bootstrap.rs:729`), резолвится
`--config → ./arch-harness.toml → ~/.config/…` (`src/config.rs:1546+`).
Решают вердикт:

- `[policy] autonomy` R0–R5 (`config.rs:654–659`, дефолт R2) — уровень
  автономии агента;
- `[gate.required]` по маршрутам — состав обязательных составляющих
  (выключение `rule_weakened`/`delta_guard` из required — ослабление);
- `require_qualified_judge` (`config.rs:1018`) — порог независимости
  судьи (ADR-048);
- пороги `decision_quality`/`semantic_quality` (`config.rs:1021–1035`).

### 1.5 Пакет передачи `.arch-handoff/`

- **`CONSTRAINTS.yaml`** — копия реестра. Резолвер гейта читает её
  **первой**, корневую — fallback; расхождение обеих — drift-находка
  (`src/control/rules.rs:155–180`, `if handoff_exists { (handoff, root) }`).
  Исполнитель, правящий пакетную копию, правит то, что проверяет его
  собственный Stop-хук.
- **`RUBRIC.yaml`** — критерии и якоря судейства handoff-пакета
  (scale_max, веса) — понижение планки судьи.
- **`TASK.md` / `SPEC.md`** — критерии приёмки и инварианты: их правка
  исполнителем переписывает условия приёмки собственной работы.
- **`ROLLBACK.yaml`**, **`rule-templates.lock`** (поставка шаблонов правил,
  `src/rule_templates.rs:44`).
- `mcp-calls.jsonl` — журнал вызовов (evidence, не конфиг; в периметр
  пинов не входит, см. §3).

### 1.6 Что вне периметра

`MANIFEST.json` (метаданные пакета: route, baseline_commit, rollback_plan;
`src/handoff.rs:301–322`) — сегодня не содержит пинов контрольной
плоскости: поля `control_plane` нет. Ключи/секреты и `.env` — не
контрольная плоскость (защита вывода — `src/secrets.rs`, волна C).

## 2. Находки

- [error] CP-1 — `.arch-handoff/CONSTRAINTS.yaml` вне `DEFAULT_PROTECTED`
  (`src/delta.rs:246`: `["model/", "ARCHITECTURE-SPINE.md",
  "CONSTRAINTS.yaml"]`; матчинг — точный файл или каталог-префикс,
  `delta.rs:440+`), при этом читается гейтом первой → правка пакетной
  копии меняет проверяемый реестр без дельты → пинуть в `control_plane`,
  добавить в защищаемые пути дельта-гейта.
- [error] CP-2 — `.claude/settings.json` вне защиты: удаление Stop-хука
  (или всего файла) невидимо → пин в `control_plane` (задание A3 прямо
  требует тест на удаление Stop-хука).
- [error] CP-3 — git-хуки в `.git/hooks/` вне версионирования: недоступны
  git-базированным гейтам в принципе → покрываются только пинами
  `control_plane` (хэш по пути), не delta guard.
- [error] CP-4 — `arch-harness.toml` вне защиты: autonomy R2→R5,
  выключение составляющих из `[gate.required]`, `require_qualified_judge =
  false`, пороги — правятся без дельты → пин в `control_plane`.
- [error] CP-5 — `RUBRIC.yaml`, `TASK.md`, `SPEC.md` пакета правятся
  исполнителем свободно (критерии приёмки своей работы) → пин в
  `control_plane`.
- [error] CP-6 — Stop-хук fail-soft по `command -v arch-be`: исполнитель
  с контролем PATH глушит последнюю линию молча → закрывается A4
  (пост-гейт в `run_harness` вне окружения исполнителя), не пинами.

В самом репозитории spine-bank сейчас: `.claude/settings.json`,
`.gitlab-ci.yml`, `Jenkinsfile` отсутствуют, CI рукописной без блоков
spine-connect — контрольная плоскость проявляется в кейсах после
`connect`/`bootstrap`, поэтому пины должны сниматься по факту файлов
кейса, а не по фиксированному списку «обязательных».

## 3. Решение для имплементации A3

1. `handoff_create` пишет в `MANIFEST.json` поле `control_plane`:
   `{путь: sha256}` по факту существования. Обязательный минимум с пином
   отсутствия: `.claude/settings.json`, `arch-harness.toml`,
   `.arch-handoff/CONSTRAINTS.yaml`, `.arch-handoff/RUBRIC.yaml`,
   `.arch-handoff/TASK.md`, `.arch-handoff/SPEC.md`,
   `.arch-handoff/ROLLBACK.yaml`. Опционально по наличию: git-хуки
   (`.git/hooks/pre-commit`, `.git/hooks/pre-push`), CI-файлы
   (`.gitlab-ci.yml`, `.github/workflows/spine-gate.yml`, `Jenkinsfile`),
   `.arch-handoff/rule-templates.lock`.
   Пин отсутствия: файла не было при выдаче пакета; появление — тоже
   расхождение.
2. Новая составляющая гейта `control_plane`: сверка текущих sha256 с
   пинами; расхождение без активной дельты, упоминающей файл —
   `[error] control_plane_tampered — <файл> изменён после выдачи пакета →
   вернуть файл или оформить дельту`. Нет `MANIFEST.json` — честный SKIP.
   Delta guard для `.arch-handoff/CONSTRAINTS.yaml` дополняет (файл
   версионируется — добавляется в защищаемые пути), git-хуки — только
   пинами (CP-3).
3. Тест приёмки: удаление Stop-хука из `.claude/settings.json` между
   выдачей пакета и прогоном → `exit 1` от гейта; понижение порога в
   `RUBRIC.yaml` → `exit 1`.

Границы: кейс без handoff-пакета остаётся без пинов (MANIFEST нет — SKIP);
пины защищают от подмены после выдачи, но не от слабой исходной
конфигурации; `mcp-calls.jsonl` не пинится (журнал растёт легально).
