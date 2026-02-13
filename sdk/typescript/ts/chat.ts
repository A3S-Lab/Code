/**
 * Chat — Multi-turn Conversation (Convenience Wrapper)
 *
 * A thin wrapper around Session for backward compatibility.
 * For new code, prefer using Session directly:
 *
 * ```typescript
 * const session = await client.createSession({ model: openai('gpt-4o') });
 * const { text } = await session.generateText({ prompt: 'Hello' });
 * const { text: reply } = await session.generateText({ prompt: 'Follow up' });
 * ```
 *
 * @example
 * ```typescript
 * import { createChat, createProvider, tool } from '@a3s-lab/code';
 *
 * const openai = createProvider({ name: 'openai', apiKey: 'sk-xxx' });
 *
 * const chat = createChat({
 *   model: openai('gpt-4o'),
 *   workspace: '/project',
 *   system: 'You are a helpful code assistant',
 * });
 *
 * const { text } = await chat.send('Hello');
 * const { textStream } = chat.stream('Follow up');
 * for await (const chunk of textStream) {
 *   process.stdout.write(chunk);
 * }
 * await chat.close();
 * ```
 */

import { A3sClient } from './client.js';
import type { A3sClientOptions, ContextUsage } from './client.js';
import type { ModelRef } from './provider.js';
import type { ToolSet } from './tool.js';
import { Session } from './session.js';
import type {
  MessageInput,
  StepResult,
  ToolCallEvent,
} from './session.js';

// ============================================================================
// Types
// ============================================================================

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
  /** Client-side tool definitions */
  tools?: ToolSet;
  /** Maximum steps per send/stream call. @default 1 */
  maxSteps?: number;
  /** Called when each step completes */
  onStepFinish?: (step: StepResult) => void | Promise<void>;
  /** Called when the model invokes a tool */
  onToolCall?: (event: ToolCallEvent) => void | unknown | Promise<void | unknown>;
}

/** Result from chat.send() */
export interface ChatSendResult {
  text: string;
  usage?: any;
  finishReason: any;
  toolCalls: any[];
  steps: StepResult[];
}

/** Result from chat.stream() */
export interface ChatStreamResult {
  textStream: AsyncIterable<string>;
  fullStream: AsyncIterable<any>;
  toolStream: AsyncIterable<any>;
  text: Promise<string>;
  steps: Promise<StepResult[]>;
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

/**
 * Create a multi-turn chat session (convenience wrapper).
 *
 * For new code, prefer `client.createSession()` directly.
 */
export function createChat(options: ChatOptions): Chat {
  const client = new A3sClient(options.server);
  let session: Session | null = null;
  let initPromise: Promise<void> | null = null;

  async function ensureSession(): Promise<Session> {
    if (session) return session;
    if (!initPromise) {
      initPromise = (async () => {
        session = await client.createSession({
          model: options.model,
          workspace: options.workspace,
          system: options.system,
        });
      })();
    }
    await initPromise;
    return session!;
  }

  function toMessages(input: string | MessageInput[]): MessageInput[] {
    if (typeof input === 'string') {
      return [{ role: 'user', content: input }];
    }
    return input;
  }

  const chat: Chat = {
    get sessionId() {
      return session?.id ?? '';
    },

    async send(input: string | MessageInput[]): Promise<ChatSendResult> {
      const s = await ensureSession();
      return s.generateText({
        messages: toMessages(input),
        tools: options.tools,
        maxSteps: options.maxSteps,
        onStepFinish: options.onStepFinish,
        onToolCall: options.onToolCall,
      });
    },

    stream(input: string | MessageInput[]): ChatStreamResult {
      // We need the session to exist before streaming
      // Use a deferred pattern to handle the async init
      let resolveText: (v: string) => void;
      let rejectAll: (e: unknown) => void;
      let resolveSteps: (v: StepResult[]) => void;

      const textPromise = new Promise<string>((res, rej) => {
        resolveText = res;
        rejectAll = rej;
      });
      const stepsPromise = new Promise<StepResult[]>((res) => {
        resolveSteps = res;
      });

      const chunks: any[] = [];
      let done = false;
      const waiters: Array<() => void> = [];

      function notify() {
        for (const w of waiters.splice(0)) w();
      }

      const produce = (async () => {
        try {
          const s = await ensureSession();
          const result = s.streamText({
            messages: toMessages(input),
            tools: options.tools,
            maxSteps: options.maxSteps,
            onStepFinish: options.onStepFinish,
            onToolCall: options.onToolCall,
          });

          for await (const chunk of result.fullStream) {
            chunks.push(chunk);
            notify();
          }

          resolveText!(await result.text);
          resolveSteps!(await result.steps);
        } catch (err) {
          rejectAll!(err);
        } finally {
          done = true;
          notify();
        }
      })();
      produce.catch(() => {});

      function iterate<T>(
        transform: (c: any) => T | null,
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
        toolStream: iterate((c) => (c.toolCall ? c.toolCall : null)),
        text: textPromise,
        steps: stepsPromise,
      };
    },

    async getUsage(): Promise<ContextUsage | undefined> {
      const s = await ensureSession();
      return s.getContextUsage();
    },

    async compact(): Promise<void> {
      const s = await ensureSession();
      await s.compactContext();
    },

    async clear(): Promise<void> {
      const s = await ensureSession();
      await s.clearContext();
    },

    async close(): Promise<void> {
      if (session) {
        await session.close();
      }
      client.close();
    },
  };

  return chat;
}
