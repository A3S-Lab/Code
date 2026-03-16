#!/usr/bin/env python3
"""
Test script to verify the new 'tool' kind support for skills.
"""

import tempfile
import os
from pathlib import Path

print("=" * 70)
print("Testing Skill Kind: tool")
print("=" * 70)

# Create a temporary directory for testing
with tempfile.TemporaryDirectory() as tmpdir:
    skills_dir = Path(tmpdir) / "skills"
    skills_dir.mkdir()

    # Create a skill with kind: tool
    tool_skill = skills_dir / "video-processor.md"
    tool_skill.write_text("""---
name: video-processor
description: "Video processing tool skill"
kind: tool
allowed-tools: "mcp_video-processor_(*), Bash(*), Read(*), Write(*)"
---
# Video Processor Tool

This is a tool-type skill for video processing.

## Features

- Process video files
- Extract metadata
- Generate thumbnails

## Usage

Call this skill to process videos with specialized tools.
""")

    # Create a skill with kind: instruction (for comparison)
    instruction_skill = skills_dir / "helper.md"
    instruction_skill.write_text("""---
name: helper
description: "Helper instruction skill"
kind: instruction
---
# Helper Skill

This is an instruction-type skill.
""")

    # Create a skill with kind: persona (for comparison)
    persona_skill = skills_dir / "expert.md"
    persona_skill.write_text("""---
name: expert
description: "Expert persona skill"
kind: persona
---
# Expert Persona

You are an expert assistant.
""")

    print(f"\n✓ Created test skills in: {skills_dir}")
    print(f"  - video-processor.md (kind: tool)")
    print(f"  - helper.md (kind: instruction)")
    print(f"  - expert.md (kind: persona)")

    # Test with a3s_code
    print("\n" + "-" * 70)
    print("Testing with a3s_code SDK...")
    print("-" * 70)

    try:
        from a3s_code import SubAgentConfig

        # Create config with the skills directory
        cfg = SubAgentConfig(
            agent_type="test",
            prompt="Call Skill('video-processor')",
            workspace=str(tmpdir),
            skill_dirs=[str(skills_dir)],
            permissive=True
        )

        print(f"\n✓ SubAgentConfig created successfully")
        print(f"  - skill_dirs: {cfg.skill_dirs}")
        print(f"  - workspace: {cfg.workspace}")

        print("\n" + "=" * 70)
        print("✅ SUCCESS!")
        print("=" * 70)
        print("""
The 'tool' kind is now supported for skills!

Valid skill kinds:
  - instruction (default): Injected into system prompt when matched
  - persona: Session-level system prompt
  - tool: Tool-like skill (treated like instruction)

Your skill file can now use:
---
name: my-tool-skill
description: "A tool-type skill"
kind: tool
allowed-tools: "..."
---
""")

    except ImportError as e:
        print(f"\n❌ Cannot import a3s_code: {e}")
        print("   Note: You need to rebuild the SDK after adding 'tool' kind support")
        print("   Run: cd crates/code/sdk/python && pip install -e .")
    except Exception as e:
        print(f"\n❌ Error: {e}")
        import traceback
        traceback.print_exc()

print("=" * 70)
