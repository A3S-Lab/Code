#!/usr/bin/env python3
"""
Integration tests for Python SDK API alignment with Rust core.

Tests the new SessionOptions fields added in this PR:
  - temperature
  - thinking_budget
  - continuation_enabled
  - max_continuation_turns
  - planning (now in SessionOptions object, not just kwargs)
  - goal_tracking (same)
  - max_parse_retries (same)
  - tool_timeout_ms (same)
  - circuit_breaker_threshold (same)

Uses the kimi-k2.5 model. API key is read from KIMI_API_KEY environment
variable via HCL env() function.

Usage:
    KIMI_API_KEY=sk-... python3 test_api_alignment.py
"""

import os
import sys
from pathlib import Path

try:
    import a3s_code
    from a3s_code import Agent, SessionOptions
except ImportError as e:
    print(f"ERROR: Failed to import a3s_code: {e}")
    sys.exit(1)

# ---------------------------------------------------------------------------
# Kimi config (API key from env var)
# ---------------------------------------------------------------------------

# Path to the HCL config that uses env("KIMI_API_KEY") and env("KIMI_BASE_URL")
KIMI_HCL_CONFIG = str(Path(__file__).parent / "agent_kimi_k2.5.hcl")

PASS = "PASS"
FAIL = "FAIL"
SKIP = "SKIP"

results = []


def check(name: str, condition: bool, detail: str = ""):
    status = PASS if condition else FAIL
    results.append((name, status, detail))
    icon = "✓" if condition else "✗"
    msg = f"  {icon} {name}"
    if detail:
        msg += f": {detail}"
    print(msg)
    return condition


def section(title: str):
    print(f"\n{'=' * 60}")
    print(f"  {title}")
    print('=' * 60)


# ---------------------------------------------------------------------------
# Phase 1: SessionOptions field presence (no LLM call)
# ---------------------------------------------------------------------------

section("Phase 1: SessionOptions field presence")

opts = SessionOptions()

check("temperature default is None", opts.temperature is None)
check("thinking_budget default is None", opts.thinking_budget is None)
check("continuation_enabled default is None", opts.continuation_enabled is None)
check("max_continuation_turns default is None", opts.max_continuation_turns is None)
check("planning default is False", opts.planning == False)
check("goal_tracking default is False", opts.goal_tracking == False)
check("max_parse_retries default is None", opts.max_parse_retries is None)
check("tool_timeout_ms default is None", opts.tool_timeout_ms is None)
check("circuit_breaker_threshold default is None", opts.circuit_breaker_threshold is None)

# Test setters
opts.temperature = 0.5
check("temperature setter", opts.temperature == 0.5)

opts.thinking_budget = 8000
check("thinking_budget setter", opts.thinking_budget == 8000)

opts.continuation_enabled = False
check("continuation_enabled setter", opts.continuation_enabled == False)

opts.max_continuation_turns = 5
check("max_continuation_turns setter", opts.max_continuation_turns == 5)

opts.planning = True
check("planning setter", opts.planning == True)

opts.goal_tracking = True
check("goal_tracking setter", opts.goal_tracking == True)

opts.max_parse_retries = 3
check("max_parse_retries setter", opts.max_parse_retries == 3)

opts.tool_timeout_ms = 30000
check("tool_timeout_ms setter", opts.tool_timeout_ms == 30000)

opts.circuit_breaker_threshold = 5
check("circuit_breaker_threshold setter", opts.circuit_breaker_threshold == 5)

# ---------------------------------------------------------------------------
# Phase 2: Integration tests against kimi-k2.5
# ---------------------------------------------------------------------------

api_key = os.environ.get("KIMI_API_KEY")
if not api_key:
    print(f"\n  SKIP: KIMI_API_KEY not set — skipping live LLM tests")
    results.append(("live LLM tests", SKIP, "KIMI_API_KEY not set"))
