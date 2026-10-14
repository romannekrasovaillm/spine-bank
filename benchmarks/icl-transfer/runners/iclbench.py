#!/usr/bin/env python3
"""benchmarks/icl-transfer — детерминированный прогон механики обучающего контура.

Режимы:
  --ci                 все руки --ci (FakeLlm + fake-harness), пишет ci-summary.json
  --ci-arm ARM          одна рука (FI|FIL|X|GEN|PAIR|PROMO|FLEET)
  --gen SEED            детерминированный выбор конвенций + задач по семени
  --gate REPO --seed N  прогон обобщённого гейта на репозитории
  --check-antigoodhart  анти-Goodhart греп: маркеры пула не в src/ assets/
  --self-test           самопроверка парсера и генератора (без рук)

Только stdlib. Пререгистрация (pool/seeds/holdout) заморожена и читается
только как вход; holdout.txt — только для исключения по id. Никакие маркерные
строки пула не зашиты в этот файл (они читаются из prereg/pool.yaml).
"""
import hashlib
import json
import os
import re
import sys

BASE = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
PREREG = os.path.join(BASE, "prereg")
SEED_DIR = os.path.join(BASE, "seed")
TEMPLATE = os.path.join(SEED_DIR, "template", "paylib")
REPO_ROOT = os.path.abspath(os.path.join(BASE, "..", ".."))

MAX_ATTEMPTS = 3          # не более 3 прогонов исполнителя на задачу (как в стенде)
PROMOTE_CONFIRMATIONS = 2  # подтверждений до промоушена урока в pre-commit (L7)


# ─────────────────────────────────────────────────────────────────────────────
# 1. Мини-YAML (подмножество: блок-мапы, списки `- `, инлайн `{}`/`[]`, кавычки)
# ─────────────────────────────────────────────────────────────────────────────

def _strip_comment(line):
    in_s = in_d = False
    out = []
    for c in line:
        if c == "'" and not in_d:
            in_s = not in_s
        elif c == '"' and not in_s:
            in_d = not in_d
        elif c == "#" and not in_s and not in_d:
            break
        out.append(c)
    return "".join(out)


def _parse_scalar(s):
    s = s.strip()
    if not s:
        return None
    if s[0] in "'\"":
        q = s[0]
        body = s[1:-1] if len(s) >= 2 and s[-1] == q else s[1:]
        return body
    if s in ("true", "True"):
        return True
    if s in ("false", "False"):
        return False
    if s in ("null", "~"):
        return None
    if s.startswith("{") or s.startswith("["):
        return _parse_flow(s)
    try:
        return int(s)
    except ValueError:
        pass
    try:
        return float(s)
    except ValueError:
        pass
    return s


def _parse_flow(s):
    """Парсит инлайн-поток `{ k: v, ... }` или `[a, b]` с учётом кавычек."""
    i = 0
    n = len(s)

    def skip_ws():
        nonlocal i
        while i < n and s[i] in " \t":
            i += 1

    def read_string():
        nonlocal i
        q = s[i]
        i += 1
        buf = []
        while i < n:
            c = s[i]
            if c == q:
                # YAML: удвоение кавычки '' экранирует её внутри строки
                if q == "'" and i + 1 < n and s[i + 1] == "'":
                    buf.append("'")
                    i += 2
                    continue
                i += 1
                return "".join(buf)
            buf.append(c)
            i += 1
        return "".join(buf)

    def read_value():
        nonlocal i
        skip_ws()
        if i >= n:
            return None
        if s[i] in "'\"":
            return read_string()
        if s[i] == "{":
            return read_map()
        if s[i] == "[":
            return read_list()
        j = i
        while j < n and s[j] not in ",}]":
            j += 1
        val = _parse_scalar(s[i:j])
        i = j
        return val

    def read_list():
        nonlocal i
        i += 1  # '['
        out = []
        while True:
            skip_ws()
            if i < n and s[i] == "]":
                i += 1
                return out
            out.append(read_value())
            skip_ws()
            if i < n and s[i] == ",":
                i += 1
            elif i < n and s[i] == "]":
                i += 1
                return out

    def read_map():
        nonlocal i
        i += 1  # '{'
        out = {}
        while True:
            skip_ws()
            if i < n and s[i] == "}":
                i += 1
                return out
            if s[i] in "'\"":
                k = read_string()
            else:
                j = i
                while j < n and s[j] not in ":}":
                    j += 1
                k = s[i:j].strip()
                i = j
            skip_ws()
            if i < n and s[i] == ":":
                i += 1
            out[k] = read_value()
            skip_ws()
            if i < n and s[i] == ",":
                i += 1
            elif i < n and s[i] == "}":
                i += 1
                return out

    return read_map() if s.lstrip().startswith("{") else read_list()


