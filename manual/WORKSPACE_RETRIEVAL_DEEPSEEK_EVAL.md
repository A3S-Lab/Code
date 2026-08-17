# Workspace Retrieval DeepSeek Evaluation

Status: semantic ablation and adversarial rerank slice passed on 2026-08-14;
the built-in chunk-strategy matrix and real CLI ACL-host composition passed on
2026-08-15. The `CODE-B2` cross-file batching rerun passed the same day. The
public Node.js, Python, and Go SDK matrix then passed at Code `cde887b`. The
Rust whole-file custom-strategy negative control remained observable but was
not quality qualified. The opt-in in-process CLI CPU route passed its final
multilingual DeepSeek and four-target offline-runtime gates on 2026-08-16 at
CLI `e03b06e`.
The published `v7.0.1` checkout at Code `5aa9642` repeated the public
three-SDK DeepSeek matrix and the Core adversarial suite on 2026-08-17; every
gate passed again.

This report records the reproducible real-chat-model ablation for A3S Code's
session-bound, in-memory Workspace Retrieval. It complements the deterministic
[release qualification](WORKSPACE_RETRIEVAL_QA.md); it does not replace its
independent relevance and security oracles.

## Decision: chunk text, reject non-text

Workspace Retrieval has a deliberately narrow input contract:

```text
workspace manifest
  |-- admitted UTF-8 source/text
  |     -> deterministic chunks
  |     -> asynchronous embedding batches
  |     -> file-atomic in-memory vector partitions
  |     -> verified semantic/hybrid results
  |
  `-- image/PDF/Office/archive/audio/video/database/model/native asset
        -> excluded before chunking and embeddings
        -> separate knowledge compiler boundary
