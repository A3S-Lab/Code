# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [8.1.0] - 2026-09-02

### Added

- Integrated `a3s-search` v3.1.0 and its Moli-backed headless engines for
  Google, Baidu, Bing, and Brave. A3S Code now resolves a packaged Moli
  sidecar first, then a digest-verified per-user shared cache, and downloads
  the pinned v1.1.1 runtime only when needed.
- Added atomic Moli installation receipts and a cross-process install lock so
  concurrent Code processes reuse one verified runtime instead of repeatedly
  downloading or installing it. Native Node.js, Python, and Go distributions
  carry the matching Moli sidecar and provenance metadata.

- Added the A3S-owned `NativeBashSandbox`, backed by the independent
  `a3s-sandbox` crate, with direct macOS Seatbelt, Linux bubblewrap
  namespace/seccomp, and Windows AppContainer/Job Object backends.
  Every backend denies network and Unix-socket creation, bounds process trees
  and output, protects workspace control metadata and credentials, and fails
  closed when its operating-system boundary is unavailable.
  Local Code sessions install it by default before capability construction, so
  built-in Bash, workflows, and delegated child runs share the same boundary;
  custom handles remain explicit overrides and non-local workspace backends
  retain their own command runners.

- Added a Code-owned A3S Vec migration shadow for semantic workspace
  retrieval. A3S Memory remains the only serving authority; the same admitted
  vectors are mirrored once into a session-local, temporary Vec collection and
  query results are compared bit-for-bit behind a shared publication gate.
  Rust, Node.js, Python, and Go expose the active engine plus bounded,
  non-sensitive shadow lifecycle, resource, mutation, and parity counters.
- Added post-build `AgentReleaseManifest::bind_publication` and a minimal
  BuildKit publication fixture. The fixture packages an exact Linux A3S CLI
  binary without its final manifest, pushes one OCI image manifest, binds the
  resolved digest and provenance through `a3s-acl`, retains a canonical
  digest-verifiable builder provenance object, and locally verifies
  digest-pinned health, value redaction, SIGTERM shutdown, and cleanup without
  claiming external Cloud Runtime certification.
- Added public `AgentExecutionFailure` accounting for Rust callers. Provider,
  tool-round, and finalization errors retain the token usage and admitted Tool
  call count completed before failure without converting the failed run into a
  successful result.
- Persisted `DurableMemoryBindingV1` identity and fail-closed resume checks for
  exact durable-memory namespace, mode, recall policy, and retrieval profile,
  plus a real file-repository restart integration gate.
- Added the versioned word/CJK-bigram lexical profile and a deterministic real
  `AgentSession` gate for English, Simplified Chinese, Japanese, and Korean
  phrase variation, candidate isolation, and tenant isolation.
- Added typed host-injected durable-memory semantic recall with bounded
  embedding execution, caller-owned A3S Memory vectors, current Active-revision
  verification, deterministic lexical/semantic RRF, lexical fallback, and a
  versioned real-session cross-language isolation gate.
- Added explicit verified semantic-index refresh from complete A3S Memory
  namespace snapshots. Node/byte budgets, backend-response recomputation,
  serialized publication, post-publication drift cleanup, and secret-free
  refresh receipts prevent partial or falsely current index generations.
- Added revision-CAS semantic refresh for capable A3S Memory vector indexes.
  A strict host requirement fails before snapshot or embedding work on weaker
  backends; delayed independent publication and cleanup cannot replace or
  remove a newer shared-index generation.
- Added opt-in, session-owned `ScheduledSemanticRefresh`. It validates an exact
  semantic binding and revision-CAS backend before starting, skips missed ticks,
  never overlaps itself, reports through maintenance health, and retains the
  latest successful secret-free refresh receipt for its host-held handle.
- Added change-aware scheduled semantic refresh. An exact ownership-epoch
  receipt, semantic generation, source identity, CAS revision, and full index
  status avoid redundant embedding and vector publication. Repositories with
  an exact namespace change token now also avoid the snapshot itself on a
  stable tick; unsupported repositories retain the complete-snapshot proof.
  Source or index drift rebuilds conservatively, and a replacement schedule
  owner discards the prior process-local receipt.
- Added bounded ownership-epoch embedding reuse for scheduled semantic rebuilds.
  Exact semantic record IDs reuse only vectors already committed by a verified
  refresh, so index-only drift needs no provider-adapter input and partial
  source drift embeds only cache misses while still atomically publishing the
  complete partition. Failed publication never promotes prepared vectors, and
  close releases the text-free cache while retaining the observable receipt.
- Added bounded ownership-epoch metrics for scheduled semantic refresh. Every
  settled published, unchanged, or failed attempt records source change-token
  requests/valid observations, source-snapshot reads, logical cache hits and
  embedding misses, provider-adapter invocations/inputs/bytes including
  retries, publication effort, and elapsed time without retaining content or
  identifiers. The latest 64 runs and
  cumulative counters survive close for inspection and reset before a
  replacement owner starts; adapter-boundary counts do not claim remote
  transmission or billing.
- Added versioned, secret-free semantic-refresh recovery checkpoints. A host can
  persist `receipt.checkpoint()` and install it through
  `ScheduledSemanticRefresh::try_new_with_checkpoint`; the recovered owner must
  verify one complete Active snapshot plus the exact vector-index history token,
  revision, and full status before avoiding provider or publication work. The
  repository-local change token is never persisted, unrelated repository or
  vector histories cannot authorize a skip, and a missing history token falls
  back to a complete verified rebuild.
- Added the opt-in `durable-memory-sqlite` Rust feature and a real close/reopen
  semantic-refresh recovery gate. A matching persisted index history recovers
  without repeated embedding or publication; this is local durability, not a
  distributed lease or replicated remote store. Copying or atomically replacing
  the closed database forks its history token on Unix and Windows.
- Added a release-only durable semantic-refresh qualification profile over
  10,000 synchronized file-repository nodes and 384-dimensional SQLite vectors.
  It fail-closes on exact initial/stable/source-drift/index-drift/recovery work,
  persists and synchronizes the checkpoint across a real handle close/reopen,
  measures warm semantic-query percentiles, and gates local disk and Linux RSS
  ceilings without claiming a real provider, remote CAS/lease, billing, or
  failover.

### Changed

- Made Moli the default headless search backend while retaining explicit
  Chrome/Chromium and Lightpanda compatibility backends. Search provisioning
  has its own bounded budget, request-level proxy propagation, and typed
  fail-closed diagnostics.

- Hardened the default A3S Code system prompt and shared repository-tool
  contract with an evidence-driven operating loop, current-turn schema
  guidance, native sandbox and permission boundaries, guarded mutations, and
  explicit completion evidence. Added regressions that keep core tool routing
  and model-delivered capabilities intact.
- Pinned A3S Memory to the CJK lexical-query and vector-index releases. Existing
  schema 1-3 snapshots remain readable but cannot silently resume under current
  retrieval or admission identities. New lexical bindings use schema `4`;
  semantic bindings use schema `5` and freeze the authority, embedding,
  execution, index, policy, and fusion generation so older binaries fail closed.
- Advanced A3S Memory to the exact dual-budget namespace-snapshot revision used
  by semantic refresh; built-in in-memory and file repositories share the same
  complete-view, digest-verification, overflow, and restart contract.
- Advanced A3S Memory to the exact vector mutation-consistency revision. Custom
  backends retain source compatibility and fail closed for conditional methods
  until they implement atomic global-revision compare-and-swap.
- Advanced A3S Memory to the exact namespace change-token revision. Built-in
  repositories publish the token with node state and reconstruct it on file
  restart; custom repositories remain source-compatible and return `None` by
  default. The pinned revision also fixes `sqlite-vec` registration before the
  first concurrent SQLite connections are opened.
- Advanced A3S Memory to the exact vector-index history-token revision. The
  optional token binds one linear mutation history with a canonical SHA-256
  digest without exposing a raw backend, endpoint, or tenant identity; custom
  indexes remain source-compatible and return `None` by default.
- Advanced A3S Memory to the exact asynchronous vector-observation and durable
  SQLite-index revision. Semantic query, CAS publication, and checkpoint
  recovery now use one fallible status/token observation; synchronous status
  remains a diagnostic compatibility hint.
- Memory maintenance close now cancels an active job and awaits its settlement
  within the configured total deadline before using bounded abort. A semantic
  refresh that has published vectors can therefore finish mandatory source
  verification and receipt publication during a clean session close.

### Fixed

- Made governed Bash execution on local workspaces fail closed when no sandbox
  is installed. Only an explicitly authorized `require_escalated` request can
  reach the local host command runner; trusted direct Tool calls and non-local
  workspace runners retain their explicit contracts.
- Enforced the Bash command deadline at the Tool boundary for sandbox-backed
  execution, including legacy or faulty Sandbox implementations that ignore
  the request timeout.
- Preserved partial Agent usage and Tool-call accounting through sequential and
  parallel planning failures and failed trajectory summaries. Usage aggregation
  now also retains cache-read/cache-write tokens and saturates on overflow.
- Removed Tool definitions from model requests when neither permission nor
  confirmation authority can admit any invocation.
- Made Python BudgetGuard callback exceptions, malformed decisions, and bounded
  callback timeouts deny governed work instead of failing open.
- Omitted absent Node.js `webSearch` options from the Rust Tool request instead
  of serializing schema-invalid `null` values.
- Broke Tool registry, orchestrator, and Skill executor reference cycles so
  Session teardown releases durable-memory repository handles and file locks.
- Made Python SDK imports fail closed when multiple generated `_native`
  extensions could select a stale interpreter-specific binary, and added a
  deterministic cleanup command before editable SDK builds.

### Removed

- Removed the Anthropic sandbox-runtime adapter, npm identity/version checks,
  Node.js launcher integration, and all SRT-specific tests and public exports.

## [8.0.4] - 2026-08-31

### Fixed

- Removed the legacy 4,096-path preflight rejection from the local SRT
  hard-link boundary. Workspace entry and depth limits continue to bound
  discovery, while file-backed macOS Seatbelt profiles can retain every
  multi-link source path in large workspaces.

## [8.0.3] - 2026-08-28

### Changed

- Python SDK release builds now use a CPython 3.10 stable-ABI extension. The
  bootstrap keeps exact-wheel fallback for older releases while allowing
  Python 3.14 on both Apple Silicon and Intel macOS 12+.

### Fixed

- DeepSeek structured-output requests now use the provider-safe JSON-object
  response format instead of forcing an incompatible tool choice.
- The Python bootstrap isolates its native cache by platform and selects the
  stable-ABI wheel on CPython 3.14.

## [8.0.2] - 2026-08-27

### Added

- Added Intel macOS native artifacts for the Node.js and Python SDKs. The
  `x86_64-apple-darwin` builds use a macOS 12 deployment target.

- Added the Rust-host `AgentSession::workflow_with_token_budget_and_tools` for exact
  workflow-child host tools that remain absent from the parent and unrelated
  workflows while retaining composed permissions, HITL, security, cancellation,
  and budget authority.
- Added `UseRuntimeTaskProjectionAdapter` as the Rust-host boundary for
  consuming one exact A3S Use capability-snapshot v2 Runtime Tool Task through
  the atomic `SessionCapabilityBatch`. The adapter retains reviewed scope,
  package/manifest digests, lifecycle generation, provider identity, bounded
  argv and deadline contracts, and fails closed on response drift while A3S
  Use retains generation-lease, dispatch, and cleanup authority.

### Changed

- Structured child results now validate the raw final assistant object before
  display sanitization. Only protocol literals explicitly authorized by schema
  `const` or `enum` survive a conflicting redaction; other post-redaction schema
  failures fail the step instead of bypassing security.

## [8.0.1] - 2026-08-25

### Changed

- Advanced the exact `a3s-flow` dependency to the published `1.1.0` release,
  preserving the qualified batch-child-workflow and bounded-retry revision
  while allowing downstream Cloud locks to resolve one crates.io source.
- Removed the Node SDK's unpublished same-version platform packages from its
  development manifest. Release automation now publishes those artifacts
  first and injects their exact versions only into the main npm package,
  restoring deterministic `npm ci` and eliminating the release dependency
  cycle.

## [8.0.0] - 2026-08-25

### Added

- Added host-injected `SessionCheckpointExportSink` support for canonical live
  checkpoint export. Code now closes the capability Turn, drains causally prior
  Run events, captures `SessionSnapshotV1` with the matching between-tool-round
  `LoopCheckpoint`, retains the source Run's frozen cognitive and scoped
  capability authorities across concurrent catalog cutover, and acknowledges
  the boundary before the agent loop advances; sink failures remain isolated
  from the live Run and mixed Session/source-Run authorities fail payload
  validation.

- Added the first scoped-capability Core kernel slice: typed A3S Use package
  and cursor generations, typed local catalog generations, sealed source-owned
  descriptor batches, and bounded immutable `Arc<CapabilitySet>` snapshots.
  An empty product projection retains its exact Use cursor. Canonical
  `BTreeMap` ordering and a domain-separated golden digest are stable across
  input order; mixed Use cursors, Built-in shadowing, conflicts, unresolved
  dependencies, and aggregate resource overflow fail before a set can escape
  construction.
- Added typed Session, Run, Turn, and Subtask capability scopes with
  catalog-bound monotonic ceilings and borrowed marker-specific leases. A
  Use-backed Run must consume the exact upstream generation lease. One bounded
  structured-concurrency supervisor owns tasks, child scopes, reverse-order
  effects, and final lease release; close is cancellation-safe and idempotent,
  while `Drop` only propagates cancellation and aborts owned futures.
- Wired the scoped capability kernel into real Agent execution. Pre-analysis,
  planning, goal checks, structured Task generation, and provider/Tool
  iterations now own Turn scopes. Foreground Skill and Task delegation
  recursively own Subtask/Turn scopes; Tool effects and stream bridges settle
  with their active Turn. Background Tasks and streaming memory extraction are
  synchronously promoted only by an active Turn and remain Run-supervised until
  bounded settlement before the exact Use lease is released.
- Added closed typed capability runtime values, immutable
  `CapabilityProjection` catalogs, non-clone exact-generation reader leases,
  and `CapabilityTxn<Staged/Prepared/Validated>`. Only validated transactions
  can publish through a generation-and-digest CAS. Failed preparation,
  validation, cancellation, dropped transactions, and lost commit races retain
  prepared effects for bounded reverse cleanup without changing the visible
  generation; retired effects remain pinned until the last old lease drops.
- Added a generation- and digest-bound `CapabilityReadinessPlan` over published
  capability surface edges. Iterative deterministic readiness waves prepare
  prerequisites before dependents; cycles and incomplete staged batches fail
  before adapter startup, prerequisite failure blocks dependent activation,
  and completed effects retain reverse rollback. Maximum-width and
  maximum-depth tests exercise the configured 4,096-capability bound without
  recursion. Cross-package edges retain one exact A3S Use cursor while package
  resolution, SemVer, Grants, cutover, and recovery remain exclusively in Use.
- Added closed Adaptive, Direct, Code, and Disabled Tool-presentation Profiles
  over the existing governed executor. Permission visibility is applied before
  deterministic projection; code mode keeps the existing `program` name and
  parameter schema while emitting a bounded compact signature catalog. The
  exact Profile persists across Session resume, delegated runs cannot broaden
  it, and execution continues through the same pinned Tool values and
  permission, confirmation, budget, hook, cancellation, security, and audit
  boundaries.
- Added `model_presentation_bound` evidence with source/presented definition
  counts, digests, token estimates, Profile identity, and profiled/auxiliary
  application kind. Node.js, Python, and Go expose typed Profile values and the
  generated event catalogs include the new event without introducing package,
  Grant, generation, or lifecycle ownership outside A3S Use.
