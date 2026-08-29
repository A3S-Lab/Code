# Durable Memory Integration

A3S Code integrates with A3S Memory V2 through an exact, host-injected
`DurableMemorySession`. Candidate shadowing measures the new integrity model
without changing model context. Active recall is a separate opt-in mode with
evidence-backed activation and fail-safe admission. Rust hosts may additionally
attach one exact semantic serving generation without moving embeddings or
retrieval policy into the A3S Memory storage kernel.

## Ownership boundaries

- A3S Code owns turn extraction, redaction, candidate proposal, activation
  gates, context admission, and exact resume-binding validation.
- A3S Memory owns exact namespace isolation, atomic and idempotent change sets,
  revision history, non-destructive lifecycle state, pure query, access events,
  durable repository recovery, and caller-owned vector-index primitives.
- A3S Code owns bounded embedding execution, hybrid fusion, candidate
  re-verification, lexical fallback, and cancellation for an attached semantic
  generation.
- The embedding host owns repository, embedding provider, and vector-index
  construction; tenant/principal/scope selection; evidence retention; explicit
  index refresh; restart reinjection; and semantic lifecycle jobs. Code can own
  their session schedule; A3S Flow may orchestrate broader jobs, but neither
  truth nor consolidation policy belongs to the storage kernel.

This division keeps the repository policy-free. A storage backend cannot decide
whether an LLM statement is true, and an extraction prompt cannot weaken the
repository's isolation or durability invariants.

## Candidate shadowing

`DurableMemoryMode::ShadowCandidates` has the following contract:

1. Existing V1 extraction and recall remain the serving path.
2. A V2 write happens only after the corresponding V1 write succeeds.
3. The V2 node is always `Candidate`, never `Active`.
4. The node contains a typed `SessionTurn` evidence reference. Its digest is
   computed from the same bounded, redacted turn fields used by extraction; the
   URI contains no prompt or response text.
5. Candidate and change-set identities are content addressed. An exact replay
   returns the original result instead of creating a duplicate.
6. V2 repository failures are reported as warnings and do not change the V1
   turn result. Shadow mode is observational, not a second serving authority.
7. V2 candidates are never queried for prompt context in this mode.

## Explicit activation and active recall

`DurableMemoryMode::ActiveRecall` retains candidate shadow writes and adds a
bounded active-only lexical query. An optional second stage performs a bounded
number of exact reads for one-hop `RelatedTo` targets. It does not recurse,
follow `ConflictsWith`, return non-Active targets, or widen the exact namespace.
It does not auto-activate extraction output. The host submits
`DurableMemoryActivation` for one exact candidate revision; Code accepts only
new Manual or Verification decision evidence, and A3S Memory stores that
evidence in the atomic activation revision. LLM confidence and importance
remain annotations and cannot authorize activation.

`DurableMemoryRecallPolicy` requires an explicit result bound and minimum
lexical score; relation reads default to zero. `preview_recall` runs the same
retrieval branch without recording admission or use and without adding content
to a prompt. Code waits for final context assembly, then records admission for
each selected exact node revision. A stale, inactive, or unpersistable
admission is removed before the model call. Candidate, superseded, conflicted,
and tombstoned nodes are not queried. When V1 and V2 return the same normalized
content, the audited V2 item is used once rather than injecting duplicate text.

Admission means the revision entered model input. Use is deliberately separate:
the host calls `DurableMemorySession::record_use` only when a node was cited,
selected, or otherwise used. Both event types are idempotent.

When V1 extraction declares that a new memory supersedes an older item, Code
marks the older item as superseded and protects it from pruning. Recall filters
the archived item, but the original content and replacement link remain
available for audit.

The lexical algorithm has an explicit stable identity:
`a3s.memory.lexical.word-cjk-bigram.v1`. It lowercases and matches ordinary
alphanumeric words. For contiguous Han, Kana, Hangul, Bopomofo, and related CJK
runs, it also indexes the whole run and overlapping character bigrams. It does
not add single-character unigrams. This supports deterministic same-language
phrase variation in text that has no spaces while limiting broad accidental
matches. It is not a stemmer, translator, embedding model, or cross-language
semantic search system.

### Optional semantic recall

`DurableMemorySemanticRecall` composes Code's existing bounded
`EmbeddingExecutor` with a caller-owned A3S Memory `VectorIndex`. It adds no
implicit task and does not traverse the repository. The host explicitly calls
`replace_namespace` with the Active nodes it has admitted for indexing; the
replacement is atomic at the opaque partition derived from the namespace and
the exact semantic serving binding. Distinct serving generations therefore do
not overwrite each other when a host deliberately shares one vector index.

