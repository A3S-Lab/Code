# A3S Code 2.0 Refactor Plan

## North Star

A3S Code 2.0 is a harness-driven runtime for coding agents.

The model reasons. The harness controls context, actions, safety, verification,
and compaction.

```text
Intent -> Context -> Action -> Observation -> Verification -> Compaction
```

The 2.0 goal is not to add more default tools. It is to make the default path
small, predictable, testable, and extensible.

## First Principles

1. The runtime kernel should be small.
2. Every context source must go through one budgeted context path.
3. Every action must go through one selection path.
4. Every side effect must go through one safety path.
5. Completion requires verification or an explicit residual-risk report.
6. Subagents isolate context; they do not dump transcripts into the parent.
7. Programmatic Tool Calling moves repetitive tool chains out of the LLM loop.
8. AHP is a harness extension, not a parallel prompt-injection path.

## Target Shape

```text
a3s-code
├── runtime kernel
│   ├── agent loop
│   ├── state
│   ├── events
│   └── trace
│
├── harness
│   ├── intent router
│   ├── context assembler
│   ├── tool selector
│   ├── program executor
│   ├── safety gate
│   ├── verification loop
│   └── compaction engine
│
├── capabilities
│   ├── core tools
│   ├── skills
│   ├── MCP
│   ├── memory
│   ├── web
│   └── git
│
├── delegation
│   ├── task
│   ├── parallel_task
│   └── workflow plugins
│
└── API
    ├── Rust
    ├── Python
    └── Node.js
```

## Default 2.0 Runtime

Default-on:

- `AgentLoop`
- `ToolExecutor`
- `ToolSelector`
- `PermissionPolicy`
- `ConfirmationPolicy`
- session compaction
- `task` and `parallel_task`
- `search_skills` and `Skill`

Default-off or advanced:

- orchestrator workflows
- `SessionLaneQueue`
- AHP
- long-term memory
- `manage_skill`
- MCP tools
- web/git/batch tools unless selected by intent

## Phase 0: Stabilize Current 1.x Path

Goal: reduce duplication and make later harness extraction safe.

- Extract session invocation assembly shared by `generate` and `generate_streaming`.
- Keep behavior unchanged.
- Keep public API unchanged.
- Preserve existing tests.

## Phase 1: Unified Tool Selection

Goal: one path decides which actions are visible to the model.

- Keep `ToolSelector` as the public selection surface.
- Keep one deterministic selector path for model-visible tools.
- Keep tool execution independent of model-visible tool definitions.
- Gate web/git/batch/team/MCP/manage_skill by intent and policy.

## Phase 2: Context Assembler

Goal: every context source produces structured items before prompt rendering.

Introduce:

```text
ContextItem {
  source
  content
  relevance
  trust
  freshness
  token_cost
  metadata
}
```

Route these through a single pipeline:

```text
providers -> rank -> dedupe -> budget -> render
```

Migrate:

- AGENTS.md
- skills
- project hints
- memory
- AHP injected context
- subagent summaries
- tool observations

## Phase 3: Programmatic Tool Calling

Goal: move repetitive tool chains out of the LLM loop.

Introduce:

- `Program`
- `ProgramExecutor`
- `ProgramResult`
- `TraceStore`
- `ProgramTool` wrapper for model-visible program calls

First programs:

- `program_code_search`
- `program_repo_map`
- `program_test_analysis`

Program results must return summaries, findings, artifact references, and next
action suggestions. Raw output belongs in trace storage.

## Phase 4: Safety Gate

Goal: one authorization decision for all side effects.

Unify:

- permission policy
- confirmation policy
- skill grants
- workspace boundaries
- security provider hints
- network and destructive-action rules

External API:

```text
SafetyGate.authorize(action, state) -> Allow | Ask | Deny
```

## Phase 5: AHP As Harness Extension

Goal: AHP suggests; the harness decides.

AHP should emit suggestions:

- add or boost context
- enable an action
- require confirmation
- request compaction
- provide policy hints

AHP suggestions must flow through:

