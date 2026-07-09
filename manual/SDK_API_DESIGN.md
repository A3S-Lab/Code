# SDK API Design Contract

This document defines the long-lived SDK API shape for A3S Code. The goal is to
keep the public SDK stable while the runtime, harness integrations, and tool
implementations continue to evolve.

## Design Goals

- Keep the kernel small: sessions run prompts, stream events, execute selected
  tools, record evidence, and expose state.
- Keep extension policy outside the kernel: AHP harnesses, host UIs, and callers
  own background advice, context supplements, and proposed programmatic actions.
- Prefer object-shaped APIs for anything that can grow.
- Keep public method names short. Prefer one-word verbs (`send`, `stream`,
  `task`, `tasks`, `git`) when the receiver already provides the domain.
- Preserve compatibility by adding new stable entry points before deprecating old
  shortcuts.
- Keep raw JSON at system boundaries only; expose typed options and typed
  snapshots everywhere SDK users build product logic.

## API Layers

### 1. Agent Factory

`Agent` owns configuration and creates sessions.

Stable calls:

```text
Agent.create(config_source)
Agent.session(workspace, options?)
Agent.resume_session(session_id, options)
Agent.session_for_agent(workspace, agent_name, agent_dirs?, options?)
Agent.session_for_worker(workspace, worker_spec, options?)
```

Rules:

- `session(...)` remains the default entry point.
- Worker/subagent setup is data-driven with `WorkerAgentSpec`; callers should not
  create temporary agent files for ephemeral workers.
- Model, memory, session store, security, queue, AHP, MCP, and worker choices are
  typed objects in `SessionOptions`, not magic backend strings.

### 2. Session Conversation

`AgentSession` / `Session` is the primary runtime handle.

Stable calls:

```text
send(prompt | { prompt, history?, attachments? }, history?)
run(prompt | { prompt, history?, attachments? }, history?)
stream(prompt | { prompt, history?, attachments? }, history?)
history()
close()/cancel()
```

Rules:

- `send` and `stream` remain prompt-first because prompt is the dominant input.
- Any future optional argument must be added through the request object, not a
  new positional parameter.
- `send_request`, `stream_request`, and attachment-specific methods remain
  compatibility helpers; new examples should use `send({ ... })` and
  `stream({ ... })`.
- Supplying explicit history keeps the operation isolated from session history.

### 3. Deterministic Capabilities

Direct calls bypass the LLM and are for deterministic product workflows.

Stable calls:

```text
tool(name, args)
program(options)
task(options)
tasks(tasks)
git(options)
web_search(options)
read_file(path, options?)
bash(command)
grep(pattern)
glob(pattern)
```

Rules:

- `tool(name, args)` is the raw escape hatch.
- `program`, `task`, `tasks`, `git`, `web_search`, and
  MCP APIs are object-shaped because their schemas can grow.
- Simple scalar helpers (`bash`, `grep`, `glob`) may stay scalar as convenience
  wrappers; richer variants should be added as object-shaped methods instead of
  widening the scalar signatures.
- `read_file` stays path-first for convenience but must expose the underlying
  `read` tool's `offset` / `limit` line window so large files can be paged
  without falling back to `tool("read", ...)`.
- Programmatic Tool Calling scripts are never auto-executed from harness advice;
  callers explicitly run them through `program(...)` so permissions,
  confirmations, and trace recording remain active.

### 4. Runtime Observability

Product UIs should build from replayable state rather than parsing text.

Stable calls:

```text
runs()
run_snapshot(run_id)
run_events(run_id)
current_run()
active_tools()
trace_events()
verification_reports()
verification_summary()
verification_summary_text()
pending_confirmations()
confirm_tool_use(tool_id, approved, reason?)
```

Rules:

- Run snapshots and event records are durable data contracts.
- Active tool snapshots are live state for UX only; run events remain the audit
  source of truth.
- Verification reports are explicit evidence and should not be inferred from the
  final answer text.

### 5. Live Extensions

Live extensions mutate session capabilities while keeping one execution path.

Stable calls:

```text
add_mcp(config)
remove_mcp(name)
mcps()
register_agent_dir(path)
register_worker_agent(worker_spec)
register_worker_agents(worker_specs)
register_dynamic_workflow_runtime()
unregister_dynamic_tool(name)
register_hook(name, event_type, handler, matcher?, config?)
unregister_hook(hook_id)
set_budget_guard(guard?)
```

Rules:

- Prefer `add_mcp(config)` over positional MCP overloads.
- MCP tools join the normal tool registry and go through the same selection,
  permission, confirmation, trace, and AHP hook paths as built-in tools.
- Dynamic workflow registration exposes the Rust core's A3S Flow-backed
  `dynamic_workflow` tool without requiring SDK callers to construct Rust trait
  objects. Arbitrary host-native dynamic tools remain Rust-only unless a typed
  SDK-safe provider shape is added.
- Runtime budget guards can be installed after session creation because JS/Python
  callbacks cannot always live inside value-typed `SessionOptions`.
- Hooks and AHP supervise the existing agent loop; they do not create a second
  in-core advisor runtime.

## Naming Rules

- Rust: snake_case methods and Rust structs.
- Python: snake_case methods, Python dictionaries for extension payloads, typed
  classes for durable option providers.
- Node.js: camelCase methods and typed option objects.
- Event `type` values stay snake_case because they are serialized runtime data.
- Public methods should be as short as clarity allows. If the receiver already
  names the domain, avoid repeating it (`session.git(...)`, not
  `session.git_command(...)`; `session.add_mcp(...)`, not
  `session.add_mcp_server_config(...)`).

## Compatibility Rules

- Add object-shaped replacements before removing positional overloads.
- Keep old overloads documented as compatibility paths only.
- Do not reuse a method name with incompatible semantics.
- Do not expose a background runtime concept that duplicates AHP or caller-owned
  harness behavior.
- Do not add model-visible shortcuts that bypass `task`, `parallel_task`,
  `program`, permissions, confirmations, or trace recording.

## Recommended API Direction

Prefer this shape for new public calls:

```typescript
await session.send({ prompt: 'Explain auth', history, attachments })
await session.git({ command: 'worktree', subcommand: 'list' })
await session.addMcp({
  name: 'github',
  transport: { type: 'stdio', command: 'npx', args: ['-y', '@modelcontextprotocol/server-github'] },
  timeoutMs: 30000,
})
await session.program({ source, inputs, allowedTools, limits })
await session.task({ agent: 'explore', description: 'Find auth', prompt: 'Summarize auth files.' })
```

Avoid this shape for new calls:

```typescript
await session.sendWithAttachments('Explain auth', attachments, history)
await session.git('worktree', 'list', null, null, true, null, false, null, null, false)
await session.addMcpServer('github', 'stdio', 'npx', ['-y', '@modelcontextprotocol/server-github'])
```

The positional forms may remain for compatibility, but new SDK examples should
use object-shaped methods.
