# Workspace Retrieval Chunking

## Scope

A3S Code chunks only manifest-admitted UTF-8 text. The manifest classifies a
file before catalog construction, so PDF, Office, image, audio, video, archive,
font, database, and other non-text assets never reach a chunking strategy or an
Embedding Provider. Parsing those assets belongs to the separate knowledge
compiler and requires an explicit typed artifact handoff.

Chunking is a Code catalog concern, not an A3S Memory concern. The same chunk
snapshot feeds BM25, semantic indexing, hybrid ranking, and final digest/range
verification. A3S Memory receives stable IDs and vectors only.

## Strategies

`WorkspaceChunkingStrategy` supports these bounded strategies:

| Strategy | Intended use | Boundary behavior | Overlap |
| --- | --- | --- | --- |
| `Lines` | Source code and compatibility with the original catalog | Closes at the first configured line or UTF-8 byte ceiling | None |
| `FixedWindow` | Uniform prose or model-independent sizing | Fixed UTF-8-safe byte windows | Configurable |
| `Recursive` | Markdown, documentation, and mixed prose | Prefers caller-ordered paragraph, line, sentence, then space separators; falls back to a UTF-8-safe hard boundary | Configurable |
| `Custom` | Host-specific token, syntax-tree, domain, or generated boundaries | A trusted Rust host returns byte ranges through `CustomWorkspaceChunkingStrategy` | Host-defined |

`Lines` remains the default. Changing strategy is explicit and session-local;
it does not alter other sessions or a host-owned custom workspace catalog.

The fixed and recursive targets are byte budgets rather than tokenizer token
counts. This keeps the catalog model-independent and avoids loading a tokenizer
for every session. A host that requires exact tokenizer, AST, sentence, or
domain boundaries can implement the custom strategy contract. Tokenizer- or
parser-specific built-ins should be added only with a measured quality gain and
a defined lifecycle for the required dependency.

## Rust configuration

Configure the catalog created by a normal local session through the typed
retrieval options:

```rust
use a3s_code_core::{
    RecursiveChunkingOptions, SessionOptions, WorkspaceChunkingStrategy,
    WorkspaceRetrievalOptions,
};

let chunking = RecursiveChunkingOptions::new(8 * 1024, 512)?
    .with_separators(["\n\n", "\n", ". ", " "])?;
let retrieval = WorkspaceRetrievalOptions::new(embedding_provider)
    .with_chunking_strategy(WorkspaceChunkingStrategy::Recursive(chunking));
let options = SessionOptions::new().with_workspace_retrieval(retrieval);
```

`with_chunking_config` controls the universal per-range byte ceiling, the line
strategy ceiling, and the per-file chunk count. `with_catalog_limits` controls
the file, chunk, retained-text, and lexical-index memory ceilings. Intentional
overlap counts against retained-text and vector-record budgets.

A host-supplied `WorkspaceServices` instance owns its `WorkspaceChunkCatalog`.
Session options reject a catalog strategy or limit override in that case rather
than silently ignoring it. Construct the host catalog with
`WorkspaceChunkCatalog::new_with_strategy` instead.

## Custom strategy contract

`CustomWorkspaceChunkingStrategy` is a synchronous `Send + Sync` Rust host
extension. It receives the normalized path, optional language, UTF-8 content,
and immutable hard limits. It returns zero-based, half-open byte ranges only.
A3S Code retains authority over chunk IDs, content digests, line anchors,
source revisions, and text ownership.

Code rejects a custom result unless all of these invariants hold:

1. Empty input produces no ranges; non-empty input produces at least one.
2. The first range starts at byte zero and the last ends at source length.
3. Every boundary is a UTF-8 boundary and every range is non-empty.
4. Starts and ends make forward progress; overlap is allowed but gaps are not.
5. Every range satisfies `max_bytes` and the list satisfies
   `max_chunks_per_file`.
6. Strategy failures and panics become bounded catalog failures; source text is
   not added to Code-owned errors or debug output.

The callback is trusted in-process code. It must be deterministic, must not
retain the borrowed input, and must not perform blocking I/O. Asynchronous
catalog reconciliation invokes CPU chunking outside Tokio worker threads.

## Asynchronous construction

Session creation starts one manifest-backed catalog projection and returns
without waiting for the corpus. Each admitted changed file is read once,
chunked once, published to the lexical catalog, then scheduled for embedding.
Unchanged files retain their existing immutable catalog partition. Changes and
deletions tombstone old content before replacement work begins.

Chunk strategy execution is therefore part of catalog construction, not query
execution. Repeated queries do not rechunk or re-embed unchanged files. Closing
the session cancels reconciliation and embedding work and releases catalog and
vector memory.

## Why an in-memory reranker is still needed

RRF already fuses exact, BM25, structural, and semantic channel ranks in
memory. It does not know that two overlapping windows may contain mostly the
same evidence. Configurable overlap therefore creates a concrete need for a
bounded second stage in A3S Code:

1. preserve the exact-identifier tier;
2. remove near-duplicate ranges and repeated boilerplate;
3. apply deterministic MMR-style diversity to a small fused candidate pool;
4. return the original RRF order if reranking fails or exceeds its budget.

This stage requires chunk ranges and channel evidence, so it does not belong in
the generic A3S Memory vector index. A host-injected local cross-encoder may be
evaluated later, default-off, behind explicit time, memory, cancellation, and
source-egress controls. No neural runtime or model download is required by the
baseline.

The release evaluation must report Recall@5/10, MRR, nDCG@10, exact-identifier
regression, duplicate-evidence rate, rerank p50/p95 latency, peak scratch bytes,
and end-to-end task completion for each strategy. The locked DeepSeek paired
task remains an integration gate, while deterministic fixtures remain the CI
correctness oracle.
