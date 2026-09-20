# Подключение Spine к вашему CLI-агенту

Пошаговое руководство для коллег: как подключить архитектурный контур Spine
(`arch-be`) к Claude Code, Qwen Code, Codex, Kimi Code, oh-my-pi (omp) или
любому MCP-совместимому агенту. Ни одного API-ключа LLM не потребуется —
модель даёт ваш агент, Spine даёт контроль, знания и гейты.

## 0. Установка

Готовый бинарь (Linux x86_64) — из раздела Releases:

```bash
curl -L -o arch-be https://github.com/romannekrasovaillm/spine-bank/releases/latest/download/arch-be-linux-x86_64
chmod +x arch-be && mv arch-be ~/.local/bin/
arch-be doctor    # проверка окружения (ключи LLM НЕ нужны — это нормально)
```

Или сборка из исходников (нужен Rust ≥ 1.85):

```bash
git clone https://github.com/romannekrasovaillm/spine-bank.git
cd spine-bank
cargo build --release --no-default-features --features core   # слим-сборка
ln -sf "$PWD/target/release/arch-be" ~/.local/bin/arch-be
```

Закрытый контур (без интернета) — **офлайн-бандл**: на машине с исходниками
`scripts/make_offline_bundle.sh` собирает `dist/spine-offline-<версия>-<os>-<arch>.tar.gz`
(core-редакция по умолчанию; `--edition full`, готовый бинарь — `--binary`).
Установка на целевой машине — одна распаковка и одна команда:

```bash
tar xzf spine-offline-*.tar.gz && cd spine-offline-*
./install.sh   # целостность (SHA256SUMS) → бинарь в ~/.local/bin → arch-be init
               # (ассеты встроены в бинарь, сеть не нужна) → cli_path Archify
arch-be doctor # проверка окружения
```

## 1. Подключение одной командой

В корне проекта, который должен контролировать Spine:

```bash
arch-be connect claude      # Claude Code — полное подключение
arch-be connect qwen        # Qwen Code — .qwen/settings.json + скиллы
arch-be connect gigacode    # GigaCode — автодетект .gigacode/ или .qwen/
arch-be connect codex       # Codex — печать TOML-блока (+ --apply-global)
arch-be connect kimi        # Kimi Code — проектный .kimi-code/mcp.json
arch-be connect omp         # oh-my-pi — .mcp.json + скиллы (если нет .claude/skills)
arch-be connect generic     # любой MCP-хост — все сниппеты на печать
```

| Хост | Что пишется в проект | Что печатается |
|---|---|---|
| `claude` | `.mcp.json`, `.claude/settings.json` (хуки), `.claude/skills/`, `CLAUDE.md` | следующие шаги |
| `qwen` | `.qwen/settings.json` (мердж `mcpServers`), скиллы в `.qwen/skills/` (существующий каталог не перетирается) | хуки — сниппет-референс |
| `gigacode` | автодетект каталога настроек (`.gigacode/` → `.qwen/` → новый `.gigacode/`): `settings.json` (мердж `mcpServers`) + скиллы в `skills/` (как у claude) | хуки — сниппет-референс |
| `codex` | ничего (с `--apply-global` — `~/.codex/config.toml`) | TOML-блок для `~/.codex/config.toml` |
| `kimi` | `.kimi-code/mcp.json` (мердж; с `--apply-global` — ещё и `~/.kimi-code/mcp.json`) | JSON-блок user-level, TOML-блок хука для `~/.kimi-code/config.toml`, напоминание про trust-диалог |
| `omp` | `.mcp.json` (мердж); скиллы в `.claude/skills/` — только если каталога ещё нет | автодискавери `.mcp.json`; хуки — TS-расширения `omp --hook <file.ts>` |
| `generic` | ничего | все сниппеты для ручной установки |

Особые значения host — не агенты, а **гейты, не зависящие от хоста** (хуки
ненадёжны: у Qwen headless-срабатывание не подтверждено, у Codex
lifecycle-хуков нет; CI и git-хуки — единственный гейт, который сработает
всегда):

