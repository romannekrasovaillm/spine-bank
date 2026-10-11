#!/usr/bin/env python3
"""recover_determinism.py — метрики 18, 19 (§9): детерминизм и время `recover`.

Волна R, R5 (ADR-069). Команда `arch-be recover <repo> --rev <sha> --format json`
выдаёт полный граф «как построено» одной ревизии (без диффа и merge-base).

Метрика 18 — детерминизм: два прогона на одном SHA дают БАЙТ-В-БАЙТ одинаковый
JSON (схема `arch-be/recover/v1`). Проба сравнивает stdout двух запусков и печатает
sha256 от него — расхождение = дефект (правило 10).

Метрика 19 — время: полный прогон `recover` укладывается в ≤ 10 с на неподвижном
SHA (потолок §9: ≤ 5 тыс. файлов). Берётся wall-clock первого полного прогона.

Клонов нет / SHA недоступен → SKIP (не «зелень»). Прогонщик измеряет и печатает
факты, exit 0; ненулевой exit — только если `arch-be` не запускается.
"""
import hashlib
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import avlib  # noqa: E402

REPOS = ("boutique", "petclinic", "buckpal", "library")
TIME_LIMIT_S = 10.0


def _sha9(sha):
    return sha[:9] if sha else "?"


def measure_repo(repo, repos_root):
    gt_path = avlib.GROUND_TRUTH / f"{repo}.yaml"
    gt = (avlib.load_yaml(gt_path) or {}) if gt_path.is_file() else {}
    clone = avlib.repo_clone_dir(repo, repos_root)
    if clone is None:
        avlib.emit_skip(f"metric 18/19 [{repo}]", "клон отсутствует (--repos-dir/ARCH_VALUE_REPOS)")
        return False
    rev = gt.get("sha") or "HEAD"
    args = ["recover", str(clone), "--rev", rev, "--format", "json"]

    t0 = time.monotonic()
    r1 = avlib.run_archbe(args, cwd=clone)
    dt = time.monotonic() - t0
    if not r1.ok:
        avlib.emit_skip(
            f"metric 18/19 [{repo}]",
            f"recover → код {r1.code}: {(r1.err or '').strip()[:160]}",
        )
        return False
    r2 = avlib.run_archbe(args, cwd=clone)
    if not r2.ok:
        avlib.emit_skip(f"metric 18 [{repo}]", f"второй прогон → код {r2.code}")
        return False

    same = r1.out == r2.out
    digest = hashlib.sha256(r1.out.encode("utf-8")).hexdigest()[:12]
    schema_ok = '"schema"' in r1.out and "arch-be/recover/v1" in r1.out
    avlib.emit_run(
        f"metric 18 [{repo}]",
        f"rev {_sha9(rev)} байт-в-байт={'да' if same else 'НЕТ'} "
        f"sha256={digest} schema={'ok' if schema_ok else 'НЕТ'}",
    )
    ok = dt <= TIME_LIMIT_S
    avlib.emit_run(
        f"metric 19 [{repo}]",
        f"rev {_sha9(rev)} время {dt:.2f}s {'≤' if ok else '>'} {TIME_LIMIT_S:.0f}s "
        f"{'OK' if ok else 'BELOW'}",
    )
    return True


def main(repos_root=None):
    avlib.emit_section("ArchRecover — метрики 18, 19 (детерминизм и время recover)")
    measured = 0
    for repo in REPOS:
        if measure_repo(repo, repos_root):
            measured += 1
    if measured == 0:
        avlib.emit_skip("metric 18/19", "ни одного клона/эталона — измерить нечего")
    return 0


if __name__ == "__main__":
    root = None
    args = sys.argv[1:]
    if args and args[0] == "--repos-dir" and len(args) > 1:
        root = args[1]
    sys.exit(main(root))
