# Workspace Retrieval Release Qualification

Status: Passed and delivered on 2026-08-14. A3S CLI `main` commit `53821c8`
pins the qualified Code and Memory revisions, and the post-pin release build
and DeepSeek CLI integration rerun passed. A later paired Core evaluation also
passed with real DeepSeek chat, explicit retrieval enable/disable control,
multi-chunk source, and non-text exclusion. The real ACL-host strategy/rerank
composition passed on 2026-08-15 and its final batching rerun is pinned by CLI
`f435950` and Code `bdb86e17`.
The subsequent `CODE-B2` qualification passed the complete serial Core suite,
strict Core and SDK bridge Clippy, the 25,000-record release gate, and a
schema-v2 paired DeepSeek rerun with 1.0x document-request amplification.
Code `cde887b` then completed the public Node.js, Python, and Go real-DeepSeek
matrix from one versioned fixture and schema-v1 report contract.

This report qualifies the first session-bound Workspace Retrieval (`WSR`)
release across A3S Memory, A3S Code, and the A3S CLI host. The release uses an
exact, memory-resident vector index and does not require a vector database or
persist vectors in session snapshots.

## Claims and non-claims

The release makes four bounded claims:

1. An enabled Code session builds lexical and semantic projections
   asynchronously without delaying session construction for a corpus read or
   embedding call.
2. Vector state, catalog state, provider identity, cancellation, and status are
   owned by one session and released when that session closes.
3. A remote Embedding Provider receives only source admitted by the host and
   Code egress policies, and a result is rendered only after authoritative
   current-source verification.
4. Exact and hybrid local retrieval meet the locked quality, latency, and
   resource gates on the reference profile.

The release does not claim that embeddings are authoritative, that a real LLM
is a ranking oracle, that vector state survives a session, or that semantic
retrieval replaces exact search, BM25, glob, or Code Intelligence. It also
does not qualify the optional SQLite `sqlite-vec` extension, which is not used
by WSR.

## First-principles adversarial plan

The review begins with assets and invariants, then chooses an attack, an
independent observable, and a release gate. A test is not accepted merely
because it exercises the implementation path it is meant to verify.

| Asset | Required invariant | Adversarial action | Independent observable | Gate |
| --- | --- | --- | --- | --- |
| Workspace credentials | Excluded content never reaches the Embedding Provider | Secret/control/generated/binary/oversized paths, a safe-looking hard-link alias, and a post-admission hard-link swap | Recording provider input IDs and text are compared with the successfully admitted catalog and sentinel set | Zero excluded or aliased bytes leave Code |
| Workspace root | Reads cannot escape the admitted root or change identity between scan and read | Symlink, rename, replacement, and hard-link races | Source-egress catalog reader resolves and revalidates every read; current digest and byte range fence rendering | Zero out-of-root or stale snippets |
| Ranking correctness | Exact vector ordering is deterministic and hybrid improves vocabulary mismatch without displacing identifiers | Random vectors, ties, updates, deletions, filters, paraphrases, duplicate names, CJK, and boilerplate | Brute-force `f64` cosine oracle plus independently labeled fixture | Exact parity; hybrid Recall@10/MRR gates; identifier first rank retained |
| Provider boundary | A malformed or hostile provider cannot publish a corrupt generation | Descriptor drift, duplicate/missing/unknown IDs, wrong dimensions, non-finite or non-normalized vectors, panic, timeout, 429/5xx, and oversized HTTP bodies | Typed executor/provider errors, immutable prior revision, lexical availability, and redacted diagnostics | No partial publication or sensitive error content |
| Session isolation | One session cannot observe or control another | Concurrent sessions over the same and different roots; cancel one during build/query | Separate result/status/revision streams, cancellation tokens, and weak index references | No cross-session hit, status mutation, cancellation, or retained allocation |
| Snapshot confidentiality | Ephemeral source, vectors, and provider identity are not durable state | Persist a live retrieval session and scan serialized snapshot | Sentinel scan of `SessionSnapshotV1` | Zero source/vector/provider sentinel matches |
| Change consistency | A query observes one immutable generation and only current source | Query during update/delete, superseded embedding, and lag recovery | Catalog/vector revision checks plus authoritative file digest and exact UTF-8 byte range | Old or mixed-generation evidence is discarded |
| Resource bounds | Work cannot grow without a checked ceiling | Record/byte/file/chunk/batch/queue overflow and repeated lifecycle churn | Index accounting, explicit degraded coverage, provider call counts, and weak-reference cleanup | Prior revision remains usable; bounded memory/tasks |
| Host trust | Repository ACL cannot silently authorize source egress or reroute credentials | Untrusted overlay enables retrieval or changes provider endpoint; redirects and sensitive-header probes | Trusted-layer resolution and recording HTTP endpoints | Double opt-in required; no redirect credential forwarding |

