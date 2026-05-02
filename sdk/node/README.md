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
  const session = agent.session('/my-project')

  const result = await session.send('What files handle authentication?')
  console.log(result.text)
}

main().catch(console.error)
```

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

## Planning Events

Planning is automatic by default. Prefer the explicit tri-state
`planningMode` contract for SDK callers:

```js
agent.session('/my-project', { planningMode: 'auto' })     // default
agent.session('/my-project', { planningMode: 'enabled' })  // force planning
agent.session('/my-project', { planningMode: 'disabled' }) // explicitly off
```

The legacy boolean shortcut still works: `{ planning: true }` forces planning
and `{ planning: false }` disables it.

When streaming, `task_updated` is the authoritative task-list snapshot for UI
rendering. `planning_end` contains the initial plan, while `step_start` and
`step_end` are fine-grained progress events.

## Delegation And Tool Introspection

The SDK exposes the core `task` / `parallel_task` tools as direct helpers:

```js
await session.delegateTask({
  agent: 'explore',
  description: 'Find auth entry points',
  prompt: 'Inspect the repository and summarize the auth-related files.',
})

await session.parallelTask([
  { agent: 'explore', description: 'Find tests', prompt: 'Locate auth tests.' },
  { agent: 'verification', description: 'Check risk', prompt: 'Review auth edge cases.' },
])
```

Use `session.toolNames()` for names and `session.toolDefinitions()` when a UI
needs the full model-visible schemas.

## Run Replay

Each `send(...)` or `stream(...)` call records a run snapshot and replayable
runtime events:

```js
await session.send('Fix the failing test')

const [run] = await session.runs()
console.log(run.id, run.status)
console.log(await session.runEvents(run.id))
```

Use `session.currentRun()` while a stream is active to inspect the current run.
Use `session.cancelRun(run.id)` to cancel only that run; stale IDs will not
cancel a newer operation.
