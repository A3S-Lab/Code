# Workspace Retrieval Operations

## Purpose and disposition

This runbook turns Workspace Retrieval qualification into an operational
decision. The capability is suitable for retrieval-dependent generation when
the gates below pass, but it remains explicit, session-bound, memory-resident,
and default-off. It does not create a durable workspace index, replace exact or
BM25 search, parse non-text assets, or change the compatible line-chunking and
RRF-only defaults.

The service boundary is:

```text
trusted opt-in + admitted UTF-8 source
        -> asynchronous session projection
        -> verified hybrid evidence
        -> governed model edit
        -> independent compile oracle
        -> session close and zero retained vectors
```

Each arrow has an observable gate. A fluent model answer is not an operational
success unless the repository outcome and lifecycle also pass.

## Enablement contract

The model-free CPU baseline is always available: exact, glob, incremental BM25,
Code Intelligence, RRF fusion, and the optional deterministic MMR stage use no
embedding model, neural reranker, remote service, or GPU. Dense semantic search
has a stricter definition and necessarily requires a text-to-vector function.
That function is optional and can run either behind a remote provider or as an
in-process CPU callback.

Rust, Node, Python, and Go hosts can already inject a local CPU implementation
through the typed `EmbeddingProvider` boundary. The qualified Python generation
run used the locked multilingual model on CPU. A3S CLI `e03b06e` now also ships
a qualified, default-off FastEmbed/ONNX adapter on Linux x64/ARM64, Windows x64,
and Apple Silicon. It is a host adapter behind the same Core contract: Code and
Memory still have no model-runtime dependency, and Intel macOS remains
model-free/remote because the pinned ONNX Runtime has no supported target.

A Rust host enables the capability only by injecting a typed
`WorkspaceRetrievalOptions` with an `EmbeddingProvider`. It disables an inherited
choice with `SessionOptions::without_workspace_retrieval()`. Node, Python, and
Go use their corresponding typed provider and option objects; a primitive
backend name is not a supported extension boundary.

The A3S CLI remote HTTP route additionally requires both `enabled = true` and
`allow_source_egress = true` in a trusted user ACL or an explicitly selected
configuration file. Its embedding route is independent from the chat model. A
local route instead selects one typed `local_cpu` block with an admitted
`artifact_manifest` and bounded `intra_threads`; it requires no source-egress
grant and is mutually exclusive with remote route fields. Model installation is
explicit and session startup never downloads artifacts. An automatically
discovered workspace ACL cannot enable either route. Use `a3s config validate`
and inspect `localCpuAvailable`, `localCpuUnavailableReason`, the two-input
microbatch limit, and the other non-sensitive `workspaceRetrieval` fields from
`a3s config show` before starting a session.

```acl
workspace_retrieval {
  enabled = true
  semantic_readiness_timeout_ms = 30000

  local_cpu {
    artifact_manifest = "models/multilingual-mini/model.acl"
    intra_threads = 2
  }
}
```

Only admitted UTF-8 text enters the catalog, chunker, or Embedding Provider.
PDF, Office, image, audio, archive, database, font, and other non-text inputs
belong to the separate knowledge-compilation system and must report zero
provider inputs here.

Configuration is evaluated when a session is created. Enabling, disabling, or
changing the provider, chunk strategy, dimension, normalization, budget, or
reranker therefore requires closing and recreating affected sessions.

## Production objectives

The fixed thresholds below are release gates for the locked reference profiles,
not promises about remote-model or network latency. Do not weaken a threshold
to make one run pass; investigate the regression or explicitly version the
profile.

