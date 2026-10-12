# Дельта: recover-indirect
- Route: Standard (score 2: new_component + api_contract_change; ADR-082)
- Created: 2026-10-12

## Проблема

Слабейшее семейство ArchBench — dochub-recover: F1(observed) 0.48 против
0.70 у брифа. Детекторы calls (ADR-069/R2) читают имя цели только из
артефакта потребителя (env/маршрут/стаб/топик); реальные ландшафты проводят
связи через ConfigMap-ref, Helm values/overlay и build-args — recover их не
видит. Решение — ADR-082: три детектора косвенной проводки + аддитивное поле
`indirect` у ArchEdge (enum Via не трогаем — recover/v1 заморожен аддитивно).

## ADDED

- When манифест k8s ссылается на ConfigMap (envFrom/valueFrom configMapRef),
  а в data ConfigMap есть URL/host/endpoint, резолвящийся в компонент
  репозитория, the система shall построить ребро calls с evidence обеих
  сторон (манифест:строка + configmap:строка), confidence=inferred,
  `indirect.layer=configmap`.
- When values[-<env>].yaml содержит ключ вида `<svc>.host|url|endpoint|addr`
  (или вложенный `<svc>: {host: …}`), резолвящийся в компонент, the система
  shall построить ребро с `indirect.layer=helm_values`; values-<env> даёт
  condition EnvOverlay (ADR-075, ортогонально косвенности).
- When Dockerfile ARG *_ADDR|*_URL|*_HOST|*_ENDPOINT связан с compose
  build.args/CI-env, the система shall построить ребро с
  `indirect.layer=build_arg`, потребитель — каталог Dockerfile.
- When попадание одностороннее (ConfigMap есть, рефов нет), the система
  shall отдать его в gaps (не ребро).
- `ArchEdge.indirect: Option<Indirect{layer, key}>` — serde skip-if-none,
  старые графы/бинари байт-в-байт прежние; рендеры md/mermaid помечают
  «· configmap», draft_model — INT с источником.
- Приёмка: фикстура «Кварц Indirect» (2 сервиса, связь только через
  ConfigMap + values-<env> + build-arg, 6–8 целевых рёбер): до волны 0,
  после ≥ 6/8 с evidence обеих сторон; условные рёбра — recall не ниже
  прямой проводки.

## MODIFIED

- `src/arch_diff/calls.rs` (или новый подмодуль indirect.rs): резолвер
  кандидатов R2 переиспользуется; ассоциация «манифест → компонент» — по
  DeployTopology/selector/имени, каждая — в evidence.
- `src/report/` (ADR-080): §11 развёртывание получает indirect-факты;
  §14.4 — ограничение «без рендера Helm часть рёбер невидима».

## REMOVED

- Ничего. Via-enum и существующие детекторы не меняются.

## План отката

Поле опционально, детекторы аддитивны: git revert коммитов дельты; графы
без косвенных рёбер байт-в-байт прежние (контрактный тест).

## Критерии приёмки

- [ ] Фикстура «Кварц Indirect»: 6/8 целевых рёбер с evidence обеих сторон,
      все confidence=inferred, layers корректны; односторонний ConfigMap —
      в gaps.
- [ ] Контракт: recover/v1 JSON без косвенных рёбер байт-в-байт прежний
      (существующие golden-тесты зелёные).
- [ ] condition EnvOverlay на values-<env> ребрах; §14.4 отчёта report
      упоминает границу метода.
- [ ] cargo test зелёный; fmt/clippy/MSRV 1.85; CHANGELOG с «Что может
      покраснеть» (новые рёбра в recover/draft_model на k8s-репозиториях —
      пайплайны, считавшие рёбра, увидят больше).
