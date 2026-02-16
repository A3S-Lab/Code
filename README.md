# A3S Code

<p align="center">
  <strong>Embeddable AI Coding Agent Library</strong>
</p>

<p align="center">
  <em>Library-first Rust framework for building AI coding agents — embed directly, no server required</em>
</p>

<p align="center">
  <a href="#features">Features</a> •
  <a href="#quick-start">Quick Start</a> •
  <a href="#configuration">Configuration</a> •
  <a href="#architecture">Architecture</a> •
  <a href="#development">Development</a>
</p>

---

## Overview

**A3S Code** is an embeddable Rust library (`a3s-code-core`) for building AI coding agents. All subsystems — hooks, security, memory, MCP/LSP, subagent delegation, planning — are wired into the core execution path and active by default. Configure via HCL or JSON, create an `Agent`, bind sessions to workspaces, and go.

```rust
use a3s_code_core::{Agent, AgentEvent};

// Load config (HCL or JSON) — one line
let agent = Agent::new("agent.hcl").await?;

// Create a workspace-bound session
let session = agent.session("/my-project");

// Non-streaming
let result = session.send("What files handle auth?").await?;
println!("{}", result.text);

// Streaming
let (mut rx, _handle) = session.stream("Refactor auth").await?;
while let Some(event) = rx.recv().await {
    match event {
        AgentEvent::TextDelta { text } => print!("{text}"),
        AgentEvent::End { .. } => break,
        _ => {}
    }
}

// Direct tool calls (no LLM, no serialization)
let content = session.read_file("src/main.rs").await?;
let files = session.glob("**/*.rs").await?;
let output = session.bash("cargo test").await?;
```

## Features

- **Library-First**: Embeddable `a3s-code-core` with `Agent` / `AgentSession` facade — use directly in Rust, no server, no serialization
- **Config-Driven**: Multi-provider LLM configuration via HCL or JSON files, with per-model API key and base URL overrides
- **Session-Per-Workspace**: `Agent` holds LLM config; `AgentSession` binds to a workspace directory — share one agent across many sessions
- **11 Built-in Tools**: bash, read, write, edit, patch, grep, glob, ls, web_fetch, web_search, cron
- **Permission System**: Allow/Deny/Ask rules for fine-grained tool access control
- **Human-in-the-Loop (HITL)**: Require user confirmation before sensitive operations
- **Skills System**: Extend the agent with Markdown-based prompt-injection skills (compatible with Claude Code Skills format)
- **Subagent Delegation**: Delegate specialized tasks to focused child agents (explore, general, plan)
- **LSP Integration**: Code intelligence via Language Server Protocol (hover, definition, references, symbols, diagnostics)
- **MCP Support**: Extend with external tools via Model Context Protocol
- **Hooks System**: 8 lifecycle events (PreToolUse, PostToolUse, GenerateStart/End, SessionStart/End, SkillLoad/Unload)
- **Security**: Output sanitization, taint tracking, injection detection, workspace boundary enforcement
- **Memory System**: Episodic, semantic, procedural, and working memory for persistent knowledge
- **Planning & Goal Tracking**: Create execution plans and track goal achievement
- **Context Compaction**: Auto-summarize long conversations at configurable threshold (default 80%)
- **Cron Scheduling**: Schedule recurring tasks with cron expressions or natural language
- **Thinking Model Support**: Full support for reasoning models (kimi-k2.5, DeepSeek-R1) with reasoning_content preservation
- **API Retry with Backoff**: Automatic retry with exponential backoff for transient LLM errors (429, 500, 502, 503, 529)
- **File Version History**: Automatic snapshots before write/edit/patch with diff and restore
- **Per-Session Cost Tracking**: Token cost calculation using model-specific pricing

## Quality Metrics

**1,492 unit tests** (0 failures, 3 ignored):

```bash
just test
```

## Quick Start

### Add Dependency

```toml
# Cargo.toml
[dependencies]
a3s-code-core = "0.1"
```

## Configuration

A3S Code uses **HCL** (preferred) or **JSON** for configuration. Format is auto-detected by file extension.

### HCL (Recommended)