The hard-link cases matter because path-only policy is insufficient: a path
named `src/apparently-safe.rs` can reference the same file identity as `.env`.
The final design applies two defenses. Manifest admission rejects sensitive,
control, generated, binary, and oversized paths without opening every file or
blocking session construction. Retrieval-enabled local sessions then use a
dedicated source-egress catalog reader, which rechecks the logical and resolved
paths and rejects every multi-link file using metadata from the same open file
handle that supplies the bytes. Ordinary workspace tools retain their existing
host-governed access path. This closes the scan/read time-of-check to
time-of-use window without a second synchronous workspace scan; on Windows it
also avoids reopening a mutable path to obtain link-count evidence.

## Independent oracles

- A3S Memory exact-search tests calculate expected top-k results with a
  separately implemented brute-force `f64` oracle across randomized corpora,
  mutations, filters, and ties.
- The nine-query Code fixture stores relevance labels separately from returned
  paths. Its deterministic fake embeddings encode only judged relationships;
  they do not infer relevance from the implementation's output.
- Egress tests use a recording provider and sentinel content. They compare
  provider inputs with the successfully cataloged set, including hard-link
  identity attacks.
- Search-result tests reread the authoritative workspace through
  `WorkspaceServices` and verify the complete content digest and exact UTF-8
  chunk range.
- Lifecycle tests retain weak references and counters outside the session, then
  require them to reach zero after bounded close.
- Real DeepSeek runs measure transport, schema/tool-loop behavior, and exact
  task completion. Independent labels and deterministic embeddings, not
  DeepSeek responses, remain the ranking oracle.

## Quality results

The locked fixture contains nine queries spanning exact terms, identifiers,
CJK, and paraphrases.

| Metric | Native BM25 | Hybrid | Gate |
| --- | ---: | ---: | --- |
| Recall@10 | 0.6667 | 1.0000 | Hybrid at least 0.85 and at least +0.15 |
| Mean reciprocal rank | 0.6667 | 1.0000 | Improve without identifier regression |
| Identifier first-rank protection | Pass | Pass | No semantic displacement |
| Exact/CJK category recall | 1.0000 | 1.0000 | No regression |
| Paraphrase category recall | 0.0000 | 1.0000 | Improve vocabulary mismatch |

The gate is implemented by
`locked_hybrid_fixture_meets_quality_and_identifier_gates`; it exercises the
real catalog, native BM25, semantic partition, RRF, diversity, and current-file
verification paths.

## Performance results

Reference profile: Intel64 Family 6 Model 143, 20 logical CPUs, Windows
x86_64, release build, 25,000 normalized 384-dimensional records, top-20, 20
warmup queries, and 100 measured queries. Provider network latency is excluded.
The hybrid measurement includes authoritative source reads from the warm OS
cache. A3S Memory is compiled with package-local `opt-level = 3`; the rest of
the size-oriented release profile is unchanged.

| Run | Exact p50 | Exact p95 | Exact max | Hybrid p50 | Hybrid p95 | Hybrid max |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| A | 7.173 ms | 8.294 ms | 12.219 ms | 39.064 ms | 51.145 ms | 55.071 ms |
| B | 7.960 ms | 12.302 ms | 16.842 ms | 41.955 ms | 54.429 ms | 60.158 ms |
| Gate | - | <= 30 ms | - | - | <= 100 ms | - |

