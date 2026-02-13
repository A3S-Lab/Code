/**
 * Streaming Demo
 *
 * Shows all streaming APIs on the Session object:
 * - session.streamText() — text streaming with tool support
 * - session.streamObject() — structured output streaming
 */

import { A3sClient, createProvider, tool } from '@a3s-lab/code';

const A3S_ADDRESS = process.env.A3S_ADDRESS || 'localhost:4088';

async function main() {
  console.log('=== Streaming Demo ===\n');

  const client = new A3sClient({ address: A3S_ADDRESS });
  const openai = createProvider({
    name: 'openai',
    apiKey: process.env.OPENAI_API_KEY || '',
  });

  // --- streamText ---
  console.log('--- streamText (basic) ---');
  {
    await using session = await client.createSession({
      model: openai('gpt-4o'),
      workspace: '/tmp/a3s-workspace',
      system: 'You are a helpful assistant. Be concise.',
    });

    const { textStream, text, finishReason } = session.streamText({
      prompt: 'Explain async/await in 3 sentences.',
    });

    process.stdout.write('Streaming: ');
    for await (const chunk of textStream) {
      process.stdout.write(chunk);
    }
    console.log('\n');
    console.log('Full text length:', (await text).length);
    console.log('Finish reason:', await finishReason);
  }

  // --- streamText with tools ---
  console.log('\n--- streamText (with tools) ---');
  {
    await using session = await client.createSession({
      model: openai('gpt-4o'),
      workspace: '/tmp/a3s-workspace',
    });

    const weatherTool = tool({
      description: 'Get weather for a city',
      parameters: {
        type: 'object',
        properties: { city: { type: 'string', description: 'City name' } },
        required: ['city'],
      },
      execute: async ({ city }) => ({
        city,
        temperature: Math.round(Math.random() * 30 + 10),
        condition: 'sunny',
      }),
    });

    const { textStream, steps } = session.streamText({
      prompt: 'What is the weather in Tokyo?',
      tools: { weather: weatherTool },
      maxSteps: 3,
      onStepFinish: (step) => {
        console.log(`  [Step ${step.stepIndex}] tools: ${step.toolCalls.length}`);
      },
    });

    process.stdout.write('Streaming: ');
    for await (const chunk of textStream) {
      process.stdout.write(chunk);
    }
    console.log('\n');
    console.log('Total steps:', (await steps).length);
  }

  // --- streamObject ---
  console.log('\n--- streamObject ---');
  {
    await using session = await client.createSession({
      model: openai('gpt-4o'),
      workspace: '/tmp/a3s-workspace',
    });

    const schema = JSON.stringify({
      type: 'object',
      properties: {
        languages: {
          type: 'array',
          items: {
            type: 'object',
            properties: {
              name: { type: 'string' },
              year: { type: 'number' },
              paradigm: { type: 'string' },
            },
          },
        },
      },
    });

    const { partialStream, object } = session.streamObject({
      schema,
      prompt: 'List 3 programming languages with their year and paradigm.',
    });

    process.stdout.write('Partial chunks: ');
    for await (const partial of partialStream) {
      process.stdout.write('.');
    }
    console.log();

    const result = await object;
    console.log('Result:', JSON.stringify(result, null, 2));
  }

  client.close();
  console.log('\nDone!');
}

main().catch(console.error);
