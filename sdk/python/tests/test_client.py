"""Unit tests for A3S Code SDK types and client."""

from a3s_code import (
    A3sClient,
    # Enums
    HealthStatus,
    HealthStatusCode,
    SessionState,
    MessageRole,
    FinishReason,
    ChunkType,
    EventType,
    SessionLane,
    TimeoutAction,
    TaskHandlerMode,
    PermissionDecision,
    # Types
    AgentInfo,
    LLMConfig,
    SessionConfig,
    ContextUsage,
    Session,
    Message,
    ToolResult,
    ToolCall,
    Usage,
    GenerateResponse,
    GenerateChunk,
    Skill,
    AgentEvent,
    ConfirmationPolicy,
    LaneHandlerConfig,
    ExternalTask,
    PermissionRule,
    PermissionPolicy,
    Todo,
)


# =============================================================================
# Enum Tests
# =============================================================================


class TestEnums:
    """Tests for enum types."""

    def test_health_status_values(self):
        """Test HealthStatus enum values."""
        assert HealthStatus.UNKNOWN.value == "STATUS_UNKNOWN"
        assert HealthStatus.HEALTHY.value == "STATUS_HEALTHY"
        assert HealthStatus.DEGRADED.value == "STATUS_DEGRADED"
        assert HealthStatus.UNHEALTHY.value == "STATUS_UNHEALTHY"

    def test_health_status_code_values(self):
        """Test HealthStatusCode enum values."""
        assert HealthStatusCode.UNKNOWN.value == 0
        assert HealthStatusCode.HEALTHY.value == 1
        assert HealthStatusCode.DEGRADED.value == 2
        assert HealthStatusCode.UNHEALTHY.value == 3

    def test_session_state_values(self):
        """Test SessionState enum values."""
        assert SessionState.UNKNOWN.value == "SESSION_STATE_UNKNOWN"
        assert SessionState.ACTIVE.value == "SESSION_STATE_ACTIVE"
        assert SessionState.PAUSED.value == "SESSION_STATE_PAUSED"
        assert SessionState.COMPLETED.value == "SESSION_STATE_COMPLETED"
        assert SessionState.ERROR.value == "SESSION_STATE_ERROR"

    def test_message_role_values(self):
        """Test MessageRole enum values (OpenAI-compatible strings)."""
        assert MessageRole.USER.value == "user"
        assert MessageRole.ASSISTANT.value == "assistant"
        assert MessageRole.SYSTEM.value == "system"
        assert MessageRole.TOOL.value == "tool"

    def test_finish_reason_values(self):
        """Test FinishReason enum values (OpenAI-compatible strings)."""
        assert FinishReason.STOP.value == "stop"
        assert FinishReason.LENGTH.value == "length"
        assert FinishReason.TOOL_CALLS.value == "tool_calls"
        assert FinishReason.ERROR.value == "error"

    def test_chunk_type_values(self):
        """Test ChunkType enum values (OpenAI-compatible strings)."""
        assert ChunkType.CONTENT.value == "content"
        assert ChunkType.TOOL_CALL.value == "tool_call"
        assert ChunkType.DONE.value == "done"

    def test_event_type_values(self):
        """Test EventType enum values."""
        assert EventType.UNKNOWN.value == "EVENT_TYPE_UNKNOWN"
        assert EventType.SESSION_CREATED.value == "EVENT_TYPE_SESSION_CREATED"
        assert EventType.TOOL_CALLED.value == "EVENT_TYPE_TOOL_CALLED"
        assert EventType.ERROR.value == "EVENT_TYPE_ERROR"
        # Memory events (Phase 3)
        assert EventType.MEMORY_STORED.value == "EVENT_TYPE_MEMORY_STORED"
        assert EventType.MEMORY_RECALLED.value == "EVENT_TYPE_MEMORY_RECALLED"
        assert EventType.MEMORIES_SEARCHED.value == "EVENT_TYPE_MEMORIES_SEARCHED"
        assert EventType.MEMORY_CLEARED.value == "EVENT_TYPE_MEMORY_CLEARED"

    def test_session_lane_values(self):
        """Test SessionLane enum values."""
        assert SessionLane.UNKNOWN.value == "SESSION_LANE_UNKNOWN"
        assert SessionLane.CONTROL.value == "SESSION_LANE_CONTROL"
        assert SessionLane.QUERY.value == "SESSION_LANE_QUERY"
        assert SessionLane.EXECUTE.value == "SESSION_LANE_EXECUTE"
        assert SessionLane.GENERATE.value == "SESSION_LANE_GENERATE"

    def test_timeout_action_values(self):
        """Test TimeoutAction enum values."""
        assert TimeoutAction.UNKNOWN.value == "TIMEOUT_ACTION_UNKNOWN"
        assert TimeoutAction.REJECT.value == "TIMEOUT_ACTION_REJECT"
        assert TimeoutAction.AUTO_APPROVE.value == "TIMEOUT_ACTION_AUTO_APPROVE"

    def test_task_handler_mode_values(self):
        """Test TaskHandlerMode enum values."""
        assert TaskHandlerMode.UNKNOWN.value == "TASK_HANDLER_MODE_UNKNOWN"
        assert TaskHandlerMode.INTERNAL.value == "TASK_HANDLER_MODE_INTERNAL"
        assert TaskHandlerMode.EXTERNAL.value == "TASK_HANDLER_MODE_EXTERNAL"
        assert TaskHandlerMode.HYBRID.value == "TASK_HANDLER_MODE_HYBRID"

    def test_permission_decision_values(self):
        """Test PermissionDecision enum values."""
        assert PermissionDecision.UNKNOWN.value == "PERMISSION_DECISION_UNKNOWN"
        assert PermissionDecision.ALLOW.value == "PERMISSION_DECISION_ALLOW"
        assert PermissionDecision.DENY.value == "PERMISSION_DECISION_DENY"
        assert PermissionDecision.ASK.value == "PERMISSION_DECISION_ASK"


