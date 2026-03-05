#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""
Simple test of Orchestrator with real Kimi API
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

    # Create SubAgent config
    print("2. Creating SubAgent...")
    config = SubAgentConfig(
        agent_type="test",
        description="Test task",
        prompt="Say hello",
        permissive=True,
        max_steps=2,
    )

    handle = orch.spawn_subagent(config)
    print(f"   OK - SubAgent spawned: {handle.id}")
    print()

    # Monitor
    print("3. Monitoring (5 snapshots)...")
    for i in range(5):
        print(f"\n   Snapshot {i+1}:")
        print(f"   Active count: {orch.active_count()}")

        # List all
        subagents = orch.list_subagents()
        for info in subagents:
            print(f"   - {info.id}: {info.state}")
            if info.current_activity:
                print(f"     Activity: {info.current_activity.activity_type}")

        await asyncio.sleep(1)

    print()

    # Control test
    print("4. Testing control...")
    print(f"   Pausing {handle.id}...")
    orch.pause_subagent(handle.id)
    await asyncio.sleep(0.5)

    info = orch.get_subagent_info(handle.id)
    print(f"   State after pause: {info.state}")

    print(f"   Resuming {handle.id}...")
    orch.resume_subagent(handle.id)
    await asyncio.sleep(0.5)

    info = orch.get_subagent_info(handle.id)
    print(f"   State after resume: {info.state}")
    print()

    # Wait for completion
    print("5. Waiting for completion...")
    orch.wait_all()
    print("   OK - All completed")
    print()

    # Final states
    print("6. Final states:")
    states = orch.get_all_states()
    for subagent_id, state in states:
        print(f"   {subagent_id}: {state}")

    print()
    print("="*60)
    print("Test completed successfully!")
    print("="*60)

if __name__ == "__main__":
    asyncio.run(main())
