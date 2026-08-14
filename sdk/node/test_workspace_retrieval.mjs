import assert from 'node:assert/strict'
import fs from 'node:fs'
import os from 'node:os'
import path from 'node:path'
import mod from './index.js'

const chunkingFixture = JSON.parse(fs.readFileSync(
  new URL('../../core/tests/fixtures/workspace-chunking-sdk-v1.json', import.meta.url),
  'utf8',
))

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
const providerInputs = []
const provider = new mod.CallbackEmbeddingProvider(
  {
    provider: 'node-fixture',
    model: 'deterministic-v1',
    dimension: 4,
    normalization: 'unit',
  },
  async (request) => {
    providerCalls += 1
    providerInputs.push(...request.inputs.map((input) => input.text))
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
  assert.ok(status.batching.documentInputs > 0)
  assert.ok(status.batching.documentTextBytes > 0)
  assert.ok(status.batching.batchLimitLowerBound > 0)
  assert.ok(
    status.batching.documentProviderRequests * 10 <=
      status.batching.batchLimitLowerBound * 11,
  )
  assert.equal(status.batching.nonTextInputs, 0)
  assert.ok(status.batching.timeToFirstReadyMs >= 0)

  const semantic = await session.semanticSearch({ query: 'cleanup session resources', limit: 3 })
  assert.equal(semantic.hits[0].chunk.path, 'src/session_cleanup.rs')
  assert.equal(semantic.hits[0].chunk.digestVerified, true)

  const hybrid = await session.hybridSearch({ query: 'terminate_owned_tasks', limit: 3 })
  assert.equal(hybrid.hits[0].chunk.path, 'src/session_cleanup.rs')
  assert.equal(hybrid.hits[0].exactIdentifier, true)
  assert.equal(hybrid.hits[0].rerankScore, hybrid.hits[0].fusedScore)
  assert.equal(hybrid.hits[0].redundancyScore, 0)
  assert.ok(hybrid.hits[0].channels.some((channel) => channel.channel === 'exact'))
  assert.equal(hybrid.rerank.requestedMode, 'rrf_only')
  assert.equal(hybrid.rerank.appliedMode, 'rrf_only')
  assert.equal(hybrid.rerank.algorithm, 'rrf_k60')
  assert.equal(hybrid.rerank.fallback, undefined)
  assert.ok(providerCalls >= 2)
} finally {
  await session.closeAsync()
}

const lineChunking = new mod.LineWorkspaceChunkingStrategy()
assert.ok(lineChunking instanceof mod.LineWorkspaceChunkingStrategy)
const fixedCase = chunkingFixture.cases.find((value) => value.name === 'fixed_window')
const fixedChunking = new mod.FixedWindowWorkspaceChunkingStrategy(
  fixedCase.target_bytes,
  fixedCase.overlap_bytes,
)
assert.equal(fixedChunking.targetBytes, fixedCase.target_bytes)
assert.equal(fixedChunking.overlapBytes, fixedCase.overlap_bytes)
const recursiveCase = chunkingFixture.cases.find((value) => value.name === 'recursive')
const recursiveChunking = new mod.RecursiveWorkspaceChunkingStrategy(
  recursiveCase.target_bytes,
  recursiveCase.overlap_bytes,
  recursiveCase.separators,
)
assert.deepEqual(recursiveChunking.separators, recursiveCase.separators)
assert.doesNotThrow(
  () => new mod.WorkspaceRetrievalOptions(provider, null, lineChunking),
)
assert.doesNotThrow(
  () => new mod.WorkspaceRetrievalOptions(provider, null, recursiveChunking),
)
for (const invalid of chunkingFixture.invalid_windows) {
  assert.throws(
    () => new mod.FixedWindowWorkspaceChunkingStrategy(
      invalid.target_bytes,
      invalid.overlap_bytes,
    ),
    /chunkingStrategy|chunking option/,
  )
}
assert.throws(
  () => new mod.RecursiveWorkspaceChunkingStrategy(64, 0, []),
  /separators/,
)
assert.throws(
  () => new mod.WorkspaceRetrievalOptions(provider, null, 'fixed_window'),
)

const chunkWorkspace = path.join(tmpRoot, 'chunk-workspace')
fs.mkdirSync(chunkWorkspace, { recursive: true })
fs.writeFileSync(path.join(chunkWorkspace, 'fixture.txt'), fixedCase.content)
const inputOffset = providerInputs.length
const chunkSession = await agent.sessionAsync(chunkWorkspace, {
  workspaceRetrieval: new mod.WorkspaceRetrievalOptions(provider, null, fixedChunking),
})
try {
  const deadline = Date.now() + 10_000
  let status = chunkSession.workspaceRetrievalStatus()
  while (status.phase === 'building' && Date.now() < deadline) {
    await new Promise((resolve) => setTimeout(resolve, 20))
    status = chunkSession.workspaceRetrievalStatus()
  }
  assert.equal(status.phase, 'ready', JSON.stringify(status))
  assert.equal(status.indexedChunks, fixedCase.ranges.length)
  const expectedTexts = fixedCase.ranges.map(
    (range) => fixedCase.content.slice(range.start, range.end),
  )
  const actualTexts = providerInputs.slice(inputOffset)
  assert.deepEqual(actualTexts, expectedTexts)
} finally {
  await chunkSession.closeAsync()
}

const reranker = new mod.DeterministicWorkspaceReranker()
assert.equal(reranker.maxCandidates, 100)
assert.equal(reranker.maxFeatureBytesPerCandidate, 4096)
assert.equal(reranker.maxFingerprintsPerCandidate, 128)
assert.equal(reranker.maxScratchBytes, 4 * 1024 * 1024)
const rerankedRetrieval = new mod.WorkspaceRetrievalOptions(provider, reranker)
const rerankedSession = await agent.sessionAsync(workspace, {
  workspaceRetrieval: rerankedRetrieval,
})
try {
  const deadline = Date.now() + 10_000
  let status = rerankedSession.workspaceRetrievalStatus()
  while (status.phase === 'building' && Date.now() < deadline) {
    await new Promise((resolve) => setTimeout(resolve, 20))
    status = rerankedSession.workspaceRetrievalStatus()
  }
  assert.equal(status.phase, 'ready', JSON.stringify(status))
  const hybrid = await rerankedSession.hybridSearch({ query: 'terminate_owned_tasks', limit: 3 })
  assert.equal(hybrid.rerank.requestedMode, 'deterministic')
  assert.equal(hybrid.rerank.appliedMode, 'deterministic')
  assert.equal(hybrid.rerank.algorithm, 'rrf_k60+deterministic_mmr_v1')
  assert.ok(hybrid.rerank.accountedScratchBytes > 0)
  assert.equal(hybrid.rerank.fallback, undefined)
} finally {
  await rerankedSession.closeAsync()
}

const invalidReranker = new mod.DeterministicWorkspaceReranker()
invalidReranker.maxCandidates = 0
const callsBeforeInvalid = providerCalls
assert.throws(
  () => new mod.WorkspaceRetrievalOptions(provider, invalidReranker),
  /rerank\.max_candidates/,
)
assert.equal(providerCalls, callsBeforeInvalid)
assert.throws(() => new mod.WorkspaceRetrievalOptions(provider, 'deterministic'))

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
