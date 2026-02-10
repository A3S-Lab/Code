"""
Memory Events Example

This example demonstrates how to listen to memory-related events
from the A3S Code Agent.
"""

import asyncio
from a3s_code import CodeAgentClient


async def main():
    # Connect to the agent
    client = CodeAgentClient('localhost:50051')

    try:
        # Initialize the agent
        print('Initializing agent...')
        await client.initialize(workspace='/tmp/memory-events-demo')

        # Create a session
        print('Creating session...')
        session = await client.create_session(
            name='memory-events-demo',
            system_prompt='You are a helpful coding assistant with memory capabilities.'
        )
        session_id = session.session_id
        print(f'Session created: {session_id}')

        # Example: Listen to Memory Events
        print('\n=== Memory Events Monitoring ===')
        print('Subscribing to events...')

        # Subscribe to events
        event_stream = client.subscribe_events(session_id=session_id)

        # Track events
        events_received = []

        # Handle events in background
        async def handle_events():
            async for event in event_stream:
                event_type = event.event_type
                events_received.append(event_type)

                if event_type == 'memory_stored':
                    memory_id = event.data.get('memory_id', 'unknown')
                    memory_type = event.data.get('memory_type', 'unknown')
                    importance = event.data.get('importance', 0.0)
                    tags = event.data.get('tags', '[]')
                    print(f'\n  📝 [MemoryStored]')
                    print(f'     ID: {memory_id}')
                    print(f'     Type: {memory_type}')
                    print(f'     Importance: {importance}')
                    print(f'     Tags: {tags}')

                elif event_type == 'memories_searched':
                    result_count = event.data.get('result_count', 0)
                    query = event.data.get('query', None)
                    tags = event.data.get('tags', '[]')
                    print(f'\n  🔍 [MemoriesSearched]')
                    print(f'     Results: {result_count}')
                    if query:
                        print(f'     Query: {query}')
                    if tags != '[]':
                        print(f'     Tags: {tags}')

                elif event_type == 'memory_recalled':
                    memory_id = event.data.get('memory_id', 'unknown')
                    relevance = event.data.get('relevance', 0.0)
                    print(f'\n  💡 [MemoryRecalled]')
                    print(f'     ID: {memory_id}')
                    print(f'     Relevance: {relevance}')

                elif event_type == 'memory_cleared':
                    tier = event.data.get('tier', 'unknown')
                    count = event.data.get('count', 0)
                    print(f'\n  🗑️  [MemoryCleared]')
                    print(f'     Tier: {tier}')
                    print(f'     Count: {count}')

                elif event_type == 'agent_end':
                    print('\n  ✅ [AgentEnd] Processing completed')
                    break

        # Start event handler
        event_task = asyncio.create_task(handle_events())

        # Perform memory operations to trigger events
        print('\n--- Performing Memory Operations ---')

        # 1. Store memories
        print('\n1. Storing memories...')
        try:
            for i in range(3):
                memory = await client.store_memory(
                    session_id=session_id,
                    memory={
                        'content': f'Test memory {i + 1}',
                        'importance': 0.5 + (i * 0.2),
                        'tags': ['test', f'memory-{i + 1}'],
                        'memory_type': 'MEMORY_TYPE_EPISODIC',
                    }
                )
                print(f'   Stored: {memory.memory_id}')
                await asyncio.sleep(0.1)  # Small delay to see events
        except Exception as e:
            if 'UNIMPLEMENTED' not in str(e):
                print(f'   Error: {e}')

        # 2. Search memories
        print('\n2. Searching memories...')
        try:
            search_response = await client.search_memories(
                session_id=session_id,
                tags=['test'],
                limit=10
            )
            print(f'   Found {search_response.total_count} memories')
            await asyncio.sleep(0.1)
        except Exception as e:
            if 'UNIMPLEMENTED' not in str(e):
                print(f'   Error: {e}')

        # 3. Get memory statistics
        print('\n3. Getting memory statistics...')
        try:
            stats_response = await client.get_memory_stats(session_id=session_id)
            print(f'   Long-term: {stats_response.stats.long_term_count}')
            print(f'   Short-term: {stats_response.stats.short_term_count}')
            print(f'   Working: {stats_response.stats.working_count}')
        except Exception as e:
            if 'UNIMPLEMENTED' not in str(e):
                print(f'   Error: {e}')

        # 4. Clear memories
        print('\n4. Clearing working memory...')
        try:
            clear_response = await client.clear_memories(
                session_id=session_id,
                clear_long_term=False,
                clear_short_term=False,
                clear_working=True
            )
            print(f'   Cleared {clear_response.cleared_count} memories')
            await asyncio.sleep(0.1)
        except Exception as e:
            if 'UNIMPLEMENTED' not in str(e):
                print(f'   Error: {e}')

        # Wait for events to be processed
        await asyncio.sleep(1)

        # Summary
        print('\n--- Event Summary ---')
        print(f'Total events received: {len(events_received)}')
        print(f'Event types: {set(events_received)}')

        # Clean up
        print('\n=== Cleanup ===')
        await client.destroy_session(session_id=session_id)
        print('Session destroyed')

        await client.shutdown()
        print('Agent shutdown complete')

    except Exception as error:
        print(f'Error: {error}')
        raise


if __name__ == '__main__':
    asyncio.run(main())