def parse_yaml(text):
    """Блок-YAML: вложенные мапы по отступам + списки `- ` (в т.ч. списки карт)."""
    items = []  # (indent, content)
    for raw in text.splitlines():
        if not raw.strip():
            continue
        stripped = _strip_comment(raw).rstrip()
        if not stripped.strip():
            continue
        indent = len(raw) - len(raw.lstrip(" "))
        items.append((indent, stripped.strip()))

    def parse_mapping(idx, cont_indent):
        """Ключи карты на отступе >= cont_indent (продолжение элемента списка)."""
        m = {}
        while idx < len(items):
            ind, content = items[idx]
            if ind < cont_indent:
                break
            if content.startswith("- "):
                break
            if not _has_kv(content):
                idx += 1
                continue
            k, v = _split_kv(content)
            idx += 1
            if v is None:
                sub, idx = parse_block(idx, ind + 1)
                m[k] = sub if sub is not None else {}
            else:
                m[k] = v
        return m, idx

    def parse_list_item(idx):
        ind, content = items[idx]
        rest = content[2:].strip()
        idx += 1
        if rest in ("", "-"):
            sub, idx = parse_block(idx, ind + 1)
            return (sub if sub is not None else {}), idx
        if _has_kv(rest) and not rest.startswith(("{", "[")):
            # список карт: первый ключ на строке `- k: v`, продолжение на cont-отступе
            cont = ind + 2
            k, v = _split_kv(rest)
            item = {}
            if v is None:
                sub, idx = parse_block(idx, ind + 1)
                item[k] = sub if sub is not None else {}
            else:
                item[k] = v
            rest_m, idx = parse_mapping(idx, cont)
            item.update(rest_m)
            return item, idx
        return _parse_scalar(rest), idx

    def parse_block(idx, min_indent):
        result = None
        is_list = None
        while idx < len(items):
            ind, content = items[idx]
            if ind < min_indent:
                break
            if is_list is None:
                is_list = content.startswith("- ")
            if is_list:
                if not content.startswith("- "):
                    break
                if result is None:
                    result = []
                item, idx = parse_list_item(idx)
                result.append(item)
            else:
                if not _has_kv(content):
                    idx += 1
                    continue
                if result is None:
                    result = {}
                k, v = _split_kv(content)
                idx += 1
                if v is None:
                    sub, idx = parse_block(idx, ind + 1)
                    result[k] = sub if sub is not None else {}
                else:
                    result[k] = v
        return result, idx

    return parse_block(0, 0)[0]


def _has_kv(content):
    """Есть ли `key: value` вне кавычек и вне инлайн-потока."""
    in_s = in_d = False
    for i, c in enumerate(content):
        if c == "'" and not in_d:
            in_s = not in_s
        elif c == '"' and not in_s:
            in_d = not in_d
        elif c == ":" and not in_s and not in_d:
            return True
    return False


def _split_kv(content):
    """Разделяет `key: value` по первому двоеточию вне кавычек."""
    in_s = in_d = False
    for i, c in enumerate(content):
        if c == "'" and not in_d:
            in_s = not in_s
        elif c == '"' and not in_s:
            in_d = not in_d
        elif c == ":" and not in_s and not in_d:
            key = content[:i].strip()
            val = content[i + 1:].strip()
            return key, (_parse_scalar(val) if val else None)
    return content, None


# ─────────────────────────────────────────────────────────────────────────────
# 2. Загрузка пререгистрации (pool / holdout / seeds)
# ─────────────────────────────────────────────────────────────────────────────

def load_pool():
    data = parse_yaml(_read(os.path.join(PREREG, "pool.yaml")))
    pool = data.get("pool", [])
    out = []
    for item in pool:
        out.append({
            "id": item["id"],
            "name": item.get("name", ""),
            "subject_kind": item.get("subject_kind", "new_module"),
            "params": item.get("params", {}) or {},
            "marker": item.get("marker", ""),
        })
    return out


def load_holdout_ids():
    """Читает holdout.txt ТОЛЬКО как множество id для исключения (не логируется)."""
    ids = set()
    for line in _read(os.path.join(PREREG, "holdout.txt")).splitlines():
        line = line.strip()
        if line and not line.startswith("#"):
            ids.add(line)
    return ids


def load_seeds():
    seeds = {}
    for line in _read(os.path.join(PREREG, "seeds.txt")).splitlines():
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        m = re.match(r"^([A-Z+M]+):\s*(.*)$", line)
        if m:
            seeds[m.group(1)] = [int(x) for x in m.group(2).split()]
    return seeds


def _read(path):
    with open(path, "r", encoding="utf-8") as fh:
        return fh.read()


# ─────────────────────────────────────────────────────────────────────────────
# 3. Детерминированный выбор конвенций (без `random`, по sha256)
# ─────────────────────────────────────────────────────────────────────────────

def select_conventions(seed, pool=None, holdout=None):
    pool = pool if pool is not None else load_pool()
    holdout = holdout if holdout is not None else load_holdout_ids()
    eligible = [c for c in pool if c["id"] not in holdout]
    eligible.sort(key=lambda c: c["id"])
    n = 2 + (seed % 2)  # 2 или 3 конвенции на прогон
    n = min(n, len(eligible))
    ranked = sorted(
        eligible,
        key=lambda c: (int(hashlib.sha256(f"{seed}:{c['id']}".encode()).hexdigest(), 16), c["id"]),
        reverse=True,
    )
    return ranked[:n]


# ─────────────────────────────────────────────────────────────────────────────
# 4. Семейства задач
# ─────────────────────────────────────────────────────────────────────────────

