# A3S Code

**Embeddable AI coding agent framework in Rust** — Build AI agents that can read, write, and execute code with full tool access, planning, and multi-language SDKs.

```rust
let agent = Agent::new("agent.hcl").await?;
let session = agent.session("/my-project", None)?;
let result = session.send("Refactor auth to use JWT").await?;
```

## Why A3S Code?

- **Embeddable** — Rust library with Node.js and Python bindings
- **Production-Ready** — Permission system, HITL confirmation, cost tracking
- **Extensible** — 14 built-in tools + MCP protocol for external tools
- **Intelligent** — Parallel plan execution, goal tracking, memory system

## Quick Start

### Installation

```bash
# Rust
cargo add a3s-code-core

# Node.js
npm install @a3s-lab/code

# Python
pip install a3s-code
```

### Minimal Example

**1. Create config file** (`agent.hcl`):

```hcl
default_model = "anthropic/claude-sonnet-4-20250514"

providers {
  name    = "anthropic"
  api_key = "sk-ant-..."
}
```

**2. Use the agent**:

<details>
<summary><b>Rust</b></summary>

```rust
use a3s_code_core::{Agent, AgentEvent};

let agent = Agent::new("agent.hcl").await?;
let session = agent.session("/my-project", None)?;

// Non-streaming
let result = session.send("What files handle auth?").await?;
println!("{}", result.text);

// Streaming
let (mut rx, _) = session.stream("Refactor auth").await?;
while let Some(event) = rx.recv().await {
    match event {
        AgentEvent::TextDelta { text } => print!("{text}"),
        AgentEvent::End { .. } => break,
        _ => {}
    }
}
```

</details>

<details>
<summary><b>TypeScript</b></summary>

```typescript
const { Agent } = require('@a3s-lab/code');

const agent = await Agent.create('agent.hcl');
const session = agent.session('/my-project');

// Non-streaming
const result = await session.send('What files handle auth?');
console.log(result.text);

// Streaming
for await (const event of session.stream('Refactor auth')) {
  if (event.type === 'text_delta') {
    process.stdout.write(event.text);
  }
}
```

</details>

<details>
<summary><b>Python</b></summary>

```python
from a3s_code import Agent

agent = Agent.create("agent.hcl")
session = agent.session("/my-project")

# Non-streaming
result = session.send("What files handle auth?")
print(result.text)

# Streaming
for event in session.stream("Refactor auth"):
    if event.event_type == "text_delta":
        print(event.text, end="", flush=True)
```

</details>

## Core Features

### Built-in Tools (14)

**File Operations:** `read`, `write`, `edit`, `patch`, `grep`, `glob`, `ls`
**Execution:** `bash`, `cron`
**Web:** `web_fetch`, `web_search`
**Skills:** `search_skills`, `install_skill`, `load_skill`

### Permission System

```rust
// Allow/Deny/Ask rules per tool
session.set_permission_policy(PermissionPolicy::new()
    .allow("read")
    .deny("bash")
    .ask("write")
).await;
```

### Planning & Parallel Execution

```rust
let session = agent.session("/project", Some(
    SessionOptions::new()
        .with_planning(true)
        .with_goal_tracking(true)
))?;

// Agent decomposes task into parallel steps
session.send("Refactor auth + update tests").await?;
```

### Multi-Machine Distribution

```rust
// Offload tool execution to remote workers
session.set_lane_handler(SessionLane::Execute, LaneHandlerConfig {
    mode: TaskHandlerMode::External,
    timeout_ms: 120_000,
}).await;

// Poll for tasks and dispatch to workers
let tasks = session.pending_external_tasks().await;
for task in tasks {
    let result = send_to_remote_worker(&task).await;
    session.complete_external_task(&task.task_id, result).await;
}
```

## Advanced Features

<details>
<summary><b>Human-in-the-Loop (HITL)</b></summary>

