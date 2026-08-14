import assert from 'node:assert/strict'
import crypto from 'node:crypto'
import fs from 'node:fs'
import os from 'node:os'
import path from 'node:path'
import mod from './index.js'

const fixture = JSON.parse(fs.readFileSync(
  new URL('../evaluation/workspace-retrieval-deepseek-v1.json', import.meta.url),
  'utf8',
))
assert.equal(fixture.schema_version, 1)
assert.equal(fixture.report_schema_version, 1)
const validateOnly = process.argv.includes('--validate-fixture')
const readyTimeoutMs = 10_000
const turnTimeoutMs = 240_000

function generatedCorpusFiles() {
  const files = fixture.corpus.source_files.map((file) => ({
    path: file.path,
    bytes: Buffer.from(file.content, 'utf8'),
    text: true,
  }))
  for (let index = 0; index < fixture.corpus.unrelated_file_count; index += 1) {
    const padded = String(index).padStart(2, '0')
    files.push({
      path: `src/unrelated_${padded}.rs`,
      bytes: Buffer.from(
        `pub fn unrelated_worker_${padded}(value: usize) -> usize { value + ${index} }\n`,
        'utf8',
      ),
      text: true,
    })
  }
  let boundary = ''
  for (let index = 0; index < fixture.corpus.boundary_filler_lines; index += 1) {
    boundary += `// deterministic chunk-boundary filler ${String(index).padStart(2, '0')}\n`
  }
  boundary += 'pub const MAX_PENDING_EMBED_BATCHES: usize = 8;\n\n'
  boundary += 'pub fn admits_batch(pending: usize) -> bool {\n'
  boundary += '    pending < MAX_PENDING_EMBED_BATCHES\n}\n'
  files.push({
    path: 'src/embedding_admission.rs',
    bytes: Buffer.from(boundary, 'utf8'),
    text: true,
  })
  files.push(...fixture.corpus.non_text_files.map((file) => ({
    path: file.path,
    bytes: Buffer.from(file.content, 'utf8'),
    text: false,
  })))
  return files.sort((left, right) => left.path.localeCompare(right.path))
}

function materializeCorpus(root) {
  const files = generatedCorpusFiles()
  const hash = crypto.createHash('sha256')
  for (const file of files) {
    const destination = path.join(root, ...file.path.split('/'))
    fs.mkdirSync(path.dirname(destination), { recursive: true })
    fs.writeFileSync(destination, file.bytes)
    hash.update(Buffer.from(file.path, 'utf8'))
    hash.update(Buffer.from([0]))
    hash.update(file.bytes)
    hash.update(Buffer.from([0]))
  }
  assert.equal(files.filter((file) => file.text).length, fixture.corpus.text_file_count)
  assert.equal(files.filter((file) => !file.text).length, fixture.corpus.non_text_file_count)
  return hash.digest('hex')
}

function stableBucket(text, buckets) {
  let hash = 2166136261
  for (const byte of Buffer.from(text, 'utf8')) {
    hash ^= byte
    hash = Math.imul(hash, 16777619) >>> 0
  }
  return hash % buckets
}

function vectorFor(id, text) {
  let axis
  if (id === fixture.embedding.query_id) {
    axis = fixture.tasks.findIndex((task) => text.trim() === task.query)
    assert.notEqual(axis, -1, `unexpected evaluation query: ${text}`)
  } else {
    axis = fixture.tasks.findIndex((task) => text.includes(task.expected_identifier))
    if (axis === -1) {
      axis = fixture.tasks.length + stableBucket(
        text,
        fixture.embedding.dimension - fixture.tasks.length,
      )
    }
  }
  const vector = Array(fixture.embedding.dimension).fill(0)
  vector[axis] = 1
  return vector
}

function evaluationProvider(counters) {
  return new mod.CallbackEmbeddingProvider(
    {
      provider: fixture.embedding.provider,
      model: fixture.embedding.model,
      revision: fixture.embedding.revision,
      dimension: fixture.embedding.dimension,
      normalization: 'unit',
    },
    async (request) => {
      assert.equal(request.signal instanceof AbortSignal, true)
      const query = request.inputs.every((input) => input.id === fixture.embedding.query_id)
      const documents = request.inputs.every((input) => input.id !== fixture.embedding.query_id)
      assert.equal(query || documents, true, 'document and query inputs must not share a batch')
      counters.requests += 1
      counters.queryRequests += query ? 1 : 0
      counters.documentRequests += documents ? 1 : 0
      for (const input of request.inputs) {
        counters.inputBytes += Buffer.byteLength(input.text, 'utf8')
        counters.queryInputs += input.id === fixture.embedding.query_id ? 1 : 0
        counters.documentInputs += input.id === fixture.embedding.query_id ? 0 : 1
        counters.nonTextInputs += input.text.includes('NON_TEXT_ASSET_SENTINEL') ? 1 : 0
      }
      await Promise.resolve()
      return {
        vectors: request.inputs.map((input) => ({
          id: input.id,
          values: vectorFor(input.id, input.text),
        })),
      }
    },
  )
}

