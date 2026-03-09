"""
A3S Code Framework - NestJS-inspired framework for Python

Provides:
- Dependency Injection (DI Container)
- Decorators (@injectable, @middleware, @guard)
- AgentFactory (NestJS-like factory pattern)
- High-level abstractions (Guards, Interceptors, Pipes, Filters)
"""

from .adapters import (
    FilterMiddleware,
    GuardMiddleware,
    InterceptorMiddleware,
    MiddlewareAdapter,
    MiddlewareResult,
    PipeMiddleware,
)
from .container import DIContainer, Scope, get_container, injectable
from .decorators import exception_filter, guard, interceptor, middleware, pipe
from .factory import AgentFactory
from .session import AgentSession

__version__ = "0.1.0"

__all__ = [
    # DI Container
    "DIContainer",
    "injectable",
    "Scope",
    "get_container",
    # Decorators
    "middleware",
    "guard",
    "interceptor",
    "pipe",
    "exception_filter",
    # Factory
    "AgentFactory",
    "AgentSession",
    # Adapters
    "MiddlewareAdapter",
    "GuardMiddleware",
    "InterceptorMiddleware",
    "PipeMiddleware",
    "FilterMiddleware",
    "MiddlewareResult",
]