Run A built the exact index in 67.066 ms and the workspace projection in
1,005.500 ms; Run B took 69.728 ms and 1,129.819 ms respectively. Synchronous
session construction was 11.614 ms and 13.289 ms and did not wait for the
workspace build. The fixture contains 4,000,000 source bytes. The full
retrieval runtime accounted for 41,397,932 bytes; the exact index accounted
for 39,900,117 bytes. It embedded 25,000 document inputs once and 120 query
inputs; repeated queries did not re-embed unchanged source. Close released the
ephemeral index.

The permanent benchmark is:

```bash
cargo run --release -p a3s-code-core --example workspace_retrieval_benchmark
```

It emits machine-readable JSON and exits non-zero when a latency, candidate,
scratch, fallback, or resource gate fails. The final executable was 13,825,536
bytes. Compared with
the earlier 13,693,952-byte size-optimized candidate, the observed upper-bound
increase was 131,584 bytes (about 0.96 percent); intervening egress hardening
means this is not a controlled attribution to the profile override alone.

### CODE-B2 cross-file batching qualification

The 2026-08-15 schema-v3 release run extends the same permanent benchmark with
hard batching gates. Each 25,000-record session reported 25,000 document inputs,
391 logical batches, a 391-request count/text/vector lower bound, 391 physical
provider requests, 1.0x amplification, one generation-complete tail flush,
zero non-text inputs, and time to first file-atomic publication of 9-10 ms.

RRF-only and deterministic hybrid p95 were 57.499 and 49.560 ms, workspace
builds were 1,201.453 and 1,104.608 ms, and synchronous session construction was
13.943 and 6.142 ms. Both sessions retained 41,397,932 vector bytes at full
coverage and released them completely on close. Exact-vector p95 was 16.333 ms.
All query latency, rerank scratch/fallback, memory, request-amplification, and
lifecycle gates passed.

The deterministic projection suite covers count, text-byte, vector-byte, and
generation-complete flushes; response-count corruption; provider retry
accounting; split-file atomicity; later-batch failure isolation; source-revision
cancellation; stable sibling chunk IDs; partial readiness; and bounded close.
The paired DeepSeek task, collision-rerank, four-strategy matrices, real CLI
ACL-host test, and three public SDK arms all pass with 1.0x request
amplification and zero non-text inputs.
The host additionally matches Core's one logical/physical/lower-bound batch
against an independent embedding-server request count for each 39-chunk session.

### CODE-R2 deterministic rerank qualification

The version-2 benchmark runs two separately constructed sessions over the same
25,000-record, 384-dimensional, top-20 reference corpus: compatibility
`rrf_k60` and opt-in `rrf_k60+deterministic_mmr_v1`. This keeps provider and
authoritative-read work in both measurements. The signed p95 difference is
reported without clipping; only positive added latency is compared with the
10 ms budget. Session construction and asynchronous corpus-build timings are
reported but are not attributed to the query-time reranker.

| Run | Exact p95 | RRF p95 | Deterministic p95 | Signed difference | Added-latency gate | Evaluated candidates | Feature bytes | Accounted scratch | Fallbacks |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| A | 11.273 ms | 57.574 ms | 52.410 ms | -5.163 ms | Pass (0 ms added) | 50 | 8,000 | 75,346 | 0 |
| B | 8.110 ms | 58.229 ms | 55.907 ms | -2.322 ms | Pass (0 ms added) | 50 | 8,000 | 75,346 | 0 |
| Gate | <= 30 ms | <= 100 ms | <= 100 ms | - | <= 10 ms positive addition | <= 100 | bounded by candidate sampling | <= 4 MiB | 0 |

