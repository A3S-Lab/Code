# Cross-SDK Workspace Retrieval Evaluation

This directory owns the versioned, language-neutral fixture used to qualify
real-model Workspace Retrieval through the Node.js, Python, and Go public SDKs.
The fixture is the source of truth for corpus bytes, task labels, embedding
axes, chunking, reranking, and report versions. Language adapters must not
silently fork those values.

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
