# A3S Code Capability Verification and Performance Contract

## Purpose

This document is the evidence ledger for every product area advertised in the
README capability map. It separates implemented code, compiled code, exercised
runtime behavior, external qualification, and measured performance. A feature
is not considered verified merely because its source exists or its crate
compiles.

The ledger has three jobs:

1. keep every public capability connected to executable evidence;
2. expose weak or missing evidence instead of hiding it behind a green aggregate
   test run;
3. define performance checks that are useful without turning noisy wall-clock
   samples into unreliable correctness gates.

## First-principles verification model

Each capability is reviewed through the following independent questions.

| Dimension       | Required evidence                                                                                                                                                      |
| --------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Public contract | A public API, tool schema, feature flag, configuration field, or documented host extension boundary identifies what callers may rely on.                               |
| Activation      | Tests prove the capability appears when enabled and remains absent or inert when disabled.                                                                             |
| Correctness     | A deterministic oracle checks successful behavior without asking the implementation to grade itself.                                                                   |
| Failure safety  | Invalid input, unavailable dependencies, permission denial, timeout, cancellation, malformed provider output, and stale state fail in the documented way.              |
| Lifecycle       | Creation, replacement, persistence, replay, cancellation, and close release owned tasks and resources without crossing session boundaries.                             |
| Concurrency     | Ordering, single-flight rules, parallel limits, backpressure, and race outcomes are observable and deterministic.                                                      |
| Resources       | Memory, records, bytes, queue depth, tool rounds, retries, output size, and provider amplification have explicit ceilings where growth is caller-controlled.           |
| Performance     | Stable local work uses warmups, multiple samples, percentile reporting, build-profile checks, and machine metadata. Network and model latency are reported separately. |
| SDK parity      | Every claimed language surface is constructed and exercised through its published wrapper, not only through Rust Core.                                                 |
| Documentation   | Activation, non-goals, failure modes, resource limits, and qualification commands are discoverable from public documentation.                                          |

An aggregate suite proves only the cases it actually executes. Ignored tests,
credentialed tests, browser tests, and object-store tests remain separate
qualification evidence and must not be counted as ordinary hermetic CI.

## Evidence levels

| Level                  | Meaning                                                                                                                  |
| ---------------------- | ------------------------------------------------------------------------------------------------------------------------ |
| Required CI            | Runs for every relevant pull request or push and blocks on failure.                                                      |
| Targeted CI            | Runs when paths for the capability change, or in a dedicated reusable workflow.                                          |
| Release qualification  | Runs with a release build, locked fixture, bounded resource profile, and machine-readable output.                        |
| External qualification | Requires a real provider, browser, object store, login, or other environment that hermetic CI cannot truthfully emulate. |
| Compile-only           | Proves type compatibility, but not runtime behavior. This is never sufficient by itself.                                 |
| Gap                    | The advertised behavior lacks one or more required evidence dimensions.                                                  |

## Scoped capability foundation evidence

The [Scoped Capability Architecture](SCOPED_CAPABILITY_ARCHITECTURE.md) is the
normative ownership and migration contract for the capability lifecycle
program. `scripts/check_scoped_capability_architecture.py` keeps its ordered
gates, states, ownership table, invariant identifiers, local evidence links,
and the corresponding Roadmap table aligned in CI.

`CAP-FND1` relies on existing executable behavior rather than duplicating it in
documentation-only tests:

| Baseline | Deterministic evidence | Preserved guarantee |
| --- | --- | --- |
| Built-in and dynamic Tool ownership | [`ToolRegistry` tests](../core/src/tools/registry/tests.rs) | A dynamic source cannot replace or remove a built-in Tool |
| Session-local Tool and Skill replacement | [`session_extensions`](../core/src/agent_api/session_extensions.rs) and [`live_skill_lifecycle`](../core/tests/live_skill_lifecycle.rs) | Removal affects only the exact registration still owned by that source |
| Run admission lease | [`run_admission`](../core/src/agent_api/run_admission.rs) | Overlapping transcript operations fail fast and lease release is RAII-safe |
| Run capability evidence | [`harness_evidence` tests](../core/src/harness_evidence/tests.rs) | Capability digests are stable for identical input and change on visible surface or readiness drift |
| Bounded close | [`session close integration`](../core/tests/test_session_close_lifecycle.rs) | Close establishes an admission boundary, propagates cancellation, drains accepted work, and is idempotent |

