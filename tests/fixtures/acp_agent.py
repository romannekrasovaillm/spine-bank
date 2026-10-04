#!/usr/bin/env python3
"""Фикстура-«агент» ACP для юнит-тестов arch-harness (ADR-057, C2).

Самодостаточна: только стандартная библиотека, сети не требует, слушает
newline-delimited JSON-RPC 2.0 на stdin/stdout. Поведение выбирается первым
аргументом (режим):

  ok          initialize → session/new → session/prompt; стрим чанков с
              tool_call между ними, финальный текст — после tool_call;
  plain       то же, но без tool_call (весь текст — финальный);
  permission  session/request_permission (allow_once + reject_once) и эхо
              выбранной опции в финальный текст;
  multi-message два агентских сообщения с РАЗНЫМИ messageId и tool_call
              между ними — финал = второе сообщение (F1);
  auth        initialize с непустым authMethods БЕЗ authenticate-cap —
              информационный список, сессия продолжается штатно (F2);
  auth-required initialize с непустым authMethods И authenticate-cap —
              клиент обязан отказаться от сессии (F2);
  unknown-update session/update с выдуманным sessionUpdate — прогон жив, в
              журнале запись (F3);
  perm-cancel ждёт допуск и молчит; по session/cancel просит допуск ещё раз
              — клиент в фазе отмены обязан ответить cancelled (F4);
  close       initialize с cap sessionCapabilities.close; после end_turn
              отвечает на session/close и пишет маркер (F5);
  fs          клиентский метод fs/read_text_file — эхо кода ошибки;
  env         эхо наличия переменной ARCH_ACP_LEAK (проверка env-политики);
  cancel      после промпта ждёт session/cancel, пишет маркер и отвечает
              stopReason=cancelled;
  cancelled-after-cancel синоним `cancel`: мягкая отмена, подтверждённая
              агентом после нашего session/cancel (срез «полный stopReason»);
  limit-tokens      частичный ответ + stopReason=max_tokens (обрыв хода по
              лимиту токенов): контракт partial должен дойти до разбора;
  limit-turns       частичный ответ + stopReason=max_turn_requests;
  refusal     текст отказа + stopReason=refusal (ошибка прогона сохранена);
  idle        один чанк и молчание (idle-детект клиента);
  active      частые чанки дольше idle-окна — прерывать нельзя;
  init-exit   ранний выход до ответа на initialize;
  init-silent молчание на initialize (таймаут инициализации).

Маркер отмены пишется в CWD процесса — каталог прогона, который назначает
клиент (`session/new.cwd`).

Конформность (TCK v1, провалы фикстуры ACP-EXT-001 / ACP-SESSION-002 /
ACP-ERROR-001): фикстура не молчит на «лишние» запросы — request с
неизвестным методом получает JSON-RPC error `-32601` с непустым
однострочным message, а каждый `session/new` — свежий уникальный
`sessionId` (последний выданный становится активным). Уведомления без id
(в т.ч. `session/cancel`) ответа не получают.
"""

import json
import os
import sys
import time

CONTRACT = '```json\n{"status": "complete", "assumptions": [], "open_questions": [], "conflicts_with_prior_decisions": []}\n```'
PARTIAL_CONTRACT = '```json\n{"status": "partial", "assumptions": ["ход оборван по лимиту"], "open_questions": [], "conflicts_with_prior_decisions": []}\n```'


def send(obj):
    sys.stdout.write(json.dumps(obj) + "\n")
    sys.stdout.flush()


def read_msg():
    line = sys.stdin.readline()
    if not line:
        return None
    line = line.strip()
    if not line:
        return read_msg()
    try:
        return json.loads(line)
    except ValueError:
        return read_msg()


def response(rid, result):
    send({"jsonrpc": "2.0", "id": rid, "result": result})


def error_response(rid, code, message):
    send({"jsonrpc": "2.0", "id": rid, "error": {"code": code, "message": message}})


def method_not_found_message(method):
    """Непустое однострочное message для ошибки -32601 (ACP-ERROR-001)."""
    name = str(method).replace("\r", " ").replace("\n", " ").strip()
    return "method not found: %s" % (name or "<unknown>")


# Уникальность sessionId и активная сессия (ACP-SESSION-002): первый
# session/new получает "s1" — как раньше, каждый следующий — свежий.
SESSION_SEQ = 0
ACTIVE_SESSION = "s1"


def open_session(msg):
    """Отвечает на session/new свежим уникальным sessionId.

    Последний выданный id становится активным: существующие односессионные
    режимы видят в session/update ровно его же ("s1").
    """
    global SESSION_SEQ, ACTIVE_SESSION
    SESSION_SEQ += 1
    ACTIVE_SESSION = "s%d" % SESSION_SEQ
    response(msg["id"], {"sessionId": ACTIVE_SESSION})
    return ACTIVE_SESSION