else:
    section("Phase 2: Live integration tests (kimi-k2.5)")

    config_path = KIMI_HCL_CONFIG

    def make_agent():
        return Agent.create(config_path)

    cwd = os.getcwd()

    # --- 2a: basic send ---
    print("\n  2a. Basic send")
    try:
        agent = make_agent()
        session = agent.session(cwd)
        result = session.send("Reply with exactly: HELLO")
        check("basic send returns result", result is not None)
        check("basic send has text", bool(result.text))
        check("basic send contains HELLO", "HELLO" in result.text.upper(),
              f"got: {result.text[:80]!r}")
    except Exception as e:
        check("basic send", False, str(e))

    # --- 2b: temperature via SessionOptions ---
    print("\n  2b. temperature in SessionOptions")
    try:
        agent = make_agent()
        opts = SessionOptions()
        opts.model = "openai/kimi-k2.5"
        opts.temperature = 0.0
        session = agent.session(cwd, opts)
        result = session.send("Reply with exactly: TEMPERATURE_OK")
        check("temperature=0.0 accepted", result is not None)
        check("temperature=0.0 has text", bool(result.text),
              f"got: {result.text[:80]!r}")
    except Exception as e:
        check("temperature via SessionOptions", False, str(e))

    # --- 2c: continuation_enabled=False ---
    print("\n  2c. continuation_enabled=False")
    try:
        agent = make_agent()
        opts = SessionOptions()
        opts.continuation_enabled = False
        session = agent.session(cwd, opts)
        result = session.send("Reply with exactly: CONT_OK")
        check("continuation_enabled=False accepted", result is not None)
        check("continuation_enabled=False has text", bool(result.text),
              f"got: {result.text[:80]!r}")
    except Exception as e:
        check("continuation_enabled=False", False, str(e))

    # --- 2d: max_continuation_turns ---
    print("\n  2d. max_continuation_turns=1")
    try:
        agent = make_agent()
        opts = SessionOptions()
        opts.max_continuation_turns = 1
        session = agent.session(cwd, opts)
        result = session.send("Reply with exactly: TURNS_OK")
        check("max_continuation_turns=1 accepted", result is not None)
    except Exception as e:
        check("max_continuation_turns=1", False, str(e))

    # --- 2e: planning via SessionOptions ---
    print("\n  2e. planning=True via SessionOptions object")
    try:
        agent = make_agent()
        opts = SessionOptions()
        opts.planning = True
        session = agent.session(cwd, opts, permissive=True)
        result = session.send("Reply with exactly: PLAN_OK")
        check("planning=True via SessionOptions accepted", result is not None)
    except Exception as e:
        check("planning=True via SessionOptions", False, str(e))

    # --- 2f: max_parse_retries via SessionOptions ---
    print("\n  2f. max_parse_retries=5 via SessionOptions object")
    try:
        agent = make_agent()
        opts = SessionOptions()
        opts.max_parse_retries = 5
        session = agent.session(cwd, opts)
        result = session.send("Reply with exactly: RETRIES_OK")
        check("max_parse_retries=5 via SessionOptions accepted", result is not None)
    except Exception as e:
        check("max_parse_retries=5 via SessionOptions", False, str(e))

    # --- 2g: tool_timeout_ms via SessionOptions ---
    print("\n  2g. tool_timeout_ms=60000 via SessionOptions object")
    try:
        agent = make_agent()
        opts = SessionOptions()
        opts.tool_timeout_ms = 60000
        session = agent.session(cwd, opts, permissive=True)
        result = session.send("Reply with exactly: TIMEOUT_OK")
        check("tool_timeout_ms=60000 accepted", result is not None)
    except Exception as e:
        check("tool_timeout_ms=60000", False, str(e))

    # --- 2h: circuit_breaker_threshold via SessionOptions ---
    print("\n  2h. circuit_breaker_threshold=5 via SessionOptions object")
    try:
        agent = make_agent()
        opts = SessionOptions()
        opts.circuit_breaker_threshold = 5
        session = agent.session(cwd, opts)
        result = session.send("Reply with exactly: CB_OK")
        check("circuit_breaker_threshold=5 accepted", result is not None)
    except Exception as e:
        check("circuit_breaker_threshold=5", False, str(e))

    # --- 2i: combined options ---
    print("\n  2i. Combined new options")
    try:
        agent = make_agent()
        opts = SessionOptions()
        opts.model = "openai/kimi-k2.5"
        opts.temperature = 0.3
        opts.continuation_enabled = True
        opts.max_continuation_turns = 2
        opts.max_parse_retries = 3
        opts.tool_timeout_ms = 30000
        opts.circuit_breaker_threshold = 5
        session = agent.session(cwd, opts)
        result = session.send("Reply with exactly: COMBINED_OK")
        check("combined options accepted", result is not None)
        check("combined options has text", bool(result.text),
              f"got: {result.text[:80]!r}")
    except Exception as e:
        check("combined options", False, str(e))


# ---------------------------------------------------------------------------
# Summary
# ---------------------------------------------------------------------------

section("Summary")

total = len(results)
passed = sum(1 for _, s, _ in results if s == PASS)
failed = sum(1 for _, s, _ in results if s == FAIL)
skipped = sum(1 for _, s, _ in results if s == SKIP)

print(f"\n  Total: {total}  Passed: {passed}  Failed: {failed}  Skipped: {skipped}")

if failed > 0:
    print("\n  Failed tests:")
    for name, status, detail in results:
        if status == FAIL:
            print(f"    ✗ {name}: {detail}")

print()
sys.exit(0 if failed == 0 else 1)
