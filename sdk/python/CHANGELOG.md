# Changelog

All notable changes to the A3S Code Python SDK will be documented in this file.

## [Unreleased]

### Added

- Added typed, session-bound ephemeral workspace retrieval with host-injected
  async embedding callbacks, lifecycle status, and digest-verified semantic and
  hybrid search DTOs.

## [6.9.0] - 2026-08-12

### Added

- Added agent-wide priority scheduling controls and occupancy snapshots shared
  by conversation runs, direct tools, and detached workflows.
- Added bounded project and personal instruction provenance, tool-result
  transform evidence, and cognitive-package binding projections from Core.

### Changed

- Updated the bundled Core with trusted lifecycle hook governance, including
  model-bound prompt rewrites, permission gates, compaction, and session events.

## [6.8.1] - 2026-08-09

### Added

- Added synchronous and asyncio exact-run admission helpers for detached starts
  and checkpoint recovery, returning the authoritative run snapshot and replay
  state.

### Changed

- Updated the bundled Core with the unified model-visible `search` and `task`
  contracts, including dependency-free bounded BM25 workspace ranking while
  preserving the direct `parallel_task` compatibility helper.
- Updated the bundled Core to A3S Flow 0.11.0 and its complete durable event
  projection contract. The Python API is unchanged.

## [6.8.0] - 2026-08-05

### Changed

- Updated the bundled Core with the native multi-session Agent Harness protocol
  kernel, exact release/session/run admission, durable event-page projection,
  and persisted-session replay. The SDK bindings and public Python API are
  otherwise unchanged.

## [6.7.0] - 2026-07-31

### Changed

- Updated the bundled Core with the workspace-confined `download` tool, the
  default lazy Chrome/Chromium search tier, fail-closed search quality
  enforcement, shared search bulkheads and request coalescing, and bounded
  browser retry budgets.

## [6.6.0] - 2026-07-29

### Added

- Added `HostEnvConfig` and `SessionOptions.host_env` for deterministic ID and
  clock replay parity with Rust, Node.js, and Go.

### Changed

- Updated the bundled Core with quality-gated search tiers, typed web-fetch
  failures, bounded session-forked workflow generation, exact durable step
  recovery, and structured transport validation contracts.

## [6.5.2] - 2026-07-28

### Changed

- Updated the bundled Core with strict built-in tool contracts, root JSON
  Schema value support, governed generation timeouts, and typed transport
  failures.

## [6.5.1] - 2026-07-28

### Changed

- Updated the bundled Core with the shared repository-tool prompt contract,
  neutral grep pagination compatibility, and real-model context-tool coverage.

## [6.5.0] - 2026-07-28

### Changed

- Updated the bundled Core with budgeted multi-file reads, compact grep result
  modes, explicit glob ordering, and previewable replacement-count-guarded
  edits.

## [6.4.3] - 2026-07-25

### Fixed

- Updated the bundled Core so Auto execution stays direct when structured
  pre-analysis is unavailable and explicit fallback plans contain the original
  request instead of fabricated numbered tasks.

## [6.4.2] - 2026-07-23

### Fixed

- Updated the bundled Core so managed macOS SRT hosts can pass large Seatbelt
  profiles by private file without exceeding the process argument-size limit.

## [6.4.1] - 2026-07-23

### Fixed

- Updated the bundled Core so SRT workspace policy scans tolerate files and
  directories removed concurrently by other workspace processes.

## [6.4.0] - 2026-07-22

### Changed

- Updated the bundled Core to Search 2.1 with structured engine failures,
  bounded automatic fallback, DuckDuckGo and Wikipedia defaults, and opt-in
  AnySearch selection.

## [6.3.1] - 2026-07-22

### Fixed

- Updated the bundled Core so managed SRT sandboxes start when protected
  workspace directories contain absent sensitive-file descendants.

## [6.3.0] - 2026-07-22

### Changed

- Updated the bundled Core with typed model-generation admission, durable
  memory observers, ordered extraction draining, and run-scoped permission and
  confirmation snapshots.

## [6.2.0] - 2026-07-22

### Changed

- Updated the bundled Core with the verified SRT process-sandbox contract and
  the latest delegated confirmation propagation fixes.

## [6.1.0] - 2026-07-20

### Changed

- Updated the bundled Core to `a3s-search` 2.0, with AnySearch as the default
  web-search provider and Tavily available as an explicit native provider.

## [6.0.0] - 2026-07-19

### Changed

- Updated the bundled Core with expanded TypeScript language profiles, PDF
  fetching, invariant-safe session forks, typed MCP results and artifacts,
  delegated permission boundaries, and tool-free standalone greetings.
- Raised the major version with Core's public standard MCP metadata fields.

## [5.3.5] - 2026-07-17

### Changed

- Updated the bundled Core with host-supplied session client bootstrap, bounded
  in-place response-stream retry and rollback, explicit search-routing
  metadata, and the Bing RSS engine.

## [5.3.4] - 2026-07-16

### Fixed

- Updated the bundled Core so a cold workspace-symbol query prepares a saved
  source document before asking the language server to search its projects.

