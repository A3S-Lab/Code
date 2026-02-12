/**
 * Memory Events Example
 *
 * Demonstrates how to listen to memory-related events from the agent:
 * - Memory stored events
 * - Memory search events
 * - Memory recall events
 * - Memory cleared events
 */

import { A3sClient } from '@a3s-lab/code';

async function memoryEventsExample(): Promise<void> {
  console.log('='.repeat(60));
  console.log('Memory Events Example');
  console.log('='.repeat(60));
  console.log();

  const client = new A3sClient({
    address: process.env.A3S_ADDRESS || 'localhost:4088',
  });

  try {
    // Create a session
    console.log('1. Creating session...');
    const session = await client.createSession({
      name: 'memory-events-demo',
      workspace: '/tmp/memory-events-test',
      systemPrompt: 'You are a helpful coding assistant with memory capabilities.',
    });
    const sessionId = session.sessionId;
    console.log(`✓ Session created: ${sessionId}`);
    console.log();

    // Track events
    const eventsReceived: string[] = [];

    // Subscribe to events in background
    console.log('2. Subscribing to events...');
    const eventPromise = (async () => {
      for await (const event of client.subscribeEvents(sessionId)) {
        const eventType = event.type || '';
        eventsReceived.push(eventType);

        if (eventType.includes('MEMORY_STORED')) {
          console.log(`  📝 [MemoryStored] id=${event.data?.memoryId}, type=${event.data?.memoryType}`);
        } else if (eventType.includes('MEMORIES_SEARCHED')) {
          console.log(`  🔍 [MemoriesSearched] results=${event.data?.resultCount}, query=${event.data?.query}`);
        } else if (eventType.includes('MEMORY_RECALLED')) {
          console.log(`  💡 [MemoryRecalled] id=${event.data?.memoryId}, relevance=${event.data?.relevance}`);
        } else if (eventType.includes('MEMORY_CLEARED')) {
          console.log(`  🗑️  [MemoryCleared] tier=${event.data?.tier}, count=${event.data?.count}`);
        } else if (eventType === 'EVENT_TYPE_AGENT_END') {
          break;
        }
      }
    })();

    console.log('✓ Event listener started');
    console.log();

    // Perform memory operations (triggers events)
    console.log('3. Storing memories (watch for events)...');
    for (let i = 0; i < 3; i++) {
      await client.storeMemory(sessionId, {
        content: `Test memory ${i + 1}: learned about feature #${i + 1}`,
        importance: 0.5 + i * 0.2,
        tags: ['test', `memory-${i + 1}`],
        memoryType: 'MEMORY_TYPE_EPISODIC',
      });
      await new Promise((r) => setTimeout(r, 100));
    }
    console.log();

    console.log('4. Searching memories (watch for events)...');
    await client.searchMemories(sessionId, { tags: ['test'], limit: 10 });
    await new Promise((r) => setTimeout(r, 100));
    console.log();

    console.log('5. Getting memory statistics...');
    const stats = await client.getMemoryStats(sessionId);
    if (stats.stats) {
      console.log(`  Long-term: ${stats.stats.longTermCount}`);
      console.log(`  Short-term: ${stats.stats.shortTermCount}`);
      console.log(`  Working: ${stats.stats.workingCount}`);
    }
    console.log();

    console.log('6. Clearing working memory (watch for events)...');
    await client.clearMemories(sessionId, {
      clearLongTerm: false,
      clearShortTerm: false,
      clearWorking: true,
    });
    await new Promise((r) => setTimeout(r, 500));
    console.log();

    // Wait for events with timeout
    await Promise.race([
      eventPromise,
      new Promise((r) => setTimeout(r, 3000)),
    ]);

    // Summary
    console.log('='.repeat(40));
    console.log('Event summary:');
    console.log(`  Total events received: ${eventsReceived.length}`);
    if (eventsReceived.length > 0) {
      const unique = [...new Set(eventsReceived)];
      console.log(`  Event types: ${unique.sort().join(', ')}`);
    }
    console.log();

    // Clean up
    console.log('7. Cleaning up...');
    await client.destroySession(sessionId);
    console.log('✓ Session destroyed');
    console.log();

    console.log('='.repeat(60));
    console.log('Memory events example complete! ✓');
    console.log('='.repeat(60));
  } catch (error) {
    console.error('Error:', error);
  }
}

memoryEventsExample();
