# Отчёт по завершении TASK-Spine-Core-0.3.12

Дата: 2026-10-04. База: v0.3.11 (`7dbf13f`). Все волны A–D закрыты;
решения человека приняты по C4, D3, ADR-055/056/057 (выборы владельца
2026-10-03/04). Репродукции «до фикса — зелёный, после — красный» —
`docs/experiments/fork-repro-0.3.12.md` (RA-1…RA-10).

## Пункт → коммит → тест, который падал до фикса

| Пункт | Коммиты | Тест/репродукция, падавшая до фикса |
|---|---|---|
| A1 тело правила в rule_weakened | `60988e7` | RA-1 (репродукция сквозная): замена command→true, сужение glob, мёртвый pattern, рост timeout — до фикса `rule_weakened` PASS; после — `BodyChanged` |
| A2 override только принятым ADR | `fe92ec4` | RA-1: выдуманный ADR-999 + until 2099 узаконивали ослабление; после — `override_adr_missing`/`override_horizon_exceeded`, гейт `exit 1` |
| A3 пины контрольной плоскости | `30b90f8`, `2fb8791` | RA-4/RA-4b: синхронное ослабление копий реестра и удаление Stop-хука зелёнили гейт; после — `control_plane_tampered` |
| A4 пост-гейт harness_run | `1c1e487`, `73b649c`, `5ed2da7` | RA-6: токсичный прогон (правки без покрытия дельтой) оставался «зелёным» итогом; после — вердикт пост-гейта в первой строке, красный = ошибка прогона |
| A5 одобрение извне диапазона | `4fd5e4c` (ADR-055), `b60bbe9`, `d6f7b2a` | RA-7: настоящий ADR + своя дельта узаконивали ослабление из агентского прогона; после — одобрение только из диапазона прогона исполнителя |
| B1 релиз с зелёного CI | `42357eb` | workflow: тег на красном коммите публиковал релиз; после — проверка CI-статуса до публикации |
| B2 SBOM/хэши от той же сборки | `5820b37` | SBOM/SHA256SUMS собирались из другого артефакта, чем публикуемые бинари; после — единая джоба сборки |
| B3 provenance/подпись | `971f2a2`→`9fd50b8` (ADR-056), `568b84a` | релизные артефакты без provenance/подписи; после — SLSA + keyless cosign, инструкция в docs/supply-chain.md |
| B4 гигиена workflows | `df01667` | actions по mutable-тегам, избыточные permissions; после — SHA-пины, минимальные permissions, cargo deny |
| B5 CODEOWNERS | `9259318` | защищённые пути без владельцев; после — CODEOWNERS + фитнес-правила dogfood |
| C1 plugins.lock | `2de11d7` (волна C), `2fbee9f` | RA-8: плагин подменялся без обнаружения; после — sha256-реестр, `plugin_tampered`, doctor-проверка |
| C2 чистое окружение на Critical | `2de11d7` | RA-9: окружение хоста (в т.ч. ключи) протекало в исполнителя; после — env-allow дефолт на Critical/bank_profile |
| C3 секреты как составляющая гейта | `2de11d7` | RA-10: хардкод-креденшл в коде целевого репо невидим гейту; после — `secret_literal` в пост-гейте A4 |
| ACP (ADR-057, сверх TASK) | `dc84711`, `ec5af25`, `34fe43b` | юнит-фикстура ACP + живой тест: initialize→fallback на qwen/claude-code-acp; полный prompt-turn по ACP подтверждён на openclaw acp (режим acp, контракт complete, 12.6 с); F2 уточнён живым зондом (authMethods фатальны только при authenticate-cap) |
| D1 bitbucket | `90e6530`, `814ae51` | `connect_ci_bitbucket_writes_insights_jenkinsfile`, `bitbucket_insights_json_has_result_and_annotations`; пути REST [ТРЕБУЕТ ПРОВЕРКИ] при внедрении |
| D2 теневой гейт | `c0cde87` | `shadow_diff_reports_new_rule_and_severity_change_without_touching_main` — падал первым: warn-находка зелёной fitness невидима полному прогону; решено диффом движка проверки реестра (подтверждено архитектором) |
| D4 runtime fitness | `7c346b3` | `metrics_verify_flags_latency_and_availability_with_guilty_hop`; отчёт docs/experiments/runtime-fitness.md |
| D0 docs (GAP-реестр, CHANGELOG) | `8e54e09`, `b655c60`, `aa6226b` | threat-model §5 дополнен строками «плагины» (C1) и «контрольная плоскость» (A3) — чек-лист §7 |

## Что не воспроизвелось / честные границы

- RA-5 (`dependency_direction` на больших реестрах патологически дорог):
  зафиксирован как сигнал (TIMEOUT пост-гейта честен), не блокер механизма.
- Точные пути/глаголы Bitbucket DC REST (Insights, build-status) —
  [ТРЕБУЕТ ПРОВЕРКИ] при внедрении против версии DC заказчика: группа API
  подтверждена по developer.atlassian.com, JS-страницы эндпоинтов
  недоступны веб-инструменту.
- claude-code-acp в live-контуре требует OAuth `claude /login` (отвечает
  -32000 на session/prompt при работающем headless через env): acp-секция
  адаптера на хосте отключена до логина (конфиг-комментарий), headless не
  задет.
- Полная обработка stopReason max_tokens/refusal в ACP — требует расширения
  Termination/контракта результата, отдельный срез (открыто).
- TCK против ACP-фикстуры — опционально, не прогонялся (ключи/опции — из
  README acp-tck при решении).

## Пункты решений человека (все закрыты)

| Пункт | Решение | ADR/фиксация |
|---|---|---|
| A5 | вариант (а) «диапазон прогона» | ADR-055 Accepted, выбор 2026-10-03 |
| B3 | вариант (а) GitHub-native SLSA | ADR-056 Accepted, выбор 2026-10-03 |
| C4 | отложить (sha256-аттестация остаётся) | ADR-058 Deferred, выбор 2026-10-04, карточка в ROADMAP |
| D3 | секция `deployment:` в CONSTRAINTS.yaml | ADR-059 §D3, выбор 2026-10-04, реализация — следующая волна |

## Чек-лист §7 — статус

- [x] Сквозная репродукция §3: RA-1 зелёный до / `exit 1` после
- [x] A1 `BodyChanged`, ужесточения чисты
- [x] A2 override без ADR не узаконивает; горизонт ограничен
- [x] A3 `control_plane_tampered` ловит удаление Stop-хука
- [x] A4 пост-гейт в harness_run; красный — код 1
- [x] A5 ADR-055 принят, вариант реализован
- [x] B1–B2 релиз/CI, SBOM/хэши от опубликованных бинарей
- [x] B3 ADR-056, provenance/подпись в release.yml, docs/supply-chain.md
- [x] B4–B5 SHA/actions, permissions, deny, CODEOWNERS — под dogfood
- [x] C1–C3 plugins.lock, env-дефолт Critical, составляющая secrets
- [x] C4, D1–D4: ADR (058 Deferred / 059 Accepted) + код D1/D2/D4 за флагами
- [x] threat-model §5 обновлён (плагины, контрольная плоскость, поставка)
- [x] CHANGELOG «что может покраснеть»: override_adr_missing,
      override_horizon_exceeded, BodyChanged, control_plane_tampered,
      пост-гейт harness_run, env_allow на Critical (+ волны D: нейтральность)

Сверх задания: ACP-режим вызова агентов (ADR-057, вариант «а») —
реализован, верифицирован юнит-фикстурой и живым прогоном; решения по
открытым вопросам исполнителей зафиксированы в дельтах.
