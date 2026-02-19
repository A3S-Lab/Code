# A3S Code Improvement Plan

## Current State Summary

- 5 core components, 14 extension points, 11 built-in tools
- 1133 tests passing, 3 SDKs (Rust/Python/Node.js)
- Lane-based priority queue with external task distribution
- Security, skills, planning, hooks all implemented and tested

---

## Phase 1: Memory System — File-Based Default Backend

**Priority: High | Effort: Medium | Impact: High**

### Problem

`MemoryStore` trait is defined with 9 methods (store, retrieve, search, search_by_tags, get_recent, get_important, delete, clear, count), but:

1. No production-ready default implementation — only `TestMemoryStore` in test code (in-memory `Mutex<Vec<MemoryItem>>`)
2. `AgentMemory` is not wired into `AgentSession` or `SessionOptions` — no `with_memory()` builder
3. `MemoryContextProvider` exists but can't be used without a real store
4. Memory is lost on process restart

### Solution

#### 1.1 Implement `FileMemoryStore`

Follow the `FileSessionStore` pattern in `store.rs`:

```
memory/
  {session_id}/
    {memory_id}.json     # Individual memory items
    index.json           # Lightweight index for fast search (tags, timestamps, importance)
```

Key design decisions:
- One JSON file per memory item (atomic writes, easy cleanup)
- `index.json` holds a compact index: `{ id, content_preview, tags, importance, timestamp, memory_type }` for each item
- Index loaded into memory on init for fast `search()` and `search_by_tags()`
- Full content loaded on-demand from individual files
- Atomic writes via temp file + rename (same as `FileSessionStore`)
- Path traversal prevention on memory IDs

```rust
pub struct FileMemoryStore {
    dir: PathBuf,
    index: RwLock<MemoryIndex>,
}

struct MemoryIndex {
    items: Vec<MemoryIndexEntry>,
}

struct MemoryIndexEntry {
    id: String,
    content_lower: String,  // For substring search
    tags: Vec<String>,
    importance: f32,
    timestamp: DateTime<Utc>,
    memory_type: MemoryType,
}
```

#### 1.2 Wire `AgentMemory` into `SessionOptions` and `AgentSession`

```rust
// SessionOptions builder
impl SessionOptions {
    pub fn with_memory(mut self, store: Arc<dyn MemoryStore>) -> Self { ... }
    pub fn with_file_memory(mut self, dir: impl Into<PathBuf>) -> Self { ... }
}

// AgentSession gains memory field
pub struct AgentSession {
    memory: Option<AgentMemory>,
    // ...
}
```

#### 1.3 Auto-remember in AgentLoop

After each successful tool execution turn:
- `remember_success()` for successful patterns
- `remember_failure()` for errors
- Inject relevant memories via `MemoryContextProvider` into system prompt

#### 1.4 SDK Bindings

Expose in Python and Node.js SDKs:
- `SessionOptions.memory_dir` / `memoryDir` — path to file-based memory store
- `session.remember(content, tags, importance)` — manual memory storage
- `session.recall(query, limit)` — manual memory recall

#### 1.5 Tests

- `FileMemoryStore` unit tests (store, retrieve, search, delete, clear, index rebuild)
- Integration test: memory persists across session restarts
- Concurrent access test (multiple readers/writers)
- Index corruption recovery test

---

## Phase 2: Session Persistence Integration

**Priority: High | Effort: Low | Impact: High**

### Problem

`FileSessionStore` and `SessionStore` trait exist and work, but `AgentSession` doesn't expose save/load/resume APIs. Sessions are always created fresh.

### Solution

#### 2.1 Session Resume API

```rust
impl Agent {
    pub fn resume_session(&self, session_id: &str, opts: Option<SessionOptions>) -> Result<AgentSession>;
}

impl AgentSession {
    pub async fn save(&self) -> Result<()>;
    pub fn session_id(&self) -> &str;
}
```

#### 2.2 Auto-Save

Optional auto-save after each turn:
```rust
SessionOptions::new().with_auto_save(true)
```

#### 2.3 SDK Bindings

- `agent.resume_session(id)` in all 3 SDKs
- `session.save()` in all 3 SDKs

---

## Phase 3: Streaming SDK Experience

**Priority: Medium | Effort: Medium | Impact: Medium**

### Problem

- Node.js `stream()` collects all events then returns array — not truly streaming
- Python `stream()` uses synchronous iterator + thread — not async native

### Solution

#### 3.1 Node.js: True Async Iterator

```typescript
const stream = session.stream("prompt");
for await (const event of stream) {
  if (event.type === "text_delta") process.stdout.write(event.text);
}
```

Implement via napi-rs `AsyncIterator` or `ReadableStream`.

#### 3.2 Python: Async Generator

```python
async for event in session.stream("prompt"):
    if event.type == "text_delta":
        print(event.text, end="")
```

Implement via PyO3 `__aiter__` / `__anext__` protocol.

---

## Phase 4: Error Recovery & Resilience

**Priority: Medium | Effort: Medium | Impact: Medium**

### Problem

- LLM returns invalid JSON → agent loop fails
- Tool execution timeout → no automatic retry/degradation
- No circuit breaker for repeated LLM failures

### Solution

#### 4.1 LLM Response Recovery

- Parse error → retry with "Your previous response was malformed, please try again"
- Max 2 retries before surfacing error

#### 4.2 Tool Execution Timeout

- Per-tool timeout configuration
- Timeout → return error result to LLM (not crash)
- LLM can decide to retry or use alternative approach

#### 4.3 Circuit Breaker

- Track consecutive LLM failures per session
- After N failures, pause and surface error to caller
- Configurable via `SessionOptions`

---

## Phase 5: A3S Box Sandbox Integration

**Priority: Low | Effort: High | Impact: High**

### Problem

`bash` tool executes commands directly on host. No isolation for untrusted code.

### Solution

#### 5.1 Sandboxed Bash Tool

```rust
SessionOptions::new()
    .with_sandbox(SandboxConfig {
        backend: SandboxBackend::Box,  // Use A3S Box
        image: "ubuntu:22.04",
        memory_limit: "512m",
        network: false,
    })
```

When sandbox is enabled, `bash` tool routes commands through A3S Box instead of `std::process::Command`.

#### 5.2 File System Isolation

Sandboxed sessions mount workspace as read-write volume. All file operations go through the sandbox.

---

## Phase 6: Multi-Modal Support

**Priority: Low | Effort: High | Impact: Medium**

### Problem

Only text input/output. No image/file attachment support.

### Solution

- Support image attachments in `send()` / `stream()`
- Support image output from tools (screenshots, diagrams)
- Requires LLM provider support (Claude vision, GPT-4V)

---

## Priority Matrix

| Phase | Priority | Effort | Impact | Dependencies |
|-------|----------|--------|--------|-------------|
| 1. Memory (File-Based) | 🔴 High | Medium | High | None |
| 2. Session Persistence | 🔴 High | Low | High | None |
| 3. Streaming SDK | 🟡 Medium | Medium | Medium | None |
| 4. Error Recovery | 🟡 Medium | Medium | Medium | None |
| 5. Sandbox Integration | 🟢 Low | High | High | A3S Box |
| 6. Multi-Modal | 🟢 Low | High | Medium | LLM providers |

**Recommended execution order:** Phase 1 → Phase 2 → Phase 4 → Phase 3 → Phase 5 → Phase 6

Phase 1 and 2 can be done in parallel. Phase 4 is independent. Phase 3 requires SDK build toolchain changes. Phase 5 and 6 are longer-term.
