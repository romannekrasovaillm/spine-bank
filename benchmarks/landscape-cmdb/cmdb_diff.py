#!/usr/bin/env python3
"""cmdb_diff.py — сверка ландшафта с выгрузкой CMDB (AB-02, волна LANDSCAPE).

Прогоняет `arch-be landscape diff` на живом клоне PetClinic в нескольких
сценариях и проверяет, что контур выдаёт ожидаемые вердикты и находки:

  1. clean        — все 8 сервисов заведены → `passed`, 0 ошибок/предупреждений;
  2. remove-one   — один сервис убран из CMDB → `cmdb-missing` [error];
  3. wrong-owner  — владелец в модели разошёлся с CMDB → `owner-conflict` [error];
  4. orphan       — в CMDB висит активная система без модели/кода → `cmdb-orphan` [warn];
  5. retired      — та же система со `status: retired` → молчит (0 предупреждений);
  6. unverifiable — пустой ландшафт (нет источников) → вердикт «не проверяема».

Сценарии 2–5 требуют модели в ландшафте (владелец модели), поэтому сначала
черновик модели пишется из CMDB через `--model-out` (неразрушающий,
`status: inferred`). Сценарий 3 правит владельца в теле SYS-файла.

Только stdlib. Пути: `ARCH_BE` (биарь), `ARCH_VALUE_REPOS` (корень клонов,
дефолт `~/bench-repos`). Без сети: нет клона/биаря → сценарий SKIP, не FAIL.
"""
import json
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

BASE = Path(__file__).resolve().parent
FIXTURE = BASE / "fixtures" / "cmdb-petclinic.yaml"

ARCH_BE = os.environ.get("ARCH_BE", "")
DEFAULT_REPOS_DIR = os.environ.get("ARCH_VALUE_REPOS", "~/bench-repos")
PETCLINIC_CLONE = "spring-petclinic-microservices"
TIMEOUT = int(os.environ.get("LANDSCAPE_TIMEOUT", "180"))


def find_arch_be():
    """Биарь: ARCH_BE → PATH → <repo>/target/{debug,release}/arch-be."""
    if ARCH_BE:
        return ARCH_BE
    on_path = shutil.which("arch-be")
    if on_path:
        return on_path
    repo_root = BASE.parent.parent
    for profile in ("debug", "release"):
        cand = repo_root / "target" / profile / "arch-be"
        if cand.exists():
            return str(cand)
    return None


def find_petclinic():
    root = Path(os.path.expanduser(DEFAULT_REPOS_DIR))
    clone = root / PETCLINIC_CLONE
    return clone if clone.is_dir() else None


def read_fixture_records():
    """Возвращает список dict-записей из фикстуры (разбор inline-карт)."""
    records = []
    for raw in FIXTURE.read_text(encoding="utf-8").splitlines():
        line = raw.strip()
        if not line.startswith("- {") or "name:" not in line:
            continue
        body = line[line.index("{") + 1 : line.rindex("}")]
        rec = {}
        for part in body.split(","):
            if ":" not in part:
                continue
            k, v = part.split(":", 1)
            rec[k.strip()] = v.strip().strip('"')
        records.append(rec)
    return records


def write_cmdb(path, records):
    lines = ["systems:"]
    for r in records:
        inner = ", ".join(f"{k}: {v}" for k, v in r.items())
        lines.append(f"  - {{{inner}}}")
    path.write_text("\n".join(lines) + "\n", encoding="utf-8")


def run_diff(arch_be, root, cmdb, repo=None, extra=None):
    """Запускает `landscape diff --format json`; возвращает (rc, dict|None, stderr)."""
    cmd = [arch_be, "landscape", "diff", "--root", str(root),
           "--cmdb", str(cmdb), "--format", "json"]
    if repo:
        cmd += ["--repo", str(repo)]
    if extra:
        cmd += extra
    proc = subprocess.run(cmd, capture_output=True, text=True, timeout=TIMEOUT)
    report = None
    if proc.stdout.strip():
        try:
            report = json.loads(proc.stdout)
        except json.JSONDecodeError:
            report = None
    return proc.returncode, report, proc.stderr


def classes(report):
    return sorted(f["class"] for f in report.get("findings", []))


def verdict_of(report, name_substr):
    for v in report.get("verdicts", []):
        if name_substr in v.get("name", ""):
            return v.get("verdict")
    return None


# ── сценарии ─────────────────────────────────────────────────────────────────

def scenario_clean(arch_be, work, repo, records):
    cmdb = work / "cmdb.yaml"
    write_cmdb(cmdb, records)
    rc, rep, _ = run_diff(arch_be, work, cmdb, repo)
    if rep is None:
        return "SKIP", f"нет JSON (rc={rc})"
    ok = rep["counts"]["errors"] == 0 and rep["counts"]["warnings"] == 0 and rep["passed"]
    detail = f"errors={rep['counts']['errors']} warnings={rep['counts']['warnings']}"
    return ("PASS" if ok else "FAIL"), detail


def scenario_remove_one(arch_be, work, repo, records):
    cmdb = work / "cmdb-remove.yaml"
    write_cmdb(cmdb, [r for r in records if r["name"] != "visits-service"])
    rc, rep, _ = run_diff(arch_be, work, cmdb, repo)
    if rep is None:
        return "SKIP", f"нет JSON (rc={rc})"
    ok = classes(rep) == ["cmdb-missing"] and rep["counts"]["errors"] == 1
    return ("PASS" if ok else "FAIL"), f"classes={classes(rep)}"


