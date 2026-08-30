# Durable Memory Semantic Refresh

`DM-REFRESH1` qualifies the explicit lifecycle boundary between the authoritative
A3S Memory repository and a caller-owned semantic vector index. `DM-CAS1` adds
revision-conditional publication for independently constructed runtimes sharing
one capable index. Neither gate adds a background scheduler, distributed lease,
or embedding policy to the repository.

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

Publication is the commit point. Cancellation after atomic replacement does
not interrupt the required post-publication source verification and cleanup.

## Ownership

A3S Memory owns complete bounded snapshot construction and identity. Code owns
embedding execution, single-live-generation serialization, atomic publication,
post-publication reconciliation, and the receipt. The host still owns:

- when refresh is requested;
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
```

The contracts cover complete Active-only publication, Candidate exclusion,
revision and status replacement, node/byte-budget failure, source drift cleanup,
serialized cloned-session refreshes, strict consistency rejection before I/O,
delayed independent publication and cleanup races, provider failure preserving
the previous partition, forged snapshot rejection, receipt identity, and public
`Send + Sync` behavior. A3S Memory's contracts separately cover deterministic
snapshot identity, byte and node overflow, restart-stable digests, exactly one
winner under concurrent CAS, stale replacement/cleanup rejection, and
source-compatible custom-backend defaults.

## Non-claims

These gates do not qualify a periodic scheduler, a distributed generation
lease, a durable remote CAS vector backend, a real embedding provider, large
independently labeled corpora, latency, billed cost, or refresh behavior during
remote failover. Those remain `DM-PROD1` host qualifications.
