#!/usr/bin/env python3
"""stores_recover.py — метрика 4 (§9): recall хранилищ в графе «как построено».

Волна R (извлекатель R3, ADR-069). Берёт замороженную секцию `stores:` эталона
`ground_truth/<repo>.yaml` и восстановленный граф снимка (`recover <clone>
--rev <sha> --format json` — тот же приём, что в metric-3, но напрямую, без
dangling-коммитов), сравнивает МНОЖЕСТВА имён хранилищ: имя эталона (например,
`redis-cart`) против хоста узла графа (`store:redis://redis-cart` → `redis-cart`,
порт отброшен). Считает recall = найдено эталонных / всего эталонных.

Эталон хранилищ: boutique 1 (`redis-cart`, redis), petclinic 3 (customers/vets/
visits-db, mysql), buckpal 1 (`account-db`, h2 in-memory), library 0.
Порог §9: recall ≥ 0.80 (baseline 0/4).

Прогонщик измеряет и печатает ФАКТ: если recall ниже порога — `[FIND]` с
причиной по каждому репо (не «зелень»; SKIP ≠ PASS). Отдельно печатаются
лишние хранилища графа (FP precision-стороны) как наблюдение — precision в
метрику 4 §9 не входит.

Как и прочие прогонщики: нет клона / SHA недоступен / нет секции `stores` →
честный SKIP. exit 0 (измерение), ненулевой — только если `arch-be` не стартует.
"""
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import avlib  # noqa: E402

REPOS = ("boutique", "petclinic", "buckpal", "library")
THRESHOLD = 0.80
# В `release/kubernetes-manifests.yaml` (сводный манифест с `image: redis:alpine`)
# ветка (3) извлекателя R3 разворачивает любой `host:port` в `store:redis://…`;
# отсюда кластер ложных хранилищ на boutique. Печатаем их, но в recall не считаем.
MAX_FP_PRINT = 6


def gt_stores(gt):
    """Имена хранилищ эталона (секция `stores:` → поле `name`)."""
    return {s.get("name") for s in (gt.get("stores") or []) if s.get("name")}


def measure_repo(repo, repos_root):
    """Возвращает (matched, total, fp_list) или None для SKIP."""
    gt_path = avlib.GROUND_TRUTH / f"{repo}.yaml"
    if not gt_path.is_file():
        avlib.emit_skip(f"metric 4 [{repo}]", f"нет эталона {gt_path}")
        return None
    gt = avlib.load_yaml(gt_path) or {}
    exp = gt_stores(gt)
    if not exp:
        avlib.emit_run(f"metric 4 [{repo}]", "vacuous: секция `stores` эталона пуста (recall не считается)")
        return None
    clone = avlib.repo_clone_dir(repo, repos_root)
    if clone is None:
        avlib.emit_skip(f"metric 4 [{repo}]", "клон отсутствует (--repos-dir/ARCH_VALUE_REPOS)")
        return None
    graph, reason = avlib.recover_graph(clone, gt.get("sha"))
    if graph is None:
        avlib.emit_skip(f"metric 4 [{repo}]", reason)
        return None
    det = avlib.datastore_names(graph)
    matched = det & exp
    fp = sorted(det - exp)
    miss = sorted(exp - det)
    recall = len(matched) / len(exp)
    ok = recall >= THRESHOLD
    avlib.emit_run(
        f"metric 4 [{repo}]",
        f"recall {len(matched)}/{len(exp)} ({recall:.2f})  "
        f"{'OK' if ok else 'BELOW'} (порог ≥{THRESHOLD:.2f})",
    )
    for name in miss:
        avlib.emit_run(f"metric 4 [{repo}] пропущено", name)
    if fp:
        avlib.emit_run(
            f"metric 4 [{repo}] лишних хранилищ (FP, precision-сторона)",
            f"{len(fp)}: " + ", ".join(fp[:MAX_FP_PRINT]) + (" …" if len(fp) > MAX_FP_PRINT else ""),
        )
    return (len(matched), len(exp))


def main(repos_root=None):
    avlib.emit_section("ArchRecover — метрика 4 (хранилища as-built: recall)")
    matched = total = 0
    for repo in REPOS:
        res = measure_repo(repo, repos_root)
        if res is not None:
            matched += res[0]
            total += res[1]
    if total == 0:
        avlib.emit_skip("metric 4", "ни одной секции `stores` не измерено (нет клонов/эталонов)")
        return 0
    recall = matched / total
    ok = recall >= THRESHOLD
    avlib.emit_run(
        "metric 4 (итог)",
        f"recall {matched}/{total} ({recall:.2f})  {'OK' if ok else 'BELOW'} (порог ≥{THRESHOLD:.2f})",
    )
    if not ok:
        avlib.emit_find(
            "metric 4",
            "recall хранилищ ниже порога: petclinic (3 СУБД) и buckpal (h2) не "
            "восстанавливаются — конфигурация datasource вне репо (Spring Cloud "
            "Config / только build-dependency). Порог §9 не меняется; см. DEVIATIONS.",
        )
    return 0


if __name__ == "__main__":
    root = None
    args = sys.argv[1:]
    if args and args[0] == "--repos-dir" and len(args) > 1:
        root = args[1]
    sys.exit(main(root))
