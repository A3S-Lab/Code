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
lifecycle. Box and OCI Runtime own execution and isolation. Gateway owns public
request traffic and inference request accounting.

The [cross-repository Agent Runtime platform roadmap](https://github.com/A3S-Lab/a3s/blob/main/docs/agent-runtime-platform-roadmap.md)
defines shared ownership and dependency order. This roadmap must not create a
second Cloud Agent lifecycle, scheduler, queue, Secret store, usage ledger, or
checkpoint authority.

## 2. Current foundation

- `Agent`, `AgentSession`, governed Tool invocation, model adapters, bounded
  context, events, artifacts, and atomic `SessionSnapshotV1` are available.
- `AgentProtocolHarness` and `AgentProtocolHost` expose release/session/run,
  cancellation, checkpoint recovery, receipts, and bounded event pages.
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
| `CAR-01` | In progress | Conform the native Harness to Cloud `A1.2`/`A1.3` command, receipt, event-page, cancellation, and recovery contracts | Cloud retains execution identity and sequencing authority |
| `CAR-02` | Delivered | Tool-request/result evidence, per-call input/usage diagnostics, and the Rust-host immutable-content adapter retain every raw Tool result plus compacted change sides behind exact content-addressed references | Cloud owns adapter authorization, provider selection, projections, and object lifecycle; Gateway usage remains the billed request ledger |
| `CAR-03` | In progress | Deterministic Tool-result transforms, versioned source/result evidence, exact algorithm/policy digest bindings, replay-time policy validation, and host-injected immutable original references are delivered; Cloud-managed profile admission and cross-repository conformance remain | Cloud pins policy; Code does not invent tenant policy or mutate past events |
| `CAR-04` | In progress | Canonical `SessionCheckpointExportV1` payloads bind `SessionSnapshotV1`, optional between-tool-round logical resume evidence, and exact component/aggregate identities; a host-injected `SessionCheckpointExportSink` captures both components from one acknowledged live Run boundary after preceding events and capability-owned effects settle; every new logical checkpoint binds the source Run's exact Code catalog, authority ceiling, and optional Use cursor; recovery pins that complete historical generation before target-Run admission, and the Code Harness restores both components plus an optional exact host capability batch as one visible admission without split store prewrites; common Harness adoption and real provider/Box certification remain | Cloud `A1.6` owns checkpoint identity, immutable-object authorization, external revision fencing, retention, approval, and fork lineage |
| `CAR-05` | Planned | Pass restart, exact replay, cancellation, hostile Tool output, bounded-content, Secret-redaction, checkpoint, and cleanup conformance through one Cloud-managed Box workload | No direct Code-to-node control path |

Observation precedes mutation: `CAR-02` must provide useful read-only context
diagnostics before `CAR-03` can reduce any Tool result. The first transform
profile is deterministic and limited to bounded truncation, head/tail
retention, repeated-line folding, structured sampling, and immutable original
content. Model-generated summarization requires a separate future policy and
replay contract.

### 3.1 Harness architecture improvements

A review of DeepSeek Harness reinforces four useful boundaries without making
Code Core a general dependency-injection or plugin framework. The model should
see the exact capability surface and readiness state it can act on; each run
should bind the actual model input rather than only its intended configuration;
temporary capabilities should have explicit lifetimes; and richer code-mode
tool presentation should remain a host/profile concern over the same governed
execution world.

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

### 3.2 Scoped capability program

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
| `HOST-FLOW1` | Delivered | Core projects each named `FlowBinding` as one exact `WorkflowSpec` plus `FlowEngine` and exposes it through a non-clone host handle | Spec/engine runtime-build incompatibility and descriptor/spec name drift fail before publication; an N handle retains N's definition, engine/store, and exact Use lease across N+1; missing lookup acquires no lease; Session close cancels active replay and explicit close releases the lease |
| `HOST-KNOWLEDGE1` | Delivered | Core projects multiple digest-bound, non-queryable Knowledge Surface readiness values while admitting at most one separately selected cognitive Knowledge authority into each Run | Same-source Flow dependencies close only against exact surface evidence; surface values never become cognitive context; N Runs retain N provider, binding, readiness set, and exact Use lease across N+1; each Run snapshot records its own cognitive binding while the Session snapshot records the next-Run binding; resume requires an exact bootstrap; multiple or ambient general Context authorities fail before publication |
| `HOST-UI1` | Delivered | Core projects bounded, path-free `UiBinding` values and exposes them through a non-clone host handle | Reviewed entry, style, and script bytes plus their content digests are frozen before publication; descriptor/binding name or digest drift and dependencies outside Tool, Skill, MCP, and Flow fail closed; an N handle retains N's exact document and Use lease across N+1; missing lookup acquires no lease; Session close cancels active host use; renderer policy remains host-owned |
| `CAP-GA1` | Planned | Legacy shadow ownership and piecemeal reconciliation removed after one major compatibility period | Official hosts and SDKs use the scoped architecture and the complete verification matrix passes |

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

### 3.3 Durable memory program

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
| `DM-REFRESH1` | Delivered | Explicit semantic refresh rebuilds one complete Active namespace from a verified dual-budget A3S Memory snapshot without adding a background task | Snapshot identity is recomputed before and after atomic publication; source drift requires partition invalidation and never returns a receipt, pre-publication failure preserves the prior partition, cloned sessions serialize cleanup, and a secret-free success receipt binds source bytes/digest, serving generation, node count, and vector revision |
| `DM-CAS1` | Delivered | A CAS-capable shared vector index rejects delayed semantic publication and delayed drift cleanup from independently constructed runtimes | A3S Memory compares the expected global index revision at the same linearization point as partition mutation; Code captures the base revision before snapshot work, cleanup uses the published revision, strict callers reject weaker backends before I/O, and adversarial races preserve the newer partition |
| `DM-SCHED1` | Delivered | An opt-in session-owned worker periodically runs the verified semantic refresh against one exact live binding | Admission requires revision CAS before worker spawn; runs do not overlap, missed ticks skip, health and the latest successful receipt are observable, and clean bounded close finishes post-publication verification before releasing the worker |
| `DM-SKIP1` | Delivered | Verified unchanged scheduled ticks avoid redundant embedding and vector publication without weakening the refresh proof | Every tick recomputes a complete bounded Active snapshot; skipping requires an ownership-epoch receipt plus exact source, generation, CAS-captured revision, and full post-snapshot index status; source or index drift rebuilds, partition-atomic backends never skip, and a replacement owner clears the prior process-local receipt |
| `DM-REUSE1` | Delivered | Scheduled rebuilds reuse exact vectors from the current ownership epoch while retaining complete atomic publication | A text-free single-partition cache is keyed by the full semantic record ID and bounded by the refresh node/vector budgets; index-only drift has zero provider inputs, partial source drift embeds only misses, removal rebuilds from retained vectors, failed CAS publication does not promote prepared embeddings, and owner close clears vectors while retaining the receipt |
| `DM-PROD1` | In progress | Host qualification on representative long-horizon, real-provider semantic, larger multi-agent, repeated-restart, and production-drift distributions | The bounded deterministic semantic, restart, verified refresh, shared-index revision-CAS, owned scheduling, verified no-change suppression, and exact embedding-reuse slices are delivered; retained reports must still qualify larger independently labeled corpora, longer consolidation/decay horizons, a durable remote CAS backend and distributed lease policy, real providers, production cadence, snapshot-check cost, cache hit distributions, latency, billed cost, failover, and drift without weakening namespace, evidence, history, admission, or lifecycle invariants |

The deterministic semantic gate proves serving mechanics and isolation, not
real embedding-model quality or remote backend continuity. Production claims
remain gated on independently labeled corpora and retained host reports. See
[Durable Memory Integration](manual/DURABLE_MEMORY.md),
[Durable Memory Retrieval Evaluation](manual/DURABLE_MEMORY_RETRIEVAL_EVAL.md),
[Durable Memory Product Evaluation](manual/DURABLE_MEMORY_PRODUCT_EVAL.md),
[Durable Memory Multilingual Evaluation](manual/DURABLE_MEMORY_MULTILINGUAL_EVAL.md),
[Durable Memory Semantic Evaluation](manual/DURABLE_MEMORY_SEMANTIC_EVAL.md),
[Durable Memory Semantic Refresh](manual/DURABLE_MEMORY_SEMANTIC_REFRESH.md),
[Durable Memory Multi-Agent Evaluation](manual/DURABLE_MEMORY_MULTI_AGENT_EVAL.md), and
[Durable Memory Restart Endurance Evaluation](manual/DURABLE_MEMORY_RESTART_ENDURANCE_EVAL.md).

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
13. The model-free CPU path is the product baseline: exact, glob, incremental
    BM25, Code Intelligence, RRF, and deterministic MMR remain useful without an
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
              LexicalIndex       source evidence       (a3s-memory)
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
| `a3s-code-core/workspace` | Text admission, typed/custom chunk strategies, shared `ChunkCatalog`, lexical index, manifest reconciliation, path/revision metadata, and structured `WorkspaceRetrieval` provider contract | Non-text parsing, provider credentials, host UI, or durable cross-session cache policy |
| `a3s-code-core/embedding` | Host-injected `EmbeddingProvider` contract, provider descriptor, batching, cancellation, bounded retry, and normalized embedding errors | Vector storage or workspace traversal |
| `a3s-code-core/session` | `WorkspaceRetrievalRuntime`, asynchronous construction, prioritization, query-time promotion, cancellation, close/replace/resume behavior, and session isolation | Process-global mutable indexes or hidden persistence |
| `a3s-code-core/tools` | `semantic`/`hybrid` search modes, RRF fusion, bounded overlap-aware reranking, path filters, source anchors, coverage/status metadata, and lexical fallback | A second chunker or direct filesystem traversal outside `WorkspaceServices` |
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
├── lexical.rs             # indexed BM25/postings
├── semantic_runtime.rs    # embedding queue and vector partitions
├── hybrid_rank.rs         # RRF and deterministic diversity
└── rerank.rs              # bounded deterministic second-stage reranker
```

The manifest owns the preceding text/non-text decision in
`core/src/workspace/manifest/file_kind.rs`. The chunker never receives a
non-text asset.

The current native BM25 path selects candidates, reads files, creates 80-line
chunks, and scores them for every query. WSR first moves chunk ownership into
the catalog, then replaces query-time corpus construction with an incremental
lexical index. The model-facing BM25 behavior and source anchors remain
compatible during that migration.

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
| `CODE-C1` | Delivered | Session-local immutable chunk catalog, conservative sensitive-path eligibility policy, UTF-8-safe deterministic chunking, per-file BM25 postings, async manifest reconciliation, stale-content tombstones, lag rebuild, and query-time zero-read BM25 path; lifecycle, locked relevance, concurrency, budget, cleanup, failure-injection, and strict Clippy gates pass |
| `CODE-C2` | Delivered | Rust Core adds compatible line, fixed UTF-8 window, recursive prioritized-separator, and host-injected custom range strategies; Code validates complete coverage and budgets, owns IDs/digests/lines, charges overlap memory, contains host failures, and wires explicit configuration into session-owned catalogs without allowing silent overrides of host-owned catalogs |
| `CODE-E1` | Delivered | Host-injected `EmbeddingProvider`, immutable descriptor, deterministic text/vector-budgeted batching, caller-order restoration, cancellation/timeout propagation, typed bounded retry, response validation, panic containment, redacted diagnostics, and deterministic fake-provider gates |
| `CODE-S1` | Delivered | Typed `WorkspaceRetrievalOptions`, async session-owned catalog projection, Memory `3293f572` exact-vector partitions, pre-replacement tombstones, superseded-generation fencing, partial/degraded status and coverage, build-failure cleanup, and bounded idempotent close |
| `CODE-Q1` | Delivered | Structured semantic search through the unified `search` tool, bounded query embedding, immutable catalog/vector revision fencing, current-file digest and byte-range verification, coverage metadata, cancellation, and explicit fallback |
| `CODE-H1` | Delivered | Exact literal, incremental BM25, optional Code Intelligence symbol, and positive-similarity semantic candidates are fused by deterministic RRF (`k=60`); exact identifiers are protected, results are capped at two chunks per file, source is reread once per selected path, stale hits are filtered, and every channel reports bounded status/fallback metadata |
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
| `WSR-DOC` | Delivered | README, changelog, baseline, operator QA report, DeepSeek task evaluation, SDK examples, ACL host guidance, text/knowledge-compiler boundary, privacy boundaries, final revisions, and release disposition are aligned; obsolete query-time-BM25 and sqlite-vec guidance is excluded |

The detailed baseline and threat model are in
[`manual/WORKSPACE_RETRIEVAL_BASELINE.md`](manual/WORKSPACE_RETRIEVAL_BASELINE.md).
Release measurements and adversarial evidence are in
[`manual/WORKSPACE_RETRIEVAL_QA.md`](manual/WORKSPACE_RETRIEVAL_QA.md).
The paired real-model task and batching evidence is in
[`manual/WORKSPACE_RETRIEVAL_DEEPSEEK_EVAL.md`](manual/WORKSPACE_RETRIEVAL_DEEPSEEK_EVAL.md).
The chunk strategy and rerank boundary is in
[`manual/WORKSPACE_RETRIEVAL_CHUNKING.md`](manual/WORKSPACE_RETRIEVAL_CHUNKING.md).

| Gate | Owner | Depends on | Deliverable | Exit criteria |
| --- | --- | --- | --- | --- |
| `WSR-00` | Code core/tests | None | Versioned retrieval fixture corpus, current BM25 baseline, sizing data, threat model, and locked quality/latency budgets | Baseline is reproducible in CI and separates identifier, paraphrase, CJK, and lifecycle cases |
| `MEM-V1` | A3S Memory | `WSR-00` contract draft | Public vector types/trait and `InMemoryVectorIndex` | Contract, oracle, concurrency, invalid-input, budget, and cleanup tests pass without SQLite features |
| `CODE-C1` | Code workspace | `WSR-00` | Shared chunk catalog, eligibility policy, deterministic chunker, lexical postings, and manifest reconciliation | Unchanged files are not reread; create/change/delete/rename and lag recovery are deterministic |
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
  source reads, has p95 at or below 100 ms for the same corpus.
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

1. **Developer preview:** feature compiled and tested but disabled unless a host
   injects a provider; expose detailed status and the diagnostic `semantic`
   mode.
2. **Opt-in beta:** ACL and SDK configuration supported; `hybrid` recommended
   for natural-language queries while BM25 remains available explicitly.
3. **Stable:** SDK parity, adversarial qualification, privacy documentation,
   and production telemetry budgets complete. Any future automatic selection
   of `hybrid` requires model-tool evaluation and a separate compatibility
   decision.

Rollback is configuration-only: cancel the retrieval runtime, hide semantic
tool modes, and retain existing exact, lexical, and Code Intelligence paths.
No migration or index deletion procedure is required because the baseline
index is session-ephemeral.

`WSR-PROD1` completes the stable opt-in evidence for the provider-injected
design. Delivered `HOST-LCPU1` adds the qualified CLI-local CPU route; it does
not block model-free search, require a new default, or move inference and
model-artifact ownership into Code Core or A3S Memory.

### 6.12 WSR non-goals

- A vector database server, durable workspace index, global daemon, or Cloud
  retrieval service.
- Serializing vectors into Code checkpoints or sharing them across tenants,
  users, worktrees, or sessions.
- Turning workspace chunks into `MemoryItem` values or applying memory
  importance, consolidation, access-count, or prune behavior to source code.
- Making A3S Memory depend on model SDKs, HTTP clients, workspace APIs, or Code.
- Moving Code-specific RRF/MMR, exact-identifier protection, overlap policy, or
  reranker model lifecycle into A3S Memory.
- Replacing `grep`, `glob`, saved-file Code Intelligence, or authoritative file
  reads with semantic similarity.
- Shipping HNSW, product quantization, persistent caches, cross-session reuse,
  or automatic local-model downloads before baseline measurements demonstrate
  a concrete need and a separate ADR defines lifecycle and security.
- Enabling a neural or remote reranker by default, or sending source text to a
  second endpoint merely because an Embedding Provider is configured.
- Sending workspace content to any embedding endpoint merely because a chat
  model is configured.
- Parsing, OCR, transcription, archive expansion, or direct vectorization of
  non-text workspace assets; those operations belong to the separate knowledge
  compiler and require an explicit typed artifact handoff.