These tests characterize required safety while later gates replace mutable
registries and pointer shadow chains. A later gate may change the mechanism but
must update or strengthen the same behavioral evidence rather than deleting it.

`CAP-SET1` adds executable evidence for the first new Core kernel slice:

| Delivered behavior | Deterministic evidence | Bound or failure rule |
| --- | --- | --- |
| Typed upstream and local identity | [`capability/id.rs`](../core/src/capability/id.rs) and [`capability_set`](../core/tests/capability_set.rs) | Invalid identifiers, digests, package generations, and unsafe segments fail construction |
| Complete source ownership | [`capability/descriptor.rs`](../core/src/capability/descriptor.rs) | Empty batches, mismatched owners, duplicate identities, self-dependencies, and duplicate edges fail closed |
| Immutable canonical set | [`capability/set.rs`](../core/src/capability/set.rs) | Sources, capabilities, dependency edges, per-capability dependencies, and canonical encoded bytes have explicit ceilings |
| Exact Use projection | [`mixed and empty projection tests`](../core/tests/capability_set.rs) | Package contributions from different capability or Registry revisions cannot share one Code set; an empty product projection still retains its upstream cursor identity |
| Lock-free frozen readers | [`Arc pinning and golden digest tests`](../core/tests/capability_set.rs) | Construction returns `Arc<CapabilitySet>`; insertion order cannot alter iteration or `a3s.code.capability-set.v1` identity |

`CAP-SCOPE1` and `HARNESS-SCOPE1` add deterministic lifecycle evidence:

| Delivered behavior | Deterministic evidence | Bound or failure rule |
| --- | --- | --- |
| Typed lifetime and kind | [`CapabilityLease` compile-fail examples](../core/src/capability/lease.rs) | A borrowed lease cannot escape its owner, and a Turn lease cannot enter a Run-only API |
| Monotonic child authority | [`child ceiling tests`](../core/tests/capability_scope.rs) | Capability, workspace, governance, and execution expansion each fail before child publication |
| Exact Use Run admission | [`Use lease tests`](../core/tests/capability_scope.rs) | Missing, unexpected, or cursor-mismatched leases fail; the accepted non-clone lease is released only after effects and descendants |
| Structured teardown | [`scope supervisor tests`](../core/tests/capability_scope.rs) | Tasks settle first, children and effects close in reverse order, and one shared deadline bounds ignored cancellation |
| Cancellation-safe ownership | [`waiter and parent-drop tests`](../core/tests/capability_scope.rs) | Cancelling a close waiter does not cancel the owned close driver; parent `Drop` synchronously aborts descendant futures without spawning cleanup |

`CAP-PROJ1` adds deterministic publication and runtime-identity evidence:

| Delivered behavior | Deterministic evidence | Bound or failure rule |
| --- | --- | --- |
| Closed typed values | [`projection value validation`](../core/tests/capability_projection.rs) | Missing, extra, kind-mismatched, public-name-mismatched, and unsupported UI values fail before an immutable projection can escape |
| Typestate publication | [`CapabilityTxn` compile-fail example](../core/src/capability/transaction.rs) | Only `CapabilityTxn<Validated>` exposes `commit`; prepared values cannot bypass complete validation |
| Failure rollback | [`prepare, cancellation, validation, and dropped-transaction tests`](../core/tests/capability_projection.rs) | Every completed effect transfers to reverse cleanup while the visible generation remains unchanged |
| Atomic commit race | [`catalog CAS race`](../core/tests/capability_projection.rs) | Generation and digest are compared under one short writer lock; exactly one competing complete generation becomes visible |
| Exact reader identity | [`old/new lease identity test`](../core/tests/capability_projection.rs) | A non-clone reader resolves definition and execution through one borrowed value; old effects retire only after the final old lease drops |
| A3S Use boundary | [`CapabilityProjection` retains `CapabilitySet`](../core/src/capability/projection.rs) | Code consumes the complete Use cursor but performs no package resolution, Grants, Use cutover, or receipt retirement; Run admission still requires the real Use lease |

