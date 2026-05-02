#!/usr/bin/env npx tsx
/**
 * Ultra-simple test to check for duplicate text_delta.
 */

import { Agent } from '../../index.js';

async function main(): Promise<void> {
  const configPath = '/Users/roylin/Desktop/code/a3s/crates/code/sdk/node/examples/streaming/test_minimax.acl';

  const agent = await Agent.create(configPath);
  const session = agent.session('.', {
    permissionPolicy: { defaultDecision: 'allow' }, maxToolRounds: 0,
  });

  // Use a very simple prompt
  const prompt = 'Hi';
  console.log(`Prompt: "${prompt}"`);

  const stream = await session.stream(prompt);

  const textDeltas: string[] = [];
  const seen = new Set<string>();
  let duplicateFound = false;
  let eventCount = 0;

  while (true) {
    const result = await stream.next();
    if (!result.value || result.done) break;

    const event = result.value;
    eventCount++;

    if (event.type === 'text_delta' && event.text) {
      const text = event.text;
      textDeltas.push(text);

      if (seen.has(text)) {
        console.log(`❌ DUPLICATE FOUND: "${text}" (event ${eventCount})`);
        duplicateFound = true;
      } else {
        seen.add(text);
        console.log(`[${eventCount}] text_delta: "${text}"`);
      }
    }
  }

  console.log('\n--- Summary ---');
  console.log(`Total text_delta events: ${textDeltas.length}`);
  console.log(`Unique text_delta events: ${seen.size}`);

  if (duplicateFound) {
    console.log('\n❌ DUPLICATES DETECTED');
    process.exit(1);
  } else {
    console.log('\n✅ No duplicates');
  }
}

main().catch(err => {
  console.error('Error:', err);
  process.exit(1);
});
