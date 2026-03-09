"""
AgentFactory - NestJS-inspired factory for creating agents

Provides a fluent API for configuring agents with DI and middleware.
"""

import uuid
from typing import Any, Optional, Type

from .adapters import (
    FilterMiddleware,
    GuardMiddleware,
    InterceptorMiddleware,
    MiddlewareAdapter,
    PipeMiddleware,
)
from .container import DIContainer, Scope


class AgentFactory:
    """Agent Factory (NestJS-like)

    Creates and configures Agent instances with DI and middleware support.

    Example:
        factory = AgentFactory.create('agent.hcl')
        factory.provide(LoggerService)
        factory.use(LoggingMiddleware)
        factory.use_guard(AuthGuard)
        session = factory.session('/project')
    """

    def __init__(self, config_path: str):
        """Initialize factory

        Args:
            config_path: Path to agent configuration file
        """
        self._config_path = config_path
        self._container = DIContainer()
        self._middleware_classes = []
        self._guard_classes = []
        self._interceptor_classes = []
        self._pipe_classes = []
        self._filter_classes = []

        # Core agent will be initialized when needed
        self._core_agent = None

    @classmethod
    def create(cls, config_path: str) -> "AgentFactory":
        """Create a new AgentFactory instance

        Args:
            config_path: Path to agent configuration file

        Returns:
            AgentFactory instance
        """
        return cls(config_path)

    def provide(self, provider_cls: Type, **kwargs) -> "AgentFactory":
        """Register a provider (DI)

        Args:
            provider_cls: The provider class to register
            **kwargs: Constructor arguments

        Returns:
            Self for chaining
        """
        scope = getattr(provider_cls, "_injectable_scope", Scope.SINGLETON)
        self._container.register(provider_cls, scope, **kwargs)
        return self

    def use(self, middleware_cls: Type) -> "AgentFactory":
        """Register middleware (auto DI)

        Args:
            middleware_cls: The middleware class to register

        Returns:
            Self for chaining
        """
        self._middleware_classes.append(middleware_cls)

        # Auto-register if not already registered
        if middleware_cls not in self._container._providers:
            scope = getattr(middleware_cls, "_injectable_scope", Scope.SINGLETON)
            self._container.register(middleware_cls, scope)

        # Resolve dependencies and create instance
        middleware_instance = self._container.resolve(middleware_cls)

        # TODO: Register with Rust core when bindings are ready
        # For now, store for later use
        if not hasattr(self, "_middleware_instances"):
            self._middleware_instances = []
        self._middleware_instances.append(middleware_instance)

        return self

    def use_guard(self, guard_cls: Type) -> "AgentFactory":
        """Register guard (auto DI)

        Args:
            guard_cls: The guard class to register

        Returns:
            Self for chaining
        """
        self._guard_classes.append(guard_cls)

        # Auto-register if not already registered
        if guard_cls not in self._container._providers:
            scope = getattr(guard_cls, "_injectable_scope", Scope.SINGLETON)
            self._container.register(guard_cls, scope)

        # Resolve dependencies and create instance
        guard_instance = self._container.resolve(guard_cls)

        # Convert to middleware
        guard_middleware = GuardMiddleware(guard_instance)

        # TODO: Register with Rust core when bindings are ready
        if not hasattr(self, "_guard_instances"):
            self._guard_instances = []
        self._guard_instances.append(guard_middleware)

        return self

    def use_interceptor(self, interceptor_cls: Type) -> "AgentFactory":
        """Register interceptor (auto DI)

        Args:
            interceptor_cls: The interceptor class to register

        Returns:
            Self for chaining
        """
        self._interceptor_classes.append(interceptor_cls)

        # Auto-register if not already registered
        if interceptor_cls not in self._container._providers:
            scope = getattr(interceptor_cls, "_injectable_scope", Scope.SINGLETON)
            self._container.register(interceptor_cls, scope)

        # Resolve dependencies and create instance
        interceptor_instance = self._container.resolve(interceptor_cls)

        # Convert to middleware
        interceptor_middleware = InterceptorMiddleware(interceptor_instance)

        # TODO: Register with Rust core when bindings are ready
        if not hasattr(self, "_interceptor_instances"):
            self._interceptor_instances = []
        self._interceptor_instances.append(interceptor_middleware)

        return self

    def use_pipe(self, pipe_cls: Type) -> "AgentFactory":
        """Register pipe (auto DI)

        Args:
            pipe_cls: The pipe class to register

        Returns:
            Self for chaining
        """
        self._pipe_classes.append(pipe_cls)

        # Auto-register if not already registered
        if pipe_cls not in self._container._providers:
            scope = getattr(pipe_cls, "_injectable_scope", Scope.SINGLETON)
            self._container.register(pipe_cls, scope)

        # Resolve dependencies and create instance
        pipe_instance = self._container.resolve(pipe_cls)

        # Convert to middleware
        pipe_middleware = PipeMiddleware(pipe_instance)

        # TODO: Register with Rust core when bindings are ready
        if not hasattr(self, "_pipe_instances"):
            self._pipe_instances = []
        self._pipe_instances.append(pipe_middleware)

        return self

    def use_filter(self, filter_cls: Type) -> "AgentFactory":
        """Register exception filter (auto DI)

        Args:
            filter_cls: The filter class to register

        Returns:
            Self for chaining
        """
        self._filter_classes.append(filter_cls)

        # Auto-register if not already registered
        if filter_cls not in self._container._providers:
            scope = getattr(filter_cls, "_injectable_scope", Scope.SINGLETON)
            self._container.register(filter_cls, scope)

        # Resolve dependencies and create instance
        filter_instance = self._container.resolve(filter_cls)

        # Convert to middleware
        filter_middleware = FilterMiddleware(filter_instance)

        # TODO: Register with Rust core when bindings are ready
        if not hasattr(self, "_filter_instances"):
            self._filter_instances = []
        self._filter_instances.append(filter_middleware)

        return self

    def session(self, workspace: str):
        """Create a session (with DI scope)

        Args:
            workspace: Workspace directory path

        Returns:
            AgentSession instance
        """
        session_id = str(uuid.uuid4())

        # TODO: Create Rust core session when bindings are ready
        # For now, return a mock session
        from .session import AgentSession

        return AgentSession(
            session_id=session_id,
            workspace=workspace,
            container=self._container,
            scope_id=session_id,
        )

    def build(self):
        """Build the final agent (optional, for explicit building)

        Returns:
            Core agent instance
        """
        # TODO: Return Rust core agent when bindings are ready
        return self._core_agent