```rust
// Require confirmation before sensitive operations
let session = agent.session("/project", Some(
    SessionOptions::new()
        .with_confirmation_policy(ConfirmationPolicy::enabled())
))?;

// Agent will emit ConfirmationRequired events
// Respond via session.confirm_tool(tool_id, approved).await
```

</details>

<details>
<summary><b>Lifecycle Hooks</b></summary>

```rust
// 8 hook events: PreToolUse, PostToolUse, GenerateStart, GenerateEnd, etc.
let hook_engine = HookEngine::new();
hook_engine.register(Hook::new("security-check", HookEventType::PreToolUse));

let session = agent.session("/project", Some(
    SessionOptions::new().with_hook_engine(hook_engine)
))?;
```

</details>

<details>
<summary><b>Memory System</b></summary>

```rust
// 4 memory types: episodic, semantic, procedural, working
session.remember_success("Implemented JWT", &["auth", "security"]).await?;
let memories = session.recall("authentication").await?;
```

</details>

<details>
<summary><b>MCP Integration</b></summary>

```rust
// Connect to external tools via Model Context Protocol
let mcp_client = McpClient::connect_stdio("path/to/mcp-server").await?;
session.add_mcp_client(mcp_client).await;
```

</details>

<details>
<summary><b>Cost Tracking</b></summary>

```rust
// Per-session token cost calculation
let usage = session.context_usage().await?;
println!("Cost: ${:.4}", usage.total_cost);
```

</details>

## Configuration

<details>
<summary><b>Complete HCL Example</b></summary>

```hcl
default_model = "anthropic/claude-sonnet-4-20250514"

max_tool_rounds  = 20
thinking_budget  = 4096
skill_dirs       = ["./skills"]
storage_backend  = "file"
sessions_dir     = "/tmp/a3s"

providers {
  name    = "anthropic"
  api_key = "sk-ant-..."

  models {
    id          = "claude-sonnet-4-20250514"
    name        = "Claude Sonnet 4"
    tool_call   = true
    cost {
      input       = 3.0
      output      = 15.0
      cache_read  = 0.3
      cache_write = 3.75
    }
    limit {
      context = 200000
      output  = 8192
    }
  }
}

providers {
  name     = "openai"
  api_key  = "sk-..."
  base_url = "https://api.openai.com"

  models {
    id        = "gpt-4o"
    name      = "GPT-4o"
    tool_call = true
  }
}
```

</details>

<details>
<summary><b>JSON Format</b></summary>

```json
{
  "defaultModel": "anthropic/claude-sonnet-4-20250514",
  "maxToolRounds": 20,
  "providers": [
    {
      "name": "anthropic",
      "apiKey": "sk-ant-...",
      "models": [
        {
          "id": "claude-sonnet-4-20250514",
          "toolCall": true
        }
      ]
    }
  ]
}
```

</details>

## Architecture

```
Agent (config-driven)
  ├─ LlmClient (Anthropic/OpenAI)
  ├─ SessionManager (multi-session)
  └─ AgentSession (workspace-bound)
       ├─ AgentLoop (execution engine)
       ├─ ToolExecutor (14 tools)
       ├─ LlmPlanner (parallel execution)
       ├─ PermissionSystem
       ├─ HookEngine (8 events)
       ├─ Memory (4 types)
       ├─ MCP (external tools)
       └─ Cost Tracking
```

## Documentation

- **[Full API Reference](./docs/api.md)** — Complete API documentation
- **[Configuration Guide](./docs/configuration.md)** — All config options
- **[Planning & Parallelism](./docs/planning.md)** — Execution plans and wave scheduling
- **[External Task Offloading](./docs/external-tasks.md)** — Multi-machine distribution
- **[Examples](./examples/)** — Code examples for common use cases

## Development

```bash
just build          # Debug build
just test           # Run tests (1104 tests)
just lint           # Clippy + fmt check
just ci             # Full CI pipeline
just doc            # Generate docs
```

## License

MIT — Built by [A3S Lab](https://github.com/a3s-lab)
