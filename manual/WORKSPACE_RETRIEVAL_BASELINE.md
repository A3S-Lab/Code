# Workspace Retrieval Baseline v1

Status: locked on 2026-08-13 for the `WSR-00` delivery gate.

This document records the measurement context, quality baseline, release
budgets, and adversarial trust boundaries for the first Workspace Retrieval
implementation. The machine-readable fixtures live in
`core/tests/fixtures/workspace-retrieval-v1/` and the real native BM25 tool
executes them in the core test suite.

## Quality baseline

The synthetic v1 corpus has nine judged queries: two exact multi-term queries,
two identifier queries, two exact CJK queries, and three paraphrase queries.
The labels are independent of the returned BM25 paths.

| Metric | Native BM25 v1 |
| --- | ---: |
| Recall@10 | 0.6667 (6/9) |
| Mean reciprocal rank | 0.6667 |
| Exact Recall@10 | 1.0000 |
| Identifier Recall@10 | 1.0000 |
| CJK Recall@10 | 1.0000 |
| Paraphrase Recall@10 | 0.0000 |

This is a compatibility baseline, not a claim that every returned path is
relevant. In particular, CamelCase splitting produces additional lexical
candidates. A future hybrid implementation must preserve the identifier
metrics while improving the independently judged paraphrase labels.

## Reference workspace profile

The profile was measured from Code commit
`68f39922d6158204287944bd40a1b4be17a6c4f9` before retrieval implementation
files were added. `scripts/measure_workspace_retrieval_profile.ps1` enumerates
files visible to `rg`, rejects files larger than 1 MiB or containing a NUL byte,
and applies the current per-file 80-line BM25 chunk envelope.

| Input | Measured value |
| --- | ---: |
| Files visible to `rg --files` | 1,568 |
| Files in the conservative text envelope | 1,567 |
| Text bytes | 16,093,802 (15.35 MiB) |
| Text lines | 449,368 |
| Per-file 80-line chunks | 6,392 |
| Raw 384-dimensional `f32` vector bytes | 9,818,112 (9.36 MiB) |
| Raw 768-dimensional `f32` vector bytes | 19,636,224 (18.73 MiB) |

The raw vector figures intentionally exclude catalog, lexical-index, allocator,
and immutable-snapshot overhead. They demonstrate why a bounded exact
in-memory scan is a reasonable first implementation; they do not replace the
25,000-record release benchmark.

Reference machine:

- Intel Xeon w5-2445, 10 physical and 20 logical cores;
- 127.7 GiB RAM;
- Windows 11 build 22631;
- Rust 1.97.1 (`8bab26f4f`, 2026-07-14).

## Locked release budgets

These are qualification targets, not measurements attributed to the current
query-time BM25 implementation.

| Boundary | Budget |
| --- | --- |
| Synchronous session construction | No full-corpus read and no embedding wait |
| Exact vector top-20, 25,000 x 384 normalized records | Release-mode p95 at or below 30 ms on the reference profile |
| Hybrid local ranking over the same corpus | p95 at or below 100 ms, excluding provider network latency and authoritative source reads |
| Default total retrieval memory | At most 256 MiB for catalog, lexical, and vector indexes |
| Resource exhaustion | Explicit partial/degraded coverage; no unbounded allocation |

Measurements must report warmup, sample count, build profile, CPU, vector
dimension, record count, and whether source reads or provider latency were
included. A later gate may tighten these budgets but cannot weaken them without
an explicit roadmap decision and new fixture version.

## Adversarial trust boundaries