## [5.3.3] - 2026-07-16

### Fixed

- Updated the bundled Core so abandoned semantic queries share cancellation-safe
  language-runtime startup and workspace shutdown remains bounded.

## [5.3.2] - 2026-07-16

### Fixed

- Updated the bundled Core with position-aware interactive shell risk
  classification and stricter detection of implicit file-writing options.

## [5.3.1] - 2026-07-16

### Fixed

- Updated the bundled Core so live worker registration is reflected in the
  model-facing delegation catalog and delegated sessions inherit live MCP
  additions and removals.

## [5.3.0] - 2026-07-15

### Added

- Added `Session.cancel_and_settle(...)` parity for bounded cooperative
  cancellation and streaming-worker cleanup before session reuse.

## [5.2.4] - 2026-07-14

### Fixed

- Updated the bundled Core to use token-budgeted rolling context compaction,
  including bounded summaries and verified prompt reduction.

## [5.2.3] - 2026-07-13

### Added

- Added `SessionOptions.max_context_tokens` parity with Rust Core and the Node
  SDK for model-aware automatic context compaction.
- Added `Agent.replace_session_async(...)` parity for atomic live-session
  reconfiguration without a closed-session gap on failure.

## [5.0.0] - 2026-07-11

### Added

- Added `StateGraphRuntime` parity for event-sourced graph patches, event-log
  restore, event-point forks, and structural diffs.

## [3.1.0] - 2026-05-23

### Added
- Added Python bindings for automatic subagent delegation configuration via
  `AutoDelegationConfig`, `SessionOptions.auto_delegation`,
  `SessionOptions.max_parallel_tasks`, and `SessionOptions.auto_parallel`.
- Added Python parity for direct worker/subagent APIs, including
  `WorkerAgentSpec`, `AgentDefinition`, `session_for_worker(...)`,
  `register_worker_agent(...)`, and `register_worker_agents(...)`.
- Added `task(...)`, `tasks(...)`, and `parallel_task(...)` helpers that call
  the core `task` / `parallel_task` tools.

### Changed
- Documented that automatic parallel fan-out can be globally disabled with
  `opts.auto_parallel = False` while manual `parallel_task` remains available.
- Updated examples around disposable worker agents and bounded parallel
  delegation.

## [2.3.0] - 2026-05-09

### Added
- Added explicit `planning_mode="auto" | "enabled" | "disabled"` while keeping
  the legacy `planning` boolean shortcut.
- Added compact APIs: `send({...})`, `run(...)`, `stream({...})`, `task({...})`,
  `tasks([...])`, `git({...})`, `add_mcp({...})`, `remove_mcp(...)`, and
  `mcps()`.
- Added `delegate_task(...)`, `parallel_task(...)`, and `tool_definitions()`
  compatibility APIs to mirror the Rust core and Node SDK surfaces.
- Added Python parity for `WorkerAgentSpec`, `AgentDefinition`,
  `ConfirmationPolicy`, `session_for_worker(...)`, live worker registration,
  HITL confirmation control, and `Session.close()`.

### Changed
- New documentation prefers short object-shaped APIs while keeping older
  positional and long-form APIs for compatibility.

## [2.0.0] - 2026-05-02

### Changed
- Reframed the primary SDK path around `Agent` and workspace-bound sessions.
- Made `task` / `parallel_task` the single recommended multi-agent delegation
  path for new integrations.
- Removed the standalone 1.x lifecycle control-plane API from the 2.0 SDK
  surface; routine delegation uses `task` / `parallel_task`.
- Clarified that lane queues are optional external/hybrid dispatch
  infrastructure, not a generic task submission API.

### Removed
- Removed 1.0-era public task/progress/idle lifecycle bindings from the SDK.
- Removed generic session queue submission helpers from the public SDK surface.
- Removed duplicate team shortcuts, lifecycle control bindings, and
  unified slot APIs.
- Removed explicit Lead/Worker/Reviewer team-runner APIs.
- Removed placeholder delegated-agent execution and standalone child-run handle/event
  bindings.

## [1.4.5] - 2026-03-16

Historical note: this section records pre-2.0 investigation work.

### Added
- Support for `kind: tool` in skill system (in addition to `instruction` and `persona`)
- Complete PyO3 bindings for pre-2.0 delegated-agent configuration fields
- Comprehensive integration tests with real LLM execution

### Fixed
- Delegated-agent `skill_dirs` parameter now properly passed from Python to Rust layer
- Skill registry now correctly filters and loads tool-type skills
- All delegated-agent configuration fields now have proper getter/setter methods

### Changed
- Reorganized test structure: `tests/unit/` for mock tests, `tests/integration/` for real API tests
- Removed hardcoded API keys from test files (now use environment variables)

### Security
- All test files now use environment variables for API credentials
- Added `.gitignore` to prevent accidental credential commits

## [1.4.4] - 2026-03-15

Historical note: this was an early Python SDK release before the 2.0 API
boundary.

### Added
- Initial Python SDK release with PyO3 bindings
- Basic agent and session management
- Skill system support
- Early lifecycle control-plane and delegated-agent execution support
