/**
 * High-Level AI Functions
 *
 * Vercel AI SDK-style API for A3S Code Agent.
 * These functions automatically manage session lifecycle,
 * so you don't need to manually create/destroy sessions.
 *
 * @example
 * ```typescript
 * import { generateText, streamText, createProvider } from '@a3s-lab/code';
 *
 * const openai = createProvider({ name: 'openai', apiKey: 'sk-xxx' });
 *
 * // One-shot generation
 * const { text } = await generateText({
 *   model: openai('gpt-4o'),
 *   prompt: 'Explain this codebase',
 *   workspace: '/project',
 * });
 *
 * // Streaming generation
 * const { textStream } = streamText({
 *   model: openai('gpt-4o'),
 *   prompt: 'Explain this codebase',
 * });
 * for await (const chunk of textStream) {
 *   process.stdout.write(chunk);
 * }
 * ```
 */

import { A3sClient } from './client.js';
import type {
  A3sClientOptions,
  Message,
  Usage,
  FinishReason,
  ToolCall,
  GenerateChunk,
} from './client.js';
import type { OpenAIMessage } from './openai-compat.js';
import type { ModelRef } from './provider.js';
import { modelRefToLLMConfig } from './provider.js';

// ============================================================================
// Shared Types
// ============================================================================

/** Message input — supports both A3S and OpenAI formats */
export type MessageInput = Message | OpenAIMessage;

/** Base options shared by all generation functions */
export interface BaseGenerateOptions {
  /** Model reference from createProvider() */
  model: ModelRef;
  /** Working directory for tool sandboxing */
  workspace?: string;
  /** System prompt */
  system?: string;
  /** gRPC server connection options */
  server?: A3sClientOptions;
}

/** Options for text generation */
export interface GenerateTextOptions extends BaseGenerateOptions {
  /** Simple text prompt (creates a single user message) */
  prompt?: string;
  /** Full message array for multi-turn input */
  messages?: MessageInput[];
}

/** Options for structured output generation */
export interface GenerateObjectOptions extends BaseGenerateOptions {
  /** Simple text prompt */
  prompt?: string;
  /** Full message array */
  messages?: MessageInput[];
  /** JSON schema string for structured output */
  schema: string;
}

// ============================================================================
// Result Types
// ============================================================================

/** Result from generateText() */
export interface GenerateTextResult {
  /** Generated text content */
  text: string;
  /** Token usage statistics */
  usage?: Usage;
  /** Why generation stopped */
  finishReason: FinishReason;
  /** Tool calls made during generation */
  toolCalls: ToolCall[];
}

/** Result from streamText() */
export interface StreamTextResult {
  /** Async iterable of text chunks (content only) */
  textStream: AsyncIterable<string>;
  /** Async iterable of full event chunks */
  fullStream: AsyncIterable<GenerateChunk>;
  /** Promise that resolves to the complete text */
  text: Promise<string>;
  /** Promise that resolves to token usage */
  usage: Promise<Usage | undefined>;
  /** Promise that resolves to finish reason */
  finishReason: Promise<FinishReason | undefined>;
}

/** Result from generateObject() */
export interface GenerateObjectResult<T = unknown> {
  /** Parsed object (JSON.parse of the response) */
  object: T;
  /** Raw JSON string */
  data: string;
  /** Token usage statistics */
  usage?: Usage;
}

/** Result from streamObject() */
export interface StreamObjectResult {
  /** Async iterable of partial JSON chunks */
  partialStream: AsyncIterable<string>;
  /** Promise that resolves to the complete parsed object */
  object: Promise<unknown>;
  /** Promise that resolves to the raw JSON string */
  data: Promise<string>;
}

// ============================================================================
// Internal Helpers
// ============================================================================

/** Resolve messages from prompt or messages option */
function resolveMessages(
  prompt?: string,
  messages?: MessageInput[],
): MessageInput[] {
  if (messages && messages.length > 0) return messages;
  if (prompt) return [{ role: 'user', content: prompt }];
  throw new Error('Either "prompt" or "messages" must be provided');
}

