#!/usr/bin/env npx tsx
/**
 * Stream fix verification test.
 * Uses kimi model from config and logs ALL event types to verify no "unknown" appears.
 */

import { Agent, Session } from '../../index.js';
import * as path from 'path';
import * as os from 'os';

async function main(): Promise<void> {
  const configPath = process.env.A3S_CONFIG || path.join(os.homedir(), '.a3s', 'config.hcl');
  console.log(`Using config: ${configPath}\n`);
  console.log('='.repeat(80));
  console.log('Stream Fix Verification Test');
  console.log('='.repeat(80));
  console.log();

  const agent = await Agent.create(configPath);
  const session = agent.session('.');

  console.log('Streaming with prompt: "Say hello in 5 words"\n');

  const stream = await session.stream('Say hello in 5 words');

  const eventTypes = new Map<string, number>();
  let totalEvents = 0;

  while (true) {
    const result = await stream.next();
    if (!result.value || result.done) break;

    const event = result.value;
    totalEvents++;
    const count = eventTypes.get(event.type) ?? 0;
    eventTypes.set(event.type, count + 1);

    // Log all events
    console.log(`[${totalEvents}] type="${event.type}"` +
      (event.text ? ` text="${event.text.slice(0, 50)}"` : '') +
      (event.toolName ? ` toolName="${event.toolName}"` : '') +
      (event.data ? ` data="${event.data.slice(0, 80)}"` : '') +
      (event.error ? ` error="${event.error}"` : '') +
    '');
  }

  console.log('\n' + '='.repeat(80));
  console.log('Summary:');
  console.log(`Total events: ${totalEvents}`);
  console.log('Event types received:');
  for (const [type, count] of eventTypes) {
    console.log(`  ${type}: ${count}`);
  }

  if (eventTypes.has('unknown')) {
    console.log('\n❌ FAILED: Still receiving "unknown" event types!');
    process.exit(1);
  } else {
    console.log('\n✅ PASSED: No "unknown" event types detected.');
  }
}

main().catch((err: unknown) => {
  console.error('Test failed:', err);
  process.exit(1);
});
