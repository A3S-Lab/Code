# Changelog

All notable changes to `a3s-code-core` are documented in this file.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.0.0/).
This project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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
