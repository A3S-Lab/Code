"""
Middleware adapters

Adapts high-level abstractions (Guards, Interceptors, Pipes, Filters)
to the low-level Middleware interface.
"""

from typing import Any, Optional


class MiddlewareResult:
    """Middleware execution result"""

    Continue = "continue"
    Abort = "abort"


class GuardMiddleware:
    """Adapter: Guard → Middleware

    Converts a Guard (can_activate) into a Middleware (handle).
    """

    def __init__(self, guard):
        self.guard = guard

    async def handle(self, ctx):
        """Execute guard's can_activate method"""
        if hasattr(self.guard, "can_activate"):
            allowed = await self.guard.can_activate(ctx)
            if not allowed:
                return {
                    "type": MiddlewareResult.Abort,
                    "reason": f"Guard {self.guard.__class__.__name__} denied access",
                }

        return {"type": MiddlewareResult.Continue}


class InterceptorMiddleware:
    """Adapter: Interceptor → Middleware

    Converts an Interceptor (before/after) into a Middleware (handle).
    """

    def __init__(self, interceptor):
        self.interceptor = interceptor

    async def handle(self, ctx):
        """Execute interceptor's before method"""
        if hasattr(self.interceptor, "before"):
            await self.interceptor.before(ctx)

        # Mark that we need to execute after hook later
        if not hasattr(ctx, "_interceptor_after_hooks"):
            ctx._interceptor_after_hooks = []

        ctx._interceptor_after_hooks.append(self.interceptor)

        return {"type": MiddlewareResult.Continue}


class PipeMiddleware:
    """Adapter: Pipe → Middleware

    Converts a Pipe (transform) into a Middleware (handle).
    """

    def __init__(self, pipe):
        self.pipe = pipe

    async def handle(self, ctx):
        """Execute pipe's transform method"""
        if hasattr(self.pipe, "transform") and hasattr(ctx, "tool_call"):
            if ctx.tool_call:
                # Transform tool arguments
                ctx.tool_call["args"] = self.pipe.transform(ctx.tool_call["args"])

        return {"type": MiddlewareResult.Continue}


class FilterMiddleware:
    """Adapter: Exception Filter → Middleware

    Converts an Exception Filter (catch) into error handling logic.
    """

    def __init__(self, filter_instance):
        self.filter = filter_instance

    async def handle(self, ctx):
        """Register filter for error handling"""
        if not hasattr(ctx, "_exception_filters"):
            ctx._exception_filters = []

        ctx._exception_filters.append(self.filter)

        return {"type": MiddlewareResult.Continue}


class MiddlewareAdapter:
    """Adapter: Python Middleware → Rust Middleware

    Wraps a Python middleware to be compatible with Rust Core.
    """

    def __init__(self, python_middleware):
        self.python_middleware = python_middleware

    async def handle(self, ctx):
        """Call Python middleware's handle method"""
        if hasattr(self.python_middleware, "handle"):
            result = await self.python_middleware.handle(ctx)
            return result

        return {"type": MiddlewareResult.Continue}
