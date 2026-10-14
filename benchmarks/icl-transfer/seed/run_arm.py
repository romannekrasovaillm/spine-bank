#!/usr/bin/env python3
"""Run one experimental arm. Usage: run_arm.py <arm: S|F|C> <replicate-id>"""
import json, os, shutil, subprocess, sys, time

LAB = "/home/claude/lab"
BIN = "/home/claude/spine-bank/target/release/arch-be"
CFG = f"{LAB}/arch-harness.toml"
TASKS = json.load(open(f"{LAB}/tasks.json"))
arm, rep = sys.argv[1], sys.argv[2]
name = f"{arm}{rep}"
root = f"{LAB}/runs/{name}"
repo = f"{root}/paylib"
shutil.rmtree(root, ignore_errors=True)
os.makedirs(root)
shutil.copytree(f"{LAB}/template/paylib", repo)
ISO = arm in ("SI", "FI")
if ISO:  # isolated repos: every task starts from the clean baseline -> no repo memory
    for t in TASKS:
        shutil.copytree(f"{LAB}/template/paylib", f"{root}/{t['id']}/paylib")
ev = open(f"{root}/events.jsonl", "a")


def log(**k):
    k["ts"] = time.time()
    ev.write(json.dumps(k, ensure_ascii=False) + "\n"); ev.flush()


HEAD = f"""Репозиторий: {repo} (git). Ты — архитектор и код не пишешь сам: каждую задачу реализует кодовый агент claude-code через инструмент harness_run (harness="claude-code", path="{repo}", task=<твоё задание исполнителю>).
Порядок работы: задачи выполняются СТРОГО последовательно, одна задача — один вызов harness_run (повторный вызов — только для исправлений по той же задаче).
После каждого прогона проверь результат приёмочным гейтом: bash `python3 {LAB}/gate/accept.py {repo}`. Если гейт FAIL — сформулируй исполнителю исправления и повтори harness_run (не более 3 прогонов на задачу). Правки кода и файлов репозитория сам не делай, гейт не редактируй. Затем переходи к следующей задаче.
"""
TAIL = "\nВ конце выведи таблицу: задача | прогонов harness_run | итог гейта."


def spine(prompt, home, tag):
    shutil.rmtree(home, ignore_errors=True)
    shutil.copytree(f"{LAB}/homes/_tpl", home)
    env = dict(os.environ, ARCH_HOME=home, SHIM_KEY="x")
    t0 = time.time()
    with open(f"{root}/{tag}.out", "w") as out:
        p = subprocess.run([BIN, "--config", CFG, "run", "--timeout", "7000", prompt], cwd=repo, env=env,
                           stdout=out, stderr=subprocess.STDOUT, timeout=7500)
    log(kind="spine_session", tag=tag, rc=p.returncode, secs=round(time.time() - t0, 1))


CODER = ["claude", "-p", "--model", "sonnet", "--permission-mode", "acceptEdits", "--allowedTools",
         "Read", "Write", "Edit", "Glob", "Grep", "Bash(python3:*)", "Bash(pytest:*)", "Bash(git add:*)",
         "Bash(git commit:*)", "Bash(git status:*)", "Bash(git diff:*)", "Bash(git log:*)", "Bash(ls:*)",
         "Bash(cat:*)", "Bash(mkdir:*)"]

HEAD_ISO = f"""Ты — архитектор и код не пишешь сам: каждую задачу реализует кодовый агент claude-code через инструмент harness_run (harness="claude-code", path=<репозиторий задачи>, task=<твоё задание исполнителю>).
Каждая задача выполняется в СВОЁМ git-репозитории (копии одной и той же кодовой базы paylib); путь указан у задачи.
Порядок работы: задачи выполняются СТРОГО последовательно, одна задача — один вызов harness_run (повторный вызов — только для исправлений по той же задаче).
После каждого прогона проверь результат приёмочным гейтом: bash `python3 {LAB}/gate/accept.py <репозиторий задачи>`. Если гейт FAIL — сформулируй исполнителю исправления и повтори harness_run (не более 3 прогонов на задачу). Правки кода и файлов репозиториев сам не делай, гейт не редактируй. Затем переходи к следующей задаче.
"""


def tline(t):
    return f"{t['id']} (репозиторий {root}/{t['id']}/paylib). {t['text']}"


if arm == "SI":
    body = "Задачи:\n" + "\n".join(tline(t) for t in TASKS)
    os.chdir(root)
    repo = root
    spine(HEAD_ISO + body + TAIL, f"{LAB}/homes/{name}", "session")
elif arm == "FI":
    repo = root
    for t in TASKS:
        log(kind="task_start", task=t["id"])
        spine(HEAD_ISO + "Задача:\n" + tline(t) + TAIL, f"{LAB}/homes/{name}-{t['id']}", t["id"])
elif arm == "S":
    body = "Задачи:\n" + "\n".join(f"{t['id']}. {t['text']}" for t in TASKS)
    spine(HEAD + body + TAIL, f"{LAB}/homes/{name}", "session")
elif arm == "F":
    for t in TASKS:
        log(kind="task_start", task=t["id"])
        spine(HEAD + f"Задача:\n{t['id']}. {t['text']}" + TAIL, f"{LAB}/homes/{name}-{t['id']}", t["id"])
elif arm == "C":
    for t in TASKS:
        log(kind="task_start", task=t["id"])
        t0 = time.time()
        with open(f"{root}/{t['id']}.out", "w") as out:
            subprocess.run(CODER, input=t["text"] + "\nПо завершении закоммить изменения в git.", text=True,
                           cwd=repo, stdout=out, stderr=subprocess.STDOUT, timeout=1800)
        subprocess.run(["git", "add", "-A"], cwd=repo)
        subprocess.run(["git", "-c", "user.email=lab@x", "-c", "user.name=lab", "commit", "-qm", f"auto {t['id']}"], cwd=repo)
        g = subprocess.run([sys.executable, f"{LAB}/gate/accept.py", repo], capture_output=True, text=True)
        log(kind="coder_task", task=t["id"], secs=round(time.time() - t0, 1), gate=g.stdout)
log(kind="arm_done")
