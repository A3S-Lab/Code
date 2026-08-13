# A3S Code Roadmap

## 1. Scope and authority

A3S Code is the native coding-Agent Harness and one implementation of the
provider-neutral A3S Cloud `AgentExecutionProvider` contract. It owns coding
Agent loop semantics, workspace tools, model adapters, context construction,
Tool request/result events, deterministic context reduction, session
snapshots, and provider-local recovery.

A3S Cloud owns Agent releases, conversations, executions, provider bindings,
grants, approvals, checkpoint/fork business lineage, audit, placement,
deployment, and product availability. A3S Runtime owns generic Task/Service
lifecycle. Box and OCI Runtime own execution and isolation. Gateway owns public
request traffic and inference request accounting.

The [cross-repository Agent Runtime platform roadmap](https://github.com/A3S-Lab/a3s/blob/main/docs/agent-runtime-platform-roadmap.md)
defines shared ownership and dependency order. This roadmap must not create a
second Cloud Agent lifecycle, scheduler, queue, Secret store, usage ledger, or
checkpoint authority.

## 2. Current foundation

- `Agent`, `AgentSession`, governed Tool invocation, model adapters, bounded
  context, events, artifacts, and atomic `SessionSnapshotV1` are available.
- `AgentProtocolHarness` and `AgentProtocolHost` expose release/session/run,
  cancellation, checkpoint recovery, receipts, and bounded event pages.
- The complete native Cloud provider integration and real Box recovery gate
  remain owned by Cloud `A1.2` and its compatibility lock.

## 3. Delivery plan

| Gate | State | Code-owned outcome | Boundary |
| --- | --- | --- | --- |
| `CAR-01` | In progress | Conform the native Harness to Cloud `A1.2`/`A1.3` command, receipt, event-page, cancellation, and recovery contracts | Cloud retains execution identity and sequencing authority |
| `CAR-02` | Planned | Emit bounded, versioned Tool request/result and context-usage evidence with byte counts, token estimates/reports, repeated-content digests, and immutable content references | Cloud owns authorized projections; Gateway usage remains the billed request ledger |
| `CAR-03` | Planned | Implement deterministic Tool-result transformations with algorithm/version, source/result digest, byte/token deltas, loss mode, and immutable original reference | Cloud pins policy; Code does not invent tenant policy or mutate past events |
| `CAR-04` | Planned | Map `SessionSnapshotV1` and logical resume evidence to the common Harness checkpoint contract | Cloud `A1.6` owns checkpoint identity, retention, approval, and fork lineage |
| `CAR-05` | Planned | Pass restart, exact replay, cancellation, hostile Tool output, bounded-content, Secret-redaction, checkpoint, and cleanup conformance through one Cloud-managed Box workload | No direct Code-to-node control path |

Observation precedes mutation: `CAR-02` must provide useful read-only context
diagnostics before `CAR-03` can reduce any Tool result. The first transform
profile is deterministic and limited to bounded truncation, head/tail
retention, repeated-line folding, structured sampling, and immutable original
content. Model-generated summarization requires a separate future policy and
replay contract.

## 4. Invariants

1. Raw Tool content uses the configured shared content adapter; events carry
   bounded fields and immutable references.
2. A transform never destroys the original authority and never changes an
   already-persisted event during replay.
3. Code does not receive brokered plaintext credentials when the Box egress
   broker can inject them at the authorized destination boundary.
4. Context estimates and Harness facts do not claim to be provider billing
   records; correlation with Cloud Inference usage is explicit.
5. Code snapshots are provider evidence. Cloud owns the business checkpoint
   and fork lineage that references them.
6. Code may run standalone, but Cloud-managed mode accepts exact immutable
   policy and cannot silently expand it.

## 5. Non-goals

- A Code-specific Cloud execution aggregate, scheduler, queue, node channel,
  deployment controller, approval store, or audit database.
- A durable credential store or direct handling of brokered credential grants.
- Egress network enforcement, idle suspension decisions, replica scaling, or
  public traffic routing.
- Treating Runtime logs, Flow history, or Gateway usage as the Agent semantic
  event stream.
- Non-deterministic or irreversible Tool-result rewriting in the baseline.

## 6. Workspace retrieval program

### 6.1 Outcome and authority

The Workspace Retrieval (`WSR`) program delivers fast exact, lexical,
structural, and semantic retrieval over the workspace visible to one Code
session. Semantic indexing is asynchronous, session-bound, memory-resident,
and optional. It does not require a vector database service or a durable local
database.

Code is the product capability owner. It owns workspace admission, chunking,
Embedding Provider integration, session lifecycle, incremental reconciliation,
hybrid ranking, evidence rendering, and the model-facing `search` contract.
A3S Memory owns only reusable vector-index primitives. Product hosts own
configuration and presentation. This section is the cross-repository source of
truth for that boundary; component repositories should link here instead of
creating competing lifecycle definitions.

The program preserves the existing search surfaces:

- `grep` remains authoritative for exact strings and regular expressions.
- `glob` remains authoritative for path discovery.
- Code Intelligence remains authoritative for saved-file symbols and semantic
  navigation.
- Lexical retrieval remains available when semantic retrieval is disabled,
  building, degraded, or unavailable.
- Semantic results are candidate evidence, never authority for a file's current
  contents. Code verifies returned snippets against the current workspace
  revision before exposing them.

### 6.2 First-principles decisions

1. A normal coding workspace is small enough for exact vector scanning. A
   contiguous in-memory flat index is the baseline because it is deterministic,
   has exact recall, is easy to bound, and has no service lifecycle. HNSW or
   quantization requires benchmark evidence and a separate implementation
   behind the same contract.
2. One workspace watcher is sufficient. The retrieval runtime subscribes to
   the existing manifest snapshot and change streams; it must not start another
   recursive watcher.
3. `MemoryItem` is not a code chunk. Importance, recency, durable deduplication,
   consolidation, and pruning semantics must not leak into workspace retrieval.
4. A3S Memory stores and compares caller-supplied vectors. It never reads a
   workspace, selects an embedding model, performs network I/O, or owns a Code
   session.
5. Session creation never waits for a full index. Partial readiness is visible
   and useful, and lexical search remains the fallback while the index builds.
6. No semantic result is returned solely from a stale vector. Top candidates
   are fenced by content digest/revision when their source snippets are read.
7. Remote embedding is source-code egress. It is explicit, policy-bound,
   observable, and disabled when no admitted provider is configured.
8. Backend choices in SDK options use typed provider objects. Raw strings such
   as `vectorBackend: "memory"` are not part of the supported SDK design.

### 6.3 Target architecture

```text
LocalWorkspaceManifest ── snapshots/changes ─┐
                                             v
WorkspaceFileSystem ── admitted reads ─> WorkspaceRetrievalRuntime
                                             │
                              ┌──────────────┴──────────────┐
                              v                             v
                       Shared ChunkCatalog          EmbeddingProvider
                              │                             │
                    ┌─────────┴─────────┐                   v
                    v                   v          InMemoryVectorIndex
              LexicalIndex       source evidence       (a3s-memory)
                    │                   │                   │
query ─> exact/symbol/lexical/semantic candidate generation ┘
                    │
                    v
          reciprocal-rank fusion + diversity
                    │
                    v
        current-content verification ─> bounded snippets
```

The `ChunkCatalog` is the session's single source of truth for searchable text
chunks. It prevents semantic retrieval and BM25 from maintaining different
chunk boundaries or repeatedly reading the same unchanged file. Each chunk
contains a stable identifier, workspace-relative path, line range, language,
optional symbol context, content digest, file revision, and bounded text.

Initial chunking is deterministic and language-independent, with both line and
byte/token ceilings. Code Intelligence may later provide symbol boundaries as
an optional enhancement, but indexing must not wait for an LSP server and must
produce equivalent fallback chunks when Code Intelligence is unavailable.

### 6.4 Subproject ownership

| Subproject | Owns | Must not own |
| --- | --- | --- |
| `a3s-memory` | Public `VectorIndex` contract, vector/result types, exact in-memory implementation, dynamic dimensions, atomic partition replacement/removal, immutable query snapshots, deterministic ordering, and memory budgets | Workspaces, files, code chunking, embedding clients, model configuration, session lifecycle, hybrid ranking, or prompt context |
| `a3s-code-core/workspace` | Chunk admission, shared `ChunkCatalog`, lexical index, manifest reconciliation, path/revision metadata, and structured `WorkspaceRetrieval` provider contract | Provider credentials, host UI, or durable cross-session cache policy |
| `a3s-code-core/embedding` | Host-injected `EmbeddingProvider` contract, provider descriptor, batching, cancellation, bounded retry, and normalized embedding errors | Vector storage or workspace traversal |
| `a3s-code-core/session` | `WorkspaceRetrievalRuntime`, asynchronous construction, prioritization, query-time promotion, cancellation, close/replace/resume behavior, and session isolation | Process-global mutable indexes or hidden persistence |
| `a3s-code-core/tools` | `semantic`/`hybrid` search modes, RRF fusion, path filters, bounded source anchors, coverage/status metadata, and lexical fallback | A second chunker or direct filesystem traversal outside `WorkspaceServices` |
| Code SDKs | Typed retrieval options, typed Embedding Provider injection, status/result DTOs, and lifecycle parity across Rust, Node, Python, and Go | Primitive backend-name options or SDK-specific ranking behavior |
| CLI/TUI and other hosts | ACL wiring, opt-in controls, readiness/degraded presentation, diagnostics, and provider-secret handling | Reimplementing indexing or making a host-specific search protocol |
| Tests, benchmarks, and docs | Shared relevance fixtures, adversarial lifecycle tests, performance baselines, examples, and operator guidance | Production-only correctness assumptions that cannot be tested deterministically |

### 6.5 Component contracts

#### A3S Memory vector kernel

A3S Memory adds a `VectorIndex: Send + Sync` capability next to, not inside,
`MemoryStore`. The first implementation is `InMemoryVectorIndex`. Its public
contract supports:

- a descriptor fixed at index construction time containing dimension,
  similarity metric, normalization rule, and byte/record budgets;
- batch replacement of one logical partition and atomic removal of a
  partition; Code maps one partition to one workspace file;
- exact top-k search with optional caller-defined labels/partition filters;
- an immutable revision and status snapshot returned with every query;
- explicit rejection of dimension mismatch, non-finite values, and invalid
  zero vectors for metrics that require normalization;
- deterministic tie-breaking by record identifier;
- `clear`, record/byte accounting, and bounded allocation failure;
- concurrent searches over an immutable snapshot while a replacement is built
  off-lock and atomically published. Each partition owns an `Arc`-backed
  contiguous vector block, so publication shares unchanged partitions and does
  not copy the full vector corpus for a one-file update.

The baseline stores normalized `f32` vectors contiguously and uses exact dot
product for cosine search. CPU-heavy scans run outside Tokio's async workers.
The implementation does not spawn an immortal background task and releases all
memory when its owning session drops it.

The existing SQLite/FTS and optional `sqlite-vec` memory backend are not the
baseline for WSR. A future SQLite adapter may implement `VectorIndex`, but WSR
must not depend on it, its current fixed dimension, or a native extension.

Suggested source layout:

```text
src/vector/
├── mod.rs
├── index.rs
├── types.rs
└── in_memory.rs
```

#### Code retrieval core

Code adds a structured `WorkspaceRetrieval` capability to `WorkspaceServices`
rather than extending the display-oriented `WorkspaceSearch::grep` response.
The provider returns paths, ranges, digests, revisions, per-channel ranks, and
coverage metadata. `SearchTool` remains the single model-facing search tool.

The local implementation owns these internal components:

```text
core/src/workspace/retrieval/
├── mod.rs                 # public provider boundary
├── types.rs               # requests, hits, status, revisions
├── chunk.rs               # deterministic bounded chunking
├── catalog.rs             # shared immutable chunk snapshots
├── lexical.rs             # indexed BM25/postings
├── semantic_runtime.rs    # embedding queue and vector partitions
└── hybrid.rs              # candidate fusion and diversity
```

The current native BM25 path selects candidates, reads files, creates 80-line
chunks, and scores them for every query. WSR first moves chunk ownership into
the catalog, then replaces query-time corpus construction with an incremental
lexical index. The model-facing BM25 behavior and source anchors remain
compatible during that migration.

The existing default session path currently constructs plain local workspace
services without a manifest. When WSR is enabled for a local session, session
capability construction must instead create or reuse one manifest-backed local
backend and share it with retrieval and Code Intelligence. If a host supplies
custom or remote `WorkspaceServices`, Code does not bypass that abstraction:
semantic modes appear only when the host also supplies a structured
`WorkspaceRetrieval` provider. The initial WSR release is therefore local-first
without making local filesystem access part of the public retrieval contract.

#### Embedding Provider

The provider boundary accepts bounded batches and a cancellation token and
returns vectors plus an immutable descriptor containing provider identity,
model identity, dimension, and normalization contract. The runtime rejects a
descriptor change within one index generation and rebuilds explicitly when the
configured model changes.

The first implementation may be an admitted OpenAI-compatible embeddings
adapter, but the interface remains host-injectable so a local model can be used
without changing retrieval. A3S Memory has no dependency on this adapter.

Remote providers must receive only chunks admitted by Code's embedding egress
policy. Sensitive configuration, credential files, private keys, generated
trees, binaries, oversized files, and workspace-private control directories
are excluded by default. Neither content nor vectors are written to logs.

### 6.6 Session lifecycle and consistency

1. Session creation constructs the runtime and returns immediately. Disabled or
   unconfigured retrieval adds no background work.
2. The runtime observes the first manifest snapshot and schedules eligible
   files. Recently touched, changed/untracked, and query-promoted files receive
   priority without excluding the rest of the admitted corpus.
3. Each file is read through `WorkspaceServices`, chunked once, and committed
   to the catalog. Lexical data becomes ready immediately; semantic data becomes
   ready after its embedding batch succeeds.
4. Completed files are atomically published as vector partitions. Queries may
   use completed partitions while the rest of the workspace is still building.
5. A changed or deleted path is tombstoned before replacement work begins.
   Results also compare the stored content digest with the text read for the
   final snippet and discard mismatches.
6. A lagged change receiver marks the runtime degraded and reconciles against
   the latest full manifest snapshot. It never silently declares full coverage.
7. Provider timeout, rate limiting, invalid vectors, or memory exhaustion
   affects semantic coverage only. Exact and lexical retrieval remain usable.
8. Closing, replacing, or cancelling a session cancels queued reads and
   embedding requests, joins owned tasks within a bounded deadline, and drops
   the vector index. Resuming a session rebuilds from the current workspace; no
   ephemeral vector state is serialized into `SessionSnapshotV1`.

`WorkspaceRetrievalStatus` exposes at least `disabled`, `building`, `ready`,
`degraded`, and `closed`, together with workspace/index revisions, eligible and
indexed file/chunk counts, coverage, queue depth, failure counts, memory bytes,
and model identity. Search output reports the status/revision that produced its
hits so partial semantic coverage cannot masquerade as a complete search.

### 6.7 Retrieval and ranking policy

The query planner generates independent bounded candidate lists:

| Channel | Best use | Baseline behavior |
| --- | --- | --- |
| Exact | Identifiers, literals, regexes | Existing `grep`; strongest signal for exact matches |
| Path | File/module discovery | Existing `glob` and catalog path terms |
| Lexical | Multi-term repository concepts | Incremental BM25 over the shared chunk catalog |
| Structural | Types, functions, definitions, references | Existing Code Intelligence symbol/navigation services |
| Semantic | Paraphrases and vocabulary mismatch | Query embedding against ready vector partitions |

Hybrid ranking uses reciprocal-rank fusion over channel ranks instead of adding
raw BM25 and cosine scores, which are not calibrated to one another. It then
applies bounded per-file diversity and transparent recency/path hints. Exact
identifier matches cannot be displaced solely by semantic similarity.

The initial tool rollout adds `semantic` for diagnosis/evaluation and `hybrid`
for normal natural-language retrieval. Those modes appear in the dynamic tool
schema only when the required provider exists. `hybrid` may operate with
partial semantic coverage, but its metadata must identify which channels ran,
their coverage, truncation, and any fallback reason. Existing `grep`, `glob`,
and `bm25` arguments and permissions remain compatible.

### 6.8 Configuration and SDK shape

Retrieval is disabled unless the host explicitly supplies an admitted
Embedding Provider or enables a supported provider block in ACL. The default
vector implementation is an internal implementation detail, not a stringly
typed user choice.

Configuration separates:

- enablement and build/query budgets;
- embedding provider/model/batch limits;
- egress admission and exclude/include rules;
- chunk size and corpus limits;
- memory byte/record budgets;
- search channel limits and fusion constants.

ACL remains the product configuration format. Provider secrets use existing
secret resolution and are never copied into persisted session data. SDKs expose
typed `WorkspaceRetrievalOptions` and provider objects; they do not accept raw
backend names. All SDKs preserve the same defaults, validation errors, status
states, and close semantics before the feature is declared stable.

### 6.9 Delivery gates and dependency order

Current implementation status:

| Gate | Status | Evidence |
| --- | --- | --- |
| `WSR-00` | Delivered | Versioned relevance and lifecycle fixtures, native BM25 CI baseline, reference sizing profile, locked budgets, and adversarial trust-boundary review |
| `MEM-V1` | Delivered | A3S Memory `main` commit `82e3734` adds the public exact ephemeral vector kernel and passes default, SQLite-feature, oracle, concurrency, budget, cleanup, Clippy, and rustdoc gates |
| `CODE-C1` | Next | Shared catalog, eligibility policy, deterministic chunking, lexical postings, and manifest reconciliation |

The detailed baseline and threat model are in
[`manual/WORKSPACE_RETRIEVAL_BASELINE.md`](manual/WORKSPACE_RETRIEVAL_BASELINE.md).

| Gate | Owner | Depends on | Deliverable | Exit criteria |
| --- | --- | --- | --- | --- |
| `WSR-00` | Code core/tests | None | Versioned retrieval fixture corpus, current BM25 baseline, sizing data, threat model, and locked quality/latency budgets | Baseline is reproducible in CI and separates identifier, paraphrase, CJK, and lifecycle cases |
| `MEM-V1` | A3S Memory | `WSR-00` contract draft | Public vector types/trait and `InMemoryVectorIndex` | Contract, oracle, concurrency, invalid-input, budget, and cleanup tests pass without SQLite features |
| `CODE-C1` | Code workspace | `WSR-00` | Shared chunk catalog, eligibility policy, deterministic chunker, lexical postings, and manifest reconciliation | Unchanged files are not reread; create/change/delete/rename and lag recovery are deterministic |
| `CODE-E1` | Code model/session | `WSR-00` | Host-injected Embedding Provider contract, batching, cancellation, and typed errors | Deterministic fake provider proves dimensions, cancellation, retry bounds, and descriptor changes |
| `CODE-S1` | Code session | `MEM-V1`, `CODE-C1`, `CODE-E1` | Asynchronous session retrieval runtime and vector partition lifecycle | Session creation does not wait; partial readiness works; close drops all owned tasks and memory |
| `CODE-Q1` | Code tools | `CODE-S1` | Structured semantic search, verified snippets, status/coverage metadata, and fallback | No stale/deleted hit is rendered and existing search modes have no behavior regression |
| `CODE-H1` | Code tools/intelligence | `CODE-Q1` | Exact, BM25, symbol, and semantic candidate fusion | Hybrid meets locked quality gates and preserves identifier precision |
| `SDK-R1` | Code SDKs | `CODE-S1`, `CODE-Q1` | Rust/Node/Python/Go typed options, status DTOs, lifecycle parity, and examples | SDK alignment checks and language-specific integration tests pass |
| `HOST-R1` | CLI/TUI hosts | `SDK-R1` | ACL wiring, readiness/degraded diagnostics, and explicit enable/disable controls | A user can identify disabled, building, partial, ready, and degraded states without debug logs |
| `WSR-QA` | Code tests/benchmarks | `CODE-H1`, `SDK-R1` | Adversarial E2E, performance benchmark, soak, and failure-injection suite | All release gates in section 6.10 pass on the reference profiles |
| `WSR-DOC` | Memory, Code, hosts | `WSR-QA` | README, roadmap status, ACL reference, SDK examples, privacy guidance, and migration notes | Examples execute and no obsolete query-time-BM25 or sqlite-vec guidance remains |

The parallelizable dependency shape is:

```text
                         ┌─> MEM-V1 ───────────────┐
WSR-00 ─────────────────┼─> CODE-C1 ──────────────┼─> CODE-S1 ─> CODE-Q1 ─> CODE-H1
                         └─> CODE-E1 ──────────────┘                  │          │
                                                                    └─> SDK-R1 ─┼─> WSR-QA ─> WSR-DOC
                                                                                 └─> HOST-R1
```

`MEM-V1`, `CODE-C1`, and `CODE-E1` should be developed in parallel after their
shared types and invariants are frozen. SDK and host work starts from the
versioned Code contract, not from private runtime structs.

### 6.10 Release qualification

`WSR-00` records the reference hardware and may tighten these thresholds, but a
later gate may not silently weaken them to make an implementation pass.

#### Correctness and retrieval quality

- Exact-vector top-k results match a brute-force f64 oracle across randomized
  corpora, updates, deletions, filters, and score ties.
- Deleted, changed, excluded, or out-of-scope content produces zero rendered
  stale hits after the corresponding workspace revision is observed.
- On the locked paraphrase fixture, hybrid Recall@10 improves over current BM25
  by at least 15 percentage points and reaches at least 0.85.
- On the identifier fixture, hybrid MRR and Recall@10 are not lower than exact
  plus BM25 baselines.
- Path filters, CJK queries, split identifiers, repeated boilerplate, and same
  symbol names in different modules have dedicated regression cases.

#### Latency and resources

- Session construction adds no full-corpus read or embedding wait to the
  synchronous creation path.
- On the reference profile, exact top-20 vector search over 25,000 normalized
  384-dimensional records has release-mode p95 at or below 30 ms.
- Hybrid local ranking, excluding external query-embedding network latency and
  source reads, has p95 at or below 100 ms for the same corpus.
- The default session budget is bounded; reaching it produces explicit partial
  coverage and never unbounded allocation. The initial target ceiling is
  256 MiB for catalog, lexical, and vector indexes combined.
- Repeated queries do not reread or re-embed unchanged files.

#### Isolation, security, and resilience

- Two sessions over the same or different roots cannot observe each other's
  chunks, vectors, status, revisions, or cancellation.
- Excluded secret/control paths are never submitted to a remote Embedding
  Provider, even through symlinks, rename races, include filters, or manifest
  lag recovery.
- Provider timeouts, 429/5xx responses, wrong dimensions, NaN/Infinity values,
  partial batches, panics, and cancellation degrade semantic retrieval without
  disabling `grep`, `glob`, BM25, or Code Intelligence.
- A change-stream overflow triggers full reconciliation; concurrent query and
  replacement observes either the old or new immutable partition, never a
  partially written partition.
- Session close during initial build, retry backoff, or a running query leaves
  no owned task, file handle, socket, or retained vector allocation after the
  bounded cleanup deadline.
- Workspace source text, vectors, and provider credentials do not appear in
  logs, metrics, error chains, or persisted session snapshots.

Deterministic fake embeddings are the required CI oracle. Opt-in real-provider
tests validate wire compatibility and cancellation but must never be the sole
proof of ranking correctness or run with repository secrets in shared CI.

### 6.11 Rollout and rollback

1. **Developer preview:** feature compiled and tested but disabled unless a host
   injects a provider; expose detailed status and the diagnostic `semantic`
   mode.
2. **Opt-in beta:** ACL and SDK configuration supported; `hybrid` recommended
   for natural-language queries while BM25 remains available explicitly.
3. **Stable:** SDK parity, adversarial qualification, privacy documentation,
   and production telemetry budgets complete. Any future automatic selection
   of `hybrid` requires model-tool evaluation and a separate compatibility
   decision.

Rollback is configuration-only: cancel the retrieval runtime, hide semantic
tool modes, and retain existing exact, lexical, and Code Intelligence paths.
No migration or index deletion procedure is required because the baseline
index is session-ephemeral.

### 6.12 WSR non-goals

- A vector database server, durable workspace index, global daemon, or Cloud
  retrieval service.
- Serializing vectors into Code checkpoints or sharing them across tenants,
  users, worktrees, or sessions.
- Turning workspace chunks into `MemoryItem` values or applying memory
  importance, consolidation, access-count, or prune behavior to source code.
- Making A3S Memory depend on model SDKs, HTTP clients, workspace APIs, or Code.
- Replacing `grep`, `glob`, saved-file Code Intelligence, or authoritative file
  reads with semantic similarity.
- Shipping HNSW, product quantization, persistent caches, cross-session reuse,
  or automatic local-model downloads before baseline measurements demonstrate
  a concrete need and a separate ADR defines lifecycle and security.
- Sending workspace content to any embedding endpoint merely because a chat
  model is configured.