The locked nine-query deterministic fixture independently records Recall@10,
MRR, and nDCG@10 of 1.0, zero selected near-duplicate evidence, and unchanged
identifier-first rank. Adversarial unit and session tests additionally cover
same-file interval containment, cross-file boilerplate, deterministic replay,
candidate-pool truncation, invalid pre-provider configuration, and scratch
budget fallback that reproduces the RRF result order.

Both paired query measurements include provider and authoritative-read noise;
their negative signed differences are evidence only that positive added p95
stayed within budget, not that deterministic reranking accelerates retrieval.

### Real DeepSeek adversarial rerank slice

An ignored, reproducible Core integration test loaded
`deepseek/deepseek-v4-pro` through the repository `.a3s/config.acl` and ran
three paired tasks against eight cross-channel collision files per task. The
embedding oracle remained deterministic and process-local. The RRF-only arm
had 0/3 exact completions, 0.0 Recall@5, 0.0 MRR, and 15/15 collision results.
The deterministic arm had 3/3 exact completions, 1.0 Recall@5, 0.3889 MRR, and
10/15 collision results, with expected paths at ranks 3, 2, and 3.

Both arms used 15,595 vector bytes and made no non-text provider calls. The
deterministic arm retained at most 12,239 feature bytes and accounted 18,346
scratch bytes with no truncation or fallback. All six sessions reached full
coverage and released all vectors after close. Document-request amplification
was 54x in that pre-`CODE-B2` run. The locked post-`CODE-B2` rerun preserves the
same quality result while reducing both arms to 1.0x.

Observed DeepSeek turn p95 was 209,318 ms for RRF and 28,102 ms for rerank,
with 19,932 and 18,276 total tokens respectively. These are three remote-model
samples per arm, including one 209-second RRF turn, and are reported without a
speedup claim. The paired release benchmark above remains the local latency
gate. The full JSON schema and reproduction command are in
`WORKSPACE_RETRIEVAL_DEEPSEEK_EVAL.md`.

The CODE-R2-focused Core, Node, Python, and Rust Go-bridge gates pass. One full
serial Core run completed 2,764 tests and ignored 18, with one failure in the
unchanged Code Intelligence language-server initialization-cancellation timing
test. Five isolated reruns produced three passes and two one-second timeout
failures, confirming an independent existing timing flake rather than a
retrieval-path failure; it remains visible and is not counted as a CODE-R2
pass.

These results qualify the local deterministic baseline and one adversarial
real-model slice, not a default change. `SDK-R2`, `HOST-R2`, and the complete
`WSR-EVAL2` execution matrix are now delivered, but the remote arms contain
only three tasks per variant. They demonstrate portable correctness rather
than a statistically and operationally meaningful default advantage, so
RRF-only remains the compatibility default.

## Deterministic test evidence

### A3S Memory

- Default suite: 105 tests passed.
- `sqlite` feature suite: 124 tests passed.
- Exact-vector benchmark: two release runs passed the 30 ms p95 budget and
  released all accounted bytes on `clear`.
- All-target/all-feature Clippy and rustdoc gates passed.
- The Windows `cargo test --all-features` failure at `no such module: vec0`
  reproduces on clean Memory `main`; WSR neither enables nor uses `sqlite-vec`.

A3S Memory `main` commit `3293f572` contains the exact-scan hot-path changes and
the permanent `vector_search_benchmark` example.

### A3S Code

Code `main` commit `cf41f09` contains the source-egress path policy,
same-open-handle hard-link enforcement, and catalog/tool access separation.

The dedicated release-QA module covers:

- `embedding_egress_contains_only_path_and_identity_admitted_source`;
- `hard_link_swap_after_admission_never_reaches_the_embedding_provider`;
- `sessions_do_not_share_retrieval_results_status_or_cancellation`;
- `persisted_session_snapshot_excludes_source_vectors_and_provider_identity`;
- `repeated_session_lifecycle_releases_every_ephemeral_index`.

