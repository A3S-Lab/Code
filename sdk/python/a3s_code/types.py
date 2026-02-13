"""
Type definitions for A3S Code Agent SDK
"""

from dataclasses import dataclass, field
from enum import Enum
from typing import Optional, List, Dict


# ============================================================================
# Enums
# ============================================================================


class HealthStatus(Enum):
    UNKNOWN = "STATUS_UNKNOWN"
    HEALTHY = "STATUS_HEALTHY"
    DEGRADED = "STATUS_DEGRADED"
    UNHEALTHY = "STATUS_UNHEALTHY"


class HealthStatusCode(Enum):
    """Health check status code (numeric)."""

    UNKNOWN = 0
    HEALTHY = 1
    DEGRADED = 2
    UNHEALTHY = 3


class SessionState(Enum):
    UNKNOWN = "SESSION_STATE_UNKNOWN"
    ACTIVE = "SESSION_STATE_ACTIVE"
    PAUSED = "SESSION_STATE_PAUSED"
    COMPLETED = "SESSION_STATE_COMPLETED"
    ERROR = "SESSION_STATE_ERROR"


class MessageRole(Enum):
    """OpenAI-compatible message roles (proto uses string values directly)."""
    USER = "user"
    ASSISTANT = "assistant"
    SYSTEM = "system"
    TOOL = "tool"


class FinishReason(Enum):
    """OpenAI-compatible finish reasons (proto uses string values directly)."""
    STOP = "stop"
    LENGTH = "length"
    TOOL_CALLS = "tool_calls"
    CONTENT_FILTER = "content_filter"
    ERROR = "error"


class ChunkType(Enum):
    """OpenAI-compatible chunk types (proto uses string values directly)."""
    CONTENT = "content"
    TOOL_CALL = "tool_call"
    TOOL_RESULT = "tool_result"
    METADATA = "metadata"
    DONE = "done"


class EventType(Enum):
    UNKNOWN = "EVENT_TYPE_UNKNOWN"
    SESSION_CREATED = "EVENT_TYPE_SESSION_CREATED"
    SESSION_DESTROYED = "EVENT_TYPE_SESSION_DESTROYED"
    GENERATION_STARTED = "EVENT_TYPE_GENERATION_STARTED"
    GENERATION_COMPLETED = "EVENT_TYPE_GENERATION_COMPLETED"
    TOOL_CALLED = "EVENT_TYPE_TOOL_CALLED"
    TOOL_COMPLETED = "EVENT_TYPE_TOOL_COMPLETED"
    ERROR = "EVENT_TYPE_ERROR"
    WARNING = "EVENT_TYPE_WARNING"
    INFO = "EVENT_TYPE_INFO"
    CONFIRMATION_REQUIRED = "EVENT_TYPE_CONFIRMATION_REQUIRED"
    CONFIRMATION_RECEIVED = "EVENT_TYPE_CONFIRMATION_RECEIVED"
    CONFIRMATION_TIMEOUT = "EVENT_TYPE_CONFIRMATION_TIMEOUT"
    EXTERNAL_TASK_PENDING = "EVENT_TYPE_EXTERNAL_TASK_PENDING"
    EXTERNAL_TASK_COMPLETED = "EVENT_TYPE_EXTERNAL_TASK_COMPLETED"
    PERMISSION_DENIED = "EVENT_TYPE_PERMISSION_DENIED"
    MEMORY_STORED = "EVENT_TYPE_MEMORY_STORED"
    MEMORY_RECALLED = "EVENT_TYPE_MEMORY_RECALLED"
    MEMORIES_SEARCHED = "EVENT_TYPE_MEMORIES_SEARCHED"
    MEMORY_CLEARED = "EVENT_TYPE_MEMORY_CLEARED"


class SessionLane(Enum):
    UNKNOWN = "SESSION_LANE_UNKNOWN"
    CONTROL = "SESSION_LANE_CONTROL"
    QUERY = "SESSION_LANE_QUERY"
    EXECUTE = "SESSION_LANE_EXECUTE"
    GENERATE = "SESSION_LANE_GENERATE"


class TimeoutAction(Enum):
    UNKNOWN = "TIMEOUT_ACTION_UNKNOWN"
    REJECT = "TIMEOUT_ACTION_REJECT"
    AUTO_APPROVE = "TIMEOUT_ACTION_AUTO_APPROVE"


