# A3S Code

<p align="center">
  <strong>Production-Ready AI Coding Agent Framework</strong>
</p>

<p align="center">
  <a href="#core-features">Features</a> •
  <a href="#quick-start">Quick Start</a> •
  <a href="#sdk">SDK</a> •
  <a href="#roadmap">Roadmap</a>
</p>

---

## Overview

**A3S Code** is a high-performance Rust framework for building AI coding agents. It provides a complete gRPC-based service with 28+ RPCs for tool execution, multi-session management, and extensible integrations.

```
┌─────────────────────────────────────────────────────────────┐
│  Your Application (SDK Client)                              │
│  - TypeScript / Python                                      │
├─────────────────────────────────────────────────────────────┤
│  A3S Code (gRPC Server :4088)                               │
│  ┌─────────────┬─────────────┬─────────────┬──────────────┐ │
│  │   Session   │    Tool     │ Permission  │     LLM      │ │
│  │   Manager   │  Executor   │   System    │   Provider   │ │
│  └─────────────┴─────────────┴─────────────┴──────────────┘ │
│  ┌─────────────┬─────────────┬─────────────┬──────────────┐ │
│  │   Skills    │  Subagent   │     LSP     │     MCP      │ │
│  │   System    │   System    │   Support   │   Support    │ │
│  └─────────────┴─────────────┴─────────────┴──────────────┘ │
└─────────────────────────────────────────────────────────────┘
```

## Core Features

### 1. Multi-Session Management

**Purpose**: Run multiple independent AI conversations simultaneously, each with its own context and permissions.

```typescript
// Create isolated sessions for different tasks
const session1 = await client.createSession({ name: 'refactor', workspace: '/project' });
const session2 = await client.createSession({ name: 'debug', workspace: '/project' });

// Each session maintains independent conversation history
await client.generate(session1.sessionId, [{ role: 'user', content: 'Refactor auth module' }]);
await client.generate(session2.sessionId, [{ role: 'user', content: 'Debug login issue' }]);
```

### 2. Built-in Tools

**Purpose**: 8 core tools for file operations, code search, and command execution.

| Tool | Purpose | Example |
|------|---------|---------|
| `bash` | Execute shell commands | `git status`, `npm install` |
| `read` | Read files with line numbers | View source code |
| `write` | Create/overwrite files | Create new files |
| `edit` | String replacement editing | Modify existing code |
| `grep` | Search file contents (ripgrep) | Find function definitions |
| `glob` | Find files by pattern | `**/*.ts`, `src/**/*.rs` |
| `ls` | List directory contents | Explore project structure |
| `web_fetch` | Fetch web content | Download documentation |

### 3. Permission System

**Purpose**: Control which tools the agent can use and when to ask for confirmation.

```typescript
// Set permission policy for a session
await client.setPermissionPolicy(sessionId, {
  rules: [
    { tool: 'bash', action: 'ask' },      // Always ask before running commands
    { tool: 'read', action: 'allow' },    // Allow reading files
    { tool: 'write', action: 'deny' },    // Deny file writes
  ]
});
```

### 4. Human-in-the-Loop (HITL)

**Purpose**: Require user confirmation before executing sensitive operations.

```typescript
// Listen for confirmation requests
for await (const event of client.streamGenerate(sessionId, messages)) {
  if (event.confirmationRequest) {
    const { toolName, args, confirmationId } = event.confirmationRequest;
    console.log(`Agent wants to run: ${toolName}(${JSON.stringify(args)})`);

    // User approves or denies
    await client.confirmToolExecution(sessionId, confirmationId, true);
  }
}
```

### 5. Skills System

**Purpose**: Extend the agent with custom tools defined in Markdown files.

```yaml
# ~/.a3s/skills/my-tools.md
---
name: my-skill
tools:
  - name: deploy
    description: Deploy to production
    backend:
      type: script
      interpreter: bash
      script: |
        kubectl apply -f deployment.yaml
    parameters:
      type: object
      properties:
        env:
          type: string
          enum: [staging, production]
---
```

Three backend types:
- **Binary**: Execute system commands (`jq`, `curl`, `git`)
- **HTTP**: Call REST APIs
- **Script**: Run inline scripts (bash, python, node)

### 6. Subagent System

**Purpose**: Delegate specialized tasks to focused child agents with isolated permissions.

```typescript
// The agent can spawn subagents via the 'task' tool
// Built-in agents:
// - explore: Fast codebase exploration (read-only)
// - general: Multi-step task execution
// - plan: Read-only planning mode
```

| Agent | Permissions | Use Case |
|-------|-------------|----------|
| `explore` | read, grep, glob, ls | Find code, understand structure |
| `general` | all except task | Complex multi-step tasks |
| `plan` | read, grep, glob, ls | Design implementation approach |

### 7. LSP Integration

**Purpose**: Code intelligence features via Language Server Protocol.

```typescript
// Start language server
await client.startLspServer('rust', 'file:///path/to/project');

// Get hover information (type info, docs)
const hover = await client.lspHover('/path/to/file.rs', 10, 5);

// Go to definition
const defs = await client.lspDefinition('/path/to/file.rs', 15, 10);

// Find all references
const refs = await client.lspReferences('/path/to/file.rs', 20, 8);

// Search symbols
const symbols = await client.lspSymbols('main');

// Get diagnostics (errors, warnings)
const diags = await client.lspDiagnostics('/path/to/file.rs');
```

