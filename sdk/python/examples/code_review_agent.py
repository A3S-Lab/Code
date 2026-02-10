"""
Code Review Agent - Complete Example

A comprehensive example that combines multiple features:
- Persistent file storage
- Read-only permissions
- HITL confirmation for git commands
- Task tracking for review items
- Context management
- Provider configuration

This demonstrates how to build a secure, production-ready code review agent.
"""

import asyncio
from datetime import datetime
from a3s_code import A3sClient, StorageType, TimeoutAction, SessionLane


async def create_code_review_agent():
    print("=" * 60)
    print("Code Review Agent - Complete Example")
    print("=" * 60)
    print()

    async with A3sClient(address="localhost:4088") as client:
        try:
            # Step 1: Create persistent session with file storage
            print("Step 1: Creating persistent session...")
            session = await client.create_session(
                name="Code Review Agent",
                workspace="/tmp/code-review-workspace",
                storage_type=StorageType.STORAGE_TYPE_FILE,
                system_prompt="""You are a code review assistant. Your role is to:
- Analyze code for bugs, security issues, and best practices
- Provide constructive feedback
- Track review progress using the task list
- Focus on code quality and maintainability

You have read-only access to the codebase. You can read files, search code, and run git commands to view history, but you cannot modify files or push changes.""",
                max_context_length=200000,
                auto_compact=True
            )

            session_id = session['session_id']
            print(f"✓ Session created: {session_id}")
            print("  Storage: File (persistent)")
            print("  Workspace: /tmp/code-review-workspace")
            print()

            # Step 2: Configure read-only permissions
            print("Step 2: Configuring read-only permissions...")
            await client.set_permission_policy(
                session_id,
                allow_rules=[
                    "Read(*)",
                    "Glob(*)",
                    "Grep(*)",
                    "Bash(git:log*)",
                    "Bash(git:diff*)",
                    "Bash(git:show*)",
                    "Bash(git:status*)",
                    "Bash(git:branch*)",
                    "Bash(ls:*)",
                    "Bash(pwd:*)",
                    "Bash(cat:*)",
                    "Bash(head:*)",
                    "Bash(tail:*)"
                ],
                deny_rules=[
                    "Write(*)",
                    "Edit(*)",
                    "Bash(git:push*)",
                    "Bash(git:commit*)",
                    "Bash(git:reset*)",
                    "Bash(rm:*)",
                    "Bash(sudo:*)"
                ],
                ask_rules=[
                    "Bash(*)"
                ]
            )
            print("✓ Permissions configured:")
            print("  ✓ Read-only access to codebase")
            print("  ✓ Safe git commands allowed")
            print("  ✓ Write operations blocked")
            print()

            # Step 3: Configure HITL for git commands
            print("Step 3: Configuring HITL confirmation...")
            await client.set_confirmation_policy(
                session_id,
                enabled=True,
                auto_approve_tools=["Read", "Glob", "Grep"],
                require_confirm_tools=["Bash"],
                default_timeout_ms=30000,
                timeout_action=TimeoutAction.TIMEOUT_ACTION_REJECT,
                yolo_lanes=[SessionLane.SESSION_LANE_QUERY]
            )
            print("✓ HITL configured:")
            print("  ✓ Auto-approve: Read, Glob, Grep")
            print("  ✓ Require confirm: Bash commands")
            print("  ✓ YOLO lanes: Query (read operations)")
            print()

            # Step 4: Set up review task list
            print("Step 4: Setting up review task list...")
            await client.set_todos(
                session_id,
                [
                    {
                        "id": "1",
                        "content": "Review authentication module for security issues",
                        "status": "pending",
                        "priority": "high"
                    },
                    {
                        "id": "2",
                        "content": "Check for SQL injection vulnerabilities",
                        "status": "pending",
                        "priority": "high"
                    },
                    {
                        "id": "3",
                        "content": "Verify error handling and logging",
                        "status": "pending",
                        "priority": "medium"
                    },
                    {
                        "id": "4",
                        "content": "Review API endpoint security",
                        "status": "pending",
                        "priority": "high"
                    },
                    {
                        "id": "5",
                        "content": "Check code style and best practices",
                        "status": "pending",
                        "priority": "low"
                    }
                ]
            )
            print("✓ Review tasks created:")
            print("  - 3 high priority tasks")
            print("  - 1 medium priority task")
            print("  - 1 low priority task")
            print()

            # Step 5: Start code review
            print("Step 5: Starting code review...")
            print("  Analyzing codebase structure...")

            structure_response = await client.generate(
                session_id,
                messages=[{
                    "role": "user",
                    "content": "Please analyze the codebase structure. List the main directories and files, and identify the authentication module."
                }]
            )

            print("✓ Structure analysis complete")
            print(f"  {structure_response['content'][:200]}...")
            print()

            # Step 6: Review authentication module
            print("Step 6: Reviewing authentication module...")
            auth_review_response = await client.generate(
                session_id,
                messages=[{
                    "role": "user",
                    "content": "Review the authentication module (src/auth/) for security issues. Check for: password hashing, session management, input validation, and SQL injection vulnerabilities. Mark task 1 as in_progress."
                }]
            )

            print("✓ Authentication review complete")
            print(f"  {auth_review_response['content'][:200]}...")
            print()

            # Step 7: Check context usage
            print("Step 7: Monitoring context usage...")
            usage = await client.get_context_usage(session_id)
            usage_percent = (usage['totalTokens'] / 200000) * 100

            print("✓ Context status:")
            print(f"  Tokens: {usage['totalTokens']} / 200000 ({usage_percent:.1f}%)")
            print(f"  Messages: {usage['messageCount']}")

            if usage_percent > 75:
                print("  ⚠️ Context getting full, auto-compact will trigger soon")
            print()

            # Step 8: Get task progress
            print("Step 8: Checking review progress...")
            todos = await client.get_todos(session_id)
            tasks = todos.get('todos', [])

            stats = {
                'total': len(tasks),
                'completed': len([t for t in tasks if t['status'] == 'completed']),
                'in_progress': len([t for t in tasks if t['status'] == 'in_progress']),
                'pending': len([t for t in tasks if t['status'] == 'pending'])
            }

            print("✓ Review progress:")
            print(f"  Total tasks: {stats['total']}")
            print(f"  Completed: {stats['completed']}")
            print(f"  In Progress: {stats['in_progress']}")
            print(f"  Pending: {stats['pending']}")
            print(f"  Progress: {(stats['completed'] / stats['total'] * 100):.1f}%")
            print()

            # Step 9: Generate review summary
            print("Step 9: Generating review summary...")
            summary_response = await client.generate(
                session_id,
                messages=[{
                    "role": "user",
                    "content": "Provide a summary of the code review so far. What issues have been found? What tasks are remaining?"
                }]
            )

            print("✓ Review summary:")
            print(f"  {summary_response['content']}")
            print()

            # Step 10: Session info
            print("Step 10: Session information...")
            print("✓ Session details:")
            print(f"  ID: {session_id}")
            print(f"  Name: {session['config']['name']}")
            print(f"  Storage: File (persists across restarts)")
            print(f"  Workspace: {session['config']['workspace']}")
            print(f"  Created: {datetime.fromtimestamp(session['createdAt']).strftime('%Y-%m-%d %H:%M:%S')}")
            print()

            print("=" * 60)
            print("Code Review Agent Setup Complete")
            print("=" * 60)
            print()
            print("The agent is now ready for code review tasks.")
            print("Key features enabled:")
            print("  ✓ Persistent storage (survives restarts)")
            print("  ✓ Read-only permissions (safe)")
            print("  ✓ HITL confirmation (controlled)")
            print("  ✓ Task tracking (organized)")
            print("  ✓ Auto-context management (efficient)")
            print()
            print(f"Session ID: {session_id}")
            print("You can resume this session later by reconnecting to the agent.")

            return session_id

        except Exception as error:
            print(f"Error: {error}")
            raise


if __name__ == "__main__":
    session_id = asyncio.run(create_code_review_agent())
    print()
    print("✓ Code review agent is ready!")
    print(f"  Session ID: {session_id}")
