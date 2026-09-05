// Compile-only check that both generated and hand-authored types resolve
// from the package entry. If the napi-rs build ever overwrites index.d.ts
// (i.e. the aggregator was lost), this file will fail to type-check.
//
// Run with: npx tsc --noEmit --module nodenext --moduleResolution nodenext test-types.ts

import type {
  // From generated.d.ts (napi-rs):
  Session,
  Agent,
  AgentEvent,
  EventStream,
  ServeHandle,
  StateGraphRuntime,
  ToolResult,
  AgentRunSpawnObject,
  ReadFileOptions,
  SessionOptions,
  ToolPresentationProfile,
  TaskSchedulerStats,
  MemoryMaintenanceHealth,
  CallbackEmbeddingProvider,
  DeterministicWorkspaceReranker,
  LineWorkspaceChunkingStrategy,
  FixedWindowWorkspaceChunkingStrategy,
  RecursiveWorkspaceChunkingStrategy,
  WorkspaceRetrievalOptions,
  WorkspaceRetrievalStatusObject,
  WorkspaceSemanticSearchResultObject,
  WorkspaceHybridSearchResultObject,
  WorkspaceRerankStatusObject,
  // From extra-types.d.ts (hand-authored):
  ToolErrorKind,
  VerificationStatus,
  VerificationCheck,
  VerificationReport,
  ToolArtifact,
  A3sCodeError,
  A3sCodeErrorCode,
  EmbeddingBatchRequest,
  EmbeddingBatchResponse,
  // From event-protocol-v1.d.ts (generated from the core catalog):
  AgentEventTypeV1,
  AgentEventV1,
  EventEnvelopeV1,
  KnownAgentEventTypeV1,
  // From evaluation-protocol-v1.d.ts (generated from the Rust catalog):
  EvaluationWireEnvelopeV1,
  EvaluationWireMessageV1,
  EvaluationWireKindV1,
  KnownEvaluationWireKindV1,
} from './index.js'
import {
  EVALUATION_WIRE_KINDS_V1,
  EvaluationWireTypeV1,
  ToolPresentationMode,
  WorkspaceLexicalEngineOption,
} from './index.js'

// Forced uses so unused-import lint stays quiet.
declare const _session: Session
declare const _agent: Agent
declare const _event: AgentEvent
declare const _eventStream: EventStream
declare const _serveHandle: ServeHandle
declare const _stateGraph: StateGraphRuntime
declare const _result: ToolResult
declare const _runSpawn: AgentRunSpawnObject
declare const _readOptions: ReadFileOptions
declare const _sessionOptions: SessionOptions
const _toolPresentationProfile: ToolPresentationProfile = {
  schema: 'a3s.code.tool-presentation-profile.v1',
  mode: ToolPresentationMode.Code,
}
const _sessionOptionsWithProfile: SessionOptions = {
  toolPresentationProfile: _toolPresentationProfile,
}
const _primitivePresentationProfileIsRejected: 'code' extends NonNullable<
  SessionOptions['toolPresentationProfile']
>
  ? false
  : true = true
declare const _schedulerStats: TaskSchedulerStats
declare const _memoryMaintenanceHealth: MemoryMaintenanceHealth
declare const _embeddingProvider: CallbackEmbeddingProvider
declare const _deterministicReranker: DeterministicWorkspaceReranker
declare const _lineChunking: LineWorkspaceChunkingStrategy
declare const _fixedChunking: FixedWindowWorkspaceChunkingStrategy
declare const _recursiveChunking: RecursiveWorkspaceChunkingStrategy
declare const _retrievalOptions: WorkspaceRetrievalOptions
const _lexicalEngineOption: WorkspaceLexicalEngineOption =
  WorkspaceLexicalEngineOption.ZvecRust
