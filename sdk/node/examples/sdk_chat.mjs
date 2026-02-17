#!/usr/bin/env node
/**
 * LLM chat example — sends a prompt and streams the response.
 *
 * Demonstrates:
 *   - Loading config from `.a3s/config.hcl`
 *   - Creating an agent via Agent.create()
 *   - Non-streaming session.send() call
 *   - Streaming session.stream() with event handling
 *   - Model override via SessionOptions
 *
 * Requires network access and valid API keys in config.
 *
 * Usage:
 *     # Build the native module first
 *     cd crates/code/sdk/node && npm run build
 *
 *     # Run the example
 *     node examples/sdk_chat.mjs
 *
 *     # With a custom prompt
 *     node examples/sdk_chat.mjs "Explain Rust's ownership model in 3 sentences"
 */

import { Agent } from '../index.js';
import { mkdtempSync } from 'fs';
import { join, dirname } from 'path';
import { tmpdir } from 'os';
import { fileURLToPath } from 'url';

const __dirname = dirname(fileURLToPath(import.meta.url));

function resolveConfig() {
  if (process.env.A3S_CONFIG) return process.env.A3S_CONFIG;
  const root = join(__dirname, '..', '..', '..', '..', '..');
  return join(root, '.a3s', 'config.hcl');
}

async function main() {
  const prompt = process.argv.length > 2
    ? process.argv.slice(2).join(' ')
    : 'What is 2+2? Reply with just the number and nothing else.';

  const configPath = resolveConfig();
  console.log('=== A3S Code Node.js SDK Chat Example ===\n');
  console.log(`Config: ${configPath}`);
  console.log(`Prompt: ${prompt}\n`);

  // -- 1. Create agent ------------------------------------------------------
  const agent = await Agent.create(configPath);
  console.log('[ok] Agent created\n');

  // -- 2. Non-streaming call ------------------------------------------------
  console.log('--- Non-streaming (session.send) ---');
  const workspace = mkdtempSync(join(tmpdir(), 'a3s-node-chat-'));
  const session = agent.session(workspace);

  const result = await session.send(prompt);
  console.log(`  Response: ${result.text.trim()}`);
  console.log(`  Tokens: ${result.promptTokens} in / ${result.completionTokens} out`);

  // -- 3. Streaming call ----------------------------------------------------
  console.log('\n--- Streaming (session.stream) ---');
  const workspace2 = mkdtempSync(join(tmpdir(), 'a3s-node-stream-'));
  const session2 = agent.session(workspace2);

  const events = await session2.stream(prompt);
  process.stdout.write('  Response: ');
  for (const event of events) {
    if (event.type === 'text_delta') {
      process.stdout.write(event.text || '');
    } else if (event.type === 'end') {
      console.log();
      console.log(`  Tokens: ${event.totalTokens} total`);
    }
  }

  // -- 4. Model override session --------------------------------------------
  console.log('\n--- Session with model override ---');
  const workspace3 = mkdtempSync(join(tmpdir(), 'a3s-node-override-'));
  try {
    const overrideSession = agent.session(workspace3, { model: 'anthropic/claude-sonnet-4-20250514' });
    console.log('[ok] Session with anthropic/claude-sonnet-4-20250514 created');
    try {
      const overrideResult = await overrideSession.send(prompt);
      console.log(`  Response: ${overrideResult.text.trim()}`);
      console.log(`  Tokens: ${overrideResult.promptTokens} in / ${overrideResult.completionTokens} out`);
    } catch (e) {
      console.log(`[skip] LLM call failed (rate limit / API error): ${e.message}`);
    }
  } catch (e) {
    console.log(`[skip] Model override failed (expected if model not in config): ${e.message}`);
  }

  console.log('\n=== Done ===');
}

main().catch(err => {
  console.error('FAILED:', err);
  process.exit(1);
});
