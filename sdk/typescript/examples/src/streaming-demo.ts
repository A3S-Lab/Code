/**
 * Streaming APIs Demo
 *
 * Demonstrates how to use the streaming APIs:
 * - streamGenerate() - Stream generation responses
 * - streamGenerateStructured() - Stream structured responses
 * - subscribeEvents() - Subscribe to agent events
 */

import { A3sClient, StorageType } from '@a3s-lab/code';

async function main() {
  const client = new A3sClient({ address: 'localhost:4088' });

  try {
    // Create a session
    console.log('=== Creating Session ===\n');
    const session = await client.createSession({
      name: 'streaming-demo',
      workspace: '/tmp/workspace',
      storageType: StorageType.STORAGE_TYPE_MEMORY,
    });
    const sessionId = session.sessionId;
    console.log(`Session created: ${sessionId}\n`);

    // Example 1: Stream Generate
    console.log('=== Example 1: Stream Generate ===\n');
    console.log('Streaming response:');

    const messages = [
      { role: 'user' as const, content: 'Write a haiku about coding.' }
    ];

    for await (const chunk of client.streamGenerate(sessionId, messages)) {
      if (chunk.type === 'content') {
        process.stdout.write(chunk.content);
      } else if (chunk.type === 'tool_call' && chunk.toolCall) {
        console.log(`\n[Tool Call: ${chunk.toolCall.name}]`);
      } else if (chunk.type === 'done') {
        console.log('\n[Stream Complete]');
      }
    }
    console.log('\n');

    // Example 2: Stream Generate Structured
    console.log('=== Example 2: Stream Generate Structured ===\n');

    const schema = JSON.stringify({
      type: 'object',
      properties: {
        title: { type: 'string' },
        summary: { type: 'string' },
        tags: { type: 'array', items: { type: 'string' } }
      },
      required: ['title', 'summary', 'tags']
    });

    const structuredMessages = [
      { role: 'user' as const, content: 'Describe TypeScript in structured format.' }
    ];

    let structuredData = '';
    for await (const chunk of client.streamGenerateStructured(sessionId, structuredMessages, schema)) {
      structuredData += chunk.data;
      process.stdout.write(chunk.data);
      if (chunk.done) {
        console.log('\n[Structured Stream Complete]');
      }
    }

    try {
      const parsed = JSON.parse(structuredData);
      console.log('\nParsed result:', JSON.stringify(parsed, null, 2));
    } catch {
      console.log('\nRaw data:', structuredData);
    }
    console.log('\n');

    // Example 3: Subscribe to Events
    console.log('=== Example 3: Subscribe to Events ===\n');
    console.log('Subscribing to events (will show events from next generation)...\n');

    // Start event subscription in background
    const eventPromise = (async () => {
      const eventStream = client.subscribeEvents(sessionId);
      let eventCount = 0;

      for await (const event of eventStream) {
        console.log(`[Event] ${event.type}: ${event.message}`);
        eventCount++;

        // Stop after receiving a few events for demo purposes
        if (eventCount >= 5 || event.type === 'EVENT_TYPE_GENERATION_COMPLETED') {
          break;
        }
      }
    })();

    // Trigger a generation to produce events
    await new Promise(resolve => setTimeout(resolve, 100)); // Small delay

    const eventMessages = [
      { role: 'user' as const, content: 'Say hello briefly.' }
    ];

    console.log('Triggering generation to produce events...');
    const response = await client.generate(sessionId, eventMessages);
    console.log(`Generation complete: ${response.message?.content?.substring(0, 50)}...`);

    // Wait for events (with timeout)
    await Promise.race([
      eventPromise,
      new Promise(resolve => setTimeout(resolve, 3000))
    ]);

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
