/**
 * Example: Using napi-rs middleware bindings
 *
 * This example demonstrates the napi-rs bindings for the Rust Core middleware system.
 */

const { MiddlewareContext, MiddlewarePipeline, LoggingMiddleware } = require('..');

// Define custom JavaScript middleware
async function customMiddleware(ctx) {
  console.log(`🟢 JavaScript middleware: session_id=${ctx.session_id}, workspace=${ctx.workspace}`);
  return { type: 'continue' };
}

async function timingMiddleware(ctx) {
  const start = Date.now();
  console.log(`⏱️  Timing middleware: started`);
  return { type: 'continue' };
}

async function abortMiddleware(ctx) {
  console.log(`🚫 Abort middleware: aborting execution`);
  return { type: 'abort', reason: 'Aborted by JavaScript middleware' };
}

async function main() {
  console.log('🚀 napi-rs Middleware Bindings Example\n');

  // 1. Create middleware pipeline
  const pipeline = new MiddlewarePipeline();
  console.log(`✅ Created pipeline\n`);

  // 2. Register middleware
  console.log('📝 Registering middleware...');

  // Register JavaScript middleware
  pipeline.useMiddleware(customMiddleware);
  pipeline.useMiddleware(timingMiddleware);

  console.log(`✅ Registered ${pipeline.len()} middleware\n`);

  // 3. Create context
  console.log('🔧 Creating context...');
  const ctx = {
    session_id: 'session-123',
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
    console.log(`\n❌ Pipeline execution failed: ${e.message}`);
  }

  // 5. Test abort
  console.log('\n\n🚫 Testing abort middleware...');
  const pipeline2 = new MiddlewarePipeline();
  pipeline2.useMiddleware(customMiddleware);
  pipeline2.useMiddleware(abortMiddleware);

  const ctx2 = {
    session_id: 'session-456',
    workspace: '/project2',
    prompt: null,
  };

  try {
    const resultCtx2 = await pipeline2.execute(ctx2);
    console.log(`❌ Should have been aborted!`);
  } catch (e) {
    console.log(`✅ Correctly aborted: ${e.message}`);
  }

  console.log('\n🎉 napi-rs middleware bindings working correctly!');
}

main().catch(console.error);
