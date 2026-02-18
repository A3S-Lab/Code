# A3S Code Integration Tests

This directory contains comprehensive integration tests for all A3S Code v0.8.0 features using real LLM configurations.

## Prerequisites

### 1. Configuration File

Tests require a valid configuration file at one of these locations:
- `~/.a3s/config.hcl` (recommended)
- `<project_root>/.a3s/config.hcl`

**Example configuration:**

```hcl
default_model = "openai/kimi-k2.5"

providers {
  name = "openai"

  models {
    id          = "kimi-k2.5"
    name        = "KIMI K2.5"
    family      = "kimi"
    api_key     = "your-api-key-here"
    base_url    = "https://api.openai.com/v1"
    attachment  = false
    reasoning   = false
    tool_call   = true
    temperature = true

    modalities {
      input  = ["text"]
      output = ["text"]
    }

    limit {
      context = 128000
      output  = 4096
    }
  }
}
```

### 2. Dependencies

All dependencies are already included in `Cargo.toml`:
- `a3s-code-core` (main library)
- `tokio` (async runtime)
- `anyhow` (error handling)
- `tracing` (logging)
- `dirs` (home directory detection)

## Available Tests

### 1. `integration_tests.rs` - Complete Feature Test Suite

Tests all major features in one comprehensive run.

**Features tested:**
- ✅ Basic tool execution (ls, read, write, edit)
- ✅ Built-in skills (all 7 skills)
- ✅ File operations (create, read, edit, delete)
- ✅ Search operations (grep, glob)
- ✅ Web search (if configured)
- ✅ Planning mode (multi-step tasks)
- ✅ Queue configuration (A3S Lane v0.4.0)

**Run:**
```bash
cargo run --example integration_tests
```

**Expected output:**
```
🚀 A3S Code Integration Tests
================================================================================
📄 Using config: /Users/you/.a3s/config.hcl
================================================================================

📦 Test 1: Basic Tool Execution
--------------------------------------------------------------------------------
Testing: List current directory...
✓ Result preview: ...
...

✅ All integration tests completed successfully!
```

---

### 2. `test_lane_features.rs` - A3S Lane v0.4.0 Advanced Features

Tests all advanced queue features introduced in A3S Lane v0.4.0.

**Features tested:**
- ✅ Retry policies (exponential/fixed backoff)
- ✅ Rate limiting (per-second/minute/hour)
- ✅ Priority boost (standard/aggressive)
- ✅ Pressure monitoring (threshold alerts)
- ✅ Per-lane timeouts (custom timeouts per lane)
- ✅ Combined features (all features together)

**Run:**
```bash
cargo run --example test_lane_features
```

**Expected output:**
```
🚀 A3S Lane v0.4.0 Advanced Features Test
================================================================================

🔄 Test 1: Retry Policy
--------------------------------------------------------------------------------
Testing: Exponential backoff retry...
✓ Exponential backoff: ...
...

✅ All A3S Lane v0.4.0 features tested successfully!
```

---

### 3. `test_search_config.rs` - A3S Search v0.7.0 Configuration

Tests the configurable web_search tool with different engine configurations.

**Features tested:**
- ✅ Default search configuration (ddg, wiki)
- ✅ Custom search configuration (timeout, engines, weights)
- ✅ Engine enable/disable control
- ✅ Health monitoring configuration

**Run:**
```bash
cargo run --example test_search_config
```

**Expected output:**
```
🚀 A3S Search v0.7.0 Configuration Test
================================================================================

🔍 Test 1: Default Search Configuration
--------------------------------------------------------------------------------
Testing: Web search with default engines (ddg,wiki)...
✓ Default search works
...

✅ All search configuration tests completed!
```

**Note:** Web search tests may fail if search engines are unavailable or blocked. This is expected behavior.

---

### 4. `test_builtin_skills.rs` - All 7 Built-in Skills

Tests each of the 7 built-in skills individually.

**Skills tested:**

**Code Assistance (4):**
1. ✅ `code-search` - Search codebase for patterns
2. ✅ `code-review` - Review code for best practices
3. ✅ `explain-code` - Explain how code works
4. ✅ `find-bugs` - Identify potential bugs

