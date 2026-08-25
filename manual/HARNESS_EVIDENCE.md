# Harness Boundary Evidence

Status: the digest-only Tool-request, capability, Tool-presentation,
model-input, and client-reported usage diagnostic slices are delivered. The
Code-owned `CAR-02` host port for authorized immutable original-content
references is also delivered; Cloud authorization, managed projection, object
lifecycle, and cross-repository conformance remain outside this component. The
Code-owned `CAR-04` slices also provide canonical portable Session checkpoint
payloads, acknowledged live-boundary export, complete-descriptor exact
recovery, and one Harness-visible restore admission without split store
prewrites. Cloud checkpoint identity, storage authorization, external revision
fencing, retention, approval, lineage, and provider certification remain
external.

## Purpose

A configured Agent is an intention. An auditable run needs evidence of what the
model could actually use, what arguments reached the provider-neutral model
boundary, and what validated Tool requests entered governance. A3S Code records
that evidence in the existing Run event journal; it does not introduce a second
audit database or a provider-specific request model.

Five versioned events provide the boundary:

| Event | Emission | Purpose |
| --- | --- | --- |
| `tool_request_bound` | After pre-tool hooks and argument validation, before permission, confirmation, budget, or execution outcomes | Bind the Tool correlation identity, request origin, serialized argument bytes, and exact post-hook arguments without retaining argument plaintext |
| `run_capability_bound` | Before the first model call and again when its digest changes | Bind model-visible tools, workspace services, run-owned governance bindings, serializable policy identities, execution ceilings, and current semantic generation |
| `model_presentation_bound` | Before every model input event | Bind the frozen Profile identity, permission-filtered source definition cost, application kind, and exact definitions submitted to the model |
| `model_input_bound` | Before every model call | Bind bounded counters and digests for the actual provider-neutral input |
| `model_usage_bound` | After every successful model call and before its response is released | Bind the input estimate and normalized `LlmClient` token/cache usage to the exact input snapshot |

The same path covers completion, streaming, structured completion, and
structured streaming. Higher-level retries, repair calls, compaction, and
helper calls that re-enter the run-scoped `LlmClient` pass through this boundary
as well; an internal transport retry remains part of its owning model call.
Work that explicitly detaches from the Run event channel, such as post-terminal
background memory extraction, receives a separate auxiliary invocation and
cannot append evidence after the Run's terminal event.

For one call the causal order is `run_capability_bound` when its digest changed,
then `model_presentation_bound`, `model_input_bound`, and finally
`model_usage_bound` after a successful provider result. Concurrent calls may
interleave, so consumers still correlate every per-call event by call sequence.

For one Tool call the causal order starts with `tool_request_bound`. An approved
call later emits `tool_execution_start` and `tool_end`; a policy rejection emits
permission evidence without an execution-start event. Budget rejection also
occurs after the request is bound and before execution. Nested orchestrators and
trusted or governed host-direct calls use the same capture path with distinct
typed origins.

## Capability snapshot

`RunCapabilitySnapshotV1` uses schema
`a3s.code.run-capability-snapshot.v1`. It records:

- the count and digest of the actual `ToolDefinition` list visible to the model;
- read, write, execution, search, Git, and Code Intelligence availability from
  the bound `WorkspaceServices`;
- whether permission, confirmation, and budget governance are bound, digests of
  configured permission/confirmation policy, and frozen execution limits;
- whether semantic retrieval exists, its phase, coverage, catalog/source/vector
  revisions, and a digest of its non-sensitive model descriptor; and
- a domain-separated digest over the complete snapshot except that digest
  field itself.

Capability emission is deduplicated inside an asynchronous critical section.
Concurrent model calls receive unique positive call sequences whose journal
arrival order need not be numeric, but they cannot publish duplicate capability
events for the same observed digest.
Consumers correlate an input with `capabilitySnapshotDigest`; they should not
depend on event adjacency.

`HARNESS-SCOPE1` leaves this strict v1 wire shape unchanged. The internal
`CapabilityScope` kernel now pins one immutable catalog, a monotonic governance
ceiling, and the exact A3S Use Run lease while borrowed marker-specific leases
control access. Agent orchestration, provider/Tool work, recursive Skill/Task
children, and promoted Task/memory work now execute inside the corresponding
Turn/Subtask/Run lifetime, but existing evidence remains derived from the live
governed executor during migration. A future separately versioned scope
snapshot may expose the internal identity; fields are not appended to v1.

## Tool-request snapshot

