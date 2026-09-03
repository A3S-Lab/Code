# Workspace Retrieval A3S Vec Migration

Status: lexical engine cutover delivered; semantic Vec-primary remains a gated
developer preview, 2026-09-03.

This document defines the A3S Code integration gates for A3S Vec. Semantic
vector migration is still differential evidence, not a stable serving-backend
promotion. Code commit `788bc61a458cafe3c6809a65d9e1e8c733a97a2e` introduced
the gated Vec-primary authority slice; the current dependency-validation pin is
Code `708a85e3ac070640ca5fb8173d0b06e6070152e7` with A3S Vec commit
`416140ec5f9bd6fc8030f9f17735c0b10d099c99` (`a3s-vec` 0.1.1).

Workspace retrieval consumes a Code-owned `WorkspaceVectorIndex` contract. The
legacy Memory trait is bound only inside the compatibility adapter, so new
semantic runtime code cannot grow direct dependency-owned method calls. The
adapter remains available for the compatibility default and rollback evidence
until the semantic promotion decision is complete.

## Authority contract

`A3sMemory` is the compatibility default. A trusted host may explicitly select
the gated `A3sVec` preview through the typed `WorkspaceVectorEngine` option.
Exactly one selected engine is authoritative for each session: its result
determines hits, ordering, scores, searched-record counts, truncation,
fallbacks, and errors. The other engine is a differential shadow; it may
collect evidence and degrade its own status, but it cannot change serving
decisions or silently fail over.

One admitted embedding response follows this path:

```text
Embedding Provider
        |
        | one validated VectorRecord batch
        v
Code publication gate
        |----------------------|
        v                      v
selected authority       differential shadow
(Memory or Vec)          (the other engine)
        |                      |
        | serving result       | comparison evidence
        |----------------------|
                   |
                   v
        compare, record counters,
        return selected result unchanged
```

Code does not call the Embedding Provider a second time. It clones the same
already-admitted records only after the provider response has passed the
existing dimension, finiteness, normalization, identity, byte, generation,
and cancellation checks.

## Lexical authority

The workspace lexical path now has one implementation: A3S Vec FTS with the
`whitespace` tokenizer over Code's normalized identifier/CJK token stream. The
incremental per-file catalog and the bounded scanner used when a custom backend
cannot provide a manifest catalog both construct temporary, manual-durability
Vec collections. Code still owns candidate-file policy, chunk boundaries, path
filters, source reads, and result rendering; it no longer computes BM25
postings or scores locally. The collection is discarded with the query or
catalog generation and is never a durable SQLite/`sqlite-vec` workspace index.

The public result shape remains `algorithm: "bm25"` for compatibility. Metadata
identifies `engine: "a3s_vec_fts"` and `tokenizer: "whitespace"`, so operators
can distinguish the engine without relying on a primitive backend selector.
The locked nine-query fixture, identifier/CJK cases, glob/path limits, and both
catalog and query-time paths must remain equal to the frozen baseline.

## Session and storage ownership

Each retrieval-enabled session owns one selected primary index and one
differential shadow when initialization succeeds. The Memory-default mode owns
a Memory primary plus a Vec collection; the gated Vec-primary mode owns a Vec
collection plus a Memory shadow. The Vec collection is created below an
operating-system temporary directory with manual durability. It is not shared,
checkpointed, reopened, or exposed as a vector-database service. Closing the
session clears and closes the collection before releasing the temporary
directory.

The normal close path removes the temporary directory. A process crash can
leave operating-system temporary residue, so hosts must apply their normal
temporary-directory permissions, encryption, retention, and scavenging policy.
No source text, query text, provider credentials, or model response body is
written into Code diagnostics. The Vec documents contain record identity,
partition identity, and embeddings for the life of the session.

## Record mapping

The adapter owns a fixed internal schema:

| Field | Vec type | Purpose |
| --- | --- | --- |
| `record_id` | string | Original Memory record ID |
| `partition` | string | Original Memory partition |
| `partition_key` | string | SHA-256 of the partition for bounded filter construction |
| `embedding` | fp32 vector | Unit-normalized vector used with inner product |

