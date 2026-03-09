//! Built-in logging middleware

use super::context::MiddlewareContext;
use super::result::MiddlewareResult;
use super::trait_def::Middleware;
use anyhow::Result;
use async_trait::async_trait;

/// Logging middleware
///
/// Logs middleware execution events at the specified level.
pub struct LoggingMiddleware {
    level: String,
}

impl LoggingMiddleware {
    /// Create a new logging middleware
    pub fn new(level: impl Into<String>) -> Self {
        Self {
            level: level.into(),
        }
    }
}

#[async_trait]
impl Middleware for LoggingMiddleware {
    async fn handle(&self, ctx: &mut MiddlewareContext) -> Result<MiddlewareResult> {
        tracing::info!(
            level = %self.level,
            session_id = %ctx.session_id,
            workspace = ?ctx.workspace,
            has_prompt = ctx.prompt.is_some(),
            tool = ?ctx.tool_call.as_ref().map(|t| &t.name),
            "Middleware: logging"
        );

        Ok(MiddlewareResult::Continue)
    }

    fn name(&self) -> &str {
        "LoggingMiddleware"
    }
}
