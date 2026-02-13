/**
 * Chat Simulation — Multi-Turn Conversation
 *
 * Shows how a single session maintains conversation context across
 * multiple generateText() calls. Uses `await using` for auto-cleanup.
 */

import { A3sClient, createProvider } from '@a3s-lab/code';

const A3S_ADDRESS = process.env.A3S_ADDRESS || 'localhost:4088';

async function main() {
  console.log('=== Multi-Turn Chat with Session ===\n');

  const client = new A3sClient({ address: A3S_ADDRESS });
  const openai = createProvider({
    name: 'openai',
    apiKey: process.env.OPENAI_API_KEY || '',
  });

  // Create session — all turns share the same context
  await using session = await client.createSession({
    model: openai('gpt-4o'),
    workspace: '/tmp/a3s-workspace',
    system: 'You are a friendly coding tutor. Keep answers short.',
  });
  console.log(`Session: ${session.id}\n`);

  // Turn 1
  const { text: t1 } = await session.generateText({
    prompt: 'What is a closure in JavaScript?',
  });
  console.log('User: What is a closure in JavaScript?');
  console.log('Assistant:', t1, '\n');

  // Turn 2 — session remembers the previous turn
  const { text: t2 } = await session.generateText({
    prompt: 'Can you give me a simple example?',
  });
  console.log('User: Can you give me a simple example?');
  console.log('Assistant:', t2, '\n');

  // Turn 3 — streaming response
  console.log('User: How is that different from a regular function?');
  process.stdout.write('Assistant: ');
  const { textStream } = session.streamText({
    prompt: 'How is that different from a regular function?',
  });
  for await (const chunk of textStream) {
    process.stdout.write(chunk);
  }
  console.log('\n');

  // Check context usage
  const usage = await session.getContextUsage();
  console.log('Context usage after 3 turns:', usage);

  client.close();
  console.log('\nDone!');
}

main().catch(console.error);
