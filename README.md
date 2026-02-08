# A3S Code

<p align="center">
  <strong>Production-Ready AI Coding Agent Framework</strong>
</p>

<p align="center">
  <em>Application layer — high-performance gRPC service for AI-powered code generation, tool execution, and multi-session management</em>
</p>

<p align="center">
  <a href="#features">Features</a> •
  <a href="#quick-start">Quick Start</a> •
  <a href="#architecture">Architecture</a> •
  <a href="#sdk">SDK</a> •
  <a href="#api-reference">API Reference</a> •
  <a href="#development">Development</a>
</p>

---

## Overview

**A3S Code** is a high-performance Rust framework for building AI coding agents. It provides a complete gRPC-based service with 77 RPCs for tool execution, multi-session management, and extensible integrations.

### Basic Usage

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

## Features

- **Multi-Session Management**: Run multiple independent AI conversations with isolated context and permissions
- **9 Built-in Tools**: bash, read, write, edit, grep, glob, ls, web_fetch, web_search
- **Permission System**: Allow/Deny/Ask rules for fine-grained tool access control
- **Human-in-the-Loop (HITL)**: Require user confirmation before sensitive operations
- **Skills System**: Extend the agent with custom tools defined in Markdown files
- **Subagent System**: Delegate specialized tasks to focused child agents (explore, general, plan)
- **LSP Integration**: Code intelligence via Language Server Protocol (hover, definition, references, symbols, diagnostics)
- **MCP Support**: Extend with external tools via Model Context Protocol
- **Cron Scheduling**: Schedule recurring tasks with cron expressions or natural language
- **Context Compaction**: Automatically summarize long conversations to stay within context limits
- **Streaming Responses**: Real-time event streaming for responsive UI updates
- **Planning & Goal Tracking**: Create execution plans and track goal achievement
- **Memory System**: Episodic, semantic, procedural, and working memory for persistent knowledge
- **Provider Configuration**: Multi-provider LLM support with per-model API key and base URL overrides
- **Todo Tracking**: Task management within sessions
- **OpenAI Compatibility**: OpenAI-compatible message format and chat completion API

## Quality Metrics

### Test Coverage

**569 comprehensive unit tests** across all modules:

```bash
# Run all tests with progress display
just test

# Run tests for specific module
cd src && cargo test -p a3s-code --lib -- test_name
```

## Architecture

### System Architecture

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
│  ┌─────────────┬─────────────┬─────────────┬──────────────┐ │
│  │    Cron     │   Memory    │  Planning   │   Context    │ │
│  │  Scheduler  │   System    │   & Goals   │  Compaction  │ │
│  └─────────────┴─────────────┴─────────────┴──────────────┘ │
└─────────────────────────────────────────────────────────────┘
```

### Built-in Tools

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
| `web_search` | Search the web | Query multiple search engines |

### Subagent Types

| Agent | Permissions | Use Case |
|-------|-------------|----------|
| `explore` | read, grep, glob, ls | Find code, understand structure |
| `general` | all except task | Complex multi-step tasks |
| `plan` | read, grep, glob, ls | Design implementation approach |

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

### Permission System

```typescript
// Set permission policy for a session
await client.setPermissionPolicy(sessionId, {
  enabled: true,
  deny: [{ rule: 'Bash(rm -rf:*)' }],
  allow: [{ rule: 'Read(src/**)' }, { rule: 'Grep(src/**)' }],
  ask: [{ rule: 'Bash(*)' }, { rule: 'Write(*)' }],
  defaultDecision: 'PERMISSION_DECISION_ASK',
});
```

### Human-in-the-Loop (HITL)

```typescript
// Set confirmation policy
await client.setConfirmationPolicy(sessionId, {
  enabled: true,
  autoApproveTools: ['read', 'grep', 'glob', 'ls'],
  requireConfirmTools: ['bash', 'write', 'edit'],
  defaultTimeoutMs: 30000,
  timeoutAction: 'TIMEOUT_ACTION_REJECT',
});

