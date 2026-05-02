#!/usr/bin/env npx tsx
/**
 * Test for reasoning_delta event type - verifies kimi reasoning_content
 * is properly emitted as a separate event type.
 */

import { Agent } from '../../index.js';

async function main(): Promise<void> {
  const configPath = '/Users/roylin/Desktop/code/a3s/crates/code/sdk/node/examples/streaming/test_config3.acl';
  console.log(`Using config: ${configPath}\n`);

  const agent = await Agent.create(configPath);
  const session = agent.session('.', {
    permissionPolicy: { defaultDecision: 'allow' }, // maxToolRounds: 0, // Don't limit - we need LLM call to test reasoning_delta
  });

  const prompt = 'Why is the sky blue? Answer in 2 sentences.';
  console.log(`Streaming with prompt: "${prompt}"\n`);

  const stream = await session.stream(prompt);

  let eventCount = 0;
  let textDeltaCount = 0;
  let reasoningDeltaCount = 0;
  let totalText = '';
  let totalReasoning = '';
  const eventTypes = new Map<string, number>();

  while (true) {
    const result = await stream.next();
    if (!result.value || result.done) break;

    const event = result.value;
    eventCount++;

    // Count event types
    const type = event.type || 'unknown';
    eventTypes.set(type, (eventTypes.get(type) || 0) + 1);

    if (type === 'text_delta' && event.text) {
      textDeltaCount++;
      totalText += event.text;
    } else if (type === 'reasoning_delta' && event.text) {
      reasoningDeltaCount++;
      totalReasoning += event.text;
    }

    console.log(`[${eventCount}] type="${type}"` +
      (event.text ? ` text="${event.text.slice(0, 60)}"` : '') +
      (event.data ? ` data="${event.data?.slice(0, 60)}"` : '')
    );
  }

  console.log('\n' + '='.repeat(80));
  console.log('Summary:');
  console.log(`Total events: ${eventCount}`);
  console.log(`Event type distribution:`);
  for (const [type, count] of eventTypes) {
    console.log(`  - ${type}: ${count}`);
  }
  console.log(`\nTextDelta events: ${textDeltaCount}`);
  console.log(`Text content length: ${totalText.length}`);
  console.log(`ReasoningDelta events: ${reasoningDeltaCount}`);
  console.log(`Reasoning content length: ${totalReasoning.length}`);

  if (totalReasoning.length > 0) {
    console.log(`\nReasoning preview: "${totalReasoning.slice(0, 150)}..."`);
  }

  // Verify reasoning_delta was received
  if (reasoningDeltaCount > 0) {
    console.log('\n✅ reasoning_delta mechanism is WORKING');
  } else {
    console.log('\n❌ reasoning_delta mechanism is NOT working (no reasoning_delta events received)');
    process.exit(1);
  }
}

main().catch((err: unknown) => {
  console.error('Test failed:', err);
  process.exit(1);
});
