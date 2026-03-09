/**
 * Middleware adapters
 *
 * Adapts high-level abstractions (Guards, Interceptors, Pipes, Filters)
 * to the low-level Middleware interface.
 */

export class MiddlewareResult {
  static readonly CONTINUE = 'continue';
  static readonly ABORT = 'abort';
}

export interface ExecutionContext {
  sessionId: string;
  workspace?: string;
  toolName?: string;
  args?: any;
  metadata: Map<string, any>;
}

/**
 * Adapter: Guard → Middleware
 */
export class GuardMiddleware {
  constructor(private guard: any) {}

  async handle(ctx: ExecutionContext): Promise<{ type: string; reason?: string }> {
    if (typeof this.guard.canActivate === 'function') {
      const allowed = await this.guard.canActivate(ctx);
      if (!allowed) {
        return {
          type: MiddlewareResult.ABORT,
          reason: `Guard ${this.guard.constructor.name} denied access`,
        };
      }
    }

    return { type: MiddlewareResult.CONTINUE };
  }
}

/**
 * Adapter: Interceptor → Middleware
 */
export class InterceptorMiddleware {
  constructor(private interceptor: any) {}

  async handle(ctx: ExecutionContext): Promise<{ type: string }> {
    if (typeof this.interceptor.before === 'function') {
      await this.interceptor.before(ctx);
    }

    // Mark that we need to execute after hook later
    if (!(ctx as any)._interceptorAfterHooks) {
      (ctx as any)._interceptorAfterHooks = [];
    }
    (ctx as any)._interceptorAfterHooks.push(this.interceptor);

    return { type: MiddlewareResult.CONTINUE };
  }
}

/**
 * Adapter: Pipe → Middleware
 */
export class PipeMiddleware {
  constructor(private pipe: any) {}

  async handle(ctx: ExecutionContext): Promise<{ type: string }> {
    if (typeof this.pipe.transform === 'function' && ctx.args) {
      ctx.args = this.pipe.transform(ctx.args);
    }

    return { type: MiddlewareResult.CONTINUE };
  }
}

/**
 * Adapter: Exception Filter → Middleware
 */
export class FilterMiddleware {
  constructor(private filter: any) {}

  async handle(ctx: ExecutionContext): Promise<{ type: string }> {
    if (!(ctx as any)._exceptionFilters) {
      (ctx as any)._exceptionFilters = [];
    }
    (ctx as any)._exceptionFilters.push(this.filter);

    return { type: MiddlewareResult.CONTINUE };
  }
}

/**
 * Adapter: TypeScript Middleware → Rust Middleware
 */
export class MiddlewareAdapter {
  constructor(private middleware: any) {}

  async handle(ctx: ExecutionContext): Promise<{ type: string; reason?: string }> {
    if (typeof this.middleware.handle === 'function') {
      return await this.middleware.handle(ctx);
    }

    return { type: MiddlewareResult.CONTINUE };
  }
}
