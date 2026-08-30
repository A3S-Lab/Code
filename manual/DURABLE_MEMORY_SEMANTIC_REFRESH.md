# Durable Memory Semantic Refresh

`DM-REFRESH1` qualifies the explicit lifecycle boundary between the authoritative
A3S Memory repository and a caller-owned semantic vector index. `DM-CAS1` adds
revision-conditional publication for independently constructed runtimes sharing
one capable index. `DM-SCHED1` adds an opt-in Code-owned periodic lifecycle for
that same verified operation. `DM-SKIP1` lets that owned schedule suppress
redundant embedding and publication only after proving the source and index are
unchanged. `DM-TOKEN1` uses A3S Memory's optional exact namespace token to
remove redundant source snapshots from that proof. `DM-REUSE1` lets a required
rebuild reuse exact embeddings already committed and verified in the current
ownership epoch. `DM-OBS1` exposes the bounded work performed by every
scheduled attempt. `DM-RECOVER1` lets a host persist secret-free recovery
evidence without trusting a token from another repository history.
`DM-SQLITE1` proves that the same recovery contract survives a real close and
reopen of A3S Memory's local SQLite vector backend. `DM-QUAL1` adds a retained
release profile over 10,000 durable source nodes and 384-dimensional SQLite
vectors. None of these gates adds a distributed lease or moves embedding policy
into the repository.

## Contract

A host invokes `DurableMemorySession::refresh_semantic_recall` for one exact
bound namespace. One successful call performs this sequence:

1. Acquire the live semantic generation's refresh lock. Cloned sessions and
   direct namespace replacement share the same lock. Read the backend's typed
   mutation consistency and, for `index_revision_cas`, use the fallible
   asynchronous index observation to capture the global revision before
   repository or embedding work.
2. Read the repository's optional exact namespace change token, then request
   the complete current `Active` view under node and canonical-payload byte
   budgets derived from the bound embedding execution policy.
3. Recompute the snapshot shape, deterministic ordering, byte count, and
   domain-separated SHA-256 identity. A custom repository's claimed digest is
   not trusted. When the first token was present, read it again and reject
   drift before embedding work begins.
4. Embed the complete snapshot off-index and atomically replace the exact
   namespace plus semantic-generation partition. A CAS-capable backend compares
   the captured revision at the same linearization point as publication.
5. Read the exact token again. Equality proves that the namespace did not
   change while vectors were prepared and avoids a second materialized
   snapshot. If the token is unavailable, retain the original complete
   post-publication snapshot comparison. Detected drift requires invalidation
   of the published partition and fails closed. CAS cleanup expects the
   revision returned by publication, so delayed cleanup cannot remove a newer
   generation. Invalidation failures are propagated rather than hidden.
6. Return a secret-free receipt containing the refresh profile, source snapshot
   profile/digest/bytes, optional content-free source change token, semantic
   binding schema, serving-generation digest, Active node count, mutation
   consistency, optional exact vector-index history token, and published
   vector-index status. Status and token come from one exact post-publication
   observation rather than separate synchronous reads.

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
from the current schedule-ownership epoch. A direct call may use a stable token
only to replace its second source snapshot; it still builds one complete
snapshot and publishes. Direct calls also remain uncached; embedding reuse is
owned by the same bounded scheduled lifecycle as its receipt.

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

The first run always constructs one complete verified Active snapshot. With a
token-capable repository it brackets that snapshot with equal token reads,
publishes, and verifies a third equal token instead of materializing a second
snapshot. A repository returning `None` retains the original pre- and
post-publication snapshot proof.

On a later tick, Code captures the CAS revision before reading the exact token,
then reads the full index status afterward. Embedding, snapshot construction,
and publication are skipped only when the token and these ownership-epoch
receipt fields match exactly: semantic binding and serving generation, mutation
consistency, CAS revision, and complete index status. This revision sandwich
proves one interval in which both source and index still matched the receipt. A
weaker `partition_atomic` backend cannot establish that proof and never skips.

