//! Middleware pipeline
//!
//! The pipeline executes middleware in sequence, similar to Express's
//! middleware chain.

use super::context::MiddlewareContext;
use super::result::MiddlewareResult;
use super::trait_def::Middleware;
use anyhow::Result;
use std::sync::Arc;

/// Middleware pipeline
///
/// Executes middleware in the order they were registered.
/// Similar to Express's `app.use()` chain.
#[derive(Clone)]
pub struct MiddlewarePipeline {
    middlewares: Vec<Arc<dyn Middleware>>,
}

impl MiddlewarePipeline {
    /// Create a new empty pipeline
    pub fn new() -> Self {
        Self {
            middlewares: Vec::new(),
        }
    }

    /// Register a middleware (similar to Express's `app.use()`)
    pub fn use_middleware(&mut self, middleware: Arc<dyn Middleware>) {
        self.middlewares.push(middleware);
    }

    /// Execute the middleware pipeline
    ///
    /// Runs all middleware in sequence until one returns Abort or all complete.
    pub async fn execute(&self, ctx: &mut MiddlewareContext) -> Result<()> {
        for middleware in &self.middlewares {
            match middleware.handle(ctx).await? {
                MiddlewareResult::Continue => {
                    // Continue to next middleware
                    continue;
                }
                MiddlewareResult::Modified(new_ctx) => {
                    // Update context and continue
                    *ctx = new_ctx;
                    continue;
                }
                MiddlewareResult::Abort(reason) => {
                    // Stop pipeline
                    return Err(anyhow::anyhow!("Middleware '{}' aborted: {}", middleware.name(), reason));
                }
            }
        }
        Ok(())
    }

    /// Get the number of registered middleware
    pub fn len(&self) -> usize {
        self.middlewares.len()
    }

    /// Check if the pipeline is empty
    pub fn is_empty(&self) -> bool {
        self.middlewares.is_empty()
    }
}

impl Default for MiddlewarePipeline {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for MiddlewarePipeline {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MiddlewarePipeline")
            .field("middleware_count", &self.middlewares.len())
            .finish()
    }
}
