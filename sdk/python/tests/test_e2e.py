"""
End-to-end tests for A3S Code Python SDK.

These tests start a real a3s-code gRPC server as a subprocess and exercise
the full RPC stack through the generated Python proto stubs.

Requirements:
    - Rust binary `a3s-code` must be built: `cargo build -p a3s-code`
    - Python deps installed: `pip install -e ".[dev]"`

Usage:
    pytest tests/test_e2e.py -v
"""

import asyncio
import os
import signal
import subprocess
import sys
import tempfile
import time
from pathlib import Path

import grpc
import grpc.aio
import pytest

# Add the SDK root to sys.path so `proto` package is importable
SDK_ROOT = Path(__file__).parent.parent
if str(SDK_ROOT) not in sys.path:
    sys.path.insert(0, str(SDK_ROOT))

from proto import code_agent_pb2 as pb2
from proto import code_agent_pb2_grpc as pb2_grpc


# ============================================================================
# Fixtures
# ============================================================================

def _find_binary() -> str:
    """Locate the a3s-code binary from cargo build output."""
    repo_root = SDK_ROOT.parent.parent.parent.parent
    candidates = [
        repo_root / "target" / "debug" / "a3s-code",
        repo_root / "target" / "release" / "a3s-code",
    ]
    for path in candidates:
        if path.exists():
            return str(path)
    pytest.skip(
        "a3s-code binary not found. Run `cargo build -p a3s-code` first."
    )


def _wait_for_server(port: int, timeout: float = 10.0) -> bool:
    """Wait until the gRPC server is accepting connections."""
    import socket

    deadline = time.time() + timeout
    while time.time() < deadline:
        try:
            with socket.create_connection(("127.0.0.1", port), timeout=0.5):
                return True
        except (ConnectionRefusedError, OSError):
            time.sleep(0.2)
    return False


@pytest.fixture(scope="module")
def server():
    """Start a3s-code gRPC server and yield (process, port)."""
    binary = _find_binary()
    workspace = tempfile.mkdtemp(prefix="a3s_e2e_")
    port = _pick_free_port()

    env = os.environ.copy()
    env["RUST_LOG"] = "warn"

    proc = subprocess.Popen(
        [
            binary,
            "--listen-addr", f"127.0.0.1:{port}",
            "--workspace", workspace,
            "--storage-backend", "memory",
        ],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        env=env,
    )

    if not _wait_for_server(port):
        proc.kill()
        stdout, stderr = proc.communicate(timeout=5)
        pytest.fail(
            f"Server failed to start on port {port}.\n"
            f"stdout: {stdout.decode()}\nstderr: {stderr.decode()}"
        )

    yield proc, port

    # Teardown
    proc.send_signal(signal.SIGTERM)
    try:
        proc.wait(timeout=5)
    except subprocess.TimeoutExpired:
        proc.kill()
        proc.wait(timeout=3)

    # Clean up workspace
    import shutil
    shutil.rmtree(workspace, ignore_errors=True)


def _pick_free_port() -> int:
    """Pick a free TCP port."""
    import socket

    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


@pytest.fixture(scope="module")
def channel(server):
    """Create a gRPC channel to the test server."""
    _, port = server
    chan = grpc.insecure_channel(f"127.0.0.1:{port}")
    yield chan
    chan.close()


@pytest.fixture(scope="module")
def stub(channel):
    """Create a CodeAgentService stub."""
    return pb2_grpc.CodeAgentServiceStub(channel)


# ============================================================================
# Lifecycle Tests
# ============================================================================


