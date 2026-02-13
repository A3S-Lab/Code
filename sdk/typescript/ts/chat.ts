/**
 * Chat — Multi-turn Conversation
 *
 * Manages a persistent session for multi-turn conversations.
 * Supports tool calling, multi-step agents, and event callbacks.
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
 *   tools: {
 *     search: tool({
 *       description: 'Search the codebase',
 *       parameters: { type: 'object', properties: { query: { type: 'string' } } },
 *       execute: async ({ query }) => ({ results: [`Found: ${query}`] }),
 *     }),
 *   },
 *   maxSteps: 5,
 *   onToolCall: ({ toolName, args }) => console.log(`Tool: ${toolName}`, args),
 *   onStepFinish: (step) => console.log(`Step ${step.stepIndex} done`),
 * });
 *
 * const { text } = await chat.send('Find all TODO comments');
 * console.log(text);
 *
 * const { textStream, toolStream } = chat.stream('Now fix them');
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
  ToolResult,
  GenerateChunk,
  ContextUsage,
} from './client.js';
import type { OpenAIMessage } from './openai-compat.js';
import type { ModelRef } from './provider.js';
import { modelRefToLLMConfig } from './provider.js';
import type { ToolSet, ToolDefinition } from './tool.js';

// ============================================================================
// Types
// ============================================================================

type MessageInput = Message | OpenAIMessage;

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
  /**
   * Maximum steps per send/stream call.
   * @default 1
   */
  maxSteps?: number;
  /** Called when each step completes */
  onStepFinish?: (step: StepResult) => void | Promise<void>;
  /** Called when the model invokes a tool */
  onToolCall?: (event: ToolCallEvent) => void | unknown | Promise<void | unknown>;
}

/** Result from chat.send() */
export interface ChatSendResult {
  text: string;
  usage?: Usage;
  finishReason: FinishReason;
  toolCalls: ToolCall[];
  steps: StepResult[];
}

/** Result from chat.stream() */
export interface ChatStreamResult {
  /** Text content chunks */
  textStream: AsyncIterable<string>;
  /** All event chunks */
  fullStream: AsyncIterable<GenerateChunk>;
  /** Tool call events */
  toolStream: AsyncIterable<ToolCall>;
  /** Complete text when done */
  text: Promise<string>;
  /** All steps when done */
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
// Internal Helpers
// ============================================================================

function toMessages(input: string | MessageInput[]): MessageInput[] {
  if (typeof input === 'string') {
    return [{ role: 'user', content: input }];
  }
  return input;
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
    error: `Tool "${toolCall.name}" has no execute function`,
    metadata: {},
  };
}

// ============================================================================
// Implementation
// ============================================================================

/**
 * Create a multi-turn chat session.
 *
 * The session is created lazily on first send/stream call.
 * Supports tool calling and multi-step agent behavior.
 * Call `close()` when done to clean up resources.
 *
 * @example
 * ```typescript
 * const chat = createChat({
 *   model: openai('gpt-4o'),
 *   workspace: '/project',
 *   system: 'You are a code reviewer',
 *   tools: { lint: lintTool },
 *   maxSteps: 3,
 * });
 *
 * const { text, steps } = await chat.send('Review this PR');
 * console.log(text);
 * console.log(`Completed in ${steps.length} steps`);
 *
 * await chat.close();
 * ```
 */
export function createChat(options: ChatOptions): Chat {
  const client = new A3sClient(options.server);
  const maxSteps = options.maxSteps ?? 1;
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

      const allSteps: StepResult[] = [];
      let fullText = '';
      let lastFinishReason: FinishReason = 'stop';
      const allToolCalls: ToolCall[] = [];

      for (let step = 0; step < maxSteps; step++) {
        const stepMessages = step === 0 ? messages : [];
        const response = await client.generate(sessionId, stepMessages);

        const stepText = response.message?.content || '';
        fullText += stepText;
        lastFinishReason = response.finishReason;

        const stepToolCalls = response.toolCalls || [];
        const stepToolResults: ToolResult[] = [];
        allToolCalls.push(...stepToolCalls);

        // Execute client-side tools
        if (options.tools) {
          for (const tc of stepToolCalls) {
            if (tc.name in options.tools) {
              const toolDef = options.tools[tc.name];
              const result = await executeClientTool(toolDef, tc, options.onToolCall);
              stepToolResults.push(result);
              tc.result = result;
            }
          }
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
    },

    stream(input: string | MessageInput[]): ChatStreamResult {
      const messages = toMessages(input);

      let fullText = '';
      const allSteps: StepResult[] = [];

      let resolveText: (value: string) => void;
      let rejectText: (reason: unknown) => void;
      let resolveSteps: (value: StepResult[]) => void;
      const textPromise = new Promise<string>((res, rej) => {
        resolveText = res;
        rejectText = rej;
      });
      const stepsPromise = new Promise<StepResult[]>((res) => {
        resolveSteps = res;
      });

      // Shared buffer
      const chunks: GenerateChunk[] = [];
      let done = false;
      let error: unknown = null;
      const waiters: Array<() => void> = [];

      function notify() {
        for (const w of waiters.splice(0)) w();
      }

      // Background producer with multi-step support
      const produce = (async () => {
        await ensureSession();
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
              if (chunk.finishReason) stepFinishReason = chunk.finishReason;
              chunks.push(chunk);
              notify();
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
                  notify();
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
          resolveSteps!(allSteps);
        } catch (err) {
          error = err;
          rejectText!(err);
          throw err;
        } finally {
          done = true;
          notify();
        }
      })();
      produce.catch(() => {});

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
        toolStream: iterate((c) => (c.toolCall ? c.toolCall : null)),
        text: textPromise,
        steps: stepsPromise,
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