// Confirm or reject tool execution
await client.confirmToolExecution(sessionId, toolId, true, 'Approved');
```

### Skills System

```yaml
# ~/.a3s/skills/deploy.md
---
name: deploy
description: Deploy to production
allowed_tools: Bash(kubectl:*)
---
Run kubectl apply to deploy the application.
```

```typescript
await client.loadSkill(sessionId, 'deploy');
const skills = await client.listSkills();
```

### LSP Integration

```typescript
await client.startLspServer('rust', 'file:///path/to/project');

const hover = await client.lspHover('/path/to/file.rs', 10, 5);
const defs = await client.lspDefinition('/path/to/file.rs', 15, 10);
const refs = await client.lspReferences('/path/to/file.rs', 20, 8);
const symbols = await client.lspSymbols('main');
const diags = await client.lspDiagnostics('/path/to/file.rs');
```

Supported: rust-analyzer, gopls, typescript-language-server, pyright, clangd

### MCP Support

```typescript
await client.registerMcpServer({
  name: 'filesystem',
  transport: { stdio: { command: 'npx', args: ['-y', '@modelcontextprotocol/server-filesystem'] } },
  enabled: true,
  env: {},
});

await client.connectMcpServer('filesystem');
const tools = await client.getMcpTools();
```

### Cron Scheduling

```typescript
const result = await client.createCronJob('backup', 'every day at 2am', 'backup.sh');
const jobs = await client.listCronJobs();
await client.runCronJob(result.job.id);
const history = await client.getCronHistory(result.job.id);
await client.pauseCronJob(result.job.id);
await client.resumeCronJob(result.job.id);

// Parse natural language to cron expression
const parsed = await client.parseCronSchedule('every 5 minutes');
// { cronExpression: '*/5 * * * *', description: 'every 5 minutes' }
```

Supported natural language formats:
- English: `every 5 minutes`, `daily at 2am`, `every monday at 9:30`
- Chinese: `每5分钟`, `每天凌晨2点`, `每周一上午9点30分`

### Memory System

```typescript
// Store a memory
await client.storeMemory(sessionId, {
  content: 'User prefers TypeScript over JavaScript',
  importance: 0.8,
  tags: ['preference', 'language'],
  memoryType: 'MEMORY_TYPE_SEMANTIC',
});

// Search memories
const results = await client.searchMemories(sessionId, 'TypeScript', ['preference'], 10);