function percentile(values, percentileValue) {
  if (values.length === 0) return 0
  const sorted = [...values].sort((left, right) => left - right)
  const rank = Math.max(0, Math.ceil(percentileValue * sorted.length) - 1)
  return sorted[rank]
}

function normalizedAnswer(text) {
  return text.trim().replace(/^`+|`+$/g, '').trim()
}

function runEventsToolCalls(events) {
  return events
    .filter((event) => event.type === 'tool_end')
    .map((event) => event.payload)
}

async function withTimeout(promise, label, timeoutMs) {
  let timer
  const timeout = new Promise((_, reject) => {
    timer = setTimeout(() => reject(new Error(`${label} timed out after ${timeoutMs}ms`)), timeoutMs)
  })
  return Promise.race([promise, timeout]).finally(() => clearTimeout(timer))
}

async function waitUntilReady(session) {
  const started = Date.now()
  let status = session.workspaceRetrievalStatus()
  while (status.phase === 'building' && Date.now() - started < readyTimeoutMs) {
    await new Promise((resolve) => setTimeout(resolve, 10))
    status = session.workspaceRetrievalStatus()
  }
  assert.equal(status.phase, 'ready', JSON.stringify(status))
  return { status, elapsedMs: Date.now() - started }
}

function assertReadyStatus(status, counters) {
  assert.equal(status.coverageBps, 10_000)
  assert.equal(status.eligibleFiles, fixture.corpus.text_file_count)
  assert.equal(status.indexedFiles, fixture.corpus.text_file_count)
  assert.equal(status.indexedChunks, fixture.corpus.expected_chunk_count)
  assert.equal(status.failedFiles, 0)
  assert.equal(status.vectorRecords, fixture.corpus.expected_chunk_count)
  assert.equal(status.batching.documentInputs, fixture.corpus.expected_chunk_count)
  assert.equal(status.batching.documentProviderRequests, 1)
  assert.equal(status.batching.batchLimitLowerBound, 1)
  assert.equal(status.batching.nonTextInputs, 0)
  assert.notEqual(status.batching.timeToFirstReadyMs, undefined)
  assert.equal(counters.documentRequests, 1)
  assert.equal(counters.documentInputs, fixture.corpus.expected_chunk_count)
  assert.equal(counters.nonTextInputs, 0)
}

function taskPrompt(task) {
  return `Inspect the search tool schema. Make exactly one search call and no other tool call. Use query exactly: ${task.query}. Set path to '.', include to '*.rs', limit to 5, and mode to 'hybrid'. After the result, return exactly the Rust function or constant declaration name that directly answers the query and is supported by the evidence, or NOT_FOUND when no relevant declaration is present. Never return a path, file stem, module name, prose, or Markdown.`
}

function normalizedBatching(batching) {
  return {
    documentInputs: batching.documentInputs,
    documentTextBytes: batching.documentTextBytes,
    documentBatches: batching.documentBatches,
    documentProviderRequests: batching.documentProviderRequests,
    batchLimitLowerBound: batching.batchLimitLowerBound,
    inputLimitFlushes: batching.inputLimitFlushes,
    textByteLimitFlushes: batching.textByteLimitFlushes,
    vectorByteLimitFlushes: batching.vectorByteLimitFlushes,
    generationCompleteFlushes: batching.generationCompleteFlushes,
    timeToFirstReadyMs: batching.timeToFirstReadyMs,
    nonTextInputs: batching.nonTextInputs,
  }
}