`ToolRequestSnapshotV1` uses schema
`a3s.code.tool-request-snapshot.v1`. It records a closed request origin, the
canonical serialized argument byte count, and separate domain-separated
SHA-256 digests for the Tool call identifier, Tool name, arguments, and complete
snapshot. Origins distinguish model, nested, trusted host-direct, governed
host-direct, and both host-direct nested paths.

The surrounding `tool_request_bound` event retains `tool_id` and `tool_name` for
lifecycle correlation. The snapshot binds both values by digest but never
copies them or the arguments. `validate_against()` verifies the event
correlation fields, exact post-hook JSON arguments, and expected origin.

The original arguments must pass the Tool schema before a pre-tool hook runs.
If a hook replaces them, the replacement is validated again and the snapshot
binds that final value. Capture occurs before the permission checker or trusted
host decision, so a valid request remains auditable even when a hook or policy
denies it. Invalid arguments that never enter governance do not emit this
event. `tool_execution_start` remains the authoritative lifecycle signal that
an approved call actually began execution.

## Model-presentation snapshot

`ModelPresentationSnapshotV1` uses schema
`a3s.code.model-presentation-snapshot.v1`. It binds one closed
`ToolPresentationProfileV1` value to the canonical Tool-definition source and
the exact provider-facing projection for a call. It records source and
presented counts, domain-separated definition digests, Code token estimates,
the positive call sequence, and one application kind:

- `profiled` means the main agent turn applied the Session's frozen Profile;
- `auxiliary` means a host-owned helper protocol supplied its own Tool list, so
  source and presented count, digest, and token estimate must be identical.

The version-1 Profile has four modes:

| Mode | Model-facing result |
| --- | --- |
| `adaptive` | Preserve the historical prompt-sensitive selector over the permission-visible source |
| `direct` | Present every permission-visible definition in canonical Tool-name order |
| `code` | Present only the existing `program` definition, with its parameter schema unchanged and a bounded compact catalog in its description |
| `disabled` | Present no Tool definitions |

Permission visibility is evaluated before the Profile. Consequently, code-mode
catalog generation cannot restore or name a permission-hidden Tool. A Profile
owns no Tool value and cannot add a name, alter a parameter schema, select an
A3S Use generation, or replace the governed executor. The main turn executes
through the same Run-owned `ToolExecutor` and pinned `Arc<dyn Tool>` values.

The Profile is persisted in the Session snapshot and an explicit different
Profile is rejected during resume. Delegated child runs inherit the parent
Profile exactly in this version; the public partial-order check also rejects a
future child mode that would broaden its parent. Node.js, Python, and Go expose
typed Profile objects rather than a primitive backend selector.

## Model-input snapshot

`ModelInputSnapshotV1` uses schema `a3s.code.model-input-snapshot.v1` and a
positive, run-local call sequence. It records counts and canonical serialized
byte measurements for messages, content blocks, images, Tool results, Tool
definitions, structured-output directives, and the complete provider-neutral
payload. Separate domain-separated SHA-256 values bind:

- messages and reasoning content;
- the optional system input;
- actual model-visible Tool definitions;
- provider-facing structured-output intent;
- the complete provider-neutral input; and
- Tool results whose preceding Tool call is `semantic`, `hybrid`, or `search`
  with `mode` equal to `semantic` or `hybrid`.

Retrieval evidence includes only a result count, canonical serialized byte
measurement, and digest. It does not copy a query or returned source into the
new event. `StructuredDirective.validation_schema` is deliberately excluded:
it is host-only validation metadata and is not transmitted to a provider.

## Model-usage snapshot

`ModelUsageSnapshotV1` uses schema `a3s.code.model-usage-snapshot.v1`. It binds
the positive call sequence, input snapshot digest, and Code prompt-token
estimate to the prompt, completion, total, cache-read, and cache-write values
returned through `LlmClient::TokenUsage`. A zero remains zero when an adapter
cannot observe a value; Core does not synthesize provider or billing usage.

The usage snapshot also measures all Tool-result content visible in that call.
It records total, unique, and repeated result counts; total and repeated
canonical content bytes; total and repeated token estimates; and ordered
aggregate digests for all and repeated results. Repetition is based on exact
content, not Tool call identity, so the same content returned under different
call IDs is still counted as redundant context. Only occurrences after the
first are included in the repeated counters. These are per-call diagnostics;
consumers may compare snapshots, but Core does not infer cross-call savings.

For non-streaming calls, usage is journaled before the response returns. For a
streaming call, it is journaled before `StreamEvent::Done` reaches the caller.
Concurrent calls can finish out of sequence, so consumers must correlate by
call sequence and input snapshot digest rather than event adjacency.

