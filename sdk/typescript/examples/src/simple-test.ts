/**
 * Simple A3S SDK Test
 *
 * A simplified test that demonstrates basic SDK usage
 * using the a3s/.a3s configuration directory.
 */

import { A3sClient, loadConfigFromDir } from '@a3s-lab/code';
import { join, dirname } from 'path';
import { fileURLToPath } from 'url';

// Get the directory of this module
const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);

// Path to a3s/.a3s config directory (relative to examples/src)
const configDir = join(__dirname, '..', '..', '..', '..', '.a3s');

async function runSimpleTest(): Promise<void> {
  console.log('='.repeat(60));
  console.log('A3S SDK Simple Test');
  console.log('='.repeat(60));
  console.log();

  // Load config to get API key and base URL
  const config = loadConfigFromDir(configDir);
  console.log('Config loaded:');
  console.log(`  Default provider: ${config?.defaultProvider}`);
  console.log(`  Default model: ${config?.defaultModel}`);
  console.log(`  Base URL: ${config?.baseUrl}`);
  console.log(`  API Key: ${config?.apiKey ? '(set)' : '(not set)'}`);
  console.log();

  // Create client with config directory
  console.log('1. Creating A3S client...');
  const client = new A3sClient({
    address: process.env.A3S_ADDRESS || 'localhost:4088',
    configDir: configDir
  });
  console.log('✓ Client created');
  console.log(`  Address: ${client.getAddress()}`);
  console.log(`  Config dir: ${client.getConfigDir()}`);
  console.log();

  try {
    // Health check
    console.log('2. Checking agent health...');
    const health = await client.healthCheck();
    console.log(`✓ Health status: ${health.status}`);
    console.log(`  Message: ${health.message}`);
    console.log();

    // Initialize agent if needed
    if (health.status === 'STATUS_DEGRADED' || health.status === 'STATUS_UNHEALTHY') {
      console.log('3. Initializing agent...');
      const initResult = await client.initialize('/tmp/test-workspace', {
        NODE_ENV: 'test'
      });
      console.log(`✓ Agent initialized: ${initResult.success}`);
      console.log(`  Message: ${initResult.message}`);
      console.log();
    }

    // Get capabilities
    console.log('3. Getting agent capabilities...');
    const capabilities = await client.getCapabilities();
    console.log('✓ Capabilities retrieved:');
    if (capabilities.info) {
      console.log(`  Agent: ${capabilities.info.name} v${capabilities.info.version}`);
    }
    console.log(`  Features: ${capabilities.features.length}`);
    console.log(`  Tools: ${capabilities.tools.length}`);
    console.log();

    // Create a session
    console.log('4. Creating a session...');
    const session = await client.createSession({
      name: 'simple-test',
      workspace: '/tmp/test',
      systemPrompt: 'You are a helpful assistant.',
      llm: {
        provider: 'anthropic',
        model: 'claude-sonnet-4-20250514'
      }
    });
    console.log(`✓ Session created: ${session.sessionId}`);
    console.log();

    // Configure session with LLM settings
    console.log('5. Configuring session with LLM...');
    await client.configureSession(session.sessionId, {
      name: 'simple-test',
      workspace: '/tmp/test',
      systemPrompt: 'You are a helpful assistant.',
      llm: {
        provider: config?.defaultProvider || 'anthropic',
        model: config?.defaultModel || 'claude-sonnet-4-20250514',
        apiKey: config?.apiKey,
        baseUrl: config?.baseUrl
      }
    });
    console.log('✓ Session configured');
    console.log();

    // List sessions
    console.log('6. Listing sessions...');
    const sessions = await client.listSessions();
    console.log(`✓ Found ${sessions.sessions.length} sessions`);
    console.log();

    // Get context usage
    console.log('7. Getting context usage...');
    const contextUsage = await client.getContextUsage(session.sessionId);
    if (contextUsage.usage) {
      console.log('✓ Context usage:');
      console.log(`  Total tokens: ${contextUsage.usage.totalTokens}`);
      console.log(`  Messages: ${contextUsage.usage.messageCount}`);
    }
    console.log();

    // Generate a response
    console.log('8. Generating a response...');
    const response = await client.generate(session.sessionId, [
      { role: 'user', content: 'Say hello in one word' }
    ]);
    console.log('✓ Response received:');
    if (response.message) {
      console.log(`  Content: ${response.message.content}`);
    }
    console.log(`  Finish reason: ${response.finishReason}`);
    console.log();

    // Streaming generation
    console.log('9. Testing streaming generation...');
    process.stdout.write('   Response: ');
    const stream = client.streamGenerate(session.sessionId, [
      { role: 'user', content: 'Count from 1 to 3' }
    ]);

    for await (const chunk of stream) {
      if (chunk.type === 'content' && chunk.content) {
        process.stdout.write(chunk.content);
      }
    }
    console.log();
    console.log('✓ Streaming complete');
    console.log();

    // Get messages
    console.log('10. Getting message history...');
    const messages = await client.getMessages(session.sessionId, 10);
    console.log(`✓ Retrieved ${messages.messages.length} messages`);
    console.log();

    // Don't destroy session - keep it for verification
    console.log('11. Session preserved for verification');
    console.log(`   Session ID: ${session.sessionId}`);
    console.log(`   Check: /tmp/a3s-workspace/sessions/`);
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

runSimpleTest().catch(error => {
  console.error('Fatal error:', error);
  process.exit(1);
});