If the token changed, Code constructs and verifies one complete bounded Active
snapshot. A stable Active projection can return an unchanged run while advancing
the receipt token, so an inactive-only namespace change costs one snapshot but
no embedding or publication; the following stable tick returns to the
zero-snapshot path. Token drift around that snapshot fails before provider work.
Drift after publication conditionally invalidates the published revision and
never promotes a receipt. Capability loss falls back to the full snapshot proof.

A verified no-change tick is a successful maintenance run with zero affected
items. It usually retains `last_receipt()` exactly and may update only its source
token after an inactive-only change. Any source, generation, receipt, or index
drift falls back to the full refresh and its existing publication verification.
When a replacement runtime successfully claims the schedule, it starts a new
ownership epoch and clears the process-local receipt before the first tick; it
therefore cannot implicitly use evidence from a different injected backend. A
host may explicitly provide the validated checkpoint described below.

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
always starts without cached vectors or an observable receipt, including when it
has unverified checkpoint recovery evidence.

## Checkpoint recovery

`DurableMemorySemanticRefreshReceipt::checkpoint()` produces a versioned,
serializable `DurableMemorySemanticRefreshCheckpoint`. The host owns its durable
storage and supplies a decoded value through
`ScheduledSemanticRefresh::try_new_with_checkpoint(interval, checkpoint)`.
Deserialization rejects unknown top-level fields, and schedule construction
verifies all profiles, canonical digests, snapshot bounds, mutation consistency,
index status, and token revision before a worker can start.

The checkpoint excludes the namespace change token. That token is exact only
inside one repository history, so persisting it could let unrelated repositories
with the same local sequence collide. Recovery instead always reads and verifies
one complete bounded Active snapshot. It then compares the source and semantic
generation with the checkpoint and sandwiches the current full index status
with the index revision and A3S Memory's exact vector-index history token.

When every value matches, the first recovered run is verified unchanged: it
performs one snapshot read but no provider-adapter invocation or vector
publication, promotes a current-epoch receipt, and lets the next stable tick use
the ordinary zero-snapshot namespace-token path. `last_receipt()` remains `None`
until that proof succeeds. A changed source, unrelated repository history,
different vector history, colliding revision/count/byte status, missing vector
token, or any other mismatch performs the complete safe rebuild. A failed
attempt retains the checkpoint only as unverified recovery evidence for retry.

The built-in in-memory index preserves its history token across clones of the
same live object. Constructing a new index creates a new history and therefore
rebuilds. With Code's `durable-memory-sqlite` feature, A3S Memory's
`SqliteVectorIndex` persists the token, descriptor, revision, and content in one
local database; the deterministic gate closes the first handle, reopens the
database, and proves unchanged recovery without provider or publication work.
A copied or atomically replaced closed database forks its token on Unix and
Windows; backup restore must replace the database file instead of overwriting
it in place. Concurrent out-of-band file operations are unsupported.
A durable remote backend can provide the same capability only by persisting one
token across the same linear mutation history and issuing a new history digest
after recreation, rollback, or divergent restore.

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
- source change-token requests and valid `Some` observations; an unsupported
  `None` response counts only as a request;
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

## Local release qualification

`durable_memory_semantic_refresh_benchmark` is the `DM-QUAL1` operational gate.
It constructs 10,000 Active nodes in A3S Memory's synchronized
`FileMemoryRepository`, refreshes a 384-dimensional `SqliteVectorIndex` through
the real owned schedule, persists and synchronizes a checkpoint, closes every
repository/index/runtime owner, and reopens both durable backends. Its exact
work assertions require:

- an initial complete publication with all 10,000 embedding inputs;
- a stable tick with zero snapshot, embedding, or publication work;
- one corrected node to produce 9,999 exact cache hits, one provider input,
  and one complete 10,000-record atomic publication;
- index-only drift to produce 10,000 cache hits, zero provider inputs, and one
  complete publication;
- the first recovered tick to read one complete source snapshot but perform
  zero provider and publication work; and
- the next stable recovered tick to return to the zero-snapshot fast path.