# =============================================================================
# Dataclass Tests
# =============================================================================


class TestDataclasses:
    """Tests for dataclass types."""

    def test_usage_defaults(self):
        """Test Usage dataclass defaults."""
        usage = Usage()
        assert usage.prompt_tokens == 0
        assert usage.completion_tokens == 0
        assert usage.total_tokens == 0

    def test_usage_with_values(self):
        """Test Usage dataclass with values."""
        usage = Usage(prompt_tokens=100, completion_tokens=50, total_tokens=150)
        assert usage.prompt_tokens == 100
        assert usage.completion_tokens == 50
        assert usage.total_tokens == 150

    def test_context_usage_defaults(self):
        """Test ContextUsage dataclass defaults."""
        ctx = ContextUsage()
        assert ctx.total_tokens == 0
        assert ctx.prompt_tokens == 0
        assert ctx.completion_tokens == 0
        assert ctx.message_count == 0

    def test_tool_result_defaults(self):
        """Test ToolResult dataclass defaults."""
        result = ToolResult()
        assert result.success is False
        assert result.output == ""
        assert result.error == ""
        assert result.metadata == {}

    def test_tool_result_with_values(self):
        """Test ToolResult dataclass with values."""
        result = ToolResult(
            success=True,
            output="Hello, World!",
            error="",
            metadata={"key": "value"},
        )
        assert result.success is True
        assert result.output == "Hello, World!"
        assert result.metadata["key"] == "value"

    def test_tool_call_defaults(self):
        """Test ToolCall dataclass defaults."""
        call = ToolCall()
        assert call.id == ""
        assert call.name == ""
        assert call.arguments == ""
        assert call.result is None

    def test_tool_call_with_result(self):
        """Test ToolCall dataclass with result."""
        result = ToolResult(success=True, output="done")
        call = ToolCall(
            id="call-123",
            name="bash",
            arguments='{"command": "ls"}',
            result=result,
        )
        assert call.id == "call-123"
        assert call.name == "bash"
        assert call.result is not None
        assert call.result.success is True

    def test_message_defaults(self):
        """Test Message dataclass defaults."""
        msg = Message()
        assert msg.role == MessageRole.USER
        assert msg.content == ""
        assert msg.metadata == {}

    def test_message_with_values(self):
        """Test Message dataclass with values."""
        msg = Message(
            role=MessageRole.USER,
            content="Hello!",
            metadata={"source": "test"},
        )
        assert msg.role == MessageRole.USER
        assert msg.content == "Hello!"
        assert msg.metadata["source"] == "test"

    def test_generate_response_defaults(self):
        """Test GenerateResponse dataclass defaults."""
        resp = GenerateResponse()
        assert resp.session_id == ""
        assert resp.message is None
        assert resp.tool_calls == []
        assert resp.usage is None
        assert resp.finish_reason == FinishReason.STOP
        assert resp.metadata == {}

    def test_generate_chunk_defaults(self):
        """Test GenerateChunk dataclass defaults."""
        chunk = GenerateChunk()
        assert chunk.type == ChunkType.CONTENT
        assert chunk.session_id == ""
        assert chunk.content == ""
        assert chunk.tool_call is None
        assert chunk.tool_result is None

    def test_llm_config_defaults(self):
        """Test LLMConfig dataclass defaults."""
        config = LLMConfig()
        assert config.provider == ""
        assert config.model == ""
        assert config.api_key is None
        assert config.base_url is None

    def test_llm_config_with_values(self):
        """Test LLMConfig dataclass with values."""
        config = LLMConfig(
            provider="anthropic",
            model="claude-sonnet-4",
            api_key="sk-xxx",
            temperature=0.7,
        )
        assert config.provider == "anthropic"
        assert config.model == "claude-sonnet-4"
        assert config.api_key == "sk-xxx"
        assert config.temperature == 0.7

    def test_session_config_defaults(self):
        """Test SessionConfig dataclass defaults."""
        config = SessionConfig()
        assert config.name == ""
        assert config.workspace == ""
        assert config.llm is None
        assert config.system_prompt is None

    def test_session_defaults(self):
        """Test Session dataclass defaults."""
        session = Session()
        assert session.session_id == ""
        assert session.config is None
        assert session.state == SessionState.UNKNOWN
        assert session.context_usage is None
        assert session.created_at == 0
        assert session.updated_at == 0

    def test_skill_defaults(self):
        """Test Skill dataclass defaults."""
        skill = Skill()
        assert skill.name == ""
        assert skill.description == ""
        assert skill.tools == []
        assert skill.metadata == {}

    def test_skill_with_values(self):
        """Test Skill dataclass with values."""
        skill = Skill(
            name="git",
            description="Git operations",
            tools=["git_status", "git_commit"],
            metadata={"version": "1.0"},
        )
        assert skill.name == "git"
        assert len(skill.tools) == 2
        assert "git_status" in skill.tools

    def test_agent_event_defaults(self):
        """Test AgentEvent dataclass defaults."""
        event = AgentEvent()
        assert event.type == EventType.UNKNOWN
        assert event.session_id is None
        assert event.timestamp == 0
        assert event.message == ""
        assert event.data == {}

    def test_confirmation_policy_defaults(self):
        """Test ConfirmationPolicy dataclass defaults."""
        policy = ConfirmationPolicy()
        assert policy.enabled is False
        assert policy.auto_approve_tools == []
        assert policy.require_confirm_tools == []
        assert policy.default_timeout_ms == 30000
        assert policy.timeout_action == TimeoutAction.REJECT
        assert policy.yolo_lanes == []

    def test_lane_handler_config_defaults(self):
        """Test LaneHandlerConfig dataclass defaults."""
        config = LaneHandlerConfig()
        assert config.mode == TaskHandlerMode.INTERNAL
        assert config.timeout_ms == 0

    def test_external_task_defaults(self):
        """Test ExternalTask dataclass defaults."""
        task = ExternalTask()
        assert task.task_id == ""
        assert task.session_id == ""
        assert task.lane == SessionLane.UNKNOWN
        assert task.command_type == ""
        assert task.payload == ""
        assert task.timeout_ms == 0
        assert task.remaining_ms == 0

    def test_permission_rule_defaults(self):
        """Test PermissionRule dataclass defaults."""
        rule = PermissionRule()
        assert rule.rule == ""

    def test_permission_rule_with_value(self):
        """Test PermissionRule dataclass with value."""
        rule = PermissionRule(rule="Bash(cargo:*)")
        assert rule.rule == "Bash(cargo:*)"

    def test_permission_policy_defaults(self):
        """Test PermissionPolicy dataclass defaults."""
        policy = PermissionPolicy()
        assert policy.deny == []
        assert policy.allow == []
        assert policy.ask == []
        assert policy.default_decision == PermissionDecision.ASK
        assert policy.enabled is False

    def test_todo_defaults(self):
        """Test Todo dataclass defaults."""
        todo = Todo()
        assert todo.id == ""
        assert todo.content == ""
        assert todo.status == "pending"
        assert todo.priority == "medium"

    def test_todo_with_values(self):
        """Test Todo dataclass with values."""
        todo = Todo(
            id="todo-1",
            content="Fix bug",
            status="in_progress",
            priority="high",
        )
        assert todo.id == "todo-1"
        assert todo.content == "Fix bug"
        assert todo.status == "in_progress"
        assert todo.priority == "high"

    def test_agent_info_defaults(self):
        """Test AgentInfo dataclass defaults."""
        info = AgentInfo()
        assert info.name == ""
        assert info.version == ""
        assert info.description == ""
        assert info.author == ""
        assert info.license == ""
        assert info.homepage == ""


