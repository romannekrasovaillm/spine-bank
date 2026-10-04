# Дельта: gate-integrity-0.3.12
- Route: Critical (significance: new_component + security_boundary_change + significant_nfr + trust_zone_change)
- Created: 2026-10-03
- Задание: TASK-Spine-Core-0.3.12.md (эшелонированная защита: разработчик → CI → мерж → реестр → рантайм → после деплоя)

## Проблема

Исполнитель, которого проверяет гейт, может сам узаконить ослабление гейта:
override принимает любой непустой номер ADR (воспроизведено в
docs/experiments/fork-repro-0.3.12.md, RA-1: exit 0 при PAN в коде,
пониженной severity и выдуманном ADR-999), дельту пишет тот же агент, а
файлы контрольной плоскости (Stop-хук, arch-harness.toml, пороги рубрик)
вне защиты. Релиз публикуется без зависимости от CI; SBOM и хэши
описывают другую сборку, чем опубликованные бинари; входы рантайма
агента (плагины, окружение) не проверяются на целостность.

## ADDED

- When в CONSTRAINTS.yaml меняется тело правила (все поля, кроме
  rationale/fix_hint/skill/ad) without активного override, the гейт shall
  выдать находку `BodyChanged` (warn по умолчанию, error по флагу
  `[gate.rule_weakened] body = "error"`) — A1.
- When override ссылается на ADR, которого нет в docs/adr/, или он не
  принят, the гейт shall выдать `override_adr_missing` /
  `override_adr_not_accepted` и не считать ослабление узаконенным — A2.
- When горизонт `until` дальше `[gate.overrides] max_horizon_months`
  (дефолт 12), the гейт shall выдать `override_horizon_exceeded` — A2.
- When MANIFEST.json содержит пины `control_plane`, а текущие файлы
  контрольной плоскости (`.claude/settings.json`, git-хуки/CI-блоки
  spine-connect, `arch-harness.toml` политики, `.arch-handoff/RUBRIC.yaml`,
  `.arch-handoff/CONSTRAINTS.yaml`) расходятся с ними без активной дельты,
  the гейт shall выдать `control_plane_tampered` (error) — A3.
- When исполнитель завершился (включая авто-коммит), the `harness_run`
  shall сам запустить `arch-be gate` по рабочему дереву с базой
  `baseline_commit` вне окружения исполнителя; красный пост-гейт —
  код выхода 1; отключение — только `post_gate = false` с предупреждением — A4.
- When загружается плагин, содержимое которого расходится с
  `plugins.lock`, the рантайм shall не загружать его и выдать
  `plugin_tampered` в чат и журнал — C1.
- When маршрут Critical или bank-profile, the окружение исполнителя
  shall по умолчанию ограничиваться env_allow
  (["PATH","HOME","LANG","TERM","TMPDIR"] + явные имена адаптера);
  полное наследование — только `env_inherit = true` с предупреждением — C2.
- Составляющая `secrets` (детекторы src/secrets.rs, область changed/all,
  по умолчанию warn) + шаблон правила в arch-governance — C3.
- Release по тегу v* зависит от зелёного CI коммита (check-runs или
  прямой прогон test/dogfood/eval) — B1; SBOM и SHA256SUMS в релизе — от
  тех же бинарей, что опубликованы (`cargo sbom` в матрице редакций) — B2.

## MODIFIED

- `active_override_keys`: было — активен любой override с непустым `adr`
  и неистёкшим `until`; стало — требуется существующий принятый ADR и
  горизонт в пределах max_horizon_months (совместимость: gate-verdict/v1
  расширяется новыми находками, формат не меняется).
- `RuleSnapshot`: было — id/exclude_glob/severity; стало — + нормализованный
  хэш тела правила. Ужесточение (новое правило, расширение glob,
  повышение severity) ослаблением не считается.
- `run_harness`: итог прогона дополняется вердиктом пост-гейта.
- `docs/supply-chain.md`, `docs/threat-model.md` §5: статусы GAP
  обновляются по мере закрытия; инструкция проверки provenance/подписи — B3.
- Волна B (файлы): `.github/workflows/release.yml` (B1 — проверка зелёного
  CI перед публикацией; B2 — `cargo sbom` по редакциям, `SHA256SUMS` с
  хэшами SBOM; B3 — `attest-build-provenance` + `cosign sign-blob`,
  ADR-056 вариант (а)), `.github/workflows/ci.yml` и `nightly.yml` (B4 —
  `uses:` по полному SHA, `permissions: contents: read`, джоба `cargo deny`),
  `.github/CODEOWNERS` (B5, новый), `deny.toml` (B4, новый), корневой
  `CONSTRAINTS.yaml` (фитнес-правила B4/B5; ужесточение — новое правило,
  расширение glob, повышение severity ослаблением не считается).
- `.arch-handoff/action-pins.txt` (B4/B3): снимок SHA-пинов используемых
  actions на дату выпуска пакета — данные для правки `uses:`, не код.
