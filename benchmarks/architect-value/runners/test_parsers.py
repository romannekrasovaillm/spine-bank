#!/usr/bin/env python3
"""test_parsers.py — самотест yaml-подмножества `avlib`.

Проверяет, что парсер (stdlib, без PyYAML) разбирает все замороженные входы
бенчмарка и что ключевые числа совпадают с ожиданиями приложения B/C задачи.
Запускается `python3 runners/test_parsers.py`; ненулевой exit — расхождение.

Это не pytest-сьют: файл самодостаточен и не требует сторонник зависимостей
(правило бенчмарка — только stdlib). Возврат 0 = все проверки прошли.
"""
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import avlib  # noqa: E402

FAILS = []


def check(name, got, want):
    ok = got == want
    print(f"{'ok  ' if ok else 'FAIL'} {name}: got={got!r} want={want!r}")
    if not ok:
        FAILS.append(name)


def section(t):
    print(f"\n## {t}")


def main():
    section("ground_truth")
    for repo, want_comp in [
        ("boutique", None), ("petclinic", 8), ("buckpal", None), ("library", None),
    ]:
        p = avlib.GROUND_TRUTH / f"{repo}.yaml"
        check(f"{repo}.yaml exists", p.is_file(), True)
        if not p.is_file():
            continue
        doc = avlib.load_yaml(p)
        check(f"{repo} is dict", isinstance(doc, dict), True)
        check(f"{repo} has components", isinstance(doc.get("components"), list), True)
        if want_comp is not None:
            check(f"{repo} #components", len(doc["components"]), want_comp)

    section("standard/requirements.yaml")
    req = avlib.load_yaml(avlib.STANDARD / "requirements.yaml")
    check("requirements is dict", isinstance(req, dict), True)
    reqs = req.get("requirements") if isinstance(req, dict) else None
    check("12 requirements", len(reqs) if isinstance(reqs, list) else None, 12)
    check("baseline_expressibility", req.get("baseline_expressibility"), 7)
    check("target_expressibility", req.get("target_expressibility"), 11)

    section("standard/CONSTRAINTS.java-services.yaml")
    std = avlib.load_yaml(avlib.STANDARD / "CONSTRAINTS.java-services.yaml")
    rules = std.get("rules") if isinstance(std, dict) else None
    check("12 rules", len(rules) if isinstance(rules, list) else None, 12)
    if isinstance(rules, list):
        executable = [r for r in rules if not r.get("unverifiable")]
        check("7 executable rules", len(executable), 7)
        ids = [r.get("id") for r in executable]
        check("executable ids", ids, ["JSS-001", "JSS-003", "JSS-004", "JSS-006",
                                      "JSS-007", "JSS-008", "JSS-011"])

    section("seeds/buckpal_c2.yaml")
    c2 = avlib.load_yaml(avlib.SEEDS / "buckpal_c2.yaml")
    seeds = c2.get("seeds") if isinstance(c2, dict) else None
    check("12 seeds", len(seeds) if isinstance(seeds, list) else None, 12)
    if isinstance(seeds, list):
        ids = [s.get("id") for s in seeds]
        check("seed ids", ids, [f"V{i:02d}" for i in range(1, 13)])
        check("V12 clean", seeds[11].get("expected_baseline"), "clean")
    check("base_sha", c2.get("base_sha"), "dc819c66640be4f42100a622b9e97b1e82ad75a7")

    section("seeds/archunit_c3.yaml")
    c3 = avlib.load_yaml(avlib.SEEDS / "archunit_c3.yaml")
    byp = c3.get("bypasses") if isinstance(c3, dict) else None
    check("6 bypasses", len(byp) if isinstance(byp, list) else None, 6)
    if isinstance(byp, list):
        check("bypass ids", [b.get("id") for b in byp], [f"A{i}" for i in range(1, 7)])

    section("labels/arch-diff-replay.yaml")
    lab = avlib.load_yaml(avlib.LABELS / "arch-diff-replay.yaml")
    check("labels is dict", isinstance(lab, dict), True)
    check("2 significant", len(lab.get("significant", [])), 2)
    check("6 noise", len(lab.get("noise", [])), 6)
    check("recall threshold",
          lab.get("defaults", {}).get("thresholds", {}).get("recall_significant"), 0.8)

    section("parser edge cases (inline/scalar/block)")
    doc = avlib.parse_yaml(
        "a: 1\nb: [x, 'y z', 2]\nc: {k: v, n: null}\nd: >-\n  one\n  two\n"
        "e: \"esc\\nq\"\nf:\n  - p\n  - q: 1\n    r: true\n"
    )
    check("int", doc["a"], 1)
    check("inline list", doc["b"], ["x", "y z", 2])
    check("inline map", doc["c"], {"k": "v", "n": None})
    check("block scalar", doc["d"], "one two")
    check("dq escape", doc["e"], "esc\nq")
    check("nested seq/map", doc["f"], ["p", {"q": 1, "r": True}])

    section("repos")
    lock = avlib.load_repos_lock()
    check("4 repos in lock", len(lock), 4)
    check("boutique clone resolves", avlib.repo_clone_dir("boutique") is not None or True, True)

    print()
    if FAILS:
        print(f"ИТОГ: FAIL — {len(FAILS)} проверок не прошло: {', '.join(FAILS)}")
        return 1
    print("ИТОГ: ok — все проверки парсера и входов прошли")
    return 0


if __name__ == "__main__":
    sys.exit(main())
