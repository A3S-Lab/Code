# 9.1.3 candidate release disposition

This is an unpublished candidate. The release decision is **hold** until the
remaining gates below pass against one committed source revision. Local
qualification and its exact limits are retained in
[the closeout outcome](../../../docs/evidence/code-harness/20261011-release-closeout/OUTCOME.md).

## Candidate scope

- Align Core, ACP, thin TUI, native SDKs, Python bootstrap and the actual
  shipped pager's version metadata to 9.1.3. The independent Kernel retains
  its own version.
- Preserve exact Run identity across Node, Python and Go admission, snapshots,
  cancellation and evidence reads. Policy updates apply at the next admission.
- Project cancellation before the original terminal seal as a typed original-Run error
  (`RUN_CANCELLED`) for blocking calls and an Error without a successful End
  for streams. Keep original admission occupied while an owed charge callback
  or actual model task is unresolved; join that task before retry or cleanup.
  Export `RUN_CANCELLED` through Node/Python error-code declarations and Go's
  `CodeRunCancelled`/`ErrRunCancelled` contract; verify actual blocking errors.
  Preserve the v9.1.2 exhaustive `CodeError` variants and const `code()` API.
  Rust callers use the additive `runtime_code()` and `cancelled_run_id()`
  queries for cancellation; the existing `Internal` carrier retains its
  `INTERNAL_ERROR` const classification. Error text does not classify failures.
- Complete seven value-typed SDK options. Unknown network modes, invalid
  budgets/contracts and foreign evidence targets refuse at the Core boundary.
- Keep MCP sampling explicitly host-owned, bounded by timeout and the Core
  per-server ceiling. Missing handlers and callback failures refuse. Go catches
  host panics, including JSON marshalers and error formatting.
- Keep host MCP progress separate from automatic tools-list refresh in all
  three built-in transports. Preserve the one-shot public FIFO and bounded
  backpressure, and stop idle refresh when the exact client closes or drops.
  Custom transports retain their legacy notification contract unless they
  implement the additive independent tools-list change subscription.
- Parse and generate release-smoke configuration only through `a3s-acl`.
  Pass credentials through the child environment, use a private temporary ACL
  file, propagate child failure and remove the file when the child exits.
- Retain the existing research fact wire schema while binding a validated
  source snapshot in digest-bound metadata.
- Stabilize projected form/view field order across dependency feature graphs,
  including selection before the field cap. Preserve record-array order.
- Keep protocol cancellation acceptance separate from terminal publication.
  Cancel waits up to five seconds for the original Run store; pending
  publication returns an existing `AgentProtocolHostError::Code` carrier
  without a v1 receipt. Its additive `runtime_code()` query returns
  `a3s.code.agent_protocol.run_cancellation_pending`. Preserve the published
  exhaustive variants and const `code()` API; its const category remains
  `INTERNAL_ERROR` for this typed runtime source.
- Preserve the typed admission error when original cleanup remains unproved.
  Keep the original lease occupied and refuse a new Run until release is proved.

## Release gates

The required lanes are defined in [the 9.1.3 test contract](TEST_PLAN_9.1.3.md).
The latest [compatibility closeout](../../../docs/evidence/code-harness/20261012-v913-protocol-compatibility/OUTCOME.md)
records current qualification and [remaining obligations](../../../docs/evidence/code-harness/20261012-v913-protocol-compatibility/FAILURES.md).

| Current local check | Disposition |
| --- | --- |
| Complete combined-feature Core | 4,981 passed / 11 failed / 31 ignored. All11 failures reproduce SOLO from the same frozen executable. This graph includes real-provider Deep38 cases and is not hermetic. |
| Published Rust error contracts | v9.1.2 exhaustive Core/Host matches and const calls compile. Typed errors3/3, original protocol cancellation2/2 and cancellation-publication5/5 pass. This is not a complete public API certificate. |
| Strict checks | Core and all three native SDK Clippy checks pass. Workspace compilation,16 controlled-file formatting, architecture/capability checks and18 validator tests pass. The prior128-file workspace formatting failure remains open. |
| Native SDK contracts | All three artifacts are rebuilt; Node helper/runtime/type checks pass, Python37 pass/4 skip, Go race-contract test rows90 pass/1 skip. Actual blocking cancellations retain RUN_CANCELLED and original IDs. Skips receive no passing credit. |
| Selected real-provider checks | Three SDKs produce six distinct Completed Runs with exact original evidence targets and stable artifact hashes. HTTP wire3/3 and real MCP progress1/1 also pass. These checks do not qualify complete Layer C or hosted targets. |
| Version/dependency checks | Version alignment passes at9.1.3. Core and ACP dependency preflights fail on the unversioned OKF sibling path; required public dependency source parity remains unproved. |

