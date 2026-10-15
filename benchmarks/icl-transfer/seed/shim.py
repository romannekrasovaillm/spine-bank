#!/usr/bin/env python3
"""OpenAI-compatible /v1/chat/completions shim over the authorized `claude -p` CLI.

Lets Spine Harness run its own agent loop (with tool calls) when no
DeepSeek/GLM/Kimi key is available. The LLM sees the tool specs as text and
returns a structured decision (content + tool_calls) via --json-schema.
Every request/response is logged to $SHIM_LOG (jsonl) for analysis.
"""
import json, os, subprocess, sys, time, uuid, threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

MODEL = os.environ.get("SHIM_MODEL", "sonnet")
LOG = os.environ.get("SHIM_LOG", "/home/user/lab/shim-log.jsonl")
lock = threading.Lock()

SCHEMA = {
    "type": "object",
    "properties": {
        "content": {"type": "string", "description": "Text for the user (may be empty if calling tools)."},
        "tool_calls": {
            "type": "array",
            "items": {
                "type": "object",
                "properties": {
                    "name": {"type": "string"},
                    "arguments": {"type": "string", "description": "JSON object string with the tool arguments"},
                },
                "required": ["name", "arguments"],
            },
        },
    },
    "required": ["content", "tool_calls"],
}

SYS = """You are the decision-making LLM inside an agent harness (like the model behind a
function-calling API). You have NO built-in tools of your own in this process — that is
expected. Instead, the harness gives you a list of TOOLS that IT will execute for you:
to use a tool, emit it in `tool_calls` (name + arguments as a JSON-object string that
matches the tool's parameters schema). The harness runs the calls and returns results in
the next request as [tool result] entries. So you CAN read files, run bash, call coding
agents, etc. — by emitting tool_calls. Never claim tools are unavailable.
When the task is finished (or you need the user), return tool_calls = [] and put the final
answer in `content`. Emit several independent tool_calls at once when useful.
Follow the HARNESS SYSTEM PROMPT below faithfully: it defines your role and discipline.

OUTPUT FORMAT (strict): reply with ONE JSON object and nothing else, no prose, no code fences:
{"content": "<text>", "tool_calls": [{"name": "<tool>", "arguments": "<JSON object as a string>"}]}
"arguments" is a STRING containing JSON (escape quotes). Do not attempt to invoke any
function yourself — just write the JSON."""


def flatten(body):
    out = []
    tools = body.get("tools") or []
    msgs = body.get("messages") or []
    for m in msgs:
        if m.get("role") == "system":
            out.append("### HARNESS SYSTEM PROMPT\n" + str(m.get("content") or ""))
    out.append("### AVAILABLE TOOLS (%d)" % len(tools))
    for t in tools:
        f = t.get("function", t)
        out.append("- %s: %s\n  params: %s" % (
            f.get("name"), (f.get("description") or "").strip(),
            json.dumps(f.get("parameters", {}), ensure_ascii=False, separators=(",", ":"))))
    out.append("### CONVERSATION")
    for m in msgs:
        r = m.get("role")
        if r == "system":
            continue
        c = m.get("content")
        if isinstance(c, list):
            c = "\n".join(p.get("text", "") for p in c if isinstance(p, dict))
        if r == "assistant":
            s = "[assistant]\n" + (c or "")
            for tc in m.get("tool_calls") or []:
                fn = tc.get("function", {})
                s += "\n<call id=%s> %s %s" % (tc.get("id"), fn.get("name"), fn.get("arguments"))
            out.append(s)
        elif r == "tool":
            out.append("[tool result id=%s]\n%s" % (m.get("tool_call_id"), c or ""))
        else:
            out.append("[%s]\n%s" % (r, c or ""))
    out.append("### YOUR NEXT STEP (structured output)")
    return "\n\n".join(out)


import re
INV = re.compile(r'<invoke name="([^"]+)">(.*?)</invoke>', re.S)
PAR = re.compile(r'<parameter name="([^"]+)">(.*?)</parameter>', re.S)


def coerce(v, typ):
    try:
        if typ in ("integer", "number", "boolean", "array", "object"):
            return json.loads(v)
    except Exception:
        pass
    return v


def parse_reply(txt, tools):
    """Robust: prefer a JSON object {"content","tool_calls"}; else XML <invoke> with known tool names."""
    known = {(t.get("function", t)).get("name") for t in tools}
    dec = json.JSONDecoder()
    for m in re.finditer(r'\{\s*"(content|tool_calls)"', txt):
        try:
            so, _ = dec.raw_decode(txt[m.start():])
            if isinstance(so, dict) and isinstance(so.get("tool_calls", []), list):
                so.setdefault("content", "")
                so.setdefault("tool_calls", [])
                if so["content"] or so["tool_calls"]:
                    return so
        except Exception:
            continue
    schemas = {}
    for t in tools:
        f = t.get("function", t)
        schemas[f.get("name")] = (f.get("parameters") or {}).get("properties", {})
    calls = []
    for name, a in re.findall(r"<tool_call>\s*<name>([^<]+)</name>\s*<arguments>(.*?)</arguments>", txt, re.S):
        name = name.strip()
        if name in known:
            try:
                json.loads(a)
                calls.append({"name": name, "arguments": a.strip()})
            except Exception:
                pass
    for name, inner in INV.findall(txt):
        if name not in known:
            continue
        props = schemas.get(name, {})
        args = {k: coerce(v, props.get(k, {}).get("type")) for k, v in PAR.findall(inner)}
        calls.append({"name": name, "arguments": json.dumps(args, ensure_ascii=False)})
    content = re.sub(r"<(invoke|tool_calls|tool_call).*", "", txt, flags=re.S).strip()
    if not calls and not content:
        raise ValueError("empty reply")
    if not calls and content.startswith("{"):
        raise ValueError("unparsed json")
    return {"content": content, "tool_calls": calls}