def load_module_tasks():
    tasks = json.loads(_read(os.path.join(SEED_DIR, "tasks.json")))
    out = []
    for t in tasks:
        m = re.search(r"функци\w*\s+(\w+)", t["text"])
        func = m.group(1) if m else t["module"]
        out.append({
            "id": t["id"],
            "family": "module",
            "module": t["module"],
            "func": func,
            "sig": _sig_for(t["text"], func),
            "text": t["text"],
            "doc": _doc_for(t["module"]),
            "subject_kinds": ["new_module", "new_public_symbol"],
        })
    return out


CLI_TASKS = [
    {"id": "C1", "family": "cli", "module": "cli_fees", "func": "cmd_fees",
     "sig": "cmd_fees(args)", "doc": "CLI-команда расчёта комиссии",
     "text": "Добавь CLI-команду `paylib fees <amount> <rate_pct> <min_fee>`: печатает комиссию за карту. Покрой тестами.",
     "subject_kinds": ["new_module", "new_public_symbol"]},
    {"id": "C2", "family": "cli", "module": "cli_fx", "func": "cmd_fx",
     "sig": "cmd_fx(args)", "doc": "CLI-команда конвертации",
     "text": "Добавь CLI-команду `paylib fx <amount> <rate>`: печатает сконвертированную сумму. Покрой тестами.",
     "subject_kinds": ["new_module", "new_public_symbol"]},
    {"id": "C3", "family": "cli", "module": "cli_interest", "func": "cmd_interest",
     "sig": "cmd_interest(args)", "doc": "CLI-команда процентов",
     "text": "Добавь CLI-команду `paylib interest <principal> <rate_pct> <days>`: печатает простые проценты. Покрой тестами.",
     "subject_kinds": ["new_module", "new_public_symbol"]},
    {"id": "C4", "family": "cli", "module": "cli_report", "func": "cmd_report",
     "sig": "cmd_report(args)", "doc": "CLI-команда отчёта операций",
     "text": "Добавь CLI-команду `paylib report`: печатает список операций из каталога. Покрой тестами.",
     "subject_kinds": ["new_module", "new_public_symbol"]},
]

# «миграция данных» — отложенное семейство (только рука GEN / E3).
MIGRATION_TASKS = [
    {"id": "M1", "family": "migration", "module": "001_add_created_at", "func": "upgrade",
     "sig": "upgrade(conn)", "doc": "миграция: колонка created_at",
     "text": "Напиши миграцию данных: добавь колонку created_at (tz-aware) в таблицу operations. Покрой тестами.",
     "subject_kinds": ["new_module"]},
    {"id": "M2", "family": "migration", "module": "002_backfill_currency", "func": "upgrade",
     "sig": "upgrade(conn)", "doc": "миграция: бэкфилл currency",
     "text": "Напиши миграцию данных: бэкфилл поля currency из legacy-таблицы. Покрой тестами.",
     "subject_kinds": ["new_module"]},
    {"id": "M3", "family": "migration", "module": "003_idempotency_index", "func": "upgrade",
     "sig": "upgrade(conn)", "doc": "миграция: индекс идемпотентности",
     "text": "Напиши миграцию данных: добавь индекс по idempotency_key. Покрой тестами.",
     "subject_kinds": ["new_module"]},
]


def _sig_for(text, func):
    m = re.search(re.escape(func) + r"\s*\(([^)]*)\)", text)
    if m:
        return f"{func}({m.group(1)})"
    return f"{func}()"


def _doc_for(module):
    docs = {
        "fees": "комиссии за операции",
        "fx": "конвертация валют",
        "interest": "простые проценты",
        "installments": "рассрочка платежей",
        "vat": "выделение НДС",
    }
    return docs.get(module, "операция")


def family_tasks(family):
    if family == "module":
        return load_module_tasks()
    if family == "cli":
        return [dict(t) for t in CLI_TASKS]
    if family == "migration":
        return [dict(t) for t in MIGRATION_TASKS]
    raise ValueError(f"неизвестное семейство: {family}")


def module_path(task):
    if task["family"] == "migration":
        return f"migrations/{task['module']}.py"
    return f"paylib/{task['module']}.py"


# ─────────────────────────────────────────────────────────────────────────────
# 5. Шаблон репозитория (in-memory tree) и «свежий репозиторий»
# ─────────────────────────────────────────────────────────────────────────────

def load_template():
    tree = {}
    for dirpath, _dirs, files in os.walk(TEMPLATE):
        for fn in files:
            full = os.path.join(dirpath, fn)
            rel = os.path.relpath(full, TEMPLATE)
            if rel == "conftest.py":
                continue
            tree[rel.replace(os.sep, "/")] = _read(full)
    return tree


def fresh_repo(template):
    return dict(template)


# ─────────────────────────────────────────────────────────────────────────────
# 6. Fake-исполнитель: генерирует файлы задачи, соблюдая видимые правила всегда,
#    скрытые — только если «научен» (урок инжектирован).
# ─────────────────────────────────────────────────────────────────────────────

def _public_funcs(source):
    return [m.group(1) for m in re.finditer(r"^def\s+([a-zA-Z_][a-zA-Z0-9_]*)\s*\(", source, re.M)]


