"""
Context Management Example

Demonstrates how to manage session context and token usage:
- Monitor context usage
- Compact context when approaching limits
- Clear context for fresh starts
- Auto-compact configuration
"""

import asyncio
from a3s_code import A3sClient


def format_tokens(tokens: int) -> str:
    """Format token counts for display"""
    if tokens >= 1000000:
        return f"{tokens / 1000000:.2f}M"
    elif tokens >= 1000:
        return f"{tokens / 1000:.1f}K"
    return str(tokens)


def progress_bar(current: int, max_val: int, width: int = 30) -> str:
    """Create a progress bar"""
    percentage = min(current / max_val, 1.0)
    filled = int(percentage * width)
    empty = width - filled
    bar = '█' * filled + '░' * empty
    return f"[{bar}] {percentage * 100:.1f}%"


async def context_management_example():
    print("=" * 60)
    print("Context Management Example")
    print("=" * 60)
    print()

    async with A3sClient(address="localhost:4088") as client:
        try:
            # Example 1: Create session with auto-compact enabled
            print("1. Creating session with auto-compact...")
            session = await client.create_session(
                name="Context Management Demo",
                workspace="/tmp/workspace",
                max_context_length=200000,
                auto_compact=True,
                system_prompt="You are a helpful assistant. Keep track of our conversation context."
            )
            session_id = session['session_id']
            print(f"✓ Session created: {session_id}")
            print("  Max context: 200K tokens")
            print("  Auto-compact: enabled")
            print()

            # Example 2: Check initial context usage
            print("2. Checking initial context usage...")
            usage = await client.get_context_usage(session_id)
            print("✓ Initial context usage:")
            print(f"  Total tokens: {format_tokens(usage['totalTokens'])}")
            print(f"  Prompt tokens: {format_tokens(usage['promptTokens'])}")
            print(f"  Completion tokens: {format_tokens(usage['completionTokens'])}")
            print(f"  Message count: {usage['messageCount']}")
            print(f"  Usage: {progress_bar(usage['totalTokens'], 200000)}")
            print()

            # Example 3: Generate some conversation to build up context
            print("3. Building up conversation context...")
            conversations = [
                "Tell me about the history of programming languages.",
                "What are the key differences between functional and object-oriented programming?",
                "Explain the concept of design patterns in software engineering.",
                "What are microservices and how do they differ from monolithic architectures?",
                "Describe the principles of clean code and why they matter."
            ]

            for i, content in enumerate(conversations):
                print(f"  [{i + 1}/{len(conversations)}] Sending message...")
                await client.generate(
                    session_id,
                    messages=[{"role": "user", "content": content}]
                )

                # Check usage after each message
                usage = await client.get_context_usage(session_id)
                print(f"      Tokens: {format_tokens(usage['totalTokens'])} {progress_bar(usage['totalTokens'], 200000, 20)}")
            print()

            # Example 4: Monitor context usage
            print("4. Current context status...")
            usage = await client.get_context_usage(session_id)
            usage_percent = (usage['totalTokens'] / 200000) * 100

            print("✓ Context usage:")
            print(f"  Total tokens: {format_tokens(usage['totalTokens'])} / 200K")
            print(f"  Messages: {usage['messageCount']}")
            print(f"  Usage: {progress_bar(usage['totalTokens'], 200000)}")
            print()

            if usage_percent > 75:
                print("⚠️  Context is getting full (>75%)")
            elif usage_percent > 50:
                print("ℹ️  Context is moderately used (>50%)")
            else:
                print("✓ Context has plenty of room (<50%)")
            print()

            # Example 5: Manual context compaction
            print("5. Manually compacting context...")
            print("  Before compaction:")
            print(f"    Tokens: {format_tokens(usage['totalTokens'])}")
            print(f"    Messages: {usage['messageCount']}")

            compact_result = await client.compact_context(session_id)
            print("  After compaction:")
            print(f"    Tokens: {format_tokens(compact_result['after']['totalTokens'])}")
            print(f"    Messages: {compact_result['after']['messageCount']}")
            saved = compact_result['before']['totalTokens'] - compact_result['after']['totalTokens']
            print(f"    Saved: {format_tokens(saved)} tokens")
            print()

            # Example 6: Continue conversation after compaction
            print("6. Continuing conversation after compaction...")
            response = await client.generate(
                session_id,
                messages=[{
                    "role": "user",
                    "content": "Can you summarize what we discussed earlier?"
                }]
            )
            print("✓ Agent can still recall context:")
            print(f"  {response['content'][:200]}...")
            print()

            # Example 7: Clear context completely
            print("7. Clearing context completely...")
            print("  Before clear:")
            usage = await client.get_context_usage(session_id)
            print(f"    Tokens: {format_tokens(usage['totalTokens'])}")
            print(f"    Messages: {usage['messageCount']}")

            await client.clear_context(session_id)

            print("  After clear:")
            usage = await client.get_context_usage(session_id)
            print(f"    Tokens: {format_tokens(usage['totalTokens'])}")
            print(f"    Messages: {usage['messageCount']}")
            print("  Note: System prompt is preserved")
            print()

            # Example 8: Context monitoring demonstration
            print("8. Context monitoring demonstration...")
            print("  Setting up monitoring (checking every 2 seconds)...")

            for i in range(3):
                await asyncio.sleep(2)

                usage = await client.get_context_usage(session_id)
                percent = (usage['totalTokens'] / 200000) * 100

                print(f"  [Monitor] Tokens: {format_tokens(usage['totalTokens'])} ({percent:.1f}%)")

                # Auto-compact if over 90%
                if percent > 90:
                    print("  [Monitor] ⚠️ Context over 90%, auto-compacting...")
                    await client.compact_context(session_id)
            print()

            # Cleanup
            print("9. Cleanup...")
            await client.destroy_session(session_id)
            print("✓ Session destroyed")
            print()

            print("=" * 60)
            print("Context Management Example Complete")
            print("=" * 60)
            print()
            print("Best practices:")
            print("  ✓ Enable auto-compact for long-running sessions")
            print("  ✓ Monitor context usage regularly")
            print("  ✓ Compact at 75-80% to avoid hitting limits")
            print("  ✓ Clear context for completely fresh starts")
            print("  ✓ Use appropriate max_context_length for your model")

        except Exception as error:
            print(f"Error: {error}")
            raise


if __name__ == "__main__":
    asyncio.run(context_management_example())
