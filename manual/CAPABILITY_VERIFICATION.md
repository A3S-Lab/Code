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
| Agent temporal composition | [`Agent Turn tests`](../core/src/agent/tests.rs) | Pre-analysis consumes its own orchestration Turn; each provider response, Tool effect, and Tool stream bridge settles before the next model call |
| Recursive delegated space | [`Skill child scope test`](../core/src/tools/skill.rs) and [`Task scope tests`](../core/src/tools/task/tests.rs) | Foreground Skill/Task Agents compose `Turn -> Subtask -> Turn`; a closed invoking Turn cannot admit foreground work or promote background work |
| Promoted Run work | [`background Task test`](../core/src/tools/task/tests.rs) and [`memory extraction test`](../core/src/agent/tests.rs) | Background Tasks and streaming memory extraction are admitted synchronously, survive normal Turn close, and settle as Run-supervised Subtask/Turn work before lease release |

`CAP-PROJ1` adds deterministic publication and runtime-identity evidence:

| Delivered behavior | Deterministic evidence | Bound or failure rule |
| --- | --- | --- |
| Closed typed values | [`projection value validation`](../core/tests/capability_projection.rs) | Missing, extra, kind-mismatched, and public-name-mismatched values fail before an immutable projection can escape; UI asset role, content identity, surface digest, dependency kind, and size bounds are also exact |
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

Delivered `HOST-CAP1` has a deterministic Core Tool/Skill host slice plus the
official CLI and Desktop adoption evidence. Delivered `HOST-AGENT1` extends
the same Core transaction and Run lease to Agent delegation. Delivered
`HOST-COMMAND1` extends that admission boundary to blocking and streaming
slash-command execution. Delivered `HOST-HOOK1` extends it to generation-exact
Hook definition/handler bindings and supervised observational work. Delivered
`HOST-MCP1` extends the Core boundary to immutable exact-client MCP bindings;
authoritative A3S Use surface projection and official-host adoption remain a
separate integration boundary. Delivered `HOST-CONTEXT1` extends the same Core
transaction to general Run-frozen Context providers without bypassing the
persisted cognitive-package boundary. Delivered `HOST-FLOW1` extends it to a
named, runtime-build-compatible `WorkflowSpec`/`FlowEngine` pair while A3S Flow
keeps store, replay, runtime, and observation ownership. Delivered
`HOST-KNOWLEDGE1` extends it to exactly one Run-frozen cognitive provider and
binding while the Knowledge host keeps OKF, indexing, retrieval, retention,
and exact query-lease ownership. Delivered `HOST-UI1` extends the Core boundary
to bounded, path-free reviewed UI bytes and a non-clone exact-generation host
handle while renderer policy and authoritative Use dependency projection remain
outside Code.