| Хост | Что пишется в проект | Примечания |
|---|---|---|
| `ci --provider gitlab` | блок между `# spine-connect:begin/end` в `.gitlab-ci.yml` (мердж, чужие джобы сохраняются) | джоба `spine-gate`: `arch-be gate --route auto --format gitlab-codequality` в артефакт `reports.codequality` — **нарушения видны в интерфейсе merge request без ручной настройки** |
| `ci --provider github` | новый `.github/workflows/spine-gate.yml` (существующий без маркера не затирается — отказ) | `gate --format sarif` артефактом прогона + markdown в Job Summary; загрузка в code scanning — закомментированным шагом (нужен Advanced Security) |
| `ci --provider jenkins` | блок между `// spine-connect:begin/end` в `Jenkinsfile` | `gate --format junit` + публикация `junit(...)`; красный гейт — `error(...)` по коду возврата |

**Адрес релизов (`--releases-url`).** Джоба `spine-gate` скачивает binary
`arch-be` с адреса релизов. Без `--releases-url` в шаблоне остаётся заглушка
`<org>/<repo>`, и джоба падает при первом же прогоне:

```bash
arch-be connect ci --provider gitlab \
  --releases-url https://github.com/<org>/<repo>/releases/download
```

Без флага «Следующие шаги» называют эту команду, а `arch-be doctor` выдаёт
предупреждение `ci-releases` — молчащая заглушка выглядит как рабочая настройка.
| `git-hooks` | `.git/hooks/pre-commit` (быстрый `arch-be control check .`) и `pre-push` (полный `arch-be gate --route auto --base <remote sha>` — база берётся из stdin git'а, для новой ветки `merge-base` с основной); в worktree — в hooks основного git-каталога | блоки между маркерами, чужие строки хуков сохраняются; fail-soft: нет `arch-be` в PATH — молча пропуск; расположение реестра хуки не проверяют (резолвит бинарь), база уходит ГОЛОЙ ревизией — дописать `...HEAD` — работа гейта, иначе вышло бы `rev...HEAD...HEAD` |

Установка бинаря в CI-джобах — curl из релизов (в публичных релизах GitHub
артефакты — сырые бинари `arch-be-linux-x86_64` + `SHA256SUMS`; tar.gz —
формат офлайн-бандла из внутреннего хранилища и текстов самих джоб): оба
варианта закомментированы в
тексте джобы (замените `<org>/<repo>` в `RELEASES_URL`). `--dry-run` печатает
план без записи, повторный запуск дублей не плодит (маркерные блоки).

![Установка и подключение](screenshots/connect/01-connect.png)

Команда **идемпотентна** (повторный запуск не плодит дубли), **не затирает
чужое** (мердж `.mcp.json` и `settings.json` с сохранением ваших серверов и
хуков), имеет `--dry-run` (напечатать план, ничего не писать) и `--rw`
(открыть записывающие инструменты — см. §4).

**Источник скиллов.** В хост доставляется вся библиотека скиллов, а не только
встроенная в бинарь часть: источник истины — каталоги `[plugins].dirs` из
конфига arch-be (та же логика приоритета диска, что у `skill_load` и
MCP-промптов): всё, что есть на диске — включая плагины без `plugin.json`
(например, `arch-distilled`) и ваши правки библиотеки — едет в хост.
Встроенные ассеты бинаря — fallback для скиллов, которых на диске нет вообще
(свежая машина без `arch-be init`). Искать их (`skill_search`, `arch-be skills
search`) можно сразу после `connect` — каталоги хостов проекта входят в индекс
наравне с `[plugins] dirs`, а пустой индекс отвечает причиной, а не нулём. В печати результата — раздельные
счётчики «из библиотеки пользователя: N, из встроенных: M», а при
расхождении дисковой копии со встроенной — одна сводная заметка «K скиллов
берутся из библиотеки пользователя поверх встроенных копий».

Что появляется в проекте (на примере Claude Code):

![Файлы подключения](screenshots/connect/02-files.png)

### Kimi Code

`arch-be connect kimi` пишет проектный `.kimi-code/mcp.json` (мердж
`mcpServers.spine`, чужие серверы сохраняются; проектная запись перекрывает
одноимённую пользовательскую — так устроен Kimi Code). Поле `cwd` не
записывается: сервер наследует рабочий каталог харнесса.

Дополнительно печатаются:

- блок для ручной регистрации на пользовательском уровне
  (`~/.kimi-code/mcp.json`, общий для всех проектов) — запись туда с
  мерджем и бэкапом делает `arch-be connect kimi --apply-global`;
- TOML-блок Stop-хука для `~/.kimi-code/config.toml` (проектных хуков у
  Kimi Code нет): `[[hooks]]` с той же командой-гейтом
  `arch-be gate --route auto`, что у Claude Code (exit 2 = блок, stderr
  уходит модели).

При первом запуске `kimi` в каталоге появится trust-диалог со списком
project-level MCP-серверов — подтвердите «Trust this folder» (в
untrusted-папке project MCP не стартует, это штатная защита). Проверка
подключения: команда `/mcp` в TUI.

### oh-my-pi (omp)

`arch-be connect omp` пишет проектный `.mcp.json` формата Claude Desktop —
omp дискаверит его автоматически, отдельная регистрация не нужна. Скиллы
omp читает нативно из `.claude/skills/`: если такого каталога в проекте
ещё нет, connect раскладывает туда скиллы библиотеки (как для Claude
Code — источник и приоритет диска см. выше в «Источник скиллов»); каталог уже есть — не трогается, чтобы не перетирать вашу
библиотеку. Хуков через connect нет: механизм хуков omp —
TypeScript-расширения, подключаемые флагом `omp --hook <file.ts>`;
команда-гейт для такого расширения — `arch-be gate`.

### GigaCode

`arch-be connect gigacode` — нативное подключение GigaCode (форк Qwen Code;
алиасы: `gigacode`, `giga-code`, `gcode`). Каталог настроек определяется
автоматически: существующий `.gigacode/` в проекте; при его отсутствии —
существующий `.qwen/` (layout совместим, пишем туда); если нет ни того ни
другого — создаётся `.gigacode/` (об исходе автоопределения — заметка в
выводе). В каталог пишутся `settings.json` (мердж `mcpServers.spine`, чужие
серверы сохраняются) и скиллы в `skills/` (с обновлением версий из
библиотеки скиллов,
как у `claude`). Хуки не записываются — подтверждённой схемы файла хуков у
форка нет (в qwen-code 0.24 хуки управляются UI `qwen hooks` и в headless
не файрят): печатается сниппет-референс и информационный вариант
`SessionEnd` из [docs/GIGACODE.md](GIGACODE.md). Развёртывание силами самого
агента и офлайн-бандл для закрытого контура — там же.

## 2. Проверка подключения

```bash
claude mcp list
# spine: arch-be mcp serve - ✔ Connected
```

Механическая проверка со стороны Spine — `arch-be doctor --host <хост>`
(в корне проекта; иначе — `--dir <путь>`): бинарь `arch-be` в PATH, файл
настроек хоста содержит `mcpServers.spine` с командой `arch-be`, скиллы на
месте (для хостов, куда connect их пишет), версия хоста по
`<бинарь> --version`. Найденная проблема (нет записи о сервере, чужая
команда) — вердикт ✗ и код выхода 1; отсутствие хоста в PATH или скиллов —
предупреждение. Для `gigacode` проверка смотрит в тот же каталог, что
выбрал connect (автоопределение `.gigacode/`/`.qwen/`).

При первом запуске Claude Code в проекте он спросит разрешение на
project-scoped сервер из `.mcp.json` и доверие каталогу — ответьте «Yes»
(штатная защита; в headless-режиме `-p` добавьте `--allowedTools "mcp__spine__*"`).

Тот же обмен можно проверить вручную — сервер говорит JSON-RPC 2.0 по stdio,
одна строка — одно сообщение:

![Рукопожатие MCP](screenshots/connect/03-mcp.png)

## 3. Работа в сессии: агент под контролем спайна

Попросите агента проверить проект — например: «Вызови
`mcp__spine__fitness_check` для этого каталога». Инструмент читает
`.arch-handoff/CONSTRAINTS.yaml` и возвращает машиночитаемый вердикт.
Если гейт FAIL — агент видит находки и чинит их сам, затем перепроверяет:

![fitness FAIL → fix → PASS](screenshots/connect/04-fitness.png)

Готовые сценарии работы со Spine доступны как **слэш-команды хоста**: сервер
отдаёт девять плейбуков `spine-*` (`spine-architect-review`, `spine-fitness-gate`
и др.) через MCP-промпты (`prompts/list`, `prompts/get`) — в Claude Code это
команды вида `/mcp__spine__spine-architect-review` из меню `/`. Текст сценария
встроен в бинарь сервера, поэтому команды работают независимо от того, куда
хост раскладывает файлы скиллов. Подробности — в `docs/mcp.md` («Промпты»).

А Stop-хук (записан в `.claude/settings.json`) не даёт агенту завершить
работу, пока гейт красный: при попытке остановки хук запускает
`arch-be gate --route auto --base <merge-base с основной веткой>` (единый
гейт: fitness + delta guard + rule_weakened + spine + trace + целостность
модели, на маршрутах Standard/Critical ещё nfr и evidence — см.
`docs/control.md`), и при ненулевом коде возврата завершение блокируется
(exit 2), находки уходят агенту как feedback:

![Stop-хук](screenshots/connect/05-stop-hook.png)

Семантика хуков — **fail-soft на инфраструктуру** (нет `arch-be` в PATH —
молча пропуск, exit 0; нет входа у составляющих гейта — внутренний SKIP) и
**fail-hard на вердикт** (ненулевой код `arch-be gate` блокирует; строки
вывода хук не разбирает). Где лежит реестр правил, хук не знает и не
проверяет: путь резолвит бинарь (корень кейса, затем `.arch-handoff/`).
Реестра нет нигде — хук не молчит, а печатает «реестр правил не найден» и
блокирует завершение (INCOMPLETE): контур, который на отсутствующем входе
зеленеет, выдаёт поломку за порядок. Дополнительный
гейт на каждую правку (`PostToolUse` для Edit/Write) включается флагом
`--strict-hooks` — учтите: если в CONSTRAINTS.yaml есть правила
`command_succeeds` (например, `cargo test`), такой гейт будет дорогим.

## 4. Режимы MCP-сервера

```bash
arch-be mcp serve          # дефолт: строго read-only (38 инструментов:
                           #   16 ручных + 22 моста в реестр)
arch-be mcp serve --rw     # + записывающие: handoff_create, adr_new,
                           #   agentsmd_generate, skill_distill, archify_*,
                           #   reverse_survey, evidence_pack, delta_propose
```

Из rw-списка в core-сборке нет только `skill_distill` (дистилляция зовёт
LLM); `handoff_create` доступен и в core (создание пакета — чисто файловая
работа, см. `docs/handoff_walkthrough.md`).

Инструменты хоста за файлы и shell отвечают сами — `bash`, `write_file`,
`edit_file`, `harness_run`, `subagent_*`, `web_*` из Spine наружу **не
отдаются никогда** (never-список зашит и охраняется тестами реестра).

## 5. LLM-судья без API-ключей

Два способа оценивать документы рубриками, не заводя ключи у Spine:

**A. `kind = "cli"`** — Spine вызывает ваш авторизованный CLI подпроцессом:

```toml
# ~/.config/arch-harness/config.toml
default_model = "claude-cli"
[models.claude-cli]
kind = "cli"
command = "claude"
args = ["-p"]
```

```bash
arch-be rubric run adr_quality docs/adr/0001-money.md
```

![Судья через подписку](screenshots/connect/06-rubric-cli-judge.png)

**B. Split-judge** — для любого MCP-хоста, полностью механически:
`rubric_prompt` возвращает системный+пользовательский промпт, JSON-схему
ответа и конфиг судьи; хост судит сам (k раз); `rubric_verify` принимает
k сырых ответов, считает медиану, проверяет цитаты по тексту и собирает
итоговый отчёт с флагами `unstable` / `evidence_not_found`.

## 6. Устранение неполадок

| Симптом | Причина и лечение |
|---|---|
| `claude mcp list`: «Pending approval» | Project-сервер не одобрен — запустите `claude` интерактивно и подтвердите, либо `claude mcp add spine --scope local -- arch-be mcp serve` |
| Хук не срабатывает | Каталог не доверен (trust-диалог) или хуки отключены глобально; проверьте `arch-be gate` вручную — должен печатать «Итог: PASS/FAIL» |
| `command not found: arch-be` | Бинарь не в PATH: `ln -sf <путь>/arch-be ~/.local/bin/arch-be` |
| `rubric_run` отвечает -32603 | Модель без `kind = "cli"` требует API-ключ; добавьте cli-модель (§5A) или используйте split-judge (§5B) |
| Скиллы не видны агенту | Они в `.claude/skills/` проекта — проверьте, что агент читает project-скиллы (Claude Code: перезапуск) |

## 7. Отключение

```bash
arch-be connect claude --dry-run   # посмотреть, что именно записано
rm .mcp.json                        # или уберите ключ "spine" из файла
# хуки помечены маркером «spine-connect» — удалите их из .claude/settings.json
```
