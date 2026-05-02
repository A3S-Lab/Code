# A3S Code — Python SDK

Native Python bindings for the A3S Code AI coding agent, built with PyO3.

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
from a3s_code import Agent, SessionOptions, DefaultSecurityProvider, FileMemoryStore, FileSessionStore

agent = Agent.create("agent.acl")
session = agent.session("/my-project",
    model="openai/gpt-4o",
    builtin_skills=True,
    planning=True,
)

# Send / Stream
result = session.send("Explain the auth module")
for event in session.stream("Refactor auth"):
    if event.event_type == "text_delta":
        print(event.text, end="", flush=True)

# Direct tools (bypass LLM)
session.read_file("src/main.py")
session.bash("pytest")
session.glob("**/*.py")
session.grep("TODO")

# Slash commands
session.list_commands()
session.register_command("ping", "Pong!", lambda args, ctx: "pong")

# Memory
session.remember_success("task", ["tool"], "result")
session.recall_similar("auth", 5)

# Hooks
session.register_hook("audit", "pre_tool_use", handler_fn)

# MCP
session.add_mcp_server("github", command="npx", args=["-y", "@modelcontextprotocol/server-github"])
session.mcp_status()
session.tool_names()
session.remove_mcp_server("github")

# Persistence
opts = SessionOptions()
opts.session_store = FileSessionStore('./sessions')
opts.session_id = 'my-session'
opts.auto_save = True
session2 = agent.session(".", opts)
resumed = agent.resume_session('my-session', opts)
```

## Advanced Sub-Agent Events

`Orchestrator.create(agent=agent)` is the advanced lifecycle-control API for
real LLM-backed sub-agents. Routine delegation should use `task` /
`parallel_task`; use `handle.events()` only when you need direct control-plane
monitoring.

```python
from a3s_code import Agent, Orchestrator, SubAgentConfig

agent = Agent.create("agent.acl")
orch = Orchestrator.create(agent=agent)
handle = orch.spawn_subagent(SubAgentConfig(
    agent_type="general",
    prompt="Use bash to print hello, then explain it.",
    max_steps=5,
))

events = handle.events()
while True:
    event = events.recv(timeout_ms=1000)
    if event is None:
        continue

    event_type = event["event_type"]

    if event_type == "sub_agent_internal_event" and event.get("type") == "text_delta":
        print(event.get("text", ""), end="", flush=True)
    elif event_type == "tool_execution_started":
        print("tool args:", event["args"])
    elif event_type == "tool_execution_completed":
        print("tool duration_ms:", event["duration_ms"])
    elif event_type == "sub_agent_completed":
        break
```

Important details:

- Event names use `sub_agent_*`, not `subagent_*`.
- `sub_agent_internal_event` payloads are flattened. For example, a text delta is:
  `{"event_type": "sub_agent_internal_event", "type": "text_delta", "text": "..."}`
  rather than nesting under `event`.
- `tool_execution_started.args` contains the accumulated tool input.
- `tool_execution_completed.duration_ms` is guaranteed to be at least `1` for real tool calls.

## License

MIT
