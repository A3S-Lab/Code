#!/usr/bin/env python3
"""Test script to verify SubAgentConfig attribute access after fix."""

# This test will work after rebuilding the Python SDK with the fix

try:
    from a3s_code import SubAgentConfig

    # Create a SubAgentConfig with skill_dirs
    cfg = SubAgentConfig(
        agent_type="my-sub-agent",
        prompt="Call Skill('scoring-video-adapter')",
        workspace="/path/to/project",
        permissive=True,
        skill_dirs=["/path/to/project/skills"],
    )

    # Test attribute access (this should work after the fix)
    print("Testing attribute access...")
    print(f"✓ agent_type: {cfg.agent_type}")
    print(f"✓ prompt: {cfg.prompt}")
    print(f"✓ workspace: {cfg.workspace}")
    print(f"✓ permissive: {cfg.permissive}")
    print(f"✓ skill_dirs: {cfg.skill_dirs}")

    # Test hasattr (this should return True after the fix)
    print("\nTesting hasattr...")
    assert hasattr(cfg, 'skill_dirs'), "skill_dirs attribute not found!"
    assert hasattr(cfg, 'agent_dirs'), "agent_dirs attribute not found!"
    assert hasattr(cfg, 'workspace'), "workspace attribute not found!"
    print("✓ All attributes accessible via hasattr")

    # Test setter
    print("\nTesting setter...")
    cfg.skill_dirs = ["/new/path"]
    assert cfg.skill_dirs == ["/new/path"], "Setter failed!"
    print(f"✓ skill_dirs updated to: {cfg.skill_dirs}")

    print("\n✅ All tests passed! SubAgentConfig attributes are now accessible.")

except ImportError as e:
    print(f"❌ Import error: {e}")
    print("Note: You need to rebuild the Python SDK first:")
    print("  cd crates/code/sdk/python")
    print("  pip install -e .")
except AttributeError as e:
    print(f"❌ Attribute error: {e}")
    print("This means the fix hasn't been applied yet or the SDK wasn't rebuilt.")
except AssertionError as e:
    print(f"❌ Assertion failed: {e}")