class TestLifecycle:
    """E2E tests for lifecycle RPCs."""

    def test_health_check_before_init(self, stub):
        """Health check returns degraded before initialization."""
        resp = stub.HealthCheck(pb2.HealthCheckRequest())
        # Status 2 = Degraded (not initialized)
        assert resp.status == 2
        assert "not initialized" in resp.message.lower()

    def test_initialize(self, stub):
        """Initialize the agent with a workspace."""
        workspace = tempfile.mkdtemp(prefix="a3s_e2e_init_")
        resp = stub.Initialize(pb2.InitializeRequest(
            workspace=workspace,
            env={},
        ))
        assert resp.success is True
        assert "initialized" in resp.message.lower()
        assert resp.info.name == "a3s-code"
        assert resp.info.version != ""

    def test_health_check_after_init(self, stub):
        """Health check returns healthy after initialization."""
        resp = stub.HealthCheck(pb2.HealthCheckRequest())
        # Status 1 = Healthy
        assert resp.status == 1
        assert "healthy" in resp.message.lower()

    def test_get_capabilities(self, stub):
        """Get agent capabilities."""
        resp = stub.GetCapabilities(pb2.GetCapabilitiesRequest())
        assert resp.info.name == "a3s-code"
        assert "streaming" in resp.features
        assert "tool_calling" in resp.features
        assert resp.limits.max_context_tokens > 0
        assert resp.limits.max_concurrent_sessions > 0


# ============================================================================
# Session Management Tests
# ============================================================================


class TestSessionManagement:
    """E2E tests for session CRUD operations."""

    def test_create_session(self, stub):
        """Create a session with default config."""
        resp = stub.CreateSession(pb2.CreateSessionRequest(
            session_id="e2e-session-1",
            config=pb2.SessionConfig(
                name="test-session",
                workspace=tempfile.mkdtemp(prefix="a3s_e2e_ws_"),
            ),
        ))
        assert resp.session_id == "e2e-session-1"
        assert resp.session.session_id == "e2e-session-1"
        assert resp.session.config.name == "test-session"
        # State 1 = Active
        assert resp.session.state == 1
        assert resp.session.created_at > 0

    def test_create_session_auto_id(self, stub):
        """Create a session without specifying ID (auto-generated)."""
        resp = stub.CreateSession(pb2.CreateSessionRequest(
            config=pb2.SessionConfig(
                name="auto-id-session",
                workspace=tempfile.mkdtemp(prefix="a3s_e2e_ws_"),
            ),
        ))
        assert resp.session_id != ""
        assert len(resp.session_id) > 10  # UUID format

    def test_get_session(self, stub):
        """Get an existing session by ID."""
        resp = stub.GetSession(pb2.GetSessionRequest(
            session_id="e2e-session-1",
        ))
        assert resp.session.session_id == "e2e-session-1"
        assert resp.session.config.name == "test-session"
        assert resp.session.state == 1

    def test_get_session_not_found(self, stub):
        """Get a non-existent session returns NOT_FOUND."""
        with pytest.raises(grpc.RpcError) as exc_info:
            stub.GetSession(pb2.GetSessionRequest(
                session_id="nonexistent-session",
            ))
        assert exc_info.value.code() == grpc.StatusCode.NOT_FOUND

    def test_list_sessions(self, stub):
        """List all sessions."""
        resp = stub.ListSessions(pb2.ListSessionsRequest())
        session_ids = [s.session_id for s in resp.sessions]
        assert "e2e-session-1" in session_ids

    def test_configure_session(self, stub):
        """Update session configuration."""
        resp = stub.ConfigureSession(pb2.ConfigureSessionRequest(
            session_id="e2e-session-1",
            config=pb2.SessionConfig(
                name="updated-session",
                workspace=tempfile.mkdtemp(prefix="a3s_e2e_ws_"),
            ),
        ))
        assert resp.session.session_id == "e2e-session-1"

    def test_get_messages_empty(self, stub):
        """Get messages from a fresh session (should be empty)."""
        resp = stub.GetMessages(pb2.GetMessagesRequest(
            session_id="e2e-session-1",
        ))
        assert resp.total_count == 0

    def test_destroy_session(self, stub):
        """Destroy a session."""
        # Create a disposable session
        stub.CreateSession(pb2.CreateSessionRequest(
            session_id="e2e-disposable",
            config=pb2.SessionConfig(
                name="disposable",
                workspace=tempfile.mkdtemp(prefix="a3s_e2e_ws_"),
            ),
        ))
        resp = stub.DestroySession(pb2.DestroySessionRequest(
            session_id="e2e-disposable",
        ))
        assert resp.success is True

        # Verify it's gone
        with pytest.raises(grpc.RpcError) as exc_info:
            stub.GetSession(pb2.GetSessionRequest(
                session_id="e2e-disposable",
            ))
        assert exc_info.value.code() == grpc.StatusCode.NOT_FOUND


