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

**A3S Code** is a high-performance Rust framework for building AI coding agents. It provides a complete gRPC-based service with 85 RPCs for tool execution, multi-session management, and extensible integrations.

### Basic Usage

**TypeScript**

```typescript
import { A3sClient, createProvider } from '@a3s-lab/code';

const client = new A3sClient({ address: 'localhost:4088' });
const anthropic = createProvider({ name: 'anthropic', apiKey: 'sk-ant-...' });

// Create session (high-level API with auto-dispose)
await using session = await client.createSession({
  model: anthropic('claude-sonnet-4-20250514'),
  workspace: '/project',
  system: 'You are a senior engineer.',
});

// Server-side agentic loop with streaming
for await (const event of session.stream('Refactor the auth module')) {
  if (event.textDelta) process.stdout.write(event.textDelta);
  if (event.toolStart) console.log(`\nTool: ${event.toolStart.name}`);
}
```

**Python**

```python
from a3s_code import A3sClient, create_provider

anthropic = create_provider(name="anthropic", api_key="sk-ant-...")

async with A3sClient(address="localhost:4088") as client:
    # Create session (high-level API with auto-cleanup)
    async with await client.session(
        model=anthropic("claude-sonnet-4-20250514"),
        workspace="/project",
        system="You are a senior engineer.",
    ) as session:
        # Server-side agentic loop with streaming
        async for event in session.stream("Refactor the auth module"):
            if event.type == "text":
                print(event.content, end="", flush=True)
```

## Features

- **Multi-Session Management**: Run multiple independent AI conversations with isolated context and permissions
- **11 Built-in Tools**: bash, read, write, edit, patch, grep, glob, ls, web_fetch, web_search, cron
- **Permission System**: Allow/Deny/Ask rules for fine-grained tool access control
- **Human-in-the-Loop (HITL)**: Require user confirmation before sensitive operations
- **Skills System**: Extend the agent with prompt-injection skills defined in Markdown files (compatible with Claude Code Skills format)
- **Subagent System**: Delegate specialized tasks to focused child agents (explore, general, plan)
- **Server-Side Agentic Loop**: Full agentic loop execution on the server with streaming events
- **Server-Side Delegation**: Delegate tasks to subagents via gRPC with streaming support
- **LSP Integration**: Code intelligence via Language Server Protocol (hover, definition, references, symbols, diagnostics)
- **MCP Support**: Extend with external tools via Model Context Protocol
- **Cron Scheduling**: Schedule recurring tasks with cron expressions or natural language
- **Context Compaction**: Automatically summarize long conversations to stay within context limits, with auto-compact triggered at configurable usage threshold (default 80%)
- **Streaming Responses**: Real-time event streaming for responsive UI updates
- **Planning & Goal Tracking**: Create execution plans and track goal achievement
- **Memory System**: Episodic, semantic, procedural, and working memory for persistent knowledge
- **Provider Configuration**: Multi-provider LLM support with per-model API key and base URL overrides
- **Thinking Model Compatibility**: Full support for reasoning models (kimi-k2.5, DeepSeek-R1) with reasoning_content preservation
- **API Retry with Backoff**: Automatic retry with exponential backoff and jitter for transient LLM API errors (429, 500, 502, 503, 529), with Retry-After header support
- **File Version History**: Automatic file snapshots before write/edit/patch operations with diff generation and version restore
- **Per-Session Token Cost Tracking**: Automatic cost calculation per session using model-specific pricing (input/output/cache tokens), with cost summary API
- **Session Export to Markdown**: Export session conversations to readable Markdown with metadata, tool calls, and usage statistics
- **Session Fork**: Fork existing sessions with full history, configuration, and state copied to a new independent session
- **Auto Title Generation**: LLM-powered automatic session title generation from conversation content
- **Todo Tracking**: Task management within sessions
- **OpenAI Compatibility**: OpenAI-compatible message format and chat completion API
- **Tool Execution Metrics**: Per-session tool call tracking with duration, success/failure rate, and per-tool aggregated statistics
- **Queue Statistics**: Monitor lane queue depths and task states per session
- **Batch Skill Loading**: Load all skills from a directory in one call
- **JSON Structured Logging**: Optional JSON log output via `--json-log` flag for log aggregation
- **Enhanced Health Check**: Subsystem diagnostics reporting version, uptime, active session count, and store health status
- **Pluggable Session Persistence**: `SessionStore` trait with `Custom` backend support — inject external stores (PostgreSQL, etc.) via `start_server_with_store()`

