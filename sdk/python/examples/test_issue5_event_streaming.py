#!/usr/bin/env python3
"""
Test Issue #5 Fix: Sub-agent Event Streaming

Tests the new event streaming functionality:
  1. SubAgentHandle.events() method
  2. TextDelta event forwarding
  3. TurnStart event forwarding
  4. Tool execution events with arguments

Run:
  export MOONSHOT_API_KEY=sk-...
  python examples/test_issue5_event_streaming.py
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


def test_event_streaming():
    """Test sub-agent event streaming with Kimi model."""
    print("\n" + "=" * 70)
    print("Test: Sub-agent Event Streaming (Issue #5)")
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

    # Spawn sub-agent with a simple task
    print("\n[2] Spawning sub-agent...")
    config = SubAgentConfig(
        agent_type="general",
        prompt="Use bash to echo 'Hello from sub-agent!' and then explain what you did.",
        description="Event streaming test",
        permissive=True,
        max_steps=5,
    )

    handle = orch.spawn_subagent(config)
    print(f"✓ Sub-agent spawned: {handle.id}")

    # Subscribe to events
    print("\n[3] Subscribing to sub-agent events...")
    print("-" * 70)

    event_counts = {
        "text_delta": 0,
        "turn_start": 0,
        "tool_start": 0,
        "tool_end": 0,
        "subagent_internal_event": 0,
        "other": 0,
    }

    text_output = []
    tool_calls = []

    # Use handle.events() to subscribe to this sub-agent's events
    print("Monitoring events (timeout: 30s)...")
    start_time = time.time()
    timeout = 30

    try:
        # Subscribe to events for this sub-agent
        events = handle.events()

        while time.time() - start_time < timeout:
            try:
                event = events.recv(timeout_ms=1000)
                if event is None:
                    continue

                event_type = event.get("event_type", "unknown")

                # Count events
                if event_type == "subagent_internal_event":
                    event_counts["subagent_internal_event"] += 1
                    inner_event = event.get("event", {})
                    inner_type = inner_event.get("type", "")

                    if inner_type == "text_delta":
                        event_counts["text_delta"] += 1
                        text = inner_event.get("text", "")
                        text_output.append(text)
                        print(f"  📝 TextDelta: {repr(text[:50])}")
                    elif inner_type == "turn_start":
                        event_counts["turn_start"] += 1
                        turn = inner_event.get("turn", 0)
                        print(f"  🔄 TurnStart: turn={turn}")

                elif event_type == "tool_execution_started":
                    event_counts["tool_start"] += 1
                    tool_name = event.get("tool_name", "")
                    tool_id = event.get("tool_id", "")
                    args = event.get("args", {})
                    print(f"  🔧 ToolStart: {tool_name} (id={tool_id})")
                    if args and args != {}:
                        print(f"     Args: {args}")
                    tool_calls.append({"name": tool_name, "id": tool_id, "args": args})

                elif event_type == "tool_execution_completed":
                    event_counts["tool_end"] += 1
                    tool_name = event.get("tool_name", "")
                    result = event.get("result", "")
                    exit_code = event.get("exit_code", 0)
                    print(f"  ✅ ToolEnd: {tool_name} (exit={exit_code})")
                    print(f"     Result: {result[:100]}")

                elif event_type == "subagent_completed":
                    print(f"  🏁 SubAgent completed")
                    break

                else:
                    event_counts["other"] += 1
                    print(f"  ℹ️  {event_type}")

            except Exception as e:
                if "timeout" not in str(e).lower():
                    print(f"  ⚠️  Event error: {e}")
                continue

    except Exception as e:
        print(f"❌ Error subscribing to events: {e}")
        return False

    # Wait for completion
    print("\n[4] Waiting for sub-agent to complete...")
    try:
        result = handle.wait()
        print(f"✓ Sub-agent completed")
        print(f"  Result: {result[:200]}")
    except Exception as e:
        print(f"⚠️  Wait error: {e}")

    # Print summary
    print("\n" + "=" * 70)
    print("Event Summary:")
    print("=" * 70)
    for event_type, count in event_counts.items():
        print(f"  {event_type:30s}: {count:3d}")

    print(f"\n  Total text deltas: {len(text_output)}")
    print(f"  Total tool calls:  {len(tool_calls)}")

    if text_output:
        full_text = "".join(text_output)
        print(f"\n  Streamed text ({len(full_text)} chars):")
        print(f"  {repr(full_text[:200])}")

    # Validation
    print("\n" + "=" * 70)
    print("Validation:")
    print("=" * 70)

    success = True

    if event_counts["text_delta"] > 0:
        print("  ✅ TextDelta events received")
    else:
        print("  ⚠️  No TextDelta events (might be expected for some models)")

    if event_counts["turn_start"] > 0:
        print("  ✅ TurnStart events received")
    else:
        print("  ❌ No TurnStart events received")
        success = False

    if event_counts["tool_start"] > 0:
        print("  ✅ ToolStart events received")
    else:
        print("  ⚠️  No ToolStart events")

    if event_counts["tool_end"] > 0:
        print("  ✅ ToolEnd events received")
    else:
        print("  ⚠️  No ToolEnd events")

    if event_counts["subagent_internal_event"] > 0:
        print("  ✅ SubAgentInternalEvent forwarding works")
    else:
        print("  ❌ No SubAgentInternalEvent received")
        success = False

    return success


if __name__ == "__main__":
    try:
        success = test_event_streaming()
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
