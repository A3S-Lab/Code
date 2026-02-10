/**
 * Memory System Demo
 *
 * Demonstrates how to use the Memory System APIs:
 * - storeMemory() - Store memory items
 * - retrieveMemory() - Retrieve memories by ID
 * - searchMemories() - Search memories
 * - getMemoryStats() - Get memory statistics
 * - clearMemories() - Clear memories
 */

import { A3sClient, StorageType, MemoryItem } from '@a3s-lab/code';

async function main() {
  const client = new A3sClient({ address: 'localhost:4088' });

  try {
    // Create a session
    console.log('=== Creating Session ===\n');
    const session = await client.createSession({
      name: 'memory-demo',
      workspace: '/tmp/workspace',
      storageType: StorageType.STORAGE_TYPE_MEMORY,
    });
    const sessionId = session.sessionId;
    console.log(`Session created: ${sessionId}\n`);

    // Example 1: Store Memories
    console.log('=== Example 1: Store Memories ===\n');

    const memories: Partial<MemoryItem>[] = [
      {
        content: 'User prefers TypeScript over JavaScript',
        importance: 0.8,
        tags: ['preference', 'language'],
        memoryType: 'MEMORY_TYPE_SEMANTIC',
        metadata: { category: 'user-preference' },
      },
      {
        content: 'Successfully implemented JWT authentication on 2026-02-05',
        importance: 0.9,
        tags: ['achievement', 'authentication'],
        memoryType: 'MEMORY_TYPE_EPISODIC',
        metadata: { project: 'todo-api', date: '2026-02-05' },
      },
      {
        content: 'How to implement async/await in TypeScript',
        importance: 0.7,
        tags: ['knowledge', 'typescript', 'async'],
        memoryType: 'MEMORY_TYPE_PROCEDURAL',
        metadata: { topic: 'async-programming' },
      },
    ];

    const storedMemoryIds: string[] = [];

    for (const memory of memories) {
      const response = await client.storeMemory(sessionId, {
        id: '',
        content: memory.content || '',
        timestamp: Date.now(),
        importance: memory.importance || 0.5,
        tags: memory.tags || [],
        memoryType: memory.memoryType || 'MEMORY_TYPE_UNKNOWN',
        metadata: memory.metadata || {},
        accessCount: 0,
      });

      if (response.success) {
        storedMemoryIds.push(response.memoryId);
        console.log(`✓ Stored: ${memory.content?.substring(0, 50)}...`);
        console.log(`  Memory ID: ${response.memoryId}`);
      }
    }
    console.log('\n');

    // Example 2: Get Memory Statistics
    console.log('=== Example 2: Memory Statistics ===\n');

    const statsResponse = await client.getMemoryStats(sessionId);
    if (statsResponse.stats) {
      console.log('Memory Statistics:');
      console.log(`  Long-term memories: ${statsResponse.stats.longTermCount}`);
      console.log(`  Short-term memories: ${statsResponse.stats.shortTermCount}`);
      console.log(`  Working memories: ${statsResponse.stats.workingCount}`);
    }
    console.log('\n');

    // Example 3: Search Memories
    console.log('=== Example 3: Search Memories ===\n');

    // Search by tag
    const searchResponse = await client.searchMemories(
      sessionId,
      undefined,
      ['typescript'],
      10,
      false,
      0.5
    );

    console.log(`Found ${searchResponse.totalCount} memories with tag "typescript":`);
    searchResponse.memories.forEach((memory, i) => {
      console.log(`  ${i + 1}. ${memory.content}`);
      console.log(`     Importance: ${memory.importance}`);
      console.log(`     Tags: ${memory.tags.join(', ')}`);
      console.log(`     Type: ${memory.memoryType}`);
    });
    console.log('\n');

    // Example 4: Retrieve Memory by ID
    console.log('=== Example 4: Retrieve Memory by ID ===\n');

    if (storedMemoryIds.length > 0) {
      const memoryId = storedMemoryIds[0];
      const retrieveResponse = await client.retrieveMemory(sessionId, memoryId);

      if (retrieveResponse.memory) {
        const memory = retrieveResponse.memory;
        console.log('Retrieved Memory:');
        console.log(`  ID: ${memory.id}`);
        console.log(`  Content: ${memory.content}`);
        console.log(`  Importance: ${memory.importance}`);
        console.log(`  Tags: ${memory.tags.join(', ')}`);
        console.log(`  Type: ${memory.memoryType}`);
        console.log(`  Access Count: ${memory.accessCount}`);
      }
    }
    console.log('\n');

    // Example 5: Search with Query
    console.log('=== Example 5: Search with Query ===\n');

    const querySearchResponse = await client.searchMemories(
      sessionId,
      'authentication',
      undefined,
      5,
      true,
      0.7
    );

    console.log(`Found ${querySearchResponse.totalCount} memories matching "authentication":`);
    querySearchResponse.memories.forEach((memory, i) => {
      console.log(`  ${i + 1}. ${memory.content}`);
      console.log(`     Importance: ${memory.importance}`);
    });
    console.log('\n');

    // Example 6: Clear Memories
    console.log('=== Example 6: Clear Memories ===\n');

    const clearResponse = await client.clearMemories(
      sessionId,
      false, // Don't clear long-term
      true,  // Clear short-term
      true   // Clear working
    );

    console.log('Clear Memories Result:');
    console.log(`  Success: ${clearResponse.success}`);
    console.log(`  Cleared Count: ${clearResponse.clearedCount}`);
    console.log('\n');

    // Cleanup
    console.log('=== Cleanup ===\n');
    await client.destroySession(sessionId);
    console.log('Session destroyed');

  } catch (error) {
    console.error('Error:', error);
  } finally {
    client.close();
  }
}

main();
