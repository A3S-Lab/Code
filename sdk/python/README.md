# A3S Code — Python SDK

Native Python bindings for the A3S Code AI coding agent, built with PyO3.

## Installation

```bash
pip install a3s-code
```

## Quick Start

```python
from a3s_code import Agent

agent = Agent.create("agent.hcl")
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
result = session.send("/cron-list")  # List scheduled tasks
```

### Custom Commands

```python
def my_handler(args: str, ctx: dict) -> str:
    return f"Model: {ctx['model']}, History: {ctx['history_len']} msgs, args: {args!r}"

session.register_command("status", "Show session info", my_handler)
result = session.send("/status hello")
```

## Scheduled Tasks

Schedule recurring prompts that fire after each `send()` call:

```python
# Via /loop slash command
r = session.send("/loop 30s check deployment status")
print(r.text)  # Scheduled [a1b2c3d4]: "check deployment status" — fires every 30s

# Programmatic API
task_id = session.schedule_task("summarize recent commits", 300)  # every 5 min

# List active tasks
for t in session.list_scheduled_tasks():
    print(f"[{t['id']}] every {t['interval_secs']}s — \"{t['prompt']}\"")

# Cancel
session.cancel_scheduled_task(task_id)
session.send(f"/cron-cancel {task_id}")
```

**Interval syntax:** `30s`, `5m`, `2h`, `1d`. Leading or trailing with `every` clause.

## Full API

```python
from a3s_code import Agent, SessionOptions, DefaultSecurityProvider, FileMemoryStore, FileSessionStore

agent = Agent.create("agent.hcl")
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

# Slash commands & scheduling
session.list_commands()
session.register_command("ping", "Pong!", lambda args, ctx: "pong")
task_id = session.schedule_task("daily report", 86400)
session.list_scheduled_tasks()
session.cancel_scheduled_task(task_id)

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

## License

MIT
