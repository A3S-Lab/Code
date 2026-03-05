# Changelog

All notable changes to `a3s-code-core` are documented in this file.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.0.0/).
This project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

---

## [1.1.0] - 2026-03-05

### New Features

- **Real-time Monitoring API** — Complete task list and activity tracking for SubAgents
  - `list_subagents()` — Get all SubAgent information with metadata and current activity
  - `get_subagent_info(id)` — Query specific SubAgent details
  - `get_active_activities()` — Get all active SubAgent activities
  - `get_all_states()` — Get all SubAgent states
  - `pause_subagent(id)` / `resume_subagent(id)` / `cancel_subagent(id)` — Control operations
  - `wait_all()` — Wait for all SubAgents to complete
- **SubAgent Activity Tracking** — 4 activity types with real-time updates
  - `Idle` — SubAgent is idle
  - `CallingTool` — Calling a tool (with tool name and arguments)
  - `RequestingLlm` — Requesting LLM (with message count)
  - `WaitingForControl` — Waiting for control signal (with reason)
- **SubAgentInfo Structure** — Complete metadata for each SubAgent
  - ID, type, description, state, parent_id
  - Created/updated timestamps
  - Current activity information
- **Python SDK** — Full monitoring API support
  - All 11 monitoring APIs implemented
  - Complete type definitions with activity tracking
- **Node.js SDK** — Full monitoring API support
  - All 11 monitoring APIs implemented
  - TypeScript definitions with JSDoc documentation
  - Activity entry types for tuple returns

### Improvements

- **Documentation** — Updated orchestrator docs with monitoring API examples
  - Added real-time monitoring section in both CN and EN docs
  - Added activity types reference table
  - Added monitoring API list with descriptions
- **Examples** — Added real-world test examples
  - `test_real_kimi.py` / `test_real_kimi.ts` — Full-featured tests with Kimi API
  - `test_simple_fixed.py` — Simplified test demonstrating all APIs
  - `test_apis.py` / `test_apis.ts` — API validation scripts
- **Test Coverage** — All monitoring APIs tested and verified
  - 11/11 core APIs working
  - 4/4 activity types detected
  - Real-time state and activity updates verified

### Documentation

- Added `API_REFERENCE.md` — Quick reference for monitoring APIs
- Added `ORCHESTRATOR_MONITORING.md` — Complete usage guide (400+ lines)
- Added `REAL_TEST_REPORT.md` — Actual test results with Kimi API
- Added `TEST_REPORT.md` — Test status summary
- Updated architecture documentation with monitoring features

## [1.0.4] - 2026-03-05

### New Features

- **Agent Orchestrator** — Main-sub agent coordinator with real-time monitoring and dynamic control
  - Event-driven architecture based on a3s-event
  - Real-time monitoring of all SubAgent behaviors, planning, and execution
  - Dynamic control: pause, resume, cancel, adjust parameters, inject prompts
  - 11 event types: started, completed, state_changed, progress, tool_execution, control_signal, etc.
  - Pluggable communication: default memory-based, supports custom NATS provider
  - Concurrency management with configurable limits
- **Python SDK Orchestrator bindings** — `Orchestrator`, `SubAgentConfig`, `SubAgentHandle` classes
  - Full API support for spawning, monitoring, and controlling SubAgents
  - Event subscription and filtering
  - State management and result collection

### Improvements

- **Test coverage** — Added 7 orchestrator unit tests (1480 total tests passing)
- **Documentation** — Added comprehensive orchestrator documentation in English and Chinese
- **Examples** — Added Python examples for orchestrator usage and integration testing

---

## [1.0.2] - 2026-03-04

### New Features

- **HCL `env()` function** — `env("VAR")` resolves environment variables at parse time; returns null if unset.
- **HCL verbatim blocks** — `env {}` and `headers {}` blocks preserve keys verbatim (no camelCase conversion).
- **Memory tier APIs** — `getWorking()`, `clearWorking()`, `getShortTerm()`, `clearShortTerm()` exposed in Node.js and Python SDKs.
- **Queue metrics** — `queueMetrics()` returns a `MetricsSnapshot` with counters, gauges, and histograms.

### Improvements

