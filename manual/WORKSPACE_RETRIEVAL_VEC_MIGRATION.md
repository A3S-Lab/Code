# Workspace Retrieval A3S Vec Migration

Status: lexical engine cutover delivered; semantic Vec-primary remains a gated
developer preview, 2026-09-03.

This document defines the A3S Code integration gates for A3S Vec. Semantic
vector migration is still differential evidence, not a stable serving-backend
promotion. Code commit `788bc61a458cafe3c6809a65d9e1e8c733a97a2e` introduced
the gated Vec-primary authority slice; the current validation pin is A3S Vec commit
`41283f6315906a2737b5a8e8612ac876a8dc9c04`.

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

The 2026-09-03 validation refresh rebuilt the release benchmark against Vec
`41283f6315906a2737b5a8e8612ac876a8dc9c04` and Code
`788bc61a458cafe3c6809a65d9e1e8c733a97a2e`
on the same Windows x86-64 class of host (25,000 records, 384 dimensions,
top-20, 100 measured queries, 20 warmups). The current release profile reports:

| Gate | Result | Limit |
| --- | ---: | ---: |
| Exact p95 | 7.5379 ms | <= 30 ms |
| Hybrid RRF-only p95 | 49.0593 ms | <= 100 ms |
| Hybrid deterministic-rerank p95 | 48.7440 ms | <= 100 ms |
| Deterministic reranker p95 delta | -0.3153 ms | <= 10 ms positive addition |
| Vec records per hybrid arm | 25,000 | exactly 25,000 |
| Vec accounted bytes per hybrid arm | 54,500,008 | reported, not an RSS claim |
| Vec revision / successful mutations | 196 / 196 | no failed mutation |
| Vec comparisons per hybrid arm | 120/120 matching | zero mismatch/failure |
| Closed authoritative and shadow state | zero records and bytes | required |

Both hybrid arms compared 120/120 queries with zero mismatches, failed
queries, initialization failures, or failed mutations. The process exited
successfully after the close/cleanup assertions. `accounted_bytes` remains a
logical Vec estimate; it is not process RSS, committed virtual memory, or
temporary-directory size.

Additional local gates passed:

- four focused Vec adapter tests, including 384-dimensional bit-exact scores,
  partition filters, replacement, removal, clear, and close;
- the catalog-backed and query-time BM25 suites (14 tests) after the lexical
  cutover, with the locked nine-query ordering unchanged;
- the 64-generation replacement soak;
- the complete serial Core all-target suite (3,059 tests discovered; 3,044
  passed, 13 ignored) with the two PowerShell-7-dependent native-sandbox tests
  explicitly blocked by this host, plus the focused shadow tests and offline
  integration/doc checks;
- workspace all-target check and strict Clippy;
- strict Node.js and Python Clippy, Go bridge tests, SDK mapping tests, and real
  Windows native Node.js/Python workspace-retrieval lifecycle tests.

A3S Vec CI run
[`33705867979`](https://github.com/A3S-Lab/Vec/actions/runs/33705867979)
passed MSRV 1.75, Linux x86-64/ARM64, Windows x86-64, macOS ARM64/Intel hosted,
format, lint, docs, feature, and fuzz jobs. Actual macOS 12 Intel hardware or an
equivalent external runner remains a release gate; the hosted Intel job is not
claimed as that evidence.

The Code validation workflows for the same pin also passed: the complete CI
matrix is recorded in
[`33712033250`](https://github.com/A3S-Lab/Code/actions/runs/33712033250), and
the release-profile qualification is recorded in
[`33712033140`](https://github.com/A3S-Lab/Code/actions/runs/33712033140).

The full Code workspace strict rustdoc command (`cargo doc --locked
--workspace --all-features --no-deps` with `RUSTDOCFLAGS=-D warnings`) passed on
the current candidate. This includes the migration adapter and its public SDK
facades; no rustdoc diagnostic is suppressed for the Vec integration.

The same revision was then run with the explicit Vec-primary selector on the
same Windows x86-64 host (25,000 records, 384 dimensions, top-20, 100 measured
queries, 20 warmups). These are local directional measurements, not a
cross-platform release claim:

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
