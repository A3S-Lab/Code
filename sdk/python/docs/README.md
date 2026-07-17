# A3S Code — Python SDK

Native Python bindings for the A3S Code AI coding agent, built with PyO3.

## Event-sourced state graphs

`StateGraphRuntime` exposes the same portable JSON contract as Rust and Node:

```python
import json
from a3s_code import StateGraphRuntime

graph = StateGraphRuntime("request-42")
graph.propose_patch(json.dumps({
    "expected_graph_version": 0,
    "operations": [{
        "op": "add_object",
        "id": "task-1",
        "object_type": "task",
        "data": {"status": "open"},
    }],
}))

restored = StateGraphRuntime.restore(graph.events_json())
fork = restored.fork_at(len(json.loads(restored.events_json())))
diff = json.loads(fork.diff_json(restored))
```

Rust Core additionally supports predicate-scoped `Behavior` callbacks and
memory/file `GraphEventStore` implementations. SDK applications can implement
their own language-native reactive loop around the lossless event and patch
contract.

Asyncio hosts should prefer `Agent.create_async()`, `agent.session_async()`,
`agent.resume_session_async()`, `session.save_async()`,
`session.cancel_async()`, `session.close_async()`, and `agent.close_async()` for
lifecycle operations. These methods return asyncio Futures and keep blocking
store/runtime waits off the event-loop task.

Model requests and run observability also provide `session.send_async()`,
`runs_async()`, `run_snapshot_async()`, `run_events_async()`, and
`run_event_page_async()` with the same result shapes as their synchronous
counterparts.

Use `session.tool_async(name, args)` for governed direct-tool execution from an
asyncio host. It preserves the synchronous `ToolResult` shape and fails closed
after session shutdown.

## Documentation Boundary

This README and `QUICK_REFERENCE.md` describe the current 2.0 Python SDK
surface. Historical investigation reports from the 1.x cleanup have been
removed so this directory stays focused on supported APIs.

## Installation

```bash
pip install a3s-code
```

## Quick Start

```python
from a3s_code import Agent

agent = Agent.create("agent.acl")
session = agent.session("/my-project")

result = session.send("What files handle authentication?")
print(result.text)
```

## Slash Commands

Every session includes built-in slash commands dispatched before the LLM:

```python
# List all available commands
commands = session.list_commands()
for cmd in commands:
    print(f"/{cmd['name']:15s} {cmd['description']}")

# Built-in commands
result = session.send("/help")       # List all commands
result = session.send("/model")      # Show current model
result = session.send("/cost")       # Token usage and cost
result = session.send("/history")    # Conversation stats
```

### Custom Commands

```python
def my_handler(args: str, ctx: dict) -> str:
    return f"Model: {ctx['model']}, History: {ctx['history_len']} msgs, args: {args!r}"

session.register_command("status", "Show session info", my_handler)
result = session.send("/status hello")
```

## Full API

