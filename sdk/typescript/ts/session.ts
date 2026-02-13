/**
 * Session — The core abstraction for A3S Code SDK.
 *
 * A Session binds a workspace and model at creation time (immutable).
 * All generation, streaming, and context management calls are methods on the session.
 *
 * Supports `using` syntax for automatic cleanup via Symbol.asyncDispose.
 *
 * @example
 * ```typescript
 * import { A3sClient, createProvider } from '@a3s-lab/code';
 *
 * const client = new A3sClient();
 * const openai = createProvider({ name: 'openai', apiKey: 'sk-xxx' });
 *
 * // Create session — model and workspace are bound here
 * const session = await client.createSession({
 *   model: openai('gpt-4o'),
 *   workspace: '/project',
 *   system: 'You are a helpful assistant',
 * });
 *
 * const { text } = await session.generateText({ prompt: 'Hello' });
 * await session.close();
 *
 * // Or with `using` for automatic cleanup:
 * await using session = await client.createSession({ ... });
 * const { text } = await session.generateText({ prompt: 'Hello' });
 * // session.close() called automatically
 * ```
 */

import type { A3sClient } from './client.js';
import type {
  Message,
  Usage,
  FinishReason,
  ToolCall,
  ToolResult,
  GenerateChunk,
  ContextUsage,
  GetMessagesResponse,
} from './client.js';
import type { OpenAIMessage } from './openai-compat.js';
import type { ModelRef } from './provider.js';
import type { ToolSet, ToolDefinition } from './tool.js';

// ============================================================================
// Types
// ============================================================================

/** Options for creating a session via client.createSession() */
export interface SessionCreateOptions {
  /** Model reference from createProvider() */
  model: ModelRef;
  /** Working directory for tool sandboxing (immutable after creation) */
  workspace?: string;
  /** System prompt */
  system?: string;
  /** Optional session ID. If omitted, the server generates one. */
  sessionId?: string;
  /** Optional initial context messages */
  initialContext?: MessageInput[];
}

/** Message input — supports both A3S and OpenAI formats */
export type MessageInput = Message | OpenAIMessage;

/** Step information passed to onStepFinish callback */
export interface StepResult {
  stepIndex: number;
  text: string;
  toolCalls: ToolCall[];
  toolResults: ToolResult[];
  usage?: Usage;
  finishReason?: FinishReason;
}

/** Tool call event for onToolCall callback */
export interface ToolCallEvent {
  toolCallId: string;
  toolName: string;
  args: Record<string, unknown>;
}

/** Options for session.generateText() */
export interface SessionGenerateTextOptions {
  /** Simple text prompt */
  prompt?: string;
  /** Full message array for multi-turn input */
  messages?: MessageInput[];
  /** Client-side tool definitions */
  tools?: ToolSet;
  /** Maximum generation + tool execution steps. @default 1 */
  maxSteps?: number;
  /** Called when each step completes */
  onStepFinish?: (step: StepResult) => void | Promise<void>;
  /** Called when the model invokes a tool */
  onToolCall?: (event: ToolCallEvent) => void | unknown | Promise<void | unknown>;
}

/** Options for session.generateObject() */
export interface SessionGenerateObjectOptions {
  /** Simple text prompt */
  prompt?: string;
  /** Full message array */
  messages?: MessageInput[];
  /** JSON schema string for structured output */
  schema: string;
}

/** Result from session.generateText() */
export interface GenerateTextResult {
  text: string;
  usage?: Usage;
  finishReason: FinishReason;
  toolCalls: ToolCall[];
  steps: StepResult[];
}

/** Result from session.streamText() */
export interface StreamTextResult {
  textStream: AsyncIterable<string>;
  fullStream: AsyncIterable<GenerateChunk>;
  toolStream: AsyncIterable<ToolCall>;
  text: Promise<string>;
  usage: Promise<Usage | undefined>;
  finishReason: Promise<FinishReason | undefined>;
  steps: Promise<StepResult[]>;
}

/** Result from session.generateObject() */
export interface GenerateObjectResult<T = unknown> {
  object: T;
  data: string;
  usage?: Usage;
}

