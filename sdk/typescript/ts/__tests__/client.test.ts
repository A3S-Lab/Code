/**
 * Unit tests for A3sClient
 *
 * Uses vitest with mocked gRPC client
 */

import { describe, it, expect, vi, beforeEach } from 'vitest';

// Mock the gRPC modules before importing the client
vi.mock('@grpc/grpc-js', () => ({
  credentials: {
    createInsecure: vi.fn(() => 'insecure-creds'),
    createSsl: vi.fn(() => 'ssl-creds'),
  },
  loadPackageDefinition: vi.fn(() => ({
    a3s: {
      code: {
        agent: {
          v1: {
            CodeAgentService: vi.fn().mockImplementation(() => mockGrpcClient),
          },
        },
      },
    },
  })),
}));

vi.mock('@grpc/proto-loader', () => ({
  loadSync: vi.fn(() => ({})),
}));

// Mock gRPC client instance
const mockGrpcClient = {
  close: vi.fn(),
  healthCheck: vi.fn(),
  getCapabilities: vi.fn(),
  initialize: vi.fn(),
  shutdown: vi.fn(),
  createSession: vi.fn(),
  destroySession: vi.fn(),
  listSessions: vi.fn(),
  getSession: vi.fn(),
  configureSession: vi.fn(),
  getMessages: vi.fn(),
  generate: vi.fn(),
  streamGenerate: vi.fn(),
  generateStructured: vi.fn(),
  streamGenerateStructured: vi.fn(),
  loadSkill: vi.fn(),
  unloadSkill: vi.fn(),
  listSkills: vi.fn(),
  getContextUsage: vi.fn(),
  compactContext: vi.fn(),
  clearContext: vi.fn(),
  subscribeEvents: vi.fn(),
  cancel: vi.fn(),
  pause: vi.fn(),
  resume: vi.fn(),
  confirmToolExecution: vi.fn(),
  setConfirmationPolicy: vi.fn(),
  getConfirmationPolicy: vi.fn(),
  setLaneHandler: vi.fn(),
  getLaneHandler: vi.fn(),
  completeExternalTask: vi.fn(),
  listPendingExternalTasks: vi.fn(),
  setPermissionPolicy: vi.fn(),
  getPermissionPolicy: vi.fn(),
  checkPermission: vi.fn(),
  addPermissionRule: vi.fn(),
  getTodos: vi.fn(),
  setTodos: vi.fn(),
  listProviders: vi.fn(),
  getProvider: vi.fn(),
  addProvider: vi.fn(),
  updateProvider: vi.fn(),
  removeProvider: vi.fn(),
  setDefaultModel: vi.fn(),
  getDefaultModel: vi.fn(),
};

// Import after mocking
import { A3sClient } from '../client.js';
import type {
  HealthCheckResponse,
  GetCapabilitiesResponse,
  InitializeResponse,
  ShutdownResponse,
  CreateSessionResponse,
  DestroySessionResponse,
  ListSessionsResponse,
  GetSessionResponse,
  ConfigureSessionResponse,
  GetMessagesResponse,
  GenerateResponse,
  LoadSkillResponse,
  UnloadSkillResponse,
  ListSkillsResponse,
  GetContextUsageResponse,
  CompactContextResponse,
  ClearContextResponse,
  CancelResponse,
  PauseResponse,
  ResumeResponse,
  ConfirmToolExecutionResponse,
  SetConfirmationPolicyResponse,
  GetConfirmationPolicyResponse,
  SetLaneHandlerResponse,
  GetLaneHandlerResponse,
  CompleteExternalTaskResponse,
  ListPendingExternalTasksResponse,
  SetPermissionPolicyResponse,
  GetPermissionPolicyResponse,
  CheckPermissionResponse,
  AddPermissionRuleResponse,
  GetTodosResponse,
  SetTodosResponse,
  ListProvidersResponse,
  GetProviderResponse,
  AddProviderResponse,
  UpdateProviderResponse,
  RemoveProviderResponse,
  SetDefaultModelResponse,
  GetDefaultModelResponse,
} from '../client.js';

