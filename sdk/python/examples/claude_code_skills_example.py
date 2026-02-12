"""
Claude Code Skills Compatibility Example

Demonstrates how to use Claude Code skills with A3S Code Agent:
- Loading skills with frontmatter (name, description, allowed-tools)
- Getting Claude Code skills
- Using skills in generation
"""

import asyncio
from a3s_code import A3sClient


async def claude_code_skills_example():
    print("=" * 60)
    print("Claude Code Skills Example")
    print("=" * 60)
    print()

    async with A3sClient(address="localhost:4088") as client:
        # Create a session
        print("1. Creating session...")
        session = await client.create_session(
            name="claude-skills-demo",
            workspace="/tmp/claude-skills-test",
            system_prompt="You are a helpful assistant.",
        )
        session_id = session["session_id"]
        print(f"✓ Session created: {session_id}")
        print()

        # =====================================================================
        # Load Claude Code Skills
        # =====================================================================
        print("2. Loading GitHub commands skill...")
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
            session_id=session_id,
            skill_name="github-commands",
            skill_content=github_skill,
        )
        print(f"✓ Loaded skill: {result}")
        print()

        print("3. Loading code review skill...")
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
            session_id=session_id,
            skill_name="code-review",
            skill_content=code_review_skill,
        )
        print(f"✓ Loaded skill: {result}")
        print()

        # =====================================================================
        # Get Claude Code Skills
        # =====================================================================
        print("4. Getting all Claude Code skills...")
        skills_response = await client.get_claude_code_skills()
        skills = skills_response.get("skills", [])
        print(f"✓ Found {len(skills)} Claude Code skills:")
        for skill in skills:
            name = skill.name if hasattr(skill, "name") else skill.get("name", "?")
            desc = skill.description if hasattr(skill, "description") else skill.get("description", "")
            print(f"  - {name}: {desc}")
            allowed = skill.allowed_tools if hasattr(skill, "allowed_tools") else skill.get("allowed_tools")
            if allowed:
                print(f"    Allowed tools: {allowed}")
            disabled = skill.disable_model_invocation if hasattr(skill, "disable_model_invocation") else False
            if disabled:
                print(f"    Model invocation disabled")
        print()

        # Get a specific skill by name
        print("5. Getting specific skill...")
        specific = await client.get_claude_code_skills(name="github-commands")
        specific_skills = specific.get("skills", [])
        if specific_skills:
            skill = specific_skills[0]
            content = skill.content if hasattr(skill, "content") else skill.get("content", "")
            print(f"✓ GitHub skill content preview:")
            print(f"  {content[:100]}...")
        print()

        # =====================================================================
        # Use Skill in Generation
        # =====================================================================
        print("6. Using skill in generation...")
        response = await client.generate(
            session_id=session_id,
            messages=[
                {
                    "role": "ROLE_USER",
                    "content": "List the open issues in this repository",
                }
            ],
        )
        if "message" in response:
            content = response["message"].get("content", "")
            print(f"✓ Response: {content[:200]}...")
        print()

        # =====================================================================
        # List All Skills (including non-Claude Code)
        # =====================================================================
        print("7. Listing all loaded skills...")
        all_skills = await client.list_skills()
        skills_list = all_skills.get("skills", [])
        print(f"✓ Total skills: {len(skills_list)}")
        for s in skills_list:
            print(f"  - {s.get('name', '?')}: {s.get('description', '')[:60]}")
        print()

        # =====================================================================
        # Unload Skills
        # =====================================================================
        print("8. Unloading skills...")
        await client.unload_skill(session_id, "github-commands")
        print("✓ Unloaded: github-commands")
        await client.unload_skill(session_id, "code-review")
        print("✓ Unloaded: code-review")
        print()

        # Clean up
        print("9. Cleaning up...")
        await client.destroy_session(session_id)
        print("✓ Session destroyed")
        print()

        print("=" * 60)
        print("Claude Code skills example complete! ✓")
        print("=" * 60)


if __name__ == "__main__":
    asyncio.run(claude_code_skills_example())