declare const _retrievalStatus: WorkspaceRetrievalStatusObject
declare const _semanticResult: WorkspaceSemanticSearchResultObject
declare const _hybridResult: WorkspaceHybridSearchResultObject
declare const _rerankStatus: WorkspaceRerankStatusObject
declare const _embeddingRequest: EmbeddingBatchRequest
declare const _embeddingResponse: EmbeddingBatchResponse
type _WorkspaceRetrievalConstructorArgs = ConstructorParameters<
  typeof import('./index.js').WorkspaceRetrievalOptions
>
declare const _workspaceRetrievalConstructorArgs: _WorkspaceRetrievalConstructorArgs
const _rerankerArgument: DeterministicWorkspaceReranker | null | undefined =
  _workspaceRetrievalConstructorArgs[1]
const _chunkingArgument:
  | LineWorkspaceChunkingStrategy
  | FixedWindowWorkspaceChunkingStrategy
  | RecursiveWorkspaceChunkingStrategy
  | null
  | undefined = _workspaceRetrievalConstructorArgs[2]
const _lexicalEngineArgument: WorkspaceLexicalEngineOption | null | undefined =
  _workspaceRetrievalConstructorArgs[3]
const _primitiveRerankerIsRejected: 'deterministic' extends NonNullable<
  _WorkspaceRetrievalConstructorArgs[1]
>
  ? false
  : true = true
const _primitiveChunkingIsRejected: 'recursive' extends NonNullable<
  _WorkspaceRetrievalConstructorArgs[2]
>
  ? false
  : true = true
declare const _err: ToolErrorKind
declare const _status: VerificationStatus
declare const _check: VerificationCheck
declare const _report: VerificationReport
declare const _artifact: ToolArtifact
declare const _codeError: A3sCodeError
declare const _versionedEvent: AgentEventV1
declare const _envelope: EventEnvelopeV1<{ opaque: Array<number> }>
const _knownEventType: KnownAgentEventTypeV1 = 'tool_execution_start'
const _futureEventType: AgentEventTypeV1 = 'future_event'
const _evaluationEnvelope: EvaluationWireEnvelopeV1 = {
  schema: 'a3s.code.evaluation-wire.v1',
  version: 1,
  kind: 'evidence_snapshot',
  payload: { snapshot_digest: 'sha256:' + 'a'.repeat(64) },
}
declare const _evaluationMessage: EvaluationWireMessageV1
const _evaluationKind: EvaluationWireKindV1 = 'evaluation_record'
const _knownEvaluationKind: KnownEvaluationWireKindV1 = 'evidence_snapshot'
void _evaluationEnvelope
void _evaluationMessage
void _evaluationKind
void _knownEvaluationKind
void EvaluationWireTypeV1.EVALUATION_RECORD
void EVALUATION_WIRE_KINDS_V1
const _busyCode: A3sCodeErrorCode = 'SESSION_BUSY'
const _serveFailureCode: A3sCodeErrorCode = 'SERVE_STARTUP_FAILED'