```python
from a3s_code import (
    Agent,
    ArtifactStoreLimits,
    ConfirmationPolicy,
    PermissionPolicy,
    SessionOptions,
    DefaultSecurityProvider,
    FileMemoryStore,
    FileSessionStore,
)

agent = Agent.create("agent.acl")
session = agent.session("/my-project",
    model="openai/gpt-4o",
    planning_mode="auto",  # "enabled" forces planning, "disabled" turns it off
)

# Send / Stream
result = session.send({"prompt": "Explain the auth module"})
active_turn = None
attempt_text = []
for event in session.stream({"prompt": "Refactor auth"}):
    if event.event_type == "turn_start":
        active_turn = event.turn
        attempt_text.clear()
    elif event.event_type == "text_delta":
        attempt_text.append(event.text or "")
    elif event.event_type == "turn_end" and event.turn == active_turn:
        print("".join(attempt_text), end="", flush=True)
        attempt_text.clear()

# Planning events
# Prefer planning_mode="auto" | "enabled" | "disabled". The legacy planning
# bool still works: True forces planning, False disables it. In streaming mode,
# render task_updated as the current task list; step_start and step_end are
# per-step progress events.

# Run replay
runs = session.runs()
if runs:
    print(runs[-1]["id"], runs[-1]["status"])
    for event in session.run_events(runs[-1]["id"]):
        print(event["type"], event["payload"], event["metadata"]["sequence"])
    print(session.active_tools())
    # Cancels only if that run is still active; stale IDs are ignored.
    session.cancel_run(runs[-1]["id"])

# Replay entries use the v1 {version, type, payload, metadata} envelope.

# Direct tools (bypass LLM)
opts = SessionOptions()
opts.artifact_store_limits = ArtifactStoreLimits(max_artifacts=64, max_bytes=8 * 1024 * 1024)
opts.tool_timeout_ms = 120_000
opts.llm_api_timeout_ms = 120_000
opts.circuit_breaker_threshold = 4
opts.duplicate_tool_call_threshold = 5
opts.manual_delegation_enabled = True
session = agent.session("/my-project", opts)
session.read_file("src/main.py")
session.read_file("src/main.py", offset=2000, limit=2000)
session.bash("pytest")
session.glob("**/*.py")
session.grep("TODO")
session.git({"command": "status"})
session.git({"command": "worktree", "subcommand": "list"})
session.get_artifact("a3s://tool-output/read/abc123")
session.register_dynamic_workflow_runtime()
session.tool("dynamic_workflow", {
    "source": """
        export default async function run(ctx, inputs) {
          if (inputs.kind === 'workflow') {
            return { type: 'complete', output: { text: inputs.input.message } };
          }
          return { type: 'fail', error: 'unexpected step invocation' };
        }
    """,
    "input": {"message": "hello from Flow"},
})
session.unregister_dynamic_tool("dynamic_workflow")

# Slash commands
session.list_commands()
session.register_command("ping", "Pong!", lambda args, ctx: "pong")

# Memory
session.remember_success("task", ["tool"], "result")
session.recall_similar("auth", 5)

# Hooks
session.register_hook("audit", "pre_tool_use", handler_fn)

# MCP
session.add_mcp({
    "name": "github",
    "transport": {
        "type": "stdio",
        "command": "npx",
        "args": ["-y", "@modelcontextprotocol/server-github"],
    },
    "timeout_ms": 30_000,
})
session.mcps()
session.tool_names()
session.remove_mcp("github")

# Evidence
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
session.verification_reports()
session.verification_summary_text()

# Persistence
opts = SessionOptions()
opts.session_store = FileSessionStore('./sessions')
opts.session_id = 'my-session'
opts.auto_save = True
session2 = agent.session(".", opts)
resumed = agent.resume_session('my-session', opts)
```


## HITL Confirmations

Use `PermissionPolicy` to decide which tools ask, then `ConfirmationPolicy` to
control confirmation runtime behavior such as timeout and YOLO lanes. Invalid
permission decisions, timeout actions, and lane names are rejected when the
session is created so unsafe fallbacks do not silently change policy.

```python
opts = SessionOptions()
opts.permission_policy = PermissionPolicy(ask=["bash*"], default_decision="allow")
opts.confirmation_policy = ConfirmationPolicy(
    enabled=True,
    default_timeout_ms=30_000,
    timeout_action="reject",
    yolo_lanes=["query"],
)
session = agent.session(".", opts)

for pending in session.pending_confirmations():
    session.confirm_tool_use(pending["tool_id"], approved=True, reason="Reviewed")
```

For the streaming event-driven loop used by UIs, see
`examples/hitl_confirmation_loop.py`.

## Delegation

Routine multi-agent work uses the model-visible `task` and `parallel_task`
tools. Automatic subagent delegation can be enabled with
`opts.auto_delegation = AutoDelegationConfig(enabled=True)`. Set
`opts.auto_parallel = False` to globally disable only automatic parallel
fan-out; manual `parallel_task` / `session.tasks(...)` stays available.
The old standalone lifecycle control-plane API is intentionally removed from
the 2.0 SDK surface.

## License

MIT