# =============================================================================
# Client Tests
# =============================================================================


class TestA3sClient:
    """Tests for A3sClient class."""

    def test_client_initialization(self, client: A3sClient):
        """Test client initialization."""
        assert client.address == "localhost:50051"
        assert client.use_tls is False
        assert client._channel is None
        assert client._stub is None

    def test_client_initialization_with_tls(self):
        """Test client initialization with TLS."""
        client = A3sClient("localhost:50051", use_tls=True)
        assert client.use_tls is True

    def test_client_initialization_custom_address(self):
        """Test client initialization with custom address."""
        client = A3sClient("example.com:8080")
        assert client.address == "example.com:8080"

    def test_client_has_lifecycle_methods(self, client: A3sClient):
        """Test client has lifecycle methods."""
        assert hasattr(client, "health_check")
        assert hasattr(client, "get_capabilities")
        assert hasattr(client, "initialize")
        assert hasattr(client, "shutdown")
        assert callable(client.health_check)
        assert callable(client.get_capabilities)
        assert callable(client.initialize)
        assert callable(client.shutdown)

    def test_client_has_session_methods(self, client: A3sClient):
        """Test client has session management methods."""
        assert hasattr(client, "create_session")
        assert hasattr(client, "destroy_session")
        assert hasattr(client, "list_sessions")
        assert hasattr(client, "get_session")
        assert hasattr(client, "configure_session")
        assert hasattr(client, "get_messages")
        assert callable(client.create_session)
        assert callable(client.destroy_session)

    def test_client_has_generation_methods(self, client: A3sClient):
        """Test client has generation methods."""
        assert hasattr(client, "generate")
        assert hasattr(client, "stream_generate")
        assert hasattr(client, "generate_structured")
        assert hasattr(client, "stream_generate_structured")
        assert callable(client.generate)
        assert callable(client.stream_generate)

    def test_client_has_skill_methods(self, client: A3sClient):
        """Test client has skill management methods."""
        assert hasattr(client, "load_skill")
        assert hasattr(client, "unload_skill")
        assert hasattr(client, "list_skills")
        assert callable(client.load_skill)
        assert callable(client.unload_skill)

    def test_client_has_context_methods(self, client: A3sClient):
        """Test client has context management methods."""
        assert hasattr(client, "get_context_usage")
        assert hasattr(client, "compact_context")
        assert hasattr(client, "clear_context")
        assert callable(client.get_context_usage)
        assert callable(client.compact_context)

    def test_client_has_control_methods(self, client: A3sClient):
        """Test client has control methods."""
        assert hasattr(client, "cancel")
        assert hasattr(client, "pause")
        assert hasattr(client, "resume")
        assert callable(client.cancel)
        assert callable(client.pause)
        assert callable(client.resume)

    def test_client_has_event_methods(self, client: A3sClient):
        """Test client has event streaming methods."""
        assert hasattr(client, "subscribe_events")
        assert callable(client.subscribe_events)

    def test_client_has_hitl_methods(self, client: A3sClient):
        """Test client has HITL methods."""
        assert hasattr(client, "confirm_tool_execution")
        assert hasattr(client, "set_confirmation_policy")
        assert hasattr(client, "get_confirmation_policy")
        assert callable(client.confirm_tool_execution)
        assert callable(client.set_confirmation_policy)

    def test_client_has_lane_handler_methods(self, client: A3sClient):
        """Test client has lane handler methods."""
        assert hasattr(client, "set_lane_handler")
        assert hasattr(client, "get_lane_handler")
        assert hasattr(client, "complete_external_task")
        assert hasattr(client, "list_pending_external_tasks")
        assert callable(client.set_lane_handler)
        assert callable(client.complete_external_task)

    def test_client_has_permission_methods(self, client: A3sClient):
        """Test client has permission methods."""
        assert hasattr(client, "set_permission_policy")
        assert hasattr(client, "get_permission_policy")
        assert hasattr(client, "check_permission")
        assert hasattr(client, "add_permission_rule")
        assert callable(client.set_permission_policy)
        assert callable(client.check_permission)

    def test_client_has_todo_methods(self, client: A3sClient):
        """Test client has todo methods."""
        assert hasattr(client, "get_todos")
        assert hasattr(client, "set_todos")
        assert callable(client.get_todos)
        assert callable(client.set_todos)

    def test_client_has_provider_methods(self, client: A3sClient):
        """Test client has provider configuration methods."""
        assert hasattr(client, "list_providers")
        assert hasattr(client, "get_provider")
        assert hasattr(client, "add_provider")
        assert hasattr(client, "update_provider")
        assert hasattr(client, "remove_provider")
        assert hasattr(client, "set_default_model")
        assert hasattr(client, "get_default_model")
        assert callable(client.list_providers)
        assert callable(client.add_provider)

    def test_client_context_manager_methods(self, client: A3sClient):
        """Test client has async context manager methods."""
        assert hasattr(client, "__aenter__")
        assert hasattr(client, "__aexit__")
        assert hasattr(client, "connect")
        assert hasattr(client, "close")


