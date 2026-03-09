# A3S Code Framework (TypeScript/Node.js SDK)

NestJS-inspired framework for TypeScript with Dependency Injection and Decorators.

## Features

- ✅ **Dependency Injection**: Automatic dependency resolution with three scopes (Singleton, Scoped, Transient)
- ✅ **Decorators**: `@Injectable`, `@Middleware`, `@Guard`, `@Interceptor`, `@Pipe`, `@ExceptionFilter`
- ✅ **AgentFactory**: NestJS-like factory pattern for creating agents
- ✅ **High-level Abstractions**: Guards, Interceptors, Pipes, Filters
- ✅ **Automatic Registration**: Components auto-register when used

## Installation

```bash
cd framework
npm install
```

## Quick Start

```typescript
import 'reflect-metadata';
import { AgentFactory, Injectable, Guard, Scope } from 'a3s-code-framework';

// 1. Define services (DI)
@Injectable({ scope: Scope.SINGLETON })
class LoggerService {
  log(msg: string) {
    console.log(`[LOG] ${msg}`);
  }
}

// 2. Define guards
@Injectable()
@Guard()
class AuthGuard {
  constructor(private logger: LoggerService) {}

  async canActivate(ctx: any): Promise<boolean> {
    this.logger.log('Checking auth...');
    return true;
  }
}

// 3. Create factory (NestJS-like)
const factory = AgentFactory.create('agent.hcl');

// 4. Register providers (DI)
factory.provide(LoggerService);

// 5. Register guards (auto DI)
factory.useGuard(AuthGuard); // LoggerService auto-injected

// 6. Create session
const session = factory.session('/project');

// 7. Run
const result = await session.run('List files');
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

```typescript
import { Injectable, Scope } from 'a3s-code-framework';

@Injectable({ scope: Scope.SINGLETON }) // Global singleton
class ConfigService {}

@Injectable({ scope: Scope.SCOPED }) // Per-session instance
class SessionStore {
  constructor(private config: ConfigService) {} // Auto-injected
}

@Injectable({ scope: Scope.TRANSIENT }) // New instance every time
class RequestHandler {}
```

### 2. Middleware (Express-like)

```typescript
import { Injectable, Middleware, MiddlewareResult } from 'a3s-code-framework';

@Injectable()
@Middleware()
class LoggingMiddleware {
  constructor(private logger: LoggerService) {}

  async handle(ctx: any) {
    this.logger.log(`Request: ${ctx.sessionId}`);
    return { type: MiddlewareResult.CONTINUE };
  }
}
```

### 3. Guards (Authorization)

```typescript
import { Injectable, Guard } from 'a3s-code-framework';

@Injectable()
@Guard()
class RateLimitGuard {
  constructor(private cache: CacheService) {}

  async canActivate(ctx: any): Promise<boolean> {
    // Check rate limit
    return true;
  }
}
```

### 4. Interceptors (Request/Response Transformation)

```typescript
import { Injectable, Interceptor } from 'a3s-code-framework';

@Injectable()
@Interceptor()
class CacheInterceptor {
  constructor(private cache: CacheService) {}

  async before(ctx: any) {
    // Check cache before execution
  }

  async after(ctx: any, result: any) {
    // Cache result after execution
  }
}
```

### 5. Pipes (Data Validation)

```typescript
import { Injectable, Pipe } from 'a3s-code-framework';

@Injectable()
@Pipe()
class ValidationPipe {
  transform(value: any): any {
    // Validate and transform data
    return value;
  }
}
```

### 6. Exception Filters (Error Handling)

```typescript
import { Injectable, ExceptionFilter } from 'a3s-code-framework';

@Injectable()
@ExceptionFilter()
class RetryFilter {
  async catch(error: Error, ctx: any) {
    // Handle error and retry
    return null;
  }
}
```

## Complete Example

See `examples/framework_demo.ts` for a complete example with:
- Multiple services with DI
- Middleware (Logging, Timing)
- Guards (Auth, RateLimit)
- Interceptors (Cache)
- Pipes (Validation)
- Filters (Retry)

Run the example:

```bash
npm run example
```

## Comparison with NestJS

| NestJS | A3S Code Framework |
|--------|-------------------|
| `NestFactory.create()` | `AgentFactory.create()` |
| `@Injectable()` | `@Injectable()` |
| `app.use(middleware)` | `factory.use(middleware)` |
| `@UseGuards()` | `factory.useGuard()` |
| `@UseInterceptors()` | `factory.useInterceptor()` |
| `@UsePipes()` | `factory.usePipe()` |
| `@UseFilters()` | `factory.useFilter()` |

## Implementation Status

- ✅ **DI Container**: Fully implemented with three scopes
- ✅ **Decorators**: All decorators implemented
- ✅ **AgentFactory**: Factory pattern with fluent API
- ✅ **Adapters**: Middleware adapters for Guards/Interceptors/Pipes/Filters
- ✅ **AgentSession**: Session wrapper with DI scope
- 🚧 **Rust Bindings**: TODO (Layer 2)
- 🚧 **Integration with Rust Core**: TODO (connect to middleware pipeline)

## Next Steps

1. Implement napi-rs bindings (Layer 2)
2. Connect AgentFactory to Rust Core middleware pipeline
3. Add comprehensive tests
4. Publish to npm

## License

MIT
