<!-- Правила — в CONTRIBUTING.md. Сообщение коммита: conventional, по-русски. -->

## Что меняется

<!-- 2–5 предложений: что и зачем. Если закрывает issue — `Closes #NN`. -->

## Чек-лист

- [ ] Гейты зелёные локально: `cargo fmt --all -- --check`,
      `cargo clippy --all-targets -- -D warnings` (+ core-профиль),
      `cargo test` (+ `--no-default-features --features core`)
- [ ] Догфуд PASS: `cargo run --quiet -- control check . --constraints CONSTRAINTS.yaml`
- [ ] На изменение добавлены тесты (детерминированные, без сети)
- [ ] Тронуты `CONSTRAINTS.yaml` / `ARCHITECTURE-SPINE.md` / `model/` →
      приложена дельта `changes/<имя>/DELTA.md`; меняется инвариант или
      контракт → приложен ADR `docs/adr/ADR-0NN-*.md`
- [ ] Без секретов и персональных путей (CI сканирует)
- [ ] User-facing изменение отражено в документации; заметка для CHANGELOG.md
      (раздел «Что может покраснеть», если меняются вердикты гейта)

## Как проверялось

<!-- Команды и их итоги; для изменений гейта — контрпроба (правило краснится
     на нарушении). -->