`CAP-DEP1` adds deterministic surface-readiness evidence:

| Delivered behavior | Deterministic evidence | Bound or failure rule |
| --- | --- | --- |
| Canonical readiness waves | [`diamond and insertion-order tests`](../core/tests/capability_readiness.rs) | Iterative Kahn traversal over ordered maps produces minimal waves and one stable activation order bound to the set generation and digest |
| Fail-closed graph admission | [`cycle and staged-completeness tests`](../core/tests/capability_readiness.rs) | Cycles and missing adapters fail before any adapter starts or a runtime projection can escape |
| Dependency-aware preparation | [`ordering and prerequisite-failure tests`](../core/tests/capability_readiness.rs) | Only already published surface edges order adapters; a failed prerequisite blocks dependents and completed effects roll back in reverse order |
| Explicit graph bounds | [`maximum-width and maximum-depth tests`](../core/tests/capability_readiness.rs) | Planning accepts at most 4,096 capabilities, 32,768 edges, 128 direct dependencies, and 4,096 iterative waves without recursion |
| A3S Use authority | [`cross-package cursor test`](../core/tests/capability_readiness.rs) | Cross-package surface edges retain one exact Use cursor; Code never reads manifests or resolves, installs, activates, or retires packages |

The in-progress `HOST-CAP1` gate now has a deterministic Core Tool/Skill host
slice. The official CLI and Desktop migrations remain required before the gate
can be marked delivered.

| Core host behavior | Deterministic evidence | Bound or failure rule |
| --- | --- | --- |
| Complete Session batch | [`capability_runtime_tests`](../core/src/agent_api/capability_runtime_tests.rs) | Every target value prepares before one generation/digest CAS; failure or cancellation leaves the visible stamp unchanged and closes prepared effects |
| Real Use lease per Run | [`exact lease and cursor mismatch tests`](../core/src/agent_api/capability_runtime_tests.rs) | The published provider is generation-specific, every Run acquires again, and Code rejects a returned generation, capability revision, or Registry revision mismatch |
| Tool N/N+1 isolation | [`definition/executor cutover test`](../core/src/agent_api/capability_runtime_tests.rs) | An admitted N Run keeps one Tool `Arc` for definition and execution plus N's Use lease while N+1 becomes visible only to a later Run |
| Skill discovery N/N+1 isolation | [`Skill search cutover test`](../core/src/agent_api/capability_runtime_tests.rs) | `search_skills` resolves the Run-frozen Skill registry while N+1 is published, rather than consulting the Session-latest map |
| Compatibility conflict boundary | [`pre-commit and post-publication conflict tests`](../core/src/agent_api/capability_runtime_tests.rs) | Tool/Skill conflicts fail before generation advance, and compatibility Tool, Skill, or MCP-wrapper mutation cannot shadow a published projection |
| Close and cancellation linearization | [`prepare cancellation, close/commit race, and exact-Run admission tests`](../core/src/agent_api/capability_runtime_tests.rs) | Close and commit have one order, prepared effects cannot become orphaned, exact Run reservations settle on admission failure, and non-clean scope teardown becomes a typed Run failure |
| Explicit migration boundary | [`SessionCapabilityBatch`](../core/src/capability/runtime.rs) | This cut accepts only Tool and Skill values; every other capability kind fails closed until its product-owned adapter is migrated |

## Capability evidence ledger

The area names and order intentionally match the README capability map. The
repository check in `scripts/check_capability_verification.py` fails if either
list changes without the other.

