/**
 * Memory Events Example
 *
 * This example demonstrates how to listen to memory-related events
 * from the A3S Code Agent.
 */

import { CodeAgentClient } from '../../src/index.js';

async function main() {
  // Connect to the agent
  const client = new CodeAgentClient('localhost:50051');

  try {
    // Initialize the agent
    console.log('Initializing agent...');
    await client.initialize({ workspace: '/tmp/memory-events-demo' });

    // Create a session
    console.log('Creating session...');
    const session = await client.createSession({
      name: 'memory-events-demo',
      systemPrompt: 'You are a helpful coding assistant with memory capabilities.',
    });
    const sessionId = session.sessionId;
    console.log(`Session created: ${sessionId}`);

    // Example: Listen to Memory Events
    console.log('\n=== Memory Events Monitoring ===');
    console.log('Subscribing to events...');

    // Subscribe to events
    const eventStream = client.subscribeEvents({ sessionId });

    // Track events
    const eventsReceived: string[] = [];

    // Handle events in background
    const eventPromise = (async () => {
      for await (const event of eventStream) {
        const eventType = event.eventType;
        eventsReceived.push(eventType);

        switch (eventType) {
          case 'memory_stored': {
            const memoryId = event.data?.memory_id || 'unknown';
            const memoryType = event.data?.memory_type || 'unknown';
            const importance = event.data?.importance || 0.0;
            const tags = event.data?.tags || '[]';
            console.log('\n  📝 [MemoryStored]');
            console.log(`     ID: ${memoryId}`);
            console.log(`     Type: ${memoryType}`);
            console.log(`     Importance: ${importance}`);
            console.log(`     Tags: ${tags}`);
            break;
          }

          case 'memories_searched': {
            const resultCount = event.data?.result_count || 0;
            const query = event.data?.query;
            const tags = event.data?.tags || '[]';
            console.log('\n  🔍 [MemoriesSearched]');
            console.log(`     Results: ${resultCount}`);
            if (query) {
              console.log(`     Query: ${query}`);
            }
            if (tags !== '[]') {
              console.log(`     Tags: ${tags}`);
            }
            break;
          }

          case 'memory_recalled': {
            const memoryId = event.data?.memory_id || 'unknown';
            const relevance = event.data?.relevance || 0.0;
            console.log('\n  💡 [MemoryRecalled]');
            console.log(`     ID: ${memoryId}`);
            console.log(`     Relevance: ${relevance}`);
            break;
          }

          case 'memory_cleared': {
            const tier = event.data?.tier || 'unknown';
            const count = event.data?.count || 0;
            console.log('\n  🗑️  [MemoryCleared]');
            console.log(`     Tier: ${tier}`);
            console.log(`     Count: ${count}`);
            break;
          }

          case 'agent_end':
            console.log('\n  ✅ [AgentEnd] Processing completed');
            break;
        }
      }
    })();

    // Perform memory operations to trigger events
    console.log('\n--- Performing Memory Operations ---');

    // 1. Store memories
    console.log('\n1. Storing memories...');
    try {
      for (let i = 0; i < 3; i++) {
        const memory = await client.storeMemory({
          sessionId,
          memory: {
            content: `Test memory ${i + 1}`,
            importance: 0.5 + i * 0.2,
            tags: ['test', `memory-${i + 1}`],
            memoryType: 'MEMORY_TYPE_EPISODIC',
          },
        });
        console.log(`   Stored: ${memory.memoryId}`);
        await new Promise((resolve) => setTimeout(resolve, 100)); // Small delay
      }
    } catch (error: any) {
      if (error.code !== 12) {
        // 12 = UNIMPLEMENTED
        console.error(`   Error: ${error}`);
      }
    }

    // 2. Search memories
    console.log('\n2. Searching memories...');
    try {
      const searchResponse = await client.searchMemories({
        sessionId,
        tags: ['test'],
        limit: 10,
      });
      console.log(`   Found ${searchResponse.totalCount} memories`);
      await new Promise((resolve) => setTimeout(resolve, 100));
    } catch (error: any) {
      if (error.code !== 12) {
        console.error(`   Error: ${error}`);
      }
    }

    // 3. Get memory statistics
    console.log('\n3. Getting memory statistics...');
    try {
      const statsResponse = await client.getMemoryStats({ sessionId });
      console.log(`   Long-term: ${statsResponse.stats?.longTermCount}`);
      console.log(`   Short-term: ${statsResponse.stats?.shortTermCount}`);
      console.log(`   Working: ${statsResponse.stats?.workingCount}`);
    } catch (error: any) {
      if (error.code !== 12) {
        console.error(`   Error: ${error}`);
      }
    }

    // 4. Clear memories
    console.log('\n4. Clearing working memory...');
    try {
      const clearResponse = await client.clearMemories({
        sessionId,
        clearLongTerm: false,
        clearShortTerm: false,
        clearWorking: true,
      });
      console.log(`   Cleared ${clearResponse.clearedCount} memories`);
      await new Promise((resolve) => setTimeout(resolve, 100));
    } catch (error: any) {
      if (error.code !== 12) {
        console.error(`   Error: ${error}`);
      }
    }

    // Wait for events to be processed
    await new Promise((resolve) => setTimeout(resolve, 1000));

    // Summary
    console.log('\n--- Event Summary ---');
    console.log(`Total events received: ${eventsReceived.length}`);
    console.log(`Event types: ${[...new Set(eventsReceived)].join(', ')}`);

    // Clean up
    console.log('\n=== Cleanup ===');
    await client.destroySession({ sessionId });
    console.log('Session destroyed');

    await client.shutdown({});
    console.log('Agent shutdown complete');
  } catch (error) {
    console.error('Error:', error);
    process.exit(1);
  }
}

main();
