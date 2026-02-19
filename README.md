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
[![Tests](https://img.shields.io/badge/tests-1158%20passing-brightgreen.svg)](./core/tests)

---

## Why A3S Code?

- **Embeddable** — Rust library, not a service. Node.js and Python bindings included.
- **Production-Ready** — Permission system, HITL confirmation, skill-based tool restrictions.
- **Extensible** — 14 trait-based extension points, all with working defaults.
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

### 🛠️ Built-in Tools (11)

| Category | Tools | Description |
|----------|-------|-------------|
| **File Operations** | `read`, `write`, `edit`, `patch` | Read/write files, apply diffs |
| **Search** | `grep`, `glob`, `ls` | Search content, find files, list directories |
| **Execution** | `bash` | Execute shell commands |
| **Web** | `web_fetch`, `web_search` | Fetch URLs, search the web |
| **Subagents** | `task` | Delegate to specialized child agents |

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

### 🔌 Extensibility (14 Extension Points)

All policies are replaceable via traits with working defaults:

| Extension Point | Purpose | Default |
|----------------|---------|---------|
| SecurityProvider | Input taint, output sanitization | DefaultSecurityProvider |
| PermissionChecker | Tool access control | PermissionPolicy |
| ConfirmationProvider | Human confirmation | ConfirmationManager |
| ContextProvider | RAG retrieval | FileSystemContextProvider |
| SessionStore | Session persistence | FileSessionStore |
| MemoryStore | Long-term memory | InMemoryStore |
| Tool | Custom tools | 11 built-in tools |
| Planner | Task decomposition | LlmPlanner |
| HookHandler | Event handling | HookEngine |
| HookExecutor | Event execution | HookEngine |
| McpTransport | MCP protocol | StdioTransport |
| HttpClient | HTTP requests | ReqwestClient |
| SessionCommand | Queue tasks | ToolCommand |
| LlmClient | LLM interface | Anthropic/OpenAI |

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

5 core components (stable, not replaceable) + 14 extension points (replaceable via traits):

```
Agent (config-driven)
  └── AgentSession (workspace-bound)
        ├── AgentLoop (core execution engine)
        │     ├── ToolExecutor (11 built-in tools)
        │     ├── Planning (task decomposition + wave execution)
        │     └── HITL Confirmation
        ├── SessionLaneQueue (a3s-lane backed)
        │     ├── Control (P0) → Query (P1) → Execute (P2) → Generate (P3)
        │     └── External Task Distribution
        ├── HookEngine (8 lifecycle events)
        ├── Security (PII redaction, injection detection)
        ├── Skills (instruction injection + tool permissions)
        ├── Context (RAG providers)
        └── Memory (episodic, semantic, procedural, working)
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
```

### AgentSession

```rust
// Prompt execution
let result = session.send("prompt", None).await?;
let (rx, handle) = session.stream("prompt", None).await?;

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
    .with_default_security()
    .with_builtin_skills()
    .with_skills_from_dir("./skills")
    .with_planning(true)
    .with_goal_tracking(true)
    .with_fs_context(".")
    .with_queue_config(queue_config)
    .with_permission_checker(policy)
    .with_confirmation_manager(mgr)
    .with_context_provider(provider)
    .with_skill_registry(registry)
    .with_hook_engine(hooks)
```

---

## Examples

See `core/examples/` for Rust, `sdk/python/examples/` for Python, `sdk/node/examples/` for Node.js.

Key examples:
- `integration_tests` — Complete feature test suite
- `test_task_priority` — Lane-based priority preemption with real LLM
- `test_external_task_handler` — Multi-machine coordinator/worker pattern
- `test_lane_features` — A3S Lane v0.4.0 advanced features
- `test_builtin_skills` — Built-in skills demonstration
- `test_custom_skills_agents` — Custom skills and agent definitions
- `test_search_config` — Web search configuration

```bash
cargo run --example integration_tests
cargo run --example test_task_priority
cargo run --example test_external_task_handler
```

---

## Testing

```bash
cargo test          # All tests
cargo test --lib    # Unit tests only
```

**Test Coverage:** 1158 tests, 100% pass rate

---

## Contributing

- Follow Rust API guidelines
- Write tests for all new code
- Use `cargo fmt` and `cargo clippy`
- Update documentation
- Use [Conventional Commits](https://www.conventionalcommits.org/)

---

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