| Area                    | Required deterministic evidence                                                                                                                                                                                                                                                                                                                                                                    | Additional qualification                                                                                                                                                                                                                                 | Performance and resource evidence                                                                                                                                                                                                                                                                                              | Current evidence state                                                   |
| ----------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | ------------------------------------------------------------------------ |
| Agent runtime           | [`agent_api` tests](../core/src/agent_api/tests.rs), [`run_lifecycle`](../core/src/agent_api/run_lifecycle.rs), and [`test_session_close_lifecycle`](../core/tests/test_session_close_lifecycle.rs) cover construction, send/stream admission, resume, replace, cancel, close, and replay.                                                                                                         | [`agent_convergence_benchmark`](../core/examples/agent_convergence_benchmark.rs) executes scripted end-to-end convergence and checkpoint recovery.                                                                                                       | Tool rounds, continuation turns, provider calls, token accounting, close deadlines, and single-flight admission are bounded. The convergence report records latency but gates deterministic work amplification rather than noisy absolute milliseconds.                                                                        | Required CI plus targeted release qualification.                         |
| Governed tools          | [`tools` tests](../core/src/tools/tests.rs), [`governed_tests`](../core/src/agent_api/direct_tools/governed_tests.rs), and [`prompt boundary tests`](../core/tests/test_prompt_boundaries_and_log_redaction.rs) cover schemas, capabilities, permission/HITL routing, hooks, sanitization, cancellation, and output transformation.                                                                | File, shell, Git, web, program, batch, delegation, MCP, and Skill integrations have concern-specific tests under [`core/src/tools`](../core/src/tools).                                                                                                  | Tool timeouts, output limits, process groups, recursion depth, batch parallelism, and retained artifacts have explicit ceilings.                                                                                                                                                                                               | Required CI.                                                             |
| Code intelligence       | [`language runtime integration tests`](../core/src/code_intelligence/language_runtime/integration_tests.rs), [`workspace runtime integration tests`](../core/src/code_intelligence/workspace_runtime/integration_tests.rs), and [`tool tests`](../core/src/tools/builtin/code_intelligence/tests.rs) cover revisions, symbols, navigation, diagnostics, stale state, initialization, and shutdown. | The fake LSP fixture in [`code_intelligence_fake_lsp.rs`](../core/tests/fixtures/code_intelligence_fake_lsp.rs) exercises the process protocol without requiring an installed language server.                                                           | [`code_intelligence_benchmark`](../core/examples/code_intelligence_benchmark.rs) gates a 5,000-file manifest, cold process start/source read, warm document/workspace symbol p95, protocol shutdown, process cleanup, and active/retained RSS. Results are retained in the [performance record](PERFORMANCE_QUALIFICATION.md). | Required correctness CI and release qualification.                       |
| Workspace retrieval     | [`retrieval tests`](../core/src/workspace/retrieval/tests.rs), [`retrieval QA tests`](../core/src/agent_api/retrieval_qa_tests.rs), and [`workspace backend tests`](../core/tests/test_workspace_backend.rs) cover lexical, semantic, hybrid, chunking, batching, generation fencing, egress, source verification, isolation, and close.                                                           | [`Workspace Retrieval QA`](WORKSPACE_RETRIEVAL_QA.md), [`DeepSeek evaluation`](WORKSPACE_RETRIEVAL_DEEPSEEK_EVAL.md), and the cross-SDK fixture provide release and real-model evidence.                                                                 | [`workspace_retrieval_benchmark`](../core/examples/workspace_retrieval_benchmark.rs) gates exact and hybrid p95, request amplification, vector bytes, scratch bytes, candidates, fallbacks, and release on close. The cross-platform churn workflow checks repeated replacement without accumulation.                          | Required CI, targeted CI, and release qualification.                     |
| Context and memory      | [`context` tests](../core/src/context), [`memory` tests](../core/src/memory.rs), and [`memory extraction runtime tests`](../core/src/agent/memory_extraction_runtime/tests.rs) cover ranked assembly, compaction, recall, extraction, relations, pruning, and provider cancellation.                                                                                                               | Real context-tool behavior is isolated in [`test_context_tools_real_llm`](../core/tests/test_context_tools_real_llm.rs).                                                                                                                                 | [`context_memory_benchmark`](../core/examples/context_memory_benchmark.rs) gates ranked/deduplicated assembly over 25,000 inputs, recall over 2,500 memories, output budgets, correctness, and active/retained RSS. Results are retained in the [performance record](PERFORMANCE_QUALIFICATION.md).                            | Required correctness CI and release qualification.                       |
| Cognitive packages      | [`cognitive_context` tests](../core/src/cognitive_context.rs) cover exact generation binding, citation/source verification, restart checks, and fail-closed provider behavior.                                                                                                                                                                                                                     | Host package resolution remains outside Code by design and must be qualified by the embedding host.                                                                                                                                                      | Source count, bytes, generation identity, and verification work are bounded. Package installation latency is explicitly outside the Code runtime claim.                                                                                                                                                                        | Required CI for the Code boundary; external host qualification required. |
| Model adapters          | [`LLM tests`](../core/src/llm/tests.rs), [`OpenAI tests`](../core/src/llm/openai/tests.rs), [`Anthropic adapter`](../core/src/llm/anthropic.rs), and [`HTTP admission tests`](../core/src/llm/admission.rs) cover normalization, streaming, tool calls, errors, retries, and redaction.                                                                                                            | [`test_deepseek_adversarial_e2e`](../core/tests/test_deepseek_adversarial_e2e.rs), Codex-login tests, and real-config scripts qualify live transport separately.                                                                                         | Request timeouts, retry limits, response limits, token usage, and cancellation are bounded. Remote latency and cost are reported, not used as deterministic Core speed gates.                                                                                                                                                  | Required protocol CI plus external qualification.                        |
| Structured output       | [`structured tests`](../core/src/llm/structured_tests.rs) and [`generate_object contract tests`](../core/src/tools/builtin/generate_object_contract_tests.rs) cover native schemas, prompt fallback, partial parse, repair, and validation.                                                                                                                                                        | [`test_structured_json_real_llm`](../core/tests/test_structured_json_real_llm.rs) and Codex-login variants exercise real providers.                                                                                                                      | Schema size, repair attempts, output size, and provider rounds are bounded.                                                                                                                                                                                                                                                    | Required CI plus external qualification.                                 |
| MCP and Skills          | [`MCP manager tests`](../core/src/mcp/manager/tests.rs), [`MCP protocol tests`](../core/src/mcp/protocol/tests.rs), [`Skill registry tests`](../core/src/skills/registry/tests.rs), and [`live_skill_lifecycle`](../core/tests/live_skill_lifecycle.rs) cover discovery, transport, isolation, registration, removal, inheritance, and validation.                                                 | Stdio, HTTP/SSE, and streamable HTTP transports are exercised with local fixtures; remote servers remain host qualification.                                                                                                                             | Discovery timeouts, reconnects, idle disconnect, payload bounds, Skill size, and invocation limits are explicit.                                                                                                                                                                                                               | Required CI.                                                             |
| Planning and delegation | [`subagent tests`](../core/src/subagent/tests.rs), [`task tool tests`](../core/src/tools/task/tests.rs), [`parallel execution tests`](../core/src/tools/task/parallel_execution.rs), and [`permission inheritance tests`](../core/tests/test_subagent_permissions.rs) cover plans, workers, foreground/background tasks, inheritance, progress, cancellation, and close.                           | Real parallel delegation is isolated in [`test_auto_delegation_real_parallel`](../core/tests/test_auto_delegation_real_parallel.rs).                                                                                                                     | Worker count, depth, steps, parallelism, budgets, and cancellation deadlines are bounded.                                                                                                                                                                                                                                      | Required CI plus external qualification.                                 |
| Priority scheduling     | [`task_scheduler` tests](../core/src/task_scheduler.rs), [`session lane queue` tests](../core/src/session_lane_queue.rs), and [`queue` tests](../core/src/queue.rs) cover priority, FIFO order, aging, occupancy, cancellation, and shutdown.                                                                                                                                                      | SDK runtime suites verify the exported configuration and snapshots.                                                                                                                                                                                      | Capacity, pending counts, aging intervals, admission timeouts, and shutdown are explicit; ordering tests use logical work rather than wall-clock throughput claims.                                                                                                                                                            | Required CI and SDK runtime CI.                                          |
| Programmable workflows  | [`program tests`](../core/src/program/tests.rs), [`program tool tests`](../core/src/tools/program_tool/tests.rs), [`dynamic workflow tests`](../core/src/dynamic_workflow/tests.rs), and [`QuickJS integration`](../core/tests/test_program_script_quickjs_integration.rs) cover bounded scripts, nested tools, Flow replay, budgets, and cancellation.                                            | [`workflow facade real LLM`](../core/tests/test_workflow_facade_real_llm.rs) is external qualification.                                                                                                                                                  | Script memory, time, calls, output, workflow steps, and shared budgets are bounded. [`flow_graph_benchmark`](../core/examples/flow_graph_benchmark.rs) gates 1,000-step projection p95, deterministic record counts, serialized bytes, and replay.                                                                             | Required correctness CI and release qualification.                       |
| Persistence             | [`store tests`](../core/src/store/tests.rs), [`persisted schema roundtrip`](../core/tests/test_persisted_schema_roundtrip.rs), and [`session persistence`](../core/src/agent_api/session_persistence.rs) cover atomic snapshots, events, traces, artifacts, verification, checkpoints, retention, and migration rejection.                                                                         | Cross-process and recovery behavior is also exercised by the protocol harness and convergence benchmark.                                                                                                                                                 | [`persistence_benchmark`](../core/examples/persistence_benchmark.rs) gates memory and synchronized file-store save/load p95 for a 1–2 MiB aggregate snapshot, repeated overwrite without accumulation, byte fidelity, list cardinality, and delete cleanup.                                                                    | Required correctness CI and release qualification.                       |
| State graph             | [`state graph tests`](../core/src/state_graph/tests.rs) and [`state graph integration`](../core/tests/test_state_graph_integration.rs) cover hash links, objects, relations, optimistic patches, replay, forks, diffs, and Flow lifecycle projection.                                                                                                                                              | Cross-project ontology fixtures in [`r0-cross-project-contract`](../core/tests/r0_cross_project_contract.rs) lock compatibility.                                                                                                                         | [`flow_graph_benchmark`](../core/examples/flow_graph_benchmark.rs) gates replay p95 and verifies that 11,008 records restore the exact object/relation shape within a 64 MiB serialized-event ceiling.                                                                                                                         | Required correctness CI and release qualification.                       |
| Agent release contract  | [`agent_release_manifest`](../core/tests/agent_release_manifest.rs), [`r0 cross-project contract`](../core/tests/r0_cross_project_contract.rs), and [`agent directory convention`](../core/tests/test_agent_dir_convention.rs) cover bounded admission, canonical identity, provenance, compatibility, and secret slots.                                                                           | Release preflight workflows verify version and dependency compatibility before publication.                                                                                                                                                              | Manifest size, file count, canonicalization work, and compatibility checks are bounded.                                                                                                                                                                                                                                        | Required CI and release preflight.                                       |
| Headless Agent protocol | [`agent_protocol_v1`](../core/tests/agent_protocol_v1.rs), [`agent_protocol_harness`](../core/tests/agent_protocol_harness.rs), and [`agent_exact_run_host`](../core/tests/agent_exact_run_host.rs) cover sessions, runs, cancellation, recovery, receipts, event pages, worktrees, and immutable patches.                                                                                         | The CLI integration workflow qualifies service transport around the Core host.                                                                                                                                                                           | Event page size, retention gaps, worktree scope, cancellation, and recovery attempts are bounded.                                                                                                                                                                                                                              | Required CI plus host integration CI.                                    |
| Headless web search     | [`web search tests`](../core/src/tools/builtin/web_search/tests.rs), [`engine tests`](../core/src/tools/builtin/web_search/engines.rs), and [`headless integration tests`](../core/tests/test_web_search_headless.rs) cover engine routing, parsing, fallback, SSRF defenses, admission, and browser lifecycle.                                                                                    | [`Hermetic Integrations`](../.github/workflows/hermetic-integrations.yml) drives installed Chrome through the production CDP path and Google parser against a controlled HTTPS fixture. Public Google/Baidu availability remains external qualification. | Query timeout, result count, request coalescing, circuits, tabs, response bytes, and browser shutdown are bounded. The retained report and request log prove parser output and browser cleanup without including public-engine network latency.                                                                                | Required deterministic and hermetic browser integration CI.              |
| S3 workspace            | [`S3 unit tests`](../core/src/workspace/s3/tests.rs) cover path normalization, capability gating, read/search limits, concurrency, and configuration. [`test_s3_backend`](../core/tests/test_s3_backend.rs) covers live read/write/edit/patch/list behavior.                                                                                                                                       | [`Hermetic Integrations`](../.github/workflows/hermetic-integrations.yml) starts pinned MinIO and executes the live backend test through write, read, list, edit, patch, and cleanup. Actual deployment compatibility remains external qualification.    | Read bytes, objects scanned, bytes per object, search concurrency, and request timeout are bounded. The retained MinIO report requires the complete roundtrip and zero residual test objects.                                                                                                                                  | Required unit and hermetic object-store integration CI.                  |
| Filesystem agent server | [`serve lifecycle tests`](../core/src/serve/lifecycle.rs), [`daemon tests`](../core/src/serve/daemon.rs), [`schedule tests`](../core/src/serve/schedule.rs), and [`agent directory tools`](../core/tests/test_agent_dir_tools.rs) cover readiness, failure state, schedules, tools, cancellation, and joined shutdown.                                                                             | [`test_serve_agent_dir_real_llm`](../core/tests/test_serve_agent_dir_real_llm.rs) qualifies real scheduled turns separately. Node and Python runtime suites exercise the public serve wrappers without credentials.                                      | Startup, schedule, tool, and shutdown deadlines are bounded.                                                                                                                                                                                                                                                                   | Required all-features CI, SDK runtime CI, and external qualification.    |
| OpenTelemetry           | [`telemetry tests`](../core/src/telemetry.rs) and [`OTLP mapping tests`](../core/src/telemetry_otel.rs) cover baseline tracing, optional exporter configuration, propagation, and mapping.                                                                                                                                                                                                         | [`Hermetic Integrations`](../.github/workflows/hermetic-integrations.yml) sends a controlled span to a pinned local Collector and verifies the exact service/span receipt. Remote deployment reachability remains external qualification.                | Export timeout, queue/batch behavior supplied by the configured SDK, and redaction boundaries are documented. The retained report gates initialization, Collector receipt, flush, and shutdown against a 10-second deadline.                                                                                                   | Required all-features and Collector integration CI.                      |