Additional deterministic suites cover source-egress hard-link rejection,
path eligibility, symlink and resolved-path confinement, catalog
reconciliation, semantic generation fencing, source verification, hybrid
quality, provider validation/retry/cancellation/panic behavior, and SDK/bridge
lifecycle parity. The later typed chunk-strategy suite adds fixed, recursive,
custom, overlap-accounting, invalid-range, panic, Unicode, ownership, and async
session cases. The complete Core library suite now passes serially with 2,757
passed, 0 failed, and 18 ignored in 167.33 seconds. Strict all-target Clippy,
formatting, the focused security suites, and both release benchmarks pass.

The three `agent_release_manifest` integration failures also reproduce on the
clean pre-WSR Code baseline and are tracked as unrelated release-contract
fixtures. High-parallelism Windows runs additionally exhibit scheduling
timeouts; the serial library result is the deterministic gate.

### A3S CLI host

The host qualification covers default-off ACL parsing, the separate
OpenAI-compatible embedding route, trusted-layer egress authorization,
endpoint validation, redirect rejection, sensitive-header isolation,
duplicate-index rejection, cancellation, rate-limit mapping, oversized
responses, error-body redaction, exec/TUI session injection, and Code Web
session propagation/status. Retrieval-focused tests passed 71/71; all targets
and strict Clippy compiled. The complete Windows suite had the same 23
pre-existing failures and 10 ignored tests as clean CLI baseline `f4377c2`,
with no WSR regression.

A3S CLI `main` commit `53821c8` pins Code `47770057` and Memory `3293f572`,
applies the Memory package-local release optimization, and passed the final
post-pin retrieval filter (19/19), formatting, all-target Clippy, and release
build gates.

Follow-up CLI `main` commit `d1c8c25` pins Code `b7a496b` and closes the
host-catalog ownership gap exposed by the real ACL-host test: exec, TUI, and
Code Web now configure the shared manifest catalog exactly once and keep
catalog options out of the session-owned semantic runtime. Retrieval tests
pass 28/28, exec policy 7/7, ACL authority 5/5, Web host/cache 5/5, config
projection 2/2, locked all-target check, format, and baseline-aware changed-
target Clippy.

CLI `main` commit `f435950` subsequently pins Code `bdb86e17`, upgrades the
host report to schema v2, and passes the 29-test retrieval filter plus the real
DeepSeek rerun with matching Core and independent provider counters.

## Real DeepSeek integration

The repository `.a3s/config.acl` was validated and inspected without printing
provider URLs, headers, environment-variable names, or secret values. It
contained one provider, two models, default model
`deepseek/deepseek-v4-pro`, retrieval disabled, and no source-egress grant.

The final release `a3s 0.11.1` build then ran in JSONL/read-only mode against
an isolated workspace. DeepSeek selected and completed exactly one governed
`Read` tool call, executed no other tool, and returned the exact marker
`WSR_FINAL_CODE_47770057_MEMORY_3293F572`. The run produced 79 events, one
successful result, no invalid JSONL or stderr, and reported retrieval as
disabled. This passes real chat, streaming/event, tool-selection, tool-result,
and final-response compatibility without authorizing source embedding egress.

DeepSeek was deliberately not used as an embedding correctness oracle. The
configured chat route and a separately admitted embeddings route have
different trust and protocol boundaries.

### Enabled/disabled task ablation

A second ignored Core integration used the same ACL-selected
`deepseek/deepseek-v4-pro` chat model in six fresh isolated sessions. The test
injected a process-local deterministic embedding oracle through Code's public
provider contract because the ACL did not configure or authorize a remote
embedding route. Each of three tasks ran once with retrieval explicitly
cleared and once enabled.

| Metric | Enabled | Disabled |
| --- | ---: | ---: |
| Exact task completion | 3/3 | 0/3 |
| Tool protocol compliance | 3/3 | 3/3 |
| Expected-path Recall@5 | 1.0000 | 0.0000 |
| Expected-path MRR | 1.0000 | 0.0000 |