// Get memory stats
const stats = await client.getMemoryStats(sessionId);
```

### Planning & Goal Tracking

```typescript
const plan = await client.createPlan(sessionId, 'Refactor auth module');
const goal = await client.extractGoal(sessionId, 'Improve test coverage to 90%');
const check = await client.checkGoalAchievement(sessionId, goal, 'Current coverage: 85%');
```

## API Reference

### Lifecycle (4 RPCs)

| Method | Description |
|--------|-------------|
| `healthCheck()` | Check agent health status |
| `getCapabilities()` | Get agent capabilities, tools, and models |
| `initialize(workspace, env)` | Initialize agent with workspace |
| `shutdown()` | Graceful shutdown |

### Session Management (6 RPCs)

| Method | Description |
|--------|-------------|
| `createSession(config, sessionId, initialContext)` | Create a new session |
| `destroySession(sessionId)` | Destroy a session |
| `listSessions()` | List all sessions |
| `getSession(sessionId)` | Get session details |
| `configureSession(sessionId, config)` | Update session configuration |
| `getMessages(sessionId, limit, offset)` | Get conversation history |

### Code Generation (4 RPCs)

| Method | Description |
|--------|-------------|
| `generate(sessionId, messages)` | Generate response (unary) |
| `streamGenerate(sessionId, messages)` | Generate response (streaming) |
| `generateStructured(sessionId, messages, schema)` | Generate structured output (unary) |
| `streamGenerateStructured(sessionId, messages, schema)` | Generate structured output (streaming) |

### Skill Management (4 RPCs)

| Method | Description |
|--------|-------------|
| `loadSkill(sessionId, skillName, skillContent)` | Load a skill into session |
| `unloadSkill(sessionId, skillName)` | Unload a skill |
| `listSkills(sessionId)` | List available skills |
| `getClaudeCodeSkills(name)` | Get Claude Code compatible skills |

### Context Management (3 RPCs)

| Method | Description |
|--------|-------------|
| `getContextUsage(sessionId)` | Get token usage |
| `compactContext(sessionId)` | Compact conversation context |
| `clearContext(sessionId)` | Clear all context |

### Event Streaming (1 RPC)

| Method | Description |
|--------|-------------|
| `subscribeEvents(sessionId, eventTypes)` | Subscribe to agent events |

### Control Operations (3 RPCs)

| Method | Description |
|--------|-------------|
| `cancel(sessionId, operationId)` | Cancel an operation |
| `pause(sessionId)` | Pause a session |
| `resume(sessionId)` | Resume a session |

### Human-in-the-Loop (3 RPCs)

| Method | Description |
|--------|-------------|
| `confirmToolExecution(sessionId, toolId, approved, reason)` | Confirm or reject tool execution |
| `setConfirmationPolicy(sessionId, policy)` | Set HITL confirmation policy |
| `getConfirmationPolicy(sessionId)` | Get current confirmation policy |

### External Task Handling (4 RPCs)

| Method | Description |
|--------|-------------|
| `setLaneHandler(sessionId, lane, config)` | Set lane handler mode |
| `getLaneHandler(sessionId, lane)` | Get lane handler config |
| `completeExternalTask(sessionId, taskId, success, result, error)` | Complete an external task |
| `listPendingExternalTasks(sessionId)` | List pending external tasks |

### Permission System (4 RPCs)

| Method | Description |
|--------|-------------|
| `setPermissionPolicy(sessionId, policy)` | Set permission policy |
| `getPermissionPolicy(sessionId)` | Get permission policy |
| `checkPermission(sessionId, toolName, args)` | Check tool permission |
| `addPermissionRule(sessionId, ruleType, rule)` | Add a permission rule |

### Todo Tracking (2 RPCs)

| Method | Description |
|--------|-------------|
| `getTodos(sessionId)` | Get todos for session |
| `setTodos(sessionId, todos)` | Set todos for session |

### Provider Configuration (7 RPCs)

| Method | Description |
|--------|-------------|
| `listProviders()` | List all providers |
| `getProvider(name)` | Get provider details |
| `addProvider(provider)` | Add a new provider |
| `updateProvider(provider)` | Update a provider |
| `removeProvider(name)` | Remove a provider |
| `setDefaultModel(provider, model)` | Set default model |
| `getDefaultModel()` | Get default model |

### Planning & Goal Tracking (4 RPCs)

| Method | Description |
|--------|-------------|
| `createPlan(sessionId, prompt, context)` | Create an execution plan |
| `getPlan(sessionId, planId)` | Get an existing plan |
| `extractGoal(sessionId, prompt)` | Extract goal from prompt |
| `checkGoalAchievement(sessionId, goal, currentState)` | Check goal progress |

### Memory System (5 RPCs)

| Method | Description |
|--------|-------------|
| `storeMemory(sessionId, memory)` | Store a memory item |
| `retrieveMemory(sessionId, memoryId)` | Retrieve memory by ID |
| `searchMemories(sessionId, query, tags, limit)` | Search memories |
| `getMemoryStats(sessionId)` | Get memory statistics |
| `clearMemories(sessionId, clearLongTerm, clearShortTerm, clearWorking)` | Clear memories |

### MCP - Model Context Protocol (5 RPCs)

| Method | Description |
|--------|-------------|
| `registerMcpServer(config)` | Register an MCP server |
| `connectMcpServer(name)` | Connect to MCP server |
| `disconnectMcpServer(name)` | Disconnect from MCP server |
| `listMcpServers()` | List all MCP servers |
| `getMcpTools(serverName)` | Get available MCP tools |

### LSP - Language Server Protocol (8 RPCs)

| Method | Description |
|--------|-------------|
| `startLspServer(language, rootUri)` | Start language server |
| `stopLspServer(language)` | Stop language server |
| `listLspServers()` | List running servers |
| `lspHover(filePath, line, column)` | Get hover information |
| `lspDefinition(filePath, line, column)` | Go to definition |
| `lspReferences(filePath, line, column)` | Find all references |
| `lspSymbols(query, limit)` | Search workspace symbols |
| `lspDiagnostics(filePath)` | Get diagnostics |

### Cron - Scheduled Tasks (10 RPCs)

| Method | Description |
|--------|-------------|
| `listCronJobs()` | List all cron jobs |
| `createCronJob(name, schedule, command, timeoutMs)` | Create a cron job |
| `getCronJob(id, name)` | Get cron job by ID or name |
| `updateCronJob(id, schedule, command, timeoutMs)` | Update a cron job |
| `pauseCronJob(id)` | Pause a cron job |
| `resumeCronJob(id)` | Resume a cron job |
| `deleteCronJob(id)` | Delete a cron job |
| `getCronHistory(id, limit)` | Get execution history |
| `runCronJob(id)` | Manually trigger a job |
| `parseCronSchedule(input)` | Parse natural language to cron |

## Development

### Dependencies

| Dependency | Install | Purpose |
|------------|---------|---------|
| `just` | `cargo install just` | Task runner |
| `cargo-llvm-cov` | `cargo install cargo-llvm-cov` | Code coverage (optional) |

### Build Commands

```bash
# Build
just build                   # Debug build
just release                 # Release build