def build_module_source(task, taught):
    mod = task["module"]
    fn = task["func"]
    fam = task["family"]
    params = task["sig"].split("(", 1)[1].rstrip(")")
    lines = [f'"""{mod} — {task["doc"]}. Owner: payments-core"""']
    lines.append("from decimal import Decimal")
    lines.append("from paylib.money import round_money, to_money")
    if taught.get("HC-03"):
        lines.append("import logging")
        lines.append(f'logger = logging.getLogger("PAYLIB.{mod.upper()}")')
    if taught.get("HC-13"):
        lines.append("from datetime import datetime, timezone")
    else:
        lines.append("from datetime import datetime")
    if not taught.get("HC-04"):
        lines.append("from paylib.store import Ledger")
    lines.append("")

    body = []
    if taught.get("HC-07"):
        body.append("@audit_op")
    body.append(f"def {task['sig']}:")
    if taught.get("HC-11"):
        body.append('    """{}.'.format(task["doc"].capitalize()))
        body.append("")
        body.append("    Args:")
        body.append(f"        {params.split(',')[0].strip() if params else 'value'}: входное значение.")
        body.append("")
        body.append("    Returns:")
        body.append("        Decimal: результат, округлённый до копеек.")
        body.append('    """')
    else:
        body.append(f'    """{task["doc"].capitalize()}."""')
    if taught.get("HC-09"):
        body.append(f"    a = to_money({params.split(',')[0].strip() if params else '0'})")
    else:
        body.append(f"    a = Decimal(str({params.split(',')[0].strip() if params else '0'}))")
    if not taught.get("HC-10"):
        body.append(f"    print('debug {fn}')")
    if not taught.get("HC-13"):
        body.append("    _now = datetime.utcnow()")
    if not taught.get("HC-04"):
        body.append("    _ledger = Ledger()")
    body.append("    result = a")
    body.append("    return round_money(result)")

    src = "\n".join(lines + body) + "\n"
    if taught.get("HC-01"):
        src += f"\n__all__ = [{', '.join(repr(f) for f in _public_funcs(src))}]\n"
    return src


def build_test_source(task, taught):
    fn = task["func"]
    mod = task["module"]
    if taught.get("HC-06"):
        name = f"test_{mod}__rounds__down"
    else:
        name = f"test_{fn}"
    return (
        f'"""{mod} — тесты. Owner: payments-core"""\n'
        f"from decimal import Decimal\n"
        f"from paylib.money import round_money\n"
        f"\n"
        f"def {name}():\n"
        f"    assert round_money(Decimal('1.005')) == Decimal('1.00')\n"
    )


def build_cli_test_source(task, taught):
    return build_test_source(task, taught)


def build_migration_test_source(task, taught):
    fn = task["func"]
    mod = task["module"]
    if taught.get("HC-06"):
        name = f"test_{mod}__applies__once"
    else:
        name = f"test_{fn}"
    return (
        f'"""{mod} — тест миграции. Owner: payments-core"""\n'
        f"\n"
        f"def {name}():\n"
        f"    assert True\n"
    )


def fake_executor(tree, task, taught):
    """Записывает файлы задачи в `tree` (in-memory), возвращает дерево."""
    path = module_path(task)
    tree[path] = build_module_source(task, taught)
    # регистрация публичной функции в registry (видимое правило G4)
    if task["family"] != "migration":
        _register(tree, task)
    # каталог операций (HC-02)
    if taught.get("HC-02"):
        _catalog_add(tree, task)
    # CHANGELOG.d (HC-05)
    if taught.get("HC-05"):
        tree[f"CHANGELOG.d/{task['id']}.md"] = f"# {task['id']}\n"
    # тест (видимое правило G7)
    if task["family"] == "migration":
        tree[f"tests/test_{task['module']}.py"] = build_migration_test_source(task, taught)
    else:
        tree[f"tests/test_{task['module']}.py"] = build_test_source(task, taught)
    return tree


def _register(tree, task):
    path = "paylib/registry.py"
    src = tree.get(path, "")
    key = f"{task['module']}.{task['func']}"
    if key not in src:
        # дописываем ключ в REGISTRY (детерминированно, в конец словаря)
        if "REGISTRY = {" in src:
            entry = f"    {key!r}: None,"
            src = src.replace("REGISTRY = {", "REGISTRY = {\n" + entry, 1)
        else:
            src += f"\nREGISTRY = {{ {key!r}: None }}\n"
        tree[path] = src


def _catalog_add(tree, task):
    path = "docs/OPERATIONS-CATALOG.md"
    src = tree.get(path, "")
    if not src:
        src = "| операция | модуль | владелец |\n|---|---|---|\n"
    src += f"| {task['func']} | {task['module']} | payments-core |\n"
    tree[path] = src


# ─────────────────────────────────────────────────────────────────────────────
# 7. Обобщённый гейт (видимые правила + конвенции пула)
# ─────────────────────────────────────────────────────────────────────────────

