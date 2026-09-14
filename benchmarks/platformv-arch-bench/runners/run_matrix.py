#!/usr/bin/env python3
"""run_matrix.py — прогон ячеек cells/ (основной массив + расширенный свип).

Параллелизм: ThreadPoolExecutor(6); семафоры: glm-канал (llm-proxy) ≤4,
deepseek-канал ≤5, claude-прокси ≤2, расширенный свип ≤2.
Таймаут ячейки 1200с (raw-llm — 600с), 2 ретрая (всего 3 попытки).
Пропуск ячейки, если answer.md уже есть и ≥ 500 байт (идемпотентность).
Ответ < 500 байт или rc != 0 считается сбоем.
meta.json: {task, condition, model, rep, cmd, secs, exit_code, bytes, ts}.
Лог: logs/generations.jsonl (append, под блокировкой).
Ключи не печатаются. Только stdlib.
"""
import json
import os
import re
import signal
import subprocess
import sys
import threading
import time
import urllib.request
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import pvlib

BASE = pvlib.BASE
CELLS = pvlib.CELLS_DIR
LOGS = pvlib.LOGS_DIR
MIN_ANSWER_BYTES = 500
TIMEOUT_AGENTIC = int(os.environ.get("PVBENCH_TIMEOUT_AGENTIC", "3000"))
TIMEOUT_RAW = 600
RETRIES = int(os.environ.get("PVBENCH_RETRIES", "2"))

MODEL_IDS = {"dsf": "deepseek-flash", "glm": "glm-5.3-flash",
             "glm53": "glm-5.3", "dsp": "deepseek-v4-pro"}
ARCH_MODELS = {"dsf": "deepseek-bench", "glm": "glm-flash-bench",
               "glm53": "glm-5.3", "dsp": "deepseek-pro-bench"}
CLAUDE_MODELS = {"dsf": "deepseek-flash", "dsp": "deepseek-v4-pro",
               "glm": "glm-5.3-flash", "glm53": "glm-5.3"}
RAW_CFG = {
    "dsf": {"url": "https://api.deepseek.com/v1/chat/completions",
            "model": "deepseek-chat", "key_env": "DEEPSEEK_API_KEY",
            "max_tokens": 16000},
    "glm": {"url": "http://127.0.0.1:8795/v1/chat/completions",
            "model": "glm-5.3-flash", "key_env": "ZHIPU_API_KEY",
            "max_tokens": 32768, "stream": True, "effort": "low"},
    # glm53/dsp — ризонящие: reasoning съедает лимит, нужен запас
    "glm53": {"url": "http://127.0.0.1:8795/v1/chat/completions",
              "model": "glm-5.3", "key_env": "ZHIPU_API_KEY",
              "max_tokens": 65536, "stream": True, "effort": "low"},
    "dsp": {"url": "https://api.deepseek.com/v1/chat/completions",
            "model": "deepseek-v4-pro", "key_env": "DEEPSEEK_API_KEY",
            "max_tokens": 32000},
}
KIMI = os.environ.get("KIMI_BIN", "kimi")
THESEUS_MAX_TURNS = os.environ.get("PVBENCH_THESEUS_MAX_TURNS", "12")

SEM = {
    "deepseek": threading.Semaphore(5),  # arch-be dsf + theseus dsf + raw dsf
    "glm": threading.Semaphore(int(os.environ.get("PVBENCH_GLM_PAR", "4"))),
    "claude": threading.Semaphore(2),
    "sweep": threading.Semaphore(2),
}
_log_lock = threading.Lock()
ANSI = re.compile(r"\x1b\[[0-9;]*m")


def channel(cond, model):
    if cond.startswith(("dsh", "codewhale", "hermes")):
        return "sweep"
    if cond.startswith("claude"):
        return "claude"
    if model in ("glm", "glm53"):
        return "glm"
    return "deepseek"