class TaskHandlerMode(Enum):
    UNKNOWN = "TASK_HANDLER_MODE_UNKNOWN"
    INTERNAL = "TASK_HANDLER_MODE_INTERNAL"
    EXTERNAL = "TASK_HANDLER_MODE_EXTERNAL"
    HYBRID = "TASK_HANDLER_MODE_HYBRID"


class PermissionDecision(Enum):
    UNKNOWN = "PERMISSION_DECISION_UNKNOWN"
    ALLOW = "PERMISSION_DECISION_ALLOW"
    DENY = "PERMISSION_DECISION_DENY"
    ASK = "PERMISSION_DECISION_ASK"


# ============================================================================
# Lifecycle Types
# ============================================================================


@dataclass
class AgentInfo:
    name: str = ""
    version: str = ""
    description: str = ""
    author: str = ""
    license: str = ""
    homepage: str = ""


@dataclass
class ToolCapability:
    name: str = ""
    description: str = ""
    parameters: List[str] = field(default_factory=list)
    is_async: bool = False


@dataclass
class ModelCapability:
    provider: str = ""
    model: str = ""
    features: List[str] = field(default_factory=list)


@dataclass
class ResourceLimits:
    max_context_tokens: int = 0
    max_concurrent_sessions: int = 0
    max_tools_per_request: int = 0


# ============================================================================
# Session Types
# ============================================================================


class StorageType(Enum):
    """Storage backend type for session persistence."""
    UNSPECIFIED = "STORAGE_TYPE_UNSPECIFIED"
    MEMORY = "STORAGE_TYPE_MEMORY"
    FILE = "STORAGE_TYPE_FILE"

    # Aliases for proto-style naming (used by examples and TS SDK parity)
    STORAGE_TYPE_UNSPECIFIED = "STORAGE_TYPE_UNSPECIFIED"
    STORAGE_TYPE_MEMORY = "STORAGE_TYPE_MEMORY"
    STORAGE_TYPE_FILE = "STORAGE_TYPE_FILE"


@dataclass
class LLMConfig:
    provider: str = ""
    model: str = ""
    api_key: Optional[str] = None
    base_url: Optional[str] = None
    temperature: Optional[float] = None
    max_tokens: Optional[int] = None


@dataclass
class SessionConfig:
    name: str = ""
    workspace: str = ""
    system_prompt: Optional[str] = None
    max_context_length: Optional[int] = None
    auto_compact: Optional[bool] = None
    storage_type: Optional[StorageType] = None


@dataclass
class ContextUsage:
    total_tokens: int = 0
    prompt_tokens: int = 0
    completion_tokens: int = 0
    message_count: int = 0


@dataclass
class Session:
    session_id: str = ""
    config: Optional[SessionConfig] = None
    state: SessionState = SessionState.UNKNOWN
    context_usage: Optional[ContextUsage] = None
    created_at: int = 0
    updated_at: int = 0


# ============================================================================
# Message Types
# ============================================================================


@dataclass
class Message:
    role: MessageRole = MessageRole.USER
    content: str = ""
    metadata: Dict[str, str] = field(default_factory=dict)


@dataclass
class TextContent:
    text: str = ""


@dataclass
class ToolUseContent:
    id: str = ""
    name: str = ""
    arguments: str = ""


@dataclass
class ToolResultContent:
    tool_use_id: str = ""
    content: str = ""
    is_error: bool = False


@dataclass
class ContentBlock:
    text: Optional[TextContent] = None
    tool_use: Optional[ToolUseContent] = None
    tool_result: Optional[ToolResultContent] = None


@dataclass
class ConversationMessage:
    id: str = ""
    role: str = ""
    content: List[ContentBlock] = field(default_factory=list)
    timestamp: int = 0
    metadata: Dict[str, str] = field(default_factory=dict)


# ============================================================================
# Generation Types
# ============================================================================


@dataclass
class ToolResult:
    success: bool = False
    output: str = ""
    error: str = ""
    metadata: Dict[str, str] = field(default_factory=dict)


@dataclass
class ToolCall:
    id: str = ""
    name: str = ""
    arguments: str = ""
    result: Optional[ToolResult] = None


@dataclass
class Usage:
    prompt_tokens: int = 0
    completion_tokens: int = 0
    total_tokens: int = 0


@dataclass
class GenerateResponse:
    """Response from a generate request."""

    session_id: str = ""
    message: Optional[Message] = None
    tool_calls: List[ToolCall] = field(default_factory=list)
    usage: Optional[Usage] = None
    finish_reason: FinishReason = FinishReason.STOP
    metadata: Dict[str, str] = field(default_factory=dict)


