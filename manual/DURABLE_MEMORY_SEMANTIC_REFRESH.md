# Durable Memory Semantic Refresh

`DM-REFRESH1` qualifies the explicit lifecycle boundary between the authoritative
A3S Memory repository and a caller-owned semantic vector index. `DM-CAS1` adds
revision-conditional publication for independently constructed runtimes sharing
one capable index. `DM-SCHED1` adds an opt-in Code-owned periodic lifecycle for
that same verified operation. `DM-SKIP1` lets that owned schedule suppress
redundant embedding and publication only after proving the source and index are
unchanged. `DM-REUSE1` lets a required rebuild reuse exact embeddings already
committed and verified in the current ownership epoch. `DM-OBS1` exposes the
bounded work performed by every scheduled attempt. None of these gates adds a
distributed lease or moves embedding policy into the repository.

## Contract

A host invokes `DurableMemorySession::refresh_semantic_recall` for one exact
bound namespace. One successful call performs this sequence:

1. Acquire the live semantic generation's refresh lock. Cloned sessions and
   direct namespace replacement share the same lock. Read the backend's typed
   mutation consistency and, for `index_revision_cas`, capture the global index
   revision before repository or embedding work.
2. Request the complete current `Active` view from `MemoryRepository` under
   node and canonical-payload byte budgets derived from the bound embedding
   execution policy.
3. Recompute the snapshot shape, deterministic ordering, byte count, and
   domain-separated SHA-256 identity. A custom repository's claimed digest is
   not trusted.
4. Embed the complete snapshot off-index and atomically replace the exact
   namespace plus semantic-generation partition. A CAS-capable backend compares
   the captured revision at the same linearization point as publication.
5. Read and verify the repository snapshot again. If it changed while vectors
   were being prepared, require invalidation of the published partition and
   fail closed. CAS cleanup expects the revision returned by publication, so a
   delayed cleanup cannot remove a newer generation. Invalidation failures are
   propagated rather than hidden.
6. Return a secret-free receipt containing the refresh profile, source snapshot
   profile/digest/bytes, semantic binding schema, serving-generation digest,
   Active node count, mutation consistency, and published vector-index status.

An over-budget snapshot, cancellation before publication, repository failure,
forged snapshot response, embedding failure, or vector replacement failure
returns no success receipt. Failures before atomic replacement preserve the
previous complete partition. Source drift discovered after replacement attempts
mandatory partition invalidation and never returns a success receipt. If the
vector backend cannot invalidate it, that cleanup error is returned; query-time
repository re-verification still prevents the residual stale vectors from
authorizing stale memory.

`refresh_semantic_recall` accepts the source-compatible `partition_atomic`
minimum and automatically uses stronger CAS when the backend advertises it.
Production hosts that require independently prepared writers to be fenced call
`refresh_semantic_recall_requiring(VectorMutationConsistency::IndexRevisionCas,
...)`. A weaker or unknown backend fails before snapshot construction or model
egress. The CAS precondition is the global index revision, so unrelated
partition churn can conservatively reject publication or cleanup; the host may
retry from a fresh source snapshot.

Direct `refresh_semantic_recall` calls remain unconditional. Change detection is
an owned-schedule optimization because its proof depends on the latest receipt
from the current schedule-ownership epoch. Direct calls also remain uncached;
embedding reuse is owned by the same bounded scheduled lifecycle as its receipt.

Publication is the commit point. Cancellation after atomic replacement does
not interrupt the required post-publication source verification and cleanup.
Dropping a directly invoked Rust future can still interrupt any future; the
direct host must await it. The owned schedule cancels the run token on close but
keeps the job future alive until it settles or the configured total shutdown
deadline forces an observable abort.

## Scheduled lifecycle

`ScheduledSemanticRefresh::try_new(interval)` is inert. Installing it through
`MemoryMaintenanceOptions::with_semantic_refresh` and building an asynchronous
session starts one worker after validating the exact durable-memory binding,
attached semantic generation, and `index_revision_cas` backend. A missing or
weaker binding fails session construction before repository or model I/O.

The first refresh waits one full host-selected interval. Runs never overlap and
missed ticks are skipped rather than replayed in a burst. Generic maintenance
health exposes the reserved `v2_semantic_refresh` worker, run/failure counts,
affected Active-node counts, and bounded errors. Clones of the schedule share
`last_receipt()`, which retains the latest successful secret-free receipt; a
later failure leaves that evidence intact for host inspection. Durable receipt
storage remains a host responsibility. One cloned schedule family can have only
one active maintenance owner, keeping that receipt attributable. Clean close
releases the claim deterministically; an unclosed runtime keeps its lease until
every aborted worker has actually settled.

The first run always performs a full verified refresh. On a later tick, Code
captures the CAS revision before reading and recomputing the complete bounded
Active snapshot, then reads the full index status after the snapshot. Embedding
and publication are skipped only when all of these values exactly match the
current ownership-epoch receipt: refresh and snapshot profiles, source digest
and byte count, Active-node count, semantic binding schema and serving generation,
mutation consistency, CAS revision, and complete index status. This revision
sandwich proves one interval in which both source and index still matched the
receipt. A weaker `partition_atomic` backend cannot establish that proof and
never skips.

A verified no-change tick is a successful maintenance run with zero affected
items and leaves `last_receipt()` unchanged. Any source, generation, receipt, or
index drift falls back to the full refresh and its existing post-publication
verification. When a replacement runtime successfully claims the schedule, it
starts a new ownership epoch and clears the process-local receipt before the
first tick; it therefore cannot use evidence from a different injected backend.

