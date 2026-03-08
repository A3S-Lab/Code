#!/usr/bin/env python3
"""
run_team built-in tool smoke test — Kimi model.

Tests Session.run_team() — the typed convenience wrapper that invokes the
`run_team` built-in tool (Lead → Worker → Reviewer) directly from SDK code,
without going through the Orchestrator API.

Compared to test_run_team_kimi.py (which tests Orchestrator.run_team()),
this test exercises the new Session.run_team() convenience method added to
the Python SDK alongside the RunTeamTool built-in.

Run with:
    cd sdk/python
    python examples/test_run_team_tool_kimi.py
"""

import sys
import os

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from a3s_code import Agent

CONFIG_PATH = os.path.normpath(os.path.join(
    os.path.dirname(__file__),
    "..", "..", "..", "..", "..", ".a3s", "config.hcl"
))


def print_section(title: str) -> None:
    print(f"\n{'─' * 60}")
    print(f"  {title}")
    print(f"{'─' * 60}")


def main() -> None:
    print("=== run_team built-in tool — Kimi integration test (Python) ===\n")

    if not os.path.exists(CONFIG_PATH):
        print(f"✗ Config not found: {CONFIG_PATH}")
        sys.exit(1)
    print(f"✓ Config: {CONFIG_PATH}")

    agent = Agent.create(CONFIG_PATH)
    print("✓ Agent created (kimi-k2.5)\n")

    # ------------------------------------------------------------------
    # Test 1: session.run_team() — typed convenience method, all defaults
    # ------------------------------------------------------------------
    print_section("Test 1: session.run_team() with defaults")

    session = agent.session(".")
    print("Goal: List 3 Python built-in data types and describe each briefly.\n")

    result = session.run_team(
        "List 3 Python built-in data types and describe each one in one sentence.",
        max_steps=5,
    )

    print(f"exit_code : {result.exit_code}")
    print(f"output    :\n{result.output}")

    assert result.exit_code == 0, f"Expected success; got exit_code={result.exit_code}"
    assert "Team run complete" in result.output, "Missing 'Team run complete' in output"
    print("✓ Test 1 passed")

    # ------------------------------------------------------------------
    # Test 2: explicit agent types
    # ------------------------------------------------------------------
    print_section("Test 2: session.run_team() with explicit agent types")

    session2 = agent.session(".")
    print("Goal: Count Rust source files in the codebase.\n")

    result2 = session2.run_team(
        "Count the number of .rs source files in the codebase and report the total.",
        lead_agent="general",
        worker_agent="explore",
        reviewer_agent="general",
        max_steps=5,
    )

    print(f"exit_code : {result2.exit_code}")
    print(f"output    :\n{result2.output}")

    assert result2.exit_code == 0, f"Expected success; got exit_code={result2.exit_code}"
    assert "Team run complete" in result2.output
    print("✓ Test 2 passed")

    # ------------------------------------------------------------------
    # Test 3: tool() generic call — same method, different surface
    # ------------------------------------------------------------------
    print_section("Test 3: session.tool('run_team', {...}) — generic call")

    session3 = agent.session(".")
    result3 = session3.tool("run_team", {
        "goal": "What programming language is this project primarily written in?",
        "max_steps": 10,
    })

    print(f"exit_code : {result3.exit_code}")
    print(f"output    :\n{result3.output}")

    assert result3.exit_code == 0
    assert "Team run complete" in result3.output
    print("✓ Test 3 passed")

    print("\n" + "=" * 60)
    print("All tests passed ✓")
    print("=" * 60)


if __name__ == "__main__":
    main()
