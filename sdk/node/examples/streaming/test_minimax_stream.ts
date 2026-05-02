#!/usr/bin/env npx tsx
/**
 * Direct test of MiniMax streaming to see raw events.
 */

import { Agent } from '../../index.js';
import * as fs from 'fs';
import * as path from 'path';
import { fileURLToPath } from 'url';

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);

function resolveConfigPath(): string {
  const candidates = [
    process.env.A3S_CONFIG,
    path.join(__dirname, '..', '..', '..', '..', '..', '..', '.a3s', 'config.acl'),
    path.join(process.env.HOME || '', '.a3s', 'config.acl'),
  ].filter(Boolean) as string[];

  for (const candidate of candidates) {
    if (fs.existsSync(candidate)) {
      return candidate;
    }
  }

  throw new Error('Config not found. Set A3S_CONFIG or create ~/.a3s/config.acl.');
}

async function main(): Promise<void> {
  const configPath = resolveConfigPath();
  console.log(`Using config: ${configPath}\n`);

  const agent = await Agent.create(configPath);
  const session = agent.session('.', {
    permissionPolicy: { defaultDecision: 'allow' },
    maxToolRounds: 0,
  });

  console.log('Streaming with prompt: "Say hello in 5 words"\n');

  const stream = await session.stream('Say hello in 5 words');

  let eventCount = 0;
  const eventTypes = new Map<string, number>();
  const textDeltas: string[] = [];
  const reasoningDeltas: string[] = [];

  while (true) {
    const result = await stream.next();
    if (!result.value || result.done) break;

    const event = result.value;
    eventCount++;
    const type = event.type || 'unknown';
    const count = eventTypes.get(type) ?? 0;
    eventTypes.set(type, count + 1);

    if (type === 'text_delta' && event.text) {
      textDeltas.push(event.text);
    } else if (type === 'reasoning_delta' && event.text) {
      reasoningDeltas.push(event.text);
    }

    // Log first 10 events
    if (eventCount <= 10) {
      console.log(
        `[${eventCount}] type="${type}"` +
          (event.text ? ` text="${event.text.slice(0, 40)}"` : '') +
          (event.data ? ` data="${event.data?.slice(0, 60)}"` : ''),
      );
    }
  }

  console.log('\n' + '='.repeat(80));
  console.log('Summary:');
  console.log(`Total events: ${eventCount}`);
  console.log('Event types received:');
  for (const [type, count] of eventTypes) {
    console.log(`  ${type}: ${count}`);
  }
  console.log(`\nTextDelta count: ${textDeltas.length}`);
  console.log(`Text content: "${textDeltas.join('')}"`);
  console.log(`\nReasoningDelta count: ${reasoningDeltas.length}`);
  console.log(`Reasoning content preview: "${reasoningDeltas.slice(0, 3).join('').slice(0, 100)}..."`);

  if (reasoningDeltas.length > 0 && textDeltas.join('').length === 0) {
    console.log('\n❌ ISSUE CONFIRMED: Received reasoning_delta but NO text_delta with actual content!');
    console.log('   MiniMax reasoning_content is being sent as reasoning_delta,');
    console.log('   but the actual response is only in the final chunk.');
  }
}

main().catch((err: unknown) => {
  console.error('Test failed:', err);
  process.exit(1);
});
