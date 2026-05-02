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
    planning_mode="auto",  # "enabled" forces planning, "disabled" turns it off
)

# Send / Stream
result = session.send("Explain the auth module")
for event in session.stream("Refactor auth"):
    if event.event_type == "text_delta":
        print(event.text, end="", flush=True)

# Planning events
# Prefer planning_mode="auto" | "enabled" | "disabled". The legacy planning
# bool still works: True forces planning, False disables it. In streaming mode,
# render task_updated as the current task list; step_start and step_end are
# per-step progress events.

# Run replay
runs = session.runs()
if runs:
    print(runs[-1]["id"], runs[-1]["status"])
    print(session.run_events(runs[-1]["id"]))
    # Cancels only if that run is still active; stale IDs are ignored.
    session.cancel_run(runs[-1]["id"])

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

## Delegation

Routine multi-agent work uses the model-visible `task` and `parallel_task`
tools. The old standalone lifecycle control-plane API is intentionally
removed from the 2.0 SDK surface.

## License

MIT
