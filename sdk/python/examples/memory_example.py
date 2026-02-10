"""
Memory System Example

This example demonstrates how to use the memory system
features of the A3S Code Agent.
"""

import asyncio
from a3s_code import CodeAgentClient


async def main():
    # Connect to the agent
    client = CodeAgentClient('localhost:50051')

    try:
        # Initialize the agent
        print('Initializing agent...')
        await client.initialize(workspace='/tmp/memory-demo')

        # Create a session
        print('Creating session...')
        session = await client.create_session(
            name='memory-demo',
            system_prompt='You are a helpful coding assistant with memory capabilities.'
        )
        session_id = session.session_id
        print(f'Session created: {session_id}')

        # Example 1: Store memories
        print('\n=== Example 1: Store Memories ===')
        try:
            # Store a success memory
            success_memory = await client.store_memory(
                session_id=session_id,
                memory={
                    'content': 'Successfully created a REST API with Flask and JWT authentication',
                    'importance': 0.9,
                    'tags': ['success', 'api', 'authentication', 'flask'],
                    'memory_type': 'MEMORY_TYPE_PROCEDURAL',
                    'metadata': {
                        'project': 'rest-api',
                        'tools': 'write,bash',
                        'duration': '30min'
                    }
                }
            )
            print(f'  Stored success memory: {success_memory.memory_id}')

            # Store a failure memory
            failure_memory = await client.store_memory(
                session_id=session_id,
                memory={
                    'content': 'Failed to connect to database: Connection refused on port 5432',
                    'importance': 0.8,
                    'tags': ['failure', 'database', 'connection'],
                    'memory_type': 'MEMORY_TYPE_EPISODIC',
                    'metadata': {
                        'error': 'ECONNREFUSED',
                        'solution': 'Check if PostgreSQL is running'
                    }
                }
            )
            print(f'  Stored failure memory: {failure_memory.memory_id}')

            # Store a fact memory
            fact_memory = await client.store_memory(
                session_id=session_id,
                memory={
                    'content': 'Flask uses decorators to define routes',
                    'importance': 0.7,
                    'tags': ['fact', 'flask', 'routing'],
                    'memory_type': 'MEMORY_TYPE_SEMANTIC'
                }
            )
            print(f'  Stored fact memory: {fact_memory.memory_id}')

        except Exception as e:
            if 'UNIMPLEMENTED' in str(e):
                print('  (Memory storage RPC not yet implemented - stub returned)')
            else:
                raise

        # Example 2: Search memories
        print('\n=== Example 2: Search Memories ===')
        try:
            search_response = await client.search_memories(
                session_id=session_id,
                query='API authentication',
                limit=5
            )
            print(f'  Found {search_response.total_count} memories:')
            for i, memory in enumerate(search_response.memories, 1):
                content_preview = memory.content[:60] + '...' if len(memory.content) > 60 else memory.content
                print(f'    {i}. [{memory.memory_type}] {content_preview}')
                print(f'       Importance: {memory.importance}, Tags: {", ".join(memory.tags)}')
        except Exception as e:
            if 'UNIMPLEMENTED' in str(e):
                print('  (Memory search RPC not yet implemented - stub returned)')
            else:
                raise

        # Example 3: Search by tags
        print('\n=== Example 3: Search by Tags ===')
        try:
            tag_search_response = await client.search_memories(
                session_id=session_id,
                tags=['success', 'api'],
                limit=10
            )
            print(f'  Found {tag_search_response.total_count} memories with tags [success, api]:')
            for i, memory in enumerate(tag_search_response.memories, 1):
                content_preview = memory.content[:60] + '...' if len(memory.content) > 60 else memory.content
                print(f'    {i}. {content_preview}')
        except Exception as e:
            if 'UNIMPLEMENTED' in str(e):
                print('  (Memory search RPC not yet implemented - stub returned)')
            else:
                raise

        # Example 4: Get memory statistics
        print('\n=== Example 4: Memory Statistics ===')
        try:
            stats_response = await client.get_memory_stats(session_id=session_id)
            print('  Memory Statistics:')
            print(f'    Long-term: {stats_response.stats.long_term_count} memories')
            print(f'    Short-term: {stats_response.stats.short_term_count} memories')
            print(f'    Working: {stats_response.stats.working_count} memories')
        except Exception as e:
            if 'UNIMPLEMENTED' in str(e):
                print('  (Memory stats RPC not yet implemented - stub returned)')
            else:
                raise

        # Example 5: Retrieve specific memory
        print('\n=== Example 5: Retrieve Memory ===')
        try:
            retrieve_response = await client.retrieve_memory(
                session_id=session_id,
                memory_id='memory-123'
            )
            if retrieve_response.memory:
                print('  Retrieved memory:')
                print(f'    Content: {retrieve_response.memory.content}')
                print(f'    Type: {retrieve_response.memory.memory_type}')
                print(f'    Importance: {retrieve_response.memory.importance}')
                print(f'    Access count: {retrieve_response.memory.access_count}')
            else:
                print('  Memory not found')
        except Exception as e:
            if 'UNIMPLEMENTED' in str(e):
                print('  (Memory retrieval RPC not yet implemented - stub returned)')
            else:
                raise

        # Example 6: Use memory in generation
        print('\n=== Example 6: Generate with Memory Context ===')
        print('Generating response with memory context...')

        # First, search for relevant memories
        try:
            relevant_memories = await client.search_memories(
                session_id=session_id,
                query='Flask API',
                limit=3
            )

            # Use memories as context in generation
            context_prompt = 'Based on past experiences:\n'
            for i, memory in enumerate(relevant_memories.memories, 1):
                context_prompt += f'{i}. {memory.content}\n'
            context_prompt += '\nNow, create a new Flask API endpoint for user profile.'

            response = await client.generate(
                session_id=session_id,
                prompt=context_prompt
            )
            print(f'Response: {response.text[:200]}...')

        except Exception as e:
            if 'UNIMPLEMENTED' in str(e):
                print('  (Memory features not yet implemented - using regular generation)')
                response = await client.generate(
                    session_id=session_id,
                    prompt='Create a new Flask API endpoint for user profile.'
                )
                print(f'Response: {response.text[:200]}...')
            else:
                raise

        # Example 7: Clear memories
        print('\n=== Example 7: Clear Memories ===')
        try:
            clear_response = await client.clear_memories(
                session_id=session_id,
                clear_long_term=False,
                clear_short_term=True,
                clear_working=True
            )
            print(f'  Cleared {clear_response.cleared_count} memories')
            print('  (Long-term memories preserved)')
        except Exception as e:
            if 'UNIMPLEMENTED' in str(e):
                print('  (Memory clearing RPC not yet implemented - stub returned)')
            else:
                raise

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
