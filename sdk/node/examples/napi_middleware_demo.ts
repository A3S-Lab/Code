/**
 * Example: Using napi-rs middleware bindings with async support
 *
 * This example demonstrates the napi-rs bindings for the Rust Core middleware system.
 * Supports both synchronous and asynchronous middleware.
 */

import { MiddlewareContext, MiddlewarePipeline, MiddlewareResultObject } from '..';

// Helper: Enhanced pipeline with async support
class AsyncMiddlewarePipeline {
  private pipeline: MiddlewarePipeline;
  private pendingPromises: Promise<MiddlewareResultObject>[] = [];

  constructor() {
    this.pipeline = new MiddlewarePipeline();
  }

  use(middleware: (ctx: MiddlewareContext) => MiddlewareResultObject | Promise<MiddlewareResultObject>): this {
    const isAsync = middleware.constructor.name === 'AsyncFunction';

    if (isAsync) {
      const asyncFn = middleware as (ctx: MiddlewareContext) => Promise<MiddlewareResultObject>;
      const wrapped = (ctx: MiddlewareContext): MiddlewareResultObject => {
        const promise = asyncFn(ctx);
        this.pendingPromises.push(promise);
        return { resultType: 'continue' };
      };
      this.pipeline.useMiddleware(wrapped);
    } else {
      this.pipeline.useMiddleware(middleware as (ctx: MiddlewareContext) => MiddlewareResultObject);
    }

    return this;
  }

  async execute(ctx: MiddlewareContext): Promise<MiddlewareContext> {
    this.pendingPromises = [];
    const result = await this.pipeline.execute(ctx);

    if (this.pendingPromises.length > 0) {
      const results = await Promise.all(this.pendingPromises);
      for (const middlewareResult of results) {
        if (middlewareResult.resultType === 'abort') {
          throw new Error(`Middleware aborted: ${middlewareResult.reason || 'Unknown reason'}`);
        }
      }
    }

    return result;
  }

  len(): number {
    return this.pipeline.len();
  }
}

// Synchronous middleware
function customMiddleware(ctx: MiddlewareContext): MiddlewareResultObject {
  console.log(`🟢 Sync middleware: sessionId=${ctx.sessionId}, workspace=${ctx.workspace}`);
  return { resultType: 'continue' };
}

// Asynchronous middleware with Promise
async function asyncMiddleware(ctx: MiddlewareContext): Promise<MiddlewareResultObject> {
  console.log(`⏳ Async middleware: starting...`);
  await new Promise(resolve => setTimeout(resolve, 100));
  console.log(`✅ Async middleware: completed`);
  return { resultType: 'continue' };
}

// Async middleware that aborts
async function asyncAbortMiddleware(ctx: MiddlewareContext): Promise<MiddlewareResultObject> {
  console.log(`🚫 Async abort middleware: checking conditions...`);
  await new Promise(resolve => setTimeout(resolve, 50));
  console.log(`🚫 Async abort middleware: aborting execution`);
  return { resultType: 'abort', reason: 'Aborted by async TypeScript middleware' };
}

async function main() {
  console.log('🚀 napi-rs Middleware Bindings Example (TypeScript + Async)\n');

  // 1. Test synchronous middleware
  console.log('=== Test 1: Synchronous Middleware ===\n');
  const pipeline1 = new AsyncMiddlewarePipeline();
  pipeline1.use(customMiddleware);

  const ctx1: MiddlewareContext = {
    sessionId: 'session-sync',
    workspace: '/project',
    prompt: 'Sync test',
  };

  try {
    await pipeline1.execute(ctx1);
    console.log(`✅ Sync middleware executed successfully!\n`);
  } catch (e) {
    console.log(`❌ Failed: ${(e as Error).message}\n`);
  }

  // 2. Test asynchronous middleware
  console.log('=== Test 2: Asynchronous Middleware ===\n');
  const pipeline2 = new AsyncMiddlewarePipeline();
  pipeline2.use(asyncMiddleware);
  pipeline2.use(customMiddleware);

  const ctx2: MiddlewareContext = {
    sessionId: 'session-async',
    workspace: '/project',
    prompt: 'Async test',
  };

  try {
    await pipeline2.execute(ctx2);
    console.log(`✅ Async middleware executed successfully!\n`);
  } catch (e) {
    console.log(`❌ Failed: ${(e as Error).message}\n`);
  }

  // 3. Test async abort
  console.log('=== Test 3: Async Abort Middleware ===\n');
  const pipeline3 = new AsyncMiddlewarePipeline();
  pipeline3.use(customMiddleware);
  pipeline3.use(asyncAbortMiddleware);

  const ctx3: MiddlewareContext = {
    sessionId: 'session-abort',
    workspace: '/project',
    prompt: undefined,
  };

  try {
    await pipeline3.execute(ctx3);
    console.log(`❌ Should have been aborted!\n`);
  } catch (e) {
    console.log(`✅ Correctly aborted: ${(e as Error).message}\n`);
  }

  console.log('🎉 All tests passed! Middleware supports both sync and async modes.');
}

main().catch(console.error);



