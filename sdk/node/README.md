# A3S Code — Node SDK

Native Node.js bindings for the A3S Code AI coding agent.

## Installation

```bash
npm install @a3s-lab/code
```

## Quick Start

```js
const { Agent } = require('@a3s-lab/code')

async function main() {
  const agent = await Agent.create('agent.acl')
  const session = await agent.sessionAsync('/my-project')

  const result = await session.send({
    prompt: 'What files handle authentication?',
  })
  console.log(result.text)
}

main().catch(console.error)
```

## Async Lifecycle APIs

Session construction, resume, cancellation, and close may wait for stores,
workspace services, MCP resources, or running children. Prefer the Promise APIs
so those waits never block the JavaScript event loop:

```js
const session = await agent.sessionAsync('/my-project', options)
const resumed = await agent.resumeSessionAsync(sessionId, resumeOptions)
const replacement = await agent.replaceSessionAsync(session, replacementOptions)
const specialist = await agent.sessionForAgentAsync('/my-project', 'explore')
const worker = await agent.sessionForWorkerAsync('/my-project', workerSpec)

await session.cancelAsync()
await session.closeAsync()
```

`replaceSessionAsync()` atomically reconfigures an idle persisted session. A
failed replacement leaves the current object live; a successful replacement
returns the same session ID and closes the previous object.

The synchronous `session()`, `resumeSession()`, `sessionForAgent()`,
`sessionForWorker()`, `cancel()`, and `close()` methods remain available for
compatibility. New event-loop applications should not use them.

## Session Operation Concurrency

A session admits one transcript-affecting operation at a time. `send`, `stream`,
attachment requests, slash commands, and run resumption share a fail-fast gate.
An overlap rejects as a busy session (`CodeError::SessionBusy` in Rust) instead
of waiting in a hidden queue. A stream retains admission until its producer has
stopped, even if the public stream handle is dropped. Finish or cancel the
active operation before starting the next one.

Fully consuming `EventStream` is a lifecycle barrier: the terminal event and
the following `{ done: true }` are not returned ahead of core cleanup, so an
immediate next conversation operation is not rejected because the prior stream
still owns admission.

## Streaming Event Protocol

Every streamed `AgentEvent` carries the stable version-1 envelope fields
`version`, `type`, `payload`, and optional `metadata`. `payload` is the complete
event payload; convenience fields such as `text`, `toolName`, and `exitCode`
are derived from that same core projection. Keep a default branch when
switching on `type`: future event names remain intact instead of becoming
`unknown`, and their payload is still available.

```js
const stream = await session.stream('Explain the current test failures')
let activeTurn
let attemptText = ''
while (true) {
  const { value: event, done } = await stream.next()
  if (!event) break
  if (event.type === 'turn_start') {
    // The same turn number means the provider stream was retried. Discard
    // provisional output from that attempt before accepting replacement deltas.
    activeTurn = event.turn
    attemptText = ''
  } else if (event.type === 'text_delta') {
    attemptText += event.text ?? ''
  } else if (event.type === 'turn_end' && event.turn === activeTurn) {
    process.stdout.write(attemptText)
    attemptText = ''
  }
  if (event.type === 'agent_end') console.log(event.verificationSummaryText ?? '')
  console.debug(event.version, event.type, event.payload, event.metadata)
  if (done) break
}
```

`turn_start` may repeat with the same `turn` when an established provider
stream is interrupted. Treat each turn as provisional until `turn_end`; reset
text, reasoning, and tool-call drafts when that turn restarts.

`agentEventTypesV1()` returns the ordered catalog known by this build, while
the exported `AgentEventTypeV1` TypeScript type deliberately remains open for
forward compatibility.

## Programmatic Tool Calling

`session.program(...)` runs a bounded JavaScript script in the embedded QuickJS
runtime. It is the SDK-friendly wrapper around the core `program` tool.

```js
const result = await session.program({
  source: `
    export default async function run(ctx, inputs) {
      const hits = await ctx.grep(inputs.query, { glob: '*.ts' })
      const files = await ctx.glob('src/**/*.ts')
      return { hits, files: files.slice(0, 10) }
    }
  `,
  inputs: { query: 'PermissionPolicy' },
  allowedTools: ['grep', 'glob'],
  limits: { timeoutMs: 30000, maxToolCalls: 20, maxOutputBytes: 65536 },
})

console.log(result.output)
```

Omit `allowedTools` to allow every registered session tool except `program`.
Scripts can also be loaded from workspace-relative `.js` or `.mjs` files with
`{ path: 'scripts/ptc/search.js' }`.

## Workspace Backends And Direct Files

