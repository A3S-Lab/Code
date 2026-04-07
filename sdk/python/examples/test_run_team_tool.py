#!/usr/bin/env python3
"""
A3S Code Python SDK - run_team Built-in Tool Integration Test

Demonstrates the `run_team` built-in tool, which the LLM can call autonomously
(or which you can invoke directly via session.tool()) to coordinate a
Lead → Worker → Reviewer team workflow.

The tool is part of the task delegation triad:
  - task          — single subagent, blocks until done
  - parallel_task — fan-out to multiple subagents concurrently
  - run_team      — Lead decomposes goal, Workers execute, Reviewer approves/rejects

Run with: python examples/test_run_team_tool.py
"""

import sys
import os
import json
from pathlib import Path

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from a3s_code import Agent
from conftest import find_config


def test_run_team_direct(agent: Agent) -> None:
    """
    Test 1: Call run_team via session.tool() directly (no LLM round-trip).

    This is the most reliable way to invoke run_team from SDK code when you
    know exactly what goal you want the team to tackle.
    """
    print("\n--- Test 1: run_team via session.tool() ---")

    session = agent.session(".")

    result = session.tool("run_team", {
        "goal": "List the top-level directories in this project and briefly describe each one.",
        "lead_agent": "general",
        "worker_agent": "general",
        "reviewer_agent": "general",
        "max_steps": 5,
    })

    print(f"Exit code : {result.exit_code}")
    print(f"Output    :\n{result.output}")
    assert result.exit_code == 0, f"Expected success, got: {result.output}"
    assert "Team run complete" in result.output
    print("✓ Test 1 passed\n")


def test_run_team_defaults(agent: Agent) -> None:
    """
    Test 2: Only 'goal' is required — all agent types default to 'general'.
    """
    print("--- Test 2: run_team with defaults ---")

    session = agent.session(".")

    result = session.tool("run_team", {
        "goal": "Count the Rust source files in the project.",
    })

    print(f"Exit code : {result.exit_code}")
    print(f"Output    :\n{result.output}")
    assert result.exit_code == 0
    assert "Team run complete" in result.output
    print("✓ Test 2 passed\n")


def test_run_team_via_llm(agent: Agent) -> None:
    """
    Test 3: LLM autonomously selects run_team.

    When the delegate-task skill is loaded, the LLM sees run_team in its
    toolbox and can choose it for goals that require decomposition + review.
    """
    print("--- Test 3: LLM-driven run_team (via session.send) ---")

    session = agent.session(".")
    # Load the delegate-task skill so the LLM knows about run_team.
    session.load_skill("delegate-task")

    response = session.send(
        "Use the run_team tool to coordinate a quick review of this project's "
        "directory structure. The team should identify the main components."
    )

    print(f"LLM response:\n{response.text}")
    print("✓ Test 3 passed\n")


def main() -> None:
    print("=== run_team Built-in Tool — Python SDK Integration Test ===\n")

    config = find_config()
    print(f"Config: {config}\n")

    agent = Agent.create(config)
    print("Agent created.\n")

    test_run_team_direct(agent)
    test_run_team_defaults(agent)
    test_run_team_via_llm(agent)

    print("=" * 60)
    print("All run_team tool tests passed.")
    print("=" * 60)


if __name__ == "__main__":
    main()
