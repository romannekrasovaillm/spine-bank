#!/usr/bin/env python3
"""generate_contexts.py — контексты B и C от «второй руки».

Модель-генератор: qwen3.8-27b@2026-08-18@llama.cpp (локальная платформа,
keyless). Она НЕ участвует в решении задач. Вход: ORG-STANDARDS.md + TASKS.md.
Промпты нейтральные: «оформи орг-стандарты в формате X», без намёков на гейт.
Результат: contexts/Txx_B.md, contexts/Txx_C.md + contexts/_gen_log.jsonl.
Только stdlib.
"""
import json
import re
import time
import urllib.request
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

# Поправка (2026-09-05T22:47, ДО генераций решателей): платформенная
# qwen3.8-27b оказалась слишком медленной (~4.4 ток/с эффективно, вызовы
# не укладываются в таймаут 300с — факт среды, залогирован). «Вторая рука»
# переназначена на glm-5.3 (полная, НЕ flash) через llm-proxy — модель,
# не участвующая в решении задач.
import os

BASE = Path(__file__).resolve().parent
API = "http://127.0.0.1:8787/v1/chat/completions"
MODEL = "glm-5.3"

ORG = (BASE / "ORG-STANDARDS.md").read_text(encoding="utf-8")
TASKS_MD = (BASE / "TASKS.md").read_text(encoding="utf-8")

TASKS = dict(re.findall(
    r"## (T\d\d) — [^\n]+\n(.+?)(?=\n## T\d\d |\Z)", TASKS_MD, re.DOTALL))
assert len(TASKS) == 10, f"задач распарсено: {len(TASKS)}"

PROMPT_B = (
    "Ниже — документ организационных стандартов разработки и постановка "
    "задачи для разработчика.\n\n=== ОРГ-СТАНДАРТЫ ===\n{org}\n\n"
    "=== ЗАДАЧА ===\n{task}\n\n"
    "Оформи эти стандарты как контекст для разработчика в формате пакета "
    "OpenSpec для этой задачи. Структура ответа:\n"
    "1) `# Spec` — требования в форме SHALL: разделы `## Требование: <имя>`, "
    "внутри формулировки «Система ДОЛЖНА …»;\n"
    "2) `# Design` — прозаическое обоснование архитектурных решений "
    "(почему приняты такие правила);\n"
    "3) `# Rules` — блок правил для агента-исполнителя (как инъецируется "
    "через config.yaml OpenSpec): короткий маркированный список.\n"
    "Не добавляй требований, которых нет в документе стандартов. "
    "Ответ — только текст контекста, без комментариев."
)

PROMPT_C = (
    "Ниже — документ организационных стандартов разработки и постановка "
    "задачи для разработчика.\n\n=== ОРГ-СТАНДАРТЫ ===\n{org}\n\n"
    "=== ЗАДАЧА ===\n{task}\n\n"
    "Оформи эти стандарты как контекст для разработчика в формате спайна: "
    "5–7 инвариантов, каждый вида «N. ЗАПРЕЩЕНО <что>. Проверка: <как "
    "проверить/как правильно>.» Не добавляй требований, которых нет "
    "в документе стандартов. Ответ — только текст инвариантов, "
    "без комментариев."
)


def call_api(prompt, tries=3):
    key = os.environ.get("ZHIPU_API_KEY")
    last = None
    for attempt in range(tries):
        try:
            req = urllib.request.Request(
                API,
                data=json.dumps({
                    "model": MODEL,
                    "messages": [{"role": "user", "content": prompt}],
                    "temperature": 0,
                    "max_tokens": 6000,
                    "reasoning_effort": "low",
                }).encode(),
                headers={"Authorization": f"Bearer {key}",
                         "Content-Type": "application/json"},
            )
            with urllib.request.urlopen(req, timeout=300) as r:
                body = json.loads(r.read())
            choice = body["choices"][0]
            msg = choice["message"]
            return msg.get("content") or "", body.get("usage", {}), None
        except Exception as e:
            last = str(e)[:200]
            time.sleep(5 * (attempt + 1))
    return "", {}, last


def gen(task_id, variant):
    prompt_tpl = PROMPT_B if variant == "B" else PROMPT_C
    prompt = prompt_tpl.format(org=ORG, task=TASKS[task_id].strip())
    t0 = time.time()
    content, usage, err = call_api(prompt)
    dt = time.time() - t0
    out = BASE / "contexts" / f"{task_id}_{variant}.md"
    rec = {"task": task_id, "variant": variant, "error": err,
           "secs": round(dt, 1), "usage": usage, "chars": len(content)}
    with (BASE / "contexts" / "_gen_log.jsonl").open("a", encoding="utf-8") as f:
        f.write(json.dumps(rec, ensure_ascii=False) + "\n")
    if err:
        print(f"{task_id}_{variant}: ERR {err}", flush=True)
        return
    out.write_text(content.strip() + "\n", encoding="utf-8")
    print(f"{task_id}_{variant}: ok ({dt:.0f}с, {len(content)} зн.)", flush=True)


def main():
    jobs = [(t, v) for t in sorted(TASKS) for v in ("B", "C")]
    with ThreadPoolExecutor(max_workers=3) as ex:
        list(ex.map(lambda j: gen(*j), jobs))
    missing = [f"{t}_{v}" for t, v in jobs
               if not (BASE / "contexts" / f"{t}_{v}.md").is_file()]
    print("не сгенерировано:", missing if missing else "нет")


if __name__ == "__main__":
    main()
