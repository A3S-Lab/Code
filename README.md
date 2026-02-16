# A3S Code

<p align="center">
  <strong>AI Coding Agent Framework</strong>
</p>

<p align="center">
  <em>Rust framework for building AI coding agents</em>
</p>

<p align="center">
  <a href="#features">Features</a> •
  <a href="#api">API</a> •
  <a href="#configuration">Configuration</a> •
  <a href="#architecture">Architecture</a> •
  <a href="#development">Development</a>
</p>

---

## Overview

**A3S Code** is an embeddable Rust library (`a3s-code-core`) for building AI coding agents. All subsystems — hooks, security, memory, MCP, subagent delegation, planning — are wired into the core and active by default. Native bindings for Node.js (napi-rs) and Python (PyO3) included.

## Features

- **Native SDKs** — Node.js (napi-rs) and Python (PyO3) bindings
- **Config-Driven** — Multi-provider LLM config via HCL or JSON
- **Session-Per-Workspace** — Share one agent across many workspaces
- **Per-Session Model Override** — Use different models per session via `provider/model`
- **14 Built-in Tools** — 11 core tools (bash, read, write, edit, patch, grep, glob, ls, web_fetch, web_search, cron) + 3 skill discovery tools (search_skills, install_skill, load_skill)
- **Permission System** — Allow/Deny/Ask rules for tool access
- **Human-in-the-Loop** — Confirmation before sensitive operations
- **Skills** — Markdown-based prompt-injection skills with runtime discovery and installation
- **Subagents** — Focused child agents (explore, general, plan)
- **MCP** — External tools via Model Context Protocol (JSON-RPC 2.0, stdio + HTTP+SSE transports)
- **8 Lifecycle Hooks** — Pre/post events for tool calls, sessions, messages, and errors
- **Security** — 5 layers: sanitizer, taint tracking, interceptor, injection detection, audit logging
- **Memory** — 4 types: episodic, semantic, procedural, working memory
- **JSON-Structured Planning** — Execution plans and goal tracking via LlmPlanner
- **Context Compaction** — Auto-summarize long conversations (80% threshold)
- **Context Store** — Persistent context storage (feature-gated: `context-store`)
- **File History** — Auto-snapshots (500-snapshot capacity) with diff and restore
- **Cost Tracking** — Per-session token cost calculation with per-model pricing
- **Thinking Models** — Reasoning models with reasoning_content
- **API Retry** — Exponential backoff for transient errors
- **Cron** — Recurring tasks via cron expressions
- **`#[non_exhaustive]` Events** — AgentEvent uses `#[non_exhaustive]` for safe SDK evolution

## API

All three languages follow the same pattern: `Agent.create(config)` → `agent.session(workspace)` → `session.send(prompt)`.

### Rust

```toml
[dependencies]
a3s-code-core = "0.7"
```

```rust
use a3s_code_core::{Agent, AgentEvent, SessionOptions};

// Create agent from config file or inline string
let agent = Agent::new("agent.hcl").await?;

// Bind session to workspace (uses default model)
let session = agent.session("/my-project", None)?;

// Override model for this session
let session = agent.session("/my-project", Some(
    SessionOptions::new()
        .with_model("openai/gpt-4o")
))?;

// Non-streaming
let result = session.send("What files handle auth?").await?;
println!("{}", result.text);

// Streaming (AgentEvent is #[non_exhaustive] — always include a wildcard arm)
let (mut rx, _handle) = session.stream("Refactor auth").await?;
while let Some(event) = rx.recv().await {
    match event {
        AgentEvent::TextDelta { text } => print!("{text}"),
        AgentEvent::ToolStart { name, .. } => println!("[tool: {name}]"),
        AgentEvent::End { .. } => break,
        _ => {} // required: AgentEvent is #[non_exhaustive]
    }
}

// Direct tool calls (no LLM)
session.read_file("src/main.rs").await?;
session.bash("cargo test").await?;
session.glob("**/*.rs").await?;
session.grep("fn main").await?;
session.tool("write", json!({"path": "x.rs", "content": "..."})).await?;
```

### TypeScript

```bash
npm install @a3s-lab/code
```

```typescript
const { Agent } = require('@a3s-lab/code');

// Create agent from config file or inline string
const agent = await Agent.create('agent.hcl');

// Bind session to workspace (uses default model)
const session = agent.session('/my-project');

// Override model for this session
const session = agent.session('/my-project', {
  model: 'openai/gpt-4o',
});

// LLM interaction
const result = await session.send('prompt');                   // non-streaming
const events = await session.stream('prompt');                 // streaming

// Direct tool calls (no LLM)
await session.readFile('src/main.rs');
await session.bash('cargo test');
await session.glob('**/*.rs');
await session.grep('fn main');
await session.tool('write', { path: 'x.rs', content: '...' });
```

