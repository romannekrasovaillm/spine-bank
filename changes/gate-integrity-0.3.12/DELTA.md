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
  обновляются по мере закрытия.

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
- [ ] B1: тестовый тег на коммите с красной джобой не создаёт GitHub Release.
- [ ] B2: sbom-<edition>.cyclonedx.json и SHA256SUMS приложены к релизу
      и соответствуют опубликованным бинарям.
- [ ] B4/B5: actions по SHA, permissions, cargo deny, CODEOWNERS — под
      фитнес-правилами dogfood (сам CONSTRAINTS.yaml).
- [ ] C1–C3: plugins.lock, env_allow на Critical, составляющая secrets;
      ключ в коде → `secret_literal` в пост-гейте.
- [ ] ADR-055 (A5), ADR-056 (B3), ADR-057 (C4) приняты человеком;
      D1–D4 — ADR или docs/experiments/, код за флагами.
- [ ] CHANGELOG: раздел «Что может покраснеть» перечисляет новые находки.
