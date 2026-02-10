/**
 * Simple Storage Configuration Demo
 *
 * Demonstrates how to create sessions with different storage types.
 */

import { A3sClient, StorageType } from '@a3s-lab/code';

async function main() {
  const client = new A3sClient({ address: 'localhost:4088' });

  try {
    // Example 1: Create session with memory storage (no persistence)
    console.log('\n=== Example 1: Memory Storage ===');
    const memorySession = await client.createSession({
      name: 'memory-session',
      workspace: '/tmp/workspace',
      storageType: StorageType.STORAGE_TYPE_MEMORY,
    });
    console.log(`Created memory session: ${memorySession.sessionId}`);
    console.log('This session will not persist after restart');

    // Example 2: Create session with file storage (persistent)
    console.log('\n=== Example 2: File Storage ===');
    const fileSession = await client.createSession({
      name: 'file-session',
      workspace: '/tmp/workspace',
      storageType: StorageType.STORAGE_TYPE_FILE,
    });
    console.log(`Created file session: ${fileSession.sessionId}`);
    console.log('This session will persist to disk');

    // Example 3: List all sessions
    console.log('\n=== Example 3: List Sessions ===');
    const sessions = await client.listSessions();
    console.log(`Total sessions: ${sessions.sessions.length}`);
    sessions.sessions.forEach(session => {
      console.log(`- ${session.sessionId}: ${session.config?.name}`);
    });

    // Cleanup
    console.log('\n=== Cleanup ===');
    await client.destroySession(memorySession.sessionId);
    await client.destroySession(fileSession.sessionId);
    console.log('Sessions destroyed');

  } catch (error) {
    console.error('Error:', error);
  } finally {
    client.close();
  }
}

main();
