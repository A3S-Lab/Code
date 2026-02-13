/**
 * A3S Code Agent gRPC Client
 *
 * Full implementation of the CodeAgentService interface.
 */

import * as grpc from '@grpc/grpc-js';
import * as protoLoader from '@grpc/proto-loader';
import { fileURLToPath } from 'url';
import { dirname, join } from 'path';
import { loadConfigFromFile, loadConfigFromDir } from './config.js';
import {
  normalizeMessages,
  type OpenAIMessage,
  type OpenAIChatCompletion,
  type OpenAIChatCompletionChunk,
  a3sResponseToOpenAI,
  a3sChunkToOpenAI,
} from './openai-compat.js';
import { modelRefToLLMConfig } from './provider.js';
import { Session as CodeSession } from './session.js';
import type { SessionCreateOptions } from './session.js';

// Get the directory of this module
const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);

// Proto file path
const PROTO_PATH = join(__dirname, '..', 'proto', 'code_agent.proto');

// ============================================================================
// Type Definitions
// ============================================================================

// --- Enums ---

export type HealthStatus = 'STATUS_UNKNOWN' | 'STATUS_HEALTHY' | 'STATUS_DEGRADED' | 'STATUS_UNHEALTHY';
export type SessionState = 'SESSION_STATE_UNKNOWN' | 'SESSION_STATE_ACTIVE' | 'SESSION_STATE_PAUSED' | 'SESSION_STATE_COMPLETED' | 'SESSION_STATE_ERROR';
// OpenAI-compatible string roles (proto now uses strings directly)
export type MessageRole = 'user' | 'assistant' | 'system' | 'tool';
// OpenAI-compatible finish reason (string values)
export type FinishReason = 'stop' | 'length' | 'tool_calls' | 'content_filter' | 'error';
// OpenAI-compatible chunk type (string values)
export type ChunkType = 'content' | 'tool_call' | 'tool_result' | 'metadata' | 'done';
export type EventType = 'EVENT_TYPE_UNKNOWN' | 'EVENT_TYPE_SESSION_CREATED' | 'EVENT_TYPE_SESSION_DESTROYED' | 'EVENT_TYPE_GENERATION_STARTED' | 'EVENT_TYPE_GENERATION_COMPLETED' | 'EVENT_TYPE_TOOL_CALLED' | 'EVENT_TYPE_TOOL_COMPLETED' | 'EVENT_TYPE_ERROR' | 'EVENT_TYPE_WARNING' | 'EVENT_TYPE_INFO' | 'EVENT_TYPE_CONFIRMATION_REQUIRED' | 'EVENT_TYPE_CONFIRMATION_RECEIVED' | 'EVENT_TYPE_CONFIRMATION_TIMEOUT' | 'EVENT_TYPE_EXTERNAL_TASK_PENDING' | 'EVENT_TYPE_EXTERNAL_TASK_COMPLETED' | 'EVENT_TYPE_PERMISSION_DENIED' | 'EVENT_TYPE_MEMORY_STORED' | 'EVENT_TYPE_MEMORY_RECALLED' | 'EVENT_TYPE_MEMORIES_SEARCHED' | 'EVENT_TYPE_MEMORY_CLEARED';
export type SessionLaneType = 'SESSION_LANE_UNKNOWN' | 'SESSION_LANE_CONTROL' | 'SESSION_LANE_QUERY' | 'SESSION_LANE_EXECUTE' | 'SESSION_LANE_GENERATE';
export type TimeoutActionType = 'TIMEOUT_ACTION_UNKNOWN' | 'TIMEOUT_ACTION_REJECT' | 'TIMEOUT_ACTION_AUTO_APPROVE';
export type TaskHandlerModeType = 'TASK_HANDLER_MODE_UNKNOWN' | 'TASK_HANDLER_MODE_INTERNAL' | 'TASK_HANDLER_MODE_EXTERNAL' | 'TASK_HANDLER_MODE_HYBRID';
export type PermissionDecision = 'PERMISSION_DECISION_UNKNOWN' | 'PERMISSION_DECISION_ALLOW' | 'PERMISSION_DECISION_DENY' | 'PERMISSION_DECISION_ASK';
export type StorageTypeString = 'STORAGE_TYPE_UNSPECIFIED' | 'STORAGE_TYPE_MEMORY' | 'STORAGE_TYPE_FILE';

// Numeric enum values
export const StorageType = {
  STORAGE_TYPE_UNSPECIFIED: 0,
  STORAGE_TYPE_MEMORY: 1,
  STORAGE_TYPE_FILE: 2,
} as const;

export const SessionLane = {
  SESSION_LANE_UNKNOWN: 0,
  SESSION_LANE_CONTROL: 1,
  SESSION_LANE_QUERY: 2,
  SESSION_LANE_EXECUTE: 3,
  SESSION_LANE_GENERATE: 4,
} as const;

export const TimeoutAction = {
  TIMEOUT_ACTION_UNKNOWN: 0,
  TIMEOUT_ACTION_REJECT: 1,
  TIMEOUT_ACTION_AUTO_APPROVE: 2,
} as const;

export const TaskHandlerMode = {
  TASK_HANDLER_MODE_UNKNOWN: 0,
  TASK_HANDLER_MODE_INTERNAL: 1,
  TASK_HANDLER_MODE_EXTERNAL: 2,
  TASK_HANDLER_MODE_HYBRID: 3,
} as const;

// --- Lifecycle Types ---

export interface HealthCheckResponse {
  status: HealthStatus;
  message: string;
  details: Record<string, string>;
}

export interface AgentInfo {
  name: string;
  version: string;
  description: string;
  author: string;
  license: string;
  homepage: string;
}

export interface ToolCapability {
  name: string;
  description: string;
  parameters: string[];
  async: boolean;
}

export interface ModelCapability {
  provider: string;
  model: string;
  features: string[];
}

export interface ResourceLimits {
  maxContextTokens: number;
  maxConcurrentSessions: number;
  maxToolsPerRequest: number;
}

export interface GetCapabilitiesResponse {
  info?: AgentInfo;
  features: string[];
  tools: ToolCapability[];
  models: ModelCapability[];
  limits?: ResourceLimits;
  metadata: Record<string, string>;
}

export interface InitializeResponse {
  success: boolean;
  message: string;
  info?: AgentInfo;
}

export interface ShutdownResponse {
  success: boolean;
  message: string;
}

// --- Session Types ---

export interface LLMConfig {
  provider: string;
  model: string;
  apiKey?: string;
  baseUrl?: string;
  temperature?: number;
  maxTokens?: number;
}

export interface SessionConfig {
  /** Session name. */
  name: string;
  /**
   * Working directory for per-session tool sandboxing.
   *
   * Each session gets its own workspace-scoped ToolContext on the server.
   * If empty, the server falls back to its default workspace directory.
   */
  workspace: string;
  llm?: LLMConfig;
  systemPrompt?: string;
  maxContextLength?: number;
  autoCompact?: boolean;
  storageType?: number;  // StorageType enum value
}

export interface ContextUsage {
  totalTokens: number;
  promptTokens: number;
  completionTokens: number;
  messageCount: number;
}