## Runtime surface ledger

| Surface            | Required runtime evidence                                                                                                                                                                                                 | Cross-platform evidence                                                                         | Current gate             |
| ------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------- | ------------------------ |
| Rust Core          | Default workspace tests, all-feature library tests, strict Clippy, semver compatibility, integration tests, and targeted qualification examples.                                                                          | Linux and Windows required CI; retrieval churn also runs on macOS.                              | Required CI.             |
| Node.js            | Build the N-API module, run type tests, construct agents and sessions, execute direct/governed tools, callbacks, stores, workspace retrieval, serve lifecycle, close behavior, and fixture validation through JavaScript. | Publication builds all supported native targets; deterministic runtime CI runs on Linux.        | Required SDK runtime CI. |
| Python             | Build and install the PyO3 extension into an isolated environment, run pytest contracts and executable lifecycle/budget/delegation scripts, validate retrieval fixtures, and close all resources through Python.          | Publication builds the supported CPython/target matrix; deterministic runtime CI runs on Linux. | Required SDK runtime CI. |
| Go                 | Build the versioned bridge, run the Go package with the race detector, and exercise lifecycle, tools, stores, retrieval, streams, runs, verification, and MCP through the bridge protocol.                                | Required runtime CI on Linux and Windows; release assets cover the target matrix.               | Required CI.             |
| Documentation site | Check formatting, lint, language parity, API accuracy, generated tutorial stability, routes, assets, and GitHub Pages output.                                                                                             | GitHub Pages is the publication environment.                                                    | Required Docs workflow.  |

