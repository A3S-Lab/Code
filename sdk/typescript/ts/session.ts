/**
 * Session — The core abstraction for A3S Code SDK.
 *
 * A Session binds a workspace and model at creation time (immutable).
 * All agentic, generation, streaming, and context management calls are methods on the session.
 *
 * Every `session.send()` can trigger the AgenticLoop — when the model calls tools,
 * the session automatically enters the loop (generate → tool call → execute → reflect → repeat).
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
 * await using session = await client.createSession({
 *   model: openai('gpt-4o'),
 *   workspace: '/project',
 *   system: 'You are a senior software engineer.',
 * });
 *
 * // Simple question — model answers directly
 * const { text } = await session.send('What is TypeScript?');
 *
 * // Complex task — auto-enters AgenticLoop
 * const { text, steps, toolCalls } = await session.send(
 *   'Refactor the auth module to use JWT',
 * );
 *
 * // Streaming with real-time events
 * const { eventStream } = session.sendStream('Fix all TODOs in src/');
 * for await (const event of eventStream) {
 *   if (event.type === 'text') process.stdout.write(event.content);
 * }
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
  SessionLaneType,
  LaneHandlerConfig as ProtoLaneHandlerConfig,
  ExternalTask,
  ConfirmationPolicy as ProtoConfirmationPolicy,
  PermissionPolicy as ProtoPermissionPolicy,
  PermissionRule,
  Skill,
  SearchSkillsResponse,
  SkillSearchResult,
  InstallSkillResponse,
  ToolStats,
  GetCostSummaryResponse,
  AgentEvent,
  AgenticGenerateResponse,
  AgenticGenerateEvent as ProtoAgenticEvent,
  DelegateResponse,
  GetQueueStatsResponse,
  ProtoLaneStats,
  MemoryItem,
  MemoryStats,
  StoreMemoryResponse,
  SearchMemoriesResponse,
  ClearMemoriesResponse,
  DeleteMemoryResponse,
  SummarizeMemoriesResponse,
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
  /** Skills to load (directories, names, or inline definitions) */
  skills?: Array<string | SkillDefinition>;
  /** HITL confirmation policy */
  confirmation?: ConfirmationConfig;
  /** Tool permission policy */
  permissions?: PermissionConfig;
  /** Lane handler overrides */
  lanes?: Partial<Record<LaneName, LaneConfig>>;
  /** Enable auto-compaction */
  autoCompact?: boolean;
  /** Auto-compact threshold (0.0-1.0) */
  autoCompactThreshold?: number;
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
// Agentic Types (send / sendStream / delegate / skills / HITL / lanes)
// ============================================================================

/** Inline skill definition */
export interface SkillDefinition {
  name: string;
  description: string;
  content: string;
  allowedTools?: string[];
  disableModelInvocation?: boolean;
}

/** Skill info returned by listSkills() */
export interface SkillInfo {
  name: string;
  description: string;
  loaded: boolean;
  source: 'project' | 'user' | 'builtin' | 'inline';
}

/** Custom agent definition for registerAgent() */
export interface AgentDefinition {
  name: string;
  description: string;
  system?: string;
  permissions?: { allow?: string[]; deny?: string[] };
  maxSteps?: number;
  model?: ModelRef;
}

/** Agent info returned by listAgents() */
export interface AgentInfo {
  name: string;
  description: string;
  mode: 'subagent';
}

/** Friendly lane name */
export type LaneName = 'control' | 'query' | 'execute' | 'generate';

/** Friendly lane handler config */
export interface LaneConfig {
  mode: 'internal' | 'external' | 'hybrid';
  timeout?: number;
  timeoutAction?: 'reject' | 'fallback-internal' | 'auto-retry';
  maxRetries?: number;
}

/** Friendly confirmation policy */
export interface ConfirmationConfig {
  requireConfirmation?: string[];
  autoApprove?: string[];
  timeout?: number;
  timeoutAction?: 'auto-approve' | 'reject';
}

/** Friendly permission policy */
export interface PermissionConfig {
  defaultAction?: 'allow' | 'deny' | 'ask';
  rules?: Array<{ tool: string; pattern?: string; action: 'allow' | 'deny' | 'ask' }>;
}

