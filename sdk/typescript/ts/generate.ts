/**
 * High-Level AI Functions (Convenience Wrappers)
 *
 * Vercel AI SDK-style standalone functions that auto-manage session lifecycle.
 * These create a temporary session, run the operation, and clean up.
 *
 * For multi-turn or long-lived usage, prefer creating a Session directly:
 * ```typescript
 * const session = await client.createSession({ model: openai('gpt-4o') });
 * const { text } = await session.generateText({ prompt: 'Hello' });
 * ```
 *
 * @example
 * ```typescript
 * import { generateText, streamText, createProvider } from '@a3s-lab/code';
 *
 * const openai = createProvider({ name: 'openai', apiKey: 'sk-xxx' });
 *
 * // One-shot generation (auto session)
 * const { text } = await generateText({
 *   model: openai('gpt-4o'),
 *   prompt: 'Explain this codebase',
 *   workspace: '/project',
 * });
 * ```
 */

import { A3sClient } from './client.js';
import type { A3sClientOptions } from './client.js';
import type { ModelRef } from './provider.js';
import type { ToolSet } from './tool.js';
import { Session } from './session.js';
import type {
  MessageInput,
  StepResult,
  ToolCallEvent,
  GenerateTextResult,
  StreamTextResult,
  GenerateObjectResult,
  StreamObjectResult,
} from './session.js';

// Re-export result types for backward compatibility
export type {
  MessageInput,
  StepResult,
  ToolCallEvent,
  GenerateTextResult,
  StreamTextResult,
  GenerateObjectResult,
  StreamObjectResult,
} from './session.js';

// ============================================================================
// Option Types
// ============================================================================

/** Base options shared by all standalone generation functions */
export interface BaseGenerateOptions {
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
  /** Maximum generation + tool execution steps. @default 1 */
  maxSteps?: number;
  /** Called when each step completes */
  onStepFinish?: (step: StepResult) => void | Promise<void>;
  /** Called when the model invokes a tool */
  onToolCall?: (event: ToolCallEvent) => void | unknown | Promise<void | unknown>;
}

/** Options for text generation */
export interface GenerateTextOptions extends BaseGenerateOptions {
  /** Simple text prompt */
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
// Standalone Functions (auto session lifecycle)
// ============================================================================

/**
 * Generate text from a language model (auto session).
 *
 * Creates a temporary session, generates text, and cleans up.
 * For multi-turn usage, create a Session directly instead.
 *
 * @example
 * ```typescript
 * const { text } = await generateText({
 *   model: openai('gpt-4o'),
 *   prompt: 'Summarize this file',
 *   workspace: '/project',
 * });
 * ```
 */
export async function generateText(
  options: GenerateTextOptions,
): Promise<GenerateTextResult> {
  const client = new A3sClient(options.server);
  const session = await client.createSession({
    model: options.model,
    workspace: options.workspace,
    system: options.system,
  });

  try {
    return await session.generateText({
      prompt: options.prompt,
      messages: options.messages,
      tools: options.tools,
      maxSteps: options.maxSteps,
      onStepFinish: options.onStepFinish,
      onToolCall: options.onToolCall,
    });
  } finally {
    await session.close();
    client.close();
  }
}

/**
 * Stream text from a language model (auto session).
 *
 * Returns immediately with stream handles. Session is cleaned up when stream ends.
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
  const client = new A3sClient(options.server);
  let session: Session | null = null;

  // We need to create the session first, then delegate to session.streamText()
  // Since streamText returns synchronously, we wrap with a deferred pattern
  let resolveText: (value: string) => void;
  let resolveUsage: (value: any) => void;
  let resolveFinishReason: (value: any) => void;
  let resolveSteps: (value: StepResult[]) => void;
  let rejectAll: (reason: unknown) => void;

  const textPromise = new Promise<string>((res, rej) => {
    resolveText = res;
    rejectAll = rej;
  });
  const usagePromise = new Promise<any>((res) => { resolveUsage = res; });
  const finishReasonPromise = new Promise<any>((res) => { resolveFinishReason = res; });
  const stepsPromise = new Promise<StepResult[]>((res) => { resolveSteps = res; });

  const chunks: any[] = [];
  let streamDone = false;
  const waiters: Array<() => void> = [];

  function notify() {
    for (const w of waiters.splice(0)) w();
  }

  const produce = (async () => {
    try {
      session = await client.createSession({
        model: options.model,
        workspace: options.workspace,
        system: options.system,
      }) as Session;

      const result = session.streamText({
        prompt: options.prompt,
        messages: options.messages,
        tools: options.tools,
        maxSteps: options.maxSteps,
        onStepFinish: options.onStepFinish,
        onToolCall: options.onToolCall,
      });

      // Pipe fullStream into our chunks buffer
      for await (const chunk of result.fullStream) {
        chunks.push(chunk);
        notify();
      }

      resolveText!(await result.text);
      resolveUsage!(await result.usage);
      resolveFinishReason!(await result.finishReason);
      resolveSteps!(await result.steps);
    } catch (err) {
      rejectAll!(err);
    } finally {
      streamDone = true;
      notify();
      if (session) await session.close();
      client.close();
    }
  })();
  produce.catch(() => {});

  function createIterator<T>(
    transform: (chunk: any) => T | null,
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
    textStream: createIterator((c) => (c.content ? c.content : null)),
    fullStream: createIterator((c) => c),
    toolStream: createIterator((c) => (c.toolCall ? c.toolCall : null)),
    text: textPromise,
    usage: usagePromise,
    finishReason: finishReasonPromise,
    steps: stepsPromise,
  };
}

/**
 * Generate a structured object from a language model (auto session).
 *
 * @example
 * ```typescript
 * const { object } = await generateObject({
 *   model: openai('gpt-4o'),
 *   schema: JSON.stringify({ type: 'object', properties: { summary: { type: 'string' } } }),
 *   prompt: 'Summarize this project',
 * });
 * ```
 */
export async function generateObject<T = unknown>(
  options: GenerateObjectOptions,
): Promise<GenerateObjectResult<T>> {
  const client = new A3sClient(options.server);
  const session = await client.createSession({
    model: options.model,
    workspace: options.workspace,
    system: options.system,
  });

  try {
    return await session.generateObject<T>({
      prompt: options.prompt,
      messages: options.messages,
      schema: options.schema,
    });
  } finally {
    await session.close();
    client.close();
  }
}

/**
 * Stream a structured object from a language model (auto session).
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
  const client = new A3sClient(options.server);
  let session: Session | null = null;

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
      let innerIter: AsyncIterator<string>;

      return {
        async next(): Promise<IteratorResult<string>> {
          if (!started) {
            started = true;
            session = await client.createSession({
              model: options.model,
              workspace: options.workspace,
              system: options.system,
            }) as Session;
            const result = session.streamObject({
              prompt: options.prompt,
              messages: options.messages,
              schema: options.schema,
            });
            innerIter = result.partialStream[Symbol.asyncIterator]();

            // Wire up the promises
            result.object.then(
              (v: unknown) => resolveObject!(v),
              (e: unknown) => rejectAll!(e),
            );
            result.data.then((v: string) => resolveData!(v));
          }

          try {
            const r = await innerIter.next();
            if (r.done) {
              if (session) await session.close();
              client.close();
              return { value: undefined as unknown as string, done: true };
            }
            return r;
          } catch (err) {
            if (session) await session.close();
            client.close();
            throw err;
          }
        },
      };
    },
  };

  return { partialStream, object: objectPromise, data: dataPromise };
}
