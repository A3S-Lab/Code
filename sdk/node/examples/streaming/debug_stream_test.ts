#!/usr/bin/env npx tsx
/**
 * Debug stream test for kimi
 */

import { Agent } from '../../index.js';
import * as path from 'path';

async function main(): Promise<void> {
  const configPath = '/Users/roylin/Desktop/code/a3s/crates/code/sdk/node/examples/streaming/test_config3.acl';
  console.log(`Using config: ${configPath}\n`);

  const agent = await Agent.create(configPath);
  const session = agent.session('.', {
    permissionPolicy: { defaultDecision: 'allow' }, maxToolRounds: 0,
  });

  console.log('Streaming with prompt: "Say hello in 5 words"\n');

  const stream = await session.stream('Say hello in 5 words');

  let eventCount = 0;
  let textDeltaCount = 0;
  let totalText = '';

  while (true) {
    const result = await stream.next();
    if (!result.value || result.done) break;

    const event = result.value;
    eventCount++;
    console.log(`[${eventCount}] type="${event.type}"` +
      (event.text ? ` text="${event.text.slice(0, 50)}"` : '') +
      (event.data ? ` data="${event.data?.slice(0, 80)}"` : '')
    );

    if (event.type === 'text_delta' && event.text) {
      textDeltaCount++;
      totalText += event.text;
    }
  }

  console.log('\n' + '='.repeat(80));
  console.log('Summary:');
  console.log(`Total events: ${eventCount}`);
  console.log(`TextDelta events: ${textDeltaCount}`);
  console.log(`Total text length: ${totalText.length}`);
  console.log(`Total text preview: "${totalText.slice(0, 100)}..."`);
}

main().catch((err: unknown) => {
  console.error('Test failed:', err);
  process.exit(1);
});
