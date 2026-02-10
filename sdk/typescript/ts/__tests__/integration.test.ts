/**
 * Integration tests for TypeScript SDK
 *
 * These tests use the real configuration file from ../../.a3s/config.json
 */

import { describe, it, expect } from 'vitest';
import { loadConfigFromFile, loadDefaultConfig, getConfig } from '../config.js';
import { join, dirname } from 'path';
import { existsSync } from 'fs';
import { fileURLToPath } from 'url';

const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);
const CONFIG_PATH = join(__dirname, '..', '..', '..', '..', '.a3s', 'config.json');

describe('Integration Tests - Configuration', () => {
  it('should load config from .a3s/config.json', () => {
    if (!existsSync(CONFIG_PATH)) {
      console.log('Config file not found, skipping test');
      return;
    }

    const config = loadConfigFromFile(CONFIG_PATH);
    expect(config).toBeDefined();
    expect(config?.defaultProvider).toBeDefined();
    expect(config?.defaultModel).toBeDefined();
    expect(config?.providers).toBeDefined();
    expect(config?.providers?.length).toBeGreaterThan(0);

    console.log('✓ Loaded config from .a3s/config.json');
    console.log(`  Default provider: ${config?.defaultProvider}`);
    console.log(`  Default model: ${config?.defaultModel}`);
    console.log(`  Providers: ${config?.providers?.length}`);
  });

  it('should have correct default provider', () => {
    if (!existsSync(CONFIG_PATH)) {
      return;
    }

    const config = loadConfigFromFile(CONFIG_PATH);
    // Default provider should be one of the configured providers
    expect(config?.defaultProvider).toBeDefined();
    expect(['anthropic', 'openai', 'kimi']).toContain(config?.defaultProvider);
  });

  it('should have providers with models', () => {
    if (!existsSync(CONFIG_PATH)) {
      return;
    }

    const config = loadConfigFromFile(CONFIG_PATH);
    const provider = config?.providers?.find(p => p.name === config.defaultProvider);

    expect(provider).toBeDefined();
    expect(provider?.models).toBeDefined();
    expect(provider?.models?.length).toBeGreaterThan(0);

    console.log(`✓ Found provider: ${provider?.name}`);
    console.log(`  Models: ${provider?.models?.length}`);
  });

  it('should have default model in provider', () => {
    if (!existsSync(CONFIG_PATH)) {
      return;
    }

    const config = loadConfigFromFile(CONFIG_PATH);
    const provider = config?.providers?.find(p => p.name === config.defaultProvider);
    const model = provider?.models?.find(m => m.id === config.defaultModel);

    expect(model).toBeDefined();
    expect(model?.toolCall).toBe(true);

    console.log(`✓ Found model: ${model?.name} (${model?.id})`);
    console.log(`  Tool Call: ${model?.toolCall}`);
    console.log(`  Reasoning: ${model?.reasoning}`);
    console.log(`  Attachment: ${model?.attachment}`);
  });

  it('should extract API key from provider', () => {
    if (!existsSync(CONFIG_PATH)) {
      return;
    }

    const config = loadConfigFromFile(CONFIG_PATH);
    expect(config?.apiKey).toBeDefined();
    expect(config?.apiKey).not.toBe('');

    console.log('✓ API key extracted from provider');
  });

  it('should have base URL configured', () => {
    if (!existsSync(CONFIG_PATH)) {
      return;
    }

    const config = loadConfigFromFile(CONFIG_PATH);
    expect(config?.baseUrl).toBeDefined();

    console.log(`✓ Base URL: ${config?.baseUrl}`);
  });

  it('should list all available models', () => {
    if (!existsSync(CONFIG_PATH)) {
      return;
    }

    const config = loadConfigFromFile(CONFIG_PATH);
    const allModels: Array<{ provider: string; model: any }> = [];

    config?.providers?.forEach(provider => {
      provider.models?.forEach(model => {
        allModels.push({ provider: provider.name, model });
      });
    });

    expect(allModels.length).toBeGreaterThan(0);

    console.log(`✓ Available models (${allModels.length}):`);
    allModels.forEach(({ provider, model }) => {
      console.log(`  - ${provider}/${model.id}: ${model.name} (tool_call: ${model.toolCall})`);
    });
  });

  it('should have model cost information', () => {
    if (!existsSync(CONFIG_PATH)) {
      return;
    }

    const config = loadConfigFromFile(CONFIG_PATH);
    const provider = config?.providers?.find(p => p.name === config.defaultProvider);
    const model = provider?.models?.find(m => m.id === config.defaultModel);

    // Cost information is optional - some models may not have it
    if (model?.cost) {
      console.log('✓ Model cost (per million tokens):');
      console.log(`  Input: $${model?.cost?.input}`);
      console.log(`  Output: $${model?.cost?.output}`);
      console.log(`  Cache Read: $${model?.cost?.cacheRead}`);
      console.log(`  Cache Write: $${model?.cost?.cacheWrite}`);
    } else {
      console.log('⚠ Model cost information not available for this model');
    }
  });

  it('should have model limits', () => {
    if (!existsSync(CONFIG_PATH)) {
      return;
    }

    const config = loadConfigFromFile(CONFIG_PATH);
    const provider = config?.providers?.find(p => p.name === config.defaultProvider);
    const model = provider?.models?.find(m => m.id === config.defaultModel);

    expect(model?.limit).toBeDefined();

    console.log('✓ Model limits:');
    console.log(`  Context: ${model?.limit?.context} tokens`);
    console.log(`  Output: ${model?.limit?.output} tokens`);
  });

  it('should have model modalities', () => {
    if (!existsSync(CONFIG_PATH)) {
      return;
    }

    const config = loadConfigFromFile(CONFIG_PATH);
    const provider = config?.providers?.find(p => p.name === config.defaultProvider);
    const model = provider?.models?.find(m => m.id === config.defaultModel);

    expect(model?.modalities).toBeDefined();

    console.log('✓ Model modalities:');
    console.log(`  Input: ${JSON.stringify(model?.modalities?.input)}`);
    console.log(`  Output: ${JSON.stringify(model?.modalities?.output)}`);
  });

  it('should find alternate provider', () => {
    if (!existsSync(CONFIG_PATH)) {
      return;
    }

    const config = loadConfigFromFile(CONFIG_PATH);
    const openaiProvider = config?.providers?.find(p => p.name === 'openai');

    if (openaiProvider) {
      console.log(`✓ Found alternate provider: ${openaiProvider.name}`);
      console.log(`  Models: ${openaiProvider.models?.length}`);

      openaiProvider.models?.forEach(model => {
        console.log(`  - ${model.id}: ${model.name}`);
        if (model.baseUrl) {
          console.log(`    Base URL: ${model.baseUrl}`);
        }
      });
    } else {
      console.log('  No alternate provider "openai" configured');
    }
  });
});
