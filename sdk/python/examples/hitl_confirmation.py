"""
HITL (Human-in-the-Loop) Confirmation Example

Demonstrates how to configure and handle tool execution confirmations:
- Setting confirmation policies
- Auto-approve vs require-confirm tools
- Handling confirmation requests
- Timeout behavior
"""

import asyncio
from a3s_code import A3sClient, TimeoutAction, SessionLane


async def hitl_confirmation_example():
    print("=" * 60)
    print("HITL (Human-in-the-Loop) Confirmation Example")
    print("=" * 60)
    print()

    async with A3sClient(address="localhost:4088") as client:
        try:
            # Create a session
            print("1. Creating session...")
            session = await client.create_session(
                name="HITL Demo Session",
                workspace="/tmp/workspace",
                system_prompt="You are a helpful assistant that needs user approval for sensitive operations."
            )
            session_id = session['session_id']
            print(f"✓ Session created: {session_id}")
            print()

            # Configure HITL policy
            print("2. Configuring HITL policy...")
            await client.set_confirmation_policy(
                session_id,
                enabled=True,
                auto_approve_tools=["Read", "Glob", "Grep"],
                require_confirm_tools=["Bash", "Write", "Edit"],
                default_timeout_ms=30000,
                timeout_action=TimeoutAction.TIMEOUT_ACTION_REJECT,
                yolo_lanes=[SessionLane.SESSION_LANE_QUERY]
            )
            print("✓ HITL policy configured:")
            print("  - Auto-approve: Read, Glob, Grep")
            print("  - Require confirm: Bash, Write, Edit")
            print("  - Timeout: 30s (reject on timeout)")
            print("  - YOLO lanes: Query")
            print()

            # Subscribe to events to handle confirmations
            print("3. Setting up event handler...")

            async def handle_events():
                """Handle confirmation events in background"""
                try:
                    async for event in client.subscribe_events(session_id, ["ToolExecutionPending"]):
                        if event.get('type') == 'ToolExecutionPending':
                            data = event.get('data', {})
                            tool_name = data.get('toolName')
                            args = data.get('args')
                            confirmation_id = data.get('confirmationId')

                            print(f"\n⚠️  Tool execution pending confirmation:")
                            print(f"  Tool: {tool_name}")
                            print(f"  Args: {args}")
                            print()

                            # Ask user for approval
                            response = input("Approve this tool execution? (y/n): ")
                            approved = response.lower() in ['y', 'yes']

                            # Send confirmation
                            await client.confirm_tool_execution(
                                session_id,
                                confirmation_id,
                                approved=approved,
                                reason="User approved" if approved else "User rejected"
                            )

                            print("✓ Approved" if approved else "✗ Rejected")
                            print()
                except Exception:
                    pass  # Event stream closed

            # Start event handler in background
            event_task = asyncio.create_task(handle_events())
            print("✓ Event handler ready")
            print()

            # Example 4: Test auto-approve (Read operation)
            print("4. Testing auto-approve (Read operation)...")
            print("  This should execute without confirmation")
            read_response = await client.generate(
                session_id,
                messages=[{
                    "role": "user",
                    "content": "Read the file /tmp/test.txt"
                }]
            )
            print("✓ Read operation completed (auto-approved)")
            print()

            # Example 5: Test require-confirm (Bash operation)
            print("5. Testing require-confirm (Bash operation)...")
            print("  This will require your confirmation")
            bash_response = await client.generate(
                session_id,
                messages=[{
                    "role": "user",
                    "content": "Run: ls -la /tmp"
                }]
            )
            print("✓ Bash operation completed")
            print()

            # Example 6: Get current policy
            print("6. Getting current confirmation policy...")
            policy = await client.get_confirmation_policy(session_id)
            print("✓ Current policy:")
            print(f"  Enabled: {policy.get('enabled')}")
            print(f"  Auto-approve tools: {', '.join(policy.get('autoApproveTools', []))}")
            print(f"  Require confirm tools: {', '.join(policy.get('requireConfirmTools', []))}")
            print()

            # Cancel event handler
            event_task.cancel()
            try:
                await event_task
            except asyncio.CancelledError:
                pass

            # Cleanup
            print("7. Cleanup...")
            await client.destroy_session(session_id)
            print("✓ Session destroyed")
            print()

            print("=" * 60)
            print("HITL Example Complete")
            print("=" * 60)

        except Exception as error:
            print(f"Error: {error}")
            raise


if __name__ == "__main__":
    asyncio.run(hitl_confirmation_example())
