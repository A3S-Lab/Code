# A3S Code

**A harness-driven runtime for coding agents.**

A3S Code is the agent-loop runtime behind `a3s code` and the Rust, Node.js,
and Python SDKs. It lets the harness own the parts a coding agent should not
improvise: context assembly, tool visibility, permissions, delegation,
workspace access, persistence, verification evidence, and event replay.

[![crates.io](https://img.shields.io/crates/v/a3s-code-core)](https://crates.io/crates/a3s-code-core)
[![PyPI](https://img.shields.io/pypi/v/a3s-code)](https://pypi.org/project/a3s-code/)
[![npm](https://img.shields.io/npm/v/@a3s-lab/code)](https://www.npmjs.com/package/@a3s-lab/code)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue)](LICENSE)

## What It Is

A3S Code is a library runtime, not a hosted agent service. The runtime provides
a small, observable execution loop:

```text
prompt
  -> context assembly
  -> optional planning
  -> selected tools / delegated child tasks
  -> permission and confirmation checks
  -> execution
  -> events, artifacts, and verification evidence
  -> compaction and persistence
```

The default session runs against a local workspace. Embedders can replace the
workspace, memory, session store, security provider, LLM client, MCP manager,
hooks, and budget guard with typed objects instead of raw backend strings.

## Capability Map

| Area | Current capability |
| --- | --- |
| Agent API | `Agent` and `AgentSession` expose `send`, `stream`, direct tool calls, run state, cancellation, persistence, and lifecycle cleanup. |
| Config | ACL config files or inline ACL source; provider/model selection; skill and agent directories; storage, search, MCP, and delegation settings. |
| LLM clients | Built-in Anthropic, OpenAI-compatible, and Zhipu-compatible clients, plus `SessionOptions::with_llm_client(...)` for host-supplied clients. |
| Tools | Files, search, shell, git, web fetch/search, batch, structured output, programmatic tool calling, skills, MCP tools, and task delegation. |
| Context | `AGENTS.md`, prompt slots, filesystem context, recent-file/ripgrep providers, memory recall, skills, MCP, and run observations. |
| Safety | Permission policies, human confirmation, workspace path checks, tool timeouts, sandbox handle for `bash`, security providers, and prompt boundary injection. |
| Delegation | Built-in worker roles, custom Markdown/YAML agents, `task`, `parallel_task`, automatic delegation controls, and subagent task tracking. |
| Orchestration | Programmable fan-out, pipelines, resumable checkpoints, workflow phases, loop caps, and shared workflow budget ledgers. |
| Workspaces | Local filesystem by default; typed workspace services for custom hosts; optional S3-compatible backend and HTTP/JSON remote-git backend. |
| Persistence | Memory and file session stores, session IDs, auto-save, run snapshots/events, trace artifacts, memory store integration, and retention caps. |
| Integration | MCP client/manager, AHP hook integration, lane queue options, OpenTelemetry feature flag, Node SDK, and Python SDK. |

## Install

```bash
npm install @a3s-lab/code
pip install a3s-code
cargo add a3s-code-core
```

The Python package is a small bootstrap that downloads the matching native
extension from the release manifest and verifies the downloaded artifact hash.
See the release notes for the current hardening plan and offline-mode details.

## Minimal Config

Use ACL for product configuration. Keep real keys and private base URLs in the
environment; commit templates, not local secrets.

```acl
default_model = "provider/model-id"
max_parallel_tasks = 4
auto_parallel = false

providers "provider" {
  apiKey = env("PROVIDER_API_KEY")
  baseUrl = env("PROVIDER_BASE_URL")

  models "model-id" {
    tool_call = true
    limit = {
      context = 128000
      output = 4096
    }
  }
}

agent_dirs = ["./.a3s/agents"]
skill_dirs = ["./skills"]
storage_backend = "file"

auto_delegation {
  enabled                 = false
  auto_parallel           = false
  allow_manual_delegation = true
  min_confidence          = 0.72
  max_tasks               = 4
}
```

Do not commit `.a3s/config.acl`, local provider URLs, access tokens, API keys,
or real tenant/user identifiers. Prefer `env("...")` in examples and CI.

## Quick Start

### Node.js

```ts
import { Agent } from '@a3s-lab/code';

const agent = await Agent.create('agent.acl');
const session = agent.session('/path/to/workspace', {
  builtinSkills: true,
  planningMode: 'auto',
  permissionPolicy: {
    allow: ['read(*)', 'grep(*)', 'glob(*)'],
    ask: ['bash(*)', 'write(*)'],
    deny: ['write(**/.env*)', 'bash(rm -rf*)'],
    defaultDecision: 'ask',
    enabled: true,
  },
});

const result = await session.send('Find the authentication entry points.');
console.log(result.text);

session.close();
await agent.close();
```

### Python

```python
from a3s_code import Agent, PermissionPolicy, SessionOptions

agent = Agent.create("agent.acl")

opts = SessionOptions()
opts.builtin_skills = True
opts.planning_mode = "auto"
opts.permission_policy = PermissionPolicy(
    allow=["read(*)", "grep(*)", "glob(*)"],
    ask=["bash(*)", "write(*)"],
    deny=["write(**/.env*)", "bash(rm -rf*)"],
    default_decision="ask",
)

session = agent.session("/path/to/workspace", opts)
result = session.send("Find the authentication entry points.")
print(result.text)

session.close()
agent.close()
```

### Rust

```rust
use a3s_code_core::{Agent, AgentEvent, SessionOptions};

# async fn run() -> anyhow::Result<()> {
let agent = Agent::new("agent.acl").await?;
let session = agent.session(
    "/path/to/workspace",
    Some(SessionOptions::new().with_planning(true)),
)?;

let result = session.send("Find the authentication entry points.", None).await?;
println!("{}", result.text);

let (mut rx, _handle) = session.stream("Summarize the test strategy.", None).await?;
while let Some(event) = rx.recv().await {
    match event {
        AgentEvent::TextDelta { text } => print!("{text}"),
        AgentEvent::End { .. } => break,
        _ => {}
    }
}
# Ok(())
# }
```

## Direct Tools

The SDKs expose direct host calls for product code that wants deterministic
tool use without asking the model to choose the tool:

```ts
await session.readFile('src/main.rs');
await session.grep('PermissionPolicy');
await session.glob('**/*.rs');
await session.bash('cargo test -p a3s-code-core');
await session.tool('generate_object', {
  schema: {
    type: 'object',
    required: ['summary'],
    properties: { summary: { type: 'string' } },
  },
  prompt: 'Summarize the current task in one sentence.',
  schema_name: 'task_summary',
});
```

Direct host calls are privileged. Gate them in the embedding application before
exposing them to end users.

## Programmatic Tool Calling

High-frequency tool chains can run inside the embedded QuickJS program tool.
This reduces model round trips while preserving the same tool registry,
permissions, limits, and trace path.

```ts
const result = await session.program({
  source: `
    export default async function run(ctx, inputs) {
      const hits = await ctx.grep(inputs.query, { glob: '*.rs' });
      const files = await ctx.glob('crates/**/*.rs');
      return { hits, files: files.slice(0, 20) };
    }
  `,
  inputs: { query: 'PermissionPolicy' },
  allowedTools: ['grep', 'glob'],
  limits: { timeoutMs: 30000, maxToolCalls: 20, maxOutputBytes: 65536 },
});
```

## Delegation And Orchestration

Model-driven delegation uses `task` and `parallel_task`; host-driven
orchestration uses deterministic SDK calls.

```ts
await session.task({
  agent: 'explore',
  description: 'Find auth entry points',
  prompt: 'Inspect the workspace and return file-level evidence.',
});

const outcomes = await session.parallel([
  { taskId: 'plan', agent: 'plan', description: 'Plan change', prompt: 'Plan the fix.' },
  { taskId: 'review', agent: 'review', description: 'Review risk', prompt: 'Review current diff.' },
]);
```

The orchestration layer includes parallel fan-out, pipelines, resumable
checkpoints, workflow phases, `execute_loop` with a mandatory hard cap, and a
shared workflow token-budget guard. It defines grammar and bookkeeping; a host
platform can still decide placement.

## Workspace Backends

By default, built-in tools operate on the local filesystem. Hosts can pass a
`WorkspaceServices` object so the same tool names target a browser workspace,
remote runner, DFS, object storage, or another controlled environment.

Tool visibility follows backend capabilities: file tools need read/write,
`grep` and `glob` need search, `bash` needs a command runner, and `git` needs a
workspace git provider. With the `s3` Cargo feature, file tools can target an
S3-compatible backend; remote git can be attached separately through the
HTTP/JSON `RemoteGitBackend`.

## Verification And Replay

Every turn can produce typed run snapshots, ordered run events, active-tool
state, verification reports, and compact artifact references. Product UIs and
harnesses should consume those APIs instead of scraping the final answer text.

Useful surfaces include:

```ts
const runs = await session.runs();
const latest = runs.at(-1);
if (latest) {
  console.log(await session.runSnapshot(latest.id));
  console.log(await session.runEvents(latest.id));
  console.log(await session.activeTools());
}
```

## Testing Evidence

The repository contains both hermetic tests and opt-in real-provider tests.
Examples:

```bash
cargo test -p a3s-code-core
cargo test -p a3s-code-core --test test_prompt_boundaries_and_log_redaction
node scripts/docs_api_contract_smoke.mjs
```

Real LLM tests are ignored by default and require explicit provider
configuration through `A3S_CONFIG_FILE` or the local git-ignored
`.a3s/config.acl`:

```bash
A3S_CONFIG_FILE=/path/to/local/config.acl \
  cargo test -p a3s-code-core --test test_real_config_env_integration -- --ignored --nocapture

A3S_CONFIG_FILE=/path/to/local/config.acl \
  cargo test -p a3s-code-core --test test_orchestration_real_llm -- --ignored --nocapture
```

Do not paste real provider values into test commands, test logs, commits, or
pull-request descriptions.

## Documentation

Full guides live in the docs site:

- [A3S Code docs](https://a3s-lab.github.io/a3s/docs/code)
- [API Contract](https://a3s-lab.github.io/a3s/docs/code/api-contract)
- [Sessions](https://a3s-lab.github.io/a3s/docs/code/sessions)
- [Tools](https://a3s-lab.github.io/a3s/docs/code/tools)
- [Providers](https://a3s-lab.github.io/a3s/docs/code/providers)
- [Workspace Backends](https://a3s-lab.github.io/a3s/docs/code/workspace-backends)
- [Orchestration](https://a3s-lab.github.io/a3s/docs/code/orchestration)
- [Security](https://a3s-lab.github.io/a3s/docs/code/security)

## Development

Run commands from this crate workspace, not from the monorepo root:

```bash
cargo fmt --all
cargo test -p a3s-code-core
cargo clippy -p a3s-code-core -- -D warnings
```

Build SDK crates individually when needed:

```bash
cargo build -p a3s-code-node
cargo build -p a3s-code-py
```

## License

MIT
