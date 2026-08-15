# Cross-SDK Workspace Retrieval Evaluation

This directory owns the versioned, language-neutral fixtures used to qualify
real-model Workspace Retrieval through public SDKs and retrieval-dependent
generation. Each fixture is the source of truth for its corpus bytes, task
labels, embedding axes, chunking, reranking, and report version. Language
adapters must not silently fork those values.

## What the evaluation proves

The evaluation separates three independent concerns:

1. A deterministic process-local embedding oracle controls relevance labels.
   DeepSeek is not treated as an embedding or ranking oracle.
2. The SDK must configure recursive 512/64 text chunking and the typed
   deterministic reranker, wait for the asynchronous session-owned in-memory
   projection, and expose matching status and batching evidence.
3. The real `deepseek/deepseek-v4-pro` chat route loaded from the monorepo
   `.a3s/config.acl` must inspect the tool schema, issue exactly one governed
   hybrid Search call, and return the independently labeled identifier.

The corpus contains 30 admitted UTF-8 text files and three non-text sentinels.
It must produce 39 chunks and no non-text embedding input. Each task uses a
fresh session so index construction, provider counters, and release evidence
cannot leak between observations. Closing the session must report zero vector
records and bytes.

The fixture digest uses `sha256(path NUL content NUL)` over files sorted by
repository-relative path. Version 1 is locked to:

```text
3e9d739225fa8d320b2166ff4283604c72d940693c0ea9879f112abe77773565
```

## Hermetic fixture gates

These commands do not call DeepSeek:

```powershell
node .\sdk\node\test_workspace_retrieval_real_deepseek.mjs --validate-fixture

$env:PYTHONPATH = (Resolve-Path '.\sdk\python\python').Path
py -3.13 .\sdk\python\tests\test_workspace_retrieval_real_deepseek.py --validate-fixture

go -C .\sdk\go test -run '^TestWorkspaceRetrievalRealFixtureContract$' -count=1
```

The Node fixture gate is part of `npm test`. Python and Go collect the fixture
contract in their normal test suites; their real-provider tests skip unless
`A3S_REAL_EVAL_ROOT` is set.

## Real DeepSeek matrix

Build the native SDK artifact required by the language under test. For Go,
build the matching bridge from the same Code checkout first:

```powershell
cargo build --locked -p a3s-code-go-bridge --bin a3s-code-go-bridge
```

Then point the tests at a monorepo checkout whose `.a3s/config.acl` contains
the authorized DeepSeek route. The tests use the config in place and do not
copy or print provider URLs, headers, environment-variable names, or secret
values.

```powershell
$env:A3S_REAL_EVAL_ROOT = (Resolve-Path 'D:\code\a3s').Path

node .\sdk\node\test_workspace_retrieval_real_deepseek.mjs

$env:PYTHONPATH = (Resolve-Path '.\sdk\python\python').Path
py -3.13 .\sdk\python\tests\test_workspace_retrieval_real_deepseek.py

$env:A3S_CODE_GO_BRIDGE_TEST_BINARY = `
  (Resolve-Path '.\target\debug\a3s-code-go-bridge.exe').Path