- **Security hardening** — Enhanced default redaction patterns and robust regex in grep/ls tools.
- **Windows path compatibility** — `canonicalize()` now strips `\\?\` UNC prefix on Windows.
- **RwLock poison recovery** — `read_or_recover()` / `write_or_recover()` prevent panics from poisoned locks.
- **Node.js TypeScript definitions** — Added 13 missing method declarations to `index.d.ts` (lane queue, memory API, session metadata).
- **Documentation** — All example pages updated: Python sync API, TypeScript code blocks, correct event field names.

---

## [1.0.1] - 2026-03-03

### New Features

- **MCP live tool injection** — Agent loop refreshes tool list from live `ToolExecutor` each turn.
- **MCP error tracking** — `McpManager.connect_errors` tracks per-server failures, exposed in `mcpStatus()`.
- **SDK introspection** — `toolNames()`, `toolDefinitions()`, `refreshMcpTools()` in both Node.js and Python SDKs.
- **MCP removal** — `removeMcpServer()` for session-level MCP cleanup.

### Bug Fixes

- **Node.js stream fix** — `stream()` return type corrected to `Promise<EventStream>` in TypeScript definitions.

---

## [1.0.0] - 2026-03-02

### New Features

- **MCP OAuth support** — HTTP-based MCP servers now support Bearer token authentication.
  Static tokens (`access_token`) and OAuth 2.0 client credentials flow are both supported.
  Configure via `OAuthConfig` in `McpServerConfig`.

- **Extended Thinking** — `SessionOptions::with_thinking_budget(tokens)` enables Claude's
  extended thinking mode for complex reasoning tasks. See `examples/extended_thinking.rs`.

- **WriteTool diff metadata** — The `write` tool now attaches `before`/`after` content as
  metadata in `ToolResult`, enabling callers (e.g., UI streaming) to render diffs.

- **Batch tool large-scale support** — Validated parallel execution of 150+ concurrent tasks
  with correct result aggregation.

### Improvements

- **Session Resume edge cases** — `FileSessionStore` now handles corrupted JSON, empty files,
  and concurrent saves (using unique temp file names to prevent race conditions).

- **Context compression edge cases** — Multiple sequential compactions and tool results added
  after compaction are now fully tested and handled correctly.

- **Concurrent save safety** — Fixed a race condition in `FileSessionStore::save()` where two
  concurrent saves to the same session could fail with "No such file or directory" due to
  colliding temporary file names.

### Breaking Changes

- **`SkillKind::Tool` and `SkillKind::Agent` removed** — These variants were declared but never
  implemented, creating a misleading public API. Remove any code referencing these variants;
  use `SkillKind::Instruction` or `SkillKind::Persona` instead.

- **HCL-only configuration** — JSON configuration file support has been fully removed.
  All `.a3s/config.hcl` files must use valid HCL syntax. Use `providers {}` blocks (not
  `provider "name" {}`) — see `agent.example.hcl` for the correct format.

### Bug Fixes

- Fixed `ToolExecutor` tool count test assertions for `sandbox` feature flag — tests now
  correctly expect 13 tools when built with `--features sandbox`.

- Fixed `test_mcp_parallel_fix` example unused import warning (`McpServerConfig`,
  `McpTransportConfig`).

- Fixed rustdoc bare URL warning in `mcp/protocol.rs`.

### Test Coverage

- Added 46+ new tests across MCP OAuth, extended thinking, session resume, batch operations,
  and context compression.
- Total: **1467 unit tests** + **25 integration tests** (2 require live API keys).
- All tests pass with `cargo test --all-features`.

---

## [0.9.5] - 2025-02-15

- Session context compaction (LLM-assisted summarization)
- Parallel task execution (`ParallelTaskTool`)
- Git worktree tool (`git_worktree`)
- MCP streamable HTTP transport
- Tool permission system with guard policy
- File history / undo support for write/edit/patch tools
- Skill system (Instruction + Persona kinds)
- Hook system for agent lifecycle events

---

## [0.9.0] - 2025-01-10

- Initial public release
- Core agent loop with tool execution
- Built-in tools: bash, read, write, edit, grep, glob, ls, patch, web_fetch, web_search
- Anthropic + OpenAI provider support
- MCP client (stdio + HTTP SSE transports)
- Session persistence (JSON file store)
- HCL configuration format
