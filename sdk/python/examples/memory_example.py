"""
Memory System Example

Demonstrates how to use the memory system for persistent agent knowledge:
- Storing memories (episodic, semantic, procedural)
- Searching memories by query and tags
- Retrieving specific memories
- Memory statistics
- Using memories as context for generation
- Clearing memories by tier
"""

import asyncio
from a3s_code import A3sClient
from a3s_code.types import MemoryItem, MemoryType


async def memory_example():
    print("=" * 60)
    print("Memory System Example")
    print("=" * 60)
    print()

    async with A3sClient(address="localhost:4088") as client:
        # Create a session
        print("1. Creating session...")
        session = await client.create_session(
            name="memory-demo",
            workspace="/tmp/memory-test",
            system_prompt="You are a helpful coding assistant with memory capabilities.",
        )
        session_id = session["session_id"]
        print(f"✓ Session created: {session_id}")
        print()

        # =====================================================================
        # Store Memories
        # =====================================================================
        print("2. Storing memories...")

        # Procedural memory: how to do something
        result = await client.store_memory(
            session_id=session_id,
            memory=MemoryItem(
                content="Successfully created a REST API with Flask and JWT authentication",
                importance=0.9,
                tags=["success", "api", "authentication", "flask"],
                memory_type=MemoryType.PROCEDURAL,
                metadata={
                    "project": "rest-api",
                    "tools": "write,bash",
                    "duration": "30min",
                },
            ),
        )
        print(f"✓ Stored procedural memory: {result.get('memory_id', 'ok')}")

        # Episodic memory: what happened
        result = await client.store_memory(
            session_id=session_id,
            memory=MemoryItem(
                content="Failed to connect to database: Connection refused on port 5432",
                importance=0.8,
                tags=["failure", "database", "connection"],
                memory_type=MemoryType.EPISODIC,
                metadata={
                    "error": "ECONNREFUSED",
                    "solution": "Check if PostgreSQL is running",
                },
            ),
        )
        print(f"✓ Stored episodic memory: {result.get('memory_id', 'ok')}")

        # Semantic memory: factual knowledge
        result = await client.store_memory(
            session_id=session_id,
            memory=MemoryItem(
                content="Flask uses decorators to define routes. Use @app.route('/path') for GET.",
                importance=0.7,
                tags=["fact", "flask", "routing"],
                memory_type=MemoryType.SEMANTIC,
            ),
        )
        print(f"✓ Stored semantic memory: {result.get('memory_id', 'ok')}")
        print()

        # =====================================================================
        # Search Memories
        # =====================================================================
        print("3. Searching memories by query...")
        search_result = await client.search_memories(
            session_id=session_id,
            query="API authentication",
            limit=5,
        )
        memories = search_result.get("memories", [])
        total = search_result.get("total_count", len(memories))
        print(f"✓ Found {total} memories:")
        for i, mem in enumerate(memories, 1):
            content = mem.content if hasattr(mem, "content") else str(mem)
            preview = content[:60] + "..." if len(content) > 60 else content
            mtype = mem.memory_type.name if hasattr(mem, "memory_type") else "?"
            print(f"  {i}. [{mtype}] {preview}")
        print()

        # Search by tags
        print("4. Searching memories by tags...")
        tag_result = await client.search_memories(
            session_id=session_id,
            tags=["success", "api"],
            limit=10,
        )
        tag_memories = tag_result.get("memories", [])
        print(f"✓ Found {len(tag_memories)} memories with tags [success, api]")
        print()

        # =====================================================================
        # Retrieve Specific Memory
        # =====================================================================
        print("5. Retrieving a specific memory...")
        if memories:
            first_id = memories[0].memory_id if hasattr(memories[0], "memory_id") else ""
            if first_id:
                retrieved = await client.retrieve_memory(
                    session_id=session_id,
                    memory_id=first_id,
                )
                mem = retrieved.get("memory")
                if mem:
                    print(f"✓ Retrieved memory:")
                    print(f"  Content: {mem.content}")
                    print(f"  Type: {mem.memory_type.name}")
                    print(f"  Importance: {mem.importance}")
                    print(f"  Access count: {mem.access_count}")
                else:
                    print("  Memory not found")
            else:
                print("  (No memory ID available)")
        else:
            print("  (No memories to retrieve)")
        print()

        # =====================================================================
        # Memory Statistics
        # =====================================================================
        print("6. Getting memory statistics...")
        stats_result = await client.get_memory_stats(session_id=session_id)
        stats = stats_result.get("stats")
        if stats:
            print(f"✓ Memory statistics:")
            print(f"  Long-term: {stats.long_term_count} memories")
            print(f"  Short-term: {stats.short_term_count} memories")
            print(f"  Working: {stats.working_count} memories")
        else:
            print("  (Stats not available)")
        print()

        # =====================================================================
        # Use Memories in Generation
        # =====================================================================
        print("7. Using memories as context for generation...")
        relevant = await client.search_memories(
            session_id=session_id,
            query="Flask API",
            limit=3,
        )
        relevant_memories = relevant.get("memories", [])

        context_parts = ["Based on past experiences:"]
        for i, mem in enumerate(relevant_memories, 1):
            content = mem.content if hasattr(mem, "content") else str(mem)
            context_parts.append(f"{i}. {content}")
        context_parts.append("\nNow, create a new Flask API endpoint for user profile.")

        response = await client.generate(
            session_id=session_id,
            messages=[{"role": "ROLE_USER", "content": "\n".join(context_parts)}],
        )
        if "message" in response:
            content = response["message"].get("content", "")
            print(f"✓ Response (with memory context): {content[:200]}...")
        print()

        # =====================================================================
        # Clear Memories
        # =====================================================================
        print("8. Clearing short-term and working memories...")
        clear_result = await client.clear_memories(
            session_id=session_id,
            clear_long_term=False,
            clear_short_term=True,
            clear_working=True,
        )
        print(f"✓ Cleared {clear_result.get('cleared_count', 0)} memories")
        print("  (Long-term memories preserved)")
        print()

        # Clean up
        print("9. Cleaning up...")
        await client.destroy_session(session_id)
        print("✓ Session destroyed")
        print()

        print("=" * 60)
        print("Memory example complete! ✓")
        print("=" * 60)


if __name__ == "__main__":
    asyncio.run(memory_example())
