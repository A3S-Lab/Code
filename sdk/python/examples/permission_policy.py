"""
Permission Policy Example

Demonstrates:
- Setting permission policies
- Allow/deny specific tool executions
- Checking permissions before execution
- Adding permission rules dynamically
"""

import asyncio
from a3s_code import A3sClient


async def permission_policy_example():
    print("=" * 60)
    print("Permission Policy Example")
    print("=" * 60)
    print()

    async with A3sClient(address="localhost:4088") as client:
        # Create a session
        print("1. Creating session...")
        session = await client.create_session(
            name="permission-demo",
            workspace="/tmp/permission-test",
            system_prompt="You are a helpful assistant.",
        )
        session_id = session["session_id"]
        print(f"✓ Session created: {session_id}")
        print()

        # Set a permission policy
        print("2. Setting permission policy...")
        await client.set_permission_policy(
            session_id=session_id,
            policy={
                "default_decision": "PERMISSION_DECISION_ASK",
                "rules": [
                    {
                        "pattern": "read(*)",
                        "decision": "PERMISSION_DECISION_ALLOW",
                        "description": "Allow all read operations",
                    },
                    {
                        "pattern": "bash(rm:*)",
                        "decision": "PERMISSION_DECISION_DENY",
                        "description": "Deny all rm commands",
                    },
                    {
                        "pattern": "write(*)",
                        "decision": "PERMISSION_DECISION_ASK",
                        "description": "Ask before writing files",
                    },
                ],
            },
        )
        print("✓ Permission policy set")
        print()

        # Get the policy
        print("3. Getting permission policy...")
        policy_result = await client.get_permission_policy(session_id)
        policy = policy_result.get("policy", {})
        print("✓ Current policy:")
        print(f"  Default decision: {policy.get('default_decision')}")
        rules = policy.get("rules", [])
        print(f"  Rules: {len(rules)}")
        for rule in rules:
            print(f"    - {rule.get('pattern')}: {rule.get('decision')}")
        print()

        # Check specific permissions
        print("4. Checking permissions...")

        read_check = await client.check_permission(
            session_id=session_id,
            tool_name="read",
            tool_args={"path": "/tmp/test.txt"},
        )
        print(f"  read(/tmp/test.txt): {read_check.get('decision')}")

        rm_check = await client.check_permission(
            session_id=session_id,
            tool_name="bash",
            tool_args={"command": "rm -rf /"},
        )
        print(f"  bash(rm -rf /): {rm_check.get('decision')}")

        write_check = await client.check_permission(
            session_id=session_id,
            tool_name="write",
            tool_args={"path": "/tmp/output.txt"},
        )
        print(f"  write(/tmp/output.txt): {write_check.get('decision')}")
        print()

        # Add a new rule dynamically
        print("5. Adding new permission rule...")
        await client.add_permission_rule(
            session_id=session_id,
            rule={
                "pattern": "bash(echo:*)",
                "decision": "PERMISSION_DECISION_ALLOW",
                "description": "Allow echo commands",
            },
        )
        print("✓ Rule added")
        print()

        # Test with actual generation
        print("6. Testing with generation (allowed operation)...")
        response = await client.generate(
            session_id=session_id,
            messages=[
                {
                    "role": "ROLE_USER",
                    "content": "Read the file /tmp/test.txt",
                }
            ],
        )
        print("✓ Generation completed (read allowed)")
        print()

        # Clean up
        await client.destroy_session(session_id)
        print("✓ Session destroyed")
        print()

        print("=" * 60)
        print("Permission policy example completed! ✓")
        print("=" * 60)


if __name__ == "__main__":
    asyncio.run(permission_policy_example())
