#!/usr/bin/env python3
"""mcp_schema_probe.py — метрики 15, 16 (§9): расхождения схема↔реализация MCP.

Метрика 15 — «расхождения схема ↔ реализация»: инструмент объявляет в
`inputSchema` одно, а реализация читает другое имя аргумента. Класс дефекта —
`reverse_survey` на 0.3.16 (схема требовала `path`, реализация читала `dir`).
Проба: поднять `arch-be mcp serve`, взять `tools/list`, для каждого инструмента
позвать `tools/call` РОВНО с аргументами из `required` (заглушки), и пометить
расхождением случай, когда ответ-ошибка называет аргумент, которого НЕТ в
`properties` (реализация требует необъявленное поле), либо `required ⊄ properties`
(структурно сломанная схема). На 0.3.17 — 0 расхождений.

Метрика 16 — есть ли у агента инструменты восстановления/диффа (`recover`,
`arch_diff`). На 0.3.17 их нет (baseline); цель 0.4.0 — есть.

Транспорт — stdio JSON-RPC NDJSON (stdlib: subprocess + json). Прогон без сети;
`ARCH_NO_EXEC=1` в окружении сервера. Сервер запускается в temp-cwd, чтобы любые
побочные записи не касались репозитория. Exit 0 всегда (измерение); отсутствие
бинарника — честный SKIP.
"""
import json
import os
import re
import subprocess
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import avlib  # noqa: E402

_PROTO = "2024-11-05"
# Маркер «реализация требует аргумент» И имя этого аргумента — СРАЗУ за маркером.
# Отделяет расхождение схемы (реализация читает иное имя, как reverse_survey 0.3.16:
# схема `path`, реализация `dir`) от обычного отказа исполнения (напр. «скилл 'x'
# не найден»: 'x' — ЗНАЧЕНИЕ заглушки) и от упоминания исторического АЛИАСА
# («требуется 'path', историческое имя 'repo'»), где нужное имя объявлено схемой.
_FIELD_REQ_RE = re.compile(
    r"(?:обязательн\w*\s+аргумент|отсутству\w*\s+аргумент|"
    r"не\s+указан\w*\s+(?:аргумент|поле)|"
    r"укажите\s+(?:ровно\s+)?один\s+из\s+аргумент\w*|"
    r"missing[\s_](?:field|argument)|unknown[\s_](?:field|argument)|"
    r"required[\s_]argument)"
    r"[\s:]*[`'\"]?([a-z_][a-z0-9_]*)?",
    re.IGNORECASE,
)
# Серверы: read-only (40 инструментов) и rw (мутации/обратный обследователь).
SERVERS = [("serve", []), ("serve --rw", ["--rw"])]
RECOVERY_TOOLS = ["recover", "arch_diff"]


class McpSession:
    """Минимальный NDJSON-клиент над `arch-be mcp serve`."""

    def __init__(self, extra_args, cwd):
        env = dict(os.environ)
        env.setdefault("ARCH_NO_EXEC", "1")
        self.p = subprocess.Popen(
            [avlib.ARCH_BE, "mcp", "serve", *extra_args],
            cwd=cwd, env=env, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
            stderr=subprocess.PIPE, text=True,
        )
        self._id = 0

    def _send(self, obj):
        self.p.stdin.write(json.dumps(obj) + "\n")
        self.p.stdin.flush()

    def _read(self):
        line = self.p.stdout.readline()
        return json.loads(line) if line.strip() else None

    def request(self, method, params):
        self._id += 1
        self._send({"jsonrpc": "2.0", "id": self._id, "method": method, "params": params})
        return self._read()

    def notify(self, method, params):
        self._send({"jsonrpc": "2.0", "method": method, "params": params})

    def initialize(self):
        r = self.request("initialize", {
            "protocolVersion": _PROTO, "capabilities": {},
            "clientInfo": {"name": "architect-value", "version": "0"},
        })
        self.notify("notifications/initialized", {})
        return r

    def list_tools(self):
        r = self.request("tools/list", {})
        return r.get("result", {}).get("tools", []) if r else []

    def call(self, name, arguments):
        return self.request("tools/call", {"name": name, "arguments": arguments})

    def close(self):
        try:
            self.p.terminate()
            self.p.wait(timeout=5)
        except Exception:
            self.p.kill()


