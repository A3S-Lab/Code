# Query-Lane Tool Parallelization

A3S Code provides Query-lane tool parallelization, allowing parallel execution when a single LLM turn returns multiple query-type tools. This significantly improves performance for network I/O and bulk query operations.

## Quick Start

### Enabling Parallelization

Parallelization is **opt-in** — you must explicitly enable it via `SessionQueueConfig.enable_parallelization`.

**Rust:**
```rust
use a3s_code_core::{Agent, SessionOptions, SessionQueueConfig};
use a3s_code_core::queue::ParallelizationStrategy;

let mut queue_config = SessionQueueConfig::default();
queue_config.enable_parallelization = true;  // OPT-IN
queue_config.query_max_concurrency = 10;

// Optional: customize strategy
let mut strategy = ParallelizationStrategy::default();
strategy.min_tool_count = 3;  // Default: 8
strategy.allowed_tools = vec!["web_fetch".into(), "web_search".into()];
queue_config.parallelization_strategy = Some(strategy);

let session = agent.session(
    ".",
    Some(SessionOptions::default().with_queue_config(queue_config))
)?;
```

**Python:**
```python
from a3s_code import Agent, SessionOptions, SessionQueueConfig, ParallelizationStrategy

queue_config = SessionQueueConfig()
queue_config.enable_parallelization = True
queue_config.set_query_concurrency(10)

strategy = ParallelizationStrategy()
strategy.min_tool_count = 3
strategy.allowed_tools = ["web_fetch", "web_search"]
queue_config.parallelization_strategy = strategy

options = SessionOptions()
options.queue_config = queue_config
session = agent.session(".", options)
```

**Node.js:**
```javascript
const { Agent } = require('@a3s-lab/code');

const agent = await Agent.create('config.hcl');
const session = agent.session('.', {
  queueConfig: {
    enableParallelization: true,
    queryConcurrency: 10,
    parallelizationStrategy: {
      minToolCount: 3,
      allowedTools: ['web_fetch', 'web_search'],
    },
  },
});
```

## How It Works

### Parallelization Trigger Conditions

Parallelization triggers only when **all** conditions are met:

1. ✅ `enable_parallelization = true` (opt-in)
2. ✅ Queue is configured (`SessionQueueConfig` provided)
3. ✅ Single LLM turn returns >= `min_tool_count` Query-lane tools (default: 8)
4. ✅ Tools are in the allowed list (if specified) and not in the blocked list

### Query-Lane Tools

| Tool | Lane | Parallelizable |
|------|------|---------------|
| `read` | Query | ✅ |
| `glob` | Query | ✅ |
| `grep` | Query | ✅ |
| `ls` | Query | ✅ |
| `search` | Query | ✅ |
| `list_files` | Query | ✅ |
| `web_fetch` | Query | ✅ |
| `web_search` | Query | ✅ |
| `bash` | Execute | ❌ (sequential) |
| `write` | Execute | ❌ (sequential) |
| `edit` | Execute | ❌ (sequential) |

### Execution Flow

```
LLM call → returns 10 web_fetch tools
           ↓
    [Parallelization check]
           ↓
    enable_parallelization? ✅
    >= min_tool_count?      ✅
    Query-lane tools?       ✅
    In allowed_tools?       ✅
           ↓
    [Parallel execution]
    fetch(url1)  ┐
    fetch(url2)  ├─ concurrent execution
    fetch(url3)  ┘
    ...
           ↓
    Collect results → return to LLM
```

## Performance

### Network I/O (recommended)

| Operation | Serial | Parallel | Speedup |
|-----------|--------|----------|---------|
| 10x web_fetch | ~17s | ~1.8s | **~9x** |
| 20x web_search | ~60s | ~8s | **~7.5x** |
| 15x API calls | ~45s | ~6s | **~7.5x** |

### Local File I/O (not recommended)

| Operation | Serial | Parallel | Speedup |
|-----------|--------|----------|---------|
| 10x read | 0.1s | 0.5s | **0.2x** ❌ |
| 20x glob | 0.2s | 0.8s | **0.25x** ❌ |

**Conclusion: parallelization is best for slow I/O, not fast local operations.**

## ParallelizationStrategy

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `min_tool_count` | usize | 8 | Minimum Query-lane tools to trigger parallelization |
| `allowed_tools` | Vec&lt;String&gt; | [] (all) | Only parallelize these tools. Empty = all Query-lane tools |
| `blocked_tools` | Vec&lt;String&gt; | [] | Never parallelize these tools. Takes precedence over allowed |

## Use Cases

### ✅ Good for parallelization

**1. Network requests**
```rust
session.send(
    "Fetch these 10 web pages and extract titles:
     1. https://example.com/page1
     2. https://example.com/page2
     ...",
    None
).await?;
```

**2. Bulk document search**
```python
result = session.send(
    "Search for 'authentication' in these 15 files:
     docs/api.md, docs/security.md, ..."
)
```

**3. Batch API calls**
```javascript
const result = await session.send(
    "Fetch user info for IDs: 1, 2, 3, ..., 20\n" +
    "Use web_fetch to call /api/users/{id}"
);
```

### ❌ Not recommended

**1. Local file reads** — too fast, parallelization overhead > benefit
**2. Few tool calls** — below `min_tool_count` threshold
**3. Fast operations** — glob/ls complete in milliseconds

## Best Practices

### 1. Choose based on scenario

```rust
// Network requests → enable parallelization
let session_web = agent.session(".", Some(
    SessionOptions::default().with_queue_config(queue_config)
))?;

// Local files → skip parallelization
let session_local = agent.session(".", None)?;
```

### 2. Monitor with logs

```bash
RUST_LOG=info cargo run
```

Look for:
```
[INFO] Using parallel execution for Query-lane tools [tool_count=10]  ← enabled
[INFO] Parallel execution bypassed: too few tools                     ← not triggered
[INFO] Parallel execution bypassed: not enabled                       ← not opt-in
```

## Performance Tests

Run the test examples to see actual performance:

```bash
# Rust
cd crates/code
cargo run --example test_internal_parallel

# Python
cd crates/code/sdk/python
python3 examples/test_internal_parallel.py

# Node.js
cd crates/code/sdk/node
node examples/test_internal_parallel.js
```

**Test scenario:** Fetch 10 web page titles

**Expected results:**
- Serial: ~17s tool execution
- Parallel: ~1.8s tool execution
- Speedup: ~9x for tool execution

## External Task Handler Tests

For Multi-Machine External Task processing (coordinator/worker pattern), see:

```bash
# Rust
cd crates/code
cargo run --example test_external_task_handler

# Python
cd crates/code/sdk/python
python3 examples/test_external_task_handler.py

# Node.js
cd crates/code/sdk/node
node examples/test_external_task_handler.js
```

These tests demonstrate:
1. Execute lane → External mode (tasks routed to external handler)
2. Hybrid mode (local execution + external notification)
3. Dynamic lane switching at runtime