# Test (with colored progress display)
just test                    # All tests with pretty output
just test-v                  # Verbose output

# Format & Lint
just fmt                     # Format code
just lint                    # Clippy lint
just ci                      # Full CI checks (fmt + lint + test)

# Utilities
just check                   # Fast compile check
just doc                     # Generate and open docs
just clean                   # Clean build artifacts
```

### Project Structure

```
code/
├── Cargo.toml
├── justfile
├── README.md
├── CLAUDE.md
├── proto/
│   └── code_agent.proto     # gRPC service definition (77 RPCs)
└── src/
    ├── lib.rs               # Library entry point
    ├── service.rs           # gRPC service implementation
    ├── session/             # Session management
    ├── tools/               # Built-in tools (bash, read, write, edit, grep, glob, ls, web)
    ├── skills/              # Skills system (Markdown-based custom tools)
    ├── subagent/            # Subagent system (explore, general, plan)
    ├── provider/            # LLM provider management
    ├── permission/          # Permission system (allow/deny/ask rules)
    ├── hitl/                # Human-in-the-loop confirmation
    ├── lsp/                 # Language Server Protocol integration
    ├── mcp/                 # Model Context Protocol support
    ├── cron/                # Cron scheduling
    ├── memory/              # Memory system
    ├── planning/            # Planning & goal tracking
    └── context/             # Context compaction
