/**
 * KIMI Model Test
 *
 * Tests the SDK with KIMI K2.5 model as an alternative to Anthropic.
 * Uses configuration from a3s/.a3s/config.json
 */

import { A3sClient, loadConfigFromDir } from '@a3s-lab/code';
import { join, dirname } from 'path';
import { fileURLToPath } from 'url';

// Get the directory of this module
const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);

// Path to a3s/.a3s config directory
const configDir = join(__dirname, '..', '..', '..', '..', '.a3s');

async function runKimiTest(): Promise<void> {
  console.log('='.repeat(60));
  console.log('KIMI K2.5 Model Test');
  console.log('='.repeat(60));
  console.log();

  // Load config
  const config = loadConfigFromDir(configDir);
  console.log('Config loaded from:', configDir);
  console.log();

  // Find KIMI model configuration
  const openaiProvider = config?.providers?.find(p => p.name === 'openai');
  const kimiModel = openaiProvider?.models?.find(m => m.id === 'kimi-k2.5');

  if (!kimiModel) {
    console.error('✗ KIMI K2.5 model not found in config');
    process.exit(1);
  }

  console.log('KIMI Model Configuration:');
  console.log(`  Model ID: ${kimiModel.id}`);
  console.log(`  Name: ${kimiModel.name}`);
  console.log(`  Base URL: ${kimiModel.baseUrl}`);
  console.log(`  API Key: ${kimiModel.apiKey ? '(set)' : '(not set)'}`);
  console.log();

  // Create client
  console.log('1. Creating A3S client...');
  const client = new A3sClient({
    address: process.env.A3S_ADDRESS || 'localhost:4088',
    configDir: configDir
  });
  console.log('✓ Client created');
  console.log();

  try {
    // Health check
    console.log('2. Checking agent health...');
    const health = await client.healthCheck();
    console.log(`✓ Health status: ${health.status}`);
    console.log();

    // Create a session with KIMI model
    console.log('3. Creating session with KIMI model...');
    const session = await client.createSession({
      name: 'kimi-test',
      workspace: '/tmp/kimi-test',
      systemPrompt: 'You are a helpful assistant.',
      llm: {
        provider: 'openai',
        model: 'kimi-k2.5',
        apiKey: kimiModel.apiKey,
        baseUrl: kimiModel.baseUrl
      }
    });
    console.log(`✓ Session created: ${session.sessionId}`);
    console.log();

    // Generate a simple response
    console.log('4. Generating a response...');
    const response = await client.generate(session.sessionId, [
      { role: 'user', content: '用一句话介绍你自己' }
    ]);
    console.log('✓ Response received:');
    if (response.message) {
      console.log(`  ${response.message.content}`);
    }
    console.log();

    // Test streaming
    console.log('5. Testing streaming generation...');
    process.stdout.write('   Response: ');
    const stream = client.streamGenerate(session.sessionId, [
      { role: 'user', content: '从1数到5' }
    ]);

    for await (const chunk of stream) {
      if (chunk.type === 'content' && chunk.content) {
        process.stdout.write(chunk.content);
      }
    }
    console.log();
    console.log('✓ Streaming complete');
    console.log();

    // Get context usage
    console.log('6. Getting context usage...');
    const contextUsage = await client.getContextUsage(session.sessionId);
    if (contextUsage.usage) {
      console.log('✓ Context usage:');
      console.log(`  Total tokens: ${contextUsage.usage.totalTokens}`);
      console.log(`  Messages: ${contextUsage.usage.messageCount}`);
    }
    console.log();

    // Cleanup
    console.log('7. Cleaning up...');
    await client.destroySession(session.sessionId);
    console.log('✓ Session destroyed');
    console.log();

    console.log('='.repeat(60));
    console.log('All tests passed! ✓');
    console.log('='.repeat(60));

  } catch (error) {
    console.error('✗ Test failed:', (error as Error).message);
    if (error instanceof Error && error.stack) {
      console.error('Stack:', error.stack);
    }
    process.exit(1);
  } finally {
    client.close();
    console.log();
    console.log('✓ Connection closed');
  }
}

runKimiTest().catch(error => {
  console.error('Fatal error:', error);
  process.exit(1);
});