## Quality Metrics

### Test Coverage

**1716 unit tests** (0 failures, 3 ignored):

Run tests:
```bash
just test
```

Run coverage report:
```bash
cargo llvm-cov --lib --summary-only
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
| `patch` | Apply unified diff patches | Complex multi-line edits |
| `grep` | Search file contents (ripgrep) | Find function definitions |
| `glob` | Find files by pattern | `**/*.ts`, `src/**/*.rs` |
| `ls` | List directory contents | Explore project structure |
| `web_fetch` | Fetch web content | Download documentation |
| `web_search` | Search the web | Query multiple search engines |
| `cron` | Manage scheduled tasks | Create/list/run cron jobs |

### Subagent Types

| Agent | Permissions | Use Case |
|-------|-------------|----------|
| `explore` | read, grep, glob, ls | Find code, understand structure |
| `general` | all except task | Complex multi-step tasks |
| `plan` | read, grep, glob, ls | Design implementation approach |

## Quick Start

### Installation

```bash
# Homebrew (macOS / Linux)
brew tap a3s-lab/tap
brew install a3s-code

# Cargo
cargo install a3s-code

# From source
cargo build --release
```

### Run

```bash
# Start with default settings
a3s-code

# With config file
a3s-code --config ~/.a3s/config.json

# With JSON structured logging
a3s-code --json-log

# With custom listen address and OTLP tracing
a3s-code --listen-addr 0.0.0.0:4088 --otlp-endpoint http://localhost:4317

# Self-update to latest version
a3s-code update
```

### CLI Options

| Flag | Env Var | Default | Description |
|------|---------|---------|-------------|
| `-c, --config` | `A3S_CONFIG` | — | Path to config.json file |
| `-l, --listen-addr` | `LISTEN_ADDR` | `0.0.0.0:4088` | gRPC server listen address |
| `--otlp-endpoint` | `OTEL_EXPORTER_OTLP_ENDPOINT` | — | OpenTelemetry OTLP endpoint |
| `--json-log` | `A3S_LOG_FORMAT` | `false` | Output logs in JSON format |

### Server Configuration

Create `~/.a3s/config.json` (optional — LLM can also be configured per-session via `ConfigureSession` RPC):

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
          "family": "claude-sonnet",
          "toolCall": true,
          "temperature": true,
          "reasoning": false,
          "attachment": false,
          "modalities": { "input": ["text"], "output": ["text"] },
          "cost": { "input": 3.0, "output": 15.0, "cacheRead": 0.3, "cacheWrite": 3.75 },
          "limit": { "context": 200000, "output": 16384 }
        }
      ]
    },
    {
      "name": "openai",
      "apiKey": "sk-...",
      "baseUrl": "https://api.openai.com",
      "models": [
        {
          "id": "gpt-4o",
          "name": "GPT-4o",
          "family": "gpt-4o",
          "toolCall": true,
          "temperature": true,
          "cost": { "input": 2.5, "output": 10.0 },
          "limit": { "context": 128000, "output": 16384 }
        }
      ]
    },
    {
      "name": "kimi",
      "apiKey": "sk-...",
      "baseUrl": "http://your-kimi-endpoint/v1",
      "models": [
        {
          "id": "kimi-k2.5",
          "name": "Kimi K2.5",
          "family": "kimi",
          "toolCall": true,
          "reasoning": true,
          "cost": { "input": 2.0, "output": 8.0 },
          "limit": { "context": 131072, "output": 8192 }
        }
      ]
    }
  ],
  "storageBackend": "file",
  "storageUrl": null,
  "sessionsDir": "~/.a3s/sessions",
  "skillDirs": ["~/.a3s/skills"],
  "agentDirs": ["~/.a3s/agents"],
  "watchEnabled": false
}
```

