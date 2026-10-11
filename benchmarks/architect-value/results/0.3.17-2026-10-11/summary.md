# Сводка прогона 0.3.17-2026-10-11

Каталог: `results/0.3.17-2026-10-11/` (полный вывод — `run_all.log`).
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
[RUN] metric 13 significant [petclinic:79b527a]: hit=да f1=0.33 surf=['azure-openai', 'openai', 'spring-petclinic-api-gateway->spring-petclinic-genai-service', 'spring-petclinic-genai-service->azure-openai', 'spring-petclinic-genai-service->openai', 'spring-petclinic-genai-service->spring-petclinic-customers-service', 'spring-petclinic-genai-service->spring-petclinic-vets-service'] exp=['api-gateway->genai-service', 'azure-openai', 'genai-service->customers-service', 'genai-service->vets-service', 'openai'] — Generative AI support for Spring Petclin
[RUN] metric 13 significant [boutique:2932624]: hit=да f1=0.80 surf=['alloydb', 'frontend->shoppingassistantservice', 'vertex-ai'] exp=['alloydb', 'frontend->shoppingassistantservice'] — Add ShoppingAssistant, and AlloyDB suppo
[FIND] metric 13 noise [petclinic:dc9ca15]: непустой дифф (ui)
[RUN] metric 13: recall_significant=2/2=1.00 (порог≥0.8); composition_F1=0.57 (порог≥0.8); noise=1/6=0.17 (порог≤0.1) [0.3.17 before: 0/2 по составу, 6/6 шум]
[RUN] metric 15 [serve]: tools=42 divergences=0
[RUN] metric 15 [serve --rw]: tools=53 divergences=0
[RUN] metric 15: divergences=0 (baseline=1 reverse_survey, target=0); servers=serve/serve --rw
[RUN] metric 16: recovery_tools=['recover', 'arch_diff'] missing=[] (baseline: нет инструментов, target: recover+arch_diff)
[RUN] metric 1 [boutique]: rel=46 abs=46 equal=yes
[RUN] metric 1 [petclinic]: rel=71 abs=71 equal=yes
[RUN] metric 1 [buckpal]: rel=22 abs=22 equal=yes
[RUN] metric 1 [library]: rel=31 abs=31 equal=yes
[RUN] metric 2 [buckpal]: F1 0.889 (P 0.80, R 1.00; TP=4 P=5 E=4)  OK (порог F1 ≥0.80)
[RUN] metric 2 [buckpal] лишнее: common
[RUN] metric 2 [library]: F1 1.000 (P 1.00, R 1.00; TP=3 P=3 E=3)  OK (порог F1 ≥0.80)
[RUN] metric 3 [boutique]: recall 17/17 (1.00)  precision 17/17 (1.00)  OK (порог: 17 рёбер, prec ≥0.90)
[RUN] metric 3 [petclinic]: recall 6/6 (1.00)  precision 6/6 (1.00)  OK (порог: 6 рёбер, prec ≥0.90)
[RUN] metric 18 [boutique]: rev 38e7348eb байт-в-байт=да sha256=bcb75cd6b394 schema=ok
[RUN] metric 19 [boutique]: rev 38e7348eb время 1.34s ≤ 10s OK
[RUN] metric 18 [petclinic]: rev 1d76b00d6 байт-в-байт=да sha256=ae567916834f schema=ok
[RUN] metric 19 [petclinic]: rev 1d76b00d6 время 1.03s ≤ 10s OK
[RUN] metric 18 [buckpal]: rev dc819c666 байт-в-байт=да sha256=15dc5cc799b4 schema=ok
[RUN] metric 19 [buckpal]: rev dc819c666 время 0.29s ≤ 10s OK
[RUN] metric 18 [library]: rev 5225ff7a0 байт-в-байт=да sha256=548a73d3673b schema=ok
[RUN] metric 19 [library]: rev 5225ff7a0 время 0.69s ≤ 10s OK
[SKIP] metric 4 (хранилища recall): нет раннера хранилищ на эталоне (recover реализован)
[SKIP] metric 5 (внешние precision): нет раннера внешних систем на эталоне (recover реализован)
[SKIP] metric 6 (шум карты survey): грунт-трус шума survey отсутствует
[SKIP] metric 11 (Kontur-bench измеримых кейсов): волна K (redteam) не реализована в 0.3.17
[SKIP] metric 12 (Kontur на реальном коде: detection/FP): волна K (redteam) не реализована в 0.3.17
[SKIP] metric 20 (квалификация судьи: различительная сила): рубрики/судья — вне Python-прогонщиков
[SKIP] metric 14 (selftest из коробки): измеряется в CI (cargo test), не прогонщиком бенчмарка
[SKIP] metric 17 (golden judge MAE / доля в диапазоне): требует API-ключ судьи — вне офлайн-прогона
```

## Счётчики

| маркер | число |
|--------|-------|
| RUN    | 47 |
| SKIP   | 9 |
| FIND   | 2 |
