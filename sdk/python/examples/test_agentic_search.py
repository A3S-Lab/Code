#!/usr/bin/env python3
"""
Agentic Search - Full Feature Test (Kimi K2.5)

Tests all agentic_search modes and parameters using the Kimi model:
  - FAST mode (default)
  - DEEP mode (Monte Carlo sampling)
  - FILENAME_ONLY mode
  - include glob filter
  - context_lines adjustment
  - max_results limit

Prerequisites:
  export KIMI_API_KEY="your-api-key"
  export KIMI_BASE_URL="http://your-kimi-endpoint/v1"
"""

import os
import sys
import time
from pathlib import Path

from a3s_code import Agent, SessionOptions


# ── Config ────────────────────────────────────────────────────────────────────

CONFIG = str(Path(__file__).parent / "agent_kimi_k2.5.hcl")
# Search target: the code crate itself (rich Rust codebase)
WORKSPACE = str(Path(__file__).parent.parent.parent / "core")


# ── Helpers ───────────────────────────────────────────────────────────────────

def section(title: str) -> None:
    print(f"\n{'─' * 60}")
    print(f"  {title}")
    print(f"{'─' * 60}")


def run_test(session, name: str, prompt: str) -> bool:
    print(f"\n▶ {name}")
    t0 = time.time()
    try:
        result = session.send(prompt)
        elapsed = time.time() - t0
        print(f"  [{elapsed:.1f}s] {result.text[:300].strip()}")
        print(f"  ✅ PASS")
        return True
    except Exception as e:
        elapsed = time.time() - t0
        print(f"  [{elapsed:.1f}s] ❌ FAIL: {e}")
        return False


# ── Tests ─────────────────────────────────────────────────────────────────────

def test_fast_mode(session) -> bool:
    return run_test(
        session,
        "FAST mode — natural language query",
        (
            f"Use the agentic_search tool on workspace '{WORKSPACE}' "
            "with these exact parameters:\n"
            "  query: \"tool execution context\"\n"
            "  mode: \"fast\"\n"
            "  max_results: 5\n"
            "  context_lines: 2\n"
            "Show the file names and relevance scores from the result."
        ),
    )


def test_deep_mode(session) -> bool:
    return run_test(
        session,
        "DEEP mode — Monte Carlo evidence sampling",
        (
            f"Use the agentic_search tool on workspace '{WORKSPACE}' "
            "with these exact parameters:\n"
            "  query: \"agent loop LLM turn execution\"\n"
            "  mode: \"deep\"\n"
            "  max_results: 3\n"
            "Show the evidence scores from the result."
        ),
    )


def test_filename_only(session) -> bool:
    return run_test(
        session,
        "FILENAME_ONLY mode — quick file discovery",
        (
            f"Use the agentic_search tool on workspace '{WORKSPACE}' "
            "with these exact parameters:\n"
            "  query: \"builtin\"\n"
            "  mode: \"filename_only\"\n"
            "  max_results: 10\n"
            "List all file paths returned."
        ),
    )


def test_include_glob(session) -> bool:
    return run_test(
        session,
        "include glob — restrict to *.rs files",
        (
            f"Use the agentic_search tool on workspace '{WORKSPACE}' "
            "with these exact parameters:\n"
            "  query: \"permission policy checker\"\n"
            "  include: \"*.rs\"\n"
            "  max_results: 5\n"
            "  context_lines: 1\n"
            "Show the file names found."
        ),
    )


def test_context_lines(session) -> bool:
    return run_test(
        session,
        "context_lines — wide context window",
        (
            f"Use the agentic_search tool on workspace '{WORKSPACE}' "
            "with these exact parameters:\n"
            "  query: \"session store save load\"\n"
            "  max_results: 2\n"
            "  context_lines: 5\n"
            "Show the matching lines with their surrounding context."
        ),
    )


def test_max_results_limit(session) -> bool:
    return run_test(
        session,
        "max_results — enforce result cap",
        (
            f"Use the agentic_search tool on workspace '{WORKSPACE}' "
            "with these exact parameters:\n"
            "  query: \"pub fn\"\n"
            "  max_results: 2\n"
            "Confirm that no more than 2 files are returned."
        ),
    )


def test_no_results(session) -> bool:
    return run_test(
        session,
        "no results — graceful empty response",
        (
            f"Use the agentic_search tool on workspace '{WORKSPACE}' "
            "with these exact parameters:\n"
            "  query: \"xyzzy_nonexistent_term_12345\"\n"
            "  mode: \"fast\"\n"
            "Confirm that no results were found."
        ),
    )


# ── Main ──────────────────────────────────────────────────────────────────────

def main() -> None:
    print("=" * 60)
    print("  Agentic Search — Full Feature Test (Kimi K2.5)")
    print("=" * 60)
    print(f"  Config:    {CONFIG}")
    print(f"  Workspace: {WORKSPACE}")

    # Check environment variables
    if not os.getenv("KIMI_API_KEY"):
        print("\n❌ Error: KIMI_API_KEY environment variable not set")
        print("   export KIMI_API_KEY='your-api-key'")
        sys.exit(1)
    if not os.getenv("KIMI_BASE_URL"):
        print("\n❌ Error: KIMI_BASE_URL environment variable not set")
        print("   export KIMI_BASE_URL='http://your-endpoint/v1'")
        sys.exit(1)

    agent = Agent.create(CONFIG)

    opts = SessionOptions()
    opts.permissive = True   # auto-approve all tool calls
    opts.max_tool_rounds = 5

    session = agent.session(WORKSPACE, opts)
    print("\n  ✓ Session ready\n")

    tests = [
        test_fast_mode,
        test_deep_mode,
        test_filename_only,
        test_include_glob,
        test_context_lines,
        test_max_results_limit,
        test_no_results,
    ]

    section("Running tests")
    results = [t(session) for t in tests]

    section("Summary")
    passed = sum(results)
    total = len(results)
    labels = [
        "FAST mode",
        "DEEP mode",
        "FILENAME_ONLY mode",
        "include glob",
        "context_lines",
        "max_results limit",
        "no results",
    ]
    for label, ok in zip(labels, results):
        status = "✅" if ok else "❌"
        print(f"  {status}  {label}")

    print(f"\n  {passed}/{total} tests passed")
    sys.exit(0 if passed == total else 1)


if __name__ == "__main__":
    main()
