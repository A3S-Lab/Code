# A3S Code

<p align="center">
  <strong>Sandboxed AI Coding Agent</strong>
</p>

<p align="center">
  <em>Application layer — tool execution, multi-session management, and extensible context providers</em>
</p>

<p align="center">
  <a href="#features">Features</a> •
  <a href="#architecture">Architecture</a> •
  <a href="#quick-start">Quick Start</a> •
  <a href="#development">Development</a>
</p>

---

## Overview

**A3S Code** is a Rust-based AI coding agent designed to run inside [A3S Box](https://github.com/A3S-Lab/Box) sandboxes. It provides tool execution, streaming responses, permission policies, and extensible context integration.

## Features

- **Multi-session Management** - Independent conversation histories with persistence
- **Streaming Responses** - Real-time event streaming for UI updates
- **Tool Calling** - 7 built-in tools (bash, read, write, edit, grep, glob, ls)
- **Dynamic Tools** - HTTP, binary, and script-based tools via skill system
- **Context Providers** - Extension point for external memory/knowledge bases
- **Permission Policies** - Declarative Allow/Deny/Ask rules for tool access
- **Human-in-the-Loop** - Confirmation system for sensitive operations
- **Hook System** - Lifecycle event interception (PreToolUse, PostToolUse, etc.)
- **Command Queue** - Lane-based priority scheduling (Control, Query, Execute, Generate)
- **LLM Support** - Anthropic Claude and OpenAI GPT with streaming

## Architecture

```
┌─────────────────────────────────────────┐
│ Host (SDK / Runtime)                    │
│ - gRPC Client                           │
│ - VM Management                         │
├─────────────────────────────────────────┤
│ Guest (A3S Code Agent)                  │
│ - gRPC Server (:4088)                   │
│ - Agent Loop (LLM + Tools cycle)        │
│ - Session Manager (multi-session)       │
│ - Tool Executor (sandboxed)             │
│ - Permission & HITL System              │
└─────────────────────────────────────────┘
```

## Quick Start

```bash
# Build
cargo build --release

# Run (requires API key)
ANTHROPIC_API_KEY=sk-ant-... \
WORKSPACE=/tmp/workspace \
LISTEN_ADDR=127.0.0.1:4088 \
cargo run
```

## Environment Variables

| Variable | Description | Default |
|----------|-------------|---------|
| `LLM_PROVIDER` | LLM provider (anthropic, openai) | anthropic |
| `ANTHROPIC_API_KEY` | Anthropic API key | - |
| `OPENAI_API_KEY` | OpenAI API key | - |
| `LLM_MODEL` | Model name | claude-sonnet-4-20250514 |
| `WORKSPACE` | Workspace directory path | /a3s/workspace |
| `LISTEN_ADDR` | gRPC listen address | 0.0.0.0:4088 |
| `RUST_LOG` | Log level | info |

## Built-in Tools

| Tool | Description |
|------|-------------|
| `bash` | Execute shell commands with timeout |
| `read` | Read files with line numbers and pagination |
| `write` | Write content to files |
| `edit` | Edit files with string replacement |
| `grep` | Search files with ripgrep |
| `glob` | Find files by pattern |
| `ls` | List directory contents |

## Project Structure

```
code/
├── Cargo.toml
├── justfile
├── build.rs            # Proto compilation
└── src/
    ├── lib.rs          # Library entry point
    ├── main.rs         # Binary entry point
    ├── agent.rs        # Agent loop
    ├── session.rs      # Session management
    ├── llm.rs          # LLM clients
    ├── permissions.rs  # Permission policies
    ├── hitl.rs         # Human-in-the-loop
    ├── hooks/          # Hook system
    ├── queue/          # Command queue
    ├── store/          # Session persistence
    └── tools/          # Tool implementations
```

## Development

### Dependencies

| Dependency | Install | Purpose |
|------------|---------|---------|
| `cargo-llvm-cov` | `cargo install cargo-llvm-cov` | Code coverage (optional) |
| `lcov` | `brew install lcov` / `apt install lcov` | Coverage report formatting (optional) |
| `cargo-watch` | `cargo install cargo-watch` | File watching (optional) |

### Build Commands

```bash
# Build
just build                   # Debug build
just release                 # Release build

# Test (with colored progress display)
just test                    # All tests with pretty output
just test-raw                # Raw cargo output
just test-v                  # Verbose output (--nocapture)

# Test subsets
just test-queue              # Queue + HITL tests
just test-hitl               # HITL module tests
just test-hitl-all           # All HITL-related tests
just test-agent              # Agent tests
just test-session            # Session tests
just test-context            # Context provider tests

# Coverage (requires cargo-llvm-cov + lcov)
just test-cov                # Pretty coverage with progress
just cov                     # Terminal coverage report
just cov-html                # HTML report (opens in browser)
just cov-table               # File-by-file table
just cov-ci                  # Generate lcov.info for CI
just cov-module agent        # Coverage for specific module

# Format & Lint
just fmt                     # Format code
just lint                    # Clippy lint
just ci                      # Full CI checks (fmt + lint + test)

# Server
just serve                   # Start with info logging
just serve-debug             # Start with debug logging

# Utilities
just check                   # Fast compile check
just watch                   # Watch and rebuild
just doc                     # Generate and open docs
just clean                   # Clean build artifacts
```

## gRPC API

The agent exposes `CodeAgentService` with operations for:

- **Session**: Create, Destroy, List, Get, Configure
- **Generation**: Generate, StreamGenerate, GenerateStructured
- **Tools**: ExecuteTool, ListTools, RegisterTool
- **Skills**: LoadSkill, UnloadSkill, ListSkills
- **Context**: GetContextUsage, CompactContext, ClearContext
- **Control**: Cancel, Pause, Resume, HealthCheck
- **HITL**: ConfirmToolExecution, SetConfirmationPolicy
- **Permissions**: SetPermissionPolicy, CheckPermission

## Extending

### Context Provider

```rust
use a3s_box_core::context::{ContextProvider, ContextQuery, ContextResult};

struct MyProvider;

#[async_trait::async_trait]
impl ContextProvider for MyProvider {
    fn name(&self) -> &str { "my-provider" }

    async fn query(&self, query: &ContextQuery) -> anyhow::Result<ContextResult> {
        // Retrieve relevant context
    }

    async fn on_turn_complete(&self, session_id: &str, prompt: &str, response: &str) -> anyhow::Result<()> {
        // Extract memories
    }
}
```

## A3S Ecosystem

A3S Code is the **application layer** of the A3S ecosystem — an AI coding agent that can run standalone or inside a secure sandbox.

```
┌──────────────────────────────────────────────────────────┐
│                    A3S Ecosystem                         │
│                                                          │
│  Infrastructure:  a3s-box     (MicroVM sandbox runtime)  │
│                      │                                   │
│  Application:     a3s-code ◄──── You are here           │
│                    /   \                                 │
│  Utilities:   a3s-lane  a3s-context                     │
│               (queue)   (memory/knowledge)               │
└──────────────────────────────────────────────────────────┘
```

| Project | Package | Relationship |
|---------|---------|--------------|
| **box** | `a3s-box-*` | Runs `code` in hardware-isolated MicroVM (optional) |
| **lane** | `a3s-lane` | Provides priority-based command scheduling |
| **context** | `a3s-context` | Provides hierarchical memory and knowledge retrieval |

**Deployment Options**:
- **Standalone**: Run `a3s-code` directly for development/trusted environments
- **Sandboxed**: Run inside `a3s-box` for production/untrusted code execution

## Roadmap

### Phase 1: Core Stability ✅

- [x] Multi-session management
- [x] Built-in tools (bash, read, write, edit, grep, glob, ls)
- [x] LLM integration (Anthropic, OpenAI)
- [x] Streaming responses
- [x] Session persistence (JSON file storage)
- [x] Permission policies
- [x] Human-in-the-loop confirmations

### Phase 2: Extensibility ✅

- [x] Context provider extension point
- [x] Hook system for lifecycle events
- [x] Dynamic tools (HTTP, binary, script)
- [x] Skill loading system
- [x] Command queue with priority lanes

### Phase 3: Ecosystem Integration 🚧

- [ ] Deep integration with `a3s-context` for persistent memory
- [ ] Use `a3s-lane` for all async task scheduling
- [ ] `a3s-box` deployment support (run as sandboxed guest agent)
- [ ] Metrics and observability (OpenTelemetry)

### Phase 4: Production Readiness 📋

- [ ] Plugin system for custom tool backends
- [ ] WebSocket transport (in addition to gRPC)
- [ ] Redis session store backend
- [ ] PostgreSQL session store backend
- [ ] Distributed session management
- [ ] Rate limiting and quota management
- [ ] Multi-tenant isolation

### Phase 5: Advanced Features 📋

- [ ] Agent-to-agent communication
- [ ] Workflow orchestration
- [ ] Tool dependency graph execution
- [ ] Checkpoint and restore for long-running tasks
- [ ] Fine-grained resource limits (CPU, memory, network)

## License

MIT
