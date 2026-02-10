/**
 * Storage Configuration Example
 *
 * Demonstrates how to configure session storage types:
 * - Memory storage (temporary, no persistence)
 * - File storage (persistent, survives restarts)
 */

import { A3sClient, loadConfigFromDir, StorageType } from '@a3s-lab/code';
import { join, dirname } from 'path';
import { fileURLToPath } from 'url';

const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);
const configDir = join(__dirname, '..', '..', '..', '..', '.a3s');

async function runStorageExample(): Promise<void> {
  console.log('='.repeat(60));
  console.log('Storage Configuration Example');
  console.log('='.repeat(60));
  console.log();

  const client = new A3sClient({
    address: process.env.A3S_ADDRESS || 'localhost:4088',
    configDir: configDir
  });

  try {
    // Example 1: Create a temporary session with memory storage
    console.log('1. Creating temporary session (memory storage)...');
    const tempSession = await client.createSession({
      name: 'Temporary Analysis',
      workspace: '/tmp/workspace',
      storageType: StorageType.STORAGE_TYPE_MEMORY,
      systemPrompt: 'You are a code analyzer for temporary tasks.'
    });
    console.log(`✓ Temporary session created: ${tempSession.sessionId}`);
    console.log(`  Storage: Memory (no persistence)`);
    console.log();

    // Example 2: Create a persistent session with file storage
    console.log('2. Creating persistent session (file storage)...');
    const persistentSession = await client.createSession({
      name: 'Long-term Project',
      workspace: '/tmp/workspace',
      storageType: StorageType.STORAGE_TYPE_FILE,
      systemPrompt: 'You are a helpful coding assistant for long-term projects.'
    });
    console.log(`✓ Persistent session created: ${persistentSession.sessionId}`);
    console.log(`  Storage: File (persists across restarts)`);
    console.log(`  Sessions will be saved to: /tmp/workspace/sessions/`);
    console.log();

    // Example 3: Use the sessions
    console.log('3. Testing sessions...');

    // Memory session - quick analysis
    console.log('  Memory session: Quick code analysis...');
    const tempResponse = await client.generate(persistentSession.sessionId, {
      messages: [{
        role: 'user',
        content: 'Analyze this code: function add(a, b) { return a + b; }'
      }]
    });
    console.log(`  ✓ Response: ${tempResponse.content.substring(0, 100)}...`);
    console.log();

    // File session - persistent work
    console.log('  File session: Starting persistent work...');
    const persistentResponse = await client.generate(persistentSession.sessionId, {
      messages: [{
        role: 'user',
        content: 'I need help refactoring a large codebase. Let\'s start by understanding the structure.'
      }]
    });
    console.log(`  ✓ Response: ${persistentResponse.content.substring(0, 100)}...`);
    console.log();

    // Example 4: List sessions
    console.log('4. Listing all sessions...');
    const sessions = await client.listSessions();
    console.log(`  Total sessions: ${sessions.sessions.length}`);
    for (const session of sessions.sessions) {
      const storageType = session.config?.storageType === StorageType.STORAGE_TYPE_MEMORY
        ? 'Memory'
        : 'File';
      console.log(`  - ${session.config?.name} (${storageType})`);
    }
    console.log();

    // Example 5: Cleanup
    console.log('5. Cleanup...');
    console.log('  Note: Memory sessions are automatically cleaned up');
    console.log('  File sessions persist and can be resumed after restart');

    // Clean up temporary session
    await client.destroySession(tempSession.sessionId);
    console.log(`  ✓ Destroyed temporary session`);

    // Keep persistent session for demonstration
    console.log(`  ✓ Persistent session kept: ${persistentSession.sessionId}`);
    console.log();

    console.log('='.repeat(60));
    console.log('Storage Configuration Example Complete');
    console.log('='.repeat(60));

  } catch (error) {
    console.error('Error:', error);
    throw error;
  }
}

// Run the example
runStorageExample().catch(console.error);
