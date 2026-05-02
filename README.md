# A3S Code

**A harness-driven runtime for coding agents.**

A3S Code is a Rust agent runtime with Python and Node.js bindings. It is built around a simple belief:

> A coding agent becomes reliable when the harness controls context, actions, safety, and verification.

The model should reason. The harness should decide what context is load-bearing, which tools are visible, which actions are safe, and how completion is verified.

[![crates.io](https://img.shields.io/crates/v/a3s-code-core)](https://crates.io/crates/a3s-code-core)
[![PyPI](https://img.shields.io/pypi/v/a3s-code)](https://pypi.org/project/a3s-code/)
[![npm](https://img.shields.io/npm/v/@a3s-lab/code)](https://www.npmjs.com/package/@a3s-lab/code)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue)](LICENSE)

---

## Why

Most coding agents fail for boring reasons:

- too many tools are injected into every prompt
- raw search results, test logs, and subagent transcripts flood the context
- memory, skills, MCP, hooks, and project hints all inject context through separate paths
- safety is split across permissions, confirmations, skills, and custom guards
- agents stop after "I changed it" instead of proving the change works

A3S Code treats the agent as an execution system:

```text
Intent -> Context -> Action -> Observation -> Verification -> Compaction
```

Everything else is an extension of that loop.

---

## Install

```bash
# Python
pip install a3s-code

# Node.js
npm install @a3s-lab/code
```

Rust users can depend on `a3s-code-core`.

---

## Quick Start

Create `agent.acl`:

```acl
default_model = "anthropic/claude-sonnet-4-20250514"

providers "anthropic" {
  apiKey = env("ANTHROPIC_API_KEY")
}
```

Python:

```python
from a3s_code import Agent

agent = Agent.create("agent.acl")
session = agent.session("/my-project")

result = session.send("Find where authentication errors are handled and summarize the flow")
print(result.text)
```

Node.js:

```typescript
import { Agent } from '@a3s-lab/code';

const agent = await Agent.create('agent.acl');
const session = agent.session('/my-project');

const result = await session.send('Find where authentication errors are handled and summarize the flow');
console.log(result.text);

session.close();
```

---

## Design Principles

### 1. Small Kernel

The core runtime should do only the irreversible work:

- maintain the agent loop
- call the LLM
- expose selected actions
- execute actions through a single executor
- record observations
- compact state when needed
- return events and results

Advanced capabilities belong in the harness, not in the kernel.

### 2. Context Is Budgeted

The model should see the smallest useful context for the current decision.

All context sources should eventually flow through one assembler:

```text
AGENTS.md
skills
memory
file search
MCP
AHP
subagents
tool observations
        -> ContextItem
        -> rank
        -> dedupe
        -> budget
        -> render
```

Raw logs, full grep output, and complete subagent transcripts should be stored as artifacts or trace data, not repeatedly injected into the prompt.

### 3. Tools Are Selected, Not Dumped

A3S Code keeps a full tool registry, but the model only sees tools relevant to the current turn.

Default core tools:

| Category | Tools |
|----------|-------|
| Files | `read`, `write`, `edit`, `patch` |
| Search | `grep`, `glob`, `ls` |
| Shell | `bash` |
| Delegation | `task`, `parallel_task` |
| Skills | `search_skills`, `Skill` |

Intent-gated tools:

| Category | Tools |
|----------|-------|
| Web | `web_fetch`, `web_search` |
| Git | `git` |
| Batch | `batch` |
| Skill admin | `manage_skill` |
| External | MCP tools |

This follows the same direction as modern agent harnesses: remove routine tool clutter from the model's context and expose capabilities only when the task asks for them.

### 4. Programmatic Tool Calling

High-frequency tool chains should move out of the LLM loop.

Instead of forcing the model through:

```text
grep -> read -> grep -> read -> summarize
```

the harness can run a bounded JavaScript program in the embedded QuickJS VM:

```js
const result = await session.program({
  source: `
    export default async function run(ctx, inputs) {
      const hits = await ctx.grep(inputs.query, { glob: '*.rs' });
      const files = await ctx.glob('crates/**/*.rs');
      return { hits, files: files.slice(0, 20) };
    }
  `,
  inputs: { query: 'PermissionPolicy' },
  allowedTools: ['grep', 'glob'],
  limits: { timeoutMs: 30000, maxToolCalls: 20, maxOutputBytes: 65536 },
});
```

The same capability is available from Python with `session.program({...})` and
from Rust by calling the core `program` tool. If an allow-list is omitted, the
script can call every registered tool except `program`; use `allowedTools` or
`allowed_tools` to narrow the surface. Programmatic tools should return
structured summaries, findings, artifact references, and suggested next actions.
Raw output belongs in trace storage.

### 5. Subagents Isolate Context

Subagents are not there to create more chat. They isolate local work.

The parent agent delegates:

```text
task(role, prompt, budget)
parallel_task(tasks)
```

Subagents should return:

- summary
- key findings
- files inspected or changed
- evidence references
- risks
- confidence
- trace reference

The parent should not ingest the full child transcript.

### 6. Safety Has One Gate

All side effects should pass through one authorization path.

Policies may be composed from workspace boundaries, permissions, confirmations, skill grants, and security providers, but execution should observe one effective decision:

```text
Allow | Ask | Deny
```

This keeps `bash`, writes, network calls, MCP calls, and release actions auditable.

### 7. Completion Requires Verification

A coding agent is not done because it produced text. It is done when the goal is satisfied and the result has been checked.

Verification can include:

- unit tests
- type checks
- lint
- command output
- git diff review
- subagent review
- explicit residual risk reporting

---

## Architecture

Current public API:

```text
Agent
  -> AgentSession
     -> ToolSelector
        -> ToolExecutor
        -> SkillRegistry
        -> Context providers
        -> Permission / confirmation
        -> Compaction
        -> Events
```

Target harness architecture:

```text
a3s-code
├── runtime kernel
│   ├── internal agent loop
│   ├── state
│   ├── events
│   └── trace
│
├── harness
│   ├── intent router
│   ├── context assembler
│   ├── tool selector
│   ├── program executor
│   ├── safety gate
│   ├── verification loop
│   └── compaction engine
│
├── capabilities
│   ├── core tools
│   ├── skills
│   ├── MCP
│   ├── memory
│   ├── web
│   └── git
│
├── delegation
│   ├── task
│   ├── parallel_task
│   └── workflow plugins
│
├── advanced control
│   ├── session-level lane queues for external/hybrid dispatch
│   └── SubAgent lifecycle control
│
└── API
    ├── Rust
    ├── Python
    └── Node.js
```

The long-term direction is a small runtime kernel with powerful harness extensions.

---

## Skills

Skills are loaded on demand. A3S Code exposes `search_skills` so the model can discover relevant skills without injecting every skill description into the prompt.

Example skill:

```markdown
---
name: safe-reviewer
description: Review code without modifying files
allowed-tools: "read(*), grep(*), glob(*)"
---

Review the code in the workspace. Focus on correctness, regressions, and missing tests.
Do not modify files.
```

Use custom skill directories:

```python
from a3s_code import SessionOptions

opts = SessionOptions()
opts.skill_dirs = ["./skills"]
session = agent.session(".", opts)
```

Built-in skills include code search, code review, explanation, and bug finding helpers.

---

## Delegation

Use delegation when a task benefits from context isolation.

Core delegation primitives:

- `task` — run one focused subagent
- `parallel_task` — run independent subagents concurrently

The older model-visible `run_team` tool is intentionally removed in 2.0.
Multi-agent work should enter through the delegation core. The duplicate
`Orchestrator.runTeam()` shortcut from 1.0 is removed.

`Orchestrator` remains an advanced control-plane API for monitoring and
controlling long-running real SubAgents from an `Agent`. It is not the default
multi-agent composition mechanism, and placeholder SubAgent execution is not
part of 2.0.

Optional lane queues are also outside the default path. They are for explicit
external/hybrid dispatch, priority experiments, and operational integrations;
ordinary sessions are queue-free unless a session queue configuration is supplied.
They are not part of the Orchestrator/SubAgent control plane.

---

## AHP Integration

AHP, the Agent Harness Protocol, is best treated as a harness extension.

It should observe runtime events and provide suggestions:

- add or boost context
- enable an action
- require confirmation
- request compaction
- provide policy hints

Those suggestions should flow through the same systems as everything else:

```text
AHP suggestion
  -> ContextAssembler
  -> ToolSelector
  -> SafetyGate
  -> CompactionEngine
```

AHP should not bypass context budgets or directly stuff prompt text into the model.

Example:

```python
from a3s_code import SessionOptions
from a3s_code.ahp import AhpHookExecutor, AhpTransport

ahp = AhpHookExecutor.new_with_config(
    AhpTransport.http("http://harness:8080/ahp", None),
    idle_threshold_ms=10_000,
)

opts = SessionOptions()
opts.ahp_executor = ahp
session = agent.session("/workspace", opts)
```

---

## Memory

Memory is optional evidence, not automatic prompt stuffing.

Recommended model:

| Layer | Purpose |
|-------|---------|
| Conversation summary | Preserve load-bearing state across long sessions |
| Working memory | Current task state |
| Long-term memory | Optional retrievable evidence across sessions |

Enable persistent memory when your product needs it:

```python
from a3s_code import SessionOptions, FileMemoryStore

opts = SessionOptions()
opts.memory_store = FileMemoryStore("./memory")
session = agent.session(".", opts)
```

---

## Safety

Configure explicit permissions:

```python
from a3s_code import SessionOptions, PermissionPolicy

opts = SessionOptions()
opts.permission_policy = PermissionPolicy(
    allow=["read(*)", "grep(*)"],
    deny=["bash(*)", "write(*)"],
    default_decision="deny",
)

session = agent.session(".", opts)
```

Built-in safeguards include:

- permission policies
- human-in-the-loop confirmation
- workspace-scoped tool context
- tool timeouts
- duplicate tool-call protection
- LLM circuit breaker
- context compaction
- output sanitization hooks

---

## MCP

Connect to Model Context Protocol servers when external capabilities are needed:

```acl
mcp_servers = [
  {
    name = "filesystem"
    transport = "stdio"
    command = "npx"
    args = ["@modelcontextprotocol/server-filesystem", "./workspace"]
  }
]
```

MCP tools are selected per turn instead of being listed wholesale in the system prompt.

---

## Slash Commands

Sessions support slash commands:

| Command | Description |
|---------|-------------|
| `/help` | List available commands |
| `/model [provider/model]` | Show or switch model |
| `/cost` | Show token usage |
| `/clear` | Clear conversation history |
| `/compact` | Manually trigger context compaction |
| `/btw <question>` | Ask a side question without polluting history |

---

## Configuration

The config language is ACL. Config files use the `.acl` extension and labeled
blocks such as `providers "anthropic" { ... }`.

```acl
default_model = "anthropic/claude-sonnet-4-20250514"

providers "anthropic" {
  apiKey = env("ANTHROPIC_API_KEY")
}

skill_dirs = ["./skills"]
mcp_servers = []

ahp = {
  enabled = true
  url     = "http://harness:8080/ahp"
  idle_ms = 10_000
}
```

---

## Development

```bash
cargo check -p a3s-code-core
cargo test -p a3s-code-core
cargo clippy -p a3s-code-core -- -D warnings
```

Build language bindings individually:

```bash
cargo build -p a3s-code-py
cargo build -p a3s-code-node
```

---

## Documentation

Full reference and guides: [a3s.dev/docs/code](https://a3s.dev/docs/code)

- [Sessions](https://a3s.dev/docs/code/sessions)
- [AHP Protocol](https://a3s.dev/docs/code/ahp)
- [Tools](https://a3s.dev/docs/code/tools)
- [Skills](https://a3s.dev/docs/code/skills)
- [Memory](https://a3s.dev/docs/code/memory)
- [Security](https://a3s.dev/docs/code/security)
- [Hooks](https://a3s.dev/docs/code/hooks)
- [Examples](https://a3s.dev/docs/code/examples)

---

## License

MIT