go -C .\sdk\go test -run '^TestWorkspaceRetrievalRealDeepSeek$' -count=1 -v
```

Every successful language runner prints one
`WSR_SDK_DEEPSEEK_EVAL=<json>` record using `report_schema_version = 1`.
The normalized report includes:

- exact task completion and one-Search protocol rates;
- Precision@5, returned-result precision, Recall@5, MRR, and nDCG@5;
- session construction, index-ready, time-to-first-ready, turn, and close
  p50/p95 observations;
- result counts, expected-path ranks, algorithm and rerank modes;
- files, chunks, vector records/bytes, batching flushes, provider request
  amplification, token usage, and non-text egress;
- post-close vector release for every session.

The live gate requires 3/3 exact completion, 3/3 exact tool protocol,
Recall@5 of 1.0, at most 1.10x document-request amplification, zero non-text
provider inputs, and complete post-close vector release. Remote-model timing
from three tasks is diagnostic rather than a release latency claim. This
matrix qualifies SDK parity; it does not justify changing the compatible line
chunker or RRF-only defaults.

## Qualified result

The 2026-08-15 run at Code `cde887b` passed every gate:

| Quality metric | Node.js | Python | Go |
| --- | ---: | ---: | ---: |
| Exact task completion | 3/3 | 3/3 | 3/3 |
| Exact one-Search protocol | 3/3 | 3/3 | 3/3 |
| Precision@5 | 0.2000 | 0.2000 | 0.2000 |
| Precision among returned results | 0.4286 (3/7) | 0.4286 (3/7) | 0.4286 (3/7) |
| Recall@5 | 1.0000 | 1.0000 | 1.0000 |
| MRR | 0.5000 | 0.5000 | 0.5000 |
| nDCG@5 | 0.6309 | 0.6309 | 0.6309 |
| Expected-path ranks | 2, 2, 2 | 2, 2, 2 | 2, 2, 2 |

Every session reached 30/30 indexed text files, 39 chunks/vectors, 9,595
accounted vector bytes, one document request against a one-request lower bound,
zero non-text provider inputs, and zero vectors after close.

| Observed metric, p50 / p95 | Node.js | Python | Go |
| --- | ---: | ---: | ---: |
| Session construction | 15 / 17 ms | 13 / 31 ms | 16 / 22 ms |
| Index ready | 712 / 859 ms | 724 / 819 ms | 592 / 1,598 ms |
| Time to first ready publication | 2 / 4 ms | 3 / 4 ms | 4 / 21 ms |
| DeepSeek turn | 4,347 / 24,820 ms | 3,418 / 6,588 ms | 25,000 / 25,052 ms |
| Session close | 5,008 / 5,013 ms | 5,012 / 5,017 ms | 5,012 / 5,015 ms |
| Total DeepSeek tokens, three tasks | 14,140 | 14,609 | 14,093 |

The first Node live attempt passed the Search-call contract but returned the
file stem `replay_fence` rather than the required declaration name, so it was
rejected. The common task prompt was clarified in every adapter to require a
Rust function or constant declaration and to forbid paths, file stems, module
names, prose, and Markdown. No expected answer was added. The table records the
subsequent complete rerun.

The close observations match the independently measured enabled and disabled
session baseline in this environment. Retrieval status reported zero vector
records and bytes after each close, so the approximately five seconds are not
attributed to retained vector state.

## Real embedding model matrix

The DeepSeek matrix deliberately uses a deterministic ranking oracle. The
separate Python runner evaluates real embedding model behavior through the
public callback provider without making Sentence Transformers a package or
Core dependency. Model revisions, expected outcomes, and comparison direction
are locked in `workspace-retrieval-embedding-models-v1.json`.

Install the optional evaluation dependency, fetch the exact locked revisions
once, then repeat offline from the local model cache:

```powershell
py -3.13 -m pip install sentence-transformers
$env:PYTHONPATH = (Resolve-Path '.\sdk\python\python').Path

py -3.13 .\sdk\python\tests\test_workspace_retrieval_real_embedding.py `
  --matrix
py -3.13 .\sdk\python\tests\test_workspace_retrieval_real_embedding.py `
  --matrix --local-files-only
