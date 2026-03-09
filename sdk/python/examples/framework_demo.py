"""
Example: Using A3S Code Framework with DI and Decorators

This example demonstrates the complete framework usage:
- Dependency Injection
- Decorators
- AgentFactory
- Guards, Interceptors, Pipes, Filters
"""

import asyncio

from a3s_code_framework import (
    AgentFactory,
    exception_filter,
    guard,
    injectable,
    interceptor,
    middleware,
    pipe,
)


# 1. Define services (DI)
@injectable(scope="singleton")
class LoggerService:
    """Logger service (singleton)"""

    def __init__(self, level: str = "info"):
        self.level = level

    def log(self, msg: str):
        print(f"[{self.level.upper()}] {msg}")


@injectable(scope="singleton")
class CacheService:
    """Cache service (singleton)"""

    def __init__(self):
        self._cache = {}

    def get(self, key: str):
        return self._cache.get(key)

    def set(self, key: str, value: str):
        self._cache[key] = value


@injectable(scope="scoped")
class SessionStore:
    """Session store (scoped - one per session)"""

    def __init__(self, logger: LoggerService, cache: CacheService):
        self.logger = logger
        self.cache = cache
        self.data = {}

    def save(self, key: str, value: str):
        self.logger.log(f"Saving {key}")
        self.data[key] = value
        self.cache.set(key, value)


# 2. Define middleware (bottom layer - Express-like)
@injectable()
@middleware
class LoggingMiddleware:
    """Logging middleware"""

    def __init__(self, logger: LoggerService):
        self.logger = logger

    async def handle(self, ctx):
        self.logger.log(f"Request: {ctx.get('session_id', 'unknown')}")
        return {"type": "continue"}


@injectable()
@middleware
class TimingMiddleware:
    """Timing middleware"""

    def __init__(self, logger: LoggerService):
        self.logger = logger

    async def handle(self, ctx):
        import time

        start = time.time()
        self.logger.log("⏱️  Timing started")

        # Store start time in context
        ctx["timing_start"] = start

        return {"type": "continue"}


# 3. Define guards (high-level - NestJS-like)
@injectable()
@guard
class AuthGuard:
    """Authentication guard"""

    def __init__(self, logger: LoggerService):
        self.logger = logger

    async def can_activate(self, ctx) -> bool:
        self.logger.log(f"🔐 Checking auth for session {ctx.get('session_id', 'unknown')}")
        # Mock: always allow
        return True


@injectable()
@guard
class RateLimitGuard:
    """Rate limiting guard"""

    def __init__(self, logger: LoggerService, cache: CacheService):
        self.logger = logger
        self.cache = cache
        self.max_requests = 100

    async def can_activate(self, ctx) -> bool:
        session_id = ctx.get("session_id", "unknown")
        count = int(self.cache.get(f"rate_limit:{session_id}") or 0)

        if count >= self.max_requests:
            self.logger.log(f"🚫 Rate limit exceeded for {session_id}")
            return False

        self.cache.set(f"rate_limit:{session_id}", str(count + 1))
        self.logger.log(f"✅ Rate limit check passed ({count + 1}/{self.max_requests})")
        return True


# 4. Define interceptors
@injectable()
@interceptor
class CacheInterceptor:
    """Cache interceptor"""

    def __init__(self, cache: CacheService, logger: LoggerService):
        self.cache = cache
        self.logger = logger

    async def before(self, ctx):
        session_id = ctx.get("session_id", "unknown")
        cached = self.cache.get(f"result:{session_id}")
        if cached:
            self.logger.log(f"💾 Cache hit for {session_id}")
            ctx["cached_result"] = cached

    async def after(self, ctx, result):
        session_id = ctx.get("session_id", "unknown")
        self.logger.log(f"💾 Caching result for {session_id}")
        self.cache.set(f"result:{session_id}", str(result))


# 5. Define pipes
@injectable()
@pipe
class ValidationPipe:
    """Validation pipe"""

    def __init__(self, logger: LoggerService):
        self.logger = logger

    def transform(self, value):
        self.logger.log(f"✅ Validating: {value}")
        # Mock: just return the value
        return value


# 6. Define exception filters
@injectable()
@exception_filter
class RetryFilter:
    """Retry filter"""

    def __init__(self, logger: LoggerService):
        self.logger = logger
        self.max_retries = 3

    async def catch(self, error, ctx):
        self.logger.log(f"🔄 Retry filter caught error: {error}")
        # Mock: return None (no recovery)
        return None


async def main():
    print("🚀 A3S Code Framework Example\n")

    # Create factory (NestJS-like)
    factory = AgentFactory.create("agent.hcl")

    # Register providers (DI)
    print("📦 Registering providers...")
    factory.provide(LoggerService, level="debug")
    factory.provide(CacheService)
    factory.provide(SessionStore)  # Auto-inject LoggerService + CacheService

    # Register middleware (bottom layer - Express-like)
    print("🔧 Registering middleware...")
    factory.use(LoggingMiddleware)  # Auto-inject LoggerService
    factory.use(TimingMiddleware)  # Auto-inject LoggerService

    # Register guards (high-level - NestJS-like)
    print("🛡️  Registering guards...")
    factory.use_guard(AuthGuard)  # Auto-inject LoggerService
    factory.use_guard(RateLimitGuard)  # Auto-inject LoggerService + CacheService

    # Register interceptors
    print("🔍 Registering interceptors...")
    factory.use_interceptor(CacheInterceptor)  # Auto-inject CacheService + LoggerService

    # Register pipes
    print("🔀 Registering pipes...")
    factory.use_pipe(ValidationPipe)  # Auto-inject LoggerService

    # Register filters
    print("🚨 Registering filters...")
    factory.use_filter(RetryFilter)  # Auto-inject LoggerService

    print("\n✅ All components registered with automatic dependency injection!\n")

    # Create session
    print("📝 Creating session...")
    session = factory.session("/project")

    # Run session
    print("\n🎬 Running session...\n")
    result = await session.run("List all files")

    print(f"\n📊 Result: {result}\n")
    print("🎉 Framework working correctly!")


if __name__ == "__main__":
    asyncio.run(main())
