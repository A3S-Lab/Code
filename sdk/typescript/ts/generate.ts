/**
 * High-Level AI Functions
 *
 * Vercel AI SDK-style API for A3S Code Agent.
 * These functions automatically manage session lifecycle,
 * so you don't need to manually create/destroy sessions.
 *
 * @example
 * ```typescript
 * import { generateText, streamText, createProvider, tool } from '@a3s-lab/code';
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
 * // Streaming with tool calls
 * const { textStream } = streamText({
 *   model: openai('gpt-4o'),
 *   prompt: 'What is the weather in Tokyo?',
 *   tools: {
 *     weather: tool({
 *       description: 'Get weather for a city',
 *       parameters: { type: 'object', properties: { city: { type: 'string' } } },
 *       execute: async ({ city }) => ({ city, temp: 72 }),
 *     }),
 *   },
 *   maxSteps: 5,
 *   onStepFinish: (step) => console.log('Step:', step),
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
  ToolResult,
  GenerateChunk,
} from './client.js';
import type { OpenAIMessage } from './openai-compat.js';
import type { ModelRef } from './provider.js';
import { modelRefToLLMConfig } from './provider.js';
import type { ToolSet, ToolDefinition } from './tool.js';

// ============================================================================
// Shared Types
// ============================================================================

/** Message input — supports both A3S and OpenAI formats */
export type MessageInput = Message | OpenAIMessage;

/** Step information passed to onStepFinish callback */
export interface StepResult {
  /** Step index (0-based) */
  stepIndex: number;
  /** Text generated in this step */
  text: string;
  /** Tool calls made in this step */
  toolCalls: ToolCall[];
  /** Tool results from this step */
  toolResults: ToolResult[];
  /** Token usage for this step */
  usage?: Usage;
  /** Why this step finished */
  finishReason?: FinishReason;
}

/** Tool call event for onToolCall callback */
export interface ToolCallEvent {
  /** Tool call ID */
  toolCallId: string;
  /** Tool name */
  toolName: string;
  /** Parsed arguments */
  args: Record<string, unknown>;
}

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
  /**
   * Client-side tool definitions.
   * Server-side tools (file ops, shell, etc.) are always available.
   */
  tools?: ToolSet;
  /**
   * Maximum number of LLM generation + tool execution steps.
   * Set to > 1 to enable multi-step agent behavior.
   * @default 1
   */
  maxSteps?: number;
  /**
   * Called when each step (generation + tool execution) completes.
   * Useful for logging, progress tracking, or early termination.
   */
  onStepFinish?: (step: StepResult) => void | Promise<void>;
  /**
   * Called when the model invokes a tool.
   * For tools without an execute function, return the result here.
   * For tools with execute, this is called before execution (for logging/approval).
   */
  onToolCall?: (event: ToolCallEvent) => void | unknown | Promise<void | unknown>;
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
  /** All steps executed (for multi-step generation) */
  steps: StepResult[];
}

/** Result from streamText() */
export interface StreamTextResult {
  /** Async iterable of text chunks (content only) */
  textStream: AsyncIterable<string>;
  /** Async iterable of full event chunks */
  fullStream: AsyncIterable<GenerateChunk>;
  /** Async iterable of tool call events */
  toolStream: AsyncIterable<ToolCall>;
  /** Promise that resolves to the complete text */
  text: Promise<string>;
  /** Promise that resolves to token usage */
  usage: Promise<Usage | undefined>;
  /** Promise that resolves to finish reason */
  finishReason: Promise<FinishReason | undefined>;
  /** Promise that resolves to all steps */
  steps: Promise<StepResult[]>;
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

/** Execute a client-side tool call */
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