/** Confirmation request passed to onConfirmation callback */
export interface ConfirmationRequest {
  confirmationId: string;
  toolName: string;
  args: Record<string, unknown>;
  timeout: number;
}

/** Agent loop event types */
export type AgentLoopEvent =
  | { type: 'text'; content: string }
  | { type: 'tool_call'; toolName: string; args: Record<string, unknown>; toolCallId: string }
  | { type: 'tool_result'; toolCallId: string; output: string; success: boolean }
  | { type: 'step_finish'; stepIndex: number; text: string; toolCalls: ToolCall[] }
  | { type: 'plan'; steps: Array<{ description: string }> }
  | { type: 'reflection'; confidence: number; shouldRetry: boolean; insight: string }
  | { type: 'confirmation_required'; confirmationId: string; toolName: string; args: Record<string, unknown>; timeout: number }
  | { type: 'confirmation_received'; approved: boolean }
  | { type: 'subagent_start'; agentName: string; task: string; sessionId: string }
  | { type: 'subagent_progress'; agentName: string; text: string }
  | { type: 'subagent_end'; agentName: string; result: string }
  | { type: 'context_compact'; beforeTokens: number; afterTokens: number }
  | { type: 'external_task_pending'; task: ExternalTask }
  | { type: 'external_task_completed'; task: ExternalTask }
  | { type: 'error'; message: string; recoverable: boolean }
  | { type: 'done'; finishReason: string };

/** Options for session.send() / session.sendStream() */
export interface SendOptions {
  /** Maximum agent loop iterations. @default 50 */
  maxSteps?: number;
  /** Execution strategy. @default 'auto' */
  strategy?: 'direct' | 'planned' | 'iterative' | 'parallel' | 'auto';
  /** Enable reflection after tool failures. @default true */
  reflection?: boolean;
  /** Enable planning before execution. @default 'auto' */
  planning?: boolean | 'auto';
  /** Additional client-side tools (on top of built-in server tools) */
  tools?: ToolSet;
  /** Abort signal for cancellation */
  signal?: AbortSignal;
  /** Called when each step completes */
  onStepFinish?: (step: StepResult) => void | Promise<void>;
  /** Called when the model invokes a tool */
  onToolCall?: (event: ToolCallEvent) => void | unknown | Promise<void | unknown>;
  /** Called for all agent loop events */
  onEvent?: (event: AgentLoopEvent) => void | Promise<void>;
  /** Called when HITL confirmation is needed. Return true to approve. */
  onConfirmation?: (request: ConfirmationRequest) => boolean | Promise<boolean>;
}

/** Result from session.send() */
export interface SendResult {
  text: string;
  steps: StepResult[];
  toolCalls: ToolCall[];
  usage: Usage;
  finishReason: 'stop' | 'max_steps' | 'cancelled' | 'error';
}

/** Result from session.sendStream() */
export interface SendStreamResult {
  eventStream: AsyncIterable<AgentLoopEvent>;
  result: Promise<SendResult>;
}

/** Per-lane queue statistics */
export interface LaneStats {
  pending: number;
  active: number;
  external: number;
  completed: number;
  failed: number;
}

/** Queue statistics across all lanes */
export interface QueueStats {
  control: LaneStats;
  query: LaneStats;
  execute: LaneStats;
  generate: LaneStats;
  deadLetters: number;
}

/** External task completion result */
export interface ExternalTaskResult {
  success: boolean;
  output?: string;
  error?: string;
  metadata?: Record<string, string>;
}

