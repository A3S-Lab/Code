/**
 * Simple Test — SDK Quick Start
 *
 * Demonstrates both API styles:
 * - Session-based API (A3sClient) — the core pattern
 * - High-level API (generateText/streamText) — convenience wrapper
 */

import {
  A3sClient,
  generateText,
  streamText,
  createProvider,
  loadConfigFromDir,
} from '@a3s-lab/code';
import { join, dirname } from 'path';
import { fileURLToPath } from 'url';

const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);
const configDir = join(__dirname, '..', '..', '..', '..', '.a3s');

async function main() {
  console.log('='.repeat(60));
  console.log('A3S SDK — Quick Start');
  console.log('='.repeat(60));
  console.log();

  // Load config
  const config = loadConfigFromDir(configDir);
  console.log('Config loaded:');
  console.log(`  Default provider: ${config?.defaultProvider}`);
  console.log(`  Default model: ${config?.defaultModel}`);
  console.log();

  // ========================================================================
  // Part 1: Session-Based API (Core Pattern)
  // ========================================================================
  console.log('=== Part 1: Session-Based API (A3sClient) ===\n');

  const client = new A3sClient({
    address: process.env.A3S_ADDRESS || 'localhost:4088',
    configDir,
  });

  try {
    // Health check
    console.log('1. Health check...');
    const health = await client.healthCheck();
    console.log(`✓ Status: ${health.status}`);
    console.log();

    // Create session
    console.log('2. Creating session...');
    const session = await client.createSession({
      name: 'simple-test',
      workspace: '/tmp/test',
      systemPrompt: 'You are a helpful assistant.',
      llm: {
        provider: config?.defaultProvider || 'openai',
        model: config?.defaultModel || 'gpt-4o',
        apiKey: config?.apiKey,
        baseUrl: config?.baseUrl,
      },
    });
    console.log(`✓ Session: ${session.sessionId}`);
    console.log();

    // Generate
    console.log('3. Generating response...');
    const response = await client.generate(session.sessionId, [
      { role: 'user', content: 'Say hello in one word' },
    ]);
    console.log(`✓ Response: ${response.message?.content}`);
    console.log(`  Finish reason: ${response.finishReason}`);
    console.log();

    // Stream
    console.log('4. Streaming response...');
    process.stdout.write('   Response: ');
    for await (const chunk of client.streamGenerate(session.sessionId, [
      { role: 'user', content: 'Count from 1 to 3' },
    ])) {
      if (chunk.type === 'content' && chunk.content) {
        process.stdout.write(chunk.content);
      }
    }
    console.log();
    console.log('✓ Streaming complete');
    console.log();

    // Context usage
    console.log('5. Context usage...');
    const usage = await client.getContextUsage(session.sessionId);
    if (usage.usage) {
      console.log(`✓ Tokens: ${usage.usage.totalTokens}, Messages: ${usage.usage.messageCount}`);
    }
    console.log();

    // Cleanup
    await client.destroySession(session.sessionId);
    console.log('✓ Session destroyed');

  } finally {
    client.close();
  }

  console.log();

  // ========================================================================
  // Part 2: High-Level API (Vercel AI SDK-style)
  // ========================================================================
  console.log('=== Part 2: High-Level API (Vercel AI SDK-style) ===\n');
  console.log('Same operations, but sessions are managed automatically.\n');

  const openai = createProvider({
    name: config?.defaultProvider || 'openai',
    apiKey: config?.apiKey || process.env.OPENAI_API_KEY || 'sk-xxx',
    baseUrl: config?.baseUrl,
  });

  // generateText — auto session
  console.log('6. generateText()...');
  const { text } = await generateText({
    model: openai(config?.defaultModel || 'gpt-4o'),
    prompt: 'Say hello in one word.',
    workspace: '/tmp/test',
  });
  console.log(`✓ Response: ${text}`);
  console.log();

  // streamText — auto session
  console.log('7. streamText()...');
  const result = streamText({
    model: openai(config?.defaultModel || 'gpt-4o'),
    prompt: 'Count from 1 to 3.',
    workspace: '/tmp/test',
  });
  process.stdout.write('   Response: ');
  for await (const chunk of result.textStream) {
    process.stdout.write(chunk);
  }
  console.log();
  console.log('✓ Streaming complete');
  console.log();

  console.log('='.repeat(60));
  console.log('All tests passed! ✓');
  console.log('='.repeat(60));
}

main().catch(error => {
  console.error('Fatal error:', error);
  process.exit(1);
});
