# A3S Code 9.1.3 test contract

The candidate remains on hold. This contract defines the required test lanes;
an inventory, a successful compile, or an ignored test does not establish a
passing execution. Retain first failures, exact SOLO reproductions, source
identities, artifact identities and terminal results in the delivery evidence.

## Required boundaries

| Boundary | Required observations | Existing executable surface |
| --- | --- | --- |
| Configuration | ACL parsing, unknown values, layer trust, bounded reads, credential environment and temporary-file cleanup | Core config tests, ACP config tests, `acl_env_config`, release-script tests |
| Kernel and lifecycle | Original Session/Run identity, admission exclusion, cancellation, cleanup acknowledgement and retained unresolved ownership | Workspace libraries; `agent_exact_run_host`, `run_control_runtime`, checkpoint recovery families |
| Durable facts | Canonical records, sequence/key integrity, original file/directory identity, no writes on refused recovery | Effect package, Core session-folder tests, ACP resume-admission tests, `test_fact_log_real_llm` |
| Tools and permissions | Read/write contracts, no partial mixed-batch disclosure, prepared/result receipts and verification-bound completion | Core tool suites, `test_scoped_batch_read_real_llm`, `test_tool_effect_receipts_real_llm` |
| MCP notifications | Host progress and automatic cache refresh both arrive while the original tools/call is outstanding; bounded pre-subscription buffering, FIFO backpressure and exact client close/drop remain observable | MCP unit/manager families, `test_mcp_notification_delivery` for both HTTP transports, exact progress case in `test_issue_fix_live_e2e` |
| Deterministic surface projection | Object insertion order and feature unification cannot change selected fields or generated control identities; field selection precedes the cap and record arrays keep their order | `surface_decision::` family under default and combined features |
| Protocol Cancel receipt | Request acceptance produces no premature terminal receipt; the actual original worker publishes terminal state; observation deadline returns typed pending while the original owner remains occupied | `op_coverage::opcov_protocol_cancel`, canonical-worker `protocol_cancel_tests` |
| Failed admission | Preserve the original typed capability error, retain unproved cleanup/ownership, perform no tool execution and refuse subsequent admission | `exact_run_admission_failure_preserves_typed_cause_and_retains_unproved_owner` |
| Checkpoint and memory ordering | Hold the actual checkpoint sender, require nonterminal state/no extraction, then join the original committer before End; host EOF and durable model-authorized extraction retain their separate boundaries | Dual-store `memory_retention_tests` family |
| Context | Project guidance retained, untrusted source remains data, complete and streaming paths, bounded tool history | `test_context_selection_real_llm`, compaction and tool-history suites |
| Memory | Explicit V1 pruning, scheduled V2 refresh, failure recovery, repository/vector history continuity, close and restart | Maintenance, semantic-refresh and durable-memory integration families |
| Retrieval | Exact/lexical/semantic fusion, stale source refusal, positive similarity, provider ceilings, index generations and bounded reranking | Core retrieval tests; all three SDK retrieval fixtures; retrieval soak |
| Protocol | Stable events, evaluation/research schemas, host operation coverage and SDK parity | Protocol generators/checkers; SDK API alignment; protocol integration tests |
| Node | Native callbacks, Promise failure/timeout, actual Run admission, original-handle cancellation and evidence scope | `npm test`, `npm run test:types`, `test_release_real_llm.mjs` |
| Python | ABI/import, async loop progress, sampling failure/timeout, Run handle and evidence scope | Full pytest collection, standalone close script, `tests/release_real_llm.py` |
| Go | Native bridge protocol, callback panic containment, races, Run handle and evidence scope | `go test -race -tags=release_contract ./...`; opt-in `release_real_llm` tag |
| ACP | Wire dialogue, permissions, process restart, model/effort switching and session recovery | ACP library/integration tests; `live_interactive_dialogue_real_llm` |
| TUI | Thin scaffold and actual shipped pager; non-terminal startup has no initialization side effects | TUI/vendor library tests; `scripts/check_shipped_tui.py` against the actual pager artifact |
| Advanced profiles | Evaluation, research, workflows, durable SQLite, S3, telemetry and typed decisions compile and execute their declared suites | `ci-all`, `deep-live`, hermetic integrations and profile build checks |
| Delivery | Exact versions, independent dependency resolution, formatting, strict lints, documentation, platform packages and release chain | Release preflight and the referenced hosted workflows |

## Execution requirements

Use Rust 1.97.1 from `apps/code/`. The default workspace and feature-enabled
workspace are distinct lanes. Native Node and Python crates require their own
builds; rebuilding the Rust workspace does not refresh their installed native
modules. Record the Go bridge selected by absolute path. Cross-platform and
standalone dependency qualification remain required release gates.

