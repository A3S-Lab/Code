/**
 * Provider Configuration Example
 *
 * Demonstrates how to manage LLM providers and models:
 * - Add multiple providers (Anthropic, OpenAI, etc.)
 * - Configure models with costs and limits
 * - Set default models
 * - Switch models per session
 * - List available providers and models
 */

import { A3sClient, loadConfigFromDir } from '@a3s-lab/code';
import { join, dirname } from 'path';
import { fileURLToPath } from 'url';

const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);
const configDir = join(__dirname, '..', '..', '..', '..', '.a3s');

async function runProviderConfigExample(): Promise<void> {
  console.log('='.repeat(60));
  console.log('Provider Configuration Example');
  console.log('='.repeat(60));
  console.log();

  const client = new A3sClient({
    address: process.env.A3S_ADDRESS || 'localhost:4088',
    configDir: configDir
  });

  try {
    // Example 1: Add Anthropic provider
    console.log('1. Adding Anthropic provider...');
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
          temperature: true,
          attachment: true,
          reasoning: true,
          cost: {
            input: 3.0,
            output: 15.0,
            cacheRead: 0.3,
            cacheWrite: 3.75
          },
          limit: {
            context: 200000,
            output: 8192
          },
          modalities: {
            input: ['text', 'image'],
            output: ['text']
          }
        },
        {
          id: 'claude-opus-4-20250514',
          name: 'Claude Opus 4',
          family: 'claude-opus',
          toolCall: true,
          temperature: true,
          attachment: true,
          reasoning: true,
          cost: {
            input: 15.0,
            output: 75.0,
            cacheRead: 1.5,
            cacheWrite: 18.75
          },
          limit: {
            context: 200000,
            output: 16384
          }
        }
      ]
    });
    console.log('✓ Anthropic provider added');
    console.log('  Models: Claude Sonnet 4, Claude Opus 4');
    console.log();

    // Example 2: Add OpenAI provider
    console.log('2. Adding OpenAI provider...');
    await client.addProvider({
      name: 'openai',
      apiKey: process.env.OPENAI_API_KEY || 'sk-xxx',
      baseUrl: 'https://api.openai.com/v1',
      models: [
        {
          id: 'gpt-4-turbo',
          name: 'GPT-4 Turbo',
          family: 'gpt-4',
          toolCall: true,
          temperature: true,
          cost: {
            input: 10.0,
            output: 30.0
          },
          limit: {
            context: 128000,
            output: 4096
          }
        },
        {
          id: 'gpt-3.5-turbo',
          name: 'GPT-3.5 Turbo',
          family: 'gpt-3.5',
          toolCall: true,
          temperature: true,
          cost: {
            input: 0.5,
            output: 1.5
          },
          limit: {
            context: 16385,
            output: 4096
          }
        }
      ]
    });
    console.log('✓ OpenAI provider added');
    console.log('  Models: GPT-4 Turbo, GPT-3.5 Turbo');
    console.log();

    // Example 3: List all providers
    console.log('3. Listing all providers...');
    const providers = await client.listProviders();
    console.log(`✓ Total providers: ${providers.providers.length}`);
    console.log();

    for (const provider of providers.providers) {
      console.log(`Provider: ${provider.name}`);
      console.log(`  Base URL: ${provider.baseUrl}`);
      console.log(`  Models: ${provider.models.length}`);

      for (const model of provider.models) {
        console.log(`    - ${model.id} (${model.name})`);
        console.log(`      Context: ${model.limit?.context || 'N/A'} tokens`);
        console.log(`      Output: ${model.limit?.output || 'N/A'} tokens`);
        console.log(`      Cost: $${model.cost?.input || 0}/M input, $${model.cost?.output || 0}/M output`);
        console.log(`      Features: ${[
          model.toolCall && 'Tool Call',
          model.temperature && 'Temperature',
          model.attachment && 'Attachments',
          model.reasoning && 'Reasoning'
        ].filter(Boolean).join(', ')}`);
      }
      console.log();
    }

    // Example 4: Set default model
    console.log('4. Setting default model...');
    await client.setDefaultModel('anthropic', 'claude-sonnet-4-20250514');
    console.log('✓ Default model set: anthropic/claude-sonnet-4-20250514');
    console.log();

    // Example 5: Get default model
    console.log('5. Getting default model...');
    const defaultModel = await client.getDefaultModel();
    console.log('✓ Current default:');
    console.log(`  Provider: ${defaultModel.provider}`);
    console.log(`  Model: ${defaultModel.model}`);
    console.log();

    // Example 6: Create session with specific model
    console.log('6. Creating session with GPT-4...');
    const gpt4Session = await client.createSession({
      name: 'GPT-4 Session',
      workspace: '/tmp/workspace',
      llm: {
        provider: 'openai',
        model: 'gpt-4-turbo',
        apiKey: process.env.OPENAI_API_KEY
      }
    });
    console.log(`✓ Session created: ${gpt4Session.sessionId}`);
    console.log('  Using: openai/gpt-4-turbo');
    console.log();

    // Example 7: Create session with Claude
    console.log('7. Creating session with Claude Sonnet...');
    const claudeSession = await client.createSession({
      name: 'Claude Session',
      workspace: '/tmp/workspace',
      llm: {
        provider: 'anthropic',
        model: 'claude-sonnet-4-20250514'
      }
    });
    console.log(`✓ Session created: ${claudeSession.sessionId}`);
    console.log('  Using: anthropic/claude-sonnet-4-20250514');
    console.log();

    // Example 8: Switch model for existing session
    console.log('8. Switching model for existing session...');
    await client.configureSession(gpt4Session.sessionId, {
      llm: {
        provider: 'anthropic',
        model: 'claude-opus-4-20250514'
      }
    });
    console.log('✓ Session model switched:');
    console.log('  From: openai/gpt-4-turbo');
    console.log('  To: anthropic/claude-opus-4-20250514');
    console.log();

    // Example 9: Get provider details
    console.log('9. Getting provider details...');
    const anthropicProvider = await client.getProvider('anthropic');
    console.log('✓ Anthropic provider details:');
    console.log(`  Name: ${anthropicProvider.name}`);
    console.log(`  Base URL: ${anthropicProvider.baseUrl}`);
    console.log(`  Models: ${anthropicProvider.models.length}`);
    console.log();

    // Example 10: Update provider
    console.log('10. Updating provider configuration...');
    await client.updateProvider('anthropic', {
      baseUrl: 'https://api.anthropic.com/v1',  // Updated URL
      apiKey: process.env.ANTHROPIC_API_KEY
    });
    console.log('✓ Provider updated');
    console.log();

    // Cleanup
    console.log('11. Cleanup...');
    await client.destroySession(gpt4Session.sessionId);
    await client.destroySession(claudeSession.sessionId);
    console.log('✓ Sessions destroyed');
    console.log();

    console.log('='.repeat(60));
    console.log('Provider Configuration Example Complete');
    console.log('='.repeat(60));
    console.log();
    console.log('Key features:');
    console.log('  ✓ Multiple provider support');
    console.log('  ✓ Model cost and limit tracking');
    console.log('  ✓ Per-session model configuration');
    console.log('  ✓ Runtime model switching');
    console.log('  ✓ Default model management');

  } catch (error) {
    console.error('Error:', error);
    throw error;
  }
}

// Run the example
runProviderConfigExample().catch(console.error);