The Vec primary key is the lowercase hexadecimal encoding of the partition,
followed by `!`, followed by the lowercase hexadecimal encoding of the record
ID. This preserves the Memory `(partition, id)` lexical tie order without
placing raw path text in the primary key or filter expression.

Both document and query vectors are normalized with the same f64 norm
calculation and converted back to f32 before Vec inner-product search. The
differential comparator requires identical record IDs, partitions, f32 score
bits, hit count, searched-record count, and truncation state. Workspace records
currently carry no labels; a labeled request or record is unsupported by this
shadow and degrades only shadow evidence.

Partition filters use only SHA-256 keys and are capped at 64 KiB. Raw
partition values are never interpolated into a Vec filter. An over-budget
filter records a typed `filter_budget` shadow failure and leaves the Memory
query result unchanged.

## Publication and failure isolation

One Code-owned asynchronous read/write gate spans both engines:

- replace, revision-checked replace, remove, revision-checked remove, clear,
  and close acquire the write side;
- search acquires the read side, runs Memory and Vec concurrently, then records
  the comparison;
- the selected primary mutates first and remains authoritative;
- Memory-default sessions mirror into Vec; Vec-primary sessions mirror into
  Memory;
- Vec replacement fetches the previous partition, deletes it, inserts the new
  documents, and attempts rollback if insertion fails;
- The Vec adapter exposes the same global revision-CAS contract as the Memory
  implementation. Its temporary collection may use multiple physical writes
  for a replacement, but the Code-owned adapter publishes one logical revision
  and rejects a stale writer before touching the collection;
- synchronous Vec work runs in Tokio's blocking pool. The owned operation guard
  moves into that blocking operation so cancelling an async waiter cannot let
  close or a later mutation overtake work that is still executing.

Vec initialization, mutation, query, worker, filesystem, filter, unsupported
label, rollback, or write-rejection failures increment only bounded counters
and set `vec_shadow.phase = degraded`. In Vec-primary mode, a primary failure is
returned to the caller and never silently replaced by Memory; in Memory-default
mode, a Vec shadow failure leaves the Memory result available. Logs contain a
static failure code, not the path, query, vector, document, or backend error
body. A mismatch degrades the shadow and leaves the selected primary result
unchanged.

## Resource contract

`max_records` and `max_bytes` are applied to the selected primary and its
shadow descriptor. Vec additionally enforces the same ceilings as collection
document, query-candidate, write-batch, and accounted-byte limits. Its storage
layout is different from Memory's, so `accounted_bytes` is a Vec logical
estimate and is not expected to equal Memory's `byte_count`.

`accounted_bytes` is not process RSS, committed virtual memory, or temporary
directory size. Promotion requires separate RSS, disk, and latency
qualification. A shadow resource-limit failure cannot weaken the Memory
limit or permit additional serving records.

## Public status

`WorkspaceRetrievalStatus` adds two backward-compatible fields:

- `active_vector_engine` / `activeVectorEngine` is `a3s_memory` by default or
  `a3s_vec` for an explicitly selected preview, and absent for a disabled
  runtime;
- `vec_shadow` / `vecShadow` reports `phase`, `revision`, `record_count`,
  `accounted_bytes`, initialization and mutation outcomes, and compared,
  matching, mismatched, and failed query counts.

Rust uses `active_vector_engine` and `vec_shadow`. Node.js uses
`activeVectorEngine` and `vecShadow`. Python dictionaries use
`active_vector_engine` and `vec_shadow`. Go exposes `ActiveVectorEngine` and
`VecShadow` while its bridge JSON remains snake_case. Legacy serialized Rust
status without these fields loads with no active engine and a disabled shadow.

The selector is typed at every supported boundary: Rust uses
`WorkspaceVectorEngine::A3sMemory`/`A3sVec`, Node uses
`WorkspaceVectorEngineOption.A3sMemory`/`A3sVec`, Python uses the native
`WorkspaceVectorEngineOption` enum, and Go uses
`WorkspaceVectorEngineA3SMemory`/`WorkspaceVectorEngineA3SVec`. Raw backend
name strings are rejected by the native option paths.