| Signal | Objective | Qualified reference | Failure action |
| --- | ---: | ---: | --- |
| Explicit enablement and remote-egress authority when applicable | 100% | Hard validation gate | Disable and reject session construction |
| Generation pass rate | >= 0.90 | 9/9 = 1.0000 | Stop promotion; classify protocol, retrieval, edit, or compile failure |
| 95% Wilson lower bound | >= 0.65 | 0.7008 | Add independent repetitions; do not infer reliability from raw pass rate |
| Tool protocol, evidence Recall@5, hidden compile, and workspace integrity | 1.0000 each | 1.0000 each | Stop promotion and preserve the failing corpus digest |
| Session-construction p95, small generation fixture | <= 100 ms | 21 ms | Check for synchronous corpus reads or provider waits |
| Full-ready p95, small generation fixture | <= 5,000 ms | 919 ms | Continue lexical service; inspect provider and failed-file status |
| Initial first-publication p95 | <= 1,000 ms | 402 ms | Inspect batch coordination and provider latency |
| Edited-generation observation / first-publication p95 | <= 2,000 / 1,000 ms | 1,054 / 40 ms | Disable semantic edits if revisions stop advancing |
| Document-request amplification | <= 1.10x lower bound | 1.0000x | Inspect batching before increasing provider capacity |
| Non-text provider inputs | 0 | 0 | Treat as a source-egress incident and disable immediately |
| Exact / hybrid local query p95, 25,000 vectors | <= 30 / 100 ms | 8.294-12.302 / 51.145-54.429 ms | Fall back to exact/BM25 and inspect candidate or source-read growth |
| Runtime memory, 25,000-vector profile | <= 256 MiB default session budget | 41,397,932 accounted bytes | Reduce admitted scope or close the session; never spill vectors implicitly |
| Close p95 and post-close vectors, generation fixture | <= 6,000 ms and 0 records/bytes | 5,018 ms and 0/0 | Stop rollout if cleanup exceeds its bound or retains vectors |
| Repeated single-file replacement | Constant live record count | 64 generations, one live vector | Treat growth as a generation leak and close the session |
| Local CPU cold / warm provider call | < 30,000 / 5,000 ms | 7,045 / 19 ms | Keep exact/BM25 active; inspect artifact media, CPU support, and native thread limits |
| Local CPU cancellation / next-success recovery | < 1,000 / 30,000 ms | 0 / 267 ms | Reject promotion if a cancelled caller holds admission or starves the next live request |
| Local CPU peak RSS increase | < 1 GiB | 1,018,519,552 bytes | Reduce native microbatch/thread limits or disable the local route; never spill vectors implicitly |

DeepSeek turn latency (7,701/30,660 ms p50/p95 in the generation run), token
usage, and hidden Cargo compilation time are diagnostic. They are end-to-end
cost and capacity inputs, but they are not attributed to local retrieval SLOs.
The local CPU ACL-host run similarly reports 27,460/28,661 ms task p50/p95 and
12,163/12,342 ms first-publication p50/p95; those fresh-process values include
artifact admission, cold ONNX load, asynchronous indexing, and remote DeepSeek
latency rather than representing query-only latency.

## Required telemetry

Expose and retain aggregate, non-sensitive fields only:

- phase, coverage basis points, eligible/indexed/failed file counts, and indexed
  chunk count;
- catalog, source, and vector revisions;
- queue depth, vector record count, and accounted vector bytes;
- active vector engine plus Vec shadow phase, revision, records, accounted
  bytes, initialization/mutation outcomes, and compared/matching/mismatched/
  failed query counters;
- document inputs, physical provider requests, the batch-limit lower bound,
  flush reasons, amplification, and time to first ready publication;
- semantic/hybrid channel status, rerank mode/version, truncation, fallback
  reason, candidates evaluated, and checked scratch bytes;
- provider/model/revision identity without URL, headers, environment-variable
  names, credentials, source text, vectors, prompts, or snippets.

Alert on a degraded retrieval or Vec shadow phase, failed files above zero,
any Vec mismatch or failed query, a Vec record-count divergence after ready,
coverage that stops
advancing, source revision advancing without a later vector revision, request
amplification above 1.10x, non-text input above zero, a rerank fallback spike,
or nonzero Memory or Vec records/bytes after close. A shadow alert does not
authorize serving its result. Raw source and vector data must never be added to
telemetry to diagnose these conditions.

## State-based response

| State | Meaning | Operator response |
| --- | --- | --- |
| `disabled` | No provider-backed projection was admitted | Expected by default; exact, glob, BM25, and Code Intelligence remain available |
| `building` or partial coverage | An immutable generation is being embedded and published file-atomically | Serve exact/BM25 immediately; expose coverage and do not block session creation |
| `ready` | The observed source generation has full semantic coverage | Permit semantic/hybrid use while continuing source-revision verification |
| `degraded` | One or more admitted files or provider operations failed | Keep lexical paths available, inspect bounded error class and counters, and avoid retry storms |
| `closed` | Indexing and queries are cancelled and the projection is released | Require zero vector records/bytes; recreate a session before re-enabling |

