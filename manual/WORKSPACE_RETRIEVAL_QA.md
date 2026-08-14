# Workspace Retrieval Release Qualification

Status: Passed and delivered on 2026-08-14. A3S CLI `main` commit `53821c8`
pins the qualified Code and Memory revisions, and the post-pin release build
and DeepSeek CLI integration rerun passed.

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
- The real DeepSeek run is a transport and tool-loop compatibility check only.
  Deterministic fixtures, not DeepSeek responses, are the relevance oracle.

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

It emits machine-readable JSON and exits non-zero when either latency or
resource gate fails. The final executable was 13,825,536 bytes. Compared with
the earlier 13,693,952-byte size-optimized candidate, the observed upper-bound
increase was 131,584 bytes (about 0.96 percent); intervening egress hardening
means this is not a controlled attribution to the profile override alone.

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
lifecycle parity. After the hard-link and same-open-handle defenses, the
complete Core library suite passed serially with 2,746 passed, 0 failed, and
18 ignored in 161.17 seconds. Strict all-target Clippy, formatting, the focused
security suites, and both release benchmarks pass.

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

## A3S Test coverage boundary

`a3s-test capabilities --json` and `a3s-test agent schema` were run from the
checked-out A3S Test implementation. The Web driver reported
`test.driver.web.capability_unavailable` because the browser command is not
installed in this environment. Therefore no browser screenshot is claimed.
Code Web host injection and retrieval-status behavior are covered by
deterministic contract tests, while the real DeepSeek E2E is a CLI workflow.

## Release disposition

Pass. Keep retrieval opt-in and source egress double-gated. The deterministic
quality, adversarial, lifecycle, confidentiality, performance, host, release
build, and real DeepSeek integration gates passed; `WSR-QA` and `WSR-DOC` are
delivered.
