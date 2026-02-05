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
  <a href="#sdk">SDK</a> •
  <a href="#architecture">Architecture</a> •
  <a href="#roadmap">Roadmap</a>
</p>

---

## Overview

**A3S Code** is a high-performance Rust framework for building AI coding agents. It provides a complete gRPC-based service for tool execution, multi-session management, permission control, and extensible integrations — designed to run standalone or inside [A3S Box](https://github.com/A3S-Lab/Box) sandboxes for maximum security.

### Key Capabilities

- **gRPC Service**: Full-featured CodeAgentService API with 28+ RPCs
- **Multi-Provider LLM**: Support for Anthropic Claude, OpenAI, and compatible APIs
- **Configuration**: JSON config files, CLI arguments, and environment variables
- **Official SDKs**: TypeScript and Python clients for easy integration

### Why A3S Code?

- **🔒 Security-First**: Fine-grained permission policies, HITL confirmations, and sandbox-ready architecture
- **⚡ High Performance**: Async Rust core with streaming responses and parallel tool execution
- **🔌 Extensible**: Plugin-based tools, context providers, and hook system for customization
- **🎯 Production-Ready**: Session persistence, operation cancellation, and comprehensive error handling

## Comparison with OpenCode

A3S Code and [OpenCode](https://github.com/anomalyco/opencode) are both open-source AI coding agents. Here's how they compare:

### Feature Comparison

| Feature | A3S Code | OpenCode |
|---------|:--------:|:--------:|
| **Multi-Provider LLM** | ✅ Anthropic, OpenAI, Google, Ollama | ✅ Same |
| **Tool System** | ✅ bash, read, write, edit, grep, glob, ls, web_fetch | ✅ Similar |
| **Session Management** | ✅ Full lifecycle | ✅ Same |
| **Permission System** | ✅ Allow/Deny/Ask rules | ✅ Same |
| **Hooks System** | ✅ Event hooks | ✅ Same |
| **Skills System** | ✅ A3S + Claude Code format | ✅ Same |
| **Memory System** | ✅ 3-tier (Working/Short/Long) | ❓ Unknown |
| **Planning System** | ✅ Goal extraction, plan generation | ✅ plan agent |
| **Reflection System** | ✅ Self-reflection capability | ❓ Unknown |
| **Subagent System** | ✅ Task delegation | ✅ general subagent |
| **HITL** | ✅ Tool confirmation | ✅ Same |
| **Context Compaction** | ✅ Auto-compaction | ✅ Same |
| **gRPC API** | ✅ Full gRPC service | ✅ Has SDK |
| **SDK** | ✅ Python + TypeScript | ✅ TypeScript |
| **Lane Integration** | ✅ External task handling | ❌ No |
| **Todo Tracking** | ✅ Task tracking | ❓ Unknown |
| **LSP Support** | ✅ Built-in | ✅ Built-in |
| **MCP Support** | ✅ Built-in | ✅ Built-in |
| **TUI Interface** | ❌ No | ✅ Built-in |
| **Desktop App** | ❌ No | ✅ Available |
| **IDE Integration** | ❌ No | ✅ Available |
| **Plugin System** | ❌ No | ✅ Built-in |

### A3S Code Advantages

| Advantage | Description |
|-----------|-------------|
| **Rust Implementation** | Higher performance, lower memory footprint |
| **Lane Integration** | Deep integration with A3S Box ecosystem |
| **3-Tier Memory** | Fine-grained memory management (Working/Short-term/Long-term) |
| **Reflection System** | Self-reflection and improvement capability |
| **Claude Code Compatibility** | Full support for Claude Code skill format |
| **LSP + MCP Support** | Both protocols now fully implemented |

### OpenCode Advantages

| Advantage | Description |
|-----------|-------------|
| **TUI Interface** | Rich terminal user interface |
| **Desktop App** | Cross-platform desktop application |
| **IDE Integration** | VS Code, Neovim integration |

### Roadmap Priority

Based on this comparison, A3S Code's development priorities are:

1. **🟡 Medium Priority**: TUI Interface, IDE Integration, Plugin System
2. **🟢 Low Priority**: Desktop App, Snapshot, Share functionality

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

### Build and Run

```bash
# Build
cargo build --release

# Run with config file
./target/release/a3s-code --config ~/.a3s/config.json

# Run with config directory
./target/release/a3s-code --config-dir ~/.a3s

# Run with environment variables
A3S_CONFIG_DIR=~/.a3s ./target/release/a3s-code
```

### CLI Options

```
Usage: a3s-code [OPTIONS]

Options:
  -d, --config-dir <PATH>    Config directory containing config.json [env: A3S_CONFIG_DIR]
  -c, --config <PATH>        Path to config.json file [env: A3S_CONFIG]
  -l, --listen-addr <ADDR>   gRPC listen address [env: LISTEN_ADDR] [default: 0.0.0.0:4088]
  -w, --workspace <PATH>     Workspace directory [env: A3S_WORKSPACE]
  -h, --help                 Print help
  -V, --version              Print version
```

### Config File Format

```json
{
  "defaultProvider": "anthropic",
  "defaultModel": "claude-sonnet-4-20250514",
  "providers": [
    {
      "name": "anthropic",
      "apiKey": "sk-ant-...",
      "baseUrl": "https://api.anthropic.com",
      "models": [
        {
          "id": "claude-sonnet-4-20250514",
          "name": "Claude Sonnet 4",
          "family": "claude-sonnet",
          "toolCall": true
        }
      ]
    }
  ],
  "skill_dirs": ["~/.a3s/skills"],
  "agent_dirs": ["~/.a3s/agents"]
}
```

## SDK

A3S Code provides official SDKs for TypeScript and Python.

### TypeScript SDK

```bash
npm install @a3s-lab/code
```

```typescript
import { A3sClient } from '@a3s-lab/code';

// Create client
const client = new A3sClient({ configDir: '~/.a3s' });
// Or: new A3sClient({ address: 'localhost:4088' })
// Or: new A3sClient({ configPath: '/path/to/config.json' })

async function main() {
  // Check health
  const health = await client.healthCheck();
  console.log('Status:', health.status);

  // Create session
  const { sessionId } = await client.createSession({
    name: 'my-session',
    workspace: '/path/to/project',
  });

  // Generate response (streaming)
  for await (const chunk of client.streamGenerate(sessionId, [
    { role: 'ROLE_USER', content: 'Explain this codebase' }
  ])) {
    if (chunk.content) {
      process.stdout.write(chunk.content);
    }
  }

  // Clean up
  await client.destroySession(sessionId);
  client.close();
}

main();
```

### Python SDK

```bash
pip install a3s-code
```

```python
import asyncio
from a3s_code import A3sClient, Message, MessageRole

async def main():
    # Create client
    async with A3sClient(config_dir="~/.a3s") as client:
        # Or: A3sClient(address="localhost:4088")
        # Or: A3sClient(config_path="/path/to/config.json")

        # Check health
        health = await client.health_check()
        print(f"Status: {health['status']}")

        # Create session
        result = await client.create_session(
            config=SessionConfig(
                name="my-session",
                workspace="/path/to/project",
            )
        )
        session_id = result["session_id"]

        # Generate response (streaming)
        async for chunk in client.stream_generate(session_id, [
            Message(role=MessageRole.USER, content="Explain this codebase")
        ]):
            if chunk.get("content"):
                print(chunk["content"], end="", flush=True)

        # Clean up
        await client.destroy_session(session_id)

asyncio.run(main())
```

### SDK Features

| Feature | TypeScript | Python |
|---------|------------|--------|
| **Lifecycle** | `healthCheck()`, `initialize()`, `shutdown()` | `health_check()`, `initialize()`, `shutdown()` |
| **Sessions** | `createSession()`, `listSessions()`, `destroySession()` | `create_session()`, `list_sessions()`, `destroy_session()` |
| **Generation** | `generate()`, `streamGenerate()` | `generate()`, `stream_generate()` |
| **Skills** | `loadSkill()`, `unloadSkill()`, `listSkills()` | `load_skill()`, `unload_skill()`, `list_skills()` |
| **Context** | `getContextUsage()`, `compactContext()`, `clearContext()` | `get_context_usage()`, `compact_context()`, `clear_context()` |
| **Control** | `cancel()`, `pause()`, `resume()` | `cancel()`, `pause()`, `resume()` |
| **HITL** | `confirmToolExecution()`, `setConfirmationPolicy()` | `confirm_tool_execution()`, `set_confirmation_policy()` |
| **Providers** | `listProviders()`, `addProvider()`, `setDefaultModel()` | `list_providers()`, `add_provider()`, `set_default_model()` |
| **Permissions** | `setPermissionPolicy()`, `checkPermission()` | `set_permission_policy()`, `check_permission()` |
| **Todos** | `getTodos()`, `setTodos()` | `get_todos()`, `set_todos()` |
| **LSP** | `startLspServer()`, `lspHover()`, `lspDefinition()`, `lspReferences()` | `start_lsp_server()`, `lsp_hover()`, `lsp_definition()`, `lsp_references()` |
| **MCP** | `registerMcpServer()`, `connectMcpServer()`, `getMcpTools()` | `register_mcp_server()`, `connect_mcp_server()`, `get_mcp_tools()` |

For complete API documentation, see:
- [TypeScript SDK](https://github.com/A3S-Lab/TypeScript-SDK)
- [Python SDK](https://github.com/A3S-Lab/Python-SDK)

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

## LSP (Language Server Protocol)

A3S Code provides built-in LSP support for code intelligence features. Language servers are automatically started when needed and provide hover information, go-to-definition, find references, and more.

### Supported Language Servers

| Language | Server | Auto-detected Extensions |
|----------|--------|-------------------------|
| Rust | rust-analyzer | `.rs` |
| Go | gopls | `.go` |
| TypeScript/JavaScript | typescript-language-server | `.ts`, `.tsx`, `.js`, `.jsx` |
| Python | pyright | `.py` |
| C/C++ | clangd | `.c`, `.cpp`, `.h`, `.hpp` |

### LSP Tools

| Tool | Description |
|------|-------------|
| `lsp_hover` | Get type information and documentation at a position |
| `lsp_definition` | Jump to the definition of a symbol |
| `lsp_references` | Find all references to a symbol |
| `lsp_symbols` | Search for symbols in the workspace |
| `lsp_diagnostics` | Get errors and warnings for a file |

### SDK Usage

**TypeScript:**

```typescript
// Start language server
await client.startLspServer('rust', 'file:///path/to/project');

// Get hover information
const hover = await client.lspHover('/path/to/file.rs', 10, 5);
if (hover.found) {
  console.log(hover.content);
}

// Go to definition
const definitions = await client.lspDefinition('/path/to/file.rs', 15, 10);
for (const loc of definitions.locations) {
  console.log(`${loc.uri}:${loc.range?.start.line}`);
}

// Find references
const refs = await client.lspReferences('/path/to/file.rs', 20, 8, true);
console.log(`Found ${refs.locations.length} references`);

// Search symbols
const symbols = await client.lspSymbols('main', 10);
for (const sym of symbols.symbols) {
  console.log(`${sym.name} (${sym.kind})`);
}

// Get diagnostics
const diags = await client.lspDiagnostics('/path/to/file.rs');
for (const d of diags.diagnostics) {
  console.log(`[${d.severity}] ${d.message}`);
}
```

**Python:**

```python
# Start language server
await client.start_lsp_server('rust', 'file:///path/to/project')

# Get hover information
hover = await client.lsp_hover('/path/to/file.rs', 10, 5)
if hover['found']:
    print(hover['content'])

# Go to definition
definitions = await client.lsp_definition('/path/to/file.rs', 15, 10)
for loc in definitions:
    print(f"{loc['uri']}:{loc['range']['start']['line']}")

# Find references
refs = await client.lsp_references('/path/to/file.rs', 20, 8, include_declaration=True)
print(f"Found {len(refs)} references")

# Search symbols
symbols = await client.lsp_symbols('main', limit=10)
for sym in symbols:
    print(f"{sym['name']} ({sym['kind']})")

# Get diagnostics
diags = await client.lsp_diagnostics('/path/to/file.rs')
for d in diags:
    print(f"[{d['severity']}] {d['message']}")
```

### gRPC API

| RPC | Description |
|-----|-------------|
| `StartLspServer` | Start a language server for a language |
| `StopLspServer` | Stop a running language server |
| `ListLspServers` | List all running language servers |
| `LspHover` | Get hover information at a position |
| `LspDefinition` | Go to definition |
| `LspReferences` | Find all references |
| `LspSymbols` | Search workspace symbols |
| `LspDiagnostics` | Get diagnostics for a file |

## MCP (Model Context Protocol)

A3S Code supports MCP for extending the agent with external tools. MCP servers can be registered and connected to provide additional capabilities.

### MCP Features

- **Local MCP Servers**: Connect to MCP servers via stdio transport
- **Dynamic Tool Loading**: Tools from MCP servers are automatically registered
- **Server Lifecycle Management**: Start, stop, and monitor MCP servers

### SDK Usage

**TypeScript:**

```typescript
// Register MCP server
await client.registerMcpServer({
  name: 'my-server',
  command: 'npx',
  args: ['-y', '@modelcontextprotocol/server-filesystem'],
  env: { HOME: '/home/user' },
});

// Connect to server
const result = await client.connectMcpServer('my-server');
console.log('Connected tools:', result.toolNames);

// List servers
const servers = await client.listMcpServers();
for (const s of servers) {
  console.log(`${s.name}: ${s.connected ? 'connected' : 'disconnected'}`);
}

// Get tools
const tools = await client.getMcpTools();
for (const t of tools) {
  console.log(`${t.fullName}: ${t.description}`);
}
```

### gRPC API

| RPC | Description |
|-----|-------------|
| `RegisterMcpServer` | Register an MCP server configuration |
| `ConnectMcpServer` | Connect to a registered MCP server |
| `DisconnectMcpServer` | Disconnect from an MCP server |
| `ListMcpServers` | List all registered MCP servers |
| `GetMcpTools` | Get tools from connected MCP servers |

## Configuration

### CLI Arguments

| Argument | Environment Variable | Description | Default |
|----------|---------------------|-------------|---------|
| `--config-dir` | `A3S_CONFIG_DIR` | Config directory containing config.json | - |
| `--config` | `A3S_CONFIG` | Path to config.json file | - |
| `--listen-addr` | `LISTEN_ADDR` | gRPC listen address | `0.0.0.0:4088` |
| `--workspace` | `A3S_WORKSPACE` | Workspace directory | `/a3s/workspace` |

### Environment Variables

| Variable | Description | Default |
|----------|-------------|---------|
| `A3S_SKILL_DIRS` | Colon-separated skill directories | - |
| `A3S_AGENT_DIRS` | Colon-separated agent directories | - |
| `A3S_WATCH_DIRS` | Enable directory watching | `false` |

### Config File Locations

Configuration is loaded in this order (first found wins):
1. `--config` CLI argument
2. `--config-dir` CLI argument (looks for `config.json`)
3. `./config.json` (current directory)
4. `~/.a3s/config.json` (user home)

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

### Phase 5: Advanced Features ✅

- [x] **MCP (Model Context Protocol) Support** — [Design Doc](../../docs/mcp-design.md)
  - Local MCP servers (stdio transport)
  - Dynamic tool loading from MCP servers
  - Server lifecycle management
- [x] **LSP (Language Server Protocol) Integration** — [Design Doc](../../docs/lsp-design.md)
  - Language server lifecycle management
  - Code intelligence tools (hover, definition, references, symbols)
  - Diagnostics (errors, warnings) integration
  - Support for rust-analyzer, gopls, typescript-language-server, pyright, clangd

### Phase 6: Future Enhancements 📋

- [ ] **MCP Advanced Features**
  - Remote MCP servers (HTTP/SSE transport)
  - OAuth authentication for MCP servers
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
