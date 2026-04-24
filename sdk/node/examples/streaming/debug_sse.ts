#!/usr/bin/env npx tsx
/**
 * Debug SSE events to see raw chunk structure.
 */

import { Agent } from '../../index.js';

async function main(): Promise<void> {
  const configPath = '/Users/roylin/Desktop/code/a3s/crates/code/sdk/node/examples/streaming/test_minimax.hcl';

  const agent = await Agent.create(configPath);
  const session = agent.session('.', {
    permissive: true,
    max_tool_rounds: 0,
  });

  const prompt = 'Say hello in 3 words.';
  console.log(`Prompt: "${prompt}"`);

  const stream = await session.stream(prompt);

  let eventCount = 0;

  while (true) {
    const result = await stream.next();
    if (!result.value || result.done) break;

    const event = result.value;
    eventCount++;

    // Log all events
    console.log(`[${eventCount}] type="${event.type}"` +
      (event.text ? ` text="${event.text.slice(0, 30)}..."` : '') +
      (event.data ? ` data="${JSON.stringify(event.data)?.slice(0, 60)}..."` : '')
    );

    // Stop after 30 events to avoid flooding
    if (eventCount >= 30) {
      console.log('\n... (stopping after 30 events)');
      break;
    }
  }
}

main().catch(err => {
  console.error('Error:', err);
  process.exit(1);
});
