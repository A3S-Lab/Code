/**
 * Memory System Example
 *
 * Demonstrates how to use the memory system for persistent agent knowledge:
 * - Storing memories (episodic, semantic, procedural)
 * - Searching memories by query and tags
 * - Retrieving specific memories
 * - Memory statistics
 * - Using memories as context for generation
 * - Clearing memories by tier
 */

import { A3sClient } from '@a3s-lab/code';

async function memoryExample(): Promise<void> {
  console.log('='.repeat(60));
  console.log('Memory System Example');
  console.log('='.repeat(60));
  console.log();

  const client = new A3sClient({
    address: process.env.A3S_ADDRESS || 'localhost:4088',
  });

  try {
    // Create a session
    console.log('1. Creating session...');
    const session = await client.createSession({
      name: 'memory-demo',
      workspace: '/tmp/memory-test',
      systemPrompt: 'You are a helpful coding assistant with memory capabilities.',
    });
    const sessionId = session.sessionId;
    console.log(`✓ Session created: ${sessionId}`);
    console.log();

    // Store memories
    console.log('2. Storing memories...');

    const successMemory = await client.storeMemory(sessionId, {
      content: 'Successfully created a REST API with Express and JWT authentication',
      importance: 0.9,
      tags: ['success', 'api', 'authentication', 'express'],
      memoryType: 'MEMORY_TYPE_PROCEDURAL',
      metadata: { project: 'rest-api', tools: 'write,bash', duration: '30min' },
    });
    console.log(`✓ Stored procedural memory: ${successMemory.memoryId || 'ok'}`);

    const failureMemory = await client.storeMemory(sessionId, {
      content: 'Failed to connect to database: Connection refused on port 5432',
      importance: 0.8,
      tags: ['failure', 'database', 'connection'],
      memoryType: 'MEMORY_TYPE_EPISODIC',
      metadata: { error: 'ECONNREFUSED', solution: 'Check if PostgreSQL is running' },
    });
    console.log(`✓ Stored episodic memory: ${failureMemory.memoryId || 'ok'}`);

    const factMemory = await client.storeMemory(sessionId, {
      content: 'Express middleware functions have access to req, res, and next',
      importance: 0.7,
      tags: ['fact', 'express', 'middleware'],
      memoryType: 'MEMORY_TYPE_SEMANTIC',
    });
    console.log(`✓ Stored semantic memory: ${factMemory.memoryId || 'ok'}`);
    console.log();

    // Search memories
    console.log('3. Searching memories by query...');
    const searchResponse = await client.searchMemories(sessionId, {
      query: 'API authentication',
      limit: 5,
    });
    console.log(`✓ Found ${searchResponse.totalCount || 0} memories:`);
    for (const [i, memory] of (searchResponse.memories || []).entries()) {
      const preview = (memory.content || '').substring(0, 60) + '...';
      console.log(`  ${i + 1}. [${memory.memoryType}] ${preview}`);
    }
    console.log();

    // Search by tags
    console.log('4. Searching memories by tags...');
    const tagSearch = await client.searchMemories(sessionId, {
      tags: ['success', 'api'],
      limit: 10,
    });
    console.log(`✓ Found ${tagSearch.totalCount || 0} memories with tags [success, api]`);
    console.log();

    // Memory statistics
    console.log('5. Getting memory statistics...');
    const stats = await client.getMemoryStats(sessionId);
    if (stats.stats) {
      console.log('✓ Memory statistics:');
      console.log(`  Long-term: ${stats.stats.longTermCount} memories`);
      console.log(`  Short-term: ${stats.stats.shortTermCount} memories`);
      console.log(`  Working: ${stats.stats.workingCount} memories`);
    }
    console.log();

    // Retrieve specific memory
    console.log('6. Retrieving a specific memory...');
    const memories = searchResponse.memories || [];
    if (memories.length > 0 && memories[0].memoryId) {
      const retrieved = await client.retrieveMemory(sessionId, memories[0].memoryId);
      if (retrieved.memory) {
        console.log('✓ Retrieved memory:');
        console.log(`  Content: ${retrieved.memory.content}`);
        console.log(`  Type: ${retrieved.memory.memoryType}`);
        console.log(`  Importance: ${retrieved.memory.importance}`);
      }
    } else {
      console.log('  (No memories to retrieve)');
    }
    console.log();

    // Use memories in generation
    console.log('7. Using memories as context for generation...');
    const relevant = await client.searchMemories(sessionId, {
      query: 'Express API',
      limit: 3,
    });
    let contextPrompt = 'Based on past experiences:\n';
    for (const [i, memory] of (relevant.memories || []).entries()) {
      contextPrompt += `${i + 1}. ${memory.content}\n`;
    }
    contextPrompt += '\nNow, create a new Express API endpoint for user profile.';

    const response = await client.generate(sessionId, [
      { role: 'ROLE_USER', content: contextPrompt },
    ]);
    if (response.message?.content) {
      console.log(`✓ Response: ${response.message.content.substring(0, 200)}...`);
    }
    console.log();

    // Clear memories
    console.log('8. Clearing short-term and working memories...');
    const clearResult = await client.clearMemories(sessionId, {
      clearLongTerm: false,
      clearShortTerm: true,
      clearWorking: true,
    });
    console.log(`✓ Cleared ${clearResult.clearedCount || 0} memories`);
    console.log('  (Long-term memories preserved)');
    console.log();

    // Clean up
    console.log('9. Cleaning up...');
    await client.destroySession(sessionId);
    console.log('✓ Session destroyed');
    console.log();

    console.log('='.repeat(60));
    console.log('Memory example complete! ✓');
    console.log('='.repeat(60));
  } catch (error) {
    console.error('Error:', error);
  }
}

memoryExample();
