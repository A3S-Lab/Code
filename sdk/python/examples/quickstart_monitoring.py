#!/usr/bin/env python3
"""
Quick Start: Orchestrator Monitoring with Kimi

A minimal example showing the core monitoring features.

Usage:
    export MOONSHOT_API_KEY=your_api_key
    python quickstart_monitoring.py
"""

import asyncio
import os
from a3s_code import Orchestrator, SubAgentConfig


async def main():
    # Check API key
    if not os.getenv("MOONSHOT_API_KEY"):
        print("Error: Set MOONSHOT_API_KEY environment variable")
        return

    # Create orchestrator
    orch = Orchestrator.create()
    print("✓ Orchestrator created\n")

    # Spawn 2 SubAgents
    config1 = SubAgentConfig(
        agent_type="explore",
        description="Find Python files",
        prompt="Use glob to find all Python files",
        permissive=True,
        max_steps=3,
    )

    config2 = SubAgentConfig(
        agent_type="analyze",
        description="Search TODOs",
        prompt="Use grep to find TODO comments",
        permissive=True,
        max_steps=3,
    )

    handle1 = orch.spawn_subagent(config1)
    handle2 = orch.spawn_subagent(config2)
    print(f"✓ Spawned: {handle1.id}")
    print(f"✓ Spawned: {handle2.id}\n")

    # Monitor for 3 seconds
    print("Monitoring for 3 seconds...\n")
    for i in range(3):
        # Get all SubAgent info
        subagents = orch.list_subagents()

        print(f"[{i+1}s] Active: {orch.active_count()}")
        for info in subagents:
            activity = "None"
            if info.current_activity:
                activity = info.current_activity.activity_type
            print(f"  {info.id}: {info.state} | Activity: {activity}")

        await asyncio.sleep(1)
        print()

    # Pause first SubAgent
    print(f"Pausing {handle1.id}...")
    orch.pause_subagent(handle1.id)
    await asyncio.sleep(0.5)

    info = orch.get_subagent_info(handle1.id)
    print(f"  State: {info.state}\n")

    # Resume
    print(f"Resuming {handle1.id}...")
    orch.resume_subagent(handle1.id)
    await asyncio.sleep(0.5)

    info = orch.get_subagent_info(handle1.id)
    print(f"  State: {info.state}\n")

    # Wait for completion
    print("Waiting for all to complete...")
    orch.wait_all()

    # Final states
    print("\nFinal states:")
    states = orch.get_all_states()
    for subagent_id, state in states:
        print(f"  {subagent_id}: {state}")

    print("\n✓ Done!")


if __name__ == "__main__":
    asyncio.run(main())
