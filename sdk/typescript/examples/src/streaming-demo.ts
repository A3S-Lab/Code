/**
 * Streaming Demo
 *
 * Demonstrates all streaming APIs in both styles:
 * - Session-based: client.streamGenerate(), client.streamGenerateStructured()
 * - High-level: streamText(), streamObject()
 */

import {
  A3sClient,
  StorageType,
  streamText,
  streamObject,
  createProvider,
  tool,
} from '@a3s-lab/code';

async function main() {
  console.log('='.repeat(60));
  console.log('Streaming Demo');
  console.log('='.repeat(60));
  console.log();

  // ========================================================================
  // Part 1: Session-Based Streaming (A3sClient)
  // ========================================================================
  console.log('=== Part 1: Session-Based Streaming ===\n');

  const client = new A3sClient({ address: 'localhost:4088' });

  try {
    const session = await client.createSession({
      name: 'streaming-demo',
      workspace: '/tmp/workspace',
      storageType: StorageType.STORAGE_TYPE_MEMORY,
    });
    const sessionId = session.sessionId;
    console.log(`Session: ${sessionId}\n`);

    // Stream text
    console.log('1. streamGenerate()...');
    process.stdout.write('   ');
    for await (const chunk of client.streamGenerate(sessionId, [
      { role: 'user', content: 'Write a haiku about coding.' },
    ])) {
      if (chunk.type === 'content') {
        process.stdout.write(chunk.content);
      } else if (chunk.type === 'tool_call' && chunk.toolCall) {
        console.log(`\n   [Tool: ${chunk.toolCall.name}]`);
      } else if (chunk.type === 'done') {
        console.log('\n   [Complete]');
      }
    }
    console.log();

    // Stream structured
    console.log('2. streamGenerateStructured()...');
    const schema = JSON.stringify({
      type: 'object',
      properties: {
        title: { type: 'string' },
        summary: { type: 'string' },
        tags: { type: 'array', items: { type: 'string' } },
      },
      required: ['title', 'summary', 'tags'],
    });

    let structuredData = '';
    process.stdout.write('   Streaming: ');
    for await (const chunk of client.streamGenerateStructured(sessionId, [
      { role: 'user', content: 'Describe TypeScript in structured format.' },
    ], schema)) {
      structuredData += chunk.data;
      process.stdout.write('.');
      if (chunk.done) {
        console.log(' done!');
      }
    }

    try {
      const parsed = JSON.parse(structuredData);
      console.log('   Parsed:', JSON.stringify(parsed, null, 2));
    } catch {
      console.log('   Raw:', structuredData);
    }
    console.log();

    // Event subscription
    console.log('3. subscribeEvents()...');
    const eventPromise = (async () => {
      let count = 0;
      for await (const event of client.subscribeEvents(sessionId)) {
        console.log(`   [Event] ${event.type}: ${event.message || ''}`);
        if (++count >= 5 || event.type === 'EVENT_TYPE_GENERATION_COMPLETED') break;
      }
    })();

    await new Promise(resolve => setTimeout(resolve, 100));
    const response = await client.generate(sessionId, [
      { role: 'user', content: 'Say hello briefly.' },
    ]);
    console.log(`   Response: ${response.message?.content?.substring(0, 50)}...`);

    await Promise.race([eventPromise, new Promise(r => setTimeout(r, 3000))]);
    console.log();

    await client.destroySession(sessionId);

  } finally {
    client.close();
  }

  // ========================================================================
  // Part 2: High-Level Streaming
  // ========================================================================
  console.log('=== Part 2: High-Level Streaming ===\n');

  const openai = createProvider({
    name: 'openai',
    apiKey: process.env.OPENAI_API_KEY || 'sk-xxx',
  });

  // streamText with textStream
  console.log('4. streamText() — textStream...');
  const result1 = streamText({
    model: openai('gpt-4o'),
    prompt: 'Write a haiku about coding.',
  });
  process.stdout.write('   ');
  for await (const chunk of result1.textStream) {
    process.stdout.write(chunk);
  }
  console.log('\n');

  // streamText with tools + multi-step
  console.log('5. streamText() — with tools...');
  const weather = tool({
    description: 'Get weather for a city',
    parameters: {
      type: 'object',
      properties: { city: { type: 'string' } },
      required: ['city'],
    },
    execute: async ({ city }) => ({
      city,
      temperature: Math.round(60 + Math.random() * 30),
      condition: 'sunny',
    }),
  });

  const result2 = streamText({
    model: openai('gpt-4o'),
    prompt: 'What is the weather in Tokyo? Be brief.',
    tools: { weather },
    maxSteps: 5,
    onStepFinish: (step) => {
      if (step.toolCalls.length > 0) {
        console.log(`   [Step ${step.stepIndex}] ${step.toolCalls.length} tool call(s)`);
      }
    },
  });

  process.stdout.write('   ');
  for await (const chunk of result2.textStream) {
    process.stdout.write(chunk);
  }
  const steps = await result2.steps;
  console.log(`\n   (${steps.length} step(s))\n`);

  // streamText — fullStream
  console.log('6. streamText() — fullStream...');
  const result3 = streamText({
    model: openai('gpt-4o'),
    prompt: 'Say hello briefly.',
  });
  for await (const chunk of result3.fullStream) {
    if (chunk.type === 'content' && chunk.content) {
      process.stdout.write(chunk.content);
    } else if (chunk.type === 'done') {
      console.log('\n   [Complete]');
    }
  }
  console.log();

  // streamObject
  console.log('7. streamObject()...');
  const { partialStream, object: finalObject } = streamObject({
    model: openai('gpt-4o'),
    schema: JSON.stringify({
      type: 'object',
      properties: {
        title: { type: 'string' },
        summary: { type: 'string' },
        tags: { type: 'array', items: { type: 'string' } },
      },
      required: ['title', 'summary', 'tags'],
    }),
    prompt: 'Describe TypeScript in structured format.',
  });

  process.stdout.write('   Streaming: ');
  for await (const _partial of partialStream) {
    process.stdout.write('.');
  }
  console.log(' done!');
  const result = await finalObject;
  console.log('   Parsed:', JSON.stringify(result, null, 2));
  console.log();

  console.log('='.repeat(60));
  console.log('Streaming demo complete! ✓');
  console.log('='.repeat(60));
}

main().catch(console.error);