At query time, Code treats vector results as untrusted candidates:

1. embed the query under the exact provider revision and bounded execution
   policy retained by the session binding;
2. search only the opaque partition derived from the exact memory namespace
   and semantic serving generation;
3. fence the vector-index revision across search and verification;
4. re-read every candidate by ID from the bound `MemoryRepository` namespace;
5. require `Active` status, the exact current node revision, and the exact
   content digest recorded with the vector;
6. fuse verified lexical and semantic ranks with the versioned RRF `k=60`
   profile, then apply the existing result and relation bounds.

An embedding, vector, verification, or concurrent-index failure drops the
semantic branch and preserves the exact lexical result. It never turns a
semantic failure into broader repository access. When both branches return the
same node, the later repository-verified revision is used once and labeled
`Hybrid`; otherwise the result records `Lexical`, `Semantic`, or `Related`.

## Host wiring

```rust,no_run
use a3s_code_core::{
    DurableMemoryRecallPolicy, DurableMemorySession, SessionOptions,
};
use a3s_memory::repository::{FileMemoryRepository, MemoryNamespace};
use std::sync::Arc;

# async fn options() -> anyhow::Result<SessionOptions> {
let repository = Arc::new(FileMemoryRepository::open(".a3s/memory-v2").await?);
let namespace = MemoryNamespace::try_new(
    "tenant-acme",
    "principal-alice",
    "repository-a3s-code",
)?;
let durable_memory = DurableMemorySession::shadow(repository, namespace);

let options = SessionOptions::new().with_durable_memory(durable_memory);
# Ok(options)
# }
```

After shadow evaluation and explicit activation are in place, a host can opt in
to active recall instead:

```rust,no_run
# use a3s_code_core::{DurableMemoryRecallPolicy, DurableMemorySession};
# use a3s_memory::repository::{InMemoryRepository, MemoryNamespace};
# use std::sync::Arc;
# fn binding() -> anyhow::Result<DurableMemorySession> {
# let repository = Arc::new(InMemoryRepository::new());
# let namespace = MemoryNamespace::try_new("tenant", "principal", "scope")?;
let recall = DurableMemoryRecallPolicy::try_new(5, 0.40)?
    .try_with_related_lookups(8)?;
let durable_memory = DurableMemorySession::active_recall(
    repository,
    namespace,
    recall,
);
# Ok(durable_memory)
# }
```

A Rust host can attach an exact semantic generation after it has selected the
provider, index, authority identity, and current Active snapshot:

```rust,no_run
# use a3s_code_core::embedding::{EmbeddingExecutorConfig, EmbeddingProvider};
# use a3s_code_core::{DurableMemoryRecallPolicy, DurableMemorySemanticRecall, DurableMemorySemanticRecallPolicy, DurableMemorySession};
# use a3s_memory::repository::{InMemoryRepository, MemoryNamespace, MemoryNode};
# use a3s_memory::vector::VectorIndex;
# use std::sync::Arc;
# use tokio_util::sync::CancellationToken;
# async fn binding(
#     provider: Arc<dyn EmbeddingProvider>,
#     index: Arc<dyn VectorIndex>,
#     active_nodes: Vec<MemoryNode>,
# ) -> anyhow::Result<DurableMemorySession> {
# let repository = Arc::new(InMemoryRepository::new());
# let namespace = MemoryNamespace::try_new("tenant", "principal", "scope")?;
let semantic = DurableMemorySemanticRecall::new(
    "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
    provider,
    EmbeddingExecutorConfig::default(),
    index,
    DurableMemorySemanticRecallPolicy::try_new(20, 0.75)?,
)?;
semantic
    .replace_namespace(&namespace, active_nodes, CancellationToken::new())
    .await?;

let durable_memory = DurableMemorySession::active_recall(
    repository,
    namespace,
    DurableMemoryRecallPolicy::try_new(5, 0.40)?,
)
.with_semantic_recall(semantic)?;
# Ok(durable_memory)
# }
```

The authority digest is a secret-free host assertion that identifies the
semantic repository/index authority. It is not a credential and Code cannot
derive it from a database path or remote service. The host must change it when
that authority changes and must refresh or clear vector partitions when Active
repository content changes. Stale vectors are filtered at serving time, but an
unrefreshed index can still reduce recall.