def dispatch_stray(msg):
    """Отвечает на «лишний» request, пришедший вместо ожидаемого метода.

    Повторный session/new получает свежий sessionId (ACP-SESSION-002),
    прочие неизвестные методы — ошибку -32601 (ACP-EXT-001/ACP-ERROR-001).
    """
    if msg.get("method") == "session/new":
        open_session(msg)
    else:
        error_response(msg["id"], -32601, method_not_found_message(msg.get("method")))


def await_request(method):
    """Читает сообщения, пока не придёт запрос с данным методом.

    «Лишние» запросы (с id) не игнорируются: повторный session/new получает
    свежий sessionId, неизвестный метод — ошибку -32601. Уведомления без id
    (в т.ч. session/cancel) ответа не получают и пропускаются.
    """
    while True:
        msg = read_msg()
        if msg is None:
            return None
        incoming = msg.get("method")
        if incoming == method:
            return msg
        if incoming is not None and msg.get("id") is not None:
            dispatch_stray(msg)


def chunk(text, message_id=None):
    content = {"type": "text", "text": text}
    if message_id is not None:
        content["messageId"] = message_id
    send(
        {
            "jsonrpc": "2.0",
            "method": "session/update",
            "params": {
                "sessionId": ACTIVE_SESSION,
                "update": {
                    "sessionUpdate": "agent_message_chunk",
                    "content": content,
                },
            },
        }
    )


def unknown_update(kind):
    """session/update с незнакомым клиенту sessionUpdate (F3)."""
    send(
        {
            "jsonrpc": "2.0",
            "method": "session/update",
            "params": {
                "sessionId": ACTIVE_SESSION,
                "update": {"sessionUpdate": kind},
            },
        }
    )


def tool_call(tool_id, title):
    send(
        {
            "jsonrpc": "2.0",
            "method": "session/update",
            "params": {
                "sessionId": ACTIVE_SESSION,
                "update": {"sessionUpdate": "tool_call", "toolCallId": tool_id, "title": title},
            },
        }
    )


def finish(stop="end_turn"):
    """Ответ на session/prompt с данным stopReason (срез «полный stopReason»)."""
    response(PROMPT_ID, {"stopReason": stop})


def request_permission():
    """Шлёт session/request_permission и возвращает выбранный optionId."""
    send(
        {
            "jsonrpc": "2.0",
            "id": 900,
            "method": "session/request_permission",
            "params": {
                "sessionId": ACTIVE_SESSION,
                "toolCall": {"toolCallId": "t1", "title": "Write file"},
                "options": [
                    {"optionId": "allow-once-1", "name": "Allow once", "kind": "allow_once"},
                    {"optionId": "allow-always-1", "name": "Always", "kind": "allow_always"},
                    {"optionId": "reject-1", "name": "Reject", "kind": "reject_once"},
                ],
            },
        }
    )
    while True:
        msg = read_msg()
        if msg is None:
            return None
        if msg.get("id") == 900 and "method" not in msg:
            return (msg.get("result") or {}).get("outcome", {}).get("optionId")


def client_request():
    """Шлёт fs-метод (не поддержан клиентом) и возвращает код ошибки."""
    send(
        {
            "jsonrpc": "2.0",
            "id": 901,
            "method": "fs/read_text_file",
            "params": {"sessionId": ACTIVE_SESSION, "path": "/etc/hostname"},
        }
    )
    while True:
        msg = read_msg()
        if msg is None:
            return None
        if msg.get("id") == 901 and "method" not in msg:
            return (msg.get("error") or {}).get("code")


def wait_for_cancel(marker):
    """Ждёт session/cancel, пишет маркер в CWD процесса.

    «Лишние» запросы обрабатываются как в `await_request`: не молчим.
    """
    while True:
        msg = read_msg()
        if msg is None:
            return False
        if msg.get("method") == "session/cancel":
            with open(marker, "w", encoding="utf-8") as fh:
                fh.write("cancel\n")
            return True
        if msg.get("method") is not None and msg.get("id") is not None:
            dispatch_stray(msg)


def permission_after_cancel():
    """После session/cancel просит допуск и возвращает outcome ответа клиента.

    Прогон уже отменяется — клиент обязан ответить `cancelled` (F4), а не
    выдать допуск: иначе агент считает, что разрешение состоялось.
    """
    send(
        {
            "jsonrpc": "2.0",
            "id": 902,
            "method": "session/request_permission",
            "params": {
                "sessionId": ACTIVE_SESSION,
                "toolCall": {"toolCallId": "t9", "title": "Write file"},
                "options": [
                    {"optionId": "allow-once-9", "name": "Allow once", "kind": "allow_once"}
                ],
            },
        }
    )
    while True:
        msg = read_msg()
        if msg is None:
            return None
        if msg.get("id") == 902 and "method" not in msg:
            return (msg.get("result") or {}).get("outcome", {}).get("outcome")


