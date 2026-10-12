#!/usr/bin/env python3
"""test_fp_replay.py — самотест прогонщика K4 (`fp_replay`) на синтетике.

Проверяет МЕХАНИКУ реплея без сети и без стандарта Java: строит временный
git-репозиторий, где один коммит вносит нарушение ERROR-уровня (PAN-литерал
в конфиге), и убеждается, что прогонщик:
  • на ЧИСТОЙ истории не даёт красных вердиктов (доля FP = 0);
  • видит ровно один красный вердикт и указывает именно PAN-коммит.

Это не pytest: файл самодостаточен (только stdlib). Требует `git` в PATH и
бинарь arch-be ($ARCH_BE); без них — честный SKIP (exit 0), а не «зелень».

Запуск: `python3 runners/test_fp_replay.py`; ненулевой exit — расхождение.
"""
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import avlib  # noqa: E402
import fp_replay as fpr  # noqa: E402

FAILS = []

# Синтетический стандарт: одно ERROR-правило на PAN-литерал. Намеренно НЕ
# тянем `standard/CONSTRAINTS.java-services.yaml` — самотест не должен
# зависеть от Java-стандарта и его порогов.
SYNTH_CONSTRAINTS = """version: "2026.10"
id: FPTEST
rules:
  - id: FPTEST-001
    name: no_pan_literal
    type: must_not_contain
    glob: ["**/*.yml", "**/*.yaml", "**/*.properties"]
    pattern: "\\\\b\\\\d{4}[- ]?\\\\d{4}[- ]?\\\\d{4}[- ]?\\\\d{4}\\\\b"
    severity: error
    owner: "@test"
    rationale: "PAN-литерал в конфиге недопустим (тест K4)."
"""


def check(name, got, want):
    ok = got == want
    print(f"{'ok  ' if ok else 'FAIL'} {name}: got={got!r} want={want!r}")
    if not ok:
        FAILS.append(name)


def section(t):
    print(f"\n## {t}")


def git(repo, *args):
    return subprocess.run(
        ["git", "-C", str(repo), "-c", "user.email=t@example.com",
         "-c", "user.name=t", *args],
        capture_output=True, text=True,
    )


def write(repo, rel, content):
    p = Path(repo) / rel
    p.parent.mkdir(parents=True, exist_ok=True)
    p.write_text(content, encoding="utf-8")


def build_repo(repo):
    """Чистая история из двух коммитов + третий с PAN. → sha пан-коммита."""
    repo.mkdir(parents=True, exist_ok=True)
    git(repo, "init", "-q")
    write(repo, "service/pom.xml", "<project><modelVersion>4.0.0</modelVersion></project>\n")
    write(repo, "service/config.yml", "app:\n  name: svc\n  card: none\n")
    git(repo, "add", "-A")
    git(repo, "commit", "-q", "-m", "clean 1")
    write(repo, "service/config2.yml", "app:\n  name: svc2\n")
    git(repo, "add", "-A")
    git(repo, "commit", "-q", "-m", "clean 2")
    clean_head = git(repo, "rev-parse", "HEAD").stdout.strip()
    write(repo, "service/payments.yml", "payment:\n  pan: 4111 1111 1111 1111\n")
    git(repo, "add", "-A")
    git(repo, "commit", "-q", "-m", "oops PAN")
    pan_sha = git(repo, "rev-parse", "HEAD").stdout.strip()
    return clean_head, pan_sha


def main():
    section("доступность arch-be и git")
    if shutil.which("git") is None:
        print("[SKIP] git не найден в PATH")
        return 0
    ver = avlib.run_archbe(["--version"], no_exec=False, timeout=30)
    if ver.code != 0:
        print(f"[SKIP] arch-be недоступен ({avlib.ARCH_BE}): {(ver.err or '').strip()[:120]}")
        return 0

    tmp = Path(tempfile.mkdtemp(prefix="fp_replay_test_"))
    try:
        repo = tmp / "synth"
        std = tmp / "FPTEST.yaml"
        std.write_text(SYNTH_CONSTRAINTS, encoding="utf-8")
        clean_head, pan_sha = build_repo(repo)

        section("чистая история → 0 красных")
        clean = fpr.replay_commits(repo, std, [clean_head])
        check("clean: есть результат", isinstance(clean, list), True)
        if isinstance(clean, list):
            check("clean: 1 коммит", len(clean), 1)
            check("clean: не красный", clean[0].get("red"), False)

        section("коммит с PAN → красный")
        both = fpr.replay_commits(repo, std, [clean_head, pan_sha])
        check("hist: 2 результата", len(both) if isinstance(both, list) else None, 2)
        if isinstance(both, list) and len(both) == 2:
            by_sha = {r["sha"]: r for r in both}
            check("clean-коммит зелёный", by_sha[clean_head].get("red"), False)
            check("PAN-коммит красный", by_sha[pan_sha].get("red"), True)
            rules = by_sha[pan_sha].get("rules") or []
            check("PAN ловит правило no_pan_literal", "no_pan_literal" in rules, True)

        section("агрегат: доля красных на чистой истории = 0")
        red, measured, _find = fpr.replay(repo, std, [clean_head])
        check("red", red, 0)
        check("measured", measured, 1)
    finally:
        shutil.rmtree(tmp, ignore_errors=True)

    print()
    if FAILS:
        print(f"ИТОГ: FAIL — {len(FAILS)} проверок не прошло: {', '.join(FAILS)}")
        return 1
    print("ИТОГ: ok — самотест fp_replay прошёл")
    return 0


if __name__ == "__main__":
    sys.exit(main())