  // Notify callback
  if (onToolCall) {
    const callbackResult = await onToolCall(event);
    // If callback returns a value and tool has no execute, use it as result
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

  // Execute the tool
  if (toolDef.execute) {
    try {
      const result = await toolDef.execute(args, {
        toolCallId: toolCall.id,
      });
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
 * Supports multi-step tool calling via `tools` and `maxSteps`.
 * Automatically manages session lifecycle.
 *
 * @example
 * ```typescript
 * // Simple generation
 * const { text } = await generateText({
 *   model: openai('gpt-4o'),
 *   prompt: 'Summarize this file',
 *   workspace: '/project',
 * });
 *
 * // With tools and multi-step
 * const { text, steps } = await generateText({
 *   model: openai('gpt-4o'),
 *   prompt: 'What is the weather in Tokyo and Paris?',
 *   tools: { weather: weatherTool },
 *   maxSteps: 5,
 *   onStepFinish: (step) => console.log(`Step ${step.stepIndex}:`, step.text),
 * });
 * ```
 */
export async function generateText(
  options: GenerateTextOptions,
): Promise<GenerateTextResult> {
  const messages = resolveMessages(options.prompt, options.messages);
  const maxSteps = options.maxSteps ?? 1;

  return withSession(options, async (client, sessionId) => {
    const allSteps: StepResult[] = [];
    let fullText = '';
    let lastFinishReason: FinishReason = 'stop';
    const allToolCalls: ToolCall[] = [];

    for (let step = 0; step < maxSteps; step++) {
      // Only pass user messages on first step; subsequent steps use
      // the server-side conversation context (tool results are auto-appended)
      const stepMessages = step === 0 ? messages : [];
      const response = await client.generate(sessionId, stepMessages);

      const stepText = response.message?.content || '';
      fullText += stepText;
      lastFinishReason = response.finishReason;

      const stepToolCalls = response.toolCalls || [];
      const stepToolResults: ToolResult[] = [];
      allToolCalls.push(...stepToolCalls);

      // Execute client-side tools if any
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

      // Stop if no tool calls were made (model is done)
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
  });
}

/**
 * Stream text from a language model.
 *
 * Returns immediately with stream handles. Supports multi-step tool calling.
 * Session is automatically cleaned up when the stream ends.
 *
 * @example
 * ```typescript
 * const { textStream, toolStream } = streamText({
 *   model: openai('gpt-4o'),
 *   prompt: 'Explain this codebase',
 *   tools: { search: searchTool },
 *   maxSteps: 5,
 *   onToolCall: ({ toolName, args }) => console.log(`Calling ${toolName}`, args),
 * });
 *
 * for await (const chunk of textStream) {
 *   process.stdout.write(chunk);
 * }
 * ```
 */
export function streamText(options: GenerateTextOptions): StreamTextResult {
  const messages = resolveMessages(options.prompt, options.messages);
  const client = new A3sClient(options.server);
  const maxSteps = options.maxSteps ?? 1;

  // Accumulated state
  let fullText = '';
  let finalUsage: Usage | undefined;
  let finalFinishReason: FinishReason | undefined;
  const allSteps: StepResult[] = [];

  // Deferred promises
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

  // Shared buffer for tee pattern
  const chunks: GenerateChunk[] = [];
  let streamDone = false;
  let streamError: unknown = null;
  const waiters: Array<() => void> = [];

  function notifyWaiters() {
    for (const w of waiters.splice(0)) w();
  }

  // Background producer
  const produce = (async () => {
    const { sessionId } = await client.createSession({
      name: `stream-${Date.now()}`,
      workspace: options.workspace || '',
      llm: modelRefToLLMConfig(options.model),
      systemPrompt: options.system,
    });

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
          if (chunk.toolCall) {
            stepToolCalls.push(chunk.toolCall);
          }
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

              // Emit tool result as a chunk
              const resultChunk: GenerateChunk = {
                type: 'tool_result',
                sessionId,
                content: '',
                toolCall: tc,
                toolResult: result,
                metadata: {},
              };
              chunks.push(resultChunk);
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

        // Stop if no tool calls (model is done)
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
      try {
        await client.destroySession(sessionId);
      } catch {
        // Ignore cleanup errors
      }
      client.close();
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
                if (streamError) throw streamError;
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
      let iter: AsyncIterator<{ data: string; done: boolean }>;

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
