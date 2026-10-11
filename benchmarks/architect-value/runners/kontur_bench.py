#!/usr/bin/env python3
"""kontur_bench.py — метрики 11–12 (§9): измеримость контур-бенча и
обнаружение на реальном коде (волна K, ADR-071).

Metric 11: `arch-be redteam --all кейсы/ --format json` — доля измеримых
кейсов (порог ≥6/14); причина неизмеримости печатается (B7: недостающие
раннеры, отсутствие входов пакета).

Metric 12: `arch-be redteam --repo <клон> --standard <стандарт> --format
json` — доля обнаружения засевов стандарта в реальном коде и доля ложных
срабатываний (пороги: detection ≥0.80, FP ≤0.03). Слои раздельно:
text/radar/archunit (стандарт Java-сервисов без archunit-правил — слой
честно не считается, знаменатель — применимые засевы).

Клоны и SHA — repos.lock (те же, что у остальных сьютов). Exit 0 — всегда
кроме неработающего arch-be; SKIP ≠ PASS.
"""
import json
import subprocess
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import avlib  # noqa: E402

TARGET_MEASURED = 6          # метрика 11: ≥6/14 измеримых
TARGET_DETECTION = 0.80      # метрика 12: обнаружение
TARGET_FP = 0.03             # метрика 12: ложные срабатывания


def run_json(args, cwd, timeout=None):
    r = avlib.run_archbe(args, cwd=cwd, timeout=timeout)
    if r.code != 0:
        return None, f"arch-be код {r.code}: {(r.err or r.out)[:160]}"
    try:
        return json.loads(r.out), None
    except json.JSONDecodeError as e:
        return None, f"не JSON: {e}"


def metric11(repo_root):
    cases_dir = repo_root / "кейсы"
    if not cases_dir.is_dir():
        avlib.emit_skip("metric 11", f"нет каталога кейсов {cases_dir}")
        return
    doc, err = run_json(
        ["redteam", "--all", str(cases_dir), "--format", "json"],
        repo_root,
        timeout=1200,
    )
    if err:
        avlib.emit_skip("metric 11", err)
        return
    cases = doc.get("cases") or []
    total = len(cases)
    measured = [c for c in cases if c.get("measured")]
    n = len(measured)
    ok = n >= TARGET_MEASURED
    avlib.emit_run(
        "metric 11",
        f"измеримых кейсов {n}/{total} (порог ≥{TARGET_MEASURED})"
        f"{' OK' if ok else ' BELOW'}; причины по остальным — в выводе redteam --all",
    )
    for c in cases:
        if not c.get("measured"):
            reason = (c.get("reason") or "?").split(";")[0][:100]
            print(f"    не измерен {c.get('name')}: {reason}")


def metric12(repo_root, repos_root):
    std = avlib.GROUND_TRUTH.parent / "standard" / "CONSTRAINTS.java-services.yaml"
    if not std.is_file():
        avlib.emit_skip("metric 12", f"нет стандарта {std}")
        return
    clone = avlib.repo_clone_dir("buckpal", repos_root)
    if clone is None:
        avlib.emit_skip("metric 12", "клон buckpal отсутствует (--repos-dir)")
        return
    doc, err = run_json(
        ["redteam", "--repo", str(clone), "--standard", str(std), "--format", "json"],
        clone,
    )
    if err:
        avlib.emit_skip("metric 12", err)
        return
    det = (doc.get("detection") or {}).get("overall_ratio") or 0.0
    fp = doc.get("false_positive_ratio") or 0.0
    layers = doc.get("detection") or {}
    parts = ", ".join(
        f"{k} {v.get('caught')}/{v.get('total')}"
        for k, v in layers.items()
        if isinstance(v, dict) and k != "overall_ratio"
    )
    ok = det >= TARGET_DETECTION and fp <= TARGET_FP
    avlib.emit_run(
        "metric 12",
        f"K2 на реальном коде (buckpal + стандарт): detection={det:.2f} "
        f"(порог ≥{TARGET_DETECTION}), FP={fp:.2f} (порог ≤{TARGET_FP}) "
        f"[{parts}]{' OK' if ok else ' BELOW'}",
    )


def main(repos_root=None):
    import os
    repos_root = repos_root or Path(
        os.environ.get("ARCH_VALUE_REPOS", "~/bench-repos")
    ).expanduser()
    repo_root = Path(__file__).resolve().parents[3]
    avlib.emit_section("Kontur — метрики 11–12 (волна K)")
    metric11(repo_root)
    metric12(repo_root, repos_root)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
