#!/usr/bin/env python3
"""
Orchestrator Real-time Monitoring Demo with Kimi Model

This example demonstrates:
1. Creating an orchestrator with Kimi LLM
2. Spawning multiple SubAgents with different tasks
3. Real-time monitoring of SubAgent states and activities
4. Dynamic control (pause, resume, cancel)
5. Querying SubAgent information and activities

Requirements:
- Set MOONSHOT_API_KEY environment variable
- pip install a3s-code
"""

import asyncio
import os
import time
from a3s_code import Orchestrator, SubAgentConfig


async def main():
    print("=== Orchestrator Real-time Monitoring with Kimi ===\n")

    # Check API key
    api_key = os.getenv("MOONSHOT_API_KEY")
    if not api_key:
        print("Error: MOONSHOT_API_KEY environment variable not set")
        print("Please set it with: export MOONSHOT_API_KEY=your_api_key")
        return

    # 1. Create orchestrator
    print("✓ Creating Orchestrator with memory-based event communication\n")
    orchestrator = Orchestrator.create()

    # 2. Configure SubAgents with different tasks
    configs = [
        SubAgentConfig(
            agent_type="explore",
            description="探索 Python 代码库",
            prompt="使用 glob 工具查找所有 Python 文件，并统计文件数量",
            permissive=True,
            max_steps=5,
        ),
        SubAgentConfig(
            agent_type="analyze",
            description="分析代码质量",
            prompt="使用 grep 工具搜索 TODO 和 FIXME 注释",
            permissive=True,
            max_steps=5,
        ),
        SubAgentConfig(
            agent_type="document",
            description="生成文档",
            prompt="使用 read 工具读取 README.md 文件并总结内容",
            permissive=True,
            max_steps=5,
        ),
    ]

    # 3. Spawn SubAgents
    print("Spawning 3 SubAgents...")
    handles = []
    for config in configs:
        handle = orchestrator.spawn_subagent(config)
        handles.append(handle)
        print(f"  ✓ Spawned: {handle.id} ({config.agent_type})")

    print(f"\n✓ Total active SubAgents: {orchestrator.active_count()}\n")

    # 4. Real-time monitoring loop
    print("=== Real-time Monitoring (5 snapshots) ===\n")

    for snapshot in range(1, 6):
        print(f"--- Snapshot #{snapshot} ---")
        print(f"Time: {time.strftime('%H:%M:%S')}")

        # Get all SubAgent information
        subagents = orchestrator.list_subagents()
        print(f"Active SubAgents: {orchestrator.active_count()}/{len(subagents)}\n")

        for info in subagents:
            print(f"SubAgent: {info.id}")
            print(f"  Type: {info.agent_type}")
            print(f"  Description: {info.description}")
            print(f"  State: {info.state}")
            print(f"  Created: {info.created_at}")

            if info.current_activity:
                activity = info.current_activity
                print(f"  Current Activity: {activity.activity_type}")
                if activity.data:
                    print(f"    Data: {activity.data}")
            else:
                print(f"  Current Activity: None")
            print()

        # Get all active activities
        activities = orchestrator.get_active_activities()
        if activities:
            print("Active Activities:")
            for subagent_id, activity in activities:
                print(f"  {subagent_id}: {activity.activity_type}")
                if activity.data:
                    print(f"    {activity.data}")
            print()

        # Wait before next snapshot
        await asyncio.sleep(1)

    # 5. Demonstrate control operations
    print("\n=== Control Operations Demo ===\n")

    if len(handles) > 0:
        first_id = handles[0].id

        # Pause
        print(f"Pausing {first_id}...")
        orchestrator.pause_subagent(first_id)
        await asyncio.sleep(0.5)

        info = orchestrator.get_subagent_info(first_id)
        if info:
            print(f"  State after pause: {info.state}")
            if info.current_activity:
                print(f"  Activity: {info.current_activity.activity_type}")
        print()

        # Resume
        print(f"Resuming {first_id}...")
        orchestrator.resume_subagent(first_id)
        await asyncio.sleep(0.5)

        info = orchestrator.get_subagent_info(first_id)
        if info:
            print(f"  State after resume: {info.state}")
        print()

    # 6. Query specific SubAgent
    print("=== Query Specific SubAgent ===\n")
    if len(handles) > 0:
        target_id = handles[0].id
        info = orchestrator.get_subagent_info(target_id)

        if info:
            print(f"SubAgent Details:")
            print(f"  ID: {info.id}")
            print(f"  Type: {info.agent_type}")
            print(f"  Description: {info.description}")
            print(f"  State: {info.state}")
            print(f"  Parent ID: {info.parent_id}")
            print(f"  Created At: {info.created_at}")
            print(f"  Updated At: {info.updated_at}")

            if info.current_activity:
                print(f"  Current Activity:")
                print(f"    Type: {info.current_activity.activity_type}")
                if info.current_activity.data:
                    print(f"    Data: {info.current_activity.data}")
            print()

    # 7. Get all states
    print("=== All SubAgent States ===\n")
    states = orchestrator.get_all_states()
    for subagent_id, state in states:
        print(f"{subagent_id}: {state}")
    print()

    # 8. Wait for all to complete
    print("Waiting for all SubAgents to complete...")
    orchestrator.wait_all()

    # 9. Final status
    print("\n=== Final Status ===\n")
    final_states = orchestrator.get_all_states()
    for subagent_id, state in final_states:
        print(f"{subagent_id}: {state}")

    print("\n✓ Demo completed successfully!")


if __name__ == "__main__":
    asyncio.run(main())
