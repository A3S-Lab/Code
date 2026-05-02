#!/usr/bin/env npx tsx
/**
 * Test script to verify the duplicate fix in LLM streaming.
 *
 * This script:
 * 1. Streams a prompt using MiniMax model
 * 2. Collects all text_delta and reasoning_delta events
 * 3. Verifies that no duplicate content is sent
 *
 * Run with:
 *   A3S_CONFIG_PATH=./sdk/node/examples/streaming/test_minimax.acl npx tsx sdk/node/examples/streaming/test_fix_verification.ts
 */

import { Agent } from '../../index.js';

async function main(): Promise<void> {
  const configPath = process.env.A3S_CONFIG_PATH
    || '/Users/roylin/Desktop/code/a3s/crates/code/sdk/node/examples/streaming/test_minimax.acl';

  console.log(`Using config: ${configPath}\n`);

  const agent = await Agent.create(configPath);
  const session = agent.session('.', {
    permissionPolicy: { defaultDecision: 'allow' }, maxToolRounds: 0,
  });

  const prompt = 'Say hello in exactly 5 Chinese characters.';
  console.log(`Streaming with prompt: "${prompt}"\n`);

  const stream = await session.stream(prompt);

  const textDeltas: string[] = [];
  const reasoningDeltas: string[] = [];
  const allEvents: Array<{ type: string; text?: string }> = [];
  let eventCount = 0;

  while (true) {
    const result = await stream.next();
    if (!result.value || result.done) break;

    const event = result.value;
    eventCount++;
    const type = event.type || 'unknown';

    if (type === 'text_delta' && event.text) {
      textDeltas.push(event.text);
      allEvents.push({ type, text: event.text });
    } else if (type === 'reasoning_delta' && event.text) {
      reasoningDeltas.push(event.text);
      allEvents.push({ type, text: event.text });
    }

    // Log first 15 events for debugging
    if (eventCount <= 15) {
      console.log(`[${eventCount}] type="${type}"` +
        (event.text ? ` text="${event.text.slice(0, 50)}"` : '') +
        (event.data ? ` data="${event.data?.slice(0, 60)}"` : '')
      );
    }
  }

  console.log('\n' + '='.repeat(80));
  console.log('Summary:');
  console.log(`Total events: ${eventCount}`);
  console.log(`TextDelta events: ${textDeltas.length}`);
  console.log(`ReasoningDelta events: ${reasoningDeltas.length}`);

  const combinedText = textDeltas.join('');
  console.log(`\nCombined text: "${combinedText}"`);
  console.log(`Text length: ${combinedText.length}`);

  // Verify no duplicates
  let hasDuplicate = false;

  // Check if any text_delta appears more than once
  const textDeltaCounts = new Map<string, number>();
  for (const delta of textDeltas) {
    const count = textDeltaCounts.get(delta) || 0;
    textDeltaCounts.set(delta, count + 1);
  }

  // Check if combined text contains any substring appearing multiple times
  // in a suspicious pattern (e.g., "你好" appearing twice when it should only appear once)
  const suspiciousDuplicates: string[] = [];
  for (const [text, count] of textDeltaCounts) {
    if (count > 1 && text.length > 2) {
      suspiciousDuplicates.push(`"${text}" (appears ${count} times)`);
      hasDuplicate = true;
    }
  }

  // Check if the combined text contains any repeated substring pattern
  // that would indicate duplicate full content being sent
  const fullTextDupCheck = checkFullTextDuplication(textDeltas);

  if (suspiciousDuplicates.length > 0) {
    console.log('\n❌ ISSUE FOUND: Duplicate text_delta detected:');
    for (const dup of suspiciousDuplicates) {
      console.log(`   - ${dup}`);
    }
  }

  if (fullTextDupCheck.duplicate) {
    console.log('\n❌ ISSUE FOUND: Full text appears to be sent multiple times:');
    console.log(`   - Pattern: ${fullTextDupCheck.pattern}`);
    console.log(`   - Combined text: "${combinedText.slice(0, 200)}..."`);
  }

  if (!hasDuplicate && !fullTextDupCheck.duplicate) {
    console.log('\n✅ No duplicate content detected.');
    console.log('   - Each text_delta is a unique chunk');
    console.log('   - No full text repetition found');
  }

  // Additional check: verify the text is not repeated as a whole
  const uniqueTextChunks = new Set(textDeltas);
  console.log(`\nUnique text chunks: ${uniqueTextChunks.size} / ${textDeltas.length}`);

  if (textDeltas.length > 0 && uniqueTextChunks.size < textDeltas.length) {
    console.log('⚠️  Some text chunks are repeated');
  }

  console.log('\n' + '='.repeat(80));
}

/**
 * Check if the full combined text contains suspicious repetition patterns
 * that would indicate the entire response was sent multiple times.
 */
function checkFullTextDuplication(deltas: string[]): { duplicate: boolean; pattern?: string } {
  if (deltas.length < 3) {
    return { duplicate: false };
  }

  const combined = deltas.join('');

  // If we have many small deltas that when combined create repetition,
  // it might indicate the full content was sent multiple times

  // Check: is the combined text mostly just repetition of a smaller pattern?
  // For example, if combined = "ABC ABC ABC ABC" and deltas = ["ABC", "ABC", "ABC", "ABC"]
  // that would be suspicious

  // Simple heuristic: if we have 4+ deltas and the combined text is made up of
  // roughly equal chunks that repeat, it's suspicious
  if (deltas.length >= 4) {
    const firstDelta = deltas[0];
    const lastDelta = deltas[deltas.length - 1];

    // If first and last delta are identical and we have many deltas,
    // it might indicate duplication
    if (firstDelta === lastDelta && deltas.length >= 4) {
      return {
        duplicate: true,
        pattern: `First and last delta identical: "${firstDelta}"`
      };
    }

    // Check for repeated patterns in combined text
    // If combined = "AABBAABB" and we only expect "AABB", that's duplication
    const halfLength = Math.floor(combined.length / 2);
    const firstHalf = combined.slice(0, halfLength);
    const secondHalf = combined.slice(halfLength);

    if (firstHalf === secondHalf && combined.length >= 10) {
      return {
        duplicate: true,
        pattern: `First half equals second half (exact repetition)`
      };
    }
  }

  return { duplicate: false };
}

main().catch((err: unknown) => {
  console.error('Test failed:', err);
  process.exit(1);
});