### Per-Session LLM Configuration

LLM can be configured per-session via `ConfigureSession` RPC, no server-level config needed:

```typescript
await client.configureSession(sessionId, {
  llm: {
    provider: 'openai',
    model: 'gpt-4o',
    apiKey: 'sk-...',
    baseUrl: 'https://api.openai.com',
    temperature: 0.7,
    maxTokens: 4096,
  },
  workspace: '/path/to/project',
  systemPrompt: 'You are a helpful coding assistant.',
  maxContextLength: 200000,
  autoCompact: true,
  autoCompactThreshold: 0.8,
});
```

```python
from a3s_code import SessionConfig, LLMConfig

await client.configure_session(session_id, SessionConfig(
    llm=LLMConfig(
        provider="openai",
        model="gpt-4o",
        api_key="sk-...",
        base_url="https://api.openai.com",
        temperature=0.7,
        max_tokens=4096,
    ),
    workspace="/path/to/project",
    system_prompt="You are a helpful coding assistant.",
    max_context_length=200000,
    auto_compact=True,
))
```

## SDK

### TypeScript

```bash
npm install @a3s-lab/code
```

```typescript
import { A3sClient, createProvider } from '@a3s-lab/code';

const client = new A3sClient({ address: 'localhost:4088' });
const anthropic = createProvider({ name: 'anthropic', apiKey: 'sk-ant-...' });

// High-level: auto-dispose session with `await using`
await using session = await client.createSession({
  model: anthropic('claude-sonnet-4-20250514'),
  workspace: '/project',
  system: 'You are a senior engineer.',
  autoCompact: true,
});

// Server-side agentic loop (recommended)
for await (const event of session.stream('Refactor the auth module')) {
  if (event.textDelta) process.stdout.write(event.textDelta);
  if (event.toolStart) console.log(`\nTool: ${event.toolStart.name}`);
  if (event.toolEnd) console.log(`Result: ${event.toolEnd.output.slice(0, 100)}`);
}

// Or use low-level client API directly
const { sessionId } = await client.createSession({
  workspace: '/project',
  llm: {
    provider: 'anthropic',
    model: 'claude-sonnet-4-20250514',
    apiKey: 'sk-ant-...',
  },
});

for await (const event of client.streamAgenticGenerate(sessionId, 'Explain this codebase')) {
  if (event.textDelta) process.stdout.write(event.textDelta);
}

await client.destroySession(sessionId);
```

### Python

```bash
pip install a3s-code
```

```python
from a3s_code import A3sClient, create_provider

anthropic = create_provider(name="anthropic", api_key="sk-ant-...")

async with A3sClient(address="localhost:4088") as client:
    # High-level: auto-cleanup session with `async with`
    async with await client.session(
        model=anthropic("claude-sonnet-4-20250514"),
        workspace="/project",
        system="You are a senior engineer.",
        auto_compact=True,
    ) as session:
        # Server-side agentic loop
        result = await session.send("Refactor the auth module")
        print(result.text)

        # Streaming
        async for event in session.stream("Explain this codebase"):
            if event.type == "text":
                print(event.content, end="", flush=True)

        # Delegate to subagent
        explore = await session.delegate("explore", "Find all API endpoints")
        print(explore.text)

    # Or use low-level client API directly
    result = await client.create_session(
        workspace="/project",
        llm={
            "provider": "anthropic",
            "model": "claude-sonnet-4-20250514",
            "api_key": "sk-ant-...",
        },
    )
    session_id = result["session_id"]

    async for event in client.stream_agentic_generate(session_id, "Explain this codebase"):
        if event.get("text_delta"):
            print(event["text_delta"], end="", flush=True)

    await client.destroy_session(session_id)
```

