/**
 * A3S Code Agent TypeScript SDK
 *
 * Session-based AI coding agent SDK with AgenticLoop, Skills, HITL, and Lane Queue.
 *
 * @example Quick Start
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
 *   confirmation: { requireConfirmation: ['Bash', 'Write'] },
 * });
 *
 * // Simple question
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
 *   if (event.type === 'tool_call') console.log(`🔧 ${event.toolName}`);
 * }
 * ```
 */

// ============================================================================
// Session (Core API)
// ============================================================================

export { Session } from './session.js';
export type {
  SessionCreateOptions,
  MessageInput,
  StepResult,
  ToolCallEvent,
  SessionGenerateTextOptions,
  SessionGenerateObjectOptions,
  GenerateTextResult,
  StreamTextResult,
  GenerateObjectResult,
  StreamObjectResult,
  // Agentic types
  SendOptions,
  SendResult,
  SendStreamResult,
  AgentLoopEvent,
  ConfirmationRequest,
  ConfirmationConfig,
  PermissionConfig,
  SkillDefinition,
  SkillInfo,
  AgentDefinition,
  AgentInfo,
  LaneName,
  LaneConfig,
  LaneStats,
  QueueStats,
  ExternalTaskResult,
  SessionStats,
} from './session.js';

// ============================================================================
// High-Level API (Vercel AI SDK-style convenience wrappers)
// ============================================================================

export { generateText, streamText, generateObject, streamObject } from './generate.js';
export type {
  BaseGenerateOptions,
  GenerateTextOptions,
  GenerateObjectOptions,
} from './generate.js';

export { createChat } from './chat.js';
export type {
  ChatOptions,
  ChatSendResult,
  ChatStreamResult,
  Chat,
} from './chat.js';

export { createProvider, model } from './provider.js';
export type {
  ProviderOptions,
  ModelRef,
  ModelSelector,
} from './provider.js';

export { tool } from './tool.js';
export type {
  JsonSchema,
  ToolExecutionOptions,
  ToolDefinition,
  ToolSet,
} from './tool.js';

// UIMessage / ModelMessage Conversion
export {
  convertToModelMessages,
  convertToUIMessages,
  a3sMessageToModel,
  modelMessageToA3s,
  a3sMessagesToModel,
  modelMessagesToA3s,
  a3sMessagesToUI,
  uiMessagesToA3s,
} from './message.js';
export type {
  UIMessage,
  UIMessagePart,
  UIMessageTextPart,
  UIMessageToolInvocationPart,
  UIMessageStepBoundaryPart,
  UIMessageReasoningPart,
  ModelMessage,
  SystemModelMessage,
  UserModelMessage,
  AssistantModelMessage,
  ToolModelMessage,
} from './message.js';

// ============================================================================
// Low-Level Client (for advanced usage)
// ============================================================================