async function runTask(agent, task, ordinal) {
  const workspace = fs.mkdtempSync(path.join(os.tmpdir(), 'a3s-node-wsr-real-'))
  const corpusDigest = materializeCorpus(workspace)
  assert.equal(corpusDigest, fixture.corpus.expected_digest)
  const counters = {
    requests: 0,
    documentRequests: 0,
    queryRequests: 0,
    documentInputs: 0,
    queryInputs: 0,
    inputBytes: 0,
    nonTextInputs: 0,
  }
  const provider = evaluationProvider(counters)
  const chunking = new mod.RecursiveWorkspaceChunkingStrategy(
    fixture.chunking.target_bytes,
    fixture.chunking.overlap_bytes,
    fixture.chunking.separators,
  )
  const reranker = new mod.DeterministicWorkspaceReranker()
  const retrieval = new mod.WorkspaceRetrievalOptions(provider, reranker, chunking)
  const constructionStarted = Date.now()
  const session = await agent.sessionAsync(workspace, {
    sessionId: `wsr-sdk-node-${ordinal}`,
    model: fixture.chat_model,
    planningMode: 'disabled',
    goalTracking: false,
    permissionPolicy: { allow: ['search(*)'], defaultDecision: 'deny' },
    guidelines: 'This is a deterministic repository retrieval evaluation. Follow the requested one-tool protocol exactly. Never guess an identifier that is absent from the tool evidence.',
    maxParseRetries: 1,
    maxToolRounds: 2,
    manualDelegationEnabled: false,
    autoDelegation: { enabled: false },
    temperature: 0,
    workspaceRetrieval: retrieval,
  })
  const sessionConstructionMs = Date.now() - constructionStarted
  try {
    const ready = await waitUntilReady(session)
    assertReadyStatus(ready.status, counters)
    const turnStarted = Date.now()
    const result = await withTimeout(session.send(taskPrompt(task)), task.name, turnTimeoutMs)
    const turnElapsedMs = Date.now() - turnStarted
    const runs = await session.runs()
    assert.equal(runs.length, 1)
    assert.equal(runs[0].status, 'completed')
    const events = await session.runEvents(runs[0].id)
    const calls = runEventsToolCalls(events)
    const call = calls[0]
    const args = call?.args ?? {}
    const metadata = call?.metadata ?? {}
    const results = Array.isArray(metadata.results) ? metadata.results : []
    const expectedIndex = results.findIndex((entry) => entry.path === task.expected_path)
    const expectedPathRank = expectedIndex === -1 ? null : expectedIndex + 1
    const protocolOk = result.toolCallsCount === 1
      && calls.length === 1
      && call.name === 'search'
      && call.exit_code === 0
      && args.query === task.query
      && args.path === '.'
      && args.include === '*.rs'
      && args.limit === 5
      && args.mode === 'hybrid'
    const completionCorrect = normalizedAnswer(result.text) === task.expected_identifier
    assert.equal(protocolOk, true, JSON.stringify(calls))
    assert.equal(completionCorrect, true, result.text)
    assert.notEqual(expectedPathRank, null, JSON.stringify(results))
    assert.equal(metadata.rerank?.requestedMode, fixture.rerank.requested_mode)
    assert.equal(metadata.rerank?.appliedMode, fixture.rerank.requested_mode)
    assert.equal(metadata.algorithm, fixture.rerank.algorithm)
    assert.equal(counters.queryRequests, 1)
    assert.equal(counters.queryInputs, 1)
    const closeStarted = Date.now()
    await session.closeAsync()
    const closeMs = Date.now() - closeStarted
    const closed = session.workspaceRetrievalStatus()
    assert.equal(closed.phase, 'closed')
    assert.equal(closed.vectorRecords, 0)
    assert.equal(closed.vectorBytes, 0)
    return {
      task: task.name,
      completionCorrect,
      toolProtocolOk: protocolOk,
      expectedPathRank,
      resultCount: results.length,
      algorithm: metadata.algorithm,
      rerankRequestedMode: metadata.rerank?.requestedMode,
      rerankAppliedMode: metadata.rerank?.appliedMode,
      sessionConstructionMs,
      indexReadyMs: ready.elapsedMs,
      turnElapsedMs,
      closeMs,
      promptTokens: result.promptTokens,
      completionTokens: result.completionTokens,
      totalTokens: result.totalTokens,
      phase: ready.status.phase,
      coverageBps: ready.status.coverageBps,
      eligibleFiles: ready.status.eligibleFiles,
      indexedFiles: ready.status.indexedFiles,
      indexedChunks: ready.status.indexedChunks,
      vectorRecords: ready.status.vectorRecords,
      vectorBytes: ready.status.vectorBytes,
      batching: normalizedBatching(ready.status.batching),
      provider: counters,
      releasedAfterClose: true,
    }
  } finally {
    if (session.workspaceRetrievalStatus().phase !== 'closed') await session.closeAsync()
    fs.rmSync(workspace, { recursive: true, force: true })
  }
}