def ask_claude(prompt, tools=()):
    cmd = ["claude", "-p", "--model", MODEL, "--output-format", "json",
           "--system-prompt", SYS, "--tools", "", "--strict-mcp-config",
           "--no-session-persistence"]
    t0 = time.time()
    for attempt in range(3):
        p = subprocess.run(cmd, input=prompt, capture_output=True, text=True, timeout=900,
                           cwd="/tmp")
        try:
            env = json.loads(p.stdout)
            txt = (env.get("result") or "").strip()
            so = parse_reply(txt, tools)
            usage = env.get("usage", {})
            so["_raw"] = txt[:3000]
            return so, usage, time.time() - t0
        except Exception as e:  # retry
            err = (p.stdout[-500:], p.stderr[-500:], str(e))
            time.sleep(3)
    return {"content": "shim error: %r" % (err,), "tool_calls": []}, {}, time.time() - t0


class H(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass

    def do_POST(self):
        n = int(self.headers.get("content-length", 0))
        body = json.loads(self.rfile.read(n))
        prompt = flatten(body)
        so, usage, dt = ask_claude(prompt, body.get("tools") or [])
        calls = []
        known = {(t.get("function", t)).get("name") for t in body.get("tools") or []}
        seen = set()
        for i, tc in enumerate(so.get("tool_calls") or []):
            args = tc.get("arguments") or "{}"
            if not isinstance(args, str):
                args = json.dumps(args, ensure_ascii=False)
            try:  # unwrap {"arguments": "<json>"} artefact
                a = json.loads(args)
                if isinstance(a, dict) and list(a) == ["arguments"]:
                    inner = a["arguments"]
                    args = inner if isinstance(inner, str) else json.dumps(inner, ensure_ascii=False)
            except Exception:
                pass
            key = (tc.get("name"), args)
            if tc.get("name") not in known or key in seen:
                continue
            seen.add(key)
            calls.append({"index": len(calls), "id": "call_" + uuid.uuid4().hex[:12], "type": "function",
                          "function": {"name": tc.get("name"), "arguments": args}})
        content = so.get("content") or ""
        pt = usage.get("input_tokens", 0) + usage.get("cache_read_input_tokens", 0) + usage.get("cache_creation_input_tokens", 0)
        ct = usage.get("output_tokens", 0)
        with lock, open(LOG, "a") as f:
            f.write(json.dumps({"ts": time.time(), "run": os.environ.get("SHIM_RUN", ""),
                                "n_msgs": len(body.get("messages", [])), "n_tools": len(body.get("tools") or []),
                                "prompt_chars": len(prompt), "latency": round(dt, 1),
                                "prompt_tokens": pt, "completion_tokens": ct,
                                "calls": [c["function"]["name"] for c in calls],
                                "content": content[:400], "raw": so.get("_raw", "")}, ensure_ascii=False) + "\n")
        finish = "tool_calls" if calls else "stop"
        cid = "chatcmpl-" + uuid.uuid4().hex[:10]
        u = {"prompt_tokens": pt, "completion_tokens": ct, "total_tokens": pt + ct}
        if body.get("stream"):
            self.send_response(200)
            self.send_header("content-type", "text/event-stream")
            self.end_headers()
            def ev(o):
                self.wfile.write(("data: " + json.dumps(o, ensure_ascii=False) + "\n\n").encode())
            delta = {"role": "assistant", "content": content}
            if calls:
                delta["tool_calls"] = calls
            ev({"id": cid, "object": "chat.completion.chunk", "model": MODEL,
                "choices": [{"index": 0, "delta": delta, "finish_reason": None}]})
            ev({"id": cid, "object": "chat.completion.chunk", "model": MODEL,
                "choices": [{"index": 0, "delta": {}, "finish_reason": finish}]})
            ev({"id": cid, "object": "chat.completion.chunk", "model": MODEL, "choices": [], "usage": u})
            self.wfile.write(b"data: [DONE]\n\n")
        else:
            msg = {"role": "assistant", "content": content}
            if calls:
                msg["tool_calls"] = [{k: v for k, v in c.items() if k != "index"} for c in calls]
            data = json.dumps({"id": cid, "object": "chat.completion", "model": MODEL,
                               "choices": [{"index": 0, "message": msg, "finish_reason": finish}],
                               "usage": u}, ensure_ascii=False).encode()
            self.send_response(200)
            self.send_header("content-type", "application/json")
            self.send_header("content-length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)


if __name__ == "__main__":
    port = int(sys.argv[1]) if len(sys.argv) > 1 else 18080
    ThreadingHTTPServer(("127.0.0.1", port), H).serve_forever()