| Threat | Required invariant | Deterministic evidence |
| --- | --- | --- |
| Path escape, symlink swap, or rename race | Every read remains governed by `WorkspaceServices`; returned evidence is fenced by current digest/revision | Out-of-root, symlink, rename-race, and stale-digest cases return no leaked or stale snippet |
| Credential or control-file egress | Remote embedding is explicit and only receives admitted chunks | Fake provider records inputs; secret, control, binary, generated, oversized, and excluded paths are absent |
| Manifest event loss | Lag marks coverage degraded and triggers a full snapshot reconciliation | Overflow fixture converges to exactly the snapshot paths and revisions |
| Cross-session observation | Catalogs, vectors, status, and cancellation are owned by one session | Two-session tests cannot retrieve or cancel the other session's state |
| Malicious or broken provider | Dimension changes, non-finite vectors, partial batches, timeout, rate limit, and panic cannot corrupt a published generation | Fault-injection tests preserve lexical service and expose typed degraded status |
| Memory or queue exhaustion | Record, byte, file, chunk, batch, and queue bounds are checked before allocation/publication | Boundary and over-budget cases remain queryable at the prior immutable revision |
| Cancellation during build/search | Close cancels queued work, joins owned tasks, and releases vector partitions within a bounded deadline | Weak-reference and task-count cleanup tests observe no retained allocation or task |
| Sensitive observability | Source, vectors, and provider credentials never enter logs, errors, metrics, or snapshots | Sentinel scanning covers error paths and serialized `SessionSnapshotV1` |

The lifecycle fixture defines create, change, delete, rename, unchanged rescan,
and lag-recovery behavior separately from relevance judgments. `CODE-C1`
consumes that contract; `WSR-QA` expands it with scheduling, cancellation, and
provider fault injection.

## CODE-C1 implementation evidence

Retrieval-enabled manifest workspaces now create one asynchronous,
session-local `WorkspaceChunkCatalog`; plain manifest and Code Intelligence
sessions do not pay this cost. The catalog shares the existing manifest
watcher, reads only through `WorkspaceFileSystem`, and publishes immutable
revisions. Normal unchanged snapshots perform zero reads; explicit changes
reread only invalidated paths; lag recovery rereads every admitted file and
reuses content-identical partitions by digest. Change notifications are
published before their corresponding manifest snapshot. Changed and removed
paths are tombstoned before replacement work so failures lower coverage rather
than returning stale content.

The default catalog independently bounds retained text and conservatively
estimated lexical-index memory at 64 MiB each, in addition to file and chunk
limits. A file that would exceed a budget is reported as a failed partition;
already admitted smaller files remain queryable and publication stays atomic.

Native BM25 reuses the catalog's per-file postings after its first source
revision. Until then, and for custom providers without a catalog, it retains the
compatible query-time scanner. The locked nine-query result ordering is equal
on both paths, while the incremental path reports zero query-time file reads.

## CODE-E1 implementation evidence

Code exposes a host-injected `EmbeddingProvider` independently from LLM clients,
workspace traversal, and A3S Memory. An immutable descriptor binds provider,
model, optional revision, dimension, and normalization for one generation.
`EmbeddingExecutor` rejects invalid descriptors and configuration before work,
then partitions admitted inputs deterministically by input count, text bytes,
and expected `f32` vector bytes. It returns all validated vectors in caller
order or a typed error; a later batch failure never returns an earlier partial
result.

Only typed timeout, rate-limit, and unavailable failures are retried. Retries,
per-attempt timeouts, retry delays, request text, input count, and expected
vector memory all have constructor-validated hard bounds. Cancellation wins at
preflight, during an active provider call, and during retry backoff; timeout
also cancels the child token passed to the provider. Authentication and invalid
request failures are never retried.

Every response must preserve the generation descriptor and contain exactly one
finite, correctly dimensioned vector for each unique input identifier. Unit
normalization is verified when promised. Provider panics are contained as typed
executor failures without copying panic payloads into the returned error.
Code-owned `Debug` and error rendering omit input text, vector values, and raw
provider response bodies. Deterministic fake-provider tests cover batching,
order, partial failure, cancellation, timeout, retry exhaustion, descriptor
drift, output identity, dimension, non-finite values, normalization, budgets,
panic containment, and diagnostic redaction.