@dataclass
class GenerateChunk:
    """A streaming chunk from generate."""

    type: ChunkType = ChunkType.CONTENT
    session_id: str = ""
    content: str = ""
    tool_call: Optional[ToolCall] = None
    tool_result: Optional[ToolResult] = None
    metadata: Dict[str, str] = field(default_factory=dict)


# ============================================================================
# Skill Types
# ============================================================================


@dataclass
class Skill:
    name: str = ""
    description: str = ""
    tools: List[str] = field(default_factory=list)
    metadata: Dict[str, str] = field(default_factory=dict)


# ============================================================================
# Event Types
# ============================================================================


@dataclass
class AgentEvent:
    type: EventType = EventType.UNKNOWN
    session_id: Optional[str] = None
    timestamp: int = 0
    message: str = ""
    data: Dict[str, str] = field(default_factory=dict)


# ============================================================================
# HITL Types
# ============================================================================


@dataclass
class ConfirmationPolicy:
    enabled: bool = False
    auto_approve_tools: List[str] = field(default_factory=list)
    require_confirm_tools: List[str] = field(default_factory=list)
    default_timeout_ms: int = 30000
    timeout_action: TimeoutAction = TimeoutAction.REJECT
    yolo_lanes: List[SessionLane] = field(default_factory=list)


# ============================================================================
# External Task Types
# ============================================================================


@dataclass
class LaneHandlerConfig:
    mode: TaskHandlerMode = TaskHandlerMode.INTERNAL
    timeout_ms: int = 0


@dataclass
class ExternalTask:
    task_id: str = ""
    session_id: str = ""
    lane: SessionLane = SessionLane.UNKNOWN
    command_type: str = ""
    payload: str = ""
    timeout_ms: int = 0
    remaining_ms: int = 0


# ============================================================================
# Permission Types
# ============================================================================


@dataclass
class PermissionRule:
    rule: str = ""


@dataclass
class PermissionPolicy:
    deny: List[PermissionRule] = field(default_factory=list)
    allow: List[PermissionRule] = field(default_factory=list)
    ask: List[PermissionRule] = field(default_factory=list)
    default_decision: PermissionDecision = PermissionDecision.ASK
    enabled: bool = False


# ============================================================================
# Todo Types
# ============================================================================


@dataclass
class Todo:
    id: str = ""
    content: str = ""
    status: str = "pending"
    priority: str = "medium"


# ============================================================================
# Provider Types
# ============================================================================


@dataclass
class ModelCostInfo:
    input: float = 0.0
    output: float = 0.0
    cache_read: float = 0.0
    cache_write: float = 0.0


@dataclass
class ModelLimitInfo:
    context: int = 0
    output: int = 0


@dataclass
class ModelModalitiesInfo:
    input: List[str] = field(default_factory=list)
    output: List[str] = field(default_factory=list)


@dataclass
class ModelInfo:
    id: str = ""
    name: str = ""
    family: str = ""
    api_key: Optional[str] = None
    base_url: Optional[str] = None
    attachment: bool = False
    reasoning: bool = False
    tool_call: bool = True
    temperature: bool = True
    release_date: Optional[str] = None
    modalities: Optional[ModelModalitiesInfo] = None
    cost: Optional[ModelCostInfo] = None
    limit: Optional[ModelLimitInfo] = None


@dataclass
class ProviderInfo:
    name: str = ""
    api_key: Optional[str] = None
    base_url: Optional[str] = None
    models: List[ModelInfo] = field(default_factory=list)


# ============================================================================
# Skill Types (Extended)
# ============================================================================


@dataclass
class ClaudeCodeSkill:
    name: str = ""
    description: str = ""
    allowed_tools: Optional[str] = None
    disable_model_invocation: bool = False
    content: str = ""


# ============================================================================
# Planning & Goal Tracking Types
# ============================================================================


class Complexity(Enum):
    UNKNOWN = "COMPLEXITY_UNKNOWN"
    SIMPLE = "COMPLEXITY_SIMPLE"
    MEDIUM = "COMPLEXITY_MEDIUM"
    COMPLEX = "COMPLEXITY_COMPLEX"
    VERY_COMPLEX = "COMPLEXITY_VERY_COMPLEX"