- Added versioned `tool_request_bound` evidence for model, nested, and trusted
  or governed host-direct Tool requests. The bounded snapshot binds the
  correlation identifiers, invocation origin, serialized argument bytes, and
  exact post-hook arguments through domain-separated digests without copying
  argument plaintext. It is emitted before permission, confirmation, budget,
  or execution outcomes, including denied requests, and is preserved exactly
  by Harness event replay and all generated SDK event catalogs.
- Added the Rust-host `ImmutableContentAdapterSession` boundary. A secret-free,
  digest-bound authority identity and byte ceiling now retain every successful
  raw Tool result plus compacted change sides before projection, validate exact
  content-addressed references, fail closed without a local fallback, persist
  the binding for exact resume re-injection, and propagate it to delegated
  children. Provider authorization, credentials, tenant projection, retention,
  and object lifecycle remain outside Code.
- Added `ToolResultTransformBindingV1` beside existing Tool-result evidence.
  Every real executor result now binds the exact deterministic algorithm and
  complete Session policy through stable domain-separated SHA-256 identities;
  binding drift fails before result release, and snapshot replay rejects a
  retained binding that differs from its exact persisted Session policy.
- Added provider-neutral `SessionCheckpointExportV1` artifacts. Recursively
  canonical JSON now binds a complete `SessionSnapshotV1`, optional exact
  between-tool-round `LoopCheckpoint`, both component identities, and the
  aggregate payload. Import rejects schema, ownership, Run-state, round,
  encoding, content, and descriptor drift without adding a Cloud checkpoint
  ID, storage provider, retention rule, approval, or fork lineage to Core.
- Added additive evidence-bound checkpoint recovery for Rust hosts.
  `AgentProtocolRunRecoverExactV1` binds the complete
  `SessionCheckpointDescriptorV1` into the receipt and target Run identity;
  `AgentProtocolHost` validates and pins the exact local boundary under the
  Session execution lease before baseline capture or Run admission.
  `AgentProtocolHarness::execute_checkpoint_recovery()` validates and decodes
  semantic and logical state from the same portable bytes, restores an
  unpublished Session, and publishes it only after exact Run admission without
  split SessionStore prewrites. Persisted semantic drift and unrelated live
  Sessions fail closed; external store revision fencing remains a Cloud/common
  Harness responsibility. The existing v1 command enum and recovery wire shape
  remain unchanged.
- Added `RunCapabilityBindingV1` to every newly admitted Run and live logical
  checkpoint. The canonical identity binds the exact Code catalog generation
  and digest, full authority-ceiling digest, and optional A3S Use cursor.
  Recovery pins and compares that complete generation before target-Run
  reservation, so N checkpoints cannot silently execute through N+1 after a
  concurrent cutover. A missing Session may perform one host-supplied,
  all-or-nothing jump from untouched generation zero to the exact historical
  generation; mismatched or missing batches publish neither Session nor Run.
- Added Run-frozen Agent projection to `SessionCapabilityBatch`. Compatibility
  and projected definitions share immutable `Arc<AgentDefinition>` values in
  an independent per-Run name map; automatic delegation, `task`, and
  `parallel_task` use that same registry. Canonical alias conflicts fail before
  publication, live worker and agent-directory registration cannot shadow a
  published Agent, and N Runs retain N delegation plus the exact A3S Use lease
  across an N+1 cutover.
- Added Run-frozen slash Command projection to `SessionCapabilityBatch`.
  Blocking and streaming command dispatch now enter the same capability Run,
  merge compatibility and projected Commands into an independent name map,
  and share the exact `Arc<dyn SlashCommand>` values. Built-in and compatibility
  conflicts fail before publication, legacy registration cannot shadow a
  published Command, and an N execution retains N plus its exact A3S Use lease
  across an N+1 cutover.
- Added Run-frozen Hook projection to `SessionCapabilityBatch`. Each
  `HookBinding` atomically pairs one immutable definition with its exact
  handler; unsupported Session/Skill lifecycle scopes and compatibility name
  conflicts fail before publication. Projected policy composes after an
  optional Session-static external executor and cannot be bypassed by its
  `Skip`. Official SDK Hook registration is atomic, deterministic equal-priority
  ordering uses Hook ID, and observational, asynchronous, and timed-out handler
  settlement is supervised by the capability Run while it retains the exact
  A3S Use lease within the configured bounded-close policy.
- Added Run-frozen MCP projection to `SessionCapabilityBatch`. Each
  `McpBinding` owns one exact initialized client and a sorted, bounded tool
  catalog; model definitions, raw calls, and delegated children resolve through
  that immutable binding instead of a mutable manager. Connection preparation
  and retirement are reversible asynchronous effects, cancellation and
  compatibility conflicts leave the visible generation unchanged, and an N
  Run retains both its local binding and separate exact A3S Use lease across
  N+1 publication. The adapter accepts only host-constructed configuration
  after authoritative Use Runtime/Gateway selection and does not inspect
  package files or resolve opaque Gateway identities.
- Added Run-frozen general Context projection to `SessionCapabilityBatch`.
  Exact provider `Arc` values are copied into the admitted Agent configuration,
  so an N Run and its separate A3S Use lease remain generation-exact across an
  N+1 publication. Descriptor/provider drift, Session-static name conflicts,
  and cognitive-package bindings on the general Context path fail before the
  catalog advances. Delegated children retain their existing isolated prompt
  context rather than resolving Session-latest providers.
- Added named A3S Flow projection to `SessionCapabilityBatch`. `FlowBinding`
  pairs one validated durable `WorkflowSpec` with the exact `FlowEngine` that
  owns its store, runtime, observer, replay, and runtime-build admission. The
  non-clone `ProjectedFlowHandle` retains one Code generation and exact A3S Use
  lease across N+1 publication; incompatible builds and descriptor/spec name
  drift fail before publication, missing lookup acquires no lease, and Session
  cancellation settles active replay before explicit handle close releases the
  lease. Projected Flow remains a host API rather than an implicit model Tool.
- Added exact Knowledge projection to `SessionCapabilityBatch`. One projected
  `CognitiveContextSession` becomes the Run-frozen cognitive authority and
  retains its Code and Use generations across N+1 publication. Run snapshots
  record their own immutable binding while the Session records the next-Run
  binding; resume requires an exact recovery bootstrap, and competing ambient
  Context authorities fail before publication without moving OKF, indexing,
  retrieval, retention, or query-lease ownership into Code.
- Added multi-instance `KnowledgeSurfaceBinding` projection beside singular
  cognitive Knowledge. Each path-free value canonically binds OKF format,
  immutable content, and exact host projection digests without exposing query
  or package-selection authority. Same-generation capabilities can depend on
  that readiness identity, while Agent Runs ignore it as cognitive context and
  retain the exact Code/Use generation until close.
- Added renderer-neutral UI projection to `SessionCapabilityBatch`.
  `UiBinding` freezes bounded, path-free HTML, CSS, and JavaScript bytes with
  canonical content and surface digests, and accepts only explicit Tool, Skill,
  MCP, and Flow dependency edges. The non-clone `ProjectedUiHandle` retains one
  exact Code generation and A3S Use lease across N+1 publication; missing
  lookup acquires no lease and Session close signals cancellation. A3S Use now
  emits versioned complete UI dependency evidence and CLI stages eligible
  Skill/UI values atomically. Rendering, origin/CSP/navigation/state, backend
  routing, Tool/MCP/Flow host adapters, and official renderer-host adoption
  remain outside this Core gate.

### Fixed

- Generalized real-provider release verification to the configured ACL default
  provider, including provider-specific environment variables, Windows CRLF
  normalization, local Node native builds, and `.exe` Go bridge discovery.
- Made the Windows Node ESM confirmation test import through a file URL, boxed
  the Go bridge lifecycle test dispatch future to keep the default test stack
  bounded, and added explicit time bounds to the supervised Hook timeout test.
- Made native Harness event pages read run state and retained events from one
  RunStore generation, preventing concurrent writes from combining an older
  state with newer events. Restored run-local observation time now remains
  monotonic across events, cancellation, and failure when host clocks differ;
  cursors at or beyond the exclusive Code tail now fail closed instead of
  silently skipping future events.
- Prevented Node.js BudgetGuard callback bridges from retaining the event loop
  after Session and Agent shutdown. The thread-safe callbacks no longer own
  process liveness, and the runtime smoke test now closes its native resources
  and removes its temporary workspace explicitly.
- Advanced the exact `a3s-flow` `1.0.0` source to revision `7c76eda9`, keeping
  the root, Node.js, and Python Cargo locks on one Flow authority while
  admitting capped exponential step retries with deterministic jitter.

## [7.0.2] - 2026-08-20

### Added

- Added an explicit one-way activation gate for local workspace manifests.
  Latency-sensitive hosts can construct a `ManifestWorkspaceBackend` without
  starting its scanner or platform watcher, retain local fallback search while
  the manifest is empty, and activate asynchronous discovery after their first
  interactive frame.
- Added a first-principles capability ledger covering all 20 advertised areas
  and five runtime surfaces, dedicated native Node.js/Python runtime jobs,
  cross-platform retrieval churn, and required hermetic MinIO, Chrome/CDP, and
  OpenTelemetry Collector qualification.
- Added release performance profiles for deterministic agent convergence,
  25,000-record Workspace Retrieval, Flow/State Graph, a 5,000-file Code
  Intelligence workspace, context/memory corpora, and synchronized session
  persistence. The workflow emits and retains six machine-readable reports
  with workload, percentile, resource, machine, and inclusion metadata.

### Fixed

- Advanced the exact `a3s-flow` `1.0.0` source to latest-main revision
  `006e988b`, keeping the root, Node.js, and Python Cargo locks on one Flow
  authority while admitting its additive bounded child-workflow batch API.
- Qualified the durable workflow integration against the exact
  `a3s-flow` `1.0.0-rc.1` candidate, migrated downstream fixtures and
  benchmarks to its public constructors, synchronized the standalone Node.js
  and Python SDK Cargo locks, and made the State Graph observer retain unknown
  compatible `1.x` events without guessing their projection.
- Synchronized the Node.js lockfile with all six `7.0.1` native optional
  packages so `npm ci` can build and execute the published wrapper contract.
- Added the common top-level `passed` verdict to the convergence report so the
  performance workflow validates every profile through the same fail-closed
  contract.

### Documentation

- Added the bilingual CLI activation boundary for asynchronous session-owned
  in-memory retrieval, including independent chat/embedding routes, trusted ACL
  requirements, remote source-egress consent, and local CPU model admission.
- Recorded the successful 2026-08-18 performance and hermetic integration runs
  with p50/p95/max results, resource ceilings, workflow links, and Artifact
  SHA-256 digests.
- Recorded the `v7.0.1` post-release `deepseek/deepseek-v4-pro` validation at
  Code `5aa9642`: the Core adversarial suite passed 3/3, the Node.js and Python
  real-config smoke tests passed, and the public Node.js, Python, and Go
  Workspace Retrieval matrix repeated 9/9 exact tasks and one-Search protocols
  with Recall@5 1.0, MRR 0.5, zero non-text inputs, and complete vector release.
- Clarified that the `real_config_env_integration.sh` wrappers rewrite only a
  provider named `openai`; native `providers "deepseek"` configurations should
  use the DeepSeek-specific Core or cross-SDK runners directly.

## [7.0.1] - 2026-08-17

### Fixed

- Updated the publishable `a3s-memory` baseline to 0.1.3 so crates.io builds
  retain the in-memory vector API used by workspace retrieval.
- Added a registry dependency preflight that rejects unpublished Git dependency
  baselines before the release build and publish jobs start.
- Hardened the release preflight by isolating real-provider configuration to
  its targeted smoke steps and raising low per-process open-file limits before
  the parallel Rust test suites.

## [7.0.0] - 2026-08-17

### Added

- Added versioned `run_capability_bound`, `model_input_bound`, and
  `model_usage_bound` Run events at the unified provider-neutral model
  boundary. They bind actual model-visible tools, workspace capabilities,
  run-owned governance bindings, serializable policy identities, execution
  ceilings, semantic readiness/generation, input shape, retrieval Tool-result
  evidence, exact repeated Tool-result context, prompt estimates, and
  normalized client token/cache usage through bounded counters and
  domain-separated SHA-256 digests without retaining new prompt, Tool-result,
  source, vector, credential, or endpoint plaintext. Capability
  emission is concurrency-safe and deduplicated by digest; every model call has
  a positive sequence and exact persisted replay coverage. Run and streaming
  caller cancellation interrupt evidence backpressure at the provider boundary.
- Added an opt-in semantic readiness barrier for asynchronous workspace
  retrieval. A host can bind a query to the current catalog generation for up
  to 30 seconds without making session construction synchronous. Publication
  notifications wake queries without polling; timeout preserves the existing
  partial `building` fallback, while caller cancellation and session close
  interrupt the wait. The default remains zero wait for compatibility.
- Added an opt-in, revision-locked real embedding model matrix through the
  Python SDK callback boundary. An English MiniLM negative control fails only
  the CJK task at Recall@5 0.6667, while multilingual MiniLM passes semantic
  and RRF-hybrid Recall@5 1.0 with ranks 2/2/2, 1.0x request amplification,
  zero non-text input, and complete release. The deterministic reranker keeps
  Recall@5 but lowers MRR from 0.5 to 0.3444 on this fixture, reinforcing the
  RRF-only default. Sentence Transformers remains an optional evaluation
  dependency and no model is bundled or downloaded by A3S Code.
- Added a session-local cross-file embedding batch coordinator for ephemeral
  workspace retrieval. It flushes deterministic catalog generations at input,
  text-byte, vector-byte, or generation-complete boundaries, retains split-file
  vectors privately until file-atomic publication, fences superseded revisions,
  and preserves already valid partitions across later provider failures.
  Machine-readable Core, Node, Python, and Go status now reports document
  inputs/bytes, logical batches, physical provider requests including retries,
  the three-limit lower bound, flush reasons, time to first ready partition,
  and non-text inputs. The 30-file and 55-file DeepSeek fixtures and the
  25,000-record release profile reduce request amplification to 1.0x while
  preserving quality, memory, and lifecycle gates. CLI `f435950` pins this
  kernel and independently reproduces 1.0x on the real 39-chunk ACL-host task.
- Added asynchronous session-owned workspace retrieval to Core with one
  manifest-derived chunk catalog, incremental BM25, host-injected embeddings,
  exact in-memory vector partitions, deterministic hybrid RRF, current-source
  verification, bounded lifecycle/status APIs, and no durable vector database.
  Remote embedding source is protected by conservative path admission and an
  O(1)-construction read-time egress boundary that rejects resolved control
  paths and every hard-linked alias using the same open file handle, without
  changing ordinary tool access.
- Added typed session-owned ephemeral workspace retrieval to the Node.js,
  Python, and Go SDKs, including host-injected embedding providers, asynchronous
  indexing status, digest-verified semantic and hybrid result DTOs, bounded
  cleanup, and cancellation propagation. Go bridge protocol v2 now explicitly
  cancels in-flight callback contexts during query and session shutdown.
- Added explicit Rust-host retrieval clearing through
  `SessionOptions::without_workspace_retrieval()`, conservative non-text asset
  classification before chunking and embeddings, and a reproducible paired
  DeepSeek task evaluation covering multi-chunk retrieval, disabled ablation,
  non-text zero-egress, request amplification, and lifecycle metrics.
- Added typed workspace text chunking strategies: the compatible line/byte
  default, UTF-8-safe fixed windows, recursive prioritized separators with
  bounded overlap, and a validated `Send + Sync` Rust host extension. Session-
  owned catalogs accept explicit strategy and memory-limit configuration;
  host-owned catalogs reject silent session overrides. Node, Python, and Go
  now expose typed line/fixed/recursive objects, share one locked range and
  invalid-window fixture, reject primitive strategy names, and validate before
  provider execution (and before Go callback registration). Shared manifest
  hosts can configure the asynchronous catalog exactly once through the public
  `ManifestWorkspaceBackend::configure_chunk_catalog` boundary before services
  attach.