The default workspace backend is the local filesystem rooted at the session
workspace. SDK callers can pass an explicit typed backend through the same
option surface used by remote, browser, DFS, and container-backed workspaces:

```js
const { Agent, LocalWorkspaceBackend } = require('@a3s-lab/code')

const agent = await Agent.create('agent.acl')
const session = agent.session('/repo', {
  workspaceBackend: new LocalWorkspaceBackend('/repo'),
})

await session.writeFile('notes.txt', 'one\ntwo\n')
await session.readFile('notes.txt')
await session.readFile('notes.txt', { offset: 1, limit: 1 })
await session.ls()
await session.editFile('notes.txt', 'one', 'uno')
await session.patchFile('notes.txt', '@@ -1,2 +1,2 @@\n uno\n-two\n+dos')
```

### S3-compatible object storage

`S3WorkspaceBackend` lets built-in file tools (`read`, `write`, `edit`,
`patch`, `ls`) target any S3-compatible endpoint — AWS S3, MinIO, RustFS,
Cloudflare R2, Backblaze B2, etc. `bash`, `git`, `grep`, and `glob` are
automatically hidden from the model because object storage cannot service
them.

```js
const { Agent, S3WorkspaceBackend } = require('@a3s-lab/code')

const agent = await Agent.create('agent.acl')
const session = agent.session('s3://workspace/users/u1/sessions/s1', {
  workspaceBackend: new S3WorkspaceBackend({
    endpoint: 'https://minio.local:9000',     // omit for AWS S3
    region: 'us-east-1',
    accessKeyId: 'AKIA...',
    secretAccessKey: '...',
    bucket: 'workspace',
    prefix: 'users/u1/sessions/s1',
    forcePathStyle: true,                     // true for MinIO/RustFS/R2
  }),
})

await session.writeFile('notes/hello.txt', 'one\ntwo\n')
await session.readFile('notes/hello.txt')
await session.readFile('notes/hello.txt', { offset: 1, limit: 1 })
await session.ls('notes')
```

S3 has no atomic read-modify-write, so concurrent writers to the same key
overwrite each other (last-writer-wins). Partition workspaces per session or
user via the `prefix` field when running multi-tenant.

## Planning Events

Planning is automatic by default. Prefer the explicit tri-state
`planningMode` contract for SDK callers:

```js
agent.session('/my-project', { planningMode: 'auto' })     // default
agent.session('/my-project', { planningMode: 'enabled' })  // force planning
agent.session('/my-project', { planningMode: 'disabled' }) // explicitly off
```

Common resilience controls are also available directly on `SessionOptions`:

```js
agent.session('/my-project', {
  toolTimeoutMs: 120000,
  llmApiTimeoutMs: 120000,
  circuitBreakerThreshold: 4,
  duplicateToolCallThreshold: 5,
  manualDelegationEnabled: true,
  autoCompact: true,
  autoCompactThreshold: 0.8,
  maxContextTokens: 128000,
})
```

Set `maxContextTokens` to the active model's context window when the model is
not declared in the agent configuration. Rolling auto-compaction can then
compact repeatedly before later requests overflow that window. The bundled
Core selects the retained suffix by token budget and rejects a replacement
that would not reduce estimated history usage.

The legacy boolean shortcut still works: `{ planning: true }` forces planning
and `{ planning: false }` disables it.

For deterministic replay, set both host-environment fields. Recreate the same
options at the beginning of each replay to reset the ID sequence:

```js
const session = await agent.sessionAsync('/my-project', {
  hostEnv: {
    sequentialIdPrefix: 'replay',
    fixedTimeMs: 1700000000000,
  },
})
```

When streaming, `task_updated` is the authoritative task-list snapshot for UI
rendering. `planning_end` contains the initial plan, while `step_start` and
`step_end` are fine-grained progress events.

Planning can also be governed through hooks. `pre_planning` runs before the
planning phase chooses a plan, and a hook can block or modify the planner input:

```js
session.registerHook(
  'planning-policy',
  'pre_planning',
  null,
  null,
  (event) => ({
    action: 'continue',
    modified: {
      modified_task: `${event.payload.task_description}\n\nKeep changes scoped and testable.`,
      selected_strategy: 'step_by_step',
      hints: ['Preserve the original user request'],
    },
  }),
)
```

Use `post_planning` to observe the selected strategy, generated subtasks,
success flag, and planning error text when available.

## Durable Request Shape

`send(...)` and `stream(...)` accept either a prompt string or an object-shaped
request. Use the object shape when the call needs history, attachments, or
future request options:

```js
const result = await session.send({
  prompt: 'Explain the auth module',
  history: previousMessages,
  attachments: [{ data: imageBuffer, mediaType: 'image/png' }],
})
```