```

## A3S Ecosystem

A3S Code is the **application layer** of the A3S ecosystem — the AI coding agent that runs inside A3S Box.

```
┌──────────────────────────────────────────────────────────┐
│                    A3S Ecosystem                         │
│                                                          │
│  Infrastructure:  a3s-box     (MicroVM sandbox runtime)  │
│                      │                                   │
│  Application:     a3s-code    (AI coding agent)          │
│                    /   \        ▲                        │
│  Utilities:   a3s-lane  a3s-context  You are here       │
│              (priority   (memory/                        │
│               queue)     knowledge)                      │
└──────────────────────────────────────────────────────────┘
```

| Project | Package | Relationship |
|---------|---------|--------------|
| **box** | `a3s-box-*` | Sandbox runtime that hosts `a3s-code` |
| **code** | `a3s-code` | AI coding agent (this project) |
| **lane** | `a3s-lane` | Priority queue used by `a3s-code` for command scheduling |
| **context** | `a3s-context` | Context management used by `a3s-code` for memory |

## Roadmap

### Phase 1: Core ✅ (Complete)

- [x] Multi-session management with isolated context
- [x] 9 built-in tools (bash, read, write, edit, grep, glob, ls, web_fetch, web_search)
- [x] LLM provider integration with streaming
- [x] Permission system (allow/deny/ask rules)
- [x] Human-in-the-loop (HITL) confirmation
- [x] Event streaming for real-time updates
- [x] Context compaction for long conversations
- [x] 569 comprehensive tests

### Phase 2: Extensibility ✅ (Complete)

- [x] Skills system (Markdown-based custom tools with binary, HTTP, script backends)
- [x] Subagent system (explore, general, plan agents with isolated permissions)
- [x] Lane integration for priority-based command scheduling
- [x] Todo/task tracking within sessions
- [x] Provider configuration (multi-provider, per-model overrides)
- [x] OpenAI-compatible message format

### Phase 3: Ecosystem ✅ (Complete)

- [x] LSP integration (hover, definition, references, symbols, diagnostics)
- [x] MCP support (register, connect, disconnect, tool discovery)
- [x] Cron scheduling (natural language + cron expressions, execution history)
- [x] Planning & goal tracking (execution plans, goal extraction, achievement checking)
- [x] Memory system (episodic, semantic, procedural, working memory)
- [x] Web search with multiple engine support
- [x] Claude Code skills compatibility

### Phase 4: SDK & API ✅ (Complete)

- [x] TypeScript SDK with full 77 RPC coverage (`@a3s-lab/code`)
- [x] Python SDK with full 77 RPC coverage (`a3s-code` on PyPI)
- [x] OpenAI-compatible chat completion API
- [x] Comprehensive type exports and documentation
- [x] Proto file synchronization across all SDKs

### Phase 5: Observability 📋

End-to-end distributed tracing across the agent lifecycle:

- [ ] **OpenTelemetry Spans**: Instrument agent loop with structured spans
  - `a3s.agent.invoke` → `a3s.llm.completion` → `a3s.tool.execute`
  - Span attributes: session_id, model, tool_name, token_count
- [ ] **LLM Cost Tracking**: Per-call recording of model / input_tokens / output_tokens / cost
  - Aggregate by: agent, session, day, model
  - Export to Prometheus / OTLP for Cost Dashboard
- [ ] **Tool Execution Metrics**: Duration, success/failure rate, retry count per tool
- [ ] **Multi-Agent Trace Propagation**: Trace context forwarded across subagent calls
- [ ] **SigNoz Dashboard Template**: Pre-built dashboard for A3S Code metrics

### Phase 6: Production 📋

- [ ] WebSocket transport (in addition to gRPC)
- [ ] Redis/PostgreSQL session persistence
- [ ] Rate limiting per session/user
- [ ] Prometheus metrics endpoint
- [ ] Health check endpoint for load balancers

### Phase 7: SafeClaw Security 📋

Privacy-focused features for SafeClaw's TEE-based security model:

- [ ] **Output Sanitizer**: Scan and redact sensitive data in AI responses
- [ ] **Taint Tracking**: Mark sensitive data at input, track through transformations
- [ ] **Tool Call Interceptor**: Block tool calls that may leak sensitive data
- [ ] **Session Isolation**: Strict memory isolation with secure wipe on session end
- [ ] **Prompt Injection Defense**: Detect and block injection attacks

### Phase 8: Distributed TEE 📋

Support for SafeClaw's split-process-merge security model:

- [ ] **Coordinator Role**: Task decomposition and result aggregation in TEE
- [ ] **Secure Worker Role**: Partial sensitive data access in TEE
- [ ] **General Worker Role**: Sanitized data only in REE
- [ ] **Validator Role**: Independent output verification in TEE
- [ ] **Inter-Agent Communication**: Secure channels with data minimization

## License

MIT

---

<p align="center">
  Built by <a href="https://github.com/a3s-lab">A3S Lab</a>
</p>