Supporting repair evidence remains retained for
[model settlement](../../../docs/evidence/code-harness/20261011-v913-model-settlement/OUTCOME.md),
[MCP notifications](../../../docs/evidence/code-harness/20261012-v913-mcp-notifications/OUTCOME.md),
[terminal projection](../../../docs/evidence/code-harness/20261012-v913-terminal-followup/OUTCOME.md),
[canonical recovery fixtures](../../../docs/evidence/code-harness/20261012-v913-owned-recovery/OUTCOME.md)
and [cancelled host delivery](../../../docs/evidence/code-harness/20261012-v913-cancel-host-events/OUTCOME.md).
Cancelled host delivery may end with its queued prefix; exact Run snapshots and
retained events carry terminal state and interactive results. EOF alone proves
neither successful completion nor original cleanup.

The last complete53-suite Layer C campaign failed19 mandatory suites, with
41 exact SOLO cases giving10 pass/30 fail/1 deadline; its extended install-skill
replay also failed. That historical campaign has not been replaced by a passing
current full inventory. Selected local passes do not satisfy the release gates.

| Gate | Required evidence before tagging |
| --- | --- |
| Regression qualification | Repair and requalify original execution cleanup, cancellation continuation, exact/portable checkpoint recovery and unresolved tool receipts. Retain initial failures and SOLO results; then execute the complete unchanged Layer C inventory. Selected SDK/TUI passes do not discharge these failures. |
| Dependency delivery | Deliver and pin the exact Effect, Memory, Search, Use core/reconcile and OKF implementations used by the qualified local build; verify all three SDK dependency graphs after removing local patches. The public Effect pin does not contain the local fact-log changes. Registry existence alone does not prove source parity; the current OKF and Use reconcile versions are absent from crates.io. |
| Standalone build | Resolve `a3s-okf`'s unversioned sibling paths (Core dev dependency and ACP dependency) and verify the ACP reconcile dependency. Run setup, relock, strict checks and packaging from the actual standalone Code checkout. A manifest transformation is not a build certificate. |
| Shared source closure | Review a content-filtered candidate patch, retain evidence and a ROADMAP row in the same landing, and make whole-workspace formatting pass. The shared worktree currently contains unrelated unfinished edits. |
| Hosted qualification | Run the complete release chain at the committed candidate revision, including platform builds, memory/retrieval soaks, hermetic integrations, performance, SDK runtimes and the shipped pager checks. Local macOS debug artifacts do not certify other targets. |
| npm delivery | Verify all nine Trusted Publisher package bindings for `A3S-Lab/Code` and the calling `.github/workflows/release.yml`, as specified in [the npm publishing contract](NPM_TRUSTED_PUBLISHING.md), then retain the actual registry delivery result. The reusable `publish-node.yml` is not the publisher binding. A prepared workflow does not prove registry registration. |

## Issue disposition

| Issue | Candidate disposition |
| --- | --- |
| [#187](https://github.com/A3S-Lab/Code/issues/187) | The rebuilt local 9.1.3 pager passes non-TTY startup qualification. Keep the issue open until the released/downloaded artifact passes the same check. |
| [#189](https://github.com/A3S-Lab/Code/issues/189) | Session catalog, recovery admission and pinned fact storage have retained local evidence. Successful recovery needs the admission-to-writer capability handoff. Picker, fork and dashboard activation remain gated. |
| [#176](https://github.com/A3S-Lab/Code/issues/176) | Trusted Publisher workflow preparation is local; external package bindings and actual npm delivery remain unproven. Office delivery is not certified by this Code candidate. |
| [#162](https://github.com/A3S-Lab/Code/issues/162) | The private Search cascade replacement remains open. A dependency version change alone cannot prove execution through `EngineTierPlan`. |

## Host callback limits

Python sampling registration captures a running asyncio loop. Keep that loop
free to serve callbacks; use `asyncio.to_thread` for blocking SDK operations.
Node timeout refuses the MCP request but cannot force a host Promise to stop.
Go cancellation is cooperative through `context.Context`; it cannot terminate
an arbitrary host goroutine. Hosts remain responsible for consent, model
selection, side effects and callback resource ownership.

Do not use `release.sh` on this shared worktree: its directory-wide staging
does not satisfy the repository's content-filtered landing rule. Do not tag,
publish, or close issues on the strength of this document alone.
