#!/usr/bin/env npx tsx
/**
 * Quick test to verify no duplicate text_delta.
 */

import { Agent } from '../../index.js';

async function main(): Promise<void> {
  const configPath = '/Users/roylin/Desktop/code/a3s/crates/code/sdk/node/examples/streaming/test_minimax.acl';

  const agent = await Agent.create(configPath);
  const session = agent.session('.', {
    permissionPolicy: { defaultDecision: 'allow' }, maxToolRounds: 0,
  });

  const prompt = 'Say "OK"';
  console.log(`Prompt: "${prompt}"`);

  const stream = await session.stream(prompt);

  const textDeltas: string[] = [];
  const seen = new Set<string>();
  let eventCount = 0;

  while (true) {
    const result = await stream.next();
    if (!result.value || result.done) break;

    const event = result.value;
    eventCount++;

    if (event.type === 'text_delta' && event.text) {
      textDeltas.push(event.text);
      if (seen.has(event.text)) {
        console.log(`❌ DUPLICATE: "${event.text}"`);
        console.log('\n--- Summary ---');
        console.log(`Total text_delta events: ${textDeltas.length}`);
        console.log(`Unique text_delta events: ${seen.size}`);
        console.log('\n❌ DUPLICATES DETECTED');
        return;
      }
      seen.add(event.text);
      console.log(`[${eventCount}] text: "${event.text.slice(0, 30)}..."`);
    }

    if (eventCount > 100) {
      console.log('\n... (stopping after 100 events)');
      break;
    }
  }

  console.log('\n--- Summary ---');
  console.log(`Total text_delta events: ${textDeltas.length}`);
  console.log(`Unique text_delta events: ${seen.size}`);

  if (textDeltas.length === seen.size) {
    console.log('\n✅ No duplicates!');
  } else {
    console.log('\n❌ DUPLICATES DETECTED');
  }
}

main().catch(err => {
  console.error('Error:', err);
  process.exit(1);
});
