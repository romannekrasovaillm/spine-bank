# Дельта: wave-d-post-gate

- Route: Standard (significance: cross_domain_integration + new_vendor;
  ADR-059, рамка TASK-0.3.12 §6 «прототипы за флагами»)
- Created: 2026-10-04
- ADR: docs/adr/ADR-059-sloi-posle-geyta-volna-d-tenevoy-geyt-bitbucket-code-insights-runtime-fitness-prototipy-za-flagami.md
  (Accepted — D1/D2/D4); D3 — отдельно, [РЕШЕНИЕ ЧЕЛОВЕКА], вне этой дельты

## Проблема

Слои после гейта в продукте почти нет (TASK §0 п.4): вердикт не доезжает до
merge checks Bitbucket Data Center заказчика; эволюция реестра правил
(пин `extends`, правки CONSTRAINTS.yaml) идёт вслепую — раздел CHANGELOG
«что может покраснеть» пишется руками и не сверяем; NFR-бюджеты из модели
не сверяются с фактическими метриками после деплоя.

## ADDED

- When гейт вызван с `--shadow-constraints <файл>`, the гейт shall
  вычислить вердикт дважды: решающий — по текущему реестру (поведение
  байт-в-байт прежнее: exit-код, gate-verdict/v1, обязательные
  составляющие), теневой — по файлу-кандидату; в отчёт добавляется блок
  shadow: появляющиеся/исчезающие находки и смены severity по id правила,
  сводка «что покраснеет при переходе».
- When теневой файл не парсится, the гейт shall записать в shadow-блок
  находку `shadow_constraints_invalid` и НЕ менять основной вердикт.
- When `control report --level corp` собирает флотовой отчёт по проектам с
  shadow-результатами, the отчёт shall агрегировать счётчики появляющихся
  находок по правилам (срез «что покраснеет по флоту»).
- When `connect ci --provider bitbucket` вызван, the генератор shall
  выдать маркерный Jenkinsfile-блок (по образцу Jenkins-провайдера) с
  curl-публикацией вердикта в Code Insights
  (`/rest/insights/1.0/projects/<proj>/repos/<repo>/reports/<key>`) и
  build-status API — сеть только в CI-скрипте заказчика, в Rust-коде
  сетевых крейтов НЕ появляется (Core-граница C-34).
- When гейт запущен с `--format bitbucket-insights`, the гейт shall
  сериализовать вердикт в JSON-отчёт Code Insights (key, title, result
  PASS/FAIL, description, findings как data-аннотации с file/line, где
  находка их несёт).
- When `nfr verify --metrics <файл>` вызван, the подкоманда shall сверить
  метрики из JSON-файла (p99 по операциям, availability, error rate) с
  бюджетами/SLA из модели (nfr budget/availability) и выдать находки
  расхождений с виновными hop/звеньями; без сети; источник — файл, не
  Prometheus-клиент.

## MODIFIED

- `src/connect/ci.rs`: `CiProvider` + `Bitbucket` (генерация по существующему
  паттерну маркеров; другие провайдеры не меняются).
- CLI гейта: флаги `--shadow-constraints`, `--format bitbucket-insights`
  (аддитивно; без флагов поведение прежнее).
- `docs/threat-model.md` §5: добавить строки GAP «плагины» (закрыт C1,
  plugins.lock, 2026-10-04) и «контрольная плоскость» (закрыт A3,
  control_plane_tampered, 2026-10-04) — чек-лист TASK §7.
- `CHANGELOG.md`: раздел «Что может покраснеть» — новые флаги и их
  нейтральность к дефолту.

## REMOVED

- Ничего.

## План отката

Всё за флагами/генераторами, дефолт байт-в-байт прежний. Откат — `git
revert` коммитов волны D; миграций и данных нет. Shadow-результаты и
Insights-отчёты — артефакты прогона, не состояние.

## Критерии приёмки

- [ ] D2: тест — репозиторий с находкой X-1 (must_not_contain), теневой
      реестр добавляет X-2 и ужесточает X-1 до error: основной вердикт и
      exit-код НЕ изменились, shadow-блок показывает X-2 (new) и X-1
      (severity change); без флага — вывод байт-в-байт прежний.
- [ ] D2: теневой файл с YAML-ошибкой → `shadow_constraints_invalid`,
      основной вердикт зелёный.
- [ ] D2: `control report --level corp` на фикстуре двух проектов с
      shadow-результатами агрегирует счётчики по правилам.
- [ ] D1: `connect ci --provider bitbucket` в пустом каталоге пишет
      Jenkinsfile-блок с маркерами; повторный вызов обновляет блок внутри
      маркеров, правки вне маркеров сохраняются (как у Jenkins); в
      сгенерированном скрипте — curl к /rest/insights/1.0 и build-status
      (точные пути API [ТРЕБУЕТ ПРОВЕРКИ] по документации Atlassian —
      пометка в комментарии скрипта).
- [ ] D1: `--format bitbucket-insights` на фикстуре вердикта с находками
      даёт валидный JSON с result=FAIL и аннотациями; PASS-вердикт —
      result=PASS.
- [ ] D4: `nfr verify --metrics` на фикстуре кейса с моделью (бюджет p99,
      SLA): расхождение → находка с виновным hop; совпадение — PASS;
      docs/experiments/runtime-fitness.md — дизайн, границы, пример
      прогона, черновик канареечного шаблона из QAS-*/NFR-*.
- [ ] Тесты существующих провайдеров/гейта зелёные; `cargo fmt --check &&
      cargo clippy --all-targets -- -D warnings && cargo test && cargo test
      --no-default-features --features core` — чисто.
- [ ] docs/threat-model.md §5: строки «плагины» и «контрольная плоскость»
      со статусами закрытия (C1/A3) — сверка с чек-листом TASK §7.
- [ ] CHANGELOG: «что может покраснеть» — shadow/insights/nfr-verify
      нейтральны к дефолту.

## D3 — вне дельты

Экспорт политик kyverno/rego ждёт [РЕШЕНИЕ ЧЕЛОВЕКА] по формату
CONSTRAINTS.yaml (новый тип правила vs секция `deployment:`) — ADR-059 §D3;
после выбора оформляется отдельной дельтой.
