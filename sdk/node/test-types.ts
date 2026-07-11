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
  ToolResult,
  ReadFileOptions,
  SessionOptions,
  // From extra-types.d.ts (hand-authored):
  ToolErrorKind,
  VerificationStatus,
  VerificationCheck,
  VerificationReport,
  ToolArtifact,
  A3sCodeError,
  A3sCodeErrorCode,
  // From event-protocol-v1.d.ts (generated from the core catalog):
  AgentEventTypeV1,
  AgentEventV1,
  EventEnvelopeV1,
  KnownAgentEventTypeV1,
} from './index.js'

// Forced uses so unused-import lint stays quiet.
declare const _session: Session
declare const _agent: Agent
declare const _event: AgentEvent
declare const _eventStream: EventStream
declare const _result: ToolResult
declare const _readOptions: ReadFileOptions
declare const _sessionOptions: SessionOptions
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
const _busyCode: A3sCodeErrorCode = 'SESSION_BUSY'

void _session.readFile('notes.txt', _readOptions)
void _session.readFile('notes.txt', { offset: 1, limit: 1 })
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
void _agent.session('repo', {
  ..._sessionOptions,
  llmApiTimeoutMs: 30_000,
  duplicateToolCallThreshold: 4,
  manualDelegationEnabled: false,
  retentionLimits: { unbounded: true },
})
void _versionedEvent.payload
void _envelope.payload.opaque
void _knownEventType
void _futureEventType
void _codeError.code
void _busyCode

async function _consumeRunReplay(): Promise<void> {
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
  }
}
