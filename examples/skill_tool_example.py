#!/usr/bin/env python3
"""
Skill Tool Example - Demonstrates skill-based tool access control

This example shows how the Skill tool enforces permission isolation:
1. Agent has Skill(*) permission but NOT direct tool permissions
2. Agent invokes Skill("data-processor") tool
3. Skill's allowed-tools (read, grep) are temporarily granted
4. After skill execution, permissions are revoked
5. Agent cannot bypass skill to directly access read/grep

This implements the design from GitHub issue #8:
https://github.com/A3S-Lab/Code/issues/8
"""

import asyncio
from a3s_code import Agent, AgentConfig, PermissionPolicy, PermissionRule

async def main():
    # Create a skill with limited tool access
    skill_content = """
# Data Processor Skill

You are a data processing specialist. You can:
- Read files to analyze data
- Search for patterns using grep
- Process and summarize information

You CANNOT:
- Write files
- Execute bash commands
- Access the network
"""

    # Create agent with Skill(*) permission only
    # Agent cannot directly call read/grep, must go through skills
    config = AgentConfig(
        permission_policy=PermissionPolicy(
            allow=[PermissionRule("Skill(*)")],
            deny=[PermissionRule("read(*)"), PermissionRule("grep(*)")],
            default_decision="deny"
        )
    )

    agent = Agent(config=config)

    # Register the data-processor skill
    agent.register_skill(
        name="data-processor",
        description="Process and analyze data files",
        allowed_tools="read(*), grep(*)",
        content=skill_content
    )

    # Create a session
    session = agent.session(".")

    # Example 1: Agent tries to directly read a file (should be denied)
    print("=== Example 1: Direct tool access (should fail) ===")
    try:
        response = await session.send("Read the README.md file")
        print(f"Response: {response}")
    except Exception as e:
        print(f"Expected error: {e}")

    # Example 2: Agent invokes skill to read file (should succeed)
    print("\n=== Example 2: Skill-based access (should succeed) ===")
    response = await session.send(
        "Use the data-processor skill to read and summarize README.md"
    )
    print(f"Response: {response}")

    # Example 3: Skill tries to write file (should be denied)
    print("\n=== Example 3: Skill exceeds permissions (should fail) ===")
    response = await session.send(
        "Use the data-processor skill to write a summary to output.txt"
    )
    print(f"Response: {response}")

if __name__ == "__main__":
    asyncio.run(main())