| Core host behavior | Deterministic evidence | Bound or failure rule |
| --- | --- | --- |
| Complete Session batch | [`capability_runtime_tests`](../core/src/agent_api/capability_runtime_tests.rs) | Every target value prepares before one generation/digest CAS; failure or cancellation leaves the visible stamp unchanged and closes prepared effects |
| Real Use lease per Run | [`exact lease and cursor mismatch tests`](../core/src/agent_api/capability_runtime_tests.rs) | The published provider is generation-specific, every Run acquires again, and Code rejects a returned generation, capability revision, or Registry revision mismatch |
| Capability checkpoint temporal identity | [`Run binding tests`](../core/src/capability/run_binding.rs), [`checkpoint recovery tests`](../core/tests/agent_capability_checkpoint_recovery_v1.rs), and [`portable format tests`](../core/tests/session_checkpoint_v1.rs) | Every new Run/checkpoint binds the exact Code generation, catalog digest, complete authority-ceiling digest, and optional Use cursor; N/N+1 drift and preparation/spawn cutover fail before target admission, while a fresh Session accepts only one exact all-or-nothing historical batch |
| Tool N/N+1 isolation | [`definition/executor cutover test`](../core/src/agent_api/capability_runtime_tests.rs) | An admitted N Run keeps one Tool `Arc` for definition and execution plus N's Use lease while N+1 becomes visible only to a later Run |
| Skill discovery N/N+1 isolation | [`Skill search cutover test`](../core/src/agent_api/capability_runtime_tests.rs) | `search_skills` resolves the Run-frozen Skill registry while N+1 is published, rather than consulting the Session-latest map |
| Agent snapshot identity | [`AgentRegistry snapshot test`](../core/src/subagent/tests.rs) | Each Run owns an independent name map, shares the exact projected `Arc<AgentDefinition>`, and cannot observe later compatibility mutation |
| Agent delegation N/N+1 isolation | [`Agent task cutover and automatic-delegation tests`](../core/src/agent_api/capability_runtime_tests/agent_projection.rs) | Parent definitions, automatic selection, and `task` execution share one Run-frozen Agent registry; an N Run starts the N child after N+1 publication and holds N's exact Use lease through foreground completion |
| Command snapshot identity | [`CommandRegistry snapshot tests`](../core/src/commands.rs) | Each Run owns an independent name map while sharing the exact projected `Arc<dyn SlashCommand>`; later compatibility mutation cannot rewrite the snapshot |
| Command dispatch N/N+1 isolation | [`Command cutover test`](../core/src/agent_api/capability_runtime_tests/command_projection.rs) | Blocking and streaming dispatch execute through the Run-frozen registry; an N Command remains on N after N+1 publication and holds N's exact Use lease through execution |
| Hook snapshot identity and scope | [`HookEngine snapshot tests`](../core/src/hooks/engine/tests.rs) | Each immutable `HookBinding` preserves exact definition/handler `Arc` identity; compatibility or duplicate names and Session/Skill lifecycle events fail before publication, and equal priorities use Hook ID order |
| Hook N/N+1 and composition | [`Hook projection tests`](../core/src/agent_api/capability_runtime_tests/hook_projection.rs) | An N Run retains N metadata, N callback, and N's exact Use lease after N+1 publication; a Session-static external executor composes without allowing its `Skip` to bypass projected policy |
| Hook structured observation | [`Hook supervision tests`](../core/src/agent_api/capability_runtime_tests/hook_projection.rs) | PostResponse, `async_execution`, gating timeout, and observational timeout work retain the exact Use lease through bounded supervisor settlement; official SDK bridges register complete Hook pairs atomically |
| MCP binding readiness and identity | [`McpBinding` tests](../core/src/mcp/binding.rs) | The exact client must be initialized, connected, and identity-matched; tool definitions are sorted and bounded, duplicate names fail, and a public wrapper calls the raw name on that client rather than a mutable manager |
| MCP N/N+1 and dual-lease isolation | [`MCP projection tests`](../core/src/agent_api/capability_runtime_tests/mcp_projection.rs) | An N Run keeps N definitions, N raw calls, and its separately acquired N Use lease after N+1 publication; the N connection effect cannot close until the final N projection reader drops |
| MCP rollback and delegation | [`stdio rollback test`](../core/src/agent_api/capability_runtime_tests/mcp_projection.rs) and [`delegated child test`](../core/src/tools/task/tests.rs) | Cancellation after exact connection readiness leaves the catalog stamp unchanged and reaps the process through rollback; a foreground delegated child receives the parent's exact binding rather than rediscovering a latest client |
| Context N/N+1 and authority isolation | [`Context projection tests`](../core/src/agent_api/capability_runtime_tests/context_projection.rs) and [`child prompt-context isolation test`](../core/src/child_run/tests.rs) | An N Run keeps N general providers and its exact N Use lease after N+1 publication; descriptor/provider and Session-static names cannot conflict; a provider carrying cognitive package authority fails before publication; delegated children keep isolated prompt context |
| Flow N/N+1 and runtime-build isolation | [`Flow projection tests`](../core/src/agent_api/capability_runtime_tests/flow_projection.rs) | `WorkflowSpec::name` binds descriptor lookup; the exact engine must admit the pinned runtime build; an N host handle keeps N's spec, engine/store, and exact Use lease after N+1; missing lookup acquires no lease; Session close cancels active replay and explicit close releases the lease |
| Knowledge N/N+1 and temporal identity | [`Knowledge projection tests`](../core/src/agent_api/capability_runtime_tests/knowledge_projection.rs), [`checkpoint format tests`](../core/tests/session_checkpoint_v1.rs), and [`Run binding tests`](../core/src/run.rs) | Exactly one cognitive authority enters a Run; N keeps N provider, binding, and Use lease after N+1; each Run records its own immutable binding while an ordinary Session save samples the next-Run binding; a live logical checkpoint instead binds its portable Session view to source Run N and rejects mixed N/N+1 recovery; resume must bootstrap exactly; ambient general Context and stale-seed exposure fail before publication |
| UI content and N/N+1 isolation | [`UiBinding` tests](../core/src/capability/ui_binding.rs), [`UI projection validation`](../core/tests/capability_projection.rs), and [`UI host-handle tests`](../core/src/agent_api/capability_runtime_tests/ui_projection.rs) | Entry, style, and script bytes are bounded, path-free, role-checked, content-addressed, and surface-addressed; only Tool, Skill, MCP, and Flow dependency edges are accepted; an N handle keeps N's exact document and Use lease after N+1; missing lookup acquires no lease; Session close signals cancellation and explicit close releases the lease |
| Compatibility conflict boundary | [`pre-commit and post-publication conflict tests`](../core/src/agent_api/capability_runtime_tests.rs) and [`MCP conflict tests`](../core/src/agent_api/capability_runtime_tests/mcp_projection.rs) | Tool, Skill, canonical Agent alias, Command, Hook, MCP server, and fully qualified MCP wrapper conflicts fail before generation advance; later compatibility mutation cannot shadow or masquerade as removal of a published value |
| Close and cancellation linearization | [`prepare cancellation, close/commit race, and exact-Run admission tests`](../core/src/agent_api/capability_runtime_tests.rs) | Close and commit have one order, prepared effects cannot become orphaned, exact Run reservations settle on admission failure, and non-clean scope teardown becomes a typed Run failure |
| Explicit migration boundary | [`SessionCapabilityBatch`](../core/src/capability/runtime.rs), [`FlowBinding`](../core/src/capability/flow_binding.rs), [`UiBinding`](../core/src/capability/ui_binding.rs), and [`McpProjectionAdapter`](../core/src/mcp/projection.rs) | Core accepts Tool, Skill, Agent, Command, Hook, MCP, named Flow, exact Knowledge, general Context, and bounded UI values. MCP and UI inputs must come from already selected authoritative Use evidence; cognitive authority remains a separately persisted exact Run/Session boundary; Flow and UI remain host-only unless explicitly adapted into governed Tools; this evidence does not claim upstream or official-host adoption |

