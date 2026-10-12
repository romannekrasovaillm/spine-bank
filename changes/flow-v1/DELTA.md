# Дельта: flow-v1
- Route: Standard (significance 2: new_component + api_contract_change; ADR-081)
- Created: 2026-10-12

## Проблема

Д-2 «Сценариев v7»: поток КА ФО — центральный объект работы SA — не выражается
в модели. Принадлежность ИВ потоку и внешние идентификаторы (ИВ МЕТЫ, API
реестра, AGR) живут в тексте карточек и не проверяются; карточки формата v7
с этими полями в шапке не загружаются вовсе (SA-Bench: 1 из 7 эталонных
грузится). Решение — ADR-081: сущность FLOW + опциональные поля шапки INT
(meta_id/registry_api/agreement) + пять проверок model_validate.

## ADDED

- When в model/ есть карточка `FLOW-nn` (type: flow), the система shall
  загрузить её как сущность с полем `integrations: [INT-…]` и включить в
  реестры/пейзаж/impact наравне с прочими видами.
- When INT несёт `meta_id`, the система shall проверить формат (`ИВ-<цифры>`
  или локальный вариант) и уникальность в рамках модели (дубль — error
  `int-meta-id-duplicate`).
- When INT несёт `registry_api`, the система shall проверить формат
  `Имя vMAJOR.MINOR` (error при нарушении). Сверка с внешним реестром — Д-5,
  не в этой волне.
- When INT несёт `agreement`, the система shall проверить существование
  файла (error `agreement-missing`, по образцу contract/ADR-035).
- When FLOW ссылается на несуществующий INT, the система shall дать error
  `flow-int-missing`; When FLOW без integrations — warn `flow-empty`.
- When INT с бюджетом/контрактом не входит ни в один FLOW — warn
  `int-without-flow` (зеркало budget-hop-uncovered) в nfr budget/trace.
- Подсказка unknown-field (П5 0.4.8) обновляется: поля
  flow/meta_id/registry_api/agreement доступны в шапке.

## MODIFIED

- `src/model.rs` `ID_PATTERN`/`EntityKind` (+Flow, 12-й префикс) — полный
  прогон точек касания (grep `EntityKind`/префиксы: landscape, impact,
  graph, recover draft, MCP-справка, bootstrap) — каждая точка либо осмысленно
  поддерживает FLOW, либо явно исключает с причиной.
- `src/model/parse.rs` allowed-поля INT (+meta_id/registry_api/agreement).
- Тестовая матрица модели: карточки приложения З.2.3 v7 грузятся без правок
  (фиксация боли SA-Bench как регрессионный тест).

## REMOVED

- Ничего. Существующие модели валидны байт-в-байт (поля опциональны).

## План отката

Поля опциональны, FLOW — новый префикс: откат — удалить Flow из EntityKind и
поля из parse; модели с FLOW после отката дают явный unknown-prefix error
(не тихий). git revert коммитов дельты.

## Критерии приёмки

- [ ] Карточка FLOW-001 + INT с meta_id/registry_api/agreement грузятся;
      З.2.3-фикстура (7 карточек v7) — 7/7 без правок.
- [ ] Пять проверок: flow-int-missing/int-meta-id-duplicate/
      int-meta-id-format/agreement-missing (error), flow-empty/
      int-without-flow (warn) — тест на каждую.
- [ ] Существующие кейсы (кейсы/salary-payments и др.) — валидны без правок.
- [ ] Перечисления kinds прогнаны: landscape/impact/graph/recover/MCP-справка
      не падают на FLOW; справка MCP синхронизирована (тест реестра).
- [ ] cargo test зелёный; fmt/clippy/MSRV 1.85 чисты; CHANGELOG с «Что может
      покраснеть» (смена текста подсказки П5 — парсеры заметят; новые
      коды находок).
