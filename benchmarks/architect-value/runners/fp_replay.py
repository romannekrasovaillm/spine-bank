#!/usr/bin/env python3
"""fp_replay.py — метрика K4 (§7): ложные срабатывания на чистой истории.

Реплей гейта по последним коммитам закреплённого репо со стандартом
приложения C: для каждого коммита разворачиваем ДЕРЕВО его ревизии
(`git archive <sha> | tar -x -C <tmp>`) и прогоняем

    arch-be control check <дерево> --constraints <стандарт> --json

Красный вердикт = `control check` вернул код 1 (есть находки error-уровня;
warn-находки гейт не роняют). Порог §7: доля красных вердиктов ≤ 3 %.

Почему доля красных = доля ложных срабатываний: репо закреплён на
«эталонной» ревизии, заведомо ЧИСТОЙ по стандарту приложения C (сервис на
другой раскладке пакетов → корп-правила слоёв уходят в N/A, log4j-core
ниже порога отсутствует). Любой красный вердикт на такой истории —
ложное срабатывание правила (дефект детектора), а не реальное нарушение.
Честность: нет клона / нет бинаря — SKIP с причиной, а не «зелень».

Transport: stdlib (subprocess+json+tarfile). Прогон без сети; `ARCH_NO_EXEC=1`.
Exit 0 (измерение).
"""
import io
import json
import os
import subprocess
import sys
import tarfile
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import avlib  # noqa: E402

REPO = "petclinic"
STD_NAME = "CONSTRAINTS.java-services.yaml"
# Последние N first-parent коммитов (§7 упоминает 100; прогонщик бенча —
# 20 по умолчанию, переопределяется ARCH_VALUE_K4_COMMITS).
REPLAY_COMMITS = int(os.environ.get("ARCH_VALUE_K4_COMMITS", "20"))
FP_TARGET = 0.03


def head_commits(clone, n):
    """Последние n first-parent коммитов (sha, новейший первым). → (shas, err)."""
    r = subprocess.run(
        ["git", "-C", str(clone), "rev-list", "--first-parent", f"-n{n}", "HEAD"],
        capture_output=True, text=True,
    )
    if r.returncode != 0:
        return None, f"git rev-list → код {r.returncode}: {(r.stderr or '').strip()[:160]}"
    return [s for s in r.stdout.split() if s], None


def materialize(clone, sha, dest):
    """Развернуть дерево ревизии sha в каталог dest. → err|None."""
    r = subprocess.run(
        ["git", "-C", str(clone), "archive", "--format=tar", sha],
        capture_output=True,
    )
    if r.returncode != 0:
        msg = r.stderr.decode("utf-8", "replace").strip()[:160]
        return f"git archive {sha[:9]} → код {r.returncode}: {msg}"
    try:
        with tarfile.open(fileobj=io.BytesIO(r.stdout)) as tf:
            try:
                tf.extractall(dest, filter="data")  # py>=3.11.4: безопасный фильтр
            except TypeError:
                tf.extractall(dest)
    except tarfile.TarError as e:
        return f"tar {sha[:9]}: {e}"
    return None


def control_check(tree, standard):
    """→ (doc|None, rc, err). Код 0|1 — норма (1 = есть error-находки)."""
    r = avlib.run_archbe(
        ["control", "check", str(tree), "--constraints", str(standard), "--json"],
        cwd=str(tree),
    )
    if r.code not in (0, 1):
        if r.code == 127:
            return None, r.code, f"arch-be не найден: {avlib.ARCH_BE}"
        return None, r.code, f"control check → код {r.code}: {(r.err or '').strip()[:160]}"
    try:
        return json.loads(r.out), r.code, None
    except json.JSONDecodeError as e:
        return None, r.code, f"control check: не JSON ({e})"


def is_red(doc, rc):
    """Красный = гейт упал (код 1) либо есть находки error-уровня в отчёте."""
    if rc == 1:
        return True
    if isinstance(doc, dict):
        return any(i.get("severity") == "error" for i in doc.get("issues", []))
    return False


def replay_commits(clone, standard, shas):
    """Прогон control check по дереву каждого sha. → [{sha, red, rules, error}]."""
    out = []
    for sha in shas:
        with tempfile.TemporaryDirectory(prefix="fp_replay_") as td:
            err = materialize(clone, sha, td)
            if err:
                out.append({"sha": sha, "red": None, "rules": [], "error": err})
                continue
            doc, rc, cerr = control_check(Path(td), standard)
            if cerr:
                out.append({"sha": sha, "red": None, "rules": [], "error": cerr})
                continue
            rules = sorted(
                {
                    i.get("rule", "")
                    for i in (doc or {}).get("issues", [])
                    if i.get("severity") == "error"
                }
            )
            out.append({"sha": sha, "red": is_red(doc, rc), "rules": rules})
    return out


def replay(clone, standard, shas):
    """Агрегат по списку sha. → (red, measured, findings).

    Неизмеренные коммиты (ошибка разворачивания/прогона) в знаменатель не
    попадают — знаменатель честный, а не разбавленный нулями."""
    res = replay_commits(clone, standard, shas)
    red, measured, findings = 0, 0, []
    for r in res:
        if r.get("red") is None:
            continue
        measured += 1
        if r["red"]:
            red += 1
            findings.append(r)
    return red, measured, findings


def measure_k4(repos_root, emit=True, n=None):
    """Измерить K4 на пинутом репо. → (red, measured, share) | None (SKIP).

    `emit` управляет печатью [RUN]/[SKIP]/[FIND]; kontur_bench зовёт с
    emit=False, чтобы посчитать строку metric 12, не дублируя вердикт."""
    n = n or REPLAY_COMMITS
    standard = avlib.STANDARD / STD_NAME
    if not standard.is_file():
        if emit:
            avlib.emit_skip("metric K4", f"нет стандарта {standard}")
        return None
    clone = avlib.repo_clone_dir(REPO, repos_root)
    if clone is None:
        if emit:
            avlib.emit_skip("metric K4", f"клон {REPO} отсутствует (--repos-dir)")
        return None
    shas, err = head_commits(clone, n)
    if err:
        if emit:
            avlib.emit_skip("metric K4", err)
        return None
    if not shas:
        if emit:
            avlib.emit_skip("metric K4", "в клоне нет коммитов")
        return None
    res = replay_commits(clone, standard, shas)
    measured = sum(1 for r in res if r.get("red") is not None)
    if measured == 0:
        if emit:
            first = res[0].get("error") if res else "нет коммитов"
            avlib.emit_skip("metric K4", f"ни один коммит не измерен: {first}")
        return None
    red = sum(1 for r in res if r.get("red"))
    share = red / measured
    ok = share <= FP_TARGET
    if emit:
        avlib.emit_run(
            "metric K4 (FP на чистой истории)",
            f"{REPO}, последние {measured} first-parent коммитов: "
            f"красных {red}/{measured}={share:.2f} (порог ≤{FP_TARGET})"
            f"{' OK' if ok else ' ABOVE'}",
        )
        for r in res:
            if r.get("red"):
                avlib.emit_find(
                    "metric K4",
                    f"{r['sha'][:9]} красный на чистой истории: "
                    f"{','.join(r.get('rules') or []) or '?'}",
                )
    return red, measured, share


def main(repos_root=None):
    avlib.emit_section("Kontur — FP на чистой истории (метрика K4)")
    measure_k4(repos_root, emit=True)
    return 0


if __name__ == "__main__":
    root = None
    args = sys.argv[1:]
    if args and args[0] == "--repos-dir" and len(args) > 1:
        root = args[1]
    sys.exit(main(root))
