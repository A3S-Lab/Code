/**
 * Provider Configuration Example
 *
 * Demonstrates provider management:
 * - createProvider() for high-level API usage
 * - Multiple providers (OpenAI, Anthropic, custom endpoints)
 * - Low-level provider management via A3sClient
 */

import {
  generateText,
  streamText,
  createProvider,
  A3sClient,
} from '@a3s-lab/code';

async function main() {
  console.log('='.repeat(60));
  console.log('Provider Configuration Example');
  console.log('='.repeat(60));
  console.log();

  // ========================================================================
  // Part 1: High-Level API — createProvider()
  // ========================================================================
  console.log('=== Part 1: createProvider() ===\n');

  // OpenAI
  const openai = createProvider({
    name: 'openai',
    apiKey: process.env.OPENAI_API_KEY || 'sk-xxx',
  });

  // Anthropic
  const anthropic = createProvider({
    name: 'anthropic',
    apiKey: process.env.ANTHROPIC_API_KEY || 'sk-ant-xxx',
  });

  // Custom endpoint (KIMI, local models, etc.)
  const kimi = createProvider({
    name: 'openai',
    apiKey: process.env.KIMI_API_KEY || 'sk-xxx',
    baseUrl: 'http://your-endpoint/v1',
  });

  console.log('Providers created:');
  console.log('  - openai (gpt-4o, gpt-3.5-turbo)');
  console.log('  - anthropic (claude-sonnet-4-20250514)');
  console.log('  - kimi (kimi-k2.5, custom endpoint)');
  console.log();

  // Use different models with the same API
  console.log('1. generateText() with OpenAI...');
  const { text: gptText } = await generateText({
    model: openai('gpt-4o'),
    prompt: 'Say hello in one word.',
  });
  console.log(`  GPT-4o: ${gptText}`);
  console.log();

  console.log('2. streamText() with Anthropic...');
  const result = streamText({
    model: anthropic('claude-sonnet-4-20250514'),
    prompt: 'Say hello in one word.',
  });
  process.stdout.write('  Claude: ');
  for await (const chunk of result.textStream) {
    process.stdout.write(chunk);
  }
  console.log('\n');

  // ========================================================================
  // Part 2: Low-Level API — Provider Management
  // ========================================================================
  console.log('=== Part 2: Low-Level Provider Management ===\n');

  const client = new A3sClient({
    address: process.env.A3S_ADDRESS || 'localhost:4088',
  });

  try {
    // Add providers
    console.log('3. Adding providers via A3sClient...');
    await client.addProvider({
      name: 'anthropic',
      apiKey: process.env.ANTHROPIC_API_KEY || 'sk-ant-xxx',
      baseUrl: 'https://api.anthropic.com',
      models: [
        {
          id: 'claude-sonnet-4-20250514',
          name: 'Claude Sonnet 4',
          family: 'claude-sonnet',
          toolCall: true,
          cost: { input: 3.0, output: 15.0 },
          limit: { context: 200000, output: 8192 },
        },
      ],
    });
    console.log('  ✓ Anthropic provider added');

    await client.addProvider({
      name: 'openai',
      apiKey: process.env.OPENAI_API_KEY || 'sk-xxx',
      baseUrl: 'https://api.openai.com/v1',
      models: [
        {
          id: 'gpt-4o',
          name: 'GPT-4o',
          family: 'gpt-4',
          toolCall: true,
          cost: { input: 5.0, output: 15.0 },
          limit: { context: 128000, output: 4096 },
        },
      ],
    });
    console.log('  ✓ OpenAI provider added');
    console.log();

    // List providers
    console.log('4. Listing providers...');
    const providers = await client.listProviders();
    for (const p of providers.providers) {
      console.log(`  ${p.name}: ${p.models.length} model(s)`);
      for (const m of p.models) {
        console.log(`    - ${m.id} (context: ${m.limit?.context || 'N/A'})`);
      }
    }
    console.log();

    // Set default model
    console.log('5. Setting default model...');
    await client.setDefaultModel('anthropic', 'claude-sonnet-4-20250514');
    const defaultModel = await client.getDefaultModel();
    console.log(`  Default: ${defaultModel.provider}/${defaultModel.model}`);
    console.log();

  } finally {
    client.close();
  }

  console.log('='.repeat(60));
  console.log('Provider configuration complete! ✓');
  console.log('='.repeat(60));
}

main().catch(console.error);