/** Create a temporary client + session, run a callback, then clean up */
async function withSession<T>(
  options: BaseGenerateOptions,
  fn: (client: A3sClient, sessionId: string) => Promise<T>,
): Promise<T> {
  const client = new A3sClient(options.server);
  const { sessionId } = await client.createSession({
    name: `auto-${Date.now()}`,
    workspace: options.workspace || '',
    llm: modelRefToLLMConfig(options.model),
    systemPrompt: options.system,
  });

  try {
    return await fn(client, sessionId);
  } finally {
    try {
      await client.destroySession(sessionId);
    } catch {
      // Ignore cleanup errors
    }
    client.close();
  }
}

// ============================================================================
// Core Functions
// ============================================================================

/**
 * Generate text from a language model.
 *
 * Automatically manages session lifecycle. Ideal for non-interactive,
 * one-shot generation tasks.
 *
 * @example
 * ```typescript
 * const { text } = await generateText({
 *   model: openai('gpt-4o'),
 *   prompt: 'Summarize this file',
 *   workspace: '/project',
 * });
 * console.log(text);
 * ```
 */
export async function generateText(
  options: GenerateTextOptions,
): Promise<GenerateTextResult> {
  const messages = resolveMessages(options.prompt, options.messages);

  return withSession(options, async (client, sessionId) => {
    const response = await client.generate(sessionId, messages);
    return {
      text: response.message?.content || '',
      usage: response.usage,
      finishReason: response.finishReason,
      toolCalls: response.toolCalls,
    };
  });
}

/**
 * Stream text from a language model.
 *
 * Returns immediately with stream handles. Session is automatically
 * cleaned up when the stream ends.
 *
 * @example
 * ```typescript
 * const { textStream } = streamText({
 *   model: openai('gpt-4o'),
 *   prompt: 'Explain this codebase',
 * });
 * for await (const chunk of textStream) {
 *   process.stdout.write(chunk);
 * }
 * ```
 */
export function streamText(options: GenerateTextOptions): StreamTextResult {
  const messages = resolveMessages(options.prompt, options.messages);
  const client = new A3sClient(options.server);

  // Accumulated state
  let fullText = '';
  let finalUsage: Usage | undefined;
  let finalFinishReason: FinishReason | undefined;

  // Deferred promises for final values
  let resolveText: (value: string) => void;
  let resolveUsage: (value: Usage | undefined) => void;
  let resolveFinishReason: (value: FinishReason | undefined) => void;
  let rejectText: (reason: unknown) => void;

  const textPromise = new Promise<string>((res, rej) => {
    resolveText = res;
    rejectText = rej;
  });
  const usagePromise = new Promise<Usage | undefined>((res) => {
    resolveUsage = res;
  });
  const finishReasonPromise = new Promise<FinishReason | undefined>((res) => {
    resolveFinishReason = res;
  });

  // Create session and start streaming
  const chunksPromise = (async function* (): AsyncGenerator<GenerateChunk> {
    const { sessionId } = await client.createSession({
      name: `stream-${Date.now()}`,
      workspace: options.workspace || '',
      llm: modelRefToLLMConfig(options.model),
      systemPrompt: options.system,
    });

    try {
      const stream = client.streamGenerate(sessionId, messages);
      for await (const chunk of stream) {
        if (chunk.content) fullText += chunk.content;
        if (chunk.finishReason) finalFinishReason = chunk.finishReason;
        yield chunk;
      }
      resolveText!(fullText);
      resolveUsage!(finalUsage);
      resolveFinishReason!(finalFinishReason);
    } catch (err) {
      rejectText!(err);
      throw err;
    } finally {
      try {
        await client.destroySession(sessionId);
      } catch {
        // Ignore cleanup errors
      }
      client.close();
    }
  })();

  // Create a tee: one for fullStream, one for textStream
  // We use a shared async generator and buffer approach
  const chunks: GenerateChunk[] = [];
  let streamDone = false;
  let streamError: unknown = null;
  const waiters: Array<() => void> = [];

  // Background consumer that fills the buffer
  const consume = (async () => {
    try {
      for await (const chunk of chunksPromise) {
        chunks.push(chunk);
        // Wake up any waiting consumers
        for (const w of waiters.splice(0)) w();
      }
    } catch (err) {
      streamError = err;
    } finally {
      streamDone = true;
      for (const w of waiters.splice(0)) w();
    }
  })();

  // Suppress unhandled rejection on the background consumer
  consume.catch(() => {});

  function createIterator<T>(
    transform: (chunk: GenerateChunk) => T | null,
  ): AsyncIterable<T> {
    return {
      [Symbol.asyncIterator]() {
        let index = 0;
        return {
          async next(): Promise<IteratorResult<T>> {
            while (true) {
              if (index < chunks.length) {
                const val = transform(chunks[index++]);
                if (val !== null) return { value: val, done: false };
                continue;
              }
              if (streamDone) {
                if (streamError) throw streamError;
                return { value: undefined as T, done: true };
              }
              // Wait for more data
              await new Promise<void>((r) => waiters.push(r));
            }
          },
        };
      },
    };
  }

  return {
    textStream: createIterator((chunk) =>
      chunk.content ? chunk.content : null,
    ),
    fullStream: createIterator((chunk) => chunk),
    text: textPromise,
    usage: usagePromise,
    finishReason: finishReasonPromise,
  };
}

