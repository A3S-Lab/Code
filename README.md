# A3S Code

**Rust framework for building agentic AI applications** — Embed agents that read, write, and execute code into any application. Tool calling, task planning, safety controls, multi-machine distribution — native Node.js and Python bindings included.

[![Crates.io](https://img.shields.io/crates/v/a3s-code-core.svg)](https://crates.io/crates/a3s-code-core)
[![Documentation](https://docs.rs/a3s-code-core/badge.svg)](https://docs.rs/a3s-code-core)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](./LICENSE)
[![Tests](https://img.shields.io/badge/tests-1477%20passing-brightgreen.svg)](./core/tests)

---

## Why A3S Code?

- **Embeddable** — Rust library, not a service. Node.js and Python bindings included. CLI for terminal use.
- **Safe by Default** — Permission system, HITL confirmation, skill-based tool restrictions, and error recovery (parse retries, tool timeout, circuit breaker).
- **Extensible** — 20 trait-based extension points, all with working defaults. Slash commands, tool search, and multi-agent teams.
- **Scalable** — Lane-based priority queue with multi-machine task distribution.

---

## Quick Start

### 1. Install

```bash
# Node.js
npm install @a3s-lab/code

# Python
pip install a3s-code
```

### 2. Configure

Create `agent.hcl`:

```hcl
default_model = "anthropic/claude-sonnet-4-20250514"

providers {
  name    = "anthropic"
  api_key = env("ANTHROPIC_API_KEY")
}
```

### 3. Use

**TypeScript**

```typescript
import { Agent, DefaultSecurityProvider } from '@a3s-lab/code';

const agent = await Agent.create('agent.hcl');
const session = agent.session('.', {
  securityProvider: new DefaultSecurityProvider(),
  builtinSkills: true,
  planning: true,
});

const result = await session.send('Refactor auth + update tests');
console.log(result.text);
```

**Python**

```python
from a3s_code import Agent, SessionOptions, DefaultSecurityProvider

agent = Agent("agent.hcl")
opts = SessionOptions()
opts.security_provider = DefaultSecurityProvider()
opts.builtin_skills = True
session = agent.session(".", opts, planning=True)

result = session.send("Refactor auth + update tests")
print(result.text)
```

---

## Core Features

### 🛠️ Built-in Tools (15)

| Category                     | Tools                                    | Description                                                                   |
| ---------------------------- | ---------------------------------------- | ----------------------------------------------------------------------------- |
| **File Operations**    | `read`, `write`, `edit`, `patch` | Read/write files, apply diffs                                                 |
| **Search**             | `grep`, `glob`, `ls`, `agentic_search` | Search content, find files, list directories, intelligent multi-phase code search |
| **Execution**          | `bash`                                 | Execute shell commands                                                        |
| **Web**                | `web_fetch`, `web_search`            | Fetch URLs, search the web                                                    |
| **Git**                | `git_worktree`                         | Create/list/remove/status git worktrees for parallel work                     |
| **Subagents**          | `task`                                 | Delegate to a named agent; blocks until the child agent replies               |
| **Parallel subagents** | `parallel_task`                        | Fan-out to multiple named agents concurrently                                 |
| **Team workflow**      | `run_team`                             | Lead → Worker → Reviewer team with dynamic decomposition and quality review |
| **Parallel tools**     | `batch`                                | Execute multiple tools concurrently in one call                               |

---

### 🔒 Security & Safety

**Permission System** — Allow/Deny/Ask rules per tool with wildcard matching. Configure via `PermissionPolicy` (Rust) or `SessionOptions.permissions` (TypeScript/Python).

**Default Security Provider** — Auto-redact PII (SSN, API keys, emails, credit cards), prompt injection detection, SHA256 hashing. Enable via `securityProvider: new DefaultSecurityProvider()` (TypeScript) or `opts.security_provider = DefaultSecurityProvider()` (Python).

**HITL Confirmation** — Require human approval for sensitive operations. Configure via `ConfirmationManager` with `ConfirmationPolicy.enabled()`.

**Skill-Based Tool Restrictions** — Skills define allowed tools via `allowed-tools` field, enforced at execution time.

---

### 🧠 Skills System (Claude Code Compatible)

7 built-in skills (4 code assistance + 3 tool documentation). Custom skills are Markdown files with YAML frontmatter:

```markdown
---
name: api-design
description: Review API design for RESTful principles
allowed-tools: "read(*), grep(*)"
kind: instruction
tags: [api, design]
version: 1.0.0
---
# API Design Review
Check for RESTful principles, naming conventions, error handling.
```

Enable via `builtinSkills: true` (TypeScript) or `opts.builtin_skills = True` (Python). Load custom skills with `skillsDirs: ['./skills']` or `opts.skills_dirs = ['./skills']`.

---

### 🎯 Planning & Goal Tracking

Decompose complex tasks into dependency-aware execution plans with wave-based parallel execution:

Enable via `planning: true` (TypeScript) or `opts.planning = True` (Python). Goal tracking monitors progress across multiple turns with `goalTracking: true`.

The planner creates steps with dependencies. Independent steps execute in parallel waves via `tokio::JoinSet`. Goal tracking monitors progress across multiple turns.

---

### 🖼️ Multi-Modal Support (Vision)

Send image attachments alongside text prompts. Requires a vision-capable model (Claude Sonnet, GPT-4o).

Supported formats: JPEG, PNG, GIF, WebP. Image data is base64-encoded for both Anthropic and OpenAI providers.

---

### ⚡ Slash Commands

Interactive session commands dispatched before the LLM. Custom commands via the `SlashCommand` trait:

| Command                       | Description                                                 |
| ----------------------------- | ----------------------------------------------------------- |
| `/help`                     | List available commands                                     |
| `/compact`                  | Manually trigger context compaction                         |
| `/cost`                     | Show token usage and estimated cost                         |
| `/model`                    | Show or switch the current model                            |
| `/clear`                    | Clear conversation history                                  |
| `/history`                  | Show conversation turn count and token stats                |
| `/tools`                    | List registered tools                                       |
| `/mcp`                      | List connected MCP servers and their tools                  |
| `/loop [interval] <prompt>` | Schedule a recurring prompt (e.g.,`/loop 5m check build`) |
| `/cron-list`                | List all scheduled recurring prompts                        |
| `/cron-cancel <id>`         | Cancel a scheduled task by ID                               |

### Programmatic Scheduler API (SDK)

```typescript
// TypeScript
const commands = session.listCommands();   // CommandInfo[]

const taskId = session.scheduleTask('check deployment status', 30);   // every 30s
const tasks  = session.listScheduledTasks();   // ScheduledTaskInfo[]
const ok     = session.cancelScheduledTask(taskId);   // boolean

// Also via slash commands:
await session.send('/loop 5m summarize recent commits');
await session.send('/cron-list');
await session.send(`/cron-cancel ${taskId}`);
```

```python
# Python
commands = session.list_commands()   # list[dict] — name, description, usage
session.register_command("status", "Show session status",
    lambda args, ctx: f"Model: {ctx['model']}, History: {ctx['history_len']} msgs")

task_id = session.schedule_task("check deployment status", 30)   # every 30s
tasks   = session.list_scheduled_tasks()   # list[dict] — id, prompt, interval_secs, fire_count, next_fire_in_secs
ok      = session.cancel_scheduled_task(task_id)   # True

# Also via slash commands:
session.send("/loop 5m summarize recent commits")
session.send("/cron-list")
session.send(f"/cron-cancel {task_id}")
```

---

### 🔍 Tool Search (Per-Turn Filtering)

When the MCP ecosystem grows large (100+ tools), injecting all tool descriptions wastes context. Tool Search selects only relevant tools per-turn based on keyword matching. Configure via `AgentConfig::tool_index` (Rust) — the agent loop extracts the last user message, searches the index, and only sends matching tools to the LLM. Builtin tools are always included.

When configured via `AgentConfig::tool_index`, the agent loop extracts the last user message, searches the index, and only sends matching tools to the LLM. Builtin tools are always included.

---

### 🔌 MCP Integration

Connect external tool servers via the [Model Context Protocol](https://modelcontextprotocol.io/). MCP tools are namespaced as `mcp__{server}__{tool}` to avoid collisions with built-in tools. Both global (config-level) and per-session servers are merged into a single `ToolExecutor` — the LLM sees all tools in one flat list.

**HCL config (global — loaded at agent startup, connected once):**

```hcl
mcp_servers {
  name    = "filesystem"
  command = "npx"
  args    = ["-y", "@modelcontextprotocol/server-filesystem", "/workspace"]
}

mcp_servers {
  name    = "github"
  command = "npx"
  args    = ["-y", "@modelcontextprotocol/server-github"]
  env     = { GITHUB_TOKEN = env("GITHUB_TOKEN") }
}
```

**Dynamic registration (per-session, runtime — no restart needed):**

**TypeScript**

```typescript
const session = agent.session('.');

// Connect and inject tools (default 60s timeout)
const count = await session.addMcpServer(
  'filesystem', 'stdio', 'npx',
  ['-y', '@modelcontextprotocol/server-filesystem', '/tmp'],
);

// Connect with custom timeout (in milliseconds)
const count2 = await session.addMcpServer(
  'github', 'stdio', 'npx',
  ['-y', '@modelcontextprotocol/server-github'],
  { timeoutMs: 120000 },  // 120 seconds
);

// Status + tools
const status = await session.mcpStatus();   // McpServerStatusEntry[]
const tools = session.toolNames();          // string[]

// Remove
await session.removeMcpServer('filesystem');

// Refresh global cache
await agent.refreshMcpTools();
```

**Python**

```python
session = agent.session(".")

# Connect and inject tools (default 60s timeout)
count = session.add_mcp_server(
    "filesystem",
    command="npx",
    args=["-y", "@modelcontextprotocol/server-filesystem", "/tmp"],
)

# Connect with custom timeout (in milliseconds)
count2 = session.add_mcp_server(
    "github",
    command="npx",
    args=["-y", "@modelcontextprotocol/server-github"],
    timeout_ms=120000,  # 120 seconds
)

# Status + tools
status = session.mcp_status()   # {name: {connected, tool_count, error}}
tools  = session.tool_names()   # list[str]

# Remove
session.remove_mcp_server("filesystem")

# Refresh global cache
agent.refresh_mcp_tools()
```

---

### 🔄 Dynamic Agent Registration

Load custom agent definitions into a running session without restarting. Useful for hot-reloading agent configurations or dynamically extending agent capabilities at runtime.

**TypeScript**

```typescript
const session = agent.session('.');

// Load all .hcl agent definitions from a directory
const count = session.registerAgentDir('./custom-agents');
console.log(`Loaded ${count} agents`);

// Now the session can spawn these agents via the Agent tool
// or use them in team workflows
```

**Python**

```python
session = agent.session(".")

# Load all .hcl agent definitions from a directory
count = session.register_agent_dir("./custom-agents")
print(f"Loaded {count} agents")

# Now the session can spawn these agents via the Agent tool
# or use them in team workflows
```

**Agent definition format** (`.hcl` files in the target directory):

```hcl
agent "custom-reviewer" {
  description = "Code review specialist"
  model       = "claude-sonnet-4-6"

  system_prompt = <<-EOT
    You are a code reviewer focused on security and performance.
    Review code for common vulnerabilities and optimization opportunities.
  EOT
}
```

The `register_agent_dir()` method scans the directory for `.hcl` files, parses agent definitions, and registers them in the session's `AgentRegistry`. These agents become immediately available for spawning via the `Agent` tool or for use in multi-agent team workflows.

---

### ⚡ Direct Queue Submission (Parallel Tasks)

Inject tasks directly into the lane queue from SDK code — bypassing the LLM turn. Useful for proactively scheduling work or fan-out patterns:

**TypeScript**

```typescript
// Single task
const result = await session.submit('execute', { command: 'cargo test' });

// Batch (parallel for query lane)
const results = await session.submitBatch('query', [
  { path: 'src/main.rs' },
  { path: 'src/lib.rs' },
]);
```

**Python**

```python
# Single task
result = session.submit("execute", {"command": "cargo test"})

# Batch (parallel for query lane)
results = session.submit_batch("query", [
    {"path": "src/main.rs"},
    {"path": "src/lib.rs"},
])
```

Lane priority: `control` (P0) > `query` (P1) > `execute` (P2) > `generate` (P3). Query-lane batches execute in parallel; all other lanes are sequential.

---

### 👥 Agent Teams (Multi-Agent Coordination)

Automated Lead → Worker → Reviewer workflows with real LLM execution:

**How it works:**

1. **Lead** decomposes the goal into a JSON task list via LLM
2. **Workers** concurrently claim and execute tasks (each via its own `AgentSession`)
3. **Reviewer** inspects completed work — APPROVED moves the task to Done, REJECTED re-queues it for retry
4. Loop continues until all tasks are Done or `max_rounds` is reached

**TypeScript**

```typescript
import { Agent, Team, TeamRunner, TeamConfig } from '@a3s-lab/code';

const agent = await Agent.create('agent.hcl');

const config: TeamConfig = { maxTasks: 50, channelBuffer: 128, maxRounds: 10, pollIntervalMs: 200 };
const team = new Team('refactor-auth', config);
team.addMember('lead', 'lead');
team.addMember('worker-1', 'worker');
team.addMember('reviewer', 'reviewer');

const runner = new TeamRunner(team);

// Option A: bind pre-built sessions
runner.bindSession('lead',     agent.session('.'));
runner.bindSession('worker-1', agent.session('.'));
runner.bindSession('reviewer', agent.session('.'));

// Option B: bind from Agent + agent definition (auto-loads AgentRegistry from agentDirs)
// runner.bindAgent('lead',     agent, '.', 'lead-agent',    ['./agents']);
// runner.bindAgent('worker-1', agent, '.', 'general',       []);
// runner.bindAgent('reviewer', agent, '.', 'code-reviewer', []);

const result = await runner.runUntilDone('Refactor auth module to use JWT');
console.log(`Done: ${result.doneTasks.length} tasks, ${result.rounds} rounds`);
for (const task of result.doneTasks) {
  console.log(`  [${task.id}] ${task.description}\n  → ${task.result}`);
}
```

**Python**

```python
from a3s_code import Agent, Team, TeamRunner, TeamConfig

agent = Agent.create("agent.hcl")

config = TeamConfig(max_rounds=10, poll_interval_ms=200)
team = Team("refactor-auth", config)
team.add_member("lead", "lead")
team.add_member("worker-1", "worker")
team.add_member("reviewer", "reviewer")

runner = TeamRunner(team)

# Option A: bind pre-built sessions
runner.bind_session("lead",     agent.session("."))
runner.bind_session("worker-1", agent.session("."))
runner.bind_session("reviewer", agent.session("."))

# Option B: bind from Agent + agent definition (auto-loads AgentRegistry from agent_dirs)
# runner.bind_agent("lead",     agent, ".", "lead-agent",    ["./agents"])
# runner.bind_agent("worker-1", agent, ".", "general",       [])
# runner.bind_agent("reviewer", agent, ".", "code-reviewer", [])

result = runner.run_until_done("Refactor auth module to use JWT")
print(f"Done: {len(result.done_tasks)} tasks, {result.rounds} rounds")
for task in result.done_tasks:
    print(f"  [{task.id}] {task.description}\n  → {task.result}")
```

Supports Lead/Worker/Reviewer roles, `mpsc` peer messaging, broadcast, and a full task lifecycle (Open → InProgress → InReview → Done/Rejected).

**`run_team` built-in tool** — The LLM can also trigger the same Lead → Worker → Reviewer workflow autonomously at runtime via the `run_team` tool (no SDK wiring required):

```python
# Python — call directly from SDK code
result = session.tool("run_team", {
    "goal": "Audit the auth module for security issues and produce a remediation plan",
    "max_steps": 10,  # per-member agent; lead/worker/reviewer all default to "general"
})
print(result.output)
```

```typescript
// TypeScript — call directly from SDK code
const result = await session.tool('run_team', {
  goal: 'Audit the auth module for security issues and produce a remediation plan',
  maxSteps: 10,
});
console.log(result.output);
```

The LLM can also call `run_team` on its own when the `delegate-task` skill is loaded — it selects it automatically for goals with an unknown number of subtasks or that require reviewer sign-off.

---

### 🎛️ Agent Orchestrator (Master-SubAgent Coordination)

Spawn, monitor, and dynamically control multiple SubAgents from a central coordinator with a real-time event bus. Supports **External Lane Dispatch** — route individual tool calls to remote workers while the orchestrator coordinates SubAgents in parallel.

| Event                              | When                                          |
| ---------------------------------- | --------------------------------------------- |
| `SubAgentStarted/Completed`      | SubAgent lifecycle                            |
| `SubAgentProgress`               | Each tool-call step                           |
| `ToolExecutionStarted/Completed` | Individual tool lifecycle                     |
| `ExternalTaskPending`            | Tool waiting for external worker              |
| `ExternalTaskCompleted`          | External result delivered, SubAgent unblocked |
| `ControlSignalReceived/Applied`  | Pause / resume / cancel                       |

**SDK shorthand** — `Orchestrator.create()` + `AgentSlot` for simpler multi-agent patterns:

**TypeScript**

```typescript
import { Agent, Orchestrator, AgentSlot } from '@a3s-lab/code';

const agent = await Agent.create('agent.hcl');
const orch = Orchestrator.create(agent);

// Spawn a single subagent by slot definition
const slot: AgentSlot = {
  agentType: 'general',
  prompt: 'Summarize the authentication module in 3 bullet points.',
  description: 'Auth summarizer',
  permissive: true,
  maxSteps: 5,
};
const handle = orch.spawn(slot);
const result = handle.wait();
console.log(result.output);

// Or run a full Lead → Worker → Reviewer team via AgentSlot array
const slots: AgentSlot[] = [
  { agentType: 'general', role: 'lead',     prompt: '', description: 'Lead',     permissive: true, maxSteps: 5 },
  { agentType: 'general', role: 'worker',   prompt: '', description: 'Worker',   permissive: true, maxSteps: 5 },
  { agentType: 'general', role: 'reviewer', prompt: '', description: 'Reviewer', permissive: true, maxSteps: 3 },
];
const teamResult = await orch.runTeam(
  'Audit the auth module for common security issues',
  '.',
  slots,
);
console.log(`Done: ${teamResult.doneTasks.length} tasks, ${teamResult.rounds} rounds`);
```

**Python**

```python
from a3s_code import Agent, Orchestrator, AgentSlot

agent = Agent.create('agent.hcl')
orch = Orchestrator.create(agent)

# Spawn a single subagent
slot = AgentSlot(
    agent_type='general',
    prompt='Summarize the authentication module in 3 bullet points.',
    description='Auth summarizer',
    permissive=True,
    max_steps=5,
)
handle = orch.spawn(slot)
result = handle.wait()
print(result.output)

# Or run a full Lead → Worker → Reviewer team
slots = [
    AgentSlot(agent_type='general', role='lead',     prompt='', description='Lead',     permissive=True, max_steps=5),
    AgentSlot(agent_type='general', role='worker',   prompt='', description='Worker',   permissive=True, max_steps=5),
    AgentSlot(agent_type='general', role='reviewer', prompt='', description='Reviewer', permissive=True, max_steps=3),
]
result = orch.run_team(
    'Audit the auth module for common security issues',
    '.',
    slots,
)
print(f"Done: {len(result.done_tasks)} tasks, {result.rounds} rounds")
```

---

### 🎨 System Prompt Customization (Slot-Based)

Customize the agent's behavior without overriding the core agentic capabilities. The default prompt (tool usage strategy, agentic behavior, completion criteria) is always preserved:

| Slot               | Position         | Behavior                                        |
| ------------------ | ---------------- | ----------------------------------------------- |
| `role`           | Before core      | Replaces default "You are A3S Code..." identity |
| `guidelines`     | After core       | Appended as `## Guidelines` section           |
| `response_style` | Replaces section | Replaces default `## Response Format`         |
| `extra`          | End              | Freeform instructions (backward-compatible)     |

**TypeScript**

```typescript
const session = agent.session('.', {
  role: 'You are a senior Rust developer',
  guidelines: 'Use clippy. No unwrap(). Prefer Result.',
  responseStyle: 'Be concise. Use bullet points.',
  extra: 'This project uses tokio and axum.',
});
```

**Python**

```python
opts = SessionOptions()
opts.role = "You are a senior Rust developer"
opts.guidelines = "Use clippy. No unwrap(). Prefer Result."
opts.response_style = "Be concise. Use bullet points."
opts.extra = "This project uses tokio and axum."
session = agent.session(".", opts)
```

---

### 💻 CLI (Terminal Agent)

Interactive AI coding agent in the terminal:

```bash
# Install
cargo install a3s-code-cli

# Interactive REPL
a3s-code

# One-shot mode
a3s-code "Explain the auth module"

# Custom config
a3s-code -c agent.hcl -m openai/gpt-4o "Fix the tests"
```

Config auto-discovery: `-c` flag → `A3S_CONFIG` env → `~/.a3s/config.hcl` → `./agent.hcl`

---

### 🚦 Lane-Based Priority Queue

Tool execution is routed through a priority queue backed by [a3s-lane](../lane):

| Lane               | Priority     | Tools                                       | Behavior   |
| ------------------ | ------------ | ------------------------------------------- | ---------- |
| **Control**  | P0 (highest) | pause, resume, cancel                       | Sequential |
| **Query**    | P1           | read, glob, grep, ls, web_fetch, web_search | Parallel   |
| **Execute**  | P2           | bash, write, edit, delete                   | Sequential |
| **Generate** | P3 (lowest)  | LLM calls                                   | Sequential |

Higher-priority tasks preempt queued lower-priority tasks. Configure per-lane concurrency via `SessionQueueConfig` with `query_max_concurrency`, `execute_max_concurrency`, `enable_metrics`, and more.

Advanced features: retry policies, rate limiting, priority boost, pressure monitoring, DLQ.

---

### 🔒 Sandbox Execution (A3S Box Integration)

Route `bash` commands through an A3S Box MicroVM for isolated execution. No Cargo feature flag required — the host application supplies a concrete `BashSandbox` implementation via `with_sandbox_handle()`.

**Transparent routing** — configure once via `with_sandbox_handle(Arc::new(my_sandbox_impl))`, and the `bash` tool routes through the sandbox automatically. Any type implementing the `BashSandbox` trait works. SafeClaw ships an A3S Box–backed implementation; supply your own for other environments.

The workspace directory is mounted at `/workspace` inside the MicroVM. See [`BashSandbox`](./core/src/sandbox.rs) for the trait definition.

---

### 🌐 Multi-Machine Distribution

Offload tool execution to external workers via three handler modes:

| Mode                         | Behavior                                       |
| ---------------------------- | ---------------------------------------------- |
| **Internal** (default) | Execute within agent process                   |
| **External**           | Send to external workers, wait for completion  |
| **Hybrid**             | Execute internally + notify external observers |

Switch a lane to External mode via `session.set_lane_handler(SessionLane::Execute, LaneHandlerConfig { mode: TaskHandlerMode::External, .. })`. Workers poll `session.pending_external_tasks()` and call `session.complete_external_task()` to return results.

---

### 🔌 Extensibility (20 Extension Points)

All policies are replaceable via traits with working defaults:

| Extension Point      | Purpose                                        | Default                   |
| -------------------- | ---------------------------------------------- | ------------------------- |
| SecurityProvider     | Input taint, output sanitization               | DefaultSecurityProvider   |
| PermissionChecker    | Tool access control                            | PermissionPolicy          |
| ConfirmationProvider | Human confirmation                             | ConfirmationManager       |
| ContextProvider      | RAG retrieval                                  | RipgrepContextProvider    |
| SessionStore         | Session persistence                            | FileSessionStore          |
| MemoryStore          | Long-term memory backend (from `a3s-memory`) | InMemoryStore             |
| Tool                 | Custom tools                                   | 14 built-in tools         |
| Planner              | Task decomposition                             | LlmPlanner                |
| HookHandler          | Event handling                                 | HookEngine                |
| HookExecutor         | Event execution                                | HookEngine                |
| McpTransport         | MCP protocol                                   | StdioTransport            |
| HttpClient           | HTTP requests                                  | ReqwestClient             |
| SessionCommand       | Queue tasks                                    | ToolCommand               |
| LlmClient            | LLM interface                                  | Anthropic/OpenAI          |
| BashSandbox          | Shell execution isolation                      | LocalBashExecutor         |
| SkillValidator       | Skill activation logic                         | DefaultSkillValidator     |
| SkillScorer          | Skill relevance ranking                        | DefaultSkillScorer        |
| DocumentParser       | Custom file format parsing for agentic_search  | None (plain text only)    |

Implement any trait and inject via `SessionOptions` builder methods (e.g., `with_security_provider`, `with_permission_checker`, `with_session_store`).

### 📄 Document Parser Extension

`agentic_search` works with plain text by default. To enable PDF, Excel, Word, or other binary formats, implement the `DocumentParser` trait and register it:

```rust
use a3s_code_core::tools::document_parser::{DocumentParser, DocumentParserRegistry};

struct PdfParser;

impl DocumentParser for PdfParser {
    fn name(&self) -> &str { "pdf" }
    fn supported_extensions(&self) -> &[&str] { &["pdf"] }
    fn parse(&self, path: &Path) -> anyhow::Result<String> {
        // use pdf-extract or similar
        todo!()
    }
}

let mut registry = DocumentParserRegistry::new();
registry.register(Arc::new(PdfParser));
let tool = AgenticSearchTool::with_parser_registry(registry);
```

---

## Architecture

5 core components (stable, not replaceable) + 20 extension points (replaceable via traits):

```
Agent (config-driven)
  ├── CommandRegistry (slash commands: /help, /model, /loop, /cron-list, /cron-cancel, ...)
  │     └── CronScheduler (session-scoped recurring prompts, lazy-start background ticker)
  └── AgentSession (workspace-bound)
        ├── AgentLoop (core execution engine)
        │     ├── ToolExecutor (15 built-in tools, batch parallel execution)
        │     ├── ToolIndex (per-turn tool filtering for large MCP sets)
        │     ├── SystemPromptSlots (role, guidelines, response_style, extra)
        │     ├── Planning (task decomposition + wave execution)
        │     └── HITL Confirmation
        ├── SessionLaneQueue (a3s-lane backed)
        │     ├── Control (P0) → Query (P1) → Execute (P2) → Generate (P3)
        │     └── External Task Distribution
        ├── HookEngine (11 lifecycle events)
        ├── Security (PII redaction, injection detection)
        ├── Skills (instruction injection + tool permissions)
        ├── Context (RAG providers: ripgrep, filesystem)
        └── Memory (AgentMemory: working/short-term/long-term via a3s-memory)

AgentTeam (multi-agent coordination)
  ├── TeamTaskBoard (post → claim → complete → review → approve/reject)
  ├── TeamMember[] (Lead, Worker, Reviewer roles)
  └── mpsc channels (peer-to-peer messaging + broadcast)

TeamRunner (LLM-integrated orchestrator)
  ├── Lead  → decomposes goal into JSON task list
  ├── Workers → concurrently claim + execute tasks via AgentSession
  └── Reviewer → approve (Done) or reject (re-queued for retry)
```

---

## Configuration

A3S Code uses HCL configuration format exclusively.

### Minimal

```hcl
default_model = "anthropic/claude-sonnet-4-20250514"

providers {
  name    = "anthropic"
  api_key = env("ANTHROPIC_API_KEY")
}
```

### Full

```hcl
default_model = "anthropic/claude-sonnet-4-20250514"

providers {
  name    = "anthropic"
  api_key = env("ANTHROPIC_API_KEY")
}

providers {
  name    = "openai"
  api_key = env("OPENAI_API_KEY")
}

queue {
  query_max_concurrency   = 10
  execute_max_concurrency = 5
  enable_metrics          = true
  enable_dlq              = true

  retry_policy {
    strategy         = "exponential"
    max_retries      = 3
    initial_delay_ms = 100
  }

  rate_limit {
    limit_type     = "per_second"
    max_operations = 100
  }

  priority_boost {
    strategy    = "standard"
    deadline_ms = 300000
  }

  pressure_threshold = 50
}

search {
  timeout = 30
  engine {
    google { enabled = true, weight = 1.5 }
    bing   { enabled = true, weight = 1.0 }
  }
}

storage_backend = "file"
sessions_dir    = "./sessions"
skill_dirs      = ["./skills"]
agent_dirs      = ["./agents"]
max_tool_rounds = 50
thinking_budget = 10000
```

---

## API Reference

### Python SDK — Agent Teams

```python
from a3s_code import Agent, Team, TeamRunner, TeamConfig, TeamTaskBoard

# Build team
config = TeamConfig(max_tasks=50, max_rounds=10, poll_interval_ms=200)
team = Team("my-team", config)
team.add_member("lead", "lead")       # role: "lead" | "worker" | "reviewer"
team.add_member("worker-1", "worker")
team.add_member("reviewer", "reviewer")

# Bind sessions and run
runner = TeamRunner(team)             # consumes the team
runner.bind_session("lead", agent.session("."))
result = runner.run_until_done("Build the feature")

# Inspect results
result.done_tasks      # List[TeamTask]
result.rejected_tasks  # List[TeamTask]  (did not pass review after max_rounds)
result.rounds          # int

# Direct board access
board = runner.task_board()
board.post("Fix lint", "lead")
board.claim("worker-1")              # → TeamTask | None
board.complete(task_id, "Fixed")
board.approve(task_id)
board.reject(task_id)
tasks = board.by_status("done")      # "open"|"in_progress"|"in_review"|"done"|"rejected"
(open, prog, rev, done, rej) = board.stats()
```

### Python SDK

```python
from a3s_code import Agent, SessionOptions, builtin_skills, DefaultSecurityProvider, FileMemoryStore, FileSessionStore, MemorySessionStore

# Create agent
agent = Agent("agent.hcl")

# Create session
opts = SessionOptions()
opts.model = "anthropic/claude-sonnet-4-20250514"
opts.builtin_skills = True
opts.role = "You are a Python expert"
opts.guidelines = "Follow PEP 8. Use type hints."
session = agent.session(".", opts)

# Send / Stream
result = session.send("Explain auth module")
for event in session.stream("Refactor auth"):
    if event.event_type == "text_delta":
        print(event.text, end="")

# Direct tools
content = session.read_file("src/main.py")
output = session.bash("pytest")
files = session.glob("**/*.py")
matches = session.grep("TODO")
result = session.tool("git_worktree", {"command": "list"})

# Memory
session.remember_success("task", ["tool"], "result")
items = session.recall_similar("auth", 5)

# Slash commands
commands = session.list_commands()   # list[dict] — name, description, usage
session.register_command("status", "Show session status",
    lambda args, ctx: f"Model: {ctx['model']}, History: {ctx['history_len']} msgs")
result = session.send("/status")

# Hooks
session.register_hook("audit", "pre_tool_use", handler_fn)

# Scheduled tasks (programmatic)
task_id = session.schedule_task("check deployment status", 30)   # every 30s
tasks   = session.list_scheduled_tasks()   # list[dict] — id, prompt, interval_secs, fire_count, next_fire_in_secs
ok      = session.cancel_scheduled_task(task_id)   # True
# Also via slash commands:
session.send("/loop 5m summarize recent commits")
session.send("/cron-list")
session.send(f"/cron-cancel {task_id}")

# MCP management
count  = session.add_mcp_server("filesystem", command="npx", args=["-y", "@modelcontextprotocol/server-filesystem", "/tmp"])
status = session.mcp_status()        # {name: {connected, tool_count, error}}
tools  = session.tool_names()        # list[str]
session.remove_mcp_server("filesystem")
agent.refresh_mcp_tools()           # refresh global MCP tool cache

# Queue
stats = session.queue_stats()
dead = session.dead_letters()

# Persistence — set ID + auto-save, then resume later
opts2 = SessionOptions()
opts2.session_store = FileSessionStore('./sessions')
opts2.session_id = 'my-session'
opts2.auto_save = True
session2 = agent.session(".", opts2)
resumed = agent.resume_session('my-session', opts2)
```

### Node.js SDK — Agent Teams

```typescript
import { Agent, Team, TeamRunner, TeamConfig, TeamTaskBoard } from '@a3s-lab/code';

// Build team
const config: TeamConfig = { maxTasks: 50, channelBuffer: 128, maxRounds: 10, pollIntervalMs: 200 };
const team = new Team('my-team', config);
team.addMember('lead', 'lead');       // role: "lead" | "worker" | "reviewer"
team.addMember('worker-1', 'worker');
team.addMember('reviewer', 'reviewer');

// Bind sessions and run
const runner = new TeamRunner(team);  // consumes the team
runner.bindSession('lead', agent.session('.'));
const result = await runner.runUntilDone('Build the feature');

// Inspect results
result.doneTasks      // TeamTask[]
result.rejectedTasks  // TeamTask[]  (did not pass review after maxRounds)
result.rounds         // number

// Direct board access
const board = runner.taskBoard();
board.post('Fix lint', 'lead');
board.claim('worker-1');             // → TeamTask | null
board.complete(taskId, 'Fixed');
board.approve(taskId);
board.reject(taskId);
const tasks = await board.byStatus('done');  // "open"|"in_progress"|"in_review"|"done"|"rejected"
const stats = board.stats();         // { open, inProgress, inReview, done, rejected, total }
```

### Node.js SDK

```typescript
import { Agent, DefaultSecurityProvider, FileMemoryStore, FileSessionStore, MemorySessionStore } from '@a3s-lab/code';

// Create agent
const agent = await Agent.create('agent.hcl');

// Create session
const session = agent.session('.', {
  model: 'anthropic/claude-sonnet-4-20250514',
  builtinSkills: true,
  role: 'You are a TypeScript expert',
  guidelines: 'Use strict mode. Prefer interfaces over types.',
});

// Send / Stream
const result = await session.send('Explain auth module');
const stream = await session.stream('Refactor auth');
for await (const event of stream) {
  if (event.type === 'text_delta') process.stdout.write(event.text);
}

// Direct tools
const content = await session.readFile('src/main.ts');
const output = await session.bash('npm test');
const files = await session.glob('**/*.ts');
const matches = await session.grep('TODO');
const result = await session.tool('git_worktree', { command: 'list' });

// Memory
await session.rememberSuccess('task', ['tool'], 'result');
const items = await session.recallSimilar('auth', 5);

// Hooks
session.registerHook('audit', 'pre_tool_use', handlerFn);

// Slash commands
const commands = session.listCommands();   // CommandInfo[]

// Scheduled tasks (programmatic)
const taskId = session.scheduleTask('check deployment status', 30);   // every 30s
const tasks  = session.listScheduledTasks();   // ScheduledTaskInfo[]
const ok     = session.cancelScheduledTask(taskId);   // boolean
// Also via slash commands:
await session.send('/loop 5m summarize recent commits');
await session.send('/cron-list');
await session.send(`/cron-cancel ${taskId}`);

// MCP management
const count  = await session.addMcpServer('filesystem', 'stdio', 'npx', ['-y', '@modelcontextprotocol/server-filesystem', '/tmp']);
const status = await session.mcpStatus();   // McpServerStatusEntry[]
const tools  = session.toolNames();         // string[]
await session.removeMcpServer('filesystem');
await agent.refreshMcpTools();             // refresh global MCP tool cache

// Queue
const stats = await session.queueStats();
const dead = await session.deadLetters();

// Persistence — set ID + auto-save, then resume later
const session2 = agent.session('.', {
  sessionStore: new FileSessionStore('./sessions'),
  sessionId: 'my-session',
  autoSave: true,
});
const resumed = agent.resumeSession('my-session', { sessionStore: new FileSessionStore('./sessions') });
```

---

## Examples

All examples use real LLM configuration from `~/.a3s/config.hcl` or `$A3S_CONFIG`.

### Tutorial Series (Rust)

| #  | Example                | Feature                                         |
| -- | ---------------------- | ----------------------------------------------- |
| 01 | `01_basic_send`      | Non-streaming prompt execution                  |
| 02 | `02_streaming`       | Real-time AgentEvent stream                     |
| 03 | `03_multi_turn`      | Context preservation across turns               |
| 04 | `04_model_switching` | Provider/model override + temperature           |
| 05 | `05_planning`        | Task decomposition + goal tracking              |
| 06 | `06_skills_security` | Built-in skills + security provider             |
| 07 | `07_direct_tools`    | Bypass LLM, call tools directly                 |
| 08 | `08_hooks`           | Lifecycle event interception                    |
| 09 | `09_queue_lanes`     | Priority-based tool scheduling                  |
| 10 | `10_resilience`      | Auto-compaction, circuit breaker, parse retries |

```bash
cargo run --example 01_basic_send
cargo run --example 02_streaming
# ... through 10_resilience
```

### Multi-Language SDKs

| Language | File                                               | Coverage                                                                                        |
| -------- | -------------------------------------------------- | ----------------------------------------------------------------------------------------------- |
| Rust     | `core/examples/test_git_worktree.rs`             | Git worktree tool: direct calls + LLM-driven                                                    |
| Rust     | `core/examples/test_prompt_slots.rs`             | Prompt slots: role, guidelines, response style, extra                                           |
| Python   | `sdk/python/examples/agentic_loop_demo.py`       | Basic send, streaming, multi-turn, planning, skills, security                                   |
| Python   | `sdk/python/examples/advanced_features_demo.py`  | Direct tools, hooks, queue/lanes, security, resilience, memory                                  |
| Python   | `sdk/python/examples/test_git_worktree.py`       | Git worktree tool: direct calls + LLM-driven                                                    |
| Python   | `sdk/python/examples/test_prompt_slots.py`       | Prompt slots: role, guidelines, response style, extra                                           |
| Python   | `sdk/python/examples/test_agent_teams.py`        | Multi-agent teams: TeamRunner, Lead/Worker/Reviewer workflow                                    |
| Python   | `sdk/python/examples/test_agent_slot_kimi.py`    | AgentSlot + Orchestrator.create(): spawn() and wait()                                           |
| Python   | `sdk/python/examples/test_run_team_kimi.py`      | Orchestrator.run_team() with AgentSlot array (Lead/Worker/Reviewer)                             |
| Python   | `sdk/python/examples/test_run_team_tool.py`      | `run_team` built-in tool via session.tool() + LLM-driven                                      |
| Python   | `sdk/python/examples/test_run_team_tool_kimi.py` | `run_team` tool smoke test with external LLM                                                  |
| Node.js  | `sdk/node/examples/agentic_loop_demo.js`         | Basic send, streaming, multi-turn, planning, skills, security                                   |
| Node.js  | `sdk/node/examples/advanced_features_demo.js`    | Direct tools, hooks, queue/lanes, security, resilience, memory                                  |
| Node.js  | `sdk/node/examples/test_git_worktree.js`         | Git worktree tool: direct calls + LLM-driven                                                    |
| Node.js  | `sdk/node/examples/test_prompt_slots.js`         | Prompt slots: role, guidelines, response style, extra                                           |
| Node.js  | `sdk/node/examples/test_agent_teams.js`          | Multi-agent teams: TeamRunner, Lead/Worker/Reviewer workflow                                    |
| Node.js  | `sdk/node/examples/test_agent_slot_kimi.ts`      | AgentSlot + Orchestrator.create(): spawn() and wait()                                           |
| Node.js  | `sdk/node/examples/test_run_team_kimi.ts`        | Orchestrator.runTeam() with AgentSlot array (Lead/Worker/Reviewer)                              |
| Node.js  | `sdk/node/examples/test_run_team_tool.ts`        | `run_team` built-in tool via session.tool() + LLM-driven                                      |
| Node.js  | `sdk/node/examples/test_run_team_tool_kimi.ts`   | `run_team` tool smoke test with external LLM                                                  |
| Python   | `sdk/python/examples/test_loop_commands.py`      | Slash commands: /loop, /cron-list, /cron-cancel, list_commands, register_command, schedule_task |
| Node.js  | `sdk/node/examples/test_loop_commands.ts`        | Slash commands: /loop, /cron-list, /cron-cancel, listCommands, scheduleTask                     |

### Integration & Feature Tests

- `integration_tests` — Complete feature test suite
- `test_task_priority` — Lane-based priority preemption with real LLM
- `test_external_task_handler` — Multi-machine coordinator/worker pattern
- `test_lane_features` — A3S Lane v0.4.0 advanced features
- `test_builtin_skills` — Built-in skills demonstration
- `test_custom_skills_agents` — Custom skills and agent definitions
- `test_search_config` — Web search configuration
- `test_auto_compact` — Context window auto-compaction
- `test_security` — Default and custom SecurityProvider
- `test_batch_tool` — Parallel tool execution via batch
- `test_ripgrep_context` — Fast indexless code search with ripgrep provider
- `test_hooks` — Lifecycle hook handlers (audit, block, transform)
- `test_parallel_processing` — Concurrent multi-session workloads
- `test_git_worktree` — Git worktree tool: create, list, remove, status + LLM-driven
- `test_prompt_slots` — System prompt slots: role, guidelines, response style, extra + tool verification

---

## Testing

```bash
cargo test          # All tests
cargo test --lib    # Unit tests only
```

**Test Coverage:** 1500 tests, 100% pass rate

---

## Contributing

- Follow Rust API guidelines
- Write tests for all new code
- **Run `cargo fmt --all` before committing** (enforced by pre-commit hook)
- Use `cargo clippy` to check for lints
- Update documentation
- Use [Conventional Commits](https://www.conventionalcommits.org/)

### Setup Pre-commit Hook

Install the pre-commit hook to automatically check formatting:

```bash
cp scripts/pre-commit .git/hooks/pre-commit
chmod +x .git/hooks/pre-commit
```

This will prevent commits with unformatted code and ensure CI checks pass.

---

## Community

Join us on [Discord](https://discord.gg/XVg6Hu6H) for questions, discussions, and updates.

## License

MIT License - see [LICENSE](./LICENSE)

---

## Star History

[![Star History Chart](https://api.star-history.com/svg?repos=A3S-Lab/Code&type=Date)](https://star-history.com/#A3S-Lab/Code&Date)

---

## Related Projects

- **[A3S Lane](../lane)** — Priority-based task queue with DLQ
- **[A3S Search](../search)** — Multi-engine web search aggregator
- **[A3S Box](../box)** — Secure sandbox runtime with TEE support
- **[A3S Event](../event)** — Event-driven architecture primitives

---

**Built by [A3S Lab](https://github.com/A3S-Lab)** | [Documentation](https://docs.a3s.dev) | [Discord](https://discord.gg/a3s)
