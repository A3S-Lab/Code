/**
 * Memory System Example
 *
 * This example demonstrates how to use the memory system
 * features of the A3S Code Agent.
 */

import { CodeAgentClient } from '../../src/index.js';

async function main() {
  // Connect to the agent
  const client = new CodeAgentClient('localhost:50051');

  try {
    // Initialize the agent
    console.log('Initializing agent...');
    await client.initialize({ workspace: '/tmp/memory-demo' });

    // Create a session
    console.log('Creating session...');
    const session = await client.createSession({
      name: 'memory-demo',
      systemPrompt: 'You are a helpful coding assistant with memory capabilities.',
    });
    const sessionId = session.sessionId;
    console.log(`Session created: ${sessionId}`);

    // Example 1: Store memories
    console.log('\n=== Example 1: Store Memories ===');
    try {
      // Store a success memory
      const successMemory = await client.storeMemory({
        sessionId,
        memory: {
          content: 'Successfully created a REST API with Express and JWT authentication',
          importance: 0.9,
          tags: ['success', 'api', 'authentication', 'express'],
          memoryType: 'MEMORY_TYPE_PROCEDURAL',
          metadata: {
            project: 'rest-api',
            tools: 'write,bash',
            duration: '30min',
          },
        },
      });
      console.log(`  Stored success memory: ${successMemory.memoryId}`);

      // Store a failure memory
      const failureMemory = await client.storeMemory({
        sessionId,
        memory: {
          content: 'Failed to connect to database: Connection refused on port 5432',
          importance: 0.8,
          tags: ['failure', 'database', 'connection'],
          memoryType: 'MEMORY_TYPE_EPISODIC',
          metadata: {
            error: 'ECONNREFUSED',
            solution: 'Check if PostgreSQL is running',
          },
        },
      });
      console.log(`  Stored failure memory: ${failureMemory.memoryId}`);

      // Store a fact memory
      const factMemory = await client.storeMemory({
        sessionId,
        memory: {
          content: 'Express middleware functions have access to req, res, and next',
          importance: 0.7,
          tags: ['fact', 'express', 'middleware'],
          memoryType: 'MEMORY_TYPE_SEMANTIC',
        },
      });
      console.log(`  Stored fact memory: ${factMemory.memoryId}`);
    } catch (error: any) {
      if (error.code === 12) {
        console.log('  (Memory storage RPC not yet implemented - stub returned)');
      } else {
        throw error;
      }
    }

    // Example 2: Search memories
    console.log('\n=== Example 2: Search Memories ===');
    try {
      const searchResponse = await client.searchMemories({
        sessionId,
        query: 'API authentication',
        limit: 5,
      });
      console.log(`  Found ${searchResponse.totalCount} memories:`);
      searchResponse.memories?.forEach((memory, i) => {
        console.log(`    ${i + 1}. [${memory.memoryType}] ${memory.content?.substring(0, 60)}...`);
        console.log(`       Importance: ${memory.importance}, Tags: ${memory.tags?.join(', ')}`);
      });
    } catch (error: any) {
      if (error.code === 12) {
        console.log('  (Memory search RPC not yet implemented - stub returned)');
      } else {
        throw error;
      }
    }

    // Example 3: Search by tags
    console.log('\n=== Example 3: Search by Tags ===');
    try {
      const tagSearchResponse = await client.searchMemories({
        sessionId,
        tags: ['success', 'api'],
        limit: 10,
      });
      console.log(`  Found ${tagSearchResponse.totalCount} memories with tags [success, api]:`);
      tagSearchResponse.memories?.forEach((memory, i) => {
        console.log(`    ${i + 1}. ${memory.content?.substring(0, 60)}...`);
      });
    } catch (error: any) {
      if (error.code === 12) {
        console.log('  (Memory search RPC not yet implemented - stub returned)');
      } else {
        throw error;
      }
    }

    // Example 4: Get memory statistics
    console.log('\n=== Example 4: Memory Statistics ===');
    try {
      const statsResponse = await client.getMemoryStats({ sessionId });
      console.log('  Memory Statistics:');
      console.log(`    Long-term: ${statsResponse.stats?.longTermCount} memories`);
      console.log(`    Short-term: ${statsResponse.stats?.shortTermCount} memories`);
      console.log(`    Working: ${statsResponse.stats?.workingCount} memories`);
    } catch (error: any) {
      if (error.code === 12) {
        console.log('  (Memory stats RPC not yet implemented - stub returned)');
      } else {
        throw error;
      }
    }

    // Example 5: Retrieve specific memory
    console.log('\n=== Example 5: Retrieve Memory ===');
    try {
      const retrieveResponse = await client.retrieveMemory({
        sessionId,
        memoryId: 'memory-123',
      });
      if (retrieveResponse.memory) {
        console.log('  Retrieved memory:');
        console.log(`    Content: ${retrieveResponse.memory.content}`);
        console.log(`    Type: ${retrieveResponse.memory.memoryType}`);
        console.log(`    Importance: ${retrieveResponse.memory.importance}`);
        console.log(`    Access count: ${retrieveResponse.memory.accessCount}`);
      } else {
        console.log('  Memory not found');
      }
    } catch (error: any) {
      if (error.code === 12) {
        console.log('  (Memory retrieval RPC not yet implemented - stub returned)');
      } else {
        throw error;
      }
    }

    // Example 6: Use memory in generation
    console.log('\n=== Example 6: Generate with Memory Context ===');
    console.log('Generating response with memory context...');

    // First, search for relevant memories
    try {
      const relevantMemories = await client.searchMemories({
        sessionId,
        query: 'Express API',
        limit: 3,
      });

      // Use memories as context in generation
      let contextPrompt = 'Based on past experiences:\n';
      relevantMemories.memories?.forEach((memory, i) => {
        contextPrompt += `${i + 1}. ${memory.content}\n`;
      });
      contextPrompt += '\nNow, create a new Express API endpoint for user profile.';

      const response = await client.generate({
        sessionId,
        prompt: contextPrompt,
      });
      console.log(`Response: ${response.text?.substring(0, 200)}...`);
    } catch (error: any) {
      if (error.code === 12) {
        console.log('  (Memory features not yet implemented - using regular generation)');
        const response = await client.generate({
          sessionId,
          prompt: 'Create a new Express API endpoint for user profile.',
        });
        console.log(`Response: ${response.text?.substring(0, 200)}...`);
      } else {
        throw error;
      }
    }

    // Example 7: Clear memories
    console.log('\n=== Example 7: Clear Memories ===');
    try {
      const clearResponse = await client.clearMemories({
        sessionId,
        clearLongTerm: false,
        clearShortTerm: true,
        clearWorking: true,
      });
      console.log(`  Cleared ${clearResponse.clearedCount} memories`);
      console.log('  (Long-term memories preserved)');
    } catch (error: any) {
      if (error.code === 12) {
        console.log('  (Memory clearing RPC not yet implemented - stub returned)');
      } else {
        throw error;
      }
    }

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
