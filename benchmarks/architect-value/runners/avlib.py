#!/usr/bin/env python3
"""avlib.py — общий слой прогонщиков architect-value.

Только stdlib (никаких PyYAML/requests): разбор yaml-подмножества замороженных
входов P1 (ground_truth/, standard/, seeds/, labels/) плюс мелкие хелперы для
запуска `arch-be` и определения путей клонов.

yaml-подмножество: отступные отображения/последовательности, скаляры
(числа/bool/null/строки с '..' и ".."), inline-списки `[a, b]`, inline-карты
`{k: v}`, блочные скаляры (`>`, `>-`, `|`, `|-`). Этого достаточно для всех
файлов бенчмарка — полноценный YAML осознанно не тянем (заморозка входов P1).

Пути клонов берутся из ARCH_VALUE_REPOS / --repos-dir (домашний путь не зашит).
Прогон без сети: сетевые/отсутствующие механики честно уходят в SKIP.
"""
import json
import os
import re
import subprocess
import sys
from pathlib import Path

BASE = Path(__file__).resolve().parent.parent          # benchmarks/architect-value
REPOS_LOCK = BASE / "repos.lock"
GROUND_TRUTH = BASE / "ground_truth"
STANDARD = BASE / "standard"
SEEDS = BASE / "seeds"
LABELS = BASE / "labels"

# Логическое имя репо B (как в ground_truth/ и labels/) → имя каталога клона
# (последний сегмент url из repos.lock). Имена расходятся: `boutique` — это
# GoogleCloudPlatform/microservices-demo, `petclinic` — spring-petclinic-microservices.
REPO_CLONES = {
    "boutique": "microservices-demo",
    "petclinic": "spring-petclinic-microservices",
    "buckpal": "buckpal",
    "library": "library",
}
# Клоны выше не имеют логического имени, совпадающего с именем каталога —
# обратный словарь нужен детекторам «есть ли клон для репо».
CLONE_REPOS = {v: k for k, v in REPO_CLONES.items()}

# Путь к бинарю arch-be и корень клонов — из окружения, дефолты без дома.
ARCH_BE = os.environ.get("ARCH_BE", "arch-be")
DEFAULT_REPOS_DIR = os.environ.get("ARCH_VALUE_REPOS", "~/bench-repos")
DEFAULT_TIMEOUT = int(os.environ.get("ARCH_VALUE_TIMEOUT", "120"))


# ── yaml-подмножество ────────────────────────────────────────────────────────
def _split_comment(line):
    out, q, i = [], None, 0
    while i < len(line):
        c = line[i]
        if q:
            out.append(c)
            if c == "\\" and q == '"' and i + 1 < len(line):
                out.append(line[i + 1])
                i += 2
                continue
            if c == q:
                q = None
        elif c in "'\"":
            q = c
            out.append(c)
        elif c == "#" and (not out or out[-1] in " \t"):
            break
        else:
            out.append(c)
        i += 1
    return "".join(out)


def _unquote(s):
    s = s.strip()
    if len(s) >= 2 and s[0] == "'" and s[-1] == "'":
        return s[1:-1].replace("''", "'")
    if len(s) >= 2 and s[0] == '"' and s[-1] == '"':
        body, out, i = s[1:-1], [], 0
        esc = {"n": "\n", "t": "\t", "r": "\r", "\\": "\\", '"': '"', "'": "'", "0": "\0"}
        while i < len(body):
            if body[i] == "\\" and i + 1 < len(body):
                out.append(esc.get(body[i + 1], body[i + 1]))
                i += 2
            else:
                out.append(body[i])
                i += 1
        return "".join(out)
    return s


def _split_top(s):
    """Разбить по запятым верхнего уровня (учитывая [] {} и кавычки)."""
    parts, cur, depth, q, i = [], [], 0, None, 0
    while i < len(s):
        c = s[i]
        if q:
            cur.append(c)
            if c == "\\" and q == '"' and i + 1 < len(s):
                cur.append(s[i + 1])
                i += 2
                continue
            if c == q:
                q = None
        elif c in "'\"":
            q = c
            cur.append(c)
        elif c in "[{":
            depth += 1
            cur.append(c)
        elif c in "]}":
            depth -= 1
            cur.append(c)
        elif c == "," and depth == 0:
            parts.append("".join(cur))
            cur = []
        else:
            cur.append(c)
        i += 1
    if cur:
        parts.append("".join(cur))
    return parts


def parse_scalar(s):
    s = s.strip()
    if s.startswith("[") and s.endswith("]"):
        inner = s[1:-1].strip()
        return [] if inner == "" else [parse_scalar(x) for x in _split_top(inner)]
    if s.startswith("{") and s.endswith("}"):
        inner = s[1:-1].strip()
        d = {}
        if inner:
            for part in _split_top(inner):
                k, _, v = part.partition(":")
                d[parse_scalar(k)] = parse_scalar(v) if v.strip() else None
        return d
    if len(s) >= 2 and s[0] == s[-1] and s[0] in "'\"":
        return _unquote(s)
    if s in ("true", "True"):
        return True
    if s in ("false", "False"):
        return False
    if s in ("null", "~", ""):
        return None
    if re.fullmatch(r"-?\d+", s):
        return int(s)
    if re.fullmatch(r"-?\d+\.\d+", s):
        return float(s)
    return s


def _is_keyval(body):
    if not body or body[0] in "-[{" :
        return False
    return body.find(":") > 0