Compilation remains useful as an early failure signal, but no SDK is considered
verified until a host-language process loads the produced native artifact and
executes its public contract.

## Performance policy

### Stable gates

Deterministic performance gates focus on bounded work rather than one noisy
elapsed-time sample:

- provider calls and retry amplification;
- LLM turns, tool attempts, continuation turns, and delegated steps;
- records, candidates, queue depth, batches, files, chunks, and retained events;
- input, output, source, vector, scratch, artifact, snapshot, and context bytes;
- task, process, browser, provider, close, and shutdown deadlines;
- release of all accounted session-owned resources after close.

These gates run in ordinary correctness or targeted CI because their result is
independent of runner speed.

### Wall-clock qualification

Latency claims require all of the following:

1. a release build;
2. a fixed corpus and algorithm configuration;
3. warmup samples before measurement;
4. multiple measured samples with p50, p95, and maximum reported;
5. operating system, architecture, logical CPU count, and relevant processor
   metadata in the result;
6. explicit inclusion rules for provider network time, source reads, cache
   state, and setup work;
7. a machine-readable artifact retained by CI;
8. a generous gate tied to a user-visible requirement rather than the fastest
   observed development machine.

The targeted Performance Qualification workflow runs deterministic agent
convergence plus release profiles for 25,000-record workspace retrieval,
Flow/State Graph, a 5,000-file Code Intelligence workspace, 25,000-item context
assembly and 2,500-item memory recall, and 1–2 MiB session persistence. The
[performance qualification record](PERFORMANCE_QUALIFICATION.md) retains the
latest workload, inclusion, percentile, resource, machine, run, and artifact
digest evidence. External LLM and public search-engine latency is reported
separately because it cannot distinguish a Code regression from provider or
network variance.