- Added an opt-in bounded deterministic hybrid reranker after RRF with
  exact-identifier protection, overlap and lexical-boilerplate similarity,
  MMR-style diversity, stable tie breaking, checked candidate/feature/scratch
  limits, unchanged-order RRF fallback, and versioned diagnostics. Node,
  Python, and Go expose typed deterministic-reranker option objects as well as
  hybrid result DTOs for the applied algorithm and resource accounting. The
  SDK option boundaries preserve Core defaults, reject primitive algorithm
  names, and fail invalid bounds before provider calls. A paired adversarial
  DeepSeek Core evaluation covers
  cross-channel duplicate collisions, end-task completion, quality, latency,
  memory, provider amplification, non-text egress, and cleanup. RRF-only
  remains the compatibility default. A second orthogonal DeepSeek slice
  qualifies line, fixed-window, and recursive chunking under the deterministic
  reranker while retaining a deliberately coarse Rust custom splitter as a
  measured negative control. The real ACL host and one versioned Node.js,
  Python, and Go SDK matrix now each complete all three exact tasks and
  one-Search protocols with Recall@5 1.0, MRR 0.5, 1.0x document-request
  amplification, zero non-text inputs, and complete release. The small matrix
  qualifies portability but does not change the compatible defaults.

### Changed

- Bumped the Rust, Node.js, Python, bootstrap, and Go bridge release line to
  7.0.0. The Go module now uses the required major-version path
  `github.com/A3S-Lab/Code/sdk/go/v7`.
- The Go SDK now selects Core's built-in security provider through the typed
  `DefaultSecurityProvider` session option. The legacy `DefaultSecurity`
  boolean remains deprecated wire compatibility, and ambiguous or unknown
  provider specifications fail closed in the bridge.
- Node.js and Python session construction now reject unknown security, memory,
  and session-store objects instead of silently disabling the requested
  security or persistence boundary.

### Fixed

- Kept post-terminal background model helpers out of a completed Run's bound
  evidence sequence, and made the deterministic convergence benchmark disable
  unrelated LLM memory extraction instead of depending on task scheduling.
- Forced the Agent release ACL contract fixture to LF so Windows
  `include_str!` tests exercise multiline duplicate and collision attacks
  instead of silently retaining the original valid fixture after a CRLF
  checkout.
- Made generated event-protocol checks insensitive to host line endings while
  continuing to reject any semantic drift across the Rust, Node.js, Python,
  and Go declarations.
- Made the Windows local shell fail closed when PowerShell cannot start instead
  of reinterpreting PowerShell syntax through `cmd.exe`.
- Made cancellation during language-server initialization or settling kill and
  reap the child process before returning, with deterministic lifecycle tests
  that reuse one compiled fake server and bound watcher/process concurrency.
- Made atomic file-session replacement tolerate bounded transient Windows
  sharing, lock, and access-denied races while continuing to fail closed for
  permanent filesystem errors.

## [6.9.0] - 2026-08-12

### Added

- Extended hierarchical instructions with a Codex-compatible personal layer:
  sessions load the first non-empty `~/.a3s/AGENTS.override.md` or
  `~/.a3s/AGENTS.md` before the project root-to-workspace chain, under one
  shared bounded byte budget and configurable `user_instructions_dir`.
- Completed lifecycle hook governance for tool-input rewrites, prompt blocks,
  permission requests, context compaction, and session start/end. Rewritten
  tool arguments are schema-validated again before any side effect, and
  PrePrompt rewrites/additional context now replace the actual model-bound
  user message instead of affecting only context lookup and telemetry. Prompt
  denials also emit an actionable terminal stream error to interactive and
  headless hosts.
- Added Codex-compatible hierarchical project instructions. Sessions now load
  `AGENTS.override.md`, `AGENTS.md`, and configured fallback names from the
  nearest Git root through the workspace, enforce a bounded UTF-8/non-symlink
  admission policy, retain source provenance, and keep the resulting chain out
  of ordinary retrieval-budget eviction.
- Added one agent-wide scheduler backed by `a3s-lane::PriorityQueue`, with a
  configurable global capacity, strict priority/FIFO admission, starvation-safe
  aging, cancellation, drain-on-close, occupancy snapshots, and per-session
  priorities across Rust, Node.js, Python, and Go. Conversation runs,
  host-direct tools, detached background children, and host-started workflows
  now share the same capacity boundary.
- Added per-conversation detached Git worktrees to the native Agent Harness and
  the bounded `/v1/agent/changes` protocol endpoint. Every terminal run can now
  expose one immutable, SHA-256-bound binary Git patch without mutating the
  source worktree or sharing workspace writes across concurrent conversations.
- Added host-pinned `a3s.code.tool-result-transform-policy.v1` policies for
  deterministic Tool-result bounding, UTF-8-safe head/tail retention, exact
  repeated-line folding, and structured JSON-array sampling. Policies persist
  in session snapshots and resume rejects policy drift.
- Added the same Tool-result transform policy surface to the Node.js, Python,
  and Go SDKs.
- Added versioned `a3s.code.tool-result-evidence.v1` observations to every
  Tool result, including original/projected byte and token estimates, exact
  repeat digests, loss mode, and immutable inline or artifact references.
- Added the Rust-host `CognitiveContextSession` boundary for one exact A3S Use
  cognitive-package generation. Typed requests and cited Markdown responses
  retain package/version, lifecycle generation, capability snapshot, Knowledge
  surface, content, source, and citation digests under strict item/byte bounds.
- Persisted cognitive-package bindings in session snapshots and added the
  `cognitive_context_bound` runtime/event-protocol record so restart and replay
  retain the same generation identity.

### Changed

- Tool-result evidence now records the versioned transform algorithm,
  source/projected SHA-256 digests, signed byte/token deltas, exact loss mode,
  and the immutable original artifact reference for every lossy projection.
- Tool-output artifact identifiers now use the original content's SHA-256
  digest instead of Rust's implementation-defined default hasher, making
  references stable across processes and runtime upgrades.
- Cognitive-package-bound turns now fail closed on provider or identity drift,
  require the same host-injected binding on resume, suppress personal-memory
  recall, and reject general RAG/graph providers instead of using them as a
  fallback.
- The Go SDK now selects Core's built-in security provider through the typed
  `DefaultSecurityProvider` session option. The legacy `DefaultSecurity`
  boolean remains deprecated wire compatibility, and ambiguous or unknown
  provider specifications fail closed in the bridge.
- Node.js and Python session construction now reject unknown security, memory,
  and session-store objects instead of silently disabling the requested
  security or persistence boundary.

### Fixed

- Made generated event-protocol checks insensitive to host line endings while
  continuing to reject any semantic drift across the Rust, Node.js, Python,
  and Go declarations.
- Made the Windows local shell fail closed when PowerShell cannot start instead
  of reinterpreting PowerShell syntax through `cmd.exe`.
- Made cancellation during language-server initialization or settling kill and
  reap the child process before returning, with deterministic lifecycle tests
  that reuse one compiled fake server and bound watcher/process concurrency.

## [6.8.1] - 2026-08-09

### Changed

- Upgraded the shared A3S Flow dependency to 0.11.0 so embedders can compose
  Code's dynamic workflows with the current durable runtime without loading a
  second, incompatible Flow engine.
- Projected Flow cancellation requests, timeouts, retry exhaustion, host
  shutdown, progress updates, and child-operation links into the State Graph
  with explicit terminal outcomes and cancellation of open projected work.
- Made the release preflight use an explicit `CARGO_TARGET_DIR` when locating
  the Go bridge integration binary.

## [6.8.0] - 2026-08-05

### Added

- Added dependency-free BM25 lexical ranking as a bounded mode of the unified
  workspace `search` tool, alongside regular-expression content search and
  path globbing.
- Added a unified model-visible `task` contract for either one focused child
  task or a bounded 1-32 item concurrent fan-out.
- Added exact detached run admission to the Node.js, Python, and Go SDKs. Each
  host-selected start or checkpoint recovery returns the authoritative run
  snapshot plus an explicit replay flag without exposing runtime task handles.
- Added the canonical bounded `a3s.code.agent.v1` headless protocol for exact
  release/session/run start, cancellation, checkpoint recovery, command
  receipts, and direct projection of the existing `EventEnvelopeV1` run-event
  pages. Hosts may transport these contracts without creating another Agent
  lifecycle or event model.
- Added `AgentProtocolHost` and exact detached-run admission. Replayed run IDs
  reuse the authoritative `AgentSession` run, conflicting immutable input is
  rejected, recovery keeps Code's checkpoint semantics, and event pages read
  the existing Code run store rather than a parallel Harness journal.
- Added the Code-owned `AgentProtocolHarness` multi-session kernel used by the
  sole `a3s code harness` process. It resumes complete persisted sessions before
  replay, creates sessions only for start/recovery admission, bounds retained
  conversations, and never introduces another run store or event journal.
- Bound protocol `agent_release_identity` to the manifest-declared immutable OCI
  artifact digest, while retaining the canonical ACL digest as the distinct
  complete-manifest admission identity.
- Added permission/HITL-aware governed direct-tool calls across the Node,
  Python, and Go SDKs, preserving `tool` as the explicit trusted
  host-control-plane API and restoring the cross-SDK alignment gate.
- Added an observable filesystem-first serve lifecycle with post-preparation
  readiness, stable terminal failure codes, and status access in the Node.js,
  Python, and Go SDK handles.

### Changed

- Consolidated model-facing workspace discovery under `search` and delegation
  under `task`. The registered `parallel_task` implementation remains as a
  hidden compatibility alias for direct SDK callers and persisted workflows.
- Reordered the automatic `web_search` cascade to use an available managed
  headless runtime first, then bounded HTTP/RSS engines, and native APIs only
  when earlier evidence remains insufficient.
- Serve startup now validates schedules before allocating sessions, and SDK
  start calls return only after session/tool preparation. Graceful stop cancels
  in-flight schedule work, closes owned sessions, and joins within a bounded
  deadline.

### Fixed

- Prevented sandboxed `program` scripts from recursively invoking
  `dynamic_workflow`; the program itself and the hidden `parallel_task` alias
  remain excluded from both default and explicit script allow-lists.
- Preserved Hook retry reasons across Rust, Node, Python, and Go callbacks and
  surfaced pre-tool denials as structured `hook_denied` tool errors with
  explicit retryability and retry delay guidance for both models and hosts.
- Upgraded the Core HTTP transport to `reqwest` 0.12 so the Windows Bash curl
  compatibility path sends its normalized JSON body without leading CRLF
  bytes, while preserving stable retryable timeout diagnostics.
- Added regression coverage proving that project-defined script tools re-apply
  session permission and HITL gates to nested `ctx.tool` calls, and corrected
  stale pre-5.0 documentation that described the former direct-registry path.

## [6.7.0] - 2026-07-31

### Added

- Added a local-workspace `download` tool with SSRF-safe redirects, strict HTTP
  range validation, adaptive bounded concurrency, retry and sequential fallback,
  optional SHA-256 verification, and crash-safe atomic publication.

### Changed

- Enabled the lazy browser-backed Google/Baidu tier by default with
  Chrome/Chromium as the cross-platform backend. Native API and HTTP/RSS tiers
  still run first and stop the cascade when their evidence is sufficient.
  Minimal Rust embeddings can opt out with `default-features = false`;
  Lightpanda remains an explicit alternative backend.
- Made `web_search` fail closed when the completed cascade remains below its
  generic quality floor. Successful JSON responses retain the existing array
  contract; insufficient responses carry a typed diagnostic envelope, and
  result rows expose their query-alignment score.
- Shared search bulkheads, browser retry budgets, and identical-request
  coalescing across delegated research contexts. Request-scoped proxies now
  reach the lazy browser tier, and coalescing diagnostics are included in tool
  metadata.

### Fixed

- Updated standalone CI dependency rewriting for the default Core search
  feature boundary so source checkouts resolve the published `a3s-search`
  package on Linux and Windows.

## [6.6.0] - 2026-07-29

### Added

- Added a quality-gated `web_search` cascade that runs native APIs first,
  conventional HTTP/RSS engines only when needed, and lazily initializes
  headless engines as the final tier. Search metadata now retains tier quality,
  engine outcomes, attempt duration, retry context, and session-scoped circuit
  state.
- Added exact durable dynamic-workflow step recovery bound to the run id,
  original query, and completed step id, plus an optional
  `maxConcurrentGenerations` limit of 1-4 for independently session-forked
  `generate_object` steps.
- Added host-only structured-response validation schemas and an explicit LLM
  client capability for distinct non-streaming transports, allowing composite
  clients to validate complete streamed JSON without presenting the same
  streaming path as an independent fallback.
- Added a pure-Go v6 SDK backed by a long-lived, capability-checked Rust bridge,
  covering sessions, lossless events, direct tools, run observation,
  verification, persistence, Skills, and MCP without requiring CGO.
- Added version-matched x86-64 Linux, macOS, and Windows bridge release assets
  with SHA-256 checksums, Go protocol-alignment checks, CI integration, and
  bilingual website documentation.

### Changed

- `web_fetch` now preserves typed transport, timeout, HTTP 429, and
  `Retry-After` evidence instead of deriving retry guidance from rendered error
  messages.
- `generate_object` forwards its governed active-generation timeout through
  the invocation gateway and holds that deadline across bounded schema repair.

## [6.5.2] - 2026-07-28

### Added

- Added strict `generate_object` argument validation, root JSON Schema support
  for references, composition, constants, arrays, and scalars, and active
  generation-timeout propagation through governed LLM clients.
- Added typed HTTP cancellation, transport, and retry-exhaustion status errors,
  including the SDK-visible `transport` tool error kind.

### Changed

- Required `parallel_task` calls to contain 2-32 independent foreground
  branches and reject invalid timeout and partial-success thresholds.
- Kept branch retry ownership inside provider and child runtimes; the parallel
  fan-out layer no longer replays failures based on rendered error messages.
- Synchronized the Node `ToolErrorKind` declarations with every Rust wire
  variant, including cancellation, partial failure, rate limiting, and
  transport failures.

### Fixed

- Preserved local `$defs` and `definitions` references when provider-facing
  structured-output schemas wrap root arrays or scalar values.
- Preserved typed transport and cancellation failures across OpenAI-compatible
  and Anthropic blocking and streaming request paths.

## [6.5.1] - 2026-07-28

### Added

- Added an opt-in real-LLM integration suite for batch-read continuation,
  every grep output mode, guarded edit previews and writes, and stable glob
  pagination.
- Added a shared repository-tool contract to every built-in agent prompt with
  canonical `read`, `grep`, `glob`, and `edit` arguments, pagination rules, and
  guarded mechanical-edit guidance.

### Fixed

- Accepted structured providers' neutral pagination defaults (`limit = 200`
  and an empty cursor) in non-paginated grep modes while continuing to reject
  effective pagination controls there.

## [6.5.0] - 2026-07-28

### Added

- Added budget-bounded `read.files` calls for 1-32 text files with ordered
  per-file results, isolated member failures, and lossless continuation
  arguments that are included in the response budget.
- Added `grep` output modes for content, paginated matching paths, paginated
  per-file matching-line counts, and full-scan summaries without rendering
  discarded match text.
- Added read-only `edit` dry runs plus exact and maximum replacement-count
  guards so mechanical edits can be previewed and bounded before CAS writes.

### Changed