During session preparation, Code fills an omitted `tenant_id` and `principal`
from the exact namespace. If the caller supplies either field explicitly, it
must match the namespace or session construction fails. The scope is never
inferred from a path or a backend name; the host selects it explicitly.

`DurableMemorySession` contains a live repository object and is intentionally
not serialized in a session snapshot. Its secret-free
`DurableMemoryBindingV1` is persisted instead: schema version, exact
tenant/principal/scope, mode, result bound, lexical threshold, and relation
lookup bound, plus the lexical retrieval and admission context-identity
profiles. Semantic sessions additionally retain the secret-free semantic
authority digest, exact embedding provider/model/revision/dimension and
normalization, embedding execution-policy digest, vector descriptor, semantic
candidate policy, and fusion profile. On restore, the host reconstructs the
repository and, when applicable, the exact provider and vector index, then
injects a binding equivalent at the typed value level. Resume fails closed when
a bound session is missing that injection, any visible binding field drifts, or
an older unbound session attempts to acquire memory authority during resume.
A rollout from shadow to active recall therefore creates a new session rather
than silently changing the semantics of an existing persisted session.

Schema `1` snapshots written before either profile existed deserialize as the
legacy `a3s.memory.lexical.word.v1` retrieval profile and
`a3s.code.memory.context.host-id.v0` context profile. Schema `2` introduced the
current lexical profile but still used the legacy host-generated context ID.
Schema `3` introduced
`a3s.code.memory.context.session-run-sequence-sha256.v1`, which separates
process-local run IDs but cannot distinguish a run ID reused after FIFO
retention and process restart. New bindings use schema `4` with
`a3s.code.memory.context.session-run-invocation-sequence-sha256.v2`. Only those
four lexical schema/profile combinations validate. New lexical-only sessions
continue to use schema `4`. Attaching a semantic generation produces schema
`5`, which requires the current lexical/context profiles, `ActiveRecall`, and a
valid exact semantic binding; schemas `1` through `4` reject a semantic field.
Exact resume rejects every legacy/current or lexical/semantic mismatch, and old
binaries reject schema `5` instead of ignoring the new serving authority.
Backward-readable data is not permission to change a live session's retrieval
or admission identity behavior.

The descriptor does not claim to authenticate a database path or remote
service instance. Repository/index construction, credentials, remote fencing,
refresh continuity, and backend continuity remain host responsibilities; Code
verifies only the namespace, current node revision/content, vector revision,
and serving identity it can observe. It never resolves `latest`, opens an
implicit repository or vector database, or substitutes a global principal.

## Explicit multi-agent sharing

Independent agents collaborate through durable memory only when the host gives
each session a `DurableMemorySession` with the same exact repository and
namespace. Code does not discover a global memory backend, widen a binding, or
inherit the parent's binding into delegated children. An unbound child remains
isolated; a host that wants a separate agent to share memory creates that agent
and injects the binding explicitly.

Every final model context uses the stable identity profile
`a3s.code.memory.context.session-run-invocation-sequence-sha256.v2`. Code hashes
the exact session ID, run ID, Code-owned invocation incarnation, and
invocation-local context sequence with domain separation, producing a bounded
opaque `a3s-code-context-v2-*` value. Clones of one live invocation share its
incarnation and monotonic sequence, while a separately reconstructed
invocation receives a fresh incarnation. Different sessions remain distinct
when host run IDs collide, and the same session remains distinct when a
process-local run ID is legitimately reused after retained run history is
evicted. Code does not rely on the weaker cross-process guarantee that the
host's `IdGenerator` contract deliberately does not make.

An admission retry remains subject to the repository's full-event idempotency
contract, including its timestamp; conflicting replay fails closed instead of
double-counting. If the scoped invocation identity, sequence, or host timestamp
is unavailable, recalled V2 items are removed before the model call.
`DurableMemoryBindingV1.contextIdProfile` persists this algorithm identity, so
resume cannot silently replace an older admission identity with the current
session/run/invocation/sequence contract. A normal new run also uses atomic
reservation: collision with a still-retained run returns
`RUN_IDENTITY_CONFLICT` before model use instead of overwriting history.

This correlation value is not an authentication token. Namespace authority
still comes from the exact host-injected binding, and repository fencing and
globally meaningful session assignment remain host responsibilities. See
[Durable Memory Multi-Agent Evaluation](DURABLE_MEMORY_MULTI_AGENT_EVAL.md) for
the executable sharing contract and
[Durable Memory Restart Endurance Evaluation](DURABLE_MEMORY_RESTART_ENDURANCE_EVAL.md)
for cross-process reuse, retention, revision, and repeated-restart evidence.