```sh
rustup run 1.97.1 cargo test --locked --workspace
rustup run 1.97.1 cargo test --locked --workspace --features ci-all --lib
rustup run 1.97.1 cargo clippy --locked --workspace --all-targets --features ci-all,deep-live -- -D warnings
```

The full real-provider inventory remains `scripts/layer_c_suites.acl` and its
existing `layer_c_live_e2e.py` runner. Do not shrink that inventory to make a
selected local campaign pass. Declare any selected subset, omitted platform
lane and external prerequisite separately. Real-provider runs must explicitly
select ignored tests and prove a nonzero executed count with zero remaining
ignored tests. Never treat retry success as first-pass success.

The new SDK live gate uses each SDK's actual native path and real configured
provider. Each lane requires two distinct completed original Runs, cross-turn
recall of an exact code, exactly one terminal event per Run, no tool execution,
old-handle cancellation refusal, identity-bound evaluation evidence and foreign
evidence refusal. It prints public model identity and Run receipts, never ACL
contents or credentials. Missing configuration or bridge refuses execution.

```sh
rustup run 1.97.1 cargo build --locked -p a3s-code-go-bridge --features advanced-harness,server
export A3S_CONFIG_FILE=/absolute/path/config.acl
export A3S_CODE_PYTHON=/absolute/path/to/current-sdk-venv/bin/python
export A3S_CODE_GO_BRIDGE_TEST_BINARY=/absolute/path/to/current/a3s-code-go-bridge
bash scripts/sdk_release_real_llm.sh
```

The wrapper reuses `with_acl_env.sh` and the existing `a3s-acl` preparation
utility. All three SDK legs execute even if one fails. The exit status is
nonzero if any leg fails. This is an additive live gate; existing SDK live,
retrieval, callback and hermetic coverage still applies.

Each SDK's hermetic release contract also holds an actual local HTTP response
open after its first text delta. Cancel that original Run, require exactly one
Error and zero successful End events, verify the Cancelled snapshot, and
complete a distinct next Run while the old handle refuses cancellation.
The same local peers also hold a blocking response open. Cancel the exact
active Run, require the actual SDK error code `RUN_CANCELLED` and original Run
ID, then complete another distinct Run. Check the exported Node/Python error
code declarations and Go `ErrRunCancelled` against this native error.
The Core counterpart additionally blocks the original charge callback and
proves that admission remains busy until that callback and actual model child
join. It checks all five usage fields through both session stores and file
reopen; a cancellation request alone is never a release certificate.

When a suite hangs, retain its unfinished test name and terminate only the
owned test process under a recorded deadline. Re-run that case alone under a
deadline and continue the remaining inventory. A killed process is a failed
observation. Tests with no executed cases, early process exits or incomplete
summaries receive no passing credit.

Cancelled host delivery must preserve its bounded queued prefix and permit the
same actual original worker to retire. Keep normal backpressure and order, and
accept a ready send after cancellation. Pin exact retained interactive results
before live projection. The actual confirmation-close case must join while its
unconsumed FIFO is still full, retain the rejection for the same original Run,
observe no tool starts in either view and no next provider response, and require
zero active admission. Run EOF and terminal status cannot substitute for the
actual original guardian's completion.

## Acceptance

The v9.1.2 downstream exhaustive matches for `CodeError` and
`AgentProtocolHostError` must compile without a wildcard, and their `code()`
methods must still compile in const expressions. Pin feature-gated variants
under the same actual feature graph. The additive runtime classifiers must
distinguish a typed original cancellation from an identical diagnostic string,
retain the original Run ID, and preserve all existing error categories. Rebuild
all three native SDKs and require their actual blocking `RUN_CANCELLED` errors.

Every required lane must have an explicit terminal disposition. Failures must
be repaired without weakening fail-closed authority, receipts or architecture
ratchets. Host diagnostic files belong outside the candidate workspace; fixture
vectors must express the declared chunking and similarity contract; scheduled
single-job fixtures must explicitly disable unrelated default pruning. The
synchronous startup contract degrades optional maintenance to disabled, while
invalid asynchronous activation still refuses.

Canonical recovery fixtures must reserve and bind through the actual original
worker. Before provider entry, require exact capability/cognitive/Run/checkpoint
identity and the actual inbox. A real seed I/O fault must await scope close and
committer retirement before reuse. A failed actual child-scope close retains
original admission and refuses another Run. Repeating one exact source checkpoint
across distinct targets must reuse its immutable negative input; changed source
content must create a distinct negative identity. Retain the four original
failed fixture observations and the current full regression separately.

No statement of complete testing or release readiness is valid while required
tests fail, hang, remain unexecuted, or refer to different source/artifact
generations. Preserve these limits in the evidence's exact
`## Explicitly not claimed` section.