The 33-entry fixture contained 30 text files, three non-text assets, and 31
chunks. One answer appeared only after the default 80-line chunk boundary. All
enabled expected paths ranked first; non-text provider inputs were zero; all
vectors were released after close. The pre-`CODE-B2` run exposed 30 document
provider requests for 31 chunks. The final schema-v2 rerun sends one request
against the same one-request lower bound, for 1.0x amplification. Full
methodology, latency, resource, token, chunking, and follow-up gates are in the
[DeepSeek evaluation report](WORKSPACE_RETRIEVAL_DEEPSEEK_EVAL.md).

### Real ACL-host strategy and rerank execution

The production CLI then composed the repository DeepSeek route with a temporary
trusted retrieval ACL, explicit recursive 512/64 chunking, deterministic
reranking, and a loopback embedding oracle. All three exact tasks and all three
one-Search protocols passed. Precision@5 was 0.2, returned-result precision
0.4286, Recall@5 1.0, MRR 0.5, and nDCG@5 0.6309; every expected path ranked
second. Each fresh session reached 100 percent coverage with 30 text files, 39
chunks/vectors, 9,595 vector bytes, zero failed files, zero non-text provider
inputs, and 1.0x request amplification. Each 39-chunk session reports one
logical batch, one physical Core request, and a one-request lower bound; the
independent provider also observes one document request. Time to first
file-atomic publication was 8-10 ms. The test also proved that the temporary API
key and endpoint were absent from stdout and stderr.

### Public SDK real-model parity

Code `cde887b` adds one language-neutral fixture and normalized schema-v1
report to the Node.js, Python, and Go public SDKs. Each adapter materializes the
same digest
`3e9d739225fa8d320b2166ff4283604c72d940693c0ea9879f112abe77773565`,
injects the same deterministic eight-dimensional embedding oracle, selects
recursive 512/64 chunking and the typed deterministic reranker, and loads the
real `deepseek/deepseek-v4-pro` chat route from the repository
`.a3s/config.acl`. DeepSeek is evaluated for schema inspection, exact tool
selection, evidence use, and completion; it is not the ranking oracle.

| Quality metric | Node.js | Python | Go |
| --- | ---: | ---: | ---: |
| Exact task completion | 3/3 | 3/3 | 3/3 |
| Exact one-Search protocol | 3/3 | 3/3 | 3/3 |
| Precision@5 | 0.2000 | 0.2000 | 0.2000 |
| Precision among returned results | 0.4286 (3/7) | 0.4286 (3/7) | 0.4286 (3/7) |
| Expected-path Recall@5 | 1.0000 | 1.0000 | 1.0000 |
| Expected-path MRR | 0.5000 | 0.5000 | 0.5000 |
| Expected-path nDCG@5 | 0.6309 | 0.6309 | 0.6309 |
| Expected-path ranks | 2, 2, 2 | 2, 2, 2 | 2, 2, 2 |

Every fresh session indexed 30 text files into 39 chunks/vectors and accounted
9,595 vector bytes. It sent one document request for the one-request lower
bound plus one query request, admitted no non-text sentinel, reached full
coverage, and reported zero vector records and bytes after close.

| Observed metric, p50 / p95 | Node.js | Python | Go |
| --- | ---: | ---: | ---: |
| Session construction | 15 / 17 ms | 13 / 31 ms | 16 / 22 ms |
| Index ready | 712 / 859 ms | 724 / 819 ms | 592 / 1,598 ms |
| Time to first ready publication | 2 / 4 ms | 3 / 4 ms | 4 / 21 ms |
| DeepSeek turn | 4,347 / 24,820 ms | 3,418 / 6,588 ms | 25,000 / 25,052 ms |
| Session close | 5,008 / 5,013 ms | 5,012 / 5,017 ms | 5,012 / 5,015 ms |
| Total DeepSeek tokens, three tasks | 14,140 | 14,609 | 14,093 |

The first Node live attempt passed the exact Search-call contract but returned
the file stem `replay_fence` instead of the labeled declaration name. The run
was rejected. The shared prompt contract was then clarified across all three
adapters to require a Rust function or constant declaration and forbid paths,
file stems, module names, prose, and Markdown; no expected answer was inserted.
The complete three-SDK matrix above is the subsequent clean rerun.