`CAP-PROFILE1` and `HARNESS-PROFILE1` add deterministic model-presentation
evidence without adding another capability or package lifecycle:

| Delivered behavior | Deterministic evidence | Bound or failure rule |
| --- | --- | --- |
| Closed Profile values | [`Tool presentation tests`](../core/src/tools/presentation.rs) | Only Adaptive, Direct, Code, and Disabled exist; outputs are canonical, bounded, and cannot add a name or change a parameter schema |
| Permission-first model projection | [`actual model-request tests`](../core/src/agent/tests.rs) | The permission checker removes definitions before code-mode catalog generation, so a hidden Tool cannot reappear by name or schema |
| Definition/execution separation | [`governed Tool instance test`](../core/src/agent/tests.rs) | A disabled Profile changes only model definitions; the existing `ToolInvoker` still resolves the original registered Tool instance under normal governance |
| Child and resume ceilings | [`child inheritance test`](../core/src/child_run/tests.rs) and [`Session persistence tests`](../core/src/agent_api/session_persistence.rs) | Delegated runs inherit the exact parent Profile, and resume inherits the persisted value or rejects an explicit mismatch |
| Per-call Profile evidence | [`Harness evidence tests`](../core/src/harness_evidence/tests.rs) and [`LLM invoker tests`](../core/src/agent/llm_invoker/tests.rs) | Profile identity, application kind, source/presented cost, and actual input agree; unknown, schema-modified, or description-injected definitions fail before provider use |
| SDK parity | [`Node conversion`](../sdk/node/src/session_options.rs), [`Python conversion`](../sdk/python/src/session_options_conversion.rs), and [`Go bridge conversion`](../sdk/go/bridge/src/lib.rs) | Every SDK accepts a typed Profile object and maps to the same Rust closed value; no Session option accepts a primitive Profile name |

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
| Context and memory      | [`context` tests](../core/src/context), [`memory` tests](../core/src/memory.rs), [`durable semantic contracts`](../core/tests/durable_memory_semantic.rs), [`semantic refresh contracts`](../core/tests/durable_memory_semantic_refresh.rs), [`semantic refresh CAS races`](../core/tests/durable_memory_semantic_refresh_cas.rs), [`semantic refresh change detection`](../core/tests/memory_semantic_refresh_change_detection.rs), [versioned durable-memory evaluations](../core/tests/fixtures), [`durable restart`](../core/tests/durable_memory_restart.rs), [`durable restart endurance`](../core/tests/durable_memory_restart_endurance_eval.rs), [`maintenance lifecycle`](../core/tests/memory_maintenance_lifecycle.rs), [`semantic refresh scheduling`](../core/tests/memory_semantic_refresh_schedule.rs), [`semantic refresh change tokens`](../core/tests/memory_semantic_refresh_change_token.rs), [`semantic refresh checkpoints`](../core/tests/memory_semantic_refresh_checkpoint.rs), [`semantic refresh metrics`](../core/tests/memory_semantic_refresh_metrics.rs), and [`bounded maintenance close`](../core/tests/memory_maintenance_close.rs) cover ranked assembly, compaction, active-only lexical/semantic/relation recall, RRF, exact namespace/revision/content verification, complete dual-budget snapshot rebuild, token-accelerated unchanged ticks and stable rebuilds, full-snapshot compatibility fallback, safe one-snapshot checkpoint recovery, repository/vector-history collision and missing-token fallback, refresh drift cleanup and receipts, strict consistency and schedule admission, delayed publication/cleanup rejection, exclusive schedule ownership, schema-4/5 resume bindings, pure preview, evidence-backed extraction, real-session admission/use, explicit sharing, repeated restart, immutable history, supersession, pruning, cancellation, health, retained receipts, bounded redacted refresh-work metrics, lock release, and post-publication close settlement. | The [retrieval](DURABLE_MEMORY_RETRIEVAL_EVAL.md), [product](DURABLE_MEMORY_PRODUCT_EVAL.md), [multilingual](DURABLE_MEMORY_MULTILINGUAL_EVAL.md), [semantic](DURABLE_MEMORY_SEMANTIC_EVAL.md), [semantic refresh](DURABLE_MEMORY_SEMANTIC_REFRESH.md), and [restart-endurance](DURABLE_MEMORY_RESTART_ENDURANCE_EVAL.md) gates lock retrieval/product quality, same-language CJK behavior, deterministic cross-language semantic serving, Candidate/tenant/stale isolation, bounded context/provider calls, admissions, verified refresh, shared-index revision CAS, session-owned refresh scheduling, safe host-persisted checkpoint recovery, and retained restart history. Real embedding models, larger independently labeled corpora, durable remote CAS and lease fencing, production refresh cadence, long-horizon consolidation, real-provider latency and cost, remote failover, and drift distributions remain host qualification. | [`context_memory_benchmark`](../core/examples/context_memory_benchmark.rs) gates ranked/deduplicated assembly over 25,000 inputs, recall over 2,500 memories, output budgets, correctness, and active/retained RSS. [`durable_memory_semantic_refresh_benchmark`](../core/examples/durable_memory_semantic_refresh_benchmark.rs) gates exact refresh work, close/reopen recovery, local semantic recall, durable bytes, and active/retained RSS over 10,000 nodes and 384-dimensional SQLite vectors. Results are retained in the [performance record](PERFORMANCE_QUALIFICATION.md). | Required correctness CI and release qualification. |
| Cognitive packages      | [`cognitive_context` tests](../core/src/cognitive_context.rs) cover exact generation binding, citation/source verification, restart checks, and fail-closed provider behavior.                                                                                                                                                                                                                     | Host package resolution remains outside Code by design and must be qualified by the embedding host.                                                                                                                                                      | Source count, bytes, generation identity, and verification work are bounded. Package installation latency is explicitly outside the Code runtime claim.                                                                                                                                                                        | Required CI for the Code boundary; external host qualification required. |
| A3S Use Runtime Tasks   | [`use_runtime_tasks` tests](../core/src/use_runtime_tasks.rs) cover exact snapshot decoding, canonical capability identity, bounded argument parsing, preparation cancellation, dispatcher request/response identity, exit failures, and declared JSON validation.                                                                                                                                  | A resident A3S Use host must separately qualify the real generation lease and `RuntimeTaskDispatcher`; Code neither resolves packages nor launches the projected command.                                                                                 | Each Task is limited to 256 arguments, 32 KiB per argument, 16 MiB each for stdout and stderr, and a one-hour maximum deadline. Dispatch amplification is exactly one host call per admitted Tool invocation.                                                                                                                     | Required CI for the Code adapter; external Use-host qualification required. |
| Model adapters          | [`LLM tests`](../core/src/llm/tests.rs), [`OpenAI tests`](../core/src/llm/openai/tests.rs), [`Anthropic adapter`](../core/src/llm/anthropic.rs), and [`HTTP admission tests`](../core/src/llm/admission.rs) cover normalization, streaming, tool calls, errors, retries, and redaction.                                                                                                            | [`test_deepseek_adversarial_e2e`](../core/tests/test_deepseek_adversarial_e2e.rs), Codex-login tests, and real-config scripts qualify live transport separately.                                                                                         | Request timeouts, retry limits, response limits, token usage, and cancellation are bounded. Remote latency and cost are reported, not used as deterministic Core speed gates.                                                                                                                                                  | Required protocol CI plus external qualification.                        |
| Structured output       | [`structured tests`](../core/src/llm/structured_tests.rs) and [`generate_object contract tests`](../core/src/tools/builtin/generate_object_contract_tests.rs) cover native schemas, prompt fallback, partial parse, repair, and validation.                                                                                                                                                        | [`test_structured_json_real_llm`](../core/tests/test_structured_json_real_llm.rs) and Codex-login variants exercise real providers.                                                                                                                      | Schema size, repair attempts, output size, and provider rounds are bounded.                                                                                                                                                                                                                                                    | Required CI plus external qualification.                                 |
| MCP and Skills          | [`MCP manager tests`](../core/src/mcp/manager/tests.rs), [`MCP protocol tests`](../core/src/mcp/protocol/tests.rs), [`Skill registry tests`](../core/src/skills/registry/tests.rs), and [`live_skill_lifecycle`](../core/tests/live_skill_lifecycle.rs) cover discovery, transport, isolation, registration, removal, inheritance, and validation.                                                 | Stdio, HTTP/SSE, and streamable HTTP transports are exercised with local fixtures; remote servers remain host qualification.                                                                                                                             | Discovery timeouts, reconnects, idle disconnect, payload bounds, Skill size, and invocation limits are explicit.                                                                                                                                                                                                               | Required CI.                                                             |
| Planning and delegation | [`subagent tests`](../core/src/subagent/tests.rs), [`task tool tests`](../core/src/tools/task/tests.rs), [`parallel execution tests`](../core/src/tools/task/parallel_execution.rs), and [`permission inheritance tests`](../core/tests/test_subagent_permissions.rs) cover plans, workers, foreground/background tasks, inheritance, progress, cancellation, and close.                           | Real parallel delegation is isolated in [`test_auto_delegation_real_parallel`](../core/tests/test_auto_delegation_real_parallel.rs).                                                                                                                     | Worker count, depth, steps, parallelism, budgets, and cancellation deadlines are bounded.                                                                                                                                                                                                                                      | Required CI plus external qualification.                                 |
| Priority scheduling     | [`task_scheduler` tests](../core/src/task_scheduler.rs), [`session lane queue` tests](../core/src/session_lane_queue.rs), and [`queue` tests](../core/src/queue.rs) cover priority, FIFO order, aging, occupancy, cancellation, and shutdown.                                                                                                                                                      | SDK runtime suites verify the exported configuration and snapshots.                                                                                                                                                                                      | Capacity, pending counts, aging intervals, admission timeouts, and shutdown are explicit; ordering tests use logical work rather than wall-clock throughput claims.                                                                                                                                                            | Required CI and SDK runtime CI.                                          |
| Programmable workflows  | [`program tests`](../core/src/program/tests.rs), [`program tool tests`](../core/src/tools/program_tool/tests.rs), [`dynamic workflow tests`](../core/src/dynamic_workflow/tests.rs), and [`QuickJS integration`](../core/tests/test_program_script_quickjs_integration.rs) cover bounded scripts, nested tools, Flow replay, budgets, and cancellation.                                            | [`workflow facade real LLM`](../core/tests/test_workflow_facade_real_llm.rs) is external qualification.                                                                                                                                                  | Script memory, time, calls, output, workflow steps, and shared budgets are bounded. [`flow_graph_benchmark`](../core/examples/flow_graph_benchmark.rs) gates 1,000-step projection p95, deterministic record counts, serialized bytes, and replay.                                                                             | Required correctness CI and release qualification.                       |
| Persistence             | [`store tests`](../core/src/store/tests.rs), [`persisted schema roundtrip`](../core/tests/test_persisted_schema_roundtrip.rs), and [`session persistence`](../core/src/agent_api/session_persistence.rs) cover atomic snapshots, events, traces, artifacts, verification, checkpoints, retention, and migration rejection.                                                                         | Cross-process and recovery behavior is also exercised by the protocol harness and convergence benchmark.                                                                                                                                                 | [`persistence_benchmark`](../core/examples/persistence_benchmark.rs) gates memory and synchronized file-store save/load p95 for a 1–2 MiB aggregate snapshot, repeated overwrite without accumulation, byte fidelity, list cardinality, and delete cleanup.                                                                    | Required correctness CI and release qualification.                       |
| State graph             | [`state graph tests`](../core/src/state_graph/tests.rs) and [`state graph integration`](../core/tests/test_state_graph_integration.rs) cover hash links, objects, relations, optimistic patches, replay, forks, diffs, and Flow lifecycle projection.                                                                                                                                              | Cross-project ontology fixtures in [`r0-cross-project-contract`](../core/tests/r0_cross_project_contract.rs) lock compatibility.                                                                                                                         | [`flow_graph_benchmark`](../core/examples/flow_graph_benchmark.rs) gates replay p95 and verifies that 11,008 records restore the exact object/relation shape within a 64 MiB serialized-event ceiling.                                                                                                                         | Required correctness CI and release qualification.                       |
| Agent release contract  | [`agent_release_manifest`](../core/tests/agent_release_manifest.rs), [`r0 cross-project contract`](../core/tests/r0_cross_project_contract.rs), and [`agent directory convention`](../core/tests/test_agent_dir_convention.rs) cover bounded admission, canonical identity, exact post-build artifact/provenance binding, compatibility, and secret slots. | The [minimal publication fixture](../fixtures/agent-release-contract/README.md) builds without the final manifest, pushes one OCI image manifest, generates canonical ACL after digest resolution, and can pull/run the exact digest through local Docker. Retained Cloud Runtime and real-provider evidence remain external gates. | Manifest size, file count, canonicalization work, compatibility checks, health wait, shutdown deadline, and retained secret-free evidence are bounded. | Required CI and release preflight; local Docker plus external certification for deployment claims. |
| Headless Agent protocol | [`agent_protocol_v1`](../core/tests/agent_protocol_v1.rs), [`agent_protocol_harness`](../core/tests/agent_protocol_harness.rs), [`agent_exact_run_host`](../core/tests/agent_exact_run_host.rs), [`agent_live_checkpoint_export_v1`](../core/tests/agent_live_checkpoint_export_v1.rs), [`agent_exact_checkpoint_recovery_v1`](../core/tests/agent_exact_checkpoint_recovery_v1.rs), and [`agent_portable_checkpoint_recovery_v1`](../core/tests/agent_portable_checkpoint_recovery_v1.rs) cover sessions, runs, cancellation, blocking/streaming live-boundary export, ordered multi-round series, export-failure isolation, identical SessionStore/host-sink logical values and terminal cleanup, latest-boundary and complete-descriptor recovery, one Harness-visible portable restore admission, zero split prewrites, persisted-generation drift, receipts, exact replay, event pages, worktrees, and immutable patches. | The CLI integration workflow qualifies service transport around the Core host; Cloud/common Harness certification must add external datastore revision fencing. | Event page size, retention gaps, worktree scope, cancellation, checkpoint payloads, sink backpressure, and recovery attempts are bounded. | Required CI plus host integration CI. |
| Headless web search     | [`web search tests`](../core/src/tools/builtin/web_search/tests.rs), [`engine tests`](../core/src/tools/builtin/web_search/engines.rs), and [`headless integration tests`](../core/tests/test_web_search_headless.rs) cover engine routing, parsing, fallback, SSRF defenses, admission, and browser lifecycle.                                                                                    | [`Hermetic Integrations`](../.github/workflows/hermetic-integrations.yml) drives installed Chrome through the production CDP path and Google parser against a controlled HTTPS fixture. Public Google/Baidu availability remains external qualification. | Query timeout, result count, request coalescing, circuits, tabs, response bytes, and browser shutdown are bounded. The retained report and request log prove parser output and browser cleanup without including public-engine network latency.                                                                                | Required deterministic and hermetic browser integration CI.              |
| S3 workspace            | [`S3 unit tests`](../core/src/workspace/s3/tests.rs) cover path normalization, capability gating, read/search limits, concurrency, and configuration. [`test_s3_backend`](../core/tests/test_s3_backend.rs) covers live read/write/edit/patch/list behavior.                                                                                                                                       | [`Hermetic Integrations`](../.github/workflows/hermetic-integrations.yml) starts pinned MinIO and executes the live backend test through write, read, list, edit, patch, and cleanup. Actual deployment compatibility remains external qualification.    | Read bytes, objects scanned, bytes per object, search concurrency, and request timeout are bounded. The retained MinIO report requires the complete roundtrip and zero residual test objects.                                                                                                                                  | Required unit and hermetic object-store integration CI.                  |
| Filesystem agent server | [`serve lifecycle tests`](../core/src/serve/lifecycle.rs), [`daemon tests`](../core/src/serve/daemon.rs), [`schedule tests`](../core/src/serve/schedule.rs), and [`agent directory tools`](../core/tests/test_agent_dir_tools.rs) cover readiness, failure state, schedules, tools, cancellation, and joined shutdown.                                                                             | [`test_serve_agent_dir_real_llm`](../core/tests/test_serve_agent_dir_real_llm.rs) qualifies real scheduled turns separately. Node and Python runtime suites exercise the public serve wrappers without credentials.                                      | Startup, schedule, tool, and shutdown deadlines are bounded.                                                                                                                                                                                                                                                                   | Required all-features CI, SDK runtime CI, and external qualification.    |
| OpenTelemetry           | [`telemetry tests`](../core/src/telemetry.rs) and [`OTLP mapping tests`](../core/src/telemetry_otel.rs) cover baseline tracing, optional exporter configuration, propagation, and mapping.                                                                                                                                                                                                         | [`Hermetic Integrations`](../.github/workflows/hermetic-integrations.yml) sends a controlled span to a pinned local Collector and verifies the exact service/span receipt. Remote deployment reachability remains external qualification.                | Export timeout, queue/batch behavior supplied by the configured SDK, and redaction boundaries are documented. The retained report gates initialization, Collector receipt, flush, and shutdown against a 10-second deadline.                                                                                                   | Required all-features and Collector integration CI.                      |

