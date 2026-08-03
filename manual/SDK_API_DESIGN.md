# SDK API Design Contract

This document defines the long-lived SDK API shape for A3S Code. The goal is to
keep the public SDK stable while the runtime, host integrations, and tool
implementations continue to evolve.

## Design Goals

- Keep the kernel small: sessions run prompts, stream events, execute selected
  tools, record evidence, and expose state.
- Keep extension policy outside the kernel: host hooks, UIs, and callers
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

Rust:
Agent.session_builder(workspace).options(options).build().await
Agent.session_async(workspace, options?)
Agent.resume_session_async(session_id, options)
Agent.session_for_agent_async(workspace, definition, options?)
Agent.session_for_worker_async(workspace, worker_spec, options?)

Node / Python facades:
Agent.session(workspace, options?)
Agent.resume_session(session_id, options)
Agent.session_for_agent(workspace, agent_name, agent_dirs?, options?)
Agent.session_for_worker(workspace, worker_spec, options?)
```

Rules:

- Rust construction is async-first. `SessionBuilder::build` and the async
  factories resolve configuration, initialize resources, and call one session
  construction kernel.
- The synchronous Rust `session(...)` factory is compatibility-only. It never
  starts or blocks an async runtime, requires an explicitly pre-initialized
  memory store, accepts only other already-ready resources, and returns
  `AsyncSessionBuildRequired` when configuration still requires asynchronous
  initialization. A manager supplied through `SessionOptions::with_mcp` always
  requires the async path for capability discovery; only already-cached
  agent-global MCP tools can be inherited synchronously.
- Node and Python keep their established factory names and delegate to the async
  core construction path inside their native bindings.
- Worker/subagent setup is data-driven with `WorkerAgentSpec`; callers should not
  create temporary agent files for ephemeral workers.
- Model, memory, session store, security, queue, MCP, and worker choices are
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
- Conversation operations are fail-fast single-flight per session. `send`,
  `stream`, attachment variants, slash commands, and run resumption return
  `SessionBusy` when another operation is active; they are not queued.
- Session closure is a gateway-wide terminal state. Conversation calls, direct
  tools (including event-streaming tool calls), child execution, and live
  capability mutation must return `SessionClosed` before starting new work.
- Immediate local capability changes (dynamic tools, workers/agents, hooks,
  slash commands, and runtime budget guards) share a close admission gate.
  Async lane/MCP mutation uses the async extension gate. Each mutation either
  linearizes before close or reports `SessionClosed`; it cannot report a
  cross-boundary capability update as successful.
- A stream owns admission until its producer finishes, not merely until the
  public stream handle is dropped.

### 3. Deterministic Capabilities

Direct calls bypass model tool selection and are for deterministic product
workflows. A selected tool may still use the configured LLM internally.

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
- Direct calls carry the explicit trusted host-control-plane origin. They skip
  model-facing permission/HITL decisions but still pass through pre/post hooks,
  budget, queue/timeout handling, cancellation, recursion protection, and
  output sanitization. The embedding application owns end-user authorization.
- `governed_tool(name, args)` (`governedTool` in Node and
  `GovernedTool` in Go) is the explicit alternative for host-coordinated
  calls that have not already been authorized. It retains the direct
  no-transcript path while reapplying session permission and HITL gates.
- When output sanitization is enabled, delta events are sanitized over the
  complete consumer-visible concatenation domain rather than independently per
  provider chunk. Text/reasoning may therefore arrive at the run boundary, and
  tool input/output deltas at their corresponding tool boundary. This is an
  intentional security/latency tradeoff: chunk boundaries must not be usable to
  reconstruct a secret. An invocation that buffers more than 8 MiB of delta
  data drops those deltas fail-closed.
- Programmatic Tool Calling scripts are never auto-executed from runtime
  suggestions. Agent-selected programs use the full model policy; direct
  `program(...)` calls and their nested tools inherit the trusted host policy.

### 4. Runtime Observability

Product UIs should build from replayable state rather than parsing text.

Stable calls:

```text
runs()
run_snapshot(run_id)
run_events(run_id)
run_event_page(run_id, after_sequence?, limit?)
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
- SDK stream events use `EventEnvelopeV1 { version, type, payload, metadata }`.
  `type` is an open string, and unknown future events must retain their complete
  payload and metadata. SDK convenience fields are derived from one core
  projection rather than independent language-specific event matches.
- Active tool snapshots are live state for UX only; run events remain the audit
  source of truth.
- Cursor pages report `retention_gap` when the requested sequence predates the
  retained FIFO window. Clients must not interpret such a page as complete
  history. This flag describes retention only; transport-loss gaps require a
  future sequenced event transport.
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
  permission, confirmation, trace, and hook paths as built-in tools.
- Agent-global and host-supplied MCP managers are inherited capability sources,
  never mutation targets for a session. Every session owns a private live
  manager; add/remove affects only that session, and delegated children inherit
  the ordered sources without taking ownership.
- Visible workers are listed by canonical name and purpose in the live
  `task`/`parallel_task` tool definitions. Registration, replacement, and
  removal are reflected on the next run; hidden workers remain callable only
  when the caller already knows their name.
- Dynamic workflow registration exposes the Rust core's A3S Flow-backed
  `dynamic_workflow` tool without requiring SDK callers to construct Rust trait
  objects. Arbitrary host-native dynamic tools remain Rust-only unless a typed
  SDK-safe provider shape is added.
- Runtime budget guards can be installed after session creation because JS/Python
  callbacks cannot always live inside value-typed `SessionOptions`.
- Hooks supervise the existing agent loop; they do not create a second in-core
  advisor runtime.

### 6. Persistence

Session persistence publishes one versioned aggregate generation.

Stable core types and calls:

```text
SessionSnapshotV1
SessionStore.save_snapshot(snapshot)
SessionStore.load_snapshot(session_id)
SessionStore.capabilities()
```

Rules:

- A snapshot contains `SessionData`, artifacts, traces, run records,
  verification reports, and delegated-task snapshots from one save operation.
- File and memory stores publish the aggregate atomically. Historical bare
  session data and fragments remain readable for migration.
- Custom stores must implement aggregate save explicitly. The default method
  errors; it must never acknowledge a fragmented or no-op save as successful.
- Schema validation happens before any persisted state is restored.
- `FileSessionStore` caps each JSON document at 256 MiB on both read and write;
  oversized files are rejected before their contents are allocated.
- Session run/event retention is re-applied during restore. Per-run event count
  and serialized-byte caps share the same FIFO policy; the default byte budget
  is 8 MiB. Trimming never rewrites cumulative `event_count` or the surviving
  event sequence values.

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
- Do not expose a background runtime concept that duplicates caller-owned control-plane
  behavior.
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
