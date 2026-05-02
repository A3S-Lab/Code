#!/usr/bin/env npx tsx
/**
 * Quick test to verify no duplicate text_delta/reasoning_delta events.
 * Uses kimi-k2.5 model with environment variable injection.
 *
 * Run with environment variables:
 *   A3S_API_KEY=your_key A3S_BASE_URL=https://api.moonshot.cn/v1 \
 *   npx tsx sdk/node/examples/streaming/test_fix_verification_kimi.ts
 */

import { Agent } from '../../index.js';

async function main(): Promise<void> {
  const apiKey = process.env.A3S_API_KEY;
  const baseUrl = process.env.A3S_BASE_URL || 'https://api.moonshot.cn/v1';

  if (!apiKey) {
    console.error('❌ Missing A3S_API_KEY environment variable');
    console.log('\nUsage:');
    console.log('  A3S_API_KEY=your_key A3S_BASE_URL=https://api.moonshot.cn/v1 \\');
    console.log('  npx tsx sdk/node/examples/streaming/test_fix_verification_kimi.ts');
    process.exit(1);
  }

  console.log(`Using API: ${baseUrl}\n`);

  // Create config inline with env vars.
  const agent = await Agent.create(`
default_model = "openai/kimi-k2.5"

providers "openai" {
  apiKey = "${apiKey}"
  baseUrl = "${baseUrl}"

  models "kimi-k2.5" {
    name = "kimi"
    reasoning = true
  }
}
`);

  const session = agent.session('.', {
    permissionPolicy: { defaultDecision: 'allow' }, maxToolRounds: 0,
  });

  const prompt = 'Say hello in exactly 3 words in Chinese.';
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
      if (eventCount <= 5) {
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

  // Check for repetition in combined text
  if (textDeltas.length >= 2) {
    // Check if text seems to be repeated
    const halfLen = Math.floor(combinedText.length / 2);
    const firstHalf = combinedText.slice(0, halfLen);
    const secondHalf = combinedText.slice(halfLen);

    if (firstHalf === secondHalf && combinedText.length > 4) {
      console.log('\n❌ DUPLICATE DETECTED: Full text appears twice!');
      process.exit(1);
    }
  }

  // Check for suspicious repeated small chunks
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