export interface SessionInfo {
  sessionId: string;
  config?: SessionConfig;
  state: SessionState;
  contextUsage?: ContextUsage;
  createdAt: number;
  updatedAt: number;
}

/** @deprecated Use SessionInfo instead */
export type Session = SessionInfo;

export interface CreateSessionResponse {
  sessionId: string;
  session?: SessionInfo;
}

export interface DestroySessionResponse {
  success: boolean;
}

export interface ListSessionsResponse {
  sessions: SessionInfo[];
}

export interface GetSessionResponse {
  session?: SessionInfo;
}

export interface ConfigureSessionResponse {
  session?: SessionInfo;
}

// --- Message Types ---

export interface Message {
  role: MessageRole;
  content: string;
  metadata?: Record<string, string>;
}

export interface TextContent {
  text: string;
}

export interface ToolUseContent {
  id: string;
  name: string;
  arguments: string;
}

export interface ToolResultContent {
  toolUseId: string;
  content: string;
  isError: boolean;
}

export interface ContentBlock {
  text?: TextContent;
  toolUse?: ToolUseContent;
  toolResult?: ToolResultContent;
}

export interface ConversationMessage {
  id: string;
  role: string;
  content: ContentBlock[];
  timestamp: number;
  metadata: Record<string, string>;
}

export interface GetMessagesResponse {
  messages: ConversationMessage[];
  totalCount: number;
  hasMore: boolean;
}

// --- Generation Types ---

export interface ToolResult {
  success: boolean;
  output: string;
  error: string;
  metadata: Record<string, string>;
}

export interface ToolCall {
  id: string;
  name: string;
  arguments: string;
  result?: ToolResult;
}

export interface Usage {
  promptTokens: number;
  completionTokens: number;
  totalTokens: number;
}

export interface GenerateResponse {
  sessionId: string;
  message?: Message;
  toolCalls: ToolCall[];
  usage?: Usage;
  finishReason: FinishReason;
  metadata: Record<string, string>;
}

export interface GenerateChunk {
  type: ChunkType;
  sessionId: string;
  content: string;
  toolCall?: ToolCall;
  toolResult?: ToolResult;
  metadata: Record<string, string>;
  finishReason?: FinishReason;
}

export interface GenerateStructuredResponse {
  sessionId: string;
  data: string;
  usage?: Usage;
  metadata: Record<string, string>;
}

export interface GenerateStructuredChunk {
  sessionId: string;
  data: string;
  done: boolean;
}

// --- Skill Types ---

export interface Skill {
  name: string;
  description: string;
  allowedTools?: string;
  disableModelInvocation: boolean;
  content: string;
  metadata: Record<string, string>;
}

export interface LoadSkillResponse {
  success: boolean;
}

export interface UnloadSkillResponse {
  success: boolean;
}

export interface ListSkillsResponse {
  skills: Skill[];
}

export interface GetSkillResponse {
  skills: Skill[];
}

// --- Context Types ---

export interface GetContextUsageResponse {
  usage?: ContextUsage;
}

export interface CompactContextResponse {
  success: boolean;
  before?: ContextUsage;
  after?: ContextUsage;
}

export interface ClearContextResponse {
  success: boolean;
}

// --- Event Types ---

export interface AgentEvent {
  type: EventType;
  sessionId?: string;
  timestamp: number;
  message: string;
  data: Record<string, string>;
}

// --- Control Types ---

export interface CancelResponse {
  success: boolean;
}

export interface PauseResponse {
  success: boolean;
}

export interface ResumeResponse {
  success: boolean;
}

// --- HITL Types ---

export interface ConfirmationPolicy {
  enabled: boolean;
  autoApproveTools: string[];
  requireConfirmTools: string[];
  defaultTimeoutMs: number;
  timeoutAction: TimeoutActionType;
  yoloLanes: SessionLaneType[];
}

export interface ConfirmToolExecutionResponse {
  success: boolean;
  error: string;
}

export interface SetConfirmationPolicyResponse {
  success: boolean;
  policy?: ConfirmationPolicy;
}

export interface GetConfirmationPolicyResponse {
  policy?: ConfirmationPolicy;
}

// --- External Task Types ---

export interface LaneHandlerConfig {
  mode: TaskHandlerModeType;
  timeoutMs: number;
}

export interface ExternalTask {
  taskId: string;
  sessionId: string;
  lane: SessionLaneType;
  commandType: string;
  payload: string;
  timeoutMs: number;
  remainingMs: number;
}

export interface SetLaneHandlerResponse {
  success: boolean;
  config?: LaneHandlerConfig;
}

export interface GetLaneHandlerResponse {
  config?: LaneHandlerConfig;
}

export interface CompleteExternalTaskResponse {
  success: boolean;
  error: string;
}

export interface ListPendingExternalTasksResponse {
  tasks: ExternalTask[];
}

// --- Permission Types ---

export interface PermissionRule {
  rule: string;
}

export interface PermissionPolicy {
  deny: PermissionRule[];
  allow: PermissionRule[];
  ask: PermissionRule[];
  defaultDecision: PermissionDecision;
  enabled: boolean;
}

export interface SetPermissionPolicyResponse {
  success: boolean;
  policy?: PermissionPolicy;
}

export interface GetPermissionPolicyResponse {
  policy?: PermissionPolicy;
}

export interface CheckPermissionResponse {
  decision: PermissionDecision;
  matchingRules: string[];
}

export interface AddPermissionRuleResponse {
  success: boolean;
  error: string;
}

// --- Todo Types ---

export interface Todo {
  id: string;
  content: string;
  status: string;
  priority: string;
}

export interface GetTodosResponse {
  todos: Todo[];
}

export interface SetTodosResponse {
  success: boolean;
  todos: Todo[];
}

// --- Provider Types ---

export interface ModelCostInfo {
  input: number;
  output: number;
  cacheRead: number;
  cacheWrite: number;
}

export interface ModelLimitInfo {
  context: number;
  output: number;
}

export interface ModelModalitiesInfo {
  input: string[];
  output: string[];
}

export interface ModelInfo {
  id: string;
  name: string;
  family: string;
  apiKey?: string;
  baseUrl?: string;
  attachment: boolean;
  reasoning: boolean;
  toolCall: boolean;
  temperature: boolean;
  releaseDate?: string;
  modalities?: ModelModalitiesInfo;
  cost?: ModelCostInfo;
  limit?: ModelLimitInfo;
}

export interface ProviderInfo {
  name: string;
  apiKey?: string;
  baseUrl?: string;
  models: ModelInfo[];
}

export interface ListProvidersResponse {
  providers: ProviderInfo[];
  defaultProvider?: string;
  defaultModel?: string;
}

export interface GetProviderResponse {
  provider?: ProviderInfo;
}

export interface AddProviderResponse {
  success: boolean;
  error: string;
  provider?: ProviderInfo;
}

export interface UpdateProviderResponse {
  success: boolean;
  error: string;
  provider?: ProviderInfo;
}

export interface RemoveProviderResponse {
  success: boolean;
  error: string;
}

export interface SetDefaultModelResponse {
  success: boolean;
  error: string;
  provider: string;
  model: string;
}

