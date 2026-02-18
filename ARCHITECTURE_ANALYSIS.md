# A3S Code Core Architecture Analysis

**Generated:** 2026-02-18
**Crate:** `a3s-code-core`
**Total Lines:** 36,477 (excluding tests: ~28,000)

---

## Executive Summary

The `a3s-code-core` crate is a **4,738-line agent loop** surrounded by **23,000+ lines of infrastructure**. The core agent execution logic is tightly coupled to 10+ subsystems, making it difficult to extract or reuse independently.

**Key Findings:**
- **Minimal Core:** ~5,000 lines (agent.rs + llm/* + tools/types.rs)
- **Extension Infrastructure:** ~23,000 lines (hooks, permissions, planning, memory, MCP, telemetry, etc.)
- **Trait-Based Extension Points:** 10 public traits for pluggability
- **Tight Coupling:** Agent loop directly depends on 8+ subsystems (HITL, permissions, planning, hooks, context, queue, telemetry)

---

## Module Inventory (by Line Count)

### Core Agent Loop (Essential)
| Module | Lines | Responsibility | Coupling |
|--------|-------|----------------|----------|
| `agent.rs` | 4,738 | Main agent execution loop, turn management, tool orchestration | **HIGH** - depends on 8+ subsystems |
| `llm/openai.rs` | 570 | OpenAI API client (streaming, function calling) | Low |
| `llm/anthropic.rs` | 470 | Anthropic API client (streaming, tool use) | Low |
| `llm/types.rs` | 196 | LLM types (Message, ToolCall, TokenUsage) | None |
| `llm/mod.rs` | 47 | LlmClient trait definition | None |
| `tools/types.rs` | 157 | Tool trait, ToolContext, ToolOutput | Low |
| `tools/mod.rs` | 409 | ToolExecutor (orchestrates tool execution) | Medium |

**Subtotal: ~6,587 lines** (18% of codebase)

### Session Management (Essential)
| Module | Lines | Responsibility | Coupling |
|--------|-------|----------------|----------|
| `session/manager.rs` | 1,465 | Multi-session lifecycle, persistence | High |
| `session/mod.rs` | 635 | Session state machine, config | Medium |
| `session/compaction.rs` | 114 | Context window compaction | Low |
| `store.rs` | 791 | SessionStore trait, FileSessionStore, MemorySessionStore | Low |

**Subtotal: ~3,005 lines** (8% of codebase)

### Configuration & Error Handling (Essential)
| Module | Lines | Responsibility | Coupling |
|--------|-------|----------------|----------|
| `config.rs` | 1,445 | CodeConfig, ModelConfig, ProviderConfig (HCL/JSON) | Low |
| `error.rs` | 233 | CodeError enum, Result type alias | None |
| `agent_api.rs` | 823 | High-level Agent/AgentSession facade | Medium |

**Subtotal: ~2,501 lines** (7% of codebase)

### Extension Systems (Could Be Externalized)
| Module | Lines | Responsibility | Trait-Based? | Coupling |
|--------|-------|----------------|--------------|----------|
| `hooks/engine.rs` | 744 | Hook lifecycle engine (8 events) | ✅ `HookHandler` | Low |
| `hooks/events.rs` | 488 | Hook event definitions | N/A | None |
| `hooks/matcher.rs` | 480 | Pattern matching for hooks | N/A | Low |
| `permissions.rs` | 1,060 | Permission policy engine (allow/deny/confirm) | ❌ Concrete | Medium |
| `planning/mod.rs` | 692 | Task planning, ExecutionPlan, AgentGoal | ❌ Concrete | Medium |
| `planning/llm_planner.rs` | 440 | LLM-based planner | ❌ Concrete | Medium |
| `memory.rs` | 1,154 | Episodic/semantic/procedural memory | ✅ `MemoryStore` | Medium |
| `context.rs` | 798 | Context providers (augment prompts) | ✅ `ContextProvider` | Low |
| `hitl.rs` | 904 | Human-in-the-loop confirmation | ❌ Concrete | Medium |
| `telemetry.rs` | 958 | OpenTelemetry spans, metrics | ❌ Concrete | Low |
| `security/mod.rs` | 68 | Security provider trait | ✅ `SecurityProvider` | Low |
| `security/config.rs` | 147 | Security config, redaction rules | N/A | Low |

**Subtotal: ~7,933 lines** (22% of codebase)

### MCP Integration (Could Be Externalized)
| Module | Lines | Responsibility | Trait-Based? | Coupling |
|--------|-------|----------------|--------------|----------|
| `mcp/protocol.rs` | 1,161 | JSON-RPC 2.0 protocol types | N/A | None |
| `mcp/manager.rs` | 545 | MCP server lifecycle management | N/A | Medium |
| `mcp/client.rs` | 374 | MCP client implementation | N/A | Low |
| `mcp/transport/stdio.rs` | 361 | Stdio transport for MCP | ✅ `McpTransport` | Low |
| `mcp/tools.rs` | 148 | MCP tool wrapper (adapts MCP tools to Tool trait) | N/A | Low |

**Subtotal: ~2,589 lines** (7% of codebase)

### Queue & Task Management (Could Be Externalized)
| Module | Lines | Responsibility | Trait-Based? | Coupling |
|--------|-------|----------------|--------------|----------|
| `session_lane_queue.rs` | 813 | Lane-based task queue (main/external/confirmation) | N/A | Medium |
| `queue.rs` | 416 | SessionCommand trait, lane definitions | ✅ `SessionCommand` | Low |
| `tools/task.rs` | 929 | Task/ParallelTask tools (subagent execution) | N/A | High |
| `subagent.rs` | 799 | Subagent registry, YAML/MD parsing | N/A | Medium |

**Subtotal: ~2,957 lines** (8% of codebase)

### Built-in Tools (Could Be Externalized)
| Module | Lines | Responsibility | Coupling |
|--------|-------|----------------|----------|
| `tools/registry.rs` | 432 | Tool registry (name → Tool mapping) | Low |
| `tools/builtin/patch.rs` | 348 | Patch tool (unified diff) | Low |
| `tools/builtin/web_search.rs` | 334 | Web search tool (Tavily API) | Low |
| `tools/builtin/web_fetch.rs` | 294 | Web fetch tool (HTTP GET) | Low |
| `tools/builtin/grep.rs` | 266 | Grep tool (ripgrep wrapper) | Low |
| `tools/builtin/edit.rs` | 221 | Edit tool (string replacement) | Low |
| `tools/builtin/ls.rs` | 181 | Ls tool (directory listing) | Low |
| `tools/builtin/read.rs` | 174 | Read tool (file reading) | Low |
| `tools/builtin/glob_tool.rs` | 172 | Glob tool (pattern matching) | Low |
| `tools/builtin/write.rs` | 167 | Write tool (file writing) | Low |
| `tools/builtin/bash.rs` | 151 | Bash tool (shell command execution) | Low |

**Subtotal: ~2,540 lines** (7% of codebase)

### Utilities (Essential)
| Module | Lines | Responsibility | Coupling |
|--------|-------|----------------|----------|
| `file_history.rs` | 612 | File version tracking, snapshots | Low |
| `retry.rs` | 516 | Exponential backoff retry logic | None |
| `llm/http.rs` | 239 | HTTP client abstraction (reqwest) | Low |
| `llm/factory.rs` | 91 | LLM client factory | Low |
| `prompts.rs` | 153 | Prompt templates (subagent, planning, etc.) | None |

**Subtotal: ~1,611 lines** (4% of codebase)

### Tests (Not Counted in Production)
| Module | Lines | Notes |
|--------|-------|-------|
| `session/tests.rs` | 3,373 | Session integration tests |
| `llm/tests.rs` | 2,694 | LLM client unit tests |
| `agent.rs` (tests) | ~500 | Agent loop unit tests |

**Subtotal: ~6,567 lines** (18% of total, excluded from production count)

---

## Dependency Graph (Cross-Module Imports)

### Agent Loop Dependencies (agent.rs imports)
```
agent.rs
├── context::{ContextProvider, ContextQuery, ContextResult}
├── hitl::{ConfirmationManager, ConfirmationPolicy}
├── hooks::{HookEngine, HookEvent, HookResult, ...}
├── llm::{LlmClient, Message, ToolCall, ToolDefinition}
├── permissions::{PermissionDecision, PermissionPolicy}
├── planning::{AgentGoal, ExecutionPlan, TaskStatus}
├── queue::{SessionCommand, SessionLane}
├── session_lane_queue::SessionLaneQueue
└── tools::{ToolContext, ToolExecutor, ToolStreamEvent}
```

**Analysis:** Agent loop has **9 direct dependencies** on extension systems. This is the primary coupling bottleneck.

### Session Manager Dependencies (session/manager.rs imports)
```
session/manager.rs
├── agent::{AgentConfig, AgentEvent, AgentLoop, AgentResult}
├── hitl::ConfirmationPolicy
├── llm::{LlmClient, LlmConfig, Message}
├── permissions::{PermissionDecision, PermissionPolicy}
├── planning::Task
├── store::{SessionStore, SessionData, LlmConfigData}
└── tools::ToolExecutor
```

### Tool Executor Dependencies (tools/mod.rs imports)
```
tools/mod.rs
├── file_history::{FileHistory}
├── llm::ToolDefinition
└── permissions::{PermissionDecision, PermissionPolicy}
```

### MCP Dependencies (mcp/manager.rs imports)
```
mcp/manager.rs
├── mcp::client::McpClient
├── mcp::protocol::{...}
└── mcp::transport::{McpTransport, stdio::StdioTransport}
```

**Analysis:** MCP subsystem is **loosely coupled** (only internal dependencies). Could be extracted to separate crate.

---

## Trait-Based Extension Points

### 1. `Tool` Trait (tools/types.rs)
**Purpose:** Define custom tools for agent execution
**Implementations:**
- 11 built-in tools (Bash, Read, Write, Edit, Grep, Ls, Glob, Patch, WebSearch, WebFetch)
- 2 task tools (Task, ParallelTask)
- 1 MCP wrapper (McpToolWrapper)

**Interface:**
```rust
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn parameters(&self) -> Value;
    async fn execute(&self, args: Value, context: &ToolContext) -> Result<ToolOutput>;
}
```

**Coupling:** Low (only depends on ToolContext, ToolOutput)

---

### 2. `LlmClient` Trait (llm/mod.rs)
**Purpose:** Pluggable LLM providers
**Implementations:**
- AnthropicClient (Claude)
- OpenAiClient (GPT-4, GPT-3.5)

**Interface:**
```rust
pub trait LlmClient: Send + Sync {
    async fn complete(&self, messages: &[Message], tools: &[ToolDefinition], ...) -> Result<LlmResponse>;
    async fn stream(&self, messages: &[Message], tools: &[ToolDefinition], ...) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>>>>>;
}
```

**Coupling:** Low (only depends on Message, ToolDefinition, LlmResponse)

---

### 3. `SessionStore` Trait (store.rs)
**Purpose:** Pluggable session persistence
**Implementations:**
- FileSessionStore (JSON files)
- MemorySessionStore (in-memory HashMap)

**Interface:**
```rust
pub trait SessionStore: Send + Sync {
    async fn save(&self, session_id: &str, data: &SessionData) -> Result<()>;
    async fn load(&self, session_id: &str) -> Result<Option<SessionData>>;
    async fn delete(&self, session_id: &str) -> Result<()>;
    async fn list(&self) -> Result<Vec<String>>;
}
```

**Coupling:** Low (only depends on SessionData)

---

### 4. `MemoryStore` Trait (memory.rs)
**Purpose:** Pluggable memory backend (episodic, semantic, procedural)
**Implementations:**
- None in core (only test mocks)

**Interface:**
```rust
pub trait MemoryStore: Send + Sync {
    async fn store(&self, item: MemoryItem) -> Result<String>;
    async fn retrieve(&self, query: &str, limit: usize, memory_type: Option<MemoryType>) -> Result<Vec<MemoryItem>>;
    async fn update(&self, id: &str, item: MemoryItem) -> Result<()>;
    async fn delete(&self, id: &str) -> Result<()>;
}
```

**Coupling:** Low (only depends on MemoryItem, MemoryType)

---

### 5. `ContextProvider` Trait (context.rs)
**Purpose:** Augment prompts with external context (files, docs, memory)
**Implementations:**
- MemoryContextProvider (memory.rs)

**Interface:**
```rust
pub trait ContextProvider: Send + Sync {
    fn name(&self) -> &str;
    async fn resolve(&self, query: &ContextQuery) -> Result<ContextResult>;
}
```

**Coupling:** Low (only depends on ContextQuery, ContextResult)

---

### 6. `HookHandler` Trait (hooks/engine.rs)
**Purpose:** React to lifecycle events (PreToolUse, PostToolUse, GenerateStart, etc.)
**Implementations:**
- None in core (only test mocks)

**Interface:**
```rust
pub trait HookHandler: Send + Sync {
    async fn handle(&self, event: &HookEvent) -> Result<HookResult>;
}
```

**Coupling:** Low (only depends on HookEvent, HookResult)

---

### 7. `SecurityProvider` Trait (security/mod.rs)
**Purpose:** Pluggable security scanning (PII detection, injection detection)
**Implementations:**
- NoOpSecurityProvider (no-op)

**Interface:**
```rust
pub trait SecurityProvider: Send + Sync {
    async fn scan_input(&self, input: &str) -> Result<SecurityScanResult>;
    async fn scan_output(&self, output: &str) -> Result<SecurityScanResult>;
}
```

**Coupling:** Low (only depends on SecurityScanResult)

---

### 8. `SessionCommand` Trait (queue.rs)
**Purpose:** Define custom commands for lane-based queue
**Implementations:**
- ToolCommand (agent.rs)

**Interface:**
```rust
pub trait SessionCommand: Send + Sync {
    fn lane(&self) -> SessionLane;
    async fn execute(&self, context: &CommandContext) -> Result<CommandResult>;
}
```

**Coupling:** Low (only depends on SessionLane, CommandContext)

---

### 9. `McpTransport` Trait (mcp/transport/mod.rs)
**Purpose:** Pluggable MCP transport layer
**Implementations:**
- StdioTransport (stdio)

**Interface:**
```rust
pub trait McpTransport: Send + Sync {
    async fn send_request(&self, request: JsonRpcRequest) -> Result<JsonRpcResponse>;
    async fn send_notification(&self, notification: JsonRpcNotification) -> Result<()>;
    async fn receive_notification(&self) -> Result<Option<McpNotification>>;
}
```

**Coupling:** Low (only depends on MCP protocol types)

---

### 10. `HttpClient` Trait (llm/http.rs)
**Purpose:** Pluggable HTTP client (for testing)
**Implementations:**
- ReqwestHttpClient (reqwest)

**Interface:**
```rust
pub trait HttpClient: Send + Sync {
    async fn post(&self, url: &str, headers: HeaderMap, body: String) -> Result<HttpResponse>;
    async fn post_stream(&self, url: &str, headers: HeaderMap, body: String) -> Result<StreamingHttpResponse>;
}
```

**Coupling:** None (pure HTTP abstraction)

---

## Coupling Analysis

### Tight Coupling (Hard to Extract)
| Module | Coupled To | Reason |
|--------|-----------|--------|
| `agent.rs` | 9 subsystems | Agent loop directly imports HITL, permissions, planning, hooks, context, queue, telemetry |
| `session/manager.rs` | 7 subsystems | Session manager orchestrates agent loop + persistence + permissions |
| `tools/task.rs` | 3 subsystems | Task tool depends on session manager, subagent registry, agent events |
| `hitl.rs` | 2 subsystems | HITL depends on queue (SessionLane), agent (AgentEvent) |
| `permissions.rs` | 0 subsystems | Self-contained (only depends on serde, regex) |

### Loose Coupling (Easy to Extract)
| Module | Coupled To | Reason |
|--------|-----------|--------|
| `mcp/*` | 0 external | MCP subsystem is self-contained (only internal dependencies) |
| `llm/*` | 0 external | LLM clients only depend on retry.rs |
| `hooks/*` | 0 external | Hook engine is self-contained |
| `memory.rs` | 1 subsystem | Only depends on context.rs (ContextProvider impl) |
| `context.rs` | 0 external | Self-contained trait definition |
| `store.rs` | 5 subsystems | Only for data types (Message, TokenUsage, Task, etc.) — no logic coupling |

---

## Minimal Core Identification

### What is the ABSOLUTE MINIMUM needed for agent loop?

**Core Agent Loop (5,000 lines):**
1. `agent.rs` (4,738 lines) — **BUT** needs refactoring to remove 9 dependencies
2. `llm/*` (1,583 lines) — LlmClient trait + Anthropic/OpenAI implementations
3. `tools/types.rs` (157 lines) — Tool trait definition
4. `error.rs` (233 lines) — Error types
5. `retry.rs` (516 lines) — Retry logic

**Total: ~7,227 lines** (20% of codebase)

**Required Refactoring:**
- Extract HITL, permissions, planning, hooks, context, telemetry from agent.rs
- Make agent.rs only depend on: LlmClient, Tool, Message, ToolCall
- Move all extension logic to optional plugins

---

## Extension vs Core Classification

### ✅ Core (Essential for Agent Loop)
| Module | Lines | Reason |
|--------|-------|--------|
| `agent.rs` | 4,738 | Agent execution loop (needs refactoring) |
| `llm/*` | 1,583 | LLM client abstraction + implementations |
| `tools/types.rs` | 157 | Tool trait definition |
| `tools/mod.rs` | 409 | ToolExecutor (orchestrates tool execution) |
| `session/mod.rs` | 635 | Session state machine |
| `config.rs` | 1,445 | Configuration loading |
| `error.rs` | 233 | Error types |
| `retry.rs` | 516 | Retry logic |
| `store.rs` | 791 | Session persistence trait |

**Subtotal: ~10,507 lines** (29% of codebase)

---

### 🔌 Extension (Could Be Externalized)
| Module | Lines | Trait-Based? | Extraction Difficulty |
|--------|-------|--------------|----------------------|
| `hooks/*` | 1,712 | ✅ `HookHandler` | **Easy** (loose coupling) |
| `mcp/*` | 2,589 | ✅ `McpTransport` | **Easy** (self-contained) |
| `memory.rs` | 1,154 | ✅ `MemoryStore` | **Easy** (only depends on context.rs) |
| `context.rs` | 798 | ✅ `ContextProvider` | **Easy** (self-contained trait) |
| `security/*` | 215 | ✅ `SecurityProvider` | **Easy** (self-contained) |
| `permissions.rs` | 1,060 | ❌ Concrete | **Medium** (agent.rs depends on it) |
| `planning/*` | 1,132 | ❌ Concrete | **Medium** (agent.rs depends on it) |
| `hitl.rs` | 904 | ❌ Concrete | **Medium** (agent.rs depends on it) |
| `telemetry.rs` | 958 | ❌ Concrete | **Medium** (agent.rs depends on it) |
| `queue.rs` | 416 | ✅ `SessionCommand` | **Medium** (agent.rs depends on it) |
| `session_lane_queue.rs` | 813 | ❌ Concrete | **Medium** (agent.rs depends on it) |
| `tools/task.rs` | 929 | ❌ Concrete | **Hard** (depends on session manager) |
| `subagent.rs` | 799 | ❌ Concrete | **Medium** (only used by task.rs) |
| `tools/builtin/*` | 2,540 | ✅ `Tool` | **Easy** (trait-based) |
| `tools/registry.rs` | 432 | N/A | **Easy** (simple HashMap) |
| `file_history.rs` | 612 | N/A | **Easy** (only used by tools) |

**Subtotal: ~17,063 lines** (47% of codebase)

---

## Recommendations

### 1. Refactor Agent Loop to Remove Hard Dependencies
**Problem:** `agent.rs` directly imports 9 subsystems, making it impossible to use the agent loop without pulling in the entire codebase.

**Solution:**
- Introduce `AgentPlugin` trait for optional features (permissions, planning, HITL, telemetry)
- Make agent loop only depend on: `LlmClient`, `Tool`, `Message`, `ToolCall`
- Move all extension logic to plugin implementations

**Example:**
```rust
pub trait AgentPlugin: Send + Sync {
    async fn before_tool_call(&self, tool_call: &ToolCall) -> Result<PluginAction>;
    async fn after_tool_call(&self, tool_call: &ToolCall, result: &ToolOutput) -> Result<()>;
}

pub enum PluginAction {
    Continue,
    Block { reason: String },
    RequestConfirmation { timeout: Duration },
}
```

---

### 2. Extract MCP to Separate Crate
**Rationale:** MCP subsystem is self-contained (2,589 lines, no external dependencies).

**Proposed Structure:**
```
a3s-code-mcp/
├── src/
│   ├── protocol.rs    (1,161 lines)
│   ├── client.rs      (374 lines)
│   ├── manager.rs     (545 lines)
│   ├── transport/
│   │   ├── mod.rs     (29 lines)
│   │   └── stdio.rs   (361 lines)
│   └── tools.rs       (148 lines)
```

**Benefits:**
- Reduces core crate size by 7%
- Allows independent versioning of MCP protocol
- Easier to maintain MCP spec compliance

---

### 3. Extract Built-in Tools to Separate Crate
**Rationale:** Built-in tools are trait-based (2,540 lines, low coupling).

**Proposed Structure:**
```
a3s-code-tools/
├── src/
│   ├── bash.rs
│   ├── read.rs
│   ├── write.rs
│   ├── edit.rs
│   ├── grep.rs
│   ├── ls.rs
│   ├── glob.rs
│   ├── patch.rs
│   ├── web_search.rs
│   └── web_fetch.rs
```

**Benefits:**
- Reduces core crate size by 7%
- Allows users to opt-out of unused tools
- Easier to add new tools without modifying core

---

### 4. Extract Hooks to Separate Crate
**Rationale:** Hook engine is self-contained (1,712 lines, trait-based).

**Proposed Structure:**
```
a3s-code-hooks/
├── src/
│   ├── engine.rs      (744 lines)
│   ├── events.rs      (488 lines)
│   └── matcher.rs     (480 lines)
```

**Benefits:**
- Reduces core crate size by 5%
- Allows independent evolution of hook system
- Easier to add new hook events

---

### 5. Make Permissions/Planning/HITL Optional via Feature Flags
**Rationale:** These are extension features, not core requirements.

**Proposed Cargo.toml:**
```toml
[features]
default = []
permissions = []
planning = []
hitl = []
telemetry = ["opentelemetry"]
full = ["permissions", "planning", "hitl", "telemetry"]
```

**Benefits:**
- Reduces binary size for minimal deployments
- Allows users to opt-in to features they need
- Clearer separation of core vs extension

---

## Conclusion

The `a3s-code-core` crate is **architecturally sound** but **over-coupled**. The agent loop (4,738 lines) is surrounded by 23,000+ lines of infrastructure, much of which could be externalized.

**Key Metrics:**
- **Minimal Core:** ~10,500 lines (29% of codebase)
- **Externalizable Extensions:** ~17,000 lines (47% of codebase)
- **Trait-Based Extension Points:** 10 public traits
- **Tight Coupling Points:** agent.rs → 9 subsystems

**Priority Actions:**
1. **Refactor agent.rs** to use plugin trait instead of direct imports
2. **Extract MCP** to `a3s-code-mcp` crate (easy win, 7% size reduction)
3. **Extract built-in tools** to `a3s-code-tools` crate (easy win, 7% size reduction)
4. **Feature-gate extensions** (permissions, planning, HITL, telemetry)
5. **Extract hooks** to `a3s-code-hooks` crate (medium effort, 5% size reduction)

**Expected Outcome:**
- Core crate: ~10,500 lines (agent loop + LLM + session + config)
- Extension crates: ~17,000 lines (MCP, tools, hooks, memory, etc.)
- **60% size reduction** in core crate
- **Improved modularity** and reusability
