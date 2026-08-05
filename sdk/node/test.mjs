import assert from 'node:assert/strict'
import mod from './index.js'
import os from 'node:os'
import path from 'node:path'
import fs from 'node:fs'

const requiredExports = [
  'Agent',
  'Session',
  'EventStream',
  'StateGraphRuntime',
  'LocalWorkspaceBackend',
  'builtinSkills',
  'agentEventTypesV1',
  'eventEnvelopeV1Version',
]

for (const name of requiredExports) {
  assert.equal(name in mod, true, `missing export: ${name}`)
}

assert.equal(typeof mod.Agent, 'function', 'Agent export should be a constructor')
assert.equal(
  typeof mod.EventStream.prototype[Symbol.asyncIterator],
  'function',
  'EventStream must implement the JavaScript async-iterator protocol'
)
assert.equal(typeof mod.builtinSkills, 'function', 'builtinSkills should be callable')
assert.equal(mod.eventEnvelopeV1Version(), 1, 'event envelope version should be stable')
const eventTypesV1 = mod.agentEventTypesV1()
assert.equal(new Set(eventTypesV1).size, eventTypesV1.length, 'event types should be unique')
assert.equal(eventTypesV1.includes('agent_start'), true, 'agent_start should be canonical')
assert.equal(eventTypesV1.includes('tool_execution_start'), true, 'execution start should be covered')
assert.equal(eventTypesV1.includes('agent_end'), true, 'agent_end should be canonical')

{
  const graph = new mod.StateGraphRuntime('node-smoke')
  const patch = JSON.stringify({
    expected_graph_version: 0,
    operations: [{ op: 'add_object', id: 'task-1', object_type: 'task', data: { status: 'open' } }],
  })
  assert.equal(graph.proposePatch(patch), true)
  assert.equal(graph.version, 1)
  const restored = mod.StateGraphRuntime.restore(graph.eventsJson())
  const fork = restored.forkAt(JSON.parse(restored.eventsJson()).length)
  assert.notEqual(fork.branchId, restored.branchId)
  assert.deepEqual(JSON.parse(fork.diffJson(restored)), {
    objects_added: [],
    objects_removed: [],
    objects_changed: [],
    relations_added: [],
    relations_removed: [],
    relations_changed: [],
  })
}

const tmpRoot = fs.mkdtempSync(path.join(os.tmpdir(), 'a3s-node-test-'))
const workspace = path.join(tmpRoot, 'workspace')
fs.mkdirSync(workspace, { recursive: true })
const canonicalWorkspace = fs.realpathSync(workspace)

const inlineConfig = `
default_model = "anthropic/claude-sonnet-4-20250514"

providers "anthropic" {
  api_key = "test-key"
  models "claude-sonnet-4-20250514" {
    name = "Claude Sonnet 4"
  }
}
`.trim()

const agent = await mod.Agent.create(inlineConfig)

// A MemorySessionStore is an identity-bearing object, not merely a backend name.
// Reusing the same instance must expose the snapshot written by the first session.
{
  const memoryStore = new mod.MemorySessionStore()
  const sessionId = `memory-store-roundtrip-${Date.now()}`
  const persisted = await agent.sessionAsync(workspace, {
    sessionId,
    sessionStore: memoryStore,
  })
  await persisted.save()
  await persisted.closeAsync()
  await assert.rejects(
    agent.resumeSessionAsync(sessionId, { sessionStore: new mod.MemorySessionStore() }),
    /Session not found/,
    'separately constructed memory stores must remain isolated',
  )
  const resumed = await agent.resumeSessionAsync(sessionId, { sessionStore: memoryStore })
  assert.equal(resumed.sessionId, sessionId, 'memory store identity must survive options conversion')
  const replacement = await agent.replaceSessionAsync(resumed, {
    sessionStore: memoryStore,
    maxContextTokens: 128_000,
  })
  assert.equal(replacement.sessionId, sessionId, 'replacement must preserve the session id')
  await assert.rejects(
    resumed.send('/help'),
    /is closed/,
    'successful replacement must close the previous session object',
  )
  assert.equal(
    (await replacement.send('/help')).text.includes('/help'),
    true,
    'replacement session must remain usable',
  )
  await replacement.closeAsync()

  assert.throws(
    () =>
      agent.session(workspace, {
        sessionStore: { backend: 'memory', instanceId: 'forged-memory-store-handle' },
      }),
    /MemorySessionStore identity is invalid or expired/,
    'unknown memory store handles must fail closed',
  )
}

