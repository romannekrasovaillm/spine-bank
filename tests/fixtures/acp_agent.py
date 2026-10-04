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
  fs          клиентский метод fs/read_text_file — эхо кода ошибки;
  env         эхо наличия переменной ARCH_ACP_LEAK (проверка env-политики);
  cancel      после промпта ждёт session/cancel, пишет маркер и отвечает
              stopReason=cancelled;
  idle        один чанк и молчание (idle-детект клиента);
  active      частые чанки дольше idle-окна — прерывать нельзя;
  init-exit   ранний выход до ответа на initialize;
  init-silent молчание на initialize (таймаут инициализации).

Маркер отмены пишется в CWD процесса — каталог прогона, который назначает
клиент (`session/new.cwd`).
"""

import json
import os
import sys
import time

CONTRACT = '```json\n{"status": "complete", "assumptions": [], "open_questions": [], "conflicts_with_prior_decisions": []}\n```'


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


def await_request(method):
    """Читает сообщения, пока не придёт запрос с данным методом."""
    while True:
        msg = read_msg()
        if msg is None:
            return None
        if msg.get("method") == method:
            return msg


def chunk(text):
    send(
        {
            "jsonrpc": "2.0",
            "method": "session/update",
            "params": {
                "sessionId": "s1",
                "update": {
                    "sessionUpdate": "agent_message_chunk",
                    "content": {"type": "text", "text": text},
                },
            },
        }
    )


def tool_call(tool_id, title):
    send(
        {
            "jsonrpc": "2.0",
            "method": "session/update",
            "params": {
                "sessionId": "s1",
                "update": {"sessionUpdate": "tool_call", "toolCallId": tool_id, "title": title},
            },
        }
    )


def finish(end_turn=True):
    response(PROMPT_ID, {"stopReason": "end_turn" if end_turn else "cancelled"})


def request_permission():
    """Шлёт session/request_permission и возвращает выбранный optionId."""
    send(
        {
            "jsonrpc": "2.0",
            "id": 900,
            "method": "session/request_permission",
            "params": {
                "sessionId": "s1",
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
            "params": {"sessionId": "s1", "path": "/etc/hostname"},
        }
    )
    while True:
        msg = read_msg()
        if msg is None:
            return None
        if msg.get("id") == 901 and "method" not in msg:
            return (msg.get("error") or {}).get("code")


def wait_for_cancel(marker):
    """Ждёт session/cancel, пишет маркер в CWD процесса."""
    while True:
        msg = read_msg()
        if msg is None:
            return False
        if msg.get("method") == "session/cancel":
            with open(marker, "w", encoding="utf-8") as fh:
                fh.write("cancel\n")
            return True


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
    response(
        init["id"],
        {
            "protocolVersion": 1,
            "agentCapabilities": {"loadSession": False},
            "agentInfo": {"name": "fixture-agent", "version": "0.0.1"},
            "authMethods": [],
        },
    )

    new = await_request("session/new")
    if new is None:
        return
    response(new["id"], {"sessionId": "s1"})

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
    elif mode == "permission":
        # Инструмент ДО запроса допуска: финальный текст — только чанки
        # после последнего tool_call (иначе клиент отбросит эхо выбора).
        tool_call("t0", "Prepare")
        chosen = request_permission()
        chunk("chosen=%s\n" % chosen)
        chunk("готово\n" + CONTRACT + "\n")
        finish()
    elif mode == "fs":
        code = client_request()
        chunk("fs_error=%s\n" % code + CONTRACT + "\n")
        finish()
    elif mode == "env":
        leak = "present" if os.environ.get("ARCH_ACP_LEAK") else "none"
        chunk("LEAK=%s\n" % leak + CONTRACT + "\n")
        finish()
    elif mode == "cancel":
        chunk("работаю\n")
        if wait_for_cancel("cancel-received"):
            finish(end_turn=False)
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