export interface GetDefaultModelResponse {
  provider?: string;
  model?: string;
}

// --- Planning & Goal Tracking Types ---

export type Complexity = 'COMPLEXITY_UNKNOWN' | 'COMPLEXITY_SIMPLE' | 'COMPLEXITY_MEDIUM' | 'COMPLEXITY_COMPLEX' | 'COMPLEXITY_VERY_COMPLEX';
export type StepStatus = 'STEP_STATUS_UNKNOWN' | 'STEP_STATUS_PENDING' | 'STEP_STATUS_IN_PROGRESS' | 'STEP_STATUS_COMPLETED' | 'STEP_STATUS_FAILED' | 'STEP_STATUS_SKIPPED';

export interface PlanStep {
  id: string;
  description: string;
  tool?: string;
  dependencies: string[];
  status: StepStatus;
  successCriteria: string[];
}

export interface ExecutionPlan {
  goal: string;
  steps: PlanStep[];
  complexity: Complexity;
  requiredTools: string[];
  estimatedSteps: number;
}

export interface AgentGoal {
  description: string;
  successCriteria: string[];
  progress: number;
  achieved: boolean;
  createdAt: number;
  achievedAt?: number;
}

export interface CreatePlanResponse {
  plan?: ExecutionPlan;
}

export interface GetPlanResponse {
  plan?: ExecutionPlan;
}

export interface ExtractGoalResponse {
  goal?: AgentGoal;
}

export interface CheckGoalAchievementResponse {
  achieved: boolean;
  progress: number;
  remainingCriteria: string[];
}

// --- Memory System Types ---

export type MemoryType = 'MEMORY_TYPE_UNKNOWN' | 'MEMORY_TYPE_EPISODIC' | 'MEMORY_TYPE_SEMANTIC' | 'MEMORY_TYPE_PROCEDURAL' | 'MEMORY_TYPE_WORKING';

export interface MemoryItem {
  id: string;
  content: string;
  timestamp: number;
  importance: number;
  tags: string[];
  memoryType: MemoryType;
  metadata: Record<string, string>;
  accessCount: number;
  lastAccessed?: number;
}

export interface MemoryStats {
  longTermCount: number;
  shortTermCount: number;
  workingCount: number;
}

export interface StoreMemoryResponse {
  success: boolean;
  memoryId: string;
}

export interface RetrieveMemoryResponse {
  memory?: MemoryItem;
}

export interface SearchMemoriesResponse {
  memories: MemoryItem[];
  totalCount: number;
}

export interface GetMemoryStatsResponse {
  stats?: MemoryStats;
}

export interface ClearMemoriesResponse {
  success: boolean;
  clearedCount: number;
}

// --- MCP (Model Context Protocol) Types ---

export interface McpStdioTransport {
  command: string;
  args: string[];
}

export interface McpHttpTransport {
  url: string;
  headers: Record<string, string>;
}

export interface McpTransport {
  stdio?: McpStdioTransport;
  http?: McpHttpTransport;
}

export interface McpServerConfig {
  name: string;
  transport: McpTransport;
  enabled: boolean;
  env: Record<string, string>;
}

export interface RegisterMcpServerResponse {
  success: boolean;
  message: string;
}

export interface ConnectMcpServerResponse {
  success: boolean;
  message: string;
  toolNames: string[];
}

export interface DisconnectMcpServerResponse {
  success: boolean;
}

export interface McpServerInfo {
  name: string;
  connected: boolean;
  enabled: boolean;
  toolCount: number;
  error?: string;
}

export interface ListMcpServersResponse {
  servers: McpServerInfo[];
}

export interface McpToolInfo {
  fullName: string;
  serverName: string;
  toolName: string;
  description: string;
  inputSchema: string;
}

export interface GetMcpToolsResponse {
  tools: McpToolInfo[];
}

// --- LSP (Language Server Protocol) Types ---

export interface LspServerInfo {
  language: string;
  name: string;
  version?: string;
  running: boolean;
}

export interface LspPosition {
  line: number;
  character: number;
}

export interface LspRange {
  start: LspPosition;
  end: LspPosition;
}

export interface LspLocation {
  uri: string;
  range?: LspRange;
}

export interface LspSymbol {
  name: string;
  kind: string;
  location?: LspLocation;
  containerName?: string;
}

export interface LspDiagnostic {
  uri: string;
  range?: LspRange;
  severity: string;
  message: string;
  code?: string;
  source?: string;
}

export interface StartLspServerResponse {
  success: boolean;
  message: string;
  serverInfo?: LspServerInfo;
}

export interface StopLspServerResponse {
  success: boolean;
}

export interface ListLspServersResponse {
  servers: LspServerInfo[];
}

export interface LspHoverResponse {
  found: boolean;
  content: string;
  range?: LspRange;
}

export interface LspDefinitionResponse {
  locations: LspLocation[];
}

export interface LspReferencesResponse {
  locations: LspLocation[];
}

export interface LspSymbolsResponse {
  symbols: LspSymbol[];
}

export interface LspDiagnosticsResponse {
  diagnostics: LspDiagnostic[];
}

// --- Cron (Scheduled Tasks) Types ---

export type CronJobStatus = 'CRON_JOB_STATUS_UNKNOWN' | 'CRON_JOB_STATUS_ACTIVE' | 'CRON_JOB_STATUS_PAUSED' | 'CRON_JOB_STATUS_RUNNING';
export type CronExecutionStatus = 'CRON_EXECUTION_STATUS_UNKNOWN' | 'CRON_EXECUTION_STATUS_SUCCESS' | 'CRON_EXECUTION_STATUS_FAILED' | 'CRON_EXECUTION_STATUS_TIMEOUT' | 'CRON_EXECUTION_STATUS_CANCELLED';

export const CronJobStatusEnum = {
  UNKNOWN: 0,
  ACTIVE: 1,
  PAUSED: 2,
  RUNNING: 3,
} as const;

export const CronExecutionStatusEnum = {
  UNKNOWN: 0,
  SUCCESS: 1,
  FAILED: 2,
  TIMEOUT: 3,
  CANCELLED: 4,
} as const;

export interface CronJob {
  id: string;
  name: string;
  schedule: string;
  command: string;
  status: CronJobStatus;
  timeoutMs: number;
  createdAt: number;
  updatedAt: number;
  lastRun?: number;
  nextRun?: number;
  runCount: number;
  failCount: number;
  workingDir?: string;
}

export interface CronExecution {
  id: string;
  jobId: string;
  status: CronExecutionStatus;
  startedAt: number;
  endedAt?: number;
  durationMs?: number;
  exitCode?: number;
  stdout: string;
  stderr: string;
  error?: string;
}

export interface ListCronJobsResponse {
  jobs: CronJob[];
}

export interface CreateCronJobResponse {
  success: boolean;
  job?: CronJob;
  error: string;
}

export interface GetCronJobResponse {
  job?: CronJob;
}

export interface UpdateCronJobResponse {
  success: boolean;
  job?: CronJob;
  error: string;
}

export interface PauseCronJobResponse {
  success: boolean;
  job?: CronJob;
  error: string;
}

