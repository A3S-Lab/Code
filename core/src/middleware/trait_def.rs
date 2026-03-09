//! Middleware trait definition
//!
//! The core trait for implementing middleware in the A3S Code framework.
//! Similar to Express middleware: `(req, res, next) => void`

use super::context::MiddlewareContext;
use super::result::MiddlewareResult;
use anyhow::Result;
use async_trait::async_trait;

/// Middleware trait
///
/// Implement this trait to create custom middleware that can intercept
/// and modify the execution flow of agent sessions.
///
/// # Example
///
/// ```rust
/// use a3s_code_core::middleware::{Middleware, MiddlewareContext, MiddlewareResult};
/// use async_trait::async_trait;
/// use anyhow::Result;
///
/// struct LoggingMiddleware;
///
/// #[async_trait]
/// impl Middleware for LoggingMiddleware {
///     async fn handle(&self, ctx: &mut MiddlewareContext) -> Result<MiddlewareResult> {
///         println!("Session: {}", ctx.session_id);
///         Ok(MiddlewareResult::Continue)
///     }
/// }
/// ```
#[async_trait]
pub trait Middleware: Send + Sync {
    /// Handle the middleware execution
    ///
    /// This method is called when the middleware is executed in the pipeline.
    /// It receives a mutable reference to the context and can:
    /// - Read/modify the context
    /// - Return Continue to proceed
    /// - Return Modified with a new context
    /// - Return Abort to stop the pipeline
    async fn handle(&self, ctx: &mut MiddlewareContext) -> Result<MiddlewareResult>;

    /// Get the middleware name (for debugging/logging)
    fn name(&self) -> &str {
        std::any::type_name::<Self>()
    }
}