The context-and-memory gate also runs
[`memory_semantic_refresh_change_detection`](../core/tests/memory_semantic_refresh_change_detection.rs).
It proves that a verified unchanged scheduled tick performs no embedding or
vector mutation, source or independent index drift triggers a rebuild, and a
replacement schedule owner cannot reuse the previous process-local receipt. The
same gate proves exact committed-vector reuse for index-only drift, partial
source changes and Active removal; a failed CAS publication cannot promote its
prepared embeddings, and owner replacement forces fresh provider input.
[`memory_semantic_refresh_checkpoint`](../core/tests/memory_semantic_refresh_checkpoint.rs)
proves that serialized recovery evidence never trusts a repository-local token,
requires one complete Active snapshot and exact vector-index history continuity,
avoids provider/publication work only after that proof, and rebuilds for
repository-token collisions, vector-status collisions, or missing vector-token
support.
[`memory_semantic_index_observation`](../core/tests/memory_semantic_index_observation.rs)
proves that semantic publication and query correctness use the fallible async
index observation even when the synchronous status hint is permanently stale.
With `durable-memory-sqlite`,
[`memory_semantic_refresh_sqlite`](../core/tests/memory_semantic_refresh_sqlite.rs)
fully releases and reopens the local SQLite vector index, preserves the exact
history token and revision, and recovers a checkpoint with one source snapshot
and zero duplicate provider or publication work. This does not qualify remote
replication or distributed lease fencing.
[`memory_semantic_refresh_metrics`](../core/tests/memory_semantic_refresh_metrics.rs)
proves that settled published, unchanged, provider-failed, CAS-lost, and
recovered runs report exact snapshot, cache, logical embedding,
provider-boundary retry, and publication work; observations are bounded,
redacted, retained through close, and reset before replacement ownership. The
adapter-boundary counts do not claim remote transmission or billing.
The release-only
[`durable_memory_semantic_refresh_benchmark`](../core/examples/durable_memory_semantic_refresh_benchmark.rs)
adds a retained 10,000-node, 384-dimensional local profile over the synchronized
file repository and SQLite vector index. It gates exact stable/source-drift/
index-drift/recovery work, a host-synchronized checkpoint across real handle
close/reopen, semantic-query p50/p95/max, disk ceilings, and Linux RSS. The
deterministic in-process adapter does not qualify real-model quality, provider
network or billing, an operating-system process restart, remote CAS/leases, or
remote failover; those remain `DM-PROD1` host evidence.

