"""
Todo/Task Tracking Example

Demonstrates how to use the built-in task tracking system:
- Create and manage task lists
- Track task status (pending/in_progress/completed/cancelled)
- Set task priorities
- Agent interaction with tasks
"""

import asyncio
from a3s_code import A3sClient


async def todo_tracking_example():
    print("=" * 60)
    print("Todo/Task Tracking Example")
    print("=" * 60)
    print()

    async with A3sClient(address="localhost:4088") as client:
        try:
            # Create a session
            print("1. Creating session...")
            session = await client.create_session(
                name="Project Management Session",
                workspace="/tmp/workspace",
                system_prompt="You are a project management assistant that helps track and complete tasks."
            )
            session_id = session['session_id']
            print(f"✓ Session created: {session_id}")
            print()

            # Example 2: Set initial task list
            print("2. Setting initial task list...")
            await client.set_todos(
                session_id,
                [
                    {
                        "id": "1",
                        "content": "Implement user authentication",
                        "status": "in_progress",
                        "priority": "high"
                    },
                    {
                        "id": "2",
                        "content": "Write unit tests for auth module",
                        "status": "pending",
                        "priority": "high"
                    },
                    {
                        "id": "3",
                        "content": "Update API documentation",
                        "status": "pending",
                        "priority": "medium"
                    },
                    {
                        "id": "4",
                        "content": "Refactor database queries",
                        "status": "pending",
                        "priority": "medium"
                    },
                    {
                        "id": "5",
                        "content": "Add logging to error handlers",
                        "status": "pending",
                        "priority": "low"
                    }
                ]
            )
            print("✓ Task list created with 5 tasks")
            print()

            # Example 3: Get and display tasks
            print("3. Getting current task list...")
            todos = await client.get_todos(session_id)
            print(f"✓ Total tasks: {len(todos.get('todos', []))}")
            print()

            print("Current tasks:")
            status_icons = {
                'pending': '⏳',
                'in_progress': '🔄',
                'completed': '✅',
                'cancelled': '❌'
            }
            priority_colors = {
                'high': '🔴',
                'medium': '🟡',
                'low': '🟢'
            }

            for todo in todos.get('todos', []):
                status_icon = status_icons.get(todo['status'], '❓')
                priority_color = priority_colors.get(todo['priority'], '⚪')

                print(f"  {status_icon} [{todo['id']}] {todo['content']}")
                print(f"     Priority: {priority_color} {todo['priority']} | Status: {todo['status']}")
            print()

            # Example 4: Agent queries tasks
            print("4. Agent querying pending tasks...")
            query_response = await client.generate(
                session_id,
                messages=[{
                    "role": "user",
                    "content": "What tasks are currently pending?"
                }]
            )
            print("✓ Agent response:")
            print(f"  {query_response['content'][:200]}...")
            print()

            # Example 5: Update task status
            print("5. Marking task 1 as completed...")
            await client.set_todos(
                session_id,
                [
                    {
                        "id": "1",
                        "content": "Implement user authentication",
                        "status": "completed",
                        "priority": "high"
                    },
                    {
                        "id": "2",
                        "content": "Write unit tests for auth module",
                        "status": "in_progress",
                        "priority": "high"
                    },
                    {
                        "id": "3",
                        "content": "Update API documentation",
                        "status": "pending",
                        "priority": "medium"
                    },
                    {
                        "id": "4",
                        "content": "Refactor database queries",
                        "status": "pending",
                        "priority": "medium"
                    },
                    {
                        "id": "5",
                        "content": "Add logging to error handlers",
                        "status": "pending",
                        "priority": "low"
                    }
                ]
            )
            print("✓ Task statuses updated")
            print()

            # Example 6: Get task statistics
            print("6. Task statistics...")
            todos = await client.get_todos(session_id)
            tasks = todos.get('todos', [])

            stats = {
                'total': len(tasks),
                'completed': len([t for t in tasks if t['status'] == 'completed']),
                'in_progress': len([t for t in tasks if t['status'] == 'in_progress']),
                'pending': len([t for t in tasks if t['status'] == 'pending']),
                'cancelled': len([t for t in tasks if t['status'] == 'cancelled']),
                'high': len([t for t in tasks if t['priority'] == 'high']),
                'medium': len([t for t in tasks if t['priority'] == 'medium']),
                'low': len([t for t in tasks if t['priority'] == 'low'])
            }

            print("✓ Statistics:")
            print(f"  Total tasks: {stats['total']}")
            print(f"  Completed: {stats['completed']}")
            print(f"  In Progress: {stats['in_progress']}")
            print(f"  Pending: {stats['pending']}")
            print(f"  Cancelled: {stats['cancelled']}")
            print()
            print("  By priority:")
            print(f"    High: {stats['high']}")
            print(f"    Medium: {stats['medium']}")
            print(f"    Low: {stats['low']}")
            print()

            # Cleanup
            print("7. Cleanup...")
            await client.destroy_session(session_id)
            print("✓ Session destroyed")
            print()

            print("=" * 60)
            print("Todo/Task Tracking Example Complete")
            print("=" * 60)
            print()
            print("Use cases:")
            print("  ✓ Project management")
            print("  ✓ Sprint planning")
            print("  ✓ Code review checklists")
            print("  ✓ Debugging task lists")
            print("  ✓ Feature implementation tracking")

        except Exception as error:
            print(f"Error: {error}")
            raise


if __name__ == "__main__":
    asyncio.run(todo_tracking_example())