export interface ResumeCronJobResponse {
  success: boolean;
  job?: CronJob;
  error: string;
}

export interface DeleteCronJobResponse {
  success: boolean;
  error: string;
}

export interface GetCronHistoryResponse {
  executions: CronExecution[];
}

export interface RunCronJobResponse {
  success: boolean;
  execution?: CronExecution;
  error: string;
}

export interface ParseCronScheduleResponse {
  success: boolean;
  cronExpression: string;
  description: string;
  error: string;
}

// ============================================================================
// Observability Types
// ============================================================================

export interface ToolStats {
  toolName: string;
  callCount: number;
  successCount: number;
  failureCount: number;
  totalDurationMs: number;
  avgDurationMs: number;
  minDurationMs: number;
  maxDurationMs: number;
}

export interface GetToolMetricsResponse {
  tools: ToolStats[];
  totalCalls: number;
  totalDurationMs: number;
}

export interface ModelCostBreakdown {
  model: string;
  costUsd: number;
  promptTokens: number;
  completionTokens: number;
  callCount: number;
}

export interface DayCostBreakdown {
  date: string;
  costUsd: number;
  callCount: number;
}

export interface GetCostSummaryResponse {
  totalCostUsd: number;
  totalPromptTokens: number;
  totalCompletionTokens: number;
  totalTokens: number;
  callCount: number;
  byModel: ModelCostBreakdown[];
  byDay: DayCostBreakdown[];
}

// ============================================================================
// Server-Side AgenticLoop Types
// ============================================================================

export type AgenticStrategyType =
  | 'AGENTIC_STRATEGY_AUTO'
  | 'AGENTIC_STRATEGY_DIRECT'
  | 'AGENTIC_STRATEGY_PLANNED'
  | 'AGENTIC_STRATEGY_ITERATIVE'
  | 'AGENTIC_STRATEGY_PARALLEL';

export interface AgenticStep {
  stepIndex: number;
  text: string;
  toolCalls: ToolCall[];
  toolResults: ToolResult[];
  usage?: Usage;
  finishReason?: string;
}

export interface AgenticGenerateResponse {
  sessionId: string;
  text: string;
  steps: AgenticStep[];
  toolCalls: ToolCall[];
  usage?: Usage;
  finishReason: string;
  plan?: ExecutionPlan;
}

export interface AgenticGenerateEvent {
  type: string;
  sessionId: string;
  content?: string;
  toolCall?: ToolCall;
  toolResult?: ToolResult;
  toolCallId?: string;
  stepIndex?: number;
  stepText?: string;
  plan?: ExecutionPlan;
  confidence?: number;
  shouldRetry?: boolean;
  insight?: string;
  confirmationId?: string;
  toolName?: string;
  toolArgs?: string;
  timeoutMs?: number;
  approved?: boolean;
  agentName?: string;
  agentTask?: string;
  agentSessionId?: string;
  agentResult?: string;
  beforeTokens?: number;
  afterTokens?: number;
  externalTask?: ExternalTask;
  errorMessage?: string;
  recoverable?: boolean;
  finishReason?: string;
  usage?: Usage;
}

// ============================================================================
// Server-Side Delegation Types
// ============================================================================

export interface DelegateResponse {
  sessionId: string;
  agentSessionId: string;
  text: string;
  steps: AgenticStep[];
  toolCalls: ToolCall[];
  usage?: Usage;
  finishReason: string;
}

// ============================================================================
// Queue Statistics Types
// ============================================================================

export interface ProtoLaneStats {
  pending: number;
  active: number;
  external: number;
  completed: number;
  failed: number;
}

export interface GetQueueStatsResponse {
  control?: ProtoLaneStats;
  query?: ProtoLaneStats;
  execute?: ProtoLaneStats;
  generate?: ProtoLaneStats;
  deadLetters: number;
}

// ============================================================================
// Batch Skill Loading Types
// ============================================================================

export interface LoadSkillsFromDirResponse {
  success: boolean;
  loadedSkills: string[];
  errors: string[];
}

// ============================================================================
// Client Options
// ============================================================================

export interface A3sClientOptions {
  /** gRPC server address (default: localhost:4088) */
  address?: string;
  /** Use TLS for connection */
  useTls?: boolean;
  /** Connection timeout in milliseconds */
  timeout?: number;
  /** Path to config directory containing config.json */
  configDir?: string;
  /** Path to config.json file */
  configPath?: string;
}

// ============================================================================
// gRPC Client Type (dynamic)
// ============================================================================

// eslint-disable-next-line @typescript-eslint/no-explicit-any
type GrpcClient = any;

// ============================================================================
// A3sClient Class
// ============================================================================

/**
 * A3S Code Agent gRPC Client
 *
 * Provides a TypeScript interface to all CodeAgentService RPCs.
 *
 * @example
 * ```typescript
 * // Create client with config directory
 * const client = new A3sClient({ configDir: '/path/to/a3s' });
 *
 * // Create client with config file
 * const client = new A3sClient({ configPath: '/path/to/config.json' });
 *
 * // Create client with explicit address
 * const client = new A3sClient({ address: 'localhost:4088' });
 * ```
 */
export class A3sClient {
  private client: GrpcClient;
  private address: string;
  private configDir?: string;

  constructor(options: A3sClientOptions = {}) {
    this.configDir = options.configDir;

    // Load config from file if specified
    let fileConfig: { address?: string; providers?: any[] } | undefined;
    if (options.configPath) {
      fileConfig = loadConfigFromFile(options.configPath);
    } else if (options.configDir) {
      fileConfig = loadConfigFromDir(options.configDir);
    }

    // Determine address: explicit > config file > default
    this.address = options.address || fileConfig?.address || 'localhost:4088';

    // Load proto definition
    const packageDefinition = protoLoader.loadSync(PROTO_PATH, {
      keepCase: false,
      longs: String,
      enums: String,
      defaults: true,
      oneofs: true,
    });

    const protoDescriptor = grpc.loadPackageDefinition(packageDefinition);
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    const a3s = protoDescriptor.a3s as any;
    const CodeAgentService = a3s.code.agent.v1.CodeAgentService;

    // Create credentials
    const credentials = options.useTls
      ? grpc.credentials.createSsl()
      : grpc.credentials.createInsecure();

    // Create client
    this.client = new CodeAgentService(this.address, credentials);
  }

  /**
   * Get the config directory path
   */
  getConfigDir(): string | undefined {
    return this.configDir;
  }

  /**
   * Get the server address
   */
  getAddress(): string {
    return this.address;
  }

  /**
   * Close the client connection
   */
  close(): void {
    this.client.close();
  }

  // ==========================================================================
  // Helper method for promisifying unary calls
  // ==========================================================================

  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  private promisify<TReq, TRes>(method: string, request: TReq): Promise<TRes> {
    return new Promise((resolve, reject) => {
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      this.client[method](request, (error: any, response: TRes) => {
        if (error) {
          reject(error);
        } else {
          resolve(response);
        }
      });
    });
  }

  // ==========================================================================
  // Lifecycle Management
  // ==========================================================================

  /**
   * Check the health status of the agent
   */
  async healthCheck(): Promise<HealthCheckResponse> {
    return this.promisify('healthCheck', {});
  }

