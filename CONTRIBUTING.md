# CONTRIBUTING — как участвовать в разработке Spine Core

Документ описывает рабочий контур репозитория: сборка, локальные гейты,
дельта-протокол для защищённых путей, ADR, правила коммитов и минимум для PR.
Конвенции кода — в [AGENTS.md](AGENTS.md), инварианты архитектуры — в
[ARCHITECTURE-SPINE.md](ARCHITECTURE-SPINE.md). Это публичный снапшот ядра:
зоны `banking/` здесь нет, происхождение форка — [NOTICE.md](NOTICE.md).

## Сборка

```bash
cargo build                    # отладочная сборка (полная, фича harness)
cargo build --release          # релизный бинарь: target/release/arch-be
cargo build --release --no-default-features --features core   # слим-сборка
```

- **MSRV — 1.85** (пин в `Cargo.toml` → `rust-version`; CI проверяет
  `cargo check` на 1.85). Тулчейн — stable, компоненты `rustfmt` и `clippy`
  перечислены в `rust-toolchain.toml`.
- **Фичи**: `default = ["harness"]` — полный харнесс (TUI, LLM-сеть);
  `core` — «орган чужого CLI-агента»: MCP-сервер, контроль, рубрики — без
  TUI и сети. Граница ядра охраняется правилом C-34
  (`scripts/check_core_deps.sh`), размер модулей — правилом C-33
  (`scripts/check_file_length.sh`, ≤ 3000 строк на продовый `.rs`).

После сборки смоук без LLM и ключей:

```bash
arch-be doctor                                        # проверка окружения
arch-be mermaid examples/mermaid/flow.mmd             # рендер в ASCII
arch-be control score --trigger new_component=true    # маршрут значимости
```

## Локальные гейты (прогонять до PR)

Те же проверки, что в CI (`.github/workflows/ci.yml`):

```bash
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo clippy --no-default-features --features core --all-targets -- -D warnings
cargo test
cargo test --no-default-features --features core
```

- Тесты детерминированы и изолированы от машины (`tempfile`, без сети и
  реального домашнего каталога); live-LLM-тесты помечены `#[ignore]` — им
  нужны ключи и сеть, в CI они не ходят.
- `cargo fmt --check` эквивалентен первой команде для проверки формата;
  форматирует — `cargo fmt`.

**Догфуд-гейт** (репозиторий проверяет сам себя собственной механикой,
50 правил из [CONSTRAINTS.yaml](CONSTRAINTS.yaml)):

```bash
cargo run --quiet -- control check . --constraints CONSTRAINTS.yaml
```

Прогон длинный (правило C-19 гоняет `cargo test` целиком) — это нормально.
Вердикт обязан быть PASS. Проверка чужого репозитория без исполнения —
`control check … --no-exec` (модель доверия `command_succeeds`, ADR-053).

## Защищённые пути и дельта-протокол

Правки под защищёнными путями — `CONSTRAINTS.yaml`,
`ARCHITECTURE-SPINE.md`, `model/` — обязаны упоминаться в активной дельте
`changes/<имя>/DELTA.md`. Гейт `delta_guard` отклоняет непокрытые правки.

Структура дельты — на примере
[changes/module-decomposition/DELTA.md](changes/module-decomposition/DELTA.md):
«Что меняется» (файлы и суть), «Зачем» (сила, делающая изменение
необходимым), «Чем подтверждается» (прогоны и контрпробы), «Ссылки».
Дельта архивируется вместе с изменением; новая дельта — новый каталог
`changes/<имя>/`.

## ADR

Решения, меняющие инварианты, контракты или границы, фиксируются до
реализации: `docs/adr/ADR-0NN-<slug>.md`, где `NN` — следующий свободный
номер (`ls docs/adr` — на момент записи последний ADR-054, следующий
ADR-055). Шаблон разделов: шапка с полями `Date` / `Status` /
`Reversibility`, далее `Context`, `Decision`, `Alternatives Considered`
(таблица с «Почему отвергнут»), `Consequences` (обязательны и Negative),
`Reversibility` (обратимость и условие пересмотра), `References`.
Каноничный шаблон — `assets/plugins/arch-core/skills/adr-authoring/references/adr-template.md`.
Чтение затронутых ADR — до изменения соответствующих мест кода.

## Коммиты

[Conventional Commits](https://www.conventionalcommits.org/), сообщение на
русском: `feat(област): что сделано`, `fix(област): …`, `refactor(област): …`,
`test(област): …`, `docs(област): …`. Префиксы волн разработки
(`fix(A1):`, `refactor(B1):`, `test(C2,C3):`) — по необходимости, когда
коммит закрывает пункт тематической волны. Один коммит — одно цельное
изменение; в сообщении — что и зачем, а не пересказ диффа.

## Минимум для PR

- Зелёный CI (`.github/workflows/ci.yml`: fmt, clippy × 2 профиля, тесты × 2,
  MSRV 1.85, cargo audit, SBOM, догфуд, скан персональных путей).
- На изменение — тесты (детерминированные, без сети; см. «Конвенции» в
  [AGENTS.md](AGENTS.md)).
- Тронуты `CONSTRAINTS.yaml` / `ARCHITECTURE-SPINE.md` / `model/` → в PR
  есть дельта `changes/<имя>/DELTA.md`; меняется инвариант или контракт →
  есть ADR.
- Без секретов и персональных путей (CI сканирует: ключи — только через
  окружение, машинные каталоги — только в пользовательский конфиг).
- Пользовательски видимое изменение отражено в документации (`docs/*.md`,
  README — релизным шагом).

## Скриншоты README

SVG-кадры TUI в `docs/screenshots/` регенерируются из кода (не править
руками):

```bash
ARCH_GEN_SHOTS=1 cargo test gen_readme_screenshots
```

Изменили рендер TUI — переснимите кадры этой командой и приложите обновлённые
SVG к PR. PNG-кадры живых прогонов харнессов (`docs/screenshots/harnesses/`,
`connect/`) из кода не воспроизводятся — переснимаются вручную на реальных
сессиях.

## Куда обращаться

- Вопросы и поддержка — [SUPPORT.md](SUPPORT.md); уязвимости — **не** в
  общий трекер, процедура в [SECURITY.md](SECURITY.md).
- Документация по устройству контура — [docs/](docs/) (карта — в
  [README.md](README.md) → «Документация»).