- Волна C (файлы, 0.3.12): `src/plugin_lock.rs` (C1, новый — формат
  `arch-be/plugins-lock/v1`, обход каталога плагина, `verify_entry`/`diff`),
  `src/plugin.rs` (C1 — `discover_report`/`TamperEvent::plugin_tampered`,
  плагин с расхождением с замком не загружается), `src/cli/library.rs`
  (C1 — `arch-be plugins lock [--check]`; выход 1 при расхождении),
  `src/doctor.rs` (C1 — целостность библиотеки в отчёте), `src/agent.rs`
  (C1 — событие `plugin_tampered` в журнал сессии), `src/harness_env.rs`
  (C2, новый — политика окружения прогона), `src/harness.rs` (C2 — применение
  whitelist и заметка в итоге), `src/config.rs` (C2 — `env_inherit`,
  `bank_profile`, `DEFAULT_ENV_ALLOW`; C3 — `[gate.secrets]`), `src/cli/mod.rs`
  (C2 — заметка окружения в выводе `harness-run`), `src/secrets.rs` (C3 —
  `scan_text` + детектор `github-token`), `src/delta.rs` (C3 — общий
  `changed_files`), `src/gate/components/mod.rs` (C3 — составляющая
  `secrets`), `src/gate/types.rs`/`src/gate/verdict.rs` (C3 — опции и сборка),
  `src/assets.rs` + `assets/rule-templates/secret-literal/**` (C3 — шаблон
  правила для `rule_template_apply`), `config.example.toml` (C2/C3 — разделы).
  Защищённые файлы спайна/реестра (`model/`, `ARCHITECTURE-SPINE.md`,
  `CONSTRAINTS.yaml`) волной C не менялись.

## REMOVED

- Ничего: выходов реестра нет; ручную проверку лицензий jq из
  docs/supply-chain.md заменяет джоба `cargo deny` (B4) — ссылка, не
  удаление ответственности.

## План отката

Дельта обратима: каждый пункт — отдельный коммит (fix(A1)/ci(B2)/…),
откат — git revert соответствующего коммита; новые находки
(`BodyChanged`, `override_*`, `control_plane_tampered`) выводятся
warn-по-умолчанию либо за флагом, краснить чужой пайплайн могут только
после явного включения (правило 4 задания: warn → error по флагу).
Пост-гейт A4 отключается адаптером `post_gate = false`.

## Критерии приёмки

- [x] RA-1 из fork-repro-0.3.12.md даёт `exit 1` с `override_adr_missing`
      (сквозной критерий успеха задания). — A2, подтверждено независимо
      (fork-repro-0.3.12.md, «RA-1 после A1+A2: приёмка — ЗАКРЫТО»).
- [x] A1: `command → true`, `src/** → src/none/**`, `pattern → x^` дают
      `BodyChanged`; правка только rationale — чисто; ужесточения чистые.
      — коммит `60988e7`, тесты `rule_weakened` 10/10.
- [x] A2: существующий тест rule_weakened_active_override_legalizes_weakening
      переписан на реальный файл ADR и зелёный. — коммит `fe92ec4`,
      `override` 15/15.
- [x] A3: аудит control-plane-0.3.12.md написан; удаление Stop-хука → exit 1.
      — коммиты `30b90f8` (пины MANIFEST) + `2fb8791` (составляющая
      `control_plane`); RA-4/RA-4b/RA-4c воспроизведены, после A3 → exit 1
      `control_plane_tampered`.
- [x] A4: прогон с заглушкой-исполнителем, удаляющей Stop-хук и пишущей
      нарушающий код, — красный итог с control_plane_tampered. — A4 + A4.1b
      (пин MANIFEST на входе): `runner.rs` пост-гейт, `manifest_tampered`;
      RA-6 воспроизведён (до — код 0 при нарушении, после — красный).
- [x] B1: тестовый тег на коммите с красной джобой не создаёт GitHub Release.
      — `ci-green` (fail-closed, check-runs API, 11 обязательных джоб = ci.yml);
      эмпирика — на теге 0.3.12 (структура и имена проверены независимо).
- [x] B2: sbom-<edition>.cyclonedx.json и SHA256SUMS приложены к релизу
      и соответствуют опубликованным бинарям. — SBOM в релизной матрице
      (linux-x86_64 на редакцию), хэши SBOM в SHA256SUMS, `--locked` в CI.
- [x] B4/B5: actions по SHA, permissions, cargo deny, CODEOWNERS — под
      фитнес-правилами dogfood (сам CONSTRAINTS.yaml). — C-35…C-43, dogfood
      59/0 PASS; 56/56 пинов сверены со списком архитектора.
- [~] C1–C3: plugins.lock, env_allow на Critical, составляющая secrets;
      ключ в коде → `secret_literal` в пост-гейте. — реализовано волной C
      (unit/CLI-тесты: «плагин с подменённым хуком/скиллом не загружается,
      `plugin_tampered` (RA-8)», «Critical-прогон получает только whitelist,
      `env_inherit` — наследование с предупреждением (RA-9)»,
      «`src/leak.go` → `secret_literal` [warn] файл:строка, severity error
      по конфигу (RA-10)»); ожидает приёмки архитектора.
- [~] ADR-055 (A5), ADR-056 (B3), ADR-057 (C4) приняты человеком;
      D1–D4 — ADR или docs/experiments/, код за флагами. — ADR-055
      Accepted 2026-10-03 (вариант (а)); ADR-056 Accepted 2026-10-03
      (вариант (а) GitHub-native SLSA); ADR-057 занят внеочередной фичей
      «режимы вызова кодового агента» (ACP, принято владельцем 2026-10-03) —
      C4 (аттестация вердикта) оформится как ADR-058. C1–C3 волны C
      реализованы (см. выше), C4/D — открыты.
- [x] CHANGELOG: раздел «Что может покраснеть» перечисляет новые находки.
      — [0.3.12] 2026-10-03: находки волн A и B.
