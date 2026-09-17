---
name: spine-adr-judge
description: Оценка документов (ADR, дизайн, спеки) рубриками Spine без API-ключей у Spine — split-judge через MCP (rubric_prompt → судишь сам k раз → rubric_verify) или rubric_run через CLI-провайдера. Используй этот навык, когда просят «оцени ADR/документ по рубрике», «прогнать рубрику», «нужен вердикт с цитатами», когда rubric_run отвечает «нет ключа» (-32603).
---

# Split-judge: ты — судья, Spine — механика

Spine не содержит LLM. Протокол оценки документа рубрикой через MCP:

1. `rubric_list` — какие рубрики есть (встроенные: `adr_quality`,
   `agents_md_quality`, `architecture_gates`, `handoff_quality`,
   `macedo_dimensions`, `solution_architecture`; свои — путём к файлу).
2. `rubric_prompt {"rubric": "adr_quality", "target": "docs/adr/0001.md"}`
   → получаешь `system_prompt`, `user_prompt`, `response_json_schema`,
   `judge_config` (сколько сэмплов просит рубрика — обычно 3).
3. **Судишь САМ, k раз, независимо** (не копируй себя: меняй порядок
   критериев в голове, перечитывай цель). Каждый ответ — СТРОГО по схеме:
   `scores[]` (criterion_id из рубрики, score в шкале, цитата-доказательство
   из текста при score ≥ 2) + `verdict`. Пустых «4/5 в целом норм» не будет:
   цитата обязана дословно находиться в документе (Spine проверяет).
4. `rubric_verify {"rubric": "adr_quality", "target": "...", "answers": [<k сырых JSON-строк>]}`
   → медиана по критериям, σ → `unstable`, цитаты → `evidence_not_found`,
   итоговый markdown-отчёт.

## Дисциплина

- Если рубрика просит 3 сэмпла — давай 3, а не 2 (иначе помечай отклонение).
- Битый JSON одного ответа не страшен (dropped), но докладывай долю.
- Флаги `unstable`/`evidence_not_found` — не прятать, а объяснить
  архитектору причину (слабый документ / спорный критерий).
- Альтернатива без MCP-цикла: `rubric_run` при настроенной модели
  `kind = "cli"` (Spine вызывает CLI харнесса сам) — тогда просто вызови
  `rubric_run` и передай отчёт.