def gate(base_repo, task, selected):
    """Возвращает (passed, violations). Каждое нарушение — (code, module, message)."""
    tree = base_repo
    path = module_path(task)
    mod = task["module"]
    src = tree.get(path, "")
    viol = []

    # Видимые правила (исполнитель соблюдает их всегда — по коду, как в стенде).
    if f"Owner: payments-core" not in src:
        viol.append(("G1", mod, "docstring модуля без 'Owner: payments-core'"))
    if re.search(r"\bfloat\s*\(|\b\d+\.\d+\b", src):
        viol.append(("G2", mod, "float-литерал/вызов float(); деньги — только Decimal"))
    if re.search(r"\bround\s*\(|\.quantize\(", src):
        viol.append(("G3", mod, "прямое округление; используйте round_money"))
    if task["family"] != "migration":
        reg = tree.get("paylib/registry.py", "")
        key = f"{task['module']}.{task['func']}"
        if f'"{key}"' not in reg and f"'{key}'" not in reg:
            viol.append(("G4", mod, f"{key} не зарегистрирована в registry.py"))
    test_path = f"tests/test_{mod}.py"
    if test_path not in tree:
        viol.append(("G7", mod, f"нет {test_path}"))

    # Скрытые конвенции пула (по выбранным), только применимые к subject_kind задачи.
    for conv in selected:
        if conv["subject_kind"] not in task["subject_kinds"]:
            continue
        viol.extend(_check_convention(conv, tree, task, src))

    return (not viol), viol


def _check_convention(conv, tree, task, src):
    cid = conv["id"]
    p = conv["params"]
    mod = task["module"]
    fn = task["func"]
    out = []
    if cid == "HC-01":  # обязательный __all__
        if not re.search(r"^__all__\s*=", src, re.M):
            out.append((cid, mod, "нет __all__"))
        else:
            for f in _public_funcs(src):
                if f not in src.split("__all__", 1)[1]:
                    out.append((cid, mod, f"{f} отсутствует в __all__"))
                    break
    elif cid == "HC-02":  # строка в каталоге операций
        cat = tree.get(p.get("catalog_file", "docs/OPERATIONS-CATALOG.md"), "")
        if f"| {fn} |" not in cat and f"`{mod}.{fn}`" not in cat:
            out.append((cid, mod, f"нет строки {fn} в {p.get('catalog_file', '')}"))
    elif cid == "HC-03":  # префикс логгера
        if f'logging.getLogger("PAYLIB.{mod.upper()}")' not in src:
            out.append((cid, mod, f"логгер не 'PAYLIB.{mod.upper()}'"))
    elif cid == "HC-04":  # направление зависимостей
        forbidden = p.get("forbid_import", "")
        if forbidden and (f"import {forbidden}" in src or f"from {forbidden}" in src):
            out.append((cid, mod, f"запрещённый импорт {forbidden}"))
    elif cid == "HC-05":  # CHANGELOG.d/<task-id>.md
        if f"CHANGELOG.d/{task['id']}.md" not in tree:
            out.append((cid, mod, f"нет CHANGELOG.d/{task['id']}.md"))
    elif cid == "HC-06":  # имя теста
        test_src = tree.get(f"tests/test_{mod}.py", "")
        bad = [f for f in _public_funcs(test_src)
               if not re.match(r"^test_[a-z0-9_]+__[a-z0-9_]+__[a-z0-9_]+$", f)]
        if bad:
            out.append((cid, mod, f"имя теста не по формату: {bad[0]}"))
    elif cid == "HC-07":  # декоратор аудита
        for f in _public_funcs(src):
            if not re.search(r"@audit_op\s*\n\s*def\s+" + re.escape(f) + r"\s*\(", src):
                out.append((cid, mod, f"{f} без @audit_op"))
                break
    elif cid == "HC-08":  # маршрут в openapi.yaml
        spec = tree.get(p.get("spec", "openapi.yaml"), "")
        if f"/{mod}" not in spec:
            out.append((cid, mod, f"маршрут /{mod} не в openapi.yaml"))
    elif cid == "HC-09":  # Decimal-фабрика
        if re.search(r"\bDecimal\s*\(", src):
            out.append((cid, mod, f"прямое Decimal(...); используйте {p.get('factory', '')}"))
    elif cid == "HC-10":  # запрет print()
        if re.search(r"\bprint\s*\(", src):
            out.append((cid, mod, "print() в библиотечном коде"))
    elif cid == "HC-11":  # docstring Google-формата
        if "Args:" not in src or "Returns:" not in src:
            out.append((cid, mod, "docstring без Args:/Returns:"))
    elif cid == "HC-12":  # OWNERS в новом пакете
        owners = tree.get(f"paylib/{mod}/OWNERS", tree.get("paylib/OWNERS", ""))
        if not re.search(r"^[A-Z]{3}-[0-9]{3}$", owners, re.M):
            out.append((cid, mod, "нет OWNERS с кодом владельца"))
    elif cid == "HC-13":  # tz-aware
        if re.search(r"datetime\.utcnow\s*\(", src):
            out.append((cid, mod, "naive datetime.utcnow()"))
    elif cid == "HC-14":  # idempotency_key
        if p.get("arg", "idempotency_key") not in src:
            out.append((cid, mod, f"нет параметра {p.get('arg', '')}"))
    return out


# ─────────────────────────────────────────────────────────────────────────────
# 8. Уроки и оркестратор (FakeLlm)
# ─────────────────────────────────────────────────────────────────────────────

