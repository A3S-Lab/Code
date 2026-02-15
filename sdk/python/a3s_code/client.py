"""
A3S Code Agent gRPC Client

Full implementation of the CodeAgentService interface.
"""

import grpc
import json
import os
from pathlib import Path
from typing import Optional, List, Dict, Iterator, Any, Union, overload

from .provider import ModelRef
from .session import CodeSession
from .types import (
    ProviderInfo,
    ModelInfo,
    ModelCostInfo,
    ModelLimitInfo,
    ModelModalitiesInfo,
    SessionConfig,
    StorageType,
    Message,
    Todo,
    ConfirmationPolicy,
    LaneHandlerConfig,
    PermissionPolicy,
    SessionLane,
    TimeoutAction,
    TaskHandlerMode,
    # Planning types
    ExecutionPlan,
    AgentGoal,
    PlanStep,
    Complexity,
    StepStatus,
    # Memory types
    MemoryItem,
    MemoryStats,
    MemoryType,
    # MCP types
    McpServerConfig,
    McpServerInfo,
    McpToolInfo,
    # Skill types
    Skill,
    # Cron types
    CronJob,
    CronExecution,
    CronJobStatus,
    CronExecutionStatus,
    # Agentic types
    AgenticStrategy,
    AgenticStep,
    AgenticGenerateResponse,
    AgenticGenerateEvent,
    # Delegation types
    DelegateResponse,
    # Queue types
    LaneStats,
    QueueStats,
)