`sendRequest(...)`, `streamRequest(...)`, and attachment-specific positional
overloads remain for compatibility.

## Delegation And Tool Introspection

The SDK exposes the core `task` / `parallel_task` tools as direct helpers:

```js
await session.task({
  agent: 'explore',
  description: 'Find auth entry points',
  prompt: 'Inspect the repository and summarize the auth-related files.',
})

await session.tasks([
  { agent: 'explore', description: 'Find tests', prompt: 'Locate auth tests.' },
  { agent: 'verification', description: 'Check risk', prompt: 'Review auth edge cases.' },
])
```

For automatic subagent delegation, `autoParallel: false` disables automatic
parallel fan-out while keeping manual `parallel_task` / `session.tasks(...)`
available:

```js
const session = agent.session('/my-project', {
  autoDelegation: { enabled: true, maxTasks: 4 },
  maxParallelTasks: 8,
  autoParallel: false,
})
```

Use `session.toolNames()` for names and `session.toolDefinitions()` when a UI
needs the full model-visible schemas.

Dynamic workflow is opt-in for SDK sessions. Register it when the host wants the
A3S Flow-backed `dynamic_workflow` tool to join the normal tool registry:

```js
session.registerDynamicWorkflowRuntime()
await session.tool('dynamic_workflow', {
  source: `
    export default async function run(ctx, inputs) {
      if (inputs.kind === 'workflow') {
        return { type: 'complete', output: { text: inputs.input.message } }
      }
      return { type: 'fail', error: 'unexpected step invocation' }
    }
  `,
  input: { message: 'hello from Flow' },
})
session.unregisterDynamicTool('dynamic_workflow')
```

## Evidence And Artifacts

Tool outputs that exceed the inline display budget are retained as session
artifacts. Use `artifactStoreLimits` to tune retention and `getArtifact(...)`
to retrieve retained content by URI:

```js
const session = agent.session('/my-project', {
  artifactStoreLimits: { maxArtifacts: 64, maxBytes: 8 * 1024 * 1024 },
})

const artifact = session.getArtifact('a3s://tool-output/read/abc123')
if (artifact) console.log(artifact.content)
```

External verification systems can attach their reports to the same session
evidence stream:

```js
session.recordVerificationReports([{
  schema: 'a3s.verification_report.v1',
  subject: 'sdk:tests',
  status: 'passed',
  checks: [{
    id: 'check:sdk',
    kind: 'test',
    description: 'Run SDK tests',
    status: 'passed',
    required: true,
  }],
}])
```

## Object-Shaped Direct Tools

New direct helpers use option objects when the command can grow over time:

```js
await session.git({ command: 'status' })
await session.git({ command: 'worktree', subcommand: 'list' })
```

The older positional `git(...)` overload and `gitCommand(...)` remain for
compatibility.

Direct helpers use the trusted host-control-plane policy. They bypass
model-facing permission and HITL decisions because application code selected
the call, but pre/post hooks, budget checks, queue/timeout handling,
cancellation, recursion protection, and security-provider output sanitization
remain active. Authenticate and authorize end users before exposing these
helpers. Direct tools do not mutate transcript history and therefore do not
claim the conversation gate.

When the host coordinates a call but has not already authorized it, use
`governedTool(name, args)`. It follows the same direct path while reapplying
the session permission policy and HITL confirmation gate:

```js
const result = await session.governedTool('write', {
  file_path: 'notes.txt',
  content: 'reviewed content',
})
```

## Disposable Worker Agents

A3S Code treats subagents as cattle, not pets: define reproducible worker specs
in code, register them on a session, and delegate by name through the existing
`task` tool.

```js
const session = agent.session('/my-project', {
  workerAgents: [
    {
      name: 'frontend-cow',
      description: 'Small verified frontend fixes',
      kind: 'implementer',
      model: 'openai/gpt-4o',
      maxSteps: 24,
      prompt: 'Keep patches focused and run the narrowest relevant check.',
      confirmationInheritance: 'auto_approve',  // child runs auto-approve Ask decisions
    },
    { name: 'review-cow', description: 'Adversarial review', kind: 'reviewer' },
  ],
})

await session.task({
  agent: 'frontend-cow',
  description: 'Fix admin chat loading state',
  prompt: 'Find and fix the loading-state regression, then summarize verification.',
})
```

You can also register workers after the session is running:

```js
session.registerWorkerAgent({
  name: 'verify-cow',
  description: 'Run focused checks without editing files',
  kind: 'verifier',
})
```

For a worker as the top-level actor, use
`await agent.sessionForWorkerAsync(workspace, spec)`.

### Confirmation Inheritance

