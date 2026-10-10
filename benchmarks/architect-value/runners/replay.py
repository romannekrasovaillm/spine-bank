#!/usr/bin/env python3
"""replay.py — метрика 13 (§9): arch-diff-replay значимых диффов и окон-шума.

Для каждой метки `labels/arch-diff-replay.yaml` гоняем
  arch-be arch-diff --repo <clone> --base <sha>~1 --head <sha> --format json
и сверяем «всплывшие» узлы/рёбра с ожиданием.

Определения (заморожены в PREREGISTRATION.md §2, метрика 13):
  recall значимых   — доля значимых коммитов, где «по составу» всплыл хотя бы один
                      ожидаемый токен РЁБЕР/ВНЕШНИХ систем. Компонент-узел (dir:)
                      сам по себе не «состав»: на 0.3.17 boutique-узел `✓`, но
                      состав (ребро frontend→assistant, AlloyDB) пропущен → 0/2.
  precision состава — macro-F1 множества всплывших состав-токенов (рёбра+внешние)
                      против ожидаемых того же вида.
  шум               — доля окон-шума с НЕПУСТЫМ диффом (любой узел/ребро/контракт/
                      NFR/инвариант/предложение либо route.triggers/score).

Нормализация токенов (label): срезается префикс `dir:`/`sys:`/`svc:`, берётся
последний сегмент пути, lower-case. Ребро — нормализованные концы через `->`.

Transport: stdlib (subprocess+json). Прогон без сети; `ARCH_NO_EXEC=1`. Exit 0;
отсутствие клона/бинаря/коммита — честный SKIP.
"""
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import avlib  # noqa: E402

LABELS = avlib.LABELS / "arch-diff-replay.yaml"
_PREFIXES = ("dir:", "sys:", "svc:", "node:", "edge:")
_NODE_LISTS = ("added_nodes", "removed_nodes")
_EDGE_LISTS = ("added_edges", "removed_edges")
# Поля, непустота любого из которых = «дифф непустой» (окно-шум дало изменение).
_NONEMPTY_FIELDS = (
    "added_nodes", "removed_nodes", "added_edges", "removed_edges",
    "contract_changes", "nfr_shifts", "invariants_touched", "proposals",
    "declared_unused", "adrs_touched",
)


def norm_token(tok):
    """dir:src/x → x; sys:tracing-server:9411 → tracing-server:9411."""
    if not isinstance(tok, str):
        return ""
    t = tok.strip().lower()
    for p in _PREFIXES:
        if t.startswith(p):
            t = t[len(p):]
            break
    return t.rsplit("/", 1)[-1]


def norm_edge(a, b):
    return f"{norm_token(a)}->{norm_token(b)}"


def surfaced(doc):
    """→ (components:set, externals:set, edges:set) из узлов/рёбер диффа."""
    comps, externals, edges = set(), set(), set()
    for key in _NODE_LISTS:
        for n in doc.get(key) or []:
            nid = n.get("id", "") if isinstance(n, dict) else str(n)
            (externals if nid.startswith("sys:") else comps).add(norm_token(nid))
    for key in _EDGE_LISTS:
        for e in doc.get(key) or []:
            if isinstance(e, dict):
                edges.add(norm_edge(e.get("from", ""), e.get("to", "")))
    return comps, externals, edges


def expected(label):
    comp = {norm_token(x) for x in label.get("expect_components", []) or []}
    ext = {norm_token(x) for x in label.get("expect_external", []) or []}
    edges = {norm_edge(a, b) for a, b in label.get("expect_edges", []) or []}
    return comp, ext, edges


def is_nonempty(doc):
    if any(doc.get(k) for k in _NONEMPTY_FIELDS):
        return True
    rt = doc.get("route", {}) or {}
    return bool(rt.get("triggers")) or (rt.get("score") or 0) > 0


def f1(surf, exp):
    inter = len(surf & exp)
    if not surf or not exp:
        return 0.0, inter
    p, r = inter / len(surf), inter / len(exp)
    return (2 * p * r / (p + r) if p + r else 0.0), inter


