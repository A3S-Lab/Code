/**
 * Configuration utilities for A3S Code Agent SDK
 */

import { readFileSync, existsSync } from 'fs';
import { join } from 'path';
import { homedir } from 'os';

export interface A3sConfig {
  /** gRPC server address */
  address: string;
  /** Default provider name */
  defaultProvider?: string;
  /** Default model ID */
  defaultModel?: string;
  /** API key for the default provider */
  apiKey?: string;
  /** Base URL override */
  baseUrl?: string;
  /** Provider configurations */
  providers?: ProviderConfig[];
}

export interface ProviderConfig {
  name: string;
  apiKey?: string;
  baseUrl?: string;
  models?: ModelConfigEntry[];
}

export interface ModelConfigEntry {
  id: string;
  name?: string;
  family?: string;
  apiKey?: string;
  baseUrl?: string;
  attachment?: boolean;
  reasoning?: boolean;
  toolCall?: boolean;
  temperature?: boolean;
  releaseDate?: string;
  modalities?: {
    input?: string[];
    output?: string[];
  };
  cost?: {
    input?: number;
    output?: number;
    cacheRead?: number;
    cacheWrite?: number;
  };
  limit?: {
    context?: number;
    output?: number;
  };
}

export interface ModelConfig {
  provider: string;
  model: string;
  apiKey?: string;
  baseUrl?: string;
}

/**
 * Load configuration from a JSON file
 *
 * @param configPath Path to config.json file
 * @returns Parsed configuration or undefined if file doesn't exist
 */
export function loadConfigFromFile(configPath: string): A3sConfig | undefined {
  if (!existsSync(configPath)) {
    return undefined;
  }

  try {
    const content = readFileSync(configPath, 'utf-8');
    const json = JSON.parse(content);

    const config: A3sConfig = {
      address: json.address || process.env.A3S_ADDRESS || 'localhost:4088',
      defaultProvider: json.defaultProvider,
      defaultModel: json.defaultModel,
      providers: json.providers,
    };

    // Extract API key and base URL from default provider if available
    if (config.defaultProvider && config.providers) {
      const provider = config.providers.find(p => p.name === config.defaultProvider);
      if (provider) {
        config.apiKey = provider.apiKey;
        config.baseUrl = provider.baseUrl;

        // Check for model-specific overrides
        if (config.defaultModel && provider.models) {
          const model = provider.models.find(m => m.id === config.defaultModel);
          if (model) {
            config.apiKey = model.apiKey || config.apiKey;
            config.baseUrl = model.baseUrl || config.baseUrl;
          }
        }
      }
    }

    return config;
  } catch (error) {
    console.error(`Failed to load config from ${configPath}:`, error);
    return undefined;
  }
}

/**
 * Load configuration from a directory
 *
 * Looks for config.json in the specified directory.
 *
 * @param configDir Directory containing config.json
 * @returns Parsed configuration or undefined if not found
 */
export function loadConfigFromDir(configDir: string): A3sConfig | undefined {
  const configPath = join(configDir, 'config.json');
  return loadConfigFromFile(configPath);
}

/**
 * Load configuration from default locations
 *
 * Tries to load from (in order):
 * 1. ./config.json (current directory)
 * 2. ~/.a3s/config.json (user home)
 *
 * @returns Parsed configuration or default config
 */
export function loadDefaultConfig(): A3sConfig {
  // Try current directory
  let config = loadConfigFromFile('config.json');
  if (config) {
    return config;
  }

  // Try ~/.a3s/config.json
  const homeConfig = join(homedir(), '.a3s', 'config.json');
  config = loadConfigFromFile(homeConfig);
  if (config) {
    return config;
  }

  // Return default config from environment
  return getConfig();
}

/**
 * Get configuration from environment variables
 *
 * Environment variables:
 * - A3S_ADDRESS: gRPC server address (default: localhost:4088)
 * - A3S_DEFAULT_PROVIDER: Default provider name
 * - A3S_DEFAULT_MODEL: Default model ID
 * - A3S_API_KEY: API key for the default provider
 * - A3S_BASE_URL: Base URL override
 */
export function getConfig(): A3sConfig {
  return {
    address: process.env.A3S_ADDRESS || 'localhost:4088',
    defaultProvider: process.env.A3S_DEFAULT_PROVIDER,
    defaultModel: process.env.A3S_DEFAULT_MODEL,
    apiKey: process.env.A3S_API_KEY,
    baseUrl: process.env.A3S_BASE_URL,
  };
}

/**
 * Get model configuration from environment or defaults
 */
export function getModelConfig(): ModelConfig | undefined {
  const config = getConfig();

  if (!config.defaultProvider || !config.defaultModel) {
    return undefined;
  }

  return {
    provider: config.defaultProvider,
    model: config.defaultModel,
    apiKey: config.apiKey,
    baseUrl: config.baseUrl,
  };
}

/**
 * Get the default model identifier string
 * Returns format: "provider/model" or undefined if not configured
 */
export function getDefaultModel(): string | undefined {
  const config = getConfig();

  if (!config.defaultProvider || !config.defaultModel) {
    return undefined;
  }

  return `${config.defaultProvider}/${config.defaultModel}`;
}

/**
 * Print current configuration to console (for debugging)
 */
export function printConfig(): void {
  const config = getConfig();

  console.log('A3S Configuration:');
  console.log(`  Address: ${config.address}`);
  console.log(`  Default Provider: ${config.defaultProvider || '(not set)'}`);
  console.log(`  Default Model: ${config.defaultModel || '(not set)'}`);
  console.log(`  API Key: ${config.apiKey ? '(set)' : '(not set)'}`);
  console.log(`  Base URL: ${config.baseUrl || '(not set)'}`);
}
