# A3S Code Python SDK 2.0 Quick Reference

This page is the short, current reference for the Python SDK.

## Default Path

```python
from a3s_code import Agent

agent = Agent.create("agent.acl")
session = agent.session(".")

result = session.send("Find the code path that handles authentication.")
print(result.text)

for event in session.stream("Refactor the tests around that code path."):
    if event.event_type == "text_delta":
        print(event.text, end="", flush=True)
```

Use the session API first. It owns normal model execution, built-in tools,
slash commands, memory, persistence, trace events, artifacts, and verification
evidence.

## Direct Tools

```python
session.read_file("src/main.py")
session.bash("cargo test -p a3s-code-core --lib")
session.glob("**/*.py")
session.grep("TODO")
session.tool("agentic_search", {"query": "router", "mode": "fast"})
```

Direct tools bypass the LLM and are useful for deterministic checks,
diagnostics, and tests.

## Evidence

```python
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

## Advanced Sub-Agent Control Plane

```python
from a3s_code import Agent, Orchestrator, SubAgentConfig

agent = Agent.create("agent.acl")
orch = Orchestrator.create(agent=agent)

handle = orch.spawn_subagent(SubAgentConfig(
    agent_type="general",
    prompt="Inspect the repo and report the risky files.",
    max_steps=5,
))

events = handle.events()
```

`Orchestrator` is for direct lifecycle and event-stream control of real
LLM-backed sub-agents. It is not the default public composition API, and the
removed 1.0 `run_team` / `runTeam` shortcuts are intentionally absent.