The `durable_memory_restart` integration test uses real
`FileMemoryRepository` and `FileSessionStore` instances across full close and
reopen boundaries. It proves that a Candidate remains unavailable before
verification, the snapshot retains the exact binding, missing and scope-drifted
reinjection fail, verified Active memory reaches the resumed model input, and
node evidence plus admission/use events survive another repository replay. It
also asserts that Session teardown releases the repository lock. The latter
regression is protected by weak back-references from registry-bound
orchestrator and Skill tools, preventing tool registration cycles from
retaining `AgentConfig.memory` after close. The same test rejects a retained
host run-ID collision before model use and proves that the original run and
admission count remain unchanged.

## Owned maintenance and consolidation

`AgentMemory` construction is side-effect free. A configured V1 `PrunePolicy`
does not spawn an immortal task from its constructor. Asynchronous session
construction starts one `MemoryMaintenanceRuntime` only when pruning or typed
host jobs are configured. The session is its owner:

- each schedule waits one full interval before its first run;
- one job never overlaps itself, and missed ticks are skipped instead of
  replayed in a burst;
- `memory_maintenance_health()` reports run counts, affected items, current
  work, and bounded last-error text;
- a failed run degrades health; a later success clears that transient error;
- `session.close().await` cancels and joins maintenance before draining final
  extraction writes, with bounded abort as a last resort;
- dropping the final runtime owner cancels and aborts remaining workers.

Semantic consolidation stays host policy. A host injects a typed
`ScheduledMemoryMaintenance` whose `MemoryMaintenanceJob` can inspect the exact
`MemoryMaintenanceContext`. Any V2 mutation must still carry deterministic
identities, evidence, and expected revisions. The runtime supplies scheduling
and ownership, not permission to auto-activate candidates.

The integration suite exercises that boundary with a verified supersession
job: one atomic change set creates a replacement Candidate, activates its exact
revision with separate Verification evidence, adds both supersession
relations, and marks the old node Superseded while retaining its history. This
is executable evidence for the mechanism, not a default consolidation policy.

```rust,no_run
use a3s_code_core::memory::{
    MemoryMaintenanceJob, MemoryMaintenanceOptions, ScheduledMemoryMaintenance,
};
use a3s_code_core::SessionOptions;
use std::{sync::Arc, time::Duration};

# fn options(
#     consolidator: Arc<dyn MemoryMaintenanceJob>,
# ) -> anyhow::Result<SessionOptions> {
let consolidation = ScheduledMemoryMaintenance::try_new(
    "verified_consolidation",
    Duration::from_secs(900),
    consolidator,
)?;
let maintenance = MemoryMaintenanceOptions::new()
    .with_job(consolidation)
    .try_with_shutdown_timeout(Duration::from_secs(5))?;
let options = SessionOptions::new().with_memory_maintenance(maintenance);
# Ok(options)
# }
```

Because maintenance owns Tokio tasks, use
`agent.session_builder(workspace).options(options).build().await`; the
synchronous compatibility factory rejects configured maintenance instead of
silently skipping it. Like the durable repository binding, host jobs are
runtime-only and must be injected again after restart.

## Cross-language boundary

Rust hosts own the complete V2 surface because `DurableMemorySession` contains
a live `MemoryRepository` and custom maintenance contains a live
`MemoryMaintenanceJob`. Node.js, Python, and Go do not accept a raw backend
name, path, or untyped callback as a substitute for those authority-bearing
objects. A future SDK extension must introduce typed repository and job
providers, plus typed semantic provider/index generations, with explicit
lifecycle and namespace semantics.

Operational observation does cross the boundary now. The same non-sensitive
health shape is available as:

- Rust: `session.memory_maintenance_health()`;
- Node.js: `session.memoryMaintenanceHealth()`;
- Python: `session.memory_maintenance_health()`;
- Go: `session.MemoryMaintenanceHealth(ctx)`.

The snapshot contains phase, bounded counters, worker state, and bounded error
text, but no memory content, evidence, credentials, or repository handles.

## Evidence and privacy

The V2 node stores a reference and SHA-256 digest, not the turn body. The digest
binds the candidate to the normalized extraction input, while the host remains
responsible for retaining any source material required by its audit policy.
Because shadow candidates are not admitted to context, an unavailable source
cannot silently become a serving memory. Activation requires separate decision
evidence, and every selected active revision must persist admission before it
enters model input.