```

A3S Code does not parse, OCR, transcribe, unpack, or vectorize non-text
workspace assets. A separate knowledge compiler owns those transformations and
may later publish an explicitly typed text artifact. Code must never infer such
a handoff from an arbitrary generated file or silently run a second document
pipeline.

The default text chunker is deterministic and has no overlapping window:

| Limit | Default |
| --- | ---: |
| Lines per chunk | 80 |
| UTF-8 bytes per chunk | 65,536 |
| Chunks per file | 128 |
| Catalog files | 50,000 |
| Catalog chunks | 100,000 |
| Retained source text | 64 MiB |
| Conservatively estimated lexical index | 64 MiB |

The line or byte boundary that is reached first closes a chunk. A single line
over the byte limit is split only on UTF-8 scalar boundaries. Chunk identity is
derived from path, full-content digest, and byte range. The final result path is
reread and its full-file digest and exact byte range are verified before source
text is rendered.

## First-principles adversarial plan

The evaluation starts with externally observable invariants rather than the
implementation's own success state.

| Boundary | Adversarial condition | Independent observable | Required gate |
| --- | --- | --- | --- |
| Host enablement | Configure a provider, then explicitly clear retrieval | Tool schema, status, and recording-provider counters | Disabled schema has no semantic/hybrid modes and provider calls remain zero |
| ACL authority | Discover a workspace chat route while a temporary trusted user layer enables retrieval | Effective layer count, redacted config projection, selected chat model, and recording endpoint | Exactly two layers; DeepSeek remains the chat model; only the trusted layer grants embedding egress; no key or endpoint appears in output |
| Catalog ownership | Combine host-supplied workspace services with an explicit chunk strategy | Host catalog debug state, session option state, and real session construction | The host configures one catalog before services attach; session options contain no catalog override; Core's ownership guard accepts the composition |
| Task utility | Run the same task and corpus with retrieval disabled and enabled | Exact final identifier, independent expected path, and recorded rank | Enabled task accuracy must exceed the paired disabled ablation |
| Chunk boundary | Place an answer after 90 filler lines in a file | Catalog chunk count and expected-path rank | File becomes two chunks and the answer-bearing chunk remains retrievable |
| Strategy isolation | Hold corpus, model, ranking, prompt, and budgets fixed while rotating line, fixed-window, and recursive splitters | Exact completion, rank, chunk/vector bytes, and lifecycle counters per strategy | Every built-in arm passes the same quality and safety gates; timings remain observations, not causal claims |
| Custom strategy trust | Return one valid whole-file range from a Rust-host callback | Independent token-containment fixture and real task accuracy | Range safety and lifecycle must pass, but the strategy is not quality-qualified merely because its ranges are valid |
| Non-text boundary | Add PDF, PPTX, and MP3 assets containing a recognizable sentinel | Eligible/indexed file counts and provider input sentinel counter | Zero non-text chunks, vectors, or provider inputs |
| Tool protocol | Ask the chat model to inspect its schema and make exactly one search call | Captured tool start/end events and arguments | One successful Search call with semantic when enabled and BM25 when disabled |
| Session lifecycle | Close every session after the turn | Post-close status and vector accounting | Zero retained vector records and bytes |
| Local ranking | Label the expected path independently from returned results | Recall@5 and reciprocal rank | Recall@5 and MRR are 1.0 on the locked tasks |

Deterministic suites separately cover symlink and hard-link attacks, stale
generation fencing, provider corruption, cancellation, confidentiality, and
cross-session isolation. Three live-model tasks are not a substitute for those
larger adversarial suites.

## Test configuration

The repository `.a3s/config.acl` was parsed through the Code ACL loader without
printing provider URLs, headers, environment-variable names, or secret values.
The selected chat model was `deepseek/deepseek-v4-pro`.

The ACL did not authorize workspace source egress or configure an embedding
model. Therefore the test used a process-local, deterministic eight-dimensional
embedding oracle injected through the public `EmbeddingProvider` interface.
DeepSeek was the real model under test for schema inspection, tool selection,
evidence use, and exact task completion. It was not treated as an embedding or
ranking oracle, and fixture source was not sent to a second remote model.

Each of three tasks ran twice in a fresh isolated session:

- disabled after an earlier typed retrieval choice was cleared with
  `without_workspace_retrieval()`;
- enabled with the same provider, prompt, corpus, permissions, temperature, and
  maximum tool rounds.

The corpus contained 30 text/source files, three non-text assets, 31 text
chunks, three semantic targets, three lexical decoys, and 24 distractors. One
target was placed in the second chunk of a 95-line source file. Only Search was
permitted, temperature was zero, delegation was disabled, and the model had to
make exactly one Search call and return one exact Rust identifier or
`NOT_FOUND`.

## Task and retrieval results

| Metric | Retrieval enabled | Retrieval disabled |
| --- | ---: | ---: |
| Exact task completion | 1.0000 (3/3) | 0.0000 (0/3) |
| Tool protocol compliance | 1.0000 (3/3) | 1.0000 (3/3) |
| Expected path Recall@5 | 1.0000 (3/3) | 0.0000 (0/3) |
| Expected path MRR | 1.0000 | 0.0000 |
| Expected path rank | 1, 1, 1 | absent, absent, absent |
| Search mode | semantic | BM25 |

All disabled runs returned one lexical decoy and the model correctly followed
the required fallback protocol, but it could not recover the expected
identifier. All enabled runs returned five candidates, ranked the independently
labeled file first, and completed with the exact identifier. The multi-chunk
backpressure task also ranked its answer-bearing file first.

## Adversarial deterministic-rerank slice

The first `WSR-EVAL2` Core slice compared compatibility RRF with the opt-in
deterministic reranker under a deliberately hostile duplicate-evidence
distribution. It is not the complete chunk-strategy/SDK/host matrix and does
not change the RRF-only default.

Each task added eight distinct text files containing the same lexical query
and a deterministic semantic-collision marker, but not the expected answer.
This produced 54 eligible text files, 55 chunks, and three still-excluded
non-text assets per isolated session. The process-local embedding oracle made
the collision files compete in both lexical and semantic channels. RRF and
rerank used the same corpus, query, DeepSeek model, prompt, limit, provider,
permissions, and fresh-session lifecycle. Execution order alternated by task
to reduce fixed arm-order bias.

| Metric | RRF only | Deterministic rerank |
| --- | ---: | ---: |
| Exact task completion | 0.0000 (0/3) | 1.0000 (3/3) |
| Tool protocol compliance | 1.0000 (3/3) | 1.0000 (3/3) |
| Expected path Recall@5 | 0.0000 (0/3) | 1.0000 (3/3) |
| Expected path MRR | 0.0000 | 0.3889 |
| Expected path rank | absent, absent, absent | 3, 2, 3 |
| Final Top-5 collision evidence | 1.0000 (15/15) | 0.6667 (10/15) |
| Total model tokens | 19,932 | 18,276 |
| DeepSeek turn p95 | 209,318 ms | 28,102 ms |
| Maximum vector bytes | 15,595 | 15,595 |
| Document-request amplification | 54.0x | 54.0x |
| Non-text provider inputs | 0 | 0 |

The deterministic arm evaluated and selected ten fused candidates per query
before the existing two-times authoritative-source verification overfetch was
reduced to the requested Top-5. Across the three runs, 22 of 30 evaluated
candidates were classified as near duplicates and 22 of 30 overfetch
selections remained near duplicates. This is expected for a corpus dominated
by adversarial collisions: the useful effect is not complete elimination, but
promotion of independently relevant evidence into every final Top-5. The
final collision rate and expected-path rank therefore remain the end-user
quality observables.

The deterministic operation retained at most 12,239 feature bytes and
accounted 18,346 scratch bytes, with no truncation or fallback. Both arms used
the same 15,595 vector bytes. Every session reached 100 percent coverage,
reported zero failed files and zero non-text provider inputs, and released all
vector records and bytes after close.

The live turn timings are six remote-model observations, including one
209-second RRF turn. They show the measured end-to-end run only; they do not
attribute a speedup to the local reranker. The release benchmark in the QA
report remains the latency gate. The 54x provider-request amplification is
also unchanged by reranking and remains owned by `CODE-B2`.

## Built-in chunk-strategy matrix and custom negative control

The second `WSR-EVAL2` slice isolated the chunking factor while holding the
corpus, deterministic embedding oracle, hybrid search, deterministic reranker,
DeepSeek model, prompt, Top-5 limit, permissions, and fresh-session lifecycle
constant. Each of the three tasks exercised four strategies, for twelve real
DeepSeek turns. Strategy order rotated by task to avoid always assigning one
arm the earliest or latest remote-service window.

The built-in arms were compatibility line chunking, a 512-byte fixed window
with 64-byte overlap, and a 512-byte recursive splitter with the explicit
separator order `\n\n`, `\n`, `. `, and space. A Rust-host whole-file splitter
was included as an intentionally coarse custom-strategy negative control. An
independent local catalog gate first proved exact chunk counts and that every
answer identifier remained complete inside at least one chunk; therefore a
model failure could not be dismissed as an accidentally severed identifier.

| Metric | Line | Fixed 512/64 | Recursive 512/64 | Whole-file custom control |
| --- | ---: | ---: | ---: | ---: |
| Quality gate required / passed | yes / yes | yes / yes | yes / yes | no / no |
| Exact task completion | 1.0000 (3/3) | 1.0000 (3/3) | 1.0000 (3/3) | 0.6667 (2/3) |
| Tool protocol compliance | 1.0000 | 1.0000 | 1.0000 | 1.0000 |
| Expected path Recall@5 | 1.0000 | 1.0000 | 1.0000 | 1.0000 |
| Expected path MRR | 0.5000 | 0.5000 | 0.5000 | 0.5000 |
| Locked chunks | 31 | 38 | 39 | 30 |
| Chunks per text file | 1.0333 | 1.2667 | 1.3000 | 1.0000 |
| Maximum vector bytes | 8,387 | 9,444 | 9,595 | 8,236 |
| Provider input bytes, three sessions | 18,279 | 19,815 | 20,007 | 18,279 |
| Index-ready p50 / p95 | 348 / 392 ms | 377 / 442 ms | 412 / 418 ms | 424 / 445 ms |
| DeepSeek turn p50 / p95 | 17,412 / 25,989 ms | 6,431 / 6,846 ms | 6,105 / 7,268 ms | 6,499 / 27,995 ms |
| Total model tokens | 14,671 | 13,939 | 14,174 | 14,150 |
| Document-request amplification | 30.0x | 30.0x | 30.0x | 30.0x |
| Non-text provider inputs | 0 | 0 | 0 | 0 |
| Sessions fully released | 3/3 | 3/3 | 3/3 | 3/3 |

Every expected path ranked second because the locked lexical decoy remained
ahead of it; DeepSeek still returned the exact answer for all nine built-in
runs. The whole-file control failed the long backpressure task in both the
initial adversarial review run and the final report run, despite finding the
expected path at rank two. Its large answer chunk also required 4,041 rerank
feature bytes on that task, versus 725 or fewer for the built-ins. This is the
desired trust-boundary finding: Core can prove custom ranges are complete,
bounded, UTF-8-safe, and releasable, but only task evidence can qualify their
context quality.

Fixed and recursive overlap increased vector memory by 12.60 and 14.40 percent
over line chunking on this corpus, while provider input bytes increased by
8.40 and 9.45 percent. In that pre-`CODE-B2` matrix, neither changed the frozen
30x per-file request amplification. The remote turn percentiles and token totals
are reported for reproduction only; three observations per arm cannot establish
a latency or cost advantage. All built-ins pass this slice, but the evidence
does not justify changing the line default.

## Real CLI ACL-host composition

The Core matrices above inject typed retrieval options directly. The final host
variant instead exercised the production `a3s code exec` path, effective ACL
layering, shared manifest backend, asynchronous catalog, semantic runtime,
hybrid Search metadata, JSONL projection, session close, and the real DeepSeek
tool loop. A3S CLI `main` commit `f435950` pins Code `bdb86e17`.

The test created a workspace below the A3S monorepo root, allowing normal
ancestor discovery to select the repository `.a3s/config.acl` for the
`deepseek/deepseek-v4-pro` chat route. A temporary trusted user ACL supplied a
process-local embedding endpoint, the two explicit egress gates, recursive
512/64 chunking with explicit separators, and the typed deterministic
reranker. The test did not copy or print the repository ACL and scanned both
stdout and stderr for the local API-key marker and endpoint.

The first live attempt failed before any DeepSeek call because the CLI placed
the chunk strategy in per-session options while also supplying host-owned
workspace services. Core's ownership guard correctly rejected that conflict.
The host fix now configures `ManifestWorkspaceBackend` once, attaches its
catalog to the shared services, and passes only provider/index/rerank options to
each session. Exec, TUI, and Code Web use the same ownership split.

| Quality metric | ACL-host result |
| --- | ---: |
| Exact task completion | 1.0000 (3/3) |
| Exact tool protocol | 1.0000 (3/3) |
| Precision@5 | 0.2000 |
| Precision among returned results | 0.4286 (3/7) |
| Mean returned results | 2.3333 |
| Expected-path Recall@5 | 1.0000 |
| Expected-path MRR | 0.5000 |
| Expected-path nDCG@5 | 0.6309 |
| Mean relevant rank | 2.0000 |

The runtime returned 2, 2, and 3 positive candidates instead of padding Top 5
with zero-similarity results. Precision@5 therefore uses the fixed five-slot
denominator, while returned-result precision separately reports the density of
evidence exposed to the model. Every independently labeled target ranked
second behind its lexical decoy, and DeepSeek still emitted the exact answer.

| Operational metric | Result per session unless noted |
| --- | ---: |
| Retrieval phase / coverage | `ready` / 100% |
| Eligible / indexed / failed files | 30 / 30 / 0 |
| Indexed chunks / vector records | 39 / 39 |
| Accounted vector bytes | 9,595 |
| Embedding requests | 2 (1 document + 1 query) |
| Embedding inputs | 40 (39 document + 1 query) |
| Document batches / physical requests / lower bound | 1 / 1 / 1 |
| Document-request amplification | 1.0x |
| Time to first file-atomic publication, p50 / p95 | 9 / 10 ms |
| Non-text provider inputs | 0 |
| End-to-end command p50 / p95 | 11,220 / 31,116 ms |
| Total DeepSeek tokens, three tasks | 39,471 |

End-to-end command timing includes process and session setup, asynchronous
indexing, remote-model latency, tool execution, and completion. It is not a
retrieval-only latency claim; the release benchmark remains that gate. The
schema-v2 host report cross-checks Core status against independent loopback
provider counters: both observe one document request for 39 chunks and a
one-request lower bound. CLI session close runs before the result process
terminates, while the Core weak-reference suite remains the authority for zero
retained vector allocations.

This qualifies the ACL-host arm of `WSR-EVAL2`, not a new default chunker or
reranker. The complete host-side threat matrix and machine-readable schema are
documented in the
[A3S CLI evaluation](https://github.com/A3S-Lab/CLI/blob/main/docs/workspace-retrieval-evaluation.md).

## Cross-SDK public API matrix

Code `cde887b` subsequently ran the same versioned corpus, deterministic
embedding oracle, recursive 512/64 strategy, typed reranker, and real DeepSeek
route through Node.js, Python, and the Go-to-Rust bridge. Each SDK passed 3/3
exact completions and one-Search protocols with Precision@5 0.2,
returned-result precision 0.4286, Recall@5 1.0, MRR 0.5, nDCG@5 0.6309, 1.0x
document-request amplification, zero non-text inputs, and complete release.
The normalized report, latency/token values, rejected prompt ambiguity, and
commands are in the [cross-SDK contract](../sdk/evaluation/README.md). These
three-task arms close `WSR-EVAL2` for API parity, not for a default change.

### v7.0.1 post-release rerun

On 2026-08-17 Code `5aa9642` loaded the repository-authorized
`deepseek/deepseek-v4-pro` route and repeated the same locked fixture through
the published-version Node.js, Python, and Go SDK surfaces. All nine exact
tasks and all nine one-Search protocols passed. Every arm retained Precision@5
0.2, returned-result precision 0.4286, Recall@5 1.0, MRR 0.5, nDCG@5 0.6309,
expected-path ranks 2/2/2, one document request for the one-request lower
bound, zero non-text provider inputs, and complete vector release after close.

| Observed metric, p50 / p95 | Node.js | Python | Go |
| --- | ---: | ---: | ---: |
| Session construction | 25 / 97 ms | 16 / 268 ms | 15 / 23 ms |
| Index ready | 320 / 395 ms | 279 / 376 ms | 81 / 151 ms |
| Time to first ready publication | 5 / 12 ms | 7 / 13 ms | 2 / 3 ms |
| DeepSeek turn | 16,033 / 16,538 ms | 15,552 / 23,751 ms | 16,636 / 19,009 ms |
| Session close | 3 / 10 ms | 1 / 2 ms | 0 / 14 ms |
| Total DeepSeek tokens, three tasks | 14,540 | 14,784 | 14,171 |

The three SDK arms consumed 43,495 model tokens in total. The same checkout
also passed all three serial Core adversarial scenarios covering prompt
injection containment, absolute-path workspace isolation, and cancellation
before a post-cancellation side effect. Lightweight Node.js and Python
real-config smoke runs completed a model turn in 3,938 ms and 4,592 ms,
respectively. These remote timings remain diagnostic samples rather than
release latency gates; the rerun changes no chunking, retrieval, or reranking
default.

## Pre-CODE-B2 construction, model, and lifecycle measurements

These live E2E samples are debug-build observations with only three samples per
arm. Their percentiles describe this run; the release benchmark below is the
performance gate.

| Metric | Retrieval enabled | Retrieval disabled |
| --- | ---: | ---: |
| Session construction p50 / p95 | 8 / 12 ms | 6 / 12 ms |
| Index ready p50 / p95 | 548 / 560 ms | not applicable |
| DeepSeek turn p50 / p95 | 7,464 / 7,950 ms | 7,289 / 7,608 ms |
| Close p50 / p95 | 5,006 / 5,007 ms | 5,007 / 5,010 ms |
| Total model tokens | 15,345 | 13,391 |

Enabled sessions used 1,954 more tokens in total, a 14.59 percent increase for
the retrieved evidence in this small corpus. Close time was effectively the
same in both arms, so the approximately five-second close observation is a
session baseline in this environment rather than retained vector cleanup.
Every enabled session reported `ready`, 100 percent coverage, and zero failed
files. Every session released all vector records and accounted vector bytes on
close.

## Pre-CODE-B2 chunk, vector, egress, and request metrics

The following values were identical across the three enabled sessions unless a
range is shown:

| Metric | Result |
| --- | ---: |
| Workspace entries | 33 |
| Eligible/indexed text files | 30 / 30 |
| Excluded non-text files | 3 |
| Indexed chunks / vector records | 31 / 31 |
| Chunks per text file | 1.0333 |
| Failed files | 0 |
| Vector bytes | 8,387 |
| Accounted bytes per vector record | 270.55 |
| Provider input bytes, including one query | 6,089-6,100 |
| Document inputs / query inputs | 31 / 1 |
| Document requests / query requests | 30 / 1 |
| Document requests per chunk | 0.9677 |
| Non-text provider inputs | 0 |
| Post-close vector records / bytes | 0 / 0 |

This table is the frozen failing baseline captured before `CODE-B2`. The default
executor can admit up to 64 inputs and 256 KiB of text per batch,
so this fixture could fit its 31 document chunks in one provider request. The
pre-`CODE-B2` runtime instead made 30 document requests because projection was
scheduled and published per file. It batched the two chunks from the long file
together but did not coalesce ready chunks across files. Relative to the
input-count lower bound for this fixture, provider request amplification is
30x. This is the clearest measured optimization opportunity; it does not affect
retrieval correctness or file-atomic publication, but it would dominate remote
provider overhead on many-small-file workspaces.

## Deterministic release benchmark

The permanent release benchmark uses 25,000 normalized 384-dimensional vectors,
top-20, 20 warmups, and 100 measured queries. Provider network latency is
excluded; hybrid includes authoritative source reads from the warm OS cache.

| Metric | Result | Gate |
| --- | ---: | ---: |
| Exact search p50 / p95 / max | 8.153 / 15.127 / 24.077 ms | p95 <= 30 ms |
| Hybrid search p50 / p95 / max | 62.195 / 81.999 / 92.464 ms | p95 <= 100 ms |
| Exact index build | 94.333 ms | reported |
| Workspace projection build | 1,272.573 ms | reported |
| Session construction | 30.132 ms | no corpus/embedding wait |
| Vector records / dimension | 25,000 / 384 | fixed profile |
| Vector bytes | 41,397,932 | bounded |
| Source bytes | 4,000,000 | fixed profile |

Both locked latency gates passed. A3S Memory's independent exact-vector run on
the same profile reported p50 7.797 ms, p95 10.725 ms, max 21.591 ms, and
39,900,117 accounted bytes.

## CODE-B2 cross-file batching qualification

On 2026-08-15 the same `deepseek/deepseek-v4-pro` configuration and deterministic
embedding oracle were rerun after introducing the session-local cross-file
coordinator. The coordinator receives one complete immutable catalog generation,
so an underfilled tail flushes immediately on generation completion; it never
waits for another source revision. Input, text-byte, or expected-vector-byte
limits can flush earlier. This makes the pre-provider queue residence bounded by
synchronous planning rather than an artificial timer.

Measure provider request amplification as:

```text
actual document requests
-----------------------------------------------
sum per session of the batch-limit lower bound
```

The implementation and report schema count document inputs and bytes, logical
batches, physical provider requests including retries, the lower bound implied
by all three limits, every flush reason, time to first file-atomic publication,
and non-text inputs. Results were:

| Profile | Document inputs per session | Lower bound | Actual requests | Amplification | First ready | Non-text inputs |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Paired semantic task | 31 | 1 | 1 | 1.0x | 0-2 ms | 0 |
| Collision/rerank task | 55 | 1 | 1 | 1.0x | 1 ms | 0 |
| Line strategy, three sessions | 31 each | 1 each | 3 total | 1.0x | 0-1 ms | 0 |
| Fixed 512/64, three sessions | 38 each | 1 each | 3 total | 1.0x | 0-1 ms | 0 |
| Recursive 512/64, three sessions | 39 each | 1 each | 3 total | 1.0x | 0-1 ms | 0 |
| Rust whole-file control, three sessions | 30 each | 1 each | 3 total | 1.0x | 0-2 ms | 0 |
| Real CLI ACL host, three sessions | 39 each | 1 each | 3 total | 1.0x | 8-10 ms | 0 |
| Release profile | 25,000 | 391 | 391 | 1.0x | 9-10 ms | 0 |

The paired semantic arm again completed 3/3 tasks versus 0/3 disabled, with
Recall@5 and MRR both 1.0. The collision rerank arm retained its 3/3 versus 0/3
quality result and reduced both former 54x request measurements to 1.0x. All
three built-in chunking arms remained 3/3; the valid whole-file custom negative
control remained 2/3. Every run reached full coverage and released all vector
state. The release run recorded 391 logical batches and 391 physical document
requests, RRF/deterministic hybrid p95 of 57.499/49.560 ms, workspace builds of
1,201.453/1,104.608 ms, session construction of 13.943/6.142 ms, and 41,397,932
vector bytes. All latency, scratch, memory, amplification, and cleanup gates
passed.

Deterministic adversarial tests additionally force each count/text/vector flush,
split one file across batches, corrupt a later provider response, retry a
request, and replace a catalog revision during an active cross-file batch. They
prove that no partial file becomes visible, already valid siblings survive a
later failure, retries are counted, stable sibling IDs are retained, and the
superseded batch is cancelled before publication.

## In-process local CPU host qualification

The final A3S CLI profile replaced the HTTP embedding route with the trusted
typed `local_cpu` ACL block while retaining the same recursive 512/64 corpus,
real `deepseek/deepseek-v4-pro` chat route, exact task prompts, permissions, and
fresh-session lifecycle. The revision-locked multilingual FastEmbed/ONNX model
was installed before the run; session startup performed no download and needed
no source-egress grant.

| Metric | In-process local CPU result |
| --- | ---: |
| Exact task/tool completion | 3/3 / 3/3 |
| Relevant ranks | 5 / 2 / 3 |
| Recall@5 / MRR / nDCG@5 | 1.0000 / 0.3444 / 0.5059 |
| Files / chunks / non-text inputs | 30 / 39 / 0 |
| Document calls / configured lower bound | 20 / 20 (1.0x) |
| First publication p50 / p95 | 12,163 / 12,342 ms |
| End-to-end task p50 / p95 | 27,460 / 28,661 ms |
| Total DeepSeek tokens | 40,241 |
| Post-close vectors / bytes | 0 / 0 |

The isolated Windows provider record reports 7,045/19 ms cold/warm calls,
0 ms caller cancellation, 267 ms recovery to the next successful request, and
a 1,018,519,552-byte peak-RSS increase below 1 GiB. Two-input microbatching is
part of the qualified contract: a 64-input cancellation probe reached about
1.60 GiB and was rejected. Deterministic MMR remains optional because it did
not improve this corpus.

[CLI CI #31917686424](https://github.com/A3S-Lab/CLI/actions/runs/31917686424)
passes real offline smoke-model admission, inference, cancellation, recovery,
and RSS checks on Linux x64/ARM64, Windows x64, and macOS ARM64. Artifact
substitution/shape failures, simulated missing x86-64-v3, and a 32-waiter
cancellation storm are separate adversarial gates. Intel macOS remains
model-free/remote because the pinned ONNX Runtime does not ship that target.

## Reproduction

From the A3S Code repository on PowerShell:

```powershell
$env:A3S_CONFIG_FILE = (Resolve-Path '..\..\.a3s\config.acl').Path
cargo test --offline --locked -p a3s-code-core `
  --test test_workspace_retrieval_real_llm -- `
  --ignored --nocapture --test-threads=1
