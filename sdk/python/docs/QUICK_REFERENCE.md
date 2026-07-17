# A3S Code Python SDK 2.3 Quick Reference

This page is the short, current reference for the Python SDK.

## Default Path

```python
from a3s_code import Agent

agent = Agent.create("agent.acl")
session = agent.session(".")

result = session.send({"prompt": "Find the code path that handles authentication."})
print(result.text)

active_turn = None
attempt_text = []
for event in session.stream({"prompt": "Refactor the tests around that code path."}):
    if event.event_type == "turn_start":
        active_turn = event.turn
        attempt_text.clear()
    elif event.event_type == "text_delta":
        attempt_text.append(event.text or "")
    elif event.event_type == "turn_end" and event.turn == active_turn:
        print("".join(attempt_text), end="", flush=True)
        attempt_text.clear()
```

Streaming events use envelope version `1`: `event.type` is the canonical open
discriminant, `event.payload` is lossless, and `event.metadata` preserves
optional protocol metadata. `event.event_type` is a compatibility alias.
Repeated `turn_start` events with the same turn replace an interrupted stream
attempt, so clear all provisional text, reasoning, and tool drafts for that
turn before applying the replacement deltas.

Use the short object-shaped request APIs first. They own normal model execution,
built-in tools,
slash commands, memory, persistence, trace events, artifacts, and verification
evidence.

## Planning

Planning is automatic by default. Prefer `planning_mode="auto" | "enabled" |
"disabled"` for an explicit SDK contract. The legacy `planning=True` and
`planning=False` shortcuts still work. In streaming mode, render `task_updated`
as the current task list; `step_start` and `step_end` are progress events for
individual steps.

## Runs

```python
runs = session.runs()
latest = runs[-1] if runs else None
events = session.run_events(latest["id"]) if latest else []
# events are v1 {version, type, payload, metadata} envelopes; replay position
# is in metadata["sequence"] and metadata["timestamp_ms"].
active_tools = session.active_tools()
cancelled = session.cancel_run(latest["id"]) if latest else False
```

## Direct Tools

```python
session.read_file("src/main.py")
session.read_file("src/main.py", offset=2000, limit=2000)
session.bash("cargo test -p a3s-code-core --lib")
session.glob("**/*.py")
session.grep("TODO")
session.git({"command": "status"})
session.register_dynamic_workflow_runtime()
session.unregister_dynamic_tool("dynamic_workflow")
```

Direct tools bypass the LLM and are useful for deterministic checks,
diagnostics, and tests.

## Evidence

```python
from a3s_code import ArtifactStoreLimits, SessionOptions

opts = SessionOptions()
opts.artifact_store_limits = ArtifactStoreLimits(max_artifacts=64, max_bytes=8 * 1024 * 1024)
opts.llm_api_timeout_ms = 120_000
opts.duplicate_tool_call_threshold = 5
opts.manual_delegation_enabled = True
session = agent.session(".", opts)

artifact = session.get_artifact("a3s://tool-output/read/abc123")

session.record_verification_reports([{
    "schema": "a3s.verification_report.v1",
    "subject": "sdk:tests",
    "status": "passed",
    "checks": [{
        "id": "check:sdk",
        "kind": "test",
        "description": "Run SDK tests",
        "status": "passed",
        "required": True,
    }],
}])

for event in session.trace_events():
    print(event)

for report in session.verification_reports():
    print(report)

print(session.verification_summary_text())
```

2.0 evidence flows through trace events, artifacts, and verification reports.
The old task/progress/idle lifecycle is not part of the public Python SDK.

## Verification

```python
presets = session.verification_presets()
report = session.verify_commands("core tests", [
    {"label": "unit", "command": "cargo test -p a3s-code-core --lib"},
])
print(report)
```

Verification commands are explicit. Presets help discover likely commands, but
the SDK does not auto-run project checks.

## Routine Delegation

Use the model-visible `task` and `parallel_task` tools for ordinary delegation.
They are the default multi-agent composition path in 2.0.
For automatic subagent delegation, set `opts.auto_parallel = False` to disable
automatic parallel fan-out while keeping manual `parallel_task` available.

## MCP

```python
session.add_mcp({
    "name": "github",
    "transport": {"type": "stdio", "command": "npx", "args": ["-y", "@modelcontextprotocol/server-github"]},
    "timeout_ms": 30_000,
})
```

Prefer the compact object-shaped API for new integrations;
`add_mcp_server_config(...)` and `add_mcp_server(...)` remain available for
existing callers.

The standalone 1.x lifecycle control plane and team shortcuts are intentionally
absent in 2.0.