### Python

```bash
pip install a3s-code
```

```python
from a3s_code import Agent

# Create agent from config file or inline string
agent = Agent.create("agent.hcl")

# Bind session to workspace (uses default model)
session = agent.session("/my-project")

# Override model for this session
session = agent.session("/my-project", model="openai/gpt-4o")

# LLM interaction
result = session.send("prompt")                                # non-streaming
for event in session.stream("prompt"):                         # streaming
    if event.event_type == "text_delta":
        print(event.text, end="", flush=True)

# Direct tool calls (no LLM)
session.read_file("src/main.rs")
session.bash("cargo test")
session.glob("**/*.rs")
session.grep("fn main")
session.tool("write", {"path": "x.rs", "content": "..."})
```

## Configuration

HCL (preferred) or JSON. Auto-detected by file extension.

### Complete HCL Reference

```hcl
# === LLM (required) ===
default_model = "anthropic/claude-sonnet-4-20250514"

# === Agent Behavior ===
max_tool_rounds  = 20          # default: 50
thinking_budget  = 4096        # reasoning token budget

# === Extensions ===
skill_dirs = ["./skills"]      # *.md skill files
agent_dirs = ["./agents"]      # *.yaml/*.md agent files

# === Storage ===
storage_backend = "file"       # "memory" | "file" | "custom"
sessions_dir    = "/tmp/a3s"   # session persistence path
storage_url     = "redis://localhost:6379"

# === Providers ===
providers {
  name    = "anthropic"
  api_key = "sk-ant-..."

  models {
    id          = "claude-sonnet-4-20250514"
    name        = "Claude Sonnet 4"
    family      = "claude-sonnet"
    tool_call   = true
    temperature = true
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

  models {
    id        = "claude-opus-4-20250514"
    name      = "Claude Opus 4"
    reasoning = true
    tool_call = true
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

### Complete JSON Reference

```json
{
  "defaultModel": "anthropic/claude-sonnet-4-20250514",
  "maxToolRounds": 20,
  "thinkingBudget": 4096,
  "skillDirs": ["./skills"],
  "agentDirs": ["./agents"],
  "storageBackend": "file",
  "sessionsDir": "/tmp/a3s",
  "storageUrl": "redis://localhost:6379",
  "providers": [
    {
      "name": "anthropic",
      "apiKey": "sk-ant-...",
      "models": [
        {
          "id": "claude-sonnet-4-20250514",
          "name": "Claude Sonnet 4",
          "family": "claude-sonnet",
          "toolCall": true,
          "temperature": true,
          "cost": { "input": 3.0, "output": 15.0, "cacheRead": 0.3, "cacheWrite": 3.75 },
          "limit": { "context": 200000, "output": 8192 }
        }
      ]
    }
  ]
}
```

### Config Options

| Field | HCL | JSON | Type | Default |
|-------|-----|------|------|---------|
| Default model | `default_model` | `defaultModel` | `string` | — (required, format: `"provider/model"`) |
| Max tool rounds | `max_tool_rounds` | `maxToolRounds` | `int?` | `50` |
| Thinking budget | `thinking_budget` | `thinkingBudget` | `int?` | `null` |
| Skill dirs | `skill_dirs` | `skillDirs` | `string[]` | `[]` |
| Agent dirs | `agent_dirs` | `agentDirs` | `string[]` | `[]` |
| Storage backend | `storage_backend` | `storageBackend` | `string` | `"file"` |
| Sessions dir | `sessions_dir` | `sessionsDir` | `string?` | `null` |
| Storage URL | `storage_url` | `storageUrl` | `string?` | `null` |

> **Note:** `skill_dirs` and `agent_dirs` are set in `CodeConfig` (agent-level), not in `SessionOptions` (session-level). Sessions only override the model.

## Architecture

```
┌──────────────────────────────────────────────────────┐
│  Agent (config-driven, workspace-independent)         │
│  ┌────────────┬──────────────┬─────────────────────┐ │
│  │ LlmClient  │  CodeConfig  │   SessionManager    │ │
│  └────────────┴──────────────┴─────────────────────┘ │
│                       │                               │
│        agent.session("/workspace", options)            │
│                       ▼                               │
│  ┌──────────────────────────────────────────────┐    │
│  │  AgentSession (workspace-bound)               │    │
│  │  ┌─────────┬──────────┬──────────┬─────────┐ │    │
│  │  │ Agent   │ Tool     │Permission│  LLM    │ │    │
│  │  │ Loop    │ Executor │ System   │ Provider│ │    │
│  │  │         │ (14)     │          │         │ │    │
│  │  ├─────────┼──────────┼──────────┼─────────┤ │    │
│  │  │ Skills  │ Subagent │  Hook    │  MCP    │ │    │
│  │  │         │          │  Engine  │         │ │    │
│  │  ├─────────┼──────────┼──────────┼─────────┤ │    │
│  │  │ Llm     │ Security │ Memory   │ File    │ │    │
│  │  │ Planner │          │          │ History │ │    │
│  │  ├─────────┼──────────┼──────────┼─────────┤ │    │
│  │  │ Context │ Cost     │ Cron     │ Session │ │    │
│  │  │Compactor│ Tracking │Scheduler │ Store   │ │    │
│  │  └─────────┴──────────┴──────────┴─────────┘ │    │
│  └──────────────────────────────────────────────┘    │
└──────────────────────────────────────────────────────┘
```

### Built-in Tools (14)

#### Core Tools (11)

| Tool | Purpose |
|------|---------|
| `bash` | Execute shell commands |
| `read` | Read files with line numbers |
| `write` | Create/overwrite files |
| `edit` | String replacement editing |
| `patch` | Apply unified diff patches |
| `grep` | Search file contents (ripgrep) |
| `glob` | Find files by pattern |
| `ls` | List directory contents |
| `web_fetch` | Fetch web content |
| `web_search` | Search the web |
| `cron` | Manage scheduled tasks |

#### Skill Discovery Tools (3)

| Tool | Purpose |
|------|---------|
| `search_skills` | Search GitHub for available skills |
| `install_skill` | Install a skill from GitHub |
| `load_skill` | Load and register an installed skill |

### Subagent Types

| Agent | Permissions | Use Case |
|-------|-------------|----------|
| `explore` | read, grep, glob, ls | Find code, understand structure |
| `general` | all except task | Complex multi-step tasks |
| `plan` | read, grep, glob, ls | Design implementation approach |

## Development

```bash
just build          # Debug build
just release        # Release build
just test           # All tests
just test-cov       # Tests with coverage
just fmt            # Format code
just lint           # Clippy lint
just ci             # Full CI (fmt + lint + test)
just check          # Fast compile check
just doc            # Generate and open docs
just publish        # Publish to crates.io
just version        # Show current version
```

### Project Structure

```
code/
├── core/                      # a3s-code-core library
│   ├── prompts/               # Prompt templates
│   ├── skills/                # Built-in skills (tool definitions)
│   └── src/
│       ├── lib.rs             # Entry point + re-exports
│       ├── agent.rs           # AgentLoop, AgentEvent, AgentConfig
│       ├── agent_api.rs       # Agent facade, AgentSession, SessionOptions
│       ├── config.rs          # CodeConfig (HCL + JSON)
│       ├── session.rs         # SessionManager, SessionState
│       ├── llm.rs             # LLM providers (Anthropic, OpenAI)
│       ├── tools/             # ToolExecutor, ToolRegistry, skill discovery
│       ├── permissions.rs     # Permission system
│       ├── hitl.rs            # Human-in-the-loop
│       ├── hooks/             # HookEngine (8 lifecycle events)
│       ├── security/          # Sanitizer, taint tracking, injection detection, audit
│       ├── memory.rs          # Episodic, semantic, procedural, working memory
│       ├── planning/          # LlmPlanner, execution plans, goal tracking
│       ├── context.rs         # Context compaction
│       ├── context_store/     # Persistent context store (feature-gated)
│       ├── mcp/               # Model Context Protocol integration
│       ├── store.rs           # Session persistence
│       ├── queue.rs           # Command queue
│       ├── session_lane_queue.rs  # Lane-based queue
│       ├── file_history.rs    # Auto-snapshots (500 capacity)
│       ├── telemetry.rs       # Metrics and cost tracking
│       ├── prompts.rs         # System prompts (pub(crate))
│       ├── retry.rs           # Exponential backoff (pub(crate))
│       └── subagent.rs        # Subagent delegation (pub(crate))
├── sdk/
│   ├── node/                  # Node.js addon (napi-rs)
│   └── python/                # Python module (PyO3)
└── .github/
    └── workflows/             # CI/CD (release, publish-node, publish-python)
```

## License

MIT

---

<p align="center">
  Built by <a href="https://github.com/a3s-lab">A3S Lab</a>
</p>