  /**
   * Get agent capabilities
   */
  async getCapabilities(): Promise<GetCapabilitiesResponse> {
    return this.promisify('getCapabilities', {});
  }

  /**
   * Initialize the agent with workspace and environment
   */
  async initialize(
    workspace: string,
    env?: Record<string, string>
  ): Promise<InitializeResponse> {
    return this.promisify('initialize', { workspace, env: env || {} });
  }

  /**
   * Shutdown the agent
   */
  async shutdown(): Promise<ShutdownResponse> {
    return this.promisify('shutdown', {});
  }

  // ==========================================================================
  // Session Management
  // ==========================================================================

  /**
   * Create a new session.
   *
   * Accepts either high-level options (with `model` from createProvider) or
   * low-level SessionConfig. Returns a Session object with send(),
   * sendStream(), generateText(), streamText(), etc.
   *
   * @example
   * ```typescript
   * // High-level (recommended)
   * const openai = createProvider({ name: 'openai', apiKey: 'sk-xxx' });
   * await using session = await client.createSession({
   *   model: openai('gpt-4o'),
   *   workspace: '/project',
   *   system: 'You are a helpful assistant',
   *   confirmation: { requireConfirmation: ['Bash', 'Write'] },
   *   lanes: { execute: { mode: 'external', timeout: 120_000 } },
   * });
   * const { text } = await session.send('Refactor the auth module');
   * ```
   */
  async createSession(options: SessionCreateOptions): Promise<CodeSession>;
  /**
   * Create a new session (low-level).
   * @deprecated Use the high-level overload with `model` instead.
   */
  async createSession(
    config?: SessionConfig,
    sessionId?: string,
    initialContext?: Message[],
  ): Promise<CreateSessionResponse>;
  async createSession(
    configOrOptions?: SessionConfig | SessionCreateOptions,
    sessionId?: string,
    initialContext?: Message[],
  ): Promise<CodeSession | CreateSessionResponse> {
    // High-level path: options has `model` (ModelRef)
    if (configOrOptions && 'model' in configOrOptions) {
      const opts = configOrOptions as SessionCreateOptions;
      const llm = modelRefToLLMConfig(opts.model);
      const resp: CreateSessionResponse = await this.promisify('createSession', {
        sessionId: opts.sessionId,
        config: {
          name: `session-${Date.now()}`,
          workspace: opts.workspace || '',
          llm,
          systemPrompt: opts.system,
          autoCompact: opts.autoCompact,
        },
        initialContext: opts.initialContext || [],
      });
      const session = new CodeSession(this, resp.sessionId);

      // Apply optional configurations after session creation
      if (opts.confirmation) {
        await session.setConfirmation(opts.confirmation);
      }
      if (opts.permissions) {
        await session.setPermissions(opts.permissions);
      }
      if (opts.lanes) {
        for (const [lane, config] of Object.entries(opts.lanes)) {
          if (config) {
            await session.setLaneHandler(lane as 'control' | 'query' | 'execute' | 'generate', config);
          }
        }
      }
      if (opts.skills) {
        for (const skill of opts.skills) {
          if (typeof skill === 'string') {
            await session.loadSkill(skill);
          } else {
            await session.addSkill(skill);
          }
        }
      }

      return session;
    }

    // Low-level path: raw SessionConfig
    return this.promisify('createSession', {
      sessionId,
      config: configOrOptions,
      initialContext: initialContext || [],
    });
  }

  /**
   * Destroy a session
   */
  async destroySession(sessionId: string): Promise<DestroySessionResponse> {
    return this.promisify('destroySession', { sessionId });
  }

  /**
   * List all sessions
   */
  async listSessions(): Promise<ListSessionsResponse> {
    return this.promisify('listSessions', {});
  }

  /**
   * Get a specific session
   */
  async getSession(sessionId: string): Promise<GetSessionResponse> {
    return this.promisify('getSession', { sessionId });
  }

  /**
   * Configure a session
   */
  async configureSession(
    sessionId: string,
    config: SessionConfig
  ): Promise<ConfigureSessionResponse> {
    return this.promisify('configureSession', { sessionId, config });
  }

  /**
   * Get messages from a session
   */
  async getMessages(
    sessionId: string,
    limit?: number,
    offset?: number
  ): Promise<GetMessagesResponse> {
    return this.promisify('getMessages', { sessionId, limit, offset });
  }

  // ==========================================================================
  // Code Generation
  // ==========================================================================

  /**
   * Generate a response (unary)
   *
   * Supports both A3S and OpenAI message formats:
   * - A3S: { role: 'ROLE_USER', content: '...' }
   * - OpenAI: { role: 'user', content: '...' }
   */
  async generate(
    sessionId: string,
    messages: (Message | OpenAIMessage)[]
  ): Promise<GenerateResponse> {
    const normalizedMessages = normalizeMessages(messages);
    return this.promisify('generate', { sessionId, messages: normalizedMessages });
  }

  /**
   * Generate a response (streaming)
   *
   * Supports both A3S and OpenAI message formats.
   */
  streamGenerate(
    sessionId: string,
    messages: (Message | OpenAIMessage)[]
  ): AsyncIterable<GenerateChunk> {
    const normalizedMessages = normalizeMessages(messages);
    const call = this.client.streamGenerate({ sessionId, messages: normalizedMessages });
    return this.streamToAsyncIterable(call);
  }

  /**
   * Generate structured output (unary)
   *
   * Supports both A3S and OpenAI message formats.
   */
  async generateStructured(
    sessionId: string,
    messages: (Message | OpenAIMessage)[],
    schema: string
  ): Promise<GenerateStructuredResponse> {
    const normalizedMessages = normalizeMessages(messages);
    return this.promisify('generateStructured', { sessionId, messages: normalizedMessages, schema });
  }

  /**
   * Generate structured output (streaming)
   *
   * Supports both A3S and OpenAI message formats.
   */
  streamGenerateStructured(
    sessionId: string,
    messages: (Message | OpenAIMessage)[],
    schema: string
  ): AsyncIterable<GenerateStructuredChunk> {
    const normalizedMessages = normalizeMessages(messages);
    const call = this.client.streamGenerateStructured({
      sessionId,
      messages: normalizedMessages,
      schema,
    });
    return this.streamToAsyncIterable(call);
  }

  // ==========================================================================
  // OpenAI-Compatible Methods
  // ==========================================================================

  /**
   * Generate a response in OpenAI ChatCompletion format
   *
   * This method provides full OpenAI API compatibility.
   *
   * @example
   * ```typescript
   * const completion = await client.chatCompletion(sessionId, [
   *   { role: 'user', content: 'Hello!' }
   * ]);
   * console.log(completion.choices[0].message.content);
   * ```
   */
  async chatCompletion(
    sessionId: string,
    messages: OpenAIMessage[],
    options?: {
      model?: string;
    }
  ): Promise<OpenAIChatCompletion> {
    const response = await this.generate(sessionId, messages);
    return a3sResponseToOpenAI(response, options?.model);
  }

