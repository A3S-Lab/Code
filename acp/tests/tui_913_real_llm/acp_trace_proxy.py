#!/usr/bin/python3
"""Test-only, byte-preserving ACP relay; retain prompt identities, never content."""

import hashlib
import json
import os
import selectors
import signal
import subprocess
import sys


def prompt_digest(prompt):
    if not isinstance(prompt, list) or not all(
        isinstance(block, dict) and block.get("type") == "text"
        and isinstance(block.get("text"), str) for block in prompt
    ):
        return None
    text = "\n".join(block["text"] for block in prompt)
    material = b"a3s.test.tui-prompt.v1\0" + text.encode("utf-8")
    return "sha256:" + hashlib.sha256(material).hexdigest()


def main():
    binary = os.environ["A3S_CODE_TUI_TEST_ACP_REAL"]
    path = os.environ["A3S_CODE_TUI_TEST_ACP_TRACE"]
    fd = os.open(path, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600)
    trace = os.fdopen(fd, "w", buffering=1)
    sequence = 0
    pending = {}

    def retain(event):
        nonlocal sequence
        sequence += 1
        trace.write(json.dumps({"seq": sequence, **event}, separators=(",", ":")) + "\n")
        trace.flush()

    def observe(packet, source):
        key = json.dumps(packet.get("id"), sort_keys=True)
        if source == "client" and packet.get("method") == "session/prompt":
            params = packet["params"]
            binding = {
                "rpc_id": packet["id"],
                "session_id": params.get("sessionId"),
                "prompt_id": params.get("_meta", {}).get("promptId"),
                "prompt_digest": prompt_digest(params.get("prompt")),
            }
            if key in pending:
                raise ValueError("duplicate in-flight prompt RPC id")
            retain({"kind": "prompt-request", "in_flight_before": len(pending), **binding})
            pending[key] = binding
        elif source == "agent" and "method" not in packet and key in pending:
            binding = pending.pop(key)
            error = packet.get("error")
            data = error.get("data") if isinstance(error, dict) else None
            retain({
                "kind": "prompt-response", **binding,
                "stop_reason": packet.get("result", {}).get("stopReason"),
                "error_code": error.get("code") if isinstance(error, dict) else None,
                # Narrow a known machine-produced error; never infer intent or
                # discharge effect obligations from the message text.
                "completion_gate": isinstance(data, str) and data.startswith(
                    "completion gate: workspace mutation "
                ),
            })

    child = subprocess.Popen(
        [binary, *sys.argv[1:]], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
        start_new_session=True,
    )
    selector = selectors.DefaultSelector()
    selector.register(sys.stdin.buffer, selectors.EVENT_READ, "client")
    selector.register(child.stdout, selectors.EVENT_READ, "agent")
    buffers = {"client": b"", "agent": b""}
    try:
        while True:
            for key, _ in selector.select():
                source = key.data
                chunk = os.read(key.fd, 65536)
                if not chunk:
                    if buffers[source]:
                        raise ValueError("ACP stream ended inside a packet")
                    return 0
                buffers[source] += chunk
                if len(buffers[source]) > 32 * 1024 * 1024:
                    raise ValueError("test relay ACP packet exceeds 32 MiB")
                while b"\n" in buffers[source]:
                    line, buffers[source] = buffers[source].split(b"\n", 1)
                    packet = json.loads(line)
                    observe(packet, source)
                    destination = child.stdin if source == "client" else sys.stdout.buffer
                    destination.write(line + b"\n")
                    destination.flush()
    finally:
        selector.close()
        child.stdin.close()
        # The session leader is our unreaped child. Never target a reused PID
        # or any process selected by name. This fixture relay is not native
        # descendant-lifetime qualification.
        if child.poll() is None:
            os.killpg(child.pid, signal.SIGTERM)
            try:
                child.wait(timeout=2)
            except subprocess.TimeoutExpired:
                os.killpg(child.pid, signal.SIGKILL)
                child.wait(timeout=2)
        retain({"kind": "proxy-closed", "pending_prompts": len(pending)})
        trace.close()


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except Exception as error:
        # Do not print packets, environment values or arbitrary child errors.
        print("ACP test relay failed: " + type(error).__name__, file=sys.stderr)
        raise SystemExit(1)
