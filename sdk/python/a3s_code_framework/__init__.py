"""
A3S Code Framework - Express-like middleware framework for Python

Provides:
- Middleware pipeline (Express-style)
- AgentSession (workspace-bound execution context)
- Middleware adapters (Guards, Interceptors, Pipes, Filters)
"""

from .adapters import (
    FilterMiddleware,
    GuardMiddleware,
    InterceptorMiddleware,
    MiddlewareAdapter,
    MiddlewareResult,
    PipeMiddleware,
)
from .session import AgentSession

__version__ = "0.1.0"

__all__ = [
    # Session
    "AgentSession",
    # Adapters
    "MiddlewareAdapter",
    "GuardMiddleware",
    "InterceptorMiddleware",
    "PipeMiddleware",
    "FilterMiddleware",
    "MiddlewareResult",
]
