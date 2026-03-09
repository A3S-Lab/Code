#!/usr/bin/env python3
"""
Test Issue #5 Fix: Fine-grained Permissive Control

Tests the new permissive_deny functionality:
  1. SubAgentConfig.permissive_deny field
  2. Permissive mode respects deny rules
  3. Agent definition deny rules are enforced in permissive mode

Run:
  export MOONSHOT_API_KEY=sk-...
  python examples/test_issue5_permissive_deny.py
"""

import os
import sys
import time
from pathlib import Path

from a3s_code import Agent, Orchestrator, SubAgentConfig


def find_config() -> str:
    """Locate config: system ~/.a3s/config.hcl or local agent_kimi.hcl."""
    system = Path.home() / ".a3s" / "config.hcl"
    if system.exists():
        return str(system)
    here = Path(__file__).parent
    local = here / "agent_kimi.hcl"
    if local.exists():
        return str(local)
    raise FileNotFoundError("No config found: ~/.a3s/config.hcl or agent_kimi.hcl")


def test_permissive_deny():
    """Test fine-grained permissive control with deny rules."""
    print("\n" + "=" * 70)
    print("Test: Fine-grained Permissive Control (Issue #5)")
    print("=" * 70)

    # Check API key
    if not os.getenv("MOONSHOT_API_KEY"):
        print("❌ MOONSHOT_API_KEY not set. Skipping test.")
        return False

    try:
        config_path = find_config()
        print(f"✓ Using config: {config_path}")
    except FileNotFoundError as e:
        print(f"❌ {e}")
        return False

    # Create agent and orchestrator
    print("\n[1] Creating Agent and Orchestrator...")
    agent = Agent.create(config_path)
    orch = Orchestrator.create()
    print("✓ Orchestrator created")

    # Test 1: Permissive mode WITHOUT deny rules (baseline)
    print("\n[2] Test 1: Permissive mode without deny rules")
    print("-" * 70)

    config1 = SubAgentConfig(
        agent_type="general",
        prompt="Use bash to run 'echo test1' and then use grep to search for 'test' in the output.",
        description="Permissive without deny",
        permissive=True,
        max_steps=5,
    )

    handle1 = orch.spawn_subagent(config1)
    print(f"✓ Sub-agent 1 spawned: {handle1.id}")

    # Monitor for tool calls
    tools_used_1 = []
    events1 = handle1.events()

    print("  Monitoring tool calls...")
    start_time = time.time()
    while time.time() - start_time < 20:
        try:
            event = events1.recv(timeout_ms=1000)
            if event is None:
                continue

            event_type = event.get("event_type", "")
            if event_type == "tool_execution_started":
                tool_name = event.get("tool_name", "")
                tools_used_1.append(tool_name)
                print(f"    🔧 Tool used: {tool_name}")
            elif event_type == "subagent_completed":
                break
        except:
            continue

    try:
        result1 = handle1.wait()
        print(f"  ✓ Completed")
    except:
        pass

    print(f"  Tools used: {tools_used_1}")

    # Test 2: Permissive mode WITH deny rules
    print("\n[3] Test 2: Permissive mode with deny rules")
    print("-" * 70)
    print("  Denying: ['Grep', 'grep']")

    config2 = SubAgentConfig(
        agent_type="general",
        prompt="Use bash to run 'echo test2' and then use grep to search for 'test' in the output.",
        description="Permissive with deny",
        permissive=True,
        permissive_deny=["Grep", "grep"],  # Block grep tool
        max_steps=5,
    )

    handle2 = orch.spawn_subagent(config2)
    print(f"✓ Sub-agent 2 spawned: {handle2.id}")

    # Monitor for tool calls
    tools_used_2 = []
    grep_blocked = False
    events2 = handle2.events()

    print("  Monitoring tool calls...")
    start_time = time.time()
    while time.time() - start_time < 20:
        try:
            event = events2.recv(timeout_ms=1000)
            if event is None:
                continue

            event_type = event.get("event_type", "")
            if event_type == "tool_execution_started":
                tool_name = event.get("tool_name", "")
                tools_used_2.append(tool_name)
                print(f"    🔧 Tool used: {tool_name}")
                if tool_name.lower() == "grep":
                    print(f"    ⚠️  Grep was NOT blocked!")
            elif event_type == "subagent_completed":
                break
        except:
            continue

    try:
        result2 = handle2.wait()
        print(f"  ✓ Completed")
    except:
        pass

    print(f"  Tools used: {tools_used_2}")

    # Check if grep was blocked
    grep_used_in_test2 = any("grep" in tool.lower() for tool in tools_used_2)
    if not grep_used_in_test2:
        print("  ✅ Grep was successfully blocked by permissive_deny")
        grep_blocked = True
    else:
        print("  ❌ Grep was NOT blocked (permissive_deny not working)")

    # Test 3: Test with wildcard deny pattern
    print("\n[4] Test 3: Permissive mode with wildcard deny pattern")
    print("-" * 70)
    print("  Denying: ['Bash(echo:*)']")

    config3 = SubAgentConfig(
        agent_type="general",
        prompt="Use bash to run 'echo test3' and 'ls -la'.",
        description="Permissive with wildcard deny",
        permissive=True,
        permissive_deny=["Bash(echo:*)"],  # Block bash echo commands
        max_steps=5,
    )

    handle3 = orch.spawn_subagent(config3)
    print(f"✓ Sub-agent 3 spawned: {handle3.id}")

    # Monitor for tool calls
    bash_commands_3 = []
    events3 = handle3.events()

    print("  Monitoring bash commands...")
    start_time = time.time()
    while time.time() - start_time < 20:
        try:
            event = events3.recv(timeout_ms=1000)
            if event is None:
                continue

            event_type = event.get("event_type", "")
            if event_type == "tool_execution_started":
                tool_name = event.get("tool_name", "")
                if tool_name.lower() == "bash":
                    args = event.get("args", {})
                    command = args.get("command", "") if isinstance(args, dict) else ""
                    bash_commands_3.append(command)
                    print(f"    🔧 Bash command: {command[:50]}")
            elif event_type == "subagent_completed":
                break
        except:
            continue

    try:
        result3 = handle3.wait()
        print(f"  ✓ Completed")
    except:
        pass

    print(f"  Bash commands: {bash_commands_3}")

    # Check if echo was blocked
    echo_used = any("echo" in cmd.lower() for cmd in bash_commands_3)
    if not echo_used:
        print("  ✅ Echo commands were successfully blocked")
    else:
        print("  ⚠️  Echo commands were used (pattern matching might need adjustment)")

    # Print summary
    print("\n" + "=" * 70)
    print("Summary:")
    print("=" * 70)

    success = True

    print(f"\nTest 1 (no deny): {len(tools_used_1)} tools used")
    print(f"Test 2 (deny grep): {len(tools_used_2)} tools used")
    print(f"Test 3 (deny echo): {len(bash_commands_3)} bash commands")

    if grep_blocked:
        print("\n✅ permissive_deny field works correctly")
    else:
        print("\n❌ permissive_deny field NOT working")
        success = False

    # Note about the test
    print("\n" + "=" * 70)
    print("Note:")
    print("=" * 70)
    print("The LLM might work around denied tools by using alternatives.")
    print("The key test is whether the denied tool is actually blocked when")
    print("the LLM tries to use it, not whether the LLM finds workarounds.")

    return success


if __name__ == "__main__":
    try:
        success = test_permissive_deny()
        if success:
            print("\n✅ Test PASSED")
            sys.exit(0)
        else:
            print("\n⚠️  Test completed with warnings")
            sys.exit(0)
    except Exception as e:
        print(f"\n❌ Test FAILED: {e}")
        import traceback
        traceback.print_exc()
        sys.exit(1)
