"""
Basic Usage Example

Demonstrates:
- Creating a client
- Health check
- Session management
- Basic generation
- Streaming generation
- Message history
"""

import asyncio
from a3s_code import A3sClient


async def basic_usage_example():
    print("=" * 60)
    print("Basic Usage Example")
    print("=" * 60)
    print()

    # Create client
    print("1. Creating A3S client...")
    async with A3sClient(address="localhost:4088") as client:
        print(f"✓ Client created")
        print(f"  Address: {client.address}")
        print()

        # Health check
        print("2. Checking agent health...")
        health = await client.health_check()
        print(f"✓ Health status: {health['status']}")
        print(f"  Message: {health.get('message', 'N/A')}")
        print()

        # Get capabilities
        print("3. Getting agent capabilities...")
        capabilities = await client.get_capabilities()
        print("✓ Capabilities retrieved:")
        if "info" in capabilities:
            info = capabilities["info"]
            print(f"  Agent: {info.get('name')} v{info.get('version')}")
        print(f"  Features: {len(capabilities.get('features', []))}")
        print(f"  Tools: {len(capabilities.get('tools', []))}")
        print()

        # Create a session
        print("4. Creating a session...")
        session = await client.create_session(
            name="basic-demo",
            workspace="/tmp/basic-test",
            system_prompt="You are a helpful assistant.",
            llm={
                "provider": "anthropic",
                "model": "claude-sonnet-4-20250514",
            },
        )
        session_id = session["session_id"]
        print(f"✓ Session created: {session_id}")
        print()

        # List sessions
        print("5. Listing sessions...")
        sessions = await client.list_sessions()
        print(f"✓ Found {len(sessions.get('sessions', []))} sessions")
        print()

        # Get context usage
        print("6. Getting context usage...")
        context_usage = await client.get_context_usage(session_id)
        if "usage" in context_usage:
            usage = context_usage["usage"]
            print("✓ Context usage:")
            print(f"  Total tokens: {usage.get('total_tokens', 0)}")
            print(f"  Messages: {usage.get('message_count', 0)}")
        print()

        # Generate a response
        print("7. Generating a response...")
        response = await client.generate(
            session_id=session_id,
            messages=[{"role": "ROLE_USER", "content": "Say hello in one word"}],
        )
        print("✓ Response received:")
        if "message" in response:
            print(f"  Content: {response['message'].get('content', '')}")
        print(f"  Finish reason: {response.get('finish_reason', 'N/A')}")
        print()

        # Streaming generation
        print("8. Testing streaming generation...")
        print("   Response: ", end="", flush=True)
        async for chunk in client.stream_generate(
            session_id=session_id,
            messages=[{"role": "ROLE_USER", "content": "Count from 1 to 3"}],
        ):
            if chunk.get("type") == "CHUNK_TYPE_CONTENT" and "content" in chunk:
                print(chunk["content"], end="", flush=True)
        print()
        print("✓ Streaming complete")
        print()

        # Get messages
        print("9. Getting message history...")
        messages = await client.get_messages(session_id, limit=10)
        print(f"✓ Retrieved {len(messages.get('messages', []))} messages")
        print()

        # Clean up
        print("10. Destroying session...")
        await client.destroy_session(session_id)
        print("✓ Session destroyed")
        print()

        print("=" * 60)
        print("All tests passed! ✓")
        print("=" * 60)


if __name__ == "__main__":
    asyncio.run(basic_usage_example())
