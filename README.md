# A3S Code

**Intent-driven AI coding agent framework.** A3S Code is a Rust library with native Python and Node.js bindings. It gives an LLM a workspace, tools, and memory — then uses intent detection and context perception to inject the right context at the right time.

[![crates.io](https://img.shields.io/crates/v/a3s-code-core)](https://crates.io/crates/a3s-code-core)
[![PyPI](https://img.shields.io/pypi/v/a3s-code)](https://pypi.org/project/a3s-code/)
[![npm](https://img.shields.io/npm/v/@a3s-lab/code)](https://www.npmjs.com/package/@a3s-lab/code)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue)](LICENSE)

---

## Install

```bash
# Python
pip install a3s-code

# Node.js
npm install @a3s-lab/code
```

---

## Quick Start

**1. Create an agent config** (`agent.hcl`):

```hcl
default_model = "anthropic/claude-sonnet-4-20250514"

providers {
  name    = "anthropic"
  api_key = env("ANTHROPIC_API_KEY")
}
```

**2. Run an agent session:**

```python
from a3s_code import Agent

agent = Agent.create("agent.hcl")
session = agent.session("/my-project")

result = session.send("Find all places where we handle authentication errors")
print(result.text)
```

```typescript
import { Agent } from '@a3s-lab/code';

const agent = await Agent.create('agent.hcl');
const session = agent.session('/my-project');

const result = await session.send('Find all places where we handle authentication errors');
console.log(result.text);
session.close();
```

---

## How It Works

### Intent Detection (AHP 2.3)

Every prompt fires the `IntentDetection` harness point, delegating intent classification to the AHP server. The harness can use LLM classification, keyword matching, or custom logic. This enables:

- **Multi-language intent recognition** — Harness can use LLM for non-English prompts
- **Centralized intent taxonomy** — Update detection logic without changing agent code
- **Custom detection rules** — Organization-specific intent patterns

If no harness is configured or the harness doesn't register `IntentDetection`, context perception is skipped entirely.

| Intent | Triggered By | Description |
|--------|----------|-------------|
| **locate** | "where is", "find", "search for" | User wants to find files/functions |
| **understand** | "how does", "explain", "what does" | User wants to understand code |
| **retrieve** | "remember", "earlier", "previous" | User references past context |
| **explore** | "project structure", "what files" | User wants overview |
| **reason** | "why did", "why is", "cause" | User asks why something happened |
| **validate** | "verify", "check if", "debug" | User wants to verify correctness |
| **compare** | "difference between", "compare" | User wants comparison |
| **track** | "status", "progress", "history" | User asks for status |

### Context Perception (AHP 2.3)

When intent is detected, AHP fires `PreContextPerception` hooks. External harnesses can inject:

- **Facts** — relevant truths from knowledge bases
- **File contents** — snippets matching the query
- **Project summary** — structure, dependencies, patterns
- **Suggestions** — next steps, alternatives

The harness decides whether to inject context, modify it, or let default providers handle it.

### Agent Styles

Based on intent, the agent selects an operating style:

| Style | Use Case | Capabilities |
|-------|----------|-------------|
| `general` | Research, multi-step tasks | Full tool access |
| `explore` | Finding files, patterns | Read-only, fast |
| `plan` | Architecture, design | Read-only, no file changes |
| `verification` | Testing, debugging | Adversarial checks |
| `code_review` | Quality analysis | Read-only, focused |

---

## Built-in Tools

**15 built-in tools:**

| Category | Tools |
|----------|-------|
| Files | `read`, `write`, `edit`, `patch` |
| Search | `grep`, `glob`, `ls` |
| Shell | `bash` |
| Web | `web_fetch`, `web_search` |
| Git | `git_worktree` |
| Delegation | `task`, `parallel_task`, `run_team`, `batch` |

---

## Multi-Agent Teams

Coordinate multiple agents with role-based task assignment:

```python
from a3s_code import AgentTeam, TeamRole

team = AgentTeam(lead="general", workers=["explore", "verification"], reviewer="code_review")
result = await team.run("Refactor the auth module")
```

- **Lead** — decomposes goals, assigns tasks
- **Worker** — executes assigned tasks
- **Reviewer** — validates output, provides feedback

---

## AHP Protocol (Agent Harness Protocol)

AHP 2.3 provides **19 harness points** for external governance:

```python
from a3s_code import SessionOptions
from a3s_code.ahp import AhpHookExecutor, AhpTransport

ahp = AhpHookExecutor.new_with_config(
    AhpTransport.http("http://harness:8080/ahp", None),
    idle_threshold_ms=10_000
)

opts = SessionOptions()
opts.ahp_executor = ahp
session = agent.session("/workspace", opts)
```

Key harness capabilities:
- **IntentDetection** — classify user intent from every prompt (AHP 2.3)
- **PreAction / PostAction** — intercept tool calls
- **PrePrompt / PostResponse** — modify prompts and responses
- **ContextPerception** — inject context based on detected intent
- **Idle** — background consolidation when agent is idle
- **Confirmation** — human-in-the-loop for ambiguous operations
- **MemoryRecall** — query agent memory on demand

---

## Safety and Control

Agents run with **explicit permissions**. Nothing executes without policy:

```python
from a3s_code import SessionOptions, PermissionPolicy, PermissionRule

opts = SessionOptions()
opts.permission_policy = PermissionPolicy(
    allow=[PermissionRule("read(*)"), PermissionRule("grep(*)")],
    deny=[PermissionRule("bash(*)"), PermissionRule("write(*)")],
    default_decision="deny",
)
session = agent.session(".", opts)
```

Other safeguards:
- **Circuit breaker** — stops after 3 consecutive LLM failures
- **Auto-compact** — rolls up context before hitting token limits
- **Continuation injection** — prevents early stop mid-task (max 3 turns)
- **HITL confirmation** — prompt before destructive operations
- **Taint tracking** — detect injected malicious content

---

## Hooks — Lifecycle Events

Intercept and modify behavior at 12 event points:

```python
from a3s_code import SessionOptions, HookHandler

class MyHook(HookHandler):
    def pre_tool_use(self, tool_name, tool_input, ctx):
        if tool_name == "bash" and "rm -rf" in str(tool_input):
            return self.block("Refusing destructive command")
        return self.continue_()

    def pre_context_perception(self, intent, query, ctx):
        # Enrich with project-specific knowledge
        return self.inject_context({"facts": [...]})

opts = SessionOptions()
opts.hook_handler = MyHook()
session = agent.session(".", opts)
```

---

## Memory System

Four memory types persist across sessions:

```python
from a3s_code import SessionOptions, FileMemoryStore

opts = SessionOptions()
opts.memory_store = FileMemoryStore("./memory")
session = agent.session(".", opts)
```

| Type | What it stores |
|------|---------------|
| **Episodic** | Conversation history, tool interactions |
| **Semantic** | Facts, rules, learned patterns |
| **Procedural** | Skills, workflows, how-to knowledge |
| **Working** | Current task context, scratchpad |

---

## Skills

Markdown files that shape LLM behavior:

```markdown
---
name: safe-reviewer
description: Review code without modifying files
allowed-tools: "read(*), grep(*), glob(*)"
---

Review the code in the workspace. You may read and search files,
but you must not write, edit, or execute anything.
```

```python
opts = SessionOptions()
opts.skill_dirs = ["./skills"]
session = agent.session(".", opts)
```

Built-in skills: `agentic-search`, `code-search`, `code-review`, `explain-code`, `find-bugs`.

---

## MCP Integration

Connect to Model Context Protocol servers:

```hcl
mcp_servers = [
  {
    name = "filesystem"
    transport = "stdio"
    command = "npx"
    args = ["@modelcontextprotocol/server-filesystem", "./workspace"]
  }
]
```

---

## Slash Commands

Sessions intercept slash commands:

| Command | Description |
|---------|-------------|
| `/help` | List available commands |
| `/model [provider/model]` | Show or switch model |
| `/cost` | Show token usage |
| `/clear` | Clear conversation history |
| `/compact` | Manually trigger context compaction |
| `/btw <question>` | Ask side question (not in history) |
| `/loop [interval] <prompt>` | Schedule recurring prompt |
| `/cron-list` | List scheduled tasks |
| `/cron-cancel <id>` | Cancel scheduled task |

---

## Configuration

**HCL format:**

```hcl
default_model = "anthropic/claude-sonnet-4-20250514"

providers {
  name    = "anthropic"
  api_key = env("ANTHROPIC_API_KEY")
}

mcp_servers = []

skills = []
skill_dirs = ["./skills"]

# Security
permission_policy = "allow_all"  # or "deny_all", "custom"

# AHP harness
ahp = {
  enabled  = true
  url      = "http://harness:8080/ahp"
  idle_ms  = 10_000
}
```

---

## Architecture

```
Agent (facade — config-driven, workspace-independent)
  ├── LlmClient (Anthropic / OpenAI / compatible)
  ├── CodeConfig (HCL / JSON)
  ├── SessionManager (multi-session support)
  │     └── AgentSession (workspace-bound)
  │           └── AgentLoop (core execution engine)
  │                 ├── AHP IntentDetection → context perception (delegated to harness)
  │                 ├── ToolExecutor (15 tools)
  │                 ├── SkillRegistry
  │                 ├── HookEngine (12 events)
  │                 ├── AHP Executor (19 harness points)
  │                 ├── Memory (4 types)
  │                 ├── MCP Client
  │                 └── Security (permissions, taint, HITL)
```

**Extension points (20):** swap any component via traits — LLM client, tools, memory, hooks, permissions, confirmation, context providers, session store, skill registry, planner, MCP transport, HTTP client, and more.

---

## Documentation

Full reference and guides: **[a3s.dev/docs/code](https://a3s.dev/docs/code)**

- [Sessions](https://a3s.dev/docs/code/sessions)
- [Intent Detection](https://a3s.dev/docs/code/intent)
- [AHP Protocol](https://a3s.dev/docs/code/ahp)
- [Tools](https://a3s.dev/docs/code/tools)
- [Skills](https://a3s.dev/docs/code/skills)
- [Multi-Agent](https://a3s.dev/docs/code/teams)
- [Memory](https://a3s.dev/docs/code/memory)
- [Security](https://a3s.dev/docs/code/security)
- [Hooks](https://a3s.dev/docs/code/hooks)
- [Examples](https://a3s.dev/docs/code/examples)

---

## License

MIT
