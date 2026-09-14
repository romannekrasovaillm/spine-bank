#!/usr/bin/env python3
"""massrun_generate.py — 10 задач × 3 контекста × 3 прогона × 2 модели = 180.

Решатели:
  ds  — deepseek-chat @ https://api.deepseek.com (ключ из env DEEPSEEK_API_KEY)
  glm — glm-5.3-flash @ http://127.0.0.1:8787/v1 (llm-proxy, ключ из env ZHIPU_API_KEY)
Температура 0.7, max_tokens 6000, таймаут 300с, ретраи ×3.
Каждая ячейка: prompt.txt, raw.md, src/main.rs, meta.json.
Сбои фиксируются (meta.json error), не подменяются. Ключи не печатаются.
Только stdlib.
"""
import json
import os
import re
import threading
import time
import urllib.request
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

BASE = Path(__file__).resolve().parent
CELLS = BASE / "cells"
RUNS = 3
TEMPERATURE = 0.7
MAX_TOKENS = 6000

TASKS_MD = (BASE / "TASKS.md").read_text(encoding="utf-8")
TASKS = dict(re.findall(
    r"## (T\d\d) — [^\n]+\n(.+?)(?=\n## T\d\d |\Z)", TASKS_MD, re.DOTALL))
assert len(TASKS) == 10

SOLVERS = {
    "ds": {
        "url": "https://api.deepseek.com/chat/completions",
        "model": "deepseek-chat",
        "key_env": "DEEPSEEK_API_KEY",
        "extra": {},
    },
    "glm": {
        "url": "http://127.0.0.1:8787/v1/chat/completions",
        "model": "glm-5.3-flash",
        "key_env": "ZHIPU_API_KEY",
        "extra": {"reasoning_effort": "low"},
    },
}

_sem = {"ds": threading.Semaphore(8), "glm": threading.Semaphore(4)}
_log_lock = threading.Lock()


def load_context(task_id, variant):
    if variant == "A":
        return None
    p = BASE / "contexts" / f"{task_id}_{variant}.md"
    return p.read_text(encoding="utf-8").strip()


def build_prompt(task_id, variant):
    task = TASKS[task_id].strip()
    ctx = load_context(task_id, variant)
    if ctx is None:
        return task
    return f"Контекст:\n{ctx}\n\nЗадача:\n{task}"


def call_api(solver, prompt, tries=3):
    cfg = SOLVERS[solver]
    key = os.environ.get(cfg["key_env"])
    if not key:
        return "", None, {}, f"нет env {cfg['key_env']}"
    payload = {
        "model": cfg["model"],
        "messages": [{"role": "user", "content": prompt}],
        "temperature": TEMPERATURE,
        "max_tokens": MAX_TOKENS,
        **cfg["extra"],
    }
    last = None
    for attempt in range(tries):
        try:
            req = urllib.request.Request(
                cfg["url"], data=json.dumps(payload).encode(),
                headers={"Authorization": f"Bearer {key}",
                         "Content-Type": "application/json"})
            with urllib.request.urlopen(req, timeout=300) as r:
                body = json.loads(r.read())
            choice = body["choices"][0]
            msg = choice["message"]
            content = msg.get("content") or ""
            finish = choice.get("finish_reason")
            if not content.strip():
                last = f"пустой ответ (finish={finish})"
                time.sleep(5 * (attempt + 1))
                continue
            return content, body.get("model"), body.get("usage", {}), None
        except Exception as e:
            last = f"{type(e).__name__}: {str(e)[:160]}"
            time.sleep(5 * (attempt + 1))
    return "", None, {}, last


def extract_code(text):
    m = re.search(r"```(?:rust|rs)?\s*\n(.*?)```", text, re.DOTALL)
    return (m.group(1) if m else text).strip() + "\n"


def gen_cell(task_id, variant, solver, run):
    label = f"{task_id}_{variant}_{solver}_{run}"
    cell = CELLS / label
    (cell / "src").mkdir(parents=True, exist_ok=True)
    prompt = build_prompt(task_id, variant)
    (cell / "prompt.txt").write_text(prompt, encoding="utf-8")
    t0 = time.time()
    with _sem[solver]:
        content, echoed, usage, err = call_api(solver, prompt)
    dt = time.time() - t0
    meta = {"cell": label, "task": task_id, "context": variant,
            "solver": solver, "run": run,
            "model_requested": SOLVERS[solver]["model"],
            "model_echo": echoed, "temperature": TEMPERATURE,
            "error": err, "secs": round(dt, 1), "usage": usage,
            "raw_chars": len(content),
            "ts": time.strftime("%Y-%m-%dT%H:%M:%S%z")}
    (cell / "raw.md").write_text(content, encoding="utf-8")
    if not err:
        code = extract_code(content)
        (cell / "src" / "main.rs").write_text(code, encoding="utf-8")
        meta["code_chars"] = len(code)
    (cell / "meta.json").write_text(
        json.dumps(meta, ensure_ascii=False, indent=2), encoding="utf-8")
    with _log_lock:
        with (BASE / "logs" / "generations.jsonl").open("a", encoding="utf-8") as f:
            f.write(json.dumps(meta, ensure_ascii=False) + "\n")
    status = f"ERR {err}" if err else f"ok {dt:.0f}с echo={echoed}"
    print(f"{label}: {status}", flush=True)


def main():
    jobs = [(t, v, s, r)
            for t in sorted(TASKS)
            for v in ("A", "B", "C")
            for s in ("ds", "glm")
            for r in (1, 2, 3)]
    # пропуск уже готовых ячеек (идемпотентный перезапуск)
    jobs = [j for j in jobs
            if not (CELLS / f"{j[0]}_{j[1]}_{j[2]}_{j[3]}" / "meta.json").is_file()]
    print(f"ячеек к генерации: {len(jobs)}", flush=True)
    with ThreadPoolExecutor(max_workers=10) as ex:
        list(ex.map(lambda j: gen_cell(*j), jobs))
    failed = [f"{t}_{v}_{s}_{r}" for t, v, s, r in jobs
              if not (CELLS / f"{t}_{v}_{s}_{r}" / "src" / "main.rs").is_file()]
    print("сбоев:", len(failed), failed if failed else "")


if __name__ == "__main__":
    main()
