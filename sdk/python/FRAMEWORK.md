# A3S Code Framework (Python SDK)

NestJS-inspired framework for Python with Dependency Injection and Decorators.

## Features

- ✅ **Dependency Injection**: Automatic dependency resolution with three scopes (Singleton, Scoped, Transient)
- ✅ **Decorators**: `@injectable`, `@middleware`, `@guard`, `@interceptor`, `@pipe`, `@exception_filter`
- ✅ **AgentFactory**: NestJS-like factory pattern for creating agents
- ✅ **High-level Abstractions**: Guards, Interceptors, Pipes, Filters
- ✅ **Automatic Registration**: Components auto-register when used

## Installation

```bash
# Add to PYTHONPATH
export PYTHONPATH=/path/to/a3s/crates/code/sdk/python:$PYTHONPATH
```

## Quick Start

```python
from a3s_code_framework import AgentFactory, injectable, guard, interceptor

# 1. Define services (DI)
@injectable(scope='singleton')
class LoggerService:
    def log(self, msg: str):
        print(f"[LOG] {msg}")

# 2. Define guards
@injectable()
@guard
class AuthGuard:
    def __init__(self, logger: LoggerService):
        self.logger = logger

    async def can_activate(self, ctx) -> bool:
        self.logger.log("Checking auth...")
        return True

# 3. Create factory (NestJS-like)
factory = AgentFactory.create('agent.hcl')

# 4. Register providers (DI)
factory.provide(LoggerService)

# 5. Register guards (auto DI)
factory.use_guard(AuthGuard)  # LoggerService auto-injected

# 6. Create session
session = factory.session('/project')

# 7. Run
result = await session.run("List files")
```

## Architecture

```
AgentFactory (NestJS-like factory)
    ↓
DIContainer (Dependency Injection)
    ↓
Middleware/Guards/Interceptors/Pipes/Filters
    ↓
AgentSession (with DI scope)
```

## Components

### 1. Dependency Injection

```python
from a3s_code_framework import injectable, Scope

@injectable(scope='singleton')  # Global singleton
class ConfigService:
    pass

@injectable(scope='scoped')  # Per-session instance
class SessionStore:
    def __init__(self, config: ConfigService):  # Auto-injected
        self.config = config

@injectable(scope='transient')  # New instance every time
class RequestHandler:
    pass
```

### 2. Middleware (Express-like)

```python
from a3s_code_framework import injectable, middleware

@injectable()
@middleware
class LoggingMiddleware:
    def __init__(self, logger: LoggerService):
        self.logger = logger

    async def handle(self, ctx):
        self.logger.log(f"Request: {ctx['session_id']}")
        return {"type": "continue"}
```

### 3. Guards (Authorization)

```python
from a3s_code_framework import injectable, guard

@injectable()
@guard
class RateLimitGuard:
    def __init__(self, cache: CacheService):
        self.cache = cache

    async def can_activate(self, ctx) -> bool:
        # Check rate limit
        return True
```

### 4. Interceptors (Request/Response Transformation)

```python
from a3s_code_framework import injectable, interceptor

@injectable()
@interceptor
class CacheInterceptor:
    def __init__(self, cache: CacheService):
        self.cache = cache

    async def before(self, ctx):
        # Check cache before execution
        pass

    async def after(self, ctx, result):
        # Cache result after execution
        pass
```

### 5. Pipes (Data Validation)

```python
from a3s_code_framework import injectable, pipe

@injectable()
@pipe
class ValidationPipe:
    def transform(self, value):
        # Validate and transform data
        return value
```

### 6. Exception Filters (Error Handling)

```python
from a3s_code_framework import injectable, exception_filter

@injectable()
@exception_filter
class RetryFilter:
    async def catch(self, error, ctx):
        # Handle error and retry
        return None
```

## Complete Example

See `examples/framework_demo.py` for a complete example with:
- Multiple services with DI
- Middleware (Logging, Timing)
- Guards (Auth, RateLimit)
- Interceptors (Cache)
- Pipes (Validation)
- Filters (Retry)

Run the example:

```bash
cd /path/to/a3s/crates/code
PYTHONPATH=sdk/python:$PYTHONPATH python3 sdk/python/examples/framework_demo.py
```

## Comparison with NestJS

| NestJS | A3S Code Framework |
|--------|-------------------|
| `NestFactory.create()` | `AgentFactory.create()` |
| `@Injectable()` | `@injectable()` |
| `app.use(middleware)` | `factory.use(middleware)` |
| `@UseGuards()` | `factory.use_guard()` |
| `@UseInterceptors()` | `factory.use_interceptor()` |
| `@UsePipes()` | `factory.use_pipe()` |
| `@UseFilters()` | `factory.use_filter()` |

## Implementation Status

- ✅ **DI Container**: Fully implemented with three scopes
- ✅ **Decorators**: All decorators implemented
- ✅ **AgentFactory**: Factory pattern with fluent API
- ✅ **Adapters**: Middleware adapters for Guards/Interceptors/Pipes/Filters
- ✅ **AgentSession**: Session wrapper with DI scope
- 🚧 **Rust Bindings**: TODO (Layer 2)
- 🚧 **Integration with Rust Core**: TODO (connect to middleware pipeline)

## Next Steps

1. Implement Layer 2 (PyO3 bindings)
2. Connect AgentFactory to Rust Core middleware pipeline
3. Implement TypeScript SDK Framework (same architecture)
