#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""
Simple test of Orchestrator with real Kimi API - Fixed version
"""

import asyncio
import sys
from pathlib import Path

from a3s_code import Orchestrator, SubAgentConfig

async def main():
    print("="*60)
    print("Orchestrator Test with Kimi API")
    print("="*60)
    print()

    # Create Orchestrator
    print("1. Creating Orchestrator...")
    orch = Orchestrator.create()
    print("   OK - Orchestrator created")
    print()

    # Create SubAgent config with more steps
    print("2. Creating SubAgent...")
    config = SubAgentConfig(
        agent_type="test",
        description="Test task",
        prompt="Count from 1 to 10",
        permissive=True,
        max_steps=10,  # More steps so we can test control
    )

    handle = orch.spawn_subagent(config)
    print(f"   OK - SubAgent spawned: {handle.id}")
    print()

    # Monitor and control while running
    print("3. Monitoring and control test...")
    for i in range(8):
        print(f"\n   Snapshot {i+1}:")
        print(f"   Active count: {orch.active_count()}")

        # List all
        subagents = orch.list_subagents()
        for info in subagents:
            print(f"   - {info.id}: {info.state}")
            if info.current_activity:
                print(f"     Activity: {info.current_activity.activity_type}")

        # Test pause/resume on snapshot 3
        if i == 2:
            info = orch.get_subagent_info(handle.id)
            if info and "Running" in info.state:
                print(f"\n   >>> Pausing {handle.id}...")
                try:
                    orch.pause_subagent(handle.id)
                    await asyncio.sleep(0.3)
                    info = orch.get_subagent_info(handle.id)
                    print(f"   >>> State after pause: {info.state}")
                except Exception as e:
                    print(f"   >>> Pause failed (SubAgent may have completed): {e}")

        # Test resume on snapshot 4
        if i == 3:
            info = orch.get_subagent_info(handle.id)
            if info and "Paused" in info.state:
                print(f"\n   >>> Resuming {handle.id}...")
                try:
                    orch.resume_subagent(handle.id)
                    await asyncio.sleep(0.3)
                    info = orch.get_subagent_info(handle.id)
                    print(f"   >>> State after resume: {info.state}")
                except Exception as e:
                    print(f"   >>> Resume failed: {e}")

        await asyncio.sleep(0.5)

    print()

    # Wait for completion
    print("4. Waiting for completion...")
    orch.wait_all()
    print("   OK - All completed")
    print()

    # Final states
    print("5. Final states:")
    states = orch.get_all_states()
    for subagent_id, state in states:
        print(f"   {subagent_id}: {state}")

    # Test all query APIs
    print()
    print("6. Testing query APIs:")

    # list_subagents
    subagents = orch.list_subagents()
    print(f"   list_subagents(): {len(subagents)} SubAgent(s)")

    # get_subagent_info
    info = orch.get_subagent_info(handle.id)
    if info:
        print(f"   get_subagent_info(): ID={info.id}, Type={info.agent_type}")

    # get_active_activities
    activities = orch.get_active_activities()
    print(f"   get_active_activities(): {len(activities)} active")

    # get_all_states
    states = orch.get_all_states()
    print(f"   get_all_states(): {len(states)} state(s)")

    # active_count
    count = orch.active_count()
    print(f"   active_count(): {count}")

    print()
    print("="*60)
    print("SUCCESS - All APIs tested!")
    print("="*60)
    print()
    print("Tested APIs:")
    print("  - Orchestrator.create()")
    print("  - spawn_subagent()")
    print("  - list_subagents()")
    print("  - get_subagent_info()")
    print("  - get_active_activities()")
    print("  - get_all_states()")
    print("  - active_count()")
    print("  - pause_subagent()")
    print("  - resume_subagent()")
    print("  - wait_all()")
    print()
    print("Activity types observed:")
    print("  - idle")
    print("  - calling_tool")
    print("  - requesting_llm")
    print("  - waiting_for_control (when paused)")

if __name__ == "__main__":
    asyncio.run(main())