Required operator invariants are:

- `active_vector_engine` equals the requested typed engine;
- `matching_queries == compared_queries`;
- `mismatched_queries == 0` and `failed_queries == 0` before promotion;
- `record_count == vector_records` after a ready generation when no shadow
  failure has occurred (the count describes the selected primary in Vec mode);
- both authoritative and shadow record/byte counts are zero after close.

## Qualification evidence

The earlier 2026-09-02 Windows x86-64 qualification used release benchmark
report schema 4 and profile `workspace-retrieval-v3`:

| Gate | Result | Limit |
| --- | ---: | ---: |
| Exact 25,000 x 384 top-20 p95 | 6.7343 ms | <= 30 ms |
| Hybrid RRF p95 | 50.7850 ms | <= 100 ms |
| Hybrid deterministic-rerank p95 | 49.7348 ms | <= 100 ms |
| Vec records per hybrid arm | 25,000 | exactly 25,000 |
| Vec accounted bytes per hybrid arm | 54,500,008 | reported, not an RSS claim |
| Vec revision / successful mutations | 196 / 196 | no failed mutation |
| Vec comparisons per hybrid arm | 120/120 matching | zero mismatch/failure |
| Closed authoritative and shadow state | zero records and bytes | required |

The 2026-09-03 local revision-bound rerun below is historical evidence for the
previous Vec `13585ccd3f956f6cb7d669b2ee6acc7096fca03d` candidate and Code
`708a85e3ac070640ca5fb8173d0b06e6070152e7` on the reference Windows x86-64
host (20 logical CPUs; 25,000 records, 384 dimensions, top-20, 100 measured
queries, 20 warmups). The revision-bound local profile reports:

| Gate | Result | Limit |
| --- | ---: | ---: |
| Exact p95 (Memory / Vec-primary) | 7.1010 / 9.4502 ms | <= 30 ms |
| Hybrid RRF-only p95 (Memory / Vec-primary) | 67.9456 / 85.1365 ms | <= 100 ms |
| Hybrid deterministic-rerank p95 (Memory / Vec-primary) | 63.9634 / 82.0097 ms | <= 100 ms |
| Deterministic reranker p95 delta (Memory / Vec-primary) | 0.0000 / 0.0000 ms | <= 10 ms positive addition |
| Vec records per hybrid arm | 25,000 | exactly 25,000 |
| Vec accounted bytes per hybrid arm | 54,500,008 | reported, not an RSS claim |
| Vec revision / successful mutations | 196 / 196 | no failed mutation |
| Vec comparisons per hybrid arm | 120/120 matching | zero mismatch/failure |
| Closed authoritative and shadow state | zero records and bytes | required |

Both hybrid arms compared 120/120 queries with zero mismatches, failed
queries, initialization failures, or failed mutations. The Memory-primary and
Vec-primary workspace-build measurements were 3,359.14 ms and 3,880.98 ms;
the Vec-primary logical vector accounting was 54,500,008 bytes versus
40,177,548 bytes for Memory-primary. These values are process/run observations,
not SLOs. `accounted_bytes` remains a logical Vec estimate; it is not process
RSS, committed virtual memory, or temporary-directory size. The hosted
revision-bound refresh is recorded below; neither hosted nor local numbers
replace the actual macOS 12 Intel runtime gate.

Additional local gates for the current dependency pin passed:

- locked Core compilation;
- all three `workspace_retrieval` library-filter tests;
- all ten `vec_shadow` library-filter tests;
- all six enabled `vec_primary` library-filter tests (the long-running soak is
  explicitly ignored in the local filter and runs in hosted qualification);
- repository-wide Rust formatting; and
- both release profiles shown above, including 120/120 differential matches in
  each hybrid arm and complete resource release.