- Added explicit `glob` ordering: `sort: "path"` provides deterministic lexical
  order before cursor pagination, while the compatible `sort: "backend"`
  default retains backend relevance or recency order.

## [6.4.3] - 2026-07-25

### Fixed

- Kept Auto execution direct when structured pre-analysis is unavailable, and
  replaced fabricated numbered fallback tasks with one step containing the
  original request when planning is explicitly enabled.

## [6.4.2] - 2026-07-23

### Changed

- Selected sandbox-runtime 0.0.67 for managed hosts.

### Fixed

- Pinned Unix managed-SRT temporary files to the private per-command scratch
  directory so hosts can pass large macOS Seatbelt profiles by file without
  exceeding the operating system argument-size limit.

## [6.4.1] - 2026-07-23

### Fixed

- Kept SRT workspace policy scans stable when another process removes an
  enumerated file or directory concurrently, while preserving fail-closed
  behavior for permission and other I/O failures.

## [6.4.0] - 2026-07-22

### Added

- Added structured per-engine search failure metadata and provider-agnostic
  fallback within the original request timeout budget.

### Changed

- Restored DuckDuckGo and Wikipedia as the built-in web-search defaults and
  made AnySearch an explicit request or ACL selection.
- Preserved bounded fallback notices and failure metadata when web search runs
  inside a batch workflow.

### Fixed

- Accepted a complete schema-valid streamed object after a short terminal-event
  grace period, so an OpenAI-compatible endpoint that omits or delays its final
  `Done` event cannot turn an already generated result into a timeout.

## [6.3.1] - 2026-07-22

### Fixed

- Removed redundant nested SRT write-deny mounts when a protected workspace
  ancestor already blocks writes, while retaining credential read denial and
  standalone sensitive-file write protection.

## [6.3.0] - 2026-07-22

### Added

- Added typed model-generation concurrency and admission contracts so hosts can
  declare and enforce bounded active generations without provider-name checks.
- Added durable `MemoryObservation` and `MemoryObserver` extension points for
  auditable preference, skill, and knowledge projections after persistence.
- Added run-scoped permission and confirmation snapshots plus targeted
  cancellation and expiry for pending tool approvals.

### Changed

- Serialized completed-turn memory extraction, drained accepted extractions at
  session close, and preserved canonical duplicate-consolidation results for
  observers.

### Fixed

- Kept LLM-authored learning titles, summaries, and instructions concise and
  user-facing, and excluded internal orchestration or handoff procedures from
  the learning signal contract.

## [6.2.0] - 2026-07-22

### Added

- Added a fail-closed SRT-backed `BashSandbox` with bounded execution,
  separated output streams, protected workspace control metadata, credential
  read boundaries, and verified exact-path npm and Node runtime constructors.

### Fixed

- Forwarded delegated child confirmation-required, confirmation-received, and
  confirmation-timeout events through the parent runtime stream so shared HITL
  providers cannot deadlock while the host UI waits for an event it never saw.
- Built MCP tool-selection context from the last six text-bearing messages
  instead of counting tool-use and tool-result messages that are discarded,
  preserving the original request across long delegated tool sequences.

## [6.1.0] - 2026-07-20

### Added

- Added a closed, bounded `.a3s/asset.acl` Agent release contract with immutable
  OCI and provenance digests, static entrypoint and health declarations, typed
  storage boundaries, schema-aware canonical identity, and pre-activation
  protocol and capability checks.
- Integrated `a3s-search` 2.0 native AnySearch and Tavily providers into the
  built-in `web_search` engine catalog. Both providers support their documented
  credential-free modes and optional environment-based authentication.

### Changed

- Made AnySearch the sole built-in default for `web_search` when neither the
  request nor `SearchConfig` selects engines. Explicit request and ACL engine
  selections continue to override the built-in default.

### Security

- Restricted release secret requirements to unique environment or
  `/run/secrets/` injection slots, rejected embedded values and ambiguous
  destinations, and kept admission diagnostics from echoing manifest values.

## [6.0.0] - 2026-07-19

### Added

- Expanded TypeScript code-intelligence discovery for nested monorepos,
  hoisted and Yarn SDKs, classic `tsserver`, and the TypeScript 7 native LSP.
- Added bounded PDF text extraction to `web_fetch`, including media/signature
  detection, normalized metadata, malformed-document errors, and image-only
  document handling.
- Added an invariant-checked session snapshot fork operation that rebinds the
  session, workspace, run ownership, and subagent parent ownership while
  preserving the complete persisted generation.
- Preserved standard MCP tool metadata and call results end to end, including
  output schemas, annotations, icons, `_meta`, `structuredContent`, decoded
  images, embedded resources, and bounded content-addressed artifacts.

### Changed

- Raised the major version because the standard MCP metadata support extends
  the public `McpTool` and `CallToolResult` structures with new fields.

### Fixed

- Kept standalone conversational greetings tool-free and prevented them from
  triggering synthetic continuation turns, while retaining the normal tool
  surface for greetings that also contain an action request.

### Security

- Made MCP confirmation annotations escalation-only: tool metadata can require
  HITL but cannot weaken a host Allow/Ask/Deny decision.
- Allowed an explicitly scoped delegated worker to see a parent-hidden tool
  while keeping both parent and worker execution policies authoritative.

## [5.3.5] - 2026-07-17

### Added

- Added `bing` as a first-class HTTP RSS search engine and exposed selected
  engines plus their request, configuration, or built-in-default origin in
  search result metadata.

### Changed

- Raised the bounded auditable `program` source limit to 192 KiB and retained
  compact search-routing fields when oversized child metadata passes through a
  batch workflow.

### Fixed

- Allowed hosts that supply an `LlmClient` in session options to bootstrap an
  agent without a configured default model, while preserving session-time
  validation when neither source is available.
- Retried an established response stream up to ten times within the same turn,
  with cancellation-aware exponential backoff and transactional rollback of
  provisional text, reasoning, and tool drafts between attempts.
- Treated OpenAI-compatible transport failures and cancellation before terminal
  evidence as incomplete streams instead of synthesizing a successful partial
  response; a received finish reason remains valid without a trailing
  `[DONE]` marker.

## [5.3.4] - 2026-07-16

### Fixed

- Prepared one saved source document per active language before a cold
  workspace-symbol search, so language servers that load projects on document
  open can serve the first semantic query instead of returning `No Project`.

## [5.3.3] - 2026-07-16

### Fixed

- Kept a shared language-runtime startup alive when the query that initiated it
  disconnects or is cancelled, so concurrent and subsequent semantic queries
  reuse one process generation instead of restarting it.
- Made language-runtime startup, source removal, and workspace shutdown use
  bounded generation-aware cleanup, preventing late readiness updates,
  overlapping replacement processes, and incomplete multi-language status.

## [5.3.2] - 2026-07-16

### Fixed

- Made interactive shell risk classification distinguish executable positions
  from harmless argument text, while retaining HITL for compact write options,
  output-producing `find` actions, and unparsed `sed` scripts.

## [5.3.1] - 2026-07-16

### Added

- Added the Search 2 native `anysearch` and `tavily` providers to the built-in
  `web_search` engine catalog while preserving the existing default engine
  selection for ordinary callers. Provider results retain normalized
  publication dates and stable contributing-engine metadata in JSON output.
- Added a typed model-generation concurrency contract and shared,
  cancellation-safe admission gate for structured generation. Providers that
  do not explicitly advertise safe parallel capacity default to
  single-flight. `generate_object` starts its active deadline only after
  admission, holds one permit across bounded schema-repair calls, and records
  queue wait, capacity, and active timeout in result metadata. A Flow step
  whose exact tool identity is `generate_object` acquires that same capacity
  before entering the bounded `program` VM, and the nested call reuses the
  one-shot identity-checked permit, so the VM deadline also excludes queue
  wait without permitting a foreign-gate or concurrent-reuse bypass.
- Added per-run snapshot hooks to `PermissionChecker` and
  `ConfirmationProvider`. Agent invocations bind the frozen checker and
  confirmation route into their tool context so delegated, parallel, Skill,
  and background child runs keep their admitted authority across later
  session-policy changes.
- Added an extended `BashSandbox` request/result contract and an
  `SrtBashSandbox` adapter with timeout, bounded streaming output, explicit
  environment, network denial, workspace/scratch write limits, protected
  control metadata, credential-read denial, and no unsandboxed fallback.
  Verified constructors now accept an exact npm installation and an explicitly
  selected Node executable so lifecycle-owning hosts do not rediscover either
  process from `PATH`.
- Added opt-in `LocalWorkspaceAccessPolicy::CredentialBoundary` enforcement
  for local and manifest-backed workspace services. Direct file reads,
  range reads, writes, and indexed or fallback grep now share the local
  command sandbox's credential and source-hardlink boundary while preserving
  ordinary package-store hardlinks. Guarded local Git diff first enumerates
  NUL-delimited changed paths and regenerates output only for allowed files.
- Delegation tools now publish a deterministic live catalog of visible worker
  names and purposes in both their descriptions and parameter schemas. Workers
  registered after session creation become model-discoverable on the next run;
  hidden or unregistered workers are omitted.
- Tool implementations can override their complete model-facing definition
  when descriptions or schemas depend on live runtime state.
- The Code TUI now projects A3S Use's built-in local PP-OCRv6 MCP and Skill
  surfaces, reports its pinned model and ONNX Runtime status, and provides the
  explicit `a3s install use/ocr` repair command.

### Removed

- Removed the unused `document_parser.ocr` backend configuration. OCR in Code
  is owned exclusively by A3S Use and runs locally through PP-OCRv6.

### Fixed

- Preferred the semantic HTML `<main>` element before falling back to `<body>`
  in `web_fetch`, removing site navigation and footer chrome from ordinary
  evidence pages while preserving the existing bounded conversion and SSRF
  boundary. The oversized fetch test module now lives in its own concern file.
- Decoded Anthropic and OpenAI-compatible SSE response bodies with one
  incremental UTF-8 decoder per stream. Multibyte text split across arbitrary
  network chunks now reaches event parsing intact instead of producing Unicode
  replacement characters in streamed research and conversation output.
- Made provider generation admission session-owned. Rebuilding an `AgentLoop`
  for each concurrent host-direct tool call now reuses the same typed capacity
  gate, so separate DynamicWorkflow calls cannot each manufacture an
  independent single-flight slot.
- Extended the local SRT credential boundary to existing `.env*` files at
  every governed source-tree depth and read/write-masked pre-existing
  multi-link source files, closing nested credential and hardlink-alias reads.
- Closed the built-in workspace-tool path around the local command sandbox:
  guarded `read`, `grep`, `write`, `edit`, and `patch` operations can no
  longer expose or mutate protected credential files or source hardlink
  aliases.
- Hardened read-only Git output: diff targets cannot be interpreted as
  options, guarded diffs omit sensitive and multi-link paths, and displayed
  remote URLs remove embedded HTTP credentials, query strings, and fragments.
- Auto-mode hosts can declare confirmation unavailable before Core emits a
  confirmation event. Unexpected parent or tool-owned escalation therefore
  fails closed instead of opening HITL or being automatically authorized.
- Propagated the parent sandbox and confirmation boundary into delegated,
  workflow, and Skill child runs. Child-local allow rules and automatic
  confirmation can narrow execution but can no longer replace the host
  boundary, and explicit Bash host escalation is now tool-declared.
- Live MCP additions and removals now refresh task delegation, and natural
  language action tokens can select matching MCP tools without requiring the
  caller to know their underscored protocol names.

## [5.3.0] - 2026-07-15

### Fixed

- Made language-server initialization and post-initialization settling
  cooperatively cancellable so session shutdown does not wait for a cold
  semantic runtime.
- Stabilized the first navigation query for each saved document revision after
  language-server initialization, preventing cold empty or partial reference
  results without treating a legitimate empty result as an error.

## [5.2.8] - 2026-07-16

### Added

- Added explainable four-level tool risk assessments for interactive Code
  hosts. Assessments expose capability type, target, impact, reversibility,
  environment sensitivity, stable reason codes, and explicit allow, confirm,
  constrained-review, or rule-deny routing actions.
- Added conservative aggregation for nested batch invocations and workspace
  symlink boundary assessments while preserving the existing
  `PermissionDecision` interface for backward-compatible HITL fallback.

### Changed

- Child and delegated runs now inherit the session's effective live Skill
  registry, including host-added Skills, instead of only the original session
  option.
- Permission policies now omit blanket-denied tools from the model-visible
  catalog. Deny-by-default workers expose only tools covered by an Allow or Ask
  rule, while argument-scoped rules remain execution-time enforced.

## [5.2.7] - 2026-07-15

### Fixed

- Normalized Rust Analyzer `linkedProjects` paths to protocol-safe forward
  slashes on Windows.
- Stabilized the Windows release gate for nested workflow cancellation and
  delegated permission inheritance under heavily loaded runners without
  weakening their side-effect or permission assertions.

## [5.2.6] - 2026-07-15

### Fixed

- Reaped the complete process group for cancelled blocking workspace discovery
  commands, so descendants holding inherited output streams cannot stall
  manifest shutdown.
- Kept process-group helpers warning-free on non-Unix targets and updated the
  durable workflow dependency to `a3s-flow` 0.4.2.

## [5.2.5] - 2026-07-15

### Added

- Added workspace-scoped Code Intelligence for saved-file symbols,
  definitions, declarations, references, implementations, and diagnostics,
  with native Rust and TypeScript/JavaScript language-server profiles.
- Added an argument-aware interactive tool guardrail for Code hosts. It keeps
  a non-bypassable safety floor while distinguishing narrow read-only actions
  from ordinary side effects that require approval.
- Added live `AgentSession` Skill lifecycle APIs and a dynamic Skill catalog
  context provider. Session-owned upgrades preserve the original shadowed
  Skill, removal is pointer-identity safe, built-ins remain protected, and
  session close releases all live registrations.
- Exposed live Skill lifecycle and cancellation-settlement APIs through the
  Node.js and Python SDKs, with aligned package versions and generated types.

### Fixed

- Retried only transient failed branches in read-only parallel tasks while
  preserving successful branch results and never replaying mutating work.
- Made context compaction account for fixed system-prompt and tool-schema
  tokens, preserve the latest user instruction and unresolved tool calls, and
  target a bounded post-compaction watermark.
- Made workspace manifest discovery cancellation-aware and kept watcher setup,
  ownership, and teardown outside Tokio's blocking pool. Hosts can now stop a
  manifest explicitly without runtime teardown waiting on platform watcher or
  filesystem operations.
- Bounded language-service shutdown and force-reaped the dedicated process
  group when a server closes its protocol streams without exiting.

## [5.2.4] - 2026-07-14

### Fixed

- Made rolling context compaction choose its retained suffix by estimated
  message tokens instead of a fixed message count. Compacted history now aims
  for half of its previous token footprint, bounds oversized summaries,
  preserves the latest safe tool-call boundary, and refuses replacements that
  would not reduce the estimated prompt.

## [5.2.3] - 2026-07-13

### Added

- Added model-window overrides for automatic context compaction through Rust
  `SessionOptions::with_max_context_tokens(...)`, Node.js
  `maxContextTokens`, and Python `max_context_tokens`.
- Added `Agent::replace_session_async(...)` for atomic live-session
  reconfiguration. A failed replacement leaves the current session registered
  and usable; a successful replacement keeps the same persisted session ID and
  closes the old runtime only after the registry swap.

### Changed

- Goal-tracked planning now evaluates and emits `goal_achieved` before the
  terminal `end` event. Hosts can therefore treat `end` as a reliable decision
  boundary and continue an unverified durable goal without racing a late
  achievement signal. Achievement evaluation now fails closed when its
  structured evaluator is unavailable; completion words alone can no longer
  terminate a host-owned goal loop.
