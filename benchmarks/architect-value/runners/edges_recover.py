#!/usr/bin/env python3
"""edges_recover.py — метрика 3 (§9): полнота/точность рёбер графа «как построено».

Волна R, извлекатели R2. Берёт замороженный эталон `ground_truth/<repo>.yaml`
(список рёбер) и восстановленный граф на пиннутом SHA (`repos.lock` → поле
`sha:` эталона), сравнивает множества НАПРАВЛЕННЫХ пар `(from, to)` рёбер вида
`calls`; считает recall и precision.

Как получить ВЕСЬ граф одного снимка через `arch-diff` (у которого нет CLI-дампа
графа напрямую): в клоне строятся ДВА отсоединённых (dangling) коммита —
пустое дерево (база) и дерево целевого SHA (голова) — через `git commit-tree`,
НЕ трогая refs и рабочее дерево. Тогда `arch-diff base..head` даёт
`added_edges` = все рёбра головы, `added_nodes` = все узлы (с `title` для
перевода id→имя). Так metric-3 не зависит от разницы окон и не требует
отдельного CLI.

Отображение id→имя для сверки с эталоном (у эталона — имена, у графа — id):
  component      → `title` (имя деплойной единицы/компонента);
  datastore      → часть после `store:<scheme>://`;
  topic          → часть после `topic:`.
Рёбра `import`/`connect`/`contract` в метрику 3 не входят (только `calls`).

Порог §9: boutique 17/17 рёбер, precision ≥ 0.90; petclinic 6/6 бизнес-рёбер.
Клонов нет / SHA недоступен / нет эталона → SKIP (не «зелень»). Прогонщик
измеряет и печатает факты, exit 0; ненулевой exit — только если `arch-be`
не запускается.
"""
import json
import os
import subprocess
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import avlib  # noqa: E402

REPOS = ("boutique", "petclinic")

# repo → (ожидаемое число рёбер эталона, минимальная precision).
# petclinic: 6 бизнес-рёбер эталона (инфра — отдельная секция `infra_edges`).
THRESHOLDS = {
    "boutique": (17, 0.90),
    "petclinic": (6, 0.90),
}


def _git_env():
    env = dict(os.environ)
    env.setdefault("GIT_AUTHOR_NAME", "arch-value")
    env.setdefault("GIT_AUTHOR_EMAIL", "arch-value@localhost")
    env.setdefault("GIT_COMMITTER_NAME", "arch-value")
    env.setdefault("GIT_COMMITTER_EMAIL", "arch-value@localhost")
    return env


def git(clone, *args, input_text=None):
    """git -C <clone> <args>; возвращает (код, stdout, stderr) как строки."""
    p = subprocess.run(
        ["git", "-C", str(clone), *args],
        input=input_text, capture_output=True, text=True, env=_git_env(),
    )
    return p.returncode, p.stdout.strip(), p.stderr.strip()


def sha_available(clone, sha):
    code, _, _ = git(clone, "cat-file", "-e", f"{sha}^{{commit}}")
    return code == 0


def make_snapshot_commits(clone, sha):
    """Пара dangling-коммитов (base=пустое дерево, head=дерево sha).

    Не трогает refs/рабочее дерево; коммиты станут недостижимыми и будут
    вычищены `git gc`, но для одного прогона arch-diff их достаточно.
    """
    code, empty_tree, err = git(clone, "mktree", input_text="")
    if code != 0 or not empty_tree:
        raise RuntimeError(f"git mktree: {err[:160]}")
    code, base, err = git(clone, "commit-tree", empty_tree, "-m", "metric3-base")
    if code != 0 or not base:
        raise RuntimeError(f"git commit-tree (base): {err[:160]}")
    code, head_tree, err = git(clone, "rev-parse", f"{sha}^{{tree}}")
    if code != 0 or not head_tree:
        raise RuntimeError(f"git rev-parse {sha}^{{tree}}: {err[:160]}")
    code, head, err = git(clone, "commit-tree", head_tree, "-p", base, "-m", "metric3-head")
    if code != 0 or not head:
        raise RuntimeError(f"git commit-tree (head): {err[:160]}")
    return base, head