The revision-bound A3S Vec CI run
[`33772179017`](https://github.com/A3S-Lab/Vec/actions/runs/33772179017)
passed MSRV 1.75, Linux x86-64/ARM64, Windows x86-64, macOS ARM64/Intel hosted,
format, lint, docs, feature, and fuzz jobs. Actual macOS 12 Intel hardware or an
equivalent external runner remains a release gate; the hosted Intel job is not
claimed as that evidence.

The Code validation workflows for the current architecture pin are recorded in
the complete CI matrix
[`33773373773`](https://github.com/A3S-Lab/Code/actions/runs/33773373773) and the
successful release-profile qualification
[`33773373522`](https://github.com/A3S-Lab/Code/actions/runs/33773373522).

The latest hosted release-profile artifact ran on Linux x86-64 (4 logical
CPUs, 25,000 records, 384 dimensions, top-20, 100 measured queries and 20
warmups). Memory-primary reported exact p95 9.7993 ms, hybrid RRF-only p95
55.6142 ms, and deterministic-rerank p95 58.2083 ms. The explicit Vec-primary
preview reported exact p95 9.6626 ms, hybrid RRF-only p95 58.6252 ms, and
deterministic-rerank p95 56.6000 ms. Both profiles matched 120/120
comparisons with zero mismatches, failed queries, failed mutations, or
initialization failures and closed with zero records and bytes. Vec-primary
workspace construction was 2,284.8 ms versus 2,212.2 ms for Memory-primary,
and its logical vector bytes were 54,500,008 versus 40,177,548; these are
directional logical measurements, not process RSS or a cross-platform SLO.

The full Code workspace strict rustdoc command (`cargo doc --locked
--workspace --all-features --no-deps` with `RUSTDOCFLAGS=-D warnings`) passed on
the current candidate. This includes the migration adapter and its public SDK
facades; no rustdoc diagnostic is suppressed for the Vec integration.

For historical context, an earlier local run used the explicit Vec-primary
selector on a Windows x86-64 host (25,000 records, 384 dimensions, top-20,
100 measured queries, 20 warmups). Those values are retained as local
directional measurements, not as the current hosted qualification:

| Engine | Exact p95 | Hybrid RRF p95 | Deterministic p95 | Vec comparisons |
| --- | ---: | ---: | ---: | ---: |
| `a3s_memory` default | 7.5379 ms | 49.0593 ms | 48.7440 ms | 120/120 |
| `a3s_vec` preview | 7.4195 ms | 47.4571 ms | 48.7821 ms | 120/120 |

Both runs reported zero mismatches, failed queries, failed mutations, and
initialization failures, and both closed with zero records and bytes. The
Vec-primary run's logical accounted bytes were 54,500,008 per hybrid arm.
The benchmark does not measure process RSS, crash recovery, or macOS 12 Intel.

## Promotion gates

Vec must not become authoritative until a separate decision records all of the
following:

1. sustained zero mismatch and zero shadow failure across representative
   dimensions, filters, churn, cancellation, and concurrent publication;
2. actual macOS 12 Intel evidence plus supported Linux, Windows, and macOS ARM
   matrices;
3. release latency, RSS, temporary-disk, file-handle, and cleanup budgets;
4. crash-residue and host temporary-directory policy review;
5. compatibility behavior for old status consumers and every public SDK;
6. a typed authority-selection and rollback design that does not accept a raw
   backend-name string;
7. an independent oracle retained during any canary serving phase;
8. an explicit decision about whether and when the Memory path may be removed.

The typed selector design and the lexical engine cutover are implemented, but
this gate does not satisfy the remaining platform, RSS, recovery, semantic
vector-removal, or release decisions implicitly. The Memory path must remain
present and the default until the later semantic promotion is reviewed and
committed.

## Rollback

A shadow-only regression does not require data migration because the Vec
collection is session-local and temporary. Recreate affected sessions with the
typed `WorkspaceVectorEngine::A3sMemory` compatibility default (or disable
workspace retrieval) and roll back the deployed Code binary if needed. There is
no public primitive backend selector and no automatic fallback from a selected
primary to its shadow.

If the regression affects the complete semantic runtime or creates unacceptable
latency before a binary rollback is available, use the existing trusted host
control to disable workspace retrieval, close affected sessions, and verify
zero Memory and Vec records/bytes. Exact, glob, BM25, and Code Intelligence
remain available. Never attempt to repair or promote a temporary shadow
collection in place.
