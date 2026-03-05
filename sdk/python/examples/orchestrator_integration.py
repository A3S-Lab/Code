#!/usr/bin/env python3
"""
Orchestrator integration test with Agent API.

Demonstrates how Orchestrator can coordinate multiple Agent sessions.
"""

import sys
import os
import time

# Add parent directory to path
sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from a3s_code import Agent, Orchestrator, SubAgentConfig

def main():
    print("=== Orchestrator Integration Test ===\n")

    # Note: This test uses placeholder SubAgents since full AgentLoop integration
    # requires passing LlmClient, ToolExecutor, and other dependencies through
    # the Orchestrator API, which would make it too complex.
    #
    # For production use, you would:
    # 1. Create Agent instances with proper LLM configuration
    # 2. Use Agent.session() to create sessions
    # 3. Coordinate multiple sessions using application-level logic
    #
    # The Orchestrator demonstrates the coordination pattern and can be extended
    # to integrate with real Agent instances.

    print("1. Creating orchestrator...")
    orch = Orchestrator.create()
    print(f"   Active SubAgents: {orch.active_count()}\n")

    # Spawn multiple SubAgents
    print("2. Spawning SubAgents...")

    config1 = SubAgentConfig(
        agent_type="general",
        prompt="Analyze code structure",
        description="Code analysis task",
        permissive=True,
        max_steps=10
    )
    handle1 = orch.spawn_subagent(config1)
    print(f"   SubAgent 1: {handle1.id}")

    config2 = SubAgentConfig(
        agent_type="explore",
        prompt="Search for documentation",
        description="Documentation search",
        permissive=True,
        max_steps=5
    )
    handle2 = orch.spawn_subagent(config2)
    print(f"   SubAgent 2: {handle2.id}")

    config3 = SubAgentConfig(
        agent_type="general",
        prompt="Generate test cases",
        description="Test generation",
        permissive=True,
        max_steps=8
    )
    handle3 = orch.spawn_subagent(config3)
    print(f"   SubAgent 3: {handle3.id}\n")

    # Monitor execution
    print("3. Monitoring execution...")
    for i in range(15):
        time.sleep(0.2)
        active = orch.active_count()
        state1 = handle1.state()
        state2 = handle2.state()
        state3 = handle3.state()

        print(f"   [{i*0.2:.1f}s] Active: {active}, States: {state1[:20]}, {state2[:20]}, {state3[:20]}")

        # Test control operations
        if i == 3:
            print("\n   -> Pausing SubAgent 2...")
            handle2.pause()

        if i == 6:
            print("   -> Resuming SubAgent 2...")
            handle2.resume()

        if i == 9:
            print("   -> Cancelling SubAgent 3...\n")
            handle3.cancel()

        # Check if all done
        if active == 0:
            print(f"\n   All SubAgents completed at {i*0.2:.1f}s")
            break

    # Collect results
    print("\n4. Collecting results...")

    try:
        result1 = handle1.wait()
        print(f"   SubAgent 1: {result1[:80]}...")
    except Exception as e:
        print(f"   SubAgent 1 error: {e}")

    try:
        result2 = handle2.wait()
        print(f"   SubAgent 2: {result2[:80]}...")
    except Exception as e:
        print(f"   SubAgent 2 error: {e}")

    try:
        result3 = handle3.wait()
        print(f"   SubAgent 3: {result3[:80]}...")
    except Exception as e:
        print(f"   SubAgent 3 error: {e}")

    # Final status
    print("\n5. Final status:")
    print(f"   Active SubAgents: {orch.active_count()}")
    print(f"   SubAgent 1 state: {handle1.state()}")
    print(f"   SubAgent 2 state: {handle2.state()}")
    print(f"   SubAgent 3 state: {handle3.state()}")

    print("\n=== Test completed successfully ===")
    print("\nNote: This test uses placeholder execution.")
    print("For real LLM execution, integrate AgentLoop in SubAgentWrapper.")

if __name__ == "__main__":
    main()
