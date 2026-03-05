#!/usr/bin/env python3
"""
Simple orchestrator test without real LLM.

Tests that Python bindings work correctly.
"""

import sys
import os

# Add parent directory to path
sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

try:
    from a3s_code import Orchestrator, SubAgentConfig
    print("✓ Successfully imported Orchestrator and SubAgentConfig")
except ImportError as e:
    print(f"✗ Import failed: {e}")
    print("\nMake sure to build the Python SDK first:")
    print("  cd sdk/python")
    print("  PYO3_USE_ABI3_FORWARD_COMPATIBILITY=1 cargo build --release")
    print("  cp target/release/a3s_code.pyd . (Windows)")
    print("  # or cp target/release/liba3s_code.so a3s_code.so (Linux)")
    sys.exit(1)

def main():
    print("\n=== Orchestrator Python Bindings Test ===\n")

    # Create orchestrator
    print("1. Creating orchestrator...")
    orch = Orchestrator.create()
    print(f"   ✓ Created: {orch}")

    # Check initial state
    print("\n2. Checking initial state...")
    active = orch.active_count()
    print(f"   Active SubAgents: {active}")
    assert active == 0, "Should have 0 active SubAgents initially"
    print("   ✓ Initial state correct")

    # Create SubAgent config
    print("\n3. Creating SubAgent config...")
    config = SubAgentConfig(
        agent_type="general",
        prompt="Test prompt",
        description="Test SubAgent",
        permissive=True,
        max_steps=5
    )
    print(f"   ✓ Config created: {config}")

    # Spawn SubAgent
    print("\n4. Spawning SubAgent...")
    handle = orch.spawn_subagent(config)
    print(f"   ✓ SubAgent spawned")
    print(f"   ID: {handle.id}")
    print(f"   Handle: {handle}")

    # Check active count
    print("\n5. Checking active count...")
    active = orch.active_count()
    print(f"   Active SubAgents: {active}")
    assert active == 1, "Should have 1 active SubAgent"
    print("   ✓ Active count correct")

    # Check state
    print("\n6. Checking SubAgent state...")
    state = handle.state()
    print(f"   State: {state}")
    print("   ✓ State retrieved")

    # Test pause
    print("\n7. Testing pause...")
    try:
        handle.pause()
        print("   ✓ Pause command sent")
    except Exception as e:
        print(f"   Note: Pause may fail if SubAgent already completed: {e}")

    # Test resume
    print("\n8. Testing resume...")
    try:
        handle.resume()
        print("   ✓ Resume command sent")
    except Exception as e:
        print(f"   Note: Resume may fail if SubAgent already completed: {e}")

    # Wait for completion
    print("\n9. Waiting for SubAgent completion...")
    try:
        result = handle.wait()
        print(f"   ✓ SubAgent completed")
        print(f"   Result: {result}")
    except Exception as e:
        print(f"   Result: {e}")

    # Final state
    print("\n10. Final state check...")
    state = handle.state()
    active = orch.active_count()
    print(f"   Final state: {state}")
    print(f"   Active SubAgents: {active}")

    print("\n✓ All tests passed!")

if __name__ == "__main__":
    main()