- Reworked automatic context compaction into a repeatable rolling lifecycle.
  The agent now checks model-specific context usage before each request,
  includes exposed tool schemas in prompt accounting, preserves tool calls and
  results in summaries, bounds oversized tool output, and continues the active
  task after every successful compaction. `context_compacted` events include
  the cumulative summary so hosts with external timelines can persist the same
  compact generation without making a second summarization request.

## [5.2.2] - 2026-07-13

### Fixed

- Made cancelled headless-search cleanup synchronously close its tab
  semaphore before scheduling asynchronous process reaping, so saturated
  runtimes cannot admit new browser work or make shutdown tests timing-based.

## [5.2.1] - 2026-07-13

### Fixed

- Scoped the macOS system-proxy parser to macOS production builds and test
  builds, keeping Linux release Clippy free of platform-specific dead code.

## [5.2.0] - 2026-07-13

### Added

- Added resumable segmented output to the built-in `write` tool through
  `mode = "append"` and a required UTF-8 byte `expected_offset`. Matching
  retries are idempotent, unsafe offsets are rejected, version-aware workspace
  backends retain compare-and-swap protection, and append metadata stays
  bounded instead of carrying the complete before/after file payload.
- Added a shared `ToolCapabilities` contract covering read-only behavior,
  idempotency, resumability, cancellation safety, pagination, parallelism, and
  output kind. Orchestrators use the per-invocation contract instead of
  hard-coded tool-name lists.
- Added bounded continuation protocols to `read`, `ls`, `glob`, Git lists and
  diffs, and `web_fetch`. Git diff continuation and fetched-content continuation
  preserve UTF-8 boundaries and expose exact range metadata.

### Fixed

- Tool execution deadlines now cancel an invocation-scoped token and wait for
  bounded settlement before publishing the timeout result, both directly and
  through the session queue. `program` propagates the same token into its VM
  and nested calls, preventing timed-out scripts from leaving child operations
  running after their terminal event.
- Replaced permissive `patch` matching with strict, ordered unified-diff
  application. Hunk counts, source and destination positions, exact context,
  whitespace, no-newline markers, and LF/CRLF style are validated before any
  write, so a stale or malformed patch fails without modifying the file.
- Preserved functional, non-sensitive query parameters on the actual
  `web_fetch` HTTP request while keeping durable source anchors and diagnostics
  query-free. Credential, token, session, and signature parameters are removed
  before the request is sent.
- Paired nested batch/program `ToolExecutionStart` events with authoritative
  `ToolEnd` events, including output metadata, so UIs no longer mark completed
  inner calls as interrupted and delegated evidence can retain nested web
  source anchors.
- Allowed permission checkers to hide selected tool definitions from model
  requests independently of execution-time permission decisions. DeepResearch
  report turns can now expose only authoring tools while retaining a separate
  execution gate.
- Governed agent, nested, and session tool calls now validate arguments against
  each tool's cached JSON Schema before approval or side effects. Validation,
  cancellation, deadline, partial-failure, and rate-limit failures have typed
  error discriminants.
- Shell capture now reads bounded byte chunks instead of unbounded lines,
  retains useful head and tail output with exact byte accounting, and kills the
  complete Unix process group on timeout or cancellation.
- `batch` is bounded to 32 calls and 16 concurrent operations. Only tools that
  declare read-only, idempotent, cancellation-safe parallel capability fan out;
  mutations are serialized, and partial failure identifies only the failed
  indices for retry. `parallel_task` has the same 32-item bound and settles
  cancelled children before returning.
- Large edit metadata no longer duplicates complete files in event streams.
  It carries bounded previews and unified diff text plus sizes, hashes, and
  retained artifacts.
- `web_search` distinguishes complete, partial, and failed engine outcomes, so
  empty results accompanied by engine failures are no longer reported as a
  successful search. `generate_object` now bounds schema depth and size,
  prompt size, partial-event frequency, and its independent deadline.
- Local Git's default diff now returns the actual unified working-tree diff
  instead of only `--stat`; the tool bounds and paginates that diff safely.

## [5.1.0] - 2026-07-12

### Added

- Added the idempotent A3S Flow-to-Graph projection bridge, projection health
  snapshots, governed typed Flow decisions, and release benchmark tooling.
- Added compare-and-swap Graph event-store publication with strict-extension
  validation; the file backend coordinates independent processes through an
  OS-level lock to prevent lost updates.
- Added durable leased Flow-decision ledgers with cross-process claims,
  completed receipts, request-identity conflicts, and crash-recovery takeover.
- Added a deterministic end-to-end Agent convergence benchmark covering task
  completion, tool-loop termination, checkpoint resume, usage, cost, and
  latency metrics in versioned JSON.
- Added real AgentLoop gates through an existing local Codex login, covering a
  workspace tool round, convergence, token accounting, checkpoint cleanup, and
  cumulative resume state with bounded call timeouts.

### Changed

- Updated a3s-search to 1.4.1, exposed explicit browser runtime lifecycle APIs,
  and added per-invocation search health and latency metadata to `web_search`.
- Graph record schema v2 uses incremental structural state hashes and
  touched-entity patch validation while retaining v1 and mixed v1-to-v2 replay.
- Agent no-progress guards now terminate repeated blocked tool calls and
  incomplete replies, and checkpoint resume preserves cumulative turn and
  convergence budgets using redacted fingerprints.
- Flow decision dispatchers now renew leased claims while sinks run and cancel
  in-flight sink futures when ownership is lost.
- Flow decision dispatchers expose cancellation-safe health snapshots for
  claims, contention, takeovers, renewals, failures, latency, and in-flight
  work.

## [5.0.0] - 2026-07-11

### Added

- Added an event-sourced reactive state graph with typed objects and relations,
  causal and tamper-evident event records, predicate-scoped behaviors,
  optimistic transactional patches, strict replay, event-point forks,
  structural branch diffs, and atomic memory/file event stores. Node and
  Python expose matching `StateGraphRuntime` patch, restore, fork, and diff
  surfaces.
- Added a versioned, lossless event envelope shared by the Rust core and both
  SDKs, including generated event catalogs and forward-compatible preservation
  of unknown event types, payloads, and metadata.
- Added async-first Rust session construction through `SessionBuilder::build`,
  `session_async`, async agent/worker factories, and `resume_session_async`, with
  typed configuration and resource-initialization errors.
- Added `SessionSnapshotV1`, a versioned aggregate containing conversation
  state, artifacts, traces, run records, verification reports, and delegated
  task snapshots. File and memory stores commit the aggregate as one atomic
  generation while retaining legacy fragmented-load compatibility.
- Added sanitized `source_anchors` to delegated `TaskResult`, `StepOutcome`, and
  successful background-task snapshots. Successful `read`, `grep`,
  `web_search`, and `web_fetch` calls now carry only normalized workspace paths
  or credential/query-free URLs, so hosts can distinguish runtime-observed
  sources from model-attested strings.

### Changed

- `web_search` now falls back to DuckDuckGo and Wikipedia when a request
  contains only known browser-backed engines such as Baidu or Bing China but
  no headless browser is configured. Unknown engine names still return a
  configuration error.
- Split streamed tool preparation from execution lifecycle events:
  `ToolStart` now opens a stable call ID, `ToolExecutionStart` carries the
  authorized arguments when execution actually begins, input deltas retain
  provider call IDs, and `ToolEnd` includes authoritative arguments for
  lossless replay and UI projection.
- Moved A3S Code dynamic workflow history from the former sibling state
  directory to the project-local `.a3s/workflow` directory.
- Session construction now validates and asynchronously resolves memory,
  persistence, queue, trajectory, and MCP resources once before assembly. The
  synchronous Rust factory is a non-blocking compatibility path that requires
  an explicit pre-initialized memory store and returns
  `AsyncSessionBuildRequired` for async-only configuration.
- **Rust migration:** callers that relied on `Agent::session` to create default
  or file-backed resources must switch to `session_async(...).await` or
  `session_builder(...).build().await`. The synchronous factory remains only for
  explicitly pre-initialized memory and other already-ready resources; persisted
  resume uses `resume_session_async(...).await`.
- **Rust migration:** external `TaskResult`, `StepOutcome`, and
  `SubagentTaskSnapshot` struct literals must initialize the new
  `source_anchors` field (usually with `Vec::new()`). Existing serialized values
  remain readable because the field defaults to an empty list during
  deserialization.
- Conversation operations are now fail-fast single-flight per session. An
  overlapping send, stream, attachment call, slash command, or run resumption
  returns `SessionBusy`, and streaming retains admission until its runtime has
  actually finished.
- Run identity, cancellation, events, and governance now travel in one
  invocation context. Scoped LLM and tool invokers apply cancellation, budget,
  policy, hooks, confirmation, queue/timeout, recursion protection, and output
  sanitization at the provider/tool boundaries, including nested and delegated
  work.
- Direct SDK tool helpers now use an explicit trusted host-control-plane policy:
  they bypass model-facing permission and confirmation decisions but still use
  hooks, budget checks, queue/timeout handling, cancellation, recursion
  protection, and output sanitization.
- Session MCP mutation is isolated from agent-global and host-supplied managers.
  Each session owns its live manager while inherited managers remain read-only
  capability sources and are propagated to delegated child agents.

### Removed

- Removed the in-core AHP integration and its SDK bindings. External governance
  now composes through the unified hook, permission, confirmation, and scoped
  invocation boundaries instead of a second parallel control plane.
- Removed the unused internal file-history module; durable session snapshots,
  run events, artifacts, and workspace version checks are the maintained
  persistence and conflict-detection paths.

## [4.3.3] - 2026-07-09

### Added

- Added real structured-output integration coverage for local Codex login
  models, using `~/.codex/auth.json` through a test-only `LlmClient`
  adapter.

### Changed

- Aligned additional Rust core APIs with the Node and Python SDK surfaces,
  including direct tool execution, session options, runtime metadata, and
  release API contract checks.
- Removed the bundled built-in skill markdown files so skills are supplied by
  projects instead of silently shipping with the core crate.
- Improved structured JSON generation and `generate_object` reliability with
  schema envelopes for top-level arrays/scalars, safer provider fallback
  routing, streaming final deltas, raw-text opt-in output, and stronger
  metadata.
- Switched planner pre-analysis to the shared structured-output path so planning
  JSON benefits from the same extraction and repair logic.

### Fixed

- Preserved `SearchResult::published_date` in both JSON and default text
  `web_search` output so research clients can compare source recency instead of
  losing engine dates. Engine provenance is now emitted in stable sorted order.
- Sanitized HTTP(S) URLs in `web_search` queries at the Core tool boundary.
  Search engines and result headers retain the useful base URL without receiving
  embedded credentials, query parameters, or fragments from any caller.
- Made the initial `web_fetch` network request use the same credential/query/
  fragment-free URL stored in source metadata, and sanitized URL-bearing fetch
  errors. Ordinary TUI calls can no longer send a sensitive raw URL while only
  presenting a safe anchor afterward.
- Unified `web_search` result URL safety across JSON, text, and source-anchor
  metadata. Credentials, query parameters, and fragments are removed before
  results reach the model, and non-HTTP(S) result URLs are excluded before the
  requested result limit is applied. HTTP(S) URLs embedded in result titles and
  snippets receive the same safe projection.
- Accepted both `data:` and `data: ` SSE frames so OpenAI-compatible streaming
  providers that omit the optional space no longer produce empty model output.
- Updated release workspace setup for the current `a3s-lane` and `a3s-search`
  dependency versions used by GitHub Actions standalone checkouts.

## [4.3.2] - 2026-07-08

### Changed

- Reserved by a prior crates.io-only core package publish. The complete
  multi-channel release continues in 4.3.3.

## [4.3.1] - 2026-07-07

### Fixed

- DynamicWorkflowRuntime PTC scripts can now call tools through the legacy
  `ctx.tools.<name>(args)` proxy as well as `ctx.tool(name, args)`, preserving
  the existing allow-list, call-count, and output-size limits.

## [4.3.0] - 2026-07-04

### Added

- Sessions now resolve a default memory store even when callers do not pass one,
  preferring an explicit store, then a file memory directory, then configured
  `memory_dir`, then `<workspace>/.a3s/memory`, with an in-memory fallback and
  init warning if the file store cannot be created.
- LLM memory extraction is enabled by default and runs behind a completed-turn
  significance gate instead of mechanically extracting after every input or tool
  result. Extraction prompts include related existing memories, including
  existing `supersedes` / `conflicts_with` relation metadata when available, so
  the model can consolidate or preserve conflicts. Streaming runs schedule gated
  extraction in the background after the final response event so UI completion is
  not blocked by the maintenance LLM call; each memory instance now also runs at
  most one background extraction at a time to keep maintenance calls bounded.

### Changed

- Successful tool outputs are no longer mechanically written when LLM memory
  extraction is enabled; failures are still stored immediately.
- Recalled memory context now carries memory metadata plus concise relation
  annotations for `supersedes` and `conflicts_with` when present.
- Memory recall now preserves query-specific search ranking when injecting
  memories into prompt context, so precise matches are not drowned out by generic
  high-importance memories. The LLM extraction parser also keeps valid extracted
  memories when a sibling item is malformed.
- Session memory now consumes the store's canonical returned item, so store-level
  duplicate consolidation keeps short-term memory and emitted memory ids aligned
  with the durable item that actually represents the fact.
- LLM memory extraction now merges near-duplicate extracted memories into the
  existing canonical item instead of silently discarding the improved wording,
  tags, importance, or provenance metadata.
- Default memory stores now also perform conservative near-duplicate
  consolidation and conflict-safe pruning, so manually written memories and LLM
  extracted memories share the same canonical-item lifecycle.

## [4.2.1] - 2026-06-23

### Fixed

- Python SDK wheel build (and therefore the PyPI + GitHub Release publish steps)
  failed for 4.2.0: the `PyAutoDelegationConfig → AutoDelegationConfig`
  conversion used a struct literal that omitted the `allow_manual_delegation`
  field added to `AutoDelegationConfig`. The conversion now falls back to core
  defaults for fields the Python SDK does not expose, so future core fields no
  longer break the wheel build. (The crates.io and npm 4.2.0 artifacts were
  unaffected; 4.2.1 completes the release across all channels.)

## [4.2.0] - 2026-06-23

### Added

- Native structured-output enforcement. `LlmClient` gains
  `native_structured_support()`, `complete_structured()`, and
  `complete_streaming_structured()` (all with non-breaking default impls). The
  structured engine now forces the provider `tool_choice` for tool mode and
  requests native `response_format` (`json_schema` / `json_object`) where the
  provider supports it, instead of merely offering a tool the model could ignore.

### Fixed

- Stabilized JSON-object generation. Forced `tool_choice` on both the blocking
  and streaming paths guarantees the model emits the structured object rather
  than prose or malformed tool arguments.
- Hardened the planner / pre-analysis JSON parsing: it now reuses the robust
  shared extractor (markdown fences, surrounding prose, braces inside strings)
  and adds one repair retry, replacing the previous naive first-`{`/last-`}`
  slice that hard-errored on fenced or prose-wrapped output.

## [4.1.0] - 2026-06-23

### Changed

- Active skill `allowed-tools` no longer globally deny ordinary session tool calls
  by default. Tool calls continue through permission policy, hooks, and HITL;
  `SessionOptions::with_active_skill_tool_restrictions(true)` (Node:
  `enforceActiveSkillToolRestrictions`) restores the legacy global restriction
  behavior. Skill-local execution still enforces each skill's `allowed-tools`.

## [4.0.0] - 2026-06-21

Milestone release: **filesystem-first agents**. A single directory
now defines a durable agent by convention — `instructions.md` (role slot),
`agent.acl` (config), `skills/`, `schedules/` (cron), and `tools/` — served by a
`serve` daemon that runs each schedule as a full harness turn. No breaking changes
to existing APIs; the new surface is additive and gated behind the `serve` feature.