# =============================================================================
# Helper Method Tests
# =============================================================================


class TestClientHelperMethods:
    """Tests for client helper methods."""

    def test_message_to_proto(self, client: A3sClient):
        """Test _message_to_proto helper."""
        msg = Message(role=MessageRole.USER, content="Hello")
        proto = client._message_to_proto(msg)
        assert proto["role"] == "user"
        assert proto["content"] == "Hello"
        assert proto["metadata"] == {}

    def test_message_to_proto_with_metadata(self, client: A3sClient):
        """Test _message_to_proto helper with metadata."""
        msg = Message(
            role=MessageRole.ASSISTANT,
            content="Hi there!",
            metadata={"key": "value"},
        )
        proto = client._message_to_proto(msg)
        assert proto["role"] == "assistant"
        assert proto["content"] == "Hi there!"
        assert proto["metadata"]["key"] == "value"

    def test_todo_to_proto(self, client: A3sClient):
        """Test _todo_to_proto helper."""
        todo = Todo(id="1", content="Test", status="pending", priority="high")
        proto = client._todo_to_proto(todo)
        assert proto["id"] == "1"
        assert proto["content"] == "Test"
        assert proto["status"] == "pending"
        assert proto["priority"] == "high"

    def test_session_config_to_proto(self, client: A3sClient):
        """Test _session_config_to_proto helper."""
        config = SessionConfig(
            name="test-session",
            workspace="/tmp/test",
            system_prompt="You are a helpful assistant.",
        )
        proto = client._session_config_to_proto(config)
        assert proto["name"] == "test-session"
        assert proto["workspace"] == "/tmp/test"
        assert proto["system_prompt"] == "You are a helpful assistant."

    def test_session_config_to_proto_with_llm(self, client: A3sClient):
        """Test _session_config_to_proto helper with LLM config."""
        llm = LLMConfig(provider="anthropic", model="claude-sonnet-4")
        config = SessionConfig(name="test", workspace="/tmp", llm=llm)
        proto = client._session_config_to_proto(config)
        assert "llm" in proto
        assert proto["llm"]["provider"] == "anthropic"
        assert proto["llm"]["model"] == "claude-sonnet-4"