/** Session statistics */
export interface SessionStats {
  totalTokens: number;
  promptTokens: number;
  completionTokens: number;
  totalCost: number;
  messageCount: number;
  toolCallCount: number;
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

/** Map friendly lane name to proto enum */
function laneNameToProto(lane: LaneName): SessionLaneType {
  const map: Record<LaneName, SessionLaneType> = {
    control: 'SESSION_LANE_CONTROL',
    query: 'SESSION_LANE_QUERY',
    execute: 'SESSION_LANE_EXECUTE',
    generate: 'SESSION_LANE_GENERATE',
  };
  return map[lane];
}

/** Map friendly handler mode to proto enum */
function handlerModeToProto(mode: LaneConfig['mode']): string {
  const map = {
    internal: 'TASK_HANDLER_MODE_INTERNAL',
    external: 'TASK_HANDLER_MODE_EXTERNAL',
    hybrid: 'TASK_HANDLER_MODE_HYBRID',
  };
  return map[mode];
}

/** Map friendly timeout action to proto enum */
function timeoutActionToProto(action?: string): string {
  if (action === 'auto-approve') return 'TIMEOUT_ACTION_AUTO_APPROVE';
  return 'TIMEOUT_ACTION_REJECT';
}

/** Map friendly permission action to proto enum */
function permissionActionToProto(action: string): string {
  const map: Record<string, string> = {
    allow: 'PERMISSION_DECISION_ALLOW',
    deny: 'PERMISSION_DECISION_DENY',
    ask: 'PERMISSION_DECISION_ASK',
  };
  return map[action] || 'PERMISSION_DECISION_ALLOW';
}

/** Map a proto AgenticGenerateEvent to an SDK AgentLoopEvent */
function mapProtoAgenticEvent(event: ProtoAgenticEvent): AgentLoopEvent | null {
  switch (event.type) {
    case 'text':
      return event.content ? { type: 'text', content: event.content } : null;
    case 'tool_call':
      if (!event.toolCall) return null;
      return {
        type: 'tool_call',
        toolName: event.toolCall.name,
        args: event.toolCall.arguments ? JSON.parse(event.toolCall.arguments) : {},
        toolCallId: event.toolCall.id,
      };
    case 'tool_result':
      return event.toolResult
        ? {
            type: 'tool_result',
            toolCallId: event.toolCallId || '',
            output: event.toolResult.output,
            success: event.toolResult.success,
          }
        : null;
    case 'step_finish':
      return {
        type: 'step_finish',
        stepIndex: event.stepIndex ?? 0,
        text: event.stepText || '',
        toolCalls: [],
      };
    case 'error':
      return {
        type: 'error',
        message: event.errorMessage || 'Unknown error',
        recoverable: event.recoverable ?? false,
      };
    case 'done':
      return { type: 'done', finishReason: event.finishReason || 'stop' };
    case 'confirmation_required':
      return {
        type: 'confirmation_required',
        confirmationId: event.confirmationId || '',
        toolName: event.toolName || '',
        args: event.toolArgs ? JSON.parse(event.toolArgs) : {},
        timeout: event.timeoutMs ?? 30000,
      };
    case 'confirmation_received':
      return { type: 'confirmation_received', approved: event.approved ?? false };
    case 'subagent_start':
      return {
        type: 'subagent_start',
        agentName: event.agentName || '',
        task: event.agentTask || '',
        sessionId: event.agentSessionId || '',
      };
    case 'subagent_end':
      return {
        type: 'subagent_end',
        agentName: event.agentName || '',
        result: event.agentResult || '',
      };
    case 'context_compact':
      return {
        type: 'context_compact',
        beforeTokens: event.beforeTokens ?? 0,
        afterTokens: event.afterTokens ?? 0,
      };
    default:
      return null;
  }
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
  /** Custom registered agents */
  private readonly _customAgents: AgentDefinition[] = [];

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
  // AgenticLoop: send() / sendStream()
  // --------------------------------------------------------------------------

  /**
   * Send a message to the agent. Automatically enters the server-side
   * AgenticLoop when the model calls tools (plan → execute → reflect → repeat).
   *
   * The entire loop runs on the server — built-in tools, planning, reflection,
   * and HITL are all handled server-side. Client-side tools (if provided) are
   * used as a fallback for tools not available on the server.
   *
   * @example
   * ```typescript
   * // Simple question — no tools needed
   * const { text } = await session.send('What is TypeScript?');
   *
   * // Complex task — server runs full AgenticLoop
   * const { text, steps, toolCalls } = await session.send(
   *   'Refactor the auth module to use JWT',
   *   { maxSteps: 50 },
   * );
   * ```
   */
  async send(prompt: string, options: SendOptions = {}): Promise<SendResult> {
    this._ensureOpen();

    const strategyMap: Record<string, string> = {
      auto: 'AGENTIC_STRATEGY_AUTO',
      direct: 'AGENTIC_STRATEGY_DIRECT',
      planned: 'AGENTIC_STRATEGY_PLANNED',
      iterative: 'AGENTIC_STRATEGY_ITERATIVE',
      parallel: 'AGENTIC_STRATEGY_PARALLEL',
    };

    const resp: AgenticGenerateResponse = await this._client.agenticGenerate(
      this.id,
      prompt,
      {
        strategy: strategyMap[options.strategy || 'auto'],
        maxSteps: options.maxSteps ?? 50,
        reflection: options.reflection ?? true,
        planning: options.planning === true || options.planning === 'auto',
      },
    );

    // Map server response to SDK types
    const steps: StepResult[] = (resp.steps || []).map((s) => ({
      stepIndex: s.stepIndex,
      text: s.text,
      toolCalls: s.toolCalls || [],
      toolResults: s.toolResults || [],
      usage: s.usage,
      finishReason: s.finishReason as FinishReason | undefined,
    }));

    // Emit events for callbacks
    if (options.onEvent) {
      await options.onEvent({ type: 'done', finishReason: resp.finishReason });
    }

    return {
      text: resp.text,
      steps,
      toolCalls: resp.toolCalls || [],
      usage: resp.usage || { promptTokens: 0, completionTokens: 0, totalTokens: 0 },
      finishReason: (resp.finishReason as SendResult['finishReason']) || 'stop',
    };
  }

  /**
   * Stream a message to the agent. Returns an event stream for real-time UI
   * and a promise for the final result.
   *
   * @example
   * ```typescript
   * const { eventStream, result } = session.sendStream('Fix all TODOs in src/');
   * for await (const event of eventStream) {
   *   if (event.type === 'text') process.stdout.write(event.content);
   *   if (event.type === 'tool_call') console.log(`🔧 ${event.toolName}`);
   * }
   * const final = await result;
   * ```
   */
  sendStream(prompt: string, options: SendOptions = {}): SendStreamResult {
    this._ensureOpen();
    const maxSteps = options.maxSteps ?? 50;

    const events: AgentLoopEvent[] = [];
    let streamDone = false;
    const waiters: Array<() => void> = [];

    function pushEvent(event: AgentLoopEvent) {
      events.push(event);
      for (const w of waiters.splice(0)) w();
    }

    const resultPromise = (async (): Promise<SendResult> => {
      const allSteps: StepResult[] = [];
      const allToolCalls: ToolCall[] = [];
      let fullText = '';
      let totalUsage: Usage = { promptTokens: 0, completionTokens: 0, totalTokens: 0 };

      try {
        for (let step = 0; step < maxSteps; step++) {
          if (options.signal?.aborted) {
            pushEvent({ type: 'done', finishReason: 'cancelled' });
            return { text: fullText, steps: allSteps, toolCalls: allToolCalls, usage: totalUsage, finishReason: 'cancelled' };
          }

          const messages: MessageInput[] = step === 0 ? [{ role: 'user', content: prompt }] : [];
          const stream = this._client.streamGenerate(this.id, messages);

          let stepText = '';
          const stepToolCalls: ToolCall[] = [];
          let stepFinishReason: FinishReason | undefined;

          for await (const chunk of stream) {
            if (chunk.content) {
              fullText += chunk.content;
              stepText += chunk.content;
              pushEvent({ type: 'text', content: chunk.content });
            }
            if (chunk.toolCall) {
              stepToolCalls.push(chunk.toolCall);
              const args = chunk.toolCall.arguments ? JSON.parse(chunk.toolCall.arguments) : {};
              pushEvent({ type: 'tool_call', toolName: chunk.toolCall.name, args, toolCallId: chunk.toolCall.id });
            }
            if (chunk.finishReason) {
              stepFinishReason = chunk.finishReason;
            }
          }

          allToolCalls.push(...stepToolCalls);

          // Execute client-side tools
          const stepToolResults: ToolResult[] = [];
          if (options.tools) {
            for (const tc of stepToolCalls) {
              if (tc.name in options.tools) {
                const toolDef = options.tools[tc.name];
                const result = await executeClientTool(toolDef, tc, options.onToolCall);
                stepToolResults.push(result);
                tc.result = result;
                pushEvent({ type: 'tool_result', toolCallId: tc.id, output: result.output, success: result.success });
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
          pushEvent({ type: 'step_finish', stepIndex: step, text: stepText, toolCalls: stepToolCalls });

          if (options.onStepFinish) {
            await options.onStepFinish(stepResult);
          }

          if (stepToolCalls.length === 0 || stepFinishReason !== 'tool_calls') {
            break;
          }
        }

        const finishReason = allSteps.length >= maxSteps ? 'max_steps' : 'stop';
        pushEvent({ type: 'done', finishReason });
        return { text: fullText, steps: allSteps, toolCalls: allToolCalls, usage: totalUsage, finishReason };
      } finally {
        streamDone = true;
        for (const w of waiters.splice(0)) w();
      }
    })();

    // Suppress unhandled rejection on the background promise
    resultPromise.catch(() => {});

    const eventStream: AsyncIterable<AgentLoopEvent> = {
      [Symbol.asyncIterator]() {
        let index = 0;
        return {
          async next(): Promise<IteratorResult<AgentLoopEvent>> {
            while (true) {
              if (index < events.length) {
                return { value: events[index++], done: false };
              }
              if (streamDone) {
                return { value: undefined as unknown as AgentLoopEvent, done: true };
              }
              await new Promise<void>((r) => waiters.push(r));
            }
          },
        };
      },
    };

    return { eventStream, result: resultPromise };
  }

  // --------------------------------------------------------------------------
  // Delegation (Subagents)
  // --------------------------------------------------------------------------

  /**
   * Delegate a task to a built-in or custom agent.
   *
   * Uses server-side delegation — the server creates a child session with
   * the agent's restricted permissions and executes the task.
   *
   * @example
   * ```typescript
   * const result = await session.delegate('explore', 'Find all API endpoints');
   * console.log(result.text);
   * ```
   */
  async delegate(agent: string, task: string, options?: { maxSteps?: number; allowedTools?: string[] }): Promise<SendResult> {
    this._ensureOpen();

    const resp: DelegateResponse = await this._client.delegate(
      this.id,
      agent,
      task,
      {
        maxSteps: options?.maxSteps ?? 50,
        allowedTools: options?.allowedTools,
      },
    );

    const steps: StepResult[] = (resp.steps || []).map((s) => ({
      stepIndex: s.stepIndex,
      text: s.text,
      toolCalls: s.toolCalls || [],
      toolResults: s.toolResults || [],
      usage: s.usage,
      finishReason: s.finishReason as FinishReason | undefined,
    }));

    return {
      text: resp.text,
      steps,
      toolCalls: resp.toolCalls || [],
      usage: resp.usage || { promptTokens: 0, completionTokens: 0, totalTokens: 0 },
      finishReason: (resp.finishReason as SendResult['finishReason']) || 'stop',
    };
  }

  /**
   * Delegate a task to a subagent with streaming.
   *
   * Uses server-side streaming delegation for real-time event updates.
   */
  delegateStream(agent: string, task: string, options?: { maxSteps?: number; allowedTools?: string[] }): SendStreamResult {
    this._ensureOpen();

    const events: AgentLoopEvent[] = [];
    let streamDone = false;
    const waiters: Array<() => void> = [];

    function pushEvent(event: AgentLoopEvent) {
      events.push(event);
      for (const w of waiters.splice(0)) w();
    }

    const resultPromise = (async (): Promise<SendResult> => {
      const allSteps: StepResult[] = [];
      const allToolCalls: ToolCall[] = [];
      let fullText = '';
      let totalUsage: Usage = { promptTokens: 0, completionTokens: 0, totalTokens: 0 };

      try {
        const stream = this._client.streamDelegate(
          this.id,
          agent,
          task,
          {
            maxSteps: options?.maxSteps ?? 50,
            allowedTools: options?.allowedTools,
          },
        );

        for await (const event of stream) {
          const mapped = mapProtoAgenticEvent(event);
          if (mapped) pushEvent(mapped);

          if (event.content) fullText += event.content;
          if (event.toolCall) allToolCalls.push(event.toolCall);
          if (event.usage) totalUsage = event.usage;
        }

        const finishReason = 'stop';
        pushEvent({ type: 'done', finishReason });
        return { text: fullText, steps: allSteps, toolCalls: allToolCalls, usage: totalUsage, finishReason };
      } finally {
        streamDone = true;
        for (const w of waiters.splice(0)) w();
      }
    })();

    resultPromise.catch(() => {});

    const eventStream: AsyncIterable<AgentLoopEvent> = {
      [Symbol.asyncIterator]() {
        let index = 0;
        return {
          async next(): Promise<IteratorResult<AgentLoopEvent>> {
            while (true) {
              if (index < events.length) {
                return { value: events[index++], done: false };
              }
              if (streamDone) {
                return { value: undefined as unknown as AgentLoopEvent, done: true };
              }
              await new Promise<void>((r) => waiters.push(r));
            }
          },
        };
      },
    };

    return { eventStream, result: resultPromise };
  }

  // --------------------------------------------------------------------------
  // Lane Queue Management
  // --------------------------------------------------------------------------

  /** Set lane execution mode (internal/external/hybrid) */
  async setLaneHandler(lane: LaneName, config: LaneConfig): Promise<void> {
    this._ensureOpen();
    const protoLane = laneNameToProto(lane);
    const protoConfig: ProtoLaneHandlerConfig = {
      mode: handlerModeToProto(config.mode) as ProtoLaneHandlerConfig['mode'],
      timeoutMs: config.timeout ?? 60_000,
    };
    await this._client.setLaneHandler(this.id, protoLane, protoConfig);
  }

  /** Get lane handler configuration */
  async getLaneHandler(lane: LaneName): Promise<LaneConfig | undefined> {
    this._ensureOpen();
    const resp = await this._client.getLaneHandler(this.id, laneNameToProto(lane));
    if (!resp.config) return undefined;
    const modeMap: Record<string, LaneConfig['mode']> = {
      TASK_HANDLER_MODE_INTERNAL: 'internal',
      TASK_HANDLER_MODE_EXTERNAL: 'external',
      TASK_HANDLER_MODE_HYBRID: 'hybrid',
    };
    return {
      mode: modeMap[resp.config.mode] || 'internal',
      timeout: resp.config.timeoutMs,
    };
  }

  /** List tasks waiting for external processing */
  async listPendingTasks(): Promise<ExternalTask[]> {
    this._ensureOpen();
    const resp = await this._client.listPendingExternalTasks(this.id);
    return resp.tasks;
  }

  /** Complete an external task */
  async completeTask(taskId: string, result: ExternalTaskResult): Promise<void> {
    this._ensureOpen();
    await this._client.completeExternalTask(
      this.id,
      taskId,
      result.success,
      result.output,
      result.error,
    );
  }

  // --------------------------------------------------------------------------
  // Skills Management
  // --------------------------------------------------------------------------

  /** Load skills from a directory (uses server-side batch loading) */
  async loadSkills(dir: string, recursive = true): Promise<string[]> {
    this._ensureOpen();
    const resp = await this._client.loadSkillsFromDir(this.id, dir, recursive);
    return resp.loadedSkills || [];
  }

  /** Load a single skill by name */
  async loadSkill(name: string, content?: string): Promise<void> {
    this._ensureOpen();
    await this._client.loadSkill(this.id, name, content);
  }

  /** Add an inline skill definition */
  async addSkill(skill: SkillDefinition): Promise<void> {
    this._ensureOpen();
    await this._client.loadSkill(this.id, skill.name, skill.content);
  }

  /** Unload a skill */
  async unloadSkill(name: string): Promise<void> {
    this._ensureOpen();
    await this._client.unloadSkill(this.id, name);
  }

  /** List available skills */
  async listSkills(): Promise<SkillInfo[]> {
    this._ensureOpen();
    const resp = await this._client.listSkills(this.id);
    return resp.skills.map((s: Skill) => ({
      name: s.name,
      description: s.description,
      loaded: true,
      source: 'builtin' as const,
    }));
  }

  /**
   * Search for skills in the open skills ecosystem.
   *
   * @param query - Search query (e.g., "react performance", "testing")
   * @param limit - Maximum number of results (default 10)
   */
  async searchSkills(query: string, limit?: number): Promise<SkillSearchResult[]> {
    const resp: SearchSkillsResponse = await this._client.searchSkills(query, limit);
    return resp.results || [];
  }

  /**
   * Install a skill from the skills ecosystem.
   *
   * @param source - Skill source (e.g., "owner/repo@skill-name")
   * @param options - Install options
   */
  async installSkill(
    source: string,
    options?: { global?: boolean; autoLoad?: boolean },
  ): Promise<InstallSkillResponse> {
    const autoLoad = options?.autoLoad ?? true;
    return this._client.installSkill(
      source,
      options?.global,
      autoLoad ? this.id : undefined,
    );
  }

  // --------------------------------------------------------------------------
  // Built-in Agents
  // --------------------------------------------------------------------------

  /** List available agents (built-in + custom) */
  async listAgents(): Promise<AgentInfo[]> {
    this._ensureOpen();
    return [
      { name: 'explore', description: 'Read-only codebase exploration', mode: 'subagent' },
      { name: 'plan', description: 'Read-only planning and analysis', mode: 'subagent' },
      { name: 'general', description: 'Multi-step task execution', mode: 'subagent' },
      ...this._customAgents.map((a) => ({
        name: a.name,
        description: a.description,
        mode: 'subagent' as const,
      })),
    ];
  }

  /** Register a custom agent */
  registerAgent(def: AgentDefinition): void {
    this._ensureOpen();
    this._customAgents.push(def);
  }

  // --------------------------------------------------------------------------
  // Memory System
  // --------------------------------------------------------------------------

  /** Store a memory item */
  async storeMemory(memory: MemoryItem): Promise<StoreMemoryResponse> {
    this._ensureOpen();
    return this._client.storeMemory(this.id, memory);
  }

  /** Retrieve a memory by ID */
  async retrieveMemory(memoryId: string): Promise<MemoryItem | undefined> {
    this._ensureOpen();
    const resp = await this._client.retrieveMemory(this.id, memoryId);
    return resp.memory;
  }

  /** Search memories */
  async searchMemories(
    query?: string,
    tags?: string[],
    limit?: number,
    recentOnly?: boolean,
    minImportance?: number,
  ): Promise<SearchMemoriesResponse> {
    this._ensureOpen();
    return this._client.searchMemories(this.id, query, tags, limit, recentOnly, minImportance);
  }

  /** Get memory statistics */
  async getMemoryStats(): Promise<MemoryStats | undefined> {
    this._ensureOpen();
    const resp = await this._client.getMemoryStats(this.id);
    return resp.stats;
  }

  /** Clear memories */
  async clearMemories(
    clearLongTerm?: boolean,
    clearShortTerm?: boolean,
    clearWorking?: boolean,
  ): Promise<ClearMemoriesResponse> {
    this._ensureOpen();
    return this._client.clearMemories(this.id, clearLongTerm, clearShortTerm, clearWorking);
  }

  /** Delete a specific memory by ID */
  async deleteMemory(memoryId: string): Promise<DeleteMemoryResponse> {
    this._ensureOpen();
    return this._client.deleteMemory(this.id, memoryId);
  }

  /** Summarize memories using LLM */
  async summarizeMemories(
    tags?: string[],
    limit?: number,
    recentHours?: number,
  ): Promise<SummarizeMemoriesResponse> {
    this._ensureOpen();
    return this._client.summarizeMemories(this.id, tags, limit, recentHours);
  }

  // --------------------------------------------------------------------------
  // HITL & Permissions
  // --------------------------------------------------------------------------

  /** Set HITL confirmation policy */
  async setConfirmation(config: ConfirmationConfig): Promise<void> {
    this._ensureOpen();
    const policy: ProtoConfirmationPolicy = {
      enabled: true,
      requireConfirmTools: config.requireConfirmation || [],
      autoApproveTools: config.autoApprove || [],
      defaultTimeoutMs: config.timeout ?? 30_000,
      timeoutAction: timeoutActionToProto(config.timeoutAction) as ProtoConfirmationPolicy['timeoutAction'],
      yoloLanes: [],
    };
    await this._client.setConfirmationPolicy(this.id, policy);
  }

  /** Set tool permission policy */
  async setPermissions(config: PermissionConfig): Promise<void> {
    this._ensureOpen();
    const allow: PermissionRule[] = [];
    const deny: PermissionRule[] = [];
    const ask: PermissionRule[] = [];

    for (const rule of config.rules || []) {
      const ruleStr = rule.pattern ? `${rule.tool}:${rule.pattern}` : rule.tool;
      if (rule.action === 'allow') allow.push({ rule: ruleStr });
      else if (rule.action === 'deny') deny.push({ rule: ruleStr });
      else ask.push({ rule: ruleStr });
    }

    const policy: ProtoPermissionPolicy = {
      enabled: true,
      allow,
      deny,
      ask,
      defaultDecision: permissionActionToProto(config.defaultAction || 'allow') as ProtoPermissionPolicy['defaultDecision'],
    };
    await this._client.setPermissionPolicy(this.id, policy);
  }

  /** Respond to a confirmation request */
  async confirm(confirmationId: string, approved: boolean, reason?: string): Promise<void> {
    this._ensureOpen();
    await this._client.confirmToolExecution(this.id, confirmationId, approved, reason);
  }

  // --------------------------------------------------------------------------
  // Observability & Stats
  // --------------------------------------------------------------------------

  /** Get session statistics (tokens, cost, message count, tool calls) */
  async getStats(): Promise<SessionStats> {
    this._ensureOpen();
    const [ctx, cost] = await Promise.all([
      this._client.getContextUsage(this.id),
      this._client.getCostSummary({ sessionId: this.id }),
    ]);
    return {
      totalTokens: cost.totalTokens,
      promptTokens: cost.totalPromptTokens,
      completionTokens: cost.totalCompletionTokens,
      totalCost: cost.totalCostUsd,
      messageCount: ctx.usage?.messageCount ?? 0,
      toolCallCount: cost.callCount,
    };
  }

  /** Get per-tool execution metrics */
  async getToolMetrics(): Promise<Record<string, ToolStats>> {
    this._ensureOpen();
    const resp = await this._client.getToolMetrics(this.id);
    const result: Record<string, ToolStats> = {};
    for (const t of resp.tools) {
      result[t.toolName] = t;
    }
    return result;
  }

  /** Get cost breakdown by model and day */
  async getCostSummary(): Promise<GetCostSummaryResponse> {
    this._ensureOpen();
    return this._client.getCostSummary({ sessionId: this.id });
  }

  /** Get per-lane queue statistics */
  async getQueueStats(): Promise<QueueStats> {
    this._ensureOpen();
    const resp: GetQueueStatsResponse = await this._client.getQueueStats(this.id);
    const mapLane = (lane?: ProtoLaneStats): LaneStats => ({
      pending: lane?.pending ?? 0,
      active: lane?.active ?? 0,
      external: lane?.external ?? 0,
      completed: lane?.completed ?? 0,
      failed: lane?.failed ?? 0,
    });
    return {
      control: mapLane(resp.control),
      query: mapLane(resp.query),
      execute: mapLane(resp.execute),
      generate: mapLane(resp.generate),
      deadLetters: resp.deadLetters ?? 0,
    };
  }

  /** Update session configuration (cannot change model or workspace) */
  async configure(options: { autoCompact?: boolean; autoCompactThreshold?: number }): Promise<void> {
    this._ensureOpen();
    const resp = await this._client.getSession(this.id);
    const existing = resp.session?.config;
    if (existing) {
      await this._client.configureSession(this.id, {
        ...existing,
        autoCompact: options.autoCompact ?? existing.autoCompact,
      });
    }
  }

  /** Subscribe to real-time agent events for this session */
  subscribeEvents(eventTypes?: string[]): AsyncIterable<AgentEvent> {
    this._ensureOpen();
    return this._client.subscribeEvents(this.id, eventTypes);
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
