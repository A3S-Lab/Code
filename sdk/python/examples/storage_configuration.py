"""
Storage Configuration Example

Demonstrates how to configure session storage types:
- Memory storage (temporary, no persistence)
- File storage (persistent, survives restarts)
"""

import asyncio
from a3s_code import A3sClient, StorageType


async def storage_configuration_example():
    print("=" * 60)
    print("Storage Configuration Example")
    print("=" * 60)
    print()

    async with A3sClient(address="localhost:4088") as client:
        try:
            # Example 1: Create a temporary session with memory storage
            print("1. Creating temporary session (memory storage)...")
            temp_session = await client.create_session(
                name="Temporary Analysis",
                workspace="/tmp/workspace",
                storage_type=StorageType.STORAGE_TYPE_MEMORY,
                system_prompt="You are a code analyzer for temporary tasks."
            )
            print(f"✓ Temporary session created: {temp_session['session_id']}")
            print("  Storage: Memory (no persistence)")
            print()

            # Example 2: Create a persistent session with file storage
            print("2. Creating persistent session (file storage)...")
            persistent_session = await client.create_session(
                name="Long-term Project",
                workspace="/tmp/workspace",
                storage_type=StorageType.STORAGE_TYPE_FILE,
                system_prompt="You are a helpful coding assistant for long-term projects."
            )
            print(f"✓ Persistent session created: {persistent_session['session_id']}")
            print("  Storage: File (persists across restarts)")
            print("  Sessions will be saved to: /tmp/workspace/sessions/")
            print()

            # Example 3: Use the sessions
            print("3. Testing sessions...")

            # Memory session - quick analysis
            print("  Memory session: Quick code analysis...")
            temp_response = await client.generate(
                persistent_session['session_id'],
                messages=[{
                    "role": "user",
                    "content": "Analyze this code: function add(a, b) { return a + b; }"
                }]
            )
            print(f"  ✓ Response: {temp_response['content'][:100]}...")
            print()

            # File session - persistent work
            print("  File session: Starting persistent work...")
            persistent_response = await client.generate(
                persistent_session['session_id'],
                messages=[{
                    "role": "user",
                    "content": "I need help refactoring a large codebase. Let's start by understanding the structure."
                }]
            )
            print(f"  ✓ Response: {persistent_response['content'][:100]}...")
            print()

            # Example 4: List sessions
            print("4. Listing all sessions...")
            sessions = await client.list_sessions()
            print(f"  Total sessions: {len(sessions['sessions'])}")
            for session in sessions['sessions']:
                storage_type = "Memory" if session['config'].get('storage_type') == StorageType.STORAGE_TYPE_MEMORY else "File"
                print(f"  - {session['config']['name']} ({storage_type})")
            print()

            # Example 5: Cleanup
            print("5. Cleanup...")
            print("  Note: Memory sessions are automatically cleaned up")
            print("  File sessions persist and can be resumed after restart")

            # Clean up temporary session
            await client.destroy_session(temp_session['session_id'])
            print("  ✓ Destroyed temporary session")

            # Keep persistent session for demonstration
            print(f"  ✓ Persistent session kept: {persistent_session['session_id']}")
            print()

            print("=" * 60)
            print("Storage Configuration Example Complete")
            print("=" * 60)

        except Exception as error:
            print(f"Error: {error}")
            raise


if __name__ == "__main__":
    asyncio.run(storage_configuration_example())