```hcl
default_provider = "anthropic"
default_model    = "claude-sonnet-4-20250514"

providers {
  name    = "anthropic"
  api_key = "sk-ant-..."

  models {
    id        = "claude-sonnet-4-20250514"
    name      = "Claude Sonnet 4"
    tool_call = true
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

### JSON

```json
{
  "defaultProvider": "anthropic",
  "defaultModel": "claude-sonnet-4-20250514",
  "providers": [
    {
      "name": "anthropic",
      "apiKey": "sk-ant-...",
      "models": [
        {
          "id": "claude-sonnet-4-20250514",
          "name": "Claude Sonnet 4",
          "toolCall": true
        }
      ]
    }
  ]
}
```

### Programmatic Configuration

```rust
use a3s_code_core::{Agent, CodeConfig, ProviderConfig, ModelConfig};

// From struct
let config = CodeConfig {
    default_provider: Some("anthropic".into()),
    default_model: Some("claude-sonnet-4-20250514".into()),
    providers: vec![ProviderConfig {
        name: "anthropic".into(),
        api_key: Some("sk-ant-...".into()),
        base_url: None,
        models: vec![ModelConfig {
            id: "claude-sonnet-4-20250514".into(),
            name: "Claude Sonnet 4".into(),
            ..serde_json::from_value(serde_json::json!({"id": "claude-sonnet-4-20250514"})).unwrap()
        }],
    }],
    ..Default::default()
};

let agent = Agent::from_config(config).await?;

// Or from file path (auto-detects HCL/JSON)
let agent = Agent::new("agent.hcl").await?;