/** Result from session.streamObject() */
export interface StreamObjectResult {
  partialStream: AsyncIterable<string>;
  object: Promise<unknown>;
  data: Promise<string>;
}

// ============================================================================
// Internal Helpers
// ============================================================================

function resolveMessages(
  prompt?: string,
  messages?: MessageInput[],
): MessageInput[] {
  if (messages && messages.length > 0) return messages;
  if (prompt) return [{ role: 'user', content: prompt }];
  throw new Error('Either "prompt" or "messages" must be provided');
}

async function executeClientTool(
  toolDef: ToolDefinition,
  toolCall: ToolCall,
  onToolCall?: (event: ToolCallEvent) => void | unknown | Promise<void | unknown>,
): Promise<ToolResult> {
  const args = toolCall.arguments ? JSON.parse(toolCall.arguments) : {};
  const event: ToolCallEvent = {
    toolCallId: toolCall.id,
    toolName: toolCall.name,
    args,
  };

  if (onToolCall) {
    const callbackResult = await onToolCall(event);
    if (callbackResult !== undefined && !toolDef.execute) {
      return {
        success: true,
        output: typeof callbackResult === 'string'
          ? callbackResult
          : JSON.stringify(callbackResult),
        error: '',
        metadata: {},
      };
    }
  }

  if (toolDef.execute) {
    try {
      const result = await toolDef.execute(args, { toolCallId: toolCall.id });
      return {
        success: true,
        output: typeof result === 'string' ? result : JSON.stringify(result),
        error: '',
        metadata: {},
      };
    } catch (err) {
      return {
        success: false,
        output: '',
        error: err instanceof Error ? err.message : String(err),
        metadata: {},
      };
    }
  }

  return {
    success: false,
    output: '',
    error: `Tool "${toolCall.name}" has no execute function and onToolCall did not return a result`,
    metadata: {},
  };
}

// ============================================================================
// Session Class
// ============================================================================

/**
 * Session — The core object for interacting with A3S Code.
 *
 * Created via `client.createSession()`. Workspace and model are immutable
 * after creation. Supports `await using` for automatic cleanup.
 */
export class Session implements AsyncDisposable {
  /** The underlying A3S client */
  private readonly _client: A3sClient;
  /** Session ID on the server */
  readonly id: string;
  /** Whether this session has been closed */
  private _closed = false;

  /** @internal — Use client.createSession() instead */
  constructor(client: A3sClient, sessionId: string) {
    this._client = client;
    this.id = sessionId;
  }

  // --------------------------------------------------------------------------
  // Text Generation
  // --------------------------------------------------------------------------

  /**
   * Generate text from the language model.
   *
   * Supports multi-step tool calling via `tools` and `maxSteps`.
   *
   * @example
   * ```typescript
   * const { text } = await session.generateText({ prompt: 'Hello' });
   *
   * // With tools
   * const { text, steps } = await session.generateText({
   *   prompt: 'What is the weather?',
   *   tools: { weather: weatherTool },
   *   maxSteps: 5,
   * });
   * ```
   */
  async generateText(options: SessionGenerateTextOptions): Promise<GenerateTextResult> {
    this._ensureOpen();
    const messages = resolveMessages(options.prompt, options.messages);
    const maxSteps = options.maxSteps ?? 1;

    const allSteps: StepResult[] = [];
    let fullText = '';
    let lastFinishReason: FinishReason = 'stop';
    const allToolCalls: ToolCall[] = [];

    for (let step = 0; step < maxSteps; step++) {
      const stepMessages = step === 0 ? messages : [];
      const response = await this._client.generate(this.id, stepMessages);

      const stepText = response.message?.content || '';
      fullText += stepText;
      lastFinishReason = response.finishReason;

      const stepToolCalls = response.toolCalls || [];
      const stepToolResults: ToolResult[] = [];
      allToolCalls.push(...stepToolCalls);

      // Execute client-side tools
      const clientToolCalls = stepToolCalls.filter(
        (tc) => options.tools && tc.name in options.tools,
      );
      for (const tc of clientToolCalls) {
        const toolDef = options.tools![tc.name];
        const result = await executeClientTool(toolDef, tc, options.onToolCall);
        stepToolResults.push(result);
        tc.result = result;
      }

      const stepResult: StepResult = {
        stepIndex: step,
        text: stepText,
        toolCalls: stepToolCalls,
        toolResults: stepToolResults,
        usage: response.usage,
        finishReason: response.finishReason,
      };
      allSteps.push(stepResult);

      if (options.onStepFinish) {
        await options.onStepFinish(stepResult);
      }

      if (stepToolCalls.length === 0 || response.finishReason !== 'tool_calls') {
        break;
      }
    }

    return {
      text: fullText,
      usage: allSteps.length > 0 ? allSteps[allSteps.length - 1].usage : undefined,
      finishReason: lastFinishReason,
      toolCalls: allToolCalls,
      steps: allSteps,
    };
  }

