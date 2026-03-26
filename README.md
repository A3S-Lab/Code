# A3S Code

**Agentic Agent Framework.** A3S Code is a Rust library with native Python and Node.js bindings. Give an LLM a workspace, a set of tools, and a system prompt — it reads files, runs commands, searches code, and acts on results.

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

---

## What the LLM Can Do

**18 built-in tools** — available in sessions by default:

| Category   | Tools                                                         |
| ---------- | ------------------------------------------------------------- |
| Files      | `read`, `write`, `edit`, `patch`                              |
| Search     | `grep`, `glob`, `ls`                                          |
| Agentic    | `agentic_search`, `agentic_parse`                             |
| Shell      | `bash`                                                        |
| Web        | `web_fetch`, `web_search`                                     |
| Git        | `git_worktree`                                                |
| Delegation | `task`, `parallel_task`, `run_team`, `batch`, `Skill`         |

You can configure the built-in agentic tools from `config.hcl`:

```hcl
agentic_search {
  enabled       = true
  default_mode  = "fast"
  max_results   = 10
  context_lines = 2
}

agentic_parse {
  enabled          = true
  default_strategy = "auto"
  max_chars        = 8000
}

default_parser {
  enabled          = true
  max_file_size_mb = 50

  ocr {
    enabled    = false
    model      = "openai/gpt-4.1-mini"
    max_images = 8
    dpi        = 144
  }
}
```

The `default_parser.ocr` block configures OCR policy. A real OCR / vision backend is supplied at runtime by the host application via `SessionOptions`, so embedded applications can choose their own provider implementation.

## Agentic Document Flow

`agentic_search` and `agentic_parse` are built-in tools. They do not parse PDFs, Office files, or emails themselves. Both tools delegate file decoding to the shared document parser registry, which includes `DefaultParser` by default.

```text
User / LLM
    |
    v
agentic_search / agentic_parse
    |
    v
ToolContext.document_parsers
    |
    v
DocumentParserRegistry
    |
    +--> PlainTextParser
    |       |
    |       `--> text/code/config files
    |
    `--> DefaultParser
            |
            `--> pdf / docx / xlsx / pptx / odt / epub / html / xml / eml / rtf
                    |
                    v
              ParsedDocument
              - title
              - blocks[]
              - block.kind / label / content / location
                    |
        +-----------+-----------+
        |                       |
        v                       v
agentic_search            agentic_parse
- builds search lines     - builds structural summary
- matches/ranks files     - detects parse strategy
- uses block metadata     - sends block-aware context to LLM
```

Responsibility split:

- `DefaultParser`: decode rich documents into a shared structured model.
- `agentic_search`: search and rank over the structured document content.
- `agentic_parse`: summarize, analyze, and answer questions over the structured document content.

---

## Slash Commands

Sessions intercept slash commands before the LLM. Type `/help` in any session:

| Command | Description |
|---------|-------------|
| `/help` | List available commands |
| `/model [provider/model]` | Show or switch the current model |
| `/cost` | Show token usage and estimated cost |
| `/clear` | Clear conversation history |
| `/compact` | Manually trigger context compaction |
| `/tools` | List registered tools |
| `/loop [interval] <prompt>` | Schedule a recurring prompt (default: 10m) |
| `/cron-list` | List scheduled tasks |
| `/cron-cancel <id>` | Cancel a scheduled task |

Register custom commands:

```python
session.register_command("status", "Show status", lambda args, ctx: f"Model: {ctx['model']}")
result = session.send("/status")
```

---

## BTW — Ephemeral Side Questions

Ask a side question without it affecting conversation history:

```python
btw = session.btw("What's the default port for PostgreSQL?")
print(btw.answer)        # "5432"
print(btw.total_tokens)  # token usage for this query only
# main conversation continues — btw question not in history
```

---

## Scheduled Tasks

Schedule recurring prompts via `/loop` or the programmatic API:

```python
# Via slash command
session.send('/loop 5m check if tests are still passing')

# Programmatic
task_id = session.schedule_task('summarize git log since last check', 300)

# List and cancel
tasks = session.list_scheduled_tasks()
session.cancel_scheduled_task(task_id)
```

Interval syntax: `30s`, `5m`, `2h`, `1d`. Max 50 tasks per session; auto-expire after 3 days.

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
- **Circuit breaker** — stops after 3 consecutive LLM failures, prevents infinite retry loops
- **Continuation injection** — prevents the LLM from stopping early mid-task (max 3 continuation turns)

---

## Hooks — Lifecycle Events

Intercept and modify agent behavior at 11 event points:

```python
from a3s_code import SessionOptions, HookHandler

class MyHook(HookHandler):
    def pre_tool_use(self, tool_name, tool_input, ctx):
        if tool_name == "bash" and "rm -rf" in str(tool_input):
            return self.block("Refusing destructive command")
        return self.continue_()

opts = SessionOptions()
opts.hook_handler = MyHook()
session = agent.session(".", opts)
```

Hook events: `PreToolUse` (blockable), `PostToolUse`, `GenerateStart` (modifiable), `GenerateEnd`, `SessionStart/End`, `SkillLoad/Unload`, `PrePrompt`, `PostResponse`, `OnError`.

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

## Multi-Agent

Delegate tasks to subagents or coordinate teams:

```python
# Single subagent
result = session.send('task: explore the codebase and summarize the architecture')

# Parallel tasks
result = session.send('parallel_task: [audit security, check performance, review tests]')

# Agent team (lead decomposes → workers execute → reviewer validates)
result = session.send('run_team: refactor the authentication module')
```

Built-in agent types: `explore` (read-only), `general` (full capabilities), `plan` (analysis only).

---

## Architecture

```
Agent (config + provider registry)
  └── Session (workspace + tools + LLM)
        └── AgentLoop (turn-based execution)
              ├── LlmClient      → sends messages, receives tool calls
              ├── ToolExecutor   → runs tools, enforces permissions
              ├── SkillRegistry  → injects skills into system prompt
              └── PluginManager  → loads optional extension plugins (for example skill bundles)
```

20 trait-based extension points: swap any policy, provider, store, or hook without touching core.

---

## Documentation

Full reference, examples, and guides: **[a3s.dev/docs/code](https://a3s.dev/docs/code)**

- [Sessions & Options](https://a3s.dev/docs/code/sessions)
- [Commands & Scheduling](https://a3s.dev/docs/code/commands) — `/btw`, `/loop`, slash commands
- [Tools](https://a3s.dev/docs/code/tools)
- [Skills](https://a3s.dev/docs/code/skills)
- [Plugin System](https://a3s.dev/docs/code/plugins)
- [Hooks](https://a3s.dev/docs/code/hooks)
- [Security](https://a3s.dev/docs/code/security)
- [Examples](https://a3s.dev/docs/code/examples)

---

## License

MIT