export { A3sClient, StorageType, SessionLane, TimeoutAction, TaskHandlerMode, CronJobStatusEnum, CronExecutionStatusEnum } from './client.js';
export type {
  A3sClientOptions,
  // Lifecycle types
  HealthStatus,
  HealthCheckResponse,
  AgentInfo,
  ToolCapability,
  ModelCapability,
  ResourceLimits,
  GetCapabilitiesResponse,
  InitializeResponse,
  ShutdownResponse,
  // Session types
  StorageTypeString,
  LLMConfig,
  SessionConfig,
  ContextUsage,
  SessionState,
  SessionInfo,
  CreateSessionResponse,
  DestroySessionResponse,
  ListSessionsResponse,
  GetSessionResponse,
  ConfigureSessionResponse,
  // Message types
  MessageRole,
  Message,
  TextContent,
  ToolUseContent,
  ToolResultContent,
  ContentBlock,
  ConversationMessage,
  GetMessagesResponse,
  // Generation types
  ToolResult,
  ToolCall,
  Usage,
  FinishReason,
  GenerateResponse,
  ChunkType,
  GenerateChunk,
  GenerateStructuredResponse,
  GenerateStructuredChunk,
  // Skill types
  Skill,
  GetSkillResponse,
  LoadSkillResponse,
  UnloadSkillResponse,
  ListSkillsResponse,
  // Context types
  GetContextUsageResponse,
  CompactContextResponse,
  ClearContextResponse,
  // Event types
  EventType,
  AgentEvent,
  // Control types
  CancelResponse,
  PauseResponse,
  ResumeResponse,
  // HITL types
  SessionLaneType,
  TimeoutActionType,
  ConfirmationPolicy,
  ConfirmToolExecutionResponse,
  SetConfirmationPolicyResponse,
  GetConfirmationPolicyResponse,
  // External task types
  TaskHandlerModeType,
  LaneHandlerConfig,
  ExternalTask,
  SetLaneHandlerResponse,
  GetLaneHandlerResponse,
  CompleteExternalTaskResponse,
  ListPendingExternalTasksResponse,
  // Permission types
  PermissionDecision,
  PermissionRule,
  PermissionPolicy,
  SetPermissionPolicyResponse,
  GetPermissionPolicyResponse,
  CheckPermissionResponse,
  AddPermissionRuleResponse,
  // Todo types
  Todo,
  GetTodosResponse,
  SetTodosResponse,
  // Provider types
  ModelCostInfo,
  ModelLimitInfo,
  ModelModalitiesInfo,
  ModelInfo,
  ProviderInfo,
  ListProvidersResponse,
  GetProviderResponse,
  AddProviderResponse,
  UpdateProviderResponse,
  RemoveProviderResponse,
  SetDefaultModelResponse,
  GetDefaultModelResponse,
  // Planning & Goal Tracking types
  Complexity,
  StepStatus,
  PlanStep,
  ExecutionPlan,
  AgentGoal,
  CreatePlanResponse,
  GetPlanResponse,
  ExtractGoalResponse,
  CheckGoalAchievementResponse,
  // Memory System types
  MemoryType,
  MemoryItem,
  MemoryStats,
  StoreMemoryResponse,
  RetrieveMemoryResponse,
  SearchMemoriesResponse,
  GetMemoryStatsResponse,
  ClearMemoriesResponse,
  // MCP (Model Context Protocol) types
  McpStdioTransport,
  McpHttpTransport,
  McpTransport,
  McpServerConfig,
  McpServerInfo,
  McpToolInfo,
  RegisterMcpServerResponse,
  ConnectMcpServerResponse,
  DisconnectMcpServerResponse,
  ListMcpServersResponse,
  GetMcpToolsResponse,
  // LSP (Language Server Protocol) types
  LspServerInfo,
  LspPosition,
  LspRange,
  LspLocation,
  LspSymbol,
  LspDiagnostic,
  StartLspServerResponse,
  StopLspServerResponse,
  ListLspServersResponse,
  LspHoverResponse,
  LspDefinitionResponse,
  LspReferencesResponse,
  LspSymbolsResponse,
  LspDiagnosticsResponse,
  // Cron (Scheduled Tasks) types
  CronJobStatus,
  CronExecutionStatus,
  CronJob,
  CronExecution,
  ListCronJobsResponse,
  CreateCronJobResponse,
  GetCronJobResponse,
  UpdateCronJobResponse,
  PauseCronJobResponse,
  ResumeCronJobResponse,
  DeleteCronJobResponse,
  GetCronHistoryResponse,
  RunCronJobResponse,
  ParseCronScheduleResponse,
  // Observability types
  ToolStats,
  GetToolMetricsResponse,
  ModelCostBreakdown,
  DayCostBreakdown,
  GetCostSummaryResponse,
} from './client.js';

export {
  getConfig,
  getModelConfig,
  getDefaultModel,
  printConfig,
  loadConfigFromFile,
  loadConfigFromDir,
  loadDefaultConfig,
} from './config.js';

export type {
  A3sConfig,
  ProviderConfig,
  ModelConfigEntry,
  ModelConfig,
} from './config.js';

// OpenAI-Compatible Types and Utilities
export {
  // Conversion functions
  openAIRoleToA3S,
  a3sRoleToOpenAI,
  openAIMessageToA3S,
  a3sMessageToOpenAI,
  openAIMessagesToA3S,
  normalizeMessage,
  normalizeMessages,
  a3sResponseToOpenAI,
  a3sChunkToOpenAI,
  // Type guards
  isOpenAIFormat,
  isTextContent,
  isImageContent,
} from './openai-compat.js';

export type {
  // OpenAI-compatible types
  OpenAIRole,
  OpenAIMessage,
  OpenAITextContent,
  OpenAIImageContent,
  OpenAIContentPart,
  OpenAIToolCall,
  OpenAIChoice,
  OpenAIUsage,
  OpenAIChatCompletion,
  OpenAIDelta,
  OpenAIStreamChoice,
  OpenAIChatCompletionChunk,
} from './openai-compat.js';
