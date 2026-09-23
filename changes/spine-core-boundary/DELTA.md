# Дельта: граница будущего ядра spine-core под гейтом (волна B2 ревью 0.3.6)

## Что меняется

- `CONSTRAINTS.yaml` — новое правило `C-34 core_boundary_no_tui_net`
  (`command_succeeds`, critical, AD-1): прогон `bash scripts/check_core_deps.sh`
  — падает, если дерево зависимостей слим-профиля
  (`cargo tree --no-default-features --features core -e normal`) содержит
  опциональные harness-крейты (`reqwest`, `ratatui`, `crossterm`, `arboard`,
  `scraper`, `tokio-util`; `tokio` — неопциональная зависимость ядра,
  допустима). Состав и смысл остальных правил не менялись, ослаблений нет.
- `scripts/check_core_deps.sh` — новая исполняемая проверка границы
  (по образцу `scripts/check_file_length.sh` из C-33).
- `docs/SPINE-CORE-API.md` — новый документ: что считается стабильной
  поверхностью будущего ядра `crates/spine-core`, что остаётся продуктовым,
  точки расширения доменов через трейты, политика semver и машинных
  контрактов (`arch-be/gate-verdict/v1`, exit-коды 0/1/3).
- `docs/adr/ADR-054-vydelenie-yadra-spine-core-v-cargo-workspace.md` — план
  физического workspace-сплита на 0.4.0 (Status: Proposed; схема префиксов
  ADR-CORE/ADR-BE зафиксирована как предложение, подлежащее ратификации
  архитектором — пункт задания помечен [РЕШЕНИЕ ЧЕЛОВЕКА]).
- Код `src/` НЕ меняется: волна документационно-гейтовая, физический сплит
  — отдельный релиз 0.4.0 по ADR-054.

## Зачем

Задание 0.3.7 (волна B2): инверсия «Spine без собственной LLM» дала
фичу `core`, но граница «ядро без TUI/сети» держалась дисциплиной
feature-флагов и не была проверяемой. Для выделения `crates/spine-core`
(план 0.4.0) граница обязана быть чистой и удерживаемой гейтом, а не
памятью — иначе сплит превращается в перепроводку фич по всему дереву.
Правило critical-уровня: загрязнение core-профиля должно краснеть в CI,
а не в отчёте.

## Чем подтверждается

- `cargo tree --no-default-features --features core -e normal` — harness-крейтов
  нет (depth-1 вывод вложен в ADR-054); `bash scripts/check_core_deps.sh` —
  зелёный.
- Контрпроба: при временном добавлении `tokio` в список запрещённых скрипт
  краснится (exit 1, назван нарушитель, ложных срабатываний на
  `tokio-macros` нет); после отката — снова зелёный.
- `arch-be control check . --constraints CONSTRAINTS.yaml` — PASS, включая
  C-34 (50 правил).
- Гейты репозитория зелёные: `cargo fmt --check`, `cargo clippy
  --all-targets -- -D warnings` (default и core-профили), `cargo test`
  (default и core, суммы тестов неизменны: код не тронут).

## Ссылки

Задание 0.3.7 (волна B2), ADR-054 (план сплита), ADR-053 (модель доверия
`command_succeeds` — тип правила C-34), AD-1 (тонкое ядро),
`docs/INVERSION.md` (шаг 4 — cargo-фичи), прецеденты C-32/C-33
(`changes/process-group-timeout/`, `changes/module-decomposition/`).
