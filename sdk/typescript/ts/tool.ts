/**
 * Tool Definition Helper
 *
 * Vercel AI SDK-style tool() function for defining tools with type safety.
 *
 * A3S Code has two kinds of tools:
 * - **Server tools**: Built-in tools (file ops, shell, etc.) that run on the server.
 *   These are always available and don't need to be defined client-side.
 * - **Client tools**: Custom tools defined via tool() that run client-side.
 *   The model can invoke these, and results are sent back to continue generation.
 *
 * @example
 * ```typescript
 * import { tool, streamText, createProvider } from '@a3s-lab/code';
 *
 * const openai = createProvider({ name: 'openai', apiKey: 'sk-xxx' });
 *
 * const weatherTool = tool({
 *   description: 'Get the weather in a location',
 *   parameters: {
 *     type: 'object',
 *     properties: {
 *       location: { type: 'string', description: 'City name' },
 *     },
 *     required: ['location'],
 *   },
 *   execute: async ({ location }) => ({
 *     location,
 *     temperature: 72,
 *     condition: 'sunny',
 *   }),
 * });
 *
 * const { text } = await generateText({
 *   model: openai('gpt-4o'),
 *   tools: { weather: weatherTool },
 *   maxSteps: 3,
 *   prompt: 'What is the weather in Tokyo?',
 * });
 * ```
 */

// ============================================================================
// Types
// ============================================================================

/** JSON Schema object for tool parameters */
export interface JsonSchema {
  type: string;
  properties?: Record<string, JsonSchema & { description?: string }>;
  required?: string[];
  items?: JsonSchema;
  enum?: string[];
  description?: string;
  [key: string]: unknown;
}

/** Options passed to tool execute function */
export interface ToolExecutionOptions {
  /** Unique ID for this tool call */
  toolCallId: string;
  /** Abort signal for cancellation */
  abortSignal?: AbortSignal;
}

/** Tool definition */
export interface ToolDefinition<TParams = Record<string, unknown>, TResult = unknown> {
  /** Description of what the tool does (shown to the model) */
  description: string;
  /** JSON Schema for the tool's parameters */
  parameters: JsonSchema;
  /**
   * Execute function — runs when the model invokes this tool.
   * If omitted, the tool call is forwarded to onToolCall callback.
   */
  execute?: (args: TParams, options: ToolExecutionOptions) => Promise<TResult>;
  /**
   * Whether this tool needs human approval before execution.
   * - `true`: always requires approval
   * - `false` or omitted: auto-execute
   * - function: dynamically decide based on args
   */
  needsApproval?: boolean | ((args: TParams) => boolean | Promise<boolean>);
}

/** Map of tool name → tool definition */
export type ToolSet = Record<string, ToolDefinition>;

// ============================================================================
// Factory
// ============================================================================

/**
 * Define a tool with type-safe parameters and execution.
 *
 * This is a type helper — it returns the definition as-is but provides
 * TypeScript inference for the execute function's parameter types.
 *
 * @example
 * ```typescript
 * const calculator = tool({
 *   description: 'Perform arithmetic',
 *   parameters: {
 *     type: 'object',
 *     properties: {
 *       expression: { type: 'string', description: 'Math expression' },
 *     },
 *     required: ['expression'],
 *   },
 *   execute: async ({ expression }) => {
 *     return { result: eval(expression) };
 *   },
 * });
 * ```
 */
export function tool<TParams = Record<string, unknown>, TResult = unknown>(
  definition: ToolDefinition<TParams, TResult>,
): ToolDefinition<TParams, TResult> {
  return definition;
}