When a full publication is required, the active owner may reuse the vector for
an exact semantic record ID from its latest verified success. That ID binds the
namespace partition, serving generation, node identity, node revision, and
content digest. Code does not infer reuse from similar text, a digest alone, or
an index hit. It reconstructs every current record and label from the complete
verified source snapshot, embeds only cache misses, and still CAS-publishes the
entire partition atomically. Consequently, unrelated index drift can rebuild
with zero provider inputs, a one-node source change sends only that node, and an
Active-node removal can rebuild entirely from retained vectors.

The cache contains no source text and retains at most the current schedule's one
semantic partition. Its record count and vector payload remain bounded by the
same refresh input and vector-byte budgets. A candidate cache replaces the old
one only after CAS publication and mandatory post-publication source/index
verification succeed. Provider, CAS, cleanup, or source-verification failure
retains the previous verified cache; prepared but uncommitted vectors are not
promoted. Clean close releases the vector cache before making the next owner
claimable while preserving `last_receipt()` for host inspection. A new owner
always starts without cached vectors or a receipt.

## Operational metrics

Cloned schedule handles expose `metrics()` independently from generic
maintenance health. A never-owned schedule reports ownership epoch zero. The
first successful claim increments the epoch; clean close retains its metrics for
inspection, while a replacement claim increments the epoch again and clears all
prior counters and recent runs before any repository or provider work.

Every settled published, verified-unchanged, or failed attempt contributes one
bounded `SemanticRefreshRunMetrics` observation. `SemanticRefreshMetrics`
retains saturating cumulative counters and at most the latest 64 observations.
They measure:

- elapsed refresh time;
- source snapshot requests plus materialized node and canonical-payload bytes,
  including both pre- and post-publication verification reads;
- logical embedding cache hits, misses presented to the bounded executor, and
  miss text bytes;
- provider-adapter invocations, input count, and UTF-8 input bytes, counting
  the same input again when a retry reaches the adapter boundary;
- complete-partition publication calls actually attempted and their record
  counts. A CAS rejection counts; later invalidation cleanup does not.

Metrics do not retain source or query text, node/record IDs, namespace or
generation digests, vectors, provider identity, credentials, or provider/error
bodies. Failed adapter invocations remain visible even when no vector or
receipt is returned. These counters prove work reached Code's provider-adapter
boundary; they do not prove that the adapter transmitted a remote request or
incurred a charge. A lost CAS publication is an attempted publication, but its
prepared embedding still cannot become the verified cache. Direct explicit
refresh remains outside this schedule-owned epoch and does not mutate these
metrics. A force-aborted non-cooperative job cannot publish a terminal run
observation because its future never settles; generic maintenance close
reporting remains the evidence for that bounded-abort path.

## Ownership

A3S Memory owns complete bounded snapshot construction and identity. Code owns
embedding execution, single-live-generation serialization, atomic publication,
post-publication reconciliation, and the receipt. The host still owns:

- whether to refresh directly or install a schedule, and at what interval;
- repository, provider, and vector-index construction;
- credentials and memory/query egress authorization;
- durable receipt storage and alerting;
- selecting and operating any distributed lease policy in addition to revision
  CAS;
- remote-vector durability, failover, and disaster recovery.

The refresh lock is shared by clones of one `DurableMemorySemanticRecall`; it is
not a remote lease. Revision CAS separately orders independently constructed
runtimes only when they share a backend that truthfully implements the A3S
Memory CAS contract and every semantic writer uses conditional mutation. It
does not prove remote durability, lease retention, or backend failover. Every
semantic query continues to re-read the current repository node and verify
status, revision, and content digest, so an unavailable or stale index can
reduce recall but cannot authorize stale memory.

## Deterministic evidence

Run from the Code crate workspace:

```text
cargo test -p a3s-code-core --test durable_memory_semantic_refresh
cargo test -p a3s-code-core --test durable_memory_semantic_refresh_cas
cargo test -p a3s-code-core --test durable_memory_semantic_refresh_failure
cargo test -p a3s-code-core --test memory_semantic_refresh_schedule
cargo test -p a3s-code-core --test memory_semantic_refresh_change_detection
cargo test -p a3s-code-core --test memory_semantic_refresh_metrics
cargo test -p a3s-code-core --test memory_maintenance_close
```

The contracts cover complete Active-only publication, Candidate exclusion,
revision and status replacement, node/byte-budget failure, source drift cleanup,
serialized cloned-session refreshes, strict consistency rejection before I/O,
delayed independent publication and cleanup races, provider failure preserving
the previous partition, forged snapshot rejection, receipt identity, and public
`Send + Sync` behavior. The schedule contracts additionally cover strict
pre-spawn admission, repeated refresh, retained receipts, clean
post-publication close settlement, verified unchanged-tick embedding/publication
suppression, source- and index-drift rebuilds, ownership-epoch receipt clearing,
exact committed-vector reuse across index drift, partial source change and
Active removal, rejection of prepared vectors after a lost CAS race, cache
release on owner close, exact settled successful/unchanged/failed work
accounting, provider-boundary retry accounting, bounded redacted retention,
ownership-epoch reset, and bounded abort for a non-cooperative job. A3S
Memory's contracts
separately cover deterministic
snapshot identity, byte and node overflow, restart-stable digests, exactly one
winner under concurrent CAS, stale replacement/cleanup rejection, and
source-compatible custom-backend defaults.

## Non-claims

These gates do not qualify a distributed generation lease, a durable remote CAS
vector backend, a real embedding provider, production refresh cadence, large
independently labeled corpora, latency, billed cost, or refresh behavior during
remote failover. An unchanged tick still pays for a bounded repository snapshot
and digest verification. The metrics make representative distributions
measurable; deterministic fixture observations do not establish production
cache-hit, latency, or billed-cost distributions. Hosts must correlate the
adapter-boundary counters with provider telemetry to establish transmission or
billing. Those remain `DM-PROD1` host qualifications.