**Tool Documentation (3):**
5. ✅ `builtin-tools` - Documentation for built-in tools
6. ✅ `delegate-task` - Task delegation guide
7. ✅ `find-skills` - Skill discovery and installation

**Run:**
```bash
cargo run --example test_builtin_skills
```

**Expected output:**
```
🚀 Testing All 7 Built-in Skills
================================================================================

📚 Code Assistance Skills (4)
================================================================================

🔍 Skill 1: code-search
--------------------------------------------------------------------------------
Description: Search codebase for patterns, functions, or types
Allowed tools: read(*), grep(*), glob(*)
✓ Result preview: ...
�� code-search skill works correctly
...

✅ All 7 built-in skills tested successfully!
```

---

### 5. `test_task_priority.rs` - Task Priority Scheduling

Tests A3S Lane's priority system to control task execution order. Demonstrates how tasks submitted later with higher priority execute before earlier tasks with lower priority.

**Features tested:**
- ✅ Basic priority ordering (submit in reverse order, execute in priority order)
- ✅ Late high-priority task preemption (urgent task jumps queue)
- ✅ Mixed priority workload (critical → normal → background)
- ✅ Real LLM execution with priority control

**Run:**
```bash
cargo run --example test_task_priority
```

**Expected output:**
```
🚀 A3S Code - Task Priority Test with Real LLM
================================================================================
📄 Using config: /Users/you/.a3s/config.hcl
================================================================================

📋 Test 1: Basic Priority Ordering
--------------------------------------------------------------------------------
Scenario: Submit 4 tasks in reverse priority order
Expected: Tasks execute in priority order (0 → 1 → 2 → 3)

Submitting tasks in reverse priority order...
[  0.00s] Submitted: Task 4 (priority 3 - lowest)
[  0.05s] Submitted: Task 3 (priority 2)
[  0.10s] Submitted: Task 2 (priority 1)
[  0.15s] Submitted: Task 1 (priority 0 - highest)
...

🚨 Test 2: Late High-Priority Task Preemption
--------------------------------------------------------------------------------
Scenario: Queue 3 low-priority tasks, then submit 1 urgent high-priority task
Expected: High-priority task executes before queued low-priority tasks

Step 1: Submitting 3 low-priority background tasks...
  ✓ Submitted: Background task 1 (list .md files)
  ✓ Submitted: Background task 2 (count .rs files)
  ✓ Submitted: Background task 3 (find TODOs)

Step 2: Submitting URGENT high-priority task...
  🚨 Submitted: URGENT task (read Cargo.toml)
...

🎯 Test 3: Mixed Priority Workload with Real LLM
--------------------------------------------------------------------------------
Scenario: Realistic workload with multiple priority levels
Expected: Critical tasks execute first, then normal, then background

📦 Background tasks:
  - Find all .toml files
  - List all directories

📋 Normal priority tasks:
  - Read README.md
  - Search for 'async'

🚨 Critical tasks:
  - Read Cargo.toml (critical)
...

✅ All task priority tests completed successfully!
```

**Use cases:**
- **Critical operations**: System health checks, security scans
- **Normal operations**: User requests, data processing
- **Background operations**: Cleanup, indexing, analytics

**Priority levels** (A3S Lane default lanes):
- Priority 0 (highest): `system` lane - Critical system operations
- Priority 1: `control` lane - Control plane operations
- Priority 2: `query` lane - Query operations (read-only)
- Priority 3: `session` lane - Session management
- Priority 4: `execute` lane - Execute operations (write)
- Priority 5 (lowest): `prompt` lane - LLM prompt processing

---

## Running All Tests

To run all integration tests sequentially:

```bash
# Run all tests
cargo run --example integration_tests && \
cargo run --example test_lane_features && \
cargo run --example test_search_config && \
cargo run --example test_builtin_skills && \
cargo run --example test_task_priority
```

Or create a shell script:

```bash
#!/bin/bash
# run_all_tests.sh

echo "Running all A3S Code integration tests..."
echo ""

echo "1. Integration Tests"
cargo run --example integration_tests
echo ""

echo "2. Lane Features Tests"
cargo run --example test_lane_features
echo ""

echo "3. Search Config Tests"
cargo run --example test_search_config
echo ""

echo "4. Built-in Skills Tests"
cargo run --example test_builtin_skills
echo ""

echo "5. Task Priority Tests"
cargo run --example test_task_priority
echo ""

echo "All tests completed!"
```