def _dummy(schema):
    if "enum" in schema:
        return schema["enum"][0]
    return {"string": "x", "integer": 0, "number": 0,
            "boolean": False, "array": [], "object": {}}.get(schema.get("type", "string"), "x")


def _err_text(resp):
    """Текст ошибки из ответа tools/call (result.content[].text либо error.message)."""
    if not resp:
        return ""
    if resp.get("error"):
        return str(resp["error"].get("message", resp["error"]))
    res = resp.get("result", {})
    if isinstance(res, dict) and res.get("isError"):
        parts = [c.get("text", "") for c in res.get("content", []) if isinstance(c, dict)]
        return " ".join(parts)
    return ""


def probe_server(label, extra_args, cwd):
    """→ (divergences:list[str], names:list[str], error|None)."""
    try:
        s = McpSession(extra_args, cwd)
    except FileNotFoundError:
        return None, None, f"arch-be не найден: {avlib.ARCH_BE}"
    try:
        if not s.initialize():
            return None, None, f"{label}: нет ответа на initialize"
        tools = s.list_tools()
        names = [t["name"] for t in tools]
        div = []
        for t in tools:
            sch = t.get("inputSchema", {}) or {}
            props = sch.get("properties", {}) or {}
            required = sch.get("required", []) or []
            # 1) структурно сломанная схема: required называет необъявленное поле.
            for r in required:
                if r not in props:
                    div.append(f"{label}:{t['name']}: required '{r}' ∉ properties")
            # 2) поведенческое: зовём ровно с required-заглушками; расхождение —
            #    когда ответ помечен «требуется аргумент 'X'», но X схема НЕ
            #    объявляет (реализация читает иное имя, как reverse_survey 0.3.16:
            #    схема `path`, реализация `dir`).
            args = {k: _dummy(props.get(k, {})) for k in required}
            text = _err_text(s.call(t["name"], args))
            for m in _FIELD_REQ_RE.finditer(text):
                field = m.group(1)
                if field and field not in props and field not in required:
                    div.append(f"{label}:{t['name']}: реализация требует '{field}' ∉ properties")
        return div, names, None
    finally:
        s.close()


def main(repos_root=None):
    avlib.emit_section("MCP: схема ↔ реализация — метрики 15, 16")
    v = avlib.archbe_version()
    if "unknown" in v.lower() and not Path(avlib.ARCH_BE).exists():
        avlib.emit_skip("metric 15/16", f"бинарник arch-be недоступен ({avlib.ARCH_BE})")
        return 0

    all_div, all_names, measured = [], set(), 0
    with tempfile.TemporaryDirectory(prefix="mcp_probe_") as td:
        for label, extra in SERVERS:
            div, names, err = probe_server(label, extra, td)
            if err:
                avlib.emit_skip(f"metric 15/16 [{label}]", err)
                continue
            measured += 1
            all_div += div
            all_names |= set(names)
            avlib.emit_run(f"metric 15 [{label}]", f"tools={len(names)} divergences={len(div)}")

    if measured == 0:
        avlib.emit_skip("metric 15/16", "ни один MCP-сервер не поднялся")
        return 0

    for d in all_div:
        print(f"[FIND] {d}")
    avlib.emit_run(
        "metric 15",
        f"divergences={len(all_div)} (baseline=1 reverse_survey, target=0)"
        + (f"; servers={'/'.join(l for l, _ in SERVERS)}"),
    )
    present = [t for t in RECOVERY_TOOLS if t in all_names]
    missing = [t for t in RECOVERY_TOOLS if t not in all_names]
    avlib.emit_run(
        "metric 16",
        f"recovery_tools={present or 'нет'} missing={missing} "
        f"(baseline: нет инструментов, target: recover+arch_diff)",
    )
    return 0


if __name__ == "__main__":
    root = None
    args = sys.argv[1:]
    if args and args[0] == "--repos-dir" and len(args) > 1:
        root = args[1]
    sys.exit(main(root))
