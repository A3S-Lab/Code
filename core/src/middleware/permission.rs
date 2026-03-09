//! Built-in permission middleware

use super::context::MiddlewareContext;
use super::result::MiddlewareResult;
use super::trait_def::Middleware;
use crate::permissions::{PermissionChecker, PermissionDecision};
use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;

/// Permission middleware
///
/// Wraps a PermissionChecker to enforce tool execution permissions
/// in the middleware pipeline.
pub struct PermissionMiddleware {
    checker: Arc<dyn PermissionChecker>,
}

impl PermissionMiddleware {
    /// Create a new permission middleware
    pub fn new(checker: Arc<dyn PermissionChecker>) -> Self {
        Self { checker }
    }
}

#[async_trait]
impl Middleware for PermissionMiddleware {
    async fn handle(&self, ctx: &mut MiddlewareContext) -> Result<MiddlewareResult> {
        // Only check permissions for tool calls
        if let Some(ref tool_call) = ctx.tool_call {
            let decision = self.checker.check(&tool_call.name, &tool_call.args);

            match decision {
                PermissionDecision::Allow => {
                    // Continue to next middleware
                    Ok(MiddlewareResult::Continue)
                }
                PermissionDecision::Deny => {
                    // Abort the pipeline
                    Ok(MiddlewareResult::Abort(format!(
                        "Permission denied for tool '{}'",
                        tool_call.name
                    )))
                }
                PermissionDecision::Ask => {
                    // Mark that confirmation is required
                    ctx.set_metadata(
                        "requires_confirmation".into(),
                        serde_json::Value::Bool(true),
                    );
                    Ok(MiddlewareResult::Continue)
                }
            }
        } else {
            // No tool call, continue
            Ok(MiddlewareResult::Continue)
        }
    }

    fn name(&self) -> &str {
        "PermissionMiddleware"
    }
}
