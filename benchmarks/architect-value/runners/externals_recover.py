#!/usr/bin/env python3
"""externals_recover.py — метрика 5 (§9): precision внешних систем as-built.

Волна R (извлекатель R3). Берёт замороженную секцию `external:` эталона
`ground_truth/<repo>.yaml` и узлы `external_system` восстановленного графа
(`recover … --format json`), сравнивает МНОЖЕСТВА имён: имя эталона (например,
`vertex-ai`) против хоста узла (`sys:vertex-ai` → `vertex-ai`, порт отброшен).
Считает precision = найдено-в-эталоне / всего-найденных.

Эталон: boutique (alloydb, vertex-ai), petclinic (openai, azure-openai),
buckpal/library — пусто. Порог §9: precision ≥ 0.80 (baseline ≤ 0.21 —
на 0.3.16 граф плодил ~19 внешних узлов, из них ≥15 шум).

Сравнение: лишние найденные узлы (не в эталоне) понижают precision; честный FP
(реальная внешняя система кода, не попавшая в замороженный эталон) фиксируется
`[FIND]` и не «подгоняется» правкой эталона (D-процедура). Нет клона / SHA → SKIP.
exit 0 (измерение).
"""
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import avlib  # noqa: E402

REPOS = ("boutique", "petclinic", "buckpal", "library")
THRESHOLD = 0.80


def gt_externals(gt):
    """Имена внешних систем эталона (секция `external:` → поле `name`)."""
    return {e.get("name") for e in (gt.get("external") or []) if e.get("name")}


def measure_repo(repo, repos_root):
    """Возвращает (tp, detected) или None для SKIP/vacuous."""
    gt_path = avlib.GROUND_TRUTH / f"{repo}.yaml"
    if not gt_path.is_file():
        avlib.emit_skip(f"metric 5 [{repo}]", f"нет эталона {gt_path}")
        return None
    gt = avlib.load_yaml(gt_path) or {}
    exp = gt_externals(gt)
    clone = avlib.repo_clone_dir(repo, repos_root)
    if clone is None:
        avlib.emit_skip(f"metric 5 [{repo}]", "клон отсутствует (--repos-dir/ARCH_VALUE_REPOS)")
        return None
    graph, reason = avlib.recover_graph(clone, gt.get("sha"))
    if graph is None:
        avlib.emit_skip(f"metric 5 [{repo}]", reason)
        return None
    det = avlib.external_names(graph)
    if not exp and not det:
        avlib.emit_run(f"metric 5 [{repo}]", "vacuous: эталон и график пусты (precision не считается)")
        return None
    tp = det & exp
    fp = sorted(det - exp)
    miss = sorted(exp - det)
    precision = len(tp) / len(det) if det else 1.0
    ok = precision >= THRESHOLD
    avlib.emit_run(
        f"metric 5 [{repo}]",
        f"precision {len(tp)}/{len(det)} ({precision:.2f})  "
        f"{'OK' if ok else 'BELOW'} (порог ≥{THRESHOLD:.2f})",
    )
    for name in fp:
        avlib.emit_run(f"metric 5 [{repo}] лишнее (FP)", name)
    for name in miss:
        avlib.emit_run(f"metric 5 [{repo}] пропущено", name)
    return (len(tp), len(det))


def main(repos_root=None):
    avlib.emit_section("ArchRecover — метрика 5 (внешние системы as-built: precision)")
    tp_total = det_total = 0
    for repo in REPOS:
        res = measure_repo(repo, repos_root)
        if res is not None:
            tp_total += res[0]
            det_total += res[1]
    if det_total == 0:
        avlib.emit_skip("metric 5", "ни одной внешней системы не измерено (нет клонов/эталонов)")
        return 0
    precision = tp_total / det_total
    ok = precision >= THRESHOLD
    avlib.emit_run(
        "metric 5 (итог)",
        f"precision {tp_total}/{det_total} ({precision:.2f})  {'OK' if ok else 'BELOW'} (порог ≥{THRESHOLD:.2f})",
    )
    if not ok:
        avlib.emit_find(
            "metric 5",
            "precision внешних систем ниже порога: лишние узлы — честные FP графа; "
            "правка замороженного эталона запрещена (D-процедура).",
        )
    return 0


if __name__ == "__main__":
    root = None
    args = sys.argv[1:]
    if args and args[0] == "--repos-dir" and len(args) > 1:
        root = args[1]
    sys.exit(main(root))
