#!/usr/bin/env python3
import json, glob, os, collections, re

LAB = "/home/user/lab"
TASKS = json.load(open(f"{LAB}/tasks.json"))
MOD = {t["id"]: t["module"] for t in TASKS}
gl = [json.loads(l) for l in open(f"{LAB}/old/gate-log-v0.jsonl") if "/runs/C1/" in l]
gl += [json.loads(l) for l in open(f"{LAB}/gate/gate-log.jsonl")]
stats = json.load(open(f"{LAB}/tool_stats.json"))
HIDDEN = {"G5", "G6"}

runs = sorted(os.path.basename(p) for p in glob.glob(f"{LAB}/runs/*") if re.match(r"^(S|F|C|SI|FI)\d$", os.path.basename(p)))
out = {}
for run in runs:
    arm = re.match(r"^[A-Z]+", run).group(0)
    rows = []
    for t in TASKS:
        tid, m = t["id"], t["module"]
        repo = f"{LAB}/runs/{run}/{tid}/paylib" if arm in ("SI", "FI") else f"{LAB}/runs/{run}/paylib"
        calls = [g for g in gl if g["repo"] == repo and m in g["modules"]]
        if not calls:
            rows.append({"task": tid, "attempted": False}); continue
        first = calls[0]
        v1 = sorted({v[0] for v in first["violations"] if v[1] == m})
        last = calls[-1]
        vl = sorted({v[0] for v in last["violations"] if v[1] == m})
        # dispatches for this task (Spine arms): harness_run tasks mentioning module
        if arm in ("S", "SI"):
            hts = stats.get(run, {}).get("harness_tasks", [])
        elif arm in ("F", "FI"):
            hts = stats.get(f"{run}-{tid}", {}).get("harness_tasks", [])
        else:
            hts = []
        def owner(h):
            txt = str(h.get("task", ""))
            mm = re.search(r"модул[ьяе]\s+(?:paylib[/.])?`?(\w+)", txt)
            if mm and mm.group(1) in MOD.values():
                return mm.group(1)
            mt = re.search(r"\bT(\d)\b", txt)
            return MOD.get("T" + mt.group(1)) if mt else None
        mine = [h for h in hts if owner(h) == m]
        if arm in ("F", "FI"):
            mine = hts
        if arm in ("SI",):
            mine = [h for h in hts if f"/{tid}/" in str(h.get("path", ""))]
        conv = [("__all__" in str(h.get("task", "")) and "CATALOG" in str(h.get("task", ""))) for h in mine]
        rows.append({"task": tid, "attempted": True, "first_codes": v1, "first_pass": not v1,
                     "first_hidden_fail": bool(set(v1) & HIDDEN), "final_pass": not vl,
                     "gate_calls": len(calls), "dispatches": len(mine) if arm != "C" else 1,
                     "first_prompt_has_conventions": conv[0] if conv else None})
    out[run] = {"arm": arm, "rows": rows}

json.dump(out, open(f"{LAB}/h1_results.json", "w"), ensure_ascii=False, indent=1)
for run, r in out.items():
    print(run, " ".join(
        (f"{x['task']}:{'P' if x['first_pass'] else 'F'}{''.join(c[1] for c in x['first_codes'])}/d{x['dispatches']}/{'K' if x['first_prompt_has_conventions'] else '-'}/{'ok' if x['final_pass'] else 'X'}"
         if x["attempted"] else f"{x['task']}:--") for x in r["rows"]))
