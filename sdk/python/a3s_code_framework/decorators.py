"""
Decorators for middleware, guards, interceptors, pipes, and filters

Provides NestJS-style decorators for marking classes.
"""

from typing import Type


def middleware(cls: Type) -> Type:
    """Decorator to mark a class as middleware

    Example:
        @middleware
        class LoggingMiddleware:
            async def handle(self, ctx):
                print(f"Request: {ctx.session_id}")
                return MiddlewareResult.Continue
    """
    cls._is_middleware = True
    return cls


def guard(cls: Type) -> Type:
    """Decorator to mark a class as a guard

    Example:
        @guard
        class AuthGuard:
            async def can_activate(self, ctx) -> bool:
                return True
    """
    cls._is_guard = True
    return cls


def interceptor(cls: Type) -> Type:
    """Decorator to mark a class as an interceptor

    Example:
        @interceptor
        class LoggingInterceptor:
            async def before(self, ctx):
                pass

            async def after(self, ctx, result):
                pass
    """
    cls._is_interceptor = True
    return cls


def pipe(cls: Type) -> Type:
    """Decorator to mark a class as a pipe

    Example:
        @pipe
        class ValidationPipe:
            def transform(self, value):
                return value
    """
    cls._is_pipe = True
    return cls


def exception_filter(cls: Type) -> Type:
    """Decorator to mark a class as an exception filter

    Example:
        @exception_filter
        class RetryFilter:
            async def catch(self, error, ctx):
                # Handle error
                return None
    """
    cls._is_exception_filter = True
    return cls
