# Workspace Retrieval Backends

Status: shipped in the current development line. The workspace catalog has a
single lexical backend boundary and a single semantic vector authority.

## Contract

Code owns file admission, deterministic chunking, stable chunk IDs, source
digests, revision fencing, path filters, result projection, and current-source
verification. The backend only indexes the admitted chunk text and returns
bounded ranked ordinals.

The product default is the official `zvec-rust` 0.7 FTS binding, identified in
result metadata as `zvec_rust_fts_v1`. Every native partition, including a
single-document partition, goes through that binding so production behavior is
the same at every workspace size. A deliberately minimal build can select the
dependency-free scorer, identified as `portable_bm25_v1`; that is an explicit
build-time fallback and does not silently change the configured engine.

Semantic retrieval uses the exact `a3s-memory` in-memory adapter. Embeddings
are validated once, grouped into bounded provider requests, and published as
immutable per-file partitions. There is no second semantic index, migration
shadow, or runtime authority selector.

## Native lexical lifecycle

For each admitted catalog partition, the zvec adapter:

1. Reuses Code's canonical tokenizer and normalizes the tokens into one FTS
   field. This keeps identifier splitting and CJK bigrams identical across
   exact, lexical, and hybrid channels.
2. Creates a temporary collection with a whitespace FTS index and inserts
   documents in bounded batches. Native primary keys are generated from dense
   ordinals; the original Code chunk ID stays in the surrounding catalog.
3. Flushes and closes the collection before publishing the partition. Queries
   reopen read-only for the shortest possible scope, map native keys back to
   immutable chunks, and discard non-finite scores.
4. Retains only the persisted temporary directory and compact key/term
   metadata. The source catalog remains authoritative and owns cleanup.

The current catalog replaces files atomically, so a per-file native collection
is intentionally ephemeral. A process-wide native-operation gate bounds the
number of simultaneous RocksDB handles during concurrent file updates and
queries. At most four hot read-only collections are retained across sessions;
colder partitions open, query, and close within one bounded operation. This
protects descriptor budgets while preserving immutable snapshot semantics. A
future workspace-wide collection may remove that serialization, but must retain
the same revision and source-verification contract.

## Query and rerank flow

The indexed lexical path first uses term metadata to select candidate files,
then asks zvec for bounded top-k FTS results per selected partition. Exact
literal, optional Code Intelligence symbols, lexical, and semantic channels
are fused with deterministic reciprocal-rank fusion (`k=60`). The optional
CPU reranker examines a bounded candidate pool, protects exact identifiers,
and applies overlap/boilerplate diversity. It returns the original RRF order
when configuration or scratch budgets are invalid.

The managed `grep` path remains exhaustive for literals and regular
expressions. It does not depend on an embedding provider or native index.
Every semantic or lexical snippet is reread through `WorkspaceServices` and
must match its recorded digest and byte range before it reaches the model.

## Packaging and runtime admission

`core/resources/zvec-runtime-manifest.json` records the supported target,
version, archive, SHA-256, and native library path. Release packaging must set
`ZVEC_LIB_DIR` to the verified target directory, run
`scripts/package_zvec.sh`, and preserve a relocatable loader path beside the
Code executable. Unsupported targets fail closed unless a developer explicitly
uses the script's diagnostic override.

The native feature is optional at compile time so downstream minimal builds
can use `--no-default-features` and the portable scorer. Official SDKs and
release binaries enable the native feature and must include the matching
library; they must not download an unverified library at runtime.

Persistent generations use an atomic `CURRENT` pointer and a schema-v2
manifest. On reopen, Code verifies every retained chunk's text digest, stable
chunk identity, byte/line range, canonical source digest, and duplicate-ID
constraint before exposing the native collection. A damaged or incompatible
generation is treated as absent and is rebuilt from the current catalog; it is
never served as a partially trusted index.

## Qualification gates

- The locked exact/identifier/CJK relevance fixture passes for both engines.
- Catalog creation, replacement, deletion, lag recovery, budget rejection,
  stale-source filtering, and cleanup are atomic and deterministic.
- Concurrent replacement and query tests pass without descriptor exhaustion.
- Default and `--no-default-features` builds compile and run their respective
  lexical suites; explicit native selection fails with a typed configuration
  error when the feature is absent.
- Node.js, Python, and Go expose the same lexical enum, status/result shape,
  typed chunking/reranking options, cancellation, and close semantics.
- Package-manifest hash and loader checks run for every supported target.

Rollback is configuration-only: disable semantic/hybrid modes or select the
portable lexical engine in a minimal build. Exact search, managed grep, Code
Intelligence, and source verification remain available in every case.
