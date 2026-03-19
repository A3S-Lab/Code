# A3S Code

**Embed AI coding agents into any application.** A3S Code is a Rust library with native Python and Node.js bindings. Give an LLM a workspace, a set of tools, and a system prompt — it reads files, runs commands, searches code, and acts on results.

[![crates.io](https://img.shields.io/crates/v/a3s-code-core)](https://crates.io/crates/a3s-code-core)
[![PyPI](https://img.shields.io/pypi/v/a3s-code)](https://pypi.org/project/a3s-code/)
[![npm](https://img.shields.io/npm/v/@a3s-lab/code)](https://www.npmjs.com/package/@a3s-lab/code)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue)](LICENSE)

---

## Install

```bash
# Python
pip install a3s-code

# Node.js
npm install @a3s-lab/code

# Rust
cargo add a3s-code-core
```

---

## Quick Start

**1. Create an agent config** (`agent.hcl`):

```hcl
default_model = "anthropic/claude-sonnet-4-20250514"

providers {
  name    = "anthropic"
  api_key = env("ANTHROPIC_API_KEY")
}
```

**2. Run an agent session:**

```python
from a3s_code import Agent

agent = Agent.create("agent.hcl")
session = agent.session("/my-project")

result = session.send("Find all places where we handle authentication errors")
print(result.text)
```

```typescript
import { Agent } from '@a3s-lab/code';

const agent = await Agent.create('agent.hcl');
const session = agent.session('/my-project');

const result = await session.send('Find all places where we handle authentication errors');
console.log(result.text);
```

```rust
use a3s_code_core::Agent;

let agent = Agent::from_file("agent.hcl").await?;
let session = agent.session("/my-project", Default::default()).await?;
let result = session.send("Find all places where we handle authentication errors").await?;
println!("{}", result.text);
```

---

## What the LLM Can Do

**16 built-in tools** — always available, no configuration:

| Category | Tools |
|----------|-------|
| Files | `read`, `write`, `edit`, `patch` |
| Search | `grep`, `glob`, `ls` |
| Shell | `bash` |
| Web | `web_fetch`, `web_search` |
| Git | `git_worktree` |
| Delegation | `task`, `parallel_task`, `run_team`, `batch`, `Skill` |

**Plugin tools** — opt-in, loaded per session:

| Plugin | Tool | What it does |
|--------|------|--------------|
| `AgenticSearch` | `agentic_search` | Natural-language code search with IDF-weighted relevance ranking |
| `AgenticParse` | `agentic_parse` | LLM-enhanced parsing for PDF, Word, CSV, code, and more |

```python
from a3s_code import Agent, SessionOptions, AgenticSearch, AgenticParse

opts = SessionOptions()
opts.plugins = [AgenticSearch(), AgenticParse()]
session = agent.session(".", opts)
```

---

## Safety and Control

Agents run with **explicit permissions**. Nothing executes by default without a policy allowing it:

```python
from a3s_code import SessionOptions, PermissionPolicy, PermissionRule

opts = SessionOptions()
opts.permission_policy = PermissionPolicy(
    allow=[PermissionRule("read(*)"), PermissionRule("grep(*)")],
    deny=[PermissionRule("bash(*)")],
    default_decision="deny",
)
session = agent.session(".", opts)
```

Other safety features:
- **Human-in-the-loop confirmation** — prompt before any tool call
- **Skill-based tool restrictions** — `allowed-tools` in skill frontmatter limits what the LLM can call
- **AHP integration** — plug in an external harness to block or sanitize tool calls at runtime
- **Auto-compact** — rolls up context before hitting token limits, keeping sessions running

---

## Persistence and Memory

Sessions can be saved and resumed. Memory persists across sessions:

```python
from a3s_code import SessionOptions, FileSessionStore, FileMemoryStore

opts = SessionOptions()
opts.session_store = FileSessionStore('./sessions')
opts.memory_store = FileMemoryStore('./memory')
opts.session_id = 'my-session'
opts.auto_save = True

session = agent.session(".", opts)
resumed = agent.resume_session('my-session', opts)
```

---

## Multi-Provider

One config, any LLM:

```hcl
default_model = "anthropic/claude-sonnet-4-20250514"

providers { name = "anthropic";  api_key = env("ANTHROPIC_API_KEY") }
providers { name = "openai";     api_key = env("OPENAI_API_KEY") }
providers { name = "deepseek";   api_key = env("DEEPSEEK_API_KEY") }
providers { name = "kimi";       api_key = env("MOONSHOT_API_KEY") }
providers { name = "together";   api_key = env("TOGETHER_API_KEY") }
providers { name = "groq";       api_key = env("GROQ_API_KEY") }
```

Switch model per session:

```python
session = agent.session(".", model="openai/gpt-4o")
```

---

## Skills

Skills are markdown files that shape LLM behavior — injected into the system prompt automatically:

```markdown
---
name: safe-reviewer
description: Review code without modifying files
allowed-tools: "read(*), grep(*), glob(*)"
---

Review the code in the workspace. You may read and search files,
but you must not write, edit, or execute anything.
```

```python
opts = SessionOptions()
opts.skill_dirs = ["./skills"]
session = agent.session(".", opts)
```

Built-in skills (enabled via `builtin_skills=True`): `agentic-search`, `code-search`, `code-review`, `explain-code`, `find-bugs`, `builtin-tools`, `delegate-task`, `find-skills`.

---

## Architecture

```
Agent (config + provider registry)
  └── Session (workspace + tools + LLM)
        └── AgentLoop (turn-based execution)
              ├── LlmClient      → sends messages, receives tool calls
              ├── ToolExecutor   → runs tools, enforces permissions
              ├── SkillRegistry  → injects skills into system prompt
              └── PluginManager  → loads opt-in tool+skill bundles
```

20 trait-based extension points: swap any policy, provider, store, or hook without touching core.

---

## Documentation

Full reference, examples, and guides: **[a3s.dev/docs/code](https://a3s.dev/docs/code)**

- [Sessions & Options](https://a3s.dev/docs/code/sessions)
- [Tools](https://a3s.dev/docs/code/tools)
- [Skills](https://a3s.dev/docs/code/skills)
- [Plugin System](https://a3s.dev/docs/code/plugins)
- [Security](https://a3s.dev/docs/code/security)
- [Examples](https://a3s.dev/docs/code/examples)

---

## License

MIT