function summarize(runs) {
  const ranks = runs.map((run) => run.expectedPathRank)
  const relevant = ranks.filter((rank) => rank !== null && rank <= 5).length
  const returned = runs.reduce((sum, run) => sum + run.resultCount, 0)
  const lowerBound = runs.reduce((sum, run) => sum + run.batching.batchLimitLowerBound, 0)
  const documentRequests = runs.reduce(
    (sum, run) => sum + run.batching.documentProviderRequests,
    0,
  )
  return {
    taskAccuracy: runs.filter((run) => run.completionCorrect).length / runs.length,
    toolProtocolRate: runs.filter((run) => run.toolProtocolOk).length / runs.length,
    precisionAt5: relevant / (runs.length * 5),
    returnedResultPrecision: relevant / returned,
    recallAt5: relevant / runs.length,
    mrr: ranks.reduce((sum, rank) => sum + (rank === null ? 0 : 1 / rank), 0) / runs.length,
    ndcgAt5: ranks.reduce(
      (sum, rank) => sum + (rank === null || rank > 5 ? 0 : 1 / Math.log2(rank + 1)),
      0,
    ) / runs.length,
    documentRequestAmplification: documentRequests / lowerBound,
    meanReturnedResults: returned / runs.length,
    sessionConstructionP50Ms: percentile(runs.map((run) => run.sessionConstructionMs), 0.5),
    sessionConstructionP95Ms: percentile(runs.map((run) => run.sessionConstructionMs), 0.95),
    indexReadyP50Ms: percentile(runs.map((run) => run.indexReadyMs), 0.5),
    indexReadyP95Ms: percentile(runs.map((run) => run.indexReadyMs), 0.95),
    timeToFirstReadyP50Ms: percentile(
      runs.map((run) => run.batching.timeToFirstReadyMs),
      0.5,
    ),
    timeToFirstReadyP95Ms: percentile(
      runs.map((run) => run.batching.timeToFirstReadyMs),
      0.95,
    ),
    turnP50Ms: percentile(runs.map((run) => run.turnElapsedMs), 0.5),
    turnP95Ms: percentile(runs.map((run) => run.turnElapsedMs), 0.95),
    closeP50Ms: percentile(runs.map((run) => run.closeMs), 0.5),
    closeP95Ms: percentile(runs.map((run) => run.closeMs), 0.95),
    totalTokens: runs.reduce((sum, run) => sum + run.totalTokens, 0),
    nonTextProviderInputs: runs.reduce((sum, run) => sum + run.provider.nonTextInputs, 0),
    releasedAfterCloseRate: runs.filter((run) => run.releasedAfterClose).length / runs.length,
  }
}

const validationRoot = fs.mkdtempSync(path.join(os.tmpdir(), 'a3s-node-wsr-fixture-'))
try {
  const digest = materializeCorpus(validationRoot)
  if (fixture.corpus.expected_digest !== 'PENDING') {
    assert.equal(digest, fixture.corpus.expected_digest)
  }
  if (validateOnly) {
    console.log(`Node workspace retrieval fixture validated: ${digest}`)
    process.exitCode = 0
  } else {
    const evaluationRoot = process.env.A3S_REAL_EVAL_ROOT
    assert.ok(evaluationRoot, 'A3S_REAL_EVAL_ROOT must point to the a3s monorepo root')
    const configPath = path.resolve(evaluationRoot, '.a3s', 'config.acl')
    assert.equal(fs.existsSync(configPath), true, 'repository .a3s/config.acl is required')
    const agent = await mod.Agent.create(configPath)
    try {
      const runs = []
      for (const [ordinal, task] of fixture.tasks.entries()) {
        runs.push(await runTask(agent, task, ordinal))
      }
      const summary = summarize(runs)
      assert.equal(summary.taskAccuracy, 1)
      assert.equal(summary.toolProtocolRate, 1)
      assert.equal(summary.recallAt5, 1)
      assert.ok(summary.documentRequestAmplification <= 1.1)
      assert.equal(summary.nonTextProviderInputs, 0)
      assert.equal(summary.releasedAfterCloseRate, 1)
      const report = {
        schemaVersion: fixture.report_schema_version,
        fixtureId: fixture.fixture_id,
        fixtureDigest: fixture.corpus.expected_digest,
        sdk: 'node',
        chatModel: fixture.chat_model,
        chunking: fixture.chunking,
        rerank: fixture.rerank,
        summary,
        runs,
        allGatesPassed: true,
      }
      console.log(`WSR_SDK_DEEPSEEK_EVAL=${JSON.stringify(report)}`)
    } finally {
      await agent.close()
    }
  }
} finally {
  fs.rmSync(validationRoot, { recursive: true, force: true })
}
