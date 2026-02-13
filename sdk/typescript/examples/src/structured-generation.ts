/**
 * Structured Generation Example
 *
 * Shows session.generateObject() and session.streamObject()
 * for getting typed JSON responses from the LLM.
 */

import { A3sClient, createProvider } from '@a3s-lab/code';

const A3S_ADDRESS = process.env.A3S_ADDRESS || 'localhost:4088';

async function main() {
  console.log('=== Structured Generation ===\n');

  const client = new A3sClient({ address: A3S_ADDRESS });
  const openai = createProvider({
    name: 'openai',
    apiKey: process.env.OPENAI_API_KEY || '',
  });

  // --- generateObject ---
  console.log('--- generateObject ---');
  {
    await using session = await client.createSession({
      model: openai('gpt-4o'),
      workspace: '/tmp/a3s-workspace',
    });

    const schema = JSON.stringify({
      type: 'object',
      properties: {
        name: { type: 'string' },
        description: { type: 'string' },
        features: { type: 'array', items: { type: 'string' } },
        version: { type: 'string' },
      },
      required: ['name', 'description', 'features'],
    });

    const { object, data, usage } = await session.generateObject({
      schema,
      prompt: 'Describe the TypeScript programming language.',
    });

    console.log('Object:', JSON.stringify(object, null, 2));
    console.log('Raw data length:', data.length);
    console.log('Usage:', usage);
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
        items: {
          type: 'array',
          items: {
            type: 'object',
            properties: {
              file: { type: 'string' },
              purpose: { type: 'string' },
              linesOfCode: { type: 'number' },
            },
          },
        },
      },
    });

    const { partialStream, object } = session.streamObject({
      schema,
      prompt: 'Describe 3 typical files in a Node.js project.',
    });

    let chunkCount = 0;
    for await (const partial of partialStream) {
      chunkCount++;
      if (chunkCount % 5 === 0) process.stdout.write('.');
    }
    console.log(`\nReceived ${chunkCount} chunks`);

    const result = await object;
    console.log('Result:', JSON.stringify(result, null, 2));
  }

  client.close();
  console.log('\nDone!');
}

main().catch(console.error);
