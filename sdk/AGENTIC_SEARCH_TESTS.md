# Agentic Search - Full Feature Tests

Complete test suites for the `agentic_search` tool using the Kimi K2.5 model.

## Test Coverage

Both Python and TypeScript tests cover all agentic_search features:

1. **FAST mode** — Default keyword cascade search (2-5s)
2. **DEEP mode** — Monte Carlo evidence sampling (10-30s)
3. **FILENAME_ONLY mode** — Quick file discovery (< 1s)
4. **include glob** — Filter by file type (e.g., `*.rs`)
5. **context_lines** — Adjust context window size
6. **max_results** — Limit result count
7. **no results** — Graceful handling of empty results

## Prerequisites

### 1. Set Environment Variables

```bash
export KIMI_API_KEY="your-api-key-here"
export KIMI_BASE_URL="http://your-kimi-endpoint/v1"
```

### 2. Install Dependencies

**Python:**
```bash
cd crates/code/sdk/python
pip install -e .
```

**TypeScript:**
```bash
cd crates/code/sdk/node
npm install
npm run build
```

## Running Tests

### Python

```bash
cd crates/code/sdk/python/examples
python test_agentic_search.py
```

**Expected output:**
```
============================================================
  Agentic Search — Full Feature Test (Kimi K2.5)
============================================================
  Config:    .../agent_kimi_k2.5.hcl
  Workspace: .../core

  ✓ Session ready

────────────────────────────────────────────────────────────
  Running tests
────────────────────────────────────────────────────────────

▶ FAST mode — natural language query
  [2.3s] Found 5 file(s) matching "tool execution context"...
  ✅ PASS

▶ DEEP mode — Monte Carlo evidence sampling
  [18.7s] Deep search found 3 evidence region(s)...
  ✅ PASS

▶ FILENAME_ONLY mode — quick file discovery
  [0.8s] src/tools/builtin/mod.rs...
  ✅ PASS

▶ include glob — restrict to *.rs files
  [1.5s] Found 5 file(s) in *.rs files...
  ✅ PASS

▶ context_lines — wide context window
  [2.1s] Showing matches with 5 lines of context...
  ✅ PASS

▶ max_results — enforce result cap
  [1.2s] Returned 2 files (max_results enforced)...
  ✅ PASS

▶ no results — graceful empty response
  [0.5s] No results found for "xyzzy_nonexistent_term_12345"...
  ✅ PASS

────────────────────────────────────────────────────────────
  Summary
────────────────────────────────────────────────────────────
  ✅  FAST mode
  ✅  DEEP mode
  ✅  FILENAME_ONLY mode
  ✅  include glob
  ✅  context_lines
  ✅  max_results limit
  ✅  no results

  7/7 tests passed
```

### TypeScript

```bash
cd crates/code/sdk/node/examples
npx tsx test-agentic-search.ts
```

**Expected output:** (same format as Python)

## Test Details

### Test 1: FAST Mode

Tests the default search mode with natural language query.

**Query:** `"tool execution context"`
**Parameters:**
- `mode: "fast"`
- `max_results: 5`
- `context_lines: 2`

**Validates:**
- Keyword extraction works
- IDF-weighted relevance scoring
- File type awareness
- Context line extraction

### Test 2: DEEP Mode

Tests Monte Carlo evidence sampling for comprehensive analysis.

**Query:** `"agent loop LLM turn execution"`
**Parameters:**
- `mode: "deep"`
- `max_results: 3`

**Validates:**
- Gaussian importance sampling
- Evidence score calculation
- Adaptive sigma adjustment
- Evidence region synthesis

### Test 3: FILENAME_ONLY Mode

Tests quick file discovery without content search.

**Query:** `"builtin"`
**Parameters:**
- `mode: "filename_only"`
- `max_results: 10`

**Validates:**
- Filename matching
- Fast execution (< 1s)
- Path-based search

### Test 4: Include Glob

Tests file type filtering with glob patterns.

**Query:** `"permission policy checker"`
**Parameters:**
- `include: "*.rs"`
- `max_results: 5`
- `context_lines: 1`

**Validates:**
- Glob pattern matching
- File type filtering
- Reduced search scope

### Test 5: Context Lines

Tests context window adjustment.

**Query:** `"session store save load"`
**Parameters:**
- `max_results: 2`
- `context_lines: 5`

**Validates:**
- Wide context extraction
- Before/after line collection
- Context formatting

### Test 6: Max Results Limit

Tests result count enforcement.

**Query:** `"pub fn"`
**Parameters:**
- `max_results: 2`

**Validates:**
- Result truncation
- Top-N selection
- Relevance-based ordering

### Test 7: No Results

Tests graceful handling of empty results.

**Query:** `"xyzzy_nonexistent_term_12345"`
**Parameters:**
- `mode: "fast"`

**Validates:**
- Empty result handling
- Error-free execution
- User-friendly message

## Troubleshooting

### Error: KIMI_API_KEY not set

```bash
export KIMI_API_KEY="your-api-key"
export KIMI_BASE_URL="http://your-endpoint/v1"
```

### Error: Module not found

**Python:**
```bash
cd crates/code/sdk/python
pip install -e .
```

**TypeScript:**
```bash
cd crates/code/sdk/node
npm install
npm run build
```

### Tests timeout

Increase the timeout in the test file or check network connectivity to the Kimi endpoint.

### DEEP mode is slow

This is expected — DEEP mode uses Monte Carlo sampling and takes 10-30 seconds. Use FAST mode for quick searches.

## Configuration Files

- `agent_kimi_k2.5.hcl` — Agent configuration for Kimi K2.5 model
- Uses environment variables for API key and base URL (secure)
- Storage backend: memory (no persistence)
- Max tool rounds: 20

## Related

- [Agentic Search Tool Reference](/en/docs/code/tools/agentic-search)
- [Agentic Search Technical Deep Dive](/en/docs/code/examples/agentic-search)
- [A3S Code SDK Documentation](https://docs.a3s.dev)