## Immutable original-content adapter

The immutable-content boundary is not another evidence event or a storage
backend selected by Code. A Rust host constructs an
`ImmutableContentAdapterBindingV1` from an opaque, secret-free authority digest
and a positive per-object byte ceiling, pairs it with its already-authorized
`ImmutableContentAdapter`, and injects the resulting
`ImmutableContentAdapterSession` through `SessionOptions`.

When that session is configured, Code submits every raw output returned by a
Tool to the adapter before releasing its bounded model-facing projection. If
large `before` or `after` values are removed by change-metadata compaction,
those sides are submitted separately. Code computes and validates:

- the closed content kind, exact UTF-8 media type, byte count, SHA-256 content
  digest, and domain-separated descriptor digest;
- the exact session binding digest and host-pinned byte ceiling; and
- the provider-neutral absolute logical URI, which must contain the exact
  content SHA-256 and cannot contain userinfo, a query, a fragment, whitespace,
  controls, or backslashes, plus a domain-separated reference digest over all
  reference fields.

The adapter request borrows the original bytes and its `Debug` representation
redacts them. Provider error detail is not released into Tool errors. A byte
ceiling violation, provider failure, cancellation, malformed reference, or any
binding/content/reference drift fails the Tool call closed. Once an adapter is
configured, Code never silently writes that content to the local
`ArtifactStore` as a fallback.

The complete reference is placed under Tool metadata
`artifact.content_reference`; `a3s_tool_result_evidence.content_ref` contains
the same URI. A lossless bounded result remains visible to the model unchanged.
For a lossy result, the reference shown in the bounded projection is rewritten
from the compatibility URI to the adapter's validated URI. The session
snapshot persists only the secret-free binding, never the trait object or
provider credentials. Resume requires the host to re-inject an adapter with
that exact binding, and delegated children inherit the same session adapter.

Without a configured adapter, standalone compatibility is unchanged: only
lossy originals and compacted change sides use the bounded session-local
`ArtifactStore`, which can be included in `SessionSnapshotV1`. That store is
not a shared authorization, retention, or object-lifecycle authority. Node.js,
Python, and Go intentionally do not accept a primitive backend name for this
Rust host callback; an official managed-host surface requires a separately
typed, authorized adapter contract.

## Tool-result transform binding

An executed Tool result carries a separate
`metadata.a3s_tool_result_transform_binding` object using schema
`a3s.code.tool-result-transform-binding.v1`. Keeping this object beside, rather
than inside, `a3s.code.tool-result-evidence.v1` preserves the existing v1
evidence wire shape. The binding contains:

- the exact `a3s.code.tool-result-transform.v1` algorithm identity;
- a SHA-256 digest of the complete validated
  `ToolResultTransformPolicyV1`; and
- a second SHA-256 digest binding the schema, algorithm, and policy digest.

The policy digest is SHA-256 over the UTF-8 domain
`a3s.code.tool-result-transform-policy-digest.v1`, one zero byte, and compact
JSON for the policy in its declared field order. The binding digest uses the
same construction with domain
`a3s.code.tool-result-transform-binding.v1` and the ordered `schema`,
`transform_algorithm`, and `policy_digest` fields. These fixed inputs let a
managed host pin an expected identity without copying tenant, provider,
credential, endpoint, or object-path data into Core.

Code creates and validates the binding before calling the Tool, attaches it to
both lossy and lossless executor results, and rejects metadata drift before
release. Internal pre-execution denials remain ordinary Tool-result evidence
without claiming that a transform ran. On snapshot load, a retained transform
binding must validate, match the Session's exact persisted policy, and name the
same algorithm as its adjacent Tool-result evidence. Snapshots written before
this additive metadata object remain loadable.

## Portable session checkpoints

`SessionCheckpointExportV1` maps Code's two time-bearing persistence values
into one portable provider artifact:

- one complete, invariant-checked `SessionSnapshotV1`; and
- optionally, one exact schema-v1 `LoopCheckpoint` taken after a completed
  Tool round.

The payload uses schema `a3s.code.session-checkpoint-payload.v1`, media type
`application/vnd.a3s.code.session-checkpoint.v1+json`, and format
`a3s_code_session_checkpoint_v1`. Its `canonical_json_v1` encoding is compact
UTF-8 JSON with every object key recursively sorted lexicographically, array
order preserved, and no insignificant whitespace. Import accepts no alternate
encoding under the same identity. The complete payload is bounded to 256 MiB
before parsing or release.

`SessionCheckpointDescriptorV1` is content-free. It contains:

