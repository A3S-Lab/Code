//! Example: Using the middleware system
//!
//! This example demonstrates how to use the Express-like middleware
//! pipeline in A3S Code.

use a3s_code_core::middleware::{
    LoggingMiddleware, Middleware, MiddlewareContext, MiddlewarePipeline, MiddlewareResult,
    PermissionMiddleware, SecurityMiddleware, ToolCallInfo,
};
use a3s_code_core::permissions::{PermissionPolicy, PermissionRule};
use a3s_code_core::security::NoOpSecurityProvider;
use anyhow::Result;
use async_trait::async_trait;
use std::path::PathBuf;
use std::sync::Arc;

// Custom middleware example
struct TimingMiddleware;

#[async_trait]
impl Middleware for TimingMiddleware {
    async fn handle(&self, ctx: &mut MiddlewareContext) -> Result<MiddlewareResult> {
        let start = std::time::Instant::now();
        println!("⏱️  Timing middleware: started");

        // Store start time in metadata
        ctx.set_metadata(
            "timing_start".into(),
            serde_json::json!(start.elapsed().as_millis()),
        );

        Ok(MiddlewareResult::Continue)
    }

    fn name(&self) -> &str {
        "TimingMiddleware"
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    println!("🚀 A3S Code Middleware System Example\n");

    // 1. Create middleware pipeline
    let mut pipeline = MiddlewarePipeline::new();

    // 2. Register middleware (Express-like app.use())
    pipeline.use_middleware(Arc::new(LoggingMiddleware::new("debug")));
    pipeline.use_middleware(Arc::new(TimingMiddleware));
    pipeline.use_middleware(Arc::new(SecurityMiddleware::new(Arc::new(
        NoOpSecurityProvider,
    ))));

    // 3. Create permission policy
    let mut policy = PermissionPolicy::default();
    policy.allow.push(PermissionRule::new("Read"));
    policy.allow.push(PermissionRule::new("Grep"));
    policy.deny.push(PermissionRule::new("Bash(rm -rf *)"));

    pipeline.use_middleware(Arc::new(PermissionMiddleware::new(Arc::new(policy))));

    println!("✅ Registered {} middleware\n", pipeline.len());

    // 4. Test with a prompt context
    println!("📝 Test 1: Prompt context");
    let mut ctx = MiddlewareContext::new("session-123".into(), PathBuf::from("/project"));
    ctx = ctx.with_prompt("List all files".into());

    pipeline.execute(&mut ctx).await?;
    println!("✅ Prompt context passed through middleware\n");

    // 5. Test with an allowed tool call
    println!("🔧 Test 2: Allowed tool call (Read)");
    let mut ctx = MiddlewareContext::new("session-123".into(), PathBuf::from("/project"));
    ctx = ctx.with_tool_call(ToolCallInfo {
        id: "tool-1".into(),
        name: "Read".into(),
        args: serde_json::json!({"file_path": "/project/README.md"}),
    });

    pipeline.execute(&mut ctx).await?;
    println!("✅ Allowed tool call passed through middleware\n");

    // 6. Test with a denied tool call
    println!("🚫 Test 3: Denied tool call (Bash rm -rf)");
    let mut ctx = MiddlewareContext::new("session-123".into(), PathBuf::from("/project"));
    ctx = ctx.with_tool_call(ToolCallInfo {
        id: "tool-2".into(),
        name: "Bash".into(),
        args: serde_json::json!({"command": "rm -rf *"}),
    });

    match pipeline.execute(&mut ctx).await {
        Ok(_) => println!("❌ Should have been denied!"),
        Err(e) => println!("✅ Correctly denied: {}\n", e),
    }

    println!("🎉 Middleware system working correctly!");

    Ok(())
}