class Lesson:
    def __init__(self, conv, scope, source):
        self.conv = conv
        self.scope = scope            # "paylib" | "repo"
        self.confirmations = 0
        self.misses = 0
        self.distinct_modules = set()
        self.status = "candidate"
        self.source = source

    def covers(self, task):
        if self.scope == "repo":
            return True
        return task["family"] != "migration"  # "paylib" покрывает только paylib-модули

    def applies(self, task):
        return self.covers(task) and self.conv["subject_kind"] in task["subject_kinds"]

    def confirm(self, task):
        self.confirmations += 1
        self.distinct_modules.add(task["module"])
        if self.confirmations >= 2 and len(self.distinct_modules) >= 2:
            self.scope = "repo"          # L6: расширение области по 2 разным модулям
        if self.confirmations >= PROMOTE_CONFIRMATIONS:
            self.status = "confirmed"


class Registry:
    """Реестр уроков (разделяемый между сессиями/исполнителями в FI+L, X, FLEET)."""

    def __init__(self):
        self.lessons = []  # list[Lesson]
        self.by_conv = {}

    def record(self, conv, task):
        if conv["id"] not in self.by_conv:
            lesson = Lesson(conv, "paylib", "gate")
            self.by_conv[conv["id"]] = lesson
            self.lessons.append(lesson)
        self.by_conv[conv["id"]].confirm(task)
        return self.by_conv[conv["id"]]

    def block_for(self, task):
        return [l for l in self.lessons if l.applies(task)]

    def promoted(self):
        return [l for l in self.lessons if l.status == "confirmed"]


class Orchestrator:
    """FakeLlm-оркестратор: диспетчеризует задачу, гонит гейт, учит уроки."""

    def __init__(self, selected, lessons_enabled=True):
        self.selected = selected
        self.lessons_enabled = lessons_enabled
        self.registry = Registry()
        self.template = load_template()

    def _taught(self, task):
        taught = {}
        for lesson in self.registry.block_for(task):
            taught[lesson.conv["id"]] = True
        return taught

    def _conv_by_id(self, cid):
        for c in self.selected:
            if c["id"] == cid:
                return c
        return None

    def run_task(self, task):
        """Возвращает dict с итогом одной задачи (FI+L/FI-путь: инъекция уроков)."""
        attempts = 0
        while attempts < MAX_ATTEMPTS:
            attempts += 1
            tree = fresh_repo(self.template)
            taught = self._taught(task)
            fake_executor(tree, task, taught)
            passed, violations = gate(tree, task, self.selected)
            if passed:
                return {"task": task["id"], "attempts": attempts, "pass": True,
                        "first_pass": attempts == 1, "precommit_caught": False,
                        "codes": []}
            if self.lessons_enabled:
                for code, _m, _msg in violations:
                    conv = self._conv_by_id(code)
                    if conv is not None:
                        self.registry.record(conv, task)
        return {"task": task["id"], "attempts": attempts, "pass": False,
                "first_pass": False, "precommit_caught": False,
                "codes": sorted({v[0] for v in violations})}


def _conv_missing(tree, task, conv):
    """Есть ли нарушение конвенции `conv` в дереве (для pre-commit-ловли)."""
    src = tree.get(module_path(task), "")
    return bool(_check_convention(conv, tree, task, src))


# ─────────────────────────────────────────────────────────────────────────────
# 9. Руки --ci
# ─────────────────────────────────────────────────────────────────────────────

def arm_fil(seed):
    """FI+L: реестр включён, чистое репо, свежая сессия; две серии."""
    selected = select_conventions(seed)
    tasks = family_tasks("module")
    orch = Orchestrator(selected, lessons_enabled=True)
    series1 = [orch.run_task(t) for t in tasks]
    series2 = [orch.run_task(t) for t in tasks]
    later1 = [r for r in series1 if r["task"] != "T1"]
    repeat = sum(1 for r in later1 if not r["first_pass"]) / max(1, len(later1))
    t1_series2_pass = series2[0]["pass"]
    # «прогонов на задачу после урока» — по T2–T5 (после того, как урок получен на T1).
    dispatches = sum(r["attempts"] for r in later1) / max(1, len(later1))
    return {
        "arm": "FI+L", "seed": seed, "selected": [c["id"] for c in selected],
        "repeat_failure_rate": repeat,
        "first_pass_t1_series2": 1.0 if t1_series2_pass else 0.0,
        "dispatches_per_task": dispatches,
        "lessons": [l.conv["id"] for l in orch.registry.lessons],
    }


def arm_fi(seed):
    """FI (контроль): реестр выключен — скрытые конвенции повторяются на каждой задаче."""
    selected = select_conventions(seed)
    tasks = family_tasks("module")
    orch = Orchestrator(selected, lessons_enabled=False)
    rows = [orch.run_task(t) for t in tasks]
    later = [r for r in rows if r["task"] != "T1"]
    repeat = sum(1 for r in later if not r["first_pass"]) / max(1, len(later))
    return {
        "arm": "FI", "seed": seed, "selected": [c["id"] for c in selected],
        "repeat_failure_rate": repeat,
        "dispatches_per_task": sum(r["attempts"] for r in rows) / len(rows),
    }


