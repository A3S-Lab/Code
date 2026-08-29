# Durable Memory Integration

A3S Code integrates with A3S Memory V2 through an exact, host-injected
`DurableMemorySession`. Candidate shadowing measures the new integrity model
without changing model context. Active recall is a separate opt-in mode with
evidence-backed activation and fail-safe admission.

## Ownership boundaries

- A3S Code owns turn extraction, redaction, candidate proposal, future
  activation gates and context admission.
- A3S Memory owns exact namespace isolation, atomic and idempotent change sets,
  revision history, non-destructive lifecycle state, pure query, access events,
  and durable repository recovery.
- The embedding host owns repository construction, tenant/principal/scope
  selection, evidence retention, restart reinjection, and semantic lifecycle
  jobs. Code can own their session schedule; A3S Flow may orchestrate broader
  jobs, but neither policy belongs to the storage kernel.

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

During session preparation, Code fills an omitted `tenant_id` and `principal`
from the exact namespace. If the caller supplies either field explicitly, it
must match the namespace or session construction fails. The scope is never
inferred from a path or a backend name; the host selects it explicitly.

`DurableMemorySession` contains a live repository object and is intentionally
not serialized in a session snapshot. On restore, the host must reconstruct the
repository and inject the same exact namespace. Code never resolves `latest`,
opens an implicit repository, or substitutes a global principal.

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

## Evidence and privacy

The V2 node stores a reference and SHA-256 digest, not the turn body. The digest
binds the candidate to the normalized extraction input, while the host remains
responsible for retaining any source material required by its audit policy.
Because shadow candidates are not admitted to context, an unavailable source
cannot silently become a serving memory. Activation requires separate decision
evidence, and every selected active revision must persist admission before it
enters model input.

## Migration sequence

1. Run shadow mode beside the existing V1 serving path.
2. Evaluate candidate precision, duplicate rate, namespace isolation, evidence
   availability, and restart replay.
3. Submit explicit Manual or Verification activation evidence with optimistic
   revisions; do not infer activation from LLM confidence alone.
4. Enable bounded `ActiveRecall`, observe admission failures, and record use
   only for exact revisions actually used downstream.
5. Run consolidation and retention through the owned maintenance lifecycle;
   inspect health and close sessions explicitly.
6. Run the locked lexical and relation-aware evaluation. Fixture v1 reaches
   relation Recall@5 `0.90`, so vectors remain deferred. Add semantic vectors
   only when versioned, independently labeled failures fall below that gate.

See [Durable Memory Retrieval Evaluation](DURABLE_MEMORY_RETRIEVAL_EVAL.md) for
the metric definitions, safety assertions, and current decision.

## Verification

Run checks from the Code crate workspace, not the monorepo root:

```text
cargo test -p a3s-code-core --lib durable_memory
cargo test -p a3s-code-core --test durable_memory_shadow
cargo test -p a3s-code-core --test durable_memory_active
cargo test -p a3s-code-core --test durable_memory_retrieval_eval -- --nocapture
cargo test -p a3s-code-core --test memory_maintenance_lifecycle
cargo test -p a3s-code-core --lib
```
