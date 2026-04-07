#!/usr/bin/env python3
"""
Real-world test of Orchestrator monitoring with actual Kimi API.
Uses the config from a3s/.a3s/config.hcl

This test will:
1. Create an Orchestrator
2. Spawn 3 SubAgents with real tasks
3. Monitor their execution in real-time
4. Demonstrate control operations
5. Show all monitoring APIs in action
"""

import asyncio
import sys
import os
from pathlib import Path

# Add parent directory to path to import a3s_code
sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

try:
    from a3s_code import Agent, Orchestrator, SubAgentConfig
    from conftest import find_config
    print("✓ Successfully imported a3s_code")
except ImportError as e:
    print(f"✗ Failed to import a3s_code: {e}")
    print("\nPlease build and install the Python SDK first:")
    print("  cd sdk/python")
    print("  maturin develop --release")
    sys.exit(1)


async def main():
    print("=" * 70)
    print("Orchestrator Real-World Test with Kimi API")
    print("=" * 70)
    print()

    # Get config path using unified config loader
    config_path = find_config()
    print(f"✓ Using config: {config_path}")
    print()

    # Create Agent with config
    print("Creating Agent with Kimi configuration...")
    try:
        agent = Agent.create(str(config_path))
        print("✓ Agent created successfully")
    except Exception as e:
        print(f"✗ Failed to create agent: {e}")
        sys.exit(1)

    print()

    # Create Orchestrator
    print("Creating Orchestrator...")
    try:
        orch = Orchestrator.create()
        print("✓ Orchestrator created")
    except Exception as e:
        print(f"✗ Failed to create orchestrator: {e}")
        sys.exit(1)

    print()

    # Configure SubAgents with real tasks
    print("Configuring SubAgents with real tasks...")
    configs = [
        SubAgentConfig(
            agent_type="explore",
            description="探索代码库结构",
            prompt="使用 glob 工具查找当前目录下的所有 .rs 文件，并统计数量",
            permissive=True,
            max_steps=3,
        ),
        SubAgentConfig(
            agent_type="analyze",
            description="分析 TODO 注释",
            prompt="使用 grep 工具搜索所有 TODO 注释，并列出前 5 个",
            permissive=True,
            max_steps=3,
        ),
        SubAgentConfig(
            agent_type="document",
            description="读取 README",
            prompt="使用 read 工具读取 README.md 文件的前 10 行",
            permissive=True,
            max_steps=2,
        ),
    ]

    # Spawn SubAgents
    print("\nSpawning SubAgents...")
    handles = []
    for i, config in enumerate(configs, 1):
        try:
            handle = orch.spawn_subagent(config)
            handles.append(handle)
            print(f"  {i}. ✓ Spawned: {handle.id} ({config.agent_type})")
            print(f"     Task: {config.description}")
        except Exception as e:
            print(f"  {i}. ✗ Failed to spawn: {e}")

    print(f"\n✓ Total SubAgents spawned: {len(handles)}")
    print(f"✓ Active count: {orch.active_count()}")
    print()

    # Real-time monitoring
    print("=" * 70)
    print("Real-time Monitoring (10 snapshots, 1 second interval)")
    print("=" * 70)
    print()

    for snapshot in range(1, 11):
        print(f"--- Snapshot #{snapshot} ({asyncio.get_event_loop().time():.1f}s) ---")

        # Get all SubAgent information
        try:
            subagents = orch.list_subagents()
            active_count = orch.active_count()
            print(f"Active: {active_count}/{len(subagents)}")
            print()

            for info in subagents:
                # Basic info
                print(f"📋 {info.id}")
                print(f"   Type: {info.agent_type}")
                print(f"   State: {info.state}")
                print(f"   Description: {info.description}")

                # Current activity
                if info.current_activity:
                    activity = info.current_activity
                    print(f"   🔄 Activity: {activity.activity_type}")
                    if activity.data:
                        # Parse and display activity data
                        import json
                        try:
                            data = json.loads(activity.data)
                            if activity.activity_type == "calling_tool":
                                print(f"      Tool: {data.get('tool_name', 'unknown')}")
                            elif activity.activity_type == "requesting_llm":
                                print(f"      Messages: {data.get('message_count', 0)}")
                            elif activity.activity_type == "waiting_for_control":
                                print(f"      Reason: {data.get('reason', 'unknown')}")
                        except:
                            pass
                else:
                    print(f"   🔄 Activity: None")

                print()

            # Show active activities summary
            activities = orch.get_active_activities()
            if activities:
                print("Active Activities Summary:")
                for subagent_id, activity in activities:
                    print(f"  • {subagent_id}: {activity.activity_type}")
                print()

        except Exception as e:
            print(f"✗ Monitoring error: {e}")

        # Wait before next snapshot
        await asyncio.sleep(1)

    # Demonstrate control operations
    print()
    print("=" * 70)
    print("Control Operations Demo")
    print("=" * 70)
    print()

    if len(handles) > 0:
        target_id = handles[0].id

        # Pause
        print(f"1. Pausing {target_id}...")
        try:
            orch.pause_subagent(target_id)
            await asyncio.sleep(0.5)

            info = orch.get_subagent_info(target_id)
            if info:
                print(f"   ✓ State: {info.state}")
                if info.current_activity:
                    print(f"   ✓ Activity: {info.current_activity.activity_type}")
        except Exception as e:
            print(f"   ✗ Failed: {e}")

        print()

        # Resume
        print(f"2. Resuming {target_id}...")
        try:
            orch.resume_subagent(target_id)
            await asyncio.sleep(0.5)

            info = orch.get_subagent_info(target_id)
            if info:
                print(f"   ✓ State: {info.state}")
        except Exception as e:
            print(f"   ✗ Failed: {e}")

        print()

    # Query specific SubAgent
    print("=" * 70)
    print("Query Specific SubAgent")
    print("=" * 70)
    print()

    if len(handles) > 0:
        target_id = handles[0].id
        print(f"Querying {target_id}...")

        try:
            info = orch.get_subagent_info(target_id)
            if info:
                print(f"  ID: {info.id}")
                print(f"  Type: {info.agent_type}")
                print(f"  Description: {info.description}")
                print(f"  State: {info.state}")
                print(f"  Parent ID: {info.parent_id or 'None'}")
                print(f"  Created: {info.created_at}")
                print(f"  Updated: {info.updated_at}")

                if info.current_activity:
                    print(f"  Current Activity:")
                    print(f"    Type: {info.current_activity.activity_type}")
                    if info.current_activity.data:
                        print(f"    Data: {info.current_activity.data}")
        except Exception as e:
            print(f"  ✗ Failed: {e}")

    print()

    # Get all states
    print("=" * 70)
    print("All SubAgent States")
    print("=" * 70)
    print()

    try:
        states = orch.get_all_states()
        for subagent_id, state in states:
            print(f"  {subagent_id}: {state}")
    except Exception as e:
        print(f"✗ Failed: {e}")

    print()

    # Wait for all to complete
    print("=" * 70)
    print("Waiting for all SubAgents to complete...")
    print("=" * 70)
    print()

    try:
        orch.wait_all()
        print("✓ All SubAgents completed")
    except Exception as e:
        print(f"✗ Wait failed: {e}")

    print()

    # Final status
    print("=" * 70)
    print("Final Status")
    print("=" * 70)
    print()

    try:
        final_states = orch.get_all_states()
        for subagent_id, state in final_states:
            print(f"  {subagent_id}: {state}")
    except Exception as e:
        print(f"✗ Failed: {e}")

    print()
    print("=" * 70)
    print("✓ Test completed successfully!")
    print("=" * 70)


if __name__ == "__main__":
    try:
        asyncio.run(main())
    except KeyboardInterrupt:
        print("\n\n✗ Test interrupted by user")
        sys.exit(1)
    except Exception as e:
        print(f"\n\n✗ Test failed with error: {e}")
        import traceback
        traceback.print_exc()
        sys.exit(1)
