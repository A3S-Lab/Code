"""
Skill Management Example

Demonstrates:
- Listing available skills
- Loading skills dynamically
- Using skill capabilities
- Unloading skills
"""

import asyncio
from a3s_code import A3sClient


async def skill_management_example():
    print("=" * 60)
    print("Skill Management Example")
    print("=" * 60)
    print()

    async with A3sClient(address="localhost:4088") as client:
        # Create a session
        print("1. Creating session...")
        session = await client.create_session(
            name="skill-demo",
            workspace="/tmp/skill-test",
            system_prompt="You are a helpful assistant with access to skills.",
        )
        session_id = session["session_id"]
        print(f"✓ Session created: {session_id}")
        print()

        # List available skills
        print("2. Listing available skills...")
        skills_list = await client.list_skills()
        skills = skills_list.get("skills", [])
        print(f"✓ Found {len(skills)} skills:")
        for skill in skills:
            print(f"  - {skill.get('name')}: {skill.get('description', 'N/A')}")
        print()

        # Load a skill
        print("3. Loading 'remotion-best-practices' skill...")
        load_result = await client.load_skill(
            session_id=session_id,
            skill_name="remotion-best-practices",
        )
        print(f"✓ Skill loaded: {load_result.get('success')}")
        print(f"  Message: {load_result.get('message', 'N/A')}")
        print()

        # Use the skill in a generation
        print("4. Using the skill...")
        response = await client.generate(
            session_id=session_id,
            messages=[
                {
                    "role": "ROLE_USER",
                    "content": "How do I create a video with Remotion?",
                }
            ],
        )
        print("✓ Response:")
        content = response.get("message", {}).get("content", "")
        print(f"  {content[:200]}...")
        print()

        # List skills again to see loaded skills
        print("5. Listing skills (after loading)...")
        skills_list_after = await client.list_skills(session_id)
        skills_after = skills_list_after.get("skills", [])
        print(f"✓ Session has {len(skills_after)} skills loaded")
        print()

        # Unload the skill
        print("6. Unloading skill...")
        unload_result = await client.unload_skill(
            session_id=session_id,
            skill_name="remotion-best-practices",
        )
        print(f"✓ Skill unloaded: {unload_result.get('success')}")
        print()

        # Clean up
        await client.destroy_session(session_id)
        print("✓ Session destroyed")
        print()

        print("=" * 60)
        print("Skill management example completed! ✓")
        print("=" * 60)


if __name__ == "__main__":
    asyncio.run(skill_management_example())
