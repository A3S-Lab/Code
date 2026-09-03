# Workspace Retrieval A3S Vec Migration

Status: developer shadow, 2026-09-03.

This document defines the first A3S Code integration gate for A3S Vec. It is a
differential migration, not a serving-backend switch. Code commit
`4163d8e3a1a96bbae430dc987005acaa362efb30` introduced the shadow; the
current validation pin is A3S Vec commit
`41283f6315906a2737b5a8e8612ac876a8dc9c04`.

## Authority contract

A3S Memory remains the only authoritative vector engine for semantic and
hybrid workspace retrieval. Its result determines hits, ordering, scores,
searched-record counts, truncation, fallbacks, and errors. The Vec shadow may
collect evidence and degrade its own status, but it cannot change any of those
serving decisions.

One admitted embedding response follows this path:

```text
Embedding Provider
        |
        | one validated VectorRecord batch
        v
Code publication gate
        |----------------------|
        v                      v
A3S Memory authority    A3S Vec shadow
        |                      |
        | query result         | differential result
        |----------------------|
                   |
                   v
        compare, record counters,
        return Memory result unchanged
```

Code does not call the Embedding Provider a second time. It clones the same
already-admitted records only after the provider response has passed the
existing dimension, finiteness, normalization, identity, byte, generation,
and cancellation checks.

## Session and storage ownership

Each retrieval-enabled session owns one Memory index and, when initialization
succeeds, one Vec collection. The collection is created below an
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
- Memory mutates first and remains authoritative;
- Vec replacement fetches the previous partition, deletes it, inserts the new
  documents, and attempts rollback if insertion fails;
- synchronous Vec work runs in Tokio's blocking pool. The owned operation guard
  moves into that blocking operation so cancelling an async waiter cannot let
  close or a later mutation overtake work that is still executing.

Vec initialization, mutation, query, worker, filesystem, filter, unsupported
label, rollback, or write-rejection failures increment only bounded counters
and set `vec_shadow.phase = degraded`. Logs contain a static failure code, not
the path, query, vector, document, or backend error body. A mismatch also
degrades the shadow and returns the Memory result unchanged.

## Resource contract

Memory's `max_records` and `max_bytes` remain the authoritative admission
limits. Vec receives `max_records` as its maximum documents, query candidates,
and write-batch documents. Memory's byte limit is not reinterpreted as a Vec
storage limit because the engines use different layouts. Vec instead reports
its deterministic `accounted_bytes` estimate for review.

`accounted_bytes` is not process RSS, committed virtual memory, or temporary
directory size. Promotion requires separate RSS, disk, and latency
qualification. A shadow resource-limit failure cannot weaken the Memory
limit or permit additional serving records.

## Public status

`WorkspaceRetrievalStatus` adds two backward-compatible fields:

- `active_vector_engine` / `activeVectorEngine` is `a3s_memory` for an enabled
  runtime and absent for a disabled runtime;
- `vec_shadow` / `vecShadow` reports `phase`, `revision`, `record_count`,
  `accounted_bytes`, initialization and mutation outcomes, and compared,
  matching, mismatched, and failed query counts.

Rust uses `active_vector_engine` and `vec_shadow`. Node.js uses
`activeVectorEngine` and `vecShadow`. Python dictionaries use
`active_vector_engine` and `vec_shadow`. Go exposes `ActiveVectorEngine` and
`VecShadow` while its bridge JSON remains snake_case. Legacy serialized Rust
status without these fields loads with no active engine and a disabled shadow.

Required operator invariants are:

- `active_vector_engine == a3s_memory` for every enabled session;
- `matching_queries == compared_queries`;
- `mismatched_queries == 0` and `failed_queries == 0` before promotion;
- `record_count == vector_records` after a ready generation when no shadow
  failure has occurred;
- both authoritative and shadow record/byte counts are zero after close.

## Qualification evidence

The 2026-09-02 Windows x86-64 qualification used release benchmark report
schema 4 and profile `workspace-retrieval-v3`:

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
`41283f6315906a2737b5a8e8612ac876a8dc9c04` on the same Windows x86-64 class of
host (25,000 records, 384 dimensions, top-20, 100 measured queries, 20
warmups). Exact p95 was 8.5055 ms, hybrid RRF-only p95 was 49.9420 ms, and
deterministic-rerank p95 was 51.3222 ms; all three remained within their
30/100/100 ms budgets. Both hybrid arms compared 120/120 queries with zero
mismatches, failed queries, initialization failures, and failed mutations;
the shadow reported 25,000 records, 196 successful mutations, and
54,500,008 accounted bytes. The reranker added 1.3802 ms at p95 and stayed
inside its 10 ms delta budget. The process exited successfully after the
close/cleanup assertions.

Additional local gates passed:

- four focused Vec adapter tests, including 384-dimensional bit-exact scores,
  partition filters, replacement, removal, clear, and close;
- the 64-generation replacement soak;
- the complete serial Core library suite (2,991 tests discovered; 2,976
  passed, 13 ignored) with the two PowerShell-7-dependent native-sandbox tests
  explicitly blocked by this host, plus the focused shadow tests and offline
  integration/doc checks;
- workspace all-target check and strict Clippy;
- strict Node.js and Python Clippy, Go bridge tests, SDK mapping tests, and real
  Windows native Node.js/Python workspace-retrieval lifecycle tests.

A3S Vec CI run
[`33517845653`](https://github.com/A3S-Lab/Vec/actions/runs/33517845653)
passed MSRV 1.75, Linux x86-64/ARM64, Windows x86-64, macOS ARM64/Intel hosted,
format, lint, docs, feature, and fuzz jobs. Actual macOS 12 Intel hardware or an
equivalent external runner remains a release gate; the hosted Intel job is not
claimed as that evidence.

The repository-wide strict rustdoc command is currently blocked by pre-existing
private or broken links in `core/src/mcp/result.rs` and `core/src/workspace/s3`.
The new Vec modules produce no rustdoc diagnostic. This baseline issue must be
closed separately rather than hidden in the migration change.

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

This gate satisfies none of those decisions implicitly. The Memory path must
remain present and authoritative until the later promotion is reviewed and
committed.

## Rollback

A shadow-only regression does not require data migration because the Vec
collection is session-local and temporary. Roll back the deployed Code binary
to a revision before `4163d8e`, then recreate affected sessions. There is no
public primitive backend selector and no separate shadow configuration knob in
this gate.

If the regression affects the complete semantic runtime or creates unacceptable
latency before a binary rollback is available, use the existing trusted host
control to disable workspace retrieval, close affected sessions, and verify
zero Memory and Vec records/bytes. Exact, glob, BM25, and Code Intelligence
remain available. Never attempt to repair or promote a temporary shadow
collection in place.