def build_cmd(task, cond, model, prompt, uniq=""):
    """argv или None для raw-llm. В meta пишем cmd с $(cat prompt.txt)."""
    if cond in ("spine-arch", "spine-min"):
        argv = ["arch-be", "run", "-q", "--model", ARCH_MODELS[model],
                "--timeout", "2200"]
        # D10: deepseek-модели бенча — с --think off, иначе reasoning съедает
        # max_tokens и финальный документ не генерируется; у glm thinking
        # не отключается (HTTP 1210), флаг не добавляем.
        if model in ("dsf", "dsp"):
            argv += ["--think", "off"]
        return argv + [prompt]
    if cond == "spine-arch-think":
        # D11: рука «spine + ризонинг»: max_tokens 65536 (dsf) —
        # reasoning + финальный документ; сравнивается со spine-arch (off)
        # и с theseus/openclaw (у которых ризонинг max по дефолту).
        # glm: thinking у GLM всегда включён — это и есть «think»-режим.
        think_models = {"dsf": "deepseek-bench-think",
                        "dsp": "deepseek-pro-bench-think",
                        "glm": "glm-flash-bench"}
        return ["arch-be", "run", "-q", "--model", think_models[model],
                "--think", "on", "--timeout", "3600", prompt]
    if cond.startswith("theseus"):
        return ["theseus", "-m", MODEL_IDS[model], "--yolo",
                "--max-turns", THESEUS_MAX_TURNS, "-p", prompt]
    if cond.startswith("claude"):
        argv = ["claude", "-p", prompt, "--model", CLAUDE_MODELS[model],
                "--output-format", "text", "--dangerously-skip-permissions"]
        if model in ("glm", "glm53"):
            # D13: glm для claude — через Anthropic-совместимый endpoint Z.AI
            # (штатный claude-прокси машины обслуживает только deepseek-*)
            return ["env",
                    "ANTHROPIC_BASE_URL=https://open.bigmodel.cn/api/anthropic",
                    f"ANTHROPIC_AUTH_TOKEN={os.environ.get('ZHIPU_API_KEY', '')}"] + argv
        return argv
    if cond == "dsh-plain":
        return ["dsh", "--profile", "headless", prompt]
    if cond == "codewhale-plain":
        return ["codewhale", "exec", prompt]
    if cond == "hermes-plain":
        return ["hermes", "-z", prompt]
    if cond == "openclaw-plain":
        # сессионный ключ уникален на ячейку — иначе повторы продолжают
        # чужую сессию; модель фиксируем, чтобы условие попадало в канал dsf
        return ["openclaw", "agent", "--local", "-m", prompt, "--json",
                "--model", f"deepseek/{MODEL_IDS[model]}",
                "--session-key", f"pvbench-{uniq or task}"]
    if cond.startswith("kimi"):
        if model in ("glm", "glm53"):
            # D17: glm для kimi — через OpenRouter (z-ai/glm-5.3-flash,
            # отдельный пул лимитов от аккаунта Z.AI); RF-доступ — через
            # vpn-egress :12080
            return ["env", "HTTPS_PROXY=http://127.0.0.1:12080/",
                    "HTTP_PROXY=http://127.0.0.1:12080/",
                    KIMI, "-p", prompt, "-m", f"openrouter/{MODEL_IDS[model]}"]
        provider = {"dsf": "deepseek", "dsp": "deepseek"}.get(model, "deepseek")
        return [KIMI, "-p", prompt, "-m", f"{provider}/{MODEL_IDS[model]}"]
    if cond == "qwen-plain":
        # qwen-code игнорирует modelProviders при заданном OPENAI_BASE_URL;
        # в env машины OPENAI_BASE_URL содержит битый суффикс /v1alias (404) —
        # переопределяем явно. Имя модели — API-идентификатор deepseek.
        return ["env", "OPENAI_BASE_URL=https://api.deepseek.com/v1",
                "OPENAI_MODEL=deepseek-flash",
                "qwen", "-p", prompt, "--approval-mode", "yolo"]
    if cond == "omp-plain":
        # pi-coding-agent: провайдер deepseek настроен в ~/.omp/agent/models.yml
        return ["omp", "--print", "--model", f"deepseek/{MODEL_IDS[model]}",
                prompt]
    return None  # raw-llm


