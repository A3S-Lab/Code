#!/usr/bin/env python3
"""
Orchestrator example with real Kimi model.

Tests main-sub agent coordination with actual LLM execution.
"""

import sys
import os

# Add parent directory to path to import a3s_code
sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from a3s_code import Agent, Orchestrator, SubAgentConfig
import time

def main():
    print("=== Orchestrator + Kimi Model Test ===\n")

    # Create main agent with Kimi model
    print("Creating main agent...")
    config_path = os.path.join(os.path.dirname(__file__), "agent_kimi.hcl")
    agent = Agent.create(config_path)
    print(f"✓ Agent created with config: {config_path}\n")

    # Create orchestrator
    print("Creating orchestrator...")
    orch = Orchestrator.create()
    print(f"✓ Orchestrator created\n")

    # Spawn SubAgent 1: Code analysis
    print("Spawning SubAgent 1: Code analysis...")
    config1 = SubAgentConfig(
        agent_type="general",
        prompt="Use glob to find all Python files in the current directory and list them",
        description="Find Python files",
        permissive=True,
        max_steps=5
    )
    handle1 = orch.spawn_subagent(config1)
    print(f"✓ SubAgent 1 spawned: {handle1.id}\n")

    # Spawn SubAgent 2: Documentation check
    print("Spawning SubAgent 2: Documentation check...")
    config2 = SubAgentConfig(
        agent_type="explore",
        prompt="Check if README.md exists and read its first 10 lines",
        description="Check documentation",
        permissive=True,
        max_steps=3
    )
    handle2 = orch.spawn_subagent(config2)
    print(f"✓ SubAgent 2 spawned: {handle2.id}\n")

    # Monitor states
    print("--- Monitoring SubAgents ---")
    for i in range(10):
        time.sleep(1)
        state1 = handle1.state()
        state2 = handle2.state()
        active = orch.active_count()
        print(f"[{i+1}s] Active: {active}, SubAgent1: {state1}, SubAgent2: {state2}")

        # Test pause/resume on SubAgent 1 after 3 seconds
        if i == 2:
            print("\n→ Pausing SubAgent 1...")
            handle1.pause()
            print("✓ SubAgent 1 paused\n")

        if i == 4:
            print("\n→ Resuming SubAgent 1...")
            handle1.resume()
            print("✓ SubAgent 1 resumed\n")

        # Check if both are done
        if "Completed" in state1 and "Completed" in state2:
            print("\n✓ Both SubAgents completed")
            break

    # Wait for results
    print("\n--- Waiting for results ---")
    try:
        result1 = handle1.wait()
        print(f"\nSubAgent 1 result:\n{result1}\n")
    except Exception as e:
        print(f"\nSubAgent 1 error: {e}\n")

    try:
        result2 = handle2.wait()
        print(f"SubAgent 2 result:\n{result2}\n")
    except Exception as e:
        print(f"SubAgent 2 error: {e}\n")

    # Final status
    print(f"--- Final Status ---")
    print(f"Active SubAgents: {orch.active_count()}")
    print(f"SubAgent 1 state: {handle1.state()}")
    print(f"SubAgent 2 state: {handle2.state()}")
    print("\n✓ Test completed")

if __name__ == "__main__":
    main()
