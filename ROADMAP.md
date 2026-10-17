# A3S Code Roadmap

## 1. Scope and authority

A3S Code is the native coding-Agent Harness and one implementation of the
provider-neutral A3S Cloud `AgentExecutionProvider` contract. It owns coding
Agent loop semantics, workspace tools, model adapters, context construction,
Tool request/result events, deterministic context reduction, session
snapshots, and provider-local recovery.

A3S Cloud owns Agent releases, conversations, executions, provider bindings,
grants, approvals, checkpoint/fork business lineage, audit, placement,
deployment, and product availability. A3S Runtime owns generic Task/Service
lifecycle. A3S Sandbox owns the local command-process boundary; Box and OCI
Runtime own stronger workload execution and isolation. Gateway owns public
request traffic and inference request accounting.

Shared ownership and Cloud Wave 3 (`A1`) dependency order are defined by the
[Cloud agent and capability portfolio roadmap](https://github.com/A3S-Lab/Cloud/blob/main/docs/project-roadmaps/agents-and-capabilities.md),
[architecture optimization roadmap](https://github.com/A3S-Lab/Cloud/blob/main/docs/architecture-optimization-roadmap.md),
and
[cloud-substrate-dependency-roadmap.md](https://github.com/A3S-Lab/a3s/blob/main/docs/cloud-substrate-dependency-roadmap.md).
This roadmap must not create a second Cloud Agent lifecycle, scheduler, queue,
Secret store, usage ledger, or checkpoint authority. Real Box recovery remains
a Cloud gate; Code does not bypass Runtime/Box.

Scientific discovery is a product composition over this runtime, not a new
runtime mode. The cross-repository implementation plan is tracked in the
[scientific discovery platform roadmap](https://github.com/A3S-Lab/a3s/blob/main/docs/scientific-discovery-platform-roadmap.md).

## 2. Current foundation

- `Agent`, `AgentSession`, governed Tool invocation, model adapters, bounded
  context, events, artifacts, and atomic `SessionSnapshotV1` are available.
- `AgentProtocolHarness` and `AgentProtocolHost` expose release/session/run,
  cancellation, checkpoint recovery, receipts, and bounded event pages.
- The independent A3S Sandbox crate is the default local command boundary for
  Code sessions on macOS, Linux, and Windows. Its native policy and lifecycle
  are linked through `NativeBashSandbox`; Bash, workflows, Skills, and delegated
  runs inherit one handle, while remote workspace services keep their own
  command-runner contract.
- Harness event pages now bind run state, logical observation time, and the
  retained event window from one atomic RunStore generation. Restored runs
  keep that run-local logical time monotonic across new events, cancellation,
  and failure instead of regressing to a host's earlier wall clock. A cursor
  at or beyond the exclusive Code tail fails closed instead of silently
  skipping future events.
- The complete native Cloud provider integration and real Box recovery gate
  remain owned by Cloud `A1.2` and its compatibility lock.

## 3. Delivery plan

| Gate | State | Code-owned outcome | Boundary |
| --- | --- | --- | --- |
| `CODE-STUDIO-REFERENCE-VERIFICATION-GUIDANCE` | Fixed-source worker ceiling verified; form/browser qualification open | Fresh verification debt and owned question transport remain authoritative. Fixed-source Studio workers explicitly admit a deny-all checker; model tool schemas are empty and a yolo mode change cannot authorize a governed read. Real Boyue B/text-clean and unchanged regex/Git graders pass (1/1, 5/5) at the recorded candidate. 249 serial ACP and 46 prompt tests pass (8 existing ignored); strict Core/ACP Clippy passes. F06 retains its invalid-domain browser failure; the native-validation API candidate passes fresh Boyue original graders, B/text-clean and the frozen form browser boundary/390px/HAR checks within the unchanged prompt budget | [Core outcome](docs/evidence/studio/20261016-source-review-constraints/OUTCOME.md), [Studio evidence](../../docs/evidence/studio/20261015-reference-alignment/OUTCOME.md). Formal deployment, native macOS, complete type and whole-product gates remain open |
| `CODE-TUI-ACL-REFUSAL-9.1.3` | Selected direct ACL TUI variants pass 3 × each; task-file reliability OPEN; release HOLD | Actual A3S Test1.0.1 engine and real DeepSeek: Bash/background direct refusals each3 passes, two distinct terminal Runs per case, original Denied+Complete then read success, Unknown0. Eleven accepted originals re-audited;29 corrupt copies rejected on8 actual baselines. Six direct runs leave no selected owned control process or scoped temporary ACL | [Actual ACL-engine evidence](../../docs/evidence/code-harness/20261012-tui-acl-native-refusal/OUTCOME.md). Task-file invocation omissions, first failures, exact SOLO and aborted observer/specs retained separately. RT10-03/04/05 live faults, full P0/catalog, default native, other adapter/platform, Core full profile, dependencies and release remain open |
| `CODE-TUI-REQUEST-TERMINALS-9.1.3` | Selected real TUI target passes 3 × 10/10; release HOLD | Same frozen pager/ACP/test binaries, actual DeepSeek route. Four sequential prompt terminals bind four distinct Runs, executed partial checker exit1, then two current-effect Verified reports and actual replies. Independent audit recomputes effects;30 corrupt coding copies rejected. Read5/5 plus26 negative copies per run;13 hermetic,3 transparent-relay tests and strict Core/ACP Clippy pass | [Request-terminal evidence](../../docs/evidence/code-harness/20261012-tui-request-terminals/OUTCOME.md). First external coding snapshot misses final two facts; later two are complete. No default-native, full P0/catalog, other adapter/platform, Core full-suite, dependency/source closure or release claim |
| `CODE-TUI-HOST-VERIFICATION-DELIVERY-9.1.3` | Historical host/delivery candidates: complete TUI gate failed; release HOLD | Host-only scoped ACL contracts, exact commands and effect-bound reports; ACP delivers an actual final model reply. Projected Tools preserve backend preflight while live authority remains enforced. Core capability28/receipts53/fact-log20/config88/verification57 and strict Clippy pass. Delivery-candidate actual TUI9/1; exact long SOLO1/0. Current ACP first198/2, both failures SOLO pass, full repeat200/0;4 ignored. Projected-candidate TUI8/2; both exact coding SOLO pass; full failure retained. Initial-context candidate: TUI9/1 and multi SOLO pass; Core fact-log22, ACP200/0+4 ignored,10 hermetic, strict Clippy and independent read5/5 pass. Historical partial-checkpoint/Run-terminal driver gap; current repair recorded separately | [Host/delivery evidence](../../docs/evidence/code-harness/20261012-tui-host-verification-delivery/OUTCOME.md); first failures and ABORTED_SPEC retained. No full-catalog/native/platform/release claim |
| `CODE-TUI-READ-TL01-9.1.3` | Selected read family 5/5 passes in three runs; strict complete gate 8 pass/2 fail | Thirteen real turns per five-case read family; independent receipt/input/fixture checks pass and 26 corrupt copies rejected per run. Original four failures reproduced SOLO and driver/spec faults repaired. Both strict coding failures rerun SOLO, still fail: formal TUI host-checker entry and exact fixture command remain open. Mandatory ten-test target retains floor10; runner27 checks and selected strict Clippy pass | [Read and coding-gate evidence](../../docs/evidence/code-harness/20261012-tui-read-real-llm/OUTCOME.md); no positive coding, full catalog, native execution, complete live gate or release claim |
| `CODE-TUI-TEN-TURN-AND-FIXTURE-TRUTH-9.1.3` | Selected recovery and truthful admission verified; full profile red; release HOLD | Fix plan-heavy edit and require real bytes, bound receipts, actual Node verification and Verified closure. Receipt family50/50; final actual-provider PTY2/2 includes10 consecutive turns,10 distinct Runs,5 Denied,5 successful reads,Unknown0. Required ci-all+deep-live Core4,998 pass/10 fail/31 ignored; all10 SOLO reproduced. Strict ci-all Clippy passes. Deep wrong-feature no-op is now explicitly ignored, forced run fails;7 admission/refusal checks pass. Welcome audit extends plan to201 rows/1,015 cases,20 structure checks pass | [Fixture/repeated-recovery evidence](../../docs/evidence/code-harness/20261012-plan-heavy-ten-turn-recovery/OUTCOME.md); native checker remains denied; dirty-candidate45-case receipt gate, dependencies and landing ratchet remain failed. No full catalog, native support or release claim |
| `CODE-TUI-NATIVE-REFUSAL-RECOVERY-9.1.3` | Selected local prevention verified; release HOLD | Side-effect-free availability checks before dispatch and after hooks prevent a known native refusal from creating Unknown debt; post-dispatch uncertainty and observed hook effects remain unresolved. Receipt family50/50 and real-provider same-session refusal→read recovery1/1 pass; strict selected Core/ACP Clippy passes | [Native refusal/recovery evidence](../../docs/evidence/code-harness/20261012-tui-native-refusal-recovery/OUTCOME.md); macOS native execution remains unavailable, old Unknown facts are not erased. Preceding full suite4,993 pass/9 fail/31 ignored, all9 reproduce SOLO; no current-generation whole-suite or release claim |
| `CODE-TUI-COMPLETE-E2E-PLAN-20261012` | Plan complete; full catalog execution pending | Map75 pager builtins, dynamic `/goal`,52 settings,72 actions and6 welcome entries; specify1,020 cases across202 behavior/setting/restriction rows, at least5 each, including host verification entry. Eleven-domain three-role review and6 ordered packages preserve real-model, artifact, recovery and platform gates | [Current plan](manual/tui-e2e/20261012/README.md), [host-entry update](../../docs/evidence/code-harness/20261012-tui-host-verification-delivery/OUTCOME.md), [request-terminal qualification](../../docs/evidence/code-harness/20261012-tui-request-terminals/OUTCOME.md);27 structure checks and156 recorded fingerprints. [Prior201-row review](../../docs/evidence/code-harness/20261012-tui-plan-role-review/OUTCOME.md) and [initial certificate](../../docs/evidence/code-harness/20261012-tui-complete-e2e-plan/OUTCOME.md) retained. Restrictions prove refusal only; no full execution or release claim |
| `CODE-MCP-NOTIFY-9.1.3` | Built-in MCP notification repair verified locally; release HOLD | Restore37 previously unregistered manager tests. Preserve the host FIFO while a separate coalesced change signal drives existing cache refresh; exact client close/drop stops idle refresh. Default and combined-profile MCP221/221, actual HTTP wire3/3 and unchanged exact real-provider progress1/1 pass; strict Core lib/tests Clippy passes | [MCP notification evidence](../../docs/evidence/code-harness/20261012-v913-mcp-notifications/OUTCOME.md); full Core serial follow-up finishes4,960 pass/22 fail/31 ignored, all22 failures reproduced SOLO. Earlier SDK artifacts require rebuilding after this Core change; complete Layer C, atomic candidate, public dependencies and hosted/native qualification remain open |
| `CODE-TERMINAL-FOLLOWUP-9.1.3` | Selected terminal/projection repairs verified locally; release HOLD | Surface41/41, protocol Cancel3/3, typed admission/retained-owner1/1 and memory retention14/14 pass. Sort object fields before the cap, observe original terminal publication before Cancel receipts, retain typed admission causes and unproved ownership, and align the memory fixture with actual committer-before-terminal ordering | [Terminal follow-up evidence](../../docs/evidence/code-harness/20261012-v913-terminal-followup/OUTCOME.md); complete current Core4,970 passed/16 failed/31 ignored, all16 reproduce SOLO. Strict Core Clippy, workspace all-target check,19-file formatting, HTTP3/3, real MCP1/1 and rebuilt Node/Python/Go SDK checks pass, including six distinct real-provider Completed Runs. Workspace formatting128 files, full Layer C, public dependencies/source closure and hosted delivery remain unqualified |
| `CODE-OWNED-RECOVERY-FIXTURES-9.1.3` | Canonical recovery fixtures30/30; release HOLD | Migrate four obsolete unbound fixtures to the same actual original worker, captured scope/inbox/committer and guardian disposition. Preserve immutable binding, actual FileLog I/O failure, failed-close owner retention and source-bound negative input identity; remove unused test-only legacy bind/failure helpers | [Owned recovery evidence](../../docs/evidence/code-harness/20261012-v913-owned-recovery/OUTCOME.md); complete current Core4,974 passed/12 failed/31 ignored, all12 reproduced SOLO. Strict Core Clippy, workspace all-target compilation,23-file formatting, architecture/capability gates and18 validation-script tests pass; workspace formatting128 files fails. Original confirmation-close diagnostic still fails, and the original test is restored unchanged. Native/Layer C, source closure, dependencies and hosted delivery remain unqualified |
| `CODE-CANCEL-HOST-EVENTS-9.1.3` | Host-event cancellation3/3 and confirmation family17/17; release HOLD | Observe actual captured Run cancellation during host sends, preserve the bounded FIFO prefix and normal backpressure, retain interactive results on the exact Run, and let the unchanged original worker/supervisor complete its actual cleanup | [Cancellation/host-event evidence](../../docs/evidence/code-harness/20261012-v913-cancel-host-events/OUTCOME.md); the repaired confirmation-close case joins with a still-full host FIFO, zero tool starts, no next provider response and zero active admission. Complete Core4,978 passed/11 failed/31 ignored, all11 reproduced SOLO. Strict checks,25-file formatting, HTTP3/3, real MCP1/1 and rebuilt three-SDK gates pass, including six actual real-provider Completed Runs. Workspace formatting128 files fails; Layer C, dependencies, source closure and hosted delivery remain unqualified |
| `CODE-PUBLISH-ATTEMPT-9.1.3` | Cargo publication preflight fails; nothing uploaded | Current version alignment passes; Core/ACP registry checks and whole formatting fail. Normal dry-run refuses295 uncommitted Core files; diagnostic package verification against public dependencies fails with36 compile errors | [Publication attempt evidence](../../docs/evidence/code-harness/20261012-v913-publish-attempt/OUTCOME.md); required Effect/Search APIs are absent from the packaged public graph, OKF/reconcile remain absent, and private OKF delivery/source closure and full certification are unresolved |
| `CODE-ERROR-COMPATIBILITY-9.1.3` | Two published Rust error contracts and rebuilt SDK gates verified; release HOLD | Preserve v9.1.2 exhaustive Core/Host variants and const code methods; additive typed runtime queries keep RUN_CANCELLED and original IDs in all three SDKs. External compile guard1/1, typed error3/3, original protocol2/2 and cancellation-publication5/5 pass | [Rust error compatibility evidence](../../docs/evidence/code-harness/20261012-v913-protocol-compatibility/OUTCOME.md); full Core4,981 passed/11 failed/31 ignored, all11 reproduced SOLO. Strict Core/three-SDK checks, workspace compilation,16-file formatting, HTTP3/3, real MCP1/1 and current SDK gates pass, including six distinct real-provider Completed Runs. Complete Layer C, prior128-file workspace formatting, dependency/source closure and hosted delivery remain open |
| `CODE-RELEASE-9.1.3-CLOSEOUT` | Historical partial local checks; full qualification held | Version alignment includes the actual pager; SDK admission/cancellation/evidence, host-owned MCP sampling and typed options are bound to Core. Strict Core/ACP/Go/Node/Python Clippy, native SDK contract checks, ACL preparation and the rebuilt 9.1.3 pager pass | [Release closeout evidence](../../docs/evidence/code-harness/20261011-release-closeout/OUTCOME.md) and [candidate disposition](manual/RELEASE_9.1.3.md); dependency delivery, standalone build, shared-source formatting/landing, hosted certification and external npm bindings remain open |
| `CODE-TEST-9.1.3` | Original-model settlement repaired locally; full53-suite campaign and independent replays failed; release HOLD | Related Rust181 pass across retained generations; five selected real-provider recovery/cancellation cases pass. All4,936 ordinary Core cases executed through disjoint serial processes, with remaining failures retained and SOLO checked. Rebuilt SDK suites and actual stream/blocking cancellation continuity pass; all three expose RUN_CANCELLED. Complete Layer C receipts reconcile: mandatory29 first-pass/2 retry-pass/19 fail, measurement3 first-pass. Exact41 SOLO cases:10 pass/30 fail/1 deadline; extended install-skill replay also fails. Corrected exact Steer oracle, formatting and strict Clippy pass. Three-SDK real-provider gate proves six distinct Completed Runs. Frozen current-generation TUI live bodies:1 pass/2 fail | [Model settlement follow-up](../../docs/evidence/code-harness/20261011-v913-model-settlement/OUTCOME.md), [complete live results](../../docs/evidence/code-harness/20261011-v913-model-settlement/LIVE_RESULTS.md), [current obligations](../../docs/evidence/code-harness/20261011-v913-model-settlement/FAILURES.md) and [comprehensive test outcome](../../docs/evidence/code-harness/20261011-v913-comprehensive-tests/OUTCOME.md); whole formatting, single committed candidate, standalone/public dependencies, missing-config false-green, shared TUI closure and hosted/platform delivery remain open |
| `CODE-SKILL-WITHHOLD1` | Lib tests passed on 2026-10-11 | A directory-discovered skill whose file differs from the admitted generation is withheld on a fork: neither body enters the run, the session still publishes, and the Skill tool returns the fixed sentence. Host `register_host` names and live `add_skill` replacements still reject the session. `snapshot_with_external_skills` still returns `NameConflict` for a conflicting skill that is passed in | [Skill withhold evidence](../../docs/evidence/code-harness/20261011-skill-name-withhold/OUTCOME.md); the running ToWork binary does not contain this change; tool, MCP, and agent name conflicts still abort; no new HTTP 200 |
| `ISSUE-162-SEARCH-CASCADE` | Locally verified; ships with 9.1.3 | `web_search` delegates tier order, engine construction, structural gating and the headless tier to a3s-search 3.3.0 `execute_cascade`; Code supplies the session circuit/bulkhead/coalescer/retry budget, safe-URL admission, and a Moli runtime provisioned in parallel | [Issue sweep evidence](../../docs/evidence/code-harness/20261015-issue-sweep/OUTCOME.md); `web_search` lib tests 44/44 without and 45/45 with `headless-search`; the Search suite passes |
| `ISSUE-189-PINNED-FACT-LOG` | Storage boundary locally verified | Capability-held Effect FileLog, bounded canonical facts, same-handle read/append, exclusive truncation scratch and atomic batch validation; 151 hermetic tests plus four actual LLM scenarios passed | [Retained fact-log evidence](../../docs/evidence/code-harness/20261011-pinned-fact-log/OUTCOME.md); Core admission-to-writer handoff, successful ACP recovery, cross-process durability and public delivery remain open |
| `STUDIO-PLUGIN-PROCESS1` | Process slice verified: 21 relevant contracts and all-target Core/ACP Clippy passed | Installed ACP Tool executions reuse Core bounded output, concurrent input and one process-group owner; cancellation, timeout and normal exit retire owned background writers, and direct-child cleanup failures surface | [Retained process evidence](../../docs/evidence/studio-plugins/20261008-acp-plugin-processes/OUTCOME.md); native scope/Use Run binding, hostile-process isolation, Windows/Linux certification and full Studio E2E remain pending |
| `STUDIO-PLUGIN-NATIVE-RUN1` | Actual native Tool/Harness Run binding verified | Distinct reconcile evidence, real exact SelectionLease provider, frozen Tool values across cutover, stale/withdrawn pre-inference refusal, cooperative process cleanup and executed merged Harness system ports | [Native Harness Tool evidence](../../docs/evidence/studio-plugins/20261008-native-harness-tool-binding/OUTCOME.md); complete native authorization, abrupt-host termination, isolated VSX hosts and full E2E remain pending |
| `STUDIO-PLUGIN-NATIVE-MCP1` | Native Skill/MCP runtime slice verified | Immutable Skill values, exact managed stdio MCP definitions/results across real Harness Runs, preparing/Ready process leases, shared close acknowledgement, cancel/timeout readiness revocation and observed rollback | [Native Skill/MCP evidence](../../docs/evidence/studio-plugins/20261008-native-skill-mcp-binding/OUTCOME.md); 501 distinct relevant contracts passed; scoped authority, nested Skill artifacts, remote side effects, VSX and complete cross-host E2E remain open |
| `STUDIO-PLUGIN-NATIVE-SPACE1` | Native workspace admission slice verified | Required launcher workspace, canonical/Unix directory identity, Core batch and Run validation, fail-closed unbound native providers, native Tool context validation and explicit stdio MCP cwd; failed late spatial check preserves observed rollback ownership | [Native workspace evidence](../../docs/evidence/studio-plugins/20261008-native-workspace-binding/OUTCOME.md); 315 contracts including actual ACP binary stdio admission passed; fine-grained resource policy, atomic file broker, persisted spatial recovery, VSX and full Studio E2E remain open |
| `STUDIO-PLUGIN-CAPABILITY-ACCESS1` | Projected access and actual fact invocation scope verified | Immutable batch allow-list, dependency denial, filtered runtime/host access, exact live Tool/MCP scope checks, actual fact Tool Turn settlement and child MCP inheritance without ambient fallback | [Capability access evidence](../../docs/evidence/studio-plugins/20261008-capability-access/OUTCOME.md); native Studio grant policy, resource containment, versioned documents, VSX and full E2E remain open |
| `STUDIO-PLUGIN-DELEGATED-TOOLS1` | Actual delegated plugin Tool slice verified | Task children inherit accepted Run-frozen generic Tool wrappers; denied names cannot execute; native cutover retains N and cancelled children settle before withdrawn artifact retirement | [Delegated Tool evidence](../../docs/evidence/studio-plugins/20261008-delegated-plugin-tools/OUTCOME.md); 128 distinct contracts, four new; arbitrary recursive Tasks, native policy/containment, VSX and full Studio GUI E2E remain open |
| `STUDIO-PLUGIN-WORKSPACE-ACCESS1` | Actual Run resource ceiling and live Turn access verified | WorkspaceServices governs all six flags; immutable identity observations and weak live access; unscoped/denied native executables cannot spawn; existing cancellation and generation retention preserved | [Workspace operation evidence](../../docs/evidence/studio-plugins/20261008-workspace-operation-access/OUTCOME.md); package/principal grants, document/VSX adapters and full Studio E2E remain open |
| `STUDIO-PLUGIN-DOCUMENT-READ1` | Native shared-document read slice verified | Opt-in compiled read adapter; actual Turn resource checks before/after ACP, exact workspace/URI and safe version bounds; cancelled/closed transport and replaced roots refuse observations | [Harness document read evidence](../../docs/evidence/studio-plugins/20261008-harness-document-read/OUTCOME.md); mutation completion receipts, package/principal grants, uncooperative native I/O, VSX and full Studio E2E remain open |
| `STUDIO-PLUGIN-RUN-COMPLETION1` | Run-owned resources and strict cleanup before completion | Live Turn registration into actual Run; send, stream, detached execution and both resume paths await scope-close acknowledgement; cleanup failure/timeout and late cancellation refuse success | [Run resource completion evidence](../../docs/evidence/studio-plugins/20261008-run-resource-completion/OUTCOME.md); native document mutation grants/receipts, durable foreign resource recovery, VSX containment and full Studio E2E remain open |
| `STUDIO-PLUGIN-DOCUMENT-PINS1` | Run-owned native readonly document leases verified | Actual Run instance observations distinguish repeated scope addresses; optional retained reads keep native models across Tool Turns and editor close; Run cleanup awaits session-bound native release and refuses failed, timed-out or cancelled completion | [Run document pin evidence](../../docs/evidence/studio-plugins/20261008-run-document-pins/OUTCOME.md); package/principal grants, mutation receipts, durable cleanup, blocked-I/O containment, VSX and full GUI E2E remain open |
| `STUDIO-PLUGIN-DOCUMENT-DIAGNOSTICS1` | Version-bound advisory diagnostic read verified | Actual Harness/ACP consumes native unsaved text and explicitly granted publications in one matching snapshot; strict publication/range validation, live Turn checks and an explicit incomplete-set flag preserve verification and cleanup gates | [Diagnostic foundation evidence](../../docs/evidence/studio-plugins/20261008-document-diagnostics/OUTCOME.md); foreign package/host generation policy, actual VSX execution, diagnostic display, mutation receipts and full GUI E2E remain open |
| `CAR-01` | Out of scope (Cloud retired 2026-09-26) | Conform the native Harness to Cloud `A1.2`/`A1.3` command, receipt, event-page, cancellation, recovery, fail-closed workspace/session admission, bounded change-set capture-state, balanced external-task lifecycle, and atomic/bounded legacy artifact persistence contracts | Cloud retains execution identity and sequencing authority |
| `CAR-02` | Delivered | Tool-request/result evidence, per-call input/usage diagnostics, and the Rust-host immutable-content adapter retain every raw Tool result plus compacted change sides behind exact content-addressed references (the digest must be a complete URI path segment; the local fallback exposes create-only replay with conflict fencing) | Cloud owns adapter authorization, provider selection, projections, and object lifecycle; Gateway usage remains the billed request ledger |
| `CAR-03` | Out of scope (Cloud retired 2026-09-26) | Deterministic Tool-result transforms, versioned source/result evidence, exact algorithm/policy digest bindings, replay-time policy validation, host-injected immutable original references, and one shared ToolRegistry observation pipeline are delivered; Cloud-managed profile admission and cross-repository conformance remain | Cloud pins policy; Code does not invent tenant policy or mutate past events |
| `CAR-04` | Out of scope (Cloud retired 2026-09-26) | Canonical `SessionCheckpointExportV1` payloads bind `SessionSnapshotV1`, optional between-tool-round logical resume evidence, and exact component/aggregate identities; a host-injected `SessionCheckpointExportSink` captures both components from one acknowledged live Run boundary after preceding events and capability-owned effects settle; every new logical checkpoint binds the source Run's exact Code catalog, authority ceiling, and optional Use cursor; recovery pins that complete historical generation before target-Run admission, and the Code Harness restores both components plus an optional exact host capability batch as one visible admission without split store prewrites; common Harness adoption and real provider/Box certification remain | Cloud `A1.6` owns checkpoint identity, immutable-object authorization, external revision fencing, retention, approval, and fork lineage |
| `CAR-05` | Out of scope (Cloud retired 2026-09-26) | Pass restart, exact replay, cancellation, hostile Tool output, bounded-content, Secret-redaction, checkpoint, and cleanup conformance through one Cloud-managed Box workload | No direct Code-to-node control path |
| `WORKFLOW-RESULT1` | Delivered | Resumable workflow checkpoints and Flow decision claims share canonical execution identities and bounded digest-only result receipts; stale or unreadable state fails closed while legacy records remain loadable | Core checkpoint, Flow ledger, restart, takeover, and identity-fencing tests pass; host policy and business retention remain outside Code |
| `WORKFLOW-SCHED1` | Delivered | Dynamic Flow history projects into the canonical `ExecutionPlan`; step admission shares cancellation and bounded per-workflow quotas, while standalone scheduler leases carry digest-only step identities and delegated tasks use the same identity boundary | Plan identity is stable across status changes; resumed projections retain prior steps; local and global admission tests cover priority, cancellation, serialization, and the max-active=1 nested-deadlock boundary |
| `WORKFLOW-CONTROL1` | Delivered | A host-facing dynamic-workflow control handle coordinates bounded inspection, trusted history, Flow durable cancellation/terminal transitions, identity-bound worker leases, and cross-process local event-store locking | Independent-process qualification covers busy ownership, killed-worker lease expiry/takeover, cancellation settlement, digest-only projections, and optimistic event-conflict retry; Flow remains the only workflow authority |
| `WORKFLOW-OBS1` | Delivered | Scheduler and dynamic-workflow control diagnostics expose bounded admission, fairness, lease, and takeover counters without retaining task or workflow payloads | Independent resumed workers prove aging prevents starvation; scheduler wait/occupancy counters and durable claim-takeover counters converge while Flow history and the lease remain authoritative |
| `WORKFLOW-QUOTA1` | Delivered | The existing scheduler actor enforces typed digest-only owner quotas for standalone Flow steps and detached Task children, propagates run identity through governed Tool contexts, and exposes a live quota projection without adding a queue or store | Mixed-owner progress, quota-blocked priority work, cancellation, identity/limit conflicts, malformed scopes, idle-state pruning, dynamic diagnostics, and detached fan-out qualification pass |
| `MODEL-ADMISSION1` | Delivered | Provider/model capacity is represented by a typed digest-only `ModelGenerationPool`; regular, streaming, and structured calls reserve that capacity through quota-only admissions in the existing scheduler, with propagation through sessions, direct Tools, delegated children, and dynamic workflows | Cross-session and mixed-provider progress, local-plus-shared quota composition, structured repair without recursive deadlock, stream/cancellation release, endpoint-credential redaction, bounded per-pool health/configuration evidence, and the full Core regression suite pass |
| `ACP-QUESTION1` | Delivered | Agent questions riding the `request_permission` channel carry a typed `question` meta marker (with `allowFreeText`) so clients render choices by name, the question text without `Allow …?` framing, and a single free-text row; typed followup text outranks the selected choice row, and sampling consent refuses typed text | [Question rendering evidence](../../docs/evidence/standards/2026-10-10-acp-question-permission-rendering/OUTCOME.md); choices keep the `RejectOnce` YOLO guard; PTY/scripted live leg and an ACP-native question request type remain open |
| `ACP-FINALITY1` | Delivered | Turn finality stops hiding failures as completion: streaming requests bound response-headers wait (default 120s) and TCP connect (30s); unparseable truncated tool arguments close the OpenAI-compatible stream without `Done` so the shape retries; a dead turn worker fails the turn naming its panic; the settled End marker carries the gate's real verification summary; and the stream de-dup is per-response so an unstreamed final answer is re-sent instead of lost on both rails | [Turn finality evidence](../../docs/evidence/standards/2026-10-10-acp-turn-finality/OUTCOME.md); `stop_reason=length` continuation, user-visible summary rendering, and deadlines on permission/question/task-wait parks remain open |
| `ACP-OUTCOME1` | Delivered | The terminal marker says how the run closed: the settled End event carries the gate's typed `completion_terminal`, the agent emits `x.ai/turn/outcome` (terminal, status, check counts) before the response resolves, and the pager renders an outcome line — verified (N checks) / waived / split digests, with failure or needs-review suffixes — while a clean narrative close stays silent | [Turn outcome evidence](../../docs/evidence/standards/2026-10-10-acp-turn-outcome/OUTCOME.md); forced-finalization cause markers, replay rendering of outcome lines, agent stderr-to-file, and inactive-agent permission surfacing remain open |
| `ACP-DIAG1` | Delivered | Parked permissions re-ring per agent (HashSet marks retained against still-pending agents; a drained agent re-arms, a second agent's bell is never silenced by the first), and the agent appends an INFO daily log under `~/.a3s/logs` (seven-day prune, env overrides, stderr-only degradation) so hangs leave an on-disk trail | [Hang diagnostics evidence](../../docs/evidence/standards/2026-10-10-acp-hang-diagnostics/OUTCOME.md); ask-timeout deadlines on parked permissions, in-day size caps, and forced-finalization cause markers remain open |
| `ACP-ASKDEADLINE1` | Delivered | Unanswered `request_permission` asks fail closed at a 30-minute default deadline (`A3S_CODE_PERMISSION_ASK_TIMEOUT_SECS`, 0 disables): a timed-out tool ask denies with a warn on the log file, a timed-out question yields no answer, both instead of parking the turn forever; the agent log gains a 16 MiB in-day size cap and dir-parameterized routing with content-pinned tests | [Ask deadline evidence](../../docs/evidence/standards/2026-10-10-acp-ask-deadline/OUTCOME.md); form requests stay unbounded by contract; forced-finalization markers and stop_reason=length continuation remain open |
| `ACP-FORCEDSTOP1` | Delivered | A forced stop says so: the tool-budget deny (observed as the typed `budget.denied` fact on the turn's log slice) and the read-only evidence ceiling (flagged on the surface at both injection sites) converge at gate_result into `AgentResult.forced_stop`, ride the End event, and render a `stopped: tool budget reached` / `stopped: read-only evidence limit` line that overrides the outcome line's narrative silence | [Forced stop evidence](../../docs/evidence/standards/2026-10-10-acp-forced-stop/OUTCOME.md); the one-word `budget` model answer text remains; stop_reason=length continuation still open |
| `ACP-TRUNCATION1` | Delivered | A length-cut final answer says so: `stop_reason` `length`/`max_tokens` with no tool call (exact closed-set match in `decision_from_response`'s Text arm) flags the same surface slot the read-only ceiling uses, riding End → `x.ai/turn/outcome` → a `stopped: answer truncated (length limit)` line; a length cut that still carries a tool call stays on the FINALITY slice's truncation-evidence refusal | [Length truncation evidence](../../docs/evidence/standards/2026-10-10-acp-length-truncation/OUTCOME.md); no continuation/retry of the cut answer (legacy loop treats length as non-continuable); outcome-line replay rendering still open |
| `TUI-913-E2E` | Delivered | Shipped 9.1.3 pager and ACP open on the ACL default model, select a catalog model, complete one hermetic turn, and refuse a missing provider without an invented reply; two consecutive real-provider turns show non-empty assistant text | [TUI 9.1.3 evidence](../../docs/evidence/code-harness/20261011-tui-913-e2e/OUTCOME.md); CLI stays on core 9.1.2; a clipboard paste tip is pager chrome; a clean checkout compiles a3s-code-acp; no release publication, full real-LLM matrix, vendor rustfmt, or Cursor live quota run |
| `TUI-913-TURNS` | Delivered | Shipped 9.1.3 pager keeps one session across a diagnosis, a fix whose prompt does not restate the bug, and a check run; one bounded task observes the failing check, edits calc.py, and quotes SUM-OK-9 | [TUI 9.1.3 turns](../../docs/evidence/code-harness/20261011-tui-913-turns/OUTCOME.md); hermetic rows pass with no live provider; CLI stays on core 9.1.2; no release publication, full real-LLM matrix, vendor rustfmt, or Cursor live quota run |
| `TUI-913-DUAL` | Delivered | Shipped 9.1.3 pager keeps one session across a two-defect diagnosis, a one-defect partial fix whose prompt does not restate either defect, a second fix, and a checker quote; one bounded task sees the checker fail, fixes one defect, appends a new failing runs.txt line while that one defect remains, fixes the other, and the quote wait ignores screen text and, after the quote prompt, ends only when runs.txt gains a new line equal to the checker token from a traced python3 gate.py whose stdout is that same line; the cargo log prints that runs.txt, the bash call id, and that command | [TUI 9.1.3 two-defect](../../docs/evidence/code-harness/20261012-tui-913-dual/OUTCOME.md); hermetic rows pass with no live provider; CLI stays on core 9.1.2; no release publication, full real-LLM matrix, vendor rustfmt, or Cursor live quota run |

Observation precedes mutation: `CAR-02` must provide useful read-only context
diagnostics before `CAR-03` can reduce any Tool result. The first transform
profile is deterministic and limited to bounded truncation, head/tail
retention, repeated-line folding, structured sampling, and immutable original
content. Model-generated summarization requires a separate future policy and
replay contract.

The transform implementation keeps ownership lazy and traverses structured
arrays and repeated-line runs incrementally, avoiding an avoidable full-input
copy or container-sized indexing allocation for hostile Tool output. Regression
coverage includes UTF-8 head/tail retention, prefix-preserving folds, and
250,000-item JSON arrays; the v1 algorithm and evidence binding remain stable.

### 3.1 Native Harness architecture

Code defines its own Harness contract and does not target DeepSeek Harness,
Cordis, or another foreign runtime. First-principles review led to four native
boundaries without making Code Core a general dependency-injection or plugin
framework: the model sees the exact capability surface and readiness state it
can act on; each Run binds the actual model input rather than only its intended
configuration; temporary capabilities have explicit lifetimes; and richer
code-mode Tool presentation remains a host/profile concern over the same
governed execution world.

| Gate | State | Code-owned outcome | Exit criteria |
| --- | --- | --- | --- |
| `HARNESS-CAP1` | Delivered | Versioned per-run capability snapshots cover actual model-visible tools, workspace services, run-owned governance bindings, configured serializable policy identities, execution ceilings, and semantic readiness/generation identities | `run_capability_bound` is emitted before provider use and its digest changes on tool, serializable policy, service, readiness, or generation drift; authorization still executes through the same run-owned governance scope |
| `HARNESS-IN1` | Delivered | `ModelInputSnapshotV1` records bounded content-addressed evidence for the actual system/message/tool-definition/provider-facing structured directive submitted to each provider-neutral model call, plus identified semantic/hybrid Tool-result evidence | Digest/counter evidence, exact event replay, and separately retained immutable Tool-content references are delivered without copying original content into the evidence journal |
| `HARNESS-USAGE1` | Delivered | `ModelUsageSnapshotV1` measures exact repeated Tool-result context and binds the prompt estimate and normalized `LlmClient` token/cache usage to its input snapshot | Successful completion and streaming calls emit validated `model_usage_bound` evidence before returning their terminal response; replay is exact and run/stream cancellation releases evidence backpressure |
| `HARNESS-SCOPE1` | Delivered | Typed Session/Run/Turn/Subtask scopes are wired into real Agent execution with borrowed capability leases, monotonic ceilings, weak registration handles, and cancellation-safe supervised teardown | Compile-fail leases cannot escape or cross marker kinds; runtime tests settle model orchestration, Tool effects and stream bridges at real Turn boundaries, recursively scope Skill/Task Agents, reject stale promotion, and settle Run-owned Task/memory work before the exact Use lease |
| `HARNESS-PROFILE1` | Delivered | Closed typed Adaptive, Direct, Code, and Disabled Tool-presentation Profiles over the existing governed executor | Permission-filtered source and exact presented definition digests/counts/token estimates are bound before every model input; Profile identity, application kind, persistence, and replay validate without retaining definition plaintext |

`CODE-RDY1` below was the first delivered slice of `HARNESS-CAP1`. The completed
capability snapshot now observes the current readiness phase, catalog/source/
vector revisions, coverage, and model-descriptor digest immediately before each
provider call. The companion `model_input_bound` event is emitted before every
completion, streaming, structured, and streaming-structured call, and
`model_usage_bound` binds its successful result to the same call sequence. The
`tool_request_bound` event separately binds validated post-hook arguments and
its model, nested, or host-direct origin before permission, confirmation,
budget, or execution outcomes, so denied requests remain replayable without
adding argument plaintext to the bounded snapshot. The
companion `model_presentation_bound` event binds the frozen Profile, its
permission-filtered source cost, and exact provider-facing projection before
each input. All five events reuse the existing Run journal and
`EventEnvelopeV1`; no parallel audit store is introduced. See
[Harness Boundary Evidence](manual/HARNESS_EVIDENCE.md).

Delivered `CAR-02` adds a separate host port rather than a sixth evidence
event. A Rust host pairs one secret-free, digest-bound authority identity and
byte ceiling with an `ImmutableContentAdapter`. When configured, Code retains
every raw output returned by a Tool before releasing its bounded projection
and also retains change sides removed by metadata compaction. The provider must
return an absolute logical URI content-addressed by Code's exact SHA-256;
binding, descriptor, size, media type, and reference drift fail closed without
falling back to the session-local compatibility `ArtifactStore`. Session
snapshots persist only the binding and require the exact host adapter to be
re-injected on resume; delegated children inherit it. Cloud still resolves
authorization, provider/namespace, tenant projection, retention, and object
lifecycle outside Code.

The first Code-owned `CAR-04` slice maps temporal state without introducing a
second checkpoint authority. `SessionCheckpointExportV1` encodes one validated
`SessionSnapshotV1` plus an optional exact `LoopCheckpoint` as bounded,
recursively key-sorted compact JSON. Its descriptor binds the complete payload
and separately binds the snapshot and logical-resume components, including the
explicit `between_tool_rounds_v1` semantics, source Run, completed round, and
checkpoint time. Import requires byte-for-byte canonical encoding and
recomputes every component and aggregate digest. A logical resume must belong
to a non-terminal Run retained by the same snapshot, and the portable Session
view must carry that source Run's exact cognitive authority rather than a
newer next-Run catalog binding. The descriptor carries no Cloud checkpoint ID,
provider/namespace, object URI, retention, approval, or fork lineage. Cloud
`A1.6` will authorize and store the immutable bytes, own those business
identities, and map the descriptor into the future common Harness contract;
that cross-repository work and real Box recovery evidence remain open.

The second Code-owned `CAR-04` slice closes local recovery drift without
changing `AgentProtocolCommandV1` or Cloud's current recovery request.
`AgentProtocolRunRecoverExactV1` carries the complete
`SessionCheckpointDescriptorV1`. Its request digest settles the exact receipt,
and the descriptor digest becomes part of the target Run input identity.
`AgentProtocolHost::execute_exact_recovery()` first acquires the Session
execution lease, loads and validates the matching `LoopCheckpoint`, and retains
that immutable value in a prepared recovery plan. Only then does it capture the
workspace baseline and reserve the target Run. A later store overwrite cannot
change the admitted state, a mismatch creates no target Run, an identical
completed command remains replayable even after source retention changes, and
a different checkpoint conflicts with the already-bound target Run ID. The
original recovery command continues to mean "load the latest boundary for this
source Run" for wire compatibility.

The third Code-owned `CAR-04` slice removes split-write recovery from the
native Harness. `execute_checkpoint_recovery()` requires the request's complete
descriptor to equal the supplied export, revalidates and decodes both temporal
components from the same canonical bytes, restores an unpublished Session
directly from the snapshot, and starts exact recovery from the supplied logical
value. Only then is the Session entered into the Harness map. A different
persisted semantic generation is rejected when no target Run exists; a
persisted target follows exact replay/conflict rules; an unrelated live Session
is never replaced. This is atomic at the Harness visibility boundary only.
`SessionStore` has no cross-provider revision/CAS contract, so Cloud/common
Harness integration must fence external writers and bind the authorized object
generation before claiming distributed atomicity. Common-contract adoption and
real Box recovery certification remain open.

The fourth Code-owned `CAR-04` slice captures those two temporal components
from a live Run without exposing an internal checkpoint marker on the public
event protocol. A host injects `SessionCheckpointExportSink`; after a completed
Tool round, Code closes the capability Turn, drains the causally preceding
agent/runtime event queues, reads the source Run's frozen cognitive binding,
materializes one validated snapshot, writes the same logical value to an
optional `SessionStore`, and awaits the host sink before acknowledging the
agent loop. Blocking and streaming runs share this coordinator. Store and host
failures are isolated independently from the live Run, and export works without
a SessionStore. Ordered multi-round, concurrent Knowledge N/N+1 cutover,
cancellation/restart, competing recovery, and file-store restart tests cover
the Code-local boundary. Sink idempotency, encryption, external durability,
distributed fencing, common Harness adoption, and real Cloud-managed Box
recovery remain host gates.

The fifth Code-owned `CAR-04` slice binds runtime authority to the temporal
boundary. `RunCapabilityBindingV1` records the exact Code catalog generation
and digest, a canonical digest of the complete Run authority ceiling, and the
optional exact A3S Use cursor. Normal Run admission writes that identity before
execution, and each live `LoopCheckpoint` copies it unchanged. Recovery first
pins the currently reconstructed capability Run and compares the full binding;
an N checkpoint presented after N+1 cutover fails before target-Run admission,
including a cutover between preparation and spawn. For a missing Session,
`execute_checkpoint_recovery_with_capability_batch()` permits one atomic host-
supplied bootstrap from untouched generation zero to the checkpoint's exact
historical generation. It never resolves `latest`, and a missing, mismatched,
or partially prepared batch publishes neither the Session nor target Run.

### 3.2 Common execution-fact and auxiliary-evaluation substrate

The evaluation substrate is deliberately provider-neutral. It supplies the
runtime mechanisms that a host may use to build a reviewer, verifier, quality
gate, or other evaluator; it does not define any product rubric or finding
vocabulary. The substrate is implemented as a sidecar over the existing Run
journal, `EventEnvelopeV1`, `ArtifactStore`, structured LLM engine, and scoped
capability lifecycle. It must not create a second Cloud audit, checkpoint, or
fork authority.

| Gate | State | Code-owned outcome | Exit criteria |
| --- | --- | --- | --- |
| `EVAL-FND1` | Delivered | Versioned `ExecutionTargetV1`/`ExecutionFrameV1`, domain-separated digests, bounded limits, and explicit Code/host/Cloud ownership | Identities reject malformed or non-canonical values; no reviewer-specific terms or Cloud business identifiers are required by Core |
| `EVAL-JRN1` | Delivered | Digest-only `ExecutionFactV1` journal with contiguous cursors, artifact-reference extraction, idempotent append, FIFO retention, and explicit retention gaps | Reordered/conflicting events fail closed; exact replay is idempotent; retained facts never contain raw prompt, Tool output, or argument content |
| `EVAL-EVID1` | Delivered | Atomic RunStore observation projected through `EvidenceReader` into bounded `EvidenceSnapshotV1`, with digest-only or explicitly bounded payload modes and optional artifact/terminal-text access | State and event window come from one Run generation; prompt/result/error byte counts and digests remain available when plaintext is omitted; source/fact window drift marks evidence incomplete; content, byte, cursor, artifact, and redaction tests pass |
| `EVAL-AUX1` | Delivered | `AuxiliaryRunSpecV1`, capability ceiling checks, independent cancellation/deadline, structured-output adapter, bounded JSON output, idempotent IDs, lifecycle handles, terminal retention, Gate-mode fail-closed admission on incomplete/retention-gapped evidence, and supervisor `Suppressed` (retryable) outcome for Gate-on-incomplete rather than failing the parent observation | Auxiliary execution cannot broaden a declared parent ceiling; timeout, cancellation, panic, schema mismatch, duplicate admission, output overflow, and Gate-on-incomplete evidence all settle deterministically; incomplete Gate denials remain retryable after evidence completes |
| `EVAL-SUP1` | Delivered | Host-injected boundary policy and `EvaluationSupervisor` with turn/terminal/event triggers, debounce, pending admission, deterministic auxiliary IDs, and non-blocking completion watchers | Concurrent observations respect `max_pending`; replay never double-dispatches; policy remains outside Core and supervisor shutdown releases pending work |
| `EVAL-STORE1` | Delivered | Generic async `EvaluationResultSink` and content-addressed `EvaluationRecordV1` CAS contract with bounded in-memory reference implementation | Exact result replay is idempotent, conflicting writes fail closed, records are queryable by execution target, and no decision enum is imposed by Core |
| `EVAL-PROTO1` | Delivered | Additive `EvaluationWireEnvelopeV1` projection for evidence requests/snapshots, auxiliary lifecycle values, and immutable result records, with one Rust catalog and generated Node/Python/Go declarations | Strict Rust decode validates schema/version/kind, size, and typed payloads; generated catalog and negative fixtures are parity-checked across SDK projections; Cloud remains the business transport owner |
| `EVAL-DUR1` | Delivered | Bounded Tokio/file-backed `EvaluationResultSink` and restart-safe `EvaluationDispatchLedger` adapters with atomic publication, cross-process fencing, checked corruption paths, and FIFO/lease retention | Independent instances serialize writes; reopened generations validate every digest and identity; expired claims can be taken over; completed claims suppress replay; no raw evaluator prompt or rubric is persisted by Core |
| `EVAL-QUAL1` | Delivered | Provider-free qualification suite for adversarial redaction, durable restart/replay, retention, cancellation composition, and strict wire/result boundaries | Seven integration tests pass locally; release performance profile is wired with deterministic resource checks; no product reviewer vocabulary or Cloud authorization enters Core |
| `EVAL-GA1` | Delivered | External evaluator composition through host-owned policy/executor/result sinks, bounded file-backed result and dispatch adapters, and hosted release qualification | A new evaluator can be implemented outside Core; adversarial prompt-injection/secret-redaction, restart, retention, cancellation, performance, strict Clippy, rustdoc, and packaged SDK gates are green in hosted qualification runs |

EVAL-GA1 evidence is now complete for the provider-neutral Code boundary. The
hosted [Code CI run 33847689080](https://github.com/A3S-Lab/Code/actions/runs/33847689080)
passed strict rustdoc and Clippy, default and feature-gated Rust tests, the
convergence benchmark, packaged Node.js/Python/Go SDK checks, hermetic
integration checks, and retrieval soak on Linux, macOS, and Windows. The
hosted [performance run 33844533910](https://github.com/A3S-Lab/Code/actions/runs/33844533910)
validated all nine machine-readable profiles, including the evaluation
substrate profile and its bounded persistence/reopen/wire budgets. This closes
the common mechanism gate; real providers, external network or Cloud
qualification, reviewer rubric/authentication, and product workflows remain
host-owned responsibilities.

The delivered gates intentionally stop at the common mechanism. Core does not
ship a reviewer prompt, rubric, severity/threshold policy, shadow-review
strategy, disposition/reflag workflow, product UI, scientific artifact
parser, authentication flow, or Cloud audit endpoint. A host supplies an
`EvaluationPolicy` and `AuxiliaryExecutor`; it may use the built-in
`StructuredAuxiliaryExecutor` or its own provider-neutral implementation.

The dependency order is `EVAL-FND1 -> EVAL-JRN1 -> EVAL-EVID1 -> EVAL-AUX1 ->
EVAL-SUP1 -> EVAL-STORE1 -> EVAL-PROTO1 -> EVAL-DUR1 -> EVAL-QUAL1 ->
EVAL-GA1`. `EVAL-PROTO1` may begin
schema fixtures after the first four gates, but cannot claim compatibility
until all lifecycle and recovery invariants are covered. Durable fact/result
storage, tenant authorization, retention, placement, and business lineage
remain host/Cloud adapter responsibilities behind the published traits.

### 3.3 Native scientific research contracts

The first Code-side slice makes scientific execution observable and
reproducible without importing a foreign Harness or moving scientific policy
into Core. These contracts are transport and integrity primitives: A3S Use
binds the selected package/environment generation, while the host or Desktop
decides how to search, review, approve, retain, and publish results.

| Gate | State | Code-owned outcome | Exit criteria |
| --- | --- | --- | --- |
| `RESEARCH-CONTRACT1` | Delivered | Versioned `ResearchRunV1`, `ResearchEvidenceFactV1`, `ResearchProvenanceReceiptV1`, `ResearchReviewFindingV1`, and `ResearchEventV1` values with bounded fields, canonical digest identity, strict schemas, lifecycle validation, and bounded validated JSON helpers for every research value | Thirty focused unit tests pass; tampering, invalid transitions, metadata bounds, digest ordering, event naming, strict unknown-field rejection, and bounded wire recovery fail closed |
| `RESEARCH-EXEC1` | Delivered | Host adapter qualification drives a research run through source capture, evidence append, create-only content-addressed artifact publication, and evaluator dispatch while retaining one Code Run identity; exact execution-target validation and explicit Run-aware Core-event projection are covered | `research_execution_qualification` passes restart/replay, cancellation terminality, contiguous evidence, artifact immutability, file-backed evaluator dispatch/result recovery, and exact Code/Use binding checks |
| `RESEARCH-REVIEW1` | Delivered | Host-owned reviewer composition over the generic evaluation substrate, with Code binding each finding and bounded finding batch to immutable evaluator and optional artifact-provenance records without introducing a Core rubric; Gate-mode reviewer admission fail-closes on incomplete evidence at spawn and surfaces as supervisor `Suppressed` so the same fact can retry once evidence is complete; strict Run-aware evaluator/provenance/batch validation fences project-namespace, project-revision, provider, seed, evaluator identity, and evaluator/batch-evidence drift, and finding locations enforce one-based line/column coordinates; `research_review_qualification` proves the full chain through a real Gate-mode auxiliary reviewer dispatch, result-store retention, run-aware batch publication, strict wire round-trip, terminal resolve/waive decisions, and incomplete-evidence rejection | Reviewer checks citations, calculations, figure/code links, and reproducibility through injected policy; Code remains policy-neutral and rejects evaluator, Run, project-namespace, project-revision, provider/seed, evidence, binding, duplicate-id, partial-batch, malformed source locations, and Gate admission on incomplete evidence |
| `RESEARCH-FABRIC1` | Delivered | Versioned `ResearchClaimV1`, `ResearchCitationV1`, and `ResearchEvidenceGraphV1` with digest-only statements, explicit `proposed`/`supported`/`conflicted`/`unsupported` claim states, Run-aware graph admission, citation-to-claim closure, and measured publication completeness (`ResearchEvidenceCompletenessV1`) that rejects proposed claims and unlinked support digests | Unit tests prove wire round-trip, illegal transitions, orphan citations, mixed Run drift, and that every publication-ready claim carries support citations or an explicit gap digest without importing scientific rubrics |
| `RESEARCH-WORKFLOW1` | Delivered | Versioned `ResearchWorkflowStepV1`/`ResearchWorkflowPlanV1` bridge workflow step identities and optional `ExecutionResultReceiptV1` bindings into a research Run, with seed fencing and DAG validation; `ResearchRerunLineageV1` projects finding-triggered affected steps in dependency-first order, closing transitive dependents without rewriting the parent evidence ledger | Unit tests prove receipt identity/evidence fencing, seed drift rejection, transitive re-run selection, and lineage wire round-trip |
| `RESEARCH-REPRO1` | Delivered | Versioned `ResearchReproducibilityManifestV1` binds provider/model identity, optional model-revision digest, environment lock, code/workflow/parameter digests, seed, tolerance digests, and output artifact digests to one research Run without retaining prompts or credentials; Run-aware and provenance validation fence seed, provider, environment, workflow, code, and output-set drift | Unit tests prove wire round-trip, sorted unique digest lists, seed drift rejection, and that every declared output artifact must appear in matching provenance receipts |
| `RESEARCH-PROTO1` | Delivered | Additive `ResearchWireEnvelopeV1` projection for every research contract value, with one Rust catalog and generated Node/Python/Go declarations | Strict Rust decode validates schema/version/kind, size, and typed payloads; generated catalog and negative fixtures are parity-checked across SDK projections; Cloud remains the business transport owner |

The delivered contract slice is documented in
[Native Research Contracts](manual/RESEARCH_CONTRACTS.md). It is deliberately
small: it does not claim a complete project aggregate, scientific knowledge
graph store, package registry, or publication service. `RESEARCH-FABRIC1`
adds the claim/citation completeness fabric, `RESEARCH-WORKFLOW1` adds
the workflow-step bridge plus affected-step re-run lineage hosts need for
reviewer-driven partial recompute, and `RESEARCH-REPRO1` closes the KRN-7
reproducibility-manifest fence. `RESEARCH-PROTO1` publishes the same
strict research wire envelope to Node/Python/Go. Broader Desktop project aggregates,
host rubrics/UI, and Use science packages remain in the cross-repository
scientific discovery roadmap.

### 3.3.9 Core event fabric

The core identity and event-fabric slice (KRN-1) is delivered as an adapter
layer, not a second audit database. `CoreIdentity` carries the shared
`OperationId`, `SourceRevision`, `CapabilityStamp`, `EvidenceCursor`, and
`ArtifactRef` plane with an injectable logical clock; `CoreEventIdentity`
adapts agent and retained run events into canonical, domain-separated,
digest-only identities; and `CoreEventLog` appends those identities into one
bounded, hash-chained stream per operation where duplicate-content conflicts,
cursor gaps and skips, causality forks, stale capability generations, and
regressing source revisions fail closed. Identical re-delivery replays
idempotently, FIFO retention keeps explicit gaps while the cursor, chain
head, generation, and source pins survive trimming, and paged replay
revalidates every retained entry so tampering is detected rather than
trusted. The run journal, evaluation fact journal, and research projections
remain the compatibility authorities; routing every projection through the
log is an incremental migration with per-surface replay proofs, and the
research projection already preserves one identity through the log in its
committed regression.



### 3.3.10 Store durability negotiation (KRN-6 slice)

`SessionStoreCapabilities` now advertises the full KRN-6 guarantee set —
aggregate CAS, append-only event log, lease fencing, encryption at rest,
watch, and reference-aware artifact GC — beside the existing atomic
snapshot flag. Every new flag defaults to unadvertised so hosts must
negotiate before relying on a semantics, and a capability test locks the
default-and-advertise contract. The built-in memory and file adapters
advertise the atomic snapshot, aggregate CAS, reference-aware artifact GC,
and commit watch they prove; the file adapter additionally advertises its
append-only WAL and writer lease fencing.

| Gate | State | Code-owned outcome | Exit criteria |
| --- | --- | --- | --- |
| `STORE-WAL1` | Delivered | `FileSessionStore` records digest-only Intent/Committed WAL entries around each atomic snapshot replace; reopen seals an open Intent when the durable snapshot digest still matches, duplicate Intent sequences fail closed, and the file adapter advertises `append_only_event_log` | Focused store tests prove intent+commit pairing, crash-window recovery, and duplicate-sequence rejection without changing the public SessionStore API |
| `STORE-CAS1` | Delivered | `SessionStore::save_snapshot_cas` compare-and-swaps one complete generation against an expected content digest; memory and file adapters advertise `aggregate_cas`, and a mismatch returns `Ok(false)` without writing | Focused store tests prove match→write and mismatch→no-write for both adapters under one store lock / map write |
| `STORE-LEASE1` | Delivered | `SessionStore::acquire_writer_lease` publishes a store-wide durable epoch; `FileSessionStore` fences snapshot commits so a stale holder fails closed after takeover and advertises `lease_fencing` | Focused store tests prove acquire bumps epoch, takeover rejects the prior holder, and the new holder can still commit |
| `STORE-GC1` | Delivered | `ArtifactStore` retains host-supplied URI roots so limit eviction cannot drop reachable content; `gc_unreferenced` removes only unpinned objects | Unit tests prove pinned URIs survive count eviction and that GC keeps exactly the retained root set |
| `STORE-GC2` | Delivered | Session snapshots and artifact manifests persist retention roots; memory and file adapters round-trip them through `SessionStore` and advertise `reference_aware_artifact_gc` | Focused store tests prove reload keeps pins and GC still drops only unreferenced objects |
| `STORE-WATCH1` | Delivered | `SessionStore::watch_commits` notifies subscribers after a durable snapshot commit; memory and file adapters publish digest-only `SessionStoreCommitEventV1` and advertise `watch` | Focused store tests prove a subscriber receives the committed session id and digest for both adapters |
| `STORE-ENCRYPT1` | Delivered | `FileSessionStore::with_encryption_key` seals durable JSON documents and artifact manifests with AES-256-GCM; wrong keys fail closed; digest-only WAL stays unencrypted; the adapter advertises `encrypted_at_rest` only when constructed with a key | Focused store tests prove on-disk bytes are sealed, round-trip decrypt works, and a mismatched key cannot load |

KRN-6 session-store capability negotiation is complete for the built-in
adapters; Cloud/Harbor/host gates remain outside this slice.
### 3.3.10 Tool result trust boundary

The KRN-5 trust slice is delivered at the value boundary. `ToolResultTrustV1`
labels every tool result and tool output as `trusted` (host-produced or
host-verified content), `workspace_data` (local governed-tool output; the
wire default for compatibility), or `external` (content that crossed a web,
MCP, or download boundary — web search, web fetch, download, and MCP
projection label their outputs at construction). The label travels with the
value through `ToolOutput -> ToolResult` conversion and the SDK wire
contract, and `may_instruct()` / `requires_redaction_review()` give adapters
one predicate instead of re-deriving trust from tool names: only `trusted`
content may occupy an instruction-adjacent position, and `external` content
requires redaction review before prompt use. The governed
invocation state machine, admission quotas, and retry/deadline semantics
delivered by the merged line complete the Tool side of KRN-5; routing every
model call through one explicit middleware pipeline remains incremental
work on top of the existing admission/budget/cancellation/evidence stages.

| Gate | State | Code-owned outcome | Exit criteria |
| --- | --- | --- | --- |
| `OPT-MW1` | Delivered | Every run-bound completion and streaming call advances through one explicit middleware stage order: trust admission → budget → evidence → generation admission → provider → usage | Focused invoker tests prove unreviewed external tool results fail closed before provider use; streaming and non-streaming share the same trust gate |
| `OPT-TRUST1` | Delivered | `ToolResultTrustV1` is preserved through `NormalizedToolResult` into `ContentBlock::ToolResult`, host guards are labeled `Trusted`, and external results require `redaction_reviewed` before prompt use | Unit tests prove trust round-trip, middleware rejection of unreviewed external content, and that provider adapters still omit trust fields from vendor payloads |
| `OPT-OBS1` | Delivered | Session-shared secret-free middleware counters expose trust admit/reject, provider, usage, and trust-label cardinality via `model_middleware_health()` without retaining prompts or tool plaintext; Node/Python/Go SDKs project the same surface | Invoker tests prove reject/admit/provider/usage counters, streaming Done records usage, rebuilt loops share one observation Arc, and SDK alignment/fixtures cover the cross-language projection |
| `OPT-PATH1` | Delivered | Run-bound tool-context scoping never returns a raw provider when a client claims managed generation without a rebinding hook; the path wraps with `LlmInvoker` so trust/budget/evidence stay on the single middleware pipeline | Invoker test `managed_marker_without_rebind_still_enforces_trust_middleware` proves unreviewed External still fails trust admission and provider is not called |
| `OPT-TRUST2` | Delivered | `requires_redaction_review()` matches the middleware fail-closed gate (External only); WorkspaceData remains loadable without that gate; `may_instruct()` stays Trusted-only | Predicate + middleware unit tests agree; ROADMAP prose no longer claims all non-trusted content is middleware-enforced |
| `OPT-MCP1` | Delivered | When a parent Run already projected `McpBinding`s, delegated children register only those bindings; `McpManager.get_all_tools()` is starved so manager refresh cannot inject unbound MCP tools | Task test `projected_mcp_bindings_starve_manager_tool_registration` with Direct presentation proves `mcp__catalog__poison` is absent while projected lookup remains |
| `OPT-POOL1` | Delivered | Shared provider capacity for Task children requires a typed `ModelGenerationPool`; quota-only scheduler bindings without a pool do not publish product `pool_health`; detached/local loops remain local-only | Admission tests distinguish quota-without-pool vs typed pool; TaskExecutor without a pool skips provider admission |
| `SDK-CAP1` | Delivered (Skill slice) | Cross-language hosts apply serializable Skill batches through `apply_capability_batch` / `ApplyCapabilityBatch` (Node/Python/Go) backed by `SdkCapabilityBatchV1`; recovery bootstrap remains Rust-host-only until typed adapters exist | Core session test advances catalog gen 0→1 and name-conflicts; SDK fixtures + `sdk_api_alignment_check` cover the public surface |
| `SDK-CP1` | Delivered | Cross-language hosts install a live `SessionCheckpointExportSink` through `set_session_checkpoint_export_sink` / `setSessionCheckpointExportSink` / `SetSessionCheckpointExportSink` (Node/Python/Go) with `SdkSessionCheckpointExportV1` wire (`descriptor` + `contentBase64`); SessionOptions still omits the trait-object field | Core runtime setter captures the next boundary; SDK fixtures + alignment cover install/clear and secret-free wire fields |
| `SDK-IMM1` | Delivered | Cross-language hosts inject `ImmutableContentAdapter` through SessionOptions (`ImmutableContentAdapterOptions` / dict / Go options) with `SdkImmutableContentWriteRequestV1` put wire; CAR-02 host path is no longer Rust-only | Fixture + Go prepare/put tests; Node/Python SessionOptions conversion; `sdk_api_alignment_check` requires the SessionOptions field |
| `OPT-PERF1` | Standing (not triggered) | Open only when a release performance profile exceeds a [PERFORMANCE_QUALIFICATION](manual/PERFORMANCE_QUALIFICATION.md) budget; optimize the over-budget path only | Fresh local release `agent_convergence_benchmark` passed (4/4 cases; 9 LLM / 8 tool attempts / 6 executed / 235 tokens); capability ledger green |
| `OPT-CTX1` | Standing (not triggered) | Open only when long-session context assembly or recall exceeds retained budgets; reuse compaction/evidence, no second context store | Fresh local release `context_memory_benchmark` passed (`context-memory-corpus-v1`; context p95 76.9 ms vs 500 ms; recall p95 0.064 ms vs 250 ms) |
| `OPT-TOOL1` | Standing (not triggered) | Open only when hostile tool output still blows change-set / truncation ceilings; strengthen transform/bound and keep `.a3s-code` excluded | No over-budget signal from qualification or local profiles; harness exclude regression remains the standing guard |
| `OPT-HOSTINV1` | Delivered (Core slice) | Permission / confirmation / budget stay authoritative over prompt text: argument-scoped deny keeps tools model-visible but blocks execution; confirmation fails closed without a manager; budget accounting survives resume | Hermetic policy + MockLlm agent tests (`argument_scoped_write_deny_*`, `prompt_injected_write_cannot_cross_*`); `safety_gate` 12/12; release `agent_convergence` checkpoint-resume case |
| `PROMPT-ALIGN1` | Delivered | System prompts match real capability: primary Auto stays GeneralPurpose; explicit specialty styles get read-only repo contracts + hard permission checkers; default prompt stays under budget without dropping GP mutating-tool guidance | Hermetic prompt/style tests + live `test_prompt_capability_real_llm` (GP write succeeds; explicit Explore cannot write) |
| `HARNESS-CONV1` | Delivered | Product capability inventory is tiered (`sdk-capabilities/v2`): baseline coding harness vs advanced surfaces; `parallel_task` omitted from planning inventory and marked deprecated; durable-memory shadow mode deprecated in favor of Active recall | `sdk_capabilities` unit tests + `sdk_api_alignment_check`; docs (tasks/orchestration/memory/API) and CHANGELOG Unreleased |
| `HARNESS-CONV2` | Delivered | Node/Python/Go SDKs cfg-gate `s3` and link it through the `server` feature (product default still enables `server`) | Coding-only (`--no-default-features --features a3s-vec-fts`) and product/`server` `cargo check` green for Node, Python, and Go bridges |
| `HARNESS-CONV3` | Delivered | Shared `content_digest` (incl. `validate_digest`) decouples store/orchestration/Flow from evaluation; `evaluation` / `research` / `state-graph` / `dynamic-workflow` (and Flow projection) compile only behind `advanced-harness` (product `default` + `scientific`/`full`; SDK product default enables the same). `minimal`/`local-code` omit Advanced modules | `content_digest` + `sdk_capabilities` tests; `cargo check/test --lib` for `local-code`; product/`advanced-harness` SDK checks (Node/Python/Go) |
| `HARNESS-CONV4` | Delivered | Physically removed dual paths: model-visible `parallel_task` is no longer registered; Node/Python/Go SDK helpers removed; durable-memory `shadow` / `ShadowCandidates` constructors and mode are removed (`CAP-GA1` slice); `task` fan-out + `active_recall` remain the only supported paths; `ParallelTaskTool` / parallel-task params are `pub(crate)` only | Registry/SDK/docs/`sdk_api_alignment_check` updated; focused ParallelTaskTool unit tests may still construct the type inside the crate |
| `HARNESS-CONV5` | Delivered | Breaking thin library/SDK `default` ≈ `local-code` / a3s-vec only (no `advanced-harness`, no `server`, no default `headless-search`); SDK Moli helpers cfg-gated behind `headless-search`; `a3s` CLI pins `scientific`; Advanced/server/headless are explicit features; `a3s-flow` is behind `dynamic-workflow` | `cargo check -p a3s-code-core` (default) and coding-only SDK crates compile without Advanced/server/Moli/Flow; scientific/full and product profiles documented |
| `HARNESS-CONV6` | Delivered | Prompt ownership tightened: Core keeps `AgentStyle` → hard permission overlays and `SystemPromptSlots`; specialty markdown bodies documented as a replaceable default pack; hosts own rubric/reviewer prompts | Docs/API comments + hermetic/live `PROMPT-ALIGN1` gates |
| `HARNESS-CONV7` | Code prep Delivered / external In progress | Code wrap-up index [HARNESS_CONVERGENCE.md](manual/HARNESS_CONVERGENCE.md); runbooks for [TB-QUAL1](manual/TERMINAL_BENCH.md), [DM-PROD1](docs/evidence/evaluations/DURABLE_MEMORY_PRODUCTION_QUALIFICATION.md), [CAR checklist](manual/CLOUD_HARNESS_CONFORMANCE.md); DeepSeek/prompt live suites retained | Reproducible Harbor/host/Cloud·Box reports without secret leakage; paste links into ROADMAP when Delivered |
| `META-HARNESS1` | Delivered | Composable Meta Harness on one fact log: crates.io `a3s-effect` 0.1.0 stock parts + `HarnessGraph`; Core `SessionOptions.harness` / kernel admission; Node/Python stock `Harness.compose`; default remains `coding_actor` | Effect compose tests; `meta_harness` admission tests; SDK SessionOptions mapping; [META_HARNESS.md](manual/META_HARNESS.md) |
| `META-HARNESS2` | Delivered | Ordered `components: [...]` with `host:<id>` mounts; `HostHarnessRegistry` / `HostHarnessAssembler`; Node/Python `Harness.host` + `components`; F31 kernel coverage + Layer C `test_meta_harness_compose_live_e2e`; still non-bypassable kernel | `meta_harness` host-mount hermetic; SDK mapping; live compose write+attest; `f_table_coverage` kernels `meta_harness.rs` / `completion_attestor.rs` |

#### Harness convergence refuse list

Do not accept work that:

- ships a product reviewer system prompt or scientific scoring rubric inside Core;
- restores `serve` / `s3` / `advanced-harness` into library or SDK `default`;
- adds unified IDE approval chrome to Core to “match Cursor”;
- opens a second *imperative* orchestration loop or durable-memory dual path after
  `HARNESS-CONV4` (restoring `parallel_task` as a second engine, or a second
  message loop beside fact-log `ingest` / `resume`);
- bypasses Core permission projection or the completion gate from host compose;
- folds evaluation Gate semantics into the baseline coding agent loop (Advanced stays opt-in).

**Allowed:** Meta Harness multi-component trees on the single fact log
(`system` / `tools` / `budget` / `compact` / `infer` via `SessionOptions.harness`
or `a3s-effect` compose). See [META_HARNESS.md](manual/META_HARNESS.md).

First-principles packaging target: embedders that `cargo add a3s-code-core` receive a
coding harness; Advanced and server surfaces require explicit features. Scientific
discovery remains product composition (`scientific` / `full`), not a new runtime mode.


`may_instruct()` / `requires_redaction_review()` remain the single predicates
adapters use. Middleware fail-closes only on unreviewed
[`ToolResultTrustV1::External`]; workspace-produced data is model-visible but
not subject to that redaction-review gate. No second model-call path around
`LlmInvoker` is allowed (`OPT-PATH1`).

Optimization Phase 0 baseline is locked by the capability evidence ledger
(`scripts/check_capability_verification.py`) and the retained
[performance qualification](manual/PERFORMANCE_QUALIFICATION.md) budgets.
Phase 3 measurement (2026-09-07) re-checked the ledger plus fresh local
release `agent_convergence` and `context_memory` profiles: all passed, so
`OPT-PERF1` / `OPT-CTX1` / `OPT-TOOL1` stay closed. Further optimization
work stays evidence-driven against those ceilings and must not introduce a
second model-call path around `LlmInvoker`.

Phase 4 product-shell work (unified approval UX, Skills/`AGENTS.md` discovery
conventions, default workspace-writable + network allowlist posture) remains
host/Desktop/CLI owned. Code's deliverables are `OPT-HOSTINV1` (prompts cannot
bypass permission, confirmation, or budget gates) and `PROMPT-ALIGN1` (prompt
text, model-visible tool contracts, and hard permission overlays stay aligned
for Auto vs explicit specialty styles).

### 3.4 Terminal-Bench reliability track

Terminal-Bench is a verifier-driven qualification boundary, not a second Code
runtime. The first-principles success condition is a monotonic chain:

```text
exact task bytes → admitted Run → bounded Tool effects → verifier evidence
→ one typed terminal result
```

The earlier diagnostic runs exposed five concrete failure classes: a text-only
`agent_end` with no useful workspace action, stream EOF after repeated retries,
timed-out commands whose descendants continued running, terminal provider
responses (notably billing/credential `4xx`) consuming retry budget, and
partial local runs being mistaken for a leaderboard score. The implementation
order below addresses those classes without introducing a benchmark-specific
executor or moving verifier authority into Core.

| Gate | State | Code-owned outcome | Exit criteria |
| --- | --- | --- | --- |
| `TB-BOUNDARY1` | Delivered | Harbor uploads the instruction byte-for-byte; the runner pins the writable General style, disables planning pre-analysis, and carries one monotonic run deadline into every command | Prompt, style, and deadline boundary tests pass; no task-specific prompt rewrite occurs |
| `TB-TERMINAL1` | Delivered | The runner persists one atomic `a3s.code.terminal-bench-result.v1` record and fails closed on stream EOF, worker failure, deadline, or missing successful Tool evidence | Result JSON and adapter metadata classify outcome/reason without stderr scraping |
| `TB-PROCESS1` | Delivered | Container commands use a bounded outer timeout and Unix process-group termination/reaping | A timed-out shell cannot leave descendants consuming the Harbor budget |
| `TB-PROVIDER1` | Delivered | OpenAI, Anthropic, and Codex login clients preserve non-retryable provider/status metadata; the runner reports `provider_rejected` with bounded diagnostics | 4xx terminal responses do not fall back or retry; provider/status metadata survives the adapter boundary |
| `TB-OUTPUT1` | Delivered | The container adapter reuses the Core bounded head/tail capture semantics instead of buffering unbounded `wait_with_output` data | Hostile high-volume stdout/stderr remains within the tool output ceiling while preserving truncation accounting |
| `TB-RETRY1` | Delivered | Core promotes retry exhaustion to a typed terminal Agent error, exposes bounded status/body diagnostics, makes streaming Provider backoff cancellation-aware, and carries an end-to-end call-count regression for inner exhaustion versus the Agent circuit breaker; the run deadline now bounds every provider attempt, retry backoff, and auto-compaction through per-attempt child cancellation scopes, and auto-compaction is transactional so a failed or timed-out summary cannot destroy tool evidence | The 429/5xx inner-budget, typed classification, cancellation-backoff, provider-timeout enforcement, run-deadline bounding, and compaction-evidence regressions are committed and green; `with_resilience_defaults` pins the enforced provider timeout |
| `TB-ACP1` | Code delivered; parity A/B and native retirement open | `a3s-code-acp --unattended` reuses the runner profile, host admission loop and result record; auto-approve without learning, fixed question reply, private state root; Harbor `A3SCodeAcpAgent` + `run_acp_tb.sh`; interactive ACP/TUI unchanged | [ACP lane evidence](../../docs/evidence/code-harness/20261014-acp-unattended-tb/OUTCOME.md); no live Harbor run, score or parity claim |
| `TB-ACP2` | Code delivered; live Harbor run open | Unattended ACP runs with every capability on: `a3s-vec` lexical FTS, Code Intelligence (new Python basedpyright/pyright and C/C++ clangd profiles), file memory, skills, web tools, automatic and manual delegation; derived indexes move to the private state root; interactive ACP/TUI sessions gain Code Intelligence; Harbor adapter uploads a host-prepared language-server bundle (`prepare_lsp_bundle.sh`, about 3 s per trial, no network) and falls back to a best-effort network install inside the setup deadline | [Full-capability evidence](../../docs/evidence/code-harness/20261014-acp-full-capability/OUTCOME.md); no Harbor run, no score, no semantic vectors |
| `TB-ACP3` | Delivered for one live task; run deadline enforced | `a3s-code-acp` advertises the model as an ACP session config option (`set_session_config_option`), so Harbor's runner on Python ACP 0.12 selects it; a host-prepared runner bundle (`prepare_runner_bundle.sh`) replaces Harbor's per-trial apt/pip install, cutting agent setup from 225–360+ s to about 6 s; live TB 4.0 `html-js-filter` with `boyue/glm-5.3` scored 1.0; each host continuation now runs only for what remains of the run budget (the 3356 s overrun of a 2400 s budget stopped at 2400 s on rerun); `--reasoning-effort` and a 120 s stream idle bound for TB runs | [GLM trial evidence](../../docs/evidence/code-harness/20261014-acp-glm-tb-trial/OUTCOME.md); one task, k=1; GLM 5.3 at `max` effort varies from 78 tool calls in 56 min to 8 in 40 min |
| `TB-ACP4` | Delivered; full GLM 5.3 run stopped partway | Four harness defects found in the full TB 4.0 run are fixed in Core: a text-only model gets a note instead of image parts (`modalities.input`); a non-git tree of up to 262144 entries is a stat-only index, so a large tree no longer leaves every write unresolved; a missing, withheld or model-disabled skill is refused at admission instead of becoming an unknown outcome; changed paths share a 1 GiB hash budget, so a data file over 16 MiB gets byte identity. Each has a reproducing regression test. No Terminal Bench task, test or verifier changed | [GLM trial evidence](../../docs/evidence/code-harness/20261014-acp-glm-tb-trial/OUTCOME.md); run stopped at user request with 1 passed task; no full-run score |
| `TB-ACP5` | Code delivered; live arm comparison (T039) open | `--engineering-policy NAME` and `--goal-continuation` trial arms for unattended ACP (Harbor env `A3S_CODE_ACP_ENGINEERING_POLICY` / `A3S_CODE_ACP_GOAL_CONTINUATION`); the goal arm runs the first attempt as the admitted one-step goal slice; the `ultracode` guideline names the registered `program` tool instead of the unregistered `dynamic_workflow` | [ACP trial arms evidence](../../docs/evidence/code-harness/20261015-acp-trial-arms/OUTCOME.md); T039 four-arm Harbor run |
| `CTX-ART1` | Delivered | `read` and `search` (grep) page retained `a3s://tool-output/...` artifacts; the truncation marker names how; a pruned result points at its artifact instead of asking for a re-run | [Artifact paging evidence](../../docs/evidence/code-harness/20261015-tool-output-artifact-paging/OUTCOME.md) |
| `CTX-BOUND1` | Delivered | The default tool-result policy is `context_bounded`: 16 KB, exact head 10582 B and tail 5290 B, no rewriting; the original stays a pageable artifact. `read` and artifact pages end inside the budget with a continuation offset. Snapshots without the field resolve to `conservative` | [Context-bounded default evidence](../../docs/evidence/code-harness/20261015-context-bounded-default/OUTCOME.md); threshold from 4793 Terminal-Bench tool results |
| `LOOP-FB1` | Delivered | A fact-run call whose name, arguments and text output match the previous such call gets a non-blocking host note with the streak count (JSON results unchanged); the unbound-mutation gate names a `bash` `test -f PATH` check that binds its digest, only when the ledger recorded that file's bytes | [Repeat and gate feedback evidence](../../docs/evidence/code-harness/20261015-repeat-and-gate-feedback/OUTCOME.md) |
| `CTX-WEB1` | Delivered | A multi-page `web_fetch` keeps the converted page as a content-addressed `a3s://tool-output/web_fetch/...` artifact named in the output and metadata; `read` and `search` grep it without refetching; each page ends inside the result budget on a char boundary. An adapter-held session (no local store) keeps offset paging only | [Fetched page artifact evidence](../../docs/evidence/code-harness/20261015-web-fetch-artifact/OUTCOME.md) |
| `SEARCH-SKIP1` | Delivered | The local and manifest grep backends report files over the 16 MiB text-file limit in `WorkspaceGrepOutcome.skipped_oversize`; `grep`/`search` name them in every output mode and in metadata instead of a bare "No matches found". Unreadable and binary files stay silent skips | [Search size-skip evidence](../../docs/evidence/code-harness/20261015-search-size-skips/OUTCOME.md) |
| `CTX-VIEW1` | Delivered | `AgentSession::context_report()` folds the session fact log into tool-result bytes, the five heaviest results and repeats of an earlier tool+arguments+text, beside recorded provider tokens and cache-read share. `/cost` prints it and no longer reports 0 tokens | [Session context view evidence](../../docs/evidence/code-harness/20261015-session-context-view/OUTCOME.md) |
| `ACP-WIRE1` | Delivered | ACP bash `raw_output` sends `output`/`output_delta` as strings instead of one JSON number per byte; the vendored pager's `BashOutput` decodes either a byte array or a string, so other producers keep working | [ACP raw_output evidence](../../docs/evidence/code-harness/20261015-acp-raw-output-text/OUTCOME.md) |
| `ISSUE-SWEEP1` | Delivered | Issue sweep: explicit stock `Harness.compose` keeps token compaction (#191); compaction can fire again within one user turn after a model call (A3S-Lab/CLI#159); `planning_mode = auto` plans on `send`/`stream` from one pre-analysis call (#190). The `v912_seam` mock now serves every request instead of the first | [Issue sweep evidence](../../docs/evidence/code-harness/20261015-issue-sweep/OUTCOME.md) |
| `CTX-E2E1` | Delivered | Real `boyue/deepseek-v4-flash` run of the 9.1.3 context harness: core scenarios on macOS (`test_context_harness_real_llm`, 3/3) and ACP on Linux (gate next step, size skip, bash artifact, repeat note, `/cost`, text `raw_output`, `ultracode`). The run found and fixed a `web_fetch` page that overflowed its budget by its own footer and named two artifacts. Open findings: native-effort models never receive the effort guideline, and an interactive ACP gate refusal reaches the client, not the model | [Real-LLM e2e evidence](../../docs/evidence/code-harness/20261015-real-llm-e2e/OUTCOME.md) |
| `TB-QUAL1` | In progress | Diagnostic and full-tagged Harbor procedure documented in [TERMINAL_BENCH.md](manual/TERMINAL_BENCH.md); runner gates TB-BOUNDARY1…TB-RETRY1 remain Delivered; complete tagged dataset with native verifier retention still required | No missing verifier result, runner error, or local-only aggregate is counted as a pass or leaderboard score |

The dependency order is `TB-BOUNDARY1 → TB-TERMINAL1 → TB-PROCESS1 →
TB-PROVIDER1 → TB-OUTPUT1 → TB-RETRY1 → TB-QUAL1`. The Harbor environment owns task images,
timeouts, artifacts, and verification; Code owns only the session loop and
its typed evidence record. A benchmark adapter must not add a second retry
loop, tool executor, or success definition.
The first reviewer-composition mechanism is now Code-owned: a
`ResearchReviewFindingV1` can bind the exact immutable `EvaluationRecordV1`
that produced it. The binding checks evaluator identity, parent Run identity,
and inclusion of the evaluator's evidence snapshot digest, while preserving an
`open` finding status until the host applies its rubric and an explicit human or
policy resolution. Existing unbound v1 findings remain readable with their
legacy digest identity; new bindings use a digest that includes the evaluator
record, so result replacement cannot masquerade as the same finding.

`ResearchReviewBatchV1` is the bounded publication boundary for one evaluator
response. It requires one project/Run/evaluation-record/evidence identity for
all findings, canonical finding ordering, and an updated batch digest after an
explicit resolution or waiver. It remains a validation primitive: reviewer
rubrics, severity thresholds, approval, retention, and publication stay with
the host or Desktop.

Review findings and batches expose bounded `from_slice`/`to_vec` helpers for
process boundaries. These helpers reject oversized JSON and validate the full
nested digest tree before a host can consume or publish a value; direct
`serde` remains available for compatibility, but is not the admission path.

Findings may additionally carry a provenance-receipt digest. The optional
binding is backward compatible with legacy findings, but once present it is
included in the finding digest and cannot be replaced without producing a new
finding identity. This makes a reviewer observation address both the exact
artifact and the reproducibility receipt used to produce it.

When the admitted `ResearchRunV1` is available, hosts should use
`bind_provenance_receipt_for_run`. In addition to the artifact, project, Run,
and evidence checks, this verifies that the receipt's project revision,
provider, and random seed are the values admitted for that Run. The
compatibility method
`bind_provenance_receipt` intentionally keeps its original object-only
semantics for older integrations.

### 3.3.1 Workflow result convergence

Resumable orchestration now treats a workflow checkpoint as a side-effect
boundary. Successful steps persist a bounded `ExecutionResultReceiptV1` whose
identity includes the workflow, task specification, prompt, parent session, and
output schema; changing a cached step or corrupting its receipt prevents a
resume from re-running ambiguous work. Checkpoints without the optional receipt
field remain readable for migration compatibility. Flow decision dispatch uses
the same identity-aware claim, renewal, completion, and release boundary and
stores a digest-only receipt for an accepted decision. The result receipt is a
mechanism for replay and fencing; source evidence, review policy, retention, and
publication decisions remain host responsibilities.

### 3.3.2 Scheduler and plan convergence

Dynamic Flow is now an adapter over the same `ExecutionPlan` used by Code
planning rather than a second progress model. A complete Flow history can be
projected into that plan on every inspection and when a resumed observer is
attached; duplicate lifecycle delivery preserves insertion order and cannot
regress a terminal status. The plan's definition identity excludes mutable
status, so retries and restarts retain one stable execution intent while the
visible progress changes.

Each Flow step crosses a cancellation-aware, bounded local admission gate. Its
identity is domain-separated from delegated Agent steps and contains only the
run/step/handler tuple plus the bounded input derivation; scheduler traces and
leases never retain input or output plaintext. Inputs at or below 64 KiB are
mixed inline; larger inputs (hard ceiling 512 KiB) fold to `sha256` + `bytes`
before derivation so host workflows such as DeepResearch multi-source selector
shards remain identity-admissible without raising the plaintext retention
budget. A standalone Flow adapter may layer the agent-wide priority/FIFO
scheduler on direct script-backed steps. Normal Session calls keep the enclosing
scheduler lease and let host `task` fan-out use its own child admission, which
prevents a single-slot scheduler from deadlocking on a nested lease. Delegated
tasks now pass their canonical step identity through that same global scheduler
boundary.

The follow-up mixed-generation continuation qualification is recorded in
sections 3.3.3 and 3.3.4. Cloud/Use package ownership, fairness policy, and
business-level retry decisions remain outside Code.

### 3.3.3 Mixed-generation continuation qualification

Dynamic workflow runs now pin a Code runtime build in the durable Flow
`WorkflowSpec`. The default compatibility window accepts legacy unpinned runs
while requiring the current build for new runs; hosts can provide an explicit
`RuntimeBuildCompatibility` set when they retain an older worker. A continuation
identity is reconstructed from the persisted Run/step definitions, source
hash, initial-input digest, runtime build, and canonical plan identity. It
excludes progress, retry events, sequence numbers, and outputs, so the same
continuation identity survives a restart and a completed replay without a
second journal.

Before a resumed run reaches Flow execution, Code validates run ownership,
sequence monotonicity, source/input equality, step identity consistency, and
runtime-build syntax. Flow's own immutable start check and build compatibility
gate then reject changed generations before any step body can run. The local
qualification fixture proves a terminal retry replay does not repeat its
side-effecting step and a worker on a different build cannot resume an active
run. Legacy unpinned histories remain readable only inside the explicit
migration window.

### 3.3.4 Parent cancellation and worker takeover

The non-terminal continuation boundary now carries a stable claim identity
through worker admission. The identity is derived only from the run id, source
hash, initial-input digest, and effective runtime build; evolving plan progress,
retry counters, and outputs cannot create a second claim. Local workspaces use
an atomic file-backed lease sidecar at `.a3s/workflow/leases`, remote hosts may
inject any `FlowDecisionLedger`, and in-memory hosts retain one tool-scoped
ledger. Neither path stores workflow source, input, output, or owner secrets.

An admitted worker renews its lease during Flow replay and inline retry waits,
and the runtime revalidates ownership immediately before each workflow or step
body. A stale owner therefore cannot start new work or complete a claim after a
takeover. Parent cancellation propagates to a child `ToolContext`; the worker
waits a bounded settlement window, releases only after the execution future has
stopped, and leaves an unsettled lease fenced until it expires. Completion uses
the same identity/owner/lease checks as Flow decision dispatch. Qualification
now covers cross-worker busy claims, expired-owner fencing, cancellation at a
retry boundary, pre-admission lease loss, generated run ids, and digest-only
sidecar records.

The event-sourced Flow history remains the workflow authority. Exactly-once
external effects still require the invoked Tool or host to provide its own
idempotency/receipt contract; Code does not pretend that a lease can undo an
effect that was committed outside the Flow journal. Cloud/Use ownership,
retention, and business retry policy remain outside Code.

### 3.3.5 Host control and cross-process recovery

`DynamicWorkflowTool::control` is the single host-facing boundary for dynamic
workflow inspection and mutation. It binds the caller's exact source, input,
runtime policy, registry, and workspace store before any operation runs. The
bounded `DynamicWorkflowControlSnapshot` exposes status, sequence, step
counts, cancellation presence, continuation/plan identities, runtime build,
and a redacted worker-lease state; it never exposes source, input, step
arguments, outputs, or owner tokens. `history()` is separate and explicitly
trusted because it returns those durable values.

`drive`, `request_cancellation`, and `force_cancel` first claim the same
identity-bound lease used by the model-visible Tool. A live worker is reported
as busy instead of being interrupted through a second control plane. Once a
claim is owned, Flow appends or replays the authoritative cancellation and
terminal events, while a bounded heartbeat/settlement loop renews and then
completes or releases the claim. An unsettled future remains fenced until
lease expiry. Local JSONL history is wrapped by
`CrossProcessFlowEventStore`, which serializes appends and reads across
processes without projecting a second state machine. The qualification test
spawns independent workers, kills an owner, waits for takeover, settles a
cleanup-aware cancellation, and forces optimistic event conflicts so Flow's
existing bounded retries are exercised. Remote or database-backed hosts can
inject one typed `FlowEventStore` through `with_flow_event_store`; the same
store is then shared by model-visible execution and the control handle, so a
non-local host does not silently fall back to a fresh in-memory journal.

### 3.3.6 Scheduler fairness and bounded control observability

The agent-wide admission scheduler now has a separate health projection from
its established occupancy API. The projection is actor-owned and bounded: it
records admissions, releases, queued cancellations/rejections, aging
promotions, peak occupancy, and aggregate wait time, while retaining no labels,
execution identities, or queue payloads. Aging updates the effective queue key
only when a request crosses a priority level, so repeated diagnostics cannot
inflate promotion counts or alter FIFO order within an aged class.

`DynamicWorkflowTool` shares a process-local, digest-free metrics block with
its control handles. It counts claim attempts, live-claim contention,
identity conflicts, lease renewals/loss, terminal settlement, and takeover
attempts; the durable ledger still owns the per-run attempt number and fencing
decision. `DynamicWorkflowControl::diagnostics` composes these counters with
the optional scheduler health snapshot as a read-only host view. It never
creates a scheduler, event journal, or workflow state store of its own.

The qualification matrix includes a resumed background workflow competing with
a stream of newer interactive admissions. After the configured aging bound,
the resumed step is admitted before those newer requests, and all counters
settle with zero in-flight work. The cross-language Agent/Session surfaces
expose the scheduler health projection through the existing typed bridge
operations; legacy occupancy calls retain their original wire shape.

### 3.3.7 Per-run quota and admission identity qualification

The shared scheduler now has one optional owner-quota projection layered inside
its existing actor. `TaskSchedulerQuota` validates a bounded descriptor and can
derive a domain-separated digest from a run/host scope; the scheduler retains
only that identity plus active/pending counters. `acquire_with_quota` applies
the owner limit while preserving the established global priority/FIFO and
aging policy. A quota-blocked owner is skipped when another owner is eligible,
so unused global capacity is not stranded behind one fan-out. Quota state is
pruned after the last pending or active request, while `TaskScheduler::health`
continues to provide the cumulative process view.

Dynamic workflow runtimes use the stable continuation claim identity for direct
globally admitted Flow steps. Detached background Task calls carry the exact
invocation run identity through `ToolContext` and derive a separate
run/session-scoped quota; this keeps nested child fan-out from reacquiring an
outer Session lease and preserves the max-active=1 deadlock boundary. The
workflow-local step gate and child-run quota remain explicit resource
boundaries, and neither creates a second Flow event store, worker lease, or
scheduler.

Qualification covers two owners competing for global capacity while one is at
its limit, queued-owner cancellation, immutable identity/limit conflicts,
malformed and overlong scopes, idle-state pruning, dynamic-workflow diagnostic
projection, and detached children sharing one run quota before using free
capacity. No scope text, prompt, Tool payload, or workflow output is retained
by the scheduler.

### 3.3.8 Provider-aware model-generation admission

Model generation is now a resource boundary rather than an incidental property
of one `LlmClient` instance. A provider adapter may publish a
`ModelGenerationPool` whose identity is derived from the provider, model,
endpoint origin, and optional non-secret account scope. URL credentials,
paths, queries, fragments, prompts, outputs, and transport headers are never
retained in that identity. Built-in Anthropic, OpenAI-compatible, Zhipu, and
Codex clients publish the descriptor; custom clients remain conservative and
single-flight unless they opt into the typed contract.

Session construction binds the pool descriptor to the existing agent scheduler
as a second quota dimension. The session's normal Run/Tool admission still
consumes one global slot, while each actual provider generation acquires a
quota-only lease from the same actor and priority queue. This preserves one
queue and one cancellation/release authority, lets independent providers use
free capacity, and prevents a nested model call from deadlocking when the
global scheduler is configured with `max_active = 1`. Local client capacity
and the shared pool capacity are both enforced; a nested workflow gate copies
the provider binding rather than bypassing it.

The `LlmInvoker` boundary covers blocking, streaming, structured, and
streaming-structured calls. A streaming lease is owned by its supervised proxy
until EOF, terminal `Done`, cancellation, or receiver drop. Structured repair
rounds release each call's lease before the next round, and an outer
preadmitted permit is transferred to the first call exactly once. Foreground
delegated children and Flow steps carry the provider quota into their own
admission; background/global children retain the outer provider reservation
and use a local gate to avoid recursive acquisition. Queue wait is reported in
the existing structured-generation metadata without exposing labels or
payloads.

Qualification covers two sessions sharing one provider pool, independent
provider progress under a full global budget, atomic multi-quota admission,
quota-only cancellation and release, managed stream lifetime, nested tighter
workflow limits, and the complete 3,205-test Core library suite. This slice
does not add provider rate-limit policy, billing, or a second scheduler;
Gateway and host policy remain the owners of those concerns.

The admission-hardening follow-up landed in Code `386d75b1` and now adds
cross-host consumption plus noisy-neighbor qualification. Node.js, Python, and
Go sessions expose the same read-only `ModelGenerationPoolHealthSnapshot`; the
versioned Go bridge advertises `session_model_generation_pool_health`, and the
capability catalog carries the operation for discovery. A repeated eight-cycle
qualification with twelve blocked waiters per cycle proves an independent
provider pool continues to admit work under a full global budget while
cancellation, release, and bounded retention counters settle. No provider
label, endpoint credential, prompt, output, or task label is added to scheduler
state.

**P3/KRN-8 host qualification evidence is delivered.** The versioned
`sdk/evaluation/model-generation-pool-health-v1.json` fixture is consumed by
the public Node.js, Python, and Go Session surfaces. Each adapter checks the
same digest-only identity, local reservation conservation, shared/local
capacity bounds, bounded aggregate field set, and recursive redaction list;
the Go adapter additionally exercises the real Rust JSONL bridge when
`A3S_CODE_GO_BRIDGE_TEST_BINARY` is configured. The default fixture uses an
unreachable endpoint and sentinel credential, so it makes no provider request
and cannot turn a health read into a billing operation. Core's existing
admission tests remain the authority for active, cancelled, released, and
retained scheduler epochs. No second scheduler or metrics store was added;
Flow remains the sole lease authority and Gateway/hosts own rate limits and
billing.

### 3.4 Scoped capability program

The [Scoped Capability Architecture](manual/SCOPED_CAPABILITY_ARCHITECTURE.md)
adopts Cordis-style context, fiber, and reversible-effect lifecycle semantics
through Rust ownership, immutable snapshots, typed scopes, and structured
concurrency. It does not turn Code Core into a package manager or general
dependency-injection framework. A3S Use remains authoritative for package
graphs, verification, Grants, lifecycle generations, atomic capability
cutover, and crash recovery. Code projects each exact Use generation into
Session- and Run-owned capability scopes.

| Gate | State | Code-owned outcome | Exit criteria |
| --- | --- | --- | --- |
| `CAP-FND1` | Delivered | Accepted ownership, lifetime, identity, failure, verification, and migration contract | The contract and Roadmap are mechanically aligned; existing lifecycle and concurrency evidence is recorded and green |
| `USE-BRIDGE1` | Delivered | Use `6ed0b4e` publishes `a3s.use.extension-snapshot-cursor.v1`, `a3s.use.capability-snapshot-cursor.v1`, and a non-clone atomic exact-generation snapshot lease | Full Use tests and strict Clippy pass; acquisition is all-or-nothing and rejects hidden, mixed, contended, stale, unleasable, or digest-mismatched generations without changing capability snapshot JSON v2 |
| `CAP-SET1` | Delivered | Typed Use package/cursor and Code catalog generations, sealed source classes, complete source-owned descriptor batches, and a bounded immutable `CapabilitySet` | `BTreeMap` ordering plus a domain-separated golden digest is insertion-order independent; mixed Use cursors, conflicts, missing edges, forged Built-in precedence, and every configured bound fail before an `Arc` can escape |
| `CAP-SCOPE1` | Delivered | Session/Run/Turn/Subtask markers, catalog-bound ceilings, borrowed leases, reversible effects, exact Use Run leases, and a structured-concurrency supervisor | Compile-fail and runtime tests prevent lease escape or child expansion; close is reverse-order, cancellation-safe, idempotent, bounded, and releases the Use lease last |
| `CAP-COMP1` | Delivered | Real Agent execution composes orchestration/provider/Tool Turns, recursive Skill/Task Subtasks, Turn-supervised bridges/effects, and Run-supervised background work under one downward-only cancellation tree | Runtime tests prove orchestration and Tool Turns, stream/effect settlement, `Run -> Turn -> Subtask -> Turn` recursion, fail-closed stale promotion, and Run-owned Task/memory settlement before lease release |
| `CAP-PROJ1` | Delivered | Closed typed runtime values, immutable projected catalogs, typestate contribution transactions, generation/digest CAS publication, and final-lease retirement | Failed prepare, validation, cancellation, dropped transaction, and commit-race paths leave the current generation unchanged and retain every prepared effect for reverse cleanup |
| `CAP-DEP1` | Delivered | Bounded surface readiness DAG | Only published surface edges are ordered; Code does not resolve packages or become general DI |
| `HOST-CAP1` | Delivered | Core exposes one atomic projection contract; the resident CLI publishes one exact Use managed MCP/Skill/reviewed Runtime Tool/Knowledge Surface/dependency-closed Flow/UI generation, while scoped CLI/Desktop execution publishes its bounded managed-MCP/Skill/UI cut | Old Runs and host handles retain N and its exact Use lease, new admissions see N+1, every new Run/checkpoint records its catalog and ceiling identity, recovery rejects N/N+1 drift or bootstraps one exact historical batch on a fresh Session, reviewed Runtime Tools, managed MCP, and immutable OKF readiness never require compatibility double writes, Flow preflight or lock-wait cancellation never advances the generation, one-shot watchers stop before Run admission, scoped HTTP Runtime/Gateway composition is lazy and Session-owned, and Desktop requires exact Code/Use evidence |
| `CAP-PROFILE1` | Delivered | Run-frozen typed Tool presentation over the same pinned executor values | Permission filtering precedes Profile projection; name/schema identity and deterministic order are preserved, code mode rephrases only the existing `program` definition, child runs cannot broaden, and exact Session resume plus Rust/Node.js/Python/Go parity pass |
| `HOST-AGENT1` | Delivered | Core projects Agent definitions into one Run-frozen registry shared by automatic and Tool-driven delegation | Canonical alias conflicts fail before publication, compatibility registration cannot shadow a published Agent, N Runs delegate through N after an N+1 cutover, and the exact N Use lease remains held through foreground child completion |
| `HOST-COMMAND1` | Delivered | Core dispatches blocking and streaming slash Commands through one Run-frozen registry | Built-in and compatibility conflicts fail before publication, legacy registration cannot shadow a published Command, N execution remains on N after an N+1 cutover, and the exact N Use lease remains held through Command execution |
| `HOST-HOOK1` | Delivered | Core composes projected Hook bindings through one Run-frozen executor | Definition/handler pairs remain generation-exact, invalid Run event scopes and compatibility conflicts fail before publication, external `Skip` cannot bypass projected policy, and supervised observations retain the exact Use lease through bounded settlement |
| `HOST-MCP1` | Delivered | Core projects each MCP server as one immutable exact-client binding and freezes its wrappers per Run | Initialization and `tools/list` finish before publication; N definitions, raw calls, foreground delegated children, and the parent Run's N Use lease remain generation-exact across N+1; rollback and final-reader retirement close the Code-owned connection effect without mutable-manager fallback |
| `HOST-CONTEXT1` | Delivered | Core projects general `ContextProvider` values into each Run-frozen Agent configuration through the same atomic batch | N Runs retain N providers and the exact N Use lease across N+1; descriptor/provider names and Session-static names cannot conflict; cognitive package bindings remain on the separately persisted Knowledge/session boundary; delegated children keep isolated prompt context |
| `HOST-FLOW1` | Delivered | With `dynamic-workflow`, Core projects each named `FlowBinding` as one exact `WorkflowSpec` plus `FlowEngine` and exposes it through a non-clone host handle | Spec/engine runtime-build incompatibility and descriptor/spec name drift fail before publication; an N handle retains N's definition, engine/store, and exact Use lease across N+1; missing lookup acquires no lease; Session close cancels active replay and explicit close releases the lease; thin/default builds do not link `a3s-flow` |
| `HOST-KNOWLEDGE1` | Delivered | Core projects multiple digest-bound, non-queryable Knowledge Surface readiness values while admitting at most one separately selected cognitive Knowledge authority into each Run | Same-source Flow dependencies close only against exact surface evidence; surface values never become cognitive context; N Runs retain N provider, binding, readiness set, and exact Use lease across N+1; each Run snapshot records its own cognitive binding while the Session snapshot records the next-Run binding; resume requires an exact bootstrap; multiple or ambient general Context authorities fail before publication |
| `HOST-UI1` | Delivered | Core projects bounded, path-free `UiBinding` values and exposes them through a non-clone host handle | Reviewed entry, style, and script bytes plus their content digests are frozen before publication; descriptor/binding name or digest drift and dependencies outside Tool, Skill, MCP, and Flow fail closed; an N handle retains N's exact document and Use lease across N+1; missing lookup acquires no lease; Session close cancels active host use; renderer policy remains host-owned |
| `CODE-APOFASI1` | Delivered | Opt-in Apofasi System One substrate (`TypedDecisionEngine`, `TypedDecisionService`, `decide_and_gate`, digest `TypedDecisionReceiptV1`, `GatePolicy`, Advanced `typed_decisions` inventory, `CodeError::TypedDecision`) behind Cargo features `apofasi` / `apofasi-infer` / `apofasi-metal`. When enabled, Code refuses to replace planning pre-analysis and goal achievement | Thin `local-code` builds do not link Apofasi; empty requests fail closed; lexical path needs no weights; neural path stays feature-gated; the system prompt is unchanged; a typed answer cannot skip a generation that returns more than one field; call sites refuse that skip; Code does not add a keyword classifier and does not lower `GatePolicy`; the escalate prompt includes the task state; extra call sites stay host-owned; no Use-projected `CapabilityKind` yet (`HOST-APOFASI` remains future work) |
| `CAP-GA1` | Delivered | Legacy shadow ownership and piecemeal reconciliation removed; durable-memory bindings are Active-recall only (`HARNESS-CONV4`) | Official hosts and SDKs use the scoped architecture and Active recall; shadow mode constructors/mode are gone |

[`USE-BRIDGE1`](https://github.com/A3S-Lab/Use/commit/6ed0b4e) is the upstream
admission boundary, `CAP-SET1` freezes its normalized identity plane, and
`CAP-SCOPE1` provides lifetimes, ceilings, exact-generation leases, and
supervised effects. `CAP-COMP1` makes those scopes operational: orchestration
calls and each real provider/Tool iteration own Turns, delegated Skill/Task
Agents recursively compose Subtask/Turn scopes, Tool stream bridges settle with
their Turn, and promoted background Task/memory work remains Run-supervised
until settlement. `CAP-PROJ1` now pairs that set with closed runtime values
and publishes one complete Code generation through a typestate transaction.
`CAP-DEP1` now derives deterministic dependency-first waves from one immutable
surface set, rejects cycles and incomplete adapter batches before preparation,
and retains the exact Use cursor without inspecting package manifests or
resolving packages. Delivered `HOST-CAP1` admits managed MCP, Skill, reviewed
Runtime Tool, immutable Knowledge Surface, dependency-closed Flow, and UI
projections as one resident Session batch, pins the exact projected values and
a real Use snapshot lease per Run, and drains retired effects at Session close.
The CLI also uses a bounded managed-MCP/Skill/UI cut of that path for a
short-lived Code Exec host that
quiesces Use discovery before Run admission. Ordinary Code Exec performs only
installed-component discovery; Desktop negotiates required `scoped-v1`
support and rejects success without canonical Code catalog and Use cursor
evidence. Delivered `HOST-AGENT1` extends the same atomic batch to Agent
definitions in Core. Each Run merges projected Agents into an independent
compatibility snapshot, binds both automatic selection and `task` delegation
to it, and continues to retain the exact A3S Use generation lease. Delivered
`HOST-COMMAND1` puts both blocking and streaming slash-command dispatch inside
that admission boundary, using a Run-frozen Command registry and retaining the
same exact Use lease through execution. Delivered `HOST-HOOK1` atomically pairs
each projected Hook definition with its exact handler, composes the frozen
projection after any Session-static external executor, and supervises detached
observations and timeout settlement under the capability Run. Session and
Skill lifecycle events remain outside projected Run scope. Delivered
`HOST-MCP1` adds a Core-owned asynchronous MCP projection boundary: one
`McpBinding` freezes an exact initialized client with its canonical tool
definitions, projected wrappers call that client directly, and delegated
children inherit the same binding. Code connection effects retire only after
the final old projection reader, while every executing Run separately retains
the exact non-clone A3S Use snapshot lease. A trusted host must construct the
adapter input from already selected Use Runtime/Gateway evidence; Code does
not inspect package files, resolve opaque `gateway:*` identities, choose a
    provider, or fall back through a mutable server name. A3S Use and the
    official CLI now complete that package-to-Core MCP projection; adoption by
    the remaining official hosts stays a separate integration boundary.
    Delivered `HOST-CONTEXT1` now freezes general Context providers in that same
Run generation without treating them as persisted cognitive authority.
Delivered `HOST-FLOW1` now binds a named, runtime-build-compatible A3S Flow
engine and durable spec to an exact host execution handle without moving Flow
store, replay, runtime, or observation ownership into Code. Delivered
`HOST-KNOWLEDGE1` now freezes multiple path-free, digest-bound Knowledge Surface
readiness values beside at most one separately selected cognitive provider and
binding per Run. Same-source Flow edges may close against those non-queryable
surface values, but they never become Agent context or select a package. Old
generations remain pinned across cutover, Run-local cognitive binding evidence
is preserved, and the binding visible to the next Run persists. Resume uses
that binding as an exact recovery seed before later generations may advance;
the Knowledge host continues to own OKF indexing, retrieval, retention, and
query leases.
Delivered `HOST-UI1` now freezes bounded, path-free HTML, CSS, and JavaScript
bytes with canonical content and surface digests, validates only explicit
Tool, Skill, MCP, and Flow readiness edges, and exposes the exact generation
through a non-clone host handle. Core does not render the document or own
origin, CSP, navigation, state, credential, or backend-routing policy.
A3S Use now publishes versioned, complete canonical UI dependency and managed
MCP evidence, and the resident CLI revalidates it before staging eligible
managed MCP, Skill, provider-qualified Runtime Tool Task, dependency-closed
local Flow, digest-bound Knowledge Surface, and UI values in one batch. Tool,
MCP, and OKF readiness edges resolve only within the same exact package
generation and unavailable providers fail before
publication without a compatibility registry write. Flow source verification,
digest staging, Native TypeScript preflight, and cancellation-aware workspace
locking finish before publication. Dynamic multi-scope OKF search remains a
separate compatibility-owned query path. Official renderer-host adoption remains separate
integration work; scoped CLI/Desktop execution intentionally keeps its narrower
managed-MCP/Skill/UI cut. Its process-owned resolver composes the trusted
Runtime/private Gateway lazily only for admitted Streamable HTTP MCP, starts
nothing for stdio/Skill/UI-only generations, and preserves Session close before
Gateway shutdown.

The upstream MCP identity gate is now closed. A3S Use preserves every named
`PluginMcpSurface` ID, multiplicity, lifecycle generation, transport, and ready
endpoint or launcher receipt in its typed snapshot. The CLI revalidates the
exact package evidence, prepares Core `McpBinding` values through the trusted
host resolver, and closes same-package MCP-dependent Flow/UI edges in the same
transaction. Extension MCP therefore no longer uses compatibility
reconciliation; the singular built-in MCP path remains only for first-party
compatibility surfaces.
`CAP-GA1` starts only after official hosts have delegated to the complete path
for one major release.

Delivered `CAP-PROFILE1` adds a presentation plane after Run admission, not a
second capability registry. The Session persists one
`ToolPresentationProfileV1`; each main turn first applies the Run-frozen
permission visibility boundary and then projects Adaptive, Direct, Code, or
Disabled definitions in canonical order. Code mode retains the existing
`program` Tool name and parameter schema while generating a bounded compact
signature catalog from only permission-visible definitions. Execution still
resolves through the pinned `ToolExecutor` and the same Tool `Arc`; the Profile
does not acquire, publish, mutate, or retire an A3S Use generation. Delegated
runs inherit the exact parent Profile, and resume rejects a different explicit
Profile.

### 3.5 Durable memory program

The durable-memory program adds reusable, evidence-backed agent state without
turning an extraction model or a storage backend into a truth authority. A3S
Memory is the policy-free integrity kernel: it owns exact namespaces, atomic
revision-checked changes, immutable history, typed evidence references, pure
query, admission/use events, and caller-owned atomic/revision-CAS vector-index
primitives. Code owns
bounded turn extraction, redaction, candidate proposal, embedding execution,
hybrid serving policy, exact candidate re-verification, final-context admission,
verified conditional index publication, and session-owned maintenance
scheduling. The embedding host owns namespace selection, evidence retention,
verification decisions, repository and semantic index reinjection, index
refresh timing, distributed lease/remote-backend policy, and consolidation
policy.

| Gate | State | Code-owned outcome | Exit criteria |
| --- | --- | --- | --- |
| `DM-SHADOW1` | Delivered | Successful V1 extraction can mirror one content-addressed V2 Candidate with bounded redacted `SessionTurn` evidence | Shadow failure cannot change V1 serving; replay is idempotent; candidates never enter prompt context |
| `DM-ACTIVE1` | Delivered | Explicit Manual/Verification activation enables bounded Active-only lexical and one-hop `RelatedTo` recall for one exact namespace | Final selected revisions persist admission before model input; stale or unpersistable items fail closed; conflict edges and non-Active nodes never expand |
| `DM-MAINT1` | Delivered | Session-owned typed maintenance jobs can apply host-decided V2 lifecycle changes without placing policy in the repository | Verified atomic supersession retains old history and inverse relations; health is bounded; close cancels and joins workers |
| `DM-EVAL1` | Delivered | Versioned retrieval and product fixtures compare no memory, V1, and V2 through deterministic public APIs and real `AgentSession` turns | Relation Recall@5 and V2 task success reach `0.90`; write precision and evidence fidelity reach `1.00`; context, calls, nominal cost, conflict preservation, and admissions pass locked gates |
| `DM-RESTART1` | Delivered | Session snapshots retain a versioned, secret-free durable-memory binding while the host re-injects the live repository | Missing, newly acquired, scope-drifted, mode-drifted, or policy-drifted bindings fail closed; file-repository restart preserves evidence and admission/use history; Session teardown releases the repository lock |
| `DM-MULTI1` | Delivered | A versioned word/CJK-bigram retrieval profile is persisted in the exact binding and exercised through real English, Simplified Chinese, Japanese, and Korean `AgentSession` turns | Recall@3 and MRR are `1.00`; context is at most one node per task; Candidate, foreign-namespace, and no-overlap translated queries produce no leakage; legacy query semantics cannot silently resume as current |
| `DM-SHARE1` | Delivered | A host can explicitly bind one exact persistent repository namespace to independent agents without implicit child inheritance | The persisted context-identity profile rejects silent resume drift; two real agents with colliding process-local run IDs record three distinct session/run admissions; Candidate and foreign-principal content stay hidden; one agent survives peer teardown; file-journal replay preserves every admission |
| `DM-ENDURE1` | Delivered | Binding schema 4 adds a Code-owned invocation incarnation and ordinary runs use non-replacing atomic reservation | Three complete process epochs resume four agents with reset process-local generators and one retained run; all 24 contexts remain distinct, a verified revision and history survive four file opens, and retained collisions fail before model use without overwriting history |
| `DM-SEM1` | Delivered | Rust hosts can compose Code's bounded embedding executor with an A3S Memory `VectorIndex`, exact Active-revision verification, deterministic lexical/semantic RRF, cancellation, and lexical fallback | Binding schema 5 freezes authority, embedding revision and execution policy, vector descriptor, semantic policy, and fusion profile; a real-session fixture reaches cross-language semantic Recall@1 `1.00` with zero lexical positives and zero Candidate, foreign-namespace, or stale-vector hits |
| `DM-REFRESH1` | Delivered | Explicit semantic refresh rebuilds one complete Active namespace from a verified dual-budget A3S Memory snapshot without adding a background task | Initial snapshot identity is recomputed; post-publication source identity is verified by an exact namespace token or a second complete snapshot, source drift requires partition invalidation and never returns a receipt, pre-publication failure preserves the prior partition, cloned sessions serialize cleanup, and a secret-free success receipt binds source bytes/digest, serving generation, node count, and vector revision |
| `DM-CAS1` | Delivered | A CAS-capable shared vector index rejects delayed semantic publication and delayed drift cleanup from independently constructed runtimes | A3S Memory compares the expected global index revision at the same linearization point as partition mutation; Code captures the base revision before snapshot work, cleanup uses the published revision, strict callers reject weaker backends before I/O, and adversarial races preserve the newer partition |
| `DM-SCHED1` | Delivered | An opt-in session-owned worker periodically runs the verified semantic refresh against one exact live binding | Admission requires revision CAS before worker spawn; runs do not overlap, missed ticks skip, health and the latest successful receipt are observable, and clean bounded close finishes post-publication verification before releasing the worker |
| `DM-SKIP1` | Delivered | Verified unchanged scheduled ticks avoid redundant embedding and vector publication without weakening the refresh proof | Skipping requires an ownership-epoch receipt plus exact source, generation, CAS-captured revision, and full index status; source or index drift rebuilds, partition-atomic backends never skip, and a replacement owner clears the prior process-local receipt |
| `DM-TOKEN1` | Delivered | An optional exact A3S Memory namespace token suppresses redundant source snapshots while preserving complete rebuild and publication proofs | Built-in repositories need zero snapshots on a stable tick and one on a stable rebuild; token drift before embedding fails without provider or publication work, post-publication drift conditionally invalidates, inactive-only changes advance the receipt without republishing, and unsupported custom repositories retain the verified two-snapshot publication path |
| `DM-REUSE1` | Delivered | Scheduled rebuilds reuse exact vectors from the current ownership epoch while retaining complete atomic publication | A text-free single-partition cache is keyed by the full semantic record ID and bounded by the refresh node/vector budgets; index-only drift has zero provider inputs, partial source drift embeds only misses, removal rebuilds from retained vectors, failed CAS publication does not promote prepared embeddings, and owner close clears vectors while retaining the receipt |
| `DM-OBS1` | Delivered | Hosts can quantify scheduled semantic-refresh work without exposing memory content | One ownership epoch retains saturating cumulative counters plus the latest 64 settled published, unchanged, or failed runs: change-token requests/valid observations, snapshot requests/node reads/bytes, logical cache hits and embedding inputs, provider-adapter invocations/inputs/bytes including retries, publication attempts/records, and elapsed time; clean close retains evidence while replacement ownership resets it, and adapter counts do not claim remote transmission or billing |
| `DM-RECOVER1` | Delivered | A host-persisted semantic-refresh checkpoint can recover an unchanged schedule without re-embedding or republishing | Recovery omits the repository-history token and always verifies one complete Active snapshot; a skip additionally requires the exact vector-index history token, revision, and full status, while unrelated repository histories, colliding index status, a missing vector token, or any drift conservatively rebuilds; the next stable tick returns to the zero-snapshot path |
| `DM-PROD1` | Delivered | Host pack `/tmp/dm-prod1-host-3b2f72aa` on tip `3b2f72aa`, `report.json` `sha256:b3f36cf99a27db359251620e846bf38e366a35f00b62cecaa4951f09b16f7938`: all seven rows pass — Boyue `text-embedding-3-small` (1536-dim probe, probe 1.64 s, latency p50 0.81 s, 19 requests, 124 inputs, 2146 tokens, 0 failures), Redis `IndexRevisionCas` via `WATCH`/`MULTI`/`EXEC`, `SET NX EX` leases with `INCR` fences, 8 independent writers racing one prefix (1 commit / 7 conflicts, convergence to 8 records), 2 restart cycles plus checkpoint resume settling `Unchanged`, drift/cache hits 0/48/51/48/0 against provider inputs 48/4/1/2/0, zero cross-namespace recall on a shared index, `HYGIENE_OK`. Harness: `examples/durable_memory_prod1_host` (feature `dm-prod1-host`) + `scripts/harbor/run_dm_prod1_host.sh`. Caveats retained per row in the report and in [DURABLE_MEMORY_PRODUCTION_QUALIFICATION.md](docs/evidence/evaluations/DURABLE_MEMORY_PRODUCTION_QUALIFICATION.md): minutes-scale single-process horizons, single-process writer fleet, client-side failover only (no Sentinel/Cluster promotion), unrenewed epoch lease, target recall graded on set membership rather than rank 0. This pack does not cover standing-context admission `9fb1cb79`, tip `2bd6a234`, tip `9f521169`, or changelog fold `439cc740`. | 2026-10-02 |
| `DM-TEL1` | Delivered | The fact conversation spine surfaces memory recall telemetry to the host event channel: the actor parks recall batches on the session surface and the stream worker emits `MemoryRecalled`/`MemoriesSearched` before `End`, never sending from inside the state machine | Mock pin `fact_memory_events` 1/1 (events before End), regression `session_close_settles_a_stream_blocked_on_confirmation` 1/1 after the direct pass-through design was rejected for stalling/racing the bounded channel, lib `4003/0`, live paraphrase recall asserts `mid_run_memory_events=1` and passes — evidence in `docs/evidence/durable-memory/2026-10-07-dm-tel1/OUTCOME.md` |
| `DM-OPT1` | Delivered | FileMemoryStore reads stop paying writes: a per-process access ledger accumulates recall counts and the item file persists exactly once per item, at the prune-protection threshold crossing — ranking never reads the counter, so only protection semantics needed durability | A 5-hit recall drops from 5 reads+5 writes to 5 reads; pins prove sub-/post-threshold retrieves leave the file byte-identical, the crossing persists once and survives reopen (`53/0` a3s-memory, `60/0` core memory family, clippy clean); structural writes unchanged — evidence in `docs/evidence/durable-memory/2026-10-07-dm-opt1/OUTCOME.md` |
| `DM-CTL1` | Delivered | CLI `VecMemoryStore` fails closed on damaged timestamps: unparsable RFC3339 values sink to the epoch with a warn instead of masquerading as `Utc::now()` and inflating recency | Pin `damaged_timestamps_fail_closed_to_the_epoch_not_now` 1/1, CLI clippy clean; the pre-existing `cli_contract` skill-command drift is attributed separately — evidence in `docs/evidence/durable-memory/2026-10-07-dm-ctl1/OUTCOME.md` |
| `DA-LIVE1` | Delivered | Live dual-agent qualification: a real main model delegates code review to the `review` subagent and reports its concrete findings; a real reviewer agent adjudicates a main run's evidence through the evaluation substrate with findings bound to the evaluator record and provenance receipt; and a skill's child agent calls an attached MCP server with annotation-escalated confirmation routed to the parent UI | Research-review live pass (structured verdict+findings, `validate_for_run` green, 9–10 s); delegation live pass (planted off-by-one found by the reviewer, digest-bound `ToolRequestBound`); skill×MCP live pass (server-log ground truth + confirmation escalation); conversation subagent telemetry lane fixed (`with_agent_event_tx`), skills validator spacing-bypass closed on parsed permissions — lib `4004/0`, clippy clean, evidence in `docs/evidence/dual-agent/2026-10-08-dual-agent-live/OUTCOME.md` |
| `DA-LIVE2` | Delivered | The durable session-review lifecycle has live coverage driven as a product host drives it, and MCP `tools/list_changed` refreshes the cached tool catalog without a reconnect | Review-loop live pass (real main turn, real structured reviewer adjudicating product-vs-source with a planted contradiction, injection proven, pending→addressed(run-bound)→accepted closed); list_changed refresher spawned at the connect choke point with a one-shot non-blocking receiver hand-off, fixture pin `tools_list_changed_grows_the_cached_catalog_without_reconnect` 1/1; lib `4029/0`, MCP family `203/0`, clippy clean — evidence in `docs/evidence/dual-agent/2026-10-08-session-review-and-mcp-refresh/OUTCOME.md` |
| `CFG-LIVE1` | Delivered | Layered config discovery overlays instead of shadowing (later provider layers win scalars they set, union model catalogs, append dir sets), `A3S_DEFAULT_MODEL` accepts real nested model ids on every frontend, unset `env()` warns at parse, and the section-preserving write→reload chain carries live traffic | Overlay live pass (user layer dead-endpoint + stale key overlaid by workspace layer serving a real turn — the pre-fix document resolves to the dead layer), dynamic reload live pass (edit preserves comments/`env()`/unrelated sections, reparse serves a real turn); core `4031/0`, ACP config `4/4` under a panic-safe env lock, TUI `21/0`, clippy clean — evidence in `docs/evidence/config/2026-10-08-config-layering-dynamic/OUTCOME.md` |
| `PGQ-LIVE1` | Historical chain retained; bounded Harness checkpoint/owner/control tests120/120 and two fresh actual LLM paths pass; full lifecycle qualification open | Mate actor/fold/inbox/single writer/guardian retained. Typed budget Deny remains terminal. Original checkpoint-committer failure and cleanup diagnostics both reach the blocking caller; fail-closed retention and release proofs unchanged. Actual export Err/save-path Err retain best-effort policy. Full lifecycle remains under [010](../../specs/010-code-steer-terminal-settlement/spec.md) | Historical evidence unchanged. [Current Code evidence](../../docs/evidence/code-harness/20261010-steer-terminal-settlement/OUTCOME.md): checkpoint repair02 compile0,120 exact first PASS,0ignored,strictClippy0;928 finite inputs stable. Ten actual public Session checkpoint-fault cases cover Memory/File send/stream panic/export Err and FileStore path-inspection Err, with actual unique committer observations and original retention/release. All baseline first/SOLO failures retained. Fresh provider-only live01 compile0,blocking+streaming terminalSteer close/newSession2/2PASS,four successful actual responses,zero tool/native execution; strictClippy explicitly reused on same928inputs. Fourteen actual Fact-value snapshots and separate independent bounded reviews retained. Full T006/T008/Q1–Q7,T009/delivery/landing,all task closure,billing and world ranking unclaimed |
| `SEC-FIX1` | Delivered | Nine security/correctness fixes from the max-effort model review: shell-quoted session cwd interpolation (metacharacter injection past permission surfaces), char-safe Windows curl rewriter (UTF-8 release-abort), 0600-before-content + symlink refusal for CLI config.acl (template + atomic rewrite floor), OS OAuth token store write window, TUI transcript export redirect, deny-only subagent confirmation tightening (DenyOnAsk not AutoApprove), verifier-turn dynamic-namespace fail-closed (mcp__/plugin__), and a 4 MiB PDF decompression budget | lib `4039/0` with three new pins (cwd quoting, deny-only inheritance, dynamic-namespace verifier closure), clippy 0 across core+cli — evidence in `docs/evidence/standards/2026-10-08-review-fix-batch/OUTCOME.md` |
| `APR-LEARN1` | Delivered | Approval learning: an approved `bash` call's leading executable token persists in a workspace ACL document (`<ws>/.a3s/approvals.acl`, parsed with a3s-acl) and later sessions of that workspace skip the confirmation round-trip for the same prefix — with a never-learn floor, plain-name validation, a 64-entry cap, and parse-back-before-write | `approval_learning` family 5/5 (learn/round-trip, floor+metacharacter refusal, non-bash exclusion, cap eviction, malformed-doc fail-closed); clippy clean; parallel plugin failures attributed to sibling in-flight files (solo green) — evidence in `docs/evidence/approval-learning/2026-10-08-apr-learn1/OUTCOME.md` |
| `CTX-STATE1` | Delivered | Injected context sections are typed and replace-on-change: `ContextAssembler::assemble` keys pending results by provider name and keeps only the latest contribution per source (first-appearance order preserved; item dedupe, ranking, and budget logic unchanged; replacement is not ranking-based), and `ContextResult::content_digest()` exposes a domain-separated content-addressed SHA-256 (`a3s.code.context.result-content.v1`) over contributed item contents so `(provider, digest)` identifies a source's current section | `context::` family 89/89 (baseline 84/84 solo before the change + 5 new: same-source latest-wins replacement beating a higher-ranked stale item, identical-content identical-digest single rendered section, distinct sources coexisting, content-addressed digest form, digest tracks every item); full `--lib` 4055 passed / 0 failed / 28 ignored; `clippy -p a3s-code-core --all-targets` 0 warnings — evidence in `docs/evidence/context/2026-10-08-ctx-state1/OUTCOME.md` |
| `CTX-REQ1` | Scoped Harness repair and read-only model checks verified | Required instructions survive source/ID deduplication; exact source breaks ranking ties. User-delegated project guidance stays below runtime authority; public streaming End preserves mapped token/cache usage | [Context selector evidence](../../docs/evidence/code-harness/20261011-context-assembly/OUTCOME.md): baseline3 failures reproduced SOLO; repaired context72/72. [Real-provider evidence](../../docs/evidence/code-harness/20261011-context-selection-live/OUTCOME.md): final3 test functions /5 scenarios first PASS,20 actual responses; prompt/usage unit47/47, strict core lib/tests Clippy0, owned fmt/diff0;931 finite inputs stable during tests. Earlier failures and fixture corrections retained; final source differs only by checked rustfmt whitespace. New suite registered in Layer C. No candidate/native/VM execution, task-quality or performance gain, full Layer C/world-ranking or landing claim |
| `SEC-READ-BATCH1` | Scoped working-tree repair verified | Authorize every built-in read batch member through the existing scalar/session policy; Deny wins across members, then Ask, then Allow. Invalid or oversized batches fail closed; rules no longer match newline-joined paths | [Retained evidence](../../docs/evidence/code-harness/20261011-batch-read-permission/OUTCOME.md): baseline9/9 failures reproduce SOLO; repaired permission/tool-name99/99 first PASS, strict core lib/tests Clippy0. Final real-provider2/2 first PASS: scoped batch plus cross-file rename verified by exact host bytes and effect digest; mixed denied batch exposes no private bytes. Earlier fixture failures/SOLOs retained; final4 successful responses,934 finite inputs stable. ACL suite registered. No candidate/native/VM execution, full behavior/Layer C/world-ranking or landing claim |
| `CTX-HISTORY1` | Scoped working-tree repair and live regressions verified | Keep original model replies/questions in the same Moore fold and preserve assistant roles in Core provider projection, so answered historical requests are not presented again as unanswered user messages | [Retained evidence](../../docs/evidence/code-harness/20261011-conversation-history/OUTCOME.md): baseline4 failures reproduce SOLO; repaired Effect whole package90/90 and Core related78/78, strict Effect/Core lib/tests Clippy0 and scoped fmt0. Strengthened compaction2/2 first PASS:13 primary rounds plus7 compaction calls, actual input-digest binding, latest corrected value and empty memory; scoped source-edit/denial2/2 compatibility first PASS. All938 finite inputs stable. No second actor/store/model loop, VM/Box/candidate execution, full behavioral/Layer C/world-ranking or landing claim |
| `CTX-TOOL-HISTORY1` | Scoped Code working-tree repair verified | Restore retained original tool ids/names/arguments across compaction, preserve actual assistant roles, apply prepared context only to the latest explicit user line, retain bounded newest tool output, and treat generated summaries as derived history | [Retained evidence](../../docs/evidence/code-harness/20261011-tool-history-compaction/OUTCOME.md): 10 new baseline failures reproduce SOLO across context and budget pins; final related Core/receipt/prompt182/182 first PASS, strict Clippy/fmt0. Real compaction6 turns/5 cuts retains exact native ToolUse/ToolResult and typed receipt/input/usage bindings; source-edit and private-denial compatibility2/2 first PASS. Final16 successful governed responses/61,263 reported tokens,942 finite inputs stable. Corrects the earlier helper-only assistant-role qualification. No full behavior/Layer C/world ranking or landing claim |
| `BUDGET-FACT-TOOL1` | Captured host tool budget restored | Consult the Run loop's existing BudgetGuard in original fact-tool preparation before hooks or tool effects; preserve soft/hard events and denied zero-effect typed receipts | [Retained evidence](../../docs/evidence/code-harness/20261011-tool-history-compaction/OUTCOME.md): 3 budget regressions reproduce SOLO; repaired4/4 budget pins plus full related182/182 first PASS. Real original read enters the captured tool guard exactly once. Permission refusal and pre-cancel enter neither budget nor tool body. Owner-lifetime/mid-wait cancellation acceptance remains with spec012 |
| `CANCEL-TOOL-BUDGET1` | Original tool-budget cancellation verified locally | Cancel the exact borrowed host BudgetGuard future before input hooks/tool effects, retain the original typed Cancelled identity, and allow a distinct next Run in the same Session | [Retained evidence](../../docs/evidence/code-harness/20261011-tool-budget-cancel/OUTCOME.md): 2 baseline regressions reproduce SOLO; final related191/191 first PASS, strict Clippy/fmt0. Real1/1 first PASS, original worker cooperatively joins, guard1/0/1/0, zero tool/command effects; next Run completes with original cancelled native pair retained. 2 successful responses/7,508 reported tokens;944 finite inputs stable; ACL53 suites valid. Public End vs canonical Cancelled boundary recorded, full owner/terminal/Layer C/world ranking remain open |
| `CANCEL-PUBLICATION1` | Original sealed cancellation projected to callers | Use the original retained publication kind for blocking cancellation errors and public Error frames; no successful End for a cancelled Run and no second token decision after a successful seal | [Retained evidence](../../docs/evidence/code-harness/20261011-cancel-publication/OUTCOME.md): 4 public-entry baseline failures reproduce SOLO; final related197/197 first PASS, strict Clippy/fmt0. Real1/1 first PASS, original public End0/cancel diagnostic1, original typed Cancelled receipt, same-session next Run OK, guard1/0/1/0; 2 responses/7,498 reported tokens and945 finite inputs stable. Actual retained-End hook late-token race preserves sealed success. API/SDK release compatibility, full owner/terminal/Layer C/world ranking remain open |
| `ISSUE-187-ACTUAL-PAGER` | Local behavior verified; release open | Correct the earlier scaffold/source-skew diagnosis: the shipped vendor pager main lacked terminal admission. Refuse interactive startup before config/runtime/terminal side effects unless both input and output are terminals; retain headless dispatch | [Evidence](../../docs/evidence/code-harness/20261011-issue-remediation/OUTCOME.md): startup4/4; actual fresh debug binary non-TTY4/4, zero startup files;4190 local inputs stable; CI now checks actual pager behavior and standalone checkout stamp. Public artifact/CLI pin delivery pending; vendor strict Clippy fails in untouched dependencies and reproduces SOLO |
| `ISSUE-189-COMMAND-BOUNDARY` | Misrouting fixed locally; session capabilities open | Unknown/unavailable/bare slash commands under the Code connection produce a visible local refusal and no model prompt. Do not unhide fork/resume/dashboard before their backend contracts exist | [Evidence](../../docs/evidence/code-harness/20261011-issue-remediation/OUTCOME.md): baseline3 failures reproduce SOLO; strengthened4/4 and related prompt197/197 first PASS. Canonical session catalog, picker/startup resume, non-destructive fork, and dashboard remain open with acceptance order recorded |
| `ISSUE-189-SESSION-CATALOG` | Read-only Core/ACP inventory verified locally; UI recovery open | Project dated session folders and legacy effect logs through the canonical fact parser, with bounded reads, no-follow directory handles, pre-open regular-file checks and a second handle gate. Bind ACP list to the launch workspace; retain scan admission through request cancellation; refuse ambiguous identity and report incomplete scans. Unknown headless kind stays unknown | [Evidence](../../docs/evidence/code-harness/20261011-session-catalog/OUTCOME.md): original ACP list returned null; Core folder family24/24, ACP list11/11 including initialize→list over the actual JSON-RPC codec and cancelled-waiter admission, pure outcome1/1; Core strict Clippy passes. ACP strict Clippy9 existing sites reproduce SOLO; picker/startup resume, typed session kind, non-destructive fork, dashboard and public delivery remain open |
| `ISSUE-189-RESUME-ADMISSION` | Read-only admission verified locally; successful recovery open | Resolve an exact persisted identity in the host workspace before model, MCP, documents or filesystem activation. Refuse missing, foreign, corrupt, colliding, ambiguous or incompletely traversed records; reuse bounded canonical fact parsing and worker-owned scan admission. Keep existing runtime checks and unavailable UI gates | [Evidence](../../docs/evidence/code-harness/20261011-resume-admission/OUTCOME.md): both original load regressions reproduce SOLO; final Core32 + ACP34 =66 passed, workspace all-target strict Clippy0, owned fmt/diff0;759 finite Code source/manifest inputs stable. Nine earlier ACP production lints repaired without exemptions. Global fmt still fails on130 other paths; recovery writer handoff/replay, picker/startup resume, non-destructive fork, dashboard and public delivery remain open |
| `ISSUE-176-OIDC` | Code workflow repaired; npm authorization open | Remove all dead-token dependencies including prepublish preflight; use Node24/npm11.5.1 OIDC only on the publishing job, with the reusable caller permission and exact Code repository identity | [Evidence](../../docs/evidence/code-harness/20261011-issue-remediation/OUTCOME.md): actionlint0, script checks0; exact nine package bindings documented. npm maintainer @ZhiXiao-Lin / Office#162 coordination and actual tag publication/provenance remain unverified |
| `ISSUE-162-DEPENDENCY-CONTRACT` | Upstream blocker corrected; migration open | Public Search3c5015c already exports the library product cascade, disproving the unpublished-API explanation. Request-local prepared-browser injection and coherent local-provider/patch publication still need resolution before deleting Code's private plan | [Evidence](../../docs/evidence/code-harness/20261011-issue-remediation/OUTCOME.md): exact public source audited; no second cascade/wrapper, pin bump, or process-global runtime workaround. Coordination address Code#162 / A3S-Lab/Search / @ZhiXiao-Lin; headless on/off and partial-result acceptance recorded |
| `MEM-CONS1` | Delivered | Memory usage telemetry plus an opt-in lexical consolidation job: after a settled stream turn, a fire-and-forget recorder bumps a metadata-backed `usage_count` on every recalled V1 item whose distinctive token (longest purely alphanumeric word ≥8 chars, case-insensitive) appears in the final assistant text (unresolvable durable/ledger ids skip silently, one bump per id per turn), and hosts opt in via `MemoryMaintenanceOptions::with_usage_consolidation` to run the `v1_usage_consolidation` built-in, which merges lexical near-duplicates (word-set Dice ≥0.60 over the first 200 normalized chars) by deleting the lower-importance twin, folding its usage counter into the survivor, and never touching curation-protected items | telemetry family 4/4 (token match → 1, absent → none, per-id dedupe, unresolvable-id skip) and consolidation family 3/3 (near-dup merge with usage fold + distinct untouched, pinned twin preserved, opt-in gating with reserved name and interval validation); `memory::` and full `--lib` green; clippy 0 — evidence in `docs/evidence/durable-memory/2026-10-08-mem-cons1/OUTCOME.md` |
| `RESUME-3G1` | Delivered | Fact-log ordinal fork: `FileLog::truncate_after(thread, seq)` atomically keeps the prefix `seq <=` (tmp+rename, 0600) and `AgentSession::fork_session_at(seq)` truncates the session's effect log so the next run replays only the retained prefix, refusing while a run is active | effect truncate family 5/5 (prefix+count, beyond-end no-op, idempotent, unknown thread, empty log); core fork family 2/2 (replay-only-prefix oracle with a recording LlmClient, busy refusal); full lib `4063/0`, clippy 0 — evidence in `docs/evidence/run-control/2026-10-08-resume-3g1/OUTCOME.md` |
| `SEC-FIX2` | Delivered | The local backend's unbounded text-read primitive is bounded: `read_text` (shared by edit before-state, grep recount/verification, bm25, retrieval reconcile, and the Read tool) and `read_text_range` (a no-newline file is one unbounded line) refuse files over `MAX_TEXT_FILE_BYTES` = 16 MiB with the structured `WorkspaceError::TooLarge { path, size_bytes, max_bytes }`, and the allocation itself is bounded by `take(limit + 1)` so a file growing between stat and read cannot exceed the bound | local family 37/37 (3 new: oversize refuse with typed fields, exact-bound read, ranged refuse on a no-newline file); error family 9/9 (display names path/size/limit); `ToolErrorKind::TooLarge` mirrors the variant through `from_workspace_error` (pinned) and `sanitize_tool_error_kind`; degrading callers verified (grep recount row + failure count, retrieval per-file `ReadFailed` keeps indexing); full lib 4067/0 with the process-group timing test green solo twice (parallel-load contention, attributed); clippy core+acp 0 — evidence in `docs/evidence/standards/2026-10-08-read-text-bound/OUTCOME.md` |
| `SEC-FIX3` | Delivered | A `~/.grok/models_cache.json` entry can no longer redirect the grok provider endpoint: `models_from_cache` stops reading `info.base_url` (previously any model entry replaced the account-level URL, last-iteration-wins, sending the bearer token and identity headers to a URL of the cache's choosing), the endpoint is the provider contract `DEFAULT_BASE_URL`, and a custom endpoint belongs in an explicit provider block with the account import disabled | grok family 4/4 with the pin flipped to a hostile `collector.example.net` cache URL that must NOT win; full acp lib 119/0 with two process-spawn timing tests (plugin_mcp/plugin_tools) green solo (parallel contention, attributed); clippy 0 — evidence in `docs/evidence/standards/2026-10-08-grok-cache-endpoint/OUTCOME.md` |
| `SEC-FIX4` | Delivered | The ACP turn queue is bounded: `TurnQueue` (an `AtomicUsize` waiting count with a `WaitingSlot` RAII guard around the turn `Mutex`) replaces the bare lock, so a prompt arriving while `MAX_QUEUED_PROMPTS` (32, half the run-control inbox cap) are already queued behind the active turn is rejected with an `invalid_params` error naming the bound instead of growing queued turns without end; the guard's `Drop` returns the slot on every exit path (granted, request future dropped mid-queue, task panic), and the owned-permit cancellation contract is unchanged | agent tests 41/41 (2 new: bound rejection + slot recycling on drop, past-bound re-rejection, drain after turn; deterministic cancelled-future pin via a single noop-waker poll — no spawn/timing race); full acp lib 121 passed with the two known process-spawn timing flakes solo-green earlier in session; clippy 0 — evidence in `docs/evidence/standards/2026-10-08-turn-queue-bound/OUTCOME.md` |
| `SEC-FIX5` | Delivered | The remote git backend refuses unauthenticated activation off-loopback: constructing with neither bearer token nor mTLS now fails with an error naming the endpoint (previously a warn-and-activate that sent unauthenticated traffic wherever `base_url` pointed); the credentialless mode remains localhost-dev-only, enforced by `base_url_is_loopback` (`localhost` or a loopback IPv4/IPv6 via `url::Url`; unparseable URLs are not loopback), and token/mTLS activation against any endpoint plus the cert-without-key fail-closed pins are unchanged | remote_git family 33/33 (3 new: remote/no-auth refuses with the endpoint named, non-loopback IP and unparseable URL refuse, loopback dev mode still constructs for localhost/127.0.0.1/[::1], token'd remote constructs and cert-without-key still fails); full lib 4071/0; clippy 0 — evidence in `docs/evidence/standards/2026-10-08-remote-git-auth/OUTCOME.md` |
| `SEC-FIX6` | Delivered | The CLI guardrail's credential/control-plane floor sees through shell glob metacharacters: a path component containing `*`/`?`/`[` is matched as a pattern (iterative star-backtracking `glob_match`; bracket classes simplified to the bracket-stripped spelling) against a representative protected-name set — fixed credential names, the `.env` stem plus one spelling per `.env.`/`.env-` prefix family, and the workspace control sets — so `cat .en?`/`.en*`/`*.env`/`~/?d_rsa`/`.gi?/config` deny while benign globs (`src/*.rs`) stay available | guardrail family 7/7 (bypass list extended with 8 glob spellings + conservative-but-not-blanket matcher pin); cli bin 148/0; clippy 0; S3 reads verified already bounded (`DEFAULT_MAX_READ_BYTES`), closing the SEC-FIX2 residual note — evidence in `docs/evidence/standards/2026-10-08-guardrail-glob/OUTCOME.md` |
| `GOAL-CLR1` | Delivered | `GoalAchieved` is consumed: the ACP event handler retires the session's active durable goal (in-memory map + the `<workspace>/.a3s` disk copy) when the mechanical evidence gate authorizes an achievement, so later prompts stop injecting the standing prefix without an explicit `/goal clear`, with a client-facing "goal achieved and cleared" note; paused goals and goal-less sessions are no-ops (the turn cannot speak for a goal whose prefix it did not carry), and no objective/description string matching is attempted — the serialized-turn semantics decide | goal family 8/8 (2 new: active-goal retirement clears map+disk with a non-verbatim description, paused/absent no-ops); agent tests 41/41; clippy 0; live verified-achievement leg deferred to the live chain's next pass — evidence in `docs/evidence/run-control/2026-10-08-goal-clear1/OUTCOME.md` |
| `APR-LEARN2` | Delivered | Approval learning verified end-to-end over the pager transport: the `approval_learning_probe` live suite drives a real model turn whose `git --version` bash call asks the ACP client exactly once (approving persists `learned_approvals = ["git"]` to `<workspace>/.a3s/approvals.acl`), then a NEW session in the same workspace runs the same command family with ZERO further permission round-trips while the stream provably carries the command's real output — the non-vacuous discriminator | live 1/1 first run (11.4s, real model); registered in the layer-c chain's `acp_suites` with the standard one-bounded-retry discipline — evidence in `docs/evidence/approval-learning/2026-10-08-apr-learn2/OUTCOME.md` |
| `CTX-REC1` | Delivered | Context sections are replay-observable: `context::section_digests` exposes each source's latest `(provider, content_digest)` (same latest-wins selection as assembly), and the fact spine's collapse point records a `context.sections` fact per model call before the `model.turn` it describes (best-effort like the steer record; an empty list is recorded evidence of "no context"), so replaying the log verifies which section content each turn actually consumed — identical pairs across turns are prompt byte-stability at the section level | unit pin 1/1 through `send`→fact-log with a mutable provider (record precedes model.turn; unchanged sources digest identically; a mutated source digests differently); context:: 92/92; fact_log 17/17; full lib 4072/0; clippy 0; durable-memory recall sections excluded by design (their lane is MemoryRecalled telemetry) — evidence in `docs/evidence/context/2026-10-08-ctx-rec1/OUTCOME.md` |
| `MEM-CONS2` | Delivered | Consolidation gains a real-model judgment stage: the lexical Dice filter recalls candidate pairs and an optional `ConsolidationJudge` confirms them before any merge (`MemoryMaintenanceOptions::with_consolidation_judge`; without one the MEM-CONS1 lexical behavior is unchanged), with `LlmConsolidationJudge` as the production judge — one structured call per pair over the same 200-char window, a first-line SAME/DIFFERENT closed-set verdict (garbage and `NOT SAME` never merge), `false`/error fail-closed to skip, and judge calls bounded at 32 per run so a pathological store cannot become an unbounded model bill | memory family 49/49 (3 new: judge-confirmed merge consulted once; rejection and error both keep the pair with the error path exercised; closed-set classification incl. decorated verdicts and negation); full lib 4075/0; clippy 0; semantic usage telemetry (embedding in the telemetry path) recorded as the next ordered item — evidence in `docs/evidence/durable-memory/2026-10-08-mem-cons2/OUTCOME.md` |
| `MEM-SEM1` | Delivered | Usage telemetry gains a semantic arm: `memory::SemanticUsageJudge` (public, host-injected over the shared `EmbeddingExecutor` infrastructure; cosine similarity on a 2,000-char window against a fail-closed-validated threshold, default 0.60) receives only the memories the free lexical arm could not decide, so a paraphrase still counts as use; embedding errors decide nothing (degrade to lexical, never a wrong bump), semantic evaluations are bounded at 8 per turn, and `MemoryConfig.semantic_usage_judge` is a `#[serde(skip)]` live handle — ACL-configured sessions run the lexical arm alone, unchanged | turn_context family 8/8 (3 new: same-bucket paraphrase bump, different-bucket + failing-provider no-bump, threshold validation); memory family 49/49; full lib 4078/0; clippy 0 — evidence in `docs/evidence/durable-memory/2026-10-08-mem-sem1/OUTCOME.md` |
| `NET-FORCE1` | Surveyed | The forced-proxy enforcement stack already exists in `crates/sandbox` (deny-by-default Seatbelt profiles with loopback-only Gate 4 CONNECT / Gate 5 SOCKS5 mediators, `compose_child_env` forced-proxy injection that overrides explicit env and empties NO_PROXY, `features.mediated_network` arming) — the gap is that a3s-code never arms it: the product exposes no surface, so sandboxed bash simply has no network. Implementation is deliberately not started: arming the mediator widens a denied surface and must land as a closed-set ACL network mode (deny default) behind the existing approval gate; the survey records the verified facts and the five-point implementation shape | survey evidence in `docs/evidence/sandbox/2026-10-08-net-force1-survey/OUTCOME.md`; standing triggers unchanged (a workload that actually needs in-sandbox egress) |
| `NET-FORCE2` | Delivered | The product surface for mediated sandbox egress: `SandboxNetworkMode` (closed set `deny` default \| `mediated`, parse error names the set) threads from `SessionOptions` into `NativeBashSandbox::with_network_mode`, where `deny` is byte-identical to the baseline policy (digest-pinned) and `mediated` arms Gate 4 on the same baseline with allow rules parsed from `host`/`host:port` origins — an empty allowlist refuses by design (mediating nothing is not a posture; discovered live), malformed origins refuse instead of widening into a typo, and no host behavior changes without the explicit opt-in | sandbox::native 4/4 (2 new: deny-digest identity; mediated arms on this macOS host with the allowlist pinned into the policy, empty-allowlist and bad-port refusals) + sandbox::tests 15/15 (closed-set parse/default pins); full lib 4082/0; clippy 0; ACP/ACL attribute mapping, Gate 5 SOCKS exposure, and a live egress round-trip are the recorded follow-ups — evidence in `docs/evidence/sandbox/2026-10-08-net-force2/OUTCOME.md` (input: the NET-FORCE1 survey) |
| `REV-SELF1` | Delivered | Adversarial self-review of the session's twenty landings: every risky edge probed with concrete scenarios — two findings confirmed and fixed same-pass (the NET-FORCE2 origin parser accepted `::1` as host `":"` port 1, kept `[::1]`'s brackets, and took port 0 — all now pinned refusals; the CTX-REC1 fact key `context:{unix_ms}` could collide into a silently-dropped record on same-millisecond model calls — the key now carries a domain-separated payload digest), six edges verified safe with the attack documented (take() backstop, turn-queue cancellation window, glob matcher, loopback classification, cosine NaN/zero norms, judge budget under errors), three accepted semantics noted | sandbox::native 4/4 with the three parser findings pinned as refusals; fact_log 17/17; full lib 4082/0; clippy 0 — evidence in `docs/evidence/standards/2026-10-08-self-review-batch/OUTCOME.md` |
| `NET-FORCE3` | Delivered | The mediated sandbox posture is reachable from product configuration, in A3S ACL only: the `sandbox` block (`network = "deny" \| "mediated"` closed set whose parse error names the set; `network_allow = ["host[:port]", …]` validated at parse time so a typo refuses config load; `mediated` with an empty allowlist refuses outright) flows ACL → `CodeConfig` → the ACP session-open path → `SessionOptions` → `NativeBashSandbox::with_network_mode`, arming the Gate 4 stack end to end from a workspace's `.a3s/config.acl`; an absent block keeps the deny baseline byte-identical for every existing config | config pin 1/1 (absent-block default, mediated+allowlist round-trip, closed-set error, empty-allowlist refusal, malformed-origin refusal at parse time); full lib 4085/0; acp agent tests 41/41 (call sites updated); clippy 0 both crates; sibling's in-flight form feature shares two files — this commit stages only the content-filtered sandbox hunks — evidence in `docs/evidence/sandbox/2026-10-08-net-force3/OUTCOME.md` |
| `SEC-EGRESS1` | Delivered | The sandbox posture is host-authoritative: only a layer the host consented to (`--config`/`A3S_CONFIG` or `~/.a3s/config.acl`) may set the `sandbox` block. ACP layer sources now carry a trust kind, and `withhold_workspace_sandbox_posture` overwrites the merged config's posture from a trusted-only parse at one choke point — a value override, not a spelling filter, so any attribute alias or `env()` attempt is withheld the same way; a workspace layer carrying the block logs a warn naming the file. `HOST_AUTHORITATIVE_BLOCKS` (closed set; extending is a platform decision) and structural `host_authoritative_block` detection live in core. Layer reads are 1 MiB-bounded and symlink-refusing. Deliberately supersedes NET-FORCE3's workspace-arming claim (move such blocks to the user layer or `--config`); narrowing is withheld symmetrically — the workspace layer has no say in either direction. Follows the CLI's `trusted_host_config` split | core config 80/0 (1 structural pin: comment/attribute spellings are not declarations, truncated blocks detected); acp config 8/0 (4 pins: arm withheld, narrow withheld, explicit keeps authority, workspace-only keeps deny); full core lib 4157/3 with all three failures attributed to a sibling session's in-flight `fact_control` refactor; acp lib 135/3/2 attributed (form-format rename in flight + two known solo-green timing flakes); clippy/fmt add nothing in this slice's files — evidence in `docs/evidence/sandbox/2026-10-09-sec-egress1/OUTCOME.md` |
| `SEC-FALLBACK1` | Delivered | The process-host fallback can never deliver a wider posture than requested: `select_default_local_sandbox` now takes the requested posture and matches on it in the fallback arm — only a `Deny` request may degrade to `ProcessHostBashSandbox` (the ratified Harbor/Terminal-Bench outer-container path, warn level, unchanged); a `Mediated` request with a failed native backend keeps `UnavailableDefaultSandbox` (Bash denied) with an error-level log and a message naming the withheld fallback, the unhonorable posture, and both remediations. Any future posture must decide its own stance in that match — it cannot inherit the fallback silently | session_sandbox 10/0 (new pin: opt-in + mediated + native failure ⇒ denied, stable message prefix kept and the refusal clauses named); sandbox:: 29/0 unchanged; `--features deep-live` compiles (the gated caller's signature updated — default builds never compile it); clippy/fmt clean in the slice's files — evidence in `docs/evidence/sandbox/2026-10-09-sec-fallback1/OUTCOME.md` |
| `HYG-GATE1` | Delivered | The clippy gate is restored on committed code: six errors that had landed on HEAD lines across six slices (unused `AtomicBool` import, unnecessary `mut` on the sampling bridge's messages, single-arm `match` → `if let` in the memory semantic arm, two `.into()` on `&'static str` session-config fields, `manual_is_multiple_of` in the long-horizon twin) are fixed — every one lint-shape only, no behavior change. Root cause stays open in HYG-RATCHET1: the pre-commit hook gates only fmt, so slices that verified tests but not clippy could land red | clippy `-p a3s-code-core --all-targets -D warnings`: the six HEAD-attributed errors gone; all remaining errors verified to sit in another session's uncommitted code (per-line HEAD-vs-worktree comparison; `surface_decision.rs` untracked); acp/kernel had none; affected families re-run 8/49/13/10/1 all green; fmt clean on the slice's lines — evidence in `docs/evidence/standards/2026-10-09-hyg-gate1/OUTCOME.md` |
| `SEC-READBOUND1` | Delivered | The two text-read paths SEC-FIX2 left unbounded are closed: `read_search_file_at` (the grep walk's and manifest search's per-file read — previously no size check at all) gets the pre-open regular-file liveness gate (a writer-less FIFO blocks the OPEN, so the check must precede it), the open-file re-check, the 16 MiB stat bound, a `take(MAX+1)` cap and post-read check, with oversize/non-regular returning `None` under the walk's per-file tolerance contract; `read_text_range` gets the same pre-open gate plus take-with-consumed-byte accounting so mid-read growth surfaces as the honest `TooLarge` instead of a take-truncated window masquerading as EOF; `read_text` gets the pre-open regular-file gate that its "still fail-closed" comment already claimed | workspace::local 40/0 (3 new pins: non-regular refusal without opening, ranged refusal, grep skips oversize+FIFO and still matches); manifest 30/0; retrieval 166/0; clippy/fmt zero in the touched files; not-found contract unchanged (pre-open gate fires only on a successful non-regular stat) — evidence in `docs/evidence/standards/2026-10-09-sec-readbound1/OUTCOME.md` |
| `SEC-CLOSEDSET1` | Delivered | The interactive approval mode is a refused closed set: `InteractiveApprovalMode::parse` replaces the silently-coercing `from_name` (unknown → `Default` widened a typo of "plan" from hard denials into ask-and-confirm), naming the set `[default, plan, auto, force, yolo]` and the refused value in the error; `as_str` gives the canonical spelling (`yolo` prints `force`); `for_mode` propagates the refusal; the coercer is removed, not deprecated. The old pin asserted the fail-open contract and was rewritten on the standard's side per the message-triage rule. Cross-tree note: `apps/cli`'s `exec_policy.rs:185` needs `?` at its next core pin bump — compile-visible, never silent | permissions 100/0 (rewritten pin: full set parses, alias canonicalizes, unknown refused with set+value named, factory propagates); fact_control 47/1 with the failure pre-attributed to the sibling's in-flight refactor; serde's unknown-variant refusal already failed closed on the other boundary; clippy/fmt zero in the slice's files — evidence in `docs/evidence/standards/2026-10-09-sec-closedset1/OUTCOME.md` |
| `HYG-EVID1` | Delivered | Evidence-retention backfill for the 2026-10-09 review's constitution-II audit: three landings that carried no evidence get retrospective OUTCOME files reconstructed from their commit bodies, honestly labeled (`1ce40830` TooLarge mapping restore, `3acb58ff` foreground canceller window — also security-review-verified liveness-only, `3c019634` teardown-lock test hygiene) under `docs/evidence/standards/2026-10-09-hyg-evid1/`; ten landings that lacked ROADMAP rows are named here as the record: those three plus the SK-MCP1 pair (7bca96c4, 98c75baf — evidence exists in `skill-mcp/20261009-sk-mcp1`), the long-horizon quartet (6dd75024, 150b581a, 45e1b766, 12ec5d87 — evidence in `long-horizon/20261009-capabilities`), and `088183f0` ask_user_form (evidence in `maas/20261008-agent-forms`); and the one OUTCOME with no exclusion section of any name (`sandbox/2026-10-08-net-force1-survey`) gets its boundary backfilled from its own standing-triggers paragraph | Retrospective records only — no commands or counts are invented (each file's "Explicitly not claimed" says so); the audit's other 31 renamed-section files are recorded as the HYG-RATCHET1 allowlist's starting inventory, normalized on extraction rather than by a 31-file churn commit |
| `HYG-RATCHET1` | Delivered | The delivery discipline is a script ratchet, not memory: `scripts/check_landing.py <base>` fails any non-merge commit in `<base>..HEAD` that touches first-party apps/code code without committing a `docs/evidence/**/OUTCOME.md` and a ROADMAP touch in the SAME commit (§10's one-commit rule — the same-range follow-up pairing is deliberately not a pattern), and fails any added/modified OUTCOME.md lacking the exact "Explicitly not claimed" heading unless it sits in the 43-entry shrink-only allowlist (`scripts/check_landing_allowlist.txt`, generated with the checker's own regex; stale or already-fixed entries demand their line's removal). Wired as `just check-landing` and the final leg of `just ci` against `origin/main` — the six pre-ratchet offenders sit inside origin/main (recorded by name in HYG-EVID1), so the gate is green today and red on the next undisciplined landing. Clippy stays ci's own earlier leg; the ratchet is fast file-set + content checks only | Positive: this session's six landings clean (base e19b1bd9) and the deployed default clean over origin/main's 9 unpushed commits; Negative: base 0a859ab8 flags exactly the audit's six row-less landings by name; py_compile clean; deletion-safe name-status parsing; script anchors itself to the repo root and reads the allowlist worktree-first for pre-commit runs — evidence in `docs/evidence/standards/2026-10-09-hyg-ratchet1/OUTCOME.md` |
| `HYG-AGENTS1` | Delivered | apps/code carries its own `AGENTS.md` (root still the authority, and the file says so): the tree's own cargo workspace and toolchain resolution, the `deep-live` declaration gating (signature changes there are proven only by a feature check), the SEC-EGRESS1 configuration-authority choke points and closed set, the error-handling reference shapes (unwrap ledger, closed-set parse errors, bounded untrusted reads with the pre-open FIFO gate), the sibling-test convention with its `#[path]` hijack lesson and solo-rerun triage, and the HYG-RATCHET1 delivery gate with shared-tree hunk staging | Every statement verified against landed state this session (the six same-day gates and the ratchet) — a distillation with no new rules and no aspirational content — evidence in `docs/evidence/standards/2026-10-09-hyg-agents1/OUTCOME.md` |
| `SEC-PATCH1` | Delivered | The multi-file patch batch is bounded on both axes: `MAX_PATCH_ENTRIES = 64` refuses oversized batches before any staging, and `MAX_PATCH_BATCH_BYTES` (16 MiB, the single-file read bound) caps the summed before-side of every staged entry — a batch may inspect at most as much content as one max-size file, closing the 1000-entries-×-16-MiB staged-memory blowup where only the small diffs rode in the request. Mid-batch refusals land in the validate-everything phase, naming "nothing written", so the all-or-nothing contract is untouched | patch family 18/0 (2 new pins: 65-entry cap refusal with the file untouched on disk; 2×14 MiB staged-byte refusal mid-batch with both files byte-identical); single-entry max-size behavior unchanged (equality passes); fmt/clippy add nothing — evidence in `docs/evidence/standards/2026-10-09-sec-patch1/OUTCOME.md` |
| `HYG-F8-T1` | Delivered | Tranche 1 of the inline-test extraction census: `agent_api/direct_tools.rs` — the worst of the near-threshold set at 958 test lines in an 1185-line file (81%) — moves its test module to the sibling `direct_tools/tests.rs` via the bare `mod tests;` rule (the 10-08 `#[path]`-hijack lesson), dedented to column 0 after a quote-parity scan proved no multi-line string literals could be corrupted, and rustfmt'd as a standalone file. Extraction is a location change only: same 20 tests, same names, green before and after | direct_tools 20/0 identical pre/post; file 1185→227 lines; fmt/clippy zero in both files; run.rs (859) and retrieval/runtime.rs (826) remain for the next tranche — evidence in `docs/evidence/standards/2026-10-09-hyg-f8-tranche1/OUTCOME.md` |
| `HYG-F8-T2` | Delivered | Tranche 2: `workspace/retrieval/runtime.rs` (826 inline test lines, and the module is FEATURE-gated `all(test, feature = "a3s-vec-fts")` sitting mid-file with production code on both sides) moves its tests to `retrieval/runtime/tests.rs`; the gated bare `mod tests;` declaration keeps the sibling out of default builds. Tranche-1 recipe (line-exact split, quote-parity scan, standalone rustfmt); no `#[path]` attributes inside — the run.rs trap is absent, which is why this file went first | with feature: runtime family 21/0; without: retrieval 137/0; default `cargo check` 0 errors; fmt/clippy zero; file 1394→569 lines — evidence in `docs/evidence/standards/2026-10-09-hyg-f8-tranche2/OUTCOME.md` |
| `HYG-F8-T3` | Delivered | Tranche 3, the intricate one: run.rs's TWO inline test modules (`retention_tests`, `tests` — 857 lines combined) move to bare-declaration siblings, and the `#[path]`-attributed soak declarations inside `mod tests` become bare `mod run_protocol_soak;`-style declarations after the compile PROVED the trap: `#[path]` in a file module resolves against the file's directory (`run/`), only bare declarations take the `<stem>/` slot where the soak files already live. The 10-08 hijack lesson gains its second, verified clause. Soak files themselves untouched | run family 53/0 + 2 env-gated ignored (same shape as before); test-marker conservation exact (17+9=26 inline; the soaks' markers always lived in their own files); fmt/clippy zero; run.rs 1713→856 lines; sub-800 stragglers stay for opportunistic extraction — evidence in `docs/evidence/standards/2026-10-09-hyg-f8-tranche3/OUTCOME.md` |
| `HYG-F2` | Delivered | The 10-06 orphan decision lands: `source_consumers.rs` was already gone at HEAD (tree evolution resolved half the finding; recorded so no review re-derives it), and `cleanup_ordering.rs` is DELETED — zero references, no mod declaration (cargo check after deletion: zero errors — it never compiled), only the seed import ever touched it, no lineage claimed it in 30+ hours; it carried 2 panic sites no compiler ever saw. Restore address: `git show 1e0321b0:apps/code/core/src/agent_api/cleanup_ordering.rs` — reversible by that exact command, not a standing TODO | cargo check 0 errors post-deletion; git grep 0 references at HEAD; the unwrap audit's orphan exclusion loses one member, the four gated-file exclusions stand — evidence in `docs/evidence/standards/2026-10-09-hyg-f2/OUTCOME.md` |
| `SEC-MCP-ISO1` | Delivered | Both MCP isolation residuals close: `union_mcp_desired` is fallible and origin-aware — same-origin collisions keep later-wins, same-definition cross-origin collisions dedupe, and a different-definition cross-origin collision (a takeover of a live session's `mcp__<server>__` tool routing, including a session overriding the config baseline's name) is refused with `invalid_params` naming the server and both origins; definitions compare through serialization (custom Deserialize, no PartialEq; serialization failure counts as inequality). `set_session_model` now carries the recorded `mcp_servers` into the rebuilt session, so a model switch no longer empties its union contribution and the next sync cannot evict a live session's servers. The `merge_server_into` primitive and its pin are deleted — the rule lives in `merge_with_origin`, pinned at union level | union family 4/4 (3 new pins: takeover refused naming both origins, same-definition dedupe, same-session later-wins); integration pin walks the exact pre-fix eviction path (session+alpha → model switch → second session's resync → alpha survives); full acp lib 138/3/2 with the three failures pre-attributed; no per-session namespacing claimed — a named refusal replaces silent redirection, namespacing stays a product decision — evidence in `docs/evidence/standards/2026-10-09-sec-mcp-iso1/OUTCOME.md` |
| `SEC-GLOB1` | Delivered | The CLI credential floor refuses unexpanded bracket classes outright instead of approximating them: a component carrying `[` hits the floor unconditionally, closing range (`.[a-e]nv`), multi-char (`.e[nv]v`), and negated (`.e[^x]v`) classes BY CONSTRUCTION — no enumeration to drift out of date — with benign brackets (`file[1-3].txt`) riding the documented deny side. The bracket-stripping branch is deleted; `glob_match` and its `*`/`?` pins are untouched. Brace expansion and ANSI-C quoting are recorded residuals of the same finding (shell-evaluation class, not glob class) | guardrail 7/0 (rewritten pin: five bracket shapes floor, four benign non-bracket patterns available); full CLI bin 148/0+1; fmt/clippy zero — evidence in `docs/evidence/standards/2026-10-09-sec-glob1/OUTCOME.md` |
| `SEC-SAMPLE1-GUARD` | Delivered | The guard leg of the sampling gate: server-initiated MCP sampling's fail-closed default is now ENFORCED, not accidental — a source-scan ratchet in `sampling_gate_tests` strips inline test modules line-structurally (a top-level module's column-0 `}` is a rustfmt invariant, immune to braces inside string literals — the naive depth counter fired on stdio.rs's own test wiring during development and was replaced) and fails on any first-party production mention of `install_sampling_handler` outside its API home, with the failure message naming the three wiring preconditions (per-request consent bridge, per-server count/rate bound, generation-enforced max_tokens). Adding an `API_HOME` entry is the explicit act of landing wiring | Guard green over four trees; negative validation: an injected wiring probe fires the guard naming the exact file with the preconditions, restoring green on removal — not vacuous; mcp::client 14/0; the preconditions themselves remain the SEC-SAMPLE1 feature gate — evidence in `docs/evidence/standards/2026-10-09-sec-sample1-guard/OUTCOME.md` |
| `SEC-EGRESS2` | Delivered | Provider routing joins the host-authoritative set (user-ratified 2026-10-09): `HOST_AUTHORITATIVE_BLOCKS = ["sandbox", "providers"]`, and the ACP choke point (`withhold_workspace_host_authoritative_blocks`) overrides the whole providers catalog from a trusted-only parse — an untrusted workspace layer can no longer route the host's model traffic and credentials to endpoints of its choosing. The enforcement the decision needed also lands: ACP's launch path now refuses a `default_model` the merged catalog does not declare (the gate the TUI merge always had), so a withheld provider or a typo'd `A3S_DEFAULT_MODEL` fails at the boundary instead of at session-client build. The workspace layer keeps its say over models/skills/default_model as scoped; CC Switch/Grok imports remain the trusted bootstrap | acp config 8/0 (rewritten overlay pin + new workspace-only self-provision refusal); core config 80/0 (structural pin on the extended set); full acp lib 139/3/2 all pre-attributed; TUI's merge remains the recorded follow-up surface — evidence in `docs/evidence/standards/2026-10-09-sec-egress2/OUTCOME.md` |
| `SEC-EGRESS2-TUI` | Delivered | The last front door joins the authority split, and the mechanism moves to its one home: `config::LayerAcl` + `withhold_untrusted_host_authoritative_blocks` land in core beside `HOST_AUTHORITATIVE_BLOCKS`; ACP becomes a thin wrapper; TUI `merge_launch_layers` classifies its layers (explicit/user trusted, workspace untrusted) and applies the choke point — a workspace layer's `providers`/`sandbox` no longer reach the merged catalog, with the withheld signal on stderr (the thin scaffold has no tracing). TUI's `read_layer` gains the same 1 MiB bound + symlink refusal + `take()` race cap the ACP/CLI paths have. The fixture pinning workspace-provider merging is rewritten to the ratified contract (providers in the user layer, workspace keeps `default_model` say); TUI's own declaration gate is what makes the withhold bite | TUI lib 22/0 (rewritten fixture + new withhold pin: workspace provider absent from the catalog, user provider drives the launch); acp config 8/0 through the wrapper; core config 80/0; clippy/fmt zero in the slice's regions (three pre-existing TUI fmt deltas left alone per surgical discipline) — evidence in `docs/evidence/standards/2026-10-09-sec-egress2-tui/OUTCOME.md` |
| `CFG-YAML1` | Planned | The second user-ratified decision (2026-10-09): the `directory_projection.rs` frontmatter surfaces (`tools/*.md`, `schedules/*.md` — MCP server commands and scheduled prompts) migrate into A3S ACL (`mcp_servers` and a schedules block), the parser already speaks the destination. Migration carries a deprecation period for the frontmatter shapes; the constitution IX DATA list does NOT gain these formats (the same-day memo already classifies "servers" as ACL territory) | Ratification recorded here; implementation is a separate slice with its own evidence when scheduled |
| `CFG-YAML1-SERVERS` | Delivered | The servers leg of the ratified YAML→ACL migration: `agent.acl`'s `mcp_servers` block family (the loader's own names and `parse_mcp_server_block`, now `pub(crate)` in its one home) becomes the declaration of record for directory MCP servers, and `tools/*.md` `kind: mcp` enters its deprecation period — the shape still loads with a warn naming the file and the migration target, refusal scheduled at the period's end (a later slice flips it). The manual's mapping table names the ACL block as the record and marks the markdown shape deprecated. `kind: script` and `schedules/*.md` remain the second leg (new closed-set ACL blocks) | directory_projection 7/0 (2 new pins: ACL declaration of record with stdio round-trip; deprecated shape still loads during the period); the live e2e fixture rides the period unchanged; fmt/clippy zero — evidence in `docs/evidence/standards/2026-10-09-cfg-yaml1-servers/OUTCOME.md` |
| `CFG-YAML1-SCHEDULES` | Delivered | The second leg completes the ratified migration's declaration side: `agent.acl` grows closed-set `schedules "name" { cron, prompt }` (cron parsed fail-closed naming the schedule; the prompt FILE stays content, read bounded — ACL has no multiline strings and the constitution's line is configuration-vs-content) and `scripts "name" { path, allowed_tools, description? }` (allowed_tools required non-empty — the authority the markdown shape smuggled), with script paths validating through the SAME `validate_script_path` extracted from the markdown path. Both markdown shapes enter their deprecation period (warn naming file and target) and YIELD: a markdown entry whose name an ACL declaration holds is skipped as superseded — pinned with same-name-different-payload fixtures for both families | directory_projection family green with 2 new pins (ACL schedules+scripts round-trip incl. due(); markdown yields to ACL names); manual mapping table updated; refusal-at-period-end and the live-e2e fixture migration land with the refusal slice — evidence in `docs/evidence/standards/2026-10-09-cfg-yaml1-schedules/OUTCOME.md` |
| `SEC-SAMPLE1-CAP` | Delivered | Precondition 1 of 3 for any sampling wiring: the server-requested maxTokens now bounds GENERATION. `LlmClient::with_output_cap` lands in the house configured-view shape with a FAIL-CLOSED default (a client that cannot express the cap returns None and the caller REFUSES — unbounded generation is not a substitute); Anthropic/OpenAI/Zhipu override it through their native `with_max_tokens` knobs (Clone-derived views sharing the http Arc), CodexLogin keeps the default and therefore refuses sampling until it grows a knob — recorded, not silent. The sampling bridge routes maxTokens through the capped view and reports real usage unclamped; the incoming clamp (1024 default, 8192 ceiling) is unchanged and now feeds generation | sampling 6/0 (2 pins: uncapped client refuses naming the exact cap; capped view receives the exact requested value); llm 497/0 through the trait change; mcp::client 16/0; the SEC-SAMPLE1 guard untouched and green — preconditions 2 (consent bridge) and 3 (count/rate bound) still gate any wiring — evidence in `docs/evidence/standards/2026-10-09-sec-sample1-cap/OUTCOME.md` |
| `SEC-SAMPLE1-BOUND` | Delivered | Precondition 3 of 3: `BoundedSamplingHandler` — a per-server count ceiling on sampled generations (`DEFAULT_CEILING = 64`, `with_ceiling` for tests/host override), claimed with `fetch_add` before the inner call (denied calls still consume — conservative) and refusing with the exhausted ceiling named. A lifetime count bound also bounds any burst, so a separate rate window is deliberately absent (documented, not implemented twice). Placement contract in the type doc: wrap OUTSIDE the approval gate so consented calls draw from the same ceiling; the wiring slice must show that composition to the guard's review | sampling 8/0 (2 pins: ceiling-of-2 admits exactly two inner calls then refuses naming the ceiling; two servers keep independent budgets); mcp::client 18/0; guard untouched and green — precondition 2 (the consent bridge onto the existing SamplingApprover seam) is the last gate before wiring — evidence in `docs/evidence/standards/2026-10-09-sec-sample1-bound/OUTCOME.md` |
| `SEC-SAMPLE1-CONSENT` | Delivered | Precondition 2 of 3: `SamplingConsentBridge` (acp) implements the existing `SamplingApprover` seam (now re-exported beside `McpClient`), parking ONE `SamplingConsentAsk { server, question, reply }` per sampled generation on a channel the agent loop will serve on request_permission — the human-facing question names the server and carries the excerpt (display-only: the excerpt is attacker-controlled content, the answer a bare boolean). Fail-closed in every non-approval direction, each pinned: denial, dropped reply, closed consent surface (refuses WITHOUT parking — never generate), and timeout (600 s default; the turn deadline still governs). Composition contract in the docs: wiring wraps this INSIDE BoundedSamplingHandler | sampling_consent 5/0 (approve/deny/drop/closed/timeout pins); staged-seam dead_code allow naming the wiring slice as consumer; guard untouched and green — the final wiring slice lands the agent-loop service, the install, the API_HOME addition, and the Bounded(ApprovalGated(Llm)) composition together — evidence in `docs/evidence/standards/2026-10-09-sec-sample1-consent/OUTCOME.md` |
| `SEC-SAMPLE1-SEAM` | Delivered | The wiring design's core half, landed whole and INERT: `McpManager` grows the exported `SamplingHandlerFactory` + `set_sampling_handler_factory`, `connect` threads it into `connect_ready_client_retained`, and `arm_client_sampling` installs the handler BEFORE initialize — installing IS the sampling capability declaration in the handshake. The projection adapter path passes None by recorded decision (frozen-binding contexts do not arm sampling). The landing guard's API_HOME gains `manager.rs` in the same commit — the seam's install call is the one sanctioned product site — and the guard stays green; nothing sets a factory, so every product path keeps the fail-closed -32601 refusal. `sampling_declared` becomes crate-visible so the arm step pins without a handshake (no factory → undeclared; factory → declared) | mcp family 175/0 (new arm pin in `sampling_arm_tests` with an unwired transport stub); fmt/clippy zero; the acp half (sessions-sharing refactor, consumer task, Bounded(ApprovalGated(Llm)) composition, staged-allow removal) stays the next slice — evidence in `docs/evidence/standards/2026-10-09-sec-sample1-seam/OUTCOME.md` |
| `SEC-SAMPLE1-WIRE-ACP` | Delivered | The wiring closes: `ensure_core_agent` arms once — consent channel built, the consumer spawned on the same LocalSet, the launch default model client composed as `Bounded(ApprovalGated(Llm))` (bridge INSIDE the ceiling), and the manager factory set; any arming failure leaves sampling refused with a warn, never crashing the agent. `serve_sampling_consent` resolves the owning session (declaring session wins; a baseline server falls to the first live session; no sessions means nobody to ask, so refuse), asks Allow-once/Deny on request_permission (RejectOnce — a YOLO drain cannot answer), and settles the reply; the bridge timeout still bounds the wait. The client field becomes Rc-shared and sessions Arc-shared (Deref-transparent); the staged dead_code allow is removed in the same commit; core gains two facade passthroughs. The guard stays green — acp composes, the two API_HOME files install | Consent resolution pinned through real opened sessions (owner wins; empty map gives None); every gate independently pinned from its own slice; full verification ran against a sibling concurrently refactoring the crates/use tree with zero errors in this slice files; no live pager leg claimed (rides the live suites) — evidence in `docs/evidence/standards/2026-10-09-sec-sample1-wire-acp/OUTCOME.md` |
| `SEC-WSTRUST1` | Delivered | Opening a repository no longer starts its processes or redirects host paths. `HOST_AUTHORITATIVE_BLOCKS` adds `search`, `sessions_dir`, `memory_dir`, `user_instructions_dir`, `storage_backend`, `storage_url` and `os`. `config::admit_untrusted_layers` keeps each workspace `mcp_servers` declaration inert until the user trusts its digest (repository root + canonical declaration; an edit is a new pending entry; no shadowing of trusted names) and drops `skill_dirs`/`agent_dirs` entries that escape the repository. The store is `<state root>/code/workspace-mcp-trust.json` (locked, atomic, 0600, fail closed). Review: `a3s-code-acp --list-workspace-mcp`, `--trust-workspace-mcp <id\|all>`, and `initialize` `_meta.a3s.workspaceMcp` | Real-binary stdio E2E 2/2 (failed before the fix: the declared command ran on `session/new`); core `workspace_trust` 9/9; acp bins 7/7; acp lib 249/4 parallel, the 4 contention cases 18/18 solo; core lib 4971/9, the 9 also fail solo, all on the macOS `native_process_lifetime_unqualified` refusal, not config loading. Hooks/plugins/extensions review, revocation command and client UI not claimed. Evidence: [`OUTCOME.md`](../../docs/evidence/code-harness/20261017-workspace-layer-trust/OUTCOME.md) |
| `SEC-APPROVAL2` | Delivered | "Don't ask again" is explicit, narrow and user-owned; supersedes the APR-LEARN1/APR-LEARN2 storage and matching. Only an `allow-always` answer learns (offered only for a learnable command; YOLO still answers `allow-once`). The rule is program + the subcommand or target that carries the risk, from a closed allowlist; shell control syntax never learns or matches, and per-program argument refusals block argument-level execution. Rules live in `<state root>/code/approvals.acl` keyed by workspace (locked, atomic, 0600, ACL round-trip, fail closed); `<workspace>/.a3s/approvals.acl` is ignored | Real-binary stdio E2E 1/1 (failed on the previous commit: a planted repository `make` entry skipped the ask); `approval_learning` 9/9 incl. a 47-command bypass corpus; agent option/answer split 1/1; acp lib 254/4 parallel, the 4 pass solo 21/21. Live probe moved to the new contract, not run live. Revocation UI and `PermissionPolicy` rule writing not claimed. Evidence: [`OUTCOME.md`](../../docs/evidence/code-harness/20261017-approval-rules/OUTCOME.md) |
| `SEC-PERM-POSTURE1` | Delivered | No permission switch skips deny rules. `PermissionPolicy.enforcement` is a closed set (`enforced` default, `unrestricted`); `unrestricted` allows only what no deny rule matches, and denied tools stay hidden from the model. The retired `enabled = false`, which returned Allow before any rule ran, is refused with an error naming the fix; Go, Node and Python bindings resolve it through the same core function | go bridge real session E2E 1/1 (failed on the previous commit: `enabled = false` opened a session that ran a denied `write`); core `permissions::` 99/99; core lib 4972 passed, 9 known macOS native refusals; acp lib 258/258; Node and Python compile-checked only. Evidence: [`OUTCOME.md`](../../docs/evidence/code-harness/20261017-permission-enforcement/OUTCOME.md) |
| `SEC-ACCOUNT1` | Delivered | A default launch reads no other product's login. CC Switch, Grok and Cursor imports (the Cursor one reads the macOS keychain and runs `agent models`) run only for the sources the host names: `import_accounts = [...]` in a trusted layer or `--import-accounts`, from the closed set `cc-switch`, `grok`, `cursor`. `import_accounts` is host-authoritative, so a repository `.a3s/config.acl` cannot opt in; a malformed value refuses the configuration | Real-binary stdio E2E 3/3 with logins at their real paths and a sentinel `agent` (failed on the previous commit: a default launch offered `grok/grok-4` and `cursor/composer-2.5`); core `account_import` 5/5; acp `config::` 17/17, bin 8/8, lib 262/262. CC Switch is not exercised end to end; the `a3s code` launcher does not forward the flag. Evidence: [`OUTCOME.md`](../../docs/evidence/code-harness/20261017-account-import-opt-in/OUTCOME.md) |
| `STD-UNWRAP2` | Delivered | Production `.unwrap()` / `.expect(..)` are counted exactly and may only shrink (release builds abort on panic). A `syn` walker follows the core and ACP module graphs from their crate roots (`mod`, inline modules, `#[path]`), skips `cfg(test)` modules, items, impl functions and files, and counts calls inside expression-list macros. Per-file baseline: 90 sites in 25 files, replacing the uncounted "115-site legacy"; a higher count, a new file with a site, a stale-high row or a dead row fails `cargo test --workspace` | Ratchet + walker fixture 2/2; a probe `unwrap` in a clean file fails it; 537 files walked, and the 36 unwalked non-test-named files are all `cfg(test)`/deep-live modules or one orphan; a spot-checked row matches by hand. No site removed; SDK crates and path-call/`panic!` forms not counted. Evidence: [`OUTCOME.md`](../../docs/evidence/code-harness/20261017-production-unwrap-ratchet/OUTCOME.md) |
| `REL-913-PUBLISH` | Candidate | Core 9.1.3 is on crates.io from `2ba6f4fc`; ACP was never published and cannot build against registry Core 9.1.3 (14 errors). Every release member moves to 9.1.4 (Core and ACP to crates.io; the pager ships as a binary, 18 path-only dependencies; SDKs version-aligned). `manual/RELEASE_9.1.4.md` holds the four-step gate | Version check passes at 9.1.4; joint `cargo publish -p a3s-code-core -p a3s-code-acp --dry-run` passes from the committed source (1066 + 94 files); a clean `cargo install --path acp --locked` reports 9.1.4 and completes one real ACP turn (`anthropic/qwen3.8-flash`, `end_turn`, `READY`). The gate found a bug: a forced structured tool plus thinking was refused with HTTP 400; fixed in `8a686a2c`. Upload (gate 4) is the maintainer's manual step and has not run. Evidence: [`OUTCOME.md`](../../docs/evidence/code-harness/20261017-release-914-candidate/OUTCOME.md) |
| `REL-TRAIN1` | Gate defined | Core and ACP ship on a two-week train (every second Thursday from 2026-10-29; freeze the day before; a red gate cancels the train, no partial release). `just release-gate` = version check + `just ci` + joint publish dry run + `just release-smoke`, one real ACP turn through the actual binary (`acp/tests/release_smoke.rs`, ignored by default, fails closed without `A3S_RELEASE_SMOKE_CONFIG`). Cadence and train log: `manual/RELEASE_CADENCE.md` | Smoke passes on `anthropic/qwen3.8-flash` (`end_turn`, 11.8 s), fails on a provider that returns HTTP 402, fails when the config is unset, and is skipped by a plain `cargo test`. Not claimed: two consecutive on-time trains (none has left yet; 9.1.4 awaits the manual upload) and a green `just ci` on this macOS host. Evidence: [`OUTCOME.md`](../../docs/evidence/code-harness/20261017-release-train/OUTCOME.md) |
| `CORE-FILES1` | Done (no production file over 2000 lines; the cap list is empty) | A production source file stays under 2000 lines (`core/tests/production_file_size_ratchet.rs`, on the module graph shared with the unwrap ratchet in `core/tests/support/production_graph.rs`; inline test modules count). Five files are split: `dynamic_workflow.rs` 3412 → 1013 (store, identity, observer, lease, tool, control, drive), `agent_api/conversation_runtime.rs` 3052 → 860 (owned launch/fact/worker, host events, turn outcome, recovery support), `agent_api/run_admission.rs` 2349 → 1454 (publication, sibling tests), `porcelain.rs` 2117 → 1931 and `workspace/retrieval/persistent.rs` 2005 → 1329 (sibling tests). Every public path is unchanged | On the committed tree: size ratchet 1/1, unwrap ratchet 2/2 (90 sites, 551 files), clippy `-D warnings` all targets and features clean, 875 affected core lib tests pass (the 9 known macOS sandbox failures aside), FTS persistent tests 11/11. `acp/src/agent.rs` was split under `ACP-SPLIT1` (1476 lines, ACP protocol limit 1500). `core/src/fact_control.rs` 4858 → 1679 (model call, turn facts, tool results, executor tools, compaction, history seed, fact reads); the last cap row is deleted, core lib 4988 passed / 0 failed with the 18 known macOS sandbox tests skipped, the size, unwrap, `turn_engine_entries` and tool parallelism ratchets pass, clippy `-D warnings` is clean. Evidence: [`OUTCOME.md`](../../docs/evidence/code-harness/20261017-production-file-size/OUTCOME.md), [`OUTCOME.md`](../../docs/evidence/code-harness/20261017-fact-control-split/OUTCOME.md) |
| `CORE-API1` | Delivered (budget); removal pending 10.0 | The documented crate root of `a3s-code-core` is the 133 names that ACP, the Node/Python/Go SDKs and the vendored pager import (measured from their `a3s_code_core::` imports), down from 777. The other 644 re-exports stay as `#[doc(hidden)]` deprecated aliases (removed no earlier than 10.0, `### Deprecated` in the 9.1.4 changelog), so no consumer breaks. `core/tests/crate_root_surface.rs` (syn) fails on more than 150 documented names, on a new hidden alias or a stale hidden baseline (644), and on a new public top-level module (81) | Surface ratchet 2/2 (fixture counts grouped, renamed, `cfg`-gated and hidden names apart); default and all-features core builds and ACP all targets compile unchanged. Not claimed: no alias removed yet; about 18 hidden names live under non-public modules (`agent_api`, `prompts`, `agent`, `work_policy`, `compaction`) and need a public path before 10.0; examples still import 26 hidden names; SDK builds not rerun. Evidence: [`OUTCOME.md`](../../docs/evidence/code-harness/20261017-crate-root-surface/OUTCOME.md) |
| `SEC-SHELL1` | Delivered | One parsed shell classifier (`core/src/permissions/shell.rs`, quote-aware and recursive into substitutions, subshells, groups, function bodies, `sh -c`/`eval`/`env -S` and heredocs fed to a shell, with wrappers such as `env`/`sudo`/`nice`/`timeout`/`xargs` removed) feeds the permission policy, the interactive guardrail, the verifier turn and ACP approval learning. `ShellRisk` and `OpaqueConstruct` are closed sets and `ShellRisk` has no allow member, so it can only narrow. Deny and Ask rules reach every simple command; argument-scoped Allow rules need every simple command to match and no opaque construct; `prefix:*` ends at a word boundary; a narrow catastrophic floor (wiping root, a home or a system directory, formatting or overwriting a disk, `kill -1`, power-off) is denied under every allow rule, YOLO lane and unrestricted enforcement. At HEAD the YOLO lane let 70/70 catastrophic commands run, prefix rules let 28/39 smuggled commands run and `deny Bash(rm:*)` missed 25/26 evasions | Shared corpus (`core/tests/support/shell_bypass_corpus.rs`, 176 bypasses plus 4 control lists) 7/7 in core and 2/2 in ACP; permissions and verifier unit tests 118/118 with every prior guardrail contract unchanged; core lib only the 9 known sandbox refusals; clippy clean. Not claimed: not a full bash parser (unknown syntax asks); inline `python -c`/`node -e` code is not analyzed; the verifier now runs `2>&1` pipelines; YOLO still runs `sudo` and `curl \| sh`; the specialty `Bash(find:*)` rule still matches `find -delete`; the vendored TUI classifier is not migrated. Evidence: [`OUTCOME.md`](../../docs/evidence/code-harness/20261017-shell-classifier/OUTCOME.md) |
| `CORE-SPLIT1` | Delivered (decision) | Measured `a3s-code-core --lib` per profile from one snapshot: `local-code` 394 deps / 93 s clean build, `+advanced-harness` 396 deps / 91 s, `full` 521 deps / 113 s. The advanced harness adds only `a3s-flow` and `serde_yaml_ng` and no measurable build time; the weight of `full` is the already-optional `scientific`/`server` stacks. ADR `docs/adr/0001-keep-advanced-harness-feature-gated.md` keeps the five advanced modules in core behind `advanced-harness` (no `a3s-code-advanced` crate) and names the thresholds that reopen it. | Evidence: `docs/evidence/code-harness/20261017-crate-split-measurement/` |
| `CORE-DEADLINE1` | Delivered | `SessionOptions::max_execution_time_ms` now bounds a session stream end to end. Before, only the legacy `AgentLoop` read it; the `FactRun` model port never did, so a Terminal-Bench trial with a 30-minute budget was still running after 1 h 47 min. `LiveCompletion` spends one budget per run across continuation segments, fails the next model call closed with a non-retried defect once it is spent, bounds the in-flight call by the time left and aborts the dropped provider read. `core/tests/session_execution_deadline.rs` (endless tool loop with and without continuation, a model call that never returns) fails at HEAD and passes now. | Evidence: `docs/evidence/code-harness/20261017-fact-run-deadline/` |
| `CORE-VOCAB1` | Delivered (authorities); budget type pending | `docs/architecture/core-authorities.md` names one authority per core concept: `harness_loop::decide_completion` decides completion (goal control and planning supply evidence), the `a3s_effect` fact log is the run history (the run store and the `AgentEvent` stream are projections; `CoreEventLog` is test-harness only), `FactRun` drives session turns, and `permissions::shell` classifies shell commands. A budget table names the owner of each of 12 limits on both turn engines and records where they differ. `core/tests/completion_authority.rs` (syn) fails if production code outside `harness_loop` constructs `CompletionGate::Allow`, or emits `GoalAchieved` or asks the goal evidence check outside its pinned sites. | Evidence: `docs/evidence/code-harness/20261017-core-authorities/` |
| `CORE-LOOP1` | Partial (ratchet); child-run migration pending | `AgentLoop` is already crate-private, so no caller outside the crate exists. Inside it, session runs use `FactRun` while the `skill` and `task` child runs and plan steps still enter `AgentLoop` directly. `core/tests/turn_engine_entries.rs` (syn) pins the 11 production sites that build an `AgentLoop` or call its entry methods; the list is shrink-only, so a new bypass of `FactRun` fails. Not done: the `TurnEngine` rename and moving the child runs onto `FactRun`. | Evidence: `docs/evidence/code-harness/20261017-turn-engine-entries/` |
| `ACP-CMD1` | Done (commands + method-not-found); auth onboarding open | After `session/new` and `session/load`, the agent sends a real `available_commands_update` (`goal`). `initialize` lists the same commands in `_meta.availableCommands`. Unknown `_…` extension methods return -32601 instead of an empty success. `acp/tests/protocol_commands_stdio.rs` runs the real binary, and both tests fail at the previous HEAD. Not done: `authenticate`/`auth_methods` onboarding. | Evidence: `docs/evidence/code-harness/20261017-acp-commands/` |
| `ACP-CAPS1` | Done (client reads); client writes and `terminal/*` open | When the client declares `fs.readTextFile`, the `read` tool and other text reads are served from the client's buffer through `fs/read_text_file`, with fallback to the workspace. Core adds `WorkspaceReadOverlay`, `WorkspaceServices::with_read_overlay`, and `SessionOptions::with_workspace_read_overlay`; the overlay is not applied under effect isolation. `acp/tests/client_fs_stdio.rs` runs the real binary with a scripted model: a declaring client's unsaved text reaches the model, and a non-declaring client gets no request. Not done: `fs/write_text_file`, `terminal/*`, search over buffers. | Evidence: `docs/evidence/code-harness/20261017-acp-client-fs/` |
| `OBS-GENAI1` | Done (turn-engine calls); auxiliary calls and GenAI metrics open | Each model call from FactRun (`session.stream`/`send`) and AgentLoop now opens one `chat` client span. The span carries OpenTelemetry GenAI attributes (`gen_ai.operation.name`, `gen_ai.provider.name`, request and response model, response id, finish reason, `gen_ai.usage.input_tokens`/`output_tokens`) plus the `a3s.llm.*` names, and records `error.type` on failure. Before, no model-call span existed and `record_llm_usage` recorded into undeclared fields. `core/tests/gen_ai_spans.rs` runs a real session and fails with the FactRun hunk removed. Not done: memory extraction, compaction, planner, and other auxiliary calls; GenAI metrics and events. | Evidence: `docs/evidence/code-harness/20261017-gen-ai-spans/` |
| `TOOL-PAR1` | Done (measured, then widened for declared tools) | Measured on the session path: four 250 ms read-only calls take 303 ms for built-in `read` but 1038 ms for a host tool declaring `read_only_paginated(4)`, because FactRun admitted parallel batches by tool name only. It now also admits registered tools whose own capabilities allow a parallel batch, up to their declared `max_parallelism`: host and mixed batches take about 280 ms (3.7x faster), and a batch above the limit stays sequential. `core/tests/tool_parallelism_probe.rs` pins all four cases. Not done: AgentLoop engine, chunked execution above the limit, MCP tools. | Evidence: `docs/evidence/code-harness/20261017-tool-parallelism/` |
| `UX-APPROVAL1` | Done (reasons on every ask, allow-always measured) | Every ACP tool permission ask now carries a text block saying why it needs approval, built from the core's closed-set risk assessment (level plus each `ToolRiskReason`, exhaustive match), e.g. "Needs approval (high risk): it runs a command whose effects are not bounded to the workspace." In a scripted 10-command dev loop on 3 rules through the shipped binary, allow-always cuts asks from 10 to 3 (70%, target 30%). `acp/tests/approval_rules_stdio.rs` pins both. Not done: naming the permission rule that triggered the ask, question asks, core-only hosts, real-session measurement. | Evidence: `docs/evidence/code-harness/20261017-approval-reasons/` |
| `ACP-SPLIT1` | Partial (protocol file split; host crate not done) | `acp/src/agent.rs` went from about 4490 lines to 1476. Session opening, the turn loop, `session/update` rendering, asks, sampling consent, models and effort, and the host extension methods moved unchanged into ten `acp/src/agent/` modules. `ext_method` now dispatches through `HostMethod`, a closed set of 11 host method families (`parse`/`as_str`); anything else is `method_not_found`. The size ratchet drops the file from `OVERSIZE` and holds it under 1500 lines. Not done: an `a3s-code-host` crate with a `HostExtension` trait, so ACP still cannot run without the host layer. | Evidence: `docs/evidence/code-harness/20261017-acp-split/` |
| `ACP-CONF1` | Partial (in-repo suite and release report; no Zed run) | `acp/tests/acp_conformance_stdio.rs` drives the shipped binary with raw JSON-RPC over stdio through 10 ACP v1 cases: version negotiation, capabilities, prompt to `end_turn`, unknown session, -32602, -32601, cancel during a model call, permission request shape and reject, cancel during a pending ask, and `session/load` replay. It writes a Markdown report; `scripts/release_preflight.sh` step 8 puts it in the release evidence directory. 10 of 10 pass on 9.1.4, and a no-op `cancel` fails two cases. Not done: runs against Zed or another third-party client. | Evidence: `docs/evidence/code-harness/20261017-acp-conformance/` |
| `DOC-DIET1` | Done (index of three kinds; evaluation reports moved) | Ten measured evaluation and qualification reports moved from `manual/` to `docs/evidence/evaluations/` with history kept, and every link to and from them was rewritten (63 checked, 0 broken). `manual/README.md` sorts the 43 remaining documents into Guide (8), Reference (28), and Release (7) by explicit list. `core/tests/manual_index.rs` fails when a document is unlisted, listed twice, or under another kind. Not done: merging document content, or splitting measured sections out of the five mixed qualification pages. | Evidence: `docs/evidence/code-harness/20261017-doc-diet/` |
| `TB-BASE1` | Subset baseline recorded (0/10, not a benchmark score) | A fixed 10-task Terminal-Bench subset (`html-js-filter`, `photonic-waveguide-routing`, `music-harmony`, `bun-sourcemap-leak`, `foodstuff-beta-activity`, `freecad-platform-drawing`, `cad-model`, `freecad-impeller`, `freecad-spring-clip`, `interleaved-vigenere`) run once with `qwen3.8-flash` through the native runner, so later runner and Core changes have a fixed comparison point | 0/10 solved, 0 harness exceptions, every trial ended at `deadline_exceeded`; six tasks pass part of their verifier (up to 25/36). Recorded for the next slice: in 7/10 tasks a retry attempt did no work before the deadline, the trajectory file is empty in all trials, and the nudge text leaks into stdout. Not a full run; approvals are automatic. Evidence: [`OUTCOME.md`](../../docs/evidence/code-harness/20261017-terminal-bench-subset/OUTCOME.md) |
| `LLM-FACTORY-FALLIBLE` | Delivered | The missed finding from the error-handling pass, cleared on a ledger re-check: `create_client_with_config` (the production session-start path) built its reqwest client with `.expect` over an environment-dependent failure (TLS backend init — exactly the standing classification for this class), a latent panic that also threatened the sampling arming path contract of warn-never-crash. The factory now degrades to `UnavailableHttpClient`, a fail-closed HttpClient whose every post/post_streaming refuses naming the construction reason and timeout — a session under a broken TLS backend fails its FIRST CALL with a precise error instead of panicking the process. The infallible signature is kept: no caller ripple, the failure moved from panic-time to call-time (the fail-closed direction); factory.rs now carries zero expect calls | llm family 497/0+7 unchanged; fallback pin covers both request shapes naming the reason; forcing the TLS failure itself is not testable without fault injection — the no-panic property is structural (no expect remains on the path) — evidence in `docs/evidence/standards/2026-10-10-llm-factory-fallible/OUTCOME.md` |
| `CFG-WR-PARSE` | Delivered | The live environment unblocked: the user config (carrying `workspace_retrieval { enabled, local_cpu {} }` since 09-27) was refused wholesale by the loader — every product launch and live test loading it died at parse; the block belongs to a real programmatic feature whose ACL arm never landed. The arm now parses strictly (`enabled` required bool, optional empty `local_cpu {}`, unknown attrs/sub-blocks refused naming their sets) into the exposed `WorkspaceRetrievalConfig`; an enabled block WARNS the truth — semantic retrieval needs a host-injected embedding provider and stays off for sessions (no silent feature switch). The unknown-block refusal is now single-sourced from `KNOWN_CONFIG_BLOCKS`, ending the message drift (the stale set omitted real arms like `sandbox`) | config family 82/0 (2 new pins: shape round-trip + refusals, message names the real dispatch set); live proof: `test_cache_hit_rate_real_llm` green on the real model, cache share 85.7% over 6 turns (turns 2-6 cache_read ≈ full previous prompt) — evidence in `docs/evidence/standards/2026-10-10-cfg-wr-parse/OUTCOME.md` |
| `PAR-BATCH-OVERLAP` | Delivered | Speed, found by measurement: the new PAR-ADOPT live rail showed the real model dominant multi-read shape is ONE read call with a files[] array (5 files in one call), and execute_batch_read awaited each file sequentially — the tool wall clock was the SUM of per-file I/O (up to 32 files). Two-phase rewrite: concurrent join_all prefetch of the independent entries (resolution + ranged read, per-entry error rendering) into an UNTOUCHED sequential assembly (segments, shared byte budget, continuation, counters — byte-identical error text). All 19 existing semantics pins green unchanged; new deterministic overlap pin (three 60ms reads under 150ms); live post-optimization both rail arms green (3 decisions, one 5-file read, correct sum, ~6s model-bound) | read family 20/0; PAR-ADOPT rail added as the standing measurement (both arms recorded: the model parallelizes inside one call more than across calls — the guided-line arm bought nothing measurable, not shipped); decision-level also-batching untouched (WC-MEAS2 domain) — evidence in `docs/evidence/standards/2026-10-10-par-batch-overlap/OUTCOME.md` |
| `PAR-PATCH-OVERLAP` | Delivered | The companion speed slice and the capability finding the new live rail caught: the multi-file patches mode (SEC-PATCH1) was UNREACHABLE — execute dispatched it while the schema declared only file_path+diff with additionalProperties false; the model reasoning on record tried multi-file diffs three ways, concluded single-file-only, fell back to three parallel calls. The schema now advertises the patches array (item shape, 64-entry cap, all-or-nothing, example; no top-level required — either mode) with a drift pin keeping advertised and dispatched surfaces together; staging prefetches the per-entry chain concurrently with ordered first-error-wins assembly (all SEC-PATCH1 pins unchanged; 3×60ms staged reads < 150ms pinned) | patch family 20/0; live PAR-PATCH rail green on the real model: ONE patches call edits three files, declared suite binds the gate, disk verified, 8.3s — the model discovered the mode from the schema alone (the prompt wording existed before and could not unlock it); covers-matcher strictness is load-bearing, the rail first-glob fixture was wrong and is recorded — evidence in `docs/evidence/standards/2026-10-10-par-patch-overlap/OUTCOME.md` |
| `WC-NOTIFY1-AGENT` | Delivered | The in-tree half of attention re-flow: `notifications { run_completion = bell|off, min_seconds = 1..=86400 default 60 }` in the launch ACL (closed sets named on refusal, absent = quiet; the block joined KNOWN_CONFIG_BLOCKS), and the ACP emitter — prompt entry stashes the turn start, settlement reads it once, and a Bell-configured run past the floor gets `\x07` on stderr plus a duration log. The decision is the pinned pure `completion_bell_due`; the signal carries NO content (the gate trust invariant). Boundary recorded: Core owns the completion fact (turn.complete already carries terminal+digest+epoch; duration derivable — no speculative field), the terminal host owns focus suppression and OS notifications | config 83/0 (closed-set/floor/absent pins); bell decision pin; the settlement path rides every hermetic acp prompt test plus the live battery; no acp real-model rail exists yet (none did) — stated, not claimed — evidence in `docs/evidence/standards/2026-10-10-wc-notify1-agent/OUTCOME.md` |
| `ISSUE-188` | Delivered | A3S-Lab/Code#188 fixed: explicit remember now persists in any language. The general guarantee moved into the extraction prompt — an explicit retention request in ANY language is importance ≥ 0.90, confidence ≥ 0.95, source preference, never narration — so correctness rests on the stated property, not a matcher; the fast path stays a documented performance courtesy with a short closed multilingual verb set. Sensitive-redaction and dedupe unchanged on both levers | memory_extraction 38/0 (matcher pin with the issue verbatim phrase + negatives; prompt-rule drift pin); live: the issue exact Chinese repro as a new leg — persist, reopen, hits=2 in 4.3s — evidence in `docs/evidence/standards/2026-10-10-issue-188-remember-i18n/OUTCOME.md` |
| `ISSUE-149` | Delivered | A3S-Lab/CLI#149 in-tree half: `--resume <id>` threads end to end through the TUI scaffold (main parses, run_fullscreen carries, open_with_session uses it as the fact-log session identity — previously hardcoded tui-turn), and `--continue` parses to the DEFAULT thread, which IS this scaffold continuation by construction. The shipped-9.1.2-binary skew (missing --cwd) is release-pipeline rebuild debt (#187), not source: at HEAD the TUI already accepted --cwd/--model. The in-process scaffold was later removed; product resume is the pager plus ACP | TUI suite 23/0 (resume-resolution pin); the vendored pager mode ring and a session registry stay out of scope — evidence in `docs/evidence/standards/2026-10-10-issue-149-resume-flags/OUTCOME.md` |
| `ISSUE-150` | Delivered | A3S-Lab/CLI#150 fixed: `a3s code exec` mounts Code Intelligence on the SAME shared manifest backend (`local_with_code_intelligence_backend`, isolation scope per execution id) so `code_symbols`/`code_navigation`/`code_diagnostics` actually register — the README and the local-workspace policy that name them stop advertising ghosts. Degrade is the optional-surface rule: provider-start failure falls back to the plain backend with an ERROR-level log naming the absent tools, never silent | exec family 8/0 (new pin reads the run own `run_capability_bound` jsonl event: CI bound, or the degrade logged); policy text unchanged — the lists were the intent, the wiring was the defect — evidence in `docs/evidence/standards/2026-10-10-issue-150-exec-ci/OUTCOME.md` |
| `ISSUE-152` | Delivered | A3S-Lab/CLI#152 fixed: the installer legacy fallback is deliberate or refused. A FAILED primary-repo query with no listable stable tag now DIES with reason + remediations instead of silently proceeding from the years-old line; an intentional fallback (primary lists nothing stable, legacy has one) prints repository + tag BEFORE downloading; and the archive-member rejection carries the decision context (which repo/tag chose it, re-resolve hint). The distinction drawn: query-FAILED refuses, query-empty falls back loudly | sh -n clean; textual+parse pin (tests/install/install_resolution.sh); no network mock claimed — the arm is three guarded lines, and the incompatible-archive path is no longer silently reachable — evidence in `docs/evidence/standards/2026-10-10-issue-152-installer-loud/OUTCOME.md` |
| `MEM-RECALL-CJK1` | Delivered | The recall side of the memory i18n gap (Code#188 fixed persistence; recall missed cross-script): `query_terms` now emits CHARACTER BIGRAMS for CJK-containing tokens — the standard lexical bridge for unspaced scripts, so a differently-phrased Chinese question shares substrings with a stored Chinese memory instead of arriving as one unmatchable whole token. Latin tokens pass through unchanged; lone CJK chars stay below the floor; one function feeds BOTH InMemoryStore and FileMemoryStore. The product reaches the fix through a `[patch.Memory.git] -> crates/memory` entry (the sandbox pattern) until the upstream rev bump | a3s-memory 55/0 (bigram-emission + paraphrase-recall pins with negatives); live: new real-model leg — Chinese teach, STORE-level stage oracle across phrasing (no model), then a new session answers a differently-phrased Chinese question from recalled context only (Y9-KESTREL-CODE); full memory live suite 7/7 — evidence in `docs/evidence/standards/2026-10-10-mem-recall-cjk/OUTCOME.md` |
| `MEM-EFF-FETCH1` | Delivered | Per-prompt memory recall stops scanning the collection: `VecMemoryStore::search` (the durable-vec store, on EVERY model prompt via recall_similar) walked the ENTIRE collection to resolve ≤limit FTS hit ids — O(collection) per turn. The id-mapping is now a primary-key `fetch_with_options(&pks, None, false)` — O(limit), vectors skipped, FTS ranking restored by the existing re-sort (pinned at 203 records: right hits, ranking order, filler absent) | durable_memory_vec 5/0; search_by_tags still iterates (no pk set; not on the per-prompt path — recorded) — evidence in `docs/evidence/standards/2026-10-10-mem-eff-fetch/OUTCOME.md` |
| `RESUME-DEBT-SURFACE` | Delivered | The multiturn checkpoint_restart live red, fixed at the root: the resumed run now SURFACES its owed work — `resume_run` computes unsettled mutation paths from the fact spine and appends a directive naming the paths, the refused-narrative contract, and (when the host declared contracts) the EXACT suite commands with the suffixed-variant warning; mutation paths normalize to workspace-relative at BOTH ledger entry points (live metadata and the surface replay, one helper) so absolute spellings of the same edit yield the same digest and can actually bind; the test declares its suites (the sanctioned host manifest) | Live, clean-worktree: checkpoint_restart GREEN (resumed text "completed", suite re-run, verified, 23.6s); resume 38/0, harness_loop 36/0, fact_control 48/1 with the one failure the sibling in-flight refactor; two_distinct remains model-gated — the normal-send path lacks the debt hint (recorded as the next slice) — evidence in `docs/evidence/standards/2026-10-10-resume-debt-surface/OUTCOME.md` |
| `OWED-DEBT-STREAM` | Delivered | Every debt-carrying turn learns what it owes: the shared `owed_work_directive` (paths + refused-narrative contract + EXACT declared suite commands with the suffixed-variant warning) now rides the normal send stream too — `surface_owed_debt_prompt` appends the `[owed work]` directive when the fact spine carries unsettled mutations (the ledger replay boundary semantics), verbatim prompt when clean. The resume replay and the send stream state identical facts | Live clean-worktree: two_distinct GREEN (23.5s — the recall turn re-verifies and settles); checkpoint_restart green on rerun (23.4s); deterministic green NOT claimed — one stream-surface run still lost the compliance lottery and the gate refused (recorded; the refusal is the contract) — evidence in `docs/evidence/standards/2026-10-10-owed-debt-stream/OUTCOME.md` |
| `CONSTITUTION-XV` | Delivered | The user-ratified amendment, landed in all four governance surfaces: MEANING IS NOT MATCHED — semantic decisions (remember-intent, preference, importance) are model- or contract-made; pattern matching is admissible only where it cannot decide meaning (fail-closed narrowing, ranking/retrieval, mechanical form); a missed phrasing rides the model path, vocabularies never grow to cover it. The explicit-remember verb list is the one frozen legacy hint, backstopped by the pinned language-neutral prompt rule. Amendment evidence = the Code#188 incident (English literals dropped Chinese memories; the fix that held put semantics in the prompt layer, cross-language legs) + the full memory-layer compliance audit table | docs/development-standards.md §14, constitution XV (NON-NEGOTIABLE), root AGENTS.md Never line, apps/code AGENTS.md error-handling form — evidence in `docs/evidence/standards/2026-10-10-constitution-xv-meaning/OUTCOME.md` |
| `MONOREPO-REVIEW` | Delivered | First full-repository standards review (all trees, constitution I-XV incl. same-day XV): four HEAD-anchored deep passes + lead mechanical layer, every HIGH/MEDIUM finding lead-verified line-by-line. 3 HIGH — V1 apps/code default tool gate (adaptive() default hides non-core tools behind EN+ZH term vocabularies; a miss is unrecoverable), V2 apps/api tobot_memory production extractor (verb-list Experience classifier + lead-char Han name inference on POST /retains; sibling in-flight fix covers the entity half), V3 crates/apofasi LexicalEngine cue list deciding refund/cancel-class questions. 4 MEDIUM — V4 apps/code fallback intent table + pub-未接线 keyword APIs, V5 api SMS-throttle/IM-provider-coercion/captcha-default trio, V6 kense residue (apps/web ~208 incl. wire UA; SPIFFE CN; TS trees never audited), V7 language-ID list freezes | docs/a3s-monorepo-standards-review-2026-10-10.md is the deliverable (verdict table, verified-clean list, remediation order V1→V6); no code fixes claimed — evidence in `docs/evidence/standards/2026-10-10-monorepo-review/OUTCOME.md` |
| `V1-TOOL-GATE` | Delivered | Monorepo-review V1 (HIGH) fixed: the DEFAULT adaptive tool presentation no longer hides specialized tools behind EN+ZH term vocabularies — every registered tool is visible on every turn; the vocabularies, MCP mention-matcher, and helpers are deleted. is_standalone_conversation survives by ruling (exact-string frozen greeting set = bookkeeping; do-not-grow freeze). The economy motive, if it reappears, takes a host-declared typed tool contract — never prompt-text classification | selector 5/5, tools 719/720 (known flake), presentation 16/0; live PAR-ADOPT both arms green (6.3s/6.9s); the dirty-tree agent failures attributed to sibling in-flight via clean-HEAD worktree (green) + old-selector A/B — evidence in `docs/evidence/standards/2026-10-10-v1-tool-gate/OUTCOME.md` |
| `V3-APOFASI-CUES` | Delivered | Monorepo-review V3 (HIGH) fixed: the apofasi LexicalEngine — the shipped default when `infer` is off — no longer boosts the true side of noul questions with a hardcoded cue list (please/refund/cancel/urgent/fail/failed/twice/duplicate/threat/asap, +1.5/cue). The list is deleted; a noul question falls to the fail-closed false baseline with the XV rationale at the code site; the admissible scoring (state × host-authored option/instruction text) survives and the Choice routing pin stays green on it alone. The test pin is flipped deliberately — the refund-flavored state containing every removed cue word must NOT assert intent | apofasi 32/0 (34/0 with infer); clippy -D warnings clean; dead TokenSet::contains removed; module doc states the XV posture (lexical never classifies meaning; neural engine for intent) — evidence in `docs/evidence/standards/2026-10-10-v3-apofasi-cues/OUTCOME.md` |
| `V5-API-TRIO` | Delivered | Monorepo-review V5 fixed: SMS OTP verification now owns the throttling its port contract promises — constant-time compare, a per-phone attempt budget of 5 (TTL-bounded), the fifth wrong guess voids the code entirely, and a fresh issue clears the budget; `A3S_CAPTCHA_SECRET` refuses activation in release builds (dev default survives only under debug_assertions, the SMS-echo posture); `IM_EVENT_PROVIDER` defaults to memory only for ABSENCE — unknown values refuse naming the closed set [iggy, memory] | member family 65/0+2 (2 new pins: budget exhaustion voids even the right code; right-code-within-budget verifies and consumes; one pre-existing key-count pin updated per the triage rule — the throttle IS the port contract); bootstrap compiles with the refusals; no transport-level rate limiting claimed — evidence in `docs/evidence/standards/2026-10-10-v5-api-trio/OUTCOME.md` |
| `V6A-KENSE-RESIDUE` | Delivered | Monorepo-review V6, no-collision leg: packages/ui Logo.tsx default alt Kense→A3S (both defaults; zero test pins on it). The REST of V6 is sibling-active — recorded per the yield discipline: their in-flight diff already fixes the SPIFFE CN (Kense→A3S Trust Domain + X.509 name assertions), .env.example and apps/web (52 dirty files, 39 with kense hits, i18n/marketing copy) and the 33 cloud comment hits all ride their pass; the wire identifiers (kense.gateway.tokenizer.v0, vnd.kense.web.bundle.v1) stay coordination-tracked (external consumers), not unilaterally renamed. Durable finding: the compliance audit never scanned TS trees — that is why the residue survived the rename-complete declaration; the method note gains TS-tree scope | Logo.tsx zero-Kense verified (both defaults + no other product-code refs); sibling coverage verified from their working-tree diff, not assumed — evidence in `docs/evidence/standards/2026-10-10-v6a-kense-residue/OUTCOME.md` |
| `V7-WORDLIST-FREEZE` | Delivered | Monorepo-review V7 (advisory): both language-ID word lists — deep-research looks_like_english (report output language) and apofasi detect.rs stopwords (Latin language → checkpoint routing) — carry the constitution-XV freeze comment (the EXPLICIT_REMEMBER_VERBS pattern: admissible form classification, the list does NOT grow to cover a missed phrasing; a miss degrades to whatlang) | apofasi 32/0, deep-research 339/0+20; comment-only, no behavior change — evidence in `docs/evidence/standards/2026-10-10-v7-wordlist-freeze/OUTCOME.md` |
| `SEC-SAMPLE1-WIRE` | Planned | The final install slice — all preconditions are landed (CAP `63492b1f`, CONSENT `8becb55e`, BOUND `c2900d34`); this slice lands the wiring in ONE commit. Design (seams read 2026-10-09): ① core `McpManager` gains a per-server sampling-handler factory seam consulted at client construction (`manager.rs:530` `McpClient::new` site, threaded from `sync_global_mcp_servers` in `agent_sessions.rs:259`); ② acp spawns one consumer task owning the bridge channel: on `SamplingConsentAsk`, resolve the OWNING session by looking the server name up in the live sessions map (`live.mcp_servers`), then reuse `request_question_answer` (agent.rs:1213, options `["Allow once","Deny"]`, RejectOnce semantics) and `reply.send(answer == Some("Allow once"))`; ③ the factory composes `Bounded(ApprovalGated(Llm))` — consent bridge INSIDE the bound, per both types' docs — with the agent default-model client; ④ the guard's `API_HOME` gains the install file and the staged `dead_code` allow is removed in the same commit; ⑤ integration leg: scripted acp client answering request_permission end-to-end (the permission-test harness shape) | Not started deliberately: a half-wired sampling surface is the exact F5 risk; the slice is the day's largest single landing and lands whole or not at all — the three precondition rows and module docs carry every contract |
| `MEM-LIVE1` | Delivered | The consolidation judge verified against the real model: the pinned layer-c client (built exactly as the product builds session clients — provider config → `LlmConfig` → `create_client_with_config`) drives `LlmConsolidationJudge` through both verdict directions — a paraphrase pair judges SAME and a different fact on the same topic judges DIFFERENT, first run | live 1/1 in 2.81s, riding the existing `test_memory_store_real_llm` suite in the layer-c chain; MEM-SEM1's embedding leg recorded as host-dependent (no production embedding provider in core or the live config); GOAL-CLR1's achievement leg recorded with the analysis that an achievable goal needs the full verification-gate choreography (prose-only goals deliberately cannot authorize GoalAchieved) — evidence in `docs/evidence/durable-memory/2026-10-08-mem-live1/OUTCOME.md` |
| `DEC-1` | Delivered | Both standing decisions ratified by the user and landed: ① memory prune defaults ON (10,000 items / 180 days / importance < 0.3 — fail-closed retention; ACL `prune = "off\|none\|disabled"` closed-set off hatch, objects tune on top of the default) with the sync-build regression it exposed fixed in-pass (144-test failure was real: default-on made sync sessions refuse `AsyncSessionBuildRequired`; the optional-surface rule now degrades sync maintenance to disabled with an error-level log); ② the agent-definition format boundary ratified into constitution IX (subagent YAML + hooks.json are feature data, the SKILL.md classification, holding only under closed-set validation, per-unit content, and narrowing authority — never naming providers, altering permission policy, or joining layered overlay) | prune pins 1/1 (default posture, off hatch, garbage refusal, object tuning); sync-degrade + `v1_prune`-only registration pinned; full lib 4086/0; clippy 0 — evidence in `docs/evidence/standards/2026-10-08-decisions-landed/OUTCOME.md`; decision memo `docs/a3s-code-agent-definition-format-decision.md` |
| `FAULT-LIVE1` | Delivered | First live gate for the timeout/circuit/continuation/recovery axes on the production fact spine: an injected-fault matrix drives real sessions against a dead provider (connection refused), a stalled socket (server accepts, never writes — the exact case the detached model task + per-attempt abort exists for), and then the SAME workspace on the REAL pinned model — every fault leg fails bounded with the framework's one consistent timeout error (no hang, no third-party leak), and the real model recovers normally after both faults, proving the failure legs poison nothing | live 1/1 first run in 14.05s (A: 4.85s classified timeout; B: 5.05s stalled-socket abort; C: 4.13s real reply); registered in the layer-c chain; run-level circuit for unattended goal loops recorded as the follow-up — evidence in `docs/evidence/run-control/2026-10-08-fault-live1/OUTCOME.md` |
| `CACHE-ALIGN1` | Delivered | Prompt-cache alignment: the Anthropic-family request builder already cached system and tools; the gap was the conversation body — the bulk of a long session's prompt, re-priced in full every turn. `build_request` now marks the final message's content with `cache_control: ephemeral` (string content lifts to a block, block content marks its last block; earlier messages stay unmarked), three of the four allowed breakpoints total, and the next request's breakpoint moves forward to hit this entry as its prefix — the standard incremental pattern riding the already-proven stable boundaries | anthropic family 20/20 (2 new: final-message breakpoint with earlier messages unmarked and the exact 3-breakpoint budget counted; block content marks only its last block); full lib 4090/0; clippy 0; live cache-read oracle recorded host-dependent (pinned model is OpenAI-compatible; usage plumbing already carries cache_read_tokens) — evidence in `docs/evidence/standards/2026-10-08-cache-align1/OUTCOME.md` |
| `PATCH-MULTI1` | Delivered | Multi-file atomic patch: `patch` gains a `patches: [{file_path, diff}, …]` mode beside the legacy single-file arguments — validation is all-or-nothing (every file read and every diff applied in memory before ANY write; a bad hunk anywhere changes nothing, pinned with the good entry unwritten on the other's failure), and the write phase is sequential per-file CAS with precise partial-state reporting (written / failed / unwritten named — the backend has no cross-file transaction and pretending otherwise would hide partial state) | patch family 16/16 (2 new: all-or-nothing validation, full-batch apply with per-file metadata); full lib 4092 ran with the known manifest contention flake green solo; clippy 0; true transactional writes recorded as follow-up — evidence in `docs/evidence/standards/2026-10-08-patch-multi1/OUTCOME.md` |
| `APPROVAL-ONFAIL1` | Delivered | The on-failure approval tier (Codex's four-tier ladder, third tier): `sandbox_failure_hint` classifies sandbox-shaped failures (blocked syscall / denied filesystem / sandbox rejection / refused egress / unreachable network / refused DNS) from the combined sandboxed-command output, and a non-zero sandboxed exit with a classified shape appends a one-shot escalation SUGGESTION naming the existing approval-gated retry (`require_escalated` + justification, host-approves) plus `metadata.sandbox_failure_hint` — nothing auto-executes; plain errors and successful runs never hint (even when the output mentions a signal word in passing), and escalated runs cannot see the hint by construction | bash family 57/57 (2 new: shaped failure hints with the classified reason, plain failure and passing-signal-word success never hint); full lib 4094/0; clippy 0; the retry rides the already-pinned approval round-trip — evidence in `docs/evidence/standards/2026-10-08-approval-onfail1/OUTCOME.md` |
| `MCP-REQ1` | Delivered | The MCP stdio transport answers server→client requests instead of silently dropping them: frames with id + method are recognized as server requests and replied on the same channel — `ping` gets the spec-mandated empty result (a dropped ping made spec-conformant servers treat the client as dead), and undeclared-capability requests (sampling/roots/elicitation) fail fast with `-32601` naming the method, so nothing hangs and nothing is silent (the client declares no such capabilities, making refusal the honest contract) | pinned end-to-end with a python probe server sending both a ping and a sampling request and reporting through a tool result that the ping was replied to and sampling got -32601; full lib 4095/0; clippy 0; serving sampling (capability declaration + host bridge) and streamable_http inbound requests recorded as follow-ups — evidence in `docs/evidence/standards/2026-10-08-mcp-req1/OUTCOME.md` |
| `MCP-UNION1` | Delivered | One session's MCP sync can no longer delete another's: the agent-global manager is synced to the UNION of the config baseline and every live session's server set (LiveSession records its `mcp_servers`; `merge_server_into` merges by name, later-wins), where previously the most recent session's desired set won alone and silently removed the others' servers — the CFG-LIVE1 open item | merge pin 1/1 (later-wins by name, new name appends); full core lib 4097/0; acp 127 passed with the two known timing flakes; clippy both 0; also restored (attributed) the sibling session's capability_run_instance_id plumbing lost in a partial rewrite — evidence in `docs/evidence/config/2026-10-08-mcp-union1/OUTCOME.md` |
| `FAULT-LIVE2` | Delivered | Consecutive faults keep the session usable: the SAME session drives two back-to-back stalled-socket turns and each fails bounded with the framework timeout (one failure must not poison the session against the next — the fact spine survives a failed run and admits the next), then the real pinned model on the same workspace answers — the failure→retry→failure→recovery chain on one session, beyond FAULT-LIVE1's single-fault + new-session recovery | live suite 2/2 in 29.15s (both fault tests, real model in the loop); concurrent-send-under-fault busy contract recorded as the duplex follow-up — evidence in `docs/evidence/run-control/2026-10-08-fault-live2/OUTCOME.md` |
| `FAULT-LIVE3` | Delivered | The busy contract under fault: a send issued while a stalled turn is in flight returns in 192µs with the RunAdmission busy-reject (`already has an active operation`) — the admission CAS is immune to a stalled provider, no head-of-line blocking, no hang — then the stalled turn settles bounded, a post-settle send stays bounded, and the real pinned model on the same workspace answers | live 1/1 in 14.67s; the ACP queueing layer under fault (TurnQueue + stall combined) remains unit-pinned and recorded — evidence in `docs/evidence/run-control/2026-10-08-fault-live3/OUTCOME.md` |
| `MCP-UNION2` | Delivered | Session close resyncs the remaining MCP union: `union_mcp_desired` (a pure function — baseline ∪ every live session's servers, skipping the remodeled session whose new set arrives via `add`) is now the single desired-set computation, and `handle_session_delete` resyncs after removal so a deleted session's servers leave the global manager instead of lingering as a stale superset (resync failure warns, never fails the delete) — completing MCP-UNION1's recorded follow-up | union pin 1/1 covering the three shapes (plain union, remodel skip+add, close = exactly the remaining union); acp 128 passed with the two known timing flakes; clippy 0; duplex-live delete leg recorded as the observation-only follow-up — evidence in `docs/evidence/config/2026-10-08-mcp-union2/OUTCOME.md` |
| `ATTACH-JSON1` | Delivered | Real-LLM gates for the attachment and structured-JSON pipelines: a 1x1 PNG rides `send_with_attachments` to `ContentBlock::Image` to the provider wire on the pinned model, which answered ATTACHED (the pinned layer-c model is vision-capable; the pin also accepts an honest classified non-vision rejection — never silence or a hang), and the model drives `generate_object` with a two-required-key schema producing parseable `{"answer":42,"label":"Everything"}` — both first-run green: the pipelines were sound, the live coverage was the gap | live 2/2 in 12.14s, registered in the layer-c chain — evidence in `docs/evidence/standards/2026-10-08-attach-json1/OUTCOME.md` |
| `BWRAP-PATH1` | Delivered | Bubblewrap resolution is configurable: `platform::bwrap_candidates()` (cfg-neutral, pinned on every host) prefers the authoritative `A3S_SANDBOX_BWRAP_PATH` override (single candidate, no PATH fallback past an explicit override), then the distribution default `/usr/bin/bwrap`, then a bare `bwrap` for a PATH lookup (NixOS / Homebrew-Linux / custom prefixes — `resolve_executable` already PATH-searches single-component names); the Linux backend takes the first resolvable and its failure context names all three options | bwrap candidate pin 1/1 (default shape + authoritative override in one test — both touch the same env var); sandbox crate 207/0; clippy 0; Linux-host runtime leg left to CI behind the cfg gate — evidence in `docs/evidence/standards/2026-10-08-bwrap-path1/OUTCOME.md` |
| `MCP-SAMPLING1` | Delivered | Server-initiated sampling is served, not refused: `SamplingHandler` is the host decision point on the stdio transport (installing answers `sampling/createMessage` with the host model; not installing keeps the honest -32601 — capability follows function and installation IS the approval), `McpClient::install_sampling_handler` declares the capability in the initialize handshake only when a handler exists, and `LlmSamplingHandler` bridges the server-supplied messages to one `complete` call (system prompt through, maxTokens capped, empty payloads refused) returning the CreateMessageResult with usage | end-to-end python probe pin (handshake capability observed server-side + round-trip through a canned handler) alongside the intact refusal pin; full lib 4098/0; clippy 0; Ask-per-sample and streamable_http recorded as follow-ups — evidence in `docs/evidence/standards/2026-10-08-mcp-sampling1/OUTCOME.md` |
| `COMPACT-FIDEL1` | Delivered | Compaction fidelity on the real model: a session with a 1024-token window, 0.05 threshold, and auto-compact on forces at least one `compaction.done` cut in the fact spine (mechanical oracle) and the post-compaction recall turn answers with the exact pre-compaction codename (fidelity oracle) — planned-order item ③, the optional benchmark, landed as a live gate | live 1/1 in 33.44s, registered in the layer-c chain; in-pass wiring discovery: the fact spine's compaction knobs flow from SESSION options through `resolve_context_window` (explicit option first), and the keep-tail is window/5 so the forcing window must be small enough for the transcript to cross the prefix bar — evidence in `docs/evidence/run-control/2026-10-08-compact-fidel1/OUTCOME.md` |
| `MCP-GATE1` | Delivered | Per-sample approval off the shelf: `SamplingApprover` (one async bool — server name + a 160-char excerpt of the sampled prompt) and `ApprovalGatedSampling`, a wrapper around any inner handler where an approved request delegates and a denial fails with "sampling request denied by the host" — a JSON-RPC error the server sees, never a silent drop, never a call into the model — composable with MCP-SAMPLING1's installation-is-approval posture for the Claude-Code-parity per-request consent pattern | gate pin 1/1 (denial refuses with inner call count 0 and the refusal naming the host; approval delegates exactly once); full lib 4101/0; clippy 0; ACP request_permission wiring recorded as the seam-bridge follow-up — evidence in `docs/evidence/standards/2026-10-09-mcp-gate1/OUTCOME.md` |

The deterministic semantic gate proves serving mechanics and isolation, not
real embedding-model quality or remote backend continuity. Production claims
remain gated on independently labeled corpora and retained host reports. See
[Durable Memory Integration](manual/DURABLE_MEMORY.md),
[Durable Memory Retrieval Evaluation](docs/evidence/evaluations/DURABLE_MEMORY_RETRIEVAL_EVAL.md),
[Durable Memory Product Evaluation](docs/evidence/evaluations/DURABLE_MEMORY_PRODUCT_EVAL.md),
[Durable Memory Multilingual Evaluation](docs/evidence/evaluations/DURABLE_MEMORY_MULTILINGUAL_EVAL.md),
[Durable Memory Semantic Evaluation](docs/evidence/evaluations/DURABLE_MEMORY_SEMANTIC_EVAL.md),
[Durable Memory Semantic Refresh](manual/DURABLE_MEMORY_SEMANTIC_REFRESH.md),
[Durable Memory Multi-Agent Evaluation](docs/evidence/evaluations/DURABLE_MEMORY_MULTI_AGENT_EVAL.md), and
[Durable Memory Restart Endurance Evaluation](docs/evidence/evaluations/DURABLE_MEMORY_RESTART_ENDURANCE_EVAL.md).

### 3.6 World-class gap path

Successor slices from [a3s-code-world-class-gap-roadmap.md](../../docs/a3s-code-world-class-gap-roadmap.md)
(the 2026-10-09 gap analysis; the `TB-QUAL1` full-matrix waiver is
maintained — no full Harbor run is scheduled or used as an exit gate).

| Gate | State | Code-owned outcome | Exit criteria |
| --- | --- | --- | --- |
| `WC-MATRIX1` | Delivered 2026-10-09 | Capability matrix extended with surface axes 17–24 (interaction / ecosystem / operations), each row evidence-tagged or host-scoped, plus the P0 measurement ledger | No row claims a capability without an evidence tag |
| `WC-MEAS1` | Delivered 2026-10-09 | Provider cache hit-rate rail (`test_cache_hit_rate_real_llm`): per-turn prompt/completion/cache recording; `A3S_TEST_MODEL` routes the provider matrix | Mechanical per-turn usage oracle; not-reported recorded, never estimated; ≥1 live route REPORTED |
| `WC-MEAS2` | Delivered 2026-10-09 | Deterministic overlap oracles on the fact-path executor (`a3s-effect batch_overlap_tests`) + recorded fan-out numbers (batch=8@60ms: 66ms vs 510ms serial) | Barrier oracle green (serialization fails closed); numbers recorded, not asserted |
| `WC-INJECT1` | Delivered 2026-10-09 | Path-scoped injection wired onto the fact spine (was silently legacy-only); bounded `with_injection_budget` API (fail-closed 1..=64 KiB); hermetic pins + live A/B | Decision recorded in the matrix P0 ledger: keep 4 KiB; format change is the prerequisite for a raise |
| `WC-BG1` | Planned | Model-facing background execution as a Run-owned effect (no component timers) | Roadmap slice P1 |
| `WC-REWIND1` | Planned | User-facing rewind as append-only fork from retained content | Roadmap slice P1 |
| `WC-WTREE1` | Planned | Worktree isolation as a governed generation cutover | Roadmap slice P1 |
| `WC-HOOKS1` | Planned | Declarative hooks surface over the existing engine | Roadmap slice P2 |
| `WC-NOTIFY1` | Planned | Host-owned attention re-flow, metadata-only payloads | Roadmap slice P2 |
| `HC-CERT1` | Implementation verified; full live pending | Explicit ACL suites, bounded retries, separate first attempts and receipt reconciliation | [Runner fault evidence](../../docs/evidence/code-harness/20261010-certification-entry/OUTCOME.md); all mandatory live suites must produce reconciled results |
| `HC-BG-CLOSE1` | In progress | Refuse background admission without the existing sandbox, explicit backend support and Run ownership | No process on missing/legacy backend or failed admission; preserve observation/cleanup |
| `HC-NATIVE-CLOSE1` | macOS refusal qualified; full native execution blocked | Both NativeBashSandbox command APIs refuse before spawn with a stable lifetime reason; no command exception or automatic host fallback | [Refusal evidence](../../docs/evidence/code-harness/20261010-native-lifetime-refusal/OUTCOME.md); mandatory native-live remains explicitly unsupported. [009 component review](../../docs/evidence/code-harness/20261010-harness-planning/OUTCOME.md) accepts only nonactivated T002; T003 owned-Child handler checks pass27 and independent limited source/evidence review accepts; original T004 exact limited-negative source/local evidence independently accepted; original T005 finite nonactivated owned-only component and exact six-doc dependency/test budget independently accepted; bounded eight-port source frozen, plus one independently reviewed foreign-negative absent/unqualified initializer; revision02 narrow corrections receive root source-only review; VM branch is halted by explicit user scope correction before any compile/test/child execution, with all existing source preserved and no activation; first handler2/0 plus backend3/1 timeout, same-bin SOLO0/1 AddrInUse, fixture-only corrected backend4/0 and four exact compatibilitySOLO1/0 each; first timeout cause unproved; two actual binaries, no final aggregate6/0; no T004 production admission activation or aggregate qualification; full original-owner continuation/retirement and Box-owned descendant/late-write containment remain required before restoring support |
| `HC-GUEST-LIFE1` | macOS guest lifetime qualified on this host only | An admitted command runs in one HVF guest. The image agent starts it in a new PID namespace and that namespace is destroyed before the call returns. Probe failure keeps `native_process_lifetime_unqualified` and does not select ProcessHost. The shared-kernel adapter stays refused. Dropping an in-flight command cancels that generation only, so the next command on the same sandbox still runs; explicit shutdown stays sticky | [Guest lifetime evidence](../../docs/evidence/code-harness/20261016-macos-guest-lifetime/OUTCOME.md). This host: HVF e2e 1 passed / 0 failed (checks passed=7 failed=0), image `5d3b1ce56238513f4875ea4f39daba2ddf4d11aed7815e2f5e52cc7c1992081b`. Native refusal 2 passed / 0 failed. Guest settlement unit tests 7 passed / 0 failed. Does not qualify HC-ACCEPT1, Linux lifetime, or Windows lifetime |
| `HC-RECEIPT1` | In progress; recovery HIGH counterexamples closed, full/native/model gate blocked | Existing actor persists preparation and exact terminal receipts; uncertain execution is never blindly replayed; Core ledger is a log projection | [Spec and tasks](../../specs/006-code-durable-tool-effects/tasks.md); [retained evidence](../../docs/evidence/code-harness/20261010-durable-tool-effects/OUTCOME.md). Fresh identity/cursor/lifecycle families 145/145; behavior contracts separately 14/14; full effect 87/87. Same frozen full run 4392 passed/14 failed/29 ignored, all failures reproduced SOLO. Independent recovery review, deep-live/evaluation compile and core evaluation all-target Clippy pass; workspace fmt, semantic/native failures, delivery and complete real-model matrix remain required |
| `HC-BG-EXEC1` | In progress; native macOS support blocked | Background uses the existing BashSandbox observer and Run cancellation/settlement; native macOS admission is refused | Same isolation/egress/protected-path boundary; no descendants or late writes; qualified containment remains required, and refusal does not establish support or Linux/Windows qualification |
| `HC-ACCEPT1` | Pure contracts, read-only seam and bounded actual-byte freeze verified; native acceptance/live open | [007](../../specs/007-code-behavior-acceptance/spec.md) through existing Evaluation; host-only descriptor-relative allowlist capture and exact framed-byte identity preserve original Mate/Core authority | Historical T002 contracts14/14 and T003 facade6/6/evidence14/14 retained. [Current T005 evidence](../../docs/evidence/code-harness/20261011-candidate-byte-freeze/OUTCOME.md): evaluation-feature integration compile0;47 exact first PASS/0ignored,strict targetClippy0;937 finite inputs stable; actual512-file capture and same-length/preserved-mtime/race/link/type/cap/stale tests. Actual copied-byte snapshot independently framed; nonUTF8 macOS EILSEQ is distinct from decoder-unit coverage. Original-maker proof, store replay/verdict, native/candidate/oracle/live and full certification remain pending; no VM execution |
| `HC-EVAL1` | Planned after HC-ACCEPT1 | Fixed paired tasks and preregistered budgets, source/model/resource identities and first-attempt costs | Freeze development and held-out tasks before selection; task-cluster uncertainty and complete attempt accounting |
| `HC-SEM1` | Planned after baseline | Model/typed task obligations survive correction, revocation and two compactions | Cross-language behavior oracles pass; no regex or word-list intent classification |
| `ENG-POLICY1` | Complete for 018 task list (32/32); M7 T033 delivered: spec 018 docs and both manuals re-aligned with source (read-only analyze 0 blockers; warnings closed on re-check; Final Verification Gate item 1 checked); M7 T037 delivered: a policy session admits its digest at session build, before any Run (no-policy sessions write nothing; conflict/legacy/corrupt refuse at build); M7 T036 delivered: a goal-step Finding's subject digests target+scope and `a3s/review/settle` refuses another target/scope or a replanned step (no new field, no SDK change); M7 T038 delivered: Linux arm64 lane (`scripts/linux_arm64_lane.sh`) runs Core lib 4870/0, integration 136/139 (4 pre-M7 failures recorded), T028 8/8 and T029 1/1 real-model on the first attempt; gate items 2–5/7 closed on Linux, `crash_mid` recovery test fixed (was SessionBusy, not native); positives pending macOS native; default/benefit gates open | [018](../../specs/018-code-durable-engineering/tasks.md): T001-T032 complete;32/32=100%; T032 manual `DURABLE_ENGINEERING.md` + META_HARNESS pointer, engineering default/automatic not granted, product auto-goal off, disable/rollback recorded, Explicitly not claimed keeps HC-ACCEPT1/HC-EVAL1 open, staged empty, PG/SQL/Cloud/OpenAPI N-A; T031 adversarial review: 20 reuse rows and 5 double-control items hold; refused/replayed Pause-Cancel no longer cancel a live Run (first-red + mutation retained); Waiting kept as wire closed set; SDK `stream_goal_tick` aligned (Session 139/omissions 49) and rebuilt against final Core; SOLO triage: long_horizon×8 same as T027, ACP/shipped/v912/cursor/run_control/checkpoint same as v913/T026, not this lane; Core strict Clippy only upstream openai dead code; T030 paired 2×2 experiment registered measurement (preregistered ordinary/engineering/goal/combined on the 4 frozen tasks plus short-rule and navigation sensitivity; two full runs retained, the first with trimmed Run-event counts; protected 40/40, strict 0/32 from native refusal, callers/boundary/scope correct 24/24, reuse policy/goal arms one identical non-reference edit of unknown correctness, cost variance exceeds arm differences; no arm declared better, no benefit claim); T029 durable goal real-LLM long task registered mandatory (policy-bound goal on the T003 caller project: goal-pause interrupt mid-slice, new-Agent file reopen, two compactions; digest bound throughout, admitted 3/settled 3, no-progress bound stops at NeedsInput because the baseline check is natively refused; positive pending native); T028 engineering-discipline real-LLM suite registered mandatory (8 tests; reuse/callers/boundary/scope on the pinned model plus moved review/audit/deferral; product invariants asserted, discipline reported; edits for callers/boundary/scope byte-equal the calibrated correct references but stay pending native `native_process_lifetime_unqualified` then no-HITL escalation refusal, completion gate refused every unverified mutation; reuse failed 4/5 no edit); T027 four modes on both session stores with real reopen, plus combined budget race, policy conflict, revision race, unsettled receipt and child-in-slice counterexamples, each with zero added calls/tools/facts (test-only; 7 mutations red); T026 ACP/pager bind every bound `/goal` verb and typed `a3s/goal/*` method to the Core facade, `/goal resume` streams one Core slice through the turn path (real pager+ACP+model 2/2 after 5 retained failures); T025 Node/Python/Go expose the work policy, goal facade and fork once on the original SessionOptions/session methods/bridge (real four-mode Run 3/3 SDKs first attempt, identical policy digest; Go bridge pre-existing stack overflow fixed at root); T007 (a/b/c) and T016 (a/b) aggregates checked; T016b no production change: a child Task bills once through the parent's original BudgetGuard and the closeout keeps the parent Run's own usage; real 3-slice lifetime survives original compaction, file reopen and goal revision, tick 4 BudgetExhausted, repeated op starts nothing, closeouts+compaction equal governed tokens (3/3 after a retained test-bug failure); no monetary hard limit claimed; T007c the last step's original judgment also covers the goal's required criteria and `a3s.goal.completed` closes the goal once only when every step is accepted, no wait is open and every slice/receipt is settled (owner or no-model reconcile; real two-step goal completes 3/3, later ticks run nothing; 3 mutations red); T024 every goal slice closes once via its original owner (completed with the governed usage, else interrupted/usage unknown) or an explicit no-replay reconcile that refuses under receipt debt (real slice usage equals BudgetGuard 3/3, cut after step.accepted reconciles once with zero model calls then the next step runs; barrier-by-barrier hermetic cuts; 3 mutations red); T023 explicit engineering scenarios registered by the ACP host for policy sessions (no trigger, no auto review) and admitted Findings saved in the original SessionSnapshot; digest binds only a Run-admitted policy (real review→admit→new-host reopen 3/3 first attempts 13.9–53.3s/42–91k tokens, lifecycle across both stores, replan stales the step; retained save-refusal failure); T022 original ReviewFindingDraft admitted via `a3s/review/admit` only for the current session policy, recomputed target bytes and optional current goal step/Run (all-or-nothing, same-id other content refused, no model/permission/Goal fact; real review→admit 3/3 34.8–44.9s/58–62k tokens, stale after edit; retained no-approval failures); T021 read-only deferral-scan guidance on the original search (real scan 3/3 and zero-marker 3/3 first attempts, 24–40s/34–69k tokens, after retained round-cap and convergence failures; navigation rerun 1 retained refusal then 2 SOLO passes); T020 review/audit via the original review worker, read-only under a host policy (real review/audit 3/3 after a retained no-delegation failure) | [M1 outcome](../../docs/evidence/code-harness/20261013-durable-engineering-m1/OUTCOME.md): T013 carry retained; T014 real Goal+policy Run and new-host/file reopen pass16.07s without extra planner calls. Core326+177 invocations/ACP214 pass; strict Clippy/deep-live/format pass. Original LOOKUP correctness passes but29.3s/12 tools exceeds previous17.35s/6; paired benefit/non-regression, native and whole-product qualification remain open |
| `GOAL-FACT1` | Complete for 018 task list (32/32); M7 T034 delivered: new automatic policies capped at 1000 slices and a 30-day deadline at admission, replay unchanged; M7 T035 closed by test: a mid-slice plan revision is refused by the Run lease with zero facts, the slice settles under its plan and the next slice uses the revision; T007b/T016a qualified through T031 adversarial review and T032 manual/evidence (positives pending native; parent HC gates open) | [018](../../specs/018-code-durable-engineering/tasks.md):32/32=100%; T032 complete (manual, decisions, Explicitly not claimed, owned/staged inventory); T031 complete (reuse audit and double-control hold; control_goal validates before cancel; stream_goal_tick in three SDKs rebuilt; verify set green except upstream openai Clippy; SOLO failures pre-existing); T030 complete (descriptive 2×2 under the frozen preregistration, two runs retained; no false Verified or Completed, protected 40/40; no formal benefit conclusion while HC-ACCEPT1 is open; Core1266/ACP220, strict Clippy, deep-live); T029 complete (real interrupt/reopen/two compactions 1/1 twice, six runs retained incl. digest-less definition refused fail-closed and an outside fact-log reader failing the Run closed; no false progress or completion; Core1266/ACP220, strict Clippy, deep-live); T028 complete (8/8 registered mandatory, six real runs retained incl. first 4/8 instrumentation gap; no false Verified; Core1266/ACP220/TUI24, strict Clippy, deep-live); T027 complete (four modes × Memory/File stores reopen equal and continue; one Run per shared budget, exhaustion survives reopen; different policy, stale revision and unsettled receipt refused with zero calls; Core1266/ACP220/TUI24, strict Clippy, deep-live; long_horizon 8 failures identical to M0 native refusal); T026 complete (bound `/goal` status/pause/resume/clear through Core, `stream_goal_tick` lets the shipped pager render and confirm the slice, step-judgment refusals are logged; vendor unchanged, automatic product mode stays disabled); T025 complete (Node/Python/Go goal commands are the Core DTOs, round-trip/refusal/fork hermetic in all three, real four-mode Runs first attempt with one digest, Core1263/ACP219/TUI24, strict Clippy); T016b complete (child Task cost once via the original guard, real lifetime across compaction/restart/revision ends BudgetExhausted, repeated op inert, settled usage+compaction = governed; Core1263/ACP219/TUI24, strict Clippy, deep-live; real 3/3); T007c complete (last-step judgment covers goal criteria, `closes_goal` flag refused off the last step, `a3s.goal.completed` folded once with every step accepted, no open wait, every slice/receipt settled; owner or no-model reconcile writes it; finished-but-unclosable plan previews NeedsInput; Core1262/ACP219/TUI24; real 3/3); T024 complete (`a3s.goal.slice.settled` in the same fold, no goal_sequence advance, owner closeout after commit_goal_step, `reconcile_goal_slices` without replay, receipt debt refuses and stays unknown; corrects T019: the owner closes a cancelled slice itself; Core1258/ACP219/TUI24, strict Clippy, deep-live; real 3/3); T020 review/audit, T021 deferral scan, T022 goal-step review binding (current step and slice Run, reference only; Findings never write step.accepted) and T023 saved Findings (replan refuses the old plan revision) complete (see ENG-POLICY1); T019 complete (settled stall bound in the same continuation fold, one slice for a changed accepted plan, else NeedsInput with zero facts; pause/resume/cancel facts after the original RunControl cancel_and_settle, old wake executes nothing, an unsettled cancelled slice returns Reconcile until T024; brief ≤4KiB/512 tokens with goal text cut last; Core892/ACP217/TUI24, strict Clippy; real stall/replan/pause 3/3 first attempts 23.8–26.1k tokens; max-size goal adds 158–168 prompt tokens vs +6764 with the bound off); T018 complete (host-typed step waits between Runs, wait-aware readiness shared by preview/slice/step/judge, quiet observations append nothing; Core889/ACP217/TUI24, strict Clippy; real wait 3/3: independent step accepted, waiting tick zero calls, answer reported in the next Run, 13.8–17.8k tokens, after retained judge-step and history-drift failures fixed at source); T015 complete (tick admits at most one original Run named by operation_id, writer re-validates under its fence; Core885/ACP217/TUI24, strict Clippy; real automatic tick 3/3 first attempts 7.68–9.20s/≈9.7–10k tokens after a retained first failure and a 16.45s SOLO under the earlier tick wording) | [M1 outcome](../../docs/evidence/code-harness/20261013-durable-engineering-m1/OUTCOME.md): original slice source and same-fold counts survive replay/replan/revision/original compaction/log/session reopen; verified progress clears only since-progress. Reserved Goal-key type substitution refuses after first/SOLO counterexamples; ordinary wire unchanged, old reader refuses added source. Core786/ACP214, strict Clippy/deep-live/24-file format pass. Real ordinary exact Run/replay/reopen SOLO9.25s/3 calls/10038 tokens passes after retained unaccepted-step failure; no extra replay accounting. No automatic authority, second ledger or driver; complete T016/T015, whole-Goal/resource closure, native/SDK/pager/product and paired-efficiency gates remain open |
| `HC-MEMAUTH1` | Bounded Code memory validation: observer-ack live6/6; normal-mode matrix40/40 scenarios; full qualification open | [011](../../specs/011-code-memory-retention-authority/spec.md): validated model output or explicit host-typed memory action uses the original governed call and writer | [Retained checkpoint](../../docs/evidence/code-harness/20261010-harness-planning/OUTCOME.md). Ten existing execution routes cross both SessionStores and both MemoryStores, with actual file reopen, original five-field callbacks and ordinary-resume zero extraction. Final affected family11/11 and strict core lib/tests Clippy0; prior memory55/55 and canonical105/105 remain historical. Live positives require primary Ack before close; old immediate-close first/SOLO failures remain. ACP A6/MT-5 fixture migration and full Q/all-mode/adversarial/quality/delivery gates remain open. Only T001/T002 are checked. |
| `HC-OWNER1` | Normal-mode two-store composition verified; complete C1–C7 and delivery gates open | [012](../../specs/012-code-execution-owner-lifetime/spec.md): one original worker, guardian, TaskLease and captured Session identity | [Source and execution history](../../docs/evidence/code-harness/20261010-harness-planning/OUTCOME.md) retains first compile failures and independent reviews. Ten normal routes × four store combinations pass with original Run/FileLog/usage association, source-checkpoint preservation, prepared nonexecution and one original lease. Intermediate owner/checkpoint/memory-lifecycle24/24; final strengthened affected family11/11. Prior canonical105/105 remains historical. This continuation adds tests only; production owner and Mate actor/fold remain unchanged. Command/drop/cancel/registry/publication barriers and full C1–C7 remain separate; 010 terminal Steer settlement stays open. All eleven task boxes remain unchecked. |
| `HC-STREAM1` | Scoped working-tree verification: OpenAI adapter51/51; strict core all-target Clippy0 | Preserve every nonempty text/reasoning delta, including repetition and prefix-shaped fragments; retain explicit message snapshot compatibility | [First/SOLO reproduction and repaired family](../../docs/evidence/code-harness/20261010-harness-planning/OUTCOME.md). One private append helper and four delta call sites; host deltas and final Done agree. Fresh compile0,916 inputs stable; no full-provider, raw live wire, latency, world-ranking or landing claim. |
| `HC-INJECT2` | Planned after baseline | Compare instruction representations at the existing 4 KiB default | Measured instruction adherence and task quality exceed baseline noise within token/cost budgets |
| `HC-WTREE2` | Planned after P1 and baseline | Real isolated edits, integration and external acceptance | Conflict and non-conflict tasks, cross-tree refusal, retained dirty output, measured serial comparison |
| `HC-HOST1` | Planned on existing Studio/Use/ACP lanes | Real authorized user workflow through interruption, recovery and accountable completion | Native document versions/receipts, VSX and host-death boundaries plus real GUI evidence |


## 4. Invariants

1. Raw Tool content uses the configured shared content adapter. Evidence events
   stay bounded, and lossy projection requires an authorized immutable original
   reference.
2. A transform never destroys the original authority and never changes an
   already-persisted event during replay. Its result binds the exact algorithm
   and policy digest, and replay rejects drift from the retained Session policy.
3. Code does not receive brokered plaintext credentials when the Box egress
   broker can inject them at the authorized destination boundary.
4. Context estimates and Harness facts do not claim to be provider billing
   records; correlation with Cloud Inference usage is explicit.
5. Code snapshots are provider evidence. Cloud owns the business checkpoint
   and fork lineage that references them.
6. Code may run standalone, but Cloud-managed mode accepts exact immutable
   policy and cannot silently expand it.

## 5. Non-goals

- A Code-specific Cloud execution aggregate, scheduler, queue, node channel,
  deployment controller, approval store, or audit database.
- A durable credential store or direct handling of brokered credential grants.
- Egress network enforcement, idle suspension decisions, replica scaling, or
  public traffic routing.
- Treating Runtime logs, Flow history, or Gateway usage as the Agent semantic
  event stream.
- Non-deterministic or irreversible Tool-result rewriting in the baseline.

## 6. Workspace retrieval program

### 6.1 Outcome and authority

The Workspace Retrieval (`WSR`) program delivers fast exact, lexical,
structural, and semantic retrieval over the workspace visible to one Code
session. Semantic indexing is asynchronous, session-bound, memory-resident,
and optional. It does not require a vector database service or a durable local
database.

Code is the product capability owner. It owns workspace admission, chunking,
Embedding Provider integration, session lifecycle, incremental reconciliation,
hybrid ranking, evidence rendering, and the model-facing `search` contract.
A3S Memory owns only reusable vector-index primitives. Product hosts own
configuration and presentation. This section is the cross-repository source of
truth for that boundary; component repositories should link here instead of
creating competing lifecycle definitions.

The program preserves the existing search surfaces:

- `grep` remains authoritative for exact strings and regular expressions.
- `glob` remains authoritative for path discovery.
- Code Intelligence remains authoritative for saved-file symbols and semantic
  navigation.
- Lexical retrieval remains available when semantic retrieval is disabled,
  building, degraded, or unavailable.
- Semantic results are candidate evidence, never authority for a file's current
  contents. Code verifies returned snippets against the current workspace
  revision before exposing them.

#### Workspace retrieval backend consolidation (2026-09-04)

The workspace catalog now has one explicit lexical boundary. Product builds
use the official `a3s-vec` FTS adapter (`a3s_vec_fts_v1`) with the
whitespace analyzer; intentionally minimal builds use the separately reported
portable BM25 implementation (`portable_bm25_v1`). Both paths share Code's
admission, tokenizer, chunk identity, source-digest verification, limits, and
result contract. The native adapter batches writes, closes collections before
publication, bounds concurrent native handles, and verifies packaged libraries
per target.

Semantic retrieval is deliberately simpler: A3S Memory is the single exact
in-memory vector projection. Embeddings are validated once, published by
generation and partition, and released with the owning session. There is no
duplicate semantic index, shadow authority, or hidden backend selector. A
lexical failure can reduce lexical coverage but cannot change semantic
authority or current-source verification.

#### Workspace source snapshot authority (KRN-4 slice)

`WorkspaceSourceSnapshotV1` is the typed revision authority that binds the
manifest scan revision, an order-independent digest of the eligible file
content identity, the eligibility policy revision, the settled document/LSP
revision, the chunk catalog revision, and the derived index generation into
one canonical, tamper-evident value. Results bound to a snapshot stay
current while the manifest content is unchanged (an index rebuild alone
never invalidates them) and go stale the moment the manifest revision,
eligible content, eligibility policy, or root changes — stale results
cannot cross a source snapshot boundary. The committed qualification
proves a served chunk digest, the catalog revision the hit was served
from, and the admitting snapshot are one traceable chain, and that a
concurrent edit invalidates prior results instead of yielding a false
current answer. The manifest, catalog, and each index remain their own
authorities; consumers migrate onto the snapshot incrementally.

### 6.2 First-principles decisions

1. A normal coding workspace is small enough for exact vector scanning. A
   contiguous in-memory flat index is the baseline because it is deterministic,
   has exact recall, is easy to bound, and has no service lifecycle. HNSW or
   quantization requires benchmark evidence and a separate implementation
   behind the same contract.
2. One workspace watcher is sufficient. The retrieval runtime subscribes to
   the existing manifest snapshot and change streams; it must not start another
   recursive watcher.
3. `MemoryItem` is not a code chunk. Importance, recency, durable deduplication,
   consolidation, and pruning semantics must not leak into workspace retrieval.
4. A3S Memory stores and compares caller-supplied vectors. It never reads a
   workspace, selects an embedding model, performs network I/O, or owns a Code
   session.
5. Session creation never waits for a full index. Partial readiness is visible
   and useful, and lexical search remains the fallback while the index builds.
6. No semantic result is returned solely from a stale vector. Top candidates
   are fenced by content digest/revision when their source snippets are read.
7. Remote embedding is source-code egress. It is explicit, policy-bound,
   observable, and disabled when no admitted provider is configured.
8. Backend choices in SDK options use typed provider objects. Raw strings such
   as `vectorBackend: "memory"` are not part of the supported SDK design.
9. Source egress and ordinary workspace tools are separate capabilities. The
   embedding catalog uses an O(1)-construction read boundary that revalidates
   logical/resolved paths and hard-link count from the same open file handle;
   enabling retrieval does not silently narrow or broaden normal tool access.
10. File classification precedes chunking. Only admitted UTF-8 text reaches a
    splitter; document and media parsing remains a knowledge-compiler concern.
11. Chunking is typed and pluggable, but Code retains chunk identity and range
    validation. Host strategies return ranges, never pre-authoritative chunks.
12. RRF and overlap-aware reranking consume Code-specific ranges, identifier
    tiers, and channel evidence, so they belong in Code rather than the generic
    A3S Memory vector kernel. A neural reranker is host-injected and default-off.
13. The model-free CPU path is the product baseline: exact, glob, a3s-vec FTS/BM25,
    Code Intelligence, RRF, and deterministic MMR remain useful without an
    Embedding Provider. Dense semantic mode is an optional enhancement and must
    degrade to those paths when its provider is absent or unhealthy.
14. A local CPU embedding runtime is a host adapter behind `EmbeddingProvider`,
    not a Core or Memory dependency. It must use a revision- and digest-locked
    artifact, bounded blocking workers, explicit CPU/RSS budgets, and no model
    download or undeclared network access during session construction.

### 6.3 Target architecture

```text
LocalWorkspaceManifest ── snapshots/changes ─┐
                                             v
WorkspaceFileSystem ── admitted reads ─> WorkspaceRetrievalRuntime
                                             │
                              ┌──────────────┴──────────────┐
                              v                             v
                       Shared ChunkCatalog          EmbeddingProvider (local CPU or remote)
                              │                             │
                    ┌─────────┴─────────┐                   v
                     v                   v          InMemoryVectorIndex
             a3s-vec FTS         source evidence       (a3s-memory)
                    │                   │                   │
query ─> exact/symbol/lexical/semantic candidate generation ┘
                    │
                    v
          reciprocal-rank fusion + diversity
                    │
                    v
        current-content verification ─> bounded snippets
```

The `ChunkCatalog` is the session's single source of truth for searchable text
chunks. It prevents semantic retrieval and BM25 from maintaining different
chunk boundaries or repeatedly reading the same unchanged file. Each chunk
contains a stable identifier, workspace-relative path, line range, language,
optional symbol context, content digest, file revision, and bounded text.

The framework also provides a workspace-owned persistent FTS projection. The
legacy `WorkspaceServices::local_with_indexed_retrieval` constructor remains a
compatibility convenience; default local Agent workspaces configure the same
projection automatically. It uses the same manifest watcher and catalog
admission policy, publishes versioned a3s-vec generations under `.a3s-code/index`,
and transparently accelerates the model-facing `search` `bm25` route. An
unavailable or read-only cache falls back to the catalog, with no model-visible
mode change. This projection is lexical only; A3S Memory remains the session
semantic authority. CPU-heavy tokenization and per-document normalization use
Rust's bounded Rayon worker pool with stable input order; native generation
publication remains serialized and atomic. During automatic cold admission,
the catalog uses the portable scorer as a verified fallback so one native
collection is not opened per source file; the workspace-wide a3s-vec generation
becomes the serving path once ready. MCP is an optional external adapter and
is not a Core dependency.

The compatibility chunker is deterministic and language-independent, with line
and UTF-8 byte ceilings. Fixed UTF-8 byte windows and recursive prioritized
separators add bounded overlap; a trusted Rust host can return custom token,
syntax-tree, or domain ranges. Code validates complete coverage, forward
progress, UTF-8 boundaries, and size/count budgets before computing stable IDs,
line anchors, and digests. Code Intelligence may later provide symbol
boundaries as an optional enhancement, but indexing must not wait for an LSP
server and must produce equivalent fallback chunks when it is unavailable.

### 6.4 Subproject ownership

| Subproject | Owns | Must not own |
| --- | --- | --- |
| `a3s-memory` | Public `VectorIndex` contract, vector/result types, exact in-memory implementation, dynamic dimensions, atomic partition replacement/removal, immutable query snapshots, deterministic ordering, and memory budgets | Workspaces, files, code chunking, embedding clients, model configuration, session lifecycle, hybrid/rerank policy, or prompt context |
| `a3s-code-core/workspace` | Text admission, typed/custom chunk strategies, shared `ChunkCatalog`, a3s-vec FTS lexical projection, optional workspace-owned persistent generations, manifest reconciliation, path/revision metadata, and structured `WorkspaceRetrieval` provider contract | Non-text parsing, provider credentials, host UI, or semantic vector persistence |
| `a3s-code-core/embedding` | Host-injected `EmbeddingProvider` contract, provider descriptor, batching, cancellation, bounded retry, and normalized embedding errors | Vector storage or workspace traversal |
| `a3s-code-core/session` | `WorkspaceRetrievalRuntime`, asynchronous construction, prioritization, query-time promotion, cancellation, close/replace/resume behavior, and session isolation | Process-global mutable indexes or hidden persistence; workspace FTS generations belong to the workspace layer |
| `a3s-code-core/tools` | `semantic`/`hybrid` search modes, a3s-vec FTS lexical fallback, RRF fusion, bounded overlap-aware reranking, path filters, source anchors, and coverage/status metadata | A second chunker or direct filesystem traversal outside `WorkspaceServices` |
| Code SDKs | Typed retrieval/chunking options, typed Embedding Provider injection, status/result DTOs, and lifecycle parity across Rust, Node, Python, and Go | Primitive strategy/backend names or SDK-specific ranking behavior |
| CLI/TUI and other hosts | ACL wiring, opt-in controls, readiness/degraded presentation, diagnostics, provider-secret handling, and optional local CPU provider adapters/model-artifact admission | Reimplementing indexing, placing model runtimes in Core/Memory, or making a host-specific search protocol |
| Tests, benchmarks, and docs | Shared relevance fixtures, adversarial lifecycle tests, performance baselines, examples, and operator guidance | Production-only correctness assumptions that cannot be tested deterministically |

### 6.5 Component contracts

#### A3S Memory vector kernel

A3S Memory adds a `VectorIndex: Send + Sync` capability next to, not inside,
`MemoryStore`. The first implementation is `InMemoryVectorIndex`. Its public
contract supports:

- a descriptor fixed at index construction time containing dimension,
  similarity metric, normalization rule, and byte/record budgets;
- batch replacement of one logical partition and atomic removal of a
  partition; Code maps one partition to one workspace file;
- exact top-k search with optional caller-defined labels/partition filters;
- an immutable revision and status snapshot returned with every query;
- explicit rejection of dimension mismatch, non-finite values, and invalid
  zero vectors for metrics that require normalization;
- deterministic tie-breaking by record identifier;
- `clear`, record/byte accounting, and bounded allocation failure;
- concurrent searches over an immutable snapshot while a replacement is built
  off-lock and atomically published. Each partition owns an `Arc`-backed
  contiguous vector block, so publication shares unchanged partitions and does
  not copy the full vector corpus for a one-file update.

The baseline stores normalized `f32` vectors contiguously and uses exact dot
product for cosine search. CPU-heavy scans run outside Tokio's async workers.
The implementation does not spawn an immortal background task and releases all
memory when its owning session drops it.

The existing SQLite/FTS and optional `sqlite-vec` memory backend are not the
baseline for WSR. A future SQLite adapter may implement `VectorIndex`, but WSR
must not depend on it, its current fixed dimension, or a native extension.

Suggested source layout:

```text
src/vector/
├── mod.rs
├── index.rs
├── types.rs
└── in_memory.rs
```

#### Code retrieval core

Code adds a structured `WorkspaceRetrieval` capability to `WorkspaceServices`
rather than extending the display-oriented `WorkspaceSearch::grep` response.
The provider returns paths, ranges, digests, revisions, per-channel ranks, and
coverage metadata. `SearchTool` remains the single model-facing search tool.

The local implementation owns these internal components:

```text
core/src/workspace/retrieval/
├── mod.rs                 # public provider boundary
├── types.rs               # requests, hits, status, revisions
├── chunk.rs               # deterministic bounded chunking
├── chunking_strategy.rs   # built-in and host range splitters
├── catalog.rs             # shared immutable chunk snapshots
├── lexical.rs             # typed a3s-vec/portable FTS projection
├── semantic_runtime.rs    # embedding queue and vector partitions
├── hybrid_rank.rs         # RRF and deterministic diversity
└── rerank.rs              # bounded deterministic second-stage reranker
```

The manifest owns the preceding text/non-text decision in
`core/src/workspace/manifest/file_kind.rs`. The chunker never receives a
non-text asset.

The compatibility BM25 path selects candidates, reads files, and creates
bounded 80-line chunks. Both the incremental catalog and the no-catalog
fallback now hand their normalized token stream to the same bounded lexical
projection; product builds use a3s-vec FTS and minimal builds use portable
BM25. Code retains candidate policy, source verification, and rendering. The
model-facing BM25 result shape and source anchors remain stable.

The existing default session path currently constructs plain local workspace
services without a manifest. When WSR is enabled for a local session, session
capability construction must instead create or reuse one manifest-backed local
backend and share it with retrieval and Code Intelligence. If a host supplies
custom or remote `WorkspaceServices`, Code does not bypass that abstraction:
semantic modes appear only when the host also supplies a structured
`WorkspaceRetrieval` provider. The initial WSR release is therefore local-first
without making local filesystem access part of the public retrieval contract.

#### Embedding Provider

The provider boundary accepts bounded batches and a cancellation token and
returns vectors plus an immutable descriptor containing provider identity,
model identity, dimension, and normalization contract. The runtime rejects a
descriptor change within one index generation and rebuilds explicitly when the
configured model changes.

The first implementation may be an admitted OpenAI-compatible embeddings
adapter, but the interface remains host-injectable so a local model can be used
without changing retrieval. A3S Memory has no dependency on this adapter.

Remote providers must receive only chunks admitted by Code's embedding egress
policy. Sensitive configuration, credential files, private keys, generated
trees, non-text assets, oversized files, and workspace-private control directories
are excluded by default. Neither content nor vectors are written to logs.

#### Text retrieval versus knowledge compilation

The workspace index and the knowledge compiler are separate systems with an
explicit ownership boundary:

| Component | Owns | Must not own |
| --- | --- | --- |
| A3S Code manifest/catalog | Conservative text classification, full UTF-8 validation, source chunking, lexical metadata, source revision and digest fencing | PDF/Office parsing, OCR, image understanding, archive expansion, media transcription |
| A3S Code semantic runtime | Session scheduling, admitted embedding calls, partial readiness, file-atomic vector publication, verified retrieval | Durable knowledge indexes or implicit ingestion of generated parser output |
| A3S Memory | Bounded exact in-memory vector storage/search and lifecycle accounting | Workspace traversal, file typing, chunking, model SDKs, or document parsing |
| CLI/host | Explicit enablement, provider injection, source-egress authorization, budgets, and status presentation | Inferring egress permission from the configured chat model |
| Separate knowledge compiler | Parse/OCR/transcribe/normalize non-text assets and publish provenance-bearing text artifacts | Mutating a live Code session's private vector index |

A future handoff from the knowledge compiler requires a typed, versioned
artifact contract containing source identity, compiler identity/version,
content digest, provenance, and trust policy. Until that ADR exists, Code skips
non-text assets and does not auto-discover compiled derivatives.

### 6.6 Session lifecycle and consistency

1. Session creation constructs the runtime and returns immediately. Disabled or
   unconfigured retrieval adds no background work.
2. The runtime observes the first manifest snapshot and schedules eligible
   files. Recently touched, changed/untracked, and query-promoted files receive
   priority without excluding the rest of the admitted corpus.
3. Each file is read through `WorkspaceServices`, chunked once, and committed
   to the catalog. Lexical data becomes ready immediately; semantic data becomes
   ready after its embedding batch succeeds.
4. Completed files are atomically published as vector partitions. Queries may
   use completed partitions while the rest of the workspace is still building.
5. A changed or deleted path is tombstoned before replacement work begins.
   Results also compare the stored content digest with the text read for the
   final snippet and discard mismatches.
6. A lagged change receiver marks the runtime degraded and reconciles against
   the latest full manifest snapshot. It never silently declares full coverage.
7. Provider timeout, rate limiting, invalid vectors, or memory exhaustion
   affects semantic coverage only. Exact and lexical retrieval remain usable.
8. Closing, replacing, or cancelling a session cancels queued reads and
   embedding requests, joins owned tasks within a bounded deadline, and drops
   the vector index. Resuming a session rebuilds from the current workspace; no
   ephemeral vector state is serialized into `SessionSnapshotV1`.

`WorkspaceRetrievalStatus` exposes at least `disabled`, `building`, `ready`,
`degraded`, and `closed`, together with workspace/index revisions, eligible and
indexed file/chunk counts, coverage, queue depth, failure counts, memory bytes,
and model identity. Search output reports the status/revision that produced its
hits so partial semantic coverage cannot masquerade as a complete search.

### 6.7 Retrieval and ranking policy

The query planner generates independent bounded candidate lists:

| Channel | Best use | Baseline behavior |
| --- | --- | --- |
| Exact | Identifiers, literals, regexes | Existing `grep`; strongest signal for exact matches |
| Path | File/module discovery | Existing `glob` and catalog path terms |
| Lexical | Multi-term repository concepts | Incremental BM25 over the shared chunk catalog |
| Structural | Types, functions, definitions, references | Existing Code Intelligence symbol/navigation services |
| Semantic | Paraphrases and vocabulary mismatch | Query embedding against ready vector partitions |

Hybrid ranking uses reciprocal-rank fusion over channel ranks instead of adding
raw BM25 and cosine scores, which are not calibrated to one another. The
current first stage protects exact identifiers, applies deterministic
path/range tie breakers, and returns at most two chunks per file. Exact
identifier matches cannot be displaced solely by semantic similarity.

Configurable overlap makes a second, in-memory stage necessary: RRF deduplicates
chunk IDs but cannot recognize two windows that repeat the same source span or
boilerplate. `CODE-R2` reranks only a bounded fused pool. Its deterministic
baseline combines interval overlap, normalized lexical shingles, channel
agreement, and MMR-style diversity while preserving the exact-identifier tier.
It is deterministic across process runs, allocates from a checked scratch
budget, and falls back to the unchanged RRF order on invalid settings or
budget failure.

The deterministic reranker remains Code-owned because it consumes source
ranges and retrieval-channel evidence. An optional
`WorkspaceReranker: Send + Sync` host port may later admit a local
cross-encoder over only the bounded top candidate pool. It is disabled by
default, has explicit model identity,
timeout, cancellation, memory and source-egress policy, and cannot make search
unavailable when it fails. Code will not add ONNX, model downloads, or a remote
rerank call to the baseline without benchmark evidence and a separate ADR.

The initial tool rollout adds `semantic` for diagnosis/evaluation and `hybrid`
for normal natural-language retrieval. Those modes appear in the dynamic tool
schema only when the required provider exists. `hybrid` may operate with
partial semantic coverage, but its metadata must identify which channels ran,
their coverage, truncation, and any fallback reason. Existing `grep`, `glob`,
and `bm25` arguments and permissions remain compatible.

### 6.8 Configuration and SDK shape

Retrieval is disabled unless the host explicitly supplies an admitted
Embedding Provider or enables a supported provider block in ACL. The default
vector implementation is an internal implementation detail, not a stringly
typed user choice.

Configuration separates:

- enablement and build/query budgets;
- embedding provider/model/batch limits;
- egress admission and exclude/include rules;
- typed chunk strategy, overlap, custom separators, and corpus limits;
- memory byte/record budgets;
- search channel, fusion, candidate-pool, rerank-time, and scratch limits.

ACL remains the product configuration format. Provider secrets use existing
secret resolution and are never copied into persisted session data. SDKs expose
typed `WorkspaceRetrievalOptions` and provider objects; they do not accept raw
backend or chunk-strategy names. Rust hosts may inject a custom range splitter;
Node, Python, and Go first expose typed built-in strategy objects and recursive
separator lists. A safe callback lifecycle for cross-language custom splitters
is a separate gate. All SDKs preserve the same defaults, validation errors,
status states, and close semantics before the feature is declared stable.

### 6.9 Delivery gates and dependency order

Current implementation status:

| Gate | Status | Evidence |
| --- | --- | --- |
| `WSR-00` | Delivered | Versioned relevance and lifecycle fixtures, native BM25 CI baseline, reference sizing profile, locked budgets, and adversarial trust-boundary review |
| `MEM-V1` | Delivered | A3S Memory `main` commit `3293f572` adds the public exact ephemeral vector kernel, streamlines the contiguous exact-scan hot path, and passes default, SQLite-feature, oracle, concurrency, budget, cleanup, benchmark, Clippy, and rustdoc gates |
| `CODE-C1` | Delivered | Session-local immutable chunk catalog, conservative sensitive-path eligibility policy, UTF-8-safe deterministic chunking, a3s-vec FTS/BM25 postings with a portable minimal-build path, async manifest reconciliation, stale-content tombstones, lag rebuild, and query-time zero-read catalog path; lifecycle, locked relevance, concurrency, budget, cleanup, failure-injection, and strict Clippy gates pass |
| `CODE-P1` | Delivered | Explicit workspace-owned persistent a3s-vec FTS generations, atomic `CURRENT` publication, cross-process publication/read/delete fencing, restart reopen, stale-revision fencing, shared manifest coordinator, bounded retry/backoff, obsolete-generation collection, building status, `.a3s-code` source exclusion, transparent `bm25` acceleration, portable cold-admission fallback that avoids one native collection per file, same-content generation reuse, adaptive Rayon multi-core tokenization/posting construction with stable ordinals, and native/portable release scale, concurrency, rebuild, cleanup, and restart qualification pass |
| `CODE-P1` | Delivered | Explicit workspace-owned persistent a3s-vec FTS generations, atomic `CURRENT` publication, restart reopen with schema-v2 chunk-payload/identity integrity checks, stale-revision fencing, shared manifest coordinator, bounded retry/backoff, obsolete-generation collection, building status, `.a3s-code` source exclusion, transparent `bm25` acceleration, portable cold-admission fallback that avoids one native collection per file, same-content generation reuse, adaptive Rayon multi-core tokenization/posting construction with stable ordinals, and native/portable release scale, concurrency, rebuild, cleanup, and restart qualification pass |
| `CODE-G1` | Delivered | Default `local-code` enables in-tree trigram candidate pruning for workspace `grep` (`grep-trigram`): fail-open literal filter under `.a3s-code/grep-trigram`, auto-rebuild on manifest version change, host override via `configure_grep_candidate_index`, crates.io-safe (no unpublished `tgrep-core`), and tool-level proof that grep never opens durable a3s-vec FTS |
| `CODE-C2` | Delivered | Rust Core adds compatible line, fixed UTF-8 window, recursive prioritized-separator, and host-injected custom range strategies; Code validates complete coverage and budgets, owns IDs/digests/lines, charges overlap memory, contains host failures, and wires explicit configuration into session-owned catalogs without allowing silent overrides of host-owned catalogs |
| `CODE-E1` | Delivered | Host-injected `EmbeddingProvider`, immutable descriptor, deterministic text/vector-budgeted batching, caller-order restoration, cancellation/timeout propagation, typed bounded retry, response validation, panic containment, redacted diagnostics, and deterministic fake-provider gates |
| `CODE-S1` | Delivered | Typed `WorkspaceRetrievalOptions`, async session-owned catalog projection, Memory `3293f572` exact-vector partitions, pre-replacement tombstones, superseded-generation fencing, partial/degraded status and coverage, build-failure cleanup, and bounded idempotent close |
| `CODE-Q1` | Delivered | Structured semantic search through the unified `search` tool, bounded query embedding, immutable catalog/vector revision fencing, current-file digest and byte-range verification, coverage metadata, cancellation, and explicit fallback |
| `CODE-H1` | Delivered | Exact literal, a3s-vec FTS/BM25, optional Code Intelligence symbol, and positive-similarity semantic candidates are fused by deterministic RRF (`k=60`); exact identifiers are protected, results are capped at two chunks per file, source is reread once per selected path, stale hits are filtered, and every channel reports bounded status/fallback metadata |
| `SDK-R1` | Delivered | Rust, Node, Python, and Go expose typed provider/options boundaries, cancellation propagation, status, and verified semantic/hybrid DTOs. Go bridge protocol v2 adds callback cancellation; unit, race, and real Go-to-Rust lifecycle E2E gates pass |
| `SDK-C2` | Delivered | Node, Python, and Go expose typed line/fixed/recursive strategy objects and recursive separator lists while omission preserves line chunking. A shared Core-owned fixture locks identical byte ranges and invalid windows; primitive names are rejected, Go validates before callback registration, the bridge revalidates typed one-of blocks, and arbitrary custom splitters remain on the Rust host boundary. Native Node/Python and real Go-to-Rust multi-chunk integration gates pass |
| `SDK-R2` | Delivered | Node, Python, and Go expose typed deterministic-reranker objects while omission preserves RRF-only. SDK/Core defaults and hard bounds align, primitive algorithm names are not accepted, invalid settings fail before provider calls or Go callback registration, result DTOs report versioned evidence, and native Node/Python plus real Go-to-Rust bridge integration gates pass |
| `HOST-R1` | Delivered | A3S CLI `main` commit `53821c8` adds default-off ACL wiring, a separate OpenAI-compatible embedding route, trusted-layer egress enforcement, bounded/redacted HTTP behavior, and session injection across exec and TUI rebuilds. It pins Code `47770057` and Memory `3293f572`; retrieval-focused tests pass `71/71`, the final post-pin filter passes `19/19`, all targets and Clippy compile, the release build passes, and the full Windows suite adds no failures relative to CLI baseline `f4377c2` |
| `HOST-C2` | Delivered | A3S CLI `main` commit `b79df10` introduced trusted typed `line`, `fixed_window`, and `recursive` ACL blocks. Follow-up `d1c8c25` configured each shared manifest catalog exactly once and kept catalog settings out of per-session options across exec and TUI. `f435950` now pins Code `bdb86e17`, projects schema-v2 batching evidence, and passes the real DeepSeek ACL-host rerun at 1.0x amplification. Omission preserves line chunking and default-off retrieval; primitive/custom selectors, workspace-layer overrides, duplicate/mixed/unknown blocks, and invalid Core-owned limits fail before provider resolution or source egress. The current retrieval filter passes `29/29`; locked all-target check, format, and change-scoped Clippy pass with only known CLI baseline warnings |
| `HOST-R2` | Delivered | A3S CLI `main` commit `c8024e6` pins Code `47337f03` and adds a trusted, default-off typed `deterministic_reranker` ACL block. Omission preserves RRF-only; primitive selectors, workspace-layer overrides, duplicate/unknown blocks, and invalid Core-owned limits fail before provider resolution or source egress. `a3s config show` reports active/requested mode, the versioned algorithm, and non-sensitive limits. Retrieval tests pass `24/24`, authority-overlay tests `5/5`, and real CLI config contracts `2/2`; locked build, format, and change-scoped all-target Clippy gates pass |
| `WSR-QA` | Delivered | Locked quality, adversarial egress/race/isolation/confidentiality/lifecycle suites, strict Core/Node/Python/Go bridge Clippy, the post-`CODE-B2` complete serial Core suite (`2777/0/18`), release benchmark runs, final host release build, and DeepSeek tool-loop E2E passes. Exact p95 is 8.294/12.302 ms and the original qualified hybrid p95 is 51.145/54.429 ms |
| `WSR-EVAL1` | Delivered | Real `deepseek/deepseek-v4-pro` paired ablation passes enabled 3/3 versus disabled 0/3, Recall@5/MRR 1.0, a target beyond the 80-line boundary, 30 text files/31 chunks, three excluded non-text assets, zero non-text provider inputs, complete post-close release, and schema-v2 `CODE-B2` document-request amplification of 1.0x |
| `CODE-B2` | Delivered | A session-local coordinator coalesces one immutable catalog generation across files and flushes on the earliest input, text-byte, vector-byte, or generation-complete boundary. Stable IDs, revision/digest fencing, cancellation, private split-file accumulation, file-atomic publication, partial readiness, and already-published sibling survival are covered by eight adversarial tests. Current-generation batching metrics are exposed by Core, Node, Python, Go, and the CLI host. The 31-chunk paired task, 55-chunk collision task, every strategy arm, the 39-chunk ACL-host task, and the 25,000-record release profile all report 1.0x request amplification; the release profile emits 391 requests for a 391-request lower bound with 9-10 ms time to first ready publication |
| `CODE-R2` | Delivered | Rust Core adds an opt-in deterministic MMR v1 stage after pure RRF with exact-tier protection, interval/lexical near-duplicate scoring, stable tie breaking, two-results-per-file diversity, 100-candidate/4-KiB/128-fingerprint/4-MiB ceilings, unchanged-order RRF fallback, and versioned diagnostics across Rust/Node/Python/Go results. Locked Recall@10/MRR/nDCG@10 are 1.0 with zero selected duplicates; two release runs report -5.163/-2.322 ms signed end-to-end p95 differences (0/0 ms positive addition), 75,346 accounted scratch bytes, and zero fallback. The default-line adversarial DeepSeek slice improves completion and Recall@5 from 0/3 to 3/3 while reducing Top-5 collision evidence from 15/15 to 10/15. RRF-only remains default |
| `CODE-RDY1` | Delivered | Core adds an opt-in event-driven semantic-readiness barrier with a 30-second hard ceiling. The zero-duration default preserves immediate partial fallback; ready/degraded publication wakes waiters, timeout retains `building`, and caller/session cancellation interrupts the wait without blocking session construction |
| `WSR-EVAL2` | Delivered | The Core rerank adversary, built-in strategy matrix, Rust custom negative control, real CLI ACL host, and public SDK real-model matrix are complete. Code `cde887b` locks one corpus/report contract across Node.js, Python, and Go; each SDK completes `3/3` exact tasks and one-Search protocols with Precision@5 `0.2`, returned-result precision `0.4286`, Recall@5 `1.0`, MRR `0.5`, nDCG@5 `0.6309`, 39 vectors/9,595 bytes per session, 1.0x document-request amplification, zero non-text inputs, and complete release. The 2026-08-17 `v7.0.1` post-release rerun at Code `5aa9642` repeated all nine SDK tasks and protocols with the same quality, egress, amplification, and release results, while the Core DeepSeek adversarial suite passed `3/3`; Node.js, Python, and Go consumed 14,540, 14,784, and 14,171 model tokens. Remote timing remains diagnostic, the whole-file control remains unqualified, and no default change is justified |
| `WSR-PROD1` | Delivered | Code `beac7cb` qualifies the revision-locked 384-dimensional multilingual CPU model with RRF Recall@5 `1.0`, MRR `0.5`, 1.0x amplification, zero non-text inputs, and complete release; the English model remains a CJK negative control and deterministic MMR remains optional after lowering MRR to `0.3444`. CLI `5a27e81` passes the same model through trusted ACL and the real loopback HTTP adapter into DeepSeek at `3/3`, with `435/454` ms p50/p95 first-ready publication and a strict UTF-8 process boundary. Code `eddeeea` then passes 9/9 compile-gated generation trials across three tasks: pass rate `1.0`, 95% Wilson lower bound `0.7008`, tool/evidence/compile/integrity/release `1.0`, 1.0x amplification, zero non-text inputs, `402/919` ms initial-publication/full-ready p95, and `40` ms edited-generation publication p95. The 64-generation replacement soak retains one live vector and releases zero/zero records/bytes on close; [CI #249](https://github.com/A3S-Lab/Code/actions/runs/31862118069) passes it on Ubuntu, macOS, and Windows together with all Code checks. The versioned SLO, telemetry, and configuration-only rollback runbook is delivered |
| `HOST-LCPU1` | Delivered | CLI `main` commit `e03b06e` qualifies the opt-in FastEmbed/ONNX CPU adapter while the default feature graph remains model-free and contains no FastEmbed/ORT dependency. Trusted typed ACL, revision/SHA-256-bound offline admission, lazy bounded blocking inference, two-input microbatching, one-model/one-native-job process ceilings, sanitized failures, stable unsupported-platform/x86-64-v3 diagnostics, cancellation recovery, and the Core readiness barrier are delivered. Native CI performs real offline inference on Linux x64/ARM64, Windows x64, and macOS ARM64; [CLI CI #31917686424](https://github.com/A3S-Lab/CLI/actions/runs/31917686424) passes every job. The Windows multilingual gate records 7,045/19 ms cold/warm calls, 0 ms caller cancellation, 267 ms recovery, and a 1,018,519,552-byte peak-RSS increase below 1 GiB. Real DeepSeek completes 3/3 tasks with Recall@5 1.0, MRR 0.3444, nDCG@5 0.5059, exact 1.0x lower-bound request amplification, and zero non-text inputs. RRF-only remains default because deterministic MMR did not improve this corpus |
| `WSR-OPT1` | Delivered | Bounded chunk deltas build the next persistent a3s-vec generation incrementally (copy-forward + engine delete/upsert) instead of rewriting every posting; manifest schema v3 persists engine primary keys so sparse delta ordinals reopen exactly | A single-file edit costs 109.0 ms vs 223.9 ms full rebuild at 2048 files (2×; 62.8 vs 66.5 ms at 64 where copy fixed cost dominates), same-content reuse stays 0.6/11.8 ms, query p95 unchanged; edit/rename/churn-fallback/missing-source/reopen pins pass (`137/0` retrieval family, `4003/0` lib), the live cycle trio drives the delta path with the real model `3/3`; any incremental failure fails closed to the full build and reopen-time digest verification is unchanged — evidence in `docs/evidence/workspace-retrieval/2026-10-07-wsr-opt1/OUTCOME.md` |
| `WSR-DOC` | Delivered | README, changelog, baseline, operator QA report, DeepSeek task evaluation, SDK examples, ACL host guidance, text/knowledge-compiler boundary, privacy boundaries, final revisions, and release disposition are aligned; obsolete query-time-BM25 and sqlite-vec guidance is excluded |

The detailed baseline and threat model are in
[`manual/WORKSPACE_RETRIEVAL_BASELINE.md`](manual/WORKSPACE_RETRIEVAL_BASELINE.md).
Release measurements and adversarial evidence are in
[`manual/WORKSPACE_RETRIEVAL_QA.md`](manual/WORKSPACE_RETRIEVAL_QA.md).
The paired real-model task and batching evidence is in
[`docs/evidence/evaluations/WORKSPACE_RETRIEVAL_DEEPSEEK_EVAL.md`](docs/evidence/evaluations/WORKSPACE_RETRIEVAL_DEEPSEEK_EVAL.md).
The chunk strategy and rerank boundary is in
[`manual/WORKSPACE_RETRIEVAL_CHUNKING.md`](manual/WORKSPACE_RETRIEVAL_CHUNKING.md).
The lexical/semantic backend contract and package qualification are in
[`manual/WORKSPACE_RETRIEVAL_BACKENDS.md`](manual/WORKSPACE_RETRIEVAL_BACKENDS.md).

| Gate | Owner | Depends on | Deliverable | Exit criteria |
| --- | --- | --- | --- | --- |
| `WSR-00` | Code core/tests | None | Versioned retrieval fixture corpus, current BM25 baseline, sizing data, threat model, and locked quality/latency budgets | Baseline is reproducible in CI and separates identifier, paraphrase, CJK, and lifecycle cases |
| `MEM-V1` | A3S Memory | `WSR-00` contract draft | Public vector types/trait and `InMemoryVectorIndex` | Contract, oracle, concurrency, invalid-input, budget, and cleanup tests pass without SQLite features |
| `CODE-C1` | Code workspace | `WSR-00` | Shared chunk catalog, eligibility policy, deterministic chunker, a3s-vec FTS lexical postings, and manifest reconciliation | Unchanged files are not reread; create/change/delete/rename and lag recovery are deterministic |
| `CODE-C2` | Code workspace | `CODE-C1` | Typed built-in chunk strategies, validated Rust custom range port, overlap accounting, and session catalog configuration | UTF-8, gaps, progress, size/count, panic, ownership, deterministic-ID, and async session tests pass; the default line strategy is unchanged |
| `CODE-E1` | Code model/session | `WSR-00` | Host-injected Embedding Provider contract, batching, cancellation, and typed errors | Deterministic fake provider proves dimensions, cancellation, retry bounds, and descriptor changes |
| `CODE-S1` | Code session | `MEM-V1`, `CODE-C1`, `CODE-E1` | Asynchronous session retrieval runtime and vector partition lifecycle | Session creation does not wait; partial readiness works; close drops all owned tasks and memory |
| `CODE-Q1` | Code tools | `CODE-S1` | Structured semantic search, verified snippets, status/coverage metadata, and fallback | No stale/deleted hit is rendered and existing search modes have no behavior regression |
| `CODE-H1` | Code tools/intelligence | `CODE-Q1` | Exact, BM25, symbol, and semantic candidate fusion | Hybrid meets locked quality gates and preserves identifier precision |
| `SDK-R1` | Code SDKs | `CODE-S1`, `CODE-Q1` | Rust/Node/Python/Go typed options, status DTOs, lifecycle parity, and examples | SDK alignment checks and language-specific integration tests pass |
| `SDK-C2` | Code SDKs | `CODE-C2` | Typed line/fixed/recursive chunk options in Node/Python/Go | Cross-SDK fixtures produce identical ranges and reject identical invalid configurations; no primitive strategy-name option is accepted |
| `SDK-R2` | Code SDKs | `CODE-R2` | Typed deterministic-reranker option objects in Node/Python/Go | Cross-SDK options preserve Core defaults and bounds, fail before provider calls, and never accept a primitive algorithm name |
| `HOST-R1` | CLI/TUI hosts | `SDK-R1` | ACL wiring, readiness/degraded diagnostics, and explicit enable/disable controls | A user can identify disabled, building, partial, ready, and degraded states without debug logs |
| `HOST-C2` | CLI/TUI hosts | `SDK-C2` | ACL chunk strategy, overlap, separator, and budget configuration | Omitted config preserves line defaults; invalid config fails before source/provider egress; effective non-sensitive settings are observable |
| `HOST-R2` | CLI/TUI hosts | `SDK-R2` | ACL reranker selection and bounded settings | Omitted config preserves RRF-only; deterministic mode is explicit; invalid settings cause zero source/provider egress |
| `WSR-QA` | Code tests/benchmarks | `CODE-H1`, `SDK-R1` | Adversarial E2E, performance benchmark, soak, and failure-injection suite | All release gates in section 6.10 pass on the reference profiles |
| `WSR-DOC` | Memory, Code, hosts | `WSR-QA` | README, roadmap status, ACL reference, SDK examples, privacy guidance, and migration notes | Examples execute and no obsolete query-time-BM25 or sqlite-vec guidance remains |
| `WSR-EVAL1` | Code real-model tests | `HOST-R1`, `WSR-QA` | Paired enabled/disabled DeepSeek task evaluation with chunk and non-text adversaries | Exact completion improves, locked retrieval metrics pass, non-text egress is zero, and close releases every vector |
| `CODE-B2` | Code semantic runtime | `WSR-EVAL1` | Session-local cross-file embedding batch coordinator and amplification metrics | At most 1.10x the per-session batch-limit request lower bound with unchanged quality, lifecycle, and time-to-first-partition gates |
| `CODE-R2` | Code ranking | `CODE-C2`, `CODE-H1` | Deterministic bounded second-stage reranker and optional typed host port | Identifier quality never regresses; duplicate evidence falls on the overlap fixture; p95/scratch limits pass; failure returns original RRF order |
| `CODE-RDY1` | Code semantic runtime | `CODE-S1`, `CODE-Q1` | Optional event-driven query barrier for an in-flight semantic generation | Default remains immediate; ready/degraded, timeout, cancellation, close, and hard-bound races are deterministic and leak no query/source content |
| `WSR-EVAL2` | Code tests/real model | `CODE-R2`, `HOST-C2`, `HOST-R2` | Strategy/rerank matrix with deterministic and DeepSeek task evidence | Every metric in the locked report is populated and no variant ships as default without a statistically and operationally meaningful gain |
| `WSR-PROD1` | Code SDK/host evaluation | `WSR-EVAL2` | Locked real embedding models, HTTP-provider qualification, generation-task corpus, soak/churn, cross-platform matrix, and SLO/rollback runbook | Representative production evidence passes without weakening source, memory, lifecycle, or compatibility boundaries |
| `HOST-LCPU1` | CLI host | `CODE-E1`, `WSR-PROD1` | Optional typed in-process CPU embedding adapter with offline artifact admission | Model-free search remains dependency-free; local semantic mode passes Windows/macOS/Linux quality, cold-load, throughput, RSS, cancellation, and zero-network gates |

The parallelizable dependency shape is:

```text
                         ┌─> MEM-V1 ───────────────┐
WSR-00 ─────────────────┼─> CODE-C1 ──────────────┼─> CODE-S1 ─> CODE-Q1 ─> CODE-H1
                         └─> CODE-E1 ──────────────┘                  │          │
                                                                    └─> SDK-R1 ─┼─> WSR-QA ─> WSR-DOC
                                                                                 └─> HOST-R1
```

`MEM-V1`, `CODE-C1`, and `CODE-E1` should be developed in parallel after their
shared types and invariants are frozen. SDK and host work starts from the
versioned Code contract, not from private runtime structs.

Delivered `CODE-C2`, `SDK-C2`, and `HOST-C2` now carry the typed chunking
built-ins across Core, language, and ACL host boundaries.
Delivered `CODE-R2`, `SDK-R2`, and `HOST-R2` now carry the explicit
deterministic option without primitive algorithm names. Core `WSR-EVAL2`
slices now lock the default-line rerank adversary and an orthogonal real-model
chunking report across all built-ins plus a Rust custom negative control. The
CLI now selects, observes, and exercises both choices through a real two-layer
ACL and DeepSeek tool loop, and independently cross-checks Core batching metrics
against its embedding server. Code `cde887b` closes the remaining release
matrix with one versioned fixture and schema-v1 report executed through the
public Node.js, Python, and Go APIs. All three language arms pass identical
quality, request-amplification, non-text-egress, and release gates. The matrix
qualifies portability; its three remote tasks per SDK do not establish a
statistically or operationally meaningful reason to change the line or
RRF-only defaults.

`HOST-LCPU1` must execute in this order:

1. Freeze the model-free path as a negative dependency gate: Core, Memory, and
   default SDK/CLI builds must continue to work without an inference runtime or
   model artifact.
2. Select the smallest maintained runtime that satisfies the locked model,
   packaging, offline, quality, latency, RSS, cancellation, and supported-target
   gates. Build a second adapter only when the selected runtime fails one of
   those observable requirements; feature-count comparison alone is not a
   release oracle. Record the binary delta, cold/warm calls, recovery, peak RSS,
   and every supported release target.
3. Define one typed local-provider ACL block and immutable artifact manifest
   containing model identity, revision, files, SHA-256 digests, tokenizer,
   dimension, normalization, license metadata, and runtime compatibility. Model
   installation is a separate explicit action; session construction never
   downloads it.
4. Run CPU inference on a bounded blocking pool, reuse one admitted model per
   compatible host process, preserve Code's batching/cancellation contract, and
   expose only non-sensitive load/RSS/throughput status. Local processing does
   not require a source-egress grant, but enabling it still requires a trusted
   host layer.
5. Qualify semantic quality, generation completion, offline startup, corrupt or
   substituted artifacts, unsupported CPU features, memory pressure, provider
   panic, close during inference, and lexical fallback. Ship only if omission
   has zero behavior/dependency regression and local CPU mode passes the same
   source, revision, amplification, non-text, and release gates as remote mode.

CLI `e03b06e` completes steps 1-5. Its exact default dependency gate excludes
`fastembed`, `ort`, and `ort-sys`; the local feature graph excludes `hf-hub`, so
admitted sessions make no runtime model download. The revision-locked
multilingual model produces 384-dimensional unit vectors with a 7,045 ms cold
call, 19 ms warm call, 0 ms caller cancellation, 267 ms recovery to the next
successful request, and a 1,018,519,552-byte peak-RSS increase. A two-input
microbatch replaced the former 64-input path after the latter reached about
1.60 GiB under cancellation load. The same source adds 28,013,568 bytes
(15.22%) to the Windows debug binary.

Three real DeepSeek tool tasks pass at target ranks 5/2/3, reach full readiness
in 12,163/12,342 ms p50/p95, retain 39 vectors / 68,251 accounted bytes, execute
20 two-input document microbatches plus one query call at the exact configured
lower bound, and send zero non-text inputs. End-to-end task p50/p95 is
27,460/28,661 ms with 40,241 total DeepSeek tokens. A smaller digest-locked
smoke model performs real offline admission, inference, cancellation, recovery,
and RSS checks on native Linux x64/ARM64, Windows x64, and macOS ARM64 CI. The
gate also rejects malformed/substituted artifacts, simulates missing x86-64-v3
before model loading, and proves a 32-waiter cancellation storm cannot exceed
one native job or starve recovery.

FastEmbed/ONNX is therefore the qualified opt-in host runtime. A second adapter
is not required merely to create a comparison: the hard observable gates above
are the decision oracle and remain reusable if the runtime is replaced later.
Intel macOS stays model-free while the pinned ONNX Runtime lacks that target.
This choice does not change Code Core, A3S Memory, the ACL shape, RRF-only
ranking, or the model-free retrieval path.

`CODE-R2` was executed in this order:

1. Lock RRF-only fixtures for overlapping ranges, repeated boilerplate, exact
   identifiers, same-file symbols, and cross-file paraphrases. Record
   Recall@5/10, MRR, nDCG@10, duplicate-evidence rate, latency, and scratch
   memory before changing ranking.
2. Bound the fused input pool and feature bytes, then compute interval overlap
   and normalized lexical shingles without retaining another copy of source
   text or vectors.
3. Add deterministic MMR-style selection with exact-tier protection and stable
   path/range/ID tie breakers. Cancellation, allocation failure, or invalid
   host output returns the original RRF order.
4. Expose algorithm/version and bounded diagnostics without queries, source
   text, vectors, or model inputs in logs. Run adversarial determinism, panic,
   timeout, memory, and stale-revision tests.
Steps 1-4 are delivered. The remaining promotion work belongs to the following
evaluation gates:

5. Evaluate an optional host-injected local cross-encoder only against the
   deterministic baseline. It must be default-off and cannot download a model,
   make an undeclared network call, or change exact-identifier precedence.
6. Promote a rerank default only if locked identifier quality is unchanged,
   duplicate evidence falls materially, nDCG/task completion improves, and
   p95 latency plus scratch-memory gates pass on the reference profile.

`CODE-B2` was executed in this order:

1. Freeze machine-readable metrics for document inputs, provider requests,
   batch-limit lower bounds, flush reasons, time to first ready partition, and
   non-text inputs; first lock the current 30x amplification as a failing gate.
2. Add one bounded session-local coordinator between completed chunks and
   `EmbeddingExecutor`. It may group different files but may not cross sessions,
   providers, descriptors, or source generations.
3. Flush on the earliest configured input, text-byte, or vector-byte bound, or
   immediately when the immutable catalog generation is exhausted. No latency
   timer is needed because the coordinator never waits for a future revision.
   Cancellation removes unpublished work without extending the close deadline.
4. Validate the complete provider response, regroup vectors by file generation,
   and publish each file atomically. One malformed file or superseded generation
   cannot expose a mixed partition or discard already valid files.
5. Rerun provider fault injection, update/delete races, partial readiness,
   non-text zero-egress, exact/hybrid quality, lifecycle, 25,000-record release
   benchmarks, and the paired DeepSeek evaluation.
6. Ship only when request amplification is at most 1.10x the batch-limit lower
   bound and session construction, time to first partition, quality, memory, and
   close gates have no regression.

Steps 1-6 are delivered. The final schema-v2 DeepSeek paired run retained 3/3
enabled task accuracy versus 0/3 disabled, Recall@5/MRR of 1.0, one document
provider request for each 31-chunk session, zero non-text inputs, and complete
release. The schema-v3 25,000-record release run emitted 391 logical and
physical batches against a 391-request lower bound, with 1.0x amplification,
9-10 ms time to first file-atomic publication, and all latency, memory, scratch,
quality, and cleanup gates passing.

The schema-v5 backend follow-up keeps Memory as the semantic authority and
qualifies the a3s-vec lexical path in both hybrid arms. Native handles use a
four-entry hot cache with transient open/query/close for colder partitions;
concurrent replacement/query bursts stay within the descriptor budget,
package manifests are target-verified, and the same exact, RRF-only, and
deterministic latency gates remain in force. The benchmark keeps the 25,000 x
384 exact-vector profile separate from its four-file, 512-chunk native hybrid
fixture so each gate names the workload it actually measures.

Knowledge-compiler integration is not part of `CODE-B2`. It starts with a
separate cross-project ADR and fixture for the typed artifact/provenance handoff;
only then may Code add an explicit host-injected artifact provider.

### 6.10 Release qualification

`WSR-00` records the reference hardware and may tighten these thresholds, but a
later gate may not silently weaken them to make an implementation pass.

#### Correctness and retrieval quality

- Exact-vector top-k results match a brute-force f64 oracle across randomized
  corpora, updates, deletions, filters, and score ties.
- Deleted, changed, excluded, or out-of-scope content produces zero rendered
  stale hits after the corresponding workspace revision is observed.
- On the locked paraphrase fixture, hybrid Recall@10 improves over current BM25
  by at least 15 percentage points and reaches at least 0.85.
- On the identifier fixture, hybrid MRR and Recall@10 are not lower than exact
  plus BM25 baselines.
- Path filters, CJK queries, split identifiers, repeated boilerplate, and same
  symbol names in different modules have dedicated regression cases.
- Line, fixed-window, recursive-separator, and representative custom strategies
  have locked byte-range fixtures. Every range set is complete, gap-free,
  UTF-8-safe, deterministic, and within file/catalog budgets.
- `CODE-R2` reports nDCG@10 and duplicate-evidence rate in addition to Recall
  and MRR. Exact-identifier MRR/Recall may not regress relative to RRF-only, and
  rerank failure must reproduce the original RRF order byte-for-byte.

#### Latency and resources

- Session construction adds no full-corpus read or embedding wait to the
  synchronous creation path.
- On the reference profile, exact top-20 vector search over 25,000 normalized
  384-dimensional records has release-mode p95 at or below 30 ms.
- Hybrid local ranking, excluding external query-embedding network latency and
  including warm-cache authoritative source reads, has p95 at or below 100 ms
  on the schema-v5 four-file, 512-chunk native fixture. The 25,000-record
  exact-vector gate remains a separate workload.
- The deterministic second-stage reranker examines at most 100 fused candidates,
  adds at most 10 ms p95 on the reference profile, and uses at most 4 MiB of
  checked per-query scratch memory. A host neural reranker has a separate,
  explicit budget and is never included in this baseline claim.
- The default session budget is bounded; reaching it produces explicit partial
  coverage and never unbounded allocation. The initial target ceiling is
  256 MiB for catalog, lexical, and vector indexes combined.
- Repeated queries do not reread or re-embed unchanged files.
- Non-text workspace assets produce zero chunks, vectors, and Embedding Provider
  inputs; their parsing belongs to the separate knowledge compiler.
- Intentional chunk overlap is included in retained-text, vector-record, provider
  input, and request-amplification measurements; metrics must not count only
  unique source bytes.
- After `CODE-B2`, document-provider request amplification is at most 1.10x the
  per-session lower bound implied by input, text-byte, and vector-byte batch
  limits, without delaying synchronous session construction.

#### Isolation, security, and resilience

- Two sessions over the same or different roots cannot observe each other's
  chunks, vectors, status, revisions, or cancellation.
- Excluded secret/control paths are never submitted to a remote Embedding
  Provider, even through symlinks, rename races, include filters, or manifest
  lag recovery.
- Provider timeouts, 429/5xx responses, wrong dimensions, NaN/Infinity values,
  partial batches, panics, and cancellation degrade semantic retrieval without
  disabling `grep`, `glob`, BM25, or Code Intelligence.
- A change-stream overflow triggers full reconciliation; concurrent query and
  replacement observes either the old or new immutable partition, never a
  partially written partition.
- Session close during initial build, retry backoff, or a running query leaves
  no owned task, file handle, socket, or retained vector allocation after the
  bounded cleanup deadline.
- Workspace source text, vectors, and provider credentials do not appear in
  logs, metrics, error chains, or persisted session snapshots.

Deterministic fake embeddings are the required CI oracle. Opt-in real-provider
tests validate wire compatibility and cancellation but must never be the sole
proof of ranking correctness or run with repository secrets in shared CI.

### 6.11 Rollout and rollback

1. **Developer preview:** native persistent FTS is enabled best-effort for
   default local Agent workspaces; hosts without the native feature or writable
   cache continue using the catalog and portable scorer.
2. **Opt-in beta:** ACL and SDK configuration supported; `hybrid` recommended
   for natural-language queries while BM25 remains available explicitly.
3. **Stable:** SDK parity, adversarial qualification, privacy documentation,
   and production telemetry budgets complete. Any future automatic selection
   of `hybrid` requires model-tool evaluation and a separate compatibility
   decision.

Rollback is configuration-only: stop the persistent index coordinator and
retain existing exact, lexical, and Code Intelligence paths. Existing
generations remain readable for a later restart or can be removed explicitly
by the host; no semantic session state is migrated.

`WSR-PROD1` completes the stable opt-in evidence for the provider-injected
design. Delivered `HOST-LCPU1` adds the qualified CLI-local CPU route; it does
not block model-free search, require a new default, or move inference and
model-artifact ownership into Code Core or A3S Memory.

The semantic Memory projection and lexical a3s-vec projection have separate
failure domains. A lexical package or native-runtime failure degrades lexical
coverage and leaves exact/semantic paths available; it cannot promote a
different vector authority or bypass current-source verification.

### 6.12 WSR non-goals

- A vector database server, global daemon, or Cloud retrieval service. The
  opt-in local a3s-vec FTS generation is a workspace projection, not a shared
  service or semantic vector authority.
- Serializing vectors into Code checkpoints or sharing them across tenants,
  users, worktrees, or sessions.
- Turning workspace chunks into `MemoryItem` values or applying memory
  importance, consolidation, access-count, or prune behavior to source code.
- Making A3S Memory depend on model SDKs, HTTP clients, workspace APIs, or Code.
- Moving Code-specific RRF/MMR, exact-identifier protection, overlap policy, or
  reranker model lifecycle into A3S Memory.
- Replacing `grep`, `glob`, saved-file Code Intelligence, or authoritative file
  reads with semantic similarity.
- Shipping HNSW, product quantization, persistent semantic/vector caches,
  cross-session semantic reuse, or automatic local-model downloads before
  baseline measurements demonstrate a concrete need and a separate ADR defines
  lifecycle and security.
- Enabling a neural or remote reranker by default, or sending source text to a
  second endpoint merely because an Embedding Provider is configured.
- Sending workspace content to any embedding endpoint merely because a chat
  model is configured.
- Parsing, OCR, transcription, archive expansion, or direct vectorization of
  non-text workspace assets; those operations belong to the separate knowledge
  compiler and require an explicit typed artifact handoff.

## Review skill collision repair — 2026-10-08

| Gate | State | Evidence |
|---|---|---|
| CODE-PROJECTED-SKILL-DISCOVERY | Core/ACP/Studio regressions, installed-selection ACP admission, and actual ToBoss/document-expert GUI replies passed | [Outcome](../../docs/evidence/studio-runtime/20261008-review-skill-conflict/OUTCOME.md) |
| CODE-DECLARED-OPENUI-SURFACE | Declared form and view project without a gateway call. A declaration that cannot be projected fails closed. An unusable session credential does not borrow the process gateway key | [Outcome](../../docs/evidence/studio-runtime/20261010-openui-declared-surface/OUTCOME.md) |

## TUI selected landing — 2026-10-12

| Gate | State | Evidence |
|---|---|---|
| CODE-TUI-MAIN-LANDING-20261012 | Selected candidate: live TUI 10/10, 13 hermetic, strict workspace Clippy passed; full CI/release qualification HOLD (format failures and partial workspace failures retained) | [Outcome](../../docs/evidence/code-harness/20261012-tui-main-landing/OUTCOME.md) |