def raw_call(model, prompt, timeout):
    cfg = RAW_CFG[model]
    key = os.environ.get(cfg["key_env"])
    if not key:
        return "", None, None, f"нет env {cfg['key_env']}"
    payload = {"model": cfg["model"],
               "messages": [{"role": "user", "content": prompt}],
               "temperature": 0.7,
               "max_tokens": cfg.get("max_tokens", 16000)}
    if cfg.get("effort"):
        payload["thinking"] = {"type": "enabled"}
        payload["reasoning_effort"] = cfg["effort"]
    # glm-семейство ризонит: нестриминговый ответ собирается дольше таймаута
    # прокси (503 ровно на 480с) — для них идём SSE и склеиваем delta.content
    if cfg.get("stream"):
        payload["stream"] = True
        try:
            req = urllib.request.Request(
                cfg["url"], data=json.dumps(payload).encode(),
                headers={"Authorization": f"Bearer {key}",
                         "Content-Type": "application/json"})
            parts, finish, model_out = [], None, None
            with urllib.request.urlopen(req, timeout=timeout) as r:
                for raw_line in r:
                    line = raw_line.decode("utf-8", "replace").strip()
                    if not line.startswith("data:"):
                        continue
                    data = line[5:].strip()
                    if data == "[DONE]":
                        break
                    try:
                        chunk = json.loads(data)
                    except json.JSONDecodeError:
                        continue  # keep-alive/неполные SSE-кадры прокси
                    model_out = chunk.get("model") or model_out
                    for ch in chunk.get("choices") or []:
                        delta = ch.get("delta") or {}
                        if delta.get("content"):
                            parts.append(delta["content"])
                        if ch.get("finish_reason"):
                            finish = ch["finish_reason"]
            return "".join(parts), model_out, finish, None
        except Exception as e:
            return "", None, None, f"{type(e).__name__}: {str(e)[:160]}"
    try:
        req = urllib.request.Request(
            cfg["url"], data=json.dumps(payload).encode(),
            headers={"Authorization": f"Bearer {key}",
                     "Content-Type": "application/json"})
        with urllib.request.urlopen(req, timeout=timeout) as r:
            body = json.loads(r.read())
        ch = body["choices"][0]
        return ch["message"].get("content") or "", body.get("model"), \
            ch.get("finish_reason"), None
    except Exception as e:
        return "", None, None, f"{type(e).__name__}: {str(e)[:160]}"


def extract_theseus(work, stdout):
    """Ответ: последнее assistant-сообщение новейшей сессии .theseus/;
    если оно пустое/короткое — черновик *.md, который агент писал в work/;
    последний fallback — эвристика по stdout."""
    sd = work / ".theseus"
    try:
        sessions = sorted(sd.glob("session-*.json"),
                          key=lambda p: p.stat().st_mtime)
        if sessions:
            d = json.loads(sessions[-1].read_text(encoding="utf-8"))
            msgs = [m for m in d.get("messages", [])
                    if m.get("role") == "assistant"]
            for m in reversed(msgs):
                if len((m.get("content") or "").encode()) >= MIN_ANSWER_BYTES:
                    return m["content"]
    except Exception:
        pass  # fallback — черновик в work/
    cands = [p for p in work.glob("*.md")
             if p.name not in ("TASK.md", "CONTEXT.md", "AGENTS.md")
             and p.stat().st_size >= 2000]
    if cands:
        best = max(cands, key=lambda p: p.stat().st_mtime)
        return best.read_text(encoding="utf-8")
    clean = ANSI.sub("", stdout)
    lines = clean.splitlines()
    try:
        end = next(i for i, l in enumerate(lines) if l.startswith("⚙ finish"))
    except StopIteration:
        end = len(lines)
    body = lines[1:end] if lines and lines[0].startswith("❯") else lines[:end]
    body = [re.sub(r"\(мышление: \d+ символов\)\s*$", "", l) for l in body]
    return "\n".join(body).strip()


