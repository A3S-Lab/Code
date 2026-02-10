#!/usr/bin/env python3
"""
Claude Code Skills Compatibility Example

Demonstrates how to use Claude Code skills with A3S Code Agent.
Claude Code skills are prompt-based skills with optional tool permissions.
"""

import asyncio
from a3s_code import CodeAgentClient


async def main():
    """Main example function."""
    async with CodeAgentClient() as client:
        # Initialize the agent
        await client.initialize(workspace="/tmp/claude-skills-demo")

        # Create a session
        session = await client.create_session()
        print(f"Created session: {session.session_id}")

        # Load a Claude Code skill (GitHub commands)
        github_skill = """---
name: github-commands
description: GitHub CLI commands for repository management
allowed-tools: Bash(gh:*)
---

Use the `gh` CLI for all GitHub operations:

1. For issues: `gh issue list`, `gh issue view <number>`
2. For PRs: `gh pr list`, `gh pr view <number>`, `gh pr create`
3. For repos: `gh repo view`, `gh repo clone`

Always prefer `gh` over direct API calls or web scraping.
"""

        result = await client.load_skill(
            session_id=session.session_id,
            skill_name="github-commands",
            skill_content=github_skill
        )
        print(f"Loaded skill with tools: {result.tool_names}")

        # Load a code review skill (Claude Code format)
        code_review_skill = """---
name: code-review
description: Code review a pull request
allowed-tools: Bash(gh issue view:*), Bash(gh pr:*), Read(*)
disable-model-invocation: false
---

Provide a code review for the given pull request.

Steps:
1. Check if the PR is open and not a draft
2. Read the PR diff using `gh pr diff`
3. Review for bugs, style issues, and CLAUDE.md compliance
4. Comment on the PR with findings
"""

        result = await client.load_skill(
            session_id=session.session_id,
            skill_name="code-review",
            skill_content=code_review_skill
        )
        print(f"Loaded code-review skill")

        # Get all Claude Code skills
        skills_response = await client.get_claude_code_skills()
        print(f"\nLoaded Claude Code skills ({len(skills_response.skills)}):")
        for skill in skills_response.skills:
            print(f"  - {skill.name}: {skill.description}")
            if skill.allowed_tools:
                print(f"    Allowed tools: {skill.allowed_tools}")
            if skill.disable_model_invocation:
                print(f"    Model invocation disabled")

        # Get a specific skill by name
        specific_skill = await client.get_claude_code_skills(name="github-commands")
        if specific_skill.skills:
            skill = specific_skill.skills[0]
            print(f"\nGitHub skill content preview:")
            print(f"  {skill.content[:100]}...")

        # Use the skill in a generation request
        response = await client.generate(
            session_id=session.session_id,
            prompt="List the open issues in this repository"
        )
        print(f"\nGeneration response: {response.content[:200]}...")

        # Clean up
        await client.destroy_session(session.session_id)
        print("\nSession destroyed")


if __name__ == "__main__":
    asyncio.run(main())