```

The semantic ablation prints one `WSR_DEEPSEEK_EVAL=<json>` record. The paired
rerank slice can be run independently with:

```powershell
cargo test --offline --locked -p a3s-code-core `
  --test test_workspace_retrieval_real_llm `
  real_deepseek_deterministic_rerank_defeats_duplicate_channel_collisions -- `
  --ignored --exact --nocapture --test-threads=1
```

It prints `WSR_DEEPSEEK_RERANK_SUMMARY=<json>` before enforcing gates and a
full `WSR_DEEPSEEK_RERANK_EVAL=<json>` record on success. Both tests are
ignored by default because they require repository credentials and network
access.

The orthogonal built-in chunking matrix and Rust custom negative control can be
run independently with:

```powershell
cargo test --offline --locked -p a3s-code-core `
  --test test_workspace_retrieval_real_llm `
  strategy_matrix::real_deepseek_chunking_strategy_matrix_qualifies_builtins_and_audits_custom_control -- `
  --ignored --exact --nocapture --test-threads=1
```

It prints `WSR_DEEPSEEK_CHUNKING_SUMMARY=<json>` and, after all gates pass, the
full `WSR_DEEPSEEK_CHUNKING_EVAL=<json>` record. The non-network fixture gate is
`strategy_matrix::locked_strategy_fixture_has_stable_chunks_and_complete_answer_tokens`.

The real ACL-host variant is owned by the A3S CLI repository and can be run
from that checkout with the monorepo root containing `.a3s/config.acl`:

```powershell
$env:A3S_REAL_EVAL_ROOT = (Resolve-Path 'C:\path\to\a3s').Path
cargo test --offline --locked `
  --test workspace_retrieval_real_deepseek `
  real_deepseek_acl_host_executes_recursive_reranked_workspace_tasks -- `
  --ignored --exact --nocapture --test-threads=1
```

It prints one `WSR_DEEPSEEK_ACL_HOST_EVAL=<json>` record after all gates pass.
For the in-process profile, set `A3S_LOCAL_CPU_MODEL_MANIFEST` to the already
installed `model.acl` and add `--features local-cpu-embedding` to that CLI test
command. The provider-only qualification command and complete immutable
artifact schema are documented in the CLI
[local CPU guide](https://github.com/A3S-Lab/CLI/blob/main/docs/local-cpu-workspace-embedding.md).

The Node.js, Python, and Go runners and their hermetic fixture commands are
listed in [`sdk/evaluation/README.md`](../sdk/evaluation/README.md). Each live
runner prints one normalized `WSR_SDK_DEEPSEEK_EVAL=<json>` record.

`a3s-test capabilities --json` succeeded during the review. The optional Web
driver schema probe reported `test.driver.web.capability_unavailable` because a
browser command was not installed, so this report claims Core/tool-loop E2E
coverage, public SDK parity, and no browser screenshot evidence.
