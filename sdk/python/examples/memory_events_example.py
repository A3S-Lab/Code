"""
Memory Events Example

Demonstrates how to listen to memory-related events from the agent:
- Memory stored events
- Memory search events
- Memory recall events
- Memory cleared events
"""

import asyncio
from a3s_code import A3sClient
from a3s_code.types import MemoryItem, MemoryType


async def memory_events_example():
    print("=" * 60)
    print("Memory Events Example")
    print("=" * 60)
    print()

    async with A3sClient(address="localhost:4088") as client:
        # Create a session
        print("1. Creating session...")
        session = await client.create_session(
            name="memory-events-demo",
            workspace="/tmp/memory-events-test",
            system_prompt="You are a helpful coding assistant with memory capabilities.",
        )
        session_id = session["session_id"]
        print(f"✓ Session created: {session_id}")
        print()

        # Track events
        events_received = []

        # Subscribe to events in background
        print("2. Subscribing to events...")

        async def handle_events():
            async for event in client.subscribe_events(session_id=session_id):
                event_type = event.get("type", "")
                events_received.append(event_type)

                if "MEMORY_STORED" in event_type:
                    data = event.get("data", {})
                    print(f"  📝 [MemoryStored] id={data.get('memory_id')}, "
                          f"type={data.get('memory_type')}, "
                          f"importance={data.get('importance')}")

                elif "MEMORIES_SEARCHED" in event_type:
                    data = event.get("data", {})
                    print(f"  🔍 [MemoriesSearched] results={data.get('result_count')}, "
                          f"query={data.get('query')}")

                elif "MEMORY_RECALLED" in event_type:
                    data = event.get("data", {})
                    print(f"  💡 [MemoryRecalled] id={data.get('memory_id')}, "
                          f"relevance={data.get('relevance')}")

                elif "MEMORY_CLEARED" in event_type:
                    data = event.get("data", {})
                    print(f"  🗑️  [MemoryCleared] tier={data.get('tier')}, "
                          f"count={data.get('count')}")

                elif event_type == "EVENT_TYPE_AGENT_END":
                    break

        event_task = asyncio.create_task(handle_events())
        print("✓ Event listener started")
        print()

        # =====================================================================
        # Perform Memory Operations (triggers events)
        # =====================================================================
        print("3. Storing memories (watch for events)...")
        for i in range(3):
            await client.store_memory(
                session_id=session_id,
                memory=MemoryItem(
                    content=f"Test memory {i + 1}: learned about feature #{i + 1}",
                    importance=0.5 + (i * 0.2),
                    tags=["test", f"memory-{i + 1}"],
                    memory_type=MemoryType.EPISODIC,
                ),
            )
            await asyncio.sleep(0.1)  # Small delay to see events
        print()

        print("4. Searching memories (watch for events)...")
        await client.search_memories(
            session_id=session_id,
            tags=["test"],
            limit=10,
        )
        await asyncio.sleep(0.1)
        print()

        print("5. Getting memory statistics...")
        stats_result = await client.get_memory_stats(session_id=session_id)
        stats = stats_result.get("stats")
        if stats:
            print(f"  Long-term: {stats.long_term_count}")
            print(f"  Short-term: {stats.short_term_count}")
            print(f"  Working: {stats.working_count}")
        print()

        print("6. Clearing working memory (watch for events)...")
        await client.clear_memories(
            session_id=session_id,
            clear_long_term=False,
            clear_short_term=False,
            clear_working=True,
        )
        await asyncio.sleep(0.5)
        print()

        # Wait for events to be processed
        try:
            await asyncio.wait_for(event_task, timeout=3.0)
        except asyncio.TimeoutError:
            event_task.cancel()

        # Summary
        print("=" * 40)
        print(f"Event summary:")
        print(f"  Total events received: {len(events_received)}")
        if events_received:
            unique = set(events_received)
            print(f"  Event types: {', '.join(sorted(unique))}")
        print()

        # Clean up
        print("7. Cleaning up...")
        await client.destroy_session(session_id)
        print("✓ Session destroyed")
        print()

        print("=" * 60)
        print("Memory events example complete! ✓")
        print("=" * 60)


if __name__ == "__main__":
    asyncio.run(memory_events_example())
