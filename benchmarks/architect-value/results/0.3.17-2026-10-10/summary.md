# Сводка прогона 0.3.17-2026-10-10

Каталог: `results/0.3.17-2026-10-10/` (полный вывод — `run_all.log`).
Прогон: `run_all.sh`; SKIP ≠ PASS (см. `PREREGISTRATION.md` §5).

## Итоги по метрикам

```
[RUN] metric 7: expressible=7/12 (baseline=7, target≥11; ids=JSS-001,JSS-003,JSS-004,JSS-006,JSS-007,JSS-008,JSS-011)
[RUN] metric 9: false_fail_errors=2 (из них пустой glob: 2; baseline=2, target=0) rules=s6_domain_independent,s7_inbound_no_outbound
[RUN] metric 10: formats_seen=3/8 (baseline=4/8, target=8/8 + диапазоны/scope); missed=svc.csproj,go.mod,build.gradle,package.json,pyproject.toml
[RUN] metric 8 C.2 [V01]: ✓ факт=caught ожид=caught (baseline 0.3.16=caught) владелец=s6_domain_independent
[RUN] metric 8 C.2 [V02]: ✗ факт=missed ожид=caught (baseline 0.3.16=missed) владелец=s6_domain_independent
[RUN] metric 8 C.2 [V03]: ✓ факт=caught ожид=caught (baseline 0.3.16=caught) владелец=s7_inbound_no_outbound
[RUN] metric 8 C.2 [V04]: ✓ факт=caught ожид=caught (baseline 0.3.16=missed) владелец=s6_domain_independent
[FIND] metric 8 C.2 [V04]: факт=caught ≠ baseline 0.3.16=missed
[RUN] metric 8 C.2 [V05]: ✓ факт=caught ожид=caught (baseline 0.3.16=caught) владелец=s6_domain_independent
[RUN] metric 8 C.2 [V06]: ✓ факт=caught ожид=caught (baseline 0.3.16=caught) владелец=s6_domain_independent
[RUN] metric 8 C.2 [V07]: ✓ факт=caught ожид=caught (baseline 0.3.16=caught) владелец=s8_no_literal_password
[RUN] metric 8 C.2 [V08]: ✗ факт=missed ожид=caught (baseline 0.3.16=missed) владелец=s1_no_embedded_db_in_prod
[RUN] metric 8 C.2 [V09]: ✗ факт=missed ожид=caught (baseline 0.3.16=missed) владелец=s1_no_embedded_db_in_prod
[RUN] metric 8 C.2 [V10]: ✓ факт=caught ожид=caught (baseline 0.3.16=caught) владелец=s6_domain_independent
[RUN] metric 8 C.2 [V11]: ✗ факт=missed ожид=caught (baseline 0.3.16=missed) владелец=s7_inbound_no_outbound
[RUN] metric 8 C.2 [V12]: ✓ факт=clean ожид=clean (baseline 0.3.16=clean) владелец=s6_domain_independent
[RUN] metric 8: correct=8/12 (baseline 0.3.16: 7/12 slash, 2/12 Java; target 0.4.0 ≥11/12)
[RUN] C.3 text [A1]: ✓ факт=caught ожид(text)=caught (archunit=caught) — import адаптера в домене
[RUN] C.3 text [A2]: ✗ факт=caught ожид(text)=missed (archunit=caught) — wildcard-import адаптера в домене
[RUN] C.3 text [A3]: ✓ факт=missed ожид(text)=missed (archunit=caught) — поле по FQN исходящего адаптера в до
[RUN] C.3 text [A4]: ✓ факт=missed ожид(text)=missed (archunit=caught) — new <FQN> исходящего адаптера в доме
[RUN] C.3 text [A5]: ✓ факт=missed ожид(text)=missed (archunit=caught) — ссылка FxClient.class по FQN в домен
[RUN] C.3 text [A6]: ✓ факт=clean ожид(text)=clean (archunit=clean) — закомментированный import (контроль 
[RUN] C.3 text_detection: correct=5/6 (текстовое правило s6)
[SKIP] C.3 archunit_detection: нужен ArchUnit-jar (`archunit fetch`, требует сеть); ARCHUNIT_JAR не задан → байткод-детектор (type: archunit, ADR-039) не измерен
[RUN] metric 13 significant [petclinic:79b527a]: hit=нет f1=0.00 surf=['.->tracing-server:9411', 'tracing-server:9411'] exp=['api-gateway->genai-service', 'azure-openai', 'genai-service->customers-service', 'genai-service->vets-service', 'openai'] — Generative AI support for Spring Petclin
[RUN] metric 13 significant [boutique:2932624]: hit=нет f1=0.00 surf=нет exp=['alloydb', 'frontend->shoppingassistantservice'] — Add ShoppingAssistant, and AlloyDB suppo
[FIND] metric 13 noise [petclinic:dc9ca15]: непустой дифф (ui)
[RUN] metric 13: recall_significant=0/2=0.00 (порог≥0.8); composition_F1=0.00 (порог≥0.8); noise=1/6=0.17 (порог≤0.1) [0.3.17 before: 0/2 по составу, 6/6 шум]
[RUN] metric 15 [serve]: tools=40 divergences=0
[RUN] metric 15 [serve --rw]: tools=51 divergences=0
[RUN] metric 15: divergences=0 (baseline=1 reverse_survey, target=0); servers=serve/serve --rw
[RUN] metric 16: recovery_tools=нет missing=['recover', 'arch_diff'] (baseline: нет инструментов, target: recover+arch_diff)
[RUN] metric 1 [boutique]: rel=46 abs=46 equal=yes
[RUN] metric 1 [petclinic]: rel=71 abs=71 equal=yes
[RUN] metric 1 [buckpal]: rel=22 abs=22 equal=yes
[RUN] metric 1 [library]: rel=31 abs=31 equal=yes
[SKIP] metric 2 (компоненты F1): волна R (recover) не реализована в 0.3.17
[SKIP] metric 3 (рёбра recall/precision): волна R (recover) не реализована в 0.3.17
[SKIP] metric 4 (хранилища recall): волна R (recover) не реализована в 0.3.17
[SKIP] metric 5 (внешние precision): волна R (recover) не реализована в 0.3.17
[SKIP] metric 6 (шум карты survey): механизм грунт-труса recover отсутствует в 0.3.17
[SKIP] metric 18 (детерминизм recover/arch-diff JSON): recover отсутствует; arch-diff даёт JSON (см. metric 13)
[SKIP] metric 19 (время recover ≤5 тыс. файлов): recover отсутствует в 0.3.17
[SKIP] metric 11 (Kontur-bench измеримых кейсов): волна K (redteam) не реализована в 0.3.17
[SKIP] metric 12 (Kontur на реальном коде: detection/FP): волна K (redteam) не реализована в 0.3.17
[SKIP] metric 20 (квалификация судьи: различительная сила): рубрики/судья — вне Python-прогонщиков
[SKIP] metric 14 (selftest из коробки): измеряется в CI (cargo test), не прогонщиком бенчмарка
[SKIP] metric 17 (golden judge MAE / доля в диапазоне): требует API-ключ судьи — вне офлайн-прогона
```

## Счётчики

| маркер | число |
|--------|-------|
| RUN    | 34 |
| SKIP   | 13 |
| FIND   | 2 |
