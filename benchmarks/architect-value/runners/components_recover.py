#!/usr/bin/env python3
"""components_recover.py — метрика 2 (§9): F1 имён компонентов графа «как построено».

Волна R, извлекатели R1/R4. Для одномодульных монолитов (единственный модуль
сборки) компонент «по факту» разбирается на кластеры ПАКЕТОВ (R4): заголовок
узла — точечный путь пакета относительно базового. Метрика берёт замороженный
эталон `ground_truth/<repo>.yaml` (уровень `package`/`contexts`), восстанавливает
граф на пиннутом SHA (`repos.lock`) и сравнивает МНОЖЕСТВА имён компонентов,
считая F1.

Граф одного снимка получается тем же приёмом, что в `edges_recover.py`: в клоне
строятся два отсоединённых (dangling) коммита — пустое дерево (база) и дерево
целевого SHA (голова) — через `git commit-tree`, не трогая refs и рабочее
дерево. Тогда `arch-diff base..head` даёт `added_nodes` = все узлы головы,
`added_edges` = все рёбра (не нужны здесь). Имя компонента — поле `title`.

Порог §9: F1 множества имён компонентов ≥ 0.80 на уровне пакетов/контекстов
(монолиты buckpal, library). Клонов нет / SHA недоступен / нет эталона → SKIP
(не «зелень»). Прогонщик измеряет и печатает факты, exit 0; ненулевой exit —
только если `arch-be` не запускается.
"""
import json
import os
import subprocess
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import avlib  # noqa: E402

# Метрика 2 — монолиты (уровень пакетов/контекстов): одномодульная сборка.
REPOS = ("buckpal", "library")

# Минимальный F1 имён компонентов (§9).
MIN_F1 = 0.80


def _git_env():
    env = dict(os.environ)
    env.setdefault("GIT_AUTHOR_NAME", "arch-value")
    env.setdefault("GIT_AUTHOR_EMAIL", "arch-value@localhost")
    env.setdefault("GIT_COMMITTER_NAME", "arch-value")
    env.setdefault("GIT_COMMITTER_EMAIL", "arch-value@localhost")
    return env


def git(clone, *args, input_text=None):
    p = subprocess.run(
        ["git", "-C", str(clone), *args],
        input=input_text, capture_output=True, text=True, env=_git_env(),
    )
    return p.returncode, p.stdout.strip(), p.stderr.strip()


def sha_available(clone, sha):
    code, _, _ = git(clone, "cat-file", "-e", f"{sha}^{{commit}}")
    return code == 0


def make_snapshot_commits(clone, sha):
    """Пара dangling-коммитов (base=пустое дерево, head=дерево sha)."""
    code, empty_tree, err = git(clone, "mktree", input_text="")
    if code != 0 or not empty_tree:
        raise RuntimeError(f"git mktree: {err[:160]}")
    code, base, err = git(clone, "commit-tree", empty_tree, "-m", "metric2-base")
    if code != 0 or not base:
        raise RuntimeError(f"git commit-tree (base): {err[:160]}")
    code, head_tree, err = git(clone, "rev-parse", f"{sha}^{{tree}}")
    if code != 0 or not head_tree:
        raise RuntimeError(f"git rev-parse {sha}^{{tree}}: {err[:160]}")
    code, head, err = git(clone, "commit-tree", head_tree, "-p", base, "-m", "metric2-head")
    if code != 0 or not head:
        raise RuntimeError(f"git commit-tree (head): {err[:160]}")
    return base, head


def component_names(added_nodes):
    """Имена узлов-компонентов (kind == component) → множество."""
    out = set()
    for n in added_nodes or []:
        if n.get("kind") != "component":
            continue
        nid = n.get("id") or ""
        out.add(n.get("title") or nid)
    return out


def gt_component_names(gt):
    """Множество имён компонентов эталона (секция `components`)."""
    out = set()
    for c in gt.get("components") or []:
        if isinstance(c, dict) and c.get("name") is not None:
            out.add(c["name"])
    return out


def measure_repo(repo, repos_root):
    gt_path = avlib.GROUND_TRUTH / f"{repo}.yaml"
    if not gt_path.is_file():
        avlib.emit_skip(f"metric 2 [{repo}]", f"нет эталона {gt_path}")
        return False
    gt = avlib.load_yaml(gt_path) or {}
    clone = avlib.repo_clone_dir(repo, repos_root)
    if clone is None:
        avlib.emit_skip(f"metric 2 [{repo}]", "клон отсутствует (--repos-dir/ARCH_VALUE_REPOS)")
        return False
    sha = gt.get("sha")
    if not sha:
        avlib.emit_skip(f"metric 2 [{repo}]", "в эталоне не указан sha")
        return False
    if not sha_available(clone, sha):
        avlib.emit_skip(f"metric 2 [{repo}]", f"SHA {sha[:9]} недоступен в клоне (нужен git fetch)")
        return False
    try:
        base, head = make_snapshot_commits(clone, sha)
    except RuntimeError as exc:
        avlib.emit_skip(f"metric 2 [{repo}]", str(exc))
        return False
    r = avlib.run_archbe(
        ["arch-diff", "--repo", str(clone), "--base", base, "--head", head, "--format", "json"],
        cwd=clone,
    )
    if not r.ok:
        avlib.emit_skip(f"metric 2 [{repo}]", f"arch-diff → код {r.code}: {(r.err or '').strip()[:160]}")
        return False
    try:
        diff = json.loads(r.out)
    except ValueError as exc:
        avlib.emit_skip(f"metric 2 [{repo}]", f"arch-diff JSON не распознан: {exc}")
        return False

    det = component_names(diff.get("added_nodes"))
    exp = gt_component_names(gt)
    tp = det & exp
    precision = len(tp) / len(det) if det else 1.0
    recall = len(tp) / len(exp) if exp else 1.0
    f1 = (2 * precision * recall / (precision + recall)) if (precision + recall) else 0.0

    ok = f1 >= MIN_F1
    avlib.emit_run(
        f"metric 2 [{repo}]",
        f"F1 {f1:.3f} (P {precision:.2f}, R {recall:.2f}; TP={len(tp)} "
        f"P={len(det)} E={len(exp)})  {'OK' if ok else 'BELOW'} (порог F1 ≥{MIN_F1:.2f})",
    )
    for name in sorted(det - exp):
        avlib.emit_run(f"metric 2 [{repo}] лишнее", name)
    for name in sorted(exp - det):
        avlib.emit_run(f"metric 2 [{repo}] пропущено", name)
    return f1


def main(repos_root=None):
    avlib.emit_section("ArchRecover — метрика 2 (имена компонентов: F1, кластеры пакетов)")
    measured = 0
    for repo in REPOS:
        if measure_repo(repo, repos_root) is not False:
            measured += 1
    if measured == 0:
        avlib.emit_skip("metric 2", "ни одного клона/эталона — измерить нечего")
    return 0


if __name__ == "__main__":
    root = None
    args = sys.argv[1:]
    if args and args[0] == "--repos-dir" and len(args) > 1:
        root = args[1]
    sys.exit(main(root))
