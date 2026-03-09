//! Built-in security middleware

use super::context::MiddlewareContext;
use super::result::MiddlewareResult;
use super::trait_def::Middleware;
use crate::security::SecurityProvider;
use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;

/// Security middleware
///
/// Wraps a SecurityProvider to perform input taint checking and
/// output sanitization in the middleware pipeline.
pub struct SecurityMiddleware {
    provider: Arc<dyn SecurityProvider>,
}

impl SecurityMiddleware {
    /// Create a new security middleware
    pub fn new(provider: Arc<dyn SecurityProvider>) -> Self {
        Self { provider }
    }
}

#[async_trait]
impl Middleware for SecurityMiddleware {
    async fn handle(&self, ctx: &mut MiddlewareContext) -> Result<MiddlewareResult> {
        // Taint input if prompt is present
        if let Some(ref prompt) = ctx.prompt {
            self.provider.taint_input(prompt);
        }

        // Taint tool arguments if tool call is present
        if let Some(ref tool_call) = ctx.tool_call {
            let args_str = serde_json::to_string(&tool_call.args)?;
            self.provider.taint_input(&args_str);
        }

        Ok(MiddlewareResult::Continue)
    }

    fn name(&self) -> &str {
        "SecurityMiddleware"
    }
}