### Permission System

```typescript
// High-level session API
await session.setPermissions({
  defaultAction: 'ask',
  rules: [
    { tool: 'Bash', pattern: 'rm -rf:*', action: 'deny' },
    { tool: 'Read', pattern: 'src/**', action: 'allow' },
    { tool: 'Grep', pattern: 'src/**', action: 'allow' },
    { tool: 'Bash', pattern: '*', action: 'ask' },
    { tool: 'Write', pattern: '*', action: 'ask' },
  ],
});
```

```python
# High-level session API
await session.set_permissions(
    default_action="ask",
    allow=["Read(src/**)", "Grep(src/**)"],
    deny=["Bash(rm -rf:*)"],
    ask=["Bash(*)", "Write(*)"],
)
```

### Human-in-the-Loop (HITL)

```typescript
// High-level session API
await session.setConfirmation({
  autoApprove: ['read', 'grep', 'glob', 'ls'],
  requireConfirmation: ['bash', 'write', 'edit'],
  timeout: 30000,
  timeoutAction: 'reject',
});

// Respond to confirmation request
await session.confirm(confirmationId, true, 'Approved');
```

```python
# High-level session API
await session.set_confirmation(
    auto_approve=["read", "grep", "glob", "ls"],
    require_confirmation=["bash", "write", "edit"],
    timeout=30000,
    timeout_action="reject",
)

# Respond to confirmation request
await session.confirm(confirmation_id, True, "Approved")
```

### Skills System

Skills are prompt-injection Markdown files that extend agent behavior. Compatible with Claude Code Skills format.

```yaml
# ~/.a3s/skills/deploy.md
---
name: deploy
description: Deploy to production
allowed_tools: Bash(kubectl:*)
---
You are a deployment specialist. Run kubectl apply to deploy the application.
Always verify the deployment status after applying changes.
```

```typescript
// High-level session API
await session.loadSkill('deploy');
await session.loadSkills('~/.a3s/skills');
const skills = await session.listSkills();
```

```python
# High-level session API
await session.load_skill("deploy")
await session.load_skills("~/.a3s/skills")
skills = await session.list_skills()
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

```python
await client.start_lsp_server("rust", "file:///path/to/project")

hover = await client.lsp_hover("/path/to/file.rs", 10, 5)
defs = await client.lsp_definition("/path/to/file.rs", 15, 10)
refs = await client.lsp_references("/path/to/file.rs", 20, 8)
symbols = await client.lsp_symbols("main")
diags = await client.lsp_diagnostics("/path/to/file.rs")
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

```python
await client.register_mcp_server({
    "name": "filesystem",
    "transport": {"stdio": {"command": "npx", "args": ["-y", "@modelcontextprotocol/server-filesystem"]}},
    "enabled": True,
    "env": {},
})

await client.connect_mcp_server("filesystem")
tools = await client.get_mcp_tools()
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

```python
result = await client.create_cron_job("backup", "every day at 2am", "backup.sh")
jobs = await client.list_cron_jobs()
await client.run_cron_job(result["job"]["id"])
history = await client.get_cron_history(result["job"]["id"])
await client.pause_cron_job(result["job"]["id"])
await client.resume_cron_job(result["job"]["id"])

# Parse natural language to cron expression
parsed = await client.parse_cron_schedule("every 5 minutes")
# {"cron_expression": "*/5 * * * *", "description": "every 5 minutes"}
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
const stats = await client.getMemoryStats(sessionId);
```

```python
# Store a memory
await client.store_memory(session_id, {
    "content": "User prefers TypeScript over JavaScript",
    "importance": 0.8,
    "tags": ["preference", "language"],
    "memory_type": "MEMORY_TYPE_SEMANTIC",
})