  // --------------------------------------------------------------------------
  // Text Streaming
  // --------------------------------------------------------------------------

  /**
   * Stream text from the language model.
   *
   * Returns immediately with stream handles. Supports multi-step tool calling.
   *
   * @example
   * ```typescript
   * const { textStream } = session.streamText({ prompt: 'Explain this' });
   * for await (const chunk of textStream) {
   *   process.stdout.write(chunk);
   * }
   * ```
   */
  streamText(options: SessionGenerateTextOptions): StreamTextResult {
    this._ensureOpen();
    const messages = resolveMessages(options.prompt, options.messages);
    const maxSteps = options.maxSteps ?? 1;

    let fullText = '';
    let finalUsage: Usage | undefined;
    let finalFinishReason: FinishReason | undefined;
    const allSteps: StepResult[] = [];

    let resolveText: (value: string) => void;
    let resolveUsage: (value: Usage | undefined) => void;
    let resolveFinishReason: (value: FinishReason | undefined) => void;
    let resolveSteps: (value: StepResult[]) => void;
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
    const stepsPromise = new Promise<StepResult[]>((res) => {
      resolveSteps = res;
    });

    const chunks: GenerateChunk[] = [];
    let streamDone = false;
    const waiters: Array<() => void> = [];

    function notifyWaiters() {
      for (const w of waiters.splice(0)) w();
    }

    const sessionId = this.id;
    const client = this._client;

    const produce = (async () => {
      try {
        for (let step = 0; step < maxSteps; step++) {
          const stepMessages = step === 0 ? messages : [];
          const stream = client.streamGenerate(sessionId, stepMessages);

          let stepText = '';
          const stepToolCalls: ToolCall[] = [];
          const stepToolResults: ToolResult[] = [];
          let stepFinishReason: FinishReason | undefined;

          for await (const chunk of stream) {
            if (chunk.content) {
              fullText += chunk.content;
              stepText += chunk.content;
            }
            if (chunk.toolCall) stepToolCalls.push(chunk.toolCall);
            if (chunk.finishReason) {
              stepFinishReason = chunk.finishReason;
              finalFinishReason = chunk.finishReason;
            }
            chunks.push(chunk);
            notifyWaiters();
          }

          // Execute client-side tools
          if (options.tools) {
            for (const tc of stepToolCalls) {
              if (tc.name in options.tools) {
                const toolDef = options.tools[tc.name];
                const result = await executeClientTool(toolDef, tc, options.onToolCall);
                stepToolResults.push(result);
                tc.result = result;

                chunks.push({
                  type: 'tool_result',
                  sessionId,
                  content: '',
                  toolCall: tc,
                  toolResult: result,
                  metadata: {},
                });
                notifyWaiters();
              }
            }
          }

          const stepResult: StepResult = {
            stepIndex: step,
            text: stepText,
            toolCalls: stepToolCalls,
            toolResults: stepToolResults,
            usage: undefined,
            finishReason: stepFinishReason,
          };
          allSteps.push(stepResult);

          if (options.onStepFinish) {
            await options.onStepFinish(stepResult);
          }

          if (stepToolCalls.length === 0 || stepFinishReason !== 'tool_calls') {
            break;
          }
        }

        resolveText!(fullText);
        resolveUsage!(finalUsage);
        resolveFinishReason!(finalFinishReason);
        resolveSteps!(allSteps);
      } catch (err) {
        rejectText!(err);
        throw err;
      } finally {
        streamDone = true;
        notifyWaiters();
      }
    })();
    produce.catch(() => {});

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

  // --------------------------------------------------------------------------
  // Structured Output
  // --------------------------------------------------------------------------

  /**
   * Generate a structured object from the language model.
   *
   * @example
   * ```typescript
   * const { object } = await session.generateObject({
   *   schema: JSON.stringify({ type: 'object', properties: { name: { type: 'string' } } }),
   *   prompt: 'Extract the name',
   * });
   * ```
   */
  async generateObject<T = unknown>(
    options: SessionGenerateObjectOptions,
  ): Promise<GenerateObjectResult<T>> {
    this._ensureOpen();
    const messages = resolveMessages(options.prompt, options.messages);
    const response = await this._client.generateStructured(
      this.id,
      messages,
      options.schema,
    );
    let parsed: T;
    try {
      parsed = JSON.parse(response.data) as T;
    } catch {
      parsed = response.data as unknown as T;
    }
    return { object: parsed, data: response.data, usage: response.usage };
  }

  /**
   * Stream a structured object from the language model.
   *
   * @example
   * ```typescript
   * const { partialStream, object } = session.streamObject({
   *   schema: '{"type":"object","properties":{"items":{"type":"array"}}}',
   *   prompt: 'List project files',
   * });
   * for await (const partial of partialStream) {
   *   console.log('partial:', partial);
   * }
   * const result = await object;
   * ```
   */
  streamObject(options: SessionGenerateObjectOptions): StreamObjectResult {
    this._ensureOpen();
    const messages = resolveMessages(options.prompt, options.messages);

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

    const sessionId = this.id;
    const client = this._client;

    const partialStream: AsyncIterable<string> = {
      [Symbol.asyncIterator]() {
        let started = false;
        let iter: AsyncIterator<{ data: string; done: boolean }>;

        return {
          async next(): Promise<IteratorResult<string>> {
            if (!started) {
              started = true;
              const stream = client.streamGenerateStructured(
                sessionId,
                messages,
                options.schema,
              );
              iter = stream[Symbol.asyncIterator]();
            }

            try {
              const result = await iter.next();
              if (result.done) {
                resolveData!(fullData);
                try {
                  resolveObject!(JSON.parse(fullData));
                } catch {
                  resolveObject!(fullData);
                }
                return { value: undefined as unknown as string, done: true };
              }
              fullData += result.value.data;
              return { value: result.value.data, done: false };
            } catch (err) {
              rejectAll!(err);
              throw err;
            }
          },
        };
      },
    };

    return { partialStream, object: objectPromise, data: dataPromise };
  }

  // --------------------------------------------------------------------------
  // Context Management
  // --------------------------------------------------------------------------

  /** Get context usage (token counts, message count) */
  async getContextUsage(): Promise<ContextUsage | undefined> {
    this._ensureOpen();
    const resp = await this._client.getContextUsage(this.id);
    return resp.usage;
  }

  /** Compact the conversation context to save tokens */
  async compactContext(): Promise<void> {
    this._ensureOpen();
    await this._client.compactContext(this.id);
  }

  /** Clear conversation history */
  async clearContext(): Promise<void> {
    this._ensureOpen();
    await this._client.clearContext(this.id);
  }

  /** Get conversation messages */
  async getMessages(limit?: number, offset?: number): Promise<GetMessagesResponse> {
    this._ensureOpen();
    return this._client.getMessages(this.id, limit, offset);
  }

  // --------------------------------------------------------------------------
  // Lifecycle
  // --------------------------------------------------------------------------

  /** Close the session and release server resources */
  async close(): Promise<void> {
    if (this._closed) return;
    this._closed = true;
    try {
      await this._client.destroySession(this.id);
    } catch {
      // Ignore cleanup errors
    }
  }

  /** Support `await using session = ...` for automatic cleanup */
  async [Symbol.asyncDispose](): Promise<void> {
    await this.close();
  }

  /** Whether this session has been closed */
  get closed(): boolean {
    return this._closed;
  }

  private _ensureOpen(): void {
    if (this._closed) {
      throw new Error(`Session ${this.id} has been closed`);
    }
  }
}