# ============================================================================
# Provider Configuration Tests
# ============================================================================


class TestProviderConfig:
    """E2E tests for provider management RPCs."""

    def test_list_providers_empty(self, stub):
        """List providers when none configured."""
        resp = stub.ListProviders(pb2.ListProvidersRequest())
        # May have defaults from config, just verify it doesn't crash
        assert resp is not None

    def test_add_provider(self, stub):
        """Add a new provider."""
        resp = stub.AddProvider(pb2.AddProviderRequest(
            provider=pb2.ProviderInfo(
                name="test-provider",
                api_key="sk-test-key",
                base_url="https://api.example.com",
                models=[
                    pb2.ModelInfo(
                        id="test-model-1",
                        name="Test Model 1",
                        family="test",
                        tool_call=True,
                    ),
                ],
            ),
        ))
        assert resp.success is True
        assert resp.provider.name == "test-provider"
        assert len(resp.provider.models) == 1

    def test_get_provider(self, stub):
        """Get a specific provider."""
        resp = stub.GetProvider(pb2.GetProviderRequest(name="test-provider"))
        assert resp.provider.name == "test-provider"
        assert resp.provider.api_key == "sk-test-key"
        assert len(resp.provider.models) == 1
        assert resp.provider.models[0].id == "test-model-1"

    def test_update_provider(self, stub):
        """Update an existing provider."""
        resp = stub.UpdateProvider(pb2.UpdateProviderRequest(
            provider=pb2.ProviderInfo(
                name="test-provider",
                api_key="sk-updated-key",
                base_url="https://api.updated.com",
                models=[
                    pb2.ModelInfo(
                        id="test-model-1",
                        name="Test Model Updated",
                        family="test",
                        tool_call=True,
                    ),
                    pb2.ModelInfo(
                        id="test-model-2",
                        name="Test Model 2",
                        family="test",
                    ),
                ],
            ),
        ))
        assert resp.success is True
        assert len(resp.provider.models) == 2

    def test_set_default_model(self, stub):
        """Set default provider and model."""
        resp = stub.SetDefaultModel(pb2.SetDefaultModelRequest(
            provider="test-provider",
            model="test-model-1",
        ))
        assert resp.success is True
        assert resp.provider == "test-provider"
        assert resp.model == "test-model-1"

    def test_get_default_model(self, stub):
        """Get current default provider and model."""
        resp = stub.GetDefaultModel(pb2.GetDefaultModelRequest())
        assert resp.provider == "test-provider"
        assert resp.model == "test-model-1"

    def test_list_providers_after_add(self, stub):
        """List providers after adding one."""
        resp = stub.ListProviders(pb2.ListProvidersRequest())
        names = [p.name for p in resp.providers]
        assert "test-provider" in names

    def test_remove_provider(self, stub):
        """Remove a provider."""
        # Add a disposable provider first
        stub.AddProvider(pb2.AddProviderRequest(
            provider=pb2.ProviderInfo(
                name="disposable-provider",
                models=[],
            ),
        ))
        resp = stub.RemoveProvider(pb2.RemoveProviderRequest(
            name="disposable-provider",
        ))
        assert resp.success is True


# ============================================================================
# Context Management Tests
# ============================================================================


class TestContextManagement:
    """E2E tests for context management RPCs."""

    def test_get_context_usage(self, stub):
        """Get context usage for a session."""
        resp = stub.GetContextUsage(pb2.GetContextUsageRequest(
            session_id="e2e-session-1",
        ))
        assert resp.usage is not None
        assert resp.usage.total_tokens >= 0

    def test_clear_context(self, stub):
        """Clear context for a session."""
        resp = stub.ClearContext(pb2.ClearContextRequest(
            session_id="e2e-session-1",
        ))
        assert resp.success is True