# Search memories
results = await client.search_memories(session_id, "TypeScript", ["preference"], 10)
stats = await client.get_memory_stats(session_id)
```

### Planning & Goal Tracking

```typescript
const plan = await client.createPlan(sessionId, 'Refactor auth module');
const goal = await client.extractGoal(sessionId, 'Improve test coverage to 90%');
const check = await client.checkGoalAchievement(sessionId, goal, 'Current coverage: 85%');
```

```python
plan = await client.create_plan(session_id, "Refactor auth module")
goal = await client.extract_goal(session_id, "Improve test coverage to 90%")
check = await client.check_goal_achievement(session_id, goal, "Current coverage: 85%")
```

## API Reference

### Lifecycle (4 RPCs)

| Method | Description |
|--------|-------------|
| `healthCheck()` | Check agent health (version, uptime, sessions, store health) |
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

### Server-Side Agentic Loop (2 RPCs)

| Method | Description |
|--------|-------------|
| `agenticGenerate(sessionId, prompt, strategy, maxSteps)` | Run full agentic loop on server (unary) |
| `streamAgenticGenerate(sessionId, prompt, strategy, maxSteps)` | Run full agentic loop on server (streaming) |

### Server-Side Delegation (2 RPCs)

| Method | Description |
|--------|-------------|
| `delegate(sessionId, agentType, prompt)` | Delegate task to subagent (unary) |
| `streamDelegate(sessionId, agentType, prompt)` | Delegate task to subagent (streaming) |

### Skill Management (5 RPCs)

| Method | Description |
|--------|-------------|
| `loadSkill(sessionId, skillName, skillContent)` | Load a skill (global) |
| `unloadSkill(sessionId, skillName)` | Unload a skill |
| `listSkills(sessionId)` | List all loaded skills |
| `getSkill(name)` | Get skill by name or all skills |
| `loadSkillsFromDir(sessionId, directory, recursive)` | Load all skills from a directory |

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

### Observability (2 RPCs)

| Method | Description |
|--------|-------------|
| `getToolMetrics(sessionId, toolName)` | Get per-tool execution metrics (calls, duration, success/failure rate) |
| `getCostSummary(sessionId)` | Get per-session token cost summary |

### Queue Statistics (1 RPC)

| Method | Description |
|--------|-------------|
| `getQueueStats(sessionId)` | Get lane queue statistics (pending, active, completed, failed) |

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
│   └── code_agent.proto     # gRPC service definition (85 RPCs)
├── skills/
│   ├── builtin-tools.md     # Built-in tool definitions (11 tools)
│   └── find-skills.md       # Built-in skill: discover & install skills
├── sdk/
│   ├── typescript/           # TypeScript SDK (@a3s-lab/code)
│   └── python/               # Python SDK (a3s-code)
└── src/
    ├── lib.rs               # Library entry point
    ├── main.rs              # CLI entry point (server startup)
    ├── service.rs           # gRPC service implementation
    ├── agent.rs             # Agentic loop execution
    ├── session.rs           # Session management
    ├── tools/               # Tool system (built-in + dynamic tools, skills)
    ├── subagent.rs          # Subagent system (explore, general, plan)
    ├── llm.rs               # LLM provider integration (Anthropic, OpenAI-compatible)
    ├── config.rs            # Configuration management
    ├── permissions.rs       # Permission system (allow/deny/ask rules)
    ├── hitl.rs              # Human-in-the-loop confirmation
    ├── lsp/                 # Language Server Protocol integration
    ├── mcp/                 # Model Context Protocol support
    ├── memory.rs            # Memory system
    ├── planning/            # Planning & goal tracking
    ├── context.rs           # Context compaction
    ├── security/            # Security guards (sanitizer, taint, injection defense)
    ├── hooks/               # Hook engine for event-driven extensions
    ├── telemetry.rs         # OpenTelemetry instrumentation + JSON logging
    └── session_lane_queue.rs # Priority queue integration
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
│                      │          ▲                        │
│  Utilities:      a3s-lane    You are here               │
│              (priority queue)                            │
└──────────────────────────────────────────────────────────┘
```