### Added

- **Filesystem-first agent directories (`AgentDir`) + the `serve` daemon.** A
  directory with a required `instructions.md` (injected as a prompt *slot*, so the
  harness keeps `BOUNDARIES`/response-format/verification authoritative) plus
  optional `agent.acl`, `skills/`, `schedules/`, and `tools/` loads via
  `AgentDir::load` into existing config objects — no new runtime, no new prompt
  system. `serve_agent_dir` runs each enabled cron schedule on its own durable
  `schedule:<name>` session; every fire is a FULL `AgentSession::send` turn
  (context, tool visibility, safety gate, verification), never a raw model call.
  Cron accepts 5- and 6-field expressions (UTC). Exposed in the Node and Python
  SDKs (`serveAgentDir` / `serve_agent_dir`) returning a `ServeHandle`. Gated
  behind the `serve` Cargo feature.
- **`tools/` declarative tools — `kind: mcp`.** A `tools/<name>.md` with
  `kind: mcp` registers an MCP server into each schedule session through the
  normal `add_mcp_server` path (namespaced `mcp__<server>__<tool>`, gated by the
  session permission policy). Duplicate names and unknown kinds fail closed at load.
- **Rehydrate-on-boot for the serve daemon.** When a `SessionStore` is configured
  (e.g. via `SessionOptions::with_file_session_store`), `serve_agent_dir` now
  *resumes* any schedule whose `schedule:<name>` session already exists in the
  store instead of starting it fresh, so a daemon restart keeps the accumulated
  conversation context. Resume restores history only — the current
  `instructions.md` / `skills/` / `tools/` are re-applied each boot, so editing
  the agent dir still takes effect. With no store configured, every boot starts
  fresh (unchanged). Reuses the existing `Agent::resume_session` path; no new
  persistence machinery.
- **Sandboxed `script` tools for filesystem-first agents (`tools/ kind: script`).**
  A `tools/<name>.md` with `kind: script` now becomes a model-visible tool backed
  by the existing sandboxed QuickJS `program` path — no new sandbox. The spec pins
  the workspace-relative `.js`/`.mjs` `path`, the `allowed_tools` allow-list, and
  the `limits` (timeout / tool-calls / output); the model supplies only `inputs`.
  - New `AgentDirScriptTool` registers through the same non-shadowing
    `register_dynamic_tool` path as builtins/MCP, so a `tools/` entry can add a
    name but never replace a builtin. The model's call to the script tool is
    permission-gated like any tool. At the time of this release, the script's
    *inner* `ctx.tool` calls were bounded only by the pinned `allowed_tools` list
    + QuickJS sandbox (no fs/net/proc/env); since 5.0.0 they also re-enter the
    session's governed permission/HITL path. The complement to `kind: mcp` (both
    now ship).
  - The `allowed_tools` list is an independent security boundary for a directory
    script, so the loader **fails it closed**: an omitted list grants NO tools
    (not all of them); list only the minimum, and avoid high-authority tools unless
    the directory is fully trusted.
  - Fails closed at load (not at first call): a non-`.js`/`.mjs` `path`, a path
    that escapes the workspace (absolute / `..`), an out-of-range sandbox limit
    (zero, or an effectively-unbounded `timeoutMs`), an unknown `kind`, or a
    duplicate tool name is a directory-load error. A `tools/` file is semi-trusted,
    so limits are bounded (≤10 min / ≤1000 calls / ≤16 MiB).
  - The serve daemon installs the agent dir's `tools/` into every schedule
    session, so scheduled turns can call them.

## [3.6.2] - 2026-06-14

Release-engineering fix for 3.6.0/3.6.1 (no library code changes). Both prior
tags published `a3s-code-core` to crates.io but failed every native SDK build,
so npm / PyPI / GitHub Release were skipped.