// Or from inline strings (auto-detects JSON vs HCL)
let agent = Agent::new(r#"{"defaultProvider": "anthropic", ...}"#).await?;
```

### Config Options

```hcl
default_provider = "anthropic"
default_model    = "claude-sonnet-4-20250514"
system_prompt    = "You are a senior Rust engineer."
max_tool_rounds  = 20

providers {
  name    = "anthropic"
  api_key = "sk-ant-..."

  models {
    id   = "claude-sonnet-4-20250514"
    name = "Claude Sonnet 4"
  }
}
```

## Architecture

```
┌────────────────────────────────────────────────────────────┐
│  Agent (config-driven, workspace-independent)               │
│  ┌──────────────┬──────────────┬─────────────────────────┐ │
│  │  LLM Client  │ Tool Executor│   AgentConfig           │ │
│  │  (shared)    │  (shared)    │  (system prompt, tools)  │ │
│  └──────────────┴──────────────┴─────────────────────────┘ │
│                         │                                   │
│          agent.session("/workspace")                        │
│                         ▼                                   │
│  ┌─────────────────────────────────────────────────────┐   │
│  │  AgentSession (workspace-bound)                      │   │
│  │  ┌───────────┬───────────┬───────────┬────────────┐ │   │
│  │  │  Agent    │   Tool    │ Permission│    LLM     │ │   │
│  │  │  Loop     │  Context  │  System   │  Provider  │ │   │
│  │  └───────────┴───────────┴───────────┴────────────┘ │   │
│  │  ┌───────────┬───────────┬───────────┬────────────┐ │   │
│  │  │  Skills   │ Subagent  │    LSP    │    MCP     │ │   │
│  │  │  System   │ Task Tool │  (auto)   │   (auto)   │ │   │
│  │  └───────────┴───────────┴───────────┴────────────┘ │   │
│  │  ┌───────────┬───────────┬───────────┬────────────┐ │   │
│  │  │   Hooks   │ Security  │  Memory   │  Planning  │ │   │
│  │  │  Engine   │  Guard    │ (Context) │  & Goals   │ │   │
│  │  └───────────┴───────────┴───────────┴────────────┘ │   │
│  │  ┌───────────┬───────────┬───────────┐              │   │
│  │  │   Cron    │  Context  │   Cost    │              │   │
│  │  │ Scheduler │ Compaction│ Tracking  │              │   │
│  │  └───────────┴───────────┴───────────┘              │   │
│  └─────────────────────────────────────────────────────┘   │
└────────────────────────────────────────────────────────────┘
```

### Built-in Tools

| Tool | Purpose | Example |
|------|---------|---------|
| `bash` | Execute shell commands | `git status`, `npm install` |
| `read` | Read files with line numbers | View source code |
| `write` | Create/overwrite files | Create new files |
| `edit` | String replacement editing | Modify existing code |
| `patch` | Apply unified diff patches | Complex multi-line edits |
| `grep` | Search file contents (ripgrep) | Find function definitions |
| `glob` | Find files by pattern | `**/*.ts`, `src/**/*.rs` |
| `ls` | List directory contents | Explore project structure |
| `web_fetch` | Fetch web content | Download documentation |
| `web_search` | Search the web | Query search engines |
| `cron` | Manage scheduled tasks | Create/list/run cron jobs |

### Subagent Types

| Agent | Permissions | Use Case |
|-------|-------------|----------|
| `explore` | read, grep, glob, ls | Find code, understand structure |
| `general` | all except task | Complex multi-step tasks |
| `plan` | read, grep, glob, ls | Design implementation approach |

## Development

### Dependencies

| Dependency | Install | Purpose |
|------------|---------|---------|
| `just` | `cargo install just` | Task runner |
| `cargo-llvm-cov` | `cargo install cargo-llvm-cov` | Code coverage (optional) |

### Build Commands

```bash
just build          # Debug build
just release        # Release build
just test           # All tests
just fmt            # Format code
just lint           # Clippy lint
just ci             # Full CI checks (fmt + lint + test)
just check          # Fast compile check
just doc            # Generate and open docs
```

### Project Structure

```
code/                          # Cargo workspace root
├── Cargo.toml                 # Workspace manifest
├── justfile
├── README.md
├── core/                      # a3s-code-core — embeddable library
│   ├── Cargo.toml
│   ├── prompts/               # Prompt templates
│   ├── skills/                # Built-in skills (tool definitions, skill discovery)
│   └── src/
│       ├── lib.rs             # Library entry point + re-exports
│       ├── agent_api.rs       # Agent / AgentSession facade
│       ├── agent.rs           # Agentic loop execution engine
│       ├── config.rs          # Configuration (HCL + JSON)
│       ├── session.rs         # Session management
│       ├── llm.rs             # LLM provider integration (Anthropic, OpenAI-compatible)
│       ├── tools/             # Tool system (built-in + dynamic + skills)
│       ├── subagent.rs        # Subagent delegation (explore, general, plan)
│       ├── permissions.rs     # Permission system (allow/deny/ask)
│       ├── hitl.rs            # Human-in-the-loop confirmation
│       ├── hooks/             # Hook engine (8 lifecycle events)
│       ├── security/          # Security guards (sanitizer, taint, injection)
│       ├── lsp/               # Language Server Protocol integration
│       ├── mcp/               # Model Context Protocol support
│       ├── memory.rs          # Memory system (episodic, semantic, procedural, working)
│       ├── planning/          # Planning & goal tracking
│       ├── context.rs         # Context compaction
│       ├── session_lane_queue.rs # Priority queue (a3s-lane)
│       └── telemetry.rs       # Metrics via tracing events
├── sdk/
│   ├── node-native/           # Native Node.js addon (napi-rs)
│   └── python-native/         # Native Python module (PyO3)
└── docs/
    └── signoz-dashboard.json  # Pre-built SigNoz observability dashboard
```

## A3S Ecosystem

A3S Code is the **application layer** of the A3S ecosystem.

| Project | Package | Relationship |
|---------|---------|--------------|
| **box** | `a3s-box-*` | MicroVM sandbox runtime that hosts `a3s-code` |
| **code** | `a3s-code-core` | AI coding agent library (this project) |
| **lane** | `a3s-lane` | Priority queue used for command scheduling |

## Roadmap

### Phase 1–9: Complete ✅

Core agent loop, 11 tools, multi-session management, permission system, HITL, skills, subagents, LSP/MCP integration, cron scheduling, memory system, planning, security hardening (5-layer defense-in-depth), context compaction, file history, cost tracking, API retry, OpenTelemetry observability.

### Phase 10: Library-First Architecture ✅

- [x] Extract all business logic into `a3s-code-core` — pure Rust library, zero server dependencies
- [x] `Agent::new()` / `Agent::from_config()` facade with config-driven constructors
- [x] HCL and JSON configuration support with auto-detection by file extension
- [x] Multi-provider LLM config (default model required, multiple providers optional)
- [x] All 11 tools callable via direct function calls without serialization
- [x] Native Python bindings (PyO3) and Node.js bindings (napi-rs)
- [x] 1,492 unit tests

### Phase 11: Multi-Model Routing 🚧

- [ ] Smart model router: auto-select model by task complexity
- [ ] Cost-aware routing with budget constraints per session
- [ ] Fallback chain: automatic failover across providers on error/rate-limit

## License

MIT

---

<p align="center">
  Built by <a href="https://github.com/a3s-lab">A3S Lab</a>
</p>