| Project | Package | Relationship |
|---------|---------|--------------|
| **box** | `a3s-box-*` | Sandbox runtime that hosts `a3s-code` |
| **code** | `a3s-code` | AI coding agent (this project) |
| **lane** | `a3s-lane` | Priority queue used by `a3s-code` for command scheduling |

## Roadmap

### Phase 1: Core ✅

- [x] Multi-session management with isolated context
- [x] 11 built-in tools (bash, read, write, edit, patch, grep, glob, ls, web_fetch, web_search, cron)
- [x] LLM provider integration with streaming (Anthropic, OpenAI-compatible)
- [x] Thinking model compatibility (kimi-k2.5, DeepSeek-R1 reasoning_content)
- [x] Permission system (allow/deny/ask rules)
- [x] Human-in-the-loop (HITL) confirmation
- [x] Event streaming for real-time updates
- [x] Context compaction (manual + auto-compact at configurable threshold)
- [x] API retry with exponential backoff (429, 500, 502, 503, 529)
- [x] File version history with snapshots, diff, and restore
- [x] Per-session token cost tracking with model-specific pricing
- [x] Session export to Markdown
- [x] Session fork with full state copy
- [x] Auto title generation
- [x] 1716 unit tests (0 failures)

### Phase 2: Extensibility ✅

- [x] Skills system (Markdown-based prompt-injection with tool permissions)
- [x] Subagent system (explore, general, plan agents)
- [x] Lane integration for priority-based command scheduling
- [x] Todo/task tracking within sessions
- [x] Provider configuration (multi-provider, per-model overrides)
- [x] OpenAI-compatible message format

### Phase 3: Ecosystem ✅

- [x] LSP integration (hover, definition, references, symbols, diagnostics)
- [x] MCP support (register, connect, disconnect, tool discovery)
- [x] Cron scheduling (natural language + cron expressions)
- [x] Planning & goal tracking
- [x] Memory system (episodic, semantic, procedural, working)
- [x] Web search with multiple engine support
- [x] Server-side agentic loop (`AgenticGenerate` / `StreamAgenticGenerate`)
- [x] Server-side delegation (`Delegate` / `StreamDelegate`)
- [x] Batch skill loading from directory

### Phase 4: SDK & API ✅

- [x] TypeScript SDK with full 85 RPC coverage (`@a3s-lab/code`)
- [x] Python SDK with full 85 RPC coverage (`a3s-code` on PyPI)
- [x] OpenAI-compatible chat completion API
- [x] Comprehensive type exports and documentation

### Phase 5: Observability ✅

- [x] OpenTelemetry spans (agent → turn → LLM/tool)
- [x] Per-session cost tracking with `GetCostSummary` RPC
- [x] Tool execution metrics with `GetToolMetrics` RPC
- [x] Queue statistics with `GetQueueStats` RPC
- [x] JSON structured logging (`--json-log`)
- [x] Concise span attributes (no prompt in spans)

### Phase 6: Production ✅

- [x] Health check endpoint with subsystem diagnostics (version, uptime, session count, store health)
- [x] Pluggable session persistence (`SessionStore` trait, `Custom` backend with `start_server_with_store()`)
- [x] Proto `STORAGE_TYPE_CUSTOM` for external backends (PostgreSQL, etc.)

### Phase 7: Security Guards ✅

- [x] Output sanitizer (scan and redact sensitive data)
- [x] Taint tracking (mark sensitive data, track through encodings)
- [x] Tool call interceptor (block leaky tool calls)
- [x] Session isolation with secure wipe
- [x] Prompt injection defense (pattern-based detection)
- [x] Adopted `a3s-privacy` crate for shared classification

## License

MIT

---

<p align="center">
  Built by <a href="https://github.com/a3s-lab">A3S Lab</a>
</p>