/**
 * Generate a structured object from a language model.
 *
 * @example
 * ```typescript
 * const { object } = await generateObject({
 *   model: openai('gpt-4o'),
 *   schema: JSON.stringify({
 *     type: 'object',
 *     properties: { summary: { type: 'string' } },
 *   }),
 *   prompt: 'Summarize this project',
 * });
 * ```
 */
export async function generateObject<T = unknown>(
  options: GenerateObjectOptions,
): Promise<GenerateObjectResult<T>> {
  const messages = resolveMessages(options.prompt, options.messages);

  return withSession(options, async (client, sessionId) => {
    const response = await client.generateStructured(
      sessionId,
      messages,
      options.schema,
    );
    let parsed: T;
    try {
      parsed = JSON.parse(response.data) as T;
    } catch {
      parsed = response.data as unknown as T;
    }
    return {
      object: parsed,
      data: response.data,
      usage: response.usage,
    };
  });
}

/**
 * Stream a structured object from a language model.
 *
 * @example
 * ```typescript
 * const { partialStream, object } = streamObject({
 *   model: openai('gpt-4o'),
 *   schema: '{"type":"object","properties":{"items":{"type":"array"}}}',
 *   prompt: 'List project files',
 * });
 * for await (const partial of partialStream) {
 *   console.log('partial:', partial);
 * }
 * const result = await object;
 * ```
 */
export function streamObject(options: GenerateObjectOptions): StreamObjectResult {
  const messages = resolveMessages(options.prompt, options.messages);
  const client = new A3sClient(options.server);

  let fullData = '';
  let resolveObject: (value: unknown) => void;
  let resolveData: (value: string) => void;
  let rejectAll: (reason: unknown) => void;

  const objectPromise = new Promise<unknown>((res, rej) => {
    resolveObject = res;
    rejectAll = rej;
  });
  const dataPromise = new Promise<string>((res) => {
    resolveData = res;
  });

  const partialStream: AsyncIterable<string> = {
    [Symbol.asyncIterator]() {
      let started = false;
      let sessionId: string;
      let stream: AsyncIterable<{ data: string; done: boolean }>;

      return {
        async next(): Promise<IteratorResult<string>> {
          if (!started) {
            started = true;
            const resp = await client.createSession({
              name: `structured-${Date.now()}`,
              workspace: options.workspace || '',
              llm: modelRefToLLMConfig(options.model),
              systemPrompt: options.system,
            });
            sessionId = resp.sessionId;
            stream = client.streamGenerateStructured(
              sessionId,
              messages,
              options.schema,
            );
          }

          try {
            const iter = stream[Symbol.asyncIterator]();
            const result = await iter.next();

            if (result.done) {
              resolveData!(fullData);
              try {
                resolveObject!(JSON.parse(fullData));
              } catch {
                resolveObject!(fullData);
              }
              try {
                await client.destroySession(sessionId);
              } catch { /* ignore */ }
              client.close();
              return { value: undefined as unknown as string, done: true };
            }

            fullData += result.value.data;
            return { value: result.value.data, done: false };
          } catch (err) {
            rejectAll!(err);
            try {
              await client.destroySession(sessionId);
            } catch { /* ignore */ }
            client.close();
            throw err;
          }
        },
      };
    },
  };

  return { partialStream, object: objectPromise, data: dataPromise };
}
