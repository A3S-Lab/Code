/**
 * A3S Code Agent TypeScript SDK
 *
 * This module exports the main client and configuration utilities
 * for interacting with the A3S Code Agent Service.
 */

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
  Session,
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
  ClaudeCodeSkill,
  GetClaudeCodeSkillsResponse,
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