def extract_openclaw(stdout):
    d = json.loads(stdout)
    return "\n".join(p.get("text") or "" for p in d.get("payloads", []))


def run_once(task, cond, model, cell):
    """Одна попытка. Возвращает (answer|None, cmd_str, exit_code, note, err)."""
    work = cell / "work"
    prompt = (cell / "prompt.txt").read_text(encoding="utf-8")
    if cond == "raw-llm":
        answer, echo, finish, err = raw_call(model, prompt, TIMEOUT_RAW)
        cmd_str = f"raw POST {RAW_CFG[model]['url']} model={RAW_CFG[model]['model']}"
        note = f"echo={echo} finish={finish}"
        if not err and finish == "length":
            err = "ответ обрезан (finish=length)"
            answer = None  # обрезанный ответ считаем сбоем, см. PREREGISTRATION
        return (answer or None), cmd_str, (0 if not err else 1), note, err
    argv = build_cmd(task, cond, model, prompt, uniq=cell.name)
    cmd_str = " ".join(a if a != prompt else "$(cat prompt.txt)" for a in argv)
    try:
        # start_new_session + killpg: иначе внуки (node-субпроцессы theseus/
        # claude) держат pipe после kill прямого ребёнка, и communicate()
        # висит бесконечно (инцидент 11.09 — дедлок воркеров на 7.5 ч).
        proc = subprocess.Popen(argv, cwd=work, stdout=subprocess.PIPE,
                                stderr=subprocess.PIPE, text=True,
                                start_new_session=True)
        try:
            out, errout = proc.communicate(timeout=TIMEOUT_AGENTIC)
        except subprocess.TimeoutExpired:
            os.killpg(proc.pid, signal.SIGKILL)
            out, errout = proc.communicate()
            return None, cmd_str, 124, None, f"timeout {TIMEOUT_AGENTIC}s"
    except Exception as e:
        return None, cmd_str, 1, None, f"{type(e).__name__}: {str(e)[:160]}"
    if proc.returncode != 0:
        err = (errout or out)[-300:]
        # theseus exit 3 = лимит ходов: финального ответа нет, но агент мог
        # успеть записать черновик/ответ файлом в work/ — извлекаем его
        # (неполный ответ судится как есть, DEVIATIONS D7) вместо сжигания
        # повторных попыток по ~2.5M токенов.
        if cond.startswith("theseus"):
            salvaged = extract_theseus(work, out or "")
            if salvaged and len(salvaged.encode("utf-8")) >= MIN_ANSWER_BYTES:
                return salvaged, cmd_str, proc.returncode, \
                    f"salvage при exit {proc.returncode} (лимит ходов)", None
        return None, cmd_str, proc.returncode, None, err
    out = out or ""
    if cond.startswith("theseus"):
        answer = extract_theseus(work, out)
    elif cond == "openclaw-plain":
        try:
            answer = extract_openclaw(out)
        except Exception as e:
            return None, cmd_str, 0, None, f"json-parse: {str(e)[:120]}"
    elif cond == "kimi-plain":
        answer = re.sub(r"^• ", "", ANSI.sub("", out).strip())
    else:
        answer = out.strip()
    # D18: arch-be в агентном режиме иногда пишет полный документ в
    # work/answer.md, а на stdout отдаёт краткое подтверждение («Готово…»)
    # — судья получал «обрыв» вместо документа. Salvage: если stdout
    # не похож на итоговый документ (нет заголовка или короткий), а
    # work/answer.md существенно полнее — берём его.
    if cond.startswith("spine"):
        draft = work / "answer.md"
        looks_like_doc = answer.lstrip().startswith("#") and \
            len(answer.encode("utf-8")) >= 2000
        if not looks_like_doc and draft.is_file() and \
                draft.stat().st_size >= 2000 and \
                draft.stat().st_size > len(answer.encode("utf-8")):
            answer = draft.read_text(encoding="utf-8")
            cmd_str += " [salvage: work/answer.md]"
    if len(answer.encode("utf-8")) < MIN_ANSWER_BYTES:
        return None, cmd_str, 0, None, \
            f"ответ слишком короткий ({len(answer.encode())} байт)"
    return answer, cmd_str, 0, None, None