  /**
   * Stream a response in OpenAI ChatCompletionChunk format
   *
   * This method provides full OpenAI API compatibility for streaming.
   *
   * @example
   * ```typescript
   * for await (const chunk of client.streamChatCompletion(sessionId, [
   *   { role: 'user', content: 'Hello!' }
   * ])) {
   *   const content = chunk.choices[0].delta.content;
   *   if (content) process.stdout.write(content);
   * }
   * ```
   */
  async *streamChatCompletion(
    sessionId: string,
    messages: OpenAIMessage[],
    options?: {
      model?: string;
    }
  ): AsyncIterable<OpenAIChatCompletionChunk> {
    for await (const chunk of this.streamGenerate(sessionId, messages)) {
      yield a3sChunkToOpenAI(chunk, options?.model);
    }
  }

  // ==========================================================================
  // Skill Management
  // ==========================================================================

  /**
   * Load a skill into a session
   */
  async loadSkill(
    sessionId: string,
    skillName: string,
    skillContent?: string
  ): Promise<LoadSkillResponse> {
    return this.promisify('loadSkill', { sessionId, skillName, skillContent });
  }

  /**
   * Unload a skill from a session
   */
  async unloadSkill(
    sessionId: string,
    skillName: string
  ): Promise<UnloadSkillResponse> {
    return this.promisify('unloadSkill', { sessionId, skillName });
  }

  /**
   * List available skills
   */
  async listSkills(sessionId?: string): Promise<ListSkillsResponse> {
    return this.promisify('listSkills', { sessionId });
  }

  /**
   * Get skills by name or all skills
   */
  async getSkill(name?: string): Promise<GetSkillResponse> {
    return this.promisify('getSkill', { name });
  }

  // ==========================================================================
  // Context Management
  // ==========================================================================

  /**
   * Get context usage for a session
   */
  async getContextUsage(sessionId: string): Promise<GetContextUsageResponse> {
    return this.promisify('getContextUsage', { sessionId });
  }

  /**
   * Compact context for a session
   */
  async compactContext(sessionId: string): Promise<CompactContextResponse> {
    return this.promisify('compactContext', { sessionId });
  }

  /**
   * Clear context for a session
   */
  async clearContext(sessionId: string): Promise<ClearContextResponse> {
    return this.promisify('clearContext', { sessionId });
  }

  // ==========================================================================
  // Event Streaming
  // ==========================================================================

  /**
   * Subscribe to agent events
   */
  subscribeEvents(
    sessionId?: string,
    eventTypes?: string[]
  ): AsyncIterable<AgentEvent> {
    const call = this.client.subscribeEvents({
      sessionId,
      eventTypes: eventTypes || [],
    });
    return this.streamToAsyncIterable(call);
  }

  // ==========================================================================
  // Control Operations
  // ==========================================================================

  /**
   * Cancel an operation
   */
  async cancel(sessionId: string, operationId?: string): Promise<CancelResponse> {
    return this.promisify('cancel', { sessionId, operationId });
  }

  /**
   * Pause a session
   */
  async pause(sessionId: string): Promise<PauseResponse> {
    return this.promisify('pause', { sessionId });
  }

  /**
   * Resume a session
   */
  async resume(sessionId: string): Promise<ResumeResponse> {
    return this.promisify('resume', { sessionId });
  }

  // ==========================================================================
  // Human-in-the-Loop (HITL)
  // ==========================================================================

  /**
   * Confirm or reject tool execution
   */
  async confirmToolExecution(
    sessionId: string,
    toolId: string,
    approved: boolean,
    reason?: string
  ): Promise<ConfirmToolExecutionResponse> {
    return this.promisify('confirmToolExecution', {
      sessionId,
      toolId,
      approved,
      reason,
    });
  }

  /**
   * Set confirmation policy for a session
   */
  async setConfirmationPolicy(
    sessionId: string,
    policy: ConfirmationPolicy
  ): Promise<SetConfirmationPolicyResponse> {
    return this.promisify('setConfirmationPolicy', { sessionId, policy });
  }

  /**
   * Get confirmation policy for a session
   */
  async getConfirmationPolicy(
    sessionId: string
  ): Promise<GetConfirmationPolicyResponse> {
    return this.promisify('getConfirmationPolicy', { sessionId });
  }

  // ==========================================================================
  // External Task Handling
  // ==========================================================================

  /**
   * Set lane handler configuration
   */
  async setLaneHandler(
    sessionId: string,
    lane: SessionLaneType,
    config: LaneHandlerConfig
  ): Promise<SetLaneHandlerResponse> {
    return this.promisify('setLaneHandler', { sessionId, lane, config });
  }

  /**
   * Get lane handler configuration
   */
  async getLaneHandler(
    sessionId: string,
    lane: SessionLaneType
  ): Promise<GetLaneHandlerResponse> {
    return this.promisify('getLaneHandler', { sessionId, lane });
  }

  /**
   * Complete an external task
   */
  async completeExternalTask(
    sessionId: string,
    taskId: string,
    success: boolean,
    result?: string,
    error?: string
  ): Promise<CompleteExternalTaskResponse> {
    return this.promisify('completeExternalTask', {
      sessionId,
      taskId,
      success,
      result: result || '',
      error: error || '',
    });
  }

  /**
   * List pending external tasks
   */
  async listPendingExternalTasks(
    sessionId: string
  ): Promise<ListPendingExternalTasksResponse> {
    return this.promisify('listPendingExternalTasks', { sessionId });
  }

  // ==========================================================================
  // Permission System
  // ==========================================================================

  /**
   * Set permission policy for a session
   */
  async setPermissionPolicy(
    sessionId: string,
    policy: PermissionPolicy
  ): Promise<SetPermissionPolicyResponse> {
    return this.promisify('setPermissionPolicy', { sessionId, policy });
  }

  /**
   * Get permission policy for a session
   */
  async getPermissionPolicy(
    sessionId: string
  ): Promise<GetPermissionPolicyResponse> {
    return this.promisify('getPermissionPolicy', { sessionId });
  }

  /**
   * Check permission for a tool call
   */
  async checkPermission(
    sessionId: string,
    toolName: string,
    args: string
  ): Promise<CheckPermissionResponse> {
    return this.promisify('checkPermission', {
      sessionId,
      toolName,
      arguments: args,
    });
  }

  /**
   * Add a permission rule
   */
  async addPermissionRule(
    sessionId: string,
    ruleType: 'allow' | 'deny' | 'ask',
    rule: string
  ): Promise<AddPermissionRuleResponse> {
    return this.promisify('addPermissionRule', { sessionId, ruleType, rule });
  }

  // ==========================================================================
  // Todo/Task Tracking
  // ==========================================================================

  /**
   * Get todos for a session
   */
  async getTodos(sessionId: string): Promise<GetTodosResponse> {
    return this.promisify('getTodos', { sessionId });
  }

  /**
   * Set todos for a session
   */
  async setTodos(sessionId: string, todos: Todo[]): Promise<SetTodosResponse> {
    return this.promisify('setTodos', { sessionId, todos });
  }

  // ==========================================================================
  // Provider Configuration
  // ==========================================================================

  /**
   * List all providers
   */
  async listProviders(): Promise<ListProvidersResponse> {
    return this.promisify('listProviders', {});
  }