The approximately five-second close values match the enabled and disabled
session baseline already measured in this environment. Vector records and
bytes are synchronously zero after each close, so the observation is not
attributed to retained retrieval state. Remote turn samples are diagnostic;
the release benchmark remains the latency gate. Reproduction commands and the
machine-readable field contract are in
[`sdk/evaluation/README.md`](../sdk/evaluation/README.md).

### Real embedding model boundary

Code `beac7cb` starts `WSR-PROD1` by replacing the deterministic embedding
oracle with revision-locked Sentence Transformers models through the public Python
callback. The English `all-MiniLM-L6-v2` negative control retrieves only the
two English targets: semantic and hybrid Recall@5 are 0.6667 and the CJK target
is absent. `paraphrase-multilingual-MiniLM-L12-v2` retrieves all targets at
semantic and RRF-hybrid ranks 2/2/2, Recall@5 1.0, MRR 0.5, and nDCG@5 0.6309.
It reaches ready in 985 ms and has 20 ms hybrid p95 on the cached local run.

The same multilingual vectors under deterministic reranking retain Recall@5
1.0 but move ranks to 5/2/3, lowering MRR to 0.3444 and nDCG@5 to 0.5059. All
arms retain 68,251 vector bytes, use 1.0x document-request amplification,
admit zero non-text inputs, and release every vector. This qualifies a real
multilingual model and RRF-only as production-evaluation candidates, not as
bundled defaults.

### Retrieval-dependent generation boundary

Code `eddeeea` adds a compile-gated three-task generation corpus and runs each
task three times with the locked multilingual embedding model and the real
repository-authorized `deepseek/deepseek-v4-pro` chat route. The model receives
no expected code. It must issue exactly one explicit hybrid Search, retrieve
both labeled evidence files, edit only the marker in `src/solution.rs`, and
return the exact completion marker. A hidden Rust test is injected only after
the session closes.

All 9/9 runs passed, for a 1.0000 pass rate and 0.7008 two-sided 95 percent
Wilson lower bound. Tool protocol, evidence Recall@5, hidden compile, workspace
integrity, and release rates were all 1.0000. The run made 18 document requests
at 1.0000x amplification, admitted zero non-text inputs, and advanced every
edited source/vector generation without accumulating a second live generation.
Initial/full-ready and edited-generation first-publication p95 were 402/919 and
40 ms respectively. The complete metrics, corpus digests, reproduction command,
and SLO thresholds are in [`sdk/evaluation/README.md`](../sdk/evaluation/README.md).

The bounded local churn gate then replaced one source file 64 times. Every
published generation retained exactly one vector record and returned only the
latest marker; close released all records and bytes. Code
[CI #249](https://github.com/A3S-Lab/Code/actions/runs/31862118069) passes the
same ignored test on Ubuntu, macOS, and Windows. Operational state handling and
configuration-only rollback are defined in the
[Workspace Retrieval Operations runbook](WORKSPACE_RETRIEVAL_OPERATIONS.md).

## A3S Test coverage boundary

`a3s-test capabilities --json` and `a3s-test agent schema` were run from the
checked-out A3S Test implementation. The Web driver reported
`test.driver.web.capability_unavailable` because the browser command is not
installed in this environment. Therefore no browser screenshot is claimed.
Code Web host injection and retrieval-status behavior are covered by
deterministic contract tests. The real DeepSeek evidence covers the CLI read
workflow, the Core Search tool loop, production CLI hybrid retrieval, and
Node.js/Python/Go public API parity with typed chunking/reranking; none requires
a browser.

## Release disposition

Pass for opt-in retrieval-dependent generation. Keep retrieval opt-in and source
egress double-gated. The deterministic quality, adversarial, lifecycle,
confidentiality, performance, host, compile-gated generation, three-platform
churn, and real DeepSeek integration gates passed. Keep line chunking and
RRF-only as compatible defaults because the real-model samples qualify a bounded
workflow, not a universal default advantage.
