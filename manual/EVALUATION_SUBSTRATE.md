# Evaluation Substrate

The Code evaluation substrate provides the common runtime mechanisms needed by
a reviewer, verifier, quality gate, or other evaluator. It is intentionally
provider- and product-neutral. A host supplies the policy, prompt, model
client, and result vocabulary; Code supplies identity, bounded evidence,
isolation, supervision, and replay-safe storage primitives.

## Ownership boundary

Code owns the execution-local facts and lifecycle contracts:

- `ExecutionTargetV1` identifies one session/run and
  `ExecutionFrameV1` records runtime parentage.
- `ExecutionFactJournal` accepts append-only, digest-only facts with
  contiguous cursors and explicit FIFO retention gaps.
- `EvidenceReader` projects one atomic `RunStore` observation into a bounded
  `EvidenceSnapshotV1`.
- `AuxiliaryRunService` admits an isolated, cancellable auxiliary execution
  under a declared capability ceiling and output schema.
- `EvaluationSupervisor` applies a host-injected boundary policy and limits
  pending dispatches without blocking the parent run.
- `EvaluationResultSink` stores immutable, content-addressed result records.

The host or Cloud remains responsible for authorization, tenant projection,
durable retention, placement, business audit, checkpoint/fork lineage,
reviewer prompts and rubrics, severity or threshold policy, findings,
dispositions, UI, and transport. These concerns must not be reimplemented as
Core evaluator semantics.

## Data flow

```text
RunEventRecord
      │
      ├── ExecutionFactRecorder ──> ExecutionFactJournal
      │                                  │ digest-only cursor
      │                                  ▼
      └── RunEvidenceReader <──── RunStore + ArtifactStore
                 │
                 ▼
        EvidenceSnapshotV1
                 │
                 ▼
        AuxiliaryRunService
                 │ bounded JSON output
                 ▼
        host EvaluationResultV1
                 │ immutable CAS record
                 ▼
        EvaluationResultSink
```

State and the event page are captured from one `InMemoryRunStore` observation
generation. If an independently durable fact journal has a different cursor or
retention window, the snapshot is marked incomplete rather than presenting a
false consistent view.

## Content and privacy modes

`EvidenceReadRequestV1` defaults to `DigestOnly`. In that mode event payloads,
prompt text, terminal text, and artifact content are not copied into the
snapshot; their sizes and domain-separated digests remain available for
correlation. An intentional digest-only projection is complete when the source
cursor and fact window are complete.

`BoundedPayload` and the explicit `include_*` flags opt into bounded plaintext.
Every request has event, artifact, prompt, and terminal-result limits. An
oversized or unavailable item is represented by a digest marker and sets
`complete = false`. Callers must treat `complete = false` or
`retention_gap = true` as insufficient evidence for a gate that requires the
missing content. Run state always carries prompt/result/error byte counts and
domain-separated digests, even when the corresponding plaintext is omitted.

The fact journal never stores raw model prompts, tool arguments, tool output,
or reasoning text. Artifact references are sorted, deduplicated, and bounded.
Result payloads are host-defined JSON and must be redacted by the host before
writing if they contain tenant-sensitive content.

## Auxiliary execution contract

Construct an `AuxiliaryRunSpecV1` with:

1. the parent `ExecutionFrameV1`;
2. a stable purpose and idempotency id;
3. the exact `EvidenceSnapshotV1.snapshot_digest`;
4. an explicit `AuxiliaryCapabilityProfileV1` and optional parent ceiling;
5. a timeout, step declaration, and optional JSON Schema for the output.

`InMemoryAuxiliaryRunService` validates the evidence digest and target before
admission, derives a child cancellation token, catches executor panics,
settles timeout/cancellation as terminal states, validates bounded output, and
returns the same handle for an exact duplicate id. The executor is host-owned:
the profile is an admission/ceiling contract, while a host that exposes real
workspace or tool operations must enforce those flags through its own scoped
capability dispatcher. Core never grants ambient tools to an auxiliary run.

`StructuredAuxiliaryExecutor` is a convenience adapter over Code's existing
schema-validated LLM engine. Its request factory remains host-owned, so Core
does not prescribe a reviewer prompt, rubric, or decision token.

## Boundary supervision

`EvaluationPlanV1` selects a generic boundary (`EveryEvent`, `TurnEnd`, or
`RunTerminal`). `EvaluationSupervisor::observe_event`:

1. converts and appends the event as a fact;
2. asks the host `EvaluationPolicy` whether a plan applies;
3. reserves the pending slot before reading evidence or spawning work;
4. derives a deterministic auxiliary id from target, sequence, purpose, and
   fact digest; and