class A3sClient:
    """
    A3S Code Agent gRPC Client

    Provides a Python interface to all CodeAgentService RPCs.

    Example:
        ```python
        # Create client with explicit address
        async with A3sClient(address="localhost:4088") as client:
            await client.add_provider(ProviderInfo(
                name="anthropic",
                api_key="sk-xxx",
                models=[ModelInfo(id="claude-sonnet-4", name="Claude Sonnet 4")]
            ))
        ```
    """

    def __init__(
        self,
        address: str = "localhost:4088",
        use_tls: bool = False,
    ):
        """
        Initialize the client.

        Args:
            address: gRPC server address (default: localhost:4088)
            use_tls: Use TLS for connection
        """
        self.address = address
        self.use_tls = use_tls
        self._channel: Optional[grpc.aio.Channel] = None
        self._stub: Any = None

    async def __aenter__(self):
        await self.connect()
        return self

    async def __aexit__(self, exc_type, exc_val, exc_tb):
        await self.close()

    async def connect(self):
        """Connect to the gRPC server."""
        if self.use_tls:
            credentials = grpc.ssl_channel_credentials()
            self._channel = grpc.aio.secure_channel(self.address, credentials)
        else:
            self._channel = grpc.aio.insecure_channel(self.address)

        # Load proto dynamically
        # Note: In production, you would use generated stubs
        # For now, we'll use reflection or generated code

    async def close(self):
        """Close the connection."""
        if self._channel:
            await self._channel.close()
            self._channel = None

    # =========================================================================
    # Lifecycle Management
    # =========================================================================

    async def health_check(self) -> Dict[str, Any]:
        """Check the health status of the agent."""
        response = await self._stub.HealthCheck({})
        return {
            "status": response.status,
            "message": response.message,
            "details": dict(response.details),
        }

    async def get_capabilities(self) -> Dict[str, Any]:
        """Get agent capabilities."""
        response = await self._stub.GetCapabilities({})
        return response

    async def initialize(
        self, workspace: str, env: Optional[Dict[str, str]] = None
    ) -> Dict[str, Any]:
        """Initialize the agent with workspace and environment."""
        response = await self._stub.Initialize({
            "workspace": workspace,
            "env": env or {},
        })
        return {
            "success": response.success,
            "message": response.message,
            "info": response.info,
        }

    async def shutdown(self) -> Dict[str, Any]:
        """Shutdown the agent."""
        response = await self._stub.Shutdown({})
        return {
            "success": response.success,
            "message": response.message,
        }

    # =========================================================================
    # Session Management
    # =========================================================================

    async def create_session(
        self,
        config: Optional[SessionConfig] = None,
        session_id: Optional[str] = None,
        initial_context: Optional[List[Message]] = None,
        *,
        name: str = "",
        workspace: str = "",
        system_prompt: Optional[str] = None,
        max_context_length: Optional[int] = None,
        auto_compact: Optional[bool] = None,
        storage_type: Optional[StorageType] = None,
    ) -> Dict[str, Any]:
        """Create a new session.

        Accepts either a SessionConfig object or keyword arguments to build one.
        If both are provided, keyword arguments override the config fields.

        Args:
            config: Optional SessionConfig object.
            session_id: Optional session ID (server generates one if omitted).
            initial_context: Optional list of messages to seed the session.
            name: Session name.
            workspace: Working directory for tool sandboxing.
            system_prompt: Custom system prompt for the session.
            max_context_length: Maximum context window length.
            auto_compact: Enable automatic context compaction.
            storage_type: Storage backend (StorageType.MEMORY or StorageType.FILE).

        Returns:
            Dict with session_id and session.
        """
        # Build config from kwargs when no config object is provided
        has_kwargs = any(v for v in [name, workspace, system_prompt,
                                     max_context_length, auto_compact, storage_type])
        if config is None and has_kwargs:
            config = SessionConfig(
                name=name,
                workspace=workspace,
                system_prompt=system_prompt,
                max_context_length=max_context_length,
                auto_compact=auto_compact,
                storage_type=storage_type,
            )
        elif config is not None and has_kwargs:
            # kwargs override config fields
            if name:
                config.name = name
            if workspace:
                config.workspace = workspace
            if system_prompt is not None:
                config.system_prompt = system_prompt
            if max_context_length is not None:
                config.max_context_length = max_context_length
            if auto_compact is not None:
                config.auto_compact = auto_compact
            if storage_type is not None:
                config.storage_type = storage_type

        request = {
            "session_id": session_id,
            "config": self._session_config_to_proto(config) if config else None,
            "initial_context": [self._message_to_proto(m) for m in (initial_context or [])],
        }
        response = await self._stub.CreateSession(request)
        return {
            "session_id": response.session_id,
            "session": response.session,
        }

    async def session(
        self,
        model: ModelRef,
        *,
        workspace: str = "",
        system: Optional[str] = None,
        session_id: Optional[str] = None,
        auto_compact: Optional[bool] = None,
        auto_compact_threshold: Optional[float] = None,
    ) -> CodeSession:
        """Create a session with a model reference (high-level API).

        Returns a CodeSession object with send(), stream(), delegate() methods.
        Supports ``async with`` for automatic cleanup.

        Args:
            model: Model reference from create_provider()
            workspace: Working directory for tool sandboxing
            system: System prompt
            session_id: Optional session ID (server generates one if omitted)
            auto_compact: Enable automatic context compaction
            auto_compact_threshold: Auto-compact threshold (0.0-1.0)

        Returns:
            CodeSession with high-level API

        Example:
            ```python
            from a3s_code import A3sClient, create_provider

            anthropic = create_provider(name="anthropic", api_key="sk-ant-xxx")

            async with A3sClient() as client:
                async with await client.session(
                    model=anthropic("claude-sonnet-4-20250514"),
                    workspace="/project",
                    system="You are a senior engineer.",
                ) as session:
                    result = await session.send("Refactor the auth module")
                    print(result.text)
            ```
        """
        from .types import LLMConfig, SessionConfig

        llm = LLMConfig(
            provider=model.provider,
            model=model.model,
            api_key=model.api_key,
            base_url=model.base_url,
        )
        config = SessionConfig(
            name=f"session-{session_id or 'auto'}",
            workspace=workspace,
            llm=llm,
            system_prompt=system,
            auto_compact=auto_compact,
        )
        request = {
            "session_id": session_id,
            "config": self._session_config_to_proto(config),
        }
        response = await self._stub.CreateSession(request)
        return CodeSession(self, response.session_id)

    async def destroy_session(self, session_id: str) -> bool:
        """Destroy a session."""
        response = await self._stub.DestroySession({"session_id": session_id})
        return response.success

    async def restore_session(self, session_id: str) -> Dict[str, Any]:
        """Restore a persisted session from store by ID.

        Args:
            session_id: The session ID to restore.

        Returns:
            Dict with success, session_id, and message.
        """
        response = await self._stub.RestoreSession({"session_id": session_id})
        return {
            "success": response.success,
            "session_id": response.session_id,
            "message": response.message,
        }

    async def list_sessions(self) -> List[Dict[str, Any]]:
        """List all sessions."""
        response = await self._stub.ListSessions({})
        return list(response.sessions)

    async def get_session(self, session_id: str) -> Dict[str, Any]:
        """Get a specific session."""
        response = await self._stub.GetSession({"session_id": session_id})
        return response.session

    async def configure_session(
        self, session_id: str, config: SessionConfig
    ) -> Dict[str, Any]:
        """Configure a session."""
        response = await self._stub.ConfigureSession({
            "session_id": session_id,
            "config": self._session_config_to_proto(config),
        })
        return response.session

    async def get_messages(
        self,
        session_id: str,
        limit: Optional[int] = None,
        offset: Optional[int] = None,
    ) -> Dict[str, Any]:
        """Get messages from a session."""
        response = await self._stub.GetMessages({
            "session_id": session_id,
            "limit": limit,
            "offset": offset,
        })
        return {
            "messages": list(response.messages),
            "total_count": response.total_count,
            "has_more": response.has_more,
        }

    # =========================================================================
    # Code Generation
    # =========================================================================

    async def generate(
        self, session_id: str, messages: List[Message]
    ) -> Dict[str, Any]:
        """Generate a response (unary)."""
        response = await self._stub.Generate({
            "session_id": session_id,
            "messages": [self._message_to_proto(m) for m in messages],
        })
        return response

    async def stream_generate(
        self, session_id: str, messages: List[Message]
    ) -> Iterator[Dict[str, Any]]:
        """Generate a response (streaming)."""
        async for chunk in self._stub.StreamGenerate({
            "session_id": session_id,
            "messages": [self._message_to_proto(m) for m in messages],
        }):
            yield chunk

    async def generate_structured(
        self, session_id: str, messages: List[Message], schema: str
    ) -> Dict[str, Any]:
        """Generate a structured response (unary).

        Args:
            session_id: Session ID
            messages: Input messages
            schema: JSON Schema for the expected output

        Returns:
            Dict with session_id, data (JSON string), usage, and metadata
        """
        response = await self._stub.GenerateStructured({
            "session_id": session_id,
            "messages": [self._message_to_proto(m) for m in messages],
            "schema": schema,
        })
        return {
            "session_id": response.session_id,
            "data": response.data,
            "usage": response.usage,
            "metadata": dict(response.metadata),
        }

    async def stream_generate_structured(
        self, session_id: str, messages: List[Message], schema: str
    ) -> Iterator[Dict[str, Any]]:
        """Generate a structured response (streaming).

        Args:
            session_id: Session ID
            messages: Input messages
            schema: JSON Schema for the expected output

        Yields:
            Dict with session_id, data (partial JSON), and done flag
        """
        async for chunk in self._stub.StreamGenerateStructured({
            "session_id": session_id,
            "messages": [self._message_to_proto(m) for m in messages],
            "schema": schema,
        }):
            yield {
                "session_id": chunk.session_id,
                "data": chunk.data,
                "done": chunk.done,
            }

    # =========================================================================
    # Skill Management
    # =========================================================================

    async def load_skill(
        self,
        session_id: str,
        skill_name: str,
        skill_content: Optional[str] = None,
    ) -> Dict[str, Any]:
        """Load a skill into a session."""
        response = await self._stub.LoadSkill({
            "session_id": session_id,
            "skill_name": skill_name,
            "skill_content": skill_content,
        })
        return {
            "success": response.success,
        }

    async def unload_skill(
        self, session_id: str, skill_name: str
    ) -> Dict[str, Any]:
        """Unload a skill from a session."""
        response = await self._stub.UnloadSkill({
            "session_id": session_id,
            "skill_name": skill_name,
        })
        return {
            "success": response.success,
        }

    async def list_skills(
        self, session_id: Optional[str] = None
    ) -> List[Dict[str, Any]]:
        """List available skills."""
        response = await self._stub.ListSkills({"session_id": session_id})
        return list(response.skills)

    async def get_skill(
        self, name: Optional[str] = None
    ) -> List[Skill]:
        """Get skill(s) by name.

        Args:
            name: Optional skill name to filter by

        Returns:
            List of Skill objects
        """
        request = {}
        if name:
            request["name"] = name
        response = await self._stub.GetSkill(request)
        return [
            Skill(
                name=s.name,
                description=s.description,
                allowed_tools=s.allowed_tools or None,
                disable_model_invocation=s.disable_model_invocation,
                content=s.content,
                kind=s.kind or "instruction",
            )
            for s in response.skills
        ]

    async def search_skills(
        self, query: str, limit: int = 10
    ) -> Dict[str, Any]:
        """Search for skills in the open skills ecosystem.

        Args:
            query: Search query (e.g., "react performance", "testing")
            limit: Maximum number of results (default 10)

        Returns:
            Dict with 'results' (list of skill search results) and 'total_count'
        """
        response = await self._stub.SearchSkills({
            "query": query,
            "limit": limit,
        })
        return {
            "results": [
                {
                    "name": r.name,
                    "description": r.description,
                    "url": r.url,
                    "stars": r.stars,
                    "topics": list(r.topics),
                    "install_source": r.install_source,
                }
                for r in response.results
            ],
            "total_count": response.total_count,
        }

    async def install_skill(
        self,
        source: str,
        global_install: bool = False,
        session_id: Optional[str] = None,
    ) -> Dict[str, Any]:
        """Install a skill from the skills ecosystem.

        Args:
            source: Skill source (e.g., "owner/repo" or "owner/repo@skill-name")
            global_install: Install globally (user-level) vs project-level
            session_id: Optional session to load the skill into after install

        Returns:
            Dict with 'success', 'message', 'installed_path', 'skill_name'
        """
        response = await self._stub.InstallSkill({
            "source": source,
            "global": global_install,
            "session_id": session_id,
        })
        return {
            "success": response.success,
            "message": response.message,
            "installed_path": response.installed_path,
            "skill_name": response.skill_name,
        }

    # =========================================================================
    # Context Management
    # =========================================================================

    async def get_context_usage(self, session_id: str) -> Dict[str, Any]:
        """Get context usage for a session."""
        response = await self._stub.GetContextUsage({"session_id": session_id})
        return response.usage

    async def compact_context(self, session_id: str) -> Dict[str, Any]:
        """Compact context for a session."""
        response = await self._stub.CompactContext({"session_id": session_id})
        return {
            "success": response.success,
            "before": response.before,
            "after": response.after,
        }

    async def clear_context(self, session_id: str) -> bool:
        """Clear context for a session."""
        response = await self._stub.ClearContext({"session_id": session_id})
        return response.success

    # =========================================================================
    # Control Operations
    # =========================================================================

    async def cancel(
        self, session_id: str, operation_id: Optional[str] = None
    ) -> bool:
        """Cancel an operation."""
        response = await self._stub.Cancel({
            "session_id": session_id,
            "operation_id": operation_id,
        })
        return response.success

    async def pause(self, session_id: str) -> bool:
        """Pause a session."""
        response = await self._stub.Pause({"session_id": session_id})
        return response.success

    async def resume(self, session_id: str) -> bool:
        """Resume a session."""
        response = await self._stub.Resume({"session_id": session_id})
        return response.success

    # =========================================================================
    # Event Streaming
    # =========================================================================

    async def subscribe_events(
        self,
        session_id: Optional[str] = None,
        event_types: Optional[List[str]] = None,
    ) -> Iterator[Dict[str, Any]]:
        """Subscribe to agent events.

        Args:
            session_id: Optional session ID to filter events
            event_types: Optional list of event types to subscribe to

        Yields:
            AgentEvent dicts with type, session_id, timestamp, message, and data
        """
        async for event in self._stub.SubscribeEvents({
            "session_id": session_id,
            "event_types": event_types or [],
        }):
            yield {
                "type": event.type,
                "session_id": event.session_id,
                "timestamp": event.timestamp,
                "message": event.message,
                "data": dict(event.data),
            }

    # =========================================================================
    # Human-in-the-Loop (HITL)
    # =========================================================================

    async def confirm_tool_execution(
        self,
        session_id: str,
        tool_id: str,
        approved: bool,
        reason: Optional[str] = None,
    ) -> Dict[str, Any]:
        """Confirm or reject a tool execution.

        Args:
            session_id: Session ID
            tool_id: Tool execution ID
            approved: Whether to approve the execution
            reason: Optional reason for the decision

        Returns:
            Dict with success and error
        """
        response = await self._stub.ConfirmToolExecution({
            "session_id": session_id,
            "tool_id": tool_id,
            "approved": approved,
            "reason": reason,
        })
        return {
            "success": response.success,
            "error": response.error,
        }

    async def set_confirmation_policy(
        self, session_id: str, policy: ConfirmationPolicy
    ) -> Dict[str, Any]:
        """Set the confirmation policy for a session.

        Args:
            session_id: Session ID
            policy: Confirmation policy configuration

        Returns:
            Dict with success and policy
        """
        response = await self._stub.SetConfirmationPolicy({
            "session_id": session_id,
            "policy": self._confirmation_policy_to_proto(policy),
        })
        return {
            "success": response.success,
            "policy": self._proto_to_confirmation_policy(response.policy),
        }

    async def get_confirmation_policy(self, session_id: str) -> ConfirmationPolicy:
        """Get the confirmation policy for a session.

        Args:
            session_id: Session ID

        Returns:
            ConfirmationPolicy
        """
        response = await self._stub.GetConfirmationPolicy({
            "session_id": session_id,
        })
        return self._proto_to_confirmation_policy(response.policy)

    # =========================================================================
    # External Task Handling
    # =========================================================================

    async def set_lane_handler(
        self, session_id: str, lane: SessionLane, config: LaneHandlerConfig
    ) -> Dict[str, Any]:
        """Set the handler configuration for a lane.

        Args:
            session_id: Session ID
            lane: Session lane
            config: Lane handler configuration

        Returns:
            Dict with success and config
        """
        response = await self._stub.SetLaneHandler({
            "session_id": session_id,
            "lane": self._session_lane_to_proto(lane),
            "config": self._lane_handler_config_to_proto(config),
        })
        return {
            "success": response.success,
            "config": self._proto_to_lane_handler_config(response.config),
        }

    async def get_lane_handler(
        self, session_id: str, lane: SessionLane
    ) -> LaneHandlerConfig:
        """Get the handler configuration for a lane.

        Args:
            session_id: Session ID
            lane: Session lane

        Returns:
            LaneHandlerConfig
        """
        response = await self._stub.GetLaneHandler({
            "session_id": session_id,
            "lane": self._session_lane_to_proto(lane),
        })
        return self._proto_to_lane_handler_config(response.config)

    async def complete_external_task(
        self,
        session_id: str,
        task_id: str,
        success: bool,
        result: str = "",
        error: str = "",
    ) -> Dict[str, Any]:
        """Complete an external task.

        Args:
            session_id: Session ID
            task_id: Task ID
            success: Whether the task succeeded
            result: Result data (JSON string)
            error: Error message if failed

        Returns:
            Dict with success and error
        """
        response = await self._stub.CompleteExternalTask({
            "session_id": session_id,
            "task_id": task_id,
            "success": success,
            "result": result,
            "error": error,
        })
        return {
            "success": response.success,
            "error": response.error,
        }

    async def list_pending_external_tasks(
        self, session_id: str
    ) -> List[Dict[str, Any]]:
        """List pending external tasks for a session.

        Args:
            session_id: Session ID

        Returns:
            List of external task dicts
        """
        response = await self._stub.ListPendingExternalTasks({
            "session_id": session_id,
        })
        return [
            {
                "task_id": t.task_id,
                "session_id": t.session_id,
                "lane": t.lane,
                "command_type": t.command_type,
                "payload": t.payload,
                "timeout_ms": t.timeout_ms,
                "remaining_ms": t.remaining_ms,
            }
            for t in response.tasks
        ]

    # =========================================================================
    # Permission System
    # =========================================================================

    async def set_permission_policy(
        self, session_id: str, policy: PermissionPolicy
    ) -> Dict[str, Any]:
        """Set the permission policy for a session.

        Args:
            session_id: Session ID
            policy: Permission policy configuration

        Returns:
            Dict with success and policy
        """
        response = await self._stub.SetPermissionPolicy({
            "session_id": session_id,
            "policy": self._permission_policy_to_proto(policy),
        })
        return {
            "success": response.success,
            "policy": self._proto_to_permission_policy(response.policy),
        }

    async def get_permission_policy(self, session_id: str) -> PermissionPolicy:
        """Get the permission policy for a session.

        Args:
            session_id: Session ID

        Returns:
            PermissionPolicy
        """
        response = await self._stub.GetPermissionPolicy({
            "session_id": session_id,
        })
        return self._proto_to_permission_policy(response.policy)

    async def check_permission(
        self, session_id: str, tool_name: str, arguments: str
    ) -> Dict[str, Any]:
        """Check permission for a tool execution.

        Args:
            session_id: Session ID
            tool_name: Tool name
            arguments: Tool arguments (JSON string)

        Returns:
            Dict with decision and matching_rules
        """
        response = await self._stub.CheckPermission({
            "session_id": session_id,
            "tool_name": tool_name,
            "arguments": arguments,
        })
        return {
            "decision": response.decision,
            "matching_rules": list(response.matching_rules),
        }

    async def add_permission_rule(
        self, session_id: str, rule_type: str, rule: str
    ) -> Dict[str, Any]:
        """Add a permission rule.

        Args:
            session_id: Session ID
            rule_type: Rule type ("allow", "deny", or "ask")
            rule: Rule pattern (e.g., "Bash(cargo:*)")

        Returns:
            Dict with success and error
        """
        response = await self._stub.AddPermissionRule({
            "session_id": session_id,
            "rule_type": rule_type,
            "rule": rule,
        })
        return {
            "success": response.success,
            "error": response.error,
        }

    # =========================================================================
    # Todo/Task Tracking
    # =========================================================================

    async def get_todos(self, session_id: str) -> List[Todo]:
        """Get todos for a session."""
        response = await self._stub.GetTodos({"session_id": session_id})
        return [self._proto_to_todo(t) for t in response.todos]

    async def set_todos(self, session_id: str, todos: List[Todo]) -> List[Todo]:
        """Set todos for a session."""
        response = await self._stub.SetTodos({
            "session_id": session_id,
            "todos": [self._todo_to_proto(t) for t in todos],
        })
        return [self._proto_to_todo(t) for t in response.todos]

    # =========================================================================
    # Provider Configuration
    # =========================================================================

    async def list_providers(self) -> Dict[str, Any]:
        """
        List all configured providers.

        Returns:
            Dict with providers list, default_provider, and default_model
        """
        response = await self._stub.ListProviders({})
        return {
            "providers": [self._proto_to_provider(p) for p in response.providers],
            "default_provider": response.default_provider or None,
            "default_model": response.default_model or None,
        }

    async def get_provider(self, name: str) -> Optional[ProviderInfo]:
        """
        Get a specific provider by name.

        Args:
            name: Provider name

        Returns:
            ProviderInfo or None if not found
        """
        response = await self._stub.GetProvider({"name": name})
        if response.provider:
            return self._proto_to_provider(response.provider)
        return None

    async def add_provider(self, provider: ProviderInfo) -> Dict[str, Any]:
        """
        Add a new provider.

        Args:
            provider: Provider configuration

        Returns:
            Dict with success, error, and provider
        """
        response = await self._stub.AddProvider({
            "provider": self._provider_to_proto(provider),
        })
        return {
            "success": response.success,
            "error": response.error,
            "provider": self._proto_to_provider(response.provider) if response.provider else None,
        }

    async def update_provider(self, provider: ProviderInfo) -> Dict[str, Any]:
        """
        Update an existing provider.

        Args:
            provider: Provider configuration (name identifies which to update)

        Returns:
            Dict with success, error, and provider
        """
        response = await self._stub.UpdateProvider({
            "provider": self._provider_to_proto(provider),
        })
        return {
            "success": response.success,
            "error": response.error,
            "provider": self._proto_to_provider(response.provider) if response.provider else None,
        }

    async def remove_provider(self, name: str) -> Dict[str, Any]:
        """
        Remove a provider.

        Args:
            name: Provider name

        Returns:
            Dict with success and error
        """
        response = await self._stub.RemoveProvider({"name": name})
        return {
            "success": response.success,
            "error": response.error,
        }

    async def set_default_model(
        self, provider: str, model: str
    ) -> Dict[str, Any]:
        """
        Set the default provider and model.

        Args:
            provider: Provider name
            model: Model ID

        Returns:
            Dict with success, error, provider, and model
        """
        response = await self._stub.SetDefaultModel({
            "provider": provider,
            "model": model,
        })
        return {
            "success": response.success,
            "error": response.error,
            "provider": response.provider,
            "model": response.model,
        }

    async def get_default_model(self) -> Dict[str, Optional[str]]:
        """
        Get the current default provider and model.

        Returns:
            Dict with provider and model (may be None)
        """
        response = await self._stub.GetDefaultModel({})
        return {
            "provider": response.provider or None,
            "model": response.model or None,
        }

    # =========================================================================
    # LSP (Language Server Protocol) Methods
    # =========================================================================

    async def start_lsp_server(
        self, language: str, root_uri: str = ""
    ) -> Dict[str, Any]:
        """
        Start a language server for a specific language.

        Args:
            language: Language name (e.g., "rust", "python", "typescript")
            root_uri: Workspace root URI (e.g., "file:///path/to/project")

        Returns:
            Dict with success, message, and server_info
        """
        response = await self._stub.StartLspServer({
            "language": language,
            "root_uri": root_uri,
        })
        server_info = None
        if response.server_info:
            server_info = {
                "language": response.server_info.language,
                "name": response.server_info.name,
                "version": response.server_info.version or None,
                "running": response.server_info.running,
            }
        return {
            "success": response.success,
            "message": response.message,
            "server_info": server_info,
        }

    async def stop_lsp_server(self, language: str) -> bool:
        """
        Stop a running language server.

        Args:
            language: Language name

        Returns:
            True if successful
        """
        response = await self._stub.StopLspServer({"language": language})
        return response.success

    async def list_lsp_servers(self) -> List[Dict[str, Any]]:
        """
        List all running language servers.

        Returns:
            List of server info dicts
        """
        response = await self._stub.ListLspServers({})
        return [
            {
                "language": s.language,
                "name": s.name,
                "version": s.version or None,
                "running": s.running,
            }
            for s in response.servers
        ]

    async def lsp_hover(
        self, file_path: str, line: int, column: int
    ) -> Dict[str, Any]:
        """
        Get hover information at a specific position.

        Args:
            file_path: Path to the file
            line: Line number (0-indexed)
            column: Column number (0-indexed)

        Returns:
            Dict with found, content, and range
        """
        response = await self._stub.LspHover({
            "file_path": file_path,
            "line": line,
            "column": column,
        })
        range_info = None
        if response.range:
            range_info = {
                "start": {"line": response.range.start.line, "character": response.range.start.character},
                "end": {"line": response.range.end.line, "character": response.range.end.character},
            }
        return {
            "found": response.found,
            "content": response.content,
            "range": range_info,
        }

    async def lsp_definition(
        self, file_path: str, line: int, column: int
    ) -> List[Dict[str, Any]]:
        """
        Go to definition at a specific position.

        Args:
            file_path: Path to the file
            line: Line number (0-indexed)
            column: Column number (0-indexed)

        Returns:
            List of location dicts with uri and range
        """
        response = await self._stub.LspDefinition({
            "file_path": file_path,
            "line": line,
            "column": column,
        })
        return [self._proto_to_lsp_location(loc) for loc in response.locations]

    async def lsp_references(
        self, file_path: str, line: int, column: int, include_declaration: bool = False
    ) -> List[Dict[str, Any]]:
        """
        Find all references at a specific position.

        Args:
            file_path: Path to the file
            line: Line number (0-indexed)
            column: Column number (0-indexed)
            include_declaration: Include the declaration in results

        Returns:
            List of location dicts with uri and range
        """
        response = await self._stub.LspReferences({
            "file_path": file_path,
            "line": line,
            "column": column,
            "include_declaration": include_declaration,
        })
        return [self._proto_to_lsp_location(loc) for loc in response.locations]

    async def lsp_symbols(
        self, query: str, limit: int = 20
    ) -> List[Dict[str, Any]]:
        """
        Search for symbols in the workspace.

        Args:
            query: Search query
            limit: Maximum number of results

        Returns:
            List of symbol dicts with name, kind, location, and container_name
        """
        response = await self._stub.LspSymbols({
            "query": query,
            "limit": limit,
        })
        return [
            {
                "name": s.name,
                "kind": s.kind,
                "location": self._proto_to_lsp_location(s.location) if s.location else None,
                "container_name": s.container_name or None,
            }
            for s in response.symbols
        ]

    async def lsp_diagnostics(
        self, file_path: Optional[str] = None
    ) -> List[Dict[str, Any]]:
        """
        Get diagnostics for a file.

        Args:
            file_path: Path to the file (optional, returns all if not specified)

        Returns:
            List of diagnostic dicts
        """
        request = {}
        if file_path:
            request["file_path"] = file_path
        response = await self._stub.LspDiagnostics(request)
        return [
            {
                "uri": d.uri,
                "range": {
                    "start": {"line": d.range.start.line, "character": d.range.start.character},
                    "end": {"line": d.range.end.line, "character": d.range.end.character},
                } if d.range else None,
                "severity": d.severity,
                "message": d.message,
                "code": d.code or None,
                "source": d.source or None,
            }
            for d in response.diagnostics
        ]

    def _proto_to_lsp_location(self, loc: Any) -> Dict[str, Any]:
        """Convert proto LspLocation to dict."""
        range_info = None
        if loc.range:
            range_info = {
                "start": {"line": loc.range.start.line, "character": loc.range.start.character},
                "end": {"line": loc.range.end.line, "character": loc.range.end.character},
            }
        return {
            "uri": loc.uri,
            "range": range_info,
        }

    # =========================================================================
    # Cron (Scheduled Tasks)
    # =========================================================================

    async def list_cron_jobs(self) -> List[CronJob]:
        """
        List all cron jobs.

        Returns:
            List of CronJob objects
        """
        response = await self._stub.ListCronJobs({})
        return [self._proto_to_cron_job(j) for j in response.jobs]

    async def create_cron_job(
        self,
        name: str,
        schedule: str,
        command: str,
        timeout_ms: Optional[int] = None,
    ) -> Dict[str, Any]:
        """
        Create a new cron job.

        Args:
            name: Job name
            schedule: Schedule expression (cron syntax or natural language)
            command: Command to execute
            timeout_ms: Execution timeout in milliseconds (default: 60000)

        Returns:
            Dict with success, job, and error fields
        """
        request = {
            "name": name,
            "schedule": schedule,
            "command": command,
        }
        if timeout_ms is not None:
            request["timeout_ms"] = timeout_ms
        response = await self._stub.CreateCronJob(request)
        return {
            "success": response.success,
            "job": self._proto_to_cron_job(response.job) if response.job else None,
            "error": response.error,
        }

    async def get_cron_job(
        self, id: Optional[str] = None, name: Optional[str] = None
    ) -> Optional[CronJob]:
        """
        Get a cron job by ID or name.

        Args:
            id: Job ID
            name: Job name (alternative to ID)

        Returns:
            CronJob or None if not found
        """
        request = {}
        if id:
            request["id"] = id
        if name:
            request["name"] = name
        response = await self._stub.GetCronJob(request)
        return self._proto_to_cron_job(response.job) if response.job else None

    async def update_cron_job(
        self,
        id: str,
        schedule: Optional[str] = None,
        command: Optional[str] = None,
        timeout_ms: Optional[int] = None,
    ) -> Dict[str, Any]:
        """
        Update a cron job.

        Args:
            id: Job ID
            schedule: New schedule expression
            command: New command
            timeout_ms: New timeout

        Returns:
            Dict with success, job, and error fields
        """
        request = {"id": id}
        if schedule is not None:
            request["schedule"] = schedule
        if command is not None:
            request["command"] = command
        if timeout_ms is not None:
            request["timeout_ms"] = timeout_ms
        response = await self._stub.UpdateCronJob(request)
        return {
            "success": response.success,
            "job": self._proto_to_cron_job(response.job) if response.job else None,
            "error": response.error,
        }

    async def pause_cron_job(self, id: str) -> Dict[str, Any]:
        """
        Pause a cron job.

        Args:
            id: Job ID

        Returns:
            Dict with success, job, and error fields
        """
        response = await self._stub.PauseCronJob({"id": id})
        return {
            "success": response.success,
            "job": self._proto_to_cron_job(response.job) if response.job else None,
            "error": response.error,
        }

    async def resume_cron_job(self, id: str) -> Dict[str, Any]:
        """
        Resume a paused cron job.

        Args:
            id: Job ID

        Returns:
            Dict with success, job, and error fields
        """
        response = await self._stub.ResumeCronJob({"id": id})
        return {
            "success": response.success,
            "job": self._proto_to_cron_job(response.job) if response.job else None,
            "error": response.error,
        }

    async def delete_cron_job(self, id: str) -> Dict[str, Any]:
        """
        Delete a cron job.

        Args:
            id: Job ID

        Returns:
            Dict with success and error fields
        """
        response = await self._stub.DeleteCronJob({"id": id})
        return {
            "success": response.success,
            "error": response.error,
        }

    async def get_cron_history(
        self, id: str, limit: Optional[int] = None
    ) -> List[CronExecution]:
        """
        Get execution history for a cron job.

        Args:
            id: Job ID
            limit: Max records to return (default: 10)

        Returns:
            List of CronExecution objects
        """
        request = {"id": id}
        if limit is not None:
            request["limit"] = limit
        response = await self._stub.GetCronHistory(request)
        return [self._proto_to_cron_execution(e) for e in response.executions]

    async def run_cron_job(self, id: str) -> Dict[str, Any]:
        """
        Manually run a cron job.

        Args:
            id: Job ID

        Returns:
            Dict with success, execution, and error fields
        """
        response = await self._stub.RunCronJob({"id": id})
        return {
            "success": response.success,
            "execution": self._proto_to_cron_execution(response.execution) if response.execution else None,
            "error": response.error,
        }

    async def parse_cron_schedule(self, input: str) -> Dict[str, Any]:
        """
        Parse natural language schedule to cron expression.

        Args:
            input: Natural language or cron expression

        Returns:
            Dict with success, cron_expression, description, and error fields
        """
        response = await self._stub.ParseCronSchedule({"input": input})
        return {
            "success": response.success,
            "cron_expression": response.cron_expression,
            "description": response.description,
            "error": response.error,
        }

    def _proto_to_cron_job(self, proto: Any) -> CronJob:
        """Convert proto to CronJob."""
        status_map = {
            0: CronJobStatus.UNKNOWN,
            1: CronJobStatus.ACTIVE,
            2: CronJobStatus.PAUSED,
            3: CronJobStatus.RUNNING,
        }
        return CronJob(
            id=proto.id,
            name=proto.name,
            schedule=proto.schedule,
            command=proto.command,
            status=status_map.get(proto.status, CronJobStatus.UNKNOWN),
            timeout_ms=proto.timeout_ms,
            created_at=proto.created_at,
            updated_at=proto.updated_at,
            last_run=proto.last_run if proto.last_run else None,
            next_run=proto.next_run if proto.next_run else None,
            run_count=proto.run_count,
            fail_count=proto.fail_count,
            working_dir=proto.working_dir if proto.working_dir else None,
        )

    def _proto_to_cron_execution(self, proto: Any) -> CronExecution:
        """Convert proto to CronExecution."""
        status_map = {
            0: CronExecutionStatus.UNKNOWN,
            1: CronExecutionStatus.SUCCESS,
            2: CronExecutionStatus.FAILED,
            3: CronExecutionStatus.TIMEOUT,
            4: CronExecutionStatus.CANCELLED,
        }
        return CronExecution(
            id=proto.id,
            job_id=proto.job_id,
            status=status_map.get(proto.status, CronExecutionStatus.UNKNOWN),
            started_at=proto.started_at,
            ended_at=proto.ended_at if proto.ended_at else None,
            duration_ms=proto.duration_ms if proto.duration_ms else None,
            exit_code=proto.exit_code if proto.exit_code else None,
            stdout=proto.stdout,
            stderr=proto.stderr,
            error=proto.error if proto.error else None,
        )

    # =========================================================================
    # Planning & Goal Tracking
    # =========================================================================

    async def create_plan(
        self, session_id: str, prompt: str, context: Optional[str] = None
    ) -> ExecutionPlan:
        """Create an execution plan.

        Args:
            session_id: Session ID
            prompt: The task to plan for
            context: Optional additional context

        Returns:
            ExecutionPlan object
        """
        request = {
            "session_id": session_id,
            "prompt": prompt,
        }
        if context:
            request["context"] = context
        response = await self._stub.CreatePlan(request)
        return self._proto_to_execution_plan(response.plan)

    async def get_plan(self, session_id: str, plan_id: str) -> ExecutionPlan:
        """Get an existing plan.

        Args:
            session_id: Session ID
            plan_id: Plan ID

        Returns:
            ExecutionPlan object
        """
        response = await self._stub.GetPlan({
            "session_id": session_id,
            "plan_id": plan_id,
        })
        return self._proto_to_execution_plan(response.plan)

    async def extract_goal(self, session_id: str, prompt: str) -> AgentGoal:
        """Extract goal from prompt.

        Args:
            session_id: Session ID
            prompt: The prompt to extract goal from

        Returns:
            AgentGoal object
        """
        response = await self._stub.ExtractGoal({
            "session_id": session_id,
            "prompt": prompt,
        })
        return self._proto_to_agent_goal(response.goal)

    async def check_goal_achievement(
        self, session_id: str, goal: AgentGoal, current_state: str
    ) -> Dict[str, Any]:
        """Check if goal is achieved.

        Args:
            session_id: Session ID
            goal: The goal to check
            current_state: Current state description

        Returns:
            Dict with achieved, progress, and remaining_criteria
        """
        response = await self._stub.CheckGoalAchievement({
            "session_id": session_id,
            "goal": self._agent_goal_to_proto(goal),
            "current_state": current_state,
        })
        return {
            "achieved": response.achieved,
            "progress": response.progress,
            "remaining_criteria": list(response.remaining_criteria),
        }

    # =========================================================================
    # Memory System
    # =========================================================================

    async def store_memory(
        self, session_id: str, memory: MemoryItem
    ) -> Dict[str, Any]:
        """Store a memory item.

        Args:
            session_id: Session ID
            memory: Memory item to store

        Returns:
            Dict with success and memory_id
        """
        response = await self._stub.StoreMemory({
            "session_id": session_id,
            "memory": self._memory_item_to_proto(memory),
        })
        return {
            "success": response.success,
            "memory_id": response.memory_id,
        }

    async def retrieve_memory(
        self, session_id: str, memory_id: str
    ) -> Optional[MemoryItem]:
        """Retrieve a memory by ID.

        Args:
            session_id: Session ID
            memory_id: Memory ID

        Returns:
            MemoryItem or None if not found
        """
        response = await self._stub.RetrieveMemory({
            "session_id": session_id,
            "memory_id": memory_id,
        })
        if response.memory:
            return self._proto_to_memory_item(response.memory)
        return None

    async def search_memories(
        self,
        session_id: str,
        query: Optional[str] = None,
        tags: Optional[List[str]] = None,
        limit: int = 10,
        recent_only: bool = False,
        min_importance: Optional[float] = None,
    ) -> Dict[str, Any]:
        """Search memories.

        Args:
            session_id: Session ID
            query: Search query
            tags: Tags to filter by
            limit: Maximum results
            recent_only: Only return recent memories
            min_importance: Minimum importance threshold

        Returns:
            Dict with memories and total_count
        """
        request = {
            "session_id": session_id,
            "limit": limit,
            "recent_only": recent_only,
        }
        if query:
            request["query"] = query
        if tags:
            request["tags"] = tags
        if min_importance is not None:
            request["min_importance"] = min_importance
        response = await self._stub.SearchMemories(request)
        return {
            "memories": [self._proto_to_memory_item(m) for m in response.memories],
            "total_count": response.total_count,
        }

    async def get_memory_stats(self, session_id: str) -> MemoryStats:
        """Get memory statistics.

        Args:
            session_id: Session ID

        Returns:
            MemoryStats object
        """
        response = await self._stub.GetMemoryStats({"session_id": session_id})
        return MemoryStats(
            long_term_count=response.stats.long_term_count,
            short_term_count=response.stats.short_term_count,
            working_count=response.stats.working_count,
        )

    async def clear_memories(
        self,
        session_id: str,
        clear_long_term: bool = False,
        clear_short_term: bool = False,
        clear_working: bool = False,
    ) -> Dict[str, Any]:
        """Clear memories.

        Args:
            session_id: Session ID
            clear_long_term: Clear long-term memories
            clear_short_term: Clear short-term memories
            clear_working: Clear working memories

        Returns:
            Dict with success and cleared_count
        """
        response = await self._stub.ClearMemories({
            "session_id": session_id,
            "clear_long_term": clear_long_term,
            "clear_short_term": clear_short_term,
            "clear_working": clear_working,
        })
        return {
            "success": response.success,
            "cleared_count": response.cleared_count,
        }

    async def delete_memory(
        self, session_id: str, memory_id: str
    ) -> Dict[str, Any]:
        """Delete a specific memory by ID.

        Args:
            session_id: Session ID
            memory_id: Memory ID to delete

        Returns:
            Dict with success
        """
        response = await self._stub.DeleteMemory({
            "session_id": session_id,
            "memory_id": memory_id,
        })
        return {
            "success": response.success,
        }

    async def summarize_memories(
        self,
        session_id: str,
        tags: Optional[List[str]] = None,
        limit: int = 50,
        recent_hours: Optional[int] = None,
    ) -> Dict[str, Any]:
        """Summarize memories using LLM.

        Args:
            session_id: Session ID
            tags: Optional tags to filter memories
            limit: Maximum memories to include
            recent_hours: Only include memories from the last N hours

        Returns:
            Dict with summary, memory_count, stored, and summary_memory_id
        """
        request = {
            "session_id": session_id,
            "limit": limit,
        }
        if tags:
            request["tags"] = tags
        if recent_hours is not None:
            request["recent_hours"] = recent_hours
        response = await self._stub.SummarizeMemories(request)
        return {
            "summary": response.summary,
            "memory_count": response.memory_count,
            "stored": response.stored,
            "summary_memory_id": response.summary_memory_id,
        }

    # =========================================================================
    # Observability
    # =========================================================================

    async def get_tool_metrics(
        self, session_id: str = "", tool_name: str = ""
    ) -> Dict[str, Any]:
        """Get tool usage metrics.

        Args:
            session_id: Optional session ID to filter by
            tool_name: Optional tool name to filter by (empty = all tools)

        Returns:
            Dict with tools (list of ToolStats), total_calls, total_duration_ms
        """
        response = await self._stub.GetToolMetrics({
            "session_id": session_id,
            "tool_name": tool_name,
        })
        tools = []
        for t in response.tools:
            tools.append({
                "tool_name": t.tool_name,
                "total_calls": t.total_calls,
                "success_count": t.success_count,
                "failure_count": t.failure_count,
                "total_duration_ms": t.total_duration_ms,
                "avg_duration_ms": t.avg_duration_ms,
                "min_duration_ms": t.min_duration_ms,
                "max_duration_ms": t.max_duration_ms,
            })
        return {
            "tools": tools,
            "total_calls": response.total_calls,
            "total_duration_ms": response.total_duration_ms,
        }

    async def get_cost_summary(
        self,
        session_id: str = "",
        model: str = "",
        start_date: str = "",
        end_date: str = "",
    ) -> Dict[str, Any]:
        """Get LLM cost summary.

        Args:
            session_id: Optional session ID to filter by
            model: Optional model name to filter by
            start_date: Optional start date (YYYY-MM-DD)
            end_date: Optional end date (YYYY-MM-DD)

        Returns:
            Dict with total_cost_usd, token counts, call_count, by_model, by_day
        """
        response = await self._stub.GetCostSummary({
            "session_id": session_id,
            "model": model,
            "start_date": start_date,
            "end_date": end_date,
        })
        by_model = []
        for m in response.by_model:
            by_model.append({
                "model": m.model,
                "cost_usd": m.cost_usd,
                "prompt_tokens": m.prompt_tokens,
                "completion_tokens": m.completion_tokens,
                "call_count": m.call_count,
            })
        by_day = []
        for d in response.by_day:
            by_day.append({
                "date": d.date,
                "cost_usd": d.cost_usd,
                "call_count": d.call_count,
            })
        return {
            "total_cost_usd": response.total_cost_usd,
            "total_prompt_tokens": response.total_prompt_tokens,
            "total_completion_tokens": response.total_completion_tokens,
            "total_tokens": response.total_tokens,
            "call_count": response.call_count,
            "by_model": by_model,
            "by_day": by_day,
        }

    # =========================================================================
    # MCP (Model Context Protocol)
    # =========================================================================

    async def register_mcp_server(
        self, config: McpServerConfig
    ) -> Dict[str, Any]:
        """Register an MCP server.

        Args:
            config: MCP server configuration

        Returns:
            Dict with success and message
        """
        response = await self._stub.RegisterMcpServer({
            "config": self._mcp_server_config_to_proto(config),
        })
        return {
            "success": response.success,
            "message": response.message,
        }

    async def connect_mcp_server(self, name: str) -> Dict[str, Any]:
        """Connect to an MCP server.

        Args:
            name: Server name

        Returns:
            Dict with success, message, and tool_names
        """
        response = await self._stub.ConnectMcpServer({"name": name})
        return {
            "success": response.success,
            "message": response.message,
            "tool_names": list(response.tool_names),
        }

    async def disconnect_mcp_server(self, name: str) -> bool:
        """Disconnect from an MCP server.

        Args:
            name: Server name

        Returns:
            True if successful
        """
        response = await self._stub.DisconnectMcpServer({"name": name})
        return response.success

    async def list_mcp_servers(self) -> List[McpServerInfo]:
        """List all MCP servers.

        Returns:
            List of McpServerInfo objects
        """
        response = await self._stub.ListMcpServers({})
        return [
            McpServerInfo(
                name=s.name,
                connected=s.connected,
                enabled=s.enabled,
                tool_count=s.tool_count,
                error=s.error or None,
            )
            for s in response.servers
        ]

    async def get_mcp_tools(
        self, server_name: Optional[str] = None
    ) -> List[McpToolInfo]:
        """Get MCP tools.

        Args:
            server_name: Optional server name to filter by

        Returns:
            List of McpToolInfo objects
        """
        request = {}
        if server_name:
            request["server_name"] = server_name
        response = await self._stub.GetMcpTools(request)
        return [
            McpToolInfo(
                full_name=t.full_name,
                server_name=t.server_name,
                tool_name=t.tool_name,
                description=t.description,
                input_schema=t.input_schema,
            )
            for t in response.tools
        ]

    # =========================================================================
    # Agentic Loop
    # =========================================================================

    async def agentic_generate(
        self,
        session_id: str,
        prompt: str,
        strategy: AgenticStrategy = AgenticStrategy.AUTO,
        max_steps: int = 50,
        reflection: bool = True,
        planning: bool = False,
    ) -> AgenticGenerateResponse:
        """Run an agentic generation loop (unary).

        Args:
            session_id: Session ID
            prompt: The task prompt
            strategy: Agentic strategy to use
            max_steps: Maximum loop iterations
            reflection: Enable reflection after tool failures
            planning: Enable planning before execution

        Returns:
            AgenticGenerateResponse
        """
        strategy_map = {
            AgenticStrategy.AUTO: 0,
            AgenticStrategy.DIRECT: 1,
            AgenticStrategy.PLANNED: 2,
            AgenticStrategy.ITERATIVE: 3,
            AgenticStrategy.PARALLEL: 4,
        }
        response = await self._stub.AgenticGenerate({
            "session_id": session_id,
            "prompt": prompt,
            "strategy": strategy_map.get(strategy, 0),
            "max_steps": max_steps,
            "reflection": reflection,
            "planning": planning,
        })
        return AgenticGenerateResponse(
            session_id=response.session_id,
            text=response.text,
            steps=[self._proto_to_agentic_step(s) for s in response.steps],
            tool_calls=list(response.tool_calls),
            usage=response.usage,
            finish_reason=response.finish_reason,
            plan=self._proto_to_execution_plan(response.plan) if response.plan else None,
        )

    async def stream_agentic_generate(
        self,
        session_id: str,
        prompt: str,
        strategy: AgenticStrategy = AgenticStrategy.AUTO,
        max_steps: int = 50,
        reflection: bool = True,
        planning: bool = False,
    ) -> Iterator[AgenticGenerateEvent]:
        """Run an agentic generation loop (streaming).

        Args:
            session_id: Session ID
            prompt: The task prompt
            strategy: Agentic strategy to use
            max_steps: Maximum loop iterations
            reflection: Enable reflection after tool failures
            planning: Enable planning before execution

        Yields:
            AgenticGenerateEvent objects
        """
        strategy_map = {
            AgenticStrategy.AUTO: 0,
            AgenticStrategy.DIRECT: 1,
            AgenticStrategy.PLANNED: 2,
            AgenticStrategy.ITERATIVE: 3,
            AgenticStrategy.PARALLEL: 4,
        }
        async for event in self._stub.StreamAgenticGenerate({
            "session_id": session_id,
            "prompt": prompt,
            "strategy": strategy_map.get(strategy, 0),
            "max_steps": max_steps,
            "reflection": reflection,
            "planning": planning,
        }):
            yield AgenticGenerateEvent(
                type=event.type,
                session_id=event.session_id,
                text=event.text,
                metadata=dict(event.metadata) if event.metadata else {},
                done=event.done,
            )

    # =========================================================================
    # Delegation
    # =========================================================================

    async def delegate(
        self,
        session_id: str,
        agent_name: str,
        task: str,
        max_steps: int = 50,
        allowed_tools: Optional[List[str]] = None,
    ) -> DelegateResponse:
        """Delegate a task to a subagent (unary).

        Args:
            session_id: Session ID
            agent_name: Built-in agent name ("explore", "plan", "general")
            task: Task description
            max_steps: Max steps for subagent
            allowed_tools: Tool whitelist for subagent

        Returns:
            DelegateResponse
        """
        response = await self._stub.Delegate({
            "session_id": session_id,
            "agent_name": agent_name,
            "task": task,
            "max_steps": max_steps,
            "allowed_tools": allowed_tools or [],
        })
        return DelegateResponse(
            session_id=response.session_id,
            agent_session_id=response.agent_session_id,
            text=response.text,
            steps=[self._proto_to_agentic_step(s) for s in response.steps],
            tool_calls=list(response.tool_calls),
            usage=response.usage,
            finish_reason=response.finish_reason,
        )

    async def stream_delegate(
        self,
        session_id: str,
        agent_name: str,
        task: str,
        max_steps: int = 50,
        allowed_tools: Optional[List[str]] = None,
    ) -> Iterator[AgenticGenerateEvent]:
        """Delegate a task to a subagent (streaming).

        Args:
            session_id: Session ID
            agent_name: Built-in agent name ("explore", "plan", "general")
            task: Task description
            max_steps: Max steps for subagent
            allowed_tools: Tool whitelist for subagent

        Yields:
            AgenticGenerateEvent objects
        """
        async for event in self._stub.StreamDelegate({
            "session_id": session_id,
            "agent_name": agent_name,
            "task": task,
            "max_steps": max_steps,
            "allowed_tools": allowed_tools or [],
        }):
            yield AgenticGenerateEvent(
                type=event.type,
                session_id=event.session_id,
                text=event.text,
                metadata=dict(event.metadata) if event.metadata else {},
                done=event.done,
            )

    # =========================================================================
    # Queue Statistics
    # =========================================================================

    async def get_queue_stats(self, session_id: str) -> QueueStats:
        """Get queue statistics for a session.

        Args:
            session_id: Session ID

        Returns:
            QueueStats
        """
        response = await self._stub.GetQueueStats({"session_id": session_id})
        return QueueStats(
            control=self._proto_to_lane_stats(response.control) if response.control else None,
            query=self._proto_to_lane_stats(response.query) if response.query else None,
            execute=self._proto_to_lane_stats(response.execute) if response.execute else None,
            generate=self._proto_to_lane_stats(response.generate) if response.generate else None,
            dead_letters=response.dead_letters,
        )

    # =========================================================================
    # Batch Skill Loading
    # =========================================================================

    async def load_skills_from_dir(
        self,
        session_id: str,
        directory: str,
        recursive: bool = False,
    ) -> Dict[str, Any]:
        """Load skills from a directory.

        Args:
            session_id: Session ID
            directory: Directory path containing skill files
            recursive: Recurse into subdirectories

        Returns:
            Dict with success, loaded_skills, and errors
        """
        response = await self._stub.LoadSkillsFromDir({
            "session_id": session_id,
            "directory": directory,
            "recursive": recursive,
        })
        return {
            "success": response.success,
            "loaded_skills": list(response.loaded_skills),
            "errors": list(response.errors),
        }

    # =========================================================================
    # Helper Methods
    # =========================================================================

    def _session_config_to_proto(self, config: SessionConfig) -> Dict[str, Any]:
        """Convert SessionConfig to proto format."""
        result = {
            "name": config.name,
            "workspace": config.workspace,
        }
        if config.system_prompt:
            result["system_prompt"] = config.system_prompt
        if config.max_context_length:
            result["max_context_length"] = config.max_context_length
        if config.auto_compact is not None:
            result["auto_compact"] = config.auto_compact
        if config.storage_type is not None:
            # Map enum value to proto int; use .value to normalize aliases
            storage_value_map = {
                "STORAGE_TYPE_UNSPECIFIED": 0,
                "STORAGE_TYPE_MEMORY": 1,
                "STORAGE_TYPE_FILE": 2,
            }
            result["storage_type"] = storage_value_map.get(config.storage_type.value, 0)
        return result

    def _message_to_proto(self, msg: Message) -> Dict[str, Any]:
        """Convert Message to proto format."""
        return {
            "role": msg.role.value,
            "content": msg.content,
            "metadata": msg.metadata,
        }

    def _todo_to_proto(self, todo: Todo) -> Dict[str, Any]:
        """Convert Todo to proto format."""
        return {
            "id": todo.id,
            "content": todo.content,
            "status": todo.status,
            "priority": todo.priority,
        }

    def _proto_to_todo(self, proto: Any) -> Todo:
        """Convert proto to Todo."""
        return Todo(
            id=proto.id,
            content=proto.content,
            status=proto.status,
            priority=proto.priority,
        )

    def _provider_to_proto(self, provider: ProviderInfo) -> Dict[str, Any]:
        """Convert ProviderInfo to proto format."""
        return {
            "name": provider.name,
            "api_key": provider.api_key,
            "base_url": provider.base_url,
            "models": [self._model_to_proto(m) for m in provider.models],
        }

    def _model_to_proto(self, model: ModelInfo) -> Dict[str, Any]:
        """Convert ModelInfo to proto format."""
        result = {
            "id": model.id,
            "name": model.name,
            "family": model.family,
            "api_key": model.api_key,
            "base_url": model.base_url,
            "attachment": model.attachment,
            "reasoning": model.reasoning,
            "tool_call": model.tool_call,
            "temperature": model.temperature,
            "release_date": model.release_date,
        }
        if model.modalities:
            result["modalities"] = {
                "input": model.modalities.input,
                "output": model.modalities.output,
            }
        if model.cost:
            result["cost"] = {
                "input": model.cost.input,
                "output": model.cost.output,
                "cache_read": model.cost.cache_read,
                "cache_write": model.cost.cache_write,
            }
        if model.limit:
            result["limit"] = {
                "context": model.limit.context,
                "output": model.limit.output,
            }
        return result

    def _proto_to_provider(self, proto: Any) -> ProviderInfo:
        """Convert proto to ProviderInfo."""
        return ProviderInfo(
            name=proto.name,
            api_key=proto.api_key or None,
            base_url=proto.base_url or None,
            models=[self._proto_to_model(m) for m in proto.models],
        )

    def _proto_to_model(self, proto: Any) -> ModelInfo:
        """Convert proto to ModelInfo."""
        modalities = None
        if proto.modalities:
            modalities = ModelModalitiesInfo(
                input=list(proto.modalities.input),
                output=list(proto.modalities.output),
            )
        cost = None
        if proto.cost:
            cost = ModelCostInfo(
                input=proto.cost.input,
                output=proto.cost.output,
                cache_read=proto.cost.cache_read,
                cache_write=proto.cost.cache_write,
            )
        limit = None
        if proto.limit:
            limit = ModelLimitInfo(
                context=proto.limit.context,
                output=proto.limit.output,
            )
        return ModelInfo(
            id=proto.id,
            name=proto.name,
            family=proto.family,
            api_key=proto.api_key or None,
            base_url=proto.base_url or None,
            attachment=proto.attachment,
            reasoning=proto.reasoning,
            tool_call=proto.tool_call,
            temperature=proto.temperature,
            release_date=proto.release_date or None,
            modalities=modalities,
            cost=cost,
            limit=limit,
        )

    def _confirmation_policy_to_proto(
        self, policy: ConfirmationPolicy
    ) -> Dict[str, Any]:
        """Convert ConfirmationPolicy to proto format."""
        return {
            "enabled": policy.enabled,
            "auto_approve_tools": policy.auto_approve_tools,
            "require_confirm_tools": policy.require_confirm_tools,
            "default_timeout_ms": policy.default_timeout_ms,
            "timeout_action": self._timeout_action_to_proto(policy.timeout_action),
            "yolo_lanes": [self._session_lane_to_proto(lane) for lane in policy.yolo_lanes],
        }

    def _proto_to_confirmation_policy(self, proto: Any) -> ConfirmationPolicy:
        """Convert proto to ConfirmationPolicy."""
        return ConfirmationPolicy(
            enabled=proto.enabled,
            auto_approve_tools=list(proto.auto_approve_tools),
            require_confirm_tools=list(proto.require_confirm_tools),
            default_timeout_ms=proto.default_timeout_ms,
            timeout_action=self._proto_to_timeout_action(proto.timeout_action),
            yolo_lanes=[self._proto_to_session_lane(lane) for lane in proto.yolo_lanes],
        )

    def _session_lane_to_proto(self, lane: SessionLane) -> int:
        """Convert SessionLane to proto format."""
        lane_map = {
            SessionLane.UNKNOWN: 0,
            SessionLane.CONTROL: 1,
            SessionLane.QUERY: 2,
            SessionLane.EXECUTE: 3,
            SessionLane.GENERATE: 4,
        }
        return lane_map.get(lane, 0)

    def _proto_to_session_lane(self, value: int) -> SessionLane:
        """Convert proto to SessionLane."""
        lane_map = {
            0: SessionLane.UNKNOWN,
            1: SessionLane.CONTROL,
            2: SessionLane.QUERY,
            3: SessionLane.EXECUTE,
            4: SessionLane.GENERATE,
        }
        return lane_map.get(value, SessionLane.UNKNOWN)

    def _timeout_action_to_proto(self, action: TimeoutAction) -> int:
        """Convert TimeoutAction to proto format."""
        action_map = {
            TimeoutAction.UNKNOWN: 0,
            TimeoutAction.REJECT: 1,
            TimeoutAction.AUTO_APPROVE: 2,
        }
        return action_map.get(action, 0)

    def _proto_to_timeout_action(self, value: int) -> TimeoutAction:
        """Convert proto to TimeoutAction."""
        action_map = {
            0: TimeoutAction.UNKNOWN,
            1: TimeoutAction.REJECT,
            2: TimeoutAction.AUTO_APPROVE,
        }
        return action_map.get(value, TimeoutAction.UNKNOWN)

    def _lane_handler_config_to_proto(
        self, config: LaneHandlerConfig
    ) -> Dict[str, Any]:
        """Convert LaneHandlerConfig to proto format."""
        mode_map = {
            TaskHandlerMode.UNKNOWN: 0,
            TaskHandlerMode.INTERNAL: 1,
            TaskHandlerMode.EXTERNAL: 2,
            TaskHandlerMode.HYBRID: 3,
        }
        return {
            "mode": mode_map.get(config.mode, 0),
            "timeout_ms": config.timeout_ms,
        }

    def _proto_to_lane_handler_config(self, proto: Any) -> LaneHandlerConfig:
        """Convert proto to LaneHandlerConfig."""
        mode_map = {
            0: TaskHandlerMode.UNKNOWN,
            1: TaskHandlerMode.INTERNAL,
            2: TaskHandlerMode.EXTERNAL,
            3: TaskHandlerMode.HYBRID,
        }
        return LaneHandlerConfig(
            mode=mode_map.get(proto.mode, TaskHandlerMode.UNKNOWN),
            timeout_ms=proto.timeout_ms,
        )

    def _permission_policy_to_proto(self, policy: PermissionPolicy) -> Dict[str, Any]:
        """Convert PermissionPolicy to proto format."""
        from .types import PermissionDecision
        decision_map = {
            PermissionDecision.UNKNOWN: 0,
            PermissionDecision.ALLOW: 1,
            PermissionDecision.DENY: 2,
            PermissionDecision.ASK: 3,
        }
        return {
            "deny": [{"rule": r.rule} for r in policy.deny],
            "allow": [{"rule": r.rule} for r in policy.allow],
            "ask": [{"rule": r.rule} for r in policy.ask],
            "default_decision": decision_map.get(policy.default_decision, 0),
            "enabled": policy.enabled,
        }

    def _proto_to_permission_policy(self, proto: Any) -> PermissionPolicy:
        """Convert proto to PermissionPolicy."""
        from .types import PermissionDecision, PermissionRule
        decision_map = {
            0: PermissionDecision.UNKNOWN,
            1: PermissionDecision.ALLOW,
            2: PermissionDecision.DENY,
            3: PermissionDecision.ASK,
        }
        return PermissionPolicy(
            deny=[PermissionRule(rule=r.rule) for r in proto.deny],
            allow=[PermissionRule(rule=r.rule) for r in proto.allow],
            ask=[PermissionRule(rule=r.rule) for r in proto.ask],
            default_decision=decision_map.get(
                proto.default_decision, PermissionDecision.UNKNOWN
            ),
            enabled=proto.enabled,
        )

    # =========================================================================
    # Planning Helper Methods
    # =========================================================================

    def _proto_to_execution_plan(self, proto: Any) -> ExecutionPlan:
        """Convert proto to ExecutionPlan."""
        complexity_map = {
            0: Complexity.UNKNOWN,
            1: Complexity.SIMPLE,
            2: Complexity.MEDIUM,
            3: Complexity.COMPLEX,
            4: Complexity.VERY_COMPLEX,
        }
        return ExecutionPlan(
            goal=proto.goal,
            steps=[self._proto_to_plan_step(s) for s in proto.steps],
            complexity=complexity_map.get(proto.complexity, Complexity.UNKNOWN),
            required_tools=list(proto.required_tools),
            estimated_steps=proto.estimated_steps,
        )

    def _proto_to_plan_step(self, proto: Any) -> PlanStep:
        """Convert proto to PlanStep."""
        status_map = {
            0: StepStatus.UNKNOWN,
            1: StepStatus.PENDING,
            2: StepStatus.IN_PROGRESS,
            3: StepStatus.COMPLETED,
            4: StepStatus.FAILED,
            5: StepStatus.SKIPPED,
        }
        return PlanStep(
            id=proto.id,
            description=proto.description,
            tool=proto.tool or None,
            dependencies=list(proto.dependencies),
            status=status_map.get(proto.status, StepStatus.UNKNOWN),
            success_criteria=list(proto.success_criteria),
        )

    def _proto_to_agent_goal(self, proto: Any) -> AgentGoal:
        """Convert proto to AgentGoal."""
        return AgentGoal(
            description=proto.description,
            success_criteria=list(proto.success_criteria),
            progress=proto.progress,
            achieved=proto.achieved,
            created_at=proto.created_at,
            achieved_at=proto.achieved_at if proto.achieved_at else None,
        )

    def _agent_goal_to_proto(self, goal: AgentGoal) -> Dict[str, Any]:
        """Convert AgentGoal to proto format."""
        result = {
            "description": goal.description,
            "success_criteria": goal.success_criteria,
            "progress": goal.progress,
            "achieved": goal.achieved,
            "created_at": goal.created_at,
        }
        if goal.achieved_at:
            result["achieved_at"] = goal.achieved_at
        return result

    # =========================================================================
    # Memory Helper Methods
    # =========================================================================

    def _memory_item_to_proto(self, memory: MemoryItem) -> Dict[str, Any]:
        """Convert MemoryItem to proto format."""
        memory_type_map = {
            MemoryType.UNKNOWN: 0,
            MemoryType.EPISODIC: 1,
            MemoryType.SEMANTIC: 2,
            MemoryType.PROCEDURAL: 3,
            MemoryType.WORKING: 4,
        }
        result = {
            "id": memory.id,
            "content": memory.content,
            "timestamp": memory.timestamp,
            "importance": memory.importance,
            "tags": memory.tags,
            "memory_type": memory_type_map.get(memory.memory_type, 0),
            "metadata": memory.metadata,
            "access_count": memory.access_count,
        }
        if memory.last_accessed:
            result["last_accessed"] = memory.last_accessed
        return result

    def _proto_to_memory_item(self, proto: Any) -> MemoryItem:
        """Convert proto to MemoryItem."""
        memory_type_map = {
            0: MemoryType.UNKNOWN,
            1: MemoryType.EPISODIC,
            2: MemoryType.SEMANTIC,
            3: MemoryType.PROCEDURAL,
            4: MemoryType.WORKING,
        }
        return MemoryItem(
            id=proto.id,
            content=proto.content,
            timestamp=proto.timestamp,
            importance=proto.importance,
            tags=list(proto.tags),
            memory_type=memory_type_map.get(proto.memory_type, MemoryType.UNKNOWN),
            metadata=dict(proto.metadata),
            access_count=proto.access_count,
            last_accessed=proto.last_accessed if proto.last_accessed else None,
        )

    # =========================================================================
    # MCP Helper Methods
    # =========================================================================

    def _mcp_server_config_to_proto(self, config: McpServerConfig) -> Dict[str, Any]:
        """Convert McpServerConfig to proto format."""
        result = {
            "name": config.name,
            "enabled": config.enabled,
            "env": config.env,
        }
        if config.transport:
            transport = {}
            if config.transport.stdio:
                transport["stdio"] = {
                    "command": config.transport.stdio.command,
                    "args": config.transport.stdio.args,
                }
            if config.transport.http:
                transport["http"] = {
                    "url": config.transport.http.url,
                    "headers": config.transport.http.headers,
                }
            result["transport"] = transport
        return result

    # =========================================================================
    # Agentic / Delegation Helper Methods
    # =========================================================================

    def _proto_to_agentic_step(self, proto: Any) -> AgenticStep:
        """Convert proto to AgenticStep."""
        return AgenticStep(
            step_number=proto.step_number,
            text=proto.text,
            tool_calls=list(proto.tool_calls),
            finish_reason=proto.finish_reason,
            duration_ms=proto.duration_ms,
        )

    def _proto_to_lane_stats(self, proto: Any) -> LaneStats:
        """Convert proto to LaneStats."""
        return LaneStats(
            pending=proto.pending,
            active=proto.active,
            external=proto.external,
            completed=proto.completed,
            failed=proto.failed,
        )
