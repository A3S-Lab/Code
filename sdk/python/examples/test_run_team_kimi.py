#!/usr/bin/env python3
"""
run_team smoke test with Kimi model.

Tests Orchestrator.run_team() — Lead decomposes a goal, Workers execute,
Reviewer approves. Uses kimi-k2.5 via local proxy.
"""

import sys
import os
from pathlib import Path

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from a3s_code import Agent, Orchestrator, AgentSlot
from conftest import find_config


def main():
    print("=== run_team + Kimi Test (Python) ===\n")

    CONFIG_PATH = find_config()
    print(f"✓ Config: {CONFIG_PATH}\n")

    print("Creating agent...")
    agent = Agent.create(CONFIG_PATH)
    print("✓ Agent created\n")

    print("Creating orchestrator (from_agent mode)...")
    orch = Orchestrator.create(agent)
    print("✓ Orchestrator created\n")

    slots = [
        AgentSlot(
            agent_type="general",
            role="lead",
            prompt="",
            description="Lead: decompose the goal into tasks",
            permissive=True,
            max_steps=5,
        ),
        AgentSlot(
            agent_type="general",
            role="worker",
            prompt="",
            description="Worker: execute assigned tasks",
            permissive=True,
            max_steps=5,
        ),
        AgentSlot(
            agent_type="general",
            role="reviewer",
            prompt="",
            description="Reviewer: approve or reject results",
            permissive=True,
            max_steps=3,
        ),
    ]

    print("Running team: Lead → Worker → Reviewer...")
    result = orch.run_team(
        "List 3 common Python data structures and briefly describe each",
        ".",
        slots,
    )

    print(f"\n✓ Done tasks:     {len(result.done_tasks)}")
    print(f"✓ Rejected tasks: {len(result.rejected_tasks)}")
    print(f"✓ Rounds:         {result.rounds}")

    for task in result.done_tasks:
        print(f"\n  Task: {task.description!r}")
        print(f"  Result: {task.result!r}")

    print("\n=== run_team test passed ✓ ===")

if __name__ == "__main__":
    main()
