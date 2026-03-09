"""
AgentSession - Session wrapper with DI support

Wraps the core session with DI container and scope management.
"""

from typing import Any, Optional

from .container import DIContainer


class AgentSession:
    """Agent Session

    Wraps the core session with DI container and scope management.
    """

    def __init__(
        self,
        session_id: str,
        workspace: str,
        container: DIContainer,
        scope_id: str,
    ):
        """Initialize framework session

        Args:
            session_id: Session ID
            workspace: Workspace directory path
            container: DI container
            scope_id: Scope ID for scoped providers
        """
        self._session_id = session_id
        self._workspace = workspace
        self._container = container
        self._scope_id = scope_id

        # TODO: Initialize Rust core session when bindings are ready
        self._core_session = None

        # Middleware instances
        self._guards = []
        self._interceptors = []
        self._pipes = []
        self._filters = []

    async def run(self, prompt: str) -> Any:
        """Run the session with a prompt

        Args:
            prompt: User prompt

        Returns:
            Session result
        """
        # TODO: Execute middleware pipeline
        # TODO: Call Rust core session.run()

        # For now, return a mock result
        return {
            "text": f"Mock response for: {prompt}",
            "session_id": self._session_id,
            "workspace": self._workspace,
        }

    def use_guard(self, guard_cls: type) -> "AgentSession":
        """Add a guard to this session

        Args:
            guard_cls: Guard class

        Returns:
            Self for chaining
        """
        guard_instance = self._container.resolve(guard_cls, self._scope_id)
        self._guards.append(guard_instance)
        return self

    def use_interceptor(self, interceptor_cls: type) -> "AgentSession":
        """Add an interceptor to this session

        Args:
            interceptor_cls: Interceptor class

        Returns:
            Self for chaining
        """
        interceptor_instance = self._container.resolve(interceptor_cls, self._scope_id)
        self._interceptors.append(interceptor_instance)
        return self

    def use_pipe(self, pipe_cls: type) -> "AgentSession":
        """Add a pipe to this session

        Args:
            pipe_cls: Pipe class

        Returns:
            Self for chaining
        """
        pipe_instance = self._container.resolve(pipe_cls, self._scope_id)
        self._pipes.append(pipe_instance)
        return self

    def use_filter(self, filter_cls: type) -> "AgentSession":
        """Add an exception filter to this session

        Args:
            filter_cls: Filter class

        Returns:
            Self for chaining
        """
        filter_instance = self._container.resolve(filter_cls, self._scope_id)
        self._filters.append(filter_instance)
        return self

    def __del__(self):
        """Cleanup: clear scoped instances"""
        self._container.clear_scope(self._scope_id)
