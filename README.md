# A3S Code

<p align="center">
  <strong>Production-Ready AI Coding Agent Framework</strong>
</p>

<p align="center">
  <em>Build, deploy, and scale AI coding agents with enterprise-grade security and extensibility</em>
</p>

<p align="center">
  <a href="#features">Features</a> •
  <a href="#quick-start">Quick Start</a> •
  <a href="#architecture">Architecture</a> •
  <a href="#roadmap">Roadmap</a>
</p>

---

## Overview

**A3S Code** is a high-performance Rust framework for building AI coding agents. It provides a complete solution for tool execution, multi-session management, permission control, and extensible integrations — designed to run standalone or inside [A3S Box](https://github.com/A3S-Lab/Box) sandboxes for maximum security.

### Why A3S Code?

- **🔒 Security-First**: Fine-grained permission policies, HITL confirmations, and sandbox-ready architecture
- **⚡ High Performance**: Async Rust core with streaming responses and parallel tool execution
- **🔌 Extensible**: Plugin-based tools, context providers, and hook system for customization
- **🎯 Production-Ready**: Session persistence, operation cancellation, and comprehensive error handling

## Features

### Core Agent Capabilities

| Feature | Description |
|---------|-------------|
| **Multi-Session Management** | Independent conversation histories with automatic persistence |
| **Streaming Responses** | Real-time event streaming for responsive UI updates |
| **Subagent System** | Delegate specialized tasks to focused child agents with isolated permissions |
| **Context Compaction** | LLM-based intelligent summarization for long conversations |
| **Operation Cancellation** | Abort running operations and pending confirmations |

### Tool System

| Feature | Description |
|---------|-------------|
| **Built-in Tools** | 8 core tools: bash, read, write, edit, grep, glob, ls, web_fetch |
| **Dynamic Tools** | HTTP, binary, and script-based tools via skill system |
| **Skills Framework** | Markdown-based tool definitions with YAML frontmatter |
| **Tool Sandboxing** | Workspace-scoped execution with path validation |

### Security & Control

| Feature | Description |
|---------|-------------|
| **Permission Policies** | Declarative Allow/Deny/Ask rules for tool access |
| **Human-in-the-Loop** | Confirmation system for sensitive operations |
| **Hook System** | Lifecycle event interception (PreToolUse, PostToolUse, etc.) |
| **Session Isolation** | Independent sessions with separate permission contexts |

### Infrastructure

| Feature | Description |
|---------|-------------|
| **Command Queue** | Lane-based priority scheduling (powered by a3s-lane) |
| **Todo/Task Tracking** | Per-session task management for multi-step workflows |
| **LLM Support** | Anthropic Claude and OpenAI-compatible APIs with streaming |
| **Context Providers** | Extension point for external memory/knowledge bases |

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

### Built-in Tools

| Tool | Description |
|------|-------------|
| `bash` | Execute shell commands with timeout |
| `read` | Read files with line numbers and pagination |
| `write` | Write content to files |
| `edit` | Edit files with string replacement |
| `grep` | Search files with ripgrep |
| `glob` | Find files by pattern |
| `ls` | List directory contents |
| `web_fetch` | Fetch web content and convert to markdown/text |

## Skills System

A3S Code uses a unified **skills system** for all tools. Skills are defined in Markdown files with YAML frontmatter and support three backend types:

### Backend Types

#### 1. Binary Tools

Execute external binaries (system-installed or downloaded).

```yaml
- name: jq
  description: Process JSON with jq
  backend:
    type: binary
    path: jq  # System binary in PATH
    args_template: "${filter}"
  parameters:
    type: object
    properties:
      filter:
        type: string
        description: jq filter expression
```

**Features:**
- Use system binaries (jq, curl, git, etc.)
- Download and cache from URLs
- Argument templating with `${arg_name}`
- Environment variables: `TOOL_ARGS` (JSON), `TOOL_ARG_*`

#### 2. HTTP Tools

Make API calls to external services.

```yaml
- name: weather
  description: Get weather data
  backend:
    type: http
    url: https://api.openweathermap.org/data/2.5/weather
    method: GET
    headers:
      Accept: application/json
    body_template: |
      {
        "q": "${city}",
        "appid": "${api_key}"
      }
    timeout_ms: 10000
  parameters:
    type: object
    properties:
      city:
        type: string
      api_key:
        type: string
```

**Features:**
- RESTful APIs (GET, POST, PUT, DELETE, etc.)
- Custom headers (auth, content-type, etc.)
- Body templating with `${arg_name}`
- Configurable timeout

#### 3. Script Tools

Execute inline scripts with various interpreters.

```yaml
- name: analyze-data
  description: Analyze data with Python
  backend:
    type: script
    interpreter: python3
    interpreter_args: []
    script: |
      import json, os
      args = json.loads(os.environ['TOOL_ARGS'])
      # Process args['data']...
      print(json.dumps({"result": 42}))
  parameters:
    type: object
    properties:
      data:
        type: array
```

**Features:**
- Multiple interpreters: bash, python3, node, ruby, perl
- Inline script content
- Environment variables: `TOOL_ARGS` (JSON), `TOOL_ARG_*`
- Interpreter arguments (e.g., `-e` for bash)

### Loading Skills

**Built-in tools** are loaded automatically from `skills/builtin-tools.md`.

**Custom skills** can be loaded via:
- gRPC API: `RegisterSkill(skill_content)`
- File system: Place `.md` files in skills directory

See [examples/skills/](examples/skills/) for complete examples of each backend type.

## Subagent System

A3S Code supports delegating specialized tasks to focused child agents (subagents). Each subagent runs in an isolated child session with restricted permissions.

### Built-in Agents

| Agent | Mode | Description | Permissions |
|-------|------|-------------|-------------|
| `explore` | subagent | Fast codebase exploration | read, grep, glob, ls (read-only) |
| `general` | subagent | Multi-step task execution | all except task |
| `plan` | primary | Read-only planning mode | read, grep, glob, ls |
| `title` | primary (hidden) | Generate session title | none |
| `summary` | primary (hidden) | Summarize session | read |

### Task Tool Usage

```json
{
  "agent": "explore",
  "description": "Find authentication code",
  "prompt": "Search for files related to user authentication...",
  "background": false
}
```

### Features

- **Session-based isolation**: Each subagent runs in a separate child session
- **Permission inheritance**: Child permissions = agent defaults + session overrides
- **Event-driven communication**: Parent subscribes to child events (SubagentStart, SubagentProgress, SubagentEnd)
- **Recursive prevention**: Subagents cannot spawn other subagents by default
- **Parallel execution**: Multiple subagents can run concurrently

## Configuration

### Environment Variables

| Variable | Description | Default |
|----------|-------------|---------|
| `LLM_PROVIDER` | LLM provider (anthropic, openai) | anthropic |
| `ANTHROPIC_API_KEY` | Anthropic API key | - |
| `OPENAI_API_KEY` | OpenAI API key | - |
| `LLM_MODEL` | Model name | claude-sonnet-4-20250514 |
| `WORKSPACE` | Workspace directory path | /a3s/workspace |
| `LISTEN_ADDR` | gRPC listen address | 0.0.0.0:4088 |

## A3S Ecosystem

A3S Code is the **application layer** of the A3S ecosystem.

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

| Project | Package | Purpose |
|---------|---------|---------|
| [box](https://github.com/a3s-lab/box) | `a3s-box-*` | MicroVM sandbox runtime |
| [lane](https://github.com/a3s-lab/lane) | `a3s-lane` | Priority-based command queue |
| [context](https://github.com/a3s-lab/context) | `a3s-context` | Hierarchical context management |

## Development

```bash
# Build
just build              # Debug build
just release            # Release build

# Test
just test               # All tests with progress display
just test-v             # Verbose output

# Code Quality
just fmt                # Format code
just lint               # Clippy lint
just ci                 # Full CI checks

# Publish
just publish            # Publish to crates.io
just publish-dry        # Dry run
```

## Roadmap

### Phase 1: Core Stability ✅

- [x] Multi-session management
- [x] Built-in tools (bash, read, write, edit, grep, glob, ls)
- [x] LLM integration (Anthropic, OpenAI-compatible APIs)
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
- [x] Operation cancellation (abort running operations and HITL confirmations)
- [x] LLM-based context compaction (intelligent summarization for long conversations)
- [x] Builtin tools migration to BinaryTool-based skills (via a3s-tools)
- [x] Multi-provider config format support
- [x] LLM integration tests (completion, streaming, context compaction)

### Phase 3: Ecosystem Integration ✅

- [x] Deep integration with `a3s-lane` for command queue (DLQ, metrics, retry)
- [x] Todo/Task tracking system for multi-step workflows
- [x] Subagent system for delegating specialized tasks to child agents
- [ ] Deep integration with `a3s-context` for persistent memory
- [ ] `a3s-box` deployment support (run as sandboxed guest agent)

### Phase 4: Production Readiness 📋

- [ ] WebSocket transport (in addition to gRPC)
- [ ] Redis/PostgreSQL session store backends
- [ ] Distributed session management
- [ ] Rate limiting and quota management
- [ ] Metrics and observability (OpenTelemetry)

### Phase 5: Advanced Features 📋

- [ ] **MCP (Model Context Protocol) Support** — [Design Doc](../../docs/mcp-design.md)
  - Local MCP servers (stdio transport)
  - Remote MCP servers (HTTP/SSE transport)
  - OAuth authentication for MCP servers
  - Dynamic tool loading from MCP servers
- [ ] **LSP (Language Server Protocol) Integration** — [Design Doc](../../docs/lsp-design.md)
  - Language server lifecycle management
  - Code intelligence tools (hover, definition, references, symbols)
  - Diagnostics (errors, warnings) integration
  - Support for rust-analyzer, gopls, typescript-language-server, pyright
- [ ] **PTY Terminal Sessions**
  - Interactive terminal for commands like `npm init`, `git rebase -i`
  - Multiple concurrent terminal sessions
  - Terminal resize and process management
- [ ] **Session Fork/Revert**
  - Fork sessions from any message point
  - Revert to previous conversation states
  - Automatic snapshots before destructive operations
- [ ] **Apply Patch Tool**
  - Apply unified diff patches
  - Multi-file batch edits
- [ ] **Web Search**
  - Web search integration
  - Search result summarization

## License

MIT License - see [LICENSE](LICENSE) for details.

---

<p align="center">
  Built by <a href="https://github.com/a3s-lab">A3S Lab</a>
</p>