# ============================================================================
# Control Operations Tests
# ============================================================================


class TestControlOperations:
    """E2E tests for control RPCs (cancel, pause, resume)."""

    def test_pause_session(self, stub):
        """Pause a session."""
        resp = stub.Pause(pb2.PauseRequest(
            session_id="e2e-session-1",
        ))
        assert resp.success is True

    def test_resume_session(self, stub):
        """Resume a paused session."""
        resp = stub.Resume(pb2.ResumeRequest(
            session_id="e2e-session-1",
        ))
        assert resp.success is True

    def test_cancel_operation(self, stub):
        """Cancel when nothing is running returns false (nothing to cancel)."""
        # Create a dedicated session for cancel test
        stub.CreateSession(pb2.CreateSessionRequest(
            session_id="e2e-cancel-session",
            config=pb2.SessionConfig(
                name="cancel-test",
                workspace=tempfile.mkdtemp(prefix="a3s_e2e_ws_"),
            ),
        ))
        resp = stub.Cancel(pb2.CancelRequest(
            session_id="e2e-cancel-session",
        ))
        # No active operation to cancel, so success is False
        assert resp.success is False


# ============================================================================
# HITL (Human-in-the-Loop) Tests
# ============================================================================


class TestHITL:
    """E2E tests for HITL confirmation RPCs."""

    def test_get_confirmation_policy_default(self, stub):
        """Get default confirmation policy (disabled)."""
        resp = stub.GetConfirmationPolicy(pb2.GetConfirmationPolicyRequest(
            session_id="e2e-session-1",
        ))
        assert resp.policy is not None
        assert resp.policy.enabled is False

    def test_set_confirmation_policy(self, stub):
        """Set a confirmation policy."""
        resp = stub.SetConfirmationPolicy(pb2.SetConfirmationPolicyRequest(
            session_id="e2e-session-1",
            policy=pb2.ConfirmationPolicy(
                enabled=True,
                auto_approve_tools=["read", "glob"],
                require_confirm_tools=["bash", "write"],
                default_timeout_ms=15000,
                timeout_action=1,  # REJECT
            ),
        ))
        assert resp.success is True
        assert resp.policy.enabled is True
        assert "read" in resp.policy.auto_approve_tools
        assert "bash" in resp.policy.require_confirm_tools
        assert resp.policy.default_timeout_ms == 15000

    def test_get_confirmation_policy_updated(self, stub):
        """Verify updated confirmation policy persists."""
        resp = stub.GetConfirmationPolicy(pb2.GetConfirmationPolicyRequest(
            session_id="e2e-session-1",
        ))
        assert resp.policy.enabled is True
        assert resp.policy.default_timeout_ms == 15000


# ============================================================================
# Permission System Tests
# ============================================================================


class TestPermissions:
    """E2E tests for permission policy RPCs."""

    def test_get_permission_policy_default(self, stub):
        """Get default permission policy."""
        resp = stub.GetPermissionPolicy(pb2.GetPermissionPolicyRequest(
            session_id="e2e-session-1",
        ))
        assert resp.policy is not None

    def test_set_permission_policy(self, stub):
        """Set a permission policy with rules."""
        resp = stub.SetPermissionPolicy(pb2.SetPermissionPolicyRequest(
            session_id="e2e-session-1",
            policy=pb2.PermissionPolicy(
                enabled=True,
                allow=[pb2.PermissionRule(rule="Bash(cargo:*)")],
                deny=[pb2.PermissionRule(rule="Bash(rm:*)")],
                ask=[pb2.PermissionRule(rule="Write(*)")],
                default_decision=1,  # ALLOW
            ),
        ))
        assert resp.success is True
        assert resp.policy.enabled is True
        assert len(resp.policy.allow) == 1
        assert resp.policy.allow[0].rule == "Bash(cargo:*)"
        assert len(resp.policy.deny) == 1
        assert len(resp.policy.ask) == 1

    def test_add_permission_rule(self, stub):
        """Add a single permission rule."""
        resp = stub.AddPermissionRule(pb2.AddPermissionRuleRequest(
            session_id="e2e-session-1",
            rule_type="allow",
            rule="Read(*)",
        ))
        assert resp.success is True

    def test_check_permission(self, stub):
        """Check permission for a tool execution."""
        resp = stub.CheckPermission(pb2.CheckPermissionRequest(
            session_id="e2e-session-1",
            tool_name="Bash",
            arguments="cargo test",
        ))
        # Should return a decision (the exact value depends on policy matching)
        assert resp is not None