def node_name(node):
    """Имя узла для сверки с эталоном (перевод стабильного id → имя)."""
    kind = node.get("kind")
    nid = node.get("id") or ""
    if kind == "component":
        return node.get("title") or nid
    if kind == "datastore" and nid.startswith("store:"):
        rest = nid[len("store:"):]
        return rest.split("://", 1)[1] if "://" in rest else rest
    if kind == "topic" and nid.startswith("topic:"):
        return nid[len("topic:"):]
    return node.get("title") or nid


def node_names(added_nodes):
    out = {}
    for n in added_nodes or []:
        nid = n.get("id")
        if nid is not None:
            out[nid] = node_name(n)
    return out


def detected_pairs(diff):
    """Множество направленных пар `(from, to)` рёбер вида `calls`."""
    names = node_names(diff.get("added_nodes"))
    pairs = set()
    for e in diff.get("added_edges") or []:
        if e.get("kind") != "calls":
            continue
        src = names.get(e.get("from"), e.get("from"))
        dst = names.get(e.get("to"), e.get("to"))
        pairs.add((src, dst))
    return pairs


def gt_pairs(gt):
    """Множество направленных пар эталона (секция `edges`)."""
    pairs = set()
    for e in gt.get("edges") or []:
        pairs.add((e.get("from"), e.get("to")))
    return pairs


def measure_repo(repo, repos_root):
    gt_path = avlib.GROUND_TRUTH / f"{repo}.yaml"
    if not gt_path.is_file():
        avlib.emit_skip(f"metric 3 [{repo}]", f"нет эталона {gt_path}")
        return False
    gt = avlib.load_yaml(gt_path) or {}
    clone = avlib.repo_clone_dir(repo, repos_root)
    if clone is None:
        avlib.emit_skip(f"metric 3 [{repo}]", "клон отсутствует (--repos-dir/ARCH_VALUE_REPOS)")
        return False
    sha = gt.get("sha")
    if not sha:
        avlib.emit_skip(f"metric 3 [{repo}]", "в эталоне не указан sha")
        return False
    if not sha_available(clone, sha):
        avlib.emit_skip(f"metric 3 [{repo}]", f"SHA {sha[:9]} недоступен в клоне (нужен git fetch)")
        return False
    try:
        base, head = make_snapshot_commits(clone, sha)
    except RuntimeError as exc:
        avlib.emit_skip(f"metric 3 [{repo}]", str(exc))
        return False
    r = avlib.run_archbe(
        ["arch-diff", "--repo", str(clone), "--base", base, "--head", head, "--format", "json"],
        cwd=clone,
    )
    if not r.ok:
        avlib.emit_skip(f"metric 3 [{repo}]", f"arch-diff → код {r.code}: {(r.err or '').strip()[:160]}")
        return False
    try:
        diff = json.loads(r.out)
    except ValueError as exc:
        avlib.emit_skip(f"metric 3 [{repo}]", f"arch-diff JSON не распознан: {exc}")
        return False

    det = detected_pairs(diff)
    exp = gt_pairs(gt)
    tp = det & exp
    recall = len(tp) / len(exp) if exp else 1.0
    precision = len(tp) / len(det) if det else 1.0

    want, min_prec = THRESHOLDS.get(repo, (len(exp), 0.0))
    ok = len(tp) == len(exp) and precision >= min_prec
    avlib.emit_run(
        f"metric 3 [{repo}]",
        f"recall {len(tp)}/{len(exp)} ({recall:.2f})  "
        f"precision {len(tp)}/{len(det)} ({precision:.2f})  "
        f"{'OK' if ok else 'BELOW'} (порог: {want} рёбер, prec ≥{min_prec:.2f})",
    )
    for pair in sorted(det - exp):
        avlib.emit_run(f"metric 3 [{repo}] лишнее", f"{pair[0]} → {pair[1]}")
    for pair in sorted(exp - det):
        avlib.emit_run(f"metric 3 [{repo}] пропущено", f"{pair[0]} → {pair[1]}")
    return True


def main(repos_root=None):
    avlib.emit_section("ArchRecover — метрика 3 (рёбра as-built: recall/precision)")
    measured = 0
    for repo in REPOS:
        if measure_repo(repo, repos_root):
            measured += 1
    if measured == 0:
        avlib.emit_skip("metric 3", "ни одного клона/эталона — измерить нечего")
    return 0


if __name__ == "__main__":
    root = None
    args = sys.argv[1:]
    if args and args[0] == "--repos-dir" and len(args) > 1:
        root = args[1]
    sys.exit(main(root))
