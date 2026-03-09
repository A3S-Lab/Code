//! Middleware execution result types

use super::context::MiddlewareContext;

/// Result of middleware execution
///
/// Middleware can return one of three outcomes:
/// - Continue: proceed to the next middleware
/// - Modified: update the context and continue
/// - Abort: stop the pipeline with an error
#[derive(Debug)]
pub enum MiddlewareResult {
    /// Continue to the next middleware without changes
    Continue,

    /// Modify the context and continue to the next middleware
    Modified(MiddlewareContext),

    /// Abort the pipeline with an error message
    Abort(String),
}

impl MiddlewareResult {
    /// Check if the result is Continue
    pub fn is_continue(&self) -> bool {
        matches!(self, MiddlewareResult::Continue)
    }

    /// Check if the result is Modified
    pub fn is_modified(&self) -> bool {
        matches!(self, MiddlewareResult::Modified(_))
    }

    /// Check if the result is Abort
    pub fn is_abort(&self) -> bool {
        matches!(self, MiddlewareResult::Abort(_))
    }
}
