#!/usr/bin/env python3
"""
AgentSlot smoke test with Kimi model.

Verifies that the new AgentSlot class and spawn() method work correctly
with a real LLM (kimi-k2.5 via local proxy).
"""

import sys
import os

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from a3s_code import Agent, Orchestrator, AgentSlot
from conftest import find_config

def main():
    print("=== AgentSlot + Kimi Test (Python) ===\n")

    config_path = find_config()
    print(f"✓ Config: {config_path}\n")

    print("Creating agent...")
    agent = Agent.create(config_path)
    print("✓ Agent created\n")

    print("Creating orchestrator (from_agent mode)...")
    orch = Orchestrator.create(agent)
    print("✓ Orchestrator created\n")

    # Test 1: spawn() via AgentSlot — standalone, no role
    print("Test 1: spawn() via AgentSlot (standalone, no role)...")
    slot = AgentSlot(
        agent_type="general",
        prompt="Reply with exactly: SLOT_OK",
        description="AgentSlot smoke test",
        permissive=True,
        max_steps=3,
    )
    print(f"  {slot!r}")
    handle = orch.spawn(slot)
    print(f"  ✓ Subagent spawned: {handle.id}")
    result = handle.wait()
    print(f"  ✓ Result: {result!r}\n")

    # Test 2: spawn() with a role field set (role is carried but spawn ignores it)
    print("Test 2: spawn() with role='worker' (standalone mode, role field present)...")
    slot_worker = AgentSlot(
        agent_type="general",
        prompt="Reply with exactly: WORKER_OK",
        description="Worker slot smoke test",
        role="worker",
        permissive=True,
        max_steps=3,
    )
    print(f"  {slot_worker!r}")
    handle2 = orch.spawn(slot_worker)
    print(f"  ✓ Subagent spawned: {handle2.id}")
    result2 = handle2.wait()
    print(f"  ✓ Result: {result2!r}\n")

    print("=== All AgentSlot tests passed ✓ ===")

if __name__ == "__main__":
    main()
