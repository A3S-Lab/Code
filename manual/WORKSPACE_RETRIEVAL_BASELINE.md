# Workspace Retrieval Baseline v1

Status: locked on 2026-08-13 for the `WSR-00` delivery gate.

Release measurements, adversarial results, and real-provider compatibility
evidence are recorded separately in the
[Workspace Retrieval Release Qualification](WORKSPACE_RETRIEVAL_QA.md).

This document records the measurement context, quality baseline, release
budgets, and adversarial trust boundaries for the first Workspace Retrieval
implementation. The machine-readable fixtures live in
`core/tests/fixtures/workspace-retrieval-v1/` and the real A3S Vec FTS/BM25 tool
executes them in the core test suite.

## Quality baseline

The synthetic v1 corpus has nine judged queries: two exact multi-term queries,
two identifier queries, two exact CJK queries, and three paraphrase queries.
The labels are independent of the returned BM25 paths.

| Metric | A3S Vec FTS/BM25 v1 |
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

This locked profile uses the compatibility line strategy. Fixed, recursive,
or custom strategies must report their own chunk count, retained bytes
including overlap, vector records, provider inputs, and request amplification;
they may not reuse the non-overlap figures above.

Reference machine:

- Intel Xeon w5-2445, 10 physical and 20 logical cores;
- 127.7 GiB RAM;
- Windows 11 build 22631;
- Rust 1.97.1 (`8bab26f4f`, 2026-07-14).

## Locked release budgets

These are qualification targets, not measurements attributed to the bounded
query-time compatibility scan.

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
| Path escape, symlink, hard-link, or rename race | Every read remains governed by `WorkspaceServices`; the source-egress reader rejects aliased identities and returned evidence is fenced by current identity/digest/revision | Out-of-root, symlink, hard-link alias/swap, rename-race, and stale-digest cases return no leaked or stale snippet |
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

Manifest admission rejects sensitive, control, generated, binary, and
oversized paths without opening every file during session construction.
Retrieval-enabled local sessions also use a dedicated source-egress catalog
reader, which revalidates logical and resolved paths and rejects every
multi-link file from the same open handle used for the embedding-source read,
while leaving ordinary workspace tools on their existing host-governed access
path. This is a second defense against a hard-link, symlink, or rename swap
between manifest admission and embedding.

The default catalog independently bounds retained text and conservatively
estimated lexical-index memory at 64 MiB each, in addition to file and chunk
limits. A file that would exceed a budget is reported as a failed partition;
already admitted smaller files remain queryable and publication stays atomic.

A3S Vec FTS/BM25 reuses the catalog's per-file postings after its first source
revision. Until then, and for custom providers without a catalog, it builds the
same bounded temporary Vec projection over the query-time scan. The locked
nine-query result ordering is equal on both paths, while the incremental path
reports zero query-time file reads. Code owns only admission, chunking, source
verification, and rendering; it no longer carries a second BM25 scorer.

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

## CODE-S1 implementation evidence

Code pins A3S Memory to commit `3293f572` and creates one exact in-memory
vector index per enabled session. The typed `WorkspaceRetrievalOptions` accepts
a provider object, embedding limits, vector record/byte budgets, catalog
chunking strategy/limits, and a bounded shutdown timeout; it does not expose a
string backend selector. Retrieval is disabled by default and the synchronous
compatibility session constructor rejects it as an async-only resource.

An enabled local session creates one manifest-backed workspace bundle and
shares its chunk catalog with lexical and semantic projections. A custom
workspace is accepted only when its `WorkspaceServices` supplies a catalog;
Code never bypasses that abstraction to read host files. `session_async`
returns while embedding is still blocked. Each file is an atomic A3S Memory
partition and becomes query-ready independently, so status exposes partial
coverage without pretending the corpus is complete.

Catalog revisions are watched directly. A changed or removed digest is marked
building and its old vector partition is removed before replacement embedding.
Generation cancellation plus digest checks before and after Memory publication
prevent an embedding completed for a superseded catalog revision from becoming
ready. Provider or vector-budget failures degrade semantic coverage only; the
catalog and lexical paths stay available.

`AgentSession::workspace_retrieval_status` exposes phase, catalog/source/vector
revisions, eligible/catalog/indexed file and chunk counts, basis-point coverage,
queue depth, current and cumulative failures, vector records/bytes, and the
immutable model descriptor. It also exposes A3S Memory as the active vector
engine and a bounded `vec_shadow` observation containing only lifecycle,
resource, mutation, and parity counters. Session close cancels active embedding,
joins or aborts the single owned task within the configured deadline, clears
and drops the Memory index, closes the session-local temporary Vec collection,
and shuts down only local manifest/catalog work created by that session.
Host-owned workspaces retain their external lifecycle. Regression
tests cover partial readiness, update-during-embedding fencing, per-file
degradation, build-after-start failure cleanup, default disablement, custom
workspace rejection, synchronous rejection, idempotent close, and weak-reference
vector cleanup.

