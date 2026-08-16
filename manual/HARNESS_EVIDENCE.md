# Harness Model-Call Evidence

Status: the digest-only capability and model-input evidence slice is delivered.
Durable references to authorized original content remain a separate `CAR-02`
gate.

## Purpose

A configured Agent is an intention. An auditable run needs evidence of what the
model could actually use and what arguments reached the provider-neutral model
boundary. A3S Code records that evidence in the existing Run event journal; it
does not introduce a second audit database or a provider-specific request model.

Two versioned events provide the boundary:

| Event | Emission | Purpose |
| --- | --- | --- |
| `run_capability_bound` | Before the first model call and again when its digest changes | Bind model-visible tools, workspace services, run-owned governance bindings, serializable policy identities, execution ceilings, and current semantic generation |
| `model_input_bound` | Before every model call | Bind bounded counters and digests for the actual provider-neutral input |

The same path covers completion, streaming, structured completion, and
structured streaming. Higher-level retries, repair calls, compaction, and
helper calls that re-enter the run-scoped `LlmClient` pass through this boundary
as well; an internal transport retry remains part of its owning model call.
Work that explicitly detaches from the Run event channel, such as post-terminal
background memory extraction, receives a separate auxiliary invocation and
cannot append evidence after the Run's terminal event.

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

## Data boundary

The two snapshots never retain prompt text, Tool-result text, source text,
vectors, credentials, headers, artifact paths, or provider endpoints. They are
small even when the underlying input is large because JSON is streamed directly
through the digest writer instead of first allocating a duplicate serialized
request.

This is a boundary for the new evidence, not a claim that every other Run event
is plaintext-free. Existing user, assistant, and Tool events retain their own
documented persistence policy. SHA-256 also provides integrity and correlation,
not encryption: low-entropy values can be guessed. A host must not export a
snapshot to a less-trusted boundary solely because it contains digests instead
of plaintext.

No vectors or workspace chunks enter session checkpoints through this feature.
An authorized immutable reference to original content is still required before
future deterministic Tool-result transformation can discard an original. That
work remains owned by `CAR-02`/`CAR-03` and Cloud projection policy.

## Validation and replay

Both public snapshot types provide `validate()`. Validation rejects:

- an unsupported schema;
- malformed or non-canonical SHA-256 values;
- impossible readiness, coverage, count, byte, or digest combinations; and
- any snapshot digest that no longer matches its contents.

`ModelInputSnapshotV1::validate_against()` additionally validates both
snapshots and requires the referenced capability digest, model-visible Tool
count, and Tool-definition digest to agree. Consumers should use this paired
method after resolving `capabilitySnapshotDigest` from the Run journal.

Evidence serialization and capture occur after the run budget pre-check and
before the provider future is polled or a streaming provider setup begins. A
capture failure therefore prevents an unbound model call. A closed event
receiver remains observational and does not become execution authority. Run
cancellation interrupts bounded-channel backpressure before provider use.

Live streams, persisted runs, bounded event pages, and replay all use the same
`EventEnvelopeV1` payload. Node.js, Python, and Go event catalogs are generated
from the Core catalog, so unknown future fields remain preservable without a
second SDK event taxonomy. The Harness regression suite validates snapshot
digests after persistence and requires replayed evidence to equal the original
events exactly.

## Operator guidance

- Retain the event protocol version, call sequence, capability digest, and input
  digest together when correlating with provider or Gateway request records.
- Treat token estimates as Code budgeting evidence, not provider billing.
- Investigate a capability digest change during a run: expected causes include
  retrieval moving from building to ready or a new catalog/vector generation.
- Do not log the underlying input to explain a digest mismatch. Reproduce in an
  authorized environment and compare versioned component digests and counters.
