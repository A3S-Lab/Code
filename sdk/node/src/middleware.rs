//! Middleware bindings for Node.js
//!
//! napi-rs bindings for the Rust Core middleware system.

use a3s_code_core::middleware::{
    LoggingMiddleware as RustLoggingMiddleware, Middleware as RustMiddleware,
    MiddlewareContext as RustMiddlewareContext, MiddlewarePipeline as RustMiddlewarePipeline,
    MiddlewareResult as RustMiddlewareResult,
};
use napi::bindgen_prelude::*;
use napi::threadsafe_function::{ErrorStrategy, ThreadsafeFunction, ThreadsafeFunctionCallMode};
use napi::{JsUnknown, NapiRaw};
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
    pub result_type: String,
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

        // Call JavaScript callback and wait for result
        let (tx, rx) = std::sync::mpsc::sync_channel(1);

        self.callback.call_with_return_value(
            js_ctx,
            ThreadsafeFunctionCallMode::NonBlocking,
            move |ret: JsUnknown| {
                // Parse the return value (handles both sync returns and resolved promises)
                let result = parse_middleware_result(ret);
                let _ = tx.send(result);
                Ok(())
            },
        );

        // Wait for the result with a timeout (use spawn_blocking since recv is blocking)
        let result = tokio::task::spawn_blocking(move || {
            rx.recv_timeout(std::time::Duration::from_secs(30))
                .map_err(|_| anyhow::anyhow!("JavaScript middleware callback timed out"))
        })
        .await
        .map_err(|_| anyhow::anyhow!("JavaScript middleware callback task panicked"))??;

        result
    }

    fn name(&self) -> &str {
        "JsMiddleware"
    }
}

/// Parse the return value from a JS middleware callback
fn parse_middleware_result(ret: JsUnknown) -> anyhow::Result<RustMiddlewareResult> {
    // Try to get the object
    let obj = ret.coerce_to_object()?;

    // Get the resultType field
    let result_type: String = obj.get_named_property("resultType")?;

    match result_type.as_str() {
        "continue" => Ok(RustMiddlewareResult::Continue),
        "abort" => {
            let reason: Option<String> = obj.get_named_property("reason").ok();
            Ok(RustMiddlewareResult::Abort(
                reason.unwrap_or_else(|| "Aborted by JavaScript middleware".to_string()),
            ))
        }
        _ => Ok(RustMiddlewareResult::Continue),
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
        #[napi(ts_arg_type = "(ctx: MiddlewareContext) => MiddlewareResultObject")]
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

        let pipeline = inner.lock().await;
        pipeline
            .execute(&mut rust_ctx)
            .await
            .map_err(|e| Error::from_reason(format!("Middleware execution failed: {}", e)))?;

        Ok(MiddlewareContext::from(rust_ctx))
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