## VEC-SHADOW1 migration evidence

Code dependency commit `708a85e3` mirrors each already-validated Memory
`VectorRecord` batch into the current A3S Vec validation pin `13585ccd` without
a second provider call. Memory remains the only result oracle. Queries execute
under one publication gate;
Vec IDs, partitions, f32 score bits, searched-record counts, and truncation are
compared, but the Memory result is returned unchanged. Any Vec initialization,
mutation, query, resource, or comparison failure degrades only the shadow.

The Vec collection is session-local and created below an operating-system
temporary directory with manual durability. It is not a durable workspace
index or shared service. The authoritative Memory `max_records` and `max_bytes`
limits remain unchanged; Vec receives the record ceiling for documents, write
batches, and query candidates and reports its separate `accounted_bytes`
estimate. See [Workspace Retrieval A3S Vec Migration](WORKSPACE_RETRIEVAL_VEC_MIGRATION.md)
for the mapping, failure isolation, benchmark evidence, promotion gates, and
binary rollback boundary.

## CODE-Q1 implementation evidence

The unified model-facing `search` tool now advertises `mode: "semantic"` only
for sessions that have both an enabled retrieval runtime and workspace read
capability. Existing disabled sessions preserve the previous grep, glob, and
BM25 schema. Hosts may also call the structured
`WorkspaceServices::semantic_search` API with typed request, hit, status, and
fallback values.

Each query is bounded, cancellable, and embedded by the same immutable
provider generation as the session index. Search captures an immutable chunk
catalog snapshot, filters candidate partitions by the host-normalized path and
optional glob, and accepts a Memory result only when its vector revision and
the catalog/source revisions remain unchanged. A concurrent replacement or
session close returns no result from a mixed generation.

Vector identity is not sufficient evidence for source text. Before rendering,
Code rereads every candidate file through the configured
`WorkspaceFileSystem`, checks the complete SHA-256 content digest, and checks
the exact UTF-8 byte range against the catalog chunk. Missing, unreadable,
timed-out, modified, or mismatched files are removed and reported with an
explicit fallback reason. Tool metadata contains status, model identity,
coverage, revisions, searched-record counts, truncation, source anchors, and a
per-hit verification marker; query/source values are absent from Code-owned
debug diagnostics and tool invocation logs at every tracing level.

Deterministic tests cover semantic ordering, path and glob filters, query
cancellation, disabled/enabled schema negotiation, direct session tool
execution, changed-source lag, deleted-source lag, lifecycle closure, and all
pre-existing search modes. Semantic remains an explicit diagnostic channel;
hybrid is the normal natural-language retrieval mode for enabled sessions.

## CODE-H1 implementation evidence

Hybrid search builds four bounded channel lists: exact literals, catalog BM25,
optional Code Intelligence workspace symbols, and semantic hits with strictly
positive cosine similarity. One-based channel ranks are fused with RRF at
`k=60`; raw BM25 and vector scores never cross calibration domains. Exact
ASCII identifier token matches occupy a protected tier, followed by stable
path, byte-offset, and chunk-id tie breakers. Final output admits at most two
chunks per file.

The fused candidate list is verified once. Candidates are grouped by path,
each file is read at most once, and both its full SHA-256 digest and exact UTF-8
chunk range must match the captured catalog revision. A catalog race returns
no mixed-generation result. Query embedding and optional structural failures
retain lexical evidence and appear in channel/final fallback metadata.

The locked fixture now carries `expected_hybrid_paths` without changing the
BM25 baseline. Its annotated deterministic embeddings map only relevant
query/document pairs, preventing zero-score corpus padding from satisfying the
Top-10 gate. Hybrid reaches Recall@10 and MRR 1.0 (BM25: 0.6667), preserves
identifier first rank, and has dedicated stale-source, semantic-degradation,
symbol-mapping, diversity, determinism, and redaction coverage.

## CODE-C2 implementation evidence

`WorkspaceChunkingStrategy` preserves the default line behavior and adds
UTF-8-safe fixed windows, recursive prioritized separators, and a trusted Rust
host custom range port. Fixed and recursive overlap is bounded and counts as
retained catalog text and as independent vector records/provider inputs.

Code accepts ranges only after checking complete coverage, no gaps, monotonic
starts and ends, UTF-8 boundaries, per-range bytes, and per-file count. It then
computes line anchors, content digests, stable IDs, and revisions itself. Host
failures and panics become bounded catalog failures. Focused tests cover ASCII
and multibyte boundaries, separator order, overlap accounting, invalid and
panicking hosts, deterministic identity, session-owned async construction, and
rejection of session overrides for a host-owned catalog.

The detailed contract and selection guidance are in
[`WORKSPACE_RETRIEVAL_CHUNKING.md`](WORKSPACE_RETRIEVAL_CHUNKING.md). The
current RRF first stage remains unchanged; `CODE-R2` owns the planned
overlap-aware deterministic reranker and its quality/resource gates.