const session = agent.session(workspace, {
  permissionPolicy: { defaultDecision: 'allow' },
  workspaceBackend: new mod.LocalWorkspaceBackend(workspace),
})
assert.equal(typeof session.spawnRunWithId, 'function', 'exact run admission must be exported')
assert.equal(
  typeof session.spawnRecoveryWithRunId,
  'function',
  'exact recovery admission must be exported',
)

const governedSession = agent.session(workspace, {
  permissionPolicy: { deny: ['write'], defaultDecision: 'allow' },
  workspaceBackend: new mod.LocalWorkspaceBackend(workspace),
})
const trustedWrite = await governedSession.tool('write', {
  file_path: 'trusted-host-write.txt',
  content: 'trusted\n',
})
assert.equal(trustedWrite.exitCode, 0, trustedWrite.output)
assert.equal(
  fs.existsSync(path.join(workspace, 'trusted-host-write.txt')),
  true,
  'tool() should preserve trusted host authority',
)
const deniedGovernedWrite = await governedSession.governedTool('write', {
  file_path: 'denied-governed-write.txt',
  content: 'must not exist\n',
})
assert.notEqual(deniedGovernedWrite.exitCode, 0, deniedGovernedWrite.output)
assert.equal(
  fs.existsSync(path.join(workspace, 'denied-governed-write.txt')),
  false,
  'governedTool() must apply the session permission policy before side effects',
)
await governedSession.closeAsync()

const write = await session.writeFile('notes.txt', 'one\ntwo\n')
assert.equal(write.exitCode, 0, write.output)

const read = await session.readFile('notes.txt')
assert.equal(read.includes('one'), true, 'readFile should read from workspace backend')
const readWindow = await session.readFile('notes.txt', { offset: 1, limit: 1 })
assert.equal(readWindow.includes('two'), true, 'readFile should pass offset/limit to read')
assert.equal(readWindow.includes('one'), false, 'readFile offset should skip earlier lines')

const listing = await session.ls()
assert.equal(listing.exitCode, 0, listing.output)
assert.equal(listing.output.includes('notes.txt'), true, 'ls should list workspace files')

const edit = await session.editFile('notes.txt', 'one', 'uno')
assert.equal(edit.exitCode, 0, edit.output)

const patch = await session.patchFile('notes.txt', '@@ -1,2 +1,2 @@\n uno\n-two\n+dos')
assert.equal(patch.exitCode, 0, patch.output)
assert.equal(fs.readFileSync(path.join(workspace, 'notes.txt'), 'utf8'), 'uno\ndos\n')

const commands = session.listCommands()
assert.equal(Array.isArray(commands), true, 'listCommands() should return an array')
assert.equal(commands.some((cmd) => cmd.name === 'help'), true, 'built-in /help should be registered')

session.registerCommand('status', 'Show session info', (args, ctx) => {
  return `args=${args};workspace=${ctx.workspace};tools=${ctx.toolNames.length}`
})

const updatedCommands = session.listCommands()
assert.equal(updatedCommands.some((cmd) => cmd.name === 'status'), true, 'custom /status should be registered')

const help = await session.send('/help')
assert.equal(help.text.includes('/help'), true, '/help should render command help text')

const helpStream = await session.stream('/help')
const helpEvents = []
for await (const event of helpStream) {
  helpEvents.push(event)
  assert.equal(event.version, 1, 'stream event should use envelope v1')
  assert.equal(typeof event.type, 'string', 'stream event type should remain an open string')
  assert.equal(typeof event.payload, 'object', 'stream event payload should be lossless JSON')
  assert.equal(event.payloadJson, JSON.stringify(event.payload), 'payload JSON should align')
}
assert.equal(helpEvents.at(-1)?.type, 'agent_end', 'terminal wire name should be canonical')
assert.notEqual(helpEvents.some((event) => event.type === 'unknown'), true)

const model = await session.send('/model')
assert.equal(
  model.text.includes('Current model: anthropic/claude-sonnet-4-20250514'),
  true,
  '/model should report the active model'
)

const cost = await session.send('/cost')
assert.equal(cost.text.includes('Model:'), true, '/cost should include model info')
assert.equal(cost.text.includes('Tokens:'), true, '/cost should include token usage')

const history = await session.send('/history')
assert.equal(history.text.includes('Messages:'), true, '/history should include message count')
assert.equal(history.text.includes('Session:'), true, '/history should include session id')

const tools = await session.send('/tools')
assert.equal(tools.text.includes('Tools:'), true, '/tools should summarize registered tools')
assert.equal(tools.text.includes('Builtin'), true, '/tools should list builtin tools')

