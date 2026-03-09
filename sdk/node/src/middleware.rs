//! Middleware bindings for Node.js
//!
//! napi-rs bindings for the Rust Core middleware system.

use a3s_code_core::middleware::{
    LoggingMiddleware as RustLoggingMiddleware, Middleware as RustMiddleware,
    MiddlewareContext as RustMiddlewareContext, MiddlewarePipeline as RustMiddlewarePipeline,
    MiddlewareResult as RustMiddlewareResult,
};
use napi::bindgen_prelude::*;
use napi::threadsafe_function::{ErrorStrategy, ThreadsafeFunction};
use std::path::PathBuf;
use std::sync::Arc;

// ============================================================================
// MiddlewareContext
// ============================================================================

/// Middleware execution context
#[napi(object)]
#[derive(Clone)]
pub struct MiddlewareContext {
    pub session_id: String,
    pub workspace: String,
    pub prompt: Option<String>,
}

impl From<RustMiddlewareContext> for MiddlewareContext {
    fn from(ctx: RustMiddlewareContext) -> Self {
        Self {
            session_id: ctx.session_id,
            workspace: ctx.workspace.to_string_lossy().to_string(),
            prompt: ctx.prompt,
        }
    }
}

impl From<MiddlewareContext> for RustMiddlewareContext {
    fn from(ctx: MiddlewareContext) -> Self {
        RustMiddlewareContext::new(ctx.session_id, PathBuf::from(ctx.workspace))
            .with_prompt(ctx.prompt.unwrap_or_default())
    }
}

// ============================================================================
// MiddlewareResult
// ============================================================================

/// Middleware execution result
#[napi(object)]
pub struct MiddlewareResultObject {
    pub r#type: String,
    pub reason: Option<String>,
}

// ============================================================================
// Middleware Trait (JavaScript Callback)
// ============================================================================

/// JavaScript middleware wrapper
pub struct JsMiddleware {
    callback: ThreadsafeFunction<MiddlewareContext, ErrorStrategy::Fatal>,
}

impl JsMiddleware {
    pub fn new(callback: ThreadsafeFunction<MiddlewareContext, ErrorStrategy::Fatal>) -> Self {
        Self { callback }
    }
}

#[async_trait::async_trait]
impl RustMiddleware for JsMiddleware {
    async fn handle(
        &self,
        ctx: &mut RustMiddlewareContext,
    ) -> anyhow::Result<RustMiddlewareResult> {
        let js_ctx = MiddlewareContext::from(ctx.clone());

        // Call JavaScript callback
        let result = self
            .callback
            .call_async::<MiddlewareResultObject>(js_ctx)
            .await
            .map_err(|e| anyhow::anyhow!("JavaScript middleware callback failed: {}", e))?;

        // Parse result
        match result.r#type.as_str() {
            "continue" => Ok(RustMiddlewareResult::Continue),
            "abort" => Ok(RustMiddlewareResult::Abort(
                result
                    .reason
                    .unwrap_or_else(|| "Aborted by JavaScript middleware".to_string()),
            )),
            _ => Ok(RustMiddlewareResult::Continue),
        }
    }

    fn name(&self) -> &str {
        "JsMiddleware"
    }
}

// ============================================================================
// MiddlewarePipeline
// ============================================================================

/// Middleware pipeline
#[napi]
pub struct MiddlewarePipeline {
    inner: Arc<tokio::sync::Mutex<RustMiddlewarePipeline>>,
}

#[napi]
impl MiddlewarePipeline {
    #[napi(constructor)]
    pub fn new() -> Self {
        Self {
            inner: Arc::new(tokio::sync::Mutex::new(RustMiddlewarePipeline::new())),
        }
    }

    #[napi]
    pub fn use_middleware(
        &self,
        #[napi(ts_arg_type = "(ctx: MiddlewareContext) => Promise<MiddlewareResultObject>")]
        callback: JsFunction,
    ) -> Result<()> {
        let tsfn: ThreadsafeFunction<MiddlewareContext, ErrorStrategy::Fatal> = callback
            .create_threadsafe_function(0, |ctx| Ok(vec![ctx.value]))?;

        let middleware = Arc::new(JsMiddleware::new(tsfn));

        let inner = self.inner.clone();
        crate::get_runtime().block_on(async move {
            let mut pipeline = inner.lock().await;
            pipeline.use_middleware(middleware);
        });

        Ok(())
    }

    #[napi]
    pub async fn execute(&self, ctx: MiddlewareContext) -> Result<MiddlewareContext> {
        let inner = self.inner.clone();
        let mut rust_ctx = RustMiddlewareContext::from(ctx);

        let result = crate::get_runtime().block_on(async move {
            let pipeline = inner.lock().await;
            pipeline.execute(&mut rust_ctx).await?;
            Ok::<_, anyhow::Error>(rust_ctx)
        });

        match result {
            Ok(updated_ctx) => Ok(MiddlewareContext::from(updated_ctx)),
            Err(e) => Err(Error::from_reason(format!(
                "Middleware execution failed: {}",
                e
            ))),
        }
    }

    #[napi]
    pub fn len(&self) -> u32 {
        let inner = self.inner.clone();
        crate::get_runtime().block_on(async move {
            let pipeline = inner.lock().await;
            pipeline.len() as u32
        })
    }
}

// ============================================================================
// Built-in Middleware
// ============================================================================

/// Logging middleware
#[napi]
pub struct LoggingMiddleware {
    inner: Arc<RustLoggingMiddleware>,
}

#[napi]
impl LoggingMiddleware {
    #[napi(constructor)]
    pub fn new(level: String) -> Self {
        Self {
            inner: Arc::new(RustLoggingMiddleware::new(level)),
        }
    }
}
