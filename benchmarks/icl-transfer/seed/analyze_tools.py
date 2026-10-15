#!/usr/bin/env python3
"""Tool-usage statistics from Spine session journals (homes/<run>/sessions/*.jsonl)."""
import glob, json, os, sys, collections

BASE4 = {"bash", "read_file", "write_file", "edit_file"}
GENERAL = BASE4 | {"glob", "grep", "propose_options", "screenshot", "read_image"}
ORCH = {"harness_run", "handoff_create", "subagent_run", "subagent_list", "subagent_result", "ralph_run",
        "worktree_new"}


def load(path):
    out = []
    for l in open(path):
        if l.startswith("#") or not l.strip():
            continue
        try:
            out.append(json.loads(l))
        except Exception:
            pass
    return out


def session_stats(home):
    calls = collections.Counter()
    usage = {"prompt": 0, "completion": 0, "turns": 0}
    harness_tasks = []
    events = collections.Counter()
    for f in sorted(glob.glob(f"{home}/sessions/*.jsonl")):
        for e in load(f):
            k = e.get("kind")
            if k == "assistant":
                usage["turns"] += 1
                for tc in e.get("tool_calls") or []:
                    fn = tc.get("function", tc)
                    n = fn.get("name")
                    calls[n] += 1
                    if n == "harness_run":
                        a = fn.get("arguments") or {}
                        if isinstance(a, str):
                            try:
                                a = json.loads(a)
                            except Exception:
                                a = {"task": a}
                        harness_tasks.append(a)
            elif k == "usage":
                usage["prompt"] += e.get("prompt_tokens", 0) or 0
                usage["completion"] += e.get("completion_tokens", 0) or 0
            elif k == "event":
                events[e.get("event") or e.get("name") or str(e)[:40]] += 1
    return calls, usage, harness_tasks, events


def classify(calls):
    g = sum(v for k, v in calls.items() if k in GENERAL)
    b4 = sum(v for k, v in calls.items() if k in BASE4)
    o = sum(v for k, v in calls.items() if k in ORCH)
    d = sum(v for k, v in calls.items() if k not in GENERAL and k not in ORCH)
    return {"total": sum(calls.values()), "base4": b4, "general": g, "orch": o, "domain": d}


if __name__ == "__main__":
    res = {}
    for home in sorted(glob.glob("/home/user/lab/homes/*")):
        name = os.path.basename(home)
        if name.startswith("_") or name == "smoke":
            continue
        calls, usage, ht, ev = session_stats(home)
        res[name] = {"calls": dict(calls), "class": classify(calls), "usage": usage,
                     "harness_tasks": ht, "events": dict(ev)}
    json.dump(res, open("/home/user/lab/tool_stats.json", "w"), ensure_ascii=False, indent=1)
    for n, r in res.items():
        print(n, r["class"], r["usage"]["turns"], dict(sorted(r["calls"].items(), key=lambda x: -x[1])))
