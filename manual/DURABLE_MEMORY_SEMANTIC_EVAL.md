# Durable Memory Semantic Evaluation

`DM-SEM1` is the deterministic serving and isolation gate for optional durable
semantic recall. It demonstrates that a Rust host can add cross-language
retrieval without giving the embedding provider or vector index authority to
decide which memory revision may enter a model prompt.

## Evaluated path

The fixture drives the production `AgentSession` context path:

```text
Active repository snapshot ──> bounded embedding ──> caller-owned VectorIndex
                                                               │
query ──> lexical branch ───────────────────────────────────────┤
  └────> bounded query embedding ──> vector candidates ─────────┘
                                      │
                                      v
                     exact namespace/revision/content re-read
                                      │
                                      v
                        deterministic RRF and result bound
                                      │
                                      v
                         final assembly ──> admission ──> model
```

The A3S Memory repository remains the serving authority. A vector hit is only a
candidate. Code re-reads the bound namespace and requires the current `Active`
revision and exact content digest. The final selected revision must still
persist admission before model use.

## Versioned fixture

The source is
[`evaluation.json`](../core/tests/fixtures/durable-memory-semantic-v1/evaluation.json).
It declares:

- four English Active procedural memories;
- Chinese, Japanese, Korean, and Arabic queries with no lexical overlap;
- exact seven-dimensional unit vectors from a deterministic fixture provider;
- one Candidate, one foreign-tenant Active node, and one Active node revised
  after its old vector was published;
- lexical, semantic-candidate, context, provider-call, admission, and leakage
  bounds;
- durable-memory binding schema 5, semantic binding v1, and RRF `k=60` profile
  identities.

The fixture provider is intentionally not a learned embedding model. Its
declared vectors isolate the serving contract from model variance and network
availability.

## Locked result

Run from the Code crate workspace:

```text
cargo test -p a3s-code-core --test durable_memory_semantic_eval -- --nocapture
```

The retained marker is:

```json
{
  "schemaVersion": 1,
  "bindingSchemaVersion": 5,
  "semanticBindingSchema": "a3s.code.memory.semantic-recall-binding.v1",
  "fusionProfile": "a3s.code.memory.hybrid.rrf-k60.v1",
  "queries": 4,
  "semanticRecallAt1": 1.0,
  "lexicalPositiveHits": 0,
  "negativeHits": 0,
  "modelCalls": 4,
  "admissions": 4,
  "maximumContextNodes": 1,
  "candidateForeignOrStaleLeaks": 0
}
```

The companion
[`durable_memory_semantic`](../core/tests/durable_memory_semantic.rs) contract
tests also lock hybrid de-duplication, byte-for-byte lexical fallback after an
embedding failure, Candidate rejection during index replacement, stale
revision filtering, namespace and same-namespace generation partition
isolation, public `Send + Sync`, and schema selection. Session-persistence
tests reject drift in provider/model
identity, authority digest, vector budgets, semantic policy, and embedding
execution policy.

The separate [semantic refresh gate](DURABLE_MEMORY_SEMANTIC_REFRESH.md) covers
complete dual-budget repository snapshots, explicit atomic rebuild, source
drift cleanup, serialized live-generation mutation, shared-index revision CAS,
and refresh receipts.

## What this gate does not claim

This gate does not establish production embedding quality, real-provider
latency or cost, scheduled refresh, durable remote-vector storage,
distributed lease policy, remote CAS/failover behavior, long-horizon
consolidation/decay quality, or
representative tenant scale. Those remain `DM-PROD1` host qualifications.

The host must authorize memory text and query egress to the selected embedding
provider and must protect the vector index, whose labels retain node IDs,
revisions, and content digests. The persisted authority digest is an exact
secret-free identity assertion, not proof of remote backend continuity.