describe('A3sClient', () => {
  let client: A3sClient;

  beforeEach(() => {
    // Reset all mocks
    vi.clearAllMocks();
    client = new A3sClient({ address: 'localhost:4088' });
  });

  describe('constructor', () => {
    it('should create client with default address', () => {
      const defaultClient = new A3sClient();
      expect(defaultClient).toBeDefined();
    });

    it('should create client with custom address', () => {
      const customClient = new A3sClient({ address: 'custom:9999' });
      expect(customClient).toBeDefined();
    });

    it('should create client with TLS', () => {
      const tlsClient = new A3sClient({ useTls: true });
      expect(tlsClient).toBeDefined();
    });
  });

  describe('close', () => {
    it('should close the client connection', () => {
      client.close();
      expect(mockGrpcClient.close).toHaveBeenCalled();
    });
  });

  // ==========================================================================
  // Lifecycle Management Tests
  // ==========================================================================

  describe('healthCheck', () => {
    it('should return health status', async () => {
      const mockResponse: HealthCheckResponse = {
        status: 'STATUS_HEALTHY',
        message: 'OK',
        details: {},
      };
      mockGrpcClient.healthCheck.mockImplementation(
        (_req: unknown, callback: (err: null, res: HealthCheckResponse) => void) => {
          callback(null, mockResponse);
        }
      );

      const result = await client.healthCheck();
      expect(result).toEqual(mockResponse);
      expect(mockGrpcClient.healthCheck).toHaveBeenCalledWith({}, expect.any(Function));
    });

    it('should handle errors', async () => {
      const error = new Error('Connection failed');
      mockGrpcClient.healthCheck.mockImplementation(
        (_req: unknown, callback: (err: Error) => void) => {
          callback(error);
        }
      );

      await expect(client.healthCheck()).rejects.toThrow('Connection failed');
    });
  });

  describe('getCapabilities', () => {
    it('should return capabilities', async () => {
      const mockResponse: GetCapabilitiesResponse = {
        info: {
          name: 'test-agent',
          version: '1.0.0',
          description: 'Test agent',
          author: 'Test',
          license: 'MIT',
          homepage: 'https://example.com',
        },
        features: ['streaming', 'tools'],
        tools: [],
        models: [],
        metadata: {},
      };
      mockGrpcClient.getCapabilities.mockImplementation(
        (_req: unknown, callback: (err: null, res: GetCapabilitiesResponse) => void) => {
          callback(null, mockResponse);
        }
      );

      const result = await client.getCapabilities();
      expect(result).toEqual(mockResponse);
    });
  });

  describe('initialize', () => {
    it('should initialize with workspace', async () => {
      const mockResponse: InitializeResponse = {
        success: true,
        message: 'Initialized',
      };
      mockGrpcClient.initialize.mockImplementation(
        (_req: unknown, callback: (err: null, res: InitializeResponse) => void) => {
          callback(null, mockResponse);
        }
      );

      const result = await client.initialize('/workspace');
      expect(result).toEqual(mockResponse);
      expect(mockGrpcClient.initialize).toHaveBeenCalledWith(
        { workspace: '/workspace', env: {} },
        expect.any(Function)
      );
    });

    it('should initialize with environment variables', async () => {
      const mockResponse: InitializeResponse = {
        success: true,
        message: 'Initialized',
      };
      mockGrpcClient.initialize.mockImplementation(
        (_req: unknown, callback: (err: null, res: InitializeResponse) => void) => {
          callback(null, mockResponse);
        }
      );

      const result = await client.initialize('/workspace', { KEY: 'value' });
      expect(result).toEqual(mockResponse);
      expect(mockGrpcClient.initialize).toHaveBeenCalledWith(
        { workspace: '/workspace', env: { KEY: 'value' } },
        expect.any(Function)
      );
    });
  });

  describe('shutdown', () => {
    it('should shutdown the agent', async () => {
      const mockResponse: ShutdownResponse = {
        success: true,
        message: 'Shutdown complete',
      };
      mockGrpcClient.shutdown.mockImplementation(
        (_req: unknown, callback: (err: null, res: ShutdownResponse) => void) => {
          callback(null, mockResponse);
        }
      );

      const result = await client.shutdown();
      expect(result).toEqual(mockResponse);
    });
  });

  // ==========================================================================
  // Session Management Tests
  // ==========================================================================

  describe('createSession', () => {
    it('should create a session', async () => {
      const mockResponse: CreateSessionResponse = {
        sessionId: 'session-123',
      };
      mockGrpcClient.createSession.mockImplementation(
        (_req: unknown, callback: (err: null, res: CreateSessionResponse) => void) => {
          callback(null, mockResponse);
        }
      );

      const result = await client.createSession();
      expect(result).toEqual(mockResponse);
    });

    it('should create a session with config', async () => {
      const mockResponse: CreateSessionResponse = {
        sessionId: 'session-123',
      };
      mockGrpcClient.createSession.mockImplementation(
        (_req: unknown, callback: (err: null, res: CreateSessionResponse) => void) => {
          callback(null, mockResponse);
        }
      );

      const config = { name: 'test', workspace: '/workspace' };
      const result = await client.createSession(config, 'custom-id');
      expect(result).toEqual(mockResponse);
      expect(mockGrpcClient.createSession).toHaveBeenCalledWith(
        { sessionId: 'custom-id', config, initialContext: [] },
        expect.any(Function)
      );
    });
  });

  describe('destroySession', () => {
    it('should destroy a session', async () => {
      const mockResponse: DestroySessionResponse = { success: true };
      mockGrpcClient.destroySession.mockImplementation(
        (_req: unknown, callback: (err: null, res: DestroySessionResponse) => void) => {
          callback(null, mockResponse);
        }
      );

      const result = await client.destroySession('session-123');
      expect(result).toEqual(mockResponse);
    });
  });

  describe('listSessions', () => {
    it('should list sessions', async () => {
      const mockResponse: ListSessionsResponse = { sessions: [] };
      mockGrpcClient.listSessions.mockImplementation(
        (_req: unknown, callback: (err: null, res: ListSessionsResponse) => void) => {
          callback(null, mockResponse);
        }
      );

      const result = await client.listSessions();
      expect(result).toEqual(mockResponse);
    });
  });

  describe('getSession', () => {
    it('should get a session', async () => {
      const mockResponse: GetSessionResponse = {
        session: {
          sessionId: 'session-123',
          state: 'SESSION_STATE_ACTIVE',
          createdAt: Date.now(),
          updatedAt: Date.now(),
        },
      };
      mockGrpcClient.getSession.mockImplementation(
        (_req: unknown, callback: (err: null, res: GetSessionResponse) => void) => {
          callback(null, mockResponse);
        }
      );

      const result = await client.getSession('session-123');
      expect(result).toEqual(mockResponse);
    });
  });

  describe('configureSession', () => {
    it('should configure a session', async () => {
      const mockResponse: ConfigureSessionResponse = {};
      mockGrpcClient.configureSession.mockImplementation(
        (_req: unknown, callback: (err: null, res: ConfigureSessionResponse) => void) => {
          callback(null, mockResponse);
        }
      );

      const config = { name: 'updated', workspace: '/workspace' };
      const result = await client.configureSession('session-123', config);
      expect(result).toEqual(mockResponse);
    });
  });

  describe('getMessages', () => {
    it('should get messages', async () => {
      const mockResponse: GetMessagesResponse = {
        messages: [],
        totalCount: 0,
        hasMore: false,
      };
      mockGrpcClient.getMessages.mockImplementation(
        (_req: unknown, callback: (err: null, res: GetMessagesResponse) => void) => {
          callback(null, mockResponse);
        }
      );

      const result = await client.getMessages('session-123', 10, 0);
      expect(result).toEqual(mockResponse);
    });
  });

  // ==========================================================================
  // Code Generation Tests
  // ==========================================================================

  describe('generate', () => {
    it('should generate a response', async () => {
      const mockResponse: GenerateResponse = {
        sessionId: 'session-123',
        toolCalls: [],
        finishReason: 'stop',
        metadata: {},
      };
      mockGrpcClient.generate.mockImplementation(
        (_req: unknown, callback: (err: null, res: GenerateResponse) => void) => {
          callback(null, mockResponse);
        }
      );

      const messages = [{ role: 'user' as const, content: 'Hello' }];
      const result = await client.generate('session-123', messages);
      expect(result).toEqual(mockResponse);
    });
  });

  // ==========================================================================
  // Skill Management Tests
  // ==========================================================================

  describe('loadSkill', () => {
    it('should load a skill', async () => {
      const mockResponse: LoadSkillResponse = {
        success: true,
        toolNames: ['tool1', 'tool2'],
      };
      mockGrpcClient.loadSkill.mockImplementation(
        (_req: unknown, callback: (err: null, res: LoadSkillResponse) => void) => {
          callback(null, mockResponse);
        }
      );

      const result = await client.loadSkill('session-123', 'my-skill');
      expect(result).toEqual(mockResponse);
    });
  });

  describe('unloadSkill', () => {
    it('should unload a skill', async () => {
      const mockResponse: UnloadSkillResponse = {
        success: true,
        removedTools: ['tool1'],
      };
      mockGrpcClient.unloadSkill.mockImplementation(
        (_req: unknown, callback: (err: null, res: UnloadSkillResponse) => void) => {
          callback(null, mockResponse);
        }
      );

      const result = await client.unloadSkill('session-123', 'my-skill');
      expect(result).toEqual(mockResponse);
    });
  });

  describe('listSkills', () => {
    it('should list skills', async () => {
      const mockResponse: ListSkillsResponse = { skills: [] };
      mockGrpcClient.listSkills.mockImplementation(
        (_req: unknown, callback: (err: null, res: ListSkillsResponse) => void) => {
          callback(null, mockResponse);
        }
      );

      const result = await client.listSkills('session-123');
      expect(result).toEqual(mockResponse);
    });
  });

  // ==========================================================================
  // Context Management Tests
  // ==========================================================================

  describe('getContextUsage', () => {
    it('should get context usage', async () => {
      const mockResponse: GetContextUsageResponse = {
        usage: {
          totalTokens: 1000,
          promptTokens: 800,
          completionTokens: 200,
          messageCount: 5,
        },
      };
      mockGrpcClient.getContextUsage.mockImplementation(
        (_req: unknown, callback: (err: null, res: GetContextUsageResponse) => void) => {
          callback(null, mockResponse);
        }
      );

      const result = await client.getContextUsage('session-123');
      expect(result).toEqual(mockResponse);
    });
  });

  describe('compactContext', () => {
    it('should compact context', async () => {
      const mockResponse: CompactContextResponse = { success: true };
      mockGrpcClient.compactContext.mockImplementation(
        (_req: unknown, callback: (err: null, res: CompactContextResponse) => void) => {
          callback(null, mockResponse);
        }
      );

      const result = await client.compactContext('session-123');
      expect(result).toEqual(mockResponse);
    });
  });

  describe('clearContext', () => {
    it('should clear context', async () => {
      const mockResponse: ClearContextResponse = { success: true };
      mockGrpcClient.clearContext.mockImplementation(
        (_req: unknown, callback: (err: null, res: ClearContextResponse) => void) => {
          callback(null, mockResponse);
        }
      );

      const result = await client.clearContext('session-123');
      expect(result).toEqual(mockResponse);
    });
  });

  // ==========================================================================
  // Control Operations Tests
  // ==========================================================================

  describe('cancel', () => {
    it('should cancel an operation', async () => {
      const mockResponse: CancelResponse = { success: true };
      mockGrpcClient.cancel.mockImplementation(
        (_req: unknown, callback: (err: null, res: CancelResponse) => void) => {
          callback(null, mockResponse);
        }
      );

      const result = await client.cancel('session-123', 'op-456');
      expect(result).toEqual(mockResponse);
    });
  });

  describe('pause', () => {
    it('should pause a session', async () => {
      const mockResponse: PauseResponse = { success: true };
      mockGrpcClient.pause.mockImplementation(
        (_req: unknown, callback: (err: null, res: PauseResponse) => void) => {
          callback(null, mockResponse);
        }
      );

      const result = await client.pause('session-123');
      expect(result).toEqual(mockResponse);
    });
  });

  describe('resume', () => {
    it('should resume a session', async () => {
      const mockResponse: ResumeResponse = { success: true };
      mockGrpcClient.resume.mockImplementation(
        (_req: unknown, callback: (err: null, res: ResumeResponse) => void) => {
          callback(null, mockResponse);
        }
      );

      const result = await client.resume('session-123');
      expect(result).toEqual(mockResponse);
    });
  });

  // ==========================================================================
  // HITL Tests
  // ==========================================================================

  describe('confirmToolExecution', () => {
    it('should confirm tool execution', async () => {
      const mockResponse: ConfirmToolExecutionResponse = {
        success: true,
        error: '',
      };
      mockGrpcClient.confirmToolExecution.mockImplementation(
        (_req: unknown, callback: (err: null, res: ConfirmToolExecutionResponse) => void) => {
          callback(null, mockResponse);
        }
      );

      const result = await client.confirmToolExecution('session-123', 'tool-1', true);
      expect(result).toEqual(mockResponse);
    });
  });

  describe('setConfirmationPolicy', () => {
    it('should set confirmation policy', async () => {
      const mockResponse: SetConfirmationPolicyResponse = { success: true };
      mockGrpcClient.setConfirmationPolicy.mockImplementation(
        (_req: unknown, callback: (err: null, res: SetConfirmationPolicyResponse) => void) => {
          callback(null, mockResponse);
        }
      );

      const policy = {
        enabled: true,
        autoApproveTools: [],
        requireConfirmTools: ['Bash'],
        defaultTimeoutMs: 30000,
        timeoutAction: 'TIMEOUT_ACTION_REJECT' as const,
        yoloLanes: [],
      };
      const result = await client.setConfirmationPolicy('session-123', policy);
      expect(result).toEqual(mockResponse);
    });
  });

  describe('getConfirmationPolicy', () => {
    it('should get confirmation policy', async () => {
      const mockResponse: GetConfirmationPolicyResponse = {
        policy: {
          enabled: false,
          autoApproveTools: [],
          requireConfirmTools: [],
          defaultTimeoutMs: 30000,
          timeoutAction: 'TIMEOUT_ACTION_REJECT',
          yoloLanes: [],
        },
      };
      mockGrpcClient.getConfirmationPolicy.mockImplementation(
        (_req: unknown, callback: (err: null, res: GetConfirmationPolicyResponse) => void) => {
          callback(null, mockResponse);
        }
      );

      const result = await client.getConfirmationPolicy('session-123');
      expect(result).toEqual(mockResponse);
    });
  });

  // ==========================================================================
  // External Task Tests
  // ==========================================================================

  describe('setLaneHandler', () => {
    it('should set lane handler', async () => {
      const mockResponse: SetLaneHandlerResponse = { success: true };
      mockGrpcClient.setLaneHandler.mockImplementation(
        (_req: unknown, callback: (err: null, res: SetLaneHandlerResponse) => void) => {
          callback(null, mockResponse);
        }
      );

      const config = {
        mode: 'TASK_HANDLER_MODE_EXTERNAL' as const,
        timeoutMs: 60000,
      };
      const result = await client.setLaneHandler(
        'session-123',
        'SESSION_LANE_EXECUTE',
        config
      );
      expect(result).toEqual(mockResponse);
    });
  });

  describe('getLaneHandler', () => {
    it('should get lane handler', async () => {
      const mockResponse: GetLaneHandlerResponse = {
        config: {
          mode: 'TASK_HANDLER_MODE_INTERNAL',
          timeoutMs: 30000,
        },
      };
      mockGrpcClient.getLaneHandler.mockImplementation(
        (_req: unknown, callback: (err: null, res: GetLaneHandlerResponse) => void) => {
          callback(null, mockResponse);
        }
      );

      const result = await client.getLaneHandler('session-123', 'SESSION_LANE_QUERY');
      expect(result).toEqual(mockResponse);
    });
  });

  describe('completeExternalTask', () => {
    it('should complete external task', async () => {
      const mockResponse: CompleteExternalTaskResponse = {
        success: true,
        error: '',
      };
      mockGrpcClient.completeExternalTask.mockImplementation(
        (_req: unknown, callback: (err: null, res: CompleteExternalTaskResponse) => void) => {
          callback(null, mockResponse);
        }
      );

      const result = await client.completeExternalTask(
        'session-123',
        'task-456',
        true,
        '{"result": "ok"}'
      );
      expect(result).toEqual(mockResponse);
    });
  });

  describe('listPendingExternalTasks', () => {
    it('should list pending external tasks', async () => {
      const mockResponse: ListPendingExternalTasksResponse = { tasks: [] };
      mockGrpcClient.listPendingExternalTasks.mockImplementation(
        (_req: unknown, callback: (err: null, res: ListPendingExternalTasksResponse) => void) => {
          callback(null, mockResponse);
        }
      );

      const result = await client.listPendingExternalTasks('session-123');
      expect(result).toEqual(mockResponse);
    });
  });

  // ==========================================================================
  // Permission System Tests
  // ==========================================================================

  describe('setPermissionPolicy', () => {
    it('should set permission policy', async () => {
      const mockResponse: SetPermissionPolicyResponse = { success: true };
      mockGrpcClient.setPermissionPolicy.mockImplementation(
        (_req: unknown, callback: (err: null, res: SetPermissionPolicyResponse) => void) => {
          callback(null, mockResponse);
        }
      );

      const policy = {
        deny: [],
        allow: [{ rule: 'Read(*)' }],
        ask: [{ rule: 'Bash(*)' }],
        defaultDecision: 'PERMISSION_DECISION_ASK' as const,
        enabled: true,
      };
      const result = await client.setPermissionPolicy('session-123', policy);
      expect(result).toEqual(mockResponse);
    });
  });

  describe('getPermissionPolicy', () => {
    it('should get permission policy', async () => {
      const mockResponse: GetPermissionPolicyResponse = {
        policy: {
          deny: [],
          allow: [],
          ask: [],
          defaultDecision: 'PERMISSION_DECISION_ASK',
          enabled: false,
        },
      };
      mockGrpcClient.getPermissionPolicy.mockImplementation(
        (_req: unknown, callback: (err: null, res: GetPermissionPolicyResponse) => void) => {
          callback(null, mockResponse);
        }
      );

      const result = await client.getPermissionPolicy('session-123');
      expect(result).toEqual(mockResponse);
    });
  });

  describe('checkPermission', () => {
    it('should check permission', async () => {
      const mockResponse: CheckPermissionResponse = {
        decision: 'PERMISSION_DECISION_ALLOW',
        matchingRules: ['Read(*)'],
      };
      mockGrpcClient.checkPermission.mockImplementation(
        (_req: unknown, callback: (err: null, res: CheckPermissionResponse) => void) => {
          callback(null, mockResponse);
        }
      );

      const result = await client.checkPermission(
        'session-123',
        'Read',
        '{"path": "/tmp/file.txt"}'
      );
      expect(result).toEqual(mockResponse);
    });
  });

  describe('addPermissionRule', () => {
    it('should add permission rule', async () => {
      const mockResponse: AddPermissionRuleResponse = {
        success: true,
        error: '',
      };
      mockGrpcClient.addPermissionRule.mockImplementation(
        (_req: unknown, callback: (err: null, res: AddPermissionRuleResponse) => void) => {
          callback(null, mockResponse);
        }
      );

      const result = await client.addPermissionRule(
        'session-123',
        'allow',
        'Read(src/**/*.ts)'
      );
      expect(result).toEqual(mockResponse);
    });
  });

  // ==========================================================================
  // Todo Tests
  // ==========================================================================

  describe('getTodos', () => {
    it('should get todos', async () => {
      const mockResponse: GetTodosResponse = {
        todos: [
          { id: '1', content: 'Task 1', status: 'pending', priority: 'high' },
        ],
      };
      mockGrpcClient.getTodos.mockImplementation(
        (_req: unknown, callback: (err: null, res: GetTodosResponse) => void) => {
          callback(null, mockResponse);
        }
      );

      const result = await client.getTodos('session-123');
      expect(result).toEqual(mockResponse);
    });
  });

  describe('setTodos', () => {
    it('should set todos', async () => {
      const todos = [
        { id: '1', content: 'Task 1', status: 'completed', priority: 'high' },
      ];
      const mockResponse: SetTodosResponse = { success: true, todos };
      mockGrpcClient.setTodos.mockImplementation(
        (_req: unknown, callback: (err: null, res: SetTodosResponse) => void) => {
          callback(null, mockResponse);
        }
      );

      const result = await client.setTodos('session-123', todos);
      expect(result).toEqual(mockResponse);
    });
  });

  // ==========================================================================
  // Provider Configuration Tests
  // ==========================================================================

  describe('listProviders', () => {
    it('should list providers', async () => {
      const mockResponse: ListProvidersResponse = {
        providers: [
          { name: 'anthropic', models: [] },
        ],
      };
      mockGrpcClient.listProviders.mockImplementation(
        (_req: unknown, callback: (err: null, res: ListProvidersResponse) => void) => {
          callback(null, mockResponse);
        }
      );

      const result = await client.listProviders();
      expect(result).toEqual(mockResponse);
    });
  });

  describe('getProvider', () => {
    it('should get a provider', async () => {
      const mockResponse: GetProviderResponse = {
        provider: { name: 'anthropic', models: [] },
      };
      mockGrpcClient.getProvider.mockImplementation(
        (_req: unknown, callback: (err: null, res: GetProviderResponse) => void) => {
          callback(null, mockResponse);
        }
      );

      const result = await client.getProvider('anthropic');
      expect(result).toEqual(mockResponse);
    });
  });

  describe('addProvider', () => {
    it('should add a provider', async () => {
      const provider = { name: 'custom', models: [] };
      const mockResponse: AddProviderResponse = {
        success: true,
        error: '',
        provider,
      };
      mockGrpcClient.addProvider.mockImplementation(
        (_req: unknown, callback: (err: null, res: AddProviderResponse) => void) => {
          callback(null, mockResponse);
        }
      );

      const result = await client.addProvider(provider);
      expect(result).toEqual(mockResponse);
    });
  });

  describe('updateProvider', () => {
    it('should update a provider', async () => {
      const provider = { name: 'anthropic', apiKey: 'new-key', models: [] };
      const mockResponse: UpdateProviderResponse = {
        success: true,
        error: '',
        provider,
      };
      mockGrpcClient.updateProvider.mockImplementation(
        (_req: unknown, callback: (err: null, res: UpdateProviderResponse) => void) => {
          callback(null, mockResponse);
        }
      );

      const result = await client.updateProvider(provider);
      expect(result).toEqual(mockResponse);
    });
  });

  describe('removeProvider', () => {
    it('should remove a provider', async () => {
      const mockResponse: RemoveProviderResponse = {
        success: true,
        error: '',
      };
      mockGrpcClient.removeProvider.mockImplementation(
        (_req: unknown, callback: (err: null, res: RemoveProviderResponse) => void) => {
          callback(null, mockResponse);
        }
      );

      const result = await client.removeProvider('custom');
      expect(result).toEqual(mockResponse);
    });
  });

  describe('setDefaultModel', () => {
    it('should set default model', async () => {
      const mockResponse: SetDefaultModelResponse = {
        success: true,
        error: '',
        provider: 'anthropic',
        model: 'claude-sonnet-4-20250514',
      };
      mockGrpcClient.setDefaultModel.mockImplementation(
        (_req: unknown, callback: (err: null, res: SetDefaultModelResponse) => void) => {
          callback(null, mockResponse);
        }
      );

      const result = await client.setDefaultModel('anthropic', 'claude-sonnet-4-20250514');
      expect(result).toEqual(mockResponse);
    });
  });

  describe('getDefaultModel', () => {
    it('should get default model', async () => {
      const mockResponse: GetDefaultModelResponse = {
        provider: 'anthropic',
        model: 'claude-sonnet-4-20250514',
      };
      mockGrpcClient.getDefaultModel.mockImplementation(
        (_req: unknown, callback: (err: null, res: GetDefaultModelResponse) => void) => {
          callback(null, mockResponse);
        }
      );

      const result = await client.getDefaultModel();
      expect(result).toEqual(mockResponse);
    });
  });
});