session.unregisterDynamicTool('dynamic_workflow')
assert.equal(
  session.toolNames().includes('dynamic_workflow'),
  false,
  'dynamic_workflow should be absent before explicit runtime registration'
)
session.registerDynamicWorkflowRuntime()
assert.equal(
  session.toolNames().includes('dynamic_workflow'),
  true,
  'registerDynamicWorkflowRuntime should expose the dynamic_workflow tool'
)
session.unregisterDynamicTool('dynamic_workflow')
assert.equal(
  session.toolNames().includes('dynamic_workflow'),
  false,
  'unregisterDynamicTool should remove dynamic_workflow'
)

const result = await session.send('/status hello world')
assert.equal(result.text.includes('args=hello world;'), true, 'custom slash command should receive args')
assert.equal(
  result.text.includes(`workspace=${canonicalWorkspace};`),
  true,
  'custom slash command should receive workspace in context'
)
assert.match(result.text, /tools=\d+$/, 'custom slash command should receive toolNames in context')

assert.equal(
  await session.runEventPage('missing-run', undefined, 1),
  null,
  'unknown runs should remain distinguishable from empty retained windows',
)
await assert.rejects(
  session.runEventPage('missing-run', 0.5, 1),
  /afterSequence must be a non-negative integer/,
  'fractional cursors must be rejected',
)

// --- Subagent task query API (PR #3): three new Session methods ---
{
  const list = await session.subagentTasks()
  assert.ok(Array.isArray(list), 'subagentTasks() should resolve to an array')
  assert.equal(list.length, 0, 'fresh session should have no subagent tasks')

  const pending = await session.pendingSubagentTasks()
  assert.ok(Array.isArray(pending), 'pendingSubagentTasks() should resolve to an array')
  assert.equal(pending.length, 0, 'fresh session should have no pending subagent tasks')

  const missing = await session.subagentTask('task-does-not-exist')
  assert.equal(missing, null, 'unknown subagent task id should resolve to null')

  const cancelled = await session.cancelSubagentTask('task-does-not-exist')
  assert.equal(cancelled, false, 'cancelling an unknown subagent task id should resolve to false')
}

// --- parallel() budget overload (offline shape check, no LLM) ---
{
  // An empty fan-out takes no LLM path. Without a budget, parallel() returns the
  // plain outcomes array (unchanged behavior).
  const plain = await session.parallel([])
  assert.deepEqual(plain, [], 'no budget -> plain outcomes array')
  // With a budget, parallel() returns { outcomes, budget } (the ledger snapshot).
  const budgeted = await session.parallel([], 50000)
  assert.deepEqual(budgeted.outcomes, [], 'empty specs -> empty outcomes')
  assert.equal(budgeted.budget.consumedTokens, 0, 'no spend yet')
  assert.equal(budgeted.budget.limitTokens, 50000, 'limit reflected in the ledger snapshot')
}

await session.closeAsync()
await assert.rejects(
  session.send('/help'),
  (error) => {
    assert.equal(error.code, 'SESSION_CLOSED', 'core error code should survive the napi boundary')
    assert.equal(error.message.includes('A3S_CODE_ERROR'), false, 'private error marker must be removed')
    return true
  },
)
await assert.rejects(
  session.readFile('notes.txt'),
  (error) => {
    assert.equal(error.code, 'SESSION_CLOSED', 'closed direct tools must fail at the Core gateway')
    return true
  },
)
assert.throws(
  () => session.registerDynamicWorkflowRuntime(),
  (error) => error.code === 'SESSION_CLOSED',
  'closed sessions must reject dynamic capability registration',
)
assert.throws(
  () => session.unregisterDynamicTool('dynamic_workflow'),
  (error) => error.code === 'SESSION_CLOSED',
  'closed sessions must reject dynamic capability removal',
)

// MCP lifecycle methods also cross the same typed Core error boundary. The
// invalid executable fails locally and must retain the stable TOOL_ERROR code.
const mcpSession = await agent.session(process.cwd())
await assert.rejects(
  mcpSession.addMcpServerConfig({
    name: 'missing-local-server',
    transport: {
      type: 'stdio',
      command: '__a3s_code_missing_mcp_executable__',
      args: [],
    },
  }),
  (error) => {
    assert.equal(error.code, 'TOOL_ERROR', 'MCP core error code should survive the napi boundary')
    assert.equal(error.message.includes('A3S_CODE_ERROR'), false, 'private MCP error marker must be removed')
    return true
  },
)
await mcpSession.closeAsync()

console.log('node sdk integration ok')
process.exit(0)
