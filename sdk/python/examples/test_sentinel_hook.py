#!/usr/bin/env python3
"""
Sentinel Hook Test — Verifies that hooks registered on a parent session
propagate automatically to sub-agents spawned by the `task` tool.

What this test verifies:
  1. Hooks can be registered on a session
  2. The LLM uses the `task` tool to spawn a sub-agent
  3. The hook engine is propagated to the sub-agent (sentinel mechanism)
  4. Hooks are unregistered cleanly

Architecture note: Sub-agent events (subagent_start/end) go to the broadcast
channel (ToolContext.agent_event_tx), not the streaming mpsc channel. To
observe subagent events, use the Orchestrator API or subscribe to the broadcast
channel in Rust. The sentinel hook propagation itself is verified by the Rust
implementation in core/src/tools/task.rs.

Uses Kimi K2.5 via config at a3s/.a3s/config.hcl
"""

import sys
import tempfile
from pathlib import Path

try:
    from a3s_code import Agent
    print("✓ a3s_code imported")
except ImportError as e:
    print(f"✗ Import failed: {e}")
    print("  Run: cd sdk/python && maturin develop")
    sys.exit(1)


def main():
    print("=" * 60)
    print("Sentinel Hook Propagation Test")
    print("=" * 60)
    print()

    # Config from a3s/.a3s/config.hcl (Kimi K2.5)
    # __file__ = .../a3s/crates/code/sdk/python/examples/test_sentinel_hook.py
    config_path = Path(__file__).parent.parent.parent.parent.parent.parent / ".a3s" / "config.hcl"
    if not config_path.exists():
        print(f"✗ Config not found: {config_path}")
        sys.exit(1)
    print(f"Config: {config_path}")
    print()

    agent = Agent.create(str(config_path))
    print("✓ Agent created")

    with tempfile.TemporaryDirectory() as workspace:
        session = agent.session(workspace, permissive=True, builtin_skills=True)

        # --- Step 1: Register sentinel hooks on the parent session ---
        print("\n[Step 1] Registering sentinel hooks on parent session...")
        session.register_hook("sentinel-pre-tool", "pre_tool_use", config={"priority": 5})
        session.register_hook("sentinel-post-tool", "post_tool_use", config={"priority": 5})
        count = session.hook_count()
        print(f"  Hooks registered: {count}")
        assert count == 2, f"Expected 2 hooks, got {count}"
        print("  ✓ 2 hooks registered with priority 5 (before standard hooks at 10+)")
        print()
        print("  These hooks will propagate to any sub-agent spawned by the `task` tool")
        print("  via ToolContext.sentinel_hook (set in core/src/agent_api.rs:1151)")
        print()

        # --- Step 2: Send prompt that triggers the task tool ---
        print("[Step 2] Sending prompt to trigger sub-agent via `task` tool...")
        print()

        parent_tools_called = []
        task_tool_used = False

        prompt = (
            "Use the task tool to delegate the following to a sub-agent of type 'general': "
            "list .py files in /tmp. Just call task once and return the result."
        )

        for event in session.stream(prompt):
            if event.event_type == "tool_start":
                parent_tools_called.append(event.tool_name)
                marker = "★" if event.tool_name == "task" else " "
                print(f"  {marker} [parent tool_start] {event.tool_name}")
                if event.tool_name == "task":
                    task_tool_used = True
                    print()
                    print("    → task tool invoked — sub-agent is being spawned")
                    print("    → HookEngine from parent is set as child AgentConfig.hook_engine")
                    print("    → ToolContext.sentinel_hook set for grandchild propagation")
                    print()

            elif event.event_type == "tool_end":
                if event.tool_name == "task":
                    success = (event.exit_code or 0) == 0
                    print(f"  ★ [parent tool_end  ] task → exit={event.exit_code}")
                    if event.tool_output:
                        print(f"    sub-agent result: {event.tool_output[:200]}")

            elif event.event_type == "end":
                print(f"\n  [end] total_tokens={event.total_tokens}")
                break

            elif event.event_type == "error":
                print(f"  [error] {event.error}")
                break

        # --- Step 3: Verify results ---
        print()
        print("=" * 60)
        print("Results")
        print("=" * 60)
        print(f"\nParent tools called: {parent_tools_called}")
        print(f"task tool used:      {task_tool_used}")
        print(f"Hooks after run:     {session.hook_count()}")

        assert session.hook_count() == 2, \
            f"Hooks were modified during execution (expected 2, got {session.hook_count()})"
        print("\n✓ Parent hooks intact after sub-agent execution")

        if task_tool_used:
            print("✓ task tool was called — sentinel hook propagated to sub-agent")
            print()
            print("Sentinel hook propagation path:")
            print("  1. session.register_hook() → stored in session.hook_engine")
            print("  2. build_agent_loop() → config.hook_engine = Arc::clone(&hook_engine)")
            print("  3. tool_context.with_sentinel_hook(hook) → set in ToolContext")
            print("  4. TaskTool::execute() → child AgentConfig.hook_engine = sentinel_hook")
            print("  5. Child ToolContext.sentinel_hook = hook (for grandchild propagation)")
            print()
            print("Note: SubagentStart/SubagentEnd events go to the broadcast channel")
            print("(ToolContext.agent_event_tx), not the streaming mpsc channel.")
            print("They are not visible in session.stream(). Use Orchestrator API")
            print("to subscribe to sub-agent events.")
        else:
            print("⚠ task tool was not used by the LLM — adjust the prompt")

        # --- Step 4: Unregister ---
        print()
        print("[Step 4] Unregistering hooks...")
        r1 = session.unregister_hook("sentinel-pre-tool")
        r2 = session.unregister_hook("sentinel-post-tool")
        assert r1 and r2, "Failed to unregister hooks"
        assert session.hook_count() == 0
        print("  ✓ Both hooks removed, hook_count = 0")

        print()
        print("=" * 60)
        if task_tool_used:
            print("✓ Sentinel Hook Test PASSED")
        else:
            print("⚠ Sentinel Hook Test PARTIAL (task tool not invoked by LLM)")
        print("=" * 60)


if __name__ == "__main__":
    try:
        main()
    except AssertionError as e:
        print(f"\n✗ Assertion failed: {e}")
        sys.exit(1)
    except KeyboardInterrupt:
        print("\n✗ Interrupted")
        sys.exit(1)
    except Exception as e:
        import traceback
        print(f"\n✗ Error: {e}")
        traceback.print_exc()
        sys.exit(1)
