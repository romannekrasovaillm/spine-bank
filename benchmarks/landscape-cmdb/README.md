# Бенчмарк AB-02 — сверка ландшафта с выгрузкой CMDB

Проверяет контур `arch-be landscape diff` (ADR-077, волна LANDSCAPE) на живом
клоне PetClinic: сверка модели ⊕ графа кода с выгрузкой CMDB, три класса
расхождений и вердикты записей.

## Что проверяется

Прогонщик `cmdb_diff.py` (только stdlib) прогоняет `landscape diff --format json`
в шести сценариях и сверяет вердикты/находки с ожидаемыми:

| Сценарий | Вход | Ожидание |
|----------|------|----------|
| `clean` | все 8 сервисов заведены | `passed`, 0 ошибок / 0 предупреждений |
| `remove-one` | убран `visits-service` | `cmdb-missing` [error], errors=1 |
| `wrong-owner` | владелец модели ≠ CMDB | `owner-conflict` [error], errors=1 |
| `orphan` | активная система без модели/кода | `cmdb-orphan` [warn], warnings=1 |
| `retired` | та же система со `status: retired` | молчит (0 предупреждений) |
| `unverifiable` | пустой ландшафт (нет источников) | вердикт «не проверяема» |

Сценарии 2–5 требуют модели в ландшафте, поэтому прогонщик сначала пишет
черновик модели из CMDB через `--model-out` (`status: inferred`).

## Запуск

```sh
export ARCH_BE=./target/debug/arch-be            # биарь (иначе PATH / target/)
export ARCH_VALUE_REPOS=~/bench-repos            # корень клонов (дефолт)
python3 benchmarks/landscape-cmdb/cmdb_diff.py --json
```

Без биаря или клона petclinic сценарии помечаются `SKIP` (не `FAIL`) — сеть не
требуется.

## Фикстура

`fixtures/cmdb-petclinic.yaml` — выгрузка CMDB со всеми 8 сервисами PetClinic
(владельцы: Платформа, Интеграции, CRM, AI, CMS, Записи). Пути к бинарю и
корню клонов — через окружение, хардкода машинных путей нет.
