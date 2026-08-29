# Durable Memory Integration

A3S Code integrates with A3S Memory V2 through an exact, host-injected
`DurableMemorySession`. The first production mode is deliberately limited to
candidate shadowing: it measures the new integrity model without allowing an
unreviewed memory to affect model context.

## Ownership boundaries

- A3S Code owns turn extraction, redaction, candidate proposal, future
  activation policy, and context admission.
- A3S Memory owns exact namespace isolation, atomic and idempotent change sets,
  revision history, non-destructive lifecycle state, pure query, access events,
  and durable repository recovery.
- The embedding host owns repository construction, tenant/principal/scope
  selection, evidence retention, restart reinjection, and any lifecycle
  scheduler. A3S Flow may orchestrate those lifecycle jobs, but it is not part
  of the storage kernel.

This division keeps the repository policy-free. A storage backend cannot decide
whether an LLM statement is true, and an extraction prompt cannot weaken the
repository's isolation or durability invariants.

## Current behavior

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

When V1 extraction declares that a new memory supersedes an older item, Code
marks the older item as superseded and protects it from pruning. Recall filters
the archived item, but the original content and replacement link remain
available for audit.

## Host wiring

```rust,no_run
use a3s_code_core::{DurableMemorySession, SessionOptions};
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

During session preparation, Code fills an omitted `tenant_id` and `principal`
from the exact namespace. If the caller supplies either field explicitly, it
must match the namespace or session construction fails. The scope is never
inferred from a path or a backend name; the host selects it explicitly.

`DurableMemorySession` contains a live repository object and is intentionally
not serialized in a session snapshot. On restore, the host must reconstruct the
repository and inject the same exact namespace. Code never resolves `latest`,
opens an implicit repository, or substitutes a global principal.

## Evidence and privacy

The V2 node stores a reference and SHA-256 digest, not the turn body. The digest
binds the candidate to the normalized extraction input, while the host remains
responsible for retaining any source material required by its audit policy.
Because shadow candidates are not admitted to context, an unavailable source
cannot silently become a serving memory. Future activation must validate the
evidence and record a separate admission event.

## Migration sequence

1. Run shadow mode beside the existing V1 serving path.
2. Evaluate candidate precision, duplicate rate, namespace isolation, evidence
   availability, and restart replay.
3. Introduce an explicit activation policy with optimistic revisions; do not
   infer activation from LLM confidence alone.
4. Enable bounded active-only recall and record admission/use events for the
   exact node revision included in context.
5. Move consolidation and retention to a lifecycle owner with cancellation,
   close, and health reporting.
6. Add semantic vectors only after lexical and relation-aware evaluation shows
   a measured retrieval gap.

## Verification

Run checks from the Code crate workspace, not the monorepo root:

```text
cargo test -p a3s-code-core --lib durable_memory
cargo test -p a3s-code-core --test durable_memory_shadow
cargo test -p a3s-code-core --lib
```