```

The runner prints `WSR_REAL_EMBEDDING_MATRIX=<json>`. It requires a locked
revision, full 30-file/39-chunk coverage, semantic and hybrid Recall@5 of 1.0
for positive cases, index readiness within 5 seconds, hybrid p95 within 1
second, at most 1.10x document-request amplification, zero non-text egress,
and complete release. The English-only model is an intentional negative
control and must fail only the CJK task.

The locked offline run at Code `beac7cb` on 2026-08-15 produced:

| Case | Semantic ranks | Hybrid ranks | Hybrid Recall@5 / MRR / nDCG@5 | Ready / hybrid p95 | Result |
| --- | --- | --- | --- | ---: | --- |
| `all-MiniLM-L6-v2` RRF negative control | 2, -, 2 | 2, -, 2 | 0.6667 / 0.3333 / 0.4206 | 850 / 15 ms | Rejected: CJK absent |
| Multilingual MiniLM RRF | 2, 2, 2 | 2, 2, 2 | 1.0000 / 0.5000 / 0.6309 | 985 / 20 ms | Qualified candidate |
| Multilingual MiniLM deterministic audit | 2, 2, 2 | 5, 2, 3 | 1.0000 / 0.3444 / 0.5059 | 799 / 24 ms | Qualified, but worse ranking |

All three cases used one document request for the one-request lower bound,
admitted zero non-text inputs, accounted 68,251 vector bytes, and released all
vectors on close. The multilingual model is
[`paraphrase-multilingual-MiniLM-L12-v2`](https://huggingface.co/sentence-transformers/paraphrase-multilingual-MiniLM-L12-v2)
at revision `e8f8c211226b894fcb81acc59f3b34ba3efd5f42`. This narrow fixture
qualifies it as a production-evaluation candidate, not a bundled default. It
also supplies real-model evidence for retaining RRF-only as the compatible
default; deterministic reranking remains an explicit corpus-dependent option.

## Retrieval-dependent generation matrix

`workspace-retrieval-generation-v1.json` advances the qualification from
answer extraction to repository mutation. It contains three independent Rust
tasks covering reconnect admission, CJK lifecycle policy, and embedding
backpressure at a chunk boundary. Each task includes two required evidence
files, a lexical trap, 18 unrelated Rust files, and three non-text sentinels.
The target initially contains only a typed signature and an implementation
marker; expected code is never included in the prompt or searchable corpus.

The real model must make exactly one explicit Top-5 hybrid Search call, use all
labeled evidence, make exactly one marker-scoped edit to `src/solution.rs`, and
touch no other model-visible file. After the session closes, the runner injects
an independent hidden Rust test and runs `cargo test --offline --quiet`.
Success therefore requires tool-protocol compliance, evidence coverage,
target-only integrity, hidden compilation, incremental reindex publication,
bounded provider amplification, zero non-text egress, and complete release; a
plausible model response alone cannot pass.

Validate the corpus contract without a model, then run the complete three-by-
three matrix from a checkout with the Python native SDK already built and the
locked Sentence Transformers revision cached:

```powershell
$env:PYTHONPATH = (Resolve-Path '.\sdk\python\python').Path
py -3.13 .\sdk\python\tests\test_workspace_retrieval_generation_real_deepseek.py `
  --validate-fixture

$env:A3S_REAL_EVAL_ROOT = (Resolve-Path 'D:\code\a3s').Path
py -3.13 .\sdk\python\tests\test_workspace_retrieval_generation_real_deepseek.py `
  --local-files-only
```

The runner prints `WSR_GENERATION_EVAL=<json>`. A full qualification requires
at least three repetitions of every task, pass rate at least 0.90, a two-sided
95 percent Wilson lower bound of at least 0.65, and 100 percent protocol,
evidence, compile, integrity, and release rates. It also locks p95 ceilings of
100 ms for session construction, 5,000 ms for full readiness, 1,000 ms for
initial publication, 2,000 ms for observing the edited generation, 1,000 ms
for its first publication, and 6,000 ms for close. Document-request
amplification must remain at most 1.10x and non-text input must remain zero.

The clean 2026-08-15 run represented by Code `eddeeea` passed all nine trials:

| Metric | Observed |
| --- | ---: |
| Hidden-test generation success | 9/9 (1.0000) |
| 95% Wilson lower bound | 0.7008 |
| Per-task success | 3/3, 3/3, 3/3 |
| Exact tool protocol / evidence Recall@5 | 1.0000 / 1.0000 |
| Hidden compile / workspace integrity / release | 1.0000 / 1.0000 / 1.0000 |
| Document requests / amplification | 18 / 1.0000x |
| Non-text provider inputs | 0 |
| Session construction p50 / p95 | 9 / 21 ms |
| Full index ready p50 / p95 | 773 / 919 ms |
| Initial first-ready publication p50 / p95 | 224 / 402 ms |
| Edited-generation observation p50 / p95 | 0 / 1,054 ms |
| Edited-generation first publication p50 / p95 | 36 / 40 ms |
| DeepSeek turn p50 / p95 | 7,701 / 30,660 ms |
| Hidden Cargo test p50 / p95 | 5,487 / 7,139 ms |
| Session close p50 / p95 | 5,009 / 5,018 ms |
| Total DeepSeek tokens | 90,088 |

Every run advanced source revision 1 to 2 and vector revision 24 to 26 after
the edit without retaining an extra generation. DeepSeek turn and Cargo build
times are diagnostic and are not retrieval latency SLOs. Nine successful
observations qualify this locked opt-in workflow; they do not establish a
universal model success rate for arbitrary repositories or justify enabling
semantic retrieval by default.
