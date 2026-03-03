#!/usr/bin/env python3
"""
Minimal stdio MCP server for integration testing.

Exposes two tools:
  echo(message)   → returns the message verbatim
  get_secret()    → returns a server-side secret (passed as argv[1])

The secret is unknown to the LLM unless it actually calls get_secret().
This prevents the LLM from faking a tool call by guessing the expected answer.

Protocol: newline-delimited JSON-RPC 2.0
No external dependencies required.
"""

import json
import sys

# Secret is passed at spawn time so the test runner knows it but the LLM does not.
SECRET = sys.argv[1] if len(sys.argv) > 1 else "default-secret"

TOOLS = [
    {
        "name": "echo",
        "description": "Echo the input message back verbatim",
        "inputSchema": {
            "type": "object",
            "properties": {
                "message": {"type": "string", "description": "The message to echo"},
            },
            "required": ["message"],
        },
    },
    {
        "name": "get_secret",
        "description": "Retrieve the server-side secret value",
        "inputSchema": {"type": "object", "properties": {}},
    },
]


def send(obj):
    sys.stdout.write(json.dumps(obj, separators=(",", ":")) + "\n")
    sys.stdout.flush()


def handle(msg):
    method = msg.get("method", "")
    msg_id = msg.get("id")

    if method == "initialize":
        send({
            "jsonrpc": "2.0",
            "id": msg_id,
            "result": {
                "protocolVersion": "2024-11-05",
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "echo", "version": "0.1.0"},
            },
        })

    elif method.startswith("notifications/"):
        pass  # no response for notifications

    elif method == "tools/list":
        send({"jsonrpc": "2.0", "id": msg_id, "result": {"tools": TOOLS}})

    elif method == "tools/call":
        params = msg.get("params", {})
        tool = params.get("name", "")
        args = params.get("arguments", {})

        if tool == "echo":
            send({
                "jsonrpc": "2.0",
                "id": msg_id,
                "result": {"content": [{"type": "text", "text": args.get("message", "")}]},
            })
        elif tool == "get_secret":
            send({
                "jsonrpc": "2.0",
                "id": msg_id,
                "result": {"content": [{"type": "text", "text": SECRET}]},
            })
        else:
            send({
                "jsonrpc": "2.0",
                "id": msg_id,
                "error": {"code": -32601, "message": f"Unknown tool: {tool}"},
            })

    elif msg_id is not None:
        send({
            "jsonrpc": "2.0",
            "id": msg_id,
            "error": {"code": -32601, "message": f"Method not found: {method}"},
        })


def main():
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            handle(json.loads(line))
        except (json.JSONDecodeError, KeyError):
            pass


if __name__ == "__main__":
    main()