```text
ContextAssembler
ToolSelector
SafetyGate
CompactionEngine
```

AHP must not bypass context budgets or directly append prompt text.

## Phase 6: Verification Loop

Goal: make verification an explicit completion condition.

Introduce:

- `Verifier`
- `Check`
- `VerificationReport`

Verification sources:

- tests
- type checks
- lint
- git diff review
- subagent review
- explicit risk report

## Phase 7: Advanced Workflow Isolation

Goal: make non-core orchestration optional.

- Remove the model-visible `run_team` tool and duplicate Orchestrator team shortcut.
- Delete explicit Lead/Worker/Reviewer team execution; route multi-agent work through `task` / `parallel_task`.
- Move remaining orchestrator APIs out of the core default path.
- Lazy-initialize queue/lane infrastructure only when configured.

## Current Execution

Completed:

1. Phase 0: extracted shared session invocation assembly.
2. Phase 0: replaced duplicate `generate` / `generate_streaming` setup.
3. Phase 1: centralized model-visible tool selection behind `ToolSelector`.
4. Phase 2: introduced `ContextAssembler` skeleton for ranking, dedupe, budget, and rendering.
5. Phase 2: routed `ContextProvider` results through `ContextAssembler` before prompt rendering.
6. Phase 2: routed auto-detected project hints through `ContextAssembler` as `a3s://project-hint`.
7. Phase 2: routed `AGENTS.md` through a `StaticContextProvider` instead of direct prompt append.
8. Phase 2: routed skill discovery guidance through `StaticContextProvider`.
9. Phase 2: routed long-term memory recall through `ContextAssembler` instead of prompt rewriting.
10. Phase 2: normalized AHP injected context, including harness suggestions, into `ContextResult`.
11. Phase 2: added context provenance, priority, trust, and freshness metadata signals.
12. Phase 2: added source-aware context item/token caps to prevent one source from dominating.
13. Phase 2: added stable artifact references for compacted subagent task outputs.
14. Phase 2: added stable artifact references for truncated tool outputs.
15. Phase 2: added an in-memory artifact store for truncated tool outputs.
16. Phase 2: added named context assembly budget policies.
17. Phase 3: introduced the minimal `Program` / `ProgramExecutor` PTC skeleton.
18. Phase 3: added instantiable PTC templates and the first built-in program catalog.
19. Phase 3: added a model-visible `program` tool gated by `ToolSelector`.
20. Phase 3: compacted PTC step outputs into bounded summaries with artifact references.
21. Phase 3: exposed artifact retrieval through `ToolExecutor` and `AgentSession`.
22. Phase 3: preserved PTC step labels in results, metadata, and rendered summaries.
23. Phase 3: added program-specific summaries for code search and repo mapping.
24. Phase 3: added bounded artifact-store limits with oldest-first eviction.
25. Phase 3: added artifact-store manifest save/load for file-backed persistence.
26. Phase 3: wired artifact persistence into session save/resume stores.
27. Phase 3: routed PTC executions into structured trace metadata and verification hints.
28. Phase 3: added configurable PTC program catalog registration.
29. Phase 3: added configurable artifact-store retention limits per session.
30. Phase 3: extracted PTC trace metadata into stable typed core structures.
31. Phase 3/6: promoted PTC verification hints into typed core contracts with artifact and failure evidence.
32. Phase 3/6: introduced a shared trace sink abstraction for compact tool and program execution events.
33. Phase 3: added a program-template plugin asset path for extending or replacing the PTC catalog.
34. Phase 3: added typed program-template validation and strict plugin asset registration.
35. Phase 6: introduced verification checks/reports and attached PTC verification reports to program metadata.
36. Phase 6: routed tool verification reports into `AgentResult` for agent completion consumers.
37. Phase 1/6: made default intent routing deterministic by removing extra low-confidence LLM classification from the main loop.
38. Phase 1/6: narrowed automatic planning to explicit planning intent instead of all general-purpose turns.
39. Phase 6: surfaced a verification summary through core `AgentResult` and Node/Python SDK result types.
40. Phase 3/6: persisted compact trace events with sessions and exposed them through core/Node/Python session APIs.
41. Phase 6: added executable verification commands for tests/check/lint through the session tool path and SDK APIs.
42. Phase 6: attached verification summaries to streaming end events across core and Node/Python SDKs.
43. Phase 6: added project-aware verification presets for Rust, Node.js, Python, and Go without auto-running them.
44. Phase 7: removed the model-visible `run_team` tool, Session SDK wrappers, and duplicate `Orchestrator.runTeam()` shortcut so multi-agent work uses one delegation/team path.
45. Phase 7: removed duplicate `AgentSlot` / `orchestrator.spawn(slot)` APIs; advanced subagents now enter through `SubAgentConfig` and `spawn_subagent` only.
46. Phase 7: documented remaining Orchestrator APIs as an advanced SubAgent control plane, not the default multi-agent composition path.
47. Phase 7: removed the unused 1.0 task/progress/idle lifecycle API from core and SDKs; runtime evidence now flows through events, trace, artifacts, and verification.
48. Phase 6/7: added a shared verification summary formatter plus Node/Python SDK helpers and streaming text fields for result viewer consumption.
49. Phase 6: persisted verification reports as session-side completion evidence and exposed session verification evidence/summary APIs across core and SDKs.
50. Phase 7: cleaned remaining model-visible and SDK/example wording so Orchestrator is documented as an advanced SubAgent control plane, not a second default multi-agent composition path.
51. Phase 7: made the lower-level `session::Session` lane queue optional so queue infrastructure is initialized only when queue configuration is explicitly present.
52. Phase 6/7: made default-history streaming sessions accumulate history and verification evidence on completion, including auto-save, while custom-history streams remain isolated.
53. Phase 7: re-ran the public README/API docs pass after queue isolation and clarified that lane queues are optional advanced external/hybrid dispatch infrastructure, not the default session path.
54. Phase 7: reviewed the lower-level `session::Session` surface and re-documented it as managed-session infrastructure below the primary `AgentSession` API, with planning snapshots and optional queues explicitly separated from default execution.
55. Phase 6/7: tightened streaming persistence tests so failed or cancelled streams do not update history or auto-save partial state.
56. Phase 7: removed generic Session queue `submit`/`submitBatch` helpers from Rust AgentSession and Node/Python SDKs so lane queues remain advanced external/hybrid dispatch infrastructure instead of a second public task execution API.
57. Phase 7: cleaned public examples after README/API cleanup by moving SubAgent control-plane examples out of Node basic examples, narrowing the MiniMax basic example to default Session/tools/events, and marking Python SubAgent docs as advanced.
58. Phase 7: fixed stale Node example bindings after the public API cleanup (`maxToolRounds` casing and inline ACL config) and verified the examples TypeScript project.
59. Phase 7: audited SDK entry/type surfaces after queue cleanup and archived historical Python docs so 1.4.x investigation reports no longer read as canonical 2.0 API guidance.
60. Phase 7: removed TeamRunner from the recommended path; new multi-agent composition has a single path: `task` / `parallel_task`.
61. Phase 7: removed overfitted Node queue examples that presented lane queues as generic task parallelism/priority APIs, leaving queue coverage as advanced external/hybrid dispatch diagnostics.
62. Phase 7: added an unreleased Python SDK 2.0 changelog boundary and fixed stale Node package example script paths after the examples reorganization.
63. Phase 7: converted Node search JavaScript examples to ESM imports so they match the examples package module mode and package scripts can execute them.
64. Phase 7: fixed remaining Node example module imports that mixed ESM files with CommonJS `require` or pointed at the examples directory instead of the SDK entrypoint.
65. Phase 7: removed legacy TeamRunner/AgentTeam types from the Rust crate root re-exports as preparation for deleting the compatibility module.
66. Phase 7: deleted explicit Lead/Worker/Reviewer TeamRunner/AgentTeam APIs from core, Node SDK, Python SDK, TypeScript declarations, and public examples.
67. Phase 7: removed the remaining TeamRunner prompt files, Node smoke-test exports, and selector compatibility remnants so deleted team orchestration is no longer present in runtime code.
68. Phase 7: deleted placeholder SubAgent execution from the Orchestrator control plane, made Node/Python `Orchestrator.create()` require a real `Agent`, removed placeholder-only Node examples, and deleted historical Python investigation reports.
69. Phase 7: removed remaining internal compatibility shims for `SystemPromptSlots::from_legacy` and `hitl::SessionLane`, made `.acl` the only supported config file extension, rejected legacy `.hcl` files, and required labeled ACL `providers` / `models` blocks.
70. Phase 7: removed no-op `SessionOptions::with_sandbox(SandboxConfig)` and `SandboxConfig`, leaving only concrete `with_sandbox_handle()` execution, removed the `ToolSearchConfig.enabled=false` all-tools fallback, and deleted unused JSON config saving.
71. Phase 7: narrowed the Rust crate root by removing internal `AgentConfig`/`AgentLoop`, command, scheduler, and prompt-constant re-exports; deleted unused prompt files and overfitted real-LLM prompt tests.
72. Phase 7: removed the hidden `AgentLoop` auto-subagent callback path, unused `AgentDefinition` spawn/mode flags, and model-visible `task.permissive`, making `task` / `parallel_task` the single explicit delegation core with caller-owned permissions.
73. Phase 7: deleted the unused task XML/JSON notification module, so delegated-work evidence now flows through the unified event, trace, artifact, verification, and optional AHP paths.
74. Phase 7: narrowed the root Rust API to the primary Agent/session/config/LLM/prompt types, moving plugin, skills, subagent, queue, program, trace, hooks, and verification access back to their owning modules.
75. Phase 7: made git and scheduler implementation modules crate-private, deleted unused git helper functions, removed the unused undercover mode and prompt, and dropped the old planning `description` alias.
76. Phase 7: made file-history snapshots internal, removed the unreachable `tool_search` / `ToolIndex` side path, deleted the overfitted Minimax context-perception integration test, and kept tool exposure on the single centralized selector path.
77. Phase 7: hid `session_lane_queue`, removed its dead EventBridge/batch/drain surface, deleted the unused `SessionManager` runtime, and reduced session tests to the retained persistence/state/optional-queue contract.
78. Phase 7: moved persisted session data types into `store`, deleted the unused low-level `session` runtime module, promoted compaction to a crate-private module, and refreshed public docs away from `AgentLoop` / `run_team` / `SandboxConfig` guidance.
79. Phase 7/AHP: integrated the AHP 2.3 protocol explicitly, made AHP executor send full `AhpEvent` envelopes so session/depth/context/metadata survive transport, added full-event client tests, and refreshed AHP examples/docs away from 2.0 decision shapes.
80. Phase 7: removed `SubAgentConfig.permissive` / `permissive_deny` from core, Node, and Python advanced orchestrator APIs so SubAgent lifecycle control no longer carries a second permission-bypass mechanism; permissions now stay on the unified session policy and agent-definition path.
81. Phase 7: removed unused Orchestrator planning event variants, deleted the unused `SubAgentEventPayload`, made `SubAgentWrapper` crate-private, and refreshed Orchestrator docs away from the old EventBus/NATS wrapper architecture.
82. Phase 7: deleted the legacy session cron scheduler (`/loop`, `/cron-list`, `/cron-cancel`) and removed the matching Node/Python `schedule_task` APIs, leaving recurring work to explicit external schedulers that call `AgentSession` through the normal 2.0 path.
83. Phase 7: refreshed user-guide architecture docs away from deleted `core/src/session` internals and updated the Kimi skill example to use an explicit `Skill(*)` permission instead of a session-wide permissive shortcut.
84. Phase 7: added explicit Node/Python `PermissionPolicy` bindings so SDK users can configure allow/deny/ask rules directly instead of relying on the coarse `permissive` compatibility shortcut.
85. Phase 7: removed the Node/Python `permissive` session shortcut and Rust `SessionOptions::with_permissive_policy()` from public session options, then migrated examples/tests to explicit `PermissionPolicy(default_decision=allow)`.
86. Phase 7: deleted broad Node streaming aggregate demos (`agentic_loop_demo`, `advanced_features_demo`, `test_advanced_features`) so streaming examples stay focused and no longer present queue/security/memory/planning as one mixed public path.
87. Phase 7: deleted the Node Orchestrator external-lane Kimi and external-task-handler examples so advanced SubAgent lifecycle control is no longer mixed with the separate optional lane-queue dispatch mechanism.
88. Phase 7: removed the public `PermissionPolicy::permissive()` constructor; allow-by-default policies now use an explicit `default_decision = Allow` shape in tests and examples.
89. Phase 7: removed the Python quick-reference `SubAgentConfig.lane_config` external-dispatch snippet so lane queues are no longer documented as part of the advanced SubAgent control-plane path.
90. Phase 7: removed SubAgent/Orchestrator lane-queue coupling from core, Node, and Python APIs; external/hybrid queue dispatch now remains a session-level mechanism instead of a second SubAgent control-plane path.
91. Phase 7: made the low-level `agent` and `agent_api` modules crate-private and migrated Node/Python bindings to the root `Agent` / `AgentSession` / `AgentEvent` exports, shrinking the public Rust surface to the intended 2.0 facade.
92. Phase 7: cleaned remaining public examples/docs and internal comments that still described ACL configs as HCL, leaving `.hcl` mentions only in the explicit 2.0 rejection path and tests.
93. Phase 7: added a 2.0 completion report, removed a stale Node package script that pointed at the deleted loop-command example, pointed Node examples at the local SDK package, and aligned Python crate repository metadata.
94. Phase 7: fixed Python wheel packaging by adding the `a3s_code` package shim, verified Node package dry-run, built the Python wheel, and smoke-tested `import a3s_code` from the built artifact.
95. Phase 7: rebuilt the Node N-API release artifact and generated declarations, removed the stale context README loop-command reference, refreshed Node package smoke tests, and verified `npm test`, `npm run test:helpers`, and package dry-run.
96. Phase 7: refreshed Node examples runtime wiring so examples install the local SDK, real-provider scripts skip cleanly without credentials/config, and TypeScript ESM files no longer depend on CommonJS `__dirname`.
97. Phase 7: added a no-credential Node examples smoke script, updated examples Quick Start away from live-provider defaults, and made the runtime-nesting example skip cleanly unless real provider credentials/config are available.
98. Phase 7: bumped Rust/Node/Python package metadata to 2.0.0, replaced stale prompt/Kimi smoke script targets with the ACL env integration suite, made the release/preflight scripts require the real-provider ACL smoke before tagging, and let MiniMax smoke/preflight accept `MINIMAX_*` aliases for the ACL `A3S_OPENAI_*` variables.
99. Phase 7: added release-version consistency validation and patch-hygiene checks to the preflight gate so Cargo, npm, Python, lockfile, and optional native package versions must agree before publishing.
100. Phase 7: added default, no-network integration tests that load the repo `.a3s/config.acl`, inject fake `A3S_OPENAI_*` values and `MINIMAX_*` aliases, and verify the MiniMax default LLM config resolves through ACL `env(...)`.
101. Phase 7: added `scripts/real_config_env_integration.sh --dry-run` and wired it into release preflight so ACL env injection can be validated explicitly without provider credentials or network access.
102. Phase 7: taught the real-provider integration script to extract literal OpenAI/MiniMax credentials from `.a3s/config.acl`, inject them into `A3S_OPENAI_*` for the test process, and run against a temporary env-style config without modifying the source config.

Next:

1. Run `REQUIRE_REAL_PROVIDER=1 scripts/release_preflight.sh` with valid provider environment variables before tagging (`A3S_OPENAI_*` or `MINIMAX_*` for the MiniMax default config).
2. Review the large deleted-example set once before publishing.
