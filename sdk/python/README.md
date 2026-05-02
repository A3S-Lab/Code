# A3S Code — Python SDK

Native Python bindings for the A3S Code AI coding agent, built with PyO3.

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
    SessionOptions,
    DefaultSecurityProvider,
    FileMemoryStore,
    FileSessionStore,
)

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

# Streams with no custom history update session history and verification evidence
# when the stream completes. Passing explicit history keeps the stream isolated.

# Direct tools (bypass LLM)
session.read_file("src/main.py")
session.bash("pytest")
session.glob("**/*.py")
session.grep("TODO")

# Programmatic Tool Calling (embedded QuickJS)
program = session.program({
    "source": """
        export default async function run(ctx, inputs) {
          const hits = await ctx.grep(inputs.query, { glob: '*.py' });
          const files = await ctx.glob('src/**/*.py');
          return { hits, files: files.slice(0, 10) };
        }
    """,
    "inputs": {"query": "PermissionPolicy"},
    "allowed_tools": ["grep", "glob"],
    "limits": {"timeoutMs": 30000, "maxToolCalls": 20, "maxOutputBytes": 65536},
})
print(program.output)

# Rich document parsing metadata
tool = session.tool("agentic_parse", {"path": "docs/scanned.pdf"})
print(tool.metadata)  # parsed dict

query_tool = session.tool(
    "agentic_parse",
    {"path": "docs/scanned.pdf", "query": "overview"},
)
for block in query_tool.agentic_parse_llm_blocks_info:
    location = block.location.display if block.location else None
    print(block.index, block.kind, block.label, location)

search = session.tool("agentic_search", {"query": "invoice total", "mode": "fast"})
for result in search.agentic_search_results_info:
    for match in result.matches:
        print(match.line_number, match.locator, match.content)

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

## Agentic Search Match Locators

`agentic_search_results_info` now exposes typed match and sampled-line entries
so you can inspect page / section locators without parsing raw metadata.

```python
search = session.tool("agentic_search", {"query": "overview", "mode": "fast"})

for result in search.agentic_search_results_info:
    for match in result.matches:
        print(match.line_number, match.locator, match.content)
        if match.context_before:
            print("before:", match.context_before[-1])

deep = session.tool("agentic_search", {"query": "overview", "mode": "deep"})
for result in deep.agentic_search_results_info:
    for sampled in result.sampled_lines:
        print(sampled.line_number, sampled.locator, sampled.distance, sampled.weight)
```

## Agentic Parse LLM Blocks

When `agentic_parse` runs with a `query`, the SDK also exposes which structured
document blocks were actually selected for the LLM input.

```python
tool = session.tool(
    "agentic_parse",
    {"path": "docs/scanned.pdf", "query": "overview"},
)

for block in tool.agentic_parse_llm_blocks_info:
    location = block.location.display if block.location else None
    print(block.index, block.kind, block.label, location)
```

## Advanced Sub-Agent Events

`Orchestrator.create(agent=agent)` is the advanced lifecycle-control API for
real LLM-backed sub-agents. Routine delegation should use `task` /
`parallel_task`; use `handle.events()` when you need direct event monitoring.

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
