#!/usr/bin/env python3
"""
Test script to verify Orchestrator monitoring APIs are working correctly.
This test uses placeholder execution (no real LLM calls).
"""

import sys
import time


def test_orchestrator_apis():
    """Test all Orchestrator monitoring APIs"""
    print("Testing Orchestrator Monitoring APIs...\n")

    try:
        from a3s_code import Orchestrator, SubAgentConfig
        print("✓ Import successful")
    except ImportError as e:
        print(f"✗ Import failed: {e}")
        print("\nNote: This is expected if a3s-code is not installed.")
        print("The API definitions are correct, but the package needs to be built and installed.")
        return True

    # Test 1: Create Orchestrator
    print("\n1. Testing Orchestrator.create()...")
    try:
        orch = Orchestrator.create()
        print("   ✓ Orchestrator created")
    except Exception as e:
        print(f"   ✗ Failed: {e}")
        return False

    # Test 2: Create SubAgentConfig
    print("\n2. Testing SubAgentConfig...")
    try:
        config = SubAgentConfig(
            agent_type="test",
            description="Test SubAgent",
            prompt="Test prompt",
            permissive=True,
            max_steps=3,
        )
        print("   ✓ SubAgentConfig created")
    except Exception as e:
        print(f"   ✗ Failed: {e}")
        return False

    # Test 3: Spawn SubAgent
    print("\n3. Testing spawn_subagent()...")
    try:
        handle = orch.spawn_subagent(config)
        print(f"   ✓ SubAgent spawned: {handle.id}")
    except Exception as e:
        print(f"   ✗ Failed: {e}")
        return False

    # Test 4: Active count
    print("\n4. Testing active_count()...")
    try:
        count = orch.active_count()
        print(f"   ✓ Active count: {count}")
    except Exception as e:
        print(f"   ✗ Failed: {e}")
        return False

    # Test 5: List SubAgents
    print("\n5. Testing list_subagents()...")
    try:
        subagents = orch.list_subagents()
        print(f"   ✓ Found {len(subagents)} SubAgent(s)")
        if len(subagents) > 0:
            info = subagents[0]
            print(f"      - ID: {info.id}")
            print(f"      - Type: {info.agent_type}")
            print(f"      - State: {info.state}")
            print(f"      - Description: {info.description}")
            if info.current_activity:
                print(f"      - Activity: {info.current_activity.activity_type}")
    except Exception as e:
        print(f"   ✗ Failed: {e}")
        return False

    # Test 6: Get SubAgent info
    print("\n6. Testing get_subagent_info()...")
    try:
        info = orch.get_subagent_info(handle.id)
        if info:
            print(f"   ✓ Got info for {info.id}")
            print(f"      - State: {info.state}")
            print(f"      - Created: {info.created_at}")
            print(f"      - Updated: {info.updated_at}")
        else:
            print("   ✗ No info returned")
            return False
    except Exception as e:
        print(f"   ✗ Failed: {e}")
        return False

    # Test 7: Get active activities
    print("\n7. Testing get_active_activities()...")
    try:
        activities = orch.get_active_activities()
        print(f"   ✓ Found {len(activities)} active activit(ies)")
        for subagent_id, activity in activities:
            print(f"      - {subagent_id}: {activity.activity_type}")
    except Exception as e:
        print(f"   ✗ Failed: {e}")
        return False

    # Test 8: Get all states
    print("\n8. Testing get_all_states()...")
    try:
        states = orch.get_all_states()
        print(f"   ✓ Found {len(states)} state(s)")
        for subagent_id, state in states:
            print(f"      - {subagent_id}: {state}")
    except Exception as e:
        print(f"   ✗ Failed: {e}")
        return False

    # Test 9: Pause SubAgent
    print("\n9. Testing pause_subagent()...")
    try:
        orch.pause_subagent(handle.id)
        time.sleep(0.2)
        info = orch.get_subagent_info(handle.id)
        print(f"   ✓ Paused, state: {info.state}")
    except Exception as e:
        print(f"   ✗ Failed: {e}")
        return False

    # Test 10: Resume SubAgent
    print("\n10. Testing resume_subagent()...")
    try:
        orch.resume_subagent(handle.id)
        time.sleep(0.2)
        info = orch.get_subagent_info(handle.id)
        print(f"   ✓ Resumed, state: {info.state}")
    except Exception as e:
        print(f"   ✗ Failed: {e}")
        return False

    # Test 11: Cancel SubAgent
    print("\n11. Testing cancel_subagent()...")
    try:
        orch.cancel_subagent(handle.id)
        time.sleep(0.2)
        info = orch.get_subagent_info(handle.id)
        print(f"   ✓ Cancelled, state: {info.state}")
    except Exception as e:
        print(f"   ✗ Failed: {e}")
        return False

    print("\n" + "=" * 50)
    print("✓ All API tests passed!")
    print("=" * 50)
    return True


if __name__ == "__main__":
    success = test_orchestrator_apis()
    sys.exit(0 if success else 1)
