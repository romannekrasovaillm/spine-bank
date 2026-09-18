# Spine-BE SDK — контракт v1 (sdk/CONTRACT.md)

Машиночитаемый контракт между бинарём `arch-be` (Spine-BE) и SDK-клиентами
(Python, Rust, Java). SDK — тонкие клиенты поверх headless CLI: запускают
процесс `arch-be`, читают stdout/stderr и exit code. Никакой сети и RPC.

- Версия контракта: **1** (совпадает с `schemaVersion` receipt'ов Archify CLI).
- Совместимость: добавление новых полей в JSON — не ломающее изменение;
  переименование/удаление полей и смена exit-кодов — ломающее (major).

## 0. Разрешение бинаря и запуск процесса

- Путь к бинарю: env `SPINE_BE_BIN` → иначе `arch-be` из `PATH`.
- Клиент запускает процесс без shell (argv-массив), ловит stdout/stderr,
  применяет клиентский таймаут (по истечении — kill процесса, ошибка таймаута).
- Рабочий каталог (cwd) — параметр клиента; команды, оперирующие репозиторием
  или относительными путями, наследуют его.

## 1. `run` — headless-прогон агента (LLM)

```
arch-be run -q [--model NAME] [--timeout SECS] [--max-turns N] [--think on|off] [PROMPT|-]
```

- Контракт `-q`: **stdout — только финальный ответ ассистента** (произвольный
  текст, не JSON). Прогресс молчит.
- Успех: exit 0, stderr пуст.
- Сбой (провайдер, таймаут `--timeout`, сетевая ошибка): exit 1, причина в stderr.
- Промпт может читаться из stdin (`-`), что удобно для длинных промптов из SDK.

Типизированный результат SDK: `{ answer: string, model?: string, durationMs: int }`.
Ошибки: `BinaryNotFound`, `Timeout` (клиентский), `ProcessFailed{exitCode, stderr}`.

## 2. `control check` — fitness-контроль репозитория

```
arch-be control check <REPO> [--constraints PATH] --json
```

stdout — ОДНА строка JSON (сериализация `FitnessReport`):

```json
{
  "repo": ".",
  "passed": true,
  "summary": "Правил: 3, нарушений: 0 (error: 0, warn: 0)",
  "issues": [
    {"file": "src/bad.py", "line": 1, "rule": "no_pan_in_code",
     "message": "must_not_contain: запрещённый паттерн ...", "severity": "error"}
  ]
}
```

- `severity`: `"error"` | `"warn"`. `line: 0` — находка на файл целиком.
- Exit: 0 — `passed=true`; 1 — `passed=false` (JSON всё равно напечатан!)
  либо ошибка запуска (тогда JSON нет, причина в stderr).
- SDK различает «FAIL по правилам» (валидный JSON, `passed=false`) и
  «ошибку исполнения» (нет JSON / невалидный JSON + stderr).
- Аддитивные поля (присутствуют только в соответствующих режимах):
  `durations[]`, `inherited[]`, `overrides[]`, карточные поля `issues[]`
  (`ad`/`adr`/`rationale`/`owner`/`fix_hint`/`skill`); режим baseline/ratchet
  (`--baseline`) — `baseline: {path, updated, debt[], closed[], debt_total,
  closed_total}`; режим среза (`--changed-since`) — `skipped[]`,
  `changed_since`, `changed_files` (семантика — `docs/control.md`).

## 3. `archify` — диаграммы (validate / deliver / compare)

```
arch-be archify validate <TYPE> <IR_PATH> [--quality standard|showcase] --json
arch-be archify deliver  <TYPE> <IR_PATH> <OUT_HTML> [--quality ...] --json
arch-be archify compare  <BASE_IR> <HEAD_IR> <OUT_HTML> [--quality ...] --json
```

`TYPE`: `architecture|workflow|sequence|dataflow|lifecycle` (compare — только
`architecture`). stdout — pretty-printed JSON-receipt Archify CLI,
`schemaVersion: 1`. Общие поля: `ok: bool`, `command`, `type`.

### 3.1 validate

```json
{"schemaVersion":1,"ok":true,"command":"validate","type":"architecture",
 "input":"/abs/path.json",
 "checks":[{"name":"single_svg","ok":true,"details":["..."]}],
 "composition":{"profile":"showcase","status":"pass",
                "summary":{"errors":0,"warnings":0},"metrics":{...}}}
```

### 3.2 deliver

```json
{"schemaVersion":1,"ok":true,"command":"deliver","type":"architecture",
 "input":"...","output":"...",
 "specification":{"sha256":"...","bytes":7456},
 "artifact":{"sha256":"...","bytes":725537},
 "validation":{...как validate...}}
```

### 3.3 compare

```json
{"schemaVersion":1,"ok":true,"command":"compare","type":"architecture",
 "comparatorVersion":1,"completeness":"complete","proofLevel":"authored",
 "base":{"title":"...","rawSha256":"...","semanticSha256":"...","bytes":7456},
 "head":{...},
 "summary":{"components":{"added":1,"changed":0,"removed":0,"moved":0,"evidenceChanged":0},
            "connections":{"added":2,"changed":0,"removed":0,"rerouted":1},
            "boundaries":{"added":1,"changed":1,"removed":0,"geometryChanged":0},
            "presentationChanged":true,"provenanceChanged":false},
 "changes":{"components":[...],"connections":[...],"boundaries":[...]},
 "artifact":{"sha256":"...","bytes":2077581},"validation":{...}}
```

- Exit: 0 — `ok=true`; при провале (`ok=false`, невалидный IR, ошибка
  аргументов CLI, таймаут) `arch-be` возвращает **1** (код выхода Archify CLI
  при этом идёт текстом в stderr — `arch-be` не пробрасывает его наружу).
- SDK обязан передавать receipt вызывающему коду целиком (типизированы только
  поля этого документа; остальное — как generic JSON).

## 4. Коды ошибок SDK (единые для трёх языков)

| Ошибка | Условие |
|--------|---------|
| `BinaryNotFound` | `SPINE_BE_BIN`/`arch-be` не найден или не исполняемый |
| `Timeout` | клиентский таймаут, процесс убит |
| `ProcessFailed` | ненулевой exit без валидного JSON-контракта; несёт `exitCode` и `stderr` |
| `ContractViolation` | stdout не парсится как JSON там, где контракт требует JSON |
| `CheckFailed` | (не ошибка исполнения) результат `control check` с `passed=false` — передаётся как данные, НЕ как исключение |

## 5. Эталонные фикстуры для тестов SDK

- IR диаграмм: `banking/demos/cli-from-claude-code/scenario2-archify-cli/sbp-v1.architecture.json`, `sbp-v2.architecture.json`.
- Гейт-фикстура: `banking/demos/cli-from-claude-code/scenario3-gate/fixtures/` (зелёная; красный кейс — добавить файл с 16-значным числом во временную копию).
- LLM-прогоны (`run`) в автотестах SDK НЕ выполняются (стоимость/время);
  тестируется только построение argv и разбор ответа на моке.