## Runtime surface ledger

| Surface            | Required runtime evidence                                                                                                                                                                                                 | Cross-platform evidence                                                                         | Current gate             |
| ------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------- | ------------------------ |
| Rust Core          | Default workspace tests, all-feature library tests, strict Clippy, semver compatibility, integration tests, and targeted qualification examples.                                                                          | Linux and Windows required CI; retrieval churn also runs on macOS.                              | Required CI.             |
| Node.js            | Build the N-API module, run type tests, construct agents and sessions, observe memory-maintenance health, execute direct/governed tools, callbacks, stores, workspace retrieval, serve lifecycle, close behavior, and fixture validation through JavaScript. | Publication builds all supported native targets; deterministic runtime CI runs on Linux.        | Required SDK runtime CI. |
| Python             | Build and install the PyO3 extension into an isolated environment, run pytest contracts and executable lifecycle/budget/delegation scripts, observe memory-maintenance health, validate retrieval fixtures, and close all resources through Python.          | Publication builds the supported CPython/target matrix; deterministic runtime CI runs on Linux. | Required SDK runtime CI. |
| Go                 | Build the versioned bridge, run the Go package with the race detector, and exercise lifecycle, memory-maintenance health, tools, stores, retrieval, streams, runs, verification, and MCP through the bridge protocol.                                | Required runtime CI on Linux and Windows; release assets cover the target matrix.               | Required CI.             |
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
assembly and 2,500-item memory recall, 10,000-node durable semantic refresh and
SQLite recovery, and approximately 1.25 MiB session persistence. The
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

