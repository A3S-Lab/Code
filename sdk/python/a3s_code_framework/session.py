"""
AgentSession - Express-like session wrapper

Wraps the core session with middleware pipeline support.
"""

from typing import Any, Callable, Optional


class AgentSession:
    """Agent Session

    Express-like session wrapper with middleware pipeline.
    """

    def __init__(
        self,
        session_id: str,
        workspace: str,
    ):
        """Initialize session

        Args:
            session_id: Session ID
            workspace: Workspace directory path
        """
        self._session_id = session_id
        self._workspace = workspace

        # TODO: Initialize Rust core session when bindings are ready
        self._core_session = None

        # Middleware pipeline
        self._middleware = []

    def use(self, middleware: Callable) -> "AgentSession":
        """Add middleware to the pipeline (Express-style)

        Args:
            middleware: Middleware function or instance

        Returns:
            Self for chaining
        """
        self._middleware.append(middleware)
        return self

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