class StepStatus(Enum):
    UNKNOWN = "STEP_STATUS_UNKNOWN"
    PENDING = "STEP_STATUS_PENDING"
    IN_PROGRESS = "STEP_STATUS_IN_PROGRESS"
    COMPLETED = "STEP_STATUS_COMPLETED"
    FAILED = "STEP_STATUS_FAILED"
    SKIPPED = "STEP_STATUS_SKIPPED"


@dataclass
class PlanStep:
    id: str = ""
    description: str = ""
    tool: Optional[str] = None
    dependencies: List[str] = field(default_factory=list)
    status: StepStatus = StepStatus.PENDING
    success_criteria: List[str] = field(default_factory=list)


@dataclass
class ExecutionPlan:
    goal: str = ""
    steps: List[PlanStep] = field(default_factory=list)
    complexity: Complexity = Complexity.UNKNOWN
    required_tools: List[str] = field(default_factory=list)
    estimated_steps: int = 0


@dataclass
class AgentGoal:
    description: str = ""
    success_criteria: List[str] = field(default_factory=list)
    progress: float = 0.0
    achieved: bool = False
    created_at: int = 0
    achieved_at: Optional[int] = None


# ============================================================================
# Memory System Types
# ============================================================================


class MemoryType(Enum):
    UNKNOWN = "MEMORY_TYPE_UNKNOWN"
    EPISODIC = "MEMORY_TYPE_EPISODIC"
    SEMANTIC = "MEMORY_TYPE_SEMANTIC"
    PROCEDURAL = "MEMORY_TYPE_PROCEDURAL"
    WORKING = "MEMORY_TYPE_WORKING"


@dataclass
class MemoryItem:
    id: str = ""
    content: str = ""
    timestamp: int = 0
    importance: float = 0.0
    tags: List[str] = field(default_factory=list)
    memory_type: MemoryType = MemoryType.UNKNOWN
    metadata: Dict[str, str] = field(default_factory=dict)
    access_count: int = 0
    last_accessed: Optional[int] = None


@dataclass
class MemoryStats:
    long_term_count: int = 0
    short_term_count: int = 0
    working_count: int = 0


# ============================================================================
# MCP (Model Context Protocol) Types
# ============================================================================


@dataclass
class McpStdioTransport:
    command: str = ""
    args: List[str] = field(default_factory=list)


@dataclass
class McpHttpTransport:
    url: str = ""
    headers: Dict[str, str] = field(default_factory=dict)


@dataclass
class McpTransport:
    stdio: Optional[McpStdioTransport] = None
    http: Optional[McpHttpTransport] = None


@dataclass
class McpServerConfig:
    name: str = ""
    transport: Optional[McpTransport] = None
    enabled: bool = True
    env: Dict[str, str] = field(default_factory=dict)


@dataclass
class McpServerInfo:
    name: str = ""
    connected: bool = False
    enabled: bool = True
    tool_count: int = 0
    error: Optional[str] = None


@dataclass
class McpToolInfo:
    full_name: str = ""
    server_name: str = ""
    tool_name: str = ""
    description: str = ""
    input_schema: str = ""


# ============================================================================
# Cron (Scheduled Tasks) Types
# ============================================================================


class CronJobStatus(Enum):
    """Cron job status"""
    UNKNOWN = 0
    ACTIVE = 1
    PAUSED = 2
    RUNNING = 3


class CronExecutionStatus(Enum):
    """Cron execution status"""
    UNKNOWN = 0
    SUCCESS = 1
    FAILED = 2
    TIMEOUT = 3
    CANCELLED = 4


@dataclass
class CronJob:
    """Cron job definition"""
    id: str = ""
    name: str = ""
    schedule: str = ""
    command: str = ""
    status: CronJobStatus = CronJobStatus.UNKNOWN
    timeout_ms: int = 60000
    created_at: int = 0
    updated_at: int = 0
    last_run: Optional[int] = None
    next_run: Optional[int] = None
    run_count: int = 0
    fail_count: int = 0
    working_dir: Optional[str] = None


@dataclass
class CronExecution:
    """Cron job execution record"""
    id: str = ""
    job_id: str = ""
    status: CronExecutionStatus = CronExecutionStatus.UNKNOWN
    started_at: int = 0
    ended_at: Optional[int] = None
    duration_ms: Optional[int] = None
    exit_code: Optional[int] = None
    stdout: str = ""
    stderr: str = ""
    error: Optional[str] = None