Control how child runs resolve Ask decisions with `confirmationInheritance`:

- `'auto_approve'` (default): Child runs auto-approve all Ask decisions
- `'deny_on_ask'`: Child runs fail immediately when encountering an Ask
- `'inherit_parent'`: Child runs inherit the parent's confirmation policy

```js
const session = agent.session('/my-project', {
  workerAgents: [
    {
      name: 'restricted-writer',
      description: 'Write files with parent confirmation',
      kind: 'implementer',
      confirmationInheritance: 'inherit_parent',  // requires parent approval
    },
  ],
})
```

## Live MCP Servers

Prefer the object-shaped MCP API for new code. It keeps transport-specific
fields grouped and leaves room for OAuth/env/timeout extensions:

```js
await session.addMcp({
  name: 'github',
  transport: {
    type: 'stdio',
    command: 'npx',
    args: ['-y', '@modelcontextprotocol/server-github'],
  },
  env: { GITHUB_TOKEN: process.env.GITHUB_TOKEN ?? '' },
  timeoutMs: 30000,
})

console.log(await session.mcps())
```

The positional `addMcpServer(...)` overload and longer
`addMcpServerConfig(...)` alias remain for compatibility.

Every session owns the manager mutated by `addMcp` and `removeMcp`. Global MCP
configuration is inherited as a read-only capability source: a local server can
shadow it only in this session, and removing the local shadow reveals the
inherited tools again. Sibling sessions and the global manager are unchanged.

## Filesystem-First Agents

Define a durable agent as a **directory** — `instructions.md` (required) plus
optional `agent.acl`, `skills/`, `schedules/` (cron), and `tools/` (`kind: mcp` or
`kind: script` sandboxed QuickJS) — and serve its schedules. Each fire is a full
harness turn (context, tool visibility, safety gate, verification). Returns a
handle you must keep and stop explicitly.

```js
const handle = await agent.serveAgentDir('./my-agent', './workspace', {
  // Optional: pass a sessionStore so each schedule resumes its accumulated
  // context across daemon restarts.
  sessionStore: new FileSessionStore('./sessions'),
})
// ... runs in the background until:
await handle.stop()
console.log(handle.isStopped()) // true
```

## HITL Confirmations

Use `permissionPolicy` to decide which tools ask, then `confirmationPolicy` to
control confirmation runtime behavior such as timeout and YOLO lanes.

```js
const session = agent.session('.', {
  permissionPolicy: { ask: ['bash*'], defaultDecision: 'allow' },
  confirmationPolicy: {
    enabled: true,
    defaultTimeoutMs: 30000,
    timeoutAction: 'reject',
    yoloLanes: ['query'],
  },
})

for (const pending of await session.pendingConfirmations()) {
  await session.confirmToolUse(pending.toolId, true, 'Reviewed')
}
```

For the streaming event-driven loop used by UIs, see
`examples/streaming/hitl_confirmation_loop.ts`.

## Run Replay

Each `send(...)` or `stream(...)` call records a run snapshot and replayable
runtime events:

```js
await session.send('Fix the failing test')

const [run] = await session.runs()
console.log(run.id, run.status)
const replay = await session.runEvents(run.id)
for (const event of replay) {
  console.log(event.version, event.type, event.payload, event.metadata.sequence)
}
console.log(await session.activeTools())
```

`runEvents()` returns the same `{ version, type, payload, metadata }` v1
envelope as live streams. Replay metadata includes `run_id`, `session_id`,
`sequence`, and `timestamp_ms`.

For incremental replay, use the exclusive cursor returned by
`runEventPage()`:

```js
let after
do {
  const page = await session.runEventPage(run.id, after, 256)
  if (!page) break
  if (page.retentionGap) throw new Error('Requested run events were evicted')
  for (const event of page.events) render(event)
  after = page.nextAfterSequence ?? after
  if (!page.hasMore) break
} while (true)
```

`retentionGap` means the requested cursor predates the retained FIFO window;
it must not be treated as complete history.

Use `session.currentRun()` while a stream is active to inspect the current run.
Use `session.cancelRun(run.id)` to cancel only that run; stale IDs will not
cancel a newer operation.

Core failures expose a stable `error.code` (for example `SESSION_BUSY`,
`SESSION_CLOSED`, or `BUDGET_EXHAUSTED`) in addition to the human-readable
message. Match the code instead of parsing message text.

## Persistence Generations

`session.save()` and `autoSave` publish one versioned session generation. The
conversation, artifacts, traces, run records, verification reports, and
subagent task snapshots are committed together as `SessionSnapshotV1`; the
built-in file and memory stores publish the aggregate atomically. Legacy
fragmented records remain readable for migration.
