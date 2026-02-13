/**
 * KIMI Model Test — Custom Provider Example
 *
 * Demonstrates using a custom provider (KIMI K2.5) with the high-level API:
 * - createProvider() with custom baseUrl
 * - generateText() and streamText() with custom models
 */

import { generateText, streamText, createProvider, loadConfigFromDir } from '@a3s-lab/code';
import { join, dirname } from 'path';
import { fileURLToPath } from 'url';

const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);
const configDir = join(__dirname, '..', '..', '..', '..', '.a3s');

async function main() {
  console.log('='.repeat(60));
  console.log('KIMI K2.5 Model Test — Custom Provider');
  console.log('='.repeat(60));
  console.log();

  // Load config to get KIMI model settings
  const config = loadConfigFromDir(configDir);
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

  // Create provider with custom endpoint
  const kimi = createProvider({
    name: 'openai',
    apiKey: kimiModel.apiKey || '',
    baseUrl: kimiModel.baseUrl,
  });

  // 1. One-shot generation
  console.log('1. generateText()...');
  const { text, finishReason } = await generateText({
    model: kimi('kimi-k2.5'),
    prompt: '用一句话介绍你自己',
    workspace: '/tmp/kimi-test',
  });
  console.log(`✓ Response: ${text}`);
  console.log(`  Finish reason: ${finishReason}`);
  console.log();

  // 2. Streaming generation
  console.log('2. streamText()...');
  const result = streamText({
    model: kimi('kimi-k2.5'),
    prompt: '从1数到5',
    workspace: '/tmp/kimi-test',
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