def scenario_wrong_owner(arch_be, work, repo, records):
    # Правим владельца в теле SYS-файла customers-service.
    target = None
    for sysmd in sorted((work / "model").glob("SYS-*.md")):
        if "customers-service" in sysmd.read_text(encoding="utf-8"):
            target = sysmd
            break
    if target is None:
        return "SKIP", "SYS customers-service не найден"
    text = target.read_text(encoding="utf-8").replace("Владелец: «CRM»", "Владелец: «Legacy»")
    target.write_text(text, encoding="utf-8")
    cmdb = work / "cmdb.yaml"
    rc, rep, _ = run_diff(arch_be, work, cmdb, repo)
    target.write_text(target.read_text(encoding="utf-8").replace("Владелец: «Legacy»", "Владелец: «CRM»"),
                      encoding="utf-8")
    if rep is None:
        return "SKIP", f"нет JSON (rc={rc})"
    ok = classes(rep) == ["owner-conflict"] and rep["counts"]["errors"] == 1
    return ("PASS" if ok else "FAIL"), f"classes={classes(rep)}"


def scenario_orphan(arch_be, work, repo, records):
    cmdb = work / "cmdb-orphan.yaml"
    write_cmdb(cmdb, records + [{"id": "CMDB-101", "name": "legacy-billing",
                                 "owner": "Финансы", "status": "active"}])
    rc, rep, _ = run_diff(arch_be, work, cmdb, repo)
    if rep is None:
        return "SKIP", f"нет JSON (rc={rc})"
    ok = classes(rep) == ["cmdb-orphan"] and rep["counts"]["warnings"] == 1
    return ("PASS" if ok else "FAIL"), f"classes={classes(rep)}"


def scenario_retired(arch_be, work, repo, records):
    cmdb = work / "cmdb-retired.yaml"
    write_cmdb(cmdb, records + [{"id": "CMDB-102", "name": "old-reports",
                                 "owner": "Финансы", "status": "retired"}])
    rc, rep, _ = run_diff(arch_be, work, cmdb, repo)
    if rep is None:
        return "SKIP", f"нет JSON (rc={rc})"
    ok = classes(rep) == [] and rep["counts"]["warnings"] == 0 and rep["passed"]
    return ("PASS" if ok else "FAIL"), f"classes={classes(rep)}"


def scenario_unverifiable(arch_be, work, repo, records):
    empty = work / "empty-root"
    empty.mkdir(exist_ok=True)
    cmdb = work / "cmdb-orphan.yaml"
    rc, rep, _ = run_diff(arch_be, empty, cmdb)
    if rep is None:
        return "SKIP", f"нет JSON (rc={rc})"
    ok = classes(rep) == [] and verdict_of(rep, "legacy-billing") == "не проверяема"
    return ("PASS" if ok else "FAIL"), f"verdict={verdict_of(rep, 'legacy-billing')}"


SCENARIOS = [
    ("clean", scenario_clean),
    ("remove-one", scenario_remove_one),
    ("wrong-owner", scenario_wrong_owner),
    ("orphan", scenario_orphan),
    ("retired", scenario_retired),
    ("unverifiable", scenario_unverifiable),
]


def main(argv):
    as_json = "--json" in argv
    arch_be = find_arch_be()
    if arch_be is None:
        print("SKIP: биарь arch-be не найден (задайте ARCH_BE или соберите)", file=sys.stderr)
        print(json.dumps({"status": "skip", "reason": "no arch-be"}) if as_json else "SKIP: no arch-be")
        return 2
    repo = find_petclinic()
    records = read_fixture_records()

    tmp = Path(tempfile.mkdtemp(prefix="landscape-cmdb-"))
    try:
        # Черновик модели из фикстуры (владельцы для сверки).
        cmdb = tmp / "cmdb.yaml"
        write_cmdb(cmdb, records)
        rc, _, err = run_diff(arch_be, tmp, cmdb, repo,
                              ["--model-out", str(tmp / "model")])
        if not (tmp / "model").is_dir():
            print(f"FAIL: черновик модели не записан (rc={rc})\n{err}", file=sys.stderr)
            return 1

        results = {}
        for name, fn in SCENARIOS:
            try:
                status, detail = fn(arch_be, tmp, repo, records)
            except Exception as ex:  # noqa: BLE001 — прогонщик: не падать, а фиксировать
                status, detail = "FAIL", f"исключение: {ex}"
            results[name] = {"status": status, "detail": detail}
    finally:
        shutil.rmtree(tmp, ignore_errors=True)

    passed = sum(1 for r in results.values() if r["status"] == "PASS")
    failed = sum(1 for r in results.values() if r["status"] == "FAIL")
    skipped = sum(1 for r in results.values() if r["status"] == "SKIP")
    verdict = "pass" if failed == 0 and skipped == 0 else ("partial" if passed else "fail")

    if as_json:
        print(json.dumps({"status": verdict, "passed": passed, "failed": failed,
                          "skipped": skipped, "scenarios": results},
                         ensure_ascii=False, indent=2))
    else:
        for name, r in results.items():
            print(f"  [{r['status']:4}] {name:14} {r['detail']}")
        print(f"итог: {verdict} — pass={passed} fail={failed} skip={skipped}")
    return 0 if verdict == "pass" else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