def gen_cell(cell_name):
    m = re.match(r"^(.+?)__(.+)__(dsf|glm|glm53|dsp|default)__r(\d+)$", cell_name)
    task, cond, model, rep = m.group(1), m.group(2), m.group(3), int(m.group(4))
    cell = CELLS / cell_name
    t0 = time.time()
    answer, cmd_str, rc, note, err = None, "", 1, None, None
    sem = SEM[channel(cond, model)]
    for attempt in range(RETRIES + 1):
        with sem:
            answer, cmd_str, rc, note, err = run_once(task, cond, model, cell)
        if answer is not None:
            break
        time.sleep(20 * (attempt + 1))
    dt = time.time() - t0
    meta = {"cell": cell_name, "task": task, "condition": cond, "model": model,
            "rep": rep, "cmd": cmd_str, "secs": round(dt, 1), "exit_code": rc,
            "bytes": len(answer.encode("utf-8")) if answer else 0,
            "note": note, "error": err,
            "ts": time.strftime("%Y-%m-%dT%H:%M:%S%z")}
    if answer is not None:
        (cell / "answer.md").write_text(answer, encoding="utf-8")
    (cell / "meta.json").write_text(
        json.dumps(meta, ensure_ascii=False, indent=2), encoding="utf-8")
    with _log_lock:
        with (LOGS / "generations.jsonl").open("a", encoding="utf-8") as f:
            f.write(json.dumps(meta, ensure_ascii=False) + "\n")
    print(f"{cell_name}: {'ok ' + str(meta['bytes']) + 'b ' + str(meta['secs']) + 's' if answer else 'ERR ' + str(err)[:100]}",
          flush=True)


def main():
    LOGS.mkdir(exist_ok=True)
    cells = sorted(p.name for p in CELLS.iterdir() if p.is_dir())
    def done(name):
        ap = CELLS / name / "answer.md"
        return ap.is_file() and ap.stat().st_size >= MIN_ANSWER_BYTES
    # PVBENCH_SKIP_MODELS="glm glm53" — пропустить ячейки этих моделей
    # (например, пока upstream Z.AI деградирован); догоняются перезапуском.
    skip_models = set(os.environ.get("PVBENCH_SKIP_MODELS", "").split())
    # PVBENCH_ONLY="qwen-plain openclaw-plain" — прогнать только эти условия
    # (точечный догон новых ячеек параллельно с основным прогоном).
    only_conds = set(os.environ.get("PVBENCH_ONLY", "").split())
    def skipped(name):
        m = re.match(r"^(.+?)__(.+)__(dsf|glm|glm53|dsp|default)__r(\d+)$", name)
        return bool(m) and m.group(3) in skip_models
    def not_only(name):
        m = re.match(r"^(.+?)__(.+)__(dsf|glm|glm53|dsp|default)__r(\d+)$", name)
        return bool(only_conds) and bool(m) and m.group(2) not in only_conds
    todo = [c for c in cells if not done(c) and not skipped(c)
            and not not_only(c)]
    print(f"ячеек всего: {len(cells)}, к прогону: {len(todo)}"
          f" (пропущено по PVBENCH_SKIP_MODELS: "
          f"{sum(1 for c in cells if not done(c)) - len(todo)})", flush=True)
    if "--dry-run" in sys.argv:
        return
    with ThreadPoolExecutor(
            max_workers=int(os.environ.get("PVBENCH_WORKERS", "8"))) as ex:
        list(ex.map(gen_cell, todo))
    failed = [c for c in todo if not done(c)]
    print(f"сбоев: {len(failed)}", flush=True)
    for c in failed:
        print(f"  FAIL {c}")


if __name__ == "__main__":
    main()
