/**
 * Event Streaming Example
 *
 * Demonstrates how to:
 * - Subscribe to agent events
 * - Handle different event types
 * - Monitor agent execution in real-time
 * - Track tool usage and progress
 */

import { A3sClient } from '@a3s-lab/code';

async function eventStreamingExample(): Promise<void> {
  console.log('='.repeat(60));
  console.log('Event Streaming Example');
  console.log('='.repeat(60));
  console.log();

  const client = new A3sClient({
    address: process.env.A3S_ADDRESS || 'localhost:4088',
  });

  try {
    // Create a session
    console.log('1. Creating session...');
    const session = await client.createSession({
      name: 'event-demo',
      workspace: '/tmp/event-test',
      systemPrompt: 'You are a helpful assistant.',
    });
    console.log(`✓ Session created: ${session.sessionId}`);
    console.log();

    // Subscribe to events
    console.log('2. Subscribing to events...');
    const eventStream = client.subscribeEvents(session.sessionId);

    // Event counters
    const eventCounts: Record<string, number> = {};

    // Handle events in background
    const eventHandler = (async () => {
      console.log('✓ Event stream started');
      console.log();
      console.log('Events:');
      console.log('-'.repeat(60));

      for await (const event of eventStream) {
        const eventType = event.type;
        eventCounts[eventType] = (eventCounts[eventType] || 0) + 1;

        switch (eventType) {
          case 'EVENT_TYPE_AGENT_START':
            console.log(`[START] Agent started processing`);
            break;

          case 'EVENT_TYPE_TURN_START':
            console.log(`[TURN] Turn ${event.turn} started`);
            break;

          case 'EVENT_TYPE_TEXT_DELTA':
            process.stdout.write(event.text || '');
            break;

          case 'EVENT_TYPE_TOOL_START':
            console.log(`\n[TOOL] Executing: ${event.toolName}`);
            break;

          case 'EVENT_TYPE_TOOL_END':
            console.log(`[TOOL] Completed: ${event.toolName} (exit: ${event.exitCode})`);
            break;

          case 'EVENT_TYPE_TURN_END':
            console.log(`[TURN] Turn ${event.turn} completed`);
            if (event.usage) {
              console.log(`  Tokens: ${event.usage.totalTokens} (prompt: ${event.usage.promptTokens}, completion: ${event.usage.completionTokens})`);
            }
            break;

          case 'EVENT_TYPE_AGENT_END':
            console.log(`\n[END] Agent completed`);
            if (event.usage) {
              console.log(`  Total tokens: ${event.usage.totalTokens}`);
            }
            break;

          case 'EVENT_TYPE_ERROR':
            console.log(`[ERROR] ${event.message}`);
            break;

          case 'EVENT_TYPE_CONTEXT_RESOLVING':
            console.log(`[CONTEXT] Resolving context from ${event.providers?.length} providers`);
            break;

          case 'EVENT_TYPE_CONTEXT_RESOLVED':
            console.log(`[CONTEXT] Resolved ${event.totalItems} items (${event.totalTokens} tokens)`);
            break;

          case 'EVENT_TYPE_PERMISSION_DENIED':
            console.log(`[PERMISSION] Denied: ${event.toolName}`);
            break;

          case 'EVENT_TYPE_CONFIRMATION_REQUIRED':
            console.log(`[HITL] Confirmation required for: ${event.toolName}`);
            break;

          default:
            // Ignore other events
            break;
        }
      }
    })();

    // Trigger some activity
    console.log('3. Triggering agent activity...');
    await new Promise(resolve => setTimeout(resolve, 1000));

    const response = await client.generate(session.sessionId, [
      {
        role: 'user',
        content: 'List files in /tmp directory and tell me what you find',
      },
    ]);

    // Wait for events to complete
    await new Promise(resolve => setTimeout(resolve, 2000));

    console.log();
    console.log('-'.repeat(60));
    console.log();

    // Print event summary
    console.log('4. Event Summary:');
    Object.entries(eventCounts)
      .sort((a, b) => b[1] - a[1])
      .forEach(([type, count]) => {
        console.log(`  ${type}: ${count}`);
      });
    console.log();

    // Clean up
    await client.destroySession(session.sessionId);
    console.log('✓ Session destroyed');
    console.log();

    console.log('='.repeat(60));
    console.log('Event streaming example completed! ✓');
    console.log('='.repeat(60));
  } catch (error) {
    console.error('✗ Error:', (error as Error).message);
    throw error;
  } finally {
    client.close();
  }
}

// Run if executed directly
if (import.meta.url === `file://${process.argv[1]}`) {
  eventStreamingExample().catch(error => {
    console.error('Fatal error:', error);
    process.exit(1);
  });
}

export { eventStreamingExample };