5. releases the slot when the auxiliary handle reaches a terminal state.

Replayed facts do not dispatch a second evaluator after a successful admission.
Failed evidence or auxiliary admission releases the reservation so a host may
retry the same fact. Cancellation propagates through the supervisor token and
never turns an auxiliary result into an implicit parent-run decision.

## Result persistence

`EvaluationResultV1.decision` is an open host-defined string. Core validates
shape, target, evidence digest, bounded JSON, and a content digest; it does not
enumerate reviewer outcomes. `EvaluationRecordV1` adds an observation time and
an immutable record digest. The in-memory sink provides a reference CAS:

- exact record replay is idempotent;
- a different valid record for the same target/evaluator/auxiliary identity is
  rejected as a conflict; and
- optional FIFO retention removes both the record and its identity index.

A production host should implement `EvaluationResultSink` over its durable
object store and apply its own authorization, encryption, retention, and
cross-process fencing.

## Versioned wire projection

`EvaluationWireEnvelopeV1` is the additive process boundary for the contracts
above. Its JSON shape is intentionally small and strict:

```json
{
  "schema": "a3s.code.evaluation-wire.v1",
  "version": 1,
  "kind": "evidence_snapshot",
  "payload": { "...": "EvidenceSnapshotV1 fields" }
}
```

The version-one catalog carries an evidence read request or snapshot, an
auxiliary specification, snapshot, or bounded output, and an evaluation
result or immutable record. Core validates the envelope identity, encoded-size
bound, closed kind catalog, and the selected Rust payload with
`deny_unknown_fields`; a host still binds an auxiliary specification to the
actual evidence snapshot at admission time. Unknown top-level or payload
fields and unsupported versions fail closed.

The Node.js declaration (`sdk/node/evaluation-protocol-v1.d.ts`), Python
typing module (`sdk/python/python/a3s_code/evaluation_protocol_v1.py`), Go
projection (`sdk/go/evaluation_protocol_v1.go`), catalog, and negative
fixtures are generated from `core/src/evaluation/protocol.rs`:

```text
node scripts/generate_evaluation_protocol_artifacts.mjs --check
node scripts/check_evaluation_protocol_artifacts.mjs
```

SDK payloads remain opaque JSON/bytes by design. This preserves one Core schema
authority while allowing a host to add a typed transport adapter without
turning Code into a reviewer or Cloud business-transport implementation.

## Minimal composition

```rust
use a3s_code_core::evaluation::{
    AuxiliaryCapabilityProfileV1, AuxiliaryRunSpecV1, EvidenceReadRequestV1,
    EvaluationRecordV1, EvaluationResultV1, EvaluationResultSink,
    ExecutionFrameV1, InMemoryAuxiliaryRunService, InMemoryEvaluationResultStore,
    RunEvidenceReader,
};

let evidence = RunEvidenceReader::new(runs)
    .read(EvidenceReadRequestV1::new(target.clone()))
    .await?;
let spec = AuxiliaryRunSpecV1::new(
    ExecutionFrameV1::root(target.clone()),
    "host-defined-evaluator",
    "host-defined instruction",
    evidence.snapshot_digest.clone(),
)
.with_id("auxiliary-id")
.with_capabilities(AuxiliaryCapabilityProfileV1::tool_free());
let output = InMemoryAuxiliaryRunService::new(executor)
    .spawn(spec, evidence.clone(), None)
    .await?
    .wait()
    .await?;
let result = EvaluationResultV1::new(
    "host-evaluator",
    target,
    "auxiliary-id",
    "host-token",
    output.value,
    evidence.snapshot_digest,
)?;
let record = EvaluationRecordV1::new(result, observed_at_ms)?;
sink.write(record).await?;
```

The example intentionally leaves `runs`, `executor`, `sink`, and the decision
token to the host. No authentication flow, Cloud audit endpoint, or product
reviewer is implied by the common API.

## Verification gates

From the Code repository, run the focused substrate checks:

```text
cargo test -p a3s-code-core evaluation:: --lib -- --nocapture
cargo test -p a3s-code-core --test evaluation_substrate -- --nocapture
cargo clippy -p a3s-code-core --all-targets -- -D warnings
node scripts/generate_evaluation_protocol_artifacts.mjs --check
node scripts/check_evaluation_protocol_artifacts.mjs
```

Release qualification additionally requires the normal workspace feature
matrix, rustdoc warning gate, protocol/SDK schema fixtures, restart and
retention tests, adversarial redaction tests, and a durable host adapter. Those
are tracked as `EVAL-PROTO1` and `EVAL-GA1` in [`ROADMAP.md`](../ROADMAP.md).