  /**
   * Get a specific provider
   */
  async getProvider(name: string): Promise<GetProviderResponse> {
    return this.promisify('getProvider', { name });
  }

  /**
   * Add a new provider
   */
  async addProvider(provider: ProviderInfo): Promise<AddProviderResponse> {
    return this.promisify('addProvider', { provider });
  }

  /**
   * Update an existing provider
   */
  async updateProvider(provider: ProviderInfo): Promise<UpdateProviderResponse> {
    return this.promisify('updateProvider', { provider });
  }

  /**
   * Remove a provider
   */
  async removeProvider(name: string): Promise<RemoveProviderResponse> {
    return this.promisify('removeProvider', { name });
  }

  /**
   * Set the default model
   */
  async setDefaultModel(
    provider: string,
    model: string
  ): Promise<SetDefaultModelResponse> {
    return this.promisify('setDefaultModel', { provider, model });
  }

  /**
   * Get the default model
   */
  async getDefaultModel(): Promise<GetDefaultModelResponse> {
    return this.promisify('getDefaultModel', {});
  }

  // ==========================================================================
  // Planning & Goal Tracking
  // ==========================================================================

  /**
   * Create an execution plan
   */
  async createPlan(
    sessionId: string,
    prompt: string,
    context?: string
  ): Promise<CreatePlanResponse> {
    return this.promisify('createPlan', { sessionId, prompt, context });
  }

  /**
   * Get an existing plan
   */
  async getPlan(sessionId: string, planId: string): Promise<GetPlanResponse> {
    return this.promisify('getPlan', { sessionId, planId });
  }

  /**
   * Extract goal from prompt
   */
  async extractGoal(
    sessionId: string,
    prompt: string
  ): Promise<ExtractGoalResponse> {
    return this.promisify('extractGoal', { sessionId, prompt });
  }

  /**
   * Check if goal is achieved
   */
  async checkGoalAchievement(
    sessionId: string,
    goal: AgentGoal,
    currentState: string
  ): Promise<CheckGoalAchievementResponse> {
    return this.promisify('checkGoalAchievement', {
      sessionId,
      goal,
      currentState,
    });
  }

  // ==========================================================================
  // Memory System
  // ==========================================================================

  /**
   * Store a memory item
   */
  async storeMemory(
    sessionId: string,
    memory: MemoryItem
  ): Promise<StoreMemoryResponse> {
    return this.promisify('storeMemory', { sessionId, memory });
  }

  /**
   * Retrieve a memory by ID
   */
  async retrieveMemory(
    sessionId: string,
    memoryId: string
  ): Promise<RetrieveMemoryResponse> {
    return this.promisify('retrieveMemory', { sessionId, memoryId });
  }

  /**
   * Search memories
   */
  async searchMemories(
    sessionId: string,
    query?: string,
    tags?: string[],
    limit?: number,
    recentOnly?: boolean,
    minImportance?: number
  ): Promise<SearchMemoriesResponse> {
    return this.promisify('searchMemories', {
      sessionId,
      query,
      tags: tags || [],
      limit: limit || 10,
      recentOnly: recentOnly || false,
      minImportance,
    });
  }

  /**
   * Get memory statistics
   */
  async getMemoryStats(sessionId: string): Promise<GetMemoryStatsResponse> {
    return this.promisify('getMemoryStats', { sessionId });
  }

  /**
   * Clear memories
   */
  async clearMemories(
    sessionId: string,
    clearLongTerm?: boolean,
    clearShortTerm?: boolean,
    clearWorking?: boolean
  ): Promise<ClearMemoriesResponse> {
    return this.promisify('clearMemories', {
      sessionId,
      clearLongTerm: clearLongTerm || false,
      clearShortTerm: clearShortTerm || false,
      clearWorking: clearWorking || false,
    });
  }

  // ==========================================================================
  // MCP (Model Context Protocol) Methods
  // ==========================================================================

  /**
   * Register an MCP server
   */
  async registerMcpServer(config: McpServerConfig): Promise<RegisterMcpServerResponse> {
    return this.promisify('registerMcpServer', { config });
  }

  /**
   * Connect to an MCP server
   */
  async connectMcpServer(name: string): Promise<ConnectMcpServerResponse> {
    return this.promisify('connectMcpServer', { name });
  }

  /**
   * Disconnect from an MCP server
   */
  async disconnectMcpServer(name: string): Promise<DisconnectMcpServerResponse> {
    return this.promisify('disconnectMcpServer', { name });
  }

  /**
   * List all MCP servers
   */
  async listMcpServers(): Promise<ListMcpServersResponse> {
    return this.promisify('listMcpServers', {});
  }

  /**
   * Get MCP tools
   */
  async getMcpTools(serverName?: string): Promise<GetMcpToolsResponse> {
    return this.promisify('getMcpTools', { serverName });
  }

  // ==========================================================================
  // LSP (Language Server Protocol) Methods
  // ==========================================================================

  /**
   * Start a language server for a specific language
   */
  async startLspServer(
    language: string,
    rootUri?: string
  ): Promise<StartLspServerResponse> {
    return this.promisify('startLspServer', {
      language,
      rootUri: rootUri || '',
    });
  }

  /**
   * Stop a running language server
   */
  async stopLspServer(language: string): Promise<StopLspServerResponse> {
    return this.promisify('stopLspServer', { language });
  }

  /**
   * List all running language servers
   */
  async listLspServers(): Promise<ListLspServersResponse> {
    return this.promisify('listLspServers', {});
  }

  /**
   * Get hover information at a specific position
   */
  async lspHover(
    filePath: string,
    line: number,
    column: number
  ): Promise<LspHoverResponse> {
    return this.promisify('lspHover', {
      filePath,
      line,
      column,
    });
  }

  /**
   * Go to definition at a specific position
   */
  async lspDefinition(
    filePath: string,
    line: number,
    column: number
  ): Promise<LspDefinitionResponse> {
    return this.promisify('lspDefinition', {
      filePath,
      line,
      column,
    });
  }

  /**
   * Find all references at a specific position
   */
  async lspReferences(
    filePath: string,
    line: number,
    column: number,
    includeDeclaration?: boolean
  ): Promise<LspReferencesResponse> {
    return this.promisify('lspReferences', {
      filePath,
      line,
      column,
      includeDeclaration: includeDeclaration || false,
    });
  }

  /**
   * Search for symbols in the workspace
   */
  async lspSymbols(query: string, limit?: number): Promise<LspSymbolsResponse> {
    return this.promisify('lspSymbols', {
      query,
      limit: limit || 20,
    });
  }

  /**
   * Get diagnostics for a file
   */
  async lspDiagnostics(filePath?: string): Promise<LspDiagnosticsResponse> {
    return this.promisify('lspDiagnostics', {
      filePath: filePath || '',
    });
  }

  // ==========================================================================
  // Cron (Scheduled Tasks)
  // ==========================================================================

  /**
   * List all cron jobs
   */
  async listCronJobs(): Promise<ListCronJobsResponse> {
    return this.promisify('listCronJobs', {});
  }

