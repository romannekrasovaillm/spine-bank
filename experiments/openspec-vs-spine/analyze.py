#!/usr/bin/env python3
"""analyze.py — анализ results.jsonl -> summary.json + таблицы в stdout.

Метрики: суммы error-нарушений по вариантам A/B/C (всего и по моделям),
таблица задача × вариант (суммы по прогонам и моделям), разрез по правилам,
доля генераций с 0 error-нарушений, эффект B→C и A→C,
bootstrap-медианы (честные формулировки, без претензии на строгость).
Только stdlib.
"""
import json
import random
import statistics
from collections import defaultdict
from pathlib import Path

BASE = Path(__file__).resolve().parent
VARIANTS = ("A", "B", "C")
SOLVERS = ("ds", "glm")

records = [json.loads(l) for l in
           (BASE / "results.jsonl").read_text(encoding="utf-8").splitlines() if l.strip()]
gated = [r for r in records if r.get("gate") == "ok"]
failed = [r for r in records if r.get("gate") != "ok"]

by_cell = {r["cell"]: r for r in gated}

def cell_key(task, variant, solver, run):
    return f"{task}_{variant}_{solver}_{run}"

tasks = sorted({r["task"] for r in gated})

# --- суммы по вариантам (всего и по моделям) ---
sums = {v: 0 for v in VARIANTS}
sums_by_solver = {s: {v: 0 for v in VARIANTS} for s in SOLVERS}
clean = {v: 0 for v in VARIANTS}
clean_by_solver = {s: {v: 0 for v in VARIANTS} for s in SOLVERS}
count = {v: 0 for v in VARIANTS}
count_by_solver = {s: {v: 0 for v in VARIANTS} for s in SOLVERS}
per_rule = {v: defaultdict(int) for v in VARIANTS}

for r in gated:
    v, s = r["context"], r["solver"]
    e = r["errors_total"]
    sums[v] += e
    sums_by_solver[s][v] += e
    count[v] += 1
    count_by_solver[s][v] += 1
    if e == 0:
        clean[v] += 1
        clean_by_solver[s][v] += 1
    for rule, n in r["per_rule_errors"].items():
        per_rule[v][rule] += n

# --- таблица задача × вариант (сумма errors по 3 прогонам × 2 моделям = 6 ячеек) ---
table = {}
for t in tasks:
    table[t] = {}
    for v in VARIANTS:
        table[t][v] = sum(by_cell[cell_key(t, v, s, r)]["errors_total"]
                          for s in SOLVERS for r in (1, 2, 3)
                          if cell_key(t, v, s, r) in by_cell)

# --- bootstrap медианы суммы на ячейку (ресемплинг ячеек, 5000 итераций) ---
rng = random.Random(42)
boot = {}
for v in VARIANTS:
    vals = [r["errors_total"] for r in gated if r["context"] == v]
    if not vals:
        continue
    meds = []
    for _ in range(5000):
        sample = [rng.choice(vals) for _ in vals]
        meds.append(statistics.mean(sample))
    meds.sort()
    boot[v] = {"mean": statistics.mean(vals),
               "ci_lo": meds[int(0.025 * len(meds))],
               "ci_hi": meds[int(0.975 * len(meds))],
               "n": len(vals)}

summary = {
    "cells_total": len(records), "gated": len(gated), "failed": len(failed),
    "failed_cells": [r["cell"] for r in failed],
    "errors_by_variant": sums,
    "errors_by_variant_solver": sums_by_solver,
    "clean_share": {v: round(clean[v] / count[v], 4) if count[v] else None for v in VARIANTS},
    "clean_share_solver": {s: {v: round(clean_by_solver[s][v] / count_by_solver[s][v], 4)
                               if count_by_solver[s][v] else None for v in VARIANTS}
                           for s in SOLVERS},
    "counts": count,
    "per_rule_errors": {v: dict(per_rule[v]) for v in VARIANTS},
    "table_task_variant": table,
    "bootstrap_mean_errors_per_cell": boot,
    "effect": {
        "A_to_C": round(1 - sums["C"] / sums["A"], 4) if sums["A"] else None,
        "B_to_C": round(1 - sums["C"] / sums["B"], 4) if sums["B"] else None,
    },
}
(BASE / "summary.json").write_text(
    json.dumps(summary, ensure_ascii=False, indent=2), encoding="utf-8")

# --- печать таблиц ---
print(f"ячеек: {len(records)}, прогейтовано: {len(gated)}, сбоев: {len(failed)}")
if failed:
    print("сбойные:", [r["cell"] for r in failed])
print("\n== Суммы error-нарушений по вариантам ==")
for v in VARIANTS:
    print(f"  {v}: {sums[v]}  (чистых генераций: {clean[v]}/{count[v]}"
          f" = {clean[v]/count[v]*100:.0f}%)" if count[v] else "")
print("\n== По моделям ==")
for s in SOLVERS:
    print(f"  {s}: " + "  ".join(f"{v}={sums_by_solver[s][v]}" for v in VARIANTS)
          + "  | чистые: " + "  ".join(
              f"{v}={clean_by_solver[s][v]}/{count_by_solver[s][v]}" for v in VARIANTS))
print("\n== Задача × вариант (сумма errors по 6 генерациям) ==")
print("задача | A | B | C")
for t in tasks:
    print(f"{t} | {table[t]['A']} | {table[t]['B']} | {table[t]['C']}")
print("\n== Разрез по правилам (errors) ==")
rules = sorted({rule for v in VARIANTS for rule in per_rule[v]})
print("правило | A | B | C")
for rule in rules:
    print(f"{rule} | {per_rule['A'].get(rule,0)} | {per_rule['B'].get(rule,0)} | {per_rule['C'].get(rule,0)}")
print("\n== Bootstrap: среднее errors на ячейку (95% CI, ресемплинг, не строго) ==")
for v in VARIANTS:
    if v in boot:
        b = boot[v]
        print(f"  {v}: mean={b['mean']:.2f} [{b['ci_lo']:.2f}; {b['ci_hi']:.2f}] (n={b['n']})")
print("\n== Эффект ==")
print(f"  A→C: {summary['effect']['A_to_C']}, B→C: {summary['effect']['B_to_C']}")
print("\nзаписано: summary.json")
