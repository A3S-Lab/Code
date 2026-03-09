/**
 * Example: Using A3S Code Framework with DI and Decorators (TypeScript)
 *
 * This example demonstrates the complete framework usage:
 * - Dependency Injection
 * - Decorators
 * - AgentFactory
 * - Guards, Interceptors, Pipes, Filters
 */

import 'reflect-metadata';
import {
  AgentFactory,
  Injectable,
  Scope,
  Guard,
  Interceptor,
  Pipe,
  ExceptionFilter,
  Middleware,
  MiddlewareResult,
} from '../framework';

// 1. Define services (DI)
@Injectable({ scope: Scope.SINGLETON })
class LoggerService {
  constructor(private level: string = 'info') {}

  log(msg: string): void {
    console.log(`[${this.level.toUpperCase()}] ${msg}`);
  }
}

@Injectable({ scope: Scope.SINGLETON })
class CacheService {
  private cache = new Map<string, string>();

  get(key: string): string | undefined {
    return this.cache.get(key);
  }

  set(key: string, value: string): void {
    this.cache.set(key, value);
  }
}

@Injectable({ scope: Scope.SCOPED })
class SessionStore {
  private data = new Map<string, string>();

  constructor(
    private logger: LoggerService,
    private cache: CacheService
  ) {}

  save(key: string, value: string): void {
    this.logger.log(`Saving ${key}`);
    this.data.set(key, value);
    this.cache.set(key, value);
  }
}

// 2. Define middleware (bottom layer - Express-like)
@Injectable()
@Middleware()
class LoggingMiddleware {
  constructor(private logger: LoggerService) {}

  async handle(ctx: any): Promise<{ type: string }> {
    this.logger.log(`Request: ${ctx.sessionId || 'unknown'}`);
    return { type: MiddlewareResult.CONTINUE };
  }
}

@Injectable()
@Middleware()
class TimingMiddleware {
  constructor(private logger: LoggerService) {}

  async handle(ctx: any): Promise<{ type: string }> {
    const start = Date.now();
    this.logger.log('⏱️  Timing started');
    ctx.metadata.set('timing_start', start.toString());
    return { type: MiddlewareResult.CONTINUE };
  }
}

// 3. Define guards (high-level - NestJS-like)
@Injectable()
@Guard()
class AuthGuard {
  constructor(private logger: LoggerService) {}

  async canActivate(ctx: any): Promise<boolean> {
    this.logger.log(`🔐 Checking auth for session ${ctx.sessionId || 'unknown'}`);
    return true; // Mock: always allow
  }
}

@Injectable()
@Guard()
class RateLimitGuard {
  private maxRequests = 100;

  constructor(
    private logger: LoggerService,
    private cache: CacheService
  ) {}

  async canActivate(ctx: any): Promise<boolean> {
    const sessionId = ctx.sessionId || 'unknown';
    const count = parseInt(this.cache.get(`rate_limit:${sessionId}`) || '0');

    if (count >= this.maxRequests) {
      this.logger.log(`🚫 Rate limit exceeded for ${sessionId}`);
      return false;
    }

    this.cache.set(`rate_limit:${sessionId}`, (count + 1).toString());
    this.logger.log(`✅ Rate limit check passed (${count + 1}/${this.maxRequests})`);
    return true;
  }
}

// 4. Define interceptors
@Injectable()
@Interceptor()
class CacheInterceptor {
  constructor(
    private cache: CacheService,
    private logger: LoggerService
  ) {}

  async before(ctx: any): Promise<void> {
    const sessionId = ctx.sessionId || 'unknown';
    const cached = this.cache.get(`result:${sessionId}`);
    if (cached) {
      this.logger.log(`💾 Cache hit for ${sessionId}`);
      ctx.cachedResult = cached;
    }
  }

  async after(ctx: any, result: any): Promise<void> {
    const sessionId = ctx.sessionId || 'unknown';
    this.logger.log(`💾 Caching result for ${sessionId}`);
    this.cache.set(`result:${sessionId}`, JSON.stringify(result));
  }
}

// 5. Define pipes
@Injectable()
@Pipe()
class ValidationPipe {
  constructor(private logger: LoggerService) {}

  transform(value: any): any {
    this.logger.log(`✅ Validating: ${JSON.stringify(value)}`);
    return value; // Mock: just return the value
  }
}

// 6. Define exception filters
@Injectable()
@ExceptionFilter()
class RetryFilter {
  private maxRetries = 3;

  constructor(private logger: LoggerService) {}

  async catch(error: Error, ctx: any): Promise<any> {
    this.logger.log(`🔄 Retry filter caught error: ${error.message}`);
    return null; // Mock: no recovery
  }
}

async function main() {
  console.log('🚀 A3S Code Framework Example (TypeScript)\n');

  // Create factory (NestJS-like)
  const factory = AgentFactory.create('agent.hcl');

  // Register providers (DI)
  console.log('📦 Registering providers...');
  factory.provide(LoggerService, 'debug');
  factory.provide(CacheService);
  factory.provide(SessionStore); // Auto-inject LoggerService + CacheService

  // Register middleware (bottom layer - Express-like)
  console.log('🔧 Registering middleware...');
  factory.use(LoggingMiddleware); // Auto-inject LoggerService
  factory.use(TimingMiddleware); // Auto-inject LoggerService

  // Register guards (high-level - NestJS-like)
  console.log('🛡️  Registering guards...');
  factory.useGuard(AuthGuard); // Auto-inject LoggerService
  factory.useGuard(RateLimitGuard); // Auto-inject LoggerService + CacheService

  // Register interceptors
  console.log('🔍 Registering interceptors...');
  factory.useInterceptor(CacheInterceptor); // Auto-inject CacheService + LoggerService

  // Register pipes
  console.log('🔀 Registering pipes...');
  factory.usePipe(ValidationPipe); // Auto-inject LoggerService

  // Register filters
  console.log('🚨 Registering filters...');
  factory.useFilter(RetryFilter); // Auto-inject LoggerService

  console.log('\n✅ All components registered with automatic dependency injection!\n');

  // Create session
  console.log('📝 Creating session...');
  const session = factory.session('/project');

  // Run session
  console.log('\n🎬 Running session...\n');
  const result = await session.run('List all files');

  console.log(`\n📊 Result:`, result);
  console.log('\n🎉 Framework working correctly!');
}

main().catch(console.error);
