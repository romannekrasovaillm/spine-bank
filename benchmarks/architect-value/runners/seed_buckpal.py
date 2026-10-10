#!/usr/bin/env python3
"""seed_buckpal.py — метрика 8 (§9) и сьют C.3: засевы дефектов в buckpal.

C.2 (метрика 8). 12 засевов `seeds/buckpal_c2.yaml`. Для каждого — свежая копия
чистого клона buckpal (pinned SHA из `repos.lock`), `insert` дописывается в конец
`target`, затем
  arch-be control check <копия> --constraints standard/CONSTRAINTS.java-services.yaml --json
Исход: `caught` — правило-владелец (`owner_rule`) есть в находках; `missed` — нет.
Контрольный засев (ожидание `clean`) — `caught` означает ЛОЖНОЕ срабатывание.
Верным считается исход, равный `expected_target` (`caught` у дефекта, `clean` у
контроля). Метрика 8 = верных/12. Порог §9: 0.3.16 baseline 7/12 (slash-нотация)
/ 2/12 (Java-нотация), цель 0.4.0 — ≥ 11/12. Расхождение факта с
`expected_baseline` печатается как [FIND], но порог §9 не двигает.

C.3. Те же 6 обходов `seeds/archunit_c3.yaml`. Половина `text_detection`
измеряется тем же текстовым `control check` (правило s6). Половина
`archunit_detection` (правило `type: archunit`, байткод, ADR-039) на 0.3.17 без
ArchUnit-jar (`archunit fetch` требует сеть) не измеряется → честный SKIP.

Transport: stdlib (subprocess+json+shutil). Прогон без сети; `ARCH_NO_EXEC=1`.
Exit 0 (измерение); отсутствие клона/бинаря — SKIP, не падение.
"""
import json
import shutil
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import avlib  # noqa: E402

C2 = avlib.SEEDS / "buckpal_c2.yaml"
C3 = avlib.SEEDS / "archunit_c3.yaml"
_STD_REL = "standard/CONSTRAINTS.java-services.yaml"


def _constraints_for(spec):
    """`standard:` из спек-файла → абсолютный путь к CONSTRAINTS.yaml."""
    rel = spec.get("standard") or _STD_REL
    return avlib.BASE / rel


def _prep_copy(clone, target_rel, insert):
    """Свежая копия клона (без .git) c дописанным `insert` в конец target."""
    dst = Path(tempfile.mkdtemp(prefix="seed_")) / "buckpal"
    shutil.copytree(clone, dst, ignore=shutil.ignore_patterns(".git"))
    tgt = dst / target_rel
    if not tgt.is_file():
        return None, dst, f"нет файла-цели {target_rel} в клоне"
    with tgt.open("a", encoding="utf-8") as f:
        f.write("\n" + insert + "\n")
    return dst, dst, None


def _copy_outcome(dst, constraints):
    r = avlib.run_archbe(
        ["control", "check", str(dst), "--constraints", str(constraints), "--json"],
    )
    if r.code not in (0, 1):
        return None, f"control check → код {r.code}: {(r.err or '').strip()[:140]}"
    try:
        return json.loads(r.out), None
    except json.JSONDecodeError as e:
        return None, f"control check: не JSON ({e})"


def _owner_hits(doc, owner_rule):
    return [i for i in doc.get("issues", []) if i.get("rule") == owner_rule]


def _outcome(hits, expect):
    """caught/missed/clean по находкам владельца и ожиданию спек-файла."""
    if expect == "clean":
        return "caught" if hits else "clean"  # caught при clean = ложное срабатывание
    return "caught" if hits else "missed"


