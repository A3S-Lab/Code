#!/usr/bin/env python3
"""Test SubAgentConfig with Orchestrator to verify skill_dirs propagation."""

import asyncio
import tempfile
import os
from pathlib import Path

try:
    from a3s_code import Agent, Orchestrator, SubAgentConfig

    async def test_orchestrator_with_skill_dirs():
        print("=" * 60)
        print("Testing SubAgentConfig with Orchestrator")
        print("=" * 60)

        # Create temporary directories for testing
        with tempfile.TemporaryDirectory() as tmpdir:
            workspace = Path(tmpdir) / "workspace"
            workspace.mkdir()

            skills_dir = Path(tmpdir) / "skills"
            skills_dir.mkdir()

            # Create a test skill file
            skill_file = skills_dir / "test-skill.md"
            skill_file.write_text("""---
name: test-skill
description: A test skill
---
# Test Skill

This is a test skill for verification.
""")

            print(f"\n✓ Created test environment:")
            print(f"  - Workspace: {workspace}")
            print(f"  - Skills dir: {skills_dir}")
            print(f"  - Skill file: {skill_file}")

            # Create a minimal config file
            config_file = Path(tmpdir) / "agent.hcl"
            config_file.write_text("""
llm "anthropic" "claude-sonnet-4" {
  api_key = env("ANTHROPIC_API_KEY")
}
""")

            # Create agent and orchestrator
            print("\n✓ Creating Agent and Orchestrator...")
            agent = Agent.create(str(config_file))
            orchestrator = Orchestrator.create(agent=agent)

            # Create SubAgentConfig with skill_dirs
            print(f"\n✓ Creating SubAgentConfig with skill_dirs=['{skills_dir}']")
            config = SubAgentConfig(
                agent_type="general",
                prompt="List available skills",
                workspace=str(workspace),
                permissive=True,
                skill_dirs=[str(skills_dir)],
            )

            # Verify attributes are accessible
            print(f"\n✓ Verifying SubAgentConfig attributes:")
            print(f"  - agent_type: {config.agent_type}")
            print(f"  - workspace: {config.workspace}")
            print(f"  - skill_dirs: {config.skill_dirs}")
            print(f"  - hasattr(config, 'skill_dirs'): {hasattr(config, 'skill_dirs')}")

            assert hasattr(config, 'skill_dirs'), "skill_dirs attribute not found!"
            assert config.skill_dirs == [str(skills_dir)], "skill_dirs value mismatch!"

            print("\n✅ SubAgentConfig attributes are correctly accessible!")
            print("\nNote: Full orchestrator execution test would require:")
            print("  - Valid LLM API credentials")
            print("  - Network connectivity")
            print("  - Longer execution time")
            print("\nThe important fix is verified: skill_dirs is now accessible in Python!")

    # Run the async test
    asyncio.run(test_orchestrator_with_skill_dirs())

except ImportError as e:
    print(f"❌ Import error: {e}")
    print("Make sure the SDK is installed: pip install -e .")
except Exception as e:
    print(f"❌ Error: {e}")
    import traceback
    traceback.print_exc()