```bash
chmod +x run_all_tests.sh
./run_all_tests.sh
```

---

## Troubleshooting

### Config file not found

**Error:**
```
Config file not found. Please create ~/.a3s/config.hcl
```

**Solution:**
1. Create `~/.a3s/config.hcl` with your LLM configuration
2. Or copy the project's `.a3s/config.hcl` to your home directory:
   ```bash
   mkdir -p ~/.a3s
   cp .a3s/config.hcl ~/.a3s/
   ```

### API key errors

**Error:**
```
Failed to authenticate with LLM provider
```

**Solution:**
1. Check your API key in `config.hcl`
2. Ensure the API key is valid and has sufficient credits
3. Verify the `base_url` is correct

### Web search failures

**Error:**
```
Search failed: No valid engines found
```

**Solution:**
This is expected if:
- Search engines are not configured in `config.hcl`
- Search engines are blocked by network/firewall
- Search engines require authentication

Web search tests will show warnings but won't fail the entire test suite.

### Timeout errors

**Error:**
```
Task timed out after 60000ms
```

**Solution:**
1. Increase timeout in queue configuration
2. Check network connectivity
3. Verify LLM API is responding

---

## Test Coverage

### Features Tested

| Feature | integration_tests | test_lane_features | test_search_config | test_builtin_skills | test_task_priority |
|---------|-------------------|--------------------|--------------------|---------------------|--------------------|
| Basic tools | ✅ | ✅ | ❌ | ❌ | ❌ |
| Built-in skills | ✅ | ❌ | ❌ | ✅ | ❌ |
| File operations | ✅ | ❌ | ❌ | ❌ | ❌ |
| Search operations | ✅ | ❌ | ❌ | ❌ | ❌ |
| Web search | ✅ | ❌ | ✅ | ❌ | ❌ |
| Planning mode | ✅ | ❌ | ❌ | ❌ | ❌ |
| Queue config | ✅ | ✅ | ❌ | ❌ | ✅ |
| Retry policies | ❌ | ✅ | ❌ | ❌ | ❌ |
| Rate limiting | ❌ | ✅ | ❌ | ❌ | ❌ |
| Priority boost | ❌ | ✅ | ❌ | ❌ | ❌ |
| Pressure monitoring | ❌ | ✅ | ❌ | ❌ | ❌ |
| Per-lane timeouts | ❌ | ✅ | ❌ | ❌ | ❌ |
| Search config | ❌ | ❌ | ✅ | ❌ | ❌ |
| Engine control | ❌ | ❌ | ✅ | ❌ | ❌ |
| Task priority | ❌ | ❌ | ❌ | ❌ | ✅ |
| Priority preemption | ❌ | ❌ | ❌ | ❌ | ✅ |

### Test Statistics

- **Total test files:** 5
- **Total features tested:** 22+
- **Code coverage:** All major v0.8.0 features
- **Real LLM:** Yes (uses actual API calls)
- **Network required:** Yes (for LLM and web search)

---

## CI/CD Integration

These tests can be integrated into CI/CD pipelines:

### GitHub Actions Example

```yaml
name: Integration Tests

on: [push, pull_request]

jobs:
  integration-tests:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v3

      - name: Setup Rust
        uses: actions-rs/toolchain@v1
        with:
          toolchain: stable

      - name: Create config
        run: |
          mkdir -p ~/.a3s
          echo '${{ secrets.A3S_CONFIG }}' > ~/.a3s/config.hcl

      - name: Run integration tests
        run: |
          cargo run --example integration_tests
          cargo run --example test_lane_features
          cargo run --example test_builtin_skills
          cargo run --example test_task_priority

      - name: Run search tests (allow failure)
        continue-on-error: true
        run: cargo run --example test_search_config
```

---

## Contributing

When adding new features to A3S Code, please:

1. Add corresponding integration tests
2. Update this README with test descriptions
3. Ensure all tests pass before submitting PR
4. Document any new configuration requirements

---

## License

MIT License - See LICENSE file for details

---

## Support

For issues or questions:
- GitHub Issues: https://github.com/A3S-Lab/Code/issues
- Documentation: https://docs.a3s.sh/code