def _run_seeds(suite, clone, specs, constraints, label_correct, label_total):
    """Прогон набора засевов; → (correct, measured, facts)."""
    correct, measured = 0, 0
    for s in specs:
        sid = s.get("id")
        target = s.get("target")
        insert = s.get("insert", "")
        owner = s.get("owner_rule")
        expect_t = s.get("expected_target")
        expect_b = s.get("expected_baseline")
        dst, root, err = _prep_copy(clone, target, insert)
        if err:
            avlib.emit_skip(f"metric 8 {suite} [{sid}]", err)
            shutil.rmtree(root.parent, ignore_errors=True)
            continue
        doc, cerr = _copy_outcome(dst, constraints)
        shutil.rmtree(root.parent, ignore_errors=True)
        if cerr:
            avlib.emit_skip(f"metric 8 {suite} [{sid}]", cerr)
            continue
        measured += 1
        hits = _owner_hits(doc, owner)
        fact = _outcome(hits, expect_b)
        ok = fact == expect_t
        correct += 1 if ok else 0
        detail = (
            f"{'✓' if ok else '✗'} факт={fact} ожид={expect_t} "
            f"(baseline 0.3.16={expect_b}) владелец={owner}"
        )
        avlib.emit_run(f"metric 8 {suite} [{sid}]", detail)
        if fact != expect_b:
            print(f"[FIND] metric 8 {suite} [{sid}]: факт={fact} ≠ baseline 0.3.16={expect_b}")
    return correct, measured


def metric8(clone):
    spec = avlib.load_yaml(C2)
    if not isinstance(spec, dict):
        avlib.emit_skip("metric 8 C.2", f"не разобран {C2}")
        return
    if clone is None:
        avlib.emit_skip("metric 8 C.2", "клон buckpal отсутствует (--repos-dir/ARCH_VALUE_REPOS)")
        return
    constraints = _constraints_for(spec)
    seeds = spec.get("seeds", []) or []
    correct, measured = _run_seeds("C.2", clone, seeds, constraints, None, None)
    if measured == 0:
        avlib.emit_skip("metric 8 C.2", "ни один засев не измерен")
        return
    avlib.emit_run(
        "metric 8",
        f"correct={correct}/{measured} "
        f"(baseline 0.3.16: 7/12 slash, 2/12 Java; target 0.4.0 ≥11/12)",
    )


def c3_text(clone):
    """C.3 text_detection (текстовое правило s6) — измеряемая половина."""
    spec = avlib.load_yaml(C3)
    if not isinstance(spec, dict):
        avlib.emit_skip("C.3 text", f"не разобран {C3}")
        return
    if clone is None:
        avlib.emit_skip("C.3 text", "клон buckpal отсутствует")
        return
    constraints = _constraints_for(spec)
    bypasses = spec.get("bypasses", []) or []
    correct, measured = 0, 0
    for s in bypasses:
        sid = s.get("id")
        expect = s.get("text_detection")
        dst, root, err = _prep_copy(clone, s.get("target"), s.get("insert", ""))
        if err:
            avlib.emit_skip(f"C.3 text [{sid}]", err)
            shutil.rmtree(root.parent, ignore_errors=True)
            continue
        doc, cerr = _copy_outcome(dst, constraints)
        shutil.rmtree(root.parent, ignore_errors=True)
        if cerr:
            avlib.emit_skip(f"C.3 text [{sid}]", cerr)
            continue
        measured += 1
        fact = _outcome(_owner_hits(doc, s.get("owner_rule")), expect)
        ok = fact == expect
        correct += 1 if ok else 0
        avlib.emit_run(
            f"C.3 text [{sid}]",
            f"{'✓' if ok else '✗'} факт={fact} ожид(text)={expect} "
            f"(archunit={s.get('archunit_detection')}) — {s.get('title', '')[:36]}",
        )
    if measured == 0:
        avlib.emit_skip("C.3 text", "ни один обход не измерен")
        return
    avlib.emit_run("C.3 text_detection", f"correct={correct}/{measured} (текстовое правило s6)")


def c3_archunit():
    """C.3 archunit_detection — SKIP: нет jar'ов ArchUnit, сеть выключена."""
    avlib.emit_skip(
        "C.3 archunit_detection",
        "нужен ArchUnit-jar (`archunit fetch`, требует сеть); ARCHUNIT_JAR не задан → "
        "байткод-детектор (type: archunit, ADR-039) не измерен",
    )


def main(repos_root=None):
    avlib.emit_section("Засевы buckpal — метрика 8 (C.2) + C.3")
    clone = avlib.repo_clone_dir("buckpal", repos_root)
    metric8(clone)
    c3_text(clone)
    c3_archunit()
    return 0


if __name__ == "__main__":
    root = None
    args = sys.argv[1:]
    if args and args[0] == "--repos-dir" and len(args) > 1:
        root = args[1]
    sys.exit(main(root))