def main():
    mode = sys.argv[1] if len(sys.argv) > 1 else "ok"

    if mode == "init-exit":
        sys.exit(3)
    if mode == "init-silent":
        # Не отвечаем на initialize: клиент обязан сработать по своему таймауту.
        time.sleep(120)
        sys.exit(0)

    init = await_request("initialize")
    if init is None:
        return
    capabilities = {"loadSession": False}
    if mode == "close":
        # F5: агент заявляет cap session/close — клиент обязан закрыть сессию.
        capabilities["sessionCapabilities"] = {"close": True}
    if mode == "auth-required":
        # F2: агент заявляет cap authenticate — клиент обязан отказаться.
        capabilities["authenticate"] = True
    auth_methods = (
        [{"id": "oauth", "name": "OAuth"}]
        if mode in ("auth", "auth-required")
        else []
    )
    response(
        init["id"],
        {
            "protocolVersion": 1,
            "agentCapabilities": capabilities,
            "agentInfo": {"name": "fixture-agent", "version": "0.0.1"},
            "authMethods": auth_methods,
        },
    )
    if mode == "auth-required":
        # Клиент обязан отказаться от сессии (аутентификацию не поддерживает).
        time.sleep(30)
        sys.exit(0)

    new = await_request("session/new")
    if new is None:
        return
    open_session(new)

    prompt = await_request("session/prompt")
    if prompt is None:
        return
    global PROMPT_ID
    PROMPT_ID = prompt["id"]

    if mode == "ok":
        chunk("читаю задачу\n")
        tool_call("t1", "Read")
        chunk("готово\n" + CONTRACT + "\n")
        finish()
    elif mode == "plain":
        chunk("работаю без инструментов\n" + CONTRACT + "\n")
        finish()
    elif mode == "auth":
        # F2: authMethods без authenticate-cap — информационный список,
        # сессия продолжается штатно до end_turn.
        chunk("authMethods информационны\n" + CONTRACT + "\n")
        finish()
    elif mode == "permission":
        # Инструмент ДО запроса допуска: финальный текст — только чанки
        # после последнего tool_call (иначе клиент отбросит эхо выбора).
        tool_call("t0", "Prepare")
        chosen = request_permission()
        chunk("chosen=%s\n" % chosen)
        chunk("готово\n" + CONTRACT + "\n")
        finish()
    elif mode == "multi-message":
        # F1: два агентских сообщения с разными messageId и tool_call между
        # ними — финальный ответ = второе сообщение (а не «после tool_call»).
        chunk("первое сообщение\n", "m1")
        tool_call("t1", "Read")
        chunk("второе сообщение\n" + CONTRACT + "\n", "m2")
        finish()
    elif mode == "unknown-update":
        # F3: незнакомый sessionUpdate — клиент журналирует и продолжает.
        unknown_update("custom_update")
        chunk("незнакомое событие пережито\n" + CONTRACT + "\n")
        finish()
    elif mode == "perm-cancel":
        # F4: агент просит допуск и молчит; клиент по таймауту отменяет
        # прогон и на висящий при отмене запрос отвечает cancelled.
        chunk("жду допуск\n")
        if wait_for_cancel("cancel-received"):
            outcome = permission_after_cancel()
            with open("perm-outcome", "w", encoding="utf-8") as fh:
                fh.write("%s\n" % outcome)
        sys.exit(0)
    elif mode == "close":
        # F5: end_turn, затем клиент при cap session/close зовёт session/close.
        chunk("готово\n" + CONTRACT + "\n")
        finish()
        close = await_request("session/close")
        if close is not None:
            response(close["id"], {})
            with open("session-closed", "w", encoding="utf-8") as fh:
                fh.write("closed\n")
        sys.exit(0)
    elif mode == "fs":
        code = client_request()
        chunk("fs_error=%s\n" % code + CONTRACT + "\n")
        finish()
    elif mode == "env":
        leak = "present" if os.environ.get("ARCH_ACP_LEAK") else "none"
        chunk("LEAK=%s\n" % leak + CONTRACT + "\n")
        finish()
    elif mode == "limit-tokens":
        # Срез «полный stopReason»: ход оборван по лимиту токенов — частичный
        # ответ (контракт partial) обязан дойти до разбора JSON-контракта.
        chunk("начал работу\n")
        chunk("оборван по лимиту токенов\n" + PARTIAL_CONTRACT + "\n")
        finish("max_tokens")
    elif mode == "limit-turns":
        chunk("оборван по лимиту ходов\n" + PARTIAL_CONTRACT + "\n")
        finish("max_turn_requests")
    elif mode == "refusal":
        # Агент отказался выполнять: текст отказа — в выводе прогона.
        chunk("отказываюсь выполнять: задача вне политики\n")
        finish("refusal")
    elif mode in ("cancel", "cancelled-after-cancel"):
        chunk("работаю\n")
        if wait_for_cancel("cancel-received"):
            finish("cancelled")
        sys.exit(0)
    elif mode == "idle":
        chunk("начал\n")
        wait_for_cancel("cancel-received")
        time.sleep(120)
        sys.exit(0)
    elif mode == "active":
        for i in range(18):
            chunk("шаг %d\n" % i)
            time.sleep(0.2)
        chunk("поток завершён\n" + CONTRACT + "\n")
        finish()
    else:
        chunk("неизвестный режим\n")
        finish()


if __name__ == "__main__":
    main()
