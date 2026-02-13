/**
 * Simple Test — Session-Centric API Demo
 *
 * Demonstrates the core session-based workflow:
 * 1. Create client + provider
 * 2. Create session (model + workspace bound here)
 * 3. Call session.generateText() / session.streamText()
 * 4. Session auto-closes via `using`
 */

import { A3sClient, createProvider } from '@a3s-lab/code';

const A3S_ADDRESS = process.env.A3S_ADDRESS || 'localhost:4088';

async function main() {
  console.log('=== A3S Code SDK — Session-Centric API ===\n');

  // 1. Create client and provider
  const client = new A3sClient({ address: A3S_ADDRESS });
  const openai = createProvider({
    name: 'openai',
    apiKey: process.env.OPENAI_API_KEY || '',
  });

  // Health check
  const health = await client.healthCheck();
  console.log('Health:', health.status);

  // 2. Create session — model and workspace are bound here, immutable after
  //    `await using` ensures session.close() is called automatically
  {
    await using session = await client.createSession({
      model: openai('gpt-4o'),
      workspace: '/tmp/a3s-workspace',
      system: 'You are a helpful coding assistant. Be concise.',
    });
    console.log(`Session created: ${session.id}\n`);

    // 3. Generate text
    console.log('--- generateText ---');
    const { text, usage, finishReason } = await session.generateText({
      prompt: 'What is TypeScript in one sentence?',
    });
    console.log('Response:', text);
    console.log('Usage:', usage);
    console.log('Finish reason:', finishReason);

    // 4. Multi-turn: same session remembers context
    console.log('\n--- Multi-turn ---');
    const { text: followUp } = await session.generateText({
      prompt: 'Now explain its type system briefly.',
    });
    console.log('Follow-up:', followUp);

    // 5. Stream text
    console.log('\n--- streamText ---');
    const { textStream } = session.streamText({
      prompt: 'List 3 benefits of TypeScript.',
    });
    process.stdout.write('Streaming: ');
    for await (const chunk of textStream) {
      process.stdout.write(chunk);
    }
    console.log('\n');

    // 6. Context management
    const ctxUsage = await session.getContextUsage();
    console.log('Context usage:', ctxUsage);
  }
  // session.close() called automatically here

  // =========================================================================
  // Convenience API (auto session, for one-shot usage)
  // =========================================================================
  console.log('\n=== Convenience API (auto session) ===\n');

  const { generateText, streamText } = await import('@a3s-lab/code');

  const { text } = await generateText({
    model: openai('gpt-4o'),
    prompt: 'What is Rust in one sentence?',
    workspace: '/tmp/a3s-workspace',
    server: { address: A3S_ADDRESS },
  });
  console.log('One-shot:', text);

  client.close();
  console.log('\nDone!');
}

main().catch(console.error);