True root cause: **`sdk/{node,python}/Cargo.lock` were git-ignored** (never
committed), so CI resolved the SDK dependency graph fresh on each release. A
newly-published `alloc-no-stdlib 3.0.0` then got pulled alongside `2.0.4`,
producing a duplicate that breaks `brotli 8.0.3`'s `StandardAlloc: Allocator`
impl. (The 3.6.1 toolchain pin was a misdiagnosis — the build fails the same way
on any toolchain when the lock isn't honored.)

### Fixed

- **Commit the SDK lockfiles** — `sdk/node/Cargo.lock` and
  `sdk/python/Cargo.lock` are now tracked (removed from `.gitignore`), pinning a
  single consistent `alloc-no-stdlib 2.0.4` + `brotli 8.0.3`. CI now builds the
  exact, locally-verified resolution instead of re-resolving. Verified with the
  real `cargo build --release` for both SDKs (not just `cargo check`).

## [3.6.1] - 2026-06-14

Release-engineering fix for 3.6.0 (no library code changes). The 3.6.0 tag
published `a3s-code-core` to crates.io, but the native SDK build jobs failed
because **brotli 8.0.3** (pulled transitively via `tower-http` under optional SDK
features) no longer compiles on the newest stable Rust — so the npm,
PyPI, and GitHub Release artifacts were skipped. This release ships those.

### Fixed

- **Pin the SDK build toolchain** — `publish-node.yml` / `publish-python.yml`
  now use `dtolnay/rust-toolchain@1.94.1` (and pass `rust-toolchain: 1.94.1` to
  `maturin-action`) instead of `@stable`, restoring the known-good build that
  shipped earlier releases. Revisit when the upstream brotli/toolchain break
  clears.
- **Refresh the SDK lockfiles** — `sdk/{node,python}/Cargo.lock` now pin
  `a3s-code-core` to the release version (previously left stale, which forced a
  dependency re-resolution that pulled a second `alloc-no-stdlib`).

## [3.6.0] - 2026-06-14

A system-prompt hardening pass plus framework-vs-host boundary tightening:
inject safety boundaries and live environment grounding, redact secrets from
logs, and wire the LLM-client extension seam through the public API.

### Added

- **`<env>` grounding block** — every augmented system prompt now carries a small,
  always-on environment block (today's date, host platform, working directory),
  computed fresh each turn in `turn_context.rs` (no shell-out). Most importantly
  it pins the current date, which the model otherwise cannot infer past its
  training cutoff.
- **`SessionOptions::with_llm_client`** — hosts can now inject a custom
  `Arc<dyn LlmClient>` (custom/unsupported provider, deterministic record/replay
  client, or HTTP proxy/audit wrapper). The `LlmClient` trait and `Arc<dyn>`
  engine already existed but were only injectable in test code; this wires the
  seam through the public API, bringing the Action-layer backend to parity with
  workspace/memory/store/security (all object-injectable). The `provider/model`
  factory remains the default when unset.

### Security

- **Tool-argument log redaction** — `ToolExecutor` no longer logs raw tool
  arguments at `info!` (which were also exported to OTLP). Bash commands and
  `write`/`edit` file contents can contain secrets; invocations now log only the
  tool name, sorted argument field names, and payload byte size. Full args remain
  available at `trace!` for local debugging. Backs the new "never log secrets"
  prompt boundary.

### Changed

- **System-prompt safety boundaries** — every assembled system prompt (all agent
  styles and delegated subagents) now carries a `## Boundaries` section
  (injection hygiene: treat file/tool/web content as untrusted data, not
  commands; secret handling; defensive-security-only) from a single source
  (`prompts/common/boundaries.md`), injected once in
  `SystemPromptSlots::build_with_style`.
- **Default prompt guidance** — added a library-availability rule (confirm a
  dependency before using it) and clarified that the dedicated read/search/edit
  tools are preferred over shelling out (`cat`/`sed`/`grep`/`find`); `bash` is
  for running commands, builds, and tests. Response-format guidance now
  discourages re-printing already-read code and creating unsolicited report
  `.md` files.

### Fixed

- **Framework no longer writes to the host's stderr** — replaced a stray
  `eprintln!("[DEBUG] HTTP error...")` in the OpenAI client (fired on every
  transport error, also leaking into the host terminal) with `tracing::error!`,
  consistent with the rest of the crate.

### Removed

- **Dead `Planner` trait** (`planning::Planner`) — it was re-exported but had no
  `dyn Planner` dispatch and no consumer; every call site uses `LlmPlanner`'s
  inherent methods directly. Removed per the pruning rule (the real variability,
  the LLM, is swappable via `with_llm_client`). `LlmPlanner` is unchanged.

## [3.4.0] - 2026-05-30

Programmable, deterministic multi-agent orchestration — a grammar for
expressing fan-out, pipelines, and resumable workflows in code (not only via
model-driven delegation), drawn along the framework / host boundary:
the framework owns the grammar + serializable contracts; the host owns
placement, transport, and scheduling. All additions are backward compatible
(new types/methods, new optional fields, new `SessionStore` methods with
default no-op impls).

### Added

- **`AgentExecutor` seam** (`orchestration` module) — the boundary between the
  orchestration grammar and the host's placement/transport/scheduling. The
  in-box `TaskExecutor` runs each step as a child agent locally; a host
  substitutes its own executor to place steps across a cluster.
  `concurrency_hint()` is advisory, not a hard local bound, so orchestration
  scales past a single process. `AgentSession::agent_executor()` /
  `session_store()` expose a session-backed executor + its store.
- **Serializable step contracts** — `AgentStepSpec` (`task_id` / `agent` /
  `description` / `prompt` / `max_steps?` / `parent_session_id?` /
  `output_schema?`) and `StepOutcome` (`+ structured?`), serializable for
  cross-node transport and checkpoints.
- **Combinators** —
  - `execute_steps_parallel` — barrier fan-out, input-order preserving,
    per-branch panic isolation, bounded by the executor's concurrency hint.
  - `execute_pipeline` — per-item chains through stages with **no inter-stage
    barrier** (item A can be in stage 3 while item B is still in stage 1);
    stages are pure spec-builders that branch on the prior outcome.
  - `execute_steps_parallel_resumable` — journals completed steps to a
    `SessionStore` at each step boundary; on resume it skips completed steps
    and re-dispatches the rest. Records only successful steps (a failed step
    retries on resume). The checkpoint is serializable, so a host can resume an
    interrupted workflow on a *different* node.
- **Schema-forced step output** — a step carrying `output_schema` returns a
  schema-validated object in `StepOutcome.structured` (reuses the
  structured-output coercion + repair). A coercion failure demotes the step to
  unsuccessful, so callers never treat unvalidated text as the promised object.
- **`WorkflowCheckpoint`** (`schema_version` / `workflow_id` / `steps` /
  `checkpoint_ms`) + `SessionStore::{save,load,delete}_workflow_checkpoint`
  (default no-ops; the file store writes crash-atomically). Loads from a
  future, incompatible schema version are rejected.
- **SDK grammar (Node + Python)** — `session.parallel(specs)`,
  `session.pipeline(items, stages)`, `session.parallelResumable(specs,
  workflowId)` (Node, camelCase) / `parallel` / `pipeline` /
  `parallel_resumable` (Python, snake_case). Pipeline stages are JS/Python
  callbacks `(ctx) -> spec | null`; the bridges fail closed — a hung,
  null-returning, or raising stage stops only its own chain. (A Node stage
  callback must not throw — return `null` on error, same constraint as
  `setBudgetGuard`.)
- **`LoopCheckpoint::ensure_loadable()`** — loads from a future, incompatible
  loop-checkpoint schema version are now rejected at the store layer (both
  file and memory), honoring the documented contract.

### Changed

- The resumable combinator now distinguishes "no checkpoint" from an
  *unreadable* one: an unreadable (e.g. future-version) checkpoint logs a
  warning and re-runs the workflow from scratch rather than silently swallowing
  the error.
- Documented the FFI panic-safety contract in each SDK's module doc (napi 2.x
  does not catch panics in sync `#[napi]` bodies by default; PyO3 0.23 catches
  `#[pyfunction]` / `#[pymethods]` bodies). No code change — both boundaries
  were audited panic-safe.

### Tests

- Persisted-schema round-trip fuzz extended to the new migratable types
  (`AgentStepSpec`, `StepOutcome`, `WorkflowCheckpoint`) — round-trip stability
  + forward/backward compat. Comprehensive unit **and** real-LLM integration
  tests for the orchestration layer (parallel fan-out, multi-item pipeline, the
  resume path, nested-schema coercion) run against `.a3s/config.acl`.

## [3.3.0] - 2026-05-29

Cluster-grade runtime: everything needed for a host platform
to run long-lived agent sessions across many nodes — graceful shutdown,
multi-tenant identity, cost governance, deterministic replay, crash-tolerant
runs, and bounded in-memory state — plus an adversarial-review hardening
pass. All additions are backward compatible (new methods, new optional
fields, new `SessionStore` trait methods with default no-op impls).

### Added

- **Session / Agent lifecycle control.**
  - `AgentSession::close()` is now a full graceful stop: flips `is_closed`
    (further `send`/`stream` fast-fail with `CodeError::SessionClosed`),
    cancels the active run, all in-flight delegated subagent tasks, and
    pending HITL confirmations. `AgentSession::is_closed()` accessor.
  - Agent-side session registry: `Agent::list_sessions()`,
    `close_session(id)`, `close()` (also disconnects global MCP), and
    `is_closed()`. Sessions are tracked by `Weak` ref and pruned lazily.
  - Session-level `CancellationToken` parent: every run derives its token
    via `child_token()`, so `close()` cascades to all in-flight work.
    `AgentSession::session_cancel_token()` exposes it for embedders.
- **Host-provided identity labels** — `tenant_id`, `principal`,
  `agent_template_id`, `correlation_id` on `SessionOptions` (builder
  methods + accessors), persisted in `SessionData`, restored on resume.
  Framework treats them as opaque; the host drives multi-tenant
  aggregation / billing / tracing. Exposed on both SDKs.
- **`BudgetGuard` cost/quota contract** (`budget` module) — host-supplied
  `check_before_llm` / `record_after_llm` / `check_before_tool`, consulted
  at the LLM call site. `Deny` aborts with `CodeError::BudgetExhausted`;
  `SoftLimit` emits an event and proceeds. SDK bridges: a Python class
  (`opts.budget_guard`) and Node `session.setBudgetGuard({...})`. The Node
  bridge fails **closed** (timeout / unreadable return → deny).
- **`HostEnv` (IdGenerator + Clock) injection** (`host_env` module) —
  replace the default UUID + wall-clock pair for deterministic replay of a
  run on another node. `SequentialIdGenerator` / `FixedClock` helpers.
- **Loop checkpoints + run resumption** (`loop_checkpoint` module) — the
  agent loop persists a `LoopCheckpoint` after each completed tool round
  (when a `SessionStore` is configured); `AgentSession::resume_run(run_id)`
  replays from the last boundary on any node sharing the store, continuing
  cumulative token/tool-call accounting. `SessionStore` gains
  `save/load/delete_loop_checkpoint`; file writes are crash-atomic.
- **`SessionRetentionLimits`** (`retention` module) — optional FIFO caps on
  the in-memory run store (runs + per-run events), trace sink, and terminal
  subagent task snapshots, so long-running sessions don't grow unbounded.
  Exposed on both SDKs. Default is unbounded (no behavior change).
- **MCP idle disconnect** — `McpManager::disconnect_idle(threshold_ms)` and
  `Agent::disconnect_idle_mcp(...)` (both SDKs) reap quiet MCP servers
  (releasing FDs / background workers) while keeping their config for
  on-demand reconnect.
- **Cluster `AgentEvent` variants** — `BudgetThresholdHit`,
  `PassivationRequested`, `PeerInvocation`: platform-level events a host
  emits via `HookExecutor` so in-session code can react uniformly.
- `SessionStore` now persists the subagent task tracker across
  save/resume (`save/load_subagent_tasks`), so a migrated session keeps a
  queryable history of its delegated child runs.
- New errors: `CodeError::SessionClosed`, `CodeError::BudgetExhausted`.

### Changed

- `resume_run` continues cumulative metrics (`total_usage`,
  `tool_calls_count`) from the checkpoint instead of restarting at zero.
- Run-store and subagent-tracker FIFO eviction now hold their parallel
  maps under a single canonical lock order, so eviction is atomic with
  respect to concurrent record/cancel (no transient map inconsistency).

### Fixed

- **Loop checkpoint leak**: checkpoints were written after every tool round
  but never deleted — unbounded disk/memory growth on every completed run.
  They are now removed when a run reaches a terminal state in-process; only
  a true crash leaves one for resume.
- **`event_count` corruption**: restoring a session whose per-run event
  buffer had been trimmed reset the cumulative `event_count` to the trimmed
  length. The persisted cumulative count is now preserved.
- **Node `BudgetGuard` fail-open**: a hung or slow guard silently *allowed*
  the LLM call (disabling enforcement). It now fails **closed** (deny) on
  timeout and on an unreadable return.
- **MCP timestamp leak**: `touch()`-without-connect orphan timestamps are
  now purged by `disconnect_idle`.
- Session registry dangling `Weak` entries are pruned on `Agent::close()`.

### Known limitations

- Node `BudgetGuard` callbacks **must not throw** — due to a napi-rs
  constraint a thrown exception aborts the host process at return-value
  conversion. Wrap guard logic in try/catch and return a decision. Hangs
  are handled safely (fail-closed timeout). The Python `BudgetGuard`
  catches exceptions and is unaffected.

## [3.2.1] - 2026-05-24

### Added

- Python SDK: small pure-Python **bootstrap** shim published to PyPI as
  `a3s-code`. On first `import a3s_code` it downloads the matching
  native wheel for the current interpreter/platform from this repo's
  GitHub Releases, verifies the wheel's sha256 against the release
  manifest, extracts the compiled `_native` extension into
  `~/.cache/a3s-code/<version>/`, and registers it as
  `sys.modules["a3s_code._native"]`. Subsequent imports use the cache.
  Source under `sdk/python-bootstrap/`.
  - Environment knobs: `A3S_CODE_CACHE_DIR`, `A3S_CODE_RELEASES_BASE_URL`,
    `A3S_CODE_SKIP_HASH_CHECK`.
  - 15 unit tests + 1 live download test gated on
    `A3S_CODE_BOOTSTRAP_LIVE=1`.
  - New workflow `publish-python-bootstrap.yml`, wired after
    `publish-python` in `release.yml`.
- `scripts/check_release_versions.sh` now also validates the bootstrap
  package version and the runtime `__version__` literal.
- `release.sh` now bumps the bootstrap version in lockstep with the
  core release.

### Fixed

- `pip install a3s-code` works again from v3.2.1, restored after v3.2.0
  could only push a single wheel to PyPI under the quota cap.

## [3.2.0] - 2026-05-24

### Added

- Added a queryable subagent task tracker so callers can observe delegated
  child runs by `task_id` instead of scanning `run_events()`. The tracker is
  a materialized view over the existing `SubagentStart` / `SubagentProgress`
  / `SubagentEnd` event stream — the stream remains the authoritative record.
- Added three new APIs on `AgentSession` (and mirrored bindings on the Node
  and Python SDKs):
  - `subagent_task(task_id)` — look up a task snapshot by id.
  - `subagent_tasks()` — list every delegated subagent task observed in this
    session, oldest first.
  - `pending_subagent_tasks()` — list only tasks still in `running` state.
- Added emission of `SubagentProgress` events from the child loop forwarder.
  Two milestones are surfaced today: `status = "tool_completed"` after each
  child tool ends (metadata: tool, exit_code, output_bytes, optional
  error_kind) and `status = "turn_completed"` after each child LLM turn
  (metadata: turn, prompt/completion/total tokens). Noisy events (TextDelta,
  ToolStart, ToolOutputDelta, nested subagent events) are intentionally not
  translated; consumers needing token-level streaming should subscribe to the
  raw event stream directly.
- Added `SubagentStatus::Cancelled` and `AgentSession::cancel_subagent_task(id)`
  for interrupting in-flight delegated child runs without cancelling the parent
  run. Bindings on both SDKs (`session.cancelSubagentTask(taskId)` /
  `session.cancel_subagent_task(task_id)`). A late `SubagentEnd` from a
  cancelled child does not downgrade the terminal status — it stays
  `Cancelled`.
- Added `SubagentTaskSnapshot` carrying `task_id`, `parent_session_id`,
  `child_session_id`, `agent`, `description`, `status`, `started_ms`,
  `updated_ms`, optional `finished_ms` / `output` / `success`, and a
  `progress` log. The Cancellation path also propagates a real cancellation
  token into the child loop via `AgentLoop::execute_with_session`, so the
  signal honors existing LLM-streaming yield points.
- Added `InMemorySubagentTaskTracker` and `SubagentProgressEntry` to the
  public crate-root re-exports of `a3s-code-core` alongside the existing
  `SubagentStatus` / `SubagentTaskSnapshot` types.

### Changed

- Marked `SubagentStatus` `#[non_exhaustive]` so future variants can be added
  without a major version bump.
- Reshaped the Node SDK type layout to survive `napi-rs` regeneration. The
  build now writes generated declarations to `generated.d.ts`; hand-authored
  types that mirror JSON wire shapes (`ToolErrorKind`, `VerificationStatus`,
  `VerificationCheck`, `VerificationReport`, `ToolArtifact`) now live in
  `extra-types.d.ts`; the published `index.d.ts` is a small hand-authored
  aggregator that re-exports both. The `types` field in `package.json` still
  points at `index.d.ts`, so consumer imports are unchanged. A new
  `npm run test:types` script type-checks the aggregator to guard against
  future regressions.

### Fixed

- Fixed `TaskExecutor::execute` and `execute_background` so the emitted
  `SubagentStart` carries the real parent session id (previously
  `String::new()`), and `execute_background` returns the same `task_id`
  that appears in lifecycle events (previously a throwaway id). The
  background path also pre-emits `SubagentStart` synchronously so callers
  that query the tracker immediately after scheduling do not race the
  spawned task.

### Packaging

- The Python SDK no longer ships native wheels to PyPI. The project
  grew past PyPI's default 10 GB per-project quota, and binary wheels
  for the full Rust × CPython × platform matrix consume that budget
  fast. From v3.2.0 onwards the canonical wheel host is GitHub
  Releases — see README for the install command. Versions up to 3.1.0
  remain installable from PyPI for backward compatibility.

### Breaking

- `TaskExecutor::execute`, `execute_parallel`, and `execute_background` now
  take an additional `parent_session_id: Option<&str>` (or
  `Option<String>` for the background variant) so the emitted lifecycle
  events can be correctly associated with the parent session. Direct
  callers of `TaskExecutor` need to pass `None` (or the parent session id)
  to keep current behavior.
- `register_task_with_mcp` gained a trailing
  `subagent_tracker: Option<Arc<InMemorySubagentTaskTracker>>` parameter so
  the session bootstrap path can share a single tracker Arc with the
  executor and the live `AgentSession`. Pass `None` to opt out.

## [3.1.0] - 2026-05-23

### Added

- Added Claude Code-style automatic subagent delegation. When a request matches
  multiple independent specialists, the runtime can pre-run those child agents
  through one bounded `parallel_task` call and feed the gathered context back
  into the main turn.
- Added `auto_delegation` configuration and session overrides, including the
  global `auto_parallel` kill switch. Setting `auto_parallel = false` disables
  automatic parallel child-agent fan-out while keeping manual `parallel_task`
  available.
- Added `auto_delegation.allow_manual_delegation` and
  `SessionOptions::with_manual_delegation_enabled(...)` so hosts can hide the
  model-visible `task` / `parallel_task` tools per session while preserving the
  child-agent registry for introspection and worker registration. This is an
  operational cost/debug control, not a security sandbox.
- Added `max_parallel_tasks` as the shared sibling fan-out limit for
  `parallel_task`, delegated plan waves, and safe parallel write batches.
- Added a reusable ordered parallel executor so concurrent child results remain
  deterministic and individual task failures are isolated.
- Added native `.a3s/agents` and `~/.a3s/agents` subagent discovery with
  recursive loading. `.claude/agents` remains a compatibility source, but
  `.a3s/agents` wins when the same agent name appears in both locations.
- Added Claude-style markdown agent compatibility for `tools`,
  `allowedTools`, and `disallowedTools` frontmatter. `tools` behaves as an
  allowlist and `disallowedTools` is a denylist that takes precedence.
- Added direct worker/subagent APIs across Rust, Node.js, and Python:
  `WorkerAgentSpec`, `AgentDefinition`, `session_for_worker`, live worker
  registration, and `task` / `parallel_task` helpers.
- Added real-provider smoke coverage for automatic parallel delegation and
  built-in subagent execution using `.a3s/config.acl`.

### Changed

- Aligned built-in subagent names and aliases with Claude Code conventions:
  `general-purpose` aliases to `general`; `verify` / `verifier` alias to
  `verification`; `code-review` / `reviewer` alias to `review`.
- Tightened built-in subagent permission boundaries. Read-only and review-style
  agents now default-deny undeclared tools, and all built-ins deny recursive
  `task` / `parallel_task` delegation to prevent unbounded nesting.
- Collapsed independent delegated plan waves into a single `parallel_task`
  operation where possible, rather than launching serial sibling `task` calls.
- Updated Node and Python SDK documentation to prefer the single delegation
  surface backed by the core `task` and `parallel_task` tools.

### Fixed

- Fixed release helper version checks so they no longer reference the removed
  `cli/` package.
- Fixed explicit subagent trigger parsing so normal phrases such as "use the
  plan" are not mistaken for a `plan` subagent request.

## [3.0.0] - 2026-05-20

### Added (Phase 8 — typed-error SDK alignment)

- New public `ToolErrorKind` enum (`#[non_exhaustive]`, JSON-tagged on
  the `type` discriminator) carries structured tool failure reasons
  from the Rust core all the way to SDK callers without losing the
  type. Six variants: `version_conflict`, `remote_git_conflict`,
  `not_found`, `invalid_argument`, `unsupported`, `timeout`.
- New optional `error_kind` field on `ToolOutput`, `ToolResult`, and
  `ToolCallResult`, plus a matching field on `AgentEvent::ToolEnd` for
  streaming consumers.
- Built-in `edit` and `patch` tools populate `error_kind` via
  `ToolErrorKind::from_workspace_error` whenever a `WorkspaceError`
  variant maps to a typed kind. The human-readable `output` /
  `content` message is unchanged so the model still gets the retry
  hint; SDK callers now have a programmatic discriminator next to it.
- Node SDK: new `errorKindJson` field on `ToolResult` and `AgentEvent`
  (JSON-encoded `ToolErrorKind`) plus a new `ToolErrorKind` TypeScript
  discriminated-union type in `index.d.ts`.
- Python SDK: new `error_kind_json` (raw) and `error_kind` (parsed
  dict) properties on `ToolResult` and `AgentEvent`.

This closes the v3.0 typed-error gap: until this commit the typed
`WorkspaceError` enum on the Rust trait surface was effectively
re-stringified at the SDK boundary, forcing JS/Python callers to
regex-match the output to detect e.g. concurrent-modification
conflicts. They now `switch` / `match` on `error_kind.type` instead.

### ⚠️ Breaking changes (3.0.0)

- **`WorkspaceFileSystem` and `WorkspaceFileSystemExt` trait methods now
  return `WorkspaceResult<T>` instead of `anyhow::Result<T>`.** The new
  result type wraps the typed `WorkspaceError` enum
  (`#[non_exhaustive]`) with structured variants for `NotFound`,
  `VersionConflict`, `RemoteGitConflict`, `InvalidArgument`, `Timeout`,
  `Unsupported`, and a `Backend(anyhow::Error)` catch-all. Callers that
  used `?` to lift errors into `anyhow::Result` keep working unchanged
  thanks to the blanket `From<WorkspaceError> for anyhow::Error` impl;
  callers that previously did `err.downcast_ref::<WorkspaceVersionConflict>()`
  now `match` on the typed variant directly:
  ```rust
  // before:
  if e.downcast_ref::<WorkspaceVersionConflict>().is_some() { ... }
  // after:
  if matches!(e, WorkspaceError::VersionConflict(_)) { ... }
  ```
  `WorkspaceServices::read_for_edit`, `write_for_edit`, and the generic
  `run_with_timeout` (now polymorphic in the error type) follow the
  same shape. The other 5 traits (`WorkspaceCommandRunner`,
  `WorkspaceSearch`, `WorkspaceGit`, `WorkspaceGitStashProvider`,
  `WorkspaceGitWorktreeProvider`) **still return `anyhow::Result`** —
  their migration to `WorkspaceResult` will be additive (non-breaking)
  in a future v3.x release.

### Added

- Added `S3WorkspaceBackend` — an S3-compatible workspace backend that lets
  built-in file tools (`read`, `write`, `edit`, `patch`, `ls`) operate
  directly against any S3-compatible endpoint (AWS S3, MinIO, RustFS, R2,
  Backblaze B2, ...). Gated behind the new `s3` Cargo feature.
- Added `S3BackendConfig` builder for configuring endpoint, region, static
  or session-token credentials, force-path-style, request timeout, and
  bucket prefix.
- Added `WorkspaceServices::s3()` factory and `WorkspaceServices::from_s3_backend()`
  helper. The factory installs a 60s default per-operation timeout and
  declines `bash`, `git`, `grep`, and `glob` capabilities — capability
  gating automatically hides those tools from the model so it cannot
  call operations the backend cannot service.
- Exposed `S3WorkspaceBackend` in the Node and Python SDKs alongside
  `LocalWorkspaceBackend`. Configuration uses the same option surface
  (`workspaceBackend` / `workspace_backend`).
- `S3WorkspaceBackend::read_text` now enforces a configurable size ceiling
  (`S3BackendConfig::max_read_bytes`, default 10 MiB) by inspecting
  `Content-Length` on the `GetObject` response before consuming the body.
  Oversized objects are rejected with a clear error and never buffered
  into memory. Responses without a `Content-Length` header are refused
  rather than risking OOM.
- Added optional `WorkspaceFileSystemExt` trait for backends that expose
  compare-and-swap writes, plus a `WorkspaceVersionConflict` error type.
  `S3WorkspaceBackend` implements it via ETag + `If-Match` on `PutObject`.
  The `edit` and `patch` tools now capture the ETag during the read and
  reject the write on version mismatch (HTTP 412), surfacing a typed
  "Concurrent modification detected" error so the model can re-read and
  retry instead of silently clobbering a concurrent writer.
  `WorkspaceServices::read_for_edit` and `write_for_edit` are the new
  helpers tools should use for any read-modify-write cycle; backends
  without versioning (e.g. local) transparently fall through to plain
  `read_text` / `write_text`.
- `S3WorkspaceBackend` now implements `WorkspaceSearch` (degraded `grep` /
  `glob` via `LIST` + `GET` + regex). Off by default; opt in via
  `S3BackendConfig::enable_search(true)`. Hard ceilings on objects scanned
  per call (`max_objects_scanned`, default 500) and per-object body size
  for `grep` (`max_grep_bytes_per_object`, default 1 MiB) bound the API
  cost. Hitting either ceiling sets `WorkspaceGrepResult::truncated = true`.
  Glob patterns follow the local backend's recursion convention: `*.rs`
  matches the immediate level, `**/*.rs` recurses.
- `S3WorkspaceBackend::grep` now downloads candidate objects in parallel
  via `futures::stream::buffer_unordered`. Concurrency defaults to 8 and
  is configurable via `S3BackendConfig::search_concurrency` (also
  exposed on both SDKs). Output ordering remains deterministic — results
  are sorted by workspace path before assembly — so callers see the same
  layout regardless of S3 response timing.

### Added

- Internal `workspace::conformance` module (test-only) codifies the
  behavioural invariants every backend implementing
  `WorkspaceFileSystem` (and optionally `WorkspaceFileSystemExt`) must
  satisfy. Two public entry points, `assert_filesystem_conformance` and
  `assert_filesystem_ext_conformance`, are run against
  `LocalWorkspaceBackend` and a new `InMemoryFileSystem` reference
  backend so the contract is exercised both over real I/O and an ideal
  HashMap-backed implementation. Future backends (GCS, container,
  browser) gain a regression suite for free — when the conformance set
  grows after a production incident, every backend running it picks up
  the new test automatically.

### Fixed

- `WorkspaceServices::with_remote_git` previously rebuilt the services
  through `WorkspaceServicesBuilder`, which silently dropped `local_root`
  (and would silently drop any future field). The decorator now goes
  through a new internal `with_git_provider` helper that uses an explicit
  struct literal — adding a new field to `WorkspaceServices` now triggers
  a compile error in every decorator, forcing a deliberate decision.
- `RemoteGitBackend::diff` previously deserialised the entire response
  body before applying `max_diff_bytes`, so a misbehaving gitserver
  returning a multi-gigabyte JSON could exhaust client memory. The diff
  path now streams the body with a hard cap (`max_diff_bytes * 4`, floor
  64 KiB), rejecting requests upfront when `Content-Length` advertises an
  oversized body and aborting the stream mid-flight when chunked encoding
  hides the size. The soft `max_diff_bytes` display truncation is
  unchanged.

### Changed

- `S3WorkspaceBackend::list_dir` now errors with "S3 path not found" when
  the LIST returns zero entries on a non-root path, matching the local
  backend's behaviour. Previously a missing prefix silently returned
  `Ok(vec![])`, masking typos. Paths that exist only as S3 zero-byte
  directory markers still return `Ok(vec![])`.
- Every S3 API call (`GET`, `PUT`, `LIST`) on `S3WorkspaceBackend` now
  emits a structured `tracing::debug!` event with fields `op`, `bucket`,
  `target`, `bytes`, `outcome`, `duration_ms`. Hosts can meter S3 cost
  by subscribing to these events without the backend taking a dependency
  on any metrics framework.
- Node and Python SDKs now expose the workspace hardening options added
  in this release. The Node `JsS3BackendConfig` and Python
  `S3WorkspaceBackend` constructor accept `maxReadBytes` /
  `max_read_bytes`, `searchEnabled` / `search_enabled`,
  `maxObjectsScanned` / `max_objects_scanned`, and
  `maxGrepBytesPerObject` / `max_grep_bytes_per_object`. A new
  `RemoteGitBackendConfig` class (Python) / `JsRemoteGitBackendConfig`
  shape (Node) and a top-level `remoteGit` / `remote_git` session
  option let SDK callers attach `RemoteGitBackend` on top of any
  workspace backend. Passing `remoteGit` without `workspaceBackend`
  raises a clear error.
- Added `RemoteGitBackend` — an HTTP/JSON `WorkspaceGit` client that
  brings the `git` tool to non-local workspaces (S3 today; future
  container / DFS). Implements `WorkspaceGit` in full and
  `WorkspaceGitStashProvider`; deliberately omits `WorkspaceGitWorktreeProvider`
  because worktrees do not map to a remote service. The protocol is
  specified in `apps/docs/content/docs/en/code/rfcs/workspace-remote-git.mdx`.
  - New types: `RemoteGitBackend`, `RemoteGitBackendConfig`,
    `RemoteGitConflict` (anyhow-downcastable for recoverable 409 / 422
    responses such as `WORKING_TREE_DIRTY` and `BRANCH_EXISTS`).
  - New factory: `WorkspaceServices::with_remote_git(config)` on any
    existing `Arc<WorkspaceServices>` to attach remote git on top of an
    S3 (or local) filesystem backend.
  - Client-side ceilings: `request_timeout` (default 30 s),
    `max_log_entries` (default 200), `max_diff_bytes` (default 1 MiB).
  - Per-call `tracing::debug!` event with fields `op`, `repo_id`,
    `status`, `bytes`, `outcome`, `duration_ms`, mirroring the S3
    metering shape so a single subscriber meters both.
  - Authentication: bearer token (header `Authorization: Bearer <token>`)
    or mTLS via `client_cert_pem` + `client_key_pem` (PKCS#8 PEM key for
    the `rustls-tls` backend). Setting only one of the mTLS pair fails
    at construction.

### Changed

- Restructured `core/src/workspace.rs` into a `workspace/` module with
  `workspace/mod.rs` (abstract traits + `WorkspaceServices`),
  `workspace/local.rs` (`LocalWorkspaceBackend`), and `workspace/s3.rs`
  (`S3WorkspaceBackend`). No behavioural change for existing callers.

## [2.6.0] - 2026-05-18

### Added

- Added `WorkspaceServices` capability abstraction (`core/src/workspace.rs`)
  that lets the host supply file system, command runner, search, and Git
  providers behind the stable built-in tool contract. The default
  `LocalWorkspaceBackend` preserves existing local-filesystem behavior, while
  DFS, browser, container, and remote backends can be assembled via
  `WorkspaceServicesBuilder`.
- Added `SessionOptions::with_workspace_backend()` (alias
  `with_workspace_services`) so callers can opt-in to non-local workspaces
  without changing tool schemas.
- Added capability-driven tool gating: `bash`, `grep`, `glob`, and `git` are
  only registered when the workspace backend declares the matching capability,
  preventing models from invoking tools the backend cannot service.
- Added `Session::write_file`, `Session::ls`, `Session::edit_file`, and
  `Session::patch_file` direct-tool APIs in core, Node, and Python SDKs,
  alongside the existing `read_file` / `bash` / `glob` / `grep`.
- Added `LocalWorkspaceBackend` class to the Node and Python SDKs as the
  explicit typed form of the default backend and the option surface for future
  remote/browser/DFS workspaces.
- Added `workspace_services` to `ChildRunContext` so child runs inherit the
  parent's workspace backend.
- Added 17 unit + integration tests covering virtual path resolution, capability
  downgrade, contract-level tool routing for files / search / bash / git
  through pluggable backends, and session-level direct-tool dispatch.

### Changed

- Refactored built-in tools `read`, `write`, `edit`, `patch`, `ls`, `bash`,
  `grep`, `glob`, and `git` to route operations through `WorkspaceServices`
  instead of hard-coded local filesystem calls. Local behavior is unchanged.
- Centralized workspace-boundary path checks in
  `ToolContext::resolve_workspace_path`, removing duplicated canonicalization
  logic from `ToolExecutor::execute`.

### Documentation

- Updated `README.md`, Node SDK README, and Python SDK README with workspace
  backend usage and the new direct-tool API surface.

## [2.5.0] - 2026-05-12

### Added

- Added `ConfirmationInheritance` enum for controlling how child runs resolve Ask
  decisions: `AutoApprove` (default), `DenyOnAsk`, and `InheritParent`.
- Added `confirmation_inheritance` field to `WorkerAgentSpec` in Node and Python
  SDKs, allowing fine-grained control over child run confirmation behavior.
- Added `ChildRunContext` for explicit parent capability inheritance, ensuring
  child runs properly inherit permission checkers and confirmation policies.
- Added comprehensive integration tests for task delegation with real LLM calls
  and mock LLM contract tests for permission and confirmation inheritance.
- Added SDK integration tests for `confirmation_inheritance` in both Node and
  Python SDKs with `.a3s/config.acl` configuration support.

### Fixed

- Fixed task delegation to properly inherit permission checker from agent
  definition in child runs (Issue #28).
- Fixed child runs to respect parent's confirmation policy when using
  `InheritParent` mode.

### Changed

- Unified `AgentDefinition` → `AgentConfig` conversion via `apply_to()` method
  for consistent configuration application.
- Refactored `ToolExecutor` to remove redundant `guard_policy` field, relying
  on `PermissionChecker` for all permission decisions.

### Documentation

- Updated Node and Python SDK READMEs with `confirmation_inheritance` examples
  and usage guidance.
- Updated English and Chinese documentation for teams and tasks with worker
  agent confirmation inheritance patterns.

## [2.4.0] - 2026-05-11

### Added

- Added `generate_object` built-in tool for structured JSON output with schema
  validation, automatic repair, and streaming partial objects. Works across all
  providers via tool-calling mode.
- Added `llm::structured` module with four output modes (tool, prompt, strict,
  json), robust JSON extraction from dirty LLM output, partial JSON parser for
  streaming, and a built-in JSON Schema validator supporting `anyOf`/`oneOf`,
  nullable types, `additionalProperties`, `pattern`, and numeric ranges.
- Added streaming partial object support: `generate_object` emits
  `tool_output_delta` events with progressively complete JSON snapshots.
- Added comprehensive documentation: structured output example (EN/CN), contract
  review tutorial (EN/CN), and 7 additional core mechanism tutorials (PTC,
  streaming, session persistence, skills, MCP, security/HITL, hooks, memory).

### Fixed

- Fixed Shiki build error in docs site caused by unsupported `acl` language
  identifier in code blocks (replaced with `text`).

## [2.3.0] - 2026-05-09

### Added

- Added compact, object-shaped SDK APIs for long-lived integrations:
  `send(...)`, `run(...)`, `stream(...)`, `task(...)`, `tasks(...)`,
  `git(...)`, `addMcp(...)` / `add_mcp(...)`, `removeMcp(...)` /
  `remove_mcp(...)`, and `mcps()`.
- Added live run/tool observability through active tool snapshots and richer
  run replay APIs across Rust, Node.js, and Python SDKs.
- Added a durable SDK API design contract under `manual/SDK_API_DESIGN.md`.
- Added Python SDK parity for worker agents, HITL confirmation policy/control,
  session-for-worker, live worker registration, and session close.

### Changed

- Split the large agent and session API implementation files into focused
  runtime modules for maintainability.
- Updated docs and examples to prefer short SDK method names while retaining
  long compatibility aliases.
- Re-exported `ActiveToolSnapshot` from the Rust core crate root.

### Removed

- Removed the obsolete sidecar/copilot/BTW/strategize/BTE mechanism and related
  prompts, docs, configs, and examples. Background advice, context supplements,
  and PTC proposals now belong to the caller or an external harness.

---

## [2.0.0] - 2026-05-02

### Changed

- Promoted A3S Code package metadata to `2.0.0` across Rust core, Node.js SDK, and Python SDK.
- Standardized runtime configuration on ACL (`.acl`) and explicit `env(...)` credential injection.
- Reworked the public API surface around `Agent`, `AgentSession`, and 2.0-compatible session/control-plane primitives.

### Added

- Release-blocking real-provider integration test for `.a3s/config.acl` environment-variable injection.
- No-network integration coverage, script dry-run support, and literal-config extraction for MiniMax ACL `env(...)` resolution.
- Release validation scripts for local core tests, version consistency, patch hygiene, and real-provider ACL smoke tests.

### Removed

- Legacy HCL config artifacts and stale prompt tests that no longer match the 2.0 ACL runtime.

---

## [v1.8.6] - 2026-04-10

### Fixed

#### web_search Tool

- **Issue #25 Fix**: The `web_search` tool now returns an error when unknown parameters are passed (e.g., `engine` instead of `engines`). Previously, unknown parameters were silently ignored, causing confusion when users specified the wrong field name.

### Changed

- `engines` parameter type changed from `string` to `array` in schema to match actual API
- Updated a3s-search integration to v1.0.0

---

## [v1.8.5] - 2026-04-05

### Added

#### Git Built-in Tool

- **Built-in Git Client**: New `git` tool with auto-install support for Windows, macOS, and Linux. Downloads official pre-built git binaries to `~/.local/git/bin/` when git is not available - no package manager required.

  Full git operations: `status`, `log`, `branch`, `checkout`, `diff`, `stash`, `remote`, `worktree`

- **Git Convenience Methods**: Python SDK (`session.git(...)`) and Node SDK (`session.git(...)`) convenience methods for git operations.

#### System Prompt Updates

- Updated all system prompts to reference "A3S Code" instead of "Claude Code"
- Updated skill references to use `a3s-lab/code-skills`

### Removed

- **Document Parser**: Removed `composite_document_parser` and `document` modules and all related code. This feature was not fully implemented and has been removed to simplify the codebase.

- **Agentic Search/Parse Tools**: Removed `agentic_search` and `agentic_parse` built-in tools.

- **Git Worktree Tool**: Replaced by the new unified `git` tool with `worktree` subcommand.

### Changed

- **Tool Count**: Updated built-in tool count from 15 to 16 to reflect new git and box tools.
- **Documentation**: Updated all documentation to reflect new tool names and capabilities.

---

## [v1.6.0] - 2026-04-02

### Added

#### Document Parsing

- **XLSB (Excel Binary) Support**: Added calamine-based BIFF12 parsing for XLSB files with proper cell value extraction, supporting Float, Int, Bool, DateTime, DateTimeIso, and DurationIso types. Significantly improves table fidelity for .xlsb files.

- **HWPX Table Extraction**: Added structured table extraction from Korean HWPX documents. Parses `tbl/tr/tc` XML hierarchy and includes `structured_payload` for `tables[]` output.

#### Search Ranking

- **Tabular Query Intent Detection**: Automatically detects when queries relate to tables (keywords: table, column, row, spreadsheet, excel, csv, cell, data, record, etc.) and boosts table line matches by +10 keyword hits plus 1.3x relevance multiplier.

- **Heading Inheritance Boost**: When search matches appear under headings that also match the query, those matches receive a relevance boost (up to 1.3x). Looks backwards to find the closest preceding heading.

#### Dependencies

- Added `calamine = "0.26"` for XLSB parsing

### Fixed

- Test assertion: `paged_text_blocks_reflow_two_column_preserves_paragraph_breaks` - Corrected expected string "Parser metadata now tracks OCR" vs "Parser metadata now tracks OCR backend"

---

## [v1.5.8] - 2026-03-07

### Added

- Phase 1 structured result surfaces:
  - `structured_payload` exposed in `agentic_parse` output and metadata
  - Table payloads in stable machine-readable form
  - Page-level data in `agentic_parse` output and metadata
  - Stable `tables[]`, `pages[]`, `elements[]` outputs

- Phase 2 PDF extraction improvements:
  - lopdf position-aware text extraction
  - Reduced dependence on weak text fallbacks
  - Position-aware table detection

- `agentic_search` enhancements:
  - Chunk context consumption
  - Tabular content consumption
  - Page numbers and locators support

### Changed

- `ParsedDocument` extended with `tables: Vec<StructuredTable>` and `pages: Vec<PageInfo>`

### Fixed

- Windows shell compatibility improvements

---

## [v1.5.7] - 2026-02-28

### Added

- Runtime session header support for OpenAI configs
- Cross-platform environment variable expansion in tests

---

## [v1.5.6] - 2026-02-20

### Added

- Enhanced agent config, document parser, LLM, tools, and SDKs
- Host shell environment propagation to tool commands

---

## [v1.5.5] - 2026-02-10

### Added

- Zhipu AI client (`ZhipuClient` formerly `GlmClient`)
- Duplicate tool call circuit breaker
- Streaming fallback support
- `agentic_parse` skill

---

## [v1.5.4] - 2026-01-28

### Added

- Session-local skill registries

---

## [v1.5.3] - 2026-01-15

### Added

- Tool schema hardening
- Slash command output restoration
