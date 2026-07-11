# A3S Code User & Developer Guide

> **Agentic Agent Framework** - A3S Code is a Rust library with native Python and Node.js bindings

---

## Table of Contents

- [Part 1: User Guide](#part-1-user-guide)
  - [1. Introduction](#1-introduction)
  - [2. Installation & Configuration](#2-installation--configuration)
  - [3. Quick Start](#3-quick-start)
  - [4. Core Concepts](#4-core-concepts)
  - [5. Tools System](#5-tools-system)
  - [6. Skills System](#6-skills-system)
  - [7. Multi-Agent Collaboration](#7-multi-agent-collaboration)
  - [8. Security & Permissions](#8-security--permissions)
  - [9. Slash Commands](#9-slash-commands)
  - [11. Session Management](#11-session-management)
- [Part 2: Developer Guide](#part-2-developer-guide)
  - [12. Architecture Overview](#12-architecture-overview)
  - [13. Development Environment](#13-development-environment)
  - [14. Core Modules](#14-core-modules)
  - [15. Extension Development](#15-extension-development)
  - [16. Hook System](#16-hook-system)
  - [17. Custom Tools and Skills](#17-custom-tools-and-skills)
  - [18. Testing & Debugging](#18-testing--debugging)
  - [19. Contributing Guidelines](#19-contributing-guidelines)

---

# Part 1: User Guide

## 1. Introduction

A3S Code is a powerful **Agentic Agent Framework** that enables Large Language Models (LLMs) to:

- **File Operations** - Read, write, edit, and patch files
- **Code Search** - Search codebases using Grep, Glob, and more
- **Command Execution** - Run shell commands in sandboxed environments
- **Web Access** - Web scraping and search capabilities
- **Task Delegation** - Distribute bounded work through `task` and `parallel_task`

### Supported Platforms

| Platform | Installation |
|----------|-------------|
| Python | `pip install a3s-code` |
| Node.js | `npm install @a3s-lab/code` |
| Rust | `cargo add a3s-code-core` |

### Supported LLM Providers

- **Anthropic** (Claude series)
- **OpenAI** (GPT series)
- **DeepSeek**
- **Kimi** (Moonshot)
- **Together**
- **Groq**

## 2. Installation & Configuration

### 2.1 Python Installation

```bash
pip install a3s-code
```

### 2.2 Node.js Installation

```bash
npm install @a3s-lab/code
```

### 2.3 Agent Configuration (agent.acl)

Create `agent.acl` configuration file:

```hcl
# Default model
default_model = "anthropic/claude-sonnet-4-20250514"

# LLM Provider Configuration
providers {
  name    = "anthropic"
  api_key = env("ANTHROPIC_API_KEY")
}

providers {
  name    = "openai"
  api_key = env("OPENAI_API_KEY")
}

# Storage backend: "memory", "file", or "custom"
storage_backend = "file"

# Sessions directory
sessions_dir = "./sessions"

# Skill directories
skill_dirs = ["./skills"]

# Maximum tool execution rounds
max_tool_rounds = 50
```

### 2.4 Environment Variables

```bash
export ANTHROPIC_API_KEY="your-key-here"
export OPENAI_API_KEY="your-key-here"
```

## 3. Quick Start

### 3.1 Python Example

```python
from a3s_code import Agent

# Create agent
agent = Agent.create("agent.acl")

# Create session
session = agent.session("/my-project")

# Send request
result = session.send("Analyze project architecture")
print(result.text)
```

### 3.2 Node.js Example

```typescript
import { Agent } from '@a3s-lab/code';

const agent = await Agent.create('agent.acl');
const session = agent.session('/my-project');

const result = await session.send('Analyze project architecture');
console.log(result.text);
```

### 3.3 First Tasks

```python
# Find authentication error handling
result = session.send("Find all places handling authentication errors")

# Review code quality
result = session.send("Review main.py code quality and suggest improvements")

# Run tests
result = session.send("Run test suite and report results")
```

## 4. Core Concepts

### 4.1 Architecture Layers

```
Agent (Config + Provider Registry)
  └── AgentSession (Workspace-bound execution API)
        ├── LlmClient      → Send messages, receive tool calls
        ├── ToolExecutor   → Run tools, enforce permissions
        ├── SkillRegistry  → Expose/invoke skills
        └── Context/trace/verification evidence
```

### 4.2 Core Components

| Component | Description |
|-----------|-------------|
| **Agent** | Top-level configuration and factory |
| **AgentSession** | Workspace-bound execution API for send/stream/tools/state |
| **Skill** | Markdown files defining behavior and capabilities |
| **Tool** | Functions the agent can invoke |

### 4.3 SessionOptions Configuration

```python
from a3s_code import Agent, SessionOptions

opts = SessionOptions()

# Specify model
opts.model = "openai/gpt-4o"

# Compatibility flag; A3S Code currently ships no embedded built-in skills.
opts.builtin_skills = True

# Load custom skills
opts.skill_dirs = ["./skills"]

# Core tools are registered by the runtime.
# Use session.tool_names() / session.tool_definitions() to inspect availability.

session = agent.session(".", opts)
```


## 5. Tools System

### 5.1 Built-in And Session Tools

#### File Tools

| Tool | Description | Example |
|------|-------------|---------|
| `read` | Read file content | `read: /path/to/file.py` |
| `write` | Write file | `write: /path/to/file.py` |
| `edit` | Edit file | `edit: /path/to/file.py` |
| `patch` | Apply patch | `patch: /path/to/file.py` |

#### Search Tools

| Tool | Description | Example |
|------|-------------|---------|
| `grep` | Text search | `grep: "function name"` |
| `glob` | File matching | `glob: "**/*.py"` |
| `ls` | Directory listing | `ls: /path/to/dir` |

#### Other Tools

| Tool | Description |
|------|-------------|
| `bash` | Execute shell commands |
| `web_fetch` | Fetch web page content |
| `web_search` | Perform web search |
| `git` | Git status, diff, branch, and worktree operations |
| `program` | Bounded programmatic tool calling (PTC) |

### 5.2 Delegation Tools

| Tool | Description |
|------|-------------|
| `task` | Delegate to single agent |
| `parallel_task` | Delegate multiple tasks in parallel |
| `batch` | Batch execute tasks |
| `Skill` | Invoke specific skill |

### 5.3 Programmatic Tool Calling

```python
result = session.program({
    "source": """
        export default async function run(ctx, inputs) {
          const hits = await ctx.grep(inputs.query, { glob: "*.py" });
          return { hits };
        }
    """,
    "inputs": {"query": "PermissionPolicy"},
    "allowed_tools": ["grep"],
})
print(result.output)
```

## 6. Skills System

Skills are Markdown files that shape LLM behavior.

### 6.1 Skill File Structure

```markdown
---
name: safe-reviewer
description: Review code without modifying files
allowed-tools: "read(*), grep(*), glob(*)"
---

Review code in the workspace. You may read and search files,
but you must not write, edit, or execute anything.

Review checklist:
1. Check for potential security issues
2. Verify error handling
3. Evaluate code readability
4. Provide improvement suggestions
```

### 6.2 Using Skills

```python
opts = SessionOptions()
opts.skill_dirs = ["./skills"]
opts.builtin_skills = True  # Compatibility no-op; no embedded built-in skills ship by default
session = agent.session(".", opts)
```

### 6.3 Skill Loading

A3S Code no longer ships default embedded skills. Load reusable behavior from
`skill_dirs`, inline skills, or an explicit `SkillRegistry`.

## 7. Multi-Agent Collaboration

### 7.1 Single Delegated Task

```python
result = session.send('task: explore codebase and summarize architecture')
```

### 7.2 Parallel Tasks

```python
result = session.send('parallel_task: [audit security, check performance, review tests]')
```

### 7.3 Delegation Model

The 2.x runtime uses a single delegation surface: `task` for one bounded child
run and `parallel_task` for independent fan-out. Planning mode can also route
plan steps to these tools deterministically when the generated step declares
`tool = "task"` or `tool = "parallel_task"`.

### 7.4 Agent Types

| Type | Description |
|------|-------------|
| `explore` | Read-only exploration |
| `general` | Full capabilities |
| `plan` | Analysis only |
| `verification` | Adversarial validation |
| `review` | Code review |

## 8. Security & Permissions

### 8.1 Permission Policy

```python
from a3s_code import SessionOptions, PermissionPolicy

opts = SessionOptions()
opts.permission_policy = PermissionPolicy(
    allow=[
        "read(*)",
        "grep(*)"
    ],
    deny=[
        "bash(*)"
    ],
    default_decision="deny",
)
session = agent.session(".", opts)
```

### 8.2 Human-in-the-Loop (HITL)

```python
# Prompt confirmation before each tool call
opts.hitl_enabled = True
```

### 8.3 Security Features

| Feature | Description |
|---------|-------------|
| **Explicit Permissions** | Deny by default, explicit allow required |
| **Human Confirmation** | Prompt before tool execution |
| **Skill Restrictions** | `allowed-tools` limits callable tools |
| **Auto-compact** | Auto compress context before token limits |
| **Circuit Breaker** | Stop after 3 consecutive failures |


## 9. Slash Commands

Type `/help` in any session to see available commands:

| Command | Description |
|---------|-------------|
| `/help` | List available commands |
| `/model [provider/model]` | Show or switch current model |
| `/cost` | Show token usage and estimated cost |
| `/clear` | Clear conversation history |
| `/compact` | Manually trigger context compaction |
| `/tools` | List registered tools |

### 9.1 Custom Commands

```python
session.register_command(
    "status", 
    "Show status", 
    lambda args, ctx: f"Model: {ctx['model']}"
)
result = session.send("/status")
```

## 11. Session Management

### 11.1 Session Persistence

```python
from a3s_code import SessionOptions, FileSessionStore, FileMemoryStore

opts = SessionOptions()
opts.session_store = FileSessionStore('./sessions')
opts.memory_store = FileMemoryStore('./memory')
opts.session_id = 'my-session'
opts.auto_save = True

session = agent.session(".", opts)

# Resume session
resumed = agent.resume_session('my-session', opts)
```

### 11.3 Multi-Provider Switching

```python
# Switch model per session
session = agent.session(".", model="openai/gpt-4o")
```

---

# Part 2: Developer Guide

## 12. Architecture Overview

### 12.1 System Architecture

```
A3S Code
├── Python SDK (PyO3)
├── Node.js SDK (NAPI)
└── Rust Core
    ├── Agent (configuration facade)
    ├── AgentSession (workspace-bound execution API)
    ├── Context assembly
    ├── Tool selection and execution
    ├── Skills and delegated task execution
    ├── Permission / HITL / hooks
    └── Trace, artifacts, and verification evidence
```

### 12.2 Core Modules

| Module | Path | Description |
|--------|------|-------------|
| `agent_api.rs` | `core/src/` | Public `Agent` / `AgentSession` facade |
| `agent.rs` | `core/src/` | Internal turn runner |
| `context/` | `core/src/context/` | Context assembly and providers |
| `tools/` | `core/src/tools/` | Tool implementations |
| `skills/` | `core/src/skills/` | Skill system |
| `llm/` | `core/src/llm/` | LLM clients |
| `permissions.rs` | `core/src/` | Permission control |
| `hooks/` | `core/src/hooks/` | Hook system |
| `trace.rs` | `core/src/` | Execution traces |
| `verification.rs` | `core/src/` | Completion evidence and verification summaries |

## 13. Development Environment

### 13.1 Prerequisites

```bash
# Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# Python (for Python SDK)
python -m pip install maturin

# Node.js (for Node.js SDK)
npm install -g napi-rs
```

### 13.2 Clone and Build

```bash
git clone <repository-url>
cd a3s-code

# Build core
cargo build --release

# Build Python SDK
cd sdk/python
maturin develop

# Build Node.js SDK
cd sdk/node
npm install
npm run build
```

### 13.3 Development Tools

```bash
# Run tests
cargo test

# Linting
cargo clippy

# Formatting
cargo fmt

# Use just for tasks
just --list
```


## 14. Core Modules

### 14.1 Agent Module (`agent_api.rs`)

```rust
let agent = Agent::create("agent.acl").await?;
let session = agent
    .session_builder("/repo")
    .options(options)
    .build()
    .await?;

let resumed = agent
    .resume_session_async("session-id", resume_options)
    .await?;
```

Rust construction is async-first. `session_async`, the async agent/worker
factories, and `resume_session_async` share the same resolved construction
kernel. The synchronous `session` factory is compatibility-only: it requires an
explicitly pre-initialized memory store, never blocks an async runtime, and
returns `AsyncSessionBuildRequired` for resources that still need async setup.
A host MCP manager in `SessionOptions` always requires the async path for tool
discovery; the sync path can only inherit already-cached agent-global tools.

### 14.2 AgentSession Module (`agent_api.rs`)

`AgentSession` exposes async `send`, `stream`, and direct tool calls. Conversation
operations are fail-fast single-flight and return `SessionBusy` on overlap.
Every run shares one invocation context for identity, cancellation, events, and
governance. SDK events project the stable, lossless `EventEnvelopeV1` wire shape.

### 14.3 Tool Module (`tools/`)

```rust
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn execute(&self, input: ToolInput) -> Result<ToolOutput>;
}
```

## 15. Extension Development

### 15.1 Creating Custom Tools

```rust
use a3s_code_core::tools::{Tool, ToolInput, ToolOutput};

pub struct MyTool;

impl Tool for MyTool {
    fn name(&self) -> &str { "my_tool" }
    fn description(&self) -> &str { "My custom tool description" }
    fn execute(&self, input: ToolInput) -> Result<ToolOutput> {
        Ok(ToolOutput::new("result"))
    }
}
```

### 15.2 Creating Custom Skills

Create Markdown file in `skills/` directory:

```markdown
---
name: my-skill
description: My custom skill
allowed-tools: "read(*), grep(*)"
---

# My Skill

Detailed description for LLM to use when executing tasks.
```

## 16. Hook System

### 16.1 Available Hook Events

| Event | Description | Blockable |
|-------|-------------|-----------|
| `PreToolUse` | Before tool use | Yes |
| `PostToolUse` | After tool use | No |
| `GenerateStart` | Before generation | Yes |
| `GenerateEnd` | After generation | No |
| `SessionStart` | Session start | No |
| `SessionEnd` | Session end | No |

### 16.2 Implementing HookHandler

```rust
use a3s_code::HookHandler;

struct MyHook;

impl HookHandler for MyHook {
    fn pre_tool_use(&self, tool_name: &str, tool_input: &Value, ctx: &Context) -> HookResult {
        if tool_name == "bash" && tool_input.contains("rm -rf") {
            return HookResult::block("Refusing destructive command");
        }
        HookResult::continue_()
    }
}
```

## 17. Custom Tools and Skills

### 17.1 Extension Surface

A3S Code 2.x keeps extension points explicit and SDK-owned. Extend the runtime with:

- Custom tools registered in the host SDK
- Markdown skills loaded from `skill_dirs`
- Hooks for policy, telemetry, and workflow integration
- MCP servers for external capabilities

## 18. Testing & Debugging

### 18.1 Unit Tests

```rust
#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_my_tool() {
        let tool = MyTool;
        let input = ToolInput::new(json!({"key": "value"}));
        let output = tool.execute(input).unwrap();
        assert_eq!(output.text(), "expected");
    }
}
```

### 18.2 Debugging Tips

```bash
# Enable verbose logs
export RUST_LOG=debug
export A3S_DEBUG=1
```

## 19. Contributing Guidelines

### 19.1 Code Style

- Follow Rust standard style
- Use `cargo fmt` for formatting
- Use `cargo clippy` for linting
- Document all public APIs

### 19.2 Commit Convention

```
feat: new feature
fix: bug fix
docs: documentation
style: formatting
refactor: refactoring
test: testing
chore: build/tools
```

---

**License**: MIT  
**Version**: See CHANGELOG in each SDK

*Last Updated: 2026-03-24*
