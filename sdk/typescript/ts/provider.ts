/**
 * Provider Factory
 *
 * Vercel AI SDK-style provider abstraction for A3S Code.
 *
 * @example
 * ```typescript
 * import { createProvider } from '@a3s-lab/code';
 *
 * const openai = createProvider({ name: 'openai', apiKey: 'sk-xxx' });
 * const kimi = createProvider({ name: 'kimi', apiKey: 'sk-xxx', baseUrl: 'http://xxx/v1' });
 *
 * // Use as model selector
 * const model = openai('gpt-4o');
 * const model2 = kimi('k2.5');
 * ```
 */

import type { LLMConfig } from './client.js';

/**
 * Provider configuration
 */
export interface ProviderOptions {
  /** Provider name (e.g., 'openai', 'anthropic', 'kimi') */
  name: string;
  /** API key */
  apiKey: string;
  /** Base URL override */
  baseUrl?: string;
}

/**
 * Model reference — a resolved provider + model pair
 */
export interface ModelRef {
  provider: string;
  model: string;
  apiKey: string;
  baseUrl?: string;
}

/**
 * Model selector function returned by createProvider()
 */
export type ModelSelector = (modelId: string) => ModelRef;

/**
 * Create a provider factory that returns model references.
 *
 * @example
 * ```typescript
 * const openai = createProvider({ name: 'openai', apiKey: 'sk-xxx' });
 * const model = openai('gpt-4o');
 * ```
 */
export function createProvider(options: ProviderOptions): ModelSelector {
  return (modelId: string): ModelRef => ({
    provider: options.name,
    model: modelId,
    apiKey: options.apiKey,
    baseUrl: options.baseUrl,
  });
}

/**
 * Shorthand: create a provider and select a model in one call.
 *
 * @example
 * ```typescript
 * const model = model({ provider: 'openai', model: 'gpt-4o', apiKey: 'sk-xxx' });
 * ```
 */
export function model(config: LLMConfig): ModelRef {
  return {
    provider: config.provider,
    model: config.model,
    apiKey: config.apiKey || '',
    baseUrl: config.baseUrl,
  };
}

/** Convert ModelRef to LLMConfig for the underlying client */
export function modelRefToLLMConfig(ref: ModelRef): LLMConfig {
  return {
    provider: ref.provider,
    model: ref.model,
    apiKey: ref.apiKey,
    baseUrl: ref.baseUrl,
  };
}
