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
[![Tests](https://img.shields.io/badge/tests-1172%20passing-brightgreen.svg)](./core/tests)

---

## Table of Contents

- [Why A3S Code?](#why-a3s-code)
- [Quick Start](#quick-start)
- [Core Features](#core-features)
  - [Built-in Tools](#️-built-in-tools-10)
  - [Security & Safety](#-security--safety)
  - [Skills System](#-skills-system-claude-code-compatible)
  - [Planning & Execution](#-planning--execution)
  - [Multi-Machine Distribution](#-multi-machine-distribution)
  - [Extensibility](#-extensibility-14-extension-points)
- [Architecture](#architecture)
- [Configuration](#configuration)
- [API Reference](#api-reference)
- [Examples](#examples)
- [Testing](#testing)
- [Contributing](#contributing)
- [License](#license)

---

## Why A3S Code?

**Embeddable** — Rust library, not a service. Integrate into your app with Node.js/Python bindings.

**Production-Ready** — Permission system, HITL confirmation, skill-based tool restrictions, audit logs.

**Extensible** — 14 trait-based extension points. Replace any policy with your own implementation.

**Scalable** — Multi-machine task distribution for horizontal scaling.

**Intelligent** — Parallel execution, planning, goal tracking, context-aware memory.

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

Create `agent.hcl` (HCL format only):

```hcl
default_model = "anthropic/claude-sonnet-4-20250514"

providers {
  name    = "anthropic"
  api_key = env("ANTHROPIC_API_KEY")
}

# Optional: Queue configuration (a3s-lane integration)
queue {
  query_max_concurrency = 10
  execute_max_concurrency = 5
  enable_metrics = true
  enable_dlq = true
}

# Optional: Search configuration (a3s-search integration)
search {
  timeout = 30
  engine {
    google { enabled = true }
    bing { enabled = true }
  }
}
```

**Note:** A3S Code only supports HCL configuration format. JSON support has been removed.

### 3. Use

<details>
<summary><b>Rust</b></summary>

```rust
use a3s_code_core::{Agent, SessionOptions};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let agent = Agent::new("agent.hcl").await?;

    // Basic usage
    let session = agent.session(".", None)?;
    let result = session.send("What files handle authentication?").await?;
    println!("{}", result.text);

    // With options
    let session = agent.session(".", Some(
        SessionOptions::new()
            .with_default_security()      // Redact sensitive data
            .with_builtin_skills()         // Enable code-search, code-review, etc.
            .with_planning(true)           // Multi-step task decomposition
    ))?;

    let result = session.send("Refactor auth + update tests").await?;
    println!("{}", result.text);

    Ok(())
}
```

</details>

<details>
<summary><b>TypeScript</b></summary>

```typescript
import { Agent, SessionOptions } from '@a3s-lab/code';

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

agent = Agent.create("agent.hcl")
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

### 🛠️ Built-in Tools (10)

| Category | Tools | Description |
|----------|-------|-------------|
| **File Operations** | `read`, `write`, `edit`, `patch` | Read/write files, apply diffs |
| **Search** | `grep`, `glob`, `ls` | Search content, find files, list directories |
| **Execution** | `bash` | Execute shell commands |
| **Web** | `web_fetch`, `web_search` | Fetch URLs, search the web (configurable via `search` config) |
| **Subagents** | `task` | Delegate to specialized sub-agents |

**Example:**

```rust
// Agent automatically uses appropriate tools
let result = session.send("Find all TODO comments in Rust files").await?;
// Behind the scenes: grep(pattern="TODO", glob="**/*.rs")
```

---

### 🔒 Security & Safety

#### 1. Default Security Provider

Auto-redact sensitive data (SSN, API keys, emails, credit cards, etc.)

```rust
SessionOptions::new()
    .with_default_security()  // Automatic PII redaction
```

**Features:**
- 8 sensitive data patterns (SSN, Email, API Keys, Phone, Credit Card, IP, URL, Path)
- 6 prompt injection detection patterns
- SHA256 hashing for privacy
- Input taint tracking
- Output sanitization

#### 2. Permission System

Allow/Deny/Ask rules per tool with wildcard matching

```rust
use a3s_code_core::permissions::PermissionPolicy;

SessionOptions::new()
    .with_permission_checker(Arc::new(
        PermissionPolicy::new()
            .allow("read(*)")      // Allow all read operations
            .deny("bash(*)")       // Deny all bash commands
            .ask("write(*)")       // Ask before writing
    ))
```

**Permission Decisions:**
- **Allow** — Execute immediately
- **Deny** — Reject with error
- **Ask** — Request human confirmation

#### 3. Skill-Based Tool Restrictions

Skills define allowed tools via `allowed-tools` field

```rust
// Create a read-only skill
let skill = Skill {
    name: "read-only".to_string(),
    description: "Read-only code analysis".to_string(),
    allowed_tools: Some("read(*), grep(*), glob(*), ls(*)".to_string()),
    kind: SkillKind::Instruction,
    content: "You can only read files, not modify them.".to_string(),
    tags: vec!["readonly".to_string()],
    version: Some("1.0.0".to_string()),
};

let registry = SkillRegistry::new();
registry.register(Arc::new(skill));

SessionOptions::new()
    .with_skill_registry(Arc::new(registry))
// Now agent can only use read, grep, glob, ls
```

#### 4. HITL (Human-in-the-Loop) Confirmation

Require human approval for sensitive operations

```rust
use a3s_code_core::hitl::{ConfirmationManager, ConfirmationPolicy};

let (event_tx, mut event_rx) = broadcast::channel(256);

SessionOptions::new()
    .with_confirmation_manager(Arc::new(
        ConfirmationManager::new(
            ConfirmationPolicy::enabled(),
            event_tx
        )
    ))

// Listen for confirmation requests
tokio::spawn(async move {
    while let Ok(event) = event_rx.recv().await {
        if let AgentEvent::ConfirmationRequired { tool_id, tool_name, args } = event {
            // Show UI prompt to user
            // Call session.approve_tool(tool_id) or session.deny_tool(tool_id)
        }
    }
});
```

**YOLO Mode** — Auto-approve by lane:

```rust
ConfirmationPolicy::yolo(vec![SessionLane::Query])  // Auto-approve Query lane
```

---

### 🧠 Skills System (Claude Code Compatible)

#### Built-in Skills (7)

**Code Assistance:**
- **code-search** — Search codebase for patterns, functions, types
- **code-review** — Review code for best practices, bugs, security
- **explain-code** — Explain how code works in clear terms
- **find-bugs** — Identify potential bugs and vulnerabilities

**Tool Documentation:**
- **builtin-tools** — Documentation for all built-in file operation and shell tools
- **delegate-task** — Guide for delegating tasks to specialized sub-agents
- **find-skills** — Discover and install agent skills from the ecosystem

```rust
SessionOptions::new()
    .with_builtin_skills()  // Enable all 7 skills
```

#### Custom Skills

Skills are Markdown files with YAML frontmatter (Claude Code format):

```markdown
---
name: api-design
description: Review API design for RESTful principles
allowed-tools: "read(*), grep(*)"
kind: instruction
tags:
  - api
  - design
version: 1.0.0
---
# API Design Review

You are an API design expert. Check for:

1. RESTful principles (proper HTTP methods, status codes)
2. Consistent naming conventions
3. Proper error handling
4. Clear documentation
5. Versioning strategy

Use `read` to examine API files and `grep` to search for patterns.
```

**Load custom skills:**

```rust
SessionOptions::new()
    .with_skills_from_dir("./skills")  // Load from directory
```

**Skill Features:**
- ✅ Automatic system prompt injection
- ✅ Tool permission enforcement
- ✅ Claude Code format compatible
- ✅ Load from directories or create programmatically
- ✅ Support for Instruction/Tool/Agent kinds

---

### 🎯 Planning & Execution

#### Parallel Execution

Query-lane tools (read, grep, glob, ls) run in parallel automatically

```rust
// These operations run in parallel:
let result = session.send("Search for auth functions in all Rust files").await?;
// Behind the scenes: multiple grep operations execute concurrently
```

#### Planning Mode

Decompose complex tasks into steps

```rust
SessionOptions::new()
    .with_planning(true)
    .with_goal_tracking(true)
```

**Example:**

```rust
let result = session.send("Refactor auth module + update tests + update docs").await?;

// Agent creates plan:
// 1. Read current auth module
// 2. Identify refactoring opportunities
// 3. Apply refactoring
// 4. Update tests
// 5. Update documentation
// 6. Verify all tests pass
```

#### Goal Tracking

Track progress toward goals across multiple turns

```rust
SessionOptions::new()
    .with_goal_tracking(true)
```

---

### 🌐 Multi-Machine Distribution

Offload tool execution to external workers for horizontal scaling.

#### Architecture

```
AgentSession (Main Process)
       ↓
SessionLaneQueue (a3s-lane)
       ↓
  External Tasks
       ↓
┌──────┴──────┬──────────┐
Worker 1   Worker 2   Worker 3
(Machine 1) (Machine 2) (Machine 3)
```

#### Task Handler Modes

1. **Internal** (default) — Execute within agent process
2. **External** — Send to external workers, wait for completion
3. **Hybrid** — Execute internally + notify external observers

#### Session Lanes

| Lane | Priority | Tools | Best For |
|------|----------|-------|----------|
| **Control** | P0 (highest) | pause, resume, cancel | Control operations |
| **Query** | P1 | read, grep, glob, ls | Parallel reads |
| **Execute** | P2 | bash, write, edit | Write operations |
| **Generate** | P3 (lowest) | LLM calls | AI generation |

#### Configuration

```rust
use a3s_code_core::queue::{SessionLane, LaneHandlerConfig, TaskHandlerMode, SessionQueueConfig};

// Enable queue
let queue_config = SessionQueueConfig {
    query_max_concurrency: 10,      // Allow 10 parallel queries
    execute_max_concurrency: 5,     // Allow 5 parallel executions
    enable_metrics: true,
    ..Default::default()
};

let session = agent.session(".", Some(
    SessionOptions::new()
        .with_queue_config(queue_config)
))?;

// Configure Query lane for external processing
session.set_lane_handler(SessionLane::Query, LaneHandlerConfig {
    mode: TaskHandlerMode::External,
    timeout_ms: 120_000,  // 2 minutes
}).await;
```

#### Worker Loop (Rust)

```rust
use a3s_code_core::queue::{ExternalTask, ExternalTaskResult};

async fn worker_loop(session: &AgentSession) -> anyhow::Result<()> {
    loop {
        // Poll for pending tasks
        let tasks = session.pending_external_tasks().await;

        if tasks.is_empty() {
            tokio::time::sleep(Duration::from_millis(100)).await;
            continue;
        }

        // Process tasks in parallel
        for task in tasks {
            let result = execute_task(&task).await;
            session.complete_external_task(&task.task_id, result).await;
        }
    }
}

async fn execute_task(task: &ExternalTask) -> ExternalTaskResult {
    match task.command_type.as_str() {
        "read" => {
            let path = task.payload["path"].as_str().unwrap();
            match std::fs::read_to_string(path) {
                Ok(content) => ExternalTaskResult {
                    success: true,
                    result: serde_json::json!({ "content": content }),
                    error: None,
                },
                Err(e) => ExternalTaskResult {
                    success: false,
                    result: serde_json::json!({}),
                    error: Some(e.to_string()),
                },
            }
        }
        _ => ExternalTaskResult {
            success: false,
            result: serde_json::json!({}),
            error: Some(format!("Unknown command: {}", task.command_type)),
        },
    }
}
```

#### Worker Loop (Python)

```python
import asyncio
from a3s_code import AgentSession, ExternalTaskResult

async def worker_loop(session: AgentSession):
    while True:
        tasks = await session.pending_external_tasks()
        if not tasks:
            await asyncio.sleep(0.1)
            continue

        for task in tasks:
            result = await execute_task(task)
            await session.complete_external_task(task.task_id, result)

async def execute_task(task):
    if task.command_type == "read":
        try:
            with open(task.payload["path"], 'r') as f:
                content = f.read()
            return ExternalTaskResult(
                success=True,
                result={"content": content},
                error=None
            )
        except Exception as e:
            return ExternalTaskResult(
                success=False,
                result={},
                error=str(e)
            )
```

#### Use Cases

1. **Parallel Code Search** — Distribute grep/read across multiple machines
2. **Distributed Test Execution** — Run test suites in parallel
3. **Multi-Region Deployments** — Execute tasks in different regions
4. **Custom Environments** — Run tools in containers, VMs, specialized hardware

**Benefits:**
- ✅ Horizontal scaling by adding workers
- ✅ Resource isolation (separate heavy computation)
- ✅ Custom execution environments
- ✅ Multi-region support
- ✅ Language-agnostic workers (Rust, Python, TypeScript, etc.)

---

### 🔌 Extensibility (14 Extension Points)

All policies are replaceable via traits. Every extension point has a default implementation.

#### Extension Points

| Extension Point | Purpose | Default Implementation |
|----------------|---------|----------------------|
| **SecurityProvider** | Input taint, output sanitization | DefaultSecurityProvider |
| **PermissionChecker** | Tool access control | PermissionPolicy |
| **ConfirmationProvider** | Human confirmation | ConfirmationManager |
| **ContextProvider** | RAG retrieval | FileSystemContextProvider |
| **SessionStore** | Session persistence | FileSessionStore |
| **MemoryStore** | Long-term memory | InMemoryStore |
| **Tool** | Custom tools | 10 built-in tools |
| **Planner** | Task decomposition | LlmPlanner |
| **HookHandler** | Event handling | HookEngine |
| **HookExecutor** | Event execution | HookEngine |
| **McpTransport** | MCP protocol | StdioTransport |
| **HttpClient** | HTTP requests | ReqwestClient |
| **SessionCommand** | Queue tasks | ToolCommand |
| **LlmClient** | LLM interface | Anthropic/OpenAI |

#### Context Providers (RAG)

```rust
use a3s_code_core::context::{FileSystemContextProvider, FileSystemContextConfig};

let fs_provider = Arc::new(FileSystemContextProvider::new(
    FileSystemContextConfig::new(".")
        .with_include_patterns(vec!["**/*.rs".to_string()])
        .with_exclude_patterns(vec!["**/target/**".to_string()])
));

SessionOptions::new()
    .with_context_provider(fs_provider)
```

**Default FileSystemContextProvider:**
- File indexing (respects .gitignore)
- TF-IDF-style search
- Three depth levels (Abstract/Overview/Full)
- Caching

#### MCP Integration

Connect to external tools via Model Context Protocol

```rust
let mcp_client = McpClient::connect_stdio("path/to/mcp-server").await?;
session.add_mcp_client(mcp_client).await;
```

#### Lifecycle Hooks

8 hook events for custom logic

```rust
use a3s_code_core::hooks::{HookEngine, Hook, HookEventType};

let hook_engine = HookEngine::new();
hook_engine.register(Hook::new("audit", HookEventType::PreToolUse));

SessionOptions::new()
    .with_hook_engine(hook_engine)
```

**Hook Events:**
- PreToolUse, PostToolUse
- GenerateStart, GenerateEnd
- ContextWarning, ContextError
- PermissionDenied, ConfirmationRequired

#### Custom Implementations

Replace any default with your own implementation:

```rust
// Custom security provider
struct MySecurityProvider;

impl SecurityProvider for MySecurityProvider {
    fn taint_input(&self, text: &str) { /* ... */ }
    fn sanitize_output(&self, text: &str) -> String { /* ... */ }
}

SessionOptions::new()
    .with_security_provider(Arc::new(MySecurityProvider))
```

---

## Architecture

A3S Code follows a **core + extensions** architecture:

- **5 Core Components** (not replaceable): Agent, AgentSession, AgentLoop, ToolExecutor, LlmClient
- **14 Extension Points** (replaceable via traits): Security, Permissions, HITL, Context, Memory, Skills, Hooks, etc.
- **All extension points have default implementations** — works out of the box

```
Agent (config-driven, workspace-independent)
  ├── LlmClient (Anthropic / OpenAI)
  ├── SessionManager (multi-session support)
  └── AgentSession (workspace-bound)
        ├── AgentLoop (core execution engine)
        │     ├── ToolExecutor (10 built-in tools)
        │     ├── Planning (task decomposition)
        │     └── HITL Confirmation
        ├── SessionLaneQueue (a3s-lane backed)
        │     ├── Control Lane (P0)
        │     ├── Query Lane (P1)
        │     ├── Execute Lane (P2)
        │     └── Generate Lane (P3)
        ├── HookEngine (8 lifecycle events)
        ├── Security (sanitizer, taint tracking, injection detection)
        ├── Skills (instruction injection + tool permissions)
        ├── Context (RAG providers)
        └── Memory (episodic, semantic, procedural, working)
```

### Core Components (5)

1. **Agent** — Configuration management and session creation
2. **AgentSession** — Workspace-bound session management
3. **AgentLoop** — Core execution loop (LLM conversation, tool orchestration)
4. **ToolExecutor** — Tool registration and execution
5. **LlmClient** — LLM provider communication

### Design Philosophy

1. **Core Stability** — Execution engine is stable and unchanging
2. **Extension Flexibility** — All policies are replaceable via traits
3. **Out-of-the-Box Usability** — Default implementations for everything
4. **Progressive Enhancement** — Start simple, add complexity as needed

---

## Configuration

**Note:** A3S Code only supports HCL configuration format. JSON support has been removed.

### Minimal Config

```hcl
default_model = "anthropic/claude-sonnet-4-20250514"

providers {
  name    = "anthropic"
  api_key = env("ANTHROPIC_API_KEY")
}
```

### Full Config

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

# Queue configuration (a3s-lane v0.4.0 integration)
queue {
  # Concurrency limits
  query_max_concurrency = 10
  execute_max_concurrency = 5

  # Basic features
  enable_metrics = true
  enable_dlq = true
  storage_path = "./queue_data"

  # Advanced: Retry policy
  retry_policy {
    strategy = "exponential"  # exponential, fixed, or none
    max_retries = 3
    initial_delay_ms = 100
  }

  # Advanced: Rate limiting
  rate_limit {
    limit_type = "per_second"  # per_second, per_minute, per_hour, unlimited
    max_operations = 100
  }

  # Advanced: Priority boost
  priority_boost {
    strategy = "standard"  # standard, aggressive, or disabled
    deadline_ms = 300000
  }

  # Advanced: Pressure monitoring
  pressure_threshold = 50
}

# Search configuration (a3s-search integration)
search {
  timeout = 30

  health {
    max_failures = 3
    suspend_seconds = 60
  }

  engine {
    google {
      enabled = true
      weight = 1.5
    }
    bing {
      enabled = true
      weight = 1.0
    }
  }
}

# Storage backend
storage_backend = "file"
sessions_dir = "./sessions"

# Skill and agent directories
skill_dirs = ["./skills"]
agent_dirs = ["./agents"]

# Execution limits
max_tool_rounds = 50
thinking_budget = 10000
```

See `agent.example.hcl` for a complete configuration example with all available options.

### Session Options

```rust
SessionOptions::new()
    .with_default_security()           // Security
    .with_builtin_skills()              // Skills
    .with_planning(true)                // Planning
    .with_goal_tracking(true)           // Goal tracking
    .with_fs_context(".")               // RAG
    .with_queue_config(queue_config)    // Queue
    .with_permission_checker(policy)    // Permissions
    .with_confirmation_manager(mgr)     // HITL
    .with_hook_engine(hooks)            // Hooks
```

---

## API Reference

### Agent

```rust
// Create agent from HCL config file
let agent = Agent::new("agent.hcl").await?;

// Create agent from HCL string
let hcl = r#"
    default_model = "anthropic/claude-sonnet-4-20250514"
    providers {
        name = "anthropic"
        api_key = env("ANTHROPIC_API_KEY")
    }
"#;
let agent = Agent::new(hcl).await?;

// Create agent from config struct
let agent = Agent::from_config(config).await?;

// Create session
let session = agent.session(".", None)?;
let session = agent.session(".", Some(options))?;
```

**Note:** Only HCL format is supported. Use `.hcl` file extension or HCL string.

### AgentSession

```rust
// Send message
let result = session.send("What files handle auth?").await?;

// Stream response
let mut stream = session.stream("Refactor auth").await?;
while let Some(event) = stream.next().await {
    // Handle event
}

// Configure lane handler
session.set_lane_handler(SessionLane::Query, config).await;

// External task handling
let tasks = session.pending_external_tasks().await;
session.complete_external_task(&task_id, result).await;

// Queue stats
let stats = session.queue_stats().await;
let metrics = session.queue_metrics().await;
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
    .with_queue_config(config)
    .with_permission_checker(Arc::new(policy))
    .with_confirmation_manager(Arc::new(mgr))
    .with_context_provider(Arc::new(provider))
    .with_skill_registry(Arc::new(registry))
    .with_hook_engine(engine)
```

---

## Examples

### Basic Usage

```rust
let agent = Agent::new("agent.hcl").await?;
let session = agent.session(".", None)?;
let result = session.send("What files handle authentication?").await?;
println!("{}", result.text);
```

### With Security and Skills

```rust
let session = agent.session(".", Some(
    SessionOptions::new()
        .with_default_security()
        .with_builtin_skills()
))?;

let result = session.send("Review the auth code for security issues").await?;
```

### With Planning

```rust
let session = agent.session(".", Some(
    SessionOptions::new()
        .with_planning(true)
        .with_goal_tracking(true)
))?;

let result = session.send("Refactor auth + update tests + update docs").await?;
```

### With External Task Distribution

```rust
let queue_config = SessionQueueConfig {
    query_max_concurrency: 10,
    enable_metrics: true,
    ..Default::default()
};

let session = agent.session(".", Some(
    SessionOptions::new()
        .with_queue_config(queue_config)
))?;

session.set_lane_handler(SessionLane::Query, LaneHandlerConfig {
    mode: TaskHandlerMode::External,
    timeout_ms: 120_000,
}).await;

// Worker loop
tokio::spawn(async move {
    loop {
        let tasks = session.pending_external_tasks().await;
        for task in tasks {
            let result = execute_task(&task).await;
            session.complete_external_task(&task.task_id, result).await;
        }
    }
});
```

### Code Examples

See `core/examples/`:
- `default_implementations.rs` — Security, context, HITL demo
- `skills_demo.rs` — Skills system demo

Run examples:
```bash
cargo run --example default_implementations
cargo run --example skills_demo
```

---

## Testing

```bash
# Run all tests
cargo test

# Run specific test suite
cargo test --lib skills::
cargo test --test skill_permissions_test
cargo test --test integration

# Run with output
cargo test -- --nocapture
```

**Test Coverage:** 1174 tests, 100% pass rate

- 1137 unit tests
- 25 integration tests
- 5 skill permission tests
- 5 skill system prompt tests
- 2 doc tests

---

## Contributing

We welcome contributions! Please follow these guidelines:

### Quick Checklist

- Follow Rust API guidelines
- Write tests for all new code
- Use `cargo fmt` and `cargo clippy`
- Update documentation
- Use Conventional Commits

### Development Workflow

1. Fork and clone the repository
2. Create a feature branch: `git checkout -b feature/my-feature`
3. Make changes and write tests
4. Run tests: `cargo test`
5. Format code: `cargo fmt`
6. Check lints: `cargo clippy`
7. Commit: `git commit -m "feat: add my feature"`
8. Push and create PR

### Commit Message Format

Use [Conventional Commits](https://www.conventionalcommits.org/):

- `feat:` — New feature
- `fix:` — Bug fix
- `docs:` — Documentation only
- `test:` — Adding or updating tests
- `refactor:` — Code change that neither fixes a bug nor adds a feature
- `perf:` — Performance improvement
- `chore:` — Maintenance tasks

---

## License

MIT License - see [LICENSE](./LICENSE)

---

## Related Projects

- **[A3S Box](../box)** — Secure sandbox runtime with TEE support
- **[A3S Lane](../lane)** — Priority-based task queue with DLQ
- **[A3S Event](../event)** — Event-driven architecture primitives
- **[A3S Search](../search)** — Multi-engine web search aggregator

---

**Built by [A3S Lab](https://github.com/A3S-Lab)** | [Documentation](https://docs.a3s.dev) | [Discord](https://discord.gg/a3s)
