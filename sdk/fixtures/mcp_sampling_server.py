"""Deterministic stdio MCP peer for SDK sampling tests; no model or tools."""

import json
import sys
from pathlib import Path


def send(value):
    print(json.dumps(value), flush=True)


sampled = False
for line in sys.stdin:
    message = json.loads(line)
    method = message.get("method")
    if method == "initialize":
        send({"jsonrpc": "2.0", "id": message["id"], "result": {
            "protocolVersion": "2024-11-05", "capabilities": {"tools": {}},
            "serverInfo": {"name": "sampling-fixture", "version": "1.0.0"},
        }})
    elif method == "tools/list":
        if not sampled:
            sampled = True
            replies = []
            for index in range(int(sys.argv[2])):
                request_id = f"sample-{index}"
                send({"jsonrpc": "2.0", "id": request_id,
                      "method": "sampling/createMessage", "params": {
                          "messages": [{"role": "user", "content": {
                              "type": "text", "text": "fixture-only"}}],
                          "maxTokens": 8,
                      }})
                for response_line in sys.stdin:
                    response = json.loads(response_line)
                    if response.get("id") == request_id:
                        replies.append(response)
                        break
            Path(sys.argv[1]).write_text(json.dumps(replies), encoding="utf-8")
        send({"jsonrpc": "2.0", "id": message["id"], "result": {"tools": []}})
    elif "id" in message and method:
        send({"jsonrpc": "2.0", "id": message["id"], "error": {
            "code": -32601, "message": "fixture has no such method"}})
