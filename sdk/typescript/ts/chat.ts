/**
 * Chat — Multi-turn Conversation
 *
 * Manages a persistent session for multi-turn conversations.
 *
 * @example
 * ```typescript
 * import { createChat, createProvider } from '@a3s-lab/code';
 *
 * const openai = createProvider({ name: 'openai', apiKey: 'sk-xxx' });
 *
 * const chat = createChat({
 *   model: openai('gpt-4o'),
 *   workspace: '/project',
 *   system: 'You are a helpful code assistant',
 * });
 *
 * const { text } = await chat.send('What does main.rs do?');
 * console.log(text);
 *
 * const { textStream } = chat.stream('Refactor it to be more idiomatic');
 * for await (const chunk of textStream) {
 *   process.stdout.write(chunk);
 * }
 *
 * await chat.close();
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
  ContextUsage,
} from './client.js';
import type { OpenAIMessage } from './openai-compat.js';
import type { ModelRef } from './provider.js';
import { modelRefToLLMConfig } from './provider.js';

// ============================================================================
// Types
// ============================================================================

type MessageInput = Message | OpenAIMessage;

/** Options for creating a chat */
export interface ChatOptions {
  /** Model reference from createProvider() */
  model: ModelRef;
  /** Working directory for tool sandboxing */
  workspace?: string;
  /** System prompt */
  system?: string;
  /** gRPC server connection options */
  server?: A3sClientOptions;
}

/** Result from chat.send() */
export interface ChatSendResult {
  text: string;
  usage?: Usage;
  finishReason: FinishReason;
  toolCalls: ToolCall[];
}

/** Result from chat.stream() */
export interface ChatStreamResult {
  textStream: AsyncIterable<string>;
  fullStream: AsyncIterable<GenerateChunk>;
  text: Promise<string>;
}

/** Chat instance for multi-turn conversations */
export interface Chat {
  /** Send a message and get a complete response */
  send(prompt: string): Promise<ChatSendResult>;
  send(messages: MessageInput[]): Promise<ChatSendResult>;

  /** Send a message and stream the response */
  stream(prompt: string): ChatStreamResult;
  stream(messages: MessageInput[]): ChatStreamResult;

  /** Get context usage for this chat session */
  getUsage(): Promise<ContextUsage | undefined>;

  /** Compact the conversation context */
  compact(): Promise<void>;

  /** Clear conversation history */
  clear(): Promise<void>;

  /** Get the underlying session ID */
  readonly sessionId: string;

  /** Close the chat and clean up resources */
  close(): Promise<void>;
}

// ============================================================================
// Implementation
// ============================================================================

function toMessages(input: string | MessageInput[]): MessageInput[] {
  if (typeof input === 'string') {
    return [{ role: 'user', content: input }];
  }
  return input;
}

/**
 * Create a multi-turn chat session.
 *
 * The session is created lazily on first send/stream call.
 * Call `close()` when done to clean up resources.
 *
 * @example
 * ```typescript
 * const chat = createChat({
 *   model: openai('gpt-4o'),
 *   workspace: '/project',
 *   system: 'You are a code reviewer',
 * });
 *
 * const { text } = await chat.send('Review this PR');
 * console.log(text);
 *
 * await chat.close();
 * ```
 */
export function createChat(options: ChatOptions): Chat {
  const client = new A3sClient(options.server);
  let sessionId = '';
  let initialized = false;
  let initPromise: Promise<void> | null = null;

  async function ensureSession(): Promise<void> {
    if (initialized) return;
    if (initPromise) {
      await initPromise;
      return;
    }
    initPromise = (async () => {
      const resp = await client.createSession({
        name: `chat-${Date.now()}`,
        workspace: options.workspace || '',
        llm: modelRefToLLMConfig(options.model),
        systemPrompt: options.system,
      });
      sessionId = resp.sessionId;
      initialized = true;
    })();
    await initPromise;
  }

  const chat: Chat = {
    get sessionId() {
      return sessionId;
    },

    async send(input: string | MessageInput[]): Promise<ChatSendResult> {
      await ensureSession();
      const messages = toMessages(input);
      const response = await client.generate(sessionId, messages);
      return {
        text: response.message?.content || '',
        usage: response.usage,
        finishReason: response.finishReason,
        toolCalls: response.toolCalls,
      };
    },

    stream(input: string | MessageInput[]): ChatStreamResult {
      const messages = toMessages(input);

      let fullText = '';
      let resolveText: (value: string) => void;
      let rejectText: (reason: unknown) => void;
      const textPromise = new Promise<string>((res, rej) => {
        resolveText = res;
        rejectText = rej;
      });

      async function* generateChunks(): AsyncGenerator<GenerateChunk> {
        await ensureSession();
        try {
          const stream = client.streamGenerate(sessionId, messages);
          for await (const chunk of stream) {
            if (chunk.content) fullText += chunk.content;
            yield chunk;
          }
          resolveText!(fullText);
        } catch (err) {
          rejectText!(err);
          throw err;
        }
      }

      // Buffer-based tee for multiple consumers
      const chunks: GenerateChunk[] = [];
      let done = false;
      let error: unknown = null;
      const waiters: Array<() => void> = [];

      const consume = (async () => {
        try {
          for await (const chunk of generateChunks()) {
            chunks.push(chunk);
            for (const w of waiters.splice(0)) w();
          }
        } catch (err) {
          error = err;
        } finally {
          done = true;
          for (const w of waiters.splice(0)) w();
        }
      })();
      consume.catch(() => {});

      function iterate<T>(
        transform: (c: GenerateChunk) => T | null,
      ): AsyncIterable<T> {
        return {
          [Symbol.asyncIterator]() {
            let i = 0;
            return {
              async next(): Promise<IteratorResult<T>> {
                while (true) {
                  if (i < chunks.length) {
                    const v = transform(chunks[i++]);
                    if (v !== null) return { value: v, done: false };
                    continue;
                  }
                  if (done) {
                    if (error) throw error;
                    return { value: undefined as T, done: true };
                  }
                  await new Promise<void>((r) => waiters.push(r));
                }
              },
            };
          },
        };
      }

      return {
        textStream: iterate((c) => (c.content ? c.content : null)),
        fullStream: iterate((c) => c),
        text: textPromise,
      };
    },

    async getUsage(): Promise<ContextUsage | undefined> {
      await ensureSession();
      const resp = await client.getContextUsage(sessionId);
      return resp.usage;
    },

    async compact(): Promise<void> {
      await ensureSession();
      await client.compactContext(sessionId);
    },

    async clear(): Promise<void> {
      await ensureSession();
      await client.clearContext(sessionId);
    },

    async close(): Promise<void> {
      if (initialized) {
        try {
          await client.destroySession(sessionId);
        } catch {
          // Ignore cleanup errors
        }
      }
      client.close();
    },
  };

  return chat;
}