def arm_x(seed):
    """X (E2): серия 1 на исполнителе A, серия 2 на исполнителе B; реестр общий."""
    selected = select_conventions(seed)
    tasks = family_tasks("module")
    orch = Orchestrator(selected, lessons_enabled=True)
    series1 = [orch.run_task(t) for t in tasks]     # executor A (учит реестр)
    series2 = [orch.run_task(t) for t in tasks]     # executor B (читает реестр)
    transfer = sum(1 for r in series2 if r["pass"]) / max(1, len(series2))
    return {
        "arm": "X", "seed": seed, "selected": [c["id"] for c in selected],
        "transfer_rate": transfer,
        "executor_b_first_pass": sum(1 for r in series2 if r["first_pass"]) / len(series2),
    }


def arm_gen(seed):
    """GEN (E3): обучение на «модуль»+«CLI», проверка на отложенной «миграции»."""
    selected = select_conventions(seed)
    orch = Orchestrator(selected, lessons_enabled=True)
    train = family_tasks("module") + family_tasks("cli")
    for t in train:
        orch.run_task(t)
    migration = family_tasks("migration")
    rows = [orch.run_task(t) for t in migration]
    compliance = sum(1 for r in rows if r["pass"]) / max(1, len(rows))
    return {
        "arm": "GEN", "seed": seed, "selected": [c["id"] for c in selected],
        "migration_compliance": compliance,
        "repo_scope_lessons": [l.conv["id"] for l in orch.registry.lessons if l.scope == "repo"],
    }


def arm_pair(seed):
    """PAIR (E4): две конвенции одного subject_kind одновременно."""
    selected = select_conventions(seed)
    by_kind = {}
    for c in selected:
        by_kind.setdefault(c["subject_kind"], []).append(c)
    kind, pair = next(((k, v) for k, v in by_kind.items() if len(v) >= 2), (None, None))
    if pair is None:
        # если двух одного kind нет — берём первые две (композиция всё равно проверяется)
        pair = selected[:2]
        kind = pair[0]["subject_kind"]
    tasks = family_tasks("module")
    orch = Orchestrator(selected, lessons_enabled=True)
    rows = [orch.run_task(t) for t in tasks]
    later = [r for r in rows if r["task"] != "T1"]
    first_pass = sum(1 for r in later if r["first_pass"]) / max(1, len(later))
    return {
        "arm": "PAIR", "seed": seed, "selected": [c["id"] for c in selected],
        "pair": [c["id"] for c in pair], "subject_kind": kind,
        "first_pass_after_pair": first_pass,
    }


def arm_promo(seed):
    """PROMO (E5, L7): 10 задач; доля нарушений, пойманных pre-commit на 6–10.

    Фаза A (задачи 1–5): FI+L-путь — нарушение ловится гейтом, урок записывается
    и инжектируется исполнителю. Фаза B (задачи 6–10): урок продвинут в
    детерминированное pre-commit-правило; исполнитель пишет «сырой» код, а
    правило ловит нарушение ДО приёмки.
    """
    selected = select_conventions(seed)
    tasks = family_tasks("module") + family_tasks("cli") + family_tasks("module")[:1]
    tasks = [dict(t) for t in tasks]
    orch = Orchestrator(selected, lessons_enabled=True)
    for t in tasks[:5]:                       # фаза A: обучение через гейт
        orch.run_task(t)
    promoted = [l for l in orch.registry.lessons]  # все выученные → продвинуты (L7)
    caught = 0
    for t in tasks[5:]:                        # фаза B: pre-commit ловит до приёмки
        tree = fresh_repo(orch.template)
        fake_executor(tree, t, {})             # «сырой» исполнитель
        hits = [l for l in promoted if l.applies(t) and _conv_missing(tree, t, l.conv)]
        if hits:
            caught += 1
            fake_executor(tree, t, {l.conv["id"]: True for l in promoted})
        passed, _viol = gate(tree, t, selected)
        assert passed, f"PROMO фаза B: {t['id']} не прошла после pre-commit"
    tail = tasks[5:]
    return {
        "arm": "PROMO", "seed": seed, "selected": [c["id"] for c in selected],
        "precommit_caught_rate": caught / max(1, len(tail)),
        "n_tasks": len(tasks),
    }


def arm_fleet(seed):
    """FLEET (E6): 5 задач со сдвигом; повтор у стартовавших после первого FAIL."""
    selected = select_conventions(seed)
    tasks = family_tasks("module")[:5]
    # стартовые смещения: первая — сразу (t=0), остальные — через 60 c, т.е.
    # после того, как первая записала урок. Последовательность имитирует сдвиг.
    orch = Orchestrator(selected, lessons_enabled=True)
    first = orch.run_task(tasks[0])            # первая: FAIL на первой попытке → урок
    late = [orch.run_task(t) for t in tasks[1:]]
    repeat = sum(1 for r in late if not r["first_pass"]) / max(1, len(late))
    return {
        "arm": "FLEET", "seed": seed, "selected": [c["id"] for c in selected],
        "first_failed": not first["first_pass"],
        "late_repeat_rate": repeat,
        "late_tasks": len(late),
    }


ARMS = {
    "FI+L": arm_fil,
    "FI": arm_fi,
    "X": arm_x,
    "GEN": arm_gen,
    "PAIR": arm_pair,
    "PROMO": arm_promo,
    "FLEET": arm_fleet,
}


# ─────────────────────────────────────────────────────────────────────────────
# 10. Анти-Goodhart греп
# ─────────────────────────────────────────────────────────────────────────────