### CI duration is not product latency

Job timeouts prevent hung builds and leaked processes. They do not prove API
latency. Build duration, dependency download time, and GitHub runner queue time
must never be presented as runtime performance results.

## Evidence gap closure and external boundaries

As of Code `a9bfac2`, no unresolved Code-owned evidence gap remains in this
ledger. The 2026-08-18 qualification closed the previously recorded gaps with:

1. a 5,000-file Code Intelligence latency, process, shutdown, and RSS profile;
2. a 25,000-input context assembly and 2,500-item memory recall/RSS profile;
3. a 1,000-step Flow projection and 11,008-record State Graph replay profile;
4. a pinned MinIO roundtrip and cleanup gate;
5. a controlled HTTPS fixture through installed Chrome, the production CDP
   path, and the Google parser;
6. a pinned local OpenTelemetry Collector receipt, flush, and shutdown gate;
7. synchronized file and in-memory persistence profiles with repeated
   overwrite and delete cleanup.

The successful [Performance Qualification run
`32130843443`](https://github.com/A3S-Lab/Code/actions/runs/32130843443), [CI run
`32130843684`](https://github.com/A3S-Lab/Code/actions/runs/32130843684), and
their artifact digests are recorded in
[Performance Qualification](PERFORMANCE_QUALIFICATION.md).

Intrinsic external claims remain explicitly scoped: live model behavior,
public search-engine availability, deployment-specific S3/Collector networks,
and cognitive-package host resolution require qualification in the target
environment. They are not Code-owned deterministic gaps. A controlled local
substitute remains required and present for Code-owned parsing, admission,
lifecycle, and resource behavior.

## Repository gates

The normal remote gates are:

```bash
python3 scripts/check_capability_verification.py
cargo fmt --all -- --check
cargo clippy --workspace --lib --bins -- -D warnings
cargo test --workspace
cargo test --workspace --all-features --lib
go -C sdk/go test -race ./...
```

Node and Python runtime commands are executed by their dedicated CI jobs after
the native modules are built. Release performance commands are executed by
`.github/workflows/performance.yml`, which retains their JSON reports as an
artifact. Real-provider and public search-engine qualifications remain
separate; the controlled browser, object-store, and Collector gates run in the
Hermetic Integrations workflow and retain their own logs and JSON reports.

## Completion rule

The repository-wide objective is complete only when every advertised area has:

- required deterministic activation, correctness, failure, and lifecycle
  evidence;
- resource ceilings for caller-controlled growth;
- a suitable performance gate or an explicit statement that performance is
  owned by an external host;
- runtime evidence for every SDK that claims the capability;
- public documentation with activation and non-goals;
- no unresolved gap in the ledger above.

Green compilation, a single real-model demonstration, or a passing subset may
support one row, but cannot close the repository-wide claim.