- `SessionSnapshotEvidenceV1`, binding the exact snapshot schema, Session ID,
  canonical byte length, and SHA-256;
- optional `SessionLogicalResumeEvidenceV1`, binding
  `between_tool_rounds_v1`, the same Session, source Run, exact loop schema,
  completed Tool-round count, checkpoint timestamp, canonical byte length, and
  SHA-256; and
- the aggregate payload byte length and SHA-256.

Each evidence digest and the outer descriptor digest uses its versioned UTF-8
domain, one zero byte, and canonical compact JSON excluding its own digest
field. A content digest is plain SHA-256 over the exact corresponding canonical
bytes. Golden integration evidence pins all four identities.

Export rejects an incompatible Session or loop schema, a zero-round resume,
foreign Session ownership, an absent source Run, or a source Run that is
already terminal. A logical resume also requires the portable Session's
cognitive binding to equal the source Run's frozen binding; this rejects a
mixed N/N+1 artifact even though an ordinary Session save may legitimately
sample the binding visible to the next Run. `from_parts()` validates descriptor
self-digests before bounded parsing, requires byte-for-byte canonical
re-encoding, validates every snapshot invariant, recomputes all component
evidence, and compares the full descriptor. `SessionSnapshotEvidenceV1::validate_for()` and
`SessionLogicalResumeEvidenceV1::validate_for()` let a host independently pin
the exact component it is about to store or resume. Runtime API keys retain
their existing `skip_serializing` behavior, and the export's `Debug` output
never includes payload bytes.

A Rust host can inject `SessionCheckpointExportSink` through `SessionOptions`
to receive the canonical artifact directly from a live blocking or streaming
Run. Code keeps the boundary marker internal: after the Tool round it closes
the capability Turn, drains preceding agent and runtime events, reads the
source Run's frozen cognitive authority, captures one semantic generation, and
pairs it with the loop-owned logical value. If a `SessionStore` is also
configured, its raw loop-checkpoint write and the host export observe the same
logical value; no store is required for export. Code awaits both handoffs before
releasing the loop, while independently logging either failure and allowing the
live Run to continue. This is a process-local causal and durability handoff,
not a Cloud identity, distributed transaction, or external revision fence.

The descriptor intentionally has no content URI, storage provider, namespace,
tenant policy, Cloud checkpoint ID, retention, approval, or parent/fork field.
An authorized host persists the payload in shared immutable-object
infrastructure; Cloud `A1.6` owns the business checkpoint record and lineage
that references these Code-computed identities.

`AgentProtocolRunRecoverExactV1` is the additive Code-host admission path for
the complete `SessionCheckpointDescriptorV1`. Its validated request digest
binds the recovery receipt, while the descriptor digest binds the target Run's
immutable input. `AgentProtocolHost::execute_exact_recovery()` uses two phases:
under one Session execution lease it loads, ownership-checks, and content-
validates the source `LoopCheckpoint`, then it captures the workspace baseline
and admits execution from the pinned in-memory value. Consequently, checkpoint
overwrite between validation and worker start cannot redirect execution;
content drift is rejected before a target Run exists; exact request replay does
not depend on continued source retention; and a changed descriptor cannot reuse
an existing target Run ID.

`AgentProtocolHarness::execute_checkpoint_recovery()` handles a complete
portable artifact without first publishing two store fragments. It compares
the request descriptor with the export before decoding, revalidates the exact
canonical bytes, extracts the snapshot and logical value from that same
payload, restores an unpublished Session, and publishes the Session in the
Harness map only after exact target-Run admission. A persisted snapshot without
the target Run must match `SessionSnapshotEvidenceV1`; an already persisted
target is restored for exact replay/conflict checking; and an unrelated live
Session is rejected rather than replaced. Failures before publication release
the Session and workspace, and descriptor or runtime-rebinding failures leave
no Harness entry or prewritten snapshot/checkpoint pair.

The Harness admission mutex provides only a process-local visibility boundary.
It cannot fence another process or turn provider-specific SessionStore files
into one distributed transaction. The future common Cloud Harness contract
must bind the authorized immutable object revision and supply CAS/lease fencing
for external writers before it claims distributed atomicity.
`AgentProtocolRunRecoverV1`, `AgentProtocolCommandV1`, and the current HTTP
command path remain unchanged while that common checkpoint contract is still
unfrozen.

## Data boundary

The five snapshots never retain Tool-argument text, prompt text, Tool-result
text, source text, vectors, credentials, headers, artifact paths, or provider
endpoints. They are small even when the underlying input is large because JSON
is streamed directly through the digest writer instead of first allocating a
duplicate serialized request.