A query must never render a stale or deleted chunk after the corresponding
workspace revision is observed. During replacement it may see the previous or
new immutable partition, never a partially written partition.

The retrieval phase and Vec shadow phase are independent. `ready` retrieval
with a `degraded` shadow means Memory remains eligible to serve while the Vec
differential evidence is unqualified. `active_vector_engine` must remain
`a3s_memory`; any other value is a release-blocking compatibility violation.

## Rollback

Rollback is configuration-only and does not require a migration:

1. In the trusted CLI ACL set `workspace_retrieval { enabled = false }`, or have
   an SDK host apply `without_workspace_retrieval()`.
2. Stop admitting new retrieval-enabled sessions.
3. Close existing affected sessions and verify `phase = closed`,
   `vector_records = 0`, and `vector_bytes = 0`.
4. Recreate sessions without retrieval. Exact, glob, BM25, and Code
   Intelligence paths remain available.
5. Preserve redacted status, counters, fixture/report versions, provider error
   class, and commit identity for diagnosis. Do not preserve source or vectors.

If only the optional deterministic reranker regresses, disable its typed block
and recreate sessions to return to RRF-only; semantic retrieval itself need not
be disabled. If the Embedding Provider, admission boundary, revision fencing,
memory accounting, or cleanup invariant regresses, disable the complete
retrieval runtime.

If only the internal Vec shadow regresses, Memory still owns every serving
result. Roll back the deployed Code binary to a revision before `4163d8e` and
recreate affected sessions; the temporary shadow carries no durable data to
migrate. This gate deliberately exposes no backend-name selector or separate
shadow toggle. Use the configuration rollback above if shadow latency or
resource use is unacceptable before the binary rollback is available.

## Release procedure

Run these checks from the Code repository, not the monorepo root:

```powershell
# Hermetic corpus and statistical contracts; no remote model call.
$env:PYTHONPATH = (Resolve-Path '.\sdk\python\python').Path
py -3.13 .\sdk\python\tests\test_workspace_retrieval_generation_real_deepseek.py `
  --validate-fixture

# Bounded replacement soak. This test is intentionally ignored by normal suites.
cargo test --locked -p a3s-code-core --lib `
  agent_api::retrieval_qa_tests::repeated_source_generations_replace_vectors_without_accumulation `
  -- --ignored --exact --test-threads=1

# Local release latency, Memory/Vec parity, batching, quality, and cleanup gates.
cargo run --release -p a3s-code-core --example workspace_retrieval_benchmark
```

For a private qualification environment, build the Python SDK, install the
optional locked Sentence Transformers runtime, set `A3S_REAL_EVAL_ROOT` to the
monorepo containing the authorized `.a3s/config.acl`, and run the generation
matrix documented in `sdk/evaluation/README.md`. Never move repository secrets
into shared CI. Shared Code CI uses deterministic embeddings and must pass the
64-generation soak on Ubuntu, macOS, and Windows. Code
[CI #249](https://github.com/A3S-Lab/Code/actions/runs/31862118069) is the first
complete passing portability run for that gate. CLI
[CI #31917686424](https://github.com/A3S-Lab/CLI/actions/runs/31917686424)
separately provisions a digest-locked smoke artifact and performs real offline
admission, inference, cancellation, recovery, and RSS checks on Linux x64/
ARM64, Windows x64, and macOS ARM64. It also fails before model loading for a
simulated missing x86-64-v3 baseline.

Promote only when the real generation report, deterministic quality and
security suites, release benchmark, three-platform soak, SDK alignment check,
formatting, and relevant builds all pass for the same commit chain. Nine clean
generation trials qualify this bounded opt-in workflow. They do not establish
a universal success rate across languages, repository sizes, embedding routes,
or future model revisions, so automatic enablement requires a separately
versioned evaluation and compatibility decision.
