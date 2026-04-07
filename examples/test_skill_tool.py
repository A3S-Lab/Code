#!/usr/bin/env python3
"""
Test Skill Tool with Kimi Model

This script tests the Skill tool implementation by:
1. Creating a test skill with limited permissions (read, grep)
2. Creating an agent with Skill(*) permission only
3. Invoking the skill through the Skill tool
4. Verifying permission isolation works
"""

import asyncio
import os
import sys
import tempfile
from pathlib import Path

# Add the SDK to path
sdk_path = Path(__file__).parent.parent / "sdk" / "python"
sys.path.insert(0, str(sdk_path))

from a3s_code import Agent

async def main():
    print("=" * 80)
    print("Skill Tool Test with Kimi Model")
    print("=" * 80)

    # Set up Kimi API credentials from environment
    if "KIMI_API_KEY" not in os.environ:
        raise RuntimeError("KIMI_API_KEY environment variable not set")
    if "KIMI_BASE_URL" not in os.environ:
        raise RuntimeError("KIMI_BASE_URL environment variable not set")

    # Create a temporary workspace with test files
    with tempfile.TemporaryDirectory() as tmpdir:
        workspace = Path(tmpdir)

        # Create test files
        test_file = workspace / "test.txt"
        test_file.write_text("Hello from Skill Tool test!\nThis is line 2.\nThis is line 3.")

        readme = workspace / "README.md"
        readme.write_text("# Test Project\n\nThis is a test project for Skill tool.")

        # Create a test skill
        skills_dir = workspace / "skills"
        skills_dir.mkdir()

        skill_file = skills_dir / "file-reader.md"
        skill_file.write_text("""---
name: file-reader
description: Read and analyze files
allowed-tools: read(*), grep(*)
---

# File Reader Skill

You are a file reading specialist. You can:
- Read files using the read tool
- Search for patterns using grep
- Analyze and summarize file contents

You CANNOT:
- Write files
- Execute bash commands
- Edit files
""")

        print(f"\n📁 Workspace: {workspace}")
        print(f"📄 Test files created:")
        print(f"   - test.txt")
        print(f"   - README.md")
        print(f"   - skills/file-reader.md")

        # Create agent with Kimi model
        print("\n🤖 Creating agent with Kimi model...")
        agent = Agent.create(str(Path(__file__).parent / "agent_kimi.hcl"))

        # Create session with the test skill
        print("📝 Creating session with file-reader skill...")
        session = agent.session(
            str(workspace),
            skill_dirs=[str(skills_dir)]
        )

        print("\n" + "=" * 80)
        print("Test 1: Invoke skill to read a file")
        print("=" * 80)

        try:
            print("\n💬 Prompt: Use the file-reader skill to read test.txt")
            result = await session.send(
                "Use the file-reader skill to read test.txt and tell me what it contains"
            )
            print(f"\n✅ Response:\n{result.text}")
            print(f"\n📊 Tool calls: {result.tool_calls_count}")
            print(f"📊 Tokens: {result.prompt_tokens} prompt + {result.completion_tokens} completion")
        except Exception as e:
            print(f"\n❌ Error: {e}")
            import traceback
            traceback.print_exc()

        print("\n" + "=" * 80)
        print("Test 2: Invoke skill to search in files")
        print("=" * 80)

        try:
            print("\n💬 Prompt: Use the file-reader skill to search for 'test' in all files")
            result = await session.send(
                "Use the file-reader skill to search for the word 'test' in all files"
            )
            print(f"\n✅ Response:\n{result.text}")
            print(f"\n📊 Tool calls: {result.tool_calls_count}")
            print(f"📊 Tokens: {result.prompt_tokens} prompt + {result.completion_tokens} completion")
        except Exception as e:
            print(f"\n❌ Error: {e}")
            import traceback
            traceback.print_exc()

        print("\n" + "=" * 80)
        print("Test 3: Try to write file (should fail - skill doesn't have write permission)")
        print("=" * 80)

        try:
            print("\n💬 Prompt: Use the file-reader skill to write a summary to output.txt")
            result = await session.send(
                "Use the file-reader skill to write a summary of test.txt to output.txt"
            )
            print(f"\n✅ Response:\n{result.text}")
            print(f"\n📊 Tool calls: {result.tool_calls_count}")

            # Check if output.txt was created (it shouldn't be)
            output_file = workspace / "output.txt"
            if output_file.exists():
                print("\n⚠️  WARNING: output.txt was created (permission isolation may not be working)")
            else:
                print("\n✅ Permission isolation working: output.txt was not created")
        except Exception as e:
            print(f"\n❌ Error: {e}")
            import traceback
            traceback.print_exc()

        print("\n" + "=" * 80)
        print("Test Complete!")
        print("=" * 80)

if __name__ == "__main__":
    asyncio.run(main())
