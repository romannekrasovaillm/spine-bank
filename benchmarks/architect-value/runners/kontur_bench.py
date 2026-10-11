#!/usr/bin/env python3
"""kontur_bench.py — метрики 11–12 (§9): измеримость контур-бенча и
обнаружение на реальном коде (волна K, ADR-071).

Metric 11: `arch-be redteam --all кейсы/ --format json` — доля измеримых
кейсов (порог ≥6/14); причина неизмеримости печатается (B7: недостающие
раннеры, отсутствие входов пакета).

Metric 12: две ноги контура на реальном коде (§9: «(K2+K3)»):
  • K2 — `arch-be redteam --repo <клон> --standard <стандарт> --format json`:
    доля обнаружения засевов стандарта в реальном коде и доля ложных
    срабатываний (пороги: detection ≥0.80, FP ≤0.03). Слои раздельно:
    text/radar/archunit (стандарт Java-сервисов без archunit-правил — слой
    честно не считается, знаменатель — применимые засевы).
  • K3 — `arch-be redteam <кейс> --corpus <корпус> --format json`: внешний
    корпус патчей реальных архитектурных нарушений (revert-фиксы, засевы) в
    формате unified-diff, применяемый к свежей копии кейса `digital-ruble-
    merchant`. Detection = поймано/найдено; порог §9 ≥0.80 (4/5: S-01 float-
    money честно НЕ ловится — правила нет в реестре кейса).

Итог metric 12 — detection (объединённый по счётчикам K2+K3) и FP (из K2)
против порогов ≥0.80 / ≤0.03. Клоны и SHA — repos.lock. Для K3 путь к кейсу
и корпусу передаётся АБСОЛЮТНЫМ (`Path.resolve()`): `corpus.rs::git_apply`
делает `git -C <мутант> apply <patch>`, поэтому относительный путь патча
разрешается от мутанта, а не от cwd прогонщика.

Exit 0 — всегда кроме неработающего arch-be; SKIP ≠ PASS.
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


def k2_stdcomply(repos_root):
    """K2: засевы стандарта в реальном коде (buckpal). → (caught, total, fp) | None."""
    std = avlib.GROUND_TRUTH.parent / "standard" / "CONSTRAINTS.java-services.yaml"
    if not std.is_file():
        avlib.emit_skip("metric 12 (K2)", f"нет стандарта {std}")
        return None
    clone = avlib.repo_clone_dir("buckpal", repos_root)
    if clone is None:
        avlib.emit_skip("metric 12 (K2)", "клон buckpal отсутствует (--repos-dir)")
        return None
    doc, err = run_json(
        ["redteam", "--repo", str(clone), "--standard", str(std), "--format", "json"],
        clone,
    )
    if err:
        avlib.emit_skip("metric 12 (K2)", err)
        return None
    layers = doc.get("detection") or {}
    caught = sum(int(v.get("caught") or 0) for v in layers.values() if isinstance(v, dict))
    total = sum(int(v.get("total") or 0) for v in layers.values() if isinstance(v, dict))
    fp = doc.get("false_positive_ratio") or 0.0
    parts = ", ".join(
        f"{k} {v.get('caught')}/{v.get('total')}"
        for k, v in layers.items()
        if isinstance(v, dict)
    )
    det = (caught / total) if total else 0.0
    avlib.emit_run(
        "metric 12 (K2, buckpal+стандарт)",
        f"detection={det:.2f} ({caught}/{total}) [{parts}] FP={fp:.2f}",
    )
    return caught, total, fp


def k3_corpus(repo_root):
    """K3: внешний корпус патчей к кейсу digital-ruble-merchant. → (caught,total)|None."""
    case = repo_root / "кейсы" / "digital-ruble-merchant"
    corpus = avlib.BASE / "corpus"
    if not case.is_dir():
        avlib.emit_skip("metric 12 (K3)", f"нет кейса {case}")
        return None
    if not corpus.is_dir():
        avlib.emit_skip("metric 12 (K3)", f"нет корпуса {corpus}")
        return None
    doc, err = run_json(
        [
            "redteam", str(case.resolve()),
            "--corpus", str(corpus.resolve()), "--format", "json",
        ],
        repo_root,
        timeout=1200,
    )
    if err:
        avlib.emit_skip("metric 12 (K3)", err)
        return None
    info = doc.get("corpus") or {}
    found = int(info.get("found") or 0)
    applied = int(info.get("applied") or 0)
    caught = int(info.get("caught") or 0)
    na = int(info.get("not_applicable") or 0)
    rejected = int(info.get("rejected") or 0)
    if found == 0:
        avlib.emit_skip("metric 12 (K3)", "корпус пуст (нет *.patch/*.diff)")
        return None
    det = caught / found
    avlib.emit_run(
        "metric 12 (K3, корпус дефектов)",
        f"detection={det:.2f} ({caught}/{found}); применено {applied}, "
        f"не применилось {na}, отклонено {rejected}"
        + ("" if na == 0 else " — ВНИМАНИЕ: патч корпуса не применился"),
    )
    return caught, found


def metric12(repo_root, repos_root):
    k2 = k2_stdcomply(repos_root)
    k3 = k3_corpus(repo_root)
    if k2 is None and k3 is None:
        avlib.emit_skip("metric 12", "обе ноги (K2/K3) недоступны")
        return
    c2, t2, fp = k2 if k2 else (0, 0, 0.0)
    c3, t3 = k3 if k3 else (0, 0)
    caught, total = c2 + c3, t2 + t3
    det = (caught / total) if total else 0.0
    ok = det >= TARGET_DETECTION and fp <= TARGET_FP
    avlib.emit_run(
        "metric 12 (K2+K3)",
        f"detection={det:.2f} ({caught}/{total}) (порог ≥{TARGET_DETECTION}), "
        f"FP={fp:.2f} (порог ≤{TARGET_FP}){' OK' if ok else ' BELOW'}",
    )
    if not ok:
        avlib.emit_find(
            "metric 12",
            "объединённое обнаружение ниже порога либо FP выше — "
            "проверить корпус K3 и стандарт K2",
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