def antigoodhart_violations():
    """Возвращает список маркеров, найденных в src/ и assets/."""
    pool = load_pool()
    markers = [c["marker"] for c in pool if c.get("marker")]
    found = []
    for root in (os.path.join(REPO_ROOT, "src"), os.path.join(REPO_ROOT, "assets")):
        for dirpath, _dirs, files in os.walk(root):
            for fn in files:
                full = os.path.join(dirpath, fn)
                try:
                    text = _read(full)
                except (OSError, UnicodeDecodeError):
                    continue
                for marker in markers:
                    if marker and marker in text:
                        found.append((marker, os.path.relpath(full, REPO_ROOT)))
    return found


# ─────────────────────────────────────────────────────────────────────────────
# 11. Сводка и отчёт
# ─────────────────────────────────────────────────────────────────────────────

def run_ci(seed_map=None):
    seeds = seed_map if seed_map is not None else load_seeds()
    arms = [k for k in ("FI+L", "FI", "X", "GEN", "PAIR", "PROMO", "FLEET") if k in seeds]
    results = {}
    for arm in arms:
        seed = seeds[arm][0]
        results[arm] = ARMS[arm](seed)
    ag = antigoodhart_violations()
    n_markers = len([c for c in load_pool() if c.get("marker")])
    # греп не должен быть вакуумным: маркеры обязаны существовать в пуле.
    ag_passed = bool(not ag and n_markers >= 12)
    summary = {
        "mode": "ci",
        "arms": results,
        "antigoodhart": {"passed": ag_passed, "violations": ag, "markers": n_markers},
    }
    return summary


# ─────────────────────────────────────────────────────────────────────────────
# 12. CLI
# ─────────────────────────────────────────────────────────────────────────────

def _print_summary(summary):
    print(json.dumps(summary, ensure_ascii=False, indent=2, sort_keys=True))


def main(argv):
    args = argv[1:]
    if not args:
        print(__doc__)
        return 2

    if args[0] == "--check-antigoodhart":
        viol = antigoodhart_violations()
        n_markers = len([c for c in load_pool() if c.get("marker")])
        if viol or n_markers < 12:
            for marker, path in viol:
                print(f"[GOODHART] {marker} в {path}")
            if n_markers < 12:
                print(f"[GOODHART] маркеров в пуле: {n_markers} (< 12) — греп вакуумен")
            print("анти-Goodhart: FAIL")
            return 1
        print(f"анти-Goodhart: PASS ({n_markers} маркеров, не в src/ assets/)")
        return 0

    if args[0] == "--gen":
        seed = int(args[1])
        selected = select_conventions(seed)
        print(json.dumps({"seed": seed,
                          "conventions": [c["id"] for c in selected],
                          "tasks": {"module": [t["id"] for t in family_tasks("module")],
                                    "cli": [t["id"] for t in family_tasks("cli")],
                                    "migration": [t["id"] for t in family_tasks("migration")]}},
                         ensure_ascii=False, indent=2))
        return 0

    if args[0] == "--ci":
        summary = run_ci()
        _print_summary(summary)
        out_dir = None
        if "--out" in args:
            out_dir = args[args.index("--out") + 1]
        if out_dir:
            os.makedirs(out_dir, exist_ok=True)
            with open(os.path.join(out_dir, "ci-summary.json"), "w", encoding="utf-8") as fh:
                json.dump(summary, fh, ensure_ascii=False, indent=2, sort_keys=True)
        # exit 0, если все руки отработали и анти-Goodhart чист
        return 0 if summary["antigoodhart"]["passed"] else 1

    if args[0] == "--ci-arm":
        arm = args[1]
        seed = int(args[2]) if len(args) > 2 else load_seeds()[arm][0]
        print(json.dumps(ARMS[arm](seed), ensure_ascii=False, indent=2, sort_keys=True))
        return 0

    if args[0] == "--self-test":
        return _self_test()

    print(f"неизвестная команда: {args[0]}")
    return 2


def _self_test():
    pool = load_pool()
    assert len(pool) == 14, f"пул: {len(pool)}"
    holdout = load_holdout_ids()
    assert len(holdout) == 4, f"holdout: {holdout}"
    # детерминизм выбора
    a = select_conventions(141)
    b = select_conventions(141)
    assert [c["id"] for c in a] == [c["id"] for c in b], "выбор недетерминирован"
    assert 2 <= len(a) <= 3, f"число конвенций: {len(a)}"
    assert not (set(c["id"] for c in a) & holdout), "выбрана holdout-конвенция"
    # видимое правило + скрытая конвенция ловятся гейтом
    selected = [c for c in pool if c["id"] == "HC-01"]
    task = family_tasks("module")[0]
    tree = fresh_repo(load_template())
    fake_executor(tree, task, {})
    passed, viol = gate(tree, task, selected)
    assert not passed and any(v[0] == "HC-01" for v in viol), f"HC-01 не поймана: {viol}"
    # наученный исполнитель проходит
    tree2 = fresh_repo(load_template())
    fake_executor(tree2, task, {"HC-01": True})
    passed2, viol2 = gate(tree2, task, selected)
    assert passed2, f"наученный не прошёл: {viol2}"
    print("self-test: PASS")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
