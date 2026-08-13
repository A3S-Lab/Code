import assert from 'node:assert/strict'
import fs from 'node:fs'
import os from 'node:os'
import path from 'node:path'
import mod from './index.js'

const tmpRoot = fs.mkdtempSync(path.join(os.tmpdir(), 'a3s-node-retrieval-'))
const workspace = path.join(tmpRoot, 'workspace')
fs.mkdirSync(path.join(workspace, 'src'), { recursive: true })
fs.writeFileSync(
  path.join(workspace, 'src', 'session_cleanup.rs'),
  'pub fn terminate_owned_tasks() {\n    // release every session resource\n}\n',
)

const inlineConfig = `
default_model = "anthropic/claude-sonnet-4-20250514"

providers "anthropic" {
  api_key = "test-key"
  models "claude-sonnet-4-20250514" {
    name = "Claude Sonnet 4"
  }
}
`.trim()

function vectorFor(text) {
  const lower = text.toLowerCase()
  return lower.includes('cleanup') || lower.includes('release every session resource')
    ? [1, 0, 0, 0]
    : [0, 1, 0, 0]
}

let providerCalls = 0
const provider = new mod.CallbackEmbeddingProvider(
  {
    provider: 'node-fixture',
    model: 'deterministic-v1',
    dimension: 4,
    normalization: 'unit',
  },
  async (request) => {
    providerCalls += 1
    assert.equal(request.signal instanceof AbortSignal, true)
    await Promise.resolve()
    return {
      vectors: request.inputs.map((input) => ({ id: input.id, values: vectorFor(input.text) })),
    }
  },
)
const retrieval = new mod.WorkspaceRetrievalOptions(provider)
retrieval.maxRecords = 100
retrieval.maxBytes = 1024 * 1024

const agent = await mod.Agent.create(inlineConfig)
const session = await agent.sessionAsync(workspace, { workspaceRetrieval: retrieval })

try {
  const initial = session.workspaceRetrievalStatus()
  assert.notEqual(initial.phase, 'disabled')

  const deadline = Date.now() + 10_000
  let status = initial
  while (status.phase === 'building' && Date.now() < deadline) {
    await new Promise((resolve) => setTimeout(resolve, 20))
    status = session.workspaceRetrievalStatus()
  }
  assert.equal(status.phase, 'ready', JSON.stringify(status))
  assert.ok(status.indexedChunks > 0)

  const semantic = await session.semanticSearch({ query: 'cleanup session resources', limit: 3 })
  assert.equal(semantic.hits[0].chunk.path, 'src/session_cleanup.rs')
  assert.equal(semantic.hits[0].chunk.digestVerified, true)

  const hybrid = await session.hybridSearch({ query: 'terminate_owned_tasks', limit: 3 })
  assert.equal(hybrid.hits[0].chunk.path, 'src/session_cleanup.rs')
  assert.equal(hybrid.hits[0].exactIdentifier, true)
  assert.ok(hybrid.hits[0].channels.some((channel) => channel.channel === 'exact'))
  assert.ok(providerCalls >= 2)
} finally {
  await session.closeAsync()
}

let providerStarted = false
let observedAbort = false
const slowProvider = new mod.CallbackEmbeddingProvider(
  {
    provider: 'node-fixture',
    model: 'slow-v1',
    dimension: 4,
    normalization: 'unit',
  },
  (request) => new Promise((resolve) => {
    providerStarted = true
    request.signal.addEventListener('abort', () => {
      observedAbort = true
      resolve({ kind: 'cancelled' })
    }, { once: true })
  }),
)
const slowSession = await agent.sessionAsync(workspace, {
  workspaceRetrieval: new mod.WorkspaceRetrievalOptions(slowProvider),
})
const startDeadline = Date.now() + 10_000
while (!providerStarted && Date.now() < startDeadline) {
  await new Promise((resolve) => setTimeout(resolve, 10))
}
assert.equal(providerStarted, true, 'background indexing must call the host provider')
await slowSession.closeAsync()
const abortDeadline = Date.now() + 2_000
while (!observedAbort && Date.now() < abortDeadline) {
  await new Promise((resolve) => setTimeout(resolve, 10))
}
assert.equal(observedAbort, true, 'session close must abort the active host provider request')
assert.equal(slowSession.workspaceRetrievalStatus().phase, 'closed')
await agent.close()
fs.rmSync(tmpRoot, { recursive: true, force: true })

console.log('Node workspace retrieval integration tests passed')
