# A3S Code Middleware System

Express-like middleware architecture for A3S Code, inspired by NestJS.

## Architecture

```
┌─────────────────────────────────────────────────────────┐
│  Layer 3: SDK Framework (Python/TypeScript)             │
│  - AgentFactory (NestJS-like factory pattern)           │
│  - Dependency Injection (DI Container)                  │
│  - Decorators (@injectable, @middleware, @guard)        │
│  - High-level abstractions (Guards, Interceptors, etc.) │
└─────────────────────────────────────────────────────────┘
                          ↓
┌─────────────────────────────────────────────────────────┐
│  Layer 2: SDK Bindings (PyO3/napi-rs)                   │
│  - Rust → Python/TypeScript bridge                      │
│  - Type conversions                                      │
│  - Minimal logic (pure bridging layer)                  │
└─────────────────────────────────────────────────────────┘
                          ↓
┌─────────────────────────────────────────────────────────┐
│  Layer 1: Rust Core (Lightweight Express-like) ✅       │
│  - MiddlewarePipeline (app.use() pattern)              │
│  - Middleware trait (simple, composable)                │
│  - Built-in middleware (Logging, Security, Permission)  │
│  - No DI, no decorators (keep it simple)               │
└─────────────────────────────────────────────────────────┘
```

## Layer 1: Rust Core (✅ Implemented)

### Core Components

- **`Middleware` trait**: Define custom middleware
- **`MiddlewarePipeline`**: Execute middleware in sequence
- **`MiddlewareContext`**: Shared context passed through pipeline
- **`MiddlewareResult`**: Continue, Modified, or Abort

### Built-in Middleware

- **`LoggingMiddleware`**: Log execution events
- **`SecurityMiddleware`**: Input taint tracking
- **`PermissionMiddleware`**: Tool permission enforcement

### Usage Example

```rust
use a3s_code_core::middleware::{
    MiddlewarePipeline, LoggingMiddleware, SecurityMiddleware, PermissionMiddleware
};
use std::sync::Arc;

// Create pipeline
let mut pipeline = MiddlewarePipeline::new();

// Register middleware (Express-like app.use())
pipeline.use_middleware(Arc::new(LoggingMiddleware::new("debug")));
pipeline.use_middleware(Arc::new(SecurityMiddleware::new(security_provider)));
pipeline.use_middleware(Arc::new(PermissionMiddleware::new(permission_checker)));

// Execute pipeline
let mut ctx = MiddlewareContext::new(session_id, workspace);
pipeline.execute(&mut ctx).await?;
```

### Custom Middleware

```rust
use a3s_code_core::middleware::{Middleware, MiddlewareContext, MiddlewareResult};
use async_trait::async_trait;

struct TimingMiddleware;

#[async_trait]
impl Middleware for TimingMiddleware {
    async fn handle(&self, ctx: &mut MiddlewareContext) -> Result<MiddlewareResult> {
        let start = std::time::Instant::now();

        // Store timing in metadata
        ctx.set_metadata("timing_start".into(), serde_json::json!(start.elapsed().as_millis()));

        Ok(MiddlewareResult::Continue)
    }
}
```

## Layer 2: SDK Bindings (🚧 TODO)

Bridge Rust middleware to Python/TypeScript:

- PyO3 bindings for Python
- napi-rs bindings for TypeScript
- Simple type conversions (no business logic)

## Layer 3: SDK Framework (🚧 TODO)

High-level framework with DI and decorators:

### Python SDK

```python
from a3s_code.framework import AgentFactory, injectable

@injectable(scope='singleton')
class LoggerService:
    def log(self, msg: str):
        print(f"[LOG] {msg}")

@injectable()
class LoggingMiddleware:
    def __init__(self, logger: LoggerService):
        self.logger = logger

    async def handle(self, ctx):
        self.logger.log(f"Request: {ctx.session_id}")
        return MiddlewareResult.Continue

# Create factory (NestJS-like)
factory = AgentFactory.create('agent.hcl')

# Register providers (DI)
factory.provide(LoggerService)

# Register middleware (auto DI)
factory.use(LoggingMiddleware)

# Create session
session = factory.session('/project')
```

### TypeScript SDK

```typescript
import { AgentFactory, Injectable } from 'a3s-code/framework';

@Injectable({ scope: 'singleton' })
class LoggerService {
  log(msg: string) {
    console.log(`[LOG] ${msg}`);
  }
}

@Injectable()
class LoggingMiddleware {
  constructor(private logger: LoggerService) {}

  async handle(ctx: MiddlewareContext) {
    this.logger.log(`Request: ${ctx.sessionId}`);
    return MiddlewareResult.Continue;
  }
}

// Create factory (NestJS-like)
const factory = AgentFactory.create('agent.hcl');

// Register providers (DI)
factory.provide(LoggerService);

// Register middleware (auto DI)
factory.use(LoggingMiddleware);

// Create session
const session = factory.session('/project');
```

## Design Principles

### 1. Layered Architecture

- **Layer 1 (Rust Core)**: Lightweight, Express-like, no DI
- **Layer 2 (Bindings)**: Pure bridging, no business logic
- **Layer 3 (Framework)**: DI + decorators + high-level abstractions

### 2. Progressive Enhancement

Users can choose which layer to use:

- **Rust Core**: Maximum performance, low-level control
- **SDK Bindings**: Cross-language, simple API
- **SDK Framework**: Developer-friendly, DI + decorators

### 3. Separation of Concerns

- **Middleware**: Request/response interception
- **Guards**: Authorization and access control
- **Interceptors**: Request/response transformation
- **Pipes**: Data validation and transformation
- **Filters**: Exception handling

## Comparison with NestJS

| NestJS | A3S Code | Layer |
|--------|----------|-------|
| Express/Fastify | Rust Core Middleware | Layer 1 |
| `@Injectable()` | `@injectable()` | Layer 3 |
| `NestFactory.create()` | `AgentFactory.create()` | Layer 3 |
| `app.use(middleware)` | `factory.use(middleware)` | Layer 3 |
| `@UseGuards()` | `factory.useGuard()` | Layer 3 |
| `@UseInterceptors()` | `factory.useInterceptor()` | Layer 3 |

## Implementation Status

- ✅ **Layer 1: Rust Core** - Implemented and tested
- 🚧 **Layer 2: SDK Bindings** - TODO
- 🚧 **Layer 3: SDK Framework** - TODO

## Examples

- `core/examples/middleware_demo.rs` - Rust Core middleware usage

## Next Steps

1. Implement Layer 2 (SDK Bindings)
   - PyO3 bindings for Python
   - napi-rs bindings for TypeScript

2. Implement Layer 3 (SDK Framework)
   - DI Container
   - Decorators
   - AgentFactory
   - Guards, Interceptors, Pipes, Filters