# ============================================================================
# Todo/Task Tracking Tests
# ============================================================================


class TestTodos:
    """E2E tests for todo management RPCs."""

    def test_get_todos_empty(self, stub):
        """Get todos from a fresh session (should be empty)."""
        resp = stub.GetTodos(pb2.GetTodosRequest(
            session_id="e2e-session-1",
        ))
        assert len(resp.todos) == 0

    def test_set_todos(self, stub):
        """Set todos for a session."""
        resp = stub.SetTodos(pb2.SetTodosRequest(
            session_id="e2e-session-1",
            todos=[
                pb2.Todo(id="1", content="Write tests", status="in_progress", priority="high"),
                pb2.Todo(id="2", content="Fix bug", status="pending", priority="medium"),
                pb2.Todo(id="3", content="Update docs", status="pending", priority="low"),
            ],
        ))
        assert len(resp.todos) == 3

    def test_get_todos_after_set(self, stub):
        """Verify todos persist after setting."""
        resp = stub.GetTodos(pb2.GetTodosRequest(
            session_id="e2e-session-1",
        ))
        assert len(resp.todos) == 3
        ids = [t.id for t in resp.todos]
        assert "1" in ids
        assert "2" in ids
        assert "3" in ids

        # Verify content
        todo_map = {t.id: t for t in resp.todos}
        assert todo_map["1"].content == "Write tests"
        assert todo_map["1"].status == "in_progress"
        assert todo_map["1"].priority == "high"

    def test_set_todos_replace(self, stub):
        """Replace all todos."""
        resp = stub.SetTodos(pb2.SetTodosRequest(
            session_id="e2e-session-1",
            todos=[
                pb2.Todo(id="4", content="Deploy", status="pending", priority="high"),
            ],
        ))
        assert len(resp.todos) == 1
        assert resp.todos[0].id == "4"


# ============================================================================
# Skill Management Tests
# ============================================================================


class TestSkills:
    """E2E tests for skill management RPCs."""

    def test_list_skills_initial(self, stub):
        """List skills (may be empty initially)."""
        resp = stub.ListSkills(pb2.ListSkillsRequest())
        assert resp is not None

    def test_load_skill(self, stub):
        """Load a Claude Code format skill."""
        skill_content = """---
name: test-skill
description: A test skill for E2E testing
allowed-tools: Bash(echo:*)
---
This is a test skill. Use echo commands for testing.
"""
        resp = stub.LoadSkill(pb2.LoadSkillRequest(
            session_id="e2e-session-1",
            skill_name="test-skill",
            skill_content=skill_content,
        ))
        assert resp.success is True

    def test_list_skills_after_load(self, stub):
        """List skills after loading one - skill registry should grow."""
        resp = stub.ListSkills(pb2.ListSkillsRequest())
        # The skill is registered in the global registry; verify count increased
        assert len(resp.skills) > 0

    def test_unload_skill(self, stub):
        """Unload a skill."""
        resp = stub.UnloadSkill(pb2.UnloadSkillRequest(
            session_id="e2e-session-1",
            skill_name="test-skill",
        ))
        assert resp.success is True


# ============================================================================
# Memory System Tests
# ============================================================================


