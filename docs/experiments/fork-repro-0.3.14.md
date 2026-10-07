# Воспроизведения находок ревью 0.3.13 → фиксы 0.3.14 (форк spine-bank)

Каждая секция — одна находка TASK-v2: что воспроизводили, результат до фикса,
результат после фикса, как повторить. Репродукции выполнялись на бинаре
`arch-be` (debug) из worktree соответствующей волны.

## Секция A. Волна A: отчёты прогонов — машинные записи (A1, A2)

### A1. «Рукописный PASS» — главная находка

**Сценарий** (репродукция раздела 3 TASK-v2): бандл Critical, где
`VALIDATION.md`, `reports/fitness.md` и `WALKING-SKELETON.md` написаны рукой
(«Итог: PASS (8 из 8)») без единого прогона.

```bash
AB=./target/debug/arch-be
$AB bootstrap "Рукописный PASS" --dir /tmp/hp --domain платежей
cd /tmp/hp
# Все заглушки заменены содержательной прозой; в трёх отчётах — только:
#   # Отчёт
#   Прогон проверен.
#   Итог: PASS (8 из 8)
# Тесты и fitness НЕ запускались. Репетиция отката — честная
# (control gate A4 --rehearse на baseline=HEAD), она не предмет находки.
$AB evidence pack . --route critical
$AB gate --repo . --route critical
```

**До фикса (e3ee160, v0.3.13):** `evidence_verify` PASS — строки «Итог: PASS»
в прозе достаточно; `Итог: PASS`, **exit 0**. Тест-репродукция:
`evidence::tests::verify_flags_unbound_handwritten_report` (падал до фикса:
находки `evidence_report_unbound` не существовало).

**После фикса (волна A):** каждый рукописный отчёт даёт находку
`[warn] evidence_report_unbound — «строка "Итог: PASS" в тексте не доказывает,
что прогон был» → запустите \`arch-be evidence record <kind>\``, вердикт
остаётся PASS (warn по умолчанию, правило 4). В кейсе с
`[evidence] require_records = true` (его генерирует `arch-be bootstrap` —
bank-профиль на Critical) та же находка — `error`, гейт FAIL, **exit 1**.

Закрытие находки — только прогоном:

```bash
arch-be evidence record fitness                 # дефолт: arch-be control check .
arch-be evidence record tests    --cmd "python3 -m pytest skeleton -q"
arch-be evidence record skeleton --cmd "python3 -m pytest skeleton -q"
arch-be evidence pack . --route critical        # артефактом становится запись
arch-be gate --repo . --route critical          # PASS
```

Правка кода после записи (`echo … >> skeleton/…py`) → находка
`evidence_record_stale` («входы прогона изменились после записи»), гейт FAIL
до повторного `evidence record`. См. ADR-065.

### A2. Выводимые артефакты не пишутся руками

**До фикса:** bootstrap создавал заглушки и для машинных артефактов
(`RISK.md`, `VALIDATION.md`, `reports/fitness.md`, `WALKING-SKELETON.md`,
`.arch-handoff/REHEARSAL.json`), строка прогресса была суммарной
(`бандл 7/13`), а `risk_level` требовал рукописный файл.

**После фикса:**

- `arch-be evidence pack` пишет в `EVIDENCE.yaml` запись значимости
  (маршрут, триггеры с источниками `declared`/`diff`, HEAD) — `risk_level`
  выводится из неё; рукописный RISK.md необязателен (legacy-файл по-прежнему
  принимается, обратная совместимость).
- `arch-be bootstrap` не создаёт заглушек выводимых артефактов: каркас
  содержит 6 заглушек автора (проблема, спека, приёмка, откат, решение A3,
  ревью) вместо 10+; свежий каркас печатает
  `бандл пишет автор 8/8 · выведет машина 1/5` (risk_level уже выведен
  записью значимости при упаковке).
- Тот же раздельный счёт печатает `arch-be evidence pack`
  (`Бандл: пишет автор X/8 · выведет машина Y/5`) — этими двумя числами
  снимается отложенный замер церемонии (файлов, которые человек правит
  руками, до/после).
- Приёмка: на Critical автор пишет ≤ 8 артефактов (тест
  `bootstrap_walks_to_green_when_artifacts_are_written` проходит полный путь
  до PASS при `require_records = true`, записав 7 файлов автора); подмена
  выводимого прозой ловится через A1 (`evidence_report_unbound`).
- Подсказка следующего шага проводника для машинного ключа ведёт к прогону
  (`evidence record` / `control gate A4 --rehearse` / `evidence pack`), а не
  к «создайте файл руками».
