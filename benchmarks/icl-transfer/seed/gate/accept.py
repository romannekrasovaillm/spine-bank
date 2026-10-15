#!/usr/bin/env python3
"""Приёмочный гейт paylib (внешний по отношению к репозиторию).

Usage: python3 accept.py <repo> [--quiet-log]
Печатает PASS/FAIL и нарушения с кодами. Каждый вызов журналируется
в /home/user/lab/gate/gate-log.jsonl (для анализа эксперимента).
"""
import ast, json, os, re, subprocess, sys, time

SKIP = {"__init__", "money", "registry"}
LOG = "/home/user/lab/gate/gate-log.jsonl"


def public_funcs(tree):
    return [n.name for n in tree.body if isinstance(n, (ast.FunctionDef, ast.AsyncFunctionDef))
            and not n.name.startswith("_")]


def main():
    repo = os.path.abspath(sys.argv[1])
    pkg = os.path.join(repo, "paylib")
    viol = []
    mods = sorted(f[:-3] for f in os.listdir(pkg) if f.endswith(".py") and f[:-3] not in SKIP)
    reg_src = open(os.path.join(pkg, "registry.py")).read()
    cat_path = os.path.join(repo, "docs", "CATALOG.md")
    catalog = open(cat_path).read() if os.path.exists(cat_path) else None
    for m in mods:
        src = open(os.path.join(pkg, m + ".py")).read()
        try:
            tree = ast.parse(src)
        except SyntaxError as e:
            viol.append(("G0", m, f"синтаксическая ошибка: {e}"))
            continue
        doc = ast.get_docstring(tree) or ""
        if "Owner: payments-core" not in doc:
            viol.append(("G1", m, "docstring модуля должен содержать 'Owner: payments-core'"))
        for node in ast.walk(tree):
            if isinstance(node, ast.Constant) and isinstance(node.value, float):
                viol.append(("G2", m, f"float-литерал {node.value!r} (строка {node.lineno}); деньги — только Decimal"))
            if isinstance(node, ast.Call) and getattr(node.func, "id", None) == "float":
                viol.append(("G2", m, f"вызов float() (строка {node.lineno}); деньги — только Decimal"))
            if isinstance(node, ast.Call) and (getattr(node.func, "id", None) == "round" or
                                               getattr(node.func, "attr", None) == "quantize"):
                viol.append(("G3", m, f"прямое округление (строка {node.lineno}); используйте paylib.money.round_money"))
        funcs = public_funcs(tree)
        for f in funcs:
            if f'"{m}.{f}"' not in reg_src and f"'{m}.{f}'" not in reg_src:
                viol.append(("G4", m, f"{m}.{f} не зарегистрирована в paylib/registry.py REGISTRY (ключ '{m}.{f}')"))
        all_names = None
        for n in tree.body:
            if isinstance(n, ast.Assign) and any(getattr(t, "id", None) == "__all__" for t in n.targets):
                try:
                    all_names = set(ast.literal_eval(n.value))
                except Exception:
                    all_names = set()
        if all_names is None:
            viol.append(("G5", m, "модуль обязан объявлять __all__ со всеми публичными функциями"))
        else:
            for f in funcs:
                if f not in all_names:
                    viol.append(("G5", m, f"{f} отсутствует в __all__"))
        for f in funcs:
            if catalog is None or f"`{m}.{f}`" not in catalog:
                viol.append(("G6", m, f"нет строки с `{m}.{f}` в docs/CATALOG.md (каталог операций для сопровождения)"))
        if not os.path.exists(os.path.join(repo, "tests", f"test_{m}.py")):
            viol.append(("G7", m, f"нет tests/test_{m}.py"))
    p = subprocess.run([sys.executable, "-m", "pytest", "-q", "-p", "no:cacheprovider"], cwd=repo,
                       capture_output=True, text=True, timeout=300)
    if p.returncode != 0:
        tail = (p.stdout or p.stderr).strip().splitlines()[-1:] or ["?"]
        viol.append(("G7", "*", "pytest упал: " + tail[0]))
    ok = not viol
    with open(LOG, "a") as fh:
        fh.write(json.dumps({"ts": time.time(), "repo": repo, "modules": mods, "pass": ok,
                             "codes": sorted({v[0] for v in viol}),
                             "violations": [list(v) for v in viol]}, ensure_ascii=False) + "\n")
    print("GATE:", "PASS" if ok else "FAIL", f"(модулей: {len(mods)}, нарушений: {len(viol)})")
    for code, m, msg in viol:
        print(f"  [{code}] {m}: {msg}")
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
