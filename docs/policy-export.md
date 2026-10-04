# Экспорт инвариантов развёртывания в политики кластера (ADR-059 §D3)

Мост «спайн → политики кластера» по аналогии с `ArchUnit`-мостом
(ADR-039): инварианты деплоя описываются один раз отдельной секцией
`deployment:` в `CONSTRAINTS.yaml`, а `arch-be policy export kyverno|rego`
проецирует их в проверяемые политики kyverno/OPA. Экспорт вручную дрейфует
от реестра; генератор из единственного источника — нет.

Секция `deployment:` **не смешивается с правилами кода**: формат `rules:`
не трогается вовсе (правило 4 TASK), гейт секцию игнорирует и работает
байт-в-байт как без неё, а `rules_report`/`control report` при желании
показывают обе плоскости отдельно.

## Быстрый старт

```bash
# Разрешить образы из внутреннего реестра и потребовать подпись
cat >> CONSTRAINTS.yaml <<'YAML'
deployment:
  images:
    registry: "registry.example.com/bank"
    signed: true
  security:
    run_as_non_root: true
  resources:
    max_cpu: "500m"
    max_memory: "512Mi"
  deny_images:
    - "docker.io/library/nginx"
YAML

arch-be policy export kyverno --repo .            # манифесты в stdout
arch-be policy export rego --repo . --output archbe/deployment.rego
```

Прогон гейта кода не меняется: `arch-be gate` / `arch-be control check`
на реестр с секцией `deployment:` дают тот же вердикт, что и без неё.

## Схема секции

Все поля опциональны, кроме идентифицирующих; неизвестные поля внутри
`deployment:` игнорируются (толерантный serde-разбор). Отсутствие секции и
секция без действующих инвариантов равнозначны: экспортировать нечего.

| Поле | Тип | Смысл |
|---|---|---|
| `deployment.images.registry` | строка | Разрешённый префикс реестра (`registry.example.com/bank`). Образы контейнеров обязаны с него начинаться. |
| `deployment.images.signed` | bool | `true` — подпись образа обязательна (Kyverno `verifyImages`). `false` — ограничение снято. |
| `deployment.security.run_as_non_root` | bool | `true` — `securityContext.runAsNonRoot` обязателен у всех контейнеров Pod. |
| `deployment.resources.max_cpu` | строка (k8s-квантор) | Потолок `limits.cpu` (`500m`, `2`). |
| `deployment.resources.max_memory` | строка (k8s-квантор) | Потолок `limits.memory` (`512Mi`, `1Gi`). |
| `deployment.deny_images` | список строк | Запрещённые ссылки на образы (tech-радар для образов); запись — префикс ссылки, включая тег/дайджест. |

Пример — в [`config.example.toml`](../config.example.toml) секции **нет**:
`deployment:` — формат целевых репозиториев, а не конфиг харнесса.

## Команда

```text
arch-be policy export <kyverno|rego> [--repo <dir>] [--constraints <file>] [--output <file>]
```

- `--repo` — репозиторий с `CONSTRAINTS.yaml` (по умолчанию текущий каталог);
- `--constraints` — явный файл ограничений (по умолчанию — единый резолвер:
  `.arch-handoff/CONSTRAINTS.yaml` → корневой `CONSTRAINTS.yaml`);
- `--output` — файл вывода (по умолчанию stdout). Каталог должен
  существовать.

Пустая или отсутствующая секция — сообщение и **exit 0** (не ошибка:
экспортировать нечего), файл не пишется.

## Kyverno-выход

По одному `ClusterPolicy` (`apiVersion: kyverno.io/v1`) на группу
инвариантов, документы разделены `---`; порядок групп фиксирован:

1. `archbe-deploy-images` — `validate` по префиксу реестра (`foreach` по
   `containers` + `initContainers`) и, при `signed: true`, правило
   `verifyImages`;
2. `archbe-deploy-nonroot` — `securityContext.runAsNonRoot: true`;
3. `archbe-deploy-resources` — `limits.cpu`/`limits.memory` не превышают
   бюджет (операторы `<=`, Kyverno понимает k8s-кванторы);
4. `archbe-deploy-deny-images` — по правилу на запись списка: негативный
   glob-паттерн `!<ссылка>*` (AND по записям) ловит и `repo`, и `repo:tag`,
   и `repo@sha256:…`.

Проверено `kyverno apply` (1.19.1): несоответствующий Pod падает на своей
группе, соответствующий — проходит.

## Rego-выход

Один файл, пакет `package archbe.deployment` (Rego v1, OPA/Conftest):
множество `deny` — сообщения о нарушениях, `allow` — объект чист.
Контейнеры Pod собираются из `spec.containers` + `spec.initContainers`;
группы: реестр, подпись, non-root, лимиты, deny-список. Проверено
`opa check` и `opa eval` (OPA 1.21.1).

## Детерминизм и канонизация

Вывод — чистая функция секции: повторный экспорт даёт байт-в-байт
идентичный результат (golden-тесты, `tests/fixtures/policy-export/`).
`deny_images` канонизируется (сортировка + дедуп) — порядок авторства на
вывод не влияет.

## Честные границы

- **Подпись в Rego.** Plain Rego не проверяет cosign-подписи. Криптопроверку
  отдаёт Kyverno `verifyImages`; в Rego удерживается её следствие —
  адресация по дайджесту (`image@sha256:…`). Это осознанная граница, а не
  полная эквивалентность.
- **Ключ проверки подписи.** В `verifyImages` генерируется ключ-заглушка
  `k8s://arch-be/image-signing-key` с комментарием — перед применением
  замените на реальный публичный ключ (cosign).
- **`kyverno.io/v1`** в Kyverno 1.19 помечен deprecated в пользу CEL-политик
  (`policies.kyverno.io`); генератор намеренно держится выбранного формата
  `ClusterPolicy` из ADR-059 §D3.
- **deny-список** сопоставляется по префиксу ссылки: запись
  `docker.io/library/nginx` запрещает `nginx`, `nginx:1.25`, `nginx@sha256:…`,
  но не `nginx-plus`. Реестр целиком ограничивается allow-списком
  `images.registry`.

## См. также

- [ADR-059](adr/ADR-059-sloi-posle-geyta-volna-d-tenevoy-geyt-bitbucket-code-insights-runtime-fitness-prototipy-za-flagami.md) §D3 — решение владельца о формате;
- [archunit.md](archunit.md) — родственный мост «единый источник → исполняемый гейт»;
- [control.md](control.md) — реестр правил и fitness-контроль.
