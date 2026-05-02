#!/usr/bin/env npx tsx
/**
 * Quick test to verify no duplicate text_delta/reasoning_delta events.
 * Uses MiniMax model from config file.
 *
 * Run with:
 *   npx tsx sdk/node/examples/streaming/test_fix_minimax.ts
 */

import { Agent } from '../../index.js';

async function main(): Promise<void> {
  const configPath = '/Users/roylin/Desktop/code/a3s/crates/code/sdk/node/examples/streaming/test_minimax.acl';
  console.log(`Using config: ${configPath}\n`);

  const agent = await Agent.create(configPath);
  const session = agent.session('.', {
    permissionPolicy: { defaultDecision: 'allow' }, maxToolRounds: 0,
  });

  const prompt = 'Say hello in 3 Chinese characters.';
  console.log(`Streaming with prompt: "${prompt}"\n`);

  const stream = await session.stream(prompt);

  const textDeltas: string[] = [];
  const reasoningDeltas: string[] = [];
  let eventCount = 0;

  while (true) {
    const result = await stream.next();
    if (!result.value || result.done) break;

    const event = result.value;
    eventCount++;
    const type = event.type || 'unknown';

    if (type === 'text_delta' && event.text) {
      textDeltas.push(event.text);
      console.log(`[${eventCount}] text_delta: "${event.text}"`);
    } else if (type === 'reasoning_delta' && event.text) {
      reasoningDeltas.push(event.text);
      if (eventCount <= 3) {
        console.log(`[${eventCount}] reasoning_delta: "${event.text.slice(0, 40)}..."`);
      }
    }
  }

  console.log('\n' + '='.repeat(80));
  console.log('Summary:');
  console.log(`Total events: ${eventCount}`);
  console.log(`TextDelta count: ${textDeltas.length}`);
  console.log(`ReasoningDelta count: ${reasoningDeltas.length}`);

  const combinedText = textDeltas.join('');
  console.log(`\nFinal text: "${combinedText}"`);

  // Check for duplicates
  const uniqueChunks = new Set(textDeltas);
  console.log(`Unique chunks: ${uniqueChunks.size} / ${textDeltas.length}`);

  if (textDeltas.length >= 2) {
    const halfLen = Math.floor(combinedText.length / 2);
    const firstHalf = combinedText.slice(0, halfLen);
    const secondHalf = combinedText.slice(halfLen);

    if (firstHalf === secondHalf && combinedText.length > 4) {
      console.log('\n❌ DUPLICATE DETECTED: Full text appears twice!');
      process.exit(1);
    }
  }

  const chunkCounts = new Map<string, number>();
  for (const chunk of textDeltas) {
    if (chunk.length > 2) {
      chunkCounts.set(chunk, (chunkCounts.get(chunk) || 0) + 1);
    }
  }

  for (const [chunk, count] of chunkCounts) {
    if (count > 1) {
      console.log(`\n❌ DUPLICATE CHUNK: "${chunk}" appears ${count} times`);
      process.exit(1);
    }
  }

  console.log('\n✅ No duplicates detected!');
}

main().catch((err: unknown) => {
  console.error('Test failed:', err);
  process.exit(1);
});