  /**
   * Create a new cron job
   * @param name Job name
   * @param schedule Schedule expression (cron syntax or natural language)
   * @param command Command to execute
   * @param timeoutMs Execution timeout in milliseconds (default: 60000)
   */
  async createCronJob(
    name: string,
    schedule: string,
    command: string,
    timeoutMs?: number
  ): Promise<CreateCronJobResponse> {
    return this.promisify('createCronJob', {
      name,
      schedule,
      command,
      timeoutMs,
    });
  }

  /**
   * Get a cron job by ID or name
   * @param id Job ID
   * @param name Job name (alternative to ID)
   */
  async getCronJob(id?: string, name?: string): Promise<GetCronJobResponse> {
    return this.promisify('getCronJob', { id, name });
  }

  /**
   * Update a cron job
   * @param id Job ID
   * @param schedule New schedule expression
   * @param command New command
   * @param timeoutMs New timeout
   */
  async updateCronJob(
    id: string,
    schedule?: string,
    command?: string,
    timeoutMs?: number
  ): Promise<UpdateCronJobResponse> {
    return this.promisify('updateCronJob', {
      id,
      schedule,
      command,
      timeoutMs,
    });
  }

  /**
   * Pause a cron job
   * @param id Job ID
   */
  async pauseCronJob(id: string): Promise<PauseCronJobResponse> {
    return this.promisify('pauseCronJob', { id });
  }

  /**
   * Resume a paused cron job
   * @param id Job ID
   */
  async resumeCronJob(id: string): Promise<ResumeCronJobResponse> {
    return this.promisify('resumeCronJob', { id });
  }

  /**
   * Delete a cron job
   * @param id Job ID
   */
  async deleteCronJob(id: string): Promise<DeleteCronJobResponse> {
    return this.promisify('deleteCronJob', { id });
  }

  /**
   * Get execution history for a cron job
   * @param id Job ID
   * @param limit Max records to return (default: 10)
   */
  async getCronHistory(id: string, limit?: number): Promise<GetCronHistoryResponse> {
    return this.promisify('getCronHistory', { id, limit });
  }

  /**
   * Manually run a cron job
   * @param id Job ID
   */
  async runCronJob(id: string): Promise<RunCronJobResponse> {
    return this.promisify('runCronJob', { id });
  }

  /**
   * Parse natural language schedule to cron expression
   * @param input Natural language or cron expression
   */
  async parseCronSchedule(input: string): Promise<ParseCronScheduleResponse> {
    return this.promisify('parseCronSchedule', { input });
  }

  // ==========================================================================
  // Observability
  // ==========================================================================

  async getToolMetrics(
    sessionId?: string,
    toolName?: string,
  ): Promise<GetToolMetricsResponse> {
    return this.promisify('getToolMetrics', {
      session_id: sessionId || '',
      tool_name: toolName || '',
    });
  }

  async getCostSummary(options?: {
    sessionId?: string;
    model?: string;
    startDate?: string;
    endDate?: string;
  }): Promise<GetCostSummaryResponse> {
    return this.promisify('getCostSummary', {
      session_id: options?.sessionId || '',
      model: options?.model || '',
      start_date: options?.startDate || '',
      end_date: options?.endDate || '',
    });
  }

  // ==========================================================================
  // Server-Side AgenticLoop
  // ==========================================================================

  /**
   * Run the server-side AgenticLoop (non-streaming).
   * The server executes the full loop: generate → tool call → execute → reflect → repeat.
   */
  async agenticGenerate(
    sessionId: string,
    prompt: string,
    options?: {
      strategy?: string;
      maxSteps?: number;
      reflection?: boolean;
      planning?: boolean;
    },
  ): Promise<AgenticGenerateResponse> {
    return this.promisify('agenticGenerate', {
      sessionId,
      prompt,
      strategy: options?.strategy || 'AGENTIC_STRATEGY_AUTO',
      maxSteps: options?.maxSteps || 50,
      reflection: options?.reflection ?? true,
      planning: options?.planning ?? true,
    });
  }

  /**
   * Stream the server-side AgenticLoop.
   * Returns a stream of AgenticGenerateEvent for real-time UI updates.
   */
  streamAgenticGenerate(
    sessionId: string,
    prompt: string,
    options?: {
      strategy?: string;
      maxSteps?: number;
      reflection?: boolean;
      planning?: boolean;
    },
  ): AsyncIterable<AgenticGenerateEvent> {
    const call = this.client.streamAgenticGenerate({
      sessionId,
      prompt,
      strategy: options?.strategy || 'AGENTIC_STRATEGY_AUTO',
      maxSteps: options?.maxSteps || 50,
      reflection: options?.reflection ?? true,
      planning: options?.planning ?? true,
    });
    return this.streamToAsyncIterable(call);
  }

  // ==========================================================================
  // Server-Side Delegation (Subagents)
  // ==========================================================================

  /**
   * Delegate a task to a built-in or custom subagent (non-streaming).
   */
  async delegateToAgent(
    sessionId: string,
    agentName: string,
    task: string,
    options?: {
      maxSteps?: number;
      allowedTools?: string[];
    },
  ): Promise<DelegateResponse> {
    return this.promisify('delegate', {
      sessionId,
      agentName,
      task,
      maxSteps: options?.maxSteps || 50,
      allowedTools: options?.allowedTools || [],
    });
  }

  /**
   * Delegate a task to a subagent with streaming events.
   */
  streamDelegateToAgent(
    sessionId: string,
    agentName: string,
    task: string,
    options?: {
      maxSteps?: number;
      allowedTools?: string[];
    },
  ): AsyncIterable<AgenticGenerateEvent> {
    const call = this.client.streamDelegate({
      sessionId,
      agentName,
      task,
      maxSteps: options?.maxSteps || 50,
      allowedTools: options?.allowedTools || [],
    });
    return this.streamToAsyncIterable(call);
  }

  // ==========================================================================
  // Queue Statistics
  // ==========================================================================

  /**
   * Get per-lane queue statistics for a session.
   */
  async getQueueStats(sessionId: string): Promise<GetQueueStatsResponse> {
    return this.promisify('getQueueStats', { sessionId });
  }

  // ==========================================================================
  // Batch Skill Loading
  // ==========================================================================

  /**
   * Load all skills from a directory.
   */
  async loadSkillsFromDir(
    sessionId: string,
    directory: string,
    recursive?: boolean,
  ): Promise<LoadSkillsFromDirResponse> {
    return this.promisify('loadSkillsFromDir', {
      sessionId,
      directory,
      recursive: recursive ?? true,
    });
  }

  // ==========================================================================
  // Helper: Convert gRPC stream to AsyncIterable
  // ==========================================================================

  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  private streamToAsyncIterable<T>(call: any): AsyncIterable<T> {
    return {
      [Symbol.asyncIterator](): AsyncIterator<T> {
        return {
          next(): Promise<IteratorResult<T>> {
            return new Promise((resolve, reject) => {
              call.once('data', (data: T) => {
                resolve({ value: data, done: false });
              });
              call.once('end', () => {
                resolve({ value: undefined as T, done: true });
              });
              call.once('error', (error: Error) => {
                reject(error);
              });
            });
          },
        };
      },
    };
  }
}
