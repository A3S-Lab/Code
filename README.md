# A3S Code

**Embeddable AI coding agent framework in Rust** — Build agents that read, write, and execute code with tool access, planning, and safety controls.

```rust
let agent = Agent::new("agent.hcl").await?;
let session = agent.session(".", None)?;
let result = session.send("Refactor auth to use JWT").await?;
```

[![Crates.io](https://img.shields.io/crates/v/a3s-code-core.svg)](https://crates.io/crates/a3s-code-core)
[![Documentation](https://docs.rs/a3s-code-core/badge.svg)](https://docs.rs/a3s-code-core)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](./LICENSE)
[![Tests](https://img.shields.io/badge/tests-1402%20passing-brightgreen.svg)](./core/tests)

---

## Why A3S Code?

- **Embeddable** — Rust library, not a service. Node.js and Python bindings included. CLI for terminal use.
- **Production-Ready** — Permission system, HITL confirmation, skill-based tool restrictions, and error recovery (parse retries, tool timeout, circuit breaker).
- **Extensible** — 19 trait-based extension points, all with working defaults. Slash commands, tool search, and multi-agent teams.
- **Scalable** — Lane-based priority queue with multi-machine task distribution.

---

## Quick Start

### 1. Install

```bash
# Rust
cargo add a3s-code-core

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

<details>
<summary><b>Rust</b></summary>

```rust
use a3s_code_core::{Agent, SessionOptions};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let agent = Agent::new("agent.hcl").await?;
    let session = agent.session(".", None)?;
    let result = session.send("What files handle authentication?", None).await?;
    println!("{}", result.text);

    // With options
    let session = agent.session(".", Some(
        SessionOptions::new()
            .with_default_security()
            .with_builtin_skills()
            .with_planning(true)
    ))?;
    let result = session.send("Refactor auth + update tests", None).await?;
    Ok(())
}
```

</details>

<details>
<summary><b>TypeScript</b></summary>

```typescript
import { Agent } from '@a3s-lab/code';

const agent = await Agent.create('agent.hcl');
const session = agent.session('.', {
  defaultSecurity: true,
  builtinSkills: true,
  planning: true,
});

const result = await session.send('Refactor auth + update tests');
console.log(result.text);
```

</details>

<details>
<summary><b>Python</b></summary>

```python
from a3s_code import Agent, SessionOptions

agent = Agent("agent.hcl")
session = agent.session(".", SessionOptions(
    default_security=True,
    builtin_skills=True,
    planning=True,
))

result = session.send("Refactor auth + update tests")
print(result.text)
```

</details>

---

## Core Features

### 🛠️ Built-in Tools (13 + 1 optional)

| Category | Tools | Description |
|----------|-------|-------------|
| **File Operations** | `read`, `write`, `edit`, `patch` | Read/write files, apply diffs |
| **Search** | `grep`, `glob`, `ls` | Search content, find files, list directories |
| **Execution** | `bash` | Execute shell commands |
| **Sandbox** | `sandbox` | MicroVM execution via A3S Box (`sandbox` feature) |
| **Web** | `web_fetch`, `web_search` | Fetch URLs, search the web |
| **Git** | `git_worktree` | Create/list/remove/status git worktrees for parallel work |
| **Subagents** | `task` | Delegate to specialized child agents |
| **Parallel** | `batch` | Execute multiple tools concurrently in one call |

---

### 🔒 Security & Safety

**Permission System** — Allow/Deny/Ask rules per tool with wildcard matching:

```rust
use a3s_code_core::permissions::PermissionPolicy;

SessionOptions::new()
    .with_permission_checker(Arc::new(
        PermissionPolicy::new()
            .allow("read(*)")
            .deny("bash(*)")
            .ask("write(*)")
    ))
```

**Default Security Provider** — Auto-redact PII (SSN, API keys, emails, credit cards), prompt injection detection, SHA256 hashing:

```rust
SessionOptions::new().with_default_security()
```

**HITL Confirmation** — Require human approval for sensitive operations:

```rust
SessionOptions::new()
    .with_confirmation_manager(Arc::new(
        ConfirmationManager::new(ConfirmationPolicy::enabled(), event_tx)
    ))
```

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

```rust
SessionOptions::new()
    .with_builtin_skills()           // Enable all 7 built-in skills
    .with_skills_from_dir("./skills") // Load custom skills
```

---

### 🎯 Planning & Goal Tracking

Decompose complex tasks into dependency-aware execution plans with wave-based parallel execution:

```rust
SessionOptions::new()
    .with_planning(true)
    .with_goal_tracking(true)
```

The planner creates steps with dependencies. Independent steps execute in parallel waves via `tokio::JoinSet`. Goal tracking monitors progress across multiple turns.

---

### 🖼️ Multi-Modal Support (Vision)

Send image attachments alongside text prompts. Requires a vision-capable model (Claude Sonnet, GPT-4o).

```rust
use a3s_code_core::Attachment;

// Send a prompt with an image
let image = Attachment::from_file("screenshot.png")?;
let result = session.send_with_attachments(
    "What's in this screenshot?",
    &[image],
    None,
).await?;

// Tools can return images too
let output = ToolOutput::success("Screenshot captured")
    .with_images(vec![Attachment::png(screenshot_bytes)]);
```

Supported formats: JPEG, PNG, GIF, WebP. Image data is base64-encoded for both Anthropic and OpenAI providers.

---

### ⚡ Slash Commands

Interactive session commands dispatched before the LLM. Custom commands via the `SlashCommand` trait:

| Command | Description |
|---------|-------------|
| `/help` | List available commands |
| `/compact` | Manually trigger context compaction |
| `/cost` | Show token usage and estimated cost |
| `/model` | Show or switch the current model |
| `/clear` | Clear conversation history |
| `/history` | Show conversation turn count and token stats |
| `/tools` | List registered tools |

```rust
use a3s_code_core::commands::{SlashCommand, CommandContext, CommandOutput};

struct PingCommand;
impl SlashCommand for PingCommand {
    fn name(&self) -> &str { "ping" }
    fn description(&self) -> &str { "Pong!" }
    fn execute(&self, _args: &str, _ctx: &CommandContext) -> CommandOutput {
        CommandOutput::text("pong")
    }
}

session.register_command(Arc::new(PingCommand));
```

---

### 🔍 Tool Search (Per-Turn Filtering)

When the MCP ecosystem grows large (100+ tools), injecting all tool descriptions wastes context. Tool Search selects only relevant tools per-turn based on keyword matching:

```rust
use a3s_code_core::tool_search::{ToolIndex, ToolSearchConfig};

let mut index = ToolIndex::new(ToolSearchConfig::default());
index.add("mcp__github__create_issue", "Create a GitHub issue", &["github", "issue"]);
index.add("mcp__postgres__query", "Run a SQL query", &["sql", "database"]);

// Integrated into AgentLoop — filters tools automatically before each LLM call
```

When configured via `AgentConfig::tool_index`, the agent loop extracts the last user message, searches the index, and only sends matching tools to the LLM. Builtin tools are always included.

---

### 👥 Agent Teams (Multi-Agent Coordination)

Automated Lead → Worker → Reviewer workflows with real LLM execution:

```rust
use a3s_code_core::{Agent, SessionOptions, agent_teams::{AgentTeam, TeamConfig, TeamRole, TeamRunner}};

let agent = Agent::new("agent.hcl").await?;

let mut team = AgentTeam::new("refactor-auth", TeamConfig::default());
team.add_member("lead", TeamRole::Lead);
team.add_member("worker-1", TeamRole::Worker);
team.add_member("reviewer", TeamRole::Reviewer);

let mut runner = TeamRunner::new(team);
runner.bind_session("lead",     Arc::new(agent.session(".", None)?))?;
runner.bind_session("worker-1", Arc::new(agent.session(".", None)?))?;
runner.bind_session("reviewer", Arc::new(agent.session(".", None)?))?;

let result = runner.run_until_done("Refactor auth module to use JWT").await?;
println!("Done: {} tasks, {} rejected, {} rounds",
    result.done_tasks.len(), result.rejected_tasks.len(), result.rounds);
```

**How it works:**
1. **Lead** decomposes the goal into a JSON task list via LLM
2. **Workers** concurrently claim and execute tasks (each via its own `AgentSession`)
3. **Reviewer** inspects completed work — APPROVED moves the task to Done, REJECTED re-queues it for retry
4. Loop continues until all tasks are Done or `max_rounds` is reached

**Low-level coordination API** (for custom orchestrators):

```rust
// Post, claim, complete manually (no LLM required)
team.task_board().post("Refactor auth", "lead", None);
let task = team.task_board().claim("worker-1").unwrap();
team.task_board().complete(&task.id, "Refactored to JWT");
team.task_board().approve(&task.id);

// Peer messaging
team.send_message("lead", "worker-1", "Focus on the refresh token flow", None).await;
let mut rx = team.take_receiver("worker-1").unwrap();
while let Some(msg) = rx.recv().await { println!("{}: {}", msg.from, msg.content); }
```

<details>
<summary><b>TypeScript</b></summary>

```typescript
import { Agent, Team, TeamRunner, TeamConfig } from '@a3s-lab/code';

const agent = await Agent.create('agent.hcl');

const config: TeamConfig = { maxTasks: 50, channelBuffer: 128, maxRounds: 10, pollIntervalMs: 200 };
const team = new Team('refactor-auth', config);
team.addMember('lead', 'lead');
team.addMember('worker-1', 'worker');
team.addMember('reviewer', 'reviewer');

const runner = new TeamRunner(team);
runner.bindSession('lead',     agent.session('.'));
runner.bindSession('worker-1', agent.session('.'));
runner.bindSession('reviewer', agent.session('.'));

const result = await runner.runUntilDone('Refactor auth module to use JWT');
console.log(`Done: ${result.doneTasks.length} tasks, ${result.rounds} rounds`);
for (const task of result.doneTasks) {
  console.log(`  [${task.id}] ${task.description}\n  → ${task.result}`);
}
```

</details>

<details>
<summary><b>Python</b></summary>

```python
from a3s_code import Agent, Team, TeamRunner, TeamConfig

agent = Agent.create("agent.hcl")

config = TeamConfig(max_rounds=10, poll_interval_ms=200)
team = Team("refactor-auth", config)
team.add_member("lead", "lead")
team.add_member("worker-1", "worker")
team.add_member("reviewer", "reviewer")

runner = TeamRunner(team)
runner.bind_session("lead",     agent.session("."))
runner.bind_session("worker-1", agent.session("."))
runner.bind_session("reviewer", agent.session("."))

result = runner.run_until_done("Refactor auth module to use JWT")
print(f"Done: {len(result.done_tasks)} tasks, {result.rounds} rounds")
for task in result.done_tasks:
    print(f"  [{task.id}] {task.description}\n  → {task.result}")
```

</details>

Supports Lead/Worker/Reviewer roles, `mpsc` peer messaging, broadcast, and a full task lifecycle (Open → InProgress → InReview → Done/Rejected).

---

### 🎨 System Prompt Customization (Slot-Based)

Customize the agent's behavior without overriding the core agentic capabilities. The default prompt (tool usage strategy, autonomous behavior, completion criteria) is always preserved:

```rust
use a3s_code_core::{SessionOptions, SystemPromptSlots};

SessionOptions::new()
    .with_prompt_slots(SystemPromptSlots {
        role: Some("You are a senior Rust developer".into()),
        guidelines: Some("Use clippy. No unwrap(). Prefer Result.".into()),
        response_style: Some("Be concise. Use bullet points.".into()),
        extra: Some("This project uses tokio and axum.".into()),
    })
```

| Slot | Position | Behavior |
|------|----------|----------|
| `role` | Before core | Replaces default "You are A3S Code..." identity |
| `guidelines` | After core | Appended as `## Guidelines` section |
| `response_style` | Replaces section | Replaces default `## Response Format` |
| `extra` | End | Freeform instructions (backward-compatible) |

<details>
<summary><b>TypeScript</b></summary>

```typescript
const session = agent.session('.', {
  role: 'You are a senior Rust developer',
  guidelines: 'Use clippy. No unwrap(). Prefer Result.',
  responseStyle: 'Be concise. Use bullet points.',
  extra: 'This project uses tokio and axum.',
});
```

</details>

<details>
<summary><b>Python</b></summary>

```python
opts = SessionOptions()
opts.role = "You are a senior Rust developer"
opts.guidelines = "Use clippy. No unwrap(). Prefer Result."
opts.response_style = "Be concise. Use bullet points."
opts.extra = "This project uses tokio and axum."
session = agent.session(".", opts)
```

</details>

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

| Lane | Priority | Tools | Behavior |
|------|----------|-------|----------|
| **Control** | P0 (highest) | pause, resume, cancel | Sequential |
| **Query** | P1 | read, glob, grep, ls, web_fetch, web_search | Parallel |
| **Execute** | P2 | bash, write, edit, delete | Sequential |
| **Generate** | P3 (lowest) | LLM calls | Sequential |

Higher-priority tasks preempt queued lower-priority tasks. Configure per-lane concurrency:

```rust
let queue_config = SessionQueueConfig {
    query_max_concurrency: 10,
    execute_max_concurrency: 5,
    enable_metrics: true,
    ..Default::default()
};

SessionOptions::new().with_queue_config(queue_config)
```

Advanced features: retry policies, rate limiting, priority boost, pressure monitoring, DLQ.

---

### 🔒 Sandbox Execution (A3S Box Integration)

Route `bash` commands through an A3S Box MicroVM for isolated execution. Requires the `sandbox` Cargo feature.

**Transparent routing** — configure once, bash tool uses sandbox automatically:

```rust
use a3s_code_core::{SessionOptions, SandboxConfig};

SessionOptions::new().with_sandbox(SandboxConfig {
    image: "ubuntu:22.04".into(),
    memory_mb: 512,
    network: false,
    ..SandboxConfig::default()
})
```

**Explicit `sandbox` tool** — with `sandbox` feature enabled, the LLM can call the `sandbox` tool directly. Workspace is mounted at `/workspace` inside the MicroVM.

Enable:

```toml
a3s-code-core = { version = "0.7", features = ["sandbox"] }
```

---

### 🌐 Multi-Machine Distribution

Offload tool execution to external workers via three handler modes:

| Mode | Behavior |
|------|----------|
| **Internal** (default) | Execute within agent process |
| **External** | Send to external workers, wait for completion |
| **Hybrid** | Execute internally + notify external observers |

```rust
// Route Execute lane to external workers
session.set_lane_handler(SessionLane::Execute, LaneHandlerConfig {
    mode: TaskHandlerMode::External,
    timeout_ms: 120_000,
}).await;

// Worker loop
let tasks = session.pending_external_tasks().await;
for task in tasks {
    let result = execute_task(&task).await;
    session.complete_external_task(&task.task_id, result).await;
}
```

---

### 🔌 Extensibility (19 Extension Points)

All policies are replaceable via traits with working defaults:

| Extension Point | Purpose | Default |
|----------------|---------|---------|
| SecurityProvider | Input taint, output sanitization | DefaultSecurityProvider |
| PermissionChecker | Tool access control | PermissionPolicy |
| ConfirmationProvider | Human confirmation | ConfirmationManager |
| ContextProvider | RAG retrieval | FileSystemContextProvider |
| EmbeddingProvider | Vector embeddings for semantic search | OpenAiEmbeddingProvider |
| VectorStore | Embedding storage and similarity search | InMemoryVectorStore |
| SessionStore | Session persistence | FileSessionStore |
| MemoryStore | Long-term memory backend (from `a3s-memory`) | InMemoryStore |
| Tool | Custom tools | 12 built-in tools |
| Planner | Task decomposition | LlmPlanner |
| HookHandler | Event handling | HookEngine |
| HookExecutor | Event execution | HookEngine |
| McpTransport | MCP protocol | StdioTransport |
| HttpClient | HTTP requests | ReqwestClient |
| SessionCommand | Queue tasks | ToolCommand |
| LlmClient | LLM interface | Anthropic/OpenAI |
| BashSandbox | Shell execution isolation | LocalBashExecutor |
| SkillValidator | Skill activation logic | DefaultSkillValidator |
| SkillScorer | Skill relevance ranking | DefaultSkillScorer |

```rust
// Example: custom security provider
impl SecurityProvider for MyProvider {
    fn taint_input(&self, text: &str) { /* ... */ }
    fn sanitize_output(&self, text: &str) -> String { /* ... */ }
}

SessionOptions::new().with_security_provider(Arc::new(MyProvider))
```

---

## Architecture

5 core components (stable, not replaceable) + 19 extension points (replaceable via traits):

```
Agent (config-driven)
  ├── CommandRegistry (slash commands: /help, /cost, /model, /clear, ...)
  └── AgentSession (workspace-bound)
        ├── AgentLoop (core execution engine)
        │     ├── ToolExecutor (13 built-in tools, batch parallel execution)
        │     ├── ToolIndex (per-turn tool filtering for large MCP sets)
        │     ├── SystemPromptSlots (role, guidelines, response_style, extra)
        │     ├── Planning (task decomposition + wave execution)
        │     └── HITL Confirmation
        ├── SessionLaneQueue (a3s-lane backed)
        │     ├── Control (P0) → Query (P1) → Execute (P2) → Generate (P3)
        │     └── External Task Distribution
        ├── HookEngine (8 lifecycle events)
        ├── Security (PII redaction, injection detection)
        ├── Skills (instruction injection + tool permissions)
        ├── Context (RAG providers: filesystem, vector)
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

### Agent

```rust
let agent = Agent::new("agent.hcl").await?;       // From file
let agent = Agent::new(hcl_string).await?;         // From string
let agent = Agent::from_config(config).await?;     // From struct
let session = agent.session(".", None)?;            // Create session
let session = agent.session(".", Some(options))?;   // With options
let session = agent.resume_session("id", options)?; // Resume saved session
```

### AgentSession

```rust
// Prompt execution
let result = session.send("prompt", None).await?;
let (rx, handle) = session.stream("prompt", None).await?;

// Multi-modal (vision)
let result = session.send_with_attachments("Describe", &[image], None).await?;
let (rx, handle) = session.stream_with_attachments("Describe", &[image], None).await?;

// Session persistence
session.save().await?;
let id = session.session_id();

// Slash commands
session.register_command(Arc::new(MyCommand));
let registry = session.command_registry();

// Memory
session.remember_success("task", &["tool"], "result").await?;
session.recall_similar("query", 5).await?;

// Direct tool access
let content = session.read_file("src/main.rs").await?;
let output = session.bash("cargo test").await?;
let files = session.glob("**/*.rs").await?;
let matches = session.grep("TODO").await?;
let result = session.tool("edit", args).await?;

// Queue management
session.set_lane_handler(lane, config).await;
let tasks = session.pending_external_tasks().await;
session.complete_external_task(&task_id, result).await;
let stats = session.queue_stats().await;
let metrics = session.queue_metrics().await;
let dead = session.dead_letters().await;
```

### SessionOptions

```rust
SessionOptions::new()
    // Security
    .with_default_security()
    .with_security_provider(Arc::new(MyProvider))
    // Skills
    .with_builtin_skills()
    .with_skills_from_dir("./skills")
    // Planning
    .with_planning(true)
    .with_goal_tracking(true)
    // Context / RAG
    .with_fs_context(".")
    .with_context_provider(provider)
    // Memory
    .with_file_memory("./memory")
    // Session persistence
    .with_file_session_store("./sessions")
    // Auto-compact
    .with_auto_compact(true)
    .with_auto_compact_threshold(0.80)
    // Error recovery
    .with_parse_retries(3)
    .with_tool_timeout(30_000)
    .with_circuit_breaker(5)
    // Queue
    .with_queue_config(queue_config)
    // Prompt customization (slot-based, preserves core agentic behavior)
    .with_prompt_slots(SystemPromptSlots {
        role: Some("You are a Python expert".into()),
        guidelines: Some("Follow PEP 8".into()),
        ..Default::default()
    })
    // Extensions
    .with_permission_checker(policy)
    .with_confirmation_manager(mgr)
    .with_skill_registry(registry)
    .with_hook_engine(hooks)
```

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
from a3s_code import Agent, SessionOptions, builtin_skills

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

# Hooks
session.register_hook("audit", "pre_tool_use", handler_fn)

# Queue
stats = session.queue_stats()
dead = session.dead_letters()

# Persistence
session.save()
resumed = agent.resume_session(session.session_id, opts)
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
import { Agent } from '@a3s-lab/code';

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

// Queue
const stats = await session.queueStats();
const dead = await session.deadLetters();

// Persistence
await session.save();
const resumed = agent.resumeSession(session.sessionId, options);
```

---

## Examples

All examples use real LLM configuration from `~/.a3s/config.hcl` or `$A3S_CONFIG`.

### Tutorial Series (Rust)

| # | Example | Feature |
|---|---------|---------|
| 01 | `01_basic_send` | Non-streaming prompt execution |
| 02 | `02_streaming` | Real-time AgentEvent stream |
| 03 | `03_multi_turn` | Context preservation across turns |
| 04 | `04_model_switching` | Provider/model override + temperature |
| 05 | `05_planning` | Task decomposition + goal tracking |
| 06 | `06_skills_security` | Built-in skills + security provider |
| 07 | `07_direct_tools` | Bypass LLM, call tools directly |
| 08 | `08_hooks` | Lifecycle event interception |
| 09 | `09_queue_lanes` | Priority-based tool scheduling |
| 10 | `10_resilience` | Auto-compaction, circuit breaker, parse retries |

```bash
cargo run --example 01_basic_send
cargo run --example 02_streaming
# ... through 10_resilience
```

### Multi-Language SDKs

| Language | File | Coverage |
|----------|------|----------|
| Rust | `core/examples/test_git_worktree.rs` | Git worktree tool: direct calls + LLM-driven |
| Rust | `core/examples/test_prompt_slots.rs` | Prompt slots: role, guidelines, response style, extra |
| Python | `sdk/python/examples/agentic_loop_demo.py` | Basic send, streaming, multi-turn, planning, skills, security |
| Python | `sdk/python/examples/advanced_features_demo.py` | Direct tools, hooks, queue/lanes, security, resilience, memory |
| Python | `sdk/python/examples/test_git_worktree.py` | Git worktree tool: direct calls + LLM-driven |
| Python | `sdk/python/examples/test_prompt_slots.py` | Prompt slots: role, guidelines, response style, extra |
| Python | `sdk/python/examples/test_agent_teams.py` | Multi-agent teams: TeamRunner, Lead/Worker/Reviewer workflow |
| Node.js | `sdk/node/examples/agentic_loop_demo.js` | Basic send, streaming, multi-turn, planning, skills, security |
| Node.js | `sdk/node/examples/advanced_features_demo.js` | Direct tools, hooks, queue/lanes, security, resilience, memory |
| Node.js | `sdk/node/examples/test_git_worktree.js` | Git worktree tool: direct calls + LLM-driven |
| Node.js | `sdk/node/examples/test_prompt_slots.js` | Prompt slots: role, guidelines, response style, extra |
| Node.js | `sdk/node/examples/test_agent_teams.js` | Multi-agent teams: TeamRunner, Lead/Worker/Reviewer workflow |

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
- `test_vector_rag` — Semantic code search with filesystem context
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

**Test Coverage:** 1402 tests, 100% pass rate

---

## Contributing

- Follow Rust API guidelines
- Write tests for all new code
- Use `cargo fmt` and `cargo clippy`
- Update documentation
- Use [Conventional Commits](https://www.conventionalcommits.org/)

---

## Community

Join us on [Discord](https://discord.gg/XVg6Hu6H) for questions, discussions, and updates.

## License

MIT License - see [LICENSE](./LICENSE)

---

## Related Projects

- **[A3S Lane](../lane)** — Priority-based task queue with DLQ
- **[A3S Search](../search)** — Multi-engine web search aggregator
- **[A3S Box](../box)** — Secure sandbox runtime with TEE support
- **[A3S Event](../event)** — Event-driven architecture primitives

---

**Built by [A3S Lab](https://github.com/A3S-Lab)** | [Documentation](https://docs.a3s.dev) | [Discord](https://discord.gg/a3s)