As of Code `9d17c63`, no unresolved Code-owned evidence gap remains in this
ledger. The retained qualification record now covers:

1. a 5,000-file Code Intelligence latency, process, shutdown, and RSS profile;
2. a 25,000-input context assembly and 2,500-item memory recall/RSS profile;
3. a 1,000-step Flow projection and 11,008-record State Graph replay profile;
4. a pinned MinIO roundtrip and cleanup gate;
5. a controlled HTTPS fixture through installed Chrome, the production CDP
   path, and the Google parser;
6. a pinned local OpenTelemetry Collector receipt, flush, and shutdown gate;
7. synchronized file and in-memory persistence profiles with repeated
   overwrite and delete cleanup;
8. a 10,000-node, 384-dimensional durable semantic-refresh profile with exact
   source/provider/publication amplification gates, SQLite and file-repository
   close/reopen recovery, synchronized checkpoint evidence, local semantic
   recall percentiles, disk ceilings, and active/retained RSS ceilings.

The successful [Performance Qualification run
`33304362997`](https://github.com/A3S-Lab/Code/actions/runs/33304362997),
[Hermetic Integration run
`32130843684`](https://github.com/A3S-Lab/Code/actions/runs/32130843684), and
their artifact digests are recorded in
[Performance Qualification](PERFORMANCE_QUALIFICATION.md).

Intrinsic external claims remain explicitly scoped: live model behavior and
real-provider quality/latency/cost, independently labeled larger memory corpora,
remote vector-backend CAS and distributed lease fencing, production refresh
cadence, remote failover, long-horizon consolidation, public search-engine
availability, deployment-specific S3/Collector networks, and cognitive-package
host resolution require qualification in the target environment. They are not
Code-owned deterministic gaps. A controlled local substitute remains required
and present for Code-owned parsing, admission, lifecycle, and resource behavior.

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
