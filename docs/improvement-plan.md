# A3S Code Improvement Plan

## Current State Summary

- 5 core components, 14 extension points, 11 built-in tools
- 1422 tests passing, 3 SDKs (Rust/Python/Node.js) + CLI
- Lane-based priority queue with external task distribution
- Security, skills, planning, hooks, memory, session persistence all implemented
- Slash commands, tool search, agent teams, multi-modal attachments

---

## Phase 1: Memory System — File-Based Default Backend ✅

**Status: Complete**

Implemented:
- `FileMemoryStore` with JSON file-per-item storage and in-memory index
- `AgentMemory` wired into `AgentSession` via `SessionOptions::with_file_memory()`
- Auto-recall in `AgentLoop` (injects relevant memories into system prompt)
- Auto-remember after tool execution (`remember_success` / `remember_failure`)
- SDK bindings: `remember_success`, `remember_failure`, `recall_similar`, `recall_by_tags`, `memory_recent`, `memory_stats`, `has_memory` in Python and Node.js
- Full test coverage

---

## Phase 2: Session Persistence Integration ✅

**Status: Complete**

Implemented:
- `Agent::resume_session(session_id, opts)` for restoring saved sessions
- `AgentSession::save()` for manual persistence
- `SessionOptions::with_file_session_store()` builder
- SDK bindings: `resume_session()` and `save()` in Python and Node.js
- Session metadata accessors: `session_id`, `workspace`, `init_warning`

---

## Phase 3: Streaming SDK Experience ✅

**Status: Complete**

Implemented:
- Python: `EventStream` with `__iter__` / `__next__` synchronous iteration
- Node.js: `EventStream` with `next()` async method and `collect()` convenience
- Both SDKs support streaming with tool events (ToolStart, ToolEnd, TextDelta)

---

## Phase 4: Error Recovery & Resilience ✅

**Status: Complete**

Implemented:
- 4.1 LLM parse error recovery: `__parse_error` fed back to LLM, max retries configurable
- 4.2 Per-tool execution timeout via `SessionOptions::with_tool_timeout()`
- 4.3 Circuit breaker: consecutive LLM failure threshold via `SessionOptions::with_circuit_breaker()`
- Continuation injection for incomplete responses (configurable)

---

## Phase 5: A3S Box Sandbox Integration ✅

**Status: Complete**

Implemented:
- `SessionOptions::with_sandbox(SandboxConfig)` builder
- `SandboxConfig` with backend, image, memory limit, network toggle
- Sandbox-aware bash tool routing
- Full test coverage

---

## Phase 6: Multi-Modal Support ✅

**Status: Complete**

Implemented:
- `Attachment` type (raw bytes + media type) for image input
- `send_with_attachments()` / `stream_with_attachments()` on `AgentSession`
- SDK bindings in Python (dict-based) and Node.js (Buffer-based)
- Supports JPEG, PNG, GIF, WebP

---

## Phase 7: SDK Completion + Tool Search Integration ✅

**Status: Complete**

Implemented:
- **Slash Commands**: `/help`, `/compact`, `/cost`, `/model`, `/clear`, `/history`, `/tools` — dispatched before LLM in `AgentSession::send()` and `stream()`
- **Tool Search**: `ToolIndex` keyword-based filtering integrated into `AgentLoop::call_llm()` — filters tools per-turn based on user prompt, reducing context usage with large MCP tool sets
- **Agent Teams**: `AgentTeam` with `TeamTaskBoard` (post/claim/complete/approve/reject workflow), `TeamRole` (Lead/Worker/Reviewer), peer-to-peer `mpsc` messaging and broadcast
- **CLI**: `a3s-code` binary with interactive REPL and one-shot modes, config auto-discovery chain
- **SDK Alignment**: `resume_session`, `send_with_attachments`, `stream_with_attachments` added to both Python and Node.js SDKs
- **SessionManager Memory**: Shared `MemoryStore` injected into `AgentConfig.memory` for `generate`/`generate_streaming`

---

## Priority Matrix

| Phase | Status | Effort | Impact |
|-------|--------|--------|--------|
| 1. Memory (File-Based) | ✅ Done | Medium | High |
| 2. Session Persistence | ✅ Done | Low | High |
| 3. Streaming SDK | ✅ Done | Medium | Medium |
| 4. Error Recovery | ✅ Done | Medium | Medium |
| 5. Sandbox Integration | ✅ Done | High | High |
| 6. Multi-Modal | ✅ Done | High | Medium |
| 7. SDK + Tool Search | ✅ Done | Medium | High |

All planned phases are complete. Future work should focus on production hardening, performance optimization, and ecosystem expansion (more MCP servers, more builtin tools).
