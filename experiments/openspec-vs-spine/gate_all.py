#!/usr/bin/env python3
"""gate_all.py — единый гейт для всех ячеек массового прогона.

Для каждой ячейки cells/<label> с src/main.rs:
  arch-be control check <cell> --constraints CONSTRAINTS.massrun.yaml --json
Результат — results.jsonl: метаданные генерации (из meta.json) + нарушения
по каждому правилу (errors/warns). Ячейки без кода — запись с gate=skipped.
Только stdlib.
"""
import json
import subprocess
from pathlib import Path

BASE = Path(__file__).resolve().parent
CONSTRAINTS = BASE / "CONSTRAINTS.massrun.yaml"
ARCH_BE = "/home/user/.local/bin/arch-be"
RULES = ["no-f64-money", "idempotency-key", "no-unwrap-expect-panic",
         "no-pii-logs", "typed-errors", "no-global-mutable-state",
         "blocking-network", "no-hardcoded-secrets", "no-string-errors",
         "money-representation"]

records = []
cells = sorted(p for p in (BASE / "cells").iterdir() if p.is_dir())
for cell in cells:
    meta_p = cell / "meta.json"
    meta = json.loads(meta_p.read_text(encoding="utf-8")) if meta_p.is_file() else {}
    rec = {"cell": cell.name, "task": meta.get("task"), "context": meta.get("context"),
           "solver": meta.get("solver"), "run": meta.get("run"),
           "model_echo": meta.get("model_echo"), "gen_error": meta.get("error"),
           "secs": meta.get("secs"), "usage": meta.get("usage")}
    if not (cell / "src" / "main.rs").is_file():
        rec["gate"] = "skipped_no_code"
        records.append(rec)
        print(f"{cell.name}: пропуск (нет кода, сбой генерации)")
        continue
    proc = subprocess.run(
        [ARCH_BE, "control", "check", str(cell),
         "--constraints", str(CONSTRAINTS), "--json"],
        capture_output=True, text=True, timeout=120)
    try:
        report = json.loads(proc.stdout)
    except json.JSONDecodeError:
        rec["gate"] = "gate_error"
        rec["gate_stderr"] = (proc.stderr or proc.stdout)[:200]
        records.append(rec)
        print(f"{cell.name}: гейт упал: {proc.stderr[:120]}")
        continue
    errors = [i for i in report["issues"] if i["severity"] == "error"]
    warns = [i for i in report["issues"] if i["severity"] != "error"]
    rec["gate"] = "ok"
    rec["passed"] = report["passed"]
    rec["errors_total"] = len(errors)
    rec["warns_total"] = len(warns)
    rec["per_rule_errors"] = {r: sum(1 for i in errors if i["rule"] == r) for r in RULES}
    rec["per_rule_warns"] = {r: sum(1 for i in warns if i["rule"] == r) for r in RULES}
    records.append(rec)
    print(f"{cell.name}: errors={len(errors)} warns={len(warns)}")

with (BASE / "results.jsonl").open("w", encoding="utf-8") as f:
    for rec in records:
        f.write(json.dumps(rec, ensure_ascii=False) + "\n")
print(f"\nзаписано {len(records)} записей -> results.jsonl")
