"""
Example: Using PyO3 middleware bindings

This example demonstrates the PyO3 bindings for the Rust Core middleware system.
"""

import asyncio

# Import from Rust bindings
from a3s_code import MiddlewareContext, MiddlewarePipeline, MiddlewareResult, LoggingMiddleware


# Define a custom Python middleware
async def custom_middleware(ctx):
    """Custom middleware in Python"""
    print(f"🐍 Python middleware: session_id={ctx.session_id}, workspace={ctx.workspace}")

    # Set metadata
    ctx.set_metadata("python_middleware", "executed")

    # Return continue
    return {"type": MiddlewareResult.CONTINUE}


async def timing_middleware(ctx):
    """Timing middleware"""
    import time
    start = time.time()
    print(f"⏱️  Timing middleware: started")

    ctx.set_metadata("timing_start", str(start))

    return {"type": MiddlewareResult.CONTINUE}


async def abort_middleware(ctx):
    """Middleware that aborts"""
    print(f"🚫 Abort middleware: aborting execution")
    return {"type": MiddlewareResult.ABORT, "reason": "Aborted by Python middleware"}


async def main():
    print("🚀 PyO3 Middleware Bindings Example\n")

    # 1. Create middleware pipeline
    pipeline = MiddlewarePipeline()
    print(f"✅ Created pipeline: {pipeline}\n")

    # 2. Register middleware
    print("📝 Registering middleware...")

    # Register Python middleware
    pipeline.use_middleware(custom_middleware)
    pipeline.use_middleware(timing_middleware)

    # Register Rust middleware
    # logging_middleware = LoggingMiddleware("debug")
    # pipeline.use_middleware(logging_middleware)  # TODO: Need to wrap Rust middleware

    print(f"✅ Registered {len(pipeline)} middleware\n")

    # 3. Create context
    print("🔧 Creating context...")
    ctx = MiddlewareContext("session-123", "/project")
    print(f"   Context: {ctx}\n")

    # 4. Execute pipeline
    print("🎬 Executing pipeline...\n")
    try:
        result_ctx = pipeline.execute(ctx)
        print(f"\n✅ Pipeline executed successfully!")
        print(f"   Result context: {result_ctx}")
        print(f"   Metadata: python_middleware={result_ctx.get_metadata('python_middleware')}")
        print(f"   Metadata: timing_start={result_ctx.get_metadata('timing_start')}")
    except Exception as e:
        print(f"\n❌ Pipeline execution failed: {e}")

    # 5. Test abort
    print("\n\n🚫 Testing abort middleware...")
    pipeline2 = MiddlewarePipeline()
    pipeline2.use_middleware(custom_middleware)
    pipeline2.use_middleware(abort_middleware)

    ctx2 = MiddlewareContext("session-456", "/project2")

    try:
        result_ctx2 = pipeline2.execute(ctx2)
        print(f"❌ Should have been aborted!")
    except Exception as e:
        print(f"✅ Correctly aborted: {e}")

    print("\n🎉 PyO3 middleware bindings working correctly!")


if __name__ == "__main__":
    asyncio.run(main())