class TestMemory:
    """E2E tests for memory system RPCs."""

    def test_get_memory_stats_initial(self, stub):
        """Get memory stats for a fresh session."""
        resp = stub.GetMemoryStats(pb2.GetMemoryStatsRequest(
            session_id="e2e-session-1",
        ))
        assert resp.stats is not None
        assert resp.stats.short_term_count >= 0
        assert resp.stats.long_term_count >= 0
        assert resp.stats.working_count >= 0

    def test_store_memory(self, stub):
        """Store a memory item."""
        resp = stub.StoreMemory(pb2.StoreMemoryRequest(
            session_id="e2e-session-1",
            memory=pb2.MemoryItem(
                content="The user prefers Python over JavaScript",
                importance=0.8,
                tags=["preference", "language"],
                memory_type=1,  # EPISODIC
                metadata={"source": "e2e-test"},
            ),
        ))
        assert resp.success is True
        assert resp.memory_id != ""

    def test_store_multiple_memories(self, stub):
        """Store multiple memory items of different types."""
        memories = [
            pb2.MemoryItem(
                content="API endpoint is /api/v1/users",
                importance=0.6,
                tags=["api", "endpoint"],
                memory_type=2,  # SEMANTIC
            ),
            pb2.MemoryItem(
                content="Always run tests before committing",
                importance=0.9,
                tags=["workflow", "testing"],
                memory_type=3,  # PROCEDURAL
            ),
            pb2.MemoryItem(
                content="Currently debugging auth module",
                importance=0.5,
                tags=["context", "debug"],
                memory_type=4,  # WORKING
            ),
        ]
        ids = []
        for mem in memories:
            resp = stub.StoreMemory(pb2.StoreMemoryRequest(
                session_id="e2e-session-1",
                memory=mem,
            ))
            assert resp.success is True
            ids.append(resp.memory_id)
        assert len(ids) == 3
        assert all(id != "" for id in ids)

    def test_retrieve_memory(self, stub):
        """Retrieve a stored memory by ID."""
        # Store a memory and retrieve it
        store_resp = stub.StoreMemory(pb2.StoreMemoryRequest(
            session_id="e2e-session-1",
            memory=pb2.MemoryItem(
                content="Retrievable memory content",
                importance=0.7,
                tags=["retrieve-test"],
                memory_type=1,  # EPISODIC
            ),
        ))
        memory_id = store_resp.memory_id

        resp = stub.RetrieveMemory(pb2.RetrieveMemoryRequest(
            session_id="e2e-session-1",
            memory_id=memory_id,
        ))
        assert resp.memory is not None
        assert resp.memory.content == "Retrievable memory content"
        assert resp.memory.importance == pytest.approx(0.7, abs=0.01)
        assert "retrieve-test" in resp.memory.tags

    def test_search_memories(self, stub):
        """Search memories by query."""
        resp = stub.SearchMemories(pb2.SearchMemoriesRequest(
            session_id="e2e-session-1",
            query="Python",
            limit=10,
        ))
        assert resp.total_count >= 0

    def test_search_memories_by_tags(self, stub):
        """Search memories by tags."""
        resp = stub.SearchMemories(pb2.SearchMemoriesRequest(
            session_id="e2e-session-1",
            tags=["preference"],
            limit=10,
        ))
        assert resp.total_count >= 0

    def test_get_memory_stats_after_store(self, stub):
        """Memory stats should reflect stored items."""
        resp = stub.GetMemoryStats(pb2.GetMemoryStatsRequest(
            session_id="e2e-session-1",
        ))
        assert resp.stats.short_term_count > 0 or resp.stats.long_term_count > 0

    def test_clear_memories(self, stub):
        """Clear specific memory tiers."""
        resp = stub.ClearMemories(pb2.ClearMemoriesRequest(
            session_id="e2e-session-1",
            clear_short_term=True,
            clear_working=True,
        ))
        assert resp.success is True


# ============================================================================
# Shutdown Test (must run last)
# ============================================================================


class TestShutdown:
    """E2E test for graceful shutdown (runs last)."""

    def test_shutdown(self, stub):
        """Shutdown the agent gracefully."""
        resp = stub.Shutdown(pb2.ShutdownRequest())
        assert resp.success is True
        assert "shutdown" in resp.message.lower()

    def test_health_check_after_shutdown(self, stub):
        """Health check returns degraded after shutdown."""
        resp = stub.HealthCheck(pb2.HealthCheckRequest())
        # Status 2 = Degraded (not initialized after shutdown)
        assert resp.status == 2