def run_diff(clone, sha):
    r = avlib.run_archbe(
        ["arch-diff", "--repo", str(clone), "--base", f"{sha}~1", "--head", sha,
         "--format", "json"],
        cwd=str(clone),
    )
    if r.code != 0:
        return None, f"arch-diff код {r.code}: {(r.err or r.out or '').strip()[:160]}"
    try:
        return json.loads(r.out), None
    except json.JSONDecodeError as e:
        return None, f"arch-diff: не JSON ({e})"


def metric13(repos_root):
    lab = avlib.load_yaml(LABELS)
    if not isinstance(lab, dict):
        avlib.emit_skip("metric 13", f"не разобран {LABELS}")
        return
    significant = lab.get("significant", []) or []
    noise = lab.get("noise", []) or []
    thresholds = (lab.get("defaults", {}) or {}).get("thresholds", {}) or {}

    clone_cache = {}

    def clone_for(repo):
        if repo not in clone_cache:
            clone_cache[repo] = avlib.repo_clone_dir(repo, repos_root)
        return clone_cache[repo]

    # ── значимые: recall по составу + macro-F1 состава ──────────────────────
    hits, f1s, sig_measured = 0, [], 0
    for s in significant:
        repo, sha, title = s.get("repo"), s.get("sha"), s.get("title", "")
        clone = clone_for(repo)
        if clone is None:
            avlib.emit_skip(f"metric 13 [{repo}:{sha[:7]}]", f"клон {repo} отсутствует")
            continue
        doc, err = run_diff(clone, sha)
        if err:
            avlib.emit_skip(f"metric 13 [{repo}:{sha[:7]}]", err)
            continue
        sig_measured += 1
        _comps, ext, edges = surfaced(doc)
        ecomp, eext, eedges = expected(s)
        surf_comp = ext | edges
        exp_comp = eext | eedges
        score, inter = f1(surf_comp, exp_comp)
        hit = inter >= 1
        hits += 1 if hit else 0
        f1s.append(score)
        avlib.emit_run(
            f"metric 13 significant [{repo}:{sha[:7]}]",
            f"hit={'да' if hit else 'нет'} f1={score:.2f} "
            f"surf={sorted(surf_comp) or 'нет'} exp={sorted(exp_comp)} — {title[:40]}",
        )

    # ── окна-шума: доля непустых диффов ─────────────────────────────────────
    noise_fp, noise_measured = 0, 0
    for n in noise:
        repo, sha, kind = n.get("repo"), n.get("sha"), n.get("kind", "")
        clone = clone_for(repo)
        if clone is None:
            avlib.emit_skip(f"metric 13 noise [{repo}:{sha[:7]}]", f"клон {repo} отсутствует")
            continue
        doc, err = run_diff(clone, sha)
        if err:
            avlib.emit_skip(f"metric 13 noise [{repo}:{sha[:7]}]", err)
            continue
        noise_measured += 1
        fp = is_nonempty(doc)
        noise_fp += 1 if fp else 0
        if fp:
            print(f"[FIND] metric 13 noise [{repo}:{sha[:7]}]: непустой дифф ({kind})")

    if sig_measured == 0 and noise_measured == 0:
        avlib.emit_skip("metric 13", "ни одна метка не измерена (нет клонов/коммитов)")
        return

    recall = (hits / sig_measured) if sig_measured else 0.0
    macro_f1 = (sum(f1s) / len(f1s)) if f1s else 0.0
    noise_ratio = (noise_fp / noise_measured) if noise_measured else 0.0
    t_recall = thresholds.get("recall_significant", 0.80)
    t_f1 = thresholds.get("composition_f1", 0.80)
    t_noise = thresholds.get("noise_ratio", 0.10)
    avlib.emit_run(
        "metric 13",
        f"recall_significant={hits}/{sig_measured}={recall:.2f} (порог≥{t_recall}); "
        f"composition_F1={macro_f1:.2f} (порог≥{t_f1}); "
        f"noise={noise_fp}/{noise_measured}={noise_ratio:.2f} (порог≤{t_noise}) "
        f"[0.3.17 before: 0/2 по составу, 6/6 шум]",
    )


def main(repos_root=None):
    avlib.emit_section("arch-diff-replay — метрика 13")
    metric13(repos_root)
    return 0


if __name__ == "__main__":
    root = None
    args = sys.argv[1:]
    if args and args[0] == "--repos-dir" and len(args) > 1:
        root = args[1]
    sys.exit(main(root))
