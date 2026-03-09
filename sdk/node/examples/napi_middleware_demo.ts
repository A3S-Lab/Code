/**
 * Example: Using napi-rs middleware bindings
 *
 * This example demonstrates the napi-rs bindings for the Rust Core middleware system.
 */

import { MiddlewareContext, MiddlewarePipeline, MiddlewareResultObject } from '..';

// Define custom TypeScript middleware (synchronous return)
function customMiddleware(ctx: MiddlewareContext): MiddlewareResultObject {
  console.log(`🟢 TypeScript middleware: sessionId=${ctx.sessionId}, workspace=${ctx.workspace}`);
  return { resultType: 'continue' };
}

function timingMiddleware(ctx: MiddlewareContext): MiddlewareResultObject {
  const start = Date.now();
  console.log(`⏱️  Timing middleware: started at ${start}`);
  return { resultType: 'continue' };
}

function abortMiddleware(ctx: MiddlewareContext): MiddlewareResultObject {
  console.log(`🚫 Abort middleware: aborting execution`);
  return { resultType: 'abort', reason: 'Aborted by TypeScript middleware' };
}

async function main() {
  console.log('🚀 napi-rs Middleware Bindings Example (TypeScript)\n');

  // 1. Create middleware pipeline
  const pipeline = new MiddlewarePipeline();
  console.log(`✅ Created pipeline\n`);

  // 2. Register middleware
  console.log('📝 Registering middleware...');

  // Register TypeScript middleware
  pipeline.useMiddleware(customMiddleware);
  pipeline.useMiddleware(timingMiddleware);

  console.log(`✅ Registered ${pipeline.len()} middleware\n`);

  // 3. Create context
  console.log('🔧 Creating context...');
  const ctx: MiddlewareContext = {
    sessionId: 'session-123',
    workspace: '/project',
    prompt: 'List all files',
  };
  console.log(`   Context:`, ctx, '\n');

  // 4. Execute pipeline
  console.log('🎬 Executing pipeline...\n');
  try {
    const resultCtx = await pipeline.execute(ctx);
    console.log(`\n✅ Pipeline executed successfully!`);
    console.log(`   Result context:`, resultCtx);
  } catch (e) {
    console.log(`\n❌ Pipeline execution failed: ${(e as Error).message}`);
  }

  // 5. Test abort
  console.log('\n\n🚫 Testing abort middleware...');
  const pipeline2 = new MiddlewarePipeline();
  pipeline2.useMiddleware(customMiddleware);
  pipeline2.useMiddleware(abortMiddleware);

  const ctx2: MiddlewareContext = {
    sessionId: 'session-456',
    workspace: '/project2',
    prompt: undefined,
  };

  try {
    const resultCtx2 = await pipeline2.execute(ctx2);
    console.log(`❌ Should have been aborted!`);
  } catch (e) {
    console.log(`✅ Correctly aborted: ${(e as Error).message}`);
  }

  console.log('\n🎉 napi-rs middleware bindings working correctly!');
}

main().catch(console.error);