Supported: rust-analyzer, gopls, typescript-language-server, pyright, clangd

### 8. MCP Support

**Purpose**: Extend the agent with external tools via Model Context Protocol.

```typescript
// Register MCP server
await client.registerMcpServer({
  name: 'filesystem',
  command: 'npx',
  args: ['-y', '@modelcontextprotocol/server-filesystem'],
});

// Connect and load tools
await client.connectMcpServer('filesystem');

// Tools are now available to the agent
const tools = await client.getMcpTools();
```

### 9. Context Compaction

**Purpose**: Automatically summarize long conversations to stay within context limits.

```typescript
// Check context usage
const usage = await client.getContextUsage(sessionId);
console.log(`${usage.usedTokens}/${usage.maxTokens} tokens`);

// Manually trigger compaction
await client.compactContext(sessionId);
```

### 10. Streaming Responses

**Purpose**: Real-time event streaming for responsive UI updates.

```typescript
for await (const event of client.streamGenerate(sessionId, messages)) {
  if (event.content) process.stdout.write(event.content);
  if (event.toolCall) console.log(`Calling: ${event.toolCall.name}`);
  if (event.toolResult) console.log(`Result: ${event.toolResult.output}`);
}
```

## Quick Start

### Installation

```bash
# Build from source
cargo build --release

# Run server
./target/release/a3s-code --config ~/.a3s/config.json
```

### Configuration

Create `~/.a3s/config.json`:

```json
{
  "defaultProvider": "anthropic",
  "defaultModel": "claude-sonnet-4-20250514",
  "providers": [{
    "name": "anthropic",
    "apiKey": "sk-ant-...",
    "models": [{
      "id": "claude-sonnet-4-20250514",
      "name": "Claude Sonnet 4",
      "toolCall": true
    }]
  }]
}
```

## SDK

### TypeScript

```bash
npm install @a3s-lab/code
```

```typescript
import { A3sClient } from '@a3s-lab/code';

const client = new A3sClient({ address: 'localhost:4088' });

// Create session and generate response
const { sessionId } = await client.createSession({ workspace: '/project' });

for await (const event of client.streamGenerate(sessionId, [
  { role: 'user', content: 'Explain this codebase' }
])) {
  if (event.content) process.stdout.write(event.content);
}

await client.destroySession(sessionId);
```

### Python

```bash
pip install a3s-code
```

```python
from a3s_code import A3sClient

async with A3sClient(address="localhost:4088") as client:
    result = await client.create_session(workspace="/project")
    session_id = result["session_id"]

    async for event in client.stream_generate(session_id, [
        {"role": "user", "content": "Explain this codebase"}
    ]):
        if event.get("content"):
            print(event["content"], end="", flush=True)

    await client.destroy_session(session_id)
```

### SDK API Reference

| Category | Methods |
|----------|---------|
| **Lifecycle** | `healthCheck`, `initialize`, `shutdown` |
| **Sessions** | `createSession`, `listSessions`, `destroySession` |
| **Generation** | `generate`, `streamGenerate` |
| **Skills** | `loadSkill`, `unloadSkill`, `listSkills` |
| **Context** | `getContextUsage`, `compactContext`, `clearContext` |
| **Control** | `cancel`, `pause`, `resume` |
| **HITL** | `confirmToolExecution`, `setConfirmationPolicy` |
| **Permissions** | `setPermissionPolicy`, `checkPermission` |
| **LSP** | `startLspServer`, `lspHover`, `lspDefinition`, `lspReferences`, `lspSymbols` |
| **MCP** | `registerMcpServer`, `connectMcpServer`, `getMcpTools` |

## A3S Ecosystem

A3S Code is part of the A3S ecosystem:

| Project | Purpose |
|---------|---------|
| [a3s-box](https://github.com/a3s-lab/box) | MicroVM sandbox runtime |
| **a3s-code** | AI coding agent (this project) |
| [a3s-lane](https://github.com/a3s-lab/lane) | Priority-based command queue |
| [a3s-context](https://github.com/a3s-lab/context) | Hierarchical context management |

## Development

```bash
just build      # Debug build
just release    # Release build
just test       # Run all tests
just fmt        # Format code
just lint       # Clippy lint
```

## Roadmap

| Phase | Status | Features |
|-------|--------|----------|
| **Core** | ✅ | Sessions, Tools, LLM, Streaming, Permissions, HITL |
| **Extensibility** | ✅ | Hooks, Skills, Context Compaction, Lane Integration |
| **Ecosystem** | ✅ | Subagents, Todo Tracking, LSP, MCP |
| **Production** | 📋 | WebSocket, Redis/PostgreSQL, Rate Limiting, Metrics |
| **Future** | 📋 | PTY Terminal, Session Fork, Web Search |

## License

MIT License - see [LICENSE](LICENSE) for details.

---

<p align="center">
  Built by <a href="https://github.com/a3s-lab">A3S Lab</a>
</p>
