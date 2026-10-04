# Дельта: policy-export-deployment

- Route: Standard (significance: new_component + api_contract_change; выбор
  владельца 2026-10-04 — секция `deployment:` в CONSTRAINTS.yaml, ADR-059 §D3)
- Created: 2026-10-04
- ADR: docs/adr/ADR-059-sloi-posle-geyta-volna-d-tenevoy-geyt-bitbucket-code-insights-runtime-fitness-prototipy-za-flagami.md
  §D3 (решение владельца: отдельная секция, НЕ новый тип правила)

## Проблема

Инварианты развёртывания (образы только из внутреннего реестра и подписанные,
non-root, лимиты ресурсов, deny-список образов) живут вне CONSTRAINTS.yaml —
моста «спайн → политики кластера» нет (прецедент экспорта — ArchUnit,
ADR-039). Экспорт вручную дрейфует от реестра.

## ADDED

- When CONSTRAINTS.yaml содержит секцию `deployment:`, the все читатели
  реестра (gate, fitness, rules_report, delta guard, handoff-копия) shall
  разбирать её ТОЛЕРАНТНО (serde default, неизвестных полей top-level не
  бояться) и НЕ изменять своё поведение: гейт её игнорирует — правила
  кода не смешиваются с инвариантами деплоя.
- When вызвана `arch-be policy export <kyverno|rego>` (без
  `--deployment`-инвариантов — сообщение «секция deployment: не найдена,
  экспортировать нечего», exit 0 с отчётом), the экспортер shall
  сгенерировать детерминированные политики из секции `deployment:`.
- Схема секции (serde-структура, все поля опциональны, кроме
  идентифицирующих):
  `deployment.images.registry` (строка — разрешённый префикс реестра),
  `deployment.images.signed` (bool — подпись образа обязательна),
  `deployment.security.run_as_non_root` (bool),
  `deployment.resources.max_cpu` / `max_memory` (строки, k8s-кванторы),
  `deployment.deny_images` (список строк — tech-радар deny_dependency для
  образов).
- Kyverno-выход: один или несколько манифестов `apiVersion:
  kyverno.io/v1, kind: ClusterPolicy` с validate-правилами по группам
  инвариантов (image-реестр/подпись → validate + verifyImages при signed,
  securityContext.runAsNonRoot, limits, deny_images → deny-список).
- Rego-выход: один файл `package archbe.deployment` с правилами по
  инвариантам (те же группы, deny/allow семантика).
- `--output <файл>` (stdout по умолчанию); форматирование стабильное
  (golden-тесты).

## MODIFIED

- CLI: новая подкоманда `policy` (`arch-be policy export …`) — аддитивно.
- CHANGELOG: раздел «Что может покраснеть» — секция `deployment:` и
  подкоманда нейтральны к дефолту (гейт поведение не меняет).

## REMOVED

- Ничего.

## План отката

`git revert` коммитов дельты; миграций нет. Секция `deployment:` в
целевых репозиториях опциональна — без неё поведение идентично прежнему.

## Критерии приёмки

- [x] CONSTRAINTS.yaml с `deployment:` не ломает gate/fitness/rules_report/
      handoff (тест: фикстура с секцией проходит гейт байт-в-байт как без
      неё).
- [x] `policy export kyverno` на фикстуре с полным набором инвариантов —
      валидные YAML-манифесты ClusterPolicy (golden-тест); пустая/
      отсутствующая секция — сообщение, exit 0.
- [x] `policy export rego` — валидный Rego (golden-тест) из той же секции.
- [x] Детерминизм: повторный экспорт — идентичный вывод (тест).
- [x] Схема задокументирована (docs/ или справка CLI); пример секции в
      config.example.toml НЕ нужен (это формат целевых репозиториев) —
      пример в docs/policy-export.md или справке.
- [x] fmt/clippy/test + core-сборка зелёные; новый код в границе C-34
      (без сетевых/TUI-крейтов).

## Приёмка (2026-10-04, архитектор)

Коммит d1dd7c2, merge ac54588. Верифицировано независимо: fmt/clippy чистые,
1802+82+56+1 (default) и 5/5 (core) зелёные. Сверх критериев: политики
проверены на реальных движках (kyverno apply 1.19.1 — нарушитель падает,
compliant проходит; opa check/eval 1.21.1 — deny непуст на нарушителе).

Ответы на открытые вопросы:
- **public_key:** ДА — расширяем схему полем `deployment.images.public_key`
  (ref на секрет/ключ), чтобы verifyImages не требовал ручной замены
  плейсхолдера; маленький срез следующей волной (вместе с конформностью
  ACP-фикстуры).
- **Rego digest-pinning:** достаточно для v1-экспорта: plain Rego не
  проверяет cosign-подписи, криптопроверка — зона Kyverno verifyImages;
  граница зафиксирована в docs/policy-export.md. Полноценная проверка
  подписей в Rego — только при живом требовании пилота.
- **kyverno.io/v1 → CEL:** остаёмся на v1 (deprecated ≠ removed); миграция
  на policies.kyverno.io — отдельной дельтой по требованию пилота на
  Kyverno 1.19+.