The release report also retains synchronized source and SQLite disk bytes,
logical vector bytes, clean-close evidence, Linux active/retained RSS, and warm
`DurableMemorySession::preview_recall` p50/p95/max over three warmups and 20
measured samples. The recall fixture has no lexical overlap with its target and
must rank that target first through the semantic channel. The p95 ceiling is a
regression budget for the fixed local workflow runner, not a remote or product
SLA. Setup, one-off refresh elapsed observations, and backend reopen time are
reported separately rather than folded into query percentiles.

The embedding adapter is deterministic and in process. Its counters prove the
Code adapter boundary and cache behavior, but the profile includes no provider
network, real model, billing, operating-system process restart, remote vector
service, or distributed lease. The Performance Qualification workflow retains
the JSON output as one of its fail-closed release artifacts.

## Ownership

A3S Memory owns complete bounded snapshot construction and identity, the
optional namespace token's same-repository-history linearization contract, and
the optional vector token's exact index-history contract. Code owns embedding
execution, single-live-generation serialization, atomic publication,
post-publication reconciliation, checkpoint validation, and ownership-epoch
receipt promotion. The host still owns:

- whether to refresh directly or install a schedule, and at what interval;
- repository, provider, and vector-index construction;
- local SQLite path, permissions, encryption, retention, backup, and deletion;
- credentials and memory/query egress authorization;
- durable checkpoint/receipt storage and alerting;
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
cargo test -p a3s-code-core --test memory_semantic_refresh_change_token
cargo test -p a3s-code-core --test memory_semantic_refresh_checkpoint
cargo test -p a3s-code-core --test memory_semantic_index_observation
cargo test -p a3s-code-core --features durable-memory-sqlite --test memory_semantic_refresh_sqlite
cargo test -p a3s-code-core --test memory_semantic_refresh_metrics
cargo test -p a3s-code-core --test memory_maintenance_close
cargo run --locked --release -p a3s-code-core --features durable-memory-sqlite --example durable_memory_semantic_refresh_benchmark
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
zero-snapshot stable ticks, one-snapshot stable rebuilds, source-compatible
two-snapshot fallback, pre-provider token-drift rejection, post-publication
conditional cleanup, inactive-only receipt advancement,
host-persisted checkpoint validation, one-snapshot unchanged recovery,
repository- and vector-history collision rejection, missing-token fallback,
exact asynchronous observation despite stale synchronous status hints, local
SQLite close/reopen checkpoint continuity without duplicate provider work,
exact committed-vector reuse across index drift, partial source change and
Active removal, rejection of prepared vectors after a lost CAS race, cache
release on owner close, exact settled successful/unchanged/failed work
accounting, provider-boundary retry accounting, bounded redacted retention,
ownership-epoch reset, and bounded abort for a non-cooperative job. A3S
Memory's contracts
separately cover deterministic
snapshot identity, byte and node overflow, restart-stable digests, exactly one
winner under concurrent CAS, stale replacement/cleanup rejection, and
source-compatible custom-backend defaults, exact namespace-token advancement
and restart reconstruction, and exact vector-history token continuity without
raw backend identity disclosure.

## Non-claims

These gates do not qualify a distributed generation lease, a durable remote CAS
vector backend, a real embedding provider, deployment-selected production
cadence, independently labeled quality corpora, remote/provider latency, billed
cost, long-horizon consolidation, or refresh behavior during remote failover.
An unchanged tick on a repository without the optional token
still pays for a bounded snapshot and digest verification; built-in repositories
avoid that snapshot but still pay for token and index-status reads. The metrics
make representative distributions measurable. A checkpoint does not itself
prove vector-backend continuity: a backend without a durable exact history token
rebuilds, and the in-memory token applies only to the retained index object and
its clones. Local close/reopen skipping is qualified only for the injected
SQLite implementation; the release profile explicitly does not restart the
operating system process. Remote skipping remains conditional on a separately
qualified durable backend. Deterministic fixture observations establish the
locked local work-amplification and SQLite query profile only; they do not
establish production model quality, cache-hit, remote-latency, or billed-cost
distributions. Hosts must correlate the adapter-boundary counters with provider
telemetry to establish transmission or billing. Those remain `DM-PROD1` host
qualifications.