def _rows(text):
    out = []
    for raw in text.splitlines():
        s = _split_comment(raw).rstrip()
        if not s.strip():
            continue
        indent = len(s) - len(s.lstrip(" "))
        out.append((indent, s.strip()))
    return out


def _assign(m, keyval, rows, i, indent):
    key, _, rest = keyval.partition(":")
    key = parse_scalar(key.strip())
    rest = rest.strip()
    if rest in (">", ">-", ">+", "|", "|-", "|+"):
        buf = []
        while i < len(rows) and rows[i][0] > indent:
            buf.append(rows[i][1])
            i += 1
        m[key] = (" " if rest.startswith(">") else "\n").join(buf)
    elif rest == "":
        if i < len(rows) and rows[i][0] > indent:
            val, i = _parse_block(rows, i)
            m[key] = val
        else:
            m[key] = None
    else:
        m[key] = parse_scalar(rest)
    return i


def _parse_map_into(m, rows, i, indent):
    while i < len(rows) and rows[i][0] == indent and _is_keyval(rows[i][1]):
        i = _assign(m, rows[i][1], rows, i + 1, indent)
    return i


def _parse_seq(rows, i, indent):
    out = []
    while i < len(rows) and rows[i][0] == indent and (
        rows[i][1] == "-" or rows[i][1].startswith("- ")
    ):
        body = rows[i][1]
        stripped_after = body[1:].lstrip()
        content = stripped_after
        content_col = indent + (len(body) - len(stripped_after))
        i += 1
        if content == "":
            if i < len(rows) and rows[i][0] > indent:
                val, i = _parse_block(rows, i)
            else:
                val = None
            out.append(val)
        elif _is_keyval(content):
            m = {}
            i = _assign(m, content, rows, i, content_col)
            i = _parse_map_into(m, rows, i, content_col)
            out.append(m)
        else:
            out.append(parse_scalar(content))
    return out, i


def _parse_block(rows, i):
    indent = rows[i][0]
    body = rows[i][1]
    if body == "-" or body.startswith("- "):
        return _parse_seq(rows, i, indent)
    m = {}
    i = _parse_map_into(m, rows, i, indent)
    return m, i


def parse_yaml(text):
    """Разбор yaml-подмножества → python-объект."""
    rows = _rows(text)
    if not rows:
        return None
    val, _ = _parse_block(rows, 0)
    return val


def load_yaml(path):
    return parse_yaml(Path(path).read_text(encoding="utf-8"))


# ── запуск arch-be ───────────────────────────────────────────────────────────
class Result:
    def __init__(self, code, out, err):
        self.code, self.out, self.err = code, out, err

    @property
    def ok(self):
        return self.code == 0

    def json(self):
        return json.loads(self.out)


def run_archbe(args, cwd=None, timeout=None, no_exec=True):
    env = dict(os.environ)
    if no_exec:
        env.setdefault("ARCH_NO_EXEC", "1")
    try:
        p = subprocess.run(
            [ARCH_BE, *args], cwd=cwd, env=env, capture_output=True, text=True,
            timeout=timeout or DEFAULT_TIMEOUT,
        )
        return Result(p.returncode, p.stdout, p.stderr)
    except FileNotFoundError:
        return Result(127, "", f"arch-be не найден: {ARCH_BE}")
    except subprocess.TimeoutExpired:
        return Result(124, "", f"timeout {timeout or DEFAULT_TIMEOUT}s")


def archbe_version():
    r = run_archbe(["--version"], no_exec=False, timeout=20)
    return (r.out or r.err).strip() or "unknown"


# ── клоны ────────────────────────────────────────────────────────────────────
def load_repos_lock():
    """{name: {url, sha, mirror}} из repos.lock."""
    out = {}
    for line in REPOS_LOCK.read_text(encoding="utf-8").splitlines():
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        parts = line.split()
        if len(parts) < 3:
            continue
        url, sha, mirror = parts[0], parts[1], parts[2]
        name = url.rstrip("/").rsplit("/", 1)[-1]
        out[name] = {"url": url, "sha": sha, "mirror": mirror}
    return out


def repos_dir(arg=None):
    """Корень клонов: --repos-dir / ARCH_VALUE_REPOS / ~/bench-repos."""
    base = arg or os.environ.get("ARCH_VALUE_REPOS") or os.path.expanduser("~/bench-repos")
    return Path(base).expanduser()


def clone_path(name, repos_root=None):
    root = repos_dir(repos_root)
    # имя каталога клона = последний сегмент url-пути (из repos.lock mirror).
    lock = load_repos_lock()
    if name in lock:
        mirror = Path(os.path.expanduser(lock[name]["mirror"]))
        if mirror.is_dir():
            return mirror
        alt = root / mirror.name
        if alt.is_dir():
            return alt
    cand = root / name
    return cand if cand.is_dir() else None


def repo_clone_dir(repo, repos_root=None):
    """Каталог клона по логическому имени репо (boutique/petclinic/buckpal/library).

    None, если клона нет (прогонщик обязан выдать SKIP, а не упасть)."""
    return clone_path(REPO_CLONES.get(repo, repo), repos_root)


# ── отчётность ───────────────────────────────────────────────────────────────
def emit_section(title):
    print(f"\n## {title}")


def emit_run(metric, detail):
    print(f"[RUN] {metric}: {detail}")


def emit_skip(metric, reason):
    print(f"[SKIP] {metric}: {reason}")


def eprint(*a):
    print(*a, file=sys.stderr)