This is a boundary for the new evidence, not a claim that every other Run event
is plaintext-free. Existing user, assistant, and Tool events retain their own
documented persistence policy. SHA-256 also provides integrity and correlation,
not encryption: low-entropy values can be guessed. A host must not export a
snapshot to a less-trusted boundary solely because it contains digests instead
of plaintext.

No vectors or workspace chunks enter session checkpoints through this feature.
Configured sessions retain original Tool content through the authorized host
adapter before any deterministic projection can discard bytes. Cloud-managed
policy selection and cross-repository projection conformance remain owned by
`CAR-03` and the Cloud integration gates. Code's secret-free transform binding
provides the identity those gates can pin; it is not proof of Cloud admission.

## Validation and replay

All five public snapshot types provide `validate()`. Validation rejects:

- an unsupported schema;
- malformed or non-canonical SHA-256 values;
- impossible readiness, coverage, count, byte, or digest combinations; and
- any snapshot digest that no longer matches its contents.

`ModelInputSnapshotV1::validate_against()` additionally validates both
snapshots and requires the referenced capability digest, model-visible Tool
count, and Tool-definition digest to agree. Consumers should use this paired
method after resolving `capabilitySnapshotDigest` from the Run journal.
`ModelUsageSnapshotV1::validate_against()` requires the call sequence, input
snapshot digest, and prompt estimate to agree with the exact input snapshot.
`ModelPresentationSnapshotV1::validate_against()` requires its call sequence,
presented Tool count, and presented-definition digest to agree with that same
input snapshot. A profiled capture also verifies that the actual definition
list is an exact-description subset of the expected projection; unknown names,
schema changes, and description injection fail before provider use.
`ToolRequestSnapshotV1::validate_against()` requires the event Tool identifier,
Tool name, final serialized arguments, and typed origin to agree with the
snapshot.
Keeping context-usage fields in this new schema leaves the existing
`ModelInputSnapshotV1` wire shape and snapshot-digest identity unchanged.

Evidence serialization and capture occur after the run budget pre-check and
before the provider future is polled or a streaming provider setup begins. A
capture failure therefore prevents an unbound model call. A closed event
receiver remains observational and does not become execution authority. Run
cancellation interrupts bounded-channel backpressure before provider use and
after provider completion. A streaming caller cancellation also interrupts
usage-evidence backpressure and prevents a terminal response from leaking into
the cancelled stream.

Tool-request capture occurs inside the cancellation-selected governance future
after final argument validation and before policy evaluation. A capture failure
prevents governance and execution. A closed event receiver remains
observational, while cancellation can interrupt bounded-channel backpressure
before any Tool side effect starts.

Live streams, persisted runs, bounded event pages, and replay all use the same
`EventEnvelopeV1` payload. Node.js, Python, and Go event catalogs are generated
from the Core catalog, so unknown future fields remain preservable without a
second SDK event taxonomy. The Harness regression suite validates snapshot
digests after persistence and requires replayed evidence to equal the original
events exactly.

## Operator guidance

- Retain the event protocol version, call sequence, capability digest, input
  digest, and usage digest together when correlating with provider or Gateway
  request records.
- Retain a Tool-request snapshot with its event Tool identifier and name; use
  `validate_against()` in an authorized environment when exact arguments are
  available.
- Treat prompt estimates and `LlmClient` usage as context diagnostics, not the
  Gateway billing ledger.
- Use repeated Tool-result bytes and token estimates to identify avoidable
  per-call context waste before enabling a deterministic projection policy.
- Treat `artifact.content_reference` as an authorized logical identifier, not
  a public download URL. Resolve it through the owning Cloud/content authority.
- Re-inject the exact immutable-content binding before resuming a configured
  session. Do not fall back to local artifacts when the provider is unavailable.
- Make a live checkpoint sink idempotent by descriptor digest, encrypt and
  authorize its payload as Session/Tool content, and return only after the
  host-required durability level is reached. Do not treat the callback as a
  distributed CAS or checkpoint-lineage authority.
- Re-inject the source Run's exact cognitive provider when restoring a logical
  checkpoint. A payload whose Session view and source Run name different
  cognitive generations is invalid.
- Investigate a capability digest change during a run: expected causes include
  retrieval moving from building to ready or a new catalog/vector generation.
- Compare source and presented token estimates to measure a Profile's context
  effect without treating that estimate as provider billing.
- Do not log the underlying input to explain a digest mismatch. Reproduce in an
  authorized environment and compare versioned component digests and counters.