Semantic indexing is an additional content-egress boundary. The selected
embedding provider receives the full text of explicitly indexed Active nodes
and each semantic query. The caller-owned vector index retains embeddings plus
bounded labels containing node IDs, revisions, and content digests. Candidate
content is rejected by `replace_namespace`, and query/error diagnostics do not
log source or query text. Hosts must still authorize the provider and index for
that tenant/principal/scope, protect their storage and transport, and choose an
authority digest that contains no credential.

## Migration sequence

1. Run shadow mode beside the existing V1 serving path.
2. Evaluate candidate precision, duplicate rate, namespace isolation, evidence
   availability, and restart replay.
3. Submit explicit Manual or Verification activation evidence with optimistic
   revisions; do not infer activation from LLM confidence alone.
4. Enable bounded `ActiveRecall`, observe admission failures, and record use
   only for exact revisions actually used downstream.
5. Save, close, reopen the repository, and resume only with the persisted exact
   `DurableMemoryBindingV1`; treat backend continuity as a separate host gate.
6. Run consolidation and retention through the owned maintenance lifecycle;
   inspect health and close sessions explicitly.
7. Run the locked lexical and relation-aware evaluation. Fixture v1 reaches
   relation Recall@5 `0.90`; keep that dependency-free branch as the fallback.
8. Run the product evaluation through real `AgentSession` turns. Require the
   no-memory/V1/V2 task-success comparison, write precision, evidence fidelity,
   conflict preservation, context bound, provider-call bound, nominal cost
   bound, and real admission counters to pass before rollout.
9. Run the multilingual gate. Require the persisted retrieval-profile identity,
   English and CJK same-language ranking, real-session context bounds,
   Candidate isolation, namespace isolation, and the explicit cross-language
   miss to remain stable.
10. If cross-language or no-token-overlap recall is required, attach a typed
    semantic generation and run the semantic gate. Require lexical-positive
    misses, semantic Recall@1, real-session admission, and zero Candidate,
    foreign-namespace, and stale-vector hits. Treat its fixture provider as a
    serving-mechanics gate, not production model-quality evidence.
11. Run the multi-agent gate. Share only an exact host-injected binding; require
     distinct session/run admissions under colliding local generators, explicit
     Candidate and foreign-principal isolation, peer-independent teardown, and
     file-journal replay.
12. Run the restart-endurance gate. Fully close and resume four independent
     agents across three epochs with one retained run, reset each process-local
     generator, revise the Active node, and require all 24 contexts plus exact
    current-revision and namespace isolation after final journal replay.

See [Durable Memory Retrieval Evaluation](DURABLE_MEMORY_RETRIEVAL_EVAL.md) for
the retrieval metric definitions and vector decision. See
[Durable Memory Product Evaluation](DURABLE_MEMORY_PRODUCT_EVAL.md) for the
end-to-end serving, capture, cost, and consolidation evidence. See
[Durable Memory Multilingual Evaluation](DURABLE_MEMORY_MULTILINGUAL_EVAL.md)
for the query-profile contract, CJK gate, and its limits. See
[Durable Memory Semantic Evaluation](DURABLE_MEMORY_SEMANTIC_EVAL.md) for the
typed hybrid serving, current-revision verification, and cross-language gate.
See
[Durable Memory Multi-Agent Evaluation](DURABLE_MEMORY_MULTI_AGENT_EVAL.md) for
explicit sharing and context-identity semantics. See
[Durable Memory Restart Endurance Evaluation](DURABLE_MEMORY_RESTART_ENDURANCE_EVAL.md)
for the bounded repeated-restart production slice.

## Verification

Run checks from the Code crate workspace, not the monorepo root:

```text
cargo test -p a3s-code-core --lib durable_memory
cargo test -p a3s-code-core --test durable_memory_shadow
cargo test -p a3s-code-core --test durable_memory_active
cargo test -p a3s-code-core --test durable_memory_restart
cargo test -p a3s-code-core --test durable_memory_retrieval_eval -- --nocapture
cargo test -p a3s-code-core --test durable_memory_product_eval -- --nocapture
cargo test -p a3s-code-core --test durable_memory_multilingual_eval -- --nocapture
cargo test -p a3s-code-core --test durable_memory_semantic
cargo test -p a3s-code-core --test durable_memory_semantic_eval -- --nocapture
cargo test -p a3s-code-core --test durable_memory_multi_agent_eval -- --nocapture
cargo test -p a3s-code-core --test durable_memory_restart_endurance_eval -- --nocapture
cargo test -p a3s-code-core --test memory_maintenance_lifecycle
cargo test -p a3s-code-core --lib
```