void _session.readFile('notes.txt', _readOptions)
void _session.readFile('notes.txt', { offset: 1, limit: 1 })
void _session.governedTool('read', { file_path: 'notes.txt' })
void _session.registerDynamicWorkflowRuntime()
void _session.unregisterDynamicTool('dynamic_workflow')
void _session.sessionId
void _session.workspace
void _session.initWarning
void _session.tenantId
void _session.principal
void _session.agentTemplateId
void _session.correlationId
void _session.hasMemory
void _session.taskSchedulerStats()
void _session.taskSchedulerHealth()
void _session.memoryMaintenanceHealth()
void _session.workspaceRetrievalStatus()
void _session.semanticSearch({ query: 'session cleanup', limit: 5 })
void _session.hybridSearch({ query: 'terminate_owned_tasks', path: 'src' })
void _retrievalOptions.maxRecords
void _lexicalEngineOption
void _lexicalEngineArgument
void _deterministicReranker.maxCandidates
void _rerankerArgument
void _primitiveRerankerIsRejected
void _retrievalStatus.coverageBps
void _semanticResult.hits[0]?.chunk.digestVerified
void _hybridResult.channels[0]?.channel
void _hybridResult.hits[0]?.rerankScore
void _hybridResult.rerank.accountedScratchBytes
void _hybridResult.rerank.algorithm
void _rerankStatus.appliedMode
void _embeddingRequest.inputs[0]?.text
void _embeddingResponse.vectors[0]?.values
void _embeddingProvider
void _agent.taskSchedulerStats()
void _agent.taskSchedulerHealth()
void _schedulerStats.activeByPriority.interactive
void _memoryMaintenanceHealth.jobs[0]?.lastAffectedItems
void _agent.session('repo', {
  ..._sessionOptions,
  llmApiTimeoutMs: 30_000,
  duplicateToolCallThreshold: 4,
  manualDelegationEnabled: false,
  retentionLimits: { unbounded: true },
})
void _serveHandle.isReady()
void _serveHandle.state()
void _serveHandle.failureCode()
void _serveHandle.isStopped()
void _versionedEvent.payload
void _envelope.payload.opaque
void _knownEventType
void _futureEventType
void _codeError.code
void _runSpawn.snapshot
void _runSpawn.replayed
void _busyCode
void _serveFailureCode

async function _consumeRunReplay(): Promise<void> {
  const started: AgentRunSpawnObject = await _session.spawnRunWithId('run-1', 'inspect')
  void started.snapshot
  void started.replayed
  const recovered = await _session.spawnRecoveryWithRunId('checkpoint-1', 'run-2')
  void recovered.snapshot
  void recovered.replayed
  const events = await _session.runEvents('run-1')
  const event = events[0]
  if (event) {
    const _version: 1 = event.version
    void _version
    void event.type
    void event.payload
    void event.metadata.run_id
    void event.metadata.session_id
    void event.metadata.sequence
    void event.metadata.timestamp_ms
  }
  const page = await _session.runEventPage('run-1', undefined, 100)
  if (page) {
    void page.events
    void page.firstAvailableSequence
    void page.latestSequenceExclusive
    void page.nextAfterSequence
    void page.retentionGap
    void page.hasMore
  }
}

async function _consumeAsyncLifecycle(): Promise<void> {
  const session = await _agent.sessionAsync('.')
  void (await session.cancelAsync())
  await session.closeAsync()
  const resumed = await _agent.resumeSessionAsync('session-1', {})
  await resumed.closeAsync()
  const replacement = await _agent.replaceSessionAsync(session, {})
  await replacement.closeAsync()
  const serveHandle = await _agent.serveAgentDir('./agent', '.', {})
  await serveHandle.stop()
}

void _consumeAsyncLifecycle

void _consumeRunReplay

async function _consumeEventStream(): Promise<void> {
  for await (const event of _eventStream) {
    const typed: AgentEvent = event
    void typed
  }
}

void _consumeEventStream

// Exhaustive narrowing on the discriminated union — confirms the union
// shape survives a regenerate.
function _describe(err: ToolErrorKind): string {
  switch (err.type) {
    case 'version_conflict':
      return `${err.path} expected=${err.expected}`
    case 'remote_git_conflict':
      return err.message
    case 'not_found':
      return err.path
    case 'invalid_argument':
    case 'unsupported':
      return err.message
    case 'timeout':
      return `${err.op} after ${err.duration_ms}ms`
    case 'transport':
    case 'cancelled':
      return err.op
    case 'partial_failure':
      return `${err.failed}/${err.total} failed`
    case 'rate_limited':
      return err.retry_after_ms === null ? 'rate limited' : `retry after ${err.retry_after_ms}ms`
    case 'hook_denied':
      return err.retryable && err.retry_after_ms !== null
        ? `retry after ${err.retry_after_ms}ms`
        : err.reason
  }
}
